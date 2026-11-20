//! 窗格工具行：**一条**可整块拖走的工具栏。
//!
//! # 结构
//!
//! 2026-11-07 逐像素量视觉基线（`clipboard-…-b4118482.png`，1576×298 @2×）。
//! 下面全是**逻辑点**，0 点 = 本条的 `max_rect().left()`（`tool_row_frame` 的
//! 内边距已经把它推到窗格左缘 + 8）：
//!
//! ```text
//! x  0.. 5     拖动抓手（2 列 × 5 行点阵，17 高）
//! x  9.5..30.5 立体按钮：窗口 + 加号（21 × 23）
//! x 33.5..42.5 立体按钮：下拉箭头（9 × 23，与上一颗只隔 3.5）
//! x 51..65     窗口 + 叉（14 见方，平面）
//! x 77..91     窗口 + 圆圈（14 见方，平面）
//! x 98..       面板段（#333333）：(i) │ ▸ 分组 ▸ 会话
//! ```
//!
//! ## 是「一条」，不是「窗格 chrome + 一块面板」
//!
//! 第一版把 x<98 那一段当成窗格自己的 chrome（画在行底 `#1C1C1C` 上、不跟着走），
//! 只让 x>98 的 `#333333` 面板可拖。用户当场纠正：「左侧那三个也是工具栏的一部分，
//! 你做成脱离的了」——**整条一起拖**才是预期行为。
//!
//! 所以现在从抓手到面板右缘是**一个矩形**、一个 `Area`、一次拖动。两段底色不同
//! 只是视觉分段（左段读 `dialog`、右段读 `tab`），不表示两个控件：
//! 左段的 `#1C1C1C` 与窗格同色，停靠时看起来像是「按钮直接坐在窗格上」，
//! 一拖走就露出它本来就在这条上。
//!
//! # 为什么全部手画
//!
//! 视觉基线的图标是 1 px 描边的几何图形，字体里没有对应字形；用 emoji 或
//! `⚙` 之类还要赌字体覆盖。全部走绘图原语（与 `session_icons.rs` /
//! `menu_tools.rs` 同一原则）。
//!
//! # 拖动的语义
//!
//! - 抓手、三颗按钮、面板空白处**都能拖**（按钮用 `Sense::click_and_drag`：
//!   按下不动是点击，按下移动是拖动，两者不打架）。
//! - 松手时整条的顶边落在工具行这条带里 → 重新停靠（`toolbar_pos = None`）。
//!   这是要求的回程：拖出去必须能拖回来。
//! - 位置存在 `TerminalWindow.toolbar_pos`，是**投影态**（和 `find_input` 同层），
//!   不进 `ViewState`——布局位置不是领域状态。
//!
//! # 为什么放在 `Area` 里
//!
//! 停靠时整条画在工具行内部；一旦被拖走，它就要落到工具行**外面**。而
//! `TopBottomPanel` 给的 `Ui` 的裁剪矩形就是条本身、`Ui::interact` 还会把
//! 点按区与裁剪矩形求交——直接画的话，拖出去之后既看不见也点不到。
//!
//! 所以整条**始终**走 `egui::Area`（`Order::Middle`，压在面板之上、对话框之下）：
//! 停靠时面积恰好等于行带，浮起时等于浮动矩形，裁剪与命中都由屏幕矩形兜底。
//! 两态共用同一个 `Id`，拖动中途由停靠变浮动时 egui 的 drag 状态不会断。
//!
//! # 右端三件（2026-11-08 补上）
//!
//! 视觉基线面板右端还有 `>`、`∨` 与行右端那颗 `⋮`。早期版本刻意**没画**：程序里
//! 没有对应动作，画出来就是禁止的假按钮。现在三件各接了一颗真菜单
//! （Goto / 会话列表 / 窗格动作），见 `tool_bar_tail.rs`；面板段因此收窄
//! [`tool_bar_tail::TAIL_W`] 给 `⋮` 让出井位，拖动热区也避开右端按钮。

