//! 视觉基线里的两份上下文菜单：**逐字清单** + 与菜单栏弹层同源的四栏手绘。
//!
//! 基路径见项目日记）：
//!
//! - `05-context-menus/01-terminal-right-click.png`
//! - `05-context-menus/02-session-panel-item-right-click.png`（同一份菜单的另一次弹出）
//! - `06-button-popups/04-quick-popup.png`（另一份更短的视图型菜单）
//!
//! 视觉与菜单栏共用 `shell::menu_style::draw_menu_row`：勾选栏 / 文本 / 快捷键
//! 固定列 / 展开箭头四栏，三态色与分隔线一模一样——右键菜单不再用 egui 默认
//! `Button` 的另一套排版，这是「右键菜单对齐」的核心。
//!
//! **（已接线＝有效果）**：清单里没有对应强类型能力的行一律
//! `MenuRowState::Disabled`，悬停给出「未接线」理由，绝不摆点了没反应的开关。
//! 项目原有但截图里没有的能力（Margin / Zoom / Run Script / 激活窗格）收进
//! 末尾 `More ▸`，见 `context_menu.rs`，保证主菜单与视觉基线 1:1。
//!
//! 2026-11-17 两处接线：
//! - `Log ▸` 展开后与菜单栏 `Session ▸ Log` **同源**（`tree_submenu_items` 从
//!   `term_app::menu::menus()` 取那一棵，判态同一个 `action_state`）；
//! - `Paste Selected Text` 是「把**屏幕上选中的字**当粘贴内容写进 PTY」，
//!   与 `Paste`（读平台剪贴板）不是同一条动作。

use eframe::egui;
use term_app::i18n::{t, Lang};
use term_app::margin::MarginPart;

use crate::ui::shell::{
    apply_popup_style, draw_menu_row, menu_separator, rows_width, CheckCell, MenuRowState, RowLook,
    CHECK_GUTTER, MENU_ROW_PAD_X, MENU_ROW_PAD_Y, POPUP_INNER_X, SHORTCUT_COL,
};

