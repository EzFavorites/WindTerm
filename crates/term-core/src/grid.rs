//! 纯数据层：网格、颜色、属性与快照。
//!
//! 本模块（以及整个 `core/`）**不得依赖任何 UI 框架**：它必须能无头
//! 编译、无头测试。`scripts/verify.sh` 第 4 项用 grep 强制这条边界。

use crate::coords::{ViewRange, ViewRow};
use crate::mouse_report::{MouseEncoding, MouseMode};

/// `window.caretStyle` of `terminal/configs/session.config` — default "block".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
    HollowBlock,
}

/// Character attributes, one bit each. Hand-rolled: eight flags do not justify
/// a bitflags dependency.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Attrs(u8);

impl Attrs {
    pub const EMPTY: Attrs = Attrs(0);
    pub const BOLD: u8 = 1 << 0;
    pub const DIM: u8 = 1 << 1;
    pub const ITALIC: u8 = 1 << 2;
    pub const UNDERLINE: u8 = 1 << 3;
    pub const BLINK: u8 = 1 << 4;
    pub const INVERSE: u8 = 1 << 5;
    pub const HIDDEN: u8 = 1 << 6;
    pub const STRIKE: u8 = 1 << 7;

    #[inline]
    pub fn contains(self, flag: u8) -> bool {
        self.0 & flag != 0
    }
    #[inline]
    pub fn insert(&mut self, flag: u8) {
        self.0 |= flag;
    }
    #[inline]
    pub fn remove(&mut self, flag: u8) {
        self.0 &= !flag;
    }
    #[inline]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// A cell's colour: the theme's own default, one of the 256 palette entries, or
/// a literal `SGR 38;2;r;g;b` triple.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Colour {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Colour,
    pub bg: Colour,
    pub attrs: Attrs,
    /// A CJK glyph painted here fills this cell and the one after it.
    pub wide: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Colour::Default,
            bg: Colour::Default,
            attrs: Attrs::EMPTY,
            wide: false,
        }
    }
}


/// 与 UI 框架无关的 RGB 三元组。`core/theme.rs` 与 `core/vt.rs` 都用它，
/// 只有外壳层（之外）才把它转成绘图框架的颜色类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub const fn from_u32(v: u32) -> Self {
        Self {
            r: ((v >> 16) & 0xFF) as u8,
            g: ((v >> 8) & 0xFF) as u8,
            b: (v & 0xFF) as u8,
        }
    }

    pub const fn to_u32(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }
}

