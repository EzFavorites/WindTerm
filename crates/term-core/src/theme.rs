//! Colours and metrics, taken from reference UI screenshots and the project's own
//! layout measurements rather than a bundled proprietary theme.

use crate::grid::Rgb;
use serde::Deserialize;

/// `terminal.ansi*` of the bundled `global/themes/dige-black/scheme.theme`.
pub const ANSI_NAMES: [&str; 16] = [
    "terminal.ansiBlack",
    "terminal.ansiRed",
    "terminal.ansiGreen",
    "terminal.ansiYellow",
    "terminal.ansiBlue",
    "terminal.ansiMagenta",
    "terminal.ansiCyan",
    "terminal.ansiWhite",
    "terminal.ansiBrightBlack",
    "terminal.ansiBrightRed",
    "terminal.ansiBrightGreen",
    "terminal.ansiBrightYellow",
    "terminal.ansiBrightBlue",
    "terminal.ansiBrightMagenta",
    "terminal.ansiBrightCyan",
    "terminal.ansiBrightWhite",
];

/// Fallback palette = the same dige-black values, used when the theme file is absent.
pub const ANSI_FALLBACK: [u32; 16] = [
    0x333333, 0xC4265E, 0x86B42B, 0xD0A500, 0x3465A4, 0x8C6BC8, 0x56ADBC, 0xE3E3DD, 0x666666,
    0xF92672, 0xA6E22E, 0x9E862F, 0x819AFF, 0xAE81FF, 0x66D9EF, 0xF8F8F2,
];

/// `text.default` of the same theme file.
pub const DEFAULT_FG: u32 = 0xF8F8F2;
pub const DEFAULT_BG: u32 = 0x1C1C1C;

/// WindTerm 内置终端配色方案名，与 `Terminal.SchemeList` 弹层 7 项一一对应。
pub const BUILTIN_SCHEME_NAMES: &[&str] = &[
    "cmd", "dumb", "linux", "macos", "none", "powershell", "routeros",
];

