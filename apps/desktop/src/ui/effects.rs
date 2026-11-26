//! Effect 落地（副作用只在这里发生）。
//!
//! `ui/mod.rs` 只负责装配与 draw 顺序；这里承接窗口 / 会话 / 缩放 / 选区
//! 相关的副作用，避免新增命令把 mod.rs 重新堆回之前的规模。
//!
//! FindState 只由 reducer 写。`find_from_selection` 拿到的是每帧从事实
//! 窗口推导的 `selection_text`（派生值，不是状态），它把「填词 + 跳一步」
//! 派发回 `dispatch_action`，由 term-app 的 find reducer 落状态。

use super::{TerminalWindow, WindowOpacity};
use crate::ui::dialogs::SessionDialogKind;
use eframe::egui;
use term_app::action::{Action, Effect, XferCmd};
use term_app::blocks; use term_app::find::FindAction;
use term_app::selection::{Pos, Selection};
use term_app::shell::ShellAction;
use term_app::tab::TabAction;
use term_core::session::{Protocol, SessionConfig};
use term_core::vt::paste_bytes_for;

// 折叠族九条的决策与落地在子模块里：它自带约 280 行（含注释），留在本文件
// 会把这里顶过 500 代码行上限。
mod debug_log;
mod fold;
mod mode;
mod theme;

impl TerminalWindow {
    /// 帧末批量落地：同步输入广播 + 折叠列点击。两项都要借整个窗口，
    /// 而调用点在 `Tab` 的借用之后——攒成一条调用，避免 draw 函数膨胀。
    pub(super) fn flush_deferred(
        &mut self,
        sync_bytes: Option<Vec<u8>>,
        pending_fold: Option<term_app::vocab::FoldCmd>,
    ) {
        if let Some(bytes) = sync_bytes {
            self.broadcast_input(&bytes);
        }
        if let Some(cmd) = pending_fold {
            self.apply_fold(cmd);
        }
    }

    /// `prefs.focus_new_tab`：新标签建好之后要不要把焦点切过去。
    ///
    /// 出厂行为就是「切过去」（`focus_new_tab = true`）。关掉时**不动
    /// `active_tab`**，但必须把它夹回存在的范围——有几个调用点在这之前刚改过
    /// 标签列表，旧下标可能已经越界；不夹的话整屏会变成「No active session.」，
    /// 而用户只是取消了一个勾选。
    ///
    /// 三个落地点共用这一条：分屏新开（`layout::split`）、会话树连接
    /// （`panels::sidebar`）、复制标签（`shell::tab_menu`）。`Reopen Closed`
    /// 不走它：它同时把布局树换成 `Layout::single(new_tab)`，不切过去就会出现
    /// 「活动标签在树上没有叶子」，那是结构不一致，不是焦点偏好。
    pub(in crate::ui) fn activate_new_tab(&mut self, new_tab: usize) {
        self.set_active_slot(next_active_tab(self.active_slot(), new_tab, self.tabs.len(), self.focus_new_tab));
    }

    /// 循环切到相对位置的标签（+1 / -1，到头回绕）。
    pub(super) fn activate_tab(&mut self, delta: i32) {
        let n = self.tabs.len();
        if n == 0 { return; }
        let n = n as i32;
        let idx = (((self.active_slot() as i32 + delta) % n) + n) % n;
        self.set_active_slot(idx as usize);
    }

    /// 会话名是纯属性，不需要选区。
    pub(super) fn copy_session_name(&self) {
        if let Some(tab) = self.tabs.get(self.active_slot()) { self.ctx.copy_text(tab.config.name.clone()); }
    }

    /// 连接串同样来自配置；Shell 本地会话退化成名字（没有 URL 可讲）。
    pub(super) fn copy_session_url(&self) {
        if let Some(tab) = self.tabs.get(self.active_slot()) { self.ctx.copy_text(session_url(&tab.config)); }
    }

    pub(super) fn open_help(&self) {
        let _ = crate::sysopen::open(std::path::Path::new(HELP_URL));
    }

    /// 用当前选区文本发起一次网络搜索。
    ///
    /// 模板来自 `prefs.search_url`（空串 → `search::default_url()`），由
    /// `Preferences ▸ Search` 维护；这里不再自己写死一个引擎——那样设置页
    /// 改了也搜不到，「同一件事两份数据」正是要消灭的东西。
    pub(super) fn search_online(&self) {
        if self.selection_text.trim().is_empty() {
            return;
        }
        // prefs 只在这一个表达式里借着：拼完 URL 就让位给 `sysopen`。
        let url = {
            let prefs = term_runtime::prefs::load().unwrap_or_default();
            term_runtime::search::build_url(prefs.search_url_template(), &self.selection_text)
        };
        let _ = crate::sysopen::open(std::path::Path::new(&url));
    }

    /// 缩放是视觉调整，不进 ViewState；下一帧 install 会因尺寸变化重装字形。
    pub(super) fn zoom_font(&mut self, dir: i8) {
        self.fonts.zoom(dir);
        self.ctx.request_repaint();
    }

