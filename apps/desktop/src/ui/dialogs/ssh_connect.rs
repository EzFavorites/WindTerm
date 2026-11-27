//! 原生 SSH 的连接流程（在投影层，不在 `term-app`）。
//!
//! 为什么这段代码在 `apps/desktop` 而不是 `term-app`：`term-app` 的依赖表里不许有
//! tokio（哪怕是间接的），而 `term-ssh` 带着 russh。于是「什么时候连、用什么
//! 凭据」这个决定只能落在可执行目标这一层——`term-tunnel` 将来也一样。
//!
//! 纯的部分在 [`ssh_ask`]（`ask()` / `next()` / `options_for()`，可单测），
//! 本文件只剩**一步状态机**：起线程、收结果、把用户的选择接回去。
//!
//! 三条必须知道的约束：
//!
//! 1. **连接是同步的，但不能阻塞帧。** `term_ssh::connect` 最多等 15 秒，
//!    所以它在一条自己的线程里跑；界面每帧 `try_recv` 一次。
//! 2. **结果到了要主动叫醒界面。** 之后空闲不重绘，线程结束时若不
//!    `ctx.request_repaint()`，用户就会看着「Connecting…」到天荒地老。
//! 3. **连上之前不建 tab。** 失败留一个空白标签比什么都不留更让人困惑。

use crate::ui::*;
// ④-0：`Split` 的锚点是窗格身份（`PaneId`），不是下标。
use crate::ui::layout::PaneId;
use crate::ui::dialogs::vault;
use term_ssh::{AuthMethod as SshAuth, ConnectHandle};

/// 纯的那一半（`ask()` / `next()` / `options_for()`）。挂在本模块下而不是
/// `dialogs` 下，是为了让它和状态机待在同一个作用域里——`Connector` 直接
/// `use` 取用，不必多一层 `super::super::`。
#[path = "ssh_ask.rs"]
mod ssh_ask;

// 原样再导出一遍：外部一律经 `ssh_connect::` 引用，拆分不该改变任何调用点的
// 写法（`ssh.rs` 仍然写 `ssh_connect::{ask, next, options_for, Ask, Next}`）。
pub(in crate::ui) use ssh_ask::{ask, next, options_for, Ask, Next, FIRST_POLICY};

/// 连接走到哪一步。
///
/// 只有三步：连接中、要密码（原版唯一的凭据弹窗）、失败。主机 key 相关的
/// 确认 / 换钥 / 忘记步骤按 owner 裁定（与原版 1:1）全部移除。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) enum Step {
    Connecting,
    NeedPassword { reason: String },
    Failed { message: String },
}

/// 连上之后，这条会话该落到哪儿。
///
/// # 为什么需要它（会话创建单一路径）
///
/// 内置 SSH 的连接是**异步**的：后台握手可能等十几秒，而 `make_tab` 是同步的
/// （起 PTY 是毫秒级）。于是「复制标签 / 重新打开 / 分屏」这些**立刻就要一个
/// 下标**的入口，只能先把意图记下来，等通道到手再一次性落地。
///
/// 没有这一层时的实际症状：在一条内置 SSH 标签上点 Duplicate，得到的是一个
/// `session = None` 的「(disconnected)」空标签——因为那条路径走 `pool.open`，
/// 而它对 `builtin_ssh` 直接返回 `OpenError::BuiltinChannelNeedsInjection`。
/// 界面照收，一句话都不说，这就是「静默失效」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum PendingOpen {
    /// 追加到标签栏末尾并激活（侧栏「连接」与「复制会话」）。
    Append,
    /// 追加、激活，并把布局切成单窗格（`Reopen Closed`：开回来是独立视图，
    /// 不是塞进现有分屏）。
    AppendSingle,
    /// 追加后切分 `anchor` 这个窗格（`Split Group`：复制配置再开一条）。
    /// `anchor` 是窗格身份（`PaneId`），由 `split_view` 在落地前翻译好。
    Split { anchor: PaneId, horizontal: bool },
    /// 填进一个已存在的槽位（断线重连：位置不变，只换掉那条断掉的会话）。
    ///
    /// 落地时会再核一次边界——连上之前用户可能已经把那条标签关了。
    Replace { slot: usize },
}

/// 一次进行中的连接。
pub(in crate::ui) struct Connector {
    pub(in crate::ui) config: SessionConfig,
    pub(in crate::ui) step: Step,
    /// 正在后台跑的那次连接（`None` = 在等用户回答）。
    ///
    /// 类型是不透明句柄：线程与队列都收在 `term-ssh` 里，
    /// 这里只轮询——与 tab 拿 `SupervisorHandle` 是同一条规矩。
    pub(in crate::ui) rx: Option<ConnectHandle>,
    /// 当前凭据。隧道 / 文件面板要用同一份再开连接，所以必须存下来。
    pub(in crate::ui) auth: Option<SshAuth>,
    /// 连上之后落到哪儿。见 [`PendingOpen`]。
    pub(in crate::ui) pending: PendingOpen,
    /// 本次尝试的代数。每次发起都从窗口领一个新值，回调落地前核对。
    pub(in crate::ui) gen: u64,
    /// 这次连接是自动重连发起的（手动 Reconnect / 侧栏连接不是）。失败时
    /// 只有自动重连会按退出原因显式排下一次。
    pub(in crate::ui) auto: bool,
    /// 建连前错误落盘的开关：复用既有的 `tab.debug`，不另设配置。
    pub(in crate::ui) debug: bool,
}