/// 各内置方案的 16 色 ANSI 调色板（RGB 三元组，顺序 0..=15 = 黑 / 红 / 绿 / 黄 / 蓝 /
/// 品红 / 青 / 白 + 各自的亮色）。取值为对应终端类型的标准调色板：
/// - `cmd` / `powershell`：Windows 控制台默认 16 色
/// - `linux` / `routeros`：VGA 文本模式默认调色板
/// - `macos`：macOS Terminal「Basic」默认 ANSI 色
/// - `none`：xterm 默认 16 色
/// - `dumb`：单色灰阶（哑终端无彩色能力）
const SCHEME_CMD: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00], [0x80, 0x00, 0x00], [0x00, 0x80, 0x00], [0x80, 0x80, 0x00],
    [0x00, 0x00, 0x80], [0x80, 0x00, 0x80], [0x00, 0x80, 0x80], [0xC0, 0xC0, 0xC0],
    [0x80, 0x80, 0x80], [0xFF, 0x00, 0x00], [0x00, 0xFF, 0x00], [0xFF, 0xFF, 0x00],
    [0x00, 0x00, 0xFF], [0xFF, 0x00, 0xFF], [0x00, 0xFF, 0xFF], [0xFF, 0xFF, 0xFF],
];
const SCHEME_DUMB: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00], [0x11, 0x11, 0x11], [0x22, 0x22, 0x22], [0x33, 0x33, 0x33],
    [0x44, 0x44, 0x44], [0x55, 0x55, 0x55], [0x66, 0x66, 0x66], [0x77, 0x77, 0x77],
    [0x88, 0x88, 0x88], [0x99, 0x99, 0x99], [0xAA, 0xAA, 0xAA], [0xBB, 0xBB, 0xBB],
    [0xCC, 0xCC, 0xCC], [0xDD, 0xDD, 0xDD], [0xEE, 0xEE, 0xEE], [0xFF, 0xFF, 0xFF],
];
const SCHEME_LINUX: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00], [0xAA, 0x00, 0x00], [0x00, 0xAA, 0x00], [0xAA, 0x55, 0x00],
    [0x00, 0x00, 0xAA], [0xAA, 0x00, 0xAA], [0x00, 0xAA, 0xAA], [0xAA, 0xAA, 0xAA],
    [0x55, 0x55, 0x55], [0xFF, 0x55, 0x55], [0x55, 0xFF, 0x55], [0xFF, 0xFF, 0x55],
    [0x55, 0x55, 0xFF], [0xFF, 0x55, 0xFF], [0x55, 0xFF, 0xFF], [0xFF, 0xFF, 0xFF],
];
const SCHEME_MACOS: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00], [0xC2, 0x36, 0x21], [0x25, 0xBC, 0x24], [0xAD, 0xAD, 0x27],
    [0x49, 0x2E, 0xE1], [0xD3, 0x38, 0xD3], [0x33, 0xBB, 0xC8], [0xCB, 0xCC, 0xCF],
    [0x81, 0x83, 0x83], [0xFC, 0x39, 0x1F], [0x31, 0xE7, 0x22], [0xEA, 0xEC, 0x23],
    [0x58, 0x33, 0xDB], [0xF9, 0x35, 0xF8], [0x14, 0xF0, 0xF0], [0xFF, 0xFF, 0xFF],
];
const SCHEME_NONE: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00], [0xCD, 0x00, 0x00], [0x00, 0xCD, 0x00], [0xCD, 0xCD, 0x00],
    [0x00, 0x00, 0xCD], [0xCD, 0x00, 0xCD], [0x00, 0xCD, 0xCD], [0xE5, 0xE5, 0xE5],
    [0x7F, 0x7F, 0x7F], [0xFF, 0x00, 0x00], [0x00, 0xFF, 0x00], [0xFF, 0xFF, 0x00],
    [0x00, 0x00, 0xFF], [0xFF, 0x00, 0xFF], [0x00, 0xFF, 0xFF], [0xFF, 0xFF, 0xFF],
];

/// 按 WindTerm 内置方案名返回其调色板；非法名返回 `None`。
///
/// 背景统一为黑（终端配色方案只定义 16 色 ANSI 与前景，背景恒为终端底色），
/// 与 WindTerm 行为一致。各方案取值见 `SCHEME_*` 常量注释。
pub fn builtin_scheme(name: &str) -> Option<Palette> {
    let (raw, fg, bg): ([[u8; 3]; 16], [u8; 3], [u8; 3]) = match name {
        "cmd" => (SCHEME_CMD, [0xC0, 0xC0, 0xC0], [0x00, 0x00, 0x00]),
        "dumb" => (SCHEME_DUMB, [0xCC, 0xCC, 0xCC], [0x00, 0x00, 0x00]),
        "linux" => (SCHEME_LINUX, [0xAA, 0xAA, 0xAA], [0x00, 0x00, 0x00]),
        "macos" => (SCHEME_MACOS, [0xCB, 0xCC, 0xCF], [0x00, 0x00, 0x00]),
        "none" => (SCHEME_NONE, [0xE5, 0xE5, 0xE5], [0x00, 0x00, 0x00]),
        "powershell" => (SCHEME_CMD, [0xC0, 0xC0, 0xC0], [0x00, 0x00, 0x00]),
        "routeros" => (SCHEME_LINUX, [0xAA, 0xAA, 0xAA], [0x00, 0x00, 0x00]),
        _ => return None,
    };
    let ansi = raw.map(|c| Rgb::new(c[0], c[1], c[2]));
    Some(Palette {
        ansi,
        default_fg: Rgb::new(fg[0], fg[1], fg[2]),
        default_bg: Rgb::new(bg[0], bg[1], bg[2]),
        ansi_source: format!("built-in scheme '{name}'"),
        theme_name: name.to_string(),
    })
}

