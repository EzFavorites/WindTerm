//! 把「一次 SFTP 会话」收进后台线程，对外只剩「发请求 / 收事件」。
//!
//! 形状照抄 `term_tunnel::start_in_background`：一条专用线程 + `current_thread`
//! 运行时，异步只活在里头。区别只有一处——隧道是**开起来就完了**（`FnOnce` 叫醒一次），
//! 而浏览目录是**一直来回**的，所以 `on_wake` 是 `Fn`：每发一个事件都要叫醒 UI。
//! 忘了叫醒的表现是「点了刷新，界面一直停在上一份列表」——之后空闲不重绘，
//! 这不是小概率，是必然。
//!
//! 为什么不给每个请求起一条线程：SFTP 是**一条会话**上的多路请求，`SftpSession`
//! 自带请求 id 与并发上限。每次列目录重连一遍要付认证与 subsystem 的钱（几百毫秒），
//! 而浏览目录的本质是「连着点几下换目录」。
//!
//! 传输（get / put）也走这条循环，而且是**串行**的：一次传输期间后面排着的请求
//! 要等它传完。这是有意的——同时传多个文件会把一条通道的带宽与并发窗口互相挤占，
//! 进度也就失去意义了。真要并行下载，该开的是第二条会话，不是往这里塞队列。

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use russh_sftp::client::SftpSession;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::cancel::{CancelToken, Disposition, Stop, disposition};
use crate::entry::Entry;
use crate::error::SftpError;
use crate::transfer::{self, Direction, TransferError};
use term_ssh::{Authenticated, SshOptions};

/// UI 发下去的请求。
pub enum Request {
    List(String),
    /// 远端 → 本地。`resume` 为真时接着传，否则覆盖。
    /// `preserve` 为真时传完把远端 mtime 写回本地（服务端没报就不写）。
    Download {
        remote: String,
        local: String,
        resume: bool,
        preserve: bool,
    },
    /// 本地 → 远端。`resume` 为真时接着传，否则覆盖。
    /// `preserve` 为真时传完把本地 mtime 写到远端（服务端没报就不写）。
    Upload {
        local: String,
        remote: String,
        resume: bool,
        preserve: bool,
    },
    /// 远端目录树 → 本地目录（`-r` 下载）。语义见 [`crate::walk`]。
    DownloadTree {
        remote: String,
        local: String,
        resume: bool,
        preserve: bool,
    },
    /// 本地目录树 → 远端目录（`-r` 上传）。
    UploadTree {
        local: String,
        remote: String,
        resume: bool,
        preserve: bool,
    },
    /// 递归扫一棵远端目录树，**只出清单不传文件**（目录同步的输入）。
    ///
    /// 与 [`Request::List`] 的区别：那个列一层、给浏览用；这个递归到底、每条带
    /// `mtime`，是 `term_xfer::sync::plan` 要的形状。应答是一条
    /// [`Event::Scanned`]，不是 N 条 `Listed`——同步层要的是「整棵树到齐了没有」，
    /// 拆成 N 条反而要在调用方重新数。
    ScanTree(String),
    /// 在远端建一个目录（单层；父目录必须存在）。
    ///
    /// 不提供递归建目录：`mkdir -p` 的语义是「一路补到目标」，中途某一层
    /// 因权限失败时该停在哪一层是策略问题，交给调用方连发几次更清楚。
    Mkdir { path: String },
    /// 远端改名 / 移动。目标已存在时**由服务端决定**（多数会覆盖）——
    /// 这里不做「目标存在就拒绝」的预检：预检与服务端之间还有竞态，
    /// 假装自己原子反而是分裂的语义。
    Rename { from: String, to: String },
    /// 删远端文件。目录不会被删掉（服务端会报错），用 [`Request::RemoveDir`]。
    RemoveFile { path: String },
    /// 删一个**空**远端目录。
    ///
    /// 不做递归删除：一个能一条请求清空整棵树的接口，误点的代价太大。
    /// 要删树请在 UI 侧逐层确认后自己发请求（或将来单独设计，带显式确认）。
    RemoveDir { path: String },
    /// 改远端权限位（`0o0000`–`0o7777`）。语义见 [`crate::mutate::merge_permissions`]。
    Chmod { path: String, mode: u32 },
}

