//! 窗口外壳：菜单栏 / 标签条 / 窗格工具行 / 快捷命令栏 / 状态栏 / 查找栏。
//!
//! 这几件是**固定框架**：它们不参与停靠与浮动（布局上钉死），
//! 所以单独成组，和 `panels`（可停靠面板）分开。
//!
//! 标签条与窗格工具行是**两条**独立的 `TopBottomPanel`，见 `pane_tools.rs`
//! 的模块头——视觉基线里从来不是一行。
//! 快捷命令栏与状态栏是窗口底部**相邻的两条整宽横栏**，见
//! `docs/design/13-ui-shell.md`；声明顺序决定谁贴最底边（状态栏先声明）。

mod find_bar;
/// `×` / `⚙` 的**唯一**画法与落位。2026-11-17 之前仓里有五份 `×`、三份 `⚙`，
/// 同一个窗口右下角能量出三种尺寸——见模块头那张对照表。
mod glyph;
mod menu_bar;
// 2026-11-08：菜单绘制按「顶栏 / 一棵树 / 一行」三分。
// `menu_bar` 摆顶级按钮，`menu_items` 遍历菜单树，`menu_style` 画单行。
mod menu_items;
mod menu_style;
mod menu_tools;
/// 三个尚未挂接的视觉基线弹层（shell 下拉 / Outline 菜单 / 地址栏下拉）。
mod popup_lists;
mod pane_tools;
mod quick_command_bar;
mod quick_config_menu;
mod quick_layout;
mod quick_mark;
mod status_bar;
mod sysmon_bar;
mod tab_bar;
mod tab_menu;
mod tab_strip;
mod tool_bar;
mod tool_bar_icons;
mod tool_bar_tail;

pub(in crate::ui) use find_bar::{draw_find_bar, FIND_BAR_H};
// 2026-11-15：`draw_menu_items` 只在 `menu_bar` 里由 `super::menu_items::` 直接
// 引用（生产二级菜单需要真实 MenuState）；组件夹具改走静态树，这个再导出
// 已无人使用，去掉以免 `clippy -D warnings` 红。
//
// 2026-11-17：右键菜单的 `Log ▸` 展开后是菜单树里的**同一批命令**，判态必须
// 是同一个谓词（`is_implemented` × 日志写入端状态），于是放开 `action_state`。
// 行仍然手绘——egui 的 `SubMenuButton` 行高差 3 点、箭头字形在本项目字体里
// 没有，混用就成了同一张菜单两种行。
pub(in crate::ui) use menu_items::{action_state, draw_menu_items_static};
pub(in crate::ui) use menu_style::popup_bg;
pub(in crate::ui) use menu_style::menu_popup_width;
// 右键菜单（terminal/context_rows.rs）复用同一套四栏手绘，视觉才和菜单栏弹层
// 对齐；这里只放宽可见性，不改任何颜色/几何常量。
pub(in crate::ui) use menu_style::{
    apply_popup_style, draw_menu_row, menu_separator, rows_width, text_cells, CheckCell,
    MenuRowState, RowLook, CHECK_GUTTER, MENU_ROW_PAD_X, MENU_ROW_PAD_Y, POPUP_INNER_X,
    SHORTCUT_COL,
};
pub(in crate::ui) use popup_lists::{draw_address_dropdown, draw_outline_menu, draw_shell_list};
// 真地址栏（`ui/address/rows.rs`）复用参考夹具那一套颜色与图标——单一份
// 定义，两处画出来才是同一个组件。`draw_address_dropdown` 本身一个字节没动。
pub(in crate::ui) use popup_lists::{
    action_cyan, address_blue, draw_session_glyph, group_green, paint_right,
};
pub(in crate::ui) use sysmon_bar::{sysmon_bar_frame, SYSMON_BAR_H};
pub(in crate::ui) use quick_command_bar::draw_quick_command_bar;
// 2026-11-17：`×` / `⚙` 收归 `glyph` 一处。原先 `paint_close_cross` 住在
// `tab_bar`，`paint_gear` 住在 `panels/session_icons`，`quick_command_bar` 还有
// 第三套（排字体字形）——四份实现各自长歪。现在全部指向 `glyph`，调用点的
// 可见性不变（仍是 `pub(in crate::ui)`）。
pub(in crate::ui) use glyph::{paint_cross, paint_gear};
// Tabs 设置页的下拉与标签条画的是同一份档位名：文案只有一处（见 `tab_bar`）。
// 2026-11-15：`ACTIVE_LINE` / `LINE_H` / `paint_tab_icon` / `paint_right_tool`
// 一并放开给 `panels/shell_panel.rs`——底带 Shell 面板的内标签条画的**就是**
// 这一套（活动标记 / 图标 / ＋、≡），只放宽可见性，颜色与几何一个字节都没动。
pub(in crate::ui) use tab_bar::{paint_right_tool, paint_tab_icon, tab_number_prefix, ACTIVE_LINE, LINE_H};
// 分屏时每片 pane 画**整条**工具栏（不只是面包屑）——图标组 / 抓手 /
// 右端控件全在 `tool_bar::draw` 里，参数化后单窗格与分屏共用一份实现。
pub(in crate::ui) use tool_bar::{draw as draw_tool_bar, BarScope};
pub(in crate::ui) use tab_bar::ELIDE_MODES;
pub(in crate::ui) use tab_strip::{ToolAction, ToolButton};