    /// 以选区文本为查询词，但状态只由 reducer 写。
    pub(super) fn find_from_selection(&mut self, dir: i8) {
        self.dispatch_action(Action::Find(FindAction::SetQuery(
            self.selection_text.clone(),
        )));
        let step = if dir > 0 { FindAction::Next } else { FindAction::Previous };
        self.dispatch_action(Action::Find(step));
    }

    /// 同步输入。把一批字节写给**除当前标签外**所有活着的会话。
    ///
    /// 只写有 PTY 句柄的标签：断开的、只剩快照的标签没有输入目标，
    /// 静默跳过比报错更合适（用户看到的 `SYNC` 徽标说明的是「生效范围」）。
    pub(super) fn broadcast_input(&self, bytes: &[u8]) {
        let active_id = self.tabs.get(self.active_slot()).map(|t| t.identity);
        let recipients = active_id
            .map(|id| term_app::sync_input::recipients(&self.view, id))
            .unwrap_or_default();
        let has_members = self.view.sync_channels.iter().any(|c| !c.is_empty());
        for tab in &self.tabs {
            if Some(tab.identity) == active_id {
                continue;
            }
            // 老 prefs 只有 `sync_input=true`、没有成员表时沿用「所有标签」；
            // 有成员表就只写它在的通道里的其它成员——用户能挑「只同步这几台」。
            if has_members && !recipients.contains(&tab.identity) {
                continue;
            }
            tab.write(bytes);
        }
    }

    /// 选中最近一条命令（`Text.SelectCommand`）。
    ///
    /// 边界来自内核记下的 OSC 133 行号；没有集成就什么都不选，并给一句提示——
    /// 猜一个「大概是这一行」的边界比不做事更糟。
    pub(super) fn select_command(&mut self) {
        let active = self.active_slot();
        let Some(tab) = self.tabs.get_mut(active) else {
            return;
        };
        let Some(rows) = blocks::typed_command_rows(&tab.snapshot.blocks, tab.snapshot.history)
        else {
            self.log_note = Some("Select Command 需要 shell 的 OSC 133 集成".to_string());
            return;
        };
        let Some((start, end)) = blocks::clamp_to_image(rows, &tab.view) else {
            self.log_note = Some("那条命令已经滚出视口".to_string());
            return;
        };
        // 尾行的列数必须从**这一行自己**取，不能靠「窗口内行号连续」去算下标。
        // 旧写法是 `content_row_to_screen_index(ViewRow::new(end), view.start())`，
        // 它假设 `lines[i].row == range.start + i` 恒成立；折叠一生效，被折的行
        // 从窗口里被 `drop_rows` 丢掉，行号就带洞了，那个下标会指到**别的行**上
        // ——off-by-N 之后选区的尾列数取自错误的行，肉眼表现为「选中少了几个字符」。
        // 行号连续时逐值等价（`position` 找到的正是同一个下标），
        // 带洞时唯一正确的做法就是按行号查。
        let last_col = tab
            .view
            .lines
            .iter()
            .find(|line| line.row == end)
            .map(|line| line.cells.len())
            .unwrap_or(0);
        // 这是一次「新选一段」——集合换成这一份，旧的几份让位。
        tab.selections
            .set_active(Some(Selection::span(Pos::new(start, 0), Pos::new(end, last_col))));
    }

    /// 「复制当前命令」：把 OSC 133 标记的那条命令整段拷进剪贴板。
    ///
    /// 与 [`Self::select_command`] 共用同一份边界推导（`typed_command_rows` +
    /// `clamp_to_image`），差别只在拿到范围后**取文本写剪贴板**而不是设选区。
    /// 不先改选区再读 `selection_text`——那是每帧派生值，同一帧里还没算出来，
    /// 读到的会是上一帧的旧内容。这里直接从 `tab.view` 按 span 取。
    ///
    /// 没有 OSC 133 集成 / 命令已滚出视口时给一句提示，不静默复制空串。
    pub(super) fn copy_command(&mut self) {
        let active = self.active_slot();
        let text = {
            let Some(tab) = self.tabs.get_mut(active) else {
                return;
            };
            let Some(rows) =
                blocks::typed_command_rows(&tab.snapshot.blocks, tab.snapshot.history)
            else {
                self.log_note = Some("Copy Command 需要 shell 的 OSC 133 集成".to_string());
                return;
            };
            let Some((start, end)) = blocks::clamp_to_image(rows, &tab.view) else {
                self.log_note = Some("那条命令已经滚出视口".to_string());
                return;
            };
            let last_col = tab
                .view
                .lines
                .iter()
                .find(|line| line.row == end)
                .map(|line| line.cells.len())
                .unwrap_or(0);
            let span = Selection::span(Pos::new(start, 0), Pos::new(end, last_col));
            span.text(&tab.view, &tab.snapshot.wraps)
        };
        if !text.is_empty() {
            self.ctx.copy_text(text);
        }
    }