/// 后台线程报上来的事。
#[derive(Debug)]
pub enum Event {
    /// 连上了，`home` 是服务端认定的家目录（绝对路径）。UI 用它的第一层开始画。
    Ready { home: String },
    /// 一个目录的完整内容（服务端没分页，SFTP 的 `readdir` 就是这样）。
    Listed { path: String, entries: Vec<Entry> },
    /// 传输进度。`path` 是这次传输的**远端**路径（上传时是目标），
    /// 一对 `path` 的进度流由 `Done` 或 `Failed` 收尾。
    ///
    /// 已节流（每 1 MB 或每 200 ms），可以直接拿去重绘；`total == 0` 表示
    /// 服务端没报大小，百分比无从算起。
    Progress {
        path: String,
        transferred: u64,
        total: u64,
    },
    /// 一次操作成功收尾。
    ///
    /// 历史上只表示「传输完成」，`bytes` 是落盘后的文件大小（续传时是完整
    /// 大小，不是本次新增）。写操作（建目录 / 改名 / 删除 / 改权限）也复用
    /// 这个事件：`bytes` 一律 `0`，**做了什么由发起方自己知道**（它发的请求）。
    ///
    /// 为什么不给写操作单独加一个变体：`Event` 在 `apps/desktop/src/ui/` 里是
    /// 穷尽匹配的，加变体就得同时改 UI；而 UI 正在并行重构。等那边落定，
    /// 再把这四类拆成 `Event::Wrote { action }`——那时的改动是纯增量。
    Done { path: String, bytes: u64 },
    /// 传输被用户取消了。
    ///
    /// `kept` 说明目标位置现在是什么：`true` = 分片留在原地（目标原本就存在，
    /// 那是用户既有数据），`false` = 本次造出来的分片已删。为什么取消也分两种，
    /// 见 `docs/design/05-files-and-transfer.md` 与 [`crate::cancel::disposition`]。
    ///
    /// 取消**不算失败**：UI 不该给它抽一条红条，队列也不该把它计入错误率。
    Cancelled { path: String, kept: bool },
    /// 一次操作失败。列目录时 `path` 是那个目录；传输时是远端路径。
    /// 失败的**动作**（列 / 下载 / 上传）由发起方自己补——它知道自己要了什么。
    Failed { path: String, message: String },
    /// 一棵远端目录树的完整清单（[`Request::ScanTree`] 的应答）。
    ///
    /// 与 [`Event::Listed`] 的区别是**递归**且每条带 `mtime`：这是目录同步的输入，
    /// 不是浏览用的。顺序是遍历顺序（目录内排序后逆序出栈的深度优先），与
    /// `scan_local_entries` 逐条一致——同步两侧按同一套口径比路径与时间。
    Scanned {
        path: String,
        entries: Vec<crate::walk::ScannedEntry>,
    },
    /// 会话没了（连接断了 / 服务器把子系统踢了）。之后不会再有事件。
    Closed { message: String },
}

/// `close()` 等后台线程收敛的上限。
///
/// 正常路径是毫秒级：请求通道一断，线程跳出循环 → 关 SFTP 通道 → 发一条
/// `SSH_MSG_DISCONNECT` → 线程返回。这个上限只兜「传输卡在一个不响应的服务端上」
/// 这一类情形——`close()` 在 UI 线程上调用（关面板 / 关标签），不能为一条已经
/// 半死的 socket 把界面冻住。超时后线程被**放弃**（`JoinHandle` 丢弃即 detach），
/// 它自己会在 socket 报错时收尾，会话不会永远挂着。
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// 轮询 `JoinHandle::is_finished` 的粒度。`std` 没有带超时的 join，只能轮询；
/// 5 ms 对一个「本来就在毫秒级收尾」的线程足够细，也不会把核烧热。
const CLOSE_POLL: Duration = Duration::from_millis(5);

