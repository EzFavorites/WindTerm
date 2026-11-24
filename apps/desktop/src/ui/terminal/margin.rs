//! 终端左侧 margin 边栏：时间戳 / 行号 / 折叠标记 / 符号四列（2026-11-07，折叠列 2026-11-18）。
//!
//! 为什么单独一个模块：`pane.rs` 已经贴着 450 行上限，而边栏是**纯绘制**
//! ——它不读也不写任何状态，只把「哪几列开着」（`app::margin::MarginState`）、
//! 内核快照里的三张表（`row_times` / `wraps` / `blocks`）以及**折叠集合**
//! （`tab.folds`）铺成像素。
//!
//! 坐标口径只有一条：**绝对行号**。视口坐标（负数 = 回看）在进这一层前就换算完，
//! 因为三张表与折叠区间全是绝对的，混着用一定错一行。折叠头判定尤其如此：
//! `FoldRegion` 的两端是 `HistoryRow`，边栏若拿窗口下标去比，折叠一开整列记号
//! 就会跟着错位（正文由 `setup::folded_view` 跳过被折行，行号不再连续）。
//!
//! 列宽一律以「单元格」为单位，唯一的像素换算是 `cell_w`——字体变了列宽跟着变，
//! 不会出现「时间戳被行号压住」这种只有某个字号下才有的 bug。

use crate::ui::*;
use term_app::folds::{block_at, Folds};
use term_app::margin::MarginState;
use term_core::coords::HistoryRow;
use term_core::grid::{GridSnapshot, ScreenImage};

/// 左边距（单元格）。`Margin Blank` 关掉时归零。
const PAD_L: f32 = 0.5;
/// 右边距：边栏与终端正文之间的呼吸位。
const PAD_R: f32 = 1.0;
/// `[HH:MM:SS]` 正好 10 个字符。
const STAMP_COLS: f32 = 10.0;
/// 列与列之间的固定间隔。
const GAP: f32 = 1.0;
/// 行号与符号之间的间隔。
///
/// 比 `GAP` 宽一档：参考截图里这一段是 2.5 格左右，行号紧贴符号会让人以为
/// `›` 是行号的一部分（3 位数以上的行号尤其明显）。
const SYMBOL_GAP: f32 = 2.0;
/// 符号列（`›` / `~`）占一格。
const SYMBOL_COLS: f32 = 1.0;
/// 折叠标记列（`▸`）占一格。与符号列同宽，开一列就是多一格。
const FOLD_COLS: f32 = 1.0;
/// 行号列最窄 3 格：`9` 后面还要留出 `10`、`100` 的位置，否则每进一位整列都会跳。
const MIN_NUMBER_COLS: usize = 3;
/// 行号列最宽 6 格（99 万行以后就让它自己截断，不再把正文挤没）。
const MAX_NUMBER_COLS: usize = 6;

/// 折叠列鼠标修饰键 → 折叠命令（纯函数，可单测）。
///
/// - Ctrl/Cmd：全部块的开关（无折 → FoldAll，已有折 → UnfoldAll）。
/// - Alt（递归）/ Shift（同类型）：本仓块平坦无嵌套、无类型概念，原版语义没有
///   对应物——不假装支持，退回单块 ToggleAt。
fn fold_cmd_for_modifiers(
    mods: egui::Modifiers,
    folds_empty: bool,
    abs: term_core::coords::HistoryRow,
) -> term_app::vocab::FoldCmd {
    use term_app::vocab::FoldCmd;
    if mods.ctrl || mods.command {
        if folds_empty { FoldCmd::FoldAll } else { FoldCmd::UnfoldAll }
    } else {
        FoldCmd::ToggleAt(abs)
    }
}

