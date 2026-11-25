//! The terminal screen: a VT100/xterm-256colour grid driven by the same escape
//! grammar a standard terminal parses. Every byte the shell writes goes through
//! `vte`'s parser, which calls back into `Terminal`, so cursor addressing,
//! scrolling regions, erase, SGR colours, the alternate screen and window titles
//! behave like a real terminal rather than a transcript.
//!
//! The queries a program can make and the answers given here:
//!
//!   `ESC [ c`      device attributes  -> `ESC [ ?62;1;2;6;9;15;18;21;22c` (VT220+)
//!   `ESC [ 5 n`    device status      -> `ESC [ 0 n`
//!   `ESC [ 6 n`    cursor position    -> `ESC [ row ; col R` (1-based)
//!   `ESC [ 18 t`   window size in cells -> `ESC [ 8 ; rows ; cols t`
//!   `OSC 0` / `OSC 2` window/icon title
//!   `ESC [ ?1049 h/l` alternate screen with saved cursor
//!   `OSC 52`       clipboard read/write

use crate::coords::ViewRange;
use crate::grid::{Attrs, Cell, Colour, CursorShape, GridSnapshot, LineSlice, Rgb, ScreenImage};
use crate::mouse_report::{MouseConfig, MouseEncoding, MouseMode};
use crate::osc133::{blocks_at, segments, CommandBlock, Mark, Segment};
use crate::theme::Palette;
use std::collections::VecDeque;
use vte::ansi::{Handler, Processor};

/// The pen: colours and attributes a newly printed cell picks up.
#[derive(Clone, Copy)]
struct Pen {
    fg: Colour,
    bg: Colour,
    attrs: Attrs,
}

impl Default for Pen {
    fn default() -> Self {
        Self {
            fg: Colour::Default,
            bg: Colour::Default,
            attrs: Attrs::EMPTY,
        }
    }
}

/// DECSET 鼠标档位的原始开关。
///
/// 存「每一位」而不是只存当前档：程序常按 `?1000h` → `?1002h` → `?1003h`
/// → `?1003l` 的顺序开关，只记一个枚举就没法在关掉 1003 时回落到仍开着的
/// 1002；编码同理（关掉 1006 要能回到还在开的 1015 / 1005）。
#[derive(Clone, Copy, Debug, Default)]
struct MouseFlags {
    clicks: bool,      // 1000
    cell_motion: bool, // 1002
    all_motion: bool,  // 1003
    utf8: bool,        // 1005
    sgr: bool,         // 1006
    urxvt: bool,       // 1015
}

impl MouseFlags {
    /// 1003 > 1002 > 1000：高优先级开着就盖住低优先级，但 reset 时一起清。
    fn mode(&self) -> MouseMode {
        if self.all_motion {
            MouseMode::AllMotion
        } else if self.cell_motion {
            MouseMode::CellMotion
        } else if self.clicks {
            MouseMode::Clicks
        } else {
            MouseMode::Off
        }
    }

    /// 1006 > 1015 > 1005 > 默认 X10。
    fn encoding(&self) -> MouseEncoding {
        if self.sgr {
            MouseEncoding::Sgr
        } else if self.urxvt {
            MouseEncoding::Urxvt
        } else if self.utf8 {
            MouseEncoding::Utf8
        } else {
            MouseEncoding::X10
        }
    }

    fn config(&self) -> MouseConfig {
        MouseConfig {
            mode: self.mode(),
            encoding: self.encoding(),
        }
    }
}

pub struct Terminal {
    pub cols: usize,
    pub rows: usize,
    pub grid: Vec<Vec<Cell>>,

    /// Lines that scrolled off the top, oldest first. Not collected while an
    /// alternate-screen program owns the display.
    pub scrollback: VecDeque<Vec<Cell>>,
    /// `window.scrollbackLimit`; the conventional default is 999999, kept
    /// smaller here so a long build log cannot exhaust memory.
    pub max_scrollback: usize,

    pub cursor_row: usize,
    pub cursor_col: usize,
    pub cursor_visible: bool,
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,

    /// `OSC 0`/`OSC 2`: what a conventional terminal shows in the tab's caption.
    pub title: String,
    /// OSC 133 标记及其**绝对行号**（`scrollback.len() + cursor_row`）。
    ///
    /// 为什么要存绝对行号：视口坐标会随滚动变化（`0` 是屏顶、负数是回看），
    /// 标记一存下来就过期。绝对行号在历史被裁剪时整体左移（见 `scroll_up`），
    /// 查询时再减掉当时的 `scrollback.len()` 换成视口坐标。
    pub cmd_marks: Vec<(i64, Mark)>,
    /// 每行的「这一段字节到达的时刻」：绝对行号 → epoch 秒。margin 时间戳列读它。
    ///
    /// 字面要求是「语义平面不得**读**墙钟」——这里一次都不调
    /// `SystemTime::now()`，只**收下**宿主（`term-runtime`）写入前递进来的时刻
    /// （`stamp_row`）。时刻的产权在宿主，语义层只负责把它和行对齐，并在历史
    /// 裁剪时跟着一起左移，规则与 `cmd_marks` 完全一致。
    pub row_times: Vec<(i64, u64)>,
    /// 宿主最近一次递进来的时刻（`stamp_row` 收下、`touch_row` 沿用）。
    /// `None` = 还没有宿主给过时刻（单测 / 无宿主的纯语义使用）。
    now: Option<u64>,
    /// 由**自动换行**产生的续行的绝对行号（这是上一行的后半截，不是新逻辑行）。
    /// margin 符号列据此画 `~`；空表 = 这段输出没有折行。
    pub wrap_rows: Vec<i64>,
    /// Bytes a program asked to put on the system clipboard (`OSC 52`).
    pub clipboard_request: Option<String>,
    /// A program asked to *read* the clipboard (`OSC 52` with `-`); the UI
    /// answers it with whatever the UI layer last copied, because the UI layer owns the
    /// clipboard and this module never touches the window system.
    pub clipboard_query: Option<u8>,

    /// Region `ESC [ r` restricts scrolling and erasing to.
    top_margin: usize,
    bottom_margin: usize,
    /// `ESC 7` cursor, together with the pen it was saved with.
    saved: Option<(usize, usize, Pen)>,
    /// Screen kept while an alternate-screen program runs (`ESC [ ?1049 h`).
    main_grid: Option<Vec<Vec<Cell>>>,
    main_scrollback: Option<VecDeque<Vec<Cell>>>,
    main_margins: Option<(usize, usize)>,
    alt_screen: bool,

    pen: Pen,
    /// A glyph printed in the last column defers the line break to the next one.
    wrap_pending: bool,

    /// `DECSET 7` (`window.autoWrapMode` is true by default).
    pub autowrap: bool,
    /// `DECSET 1`: arrow keys send `ESC O A` instead of `ESC [ A`.
    pub application_cursor_keys: bool,
    /// `DECSET 6`: cursor addressing counts from the region's top line.
    pub origin_mode: bool,
    /// `DECSET 2004`: paste arrives wrapped in `ESC [ 200~ … ESC [ 201~`.
    pub bracketed_paste: bool,
    /// `DECSET 1000/1002/1003`: the program wants mouse reports.
    ///
    /// 保留这个布尔字段是为了不波及其它读者；`mouse.mode != Off` 与它始终同步。
    pub mouse_reporting: bool,
    /// 鼠标报告的完整配置（档位 + 编码）。快照从这里读到真实档位。
    pub mouse: MouseConfig,
    /// 各档位/编码的原始开关，供「关掉高优先级后回落」使用。
    mouse_flags: MouseFlags,
    /// `IRM` (`ESC [ 4 h`): printing inserts blank cells instead of overwriting.
    pub insert_mode: bool,

    parser: Processor,
    /// Bytes the emulator owes the shell (device attributes, cursor position).
    replies: Vec<u8>,
    /// Total bytes received, for the status bar.
    pub received_bytes: u64,
    /// 累计 BEL 次数。宿主靠它的**变化**决定要不要响铃；语义层自己不读钟、
    /// 不发声，所以这个计数不进 `GridSnapshot`（金样本 JSON 也不包含它）。
    pub bell_seq: u64,
}

impl Terminal {
    pub fn clear(&mut self) {
        let last_row = self.rows.saturating_sub(1);
        let last_col = self.cols.saturating_sub(1);
        self.erase_area(0, 0, last_row, last_col);
        self.cursor_row = 0;
        self.cursor_col = 0;
        // 末列欠着的换行属于「被清掉的那一行」，不能跟到下一字符上。
        self.wrap_pending = false;
    }

    /// 历史一旦丢弃，按**绝对行号**建的三张侧表就一起失效。
    ///
    /// 只清 `scrollback` 会留下指向不存在历史的 `cmd_marks` / `row_times` /
    /// `wrap_rows`：这些表的行号原本是 `scrollback.len() + cursor_row`，历史归零后
    /// 仍是旧的大数，`grid::is_continued` 的二分查找与 margin 的绝对行换算都会错位。
    /// 所有清历史入口（菜单、ED 1/2/3）共用这一处，避免再漏掉某一张表。
    fn drop_history(&mut self) {
        self.scrollback.clear();
        self.cmd_marks.clear();
        self.row_times.clear();
        self.wrap_rows.clear();
    }

    /// `Terminal.ClearScrollback` — drop every line above the current screen.
    pub fn clear_scrollback(&mut self) {
        self.drop_history();
    }

    /// `Terminal.ClearScreenAndScrollback` — the two together.
    pub fn clear_screen_and_scrollback(&mut self) {
        self.drop_history();
        self.clear();
    }

    /// `Terminal.SelectAll` — the whole screen as one string (the conventional
    /// selection engine is not modelled here, so the practical effect is the
    /// same as `Text.Copy` of everything on screen).
    pub fn select_all_text(&self) -> String {
        self.screen_text()
    }

    pub fn new(cols: usize, rows: usize, max_scrollback: usize) -> Self {
        let cols = cols.clamp(8, crate::config::MAX_COLS);
        let rows = rows.clamp(2, crate::config::MAX_ROWS);
        let max_scrollback = max_scrollback.clamp(0, crate::config::MAX_SCROLLBACK);
        Self {
            cols,
            rows,
            grid: Self::blank_grid(cols, rows),
            scrollback: VecDeque::new(),
            max_scrollback: max_scrollback.clamp(0, crate::config::MAX_SCROLLBACK),
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            cursor_shape: CursorShape::Block,
            cursor_blink: false,
            title: String::new(),
            cmd_marks: Vec::new(),
            row_times: Vec::new(),
            now: None,
            wrap_rows: Vec::new(),
            clipboard_request: None,
            clipboard_query: None,
            top_margin: 0,
            bottom_margin: rows - 1,
            saved: None,
            main_grid: None,
            main_scrollback: None,
            main_margins: None,
            alt_screen: false,
            pen: Pen::default(),
            wrap_pending: false,
            autowrap: true,
            application_cursor_keys: false,
            origin_mode: false,
            bracketed_paste: false,
            mouse_reporting: false,
            mouse: MouseConfig::default(),
            mouse_flags: MouseFlags::default(),
            insert_mode: false,
            parser: Processor::new(),
            replies: Vec::new(),
            received_bytes: 0,
            bell_seq: 0,
        }
    }

