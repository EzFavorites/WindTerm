//! 本地文件端口（ADR-0027 ③）。
//!
//! # 为什么脚本需要它
//!
//! CRT 的脚本语言（VBScript / Python）自带 `open()`，所以「读一份清单、
//! 循环里发命令、把结果写成 CSV」是它的天然用法。Rhai 默认**没有**文件访问
//! （沙箱只做减法），不补这个端口就等于少了半个语言。
//!
//! # 谁来做授权
//!
//! 端口只声明「读写列删」，**授权判定在实现侧**（`apps/desktop/src/script_fs.rs`）：
//! 它先 [`normalize_for`] 再交给 [`term_core::script_caps`] 比对白名单。之所以
//! 不让端口自己判：白名单是**本次运行**的信封，而端口是宿主接进来的；
//! 规范化函数放这里是为了让 `sftp_get` / `sftp_put` 的本地端点复用同一条判定。

use std::ffi::OsString;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use term_core::session_runtime::Cancel;

/// 目录里的一项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
}

/// 一个文件 / 目录的元信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStat {
    pub path: String,
    pub size: u64,
    pub dir: bool,
    /// 只读（权限位上没有写位）。
    pub readonly: bool,
    /// 修改时间（Unix 秒）；取不到时为 `None`。
    pub modified: Option<u64>,
}

/// 文件操作的失败方式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsError {
    /// 没有授权 / 路径落在白名单外（软链指向外面也走这条）。
    Denied(String),
    /// 宿主没有文件能力（无头 / 未接线）。
    Unsupported(String),
    /// 被取消（大文件循环里）。
    Cancelled,
    /// 真的 IO 失败（带路径与原因）。
    Io(String),
    /// 参数不合法（空路径、目录当文件读）。
    BadArgument(String),
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsError::Denied(why) => write!(f, "{why}"),
            FsError::Unsupported(why) => write!(f, "当前宿主没有文件能力：{why}"),
            FsError::Cancelled => f.write_str("文件操作已被取消"),
            FsError::Io(why) => write!(f, "文件操作失败：{why}"),
            FsError::BadArgument(why) => write!(f, "文件操作的参数不合法：{why}"),
        }
    }
}

impl std::error::Error for FsError {}

/// 本地文件端口。
///
/// 每个方法都收**字符串路径**而不是 `PathBuf`：路径来自脚本，报错时要原样
/// 回显用户写的那一串（`PathBuf` 的 Debug 会加引号与转义）。
pub trait LocalFs: Send + Sync {
    fn read(&self, path: &str) -> Result<Vec<u8>, FsError>;

    /// 写入；`append = true` 时追加。返回写入的字节数。
    fn write(&self, path: &str, data: &[u8], append: bool) -> Result<u64, FsError>;

    fn list(&self, path: &str) -> Result<Vec<DirEntry>, FsError>;

    fn stat(&self, path: &str) -> Result<FileStat, FsError>;

    fn mkdir(&self, path: &str) -> Result<(), FsError>;

    fn remove(&self, path: &str) -> Result<(), FsError>;

    /// 只判定 + 规范化本地端点，**不读写文件**：给 `sftp_get` / `sftp_put` 用。
    ///
    /// 远端路径不属于本地白名单这个信任域，但每次落盘仍然要过 `LocalFs`
    /// （本模块 [`RemoteFiles`] 的契约）。默认显式 `Unsupported`：没接文件端口
    /// 就没有判定的依据，不能因为「只是个参数」就放行。
    fn authorize_read(&self, path: &str) -> Result<PathBuf, FsError> {
        let _ = path;
        Err(FsError::Unsupported(
            "没有接入本地文件端口，无法判定传输的本地路径".to_string(),
        ))
    }

    /// 同上，判写权限（目标文件可以尚不存在）。
    fn authorize_write(&self, path: &str) -> Result<PathBuf, FsError> {
        let _ = path;
        Err(FsError::Unsupported(
            "没有接入本地文件端口，无法判定传输的本地路径".to_string(),
        ))
    }
}

