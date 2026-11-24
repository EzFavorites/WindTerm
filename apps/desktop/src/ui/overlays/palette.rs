//! 命令面板的绘制——只读状态、跨帧输入缓冲、动作统一出队。
//!
//! 与 Find 栏同一套受控契约：`ViewState.command_palette` 只读，正在敲的字
//! 落在投影层的 `palette_input` 缓冲里，改动以 `SetQuery` 派发回 reducer；
//! 条目与过滤结果由 `app::palette` 从菜单树 + 查询词现算，本文件不算第二遍。
//!
//! 为什么执行也要出队：面板上的 Enter / 点击只产出 `Action`，真正「关掉面板并
//! 执行选中命令」由顶层 `reduce` 递归派发完成。投影层因此没有第二个状态写入者，
//! 也没有绕过 `Action::parse` 的字符串派发旁路。

use crate::ui::harness::AnchoredWindow;
use crate::ui::*;
use term_app::i18n::Lang;
use term_app::palette::{self, PaletteAction, PaletteMode, PaletteState};

/// 一屏最多画几条：浮层不该盖住整个终端。
const VISIBLE_ROWS: usize = 10;

/// 计算器补全用的当前标识符片段：取表达式最后一段字母/数字/下划线。
/// 返回 `(片段之前的文本, 片段)`，点击补全时把片段换成候选名即可。
fn calc_fragment(input: &str) -> (&str, &str) {
    let expr = term_app::calculator::expression(input);
    let start = expr
        .char_indices()
        .rev()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '$')))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    (&expr[..start], &expr[start..])
}

/// 结果集比一屏长时，围绕选中项算窗口起点（纯函数，单独可测）。
fn window_start(total: usize, selected: Option<usize>, visible: usize) -> usize {
    if total <= visible || visible == 0 {
        return 0;
    }
    let focus = selected.unwrap_or(0).min(total - 1);
    focus.saturating_sub(visible / 2).min(total - visible)
}

impl TerminalWindow {
    /// 面板浮层。默认关闭时一行都不画——五张视觉基线因此不受影响。
    pub(in crate::ui) fn draw_command_palette(&mut self, ctx: &egui::Context) {
        if !self.view.command_palette.visible {
            return;
        }
        let mut actions: Vec<Action> = Vec::new();
        {
            // 状态与缓冲是两个不相交的字段，借用互不干扰。
            let state = &self.view.command_palette;
            let input = &mut self.palette_input;
            let mut dispatch = |action: Action| actions.push(action);
            draw_panel(ctx, state, input, self.view.lang, &mut dispatch);
        }
        // 面板画完再统一 reduce，避免绘制期间持有窗口的只读借用。
        for action in actions {
            self.dispatch_action(action);
        }
    }
}

