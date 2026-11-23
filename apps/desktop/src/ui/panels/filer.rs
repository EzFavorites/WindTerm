//! Filer：文件面板的数据与数据源。**绘制在 `filer_view.rs`**。
//!
//! 本地 shell 走本地目录，SSH 会话走 SFTP。两条源都归一成 [`FileLine`] 再交给
//! 绘制——夹具与真实运行画的是同一段代码，不是两条分叉的假界面。
//!
//! # 2026-11-08 与新版截图比对
//!
//! 视觉基线里 Filer 是「路径栏 + `Name / Date` 表头 + 蓝色文件夹行 + 底部
//! `N items  size` 状态行」。此前只有 SFTP 一条源：默认 Local Shell 会话没有
//! 凭据，于是整块面板只显示一句「没有文件面板」，左带下半一大片空白。
//! WindTerm 在本地会话下看的其实是**本地目录**，所以补上本地源：
//!
//! - `term_runtime::local_dir` 负责读目录与 `mtime → YYYY/MM/DD`（IO 不进 `ui/`）；
//! - `--demo` 时用 `demo.rs::demo_files()` 的固定条目，基线不钉在真实家目录上；
//! - SSH 会话仍走原来的 `SftpHandle`，`date` 列为空（服务端那条 `Entry` 没有
//!   mtime，不编一个假日期出来）。
//!
//! 面板持有 `SftpHandle`（线程与队列），所以状态放在 `TerminalWindow` 上，
//! 不进 `ViewState`（`show_filer` 那个 bool 才是领域状态）。

use crate::ui::*;
use crate::ui::dialogs::ssh_connect;
use crate::ui::panels::xfer_queue::XferQueue;
use std::path::{Path, PathBuf};
use term_runtime::local_dir::{self, Row};
use term_sftp::{Entry, SftpHandle};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Source {
    #[default]
    Local,
    Sftp,
}

/// 文件面板是否走 SFTP：既要有远端凭据，也要会话的 `Sftp` 子系统开关开着。
///
/// `SessionConfig.sftp` 有 UI 开关与存储，但 `ensure_filer_session` 此前不看它——
/// 关掉后仍会起 SFTP。产品裁定：关掉 Sftp 的会话退回本地用户目录，与本地
/// shell / 系统 ssh 的「无凭据看本地」同一条路径，不另造空态。
fn filer_uses_sftp(config: &SessionConfig, has_auth: bool) -> bool {
    config.sftp && has_auth
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SortBy {
    #[default]
    Name,
    Date,
    Size,
}

impl SortBy {
    fn next(self) -> Self {
        match self {
            SortBy::Name => SortBy::Date,
            SortBy::Date => SortBy::Size,
            SortBy::Size => SortBy::Name,
        }
    }

    fn label(self) -> &'static str {
        match self {
            SortBy::Name => "Name",
            SortBy::Date => "Date",
            SortBy::Size => "Size",
        }
    }
}