/// 一条菜单行能落地的动作。没有真动作的行 `work: None` → 灰显。
#[derive(Clone, Copy, Debug)]
pub(in crate::ui) enum Work {
    Copy,
    /// 复制 OSC 133 标记的**当前整条命令**（不是选区）到剪贴板。
    ///
    /// 与 `Copy` 是两件事：`Copy` 复制用户已经选中的那几个字，这条按
    /// `blocks::typed_command_rows` 找命令边界、整段取走。2026-11-19 新接。
    CopyCommand,
    Paste,
    /// 把**当前选区**的文本当成粘贴内容写给 PTY（不是剪贴板）。
    ///
    /// 与 `Paste` 是两件事：`Paste` 读平台剪贴板（走 `ViewportCommand::RequestPaste`
    /// → 下一帧 `Event::Paste`），这条读的是屏幕上已经选中的那几个字。
    PasteSelection,
    SelectAll,
    Find,
    ClearScreen,
    /// 只丢历史，屏幕留着。
    ClearScrollback,
    /// 清屏 + 清历史。
    ClearAll,
    /// 跳到上一条 / 下一条命令边界（OSC 133 块表的 prompt_end）。
    GotoCommand { next: bool },
    /// 打开设置并停在 Triggers 页。
    ManageTriggers,
    /// 全屏（与菜单栏 `Window.ToggleFullScreen` 同一条命令）。
    FullScreen,
    Zoom(i8),
    Focus,
    Split(bool), // true = horizontally
    Margin(MarginPart),
    RunScript,
    /// 菜单树里的一条命令（目前只有 `Log ▸` 子树用它）。派发走
    /// `TerminalWindow::dispatch`，与菜单栏点同一条命令是同一条路径。
    Command(&'static str),
}

/// 视觉基线一行的全部可见信息。`work` 为空即未接线。
#[derive(Clone, Copy)]
pub(in crate::ui) struct Row {
    pub label: &'static str,
    pub shortcut: &'static str,
    /// 英文文案里访问字母的**字节**下标（视觉基线的 Qt 助记符）。
    pub acc: Option<usize>,
    pub work: Option<Work>,
    pub submenu: bool,
    /// 子菜单内容取自菜单树里**同名**子菜单（目前的唯一值是 `"Log"`）。
    ///
    /// `None` = 视觉基线有箭头但原程序也没展开过子项（`Search Online` /
    /// `Transfer Binary` / `Reset`），保持灰箭头；`Some(label)` = 展开后画的
    /// 就是菜单栏那一份树（`draw_menu_items`），两处因此不可能对不上。
    pub children: Option<&'static str>,
    /// 判态用的命令 ID：有它就问 `capability::is_implemented` × 写入端状态
    /// （菜单树里的行），没有就按「有没有 `Work` / 子菜单」判。
    pub gate: Option<&'static str>,
    pub check: Option<bool>,
    pub swatches: bool,
    pub separator: bool,
}

const fn row(label: &'static str, shortcut: &'static str, acc: Option<usize>, work: Option<Work>) -> Row {
    Row { label, shortcut, acc, work, submenu: false, children: None, gate: None, check: None, swatches: false, separator: false }
}

const fn disabled(label: &'static str, shortcut: &'static str, acc: Option<usize>) -> Row {
    row(label, shortcut, acc, None)
}

/// 菜单树里的一条命令：判态走 `gate`（`action_state` = 台账 × 写入端），
/// 派发走 `Work::Command(id)` → `dispatch`。两个斜杠里的 ID 必须是**同一个**，
/// 只给一头就会「灰错」或「点了没反应」（与 `tree_rows` 同一口径）。
const fn command(label: &'static str, shortcut: &'static str, acc: Option<usize>, id: &'static str) -> Row {
    Row { work: Some(Work::Command(id)), gate: Some(id), ..row(label, shortcut, acc, None) }
}

const fn sub(label: &'static str, acc: Option<usize>) -> Row {
    Row { submenu: true, ..disabled(label, "", acc) }
}

/// 有真实内容的子菜单：展开时按 `tree_label` 去菜单树里取那一棵。
const fn sub_live(label: &'static str, acc: Option<usize>, tree_label: &'static str) -> Row {
    Row { children: Some(tree_label), ..sub(label, acc) }
}

const fn check(label: &'static str, shortcut: &'static str, acc: Option<usize>, on: bool) -> Row {
    Row { check: Some(on), ..disabled(label, shortcut, acc) }
}

/// 已接线的勾选项：在 [`check`] 上补 `work` + `gate`，判态走台账、点击派发命令。
/// `on` 是夹具出厂态（生产里勾选框的 live 态由菜单树的 `CheckState::Live` 提供）。
const fn command_check(
    label: &'static str,
    shortcut: &'static str,
    acc: Option<usize>,
    id: &'static str,
    on: bool,
) -> Row {
    Row { work: Some(Work::Command(id)), gate: Some(id), ..check(label, shortcut, acc, on) }
}

const fn swatches() -> Row {
    Row { swatches: true, ..disabled("", "", None) }
}

const fn sep() -> Row {
    Row { separator: true, ..disabled("", "", None) }
}

/// `05-context-menus`：终端 / 会话面板条目右键菜单，逐字抄自放大截图。
///
/// 19 个文字行 + 1 个七色圆点行 + 8 条分隔线；4 行带子菜单箭头。
pub(in crate::ui) const TERMINAL_ROWS: &[Row] = &[
    row("Copy", "Shift+Del", Some(0), Some(Work::Copy)),
    // 2026-11-19：`Copy Command` 接成真动作——复用 OSC 133 命令边界整段复制
    // （`effects::copy_command`）。它没有独立命令 ID（不属菜单树），所以走
    // `Work` 而不是 `Work::Command`，与 `Copy` / `Paste` 同一层级。
    row("Copy Command", "", Some(6), Some(Work::CopyCommand)),
    sep(),
    row("Paste", "Shift+Ins", Some(0), Some(Work::Paste)),
    // 2026-11-17 用户校正：「粘贴选中」不是「粘贴」的别名——它粘的是**屏幕上
    // 已选中的文本**，不碰剪贴板。两行因此各接各的 `Work`。
    row(
        "Paste Selected Text",
        "Meta+Shift+X",
        Some(6),
        Some(Work::PasteSelection),
    ),
    swatches(),
    sep(),
    // 2026-11-19：底层 `Text.HighlightText`（CenterPage::HighlightText）已接线，
    // 渲染读者在 `pane_overlays.rs`；原先手绘 disabled 是「假灰」，现在按台账接真。
    command("Highlight Text", "", Some(0), term_app::menu::context_ids::HIGHLIGHT_TEXT),
    sep(),
    row("Find", "Meta+F", Some(0), Some(Work::Find)),
    // 菜单树里 `Search Online ▸` 有一棵真子树（`Text.SearchOnline` 已接线），
    // 原来用 `sub()` 画成灰箭头，与菜单栏不一致；改用 `sub_live` 读同一棵树。
    sub_live("Search Online", Some(0), "Search Online"),
    sep(),
    row(
        "Clear Scrollback",
        "Meta+Shift+L",
        Some(6),
        Some(Work::ClearScrollback),
    ),
    row("Clear Screen", "Meta+L", Some(6), Some(Work::ClearScreen)),
    row("Clear All", "", Some(6), Some(Work::ClearAll)),
    sep(),
    row("Select All", "Meta+Shift+A", Some(7), Some(Work::SelectAll)),
    // 2026-11-19：`Text.SelectCommand` 已接线（OSC 133 命令选区），接真。
    command("Select Command", "Ctrl+Shift+/", Some(7), term_app::menu::context_ids::SELECT_COMMAND),
    sep(),
    row(
        "Goto Previous Command",
        "Meta+Shift+Left",
        Some(5),
        Some(Work::GotoCommand { next: false }),
    ),
    row(
        "Goto Next Command",
        "Meta+Shift+Right",
        Some(5),
        Some(Work::GotoCommand { next: true }),
    ),
    row("Session Triggers", "", Some(8), Some(Work::ManageTriggers)),
    sep(),
    // 2026-11-17 用户要求：`Transfer Binary` 展开后要与「Tool ▸ Transfer Binary」
    // **一致**（视觉基线里 Send 三条一组 / Receive 三条一组 / Cancel 带 `Meta+C`，
    // 中间两条分隔线）。做法同下面的 `Log`：读同一棵菜单树，不抄第二份。
    sub_live("Transfer Binary", Some(0), "Transfer Binary"),
    sep(),
    // 2026-11-17 用户要求：这里的 Log 展开后要与「Session ▸ Log」**一致**。
    // 做法不是把七条命令抄一份，而是让两者读**同一棵树**（`term_app::menu::menus`）：
    // 分隔线、命令 ID、可用性（`is_implemented` × 日志写入端状态）全部同源。
    sub_live("Log", Some(0), "Log"),
    sep(),
    sub("Reset", Some(0)),
];

/// 工具栏 ⋮ 按钮（Show More Tools）的视图型上下文菜单。
///
/// 真值：`06-button-popups/show-more-tools-clear.png` + `address-filter-down.png`
/// 两张**正式入库的局部清晰图**（2026-11-12 补采，两图互证）。12 个文字行 +
/// 3 个勾选框 + 4 条分隔线，全套快捷键。
///
/// 注意：08-20 那张 clipboard 整窗裁图曾显示「无快捷键 / 无 Paste Selected Text」，
/// 与本次两张局部图相反——那是整窗缩图丢字，已弃用；以本次局部图为准。
///
/// 对齐策略：文字 / 快捷键 / 行结构照局部图逐字抄；灰显态按「已接线＝有效果」判。
///
/// 2026-11-19 校正：原先这段按「只有 Copy/Paste/Zoom 有真 Work」把 Save Session
/// 与 Line Endings / Wrap Symbols 一并灰掉，但那三条命令（`Terminal.Save` /
/// `View.ToggleLineEndings` / `View.ToggleWrapSymbols`）早已接线并有读者（见
/// `capability.rs`），是「假灰」。现在按台账接真。`Word Wrap` 仍是 `Planned`
/// （要内核 reflow），保持灰勾选——视觉基线图里它亮，但按 ADR-0015 不摆假开关。
pub(in crate::ui) const VIEW_ROWS: &[Row] = &[
    row("Copy", "Shift+Del", Some(0), Some(Work::Copy)),
    row("Paste", "Shift+Ins", Some(0), Some(Work::Paste)),
    // 与内容菜单同一行同一条动作：粘的是**屏幕上的选区**，不是剪贴板。
    row(
        "Paste Selected Text",
        "Meta+Shift+X",
        Some(9),
        Some(Work::PasteSelection),
    ),
    sep(),
    // 2026-11-19：`Terminal.Save` 已接线（CenterPage::Save → 保存到会话库），接真。
    command("Save Session", "Meta+S", Some(0), term_app::menu::context_ids::SAVE_SESSION),
    sep(),
    // Word Wrap 仍 Planned（需内核 reflow），保持灰勾选。
    check("Word Wrap", "Alt+M, Alt+W", Some(0), true),
    // 这两条命令已接线（`parse.rs:106/109` → MarksKnob → Tab.marks），接真。
    // 勾选态走 gate 判态时不读常量 on；这里把 work + gate 补上，勾选框的
    // 常量 false 只是夹具出厂态（真实 live 态由菜单树的 CheckState::Live 提供）。
    command_check("Line Endings", "Alt+T, Alt+E", Some(0), term_app::menu::context_ids::LINE_ENDINGS, false),
    command_check("Wrap Symbols", "Alt+T, Alt+W", Some(0), term_app::menu::context_ids::WRAP_SYMBOLS, false),
    sep(),
    row("Zoom In", "Meta+=", Some(5), Some(Work::Zoom(1))),
    row("Zoom Out", "Meta+-", Some(5), Some(Work::Zoom(-1))),
    sep(),
    row(
        "Split View Horizontally",
        "Alt+W, Alt+H",
        Some(0),
        Some(Work::Split(true)),
    ),
    row(
        "Split View Vertically",
        "Alt+W, Alt+V",
        Some(0),
        Some(Work::Split(false)),
    ),
    row("Full Screen", "Meta+F11", Some(0), Some(Work::FullScreen)),
];

/// 视觉基线那排七色圆点：红 / 橙 / 黄 / 绿 / 蓝 / 青 / 紫。
const SWATCHES: [(u8, u8, u8); 7] = [
    (0xE4, 0x3B, 0x35),
    (0xEA, 0x8C, 0x22),
    (0xF0, 0xCF, 0x2A),
    (0x2F, 0xA6, 0x48),
    (0x3B, 0x6F, 0xD6),
    (0x15, 0xB7, 0xC9),
    (0xA4, 0x3C, 0xB6),
];

/// 画七色圆点行。行高与 `draw_menu_row` 一致，圆点等距铺满一行。
///
/// 「给选中文本上色」目前没有纯层能力与渲染读者，不接成可点开关：
/// 圆点照视觉基线亮着，但只 `Sense::hover` + 悬停说明未接线。
fn draw_swatches(lang: Lang, ui: &mut egui::Ui) {
    // 行高与左右留白取菜单行的同一组常量（`MENU_ROW_PAD_*`），圆点行才和
    // 上下文字行严丝合缝——`button_padding` 在菜单弹层里会被 egui 改成 (2,0)。
    let row_h = ui.text_style_height(&egui::TextStyle::Button) + 2.0 * MENU_ROW_PAD_Y;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), row_h),
        egui::Sense::hover(),
    );
    let pad = MENU_ROW_PAD_X;
    let span = rect.width() - 2.0 * pad;
    let radius = 5.5;
    for (i, (r, g, b)) in SWATCHES.iter().enumerate() {
        let x = rect.left() + pad + span * (i as f32 + 0.5) / SWATCHES.len() as f32;
        ui.painter().circle_filled(
            egui::pos2(x, rect.center().y),
            radius,
            egui::Color32::from_rgb(*r, *g, *b),
        );
    }
    resp.on_hover_text(t(lang, "Not wired (placeholder)"));
}

