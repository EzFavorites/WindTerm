//! 选区命令的落地点：位移规则、整屏全选、多选区集合与命令分派。
//!
//! 从 `selection.rs` 搬出来的一段。「选择」菜单、Shift+方向键、右键菜单都落到
//! 这里的同一条纯函数路径上，所以「点菜单下一行」和「按 Shift+↓」不可能出现
//! 两种行为。数据模型（`Pos` / `Shape` / `Selection`）留在 `selection.rs`，
//! 这里的每个函数都只读它们。

use term_core::grid::ScreenImage;
use term_core::osc133::CommandBlock;

use crate::vocab::{Cmd, Op, SetOp};
use super::geometry::{
    caret, char_next, char_next_in_row, char_prev, char_prev_in_row, clamp_col, clamp_row,
    command_rows, content_len, document_end, indentation_target, line_end, nearest_above,
    nearest_below, page, pair_end, pair_next, pair_prev, pair_start, paragraph_next, paragraph_prev,
    scope_rows, stuttered_page, window_start, last_row,
};
use super::words::{
    line_span_at, word_at, word_end_next, word_end_prev, word_part_next, word_part_prev,
    word_start_next, word_start_prev,
};
use super::{Pos, Selection, Shape};

// ------------------------------------------------- 键盘 / 菜单的选区命令

/// 把操作作用到选区上，返回新的选区。
///
/// 规则只有一条：**锚点不动，只挪头部**——这就是「扩选」的定义。没有现成选区时，
/// 锚点取终端光标（回看时窗口里没有光标，退回窗口首行），所以第一次按 Shift+→
/// 会从光标处开始扩，而不是从屏幕左上角。
///
/// 移动一律**钳在看得见的窗口内**：选不到屏外的行，因为那些行的文本也复制不出来
/// （`text()` 只取窗口内的行）。这与滚动选区的鼠标路径不一致，是故意的——
/// 键盘看不到目标位置，让它把视口拖着滚只会让人失去方向。
///
/// 这是 [`apply_with_blocks`] 的「没有命令块表」版本：`Text.GotoPreviousCommand`
/// 一族需要 OSC 133 的块表，投影层有它、这里没有，所以空表下那 4 条按规格
/// 「不动」（**不是**跳到第 0 行——那会让人以为命令执行了）。
pub fn apply(op: Op, image: &ScreenImage, current: Option<Selection>) -> Option<Selection> {
    apply_with_blocks(op, image, current, &[])
}