use crate::ui::*;
use term_core::session_runtime::SessionId;
use term_core::theme::CHROME_SCALE;

/// 画哪一条工具栏（ADR-0026）。
///
/// 单窗格：一条全窗工具栏，`salt = None`、`float = true`（保留整条拖走/回停）。
/// 分屏：每片 pane 各画一条，`salt = Some(pane)` 让所有 `egui` id 唯一、
/// `float = false` 固定在本格行带上——三格共用一个 `toolbar_pos` 会互相抢。
#[derive(Clone, Copy)]
pub(in crate::ui) struct BarScope {
    /// 面包屑取哪条会话的配置（分屏时是**本格**的活动会话，不是全局 `active`）。
    pub session: SessionId,
    /// 分屏时的 pane 身份；`None` = 单窗格全窗那一条（id 保持原样）。
    pub salt: Option<u64>,
    /// 是否允许整条拖走 / 浮起。分屏时 `false`。
    pub float: bool,
}

impl BarScope {
    /// 单窗格默认形态：全窗一条、可拖动。
    pub(in crate::ui) fn single(session: SessionId) -> Self {
        Self { session, salt: None, float: true }
    }

    fn id(&self, name: &'static str) -> egui::Id {
        match self.salt {
            Some(s) => egui::Id::new((name, s)),
            None => egui::Id::new(name),
        }
    }
}

use super::tool_bar_icons::{
    chrome_icon, icon_in, paint_caret, paint_grip, paint_panel, paint_strip,
    paint_window_cross, paint_window_plus, paint_window_ring,
};
use super::tool_bar_tail::{self, TailWork, TAIL_W};

/// 行高。视觉基线 y 64..115（2×）→ 26 逻辑点。
pub(in crate::ui) const ROW_H: f32 = 26.0;

// ── 各元素的水平位置：相对**整条**左缘（已减去 8 点内边距） ──
/// 拖动抓手。
const GRIP_W: f32 = 5.0;
const GRIP_H: f32 = 17.0;
/// 立体按钮组里「窗口 + 加号」那颗。
const NEW_DX: f32 = 9.5;
const NEW_W: f32 = 21.0;
/// 紧邻的下拉箭头（同一视觉组，中间只隔 3.5）。
const CARET_DX: f32 = 33.5;
const CARET_W: f32 = 9.0;
/// 窗口 + 叉（断开）。
const CLOSE_DX: f32 = 51.0;
/// 窗口 + 圆圈（连接）。
const OPEN_DX: f32 = 77.0;
/// 平面图标的点按区宽度。视觉基线里窗口轮廓 + 右上角符号的墨迹 13.5 点宽。
const ICON_W: f32 = 13.5;
/// 面板段的起点：这里往右换成 `#333333`。
///
/// `pub(super)`：底色与面板内容在 `tool_bar_icons` 里画，要用到这条分界。
pub(super) const PANEL_DX: f32 = 98.0;
/// 浮动时面板段的固定宽度：停靠宽度随窗口变，浮起来必须有一个确定值。
const PANEL_FLOAT_W: f32 = 520.0;
/// 浮动时整条的宽度 = 抓手与按钮段（0..98）+ 固定面板段。
const FLOAT_W: f32 = PANEL_DX + PANEL_FLOAT_W;
/// 立体底上下各收 1.5：视觉基线里它高 23、行高 26。
const BEVEL_INSET_Y: f32 = 1.5;

/// 一条工具栏的拖动状态：本帧位移 + 本帧是否松手。
///
/// 两者要分开收：位移在拖动中每帧都变，松手只在最后一帧发生一次。
#[derive(Default)]
struct Drag {
    delta: egui::Vec2,
    stopped: bool,
}