/// 面板状态。`Default` = 没绑定任何会话（下次画的时候重连 / 重读目录）。
#[derive(Default)]
pub(in crate::ui) struct Filer {
    /// 当前绑在哪条会话上（按名字认，与属性表单同一套「认名字不认序号」）。
    session: Option<String>,
    handle: Option<SftpHandle>,
    /// SFTP 当前目录；空 = 还不知道（等 `Ready`）。
    path: String,
    entries: Vec<Entry>,
    error: Option<String>,
    loading: bool,
    /// Batch3 的传输进度（get/put 已节流：每 1 MB 或 200 ms 一条）。
    ///
    /// 这是**当前那一次**传输的一行字（`path: 1 / 2 MB`），与下面的队列不是重复：
    /// 队列说的是「这个会话传过的所有东西各到哪一步」，这一行说的是「此刻在动的
    /// 那一个」。两者都留，是因为删掉任何一个都会丢信息。
    progress: Option<String>,
    /// 上传 / 下载的队列（纯逻辑，不持句柄；请求由 `xfer.rs` 发出）。
    ///
    /// 跟着面板走：面板关掉、换会话时整个 `Filer` 重建，队列与它的 SFTP 连接
    /// 一起消失——传输依附于那条连接，连接没了还留着「传输中」的行只是骗人。
    xfer_queue: XferQueue,
    /// 本地数据源的当前目录与条目。
    source: Source,
    local_path: PathBuf,
    local_rows: Vec<Row>,
    /// 路径栏的跨帧输入缓冲（与 `find_input` 同类，不进 `ViewState`）。
    path_input: String,
    /// 列表滚过了几行（按整行计，避免半行错位）。
    scroll: usize,
    sort_by: SortBy,
    /// 整窗夹具注入的固定条目。跨帧保留，设了就不再读磁盘。
    demo_rows: Option<Vec<Row>>,
    /// 等着用户确认的写操作（`None` = 没有弹层）。
    ///
    /// 存的是**绝对路径**而不是「第几行」：等用户点确认时列表可能已经刷新过。
    op: Option<ops::FilerOp>,
    /// 弹层输入框的跨帧缓冲（与 `path_input` 同类，不进 `ViewState`）。
    op_input: String,
    /// 本地校验没过时的那句话。**服务端**的错误仍然走 `error` 槽——
    /// 两者分开是因为一个是「你填错了」，一个是「远端不答应」。
    op_error: Option<String>,
    /// `Preferences ▸ Filer ▸ Show hidden items`：列本地目录时是否带上
    /// `.` 开头的条目（`.git` / `.env`）。
    ///
    /// 存在面板上而不是每帧读 prefs：过滤发生在 `local_dir::list` 那一次 IO 里，
    /// 而列表是跨帧缓存的数据——开关若每帧单独读一次，改了开关就得等下一次
    /// 重列才生效，界面会出现「勾已经打上、列表没变」的一帧。真值仍是
    /// `prefs.filer_show_hidden`，这里只是它的工作副本；重建面板时经
    /// [`Filer::from_prefs`] 重新对齐。
    pub(in crate::ui) show_hidden: bool,
    /// 远端预览的待收件箱：正在等哪一份文件传完。
    ///
    /// 双击远端文件不会凭空出现内容——必须先把整份文件搬到本地（`term_sftp`
    /// 的 `download`），传完才有东西可画。所以这里记住「我们在等哪个远端路径、
    /// 它落到哪、屏上该显示成什么」，`poll_filer` 收到它的 `Done` 时开窗。
    ///
    /// 一次只等一个：预览窗一次只显示一个文件，后点的那个直接顶掉前一个
    /// （前一个的下载不会被取消，`Done` 回来时匹配不上就什么也不做）。
    pending_preview: Option<PendingPreview>,
    /// 目录同步：`Some` = 正在扫描 / 待审阅 / 正在执行。
    ///
    /// 跟着面板走，理由同 `xfer_queue`：同步依附于这条会话的远端目录，
    /// 面板重建后一份属于旧目录的计划没有任何意义。
    sync: Option<sync::SyncState>,
}

impl Filer {
    /// 此刻有没有文件面板自己的弹层（写操作确认 / 目录同步三窗）。
    ///
    /// 供整窗的模态灰罩判定使用：`op` / `sync` 是本模块的私有字段，
    /// 不能让 `ui/mod.rs` 直接摸；一个布尔出口也免得将来多一处漏算。
    pub(in crate::ui) fn has_modal(&self) -> bool {
        self.op.is_some() || self.sync.is_some()
    }
}

/// 一次「为了预览而下载」的在途请求。
///
/// 三个字段缺一不可：`remote` 用来认事件（`Done::path` 是远端路径），`local`
/// 是下载落点（开窗要一个真实的本地路径），`display` 是屏上要显示的
/// `host:/path`——远端文件的本地副本是临时文件，把它画给用户等于骗人。
struct PendingPreview {
    remote: String,
    local: PathBuf,
    display: String,
}

impl Filer {
    /// 夹具专用：固定条目 + 未绑定会话，下一帧 `ensure_filer_session` 据此铺表。
    ///
    /// `show_hidden` 仍从 prefs 取：夹具只替换**条目**，开关属于用户偏好。
    pub(in crate::ui) fn with_demo_rows(rows: Vec<Row>) -> Self {
        Filer {
            demo_rows: Some(rows),
            ..Filer::from_prefs()
        }
    }

    /// 面板重建时的初值：`show_hidden` 只认 prefs。
    ///
    /// 三处重建（切会话、关掉面板、关标签）都走这里。直接 `..Filer::default()`
    /// 会让勾上的开关在面板重开时悄悄弹回去——默认值恰好是出厂 `false`，
    /// 症状是「设置里明明勾着，面板还是看不到隐藏文件」。
    pub(in crate::ui) fn from_prefs() -> Self {
        Filer {
            show_hidden: term_runtime::prefs::load().is_some_and(|p| p.filer_show_hidden),
            ..Filer::default()
        }
    }
}

