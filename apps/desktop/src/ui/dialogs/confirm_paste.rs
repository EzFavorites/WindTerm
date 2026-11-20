//! 粘贴确认弹窗（`Terminal ▸ Show paste dialog on the main / alternative screen`）。
//!
//! # 为什么不复用 `ConfirmClose`
//!
//! `dialogs::confirm` 那个枚举的文案、窗口高度与 `win_rect()` 全是按参考截图
//! **逐像素**钉死的（`Window` 两行 176 高、`Tab` / `AllTabs` 一行 162 高，
//! 两张组件基线守着一像素的差）。这里要塞一段可滚动的文本预览——两者的内容
//! 与高度都不是一类东西，硬塞进那个枚举就等于每加一种弹窗就动一遍已冻结的
//! 基线，而且「关会话」那句文案会跟着预览一起变形。
//!
//! 所以它单独一份：视觉沿用同一族（`DESIGN_TOKENS.panel_bg` 底、8 圆角、
//! 同一条标题条与红绿灯），但没有截图基线——兼容终端里没有这个弹窗的图，
//! 给它冻一张基线等于把「我设计的样子」冒充成「兼容终端的样子」。
//!
//! # 状态归属
//!
//! [`PendingPaste`] 挂在 `TerminalWindow.pending_paste`，与 `ConfirmState`
//! 同类：**一次待执行的副作用**（一段剪贴板文本 + 一个标签下标），不进
//! `ViewState`——reducer 不该认识剪贴板。
//!
//! # 落点
//!
//! `Paste` → `commit_paste`（与不弹窗的直通**同一个**方法）；`Cancel` →
//! 丢掉，一个字节都不写。标签在弹窗打开期间被关掉 → `commit_paste` 里
//! `get_mut` 拿不到，丢弃这次粘贴，不 panic。

use crate::ui::harness;
use crate::ui::*;

/// 待确认的一次粘贴。`tab` 是**槽位**（主区第几个标签 / 底带 Shell 面板第几条）：
/// 它要与那一刻画的那块区域对齐才能写回，槽位失效（标签被关）时整笔丢弃——
/// 见 `commit_paste`。
///
/// 2026-11-15：从裸下标换成 [`TabSlot`]。底带 Shell 面板也是一个终端、也要走
/// 粘贴确认（用户裁定面板与标签页同一套交互），而面板的标签不在 `tabs` 里，
/// 一个裸下标根本没有办法说出「面板里第二条」。
pub(in crate::ui) struct PendingPaste {
    pub(in crate::ui) tab: crate::ui::terminal::TabSlot,
    pub(in crate::ui) text: String,
}

/// 「这次粘贴要不要先问一句」——判据只有这一份。
///
/// 备用屏（`GridSnapshot.alt`，1049h）与主屏分开两个开关：全屏程序里粘贴
/// （vim 的 `:` 命令、less 的搜索）与 shell 提示符下的风险完全不同，一个开关
/// 替另一个做决定就是不诚实的粒度。
pub(in crate::ui) fn paste_needs_confirm(alt_screen: bool, main_flag: bool, alt_flag: bool) -> bool {
    if alt_screen {
        alt_flag
    } else {
        main_flag
    }
}

/// 预览上限：行数与字符数各一条。两条都要——一行几 MB 的 base64 按行数截不住，
/// 而 20 行空行按字符数也截不住。
const PREVIEW_MAX_LINES: usize = 20;
const PREVIEW_MAX_CHARS: usize = 800;

/// 预览的截断结果。
///
/// **只影响显示**：真正写进 PTY 的永远是全文（`commit_paste` 拿的是
/// `PendingPaste.text`，不是这个 `shown`）。把「看到多少」与「贴多少」分开，
/// 是为了让长粘贴不被截断，同时又不让弹窗长到屏幕上放不下。
pub(in crate::ui) struct Preview {
    pub(in crate::ui) shown: String,
    pub(in crate::ui) total_lines: usize,
    pub(in crate::ui) hidden_lines: usize,
    /// 字符上限先到（最后一行只显示了一截）。
    pub(in crate::ui) char_cut: bool,
}

