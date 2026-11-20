//! D5 窗口透明度里**带命令 ID 入口**的端到端判据。
//!
//! # 为什么单独成文件
//!
//! 4e（I5）扫 `ui/` 下一处命令 ID 字面量都不许有，只放过由
//! `#[cfg(test)] #[path = …] mod …;` 两行声明拉进来的纯测试模块
//! （见 `ui/mod.rs`）。而这里的判据**必须**走 `dispatch("…")` 字符串入口：
//! 直接调 `apply_effect` 只能证明落地那几行对，证明不了「菜单点下去 →
//! `Prefs` 真的落盘 → 重启仍生效」这一整条。
//!
//! # 为什么临时 profile 目录
//!
//! 落地会 `save_prefs`（load-modify-save 写 `prefs.json`）。测试不能改用户
//! 真实的配置文件，所以每条用例把 `WINDTERM_PROFILE_DIR` 指到一次性的临时
//! 目录并在结束时还原。该变量是进程级全局，用一把**共享的**锁
//! （`lifecycle_test_support::with_temp_profile`，应用面那组用例用的是同一把）
//! 把「设 → 跑 → 还原」包住，避免各组用例互相看见对方的目录。
//!
//! **需用户有头环境肉眼确认**的部分（OS 窗口是否真的透、半透明到哪一档
//! 好看）不在这里伪造：无头测试只能验状态与落盘，不能验像素。

use crate::ui::dialogs::SettingsPage;
use crate::ui::lifecycle_test_support::{fake_tab, window_with, with_temp_profile, CloseCount};
use crate::ui::{TerminalWindow, WindowOpacity};
use eframe::egui;

/// egui 0.29 的 `Context` 没有 `visuals()` 取值器，得从 `style()` 上取。
fn visuals(ctx: &egui::Context) -> egui::Visuals {
    ctx.style().visuals.clone()
}

/// 一条标签的真窗口（菜单命令要活动标签才走得通）。
fn one_tab_window(ctx: &egui::Context) -> TerminalWindow {
    let pool = std::sync::Arc::new(term_session::SessionPool::new());
    let closes = CloseCount::default();
    let tab = fake_tab(&pool, ctx, 0, &closes);
    window_with(pool, ctx, vec![tab])
}

/// 读回盘上那份 `Prefs`，给出窗口透明度的两个字段。
fn saved() -> (bool, f32) {
    let prefs = term_runtime::prefs::load().expect("这一刀必须真的写盘");
    (prefs.window_transparent, prefs.window_opacity)
}

/// 三档预设：逐条走 `dispatch("ID")`，同时验运行期副本与**落盘值**。
///
/// 只验内存字段是不够的——「重启仍生效」是这次接线的硬要求，而盘上那份
/// 才是重启时唯一的事实来源。档位数值与纯层 `WindowOpacityPreset` 同一组。
#[test]
fn the_three_presets_dispatch_to_their_own_prefs_values() {
    with_temp_profile(|| {
        let cases: [(&str, bool, f32); 3] = [
            ("Application.SetWindowOpaque", false, 1.0),
            ("Application.SetWindowTranslucent", true, 0.6),
            ("Application.SetWindowTransparent", true, 0.3),
        ];
        for (id, transparent, opacity) in cases {
            let ctx = egui::Context::default();
            let mut win = one_tab_window(&ctx);
            win.dispatch(id);

            assert_eq!(win.window_opacity.transparent, transparent, "{id} 运行期副本");
            assert!(
                (win.window_opacity.opacity - opacity).abs() < 1e-6,
                "{id} 运行期副本 opacity"
            );
            let (saved_transparent, saved_opacity) = saved();
            assert_eq!(saved_transparent, transparent, "{id} 落盘 transparent");
            assert!(
                (saved_opacity - opacity).abs() < 1e-6,
                "{id} 落盘 opacity：{saved_opacity}"
            );
            // 重启 = 从盘上重新接一份运行期副本，必须与菜单刚点的完全一致。
            let restart = WindowOpacity::from_prefs(
                &term_runtime::prefs::load().expect("重启时要读得回 prefs"),
            );
            assert_eq!(restart, win.window_opacity, "{id} 重启仍生效");
        }
    });
}