/// 表格只认这个。两条数据源先归一，绘制处就不必有第二套分支。
pub(super) struct FileLine {
    /// 画在屏上的名字：目录带 `/`、链接带 `@`（见 [`row_label`]）。
    name: String,
    /// 真名。**写操作只认这个**：拿着 `name` 去改名会把尾斜杠一起写进去。
    raw_name: String,
    is_dir: bool,
    is_symlink: bool,
    size: u64,
    /// `YYYY/MM/DD`；SFTP 拿不到 mtime 时为空。
    date: String,
    /// 点目录行时要去的目标路径。
    target: String,
}

/// 一行怎么显示：目录带尾斜杠，链接带 `@`。
///
/// 尾斜杠不是装饰：`read_dir` 给的 `is_dir` 是 `lstat` 结果，而符号链接指向目录时
/// `is_dir` 是 `false`。用 `@` 标出链接、用 `/` 标出目录，两个信息都不丢。
pub(in crate::ui) fn row_label(entry: &Entry) -> String {
    let mut label = entry.name.clone();
    if entry.is_symlink {
        label.push('@');
    }
    if entry.is_dir {
        label.push('/');
    }
    label
}

/// 上一级目录；已在根或不知道该往哪走时 `None`（按钮据此置灰）。
pub(in crate::ui) fn up_target(path: &str) -> Option<String> {
    term_sftp::parent(path)
}

/// 目录恒在前，排序只决定同组内部次序（视觉基线里 4 个文件夹在最上面）。
pub(super) fn sort_lines(lines: &mut [FileLine], by: SortBy) {
    lines.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| match by {
            SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortBy::Date => b.date.cmp(&a.date),
            SortBy::Size => b.size.cmp(&a.size),
        })
    });
}

impl TerminalWindow {
    /// 收掉面板当前那条 SFTP 会话。
    ///
    /// 三处重建（换会话 / 关面板 / 会话结束）都必须先过这里，再换新的 `Filer`。
    /// 为什么不是「丢句柄让 `Drop` 去收」：`Drop` 里不能 await，后台线程只能
    /// 被动地关通道；而要的是**可观测**的显式收尾——`SftpHandle::close()`
    /// 会等后台线程收敛（有上限），再由后台线程 `auth.disconnect()` 发出
    /// `SSH_MSG_DISCONNECT`。少了这一下，用户反复开关文件面板，服务端就一层层
    /// 堆半死的会话，顶满 `MaxSessions` 之后连不上。
    pub(in crate::ui) fn close_filer_handle(&mut self) {
        if let Some(mut handle) = self.filer.handle.take() {
            // 返回值是「后台线程有没有在预算内收敛」。面板已经要关了，界面没有
            // 能做的补救动作，所以丢掉它——但它存在不是多余的：测试靠它判断
            // 「收尾跑完了」，而不是「线程碰巧停了」。
            let _ = handle.close();
        }
    }

    /// 面板的会话对齐：切了标签、换了连接、或者刚打开面板，都在这里重开。
    pub(in crate::ui) fn ensure_filer_session(&mut self) {
        let Some(tab) = self.tabs.get(self.active_slot()) else {
            return;
        };
        let name = tab.config.name.clone();
        if self.filer.session.as_deref() == Some(name.as_str()) {
            return;
        }
        // 换了会话：旧会话显式挂断（见 `close_filer_handle` 的说明）。
        // demo_rows 是夹具的跨帧资产，不能跟着一起清空；`show_hidden` 从 prefs
        // 重新取（`Filer::from_prefs`），否则开关会随重建掉回出厂态。
        self.close_filer_handle();
        let demo_rows = self.filer.demo_rows.take();
        self.filer = Filer {
            session: Some(name),
            demo_rows,
            ..Filer::from_prefs()
        };

        let tab = self.tabs.get(self.active_slot()).expect("上一行刚查过");
        // 没有远端凭据（本地 shell / 系统 ssh）或会话显式关掉了 Sftp 子系统时，
        // 都退回本地用户目录——与面板既有的「无凭据看本地」同一条路径，不另造空态。
        if !filer_uses_sftp(&tab.config, tab.auth.is_some()) {
            self.show_local_dir_in_filer();
            return;
        }
        let auth = tab
            .auth
            .clone()
            .expect("上面已确认有凭据且 Sftp 开着");
        self.filer.source = Source::Sftp;
        // 主机密钥用 `Tofu`：终端那条连接已经裁决过（能连上说明账本里有它），
        // 这里再问一遍指纹只会让用户困惑「为什么又问」。
        // 参数拼不出来（手写的 `ProxyJump` 链有误）时**不 panic、也不静默**：
        // 面板自己把那句话显示在既有的 `error` 槽里，用户去改会话属性即可。
        let opts = match ssh_connect::options_for(&tab.config, auth, term_ssh::HostKeyPolicy::Tofu) {
            Ok(opts) => opts,
            Err(e) => {
                self.filer.error = Some(format!("会话的连接参数有误，无法打开 SFTP：{e}"));
                self.filer.loading = false;
                return;
            }
        };
        let ctx = self.ctx.clone();
        self.filer.handle = Some(term_sftp::start_in_background(opts, move || {
            ctx.request_repaint()
        }));
        self.filer.loading = true;
    }

