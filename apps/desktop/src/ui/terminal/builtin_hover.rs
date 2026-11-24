//! 官网 #47 / #48 / #49 三个内置触发器的悬停浮层：Hexadecimal Converter、
//! Timestamp Converter、Unarchiver。
//!
//! 检测与换算全在 `term_app::trigger_builtins`（纯函数、零 IO / 零墙钟）；这里只做
//! 两件事：把指针所在 cell 反查成被命中的片段；把换算结果拼成文案画进 `egui::Area`。
//! 这三个触发器只**展示**结果——不写 PTY、不改选区、不返回 `TriggerAction`
//! （要执行动作的 IP 菜单走 `ip_hover.rs`，与本文件互不相干）。

use super::pane_pointer::GridGeom;
use term_app::trigger_builtins::{self, Builtin, Found};
use term_core::grid::ScreenImage;

/// 一条命中 → 悬停文案。纯函数，便于无头单测；不属于本层的 Builtin 返回 `None`。
///
/// 文案里只放算出来的值，不读系统时间：时间戳的 UTC 文本是纯格式化产物。
pub(super) fn tooltip_text(found: &Found) -> Option<String> {
    match found.kind {
        Builtin::Hex => {
            let (dec, oct) = trigger_builtins::hex_parts(&found.text)?;
            Some(format!(
                "Hex {} → {dec} dec / 0o{oct} oct",
                found.text
            ))
        }
        Builtin::Timestamp => {
            let utc = trigger_builtins::timestamp_utc(&found.text)?;
            Some(format!("Timestamp {} → {utc}", found.text))
        }
        Builtin::Archive => {
            let kind = trigger_builtins::archive_kind(&found.text)?;
            Some(format!("Archive {} → {kind}", found.text))
        }
        // URL / IP 由链接层与 `ip_hover` 负责，这里一律不弹。
        Builtin::Url | Builtin::Ipv4 | Builtin::Ipv6 => None,
    }
}

/// 指针所在 cell 是否落在某个可悬停的内置片段上，命中则返回它的文案。
///
/// 软折开启时屏幕行 ≠ 逻辑行，这里不猜映射（宁可不弹，也不弹在错误的片段上）；
/// 三种 token 恒为 ASCII，`detect` 的字节偏移就是屏幕列。
fn tooltip_at_pointer(image: &ScreenImage, geom: GridGeom, pos: egui::Pos2) -> Option<String> {
    let row = ((pos.y - geom.start.y) / geom.cell.y).floor();
    let col = ((pos.x - geom.start.x) / geom.cell.x).floor();
    if row < 0.0 || col < 0.0 {
        return None;
    }
    let line = image.lines.get(row as usize)?;
    trigger_builtins::detect(&line.text())
        .into_iter()
        .find(|found| {
            matches!(
                found.kind,
                Builtin::Hex | Builtin::Timestamp | Builtin::Archive
            ) && (col as usize) >= found.start
                && (col as usize) < found.start + found.text.len()
        })
        .and_then(|found| tooltip_text(&found))
}

/// 画浮层。未命中 / 软折 / 无指针时零分配、零像素变化。
pub(super) fn paint(ui: &egui::Ui, image: &ScreenImage, geom: GridGeom, word_wrap: bool) {
    if word_wrap {
        return;
    }
    let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let Some(text) = tooltip_at_pointer(image, geom, pos) else {
        return;
    };
    egui::Area::new(egui::Id::new("builtin_trigger_tooltip"))
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::LEFT_TOP, pos.to_vec2() + egui::vec2(10.0, 10.0))
        .show(ui.ctx(), |ui| {
            ui.label(text);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(kind: Builtin, text: &str) -> Found {
        Found {
            kind,
            text: text.to_string(),
            start: 0,
        }
    }

    #[test]
    fn hex_tooltip_shows_decimal_and_octal() {
        assert_eq!(
            tooltip_text(&found(Builtin::Hex, "0xFF")),
            Some("Hex 0xFF → 255 dec / 0o377 oct".to_string())
        );
        assert_eq!(
            tooltip_text(&found(Builtin::Hex, "deadbeef")),
            Some("Hex deadbeef → 3735928559 dec / 0o33653337357 oct".to_string())
        );
    }

    #[test]
    fn timestamp_tooltip_uses_only_the_learned_value() {
        assert_eq!(
            tooltip_text(&found(Builtin::Timestamp, "1700000000")),
            Some("Timestamp 1700000000 → 2023-11-14T22:13:20Z".to_string())
        );
        assert_eq!(
            tooltip_text(&found(Builtin::Timestamp, "1700000000000")),
            Some("Timestamp 1700000000000 → 2023-11-14T22:13:20Z".to_string())
        );
        assert_eq!(
            tooltip_text(&found(Builtin::Timestamp, "-1700000000")),
            Some("Timestamp -1700000000 → 1916-02-18T01:46:40Z".to_string())
        );
        // 越界 / 非时间戳不造文案，也绝不读墙钟兜底。
        assert_eq!(tooltip_text(&found(Builtin::Timestamp, "123456789a")), None);
    }

    #[test]
    fn archive_tooltip_names_the_format() {
        assert_eq!(
            tooltip_text(&found(Builtin::Archive, "build/out.tar.gz")),
            Some("Archive build/out.tar.gz → gzip compressed tar".to_string())
        );
        assert_eq!(
            tooltip_text(&found(Builtin::Archive, "a.ZIP")),
            Some("Archive a.ZIP → ZIP archive".to_string())
        );
        assert_eq!(tooltip_text(&found(Builtin::Archive, "a.unknown")), None);
    }

    #[test]
    fn other_builtins_and_bad_values_do_not_show_a_tooltip() {
        assert_eq!(tooltip_text(&found(Builtin::Url, "https://x.io")), None);
        assert_eq!(tooltip_text(&found(Builtin::Ipv4, "10.0.0.9")), None);
        assert_eq!(tooltip_text(&found(Builtin::Ipv6, "::1")), None);
        assert_eq!(
            tooltip_text(&found(Builtin::Hex, "123456789012345678901234567890123")),
            None,
            "换算越界只返回 None，不 panic"
        );
    }
}
