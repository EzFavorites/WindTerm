//! 脚本面板的多页容器与底部数字页签条（从 `script_panel.rs` 拆出，守 500 行门禁）。
//!
//! 每页一整份 `ScriptPanelState`（运行句柄 / 结果 / 会话 / 授权），切页不打断另一页。
//! 形态照 Sender：单页不画页签条，多页时底部每页 24 点宽。

use eframe::egui;

use super::script_panel::ScriptPanelState;
use crate::ui::TerminalWindow;

pub(in crate::ui) const TAB_STRIP_H: f32 = 24.0;

/// 一个脚本页：编号 + 一整份独立状态（运行句柄 / 结果 / 会话 / 授权）。
#[derive(Default)]
pub(in crate::ui) struct ScriptTab {
    pub no: u32,
    pub state: ScriptPanelState,
}

/// Sender 形态的多页容器：每页互不打断，底部数字页签切换。
pub(in crate::ui) struct ScriptPanel {
    pub tabs: Vec<ScriptTab>,
    pub active: usize,
    pub next_no: u32,
}

impl Default for ScriptPanel {
    fn default() -> Self {
        Self {
            tabs: vec![ScriptTab { no: 0, state: ScriptPanelState::default() }],
            active: 0,
            next_no: 1,
        }
    }
}

impl ScriptPanel {
    pub fn current(&self) -> &ScriptPanelState {
        &self.tabs[self.active.min(self.tabs.len() - 1)].state
    }
    pub fn current_mut(&mut self) -> &mut ScriptPanelState {
        let i = self.active.min(self.tabs.len() - 1);
        &mut self.tabs[i].state
    }
    pub fn add(&mut self) {
        self.tabs.push(ScriptTab { no: self.next_no, state: ScriptPanelState::default() });
        self.next_no += 1;
        self.active = self.tabs.len() - 1;
    }
    pub fn close_active(&mut self) {
        if self.tabs.len() == 1 {
            return;
        }
        self.tabs.remove(self.active);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
    }
    pub fn select(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            self.active = idx;
        }
    }
}

/// 底部数字页签条：与 Sender 一样，单页不画；每页 24 点宽，选中块用正文色。
pub(in crate::ui) fn draw_tabs(win: &mut TerminalWindow, ui: &egui::Ui, rect: egui::Rect) {
    if win.script_panel.tabs.len() < 2 {
        return;
    }
    let p = ui.painter();
    let font = egui::TextStyle::Button.resolve(ui.style());
    let mut picked = None;
    for (i, tab) in win.script_panel.tabs.iter().enumerate() {
        let r = egui::Rect::from_min_size(
            egui::pos2(rect.left() + i as f32 * TAB_STRIP_H, rect.top()),
            egui::vec2(TAB_STRIP_H, TAB_STRIP_H),
        );
        let active = i == win.script_panel.active;
        let fill = if active {
            crate::theme::chrome(term_core::theme::CHROME_SCALE.dialog)
        } else {
            crate::theme::chrome(term_core::theme::CHROME_SCALE.tab)
        };
        p.rect_filled(r, egui::Rounding::ZERO, fill);
        p.text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            tab.no.to_string(),
            font.clone(),
            crate::theme::chrome(term_core::theme::CHROME_SCALE.text),
        );
        if ui
            .interact(r, egui::Id::new(("script_tab", i)), egui::Sense::click())
            .clicked()
        {
            picked = Some(i);
        }
    }
    if let Some(i) = picked {
        win.script_panel.select(i);
    }
}

