//! 原生 SSH 的四个对话框与收线（把 `Connector` 的结果落到 tab 上）。
//!
//! 从 `ssh_connect.rs` 拆出（4j：那个文件只留纯逻辑与 `Connector` 状态机，
//! 「谁画谁」仍然各归各的文件）。这里全部是 `TerminalWindow` 的方法：
//! 开始连接 / 每帧收结果 / 四个窗口（Connecting / Unknown host / Password / Failed）。

use crate::ui::kit::field::{choice_row_styled, ChoiceKind, UnderlineStyle};
use crate::ui::dialogs::ssh_connect::{
    ask, next, options_for, Ask, Connector, Next, PendingOpen, SshAction, Step, FIRST_POLICY,
};
use crate::ui::kit::window::{DialogId, DialogWindow};
use crate::ui::*;
use term_core::onekey::OneKey;
use term_core::session::Protocol;
// ④-0：落位时要按「窗格身份」建 group。
use crate::ui::layout::PaneGroup;
use crate::ui::layout::mint_pane_identity;
use term_runtime::persist;
use term_ssh::AuthMethod as SshAuth;

/// 连接流程里的内联动作按钮：沿用裸 `ui.button` 的尺寸口径，
/// 绘制走 `kit::footer` 原语（配色/描边不再散在四个弹窗里）。
fn action_button(ui: &mut egui::Ui, label: &str) -> bool {
    use crate::ui::kit::spec::FooterButton;
    let spec = FooterButton::Plain
        .spec()
        .with_size(crate::ui::kit::footer::text_button_size(ui, label));
    crate::ui::kit::footer::widget_button(ui, label, spec, true).clicked()
}

impl TerminalWindow {
    /// 开始用内置客户端连一个 SSH 会话。
    ///
    /// `password` 是调用方手上已有的（比如刚在表单里填的）；没有就去保险库拿
    /// ——「密码存在库里」是正常情况，而不是特例。
    pub(in crate::ui) fn begin_builtin_ssh(&mut self, config: SessionConfig, password: Option<String>) {
        // 默认落在标签栏末尾——侧栏「连接」与表单「Create & Connect」都是这个落点。
        self.begin_builtin_ssh_with(config, password, PendingOpen::Append);
    }

    /// 自动重连专用的内置 SSH 发起口：除了落位，还把这次尝试标记为「自动」，
    /// 于是失败时才能按退出原因决定是否显式排下一次（手动重连失败不排）。
    pub(in crate::ui) fn begin_auto_reconnect_ssh(
        &mut self,
        config: SessionConfig,
        pending: PendingOpen,
        gen: u64,
    ) {
        self.begin_builtin_ssh_with(config, None, pending);
        if let Some(c) = self.connector.as_mut() {
            c.mark_auto(gen);
        }
        // `begin_builtin_ssh_with` 会自己再领一个号；把它退回本次尝试的代数，
        // 否则刚发起的连接立刻被 `poll_connector` 的过期检查丢掉。
        self.reconnect.set_gen(gen);
    }

