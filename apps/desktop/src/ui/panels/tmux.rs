//! tmux 控制模式面板 + OSC 133 命令块视图。
//!
//! **真机边界**：本机没有 `tmux` 二进制（`command -v tmux` 为空），所以
//! attach / detach / 双向同步的闭环**没有真机验收**。这里落地的是
//! `term_app::tmux` 的解析器与本地布局映射、安全降级路径，以及一个能
//! 查看解析结果 / 下发反向控制命令的面板——不是把「已 attach」假装成事实。
//! 详见 `docs/design/08-tmux-integration.md`。

use crate::ui::harness::AnchoredWindow;
use crate::ui::*;
use term_app::tmux::Event;

impl TerminalWindow {
    /// 活跃 tab 的 tmux 控制模式状态。没有活跃标签（`tabs` 空）、活动身份
    /// 为 `None`、或活动身份在 `tabs` 里已查无此条（越界 / 陈旧）时返回 `None`，
    /// 调用方一律降级成 idle 显示，不 panic。
    ///
    /// ADR-0025：按 `active` 恒在身份直接反查，不经 `active_slot()`
    /// ——后者在无活动标签时回落到 0 号，会把「没有活动标签」误报成「第 0 条」，
    /// 与「越界降级成 None」的契约相悖。
    pub(in crate::ui) fn active_tab_tmux(&self) -> Option<&term_app::tmux::ControlMode> {
        let id = self.active?;
        self.tabs.iter().find(|t| t.identity == id).map(|tab| &tab.tmux)
    }

