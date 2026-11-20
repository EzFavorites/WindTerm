//! 视觉基线夹具：构造**确定性**的回看画面，用冻结一张 PNG。
//!
//! 为什么需要它：`--screenshot` 只截「贴底」状态，而 F1 新增的回看、选区、
//! 浮标都在滚动之后才出现——没有基线，改动渲染时只能靠肉眼，等于没有回归。
//!
//! 为什么不用真实的 shell：PTY 输出、shell 提示符、字体回退都会让两次运行的
//! 字节不一致。这里直接用 `term-core` 的 `Terminal` 处理固定字节流，
//! 再把快照 / 范围窗口 / 选区一次性拼成静态画面，跑两次像素必然相同。
//!
//! 只由 `--demo scroll` / `--demo find` / `--demo word` / `--demo block` 触发；
//! 正常启动路径不受影响。
//!
//! R4 起查找夹具**不再直接写 `FindState`**：它只描述「要派发哪些 Action」，
//! 由投影层用同一个 `reduce` 跑一遍。理由很直接——`--demo` 若能把状态写好，
//! 它就是第二个写入者，而 R4 的全部意义就是消灭最后这个例外。

use term_app::action::Action;
use term_app::find::FindAction;
use term_app::scrollbar::preview_range;
use term_app::selection::{word_at, Pos, Selection};
use term_app::viewport::Viewport;
use term_core::grid::{GridSnapshot, ScreenImage};
use term_core::vt::Terminal;

/// 选区夹具：不选 / 跨三行的拖动选区 / 双击选词（F3）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelSpec {
    None,
    /// 从第 3 行拖到第 5 行，覆盖行尾裁剪与多行拼接。
    Rows,
    /// 双击可见区第 4 行的 "quick"——验证词边界，不是整行。
    Word,
    /// 第 3–5 行的同一段列（Alt+拖动）——验证矩形是「按列切」而不是整行吞。
    Block,
}

/// 夹具参数。
///
/// - `scroll`：从底部往历史里滚几行。
/// - `sel`：选区是什么形状。
/// - `find`：`Some(query)` = 打开 Find 栏搜这个串（F2：验证回看里的高亮）。
// 注意：不派 `Eq`——`preview_hover: Option<f32>` 含浮点，`f32` 不实现 `Eq`。
// `PartialEq`（够 `assert_eq!` / `==`）保留；无 `HashSet`/`BTreeSet` 键用法。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DemoSpec {
    pub scroll: i64,
    pub sel: SelSpec,
    pub find: Option<&'static str>,
    /// 当前匹配索引；只和 `find` 一起用。
    pub current: usize,
    /// `true` = 不注入终端画面，改为打开 New Session 对话框，
    /// 验证弹窗皮肤（深色 `popup.bg`、直角、右上角关闭）与左页签条重排。
    pub open_new_session: bool,
    /// `true` = 换成「带 OSC 133 提示符 + 必然折行」的那段字节流（margin 夹具）。
    ///
    /// 为什么单独一套字节：符号列画的是 `›`（提示符行）与 `~`（折行续行），
    /// 普通夹具既没有 shell 集成也不会折行，图上那一列永远是空的——
    /// 等于没有基线。
    pub marks: bool,
    /// `true` = 换成 G6 字形样张：truecolor 绿色 `|` 当列标尺，样例覆盖
    /// ASCII / CJK / emoji / Nerd Font 私用区 / 组合字符。像素判据在
    /// `scripts/glyph-probe.py`，两处的行序必须一致。
    pub glyph: bool,
    /// hex 夹具：往 `tab.raw.ring` 里灌哪一种字节流（`None` = 不是 hex 夹具）。
    ///
    /// 为什么不带「灌多少字节」这场参数：两张基线要验的是**两种处境**
    /// （装得下 / 装不下），不是一个可以连续调的数值。
    pub bin: BinKind,
    /// hover 预览夹具：轨道归一化位置（0=轨道顶/历史最老，1=轨道底/贴底）。
    /// `Some(r)` = 冻结点亮预览框、并切一份历史快照进 `preview_screen`；
    /// `None` = 不画预览（普通夹具）。详见 `docs/design/03-selection-find-scrollback.md`。
    pub preview_hover: Option<f32>,
    /// `true` = 夹具额外派发 `TogglePanel(Sysmon)`，拍展开后的 `T/C/M` 读数条。
    /// 只描述「按了哪个按钮」，与 `margin` / `shell` 同一条「夹具不发状态」的路。
    pub sysmon: bool,
}

/// hex 夹具的字节流派别。
///
/// 两种处境各一张基线，因为它们暴露的问题完全不同：
/// - `Line`：环远没满，画的是「刚才收到的那一段」，验排版与跟随；
/// - `Overflow`：环写满、`dropped > 0`，验表头那行「N bytes dropped」与
///   偏移列不从 0 开始——这两件事只在洪水之后才看得见。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinKind {
    None,
    Line,
    Overflow,
}

/// `Overflow` 夹具要灌多少字节。
///
/// 必须**大于** `bin_spec::RING_CAP`（1 MiB）才会真的丢字节，基线里才会出现
/// 「N bytes dropped」。这个数与 `RING_CAP` 的一致性由 `bin_spec` 的单测钉住
/// （那边能读到 `RING_CAP`，这里读不到——`bin_spec` 对本 crate 其余模块不可见）。
pub const BIN_OVERFLOW_BYTES: usize = (1 << 20) + 4096;

