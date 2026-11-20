//! Trigger 对话框的几何 / 文案 / 色板规格（R2，零 egui）。
//!
//! # 坐标来源与换算
//!
//!
//! - 第一批主对话框（`01` / `02` / `04` / `05`）是 `1280×937`，**逻辑点 = 像素 × 0.6**
//!   （`1280×0.6 = 768`，`937×0.6 = 562.2`）；
//! - 第二批（`10` / `15` 等）是 `1536×1124`，**逻辑点 = 像素 ÷ 2**
//!   （`1536÷2 = 768`，`1124÷2 = 562`）。
//!
//! 两条口径归一后窗口都是 **768×562 逻辑点**。本模块所有常数都是归一后的
//! egui 逻辑点，可直接被 R3 绘制层消费。
//!
//! 所有矩形为 `(left, top, width, height)`，单位 egui 逻辑点，原点在窗口左上。
//! 页面内容坐标相对 [`BODY`] 左上（即 `(280.8, 63.0)`）。

use serde_json::{json, Value};

/// 主对话框逻辑尺寸：`1536×1124 ÷ 2`。
pub const DIALOG_W: f32 = 768.0;
pub const DIALOG_H: f32 = 562.0;

// ── 窗口骨架（量自 `01-rules-empty-highlight-text.png`） ──
//
// 视觉基线 1280 空间实测：左栏 x 18..447 / y 18..868；标签条 x 468..1261 / y 18..57；
// 右内容框 y 105..868；底部工具条 y 834..868；页脚按钮 y 878..918。均 ×0.6。

/// 左栏（触发器列表）外框。
pub const LEFT: (f32, f32, f32, f32) = (10.8, 10.8, 257.4, 510.0);
/// 顶部标签条（类型标题 + Sessions）。
pub const TABBAR: (f32, f32, f32, f32) = (280.8, 10.8, 475.8, 23.4);
/// 标签条内两页的分界（相对标签条左边）。
pub const TAB_SPLIT: f32 = 238.8;
/// 内容区上方的标签行（`Rules:` / `Action:` / `Match:`）。
pub const LABEL_Y: f32 = 44.4;
/// 启用开关。
pub const SWITCH: (f32, f32, f32, f32) = (725.4, 41.4, 30.6, 15.0);
/// 右侧内容区（`Rules` 页是表格外框；其它页是内容起点）。
///
/// 高度 457.8 = 到**外框底**（520.8）；表格本身只到工具条上沿，见
/// [`BODY_CONTENT_H`]。
pub const BODY: (f32, f32, f32, f32) = (280.8, 63.0, 475.8, 457.8);
/// 内容可用高度：到工具条上沿（`500.4 - 63.0`）。
pub const BODY_CONTENT_H: f32 = 437.4;
/// 底部两条工具条。
pub const STRIP_Y: f32 = 500.4;
pub const STRIP_H: f32 = 20.4;
/// 页脚按钮：`Cancel` 与 `Ok`。
///
/// 视觉基线 `21` / `26` / `28` 三张图实测同一组数值：按钮外框 `528.5..550`（高
/// 21.5，含 1px 描边）、`Cancel` 左边 `310.2`（宽 77）、主按钮 `395.3..475.8`
/// （宽 80.5，右缘与 `BODY` 右缘齐平）。早期写的 `526.8 / 高 24` 比视觉基线高
/// 2.5 点，且 `Cancel` 左移 1 点。
pub const BTN_H: f32 = 21.5;
pub const BTN_Y: f32 = 528.5;
pub const BTN_CANCEL: (f32, f32) = (591.0, 77.0);
pub const BTN_OK: (f32, f32) = (676.05, 80.55);
/// 列表为空时那句灰字的位置（窗口坐标，垂直居中于左栏）。
pub const LEFT_EMPTY_Y: f32 = 252.0;