/// Chrome (window furniture) colours, from `global/themes/dige-black/gui.theme`.
pub const CHROME_BG: u32 = 0x2B2B2B;
pub const CHROME_PANEL: u32 = 0x313131;
pub const CHROME_TOOLBAR: u32 = 0x262626;
pub const CHROME_HIGHLIGHT: u32 = 0x3A3D41;
pub const CHROME_SELECTED: u32 = 0x2D5A88;
pub const CHROME_TEXT: u32 = 0xD4D4D4;
pub const CHROME_TEXT_DIM: u32 = 0x808080;
pub const CHROME_BORDER: u32 = 0x1E1E1E;
/// WindTerm 蓝强调色：菜单选中蓝块（`menu_style::MENU_HIGHLIGHT`）、活动标签顶部蓝线
/// （`tab_bar::ACTIVE_LINE`）、菜单栏 `Tunnels` 的胶囊开关开态，以及 2026-11-17 起
/// **查找命中时整条状态栏的染色**（`shell::status_bar_frame`）——四处共用这一个值。
/// 禁止在 `ui/*.rs` 里硬编码此色（ADR-0021 令牌单一真相源）。
pub const ACCENT_BLUE: u32 = 0x007CD2;
pub const LINK_BLUE: u32 = 0x4B9EDE;

/// `Palette::ansi_source` 取这个值时表示「没找到主题文件，用的是内置回退」。
///
/// 单独公开一个常量，是因为判断「颜色来自文件还是回退」的人不止一个
/// （desk 的自检子命令要报告，排障时要确认），复制那句中文字面量迟早会漂。
pub const ANSI_SOURCE_BUILTIN: &str = "built-in defaults (theme file not found)";

/// A theme loaded from a `.theme`-style file layout, or the fallbacks above.
pub struct Palette {
    pub ansi: [Rgb; 16],
    pub default_fg: Rgb,
    pub default_bg: Rgb,
    /// Where the 16 ANSI colours came from: a theme file, or built-in defaults.
    pub ansi_source: String,
    pub theme_name: String,
}

#[derive(Deserialize)]
struct ThemeFile {
    styles: Vec<StyleEntry>,
}

#[derive(Deserialize)]
struct StyleEntry {
    name: String,
    style: StyleBody,
}

#[derive(Deserialize)]
struct StyleBody {
    foreground: Option<String>,
    background: Option<String>,
}

impl Palette {
    /// 纯函数：从主题文件内容解析调色板，解析失败返回 `None`。
    /// 找路径与读文件属于 `term_import::theme`（2026-11-12 收敛后从 term-runtime 搬走）。
    pub fn from_scheme(raw: &str, source: String, theme_name: &str) -> Option<Self> {
        let ansi = parse_scheme(raw)?;
        let (fg, bg) = parse_default_colors(raw);
        Some(Self {
            ansi,
            default_fg: rgb(fg),
            default_bg: rgb(bg),
            ansi_source: source,
            theme_name: theme_name.to_string(),
        })
    }

    /// 纯函数：主题文件不存在时的内置回退调色板。
    pub fn builtin(theme_name: &str) -> Self {
        Self {
            ansi: ANSI_FALLBACK.map(rgb),
            default_fg: rgb(DEFAULT_FG),
            default_bg: rgb(DEFAULT_BG),
            ansi_source: ANSI_SOURCE_BUILTIN.to_string(),
            theme_name: theme_name.to_string(),
        }
    }

    /// A 0..=255 xterm-256 colour palette entry.
    pub fn xterm256(&self, index: u8) -> Rgb {
        match index {
            0..=15 => self.ansi[index as usize],
            16..=231 => {
                let i = index - 16;
                let conv = |v: u8| -> u8 {
                    if v == 0 {
                        0
                    } else {
                        55 + v * 40
                    }
                };
                Rgb::new(
                    conv(i / 36),
                    conv((i / 6) % 6),
                    conv(i % 6),
                )
            }
            _ => {
                let v = 8 + (index - 232) * 10;
                Rgb::new(v, v, v)
            }
        }
    }
}

