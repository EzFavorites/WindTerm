//! P4：停靠 / 浮动的**词表**——面板身份、停靠带、四个布局动作。
//!
//! 这一层只有名字与意图，没有一个字节的几何：`term-app` 不依赖 `egui`（B1），
//! 「浮在哪个坐标、拖到第几位」由投影层算。这么切是因为两边的生命周期不同：
//! 词表要能进 `Action` 被 reducer 单测，几何只活在 `ui/layout/dock.rs`。
//!
//! 谁不在这张表里：菜单栏 / 标签条 / 状态栏 / 查找栏。它们是 `ui/shell/` 的
//! 固定框架——视觉基线里这四条从来不动，做成可拖只会让人把窗口拖散。可停靠的
//! 是「有开关的面板」：侧栏、文件面板、七个工具面板。
//!
//! **布局状态不在这里**：`DockLayout`（谁在第几带第几位）是投影层的视图结构，
//! 与 `ui/layout/split.rs` 的 `Layout` 同类。reducer 只声明意图（本模块的
//! `LayoutAction`），落地的 Effect 由投影层执行——它才认识矩形。

use crate::effect::Effect;

pub use crate::vocab::PanelId;


pub use crate::vocab::DockZone;


pub use crate::vocab::LayoutAction;


/// 纯层只声明意图。`ViewState` 一个字段都不写——布局树不在那里。
/// 命令 ID → 本能力的动作。未接线的 ID 返回 `None`。
///
/// 2026-11-12 R2：实现只有一份，在 [`crate::parse`]（入口层）。这里保留同名入口，
/// 一是因为「每个能力自带字符串入口」是这个 crate 对外的约定，二是因为
/// `apps/desktop` 有直接调 `term_app::log::parse` 的地方——把路径改掉没有架构价值。
pub fn parse(id: &str) -> Option<LayoutAction> {
    crate::parse::dock_parse(id)
}

pub fn reduce(action: LayoutAction) -> Vec<Effect> {
    vec![Effect::Layout(action)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_panel_name_round_trips() {
        for panel in PanelId::ALL {
            assert_eq!(
                PanelId::parse(panel.name()),
                Some(panel),
                "{}",
                panel.name()
            );
        }
        assert_eq!(PanelId::parse("nope"), None);
    }

    #[test]
    fn panel_names_are_unique_and_file_safe() {
        let mut seen = Vec::new();
        for panel in PanelId::ALL {
            assert!(!seen.contains(&panel.name()), "重名：{}", panel.name());
            assert!(
                panel
                    .name()
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_'),
                "名字要能直接当文件名：{}",
                panel.name()
            );
            seen.push(panel.name());
        }
    }

    #[test]
    fn exactly_seven_panels_get_a_heading_from_the_dock() {
        let tool: Vec<&str> = PanelId::ALL
            .into_iter()
            .filter(|p| p.is_tool_pane())
            .map(|p| p.name())
            .collect();
        // 工具面板 = 全部 − {侧栏, 文件}：前者第一格是「＋ New Session」，后者是地址栏，
        // 都不该再被停靠容器加标题。新增 `Script` 后总数 = 10 − 2 = 8。
        assert_eq!(tool.len(), 8);
        assert!(!tool.contains(&"sidebar") && !tool.contains(&"filer"));
    }

    #[test]
    fn zone_names_round_trip() {
        for zone in DockZone::ALL {
            assert_eq!(DockZone::parse(zone.name()), Some(zone));
        }
        assert_eq!(DockZone::parse("top"), None, "顶上是固定框架，不是停靠带");
    }

    #[test]
    fn the_default_bands_match_the_new_full_window_golden() {
        // 新版截图：左 Explorer/Filer，右 Session(Sidebar)/Outline，底 Sender/Shell。
        let names = |zone| -> Vec<&str> {
            PanelId::ALL
                .into_iter()
                .filter(|p| p.default_zone() == zone)
                .map(|p| p.name())
                .collect()
        };
        assert_eq!(names(DockZone::Left), vec!["pane_explorer", "filer"]);
        assert_eq!(
            names(DockZone::Right),
            vec![
                "sidebar",
                "pane_outline",
                "pane_session",
                "pane_transfer",
                "pane_sysmon",
            ]
        );
        assert_eq!(
            names(DockZone::Bottom),
            vec!["pane_sender", "pane_shell", "pane_script"]
        );
    }

    #[test]
    fn the_bottom_band_owns_sender_and_shell() {
        // 底带的成员是**默认值**的一部分：视觉基线里底带就是这两颗标签，加上后来接入的
        // `Script` 停靠面板（同样归底带）。
        let bottom: Vec<&str> = PanelId::ALL
            .into_iter()
            .filter(|p| p.default_zone() == DockZone::Bottom)
            .map(|p| p.name())
            .collect();
        assert_eq!(bottom, vec!["pane_sender", "pane_shell", "pane_script"]);
    }

    #[test]
    fn reset_layout_is_a_real_command_but_the_others_are_not() {
        assert_eq!(parse("Window.ResetLayout"), Some(LayoutAction::Reset));
        assert_eq!(parse("Window.FloatSidebar"), None);
    }

    #[test]
    fn reduce_only_forwards_the_intent() {
        let action = LayoutAction::Dock {
            panel: PanelId::Filer,
            zone: DockZone::Left,
            index: 1,
        };
        assert_eq!(reduce(action.clone()), vec![Effect::Layout(action)]);
    }
}