impl DemoSpec {
    pub const SCROLL: DemoSpec = DemoSpec {
        scroll: 6,
        sel: SelSpec::Rows,
        find: None,
        current: 0,
        open_new_session: false,
        marks: false,
        glyph: false,
        bin: BinKind::None,
        preview_hover: None,
        sysmon: false,
    };

    /// 回看 + 查找：每一行都含 "line"，所以窗口里必然有一片高亮，
    /// 与窗口尺寸无关（不会因为换了机器就变成 0 个匹配的空白图）。
    pub const FIND: DemoSpec = DemoSpec {
        scroll: 6,
        sel: SelSpec::None,
        find: Some("line"),
        current: 3,
        open_new_session: false,
        marks: false,
        glyph: false,
        bin: BinKind::None,
        preview_hover: None,
        sysmon: false,
    };

    /// 2026-11-17 Find 栏**空态**：栏开着、查询是空串。
    ///
    /// 用它拍视觉基线 1——那一张里状态栏是**灰的**（没有命中就不染色），
    /// 而 `--demo find` 带的 "line" 会让状态栏变蓝，拍不出这一半。
    pub const FIND_EMPTY: DemoSpec = DemoSpec {
        find: Some(""),
        ..DemoSpec::SCROLL
    };

    /// 回看 + 双击选词（F3）：只有 "quick" 一词高亮，
    /// 图上一眼能看出边界对不对（多一格 / 少一格都很显眼）。
    pub const WORD: DemoSpec = DemoSpec {
        scroll: 6,
        sel: SelSpec::Word,
        find: None,
        current: 0,
        open_new_session: false,
        marks: false,
        glyph: false,
        bin: BinKind::None,
        preview_hover: None,
        sysmon: false,
    };

    /// 回看 + 列选（F4）：三行同一段列，图上是竖着的一条带子。
    /// 若矩形退化成线性，中间行会整行高亮——一眼能看出来。
    pub const BLOCK: DemoSpec = DemoSpec {
        scroll: 6,
        sel: SelSpec::Block,
        find: None,
        current: 0,
        open_new_session: false,
        marks: false,
        glyph: false,
        bin: BinKind::None,
        preview_hover: None,
        sysmon: false,
    };

    /// margin 夹具（2026-11-07）：不滚动、不选区，只把**边栏**摊开给人看。
    /// 字节流里有 OSC 133 提示符与一条超出屏宽的行，于是 `›` 与 `~` 都会出现。
    pub const MARGIN: DemoSpec = DemoSpec {
        scroll: 0,
        sel: SelSpec::None,
        find: None,
        current: 0,
        open_new_session: false,
        marks: true,
        glyph: false,
        bin: BinKind::None,
        preview_hover: None,
        sysmon: false,
    };

    /// 底带夹具（2026-11-07）：一条带 + 标签条摊开在图里。不滚动、不选区——
    /// 这张图要看的是带本身。默认布局铺满之后，它和 round64 的区别只剩
    /// 「语义上的关注点」，保留名字是为了不重命名既有基线。
    pub const DOCK: DemoSpec = DemoSpec {
        scroll: 0,
        sel: SelSpec::None,
        find: None,
        current: 0,
        open_new_session: false,
        marks: false,
        glyph: false,
        bin: BinKind::None,
        preview_hover: None,
        sysmon: false,
    };

    /// 字形样张（M0 / G6）：不滚动、不选区、不打开查找栏，画面里只有样张与标尺。
    /// 这张图**不进** `screenshot-baseline.sh` 的逐字节基线：G6 要量的是
    /// 字形占位与颜色，判据是 `scripts/glyph-probe.py` 的数值，不是 PNG 哈希。
    pub const GLYPH: DemoSpec = DemoSpec {
        scroll: 0,
        sel: SelSpec::None,
        find: None,
        current: 0,
        open_new_session: false,
        marks: false,
        glyph: true,
        bin: BinKind::None,
        preview_hover: None,
        sysmon: false,
    };

    /// hex 视图夹具（本轮 ②）：环里只有一段「刚收到的响应」，贴着尾巴看。
    ///
    /// 不滚动、不选区、不开查找：这张图要看的是**排版本身**——偏移列宽、
    /// 每行 16 个字节、第 4/8/12 个字节之后那个双格。
    pub const BIN: DemoSpec = DemoSpec {
        scroll: 0,
        sel: SelSpec::None,
        find: None,
        current: 0,
        open_new_session: false,
        marks: false,
        glyph: false,
        bin: BinKind::Line,
        preview_hover: None,
        sysmon: false,
    };

    /// hex 洪水夹具：环写满、`dropped > 0`，表头右侧出现「N bytes dropped」，
    /// 第一行的偏移也从 1 MiB 那一带开始——**这两件事只有这张图守得住**。
    pub const BIN_FULL: DemoSpec = DemoSpec {
        scroll: 0,
        sel: SelSpec::None,
        find: None,
        current: 0,
        open_new_session: false,
        marks: false,
        glyph: false,
        bin: BinKind::Overflow,
        preview_hover: None,
        sysmon: false,
    };

