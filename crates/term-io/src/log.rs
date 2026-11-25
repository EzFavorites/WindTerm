//! 会话日志：把一条会话的输出写成一个能 `grep` 的文本文件。
//!
//! 四个决定写在最前面，因为它们决定了这个模块的形状：
//!
//! 1. **默认写清洗后的文本，不是原始字节**（`Log types = Log printable output`）。
//!    原始字节能完整回放（含颜色、光标），但日志的用途是事后翻一遍，不是放录像；
//!    `term_core::ansi::TextCleaner` 只留可打印字符与换行。选 `Log all session
//!    output` 时改走原始字节——那是用户明确要回放能力。
//! 2. **写入端在监管者线程里，不在 UI 线程。** `ch.drain()` 拿到的字节只有那一处有，
//!    而且界面卡一帧不该丢一屏输出。
//! 3. **暂停 = 不写，但字节照样从屏幕上过。** 恢复时在文件里插一行标记，说明停了多久、
//!    期间丢了多少字节——否则日志看起来像「这段时间什么都没发生」，那是最坏的误导。
//! 4. **文件名 / 每行格式 / 覆盖追加 / 大小上限都是用户可配的**（配置面在
//!    `term_core::log_opts`，界面在 `Session Settings ▸ Logging`）。这个模块只负责
//!    按 `LogOptions` 落盘，不自己发明规则。
//!
//! 这里是 `term-io`（2026-11-12 从 term-runtime 拆出），可以碰文件与时钟；
//! 文本清洗在 `term-core::ansi`（纯语义），模板展开在 `term-core::log_opts`（纯语义）。

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use term_core::ansi::TextCleaner;
use term_core::log_opts::{self, LogOptions, LogType};

/// 两次自动 flush 的最小间隔。
///
/// `BufWriter` 默认攒满 8 KiB 才落盘，低频会话（敲命令、看输出）可能几分钟都到不了
/// 8 KiB，`tail -f` 就一直是空的。按时间刷一次，代价是每秒最多一次 write 系统调用。
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// 已知应答 / 密码登记表：写盘前统一脱敏。
///
/// 这些值是 UI 在提交密码框、OneKey 应答时登记的，只活在本进程内存里。
/// 日志会跟着会话备份、贴到工单里——明文应答一旦落盘就是一次已发生的泄漏。
static KNOWN_SECRETS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn register_secret(secret: &str) {
    if !secret.is_empty() {
        if let Ok(mut known) = KNOWN_SECRETS.lock() {
            if !known.iter().any(|s| s == secret) { known.push(secret.into()); }
        }
    }
}

fn redact(bytes: &[u8]) -> Vec<u8> {
    let known = KNOWN_SECRETS.lock().map(|k| k.clone()).unwrap_or_default();
    if known.is_empty() { return bytes.to_vec(); }
    let mut out = String::from_utf8_lossy(bytes).into_owned();
    for secret in known { out = out.replace(&secret, "[redacted]"); }
    out.into_bytes()
}

/// 日志目录：profile 目录下的 `logs/`。
///
/// 与 `sessions.db` / `vault.ztv` 同一个地方的原因很实际：用户要备份或整体搬走配置时，
/// 只有一个目录要处理。`WINDTERM_PROFILE_DIR` 因此也能把日志重定向到临时目录（测试用）。
pub fn default_dir() -> Option<PathBuf> {
    Some(term_paths::profile_dir()?.join("logs"))
}

/// 日志状态。**只描述「现在是什么状态」，不带字节数**——
/// 文件多大是磁盘上的事实，界面要看就直接 `metadata().len()`，
/// 在这里维护一份计数迟早会与文件不一致。
///
/// 2026-11-08 类型本体搬到 `term_core::session_runtime`（控制面要给 UI 报
/// `Session::log_status`，而 `term-core` 不能反过来依赖本 crate）。这里保留
/// 同一类型的**再导出**，`use term_runtime::log::{LogStatus, LogPhase}` 继续可用。
pub use term_core::session_runtime::{LogPhase, LogStatus};

