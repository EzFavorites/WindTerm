//! L1 / L3（`docs/design/10-crash-hardening.md`）的一行提示条。
//!
//! 两件事共用这一条：**界面某帧内部出错**（L1，说明进程还活着、只是跳过了一帧）
//! 与**上次没走正常退出**（L3，说明恢复自最近留存点、可能丢了几行输出）。
//!
//! # 为什么是浮层而不是界面上的一条固定带
//!
//! 它一天可能都不出现一次，占一行固定高度等于让每次启动都为一次崩溃买单——所有
//! 视觉基线也会跟着整体位移。浮层（`Order::Foreground`）只在真有话说的时候存在，
//! 出现时压在终端区上，不影响任何面板的布局计算。
//!
//! # 为什么画在最前而不是最后
//!
//! L1 的提示条**必须**在可能崩溃的那段代码之前画出来：`catch_unwind` 吞掉 panic
//! 之后本帧剩下的一半不会再执行，画在最后的提示条就永远不会出现。前景层保证它
//! 就算先画也在最上面。

use crate::ui::*;

/// 提示的性质决定配色与措辞的分量。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum NoticeKind {
    /// L3：上次没走正常退出，已恢复最近一次留存。
    Recovered,
    /// L1：界面某帧内部出错，已跳过该帧。
    UiPanic,
}

/// 一条提示。
pub(in crate::ui) struct Notice {
    pub(in crate::ui) kind: NoticeKind,
    text: String,
}

impl Notice {
    /// 「恢复自上次留存」：说清**恢复到了哪一刻**以及可能丢了什么。
    pub(in crate::ui) fn recovered(saved_at: Option<u64>, now: u64) -> Self {
        let ago = match saved_at {
            Some(at) => format!("{} 秒", now.saturating_sub(at)),
            None => String::new(),
        };
        Self {
            kind: NoticeKind::Recovered,
            text: format!("上次未正常退出，已恢复到 {ago}前的留存点（这段时间的输出可能丢失）"),
        }
    }

    /// 「这一帧画崩了」：把次数与原因一起说，用户才知道要不要重启。
    pub(in crate::ui) fn ui_panic(times: u32, reason: &str) -> Self {
        Self {
            kind: NoticeKind::UiPanic,
            // 原因可能很长（unwrap 的整段格式化消息），截一段就够定位；完整文本
            // 已经在 stderr 上。
            text: format!("界面内部错误（第 {times} 次，已跳过该帧，会话未受影响）：{}", clip(reason)),
        }
    }
}

/// 连续崩到这么多次就不再主动要重绘。
///
/// 目的是**留活口**而不是「放弃」：每帧都炸的画面如果还每帧 `request_repaint`，
/// CPU 会一直烧在一个永远画不出来的帧上。停在这里之后进程、会话、隧道全都还在，
/// 用户任何一次交互（点一下、按个键）都会重新触发一帧——真修好了就自动恢复。
const REPAINT_BACKOFF_AFTER: u32 = 10;

impl TerminalWindow {
    /// L1（§3b②）：界面这一帧没画完。记账 + 出提示条 + 要下一帧。
    pub(crate) fn note_ui_panic(&mut self, ctx: &egui::Context, reason: &str) {
        self.frame_panics = self.frame_panics.saturating_add(1);
        self.notice = Some(Notice::ui_panic(self.frame_panics, reason));
        if self.frame_panics < REPAINT_BACKOFF_AFTER {
            ctx.request_repaint();
        }
    }

    /// 一帧画完了：把「连续崩」计数清零（重绘退避据此放大）。
    ///
    /// **不清提示条**：一条「界面内部出过错」的痕迹不该因为下一帧画成功了就消失
    /// ——那正好是最容易发生的情况（错一次、之后全好），也正好是最容易让人误以为
    /// 「什么都没发生过」的情况。用户点 × 才消失；再崩一次则更新成「第 N 次」。
    pub(crate) fn note_frame_ok(&mut self) {
        self.frame_panics = 0;
    }

    /// 画提示条（在 `draw` 的最前面调，理由见模块注释）。
    pub(in crate::ui) fn draw_notice(&mut self, ctx: &egui::Context) {
        let Some(notice) = &self.notice else {
            return;
        };
        if draw(ctx, notice) {
            self.notice = None;
            // 用户点了 ×：这一帧的交互要落地（否则提示条要等下一次输入才消失）。
            ctx.request_repaint();
        }
    }
}