/// 带上命令块表的位移入口：`blocks` 是 OSC 133 的块表（**绝对行号**）。
///
/// 为什么不合并进 `apply` 的签名：`apply` 有 17 条命令根本不看块表，把
/// 「块表」塞进每一个调用点等于让所有测试夹具凭空多一个参数。分成两个入口，
/// 默认那条零成本，需要的那条显式传——块表从哪来只有投影层知道。
pub fn apply_with_blocks(
    op: Op,
    image: &ScreenImage,
    current: Option<Selection>,
    blocks: &[CommandBlock<i64>],
) -> Option<Selection> {
    let head = current.map(|s| s.head()).or_else(|| caret(image))?;
    match op {
        Op::All => select_all(image),
        // 选词 / 选行没有「方向」：命中空白就什么都不变，而不是把选区清掉。
        Op::Word => word_at(image, head).or(current),
        Op::Line => line_span_at(image, head.row).or(current),
        Op::ToDocumentStart => extend(current, head, Pos::new(window_start(image), 0)),
        Op::ToDocumentEnd => extend(current, head, document_end(image)),
        Op::ToLineHome => extend(current, head, Pos::new(head.row, 0)),
        Op::ToLineEnd => extend(current, head, Pos::new(head.row, line_end(image, head.row))),
        Op::CharPrev => extend(current, head, char_prev(image, head)),
        Op::CharNext => extend(current, head, char_next(image, head)),
        Op::LinePrev => extend(current, head, clamp_row(image, head.row - 1, head.col)),
        Op::LineNext => extend(current, head, clamp_row(image, head.row + 1, head.col)),
        Op::PagePrev => extend(current, head, clamp_row(image, head.row - page(image), head.col)),
        Op::PageNext => extend(current, head, clamp_row(image, head.row + page(image), head.col)),
        Op::WordStartPrev => extend(current, head, word_start_prev(image, head)),
        Op::WordStartNext => extend(current, head, word_start_next(image, head)),
        Op::WordEndPrev => extend(current, head, word_end_prev(image, head)),
        Op::WordEndNext => extend(current, head, word_end_next(image, head)),

        // ── Select Column To：10 条，矩形语义 ──
        //
        // 与上面同名的几条只差两件事：Char 两条**不跨行回绕**，形状一律落成
        // `Block`。位移数学没有再写一套——行 / 页两条复用 `clamp_row`，
        // 段两条复用 `paragraph_*`，行首行尾复用 `line_end`。
        Op::ColumnCharPrev => column(current, head, char_prev_in_row(image, head)),
        Op::ColumnCharNext => column(current, head, char_next_in_row(image, head)),
        Op::ColumnLinePrev => column(
            current,
            head,
            clamp_row(image, head.row - 1, clamp_col(image, head.col)),
        ),
        Op::ColumnLineNext => column(
            current,
            head,
            clamp_row(image, head.row + 1, clamp_col(image, head.col)),
        ),
        Op::ColumnPagePrev => column(
            current,
            head,
            clamp_row(image, head.row - page(image), clamp_col(image, head.col)),
        ),
        Op::ColumnPageNext => column(
            current,
            head,
            clamp_row(image, head.row + page(image), clamp_col(image, head.col)),
        ),
        Op::ColumnParagraphPrev => {
            // 走不到上一段时**形状照样落成矩形**：命令动作是「按列选」，
            // 目标行到不了不改变这件事（头部原地不动，就是没扩）。
            let to = match paragraph_prev(image, head.row) {
                Some(row) => Pos::new(row, clamp_col(image, head.col)),
                None => head,
            };
            column(current, head, to)
        }
        Op::ColumnParagraphNext => {
            let to = match paragraph_next(image, head.row) {
                Some(row) => Pos::new(row, clamp_col(image, head.col)),
                None => head,
            };
            column(current, head, to)
        }
        Op::ColumnLineHome => column(current, head, Pos::new(head.row, 0)),
        Op::ColumnLineEnd => column(current, head, Pos::new(head.row, line_end(image, head.row))),

        // ── Word Part ──
        Op::WordPartPrev => extend(current, head, word_part_prev(image, head)),
        Op::WordPartNext => extend(current, head, word_part_next(image, head)),

        // ── Stuttered Page：半页 ──
        Op::StutteredPagePrev => extend(
            current,
            head,
            clamp_row(image, head.row - stuttered_page(image), head.col),
        ),
        Op::StutteredPageNext => extend(
            current,
            head,
            clamp_row(image, head.row + stuttered_page(image), head.col),
        ),

        // ── Paragraph ──
        Op::ParagraphPrev => match paragraph_prev(image, head.row) {
            Some(row) => extend(current, head, Pos::new(row, head.col)),
            None => current,
        },
        Op::ParagraphNext => match paragraph_next(image, head.row) {
            Some(row) => extend(current, head, Pos::new(row, head.col)),
            None => current,
        },

        // ── Command：OSC 133 的 `prompt_end`（用户开始打命令处）──
        //
        // 没有块表就**不动**：跳到第 0 行会让「点了没反应」变成「点了乱跳」，
        // 后者更难查出是数据不在而不是命令没实现。
        Op::CommandPrev => match nearest_above(&command_rows(image, blocks), head.row) {
            Some(row) => extend(current, head, Pos::new(row, 0)),
            None => current,
        },
        Op::CommandNext => match nearest_below(&command_rows(image, blocks), head.row) {
            Some(row) => extend(current, head, Pos::new(row, 0)),
            None => current,
        },

        // ── Scope：终端没有嵌套作用域，退化成一整条命令区段（`prompt_start`）──
        //
        // 与 Command 的差别只在两处：落 `prompt_start`（区段开头），且找不到
        // 候选时**退化到窗口首 / 末行**（作用域是一个覆盖全屏的概念，
        // 「没有作用域」等于「整个可见窗口」）。
        Op::ScopePrev => {
            let to = match nearest_above(&scope_rows(image, blocks), head.row) {
                Some(row) => row,
                None => window_start(image),
            };
            extend(current, head, Pos::new(to, 0))
        }
        Op::ScopeNext => {
            let to = match nearest_below(&scope_rows(image, blocks), head.row) {
                Some(row) => row,
                None => last_row(image),
            };
            extend(current, head, Pos::new(to, 0))
        }

        // ── Indentation：行首空白宽度变化处 ──
        Op::IndentationPrev => match indentation_target(image, head.row, -1) {
            Some(to) => extend(current, head, to),
            None => current,
        },
        Op::IndentationNext => match indentation_target(image, head.row, 1) {
            Some(to) => extend(current, head, to),
            None => current,
        },

        // ── Pair：`()[]{}` 深度配对，不平衡就不动 ──
        Op::PairStart => match pair_start(image, head) {
            Some(to) => extend(current, head, to),
            None => current,
        },
        Op::PairEnd => match pair_end(image, head) {
            Some(to) => extend(current, head, to),
            None => current,
        },
        Op::PairPrev => match pair_prev(image, head) {
            Some(to) => extend(current, head, to),
            None => current,
        },
        Op::PairNext => match pair_next(image, head) {
            Some(to) => extend(current, head, to),
            None => current,
        },
    }
}

