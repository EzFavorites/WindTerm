//! 「运行脚本」窗口：选一个脚本文件，在**当前 tab 的会话**上跑。
//!
//! 与 `desk script run`（进程级）的分工：那条命令自己开池、自己开会话、跑完就退，
//! 是给 CI 与 shell 用的；这里跑在用户正看着的那个终端上——脚本 `send` 出去的
//! 东西会出现在同一块屏幕上，结果也回在这里。
//!
//! 为什么结果不直接写进终端：那是**伪造输出**。终端里的每一个字节都该来自会话，
//! 否则用户没法分辨哪句是远端说的、哪句是我们假装远端说的。
//!
//! 真正的执行线程在 `apps/desktop/src/script_run.rs`（IO 不进投影层）。

use crate::script_run::{self, Outcome, RunOptions, ScriptRun};
use crate::ui::kit::window::{DialogId, DialogWindow};
use crate::ui::*;
use term_core::session_runtime::SessionId;

/// 两份清单合起来的高度（列头 + 五行）。
///
/// 写死而不是「内容多高就多高」：空列与满列必须占同样一块地方，否则窗口会在
/// 「跑过一次脚本」前后跳高一次，底下的按钮跟着上下移动。
const LIST_H: f32 = 108.0;

/// 脚本窗口的全部状态。
///
/// 放 `TerminalWindow` 上而不是 `ViewState`：里面有一个线程句柄（`ScriptRun`），
/// reducer 不该持有这种东西。
#[derive(Default)]
pub(in crate::ui) struct ScriptDialog {
    /// 脚本路径（跨帧输入缓冲）。
    pub path: String,
    /// 脚本库清单：`(完整路径, 展示名)`，**打开窗口时取一次**（理由见
    /// `script_library` 模块头）。存在这里而不是每帧读盘，也让组件夹具能像填
    /// `path` 一样填它——基线才拍得到「有脚本」的样子。
    pub library: Vec<(String, String)>,
    /// 在跑的那一次（`None` = 没在跑）。
    pub run: Option<ScriptRun>,
    /// 上一次的结果。留着给人看，直到下一次运行。
    pub outcome: Option<Outcome>,
    /// 本地错误：读文件失败 / 起不了线程。**不是**脚本自己的错。
    pub error: Option<String>,
    /// 脚本对着哪个会话跑（打开窗口那一刻定下来）。
    pub session: Option<SessionId>,
    /// 预算档位与手勾授权都在 `TerminalWindow.run_config` 上（三扇窗共享）。
    /// 兼容终端没有「本次授权 / 批量档」这两层，是我们为自动化安全加的；
    /// 这里不再留一份镜像，否则对话框里勾的与面板 / 编辑器拿到的会各说一套。
    /// 触发器经命令 ID 拉起的脚本走后台：同一会话的前台名额不受它影响。
    pub background: bool,
}

impl ScriptDialog {
    /// 打开窗口时的初值：路径 + **当帧取一次**的脚本库清单。
    ///
    /// 两条入口（「运行脚本」菜单、按路径直接跑）都要它：只给 `path` 的话，
    /// 另一条路上的窗口会显示「no scripts in library」——那不是事实，是没填。
    pub(in crate::ui) fn opened(session: Option<SessionId>, path: impl Into<String>) -> Self {
        Self::opened_with(session, path, false)
    }

    /// `background = true` 只给触发器入口用：不占用会话的前台脚本名额。
    pub(in crate::ui) fn opened_background(session: Option<SessionId>, path: impl Into<String>) -> Self {
        Self::opened_with(session, path, true)
    }

    fn opened_with(session: Option<SessionId>, path: impl Into<String>, background: bool) -> Self {
        Self {
            path: path.into(),
            library: super::script_library::library(),
            session,
            background,
            ..Self::default()
        }
    }

    /// 状态行要显示什么。四种状态各自不同——「在跑」不能长成「跑完了」。
    pub(in crate::ui) fn status(&self) -> String {
        if let Some(err) = &self.error {
            return err.clone();
        }
        if self.run.is_some() {
            return "Running…".to_string();
        }
        match &self.outcome {
            Some(Outcome::Done(value)) if value.is_empty() => "Done (no value)".to_string(),
            Some(Outcome::Done(value)) => format!("Done: {value}"),
            Some(Outcome::Failed { message, .. }) => format!("Failed: {message}"),
            None => "Ready".to_string(),
        }
    }
}

impl TerminalWindow {
    /// 按**槽位**打开窗口：底带 `Shell` 面板也是真会话，脚本能对它跑。
    ///
    /// 与主区标签页同一条路：只决定「绑哪条会话」，运行窗口与执行线程仍然是
    /// 同一份（`script_run.rs`）。2026-11-15 之前它收的是标签下标——面板没有
    /// 下标，于是面板里那颗「运行脚本」只能灰着。
    pub(in crate::ui) fn open_script_dialog_for(&mut self, slot: super::TabSlot) {
        let session = super::tab_slot::tab_of(&mut self.tabs, &mut self.shell_tabs, slot)
            .and_then(|t| t.session());
        self.script = Some(ScriptDialog::opened(session, ""));
        self.ctx.request_repaint();
    }

    /// `Tool ▸ Script ▸ Run Script` 的落点：绑当前活动标签，与终端右键
    /// `More ▸ Run Script` 走的是同一个 `open_script_dialog_for`。
    ///
    /// 单独成方法，是为了让 `ui/effects.rs` 的落地不出现命令 ID 字符串——
    /// 那条门禁只允许 ID 活在 `crates/term-app` 的数据层。
    pub(in crate::ui) fn open_script_dialog_for_active(&mut self) {
        if let Some(id) = self.active {
            self.open_script_dialog_for(super::TabSlot::Main(id));
        }
    }

