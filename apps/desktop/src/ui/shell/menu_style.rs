//! 弹层里**一行**怎么画：取色、三态、勾选框、文本块排版。
//!
//! R9：2026-11-08 从 `menu_bar.rs` 拆出（勾选栏把那个文件顶到 613 行），
//! 同一天再按「一行 / 一棵树」分成两个文件：本文件只画**单行**，
//! `menu_items.rs` 负责遍历菜单树。这样「加一个勾选框」不会牵动遍历逻辑。

use crate::ui::*;

/// 菜单三态的实测取色（2026-11-08 两张视觉基线逐像素统计，见 visual-golden §7.22）：
///
/// | 状态 | 整行底 | 文字 |
/// |---|---|---|
/// | 启用 | `#333333` | `#BBBBBB` |
/// | 禁用 | `#333333` | `#555555` |
/// | 选中 | `#007CD2` | `#BBBBBB`（**不是白字**） |
///
/// 分隔线 `#555555`，与禁用文字同色。先前把文字交给 `Color32::PLACEHOLDER`
/// 让 egui 自动置灰，色值完全不可控；现在三态全部显式，夹具能摆出任意一态。
// 2026-11-17：三态色值搬到 `kit::spec` 作为唯一出处，这里保留别名——
// 下面几十处画菜单行的代码一个字都不用改，夹具仍钉同一组数值。
const MENU_BG: u32 = crate::ui::kit::spec::MENU_BG;
const MENU_TEXT: u32 = crate::ui::kit::spec::MENU_TEXT;
const MENU_TEXT_DISABLED: u32 = crate::ui::kit::spec::MENU_TEXT_DISABLED;
const MENU_SEPARATOR: u32 = crate::ui::kit::spec::MENU_SEPARATOR;
pub(super) const MENU_HIGHLIGHT: u32 = crate::ui::kit::spec::MENU_HIGHLIGHT;
/// 快捷键列起点（等宽字符格）。视觉基线里快捷键固定从约 23 格处起
/// （菜单宽 ~285，最长的 `Save Selection As...` 20 格）。
pub(in crate::ui) const SHORTCUT_COL: usize = 23;
/// 文本顶过快捷键列时，文字与快捷键之间的**最小间隙**。
///
/// 之前这一档是 0：`Split View Horizontally` 这类 22 格的标签一超列，快捷键就
/// 直接贴上去（`Alt+W, Alt+H` 与文字之间一个像素都没有，实测图里粘成一片）。
/// 固定列那一档本来就有天然空隙，只有「文本更长」这一支要有下限。
pub(in crate::ui) const SHORTCUT_GAP: f32 = 7.0;
/// 快捷键文字的灰色（子菜单展开箭头同色）。
pub(in crate::ui) const SHORTCUT_GRAY: u32 = 0xA8A8A8;

// ── 勾选栏（2026-11-08 新加，`Item::Check` 的绘制） ──
//
// 尺寸与取色来自 `01-menu-bar/mode/01-mode-menu.png`（放大 3× 逐像素量，
// 结论写进 `docs/visual-golden.md` §7.23）：弹层左内边到框左缘 8、
// 方框 14×14、框右缘到文字 8 → 整栏 30。
//
// 2026-11-11 收紧：30 的占位太宽——勾选框与文字之间约 12px 空隙，且把每行的
// 文本整体右推 30px，无勾选的行也跟着留一道宽空白，看起来不协调。改为
// 左内边 6、方框 14×14、框右缘到文字 6 → 整栏 22，空隙减半、文本更靠左，
// 仍不压框（框右缘 20 < 栏宽 22）、不与文本重叠。
//
// 2026-11-14 再收：参考实现（同一张 `01-mode-menu.png`）实量复选框高 / 行高 ≈ 0.47，
// 而 14×14 在 ~23pt 行高下行高占比 0.61，明显比参考实现大。按用户核对，复选框
// 缩到 11×11（比文字字高略小，参考实现即如此），左内边 5、框右缘到文字 2 → 整栏 18，
// 文本更靠左、不与框重叠（框右缘 16 < 栏宽 18）。行高由 `MENU_ROW_PAD_Y` 管。
pub(in crate::ui) const CHECK_GUTTER: f32 = 18.0;
/// 全仓**手绘复选框的唯一边长**（2026-11-15 用户点名「所有复选框大小对齐」）。
///
/// 之前有两套：菜单行 11×11，快捷命令栏左侧 `Groups` 弹层 14×14，同一屏里
/// 一大一小。现在凡是自己画方框的弹层都从这一个常量取边长，行高不同只准改
/// 左内边距（[`check_rect_with_inset`]），不准另写一份尺寸。
///
/// 2026-11-17：数字搬到 [`crate::ui::kit::checkbox`]，这里只剩别名——方框外观
/// 整个收进了那个组件，本文件不再持有勾子、线宽、颜色。
pub(in crate::ui) const CHECK_SIZE: f32 = crate::ui::kit::checkbox::MENU_SIZE;
const CHECK_INSET: f32 = 5.0;