/// 锚点取自 `current`，头部挪到 `to`；没有现成选区就从 `base` 起一个。
fn extend(current: Option<Selection>, base: Pos, to: Pos) -> Option<Selection> {
    let mut sel = current.unwrap_or_else(|| Selection::new(base));
    sel.drag_to(to);
    Some(sel)
}

/// 矩形（列选）语义的一次位移：锚点不动，头部挪到 `to`，**形状强制落成 `Block`**。
///
/// 10 条 `Select Column To` 共用它，所以「首次按也是矩形」这条规则只写了一遍。
/// 为什么首次也要落成 `Block`：命令名字就叫「按列选」，第一下若不是矩形，
/// 用户按十次才在某个别的入口（Alt+拖动）里得到矩形，等于它从来没生效过。
fn column(current: Option<Selection>, base: Pos, to: Pos) -> Option<Selection> {
    let mut sel = current.unwrap_or_else(|| Selection::new(base)).block();
    sel.drag_to(to);
    Some(sel)
}

/// 整屏：从窗口首行行首到最后一个有内容的行尾。
///
/// 末行往往是屏幕下方的空白填充，选进去只会在复制结果末尾留一串空行，
/// 所以头部落在「最后一个非空行」的内容末尾。
fn select_all(image: &ScreenImage) -> Option<Selection> {
    let first = image.lines.first()?.row;
    let gs_by_row: Vec<(i64, usize)> = image
        .lines
        .iter()
        .map(|l| (l.row, content_len(&l.cells)))
        .filter(|(_, len)| *len > 0)
        .collect();
    let (row, len) = *gs_by_row.last()?;
    Some(Selection::span(Pos::new(first, 0), Pos::new(row, len)))
}

// ------------------------------------------------------ 多选区（集合）

/// 多选区集合：`Selections` 是 9 条集合命令的**全部状态**。
///
/// 为什么不把集合直接塞进 `Selection`：`Selection` 是「一段区间」，它是 `Copy`
/// 的，被渲染、取文本、鼠标拖动按值搬来搬去；让它自带 `Vec` 会把每一个只关心
/// 一段区间的调用点都拖进所有权问题。所以分成两层：
///
/// - `Selection`：一段区间（形状 / 端点 / 取文本）；
/// - `Selections`：若干段区间 + 谁是**活动**的那段 + 锁没锁。
///
/// 每个标签只持有一份 `Selections`，活动选区是一个**下标**而不是第二份槽位——
/// 两份状态并存的话，删除 / 拆分之后迟早会指到不同的东西上。
///
/// 本类型零 UI 依赖：9 条命令各是一个方法，单测不碰 egui。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selections {
    items: Vec<Selection>,
    /// 活动选区在 `items` 里的下标。空集合时无意义（恒 0）。
    active: usize,
    /// 锁定：鼠标拖动与键盘扩选不改活动选区（集合命令照旧生效）。
    locked: bool,
}

