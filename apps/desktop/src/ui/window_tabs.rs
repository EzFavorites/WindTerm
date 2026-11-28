//! 标签身份与模态判定的窗口方法（从 `ui/mod.rs` 拆出，4j：单文件 500 代码行上限）。
//! 仍是 `TerminalWindow` 的固有方法；调用方路径与行为一字未改。

use super::*;

impl TerminalWindow {
    /// UDS 控制端点与窗口共享同一个池（不另建第二个池）。
    pub(crate) fn session_pool(&self) -> Arc<term_session::SessionPool> {
        self.pool.clone()
    }

    /// ADR-0025 阶段 1：往主区 `tabs` 追加一条标签，同步 `open_order`，返回其下标。
    /// 所有「打开新标签」的入口收口到这里，避免散落各文件各自写一遍登记。
    pub(in crate::ui) fn add_tab(&mut self, tab: Tab) -> usize {
        let idx = self.tabs.len();
        self.tabs.push(tab);
        self.sync_open_order();
        idx
    }

    /// ADR-0025 阶段 1：在 `tabs` 增删之后，从 `tabs` 重算 `open_order`（与 `tabs`
    /// 等长、同序），让窗口恒在身份顺序始终反映真实标签集合。集中在这一处，
    /// 漏一次增量维护就错位。`active` 的同步属阶段 2，本阶段不动。
    pub(in crate::ui) fn sync_open_order(&mut self) {
        self.open_order = self.tabs.iter().map(|t| t.identity).collect();
    }

    /// ADR-0025 阶段 2：当前活动标签的**槽位下标**，由 `active`（身份）反查。
    /// 返回值供尚未身份化的消费者（分屏树 / 面板 / 绘制）沿用，直到阶段 3 把布局
    /// 也改成按身份寻址。身份不存在时退化成 0，与旧 `active_tab` 出厂值一致；
    /// 真正越界由 `.get ` 收口，不再有「下标指空」的隐患。
    pub(in crate::ui) fn active_slot(&self) -> usize {
        match self.active {
            Some(id) => self.tabs.iter().position(|t| t.identity == id).unwrap_or(0),
            None => 0,
        }
    }

    /// ADR-0025 阶段 2：`active`（身份）的写入收口——把槽位下标翻译成身份登记。
    /// 下标越界 / 空窗口时登记 `None`，等价「没有活动标签」，比旧 `active_tab`
    /// 存一个 stale 下标更安全。
    pub(in crate::ui) fn set_active_slot(&mut self, idx: usize) {
        self.active = self.tabs.get(idx).map(|t| t.identity);
        // ④-0：单窗格时，那唯一一片窗格的 group 也要跟着 active 走，
        // 这样 split 时 `pane_of_session(self.active)` 才找得到锚点。
        self.sync_single_pane_to_active();
    }

    /// ④-0：单窗格时把那唯一一片窗格的 group 指到当前 `active` 会话。
    ///
    /// 分屏态不碰 group——每个窗格各显各的会话，切全局 active 不改叶子内容
    /// （与旧行为一致：`draw_node` 按 `pane_groups[pane].active` 渲染，而切全局
    /// active 不会重排分屏树）。这只在「单窗格」下有意义：单窗格的渲染走
    /// `draw_terminal_area`（看 `active`），分屏时看 `pane_groups[pane].active`，
    /// 两者在单窗格下必须一致，否则从单窗格分屏会锚到错误的会话。
    pub(in crate::ui) fn sync_single_pane_to_active(&mut self) {
        if !self.layout.is_single() {
            return;
        }
        let Some(pane) = self.layout.leaf_panes().into_iter().next() else {
            return;
        };
        let Some(sess) = self.active else { return };
        self.pane_groups
            .insert(pane, PaneGroup { order: vec![sess], active: sess });
    }

    /// 此刻有没有**模态对话框**立着。灰罩与弹窗必须同一口径：
    /// 少算一处，那个弹窗还是融在主窗口里；多算一处，主窗口会莫名变暗。
    ///
    /// 命令面板 / Goto / 工作台 / 预览窗是有意**不**算的：它们是浮层而非模态，
    /// 主窗口在它们开着时仍可点（工作台里就摆着「切标签 / 关标签」的按钮）。
    pub(in crate::ui) fn modal_dialog_open(&self) -> bool {
        let v = &self.view;
        // 新建会话默认在**系统独立窗口**里画（用户两次确认）：它自带
        // 窗管外框，主体不需要被压暗——遮罩是「应用内弹层」的语义。只有在
        // 应用内回退路径上（截图夹具 / 后端没有多视口）它才退化成普通模态弹窗，
        // 那时照旧加罩，否则弹窗和终端糊成一片。
        //
        // 会话属性 / 默认模板始终是应用内弹层，一律加罩。
        let new_session_in_app = match self.session_dialog {
            dialogs::SessionDialogKind::New { .. } => harness::fixture_mode(),
            _ => true,
        } && v.show_new_session;
        new_session_in_app
            || v.show_settings
            || v.show_about
            || v.show_log_settings
            || v.show_triggers
            || v.show_onekeys
            || v.show_shortcuts
            || v.show_quick_commands
            || v.show_onboarding
            || self.confirm.pending.is_some()
            || self.pending_paste.is_some()
            || self.connector.is_some()
            || self.tunnel_editor.is_some()
            || self.xfer_prompt.is_some()
            || self.script.is_some() || self.logon_script_editor.is_some()
            || self.script_ask.is_some()
            // `Option` 就是唯一的开 / 关真相：`Some` 即开，`None` 即关。
            || self.script_editor.is_some()
            || self.filer.has_modal()
    }

}