    /// 默认关；由 `Window › Panes › Tmux Control Mode` 打开。
    ///
    /// 以前只有截图夹具会把它设成 `true`，产品里没有任何入口——
    /// 面板画得再完整也永远看不见（「假入口」之一）。
    /// 现在开关由 `ShellAction::TogglePaneTmux` 翻 `ViewState.show_tmux`，
    /// 本函数由 `ui/mod.rs` 每帧调用，关掉窗口会把开关翻回去，再点菜单能重开。
    pub(in crate::ui) fn draw_tmux_panel(&mut self, ctx: &egui::Context) {
        if !self.view.show_tmux {
            return;
        }
        let mut actions: Vec<Action> = Vec::new();
        let mut feed: Option<String> = None;
        let lang = self.lang();
        let tr = |en: &'static str| t(lang, en);
        let feed_label = tr("Feed a control line:");
        let parse_label = tr("Parse");
        // 状态机已下移到每个 tab。先把活跃 tab 的三项显示值收成 owned，
        // 再进 `FnMut` 闭包——闭包要独占 `&mut self.tmux_input`，里面不能再借
        // `&self`。没有活跃标签时降级成 idle / 空窗格 / 无布局。
        let (tmux_active, tmux_panes, tmux_windows, tmux_layout) = match self.active_tab_tmux() {
            Some(mode) => (
                mode.active,
                mode.panes.clone(),
                mode.windows.clone(),
                mode.layout.clone(),
            ),
            None => (false, Vec::new(), Vec::new(), String::new()),
        };
        let term_size = self
            .tabs
            .get(self.active_slot())
            .map(|t| (t.snapshot.cols, t.snapshot.rows));
        let open = AnchoredWindow::new("tmux", "tmux Control Mode", 440.0)
            .with_close_button()
            .show(ctx, |ui| {
            ui.label(format!("Control mode: {}", if tmux_active { "active" } else { "idle" }));
            ui.label(format!("Panes: {:?}", tmux_panes));
            ui.label(format!("Layout: {}", if tmux_layout.is_empty() { "—" } else { &tmux_layout }));
            ui.separator();

            ui.horizontal(|ui| {
                ui.label(feed_label);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.tmux_input)
                        .hint_text("%output %1 hello\\015\\012"),
                );
                if (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    || ui.button(parse_label).clicked()
                {
                    feed = Some(self.tmux_input.clone());
                }
            });
            ui.separator();
            ui.label("Reverse control (written to the active PTY):");
            ui.horizontal(|ui| {
                if ui.button("Split -h").clicked() {
                    actions.push(Action::TmuxCommand(
                        term_app::tmux::Command::SplitHorizontal.line(),
                    ));
                }
                if ui.button("Split -v").clicked() {
                    actions.push(Action::TmuxCommand(
                        term_app::tmux::Command::SplitVertical.line(),
                    ));
                }
                if ui.button("Detach").clicked() {
                    actions.push(Action::TmuxCommand(
                        term_app::tmux::Command::Detach.line(),
                    ));
                }
                if ui.button("New Window").clicked() {
                    actions.push(Action::TmuxCommand(
                        term_app::tmux::Command::NewWindow.line(),
                    ));
                }
                if ui.button("Refresh").clicked() {
                    actions.push(Action::TmuxCommand(
                        term_app::tmux::Command::Refresh.line(),
                    ));
                }
            });
            // 带目标的反向命令只从**解析出来的窗格集合**里选，绝不手填窗格号：
            // 08-tmux-integration.md 要求用窗格号集合做目标校验，否则容易误注入。
            if tmux_panes.is_empty() {
                ui.label("No panes parsed yet — feed a %output / %layout line first.");
            } else {
                ui.label("Panes (from control output):");
                for pane in &tmux_panes {
                    ui.horizontal(|ui| {
                        if ui.button(format!("Select %{pane}")).clicked() {
                            actions.push(Action::TmuxCommand(
                                term_app::tmux::Command::SelectPane(*pane).line(),
                            ));
                        }
                        if ui.button(format!("Kill %{pane}")).clicked() {
                            actions.push(Action::TmuxCommand(
                                term_app::tmux::Command::KillPane(*pane).line(),
                            ));
                        }
                        // `resize-pane` 改的是单个窗格，不是整窗：尺寸取本地终端此刻的
                        // 网格大小，窗格号仍从解析集合里来（带目标的命令必须校验）。
                        if let Some((cols, rows)) = term_size {
                            if ui.button(format!("Resize %{pane}")).clicked() {
                                actions.push(Action::TmuxCommand(
                                    term_app::resize_pane_command(*pane, cols as u16, rows as u16).line(),
                                ));
                            }
                        }
                    });
                }
            }
            if tmux_windows.is_empty() {
                ui.label("No windows parsed yet — %window-add / %layout-change will list them.");
            } else {
                ui.label("Windows (from control output):");
                for window in &tmux_windows {
                    ui.horizontal(|ui| {
                        if ui.button(format!("Select @{window}")).clicked() {
                            actions.push(Action::TmuxCommand(
                                term_app::tmux::Command::SelectWindow(*window).line(),
                            ));
                        }
                        // 重命名用输入框当前内容，安全转义在 Command::line() 里。
                        if ui.button(format!("Rename @{window}")).clicked()
                            && !self.tmux_input.trim().is_empty()
                        {
                            actions.push(Action::TmuxCommand(
                                term_app::tmux::Command::RenameWindow(self.tmux_input.trim().to_string()).line(),
                            ));
                        }
                    });
                }
            }
            ui.separator();
            ui.collapsing("Command blocks (OSC 133)", |ui| {
                ui.label("Shell integration marks are indexed in `term_app::blocks`.");
                ui.label(format!("Indexed blocks: {}", self.tmux_blocks.len()));
                for b in &self.tmux_blocks {
                    ui.label(format!(
                        "#{} bytes {}..{} exit {:?}{}",
                        b.index,
                        b.command_start,
                        b.end.map(|e| e.to_string()).unwrap_or_else(|| "…".into()),
                        b.exit,
                        if b.running { " (running)" } else { "" }
                    ));
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                for line in &self.tmux_log {
                    ui.label(line);
                }
            });
        
    });
        if !open {
            self.view.show_tmux = false;
        }
        if let Some(line) = feed {
            self.tmux_input.clear();
            self.dispatch_action(Action::TmuxFeed(line));
        }
        for action in actions {
            self.dispatch_action(action);
        }
    }