    /// 应用级退出。只向窗口系统发一次 Close，不在这里 process::exit——
    /// 那样会跳过 eframe 的收尾（会话句柄、隧道、日志写入端都要正常 drop）。
    ///
    /// 2026-11-08：本入口已被 `dialogs::confirm` 拦截过一次（Yes 之后才到这），
    /// 所以这里直接置 `close_confirmed` 放行，不再弹第二次。
    pub(super) fn exit_application(&mut self) {
        // 退出前把结构落盘，下次启动才能还原（E-RESTORE）。
        self.save_layout();
        self.confirm.close_confirmed = true;
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// 关掉当前标签。与标签栏 × 共用 `ui/tabs.rs` 的 `close_tab`。
    ///
    /// 2026-11-08：先经关闭确认弹窗（Yes 才真关，No 就地返回）；
    /// 勾了「不再提示」的直接走。2026-11-10：门控与另外两处共用
    /// `confirm::should_confirm_close`（`prefs.confirm_close_multi_tabs`，原 Behaviour 页
    /// 已移除；关多标签的粒度只在该函数里判一次）。
    pub(super) fn close_active_view(&mut self) {
        let i = self.active_slot();
        if crate::ui::dialogs::confirm::should_confirm_close(
            self.tabs.len(),
            self.confirm.suppress_session,
            self.confirm.multi_tabs,
        ) {
            let Some(kind) = crate::ui::dialogs::confirm::tab_at(&self.tabs, i) else {
                return;
            };
            self.confirm.pending = Some(kind);
            self.confirm.check = false;
            return;
        }
        self.close_tab(i);
        self.save_layout();
    }

    /// 关掉全部标签（关空后按既有约定补一条本地 shell）。
    ///
    /// 同一条判据：`tabs.len() > 1` 时才看「关多标签要不要确认」那个字段；
    /// 只剩一条时它管的不是 multiple，仍按「不再提示」那一位决定。
    pub(super) fn close_all_views(&mut self) {
        if crate::ui::dialogs::confirm::should_confirm_close(
            self.tabs.len(),
            self.confirm.suppress_session,
            self.confirm.multi_tabs,
        ) {
            self.confirm.pending = Some(crate::ui::dialogs::confirm::ConfirmClose::AllTabs);
            self.confirm.check = false;
            return;
        }
        self.confirm_close_all_views();
    }

    /// 把回看绝对行号放到活动标签视口顶部。
    ///
    /// 行号由 reducer 从状态里解析（`goto::parse_line`），这里只负责把它交给
    /// `Viewport::jump_to_row`——上限来自内核报告的 `snapshot.history`，不自己猜。
    /// 清掉上一次范围请求让 `ensure_view` 立刻按新区间重发事实窗口。
    pub(super) fn goto_line(&mut self, row: i64) {
        let active = self.active_slot();
        if let Some(tab) = self.tabs.get_mut(active) {
            let max = tab.snapshot.history as i64;
            tab.viewport.jump_to_row(row, max);
            tab.last_request = None;
        }
        self.ctx.request_repaint();
    }

    /// 切到第几个已打开的标签。索引越界时安静忽略（列表可能刚变短）。
    pub(super) fn goto_session(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            self.set_active_slot(idx);
        }
        self.ctx.request_repaint();
    }

    /// 快捷命令栏把命令文本送进活动会话的输入行。
    ///
    /// **不追加回车**：这一栏是「送文本」，不是「执行」，见 `Action::QuickCommand`。
    /// 编码走 `paste_bytes_for`，与 `Effect::PasteSelection` 同一条路径——括号
    /// 粘贴与控制字符过滤在不同终端模式下因此只有一份行为。
    /// 没有活动 PTY（夹具 / 已断开）时静默，与既有粘贴路径一致。
    pub(super) fn send_command(&mut self, text: &str) {
        if text.is_empty() { return; }
        // 「按钮栏」：快速命令里写一条 `Script.RunFile:<路径>`，点它就
        // 是一个脚本按钮——与映射键同一个命令 ID，三条入口一套写法。
        //
        // 在这里拦而不是在快速命令栏那一层：那一层只产出 `Action`（`ui/` 里也不许
        // 出现命令 ID 字面量），而这里是「一串要发出去的文字」的唯一漏斗。
        if let Some(path) = crate::script_run::run_file_target(text) {
            self.open_script_for_path(path);
            return;
        }
        if let Some(tab) = self.tabs.get(self.active_slot()) {
            // 快捷命令是一次有意输入的按键序列，不是粘贴文本：`\n` / `\x03`
            // 等转义要解码成真字节。paste_bytes_for 会剥掉控制字符，不能走那条。
            let bytes = term_core::keys::decode_send_text(text);
            tab.write(&bytes);
        }
        // 点按钮把焦点从终端带走了：送完文本要还回去，用户才能接着敲参数。
        self.restore_focus = true;
    }

