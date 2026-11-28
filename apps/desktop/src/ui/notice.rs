//! （`docs/design/10-crash-hardening.md`）的一行提示条。
//!
//! 两件事共用这一条：**界面某帧内部出错**（说明进程还活着、只是跳过了一帧）
//! 与**上次没走正常退出**（说明恢复自最近留存点、可能丢了几行输出）。
//!
//! # 为什么是浮层而不是界面上的一条固定带
//!
//! 它一天可能都不出现一次，占一行固定高度等于让每次启动都为一次崩溃买单——所有
//! 视觉基线也会跟着整体位移。浮层（`Order::Foreground`）只在真有话说的时候存在，
//! 出现时压在终端区上，不影响任何面板的布局计算。
//!
//! # 为什么画在最前而不是最后
//!
//! 提示条**必须**在可能崩溃的那段代码之前画出来：`catch_unwind` 吞掉 panic
//! 之后本帧剩下的一半不会再执行，画在最后的提示条就永远不会出现。前景层保证它
//! 就算先画也在最上面。

use crate::ui::*;

/// 提示的性质决定配色与措辞的分量。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum NoticeKind {
    /// 上次没走正常退出，已恢复最近一次留存。
    Recovered,
    /// 界面某帧内部出错，已跳过该帧。
    UiPanic,
    /// 错误类提示（转义写错 / 偏好写盘冲突 / 待发送被丢弃）。
    ///
    /// 与那两条一样走 `Order::Foreground`：状态栏左区在连接 / 隧道 / 日志
    /// 状态下会被更高优先级标签占住，单靠 `log_note` 会把这类提示吃掉。
    Error,
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

    /// 错误类提示：同一句话既进状态栏左区，也占这条前景浮层。
    pub(in crate::ui) fn error(text: &str) -> Self {
        Self {
            kind: NoticeKind::Error,
            text: clip(text),
        }
    }

    /// 前景条上的文本（供状态栏把错误抢到高优先级那一路）。
    pub(in crate::ui) fn text(&self) -> &str {
        &self.text
    }
}

/// 连续崩到这么多次就不再主动要重绘。
///
/// 目的是**留活口**而不是「放弃」：每帧都炸的画面如果还每帧 `request_repaint`，
/// CPU 会一直烧在一个永远画不出来的帧上。停在这里之后进程、会话、隧道全都还在，
/// 用户任何一次交互（点一下、按个键）都会重新触发一帧——真修好了就自动恢复。
const REPAINT_BACKOFF_AFTER: u32 = 10;

impl TerminalWindow {
    /// 界面这一帧没画完。记账 + 出提示条 + 要下一帧。
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

    /// 一帧里攒下的提示落地。
    ///
    /// `note` 是每帧新建的局部缓冲（`let mut note = None`），由 `poll_with_note`
    /// / `lifecycle_events::poll` 写入。帧末在这里交回窗口：普通提示只进
    /// `log_note`，错误类还升到前景 Notice——后者不会被「复制成功 / 模式切换」
    /// 这些普通提示冲掉，状态栏被连接 / 隧道标签占住时也看得见。
    pub(in crate::ui) fn commit_note(&mut self, note: Option<String>) {
        let Some(text) = note else {
            return;
        };
        // 已有错误时追加而不是替换：帧首（写盘冲突 / 迁移提示）与帧末
        // （poll 的转义错误）两条都要留在状态栏单槽里，谁也别吃掉谁。
        // 普通提示仍走直接赋值那条路（由各功能模块自己写 log_note）。
        if self.error_notice_text().is_some() {
            if let Some(prev) = self.log_note.as_mut() {
                if !prev.contains(&text) {
                    prev.push('\n');
                    prev.push_str(&text);
                }
            } else {
                self.log_note = Some(text.clone());
            }
        } else {
            self.log_note = Some(text.clone());
        }
        // 状态栏是一帧的易失状态：刷新、关窗、重开都拿不回这句话。把它同步
        // 写进当前会话日志（若正在记录），事后排查才有据可查。同一句已经
        // 在前景条上就不再重复写，避免每帧刷日志。
        let already = self.error_notice_text().is_some_and(|cur| cur.contains(&text));
        if !already {
            let active = self.active_slot();
            if let Some(id) = self.tabs.get(active).and_then(|t| t.session) {
                self.pool.log_note(id, &text);
            }
        }
        self.raise_error_notice(&text);
    }

    /// 把一条错误类提示抬到前景条；已有错误时**合并文本**而不是替换。
    ///
    /// 恢复 / 帧 panic 的通知优先，不被错误提示顶掉；同一句话也不重复抬，
    /// 否则用户点 × 之后下一帧又会冒出来。
    pub(in crate::ui) fn raise_error_notice(&mut self, text: &str) {
        if self
            .notice
            .as_ref()
            .is_some_and(|n| n.kind != NoticeKind::Error)
        {
            return;
        }
        let incoming = Notice::error(text);
        if incoming.text.is_empty() {
            return;
        }
        if let Some(existing) = self.notice.as_mut() {
            if existing.kind == NoticeKind::Error {
                // 已经含这句话就不重复抬（点 × 之后不能自己长回来）。
                if existing.text.contains(&incoming.text) {
                    return;
                }
                if !existing.text.is_empty() {
                    existing.text.push('\n');
                }
                existing.text.push_str(&incoming.text);
                return;
            }
        }
        self.notice = Some(incoming);
    }