/// 一次算好、逐行复用：每帧只构造一次，行循环里不再做几何判断。
pub(in crate::ui) struct MarginMetrics {
    /// 整个边栏占多宽（像素）。0 = 一列都没开，投影层连预留都不留。
    pub width: f32,
    stamp_x: f32,
    num_right: f32,
    fold_center: f32,
    fold_on: bool,
    sym_center: f32,
}

fn number_cols(last_line: i64) -> usize {
    let n = last_line.max(0) as u64 + 1;
    let digits = n.to_string().len();
    digits.clamp(MIN_NUMBER_COLS, MAX_NUMBER_COLS)
}

impl MarginMetrics {
    /// `last_line` 是窗口里最后一个**绝对行号**，用来定行号列宽度。
    pub(in crate::ui) fn new(m: &MarginState, cell_w: f32, last_line: i64) -> Self {
        if !m.any_column() {
            return Self {
                width: 0.0,
                stamp_x: 0.0,
                num_right: 0.0,
                fold_center: 0.0,
                fold_on: false,
                sym_center: 0.0,
            };
        }
        let mut x = if m.blank { PAD_L * cell_w } else { 0.0 };
        let stamp_x = x;
        if m.timestamp {
            x += (STAMP_COLS + GAP) * cell_w;
        }
        let digits = if m.number { number_cols(last_line) } else { 0 };
        let num_right = x + digits as f32 * cell_w;
        if m.number {
            x += digits as f32 * cell_w + SYMBOL_GAP * cell_w;
        }
        // 折叠列夹在行号与符号之间：它标的是「这一行是折叠头」，
        // 与行号同属「这一行是谁」的列，而符号列讲的是「这一行怎么来的」
        // （提示符 / 续行）。顺序换了会让两列语义混在一起看。
        let fold_center = x + FOLD_COLS * cell_w * 0.5;
        if m.fold {
            x += (FOLD_COLS + GAP) * cell_w;
        }
        let sym_center = x + SYMBOL_COLS * cell_w * 0.5;
        if m.symbol {
            x += SYMBOL_COLS * cell_w;
        }
        let pad = if m.blank { PAD_R } else { 0.0 };
        Self {
            width: x + pad * cell_w,
            stamp_x,
            num_right,
            fold_center,
            fold_on: m.fold,
            sym_center,
        }
    }

    /// 折叠列上的鼠标点击产出哪条折叠命令。空白 / 列没开 / 不在本列 → `None`。
    /// 命中判据与 [`paint`] 画 `▸` 的那一格同源，只认真正的折叠头。
    ///
    /// 修饰键（原版 WindTerm 语义）：
    /// - 无修饰 → `ToggleAt`：切换这一块。
    /// - Ctrl → 全部块的 Fold/Unfold 开关：当前无折则 `FoldAll`，否则 `UnfoldAll`。
    /// - Alt（递归）/ Shift（同类型）→ 本仓块平坦不嵌套、无类型概念，语义未定义，
    ///   先退回 `ToggleAt`，在 hover / 日志里说明，不假装支持。
    #[allow(clippy::too_many_arguments)]
    pub(in crate::ui) fn fold_click_cmd(
        &self,
        cell_w: f32,
        area_min: egui::Pos2,
        geom_start: egui::Pos2,
        geom_cell: egui::Vec2,
        image: &ScreenImage,
        folds: &Folds,
        inputs: &[HistoryRow],
        resp: &egui::Response,
        mods: egui::Modifiers,
    ) -> Option<term_app::vocab::FoldCmd> {
        if !self.fold_on || !resp.clicked() {
            return None;
        }
        let (cx0, cx1) = (area_min.x + self.fold_center - cell_w * 0.5,
                         area_min.x + self.fold_center + cell_w * 0.5);
        let p = resp.interact_pointer_pos()?;
        if !(cx0..=cx1).contains(&p.x) {
            return None;
        }
        let row = ((p.y - geom_start.y) / geom_cell.y).floor().max(0.0) as usize;
        let line = image.lines.get(row)?;
        let abs = HistoryRow::new(image.history as i64 + line.row)?;
        if folds.regions().iter().any(|r| r.start() == abs) || block_at(inputs, abs).is_some() {
            Some(fold_cmd_for_modifiers(mods, folds.is_empty(), abs))
        } else {
            None
        }
    }
}