use term_core::theme::DESIGN_TOKENS;

/// 两条底栏的高度。
///
/// 2026-11-17 按用户新视觉基线重测（@2× 截图 ÷2）：快捷栏 69.5..97、状态栏
/// 97..124.5，**两条都是 27.5**、等高且紧邻。此前是「token 23 再各收 2/1」，
/// 那套是为了对齐更早的一张视觉基线静帧，比现在的视觉基线矮 6.5。
pub(in crate::ui) const STATUS_BAR_H: f32 = DESIGN_TOKENS.status_h;
/// 快捷命令栏与状态栏等高（视觉基线实测两条同高）。
pub(in crate::ui) const QUICK_BAR_H: f32 = DESIGN_TOKENS.status_h;

// ── 顶部三条：必须**定高 + 垂直居中**，否则文字整体偏上 ──
//
// 2026-11-07 实测：这三个 `TopBottomPanel` 原本都没给 `exact_height`，高度由
// 内容撑开，而里面的 `ui.horizontal` 是**顶部对齐**——于是同一行里 24px 的图标
// 按钮贴着上沿、14px 的文字也贴着上沿，两者墨迹中线差 2–2.75px（菜单栏最明显）。
// 定高之后配 `horizontal_centered`，一行里的文字与图标共用同一条中线。
//
// 值取自新版截图逐像素测量（2× 图，除以 2）：菜单栏 62 px→31、标签条 60→30、
// 工具行 52→26。**2026-11-07 第二次实测修正**：菜单栏原先按 21 定高，是把
// 「条体」之外的分隔线也算进了别处；大图（2764×796）里 y=0..61 是条体、
// y=62..63 是 1 px `#444444` 分隔线、y=64 起才是标签条，所以条体是 31 而不是 21。
//
// **注意这个常量含分隔线**：视觉基线是「条体 31 + 1 px 分隔线」，而 `TopBottomPanel`
// 的 `show_separator_line` 把线画在条**内**的最后 1 px。所以要让标签条落在参考的
// 同一像素（y=64 @2×），这里得填 31 + 1 = 32。
//
// **2026-11-12 token 接线**：本常量改由 `DESIGN_TOKENS.menu_row_h`（33.0）
// 派生，减去条内 1 px 分隔线后 = 32.0，与实测值一致。原先 32.0 是硬编码魔数，
// `menu_row_h` 令牌从未被消费（死令牌）；用户拍板「先完成功能、后统一修基线」，
// 故此处直接接线。33（token 条体）vs 31（实测条体）的 2px 差异在后续基线
// 评审时一并裁定（见 `p0-2-menu-deviation.md` §3 口径冲突）。
pub(in crate::ui) const MENU_BAR_H: f32 = DESIGN_TOKENS.menu_row_h - 1.0;