    pub(super) fn apply_effect(&mut self, effect: Effect) {
        match effect {
            Effect::Disconnect => self.disconnect_active_tab(),
            // 「重新连接会话」指的就是眼前这条。以前它连侧栏选中的那一条
            // ——用户在一个断开的标签上点 Reconnect，连的却是另一台机器。
            Effect::Reconnect => self.reconnect_active_tab(),
            // 菜单开新会话：只清掉上次「属性」的编辑目标，免得改到那条会话。
            Effect::OpenNewSession => self.session_dialog = SessionDialogKind::New { group: None },
            Effect::OpenSettings | Effect::ShowAbout => {
                // 对话框开关已由 reduce 写入 ViewState，无需额外副作用。
            }
            Effect::OpenDefaultSessionSettings => {
                // 把默认模板填进与 New/Edit 共用的会话弹窗；保存语义（只写回
                // 默认模板、不连接）由 `save_session_dialog` 按 `session_dialog`
                // 分派，无需在这里再判断。
                self.open_default_session_settings();
            }
            Effect::OpenActiveSessionSettings => {
                // 打开当前选中会话的属性对话框——与侧边栏右键 `Properties`
                // 同一条落地路径。没有选中会话时静默无操作，不报错。
                if self.selected_session < self.sessions.len() {
                    self.open_session_properties(self.selected_session);
                }
            }
            Effect::CopySelection => {
                // 文本已在绘制时从事实窗口提取；这里只做「写剪贴板」这一件副作用。
                if !self.selection_text.is_empty() { self.ctx.copy_text(self.selection_text.clone()); }
            }
            // `Cmd::Move` 与 `Cmd::Set`（集合命令）共用一个待办槽位——
            // 落地在 `ui/terminal/pane.rs`：只有那一处手上有本帧的事实窗口。
            Effect::Select(cmd) => self.pending_select = Some(cmd),
            Effect::Log(action) => self.apply_log(action),
            Effect::Paste => {
                // 平台剪贴板读由框架代劳。egui 0.29 只暴露写接口，
                // 但 `RequestPaste` 会让 eframe 去读系统剪贴板，并在下一帧
                // 把结果作为 `Event::Paste` 投回输入队列——和用户按 Cmd+V
                // 完全同一条路径，因此换行/控制字符的处理也只有一份。
                self.ctx
                    .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                // 点菜单已经让终端失去焦点，而粘贴事件下一帧才到。
                self.restore_focus = true;
            }
            // ── Goto 的两条跳转意图 ──
            Effect::GotoLine(row) => self.goto_line(row),
            Effect::GotoSession(idx) => self.goto_session(idx),
            // ── 分屏六条命令，落地都在 `ui/layout.rs` ──
            Effect::SplitView(horizontal) => self.split_view(horizontal, false),
            Effect::SplitGroup(horizontal) => self.split_view(horizontal, true),
            Effect::CloseActiveGroup => self.close_active_group(),
            Effect::MoveGroupToGroup => self.move_active_to_next_group(),
            // ── 应用级退出与标签页关闭 ──
            // `Application.NewWindow`：`#[must_use]` 保证 plan 一定被 `launch()`
            // 消费，失败落到状态栏的 `log_note`（不许静默失败）。
            Effect::NewWindow => match crate::new_window::new_window_plan().launch() {
                Ok(()) => {}
                Err(err) => {
                    self.log_note = Some(err.message());
                    self.ctx.request_repaint();
                }
            },
            Effect::Exit => self.exit_application(),
            Effect::CloseActiveTab => self.close_active_view(),
            Effect::CloseAllTabs => self.close_all_views(),
            // ── 窗口 / 会话 / 缩放 / 选区相关副作用 ──
            Effect::ActivateTab(delta) => self.activate_tab(delta),
            Effect::CopySessionName => self.copy_session_name(),
            Effect::CopySessionUrl => self.copy_session_url(),
            Effect::OpenHelp => self.open_help(),
            Effect::SearchOnline => self.search_online(),
            Effect::Zoom(dir) => self.zoom_font(dir),
            // FindState 只由 reducer 写；查询词是每帧推导的派生值，
            // 因此把「填词 + 跳一步」派发回 dispatch_action 落状态。
            Effect::FindSelection(dir) => self.find_from_selection(dir),
            Effect::PasteSelection => {
                // 把当前选区文本写给 PTY，编码与 Effect::Paste 完全同一条路径。
                let text = self.selection_text.clone();
                if !text.is_empty() {
                    if let Some(tab) = self.tabs.get(self.active_slot()) {
                        let bytes = paste_bytes_for(&text, tab.snapshot.bracketed_paste);
                        tab.write(bytes.as_bytes());
                    }
                }
            }
            // ── 导入与布局持久化 ──
            Effect::ImportSessions(source) => self.import_sessions(source),
            Effect::SaveLayout => self.save_layout(),
            // 窗口开关已由 reducer 写进 ViewState，这里没有额外副作用。
            Effect::OpenShortcuts => {}
            // 选中最近一条命令（块边界来自内核的 OSC 133 记录）。
            Effect::SelectCommand => self.select_command(),
            // ── tmux 反向控制命令，编码与粘贴同一条路径 ──
            Effect::TmuxCommand(cmd) => self.tmux_send(&cmd),
            Effect::TmuxFeed(line) => self.tmux_feed(&line),
            // 布局。`DockLayout` 是投影层的视图结构，reducer 不认识它，
            // 所以停靠 / 浮动的落地只能在这里（与 `SplitView` 同一条分工）。
            Effect::Layout(action) => self.apply_layout_action(action),
            // 快捷命令栏。
            Effect::SendCommand(text) => self.send_command(&text),
            // ── 2026-11-09：文件传输 ──
            // 取消没有输入要填，直接落到句柄；发 / 收要先问路径，开弹层。
            // 菜单点下去时**没有**路径可传（`XferHandle::start` 要 target），
            // 所以这里既不假装开始，也不报错——只把弹层打开。
            Effect::Xfer(XferCmd::Cancel) => {
                if let Some(job) = self.xfer.as_ref() {
                    job.handle.cancel();
                }
            }
            Effect::Xfer(cmd) => {
                // 菜单、`modem_use_default` 直启共用这一处：弹层出厂值必须来自
                // 活动会话，否则属性页选了策略/打开目录却完全不生效。
                let (idx, open) = self.tabs.get(self.active_slot())
                    .map(|t| (t.config.modem_conflict_policy, t.config.modem_open_after))
                    .unwrap_or((0, false));
                self.xfer_prompt = xfer_prompt(cmd, idx, open);
            }
            // 标签右键只发 `Action::Tab`，落地全在这一处。
            Effect::Tab(action) => self.apply_tab_action(action),
            // 显示模式。reducer 只声明「切一下」（它不认识会话），
            // 翻哪一位、存哪儿都在这里——见 `toggle_hex_mode`。
            Effect::ToggleHex => self.toggle_hex_mode(),
            // 调试覆盖层：与上一条同构——reducer 只声明「切一下」，翻哪一位、
            // 存哪儿都在 `toggle_debug_mode`。**屏幕变化**在
            // `terminal/debug_paint.rs`，本函数只翻 bool 并请一次重绘。
            Effect::ToggleDebug => self.toggle_debug_mode(),
            // 只读查看（`Window ▸ Local Mode`）：翻一位 + 打开时取消在跑的脚本。
            // 四条消费者（write / poll / pane / paint_cursor）读的都是这一位，
            // 所以本函数只负责翻它，别再在这里顺手改别的东西。
            Effect::ToggleLocalMode => self.toggle_local_mode(),
            Effect::SetRemoteMode => self.set_remote_mode(),
            Effect::ToggleWordWrap => self.toggle_word_wrap(),
            // 2026-11-17 Tunnels：总线开关会真的停/起连接，所以必须先落内存
            // 再写盘——反过来的话，写盘失败会让屏幕与实际转发不一致。
            Effect::ToggleTunnels => {
                let on = !self.tunnels_enabled;
                self.set_tunnels_enabled(on);
                crate::ui::dialogs::tunnel_settings::save_enabled(on);
            }
            Effect::OpenTunnelSettings => {
                self.tunnel_editor = Some(crate::ui::dialogs::tunnel_settings::Editor::new(
                    self.tunnel_profiles.clone(),
                ));
            }
            // hex 三个参数（`View ▸ Hex Base / Columns / Group Chars`）。
            Effect::SetHex(knob) => self.set_tab_hex(knob),
            // 栅格可视化十条（`View ▸ Symbols` / `View ▸ White Spaces` /
            // `Text ▸ Highlight Caret Line`）。与上面一条同构：reducer 只说
            // 「点了哪一个旋钮」，翻转 / 设档与「改哪一份参数」都在 `setup.rs`
            // 的 `set_marks_knob` 一处——它改活动标签的 `Tab.marks`，不落盘。
            Effect::SetMarks(knob) => self.set_marks_knob(knob),
            // `Terminal.ManageTriggers` / `ManageOneKeys` 自 2026-11-15 起
            // 直接置 `ViewState::show_triggers` / `show_onekeys`——落点是独立
            // 对话框，不再需要「开设置再翻页」这两条效应。
            // `Terminal.ClearRecentUrls`：与地址栏右键「Clear Recent」同一个函数
            // （清内存副本 + load-modify-save 落盘），不另写一份清理逻辑。
            Effect::ClearRecentUrls => self.clear_recent_targets(),
            // 三档预设共用这一条带参数的 `Effect`。`from_permille` 是
            // 千分比 → f32 的唯一换算点；`set_window_opacity` 再一并改运行期
            // 副本与 `Prefs`（重启仍生效）。此处不碰 `Visuals`：那是每帧
            // `sync_window_opacity` 按钳制后的值做的事。
            Effect::SetWindowOpacity {
                transparent,
                opacity_permille,
            } => {
                self.set_window_opacity(WindowOpacity::from_permille(
                    transparent,
                    opacity_permille,
                ));
            }
            // 第四条命令不设值——reducer 已把 `show_settings` 打开，
            // 这里只把左栏翻到 `Appearance`（那一页有开关 + 滑杆真控件）。
            Effect::FocusAppearancePage => {
                self.settings_page = crate::ui::dialogs::SettingsPage::Appearance;
                self.ctx.request_repaint();
            }
            // 2026-11-14：折叠族九条的唯一落地点。
            Effect::Fold(cmd) => self.apply_fold(cmd),
        }
    }