    fn blank_grid(cols: usize, rows: usize) -> Vec<Vec<Cell>> {
        vec![vec![Cell::default(); cols.max(1)]; rows.max(1)]
    }

    pub fn in_alt_screen(&self) -> bool {
        self.alt_screen
    }

    /// Feed shell output through the escape-sequence parser.
    pub fn process(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.received_bytes += bytes.len() as u64;
        }
        // The parser needs `&mut self` (it calls back into this terminal) while
        // it also holds its own state, so the two are kept in separate places
        // for the duration of the call.
        //
        // OSC 133 的标记要在「此刻光标在哪一行」被记下，所以整段不能一次喂完：
        // 按标记切开，喂一段、记一条，再喂下一段。`vte` 的状态跨段延续。
        let mut parser = std::mem::replace(&mut self.parser, Processor::new());
        for segment in segments(bytes) {
            match segment {
                Segment::Bytes(chunk) => parser.advance(self, chunk),
                Segment::Mark { mark, .. } => self.record_mark(mark),
            }
        }
        self.parser = parser;
    }

    /// 记一条 OSC 133 标记，行号取**当前光标所在的绝对行**。
    ///
    /// 备用屏里的标记不记：全屏程序（vim / less）自己没有「命令行」的概念，
    /// 记下来只会让 `Select Command` 命中一段无关的界面文字。
    fn record_mark(&mut self, mark: Mark) {
        if self.alt_screen {
            return;
        }
        let row = self.scrollback.len() as i64 + self.cursor_row as i64;
        self.cmd_marks.push((row, mark));
        // 上限按块数粗算（一条命令 4 条标记），免得长跑会话把标记堆成泄漏。
        const MAX_MARKS: usize = 16_384;
        if self.cmd_marks.len() > MAX_MARKS {
            let drop = self.cmd_marks.len() - MAX_MARKS;
            self.cmd_marks.drain(..drop);
        }
    }

    /// 已识别的命令块（行号坐标）。空表 = 这个 shell 没装 OSC 133 集成，
    /// 调用方据此**不做**「猜一个边界」，而不是给个假块。
    pub fn command_blocks(&self) -> Vec<CommandBlock<i64>> {
        blocks_at(self.cmd_marks.iter().copied())
    }

    /// 达到上限时从头上丢，不整体左移——行号是绝对的，丢旧的不影响新的。
    fn cap_marks<T>(rows: &mut Vec<T>, max: usize) {
        if rows.len() > max {
            let drop = rows.len() - max;
            rows.drain(..drop);
        }
    }

    /// 按绝对行号**有序**插入时间戳；同一行只留第一次到达的时刻。
    ///
    /// 只比对 `last()` 的前提是行号单调递增，而光标会回跳（`CSI H` / `DECSET 6`
    /// 退出等）。一旦回跳后写旧行，表尾就比表头小，`grid::is_continued` 与
    /// margin 的二分查找立刻失效——这里用 `binary_search` 找插入点，同键不重复。
    fn insert_time(&mut self, row: i64, epoch_secs: u64) {
        match self.row_times.binary_search_by_key(&row, |(r, _)| *r) {
            Ok(_) => {}
            Err(at) => {
                self.row_times.insert(at, (row, epoch_secs));
                Self::cap_marks(&mut self.row_times, 16_384);
            }
        }
    }

    /// 续行表同理：有序插入且同一行不记两次。
    fn insert_wrap(&mut self, row: i64) {
        match self.wrap_rows.binary_search(&row) {
            Ok(_) => {}
            Err(at) => {
                self.wrap_rows.insert(at, row);
                Self::cap_marks(&mut self.wrap_rows, 16_384);
            }
        }
    }

    /// 给**当前光标所在行**记一个到达时刻（epoch 秒），宿主在写入前调用。
    ///
    /// 同一行只认第一次：一行是被逐字符写出来的，每次都覆盖会让「行首时间」
    /// 变成「行尾时间」。时刻由宿主给，语义层不读钟。
    ///
    /// 记下的时刻会被 `touch_row` 沿用：宿主一次喂进来的字节往往写穿好几行，
    /// 那些行都属于同一批到达的输出，理应共享同一个时刻——否则边栏上会出现
    /// 「一行有时间、隔一行空着」的锯齿。
    pub fn stamp_row(&mut self, epoch_secs: u64) {
        if self.alt_screen {
            return;
        }
        self.now = Some(epoch_secs);
        self.push_time(epoch_secs);
    }

    /// 把 `now` 盖到当前行上（每一格真正落笔前调用一次）。
    fn touch_row(&mut self) {
        if let Some(now) = self.now {
            self.push_time(now);
        }
    }

    fn push_time(&mut self, epoch_secs: u64) {
        if self.alt_screen {
            return;
        }
        let row = self.scrollback.len() as i64 + self.cursor_row as i64;
        self.insert_time(row, epoch_secs);
    }

    /// 刚换行，而这一行是**上一条逻辑行的后半截**（自动换行）——记下来。
    ///
    /// 为什么不在 `put` 里直接判断「光标是不是在末列」：自动换行是**延迟**的
    /// （`wrap_pending`），只有真的落到下一行才算折行。这里被调用时行已经换完，
    /// 所以行号取的是新行。
    fn mark_wrap(&mut self) {
        if self.alt_screen {
            return;
        }
        let row = self.scrollback.len() as i64 + self.cursor_row as i64;
        self.insert_wrap(row);
    }

    /// Take anything the emulator has to write back to the shell.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    fn reply(&mut self, s: &str) {
        self.replies.extend_from_slice(s.as_bytes());
    }

    // -------------------------------------------------------------- geometry

    /// Resize the grid, keeping what is on screen anchored top-left, which is
    /// what `window.autoAdjustRowsAndColumns` does in a conventional terminal.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.clamp(8, crate::config::MAX_COLS);
        let rows = rows.clamp(2, crate::config::MAX_ROWS);
        if cols == self.cols && rows == self.rows {
            return;
        }
        let mut next = Self::blank_grid(cols, rows);
        for (r, row) in self.grid.iter().enumerate().take(rows) {
            let copy = cols.min(self.cols).min(row.len());
            next[r][..copy].copy_from_slice(&row[..copy]);
        }
        self.grid = next;
        self.cols = cols;
        self.rows = rows;
        if self.alt_screen {
            self.top_margin = 0;
            self.bottom_margin = rows - 1;
        } else {
            self.top_margin = self.top_margin.min(rows - 1);
            self.bottom_margin = self.bottom_margin.min(rows - 1);
            if self.top_margin >= self.bottom_margin {
                self.reset_region();
            }
        }
        self.cursor_row = self.cursor_row.min(rows - 1);
        self.cursor_col = self.cursor_col.min(cols - 1);
        if let Some((r, c, pen)) = self.saved.take() {
            self.saved = Some((r.min(rows - 1), c.min(cols - 1), pen));
        }
        self.wrap_pending = false;
    }

    pub fn cell(&self, row: usize, col: usize) -> Option<Cell> {
        self.grid.get(row)?.get(col).copied()
    }

    /// 颜色解析委托给 core/theme 的纯函数：UI 只读快照，也能用同一个函数。
    pub fn resolved(&self, palette: &Palette, colour: Colour, is_fg: bool) -> Rgb {
        crate::theme::resolve(palette, colour, is_fg)
    }

    /// 生成一份只读快照交给 UI 绘制，网格按行主序展平。
    pub fn snapshot(&self, version: u64) -> GridSnapshot {
        let mut cells = Vec::with_capacity(self.cols * self.rows);
        for row in self.grid.iter().take(self.rows) {
            for c in 0..self.cols {
                cells.push(row.get(c).copied().unwrap_or_default());
            }
        }
        GridSnapshot {
            cols: self.cols,
            rows: self.rows,
            cells,
            cursor: (self.cursor_row, self.cursor_col),
            cursor_visible: self.cursor_visible,
            title: self.title.clone(),
            version,
            bytes_received: self.received_bytes,
            application_cursor_keys: self.application_cursor_keys,
            bracketed_paste: self.bracketed_paste,
            mouse_reporting: self.mouse_reporting,
            mouse_mode: self.mouse.mode,
            mouse_encoding: self.mouse.encoding,
            alt: self.in_alt_screen(),
            history: self.scrollback.len(),
            blocks: self.command_blocks(),
            row_times: self.row_times.clone(),
            wraps: self.wrap_rows.clone(),
        }
    }

    /// 从当前状态切出任意行窗口。纯函数，不读墙钟、不起线程、不持锁。
    ///
    /// `range` 用相对当前可视区的行号：负数 = 回看（-1 为最近一行历史），
    /// `0..rows` = 当前屏。越界行以空白补齐，保证 `lines.len() == range.len()`，
    /// 投影不需要知道内核的历史上限；请求不产生新事实，`version` 由调用方注入。
    pub fn slice_range(&self, version: u64, range: std::ops::Range<i64>) -> ScreenImage {
        let rows = self.rows as i64;
        let lines: Vec<LineSlice> = range
            .clone()
            .map(|row| {
                let source: Option<Vec<Cell>> = if row < 0 {
                    // scrollback 以 oldest-first 存放；-1 取最新一行历史。
                    // `i64::MIN` 取负会溢出，先 checked 再换算下标：算不出
                    // 就是越界历史，按空白行处理，不能把 panic 带进渲染路径。
                    row.checked_neg()
                        .and_then(|n| n.checked_sub(1))
                        .and_then(|n| usize::try_from(n).ok())
                        .and_then(|idx| self.scrollback.iter().rev().nth(idx).cloned())
                } else if row < rows {
                    self.grid.get(row as usize).cloned()
                } else {
                    None
                };
                let mut cells = Vec::with_capacity(self.cols);
                match source {
                    Some(row_cells) => {
                        for c in 0..self.cols {
                            cells.push(row_cells.get(c).copied().unwrap_or_default());
                        }
                    }
                    None => cells.resize(self.cols, Cell::default()),
                }
                LineSlice { row, cells }
            })
            .collect();
        let cursor = if range.contains(&(self.cursor_row as i64)) {
            Some((self.cursor_row as i64, self.cursor_col))
        } else {
            None
        };
        ScreenImage {
            cols: self.cols,
            version,
            range,
            lines,
            cursor,
            history: self.scrollback.len(),
        }
    }

    /// [`Terminal::slice_range`] 的类型化入口：值域完全一致，只是不收裸区间。
    ///
    /// 切窗口的逐行取数、越界补白、光标判定全部仍由 `slice_range` 一处实现，
    /// 这里只做一次 `to_i64()`，两者不会各自漂移。
    pub fn slice_view(&self, version: u64, view: ViewRange) -> ScreenImage {
        self.slice_range(version, view.to_i64())
    }

    /// Plain text of a row with trailing blanks removed — used for selection,
    /// copying and search. 宽字符的延续格被跳过（`grid::cells_text`）。
    pub fn row_text(row: &[Cell]) -> String {
        crate::grid::cells_text(row)
    }

    pub fn screen_text(&self) -> String {
        let mut out = String::new();
        for row in &self.grid {
            out.push_str(&Self::row_text(row));
            out.push('\n');
        }
        out
    }

    fn reset_region(&mut self) {
        self.top_margin = 0;
        self.bottom_margin = self.rows.saturating_sub(1);
    }

    // -------------------------------------------------------------- printing

    fn put(&mut self, ch: char) {
        if self.wrap_pending && self.autowrap {
            self.cursor_col = 0;
            self.linefeed();
            self.wrap_pending = false;
            // 这是「上一行写满了」造成的换行：新行是续行，margin 要画 `~`。
            self.mark_wrap();
        }
        let width = if crate::width::is_wide(ch) { 2 } else { 1 };
        // A double-width glyph cannot start in the last column: break first.
        if width == 2 && self.cursor_col + 1 >= self.cols && self.autowrap {
            self.cursor_col = 0;
            self.linefeed();
            self.wrap_pending = false;
            self.mark_wrap();
        }
        let (r, c) = (self.cursor_row, self.cursor_col);
        if r >= self.rows || c >= self.cols {
            return;
        }
        // 真正要落笔了：这一行有内容，给它盖上本批字节到达的时刻。
        self.touch_row();
        let pen = self.pen;
        if self.insert_mode {
            self.insert_blank(width);
        }
        self.grid[r][c] = Cell {
            ch,
            fg: pen.fg,
            bg: pen.bg,
            attrs: pen.attrs,
            wide: width == 2,
        };
        if width == 2 && c + 1 < self.cols {
            // The continuation cell carries the background but no glyph.
            self.grid[r][c + 1] = Cell {
                ch: ' ',
                fg: pen.fg,
                bg: pen.bg,
                attrs: pen.attrs,
                wide: false,
            };
        }
        let next = c + width;
        if next >= self.cols {
            // Stay on the last column and remember the owed line break.
            self.wrap_pending = self.autowrap;
        } else {
            self.cursor_col = next;
            self.wrap_pending = false;
        }
    }

    /// A blank cell in the current pen's background, so `EL`/`ED` keep colours.
    fn blank(&self) -> Cell {
        Cell {
            ch: ' ',
            fg: self.pen.fg,
            bg: self.pen.bg,
            attrs: Attrs::EMPTY,
            wide: false,
        }
    }

    fn erase_area(&mut self, from_row: usize, from_col: usize, to_row: usize, to_col: usize) {
        let blank = self.blank();
        let last_row = self.rows.saturating_sub(1);
        let last_col = self.cols.saturating_sub(1);
        for r in from_row..=to_row.min(last_row) {
            let c0 = if r == from_row { from_col } else { 0 };
            let c1 = to_col.min(last_col);
            if let Some(row) = self.grid.get_mut(r) {
                for c in c0..=c1 {
                    if c < row.len() {
                        row[c] = blank;
                    }
                }
            }
        }
    }

    fn linefeed(&mut self) {
        if self.cursor_row == self.bottom_margin {
            self.scroll_region_up(1);
        } else if self.cursor_row < self.rows - 1 {
            self.cursor_row += 1;
        }
    }

    fn scroll_region_up(&mut self, count: usize) {
        let (top, bottom) = (self.top_margin, self.bottom_margin);
        if top >= bottom || bottom >= self.rows {
            return;
        }
        let count = count.min(bottom - top + 1);
        // Only a full-screen scroll hands lines to the scrollback reader.
        let keep = top == 0 && !self.alt_screen;
        for _ in 0..count {
            if keep {
                if let Some(row) = self.grid.first() {
                    let row = row.clone();
                    self.scrollback.push_back(row);
                    while self.scrollback.len() > self.max_scrollback {
                        self.scrollback.pop_front();
                        // 历史少了一行，所有绝对行号整体左移；已经掉出去的行
                        // 对应的标记也一并丢弃，免得 `Select Command` 选到
                        // 一条内容已经不在缓冲里的命令。
                        for m in self.cmd_marks.iter_mut() {
                            m.0 -= 1;
                        }
                        self.cmd_marks.retain(|m| m.0 >= 0);
                        // 时间戳与续行标记用的是同一套绝对行号，规则必须一致，
                        // 否则 margin 会把 A 行的时间贴到 B 行上（差一行就全错）。
                        for t in self.row_times.iter_mut() {
                            t.0 -= 1;
                        }
                        self.row_times.retain(|t| t.0 >= 0);
                        for w in self.wrap_rows.iter_mut() {
                            *w -= 1;
                        }
                        self.wrap_rows.retain(|w| *w >= 0);
                    }
                }
            }
            if top < self.grid.len() {
                self.grid.remove(top);
            }
            let at = bottom.min(self.grid.len());
            self.grid.insert(at, vec![Cell::default(); self.cols]);
        }
        while self.grid.len() < self.rows {
            self.grid.push(vec![Cell::default(); self.cols]);
        }
    }

    fn scroll_region_down(&mut self, count: usize) {
        let (top, bottom) = (self.top_margin, self.bottom_margin);
        if top >= bottom || bottom >= self.rows {
            return;
        }
        let count = count.min(bottom - top + 1);
        for _ in 0..count {
            if bottom < self.grid.len() {
                self.grid.remove(bottom);
            }
            self.grid
                .insert(top.min(self.grid.len()), vec![Cell::default(); self.cols]);
        }
        while self.grid.len() > self.rows {
            self.grid.pop();
        }
    }

    fn enter_alt_screen(&mut self) {
        if self.alt_screen {
            return;
        }
        self.main_grid = Some(std::mem::replace(
            &mut self.grid,
            Self::blank_grid(self.cols, self.rows),
        ));
        self.main_scrollback = Some(std::mem::take(&mut self.scrollback));
        self.main_margins = Some((self.top_margin, self.bottom_margin));
        self.alt_screen = true;
        self.reset_region();
        self.cursor_row = 0;
        self.cursor_col = 0;
        // 主屏末列欠着的换行不能带进备用屏的第一字符。
        self.wrap_pending = false;
    }

    fn leave_alt_screen(&mut self) {
        if !self.alt_screen {
            return;
        }
        if let Some(grid) = self.main_grid.take() {
            self.grid = grid;
        }
        if let Some(sb) = self.main_scrollback.take() {
            self.scrollback = sb;
        }
        if let Some((t, b)) = self.main_margins.take() {
            self.top_margin = t.min(self.rows - 1);
            self.bottom_margin = b.min(self.rows - 1);
        }
        self.alt_screen = false;
        self.cursor_row = self.cursor_row.min(self.rows - 1);
        self.cursor_col = self.cursor_col.min(self.cols - 1);
        // 备用屏里同样可能写到末列；回主屏后下一字符不应提前换行。
        self.wrap_pending = false;
    }

    // -------------------------------------------------------------- keyboard

    /// Bytes for an arrow key, honouring `DECSET 1`.
    pub fn arrow_bytes(&self, arrow: Arrow) -> &'static str {
        arrow_bytes_for(arrow, self.application_cursor_keys)
    }

    /// 粘贴文本 → 送往 PTY 的字节。见 `paste_bytes_for`。
    pub fn paste_bytes(&self, text: &str) -> String {
        paste_bytes_for(text, self.bracketed_paste)
    }
}