/// 菜单栏相邻两项**墨迹之间**的间距。
///
/// 2026-11-07 逐像素测速（2× 图，十项全量）：
///
/// | | 视觉基线 | 修改前 | 修改后 |
/// |---|---|---|---|
/// | 首项墨迹左边 | 24 px | 39 px | 23 px |
/// | 相邻两项墨迹间隙 | 31–34 px | 47–48 px | 31.5 px |
///
/// 修改前的间隙是 `button_padding.x(8)×2 + item_spacing.x(6) = 22`，比视觉基线
/// 宽 7.25 逻辑像素；十项累计漂移 70 px，整排菜单明显「散」。视觉基线的实际
/// 构成是**零内边距 + 间距**。零内边距同时让首项墨迹回到 11 + 0.5 = 11.5，
/// 与参考的 12 对齐。
///
/// 间距取 15.5 而不是逐项反推出的 14：本机的等宽字形比视觉基线窄 0.5–0.7 逻辑
/// 像素/项（同词同字号，`Session` 96 vs 97、`Selection` 123 vs 127、`Window`
/// 85 vs 86）。只对齐单项间隙的话，十项会一路收窄，末项右缘落在 1000 px 而
/// 参考是 1027 px——整排明显短一截。把这点字形差补进间隙后，十项起止两端
/// 同时对齐。
pub(in crate::ui) const MENU_ITEM_GAP: f32 = 15.5;

/// 标签条：活动标签块 24.5 高，上下各留 1.75。
///
/// **2026-11-07 第三次实测**：大图（1576×298 @2×）里标签条占 y 4..63 → 30 逻辑
/// 点，原先按 28 定高少了 2 px，标签块因此比参考矮一档。
pub(in crate::ui) const TAB_BAR_H: f32 = 30.0;
/// 窗格工具行：大图 y 64..115 → **26** 逻辑点。
///
/// 这一条里装的是「拖动抓手 + 三颗窗格动作 + 可拖走的工具栏面板」，见
/// `tool_bar.rs`。原先按 28 定高，比参考高 2 px，工具行与终端的分界因此整体
/// 下移一行。
pub(in crate::ui) const PANE_TOOLS_H: f32 = 26.0;

/// 顶部三条横栏共用的框：**纵向零内边距**。
///
/// `egui` 默认的 `Frame::side_top_panel` 会在上下各加 `menu_margin.y`（3px），
/// 于是 `exact_height(21)` 实际画出 27px 的条——「设了定高却不等于实际高度」
/// 是上面那批对齐问题里最隐蔽的一条。横向仍保留 `menu_pad_x`，第一个菜单 /
/// 第一个标签才不会贴到窗口边上。
/// Find 栏专用的框：**零内边距**。
///
/// 它内部所有控件都按视觉基线的绝对坐标画（`Find:` 在 x=11、输入框在 x=52、
/// `Find All` 右沿距窗口右端 8.5），套上 `bar_frame()` 那圈 11 点 margin 会让
/// 整条右移。底色与 `dialog` 同色——视觉基线里这条栏与终端区同底色，只靠上下
/// 各一条 `#444444` 1 点线分界，那两条线由 `find_bar.rs` 自己画。
pub(in crate::ui) fn find_bar_frame() -> eframe::egui::Frame {
    eframe::egui::Frame::none()
        .fill(crate::theme::chrome(term_core::theme::CHROME_SCALE.dialog))
}

/// 状态栏的框。`accent = true` 时整条填那个指向蓝——查找命中时状态栏被染色，
/// 这是视觉基线里「Find 栏与状态栏联动」的样子（`18 of 30 Found` 白字压在蓝底上）。
/// 底色必须在 **frame** 这一层换：panel 背景由 frame 画，内容层再铺一块会漏掉
/// 左右两圈 `menu_pad_x` 的内边距，边上留两条底色。
pub(in crate::ui) fn status_bar_frame(accent: bool) -> eframe::egui::Frame {
    let fill = if accent {
        crate::theme::chrome(term_core::theme::ACCENT_BLUE)
    } else {
        crate::theme::chrome(term_core::theme::CHROME_SCALE.dialog)
    };
    eframe::egui::Frame::none()
        .inner_margin(eframe::egui::Margin::symmetric(DESIGN_TOKENS.menu_pad_x, 0.0))
        .fill(fill)
}

pub(in crate::ui) fn bar_frame() -> eframe::egui::Frame {
    eframe::egui::Frame::none()
        .inner_margin(eframe::egui::Margin::symmetric(DESIGN_TOKENS.menu_pad_x, 0.0))
        .fill(crate::theme::chrome(term_core::theme::CHROME_SCALE.dialog))
}