    /// 同 [`begin_builtin_ssh`](Self::begin_builtin_ssh)，但指定**连上之后落在哪儿**。
    ///
    /// S1：Duplicate / Reopen / Split Group / 断线重连都要在连接完成之后才建 tab
    /// （内置 SSH 的握手是异步的），所以「落在哪儿」必须跟着这条连接一起存着，
    /// 不能在调用点当场拍一个下标——那时 tab 还不存在。
    pub(in crate::ui) fn begin_builtin_ssh_with(
        &mut self,
        config: SessionConfig,
        password: Option<String>,
        pending: PendingOpen,
    ) {
        // OneKey：会话选了一个**启用**的方案时，先按方案覆盖用户名与当前系统的
        // 私钥。这件事的**唯一实现**在控制面（`SessionPool::resolve_onekey`）——
        // 系统 `ssh` 那条路径不经过本函数，只有控制面才能同时罩住两条。
        // 这里问它要两份东西：覆盖后的配置（喂给 `ask`）和方案本身（凭据在
        // 保险库里，控制面不认识保险库）。
        // 没选 OneKey 方案（或方案为 None）时，OneKey 不参与自动登录：
        // 不覆盖用户名/私钥、不取保险库里的密码与键盘交互应答，照旧问用户。
        // 判定只有 `wants_onekey_auto_login` 一个出处——无窗口那条路径
        // （`session_open::open_session`）用的是同一个函数，不会出现
        //「界面自动登了、脚本没登」这种只在一条路径上偶发的分叉。
        let auto_login = config.wants_onekey_auto_login();
        let scheme = auto_login.then(|| self.pool.onekey_for(&config.onekey)).flatten();
        let config = if auto_login {
            self.pool.resolve_onekey(&config)
        } else {
            config
        };
        let answers = scheme
            .as_ref()
            .map(|s| onekey_answers(s, self.vault.as_ref()))
            .unwrap_or_default();
        let password = password
            .or_else(|| self.stored_password(&config.name))
            // 方案的密码在保险库里（条目 `onekey:<方案名>`）：会话自己没存密码
            // 时就用它。「改一处 OneKey，所有用它的服务器都跟着变」——这句话
            // 就落在这一句上。
            .or_else(|| {
                scheme
                    .as_ref()
                    .map(|s| self.onekey_password(&s.name))
                    .filter(|p| !p.is_empty())
            });
        let home = dirs::home_dir();
        let mut connector = match ask(&config, password.as_deref(), &answers, home.as_deref()) {
            Ask::Connect(auth) => {
                let mut c = Connector::new(config, Step::Connecting).with_pending(pending);
                c.launch(auth, &self.ctx);
                c
            }
            Ask::Password => {
                let reason = self.why_no_password(&config, scheme.is_some());
                Connector::waiting_password_with(config, reason).with_pending(pending)
            }
            // 连不上就没有「落在哪儿」这回事，但 pending 还是记着——用户可能重试。
            Ask::Unsupported(message) => Connector::failed(config, message).with_pending(pending),
        };
        // 每次发起都领一个新的 epoch。异步回调落地前会核对它，避免
        // `PendingOpen::Replace` 与「到期重连」竞态时旧回调顶掉新会话。
        connector.gen = self.reconnect.next_gen();
        // 换了目标就把旧的丢掉：同时只做一件事，避免两个对话框叠着问。
        self.connector = Some(connector);
        self.ssh_password.clear();
        self.save_password = false;
        self.ctx.request_repaint();
    }

    /// 弹密码框之前的那句原因。三种情况必须分开说，否则用户不知道该去解锁、
    /// 还是该去补方案，还是这台机器压根没存过密码。
    fn why_no_password(&self, config: &SessionConfig, has_scheme: bool) -> String {
        // 库里存着但锁着：说清楚该去哪解锁，别让人以为密码丢了。
        if persist::vault_entry(&config.name).is_some() {
            return "这个会话的密码存在保险库里，但保险库现在锁着——先在会话属性的 \
                    Authentication 页解锁，或者直接在这里输入"
                .to_string();
        }
        if has_scheme {
            return if persist::has_vault() && !self.vault_unlocked() {
                format!(
                    "这条会话的 OneKey 方案「{}」把密码存在保险库里，但保险库现在 \
                     锁着——先在会话属性的 Authentication 页解锁，或者直接在这里输入",
                    config.onekey
                )
            } else {
                format!(
                    "OneKey 方案「{}」里没有可用的密码，这条会话也没有保存密码。\
                     在 OneKeys 里给它填一个密码，或者直接在这里输入",
                    config.onekey
                )
            };
        }
        "这个会话没有保存密码（我们也不存明文）".to_string()
    }