/// 提示里最多显示多少个字符。超出部分用省略号——留出宽度给右侧的 ×。
fn clip(text: &str) -> String {
    const MAX: usize = 80;
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    let head: String = text.chars().take(MAX).collect();
    format!("{head}…")
}

/// 画这条提示，返回 `true` 表示用户点了右侧的 ×。
///
/// 位置钉在底部居中线上方 40 点（状态栏高度 27.5，再留一点缝），与窗口宽度无关
/// ——提示条比窗口窄得多，窗口被拖多大都不会跟着变形。
pub(in crate::ui) fn draw(ctx: &egui::Context, notice: &Notice) -> bool {
    let (accent, fill) = match notice.kind {
        // 崩溃恢复是「一句解释」，不是错误：琥珀色。
        NoticeKind::Recovered => (egui::Color32::from_rgb(0xDC, 0xB4, 0x3C), egui::Color32::from_rgb(0x2A, 0x24, 0x18)),
        // 界面内部错误：红色，明确「出问题了」。
        NoticeKind::UiPanic => (egui::Color32::from_rgb(0xD9, 0x5C, 0x5C), egui::Color32::from_rgb(0x2C, 0x1C, 0x1C)),
    };
    let mut dismissed = false;
    egui::Area::new(egui::Id::new("zterm-notice"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -40.0))
        .movable(false)
        // 关掉淡入。`Area` 默认 `fade_in(true)`，与 `tool_bar.rs` 的 pane 工具条
        // 同一个坑：夹具把 `i.time` 钉在 0，整条提示永远停在淡入第一帧（几乎全透明）。
        // 交互路径上只是少一个 100ms 的动画——一条「出过错」的提示本来也不该
        // 慢悠悠地浮现。
        .fade_in(false)
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, accent))
                .rounding(egui::Rounding::same(2.0))
                .inner_margin(egui::Margin::symmetric(10.0, 5.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        ui.label(
                            egui::RichText::new(&notice.text)
                                .color(egui::Color32::from_rgb(0xE6, 0xE6, 0xE6))
                                .size(crate::ui::type_scale::SIZE_BODY),
                        );
                        // × 用全软件那一份画法（`shell::glyph`），尺寸与各处标题栏一致。
                        let (rect, resp) =
                            ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
                        let color = if resp.hovered() { accent } else { egui::Color32::from_rgb(0x99, 0x99, 0x99) };
                        shell::paint_cross(ui.painter(), rect.center(), color);
                        if resp.clicked() {
                            dismissed = true;
                        }
                        if resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                    });
                });
        });
    dismissed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// L1 的语义坑：**一帧画好了不等于没出过错**。错一次、之后全好是最常见的情形，
    /// 也正是最容易变成「什么都没发生过」的情形——所以提示条只能由用户点 × 撤掉。
    #[test]
    fn a_panicking_frame_leaves_a_notice_that_outlives_good_frames() {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let mut win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
        assert!(win.notice.is_none(), "出厂窗口不该有话说");

        win.note_ui_panic(&ctx, "boom");
        assert!(win.notice.is_some(), "崩了一帧就该有提示条");
        assert_eq!(win.frame_panics, 1);

        win.note_frame_ok();
        assert_eq!(win.frame_panics, 0, "连续计数要清零（重绘退避据此判断）");
        assert!(win.notice.is_some(), "好的一帧不能把出过错的痕迹抹掉");
    }

    /// 两条提示的措辞各说各的事：恢复提示要给「多久以前」，错误提示要给「第几次」；
    /// 超长的 panic 文本要截断，否则提示条会横穿整个窗口。
    #[test]
    fn the_two_notices_say_different_things_and_long_reasons_are_clipped() {
        let recovered = Notice::recovered(Some(1_000), 1_090);
        assert_eq!(recovered.kind, NoticeKind::Recovered);
        assert!(recovered.text.contains("90 秒"), "要说清多久以前：{}", recovered.text);
        assert!(recovered.text.contains("未正常退出"));

        let long = "x".repeat(500);
        let panicked = Notice::ui_panic(3, &long);
        assert_eq!(panicked.kind, NoticeKind::UiPanic);
        assert!(panicked.text.contains("第 3 次"));
        assert!(panicked.text.ends_with('…'), "超长原因必须截断：{}", panicked.text);
        assert!(panicked.text.chars().count() < 140);
        // 未超长的原因一个字都不该丢。
        assert!(Notice::ui_panic(1, "short").text.contains("short"));
    }
}