/// 一次 SFTP 会话的句柄。
///
/// # 收尾：显式 [`SftpHandle::close`]，`Drop` 只是兜底
///
/// 旧实现只靠「丢句柄」隐式收尾：`req` 一断，后台线程关掉 SFTP 通道就结束，
/// 但它**从不发 `SSH_MSG_DISCONNECT`**，服务端因此留着一条半死的会话（要到
/// 自己的超时或 `MaxSessions` 顶满才回收）。用户在界面里反复开关文件面板，
/// 会话就一层层堆上去，堆满之后连不上——这就是记的那条泄漏。
///
/// 现在收尾由 [`SftpHandle::close`] 显式完成：断请求通道 → 等线程收敛（有上限）
/// → 后台线程 `sftp.close()` + `auth.disconnect()`。`Drop` 里也调它，因为
/// 「句柄被丢掉」本身就是一个收尾信号，不能因为某条 UI 路径忘了显式 close
/// 就把会话漏在那里。
pub struct SftpHandle {
    /// `None` = 已经 close 过。请求通道的**唯一持有者**就是这里，`close()`
    /// 把它丢掉就等于通知后台线程「没有下一条请求了」。
    req: Option<UnboundedSender<Request>>,
    evt: Receiver<Event>,
    /// 与后台线程共用的取消旗子。
    cancel_token: CancelToken,
    /// 后台线程。`close()` 用它等收敛；`None` = 已经等过（或已经放弃）。
    worker: Option<JoinHandle<()>>,
    /// 上一次 `close()` 的结论（`None` = 还没关过）。幂等的第二次直接重放它。
    converged: Option<bool>,
    closed: bool,
}

impl SftpHandle {
    /// 收掉这条会话：**幂等**，连调两次不会 panic、也不会再等第二次。
    ///
    /// 三步：
    ///
    /// 1. 取消正在跑的传输（`cancel`）。不先做这一步，一个传到一半的文件会让
    ///    后台线程在执行完整个传输之前都读不到「通道已断」，`close()` 就只能
    ///    一路等到超时。
    /// 2. 丢掉请求通道 → 后台线程的 `recv()` 返回 `None`，跳出循环，关 SFTP
    ///    通道并**发 `SSH_MSG_DISCONNECT`**（见 [`start_in_background`] 的收尾）。
    /// 3. 等线程收敛，最多 [`CLOSE_WAIT`]。超时就不等了——线程已 detach，
    ///    socket 报错时它自己会结束；宁可让那一条连接自己烂掉，也不让界面卡住。
    ///
    /// 「会话已经自然断了」之后再调同样安全：线程早就退出了，第 3 步立刻返回。
    ///
    /// 返回值是「后台线程有没有在预算内收敛」：`true` = 已经结束（会话正常
    /// 收尾，或它早就自己断了）；`false` = 等到 [`CLOSE_WAIT`] 还没结束，线程
    /// 被 detach 了。界面用不上这个值，但测试与日志需要——收尾失败如果只发生
    /// 在「没人看的地方」，那就等于没发生过。
    ///
    /// 注意它**不**代表「`SSH_MSG_DISCONNECT` 发出去了」：进程内看不出这条
    /// 区别（socket 迟早都要关），只有服务端知道。这里能保证的是收尾代码
    /// 跑完了，而不是被丢在半路。
    pub fn close(&mut self) -> bool {
        // 幂等判据是「上次的结论在不在」，不是 `closed` 这一个位：事件流先报
        // `Closed` 时 `closed` 已经是真，但那会儿后台线程与请求通道还在手里，
        // 该收的还得收。真收过一次才会留下 `converged`，之后就只重放它。
        if let Some(converged) = self.converged {
            return converged;
        }
        self.closed = true;
        // 正在跑的传输先停：它不会自己看请求通道，只看这面旗子。
        self.cancel_token.cancel();
        drop(self.req.take());
        let mut converged = true;
        if let Some(worker) = self.worker.take() {
            let deadline = Instant::now() + CLOSE_WAIT;
            while !worker.is_finished() {
                if Instant::now() >= deadline {
                    // 放弃等待（`worker` 在这里被丢弃 = detach）。不 panic：
                    // 收尾失败不是调用方的错，界面也不该因此崩。
                    converged = false;
                    break;
                }
                thread::sleep(CLOSE_POLL);
            }
            if converged {
                // 已经结束，join 立刻返回；顺手把线程的 panic 吃掉——
                // 后台线程的失败已经在事件流里报过（`Closed`），不该在关闭
                // 路径上二次炸一次。
                let _ = worker.join();
            }
        }
        self.converged = Some(converged);
        converged
    }

    /// 这条会话是否已经收尾（`close()` 调过，或事件流已经 `Closed`）。
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// 发一条请求。已经 close 过就什么也不做——`close()` 之后不该再有请求
    /// 落到一条正在拆的会话上。
    fn send(&self, req: Request) {
        if let Some(tx) = &self.req {
            let _ = tx.send(req);
        }
    }

