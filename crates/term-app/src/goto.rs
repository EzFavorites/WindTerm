//! Goto 能力——跳到回看绝对行 / 跳到某个已打开的会话。
//!
//! 把这两条留在 A 类，理由是「各要一个输入框 / 选择器，缺基础设施」；
//! 的命令面板已经把输入框、过滤列表、`↑↓/Enter/Esc` 键盘导航落地，
//! 所以本批只剩**各自的语义**：前者要把行号换算成视口位移，后者要在标签列表里
//! 切换活动项。两者共用一套状态形状（开着 / 哪种模式 / 输入 / 选中 / 错误），
//! 但都不碰标签列表与窗口系统——那些活在投影层。
//!
//! 与 Find 栏同构：输入是 `GotoState.input`（由 `reduce` 写），投影层只持有
//! 跨帧编辑缓冲；`Confirm` 由 reducer 解析输入并产出 `Effect`，投影层一个值都不
//! 回写。解析失败不是「点了没反应」：状态里的 `error` 会被面板读出来提示用户。

use crate::effect::Effect;

pub use crate::vocab::GotoMode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GotoState {
    pub visible: bool,
    pub mode: GotoMode,
    /// Line 模式是行号文本，Sessions 模式是过滤词。
    pub input: String,
    /// Sessions 模式的选中项（对过滤结果取模）。
    pub selected: usize,
    /// 上一次输入解析失败（只由 reducer 写，面板只读）。
    pub error: bool,
}

impl Default for GotoState {
    fn default() -> Self {
        Self {
            visible: false,
            mode: GotoMode::Line,
            input: String::new(),
            selected: 0,
            error: false,
        }
    }
}

pub use crate::vocab::GotoAction;


/// 回看绝对行号：0 = 当前屏首行，负数 = 历史行，正数 = 屏幕内下方的行。
///
/// 只允许纯整数（可带前导 `-`），空串 / 空白 / 带单位 / 小数一律拒绝。
/// 解析在这里做，投影层就不需要第二套「什么算合法行号」的判断（F3 教训）。
pub fn parse_line(input: &str) -> Option<i64> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let digits = s.strip_prefix('-').unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<i64>().ok()
}

/// 纯函数：只改 `GotoState`，返回待执行的跳转意图。
/// 命令 ID → 本能力的动作。未接线的 ID 返回 `None`。
///
/// 2026-11-12：实现只有一份，在 [`crate::parse`]（入口层）。这里保留同名入口，
/// 一是因为「每个能力自带字符串入口」是这个 crate 对外的约定，二是因为
/// `apps/desktop` 有直接调 `term_app::log::parse` 的地方——把路径改掉没有架构价值。
pub fn parse(id: &str) -> Option<GotoAction> {
    crate::parse::goto_parse(id)
}

pub fn reduce(state: &mut GotoState, action: GotoAction) -> Vec<Effect> {
    match action {
        GotoAction::Open(mode) => {
            state.visible = true;
            state.mode = mode;
            state.input.clear();
            state.selected = 0;
            state.error = false;
            Vec::new()
        }
        GotoAction::SetInput(text) => {
            state.input = text;
            // 重新输入就撤掉上一次的错误提示，否则用户改对了还挂着红字。
            state.error = false;
            state.selected = 0;
            Vec::new()
        }
        GotoAction::Move(delta, total) => {
            state.selected = if total == 0 {
                0
            } else {
                ((state.selected % total) as i64 + delta as i64).rem_euclid(total as i64) as usize
            };
            Vec::new()
        }
        GotoAction::Select(idx) => {
            state.selected = idx;
            Vec::new()
        }
        GotoAction::Confirm => match state.mode {
            GotoMode::Line => match parse_line(&state.input) {
                Some(row) => {
                    reset(state);
                    vec![Effect::GotoLine(row)]
                }
                None => {
                    // 不关面板、不假装成功：把失败显式记进状态给面板读。
                    state.error = true;
                    Vec::new()
                }
            },
            GotoMode::Sessions => {
                let idx = state.selected;
                reset(state);
                vec![Effect::GotoSession(idx)]
            }
        },
        GotoAction::Close => {
            reset(state);
            Vec::new()
        }
    }
}

