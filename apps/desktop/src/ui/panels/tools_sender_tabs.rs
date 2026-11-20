//! Sender 面板的**多页**：数据结构 + 底部那条数字 tab 条。
//!
//! # 用户口径（2026-11-17 原话）
//!
//! > sender 是底部 tab，只有一个的时候不显示 tab，多个的时候，每 + 一个，
//! > tab 标题按照数字 +1，底部 tab 显示 123456 这样，关闭也只是关闭当前的，
//! > 比如关闭 2，剩下的就是 13456 这样。
//!
//! 三件事被这句话钉死：
//!
//! 1. **只有一页时整条 tab 条不画**——不是画一条只有一个 tab 的条；
//! 2. **编号只增不重排**：`next_no` 是编号来源，与 `tabs.len()` 无关。关掉
//!    2 号之后剩下 `0 1 3 4 5`，再新增是 6，不会有人被改成别的号；
//! 3. **关闭只关当前那一页**，`−` 在只剩一页时是死键（不是清空）。
//!
//! # 外观：`sender-multi-tab` 视觉基线（2026-11-17 用户提供）
//!
//! 2× 截图逐像素量出来的口径（除 2 得逻辑点）：
//!
//! | 元素 | 视觉基线 | 逻辑点 |
//! |---|---|---|
//! | tab 条高 | 48 px | 24 |
//! | 一格宽 | 48 px | 24 |
//! | 格间分隔线 | 2 px | 1 |
//! | 未选中底 | `#333333` | `CHROME_SCALE.tab` |
//! | 选中底 | `#1C1C1C` | 与面板正文同色，看起来「连着正文」 |
//! | tab 条**在面板最底部** | — | 不是正文上方 |
//!
//! 第 5 条是这次纠正的重点：上一版把条放在工具行下面（正文上方），视觉基线里
//! 它在内容区**最下沿**、整条贴着面板底边。首格编号也从 1 改成 0（视觉基线是
//! `0 1 2 … 9`）。
//!
//! # 为什么一页一整份 `SenderState`
//!
//! 每一页都是一份完整的「发送任务」：文本、Text/Hex、By 行/字符、Count、
//! Interval、Targets、勾选集、正在跑的那个 `Run`。共用一个 `Run` 会让两页
//! 抢同一个定时器——用户切走一页，另一页的节奏就断了。

use eframe::egui;
use term_core::theme::CHROME_SCALE;

use super::tools_sender_state::SenderState;

/// tab 条高（视觉基线 48 px / 2）。
pub(in crate::ui) const TAB_STRIP_H: f32 = 24.0;
/// 一格宽（视觉基线 48 px / 2）。**定宽**而不是按文字量：视觉基线里 `1` 和 `8`
/// 的格子一样宽，条看起来才是刻度尺而不是一排按钮。
const TAB_W: f32 = 24.0;
/// 格间那根 1 点分隔线。
const SEP_W: f32 = 1.0;

/// 一页：`no` 是给用户看的编号，`state` 是这一页的全部配置。
pub(in crate::ui) struct SenderTab {
    pub no: u32,
    pub state: SenderState,
}

/// 整个 Sender 面板：若干页 + 当前页。
pub(in crate::ui) struct SenderPanel {
    tabs: Vec<SenderTab>,
    active: usize,
    /// 下一个新页的编号。**只增不减**，关页不回退。
    next_no: u32,
}

impl Default for SenderPanel {
    fn default() -> Self {
        Self {
            // 开面板就有一页 **0 号**（视觉基线首格是 0）：用户第一次看到的就是
            // 一个能直接敲的输入区，编号与视觉基线一致。
            tabs: vec![SenderTab {
                no: 0,
                state: SenderState::default(),
            }],
            active: 0,
            next_no: 1,
        }
    }
}

