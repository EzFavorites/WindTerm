//! `Text ▸ Caret` 页（视觉基线 `13-caret.png`）。
//!
//! # 为什么单独一个文件
//!
//! 把 `Period` / `Width` 从灰显接成真控件之后，这一页要同时管
//! 下拉、预览方块、两个套框数字框与三项写回，塞在 `settings_live.rs` 里把那个
//! 文件顶过  的 500 代码行上限。拆分口径与 `settings_live_font` /
//! `settings_live_tabs` 一致：一页一个文件，页里的画法与它的写回放在一起。
//!
//! # 三项都是「新会话初值」
//!
//! `caret_style` / `caret_period` / `caret_width` 都是**会话属性**
//! （`SessionConfig` 的字段），不是全局显示项：改完只影响**之后新建**的标签 /
//! 分屏 / 会话。已经开着的会话不动——所以这里写的是窗口上的运行期副本
//! （下一帧起新建时读它）与 `prefs.json`（下次启动的初值），不碰任何活动标签。

use super::settings_live::{recovery_notice, save_prefs};
use super::settings_pages_view::{
    field_label, field_row, group_box, label_col, spin_field, Slot,
};
use crate::ui::*;

/// `caret` 页的编辑结果。三个字段各自 `Option`：`None` = 这一项没动。
#[derive(Default)]
struct CaretEdit {
    style: Option<usize>,
    period: Option<f32>,
    width: Option<f32>,
}

impl CaretEdit {
    /// 把动过的那几项写进 `prefs`（load-modify-save 的 modify 那一半）。
    ///
    /// 只碰自己这三个字段：语言 / 历史容量 / 启动策略原样留着——
    /// 一次写盘只认「用户刚改了什么」。
    fn write_into(&self, prefs: &mut term_profile::prefs::Prefs) {
        if let Some(i) = self.style {
            prefs.caret_style = i;
        }
        if let Some(p) = self.period {
            prefs.caret_period = p;
        }
        if let Some(w) = self.width {
            prefs.caret_width = w;
        }
    }
}

/// `Text ▸ Caret`：视觉基线 `13-caret.png` 的 `Caret` 分组框。
///
/// 八种样式就是 `term_core::config::CARET_STYLES`（0-3 不闪、4-7 闪），
/// 它是 `SessionConfig.caret_style` 的全部取值，所以这里只放一个真下拉。
///
/// 形状名就是 `term_core::config::CARET_STYLES`（0-3 不闪、4-7 闪），
/// 「闪不闪」靠名字里的 `Blinking …` 前缀交代。
///
/// 这里原先还在下拉右边画了一个光标预览方块（调 `terminal::draw_caret`），
/// 去掉：它挂在字段框**外面**，把 `Style` 一行的框挤得比
/// `Period` / `Width` 短一截；而视觉基线 `13-text-caret.png` 的 `Shape` 行
/// 只有一个 `▾`，没有预览。
///
/// ：`Period` / `Width` 两行从灰显接成真控件。此前它们是渲染层常数
/// （`time*2 % 1` 的 1 Hz 方波、`cell_h*0.14` 的比例宽度），现在分别是
/// `SessionConfig.caret_period`（秒，「亮多久 = 灭多久」）与 `caret_width`
/// （像素，`Underline` / `Beam` 用）。
pub(in crate::ui) fn caret(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let edit = draw(ui, win.caret_style, win.caret_period, win.caret_width);
    let touched = edit.style.is_some() || edit.period.is_some() || edit.width.is_some();
    if !touched {
        return;
    }
    // 先换窗口副本（下一帧起新建的标签就按它来），再落盘。
    if let Some(i) = edit.style {
        win.caret_style = i;
        // 新建会话表单是**草稿缓冲**：偏好变了，草稿的初值也跟上；
        // 正在编辑的那条会话要等「保存」才会被写入。
        win.new_session.caret_style = i;
    }
    if let Some(p) = edit.period {
        win.caret_period = p;
    }
    if let Some(w) = edit.width {
        win.caret_width = w;
    }
    save_prefs(|prefs| edit.write_into(prefs));
}

