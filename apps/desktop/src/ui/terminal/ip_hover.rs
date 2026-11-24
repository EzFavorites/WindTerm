//! 内置 IP 地址触发器（官网 page/10）：悬停终端里的 IPv4/IPv6 弹出菜单。
//!
//! 检测用的是 `term_app::trigger_builtins` 的纯函数，零 IO、零正则。这里只做
//! 两件事：把指针落到的事实窗口 cell 反查成被命中文本的列区间；把用户点中的
//! 菜单项变成「在哪个 shell 里执行什么命令」的意图。真正的写入在 `ip_probe`
//! 帧末落地——绘制期不碰 PTY，也不持 OS 进程。

use super::pane_pointer::GridGeom;
use crate::ui::*;
use term_app::trigger::{self, Rule};
use term_app::trigger_builtins::{self, Builtin};
use term_core::trigger::{ActionKind, TriggerAction, TriggerOn};
use term_core::grid::ScreenImage;

/// Custom Menu 规则在指针所在行命中了哪一段文本。复用 `feed_for` 的同一个
/// `find_in`，所以"输出时命中"与"悬停时能点到"是同一套判据。
fn custom_menu_at_pointer<'a>(
    image: &ScreenImage,
    geom: GridGeom,
    pos: egui::Pos2,
    rules: &'a [Rule],
) -> Option<(&'a Rule, String)> {
    let row = ((pos.y - geom.start.y) / geom.cell.y).floor();
    let col = ((pos.x - geom.start.x) / geom.cell.x).floor();
    if row < 0.0 || col < 0.0 {
        return None;
    }
    let line = image.lines.get(row as usize)?;
    let text = line.text();
    rules
        .iter()
        .filter(|rule| {
            rule.enabled
                && rule.on == TriggerOn::Output
                && !rule.pattern.is_empty()
                && !rule.config.menu.items.is_empty()
        })
        .find_map(|rule| {
            let (start, end) = trigger::match_in_line(rule, &text)?;
            let col = col as usize;
            if col < start || col >= end {
                return None;
            }
            let matched: String = text.chars().skip(start).take(end - start).collect();
            Some((rule, matched))
        })
}

/// 菜单项 → 触发器动作。四种动作都有执行器：`Open hyperlink` 走投影层的
/// `sysopen::open`（与 Help / Search online 同一个 opener），不再画假禁用项。
fn menu_action(item: &term_core::trigger::MenuItem, rule: &Rule, matched: &str) -> Option<TriggerAction> {
    let expand = |text: &str| text.replace("${match}", matched);
    match item.action {
        ActionKind::RunCommand => Some(TriggerAction::RunCommand(expand(&rule.config.action.os[0].command))),
        ActionKind::PlaySound => Some(TriggerAction::Beep),
        ActionKind::SendText => Some(TriggerAction::SendText(expand(&rule.config.send.text))),
        ActionKind::OpenHyperlink => {
            let configured = rule.config.action.hyperlink.trim();
            let url = if configured.is_empty() {
                matched.to_owned()
            } else {
                expand(configured)
            };
            Some(TriggerAction::OpenLink(url))
        }
    }
}

fn ping_command(ip: &str) -> String {
    if cfg!(windows) {
        format!("ping -n 4 {ip}")
    } else {
        format!("ping -c 4 {ip}")
    }
}

fn trace_command(ip: &str) -> String {
    if cfg!(windows) {
        format!("tracert {ip}")
    } else {
        format!("traceroute {ip}")
    }
}

/// 指针所在 cell 是否落在某个 IP 的文本区间内。
///
/// 软折开启时屏幕行 ≠ 逻辑行，这里不猜映射（宁可不弹，也不弹在错误的 IP 上）。
fn ip_at_pointer(image: &ScreenImage, geom: GridGeom, pos: egui::Pos2) -> Option<String> {
    let row = ((pos.y - geom.start.y) / geom.cell.y).floor();
    let col = ((pos.x - geom.start.x) / geom.cell.x).floor();
    if row < 0.0 || col < 0.0 {
        return None;
    }
    let line = image.lines.get(row as usize)?;
    let text = line.text();
    trigger_builtins::detect(&text)
        .into_iter()
        .find(|found| {
            if !matches!(found.kind, Builtin::Ipv4 | Builtin::Ipv6) {
                return false;
            }
            // detect 返回的是字节偏移；IP 字面量恒为 ASCII，字符列 == 字节列。
            let start = found.start;
            let end = start + found.text.len();
            (col as usize) >= start && (col as usize) < end
        })
        .map(|found| found.text)
}

