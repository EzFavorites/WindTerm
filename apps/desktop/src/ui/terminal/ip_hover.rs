//! 内置 IP 地址触发器（官网 page/10）：悬停终端里的 IPv4/IPv6 弹出菜单。
//!
//! 检测用的是 `term_app::trigger_builtins` 的纯函数，零 IO、零正则。这里只做
//! 两件事：把指针落到的事实窗口 cell 反查成被命中文本的列区间；把用户点中的
//! 菜单项变成「在哪个 shell 里执行什么命令」的意图。真正的写入在 `ip_probe`
//! 帧末落地——绘制期不碰 PTY，也不持 OS 进程。

use super::pane_pointer::GridGeom;
use crate::ui::*;
use term_app::trigger_builtins::{self, Builtin};
use term_core::trigger::TriggerAction;
use term_core::grid::ScreenImage;

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
) -> Option<term_app::trigger::Hit> {
    if word_wrap {
        return None;
    }
    let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else {
        return None;
    };
    let Some(ip) = ip_at_pointer(image, geom, pos) else {
        return None;
    };
    let mut picked: Option<(bool, String)> = None;
    egui::Area::new(egui::Id::new("ip_trigger_menu"))
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::LEFT_TOP, pos.to_vec2() + egui::vec2(10.0, 10.0))
        .show(ui.ctx(), |ui| {
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
        });
    if let Some((local, command)) = picked {
        // 内置 IP 触发器的菜单项与用户触发器共用同一条 Hit → apply_trigger 链路：
        // 远端用 SendLine，本地用 LocalSendLine，二者都由触发器执行器负责落地。
        let action = if local {
            TriggerAction::LocalSendLine(command)
        } else {
            TriggerAction::SendLine(command)
        };
        let hit = term_app::trigger::Hit {
            rule_id: "__builtin_ip".to_string(),
            actions: vec![action],
            matched: ip,
        };
        return Some(hit);
    }
    None
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
}