    /// 每帧收事件。面板关着就把连接收掉——没有界面还留一条连接是纯浪费。
    pub(in crate::ui) fn poll_filer(&mut self) {
        if !self.view.show_filer {
            if self.filer.handle.is_some() || self.filer.session.is_some() {
                // 用户把面板关了：这条会话到此为止。显式 close 而不是删掉结构体
                // 让它自己烂掉——服务端那边要看到一次挂断，否则会话累积。
                self.close_filer_handle();
                let demo_rows = self.filer.demo_rows.take();
                self.filer = Filer {
                    demo_rows,
                    ..Filer::from_prefs()
                };
            }
            return;
        }
        // 队列的时钟：`now` 由界面层给（`term-xfer` 与 `queue.rs` 都不读墙钟）。
        // 秒是队列的口径（退避 2/4/8 秒），取整到秒足够。
        let now = self.ctx.input(|i| i.time).max(0.0) as u64;
        // 速率要毫秒，而 `time` 已截到秒——分辨率 1 秒够用（`RateMeter` 的窗口
        // 下限是 100 ms，同一秒里的重复采样自有「时间没前进」那条分支挡住）。
        let now_ms = now * 1000;
        let Some(mut handle) = self.filer.handle.take() else {
            // 没有句柄也要推进：同步的扫描那半不依赖句柄（远端请求早发出去了、
            // 本地线程也不看它），排队项此时只能判失败 / 等退避，不推的话它们
            // 会永远停在「排队」上。
            if self.filer.poll_sync(None, now_ms) {
                self.ctx.request_repaint();
            }
            self.pump_transfer(now);
            return;
        };
        let mut events = Vec::new();
        while let Some(event) = handle.try_event() {
            events.push(event);
        }
        for event in events {
            match event {
                term_sftp::Event::Ready { home } => {
                    self.filer.path = home.clone();
                    // 第一屏就是家目录——不让用户先看到「根目录里有什么」。
                    handle.list(home);
                    self.filer.path_input = self.filer.path.clone();
                    self.filer.loading = true;
                }
                term_sftp::Event::Listed { path, entries } => {
                    self.filer.path = path;
                    self.filer.entries = entries;
                    self.filer.error = None;
                    self.filer.loading = false;
                    self.filer.progress = None;
                    self.filer.path_input = self.filer.path.clone();
                }
                term_sftp::Event::Progress {
                    path,
                    transferred,
                    total,
                } => {
                    // 一行字给「此刻在动的那个文件」，队列给「这一次排队里各项各到
                    // 哪一步」——同一条事件的两个读者，都不是对方的副本。
                    self.filer.xfer_queue.on_progress(&path, transferred);
                    // 同步执行器也在听同一批事件，只认自己在途的那个远端路径。
                    self.filer.note_sync_progress(&path, transferred);
                    // 同步在跑时那行字归执行窗自己画（它有自己的进度与失败原话），
                    // 这里再写一遍会让 `progress` 槽在同步结束后留下一条「某文件
                    // 500/900 B」的残影。
                    if matches!(self.filer.sync, Some(sync::SyncState::Running(_))) {
                        continue;
                    }
                    self.filer.progress = Some(if total > 0 {
                        format!(
                            "{path}: {} / {} B ({:.0}%)",
                            transferred,
                            total,
                            transferred as f64 / total as f64 * 100.0
                        )
                    } else {
                        format!("{path}: {transferred} B")
                    });
                }
                term_sftp::Event::Done { path, bytes } => {
                    // `bytes == 0` 是写操作（mkdir / rename / remove / chmod）的
                    // 完成信号，不是「传完了 0 字节」。给它写一行「done (0 B)」
                    // 只会让人以为刚才传了个空文件——目录列表马上会刷新，
                    // 刷新本身就说明了「成了」。
                    //
                    // 队列那边不用判：它只认自己发出去的路径，写操作的路径不在
                    // 表里，`on_done` 会直接返回。树传输也把 bytes 交给它——
                    // 文件级 Done 用来记这一棵树的累计字节，树根 Done 的 bytes
                    // 是整棵树的总字节。
                    self.filer.xfer_queue.on_done(&path, bytes);
                    // 同步执行器也要认这一步：它只认自己在途的那个远端路径，
                    // 认不上（普通传输 / 写操作）就什么也不做。
                    self.filer.note_sync_done(&path, bytes);
                    if bytes > 0 && !matches!(self.filer.sync, Some(sync::SyncState::Running(_))) {
                        self.filer.progress = Some(format!("{path}: done ({bytes} B)"));
                    }
                    // 这次 Done 是不是「为了预览而下载」的那一份？是就开窗。
                    // 判据只用远端路径，**不用 `bytes`**：`bytes == 0` 在这个事件
                    // 上是「写操作完成」的信号，但一个 0 字节的文件下载完也是 0
                    // ——拿它当判据会让「预览一个空文件」什么也不发生。我们知
                    // 道自己发的是 download，所以路径对上了就是它。
                    let wanted =
                        matches!(&self.filer.pending_preview, Some(p) if p.remote == path);
                    if wanted {
                        if let Some(pending) = self.filer.pending_preview.take() {
                            self.open_preview_remote(&pending.display, &pending.local);
                        }
                    }
                }
                term_sftp::Event::Cancelled { path, kept } => {
                    // 取消不是错误：写进 progress 槽，不要占 error（那会画成红条）。
                    self.filer.loading = false;
                    self.filer.xfer_queue.on_cancelled(&path);
                    self.filer.progress = Some(if kept {
                        format!("{path}: cancelled (partial kept for resume)")
                    } else {
                        format!("{path}: cancelled (partial removed)")
                    });
                }
                term_sftp::Event::Scanned { path, entries } => {
                    // 只认自己等的那棵树（状态机内部按根路径比对）：扫描期间用户可能
                    // 已经切到别的目录又发起了一次，旧清单不能算进新计划。
                    self.filer.note_scan_remote(&path, entries);
                }
                term_sftp::Event::Failed { path, message } => {
                    // 远端扫描 / 同步中某一步失败都走这一支（前者挂在树根上，后者挂在
                    // 那一步的远端路径上）。两个入口各自只认自己的路径，认不上就什么
                    // 也不做——同一支还要服务列目录与普通传输的失败。
                    self.filer.fail_scan_remote(&path, message.clone());
                    self.filer.note_sync_failed(&path, &message);
                    // 队列先收：它要按「还有没有重试额度」决定这一项是回排队
                    // 还是终态。`path` 不在表里（列目录 / 写操作失败）时它什么也不做。
                    self.filer.xfer_queue.on_failed(&path, &message, now);
                    // 为预览而下的那一份失败了——把等待摘掉并说一句，
                    // 否则下一次同路径的 Done 会开一个内容不对的窗。
                    if matches!(&self.filer.pending_preview, Some(p) if p.remote == path) {
                        self.filer.pending_preview = None;
                        self.log_note = Some(format!(
                            "{}: {message}",
                            t(self.view.lang, "Could not download the file")
                        ));
                    }
                    self.filer.error = Some(format!("{path}：{message}"));
                    self.filer.loading = false;
                    self.filer.progress = None;
                }
                term_sftp::Event::Closed { message } => {
                    // 执行中的同步没法继续了：把状态清掉并说明原因，否则界面会停在
                    // 「3/12」上再也不动。
                    self.filer.abort_sync(message.clone());
                    self.filer.error = Some(message);
                    self.filer.loading = false;
                    // 会话没了，等着的预览也等不到了。
                    self.filer.pending_preview = None;
                    // 不再放回句柄：会话已经没了，留着它只会让人以为还能刷新。
                    // 队列里还在排队的项由 `pump_transfer` 按「没有句柄」逐条判掉。
                    self.pump_transfer(now);
                    return;
                }
            }
        }
        // 执行阶段的一步要借句柄发请求，所以放在「事件已取完、句柄还没放回」这
        // 一小段里；扫描阶段不看句柄，两种状态都由 `poll_sync` 一个入口推进。
        let busy = self.filer.poll_sync(Some(&handle), now_ms);
        self.filer.handle = Some(handle);
        self.pump_transfer(now);
        if busy {
            self.ctx.request_repaint();
        }
    }

