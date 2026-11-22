//! 折叠族九条的决策与落地（`ui/effects.rs` 的子模块）。
//!
//! # 为什么单独一个文件
//!
//! 折叠这九条命令一共约 280 行（含注释），堆进 `effects.rs` 会把它顶过 4j 的
//! 500 代码行上限——那条门禁的口径是「新代码别再往同一个文件里堆」，而折叠
//! 恰好是一个**有明确边界**的关注点：它的全部输入只有四个值（输入行、光标行、
//! 现有集合、视口区间），没有一个字节来自 PTY 或窗口系统。
//!
//! # 两层分工
//!
//! - [`decide_fold`] 是**纯函数**：不碰 `Tab`、不碰 `egui`、不读盘。所以「点了
//!   之后折没折对」可以在没有任何会话的测试里逐条验（见 `fold_tests.rs`）。
//! - [`TerminalWindow::apply_fold`] 只做三件必须碰窗口的事——把新集合写回标签、
//!   把视口挪到折叠头、请一次重绘。
//!
//! 这么分不是为了好看：判断全在纯函数里，才能在不启动 GUI 的前提下钉住
//! 「最后一条输入之后不折」「已经折过的块再折一次不是错误」「视口没有下一个
//! 折叠头时不要假装跳过去了」这些只会在真机上出现一次的分支。

use crate::ui::TerminalWindow;
use term_app::folds::{block_at, blocks_from_inputs, Folds, StructureError};
use term_app::vocab::FoldCmd;
use term_core::coords::{history_to_view, view_to_history, HistoryRow, ViewRange, ViewRow};

impl TerminalWindow {
    /// 折叠族九条（`View ▸ Folds` 六条 + `Text ▸ Margin Fold` + `Goto ▸ Fold` 两条）
    /// 的**唯一落地点**。
    ///
    /// 分工：决策在纯函数 [`decide_fold`]（输入是输入行 / 光标行 / 视口区间 /
    /// 现有集合，输出一份新的 `Folds` + 跳转目标 + 一句提示），这里只做三件
    /// 必须碰窗口的事——把新集合写回标签、把视口挪到折叠头、请一次重绘。
    /// 这么分不是为了好看：`decide_fold` 没有 `Tab` 也能跑，于是「点了之后折没
    /// 折对」可以在没有 PTY、没有窗口的测试里逐条验（见 `mod tests`）。
    ///
    /// 没有活动标签时静默——与 `toggle_hex_mode` 同一条约定：菜单在没有标签的
    /// 窗口里点得到，那时候什么也不该发生。
    pub(in crate::ui) fn apply_fold(&mut self, cmd: FoldCmd) {
        // 借用分两段：第一段独占 `tab` 把决策做完并落地，第二段才碰
        // `self.log_note` / `self.ctx`（它们挂在窗口上，与 `self.tabs` 互斥）。
        let (changed, jumped, note) = {
            let active = self.active_slot();
            let Some(tab) = self.tabs.get_mut(active) else {
                return;
            };
            let inputs = tab.fold_inputs();
            let view = tab.view.view_range();
            let history = tab.snapshot.history as u64;
            let decision = decide_fold(cmd, &inputs, tab.cursor_abs_row(), &tab.folds, view, history);

            // 「变了没有」由集合本身回答，不由命令种类猜：`FoldAll` 在已经全折起来
            // 时什么都没变，`Unfold` 在平地上也是。判据放在这里，下面两处
            // （`last_request` 与重绘）就只有一个真相。
            let changed = decision.folds != tab.folds;
            let jumped = decision.jump.is_some();
            tab.folds = decision.folds;
            if let Some(row) = decision.jump {
                // `jump_to_row` 自己按内核报告的历史上限 clamp，所以这里不夹第二次。
                let max = tab.snapshot.history as i64;
                tab.viewport.jump_to_row(row, max);
            }
            if changed || jumped {
                // 折叠改了「这一屏需要哪些行」：`ensure_view` 在折叠非空时走
                // `folded_view`，它算出的 `needed` 比 `viewport.rows(rows)` 更长
                // （向上多要几行补齐被折掉的行数）。`last_request` 的判据是
                // 「上次要的 range/version 与这次算出来的一样吗」，所以 needed 变了
                // 它本来就会重发。
                //
                // 显式置空是**不把正确性押在那条推论上**：`GotoPreviousFold` 的
                // 跳转若落在当前窗口之内（`needed` 一个值都没变），refold 与否都不会
                // 触发重发，而这一帧的 `tab.screen` 还是跳转前那份——视口变了、
                // 图没变的画面要等下一次版本变化才纠正。置空的代价只是多一次上行
                // 请求（幂等，`slice_view` 一回就是同一段），比「画面和状态对不上」
                // 便宜得多。
                tab.last_request = None;
            }
            (changed, jumped, decision.note)
        };
        if let Some(note) = note {
            // 只覆盖、不清空：`log_note` 是窗口级的一次性反馈通道，别的功能
            // （日志、SFTP、传输）正在用。
            self.log_note = Some(note.to_string());
        }
        if changed || jumped {
            self.ctx.request_repaint();
        }
    }
}

