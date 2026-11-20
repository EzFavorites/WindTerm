//! 剪贴板端口（ADR-0027 D1 ⑤）与窗口端口（⑥）。
//!
//! # 剪贴板：为什么「读」要授权、「写」不要
//!
//! 读剪贴板等于读用户上一次复制的东西——可能是密码管理器里刚复制的那一条。
//! 写剪贴板只是把脚本的结果放到用户手上，不泄露任何东西。所以两个方向
//! 的授权位不同：`clipboard` 位只管 `get`。这与 CRT 的 `Clipboard` 对象
//! 两向都有是对齐的，只是我们给「读」多加了一道门。
//!
//! 只支持 UTF-8 文本。`CF_TEXT` / RTF / HTML 一律
//! [`ClipError::Unsupported`]——**不**静默当成纯文本返回（那会让脚本以为
//! 「剪贴板里只有这段字」，而用户其实复制了一张图）。
//!
//! # 窗口：为什么放在这里
//!
//! `window` / `app_quit` / `selection` 都只能由**界面**回答；无头宿主必须
//! 显式 `Unsupported`。它们与剪贴板同属「脚本碰不到、只能问宿主」的那类，
//! 放一个文件里是为了让「谁有界面谁实现」一眼看清。

use std::fmt;

use term_core::session_runtime::SessionId;

/// 剪贴板失败方式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipError {
    /// 没有 `clipboard` 授权（只针对读）。
    Denied(String),
    /// 宿主没有剪贴板（无头 / CLI 没装系统命令）。
    Unsupported(String),
    /// 被取消（等用户从别的 App 复制回来时）。
    Cancelled,
    /// 平台层失败。
    Io(String),
}

impl fmt::Display for ClipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClipError::Denied(why) => write!(f, "{why}"),
            ClipError::Unsupported(why) => write!(f, "当前宿主没有剪贴板：{why}"),
            ClipError::Cancelled => f.write_str("读剪贴板已被取消"),
            ClipError::Io(why) => write!(f, "剪贴板操作失败：{why}"),
        }
    }
}

impl std::error::Error for ClipError {}

/// 剪贴板端口。只支持 UTF-8 文本。
pub trait Clipboard: Send + Sync {
    fn get(&self) -> Result<String, ClipError>;

    fn set(&self, text: &str) -> Result<(), ClipError>;
}

/// 未接线的宿主：读 `Unsupported`，写 `Unsupported`。
#[derive(Clone, Copy, Debug, Default)]
pub struct NoClipboard;

impl Clipboard for NoClipboard {
    fn get(&self) -> Result<String, ClipError> {
        Err(ClipError::Unsupported("没有接入剪贴板".to_string()))
    }
    fn set(&self, _: &str) -> Result<(), ClipError> {
        Err(ClipError::Unsupported("没有接入剪贴板".to_string()))
    }
}

/// 窗口操作失败方式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiError {
    /// 没有 `ui` 授权。
    Denied(String),
    /// 没有界面（无头 / CLI）。
    Unsupported(String),
    /// 用户取消了（退出确认里点了「取消」）。
    Cancelled,
    /// 界面层失败。
    Io(String),
}

impl fmt::Display for UiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UiError::Denied(why) => write!(f, "{why}"),
            UiError::Unsupported(why) => write!(f, "当前宿主没有窗口：{why}"),
            UiError::Cancelled => f.write_str("用户在确认框中取消了"),
            UiError::Io(why) => write!(f, "窗口操作失败：{why}"),
        }
    }
}

impl std::error::Error for UiError {}

/// 窗口 / 视图端口。三个方法都只能由有界面的宿主回答。
pub trait Ui: Send + Sync {
    /// 当前选区的文本（没有选区 = 空串，**不是**错误：CRT 同语义）。
    ///
    /// 为什么不快照：选区随着用户操作一直在变，脚本取到一份「几秒前的选区」
    /// 比取不到更糟——它看起来是有效数据。所以每次都向界面现取。
    fn selection(&self, _session: SessionId) -> Result<String, UiError> {
        Err(UiError::Unsupported("没有界面可取选区".to_string()))
    }

    /// 显示 / 隐藏主窗口。
    fn set_visible(&self, _visible: bool) -> Result<(), UiError> {
        Err(UiError::Unsupported("没有窗口可显示".to_string()))
    }

    /// 退出应用。`save = true` 时先保存布局与会话库。
    fn quit(&self, _save: bool) -> Result<(), UiError> {
        Err(UiError::Unsupported("没有应用可退出".to_string()))
    }
}

/// 未接线的宿主：三个方法都 `Unsupported`。
#[derive(Clone, Copy, Debug, Default)]
pub struct NoUi;

impl Ui for NoUi {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unwired_hosts_are_explicit() {
        assert!(matches!(NoClipboard.get(), Err(ClipError::Unsupported(_))));
        assert!(matches!(
            NoUi.selection(1),
            Err(UiError::Unsupported(_))
        ));
        assert!(matches!(NoUi.quit(true), Err(UiError::Unsupported(_))));
        assert!(matches!(NoUi.set_visible(false), Err(UiError::Unsupported(_))));
    }
}