/// 前 20 行 / 前 800 字。剩下的行数如实报出来，不假装预览就是全文。
pub(in crate::ui) fn preview(text: &str) -> Preview {
    let mut shown = String::new();
    let mut used_lines = 0usize;
    let mut total_lines = 0usize;
    let mut char_cut = false;
    for line in text.lines() {
        total_lines += 1;
        let full = shown.chars().count() >= PREVIEW_MAX_CHARS;
        if full || used_lines >= PREVIEW_MAX_LINES {
            continue;
        }
        if !shown.is_empty() {
            shown.push('\n');
        }
        let room = PREVIEW_MAX_CHARS.saturating_sub(shown.chars().count());
        let take: String = line.chars().take(room).collect();
        char_cut |= take.chars().count() < line.chars().count();
        shown.push_str(&take);
        used_lines += 1;
    }
    Preview {
        // `lines()` 对空串返回 0 行；「粘一段空文本」也该显示成 1 行空行，
        // 否则标题会写「0 行」。
        total_lines: total_lines.max(1),
        hidden_lines: total_lines.saturating_sub(used_lines),
        char_cut,
        shown,
    }
}

// ── 视觉常量（逻辑点）────────────────────────────────────────────────

/// 弹窗宽高。比关闭确认那张宽一截：这里要放等宽预览。
const WIN_W: f32 = 460.0;
const WIN_H: f32 = 300.0;
/// 标题条高（与 `confirm.rs` 同一条）。
const TITLE_H: f32 = 26.0;
/// 等宽字号（与确认弹窗同一条，切页时文字大小不跳）。
const TEXT_SIZE: f32 = crate::ui::type_scale::SIZE_BODY;

const TITLE_BAR: u32 = 0xECECEC;
const TRAFFIC_RED: u32 = 0xFF5F57;
const TRAFFIC_GRAY: u32 = 0xC5C5C5;
const TEXT: u32 = 0xF0F0F0;
const PREVIEW_BG: u32 = 0x1C1D1F;
const PREVIEW_BORDER: u32 = 0x3A3A3A;

fn col(hex: u32) -> egui::Color32 {
    crate::theme::to_color32(term_core::theme::rgb(hex))
}

/// 弹窗矩形：在视觉基线主窗口（1280×760）里居中，与确认弹窗同一条算法。
fn win_rect() -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2((1280.0 - WIN_W) / 2.0, (760.0 - WIN_H) / 2.0),
        egui::vec2(WIN_W, WIN_H),
    )
}