/// 一行单元格 `[from, to)` → 文本，跳过宽字符的**延续格**。
///
/// 网格按格存：一个汉字占两格（起始格带字形，第二格是占位空格）。如果照格
/// 逐个取 `ch`，`你好` 会变成 `"你 好"`——复制、查找、行文本全都错。
/// 这是唯一的取文本实现，`row_text` / `LineSlice::text` / `Selection` / 查找
/// 全部走它，避免再出现「某处按格、某处按字形」的第二套口径。
///
/// `from` 落在延续格上时该字形不输出（选区从半个宽字符中间开始时，
/// 那半个字形没有可复制的字符）。
pub fn cells_text_range(cells: &[Cell], from: usize, to: usize) -> String {
    let to = to.min(cells.len());
    let from = from.min(to);
    let mut out = String::with_capacity(to - from);
    let mut col = from;
    while col < to {
        if col > 0 && cells[col - 1].wide {
            col += 1; // 延续格：没有字形
            continue;
        }
        out.push(cells[col].ch);
        col += 1;
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// 一行的「字形 → 起始 cell 列」表，裁掉行尾空白。
///
/// `cols[i]` 是第 i 个字形的起始 cell 下标（宽字符占两格：字形格 + 占位格），
/// 行尾的空格不是真内容、不参与——这是查找与软换行共用的唯一字形口径。
/// 宽字符的占位格没有自己的字形，跳过。
///
/// 查找（`find::glyphs_and_cols`）、软换行（`wrap::glyph_widths`）、文本提取
/// (`cells_text_range`) 三处都按这条规则数格子；口径分叉的症状是「查找按字形、
/// 折行按 cell，各偏一列」，屏幕上只看到「折行位置怪怪的」，极难反推。
pub fn glyph_columns(cells: &[Cell]) -> Vec<usize> {
    let mut cols: Vec<usize> = Vec::with_capacity(cells.len());
    for col in 0..cells.len() {
        if col > 0 && cells[col - 1].wide {
            continue; // 宽字符的延续格：没有字形
        }
        cols.push(col);
    }
    // 从最后一个字形往回裁行尾空白（占位格已被跳过，不会在这里）。
    while cols.last().is_some_and(|&c| cells[c].ch.is_whitespace()) {
        cols.pop();
    }
    cols
}

/// 整行文本（行尾空白已去掉）。
pub fn cells_text(cells: &[Cell]) -> String {
    cells_text_range(cells, 0, cells.len())
}

/// Supervisor 发布给 UI 的只读快照：行主序、带版本号。
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct GridSnapshot {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
    pub cursor: (usize, usize),
    pub cursor_visible: bool,
    pub title: String,
    pub version: u64,
    /// 累计收到的字节数（状态栏 "Bytes received"）。
    pub bytes_received: u64,
    /// DECSET 1：方向键发 `ESC O A` 还是 `ESC [ A`。
    pub application_cursor_keys: bool,
    /// DECSET 2004：粘贴要包进 `ESC[200~ … ESC[201~`。
    /// 投影据此调用 `paste_bytes_for`——它手上只有快照，没有 `Terminal`。
    pub bracketed_paste: bool,
    /// DECSET 1000/1002/1003：程序是否要鼠标报告。
    /// 投影只有快照，没有 `Terminal`，手势编码全靠这三个字段。
    /// 与下面两项一样不进金样本 JSON（金样本只比对网格内容与样式）。
    pub mouse_reporting: bool,
    /// 报告档位（1000 / 1002 / 1003）。
    pub mouse_mode: MouseMode,
    /// 报告编码（X10 / 1005 / 1006 / 1015）。
    pub mouse_encoding: MouseEncoding,
    /// 是否处于备用屏（1049h）。
    pub alt: bool,
    /// OSC 133 命令块（绝对行号）。空表 = shell 没装集成。
    ///
    /// 行号是**绝对行**（从历史第一条算起）：投影用 `row - history` 换成
    /// 视口坐标；历史被裁剪时内核已整体左移，所以这里的值不会漂。
    /// 不进金样本 JSON（金样本只比对网格内容与样式）。
    pub blocks: Vec<crate::osc133::CommandBlock<i64>>,
    /// margin 时间戳列：绝对行号 → 这一段字节到达的 epoch 秒。
    ///
    /// 语义层不读钟，这张表是宿主（`term-runtime`）在写入前递进来的，
    /// 所以空表有两种合法含义：没有宿主（夹具 / 单测）或还没写入过。
    /// 与 `blocks` 一样不进金样本 JSON。
    pub row_times: Vec<(i64, u64)>,
    /// margin 符号列：由自动换行产生的续行的绝对行号（画 `~`）。
    pub wraps: Vec<i64>,
    /// 此刻可回看的历史行数。投影据此钳制滚动上限，
    /// 不需要额外一次范围请求去「探一下还有多少历史」。
    /// 不进金样本 JSON（金样本只比对网格内容与样式）。
    pub history: usize,
}

impl GridSnapshot {
    /// 连接尚未建立 / 已断开时的空网格，UI 仍能据此绘制。
    pub fn empty(cols: usize, rows: usize) -> Self {
        // 恶意/算错的尺寸不能让 `cols * rows` 溢出（release 下会静默截断成
        // 一块小得多的网格，debug 下直接 panic）。与 `Terminal::new` 同口径
        // 先钳到配置上限，再用 checked_mul 兜底：cells 长度与 cols/rows 自洽。
        let cols = cols.min(crate::config::MAX_COLS);
        let rows = rows.min(crate::config::MAX_ROWS);
        let len = cols.checked_mul(rows).unwrap_or(0);
        Self {
            cols,
            rows,
            cells: vec![Cell::default(); len],
            cursor: (0, 0),
            cursor_visible: true,
            title: String::new(),
            version: 0,
            bytes_received: 0,
            application_cursor_keys: false,
            bracketed_paste: false,
            mouse_reporting: false,
            mouse_mode: MouseMode::Off,
            mouse_encoding: MouseEncoding::X10,
            alt: false,
            history: 0,
            blocks: Vec::new(),
            row_times: Vec::new(),
            wraps: Vec::new(),
        }
    }

    pub fn screen_text(&self) -> String {
        (0..self.rows)
            .map(|r| self.row_text(r))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn cell(&self, row: usize, col: usize) -> Option<Cell> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        self.cells.get(row * self.cols + col).copied()
    }

    pub fn row_text(&self, row: usize) -> String {
        cells_text(self.row_cells(row).unwrap_or(&[]))
    }

    /// 一整行的只读切片（行主序内部连续的一段），越界返回 `None`。
    ///
    /// 查找引擎要逐行扫单元格，按行取切片比 `cell(r, c)` 逐格复制快，
    /// 也让「快照」和「事实窗口」能喂给同一个扫描器。
    pub fn row_cells(&self, row: usize) -> Option<&[Cell]> {
        if row >= self.rows || self.cols == 0 {
            return None;
        }
        let from = row * self.cols;
        self.cells.get(from..(from + self.cols).min(self.cells.len()))
    }

    /// 屏幕上的一个矩形区域，坐标**1 起、含端点**（SecureCRT `Screen.Get` 的形状）。
    ///
    /// 越界坐标**裁剪**而不是报错：CRT 的 `Get` 就是这样——脚本按 `Rows`/`Columns`
    /// 拼坐标时多算一行是常事，为此中断整段脚本不值。行间用 `\r\n`（CRT 同形），
    /// 每行行尾空白已去掉（`cells_text_range` 的唯一口径）。
    ///
    /// 返回空串有两种情况：区域完全落在屏外，或区域里只有空格。
    pub fn region_text(
        &self,
        row1: usize,
        col1: usize,
        row2: usize,
        col2: usize,
    ) -> String {
        self.region_rows(row1, col1, row2, col2)
            .join("\r\n")
    }

    /// 同上，但行间用 `\n`：脚本要 `split("\n")` 逐行处理时用它。
    pub fn region_lines(
        &self,
        row1: usize,
        col1: usize,
        row2: usize,
        col2: usize,
    ) -> String {
        self.region_rows(row1, col1, row2, col2).join("\n")
    }

    /// 区域内逐行的文本（已裁剪、已去行尾空白）。
    fn region_rows(
        &self,
        row1: usize,
        col1: usize,
        row2: usize,
        col2: usize,
    ) -> Vec<String> {
        if self.cols == 0 || self.rows == 0 {
            return Vec::new();
        }
        // 1 起的含端点坐标 → 0 起的半开区间；两个方向都允许（脚本写反了
        // 不该得到空串，那是 CRT 也不做的事）。
        let (r_from, r_to) = if row1 <= row2 { (row1, row2) } else { (row2, row1) };
        let (c_from, c_to) = if col1 <= col2 { (col1, col2) } else { (col2, col1) };
        // 0 是「第 0 行」——CRT 的坐标从 1 起，0 只能是算错了。这里与越界同样
        // 裁剪到 1，而不是返回空串让脚本以为「屏幕上什么都没有」。
        let r_from = r_from.max(1).min(self.rows);
        let r_to = r_to.max(1).min(self.rows);
        let c_from = c_from.max(1).min(self.cols);
        let c_to = c_to.max(1).min(self.cols);
        (r_from..=r_to)
            .filter_map(|row| {
                let cells = self.row_cells(row - 1)?;
                Some(cells_text_range(cells, c_from - 1, c_to))
            })
            .collect()
    }

    /// 把当前屏转成事实窗口的同一形状（`range = 0..rows`，无历史）。
    /// 投影只有一条渲染路径——永远读 `ScreenImage`。贴底时用它，
    /// 回看时用监管者返回的范围窗口；两者形状一致，绘制代码不需要分叉。
    pub fn to_screen_image(&self) -> ScreenImage {
        let lines = (0..self.rows)
            .map(|r| {
                let from = r * self.cols;
                let mut cells = if from < self.cells.len() {
                    self.cells[from..(from + self.cols).min(self.cells.len())].to_vec()
                } else {
                    Vec::new()
                };
                cells.resize(self.cols, Cell::default());
                LineSlice {
                    row: r as i64,
                    cells,
                }
            })
            .collect();
        ScreenImage {
            cols: self.cols,
            version: self.version,
            range: 0..self.rows as i64,
            lines,
            cursor: Some((self.cursor.0 as i64, self.cursor.1)),
            history: self.history,
        }
    }
}

/// 这一行的绝对行号在不在 `wraps` 里（= 它是自动换行的续行）。
///
/// `wraps` 是**历史绝对行号**的升序表：内核按写入顺序 push（见 `vt::mark_wrap`），
/// 所以二分查就够，不需要先排序、也不需要每次调用都建一张集合。
///
/// 判据住在这里而不是某个调用方，是因为它有**三**个消费者：
/// `ui/terminal/marks.rs::is_wrapped`（画行尾折行标记）、
/// `ui/terminal/margin.rs::is_wrapped`（画边栏的 `~`），
/// 以及选区提取文本（决定相邻两行之间该不该插 `\n`）。前两处的注释原先写着
/// 「两处必须同步」——把判据下沉之后这条注释债就消掉了：三处共用一句
/// `binary_search`，不可能再各自漂。
///
/// 被否掉的替代：给 `ScreenImage` 加一个 `wraps` 字段。`ScreenImage` 是
/// 「一屏多行的事实快照」（值类型契约，见它的文档），塞进另一张表的标记会让
/// 它同时携带两种坐标系的数据；而 `wraps` 已经随 `GridSnapshot` 送到投影手上，
/// 调用方原地多传一个 `&[i64]` 比扩宽事实窗口的字段面便宜得多。
pub fn is_continued(wraps: &[i64], abs_row: i64) -> bool {
    wraps.binary_search(&abs_row).is_ok()
}

/// 按行失效的集合；`full` 表示整屏重排（resize / 清屏 / 切换备用屏）。
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct DirtyRows {
    pub rows: Vec<usize>,
    pub full: bool,
}

impl DirtyRows {
    pub fn mark(&mut self, row: usize) {
        if !self.full && !self.rows.contains(&row) {
            self.rows.push(row);
        }
    }

    pub fn mark_all(&mut self) {
        self.full = true;
        self.rows.clear();
    }

    pub fn is_dirty(&self, row: usize) -> bool {
        self.full || self.rows.contains(&row)
    }

    pub fn clear(&mut self) {
        self.rows.clear();
        self.full = false;
    }
}

// ── 按范围请求的只读事实窗口 ──────────────────────────────

/// 一行拥有式切片。`row` 是相对当前可视区的行号：负数 = 回看，0..rows = 当前屏。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineSlice {
    pub row: i64,
    pub cells: Vec<Cell>,
}

impl LineSlice {
    pub fn text(&self) -> String {
        cells_text(&self.cells)
    }
}

/// 事实流中一次范围请求的返回值。**值类型**，不含锁、句柄或闭包。
/// `lines` 与 `range` 一一对应；越界行以空白补齐，投影无需知道内核的历史上限。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenImage {
    pub cols: usize,
    pub version: u64,
    pub range: std::ops::Range<i64>,
    pub lines: Vec<LineSlice>,
    /// 光标仅当落在请求范围内时存在；行号为相对行号。
    pub cursor: Option<(i64, usize)>,
    /// 内核此刻可回看的历史行数（`scrollback.len()`）。
    ///
    /// 投影用它钳制滚动上限，而不是自己猜一个数字；它仍是纯值，
    /// 不暴露历史缓冲本身（事实以值传递，不给共享句柄）。
    pub history: usize,
}