impl Selections {
    /// 空集合。
    pub fn new() -> Self {
        Self::default()
    }

    /// 用一份（可能为空的）选区起一个新集合：等于「空集合 + `set_active`」。
    ///
    /// 与 `set_active` 的区别只在**锁**：构造不认锁，是把整份状态换掉
    /// （视觉基线夹具注入就是这种场合）；`set_active` 是用户操作，锁着就不许动。
    pub fn from_active(sel: Option<Selection>) -> Self {
        let mut set = Self::default();
        set.set_active(sel);
        set
    }

    pub fn items(&self) -> &[Selection] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// 有活动选区可操作吗。空集合返回 `false`，活动的下标本身不对外暴露。
    pub fn has_active(&self) -> bool {
        !self.items.is_empty()
    }

    /// 活动选区的下标；空集合返回 `None`。
    pub fn active_index(&self) -> Option<usize> {
        if self.items.is_empty() {
            None
        } else {
            Some(self.active)
        }
    }

    /// 活动选区本身（按值，`Selection` 是 `Copy`）。
    pub fn active(&self) -> Option<Selection> {
        self.items.get(self.active).copied()
    }

    /// 鼠标 / 键盘新起一份选区。锁定时原地不动——这就是「Lock Selection」。
    ///
    /// 非锁定时**替换整个集合**：新拖一次的意思是「我要这一段」，不是往旧集合里
    /// 再加一段。多份选区只由 `split` / `invert` 产生，用户不会「手滑多出几段」。
    pub fn set_active(&mut self, sel: Option<Selection>) {
        if self.locked {
            return;
        }
        self.items.clear();
        self.active = 0;
        if let Some(sel) = sel {
            self.items.push(sel);
        }
    }

    /// 拖动 / shift-click 扩选：只挪活动那份的头部。锁定时不动。
    pub fn drag_active_to(&mut self, pos: Pos) {
        if self.locked {
            return;
        }
        if let Some(sel) = self.items.get_mut(self.active) {
            sel.drag_to(pos);
        }
    }

    /// 清空（单击、菜单「Cancel Selection」）。锁定位一并复位：
    /// 没有选区可锁的时候还留着锁，下一份选区一出生就是锁死的。
    pub fn clear(&mut self) {
        self.items.clear();
        self.active = 0;
        self.locked = false;
    }

    /// 切到下一份选区（到头回绕）。只有一份时是空操作。
    pub fn activate_next(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.active = (self.active + 1) % self.items.len();
    }

    /// 切到上一份选区（到头回绕）。只有一份时是空操作。
    pub fn activate_previous(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.active = (self.active + self.items.len() - 1) % self.items.len();
    }

    /// 锁 / 解锁活动选区。
    ///
    /// 为什么是**开关**：菜单里没有单独的 Unlock，单向锁会留下一条「锁上就再也
    /// 动不了」的死路（关掉必须有回程）。空集合无事可锁，也不翻锁定位。
    pub fn toggle_lock(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.locked = !self.locked;
    }

    /// 只移除**活动**那一份，其余留着（「Drop Selection ≠ 清空」）。
    pub fn drop_active(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.items.remove(self.active);
        if self.items.is_empty() {
            self.active = 0;
            self.locked = false;
            return;
        }
        // 删掉的是末尾那份时，活动位要退一格，否则下标越界到集合之外。
        if self.active >= self.items.len() {
            self.active = self.items.len() - 1;
        }
    }

