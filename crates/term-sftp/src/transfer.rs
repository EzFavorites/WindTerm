//! 下载 / 上传，以及断点续传。
//!
//! 分两半：
//!
//! - **纯函数**：续传偏移、远端路径 → 文件名、本地/远端目标拼装、进度格式化、
//!   上报节流。单测不需要连任何服务端。
//! - **干活的那一半**：在一条已经握好手的 `SftpSession` 上读写文件，进度通过
//!   回调交出去。它不知道事件通道的存在——怎么节流、要不要唤醒 UI 是
//!   `background` 的事。
//!
//! 续传的判据有两条：**已有前缀不能比完整文件长**，而且**前缀内容必须逐字节
//! 一致**。只比长度是不够的——同一长度的半截文件可能来自另一个版本 / 另一台
//! 机器，接着写只会拼出一个看起来完整、内容却是垃圾的文件。校验失败时明确
//! 报错并让用户删除重传；绝不静默覆盖，也不让 `--resume` 变成一句假话。
//! 默认（不带 `--resume`）才是覆盖。

use std::fmt;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, ReadBuf};

use crate::cancel::{CancelToken, Disposition, Stop, disposition};
use crate::entry::format_size;
use crate::preserve;

/// 每块读写的字节数。64 KiB 是「一个 SFTP 包装得下、进度又不至于变成摆设」
/// 的折中：再大要被 `max_write_packet_len` 切开，再小则往返次数白涨。
const CHUNK: usize = 64 * 1024;

/// 进度上报节流：涨了 1 MB **或**过了 200 ms 就报一次，先到先报。
const PROGRESS_BYTES: u64 = 1024 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// 一次传输的失败方式。
///
/// **不把所有失败都摊成 `String`**：调用方必须分得清「用户取消」与「传坏了」——
/// 两者对目标文件的处置正好相反（见 [`crate::cancel::disposition`]），而处置
/// 已经在 `download` / `upload` 里执行过了；这个类型负责把「是哪种停下」带回
/// 上报层，让它能发 `Cancelled` 而不是笼统一句 `Failed`。
#[derive(Debug)]
pub enum TransferError {
    /// 用户按了取消。`pre_existing` 是传输开始前目标是否已存在，供上报层
    /// 说清「分片留在原地」还是「已删除」。
    Cancelled { pre_existing: bool },
    /// 真的失败了（网络、权限、磁盘）。分片一律保留，等续传。
    Failed(String),
}

impl fmt::Display for TransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled { pre_existing } => {
                f.write_str("传输已取消")?;
                if *pre_existing {
                    f.write_str("（目标原已存在，分片保留）")
                } else {
                    f.write_str("（未完成的分片已删除）")
                }
            }
            Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for TransferError {}

/// 让 `map_err(|e| format!(...))?` 这类既有写法继续可用：字符串错误就是
/// `Failed`。反过来不提供——`TransferError` 到 `String` 要显式 `.to_string()`，
/// 免得「取消」在某处被悄悄拍平成一条错误消息。
impl From<String> for TransferError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

/// 传输方向。续传时「哪一边是已有前缀」由它决定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// 远端 → 本地：本地文件是已有前缀，远端大小是完整大小。
    Download,
    /// 本地 → 远端：远端文件是已有前缀，本地大小是完整大小。
    Upload,
}

impl Direction {
    /// 中文动词，给错误消息与 CLI 用。
    pub fn verb(self) -> &'static str {
        match self {
            Direction::Download => "下载",
            Direction::Upload => "上传",
        }
    }
}

/// 已有前缀比完整文件还长——续传无从谈起。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResumeError {
    pub direction: Direction,
    /// 已有那一段的字节数（下载看本地，上传看远端）。
    pub existing: u64,
    /// 完整文件的字节数（下载看远端，上传看本地）。
    pub total: u64,
}

impl fmt::Display for ResumeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (existing_side, total_side, hint) = match self.direction {
            Direction::Download => ("本地", "远端", "本地文件"),
            Direction::Upload => ("远端", "本地", "远端文件"),
        };
        write!(
            f,
            "{existing_side}已有 {} 字节，比{total_side}文件（{} 字节）还大，没法续传；\
             确认后删掉{hint}再试（不加 --resume 会从头覆盖）",
            self.existing,
            self.total,
        )
    }
}

impl std::error::Error for ResumeError {}

