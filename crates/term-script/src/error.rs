//! 脚本原语与引擎的失败方式。文案直接面向脚本作者（经 Rhai 错误透出）。

use std::fmt;

use term_core::session_runtime::WaitError;

/// 脚本原语的失败方式。一律显式失败，禁止静默空成功（ADR-0019）。
///
/// 带 `String` 的三个变体（[`UnknownKey`](Self::UnknownKey) 与两个会话名）
/// 让它失去 `Copy`：名字都是脚本运行时给的，不像整数那样拷贝免费。这点小代价
/// 换的是「哪个名字不认识」这句话能直接说给脚本作者听——只报「名字不认识」
/// 的话，脚本里同时握着好几个会话时得逐个试。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrimitiveError {
    /// 匹配串为空——空模式会立刻匹配任何东西，是脚本 bug，直接拒绝。
    /// `wait_any` 的空模式列表同样落到这里。
    EmptyPattern,
    /// 等待超时：会话输出里没出现匹配串。
    Timeout,
    /// 会话已结束（登记失败、没有屏幕事实、往死会话写字节）。
    Closed,
    /// 已经在记录日志时又调 `log_start`——静默换文件会截断旧日志。
    AlreadyRecording,
    /// 没在记录日志时调 `log_stop`——空成功会让调用方以为文件收尾了。
    NotRecording,
    /// `session_use` / `session_close` 点了本进程里不存在的会话名。带名字是
    /// 为了让报错直接指出「是哪一个」——脚本里通常同时握着好几个名字。
    ///
    /// 注意「名字不存在」与「会话已结束」是两件事：结束的会话仍在清单里，
    /// 名字照样能寻址（只是写不进去）。
    NoSuchSession(String),
    /// 关掉**当前绑定**的会话。不让它成功，是因为关掉之后剩下的原语就没有
    /// 作用对象了，而自动改绑到别的会话属于静默换目标——脚本会接着往一个
    /// 自己没打算用的会话里发命令。
    CloseCurrentSession(String),
    /// 当前没有任何绑定（所有会话都被关掉了，或集合报不出当前会话的名字）。
    NoBinding,
    /// 脚本给的参数不合法（尺寸 < 1 之类）。带一句原因：
    /// 「哪个参数错了」比「参数错」有用得多，而 `&'static str` 不破坏 `Copy`。
    BadArgument(&'static str),
    /// `keys` 收到认不出来的键名。带**名字**：一次可能按好几个键，只说
    /// 「键名不认识」等于让人逐个试。
    UnknownKey(String),
    /// 名字在**某一层自己的表**里不存在（`app_panel_set` 的停靠面板名、将来的
    /// 偏好键）。`kind` 是给人看的中文词（"停靠面板" / "偏好"）。
    ///
    /// 不复用 `UnknownKey`：那是键盘键名专用，文案里会拼 `known_names()`，
    /// 套到面板名上会输出「认不出的键名 "pane_filer"：可用 left shift…」。
    UnknownName { kind: &'static str, name: String },
    /// `ask` 在这里没有可交互的输入（界面里的脚本、`desk serve` 无窗口进程）。
    /// 不返回空串冒充「用户按了回车」——那会让「要不要继续」这条分支静默走成
    /// 「继续」。
    NoInput,
    /// 用户按了「停止」（ADR-0027）。与超时分开：那是「等的东西没来」，
    /// 这是「人不让它等了」。
    Cancelled,
    /// 授权信封里没有这一项。文案里带 requested / granted。
    Denied(String),
    /// 宿主没有该能力（无头宿主的窗口、CLI 的文件框……）。
    Unsupported(String),
    /// 其它已有完整文案的失败（人机通道、IO、SFTP……）。**不再二次包装**，
    /// 因为这些文案已经把「缺什么 / 试了什么」写清楚了。
    Failed(String),
}

impl fmt::Display for PrimitiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrimitiveError::EmptyPattern => {
                f.write_str("wait/read_until/wait_any 的匹配串（或模式列表）不能为空")
            }
            PrimitiveError::Timeout => f.write_str("等待超时：会话输出里没出现匹配串"),
            PrimitiveError::Closed => f.write_str("会话已结束"),
            PrimitiveError::AlreadyRecording => {
                f.write_str("该会话已经在记录日志；先 log_stop 再重新开始，不要静默换文件")
            }
            PrimitiveError::NotRecording => f.write_str("该会话当前没有在记录日志，无可停止"),
            PrimitiveError::NoSuchSession(name) => write!(
                f,
                "没有名为 {name:?} 的会话（先用 session_list() 取当前清单，不要拿旧名字试探）"
            ),
            PrimitiveError::CloseCurrentSession(name) => write!(
                f,
                "{name:?} 是当前绑定的会话，不能直接关：先 session_use(别的名字) 改绑，\
                 或让脚本正常结束（脚本退出时自带的那条会话由调用方关闭）"
            ),
            PrimitiveError::NoBinding => {
                f.write_str("当前没有绑定的会话：所有会话都已关闭，后续原语无从下手")
            }
            PrimitiveError::BadArgument(why) => write!(f, "参数不合法：{why}"),
            PrimitiveError::UnknownKey(name) => write!(
                f,
                "认不出的键名 {name:?}：可用 {}",
                term_core::keys::known_names()
            ),
            PrimitiveError::UnknownName { kind, name } => write!(
                f,
                "不认识的{kind}名 {name:?}：它不在{kind}表里（用清单接口取有效名字，\
                 不要拿拼错的试探）"
            ),
            PrimitiveError::NoInput => f.write_str(
                "这里没有可交互的输入（界面里的脚本与无窗口进程都不带标准输入）：\
                 ask 要不到回答，请把「要人确认」的那条分支显式跳过",
            ),
            PrimitiveError::Cancelled => f.write_str("脚本已被取消"),
            PrimitiveError::Denied(why) => f.write_str(why),
            PrimitiveError::Unsupported(why) => write!(f, "当前宿主不支持：{why}"),
            PrimitiveError::Failed(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for PrimitiveError {}

impl From<WaitError> for PrimitiveError {
    fn from(e: WaitError) -> Self {
        match e {
            WaitError::EmptyPattern => PrimitiveError::EmptyPattern,
            WaitError::Timeout => PrimitiveError::Timeout,
            WaitError::Closed => PrimitiveError::Closed,
            WaitError::Cancelled => PrimitiveError::Cancelled,
        }
    }
}

impl From<crate::cancel::Cancelled> for PrimitiveError {
    fn from(_: crate::cancel::Cancelled) -> Self {
        PrimitiveError::Cancelled
    }
}

impl From<crate::localfs::FsError> for PrimitiveError {
    fn from(e: crate::localfs::FsError) -> Self {
        use crate::localfs::FsError;
        match e {
            FsError::Denied(why) => PrimitiveError::Denied(why),
            FsError::Unsupported(why) => PrimitiveError::Unsupported(why),
            FsError::Cancelled => PrimitiveError::Cancelled,
            FsError::Io(why) => PrimitiveError::Failed(why),
            FsError::BadArgument(why) => PrimitiveError::Failed(why),
        }
    }
}

impl From<crate::proc::ProcError> for PrimitiveError {
    fn from(e: crate::proc::ProcError) -> Self {
        use crate::proc::ProcError;
        match e {
            ProcError::Denied(why) => PrimitiveError::Denied(why),
            ProcError::Unsupported(why) => PrimitiveError::Unsupported(why),
            ProcError::Cancelled => PrimitiveError::Cancelled,
            ProcError::Timeout => PrimitiveError::Timeout,
            ProcError::Io(why) => PrimitiveError::Failed(why),
        }
    }
}

impl From<crate::clip_ui::ClipError> for PrimitiveError {
    fn from(e: crate::clip_ui::ClipError) -> Self {
        use crate::clip_ui::ClipError;
        match e {
            ClipError::Denied(why) => PrimitiveError::Denied(why),
            ClipError::Unsupported(why) => PrimitiveError::Unsupported(why),
            ClipError::Cancelled => PrimitiveError::Cancelled,
            ClipError::Io(why) => PrimitiveError::Failed(why),
        }
    }
}

impl From<crate::clip_ui::UiError> for PrimitiveError {
    fn from(e: crate::clip_ui::UiError) -> Self {
        use crate::clip_ui::UiError;
        match e {
            UiError::Denied(why) => PrimitiveError::Denied(why),
            UiError::Unsupported(why) => PrimitiveError::Unsupported(why),
            UiError::Cancelled => PrimitiveError::Cancelled,
            UiError::Io(why) => PrimitiveError::Failed(why),
        }
    }
}

/// 应用控制面的失败 → 原语错误（ADR-0027 的第七端口）。
///
/// 映射三条：授权不足与「宿主没有」各走各的变体（脚本据此改分支）；
/// `Busy` / `Io` / 参数错落到 `Failed`——脚本侧拿不到「重试时机」，
/// 多一个类型只多一条没人读的分支（取舍）。
impl From<crate::app_ui::AppError> for PrimitiveError {
    fn from(err: crate::app_ui::AppError) -> Self {
        use crate::app_ui::AppError;
        match err {
            AppError::Denied(why) => PrimitiveError::Denied(why),
            AppError::Unsupported(why) => PrimitiveError::Unsupported(why),
            AppError::Cancelled => PrimitiveError::Cancelled,
            AppError::Timeout => PrimitiveError::Timeout,
            AppError::BadArgument(why) => PrimitiveError::Failed(format!("参数不合法：{why}")),
            AppError::UnknownName { kind, name } => PrimitiveError::UnknownName { kind, name },
            AppError::Busy => PrimitiveError::Failed("另一个应用状态请求还在处理".to_string()),
            AppError::Io(why) => PrimitiveError::Failed(why),
        }
    }
}

impl From<crate::human::AskError> for PrimitiveError {
    fn from(e: crate::human::AskError) -> Self {
        use crate::human::AskError;
        match e {
            AskError::NoInput => PrimitiveError::NoInput,
            AskError::Cancelled => PrimitiveError::Cancelled,
            AskError::Unsupported(why) => PrimitiveError::Unsupported(why),
            AskError::BadRequest(why) => PrimitiveError::Failed(why),
            AskError::Denied(why) => PrimitiveError::Denied(why),
        }
    }
}

/// 脚本执行的失败方式。
///
/// 两个变体都额外携带一个可选的 `(行, 列)` 位置（Rhai 原生 1 起坐标）。
/// 语法错的位置取自 Rhai 的 [`rhai::ParseError`]，运行期错的位置取自
/// [`rhai::EvalAltResult`]。位置为 `None` 表示引擎没给出（例如原语超时、
/// 沙箱超限这类不挂在某一行上的错误）。
///
/// `Display` 文案**只**由 `message` 决定、与位置无关——展示层若要结构化定位
/// （比如「点错误跳到第 N 行」），走 [`ScriptError::line`] / [`ScriptError::column`]，
/// 不要去正则抠 `Display` 文本。
#[derive(Debug, PartialEq, Eq)]
pub enum ScriptError {
    /// 编译期错误（语法错）。
    Syntax(String, Option<(usize, usize)>),
    /// 执行期错误：原语显式失败、沙箱超限、执行超时、脚本运行时异常。
    Runtime(String, Option<(usize, usize)>),
}

impl ScriptError {
    /// 错误所在的行（Rhai 原生 1 起；引擎未给出位置时返回 `None`）。
    pub fn line(&self) -> Option<usize> {
        self.position().map(|(line, _)| line)
    }

    /// 错误所在的列（Rhai 原生 1 起；引擎未给出位置时返回 `None`）。
    pub fn column(&self) -> Option<usize> {
        self.position().map(|(_, column)| column)
    }

    /// 行与列。`(行, 列)` 与 Rhai 对齐，均为 1 起；没有位置时返回 `None`。
    fn position(&self) -> Option<(usize, usize)> {
        match self {
            ScriptError::Syntax(_, pos) | ScriptError::Runtime(_, pos) => *pos,
        }
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScriptError::Syntax(msg, _) => write!(f, "脚本语法错误：{msg}"),
            ScriptError::Runtime(msg, _) => write!(f, "脚本执行失败：{msg}"),
        }
    }
}

impl std::error::Error for ScriptError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_text_is_unchanged_when_position_is_absent() {
        // 回归护栏：任何展示层都依赖这段文案，位置字段绝不能改它。
        let syntax = ScriptError::Syntax("bad token".to_string(), None);
        let runtime = ScriptError::Runtime("boom".to_string(), None);
        assert_eq!(syntax.to_string(), "脚本语法错误：bad token");
        assert_eq!(runtime.to_string(), "脚本执行失败：boom");
    }

    #[test]
    fn line_and_column_read_from_the_position_tuple() {
        let syntax = ScriptError::Syntax("bad token".to_string(), Some((3, 7)));
        let runtime = ScriptError::Runtime("boom".to_string(), Some((12, 1)));
        assert_eq!(syntax.line(), Some(3));
        assert_eq!(syntax.column(), Some(7));
        assert_eq!(runtime.line(), Some(12));
        assert_eq!(runtime.column(), Some(1));
    }

    #[test]
    fn missing_position_yields_none() {
        let err = ScriptError::Runtime("boom".to_string(), None);
        assert_eq!(err.line(), None);
        assert_eq!(err.column(), None);
    }
}