/// `Rules` 视图（`BODY` 相对坐标）。
pub mod rules {
    /// 表头高（视觉基线 1280 空间 105..139，×0.6）。
    pub const HEAD_H: f32 = 20.4;
    /// 三列竖线的 x（`BODY` 相对）：Pattern | Description | Style。
    pub const COL_X: [f32; 3] = [157.2, 315.0, 475.8];
    /// 表头文字相对表头左上角的偏移。
    pub const HEAD_PAD: (f32, f32) = (4.2, 4.8);
    /// 表头列名。
    pub const HEADERS: [&str; 3] = ["Pattern", "Description", "Style"];
    /// 空态文案（居中于表体）。
    pub const EMPTY: &str = "No text mark available";
}

/// `Sessions` 视图（`BODY` 相对坐标）。
pub mod sessions {
    pub const LABEL_A: &str = "Target Systems:";
    pub const LABEL_B: &str = "Target Sessions:";
    // 全部量自 `02-sessions-tab.png`（1280×937 = 768×562 逻辑点，1.667 px/pt；
    // `BODY` 原点在像素 (468,105)）。视觉基线里两个列表**紧挨着**，中间只隔一行
    // 标题——不是把可用高度平分成两块。
    /// 第一个列表：`BODY` 相对 top / height（像素 105..308 → 0..121.8）。
    pub const LIST_A: (f32, f32) = (0.0, 142.8);
    /// 第一个列表下的工具条 top。
    pub const TOOLS_A: f32 = 142.8;
    /// 第二个列表标题 top / 列表 top（像素 359 标题 → 152.4；框顶 388 → 169.8）。
    pub const LABEL_B_TOP: f32 = 152.4;
    pub const LIST_B_TOP: f32 = 171.0;
    /// 第二个列表底（9 行 × 22 点 + 上下留白）。
    pub const LIST_B_BOTTOM: f32 = 390.0;
    pub const TOOLS_H: f32 = 21.0;
    /// 行高：Systems 像素行距 34（20.4 点），Sessions 像素行距 37（22.2 点）。
    pub const ROW_A_H: f32 = 20.0;
    pub const ROW_H: f32 = 22.0;
    /// 系统行（视觉基线 `02-sessions-tab.png`，从上到下）。
    pub const SYSTEMS: [&str; 6] = ["all systems", "cmd", "dumb", "linux", "macos", "none"];
    /// 会话行（图标类别由绘制层按后缀决定）。
    pub const SESSIONS: [&str; 9] = [
        "Shell sessions / bash",
        "Shell sessions / csh",
        "Shell sessions / dash",
        "Shell sessions / ksh",
        "Shell sessions / sh",
        "Shell sessions / tcsh",
        "Shell sessions / zsh",
        "bash",
        "cu.debug-console",
    ];
    /// 工具条按钮。
    pub const CHECK_ALL: &str = "Check All";
    pub const UNCHECK_ALL: &str = "Uncheck All";
}

/// 事件类（Action 下拉 + OS 选项卡 + 三字段）视图，`BODY` 相对坐标。
///
/// 同时服务 `After connection` / `Before connection` / `After disconnection` /
/// `Before disconnection` / `Run command` 五种类型——它们的编辑面板同形。
pub mod action {
    /// Action 下拉框。
    pub const COMBO: (f32, f32, f32) = (0.0, 0.0, 22.8);
    /// OS 选项卡条 top / 高（视觉基线 `04` 实测 133..161 @1084 → 逻辑 20）。
    pub const OS_TABS_TOP: f32 = 31.5;
    pub const OS_TABS_H: f32 = 19.8;
    /// 三行输入框：(top, height)，已含行距。
    pub const ROWS: [(f32, f32); 3] = [(66.0, 21.0), (96.0, 21.0), (126.0, 21.0)];
    /// 字段左边与宽度。
    pub const FIELD_LEFT: f32 = 146.4;
    pub const FIELD_W: f32 = 318.6;
    /// 三行标签文字。
    pub const LABELS: [&str; 3] = ["Command:", "Arguments:", "Working Directory:"];
    /// OS 选项卡文字。
    pub const OS: [&str; 3] = ["macOS", "Linux", "Windows"];
    /// `Working Directory` 的占位提示。
    pub const WORKDIR_HINT: &str = "${HomeDir}";
    /// Action 下拉的四个选项。
    pub const ACTIONS: [&str; 4] = ["Run command", "Play sound", "Open hyperlink", "Send text"];
}