/// 一行的三态。抽成纯函数是为了能单测这条判据：没有 `Work`（也没有可展开的子菜单）
/// 的行**必须**是 `Disabled`，防止日后往清单里塞一条「看得见但点了没反应」的开关。
///
/// `pointed` 是截图那一瞬鼠标划过的行，`Selected` 在这里只用来**定格指向指示色**
/// （整行蓝块与 `draw_menu_row` 的 `resp.hovered()` 同色），不是第四种「选中」态。
/// 生产路径传 `None`，蓝块由真实 hover 自己出。
///
/// 有内容的子菜单（`children: Some`）算 Enabled：它点了确实会展开一棵树，
/// 只是那一棵的每一行由菜单栏同一份代码判态。
fn row_state(r: &Row, pointed: Option<&'static str>) -> MenuRowState {
    if pointed == Some(r.label) {
        MenuRowState::Selected
    } else if r.work.is_some() || r.children.is_some() {
        MenuRowState::Enabled
    } else {
        MenuRowState::Disabled
    }
}

/// 画一份上下文菜单。生产路径与 `--demo comp:` 夹具共用这一处，
/// 差别只在 `pointed`（静态出图没有真鼠标，用它把视觉基线里鼠标指向的那行
/// 定格成蓝块）。
///
/// `win` 只为**有内容的子菜单**存在：那些子菜单展开后走菜单栏那一份
/// `draw_menu_items`（同一棵树、同一套判态与派发），所以这里需要一个能
/// `dispatch` 的窗口。主行一个字节都不读 `win`。
pub(in crate::ui) fn draw_context_rows(
    win: &mut crate::ui::TerminalWindow,
    lang: Lang,
    ui: &mut egui::Ui,
    rows: &[Row],
    work: &mut Vec<Work>,
    pointed: Option<&'static str>,
) {
    apply_popup_style(ui);
    // 宽度**自适应内容**，且与菜单栏共用 [`crate::ui::shell::rows_width`] 这一条
    // 公式：右键菜单和下拉菜单从此只有一种宽度答案。`context_menu` 的弹层同样是
    // 由 egui 建的 `Area`（`available_width` 是 `spacing.menu_width`，默认 400），
    // 不设宽每行都会摊满那 400 点。
    ui.set_width(context_rows_width(ui, rows));
    // 有勾选框的菜单：所有行共享同一条文字竖线（与 `menu_style::reserve_check_column`
    // 同一判据，区别是这里没有子菜单）。
    let base = RowLook {
        state: MenuRowState::Enabled,
        check: CheckCell::None,
        indent: rows.iter().any(|r| r.check.is_some()),
        cells: 0,
    };
    for r in rows {
        if r.separator {
            menu_separator(ui);
            continue;
        }
        if r.swatches {
            draw_swatches(lang, ui);
            continue;
        }
        // 有内容的子菜单（`Log`）：行本身仍然是**手绘**的（`▸` 箭头、行高、
        // 三态色与其它 25 行同一个 `draw_menu_row`），展开的弹层再从菜单树取
        // 同一棵——「两处一致」靠同源数据 + 同一套行原语，而不是靠两处长得像。
        //
        // 为什么不直接 `ui.menu_button`（菜单栏那条路）：egui 的 `SubMenuButton`
        // 在本项目里有两个硬伤——行高比手绘行少 3 点（同一张菜单两种行距），
        // 箭头用 `⏵`(U+23F5) 而这个字形在本项目加载的字体里没有，实测**一个
        // 像素都不画**（菜单栏里能看见的箭头其实是夹具 `draw_menu_items_static`
        // 手绘的 `▸`）。所以展开态自己摆一个 `Area`，行还是同一个函数画。
        if r.children.is_some() {
            draw_children_row(win, lang, ui, r, &base, work);
            continue;
        }
        let text = t(lang, r.label);
        // 中文文案按首字划下划线（与 `menu_items::underline_for` 同一口径）。
        let acc = if text == r.label { r.acc } else { Some(0) };
        // `gate` 的行是菜单树里来的命令：可用性问**同一个**判据
        // （`is_implemented` × 日志写入端状态），与菜单栏里那一份逐行相同。
        let (state, why) = match r.gate {
            Some(id) => crate::ui::shell::action_state(win, id),
            None => (row_state(r, pointed), "Not wired (placeholder)"),
        };
        let check = match r.check {
            Some(on) => CheckCell::Box(on),
            None => CheckCell::None,
        };
        let response = draw_menu_row(
            ui,
            text,
            acc,
            r.shortcut,
            RowLook { state, check, ..base },
            r.submenu,
        )
        .on_disabled_hover_text(t(lang, why));
        if response.clicked() {
            if let Some(w) = r.work {
                work.push(w);
                ui.close_menu();
            }
        }
    }
}