    /// 取消**正在跑**的那一次传输。
    ///
    /// 语义能说得这么确定，是因为后台是串行的（一次只跑一个请求，见模块头）：
    /// 「当前传输」只有一个，所以不需要请求 id，也不会错杀别的请求。
    /// 排队中还没轮到的传输不受影响；下一次传输开始时旗子会被清位
    /// （[`CancelToken::reset`]），这次的取消不会顺延过去。
    ///
    /// 收尾一定会来一个 `Event::Cancelled`（不是 `Failed`）；只有一种情况什么
    /// 也不发生——点下去时一条传输都没在跑，那次取消在下一个请求开始时被清掉。
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    /// 请后台列一个目录。会话已经断了就什么也不做（`Closed` 事件已经发过了）。
    pub fn list(&self, path: impl Into<String>) {
        self.send(Request::List(path.into()));
    }

    /// 请后台下载一个远端文件。语义见 [`crate::transfer::resume_offset`]。
    ///
    /// `preserve` 为真时传完把远端 mtime 写回本地（服务端没报就不写）。
    pub fn download(
        &self,
        remote: impl Into<String>,
        local: impl Into<String>,
        resume: bool,
        preserve: bool,
    ) {
        self.send(Request::Download {
            remote: remote.into(),
            local: local.into(),
            resume,
            preserve,
        });
    }

    /// 请后台上传一个本地文件。
    ///
    /// `preserve` 为真时传完把本地 mtime 写到远端（服务端没报就不写）。
    pub fn upload(
        &self,
        local: impl Into<String>,
        remote: impl Into<String>,
        resume: bool,
        preserve: bool,
    ) {
        self.send(Request::Upload {
            local: local.into(),
            remote: remote.into(),
            resume,
            preserve,
        });
    }

    /// 请后台递归下载一个远端目录树（`-r`）。
    ///
    /// 事件流：每个文件一组 `Progress` + `Done` / `Failed`（`path` 是该文件的
    /// 远端路径），最后由树根的 `Done`（`bytes` 是全部文件的总字节）收尾。
    /// 任一文件失败即终止（`Failed` 挂在树根上）——半棵树比错内容的树好排查。
    pub fn download_tree(
        &self,
        remote: impl Into<String>,
        local: impl Into<String>,
        resume: bool,
        preserve: bool,
    ) {
        self.send(Request::DownloadTree {
            remote: remote.into(),
            local: local.into(),
            resume,
            preserve,
        });
    }

    /// 请后台递归上传一个本地目录树（`-r`）。事件流同 [`SftpHandle::download_tree`]。
    pub fn upload_tree(
        &self,
        local: impl Into<String>,
        remote: impl Into<String>,
        resume: bool,
        preserve: bool,
    ) {
        self.send(Request::UploadTree {
            local: local.into(),
            remote: remote.into(),
            resume,
            preserve,
        });
    }

    /// 在远端建一个目录（单层）。成功 → `Done { bytes: 0 }`；失败 → `Failed`。
    pub fn mkdir(&self, path: impl Into<String>) {
        self.send(Request::Mkdir { path: path.into() });
    }

    /// 递归扫一棵远端目录树，只要清单（目录同步用）。
    ///
    /// 成功 → 一条 `Event::Scanned { path, entries }`；不是目录或读不下去 → `Failed`。
    /// **不报进度**：一趟遍历只发一次事件，大目录期间界面靠自己画的「扫描中」兜住。
    pub fn scan_tree(&self, path: impl Into<String>) {
        self.send(Request::ScanTree(path.into()));
    }

    /// 远端改名 / 移动。失败挂在 `from` 上（发起方认的是源路径）。
    pub fn rename(&self, from: impl Into<String>, to: impl Into<String>) {
        self.send(Request::Rename {
            from: from.into(),
            to: to.into(),
        });
    }

    /// 删一个远端文件。目录请用 [`SftpHandle::remove_dir`]。
    pub fn remove_file(&self, path: impl Into<String>) {
        self.send(Request::RemoveFile { path: path.into() });
    }

    /// 删一个**空**远端目录。非空时服务端会拒绝（这里不替它递归）。
    pub fn remove_dir(&self, path: impl Into<String>) {
        self.send(Request::RemoveDir { path: path.into() });
    }

    /// 改远端权限位。`mode` 只取低 12 位（`0o7777`），文件类型位由服务端上报的
    /// 元数据保留——见 [`crate::mutate::merge_permissions`]。
    ///
    /// 事件里没有「新权限是多少」：发起方已经知道自己请求了什么；真想知道结果
    /// 就再 `list` 一次父目录（那才是服务端的事实）。
    pub fn chmod(&self, path: impl Into<String>, mode: u32) {
        self.send(Request::Chmod {
            path: path.into(),
            mode,
        });
    }