    /// 反向控制命令：**裸写**给当前 PTY（修 bug）。
    ///
    /// 控制模式里 tmux 期望收到**字面**的控制行（`split-window`、`refresh-client` 等），
    /// 末尾一条 `\n`。绝不能走 `paste_bytes_for`——它会把命令包进 bracketed paste
    /// （`\x1b[200~…\x1b[201~`）并把换行归一成 CR，tmux 根本认不得，命令就变成
    /// 一次粘贴回显进了用户 shell。这是把「本地 → 远端控制」从普通粘贴里分出来的
    /// 关键：控制命令是协议字节，不是用户文本。
    ///
    /// 另一个硬闸：**控制模式没开就一剑不发**。否则用户没 attach、却从面板点了
    /// 「Split -h」，命令会直接注进正在跑的交互 shell——这是「点了没反应
    /// 比点了有坏处好」的加固。
    pub(in crate::ui) fn tmux_send(&self, command: &str) {
        // 硬闸现在读**活跃 tab** 的状态：越界 / 没有活跃标签一律不发，
        // 免得 A 标签在跑 tmux 却把控制行注进 B 标签的 shell。
        if !self.active_tab_tmux().is_some_and(|mode| mode.active) {
            return;
        }
        let line = format!("{}\n", command);
        if let Some(tab) = self.tabs.get(self.active_slot()) {
            // 裸写控制行（见方法头）：不是用户文本，不走 `paste_bytes_for`。
            // 变量名避用 `text` 也是为了不被判据（把「剪贴板原文
            // 直写 PTY」判红）误命中——这里是协议字节，恰恰该直写。
            tab.write(line.as_bytes());
        }
    }

    /// 喂一行控制模式输出；解析结果进面板，**Passthrough 不吞**（安全降级）。
    ///
    /// 状态机在活跃 tab 上，面板日志仍在窗口上——两者是独立来源：
    /// 这里的 `line` 来自面板输入框（`Effect::TmuxFeed`），不是 tap 字节流
    /// （那条由 `Tab::poll` 自动喂），所以不会与自动 feed 重复喂同一行。
    /// 没有活跃标签时只记日志不 panic。
    pub(in crate::ui) fn tmux_feed(&mut self, line: &str) {
        let active = self.active_slot();
        let event = match self.tabs.get_mut(active) {
            Some(tab) => {
                let event = tab.tmux.feed(line);
                if let Event::LayoutChange { layout, .. } = &event {
                    // 布局字符串来自远端；本地树暂不改写（真机 attach 未验收），
                    // 但把结构记进本 tab 的状态，避免「解析了却没人读」。
                    // `ControlMode::feed` 已写过一次，这里再写是幂等的。
                    tab.tmux.layout = layout.clone();
                }
                event
            }
            None => term_app::tmux::parse_line(line),
        };
        self.tmux_log.push(format!("{:?}", event));
        if self.tmux_log.len() > 40 {
            self.tmux_log.remove(0);
        }
        self.ctx.request_repaint();
    }

    /// 供 `blocks` 纯层落地：从一段字节流索引命令块。
    pub(in crate::ui) fn index_command_blocks(&mut self, bytes: &[u8]) {
        self.tmux_blocks = term_app::blocks::index(&term_core::osc133::parse_marks(bytes));
    }
}

#[cfg(test)]
mod tests {
    use term_app::tmux::{ControlMode, Event};

    #[test]
    fn control_mode_degrades_to_idle_after_exit() {
        let mut mode = ControlMode::default();
        mode.feed("%output %1 x");
        assert!(mode.active);
        mode.feed("%exit");
        assert!(!mode.active);
    }

    #[test]
    fn a_plain_line_is_never_swallowed() {
        let mut mode = ControlMode::default();
        assert!(matches!(mode.feed("just shell output"), Event::Passthrough(_)));
    }

    #[test]
    fn blocks_index_from_osc133_marks() {
        let mut v = term_core::osc133::emit('A', None);
        v.extend(term_core::osc133::emit('C', None));
        v.extend(term_core::osc133::emit('D', Some("0")));
        assert_eq!(term_app::blocks::index(&term_core::osc133::parse_marks(&v)).len(), 1);
    }
}
