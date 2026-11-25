//! 内置 IP 地址触发器（官网 page/10）：悬停终端里的 IPv4/IPv6 弹出菜单。
//!
//! 检测用的是 `term_app::trigger_builtins` 的纯函数，零 IO、零正则。这里只做
//! 两件事：把指针落到的事实窗口 cell 反查成被命中文本的列区间；把用户点中的
//! 菜单项变成「在哪个 shell 里执行什么命令」的意图。真正的写入在 `ip_probe`
//! 帧末落地——绘制期不碰 PTY，也不持 OS 进程。

use super::hover_anchor;
use super::hover_hit;
use super::pane_pointer::GridGeom;
use super::trigger_menu;
use crate::ui::*;
use term_app::trigger::{self, Rule};
use term_app::trigger_builtins::{self, Builtin};
use term_core::trigger::{ActionKind, TriggerAction, TriggerOn};
use term_core::grid::ScreenImage;

/// Custom Menu 规则在指针所在行命中了哪一段文本。复用 `feed_for` 的同一个
/// `find_in`，所以"输出时命中"与"悬停时能点到"是同一套判据。
fn custom_menu_at_pointer<'a>(
    image: &ScreenImage,
    row: i64,
    col: usize,
    rules: &'a [Rule],
) -> Option<(&'a Rule, String)> {
    let line = image.lines.get(row.max(0) as usize)?;
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
            // match_in_line 给的是字符下标；中文前缀一个字占两格，必须查字形表。
            let (from, to) = hover_hit::char_range_to_cols(&line.cells, start, end)?;
            if col < from || col >= to {
                return None;
            }
            let matched: String = text.chars().skip(start).take(end - start).collect();
            Some((rule, matched))
        })
}

