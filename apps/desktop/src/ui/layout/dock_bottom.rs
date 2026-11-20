//! 底带：**一条带 + 一条标签条**（2026-11-07）。
//!
//! # 它和左右两条带不是同一种东西
//!
//! 左右带里每块面板各占一条 `SidePanel`，竖着叠；底带在视觉基线里是**一条带**，
//! 上面一排标签（`Sender` / `Shell`），下面只画**当前那颗**的内容。所以底带
//! 不能沿用「每个成员一个 `TopBottomPanel`」的老画法——那样两个面板会上下叠在
//! 一起，标签条也无处安放。
//!
//! ```text
//! ┌──────────────────────────────────────────────┬─┬─┬─┐
//! │ ▌ Sender │ ▌ Shell                            │^│⚙│×│  ← 25 高
//! ├──────────────────────────────────────────────┴─┴─┴─┤
//! │  当前标签的面板内容                                  │
//! └────────────────────────────────────────────────────┘
//! ```
//!
//! # 逐像素（视觉基线 1280×223 @1x）
//!
//! | 元素 | 视觉基线 |
//! |---|---|
//! | 标签条高 | 25 |
//! | 活动标签底 | `#333333`（整条高，不圆角） |
//! | 标签左内边距 / 右内边距 | 11 / 27 |
//! | 色块 | 5×5，Sender `#996EE2` / Shell `#FF0099` |
//! | 色块 → 文字 | 8 |
//! | 右端三颗 | `^` `⚙` `×`，点按区宽 18（用户反馈 22 过大收紧），右缘留 4 |
//!
//! # 三颗按钮都是真的（R5）
//!
//! - `^` 向上展开覆盖 —— 底带以浮层盖住上方所有面板、顶到菜单栏，再点收回
//!   恢复原位（图标翻成 `v`）。**它是覆盖不是布局切分**：展开态不声明
//!   `TopBottomPanel`，否则会抢走随后所有面板的高度、把它们挤在顶部叠成一堆。
//!   展开是投影态，不进 `ViewState`（与 `toolbar_pos` 同理：几何不是领域语义）。
//!   实现见 [`TerminalWindow::draw_bottom_dock_overlay`]。
//! - `⚙` 菜单 —— `Split` / `Float` / 七个面板的勾选表。勾上 = 拉到底带，
//!   取消 = 送回它自己的默认带。菜单里没有侧栏：视觉基线那张表也没有它，
//!   侧栏是整高的会话树，塞进底带没有意义。
//! - `×` 关掉这条底带 —— 对带内每个**看得见**的成员各发一次 `TogglePanel`，
//!   与菜单里那个开关是同一条路径，不是第二套开合。
//!
//! # 标签不带拖拽
//!
//! 面板进出底带走菜单（勾选 / `Float`），标签本身只负责「切内容」。不做「拖标签
//! 重排」是刻意的：它和「拖标签扔掉 / 拖出去浮起来」是同一块手势区，两件事一起
//! 上没有不打架的判据，而菜单已经给了确定的出口（R5 要的是回程，不是手势）。
//!
//! # 为什么 `Split` 是「分组」而不是「再开一条带」
//!
//! 底带只有一条（发射点固定），`Split` 在**带内部**按活动标签一分二：左组放
//! 活动标签及它前面的，右组放剩下的。两组各有一条标签条与自己的内容；Dock 级
//! 的三颗按钮只画在最右那组——它们是整条带的控制，不是某一组的。
//! 再点一次 `Split` 合并回一组（`bottom_split = None`）。

use crate::ui::*;
use eframe::egui;
use term_app::action::Action;
use term_app::dock::{DockZone, LayoutAction, PanelId};
use term_app::shell::ShellAction;
use term_core::theme::CHROME_SCALE;

/// 标签条高。视觉基线 y 0..24。
pub(in crate::ui) const TAB_H: f32 = 25.0;

