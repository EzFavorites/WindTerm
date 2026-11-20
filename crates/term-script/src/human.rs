//! 人机通道（ADR-0027 ②）：脚本向**人**要一样东西的唯一边界。
//!
//! # 为什么不是「`ask(prompt) -> String` 一个方法」
//!
//! SecureCRT 的 `Dialog` 有四种用法（提示框 / 确认框 / 输入框 / 密码框），
//! 它们要回来的东西**类型不同**：一个字符串、一个布尔、一个 1 起序号。
//! 压成一个字符串再让脚本自己解析（`"y"` / `"1"`）是这类 API 最常见的
//! 静默错误源——用户输入 `yes please` 时到底算不算「是」。
//!
//! 所以这里用枚举把「问什么」与「答什么」配对：
//! [`Ask`] 六个形态 ↔ [`Answer`] 四个形态，引擎侧按形态取对应的那个返回值。
//!
//! # 三档降级
//!
//! | 宿主 | 提示 / 确认 / 输入 / 选择 | 密码 | 文件框 |
//! |---|---|---|---|
//! | 界面 | 模态 | 模态（掩码） | 原生框 |
//! | CLI + TTY | stderr + 读一行 | **显式拒绝** | 显式拒绝 |
//! | 无头 | `NoInput` | 显式拒绝 | 显式拒绝 |
//!
//! CLI 的密码输入刻意不支持：`read_line` 会把密码**明文回显**到终端上，
//! 比「不支持」更糟——同一段脚本在界面里掩码、在 CI 日志里明文，用户不会
//! 想到去查这件事。

use std::path::PathBuf;

/// 消息框的图标（纯展示，宿主不认识就画默认图标）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageIcon {
    None,
    Info,
    Warning,
    Error,
    Question,
}

/// 消息框的按钮组。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageButton {
    Ok,
    OkCancel,
    YesNo,
    YesNoCancel,
}

impl MessageButton {
    /// 这一组按钮的标签（顺序即返回值集合）。
    pub fn labels(self) -> &'static [&'static str] {
        match self {
            MessageButton::Ok => &["ok"],
            MessageButton::OkCancel => &["ok", "cancel"],
            MessageButton::YesNo => &["yes", "no"],
            MessageButton::YesNoCancel => &["yes", "no", "cancel"],
        }
    }

    /// `"yes"` / `"no"` / `"ok"` / `"cancel"` 是否是这组按钮里的一个。
    pub fn contains(self, label: &str) -> bool {
        self.labels().contains(&label)
    }
}

/// 「问什么」。`title` 是框的标题，`text` / `prompt` 是框里的正文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// 一个 OK 按钮的提示框（返回 [`Answer::Ack`]）。
    Alert { title: String, text: String },
    /// 带按钮组的消息框（返回 [`Answer::Text`]，值是按钮标签）。
    ///
    /// 为什么不返回整数 ID（CRT 的 1/2/6/7）：那一组编号把 Win32 的历史
    /// 焊进了脚本，读代码的人得查表才知道 6 是「是」。
    Message {
        title: String,
        text: String,
        icon: MessageIcon,
        buttons: MessageButton,
        default_button: Option<String>,
    },
    /// 是 / 否（返回 [`Answer::Bool`]）。
    Confirm { title: String, text: String },
    /// 单行文本输入（返回 [`Answer::Text`]）。
    Text {
        title: String,
        prompt: String,
        default: String,
    },
    /// 密码输入（掩码；返回 [`Answer::Text`]）。
    Secret { title: String, prompt: String },
    /// 多选一（返回 [`Answer::Choice`]，**1 起**，与 `wait_any` 的 index 一致）。
    Choose {
        title: String,
        prompt: String,
        options: Vec<String>,
    },
}

impl Ask {
    /// 一句话描述这次询问（日志与错误信息用）。
    pub fn describe(&self) -> String {
        match self {
            Ask::Alert { title, .. } => format!("提示框 {title:?}"),
            Ask::Message { title, .. } => format!("消息框 {title:?}"),
            Ask::Confirm { title, .. } => format!("确认框 {title:?}"),
            Ask::Text { title, .. } => format!("输入框 {title:?}"),
            Ask::Secret { title, .. } => format!("密码框 {title:?}"),
            Ask::Choose { title, .. } => format!("选择框 {title:?}"),
        }
    }
}