/// 把一个脚本写的路径变成**绝对、无 `..`** 的形态。
///
/// - 已存在的路径：`canonicalize`（软链一并解开）。
/// - 写新文件：从**最长已存在的祖先**规范起，再逐个拼回剩余分量——
///   `canonicalize` 对不存在的路径会失败，而 `fs_mkdir("out/a/b")` 的中间层
///   通常都不存在（见 [`normalize_missing_write_target`]）。
/// - 相对路径、含 `..` 的路径：直接拒（判定要在比较**之前**，见
///   `term_core::script_caps` 的模块文档）。
pub fn normalize_for(path: &str, write: bool) -> Result<PathBuf, FsError> {
    let raw = Path::new(path);
    if path.trim().is_empty() {
        return Err(FsError::BadArgument("路径不能为空".to_string()));
    }
    if raw.is_relative() {
        return Err(FsError::BadArgument(format!(
            "{path:?} 是相对路径；脚本里的文件路径必须写绝对路径（不知道「当前目录」是哪）"
        )));
    }
    if raw.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(FsError::BadArgument(format!(
            "{path:?} 含 ..；规范之前就拒（否则 logs/../../etc 会在 logs 根下通过）"
        )));
    }
    match std::fs::canonicalize(raw) {
        Ok(resolved) => Ok(resolved),
        Err(_) if write => normalize_missing_write_target(raw),
        Err(e) => Err(FsError::Io(format!("{path:?} 不可用：{e}"))),
    }
}

/// 目标不存在时：向上找到最长已存在的祖先，规范它，再拼回剩余分量。
///
/// 每一级「已存在」用 `symlink_metadata` 判：软链也算存在，于是它会被
/// `canonicalize` 解到真实目标；指向白名单外的目标由调用方的 `check_write`
/// 拒掉。悬空软链解不开，直接拒——否则 `open()` 会跟着它到白名单外建文件。
fn normalize_missing_write_target(raw: &Path) -> Result<PathBuf, FsError> {
    if is_dangling_symlink(raw) {
        return Err(FsError::Denied(format!(
            "{} 是指向不存在目标的软链，拒绝写入",
            raw.display()
        )));
    }
    let mut tail: Vec<OsString> = Vec::new();
    let name = raw
        .file_name()
        .ok_or_else(|| FsError::BadArgument(format!("{} 没有文件名", raw.display())))?;
    tail.push(name.to_os_string());
    let mut probe = raw
        .parent()
        .ok_or_else(|| {
            FsError::BadArgument(format!("{} 没有父目录，无从确定写到哪里", raw.display()))
        })?
        .to_path_buf();
    loop {
        match std::fs::symlink_metadata(&probe) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let name = probe.file_name().ok_or_else(|| {
                    FsError::BadArgument(format!("{} 没有文件名", probe.display()))
                })?;
                tail.push(name.to_os_string());
                probe = probe
                    .parent()
                    .ok_or_else(|| FsError::BadArgument(format!("{} 没有父目录", raw.display())))?
                    .to_path_buf();
            }
            Err(e) => {
                return Err(FsError::Io(format!(
                    "查不到 {} 的祖先 {}：{e}",
                    raw.display(),
                    probe.display()
                )))
            }
        }
    }
    let mut resolved = match std::fs::canonicalize(&probe) {
        Ok(path) => path,
        Err(_) if is_dangling_symlink(&probe) => {
            return Err(FsError::Denied(format!(
                "{} 是指向不存在目标的软链，拒绝写入",
                probe.display()
            )))
        }
        Err(e) => {
            return Err(FsError::Io(format!(
                "{} 的祖先 {} 不可用：{e}",
                raw.display(),
                probe.display()
            )))
        }
    };
    for part in tail.iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

fn is_dangling_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
        && std::fs::canonicalize(path).is_err()
}

/// 未接线的宿主：一律 `Unsupported`（不是静默成功，也不是伪装成 IO 错误）。
#[derive(Clone, Copy, Debug, Default)]
pub struct NoFiles;

impl LocalFs for NoFiles {
    fn read(&self, _: &str) -> Result<Vec<u8>, FsError> {
        Err(FsError::Unsupported("没有接入本地文件端口".to_string()))
    }
    fn write(&self, _: &str, _: &[u8], _: bool) -> Result<u64, FsError> {
        Err(FsError::Unsupported("没有接入本地文件端口".to_string()))
    }
    fn list(&self, _: &str) -> Result<Vec<DirEntry>, FsError> {
        Err(FsError::Unsupported("没有接入本地文件端口".to_string()))
    }
    fn stat(&self, _: &str) -> Result<FileStat, FsError> {
        Err(FsError::Unsupported("没有接入本地文件端口".to_string()))
    }
    fn mkdir(&self, _: &str) -> Result<(), FsError> {
        Err(FsError::Unsupported("没有接入本地文件端口".to_string()))
    }
    fn remove(&self, _: &str) -> Result<(), FsError> {
        Err(FsError::Unsupported("没有接入本地文件端口".to_string()))
    }
}