impl Drag {
    fn track(&mut self, resp: &egui::Response) {
        if resp.dragged() {
            self.delta = resp.drag_delta();
        }
        if resp.drag_stopped() {
            self.stopped = true;
        }
    }
}

/// 画整条工具栏。发射点在 `ui/mod.rs`（标签条之后的第二条 `TopBottomPanel`），
/// 经 `pane_tools::draw_pane_tools` 进来。
pub(in crate::ui) fn draw(win: &mut TerminalWindow, ui: &mut egui::Ui, scope: BarScope) {
    let dock = ui.max_rect();
    let font = egui::TextStyle::Button.resolve(ui.style());
    // 面包屑按 `scope.session` 取：分屏时是本格活动会话，单窗格时与旧
    // `tabs.get(active_slot())` 完全等价（调用方已把 active 翻成身份）。
    let crumbs = win
        .tabs
        .iter()
        .find(|t| t.identity == scope.session)
        .map(|tab| super::pane_tools::breadcrumb(&tab.config))
        .unwrap_or_default();
    let entries = menu_entries(win);
    let sessions: Vec<String> = win.sessions.iter().map(|s| s.name.clone()).collect();
    let lang = win.lang();
    let (new_on, close_on, open_on) = (
        !hidden(win, ToolAction::NewSession),
        !hidden(win, ToolAction::Disconnect),
        !hidden(win, ToolAction::Connect),
    );
    // 分屏时不允许浮起：`toolbar_pos` 是窗口级单份，三格共用会互相抢位置。
    let pos = if scope.float { win.toolbar_pos } else { None };
    let rect = match pos {
        None => dock,
        Some(p) => egui::Rect::from_min_size(p, egui::vec2(FLOAT_W, ROW_H)),
    };

    let mut drag = Drag::default();
    let mut picked: Option<ToolAction> = None;
    let mut tail_work = TailWork::default();
    // 面板段被**点**了一下（而非拖动）。记下它本帧的矩形，画完再开地址栏
    // ——地址栏要拿它当锚点，而这个矩形只有 `Area` 内部才算得出来。
    let mut panel_clicked: Option<egui::Rect> = None;

    egui::Area::new(scope.id("pane_toolbar"))
        .order(egui::Order::Middle)
        .fixed_pos(rect.min)
        // `fade_in` 必须关掉：淡入按 `i.time` 算，而夹具把时钟钉在 0，于是整条
        // 停在第一帧——`#333333` 的底被合成成 `#212121`（实测，还带 1 px 抖动），
        // 看起来像「没用设计令牌」。同一坑 `harness` / 浮动面板都踩过。
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            let cy = rect.center().y;
            let left = rect.left();
            paint_strip(ui.painter(), rect);

            // 抓手：点按区比点阵宽一圈，否则 5 点的点阵根本抓不住。
            let grip = egui::Rect::from_center_size(
                egui::pos2(left + GRIP_W / 2.0, cy),
                egui::vec2(GRIP_W + 8.0, ROW_H),
            );
            // 分屏时抓手不接拖动：三格共用 `toolbar_pos`，拖起来会互相躲。
            let grip_sense = if scope.float {
                egui::Sense::drag()
            } else {
                egui::Sense::hover()
            };
            let resp = ui.interact(grip, scope.id("tool_grip"), grip_sense);
            paint_grip(
                ui.painter(),
                egui::Rect::from_center_size(grip.center(), egui::vec2(GRIP_W, GRIP_H)),
            );
            drag.track(&resp);

            // 立体组：新会话（+ 下拉）。两颗各有各的底，中间空 3.5。
            if new_on {
                let r = slot(left, cy, NEW_DX, NEW_W);
                let resp = hit(ui, scope.id("tool_new"), r);
                paint_button(ui.painter(), &resp, r, Chrome::Bevel);
                paint_window_plus(ui.painter(), icon_in(r), chrome_icon());
                if resp.clicked() {
                    picked = Some(ToolAction::NewSession);
                }
                drag.track(&resp);
            }
            caret_menu(ui, slot(left, cy, CARET_DX, CARET_W), &entries, &mut picked);

            // 平面两颗：断开 / 连接。
            if close_on {
                let r = slot(left, cy, CLOSE_DX, ICON_W);
                let resp = hit(ui, scope.id("tool_close"), r);
                paint_button(ui.painter(), &resp, r, Chrome::Flat);
                paint_window_cross(ui.painter(), icon_in(r), chrome_icon());
                if resp.clicked() {
                    picked = Some(ToolAction::Disconnect);
                }
                drag.track(&resp);
            }
            if open_on {
                let r = slot(left, cy, OPEN_DX, ICON_W);
                let resp = hit(ui, scope.id("tool_open"), r);
                paint_button(ui.painter(), &resp, r, Chrome::Flat);
                paint_window_ring(ui.painter(), icon_in(r), chrome_icon());
                if resp.clicked() {
                    picked = Some(ToolAction::Connect);
                }
                drag.track(&resp);
            }

            // 面板段：停靠时铺满剩余宽度，浮动时跟整条一起走（宽 520）。
            // 右端收窄 TAIL_W 给 `⋮` 留井位（视觉基线里地址栏框在它之前结束）。
            let panel = egui::Rect::from_min_max(
                egui::pos2(left + PANEL_DX, rect.top()),
                egui::pos2(rect.right() - TAIL_W, rect.bottom()),
            );
            let tail = egui::Rect::from_min_max(
                egui::pos2(panel.right(), rect.top()),
                rect.max,
            );
            // 拖动热区避开右端三颗菜单：叠上去会跟 `menu_custom_button` 抢同一次按下。
            let drag_area = egui::Rect::from_min_max(
                panel.min,
                egui::pos2(panel.right() - 46.0, panel.bottom()),
            );
            // `click_and_drag`（而不是 `drag`）——按下不动是「开地址栏」，
            // 按下移动是「拖动整条」。两个 sense 在 egui 里本来就互斥判定，
            // `clicked()` 只在「按下又松开且没移动」时为真，所以整条工具栏
            // 照旧拖得动（下面那句 `drag.track(&resp)` 一行没改）。
            let resp = ui.interact(
                drag_area,
                scope.id("tool_panel"),
                egui::Sense::click_and_drag(),
            );
            paint_panel(ui.painter(), panel, &crumbs, &font);
            tool_bar_tail::draw(
                ui,
                panel,
                tail,
                &sessions,
                &entries,
                lang,
                &mut tail_work,
            );
            drag.track(&resp);
            if resp.clicked() {
                panel_clicked = Some(panel);
            }

            // 浮起来才描边：停靠时整条与窗格同底，加框会凭空多出一条横线。
            if pos.is_some() {
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.0, crate::theme::chrome(CHROME_SCALE.border)),
                );
            }
        });

    if let Some(action) = picked {
        win.handle_tool_action(action);
    }
    // 右端三颗菜单的动作统一在 Area 之外落地：菜单闭包只收集意图，不改窗口状态。
    if let Some(i) = tail_work.connect {
        win.selected_session = i;
        win.connect_selected_session();
    }
    if let Some(action) = tail_work.tool {
        win.handle_tool_action(action);
    }
    if let Some(id) = tail_work.dispatch {
        win.dispatch(id);
    }
    // 面板段点了一下 → 从它的左下角展开地址栏，输入框为空
    // （打开即览全部候选，这是地址栏该有的行为）。
    if let Some(panel) = panel_clicked {
        win.open_address_bar("", Some(panel));
    }
    // delta 是**本帧**位移，所以累加到已存的位置上；停靠态第一次拖动时以停靠位
    // 为起点，整条才不会被瞬移。
    // 分屏时 `scope.float == false`：位置是每格自己的行带，不写窗口级 `toolbar_pos`。
    if scope.float {
        if drag.delta != egui::Vec2::ZERO {
            let base = pos.unwrap_or(dock.min);
            win.toolbar_pos = Some(clamp_to(ui.ctx().screen_rect(), base + drag.delta));
        }
        // 回程：松手时顶边落在工具行这条带里就重新停靠。
        if drag.stopped {
            if let Some(p) = win.toolbar_pos {
                if should_redock(p.y, dock.top()) {
                    win.toolbar_pos = None;
                }
            }
        }
    }
}