    /// 取一个事件；没有就返回 `None`（UI 每帧问一次）。
    pub fn try_event(&mut self) -> Option<Event> {
        if self.closed {
            return None;
        }
        match self.evt.try_recv() {
            Ok(e) => {
                if matches!(e, Event::Closed { .. }) {
                    self.closed = true;
                }
                Some(e)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.closed = true;
                None
            }
        }
    }
}

/// 丢掉句柄 = 收尾。**不是**「唯一的收尾方式」：UI 的关面板 / 换会话 / 关标签
/// 三处都显式调了 [`SftpHandle::close`]（见 `apps/desktop/src/ui/panels/filer.rs`），
/// 这里是防漏网的那一层——漏一条路径，代价就是服务端多留一条半死的 SSH 会话。
///
/// `close()` 自带上限，所以在这里阻塞最多 [`CLOSE_WAIT`]，不会把退出流程挂住。
impl Drop for SftpHandle {
    fn drop(&mut self) {
        // 返回值在这里没有意义：Drop 已经是最外层，没人能补救。
        let _ = self.close();
    }
}

/// 起一条后台 SFTP 会话。`on_wake` 在每个事件之后调一次（UI 传 `request_repaint`）。
///
/// `on_wake` 要 `Sync`：它会被 `Arc` 共享给「连接失败」与「请求处理」两条上报路径。
/// `egui::Context` 本身是 `Send + Sync`（克隆很便宜），所以对调用方没有额外负担。
///
/// 连接失败不在这里返回错误：失败也是**事件**（`Closed`），因为 UI 拿不到
/// 「起线程」的返回值之外的东西——它只能每帧问「有事吗」。
pub fn start_in_background(
    opts: SshOptions,
    on_wake: impl Fn() + Send + Sync + 'static,
) -> SftpHandle {
    let (req_tx, mut req_rx) = unbounded_channel::<Request>();
    let (evt_tx, evt_rx) = channel::<Event>();
    // 线程外层还要用一次（连接失败时要报 `Closed`），所以两个都留一份克隆。
    let report_tx = evt_tx.clone();
    let wake = Arc::new(on_wake);
    // 旗子：句柄一份（响应「取消」），后台一份（每个分片前轮询）。
    let cancel_token = CancelToken::new();
    let worker_cancel = cancel_token.clone();

    let worker = thread::spawn(move || {
        let wake_inner = Arc::clone(&wake);
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = report_tx.send(Event::Closed {
                    message: format!("起不了后台运行时：{e}"),
                });
                wake();
                return;
            }
        };

        let report = |e: SftpError| Event::Closed {
            message: e.to_string(),
        };
        // 显式标注返回类型：块里有几条手写的 `return Err(SftpError::…)`，
        // 不标的话推断会从第一个 `?` 挑成 `SshError`。
        let result: Result<(), SftpError> = rt.block_on(async move {
            // 连不上就没有「挂断」这回事：握手没成，服务端那边也没会话可留，
            // 所以这一个失败点是唯一不需要显式 disconnect 的。
            let auth = match Authenticated::connect(&opts).await {
                Ok(auth) => auth,
                Err(e) => return Err(SftpError::Ssh(e)),
            };
            // 子系统请求不带 want_reply：应答只是「收到」，真正的可用性由
            // 下面的 SFTP 版本握手判定（谈不拢会超时，比一个空洞的 `true` 有意义）。
            let channel = match auth.opener().subsystem("sftp").await {
                Ok(channel) => channel,
                // 会话已经认证成功、服务端确实记着它了——不挂断就是一条半死的
                // 会话。这个失败点最容易被反复触发（服务端没开 sftp 子系统、
                // 或者 MaxSessions 已经顶满），漏掉就是「越重试越连不上」。
                Err(e) => {
                    auth.disconnect().await;
                    return Err(SftpError::Ssh(e));
                }
            };
            let sftp = match SftpSession::new(channel.into_stream()).await {
                Ok(sftp) => sftp,
                Err(e) => {
                    auth.disconnect().await;
                    return Err(SftpError::Session(e.to_string()));
                }
            };
            let home = match sftp.canonicalize(".").await {
                Ok(home) => home,
                Err(e) => {
                    let _ = sftp.close().await;
                    auth.disconnect().await;
                    return Err(SftpError::Session(format!("问不到家目录：{e}")));
                }
            };

            let _ = evt_tx.send(Event::Ready { home });
            wake_inner();

            while let Some(req) = req_rx.recv().await {
                let event = match req {
                    Request::List(path) => match sftp.read_dir(&path).await {
                        Ok(rd) => Event::Listed {
                            path,
                            entries: crate::entry::collect(rd),
                        },
                        Err(e) => Event::Failed {
                            path,
                            message: e.to_string(),
                        },
                    },
                    Request::ScanTree(path) => {
                        match crate::walk::scan_remote_entries(&sftp, &path).await {
                            Ok(entries) => Event::Scanned { path, entries },
                            Err(e) => Event::Failed {
                                path,
                                message: e.to_string(),
                            },
                        }
                    }
                    Request::Download {
                        remote,
                        local,
                        resume,
                        preserve,
                    } => {
                        // 清位紧贴传输开始：句柄是复用的，上一次的取消不许顺延，
                        // 但「点了取消、这次还没开始」的那一下也要能生效——所以
                        // 清在这里，不是收请求之前。
                        worker_cancel.reset();
                        let reporter = Reporter {
                            evt: &evt_tx,
                            wake: &*wake_inner,
                        };
                        transfer_and_report(
                            &sftp,
                            Direction::Download,
                            &remote,
                            &local,
                            TransferOpts {
                                resume,
                                preserve,
                                cancel: &worker_cancel,
                            },
                            &reporter,
                        )
                        .await
                    }
                    Request::Upload {
                        local,
                        remote,
                        resume,
                        preserve,
                    } => {
                        // 清位紧贴传输开始：句柄是复用的，上一次的取消不许顺延，
                        // 但「点了取消、这次还没开始」的那一下也要能生效——所以
                        // 清在这里，不是收请求之前。
                        worker_cancel.reset();
                        let reporter = Reporter {
                            evt: &evt_tx,
                            wake: &*wake_inner,
                        };
                        transfer_and_report(
                            &sftp,
                            Direction::Upload,
                            &remote,
                            &local,
                            TransferOpts {
                                resume,
                                preserve,
                                cancel: &worker_cancel,
                            },
                            &reporter,
                        )
                        .await
                    }
                    Request::DownloadTree {
                        remote,
                        local,
                        resume,
                        preserve,
                    } => {
                        // 清位紧贴传输开始：句柄是复用的，上一次的取消不许顺延，
                        // 但「点了取消、这次还没开始」的那一下也要能生效——所以
                        // 清在这里，不是收请求之前。
                        worker_cancel.reset();
                        let reporter = Reporter {
                            evt: &evt_tx,
                            wake: &*wake_inner,
                        };
                        tree_and_report(
                            &sftp,
                            Direction::Download,
                            &remote,
                            &local,
                            TransferOpts {
                                resume,
                                preserve,
                                cancel: &worker_cancel,
                            },
                            &reporter,
                        )
                        .await
                    }
                    Request::UploadTree {
                        local,
                        remote,
                        resume,
                        preserve,
                    } => {
                        // 清位紧贴传输开始：句柄是复用的，上一次的取消不许顺延，
                        // 但「点了取消、这次还没开始」的那一下也要能生效——所以
                        // 清在这里，不是收请求之前。
                        worker_cancel.reset();
                        let reporter = Reporter {
                            evt: &evt_tx,
                            wake: &*wake_inner,
                        };
                        tree_and_report(
                            &sftp,
                            Direction::Upload,
                            &remote,
                            &local,
                            TransferOpts {
                                resume,
                                preserve,
                                cancel: &worker_cancel,
                            },
                            &reporter,
                        )
                        .await
                    }
                    // 写操作四条：都不走传输、不碰取消旗子（它们不是「一次传输」，
                    // 也没有可中断的中间态）。参数判据先跑纯函数，再发请求——
                    // 能本地判定错的就别花一个往返去问服务端。
                    Request::Mkdir { path } => match crate::mutate::check_path(&path) {
                        Err(message) => Event::Failed { path, message },
                        Ok(()) => {
                            let result = sftp.create_dir(path.as_str()).await;
                            // 新目录的权限：`russh-sftp` 的 `create_dir` 把 attrs
                            // 写死成 `FileAttributes::empty()`，没有带 permissions 的
                            // 变体，只能事后 `setstat`。
                            //
                            // **只在这一步真的建成之后做**：`create_dir` 对已存在目录
                            // 会失败，那条路径不该顺手改掉一个既有目录的权限
                            // （与上传那条「覆盖不动权限」是同一条边界）。
                            // 失败静默：目录已经建好了，权限是锦上添花。
                            if result.is_ok() {
                                crate::preserve::write_remote_attrs(
                                    &sftp,
                                    path.as_str(),
                                    Some(
                                        term_core::config::DEFAULT_DIR_MODE
                                            | term_core::config::S_IFDIR,
                                    ),
                                    None,
                                )
                                .await;
                            }
                            wrote(path, result)
                        }
                    },
                    Request::Rename { from, to } => match crate::mutate::check_path(&from)
                        .and_then(|()| crate::mutate::check_path(&to))
                    {
                        Err(message) => Event::Failed {
                            path: from,
                            message,
                        },
                        Ok(()) => wrote(from.clone(), sftp.rename(from.as_str(), to.as_str()).await),
                    },
                    Request::RemoveFile { path } => match crate::mutate::check_path(&path) {
                        Err(message) => Event::Failed { path, message },
                        Ok(()) => wrote(path.clone(), sftp.remove_file(path.as_str()).await),
                    },
                    Request::RemoveDir { path } => match crate::mutate::check_path(&path) {
                        Err(message) => Event::Failed { path, message },
                        Ok(()) => wrote(path.clone(), sftp.remove_dir(path.as_str()).await),
                    },
                    Request::Chmod { path, mode } => match crate::mutate::check_path(&path)
                        .and_then(|()| crate::mutate::check_mode(mode))
                    {
                        Err(message) => Event::Failed { path, message },
                        Ok(()) => match sftp.metadata(path.as_str()).await {
                            // 取不到元数据就没法保留类型位，下面的 SETSTAT 会把
                            // 类型清成未知——宁可失败，也不写一个坏 mode。
                            Err(e) => Event::Failed {
                                path,
                                message: format!("改权限前要读一次文件属性：{e}"),
                            },
                            Ok(mut attrs) => {
                                attrs.permissions =
                                    Some(crate::mutate::merge_permissions(attrs.permissions, mode));
                                wrote(path.clone(), sftp.set_metadata(path.as_str(), attrs).await)
                            }
                        },
                    },
                };
                let _ = evt_tx.send(event);
                wake_inner();
            }
            // 请求通道断了 = 句柄那边 `close()` 了（或句柄被丢掉）。
            //
            // 两步缺一不可：
            // 1. `sftp.close()` 关掉 SFTP 这一条**通道**（SSH_FXP 层的收尾）；
            // 2. `auth.disconnect()` 发 `SSH_MSG_DISCONNECT` 挂断**整条 SSH 会话**。
            //    只做第 1 步就是记的那条泄漏：通道关了，会话还在服务端挂着，
            //    反复开关文件面板会一层层堆上去，直到 sshd 的 `MaxSessions` 顶满。
            let _ = sftp.close().await;
            auth.disconnect().await;
            Ok::<(), SftpError>(())
        });

        if let Err(e) = result {
            let _ = report_tx.send(report(e));
            wake();
        }
    });

    SftpHandle {
        req: Some(req_tx),
        evt: evt_rx,
        cancel_token,
        worker: Some(worker),
        converged: None,
        closed: false,
    }
}