    /// 每帧收一次后台结果。
    pub(in crate::ui) fn poll_connector(&mut self) {
        // 过期回调：`PendingOpen::Replace` 与「到期重连」可能同时指向同一槽位。
        // 落地前必须核对 epoch（是不是最新一次尝试）+ 槽位上还是不是同一条
        // 会话名（用户可能已经把它关掉 / 换成别的机器），否则旧回调会把
        // 新会话顶掉，或者把结果贴到一条不相干的标签上。
        if let Some(c) = self.connector.as_ref() {
            let slot_matches = match c.pending {
                PendingOpen::Replace { slot } => self
                    .tabs
                    .get(slot)
                    .is_some_and(|t| t.config.name == c.config.name),
                _ => true,
            };
            if self.reconnect.stale(c.gen) || !slot_matches {
                self.connector = None;
                return;
            }
        }
        let Some(connector) = self.connector.as_mut() else {
            return;
        };
        // 结果到了没有由句柄回答（`None` = 还在连）；线程死了没留话这种事
        // 也由句柄翻译成错误，这里不接触队列细节（B2）。
        let outcome = connector.rx.as_mut().and_then(|rx| rx.try_result());
        match outcome {
            Some(Ok(channel)) => {
                let connector = self.connector.take().expect("connector 刚还在");
                // 隧道要用同一份凭据再连一次，所以把 auth 一起带过去。
                let auth = connector.auth.clone();
                // 「连上之后落在哪儿」跟着这条连接一起走过来的（S1）。
                self.adopt_channel(connector.config, channel, auth, connector.pending);
            }
            Some(Err(e)) => {
                let kind = super::super::reconnect::exit_kind_for_ssh(&e);
                let pending = connector.pending;
                let auto = connector.auto;
                let name = connector.config.name.clone();
                let password_auth = connector
                    .auth
                    .as_ref()
                    .is_some_and(|a| matches!(a, SshAuth::Password { .. }));
                match next(&e, password_auth) {
                    Next::AskHostKey { fingerprint } => {
                        connector.rx = None;
                        connector.step = Step::NeedHostKey { fingerprint };
                    }
                    Next::AskPassword { reason } => {
                        connector.rx = None;
                        connector.step = Step::NeedPassword { reason };
                        self.ssh_password.clear();
                    }
                    Next::OfferReplace { changed } => {
                        // 停住，不重连：账本还是旧的，什么也没写。
                        connector.rx = None;
                        connector.step = Step::ReplaceKey { changed };
                    }
                    Next::ShowError { message } => {
                        connector.rx = None;
                        connector.step = Step::Failed { message };
                    }
                }
                // 内置 SSH 是默认路径，只处理 `make_tab` 的同步失败会把这一整类
                // 断线漏掉。这里按同一个退出分类收口：终止性立即停（并把这条
                // 标签标成 blocked，避免别的边沿又把它排上），可重试显式排下一次。
                if auto {
                    if let PendingOpen::Replace { slot } = pending {
                        if kind.allows_reconnect() {
                            self.schedule_failed_attempt(slot, &name);
                        } else if let Some(tab) = self.tabs.get_mut(slot) {
                            if tab.config.name == name {
                                tab.auto_reconnect_blocked = true;
                                tab.auto_reconnect_at = None;
                            }
                        }
                    }
                }
            }
            None => {}
        }
    }

    /// 通道到手：交给监管者，开 tab，并按 [`PendingOpen`] 落到该去的位置。
    fn adopt_channel(
        &mut self,
        config: SessionConfig,
        channel: term_ssh::SshChannel,
        auth: Option<SshAuth>,
        pending: PendingOpen,
    ) {
        let tab = self.build_ssh_tab(config.clone(), Box::new(channel), pending);
        let slot = self.place_tab(tab, pending);
        self.set_active_slot(slot);
        // R13-f：把这次用的凭据留在 tab 上，文件面板等下要用它再开一条连接。
        // 只在内存里（`SessionConfig` 不存密码，落盘的那份在保险库里）。
        if let (Some(tab), Some(auth)) = (self.tabs.get_mut(slot), auth.clone()) {
            tab.auth = Some(auth);
        }
        // 会话配置里写了转发规则，就在这一刻起隧道。
        //
        // 放在「连上之后」而不是「连接之前」：主机密钥已经由这次连接裁决过了，
        // 隧道那条连接不用再问一遍指纹；而且连不上时不该留一个只转发不干活的进程。
        self.start_session_tunnel(&config, auth.as_ref());
        self.ctx.request_repaint();
    }

