//! 终端行过滤栏：`grep` / `grep -v` 的行级过滤。
//!
//! 与 `find_bar.rs` 同三条纪律：状态只读、缓冲受控、计数现算。
//! 这里只收用户输入并派发 `Action::Filter`；真正的行隐藏发生在
//! `ui/terminal/setup.rs` 的最后一步 `ScreenImage::drop_rows`。
//!
//! 整栏用 egui 布局而非逐像素手绘：它是新增构件，不受 Find 栏那套视觉
//! 基线的绝对坐标约束。颜色仍全部来自主题令牌，不在本文件硬编码。

use crate::ui::tr::t;
use eframe::egui;
use term_app::filter::{self, FilterUiCommand};
use term_app::i18n::Lang;
use term_app::state::{FilterMode, FilterState};
use term_app::vocab::{Action, FilterAction};
use term_core::theme::CHROME_SCALE;

/// 过滤栏整条高度（与 Find 栏一致）。
pub const FILTER_BAR_H: f32 = 43.0;

/// 过滤栏零内边距框：高度由 `TopBottomPanel::exact_height` 精确控制。
pub fn filter_bar_frame() -> egui::Frame {
    egui::Frame::none().fill(crate::theme::chrome(CHROME_SCALE.dialog))
}

/// 画过滤栏。`kept` / `hidden` 由调用方每帧从 `(状态, 事实窗口)` 现算。
pub fn draw_filter_bar(
    ui: &mut egui::Ui,
    state: &FilterState,
    input: &mut String,
    kept: usize,
    hidden: usize,
    lang: Lang,
    dispatch: &mut dyn FnMut(Action),
) {
    if !state.visible {
        return;
    }

    let regex_error = if state.regex && !state.query.is_empty() {
        filter::compile(state).err()
    } else {
        None
    };

    ui.horizontal_centered(|ui| {
        ui.label(t(lang, "Filter:"));

        let border = if regex_error.is_some() {
            egui::Color32::from_rgb(0xE0, 0x6C, 0x6C)
        } else {
            crate::theme::chrome(CHROME_SCALE.border)
        };
        let response = ui.add(
            egui::TextEdit::singleline(input)
                .desired_width(320.0)
                .hint_text(t(lang, "Filter hint")),
        );
        // 输入框的描边：egui 自身样式没有单色边框入口，手动画一圈。
        ui.painter().rect_stroke(
            response.rect.expand(1.0),
            0.0,
            egui::Stroke::new(1.0, border),
        );
        if response.changed() {
            dispatch(Action::Filter(FilterAction::SetQuery(input.clone())));
        }
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            dispatch(Action::Filter(FilterAction::Close));
        }

        let mode_label = match state.mode {
            FilterMode::KeepMatching => t(lang, "grep"),
            FilterMode::DropMatching => t(lang, "grep -v"),
        };
        if ui.button(mode_label).clicked() {
            dispatch(filter::ui_command_action(FilterUiCommand::ToggleMode));
        }
        if ui
            .selectable_label(state.case_sensitive, t(lang, "Aa"))
            .clicked()
        {
            dispatch(Action::Filter(FilterAction::ToggleCase));
        }
        if ui
            .selectable_label(state.regex, t(lang, "Regular expression"))
            .clicked()
        {
            dispatch(Action::Filter(FilterAction::ToggleRegex));
        }

        ui.label(format!("{kept}/{hidden}"));
        if ui.button(t(lang, "Close")).clicked() {
            dispatch(Action::Filter(FilterAction::Close));
        }

        if let Some(err) = regex_error.as_deref() {
            ui.label(egui::RichText::new(err).color(egui::Color32::from_rgb(0xE0, 0x6C, 0x6C)));
        }
    });
}