/// 从哪个偏移开始传。
///
/// - 不带 `--resume`：一律 `0`，走覆盖语义（下载截断本地、上传截断远端）。
/// - 带 `--resume`：`existing <= total` 时从 `existing` 接着写；
///   `existing > total` 明确失败（`ResumeError`）。
///
/// `existing == total` 落在「从末尾接着写」上：循环立刻读到 EOF，结果是
/// 「已经传完了」——这是事实，不是错误。
pub fn resume_offset(
    direction: Direction,
    existing: u64,
    total: u64,
    resume: bool,
) -> Result<u64, ResumeError> {
    if !resume {
        return Ok(0);
    }
    if existing > total {
        return Err(ResumeError {
            direction,
            existing,
            total,
        });
    }
    Ok(existing)
}

/// 把同步 `Read` 包成 `AsyncRead`，让本地分片与远端 SFTP 文件走同一条
/// 异步比对循环。term-sftp 没开 tokio 的 `fs` feature，本地文件本来就在
/// 页缓存里，同步读不会真的把 runtime 钉住。
struct SyncAsAsync<R>(R);

impl<R: Read + Unpin> AsyncRead for SyncAsAsync<R> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let mut tmp = [0u8; 8 * 1024];
        let want = tmp.len().min(buf.remaining());
        let this = self.get_mut();
        loop {
            match this.0.read(&mut tmp[..want]) {
                Ok(0) => return std::task::Poll::Ready(Ok(())),
                Ok(n) => {
                    buf.put_slice(&tmp[..n]);
                    return std::task::Poll::Ready(Ok(()));
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return std::task::Poll::Ready(Err(e)),
            }
        }
    }
}

/// 读到 `buf` 满或 EOF。一次 `read` 返回得少是常态，不能直接拿它和另一条
/// 流的单次返回值比——那会把「两边分片不同」误判成内容不一致。
async fn read_up_to<R: AsyncRead + Unpin>(reader: &mut R, buf: &mut [u8]) -> Result<usize, String> {
    let mut filled = 0usize;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]).await {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(filled)
}

/// 逐块比对两条流的前 `len` 字节。`Ok(None)` = 完全一致；`Ok(Some(at))` 是
/// 第一个不同的字节偏移；`Err` = 读失败，或文件比声明的 `len` 短。
async fn first_prefix_mismatch<A, B>(
    partial: &mut A,
    source: &mut B,
    len: u64,
) -> Result<Option<u64>, String>
where
    A: AsyncRead + Unpin,
    B: AsyncRead + Unpin,
{
    let mut left = vec![0u8; CHUNK];
    let mut right = vec![0u8; CHUNK];
    let mut done = 0u64;
    while done < len {
        let want = (CHUNK as u64).min(len - done) as usize;
        let a = read_up_to(partial, &mut left[..want]).await?;
        let b = read_up_to(source, &mut right[..want]).await?;
        if a != want || b != want {
            return Err(format!(
                "前缀只读到 {} / {} 字节，比声明的 {len} 字节短（文件在传输期间被改过）",
                a.min(b),
                want
            ));
        }
        if let Some(i) = (0..want).find(|&i| left[i] != right[i]) {
            return Ok(Some(done + i as u64));
        }
        done += want as u64;
    }
    Ok(None)
}

/// 内容对不上时的用户可见文案。把判据写进错误里：用户得知道「长度一样也
/// 不够」，否则他会以为程序只是在挑剔文件大小，然后继续用坏分片重试。
fn prefix_mismatch_message(direction: Direction, offset: u64, detail: String) -> String {
    let (existing, full) = match direction {
        Direction::Download => ("本地", "远端"),
        Direction::Upload => ("远端", "本地"),
    };
    format!(
        "续传校验失败：{existing}已有的 {offset} 字节前缀与{full}文件内容不一致（{detail}）；\
         只比长度就接着写会拼出坏文件。确认后删掉{existing}文件重传，\
         或不加 --resume 从头覆盖"
    )
}

/// 远端路径的最后一截，作为本地文件名：`"/var/log/syslog"` → `"syslog"`。
///
/// 末尾斜杠先去掉（`/var/log/` 也是 `log`）；`/`、`.`、`..`、空串这类没有文件名的
/// 情况给 `None`——拼出 `..` 当文件名会写到意料之外的目录里。
pub fn local_name_for(remote: &str) -> Option<String> {
    let trimmed = remote.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rsplit('/').next() {
        Some(name) if !name.is_empty() && name != "." && name != ".." => Some(name.to_string()),
        _ => None,
    }
}