    /// 两条数据源归一成表格行。返回拥有所有权的值，绘制时不再借 `self`。
    ///
    /// 只给 `view` 子模块用（同模块的私有方法），所以不必提可见性。
    fn filer_lines(&self) -> Vec<FileLine> {
        if self.filer.source == Source::Local {
            let base = self.filer.local_path.clone();
            self.filer
                .local_rows
                .iter()
                .map(|r| FileLine {
                    name: r.name.clone(),
                    raw_name: r.name.clone(),
                    is_dir: r.is_dir,
                    is_symlink: r.is_symlink,
                    size: r.size,
                    date: r.date.clone(),
                    target: base.join(&r.name).to_string_lossy().into_owned(),
                })
                .collect()
        } else {
            let base = self.filer.path.clone();
            self.filer
                .entries
                .iter()
                .map(|e| FileLine {
                    name: row_label(e),
                    raw_name: e.name.clone(),
                    is_dir: e.is_dir,
                    is_symlink: e.is_symlink,
                    size: e.size,
                    date: String::new(),
                    target: e.path_in(&base),
                })
                .collect()
        }
    }

    /// 会话断开 / 关标签时把面板也收掉：连接不该比它的会话活得久。
    pub(in crate::ui) fn close_filer_of(&mut self, session: &str) {
        if self.filer.session.as_deref() == Some(session) {
            // 标签都关了，还留着一条已认证的 SSH 会话是最典型的泄漏形态：
            // 用户看不见它，服务端却要一直替它占一个会话位。
            self.close_filer_handle();
            let demo_rows = self.filer.demo_rows.take();
            self.filer = Filer {
                demo_rows,
                ..Filer::from_prefs()
            };
        }
    }