/// The bundled `.theme` files start with a `//` comment line; strip comments, then parse.
fn strip_json_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let bytes = src.as_bytes();
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_string {
            out.push(c);
            if c == '\\' && i + 1 < bytes.len() {
                i += 1;
                out.push(bytes[i] as char);
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
                i += 1;
            }
            '/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn parse_scheme(src: &str) -> Option<[Rgb; 16]> {
    let file: ThemeFile = serde_json::from_str(&strip_json_comments(src)).ok()?;
    let mut ansi = ANSI_FALLBACK.map(rgb);
    let mut found = 0usize;
    for entry in &file.styles {
        if let Some(pos) = ANSI_NAMES.iter().position(|n| *n == entry.name) {
            if let Some(fg) = &entry.style.foreground {
                if let Some(v) = parse_hex(fg) {
                    ansi[pos] = rgb(v);
                    found += 1;
                }
            }
        }
    }
    if found == ANSI_NAMES.len() {
        Some(ansi)
    } else {
        None
    }
}

fn parse_default_colors(src: &str) -> (u32, u32) {
    if let Ok(file) = serde_json::from_str::<ThemeFile>(&strip_json_comments(src)) {
        for entry in &file.styles {
            if entry.name == "text.default" {
                let fg = entry
                    .style
                    .foreground
                    .as_deref()
                    .and_then(parse_hex)
                    .unwrap_or(DEFAULT_FG);
                let bg = entry
                    .style
                    .background
                    .as_deref()
                    .and_then(parse_hex)
                    .unwrap_or(DEFAULT_BG);
                return (fg, bg);
            }
        }
    }
    (DEFAULT_FG, DEFAULT_BG)
}

/// `#RRGGBB`, or `#AARRGGBB` (the theme file also uses the Qt form with leading alpha).
fn parse_hex(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(s, 16).ok()?;
    match s.len() {
        6 => Some(v),
        8 => Some(v & 0x00FF_FFFF),
        _ => None,
    }
}

pub fn rgb(v: u32) -> Rgb {
    Rgb::from_u32(v)
}

/// 从 `reference-spec` 逐帧像素采样得到的设计 token（单一真相源）。
///
/// 纯数据、零 UI 依赖；投影层 `apps/desktop/src/theme.rs` 再把它转成绘制用的颜色
/// 与尺寸（`to_color32` / `apply_ui_style`）。这里只认 `u32` 色值与点数，
/// 不认任何绘制框架的类型——判据是按整个文件 grep 的，注释里点名也不行。
/// 每个值都可回溯到 `reference-spec/extract_design.py` 的输出或 `SPEC.md`：
/// - 强调蓝 `#047CCC`、弹窗底 `#343434`、面板底 `#1C1C1C`、文字灰阶 `#FCFCFC→#A4A4A4`：
///   跨 657 张图累计主色（见 `extracted-palette.json`）。
/// - 侧栏 200px：参考文档常量。标题条 22px / 状态栏 23px：`measure_layout.py` 实测。
/// - 12 标签色：`picture-100.jpg` 像素聚类实测（见 `design-tokens.json`）。
pub struct DesignTokens {
    // 颜色（#RRGGBB）
    pub panel_bg: u32,
    pub panel_bg2: u32,
    pub popup_bg: u32,
    pub text_bright: u32,
    pub text: u32,
    pub text_dim: u32,
    pub accent: u32,
    pub accent_popup: u32,
    pub terminal_bg: u32,
    pub border: u32,
    pub separator: u32,
    pub title_bar: u32,
    pub tab_colors: [u32; 12],
    // 尺寸（px）
    pub sidebar_width: f32,
    pub title_h: f32,
    pub status_h: f32,
    pub menu_row_h: f32,
    pub menu_pad_x: f32,
    pub radius: f32,
}