    /// 建一条已连上的 SSH tab（还不放进标签栏）。
    ///
    /// `Replace` 要沿用**原来那条标签的 egui id**：id 一换，egui 就当成新控件，
    /// 滚动位置、右键菜单的悬停态全丢——用户看到的是一次闪烁而不是「连回来了」。
    fn build_ssh_tab(
        &self,
        config: SessionConfig,
        ch: Box<dyn term_runtime::channel::TermChannel>,
        pending: PendingOpen,
    ) -> Tab {
        let id = match pending {
            PendingOpen::Replace { slot } => self
                .tabs
                .get(slot)
                .map_or_else(|| egui::Id::new(format!("tab_{}", self.tabs.len())), |t| t.id),
            _ => egui::Id::new(format!("tab_{}", self.tabs.len())),
        };
        // 档 1.2：内置 SSH 的通道经**控制面**注入——协议差异留在这一层，
        // 控制面只负责监管。这是 `SessionPool::open_with_channel` 的第一个消费者。
        terminal::make_tab_with_channel(&self.pool, config, ch, id, &self.ctx)
    }

    /// 把建好的 tab 放进标签栏，返回它**实际**占的下标。
    ///
    /// 这是 S1 的落地点：所有「连上之后才存在」的入口（复制 / 重开 / 分屏 /
    /// 重连）都在这里收口，不再各自去猜一个下标。
    fn place_tab(&mut self, tab: Tab, pending: PendingOpen) -> usize {
        match pending {
            PendingOpen::Append => {
                // 单窗格时新标签即活动标签，`adopt_channel` 随后 `set_active_slot`
                // 已把唯一窗格指过来；分屏时它不塞进分屏树（与旧行为一致：Append
                // 不摆进分屏）。这里什么都不用做，group 由 set_active_slot 同步。
                self.add_tab(tab)
            }
            PendingOpen::AppendSingle => {
                let idx = self.add_tab(tab);
                // `Reopen Closed` 的语义：开回来是一个独立视图，不是塞进现有分屏。
                // ④-0：清掉旧窗格，只留这一片新窗格（含新标签对应的 group）。
                let new_id = self.tabs[idx].identity;
                let pane = mint_pane_identity();
                self.pane_groups.clear();
                self.pane_groups
                    .insert(pane, PaneGroup { order: vec![new_id], active: new_id });
                self.layout = Layout::single(pane);
                idx
            }
            PendingOpen::Split { anchor, horizontal } => {
                let idx = self.add_tab(tab);
                let new_id = self.tabs[idx].identity;
                // ④-0：分屏 = 给当前窗格复制一份配置、但开**新独立会话**（不是共享
                // `SessionId`），所以新窗格是一个新的 `PaneId` + 只含新会话的 group。
                let new_pane = mint_pane_identity();
                self.pane_groups.insert(
                    new_pane,
                    PaneGroup { order: vec![new_id], active: new_id },
                );
                // `anchor` 已经是窗格身份（`split_view` 在落地前翻译好的）；若分屏期间
                // 锚点窗格被关掉，`split` 会退化成「整棵树 + 新叶子」，不会丢已有窗格。
                self.layout.split(anchor, horizontal, new_pane);
                // 结构变了，本帧末即时落盘（内置 SSH 分屏这类异步落地的入口）。
                self.pending_save_layout = true;
                idx
            }
            // 重连：位置不变，只换会话。先把旧的那条真关掉——它是断开态时
            // 这就是空操作，但别指望它一定是断开的。
            PendingOpen::Replace { slot } if slot < self.tabs.len() => {
                self.tabs[slot].close();
                self.tabs[slot] = tab;
                self.sync_open_order();
                slot
            }
            // 连上之前那条标签已经被关掉了：退化成追加，而不是把会话丢在地上。
            PendingOpen::Replace { .. } => {
                self.add_tab(tab)
            }
        }
    }

    /// 「重新连接会话」落到**当前标签**上（S1 的另一半）。
    ///
    /// 以前它调 `connect_selected_session`——连的是**侧栏里选中**的那一条。
    /// 于是用户在一个断开的标签上点 Reconnect，连的却是侧栏高亮的另一台机器；
    /// 更糟的是那条可能已经存在，于是这次点击什么也没发生（按名字撞车直接
    /// 激活旧标签）。菜单项写的是 "Reconnect Session"，指的就是眼前这条。
    pub(in crate::ui) fn reconnect_active_tab(&mut self) {
        let slot = self.active_slot();
        let gen = self.reconnect.next_gen();
        self.reconnect_tab_at(slot, true, gen);
    }

