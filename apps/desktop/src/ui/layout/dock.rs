//! 停靠位——**面板停在哪条带的第几位**（模型层，纯数据）。
//!
//! 三件事分在三个文件里，因为它们回答三个不同的问题：
//!
//! - **本文件**：谁在哪（`DockLayout`）+ 存盘形态（`to_saved` / `from_saved`）。
//! - `dock_drag.rs`：松手的位置算是什么意图（`resolve_drop`，纯函数，可单测）。
//! - `dock_paint.rs`：真去画（两条带 + 浮动窗 + 把动作落到模型上）。
//!
//! 与 `split.rs` 同一类：**布局是投影层的视图结构，不是 `ViewState` 字段**。
//! reducer 只声明意图（`term_app::dock::LayoutAction`），改动由 `effects` 落地。
//! 这条分工不是洁癖：布局要处理矩形与指针，把它塞进纯函数 reducer 就等于让
//! reducer 认识 `egui`（直接破）。
//!
//! # 发射点固定，成员与位次是数据
//!
//! `draw()` 里三个停靠带的**发射位置**是写死的，因为那由视觉基线的几何决定：
//!
//! ```text
//! 菜单栏（顶） → 状态栏（底） → 左带 → 标签条（顶） → 右带 → 底带 → 中央区
//! ```
//!
//! 左带在标签条**之前**声明，所以左栏是整高的、标签条只横跨终端区（视觉基线里
//! 会话树确实顶到菜单栏下沿）。右带在标签条之后，所以右栏从标签条下沿开始。
//! 把这两处对调，标签条就会横跨到左栏上方——这是**像素级**差别，六张整窗基线
//! 会立刻变红，所以它不能是「顺手调整」。
//!
//! 带内**谁在第几位**、以及谁被拖成了浮动窗，全部来自 [`DockLayout`]。默认布局
//! 由 `PanelId::ALL` 与 `default_zone()` 推出来，与三个写死 `SidePanel`
//! 调用逐字等价（`the_default_layout_is_exactly_the_p3_emission_order` 守这一条）。

use eframe::egui;
use term_app::dock::{DockZone, PanelId};
use term_runtime::layout_state::SavedPlacement;

/// 一个面板的停放方式。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::ui) enum Placement {
    /// 停在某带的第几位（`order` 决定同带内的左右 / 上下顺序）。
    Dock { zone: DockZone, order: u16 },
    /// 浮在上面，位置由用户拖出来。
    Floating { rect: egui::Rect },
}

/// 持久化「整条带浮起来」用的 zone 前缀。
///
/// 借 `SavedPlacement.zone` 这个已有字符串字段编码，**不改 layout.json 的
/// schema**：老版本读到不认识的 zone 会按既有规则跳过那一条，正好是安全降级。
/// `panel` 位写这条带的锚点面板，只为让人读文件时知道是哪条带；恢复只认 zone。
const BAND_FLOAT_PREFIX: &str = "float-band:";

/// 全部可停靠面板的停放表。
///
/// 用 `Vec<(PanelId, Placement)>` 而不是 `HashMap`：面板只有 8 个，线性查找
/// 更快也更容易解释；而且**顺序**是身份的一部分（`zone()` 要按 `order` 排）。
#[derive(Clone, Debug, PartialEq)]
pub(in crate::ui) struct DockLayout {
    placements: Vec<(PanelId, Placement)>,
    /// 带级浮动。成员**仍保留各自的 `Dock` 位次**，只把整条带记在这里：
    /// 这样收回时原位次一分不差（`redock` 单面板才会追加到末尾），而 `zone()`
    /// 对浮动带返回空，原停靠位自然呈现空壳。
    floating_bands: Vec<(DockZone, egui::Rect)>,
}

impl DockLayout {
    /// 默认布局：`ALL` 顺序 + `default_zone()`，与写死顺序逐字等价。
    pub(in crate::ui) fn new() -> Self {
        let mut out = Self {
            placements: Vec::new(),
            floating_bands: Vec::new(),
        };
        for id in PanelId::ALL {
            out.dock(id, id.default_zone());
        }
        out
    }

    pub(in crate::ui) fn placement(&self, id: PanelId) -> Placement {
        self.placements
            .iter()
            .find(|(p, _)| *p == id)
            .map(|(_, place)| *place)
            .unwrap_or(Placement::Dock {
                zone: id.default_zone(),
                order: 0,
            })
    }