/// 画一条「有内容的子菜单」行，并把它那一棵子树挂在悬停弹层里。
///
/// 树里一条命令都没有时退回灰箭头（与 `menu_items` 里「空子菜单不画成能点开
/// 的样子」同一判据）：宁可灰着，也不摆一个点开只有横线的箭头。
fn draw_children_row(
    win: &mut crate::ui::TerminalWindow,
    lang: Lang,
    ui: &mut egui::Ui,
    r: &Row,
    base: &RowLook,
    work: &mut Vec<Work>,
) {
    let Some(tree_label) = r.children else {
        return;
    };
    let items = tree_submenu_items(tree_label);
    let live = items.iter().any(|i| {
        matches!(
            i,
            term_app::menu::Item::Action { .. } | term_app::menu::Item::Check { .. }
        )
    });
    let text = t(lang, r.label);
    let acc = if text == r.label { r.acc } else { Some(0) };
    let state = if live { MenuRowState::Enabled } else { MenuRowState::Disabled };
    let row = draw_menu_row(
        ui,
        text,
        acc,
        r.shortcut,
        RowLook { state, ..*base },
        true,
    )
    .on_disabled_hover_text(t(lang, "Not wired (placeholder)"));
    if !live {
        return;
    }
    let rows = tree_rows(&items);
    submenu_popup(win, lang, ui, &row, r.label, &rows, work);
}