/// 一行的左边那个方框：整行左侧 `inset` 起、`CHECK_SIZE` 见方、垂直居中。
///
/// 菜单行用 [`check_rect`]（内边距 5）；快捷命令栏 `Groups` 行高 28、视觉基线
/// 实测左内边距 8，用同一个函数传 8——边长仍是同一个常量。
pub(in crate::ui) fn check_rect_with_inset(row: egui::Rect, inset: f32) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(row.left() + inset, row.center().y - CHECK_SIZE / 2.0),
        egui::vec2(CHECK_SIZE, CHECK_SIZE),
    )
}

/// 菜单行的方框（左内边距 [`CHECK_INSET`]）。
pub(in crate::ui) fn check_rect(row: egui::Rect) -> egui::Rect {
    check_rect_with_inset(row, CHECK_INSET)
}
/// 禁用行的框线跟着文字一起暗下去——灰字配亮框比不灰更显眼。
///
/// 框内挖暗色（`#1C1C1C`）、框线（`#575757`）、勾色（`#7A7A7A`）三个实测值
/// 已搬进 [`crate::ui::kit::checkbox::CheckboxProps::menu`]，本文件只留这一档
/// 「禁用」差异。
const CHECK_BORDER_OFF: u32 = 0x3F3F3F;

/// 一行左边的勾选框。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::ui) enum CheckCell {
    /// 这一行没有框（绝大多数行）。
    None,
    /// 有框，`true` = 框里画 ✓。
    Box(bool),
}

/// 参考实现那种**胶囊开关**（`Tunnels` 下拉第一行右端那颗）。
///
/// 2026-11-17 量自 `Tunnels` 下拉视觉基线：胶囊 **31.5×15.5**（物理 63×31），
/// 开态底 `#007CD2`、关态底 `#444444`，白滑块直径 11.5 且四周各留 2 点。
/// 与 `dialog_chrome::toggle` 同一份形状——那边吃 `&mut Ui` 做交互，
/// 这边只要一支 `Painter`，用来画在没有勾选栏的菜单行右端。
pub(in crate::ui) fn paint_switch(p: &egui::Painter, rect: egui::Rect, on: bool) {
    let r = rect.height() / 2.0;
    let bg = if on {
        egui::Color32::from_rgb(0x00, 0x7C, 0xD2)
    } else {
        egui::Color32::from_rgb(0x44, 0x44, 0x44)
    };
    p.rect_filled(rect, egui::Rounding::same(r), bg);
    let knob = rect.height() - 4.0;
    let cx = if on {
        rect.right() - 2.0 - knob / 2.0
    } else {
        rect.left() + 2.0 + knob / 2.0
    };
    p.circle_filled(egui::pos2(cx, rect.center().y), knob / 2.0, egui::Color32::WHITE);
}

/// 菜单行右端那颗开关的尺寸（视觉基线实测）。
pub(in crate::ui) const SWITCH_SIZE: egui::Vec2 = egui::vec2(31.5, 15.5);