    /// 某带的停靠成员（不看带是否浮着），按位次。
    fn docked_members(&self, zone: DockZone) -> Vec<PanelId> {
        let mut members: Vec<(u16, PanelId)> = PanelId::ALL
            .into_iter()
            .filter_map(|id| match self.placement(id) {
                Placement::Dock { zone: z, order } if z == zone => Some((order, id)),
                _ => None,
            })
            .collect();
        members.sort_by_key(|(order, _)| *order);
        members.into_iter().map(|(_, id)| id).collect()
    }

    /// 某带的成员，按位次。这是 `draw()` 真正遍历的东西。
    ///
    /// 整条带浮起来时**这里返回空**：原停靠位要让给空壳，成员只在
    /// 浮动窗内出现。成员本身的位次仍在 `docked_members` 里，收回即可复原。
    pub(in crate::ui) fn zone(&self, zone: DockZone) -> Vec<PanelId> {
        if self.is_band_floating(zone) {
            Vec::new()
        } else {
            self.docked_members(zone)
        }
    }

    /// 浮动窗里要显示的成员（含位次），与原停靠位无关。
    /// 单独浮出去的成员因为不再是 `Dock`，自然不在这里。
    pub(in crate::ui) fn band_members(&self, zone: DockZone) -> Vec<PanelId> {
        self.docked_members(zone)
    }

    /// 浮着的整带，按 `ALL` 的带序，保证绘制顺序稳定。
    pub(in crate::ui) fn floating_bands(&self) -> Vec<(DockZone, egui::Rect)> {
        DockZone::ALL
            .into_iter()
            .filter_map(|zone| {
                self.floating_bands
                    .iter()
                    .find(|(z, _)| *z == zone)
                    .map(|(_, rect)| (zone, *rect))
            })
            .collect()
    }

    pub(in crate::ui) fn is_band_floating(&self, zone: DockZone) -> bool {
        self.floating_bands.iter().any(|(z, _)| *z == zone)
    }

    /// 整带浮起：只记带与矩形，成员的位次原封不动。
    pub(in crate::ui) fn band_float(&mut self, zone: DockZone, rect: egui::Rect) {
        self.floating_bands.retain(|(z, _)| *z != zone);
        self.floating_bands.push((zone, rect));
    }

    /// 整带收回：删掉浮动记录即可，成员原位次还在。
    pub(in crate::ui) fn band_redock(&mut self, zone: DockZone) {
        self.floating_bands.retain(|(z, _)| *z != zone);
    }

    /// 浮动着的那几个（按 `ALL` 顺序，保证绘制顺序与遍历顺序无关地稳定）。
    pub(in crate::ui) fn floating(&self) -> Vec<(PanelId, egui::Rect)> {
        PanelId::ALL
            .into_iter()
            .filter_map(|id| match self.placement(id) {
                Placement::Floating { rect } => Some((id, rect)),
                _ => None,
            })
            .collect()
    }

    pub(in crate::ui) fn is_floating(&self, id: PanelId) -> bool {
        matches!(self.placement(id), Placement::Floating { .. })
    }

    /// 停到某带末尾（菜单 / 收回用）。
    pub(in crate::ui) fn dock(&mut self, id: PanelId, zone: DockZone) {
        self.insert(id, zone, usize::MAX);
    }

    /// 插到某带的第 `index` 位，并把该带重新编号。
    ///
    /// 重编号是必须的：`order` 只在带内比较，留空洞会让排序结果取决于 `Vec` 的
    /// 插入顺序而不再是位次。**两条带都要重编**：面板从一个带搬到另一个带时，
    /// 原带会留下一个洞，「文件里读回来的布局」与「内存里搬过的布局」就会在
    /// 位次上对不上（功能相同但不再是同一个值，往返测试立刻红）。
    pub(in crate::ui) fn insert(&mut self, id: PanelId, zone: DockZone, index: usize) {
        let came_from = match self.placement(id) {
            Placement::Dock { zone: from, .. } => Some(from),
            Placement::Floating { .. } => None,
        };
        // 用 `docked_members` 而不是 `zone()`：带浮着时 `zone()` 故意返回空，
        // 若在恢复路径上用它来重编号，会把所有成员的 order 压成 0。
        let mut members: Vec<PanelId> = self
            .docked_members(zone)
            .into_iter()
            .filter(|p| *p != id)
            .collect();
        let at = index.min(members.len());
        members.insert(at, id);
        for (order, panel) in members.iter().enumerate() {
            self.set(
                *panel,
                Placement::Dock {
                    zone,
                    order: order as u16,
                },
            );
        }
        if let Some(from) = came_from {
            if from != zone {
                self.renumber(from);
            }
        }
    }