/// `Custom link` 视图（量自 `10-custom-link-page.png`，÷2），`BODY` 相对坐标。
pub mod custom {
    /// Match 多行框 top / 高。
    pub const MATCH_TOP: f32 = 0.0;
    pub const MATCH_H: f32 = 150.0;
    /// 框底 flag bar 高。
    pub const FLAG_H: f32 = 20.5;
    /// Action 下拉 top / 高。
    pub const COMBO_TOP: f32 = 199.0;
    pub const COMBO_H: f32 = 22.5;
    /// OS 选项卡条 top / 高。
    pub const OS_TABS_TOP: f32 = 229.0;
    pub const OS_TABS_H: f32 = 24.5;
    /// 三行输入框 top（高 23.5）。
    pub const ROWS: [f32; 3] = [263.0, 293.0, 323.0];
    pub const FIELD_H: f32 = 23.5;
    /// 字段左边与宽度。
    pub const FIELD_LEFT: f32 = 146.0;
    pub const FIELD_W: f32 = 319.5;
    /// `Mark Style` / `Mark Color` 两行（下拉 top / 高）。
    pub const MARK_STYLE_TOP: f32 = 405.0;
    pub const MARK_COLOR_TOP: f32 = 435.0;
    pub const MARK_COMBO_LEFT: f32 = 86.2;
    /// Mark 下拉的宽度到内容右缘。
    pub const MARK_COMBO_H: f32 = 22.5;
}

/// `Send text` 视图（量自 `15-send-text-page.png`，÷2），`BODY` 相对坐标。
pub mod send {
    pub const MATCH_TOP: f32 = 0.0;
    pub const MATCH_H: f32 = 159.0;
    pub const FLAG_H: f32 = 20.5;
    /// `Send:` 多行框。
    pub const SEND_TOP: f32 = 198.0;
    pub const SEND_H: f32 = 164.5;
    /// `Text` / `Script` 选项卡条。
    pub const MODE_TABS_TOP: f32 = 362.5;
    pub const MODE_TABS_H: f32 = 20.0;
    /// `Send Condition` 行与 `Send Later` 行。
    pub const COND_TOP: f32 = 386.0;
    pub const LATER_TOP: f32 = 411.0;
    pub const ROW_H: f32 = 15.5;
    /// 字段左边与宽度。
    pub const FIELD_LEFT: f32 = 111.0;
    pub const FIELD_W: f32 = 358.2;
    /// 标签文字（带助记符下划线，由绘制层处理）。
    pub const LABELS: [&str; 4] = ["Match:", "Send:", "Send Condition:", "Send Later:"];
    /// Send 框占位提示（两行）。
    pub const SEND_HINT: [&str; 2] = ["E.g.:", "echo ${match}\\n"];
    /// 模式选项卡文字。
    pub const MODES: [&str; 2] = ["Text", "Script"];
    /// 右侧 `Captures` 下拉宽度。
    pub const CAPTURES_W: f32 = 74.0;
}

/// 新建触发器弹窗（`09-new-trigger-dialog.png` 2px/点 + `03` 1.668px/点 互校）。
///
/// 行距实测 60 px@2 = 30 逻辑点，字段高 48 px@2 = 24 逻辑点；`Type` 展开时
/// 列表**盖住**输入框，首行与框同高（不是落在框下方）。
pub mod new_trigger {
    pub const W: f32 = 445.0;
    pub const H: f32 = 156.0;
    /// `Type` 下拉的每项高度（44 px@2）。
    pub const ITEM_H: f32 = 22.0;
    /// 三个字段的左边与宽度（相对弹窗）。
    pub const FIELD_X: f32 = 104.0;
    pub const FIELD_W: f32 = 333.0;
    pub const FIELD_H: f32 = 24.0;
    /// 三行的 top：Name / Description / Type。
    pub const ROW_Y: [f32; 3] = [30.0, 60.0, 90.0];
}