/// 工具行专用的框：**左右各 8**。
///
/// 其余三条顶栏用 `menu_pad_x`（11）把内容从窗口边推远，那是菜单文字与标签的
/// 习惯；工具行不同——视觉基线里拖动抓手离窗格左缘只有 8 px（大图 x 0..8 是
/// 纯色内边距，抓手从 x=8 起）。用 11 会让整串窗格 chrome 右移 3 px，
/// 抓手、三颗按钮、工具栏面板左沿全部跟着漂。
pub(in crate::ui) fn tool_row_frame() -> eframe::egui::Frame {
    eframe::egui::Frame::none()
        .inner_margin(eframe::egui::Margin::symmetric(8.0, 0.0))
        .fill(crate::theme::chrome(term_core::theme::CHROME_SCALE.dialog))
}

/// 立体按钮底：`#242424` 圆角边框 + 一圈 `#616161` 内高光 + 竖向渐变
/// `#4B4B4B → #404040` + 底部一条 `#565656`。视觉基线里菜单栏最右那颗布局按钮与
/// 工具行的「新会话 / 下拉」两颗共用这一套。
///
/// **2026-11-07 逐行重采**（工具行大图，2×→逻辑点，相对按钮顶边，按钮 23 高）：
///
/// | 纵向 | 颜色 |
/// |---|---|
/// | 0..1 | `#242424` 边框（圆角 ≈1.5） |
/// | 1..2 | `#616161` 内高光 |
/// | 2..21 | `#4B4B4B` → `#404040` |
/// | 21..22 | `#565656` |
/// | 22..23 | `#242424` 边框 |
///
/// 内高光**不是只有顶上一行**：逐像素看是沿左、右、上三边各 1 点的环，到底部
/// 高光为止。前两版只画了顶上一行，配上骑在边线上的描边，整颗按钮看起来是
/// 「一块平底贴了个黑框」——视觉基线里它是凸的。
pub(in crate::ui) fn bevel(p: &eframe::egui::Painter, rect: eframe::egui::Rect) {
    use eframe::egui;
    const RADIUS: f32 = 1.5;
    p.rect_filled(
        rect,
        RADIUS,
        egui::Color32::from_rgb(0x24, 0x24, 0x24),
    );

    let inner = rect.shrink(1.0);
    let top = egui::Color32::from_rgb(0x4B, 0x4B, 0x4B);
    let bottom = egui::Color32::from_rgb(0x40, 0x40, 0x40);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(inner.left_top(), top);
    mesh.colored_vertex(inner.right_top(), top);
    mesh.colored_vertex(inner.right_bottom(), bottom);
    mesh.colored_vertex(inner.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(egui::Shape::mesh(mesh));

    let hi = egui::Color32::from_rgb(0x61, 0x61, 0x61);
    let strip = |a: egui::Pos2, b: egui::Pos2| {
        p.rect_filled(egui::Rect::from_min_max(a, b), 0.0, hi);
    };
    // 左、右两条竖高光，上边一条横高光；底部那条是另一种颜色，画在最后盖住竖条末端。
    strip(inner.left_top(), egui::pos2(inner.left() + 1.0, inner.bottom()));
    strip(
        egui::pos2(inner.right() - 1.0, inner.top()),
        inner.right_bottom(),
    );
    strip(inner.left_top(), egui::pos2(inner.right(), inner.top() + 1.0));
    p.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(inner.left(), inner.bottom() - 1.0),
            inner.right_bottom(),
        ),
        0.0,
        egui::Color32::from_rgb(0x56, 0x56, 0x56),
    );
}

/// 菜单栏专用的框：**左边距仍是 `menu_pad_x`，右边距收成 4.5**。
///
/// 视觉基线里左侧首项墨迹在 x=12、右侧布局按钮的右缘在 x=2339/2≈1169.5（窗口
/// 宽 1174）——也就是说右边缘只留 4.5 px。左边 11、右边 4.5 的不对称是原样，
/// 因为右边那件是**带底的按钮**，它自己还有视觉体积；贴太远反而显得吊着。
pub(in crate::ui) fn menu_bar_frame() -> eframe::egui::Frame {
    eframe::egui::Frame::none()
        .inner_margin(eframe::egui::Margin {
            left: DESIGN_TOKENS.menu_pad_x,
            right: 4.5,
            top: 0.0,
            bottom: 0.0,
        })
        .fill(crate::theme::chrome(term_core::theme::CHROME_SCALE.dialog))
}
