//! OneKeys 对话框的 `Account` / `Sessions` 两页（2026-11-17 按三张视觉基线重排）。
//!
//! # 与旧版的区别
//!
//! 旧版 Account 页是两组 `Expect :` / `Send :`，把会话期的自动应答也做成
//! 方案字段。会话期 expect → send 已删除（ADR-0030 决策 5），这一页现在
//! 只画**连接期真正用到的两项**：`OneKey.username` 与保险库条目
//! `onekey:<name>` 的密码。两组的分组框与实测坐标照旧保留。
//!
//! Sessions 是**整宽** Filter + `Session | OneKey` 两列表格，占满主体高度，
//! 没有左栏列表与工具行。
//!
//! 所有坐标都是视觉基线（@2×）物理像素 ÷2 的实测值，相对**右栏内容原点**；
//! 绘制仍走 `place` 绝对定位，不用 flow——`add_space` 会随字体度量漂。

use super::dialog_chrome::{dark_field, fill_field, place, rgb};
use crate::ui::*;
use term_core::onekey::OneKey;

const PANEL_W: f32 = 420.0;
const BOX_W: f32 = 420.5;
const LABEL_X: f32 = 12.0;
const FIELD_X: f32 = 68.0;
const FIELD_W: f32 = 341.0;
const FIELD_H: f32 = 24.0;

// 各行的 y 相对右栏原点（视觉基线逐行量取，单位逻辑点）。
const USER_TITLE_Y: f32 = 5.5;
const USER_BOX_Y: f32 = 19.5;
const USER_BOX_H: f32 = 77.0;
const USER_FIELD_Y: f32 = 29.5;
const PASS_TITLE_Y: f32 = 106.5;
const PASS_BOX_Y: f32 = 122.5;
const PASS_BOX_H: f32 = 77.0;
const PASS_FIELD_Y: f32 = 132.5;

/// 首字符带下划线的短标签（视觉基线的键盘加速器：U / P）。
fn access_label(ui: &mut egui::Ui, at: egui::Pos2, text: &str, hex: u32) {
    place(ui, egui::Rect::from_min_size(at, egui::vec2(96.0, 14.0)), |ui| {
        let font = egui::TextStyle::Button.resolve(ui.style());
        let color = rgb(hex);
        let mut job = egui::text::LayoutJob::default();
        let (first, rest) = match text.char_indices().nth(1) {
            Some((i, _)) => (&text[..i], &text[i..]),
            None => (text, ""),
        };
        let plain = egui::TextFormat::simple(font.clone(), color);
        job.append(
            first,
            0.0,
            egui::TextFormat {
                underline: egui::Stroke::new(1.0, color),
                ..plain.clone()
            },
        );
        job.append(rest, 0.0, plain);
        ui.add(egui::Label::new(job));
    });
}

/// 分组框：深底、1px `#2E2E2E` 描边、3 点圆角（视觉基线实测量）。
fn group_box(ui: &egui::Ui, o: egui::Pos2, y: f32, h: f32) {
    let rect = egui::Rect::from_min_size(o + egui::vec2(0.0, y), egui::vec2(BOX_W, h));
    let p = ui.painter();
    // 两层描边：视觉基线外圈 #2E2E2E、内圈 #1B1B1B。矩形落在整数像素上时
    // 1px 描边不产生混色，直接按色号落。
    p.rect_stroke(
        rect,
        egui::Rounding::same(3.0),
        egui::Stroke::new(1.0, rgb(0x2E2E2E)),
    );
    p.rect_stroke(
        rect.shrink(1.0),
        egui::Rounding::same(2.0),
        egui::Stroke::new(1.0, rgb(0x1B1B1B)),
    );
}

/// 密码框右端的眼睛：椭圆轮廓 + 瞳孔，几何画出，不依赖 emoji 字形。
fn eye_button(ui: &mut egui::Ui, center: egui::Pos2, visible: bool) -> bool {
    let rect = egui::Rect::from_center_size(center, egui::vec2(16.0, 14.0));
    let resp = ui.allocate_rect(rect, egui::Sense::click());
    let p = ui.painter_at(rect);
    let c = rgb(0xBBBBBB);
    let st = egui::Stroke::new(1.0, c);
    p.circle_stroke(center, 5.0, st);
    p.circle_filled(center, if visible { 2.6 } else { 1.8 }, c);
    resp.clicked()
}

/// 密码框：与 `dark_field` 同底同内缩，只多一层掩码开关。
fn secret_field(ui: &mut egui::Ui, rect: egui::Rect, value: &mut String, visible: bool) {
    fill_field(ui, rect, 0x333333);
    place(ui, rect.shrink2(egui::vec2(3.0, 1.0)), |ui| {
        ui.add(
            egui::TextEdit::singleline(value)
                .password(!visible)
                .desired_width(f32::INFINITY)
                .frame(false),
        );
    });
}

/// 一组的四个实测 y 值，按值传，避免给绘制函数塞一长串 f32。
#[derive(Clone, Copy)]
struct GroupLayout {
    title_y: f32,
    box_y: f32,
    box_h: f32,
    field_y: f32,
}