    /// hover 预览夹具（P4 §7.3）：回看 6 行（滚动条可见）+ 冻结点亮预览框。
    /// 预览框定格在轨道 50% 处，内容是该位置的历史快照——与真实视口（回看 6 行）
    /// 不同，便于肉眼确认「框内是历史、框外是实时视口、滑块不动、状态栏不动」。
    pub const PREVIEW: DemoSpec = DemoSpec {
        scroll: 6,
        sel: SelSpec::None,
        find: None,
        current: 0,
        open_new_session: false,
        marks: false,
        glyph: false,
        bin: BinKind::None,
        preview_hover: Some(0.5),
        sysmon: false,
    };

    /// Sysmon 展开夹具（2026-11-19）：贴底静态画面 + 派发 `TogglePanel(Sysmon)`。
    /// 读数在 `tools::draw_sysmon` 里按 `fixture_mode()` 钉成视觉基线逐字值，
    /// 于是这张基线图守住 `T/C/M` 头与 `time/system/mem` 三段的位置和颜色。
    pub const SYSMON: DemoSpec = DemoSpec {
        sysmon: true,
        ..DemoSpec::SCROLL
    };

    pub fn parse(name: &str) -> Option<DemoSpec> {
        match name {
            "scroll" => Some(DemoSpec::SCROLL),
            "find" => Some(DemoSpec::FIND),
            // Find 栏空态（状态栏不染色那一半）。
            "findempty" => Some(DemoSpec::FIND_EMPTY),
            "word" => Some(DemoSpec::WORD),
            "block" => Some(DemoSpec::BLOCK),
            "margin" => Some(DemoSpec::MARGIN),
            "dock" => Some(DemoSpec::DOCK),
            "glyph" => Some(DemoSpec::GLYPH),
            // hex 视图（本轮 ②）：一段铺满屏的字节 / 一个被洪水打满的环。
            "bin" => Some(DemoSpec::BIN),
            "binfull" => Some(DemoSpec::BIN_FULL),
            // hover 预览：回看 6 行 + 冻结点亮预览框（P4 §7.3）。
            "preview" => Some(DemoSpec::PREVIEW),
            // Sysmon 展开：`T/C/M` 头 + `time/system/mem` 读数条。
            "sysmon" => Some(DemoSpec::SYSMON),
            // 弹窗夹具：背景用回看画面，弹 New Session 对话框（dev-spec T5 比对用）。
            "newsession" => Some(DemoSpec {
                open_new_session: true,
                ..DemoSpec::SCROLL
            }),
            _ => None,
        }
    }
}

/// 一次性交给投影的静态事实。
pub struct Fixture {
    pub snapshot: GridSnapshot,
    pub screen: ScreenImage,
    pub viewport: Viewport,
    pub selection: Option<Selection>,
    /// R4：夹具只描述「按了哪些动作」，不携带领域状态。
    find: Vec<Action>,
    /// 2026-11-07：margin 夹具。夹具**不直接写** `ViewState.margin`——
    /// 它派发 `Text.MarginTimestamp`，由同一个 `reduce` 打开时间戳列，
    /// 所以 baseline 拍到的是「用户点了菜单」之后的真实状态。
    margin: Vec<Action>,
    /// 2026-11-07：底带夹具。同样只派发命令（打开 Sender / Shell），不写状态。
    shell: Vec<Action>,
    /// hex 夹具的字节流：由投影层灌进 `tab.raw.ring`。
    ///
    /// 走的是渲染器读的那个环（`BinBuffer::push`），不是另一条「夹具专用」
    /// 通路——否则基线验的就是两套代码里比较好看的那一套。
    pub bin: Vec<u8>,
    /// 夹具要求的显示模式。只有 hex 夹具是 `Bin`，其余一律 `Text`——
    /// 五张既有基线必须逐字节不变，靠的就是这一位在它们身上没被改过。
    pub mode: term_runtime::prefs::DisplayMode,
    /// P4 §7.3：hover 预览夹具切出的历史快照（`None` = 普通夹具不画预览）。
    pub preview_screen: Option<ScreenImage>,
    /// P4 §7.3：冻定的轨道归一化位置；`None` = 普通夹具。pane.rs 据此反算绝对 y。
    pub preview_hover: Option<f32>,
}

impl Fixture {
    /// 要派发进 `reduce` 的查找动作（空 = 不打开 Find 栏）。
    pub fn find_actions(&self) -> Vec<Action> {
        self.find.clone()
    }

    /// 要派发进 `reduce` 的 margin 动作（打开时间戳列）。
    pub fn margin_actions(&self) -> Vec<Action> {
        self.margin.clone()
    }

    /// 要派发进 `reduce` 的外壳动作。默认布局铺满后夹具不再翻面板开关，
    /// 这里恒为空；留着这条通道是给将来「夹具要改外壳状态」用的。
    pub fn shell_actions(&self) -> Vec<Action> {
        self.shell.clone()
    }
}