/// epoch 秒 → `[HH:MM:SS]`。
///
/// **UTC**，与状态栏的时钟同一口径（`ui/shell/status_bar.rs` 的
/// `format_utc`，同样不引 chrono）。换成本地时要一起改两处，别只改边栏——
/// 那时会出现「状态栏 08:12 / 边栏 00:12」的矛盾。
fn stamp_text(epoch: u64) -> String {
    let s = epoch % 86_400;
    format!("[{:02}:{:02}:{:02}]", s / 3600, s % 3600 / 60, s % 60)
}

/// 在这一行的时间戳（没有就是空串，不画）。
fn stamp_at(snap: &GridSnapshot, abs: i64) -> Option<u64> {
    snap.row_times
        .binary_search_by_key(&abs, |(row, _)| *row)
        .ok()
        .map(|i| snap.row_times[i].1)
}

/// 这一行是不是 OSC 133 的提示符行（`A` 标记所在行）。
fn is_prompt(snap: &GridSnapshot, abs: i64) -> bool {
    snap.blocks
        .binary_search_by_key(&abs, |b| b.prompt_start)
        .is_ok()
}

/// 这一行是不是自动换行产生的续行。
fn is_wrapped(snap: &GridSnapshot, abs: i64) -> bool {
    term_core::grid::is_continued(&snap.wraps, abs)
}

/// 这一行是不是某个折叠区间的**首行**（也就是折叠头，用户点开它的那一行）。
///
/// 判据只有一条：`Folds::regions()` 里存在 `start() == abs`。刻意**不**写成
/// `!folds.hides(abs)`——`hides` 说「这一行藏不藏」，而首行按约定永远不藏，
/// 于是屏幕上一大片没折的行都会满足它；反过来用也会把「首行」与「区间外的行」
/// 混成一类。
///
/// `abs` 是绝对历史行号（`image.history + line.row`，与 `paint` 的行循环同源），
/// 负数（理论上不该出现）直接当非折叠头处理，不构造 `HistoryRow`。
fn is_fold_head(folds: &Folds, abs: i64) -> bool {
    let Some(row) = HistoryRow::new(abs) else {
        return false;
    };
    folds
        .regions()
        .binary_search_by_key(&row, |region| region.start())
        .is_ok()
}

