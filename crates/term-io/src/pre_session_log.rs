//! 建连前错误的落盘：连接 / 认证 / SFTP / 隧道失败都发生在会话建立之前，
//! 那时没有 supervisor、没有 `Tab`、也没有会话日志槽，错误原来只弹窗不落盘。
//! 这里把它们写进与 `Mode ▸ Debug Mode` 排障日志**同源**的文件：同一个
//! `default_dir()`、同一套 `term_core::debug_log::options()`、同一条
//! `log_redact` 脱敏漏斗（见 `SessionLog::raw_write`）。
//!
//! 三条边界：
//! 1. 只有既有的 Debug 开关（`tab.debug`，由调用方传进来）开着才写——不新增第二个开关；
//! 2. 目录建不出来 / 文件打不开一律静默返回：写日志失败绝不挡弹窗、绝不 panic；
//! 3. 只写错误文本，密码 / 密钥 / 令牌这类已登记凭据经 `redact` 换成 `[redacted]`。

use std::path::Path;

use crate::log::{self, SessionLog};

/// 失败发生在哪一步。写进 note 的前缀，排障时一眼看出是哪一环。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorStage {
    /// SSH 连接本身（握手、ProxyJump 参数拼装）。
    Connect,
    /// 认证（含凭据解析与密钥认证失败）。
    Auth,
    /// SFTP 子系统连接。
    Sftp,
    /// 端口转发隧道连接。
    Tunnel,
}

impl ErrorStage {
    /// note 里用的短标签，固定小写便于 `grep '\[sftp\]'`。
    pub fn label(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Auth => "auth",
            Self::Sftp => "sftp",
            Self::Tunnel => "tunnel",
        }
    }
}

/// 纯函数：阶段 + 错误 → 一条已脱敏的 note 文本。
///
/// 不带时间戳、不碰文件 IO——时间由 `SessionLog::note` 按同一口径加。
/// 单测打这里：四个阶段的前缀与凭据清洗都不需要磁盘。
pub fn format_note(stage: ErrorStage, err: &str) -> String {
    let raw = format!("[{}] {}", stage.label(), err);
    String::from_utf8_lossy(&log::log_redact::redact(raw.as_bytes())).into_owned()
}

/// 建连前错误留痕的统一入口：在弹窗**之前**调用。
///
/// `enabled` 就是既有的 Debug 日志开关（窗口内任一 `tab.debug`）当前的值，
/// 由调用方传来；helper 自己判断关着就不写，不另设配置或环境变量。
/// 开关开、目录与文件都可用时，写一条 `# <时间> [stage] <错误>` 到
/// `default_dir()` 下的 `debug_pre-session_<时间>.log`——与事后排障日志同源。
///
/// 任何 IO 失败都静默降级：弹窗照出，用户可见行为一个字都不变。
pub fn log_pre_session_error(stage: ErrorStage, err: &str, enabled: bool) {
    if !enabled {
        return;
    }
    let Some(dir) = log::default_dir() else {
        return;
    };
    write_note(&dir, stage, err);
}

/// 文件 I/O 的最小单元：拆出来让测试能传一个不可用的路径进来。
fn write_note(dir: &Path, stage: ErrorStage, err: &str) {
    // 与 `Debug Mode` 那份日志完全同一套选项：同目录、同 `debug_` 命名前缀、
    // 同轮转上限、同脱敏漏斗。不复制一份配置——复制早晚会漂移。
    let opts = term_core::debug_log::options();
    let Ok(mut log_file) = SessionLog::create(dir, "pre-session", 0, 0, &opts) else {
        return;
    };
    log_file.note(&format_note(stage, err));
    log_file.finish();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(1);
    /// 环境变量是全进程共享的；设 → 读 → 还原必须串起来。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("zterm-pre-session-{}-{n}", std::process::id()));
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

    fn with_profile_dir<F: FnOnce()>(dir: &Path, check: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var("WINDTERM_PROFILE_DIR").ok();
        std::env::set_var("WINDTERM_PROFILE_DIR", dir);
        check();
        match previous {
            Some(v) => std::env::set_var("WINDTERM_PROFILE_DIR", v),
            None => std::env::remove_var("WINDTERM_PROFILE_DIR"),
        }
    }

    fn notes_in(dir: &Path) -> String {
        let logs = dir.join("logs");
        let Ok(entries) = fs::read_dir(&logs) else {
            return String::new();
        };
        let mut all = String::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("debug_pre-session_") {
                all.push_str(&fs::read_to_string(entry.path()).unwrap_or_default());
            }
        }
        all
    }

    #[test]
    fn four_stages_each_own_their_prefix() {
        assert_eq!(format_note(ErrorStage::Connect, "握手超时"), "[connect] 握手超时");
        assert_eq!(format_note(ErrorStage::Auth, "密码错误"), "[auth] 密码错误");
        assert_eq!(format_note(ErrorStage::Sftp, "连接失败"), "[sftp] 连接失败");
        assert_eq!(format_note(ErrorStage::Tunnel, "起不来"), "[tunnel] 起不来");
    }

    #[test]
    fn a_registered_secret_never_reaches_the_note() {
        let secret = "pre-session-pw-73";
        log::register_secret(secret);
        let note = format_note(ErrorStage::Auth, &format!("认证失败：password={secret}"));
        assert!(!note.contains(secret), "密码样串没被清洗：{note}");
        assert!(note.contains("[redacted]"), "{note}");
        assert!(note.starts_with("[auth] "), "{note}");
    }

    #[test]
    fn enabled_writes_one_note_into_a_debug_log_file() {
        let dir = TempDir::new();
        let d = dir.path().to_path_buf();
        with_profile_dir(&d, || {
            log_pre_session_error(ErrorStage::Sftp, "会话的连接参数有误", true);
        });
        let text = notes_in(&d);
        assert!(text.contains("[sftp] 会话的连接参数有误"), "{text}");
        assert!(text.contains("# WindTerm session log"), "{text}");
        // 与排障日志同源：文件名沿用 `debug_` 前缀而不是另造一套。
        assert!(fs::read_dir(d.join("logs"))
            .expect("logs 目录")
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("debug_pre-session_")));
    }

    #[test]
    fn switch_off_does_not_touch_the_disk() {
        let dir = TempDir::new();
        let d = dir.path().to_path_buf();
        with_profile_dir(&d, || {
            log_pre_session_error(ErrorStage::Tunnel, "隧道起不来", false);
        });
        assert!(notes_in(&d).is_empty(), "开关关着不该落盘");
        assert!(!d.join("logs").exists(), "开关关着连目录都不该建");
    }

    #[test]
    fn unwritable_dir_degrades_silently_without_panicking() {
        // 传一个「文件」当目录：`create_dir_all` 必然失败。
        // 调用方拿回正常控制流，于是照旧弹窗——这里只要不 panic 即为证据。
        let dir = TempDir::new();
        let blocker = dir.path().join("not-a-dir");
        fs::write(&blocker, b"x").expect("造一个挡路的文件");
        write_note(&blocker, ErrorStage::Connect, "连不上");
        assert_eq!(fs::read(&blocker).expect("原文件未被改动"), b"x");
    }
}