/// WindTerm 实测设计 token。改这里 = 改全局视觉（禁止在 `ui/*.rs` 里硬编码颜色）。
pub const DESIGN_TOKENS: DesignTokens = DesignTokens {
    panel_bg: 0x1C1C1C,
    panel_bg2: 0x242424,
    popup_bg: 0x343434,
    text_bright: 0xFCFCFC,
    text: 0xC4C4C4,
    text_dim: 0xA4A4A4,
    accent: 0x047CCC,
    accent_popup: 0x0078C8,
    terminal_bg: 0x000000,
    border: 0x3C3C3C,
    separator: 0x2C2C2C,
    title_bar: 0xF0F0F0,
    tab_colors: [
        0x14648C, 0xDC1478, 0x147814, 0xB48C14, 0xB45050, 0xA050B4, 0x6450C8, 0x646400, 0xDC0000,
        0x3C5064, 0x3C78A0, 0x8CB4C8,
    ],
    sidebar_width: 200.0,
    title_h: 22.0,
    status_h: 27.5,
    menu_row_h: 33.0,
    menu_pad_x: 11.0,
    radius: 0.0,
};

/// 控件表面的**一条**中性灰阶，逐点采自
/// Session Property 弹窗，只读视觉基线，永不重冻结）。
///
/// # 为什么单开一组，而不是继续用 `DESIGN_TOKENS`
///
/// `DESIGN_TOKENS` 是跨 657 张图累计出来的**全局主色**，适于终端与外壳；
/// 但把 `popup_bg = #343434` 当弹窗底、把 `accent_popup = #0078C8` 当选中最、
/// 再把终端黑 `#1C1C1C` 经 `extreme_bg_color` 当输入框底，等于让三套色系在
/// 同一张弹窗里互相打架：输入框比弹窗暗得像挖了几个洞，选中项被画成带亮边的
/// 按钮，按下/悬停又跳成亮蓝。用户原话：「选中的背景，以及输入框的背景，
/// 还有按钮背景，都不协调，看起来非常突兀」。
///
/// 这组只回答一件事：**所有控件表面共用同一条灰阶，饱和蓝色只留给焦点**。
///
/// # 采样证据（原图 1156×972，可复跑 `scripts/compare-golden.py`）
///
/// - `#1C1C1C` 表单/弹窗底 597266 px；`#191919` 左侧树底 199494 px。
/// - 选中行 y=493..538 共 46 行、每行 279 px 的 `#262626`，行内文字仍是 `#BBBBBB`
///   （**选中不提亮文字**，只是换底色，也没有描边）。
/// - 输入框边框 `#444444` 出现在 y=395/434/447/488/501/540/553/592，452 px 宽；
///   框内填充 `#1C1C1C`，与弹窗底同色——只靠边框区分。
/// - 未选中协议标签 `#333333`；选中的 SSH 标签 `#191919`（**比未选中更暗**）。
/// - 正文 `#BBBBBB`；`dodger blue #1E90FF` 全图仅约 100 px（标题栏小方块）。
///
/// `hover` / `pressed` 是灰阶内插值端点，不是独立采样点：它们只在交互瞬间出现，
/// 视觉基线静帧里没有对应像素，因此按相邻灰阶取中间值，并在下面标注。
pub struct ChromeScale {
    /// 弹窗 / 表单底。
    pub dialog: u32,
    /// 侧栏导航树底。
    ///
    /// **等于 `dialog`**：3.x 的新版截图里菜单栏、每一个面板、终端区、状态栏
    /// 全是同一个 `#1C1C1C`，`#191919` 只出现在 2.6.1（旧版把树底压暗一档）。
    /// 保留独立字段是为了「树底要不要压暗」这件事只有一处可改。
    pub nav: u32,
    /// 未选中协议标签块。
    pub tab: u32,
    /// 选中协议标签块：采用压暗而非抬亮。
    pub tab_active: u32,
    /// 输入框填充：**必须等于 `dialog`**，否则输入框看起来像挖洞。
    pub field: u32,
    /// 选中行高亮（导航树 / 菜单选中）。
    pub selected: u32,
    /// 按钮常态底。
    pub button: u32,
    /// 悬停底（灰阶插值）。
    pub hover: u32,
    /// 按下底（灰阶插值端点）。
    pub pressed: u32,
    /// 控件细边框（输入框 / 按钮）。
    pub border: u32,
    /// 分隔线 / 非交互描边。
    pub line: u32,
    /// 正文与选中文字共用同一灰度。
    pub text: u32,
    /// 选中协议标签的加粗文字，仅比正文亮一档。
    pub text_strong: u32,
    /// 面板标题带右侧的小图标（齿轮 / 叉），比正文暗一档。
    pub icon: u32,
    /// 空输入框的占位文字。实测 `#444444`——比正文暗得多，看不清是**故意的**：
    /// 它是提示而不是内容，和分隔线同灰度，不抢眼神。
    pub hint: u32,
    /// 唯一允许的饱和色：焦点环 / 标题栏小方块。
    pub focus: u32,
}