// ── 折叠：不依赖窗口的那一半 ───────────────────────────────────────────────
//
// 两个东西（`view_span_in_history` / `decide_fold`）刻意做成自由函数而不是
// `TerminalWindow` 的方法：折叠的全部判断——折哪个块、跳哪一行、什么时候该说
// 一句话——都能只用「输入行 + 光标行 + 视口区间 + 现有集合」四个值算出来，
// 而它们全都可以在没有 PTY、没有 `egui` 的测试里构造。
// `apply_fold` 只是把结果写回标签的那三行。
//
// 光标那一个值来自 `Tab::cursor_abs_row`（`tab.rs`），本模块不另抄一份换算：
// 两份 `history + min(cursor.0, rows - 1)` 迟早会漂，而漂了的表现是「Fold 折掉
// 了别的命令的输出」。

/// 视口（视口行半开区间）在当前历史长度下覆盖到的**历史行闭区间**。
///
/// 折叠集合存的是历史行，视口说的是视口行（`0` = 贴底时的屏首、负数是回看），
/// 换算只走 `term_core::coords` 这一个出口——本仓不允许在 UI 里再写一份
/// `row - history`。
///
/// 返回 `None` 的三种情形都是一句话的事：视口为空（`rows == 0`）、
/// 尾行减一溢出、以及首尾落到历史第一条之前（那样「视口里看得见的行」根本不存在）。
fn view_span_in_history(view: ViewRange, history: u64) -> Option<(HistoryRow, HistoryRow)> {
    let start = view_to_history(view.start(), history)?;
    let last = view.end().get().checked_sub(1)?;
    let end = view_to_history(ViewRow::new(last), history)?;
    if end < start {
        return None;
    }
    Some((start, end))
}

/// 一条折叠命令的决策结果。
///
/// 三个字段各自回答一个「谁来看」的问题：
///
/// - `folds`：写回 `Tab.folds`（渲染侧 `setup::folded_view` 与 margin 折叠列读它）；
/// - `jump`：视口行，交给 `Viewport::jump_to_row`（只有 `Previous` / `Next` 会给）；
/// - `note`：一句话，写进 `log_note` 在状态栏显示。
///
/// `note` 存在不是为了好看：折叠的大多数命令在「没东西可折」时**什么都不该发生**，
/// 而一个点了没反应的菜单项与一个坏掉的菜单项在屏幕上长得一模一样。所以每条
/// 空操作都要说清是哪一种空（没有块 / 已经折过 / 这个方向没有折叠头）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) struct FoldDecision {
    pub folds: Folds,
    pub jump: Option<i64>,
    pub note: Option<&'static str>,
}