    /// 把面板切到本地用户目录（无远端凭据 / 关掉 Sftp 的会话）。
    ///
    /// 从 `ensure_filer_session` 抽出，是为了两条入口共用同一份「看本地」路径，
    /// 不会出现「关掉 Sftp 进去看到的东西」与「本地 shell 进去看到的」两套。
    fn show_local_dir_in_filer(&mut self) {
        self.filer.source = Source::Local;
        let path = if self.filer.demo_rows.is_some() {
            PathBuf::from("/Users/unique/")
        } else {
            local_dir::home()
        };
        self.filer.local_path = path.clone();
        self.filer.path_input = path.to_string_lossy().into_owned();
        self.reload_local_dir();
    }

    /// 重列当前本地目录（`Show hidden items` 改了就当场调它）。
    ///
    /// 本地面板的 `local_rows` 只有两处来源：这里是**唯一**的重列入口，
    /// 另一处是 `show_local_dir_in_filer`（换会话）。夹具注入的固定条目优先——
    /// 截图基线不能钉在真实家目录上（见模块注释），过滤对它们不适用。
    pub(in crate::ui) fn reload_local_dir(&mut self) {
        if let Some(rows) = &self.filer.demo_rows {
            self.filer.local_rows = rows.clone();
            return;
        }
        self.filer.local_rows = local_dir::list(&self.filer.local_path, self.filer.show_hidden);
    }

    /// 进入一个本地目录。返回 `false` 表示它不是目录（什么也没改）。
    ///
    /// 抽成方法而不是在绘制处内联，是因为**两条入口都要它**：单击 / 回车走的
    /// 路径栏落点，与双击走的 [`TerminalWindow::open_filer_entry`]。两处各写一遍
    /// 「设路径 + 重列」就是两份会漂移的实现——「进得去但列表没变」这种 bug
    /// 只在其中一份上出现，最难查。
    pub(in crate::ui) fn enter_filer_dir(&mut self, target: &str) -> bool {
        let path = PathBuf::from(target);
        if !path.is_dir() {
            return false;
        }
        self.filer.local_path = path;
        // 走与「勾选 Show hidden items」同一条重列路径：过滤开关只有一份实现。
        self.reload_local_dir();
        self.filer.path_input = target.to_string();
        self.filer.scroll = 0;
        true
    }