/// 一行在绘制时的外观四件套（三态 + 勾选框 + 要不要留白 + 垫到几格）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::ui) struct RowLook {
    pub state: MenuRowState,
    pub check: CheckCell,
    /// 该菜单是否为**所有**行预留勾选栏，见 [`reserve_check_column`]。
    pub indent: bool,
    /// 整张菜单最宽的一行的格宽，见 [`widest_cells`]。每行都垫到它，
    /// 否则 `Button` 的居中会把没有快捷键的行推到右边去。
    pub cells: usize,
}

/// 这个菜单要不要为**所有**行预留勾选栏。
///
/// 视觉基线两张图不一致，这里按实测规律实现（**n=2，别急着推广**）：
///
/// - `mode/01-mode-menu.png`：4 个勾选项、**没有**子菜单 → 连 `Local Mode` /
///   `Hex Edit` 这种无框行的文字也右移到框后，整张菜单共用一条文字竖线；
/// - `view/01-view-menu.png`：1 个勾选项、**有**子菜单 → 只有
///   `Highlight Caret Line` 自己把文字让开，其余行照旧贴左。
///
/// 两张图出自同一个 WindTerm（Qt），两种排法只能说它的列宽是按「菜单 tab」
/// 算的。下一张带勾的菜单出现前，用这条最小判别式对齐现状；出图对不上时
/// **先改这条判据，别去改基线**。
pub(super) fn reserve_check_column(items: &[Item]) -> bool {
    let has_check = items.iter().any(|i| matches!(i, Item::Check { .. }));
    let has_submenu = items.iter().any(|i| matches!(i, Item::Submenu { .. }));
    has_check && !has_submenu
}

/// 画一个方框（框内挖暗、可选的 ✓、禁用时整体降亮度）。
///
/// 2026-11-17：几何与勾形交给 [`crate::ui::kit::checkbox`]，这里只剩「菜单这一档
/// 禁用成什么色」——框线与勾一起按 `#3F3F3F` / `#555555` 暗下去，是菜单独有的规则。
pub(super) fn paint_check(ui: &egui::Ui, row: egui::Rect, on: bool, state: MenuRowState) {
    let rect = check_rect(row);
    let mut props = crate::ui::kit::checkbox::CheckboxProps::menu(on);
    if state == MenuRowState::Disabled {
        props = props
            .dimmed(col(CHECK_BORDER_OFF))
            .marking(col(MENU_TEXT_DISABLED));
    }
    crate::ui::kit::checkbox::paint(ui.painter(), rect, &props);
}

pub(super) fn col(hex: u32) -> egui::Color32 {
    crate::theme::to_color32(term_core::theme::rgb(hex))
}

/// 弹出菜单层的底色（2026-11-08 两张视觉基线实测 `#333333`，比正文
/// `#1C1C1C` 亮一档）。组件夹具（`comp:menu_session` / `comp:menu_log`）
/// 与真实弹层共用这一个值；`theme.rs::window_fill` 同值。
pub(in crate::ui) fn popup_bg() -> egui::Color32 {
    col(MENU_BG)
}

/// 菜单行的三种状态：启用 / 禁用 / 选中。生产路径由「能力台账 + 写入端状态
/// + egui 悬停」得出，组件夹具可以显式摆出任意一态做逐字节比对。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::ui) enum MenuRowState {
    Enabled,
    Disabled,
    Selected,
}

impl MenuRowState {
    /// 文字色：选中态沿用启用色（视觉基线实测 `#BBBBBB`，不是白字）。
    fn text_hex(self) -> u32 {
        match self {
            Self::Disabled => MENU_TEXT_DISABLED,
            Self::Enabled | Self::Selected => MENU_TEXT,
        }
    }

    fn shortcut_hex(self) -> u32 {
        match self {
            Self::Disabled => MENU_TEXT_DISABLED,
            Self::Enabled | Self::Selected => SHORTCUT_GRAY,
        }
    }
}

