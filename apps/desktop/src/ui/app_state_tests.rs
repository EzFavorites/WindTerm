//! 应用状态**采集层**的判据（Q2 §10 刀1 验收 3）。
//!
//! `panels` 恒十条、`zone/order/floating` 与 `DockLayout::placement` 逐条一致——
//! 这两条只有采集层能证：`term-core` 那边只有形状（`AppSnapshot` 的字段与不变量），
//! 真实停靠位与可见性都只有 `ui` 拿得到。
//!
//! 与 `window_opacity_tests` 同一种组织方式：单独成文件、由 `ui/mod.rs` 的两行
//! 声明拉进来，不再往 `mod.rs` 里塞内联 `mod tests`。

use crate::ui::layout::Placement;
use crate::ui::lifecycle_test_support::{fake_tab, window_with, with_temp_profile, CloseCount};
use crate::ui::TerminalWindow;
use eframe::egui;
use term_app::dock::PanelId;
use term_app::shell::panel_visible;
use term_script::{AppError, LayoutOp};

fn one_tab_window(ctx: &egui::Context) -> TerminalWindow {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let closes = CloseCount::default();
    let tab = fake_tab(&pool, ctx, 0, &closes);
    window_with(pool, ctx, vec![tab])
}

/// 按名字找回 `PanelId`（快照里的 `id` 是 `PanelId::name()` 的字符串形态）。
fn panel_id(name: &str) -> PanelId {
    *PanelId::ALL
        .iter()
        .find(|id| id.name() == name)
        .unwrap_or_else(|| panic!("快照里的面板名 {name:?} 不在 PanelId::ALL 里"))
}

#[test]
fn panels_are_always_the_ten_dockable_panels_in_all_order() {
    let ctx = egui::Context::default();
    let window = one_tab_window(&ctx);
    let snapshot = window.app_snapshot();

    assert_eq!(snapshot.panels.len(), 10, "面板恒十条");
    let got: Vec<&str> = snapshot.panels.iter().map(|p| p.id).collect();
    let want: Vec<&str> = PanelId::ALL.iter().map(|id| id.name()).collect();
    assert_eq!(got, want, "顺序即 PanelId::ALL：客户端按 id 取，不靠长度变化");
    assert_eq!(
        snapshot.leaves,
        window.layout.leaf_count(),
        "窗格数取 Layout::leaf_count()，不是另算一份"
    );
    assert_eq!(snapshot.rev, 0, "刚起来的窗口没有写入，版本是 0");
    assert_eq!(snapshot.prefs_rev, 0, "偏好版本与界面版本分开计");
}