    /// 把活动选区按行拆成多份，**第一份（最上面那一行）保持活动**。
    ///
    /// 拆出来的每一份都是单行：单行上的矩形与线性是同一个区间，没有第三种形状
    /// 可讲，所以一律落成 `Linear`。
    ///
    /// 列上界钳进 `image.cols`：线性选区中间行的 `columns_on` 返回 `usize::MAX`
    /// （整行选中），直接拿来当 head.col 虽然取文本时会钳住，但用户再去扩它
    /// 就会跳到行尾之外——落成真实列号更诚实。
    pub fn split(&mut self, image: &ScreenImage) {
        let Some(sel) = self.active() else {
            return;
        };
        let (first, last) = match sel.shape() {
            Shape::Linear => {
                let (start, end) = sel.normalized();
                (start.row, end.row)
            }
            Shape::Block => {
                let (a, b) = (sel.anchor().row, sel.head().row);
                (a.min(b), a.max(b))
            }
        };
        let mut pieces: Vec<Selection> = Vec::new();
        for row in first..=last {
            let Some((from, to)) = sel.columns_on(row) else {
                continue;
            };
            let from = from.min(image.cols);
            let to = to.min(image.cols);
            // 零宽的一行拆不出内容（矩形只有行没有列），跳过而不是留一个空选区。
            if to <= from {
                continue;
            }
            pieces.push(Selection::span(
                Pos::new(row, from),
                Pos::new(row, to),
            ));
        }
        if pieces.is_empty() {
            return;
        }
        // 第一份落在原活动位置上，后面几份紧跟其后。
        self.items.splice(self.active..=self.active, pieces);
    }

    /// 反选：把**当前可见窗口**整块减去现有集合。
    ///
    /// 只按可见窗口算，这是有意的：窗口外的历史行反选出来也复制不出来
    /// （`text()` 只看窗口内），而且每滚一屏反选结果就变一次——那不是一个
    /// 能存下来的选区，是一台会漂的机器。
    ///
    /// 同一段列区间在连续几行上出现时合成**一份矩形**（而不是每行一份），
    /// 否则反选一次能产出几十份选区，`Activate Next Selection` 就没法用了。
    pub fn invert(&mut self, image: &ScreenImage) {
        let cols = image.cols;
        if cols == 0 || image.lines.is_empty() {
            self.clear();
            return;
        }
        // 按「补集区间的第几段」分组：第 k 段在连续行上不变得以延长成一块矩形。
        let mut runs: Vec<Vec<(usize, usize, i64, i64)>> = Vec::new();
        for line in &image.lines {
            let gaps = self.complement_on(line.row, cols);
            for (k, (from, to)) in gaps.iter().enumerate() {
                if runs.len() <= k {
                    runs.push(Vec::new());
                }
                let bucket = &mut runs[k];
                match bucket.last_mut() {
                    Some(last) if last.0 == *from && last.1 == *to && last.3 == line.row - 1 => {
                        last.3 = line.row
                    }
                    _ => bucket.push((*from, *to, line.row, line.row)),
                }
            }
        }
        let mut items: Vec<Selection> = runs
            .iter()
            .flatten()
            .map(|(from, to, first, last)| {
                Selection::span(Pos::new(*first, *from), Pos::new(*last, *to)).block()
            })
            .collect();
        // 阅读顺序（先首行、再首列）。分组过程按列段轮转，顺序不是天然排好的。
        items.sort_by_key(|s| (s.anchor().row, s.anchor().col));
        self.items = items;
        self.active = 0;
        // 锁的是旧的那一份，整批换掉之后锁跟着作废——新集合从没被锁过。
        self.locked = false;
    }

    /// 活动选区的形状 Linear ↔ Block。
    pub fn switch_shape(&mut self) {
        if let Some(sel) = self.items.get_mut(self.active) {
            *sel = sel.toggle_shape();
        }
    }

    /// 活动选区的 anchor / head 互换。
    pub fn exchange_ends(&mut self) {
        if let Some(sel) = self.items.get_mut(self.active) {
            *sel = sel.swap_ends();
        }
    }

    /// 整批选区的文本（活动不活动都算），按集合顺序用换行连接。
    ///
    /// 只有一份时与 `Selection::text` 逐字节相同；多份时拼接——复制读的就是
    /// 用户看到的那一片高亮，不该只取活动那一份。空选区（塌缩 / 只有行没有列）
    /// 不贡献空行。
    ///
    /// `wraps` 原样透传给每一份选区：硬折行的续行不插 `\n` 这条判据只在
    /// `Selection::text` 里有一份实现，这里不复述。
    pub fn text(&self, image: &ScreenImage, wraps: &[i64]) -> String {
        let mut out: Vec<String> = Vec::new();
        for sel in &self.items {
            let part = sel.text(image, wraps);
            if !part.is_empty() {
                out.push(part);
            }
        }
        out.join("\n")
    }