/// 悬停展开的那层弹层。
///
/// 「指针在上一次的弹层里也算展开」是必需的：从行移进弹层的那一帧 `hovered()`
/// 已经是假，不认上一帧的矩形，弹层就会闪一下再重开。egui 把每个 `Area`
/// 上一帧的矩形存在 memory 里（`Memory::area_rect`），查它即可——自己存一份
/// 「开着没开着」还得处理父菜单关闭时的残留，反而更容易错。
fn submenu_popup(
    win: &mut crate::ui::TerminalWindow,
    lang: Lang,
    ui: &mut egui::Ui,
    row: &egui::Response,
    label: &'static str,
    rows: &[Row],
    work: &mut Vec<Work>,
) {
    let area_id = ui.id().with(("ctx_submenu", label));
    let over_popup = ui.ctx().pointer_hover_pos().is_some_and(|p| {
        ui.ctx()
            .memory(|m| m.area_rect(area_id).is_some_and(|r| r.contains(p)))
    });
    // 出图夹具（不入库）：静态图里没有鼠标，悬停展开的弹层永远拍不到。
    // `WINDTERM_CTX_SUBMENU=Transfer Binary` 强制摆开这一棵子树，
    // 供 `--demo comp:menu_terminal_context --screenshot` 与视觉基线逐行比对。
    // 默认不设 = 生产路径零变化。
    let forced = std::env::var("WINDTERM_CTX_SUBMENU").is_ok_and(|v| v == label);
    if !row.hovered() && !over_popup && !forced {
        return;
    }
    egui::Area::new(area_id)
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(row.rect.right(), row.rect.top()))
        // 夹具时钟 t=0，默认淡入会把弹层底色合成成深色（与菜单夹具同一条）。
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            // 外框与父菜单同一份 `Frame::menu`：内边距 / 圆角 / 底色 / 阴影
            // 都取 egui 的菜单令牌，两层看起来才是同一套弹层。
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                draw_context_rows(win, lang, ui, rows, work, None);
            });
        });
}

