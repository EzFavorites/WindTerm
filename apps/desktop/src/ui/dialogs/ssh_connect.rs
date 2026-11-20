//! R13-d2：原生 SSH 的连接流程（在投影层，不在 `term-app`）。
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
//! 2. **结果到了要主动叫醒界面。** S4 之后空闲不重绘，线程结束时若不
//!    `ctx.request_repaint()`，用户就会看着「Connecting…」到天荒地老。
//! 3. **连上之前不建 tab。** 失败留一个空白标签比什么都不留更让人困惑。

use crate::ui::*;
// ④-0：`Split` 的锚点是窗格身份（`PaneId`），不是下标。
use crate::ui::layout::PaneId;
use std::path::PathBuf;
use term_ssh::{AuthMethod as SshAuth, ConnectHandle, HostKeyPolicy, SshOptions};

/// 纯的那一半（`ask()` / `next()` / `options_for()`）。挂在本模块下而不是
/// `dialogs` 下，是为了让它和状态机待在同一个作用域里——`Connector` 直接
/// `use` 取用，不必多一层 `super::super::`。
#[path = "ssh_ask.rs"]
mod ssh_ask;

// 原样再导出一遍：外部一律经 `ssh_connect::` 引用，拆分不该改变任何调用点的
// 写法（`ssh.rs` 仍然写 `ssh_connect::{ask, next, options_for, Ask, Next}`）。
pub(in crate::ui) use ssh_ask::{
    ask, next, options_for, Ask, Next, Replacement, FIRST_POLICY,
};

/// 连接走到哪一步。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) enum Step {
    Connecting,
    NeedHostKey { fingerprint: String },
    NeedPassword { reason: String },
    /// 主机钥匙与账本里的不一致。**不是**「要不要继续」，是「要不要换掉记录」。
    ReplaceKey { changed: Replacement },
    Failed { message: String },
}

/// 连上之后，这条会话该落到哪儿。
///
/// # 为什么需要它（S1：会话创建单一路径）
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
    /// 类型是不透明句柄（B2）：线程与队列都收在 `term-ssh` 里，
    /// 这里只轮询——与 tab 拿 `SupervisorHandle` 是同一条规矩。
    pub(in crate::ui) rx: Option<ConnectHandle>,
    /// 当前凭据。确认指纹之后要用同一份重连，所以必须存下来。
    pub(in crate::ui) auth: Option<SshAuth>,
    /// 连上之后落到哪儿。见 [`PendingOpen`]。
    pub(in crate::ui) pending: PendingOpen,
    /// 用户已经点过「信任」的指纹：重连时改用 `Tofu`。
    accepted_fingerprint: Option<String>,
}

impl Connector {
    pub(in crate::ui) fn new(config: SessionConfig, step: Step) -> Self {
        Self {
            config,
            step,
            rx: None,
            auth: None,
            pending: PendingOpen::Append,
            accepted_fingerprint: None,
        }
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
    /// R13-b2 起多了一种情况：密码存在保险库里、但库锁着——那时该提示
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
        // 已经信任过的主机用 `Tofu`（记账后继续），否则 `Strict`（先问）。
        let policy = if self.accepted_fingerprint.is_some() {
            HostKeyPolicy::Tofu
        } else {
            FIRST_POLICY
        };
        // 参数拼不出来（目前唯一的来源是手写的 `ProxyJump` 链）就停在失败窗口里，
        // 一句原话交给用户——**不要**退回直连：那会连到一台他没打算连的机器上。
        let opts = match options_for(&self.config, auth.clone(), policy) {
            Ok(opts) => opts,
            Err(e) => {
                self.auth = Some(auth);
                self.step = Step::Failed {
                    message: format!("跳板链（ProxyJump）写得不对：{e}"),
                };
                return;
            }
        };
        self.auth = Some(auth);
        self.step = Step::Connecting;
        // 线程与结果队列都在库里（`ConnectHandle`）；`request_repaint` 经回调
        // 传进去——结果就绪时叫醒这一帧（S4 之后空闲不重绘，不叫结果就躺着没人取）。
        let ctx = ctx.clone();
        self.rx = Some(term_ssh::connect_in_background(opts, move || ctx.request_repaint()));
    }

    /// 用户给了密码：从「等密码」进入「连接中」。
    pub(in crate::ui) fn submit_password(&mut self, password: String, ctx: &egui::Context) {
        self.launch(SshAuth::Password { password }, ctx);
    }

    /// 用户点了「信任这把钥匙」：记下来，然后用同一份凭据重连。
    pub(in crate::ui) fn accept_host_key(&mut self, ctx: &egui::Context) {
        let fingerprint = match &self.step {
            Step::NeedHostKey { fingerprint } => fingerprint.clone(),
            _ => return,
        };
        self.accepted_fingerprint = Some(fingerprint);
        if let Some(auth) = self.auth.clone() {
            self.launch(auth, ctx);
        }
    }

    /// 用户确认「换掉这台机器记着的钥匙」。
    ///
    /// 两件事按顺序做：先改账本（`trust_openssh` 会删旧记录、写新记录），
    /// 再用 **`Strict`** 重连一次。
    ///
    /// 为什么不设 `accepted_fingerprint` 走 `Tofu`：那等于「我刚写进去的钥匙，
    /// 我不验证就直接用」。用 `Strict` 重连才是对它的一次真校验——写错了会立刻
    /// 报出来，而不是留一个「账本和实际不一样」的隐患。
    pub(in crate::ui) fn replace_host_key(&mut self, ctx: &egui::Context) {
        let Step::ReplaceKey { changed } = &self.step else {
            return;
        };
        let changed = changed.clone();
        let Some(path) = self.known_hosts_path() else {
            self.step = Step::Failed {
                message: "找不到 known_hosts 的位置（没有家目录），无法替换记录".to_string(),
            };
            return;
        };
        match term_ssh::known_hosts_admin::trust_openssh(
            &path,
            &changed.host,
            changed.port,
            &changed.key_line,
        ) {
            Ok(_outcome) => {
                if let Some(auth) = self.auth.clone() {
                    self.launch(auth, ctx);
                }
            }
            Err(e) => {
                self.step = Step::Failed {
                    message: format!("替换 known_hosts 记录失败：{e}"),
                };
            }
        }
    }

    /// 这次连接会读写的那份账本。
    ///
    /// 与 `term_ssh` 内部的判定走**同一个**函数（`SshOptions::resolved_known_hosts`），
    /// 否则会出现「校验读 A 文件、换钥写 B 文件」这种极难查的错位。
    fn known_hosts_path(&self) -> Option<PathBuf> {
        let auth = self.auth.clone().unwrap_or(SshAuth::Agent);
        // 这里要的只是**账本的位置**，与跳板链无关：`ProxyJump` 解析失败不该
        // 挡住「换掉这台机器记着的钥匙」（那是主机密钥层的事，与路由无关）。
        // 所以失败时退回一份最小参数，只取 `known_hosts`。
        let opts = options_for(&self.config, auth.clone(), FIRST_POLICY).unwrap_or_else(|_| {
            SshOptions::new(self.config.host.trim(), self.config.username.trim(), auth)
        });
        opts.resolved_known_hosts()
    }
}

/// 对话框里能点的东西。先收集、后执行——否则绘制中改状态会和借用打架。
pub(in crate::ui) enum SshAction {
    None,
    Cancel,
    Trust,
    SubmitPassword,
    /// 换掉这台机器记着的钥匙（危险动作，只在 `Step::ReplaceKey` 里出现）。
    ReplaceKey,
}