/// Highlight 设置弹窗（量自 `11-highlight-settings-dialog.png`，794×1126）。
///
/// **口径是 2 px/逻辑点**（`794÷2 = 397`，`1126÷2 = 563`），与 `09` 同尺度：
/// 两图里 `Description:` 的墨迹宽度都是 167 px、字距都是 14.7 px。曾按
/// `1.668 px/点` 推出 476×675，那是错的——验证方法是拿 `09` 的已知宽度
/// （输入框 666 px = 333 逻辑点）做同尺度互校。
///
/// 版式要点（易错）：
/// - 弹窗内部是**两个独立的分组框**（`#2e2e2e`，圆角 3 点），不是一个大框；
/// - `Match` / `Style` 是分组**标题**，不带下划线；`Pattern:` 等是字段**标签**，
///   首字母带下划线；
/// - `.* Aa W` 的 flag bar 贴在 Pattern 框**内部**下沿（高 20 点，上方一条分隔线）；
/// - 弹窗高 563 比父窗口 562 还高 1 点，视觉上就是「顶满窗口高度」。
pub mod highlight {
    pub const W: f32 = 397.0;
    pub const H: f32 = 563.0;

    /// 两个分组框共用的左边与宽度（像素 24..771）。
    pub const GROUP_X: f32 = 12.0;
    pub const GROUP_W: f32 = 374.0;
    /// `Match` 组（像素 y 62..595）/ `Style` 组（y 648..1043）：top / 高。
    pub const G1: (f32, f32) = (31.0, 267.0);
    pub const G2: (f32, f32) = (324.0, 198.0);
    /// 组标题（`LEFT_TOP` 锚点，像素 x32 / x31，y 取墨迹顶 33 / 619）。
    /// 已扣掉 egui galley 顶部约 3 点的行距留白。
    pub const MATCH_TITLE: (f32, f32) = (15.0, 13.5);
    pub const STYLE_TITLE: (f32, f32) = (13.5, 306.5);
    /// 左标签列的 x（像素 46）。
    pub const LABEL_X: f32 = 22.0;

    /// `Pattern:` 标签（`LEFT_CENTER`，墨迹 103..126 的中点 57.25，已扣掉
    /// galley 居中偏置 1.5 点）与多行框（像素 232..747 × 84..510）。
    pub const PATTERN_LABEL: (f32, f32) = (22.0, 55.8);
    pub const PATTERN: (f32, f32, f32, f32) = (116.0, 42.0, 258.0, 213.0);
    /// flag bar 高（像素 470..510）。
    pub const FLAG_H: f32 = 20.0;
    /// `Description:` 标签（`LEFT_CENTER`，墨迹 539..562 的中点 275.25，同上扣
    /// 1.5 点）与单行框（像素 524..571）。
    pub const DESC_LABEL: (f32, f32) = (22.0, 273.8);
    pub const DESC: (f32, f32, f32, f32) = (116.0, 262.0, 258.0, 23.5);

    /// Style 组四行的 top（像素 672 / 730 / 790 / 850）。
    pub const ROWS: [f32; 4] = [336.0, 365.0, 395.0, 425.0];
    pub const ROW_FIELD_X: f32 = 152.0;
    pub const ROW_FIELD_W: f32 = 222.0;
    pub const ROW_H: f32 = 22.0;
    /// `Preview` 展示块（像素 46..747 × 908..1019）。
    pub const PREVIEW: (f32, f32, f32, f32) = (23.0, 454.0, 350.5, 55.5);