/// 菜单树 → 手绘行。命令 ID 进 `gate`（判态）与 `Work::Command`（派发），
/// 文案 / 快捷键 / 访问字母与菜单栏那一份同源（`shortcut_text` /
/// `access_char_pos`），所以「右键里的 Log」与「Session ▸ Log」逐行相同。
fn tree_rows(items: &[term_app::menu::Item]) -> Vec<Row> {
    use term_app::menu::Item;
    items
        .iter()
        .map(|it| match it {
            Item::Action { label, id } => Row {
                acc: term_app::menu::access_char_pos(label),
                shortcut: term_app::menu::shortcut_text(id),
                work: Some(Work::Command(id)),
                gate: Some(id),
                ..row(label, "", None, None)
            },
            // 勾选型命令（Log 子树里目前没有，留着以免将来加了就变成一条死行）：
            // 勾由 `Live` 现读，这里按未勾画——本菜单没有 `Log` 之外的子树。
            Item::Check { label, id, .. } => Row {
                acc: term_app::menu::access_char_pos(label),
                shortcut: term_app::menu::shortcut_text(id),
                work: Some(Work::Command(id)),
                gate: Some(id),
                ..row(label, "", None, None)
            },
            Item::Separator => sep(),
            // 三层以上子菜单（`Log` 里没有）：保持灰箭头，不递归。
            Item::Submenu { label, .. } => sub(label, Some(0)),
        })
        .collect()
}