/// 画这一页，返回「哪几项被改了」。拆分出来只为让 [`caret`] 的写回部分读起来
/// 是一段直线（画 → 判 → 写），不被几十行布局代码隔开。
fn draw(ui: &mut egui::Ui, current: usize, period: f32, width: f32) -> CaretEdit {
    use term_core::config::CARET_STYLES;

    let mut edit = CaretEdit::default();
    // 越界（手改 prefs）按 Block 显示，与 `Prefs::caret_style_index` 同一条兜底。
    let shown = current.min(CARET_STYLES.len() - 1);
    recovery_notice(ui);
    ui.add_space(6.0);
    // 视觉基线 `13-text-caret.png`：内容区第一个字就是组标题 `Caret`。
    // 老实现画了灰页标题 `Caret` + 框内标题 `Caret`，同一个词出现两次。
    // 标签列按本页最长的 `Width:` 量。
    let col = label_col(ui, &["Style:", "Period:", "Width:"]);
    group_box(ui, Some("Caret"), |ui| {
        // `Style` 与同页 `Period` / `Width` 是同一个盒子：标签 + 通栏字段框。
        //
        // 这里原先在下拉右边另挂了一个 12×18 的光标预览方块，**挂在框外**——
        // 于是这一行的框比另两行短 12 点，右缘对不齐；方块本身画出来是「下拉右边
        // 多了一块白」，而视觉基线 `13-text-caret.png` 的 `Shape` 行只有一个 `▾`。
        // 把预览整个拿掉：三行现在逐个对得上，形状名字（`Block` /
        // `Blinking Block` …）本来就写在字段里，方块没多告诉用户什么。
        field_row(ui, "Style:", Slot::Inline, col, "", |ui| {
            egui::ComboBox::from_id_salt("prefs_caret_style")
                .selected_text(CARET_STYLES[shown])
                .width(ui.available_width())
                .wrap_mode(egui::TextWrapMode::Truncate)
                .show_ui(ui, |ui| {
                    for (i, name) in CARET_STYLES.iter().enumerate() {
                        if ui.selectable_label(i == shown, *name).clicked() {
                            edit.style = Some(i);
                        }
                    }
                });
        });
        // ：Period / Width 接成真控件（此前是渲染层常数）。
        // 两行同一套画法：左对齐标签 + 通栏字段框（`field_box`：描边 + 占满整行，
        // 框内的 `DragValue` 不再画自己那层小框）。后缀与区间照视觉基线。
        let number = |ui: &mut egui::Ui,
                      label: &str,
                      value: f32,
                      range: std::ops::RangeInclusive<f32>,
                      speed: f32,
                      suffix: &str|
         -> Option<f32> {
            let mut out = None;
            ui.horizontal(|ui| {
                field_label(ui, label, col);
                spin_field(ui, |ui| {
                    let mut v = value;
                    let r = ui.add(
                        egui::DragValue::new(&mut v)
                            .range(range)
                            .speed(speed)
                            .suffix(suffix),
                    );
                    if r.changed() {
                        out = Some(v);
                    }
                });
            });
            out
        };
        if let Some(p) = number(
            ui,
            "Period:",
            period,
            term_core::config::CARET_PERIOD_RANGE,
            0.05,
            " seconds",
        ) {
            edit.period = Some(term_core::config::clamp_caret_period(p));
        }
        if let Some(w) = number(
            ui,
            "Width:",
            width,
            term_core::config::CARET_WIDTH_RANGE,
            0.1,
            " pixels",
        ) {
            edit.width = Some(term_core::config::clamp_caret_width(w));
        }
    });
    // 视觉基线没有页脚说明；灰行理由挂在 hover 上。
    edit
}

/// Caret 页底部说明：这一页现在三项全真，保留常量供测试钉住「不再有灰行」。
const CARET_PAGE_NOTE: &str = "Caret style, blink period and width are stored and apply to new \
     sessions.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_caret_page_admits_period_and_width_are_real_fields_now() {
        // ：Period / Width 已接真。页底若仍写「渲染层常数」
        // 就是反向的谎——用户会以为拖了没用，而它确实会进新会话。
        assert!(
            !CARET_PAGE_NOTE.contains("constants"),
            "接真后不能再说周期与宽度是常数：{CARET_PAGE_NOTE}"
        );
        assert!(
            CARET_PAGE_NOTE.contains("blink period") && CARET_PAGE_NOTE.contains("width"),
            "页底要点名这两项"
        );
        assert!(
            CARET_PAGE_NOTE.contains("new sessions"),
            "三项都是会话属性，必须说清只影响新建会话"
        );
    }

    #[test]
    fn each_field_is_written_on_its_own() {
        // load-modify-save 的 modify 半边：只动被改的那一项。
        // 拖宽度不许把周期带回出厂值，反之亦然。
        let mut prefs = term_profile::prefs::Prefs {
            language: "zh-CN".to_string(),
            caret_style: 2,
            caret_period: 1.0,
            caret_width: 4.0,
            ..Default::default()
        };
        CaretEdit {
            width: Some(2.5),
            ..Default::default()
        }
        .write_into(&mut prefs);
        assert_eq!(prefs.caret_width, 2.5);
        assert_eq!(prefs.caret_period, 1.0, "没动过的周期必须原样保留");
        assert_eq!(prefs.caret_style, 2, "没动过的形状必须原样保留");
        assert_eq!(prefs.language, "zh-CN");

        CaretEdit {
            style: Some(6),
            period: Some(0.25),
            ..Default::default()
        }
        .write_into(&mut prefs);
        assert_eq!(prefs.caret_style, 6);
        assert_eq!(prefs.caret_period, 0.25);
        assert_eq!(prefs.caret_width, 2.5, "上一次写的宽度不许被冲掉");
    }
}