impl ScreenImage {
    /// 空投影：模拟「投影被销毁后尚未收到事实」的占位，不持有任何权威状态。
    pub fn blank(cols: usize, version: u64, range: std::ops::Range<i64>) -> Self {
        let lines = range
            .clone()
            .map(|row| LineSlice {
                row,
                cells: vec![Cell::default(); cols],
            })
            .collect();
        Self {
            cols,
            version,
            range,
            lines,
            cursor: None,
            history: 0,
        }
    }

    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 从本窗口里裁出 `range`。本窗口未完整覆盖时返回 `None`——
    /// 投影据此判断「手上的事实够不够画这一屏」，不够就再请求，而不是硬画。
    ///
    /// 注意下标假设：本方法按下标换算（`range.start - self.range.start`），只在
    /// **行号连续**的窗口上成立。一旦本值被 [`ScreenImage::drop_rows`] 过滤过，
    /// 这个假设就不成立了；过滤因此只允许作为渲染前的**最后一步**，此后不得再
    /// `crop`（详见 `drop_rows` 的文档）。
    pub fn crop(&self, range: std::ops::Range<i64>) -> Option<ScreenImage> {
        if range.start < self.range.start || range.end > self.range.end {
            return None;
        }
        let from = (range.start - self.range.start) as usize;
        let to = (range.end - self.range.start) as usize;
        let lines = self.lines.get(from..to)?.to_vec();
        Some(ScreenImage {
            cols: self.cols,
            version: self.version,
            range: range.clone(),
            lines,
            cursor: self.cursor.filter(|(r, _)| range.contains(r)),
            history: self.history,
        })
    }