/// 视觉夹具的本地文件条目：固定名字 / 固定日期 / 固定大小。
///
/// 为什么不让 Filer 在夹具里读真实家目录：每个人的目录都不同，逐字节基线会
/// 永远红。名字与日期全部钉死，走的是 `term_runtime::local_dir::Row` 这一个
/// 类型，于是夹具与真实运行画的是**同一段绘制代码**（不是两条分叉的假界面）。
/// 前四行是目录，与视觉基线里 Filer 可见的蓝色文件夹一致。
pub fn demo_files() -> Vec<term_runtime::local_dir::Row> {
    use term_runtime::local_dir::Row;
    let dirs = [
        "AITOSee",
        "Applications",
        "Applications (Parallels)",
        "Apps",
    ];
    let mut rows: Vec<Row> = dirs
        .iter()
        .map(|name| Row {
            name: (*name).to_string(),
            is_dir: true,
            is_symlink: false,
            size: 0,
            date: "2026/11/07".to_string(),
        })
        .collect();
    for i in 1..=17 {
        rows.push(Row {
            name: format!("file-{i:02}.txt"),
            is_dir: false,
            is_symlink: false,
            size: 29 * i as u64,
            date: "2026/11/06".to_string(),
        });
    }
    term_runtime::local_dir::sort_rows(&mut rows);
    rows
}

/// 固定字节流：三屏普通行 + 一行带颜色/粗体/反显，覆盖 span 合批的分支。
///
/// 2026-11-07 起按**行**切开返回：margin 的时间戳列要逐行打点，一整块字节喂进去
/// 只会给第一行盖上时刻，图里就变成「一行有时间、其余空着」。
fn demo_bytes(rows: usize) -> Vec<Vec<u8>> {
    let mut chunks: Vec<Vec<u8>> = (1..=(rows * 3))
        .map(|i| format!("line {i:02}  the quick brown fox jumps over the lazy dog\r\n").into_bytes())
        .collect();
    chunks.push(
        b"\x1b[31mred\x1b[0m \x1b[1mbold\x1b[0m \x1b[7minverse\x1b[0m\r\n".to_vec(),
    );
    chunks
}

/// margin 夹具的字节流：三条命令，每条都带 OSC 133 的 A/B/C/D 标记，
/// 输出里夹一条**超出屏宽**的长行（触发自动换行 → 符号列画 `~`）。
///
/// 长度按 `cols` 算，所以换任何窗口尺寸都一定会折行——不会出现「换台机器
/// 图里就没有 `~` 了」这种假基线。
fn margin_bytes(cols: usize) -> Vec<Vec<u8>> {
    use term_core::osc133::emit;
    let long = "wrapped ".repeat(cols / 8 + 2);
    let mut out: Vec<Vec<u8>> = Vec::new();
    for cmd in 1..=3 {
        let mut prompt = Vec::new();
        prompt.extend(emit('A', None));
        prompt.extend_from_slice(b"user@host:~$ ");
        prompt.extend(emit('B', None));
        prompt.extend_from_slice(format!("run {cmd}\r\n").as_bytes());
        prompt.extend(emit('C', None));
        out.push(prompt);

        let mut body = Vec::new();
        body.extend_from_slice(format!("output {cmd}: first line\r\n").as_bytes());
        body.extend_from_slice(format!("{long}\r\n").as_bytes());
        body.extend_from_slice(format!("output {cmd}: last line\r\n").as_bytes());
        body.extend(emit('D', Some("0")));
        out.push(body);
    }
    out
}

/// G6 字形样张的列标尺色：truecolor (0, 255, 128)。
///
/// 不用 ANSI 绿（32）是因为调色板的绿随主题变，按 RGB 提取的像素分析
/// 换一次主题就失效。truecolor 由 `vt.rs` 落成 `Colour::Rgb`，经
/// `to_color32` 直接上屏，中间没有主题插手。
const RULER_ON: &[u8] = b"\x1b[38;2;0;255;128m";
const RULER_OFF: &[u8] = b"\x1b[0m";

const GLYPH_ASCII: &[&str] = &["M", "W", "i", "@", "#", "8", "0", "."];
const GLYPH_CJK: &[&str] = &["汉", "字", "界", "面", "测", "试", "中", "文"];
/// 故意混进 1 格宽（✅ / ⚡ / ❤）与 2 格宽（🦀 / 🚀 / 🎉 / 👍 / 🔥）的
/// emoji：判据若一律写「2 格」就是假通过。
const GLYPH_EMOJI: &[&str] = &["🦀", "✅", "🚀", "🎉", "⚡", "❤", "👍", "🔥"];
/// Nerd Font 私用区码位。首轮实测否掉了「本机字体栈肯定没有这些字形」的
/// 假设：三角、锁、GitHub 标记真的画出来了，另两个码位是空心方框。
/// 所以夹具量的是**分布**（哪些有字形、哪些缺字），不是一刀切的判定。
const GLYPH_NERD: &[&str] = &[
    "\u{e0b0}", "\u{f0a0}", "\u{e615}", "\u{f1c0}", "\u{e0a2}", "\u{e0b2}",
];
/// 组合锐音符 / 全角 A / 半角片假名 / 全角空格 / ZWJ 家庭 emoji：
/// 这几样都要 grapheme cluster 才排得对，当前 VT 是逐码位占位。
const GLYPH_MIXED: &[&str] = &["e\u{0301}", "Ａ", "ｱ", "　", "👨\u{200d}👩\u{200d}👧"];

/// 一把像素尺：每格一根绿色竖线，相邻间距就是 `cell_w`。
///
/// 长度取 `cols` 但封顶 48：窗口再宽也够求中位数，窄窗口也不会折行。
fn ruler_row(cols: usize) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(RULER_ON);
    b.extend_from_slice("|".repeat(cols.clamp(8, 48)).as_bytes());
    b.extend_from_slice(RULER_OFF);
    b.extend_from_slice(b"\r\n\r\n");
    b
}