/// 画悬停菜单。命中才建浮层，未命中时零分配、零像素变化。
pub(super) fn paint(
    ui: &egui::Ui,
    image: &ScreenImage,
    geom: GridGeom,
    word_wrap: bool,
    rules: &[Rule],
) -> Option<term_app::trigger::Hit> {
    if word_wrap {
        return None;
    }
    let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else {
        return None;
    };
    let mut picked: Option<(bool, String)> = None;
    let mut clicked_actions: Vec<TriggerAction> = Vec::new();
    let custom = if ip_at_pointer(image, geom, pos).is_none() {
        custom_menu_at_pointer(image, geom, pos, rules)
    } else {
        None
    };
    let ip = match (ip_at_pointer(image, geom, pos), custom.as_ref()) {
        (Some(ip), _) => Some(ip),
        (None, Some((_, matched))) => Some(matched.clone()),
        _ => None,
    };
    let Some(ip) = ip else {
        return None;
    };
    let id = if custom.is_some() {
        egui::Id::new("custom_trigger_menu")
    } else {
        egui::Id::new("ip_trigger_menu")
    };
    egui::Area::new(id)
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::LEFT_TOP, pos.to_vec2() + egui::vec2(10.0, 10.0))
        .show(ui.ctx(), |ui| {
            if let Some((rule, matched)) = &custom {
                ui.label(matched.as_str());
                for item in &rule.config.menu.items {
                    if item.separator {
                        ui.separator();
                        continue;
                    }
                    let action = menu_action(item, rule, matched);
                    if ui.button(item.label.clone()).clicked() {
                        // 不用 unwrap/expect：gate 4p 冻结了生产路径 panic 数。
                        if let Some(action) = action {
                            clicked_actions.push(action);
                        }
                    }
                }
            } else {
                ui.label(format!("IP: {ip}"));
                if ui.button("Ping (remote session)").clicked() {
                    picked = Some((false, ping_command(&ip)));
                }
                if ui.button("Traceroute (remote session)").clicked() {
                    picked = Some((false, trace_command(&ip)));
                }
                ui.separator();
                if ui.button("Ping (local shell)").clicked() {
                    picked = Some((true, ping_command(&ip)));
                }
                if ui.button("Traceroute (local shell)").clicked() {
                    picked = Some((true, trace_command(&ip)));
                }
            }
        });
    let actions = if !clicked_actions.is_empty() {
        clicked_actions
    } else if let Some((local, command)) = picked {
        // 内置 IP 触发器的菜单项与用户触发器共用同一条 Hit → apply_trigger 链路：
        // 远端用 SendLine，本地用 LocalSendLine，二者都由触发器执行器负责落地。
        vec![if local {
            TriggerAction::LocalSendLine(command)
        } else {
            TriggerAction::SendLine(command)
        }]
    } else {
        return None;
    };
    let rule_id = custom
        .as_ref()
        .map(|(rule, _)| rule.id.clone())
        .unwrap_or_else(|| "__builtin_ip".to_string());
    Some(term_app::trigger::Hit { rule_id, actions, matched: ip })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_commands_use_the_platform_counter_and_never_a_shell() {
        let ping = ping_command("2001:db8::1");
        let trace = trace_command("10.0.0.9");
        if cfg!(windows) {
            assert_eq!(ping, "ping -n 4 2001:db8::1");
            assert_eq!(trace, "tracert 10.0.0.9");
        } else {
            assert_eq!(ping, "ping -c 4 2001:db8::1");
            assert_eq!(trace, "traceroute 10.0.0.9");
        }
        // 检测产物只有十六进制、冒号与点，拼进 `Command` 参数不经过 shell，
        // 所以这里不需要额外的注入转义。
        assert!(ping.chars().all(|c| c.is_ascii_alphanumeric() || "-. :".contains(c)));
    }

    #[test]
    fn open_hyperlink_item_is_a_real_enabled_action() {
        let mut rule = Rule::default();
        rule.config.action.hyperlink = "https://e.test/${match}".into();
        let item = term_core::trigger::MenuItem {
            action: ActionKind::OpenHyperlink,
            ..term_core::trigger::MenuItem::default()
        };
        // 菜单项现在真的产出动作 —— paint 里不再有禁用项与「无 opener」提示。
        assert_eq!(
            menu_action(&item, &rule, "42"),
            Some(TriggerAction::OpenLink("https://e.test/42".into()))
        );
    }

    #[test]
    fn open_hyperlink_item_falls_back_to_the_matched_text() {
        let rule = Rule::default();
        let item = term_core::trigger::MenuItem {
            action: ActionKind::OpenHyperlink,
            ..term_core::trigger::MenuItem::default()
        };
        assert_eq!(
            menu_action(&item, &rule, "https://fallback.test/x"),
            Some(TriggerAction::OpenLink("https://fallback.test/x".into()))
        );
    }
}
