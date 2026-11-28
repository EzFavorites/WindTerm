//! Script 面板的主操作条：**一排**放完「跑哪个 / 跑在哪 / 怎么选 / 本次授权」。
//!
//! 旧版把「脚本下拉」「Path 输入」「Record + 权限」堆成三排，结果区被压扁，
//! 而下拉和输入框其实是同一个脚本来源的两份真相。现在只留一个路径输入框：
//! `▾` 从脚本库挑、`⋯` 打开文件选择器，两者都把结果写回同一个框；录制不占
//! 面板（菜单里已有），`⚙` 只在需要时展开本次运行授权。
//!
//! 形态仍复用 Sender 的 `Row` 原语：同样的图标尺寸、分隔线、下拉框与 18 点高。

use eframe::egui;
use term_core::theme::CHROME_SCALE;

use super::tools_sender_row::{
    Glyph, Row, GROUP_GAP, ICON_GAP, ICON_W, LABEL_GAP, ROW_H, SEP_PAD,
};
use crate::ui::TerminalWindow;

const RUN_GREEN: u32 = 0x008200;
const STOP_GREY: u32 = 0x646464;

pub(super) const SCRIPT_ROW_H: f32 = ROW_H;
/// 路径输入的最小可读宽；窄面板下宁可收窄会话格也不把路径切成省略号。
const PATH_MIN_W: f32 = 140.0;
const SESSION_W: f32 = 180.0;

/// 主操作条上的意图。绘制只产出它，落地统一在 `TerminalWindow::apply_script_cmd`。
pub(super) enum ScriptCmd {
    Run,
    Stop,
    NewTab,
    CloseTab,
    /// 管理脚本（右侧的 `×` 带 tooltip）。
    OpenConfig,
    /// `▾`：展开脚本库列表。
    OpenLibrary,
    /// `⋯`：打开系统文件选择器。
    BrowsePath,
    /// `⚙`：展开 / 收起本次运行授权。
    TogglePermissions,
    /// 点击会话下拉。
    OpenSessions,
    /// 选中会话列表的第 `usize` 项（下标 0 = `Current Session`）。
    PickSession(usize),
}

pub(super) struct ScriptRowOut {
    pub library_anchor: egui::Rect,
    pub session_anchor: egui::Rect,
    /// 输入框里**规范化之后**的路径，调用方写回页签状态。
    pub path: String,
    pub cmd: Option<ScriptCmd>,
}

/// 画主操作条。`path` 是本帧的输入缓冲（调用方从页签状态里取出来）。
pub(super) fn script_row(
    win: &TerminalWindow,
    ui: &mut egui::Ui,
    rect: egui::Rect,
    tab: usize,
    path: String,
    session_label: &str,
) -> ScriptRowOut {
    let ctx = ui.ctx().clone();
    let mut row = Row::new(ui, rect);
    let mut cmd = None;

    if row.icon("script_run", Glyph::Run, crate::theme::chrome(RUN_GREEN)).clicked() {
        cmd = Some(ScriptCmd::Run);
    }
    row.gap(ICON_GAP);
    if row.icon("script_stop", Glyph::Stop, crate::theme::chrome(STOP_GREY)).clicked() {
        cmd = Some(ScriptCmd::Stop);
    }
    row.gap(ICON_GAP);
    if row.icon("script_newtab", Glyph::Plus, crate::theme::chrome(CHROME_SCALE.icon)).clicked() {
        cmd = Some(ScriptCmd::NewTab);
    }
    row.gap(ICON_GAP);
    if row.icon("script_closetab", Glyph::Minus, crate::theme::chrome(CHROME_SCALE.icon)).clicked() {
        cmd = Some(ScriptCmd::CloseTab);
    }
    row.gap(SEP_PAD);
    if row
        .icon("script_config", Glyph::Cross, crate::theme::chrome(CHROME_SCALE.icon))
        .on_hover_text(win.tr("Manage Scripts"))
        .clicked()
    {
        cmd = Some(ScriptCmd::OpenConfig);
    }
    row.gap(SEP_PAD);
    row.separator();
    row.gap(SEP_PAD);

    let text_c = row.text_c;
    let session_word = win.tr("Session:");
    row.label(win.tr("Script:"), text_c);
    row.gap(LABEL_GAP);

    // 预留：会话标签 + 会话下拉 + 三个辅助图标（▾ ⋯ ⚙）与他们之间的间隔。
    let reserved = LABEL_GAP
        + row.text_width(session_word)
        + GROUP_GAP
        + SESSION_W
        + 3.0 * (ICON_W + ICON_GAP);
    let path_w = (rect.right() - 8.0 - row.cursor() - reserved).max(PATH_MIN_W);
    let mut path = path;
    let resp = row.text_field(
        &mut path,
        path_w,
        egui::Id::new(("script_panel_path", tab)),
        &win.tr("/path/to/script.rhai (relative = script library)"),
    );
    // 失焦 / 回车才规范化，输入中途不挪动光标。
    if resp.lost_focus() || (resp.has_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter))) {
        path = super::script_panel_path::normalize_with(&path, term_runtime::paths::scripts_dir().as_deref());
    }

    row.gap(ICON_GAP * 0.5);
    let lib_resp = row
        .icon("script_library_pick", Glyph::ChevronDown, crate::theme::chrome(CHROME_SCALE.icon))
        .on_hover_text(win.tr("Pick from script library"));
    let library_anchor = lib_resp.rect;
    if lib_resp.clicked() {
        cmd = Some(ScriptCmd::OpenLibrary);
    }
    row.gap(ICON_GAP * 0.5);
    if row
        .icon("script_browse", Glyph::Ellipsis, crate::theme::chrome(CHROME_SCALE.icon))
        .on_hover_text(win.tr("Browse for a script file"))
        .clicked()
    {
        cmd = Some(ScriptCmd::BrowsePath);
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
    row.gap(ICON_GAP);
    if row
        .icon("script_options", Glyph::Gear, crate::theme::chrome(CHROME_SCALE.icon))
        .on_hover_text(win.tr("Run options (this run only)"))
        .clicked()
    {
        cmd = Some(ScriptCmd::TogglePermissions);
    }

    ScriptRowOut {
        library_anchor,
        session_anchor,
        path,
        cmd,
    }
}

/// 工具行高与下拉框高在两边必须一致（同一个 `ROW_H` / `FIELD_H`）。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_row_reuses_the_sender_row_metrics() {
        assert_eq!(SCRIPT_ROW_H, ROW_H, "Script 行高必须等于 Sender 行高");
        assert_eq!(ROW_H, 24.0);
        const { assert!(SESSION_W >= 150.0) };
        const { assert!(PATH_MIN_W >= 120.0) };
    }
}
