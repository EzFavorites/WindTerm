//! 终端行过滤器：`grep` 只留命中行，`grep -v` 丢掉命中行。
//!
//! 与 `find` 同纪律：状态只存用户的选择（开没开、查询、模式、大小写、正则），
//! **命中集合不是状态**。每一帧由 [`hidden_rows`] 从「当前事实窗口」现算，
//! 调用方把结果交给 `ScreenImage::drop_rows`，且必须是最后一次变换（`crop`
//! 依赖行号连续）。`LineSlice.row` 原样保留，因此行号与时间戳带洞而不重排。
//!
//! 行判定复用 `term_core::automation` 的 `Pattern` / `LinePolicy`：与脚本
//! `grep(pat)` / `grep_v(pat)` 同一套编译器，字面与正则共用一条语法路径。
//! 这里不引入第二个正则实现，也不碰 PTY / 窗口。

pub use crate::state::FilterState;
pub use crate::vocab::{Action, FilterAction, FilterMode};
use std::collections::HashSet;
use term_core::automation::{LinePolicy, Pattern, PatternError, Syntax};
use term_core::grid::ScreenImage;

/// 栏内开关走 `SetQuery` 的保留前缀（同 Find 的做法），不新增命令 ID。
/// 用 `flt` 与 Find 的 `fb` 区分，避免两边负载串台。
const UI_CMD_PREFIX: &str = "\u{0}flt";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterUiCommand {
    ToggleMode,
    ToggleCase,
    ToggleRegex,
}

impl FilterUiCommand {
    fn code(self) -> char {
        match self {
            Self::ToggleMode => 'm',
            Self::ToggleCase => 'c',
            Self::ToggleRegex => 'r',
        }
    }
}

pub fn ui_command(cmd: FilterUiCommand) -> FilterAction {
    FilterAction::SetQuery(format!("{UI_CMD_PREFIX}{}", cmd.code()))
}

pub fn ui_command_action(cmd: FilterUiCommand) -> Action {
    Action::Filter(ui_command(cmd))
}

fn decode_ui_command(payload: &str) -> Option<FilterUiCommand> {
    match payload.strip_prefix(UI_CMD_PREFIX)? {
        "m" => Some(FilterUiCommand::ToggleMode),
        "c" => Some(FilterUiCommand::ToggleCase),
        "r" => Some(FilterUiCommand::ToggleRegex),
        _ => None,
    }
}

/// 编译当前状态对应的行策略。
///
/// - 关栏或空查询 → `Ok(None)`（不过滤）。空查询不采用 `grep -v ""` 的
///   “全丢”语义，那会让用户一敲删除键整屏消失。
/// - 模式写错 → `Err(消息)`，调用方保持上一帧画面并给出红框提示，**不**
///   静默退回字面匹配（与 Find 的正则错误反馈同一原则）。
pub fn compile(state: &FilterState) -> Result<Option<LinePolicy>, PatternError> {
    if !state.visible || state.query.is_empty() {
        return Ok(None);
    }
    let syntax = if state.regex {
        Syntax::Regex
    } else {
        Syntax::Literal
    };
    let pattern = Pattern::compile(&state.query, syntax, state.case_sensitive)?;
    let policy = match state.mode {
        FilterMode::KeepMatching => LinePolicy::KeepIf(pattern),
        FilterMode::DropMatching => LinePolicy::DropIf(pattern),
    };
    Ok(Some(policy))
}

/// 需要隐藏的内容行号集合。纯函数：同一个窗口与状态必然给出同一结果。
///
/// 只判定窗口里**现有的**行；不在窗口里的行不进集合（滚动换窗口后重算，
/// 不会留下过期行号）。`LineSlice::text()` 是“渲染后的行文本”——跳过宽字符
/// 延续格、去掉行尾空白，与 Find 的扫描口径一致。
pub fn hidden_rows(state: &FilterState, window: &ScreenImage) -> HashSet<i64> {
    let (hidden, _) = diagnose(state, window);
    hidden
}

/// 每帧现算的可见 / 隐藏计数，供过滤栏显示 `kept/hidden`。不进状态。
pub fn counts(state: &FilterState, window: &ScreenImage) -> (usize, usize) {
    let hidden = hidden_rows(state, window).len();
    (window.lines.len().saturating_sub(hidden), hidden)
}

/// 一帧的诊断结果：`kept/hidden` 计数 + 正则错误。**命中集合不进状态**——它由
/// [`diagnose`] 在渲染前最后一步现算，UI 只读这一份，不再二次编译。
#[derive(Clone, Debug, Default)]
pub struct FilterDiag {
    pub kept: usize,
    pub hidden: usize,
    pub error: Option<PatternError>,
}

/// 一次编译给出「要丢哪些行 + 诊断」。同一帧只应调用一次（`ensure_view` 在
/// `drop_rows` **之前**），计数与丢弃因此必然同源——绝不允许 UI 从丢掉之后的
/// 窗口反算（`hidden` 会恒为 0），也不允许 UI 再编译一遍（一帧编译两次正则）。
pub fn diagnose(
    state: &FilterState,
    window: &ScreenImage,
) -> (HashSet<i64>, FilterDiag) {
    match compile(state) {
        Ok(Some(policy)) => {
            let hidden: HashSet<i64> = window
                .lines
                .iter()
                .filter(|line| !policy.admits(&line.text()))
                .map(|line| line.row)
                .collect();
            let diag = FilterDiag {
                kept: window.lines.len().saturating_sub(hidden.len()),
                hidden: hidden.len(),
                error: None,
            };
            (hidden, diag)
        }
        Ok(None) => {
            let diag = FilterDiag {
                kept: window.lines.len(),
                hidden: 0,
                error: None,
            };
            (HashSet::new(), diag)
        }
        Err(e) => {
            // 模式写错：**不过滤**（保持上一帧画面），只报诊断。与 Find 的正则
            // 错误反馈同一原则——不静默退回字面匹配。
            let diag = FilterDiag {
                kept: window.lines.len(),
                hidden: 0,
                error: Some(e),
            };
            (HashSet::new(), diag)
        }
    }
}

/// 只写 `FilterState`，不写任何窗口状态。
pub fn reduce(state: &mut FilterState, action: FilterAction) {
    match action {
        FilterAction::Open => {
            state.visible = true;
            state.query.clear();
            state.mode = FilterMode::KeepMatching;
            state.case_sensitive = false;
            state.regex = false;
        }
        FilterAction::Toggle => {
            state.visible = !state.visible;
        }
        FilterAction::Close => {
            state.visible = false;
            state.query.clear();
        }
        FilterAction::SetQuery(q) => match decode_ui_command(&q) {
            Some(FilterUiCommand::ToggleMode) => {
                state.mode = match state.mode {
                    FilterMode::KeepMatching => FilterMode::DropMatching,
                    FilterMode::DropMatching => FilterMode::KeepMatching,
                };
            }
            Some(FilterUiCommand::ToggleCase) => state.case_sensitive = !state.case_sensitive,
            Some(FilterUiCommand::ToggleRegex) => state.regex = !state.regex,
            None => state.query = q,
        },
        FilterAction::SetMode(mode) => state.mode = mode,
        FilterAction::ToggleCase => state.case_sensitive = !state.case_sensitive,
        FilterAction::ToggleRegex => state.regex = !state.regex,
    }
}

#[cfg(test)]
#[path = "filter_tests.rs"]
mod tests;
