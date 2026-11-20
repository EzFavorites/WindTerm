//! P4：拖拽落点的判定——**纯函数**，不碰 `Ui`、不碰 `Context`。
//!
//! 只回答一个问题：指针在某处松手时，这个动作是「停到哪条带的第几位」还是
//! 「变成浮动窗」。所有输入都是值（指针、整窗矩形、各面板本帧的真实矩形），
//! 所以可以脱离窗口系统单测——这正是把它从绘制里拆出来的理由。
//!
//! 判定用的是**本帧各面板真正画出来的矩形**，不是估算的槽位。这样「插在第 2 位」
//! 的含义与屏幕上看到的一致：指针越过了第 1 块面板的中线，就排到它后面。

use eframe::egui;
use term_app::dock::{DockZone, PanelId};

/// 落点边带的厚度（点）。小于这个距离才算「贴边停靠」，中间一律当浮动。
pub(in crate::ui) const DOCK_EDGE: f32 = 36.0;

/// 拖动判定阈值：位移小于这个数就按「点击」处理，不改布局。
///
/// 抓手盖住整块面板，而按钮画在它之后（`egui` 里后注册的控件在上），所以按
/// 按钮时按钮赢。这一条是兜底：万一某个控件没接住按下事件，短距离松手也不会
/// 把面板甩出去。
pub(in crate::ui) const GRAB_MIN_DISTANCE: f32 = 8.0;

/// 拖拽松手的位置翻译成什么。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum DropTarget {
    /// 停到某带第几位。
    Dock { zone: DockZone, index: usize },
    /// 落在中间：变成浮动窗。
    Float,
}

/// 落点判定。
///
/// `docked` 是本帧各面板**实际画出来的矩形**，按发射顺序排列——也就是带内位次。
/// 用它数「指针之前有几块面板」得到插入位次，于是落点与看得见的位置一致。
///
/// 优先级：左右两条竖带先判（角落归竖带），再判底带。这样把面板拖到右下角得到
/// 的是「右带末尾」而不是「底带最右」——竖带是整高的，视觉上更自然。
pub(in crate::ui) fn resolve_drop(
    pointer: egui::Pos2,
    bounds: egui::Rect,
    docked: &[(PanelId, DockZone, egui::Rect)],
    edge: f32,
) -> DropTarget {
    let zone = if pointer.x <= bounds.left() + edge {
        DockZone::Left
    } else if pointer.x >= bounds.right() - edge {
        DockZone::Right
    } else if pointer.y >= bounds.bottom() - edge {
        DockZone::Bottom
    } else {
        return DropTarget::Float;
    };
    let along_x = zone == DockZone::Bottom;
    let mut index = 0;
    for (_, z, rect) in docked {
        if *z != zone {
            continue;
        }
        let (mid, at) = if along_x {
            (rect.center().x, pointer.x)
        } else {
            (rect.center().y, pointer.y)
        };
        if mid < at {
            index += 1;
        }
    }
    DropTarget::Dock { zone, index }
}

/// 位移够不够算一次拖动。
pub(in crate::ui) fn drag_far_enough(press: egui::Pos2, release: egui::Pos2) -> bool {
    press.distance(release) >= GRAB_MIN_DISTANCE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
    }

    /// 一块 1200×800 的窗口；左栏 200 宽、两块右栏各 260 宽。
    fn scene() -> (egui::Rect, Vec<(PanelId, DockZone, egui::Rect)>) {
        (
            rect(0.0, 0.0, 1200.0, 800.0),
            vec![
                (
                    PanelId::Sidebar,
                    DockZone::Left,
                    rect(0.0, 24.0, 200.0, 700.0),
                ),
                (
                    PanelId::Filer,
                    DockZone::Right,
                    rect(940.0, 60.0, 260.0, 700.0),
                ),
                (
                    PanelId::Explorer,
                    DockZone::Right,
                    rect(680.0, 60.0, 260.0, 700.0),
                ),
            ],
        )
    }

    #[test]
    fn a_drop_near_an_edge_docks_into_that_band() {
        let (bounds, docked) = scene();
        assert_eq!(
            resolve_drop(egui::pos2(10.0, 500.0), bounds, &docked, DOCK_EDGE),
            DropTarget::Dock {
                zone: DockZone::Left,
                index: 1
            },
            "落在左带、且越过侧栏的中线 → 插在它后面"
        );
        assert_eq!(
            resolve_drop(egui::pos2(10.0, 100.0), bounds, &docked, DOCK_EDGE),
            DropTarget::Dock {
                zone: DockZone::Left,
                index: 0
            },
            "中线以上 → 插在第一位"
        );
        assert_eq!(
            resolve_drop(egui::pos2(1190.0, 100.0), bounds, &docked, DOCK_EDGE),
            DropTarget::Dock {
                zone: DockZone::Right,
                index: 0
            }
        );
    }

    #[test]
    fn dropping_in_the_middle_floats_the_panel() {
        let (bounds, _) = scene();
        assert_eq!(
            resolve_drop(egui::pos2(600.0, 400.0), bounds, &[], DOCK_EDGE),
            DropTarget::Float
        );
    }

    #[test]
    fn a_corner_belongs_to_the_vertical_band() {
        let (bounds, _) = scene();
        assert_eq!(
            resolve_drop(egui::pos2(1195.0, 795.0), bounds, &[], DOCK_EDGE),
            DropTarget::Dock {
                zone: DockZone::Right,
                index: 0
            },
            "右下角归右带：竖带是整高的，比底带更符合直觉"
        );
        assert_eq!(
            resolve_drop(egui::pos2(600.0, 795.0), bounds, &[], DOCK_EDGE),
            DropTarget::Dock {
                zone: DockZone::Bottom,
                index: 0
            }
        );
    }

    #[test]
    fn the_bottom_band_counts_along_the_horizontal_axis() {
        let (bounds, _) = scene();
        let docked = vec![(
            PanelId::Shell,
            DockZone::Bottom,
            rect(0.0, 600.0, 400.0, 200.0),
        )];
        assert_eq!(
            resolve_drop(egui::pos2(700.0, 790.0), bounds, &docked, DOCK_EDGE),
            DropTarget::Dock {
                zone: DockZone::Bottom,
                index: 1
            }
        );
        assert_eq!(
            resolve_drop(egui::pos2(100.0, 790.0), bounds, &docked, DOCK_EDGE),
            DropTarget::Dock {
                zone: DockZone::Bottom,
                index: 0
            }
        );
    }

    #[test]
    fn a_short_wobble_is_a_click_not_a_drag() {
        assert!(!drag_far_enough(
            egui::pos2(10.0, 10.0),
            egui::pos2(13.0, 12.0)
        ));
        assert!(drag_far_enough(
            egui::pos2(10.0, 10.0),
            egui::pos2(10.0, 40.0)
        ));
    }
}