/// WindTerm 对话框表面实测值。改这里 = 改所有控件表面（禁止在 `ui/*.rs` 抄色值）。
pub const CHROME_SCALE: ChromeScale = ChromeScale {
    dialog: 0x1C1C1C,
    nav: 0x1C1C1C,
    tab: 0x333333,
    tab_active: 0x191919,
    field: 0x1C1C1C,
    selected: 0x262626,
    button: 0x262626,
    hover: 0x2A2A2A,  // 灰阶插值：selected(0x26) 与 tab(0x33) 之间
    pressed: 0x333333, // 灰阶端点，等于 tab
    border: 0x444444,
    line: 0x2C2C2C,
    text: 0xBBBBBB,
    text_strong: 0xE8E8E8, // 选中标签的加粗字，视觉基线静帧里接近 #FFFFFF
    icon: 0xA1A1A1,       // 标题带齿轮 / 叉，实测
    hint: 0x444444,       // Filter 占位文字，实测
    focus: 0x1E90FF,
};

/// 快捷命令栏四组的**标记色**，逐点采自
///
/// 与 `CHROME_SCALE` 的分工：灰阶管的是「控件表面」，这里管的是「十五个小图形」。
/// 视觉基线主窗口里饱和色几乎只出现在这 15 个图形上——`session-property.png`
/// 里「dodger blue 全图仅约 100 px」的结论只在那张弹窗图成立。
/// File 组的蓝恰好等于焦点蓝 `#1E90FF`，所以这一组不是另起一套蓝。
///
/// 颜色只在这里定义；`ui/*.rs` 一律经外壳适配层的 `chrome()` 取用。
pub struct QuickBarPalette {
    pub file: u32,
    pub network: u32,
    pub shell: u32,
    pub system: u32,
}

/// WindTerm 快捷栏四组实测色。
pub const QUICK_BAR: QuickBarPalette = QuickBarPalette {
    file: 0x1E90FF,
    network: 0xFF1493,
    shell: 0xFFD700,
    system: 0x228B22,
};