    /// 叫停本窗口所有正在跑的脚本：`Tool ▸ Script ▸ Stop Script` 与
    /// `Mode ▸ Local Mode` 共用这一处。
    ///
    /// 脚本拿的是 `SessionPool`，不读 `Tab`，所以只读切换必须把所有表面的句柄
    /// 一一取消——菜单里的 Stop 是同一个意图，不该写第二份遍历。
    /// 句柄留着直到 `Outcome` 到达（面板 / 对话框各自 poll 后清理）。
    pub(in crate::ui) fn cancel_all_script_runs(&self) {
        let mut any = false;
        if let Some(run) = self.script.as_ref().and_then(|dlg| dlg.run.as_ref()) {
            run.cancel();
            any = true;
        }
        for run in &self.logon_runs {
            run.cancel();
            any = true;
        }
        for tab in &self.script_panel.tabs {
            if let Some(run) = tab.state.run.as_ref() {
                run.cancel();
                any = true;
            }
        }
        if let Some(run) = self.script_editor.as_ref().and_then(|s| s.run.as_ref()) {
            run.cancel();
            any = true;
        }
        if any {
            self.ctx.request_repaint();
        }
    }

    /// ADR-0025：把脚本新开的会话变成标签页。每帧调一次，**与运行窗口
    /// 开没开无关**——脚本线程在窗口关掉之后仍可能接着 `session_open`。
    ///
    /// 三条规矩：
    ///
    /// 1. **不抢焦点**。脚本开会话是它自己的事，用户手上可能正敲着另一条。
    ///    标签排在末尾、需要时自己点。抢焦点会让「跑个脚本」变成「打断我干活」。
    /// 2. **同一条会话不建第二次**。脚本对已存在的会话只会 `session_use`，不会
    ///    `session_open`，所以正常不会撞；但队列是跨线程的，去重比事后解释便宜。
    /// 3. **`egui::Id` 由会话 id 派生**。标签的焦点 id 要在本窗口内稳定，而
    ///    会话 id 正是一个不随重排漂移的值（`egui::Id::new(字符串)` 只按内容取
    ///    哈希，重复登记同一条会话时不会换 id）。
    pub(in crate::ui) fn absorb_script_opened(&mut self) {
        // 先整批取走再建标签：`add_tab` 要 `&mut self`，锁着队列改 `self` 会死锁
        // （同一把锁在 `PooledSessions::open` 里由脚本线程持有）。
        let opened: Vec<(SessionId, term_core::session::SessionConfig)> =
            match self.script_opened.lock() {
                Ok(mut rows) => std::mem::take(&mut *rows),
                // 中毒说明取队列的某一帧崩过；队列本身还能用，这一帧就当没有新会话。
                Err(_) => return,
            };
        if opened.is_empty() {
            return;
        }
        for (session, config) in opened {
            if self.tabs.iter().any(|t| t.session == Some(session)) {
                continue;
            }
            let id = egui::Id::new(("tab_script_opened", session));
            let ctx = self.ctx.clone();
            // 会话已经在池子里了，所以走 `tab_from`（`Some(session)`）而不是
            // `make_tab`——后者会再 `pool.open` 一次，多出一条没人看见的重复会话。
            let tab = crate::ui::terminal::setup::tab_from(&self.pool, config, Some(session), id, &ctx);
            self.add_tab(tab);
        }
        self.ctx.request_repaint();
    }

    /// 组一次「在界面里跑脚本」的全部选项：会话递回 / 提问 / 文件框 / 剪贴板 /
    /// 窗口 / 激活 / 已知配置 / **同会话互斥**。
    ///
    /// **三条运行入口（运行窗口 / 停靠面板 / 编辑器）都从这里取**：手写三份的话，
    /// 迟早有一份忘了接提问队列，那条路上的脚本一 `ask` 就报「没有可交互输入」；
    /// 互斥也会跟着漏——那样三条入口各持一本登记簿，等于没有互斥。
    ///
    /// `who` 是「谁在跑」的提示里那个名字（用脚本路径）。
    pub(in crate::ui) fn script_run_options(&self, who: &str) -> RunOptions {
        let mut options = RunOptions::with_opened(self.script_opened.clone())
            .with_window(
                self.ctx.clone(),
                self.script_selection.clone(),
                self.script_asks.clone(),
                self.script_picks.clone(),
                self.script_active.clone(),
                self.script_activated.clone(),
            )
            .with_configs(
                self.tabs
                    .iter()
                    .filter_map(|tab| tab.session.map(|id| (id, tab.config.clone())))
                    .collect(),
            )
            .with_app(self.app_queue.clone())
            .with_registry(self.script_runs.clone(), who);
        // 已登记脚本的参数从注册表取。三条运行表面（运行窗口 / 停靠面板 /
        // 编辑器）都走这一个构造器，所以 `ScriptEntry.args` 不会因为入口不同
        // 而只对其中一条生效。`who` 为空（编辑器未保存）时自然没有参数。
        options.args = registered_script_args(who);
        options
    }

    /// 每帧把「界面当前是什么样」抄进脚本能看见的那几个槽：选区、活动会话。
    ///
    /// 两件事都是**派生值**（`selection_text` / `active`），脚本需要时现取，
    /// 但脚本线程读不到 `&self`，所以每帧抄一次。抄的成本是一次 `lock` + 一次
    /// `clone`，在没有脚本运行时也照抄——省这一次会让「脚本刚起来时读到的
    /// 是陈年选区」。
    pub(in crate::ui) fn sync_script_slots(&mut self) {
        if let Ok(mut slot) = self.script_selection.lock() {
            if *slot != self.selection_text {
                slot.clear();
                slot.push_str(&self.selection_text);
            }
        }
        if let Ok(mut slot) = self.script_active.lock() {
            let active = self.active;
            if *slot != active {
                *slot = active;
            }
        }
    }

