//! X/Y/Z Modem 页（金标准 `02-session-dialog/serial/02..05`）。
//! 独立成文件只为保住 `session_pages.rs` 的 500 行上限。

use crate::ui::kit::field::{
    browse_field, choice_row, choice_row_resp, underlined_job, ChoiceKind, UnderlineStyle,
};
use crate::ui::kit::form::field_box;
use crate::ui::kit::spec::{FIELD_TO_ROW, LABEL_TO_FIELD};
use crate::ui::kit::tabs::{panel, strip};
use crate::ui::*;

/// 16 `X/Y/Z Modem`：视觉基线 `02-session-dialog/serial/02-serial-xymodem.png`。
///
/// 布局逐字照原图：**标签独占一行、输入框在下一行且占满整宽**（不是本文件
/// 其它页的两列网格）。顺序：说明 → 协议下拉(Z Modem) → Upload directory +
/// 路径框(带 `| ...` 浏览按钮) → Download directory + 框 → 默认勾选 →
/// X/Y/Z 三页签 → 选中页的字段。`Upload command:` 标签在上、命令框在下占满宽；
/// `Packet size:` 是标签在左、两个单选同一行。
///
/// 三个页签各有 补采的视觉基线真值（`03/04/05-serial-*modem-tab.png`）：
/// X=`rx`+128(XModem) 选中；Y=`rb`+1024(YModem-1K) 选中；Z=`rz` 且**没有**包长行。
///
/// 2026-11-22 起命令与包长是**真字段**（`SessionConfig::modem_commands` /
/// `modem_large_packets`），三页签各自可编辑；页面不再有「灰显说明」那段噪声。
/// 页签切换存 egui memory，不进领域状态；点得动且真换内容。
///
/// 所有几何都来自 `kit::field` / `kit::tabs` / `kit::form` 原语：通栏浏览框、
/// 相邻页签条、与选中页签相连的内容面板，不再由本页手写裸 egui 控件。
pub(in crate::ui) fn modem_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {

    const TABS: [&str; 3] = ["X Modem", "Y Modem", "Z Modem"];
    const TAB_ID: &str = "new_session_modem_tab";
    let tab: usize = ui.memory(|m| m.data.get_temp::<usize>(egui::Id::new(TAB_ID)).unwrap_or(0));
    let hint = "/Users/unique/Downloads";
    let mut protocol = win.new_session.modem_protocol;
    let mut use_default = win.new_session.modem_use_default;
    let mut up_dir = win.new_session.modem_upload_dir.clone();
    let mut down_dir = win.new_session.modem_download_dir.clone();
    let mut commands = win.new_session.modem_commands.clone();
    let large = win.new_session.modem_large_packets;

    // 本页是「标签在上、框在下」的堆叠布局（与同文件其它页的两列网格不同），
    // 节奏必须按金标准逐段给：标签→框 7pt、框→下一标签 9pt。用 scope 把
    // item_spacing 的修改关在本页内，不污染同一弹窗的其它页。
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = FIELD_TO_ROW;

        // 说明行 + 通栏协议下拉（视觉基线下拉框占满内容区宽度）。
        ui.small("Specify the protocol when dragging and dropping files:");
        egui::ComboBox::from_id_salt("modem_protocol")
            .selected_text(TABS[protocol.min(2)])
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for (i, name) in TABS.iter().enumerate() {
                    if ui.selectable_label(protocol == i, *name).clicked() {
                        protocol = i;
                    }
                }
            });

        // 段间留白：9(item_spacing) + 5 = 14px/2 = 7pt，与金标准
        // 「下拉框底 → Upload directory 标签顶」一致。
        ui.add_space(5.0);

        // 目录框：标签独占一行，框通栏、右端内嵌 `| ...` 浏览槽。
        ui.label(egui::WidgetText::from(underlined_job(
            ui, "Upload directory:", "Upload", UnderlineStyle::Dialog,
        )));
        ui.add_space(LABEL_TO_FIELD - FIELD_TO_ROW);
        let (_, browse) = browse_field(ui, &mut up_dir, hint, "...", 24.0, 40.0);
        if browse
            .on_hover_text("Browse for the upload directory")
            .clicked()
        {
            win.open_path_picker(crate::ui::path_picker::PathTarget::ModemUploadDir, &up_dir);
        }

        ui.label(egui::WidgetText::from(underlined_job(
            ui, "Download directory:", "Download", UnderlineStyle::Dialog,
        )));
        ui.add_space(LABEL_TO_FIELD - FIELD_TO_ROW);
        let (_, browse) = browse_field(ui, &mut down_dir, hint, "...", 24.0, 40.0);
        if browse
            .on_hover_text("Browse for the download directory")
            .clicked()
        {
            win.open_path_picker(crate::ui::path_picker::PathTarget::ModemDownloadDir, &down_dir);
        }

        choice_row(
            ui,
            ChoiceKind::Checkbox,
            "Use this as the default and do not ask again",
            "Use",
            &mut use_default,
            true,
        );

        // 勾选行 → 页签条：行距 9pt 已由 item_spacing 给出，不再另加空白。
        let outcome = strip(ui, &TABS, tab, 25.0, 8.0);
        if let Some(next) = outcome.clicked {
            ui.memory_mut(|m| m.data.insert_temp(egui::Id::new(TAB_ID), next));
        }
        // 页签与面板必须**紧贴**：horizontal 之后 egui 还会按 item_spacing.y
        // 推一次光标，这里把它抵消，选中页签才真的与面板顶框连成一条。
        ui.add_space(-FIELD_TO_ROW);

        // 页签下方的内容面板：选中页签与面板顶框相连。
        // 金标准 `02-serial-xymodem.png` @2x：面板 504..1038 → 267 逻辑点。
        panel(ui, outcome.selected_rect, 267.0, |ui| {
            ui.spacing_mut().item_spacing.y = FIELD_TO_ROW;
            let i = outcome.clicked.unwrap_or(tab).min(2);

            ui.label(egui::WidgetText::from(underlined_job(
                ui, "Upload command:", "Upload", UnderlineStyle::Dialog,
            )));
            ui.add_space(LABEL_TO_FIELD - FIELD_TO_ROW);
            field_box(ui, |ui| {
                ui.add_sized(
                    egui::vec2(ui.available_width(), 24.0),
                    egui::TextEdit::singleline(&mut commands[i]).frame(false),
                );
            });

            // 视觉基线 Z 页签没有 Packet size 行，不臆造。
            //
            // X/Y 的包长虽然存进了 `SessionConfig::modem_large_packets`，但
            // 传输层（`term-xfer` 的 X/Y/Z Modem）从来不读它——勾了也没有
            // 任何效果。按 ADR-0015「未接线命令显式置灰」：保留金标准的两
            // 个选项与选中态，但不可点，悬停给出理由，不假装它已经生效。
            if i < 2 {
                ui.horizontal(|ui| {
                    ui.label("Packet size:");
                    let proto = if i == 0 { "XModem" } else { "YModem" };
                    let mut small = !large[i];
                    let mut big = large[i];
                    let hint = "包长尚未接线到 X/Y Modem 传输（ADR-0015：未接线项显式置灰）";
                    choice_row_resp(
                        ui,
                        ChoiceKind::Radio,
                        &format!("128 ({proto})"),
                        proto,
                        &mut small,
                        false,
                        UnderlineStyle::Dialog,
                    )
                    .on_hover_text(hint);
                    choice_row_resp(
                        ui,
                        ChoiceKind::Radio,
                        &format!("1024 ({proto}-1K)"),
                        proto,
                        &mut big,
                        false,
                        UnderlineStyle::Dialog,
                    )
                    .on_hover_text(hint);
                });
            }
        });
    });

    let cfg = &mut win.new_session;
    cfg.modem_protocol = protocol;
    cfg.modem_use_default = use_default;
    cfg.modem_upload_dir = up_dir;
    cfg.modem_download_dir = down_dir;
    cfg.modem_commands = commands;
    cfg.modem_large_packets = large;
}
