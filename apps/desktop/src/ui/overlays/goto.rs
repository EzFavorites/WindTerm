//! Goto 面板的绘制——跳到回看绝对行 / 切到某个已打开的会话。
//!
//! 与 Find 栏、命令面板同一套受控契约：`ViewState.goto` 只读，正在敲的字
//! 落在投影层的 `goto_input` 缓冲里，改动以 `SetInput` 派发回 reducer；解析是否
//! 合法、失败提示、跳转意图全部由 `term-app/goto.rs` 产出，本文件不算第二遍。
//!
//! Sessions 模式刻意**不做文本过滤**：列表就是当前打开的标签，`selected` 始终是
//! 真实标签下标，`Confirm` 直接产出这个下标，落地处不需要把「过滤后第 n 项」
//! 映射回去（那种映射在状态复位后必然丢失）。会话树的搜索已由 sidebar 承担。

use crate::ui::harness::AnchoredWindow;
use crate::ui::*;
use term_app::goto::{GotoAction, GotoMode, GotoState};
use term_app::i18n::Lang;

impl TerminalWindow {
    /// Goto 浮层。默认关闭时一行都不画——五张视觉基线因此不受影响。
    pub(in crate::ui) fn draw_goto(&mut self, ctx: &egui::Context) {
        if !self.view.goto.visible {
            return;
        }
        let lang = self.lang();
        let disconnected = self.tr(" (disconnected)");
        let needle = self.view.goto.input.trim().to_lowercase();
        let mode = self.view.goto.mode;
        let (targets, labels): (Vec<i64>, Vec<String>) = if mode == GotoMode::Symbol {
            // 符号表就是 OSC 133 的命令块：与 Outline 面板同源，过滤词也复用
            // 同一套判据。targets 存绝对历史行号，Confirm 直接跳过去。
            let items = self
                .tabs
                .get(self.active_slot())
                .map(|t| {
                    crate::ui::panels::outline_items(
                        &t.snapshot.blocks,
                        &t.view.lines,
                        t.snapshot.history,
                        &needle,
                    )
                })
                .unwrap_or_default();
            (
                items.iter().map(|(row, _)| *row).collect(),
                items.into_iter().map(|(_, text)| text).collect(),
            )
        } else {
            let entries: Vec<(usize, String)> = self
                .tabs
                .iter()
                .enumerate()
                .map(|(i, tab)| {
                    let state = if tab.is_live() {
                        tab.config.name.clone()
                    } else {
                        format!("{}{disconnected}", tab.config.name)
                    };
                    (i, format!("{} · {}", i + 1, state))
                })
                .collect();
            let filtered: Vec<(usize, String)> =
                if mode == GotoMode::Sessions && !needle.is_empty() {
                    entries
                        .into_iter()
                        .filter(|(_, label)| label.to_lowercase().contains(&needle))
                        .collect()
                } else {
                    entries
                };
            (
                filtered.iter().map(|(i, _)| *i as i64).collect(),
                filtered.into_iter().map(|(_, label)| label).collect(),
            )
        };
        let active = self.tabs.get(self.active_slot());
        let history = active.map(|t| t.snapshot.history as i64).unwrap_or(0);
        // `+N` 的基准是当前光标所在的内容行；视图 range 已经是内容行号。
        let line_base = active
            .and_then(|t| t.view.cursor.map(|(row, _)| t.view.range.start + row))
            .unwrap_or(0);
        if self.view.goto.visible && self.view.goto.line_base != line_base {
            self.dispatch_action(Action::Goto(GotoAction::SetBase(line_base)));
        }

        let mut actions: Vec<Action> = Vec::new();
        {
            let state = &self.view.goto;
            let input = &mut self.goto_input;
            let mut dispatch = |action: Action| actions.push(action);
            draw_panel(ctx, state, input, &labels, &targets, history, lang, &mut dispatch);
        }
        for action in actions {
            self.dispatch_action(action);
        }
    }
}