#[derive(Clone, Copy)]
pub enum Arrow {
    Up,
    Down,
    Left,
    Right,
}

/// 不依赖 Terminal 实例的方向键编码：UI 只需快照里的
/// `application_cursor_keys` 标志即可调用。
pub fn arrow_bytes_for(arrow: Arrow, application_cursor_keys: bool) -> &'static str {
    let letter = match arrow {
        Arrow::Up => 'A',
        Arrow::Down => 'B',
        Arrow::Right => 'C',
        Arrow::Left => 'D',
    };
    if application_cursor_keys {
        match letter {
            'A' => "\x1bOA",
            'B' => "\x1bOB",
            'C' => "\x1bOC",
            _ => "\x1bOD",
        }
    } else {
        match letter {
            'A' => "\x1b[A",
            'B' => "\x1b[B",
            'C' => "\x1b[C",
            _ => "\x1b[D",
        }
    }
}

/// 粘贴文本 → 送往 PTY 的字节。
///
/// 与 `arrow_bytes_for` 同样是「只要一个模式位」的纯函数：投影层手里只有
/// 快照，拿不到 `Terminal`，但粘贴同样需要按终端状态编码。三件事：
///
/// 1. **换行归一成 `\r`**。终端的「回车」是 CR；`\n` 只在 cooked 模式下
///    碰巧等价（内核把 `\n` 当行结束符）。全屏程序（vim / less）跑在 raw
///    模式下，裸 `\n` 会被当成 ^J 插进缓冲区，粘贴一段多行文本就变成
///    「一行里塞满 ^J」。等价地，`\r\n` 不能变成两个 `\r`，否则每行多一个空行。
/// 2. **控制字符一律丢弃，只留 TAB**。剪贴板内容不可信，而 `ESC` 后面跟着的
///    任何东西都是「键盘输入」：`ESC [ 201~` 能提前闭合括号粘贴，`ESC ] 0;… BEL`
///    能改窗口标题。丢掉控制字符比逐个识别危险序列更彻底——它们对一段要
///    粘进命令行的文本没有任何正面作用。
/// 3. **`DECSET 2004` 打开时整段包进 `ESC[200~ … ESC[201~`**，让 shell 知道
///    这是粘贴（zsh / bash 的 bracketed-paste 会因此不再逐字符触发补全或缩进）。
///
/// 空文本返回空串：不产生一对空括号，避免让 shell 收到一次「粘贴了空内容」。
pub fn paste_bytes_for(text: &str, bracketed: bool) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut body = String::with_capacity(text.len() + 16);
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            // CRLF 与裸 LF 都收敛成单个 CR。
            '\r' => {
                body.push('\r');
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            '\n' => body.push('\r'),
            '\t' => body.push('\t'),
            // 其余 C0 / C1 控制字符与 DEL：丢弃（见文档第 2 条）。
            c if c.is_control() => {}
            c => body.push(c),
        }
    }
    if bracketed {
        format!("\x1b[200~{body}\x1b[201~")
    } else {
        body
    }
}

impl vte::ansi::Handler for Terminal {
    fn input(&mut self, c: char) {
        self.put(c);
    }

    fn set_title(&mut self, title: Option<String>) {
        if let Some(t) = title {
            self.title = t;
        }
    }

    fn goto(&mut self, line: i32, col: usize) {
        let mut row = if line < 0 {
            (self.rows as i32 + line).max(0) as usize
        } else {
            line as usize
        };
        if self.origin_mode {
            row += self.top_margin;
        }
        self.cursor_row = row.min(self.rows - 1);
        self.cursor_col = col.min(self.cols - 1);
        self.wrap_pending = false;
    }