    /// 按**槽位**重连。`activate=false` 给自动重连用：后台那条断开的标签
    /// 在原位复活，不抢用户当前正在看的另一条。`gen` 是这次尝试的代数，
    /// 异步 SSH 回调落地前会核对它，挡住 Replace 与到期重连的竞态。
    pub(in crate::ui) fn reconnect_tab_at(&mut self, slot: usize, activate: bool, gen: u64) {
        let Some(mut cfg) = self.tabs.get(slot).map(|t| t.config.clone()) else {
            return;
        };
        // 串口设备可能在两次尝试之间被拔掉又插回（Linux 的 by-id 路径稳定，
        // `list_devices()` 已把它排在内核名前面）。旧路径还在就继续用，不在
        // 才换到当前枚举到的第一台——绝不让重连对着一个不存在的路径空转。
        if cfg.protocol == Protocol::Serial {
            let devices = term_io::serial::list_devices();
            if !devices.is_empty() && !devices.contains(&cfg.host) {
                cfg.host = devices[0].clone();
            }
        }
        // 退避次数跨 `Replace` 传递：新建的 tab 出厂 attempts=0，不接过来就
        // 会永远从第 1 秒重来，180s 窗口内的延迟序列断掉。
        let old_attempts = self.tabs[slot].auto_reconnect_attempts;
        let old_blocked = self.tabs[slot].auto_reconnect_blocked;
        let old_identity = self.tabs[slot].identity;
        // 活着的先按「断开」处理：Reconnect 的语义是重来一次，不是再开一条。
        // 这一步同时收掉隧道与文件面板——它们不该比会话活得久。
        if self.tabs[slot].session.is_some() && activate {
            // 手动 Reconnect 才需要先断开；自动重连走到这里时本来就是断开态。
            self.disconnect_active_tab();
        }
        // N：保存待恢复的运行态（新 tab 会替换旧的，字段要在新 tab 上重建）
        let restore_scroll_top = self.tabs[slot].restore_scroll_top.take();
        let restore_history_len = self.tabs[slot].restore_history_len.take();
        if panels::sidebar::uses_builtin_ssh(&cfg) {
            // 内置 SSH 走异步连接：旧 tab 留在原位，失败回调在新 tab 建起来之前
            // 就能按同一个 slot / 会话名 / epoch 把下一次尝试排上去。
            self.begin_auto_reconnect_ssh(cfg, PendingOpen::Replace { slot }, gen);
            return;
        }
        // 系统 ssh / 串口：同步起，不需要异步流程，但同样**原地替换**。
        let mut tab = terminal::make_tab(&self.pool, cfg, self.tabs[slot].id, &self.ctx);
        tab.identity = old_identity;
        tab.auto_reconnect_attempts = old_attempts;
        tab.auto_reconnect_blocked = old_blocked;
        // N：应用恢复态（make_tab 里对存活会话已自动应用，这里是为了手动 Reconnect 场景）
        tab.restore_scroll_top = restore_scroll_top;
        tab.restore_history_len = restore_history_len;
        tab.maybe_apply_restore();
        // 同步建连失败也必须有下一次：新 tab 的 was_live=false 永远不会产生
        // 「断开边沿」，不显式排的话 180s 窗口第一次就断了。
        if !activate && !old_blocked && tab.session.is_none() {
            let name = tab.config.name.clone();
            self.place_tab(tab, PendingOpen::Replace { slot });
            self.schedule_failed_attempt(slot, &name);
        } else {
            let slot_idx = self.place_tab(tab, PendingOpen::Replace { slot });
            if activate {
                self.set_active_slot(slot_idx);
            }
        }
        self.ctx.request_repaint();
    }