fn reset(state: &mut GotoState) {
    state.visible = false;
    state.input.clear();
    state.selected = 0;
    state.error = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(mode: GotoMode) -> GotoState {
        let mut s = GotoState::default();
        reduce(&mut s, GotoAction::Open(mode));
        s
    }

    #[test]
    fn parse_opens_the_two_modes() {
        assert_eq!(
            parse("Window.GotoLine"),
            Some(GotoAction::Open(GotoMode::Line))
        );
        assert_eq!(
            parse("Window.GotoSession"),
            Some(GotoAction::Open(GotoMode::Sessions))
        );
        assert_eq!(parse("Window.GotoSymbol"), None);
    }

    #[test]
    fn line_parsing_accepts_only_plain_integers_with_optional_minus() {
        assert_eq!(parse_line("0"), Some(0));
        assert_eq!(parse_line("-12"), Some(-12));
        assert_eq!(parse_line(" 42 "), Some(42));
        assert_eq!(parse_line("+7"), None, "不带加号，避免和终端行号混淆");
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("   "), None);
        assert_eq!(parse_line("12a"), None);
        assert_eq!(parse_line("1.5"), None);
        assert_eq!(parse_line("-"), None);
    }

    #[test]
    fn opening_resets_every_field() {
        let mut s = open(GotoMode::Line);
        reduce(&mut s, GotoAction::SetInput("-9".into()));
        s.error = true;
        s.selected = 4;
        reduce(&mut s, GotoAction::Open(GotoMode::Sessions));
        assert!(s.visible);
        assert_eq!(s.mode, GotoMode::Sessions);
        assert!(s.input.is_empty());
        assert_eq!(s.selected, 0);
        assert!(!s.error);
    }

    #[test]
    fn confirming_a_valid_line_closes_and_emits_the_row() {
        let mut s = open(GotoMode::Line);
        reduce(&mut s, GotoAction::SetInput("-25".into()));
        assert_eq!(
            reduce(&mut s, GotoAction::Confirm),
            vec![Effect::GotoLine(-25)]
        );
        assert!(!s.visible);
        assert!(s.input.is_empty());
    }

    #[test]
    fn an_invalid_line_keeps_the_panel_open_and_marks_the_error() {
        let mut s = open(GotoMode::Line);
        reduce(&mut s, GotoAction::SetInput("abc".into()));
        assert!(reduce(&mut s, GotoAction::Confirm).is_empty());
        assert!(s.visible, "不能静默关掉，否则用户不知道输错了");
        assert!(s.error);
        reduce(&mut s, GotoAction::SetInput("-3".into()));
        assert!(!s.error, "重新输入要撤掉错误提示");
    }

    #[test]
    fn confirming_a_session_emits_the_selected_index() {
        let mut s = open(GotoMode::Sessions);
        reduce(&mut s, GotoAction::Move(2, 4));
        assert_eq!(s.selected, 2);
        assert_eq!(
            reduce(&mut s, GotoAction::Confirm),
            vec![Effect::GotoSession(2)]
        );
        assert!(!s.visible);
    }

    #[test]
    fn selection_wraps_and_empty_lists_cannot_overflow() {
        let mut s = open(GotoMode::Sessions);
        reduce(&mut s, GotoAction::Move(-1, 3));
        assert_eq!(s.selected, 2);
        reduce(&mut s, GotoAction::Move(1, 3));
        assert_eq!(s.selected, 0);
        reduce(&mut s, GotoAction::Move(5, 0));
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn close_clears_the_error_too() {
        let mut s = open(GotoMode::Line);
        reduce(&mut s, GotoAction::SetInput("nope".into()));
        reduce(&mut s, GotoAction::Confirm);
        assert!(s.error);
        reduce(&mut s, GotoAction::Close);
        assert!(!s.visible && !s.error && s.input.is_empty());
    }
}