    fn goto_line(&mut self, line: i32) {
        let mut row = if line < 0 {
            (self.rows as i32 + line).max(0) as usize
        } else {
            line as usize
        };
        if self.origin_mode {
            row += self.top_margin;
        }
        self.cursor_row = row.min(self.rows - 1);
        self.wrap_pending = false;
    }

    fn goto_col(&mut self, col: usize) {
        self.cursor_col = col.min(self.cols - 1);
        self.wrap_pending = false;
    }

    fn move_up(&mut self, count: usize) {
        let floor = if self.cursor_row >= self.top_margin {
            self.top_margin
        } else {
            0
        };
        self.cursor_row = self.cursor_row.saturating_sub(count).max(floor);
        self.wrap_pending = false;
    }

    fn move_down(&mut self, count: usize) {
        let ceiling = self.bottom_margin.min(self.rows - 1);
        self.cursor_row = if self.cursor_row < self.top_margin {
            count.min(self.rows - 1 - self.cursor_row) + self.cursor_row
        } else {
            (self.cursor_row + count).min(ceiling)
        };
        self.wrap_pending = false;
    }

    fn move_forward(&mut self, count: usize) {
        self.cursor_col = (self.cursor_col + count).min(self.cols - 1);
        self.wrap_pending = false;
    }

    fn move_backward(&mut self, count: usize) {
        self.cursor_col = self.cursor_col.saturating_sub(count);
        self.wrap_pending = false;
    }

    fn move_down_and_cr(&mut self, count: usize) {
        self.move_down(count);
        self.cursor_col = 0;
    }

    fn move_up_and_cr(&mut self, count: usize) {
        self.move_up(count);
        self.cursor_col = 0;
    }

    fn put_tab(&mut self, mut count: u16) {
        // Tabs every 8 columns, the only interval exposed.
        while count > 0 && self.cursor_col + 1 < self.cols {
            self.cursor_col += 1;
            if self.cursor_col.is_multiple_of(8) {
                count -= 1;
            }
        }
        self.wrap_pending = false;
    }

    fn backspace(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        }
        self.wrap_pending = false;
    }

    fn carriage_return(&mut self) {
        self.cursor_col = 0;
        self.wrap_pending = false;
    }

    fn linefeed(&mut self) {
        self.linefeed();
        self.wrap_pending = false;
    }

    fn newline(&mut self) {
        self.linefeed();
        self.cursor_col = 0;
        self.wrap_pending = false;
    }

    fn reverse_index(&mut self) {
        if self.cursor_row == self.top_margin {
            self.scroll_region_down(1);
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
        }
        self.wrap_pending = false;
    }

    fn bell(&mut self) {
        // 语义平面不得读墙钟也不发声：只递增计数，宿主看变化量决定去重与播放。
        self.bell_seq += 1;
    }

    fn scroll_up(&mut self, count: usize) {
        self.scroll_region_up(count);
    }

    fn scroll_down(&mut self, count: usize) {
        self.scroll_region_down(count);
    }

    fn insert_blank_lines(&mut self, count: usize) {
        if self.cursor_row < self.top_margin || self.cursor_row > self.bottom_margin {
            return;
        }
        let start = self.cursor_row;
        let end = self.bottom_margin;
        for _ in 0..count.min(end.saturating_sub(start) + 1) {
            if end < self.grid.len() {
                self.grid.remove(end);
            }
            let at = start.min(self.grid.len());
            self.grid.insert(at, vec![Cell::default(); self.cols]);
        }
        self.wrap_pending = false;
    }

    fn delete_lines(&mut self, count: usize) {
        if self.cursor_row < self.top_margin || self.cursor_row > self.bottom_margin {
            return;
        }
        let start = self.cursor_row;
        let end = self.bottom_margin;
        for _ in 0..count.min(end.saturating_sub(start) + 1) {
            if start < self.grid.len() {
                self.grid.remove(start);
            }
            let at = end.saturating_sub(1).min(self.grid.len());
            self.grid.insert(at, vec![Cell::default(); self.cols]);
        }
        self.wrap_pending = false;
    }

    fn insert_blank(&mut self, count: usize) {
        self.insert_blank_at(count);
    }

    fn delete_chars(&mut self, count: usize) {
        self.delete_chars_at(count);
    }

    fn erase_chars(&mut self, count: usize) {
        let c = self.cursor_col;
        self.erase_area(self.cursor_row, c, self.cursor_row, c + count.saturating_sub(1));
    }

    fn clear_line(&mut self, mode: vte::ansi::LineClearMode) {
        let r = self.cursor_row;
        let last_col = self.cols.saturating_sub(1);
        match mode {
            vte::ansi::LineClearMode::Right => self.erase_area(r, self.cursor_col, r, last_col),
            vte::ansi::LineClearMode::Left => self.erase_area(r, 0, r, self.cursor_col),
            vte::ansi::LineClearMode::All => self.erase_area(r, 0, r, last_col),
        }
    }

    fn clear_screen(&mut self, mode: vte::ansi::ClearMode) {
        let last_row = self.rows.saturating_sub(1);
        let last_col = self.cols.saturating_sub(1);
        // 擦除/清历史都重设了光标语境，末列的欠换行不再成立。
        self.wrap_pending = false;
        match mode {
            vte::ansi::ClearMode::Below => {
                self.erase_area(self.cursor_row, self.cursor_col, last_row, last_col)
            }
            vte::ansi::ClearMode::Above => {
                self.erase_area(0, 0, self.cursor_row, self.cursor_col);
                if self.cursor_row == 0 {
                    self.drop_history();
                }
            }
            vte::ansi::ClearMode::All => {
                self.erase_area(0, 0, last_row, last_col);
                self.drop_history();
            }
            vte::ansi::ClearMode::Saved => self.drop_history(),
        }
    }

    fn terminal_attribute(&mut self, attr: vte::ansi::Attr) {
        use vte::ansi::Attr;
        match attr {
            Attr::Reset => self.pen = Pen::default(),
            Attr::Bold => self.pen.attrs.insert(Attrs::BOLD),
            Attr::Dim => self.pen.attrs.insert(Attrs::DIM),
            Attr::Italic => self.pen.attrs.insert(Attrs::ITALIC),
            Attr::Underline
            | Attr::DoubleUnderline
            | Attr::Undercurl
            | Attr::DottedUnderline
            | Attr::DashedUnderline => self.pen.attrs.insert(Attrs::UNDERLINE),
            Attr::BlinkSlow | Attr::BlinkFast => self.pen.attrs.insert(Attrs::BLINK),
            Attr::Reverse => self.pen.attrs.insert(Attrs::INVERSE),
            Attr::Hidden => self.pen.attrs.insert(Attrs::HIDDEN),
            Attr::Strike => self.pen.attrs.insert(Attrs::STRIKE),
            Attr::CancelBold => self.pen.attrs.remove(Attrs::BOLD),
            Attr::CancelBoldDim => self.pen.attrs.remove(Attrs::BOLD | Attrs::DIM),
            Attr::CancelItalic => self.pen.attrs.remove(Attrs::ITALIC),
            Attr::CancelUnderline => self.pen.attrs.remove(Attrs::UNDERLINE),
            Attr::CancelBlink => self.pen.attrs.remove(Attrs::BLINK),
            Attr::CancelReverse => self.pen.attrs.remove(Attrs::INVERSE),
            Attr::CancelHidden => self.pen.attrs.remove(Attrs::HIDDEN),
            Attr::CancelStrike => self.pen.attrs.remove(Attrs::STRIKE),
            Attr::Foreground(color) => self.pen.fg = colour_of(color),
            Attr::Background(color) => self.pen.bg = colour_of(color),
            Attr::UnderlineColor(_) => {}
        }
    }

    fn set_mode(&mut self, mode: vte::ansi::Mode) {
        if matches!(mode, vte::ansi::Mode::Named(vte::ansi::NamedMode::Insert)) {
            self.insert_mode = true;
        }
    }

    fn unset_mode(&mut self, mode: vte::ansi::Mode) {
        if matches!(mode, vte::ansi::Mode::Named(vte::ansi::NamedMode::Insert)) {
            self.insert_mode = false;
        }
    }

    fn set_private_mode(&mut self, mode: vte::ansi::PrivateMode) {
        use vte::ansi::NamedPrivateMode as P;
        // 鼠标档位统一按**数值**识别：vte 0.15 把 1000/1002/1003/1005/1006
        // 收成 Named，1015 仍走 Unknown(1015)。这样版本差异不会把编码档位
        // 悄悄丢进 private_mode_name 的 Err 分支里。
        if self.apply_mouse_decset(mode.raw(), true) {
            return;
        }
        let Ok(named) = private_mode_name(mode) else {
            return;
        };
        match named {
            P::ShowCursor => self.cursor_visible = true,
            P::LineWrap => self.autowrap = true,
            P::CursorKeys => self.application_cursor_keys = true,
            P::BlinkingCursor => self.cursor_blink = true,
            P::Origin => {
                self.origin_mode = true;
                self.cursor_row = self.top_margin;
                self.cursor_col = 0;
            }
            P::BracketedPaste => self.bracketed_paste = true,
            P::SwapScreenAndSetRestoreCursor => {
                self.save_cursor_position();
                self.enter_alt_screen();
            }
            // 鼠标相关的 Named 变体已由 apply_mouse_decset 处理；其余仍是无操作。
            P::ReportMouseClicks
            | P::ReportCellMouseMotion
            | P::ReportAllMouseMotion
            | P::Utf8Mouse
            | P::SgrMouse
            | P::ColumnMode
            | P::ReportFocusInOut
            | P::AlternateScroll
            | P::UrgencyHints
            | P::SyncUpdate => {}
        }
    }

    fn unset_private_mode(&mut self, mode: vte::ansi::PrivateMode) {
        use vte::ansi::NamedPrivateMode as P;
        if self.apply_mouse_decset(mode.raw(), false) {
            return;
        }
        let Ok(named) = private_mode_name(mode) else {
            return;
        };
        match named {
            P::ShowCursor => self.cursor_visible = false,
            P::LineWrap => self.autowrap = false,
            P::CursorKeys => self.application_cursor_keys = false,
            P::BlinkingCursor => self.cursor_blink = false,
            P::Origin => {
                self.origin_mode = false;
                self.cursor_row = self.cursor_row.min(self.rows - 1);
            }
            P::BracketedPaste => self.bracketed_paste = false,
            P::SwapScreenAndSetRestoreCursor => {
                self.leave_alt_screen();
                self.restore_cursor_position();
            }
            P::ReportMouseClicks
            | P::ReportCellMouseMotion
            | P::ReportAllMouseMotion
            | P::Utf8Mouse
            | P::SgrMouse
            | P::ColumnMode
            | P::ReportFocusInOut
            | P::AlternateScroll
            | P::UrgencyHints
            | P::SyncUpdate => {}
        }
    }

    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        // `vte` hands these over 1-based, straight from the sequence.
        let t = top.saturating_sub(1);
        let b = bottom.unwrap_or(self.rows).saturating_sub(1);
        if t < b && b < self.rows {
            self.top_margin = t;
            self.bottom_margin = b;
        } else {
            self.reset_region();
        }
        self.cursor_row = self.top_margin;
        self.cursor_col = 0;
        // 滚动区换了，末列欠着的换行不能跨区生效。
        self.wrap_pending = false;
    }

    fn save_cursor_position(&mut self) {
        self.saved = Some((self.cursor_row, self.cursor_col, self.pen));
    }

    fn restore_cursor_position(&mut self) {
        if let Some((r, c, pen)) = self.saved {
            self.cursor_row = r.min(self.rows.saturating_sub(1));
            self.cursor_col = c.min(self.cols.saturating_sub(1));
            self.pen = pen;
            self.wrap_pending = false;
        }
    }

    fn set_cursor_shape(&mut self, shape: vte::ansi::CursorShape) {
        self.cursor_shape = match shape {
            vte::ansi::CursorShape::Block => CursorShape::Block,
            vte::ansi::CursorShape::Underline => CursorShape::Underline,
            vte::ansi::CursorShape::Beam => CursorShape::Beam,
            vte::ansi::CursorShape::HollowBlock => CursorShape::HollowBlock,
            vte::ansi::CursorShape::Hidden => CursorShape::Block,
        };
        if matches!(shape, vte::ansi::CursorShape::Hidden) {
            self.cursor_visible = false;
        }
    }

    fn set_cursor_style(&mut self, style: Option<vte::ansi::CursorStyle>) {
        if let Some(style) = style {
            self.cursor_blink = style.blinking;
            self.set_cursor_shape(style.shape);
        }
    }

    fn device_status(&mut self, code: usize) {
        match code {
            5 => self.reply("\x1b[0n"),
            6 => {
                let row = if self.origin_mode {
                    // 光标可能落在滚动区上方（resize / 手动挪动）：饱和减，
                    // 不能让查询 panic，也不能绕回一个巨大行号。
                    self.cursor_row.saturating_sub(self.top_margin)
                } else {
                    self.cursor_row
                };
                self.reply(&format!("\x1b[{};{}R", row + 1, self.cursor_col + 1));
            }
            _ => {}
        }
    }

    fn identify_terminal(&mut self, _intermediate: Option<char>) {
        // Answer as a VT220 with VT320-level features: what a program running
        // under `TERM=xterm-256color` expects to hear.
        self.reply("\x1b[?62;1;2;6;9;15;18;21;22c");
    }

    fn text_area_size_chars(&mut self) {
        // `CSI 18 t` — vim and less ask for the size before they draw.
        self.reply(&format!("\x1b[8;{};{}t", self.rows, self.cols));
    }

    fn reset_state(&mut self) {
        self.pen = Pen::default();
        self.reset_region();
        self.autowrap = true;
        self.application_cursor_keys = false;
        self.origin_mode = false;
        self.bracketed_paste = false;
        self.mouse_flags = MouseFlags::default();
        self.mouse = MouseConfig::default();
        self.mouse_reporting = false;
        self.insert_mode = false;
        self.cursor_visible = true;
        self.cursor_shape = CursorShape::Block;
        self.saved = None;
        // ESC c 是全量复位：末列欠着的换行也一并作废。
        self.wrap_pending = false;
    }

    fn decaln(&mut self) {
        for row in &mut self.grid {
            for cell in row.iter_mut() {
                *cell = Cell {
                    ch: 'E',
                    ..Cell::default()
                };
            }
        }
    }

    fn clipboard_store(&mut self, _clipboard: u8, data: &[u8]) {
        // OSC 52 "put": hand the decoded text to the UI, which owns the
        // clipboard (a terminal emulator, not the shell, talks to the window
        // system).
        self.clipboard_request = Some(String::from_utf8_lossy(&base64_decode(data)).into_owned());
    }

    fn clipboard_load(&mut self, clipboard: u8, _target: &str) {
        // OSC 52 "query": the clipboard lives with the UI layer, so park the request
        // and let the frame handler answer it (see `answer_clipboard`).
        self.clipboard_query = Some(clipboard);
    }
}