/// 标签左内边距（视觉基线：标签左缘 → 色块 11）。
pub(super) const TAB_PAD_L: f32 = 11.0;
/// 色块边长（视觉基线 5×5）。
pub(super) const DOT_W: f32 = 5.0;
/// 色块 → 文字。
pub(super) const DOT_GAP: f32 = 8.0;
/// 标签右内边距（视觉基线两颗标签都是 27）。
const TAB_PAD_R: f32 = 27.0;
/// 一颗标签的最小宽度：太窄的话色块与文字会挤在一起。
const TAB_MIN_W: f32 = 64.0;
/// 右端三颗按钮的点按区宽。视觉基线量得 22，但实测点按区偏胖、按钮显大，
/// 收到 18：三颗更紧凑，hover / 误触的命中区也更准（用户 2026-11-15 反馈）。
pub(super) const BTN_W: f32 = 18.0;
// 2026-11-17：原来的 `BTN_ICON`（10.0）已退役——齿轮与 `×` 的尺寸收归
// `shell::glyph`（⚙ 外径 10.0 / × 边长 6.5），不再由这条带自己算一份。
/// 右端按钮与窗沿的距离（视觉基线 4）。
pub(super) const EDGE_PAD: f32 = 4.0;
/// 底带默认高度。与 P4 的 `BOTTOM_DEFAULT_HEIGHT` 一致。
pub(in crate::ui) const DEFAULT_H: f32 = 220.0;
/// 内容区左右内边距。
const BODY_PAD: f32 = 8.0;

/// 面板标签上的色块。
///
/// **量出来的五块**：Sender `#996EE2` / Shell `#FF0099`（底带）；Explorer
/// `#FF787C` / Outline `#8FDBF9` / Session `#0093FF`（2026-11-17 取自浮窗视觉基线
/// `clipboard-2026-11-16T13-36-02-041Z-fc3bb6c9.png`，标签条 y=59 逐像素采样）。
///
/// 原表里 Explorer 是蓝、Outline 是绿、Session 是红——全部猜错，图上一目了然：
/// Explorer 是**珊瑚红**、Outline 是**浅青**、Session 是**蓝**。注意最后一条：
/// 参考实现那个「Session」面板就是本项目的 `Sidebar`（标题同为 "Session"，正文同为
/// 「Filter + 分组会话树」），所以 `#0093FF` 本就该挂在 `Sidebar` 上；`PanelId::Session`
/// 是本项目自有的精简列表，视觉基线里没有，仍是推定值。
///
/// 其余未出现在视觉基线里的面板按同一饱和度推定——换掉只需改这一张表。
pub(super) fn dot_color(id: PanelId) -> egui::Color32 {
    match id {
        PanelId::Sender => egui::Color32::from_rgb(0x99, 0x6E, 0xE2),
        PanelId::Shell => egui::Color32::from_rgb(0xFF, 0x00, 0x99),
        PanelId::Explorer => egui::Color32::from_rgb(0xFF, 0x78, 0x7C),
        PanelId::Filer => egui::Color32::from_rgb(0xF5, 0xA6, 0x23),
        PanelId::Outline => egui::Color32::from_rgb(0x8F, 0xDB, 0xF9),
        PanelId::Session => egui::Color32::from_rgb(0xE0, 0x6C, 0x75),
        PanelId::Transfer => egui::Color32::from_rgb(0x00, 0xBC, 0xD4),
        PanelId::Sysmon => egui::Color32::from_rgb(0x8B, 0xC3, 0x4A),
        PanelId::Script => egui::Color32::from_rgb(0x9C, 0x27, 0xB0),
        PanelId::Sidebar => egui::Color32::from_rgb(0x00, 0x93, 0xFF),
    }
}

/// 带内**看得见**的成员，按位次。看不见的成员不占标签——标签点开却是一片
/// 空白，比「标签不在」更糟。
fn visible_members(win: &TerminalWindow) -> Vec<PanelId> {
    win.dock
        .zone(DockZone::Bottom)
        .into_iter()
        .filter(|id| win.panel_visible(*id))
        .collect()
}

/// 按 `split_at` 把成员切成 1~2 组。
///
/// 切点落在两端时退化成一组：只有一颗标签时按 `Split`，不该把这条带切成
/// 「有内容 + 空白」两半。
fn split_groups(members: &[PanelId], split_at: Option<usize>) -> Vec<Vec<PanelId>> {
    match split_at {
        Some(k) if k > 0 && k < members.len() => {
            vec![members[..k].to_vec(), members[k..].to_vec()]
        }
        _ => vec![members.to_vec()],
    }
}

/// 一组里该显示哪一颗：优先全局活动标签，否则这一组的第一颗。
fn group_active(group: &[PanelId], active: Option<PanelId>) -> PanelId {
    group
        .iter()
        .copied()
        .find(|p| Some(*p) == active)
        .unwrap_or(group[0])
}