    /// 页脚两个按钮（像素 444..595 / 616..767 × 1058..1101）。
    pub const BTN_W: f32 = 76.0;
    pub const BTN_H: f32 = 22.0;
    pub const BTN_Y: f32 = 529.0;
    pub const BTN_CANCEL_X: f32 = 222.0;
    pub const BTN_OK_X: f32 = 308.0;
    /// Style 组四行的标签。
    pub const LABELS: [&str; 4] = ["Text Color:", "Background Color:", "Line Style:", "Line Color:"];
}

/// Edit Color 取色器（`06-color-picker.png`，1052×936 → ×0.6）。
/// `Edit Color` 取色器（视觉基线 `06-color-picker.png`）。
///
/// **口径校正（本轮实测）**：`06` 是 `1052×936`，此前按 `÷1.667` 换算成 631×562，
/// 那是错的。判据是文字格宽：`Basic colors`（12 字）墨迹宽 169 px，与 `11` 里
/// `Description:`（12 字、2× 空间）的 167–168 px 一致；下拉菜单 `14` 的文本行距
/// 56.5 px 也正好是 28 点 × 2。所以 `06` 与 `14` 一样是 **2×**，窗口 =
/// `1052÷2 × 936÷2 = 526×468`。下面所有常数都是 `像素 ÷ 2` 后的逻辑点。
///
/// 版式（自上而下）：标题栏 28 → `Basic colors` 8×6 色板 → `Pick Screen Color`
/// → 大留白 → `Custom colors` 8×2 色板 → `Add to Custom Colors`；右侧从色域方块
/// 起，方块下方是预览块 + HSV/RGB 两列数值，再 `Alpha channel:` 与 `HTML:`，
/// 页脚 `Cancel` / `OK` 贴底。
pub mod color_picker {
    pub const W: f32 = 526.0;
    pub const H: f32 = 468.0;
    /// 标题栏高度（像素 0..55，底色 `#393c3e`）。
    pub const TITLE_H: f32 = 28.0;
    /// 三个窗口圆点：圆心 y=14、x=15.75/35.75/55.75、半径 6（像素直径 24）。
    pub const DOT_X: f32 = 15.75;
    pub const DOT_STEP: f32 = 20.0;
    pub const DOT_Y: f32 = 14.0;
    pub const DOT_R: f32 = 6.0;
    /// 色域方块（像素 494..935 × 96..497）。
    pub const SQUARE: (f32, f32, f32, f32) = (247.0, 48.0, 221.0, 201.0);
    /// 灰度亮度条（像素 970..999 × 96..499）。
    pub const VALUE_BAR: (f32, f32, f32, f32) = (485.0, 48.0, 15.0, 201.0);
    /// 亮度条右肩的取值三角（像素 x 1001..1009、随取值上下走）。
    pub const BAR_MARK_W: f32 = 4.5;
    pub const BAR_MARK_H: f32 = 9.0;
    /// 预览块（像素 494..611 × 542..839），左缘与方块对齐。
    pub const PREVIEW: (f32, f32, f32, f32) = (247.0, 271.0, 59.0, 149.0);
    /// 色板：块 40×32 px → 20×16 点，缝 16 px → 8 点，**8 列 × 6 行 = 48 色**。
    pub const SWATCH_W: f32 = 20.0;
    pub const SWATCH_H: f32 = 16.0;
    pub const SWATCH_GAP_X: f32 = 8.0;
    pub const SWATCH_GAP_Y: f32 = 8.0;
    pub const PALETTE_COLS: usize = 8;
    pub const PALETTE_ROWS: usize = 6;
    /// 选中环：色块外扩 4 点的一圈 1 点描边（像素 414..469 对 422..461）。
    pub const RING_PAD: f32 = 4.0;
    pub const PALETTE_LEFT: f32 = 15.0;
    /// 第一行色块顶（像素 130）。
    pub const PALETTE_TOP: f32 = 65.0;
    /// 自定义色板第一行顶（像素 694）。
    pub const CUSTOM_TOP: f32 = 347.0;
    /// 两处色板标题的墨迹左缘与基线（像素 `Basic colors` 24/102）。
    pub const LABEL_X: f32 = 12.0;
    pub const LABEL_BASIC_Y: f32 = 51.0;
    pub const LABEL_CUSTOM_Y: f32 = 333.0;
    /// 两个扁平按钮：像素 x 28..463，`Pick Screen Color` y 424..467、
    /// `Add to Custom Colors` y 796..839。
    pub const BTN: (f32, f32, f32, f32) = (13.0, 211.0, 218.0, 22.0);
    pub const BTN_ADD_Y: f32 = 397.0;
    /// 两列数值面板：标签右缘、输入框左缘/宽/高、首行顶、行距。
    /// 标签右缘（像素 `Hue:` 墨迹 642..692、`Green:` 826..902）与框左缘。
    pub const HSV_LABEL_RIGHT: f32 = 346.0;
    pub const HSV_BOX_X: f32 = 355.0;
    pub const RGB_LABEL_RIGHT: f32 = 451.0;
    pub const RGB_BOX_X: f32 = 460.0;
    pub const BOX_W: f32 = 48.5;
    pub const BOX_H: f32 = 22.5;
    pub const READOUT_TOP: f32 = 271.0;
    pub const READOUT_ROW_H: f32 = 30.0;
    /// `HTML:` 输入框（像素 710..967 × 794..841）。
    pub const HTML_BOX: (f32, f32, f32, f32) = (355.0, 397.0, 128.5, 24.0);
    /// 页脚按钮（像素 Cancel 704..851、OK 874..1025，y 868..911）。
    pub const CANCEL: (f32, f32, f32, f32) = (352.0, 434.0, 74.0, 22.0);
    pub const OK: (f32, f32, f32, f32) = (437.0, 434.0, 76.0, 22.0);
}