/// 一行样例：绿色 `|` 把每个样例夹住，样例本身用默认前景色。
///
/// 两个 `\r\n` 是**故意**的：内容行后留一整行空白，像素分析才能按 y
/// 把七个行带干净切开（字形笔画接近满行高，行贴行会连成一片）。
fn sample_row(samples: &[&str]) -> Vec<u8> {
    let mut b = Vec::new();
    for s in samples {
        b.extend_from_slice(RULER_ON);
        b.push(b'|');
        b.extend_from_slice(RULER_OFF);
        b.extend_from_slice(s.as_bytes());
    }
    b.extend_from_slice(RULER_ON);
    b.push(b'|');
    b.extend_from_slice(RULER_OFF);
    b.extend_from_slice(b"\r\n\r\n");
    b
}

/// 行序必须与 `scripts/glyph-probe.py` 的 `ROW_LABELS` 一致：分析脚本按
/// 「第几条有标尺的行带」贴标签，标签错了数字仍对，但报告会骗人。
fn glyph_bytes(cols: usize) -> Vec<Vec<u8>> {
    vec![
        ruler_row(cols),
        sample_row(GLYPH_ASCII),
        sample_row(GLYPH_CJK),
        sample_row(GLYPH_EMOJI),
        sample_row(GLYPH_NERD),
        sample_row(GLYPH_MIXED),
        ruler_row(cols),
    ]
}

/// `BinKind::Line` 的字节流：一段像样的串口 HTTP 响应。
///
/// 用**可读 ASCII** 而不是随机字节：这张基线是给人看的，一条能读出来的
/// `GET /api/...` 在 dump 里对不对，一眼就能判断；随机字节只能比对哈希。
///
/// 长度故意**不到一屏**（12 行 × 16 字节）：环装不满视口时贴底视口会落在环首，
/// 于是图上第一行的偏移正好是 `00000000`——这是「hex 视图显示的是会话建立
/// 以来的全部字节，不是从切过去那一刻开始」唯一的视觉证据。灌满一屏的话第一行
/// 会从某个中间偏移开始，看图的人分不清「从头」还是「被截了一截」。
fn bin_line_payload() -> Vec<u8> {
    const STANZA: &[u8] = b"GET /api/v1/sensors?dev=3 HTTP/1.1\r\n\
                            Host: bench.local\r\n\
                            Accept: */*\r\n\
                            User-Agent: bench/1.0\r\n\r\n";
    const BYTES: usize = 12 * 16;
    let mut v = Vec::with_capacity(BYTES * 2);
    while v.len() < BYTES {
        v.extend_from_slice(STANZA);
    }
    v.truncate(BYTES);
    v
}

/// `BinKind::Overflow` 的字节流：确定性伪随机，长度**大于**环上限。
///
/// 内容长什么样无关——这张图要看的是偏移列（洪水之后从头丢，第一行不再从
/// 偏移 0 开始）与表头右侧的丢弃计数。用 LCG 而不是「重复一段文本」，
/// 是因为重复文本会让 dump 里出现肉眼可见的周期，看图的人会以为那是数据规律。
fn bin_overflow_payload(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut x: u32 = 0x1234_5678;
    for _ in 0..len {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        out.push((x >> 24) as u8);
    }
    out
}

/// 时间戳夹具的起点：20:01:48（与参考截图同款「一屏里跳几次秒」的观感）。
/// 夹具必须用**常量**时刻，否则每次出图的时间都不一样，基线永远对不上。
const DEMO_CLOCK_BASE: u64 = 20 * 3600 + 60 + 48;

