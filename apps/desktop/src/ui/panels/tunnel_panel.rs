//! 隧道管理面板（`Terminal.ManageTunnels` 的读者）。
//!
//! 内容全部来自 `ui/tunnels.rs` 已有的「在跑的隧道」：面板只是把它们列出来，
//! 再给两件真事——停一条、改规则后重起。没有第二条隧道实现，也没有第二份
//! 规则语法：`parse_rules` 仍是唯一入口。
//!
//! 为什么不放在 `ui/tunnels.rs` 里：那个文件管「起 / 轮询 / 收摊」的生命周期，
//! 这个文件管「给人看的界面」，两者规模都够一个文件了。

use crate::ui::harness::AnchoredWindow;
use crate::ui::*;

impl TerminalWindow {
    /// 画隧道面板。`show_tunnels` 为假时一行都不画（视觉基线靠它守住）。
    pub(in crate::ui) fn draw_tunnel_manager(&mut self, ctx: &egui::Context) {
        if !self.view.show_tunnels {
            return;
        }
        let mut stop: Option<String> = None;
        let mut restart: Option<String> = None;
        let mut remove: Option<usize> = None;
        let mut add: Option<String> = None;

        let active = self.tabs.get(self.active_slot()).map(|t| t.config.name.clone());
        let rows = self.tunnel_rows();
        let lang = self.lang();
        let tr = |en: &'static str| t(lang, en);
        let title = tr("Tunnels");
        let empty = tr("No forwarding tunnel is running.");
        let stop_label = tr("Stop");
        let restart_label = tr("Restart");
        let rules_label = tr("Forwarding rules of the active session");
        let add_label = tr("Add");

        let open = AnchoredWindow::new("tunnels", title, 480.0)
            .with_close_button()
            .with_max_height(420.0)
            .show(ctx, |ui| {
            if rows.is_empty() {
                ui.label(empty);
            }
            for (session, state, rules, error) in &rows {
                ui.horizontal(|ui| {
                    ui.strong(session);
                    ui.label(state);
                    if ui.small_button(stop_label).clicked() {
                        stop = Some(session.clone());
                    }
                    if ui.small_button(restart_label).clicked() {
                        restart = Some(session.clone());
                    }
                });
                for rule in rules {
                    ui.label(format!("    {rule}"));
                }
                if let Some(error) = error {
                    ui.colored_label(egui::Color32::from_rgb(200, 90, 90), format!("    {error}"));
                }
                ui.separator();
            }

            ui.label(rules_label);
            if let Some(name) = &active {
                let rules = self.forwards_of(name);
                if rules.is_empty() {
                    ui.label("(none)");
                }
                for (i, rule) in rules.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(rule);
                        if ui.small_button("x").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.tunnel_input)
                            .hint_text("L 8080:db.internal:5432"),
                    );
                    if ui.button(add_label).clicked() {
                        add = Some(self.tunnel_input.clone());
                    }
                });
                if let Some(err) = &self.tunnel_error {
                    ui.colored_label(egui::Color32::from_rgb(200, 90, 90), err);
                }
            } else {
                ui.label("(no active session)");
            }
        
    });

        // 界面上的意图统一在窗口关掉之后落地：绘制期间 `self` 的借用已经结束。
        if let Some(session) = stop {
            self.stop_tunnel(&session);
        }
        if let Some(session) = restart {
            self.restart_tunnel(&session);
        }
        if let Some(i) = remove {
            self.remove_forward(i);
        }
        if let Some(text) = add {
            self.add_forward(&text);
        }
        if !open {
            self.dispatch_action(Action::Shell(term_app::shell::ShellAction::ManageTunnels));
        }
    }
}

#[cfg(test)]
mod tests {
    // 目录重排后 `tunnels` 不再与本文同级（原来靠 `use super::*` 顺手带进来），
    // 得显式指到 `panels::tunnels`——那行 glob 也就没用了，留着只会被
    // `--all-targets` 的 clippy 报成 unused import。
    use crate::ui::panels::tunnels;

    #[test]
    fn adding_a_rule_accepts_ssh_style_lines_and_rejects_junk() {
        // 规则语法只有 `tunnels::parse_rule` 一处：面板不自己解释字符串。
        assert!(tunnels::parse_rule("L 8080:db:5432").is_ok());
        assert!(tunnels::parse_rule("hello").is_err());
        // 三种类型各归各类，面板不该把它们混成一种。
        assert!(matches!(
            tunnels::parse_rule("L 8080:h:1").unwrap(),
            term_tunnel::Forward::Local { .. }
        ));
        assert!(matches!(
            tunnels::parse_rule("R 9000:localhost:3000").unwrap(),
            term_tunnel::Forward::Remote { .. }
        ));
        assert!(matches!(
            tunnels::parse_rule("D 1080").unwrap(),
            term_tunnel::Forward::Socks5 { .. }
        ));
    }

    #[test]
    fn storing_the_display_text_would_break_the_config() {
        // 配置里存的是**输入语法**（`L 8080:db:5432`），不是 `describe()` 的
        // 显示文本（`-L … → …`）。这条单测钉的是那个坑：存错了下次就连不上，
        // 而且用户看到的是「规则还在列表里」。
        let line = "L 8080:db:5432";
        let parsed = tunnels::parse_rule(line).expect(line);
        assert!(tunnels::parse_rule(&parsed.describe()).is_err());
        assert!(tunnels::parse_rule(line).is_ok());
    }
}