    /// 脚本的 `session_activate`：把那条会话的标签切到前台。
    ///
    /// 找不到那条标签时**什么也不做**：脚本可能激活一条刚被用户关掉的会话，
    /// 那不是错误，只是「来不及」。报错会让脚本因为一件它控制不了的事失败。
    pub(in crate::ui) fn drain_script_activated(&mut self) {
        let requested: Vec<SessionId> = match self.script_activated.lock() {
            Ok(mut rows) => std::mem::take(&mut *rows),
            Err(_) => return,
        };
        let Some(wanted) = requested.last().copied() else {
            return;
        };
        if let Some(index) = self.tabs.iter().position(|tab| tab.session == Some(wanted)) {
            self.set_active_slot(index);
            // 分屏态下还要把那片窗格的活动会话换过去：只切全局 `active` 不会
            // 让任何一块窗格换内容（用户看到的是「点了没反应」）。
            if let Some(pane) = self.pane_of_session(wanted) {
                if let Some(group) = self.pane_groups.get_mut(&pane) {
                    group.active = wanted;
                }
            }
            self.ctx.request_repaint();
        }
    }

    /// 脚本提问的模态框：取一个、画一个、回一个。
    ///
    /// 一次只处理一个请求——脚本是顺序执行的，答案也该按序给。已经立着一个
    /// 模态框时不再取，否则「回车」会落到没显示的那一个上。
    pub(in crate::ui) fn poll_script_ask(&mut self, ctx: &egui::Context) {
        // 脚本可能在模态框立着的时候被「停止」；那扇窗已经没人等回信了，撤掉。
        if self.script_ask.as_ref().is_some_and(|modal| modal.is_stale()) {
            self.script_ask = None;
        }
        if self.script_ask.is_none() {
            self.script_ask = overlays::poll_ask(&self.script_asks, &self.script_picks);
        }
        // `lang()` 先取：下面要把 `script_ask` 以可变借用拿出来，两者不能同时借 `self`。
        let lang = self.lang();
        let Some(modal) = self.script_ask.as_mut() else {
            return;
        };
        if let Some(answer) = overlays::draw_ask(ctx, modal, lang) {
            let Some(modal) = self.script_ask.take() else {
                return;
            };
            let resolved = overlays::resolve_ask(&modal, answer);
            modal.deliver(resolved);
            // 还有一个在等就立刻再取一个（脚本可能连问两次）。
            self.ctx.request_repaint();
        }
    }

    /// 登录脚本（ADR-0027）：标签带着**活会话**出现之后，自动给它跑一次。
    ///
    /// 为什么是每帧扫一遍而不是在「连上」那一刻显式调用：
    ///
    /// 1. 建标签页的入口有五六条（内置 SSH 的 `adopt_channel`、系统 ssh、本地
    ///    Shell、重连、布局恢复、脚本自己 `session_open` 出来的）。逐条去挂钩子，
    ///    漏一条就是「这条路上登录脚本不跑」，而且极难发现。
    /// 2. 判据本身很简单——「有会话 + 有配置 + 没跑过」——不需要时序信号。
    ///
    /// 内置 SSH 的标签页是**认证成功之后**才建的（`adopt_channel` 拿到通道才
    /// `place_tab`），所以「标签页存在」在这里已经等于 CRT 的「会话 Alive」。
    pub(in crate::ui) fn fire_logon_scripts(&mut self) {
        let mut todo: Vec<(SessionId, String)> = Vec::new();
        for tab in &self.tabs {
            let Some(id) = tab.session else { continue };
            if self.logon_fired.contains(&id) {
                continue;
            }
            let path = tab.config.logon_script.trim().to_string();
            if path.is_empty() {
                continue;
            }
            todo.push((id, path));
        }
        for (id, path) in todo {
            // 2026-11-19 只读查看：登录脚本同样不跑。这里**故意不记** `logon_fired`
            // ——只读是用户临时挂起，退出后这条「刚连上就跑一次」的约定该补上，
            // 否则用户得手动重连才能看到登录脚本生效。
            if self
                .tabs
                .iter()
                .any(|tab| tab.session == Some(id) && tab.local_mode)
            {
                continue;
            }
            // 先记再跑：读文件 / 起线程这两步都可能失败，而「失败」不该让下一帧
            // 再试一次——那样一个打错路径的登录脚本会每帧失败一次，把状态栏刷爆。
            self.logon_fired.insert(id);
            let source = match script_run::read_source(&path) {
                Ok(text) => text,
                Err(message) => {
                    self.log_note = Some(message);
                    continue;
                }
            };
            let options = self.script_run_options(&path);
            match ScriptRun::start(self.pool.clone(), id, source, options) {
                Ok(run) => self.logon_runs.push(run),
                Err(message) => self.log_note = Some(message.to_string()),
            }
        }
        // 收尸：结果照旧落到状态栏（失败要说原因），句柄一并丢掉——那是给
        // 同会话互斥放行的唯一时机。
        //
        // 先整批取出来再逐条判：闭包里同时改 `log_note` 和 `logon_runs` 是两处
        // 可变借用，编译器不会让我们俩同时存在。
        for mut run in std::mem::take(&mut self.logon_runs) {
            let notes = run.drain_notes();
            if let Some(last) = notes.last() {
                self.log_note = Some(last.clone());
            }
            match run.try_outcome() {
                Some(Outcome::Failed { message, .. }) => {
                    self.log_note = Some(format!("Logon script failed: {message}"));
                }
                Some(Outcome::Done(_)) => {}
                // 还在跑：句柄放回去，下一帧接着收。
                None => self.logon_runs.push(run),
            }
        }
        // 会话关掉之后那一条记录就没用了；不剪的话它会跟着这一次运行一直留着
        // （`SessionId` 是进程内自增值，不会复用，所以只是内存上的小账）。
        let live: std::collections::HashSet<SessionId> =
            self.tabs.iter().filter_map(|tab| tab.session).collect();
        self.logon_fired.retain(|id| live.contains(id));
    }