/// 菜单树里同名子菜单的条目（`"Log"` → Session 菜单下那 7 条命令 + 3 条分隔线）。
///
/// 深度优先找第一个同名子菜单；找不到就是空表，调用方据此退回灰箭头。
/// 抄一份常量也能画出一样的东西，但那正是「两处各写一遍、迟早只改一处」的由来。
fn tree_submenu_items(tree_label: &str) -> Vec<term_app::menu::Item> {
    fn walk(items: &[term_app::menu::Item], want: &str) -> Option<Vec<term_app::menu::Item>> {
        for item in items {
            if let term_app::menu::Item::Submenu { label, items } = item {
                if *label == want {
                    return Some(items.clone());
                }
                if let Some(found) = walk(items, want) {
                    return Some(found);
                }
            }
        }
        None
    }
    for menu in term_app::menu::menus() {
        if let Some(found) = walk(&menu.items, tree_label) {
            return found;
        }
    }
    Vec::new()
}

/// 上下文菜单的**行宽**（`Frame` 之内用；总宽由调用方的 `Frame` 内边距加上去）。
///
/// 与菜单栏 [`crate::ui::shell::menu_style::menu_rows_width`] 共用
/// [`rows_width`] 这一条公式——右键菜单与下拉菜单的宽度从此只有一个来源，
/// 不会再出现「一边自适应、一边写死」。
pub(in crate::ui) fn context_rows_width(ui: &egui::Ui, rows: &[Row]) -> f32 {
    let gutter = if rows.iter().any(|r| r.check.is_some()) {
        CHECK_GUTTER
    } else {
        0.0
    };
    let mut cells = 0usize;
    for r in rows {
        if r.separator || r.swatches {
            continue;
        }
        let t = r.label.chars().count();
        // 与 `menu_style::row_cells` 同一条规则：文本顶过固定列时要多留 1 格
        // 当「文字 → 快捷键」的间隙，否则 `Split View Horizontally` 那行会把
        // `Alt+W, Alt+H` 粘在文字上。
        let sc = r.shortcut.chars().count();
        let row = if r.shortcut.is_empty() {
            t
        } else if t >= SHORTCUT_COL {
            t + 1 + sc
        } else {
            t.max(SHORTCUT_COL) + sc
        };
        cells = cells.max(row + usize::from(r.submenu));
    }
    // 180 是菜单该有的最小「观感宽度」（原图最窄的弹层）；它含两侧 4 的 Frame
    // 内边距，扣掉再当行宽。
    rows_width(ui, cells, gutter).max(180.0 - 2.0 * POPUP_INNER_X)
}