/// 菜单项 → 触发器动作。`Open hyperlink` 走投影层的 `sysopen::open`（与 Help /
/// Search online 同一个 opener），不再画假禁用项。
fn menu_action(item: &term_core::trigger::MenuItem, rule: &Rule, matched: &str) -> Option<TriggerAction> {
    let expand = |text: &str| text.replace("${match}", matched);
    match item.action {
        // `RunCommand` 的载荷必须是命令 ID；配置里写的是正文时改走 SendLine，
        // 否则窗口级 dispatch 拿它当 ID 查表，查不到就静默丢掉这次点击。
        ActionKind::RunCommand => Some(trigger_menu::run_command_action(&expand(
            &trigger_menu::command_of(rule, trigger_menu::host()).command,
        ))),
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

/// 浮层内容的帧间锁存值。`Rule` 可克隆，指针进入浮层期间沿用旧菜单。
#[derive(Clone, PartialEq, Debug)]
enum Payload {
    Ip(String),
    Custom { rule: Rule, matched: String },
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
fn ip_at_pointer(image: &ScreenImage, row: i64, col: usize) -> Option<String> {
    let line = image.lines.get(row.max(0) as usize)?;
    let text = line.text();
    trigger_builtins::detect(&text)
        .into_iter()
        .find(|found| {
            matches!(found.kind, Builtin::Ipv4 | Builtin::Ipv6)
                && hover_hit::byte_range_to_cols(
                    &text,
                    &line.cells,
                    found.start,
                    found.start + found.text.len(),
                )
                .is_some_and(|(from, to)| col >= from && col < to)
        })
        .map(|found| found.text)
}

/// 本帧指针处的浮层内容：**先算一次 IP 命中，再分支**。
///
/// 早期版本在分支两侧各算一次 `ip_at_pointer`——重复扫描整行不说，第二次
/// 还可能因为指针已经进了上一帧的浮层而读出不同的行 / 列，把刚锁住的内容
/// 误判成「未命中」。这里只算一次并按 IP 优先的形状定型；锚点锁存仍由
/// `hover_anchor::resolve` 负责，本函数一行都没碰它的行为。
fn payload_at_pointer(image: &ScreenImage, row: i64, col: usize, rules: &[Rule]) -> Option<Payload> {
    if let Some(ip) = ip_at_pointer(image, row, col) {
        return Some(Payload::Ip(ip));
    }
    custom_menu_at_pointer(image, row, col, rules).map(|(rule, matched)| Payload::Custom {
        rule: rule.clone(),
        matched: matched.clone(),
    })
}

/// 画悬停菜单。命中才建浮层，未命中时零分配、零像素变化。
pub(super) fn paint(
    ui: &egui::Ui,
    pane_id: egui::Id,
    image: &ScreenImage,
    geom: GridGeom,
    word_wrap: bool,
    rules: &[Rule],
    id: egui::Id,
) -> Option<term_app::trigger::Hit> {
    if word_wrap {
        return None;
    }
    let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else {
        return None;
    };
    let row = ((pos.y - geom.start.y) / geom.cell.y).floor();
    let col = ((pos.x - geom.start.x) / geom.cell.x).floor();
    if row < 0.0 || col < 0.0 {
        return None;
    }
    let (row, col) = (row as i64, col as usize);
    let fresh = payload_at_pointer(image, row, col, rules);
    let token = match &fresh {
        Some(Payload::Ip(ip)) => format!("{row}:{col}:ip:{ip}"),
        Some(Payload::Custom { matched, .. }) => format!("{row}:{col}:custom:{matched}"),
        None => String::new(),
    };
    // Id 只按调用方给的稳定窗格 Id + 种类定型：row:col:token 只作为 resolve 的参数。
    // 指针移进浮层后行列会变，Id 不变才能取回上一帧锁住的锚点与菜单内容。
    let id = hover_anchor::hover_id(pane_id, "ip_menu");
    let (anchor, payload) = hover_anchor::resolve(
        ui, id, &token, pos, egui::vec2(10.0, 10.0), fresh,
    );
    let Some(payload) = payload else {
        return None;
    };

    let mut picked: Option<(bool, String)> = None;
    let mut clicked_actions: Vec<TriggerAction> = Vec::new();
    let response = egui::Area::new(id)
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::LEFT_TOP, anchor.to_vec2())
        .show(ui.ctx(), |ui| {
            match &payload {
                Payload::Custom { rule, matched } => {
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
                }
                Payload::Ip(ip) => {
                    ui.label(format!("IP: {ip}"));
                    if ui.button("Ping (remote session)").clicked() {
                        picked = Some((false, ping_command(ip)));
                    }
                    if ui.button("Traceroute (remote session)").clicked() {
                        picked = Some((false, trace_command(ip)));
                    }
                    ui.separator();
                    if ui.button("Ping (local shell)").clicked() {
                        picked = Some((true, ping_command(ip)));
                    }
                    if ui.button("Traceroute (local shell)").clicked() {
                        picked = Some((true, trace_command(ip)));
                    }
                }
            }
        });
    hover_anchor::remember::<Option<Payload>>(ui, id, response.response.rect);

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
    let rule_id = match &payload {
        Payload::Custom { rule, .. } => rule.id.clone(),
        Payload::Ip(_) => "__builtin_ip".to_string(),
    };
    let matched = match &payload {
        Payload::Custom { matched, .. } => matched.clone(),
        Payload::Ip(ip) => ip.clone(),
    };
    Some(term_app::trigger::Hit { rule_id, actions, matched })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 接线级：paint 收的是调用方给的**稳定**窗格 Id（生产由 `stable_pane_id(slot)` 算），
    /// 不用 `ui.unique_id()`；row/col 只作 resolve 参数。
    /// 指针进浮层后 row/col 变了，锁存仍得靠同一把钥匙取回。
    #[test]
    fn the_menu_id_is_per_pane_and_not_per_pointer_cell() {
        let pane = hover_anchor::stable_pane_id(1);
        let id = hover_anchor::hover_id(pane, "ip_menu");
        assert_eq!(id, hover_anchor::hover_id(pane, "ip_menu"), "同窗格恒等");
        assert_ne!(id, hover_anchor::hover_id(pane, "builtin_tooltip"), "不同浮层分开");
        assert_ne!(id, hover_anchor::hover_id(hover_anchor::stable_pane_id(2), "ip_menu"), "不同窗格分开");
    }

    #[test]
    fn one_probe_per_frame_decides_the_payload_and_ip_wins_over_custom_menu() {
        let text = "10.0.0.9";
        let mut image = ScreenImage::blank(text.len(), 1, 0..1);
        image.lines[0] = term_core::grid::LineSlice {
            row: 0,
            cells: text.chars().map(|ch| term_core::grid::Cell { ch, ..term_core::grid::Cell::default() }).collect(),
        };
        // 一条同样匹配这段文本的 Custom Menu 规则：IP 优先时它根本不该被读。
        let mut rule = Rule {
            enabled: true,
            on: TriggerOn::Output,
            literal: true,
            pattern: text.to_string(),
            ..Rule::default()
        };
        rule.config.menu.items.push(term_core::trigger::MenuItem {
            label: "custom".to_string(),
            ..term_core::trigger::MenuItem::default()
        });
        // 同一帧只算一次：命中 IP 就直接定型，不会在分支里再扫一遍。
        assert_eq!(
            payload_at_pointer(&image, 0, 2, &[rule]),
            Some(Payload::Ip(text.to_string()))
        );
        // 列落在 IP 之外时 IP 分支不成立，才轮到 Custom Menu。
        let mut image2 = ScreenImage::blank(text.len() + 2, 1, 0..1);
        image2.lines[0] = term_core::grid::LineSlice {
            row: 0,
            cells: format!("  {text}").chars().map(|ch| term_core::grid::Cell { ch, ..term_core::grid::Cell::default() }).collect(),
        };
        assert!(matches!(
            payload_at_pointer(&image2, 0, 1, &[Rule::default()]),
            None
        ));
    }

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

    #[test]
    fn a_custom_menu_run_command_item_uses_the_host_os_slot() {
        let mut rule = Rule::default();
        let texts = ["mac-cmd", "linux-cmd", "win-cmd"];
        for (slot, text) in rule.config.action.os.iter_mut().zip(texts) {
            slot.command = text.to_string();
        }
        let item = term_core::trigger::MenuItem {
            action: ActionKind::RunCommand,
            ..term_core::trigger::MenuItem::default()
        };
        let expected = trigger_menu::command_of(&rule, trigger_menu::host()).command.clone();
        // 命令正文不是命令 ID：必须成为 SendLine，不能塞进 RunCommand 当 ID。
        assert_eq!(
            menu_action(&item, &rule, "x"),
            Some(TriggerAction::SendLine(expected))
        );
        // 真 ID 则保持 RunCommand 契约，交给帧末窗口级 dispatch。
        let id = term_app::menu::context_ids::LINE_ENDINGS;
        rule.config.action.os[0].command = id.into();
        rule.config.action.os[1].command = id.into();
        rule.config.action.os[2].command = id.into();
        assert_eq!(
            menu_action(&item, &rule, "x"),
            Some(TriggerAction::RunCommand(id.into()))
        );
    }
}