/// 松手位置算不算「放回工具行」。
///
/// 判据是**整条顶边与停靠位的距离**，不是指针位置：指针可以落在条里而整条还
/// 在下面。留半个行高的容差，是为了让「差一点没放进去」也能回位。
fn should_redock(panel_top: f32, dock_top: f32) -> bool {
    (panel_top - dock_top).abs() <= ROW_H / 2.0
}

/// 浮动位置不许飘出窗口：至少留 `KEEP` 点露在外面，否则拖丢了就再也抓不回来。
fn clamp_to(screen: egui::Rect, p: egui::Pos2) -> egui::Pos2 {
    const KEEP: f32 = 60.0;
    egui::pos2(
        p.x.clamp(screen.right() - FLOAT_W + KEEP, screen.right() - KEEP),
        p.y.clamp(screen.top(), screen.bottom() - ROW_H),
    )
}

// ── 小工具 ────────────────────────────────────────────────────────────────

fn slot(left: f32, cy: f32, dx: f32, w: f32) -> egui::Rect {
    egui::Rect::from_center_size(egui::pos2(left + dx + w / 2.0, cy), egui::vec2(w, ROW_H))
}

/// 工作台「工具栏定制」页写的隐藏位。索引对的是 `TerminalWindow.tools`，
/// 所以这里必须按**动作**回查，不能按下标——工具栏上这三颗的位置是钉死的。
fn hidden(win: &TerminalWindow, action: ToolAction) -> bool {
    win.tools.iter().enumerate().any(|(i, t)| {
        t.action == action && win.view.center.hidden_tools.get(i).copied().unwrap_or(false)
    })
}