fn draw_panel(
    ctx: &egui::Context,
    state: &GotoState,
    input: &mut String,
    labels: &[String],
    targets: &[i64],
    history: i64,
    lang: Lang,
    dispatch: &mut dyn FnMut(Action),
) {
    let tr = |en: &'static str| t(lang, en);
    // 只有外部改了 input（Open / Close / 菜单派发）才覆盖缓冲。
    if input != &state.input {
        input.clear();
        input.push_str(&state.input);
    }
    let is_line = state.mode == GotoMode::Line;
    let total = labels.len();
    let selected = if total == 0 {
        None
    } else {
        Some(state.selected % total)
    };

    let (up, down, enter, escape) = ctx.input(|i| {
        (
            i.key_pressed(egui::Key::ArrowUp),
            i.key_pressed(egui::Key::ArrowDown),
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });
    if escape {
        dispatch(Action::Goto(GotoAction::Close));
        return;
    }
    if enter {
        // 过滤后的选中下标要映射回真实目标：会话模式映射成标签下标，
        // 符号模式映射成绝对历史行——Confirm 只消费这个已经解引用过的值。
        if let Some(target) = selected.and_then(|s| targets.get(s).copied()) {
            match state.mode {
                GotoMode::Sessions => dispatch(Action::Goto(GotoAction::Select(target as usize))),
                GotoMode::Symbol => dispatch(Action::Goto(GotoAction::SelectSymbol(target))),
                GotoMode::Line => {}
            }
        }
        dispatch(Action::Goto(GotoAction::Confirm));
        return;
    }
    // ↑↓ 只在会话列表里有意义；行号输入框里不吞方向键。
    if !is_line && up {
        dispatch(Action::Goto(GotoAction::Move(-1, total)));
    }
    if !is_line && down {
        dispatch(Action::Goto(GotoAction::Move(1, total)));
    }

    let title = match state.mode {
        GotoMode::Line => tr("Goto Line"),
        GotoMode::Sessions => tr("Goto Session"),
        GotoMode::Symbol => tr("Goto Symbol"),
    };
    // 稳定 id `goto`：两种标题是同一个浮层，标题变了不该换一份窗口状态。
    AnchoredWindow::new("goto", title, 380.0).show(ctx, |ui| {
            if is_line {
                let response = ui.add(
                    egui::TextEdit::singleline(input)
                        .hint_text(tr("Line (0 = screen top, negative = history)"))
                        .desired_width(340.0),
                );
                if !response.has_focus() {
                    response.request_focus();
                }
                if response.changed() {
                    dispatch(Action::Goto(GotoAction::SetInput(input.clone())));
                }
                ui.label(
                    tr("Scrollback up to line {n}")
                        .replace("{n}", &(-history).to_string()),
                );
                if state.error {
                    ui.colored_label(
                        egui::Color32::from_rgb(0xE0, 0x60, 0x60),
                        tr("Enter an integer line number (may start with -)"),
                    );
                }
            } else if labels.is_empty() {
                ui.label(tr("No open sessions"));
            } else {
                for (idx, label) in labels.iter().enumerate() {
                    let row = ui.selectable_label(Some(idx) == selected, label.clone());
                    if row.clicked() {
                        match targets.get(idx).copied() {
                            Some(row) if state.mode == GotoMode::Symbol => {
                                dispatch(Action::Goto(GotoAction::SelectSymbol(row)));
                            }
                            Some(idx) => {
                                dispatch(Action::Goto(GotoAction::Select(idx as usize)));
                            }
                            None => {}
                        }
                        dispatch(Action::Goto(GotoAction::Confirm));
                    }
                }
            }
            ui.separator();
            ui.label(if is_line {
                tr("Enter Jump · Esc Close")
            } else {
                tr("↑↓ Select · Enter Switch · Esc Close")
            });
    });
}

#[cfg(test)]
mod tests {
    use term_app::goto::{parse_line, parse_line_with_base};

    #[test]
    fn line_mode_input_is_parsed_in_the_pure_layer() {
        // 面板不重复实现「什么算合法行号」；这里只复核接线用的纯函数。
        assert_eq!(parse_line("-7"), Some(-7));
        assert_eq!(parse_line("7x"), None);
        assert_eq!(parse_line_with_base("+3", 10), Some(13));
        assert_eq!(parse_line_with_base("+x", 10), None);
    }
}
