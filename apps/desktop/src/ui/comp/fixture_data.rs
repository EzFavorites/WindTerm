//! 组件夹具的确定性数据摆位（从 `comp/mod.rs` 拆出，守 500 行门禁）。

use crate::ui::TerminalWindow;

/// 原版 `09-panels/session-tree.png`：Shell sessions 分组 + 8 行会话。
pub(in crate::ui) fn seed_session_tree(win: &mut TerminalWindow) {
    win.groups = vec!["Shell sessions".to_string()];
    win.sessions = [
        "10.255.255.1", "10.255.255.1", "u****@localhost", "u****@localhost",
        "u****@localhost", "u****@localhost", "u****@localhost", "u****@localhost",
    ]
    .iter()
    .map(|name| {
        let mut cfg = term_core::session::SessionConfig::new(term_core::session::Protocol::Ssh);
        cfg.name = (*name).to_string();
        cfg.group = "Shell sessions".to_string();
        cfg
    })
    .collect();
    win.selected_session = 0;
}

/// 原版会话树面板的标题带：蓝方块 + Session + 齿轮 + ×。
pub(in crate::ui) fn draw_session_tree_title(ui: &mut egui::Ui) {
    let top = ui.max_rect().min;
    let bar = egui::Rect::from_min_size(top, egui::vec2(268.0, 21.0));
    let p = ui.painter().clone();
    let s = &term_core::theme::CHROME_SCALE;
    p.rect_filled(
        egui::Rect::from_center_size(
            egui::pos2(top.x + 12.0, bar.center().y),
            egui::vec2(8.0, 8.0),
        ),
        egui::Rounding::ZERO,
        egui::Color32::from_rgb(0x00, 0x78, 0xD7),
    );
    p.text(
        egui::pos2(top.x + 22.0, bar.center().y),
        egui::Align2::LEFT_CENTER,
        "Session",
        egui::TextStyle::Button.resolve(ui.style()),
        crate::theme::chrome(s.text),
    );
    crate::ui::shell::paint_gear(&p, egui::pos2(top.x + 240.0, bar.center().y), crate::theme::chrome(s.icon));
    crate::ui::shell::paint_cross(&p, egui::pos2(top.x + 259.0, bar.center().y), crate::theme::chrome(s.icon));
    ui.advance_cursor_after_rect(bar);
}
