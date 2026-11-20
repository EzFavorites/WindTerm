//! XFER-07：把一个**已落盘的文件**交给系统默认程序打开。
//!
//! 这是「用默认编辑器打开远端文件」这条需求的最后一米：远端文件先经 SFTP
//! 下载到临时目录（`temp_preview_path`），再由这里交给 `open` / `xdg-open` /
//! `cmd /C start`。**不新造内建编辑器**——`docs/10-prd.md` 等四处已经裁定不做
//! WindEdit，本文件就是那个裁定的兑现方式。
//!
//! # 为什么在 `ui/` 之外
//!
//! `scripts/verify.sh` 第 5 项（B2）的判据是「`ui/` 不得持有 OS 资源」，本项目
//! 的分层口径里「拉起一个外部进程」不是画。本文件与 `sysopen.rs` / `clipboard.rs`
//! / `fonts.rs` 同级：投影层的 OS 集成，且各只有一处。
//!
//! # 为什么复用 `sysopen` 而不是再写一份平台分派
//!
//! 平台分派（`open` / `xdg-open` / Windows）在 `crate::sysopen` 里已经存在，
//! 全仓五个调用点（帮助页 / 日志文件 / 日志目录 / …）都走它。再抄一份
//! `match target_os` 等于给自己两个必须同步维护的真相——而 Windows 那一条恰好
//! 是最容易写歪的（`explorer` 与 `cmd /C start` 对同一个路径的行为不同）。
//!
//! 所以本文件只做 `sysopen` **不做**的三件事，一件都不多：
//!
//! 1. **存在性前置校验**：路径不存在时**不** spawn 任何进程（否则用户的桌面
//!    会弹一个「找不到文件」的系统对话框，而我们这边拿到了 `Ok`）；
//! 2. **把 `io::Error` 翻成一句用户能读的话**（`Result<(), String>`，口径与
//!    `ui/dialogs/log.rs` 的两处调用一致）；
//! 3. **远端文件的临时落点**（`temp_preview_path`）：远端路径是本 crate 拿不到
//!    的东西，必须先在本地造一个真实路径。
//!
//! # 怎么避免 shell 注入
//!
//! 全程 `Command::new(程序名).arg(路径)`，**没有一次把路径拼进 shell 字符串**。
//! 路径里的空格、引号、`$`、`;` 都只是 `argv` 里的字节，不会被任何 shell 解释。
//! 这也是本文件不提供「自定义打开命令」配置项的原因：那等于把 `sh -c` 请回来。

use std::path::{Path, PathBuf};

/// `open_path` 的失败原因。
///
/// **不用 `String`**：`scripts/verify.sh` 第 4q 项把全仓「错误边界 String 化」
/// 冻结在 40 处（只许减不许增），而这两种失败的处置完全不同——「路径没了」是
/// 我们自己把路径拼错了（该修代码），「spawn 失败」是这台机器没有 `xdg-open`
/// （该告诉用户装一个）。拍平成一句话就没法区别了。
#[derive(Debug)]
pub enum OpenError {
    /// 路径不存在。**没有 spawn 任何进程**。
    NotFound(PathBuf),
    /// 系统默认程序起不来。`detail` 是平台给的原因（`io::Error` 的文本）。
    Spawn { path: PathBuf, detail: String },
}

impl OpenError {
    /// 一行可以直接画给用户的说明。
    pub fn message(&self) -> String {
        match self {
            OpenError::NotFound(path) => format!("{}: no such file", path.display()),
            OpenError::Spawn { path, detail } => format!("{}: {detail}", path.display()),
        }
    }
}

/// 交给系统默认程序打开。返回失败原因（调用方决定怎么告诉用户）。
///
/// 前置校验先于副作用：路径不存在直接 `Err`，**不 spawn**。这条不只是为了
/// 省一次进程——`open` 对不存在的路径会弹系统对话框，那既打断用户又掩盖
/// 「我们的路径拼错了」这个真问题。
pub fn open_path(path: &Path) -> Result<(), OpenError> {
    if !path.exists() {
        return Err(OpenError::NotFound(path.to_path_buf()));
    }
    crate::sysopen::open(path).map_err(|err| OpenError::Spawn {
        path: path.to_path_buf(),
        detail: err.to_string(),
    })
}