/// 回答。与 [`Ask`] 的形态一一对应，取错形态是脚本 bug（引擎报错）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// 提示框被确认。
    Ack,
    /// 是 / 否。
    Bool(bool),
    /// 文本（含按钮标签 `"yes"` / `"no"` / `"ok"` / `"cancel"`）。
    Text(String),
    /// 多选一的序号（**1 起**）。
    Choice(usize),
}

/// 问不到 / 问不成的原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AskError {
    /// 这里没有人可以回答（无头、界面里的脚本没有输入源）。
    NoInput,
    /// 用户取消了（关掉对话框 / 按了 Esc）。
    ///
    /// 与「返回空串」是两件事：空串是用户按了回车，取消是用户不想回答。
    Cancelled,
    /// 这个宿主没有该能力（CLI 的密码框、文件框）。
    Unsupported(String),
    /// 请求本身不合法（未知 `kind`、按钮组里没有那个默认按钮、选项为空）。
    BadRequest(String),
    /// 能力没有授予（文件框需要 `fs` 授权）。
    Denied(String),
}

impl std::fmt::Display for AskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AskError::NoInput => f.write_str(
                "这里没有人可以回答（界面里的脚本与无窗口进程都不带输入源）：\
                 请把「要人确认」的那条分支显式跳过",
            ),
            AskError::Cancelled => f.write_str("用户取消了这次询问"),
            AskError::Unsupported(why) => write!(f, "当前宿主不支持这次询问：{why}"),
            AskError::BadRequest(why) => write!(f, "询问的参数不合法：{why}"),
            AskError::Denied(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for AskError {}

/// 文件选择框。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickFile {
    Open {
        title: String,
        filter: String,
        default: String,
    },
    Save {
        title: String,
        filter: String,
        default: String,
    },
}

impl PickFile {
    pub fn is_save(&self) -> bool {
        matches!(self, PickFile::Save { .. })
    }
}

/// 人机通道。三个方法都带默认实现 = **显式降级**，不是静默成功。
pub trait HumanChannel: Send + Sync {
    /// 把一句话送到用户眼前。没有读者时写 stderr（留在日志里总比丢掉好），
    /// 但**绝不**往会话里写——那会被 `read` 当成远端输出读走。
    fn notify(&self, message: &str) {
        eprintln!("[script] {message}");
    }

    /// 问一次。没有可回答的人时返回 [`AskError::NoInput`]，
    /// **不**返回默认值冒充「用户选了默认」。
    fn ask(&self, _request: &Ask) -> Result<Answer, AskError> {
        Err(AskError::NoInput)
    }

    /// 打开文件框。用户选中的路径由调用方追加进本次 `fs` 授权
    /// （「用户亲手选的就是用户同意的那一个」，ADR-0027）。
    fn pick_file(&self, _request: &PickFile) -> Result<Option<PathBuf>, AskError> {
        Err(AskError::Unsupported(
            "当前宿主没有文件选择框（无头运行 / 没有窗口）".to_string(),
        ))
    }
}

/// 无头宿主：提示进 stderr，提问一律 `NoInput`，文件框一律拒绝。
///
/// 名字叫 `Headless` 而不是 `Null`：它的行为是**有语义的降级**，不是一个
/// 空对象——`notify` 真的会把话写出去。
#[derive(Clone, Copy, Debug, Default)]
pub struct HeadlessHuman;

impl HumanChannel for HeadlessHuman {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_headless_channel_refuses_instead_of_inventing_answers() {
        let human = HeadlessHuman;
        assert_eq!(human.ask(&Ask::Confirm { title: "t".into(), text: "x".into() }), Err(AskError::NoInput));
        assert!(matches!(
            human.pick_file(&PickFile::Open { title: "t".into(), filter: String::new(), default: String::new() }),
            Err(AskError::Unsupported(_))
        ));
    }

    #[test]
    fn message_buttons_agree_with_their_labels() {
        for buttons in [
            MessageButton::Ok,
            MessageButton::OkCancel,
            MessageButton::YesNo,
            MessageButton::YesNoCancel,
        ] {
            for label in buttons.labels() {
                assert!(buttons.contains(label), "{buttons:?} 应含 {label}");
            }
            assert!(!buttons.contains("maybe"), "{buttons:?} 不该含 maybe");
        }
        assert_eq!(MessageButton::YesNoCancel.labels().len(), 3);
    }

    #[test]
    fn cancelling_is_told_apart_from_an_empty_answer() {
        assert_ne!(AskError::Cancelled, AskError::NoInput);
        assert!(AskError::Cancelled.to_string().contains("取消"));
    }
}