    /// `Mode ▸ Hex Edit` 的落点：翻**当前标签**的显示模式，**不落盘**。
    ///
    /// 「不写盘」是用户定死的行为（「View 可以切当前会话的显示方式，但是不会
    /// 保存」）。原先这里还 `save_prefs(|p| p.set_session_mode(..))`，那张按
    /// 会话名索引的表已经整条删掉——留着就是「说不会保存、其实在保存」。
    ///
    /// 没有活动标签时静默——菜单能在没有标签的窗口里点到，那时候什么也不该发生。
    fn toggle_hex_mode(&mut self) {
        let active = self.active_slot();
        let Some(tab) = self.tabs.get_mut(active) else {
            return;
        };
        let next = match tab.mode {
            term_runtime::prefs::DisplayMode::Text => term_runtime::prefs::DisplayMode::Bin,
            term_runtime::prefs::DisplayMode::Bin => term_runtime::prefs::DisplayMode::Text,
        };
        tab.mode = next;
        // 切到 Bin 时回到贴底跟随：新切过来的人要看的是**最新**那一段，
        // 而不是上次回看停下的位置。只动视口锚点，环一个字节都不动——
        // 「换一种看法」不是「丢一次历史」。
        if next == term_runtime::prefs::DisplayMode::Bin {
            tab.raw.follow();
        }
        // 切换在下一帧立刻可见：渲染器按 `tab.mode` 分叉（见 `pane.rs`），
        // 这里只负责请一次重绘。
        self.ctx.request_repaint();
    }