/// 本地路径的最后一截（同时认 `/` 与 `\`，用户从别处粘来的路径两种都可能有）。
///
/// 结尾带分隔符（`/tmp/`）是「这是个目录」的意思，不是一个文件的名字，给 `None`。
pub fn local_base_name(local: &str) -> Option<String> {
    if local.is_empty() || local.ends_with(['/', '\\']) {
        return None;
    }
    match local.rsplit(['/', '\\']).next() {
        Some(name) if !name.is_empty() && name != "." && name != ".." => Some(name.to_string()),
        _ => None,
    }
}

/// 下载的落地路径：`local` 是已存在的目录时，把远端文件名接上去。
///
/// 目录判定由调用方做（`Path::is_dir()`），这里只管拼——这样这条规则可以单测，
/// 不用造目录。远端路径推不出文件名时返回 `None`，让调用方明确报错，
/// 而不是写到一个叫 `..` 的文件里。
pub fn local_destination(local: &str, remote: &str, local_is_dir: bool) -> Option<String> {
    if !local_is_dir {
        return Some(local.to_string());
    }
    let name = local_name_for(remote)?;
    Some(join_local(local, &name))
}

/// 上传的目标路径：`remote` 以 `/` 结尾（用户表达「这是目录」）时，把本地文件名
/// 接上去。远端目录是否存在由连上之后的 `metadata` 判定，这里不猜。
pub fn remote_destination(remote: &str, local: &str) -> Option<String> {
    if !remote.ends_with('/') {
        return Some(remote.to_string());
    }
    let name = local_base_name(local)?;
    Some(format!("{remote}{name}"))
}

