//! 自动补全候选弹层：状态机 + 绘制（I-AC）。
//!
//! # 为什么状态挂在窗口而不是标签
//!
//! 候选是「当前这一帧正在敲的那一行」的产物，切换标签后旧候选立刻失效。
//! 挂在窗口上只需一处字段；每个状态记录 `owner`（发起它的标签 id），
//! 绘制与取用时先比对 owner，不符就当不存在——不必去改所有标签切换路径。
//!
//! # 为什么锚在终端右下角而不是光标格
//!
//! 补全只在贴底的命令行上有意义（回看旧输出时没有「正在输入的行」）。
//! 命令行总在屏幕底部，所以弹层钉在终端区右下角即可，不必把
//! `ScreenImage.cursor` 的相对行号换算成折叠后的屏幕行——那套换算在
//! 折叠/回看时有洞，算错一格就会把弹层画到别人的会话输出上。
//!
//! # 只写后缀
//!
//! [`CompletionState::accept`] 给出的是 `Candidate::insert`（缺失的后缀），
//! 调用方直接把这串字节写进 PTY。绝不发退格：shell 的行编辑状态本仓看不到，
//! 用户按过 `←` 之后退格会删掉别人的内容。方向键一旦按下就关闭弹层，
//! 正是为了让「光标可能不在行尾」的那一刻不再建议插入。

use egui::{Align2, Pos2, Rect, Rounding, Stroke, Ui, Vec2};
use term_core::completion::{Candidate, Preselect, Source};
use term_core::theme::CHROME_SCALE;

use crate::theme::chrome;

/// 在借用标签之前取好候选源。总开关关着时返回两张空表，一帧只多两次分配。
pub(in crate::ui) fn prepare_sources(
    enabled: bool,
    history: &term_core::command_history::CommandHistory,
    quickbar: &term_profile::quickbar::QuickBarConfig,
    scope: &str,
) -> (Vec<(String, u64)>, Vec<String>) {
    if !enabled {
        return (Vec::new(), Vec::new());
    }
    (
        history
            .recent_with_ts_for(scope, 2000)
            .into_iter()
            .map(|(line, ts)| (line.to_owned(), ts))
            .collect(),
        quickbar
            .groups
            .iter()
            .flat_map(|g| g.commands.iter().map(|c| c.send.trim_end().to_owned()))
            .collect(),
    )
}

/// 一个按键在补全上下文里的归属：`(意图, 是否阻止重算, 是否消费事件)`。
/// 消费 = 不写进 PTY；阻止重算 = 这一帧别把刚 dismiss 的候选弹回来。
pub(in crate::ui) fn action_for_key(
    key: &egui::Key,
    popup_visible: bool,
    completion_on: bool,
) -> Option<(Action, bool, bool)> {
    if !completion_on {
        return None;
    }
    if popup_visible {
        match key {
            // Move 也要阻止重算：否则刚选中的那一项会被同帧 update 按
            // Preselect 重置回 Nowhere，按 ↓ 看起来毫无反应。
            egui::Key::ArrowUp => return Some((Action::Move(-1), true, true)),
            egui::Key::ArrowDown => return Some((Action::Move(1), true, true)),
            egui::Key::Tab => return Some((Action::Accept, false, true)),
            egui::Key::Escape => return Some((Action::Dismiss, true, true)),
            _ => {}
        }
    }
    // 左右键意味着光标可能不在行尾，关掉弹层，但键本身仍要转发给 shell。
    if matches!(key, egui::Key::ArrowLeft | egui::Key::ArrowRight) {
        return Some((Action::Dismiss, true, false));
    }
    None
}

/// 把补全意图落到状态上。Accept 的后缀并进 `key_bytes`，与真实按键走同一条
/// write/feed 路径——录制器、同步输入也都一致。
pub(in crate::ui) fn apply_action(
    action: Action,
    owner: egui::Id,
    state: &mut CompletionState,
    key_bytes: &mut Vec<u8>,
) {
    match action {
        Action::Move(d) => state.move_selection(d),
        Action::Dismiss => state.dismiss(),
        Action::Accept => {
            if let Some(insert) = state.accept(owner) {
                state.dismiss();
                key_bytes.extend_from_slice(insert.as_bytes());
            }
        }
    }
}

/// 输入变化后重算候选。
pub(in crate::ui) fn recompute(
    owner: egui::Id,
    line: &str,
    history: &[(String, u64)],
    quick: &[String],
    settings: &term_core::completion::Settings,
    now: u64,
    state: &mut CompletionState,
) {
    let hist: Vec<(&str, u64)> = history.iter().map(|(s, ts)| (s.as_str(), *ts)).collect();
    let q: Vec<&str> = quick.iter().map(String::as_str).collect();
    let candidates = term_core::completion::complete(term_core::completion::Input {
        line,
        history: &hist,
        quick: &q,
        settings,
        now,
    });
    state.update(owner, candidates, settings.preselect);
}