/// 菜单 / 按钮产生的意图。攒到面板画完再落地：绘制中途改 `DockLayout` 会让
/// 本帧的成员表与矩形对不上。
///
/// `Copy`：齿轮菜单把这些意图先装进一张行表（要先按标签算宽度，再逐行画），
/// 点击时从表里取值——不 Copy 就得为「点完还要用」再克隆一遍。
///
/// 2026-11-13：三个带的齿轮菜单**共用这一张表**，所以 `Pull` / `Uncheck` 带
/// 目标带——同一份菜单在左带、右带、底带里说的是「进这条带 / 离开这条带」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum Cmd {
    /// 切分当前带。**只有底带**用得上（左右带是竖直堆叠，没有「切一刀」的语义）。
    Split,
    /// 浮动**整条带**（参考实现浮动窗内是横向 tab，不是单块面板）。
    /// 单块浮起仍由拖拽落点 `LayoutAction::Float(panel)` 负责。
    FloatBand(DockZone),
    /// 浮窗里 `Float` 的语义：把当前 tab 单独拆成一块浮窗（参考实现菜单文案仍是
    /// "Float"，不能为了给「整带收回」造一个视觉基线里没有的 "Dock" 行）。
    Float(PanelId),
    /// 勾上：拉进本条带（并确保可见）。
    Pull(PanelId, DockZone),
    /// 取消：离开本条带——送回它的默认带；它本来就在默认带就隐藏。
    Uncheck(PanelId, DockZone),
    /// 面板的显示开关（`×` 只关当前标签）。
    Toggle(PanelId),
    /// 向上展开占满 / 还原（`^`）。
    Maximize,
}

impl TerminalWindow {
    /// 一个标签的宽度：内边距 + 色块 + 间隙 + 文字 + 内边距。
    pub(super) fn tab_width(&self, ui: &egui::Ui, id: PanelId) -> f32 {
        let font = egui::TextStyle::Button.resolve(ui.style());
        let w = ui
            .painter()
            .layout_no_wrap(self.tr(id.title()).to_string(), font, egui::Color32::WHITE)
            .size()
            .x;
        (TAB_PAD_L + DOT_W + DOT_GAP + w + TAB_PAD_R).max(TAB_MIN_W)
    }

    /// 画底带（**收回态**）。发射点在 `dock_paint::draw_dock_zone`，与左右两带
    /// 同一个入口。展开态不在这里画——见 [`Self::draw_bottom_dock_overlay`]。
    pub(in crate::ui) fn draw_bottom_dock(&mut self, ctx: &egui::Context) {
        // 展开态绝不声明 `TopBottomPanel`：一旦声明就会参与布局切分，把随后
        // 声明的左右带 / 标签条 / 工具行 / Sysmon / 中央区挤到 ≈0 高度里叠成一堆。
        if self.bottom_maximized {
            return;
        }
        let ids = visible_members(self);
        let groups = split_groups(&ids, self.bottom_split);
        let mut cmds: Vec<Cmd> = Vec::new();
        // K3-a：空带不再整条消失（整带浮起时原停靠位也是这个形态）。
        let empty = ids.is_empty();
        let anchor = PanelId::ALL
            .into_iter()
            .find(|p| p.default_zone() == DockZone::Bottom)
            .unwrap_or(PanelId::Sender);

        let shown = egui::TopBottomPanel::bottom("dock_bottom")
            .frame(super::dock_paint::panel_frame())
            // 视觉基线里标签条与内容同色相接，没有分隔线（同 `tabs_bar`）。
            .show_separator_line(false)
            .default_height(if empty { TAB_H + 4.0 } else { self.bottom_height })
            .min_height(if empty { TAB_H } else { TAB_H + 40.0 })
            .resizable(!empty)
            .show(ctx, |ui| {
                // 面板高度由 `default_height` 定，但 `TopBottomPanel` 会把「内容实际
                // 高度」记下来给下一帧用——不把内容撑满，第二帧就缩成标签条那么高
                // （实测：220 → 65，只剩一条）。所以先占满，再往里摆。
                ui.set_min_height(ui.max_rect().height());
                let full = ui.max_rect();
                if empty {
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .id_salt("dock_bottom_empty")
                            .max_rect(full)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    child.set_clip_rect(full);
                    self.draw_compact_chrome(&mut child, anchor, DockZone::Bottom, &[], &mut cmds);
                    ui.advance_cursor_after_rect(full);
                } else {
                    self.draw_bottom_band(ui, full, &ids, &groups, false, &mut cmds);
                }
            });

        // 把这一帧的真实高度记回「回位值」。展开态不声明面板、不写这里，
        // 所以展开期间 `bottom_height` 与 egui 里 `dock_bottom` 的高度记忆都
        // 原封不动——收回时 `default_height` 精确恢复原位（不再需要独立 id）。
        // 空壳的高度不记：否则面板勾回来时只剩一条标签条。
        let h = shown.response.rect.height();
        if !empty && (h - self.bottom_height).abs() > 0.5 {
            self.bottom_height = h;
        }
        for cmd in cmds {
            self.apply_dock_cmd(cmd, DockZone::Bottom, &ids);
        }
    }