/// 远端文件（SFTP）端口（ADR-0027 `FileTransfer`）。
///
/// 与 [`LocalFs`] 分开：远端路径**不经过**本地白名单（那是另一个信任域），
/// 而本地路径的每次落盘仍要过 [`LocalFs`] 的授权。
pub trait RemoteFiles: Send + Sync {
    fn list(&self, path: &str) -> Result<Vec<DirEntry>, FsError>;
    fn get(&self, remote: &str, local: &str) -> Result<u64, FsError>;
    fn put(&self, local: &str, remote: &str) -> Result<u64, FsError>;

    /// 可取消形态：脚本的 `sftp_get` / `sftp_put` 走这里，把「停止」接进传输循环。
    /// 缺省退回不可取消的 `get` / `put`，让只实现基本形态的宿主照常工作。
    fn get_cancellable(
        &self,
        remote: &str,
        local: &str,
        _cancel: Arc<dyn Cancel>,
    ) -> Result<u64, FsError> {
        self.get(remote, local)
    }

    fn put_cancellable(
        &self,
        local: &str,
        remote: &str,
        _cancel: Arc<dyn Cancel>,
    ) -> Result<u64, FsError> {
        self.put(local, remote)
    }

    fn mkdir(&self, path: &str) -> Result<(), FsError>;
    fn rename(&self, from: &str, to: &str) -> Result<(), FsError>;
    fn remove(&self, path: &str) -> Result<(), FsError>;
    fn stat(&self, path: &str) -> Result<FileStat, FsError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unwired_host_says_unsupported_rather_than_pretending() {
        let files = NoFiles;
        for result in [
            files.read("/tmp/x").map(|_| ()),
            files.write("/tmp/x", b"hi", false).map(|_| ()),
            files.list("/tmp").map(|_| ()),
            files.stat("/tmp/x").map(|_| ()),
            files.mkdir("/tmp/x"),
            files.remove("/tmp/x"),
        ] {
            assert!(matches!(result, Err(FsError::Unsupported(_))));
        }
    }

    #[test]
    fn errors_keep_the_path_and_the_reason() {
        let err = FsError::Io("读不了 /tmp/x：没有那个文件".to_string());
        assert!(err.to_string().contains("/tmp/x"));
        assert!(FsError::Denied("没有 fs.read 授权".to_string())
            .to_string()
            .contains("fs.read"));
    }

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zterm-normalize-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        std::fs::canonicalize(&dir).expect("规范化临时目录")
    }

    #[test]
    fn normalization_refuses_relative_dotdot_and_empty_before_any_io() {
        for bad in ["", "  ", "a.txt", "./a.txt"] {
            assert!(matches!(
                normalize_for(bad, true),
                Err(FsError::BadArgument(_))
            ));
        }
        let root = sandbox("dotdot");
        let bad = format!("{}/../x", root.display());
        assert!(matches!(
            normalize_for(&bad, true),
            Err(FsError::BadArgument(_))
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    /// `fs_mkdir("out/a/b")` 的中间层都不存在：必须从最长存在的祖先规范起。
    #[test]
    fn a_chain_of_missing_ancestors_is_normalized_from_the_longest_existing_one() {
        let root = sandbox("ancestors");
        let target = root.join("out/a/b.txt");
        let resolved = normalize_for(&target.to_string_lossy(), true).expect("规范化");
        assert_eq!(resolved, target, "根已规范，剩余分量原样拼回");
        assert!(!resolved
            .components()
            .any(|c| matches!(c, Component::ParentDir)));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_symlink_inside_a_missing_chain_is_refused() {
        let root = sandbox("dangling-chain");
        let outside = std::env::temp_dir().join(format!(
            "zterm-normalize-outside-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&outside).expect("外部目录");
        let link = root.join("link");
        std::os::unix::fs::symlink(outside.join("missing"), &link).expect("悬空软链");
        let target = link.join("new.txt");
        match normalize_for(&target.to_string_lossy(), true) {
            Err(FsError::Denied(why)) => assert!(why.contains("软链"), "{why}"),
            other => panic!("悬空软链必须拒，实得 {other:?}"),
        }
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }
}