/// 写操作的结果 → 事件。
///
/// 成功统一用 [`Event::Done`] 且 `bytes = 0`（见该变体的说明）；失败把服务端
/// 的原话带回去——「Permission denied」这类措辞是用户唯一能拿到的线索，
/// 转述成「操作失败」只会让人去找客服。
fn wrote(path: String, result: Result<(), impl std::fmt::Display>) -> Event {
    match result {
        Ok(()) => Event::Done { path, bytes: 0 },
        Err(e) => Event::Failed {
            path,
            message: e.to_string(),
        },
    }
}

/// 后台上报的两个动作：发事件、叫醒 UI。封在一起是因为「发事件 + 叫醒」
/// 总是成对出现，分开传只是多两个参数，还会让 `transfer_and_report` 撞上
/// `too-many-arguments`。
struct Reporter<'a> {
    evt: &'a std::sync::mpsc::Sender<Event>,
    wake: &'a dyn Fn(),
}

impl<'a> Reporter<'a> {
    fn progress(&self, path: &str, transferred: u64, total: u64) {
        let _ = self.evt.send(Event::Progress {
            path: path.to_string(),
            transferred,
            total,
        });
        (self.wake)();
    }

    /// 树传输里一个文件收尾。`Event::Done` 的语义本来就是「这次传输完成」，
    /// 文件级复用它：`path` 是该文件的远端路径，与单文件传输的形状一致。
    fn file_done(&self, path: &str, bytes: u64) {
        let _ = self.evt.send(Event::Done {
            path: path.to_string(),
            bytes,
        });
        (self.wake)();
    }
}