    /// Authentication 页「Execute command after authentication」：标签带着活会话
    /// 出现之后，等 `Delay` 秒把命令写进**同一条**会话的输入。
    ///
    /// 内置 SSH 的标签在认证成功之后才建（`adopt_channel`），所以「标签存在」
    /// 在这里就是认证完成的信号。系统 `ssh` 路径的标签先出现再认证，那条路上
    /// 命令会写进远端 shell 的键盘缓冲区（等认证完自然被读到），语义仍然成立。
    /// 命令文本是用户填的，不在这里解析、不伪造输出：写进去什么就是什么。
    pub(in crate::ui) fn fire_auth_commands(&mut self) {
        use std::time::{Duration, Instant};
        let platform = term_core::session::auth_platform();
        let now = Instant::now();
        let mut due: Vec<(SessionId, String)> = Vec::new();
        for tab in &self.tabs {
            let Some(id) = tab.session else { continue };
            if self.auth_exec_fired.contains(&id) || tab.local_mode {
                continue;
            }
            let command = if tab.config.protocol == Protocol::Ssh && tab.config.builtin_ssh {
                // 只在内置客户端上发：系统 ssh 的标签在建连前就出现，那时把
                // 命令写进 PTY 会被 ssh 的密码提示吃掉，变成一次莫名其妙的认证失败。
                tab.config.auth_exec_command[platform].trim()
            } else {
                ""
            };
            if command.is_empty() {
                continue;
            }
            match self.auth_exec_pending.get(&id) {
                None => {
                    let delay = tab.config.auth_exec_delay[platform];
                    self.auth_exec_pending
                        .insert(id, now + Duration::from_secs(u64::from(delay)));
                }
                Some(at) if now >= *at => due.push((id, command.to_string())),
                _ => {}
            }
        }
        for (id, command) in due {
            self.auth_exec_fired.insert(id);
            let line = format!("{command}\r");
            if !self.pool.send(id, line.as_bytes()) {
                self.log_note = Some("The session ended before its post-auth command ran".to_string());
            }
        }
        let live: std::collections::HashSet<SessionId> =
            self.tabs.iter().filter_map(|tab| tab.session).collect();
        self.auth_exec_pending.retain(|id, _| live.contains(id));
        self.auth_exec_fired.retain(|id| live.contains(id));
    }

    /// 每帧收一次结果与提示。
    pub(in crate::ui) fn poll_script(&mut self) {
        let Some(dialog) = self.script.as_mut() else {
            return;
        };
        let Some(run) = dialog.run.as_mut() else {
            return;
        };
        // `notify` 的话先落进状态栏（既有的一次性反馈通道），再看有没有结果：
        // 脚本可能先提示一句、再失败，两件事不该互相覆盖掉。
        let notes = run.drain_notes();
        if let Some(last) = notes.last() {
            self.log_note = Some(last.clone());
        }
        if let Some(outcome) = run.try_outcome() {
            dialog.outcome = Some(outcome);
            dialog.run = None;
        }
    }