impl TerminalWindow {
    /// 弹窗打开期间的键盘模态。与 `confirm_pre_update` 同一个理由：模态要赶在
    /// 终端吃到键事件**之前**把键拿走，否则弹窗开着还能往远端打字。
    ///
    /// `Esc` / `Enter` 都归 `Cancel`（默认键，与兼容终端的「No 是默认」同一条
    /// 取舍）：粘贴是不可逆的字节投递，默认动作不该是「贴进去」。
    pub(in crate::ui) fn paste_pre_update(&mut self, ctx: &egui::Context) {
        if self.pending_paste.is_none() {
            return;
        }
        let cancel = ctx.input(|i| {
            i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter)
        });
        // 再按一次 Cmd+V 也不该排第二笔：`Paste` 事件一起吃掉。
        ctx.input_mut(|i| {
            i.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Paste(_)
                )
            })
        });
        if cancel {
            self.pending_paste = None;
        }
    }

    /// 弹窗本体。没有 `pending_paste` 时一行都不画。
    pub(in crate::ui) fn draw_paste_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_paste.as_ref() else {
            return;
        };
        // 预览在闭包外算好：闭包里还要借 `self`（判定按钮落点），
        // 而 `tr()` 借的是 `self.view`（about.rs 的教训）。
        let preview = preview(&pending.text);
        let heading = self.tr("Paste into the current session?");
        let cancel_text = self.tr("Cancel");
        let paste_text = self.tr("Paste");
        let more = if preview.hidden_lines > 0 {
            Some(format!("… {} more lines", preview.hidden_lines))
        } else if preview.char_cut {
            Some("…".to_string())
        } else {
            None
        };
        let shown = preview.shown.clone();

        let mut yes = false;
        let mut no = false;
        let id = egui::Id::new("confirm_paste");

        if harness::fixture_mode() {
            // 与 `confirm.rs` 同一条结论：首帧是 sizing pass，形状全是 Noop，
            // 必须先播种 AreaState。这个弹窗没有截图基线，播种留着是为了
            // 将来真要出图时（手动 `--demo`）不是一张空白。
            egui::Window::new("confirm_paste")
                .id(id)
                .collapsible(false)
                .resizable(false)
                .fade_in(false)
                .fixed_rect(win_rect())
                .title_bar(false)
                .frame(egui::Frame::none())
                .show(ctx, |ui| ui.set_min_size(egui::vec2(WIN_W, WIN_H)));
        }

        egui::Window::new("confirm_paste")
            .id(id)
            .collapsible(false)
            .resizable(false)
            .fade_in(false)
            .fixed_rect(win_rect())
            .title_bar(false)
            .frame(
                egui::Frame::none()
                    .fill(col(term_core::theme::DESIGN_TOKENS.panel_bg))
                    .rounding(8.0)
                    .shadow(egui::Shadow {
                        offset: egui::vec2(0.0, 6.0),
                        blur: 18.0,
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(110),
                    }),
            )
            .show(ctx, |ui| {
                ui.set_min_size(egui::vec2(WIN_W, WIN_H));
                let o = ui.min_rect().min;
                let p = ui.painter_at(egui::Rect::from_min_size(
                    o,
                    egui::vec2(WIN_W, WIN_H),
                ));

                // 标题条 + 红绿灯（与确认弹窗同一族，用户不会觉得是两个程序）。
                p.rect_filled(
                    egui::Rect::from_min_size(o, egui::vec2(WIN_W, TITLE_H)),
                    egui::Rounding {
                        nw: 8.0,
                        ne: 8.0,
                        sw: 0.0,
                        se: 0.0,
                    },
                    col(TITLE_BAR),
                );
                for (i, x) in [16.0, 34.0, 52.0].iter().enumerate() {
                    let c = o + egui::vec2(*x, TITLE_H / 2.0);
                    let hex = if i == 0 { TRAFFIC_RED } else { TRAFFIC_GRAY };
                    p.circle_filled(c, 5.0, col(hex));
                }

                p.text(
                    o + egui::vec2(16.0, 42.0),
                    egui::Align2::LEFT_CENTER,
                    heading,
                    crate::ui::type_scale::font(TEXT_SIZE),
                    col(TEXT),
                );

                // 预览框：底 + 边由 painter 画，正文交给 `ScrollArea`——
                // 20 行在框里放不下时可以滚，而不是被裁掉。
                let box_rect = egui::Rect::from_min_max(
                    o + egui::vec2(16.0, 58.0),
                    o + egui::vec2(WIN_W - 16.0, WIN_H - 52.0),
                );
                p.rect_filled(box_rect, 4.0, col(PREVIEW_BG));
                p.rect_stroke(
                    box_rect,
                    4.0,
                    egui::Stroke::new(1.0, col(PREVIEW_BORDER)),
                );
                let inner = box_rect.shrink2(egui::vec2(8.0, 6.0));
                ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("confirm_paste_preview")
                        .max_height(inner.height())
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&shown)
                                        .monospace()
                                        .size(TEXT_SIZE)
                                        .color(col(TEXT)),
                                )
                                .wrap(),
                            );
                            if let Some(more) = &more {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(more).monospace().size(TEXT_SIZE).weak(),
                                    )
                                    .wrap(),
                                );
                            }
                        });
                });

                // 右下两颗按钮：`Cancel`（蓝描边默认）/ `Paste`（灰描边）。
                // 与关闭确认弹窗共用 kit::footer 原语，靠规格变体保留宽度差。
                let by = WIN_H - 38.0;
                let cancel_spec = crate::ui::kit::spec::FooterButton::PasteCancel.spec();
                let paste_spec = crate::ui::kit::spec::FooterButton::PastePaste.spec();
                let cancel_rect = egui::Rect::from_min_size(
                    o + egui::vec2(276.0, by),
                    cancel_spec.size,
                );
                let paste_rect = egui::Rect::from_min_size(
                    o + egui::vec2(360.0, by),
                    paste_spec.size,
                );
                let cancel_resp = ui.allocate_rect(cancel_rect, egui::Sense::click());
                let paste_resp = ui.allocate_rect(paste_rect, egui::Sense::click());
                let clip = egui::Rect::from_min_size(o, egui::vec2(WIN_W, WIN_H));
                crate::ui::kit::footer::draw(
                    ui, clip, cancel_rect, cancel_text, cancel_spec, cancel_resp.hovered(), true,
                );
                crate::ui::kit::footer::draw(
                    ui, clip, paste_rect, paste_text, paste_spec, paste_resp.hovered(), true,
                );
                if cancel_resp.clicked() {
                    no = true;
                }
                if paste_resp.clicked() {
                    yes = true;
                }
            });

        // R5：两个按钮的真效果只在闭包外落地（闭包里 `self` 还被借着）。
        if yes {
            if let Some(pending) = self.pending_paste.take() {
                // 全文，不是 `preview.shown`：预览只是显示层。
                self.commit_paste(pending.tab, &pending.text);
            }
        } else if no {
            self.pending_paste = None;
        }
    }
}

#[cfg(test)]
#[path = "confirm_paste_tests.rs"]
mod tests;
