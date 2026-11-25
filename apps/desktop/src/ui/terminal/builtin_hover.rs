//! 官网 #47 / #48 / #49 三个内置触发器的悬停浮层：Hexadecimal Converter、
//! Timestamp Converter、Unarchiver。
//!
//! 检测与换算全在 `term_app::trigger_builtins`（纯函数、零 IO / 零墙钟）；这里只做
//! 两件事：把指针所在 cell 反查成被命中的片段；把换算结果拼成文案画进 `egui::Area`。
//! 这三个触发器只**展示**结果——不写 PTY、不改选区、不返回 `TriggerAction`
//! （要执行动作的 IP 菜单走 `ip_hover.rs`，与本文件互不相干）。

use super::hover_anchor;
use super::hover_hit;
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
/// 软折开启时屏幕行 ≠ 逻辑行，这里不猜映射（宁可不弹，也不弹在错误的片段上）。
/// `detect` 给的是字节偏移，中文前缀要先换成字形、再换成 cell 列。
fn tooltip_at_pointer(image: &ScreenImage, row: i64, col: usize) -> Option<String> {
    let line = image.lines.get(row.max(0) as usize)?;
    let text = line.text();
    trigger_builtins::detect(&text)
        .into_iter()
        .find(|found| {
            matches!(
                found.kind,
                Builtin::Hex | Builtin::Timestamp | Builtin::Archive
            ) && hover_hit::byte_range_to_cols(
                &text,
                &line.cells,
                found.start,
                found.start + found.text.len(),
            )
            .is_some_and(|(from, to)| col >= from && col < to)
        })
        .and_then(|found| tooltip_text(&found))
}

/// 画浮层。未命中 / 软折 / 无指针时零分配、零像素变化。
pub(super) fn paint(
    ui: &egui::Ui,
    pane_id: egui::Id,
    image: &ScreenImage,
    geom: GridGeom,
    word_wrap: bool,
) {
    if word_wrap {
        return;
    }
    let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let row = ((pos.y - geom.start.y) / geom.cell.y).floor();
    let col = ((pos.x - geom.start.x) / geom.cell.x).floor();
    if row < 0.0 || col < 0.0 {
        return;
    }
    let (row, col) = (row as i64, col as usize);
    let fresh = tooltip_at_pointer(image, row, col);
    let token = match &fresh {
        Some(text) => format!("{row}:{col}:{text}"),
        None => String::new(),
    };
    // 与 ip 菜单同一把口径：Id 只按调用方给的稳定窗格 Id + 种类定型，
    // 不随指针行列漂移，也不用随帧内控件数漂移的 `ui.unique_id()`。
    let id = hover_anchor::hover_id(pane_id, "builtin_tooltip");
    let (anchor, payload) = hover_anchor::resolve(
        ui, id, &token, pos, egui::vec2(10.0, 10.0), fresh,
    );
    let Some(text) = payload else {
        return;
    };
    let response = egui::Area::new(id)
        .order(egui::Order::Tooltip)
        .anchor(egui::Align2::LEFT_TOP, anchor.to_vec2())
        .show(ui.ctx(), |ui| {
            ui.label(text);
        });
    hover_anchor::remember::<Option<String>>(ui, id, response.response.rect);
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

    /// 接线级：paint 收的是调用方给的稳定窗格 Id（生产由 `stable_pane_id(slot)` 算）；
    /// 指针进浮层时行列变了也不能换钥匙。
    #[test]
    fn the_tooltip_id_is_per_pane_and_not_per_pointer_cell() {
        let pane = hover_anchor::stable_pane_id(1);
        let id = hover_anchor::hover_id(pane, "builtin_tooltip");
        assert_eq!(id, hover_anchor::hover_id(pane, "builtin_tooltip"));
        assert_ne!(id, hover_anchor::hover_id(pane, "ip_menu"));
        assert_ne!(id, hover_anchor::hover_id(hover_anchor::stable_pane_id(2), "builtin_tooltip"));
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

    #[test]
    fn a_hex_token_after_a_chinese_prefix_is_hittable() {
        let mut image = ScreenImage::blank(30, 1, 0..1);
        let text = "前缀 0xFF rest";
        let mut col = 0usize;
        for ch in text.chars() {
            let wide = ('\u{4e00}'..='\u{9fff}').contains(&ch);
            image.lines[0].cells[col] = term_core::grid::Cell { ch, wide, ..term_core::grid::Cell::default() };
            col += 1;
            if wide {
                image.lines[0].cells[col] = term_core::grid::Cell::default();
                col += 1;
            }
        }
        let tooltip = tooltip_at_pointer(&image, 0, 5);
        assert!(tooltip.as_deref().is_some_and(|s| s.starts_with("Hex 0xFF")), "前缀占 5 格，0xFF 在 5..9：{tooltip:?}");
        assert_eq!(tooltip_at_pointer(&image, 0, 0), None, "中文那两格不该弹 hex");
    }
}