    /// 双击一行（或右键「预览」）的落地。
    ///
    /// `index` 是**排序后整张表**的下标，不是屏上那几行里的下标：行序由
    /// [`sort_lines`] 每帧从同一份数据确定性地算出，所以同一帧内索引稳定。
    /// 越界（列表刚刷过一轮）时什么也不做——不 panic，也不猜用户想开哪一行。
    ///
    /// 三条源归一成一个入口，是因为「双击一个文件会发生什么」只该有一个答案：
    /// 目录进得去、本地文件立刻预览、远端文件先下载再预览。
    pub(in crate::ui) fn open_filer_entry(&mut self, index: usize) {
        let mut lines = self.filer_lines();
        sort_lines(&mut lines, self.filer.sort_by);
        let Some(line) = lines.get(index) else {
            return;
        };
        if line.is_dir {
            if self.filer.source == Source::Sftp {
                if let Some(handle) = self.filer.handle.as_ref() {
                    handle.list(line.target.clone());
                    self.filer.loading = true;
                    self.filer.error = None;
                    self.filer.scroll = 0;
                }
                return;
            }
            self.enter_filer_dir(&line.target);
            return;
        }
        match self.filer.source {
            Source::Local => self.open_preview_local(Path::new(&line.target)),
            Source::Sftp => self.start_remote_preview(&line.target, &line.raw_name),
        }
    }

    /// 远端文件：先下载到临时目录，`Done` 回来后自动开窗（见
    /// [`PendingPreview`]）。期间只给一句一次性提示，**不做假进度条**——
    /// 进度是 Transfer 面板的事，这里出现第二条进度只会让人猜哪个是真的。
    fn start_remote_preview(&mut self, remote: &str, name: &str) {
        let lang = self.view.lang;
        let local = match crate::open_with::temp_preview_path(name) {
            Ok(path) => path,
            Err(err) => {
                self.log_note = Some(format!(
                    "{}: {err}",
                    t(lang, "Could not download the file")
                ));
                return;
            }
        };
        let Some(handle) = self.filer.handle.as_ref() else {
            // 没有连接就没有「稍等一下」这回事：直说原因，别让用户等一个
            // 永远不会来的事件。
            self.log_note = Some(t(lang, "Not connected to SFTP").to_string());
            return;
        };
        handle.download(remote, local.to_string_lossy().into_owned(), false, false);
        self.filer.pending_preview = Some(PendingPreview {
            remote: remote.to_string(),
            local,
            display: self.remote_display(remote),
        });
        self.log_note = Some(format!(
            "{} {name}",
            t(lang, "Downloading to a temporary file")
        ));
        // 传输线程不会通知 egui：之后空闲不重绘，不主动要一帧的话
        // 「已开始下载」这句提示要等到用户碰鼠标才出现。
        self.ctx.request_repaint();
    }

    /// 屏上要显示的远端路径：`host:/path`。主机名取自当前标签的会话配置——
    /// 远端文件离开那条连接就没有身份，把 `host` 带着才能让用户认出来源。
    fn remote_display(&self, remote: &str) -> String {
        let host = self
            .tabs
            .get(self.active_slot())
            .map(|tab| tab.config.host.clone())
            .unwrap_or_default();
        if host.is_empty() {
            remote.to_string()
        } else {
            format!("{host}:{remote}")
        }
    }
}

/// 绘制（拆出）。**挂在 `filer` 模块下**而不是 `panels` 下，是为了能直接
/// 用 `Filer` 的私有字段，不必把十几个字段逐个提成 `pub(super)`。
#[path = "filer_view.rs"]
mod view;

/// 写操作：新建目录 / 改名 / 删除 / 改权限。同样挂在 `filer` 下，理由同上——
/// 它要读写 `Filer` 的 `op` / `op_input` / `op_error` 与 `handle`。
#[path = "filer_ops.rs"]
pub(in crate::ui) mod ops;

/// 上传 / 下载：右键入口的判定、请求的发出、队列推进。挂在这里同理（要碰
/// `handle` 与 `xfer_queue`），同时它与 `ops.rs` 是两类事——`ops` 改远端，
/// 这个搬字节，失败方式完全不同。
#[path = "filer_xfer.rs"]
pub(in crate::ui) mod xfer;

/// 目录同步：扫描 → 审阅 → 执行的状态机。挂在这里是因为它要碰
/// `handle`（发请求）、`local_path`（本地根）与 `progress`（报结果）。
#[path = "filer_sync.rs"]
pub(in crate::ui) mod sync;