    /// 某一行「没被选中」的列区间，已排序、已合并、已钳进 `cols`。
    fn complement_on(&self, row: i64, cols: usize) -> Vec<(usize, usize)> {
        let mut covered: Vec<(usize, usize)> = Vec::new();
        for sel in &self.items {
            let Some((from, to)) = sel.columns_on(row) else {
                continue;
            };
            let from = from.min(cols);
            let to = to.min(cols);
            if to > from {
                covered.push((from, to));
            }
        }
        covered.sort_unstable();
        let mut gaps: Vec<(usize, usize)> = Vec::new();
        let mut x = 0usize;
        for (from, to) in covered {
            if from > x {
                gaps.push((x, from));
            }
            x = x.max(to);
        }
        if x < cols {
            gaps.push((x, cols));
        }
        gaps
    }

    /// 写回活动位置（`Cmd::Move` 的落点）：不动集合的其余成员，也不再过锁——
    /// 调用方 `apply_cmd` 已经判过锁了，这里再判一次会让扩选悄悄失效。
    fn assign_active(&mut self, sel: Option<Selection>) {
        if self.items.is_empty() {
            if let Some(sel) = sel {
                self.items.push(sel);
            }
            self.active = 0;
            return;
        }
        match sel {
            Some(sel) => self.items[self.active] = sel,
            // 位移算不出结果（空屏上的「全选」）就真的没有选区可留。
            None => self.clear(),
        }
    }
}

/// 把一条命令作用到集合上。**这是 13 条命令唯一的落地点**，
/// 菜单 / 键盘 / 右键菜单都调它。
///
/// 没有命令块表：`Command` / `Scope` 两条会按「没有块表就不动」执行。
/// 投影层有块表，走 [`apply_cmd_with_blocks`]。
pub fn apply_cmd(cmd: Cmd, image: &ScreenImage, set: &mut Selections) {
    apply_cmd_with_blocks(cmd, image, &[], set);
}

/// 带上命令块表的集合命令入口。`blocks` 是 OSC 133 块表（绝对行号）。
///
/// 形状与 [`apply_cmd`] 完全一致，只是把块表往下透传——集合本身的处理
/// （锁 / 活动下标 / 写回）只有这一份实现，不存在第二个 `apply_cmd`。
pub fn apply_cmd_with_blocks(
    cmd: Cmd,
    image: &ScreenImage,
    blocks: &[CommandBlock<i64>],
    set: &mut Selections,
) {
    match cmd {
        Cmd::Move(op) => {
            if set.is_locked() {
                return;
            }
            let updated = apply_with_blocks(op, image, set.active(), blocks);
            set.assign_active(updated);
        }
        Cmd::Set(op) => match op {
            SetOp::ActivateNext => set.activate_next(),
            SetOp::ActivatePrevious => set.activate_previous(),
            SetOp::Lock => set.toggle_lock(),
            SetOp::Drop => set.drop_active(),
            SetOp::Cancel => set.clear(),
            SetOp::Split => set.split(image),
            SetOp::Invert => set.invert(image),
            SetOp::SwitchShape => set.switch_shape(),
            SetOp::Exchange => set.exchange_ends(),
        },
    }
}

/// 命令 ID → 本能力的动作。未接线的 ID 返回 `None`。
///
/// 2026-11-12：实现只有一份，在 [`crate::parse`]（入口层）。这里保留同名入口，
/// 一是因为「每个能力自带字符串入口」是这个 crate 对外的约定，二是因为
/// `apps/desktop` 有直接调 `term_app::log::parse` 的地方——把路径改掉没有架构价值。
pub fn parse(id: &str) -> Option<Op> {
    crate::parse::selection_parse(id)
}

/// 命令 ID → 本能力的动作。未接线的 ID 返回 `None`。
///
/// 2026-11-12：实现只有一份，在 [`crate::parse`]（入口层）。这里保留同名入口，
/// 一是因为「每个能力自带字符串入口」是这个 crate 对外的约定，二是因为
/// `apps/desktop` 有直接调 `term_app::log::parse` 的地方——把路径改掉没有架构价值。
pub fn parse_set(id: &str) -> Option<SetOp> {
    crate::parse::selection_set_parse(id)
}
