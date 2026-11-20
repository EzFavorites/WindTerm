//! D5：窗口透明度的**运行期副本 + 渲染落地**。
//!
//! 全链路有四段，这一段是最后一段：
//!
//! ```text
//! ShellAction::SetWindowOpacity(preset) / ManageWindowOpacity
//!   → Effect::SetWindowOpacity{…} / Effect::FocusAppearancePage
//!   → ui/effects.rs::apply_effect
//!   → 本文件的 set_window_opacity（改运行期副本 + 落盘）
//!   → main.rs::update 调 sync_window_opacity（只在值变化时应用）
//! ```
//!
//! # 为什么要有运行期副本
//!
//! `Prefs` 在盘上，每帧读一次盘去画背景既慢又会把「读盘失败」变成「窗口忽然
//! 不透明」。所以窗口上存一份 [`WindowOpacity`]（初值来自 `prefs`），菜单与
//! 设置页都改它并同时 `save_prefs`；渲染只读这份副本。**落盘是唯一持久化
//! 事实**，副本丢了下次启动会从盘上恢复。
//!
//! # egui 0.29 没有数值 opacity 命令
//!
//! `ViewportCommand` 只有 `Transparent(bool)`（管 OS 窗口能不能透），没有
//! 「把整窗画成 60% 不透明」这一条。所以数值效果由两条腿合成：
//!
//! 1. `theme::set_window_alpha` 把 `egui::Visuals` 的背景 / 控件表面色设成
//!    带 alpha 的颜色（字与描边保持不透明，否则整窗会糊）；
//! 2. 终端区那块**不透的调色板底色**由 [`TerminalWindow::window_bg_color`]
//!    按同一份 alpha 画出来——不处理它的话只有 chrome 透、正文仍是一块黑板。
//!
//! `theme::apply_ui_style` 每帧重建 `Visuals`，所以它会把当前 alpha 读回来
//! 再装回去（见那里的注释）；真正改值的动作只在下面 `sync_window_opacity`
//! 判到「值变了」的那一帧发生，与 `sync_window_modes` 的 `applied_window`
//! 同一个模式：不每帧发视口命令、不每帧重装一整份皮肤。
//!
//! # 出厂态 = 现状
//!
//! `window_transparent = false` 时 `alpha()` 恒为 1.0，`with_window_alpha`
//! 直接原样返回、`set_window_alpha` 不会被调用、`ViewportCommand::Transparent`
//! 也不会发（构造时就把 `applied_window.transparent` 初始化成盘上那个值）。
//! 于是所有既有视觉基线一个字节都不变。
//!
//! **可在 GUI 验收后调整**：三档预设的具体数值（1.0 / 0.6 / 0.3）写在
//! `term_app::vocab::WindowOpacityPreset`；本文件只负责「照着画」。

use super::TerminalWindow;
use eframe::egui;
use term_runtime::prefs::{self, Prefs};

/// 窗口上的运行期透明度副本。字段与 `Prefs` 同名，方便 load-modify-save 时
/// 一眼对上；它不是另一份真相，只是这一帧画背景用的缓存。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::ui) struct WindowOpacity {
    pub transparent: bool,
    /// 1.0 = 完全不透明。允许存任意 `f32`，钳制只在 `alpha()` 做。
    pub opacity: f32,
}

impl Default for WindowOpacity {
    fn default() -> Self {
        Self {
            transparent: false,
            opacity: 1.0,
        }
    }
}

impl WindowOpacity {
    /// 启动时从盘上接过来。**不钳制**：手改过的 `prefs.json` 里是什么就留什么，
    /// 免得一次无关的保存就把用户写的值抹掉（见 `prefs::clamp_window_opacity`）。
    pub(in crate::ui) fn from_prefs(prefs: &Prefs) -> Self {
        Self {
            transparent: prefs.window_transparent,
            opacity: prefs.window_opacity,
        }
    }

    /// 由 `Effect` 的千分比造一份（投影层唯一的换算点）。
    pub(in crate::ui) fn from_permille(transparent: bool, permille: u32) -> Self {
        Self {
            transparent,
            opacity: permille as f32 / 1000.0,
        }
    }

    /// 这一帧该把背景画成多不透明。
    ///
    /// 透明开关关着时**恒为 1.0**，与 `opacity` 存了什么无关——这就是
    /// 「Set Window Opaque 之后现状 100% 不变」的判据：一个像素都不碰。
    pub(in crate::ui) fn alpha(self) -> f32 {
        if self.transparent {
            prefs::clamp_window_opacity(self.opacity)
        } else {
            prefs::WINDOW_OPACITY_MAX
        }
    }
}