/// 色板 token（u32 RGB，与渲染后端无关）。
pub mod color {
    pub const BG: u32 = 0x1c1c1c;
    pub const BG_2: u32 = 0x202020;
    pub const LINE: u32 = 0x333333;
    pub const LINE_SOFT: u32 = 0x282828;
    pub const SEL: u32 = 0x282828;
    pub const TAB: u32 = 0x333333;
    pub const HEAD: u32 = 0x242424;
    pub const FIELD_LINE: u32 = 0x404040;
    pub const TEXT: u32 = 0xffffff;
    /// 正文 / 标签 / 下拉值。视觉基线 `09` / `10` / `15` 实测全是 `#bbbbbb`。
    pub const TEXT_2: u32 = 0xbbbbbb;
    pub const TEXT_3: u32 = 0x7a7a7a;
    pub const TEXT_4: u32 = 0x5a5a5a;
    pub const BLUE: u32 = 0x3facff;
    /// 列表图标 / 开关轨道用的蓝（`13` 实测 `#007cd2`）。
    pub const ICON: u32 = 0x007cd2;
    pub const BLUE_TRACK: u32 = 0x007cd2;
    pub const BLUE_DEEP: u32 = 0x003e69;
    pub const BLUE_FOCUS: u32 = 0x0074c3;
    pub const BTN_BG: u32 = 0x252b31;
    /// Color 下拉的 16 ANSI 色。
    pub const ANSI: [u32; 16] = [
        0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000cd, 0xcd00cd, 0x00cdcd, 0xe5e5e5,
        0x4d4d4d, 0xff0000, 0x00ff00, 0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
    ];
    pub const ANSI_LABELS: [&str; 16] = [
        "Ansi Black",
        "Ansi Red",
        "Ansi Green",
        "Ansi Yellow",
        "Ansi Blue",
        "Ansi Magenta",
        "Ansi Cyan",
        "Ansi White",
        "Ansi Bright Black",
        "Ansi Bright Red",
        "Ansi Bright Green",
        "Ansi Bright Yellow",
        "Ansi Bright Blue",
        "Ansi Bright Magenta",
        "Ansi Bright Cyan",
        "Ansi Bright White",
    ];
    /// Basic colors 色板 8×6（视觉基线 `06-color-picker.png`，逐格取中心像素采样）。
    pub const BASIC: [u32; 48] = [
        0x000000, 0xba0000, 0x005700, 0xb74f00, 0x00ad00, 0xaaaa00, 0x00ff00, 0x8dff00,
        0x000085, 0xba0082, 0x005783, 0xb74f81, 0x00ad7c, 0xaaaa7a, 0x00ff6d, 0x8dff6b,
        0x0000ff, 0xba00ff, 0x0057ff, 0xb74fff, 0x00adff, 0xaaaaff, 0x00ffff, 0x8dffff,
        0x5e0000, 0xff0000, 0x555500, 0xff4100, 0x27ad00, 0xffa500, 0x00ff00, 0xffff00,
        0x5e0084, 0xff007f, 0x555582, 0xff417d, 0x27ad7b, 0xffa576, 0x00ff6d, 0xffff67,
        0x5e00ff, 0xff00ff, 0x5555ff, 0xff41ff, 0x27adff, 0xffa5ff, 0x00ffff, 0xffffff,
    ];
}