/// 整条上的按钮用 `click_and_drag`：按下不动是点击，按下移动是拖动整条。
///
/// 只用 `Sense::click()` 的话，在按钮上起手就拖不动整条；只用 `Sense::drag()`
/// 则按钮永远点不亮。这两个 sense 在 egui 里本来就是互斥判定的，合起来用不会
/// 互相抢——`clicked()` 只在「按下又松开且没移动」时为真。
fn hit(ui: &mut egui::Ui, id: egui::Id, rect: egui::Rect) -> egui::Response {
    ui.interact(rect, id, egui::Sense::click_and_drag())
}

enum Chrome {
    /// 只在悬停时铺一层底色（视觉基线里两颗平面图标没有底）。
    Flat,
    /// 竖向渐变底，见 `shell::bevel`。
    Bevel,
}

/// 一颗按钮的底与悬停反馈。点击判定在调用处——那里才知道它对应哪个动作。
fn paint_button(p: &egui::Painter, resp: &egui::Response, rect: egui::Rect, style: Chrome) {
    match style {
        Chrome::Bevel => super::bevel(p, rect.shrink2(egui::vec2(0.0, BEVEL_INSET_Y))),
        Chrome::Flat => {
            if resp.hovered() {
                p.rect_filled(
                    rect.shrink2(egui::vec2(0.0, 3.0)),
                    2.0,
                    crate::theme::chrome(CHROME_SCALE.hover),
                );
            }
        }
    }
}

