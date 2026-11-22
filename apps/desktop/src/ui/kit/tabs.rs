//! 页签条与相连的内容面板，函数式原语。
//!
//! 兼容终端的页签不是 `egui::SelectableLabel`（圆角按钮）：三块**相邻**的
//! 方标签，选中块比未选块更暗、与下方内容框**连成一体**，每块首字符带
//! 访问键下划线。OneKeys / Triggers 顶部与 Modem 的 X/Y/Z 都用这一套；
//! 过去两处各画一份，Modem 还退化成了按钮，所以这里收成一份。

use eframe::egui;

fn rgb(hex: u32) -> egui::Color32 {
    egui::Color32::from_rgb((hex >> 16) as u8, ((hex >> 8) & 0xFF) as u8, (hex & 0xFF) as u8)
}

/// 一页签。几何完全由调用方给（宽度可按文字量），配色与下划线只有这一份。
pub(in crate::ui) fn tab(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    label: &str,
    active: bool,
) -> egui::Response {
    tab_with_rect(ui, width, height, label, active).0
}

/// 同 [`tab`]，额外返回页签矩形，供 [`strip`] 传给 [`panel`] 抹掉相连处的顶框。
pub(in crate::ui) fn tab_with_rect(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    label: &str,
    active: bool,
) -> (egui::Response, egui::Rect) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let p = ui.painter_at(rect);
    let (bg, fg) = if active {
        (0x1C1C1C, 0xE6E6E6)
    } else {
        (0x333333, 0xBFBFBF)
    };
    p.rect_filled(rect, egui::Rounding::ZERO, rgb(bg));
    let color = rgb(fg);
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = p.layout_no_wrap(label.to_owned(), font.clone(), color);
    let w = galley.size().x;
    p.galley(rect.center() - galley.size() / 2.0, galley, color);
    // 首字符下划线：等宽字体下 cw = 总宽 / 字符数。
    let cw = w / label.chars().count().max(1) as f32;
    let x0 = rect.center().x - w / 2.0;
    let y = rect.center().y + ui.style().text_styles[&egui::TextStyle::Button].size * 0.62;
    p.line_segment([egui::pos2(x0, y), egui::pos2(x0 + cw, y)], egui::Stroke::new(1.0, color));
    (resp, rect)
}

/// 一条相邻页签的结果：被点的下标 + 选中页签的矩形（给 [`panel`] 抹掉相连处的顶框）。
#[derive(Clone, Copy, Debug)]
pub(in crate::ui) struct StripOutcome {
    pub clicked: Option<usize>,
    pub selected_rect: egui::Rect,
}

/// 一条相邻页签。返回本帧被点的下标（调用方负责把它记进状态/egui memory）。
///
/// `pad` 是每块文字左右的留白；`height` 与下方 [`panel`] 的顶边相接。
pub(in crate::ui) fn strip(
    ui: &mut egui::Ui,
    labels: &[&str],
    selected: usize,
    height: f32,
    pad: f32,
) -> StripOutcome {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let widths: Vec<f32> = labels
        .iter()
        .map(|s| {
            ui.fonts(|f| f.layout_no_wrap((*s).to_owned(), font.clone(), egui::Color32::WHITE))
                .size()
                .x
                + pad * 2.0
        })
        .collect();
    let mut clicked = None;
    let mut selected_rect = egui::Rect::NOTHING;
    ui.horizontal(|ui| {
        for (i, (label, w)) in labels.iter().zip(widths).enumerate() {
            if i > 0 {
                // 抵消 horizontal 自带的 item_spacing，三块必须紧贴。
                ui.add_space(-ui.spacing().item_spacing.x);
            }
            let (resp, rect) = tab_with_rect(ui, w, height, label, i == selected);
            if resp.clicked() {
                clicked = Some(i);
            }
            if i == selected {
                selected_rect = rect;
            }
        }
    });
    StripOutcome { clicked, selected_rect }
}

/// 与 [`strip`] 相连的内容面板：1pt #333 框、10pt 内边距、与页签同宽。
///
/// `merge` 是 [`strip`] 返回的选中页签矩形：把它盖住的那段顶框抹掉，
/// 选中页签才真正与面板连成一片（兼容终端的样子），否则会多出一条横线。
pub(in crate::ui) fn panel(
    ui: &mut egui::Ui,
    merge: egui::Rect,
    min_height: f32,
    add: impl FnOnce(&mut egui::Ui),
) {
    let left = if merge.is_finite() {
        merge.left()
    } else {
        ui.cursor().left()
    };
    let right = ui.clip_rect().right() - 2.0;
    let top = ui.cursor().top();
    // 面板宽度必须与**内容区**同宽，而 `strip` 画完后光标停在页签条右端
    // （页签条比内容区窄）。用 `allocate_ui_at_rect` 从内容区左缘起画，
    // 同时把光标推进交给子 Ui 的 min_rect（不是这个大 max_rect）。
    let area = egui::Rect::from_min_max(
        egui::pos2(left, top),
        egui::pos2(right, top + 4000.0),
    );
    let resp = ui.allocate_new_ui(egui::UiBuilder::new().max_rect(area), |ui| {
        egui::Frame::none()
            .stroke(egui::Stroke::new(1.0, rgb(0x333333)))
            .inner_margin(egui::Margin::symmetric(10.0, 10.0))
            .show(ui, |ui| {
                ui.set_width(right - left - 2.0);
                // 内容区是固定身高的 tab view：金标准里它一直铺到页脚上方，
                // 不随本页字段多少而伸缩（否则三页签切来切去窗口会跳）。
                ui.set_min_height(min_height);
                add(ui);
            })
    });
    if merge.is_finite() {
        let outer = resp.inner.response.rect;
        let cap = egui::Rect::from_min_max(
            egui::pos2(merge.left(), outer.top() - 1.0),
            egui::pos2(merge.right(), outer.top() + 1.5),
        );
        ui.painter_at(cap.expand(1.0))
            .rect_filled(cap, egui::Rounding::ZERO, rgb(0x1C1C1C));
    }
}

#[cfg(test)]
mod tests {
    use super::rgb;

    #[test]
    fn palette_colors_are_the_single_tab_skin() {
        // 页签只吃两档底色：选中与页面同色（与面板连成一片），未选高一档。
        assert_eq!(rgb(0x1C1C1C), egui::Color32::from_rgb(0x1C, 0x1C, 0x1C));
        assert_eq!(rgb(0x333333), egui::Color32::from_rgb(0x33, 0x33, 0x33));
    }
}