impl SenderPanel {
    /// 首帧就铺 `n` 页（`n ≤ 1` 与 [`Default`] 等价）。
    ///
    /// 只给截图夹具用（`WINDTERM_SENDER_PAGES`）：多页的 tab 条只能在出图里
    /// 一眼看出对错，而点十次 `＋` 的夹具没法用命令行复现。
    pub(in crate::ui) fn seeded(n: usize) -> Self {
        let mut panel = Self::default();
        for _ in 1..n {
            panel.add();
        }
        panel.select(0);
        panel
    }

    pub(in crate::ui) fn tabs(&self) -> &[SenderTab] {
        &self.tabs
    }

    /// 各页的可写视图。定时推进要逐页取出 / 放回 `run`，走这里；
    /// 单页的配置仍走 [`Self::state_mut`]，别用它绕开「当前页」这个概念。
    pub(in crate::ui) fn tabs_mut(&mut self) -> &mut [SenderTab] {
        &mut self.tabs
    }

    pub(in crate::ui) fn active(&self) -> usize {
        self.active
    }

    /// 当前页的全部配置（只读）。
    pub(in crate::ui) fn state(&self) -> &SenderState {
        &self.tabs[self.active].state
    }

    /// 当前页的全部配置（可写）。**唯一**改配置的入口。
    pub(in crate::ui) fn state_mut(&mut self) -> &mut SenderState {
        let i = self.active;
        &mut self.tabs[i].state
    }

    /// 当前页的编号（tab 条的激活项、编辑器 id_salt 都用它）。
    pub(in crate::ui) fn active_no(&self) -> u32 {
        self.tabs[self.active].no
    }

    /// 各页编号，按显示顺序。
    pub(in crate::ui) fn nums(&self) -> Vec<u32> {
        self.tabs.iter().map(|t| t.no).collect()
    }

    /// 只有一页时 tab 条不画。
    pub(in crate::ui) fn single(&self) -> bool {
        self.tabs.len() == 1
    }

    /// `＋`：新开一页并切过去，返回它的编号。
    pub(in crate::ui) fn add(&mut self) -> u32 {
        let no = self.next_no;
        self.next_no += 1;
        self.tabs.push(SenderTab {
            no,
            state: SenderState::default(),
        });
        self.active = self.tabs.len() - 1;
        no
    }

    /// `−`：关掉**当前**那一页。只剩一页时不动并返回 `false`。
    ///
    /// 关掉 2 号之后当前页落到原来的 3 号（同一位置的下一个），关掉最后一页
    /// 则退到前一页——用户视线不会因为关页跳到远处。
    pub(in crate::ui) fn close_active(&mut self) -> bool {
        if self.tabs.len() <= 1 {
            return false;
        }
        self.tabs.remove(self.active);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
        true
    }

    /// 切到第 `i` 页（越界忽略——tab 条与状态可能差一帧）。
    pub(in crate::ui) fn select(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.active = i;
        }
    }
}