/// 等宽显示宽：CJK 一律按 2 格算（中文标签的快捷键列才能对齐）。
fn display_cells(s: &str) -> usize {
    s.chars().map(|c| if c as u32 >= 0x2E80 { 2 } else { 1 }).sum()
}

/// [`display_cells`] 的对外名字：面板齿轮菜单等自己排行的弹层也要按同一口径
/// 数字符格，宽度才能和菜单栏算出同一个答案（同一个短语不能一处算 1 格、
/// 一处算 2 格，否则中文界面的弹层会宽窄不一）。
pub(in crate::ui) fn text_cells(s: &str) -> usize {
    display_cells(s)
}

/// 一行文本块的**格宽**（含垫到快捷键列的那串空格 + 快捷键本身）。
///
/// 这个数字必须与 [`menu_item_job`] 里实际拼出来的格数**一模一样**——
/// 两边算错一格，对齐就前功尽弃（[`widest_cells`] 靠它决定垫多少）。
fn row_cells(text: &str, shortcut: &str) -> usize {
    let used = display_cells(text);
    if shortcut.is_empty() {
        used
    } else if used >= SHORTCUT_COL {
        // 文字顶过固定列：它和快捷键之间要留出 [`SHORTCUT_GAP`]，这里折成 1 格
        // 记进宽度（一格约 7 点，正好覆盖那个间隙，行宽才不会把快捷键切掉）。
        used + 1 + display_cells(shortcut)
    } else {
        used.max(SHORTCUT_COL) + display_cells(shortcut)
    }
}

/// 这张菜单里最宽的一行有多少格。
///
/// # 为什么需要它（2026-11-08 对齐事故）
///
/// `egui::Button` 把内容**居中**放进自己的矩形。我们又把每行撑到弹层满宽
/// 去画整行蓝底，于是「文本块比按钮窄」的行会被推到中间：有快捷键的行
/// （块宽 = 23 格 + 快捷键）看起来贴左，没有快捷键的行（块宽 = 标签长度）
/// 整体右移十几像素——同一张菜单里出现两条文字竖线，用户一眼就指出来了。
///
/// 修法不是改对齐（`Button` 不给这个开关），而是把每行的文本块垫到**同一
/// 格宽**：块宽相同 → 居中偏移相同 → 文字落在同一条竖线上。
pub(super) fn widest_cells(win: &TerminalWindow, items: &[Item]) -> usize {
    let mut widest = 0;
    for item in items {
        let cells = match item {
            Item::Action { label, id } | Item::Check { label, id, .. } => {
                row_cells(win.tr(label), term_app::menu::shortcut_text(id))
            }
            // 子菜单的箭头画在弹层右边缘（见 `draw_menu_row` 的 `submenu` 分支），
            // 不占中间的快捷键列，所以这里只算文本宽度；否则 `▸` 会被算进最宽行，
            // 把所有行无谓地垫宽一格。
            Item::Submenu { label, .. } => row_cells(win.tr(label), ""),
            Item::Separator => 0,
        };
        widest = widest.max(cells);
    }
    widest
}

/// 单行文本块（带可选访问字母下划线），颜色烤死为 `color`。
///
/// 颜色全部按 [`MenuRowState`] 烤死，**不用 `PLACEHOLDER`**：后者交给
/// `Button` 按控件状态回填，禁用色随主题走，做不出视觉基线的 `#555555`。
/// 下划线在 layout 时烘焙，同样要烤成对应状态的文字色。
fn text_job(
    text: &str,
    underline: Option<usize>,
    color: egui::Color32,
    font: &egui::FontId,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    // 菜单行绝不换行：任意 wrap 宽度都可能把长标签断成两行。
    job.wrap.max_width = f32::INFINITY;
    let plain = egui::TextFormat::simple(font.clone(), color);
    let underlined = egui::TextFormat {
        underline: egui::Stroke::new(1.0, color),
        ..plain.clone()
    };
    match underline {
        // 按字节位切成三段：前缀 / 访问字母（下划线）/ 其余。
        Some(pos) if text.is_char_boundary(pos) && pos < text.len() => {
            let rest = &text[pos..];
            let acc = rest.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
            job.append(&text[..pos], 0.0, plain.clone());
            job.append(&rest[..acc], 0.0, underlined);
            job.append(&rest[acc..], 0.0, plain.clone());
        }
        _ => job.append(text, 0.0, plain),
    }
    job
}