/// 会话树（Session Manager）的**图标点缀色**，采自新版（3.x）会话管理截图，
/// 放大到 2× 后逐像素取值：标题带左侧小方块 `#0093FF`，分组是**青色渐变文件夹**
/// （背板带 `#00AAE1`，主体从 `#61D7FB` 渐变到 `#00B6E7`）。
///
/// 与 `QUICK_BAR` 同一分工：`CHROME_SCALE` 管**控件表面**，这一组只管树上那几个
/// 14px 的小图标。按「协议」这一维给色，而我们的数据只有 `Protocol`，
/// 所以按协议映射——SSH 借 PowerShell 那支蓝，Shell 借 cmd 那支灰。
///
/// 颜色只在这里定义；`ui/*.rs` 一律经外壳适配层的 `chrome()` 取用。
pub struct SessionTreePalette {
    /// 标题带左侧的小方块（`#0093FF`）。**比焦点蓝** `#1E90FF` **更冷**，
    /// 不要为了「少一个色」把它并进 `focus`——放大后两者肉眼可分。
    pub mark: u32,
    /// 文件夹背板（顶部那条深一档的蓝带）。
    pub folder_flap: u32,
    /// 文件夹主体渐变的上端（亮青）。
    pub folder_light: u32,
    /// 文件夹主体渐变的下端。
    pub folder_deep: u32,
    /// 本地 Shell（灰）。
    pub shell: u32,
    /// SSH（蓝）。
    pub ssh: u32,
    /// Telnet（橙）。
    pub telnet: u32,
    /// TCP（绿）。
    pub tcp: u32,
    /// 串口（紫）。
    pub serial: u32,
}

/// 会话树图标色（区分五个协议的配色，避免同色分不清）。
pub const SESSION_TREE: SessionTreePalette = SessionTreePalette {
    mark: 0x0093FF,
    folder_flap: 0x00AAE1,
    folder_light: 0x61D7FB,
    folder_deep: 0x00B6E7,
    shell: 0xC8C8C8,
    ssh: 0x3AA0FF,
    telnet: 0xFF8A50,
    tcp: 0x62C46A,
    serial: 0xA97BE0,
};

/// 终端 margin（左侧边栏）的取色板。
///
/// 取自视觉基线里边栏的取色口径（`docs/design/11-theme-and-visual-baseline.md`），
/// **不是**从 16 色 ANSI 调色板推的：边栏是外壳的一部分，用户换配色方案时
/// 它不该跟着变——否则行号会和输出文字撞色。
pub struct MarginPalette {
    /// 时间戳 `#808080`：比正文暗一档，退到背景里。
    pub time: u32,
    /// 行号 `#0094B2`：青，比时间戳醒目、又不抢正文。
    pub number: u32,
    /// 提示符行符号 `›`（绿 `#94E400`）：与光标色系分开。
    pub prompt: u32,
    /// 自动换行续行符号 `~`（亮青 `#13DCF2`）。
    pub wrap: u32,
    /// 光标所在行的时间戳与行号（品红 `#FF0091`）——一眼看到「我在第几行」。
    pub current: u32,
    /// 光标所在行的整行底色 `#282833`：带一点蓝，和纯灰分隔线区分得开。
    pub current_bg: u32,
}

pub const MARGIN: MarginPalette = MarginPalette {
    time: 0x808080,
    number: 0x0094B2,
    prompt: 0x94E400,
    wrap: 0x13DCF2,
    current: 0xFF0091,
    current_bg: 0x282833,
};

/// 把 cell 的 `Colour` 解析成纯 `Rgb`。`Terminal::resolved` 与 UI 绘制共用，
/// 这样 UI 不必持有 `Terminal` 实例，只依赖一份 `GridSnapshot`。
pub fn resolve(palette: &Palette, colour: crate::grid::Colour, is_fg: bool) -> Rgb {
    use crate::grid::Colour;
    match colour {
        Colour::Default => {
            if is_fg {
                palette.default_fg
            } else {
                palette.default_bg
            }
        }
        Colour::Indexed(i) => palette.xterm256(i),
        Colour::Rgb(r, g, b) => Rgb::new(r, g, b),
    }
}

#[cfg(test)]
mod chrome_scale_tests {
    use super::*;

    fn surfaces() -> Vec<(&'static str, u32)> {
        let s = &CHROME_SCALE;
        vec![
            ("dialog", s.dialog),
            ("nav", s.nav),
            ("tab", s.tab),
            ("tab_active", s.tab_active),
            ("field", s.field),
            ("selected", s.selected),
            ("button", s.button),
            ("hover", s.hover),
            ("pressed", s.pressed),
            ("border", s.border),
            ("line", s.line),
            ("text", s.text),
            ("text_strong", s.text_strong),
        ]
    }