/// 画一组凭据输入框：`secret` 为 `Some` 时用掩码字段并带眼睛开关。
fn credential_group(
    ui: &mut egui::Ui,
    o: egui::Pos2,
    title: &str,
    label: &str,
    l: GroupLayout,
    value: &mut String,
    secret: Option<&mut bool>,
) {
    access_label(ui, o + egui::vec2(3.5, l.title_y), title, 0xBBBBBB);
    group_box(ui, o, l.box_y, l.box_h);

    access_label(
        ui,
        o + egui::vec2(LABEL_X, l.field_y + 4.0),
        label,
        0xC8C8C8,
    );
    let rect = egui::Rect::from_min_size(
        o + egui::vec2(FIELD_X, l.field_y),
        egui::vec2(FIELD_W, FIELD_H),
    );
    match secret {
        Some(show) => {
            secret_field(ui, rect, value, *show);
            if eye_button(ui, egui::pos2(rect.right() - 16.0, rect.center().y), *show) {
                *show = !*show;
            }
        }
        None => dark_field(ui, rect, value),
    }
}

/// Account 标签：直接画 `OneKey.username` 与保险库条目 `onekey:<name>` 的密码。
/// 会话期的 `expect → send` 已删除，这里只维护连接期用到的这两项。
pub(in crate::ui) fn account_editor(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    key: &mut OneKey,
    show_password: &mut bool,
) {
    let o = ui.cursor().min;
    credential_group(
        ui, o, "Username", "Username :",
        GroupLayout {
            title_y: USER_TITLE_Y,
            box_y: USER_BOX_Y,
            box_h: USER_BOX_H,
            field_y: USER_FIELD_Y,
        },
        &mut key.username, None,
    );
    // 密码只在保险库里：显示时临时取一份，改动按方案名记草稿，
    // 保存时走与 SSH 页同一条落盘路径（`vault.rs::save_onekeys`）。
    let mut password = win.onekey_password(&key.name);
    let before_edit = password.clone();
    credential_group(
        ui, o, "Password", "Password :",
        GroupLayout {
            title_y: PASS_TITLE_Y,
            box_y: PASS_BOX_Y,
            box_h: PASS_BOX_H,
            field_y: PASS_FIELD_Y,
        },
        &mut password, Some(show_password),
    );
    if password != before_edit {
        win.onekeys_password_draft = Some((key.name.clone(), password));
    }
    // 绝对定位不占流式高度。
    ui.advance_cursor_after_rect(egui::Rect::from_min_size(
        o,
        egui::vec2(PANEL_W, PASS_BOX_Y + PASS_BOX_H),
    ));
}

// ── Sessions 页 ──────────────────────────────────────────────────────

const SESSION_COL_W: f32 = 464.0;
const FILTER_H: f32 = 20.0;
const HEAD_H: f32 = 24.0;
const ROW_H: f32 = 24.0;
const TABLE_TOP: f32 = 36.0;

/// 视觉基线里的会话图标：内置 shell 是菱形，分组是齿轮状，其余是小方块。
/// 全部几何/字形画，颜色 `#BBBBBB`，与表格文字同族。
fn session_glyph(p: &egui::Painter, center: egui::Pos2, name: &str, row: usize) {
    // 参考图 Sessions 表按会话/外壳类型着色：shell 子项用粉/橙/蓝，普通会话
    // 用八色轮转盘。没有领域色字段时按行号取色，纯视觉、不假装能改颜色。
    const ROW_COLORS: [u32; 8] = [
        0xFF7A2F, 0xE02020, 0xA020E0, 0x20A020, 0xE02020, 0xA020E0, 0x2E8BEF, 0xE08080,
    ];
    let shell = name.rsplit('/').next().unwrap_or(name);
    let c = match shell {
        "csh" | "tcsh" | "zsh" => rgb(0xFF008E),
        "dash" | "ksh" => rgb(0xFF6A00),
        "sh" => rgb(0x2E8BEF),
        "bash" => rgb(0xFFB000),
        _ => rgb(ROW_COLORS[row % ROW_COLORS.len()]),
    };
    if name.contains('/') {
        p.text(
            center,
            egui::Align2::CENTER_CENTER,
            "✳",
            crate::ui::type_scale::small(),
            c,
        );
    } else if name.contains("debug") {
        p.rect_filled(
            egui::Rect::from_center_size(center, egui::vec2(6.0, 6.0)),
            egui::Rounding::ZERO,
            c,
        );
    } else {
        p.add(egui::Shape::convex_polygon(
            vec![
                center + egui::vec2(0.0, -5.0),
                center + egui::vec2(5.0, 0.0),
                center + egui::vec2(0.0, 5.0),
                center + egui::vec2(-5.0, 0.0),
            ],
            c,
            egui::Stroke::NONE,
        ));
    }
}

