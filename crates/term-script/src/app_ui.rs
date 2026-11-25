//! 应用控制面端口（ADR-0027 第七端口）。
//!
//! 脚本要读应用状态（面板可见性 / 停靠位 / 主区窗格数 / 窗口旗标 / 语言）、
//! 要改面板可见性——这两件事既不属于会话面（参数里没有 `SessionId`），
//! 也不属于 `ui`（`ui` 管的是窗口显隐 / 退出 / 取选区，见 `clip_ui.rs`）。
//! 所以单独一个端口，与 `HumanChannel` / `LocalFs` / `Process` 同一套形状：
//!
//! - **默认实现全 `Unsupported`**。无头宿主（`desk serve` / `desk script run`）
//!   显式报不支持，**绝不返回空快照冒充成功**——那会让脚本把「这里没有界面」
//!   当成「界面是空的」。
//! - **方法只认纯数据**：入参与返回都是 `term_core::app_state` 里的类型，
//!   本 crate 不认识 `ViewState` / `DockLayout` / `egui`（`term-script` 不依赖 UI）。
//! - **回执带 `changed`**：`changed` 为空即「确实什么都没变」（设成同值）。
//!   这一条把「点了有效果」变成可判定的语句：调用方不必截图确认。
//!
//! # 本刀只放两个方法
//!
//! `layout` / `prefs_set` 留在 ADR-0027 里但**尚未定义实现**：
//! 布局的 `float` 依赖指针位置、偏好只有 `language` 一条 live 通路，
//! 两者都要先补下层。这里先不给空方法，免得有人以为
//! 「trait 上有就是能用」。

use std::fmt;

use term_core::app_state::{AppSnapshot, PanelState};

/// 应用控制面的一次失败。
///
/// 与 `UiError` / `AskError` 同一粒度：**分得清「没授权」「宿主没有」「参数错」
/// 「正忙」**，脚本才能据此走不同分支，而不是撞一个笼统的失败。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppError {
    /// 授权不足（`Capabilities.app` 档不够）。文案带 requested / granted。
    Denied(String),
    /// 宿主没有界面（无头进程）。**不是**「界面是空的」。
    Unsupported(String),
    /// 参数本身不合法（可见性不是 bool、布局序号越界这类**形状**问题）。
    BadArgument(String),
    /// 名字在这一层的表里不存在（`kind` = "停靠面板" / "偏好"）。
    ///
    /// 与 `BadArgument` 分开：拼错名字与参数形状不对是两种改法——前者要
    /// 拿清单对一遍，后者要改类型。`kind` 用中文词而不是 `"panel"` 这类
    /// 标识符，因为它直接进脚本看到的文案。
    UnknownName { kind: &'static str, name: String },
    /// 队列里已有未落地的请求（同一时刻只处理一条）。
    Busy,
    /// 等回执超时（上限 2 秒或剩余预算）。
    Timeout,
    /// 脚本被停止。
    Cancelled,
    /// 宿主侧 IO 失败（目前只有偏好落盘会走到）。
    Io(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Denied(why) => write!(f, "{why}"),
            AppError::Unsupported(why) => write!(f, "当前宿主没有应用界面：{why}"),
            AppError::BadArgument(why) => write!(f, "参数不合法：{why}"),
            AppError::UnknownName { kind, name } => {
                write!(f, "不认识的{kind}名 {name:?}：它不在 {kind} 表里")
            }
            AppError::Busy => f.write_str("另一个应用状态请求还在处理，稍后重试"),
            AppError::Timeout => f.write_str("等界面回执超时（界面可能卡住了）"),
            AppError::Cancelled => f.write_str("脚本被停止"),
            AppError::Io(why) => write!(f, "宿主侧写盘失败：{why}"),
        }
    }
}

impl std::error::Error for AppError {}

/// 一次成功写入的回执。
///
/// `changed` 只列**真的变了**的字段（`panels.filer` 这种路径式名字）；
/// 设成同值它就是空数组——这正是「幂等」在回执里的样子。
///
/// `panels` 是写入**之后**的十块面板全景（回执形状）。回执里带全景
/// 而不是只给 `changed`，是为了让脚本的读-改-写**不必赌时序**：如果只回
/// 「变了什么」，想接着判断现状就得再发一次 `app_state`，而中间可能隔着别人的写入。
#[derive(Clone, Debug, PartialEq)]
pub struct AppDelta {
    pub changed: Vec<String>,
    /// 写入之后的界面状态版本（与 [`AppSnapshot::rev`] 同一个计数器）。
    pub rev: u64,
    /// 写入之后的十块面板（`PanelId::ALL` 顺序）。
    pub panels: Vec<PanelState>,
}

impl AppDelta {
    /// 什么都没变（设成同值 / 名字对上了但状态已一致）。
    pub fn unchanged(rev: u64, panels: Vec<PanelState>) -> Self {
        Self {
            changed: Vec::new(),
            rev,
            panels,
        }
    }
}

/// 一次命令派发的回执。
///
/// `dispatched = false` 是**显式**的「命令 ID 没被认出来，什么都没做」：脚本
/// 不能把未知 ID 当成执行成功（ADR-0019 禁止静默成功）。宿主没有界面时走
/// `AppError::Unsupported`，与 `AppDelta` 那一族同一纪律。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandAck {
    pub dispatched: bool,
}

impl CommandAck {
    pub fn dispatched(dispatched: bool) -> Self {
        Self { dispatched }
    }
}