/// 下拉菜单的条目：`self.tools` 里**已接线**且未被隐藏的那些。
///
/// 为什么不写死一张表：工作台的「工具栏定制」页改的是 `hidden_tools`，写死一张
/// 表就等于让那个开关对着空气生效。同理，`Refresh` / `SFTP` / `Serial`
/// 三个动作在 `handle_tool_action` 里是 `_ => {}`，列出来就是点了没反应的假条目。
fn menu_entries(win: &TerminalWindow) -> Vec<(ToolAction, &'static str)> {
    win.tools
        .iter()
        .enumerate()
        .filter(|(i, t)| {
            !win.view.center.hidden_tools.get(*i).copied().unwrap_or(false) && wired(t.action)
        })
        .map(|(_, t)| (t.action, t.tooltip))
        .collect()
}

/// 下拉按钮。**它是菜单入口，不参与拖动**：`menu_custom_button` 内部自己接管
/// 了指针，外面再叠一个 drag 热区会跟它的开合抢同一次按下。
fn caret_menu(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    entries: &[(ToolAction, &'static str)],
    picked: &mut Option<ToolAction>,
) {
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        egui::menu::menu_custom_button(
            ui,
            egui::Button::new("").frame(false).min_size(rect.size()),
            |ui| {
                for (action, label) in entries {
                    if ui.button(*label).clicked() {
                        *picked = Some(*action);
                        ui.close_menu();
                    }
                }
            },
        );
    });

    let p = ui.painter();
    // 立体底 + 箭头。`menu_custom_button` 自己那颗是空按钮，底色由这里给。
    super::bevel(p, rect.shrink2(egui::vec2(0.0, BEVEL_INSET_Y)));
    paint_caret(
        p,
        egui::Rect::from_center_size(rect.center(), egui::vec2(4.5, 3.0)),
        chrome_icon(),
    );
}

/// `handle_tool_action` 里真的有分支的动作。
fn wired(action: ToolAction) -> bool {
    matches!(
        action,
        ToolAction::NewSession
            | ToolAction::Connect
            | ToolAction::Disconnect
            | ToolAction::Settings
            | ToolAction::Terminal
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panel_dropped_back_in_the_row_redocks() {
        // 顶边与停靠位差不到半个行高 → 回位。
        assert!(should_redock(62.0, 62.0));
        assert!(should_redock(62.0 + ROW_H / 2.0 - 0.1, 62.0));
        assert!(should_redock(62.0 - ROW_H / 2.0 + 0.1, 62.0));
    }

    #[test]
    fn a_panel_dragged_away_stays_floating() {
        // 拖到终端里就不该自己弹回去——否则「拖出去」这件事根本没发生。
        assert!(!should_redock(62.0 + ROW_H, 62.0));
        assert!(!should_redock(400.0, 62.0));
    }

    #[test]
    fn the_floating_panel_keeps_a_handle_on_screen() {
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1280.0, 800.0));
        // 往右拖出屏幕：左边至少留 60 点可抓，右缘可以溢出。
        let far = clamp_to(screen, egui::pos2(5000.0, 100.0));
        assert_eq!(far.x, 1280.0 - 60.0);
        assert!(far.x + FLOAT_W > screen.right());
        // 往上/下超界：夹在屏幕内，整条始终完整可见。
        assert_eq!(clamp_to(screen, egui::pos2(100.0, -50.0)).y, 0.0);
        assert_eq!(
            clamp_to(screen, egui::pos2(100.0, 5000.0)).y,
            800.0 - ROW_H
        );
    }

    #[test]
    fn the_whole_row_travels_as_one_piece() {
        // 抓手与面板段在同一个矩形里，宽度差就是按钮段——这条守着「别再把它拆开」。
        // 两个都是布局常量之间的不变式，用 `const {}` 块让它在编译期就炸，不必等
        // 测试跑到这里；clippy 的 `assertions_on_constants` 也是这么要求的。
        const { assert!(FLOAT_W == PANEL_DX + PANEL_FLOAT_W) };
        const { assert!(FLOAT_W > PANEL_DX) };
        // 三颗按钮到面板左缘为止，谁也没越过分界线。
        const { assert!(OPEN_DX + ICON_W <= PANEL_DX) };
    }
}