    fn channels(v: u32) -> (u8, u8, u8) {
        (
            ((v >> 16) & 0xFF) as u8,
            ((v >> 8) & 0xFF) as u8,
            (v & 0xFF) as u8,
        )
    }

    #[test]
    fn every_surface_is_a_neutral_gray_in_a_narrow_band() {
        for (name, v) in surfaces() {
            let (r, g, b) = channels(v);
            assert_eq!(r, g, "{name} 不是中性灰（偏红/绿）");
            assert_eq!(g, b, "{name} 不是中性灰（偏蓝）");
            assert!((0x19..=0xE8).contains(&r), "{name}=#{r:02X} 跳出视觉基线灰阶");
        }
    }

    #[test]
    fn no_surface_is_a_saturated_blue_fill() {
        // 旧实现把 accent_popup(#0078C8) 铺在选中态、active 按钮上，
        // 这是「突兀」的直接来源。表面色里一个蓝都不许有。
        for (name, v) in surfaces() {
            let (r, _, b) = channels(v);
            assert!(b as i32 <= r as i32 + 16, "{name} 是蓝色系大面积铺底");
        }
        // 唯一的饱和蓝是焦点色，且不被任何表面引用。
        let (fr, _, fb) = channels(CHROME_SCALE.focus);
        assert!(fb as i32 > fr as i32 + 40);
        assert!(!surfaces().iter().any(|(_, v)| *v == CHROME_SCALE.focus));
    }

    #[test]
    fn input_fields_share_the_dialog_background() {
        // 输入框背景取自绘图框架的 `extreme_bg_color`（外壳适配层里接线）。
        // 它一旦比 window_fill 暗，输入框就会像在弹窗上挖了几个洞。
        assert_eq!(CHROME_SCALE.field, CHROME_SCALE.dialog);
    }

    #[test]
    fn selection_and_buttons_are_subtle_steps_not_hard_jumps() {
        let s = &CHROME_SCALE;
        let d = channels(s.dialog).0 as i32;
        for (name, v) in [
            ("selected", s.selected),
            ("button", s.button),
            ("hover", s.hover),
        ] {
            let delta = (channels(v).0 as i32 - d).abs();
            assert!(delta <= 0x14, "{name} 与弹窗底差 {delta:#X}，超出视觉基线的一档灰");
        }
        // 选中协议标签反而压暗，不能画成「抬亮的浮起块」。
        assert!(channels(s.tab_active).0 < channels(s.tab).0);
    }
}

#[cfg(test)]
mod quick_bar_tests {
    use super::*;

    fn channels(v: u32) -> (u8, u8, u8) {
        (
            ((v >> 16) & 0xFF) as u8,
            ((v >> 8) & 0xFF) as u8,
            (v & 0xFF) as u8,
        )
    }

    #[test]
    fn four_group_marks_are_distinct_and_saturated() {
        let colors = [
            QUICK_BAR.file,
            QUICK_BAR.network,
            QUICK_BAR.shell,
            QUICK_BAR.system,
        ];
        let mut sorted = colors.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 4, "四组颜色必须互不相同");
        for c in colors {
            let (r, g, b) = channels(c);
            let hi = [r, g, b].into_iter().max().unwrap() as i32;
            let lo = [r, g, b].into_iter().min().unwrap() as i32;
            assert!(hi - lo > 80, "组标记色必须是饱和色，而不是灰阶");
        }
    }

    #[test]
    fn file_group_reuses_the_one_focus_blue() {
        // 主窗口里出现的蓝与焦点环是同一个值；再引入第二种蓝会立刻回到
        // 修掉的「三套色系打架」。
        assert_eq!(QUICK_BAR.file, CHROME_SCALE.focus);
    }
}
