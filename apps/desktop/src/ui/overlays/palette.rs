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
            // 已登记脚本从 prefs 现取一次：它们是用户数据，纯层不认识。
            // 读盘失败退回空清单，面板照常可用（只是少一组脚本条目）。
            let scripts: Vec<(String, String)> = term_profile::prefs::load()
                .map(|p| {
                    p.scripts
                        .into_iter()
                        .filter(|e| e.enabled)
                        .map(|e| (e.name, e.path))
                        .collect()
                })
                .unwrap_or_default();
            draw_panel(ctx, state, input, self.view.lang, &scripts, &mut dispatch);
        }
        // 面板画完再统一 reduce，避免绘制期间持有窗口的只读借用。
        for action in actions {
            self.dispatch_action(action);
        }
    }
}

/// 当前选中的「已登记脚本」路径（`Script.RunFile:` 那条 Action 的载荷）。
/// 用 `script_run` 的同一个解析器，不在浮层里自己拼命令 ID。
fn selected_script(state: &PaletteState, scripts: &[(String, String)]) -> Option<String> {
    let entry = palette::selected_entry_with_scripts(state, scripts)?;
    crate::script_run::run_file_target(&entry.id).map(str::to_string)
}

/// 鼠标点中的那一行的脚本路径（下标指**带脚本**的过滤结果）。
fn script_at(
    state: &PaletteState,
    scripts: &[(String, String)],
    idx: usize,
) -> Option<String> {
    let list = palette::filtered_with_scripts(state, scripts);
    let entry = list.get(idx)?;
    crate::script_run::run_file_target(&entry.id).map(str::to_string)
}

fn draw_panel(
    ctx: &egui::Context,
    state: &PaletteState,
    input: &mut String,
    lang: Lang,
    scripts: &[(String, String)],
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
    let list = palette::filtered_with_scripts(state, scripts);
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
        if calc.is_some() {
            if let Some(Ok(value)) = &calc {
                ctx.copy_text(value.clone());
            }
            dispatch(Action::Palette(PaletteAction::Close));
        } else {
            // 非计算器：Enter 是「执行选中项」，不是单纯关窗。Confirm 与鼠标
            // 点击走同一条 Action 通道，由顶层 reduce 把选中命令再派发一次，
            // 所以「敲回车」和「点那一行」产生的是同一个 Effect。
            // 已登记脚本的路径是用户数据，reducer 的静态清单不认识它；脚本行
            // 直接发 `Action::RunScript`，与按钮栏 / 映射键共用同一条 Action
            // 通道。这样也不会让静态清单里「同下标」的另一条命令被误执行。
            if let Some(path) = selected_script(state, scripts) {
                dispatch(Action::RunScript(path));
            } else {
                dispatch(Action::Palette(PaletteAction::Confirm));
            }
        }
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
                    // 脚本名是用户数据（owned String），不是可翻译的静态文案。
                    let label = if entry.key.is_empty() {
                        entry.label.clone()
                    } else {
                        tr(entry.key).to_string()
                    };
                    let text = format!("{} › {}", tr(entry.menu), label);
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
                        // 脚本行走自己的 Action（见 Enter 分支的理由），其余行
                        // 两条动作同一帧顺序派发：先写选中项，再确认执行。
                        if let Some(path) = script_at(state, scripts, idx) {
                            dispatch(Action::RunScript(path));
                        } else {
                            dispatch(Action::Palette(PaletteAction::Select(idx)));
                            dispatch(Action::Palette(PaletteAction::Confirm));
                        }
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

    /// Enter 是「执行选中项」而不是「关窗」：它派发 `Confirm`，顶层 reduce 把
    /// 选中命令再走一次 `Action::parse` → reducer → `Effect`。这条断言从浮层
    /// 出发，一直钉到有副作用产出，防止有人把 Enter 悄悄改回 Close。
    #[test]
    fn enter_dispatches_confirm_and_produces_an_effect() {
        let ctx = egui::Context::default();
        let mut state = PaletteState::default();
        term_app::palette::reduce(&mut state, PaletteAction::Open(PaletteMode::Command));
        term_app::palette::reduce(&mut state, PaletteAction::SetQuery("Zoom In".into()));
        let mut input = String::new();
        let mut actions: Vec<Action> = Vec::new();
        let press = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: Some(egui::Key::Enter),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        };
        let _ = ctx.run(
            egui::RawInput {
                events: vec![press],
                ..Default::default()
            },
            |ctx| draw_panel(ctx, &state, &mut input, Lang::En, &[], &mut |a| actions.push(a)),
        );
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::Palette(PaletteAction::Confirm))),
            "Enter 必须派发 Confirm（不是 Close）：{actions:?}"
        );
        let mut view = term_app::state::ViewState::default();
        view.command_palette = state.clone();
        let effects: Vec<_> = actions
            .into_iter()
            .flat_map(|a| term_app::reduce::reduce(&mut view, a, None))
            .collect();
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, term_app::effect::Effect::Zoom(1))),
            "Confirm 必须真的产生 Effect：{effects:?}"
        );
    }

    /// 已登记脚本行与菜单项走同一条 Action 通道，但不经过静态下标的 Confirm：
    /// 否则「同下标的另一条静态命令」会被一起执行。
    #[test]
    fn enter_on_a_registered_script_dispatches_run_script() {
        let ctx = egui::Context::default();
        let mut state = PaletteState::default();
        term_app::palette::reduce(&mut state, PaletteAction::Open(PaletteMode::Command));
        term_app::palette::reduce(&mut state, PaletteAction::SetQuery("备份".into()));
        let scripts = vec![("备份".to_string(), "/tmp/backup.rhai".to_string())];
        let mut input = String::new();
        let mut actions: Vec<Action> = Vec::new();
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: Some(egui::Key::Enter),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        };
        let _ = ctx.run(
            egui::RawInput {
                events: vec![enter],
                ..Default::default()
            },
            |ctx| draw_panel(ctx, &state, &mut input, Lang::En, &scripts, &mut |a| actions.push(a)),
        );
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::RunScript(p) if p == "/tmp/backup.rhai")),
            "脚本行必须派发带路径的 Action：{actions:?}"
        );
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, Action::Palette(PaletteAction::Confirm))),
            "脚本不能走静态下标的 Confirm：{actions:?}"
        );
    }
}