    /// `Mode ▸ Debug Mode` 的落点：翻**当前标签**的调试覆盖层，并在日志槽
    /// 空闲时顺带起一份**原始字节排障日志**（见 `debug_log::sync_debug_log`）。
    ///
    /// 与 [`Self::toggle_hex_mode`] 同形，但**没有**「切过去要跟随」那种尾部
    /// 副作用：**覆盖层**不改视口、不改环、不改任何输入路径——它只是让
    /// `pane_overlays.rs` 多画一遍已被渲染的信息；落盘的排障日志由
    /// `debug_log::sync_debug_log` 负责，两者共用一个开关但不共用一个槽。
    ///
    /// 没有活动标签时静默——菜单能在没有标签的窗口里点到（与 `toggle_hex_mode` 同）。
    ///
    /// **`bin` 下点了屏幕上不会有变化**：覆盖层只画在字符网格上
    /// （`pane_overlays.rs` 的 `if !bin && tab.debug`），位照样翻，切回 `Text`
    /// 就看见。这是定的，不是 bug——hex 视图没有字符格坐标系。
    ///
    /// `request_repaint` 不能省：不开的时候屏幕上什么都不会变，而 egui 空闲时
    /// 不重画——不请求一次，用户会看到「关掉之后标记还在」。
    fn toggle_debug_mode(&mut self) {
        // 与 `toggle_hex_mode` 同形：之后活动位是身份（`active`），
        // 下标必须经 `active_slot()` 派生；先取值再 `get_mut` 是为了拆开两次借。
        let active = self.active_slot();
        let Some(tab) = self.tabs.get_mut(active) else {
            return;
        };
        tab.debug = !tab.debug;
        let on = tab.debug;
        let busy_log = tab.log_status().is_recording();
        debug_log::sync_debug_log(&self.pool, tab, on, busy_log);
        self.ctx.request_repaint();
    }

    /// 这个旋钮**此刻**在活动标签上是开着的吗——菜单的勾就画这个值。
    ///
    /// 与 [`Self::set_marks_knob`] 是同一张表的两个方向：那边「点一下改成什么」，
    /// 这边「现在是怎样」。两者必须同构，否则勾会与状态错开一档
    /// （例如 `ColumnMarker` 判成恒 `true`，用户关掉参考线之后勾还亮着）。
    ///
    /// 没有活动标签时 `active_marks()` 给出厂值（全关）——菜单能在没有标签的
    /// 窗口里点开，那时所有勾都该是空框，不该 panic。
    pub(in crate::ui) fn marks_knob_on(&self, knob: term_app::shell::MarksKnob) -> bool {
        use term_app::shell::MarksKnob;
        let marks = self.active_marks();
        match knob {
            MarksKnob::LineEndings => marks.line_endings,
            // 参考列是 `Option<usize>`：「开」= 有列号可画，与「第几列」无关。
            MarksKnob::ColumnMarker => marks.column_marker.is_some(),
            MarksKnob::IndentGuides => marks.indent_guides,
            MarksKnob::WrapSymbols => marks.wrap_symbols,
            MarksKnob::CaretLine => marks.caret_line,
            // 五档互斥：只有**当前那一档**打勾，其余四档空框。
            MarksKnob::Whitespace(index) => marks.whitespace.index() == usize::from(index),
        }
    }