    /// 把一条带的位次压实成 `0..n`。任何「让面板离开某带」的动作都要调它，
    /// 否则原带留下空洞，「搬过的布局」与「读回来的布局」就不是同一个值了。
    fn renumber(&mut self, zone: DockZone) {
        for (order, panel) in self.docked_members(zone).into_iter().enumerate() {
            self.set(
                panel,
                Placement::Dock {
                    zone,
                    order: order as u16,
                },
            );
        }
    }

    pub(in crate::ui) fn float(&mut self, id: PanelId, rect: egui::Rect) {
        let came_from = match self.placement(id) {
            Placement::Dock { zone, .. } => Some(zone),
            Placement::Floating { .. } => None,
        };
        self.set(id, Placement::Floating { rect });
        // 浮起来的面板离开了它的带，原带要压实——理由同 `renumber` 的注释。
        if let Some(from) = came_from {
            self.renumber(from);
        }
    }

    /// 收回默认带（浮动窗的 ×）。
    pub(in crate::ui) fn redock(&mut self, id: PanelId) {
        self.dock(id, id.default_zone());
    }

    pub(in crate::ui) fn reset(&mut self) {
        *self = Self::new();
    }

    fn set(&mut self, id: PanelId, place: Placement) {
        match self.placements.iter_mut().find(|(p, _)| *p == id) {
            Some(slot) => slot.1 = place,
            None => self.placements.push((id, place)),
        }
    }

    // ── 持久化 ──

    pub(in crate::ui) fn to_saved(&self) -> Vec<SavedPlacement> {
        let mut out: Vec<SavedPlacement> = PanelId::ALL
            .into_iter()
            .map(|id| match self.placement(id) {
                Placement::Dock { zone, order } => SavedPlacement {
                    panel: id.name().to_string(),
                    zone: zone.name().to_string(),
                    order,
                    rect: None,
                },
                Placement::Floating { rect } => SavedPlacement {
                    panel: id.name().to_string(),
                    zone: "float".to_string(),
                    order: 0,
                    rect: Some([
                        rect.min.x.round() as i32,
                        rect.min.y.round() as i32,
                        rect.width().round() as i32,
                        rect.height().round() as i32,
                    ]),
                },
            })
            .collect();
        // 一条带一条伪记录：panel 只是给人看的锚点，恢复只认 zone 与 rect。
        for (zone, rect) in &self.floating_bands {
            let anchor = self
                .docked_members(*zone)
                .first()
                .copied()
                .or_else(|| {
                    PanelId::ALL
                        .into_iter()
                        .find(|p| p.default_zone() == *zone)
                })
                .unwrap_or(PanelId::Explorer);
            out.push(SavedPlacement {
                panel: anchor.name().to_string(),
                zone: format!("{BAND_FLOAT_PREFIX}{}", zone.name()),
                order: 0,
                rect: Some([
                    rect.min.x.round() as i32,
                    rect.min.y.round() as i32,
                    rect.width().round() as i32,
                    rect.height().round() as i32,
                ]),
            });
        }
        out
    }