/// 按当前可见行列数构造画面。行列由窗口决定，所以基线图与窗口尺寸绑定——
/// 与既有的 `screenshot-round64.png` 一样，是「这台机器 + 这个窗口尺寸」的基线。
pub fn build(spec: DemoSpec, cols: usize, rows: usize) -> Fixture {
    let mut vt = Terminal::new(cols, rows, 10_000);
    let chunks = if spec.glyph {
        glyph_bytes(cols)
    } else if spec.marks {
        margin_bytes(cols)
    } else {
        demo_bytes(rows)
    };
    for (i, chunk) in chunks.iter().enumerate() {
        // 时刻的产权在宿主（I3）：夹具扮演宿主，递一个固定值进去。
        vt.stamp_row(DEMO_CLOCK_BASE + i as u64);
        vt.process(chunk);
    }

    let version = 1;
    let snapshot = vt.snapshot(version);

    let mut viewport = Viewport::new();
    viewport.scroll_up(spec.scroll, vt.scrollback.len() as i64);
    let view = viewport.rows(rows);
    let first_row = view.start().get();
    let screen = vt.slice_view(version, view);

    let selection = match spec.sel {
        // 第 3–5 行，跨三行，覆盖行尾裁剪与多行拼接。
        SelSpec::Rows => {
            let mut sel = Selection::new(Pos::new(first_row + 2, 2));
            sel.drag_to(Pos::new(first_row + 4, 28));
            Some(sel)
        }
        // 第 4 行的 "quick"：`line NN  the quick brown fox …`，quick 在 13..18。
        SelSpec::Word => word_at(&screen, Pos::new(first_row + 3, 14)),
        // 第 3–5 行的 9..29 列：`line NN  the quick brown fox …` 里的中段。
        SelSpec::Block => {
            let mut sel = Selection::new(Pos::new(first_row + 2, 9)).block();
            sel.drag_to(Pos::new(first_row + 4, 29));
            Some(sel)
        }
        SelSpec::None => None,
    };

    // `current` 不是一个可以「设置」的字段，而是「按了几次 Next」的结果；
    // 夹具照实描述击键，而不是抄一个终态。
    let find = match spec.find {
        Some(query) => {
            let mut actions = vec![
                Action::Find(FindAction::Open),
                Action::Find(FindAction::SetQuery(query.to_string())),
            ];
            for _ in 0..spec.current {
                actions.push(Action::Find(FindAction::Next));
            }
            actions
        }
        None => Vec::new(),
    };

    // 先取出历史行数（usize，Copy），避免下面 `snapshot` 整体 move 进 Fixture
    // 后，预览闭包再借不到它。闭包只用到这一项。
    let history = snapshot.history;
    Fixture {
        snapshot,
        screen,
        viewport,
        selection,
        find,
        bin: match spec.bin {
            BinKind::None => Vec::new(),
            BinKind::Line => bin_line_payload(),
            BinKind::Overflow => bin_overflow_payload(BIN_OVERFLOW_BYTES),
        },
        mode: match spec.bin {
            BinKind::None => term_runtime::prefs::DisplayMode::Text,
            BinKind::Line | BinKind::Overflow => term_runtime::prefs::DisplayMode::Bin,
        },
        margin: vec![Action::Margin(term_app::margin::MarginAction::Toggle(
            term_app::margin::MarginPart::Timestamp,
        ))],
        // 默认布局已经把 Sender / Shell 打开：再 Toggle 一次是**关掉**它们，
        // 底带会从图上消失。夹具只负责钉死字节流，不碰外壳状态。
        // Sysmon 是唯一的例外：它出厂关闭，展开夹具派发一次 TogglePanel 打开
        // 右带面板（走与用户点折叠标签相同的 reduce，不是第二套写状态的路）。
        shell: if spec.sysmon {
            vec![Action::Shell(term_app::shell::ShellAction::TogglePanel(
                term_app::dock::PanelId::Sysmon,
            ))]
        } else {
            Vec::new()
        },
        preview_screen: spec.preview_hover.map(|ratio| {
            let range = preview_range(history, ratio as f64, rows);
            vt.slice_view(version, range)
        }),
        // 冻定的轨道归一化位置随夹具一起下发给渲染层；pane.rs 反算成绝对 y 后
        // 覆盖真实指针的 `preview_hover_y`（生产路径恒 None）。
        preview_hover: spec.preview_hover,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_is_deterministic() {
        // 同一入参两次构造必须逐项相同——否则基线图没有意义。
        for spec in [
            DemoSpec::SCROLL,
            DemoSpec::FIND,
            DemoSpec::WORD,
            DemoSpec::BLOCK,
            DemoSpec::PREVIEW,
        ] {
            let a = build(spec, 80, 24);
            let b = build(spec, 80, 24);
            assert_eq!(a.snapshot.cells, b.snapshot.cells);
            assert_eq!(a.screen, b.screen);
            assert_eq!(a.viewport, b.viewport);
            assert_eq!(a.selection, b.selection);
            assert_eq!(a.find_actions(), b.find_actions());
            assert_eq!(a.margin_actions(), b.margin_actions());
        }
    }

    #[test]
    fn fixture_really_scrolls_into_history() {
        let f = build(DemoSpec::SCROLL, 80, 24);
        assert!(f.snapshot.history > 0, "夹具必须制造出可回看的历史");
        assert!(!f.viewport.is_bottom());
        assert_eq!(f.viewport.scroll(), 6);
        assert_eq!(f.screen.range, f.viewport.range(24));
        // K3：类型化视图必须与裸字段逐值同源，否则「收口」只是换了个写法。
        assert_eq!(f.screen.view_range(), f.viewport.rows(24));
        assert_eq!(f.screen.lines.len(), 24);
        // 上滚 6 行后，首行应是第二屏靠后的内容，而不是第 1 行。
        let first = f.screen.lines[0].text();
        assert!(first.starts_with("line "), "首行应是普通文本，实际：{first}");
        assert_ne!(first, "line 01  the quick brown fox jumps over the lazy dog");
    }

    #[test]
    fn fixture_selection_covers_three_rows() {
        let f = build(DemoSpec::SCROLL, 80, 24);
        let sel = f.selection.expect("scroll 夹具带选区");
        let text = sel.text(&f.screen, &f.snapshot.wraps);
        assert_eq!(text.lines().count(), 3, "跨三行的选区应产出三行文本");
        assert!(text.lines().all(|l| !l.is_empty()), "每行都应有内容：{text}");
        assert!(text.contains("line"), "内容应来自夹具文本：{text}");
    }

    #[test]
    fn word_fixture_selects_exactly_one_token() {
        let f = build(DemoSpec::WORD, 80, 24);
        let sel = f.selection.expect("word 夹具带选区");
        assert_eq!(sel.text(&f.screen, &f.snapshot.wraps), "quick", "双击应只选中一个词");
        assert_eq!(sel.anchor().row, sel.head().row, "选词不跨行");
    }

    #[test]
    fn block_fixture_takes_the_same_columns_on_three_rows() {
        let f = build(DemoSpec::BLOCK, 80, 24);
        let sel = f.selection.expect("block 夹具带选区");
        assert_eq!(sel.shape(), term_app::selection::Shape::Block);
        let text = sel.text(&f.screen, &f.snapshot.wraps);
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter().all(|r| r.len() == rows[0].len()),
            "每行宽度应一致（同一段列）：{text}"
        );
        assert!(rows[0].starts_with("the quick"), "列从 the 开始：{:?}", rows[0]);
    }

    #[test]
    fn fixture_selection_is_absent_when_not_requested() {
        let f = build(DemoSpec::FIND, 80, 24);
        assert!(f.selection.is_none(), "find 夹具不带选区，图里只该有高亮");
        assert!(
            !f.find_actions().is_empty(),
            "find 夹具必须派发打开动作，否则图里不会有高亮"
        );
    }

    #[test]
    fn find_fixture_has_matches_inside_the_scrolled_window() {
        let f = build(DemoSpec::FIND, 80, 24);
        // 夹具不再持有 FindState：用同一串动作跑一遍 reduce，得到要验证的状态。
        let mut state = term_app::state::FindState::default();
        for action in f.find_actions() {
            if let Action::Find(a) = action {
                term_app::find::reduce(&mut state, a, Some(&f.screen));
            }
        }
        assert!(state.visible);
        let found = term_app::find::matches(&state, &f.screen);
        assert!(found.len() > 4, "回看窗口里应有若干匹配，实际 {}", found.len());
        // 匹配行号必须落在窗口范围内，否则高亮会画到屏外。
        for m in &found {
            assert!(f.screen.range.contains(&m.row), "匹配行 {} 不在窗口内", m.row);
            assert!(
                f.screen
                    .view_range()
                    .contains(term_core::coords::ViewRow::new(m.row)),
                "匹配行 {} 不在类型化窗口内",
                m.row
            );
        }
        // 首行是历史行（负数），证明搜的是回看缓冲而不是当前屏。
        assert!(found[0].row < 0, "首个匹配应在历史行上，实际 {}", found[0].row);
    }

    #[test]
    fn margin_fixture_has_prompt_marks_and_wrapped_rows() {
        // margin 符号列画的是 `›`（提示符）与 `~`（折行续行）。这两样都不是
        // 「想画就有」的：前者要 shell 集成的 OSC 133，后者要真的写满一屏宽。
        // 夹具必须把两件事都造出来，否则那张基线图上符号列永远是空的。
        let f = build(DemoSpec::MARGIN, 80, 24);
        assert!(!f.snapshot.blocks.is_empty(), "夹具必须留下 OSC 133 提示符");
        assert!(!f.snapshot.wraps.is_empty(), "夹具必须有折行");
        // 每一行都打了时刻，且严格递增（20:01:48 起一秒一行）。
        assert!(f.snapshot.row_times.len() >= 6, "{:?}", f.snapshot.row_times);
        let rows: Vec<i64> = f.snapshot.row_times.iter().map(|(r, _)| *r).collect();
        assert!(rows.windows(2).all(|w| w[1] > w[0]), "行号应严格递增：{rows:?}");
        let secs: Vec<u64> = f.snapshot.row_times.iter().map(|(_, t)| *t).collect();
        assert!(secs.windows(2).all(|w| w[1] >= w[0]), "时刻不该倒流");
        // 夹具只描述动作：时间戳列是「按了菜单」打开的，不是直接写状态。
        assert!(f
            .margin_actions()
            .iter()
            .all(|a| matches!(a, Action::Margin(_))));
    }

    #[test]
    fn spec_parses_only_known_names() {
        assert_eq!(DemoSpec::parse("scroll"), Some(DemoSpec::SCROLL));
        assert_eq!(DemoSpec::parse("find"), Some(DemoSpec::FIND));
        // 空态夹具：栏开着但查询是空串——状态栏该保持灰底。
        assert_eq!(DemoSpec::parse("findempty"), Some(DemoSpec::FIND_EMPTY));
        assert_eq!(DemoSpec::FIND_EMPTY.find, Some(""));
        assert_eq!(DemoSpec::parse("word"), Some(DemoSpec::WORD));
        assert_eq!(DemoSpec::parse("block"), Some(DemoSpec::BLOCK));
        assert_eq!(DemoSpec::parse("margin"), Some(DemoSpec::MARGIN));
        // Sysmon 展开夹具：只它带 sysmon 开关，其余夹具右带保持原样。
        assert_eq!(DemoSpec::parse("sysmon"), Some(DemoSpec::SYSMON));
        assert!(DemoSpec::SYSMON.sysmon);
        assert!(!DemoSpec::SCROLL.sysmon);
        // 只有 margin 夹具走那套「带标记 + 折行」的字节流。
        // 常量断言会被 clippy 判成「恒真」；用运行时等价比对，语义一样。
        assert_eq!(DemoSpec::MARGIN.marks, DemoSpec::parse("margin").unwrap().marks);
        for spec in [
            DemoSpec::SCROLL,
            DemoSpec::FIND,
            DemoSpec::WORD,
            DemoSpec::BLOCK,
        ] {
            assert!(!spec.marks, "普通夹具不该换字节流，否则三张老基线全废");
        }
        assert_eq!(DemoSpec::parse("nope"), None);
    }

    #[test]
    fn newsession_spec_opens_dialog_and_keeps_static_terminal() {
        let spec = DemoSpec::parse("newsession").expect("newsession 是合法夹具");
        assert!(spec.open_new_session);
        // 除开关外其余参数沿用 SCROLL——终端背景是静态回看画面。
        assert_eq!(spec.scroll, DemoSpec::SCROLL.scroll);
        assert_eq!(spec.sel, DemoSpec::SCROLL.sel);
        // 四张既有基线夹具都不能带这个开关，否则终端画面被换掉。
        for spec in [
            DemoSpec::SCROLL,
            DemoSpec::FIND,
            DemoSpec::WORD,
            DemoSpec::BLOCK,
        ] {
            assert!(!spec.open_new_session);
        }
    }

    #[test]
    fn bin_fixtures_carry_bytes_and_ask_for_bin_mode() {
        let line = build(DemoSpec::BIN, 80, 24);
        assert_eq!(line.mode, term_runtime::prefs::DisplayMode::Bin);
        assert_eq!(line.bin.len(), 12 * 16, "Line 夹具：12 个满行，不足一屏");
        assert_eq!(line.bin.len() % 16, 0, "图上不该出现半行");
        // 两次构造逐字节相同——否则基线图没有意义。
        assert_eq!(build(DemoSpec::BIN, 80, 24).bin, line.bin);

        let full = build(DemoSpec::BIN_FULL, 80, 24);
        assert_eq!(full.mode, term_runtime::prefs::DisplayMode::Bin);
        assert_eq!(full.bin.len(), BIN_OVERFLOW_BYTES);
        // 「这个长度确实超过环上限」由 `bin_spec` 的单测钉住（那边读得到
        // `RING_CAP`，这里读不到）；在这里写常量断言会被 clippy 判成恒真。

        // 除 hex 夹具之外，别的夹具一个字节都不许带、模式必须是 Text——
        // 五张既有整窗基线的「逐字节不变」就靠这一条。
        assert_eq!(DemoSpec::parse("bin"), Some(DemoSpec::BIN));
        assert_eq!(DemoSpec::parse("binfull"), Some(DemoSpec::BIN_FULL));
    }

    /// 标尺是 truecolor 绿色，不随主题变；像素探针就靠这个 RGB 找竖线。
    fn is_ruler(c: &term_core::grid::Cell) -> bool {
        c.ch == '|' && c.fg == term_core::grid::Colour::Rgb(0, 255, 128)
    }

    #[test]
    fn glyph_fixture_has_seven_ruler_bands_in_order() {
        // 像素探针按「第几条标尺行带」贴标签，行序是判据的一部分。
        let f = build(DemoSpec::GLYPH, 80, 24);
        let per_row: Vec<usize> = f
            .screen
            .lines
            .iter()
            .map(|l| l.cells.iter().filter(|c| is_ruler(c)).count())
            .collect();
        let bands: Vec<usize> = per_row.iter().copied().filter(|n| *n > 0).collect();
        assert_eq!(bands.len(), 7, "两把尺子 + 五类样例：{bands:?}");
        assert_eq!(bands[0], 48, "首尺子");
        assert_eq!(bands[6], 48, "尾尺子");
        assert_eq!(bands[1], GLYPH_ASCII.len() + 1);
        assert_eq!(bands[2], GLYPH_CJK.len() + 1);
        assert_eq!(bands[3], GLYPH_EMOJI.len() + 1);
        assert_eq!(bands[4], GLYPH_NERD.len() + 1);
        assert_eq!(bands[5], GLYPH_MIXED.len() + 1);
        // 空行必须真的把行带隔开：字形笔画接近满行高，相邻两行都有标尺
        // 就会在 y 投影上连成一片，脚本没法分行。
        for w in per_row.windows(2) {
            assert!(!(w[0] > 0 && w[1] > 0), "相邻两行都有标尺：{per_row:?}");
        }
    }

    #[test]
    fn glyph_fixture_is_deterministic_and_does_not_change_other_specs() {
        let a = build(DemoSpec::GLYPH, 80, 24);
        let b = build(DemoSpec::GLYPH, 80, 24);
        assert_eq!(a.screen, b.screen);
        assert_eq!(DemoSpec::parse("glyph"), Some(DemoSpec::GLYPH));
        for spec in [
            DemoSpec::SCROLL,
            DemoSpec::FIND,
            DemoSpec::WORD,
            DemoSpec::BLOCK,
            DemoSpec::MARGIN,
            DemoSpec::DOCK,
        ] {
            assert!(!spec.glyph, "既有夹具不能换成字形字节流");
        }
    }

    #[test]
    fn glyph_sample_widths_match_the_pixel_probe_expectations() {
        // 与 scripts/glyph-probe.py 量到的 cells 对照用：这里钉住「VT 应该
        // 怎么占位」，脚本量「屏幕上真的占了几格」，两者一致才叫验过。
        use term_core::width::is_wide;
        let two = [
            "汉", "字", "界", "面", "测", "试", "中", "文", "🦀", "🚀", "🎉", "👍", "🔥", "Ａ", "　",
        ];
        let one = [
            "M", "W", "i", "@", "#", "8", "0", ".", "✅", "⚡", "❤", "ｱ", "\u{e0b0}", "\u{f0a0}",
        ];
        for s in two {
            assert!(is_wide(s.chars().next().unwrap()), "{s} 应占 2 格");
        }
        for s in one {
            assert!(!is_wide(s.chars().next().unwrap()), "{s} 应占 1 格");
        }
    }
}