impl Terminal {
    /// `ICH` (`ESC [ @`): push the cells under the cursor to the right.
    fn insert_blank_at(&mut self, count: usize) {
        let (r, c) = (self.cursor_row, self.cursor_col);
        if r >= self.rows || c >= self.cols {
            return;
        }
        let cols = self.cols;
        if let Some(row) = self.grid.get_mut(r) {
            let snapshot: Vec<Cell> = row[..cols.min(row.len())].to_vec();
            for i in c..cols {
                if i < row.len() {
                    row[i] = if i >= c + count {
                        snapshot.get(i - count).copied().unwrap_or_default()
                    } else {
                        Cell::default()
                    };
                }
            }
        }
    }

    /// `DCH` (`ESC [ P`): pull the cells after the cursor back over it.
    fn delete_chars_at(&mut self, count: usize) {
        let (r, c) = (self.cursor_row, self.cursor_col);
        if r >= self.rows || c >= self.cols {
            return;
        }
        let cols = self.cols;
        if let Some(row) = self.grid.get_mut(r) {
            let snapshot: Vec<Cell> = row[..cols.min(row.len())].to_vec();
            for i in c..cols {
                if i < row.len() {
                    row[i] = snapshot.get(i + count).copied().unwrap_or_default();
                }
            }
        }
    }

    /// Answer a parked `OSC 52` read with the text the UI layer last copied.
    pub fn answer_clipboard(&mut self, text: &str) {
        let Some(clipboard) = self.clipboard_query.take() else {
            return;
        };
        let encoded = base64_encode(text.as_bytes());
        self.reply(&format!("\x1b]52;{};{encoded}\x07", clipboard as char));
    }
}

impl Terminal {
    /// 按数值处理鼠标相关的 DECSET/DECRST。返回 `true` 表示这个模式已被消费。
    ///
    /// 回落规则：每一位独立记账，档位与编码都从 `MouseFlags` 重新推导——
    /// 关掉 1003 会自动落回仍开着的 1002/1000；关掉 1006 会落回仍开着的
    /// 1015/1005，全关则回到默认 X10。全关由 `reset_state` 一次性清零。
    fn apply_mouse_decset(&mut self, raw: u16, on: bool) -> bool {
        let handled = match raw {
            1000 => {
                self.mouse_flags.clicks = on;
                true
            }
            1002 => {
                self.mouse_flags.cell_motion = on;
                true
            }
            1003 => {
                self.mouse_flags.all_motion = on;
                true
            }
            1005 => {
                self.mouse_flags.utf8 = on;
                true
            }
            1006 => {
                self.mouse_flags.sgr = on;
                true
            }
            1015 => {
                self.mouse_flags.urxvt = on;
                true
            }
            _ => false,
        };
        if handled {
            self.mouse = self.mouse_flags.config();
            self.mouse_reporting = self.mouse.mode != MouseMode::Off;
        }
        handled
    }
}

/// `vte` wraps modes it does not know; unwrap the ones we act on.
fn private_mode_name(
    mode: vte::ansi::PrivateMode,
) -> Result<vte::ansi::NamedPrivateMode, vte::ansi::PrivateMode> {
    match mode {
        vte::ansi::PrivateMode::Named(n) => Ok(n),
        vte::ansi::PrivateMode::Unknown(u) => Err(vte::ansi::PrivateMode::Unknown(u)),
    }
}

fn colour_of(color: vte::ansi::Color) -> Colour {
    use vte::ansi::Color;
    match color {
        // The named colours are enum-ordered black..bright white, so the
        // discriminant is the palette index; `Foreground`/`Background` map back
        // to the theme defaults.
        Color::Named(named) => match named as u16 {
            0..=15 => Colour::Indexed(named as u8),
            _ => Colour::Default,
        },
        Color::Indexed(i) => Colour::Indexed(i),
        Color::Spec(rgb) => Colour::Rgb(rgb.r, rgb.g, rgb.b),
    }
}

// A small base64 codec, so OSC 52 works without another dependency.
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | b2 as u32;
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