/// 第四条命令不设值：打开设置并**预选 `Appearance` 页**（那页有开关 + 滑杆）。
/// 停在默认页也是一种没接好——2026-11-15 起默认页已按参考实现改成 `Application`，
/// 所以这条命令必须显式 `FocusAppearancePage`，这里同时钉住「对话框真的开了」。
#[test]
fn the_fourth_command_opens_settings_on_the_appearance_page() {
    with_temp_profile(|| {
        let ctx = egui::Context::default();
        let mut win = one_tab_window(&ctx);
        assert!(!win.view.show_settings, "出厂不该已经弹着设置");

        win.dispatch("Application.SetWindowOpacity");

        assert!(win.view.show_settings, "对话框必须打开");
        assert_eq!(
            win.settings_page,
            SettingsPage::Appearance,
            "必须停在能调透明度的那一页"
        );
        // 这条命令不该改任何透明度值（它只是把用户带到控件面前）。
        assert_eq!(win.window_opacity, WindowOpacity::default());
        assert!(!term_runtime::prefs::load().is_some_and(|p| p.window_transparent));
    });
}

/// 「Opaque 后现状 100% 不变」：半透明之后再点 Opaque，背景必须回到完全
/// 不透明，且与**从来没开过透明**的那份 `Visuals` 一样不透明。
///
/// 这里手动调 `sync_window_opacity` 模拟 `main.rs::update` 的每帧钩子
/// （测试不跑整个 draw 循环）；判据只认 alpha，不认 RGB 的取整细节——
/// 后者由下一帧 `apply_ui_style` 从基础调色板整体重建。
#[test]
fn opaque_after_translucent_restores_the_factory_visuals() {
    with_temp_profile(|| {
        let ctx = egui::Context::default();
        let mut win = one_tab_window(&ctx);
        let factory = visuals(&ctx);
        assert!(factory.window_fill.is_opaque(), "出厂皮肤必须不透明");

        win.dispatch("Application.SetWindowTranslucent");
        win.sync_window_opacity(&ctx);
        assert!(
            !visuals(&ctx).window_fill.is_opaque(),
            "半透明那一档必须真的把背景 alpha 装上"
        );
        assert_eq!(visuals(&ctx).window_fill.a(), 153, "0.6 * 255 = 153");

        win.dispatch("Application.SetWindowOpaque");
        win.sync_window_opacity(&ctx);
        assert_eq!(win.window_opacity.alpha(), 1.0, "Opaque 后渲染侧读回 1.0");
        assert!(visuals(&ctx).window_fill.is_opaque(), "回到不透明");
        assert!(visuals(&ctx).panel_fill.is_opaque(), "所有表面色一起回来");
        assert_eq!(
            visuals(&ctx).widgets.inactive.bg_fill.a(),
            factory.widgets.inactive.bg_fill.a(),
            "控件表面与出厂态一致"
        );
        let (transparent, opacity) = saved();
        assert!(!transparent, "落盘也回到不透明");
        assert_eq!(opacity, 1.0);
    });
}

/// 只在值变化时应用：连续两帧同一个值，`applied_window` 的记账不动，
/// 也不会把 `transparent` 再发一次。这条钉的是 `applied_window` 模式本身
/// （每帧无条件发视口命令会让部分平台窗口抖动）。
#[test]
fn syncing_an_unchanged_value_is_a_no_op() {
    with_temp_profile(|| {
        let ctx = egui::Context::default();
        let mut win = one_tab_window(&ctx);
        win.dispatch("Application.SetWindowTransparent");
        win.sync_window_opacity(&ctx);
        let applied = win.applied_window;
        assert_eq!(applied.transparent, Some(true));
        assert_eq!(applied.alpha_permille, Some(300));

        win.sync_window_opacity(&ctx);
        assert_eq!(win.applied_window.transparent, applied.transparent);
        assert_eq!(win.applied_window.alpha_permille, applied.alpha_permille);
        assert_eq!(visuals(&ctx).window_fill.a(), 77, "0.3 * 255 ≈ 77，值没被反复重装");
    });
}

/// 钳制边界在渲染侧：手改过的 `prefs.json` 里写一个 0.05，启动时的运行期
/// 副本照原样接过来，`alpha()` 却是 0.3；落盘值一个字节都不许被改掉。
#[test]
fn a_hand_edited_prefs_value_is_clamped_only_when_rendered() {
    with_temp_profile(|| {
        let ctx = egui::Context::default();
        let mut win = one_tab_window(&ctx);
        win.window_opacity = WindowOpacity {
            transparent: true,
            opacity: 0.05,
        };
        assert_eq!(win.window_opacity.alpha(), 0.3);
        assert_eq!(win.window_opacity.opacity, 0.05, "钳制不回写副本");
        assert!(!term_runtime::prefs::exists(), "这条用例不该写盘");
    });
}