fn draw_panel(
    ctx: &egui::Context,
    state: &PaletteState,
    input: &mut String,
    lang: Lang,
    dispatch: &mut dyn FnMut(Action),
) {
    let tr = |en: &'static str| t(lang, en);
    // 只有外部改了 query（打开 / 关闭 / 菜单派发）才覆盖缓冲；用户正在敲的字
    // 已经在缓冲里，每帧覆盖会把光标与输入法预编辑一起打断。
    if input != &state.query {
        input.clear();
        input.push_str(&state.query);
    }

    // 官网 page/24：查询以 `=` 开头时面板变计算器。求值是纯函数，
    // 每帧现算、不缓存；这里只负责显示结果与补全，不另建第二份状态。
    let calc = if term_app::calculator::is_calculator_query(input) {
        Some(term_app::calculator::evaluate(term_app::calculator::expression(input)))
    } else {
        None
    };
    let list = palette::filtered(state);
    let selected = if list.is_empty() {
        None
    } else {
        Some(state.selected % list.len())
    };

    // 键盘在浮层里统一解释。面板打开时终端区有 guard，不会把同一批键再发给 PTY。
    let (up, down, enter, escape) = ctx.input(|i| {
        (
            i.key_pressed(egui::Key::ArrowUp),
            i.key_pressed(egui::Key::ArrowDown),
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });
    if escape {
        dispatch(Action::Palette(PaletteAction::Close));
        return;
    }
    if up {
        dispatch(Action::Palette(PaletteAction::Move(-1)));
    }
    if down {
        dispatch(Action::Palette(PaletteAction::Move(1)));
    }
    if enter {
        // 计算器模式的 Enter 把结果写进系统剪贴板并关窗——不碰 PTY，
        // 也不假装「自动粘到终端」：是否写入由用户下一次粘贴决定。
        if let Some(Ok(value)) = &calc {
            ctx.copy_text(value.clone());
        }
        dispatch(Action::Palette(PaletteAction::Close));
        return;
    }

    let (title, hint) = if calc.is_some() {
        ("Calculator", "Enter a formula, e.g. =sin(PI/2)")
    } else {
        match state.mode {
        PaletteMode::Command => (tr("Command Palette"), tr("Search commands")),
        PaletteMode::Anything => (tr("Palette Anything"), tr("Search all menu items")),
        }
    };

    // 定宽不定高：结果集 0 项与 10 项不该是同一个高度。窗口外壳走 harness，
    // 稳定 id 用 `palette`（标题随模式变）。
    AnchoredWindow::new("palette", title, 440.0).show(ctx, |ui| {
            let response = ui.add(
                egui::TextEdit::singleline(input)
                    .hint_text(hint)
                    .desired_width(400.0)
                    .min_size(egui::vec2(400.0, 0.0)),
            );
            // 打开期间输入框始终持有焦点，敲字即搜索；焦点已是它时不会触发重绘。
            if !response.has_focus() {
                response.request_focus();
            }
            if response.changed() {
                dispatch(Action::Palette(PaletteAction::SetQuery(input.clone())));
            }
            ui.separator();

            if let Some(result) = &calc {
                // 结果显示与官方 GIF 同构：成功给值，失败给 `Invalid formula: …`。
                // `Copy result` 是真按钮，Enter 也复制；不写 PTY、不造终端输入。
                match result {
                    Ok(value) => {
                        ui.colored_label(
                            egui::Color32::from_rgb(0x8D, 0xD3, 0x8D),
                            format!("= {value}"),
                        );
                        if ui.button("Copy result").clicked() {
                            ctx.copy_text(value.clone());
                        }
                    }
                    Err(error) => {
                        ui.colored_label(
                            egui::Color32::from_rgb(0xE0, 0x6C, 0x6C),
                            error.to_string(),
                        );
                    }
                }
                ui.separator();
                // 先拷贝成 owned：下面点击补全要写 input，不能再借着它不放。
                let (prefix, fragment) = {
                    let (p, f) = calc_fragment(input);
                    (p.to_string(), f.to_string())
                };
                let lower = fragment.to_ascii_lowercase();
                let mut shown = 0;
                for name in term_app::calculator::suggestions() {
                    if shown >= 10 {
                        break;
                    }
                    if !name.to_ascii_lowercase().starts_with(&lower) {
                        continue;
                    }
                    shown += 1;
                    let label = if term_app::calculator::is_function(name) {
                        format!("{name}(…)")
                    } else {
                        (*name).to_string()
                    };
                    let note = term_app::calculator::description(name).unwrap_or("");
                    if ui.button(format!("{label}  {note}")).clicked() {
                        input.clear();
                        input.push('=');
                        input.push_str(&prefix);
                        input.push_str(name);
                        if term_app::calculator::is_function(name) {
                            input.push('(');
                        }
                        dispatch(Action::Palette(PaletteAction::SetQuery(input.clone())));
                    }
                }
            } else if list.is_empty() {
                ui.label(tr("No matches"));
            } else {
                let start = window_start(list.len(), selected, VISIBLE_ROWS);
                for (offset, entry) in list.iter().enumerate().skip(start).take(VISIBLE_ROWS) {
                    let idx = start + offset;
                    let text = format!("{} › {}", tr(entry.menu), tr(entry.label));
                    let row = ui.add_enabled(
                        entry.implemented,
                        egui::SelectableLabel::new(Some(idx) == selected, text),
                    );
                    // `on_disabled_hover_text` 会消费 Response，得把返回值接回来。
                    let row = if entry.implemented {
                        row
                    } else {
                        row.on_disabled_hover_text(tr("Not wired (placeholder)"))
                    };
                    if row.clicked() {
                        // 两条动作同一帧顺序派发：先由 reducer 写选中项，再确认执行。
                        dispatch(Action::Palette(PaletteAction::Select(idx)));
                        dispatch(Action::Palette(PaletteAction::Confirm));
                    }
                }
                if list.len() > VISIBLE_ROWS {
                    ui.label(
                        tr("{n} items, ↑↓ to scroll")
                            .replace("{n}", &list.len().to_string()),
                    );
                }
            }

            ui.separator();
            ui.label(tr("↑↓ Select · Enter Run · Esc Close"));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculator_fragment_splits_the_prefix_from_the_current_name() {
        assert_eq!(calc_fragment("=sin(PI/2)"), ("sin(PI/2)", ""));
        assert_eq!(calc_fragment("=sqrt(PI"), ("sqrt(", "PI"));
        assert_eq!(calc_fragment("=max(1,2"), ("max(1,", "2"));
    }

    #[test]
    fn short_result_sets_start_at_the_first_row() {
        assert_eq!(window_start(3, Some(2), VISIBLE_ROWS), 0);
        assert_eq!(window_start(0, None, VISIBLE_ROWS), 0);
    }

    #[test]
    fn long_result_sets_keep_the_selection_in_view() {
        let start = window_start(40, Some(30), VISIBLE_ROWS);
        assert!(start <= 30 && 30 < start + VISIBLE_ROWS);
        assert!(start + VISIBLE_ROWS <= 40);
    }

    #[test]
    fn selection_never_pushes_the_window_past_the_end() {
        assert_eq!(window_start(40, Some(39), VISIBLE_ROWS), 30);
        assert_eq!(window_start(40, None, VISIBLE_ROWS), 0);
    }
}