pub fn base64_decode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &byte in data {
        if byte == b'=' {
            break;
        }
        let Some(pos) = B64.iter().position(|&c| c == byte) else {
            continue;
        };
        acc = (acc << 6) | pos as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(input: &str) -> Terminal {
        let mut t = Terminal::new(80, 24, 100);
        t.process(input.as_bytes());
        t
    }

    #[test]
    fn plain_text_lands_on_the_grid() {
        let t = feed("hello");
        assert_eq!(Terminal::row_text(&t.grid[0]), "hello");
        assert_eq!((t.cursor_row, t.cursor_col), (0, 5));
    }

    #[test]
    fn cursor_addressing_is_one_based_in_sequences() {
        let t = feed("\x1b[3;7Hx");
        assert_eq!(t.cell(2, 6).unwrap().ch, 'x');
    }

    /// 三种「清」是三条不同的路径：清屏不动历史、清历史不动屏幕、清全部两样都清。
    ///
    /// 这三条对应右键菜单的 `Clear Screen` / `Clear Scrollback` / `Clear All`——
    /// 内核层先钉死语义，UI 才不会把它做成同一个动作的三个名字。
    #[test]
    fn the_three_clear_paths_touch_different_things() {
        let build = || {
            let mut t = Terminal::new(20, 3, 10);
            t.process(b"one\r\ntwo\r\nthree\r\nfour");
            t
        };
        // 前提：这时确实有历史（否则下面三条断言都等于在测空集合）。
        assert_eq!(build().scrollback.len(), 1);

        // 只清屏：历史留着。
        let mut a = build();
        a.clear();
        assert_eq!(Terminal::row_text(&a.grid[0]), "");
        assert_eq!(a.scrollback.len(), 1, "清屏不该动历史");

        // 只清历史：屏幕留着。
        let mut b = build();
        let screen_before = Terminal::row_text(&b.grid[0]).to_string();
        b.clear_scrollback();
        assert!(b.scrollback.is_empty(), "历史应被丢光");
        assert_eq!(Terminal::row_text(&b.grid[0]), screen_before, "屏幕不该被动");

        // 两者都清。
        let mut c = build();
        c.clear_screen_and_scrollback();
        assert!(c.scrollback.is_empty());
        assert_eq!(Terminal::row_text(&c.grid[0]), "");
        assert_eq!((c.cursor_row, c.cursor_col), (0, 0), "光标回到原点");
    }

    #[test]
    fn sgr_256_and_truecolour_reach_the_cell() {
        let t = feed("\x1b[38;5;196mR\x1b[0m \x1b[38;2;1;2;3mT");
        assert_eq!(t.cell(0, 0).unwrap().fg, Colour::Indexed(196));
        assert_eq!(t.cell(0, 2).unwrap().fg, Colour::Rgb(1, 2, 3));
    }

    #[test]
    fn scrolling_keeps_history() {
        let mut t = Terminal::new(20, 3, 10);
        t.process(b"one\r\ntwo\r\nthree\r\nfour");
        assert_eq!(Terminal::row_text(&t.grid[0]), "two");
        assert_eq!(Terminal::row_text(t.scrollback.back().unwrap()), "one");
    }

    #[test]
    fn alternate_screen_restores_the_main_one() {
        let mut t = Terminal::new(20, 4, 10);
        t.process(b"main");
        t.process(b"\x1b[?1049halt");
        assert!(t.in_alt_screen());
        assert_eq!(Terminal::row_text(&t.grid[0]), "alt");
        t.process(b"\x1b[?1049l");
        assert!(!t.in_alt_screen());
        assert_eq!(Terminal::row_text(&t.grid[0]), "main");
    }

    #[test]
    fn queries_are_answered() {
        let mut t = feed("\x1b[6n\x1b[cx\x1b[18t");
        let reply = String::from_utf8(t.take_replies()).unwrap();
        assert!(reply.contains("\x1b[1;1R"), "{reply}");
        assert!(reply.contains("\x1b[?62;"), "{reply}");
        assert!(reply.contains("\x1b[8;24;80t"), "{reply}");
    }

    #[test]
    fn wide_glyph_occupies_two_cells() {
        let t = feed("中x");
        assert!(t.cell(0, 0).unwrap().wide);
        assert_eq!(t.cell(0, 2).unwrap().ch, 'x');
        assert_eq!(t.cursor_col, 3);
    }

    #[test]
    fn base64_round_trips() {
        assert_eq!(base64_encode(b"hello world"), "aGVsbG8gd29ybGQ=");
        assert_eq!(base64_decode(b"aGVsbG8gd29ybGQ="), b"hello world");
    }

    // ── 确定性语义────────────────────────────────────────────

    #[test]
    fn bell_has_no_observable_effect() {
        // BEL 从不进入快照；网格状态必须逐项一致。唯一的痕迹是给宿主看的
        // `bell_seq`，它不属于事实窗口，截图与金样本都不受影响。
        let with_bell = feed("hi\x07there");
        let without_bell = feed("hithere");
        assert_eq!(with_bell.bell_seq, 1);
        assert_eq!(without_bell.bell_seq, 0);
        assert_eq!(with_bell.screen_text(), without_bell.screen_text());
        assert_eq!(
            (with_bell.cursor_row, with_bell.cursor_col),
            (without_bell.cursor_row, without_bell.cursor_col)
        );
        assert_eq!(with_bell.title, without_bell.title);
        assert_eq!(with_bell.scrollback.len(), without_bell.scrollback.len());
    }

    #[test]
    fn same_byte_stream_yields_identical_state() {
        // 同一字节流重放两次，确定性字段逐项相同：无墙钟、无随机、无 IO。
        let bytes = b"\x1b[31mred\x1b[0m\r\nline2\x07\x1b[6n\x1b[2Jdone";
        let mut a = Terminal::new(20, 4, 10);
        let mut b = Terminal::new(20, 4, 10);
        a.process(bytes);
        b.process(bytes);
        assert_eq!(a.screen_text(), b.screen_text());
        assert_eq!((a.cursor_row, a.cursor_col), (b.cursor_row, b.cursor_col));
        assert_eq!(a.title, b.title);
        assert_eq!(a.scrollback.len(), b.scrollback.len());
        assert_eq!(a.take_replies(), b.take_replies());
        // 固定 version 下快照内容也应一致（version 由调用方注入，不是内核状态）。
        let sa = a.snapshot(7);
        let sb = b.snapshot(7);
        assert_eq!(sa.screen_text(), sb.screen_text());
        assert_eq!(sa.cursor, sb.cursor);
        assert_eq!(sa.version, sb.version);
    }

    // ── 按范围请求的只读事实窗口 ───────────────────────────

    #[test]
    fn slice_range_current_screen_matches_snapshot() {
        let t = feed("hello\r\nworld");
        let img = t.slice_range(7, 0..3);
        assert_eq!(img.version, 7);
        assert_eq!(img.cols, t.cols);
        assert_eq!(img.range, 0..3);
        assert_eq!(img.lines.len(), 3);
        assert_eq!(img.lines[0].text(), "hello");
        assert_eq!(img.lines[1].text(), "world");
        assert_eq!(img.cursor, Some((1, 5)));
    }

    #[test]
    fn slice_range_reads_scrollback_newest_last() {
        let mut t = Terminal::new(20, 3, 100);
        t.process(b"L1\r\nL2\r\nL3\r\nL4\r\nL5");
        assert!(t.scrollback.len() >= 2, "需要至少两行历史");
        let img = t.slice_range(9, -2..0);
        assert_eq!(img.lines.len(), 2);
        // -2 是较旧的一行，-1 是最新一行历史；按屏幕从上到下排列。
        let newest = Terminal::row_text(t.scrollback.back().unwrap());
        assert_eq!(img.lines[1].text(), newest);
        assert_ne!(img.lines[0].text(), img.lines[1].text());
    }

    #[test]
    fn slice_range_mixed_scrollback_and_screen() {
        let mut t = Terminal::new(20, 3, 100);
        t.process(b"L1\r\nL2\r\nL3\r\nL4\r\nL5");
        let img = t.slice_range(3, -1..2);
        assert_eq!(img.lines.len(), 3);
        assert_eq!(
            img.lines[0].text(),
            Terminal::row_text(t.scrollback.back().unwrap())
        );
        assert_eq!(img.lines[1].text(), Terminal::row_text(&t.grid[0]));
        assert_eq!(img.lines[2].text(), Terminal::row_text(&t.grid[1]));
        assert_eq!(img.range, -1..2);
    }

    #[test]
    fn slice_range_out_of_bounds_is_blank() {
        let t = feed("hi");
        let img = t.slice_range(1, 100..102);
        assert_eq!(img.lines.len(), 2);
        assert_eq!(img.lines[0].cells.len(), t.cols);
        assert!(img.lines[0].text().is_empty());
        assert!(img.lines[1].text().is_empty());
        assert_eq!(img.cursor, None);
    }

    #[test]
    fn slice_range_empty_has_no_lines() {
        let t = feed("hi");
        let img = t.slice_range(1, 0..0);
        assert!(img.lines.is_empty());
        assert_eq!(img.cursor, None);
        assert!(img.text().is_empty());
    }

    #[test]
    fn slice_range_keeps_wide_glyph_placeholder() {
        let t = feed("中x");
        let img = t.slice_range(1, 0..1);
        assert!(img.lines[0].cells[0].wide);
        assert_eq!(img.lines[0].cells[1].ch, ' ');
        assert_eq!(img.lines[0].cells[2].ch, 'x');
    }

    #[test]
    fn slice_range_cursor_only_inside_range() {
        let t = feed("hello");
        assert_eq!(t.slice_range(1, 0..2).cursor, Some((0, 5)));
        assert_eq!(t.slice_range(1, 2..4).cursor, None);
        assert_eq!(t.slice_range(1, -1..0).cursor, None);
    }

    #[test]
    fn slice_range_is_rebuildable() {
        // 投影被销毁（blank）后，用同一 version + range 重新请求事实，
        // 得到的图像与未销毁时逐项一致；重建不依赖任何投影侧状态。
        let mut t = Terminal::new(20, 3, 100);
        t.process(b"L1\r\nL2\r\nL3\r\nL4\r\nL5");
        let range = -1..3;
        let first = t.slice_range(42, range.clone());
        let mut destroyed = ScreenImage::blank(t.cols, 42, range.clone());
        assert_ne!(destroyed, first);
        destroyed = t.slice_range(42, range);
        assert_eq!(destroyed, first);
        assert_eq!(destroyed.version, 42);
    }

    #[test]
    fn slice_range_reports_history_length_for_clamping() {
        // 投影靠 `history` 钳制回看上限，不自己猜；仍只是数字，不含历史缓冲。
        let mut t = Terminal::new(20, 3, 100);
        t.process(b"L1\r\nL2\r\nL3\r\nL4\r\nL5");
        assert_eq!(t.scrollback.len(), 2, "前置：5 行内容在 3 行屏内应留 2 行历史");
        let img = t.slice_range(1, -2..2);
        assert_eq!(img.history, 2);
        // 越界回看仍返回空白行，但 history 不因此虚增。
        let far = t.slice_range(1, -50..-48);
        assert_eq!(far.history, 2);
        assert!(far.lines.iter().all(|l| l.text().is_empty()));
    }

    #[test]
    fn to_screen_image_matches_the_live_screen() {
        // 贴底渲染走 `GridSnapshot::to_screen_image()`，必须与原快照逐 cell 一致。
        let mut t = Terminal::new(12, 3, 100);
        t.process(b"abc\r\ndef");
        let snap = t.snapshot(7);
        let img = snap.to_screen_image();
        assert_eq!(img.range, 0..3);
        assert_eq!(img.version, 7);
        assert_eq!(img.history, 0);
        assert_eq!(img.text(), t.slice_range(7, 0..3).text());
        for row in 0..3 {
            let line = img.line_at(row as i64).expect("行必须存在");
            for col in 0..12 {
                assert_eq!(line.cells[col], snap.cell(row, col).unwrap());
            }
        }
    }

    #[test]
    fn screen_image_crop_refuses_windows_it_cannot_cover() {
        // 投影靠 crop 判断手上的事实够不够，不够就再请求，不硬画。
        let mut t = Terminal::new(20, 3, 100);
        t.process(b"L1\r\nL2\r\nL3\r\nL4\r\nL5");
        let img = t.slice_range(1, -2..3);
        let inside = img.crop(-1..2).expect("应能裁出子窗口");
        assert_eq!(inside.range, -1..2);
        assert_eq!(inside.lines.len(), 3);
        assert_eq!(inside.text(), "L2\nL3\nL4");
        assert!(img.crop(-3..1).is_none(), "越出窗口起点不得裁");
        assert!(img.crop(0..5).is_none(), "越出窗口终点不得裁");
    }

    #[test]
    fn screen_image_crop_drops_sub_window_cursor() {
        let mut t = Terminal::new(20, 3, 100);
        t.process(b"abc");
        let img = t.slice_range(1, 0..3);
        assert_eq!(img.cursor, Some((0, 3)));
        assert_eq!(img.crop(0..2).unwrap().cursor, Some((0, 3)));
        assert_eq!(img.crop(1..3).unwrap().cursor, None, "光标不在子窗口内就应消失");
    }

    #[test]
    fn screen_image_line_at_uses_absolute_rows() {
        let mut t = Terminal::new(20, 3, 100);
        t.process(b"L1\r\nL2\r\nL3\r\nL4\r\nL5");
        let img = t.slice_range(1, -2..2);
        assert_eq!(img.line_at(-2).map(|l| l.text()), Some("L1".to_string()));
        assert_eq!(img.line_at(-1).map(|l| l.text()), Some("L2".to_string()));
        assert_eq!(img.line_at(0).map(|l| l.text()), Some("L3".to_string()));
        assert!(img.line_at(2).is_none(), "超出请求范围的行不存在");
        assert!(img.line_at(-3).is_none());
    }

    // ------------------------------------------- 宽字符的文本口径

    #[test]
    fn cjk_rows_read_back_without_inserted_spaces() {
        // 宽字符占两格（第二格是占位），文本取只能取字形——否则「你好」
        // 读回来是「你 好」，复制和查找都会错。
        let mut t = Terminal::new(12, 2, 100);
        t.process("你好 world".as_bytes());
        assert_eq!(
            t.screen_text().lines().next(),
            Some("你好 world"),
            "行文本不得在汉字之间插空格"
        );
        let img = t.slice_range(1, 0..2);
        assert_eq!(img.text().lines().next(), Some("你好 world"));
        assert_eq!(Terminal::row_text(&t.grid[0]), "你好 world");
    }

    // ------------------------------------------------------ 粘贴编码

    #[test]
    fn paste_normalises_every_kind_of_line_break_to_cr() {
        // 终端要 CR；裸 LF 在 raw 模式下是 ^J，CRLF 若不合并会多出空行。
        assert_eq!(paste_bytes_for("a\nb", false), "a\rb");
        assert_eq!(paste_bytes_for("a\r\nb", false), "a\rb");
        assert_eq!(paste_bytes_for("a\rb", false), "a\rb", "已经是 CR 的不能翻倍");
        assert_eq!(paste_bytes_for("a\n\rb", false), "a\r\rb", "\\n\\r 是两个换行，不合并");
        assert_eq!(paste_bytes_for("a\n\nb", false), "a\r\rb");
    }

    #[test]
    fn paste_drops_control_characters_but_keeps_tabs() {
        // 剪贴板不可信：ESC 后面跟什么都等于键盘输入，整类丢掉。
        assert_eq!(paste_bytes_for("a\tb", false), "a\tb");
        assert_eq!(paste_bytes_for("a\x1b[201~rm -rf /\n", false), "a[201~rm -rf /\r");
        assert_eq!(paste_bytes_for("\x1b]0;evil\x07x", false), "]0;evilx");
        assert_eq!(paste_bytes_for("a\x7fb\x00c", false), "abc");
    }

    #[test]
    fn paste_wraps_only_when_bracketed_paste_is_on() {
        assert_eq!(paste_bytes_for("hi", true), "\x1b[200~hi\x1b[201~");
        assert_eq!(paste_bytes_for("hi\nthere", true), "\x1b[200~hi\rthere\x1b[201~");
        // 空文本不产生一对空括号。
        assert_eq!(paste_bytes_for("", true), "");
        assert_eq!(paste_bytes_for("", false), "");
    }

    #[test]
    fn terminal_paste_bytes_follows_decset_2004() {
        let mut t = Terminal::new(20, 3, 100);
        assert_eq!(t.paste_bytes("x\ny"), "x\ry", "默认没有括号粘贴");
        t.process(b"\x1b[?2004h");
        assert_eq!(t.paste_bytes("x\ny"), "\x1b[200~x\ry\x1b[201~");
        // 快照必须带上这个位，否则投影层没法自己编码（它没有 Terminal）。
        assert!(t.snapshot(1).bracketed_paste);
        t.process(b"\x1b[?2004l");
        assert!(!t.snapshot(1).bracketed_paste);
    }

    #[test]
    fn decset_mouse_modes_and_encodings_reach_the_snapshot() {
        let mut t = Terminal::new(20, 3, 100);
        let s = t.snapshot(1);
        assert!(!s.mouse_reporting);
        assert_eq!(s.mouse_mode, MouseMode::Off);
        assert_eq!(s.mouse_encoding, MouseEncoding::X10);

        // 1000 + 1006：按下/释放档 + SGR 编码，两者都要进快照。
        t.process(b"\x1b[?1000h\x1b[?1006h");
        let s = t.snapshot(2);
        assert!(s.mouse_reporting);
        assert_eq!(s.mouse_mode, MouseMode::Clicks);
        assert_eq!(s.mouse_encoding, MouseEncoding::Sgr);

        // 档位优先级 1003 > 1002 > 1000，关掉高档要回落到仍开着的那一档。
        t.process(b"\x1b[?1002h\x1b[?1003h");
        assert_eq!(t.snapshot(3).mouse_mode, MouseMode::AllMotion);
        t.process(b"\x1b[?1003l");
        assert_eq!(t.snapshot(4).mouse_mode, MouseMode::CellMotion);
        t.process(b"\x1b[?1002l");
        assert_eq!(t.snapshot(5).mouse_mode, MouseMode::Clicks);

        // 1015 在 vte 0.15 里是 Unknown(1015)：数值兜底必须认得。
        t.process(b"\x1b[?1006l\x1b[?1015h");
        assert_eq!(t.snapshot(6).mouse_encoding, MouseEncoding::Urxvt);
        t.process(b"\x1b[?1005h");
        assert_eq!(t.snapshot(7).mouse_encoding, MouseEncoding::Urxvt);
        t.process(b"\x1b[?1015l");
        assert_eq!(t.snapshot(8).mouse_encoding, MouseEncoding::Utf8);
        t.process(b"\x1b[?1005l");
        assert_eq!(t.snapshot(9).mouse_encoding, MouseEncoding::X10);

        // 关掉 1000 归零；RIS 把模式与编码一起清。
        t.process(b"\x1b[?1000l");
        let s = t.snapshot(10);
        assert!(!s.mouse_reporting);
        assert_eq!(s.mouse_mode, MouseMode::Off);
        t.process(b"\x1b[?1003h\x1b[?1006h");
        t.process(b"\x1bc");
        let s = t.snapshot(11);
        assert!(!s.mouse_reporting);
        assert_eq!(s.mouse_mode, MouseMode::Off);
        assert_eq!(s.mouse_encoding, MouseEncoding::X10);
    }

    #[test]
    fn osc133_marks_land_on_the_lines_they_were_emitted_on() {
        let mut t = Terminal::new(20, 4, 100);
        let mut bytes = Vec::new();
        bytes.extend(crate::osc133::emit('A', None));
        bytes.extend_from_slice(b"$ ");
        bytes.extend(crate::osc133::emit('B', None));
        bytes.extend_from_slice(b"ls\r\n");
        bytes.extend_from_slice(b"one\r\ntwo\r\n");
        bytes.extend(crate::osc133::emit('C', None));
        bytes.extend_from_slice(b"out\r\n");
        bytes.extend(crate::osc133::emit('D', Some("0")));
        t.process(&bytes);
        let blocks = t.command_blocks();
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        let b = blocks[0];
        // 提示符在第 0 行，命令在 0..2 行之间开始，输出到第 4 行收尾。
        assert_eq!(b.prompt_start, 0);
        assert_eq!(b.prompt_end, Some(0));
        assert!(b.command_start >= 2, "{b:?}");
        assert_eq!(b.exit, Some(0));
        // 快照把同一份块带给投影。
        assert_eq!(t.snapshot(1).blocks.len(), 1);
    }

    #[test]
    fn marks_inside_the_alternate_screen_are_not_recorded() {
        let mut t = Terminal::new(20, 4, 100);
        t.process(b"\x1b[?1049h");
        let mut bytes = crate::osc133::emit('A', None);
        bytes.extend(crate::osc133::emit('B', None));
        bytes.extend(crate::osc133::emit('C', None));
        bytes.extend(crate::osc133::emit('D', Some("0")));
        t.process(&bytes);
        assert!(t.command_blocks().is_empty(), "全屏程序里没有命令行");
    }

    #[test]
    fn trimming_the_scrollback_shifts_recorded_rows() {
        let mut t = Terminal::new(20, 3, 2);
        // 每条命令占 3 行，滚动后历史被裁到 2 行。
        for _ in 0..4 {
            let mut bytes = crate::osc133::emit('A', None);
            bytes.extend_from_slice(b"$ ");
            bytes.extend(crate::osc133::emit('B', None));
            bytes.extend_from_slice(b"x\r\n");
            bytes.extend(crate::osc133::emit('C', None));
            bytes.extend_from_slice(b"y\r\n\r\n");
            bytes.extend(crate::osc133::emit('D', Some("0")));
            t.process(&bytes);
        }
        let blocks = t.command_blocks();
        assert!(!blocks.is_empty());
        // 绝对行号非负且单调递增：裁剪后没有留下指向「已不存在历史」的行。
        for b in &blocks {
            assert!(b.prompt_start >= 0, "{b:?}");
            assert!(b.prompt_start <= b.command_start, "{b:?}");
        }
    }

    #[test]
    fn plain_output_without_shell_integration_yields_no_blocks() {
        let mut t = Terminal::new(20, 3, 100);
        t.process(b"just output\r\n");
        assert!(t.command_blocks().is_empty());
    }

    /// 所有清历史入口（菜单两处 + ED 1/2/3）都要把按绝对行号建的三张侧表
    /// 一起丢掉；只清 `scrollback` 会留下已失效的大行号，二分查找再也对不上。
    #[test]
    fn clearing_history_empties_blocks_times_and_wraps() {
        fn seeded() -> Terminal {
            let mut t = Terminal::new(20, 6, 100);
            // 先滚出几行历史，让后续标记的绝对行号都大于屏幕行号。
            for i in 0..10 {
                t.process(format!("seed{i}\r\n").as_bytes());
            }
            assert!(!t.scrollback.is_empty());
            t.process(b"\x1b[2;1H"); // row 1
            t.stamp_row(456);
            t.process(b"aaaaaaaaaaaaaaaaaaaaaaaaa"); // 20 列写 25 字：续行落在 row 2
            let mut bytes = crate::osc133::emit('A', None);
            bytes.extend_from_slice(b"$ ");
            bytes.extend(crate::osc133::emit('B', None));
            bytes.extend_from_slice(b"cmd\r\n");
            bytes.extend_from_slice(b"out\r\n");
            bytes.extend(crate::osc133::emit('C', None));
            bytes.extend_from_slice(b"more\r\n");
            bytes.extend(crate::osc133::emit('D', Some("0")));
            t.process(&bytes);
            assert!(!t.command_blocks().is_empty(), "前提：有可清的命令块");
            assert!(!t.row_times.is_empty(), "前提：有时间戳");
            assert!(!t.wrap_rows.is_empty(), "前提：有续行标记");
            t
        }
        fn assert_clean(t: &Terminal) {
            assert!(t.scrollback.is_empty());
            assert!(t.cmd_marks.is_empty());
            assert!(t.command_blocks().is_empty(), "清历史后不该留绝对行号块");
            assert!(t.row_times.is_empty(), "时间戳表必须一起清");
            assert!(t.wrap_rows.is_empty(), "续行表必须一起清");
        }

        let mut a = seeded();
        a.clear_scrollback();
        assert_clean(&a);
        let mut b = seeded();
        b.clear_screen_and_scrollback();
        assert_clean(&b);
        let mut c = seeded();
        c.process(b"\x1b[2J"); // ED 2: All
        assert_clean(&c);
        let mut d = seeded();
        d.process(b"\x1b[3J"); // ED 3: Saved
        assert_clean(&d);
        let mut e = seeded();
        e.process(b"\x1b[1;1H\x1b[1J"); // ED 1: Above, 光标在第 0 行
        assert_clean(&e);
    }

    /// 末列写满时欠着的换行不能跨过「语境重置」：复位 / 清屏 / 切屏 / 改滚动区
    /// 之后，下一字符必须写在当前位置的当前行。
    #[test]
    fn pending_wrap_does_not_survive_reset_clear_or_screen_changes() {
        fn filled_last_column() -> Terminal {
            let mut t = Terminal::new(10, 4, 20);
            t.process(b"abcdefghij"); // 正好写满第 0 行末列
            assert!(t.wrap_pending, "前提：换行欠账已挂起");
            t
        }

        let mut a = filled_last_column();
        a.process(b"\x1bc"); // ESC c / RIS
        assert!(!a.wrap_pending, "复位时欠账应清零");
        a.process(b"X"); // 写在原行末列；若欠账没清，这一字会先换行
        assert_eq!((a.cursor_row, a.cursor_col), (0, 9));
        assert_eq!(a.cell(0, 9).unwrap().ch, 'X');

        let mut b = filled_last_column();
        b.clear();
        b.process(b"X");
        assert!(!b.wrap_pending);
        assert_eq!((b.cursor_row, b.cursor_col), (0, 1));
        assert_eq!(b.cell(0, 0).unwrap().ch, 'X');

        let mut c = filled_last_column();
        c.process(b"\x1b[?1049hX"); // 进备用屏
        assert!(!c.wrap_pending);
        assert_eq!((c.cursor_row, c.cursor_col), (0, 1));
        assert_eq!(c.cell(0, 0).unwrap().ch, 'X');

        let mut d = filled_last_column();
        d.process(b"\x1b[?1049h");
        d.process(b"abcdefghij"); // 备用屏也写到末列
        assert!(d.wrap_pending);
        d.leave_alt_screen(); // 回主屏（保存的游标在末列）
        assert!(!d.wrap_pending, "切屏时欠账清零");
        d.process(b"X");
        assert_eq!((d.cursor_row, d.cursor_col), (0, 9));
        assert_eq!(d.cell(0, 9).unwrap().ch, 'X');

        let mut e = filled_last_column();
        e.process(b"\x1b[2;4rX"); // 滚动区改为 2..4 行，光标到 row 1
        assert!(!e.wrap_pending);
        assert_eq!((e.cursor_row, e.cursor_col), (1, 1));
    }

    #[test]
    fn cursor_position_report_saturates_above_the_scrolling_region() {
        let mut t = Terminal::new(10, 6, 20);
        t.origin_mode = true;
        t.top_margin = 3;
        t.cursor_row = 1; // 光标在滚动区上方：直接减会下溢
        t.process(b"\x1b[6n");
        assert_eq!(String::from_utf8(t.take_replies()).unwrap(), "\x1b[1;1R");
        t.cursor_row = 5;
        t.process(b"\x1b[6n");
        assert_eq!(String::from_utf8(t.take_replies()).unwrap(), "\x1b[3;1R");
    }
}