/// 预览远端文件时的本地落点：临时目录下一个本次运行独有的子目录 + 远端真名。
///
/// 为什么每个文件一个子目录：远端 `a/notes.txt` 与 `b/notes.txt` 的 basename
/// 相同，共用一个平铺目录会让第二次下载覆盖第一次的副本——而预览窗可能还
/// 开着前一个。
///
/// 目录名带纳秒时间戳（与 `term_io::preview` 的测试夹具同一写法）：不引
/// `tempfile`，也不需要在调用点判断撞名。**不清理**是刻意的：临时目录由系统
/// 按自己的策略回收，而在我们这边删掉用户可能正在编辑器里开的文件，比留一份
/// 副本更糟。
pub fn temp_preview_path(name: &str) -> std::io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("windterm-preview-{nanos}"));
    std::fs::create_dir_all(&dir)?;
    // 路径分隔符与 `..` 在远端文件名里是合法的字面字符，落到本地却会变成
    // 「往别的目录写」。只留最后一段，且把两个危险形态就地替换掉。
    let leaf = sanitize_leaf(name);
    Ok(dir.join(leaf))
}

/// 把远端 basename 收敛成一个安全的本地文件名。
///
/// 只做三件事：丢掉任何路径分隔符之前的部分、`..`/`.` 改成 `_`、空名给个兜底。
/// 不做 URL 编码：文件名要能被用户认出来，改成 `%2F` 反而看不懂。
fn sanitize_leaf(name: &str) -> String {
    let leaf = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let leaf = leaf.trim();
    if leaf.is_empty() || leaf == "." || leaf == ".." {
        "download".to_string()
    } else {
        leaf.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_path_is_rejected_before_any_process_is_spawned() {
        // 这是 `open_path` 唯一能在无头环境里安全测的分支：真的 spawn 一个
        // 编辑器会打开窗口、抢前台，CI 上还会失败在别的地方。
        let missing = std::env::temp_dir().join("windterm-no-such-file-xfer07.txt");
        let err = open_path(&missing).unwrap_err();
        // 失败原因必须可区分：这一支是「路径没了」，不是「起不来」。
        assert!(
            matches!(err, OpenError::NotFound(_)),
            "不存在的路径要落在 NotFound：{err:?}"
        );
        let message = err.message();
        assert!(message.contains("no such file"), "失败原因要能读懂：{message}");
        // 错误信息里带上路径本身，用户才知道是哪一个文件没开成。
        assert!(message.contains("windterm-no-such-file-xfer07.txt"), "{message}");
    }

    #[test]
    fn temp_preview_path_puts_the_remote_basename_under_a_unique_subdir() {
        let path = temp_preview_path("/srv/a/notes.txt").unwrap();
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("notes.txt"),
            "文件名取远端 basename"
        );
        let dir_name = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        assert!(
            dir_name.starts_with("windterm-preview-"),
            "子目录要带固定前缀，用户排查时一眼能认出是谁造的：{dir_name}"
        );
        assert!(
            path.parent().is_some_and(|p| p.is_dir()),
            "子目录要真的建出来，否则 SFTP 下载会因为没有父目录而失败"
        );
        assert_eq!(
            path.parent().and_then(|p| p.parent()),
            Some(std::env::temp_dir().as_path())
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap_or(&std::env::temp_dir()));
    }

    #[test]
    fn two_calls_never_share_a_directory() {
        // 同名不同目录的两个远端文件必须各拿一份副本：`a/notes.txt` 与
        // `b/notes.txt` 共用一个落点会互相覆盖。
        let first = temp_preview_path("/a/notes.txt").unwrap();
        let second = temp_preview_path("/b/notes.txt").unwrap();
        assert_ne!(first.parent(), second.parent());
        for path in [&first, &second] {
            let _ = std::fs::remove_dir_all(path.parent().unwrap_or(&std::env::temp_dir()));
        }
    }

    #[test]
    fn a_traversal_shaped_name_cannot_escape_the_temp_dir() {
        // 远端文件名里带 `../` 时，落点必须仍在那一个临时子目录里。
        let path = temp_preview_path("../../etc/passwd").unwrap();
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("passwd"));
        assert_eq!(
            path.parent().and_then(|p| p.parent()),
            Some(std::env::temp_dir().as_path()),
            "不许跳出 temp_dir 这一层"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap_or(&std::env::temp_dir()));
    }

    #[test]
    fn an_empty_or_dot_name_falls_back_to_a_usable_file_name() {
        // 远端可以给出空名 / `..`（畸形服务端或桩实现）。给一个能写盘的名字，
        // 而不是让 create/write 在系统调用边界上失败。
        for name in ["", "..", ".", "/", "a/b/"] {
            let leaf = sanitize_leaf(name);
            assert!(!leaf.is_empty(), "{name:?} 收敛出空名");
            assert_ne!(leaf, "..");
            assert!(!leaf.contains('/'));
        }
    }
}