/// 键盘漏斗里补全可以消费的三种意图。`Move` 的增量是 ±1。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum Action {
    Move(i32),
    Accept,
    Dismiss,
}

/// 一帧的候选状态。纯数据 + 纯方法，不认识 PTY，也不读墙钟。
#[derive(Clone, Debug, Default)]
pub(in crate::ui) struct CompletionState {
    candidates: Vec<Candidate>,
    selected: Option<usize>,
    owner: Option<egui::Id>,
}

impl CompletionState {
    /// 绑定标签：换标签时旧候选立即失效。
    pub(in crate::ui) fn bind(&mut self, owner: egui::Id) {
        if self.owner != Some(owner) {
            self.candidates.clear();
            self.selected = None;
            self.owner = Some(owner);
        }
    }

    /// 本标签此刻是否有可见候选。
    pub(in crate::ui) fn is_visible(&self, owner: egui::Id) -> bool {
        self.owner == Some(owner) && !self.candidates.is_empty()
    }

    /// 用新算出的候选替换整表，并按 `Preselect` 决定高亮项。
    pub(in crate::ui) fn update(
        &mut self,
        owner: egui::Id,
        candidates: Vec<Candidate>,
        preselect: Preselect,
    ) {
        self.bind(owner);
        self.selected = term_core::completion::preselect_index(&candidates, preselect);
        self.candidates = candidates;
    }

    /// 关闭弹层（Esc / 方向键 / 失去焦点）。
    pub(in crate::ui) fn dismiss(&mut self) {
        self.candidates.clear();
        self.selected = None;
    }

    /// 上下移动高亮；`Nowhere` 起步时向下第一下选第 0 项。
    pub(in crate::ui) fn move_selection(&mut self, delta: i32) {
        if self.candidates.is_empty() {
            return;
        }
        let last = self.candidates.len() - 1;
        let next = match self.selected {
            None if delta > 0 => 0,
            None => last,
            Some(i) => (i as i32 + delta).clamp(0, last as i32) as usize,
        };
        self.selected = Some(next);
    }

    /// 当前接受的后缀。没有候选时返回 `None`，调用方据此把 Tab 透传给 shell。
    pub(in crate::ui) fn accept(&self, owner: egui::Id) -> Option<String> {
        if !self.is_visible(owner) {
            return None;
        }
        let i = self.selected.unwrap_or(0);
        self.candidates.get(i).map(|c| c.insert.clone())
    }

    /// 供绘制与测试读取，不暴露可变借用。
    pub(in crate::ui) fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    pub(in crate::ui) fn selected(&self) -> Option<usize> {
        self.selected
    }
}

/// 弹层尺寸。宽度取终端区的一小部分，长文案省略——补全列表不是主视图。
const POPUP_W: f32 = 300.0;
const ROW_H: f32 = 18.0;
const PAD: f32 = 6.0;
const ANCHOR_PAD: f32 = 12.0;

/// 只在真正可见时画的入口，让终端面板只留一行调用。
pub(in crate::ui) fn paint_if_visible(
    ui: &Ui,
    area: Rect,
    state: &CompletionState,
    enabled: bool,
    owner: egui::Id,
    list_height: usize,
) {
    if enabled && state.is_visible(owner) {
        paint(ui, area, state, list_height);
    }
}

/// 在终端区右下角画候选列表。`state` 必须是本标签的那一份。
pub(in crate::ui) fn paint(ui: &Ui, area: Rect, state: &CompletionState, list_height: usize) {
    if state.candidates.is_empty() {
        return;
    }
    let n = state.candidates.len().min(list_height.max(1));
    let h = (n as f32 * ROW_H + PAD * 2.0).min(area.height().max(ROW_H));
    let w = POPUP_W.min(area.width().max(40.0));
    let min = Pos2::new(
        area.right() - ANCHOR_PAD - w,
        area.bottom() - ANCHOR_PAD - h,
    );
    let rect = Rect::from_min_size(min, Vec2::new(w, h));
    let p = ui.painter_at(rect.expand(2.0));

    p.rect_filled(rect, Rounding::same(4.0), chrome(CHROME_SCALE.dialog));
    p.rect_stroke(rect, Rounding::same(4.0), Stroke::new(1.0, chrome(CHROME_SCALE.border)));

    let font = egui::TextStyle::Body.resolve(ui.style());
    for (i, c) in state.candidates.iter().take(n).enumerate() {
        let row = Rect::from_min_size(
            Pos2::new(rect.min.x + PAD, rect.min.y + PAD + i as f32 * ROW_H),
            Vec2::new(rect.width() - PAD * 2.0, ROW_H),
        );
        if Some(i) == state.selected {
            p.rect_filled(row, Rounding::ZERO, chrome(CHROME_SCALE.selected));
        }
        let label = format!("{}  {}", source_tag(c.source), c.display);
        p.text(
            row.left_top(),
            Align2::LEFT_TOP,
            label,
            font.clone(),
            chrome(if Some(i) == state.selected {
                CHROME_SCALE.text_strong
            } else {
                CHROME_SCALE.text
            }),
        );
    }
}