/// 画边栏。调用点在画正文**之前**：当前行的整行底色要压在 cell 背景下面。
///
/// `area` 是终端区整块矩形（当前行底色的右端），`start_y` 是第一行的顶边，
/// `current_row` 是光标所在绝对行（只在不回看时给 `Some`），`folds` 是活动标签的
/// 折叠集合（折叠列拿它判「这一行是不是折叠头」）。
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn paint(
    painter: &egui::Painter,
    m: &MarginState,
    met: &MarginMetrics,
    area: Rect,
    start_y: f32,
    cell_h: f32,
    image: &ScreenImage,
    snap: &GridSnapshot,
    folds: &Folds,
    font: &egui::FontId,
    current_row: Option<i64>,
    // 与正文同源的一帧折行配置（mode / column / 软折开关）。
    wrap_cfg: term_app::wrap::WrapConfig,
) {
    if met.width <= 0.0 {
        return;
    }
    // 与正文同一套段表口径：软折开时按同一份 mode / column 折，关时恒等。
    // 纯函数、同输入同输出，算两次无副作用（margin 与正文各算一份）。
    let segments = term_app::wrap::segments_for_wrap(image, wrap_cfg);
    let segments = &segments.rows;
    let pal = term_core::theme::MARGIN;
    let time_col = to_color32(Rgb::from_u32(pal.time));
    let number_col = to_color32(Rgb::from_u32(pal.number));
    let current_col = to_color32(Rgb::from_u32(pal.current));
    let prompt_col = to_color32(Rgb::from_u32(pal.prompt));
    let wrap_col = to_color32(Rgb::from_u32(pal.wrap));
    let band = to_color32(Rgb::from_u32(pal.current_bg));

    // 按段循环而非按逻辑行：软折时一条逻辑行占多段，正文按段序号算 y，边栏
    // 必须同口径（否则行号 / 提示符 / 当前行条带与正文错行）。
    // 行号 / 时间戳 / 提示符 / 折叠头 / 当前行底色只在**首段**画（用户裁定），
    // `~` 续行标记画在软折续段上；软折关时 seg 恒等（每段==一行），像素不变。
    for (seg_idx, seg) in segments.iter().enumerate() {
        let Some(line) = image.lines.get(seg.line) else {
            continue;
        };
        let i = seg_idx;
        let soft_cont = seg.soft_continued;
        // 行号取内核给的那一行（`line.row`），不再做「窗口起点 + 下标」的减法。
        //
        // 理由与 `view.rs` 隔行底色那一条相同：折叠把行收起来之后，事实窗口里的
        // 行号**不再连续**（`lines[i].row != range.start + i`），减法会指到相邻的
        // 另一行——边栏的行号 / 时间戳 / 提示符符号会整列错位。而 `line.row` 本来
        // 就是内核给的行号：**行连续时两者逐值相等**，所以这是零行为变化的改法
        // （空折叠那条路径一个像素都没动）。
        //
        // `image.history as i64 +` 刻意保持裸加：整段换成 `view_to_history` 会在
        // 溢出与负历史处与旧式不等价，那是行为修正，不在「零行为变化」这一刀里。
        let abs = image.history as i64 + line.row;
        let y = start_y + i as f32 * cell_h;
        let is_current = current_row == Some(abs);
        // 当前行：整行铺一条淡底（含边栏），时间戳与行号换成强调色。
        // 视觉基线里这一条带子横跨整屏——它标的是「输入行」，不是选区。
        if is_current {
            painter.rect_filled(
                Rect::from_min_size(
                    egui::pos2(area.min.x, y),
                    Vec2::new(area.width(), cell_h),
                ),
                0.0,
                band,
            );
        }
        // 行号 / 时间戳只在首段画（续段那一行边栏留空，避免重复号码）。
        if m.timestamp && !soft_cont {
            if let Some(epoch) = stamp_at(snap, abs) {
                painter.text(
                    egui::pos2(area.min.x + met.stamp_x, y),
                    Align2::LEFT_TOP,
                    stamp_text(epoch),
                    font.clone(),
                    if is_current { current_col } else { time_col },
                );
            }
        }
        if m.number && !soft_cont {
            painter.text(
                egui::pos2(area.min.x + met.num_right, y),
                Align2::RIGHT_TOP,
                format!("{}", abs + 1),
                font.clone(),
                if is_current { current_col } else { number_col },
            );
        }
        if m.fold && is_fold_head(folds, abs) {
            // `▸`：两条线段画的三角，不依赖字体有没有这个字形（与 `›` 同一做法）。
            // 它标的是「这一行是一段被折起来的输出的头」——被折掉的行根本不在
            // `image.lines` 里，所以这一列永远不会出现「一排记号」，只会出现在
            // 折叠头上，本身就是「这里收着一块」的提示。
            let cx = area.min.x + met.fold_center;
            let cy = y + cell_h * 0.5;
            let h = cell_h * 0.24;
            let w = cell_h * 0.18;
            painter.add(egui::Shape::line(
                vec![
                    egui::pos2(cx - w, cy - h),
                    egui::pos2(cx + w, cy),
                    egui::pos2(cx - w, cy + h),
                    egui::pos2(cx - w, cy - h),
                ],
                egui::Stroke::new((cell_h * 0.07).max(1.0), if is_current {
                    current_col
                } else {
                    number_col
                }),
            ));
        }
        if m.symbol {
            let cx = area.min.x + met.sym_center;
            let cy = y + cell_h * 0.5;
            // `~` 续行标记：软折开时画在 soft_continued 段（wrap column 处），
            // 否则沿用内核硬折 wraps。提示符 `›` 只画在首段。
            let cont = soft_cont || (!wrap_cfg.enabled && is_wrapped(snap, abs));
            if !soft_cont && is_prompt(snap, abs) {
                // `›`：两条线段，不依赖字体有没有这个字形（同会话树的做法）。
                let h = cell_h * 0.26;
                let w = cell_h * 0.20;
                painter.add(egui::Shape::line(
                    vec![
                        egui::pos2(cx - w * 0.5, cy - h),
                        egui::pos2(cx + w * 0.5, cy),
                        egui::pos2(cx - w * 0.5, cy + h),
                    ],
                    egui::Stroke::new((cell_h * 0.07).max(1.0), prompt_col),
                ));
            } else if cont {
                // `~`：换行续行。ASCII 波浪号哪个字体都有，直接排字。
                painter.text(
                    egui::pos2(cx, cy),
                    Align2::CENTER_CENTER,
                    "~",
                    font.clone(),
                    wrap_col,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_is_ten_cells_and_zero_padded() {
        assert_eq!(stamp_text(0), "[00:00:00]");
        assert_eq!(stamp_text(3723), "[01:02:03]");
        assert_eq!(stamp_text(86_399), "[23:59:59]");
        for t in [0u64, 1, 59, 3_600, 43_199, 86_399, 90_000] {
            assert_eq!(stamp_text(t).chars().count(), STAMP_COLS as usize);
        }
    }

    #[test]
    fn number_column_grows_in_three_cell_steps() {
        // 9 行也要 3 格：否则第 10 行一出现，整列会整体右跳一格。
        assert_eq!(number_cols(8), 3);
        assert_eq!(number_cols(99), 3);
        assert_eq!(number_cols(100), 3);
        // 行号 = 绝对行号 + 1，所以 998 是第 999 行，仍是一位数三格。
        assert_eq!(number_cols(998), 3);
        assert_eq!(number_cols(999), 4);
        assert_eq!(number_cols(1_000), 4);
        // 极端长跑仍要留出正文。
        assert_eq!(number_cols(10_000_000), MAX_NUMBER_COLS);
    }

    #[test]
    fn width_follows_enabled_columns() {
        let cell = 10.0;
        let mut m = MarginState::default();
        // 默认开时间戳：0.5 留白 + 10 时间戳 + 1 间隙 + 行号 3 + 符号前间隙 2
        // + 符号 1 + 右边距 1 = 18.5 格。
        assert_eq!(MarginMetrics::new(&m, cell, 20).width, 185.0);
        // 关掉时间戳省下 11 格：0.5 + 3 + 2 + 1 + 1 = 7.5 格。
        m.timestamp = false;
        assert_eq!(MarginMetrics::new(&m, cell, 20).width, 75.0);
        // 三列全关：宽度 0，正文从左边开始（不该留一条死白）
        m.number = false;
        m.symbol = false;
        assert_eq!(MarginMetrics::new(&m, cell, 20).width, 0.0);
    }

    #[test]
    fn blank_toggle_shaves_both_edges() {
        let cell = 10.0;
        let mut m = MarginState::default();
        let with = MarginMetrics::new(&m, cell, 20).width;
        m.blank = false;
        let without = MarginMetrics::new(&m, cell, 20).width;
        // 左边距 0.5 + 右边距 1.0 = 1.5 格
        assert_eq!(with - without, 15.0);
    }

    #[test]
    fn the_fold_column_widens_the_margin_by_exactly_one_cell_and_a_gap() {
        let cell_w = 8.0;
        let mut m = MarginState::default();
        // 默认四列里没有折叠列。
        assert!(!m.fold);
        let without = MarginMetrics::new(&m, cell_w, 120).width;
        m.fold = true;
        let with = MarginMetrics::new(&m, cell_w, 120).width;
        assert_eq!(with - without, (FOLD_COLS + GAP) * cell_w);
        // 折叠列开在行号与符号之间：行号右边界不因它而动。
        let off = MarginState::default();
        assert_eq!(
            MarginMetrics::new(&m, cell_w, 120).num_right,
            MarginMetrics::new(&off, cell_w, 120).num_right,
            "折叠列排在行号之后，不许把行号推走"
        );
    }

    #[test]
    fn fold_heads_are_matched_by_absolute_row_not_by_window_index() {
        let mut folds = Folds::new();
        let start = HistoryRow::new(104).expect("非负行号");
        let end = HistoryRow::new(109).expect("非负行号");
        folds
            .add(term_app::folds::FoldRegion::new(start, end).expect("合法区间"))
            .expect("空集合插入必成功");

        // 只有首行算折叠头（它就是用户点开的那一行）。
        assert!(is_fold_head(&folds, 104));
        // 区间内部与尾行都不是头：正文里它们已被藏掉，边栏不该冒记号。
        assert!(!is_fold_head(&folds, 105));
        assert!(!is_fold_head(&folds, 109));
        // 区间外一行都不算。
        assert!(!is_fold_head(&folds, 103));
        assert!(!is_fold_head(&folds, 110));
        // 负数历史行不构造 `HistoryRow`，直接当「不是头」。
        assert!(!is_fold_head(&folds, -1));
        // 空集合里没有任何头（空折叠 = 老路径，一个像素都不变）。
        assert!(!is_fold_head(&Folds::new(), 104));
    }

    #[test]
    fn lookup_tables_use_absolute_rows() {
        use term_core::grid::GridSnapshot;
        let mut snap = GridSnapshot::empty(10, 4);
        snap.history = 100;
        snap.row_times = vec![(100, 0), (101, 3_723)];
        snap.wraps = vec![101];
        assert_eq!(stamp_at(&snap, 100), Some(0));
        assert_eq!(stamp_at(&snap, 101), Some(3_723));
        assert_eq!(stamp_at(&snap, 102), None);
        assert!(is_wrapped(&snap, 101));
        assert!(!is_wrapped(&snap, 100));
        // 没有 OSC 133 集成时一个提示符都不该冒出来（不能猜）。
        assert!(!is_prompt(&snap, 100));
    }

    #[test]
    fn fold_click_modifiers_choose_the_right_command() {
        use term_app::vocab::FoldCmd;
        let abs = HistoryRow::new(10).unwrap();
        let none = egui::Modifiers::NONE;
        let ctrl = egui::Modifiers { ctrl: true, ..Default::default() };
        let alt = egui::Modifiers { alt: true, ..Default::default() };
        let shift = egui::Modifiers { shift: true, ..Default::default() };

        // 无修饰 / Alt / Shift：本仓均无对应语义，退回单块 ToggleAt。
        assert_eq!(fold_cmd_for_modifiers(none, false, abs), FoldCmd::ToggleAt(abs));
        assert_eq!(fold_cmd_for_modifiers(alt, false, abs), FoldCmd::ToggleAt(abs));
        assert_eq!(fold_cmd_for_modifiers(shift, false, abs), FoldCmd::ToggleAt(abs));
        // Ctrl：无折 → 全折，有折 → 全展开。
        assert_eq!(fold_cmd_for_modifiers(ctrl, true, abs), FoldCmd::FoldAll);
        assert_eq!(fold_cmd_for_modifiers(ctrl, false, abs), FoldCmd::UnfoldAll);
    }
}