    /// 活动标签是不是**只读查看**态（`Window ▸ Local Mode`）。
    ///
    /// 与 [`Self::marks_knob_on`] 同一类：给「设置页的勾」与「菜单的勾」提供
    /// 当前值。没有活动标签时给 `false`——那时候没有任何会话会被冻住，
    /// 画成「已勾」等于说一条不存在的会话正在只读。
    pub(in crate::ui) fn local_mode_on(&self) -> bool {
        self.tabs
            .get(self.active_slot())
            .is_some_and(|tab| tab.local_mode)
    }

    /// `Mode ▸ Word Wrap` 的勾：活动标签是否开着软换行。
    pub(in crate::ui) fn word_wrap_on(&self) -> bool {
        self.tabs
            .get(self.active_slot())
            .is_some_and(|tab| tab.word_wrap)
    }

    /// `View ▸ Hex Base / Hex Columns / Hex Group Chars` 的**唯一落地点**：
    /// 改**活动标签**的排版，**不落盘**。
    ///
    /// 名字里带 `tab` 是为了不让下一个人以为它还写盘。设置 ▸ Binary 页那一组
    /// 数字框写的是另一层（`Prefs.hex_*`，即「默认参数」，只对新会话生效），
    /// 两层的分界写在 `terminal/hex_params.rs` 的模块头。
    ///
    /// 「不落盘」不是靠自觉：全部数学在 `hex_params::apply_knob`，那个函数的
    /// 签名里既没有 `Prefs` 也没有盘——它做不到。这里只负责把结果写回标签。
    fn set_tab_hex(&mut self, knob: term_app::shell::HexKnob) {
        let active = self.active_slot();
        let Some(tab) = self.tabs.get_mut(active) else {
            return;
        };
        tab.hex = crate::ui::terminal::hex_params::apply_knob(tab.hex, knob);
        // Bin 那两张基线与表头读数要立刻跟上，否则用户点完菜单看着像没生效。
        self.ctx.request_repaint();
    }

    /// 对活动标签执行一条清理动作。先取下标再借用标签，避免
    /// `get_mut(active_slot())` 同时借 `self` 引发的 E0502。
    fn with_active_tab(&mut self, f: impl FnOnce(&mut crate::ui::Tab)) {
        let active = self.active_slot();
        if let Some(tab) = self.tabs.get_mut(active) {
            f(tab);
        }
    }

    /// 标签右键 10 个动作的**唯一**落地点。
    ///
    /// `target = None` 是命令 ID 路径（命令面板 / 快捷键），取当前活动标签；
    /// `Some(i)` 是右键点中的那个标签，两者在这里合流，不下沉到纯层。
    /// 复用的是标签 × / 菜单 Close 一直在用的那批方法，没有第二套语义。
    fn apply_tab_action(&mut self, action: TabAction) {
        let index = |t: Option<usize>, active: usize| t.unwrap_or(active);
        match action {
            TabAction::Activate { target } => {
                self.set_active_slot(index(target, self.active_slot()));
            }
            TabAction::Close { target } => {
                let i = index(target, self.active_slot());
                self.close_tab_with_history(i);
            }
            TabAction::CloseOthers { target } => {
                let keep = index(target, self.active_slot());
                // 先记下名字，再从后往前关，避免下标漂移。
                let names: Vec<String> = self
                    .tabs
                    .iter()
                    .map(|tab| tab.config.name.clone())
                    .collect();
                for i in (0..names.len()).rev() {
                    if i != keep {
                        self.closed_sessions.push(names[i].clone());
                    }
                }
                for i in (0..self.tabs.len()).rev() {
                    if i != keep {
                        self.close_tab(i);
                    }
                }
                self.set_active_slot(0);
            }
            TabAction::CloseAll => self.close_all_views(),
            TabAction::Reopen => self.reopen_closed(),
            TabAction::Duplicate { target } => {
                let i = index(target, self.active_slot());
                self.duplicate_tab(i);
            }
            TabAction::Split { horizontal } => {
                let shell = if horizontal {
                    ShellAction::SplitViewHorizontally
                } else {
                    ShellAction::SplitViewVertically
                };
                self.dispatch_action(Action::Shell(shell));
            }
            TabAction::CopyName => self.copy_session_name(),
            TabAction::CopyUrl => self.copy_session_url(),
            TabAction::Color { target, slot } => {
                let i = index(target, self.active_slot());
                self.set_tab_color(i, slot);
            }
            // 终端内容右键的四条真实命令。此前它们在 `context_menu.rs` 的私有
            // `Work` 里直接调这三个方法，绕过 `Action::parse`；现在右键只发
            // 命令 ID，走到这里与菜单 / 快捷键 / 脚本 `app_command` 同一条路。
            // 复用的是右键一直在用的三个 `Tab` 方法，没有第二套清理语义。
            TabAction::CopyCommand => self.copy_command(),
            TabAction::ClearScreen => self.with_active_tab(|t| t.clear()),
            TabAction::ClearScrollback => self.with_active_tab(|t| t.clear_scrollback()),
            TabAction::ClearAll => self.with_active_tab(|t| t.clear_all()),
        }
    }

}