/// 自省清单里的一条。
///
/// `source` 取 `app`（应用命令台账）、`script`（会话能力面）、`crt`（兼容门面）
/// 三者之一，让脚本一眼看出「这个 ID 从哪来、该用哪种写法」。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogEntry {
    pub id: String,
    pub summary: String,
    pub source: String,
}

/// 一次布局请求。
///
/// `panel` / `zone` 用字符串、校验放在**宿主侧**：`PanelId` / `DockZone` 住在
/// `term-app`，而 `term-script` 只依赖 `term-core`——在这层再抄一份名字表，
/// 就是两张会各自漂的表。校验在「拿得到那张表的一侧」做，错误也由那边报。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutOp {
    /// 全部回到默认布局。
    Reset,
    /// 停到某带的第 `index` 位。`None` = 追加到该带末尾（`DockLayout::insert`
    /// 的夹取方向就是追加，API 层只在**显式**越界时拒绝）。
    Dock {
        panel: String,
        zone: String,
        index: Option<u16>,
    },
    /// 收回它的默认带（浮窗 → 停靠）。
    Redock { panel: String },
}

/// 应用控制面。方法都有默认实现 = 显式降级。
pub trait AppControl: Send + Sync {
    /// 读一份应用状态快照。
    ///
    /// 走的是「请求 → 界面线程回执」的同步往返（与 `ask` 同一条纪律），
    /// 所以返回的是**某一帧**的状态，而不是共享槽里那份可能过期的副本。
    fn snapshot(&self) -> Result<AppSnapshot, AppError> {
        Err(AppError::Unsupported("没有界面可读".to_string()))
    }

    /// 置位一块面板的显示开关（幂等）。
    ///
    /// 失败语义与回执见 [`AppError`] / [`AppDelta`]；`panel` 取
    /// [`AppSnapshot::panels`] 里的 `id`（`PanelId::name()` 的小写名）。
    fn panel_set(&self, _panel: &str, _visible: bool) -> Result<AppDelta, AppError> {
        Err(AppError::Unsupported("没有界面可改".to_string()))
    }

    /// 改布局（`reset` / `dock` / `redock`）。
    ///
    /// **会落盘**：`apply_layout_action` 末尾的 `save_layout()` 是既有行为，
    /// 这一刀不改它。所以授权要求 `app=configure`——`AppScope::Tune` 的定义是
    /// 「不落盘」，让 Tune 改用户的布局文件就把它自己的定义推翻了。
    fn layout(&self, _op: &LayoutOp) -> Result<AppDelta, AppError> {
        Err(AppError::Unsupported("没有界面可改布局".to_string()))
    }

    /// 按命令 ID 派发一条应用命令（2026-11-25）。
    ///
    /// 宿主负责把 ID 转给 `Action::parse`——`term-script` 不认识 `term-app`，
    /// 也绝不在本层抄一份命令 ID 表。默认实现显式报不支持，无头宿主不会
    /// 返回 `dispatched: true` 冒充执行过。
    fn command(&self, _id: &str) -> Result<CommandAck, AppError> {
        Err(AppError::Unsupported("没有界面可执行命令".to_string()))
    }

    /// 合并后的命令 / 动词清单，供脚本自省。
    ///
    /// 默认同样是显式降级：无头宿主不该返回空清单冒充「这个应用没有命令」。
    fn catalog(&self) -> Result<Vec<CatalogEntry>, AppError> {
        Err(AppError::Unsupported("没有界面可列命令".to_string()))
    }
}

/// 未接线的宿主：两个方法都 `Unsupported`。
///
/// 无头进程（`desk serve` / `desk script run`）与单测默认用它——
/// 与 `NoUi` / `NoFiles` / `NoProcess` 同一条规矩。
#[derive(Clone, Copy, Debug, Default)]
pub struct NoApp;

impl AppControl for NoApp {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_port_refuses_instead_of_pretending() {
        let app = NoApp;
        // 两件事都必须**显式**报不支持：返回空快照会让脚本以为「界面是空的」。
        assert!(matches!(app.snapshot(), Err(AppError::Unsupported(_))));
        assert!(matches!(
            app.panel_set("filer", true),
            Err(AppError::Unsupported(_))
        ));
    }

    #[test]
    fn an_unchanged_delta_has_no_changed_paths_but_keeps_the_rev() {
        let delta = AppDelta::unchanged(7, Vec::new());
        assert!(delta.changed.is_empty(), "设成同值就是空 changed");
        assert_eq!(delta.rev, 7, "版本号照旧传出，客户端据此判断要不要重取");
    }

    #[test]
    fn every_error_variant_says_what_happened() {
        // 文案是脚本作者唯一的线索：不能出现空串，也不能只说「失败了」。
        let cases = [
            AppError::Denied("缺 app 授权".to_string()),
            AppError::Unsupported("没有界面".to_string()),
            AppError::BadArgument("visible 必须是 bool".to_string()),
            AppError::UnknownName {
                kind: "停靠面板",
                name: "pane_filer".to_string(),
            },
            AppError::Busy,
            AppError::Timeout,
            AppError::Cancelled,
            AppError::Io("prefs.json 不可写".to_string()),
        ];
        for err in cases {
            let text = err.to_string();
            assert!(!text.is_empty(), "{err:?} 的文案不能是空串");
            assert!(text.chars().count() > 4, "{err:?} 的文案太短：{text}");
        }
    }
}