fn source_tag(source: Source) -> &'static str {
    match source {
        Source::Command => "cmd",
        Source::Option => "opt",
        Source::Parameter => "arg",
        Source::History => "his",
        Source::Quick => "qck",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(display: &str, insert: &str) -> Candidate {
        Candidate {
            display: display.into(),
            insert: insert.into(),
            source: Source::Command,
            score: 1.0,
        }
    }

    #[test]
    fn new_state_is_invisible() {
        let s = CompletionState::default();
        assert!(!s.is_visible(egui::Id::new("tab-a")));
        assert_eq!(s.accept(egui::Id::new("tab-a")), None);
    }

    #[test]
    fn switching_owner_discards_candidates() {
        let mut s = CompletionState::default();
        let a = egui::Id::new("a");
        s.update(a, vec![candidate("status", "atus")], Preselect::First);
        assert!(s.is_visible(a));
        s.bind(egui::Id::new("b"));
        assert!(!s.is_visible(a));
    }

    #[test]
    fn first_down_selects_zero_and_up_from_nowhere_selects_last() {
        let mut s = CompletionState::default();
        let a = egui::Id::new("a");
        s.update(
            a,
            vec![candidate("a", "a"), candidate("b", "b")],
            Preselect::Nowhere,
        );
        s.move_selection(1);
        assert_eq!(s.selected(), Some(0));
        s.dismiss();
        s.update(
            a,
            vec![candidate("a", "a"), candidate("b", "b")],
            Preselect::Nowhere,
        );
        s.move_selection(-1);
        assert_eq!(s.selected(), Some(1));
    }

    #[test]
    fn move_selection_clamps_at_bounds() {
        let mut s = CompletionState::default();
        let a = egui::Id::new("a");
        s.update(a, vec![candidate("a", "a")], Preselect::First);
        s.move_selection(9);
        assert_eq!(s.selected(), Some(0));
        s.move_selection(-9);
        assert_eq!(s.selected(), Some(0));
    }

    #[test]
    fn accept_returns_only_the_suffix() {
        let mut s = CompletionState::default();
        let a = egui::Id::new("a");
        s.update(a, vec![candidate("git status", "atus")], Preselect::First);
        assert_eq!(s.accept(a).as_deref(), Some("atus"));
        s.dismiss();
        assert_eq!(s.accept(a), None);
    }

    #[test]
    fn preselect_policy_is_applied_on_update() {
        let mut s = CompletionState::default();
        let a = egui::Id::new("a");
        let list = vec![candidate("a", "a"), candidate("b", "b")];
        s.update(a, list.clone(), Preselect::Nowhere);
        assert_eq!(s.selected(), None);
        s.update(a, list, Preselect::Best);
        assert_eq!(s.selected(), Some(0));
    }

    #[test]
    fn source_tags_are_distinct() {
        let tags: Vec<_> = [
            Source::Command,
            Source::Option,
            Source::Parameter,
            Source::History,
            Source::Quick,
        ]
        .into_iter()
        .map(source_tag)
        .collect();
        let mut uniq = tags.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(tags.len(), uniq.len());
    }

    #[test]
    fn tab_is_only_consumed_when_popup_is_visible() {
        assert_eq!(
            action_for_key(&egui::Key::Tab, true, true),
            Some((Action::Accept, false, true))
        );
        assert_eq!(action_for_key(&egui::Key::Tab, false, true), None);
        assert_eq!(action_for_key(&egui::Key::Tab, true, false), None);
    }

    #[test]
    fn navigation_consumes_and_blocks_recompute() {
        for key in [egui::Key::ArrowUp, egui::Key::ArrowDown] {
            let (action, blocked, consume) =
                action_for_key(&key, true, true).expect("弹层可见时应归补全");
            assert!(matches!(action, Action::Move(_)));
            assert!(blocked, "不能让同帧 update 把高亮重置掉");
            assert!(consume, "↑↓ 不该写进 PTY");
        }
    }

    #[test]
    fn left_arrow_dismisses_but_still_reaches_the_shell() {
        let (action, blocked, consume) = action_for_key(&egui::Key::ArrowLeft, true, true)
            .expect("方向键总是要关弹层");
        assert_eq!(action, Action::Dismiss);
        assert!(blocked);
        assert!(!consume, "左右键本身仍要转发给 shell");
    }

    #[test]
    fn typing_then_accepting_produces_the_whole_command() {
        let mut state = CompletionState::default();
        let owner = egui::Id::new("pane");
        let typed = "git st";
        let mut bytes = typed.as_bytes().to_vec();
        let candidate = candidate("git status", "atus");
        state.update(owner, vec![candidate], Preselect::First);
        let insert = state.accept(owner).expect("有候选");
        bytes.extend_from_slice(insert.as_bytes());
        assert_eq!(String::from_utf8(bytes).unwrap(), "git status");
    }
}