    /// 状态栏那一行要显示什么（`None` = 没有进行中的连接）。
    pub(in crate::ui) fn connecting_label(&self) -> Option<String> {
        let c = self.connector.as_ref()?;
        match &c.step {
            Step::Connecting => {
                // 这一句只为了**日志与状态栏**（`description ` 里没有密码、没有私钥
                // 路径）。跳板链书写有误时拼不出 `SshOptions`，但那时错误正显示在
                // 失败窗口里——状态栏没必要跟着变成一片空白，退成 host:port 即可。
                let target = match options_for(&c.config, SshAuth::Agent, FIRST_POLICY) {
                    Ok(opts) => opts.description(),
                    Err(_) => format!("{}:{}", c.config.host.trim(), c.config.port),
                };
                Some(format!("Connecting to {target}…"))
            }
            Step::NeedHostKey { .. } => Some("Waiting for host key confirmation".to_string()),
            Step::NeedPassword { .. } => Some("Waiting for a password".to_string()),
            Step::ReplaceKey { .. } => Some("Host key changed — replace the stored key?".to_string()),
            Step::Failed { .. } => Some("Connection failed".to_string()),
        }
    }

    pub(in crate::ui) fn draw_ssh_dialogs(&mut self, ctx: &egui::Context) {
        let Some(connector) = self.connector.as_ref() else {
            return;
        };
        let title = format!("{} — {}", connector.config.name, connector.config.host);
        let mut action = SshAction::None;
        // Esc 一律走「取消连接」；主机密钥 / 替换密钥两步**不接 Enter**，
        // 避免误点安全确认。密码框的 Enter 仍走它自己的 `lost_focus()` 路径。
        let step_id = match &connector.step {
            Step::Connecting => DialogId::SshConnecting,
            Step::NeedHostKey { .. } => DialogId::SshHostkey,
            Step::NeedPassword { .. } => DialogId::SshPassword,
            Step::Failed { .. } => DialogId::SshFailed,
            Step::ReplaceKey { .. } => DialogId::SshReplaceKey,
        };
        if step_id.keys().poll(ctx).cancel {
            action = SshAction::Cancel;
        }

        match &connector.step {
            Step::Connecting => {
                // 稳定 id：标题带会话名与主机，两者都可能变（重连接别的机器时）。
                DialogWindow::new(DialogId::SshConnecting, &title)
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Connecting…");
                        });
                        ui.add_space(6.0);
                        if action_button(ui, "Cancel") {
                            action = SshAction::Cancel;
                        }
                    });
            }
            Step::NeedHostKey { fingerprint } => {
                DialogWindow::new(DialogId::SshHostkey, &title)
                    .without_title_bar()
                    .show(ctx, |ui| {
                        ui.heading("Unknown host");
                        ui.label(format!(
                            "This is the first time connecting to {}. Confirm the fingerprint \
                             matches what the server administrator gave you:",
                            connector.config.host
                        ));
                        ui.add_space(4.0);
                        ui.monospace(fingerprint.clone());
                        ui.add_space(6.0);
                        ui.label(
                            "Choosing Trust writes this key into ~/.ssh/known_hosts. \
                             If it ever changes afterwards, the connection is refused.",
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if action_button(ui, "Cancel") {
                                action = SshAction::Cancel;
                            }
                            if ui.button("Trust and connect").clicked() {
                                action = SshAction::Trust;
                            }
                        });
                    });
            }
            Step::NeedPassword { reason } => {
                // 「能不能存」在进窗口之前问清楚：`vault_unlocked ` 借的是整个
                // `self`，而下面要可变借 `ssh_password` / `save_password`。
                let can_save = self.vault_unlocked();
                let password = &mut self.ssh_password;
                let save = &mut self.save_password;
                DialogWindow::new(DialogId::SshPassword, &title)
                    .without_title_bar()
                    .show(ctx, |ui| {
                        ui.label(reason.clone());
                        ui.add_space(6.0);
                        ui.label("Password:");
                        let resp = ui.add(
                            TextEdit::singleline(password)
                                .password(true)
                                .desired_width(ui.available_width())
                                .hint_text("not saved to disk"),
                        );
                        let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        ui.add_space(4.0);
                        ui.add_enabled_ui(can_save, |ui| {
                            choice_row_styled(ui, ChoiceKind::Checkbox, "Save in vault", "", save, true, UnderlineStyle::None);
                        });
                        if !can_save {
                            ui.weak(
                                "The credentials vault is locked — unlock it in \
                                 Properties → Authentication to remember this password.",
                            );
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if action_button(ui, "Cancel") {
                                action = SshAction::Cancel;
                            }
                            if action_button(ui, "Connect")
                                || (entered && !password.is_empty())
                            {
                                action = SshAction::SubmitPassword;
                            }
                        });
                    });
            }
            Step::Failed { message } => {
                DialogWindow::new(DialogId::SshFailed, &title)
                    .show(ctx, |ui| {
                        ui.heading("Connection failed");
                        ui.label(message.clone());
                        ui.add_space(8.0);
                        if ui.button("Close").clicked() {
                            action = SshAction::Cancel;
                        }
                    });
            }
            Step::ReplaceKey { changed } => {
                DialogWindow::new(DialogId::SshReplaceKey, &title)
                    .show(ctx, |ui| {
                        ui.heading("Host key changed");
                        ui.label(format!(
                            "The key recorded for {}:{} (known_hosts line {}) is not the one \
                             this server just offered. Either the machine was reinstalled, \
                             or this connection is not going where you think it is.",
                            changed.host, changed.port, changed.line
                        ));
                        ui.add_space(4.0);
                        ui.monospace(changed.fingerprint.clone());
                        ui.add_space(6.0);
                        // 这一段是这个窗口存在的理由：说清「现在什么都没写」和
                        // 「什么情况下才该替换」。不写「是否继续」那种问句。
                        ui.label(
                            "Nothing has been written yet. Replace the record only if you \
                             got this fingerprint from the server's administrator, or you \
                             know the machine was reinstalled. The old line is deleted.",
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if action_button(ui, "Cancel") {
                                action = SshAction::Cancel;
                            }
                            if action_button(ui, "Replace stored key") {
                                action = SshAction::ReplaceKey;
                            }
                        });
                    });
            }
        }

        // 先收集、后执行：绘制期间已经借了 `self.connector` 与 `self.ssh_password`。
        match action {
            SshAction::None => {}
            SshAction::Cancel => {
                self.connector = None;
                self.ssh_password.clear();
            }
            SshAction::Trust => {
                let ctx = self.ctx.clone();
                if let Some(c) = self.connector.as_mut() {
                    c.accept_host_key(&ctx);
                }
            }
            SshAction::ReplaceKey => {
                // 危险动作：改账本 + 用 Strict 重连。失败会落到 `Step::Failed`，
                // 所以这里不需要再报一次错。
                let ctx = self.ctx.clone();
                if let Some(c) = self.connector.as_mut() {
                    c.replace_host_key(&ctx);
                }
            }
            SshAction::SubmitPassword => {
                let password = std::mem::take(&mut self.ssh_password);
                let save = std::mem::take(&mut self.save_password);
                // 先存（用户勾了才存），再连：存失败不该拦住这次连接——密码就在
                // 手上，连上比「记住它」重要。错误留在属性页那块状态里。
                if save {
                    if let Some(name) = self.connector.as_ref().map(|c| c.config.name.clone()) {
                        if let Err(e) = self.remember_password(&name, &password) {
                            self.vault_form.error = Some(format!("保存密码失败：{e}"));
                        }
                    }
                }
                let ctx = self.ctx.clone();
                if let Some(c) = self.connector.as_mut() {
                    c.submit_password(password, &ctx);
                }
            }
        }
    }
}

/// OneKey 方案里键盘交互应答表的内容，按服务端提示顺序。
///
/// 编辑页按 `+` 加行、`-` 删行，中间不留空是一位用户很难做到的事，所以
/// **在第一个空应答处停**：`term-ssh` 是按下标取答案的，空串会被原样发出去
/// （服务端只会拒绝一次），截断比乱填诚实。
///
/// 只读保险库：应答存在 `onekey:<name>:ki`（JSON 数组），锁着或没有条目时
/// 回落方案里的旧明文答复，兼容还没迁移的会话。
fn onekey_answers(scheme: &OneKey, vault: Option<&term_vault::Vault>) -> Vec<String> {
    if let Some(vault) = vault {
        let id = term_core::onekey::ki_vault_id(&scheme.name);
        if let Some(answers) = vault
            .find(&id)
            .and_then(|e| e.secret())
            .map(term_core::onekey::decode_ki_answers)
        {
            if !answers.is_empty() {
                return answers;
            }
        }
    }
    scheme
        .keyboard_interactive
        .iter()
        .map(|pair| pair.response.as_str())
        .take_while(|response| !response.is_empty())
        .map(str::to_string)
        .collect()
}
