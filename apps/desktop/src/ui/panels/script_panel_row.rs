//! Script 面板的工具行：`▶ / ■ / ×` + 脚本下拉 + 执行会话下拉（**单选**）。
//!
//! 形态复用 Sender：同一套 `Row` 原语（`tools_sender_row`）、同样的图标尺寸与
//! 分隔线、同样 18 点高的手绘下拉框。差别只在内容——Sender 那行是「发什么、
//! 怎么切、发多少、多久一次、发给谁」，这里只有「跑哪个脚本、跑在哪个会话上」。
//!
//! 用户口径（2026-11-17）：
//!
//! > 设计按照复用 sender，只是没有了 sender 内容控制这部分，改成脚本相关，
//! > 执行会话也保持下拉框，默认当前会话，可以选择其他会话执行，但是只能单选，
//! > 不能像 sender 一样多选。
//!
//! 所以这里**没有** Text/Hex、By Line/Char、Count、Interval，会话下拉也是
//! 单选（圆点，不是方块勾）——多选那一套留在 Sender 里。

use eframe::egui;

use crate::ui::TerminalWindow;

use super::tools_sender_row::{Glyph, Row, GROUP_GAP, ICON_GAP, LABEL_GAP, ROW_H, SEP_PAD};
use term_core::theme::CHROME_SCALE;

/// 运行键的绿（与 Sender 同一颗，视觉基线 `#008200`）。
const RUN_GREEN: u32 = 0x008200;
/// 停止键的灰：没在跑时它比别的图标暗一档。
const STOP_GREY: u32 = 0x646464;
/// 工具行高。与 Sender 同一个数（`ROW_H`），两个面板并排时才是一条线。
pub(super) const SCRIPT_ROW_H: f32 = ROW_H;
/// 脚本下拉的最小可读宽。窄面板下宁可收窄到这个宽，也不画到裁剪区外。
const SCRIPT_MIN_W: f32 = 130.0;
/// 会话下拉的宽。它装的是标签名，比脚本名短，固定宽比按比例分更好看。
const SESSION_W: f32 = 180.0;

/// 工具行上的意图。绘制只产出它，落地统一在 `TerminalWindow::apply_script_cmd`。
pub(super) enum ScriptCmd {
    Run,
    Stop,
    NewTab,
    CloseTab,
    OpenConfig,
    /// 清掉上一次的结果 / 错误（不清缓冲，也不停正在跑的那次）。
    Clear,
    OpenScripts,
    OpenSessions,
    /// 选中脚本列表的第 `usize` 项。
    PickScript(usize),
    /// 选中会话列表的第 `usize` 项（下标 0 = `Current Session`）。
    PickSession(usize),
}

/// 本帧绘制结果：两个下拉的锚点 + 意图。
pub(super) struct ScriptRowOut {
    pub script_anchor: egui::Rect,
    pub session_anchor: egui::Rect,
    pub cmd: Option<ScriptCmd>,
}

/// 工具行。只读 `win`——本帧所有状态改动都攒成 `ScriptCmd`，由调用方落地。
///
/// `script_label` / `session_label` 是两格**未展开时**显示的文字（已由调用方
/// 折算过：脚本给文件名、会话给标签名或 `Current Session`）。
pub(super) fn script_row(
    win: &TerminalWindow,
    ui: &mut egui::Ui,
    rect: egui::Rect,
    script_label: &str,
    session_label: &str,
) -> ScriptRowOut {
    let mut row = Row::new(ui, rect);
    let mut cmd = None;

    if row.icon("script_run", Glyph::Run, crate::theme::chrome(RUN_GREEN)) {
        cmd = Some(ScriptCmd::Run);
    }
    row.gap(ICON_GAP);
    if row.icon("script_stop", Glyph::Stop, crate::theme::chrome(STOP_GREY)) {
        cmd = Some(ScriptCmd::Stop);
    }
    row.gap(ICON_GAP);
    row.gap(ICON_GAP);
    if row.icon("script_newtab", Glyph::Plus, crate::theme::chrome(CHROME_SCALE.icon)) {
        cmd = Some(ScriptCmd::NewTab);
    }
    row.gap(ICON_GAP);
    if row.icon("script_closetab", Glyph::Minus, crate::theme::chrome(CHROME_SCALE.icon)) {
        cmd = Some(ScriptCmd::CloseTab);
    }
    row.gap(SEP_PAD);
    if row.icon("script_config", Glyph::Cross, crate::theme::chrome(CHROME_SCALE.icon)) {
        cmd = Some(ScriptCmd::OpenConfig);
    }
    row.gap(SEP_PAD);
    row.separator();
    row.gap(SEP_PAD);

    let text_c = row.text_c;
    let session_word = win.tr("Session:");
    row.label(win.tr("Script:"), text_c);
    row.gap(LABEL_GAP);
    // 两个下拉平分剩余宽度：会话那格先按固定宽预留（含它自己的标签），
    // 剩下的全给脚本名——脚本名比标签名长得多，反过来分会让脚本名被切。
    let reserved = LABEL_GAP + row.text_width(session_word) + GROUP_GAP + SESSION_W;
    let avail = (rect.right() - 8.0 - row.cursor() - reserved).max(SCRIPT_MIN_W);
    let (script_resp, script_anchor) = row.dropdown(script_label, avail, "script_pick");
    if script_resp.clicked() {
        cmd = Some(ScriptCmd::OpenScripts);
    }
    row.gap(GROUP_GAP);
    row.label(session_word, text_c);
    row.gap(LABEL_GAP);
    let rest = (rect.right() - 8.0 - row.cursor()).max(60.0);
    let (session_resp, session_anchor) =
        row.dropdown(session_label, rest.min(SESSION_W), "script_session");
    if session_resp.clicked() {
        cmd = Some(ScriptCmd::OpenSessions);
    }

    ScriptRowOut {
        script_anchor,
        session_anchor,
        cmd,
    }
}

/// 工具行高与下拉框高在两边必须一致（同一个 `ROW_H` / `FIELD_H`）。
#[cfg(test)]
mod tests {
    use super::*;

    /// 两个下拉共用工具行的行高：任何一边改了数字，另一种行就错位。
    #[test]
    fn the_script_row_reuses_the_sender_row_metrics() {
        assert_eq!(SCRIPT_ROW_H, ROW_H, "Script 行高必须等于 Sender 行高");
        assert_eq!(ROW_H, 24.0);
        // 会话格固定宽要放得下「Current Session」这种 15 字符的名字；
        // 脚本格在窄面板下的下限：再窄就只剩省略号了。
        const { assert!(SESSION_W >= 150.0) };
        const { assert!(SCRIPT_MIN_W >= 120.0) };
    }
}