/// 一条会话的日志写入端。
pub struct SessionLog {
    file: BufWriter<File>,
    path: PathBuf,
    dir: PathBuf,
    /// 会话名（`LogStatus` 不报它，但内容模板的 `%n` 与轮转时的重开都要它）。
    session: String,
    /// 宿主名（`%{host}` 用；空串 = 调用方不知道，模板里展开成空）。
    host: String,
    /// 用户给的那份配置。`create` 里已经 `normalized()` 过，这里不会为空。
    opts: LogOptions,
    /// `Printable` / `All`。由 `opts.mode()` 定，决定 `write` 走哪条路。
    mode: LogType,
    /// 建文件时已知的终端尺寸：轮转要重新写头部，那时 `vt` 的尺寸可能已经变了。
    cols: usize,
    rows: usize,
    cleaner: TextCleaner,
    paused: bool,
    /// 暂停期间被丢掉的原始字节数（恢复时写进标记行）。
    paused_bytes: u64,
    /// 本文件已写字节数（含头部）。`max_size_mb` 的判据，`0` 时恒不轮转。
    written: u64,
    /// 下一段文本是不是落在行首。内容模板按行加前缀，靠它决定要不要套。
    at_line_start: bool,
    /// 写失败后就地停手并记住原因：磁盘满了还继续每 2 ms 试一次只是把 CPU 烧掉。
    error: Option<String>,
    last_flush: Instant,
}

impl SessionLog {
    /// 新建一个日志文件并写入头部。
    ///
    /// 文件名由 `LogOptions::file_name_for` 展开（会话名 + 时间抽模板）；
    /// 同名时怎么办由 `overwrite` 决定：`Some(true)` 覆盖、`Some(false)` 追加、
    /// `None`（视觉基线出厂态）加 `-2`…`-99` 序号。**默认绝不是覆盖**——覆盖等于
    /// 把上一段日志悄悄删掉。
    pub fn create(
        dir: &Path,
        session: &str,
        cols: usize,
        rows: usize,
        opts: &LogOptions,
    ) -> std::io::Result<Self> {
        Self::create_for(dir, session, "", cols, rows, opts)
    }