    /// 状态栏左区要念的错误提示。只有错误类越过其它标签的优先级。
    pub(in crate::ui) fn error_notice_text(&self) -> Option<&str> {
        self.notice
            .as_ref()
            .filter(|n| n.kind == NoticeKind::Error)
            .map(Notice::text)
    }

    /// 画提示条（在 `draw` 的最前面调，理由见模块注释）。
    pub(in crate::ui) fn draw_notice(&mut self, ctx: &egui::Context) {
        let Some(notice) = &self.notice else {
            return;
        };
        if draw(ctx, notice) {
            // 错误提示点 × 就别让下一帧的同名 note 又把它抬回来。
            let clear_log = notice.kind == NoticeKind::Error
                && self.log_note.as_deref() == Some(notice.text.as_str());
            self.notice = None;
            if clear_log {
                self.log_note = None;
            }
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
        // 操作失败：同属红色系，但边框比帧 panic 略亮一档。
        NoticeKind::Error => (egui::Color32::from_rgb(0xE0, 0x6C, 0x4C), egui::Color32::from_rgb(0x2C, 0x1C, 0x1C)),
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
    use crate::ui::lifecycle_test_support::{fake_tab, window_with, CloseCount};
    use crate::ui::dialogs::triggers::model::{Rule, UiAction, Channel};

    /// 真实的每帧调用形态：`log_note` 长期非空（旧值一直在），但传给 poll 的
    /// 是本帧新建的空缓冲，帧末再交回窗口。若照旧把 `&mut self.log_note`
    /// 当局部缓冲传进去，`note.is_none()` 恒 false，转义错误就永远丢失。
    #[test]
    fn an_escape_error_is_reported_with_a_nonempty_window_note_in_the_real_frame_shape() {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let closes = CloseCount::default();
        let tab = fake_tab(&pool, &ctx, 0, &closes);
        let mut win = window_with(pool, &ctx, vec![tab]);
        win.log_note = Some("上一帧：配置已保存".into());

        let mut rule = Rule::default();
        rule.id = "bad-escape".into();
        rule.enabled = true;
        rule.on = Channel::BeforeConnect;
        rule.actions = vec![UiAction::SendText(r"\x0".into())];
        win.triggers = vec![rule];
        // 与真实帧一样先把窗口规则装进标签的求值器：draw 里由
        // `Tab::poll_with_policy` 每帧 reload，生命周期事件才看得到新规则。
        let core: Vec<_> = win
            .triggers
            .iter()
            .map(crate::ui::dialogs::triggers::model::ViewRule::to_core)
            .collect();
        win.tabs[0].triggers.reload(&core);
        win.lifecycle_events
            .lock()
            .expect("事件队列")
            .push(term_core::automation::Lifecycle::BeforeConnect);

        // 与 `mod.rs::draw` 同一形态：poll 只排队意图，帧末还要 drain 才落地。
        let mut note = None;
        let mut cmds = Vec::new();
        crate::ui::lifecycle_events::poll(&mut win, 100, &mut cmds, &mut note);
        crate::ui::trigger_intents::drain(&mut win, &mut cmds, &mut note);
        win.commit_note(note.take());

        let reported = win.log_note.as_deref().expect("错误不能被旧提示吃掉");
        assert!(reported.contains("转义写错"), "note={reported:?}");
        assert!(reported.contains("\\x 需要两位十六进制"), "note={reported:?}");
        assert!(
            win.error_notice_text().is_some_and(|t| t.contains("转义写错")),
            "错误类还要走前景通道"
        );
    }

    /// 错误类提示用前景 Notice 独立存活：普通提示写进单槽 `log_note` 不会
    /// 把它冲掉（「请重做」被复制 / 模式切换吃掉的那一路）。
    #[test]
    fn an_error_notice_outlives_ordinary_notes_in_the_single_slot() {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let closes = CloseCount::default();
        let mut win = window_with(pool.clone(), &ctx, vec![fake_tab(&pool, &ctx, 0, &closes)]);

        win.commit_note(Some("请重新打开设置页再试一次".into()));
        // 普通提示同一帧后到，只覆盖状态栏那条单槽。
        win.log_note = Some("已复制".into());
        assert_eq!(win.log_note.as_deref(), Some("已复制"));
        assert_eq!(
            win.error_notice_text(),
            Some("请重新打开设置页再试一次"),
            "前景错误提示不能被普通提示冲掉"
        );
        // 同一句不重复抬：点 × 之后下一帧不该自己长回来。
        win.raise_error_notice("请重新打开设置页再试一次");
        win.notice = None;
        win.commit_note(None);
        assert!(win.notice.is_none());
    }

    /// 两条错误先后落地（帧首 / 帧末）时合并文本，谁也别吃掉谁：单槽
    /// `log_note` 与前景 Notice 两处都要能同时看到两条。
    #[test]
    fn two_error_notes_are_merged_instead_of_replacing_each_other() {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let closes = CloseCount::default();
        let mut win = window_with(pool.clone(), &ctx, vec![fake_tab(&pool, &ctx, 0, &closes)]);

        win.commit_note(Some("帧首：偏好写盘冲突".into()));
        win.commit_note(Some("帧末：转义写错 \\x0".into()));

        let merged = win
            .error_notice_text()
            .expect("两条错误都要留在前景条")
            .to_string();
        assert!(merged.contains("帧首：偏好写盘冲突"), "帧首被吃掉：{merged:?}");
        assert!(merged.contains("帧末：转义写错"), "帧末被吃掉：{merged:?}");
        let log = win.log_note.as_deref().expect("单槽也要拼起来");
        assert!(log.contains("帧首") && log.contains("帧末"), "log_note={log:?}");
        // 同一句重复抬不叠加；点 × 之后也不该自己长回来。
        win.raise_error_notice("帧末：转义写错 \\x0");
        assert_eq!(
            win.error_notice_text().map(str::to_string),
            Some(merged.clone())
        );
    }

    /// 跨函数复核：帧首 `sync_prefs_rev` 的写盘错误与帧末 `poll` 的转义错误
    /// 都经 `commit_note`，合并后前景条、状态栏、单槽三处都同时可见。
    #[test]
    fn a_frame_head_save_error_and_a_frame_end_poll_error_are_both_visible() {
        crate::ui::lifecycle_test_support::with_temp_profile(|| {
            let ctx = egui::Context::default();
            let pool = std::sync::Arc::new(term_session::SessionPool::new());
            let closes = CloseCount::default();
            let mut win = window_with(pool.clone(), &ctx, vec![fake_tab(&pool, &ctx, 0, &closes)]);
            let _ = crate::ui::dialogs::shortcuts::take_startup_migration_notice();

            // 帧首：真实写盘冲突，交给 sync_prefs_rev 抬到窗口。
            let mut prefs = term_profile::prefs::Prefs::default();
            prefs.language = "en".into();
            assert!(term_profile::prefs::save(&prefs));
            let path = term_paths::profile_dir()
                .expect("临时 profile 目录")
                .join(term_profile::prefs::PREFS_FILE);
            std::fs::write(&path, r#"{"language":"zh-CN"}"#).expect("伪造外部改动");
            prefs.font_size = 20.0;
            assert!(!term_profile::prefs::save(&prefs));
            win.sync_prefs_rev(&ctx);
            assert!(
                win.error_notice_text()
                    .is_some_and(|t| t.contains("请重新打开设置页")),
                "帧首错误要先上屏"
            );

            // 帧末：lifecycle poll 的转义错误攒进本帧局部缓冲，再交回窗口。
            let mut rule = Rule::default();
            rule.id = "frame-end-escape".into();
            rule.enabled = true;
            rule.on = Channel::BeforeConnect;
            rule.actions = vec![UiAction::SendText(r"\x0".into())];
            win.triggers = vec![rule];
            let core: Vec<_> = win
                .triggers
                .iter()
                .map(crate::ui::dialogs::triggers::model::ViewRule::to_core)
                .collect();
            win.tabs[0].triggers.reload(&core);
            win.lifecycle_events
                .lock()
                .expect("事件队列")
                .push(term_core::automation::Lifecycle::BeforeConnect);
            let mut note = None;
            let mut cmds = Vec::new();
            crate::ui::lifecycle_events::poll(&mut win, 100, &mut cmds, &mut note);
            crate::ui::trigger_intents::drain(&mut win, &mut cmds, &mut note);
            win.commit_note(note.take());

            let merged = win
                .error_notice_text()
                .expect("两帧两条错误都要在前景条")
                .to_string();
            assert!(
                merged.contains("请重新打开设置页"),
                "帧首错误被帧末吃掉：{merged:?}"
            );
            assert!(merged.contains("转义写错"), "帧末错误被吃掉：{merged:?}");
            let rendered = win.status_text();
            assert!(
                rendered.contains("请重新打开设置页"),
                "状态栏也要念出帧首错误：{rendered:?}"
            );
            assert!(
                rendered.contains("转义写错"),
                "状态栏也要念出帧末错误：{rendered:?}"
            );
            let log = win.log_note.as_deref().expect("单槽也要拼起来");
            assert!(
                log.contains("请重新打开设置页") && log.contains("转义写错"),
                "log_note={log:?}"
            );
        });
    }

    /// 语义坑：**一帧画好了不等于没出过错**。错一次、之后全好是最常见的情形，
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
