//! `Effect::Filter` 的落地：状态属于每条会话，不碰 `ViewState`。
//!
//! 从 `effects.rs` 拆出，既让那个文件的代码行守住 500 门禁，也让「过滤动作
//! 只改指定 tab」这条纪律有一个独立的、可单测的落点。

use crate::ui::*;

impl TerminalWindow {
    /// `Action::Filter` 的唯一落地点：对**活动 tab** 调 `filter::reduce`。
    pub(in crate::ui) fn apply_filter(&mut self, action: term_app::vocab::FilterAction) {
        let Some(session) = self.tabs.get(self.active_slot()).map(|t| t.identity) else {
            return;
        };
        self.apply_filter_on(session, action);
    }

    /// 对**指定会话**落地一次过滤动作（分屏时每个 pane 只操作自己那条会话）。
    /// `filter_input` 与已提交的 `filter.query` 同源，不留两份真相。
    pub(in crate::ui) fn apply_filter_on(
        &mut self,
        session: term_core::session_runtime::SessionId,
        action: term_app::vocab::FilterAction,
    ) {
        let Some(slot) = self.tabs.iter().position(|t| t.identity == session) else {
            return;
        };
        let tab = &mut self.tabs[slot];
        term_app::filter::reduce(&mut tab.filter, action);
        tab.filter_input = tab.filter.query.clone();
        self.ctx.request_repaint();
    }
}