    /// 从布局文件重建。不认识的键 / 带名一律跳过那一条，不整份作废。
    pub(in crate::ui) fn from_saved(saved: &[SavedPlacement]) -> Self {
        let mut out = Self::new();
        let mut docks: Vec<(DockZone, u16, PanelId)> = Vec::new();
        for item in saved {
            // 整带浮动的伪记录先认：它的 panel 是锚点、zone 带前缀，不能走下面
            // 的面板解析（否则会把锚点面板误当成单面板浮动）。
            if let Some(name) = item.zone.strip_prefix(BAND_FLOAT_PREFIX) {
                if let (Some(zone), Some([x, y, w, h])) =
                    (DockZone::parse(name), item.rect)
                {
                    out.band_float(
                        zone,
                        egui::Rect::from_min_size(
                            egui::pos2(x as f32, y as f32),
                            egui::vec2(w as f32, h as f32),
                        ),
                    );
                }
                continue;
            }
            let Some(panel) = PanelId::parse(&item.panel) else {
                continue;
            };
            if item.zone == "float" {
                let Some([x, y, w, h]) = item.rect else {
                    continue;
                };
                out.float(
                    panel,
                    egui::Rect::from_min_size(
                        egui::pos2(x as f32, y as f32),
                        egui::vec2(w as f32, h as f32),
                    ),
                );
            } else if let Some(zone) = DockZone::parse(&item.zone) {
                docks.push((zone, item.order, panel));
            }
        }
        // 先按「带 + 位次」排好再依次追加：`dock()` 追加到末尾，于是顺序就是
        // 文件里写的顺序。直接逐条 `insert(…, order)` 会让后写的条目把先写的挤走。
        docks.sort_by_key(|(zone, order, _)| (zone_rank(*zone), *order));
        for (zone, _, panel) in docks {
            out.dock(panel, zone);
        }
        out
    }
}

impl Default for DockLayout {
    fn default() -> Self {
        Self::new()
    }
}