    /// 展开态：底带**覆盖层**。
    ///
    /// 参考实现 `^` 是「向上展开覆盖上面所有面板、顶到菜单栏」，不是把面板重排挤扁。
    /// 因此这里**不声明** `TopBottomPanel`——它是布局切分，会抢走随后所有面板的
    /// 高度（2026-11-15 实测：左右带 / 标签条 / 工具行 / Sysmon / 中央区被压到
    /// ≈0 高，全部叠在窗口顶部）。改为在所有面板画完之后，用一层
    /// `Order::Middle` 浮层把「菜单栏下沿 → 状态栏上沿」整块盖住：下面的面板
    /// 保持原位、只是被盖住，而 panel 都在 `Order::Background`，必定被压住。
    ///
    /// `rect` 由 `draw()` 在声明完菜单栏 / 状态栏 / 快捷栏之后取
    /// `ctx.available_rect()` 得到，正是「上沿到菜单栏」的那一块。
    pub(in crate::ui) fn draw_bottom_dock_overlay(&mut self, ctx: &egui::Context, rect: egui::Rect) {
        if !self.bottom_maximized {
            return;
        }
        let ids = visible_members(self);
        if ids.is_empty() {
            return;
        }
        let groups = split_groups(&ids, self.bottom_split);
        let mut cmds: Vec<Cmd> = Vec::new();

        // 与 `egui::Area::content_ui` 同一构造方式，但单帧确定：`max_rect` 与
        // 裁剪矩形都精确等于 `rect`，不吃 `Area` 的 sizing pass / 淡入状态
        // （夹具把时钟钉在 0，带淡入的浮层会停在半透明第一帧）。
        let layer_id = egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::new("dock_bottom_overlay"),
        );
        let mut ui = egui::Ui::new(
            ctx.clone(),
            layer_id,
            layer_id.id,
            egui::UiBuilder::new().max_rect(rect),
        );
        ui.set_clip_rect(rect);
        // 不透明底：把下面的面板彻底盖住，而不是让它们透上来。
        ui.painter().rect_filled(
            rect,
            0.0,
            crate::theme::chrome(CHROME_SCALE.dialog),
        );
        self.draw_bottom_band(&mut ui, rect, &ids, &groups, true, &mut cmds);