/// Sessions 标签：整宽 Filter + `Session | OneKey` 两列表格。
/// 点一行即可把当前方案分配给该会话（再点一次撤销）；会话表单里的下拉
/// 读的是同一个 `SessionConfig.onekey`，所以这里不是双写，而是同一份数据的
/// 第二个入口。
pub(in crate::ui) fn sessions_editor(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    key: &OneKey,
    filter: &mut String,
    focus_filter: bool,
) {
    let o = ui.cursor().min;
    // Painter 廉价克隆：下面 TextEdit 要借 &mut ui，p 不能一直占着不可变借。
    let p = ui.painter().clone();

    // Filter：整宽输入框，视觉基线 y 27..44（相对主体原点 0..18）。
    // 底色 #1C1C1C；聚焦时 #007CD2 描边（参考图 Sessions 图就是聚焦态）。
    let fr = egui::Rect::from_min_size(o, egui::vec2(PANEL_W + 264.0, FILTER_H));
    p.rect_filled(fr, egui::Rounding::ZERO, rgb(0x1C1C1C));
    let focused = focus_filter;
    place(ui, fr.shrink2(egui::vec2(6.0, 1.0)), |ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(filter)
                .hint_text("Filter")
                .desired_width(f32::INFINITY)
                .frame(false),
        );
        if focused && !resp.has_focus() {
            resp.request_focus();
        }
    });
    p.rect_stroke(
        fr,
        egui::Rounding::ZERO,
        egui::Stroke::new(
            1.0,
            if focused { rgb(0x007CD2) } else { rgb(0x444444) },
        ),
    );

    let table = egui::Rect::from_min_size(
        o + egui::vec2(0.0, TABLE_TOP),
        egui::vec2(PANEL_W + 264.0, 551.0),
    );
    let head = egui::Rect::from_min_size(table.min, egui::vec2(table.width(), HEAD_H));
    p.rect_filled(head, egui::Rounding::ZERO, rgb(0x282828));
    p.rect_stroke(table, egui::Rounding::ZERO, egui::Stroke::new(1.0, rgb(0x444444)));
    p.line_segment(
        [
            egui::pos2(table.left(), head.bottom()),
            egui::pos2(table.right(), head.bottom()),
        ],
        egui::Stroke::new(1.0, rgb(0x444444)),
    );
    let divider = table.left() + SESSION_COL_W;
    p.line_segment(
        [egui::pos2(divider, table.top()), egui::pos2(divider, table.bottom())],
        egui::Stroke::new(1.0, rgb(0x444444)),
    );
    let font = egui::TextStyle::Button.resolve(ui.style());
    p.text(
        head.min + egui::vec2(10.0, HEAD_H / 2.0),
        egui::Align2::LEFT_CENTER,
        "Session",
        font.clone(),
        rgb(0xBBBBBB),
    );
    p.text(
        egui::pos2(divider - 12.0, head.center().y),
        egui::Align2::RIGHT_CENTER,
        "▲",
        font.clone(),
        rgb(0x888888),
    );
    p.text(
        egui::pos2(divider + 10.0, head.center().y),
        egui::Align2::LEFT_CENTER,
        "OneKey",
        font.clone(),
        rgb(0xBBBBBB),
    );

    let needle = filter.trim().to_lowercase();
    let assigned: Vec<(usize, String, String)> = win
        .sessions
        .iter()
        .enumerate()
        .filter(|(_, c)| needle.is_empty() || c.name.to_lowercase().contains(&needle))
        .map(|(idx, c)| (idx, c.name.clone(), c.onekey.clone()))
        .collect();
    for (i, (session_idx, name, onekey)) in assigned.iter().enumerate() {
        let ry = head.bottom() + i as f32 * ROW_H;
        if ry + ROW_H > table.bottom() {
            break;
        }
        let row = egui::Rect::from_min_size(
            egui::pos2(table.left() + 1.0, ry),
            egui::vec2(table.width() - 2.0, ROW_H),
        );
        if i == 0 {
            p.rect_filled(row, egui::Rounding::ZERO, rgb(0x2E2E2E));
        }
        let cy = row.center().y;
        session_glyph(&p, egui::pos2(row.left() + 20.0, cy), name, i);
        p.text(
            egui::pos2(row.left() + 36.0, cy),
            egui::Align2::LEFT_CENTER,
            name,
            font.clone(),
            rgb(0xC8C8C8),
        );
        let value = if onekey.is_empty() { "None" } else { onekey.as_str() };
        p.text(
            egui::pos2(divider + 10.0, cy),
            egui::Align2::LEFT_CENTER,
            value,
            font.clone(),
            rgb(0xC8C8C8),
        );
        if ui
            .interact(row, egui::Id::new(("onekey_assign", *session_idx)), egui::Sense::click())
            .on_hover_text("Click to assign / unassign this OneKey scheme")
            .clicked()
        {
            if let Some(config) = win.sessions.get_mut(*session_idx) {
                config.onekey = if onekey == &key.name {
                    "None".to_string()
                } else {
                    key.name.clone()
                };
                win.save_sessions();
            }
        }
    }
    ui.advance_cursor_after_rect(egui::Rect::from_min_size(o, egui::vec2(PANEL_W + 264.0, TABLE_TOP + 551.0)));
}