/// 本地路径拼接：只补一个分隔符，`/tmp` 与 `/tmp/` 都得到 `/tmp/x.txt`。
fn join_local(dir: &str, name: &str) -> String {
    if dir.ends_with('/') || dir.ends_with('\\') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// 百分比。`total == 0`（服务端没报大小）返回 `None`：这时百分比没有意义，
/// 显示 0% 会让人以为卡住了，显示 100% 更糟。
pub fn percent(transferred: u64, total: u64) -> Option<u8> {
    if total == 0 {
        return None;
    }
    Some((transferred.min(total) * 100 / total) as u8)
}

/// 一行进度：`" 45%  4.5 MB / 10.0 MB"`；大小未知时只有已传字节。
pub fn format_progress(transferred: u64, total: u64) -> String {
    match percent(transferred, total) {
        Some(p) => format!(
            "{p:>3}%  {} / {}",
            format_size(transferred),
            format_size(total)
        ),
        None => format!("     {}", format_size(transferred)),
    }
}

/// 进度上报的节流器。
///
/// 时间由调用方给（`now`），这样「200 ms 报一次」能单测——否则只能靠 sleep
/// 去赌调度，测试既慢又飘。
pub struct Throttle {
    last_at: Instant,
    last_bytes: u64,
}

impl Throttle {
    pub fn new(at: Instant) -> Self {
        Self {
            last_at: at,
            last_bytes: 0,
        }
    }

    /// 该不该报？该报就把基准挪到这一次（不报则保持原基准，攒着）。
    pub fn should_report(&mut self, transferred: u64, now: Instant) -> bool {
        let enough_bytes = transferred.saturating_sub(self.last_bytes) >= PROGRESS_BYTES;
        let enough_time = now.duration_since(self.last_at) >= PROGRESS_INTERVAL;
        if enough_bytes || enough_time {
            self.last_bytes = transferred;
            self.last_at = now;
            true
        } else {
            false
        }
    }
}

/// 下载一个远端文件到本地。返回最终大小（字节）。
///
/// `progress` 被调用的次数由 [`Throttle`] 控制；开头一定先报一次（续传时是
/// 「从哪儿接上」），结尾再报一次（收尾的 100%）。
///
/// `cancel`：每个分片之前问一次（最坏延迟一个 `CHUNK`）。置位时不当作失败：按
/// [`crate::cancel::disposition`] 收尾（目标原本不存在的半成品删掉，原本存在的
/// 一个字节都不动），返回 [`TransferError::Cancelled`]，由上报层发出
/// `Cancelled` 而不是 `Failed`。
///
/// `preserve`：传完之后要不要把源文件的 mtime 写到目标上。
///
/// - 下载：源是**远端**，`meta`（开头 `metadata(remote)` 的结果）里就有它的 mtime，
///   传完 `set_times` 写回本地；
/// - 上传：源是**本地**，`meta`（开头 `fs::metadata(local)` 的结果）里就有它的 mtime，
///   传完 `set_metadata` 写到远端。
///
/// 服务端没报 mtime（`None`）就什么都不做：写 `0` 等于把时间设成 1970，
/// 比不设更糟。写回失败（只读挂载、远端拒绝 `setstat`）也不让整个传输失败——
/// 文件已经传好了，时间戳是锦上添花。
pub async fn download(
    sftp: &SftpSession,
    remote: &str,
    local: &str,
    resume: bool,
    preserve: bool,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<u64, TransferError> {
    let meta = sftp
        .metadata(remote)
        .await
        .map_err(|e| format!("读不到远端文件信息：{e}"))?;
    if meta.is_dir() {
        return Err(format!("远端路径是个目录：{remote}（get 只传单个文件）").into());
    }
    let total = meta.size.unwrap_or(0);

    let local_path = Path::new(local);
    // `preexisted` 与 `existing` 是两件事：大小为 0 的空文件也算「用户原有的」，
    // 取消时不能删（`cancel::disposition` 的第三行）。
    let (preexisted, existing) = match std::fs::metadata(local_path) {
        Ok(m) if m.is_dir() => {
            return Err(format!("本地路径是个目录：{local}（请给出文件名）").into());
        }
        Ok(m) => (true, m.len()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (false, 0),
        Err(e) => return Err(format!("读不到本地文件 {local}：{e}").into()),
    };

    let offset = resume_offset(Direction::Download, existing, total, resume)
        .map_err(|e| e.to_string())?;

    // 排队期间用户就可能已经点了取消：那会儿一个字节都还没写，也就没什么可删。
    if cancel.is_cancelled() {
        return Err(TransferError::Cancelled {
            pre_existing: preexisted,
        });
    }

    // 长度相同也不代表是同一个文件的同一段前缀。先比对已有前缀的内容，
    // 不一致就明确失败，绝不把两段不同来源的字节拼成一个「成功」的文件。
    if resume && offset > 0 {
        let mut partial = SyncAsAsync(
            std::fs::File::open(local_path)
                .map_err(|e| format!("打开本地分片校验失败：{e}"))?,
        );
        let mut source = sftp
            .open(remote)
            .await
            .map_err(|e| format!("打开远端文件校验失败：{e}"))?;
        match first_prefix_mismatch(&mut partial, &mut source, offset).await {
            Ok(None) => {}
            Ok(Some(at)) => {
                return Err(prefix_mismatch_message(
                    Direction::Download,
                    offset,
                    format!("第 {at} 字节起不同"),
                )
                .into());
            }
            Err(e) => {
                return Err(prefix_mismatch_message(Direction::Download, offset, e).into());
            }
        }
    }

    let mut out = open_local_for_write(local_path, offset)?;
    let mut src = sftp
        .open(remote)
        .await
        .map_err(|e| format!("打开远端文件失败：{e}"))?;
    if offset > 0 {
        src.seek(SeekFrom::Start(offset))
            .await
            .map_err(|e| format!("定位远端文件失败：{e}"))?;
    }

    let mut buf = vec![0u8; CHUNK];
    let mut transferred = offset;
    let mut throttle = Throttle::new(Instant::now());
    progress(transferred, total);
    loop {
        if cancel.is_cancelled() {
            // 先放手再删：Windows 上文件被自己占着是删不掉的。
            drop(out);
            discard_partial(disposition(Stop::Cancelled, preexisted), local_path);
            return Err(TransferError::Cancelled {
                pre_existing: preexisted,
            });
        }
        let n = src
            .read(&mut buf)
            .await
            .map_err(|e| format!("读远端文件失败：{e}"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])
            .map_err(|e| format!("写本地文件失败：{e}"))?;
        transferred += n as u64;
        if throttle.should_report(transferred, Instant::now()) {
            progress(transferred, total);
        }
    }
    out.flush().map_err(|e| format!("写本地文件失败：{e}"))?;
    progress(transferred, total);

    if preserve {
        // mtime 在开头那次 `metadata(remote)` 里已经拿到了，直接写回本地；
        // 传完再查一遍既多一个往返，又可能读到「传输期间被改过」的新值。
        preserve::write_local_mtime(local, meta.mtime);
    }
    Ok(transferred)
}

/// 上传一个本地文件到远端。返回最终大小（字节）。
///
/// `cancel`：语义与 [`download`] 完全一致——处置表按「传输前远端是否存在」判，
/// 删除走 `remove_file`。
///
/// `preserve`：传完之后要不要把源文件的 mtime 写到目标上。
///
/// - 下载：源是**远端**，`meta`（开头 `metadata(remote)` 的结果）里就有它的 mtime，
///   传完 `set_times` 写回本地；
/// - 上传：源是**本地**，`meta`（开头 `fs::metadata(local)` 的结果）里就有它的 mtime，
///   传完 `set_metadata` 写到远端。
///
/// 服务端没报 mtime（`None`）就什么都不做：写 `0` 等于把时间设成 1970，
/// 比不设更糟。写回失败（只读挂载、远端拒绝 `setstat`）也不让整个传输失败——
/// 文件已经传好了，时间戳是锦上添花。
pub async fn upload(
    sftp: &SftpSession,
    local: &str,
    remote: &str,
    resume: bool,
    preserve: bool,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<u64, TransferError> {
    let local_path = Path::new(local);
    let meta =
        std::fs::metadata(local_path).map_err(|e| format!("读不到本地文件 {local}：{e}"))?;
    if meta.is_dir() {
        return Err(format!("本地路径是个目录：{local}（put 只传单个文件）").into());
    }
    let total = meta.len();

    // 远端不存在是正常情况（`existing = 0`）；存在就得知道它多大、是不是目录。
    // `preexisted` 决定取消时删不删远端分片——判据是「传输前有没有」，不是「传了多少」。
    let (preexisted, existing) = if sftp
        .try_exists(remote)
        .await
        .map_err(|e| format!("问不到远端文件信息：{e}"))?
    {
        let m = sftp
            .metadata(remote)
            .await
            .map_err(|e| format!("问不到远端文件信息：{e}"))?;
        if m.is_dir() {
            return Err(format!("远端路径是个目录：{remote}（请给出目标文件名）").into());
        }
        (true, m.size.unwrap_or(0))
    } else {
        (false, 0)
    };

    let offset =
        resume_offset(Direction::Upload, existing, total, resume).map_err(|e| e.to_string())?;

    // 排队期间就点了取消：远端一个字节都还没动过，连文件都不用建。
    if cancel.is_cancelled() {
        return Err(TransferError::Cancelled {
            pre_existing: preexisted,
        });
    }

    // 与下载对称：远端大小对得上也要比内容。否则拿一个同长度的旧版本文件
    // 续传，就会把新内容的尾巴接到旧前缀上，得到一个谁都没要过的文件。
    if resume && offset > 0 {
        let mut partial = sftp
            .open(remote)
            .await
            .map_err(|e| format!("打开远端分片校验失败：{e}"))?;
        let mut source = SyncAsAsync(
            std::fs::File::open(local_path)
                .map_err(|e| format!("打开本地文件校验失败：{e}"))?,
        );
        match first_prefix_mismatch(&mut partial, &mut source, offset).await {
            Ok(None) => {}
            Ok(Some(at)) => {
                return Err(prefix_mismatch_message(
                    Direction::Upload,
                    offset,
                    format!("第 {at} 字节起不同"),
                )
                .into());
            }
            Err(e) => return Err(prefix_mismatch_message(Direction::Upload, offset, e).into()),
        }
    }

    let mut src =
        std::fs::File::open(local_path).map_err(|e| format!("打开本地文件失败：{e}"))?;
    if offset > 0 {
        src.seek(SeekFrom::Start(offset))
            .map_err(|e| format!("定位本地文件失败：{e}"))?;
    }

    // 新建才落权限。覆盖已有文件一律不动：对端那个 `0600` 可能有理由，
    // 把它放宽成 `0644` 是数据事故。`preexisted` 是「传输前就存在」的判据，
    // 不是「传了多少」——这两个问题在这一层第一次能分开答。
    let new_mode = (!preexisted).then_some(
        term_core::config::DEFAULT_FILE_MODE | term_core::config::S_IFREG,
    );

    // 从 0 开始 = 覆盖（create 带 TRUNCATE）；续传只开 WRITE，保住已有前缀。
    let mut dst = if offset == 0 {
        // 带 permissions 的 open 对**已存在**文件通常被服务端忽略，但不同实现
        // 不一致——所以靠 `new_mode` 显式分支，而不是赌「反正会被忽略」。
        let attrs = match new_mode {
            Some(mode) => FileAttributes {
                permissions: Some(mode),
                ..Default::default()
            },
            None => FileAttributes::empty(),
        };
        sftp.open_with_flags_and_attributes(
            remote,
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            attrs,
        )
        .await
        .map_err(|e| format!("创建远端文件失败：{e}"))?
    } else {
        sftp.open_with_flags(remote, OpenFlags::WRITE)
            .await
            .map_err(|e| format!("打开远端文件失败：{e}"))?
    };
    if offset > 0 {
        dst.seek(SeekFrom::Start(offset))
            .await
            .map_err(|e| format!("定位远端文件失败：{e}"))?;
    }

    let mut buf = vec![0u8; CHUNK];
    let mut transferred = offset;
    let mut throttle = Throttle::new(Instant::now());
    progress(transferred, total);
    loop {
        if cancel.is_cancelled() {
            // 先把远端句柄关掉（flush 在途写），再按处置表决定删不删。
            let _ = dst.close().await;
            if disposition(Stop::Cancelled, preexisted) == Disposition::Delete {
                // 删不掉也不改判：真正的结论是「用户取消了」，不是「删除也失败了」。
                let _ = sftp.remove_file(remote).await;
            }
            return Err(TransferError::Cancelled {
                pre_existing: preexisted,
            });
        }
        let n = src
            .read(&mut buf)
            .map_err(|e| format!("读本地文件失败：{e}"))?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])
            .await
            .map_err(|e| format!("写远端文件失败：{e}"))?;
        transferred += n as u64;
        if throttle.should_report(transferred, Instant::now()) {
            progress(transferred, total);
        }
    }
    // flush 把在途写等完；close 再等一次 close 的状态——不等的话最后的丢失是静默的。
    dst.flush()
        .await
        .map_err(|e| format!("写远端文件失败：{e}"))?;
    dst.close()
        .await
        .map_err(|e| format!("关闭远端文件失败：{e}"))?;
    progress(transferred, total);

    // 时间戳照旧：上传方向源是本地，`meta`（开头 `fs::metadata(local)`）里就有
    // 它的 mtime，传完写到远端。服务端没报就什么都不做。
    let mtime = if preserve {
        preserve::to_epoch_secs(
            meta.modified().map_err(|e| format!("读不到本地文件时间：{e}"))?,
        )
    } else {
        None
    };
    // 权限与 mtime 一次写完。权限**只在新建时有值**：`open` 的 mode 会被服务端
    // umask 再削一遍（umask 022 时 0644 保得住，077 时会变成 0600），所以要多这一步
    // `setstat` 才真的「换台服务端也不变」。覆盖路径 `new_mode` 是 `None`，
    // 这次 setstat 只写 mtime、不碰权限。
    preserve::write_remote_attrs(sftp, remote, new_mode, mtime).await;
    Ok(transferred)
}

/// 按处置表收尾本地目标：`Delete` 才动手，`Keep` 连 `remove_file` 都不试。
///
/// 删除失败**不上报**：此刻真正的结论是「用户取消了」，再叠一条「删除失败」
/// 只会盖掉原因、让 UI 同时弹两个错。留下的半成品下次照样能被 `--resume` 接上，
/// 不比「删了一半」更糟。
fn discard_partial(how: Disposition, local: &Path) {
    if how == Disposition::Delete {
        let _ = std::fs::remove_file(local);
    }
}

/// `offset == 0` 截断重写；`offset > 0` 只追加（不 seek：append 模式本身就在末尾）。
fn open_local_for_write(path: &Path, offset: u64) -> Result<std::fs::File, String> {
    let file = if offset == 0 {
        std::fs::File::create(path)
    } else {
        std::fs::OpenOptions::new().append(true).open(path)
    };
    file.map_err(|e| format!("打开本地文件 {} 失败：{e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    /// 处置为 `Delete` 时要真删掉；为 `Keep` 时**一个字节都不动**。
    #[test]
    fn discard_partial_only_touches_the_file_when_it_must() {
        let dir = std::env::temp_dir().join(format!("zterm-discard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("partial.bin");

        std::fs::write(&file, b"half").unwrap();
        discard_partial(Disposition::Delete, &file);
        assert!(!file.exists(), "Delete 必须真的删掉半成品");

        std::fs::write(&file, b"half").unwrap();
        discard_partial(Disposition::Keep, &file);
        assert_eq!(
            std::fs::read(&file).unwrap(),
            b"half",
            "Keep 不许碰用户的既有文件"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 取消与失败的**说法**必须不同：UI 靠这句话决定画红条还是画灰字。
    #[test]
    fn cancel_and_failure_read_differently() {
        let cancel_new = TransferError::Cancelled {
            pre_existing: false,
        }
        .to_string();
        let cancel_old = TransferError::Cancelled { pre_existing: true }.to_string();
        let failed = TransferError::Failed("连不上".to_string()).to_string();

        assert!(cancel_new.contains("已删除"), "新目标取消后分片应已删除：{cancel_new}");
        assert!(cancel_old.contains("保留"), "既有目标取消后应保留：{cancel_old}");
        assert_eq!(failed, "连不上", "失败原样透出原因");
        assert_ne!(cancel_new, failed);
    }

    #[test]
    fn download_resume_starts_where_the_local_partial_ends() {
        // 本地比远端小：接着写。
        assert_eq!(
            resume_offset(Direction::Download, 3 * MB, 10 * MB, true),
            Ok(3 * MB)
        );
        // 一样大：已经传完了，从末尾接着读（立刻 EOF）。
        assert_eq!(
            resume_offset(Direction::Download, 10 * MB, 10 * MB, true),
            Ok(10 * MB)
        );
        // 本地更大：对不上，明确失败。
        assert_eq!(
            resume_offset(Direction::Download, 11 * MB, 10 * MB, true),
            Err(ResumeError {
                direction: Direction::Download,
                existing: 11 * MB,
                total: 10 * MB,
            })
        );
    }

    #[test]
    fn download_without_resume_always_overwrites_even_if_local_is_bigger() {
        assert_eq!(
            resume_offset(Direction::Download, 11 * MB, 10 * MB, false),
            Ok(0)
        );
        assert_eq!(resume_offset(Direction::Download, 0, 0, false), Ok(0));
    }

    #[test]
    fn upload_resume_is_the_mirror_image() {
        assert_eq!(
            resume_offset(Direction::Upload, 3 * MB, 10 * MB, true),
            Ok(3 * MB)
        );
        assert_eq!(
            resume_offset(Direction::Upload, 10 * MB, 10 * MB, true),
            Ok(10 * MB)
        );
        assert_eq!(
            resume_offset(Direction::Upload, 11 * MB, 10 * MB, true),
            Err(ResumeError {
                direction: Direction::Upload,
                existing: 11 * MB,
                total: 10 * MB,
            })
        );
        assert_eq!(
            resume_offset(Direction::Upload, 11 * MB, 10 * MB, false),
            Ok(0)
        );
    }

    #[test]
    fn mismatch_messages_say_which_side_is_ahead() {
        let down = ResumeError {
            direction: Direction::Download,
            existing: 2 * MB,
            total: MB,
        }
        .to_string();
        assert!(down.contains("本地"), "{down}");
        assert!(down.contains("远端文件"), "{down}");

        let up = ResumeError {
            direction: Direction::Upload,
            existing: 2 * MB,
            total: MB,
        }
        .to_string();
        assert!(up.contains("远端"), "{up}");
        assert!(up.contains("本地文件"), "{up}");
    }

    #[test]
    fn remote_paths_give_their_last_component() {
        assert_eq!(local_name_for("/var/log/syslog").as_deref(), Some("syslog"));
        assert_eq!(local_name_for("/var/log/").as_deref(), Some("log"));
        assert_eq!(local_name_for("report.tar.gz").as_deref(), Some("report.tar.gz"));
        // 没有文件名的情况：拼出来会写到意料之外的地方。
        assert_eq!(local_name_for("/"), None);
        assert_eq!(local_name_for(""), None);
        assert_eq!(local_name_for("/a/.."), None);
        assert_eq!(local_name_for("/a/."), None);
    }

    #[test]
    fn local_base_name_understands_both_separators() {
        assert_eq!(local_base_name("/tmp/a.txt").as_deref(), Some("a.txt"));
        assert_eq!(local_base_name("C:\\work\\a.txt").as_deref(), Some("a.txt"));
        assert_eq!(local_base_name("/tmp/"), None);
    }

    #[test]
    fn downloading_into_a_directory_takes_the_remote_file_name() {
        assert_eq!(
            local_destination("/tmp/", "/var/log/syslog", true).as_deref(),
            Some("/tmp/syslog")
        );
        // 目录名不带结尾斜杠也拼得对。
        assert_eq!(
            local_destination("/tmp", "/var/log/syslog", true).as_deref(),
            Some("/tmp/syslog")
        );
        // 不是目录就原样使用（显式给了文件名就别改）。
        assert_eq!(
            local_destination("/tmp/other.log", "/var/log/syslog", false).as_deref(),
            Some("/tmp/other.log")
        );
        // 远端推不出文件名：明确失败，不猜。
        assert_eq!(local_destination("/tmp/", "/", true), None);
    }

    #[test]
    fn uploading_to_a_trailing_slash_target_keeps_the_local_name() {
        assert_eq!(
            remote_destination("/srv/", "/tmp/a.txt").as_deref(),
            Some("/srv/a.txt")
        );
        assert_eq!(
            remote_destination("/srv/", "C:\\work\\a.txt").as_deref(),
            Some("/srv/a.txt")
        );
        // 不带结尾斜杠 = 已经给了目标文件名。
        assert_eq!(
            remote_destination("/srv/b.txt", "/tmp/a.txt").as_deref(),
            Some("/srv/b.txt")
        );
        assert_eq!(remote_destination("/srv/", "/tmp/"), None);
    }

    #[test]
    fn percent_rounds_down_and_gives_up_on_unknown_totals() {
        assert_eq!(percent(0, 10), Some(0));
        assert_eq!(percent(5, 10), Some(50));
        assert_eq!(percent(10, 10), Some(100));
        // 远端长得比报的还快：夹到 100，不显示 120%。
        assert_eq!(percent(12, 10), Some(100));
        // total 未知：百分比没有意义。
        assert_eq!(percent(0, 0), None);
        assert_eq!(percent(4096, 0), None);
    }

    #[test]
    fn progress_lines_align_the_percentage_column() {
        assert_eq!(format_progress(5 * MB, 10 * MB), " 50%  5.0 MB / 10.0 MB");
        assert_eq!(format_progress(10 * MB, 10 * MB), "100%  10.0 MB / 10.0 MB");
        assert_eq!(format_progress(512, 1024), " 50%  512 B / 1.0 KB");
        assert_eq!(format_progress(4096, 0), "     4.0 KB");
    }

    #[test]
    fn throttle_reports_on_bytes_or_time_whichever_comes_first() {
        let t0 = Instant::now();
        let mut th = Throttle::new(t0);

        // 不到 1 MB、不到 200 ms：忍住。
        assert!(!th.should_report(64 * 1024, t0 + Duration::from_millis(50)));
        // 时间到了：报，并把基准挪过来。
        assert!(th.should_report(128 * 1024, t0 + Duration::from_millis(200)));
        // 刚报过，什么都没变：不重复报。
        assert!(!th.should_report(128 * 1024, t0 + Duration::from_millis(210)));
        // 字节够 1 MB：立刻报，不用等时间。
        assert!(th.should_report(128 * 1024 + MB, t0 + Duration::from_millis(220)));
        // 报过之后基准是新值：再涨 1 MB 才报。
        assert!(!th.should_report(128 * 1024 + MB, t0 + Duration::from_millis(230)));
        assert!(th.should_report(128 * 1024 + 2 * MB, t0 + Duration::from_millis(240)));
    }

    /// 长度相等但内容不同：旧实现会直接续传，新实现必须指出第一个不同的字节。
    #[tokio::test]
    async fn resume_checks_prefix_content_not_just_its_length() {
        let partial = &b"hello"[..];
        let source = &b"hellX"[..];
        assert_eq!(
            first_prefix_mismatch(&mut &partial[..], &mut &source[..], 5).await,
            Ok(Some(4)),
            "同样的 5 字节前缀，内容不同也要拒绝"
        );
        assert_eq!(
            first_prefix_mismatch(&mut &b"hello"[..], &mut &b"hello world"[..], 5).await,
            Ok(None),
            "前缀一致、源文件更长才是正常续传"
        );
        // 跨 CHUNK 边界也要逐块比，不能只看第一块。
        let long = vec![b'x'; CHUNK + 32];
        let mut changed = long.clone();
        changed[CHUNK + 7] = b'y';
        assert_eq!(
            first_prefix_mismatch(&mut &long[..], &mut &changed[..], (CHUNK + 32) as u64).await,
            Ok(Some((CHUNK + 7) as u64))
        );
    }

    #[tokio::test]
    async fn a_prefix_shorter_than_its_declared_length_is_an_error() {
        let err = first_prefix_mismatch(&mut &b"ab"[..], &mut &b"abcde"[..], 4)
            .await
            .expect_err("文件被改短了不该被当成一致");
        assert!(err.contains("比声明的"), "{err}");
    }

    #[test]
    fn prefix_mismatch_error_names_both_sides_and_the_length_only_trap() {
        let down = prefix_mismatch_message(Direction::Download, 10, "第 3 字节起不同".into());
        assert!(down.contains("本地") && down.contains("远端"), "{down}");
        assert!(down.contains("只比长度"), "得把判据写进用户可见语义：{down}");
        assert!(down.contains("--resume"), "{down}");

        let up = prefix_mismatch_message(Direction::Upload, 10, "第 3 字节起不同".into());
        assert!(up.contains("远端") && up.contains("本地"), "{up}");
        assert!(up.contains("只比长度"), "{up}");
    }
}