/// 纯决策：一条折叠命令作用到「输入行 + 光标行 + 现有集合 + 视口」上的结果。
///
/// 这个函数不碰 `Tab`、不碰 PTY、不碰 `egui`，也不读盘，所以它的每一条分支都能
/// 被单测钉住（见 `mod tests` 里那一组）。
///
/// # 语义来源
///
/// 用户定死的口径：「折叠点是按照用户输入计算的，折叠的是两次输入之间的内容」。
/// 落地在 `folds::blocks_from_inputs`：相邻两个输入 `a < b` → 块 `[a, b - 1]`，
/// `a` 是折叠头（命令行本身，永远可见），`b == a + 1`（中间没有输出）不产生块，
/// 最后一条输入之后不折。本函数只是把九条命令各自映射到这套块集合上的操作，
/// **不自己算任何边界**——算边界是领域层的事，这里再算一遍就是第二份真相。
pub(in crate::ui) fn decide_fold(
    cmd: FoldCmd,
    inputs: &[HistoryRow],
    cursor: Option<HistoryRow>,
    folds: &Folds,
    view: ViewRange,
    history: u64,
) -> FoldDecision {
    // 从克隆改起：九条命令里有七条是「在现有集合上增删」，只有 `UnfoldAll` 是清空。
    // 用一份副本 + 最后整体交回去，调用方只要比较新旧两值就知道「到底变了没有」，
    // 不必让每条分支自己维护一个 `changed` 标志（那样最容易在 `add` 失败的分支里
    // 忘了置 false）。
    let mut next = folds.clone();
    let mut jump: Option<i64> = None;
    let mut note: Option<&'static str> = None;

    match cmd {
        FoldCmd::Fold => {
            if let Some(block) = cursor.and_then(|row| block_at(inputs, row)) {
                // 已经折过：`add` 对任何相交都返回同一个错误，而这里唯一可能的
                // 相交就是「这个块已经在集合里」。**这是合法空操作，不是错误**，
                // 所以吞掉它并说明白——绝不能 `unwrap`，也不能当成失败报错。
                if next.add(block) == Err(StructureError::OverlappingFoldRegions) {
                    note = Some("这个块已经折起来了");
                }
            } else {
                // 两种情形：光标在最后一条输入之后（正在产生的输出，不折）、
                // 或光标在第一条输入之前（那里还没有块）。
                note = Some("光标行不在任何折叠块里");
            }
        }
        FoldCmd::Unfold => match cursor {
            Some(row) => {
                if !next.remove_containing(row) {
                    note = Some("光标行上没有折起来的块");
                }
            }
            None => note = Some("光标行不在任何折叠块里"),
        },
        // `Inside` = 「视口内」（本仓的块平坦不嵌套，见 `FoldCmd::FoldInside`）。
        // 交集的判据是闭区间：块 `[a, b-1]` 与视口 `[lo, hi]` 有交集当且仅当
        // `a <= hi && lo <= b-1`。
        FoldCmd::FoldInside => match view_span_in_history(view, history) {
            Some((lo, hi)) => {
                let mut hit = 0usize;
                for block in blocks_from_inputs(inputs) {
                    if block.end() < lo || hi < block.start() {
                        continue;
                    }
                    hit += 1;
                    // 视口里已经折好的块再折一次也是空操作，不是错误；这里连日志
                    // 都不记——「视口内 3 个块，其中 1 个早折好了」没有可操作性。
                    let _ = next.add(block);
                }
                if hit == 0 {
                    note = Some("视口里没有可折的块");
                }
            }
            None => note = Some("视口里没有可见的行"),
        },
        FoldCmd::UnfoldInside => match view_span_in_history(view, history) {
            Some((lo, hi)) => {
                if next.remove_overlapping(lo, hi) == 0 {
                    note = Some("视口里没有折起来的块");
                }
            }
            None => note = Some("视口里没有可见的行"),
        },
        FoldCmd::FoldAll => {
            let blocks = blocks_from_inputs(inputs);
            if blocks.is_empty() {
                note = Some("没有可折的块");
            } else {
                for block in &blocks {
                    // 已经折起来的块在这里被跳过（`Err`），剩下的逐个插入。
                    let _ = next.add(block.clone());
                }
                // 一个都没插进去 = 全都已经折好了。这句必须说：否则第二次点
                // `Fold All` 看起来完全像是命令坏了。
                if next == *folds {
                    note = Some("所有块都已经折起来了");
                }
            }
        }
        // 清空不需要任何前置条件，也不留提示：用户点「全部展开」看到的就是
        // 「折着的东西全摊开了」，没有折叠时本来也没有可说的话。
        FoldCmd::UnfoldAll => {
            next.clear();
        }
        // `Text ▸ Margin Fold`：同一个入口一去一回。已经落在某个块里（含头行——
        // 光标就停在命令行上是常见情形）就解开，否则折起来。
        FoldCmd::Toggle => match cursor {
            Some(row) => {
                if next.containing(row).is_some() {
                    next.remove_containing(row);
                } else if let Some(block) = block_at(inputs, row) {
                    if next.add(block) == Err(StructureError::OverlappingFoldRegions) {
                        note = Some("这个块已经折起来了");
                    }
                } else {
                    note = Some("光标行不在任何折叠块里");
                }
            }
            None => note = Some("光标行不在任何折叠块里"),
        },
        // 鼠标点折叠列：切换那一个头对应的块。头行由点击的屏幕行换算成绝对行号，
        // 领域层只按这个行号找块——与 `Toggle` 的区别是「不依赖光标在哪」。
        FoldCmd::ToggleAt(row) => {
            if next.regions().iter().any(|r| r.start() == row) {
                next.remove_containing(row);
            } else if let Some(block) = block_at(inputs, row) {
                if next.add(block) == Err(StructureError::OverlappingFoldRegions) {
                    note = Some("这个块已经折起来了");
                }
            } else {
                note = Some("这一行不是折叠头");
            }
        }
        // 跳转的基准是**视口首行**，不是光标：终端里没有可移动的光标（它属于 PTY），
        // 于是「跳到下一个折叠」只能是「把视口挪到那个折叠头上」。
        // 用视口首行而不是视口末尾，是为了让「上一个 / 下一个」与用户的直觉一致：
        // 屏幕上最上面那一行之上就是「上一个」，之下是「下一个」。
        FoldCmd::Previous | FoldCmd::Next => {
            let top = view_to_history(view.start(), history);
            let target = top.and_then(|top| match cmd {
                FoldCmd::Previous => next.prev_header_before(top),
                _ => next.next_header_after(top),
            });
            match target {
                Some(row) => jump = Some(history_to_view(row, history).get()),
                // 两个方向的「到头了」必须分开说：`Previous` 停在第一个折叠头上
                // 再按会得到「没有折叠块」，而那时屏幕上明明折着好几块——用户
                // 会以为命令坏了。所以集合非空时说的是「到头了」。
                None => {
                    note = Some(if next.is_empty() {
                        "没有折叠块"
                    } else {
                        "这个方向没有折叠头了"
                    })
                }
            }
        }
    }

    FoldDecision {
        folds: next,
        jump,
        note,
    }
}

// 决策的断言单独一个文件：29 条用例连同注释塞进来，会让这个文件读起来像两件事
// （和 `zebra_scope_tests.rs` / `tmux_feed_tests.rs` 同一套做法）。
#[cfg(test)]
#[path = "fold_tests.rs"]
mod tests;