/// 画一行菜单，显式四栏：`[勾选栏] [文本] [快捷键] [展开 ▸]`。
///
/// 之前用「单个 `Button` + 空格垫列」的等宽对齐 hack：把快捷键当一串空格
/// 垫到固定字符格，依赖字体严格等宽。字体一旦不是等宽、或 CJK 字形宽度漂移，
/// 快捷键列就错位——这正是「看起来不协调」的根因。
///
/// 现在改手绘四栏：勾选栏占 [`CHECK_GUTTER`]、文本左对齐、快捷键落在固定列
/// （[`SHORTCUT_COL`] 字符格，文本更长时退到文本之后）、展开箭头贴右缘。
/// 交互（点击 / 悬停 / 禁用态）与选中蓝块仍由 `allocate` + 手绘负责，
/// 视觉与视觉基线逐像素一致。
pub(in crate::ui) fn draw_menu_row(
    ui: &mut egui::Ui,
    text: &str,
    underline: Option<usize>,
    shortcut: &str,
    look: RowLook,
    submenu: bool,
) -> egui::Response {
    let state = look.state;
    let row_width = ui.available_width();
    let gutter = if look.indent || look.check != CheckCell::None {
        CHECK_GUTTER
    } else {
        0.0
    };
    let font = egui::TextStyle::Button.resolve(ui.style());
    let pad_x = MENU_ROW_PAD_X;
    // 行高与 egui `Button`（内容高 + 上下内边距）同一口径，弹层里各行才能严丝合缝。
    let row_h = ui.text_style_height(&egui::TextStyle::Button) + 2.0 * MENU_ROW_PAD_Y;
    // 禁用行不可点：用 `hover` 感应（仍能显示禁用原因 tooltip），但 `clicked()`
    // 永不为真，避免把「未接线」命令派发出去。
    let sense = if state == MenuRowState::Disabled {
        egui::Sense::hover()
    } else {
        egui::Sense::click()
    };
    let (row_rect, resp) = ui.allocate_exact_size(egui::vec2(row_width, row_h), sense);

    let painter = ui.painter();
    // 选中 / 悬停：整行蓝块（含勾选栏）。禁用行不画高亮。
    if state != MenuRowState::Disabled
        && (state == MenuRowState::Selected || resp.hovered())
    {
        painter.rect_filled(row_rect, egui::Rounding::ZERO, col(MENU_HIGHLIGHT));
    }

    // 栏 1：勾选栏。仅 `Box` 时画框，其余行留白以保持文本竖线对齐。
    // `paint_check` 在传入矩形的左侧加 `CHECK_INSET` 起笔，所以这里必须传
    // **整行** `row_rect`（与旧实现一致），勾选框才会落在左侧 30px 沟槽里，
    // 而不是叠到文本上。
    let inner_left = row_rect.left() + gutter;
    if let CheckCell::Box(on) = look.check {
        paint_check(ui, row_rect, on, state);
    }

    let text_color = col(state.text_hex());
    let sc_color = col(state.shortcut_hex());
    let text_x = inner_left + pad_x;
    // 栏 2：文本（左对齐，访问字母带下划线）。
    let tjob = text_job(text, underline, text_color, &font);
    let tgalley = ui.fonts(|f| f.layout_job(tjob));
    let text_end = text_x + tgalley.size().x;
    let text_y = row_rect.center().y - tgalley.size().y / 2.0;
    painter.galley(egui::pos2(text_x, text_y), tgalley, text_color);

    let char_w = ui.fonts(|f| f.glyph_width(&font, 'M'));
    let column_x = inner_left + pad_x + (SHORTCUT_COL as f32) * char_w;
    // 栏 3：快捷键（固定列；文本更长时退到文本之后，不重叠）。
    if !shortcut.is_empty() {
        // 固定列那一档本身就有空隙；文本顶过列时靠 `SHORTCUT_GAP` 兜底，
        // 不然 22 格的标签会把快捷键粘在最后一个字母上（实测图）。
        let sc_x = column_x.max(text_end + SHORTCUT_GAP);
        let sjob =
            egui::text::LayoutJob::simple(shortcut.to_string(), font.clone(), sc_color, f32::INFINITY);
        let sgalley = ui.fonts(|f| f.layout_job(sjob));
        let sc_y = row_rect.center().y - sgalley.size().y / 2.0;
        painter.galley(egui::pos2(sc_x, sc_y), sgalley, sc_color);
    }
    // 栏 4：展开箭头贴右缘。
    if submenu {
        let ajob = egui::text::LayoutJob::simple("▸".to_string(), font.clone(), sc_color, f32::INFINITY);
        let agalley = ui.fonts(|f| f.layout_job(ajob));
        let a_x = row_rect.right() - pad_x - agalley.size().x;
        let a_y = row_rect.center().y - agalley.size().y / 2.0;
        painter.galley(egui::pos2(a_x, a_y), agalley, sc_color);
    }

    resp
}