    /// 按绝对行号取一行（含回看的负行号）。
    ///
    /// 注意下标假设：与 [`ScreenImage::crop`] 同一套（`row - self.range.start` 当
    /// 下标用），只在**行号连续**的窗口上成立。被 [`ScreenImage::drop_rows`] 过滤
    /// 过的窗口里行号是**带洞**的，这里会算到相邻的另一行上去——那种窗口请改用
    /// `lines.iter().find(|l| l.row == row)`（例如 `ui/terminal/pane.rs` 的
    /// `screen_index_of`：折叠之后它必须查表，减法一定会错行）。
    pub fn line_at(&self, row: i64) -> Option<&LineSlice> {
        // 极端窗口（start=i64::MIN / row=i64::MAX）的减法会溢出。换算统一走
        // coords 的 checked 入口：负偏移、溢出与 32 位截断一律给 None，
        // 不再在裸 i64 上自行相减。
        let index = crate::coords::content_row_to_screen_index(
            ViewRow::new(row),
            ViewRow::new(self.range.start),
        )?;
        self.lines.get(index)
    }

    /// 本窗口的视口行区间（类型化视图）。
    ///
    /// 字段 `range` 保持裸 `Range<i64>` 不变——它是金样本数据源的形状，按架构
    /// 蓝图 §8 冻结。这里只把它读成 [`ViewRange`]，不做任何换算，因此恒等于
    /// `ViewRange::new(ViewRow::new(range.start), ViewRow::new(range.end))`。
    pub fn view_range(&self) -> ViewRange {
        ViewRange::new(ViewRow::new(self.range.start), ViewRow::new(self.range.end))
    }