impl TerminalWindow {
    /// 菜单三档预设 / 设置页开关滑杆的**唯一落地点**。
    ///
    /// 两件事一起做，缺一件都是「点了没效果」：改运行期副本（本帧立刻可见）、
    /// load-modify-save 写回 `Prefs`（重启仍生效）。`save_prefs` 走的是
    /// `settings_live` 里那个已有的唯一写口，不另开一条写盘路径。
    pub(in crate::ui) fn set_window_opacity(&mut self, next: WindowOpacity) {
        self.window_opacity = next;
        crate::ui::dialogs::save_prefs(|prefs| {
            prefs.window_transparent = next.transparent;
            prefs.window_opacity = next.opacity;
        });
        self.ctx.request_repaint();
    }

    /// 每帧调用（`main.rs::update` 里 `draw` 之前）。**只在值变化时应用**，
    /// 与 `sync_window_modes` 的 `applied_window` 同一模式。
    ///
    /// 变化分两种，分别记账：`transparent` 变了才发一次
    /// `ViewportCommand::Transparent`（OS 窗口能不能透，每帧发会让部分平台抖）；
    /// alpha 变了才改一次 `egui::Visuals`（egui 0.29 没有数值 opacity 命令）。
    /// 两者都没变就立刻返回，空闲帧零开销。
    pub(crate) fn sync_window_opacity(&mut self, ctx: &egui::Context) {
        let desired = self.window_opacity;
        let alpha = desired.alpha();
        // 用整数记账：f32 的 `Option` 比较会把「0.6000001」当成一次变化，
        // 于是每帧都重装皮肤。千分比与 `Effect` 里那一档同一个口径。
        let alpha_permille = (alpha * 1000.0).round() as u32;
        let transparent_changed = self.applied_window.transparent != Some(desired.transparent);
        let alpha_changed = self.applied_window.alpha_permille != Some(alpha_permille);
        if !transparent_changed && !alpha_changed {
            return;
        }
        if transparent_changed {
            ctx.send_viewport_cmd(egui::ViewportCommand::Transparent(desired.transparent));
            self.applied_window.transparent = Some(desired.transparent);
        }
        if alpha_changed {
            // 出厂态（1.0）且上一帧也是不透明时**不要**碰 `Visuals`：
            // `set_window_alpha(_, 1.0)` 会走一遍颜色反预乘还原，那会在
            // 第一帧把既有视觉基线漂掉一两个色阶。只有「要变透」或
            // 「刚从透变回来」才真的重装皮肤。
            let was_translucent = self
                .applied_window
                .alpha_permille
                .is_some_and(|applied| applied < 1000);
            if alpha < 1.0 || was_translucent {
                let mut visuals = ctx.style().visuals.clone();
                crate::theme::set_window_alpha(&mut visuals, alpha);
                ctx.set_visuals(visuals);
            }
            self.applied_window.alpha_permille = Some(alpha_permille);
        }
        // 值变了就请一次重绘：S4 的空闲 0 CPU 约定下，不请这一下用户要等
        // 到下一次终端输出才看得到窗口变透。
        ctx.request_repaint();
    }