/// 视觉基线的分隔线：1px 实线 `#555555`，铺满弹层宽度。
pub(in crate::ui) fn menu_separator(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter_at(rect)
        .rect_filled(rect, egui::Rounding::ZERO, col(MENU_SEPARATOR));
}

/// 弹层内的公共样式：普通项无边框无底，悬停 / 展开整行蓝块。
pub(in crate::ui) fn apply_popup_style(ui: &mut egui::Ui) {
    let w = &mut ui.style_mut().visuals.widgets;
    w.hovered.weak_bg_fill = col(MENU_HIGHLIGHT);
    w.open.weak_bg_fill = col(MENU_HIGHLIGHT);
    w.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    w.inactive.bg_fill = egui::Color32::TRANSPARENT;
    w.inactive.bg_stroke = egui::Stroke::NONE;
    w.active.bg_stroke = egui::Stroke::NONE;
}

/// 弹层里**行**需要多宽（`Frame` 内边距之内）＝ 最宽行的格数 × 等宽字宽
/// + 勾选栏 + 两侧 [`MENU_ROW_PAD_X`]。
///
/// # 为什么必须显式设宽（2026-11-13 根因）
///
/// [`draw_menu_row`] 用 `ui.available_width()` 当行宽去画整行蓝底。而
/// `ui.menu_button` 的弹层是一个 `Area`，egui 用 `spacing.menu_width`
/// （默认 **400**）当它的 `default_width`——不设宽，每行都摊到这 400 点，
/// 弹层跟着变成一堵灰墙，文字全挤在左边，右边一整片空。组件夹具一直没这个
/// 毛病，因为夹具自己 `set_width` 过；于是出现「夹具好看、真菜单不对」，
/// 而且只改夹具、或只改右键菜单，都看不出改善。
///
/// # 行宽 vs 总宽（2026-11-13 第二处事故）
///
/// 这两个数**不能混**：`Frame` 的左右内边距在生产里是 `spacing.menu_margin.x`
/// （本项目 11），夹具里是 4。把「总宽」当行宽塞进 Frame 里，文字就会顶到
/// 右边框（齿轮菜单里 `Transfer` 被吃掉半个字母，实测）。所以：
/// [`rows_width`] = 给 `Frame` **里面**的 `ui.set_width`；
/// [`popup_width`] = 给 `Frame` **外面**的 `Area::set_width`。
pub(in crate::ui) fn rows_width(ui: &egui::Ui, cells: usize, gutter: f32) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let char_w = ui.fonts(|f| f.glyph_width(&font, 'M'));
    cells as f32 * char_w + gutter + 2.0 * MENU_ROW_PAD_X
}