/// 终端左边缘（margin 栏）右键菜单的行定义（兼容终端：左边缘与内容区两份菜单）。
/// 勾选态按当前 `MarginState` 现算，所以每帧重建。
pub(in crate::ui) fn margin_rows(
    margin: &term_app::margin::MarginState,
    lang: Lang,
) -> Vec<Row> {
    let mk = |label: &'static str, sc: &'static str, work: Option<Work>, on: bool| Row {
        label,
        shortcut: sc,
        acc: Some(0),
        work,
        submenu: false,
        children: None,
        gate: None,
        check: Some(on),
        swatches: false,
        separator: false,
    };
    vec![
        // 键位原文取 `term-app` 那张表——与绑定表同源，菜单写着什么就真能按出什么。
        mk(t(lang, "Margin Blank"), term_app::menu::margin_seq(MarginPart::Blank), Some(Work::Margin(MarginPart::Blank)), margin.get(MarginPart::Blank)),
        mk(t(lang, "Margin Number"), term_app::menu::margin_seq(MarginPart::Number), Some(Work::Margin(MarginPart::Number)), margin.get(MarginPart::Number)),
        // 2026-11-19：折叠引擎（2026-11-14 接通）已有折叠头绘制读者
        // （`terminal/margin.rs::is_fold_head`），这里缺的只是一行 Work——
        // 与上下四列开关同走 `Action::Margin(Toggle(Fold))`，接真。
        mk(t(lang, "Margin Fold"), term_app::menu::margin_seq(MarginPart::Fold), Some(Work::Margin(MarginPart::Fold)), margin.get(MarginPart::Fold)),
        mk(t(lang, "Margin Symbol"), term_app::menu::margin_seq(MarginPart::Symbol), Some(Work::Margin(MarginPart::Symbol)), margin.get(MarginPart::Symbol)),
        mk(t(lang, "Margin Timestamp"), term_app::menu::margin_seq(MarginPart::Timestamp), Some(Work::Margin(MarginPart::Timestamp)), margin.get(MarginPart::Timestamp)),
        sep(),
        // Symbols / White Spaces / Folds：View 里同名（未接线），手绘灰显箭头行。
        sub(t(lang, "Symbols"), Some(0)),
        sub(t(lang, "White Spaces"), Some(0)),
        sub(t(lang, "Folds"), Some(0)),
        sep(),
        // 手绘框架不展开子菜单，Zoom 用三行内联（功能可用 + 同风格）。
        row(t(lang, "Zoom In"), "Meta+=", Some(5), Some(Work::Zoom(1))),
        row(t(lang, "Zoom Out"), "Meta+-", Some(5), Some(Work::Zoom(-1))),
        row(t(lang, "Zoom Reset"), "Meta+0", Some(5), Some(Work::Zoom(0))),
    ]
}

// 行表 + 接线的单测单独一个文件：2026-11-17 接通 `Log ▸` 与「粘贴选中」后
// 本文件逼近门禁 4j 的 500 代码行上限，测试塞在里面就只能删注释或删行。
#[cfg(test)]
#[path = "context_rows_tests.rs"]
mod tests;