    /// 终端区整块底色的颜色。
    ///
    /// 原来这里是 `to_color32(self.palette.default_bg)`——一块完全不透的黑板。
    /// 只把 `Visuals` 改透而留着它，用户会看到「边框透了、正文没透」。
    /// `alpha = 1.0` 时 `with_window_alpha` 原样返回，与旧代码逐字节相同。
    pub(in crate::ui) fn window_bg_color(&self) -> egui::Color32 {
        crate::theme::with_window_alpha(
            crate::theme::to_color32(self.palette.default_bg),
            self.window_opacity.alpha(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三档预设各自落到哪一份运行期副本（与纯层的档位表同一组数）。
    /// 这里用的是投影层的换算入口，不是手写 f32 常量——少一处抄写。
    #[test]
    fn the_three_presets_land_on_their_own_transparent_and_opacity() {
        let opaque = WindowOpacity::from_permille(false, 1000);
        assert!(!opaque.transparent);
        assert!((opaque.opacity - 1.0).abs() < 1e-6);
        assert_eq!(opaque.alpha(), 1.0);

        let translucent = WindowOpacity::from_permille(true, 600);
        assert!(translucent.transparent);
        assert!((translucent.opacity - 0.6).abs() < 1e-6);
        assert!((translucent.alpha() - 0.6).abs() < 1e-6);

        let transparent = WindowOpacity::from_permille(true, 300);
        assert!(transparent.transparent);
        assert!((transparent.opacity - 0.3).abs() < 1e-6);
        assert!((transparent.alpha() - 0.3).abs() < 1e-6);
    }

    /// 钳制边界 + 「Opaque 后现状不变」：开关一关，无论落盘值多离谱，
    /// 渲染侧都读回 1.0（出厂态，什么也不改）。
    #[test]
    fn opaque_ignores_the_stored_value_and_clamping_is_read_side_only() {
        let opaque = WindowOpacity {
            transparent: false,
            opacity: 0.05,
        };
        assert_eq!(opaque.alpha(), 1.0, "关着透明开关时必须是出厂不透明");
        assert_eq!(opaque.opacity, 0.05, "钳制不改写落盘值");

        let below = WindowOpacity {
            transparent: true,
            opacity: 0.01,
        };
        assert_eq!(below.alpha(), 0.3);
        let above = WindowOpacity {
            transparent: true,
            opacity: 9.0,
        };
        assert_eq!(above.alpha(), 1.0);
        let nan = WindowOpacity {
            transparent: true,
            opacity: f32::NAN,
        };
        assert_eq!(nan.alpha(), 1.0, "坏值按出厂不透明兜底");
    }

    /// 出厂态下换色函数必须**原样返回**：这是「所有既有视觉基线不受影响」
    /// 的最直接判据（颜色一变，十一张 PNG 就全变了）。
    #[test]
    fn an_opaque_window_never_touches_the_palette_colours() {
        let base = egui::Color32::from_rgb(0x1C, 0x22, 0x2A);
        assert_eq!(crate::theme::with_window_alpha(base, 1.0), base);
        let mut visuals = egui::Visuals::dark();
        let before = visuals.panel_fill;
        let before_window = visuals.window_fill;
        let before_widget = visuals.widgets.inactive.bg_fill;
        // 出厂路径不会调 set_window_alpha；这里直接验「真调了 1.0 也只是还原」。
        crate::theme::set_window_alpha(&mut visuals, 1.0);
        assert_eq!(visuals.window_fill.a(), 255);
        assert_eq!(visuals.panel_fill.a(), before.a());
        assert_eq!(visuals.widgets.inactive.bg_fill.a(), before_widget.a());
        assert!(before_window.is_opaque());
    }

    /// 半透明那一档真的把所有表面色画成同一个 alpha，字与描边不动。
    #[test]
    fn a_translucent_window_sets_the_surface_alpha_but_keeps_text_opaque() {
        let mut visuals = egui::Visuals::dark();
        let text_before = visuals.widgets.inactive.fg_stroke;
        crate::theme::set_window_alpha(&mut visuals, 0.6);
        for c in [
            visuals.panel_fill,
            visuals.window_fill,
            visuals.extreme_bg_color,
            visuals.faint_bg_color,
            visuals.code_bg_color,
            visuals.selection.bg_fill,
            visuals.widgets.noninteractive.bg_fill,
            visuals.widgets.active.weak_bg_fill,
        ] {
            assert_eq!(c.a(), 153, "0.6 * 255 = 153");
        }
        assert_eq!(visuals.widgets.inactive.fg_stroke, text_before, "字形不许跟着变淡");
        assert_eq!(visuals.window_stroke.color.a(), 255, "描边不许跟着变淡");
    }

    /// 从半透明回到 Opaque：alpha 恢复 255，RGB 回到原来的量级（不是被预乘
    /// 暗掉之后就再也回不来）。
    #[test]
    fn going_back_to_opaque_restores_full_alpha() {
        let mut visuals = egui::Visuals::dark();
        let base = visuals.panel_fill;
        crate::theme::set_window_alpha(&mut visuals, 0.3);
        assert!(!visuals.panel_fill.is_opaque());
        crate::theme::set_window_alpha(&mut visuals, 1.0);
        assert!(visuals.panel_fill.is_opaque());
        assert!(visuals.window_fill.is_opaque());
        assert_eq!(visuals.panel_fill.r(), base.r(), "RGB 不该被预乘抹掉");
    }
}
