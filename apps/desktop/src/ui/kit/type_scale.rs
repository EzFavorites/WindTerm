//! 全 UI 字体系统：**字号与字体族的唯一出处**（2026-11-19 统一）。
//!
//! # 为什么要有这一层
//!
//! 统一之前，字体/字号散落在几十个文件里，各写各的：
//! - `theme.rs` 全局 TextStyle 定义一套（Small 10.5 / Body 12 / Heading 13）；
//! - `settings` / `settings_live_onekeys` / `settings_live_triggers` / `triggers::fields`
//!   又各自 `text_styles.insert(...)` 覆盖一套；
//! - 三十多处 `painter.text(...)` / `RichText::font(...)` 直接写
//!   `FontId::proportional(13.0)` / `FontId::monospace(11.0)`。
//!
//! 结果是同一个 Body 在不同对话框被设成不同字号，字体族比例/等宽混用——
//! 用户看到的「字号不统一、字体乱」。这一层把它们收成一个语义阶梯：
//! 想改全 UI 的字号，只改本文件的常量，所有引用点一起生效。
//!
//! # 字体族：全等宽
//!
//! `theme.rs` 早已逐字确认视觉基线的 UI 字体是等宽（Roboto Mono / Menlo）：
//! 「放大视觉基线菜单栏 `Session` 里 i 与 o 占同一格宽」。等宽体还让文字与
//! 同行的图标共用一条墨迹中线（比例体墨迹中线比 galley 中线高约 2px）。
//! 因此这里所有语义 font 都用 [`FontFamily::Monospace`]；历史散落的
//! `proportional(...)` 是没跟上这条结论的遗留，统一时一并改回等宽。
//!
//! # 边界
//!
//! 只定义「字号数值 + 字体族 + 怎么装进 egui Style」，不含任何颜色、间距、
//! 业务状态。终端网格自己那套（按 cell 宽高算字号）不属于 UI chrome，不在此列。

use egui::{FontFamily, FontId, Style, TextStyle};

/// 微小字： tablets 表头、状态栏次要读数（原散落值 10.0）。
pub(crate) const SIZE_MICRO: f32 = 10.0;
/// 小字：egui Small、次要说明（原全局 Small 10.5、部分散落 11.0）。
pub(crate) const SIZE_SMALL: f32 = 11.0;
/// 正文：Body / Button / Monospace 基准（原全局 12.0，绝大多数控件）。
pub(crate) const SIZE_BODY: f32 = 12.0;
/// 标题：Heading、对话框标题条（原全局 13.0）。
pub(crate) const SIZE_HEADING: f32 = 13.0;
/// 字段输入：Trigger 对话框按视觉基线逐字测出的 7.35 逻辑点/字符 × Menlo
/// 0.602em 字距 = 12.2。刻意不并入 Body，否则原来 1:1 的字段宽度会缩 10%。
/// 集中到这里，是为了「一处可调」而不是各处各写一个 12.2。
pub(crate) const SIZE_FIELD: f32 = 12.2;
/// 强调字：Trigger 对话框 `Preview` 页签，按视觉基线实测 14.6。
pub(crate) const SIZE_EMPHASIS: f32 = 14.6;

/// 全 UI 统一字体族：等宽（理由见模块头）。
#[inline]
pub(crate) fn family() -> FontFamily {
    FontFamily::Monospace
}

/// 按统一字体族造一个指定字号的 FontId。
/// 散落点若确实需要非阶梯字号（如编辑器按行高算），至少字体族是统一的。
#[inline]
pub(crate) fn font(size: f32) -> FontId {
    FontId::new(size, family())
}

#[inline]
pub(crate) fn micro() -> FontId { font(SIZE_MICRO) }
#[inline]
pub(crate) fn small() -> FontId { font(SIZE_SMALL) }
#[inline]
pub(crate) fn body() -> FontId { font(SIZE_BODY) }
#[inline]
pub(crate) fn heading() -> FontId { font(SIZE_HEADING) }
#[inline]
pub(crate) fn field() -> FontId { font(SIZE_FIELD) }
#[inline]
pub(crate) fn emphasis() -> FontId { font(SIZE_EMPHASIS) }

/// 把统一字体阶梯装进全局 egui Style。`theme.rs` 建皮肤时调这一处即可，
/// 不再自己写五个 `text_styles.insert`。
pub(crate) fn install_text_styles(style: &mut Style) {
    style.text_styles.insert(TextStyle::Small, small());
    style.text_styles.insert(TextStyle::Body, body());
    style.text_styles.insert(TextStyle::Button, body());
    style.text_styles.insert(TextStyle::Heading, heading());
    style.text_styles.insert(TextStyle::Monospace, body());
}

/// 对话框局部把 Button/Body 压到 small 一档（Triggers 这类按视觉基线实测
/// 比全局正文小一号的页），字体族仍走统一等宽。
pub(crate) fn install_compact_styles(style: &mut Style) {
    install_sized_styles(style, SIZE_SMALL);
}

/// 只覆盖 Button / Body 两档到指定字号，其余 TextStyle 保持继承值。
/// OneKeys 用 `SIZE_BODY`、Triggers 用 `SIZE_SMALL`，都是视觉基线实测值；
/// 这里是它们共同的入口，字号参数必须来自本文件的常量。
pub(crate) fn install_sized_styles(style: &mut Style, size: f32) {
    let f = font(size);
    style.text_styles.insert(TextStyle::Button, f.clone());
    style.text_styles.insert(TextStyle::Body, f);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_semantic_fonts_are_monospace() {
        for f in [micro(), small(), body(), field(), heading(), emphasis()] {
            assert!(matches!(f.family, FontFamily::Monospace));
        }
    }

    #[test]
    fn size_ladder_is_monotonic() {
        assert!(SIZE_MICRO < SIZE_SMALL);
        assert!(SIZE_SMALL < SIZE_BODY);
        assert!(SIZE_BODY < SIZE_FIELD);
        assert!(SIZE_FIELD < SIZE_HEADING);
        assert!(SIZE_HEADING < SIZE_EMPHASIS);
    }

    #[test]
    fn install_covers_all_five_text_styles() {
        let mut style = Style::default();
        install_text_styles(&mut style);
        for ts in [
            TextStyle::Small,
            TextStyle::Body,
            TextStyle::Button,
            TextStyle::Heading,
            TextStyle::Monospace,
        ] {
            assert!(matches!(style.text_styles[&ts].family, FontFamily::Monospace));
        }
    }
}