/// 类型标题（`01` / `07` / `23` / `26` / `28` / `30` 实测）。带助记符下划线的
/// 位置由 UI 层处理，这里只存纯文本。
///
/// 注意它与 [`term_core::trigger::TriggerKind::label`] **不是同一批字符串**：
/// 视觉基线 `21` 的 `Action` 下拉取值写 `Play sound`（句首大写），而同一张图的
/// 页签标题写 `Play Sound`（词首大写）。四个复合词（`Play Sound` /
/// `Run Command` / `Show Tooltip` / `Send Text`）都是词首大写，四个事件类型
/// 反过来是句首大写（`After connection`）。
pub fn type_title(kind: term_core::trigger::TriggerKind) -> &'static str {
    use term_core::trigger::TriggerKind;
    match kind {
        TriggerKind::CustomLink => "Custom Link",
        TriggerKind::CustomMenu => "Custom Menu",
        TriggerKind::AfterConnect => "After connection",
        TriggerKind::AfterDisconnect => "After disconnection",
        TriggerKind::BeforeConnect => "Before connection",
        TriggerKind::BeforeDisconnect => "Before disconnection",
        TriggerKind::HighlightText => "Highlight Text",
        TriggerKind::PlaySound => "Play Sound",
        TriggerKind::RunCommand => "Run Command",
        TriggerKind::SendText => "Send Text",
        TriggerKind::ShowTooltip => "Show Tooltip",
    }
}

/// 类型标题的助记符字符（视觉基线里被下划线标出的那一个字母）。
///
/// `Run Command` 划线的是 `Command` 的 `C` 而不是 `Run` 的 `R`（视觉基线 `26`）。
pub fn type_mnemonic(kind: term_core::trigger::TriggerKind) -> char {
    use term_core::trigger::TriggerKind;
    match kind {
        TriggerKind::CustomLink | TriggerKind::CustomMenu => 'C',
        TriggerKind::AfterConnect | TriggerKind::AfterDisconnect => 'A',
        TriggerKind::BeforeConnect | TriggerKind::BeforeDisconnect => 'B',
        TriggerKind::HighlightText => 'H',
        TriggerKind::PlaySound => 'P',
        TriggerKind::RunCommand => 'C',
        TriggerKind::SendText | TriggerKind::ShowTooltip => 'S',
    }
}

/// R2 视图快照：换成 JSON 仍可消费（ADR-0021 §2 的序列化判据）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriggerViewSnapshot {
    pub selected: Option<usize>,
    pub tab_sessions: bool,
    pub active_dialog: Option<String>,
    pub focused_rule_id: String,
}