    /// [`ScreenImage::blank`] 的类型化入口：值域完全一致，只是不收裸区间。
    pub fn blank_view(cols: usize, version: u64, view: ViewRange) -> Self {
        Self::blank(cols, version, view.to_i64())
    }

    /// 回看时「事实在路上」那一帧的空白窗口：**内容与 [`blank_view`] 一致，
    /// 但保留真实 `history`**。
    ///
    /// `blank_view` 的 `history = 0` 只适合「投影刚起来、还没有任何权威状态」的
    /// 初始占位。拖动滚动条时用的是另一种空白：范围请求已经发出、还没回来，
    /// 这一帧正文确实没内容可画，但 `history` 是已知的事实。边栏行号 / 时间戳
    /// 按 `history + line.row` 取绝对行——若这里填 0，回看的负 `line.row` 会算出
    /// 负数行号、时间戳整列消失，下一帧真数据到了又恢复，于是拖动时左边栏
    /// 一闪一闪还带负数。带上真 history 后，空白帧与
    /// 真数据帧算出的绝对行号逐值相等，左边栏稳定。
    pub fn blank_view_with_history(
        cols: usize,
        version: u64,
        view: ViewRange,
        history: usize,
    ) -> Self {
        let mut blank = Self::blank_view(cols, version, view);
        blank.history = history;
        blank
    }

    /// [`ScreenImage::crop`] 的类型化入口：值域完全一致，只是不收裸区间。
    ///
    /// 越界判断与下标换算仍由 `crop` 一处实现，两者不会各自漂移。
    pub fn crop_view(&self, view: ViewRange) -> Option<ScreenImage> {
        self.crop(view.to_i64())
    }