/// 一次传输的三个选项。收成结构不是为了好看：它们总是**一起**从 `Request`
/// 里出来，平铺就是 8 个参数（clippy 的 `too_many_arguments` 上限是 7），
/// 而且以后加「限速」「跳过已存在」时不用再改两处签名。
struct TransferOpts<'a> {
    resume: bool,
    preserve: bool,
    cancel: &'a CancelToken,
}

/// 跑一次传输，把进度即时发出去，把结果收成一个 `Done` / `Cancelled` / `Failed` 事件。
///
/// 进度**不排队**：它是给人看的心跳，中间丢几帧没关系，攒着反而会在传完之后
/// 一次性倾泻。终态（`Done` / `Failed`）由调用方在返回后发送，保证它排在
/// 全部进度之后。
async fn transfer_and_report(
    sftp: &SftpSession,
    direction: Direction,
    remote: &str,
    local: &str,
    opts: TransferOpts<'_>,
    reporter: &Reporter<'_>,
) -> Event {
    let path = remote.to_string();
    let TransferOpts {
        resume,
        preserve,
        cancel,
    } = opts;
    let mut on_progress = |transferred: u64, total: u64| reporter.progress(&path, transferred, total);
    let result = match direction {
        Direction::Download => {
            transfer::download(sftp, remote, local, resume, preserve, cancel, &mut on_progress)
                .await
        }
        Direction::Upload => {
            transfer::upload(sftp, local, remote, resume, preserve, cancel, &mut on_progress).await
        }
    };
    match result {
        Ok(bytes) => Event::Done { path, bytes },
        Err(e) => stop_event(path, e),
    }
}