fn zone_rank(zone: DockZone) -> u8 {
    match zone {
        DockZone::Left => 0,
        DockZone::Right => 1,
        DockZone::Bottom => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
    }

    #[test]
    fn the_default_layout_is_the_full_window_golden() {
        // 2026-11-08 新版截图的默认铺满布局：左带 Explorer → Filer 上下堆叠，
        // 右带会话树 → Outline 上下堆叠，底带 Sender/Shell 两颗标签。
        // 这条断言是硬判据：默认布局一变，九张整窗基线立刻红。
        let layout = DockLayout::new();
        assert_eq!(
            layout.zone(DockZone::Left),
            vec![PanelId::Explorer, PanelId::Filer]
        );
        // Sysmon 不是停靠面板（终端底部细带），不在右带。
        assert_eq!(
            layout.zone(DockZone::Right),
            vec![
                PanelId::Sidebar,
                PanelId::Outline,
                PanelId::Session,
                PanelId::Transfer,
            ]
        );
        assert_eq!(
            layout.zone(DockZone::Bottom),
            vec![PanelId::Sender, PanelId::Shell, PanelId::Script]
        );
        assert!(layout.floating().is_empty());
    }

    #[test]
    fn moving_a_panel_to_another_band_renumbers_both_bands() {
        let mut layout = DockLayout::new();
        layout.insert(PanelId::Sidebar, DockZone::Left, 0);
        assert_eq!(
            layout.zone(DockZone::Left),
            vec![PanelId::Sidebar, PanelId::Explorer, PanelId::Filer]
        );
        assert_eq!(
            layout.zone(DockZone::Right),
            vec![PanelId::Outline, PanelId::Session, PanelId::Transfer],
            "换带之后原位次要重排，不能留下空洞"
        );
    }

    #[test]
    fn inserting_past_the_end_appends_instead_of_panicking() {
        let mut layout = DockLayout::new();
        layout.insert(PanelId::Sidebar, DockZone::Right, 99);
        assert_eq!(
            layout.zone(DockZone::Right).last().copied(),
            Some(PanelId::Sidebar)
        );
    }

    #[test]
    fn floating_takes_the_panel_out_of_its_band_and_redocking_puts_it_back() {
        let mut layout = DockLayout::new();
        layout.float(PanelId::Explorer, rect(100.0, 80.0, 260.0, 420.0));
        assert!(layout.is_floating(PanelId::Explorer));
        assert_eq!(layout.zone(DockZone::Left), vec![PanelId::Filer]);
        layout.redock(PanelId::Explorer);
        assert!(!layout.is_floating(PanelId::Explorer));
        // redock 是「追加到默认带末尾」，不是「插回原来的位次」——所以这里是
        // [Filer, Explorer]。要精确恢复出厂顺序得走 Reset Layout。
        assert_eq!(
            layout.zone(DockZone::Left),
            vec![PanelId::Filer, PanelId::Explorer]
        );
    }

    #[test]
    fn a_floating_band_leaves_an_empty_band_but_keeps_its_member_order() {
        let mut layout = DockLayout::new();
        let before = layout.zone(DockZone::Right);
        layout.band_float(DockZone::Right, rect(120.0, 80.0, 440.0, 400.0));
        assert!(layout.is_band_floating(DockZone::Right));
        assert!(
            layout.zone(DockZone::Right).is_empty(),
            "原停靠位要空出来给空壳"
        );
        assert_eq!(layout.band_members(DockZone::Right), before);
        layout.band_redock(DockZone::Right);
        assert!(!layout.is_band_floating(DockZone::Right));
        assert_eq!(
            layout.zone(DockZone::Right), before,
            "整带收回要回到原位次，不像单面板 redock 那样追加到末尾"
        );
    }

    #[test]
    fn a_floating_band_survives_a_round_trip_through_the_saved_form() {
        let mut layout = DockLayout::new();
        layout.band_float(DockZone::Bottom, rect(301.0, 202.0, 640.0, 318.0));
        layout.float(PanelId::Filer, rect(80.0, 60.0, 260.0, 420.0));
        assert_eq!(DockLayout::from_saved(&layout.to_saved()), layout);
    }

    #[test]
    fn a_bad_band_float_entry_is_skipped_without_touching_the_layout() {
        let saved = vec![
            SavedPlacement {
                panel: "pane_shell".into(),
                zone: "float-band:top".into(),
                order: 0,
                rect: Some([0, 0, 100, 100]),
            },
            SavedPlacement {
                panel: "pane_shell".into(),
                zone: "float-band:right".into(),
                order: 0,
                rect: None,
            },
        ];
        assert_eq!(DockLayout::from_saved(&saved), DockLayout::new());
    }

    #[test]
    fn reset_undoes_every_move_and_float() {
        let mut layout = DockLayout::new();
        layout.insert(PanelId::Filer, DockZone::Bottom, 0);
        layout.float(PanelId::Explorer, rect(10.0, 10.0, 200.0, 200.0));
        layout.band_float(DockZone::Right, rect(10.0, 10.0, 300.0, 300.0));
        layout.reset();
        assert_eq!(layout, DockLayout::new());
    }

    #[test]
    fn a_layout_survives_a_round_trip_through_the_saved_form() {
        let mut layout = DockLayout::new();
        layout.insert(PanelId::Transfer, DockZone::Left, 0);
        layout.float(PanelId::Filer, rect(120.0, 90.0, 260.0, 420.0));
        assert_eq!(DockLayout::from_saved(&layout.to_saved()), layout);
    }

    #[test]
    fn saved_entries_are_applied_in_file_order_not_in_one_pass() {
        // 一行一条、逐条 `dock()`：文件里的先后就是带内位次，读回来必须一致。
        //
        // 从「默认布局」起步，所以底带开头本来就有 Sender / Shell（`default_zone`
        // 的默认成员），文件里那两条追加在它们后面。这里断言的是**追加的那一段**
        // 保持文件顺序——那正是「逐条 `dock()` 而不是一次性 `insert(order)`」的
        // 分水岭。
        let saved = vec![
            SavedPlacement {
                panel: "pane_explorer".into(),
                zone: "bottom".into(),
                order: 0,
                rect: None,
            },
            SavedPlacement {
                panel: "pane_outline".into(),
                zone: "bottom".into(),
                order: 1,
                rect: None,
            },
        ];
        let layout = DockLayout::from_saved(&saved);
        let bottom = layout.zone(DockZone::Bottom);
        assert_eq!(
            bottom,
            vec![
                PanelId::Sender,
                PanelId::Shell,
                PanelId::Script,
                PanelId::Explorer,
                PanelId::Outline
            ]
        );
    }

    #[test]
    fn unknown_panels_and_bands_are_skipped_one_by_one() {
        let saved = vec![
            SavedPlacement {
                panel: "no_such_panel".into(),
                zone: "left".into(),
                order: 0,
                rect: None,
            },
            SavedPlacement {
                panel: "filer".into(),
                zone: "top".into(),
                order: 0,
                rect: None,
            },
            SavedPlacement {
                panel: "sidebar".into(),
                zone: "float".into(),
                order: 0,
                rect: None,
            },
        ];
        assert_eq!(
            DockLayout::from_saved(&saved),
            DockLayout::new(),
            "坏记录不该改变布局"
        );
    }
}