/// 同步的两个窗（扫描中 / 审阅页 / 执行进度）的绘制。与 `sync` 分开的理由同
/// `filer.rs` / `filer_view.rs`：那边是状态与推进，这边只画。
#[path = "filer_sync_view.rs"]
mod sync_view;

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool, is_symlink: bool) -> Entry {
        Entry {
            name: name.to_string(),
            is_dir,
            is_symlink,
            size: 0,
            // 夹具不关心 mtime：显示层不用它，只有目录同步的判定才用。
            mtime: None,
        }
    }

    fn line(name: &str, is_dir: bool, date: &str, size: u64) -> FileLine {
        FileLine {
            name: name.to_string(),
            raw_name: name.to_string(),
            is_dir,
            is_symlink: false,
            size,
            date: date.to_string(),
            target: name.to_string(),
        }
    }

    #[test]
    fn filer_uses_sftp_only_with_both_credentials_and_the_session_switch() {
        let mut config = SessionConfig::new(Protocol::Ssh);
        // 两个条件缺一不可：没凭据（本地 shell / 系统 ssh）看本地；
        // 有凭据但用户关掉 Sftp 子系统，也看本地用户目录。
        assert!(!filer_uses_sftp(&config, false), "无凭据不该起 SFTP");
        config.sftp = false;
        assert!(!filer_uses_sftp(&config, true), "关掉 Sftp 不该起 SFTP");
        config.sftp = true;
        assert!(filer_uses_sftp(&config, true), "两项都满足才起 SFTP");
    }

    #[test]
    fn directories_carry_a_trailing_slash_and_links_an_at_sign() {
        assert_eq!(row_label(&entry("etc", true, false)), "etc/");
        assert_eq!(row_label(&entry("notes.txt", false, false)), "notes.txt");
        // 链接指向目录时 `is_dir` 为假——两个标记都要在，信息才不丢。
        assert_eq!(row_label(&entry("latest", false, true)), "latest@");
    }

    #[test]
    fn the_up_button_knows_when_there_is_no_up() {
        assert_eq!(up_target("/home/demo").as_deref(), Some("/home"));
        assert_eq!(up_target("/home").as_deref(), Some("/"));
        assert_eq!(up_target("/"), None, "根目录没有上一级");
        assert_eq!(up_target(""), None, "还不知道路径时不要给方向");
    }

    #[test]
    fn sorting_keeps_directories_on_top_in_every_mode() {
        let mut lines = vec![
            line("a.txt", false, "2026/11/01", 900),
            line("Zeta", true, "2026/11/02", 0),
            line("b.txt", false, "2026/11/03", 10),
        ];
        for by in [SortBy::Name, SortBy::Date, SortBy::Size] {
            sort_lines(&mut lines, by);
            assert_eq!(lines[0].name, "Zeta", "{by:?} 把文件排到了目录前面");
        }
    }

    #[test]
    fn sorting_by_size_and_date_really_changes_the_order() {
        let mut by_date = vec![
            line("a", false, "2026/11/01", 1),
            line("b", false, "2026/11/09", 2),
        ];
        sort_lines(&mut by_date, SortBy::Date);
        assert_eq!(by_date[0].name, "b");
        let mut by_size = vec![
            line("a", false, "2026/11/01", 1),
            line("b", false, "2026/11/09", 2),
        ];
        sort_lines(&mut by_size, SortBy::Size);
        assert_eq!(by_size[0].name, "b");
        let mut by_name = vec![line("z", false, "", 9), line("a", false, "", 1)];
        sort_lines(&mut by_name, SortBy::Name);
        assert_eq!(by_name[0].name, "a");
    }

    #[test]
    fn the_sort_button_cycles_through_all_three_modes() {
        assert_eq!(SortBy::default().next(), SortBy::Date);
        assert_eq!(SortBy::Date.next(), SortBy::Size);
        assert_eq!(SortBy::Size.next(), SortBy::Name);
    }

    #[test]
    fn a_fresh_panel_starts_from_the_factory_hidden_switch() {
        // `Filer::default()` 是「面板还没绑会话」时的初值，它必须等于出厂偏好
        // （不显示隐藏项）；两边一旦分叉，勾上的开关会在重建后自己弹回去。
        assert!(!Filer::default().show_hidden);
        assert_eq!(
            Filer::default().show_hidden,
            term_runtime::prefs::Prefs::default().filer_show_hidden
        );
    }
}