#[cfg(test)]
mod margin_data_tests {
    use super::*;
    use crate::osc133::emit;

    #[test]
    fn stamp_keeps_the_first_time_of_a_row() {
        // 一行是逐字符写出来的：时间戳认「行首」，后到的时刻不许覆盖它，
        // 否则「这一行什么时候开始输出」就变成了「什么时候写完」。
        let mut t = Terminal::new(20, 4, 100);
        t.stamp_row(100);
        t.process(b"abc");
        t.stamp_row(200);
        assert_eq!(t.row_times, vec![(0, 100)]);
        // 换行后是新行：要新的一条。
        t.process(b"\r\n");
        t.stamp_row(300);
        assert_eq!(t.row_times, vec![(0, 100), (1, 300)]);
    }

    #[test]
    fn wrap_marks_the_continuation_row_only() {
        // 20 列写 25 个字符：第 0 行写满，第 1 行是它的后半截 → 只有 1 是续行。
        let mut t = Terminal::new(20, 4, 100);
        t.process(b"aaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(t.wrap_rows, vec![1]);
        // 回车换行是**新逻辑行**，不是续行。
        t.process(b"\r\nbb\r\n");
        assert_eq!(t.wrap_rows, vec![1], "换行不该被记成折行");
    }

    #[test]
    fn trimming_history_shifts_times_and_wraps_by_the_same_rule_as_marks() {
        // 绝对行号在历史裁剪时整体左移；三张表必须同步，否则 margin 会把
        // A 行的时间贴到 B 行上。这里把 max_scrollback 压到 1 行来触发裁剪。
        let mut t = Terminal::new(20, 2, 1);
        for i in 1..=6 {
            t.stamp_row(i as u64);
            t.process(b"line\r\n");
        }
        t.process(emit('A', None).as_slice());
        // 屏幕只剩 2 行、历史只剩 1 行，任何残留的行号都必须 >= 0。
        assert!(t.row_times.iter().all(|(r, _)| *r >= 0), "{:?}", t.row_times);
        assert!(t.wrap_rows.iter().all(|r| *r >= 0), "{:?}", t.wrap_rows);
        assert!(t.cmd_marks.iter().all(|(r, _)| *r >= 0), "{:?}", t.cmd_marks);
        // 表里最后一条必须对得上当前光标所在行。
        let cursor_abs = t.scrollback.len() as i64 + t.cursor_row as i64;
        assert!(t.row_times.last().is_some_and(|(r, _)| *r <= cursor_abs));
    }

    #[test]
    fn clearing_scrollback_drops_the_side_tables() {
        let mut t = Terminal::new(20, 4, 100);
        t.stamp_row(1);
        t.process(b"aaaaaaaaaaaaaaaaaaaaaaaaa\r\n");
        t.clear_scrollback();
        assert!(t.row_times.is_empty());
        assert!(t.wrap_rows.is_empty(), "清历史后不该还留着续行标记");
    }

    #[test]
    fn alt_screen_records_neither_times_nor_wraps() {
        // 全屏程序（vim / less）没有「命令行」的概念，边栏不该跟着它跑。
        let mut t = Terminal::new(20, 4, 100);
        t.process(b"\x1b[?1049h");
        t.stamp_row(1);
        t.process(b"aaaaaaaaaaaaaaaaaaaaaaaaa");
        assert!(t.row_times.is_empty());
        assert!(t.wrap_rows.is_empty());
    }

    #[test]
    fn slice_view_matches_slice_range_for_negative_mixed_and_far_ranges() {
        use crate::coords::ViewRow;
        let mut t = Terminal::new(12, 4, 100);
        t.process(b"one\r\ntwo\r\nthree\r\n");
        // 回看、混跨、远端越界、空区间、远负区间五类都要比对。
        for (start, end) in [
            (-2i64, 2i64),
            (-1, 1),
            (0, 4),
            (100, 102),
            (0, 0),
            (-50, -48),
        ] {
            let raw = t.slice_range(9, start..end);
            let typed = t.slice_view(9, ViewRange::new(ViewRow::new(start), ViewRow::new(end)));
            assert_eq!(typed.range, raw.range, "start={start} end={end}");
            assert_eq!(typed.cols, raw.cols);
            assert_eq!(typed.version, raw.version);
            assert_eq!(typed.cursor, raw.cursor);
            assert_eq!(typed.history, raw.history);
            assert_eq!(typed.lines.len(), raw.lines.len());
            for (a, b) in typed.lines.iter().zip(raw.lines.iter()) {
                assert_eq!(a.row, b.row);
                assert_eq!(a.cells, b.cells);
            }
        }
    }

    #[test]
    fn slice_view_keeps_history_cursor_and_version() {
        use crate::coords::ViewRow;
        let mut t = Terminal::new(10, 3, 100);
        for i in 0..8 {
            t.process(format!("line{i}\r\n").as_bytes());
        }
        let view = ViewRange::new(ViewRow::new(-3), ViewRow::new(3));
        let img = t.slice_view(77, view.clone());
        // 版本由调用方注入，不被内核改写。
        assert_eq!(img.version, 77);
        // 历史行数来自 scrollback 本身，不是猜的。
        assert_eq!(img.history, t.scrollback.len());
        // 窗口与请求逐值一致，光标只在落在窗口内时才出现。
        assert_eq!(img.view_range(), view);
        assert_eq!(
            img.cursor.is_some(),
            view.contains(ViewRow::new(t.cursor_row as i64))
        );
    }

    #[test]
    fn mark_tables_stay_sorted_after_the_cursor_jumps_back() {
        let mut t = Terminal::new(10, 6, 100);
        // 先在高行号留下时间戳和续行标记。
        t.process(b"\x1b[4;1H"); // row 3
        t.stamp_row(200);
        t.process(b"aaaaaaaaaaaaaaa"); // 15 字：只在 row 3→4 折一次
        // 光标回跳到更前的行，再标记：只比对 last() 会把新行号追加到表尾。
        t.process(b"\x1b[2;1H"); // row 1
        t.stamp_row(100);
        t.process(b"bbbbbbbbbbbbbb"); // 换行到 row 2 → 续行标记 2

        let times: Vec<i64> = t.row_times.iter().map(|(r, _)| *r).collect();
        assert!(
            times.windows(2).all(|w| w[0] < w[1]),
            "时间戳必须严格升序: {times:?}"
        );
        assert_eq!(times.iter().filter(|&&r| r == 1).count(), 1, "同行不重复");
        assert!(
            t.wrap_rows.windows(2).all(|w| w[0] < w[1]),
            "续行表必须严格升序: {:?}",
            t.wrap_rows
        );
        assert_eq!(t.wrap_rows, vec![2, 4]);
        // 下游 grid.rs 的二分查找在回跳之后仍能命中真续行。
        assert!(crate::grid::is_continued(&t.wrap_rows, 2));
        assert!(crate::grid::is_continued(&t.wrap_rows, 4));
        assert!(!crate::grid::is_continued(&t.wrap_rows, 3));
    }

    #[test]
    fn slice_range_handles_i64_extreme_rows_without_overflow() {
        let t = Terminal::new(8, 4, 10);
        for (start, end) in [
            (i64::MIN, i64::MIN + 2),
            (i64::MAX - 2, i64::MAX),
            (-4i64, 2i64),
        ] {
            let img = t.slice_range(1, start..end);
            assert_eq!(img.lines.len(), (end - start) as usize);
            assert!(img.lines.iter().all(|l| l.text().is_empty()), "越界补白");
        }
        let far = t.slice_range(1, i64::MIN..i64::MIN + 2);
        assert_eq!(far.lines[0].row, i64::MIN);
        assert_eq!(far.cursor, None);
    }
}