        for cmd in cmds {
            self.apply_dock_cmd(cmd, DockZone::Bottom, &ids);
        }
    }

    /// 带内结构（标签条 + 正文 + 外框）：收回态与展开态共用同一份绘制，
    /// `maximized` 只决定右端那颗画 `^` 还是 `v`。
    #[allow(clippy::too_many_arguments)]
    fn draw_bottom_band(
        &mut self,
        ui: &mut egui::Ui,
        full: egui::Rect,
        ids: &[PanelId],
        groups: &[Vec<PanelId>],
        maximized: bool,
        cmds: &mut Vec<Cmd>,
    ) {
        let n = groups.len();
        for (i, group) in groups.iter().enumerate() {
            let x0 = full.left() + full.width() * i as f32 / n as f32;
            let x1 = full.left() + full.width() * (i + 1) as f32 / n as f32;
            let col = egui::Rect::from_min_max(
                egui::pos2(x0, full.top()),
                egui::pos2(x1, full.bottom()),
            );
            let last = i + 1 == n;
            let active = group_active(group, self.band_tab(DockZone::Bottom));
            let picked = self.tab_strip(
                ui, col, group, ids, active, last, maximized, cmds,
                DockZone::Bottom, false, true, false,
            );
            if let Some(id) = picked {
                self.set_band_tab(DockZone::Bottom, id);
            }
            // 正文始终画：展开态也要显示内容（参考实现展开是看内容，不是收起）。
            let body =
                egui::Rect::from_min_max(egui::pos2(col.left(), col.top() + TAB_H), col.max);
            self.draw_bottom_body(ui, body, active);
            // 底带每个分组也各画一圈 1px 外框（与左右带面板同一函数，统一）。
            super::dock_paint::paint_panel_border(ui.painter(), col);
            self.dock_rects.push((active, DockZone::Bottom, col));
        }
    }

    /// 一组的内容区。
    fn draw_bottom_body(&mut self, ui: &mut egui::Ui, body: egui::Rect, id: PanelId) {
        let inner = body.shrink2(egui::vec2(BODY_PAD, 4.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        if id.scrolls_in_dock() {
            // 标题由标签条代了（再画一行 heading 就是两行标题）。
            // 分组时两组共用同一个自动 id 会撞车（egui 会画一条红字「First use
            // of ScrollArea ID …」）。用面板名当 salt，一块面板一份滚动状态。
            //
            // `PanelId::Shell` **排除**：那块面板装的是终端（与主区同一份渲染），
            // 滚轮属于终端自己的回看（`viewport.scroll_up/down`），而且
            // `ScrollArea` 的内容方向是无穷高——`available_rect_before_wrap`
            // 会变成 `f32::INFINITY`，PTY 的行数就算不出来了。判据在
            // `PanelId::scrolls_in_dock`。
            egui::ScrollArea::vertical()
                .id_salt(id.name())
                .show(&mut child, |ui| self.draw_panel_body(id, ui));
        } else {
            self.draw_panel_body(id, &mut child);
        }
    }

    /// 齿轮菜单命令的落地。**唯一**改底带 / 带成员的地方。
    ///
    /// `zone` 是「这条命令是从哪条带的齿轮菜单发出来的」：`Split` 只对底带有
    /// 意义（左右带是竖直堆叠），`Pull` / `Uncheck` 已经自带目标带，这里只用
    /// `zone` 判断要不要顺带清掉底带的活动标签。
    pub(super) fn apply_dock_cmd(&mut self, cmd: Cmd, zone: DockZone, ids: &[PanelId]) {
        match cmd {
            Cmd::Split => {
                // 只有底带能切：左右带里的面板是上下均分的，没有「切一刀」。
                if zone != DockZone::Bottom {
                    return;
                }
                let ids: Vec<PanelId> = ids.to_vec();
                self.bottom_split = match self.bottom_split {
                    Some(_) => None,
                    None => {
                        let active = group_active(&ids, self.band_tab(DockZone::Bottom));
                        let at = ids.iter().position(|p| *p == active).unwrap_or(0) + 1;
                        (at < ids.len()).then_some(at)
                    }
                };
            }
            Cmd::FloatBand(band) => {
                self.dispatch_action(Action::Layout(LayoutAction::FloatBand(band)));
            }
            Cmd::Float(panel) => {
                self.dispatch_action(Action::Layout(LayoutAction::Float(panel)));
            }
            Cmd::Pull(id, zone) => {
                // 勾上 = 拉进本条带；顺带把它打开（勾了却看不见等于没勾）。
                if !self.panel_visible(id) {
                    self.dispatch_action(Action::Shell(ShellAction::TogglePanel(id)));
                }
                self.dispatch_action(Action::Layout(LayoutAction::Dock {
                    panel: id,
                    zone,
                    index: usize::MAX,
                }));
                self.forget_band_tab(id);
            }
            Cmd::Uncheck(id, zone) => {
                // 它在默认带上却出现在这条带里 → 取消就是「回到默认带」；
                // 它本来就在默认带里 → 取消只能是「别显示了」，否则这个勾
                // 永远勾着不动，是个死开关。
                if id.default_zone() == zone {
                    self.dispatch_action(Action::Shell(ShellAction::TogglePanel(id)));
                } else {
                    self.dispatch_action(Action::Layout(LayoutAction::Dock {
                        panel: id,
                        zone: id.default_zone(),
                        index: usize::MAX,
                    }));
                }
                self.forget_band_tab(id);
            }
            Cmd::Toggle(id) => {
                self.dispatch_action(Action::Shell(ShellAction::TogglePanel(id)));
                self.forget_band_tab(id);
            }
            Cmd::Maximize => self.bottom_maximized = !self.bottom_maximized,
        }
    }

    /// 落地 Sidebar 齿轮菜单攒下的命令（见 `pending_dock_cmds` 的注释）。
    ///
    /// 侧带菜单里没有 `Split`，所以不需要带成员表；空表传下去即可。
    pub(in crate::ui) fn flush_pending_dock_cmds(&mut self) {
        if self.pending_dock_cmds.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending_dock_cmds);
        for (cmd, zone) in pending {
            self.apply_dock_cmd(cmd, zone, &[]);
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting_needs_a_cut_inside_the_band() {
        let all = vec![PanelId::Sender, PanelId::Shell];
        assert_eq!(split_groups(&all, None), vec![all.clone()]);
        // 切在两端 = 没切：一条带不该被切成「有内容 + 空白」。
        assert_eq!(split_groups(&all, Some(0)), vec![all.clone()]);
        assert_eq!(split_groups(&all, Some(2)), vec![all.clone()]);
        assert_eq!(
            split_groups(&all, Some(1)),
            vec![vec![PanelId::Sender], vec![PanelId::Shell]]
        );
    }

    #[test]
    fn a_group_shows_the_active_tab_or_its_first_member() {
        let group = [PanelId::Sender, PanelId::Shell];
        assert_eq!(group_active(&group, Some(PanelId::Shell)), PanelId::Shell);
        // 活动标签在别的组：这一组退回自己的第一颗，不会空着。
        assert_eq!(
            group_active(&group, Some(PanelId::Explorer)),
            PanelId::Sender
        );
        assert_eq!(group_active(&group, None), PanelId::Sender);
    }

    #[test]
    fn every_panel_has_its_own_dot_colour() {
        // Sender / Shell 取自底带图；Explorer / Outline / Sidebar 取自浮窗图
        // （`clipboard-2026-11-16T13-36-02-041Z-fc3bb6c9.png`，标签条逐像素采样）。
        // 这三条以前是猜的，且全猜反了：Explorer 是珊瑚红、Outline 是浅青、
        // Session（= `Sidebar`）是蓝。
        assert_eq!(
            dot_color(PanelId::Sender),
            egui::Color32::from_rgb(0x99, 0x6E, 0xE2)
        );
        assert_eq!(
            dot_color(PanelId::Shell),
            egui::Color32::from_rgb(0xFF, 0x00, 0x99)
        );
        assert_eq!(
            dot_color(PanelId::Explorer),
            egui::Color32::from_rgb(0xFF, 0x78, 0x7C)
        );
        assert_eq!(
            dot_color(PanelId::Outline),
            egui::Color32::from_rgb(0x8F, 0xDB, 0xF9)
        );
        assert_eq!(
            dot_color(PanelId::Sidebar),
            egui::Color32::from_rgb(0x00, 0x93, 0xFF)
        );
        let mut seen = Vec::new();
        for id in PanelId::ALL {
            assert!(
                !seen.contains(&dot_color(id)),
                "{} 的色块与别人撞色",
                id.name()
            );
            seen.push(dot_color(id));
        }
    }

    /// 跑一帧覆盖层，喂一组指针事件。
    fn frame_of_overlay(
        ctx: &egui::Context,
        win: &mut TerminalWindow,
        rect: egui::Rect,
        events: Vec<egui::Event>,
    ) {
        let input = egui::RawInput {
            screen_rect: Some(rect),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| win.draw_bottom_dock_overlay(ctx, rect));
    }

    fn click(ctx: &egui::Context, win: &mut TerminalWindow, rect: egui::Rect, pos: egui::Pos2) {
        // egui 的 `clicked()` 要求按下与松开落在两个 pass 里。
        for pressed in [true, false] {
            frame_of_overlay(
                ctx,
                win,
                rect,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                }],
            );
        }
    }

    /// 展开态那颗 `v` 必须点得动（覆盖层是手动 `Ui::new`，命中区要真登记进
    /// egui），并且展开期间绝不改 `bottom_height`——收回才能精确恢复原位。
    /// 这条守的正是「收不回来」与「顶部挤成一堆」两次反馈的交集。
    #[test]
    fn the_overlay_collapse_button_restores_the_dock_without_touching_its_height() {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let mut win =
            crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        win.bottom_maximized = true;
        let restore_h = win.bottom_height;

        // 热身一帧：让 egui 把覆盖层里的 widget 位置登记好。
        frame_of_overlay(&ctx, &mut win, rect, Vec::new());

        // 三颗里第一颗（`v`）的中心，与 `tab_strip` 的算位完全一致。
        let btns_left = rect.right() - EDGE_PAD - BTN_W * 3.0;
        let chevron = egui::pos2(
            btns_left + EDGE_PAD + BTN_W / 2.0,
            rect.top() + TAB_H / 2.0,
        );
        click(&ctx, &mut win, rect, chevron);

        assert!(
            !win.bottom_maximized,
            "点展开态的 v 必须收回到正常底带，否则又变成「收不回来」"
        );
        assert_eq!(
            win.bottom_height, restore_h,
            "展开期间不得改动回位高度，收回才能恢复原位"
        );
    }
}