impl TriggerViewSnapshot {
    /// 不依赖 serde derive：手工投影成 `serde_json::Value`，换 web 前端可直接消费。
    pub fn to_json(&self) -> Value {
        json!({
            "selected": self.selected,
            "tab_sessions": self.tab_sessions,
            "active_dialog": self.active_dialog,
            "focused_rule_id": self.focused_rule_id,
        })
    }
}

#[cfg(test)]
#[allow(clippy::assertions_on_constants)] // 本模块的断言对象**就是**规格常量
mod tests {
    use super::*;

    #[test]
    fn dialog_size_matches_both_reference_scales() {
        // 1280×937 ×0.6 与 1536×1124 ÷2 必须归一到同一尺寸。
        assert!((1280.0 * 0.6 - DIALOG_W).abs() < 0.001);
        assert!((937.0 * 0.6 - DIALOG_H).abs() < 0.5);
        assert!((1536.0 / 2.0 - DIALOG_W).abs() < 0.001);
        assert!((1124.0 / 2.0 - DIALOG_H).abs() < 0.001);
    }

    #[test]
    fn panes_fit_inside_dialog() {
        let (lx, ly, lw, lh) = LEFT;
        let (bx, by, bw, bh) = BODY;
        assert!(lx + lw <= DIALOG_W && ly + lh <= DIALOG_H);
        assert!(bx + bw <= DIALOG_W && by + bh <= DIALOG_H);
        assert!(bx > lx + lw, "左右栏不能重叠");
        // 左栏外框把底部工具条包在里面：框底 = 工具条顶 + 工具条高。
        assert!((ly + lh - (STRIP_Y + STRIP_H)).abs() < 0.001);
    }

    #[test]
    fn footer_buttons_sit_below_strip() {
        assert!(BTN_Y > STRIP_Y + STRIP_H);
        assert!(BTN_Y + BTN_H <= DIALOG_H);
        let (cx, cw) = BTN_CANCEL;
        let (ox, ow) = BTN_OK;
        assert!(ox > cx + cw, "Ok 在 Cancel 右边");
        assert!((ox + ow - (DIALOG_W - 11.4)).abs() < 0.01);
    }

    #[test]
    fn body_fits_between_label_and_strip() {
        // 内容区顶在标签行下方、底在工具条上方。
        assert!(BODY.1 > LABEL_Y);
        assert!((BODY.1 + BODY.3 - (STRIP_Y + STRIP_H)).abs() < 0.001);
        assert!((BODY.1 + BODY_CONTENT_H - STRIP_Y).abs() < 0.001);
    }

    #[test]
    fn color_palettes_have_expected_sizes() {
        assert_eq!(color::BASIC.len(), 48);
        assert_eq!(color::BASIC.len(), color_picker::PALETTE_COLS * color_picker::PALETTE_ROWS);
        assert_eq!(color::ANSI.len(), 16);
        assert_eq!(color::ANSI_LABELS.len(), 16);
    }

    #[test]
    fn every_kind_has_title_and_mnemonic() {
        use term_core::trigger::TriggerKind;
        for k in TriggerKind::ALL {
            let t = type_title(k);
            assert!(!t.is_empty());
            let m = type_mnemonic(k);
            assert!(t.chars().any(|c| c == m), "{t:?} 里没有助记符 {m:?}");
        }
        assert_eq!(TriggerKind::ALL.len(), 11);
    }

    #[test]
    fn snapshot_is_serializable() {
        let s = TriggerViewSnapshot {
            selected: Some(2),
            tab_sessions: false,
            active_dialog: Some("highlight".into()),
            focused_rule_id: "r1".into(),
        };
        let v = s.to_json();
        assert_eq!(v["selected"], json!(2));
        assert_eq!(v["active_dialog"], json!("highlight"));
        assert_eq!(v["focused_rule_id"], json!("r1"));
    }
}