    /// 丢掉命中 `hidden` 的行，返回**只看得到的那几行**。
    ///
    /// `range` / `version` / `cols` / `history` 原样保留，`lines` 是原来的子集，
    /// 因此**行号不再连续**：`lines[i].row != range.start + i`。
    /// 调用方一律用 `line.row` 取行号，不能再做减法（减法会指到相邻的另一行，
    /// 折叠一开整屏标记就会错行）。
    ///
    /// `cursor` 也原样保留：它说的是「光标在哪个内容行」，不是「第几根线」。
    /// 那一行若被过滤掉，画光标的一方查表会落空、于是不画——那才是诚实的，
    /// 把行号改成旁边的行等于指着另一行说「光标在这」。
    ///
    /// # 为什么刻意不接受 `&mut self`
    ///
    /// `ScreenImage` 是冻结的值类型（金样本数据源）。就地删行会让 [`ScreenImage::crop`]
    /// 的下标换算（`range.start` 偏移）当场失效——而 `crop` 还有别的调用方，
    /// 它们手上的窗口必须保持「行号连续、`lines.len() == range.len()`」。
    /// 所以过滤只能**产出新值**，且只能发生在**最后一次** `crop` 之后。
    pub fn drop_rows(&self, hidden: impl Fn(i64) -> bool) -> ScreenImage {
        ScreenImage {
            cols: self.cols,
            version: self.version,
            range: self.range.clone(),
            lines: self
                .lines
                .iter()
                .filter(|line| !hidden(line.row))
                .cloned()
                .collect(),
            cursor: self.cursor,
            history: self.history,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_round_trips_through_u32() {
        let c = Rgb::new(0x1C, 0x2D, 0x3E);
        assert_eq!(Rgb::from_u32(c.to_u32()), c);
        assert_eq!(c.to_u32(), 0x1C2D3E);
    }

    #[test]
    fn cells_text_skips_the_continuation_cell_of_a_wide_glyph() {
        // 内核的写法：中(0, wide) 占位(1) 文(2, wide) 占位(3) a(4) b(5)。
        let mut cells = vec![Cell::default(); 8];
        cells[0] = Cell {
            ch: '中',
            wide: true,
            ..Cell::default()
        };
        cells[2] = Cell {
            ch: '文',
            wide: true,
            ..Cell::default()
        };
        cells[4] = Cell {
            ch: 'a',
            ..Cell::default()
        };
        cells[5] = Cell {
            ch: 'b',
            ..Cell::default()
        };
        assert_eq!(cells_text(&cells), "中文ab", "延续格不得变成插入的空格");
        // 从延续格开始取：那个字形没有可输出的字符，跳过。
        assert_eq!(cells_text_range(&cells, 1, 6), "文ab");
        assert_eq!(
            cells_text_range(&cells, 0, 1),
            "中",
            "起点落在字形第一格就输出该字形（半格也带字）"
        );
        assert_eq!(cells_text_range(&cells, 4, 6), "ab");
    }

    #[test]
    fn snapshot_is_row_major() {
        let mut snap = GridSnapshot {
            cols: 3,
            rows: 2,
            cells: vec![Cell::default(); 6],
            cursor: (0, 0),
            cursor_visible: true,
            title: String::new(),
            version: 1,
            bytes_received: 0,
            application_cursor_keys: false,
            bracketed_paste: false,
            mouse_reporting: false,
            mouse_mode: MouseMode::Off,
            mouse_encoding: MouseEncoding::X10,
            alt: false,
            history: 0,
            blocks: Vec::new(),
            row_times: Vec::new(),
            wraps: Vec::new(),
        };
        snap.cells[4].ch = 'X';
        assert_eq!(snap.cell(1, 1).unwrap().ch, 'X');
        assert!(snap.cell(2, 0).is_none());
        assert_eq!(snap.row_text(1), " X");
    }

    #[test]
    fn dirty_rows_resets() {
        let mut d = DirtyRows::default();
        d.mark(3);
        d.mark(3);
        assert_eq!(d.rows, vec![3]);
        assert!(d.is_dirty(3));
        d.mark_all();
        assert!(d.is_dirty(0));
        d.clear();
        assert!(!d.is_dirty(0));
    }

    #[test]
    fn blank_view_with_history_keeps_absolute_rows_stable_for_scrollback() {
        // 拖动滚动条时的「事实在路上」帧：正文空白，但 history 是真值。
        // 边栏按 history + line.row 取绝对行——空白帧必须与真数据帧逐值相等，
        // 否则左边栏行号会闪出负数、时间戳整列消失。
        use crate::coords::{ViewRange, ViewRow};
        let view = ViewRange::new(ViewRow::new(-40), ViewRow::new(-16));
        let blank = ScreenImage::blank_view_with_history(80, 7, view.clone(), 100);
        assert_eq!(blank.history, 100);
        // blank_view 仍是 0（初始占位语义不变）。
        assert_eq!(ScreenImage::blank_view(80, 7, view).history, 0);
        // 负视口行 + 真 history => 非负绝对行号，与真窗口同源。
        for line in &blank.lines {
            let abs = blank.history as i64 + line.row;
            assert!(abs >= 0, "绝对行号不该为负：{abs}");
        }
        assert_eq!(blank.lines[0].row, -40);
    }

    #[test]
    fn view_range_is_derived_from_the_raw_field_for_negative_and_empty_windows() {
        for (start, end) in [(-7i64, 17i64), (-3, 21), (0, 0), (5, 4), (-24, 0)] {
            let img = ScreenImage::blank(4, 7, start..end);
            let typed = img.view_range();
            assert_eq!(typed.start().get(), start, "start={start}");
            assert_eq!(typed.end().get(), end, "end={end}");
            // 从类型化视图回到裸值，必须与字段本身逐值相同。
            assert_eq!(typed.to_i64(), img.range, "start={start} end={end}");
        }
    }

    #[test]
    fn blank_view_is_value_identical_to_blank() {
        for (start, end) in [(-2i64, 2i64), (0, 24), (-1, 0)] {
            let raw = ScreenImage::blank(6, 3, start..end);
            let typed = ScreenImage::blank_view(
                6,
                3,
                ViewRange::new(ViewRow::new(start), ViewRow::new(end)),
            );
            assert_eq!(typed.cols, raw.cols);
            assert_eq!(typed.version, raw.version);
            assert_eq!(typed.range, raw.range);
            assert_eq!(typed.cursor, raw.cursor);
            assert_eq!(typed.history, raw.history);
            // 逐行内容也要一致，避免只比了外壳。
            assert_eq!(typed.lines.len(), raw.lines.len());
            for (a, b) in typed.lines.iter().zip(raw.lines.iter()) {
                assert_eq!(a.row, b.row);
                assert_eq!(a.cells, b.cells);
            }
        }
    }

    #[test]
    fn crop_view_is_value_identical_to_crop_including_out_of_bounds() {
        let base = ScreenImage::blank(5, 11, -4..8);
        // 完全覆盖、部分覆盖、前端越界、尾端越界、空区间五类都要比对。
        for (start, end) in [(-4i64, 8i64), (-2, 3), (-5, 2), (0, 12), (2, 2)] {
            let raw = base.crop(start..end);
            let typed = base.crop_view(ViewRange::new(ViewRow::new(start), ViewRow::new(end)));
            assert_eq!(typed.is_some(), raw.is_some(), "start={start} end={end}");
            match (typed, raw) {
                (Some(t), Some(r)) => {
                    assert_eq!(t.range, r.range);
                    assert_eq!(t.lines.len(), r.lines.len());
                    assert_eq!(t.cursor, r.cursor);
                    assert_eq!(t.history, r.history);
                }
                (None, None) => {}
                _ => panic!("同源实现不应一边有一边无，start={start} end={end}"),
            }
        }
    }

    /// 什么都不藏时必须**逐字段**等于原值。
    ///
    /// 这条是「过滤不该顺手改别的」的守卫：`drop_rows` 是渲染前的最后一transform，
    /// 若它在顺手重建时丢了 `history` 或把 `range` 换成实际保留的行数，边栏与
    /// 滚动的上限就会跟着变——那是只在「有折叠」时才出现的第二套事实。
    #[test]
    fn drop_rows_keeping_everything_is_value_identical() {
        let base = ScreenImage::blank(3, 7, -2..2);
        let kept = base.drop_rows(|_| false);
        assert_eq!(kept.cols, base.cols);
        assert_eq!(kept.version, base.version);
        assert_eq!(kept.range, base.range);
        assert_eq!(kept.cursor, base.cursor);
        assert_eq!(kept.history, base.history);
        assert_eq!(kept.lines.len(), base.lines.len());
        for (a, b) in kept.lines.iter().zip(base.lines.iter()) {
            assert_eq!(a.row, b.row);
            assert_eq!(a.cells, b.cells);
        }
    }

    /// 中间挖洞之后，`lines[i].row` 的序列跳过了被藏的行——**这正是**下游
    /// 不能再做减法的原因：`lines[2].row` 不是 `range.start + 2`。
    #[test]
    fn drop_rows_leaves_a_hole_in_the_row_numbers() {
        let base = ScreenImage::blank(2, 9, 0..5);
        let kept = base.drop_rows(|row| row == 2);
        let rows: Vec<i64> = kept.lines.iter().map(|l| l.row).collect();
        assert_eq!(rows, vec![0, 1, 3, 4], "只少 2 那一行");
        // 减法会指到 2 去，而 2 已经不在窗口里了；查表才是唯一正确读法。
        assert_eq!(kept.lines[2].row, 3);
        assert_eq!(kept.line_at(2), Some(&kept.lines[2]), "减法口径：算到 3 那一行上");
    }

    /// `range` / `version` / `cols` / `history` 原样保留：被藏的行仍然算在窗口里，
    /// 窗口的身份（我请求的是哪一段、事实是第几个版本）不会因为画得少而改变。
    #[test]
    fn drop_rows_keeps_the_window_identity_and_history() {
        let mut base = ScreenImage::blank(4, 12, -3..2);
        base.history = 3;
        base.cursor = Some((-1, 0));
        let kept = base.drop_rows(|row| row < 0);
        assert_eq!(kept.range, -3..2, "窗口区间不变（不是 0..2）");
        assert_eq!(kept.version, 12);
        assert_eq!(kept.cols, 4);
        assert_eq!(kept.history, 3, "历史行数不变，边栏与滚动上限据此");
        assert_eq!(kept.cursor, Some((-1, 0)), "光标说「内容行」，不跟着行数走");
        let rows: Vec<i64> = kept.lines.iter().map(|l| l.row).collect();
        assert_eq!(rows, vec![0, 1], "负行号全被藏掉");
        // `line_at` 的前提（行号连续）在这里已不成立：查 0 那一行会按下标
        // `0 - (-3) = 3` 去取，落空——这正是「过滤之后一律查表」的理由。
        assert_eq!(kept.line_at(0), None, "减法口径在带洞窗口上会落空");
    }

    /// 造一块 4 列 × 3 行的屏：`ab` / `cdef` / 空行。
    fn region_fixture() -> GridSnapshot {
        let mut snap = GridSnapshot::empty(4, 3);
        for (i, ch) in "ab".chars().enumerate() {
            snap.cells[i] = Cell { ch, ..Cell::default() };
        }
        for (i, ch) in "cdef".chars().enumerate() {
            snap.cells[4 + i] = Cell { ch, ..Cell::default() };
        }
        snap
    }

    #[test]
    fn region_text_uses_one_based_inclusive_coordinates() {
        let snap = region_fixture();
        // 第 1 行第 1 列到第 2 行第 2 列 = "ab" + CRLF + "cd"
        assert_eq!(snap.region_text(1, 1, 2, 2), "ab\r\ncd");
        assert_eq!(snap.region_lines(1, 1, 2, 2), "ab\ncd");
        // 单行单列：只取一个字符（第 2 行是 "cdef"，第 2 列是 `d`）。
        assert_eq!(snap.region_text(2, 2, 2, 2), "d");
    }

    #[test]
    fn region_text_clamps_instead_of_failing_on_out_of_range() {
        let snap = region_fixture();
        // 行号大过屏幕 / 列号大过宽度：裁剪到边界，不报错（与 CRT 的 Get 一致）。
        assert_eq!(snap.region_text(2, 1, 99, 99), "cdef\r\n");
        // 写反了坐标：按 min/max 归一，不返回空串。
        assert_eq!(snap.region_text(2, 2, 1, 1), "ab\r\ncd");
        // 0 起坐标被裁剪到第 1 行/列（脚本算错一格的常见形态）。
        assert_eq!(snap.region_lines(0, 0, 1, 2), "ab");
    }

    #[test]
    fn region_text_is_empty_when_the_rectangle_is_all_spaces() {
        let snap = region_fixture();
        assert_eq!(snap.region_text(3, 1, 3, 4), "", "空行取出来是空串，不是四个空格");
    }

    #[test]
    fn empty_snapshot_clamps_oversized_dimensions_without_overflow() {
        // 任一维是 usize::MAX 时 cols*rows 会溢出；钳到内核上限后
        // cols/rows 与 cells 长度仍必须自洽。
        let wide = GridSnapshot::empty(usize::MAX, 4);
        assert_eq!(wide.cols, crate::config::MAX_COLS);
        assert_eq!(wide.rows, 4);
        assert_eq!(wide.cells.len(), crate::config::MAX_COLS * 4);
        assert!(wide.cell(3, crate::config::MAX_COLS - 1).is_some());
        let tall = GridSnapshot::empty(2, usize::MAX);
        assert_eq!(tall.rows, crate::config::MAX_ROWS);
        assert_eq!(tall.cells.len(), 2 * crate::config::MAX_ROWS);
        assert!(tall.cell(crate::config::MAX_ROWS - 1, 1).is_some());
    }

    #[test]
    fn line_at_handles_extreme_row_numbers_without_overflow() {
        // row - range.start 在两头都可能溢出；越界一律返回 None。
        let img = ScreenImage::blank(2, 1, 0..3);
        assert_eq!(img.line_at(i64::MIN), None);
        assert_eq!(img.line_at(i64::MAX), None);
        assert!(img.line_at(2).is_some());

        let near_max = ScreenImage::blank(2, 1, i64::MAX - 1..i64::MAX);
        assert_eq!(near_max.line_at(i64::MIN), None);
        assert!(near_max.line_at(i64::MAX - 1).is_some());

        let near_min = ScreenImage::blank(2, 1, i64::MIN..i64::MIN + 2);
        assert_eq!(near_min.line_at(i64::MAX), None);
        assert!(near_min.line_at(i64::MIN).is_some());
    }
}
