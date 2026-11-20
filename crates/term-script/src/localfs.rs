//! 本地文件端口（ADR-0027 ③）。
//!
//! # 为什么脚本需要它
//!
//! SecureCRT 的脚本语言（VBScript / Python）自带 `open()`，所以「读一份清单、
//! 循环里发命令、把结果写成 CSV」是它的天然用法。Rhai 默认**没有**文件访问
//! （沙箱只做减法），不补这个端口就等于少了半个语言。
//!
//! # 谁来做授权
//!
//! 端口只声明「读写列删」，**授权判定在实现侧**（`apps/desktop/src/script_fs.rs`）：
//! 它先 `canonicalize` 再交给 [`term_core::script_caps`] 比对白名单。之所以
//! 不在这里判：规范化是 IO（要问文件系统软链指向哪），`term-script` 是与
//! 副作用无关的一层。

use std::fmt;

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
}