/// 弹层**总宽** ＝ [`rows_width`] + 两侧 `Frame` 内边距。
///
/// `margin_x` 必须传真实值：生产菜单是 `spacing.menu_margin.x`（11），
/// 组件夹具的 `Frame::none().inner_margin(4, 6)` 是 [`POPUP_INNER_X`]。
pub(in crate::ui) fn popup_width(ui: &egui::Ui, cells: usize, gutter: f32, margin_x: f32) -> f32 {
    rows_width(ui, cells, gutter) + 2.0 * margin_x
}

/// 弹层 `Frame` 的左右内边距（与夹具 `inner_margin` 同值）。
///
/// 行宽要在**这层之内**，否则 `Area` 会被撑大、弹层比内容宽一圈。
pub(in crate::ui) const POPUP_INNER_X: f32 = 4.0;

/// 行内文字到行左缘的距离。
///
/// **不从 `spacing.button_padding` 读**：egui 的 `set_menu_style`（`menu_button`
/// 内部每帧都会调）把 `button_padding` 改成 `(2, 0)`，而夹具里是主题的 `(8, 3)`。
/// 于是同一个 `draw_menu_row` 在生产里比基线里窄 12 点、矮 6 点，文字整体左移
/// 6 点——这正是「夹具好看、真菜单对不上」的另一半根因。钉成常量，两边才同尺寸。
pub(in crate::ui) const MENU_ROW_PAD_X: f32 = 8.0;
/// 行内文字上下留白（行高 = 字高 + 2×这个值）。
///
/// 2026-11-14：从 3 → 7。参考实现菜单（同一张 `01-mode-menu.png`）每行上下留白约
/// 5–6px，此前 3 太挤；先试 5 仍偏挤（出图实测行高仍约 19pt），提到 7 后纵向
/// 明显舒展。复选框按比例缩小到 11×11 后，行内也不再被大框挤。
pub(in crate::ui) const MENU_ROW_PAD_Y: f32 = 7.0;

/// 这棵菜单树的**行宽**（`Frame` 之内用）。
pub(in crate::ui) fn menu_rows_width(
    win: &TerminalWindow,
    items: &[Item],
    ui: &egui::Ui,
) -> f32 {
    let gutter = if reserve_check_column(items) {
        CHECK_GUTTER
    } else {
        0.0
    };
    rows_width(ui, widest_cells(win, items), gutter)
}