    pub(in crate::ui) fn draw_script_dialog(&mut self, ctx: &egui::Context) {
        // 「最近用过」是窗口上的 MRU（`note_recent_script` 每次起跑都会更新它），
        // 先克隆一份：下面 `self.script.as_mut()` 已经把整窗借走了。
        let recent = self.script_recent.clone();
        // 后台脚本不占会话名额，不会把 Run 灰掉，所以要在窗口里明说有人在跑。
        let bg_running = self.script_runs.background_count();
        // 「长跑」勾选与面板的授权勾选共用窗口上那一份 RunConfig：三扇窗看到
        // 的是同一档预算、同一组能力（兼容终端没有这一层，是我们加的安全开关）。
        // 先取出来再借 `self.script`，免得两处可变借撞在一起。
        let mut batch = self.run_config.batch;
        let Some(dialog) = self.script.as_mut() else {
            return;
        };
        let lang = self.view.lang;
        // 没绑上会话时按钮要灰：脚本没有地方跑，点了也只能报错。
        let bound = dialog.session.is_some();
        let running = dialog.run.is_some();
        let denied = dialog.error.is_some()
            || matches!(dialog.outcome, Some(Outcome::Failed { .. }));
        let mut action = ScriptAction::None;
        let mut path = std::mem::take(&mut dialog.path);
        let status = dialog.status();

        DialogWindow::new(DialogId::ScriptRun, "Run script").show(
            ctx,
            |ui| {
                ui.label("Script file (Rhai). It runs on this tab's session.");
                ui.add_space(4.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut path)
                        .id(egui::Id::new("script_path_input"))
                        // 占满内容宽：默认宽度只有 240 点，而真实路径（库目录 +
                        // 文件名）比它长——被裁掉的正好是文件名，也就是唯一认得出
                        // 来的那一段。用 `available_width()` 而不是 `INFINITY`：
                        // 后者会让父容器跟着文本无限变宽（弹窗被推出画面）。
                        .desired_width(ui.available_width())
                        .hint_text("/path/to/script.rhai"),
                );
                let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(6.0);
                // 两份清单（后半）：左列脚本库、右列最近用过。点一行只
                // **填路径**，开跑仍然要按 Run——列表就在路径框正下方，误点的
                // 代价是一次不可撤销的远端执行。
                if let Some(picked) = super::script_library::draw_lists(
                    ui,
                    lang,
                    &dialog.library,
                    &recent,
                    &path,
                    LIST_H,
                ) {
                    path = picked;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Close").clicked() {
                        action = ScriptAction::Close;
                    }
                    ui.add_enabled_ui(bound && !running && !path.trim().is_empty(), |ui| {
                        if ui.button("Run").clicked() || (entered && !running) {
                            action = ScriptAction::Run;
                        }
                    });
                    // 脚本编辑器入口。路径可以空着（打开一个空白脚本自己写），
                    // 所以只要求「没在跑」。
                    ui.add_enabled_ui(!running, |ui| {
                        if ui.button(t(lang, "Edit")).clicked() {
                            action = ScriptAction::Edit;
                        }
                    });
                    // ADR-0027：在跑的必须能停下来。取消是协作式的（每个
                    // 阻塞点的 50ms 切片都会看一眼），所以按钮立刻生效，但脚本
                    // 要跑到下一个检查点才收场——最多一两百毫秒。
                    if running {
                        if ui.button("Stop").clicked() {
                            action = ScriptAction::Stop;
                        }
                        ui.spinner();
                    }
                    // 档位：交互档够用就不要给「能跑十分钟」的选项摆在第一屏。
                    // 文案统一走 `t(lang, …)`；英文原文与原基线逐字相同，中文界面
                    // 不再露英文。勾选写回共享 RunConfig，面板 / 编辑器立刻见到。
                    ui.add_enabled_ui(!running, |ui| {
                        ui.checkbox(&mut batch, t(lang, "Long-running"))
                            .on_hover_text(t(lang, "10 min / 50M operations instead of 30 s / 1M"));
                    });
                    if !bound {
                        ui.weak(t(lang, "This tab has no live session"));
                    }
                });
                ui.add_space(6.0);
                // 结果用 monospace：脚本返回值可能是多行的，比例字体对不齐。
                ui.add(egui::Label::new(egui::RichText::new(&status).monospace()).wrap());
                // 能力被拒时给一条可操作的提示。只在失败态出现：Ready / Running /
                // Done 三态一行都不画，基线里那张「刚打开」的图因此逐字节不变。
                // `// @cap` 是脚本头里的永久声明，面板的 Permissions 是本次手勾——
                // 两者都是本项目的设计，兼容终端没有能力授予这一层。
                if denied {
                    ui.label(
                        egui::RichText::new(t(lang, "Permission denied? Declare it with // @cap in the script, or enable it under Script panel ▸ Permissions (this run only)."))
                            .small(),
                    );
                }
                if bg_running > 0 {
                    ui.label(
                        egui::RichText::new(format!("Background scripts running ({bg_running})")).small(),
                    );
                }
            },
        );

        dialog.path = path;
        self.run_config.batch = batch;
        match action {
            ScriptAction::None => {}
            ScriptAction::Close => self.script = None,
            ScriptAction::Run => self.start_script(),
            ScriptAction::Stop => {
                // 只叫停，不摘句柄：句柄要留到结果真的回来（`poll_script` 收到
                // 之后才置空），否则界面上会先变回「Ready」再闪一下「Cancelled」。
                if let Some(run) = self.script.as_ref().and_then(|d| d.run.as_ref()) {
                    run.cancel();
                }
                self.ctx.request_repaint();
            }
            ScriptAction::Edit => {
                // 取会话与路径，再交给编辑器（编辑器自己读文件 / 建空缓冲）。
                let (session, path) = self
                    .script
                    .as_ref()
                    .map(|dialog| (dialog.session, dialog.path.clone()))
                    .unwrap_or((None, String::new()));
                if path.trim().is_empty() {
                    // 没填路径 = 新建脚本，落点预填到脚本库目录。
                    self.open_new_script(session);
                } else {
                    self.open_script_editor(session, Some(&path));
                }
                // 「运行一次」的窗口让位给编辑器，避免两个窗口叠着。
                self.script = None;
            }
        }
    }

    /// 记一条「最近跑过的脚本」并落盘（ADR-0027）。
    ///
    /// 落盘走 load-modify-save：`Prefs` 字段只增不减，整份重建会把语言和其余
    /// 几十项设置一起清掉（`address::save_recent_prefs` 里记过同一个坑）。
    pub(in crate::ui) fn note_recent_script(&mut self, path: &str) {
        crate::script_run::push_recent(&mut self.script_recent, path);
        let mut prefs = term_runtime::prefs::load().unwrap_or_default();
        prefs.language = self.view.lang.tag().to_string();
        prefs.recent_scripts = self.script_recent.clone();
        term_runtime::prefs::save(&prefs);
    }

    /// 录制器每帧一拍：把「屏幕安静了吗」告诉它（ADR-0027）。
    ///
    /// 锚点取**最后一段非空的行文本**。取整屏是不行的：整屏几乎每帧都在变
    /// （滚动、光标、时间戳），那样永远等不到安静；取最后一行正是「提示符」所在。
    pub(in crate::ui) fn tick_recorder(&mut self) {
        let recorder = std::sync::Arc::clone(&self.recorder);
        let Ok(mut slot) = recorder.lock() else {
            // 中毒 = 录制线程那一侧崩过。这一帧当没录，下一条命令照记。
            return;
        };
        let Some(rec) = slot.as_mut() else {
            return;
        };
        let now = std::time::Instant::now();
        let tail = self
            .tabs
            .get(self.active_slot())
            .map(|tab| {
                (0..tab.snapshot.rows)
                    .rev()
                    .map(|row| tab.snapshot.row_text(row))
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        rec.note_screen(&tail, now);
        rec.tick(now);
        // 录制中要持续重绘：静默是靠「时间过去了」判定，没有新事件就没有下一帧。
        if !rec.is_empty() {
            self.ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// 开始 / 结束录制。结束返回生成物落地的路径（录制为空时返回 `None`）。
    pub(in crate::ui) fn toggle_recording(&mut self) -> Result<Option<std::path::PathBuf>, String> {
        let recorder = std::sync::Arc::clone(&self.recorder);
        let mut slot = recorder.lock().map_err(|_| "Recorder state is broken".to_string())?;
        if slot.is_some() {
            let Some(mut rec) = slot.take() else {
                return Ok(None);
            };
            let steps = rec.finish(std::time::Instant::now());
            if steps.is_empty() {
                return Ok(None);
            }
            let path = crate::script_recorder::save_to_library(&crate::script_recorder::to_script(
                &steps,
            ))?;
            return Ok(Some(path));
        }
        *slot = Some(crate::script_recorder::Recorder::new(std::time::Instant::now()));
        Ok(None)
    }

    /// 正在录吗（面板据此把按钮从 Record 变 Stop Recording）。
    pub(in crate::ui) fn is_recording(&self) -> bool {
        self.recorder
            .lock()
            .map(|slot| slot.is_some())
            .unwrap_or(false)
    }

    /// 按路径直接跑一个脚本（映射键 / 按钮栏 / 最近列表共用）。
    ///
    /// 与「运行窗口」的区别只在**路径从哪来**：这里不填输入框，直接开跑——
    /// 用户按的是绑定好的键或按钮，再让他回头确认一次路径就是多余的。
    pub(in crate::ui) fn open_script_for_path(&mut self, path: &str) {
        self.open_script(path, false);
    }

    /// 触发器的 `Script.RunFile:`：开运行窗口但按后台跑，不占前台名额。
    pub(in crate::ui) fn open_script_for_path_background(&mut self, path: &str) {
        self.open_script(path, true);
    }

    fn open_script(&mut self, path: &str, background: bool) {
        // CRT 没有前台 / 后台，也没有「拒绝排队」这套说法——这两条都是
        // 本项目额外的自动化安全设计。上一次按路径起跑的句柄还留在 `self.script`
        // 里时，直接换掉整个 `ScriptDialog` 会把 `ScriptRun` 一起丢掉：租约提前
        // 放行，而线程还在对同一条 PTY 说话。这里显式拒绝（不排队、不顶替），
        // 把原因留给已经开着的那扇窗；等它跑完再按一次即可。
        if let Some(dialog) = self.script.as_mut() {
            if dialog.run.is_some() {
                dialog.error = Some(
                    t(self.view.lang, "A script is already running; wait for it to finish")
                        .to_string(),
                );
                self.ctx.request_repaint();
                return;
            }
        }
        let session = self.tabs.get(self.active_slot()).and_then(|t| t.session);
        self.script = Some(if background {
            ScriptDialog::opened_background(session, path)
        } else {
            ScriptDialog::opened(session, path)
        });
        self.start_script();
        self.ctx.request_repaint();
    }

    /// 读文件 → 起线程。失败留在 `error` 里，不弹第二个窗口。
    fn start_script(&mut self) {
        let lang = self.view.lang;
        // 手勾授权与预算档只在这三处**显式 Run** 的调用点贴上：共享构造器
        // `script_run_options()` 还被登录脚本与触发器的后台运行用着，那两条
        // 什么也不该继承。兼容终端没有这套能力授予，是我们自己的安全设计。
        let allow = self.run_config.allow();
        let budget = self.run_config.budget();
        // 会话与路径先取出来：`script_run_options` 借的是整窗 `self`，之后才能
        // 拿 `dialog` 的可变借用。`who` 用脚本路径——「谁在跑」的提示要能对应到
        // 用户手上那个文件。
        let (session, path, background) = match self.script.as_ref() {
            Some(dialog) => (
                dialog.session,
                dialog.path.trim().to_string(),
                dialog.background,
            ),
            None => return,
        };
        // 参数由 `script_run_options` 统一解析，这里不再单独查一遍注册表。
        let mut options = self
            .script_run_options(&path)
            .with_allow(allow)
            .with_budget(budget);
        if background {
            options = options.with_background();
        }
        let Some(dialog) = self.script.as_mut() else {
            return;
        };
        let Some(id) = session else {
            dialog.error = Some(t(lang, "This tab has no live session").to_string());
            return;
        };
        // 2026-11-19 只读查看：脚本是「往会话里写字」的一整条通道，只读态下
        // 不让它起。判据读 `Tab.local_mode`——与 `Tab::write` 同一位，所以
        // 「键盘发不出去、脚本却能跑」这种自相矛盾不会出现。
        //
        // 为什么在入口拦而不是靠 `write`：脚本拿的是 `SessionPool`（`script_env`
        // 的宿主端口），不经 `Tab::write`。入口拦一次，比在脚本宿主里到处加判断实在。
        if self
            .tabs
            .iter()
            .any(|tab| tab.session == Some(id) && tab.local_mode)
        {
            dialog.error = Some(
                t(lang, "This session is read-only (Local mode); scripts will not run")
                    .to_string(),
            );
            dialog.outcome = None;
            return;
        }
        // 按扩展名选引擎：.py 走外部 Python + stdio 桥，其余照旧走内嵌 Rhai。
        // 两条路共用同一个 ScriptRun 句柄，界面上的停止 / 结果轮询不用分叉。
        let is_python = path.to_ascii_lowercase().ends_with(".py");
        let started = if is_python {
            ScriptRun::start_python(self.pool.clone(), id, &path, options)
        } else {
            let source = match script_run::read_source(&path) {
                Ok(text) => text,
                Err(message) => {
                    dialog.error = Some(message);
                    dialog.outcome = None;
                    return;
                }
            };
            ScriptRun::start(self.pool.clone(), id, source, options).map_err(Into::into)
        };
        match started {
            Ok(run) => {
                dialog.run = Some(run);
                dialog.outcome = None;
                dialog.error = None;
                // 只在**真的跑起来了**之后记账：读不到文件 / 互斥被拒的那些次
                // 不该占据「最近」列表的格子。
                let ran = path.clone();
                self.ctx.request_repaint();
                self.note_recent_script(&ran);
            }
            Err(message) => dialog.error = Some(message.to_string()),
        }
    }
}

/// 按路径查已登记脚本的 `args`。读盘失败 / 没登记时给空串组——按路径直接跑
/// 的脚本本来就不要求先登记，参数缺省为空才是正常语义。
fn registered_script_args(path: &str) -> Vec<String> {
    term_profile::prefs::load()
        .map(|prefs| args_from_prefs(&prefs, path))
        .unwrap_or_default()
}

/// 纯查表，便于单测：同一路径的 `ScriptEntry.args` 原样交给 `RunOptions.args`。
fn args_from_prefs(prefs: &term_profile::prefs::Prefs, path: &str) -> Vec<String> {
    prefs
        .scripts
        .iter()
        .find(|entry| entry.path == path)
        .map(|entry| entry.args.clone())
        .unwrap_or_default()
}

enum ScriptAction {
    None,
    Close,
    Run,
    /// 切到脚本编辑器（可改可存），不再只是「按路径跑一次」。
    Edit,
    /// 叫停在跑的那一次（ADR-0027）。
    Stop,
}

/// 记一步人的输入（键盘漏斗里调，`pane.rs`）。
///
/// 为什么收一个 `Arc` 而不是 `&TerminalWindow`：调用点在 `pane.rs`，那里
/// `tab_of(&mut self.tabs, …)` 已经把 `self` 借走了，再要 `&self` 就是冲突。
/// 把那一格的 `Arc` 先克隆出来，同一个作用域里就能记账——共享状态本来就是
/// 跨线程/跨借用的，`Arc<Mutex<…>>` 正好是它该有的形状。
///
/// 只记**键盘**：设备应答（DSR / DA）与粘贴都不经过这个调用点。那两样不是人
/// 敲的，录进去回放时会多出一堆莫名其妙的字符。
pub(in crate::ui) fn note_recorded_input(
    recorder: &std::sync::Arc<std::sync::Mutex<Option<crate::script_recorder::Recorder>>>,
    bytes: &[u8],
) {
    let Ok(mut slot) = recorder.lock() else {
        return;
    };
    let Some(rec) = slot.as_mut() else {
        return;
    };
    // 键位映射产出的都是完整字符，非 UTF-8 的续块实际上不会出现；真出现也只会
    // 丢这一步的一部分，不影响已录到的东西。
    if let Ok(text) = std::str::from_utf8(bytes) {
        rec.note_input(text, std::time::Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_script_args_reach_the_run_options() {
        let mut prefs = term_profile::prefs::Prefs::default();
        prefs.scripts.push(term_profile::prefs::ScriptEntry {
            name: "备份".into(),
            path: "/tmp/backup.rhai".into(),
            args: vec!["--all".into(), "42".into()],
            description: String::new(),
            enabled: true,
        });
        assert_eq!(
            args_from_prefs(&prefs, "/tmp/backup.rhai"),
            vec!["--all".to_string(), "42".to_string()]
        );
        assert!(args_from_prefs(&prefs, "/tmp/other.rhai").is_empty());
    }

    #[test]
    fn the_status_line_tells_apart_idle_running_done_and_failed() {
        let mut d = ScriptDialog::default();
        assert_eq!(d.status(), "Ready");

        // 「跑完了但没有返回值」不能说成「跑完了：」——那个冒号会让人以为值丢了。
        d.outcome = Some(Outcome::Done(String::new()));
        assert_eq!(d.status(), "Done (no value)");

        d.outcome = Some(Outcome::Done("ok".to_string()));
        assert!(d.status().contains("ok"));

        d.outcome = Some(Outcome::Failed {
            message: "第 3 行：超时".to_string(),
            at: Some((3, 1)),
            kind: crate::script_run::FailureKind::Failed,
        });
        assert!(d.status().starts_with("Failed"), "{}", d.status());
        assert!(d.status().contains("第 3 行"), "{}", d.status());
    }

    /// 脚本在界面里 `ask`：模态框立起来，用户答完，答案真的回到脚本线程。
    ///
    /// 这条是接线的端到端证据：脚本线程阻塞在回信通道上、窗口每帧从队列
    /// 取一个请求画成模态框、点击之后 `deliver` 把答案送回。少了任何一环，
    /// 表现出来都是「脚本卡住」——最难查的那种。
    #[test]
    fn a_script_question_becomes_a_modal_and_the_answer_goes_back() {
        use crate::script_human::PendingAsk;

        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let mut window =
            crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());

        // 「脚本线程」这一侧：把问题排进队列，然后等在回信通道上。
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        window
            .script_asks
            .lock()
            .expect("队列锁")
            .push(PendingAsk {
                seq: 1,
                cancel: term_script::CancelToken::new(),
                request: term_script::Ask::Confirm {
                    title: "确认".to_string(),
                    text: "继续？".to_string(),
                },
                reply: tx,
            });

        // 窗口这一侧：跑一帧把模态框立起来（`egui::Window` 只能在帧内画）。
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            window.poll_script_ask(ctx);
        });
        assert!(window.script_ask.is_some(), "队列里有请求就该立起模态框");

        // 用户点了「是」：把答案交回去。
        let modal = window.script_ask.take().expect("模态框");
        let resolved = overlays::resolve_ask(&modal, term_script::Answer::Bool(true));
        modal.deliver(resolved);
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(1)),
            Ok(term_script::Answer::Bool(true)),
            "答案要真的回到脚本线程"
        );

        // 队列空 = 不立模态框。
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            window.poll_script_ask(ctx);
        });
        assert!(window.script_ask.is_none());
    }

    /// 一条请求被答复之后，队列里下一个立刻接上（脚本可以连问两次）。
    #[test]
    fn a_second_question_is_picked_up_after_the_first_is_answered() {
        use crate::script_human::PendingAsk;

        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let mut window =
            crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
        let channels: Vec<_> = (0..2)
            .map(|index| {
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                window
                    .script_asks
                    .lock()
                    .expect("队列锁")
                    .push(PendingAsk {
                        seq: index as u64 + 1,
                        cancel: term_script::CancelToken::new(),
                        request: term_script::Ask::Text {
                            title: String::new(),
                            prompt: format!("第 {index} 个"),
                            default: String::new(),
                        },
                        reply: tx,
                    });
                rx
            })
            .collect();

        for (index, rx) in channels.iter().enumerate() {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                window.poll_script_ask(ctx);
            });
            let modal = window.script_ask.take().expect("模态框");
            let resolved =
                overlays::resolve_ask(&modal, term_script::Answer::Text(format!("答案{index}")));
            modal.deliver(resolved);
            assert_eq!(
                rx.recv_timeout(std::time::Duration::from_secs(1)),
                Ok(term_script::Answer::Text(format!("答案{index}"))),
                "第 {index} 个问题要拿到它自己的答案"
            );
        }
    }

    /// ADR-0025 的核心行为：脚本线程推进队列的会话，下一帧变成标签页。
    ///
    /// 这条用**真会话**（`open_exec("cat")`）而不是造一个假 id：标签页要绑的是
    /// 池子里活着的那一条，绑错（比如另开一条）正是最容易犯的错——
    /// 用假 id 的话，「绑到一条不存在的会话」也能通过。
    #[test]
    fn a_session_the_script_opened_becomes_a_tab() {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let mut window = crate::ui::lifecycle_test_support::window_with(
            std::sync::Arc::clone(&pool),
            &ctx,
            Vec::new(),
        );

        let session = pool
            .open_exec("cat", 80, 24)
            .expect("起不了 `cat`（本机 PTY 不可用？）");
        let mut config = term_core::session::SessionConfig::new_local_shell("tester");
        config.name = pool.name(session).unwrap_or_else(|| "cat".to_string());
        window
            .script_opened
            .lock()
            .expect("队列锁")
            .push((session, config));

        window.absorb_script_opened();

        assert_eq!(window.tabs.len(), 1, "脚本开的会话必须变成一条标签页");
        assert_eq!(
            window.tabs[0].session,
            Some(session),
            "标签页要绑**同一条**会话，不能另开一条"
        );
        assert_eq!(
            window.open_order,
            vec![window.tabs[0].identity],
            "顺序表要跟着长"
        );
        assert!(
            window.script_opened.lock().expect("队列锁").is_empty(),
            "队列要被取空，否则每帧都会重复建标签"
        );

        // 再吸一次：队列空 = 什么都不做（幂等）。
        window.absorb_script_opened();
        assert_eq!(window.tabs.len(), 1, "空队列不该长出第二条");

        pool.close(session);
    }

    /// 同一条会话第二次出现在队列里时不能建出两条标签页：队列是跨线程的，
    /// 去重比事后解释便宜。
    #[test]
    fn the_same_session_is_never_projected_twice() {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let mut window = crate::ui::lifecycle_test_support::window_with(
            std::sync::Arc::clone(&pool),
            &ctx,
            Vec::new(),
        );

        let session = pool
            .open_exec("cat", 80, 24)
            .expect("起不了 `cat`（本机 PTY 不可用？）");
        let config = term_core::session::SessionConfig::new_local_shell("tester");
        for _ in 0..2 {
            window
                .script_opened
                .lock()
                .expect("队列锁")
                .push((session, config.clone()));
        }

        window.absorb_script_opened();
        assert_eq!(window.tabs.len(), 1, "同一条会话只能有一条标签页");

        pool.close(session);
    }

    /// 登录脚本：标签带着活会话出现之后**自己**跑一次，而且只跑一次。
    ///
    /// 两个判据分开守：不跑 = 功能没接上；跑两次 = 登录脚本干了双份（这类脚本
    /// 常常是 `sudo` 或改配置，跑两遍比不跑更糟）。
    #[test]
    fn a_logon_script_runs_once_when_its_session_tab_appears() {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let session = pool
            .open_exec("cat", 80, 24)
            .expect("起不了 `cat`（本机 PTY 不可用？）");

        // 脚本文件写进临时目录：登录脚本这条路真的会去读盘。
        let dir = std::env::temp_dir().join(format!("desk-logon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("logon.rhai");
        std::fs::write(&path, "1 + 1").expect("写脚本");

        let mut config = term_core::session::SessionConfig::new_local_shell("tester");
        config.logon_script = path.to_string_lossy().into_owned();
        let tab = crate::ui::terminal::setup::tab_from(
            &pool,
            config,
            Some(session),
            egui::Id::new("tab_logon"),
            &ctx,
        );
        let mut window = crate::ui::lifecycle_test_support::window_with(
            std::sync::Arc::clone(&pool),
            &ctx,
            vec![tab],
        );

        window.fire_logon_scripts();
        assert!(window.logon_fired.contains(&session), "应当记下跑过了");
        assert_eq!(window.logon_runs.len(), 1, "应当起来一次");

        // 再跑一帧：不能又起一次。
        window.fire_logon_scripts();
        assert_eq!(window.logon_runs.len(), 1, "不许跑第二遍");

        // 脚本真的跑完了（不是「起了个空线程」）。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let outcome = loop {
            if let Some(outcome) = window
                .logon_runs
                .last_mut()
                .and_then(|run| run.try_outcome())
            {
                break outcome;
            }
            assert!(std::time::Instant::now() < deadline, "登录脚本 10 秒没结果");
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(outcome, Outcome::Done("2".to_string()));

        pool.close(session);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 没有登录脚本的会话不能凭空起一次运行（绝大多数会话都没有）。
    #[test]
    fn a_session_without_a_logon_script_starts_nothing() {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let session = pool
            .open_exec("cat", 80, 24)
            .expect("起不了 `cat`（本机 PTY 不可用？）");
        let config = term_core::session::SessionConfig::new_local_shell("tester");
        let tab = crate::ui::terminal::setup::tab_from(
            &pool,
            config,
            Some(session),
            egui::Id::new("tab_plain"),
            &ctx,
        );
        let mut window = crate::ui::lifecycle_test_support::window_with(
            std::sync::Arc::clone(&pool),
            &ctx,
            vec![tab],
        );

        window.fire_logon_scripts();
        assert!(window.logon_runs.is_empty());
        assert!(window.logon_fired.is_empty(), "没脚本就不该留记录");

        pool.close(session);
    }

    /// 菜单的录制开关与面板按钮共用同一个槽位；键盘漏斗的字节要真的落进步序列。
    /// 这里只测开关与击键接线，空录制不落文件——落库命名由 `script_recorder`
    /// 自己的用例守，免得单测往用户脚本库里写东西。
    #[test]
    fn record_toggle_and_keyboard_tap_share_one_recorder_slot() {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let mut window = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());

        assert!(!window.is_recording());
        assert!(
            window.toggle_recording().unwrap().is_none(),
            "第一次点开始录制"
        );
        assert!(window.is_recording());

        note_recorded_input(&window.recorder, b"ls\r");
        assert_eq!(
            window
                .recorder
                .lock()
                .unwrap()
                .as_ref()
                .expect("录制中")
                .len(),
            1,
            "击键漏斗的一行要落成一步"
        );

        // 空停止不写文件；这里用的是另一条没有输入的窗口，避免动用户脚本库。
        let mut empty = crate::ui::lifecycle_test_support::window_with(
            std::sync::Arc::new(term_session::SessionPool::new()),
            &ctx,
            Vec::new(),
        );
        empty.toggle_recording().unwrap();
        assert!(empty.toggle_recording().unwrap().is_none());
        assert!(!empty.is_recording());
    }
}