impl Connector {
    pub(in crate::ui) fn new(config: SessionConfig, step: Step) -> Self {
        Self {
            config,
            step,
            rx: None,
            auth: None,
            pending: PendingOpen::Append,
            gen: 0,
            auto: false,
            debug: false,
        }
    }

    /// 把既有 Debug 日志开关带进来，供建连前错误落盘判定。
    pub(in crate::ui) fn with_debug(mut self, on: bool) -> Self {
        self.debug = on;
        self
    }

    /// 建连前失败的单一出口：先写一条排障 note，再进失败弹窗的 `Step`。
    ///
    /// 日志写入永远静默降级（见 `log_pre_session_error`），所以文案与用户
    /// 可见行为与加日志之前完全一致。
    fn fail(&mut self, stage: term_io::pre_session_log::ErrorStage, message: String) {
        term_io::pre_session_log::log_pre_session_error(stage, &message, self.debug);
        self.step = Step::Failed { message };
    }

    /// 把这次连接标记为自动重连，并钉住它领到的代数。
    pub(in crate::ui) fn mark_auto(&mut self, gen: u64) {
        self.gen = gen;
        self.auto = true;
    }

    /// 指定这条会话连上之后落在哪儿（默认 [`PendingOpen::Append`]）。
    pub(in crate::ui) fn with_pending(mut self, pending: PendingOpen) -> Self {
        self.pending = pending;
        self
    }

    /// 等用户输密码（还没连过）。
    pub(in crate::ui) fn waiting_password(config: SessionConfig) -> Self {
        Self::waiting_password_with(
            config,
            "这个会话没有保存密码（我们也不存明文）".to_string(),
        )
    }

    /// 同 `waiting_password`，但由调用方说明「为什么在问」。
    ///
    /// 多了一种情况：密码存在保险库里、但库锁着——那时该提示
    /// 「去解锁」，而不是「没有保存密码」（那句话会让人以为存丢了）。
    pub(in crate::ui) fn waiting_password_with(config: SessionConfig, reason: String) -> Self {
        Self::new(config, Step::NeedPassword { reason })
    }

    /// 直接失败，不进网络。
    pub(in crate::ui) fn failed(config: SessionConfig, message: String) -> Self {
        Self::new(config, Step::Failed { message })
    }

    /// 起一次（或再起一次）后台连接。
    pub(in crate::ui) fn launch(&mut self, auth: SshAuth, ctx: &egui::Context) {
        // 真凭据只活到参数拼好这一刻；`self.auth` 只留一个运行时引用，
        // Tab / 隧道 / 文件面板以后凭它（未锁定时）再取，锁屏即失效。
        let real = match ssh_ask::resolve_auth(auth) {
            Ok(auth) => auth,
            Err(e) => {
                self.fail(term_io::pre_session_log::ErrorStage::Auth, e.to_string());
                return;
            }
        };
        self.auth = Some(vault::stash_auth(&real, &self.config.name));
        // 与原版 1:1：host key 在客户端内部自动接受，没有确认窗、不写账本，
        // 因此也不需要「确认后重连」这一步。
        let policy = FIRST_POLICY;
        // 参数拼不出来（目前唯一的来源是手写的 `ProxyJump` 链）就停在失败窗口里，
        // 一句原话交给用户——**不要**退回直连：那会连到一台他没打算连的机器上。
        let opts = match options_for(&self.config, real, policy) {
            Ok(opts) => opts,
            Err(e) => {
                self.fail(
                    term_io::pre_session_log::ErrorStage::Connect,
                    format!("跳板链（ProxyJump）写得不对：{e}"),
                );
                return;
            }
        };
        self.step = Step::Connecting;
        // 线程与结果队列都在库里（`ConnectHandle`）；`request_repaint` 经回调
        // 传进去——结果就绪时叫醒这一帧（之后空闲不重绘，不叫结果就躺着没人取）。
        let ctx = ctx.clone();
        self.rx = Some(term_ssh::connect_in_background(opts, move || ctx.request_repaint()));
    }

    /// 用户给了密码：从「等密码」进入「连接中」。
    pub(in crate::ui) fn submit_password(&mut self, password: String, ctx: &egui::Context) {
        // 这串密码会经回显 / 会话日志：先登记脱敏，再用于这一次握手。
        term_io::log::register_secret(&password);
        self.launch(SshAuth::Password { password }, ctx);
    }

}

/// 对话框里能点的东西。先收集、后执行——否则绘制中改状态会和借用打架。
pub(in crate::ui) enum SshAction {
    None,
    Cancel,
    SubmitPassword,
}
