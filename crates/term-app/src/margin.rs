//! 终端 margin（左侧边栏）的开关状态与五条命令。
//!
//! 这一层只回答「哪一列是开的」，一个像素都不碰：列宽、字号、颜色在投影层
//! （`apps/desktop/src/ui/terminal/margin.rs`），行号 / 时间戳 / 续行标记的数据来自
//! 内核快照（`GridSnapshot::row_times` / `wraps` / `blocks`）。
//!
//! **五列全是真的**（`Fold` 列 2026-11-18 接线）：
//! - `Blank`：边栏留白。关掉就贴边（效果可见，不是空转 bool）；
//! - `Number`：行号（绝对行号，来源见内核 `cmd_marks` 同一套坐标）；
//! - `Symbol`：`›` 提示符行 / `~` 自动换行续行（数据来自 OSC 133 与 `wrap_rows`）；
//! - `Timestamp`：`[HH:MM:SS]`（时刻由宿主递进内核，见 `Terminal::stamp_row`）；
//! - `Fold`：折叠标记列（命令 `Text.MarginFoldColumn`）。它画的是**折叠头**——
//!   判定来自 `Tab.folds` 的 `FoldRegion::start()`，与 `setup::folded_view` 藏行
//!   用的是同一个集合、同一套**绝对历史行号**，所以边栏与正文不会各说一套。
//!
//! # 两个带 `Margin` 的 ID 不是一回事（2026-11-14 订正，2026-11-18 补全）
//!
//! - `Text.MarginFold`：折的是**光标所在的块**（`Action::Fold(FoldCmd::Toggle)`）；
//! - `Text.MarginFoldColumn`：画不画**折叠标记列**（`MarginPart::Fold`）。
//!
//! 前者不是列开关，所以 `margin::parse` 收到它时必须仍然是 `None`；
//! 后者才是本模块认的那一条。两个 ID 差一个词，混起来的表现是「点菜单折了块，
//! 但边栏出现/消失一列」——两件事同时发生，用户没法判断自己开的是哪一个。
//!
//! （这里刻意不把两个 ID 写成带双引号的字面量：`scripts/verify.sh` 的 4e
//! 「命令 ID 只在数据层」会扫本文件 `#[cfg(test)]` 之前的每一行，注释里的引号
//! 同样算硬编码。测试里怎么引都行，`#[cfg(test)]` 之后的部分不扫。）
//!
//! 也正因为如此，本模块的 reducer 只翻五位 `bool`，一个 `Effect` 都不产。

use crate::state::ViewState;

pub use crate::vocab::MarginPart;

pub use crate::vocab::MarginAction;

pub use crate::state::MarginState;

impl Default for MarginState {
    fn default() -> Self {
        Self {
            blank: true,
            number: true,
            fold: false,
            symbol: true,
            timestamp: false,
        }
    }
}

impl MarginState {
    /// 这一列开没开。`Fold` 恒为 `false`——没有任何命令 ID 映射到它（见模块头）。
    pub fn get(&self, part: MarginPart) -> bool {
        match part {
            MarginPart::Blank => self.blank,
            MarginPart::Number => self.number,
            MarginPart::Fold => self.fold,
            MarginPart::Symbol => self.symbol,
            MarginPart::Timestamp => self.timestamp,
        }
    }

    /// 有没有**任何一列**要画。全关时投影层连边栏宽度都不该留（否则是一片死白）。
    pub fn any_column(&self) -> bool {
        self.number || self.symbol || self.timestamp || self.fold
    }
}


/// 命令 ID → 本能力的动作。认不出的 ID（含 `Text.MarginFold`，见模块头）返回 `None`。
///
/// 2026-11-12：实现只有一份，在 [`crate::parse`]（入口层）。这里保留同名入口，
/// 一是因为「每个能力自带字符串入口」是这个 crate 对外的约定，二是因为
/// `apps/desktop` 有直接调 `term_app::log::parse` 的地方——把路径改掉没有架构价值。
pub fn parse(id: &str) -> Option<MarginAction> {
    crate::parse::margin_parse(id)
}