/// 画 tab 条。返回被点中的页下标（`None` = 这一帧没点）。
///
/// 只读入参、不碰窗口状态：夹具能定格任意一页，生产路径由调用方落地
/// `select()`。
///
/// 画法与视觉基线一一对应：格子定宽 24、格间 1 点深色分隔线；**未选中**格
/// 铺 `#333333`，**选中**格不铺（露出面板正文色）——选中那一格看起来「连着
/// 正文」，这是参考实现区分当前页的方式。条右端没有额外背景：视觉基线里最后一个
/// 数字右边就是面板底色。
pub(in crate::ui) fn strip(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    nums: &[u32],
    active: usize,
) -> Option<usize> {
    let p = ui.painter_at(rect);
    let font = egui::TextStyle::Button.resolve(ui.style());
    let body = crate::theme::chrome(CHROME_SCALE.dialog);
    let mut x = rect.left();
    let mut hit = None;
    for (i, no) in nums.iter().enumerate() {
        let r = egui::Rect::from_min_size(
            egui::pos2(x, rect.top()),
            egui::vec2(TAB_W.min(rect.right() - x), rect.height()),
        );
        if r.width() <= 1.0 {
            break; // 面板窄到放不下：宁可少画，也不画到裁剪区外
        }
        let resp = ui
            .interact(r, egui::Id::new(("sender_tab", i)), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if i == active {
            // 选中格 = 正文色（不铺 #333），格与正文连成一片。
            p.rect_filled(r, egui::Rounding::ZERO, body);
        } else if resp.hovered() {
            p.rect_filled(r, egui::Rounding::ZERO, crate::theme::chrome(CHROME_SCALE.hover));
        } else {
            p.rect_filled(r, egui::Rounding::ZERO, crate::theme::chrome(CHROME_SCALE.tab));
        }
        let c = if i == active {
            crate::theme::chrome(CHROME_SCALE.text_strong)
        } else {
            crate::theme::chrome(CHROME_SCALE.text)
        };
        let g = p.layout_no_wrap(no.to_string(), font.clone(), c);
        p.galley(
            egui::pos2(r.center().x - g.size().x / 2.0, r.center().y - g.size().y / 2.0),
            g,
            c,
        );
        // 格间一根 1 点深色竖线（视觉基线 2 px / 2）；最后一格右边不画。
        if i + 1 < nums.len() {
            p.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(r.right(), rect.top()),
                    egui::vec2(SEP_W, rect.height()),
                ),
                egui::Rounding::ZERO,
                body,
            );
        }
        if resp.clicked() {
            hit = Some(i);
        }
        x = r.right() + SEP_W;
    }
    hit
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 打开就是 0 号一页（视觉基线首格是 0），且这一页不显示 tab 条。
    #[test]
    fn a_fresh_panel_has_exactly_one_page_numbered_zero() {
        let p = SenderPanel::default();
        assert_eq!(p.nums(), vec![0]);
        assert!(p.single(), "只有一页时不画 tab 条");
        assert_eq!(p.active_no(), 0);
    }

    /// 用户举的那串数字：关掉 2 之后剩下的编号**不重排**，新增接着往后数。
    /// 0 起时同样的例子是 `0 1 3 4 5`，再新增是 6。
    #[test]
    fn closing_a_page_leaves_the_others_numbered_as_they_were() {
        let mut p = SenderPanel::default();
        for _ in 0..5 {
            p.add();
        }
        assert_eq!(p.nums(), vec![0, 1, 2, 3, 4, 5]);
        // 切到 2 号（下标 2）再关掉它。
        p.select(2);
        assert!(p.close_active());
        assert_eq!(p.nums(), vec![0, 1, 3, 4, 5]);
        // 关掉当页后停在原位置的下一个（原来的 3 号）。
        assert_eq!(p.active_no(), 3);
        // 新增接着 5 往后数，不会补进被关掉的 2。
        assert_eq!(p.add(), 6);
        assert_eq!(p.nums(), vec![0, 1, 3, 4, 5, 6]);
    }

    /// 只剩一页时 `−` 是死键，不是「清空并重建」。
    #[test]
    fn the_last_page_cannot_be_closed() {
        let mut p = SenderPanel::default();
        assert!(!p.close_active());
        assert_eq!(p.nums(), vec![0]);
        // 两页时关掉末尾那一页，当前页回退到前一页。
        p.add();
        p.select(1);
        assert!(p.close_active());
        assert_eq!(p.active_no(), 0);
        assert!(p.single());
    }

    /// 每页的配置互不串：文本、编码、节奏、勾选集都是各自一份。
    #[test]
    fn pages_keep_their_own_text_and_settings() {
        let mut p = SenderPanel::default();
        p.state_mut().buf = "first".into();
        p.add();
        assert_eq!(p.state().buf, "", "新页是干净的");
        p.state_mut().buf = "second".into();
        p.state_mut().hex = true;
        p.select(0);
        assert_eq!(p.state().buf, "first");
        assert!(!p.state().hex);
        p.select(1);
        assert_eq!(p.state().buf, "second");
        assert!(p.state().hex);
    }
}