    /// 带宿主名的一档（ADR-0027）：`%{host}` 只在真的知道主机时才有值。
    pub fn create_for(
        dir: &Path,
        session: &str,
        host: &str,
        cols: usize,
        rows: usize,
        opts: &LogOptions,
    ) -> std::io::Result<Self> {
        let opts = opts.clone().normalized();
        let mode = opts.mode();
        fs::create_dir_all(dir)?;
        let host_opt = (!host.is_empty()).then_some(host);
        let name = opts.file_name_env(session, host_opt, now_stamp(), &|n| std::env::var(n).ok());
        let (file, path) = open_target(dir, &name, opts.overwrite)?;
        let mut log = Self {
            file: BufWriter::new(file),
            path,
            dir: dir.to_path_buf(),
            session: session.to_string(),
            host: host.to_string(),
            opts,
            mode,
            cols,
            rows,
            cleaner: TextCleaner::new(),
            paused: false,
            paused_bytes: 0,
            written: 0,
            at_line_start: true,
            error: None,
            last_flush: Instant::now(),
        };
        log.header(None);
        log.flush();
        Ok(log)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 头部注释块。`rotated_from` = 轮转出来的新文件，记一笔上一份是谁，
    /// 否则读者会把「一小时内出现三个文件」当成三次独立会话。
    fn header(&mut self, rotated_from: Option<&Path>) {
        let mut h = format!(
            "# WindTerm session log\n\
             # session: {}\n\
             # started: {}\n\
             # size: {}x{}\n",
            self.session,
            log_opts::iso(now_secs()),
            self.cols,
            self.rows
        );
        match self.mode {
            LogType::All => h.push_str("# note: raw bytes; escape sequences kept\n"),
            _ => {
                h.push_str("# note: escape sequences stripped; CR written as LF\n");
                h.push_str(&format!("# content: {}\n", self.opts.content_pattern));
            }
        }
        if let Some(prev) = rotated_from {
            h.push_str(&format!("# rotated from {}\n", prev.display()));
        }
        h.push_str("# ----- output follows -----\n");
        self.raw_write(h.as_bytes());
    }

    /// 收到一段输出。暂停期间只记账，不落盘。
    pub fn write(&mut self, bytes: &[u8]) {
        if bytes.is_empty() || self.error.is_some() {
            return;
        }
        if self.paused {
            self.paused_bytes += bytes.len() as u64;
            return;
        }
        match self.mode {
            LogType::All => self.emit_raw(bytes),
            _ => {
                let text = self.cleaner.feed(bytes);
                if text.is_empty() {
                    return;
                }
                self.emit_text(&text);
            }
        }
        if self.last_flush.elapsed() >= FLUSH_INTERVAL {
            self.flush();
        }
    }

    /// 原始字节模式：一个字节都不动。内容模板在这种情况下**不生效**——
    /// 逐行加前缀就得先按 `\n` 切分，而原始字节里的 `\r`、转义序列与
    /// 跨 chunk 的半个字符会让切分本身失真，回放也就毁了。
    fn emit_raw(&mut self, bytes: &[u8]) {
        self.rotate_if_needed(bytes.len());
        self.raw_write(bytes);
    }

    /// 清洗后的文本：按行套内容模板。
    fn emit_text(&mut self, text: &str) {
        if self.opts.content_is_plain() {
            self.rotate_if_needed(text.len());
            self.raw_write(text.as_bytes());
            return;
        }
        let stamp = now_stamp();
        let host = (!self.host.is_empty()).then_some(self.host.as_str());
        let mut out = String::with_capacity(text.len() + 32);
        for seg in text.split_inclusive('\n') {
            let nl = seg.ends_with('\n');
            let body = if nl { &seg[..seg.len() - 1] } else { seg };
            // 空行不套前缀：给一行空白打时间戳只是噪音，而读者需要的
            // 「这一行什么时候出来的」在非空行上已经有了。
            if self.at_line_start && !body.trim().is_empty() {
                out.push_str(&self.opts.line_env(&self.session, host, body, stamp, &|n| std::env::var(n).ok()));
            } else {
                out.push_str(body);
            }
            if nl {
                out.push('\n');
            }
            self.at_line_start = nl;
        }
        self.rotate_if_needed(out.len());
        self.raw_write(out.as_bytes());
    }

    /// 超过 `max_size_mb` 就换一个文件。`0` = ∞，恒不轮转。
    fn rotate_if_needed(&mut self, incoming: usize) {
        let Some(cap) = self.opts.max_bytes() else {
            return;
        };
        // 空的文件不轮转：否则 `max_size_mb` 设得比一屏还小时会造出一串空文件。
        if self.written == 0 || self.written + incoming as u64 <= cap {
            return;
        }
        self.rotate();
    }

    fn rotate(&mut self) {
        let prev = self.path.clone();
        if self.error.is_none() {
            let line = format!("# rotated at {}\n", log_opts::iso(now_secs()));
            self.raw_write(line.as_bytes());
        }
        self.flush();
        // 轮转**永远走新文件**，即使 `overwrite = 覆盖`：同一秒里同名会被
        // 截断，那等于把刚记满的那一段删掉——轮转的意义就反了。
        let host = (!self.host.is_empty()).then_some(self.host.as_str());
        let name = self.opts.file_name_env(&self.session, host, now_stamp(), &|n| std::env::var(n).ok());
        match open_unique(&self.dir, &name) {
            Ok((file, path)) => {
                self.file = BufWriter::new(file);
                self.path = path;
                self.written = 0;
                self.at_line_start = true;
                // 清洗器跨 chunk 的解析状态跟着旧文件走：新文件的第 0 字节
                // 就是新的一行，把上一份的半截 `\x1b[` 带过去只会串味。
                self.cleaner = TextCleaner::new();
                self.header(Some(&prev));
            }
            Err(e) => self.error = Some(format!("轮转日志失败：{e}")),
        }
    }

    pub fn pause(&mut self) {
        if self.paused || self.error.is_some() {
            return;
        }
        self.paused = true;
        let line = format!("\n# paused at {}\n", log_opts::iso(now_secs()));
        self.raw_write(line.as_bytes());
        self.flush();
    }

    pub fn resume(&mut self) {
        if !self.paused {
            return;
        }
        self.paused = false;
        let dropped = std::mem::take(&mut self.paused_bytes);
        let line = format!(
            "# resumed at {} ({dropped} bytes were not recorded)\n",
            log_opts::iso(now_secs())
        );
        self.raw_write(line.as_bytes());
        self.flush();
    }

    /// 收尾：写一行结束标记并落盘。重复调用无害。
    pub fn finish(&mut self) {
        if self.error.is_none() {
            let line = format!("# stopped at {}\n", log_opts::iso(now_secs()));
            self.raw_write(line.as_bytes());
        }
        self.flush();
    }

    /// 当前状态，交给监管者线程发给界面。
    pub fn status(&self) -> LogStatus {
        let phase = match (&self.error, self.paused) {
            (Some(e), _) => LogPhase::Failed(e.clone()),
            (None, paused) if paused => LogPhase::Paused,
            (None, _) => LogPhase::Recording,
        };
        LogStatus {
            phase,
            path: Some(self.path.clone()),
        }
    }

    fn raw_write(&mut self, bytes: &[u8]) {
        if self.error.is_some() {
            return;
        }
        // 唯一的落盘漏斗：清洗文本与原始字节两条路都在这里脱敏。
        let bytes = redact(bytes);
        if let Err(e) = self.file.write_all(&bytes) {
            self.error = Some(format!("写入日志失败：{e}"));
            return;
        }
        // 只在真的写进去之后记账：写失败还往上加，`max_size_mb` 会把一个
        // 已经坏掉的写入端推去轮转，产出一串同样打不开的文件。
        self.written += bytes.len() as u64;
    }

    fn flush(&mut self) {
        if self.error.is_none() {
            if let Err(e) = self.file.flush() { self.error = Some(format!("刷新日志失败：{e}")); }
        }
        self.last_flush = Instant::now();
    }
}

/// 打开目标文件。`overwrite` 的三种取值对应视觉基线那两行单选 + 出厂态。
fn open_target(dir: &Path, name: &str, overwrite: Option<bool>) -> std::io::Result<(File, PathBuf)> {
    let path = dir.join(name);
    if let Some(overwrite) = overwrite {
        let mut oo = OpenOptions::new();
        oo.write(true).create(true);
        if overwrite {
            // 覆盖：用户明确要同一份文件被重写（脚本里常见）。
            oo.truncate(true);
        } else {
            // 追加：多段会话接进同一份文件。时间戳模板此时通常只区分「哪一段」。
            oo.append(true);
        }
        // 打开失败（同名目录、权限）就落到下面的避让：宁可多一个文件，
        // 也不丢日志。
        if let Ok(f) = oo.open(&path) {
            return Ok((f, path));
        }
    }
    open_unique(dir, name)
}

/// 建文件，冲突就试 `-2`…`-99`（插在扩展名前：`x.log` → `x-2.log`）。
fn open_unique(dir: &Path, name: &str) -> std::io::Result<(File, PathBuf)> {
    let mut last_err = None;
    for n in 1..=99 {
        let path = dir.join(with_suffix(name, n));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => return Ok((f, path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last_err = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::other("日志文件重名次数过多")))
}

fn with_suffix(name: &str, n: usize) -> String {
    if n == 1 {
        return name.to_string();
    }
    match name.rfind('.') { Some(i) if i > 0 => format!("{}-{n}{}", &name[..i], &name[i..]), _ => format!("{name}-{n}") }
}

fn now_stamp() -> term_core::log_opts::LogStamp {
    let (secs, millis) = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map(|d| (d.as_secs(), d.subsec_millis()))
        .unwrap_or((0, 0));
    term_core::log_opts::LogStamp { secs, millis }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(1);

    #[test]
    fn a_known_secret_never_reaches_the_log_file() {
        register_secret("s3cret-for-log-test");
        let bytes = redact(b"password: s3cret-for-log-test\n");
        assert_eq!(String::from_utf8_lossy(&bytes), "password: [redacted]\n");
    }

    /// 每个用例一个独立目录，跑完自己删。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("zterm-log-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).expect("建临时目录");
            Self(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).expect("读日志")
    }

    /// 出厂选项 + 显式内容模板 `%v`：这一层的判据大多关心「写了什么」，
    /// 不关心时间戳前缀。前缀本身在 `term-core::log_opts` 有独立判据。
    fn plain() -> LogOptions {
        LogOptions {
            content_pattern: "%v".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn create_writes_a_header_with_the_session_name() {
        let dir = TempDir::new();
        let log = SessionLog::create(dir.path(), "prod-db", 120, 36, &LogOptions::default())
            .expect("建日志");
        let path = log.path().to_path_buf();
        assert!(path.starts_with(dir.path()));
        // 文件名按视觉基线的模板展开：`%n_%Y-%m-%d_%H-%M-%S.log`。
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("prod-db_"),
            "{}",
            path.display()
        );
        let text = read(&path);
        assert!(text.contains("# session: prod-db"), "{text}");
        assert!(text.contains("# size: 120x36"), "{text}");
        assert!(text.starts_with("# WindTerm session log\n"), "{text}");
        // 头部要把「这份日志是按哪个内容模板写的」交代清楚，否则时间戳
        // 前缀在别人眼里就是一串没来由的方括号。
        assert!(text.contains("# content: [%Y-%m-%d %H:%M:%S] %v"), "{text}");
    }

    #[test]
    fn written_output_has_no_escape_sequences() {
        let dir = TempDir::new();
        let mut log = SessionLog::create(dir.path(), "s", 80, 24, &plain()).expect("建日志");
        let path = log.path().to_path_buf();
        log.write(b"\x1b[1;32muser@host\x1b[0m$ echo hi\r\nhi\r\n");
        log.finish();
        let text = read(&path);
        assert!(text.contains("user@host$ echo hi\nhi\n"), "{text}");
        assert!(!text.contains('\x1b'), "日志里不该有转义字符：{text:?}");
    }

    #[test]
    fn the_default_content_pattern_stamps_every_line_but_not_blank_ones() {
        let dir = TempDir::new();
        let mut log = SessionLog::create(dir.path(), "web", 80, 24, &LogOptions::default())
            .expect("建日志");
        let path = log.path().to_path_buf();
        log.write(b"first\r\n\r\nsecond\r\n");
        log.finish();
        let body: String = read(&path)
            .lines()
            .filter(|l| !l.starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        // 两行非空各带一个前缀；中间的空行保持空（给空白打时间戳只是噪音）。
        assert_eq!(body.matches("[20").count(), 2, "{body}");
        assert!(body.contains("] first\n\n"), "{body}");
        assert!(body.contains("] second"), "{body}");
    }

    #[test]
    fn raw_mode_keeps_the_escape_sequences_verbatim() {
        // `Log all session output` = 用户明确要回放能力，那就一个字节都别动。
        let dir = TempDir::new();
        let opts = LogOptions {
            log_type: LogType::All,
            ..Default::default()
        };
        let mut log = SessionLog::create(dir.path(), "s", 80, 24, &opts).expect("建日志");
        let path = log.path().to_path_buf();
        log.write(b"\x1b[1;32mhi\x1b[0m\r\n");
        log.finish();
        let bytes = fs::read(&path).expect("读日志");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("\x1b[1;32mhi\x1b[0m\r\n"), "{text:?}");
        assert!(text.contains("# note: raw bytes"), "{text}");
    }

    #[test]
    fn overwrite_rewrites_the_same_file_and_append_keeps_both_runs() {
        let dir = TempDir::new();
        // 固定模板：不带时间戳，让两次 create 落在同一个名字上。
        let fixed = |overwrite| LogOptions {
            overwrite,
            file_name: "fixed.log".to_string(),
            content_pattern: "%v".to_string(),
            ..Default::default()
        };

        let mut a = SessionLog::create(dir.path(), "s", 80, 24, &fixed(Some(true))).expect("覆盖一");
        a.write(b"first run\r\n");
        a.finish();
        let mut b = SessionLog::create(dir.path(), "s", 80, 24, &fixed(Some(true))).expect("覆盖二");
        b.write(b"second run\r\n");
        b.finish();
        assert_eq!(a.path(), b.path(), "覆盖：同一个文件");
        let text = read(a.path());
        assert!(!text.contains("first run"), "覆盖之后不该还留着上一段：{text}");
        assert!(text.contains("second run"), "{text}");

        let dir2 = TempDir::new();
        let mut c = SessionLog::create(dir2.path(), "s", 80, 24, &fixed(Some(false))).expect("追加一");
        c.write(b"first run\r\n");
        c.finish();
        let mut d = SessionLog::create(dir2.path(), "s", 80, 24, &fixed(Some(false))).expect("追加二");
        d.write(b"second run\r\n");
        d.finish();
        assert_eq!(c.path(), d.path(), "追加：同一个文件");
        let text = read(c.path());
        assert!(text.contains("first run"), "{text}");
        assert!(text.contains("second run"), "{text}");
        assert_eq!(text.matches("# WindTerm session log").count(), 2, "两段各有头部");
    }

    #[test]
    fn hitting_the_size_cap_rotates_instead_of_dropping_output() {
        let dir = TempDir::new();
        // 1 MB 的下限没法用小字数触发，所以这里直接问写入端：把阈值设成 1 MB
        // 而喂超过它一次的量。轮转后必须仍有完整内容（不丢字节）且是新文件。
        let opts = LogOptions {
            file_name: "capped.log".to_string(),
            max_size_mb: 1,
            content_pattern: "%v".to_string(),
            ..Default::default()
        };
        let mut log = SessionLog::create(dir.path(), "s", 80, 24, &opts).expect("建日志");
        let first = log.path().to_path_buf();
        let chunk = vec![b'x'; 700 * 1024];
        log.write(&chunk);
        log.write(b"\n");
        log.write(&chunk);
        log.write(b"tail\r\n");
        log.finish();
        assert_ne!(log.path(), first, "超过上限必须换文件");
        assert!(log.path().to_string_lossy().contains("-2.log"), "{}", log.path().display());
        let rotated = read(log.path());
        assert!(rotated.contains("# rotated from"), "{rotated}");
        assert!(rotated.contains("tail"), "换文件之后的内容不能丢：{}", rotated.len());
        // 两份加起来仍是全部字节（只有标记行是新增）。
        let total = read(&first).len() + rotated.len();
        assert!(total >= 700 * 1024 * 2, "总字节不该缩水：{total}");
    }

    #[test]
    fn pausing_stops_writing_and_resuming_leaves_a_marker() {
        let dir = TempDir::new();
        let mut log = SessionLog::create(dir.path(), "s", 80, 24, &plain()).expect("建日志");
        let path = log.path().to_path_buf();

        log.write(b"before\r\n");
        log.pause();
        log.write(b"secret during pause\r\n");
        assert_eq!(log.status().phase, LogPhase::Paused);
        log.resume();
        log.write(b"after\r\n");
        log.finish();

        let text = read(&path);
        assert!(text.contains("before\n"));
        assert!(text.contains("after\n"));
        assert!(!text.contains("secret"), "暂停期间的输出不该落盘：{text}");
        assert!(text.contains("# paused at "), "{text}");
        assert!(text.contains("bytes were not recorded"), "{text}");
    }

    #[test]
    fn two_logs_in_the_same_second_do_not_overwrite_each_other() {
        let dir = TempDir::new();
        let opts = plain();
        let a = SessionLog::create(dir.path(), "same", 80, 24, &opts).expect("第一条");
        let b = SessionLog::create(dir.path(), "same", 80, 24, &opts).expect("第二条");
        assert_ne!(a.path(), b.path(), "同名同秒必须落到不同文件");
        // 序号插在扩展名前，`ls` 里还能按扩展名筛。
        assert!(b.path().to_string_lossy().contains("-2.log"));
    }

    #[test]
    fn status_reports_the_path_and_phase() {
        let dir = TempDir::new();
        let log = SessionLog::create(dir.path(), "s", 80, 24, &plain()).expect("建日志");
        let st = log.status();
        assert_eq!(st.phase, LogPhase::Recording);
        assert_eq!(st.path.as_deref(), Some(log.path()));
        assert!(st.is_recording());
        assert_eq!(LogStatus::default().phase, LogPhase::Off);
        assert!(!LogStatus::default().is_recording());
    }
}