/// 这棵菜单树的**弹层总宽**（`Area` 上用，夹具的 `Frame` 内边距按 4 算）。
pub(in crate::ui) fn menu_popup_width(
    win: &TerminalWindow,
    items: &[Item],
    ui: &egui::Ui,
) -> f32 {
    let gutter = if reserve_check_column(items) {
        CHECK_GUTTER
    } else {
        0.0
    };
    popup_width(ui, widest_cells(win, items), gutter, POPUP_INNER_X)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三态色值来自 2026-11-08 两张视觉基线的逐像素统计（visual-golden §7.22）。
    /// 钉住它们是防止「用 PLACEHOLDER 让 egui 自动置灰」的老毛病回流：
    /// 那样禁用色随主题走，做不出 `#555555`，也摆不出静态选中态。
    #[test]
    fn the_three_menu_states_keep_their_measured_colors() {
        assert_eq!(MENU_BG, 0x333333);
        assert_eq!(MENU_HIGHLIGHT, 0x007CD2);
        assert_eq!(MenuRowState::Enabled.text_hex(), 0xBBBBBB);
        // 选中态文字不是纯白，仍是 #BBBBBB。
        assert_eq!(MenuRowState::Selected.text_hex(), 0xBBBBBB);
        assert_eq!(MenuRowState::Disabled.text_hex(), 0x555555);
        assert_eq!(MENU_SEPARATOR, MENU_TEXT_DISABLED);
        assert_ne!(
            MenuRowState::Enabled.text_hex(),
            MenuRowState::Disabled.text_hex()
        );
        // 禁用时快捷键与下划线一起变暗，不能留亮色。
        assert_eq!(MenuRowState::Disabled.shortcut_hex(), MENU_TEXT_DISABLED);
        assert_eq!(MenuRowState::Enabled.shortcut_hex(), SHORTCUT_GRAY);
    }

    /// 2026-11-15「快捷命令栏 group 复选框太大」的防回归：所有手绘弹层都从
    /// 同一份几何取边长，行高不同只许传不同的 inset。谁再写一份 14×14，
    /// 这条会红（另一个模块里那份绕过常量写死尺寸，由 `quick_config_menu`
    /// 自己的测试守住）。
    #[test]
    fn every_hand_drawn_checkbox_shares_one_square_size() {
        let row = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(98.0, 28.0));
        let menu = check_rect(row);
        let quick = check_rect_with_inset(row, 8.0);
        assert_eq!(menu.size(), egui::vec2(11.0, 11.0));
        assert_eq!(quick.size(), menu.size(), "边长不许按弹层分家");
        assert_eq!(menu.width(), CHECK_SIZE);
        assert_eq!(menu.center().y, row.center().y, "垂直居中");
        assert_eq!(menu.left(), row.left() + CHECK_INSET);
        assert_eq!(quick.left(), row.left() + 8.0);
        // 框右缘必须留在勾选栏里，不能压到文字列。
        assert!(menu.right() < CHECK_GUTTER, "框右缘越过了勾选栏");
    }

    /// 2026-11-13「所有 menu 都是 400 宽的灰墙」的防回归。
    ///
    /// 机制有两段，缺一句就复发：
    /// 1. `ui.menu_button` 的弹层是 `Area`，egui 用 `spacing.menu_width`
    ///    （默认 **400**）当它的 `default_width`——所以弹层里
    ///    `available_width` 天生就有 400 那么宽，不是「内容宽」；
    /// 2. 手绘行按 `available_width` 铺满，于是整张菜单跟着变成 400 宽。
    ///
    /// 这条测试把两段都钉住：`Area` 里的可用宽远大于内容所需，而
    /// `set_width(popup_width(..))` 之后它就**正好**等于内容宽。
    #[test]
    fn a_popup_is_sized_by_content_not_by_the_areas_default_width() {
        let ctx = egui::Context::default();
        let mut avail_in_area = 0.0;
        let mut want = 0.0;
        let mut pinned = 0.0;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::Area::new(egui::Id::new("popup_width_probe"))
                .default_width(ctx.style().spacing.menu_width)
                .show(ctx, |ui| {
                    avail_in_area = ui.available_width();
                    want = rows_width(ui, 12, CHECK_GUTTER);
                    ui.set_width(want);
                    pinned = ui.available_width();
                });
        });
        assert!(
            (100.0..260.0).contains(&want),
            "12 格 + 勾选栏的行宽应在一两百点量级，实际 {want}"
        );
        assert!(
            avail_in_area > want + 100.0,
            "Area 里的可用宽本来就比内容宽得多（egui 默认 menu_width=400），\
             实际 {avail_in_area} vs {want}——不显式设宽就会变成一堵灰墙"
        );
        assert_eq!(pinned, want, "设宽之后可用宽必须等于内容宽");
    }

    /// 「行宽」和「总宽」只差两侧 `Frame` 内边距。两个数混用就会出现
    /// 2026-11-13 那两次事故：弹层一堵灰墙（该设行宽却用了 400）、
    /// 或文字顶掉右边框（把总宽当行宽塞进 `Frame`）。
    #[test]
    fn total_width_is_row_width_plus_the_frame_margins() {
        let ctx = egui::Context::default();
        let mut row = 0.0;
        let mut total = 0.0;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::Area::new(egui::Id::new("width_algebra_probe")).show(ctx, |ui| {
                row = rows_width(ui, 8, CHECK_GUTTER);
                total = popup_width(ui, 8, CHECK_GUTTER, 11.0);
            });
        });
        assert!((total - row - 22.0).abs() < 0.01, "{total} vs {row}");
    }
}