/// 纯 reducer：只翻一位 `bool`，没有任何副作用。
///
/// 五位都翻得动，因为五列都有读者（`Fold` 列自从有了
/// `apps/desktop/src/ui/terminal/margin.rs` 的折叠头绘制之后才允许翻——
/// 在那之前 `Fold` 到这里是 `return`，理由正是「状态位必须和画得出来对齐」。
pub fn reduce(state: &mut ViewState, action: MarginAction) {
    let MarginAction::Toggle(part) = action;
    let slot = match part {
        MarginPart::Blank => &mut state.margin.blank,
        MarginPart::Number => &mut state.margin.number,
        MarginPart::Fold => &mut state.margin.fold,
        MarginPart::Symbol => &mut state.margin.symbol,
        MarginPart::Timestamp => &mut state.margin.timestamp,
    };
    *slot = !*slot;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_keep_the_margin_narrow() {
        let m = MarginState::default();
        assert!(m.blank && m.number && m.symbol);
        assert!(!m.timestamp, "10 格的时间戳列默认不占屏");
        assert!(!m.fold);
        assert!(m.any_column());
    }

    #[test]
    fn every_wired_id_toggles_exactly_one_column() {
        for (id, part) in [
            ("Text.MarginBlank", MarginPart::Blank),
            ("Text.MarginNumber", MarginPart::Number),
            ("Text.MarginFoldColumn", MarginPart::Fold),
            ("Text.MarginSymbol", MarginPart::Symbol),
            ("Text.MarginTimestamp", MarginPart::Timestamp),
        ] {
            let mut state = ViewState::default();
            let before = state.margin;
            let action = parse(id).unwrap_or_else(|| panic!("{id} 必须已接线"));
            reduce(&mut state, action);
            // 只有一格变了。
            let changes = [
                before.blank != state.margin.blank,
                before.number != state.margin.number,
                before.fold != state.margin.fold,
                before.symbol != state.margin.symbol,
                before.timestamp != state.margin.timestamp,
            ]
            .iter()
            .filter(|c| **c)
            .count();
            assert_eq!(changes, 1, "{id} 应只翻一列");
            assert_eq!(state.margin.get(part), !before.get(part));
            // 再翻一次回到原状（开关必须有回程）。
            reduce(&mut state, action);
            assert_eq!(state.margin, before);
        }
    }

    #[test]
    fn the_fold_column_and_the_fold_command_are_two_ids() {
        // 两个 ID 只差一个 `Column`，混起来的表现是「点一下折了块，边栏又冒出一列」。
        // 三条都要钉住：
        //
        // 1) `Text.MarginFold` 仍然可点（它是折叠族第 9 条，折的是光标所在块）；
        // 2) `Text.MarginFold` **不许**从 `margin::parse` 出来（它不是列开关）；
        // 3) `Text.MarginFoldColumn` 才是列开关，且翻得动那一位。
        assert!(
            crate::capability::is_implemented("Text.MarginFold"),
            "Text.MarginFold 必须已接线"
        );
        assert_eq!(parse("Text.MarginFold"), None, "它不是 margin 的列开关");

        let mut state = ViewState::default();
        let before = state.margin.fold;
        reduce(
            &mut state,
            parse("Text.MarginFoldColumn").expect("列开关必须已接线"),
        );
        assert_eq!(state.margin.fold, !before, "折叠列必须真的翻得动");
        assert!(state.margin.any_column(), "开了折叠列，边栏就得留宽度");
    }

    #[test]
    fn closing_every_column_leaves_no_margin() {
        let mut state = ViewState::default();
        // 默认开的是「行号 + 符号」；时间戳本来就关着，别再翻它。
        assert!(!state.margin.timestamp);
        for id in ["Text.MarginNumber", "Text.MarginSymbol"] {
            reduce(&mut state, parse(id).unwrap());
        }
        assert!(!state.margin.any_column(), "三列全关时投影不该留边栏宽度");
    }
}