/// 把一次传输的结果收成终态事件。**取消不等于失败**，所以分两支：
/// 取消按处置表说清分片是留是删，失败保留分片等续传。
fn stop_event(path: String, error: TransferError) -> Event {
    match error {
        TransferError::Cancelled { pre_existing } => Event::Cancelled {
            path,
            kept: disposition(Stop::Cancelled, pre_existing) == Disposition::Keep,
        },
        TransferError::Failed(message) => Event::Failed { path, message },
    }
}

/// 跑一棵树的传输：先展开成计划，再按计划建目录、逐文件传。
///
/// 每个文件自己的 `Progress` / `Done` / `Failed` 挂在该文件的远端路径上；
/// 终态 `Done` 挂在**树根**上（`bytes` = 全部文件的总字节）。任一文件失败即
/// 整树失败——半棵树加一条明确的错误，比静默跳过再让用户猜少了吗强。
///
/// 树是**先完整展开再传**的：展开期间远端的改动不会传一半（快照语义）；
/// 传完后新增的文件不会补传，这与 `rsync` 的「边扫边传」不同，是刻意取舍——
/// 「先扫完」意味着总大小与文件清单在传输开始前就确定，进度才是可预期的。
async fn tree_and_report(
    sftp: &SftpSession,
    direction: Direction,
    remote: &str,
    local: &str,
    opts: TransferOpts<'_>,
    reporter: &Reporter<'_>,
) -> Event {
    let root = remote.to_string();
    let plan = match direction {
        Direction::Download => crate::walk::plan_remote(sftp, remote).await,
        Direction::Upload => crate::walk::plan_local(local),
    };
    let plan = match plan {
        Ok(p) => p,
        Err(message) => return Event::Failed { path: root, message },
    };
    let mut on_progress =
        |path: &str, transferred: u64, total: u64| reporter.progress(path, transferred, total);
    let mut on_file_done = |path: &str, bytes: u64| reporter.file_done(path, bytes);
    let mut job = crate::walk::Job {
        plan: &plan,
        remote_root: remote,
        local_root: local,
        resume: opts.resume,
        preserve: opts.preserve,
        cancel: opts.cancel,
        progress: &mut on_progress,
        file_done: &mut on_file_done,
    };
    let result = crate::walk::run(sftp, direction, &mut job).await;
    match result {
        Ok(bytes) => Event::Done { path: root, bytes },
        Err(e) => stop_event(root, e),
    }
}