/// Help 的落点：本项目自己的仓库（About 与状态栏的链接都走这里）。
const HELP_URL: &str = "https://github.com/EzFavorites/WindTerm";

/// 新标签建好之后 `active_tab` 该取什么值（`prefs.focus_new_tab`）。
///
/// 抽成纯函数是为了让「关掉之后不许把下标指到不存在的标签上」这件事能被单测
/// 钉住：`activate_new_tab` 只是把它写回窗口，判断逻辑只有这一份。
/// `tab_count` 是**建完新标签之后**的数量。
pub(in crate::ui) fn next_active_tab(
    active: usize,
    new_tab: usize,
    tab_count: usize,
    focus_new_tab: bool,
) -> usize {
    if focus_new_tab {
        return new_tab;
    }
    // 不抢焦点：保持不变，但要夹进现有范围——调用点可能刚删过标签。
    active.min(tab_count.saturating_sub(1))
}

/// `XferPrompt::from` 只拆命令；会话默认（冲突策略 / 打开目录）在这里注入，
/// 所以直启路径与菜单路径拿到的是同一份弹层状态。无活动标签时用出厂值。
fn xfer_prompt(cmd: XferCmd, index: usize, open_after: bool) -> Option<crate::ui::dialogs::xfer::XferPrompt> {
    let mut prompt = crate::ui::dialogs::xfer::XferPrompt::from(cmd)?;
    prompt.policy = crate::xfer_conflict::policy_from_index(index);
    prompt.open_after = open_after;
    Some(prompt)
}

/// 把会话配置拼成连接串（`ssh://user@host:port`），Shell 退化成名字。
fn session_url(cfg: &SessionConfig) -> String {
    if matches!(cfg.protocol, Protocol::Shell) {
        return cfg.name.clone();
    }
    let scheme = match cfg.protocol {
        Protocol::Ssh => "ssh",
        Protocol::Telnet => "telnet",
        Protocol::Tcp => "tcp",
        _ => "ssh",
    };
    let mut s = format!("{}://", scheme);
    if !cfg.username.trim().is_empty() {
        s.push_str(cfg.username.trim());
        s.push('@');
    }
    s.push_str(cfg.host.trim());
    let def = cfg.protocol.default_port();
    if cfg.port != 0 && cfg.port != def {
        s.push(':');
        s.push_str(&cfg.port.to_string());
    }
    s
}


#[cfg(test)]
mod tests {
    use super::{next_active_tab, xfer_prompt};
    use term_app::action::{XferCmd, XferProtocol};

    #[test]
    fn a_new_xfer_prompt_inherits_the_active_sessions_receive_defaults() {
        // 菜单与 `modem_use_default` 直启都经这里；出厂是 Overwrite / 不打开。
        let cmd = XferCmd::Receive(XferProtocol::Zmodem);
        let fresh = xfer_prompt(cmd, 0, false).unwrap();
        assert_eq!(fresh.policy, crate::xfer_conflict::policy_from_index(0));
        assert!(!fresh.open_after);
        let configured = xfer_prompt(cmd, 4, true).unwrap();
        assert_eq!(configured.policy, crate::xfer_conflict::policy_from_index(4));
        assert!(configured.open_after);
        // 手改 JSON 的越界下标回落 Overwrite，不会让弹层画出空白档。
        assert_eq!(xfer_prompt(cmd, usize::MAX, false).unwrap().policy, fresh.policy);
        assert!(xfer_prompt(XferCmd::Cancel, 4, true).is_none());
    }

    #[test]
    fn a_new_tab_takes_focus_when_the_setting_is_on() {
        // 出厂行为：开一条新标签就切过去（分屏 / 从会话树连接 / 复制标签同一条）。
        assert_eq!(next_active_tab(0, 2, 3, true), 2);
    }

    #[test]
    fn without_focus_the_old_tab_keeps_the_keyboard() {
        // 新标签下标 2 已经存在（tab_count = 3），但不该抢走焦点。
        assert_eq!(next_active_tab(1, 2, 3, false), 1, "焦点必须留在原来那条");
    }

    #[test]
    fn without_focus_the_index_never_points_past_the_last_tab() {
        // 中间关过标签之后再开新标签：旧下标可能已经越界，夹到最后一个存在的。
        assert_eq!(next_active_tab(9, 1, 2, false), 1);
        // 极端情况（一条标签都没有时也走到这里）：不 panic、也不返回越界值。
        assert_eq!(next_active_tab(3, 0, 0, false), 0);
    }
}