#[test]
fn placement_and_visibility_follow_the_live_window() {
    let ctx = egui::Context::default();
    let mut window = one_tab_window(&ctx);

    // 先把一块面板拖成浮动、再改它的可见性：两个来源都必须各自生效。
    window.dock.float(
        PanelId::Filer,
        egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(300.0, 400.0)),
    );
    window.view.show_filer = true;

    let snapshot = window.app_snapshot();
    for panel in &snapshot.panels {
        let id = panel_id(panel.id);
        // 逐条与**两个事实来源**对账：停靠位来自 DockLayout，可见性来自 ViewState。
        match window.dock.placement(id) {
            Placement::Dock { zone, order } => {
                assert_eq!(panel.zone, zone.name(), "{} 的停靠带", panel.id);
                assert_eq!(panel.order, order, "{} 的带内位次", panel.id);
                assert!(!panel.floating, "{} 停靠时不该报浮动", panel.id);
                assert!(panel.rect.is_none(), "{} 停靠时没有矩形", panel.id);
            }
            Placement::Floating { rect } => {
                assert!(panel.floating, "{} 是浮窗", panel.id);
                assert_eq!(
                    panel.rect,
                    Some([
                        rect.min.x.round() as i32,
                        rect.min.y.round() as i32,
                        rect.width().round() as i32,
                        rect.height().round() as i32,
                    ]),
                    "{} 的矩形必须与 DockLayout 给的一致（整数逻辑点）",
                    panel.id
                );
                assert_eq!(
                    panel.zone,
                    id.default_zone().name(),
                    "{} 浮动时仍报它的默认带，避免客户端拿空字符串当带名",
                    panel.id
                );
                assert_eq!(panel.order, 0, "{} 浮窗没有带内位次，给 0 而非随机值", panel.id);
            }
        }
        assert_eq!(
            panel.visible,
            panel_visible(&window.view, id),
            "{} 的可见性必须来自 ViewState",
            panel.id
        );
    }

    let filer = snapshot
        .panels
        .iter()
        .find(|p| p.id == "filer")
        .expect("filer 在十条里");
    assert!(filer.floating && filer.visible, "刚拖成浮动并设成可见");
    assert_eq!(filer.rect, Some([10, 20, 300, 400]));

    // 再拖回停靠带：采集必须跟着变，不能缓存上一帧的 placement。
    window.dock.redock(PanelId::Filer);
    let after = window.app_snapshot();
    let filer = after
        .panels
        .iter()
        .find(|p| p.id == "filer")
        .expect("filer 在十条里");
    assert!(!filer.floating && filer.rect.is_none(), "redock 之后不再报浮动");
}
/// 布局动作的验收（Q2 §10 刀3 四条）。四条共用一次临时 profile：
/// 布局**必然落盘**，所以「文件变了没有」本身是判据的一部分。
#[test]
fn app_layout_dock_index_guard_and_reset() {
    with_temp_profile(|| {
        let ctx = egui::Context::default();
        let mut window = one_tab_window(&ctx);

        // ① dock：`zone/order` 与请求一致，`changed` 有面板与 layout 两项，`rev`+1。
        let delta = window
            .apply_app_layout(&LayoutOp::Dock {
                panel: "pane_script".to_string(),
                zone: "bottom".to_string(),
                index: Some(0),
            })
            .expect("dock 成功");
        assert_eq!(
            delta.changed,
            vec!["panels.pane_script".to_string(), "layout".to_string()],
            "changed 列真的变了的字段：面板位次 + layout"
        );
        assert_eq!(delta.rev, 1, "写入成功推进版本");
        let moved = delta
            .panels
            .iter()
            .find(|p| p.id == "pane_script")
            .expect("pane_script 在十条里");
        assert_eq!(moved.zone, "bottom");
        assert_eq!(moved.order, 0, "钉到该带首位");

        // ② 落盘：同一份事实必须能在文件里读到（「重启仍生效」的判据）。
        let saved = term_profile::layout_state::load().expect("成功就该落盘");
        let entry = saved
            .dock
            .iter()
            .find(|d| d.panel == "pane_script")
            .expect("文件里有 pane_script");
        assert_eq!(entry.zone, "bottom");
        assert_eq!(entry.order, 0);

        // ③ 显式越界 → BadArgument；失败路径**零副作用**（文件与版本都不动）。
        let before = term_profile::layout_state::load();
        let err = window
            .apply_app_layout(&LayoutOp::Dock {
                panel: "pane_script".to_string(),
                zone: "bottom".to_string(),
                index: Some(99),
            })
            .expect_err("越界该拒");
        assert!(matches!(err, AppError::BadArgument(_)), "实得 {err:?}");
        assert_eq!(
            term_profile::layout_state::load(),
            before,
            "校验在派发之前，布局文件一个字节都不该动"
        );
        assert_eq!(window.app_snapshot().rev, 1, "失败不推进版本");

        // ④ reset：十块面板回各自的默认带，且这次也落盘。
        let reset = window.apply_app_layout(&LayoutOp::Reset).expect("reset 成功");
        assert_eq!(reset.rev, 2);
        assert_eq!(reset.changed, vec!["layout".to_string()], "reset 不改面板名，只动位次");
        for panel in &reset.panels {
            let id = panel_id(panel.id);
            assert_eq!(panel.zone, id.default_zone().name(), "{} 回默认带", panel.id);
            assert!(!panel.floating, "{} 不该是浮窗", panel.id);
        }
        let after = term_profile::layout_state::load().expect("reset 也落盘");
        assert_eq!(
            after.dock,
            window.dock.to_saved(),
            "内存里的布局与文件必须同一份事实"
        );
    });
}

/// `redock` 只收浮窗：停靠状态下调用是参数错，不是静默成功。
#[test]
fn app_layout_redock_refuses_a_docked_panel() {
    with_temp_profile(|| {
        let ctx = egui::Context::default();
        let mut window = one_tab_window(&ctx);
        let err = window
            .apply_app_layout(&LayoutOp::Redock {
                panel: "filer".to_string(),
            })
            .expect_err("停靠态 redock 该拒");
        assert!(matches!(err, AppError::BadArgument(_)), "实得 {err:?}");

        // 拖成浮窗之后同一调用才成立，并且真的收回默认带。
        window.dock.float(
            term_app::dock::PanelId::Filer,
            egui::Rect::from_min_size(egui::pos2(1.0, 2.0), egui::vec2(100.0, 200.0)),
        );
        let delta = window
            .apply_app_layout(&LayoutOp::Redock {
                panel: "filer".to_string(),
            })
            .expect("浮窗可以 redock");
        let filer = delta
            .panels
            .iter()
            .find(|p| p.id == "filer")
            .expect("filer 在十条里");
        assert!(!filer.floating, "redock 之后不再是浮窗");
    });
}
