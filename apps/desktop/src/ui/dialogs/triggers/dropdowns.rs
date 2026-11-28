//! 各类下拉浮层：Action / Line Style / Color / Send Condition / Captures。
//!
//! 浮层画在 `Order::Foreground` 的 `Area` 上，锚点由打开它的那次点击记录在
//! [`View::anchor`] 里——这样不管面板怎么重排，菜单总贴着触发它的控件。
//!
//! # 两套皮肤（视觉基线实测，别混用）
//!
//! 兼容终端是 Qt，两类控件长得不一样：
//!
//! | | 组合框下拉（`12` / `16`） | 颜色菜单（`14`） |
//! |---|---|---|
//! | 背景 | `#1c1c1c` | `#333333` |
//! | 边框 | `#2e2e2e` | `#535353` |
//! | 行高 | 20 点 | 28 点 |
//! | 左边距 | 6 点 | 32 点（给勾选指示位留白） |
//! | 文字 | `#bbbbbb` | `#bbbbbb` + 助记符下划线 |
//! | 分隔线 | 无 | `Color…` 之后、`Empty` 之前各一条 `#555555` |
//!
//! 颜色菜单里**没有色块**——视觉基线 `14` 通篇只有文字，颜色靠名字与助记符辨认。

use super::fields::{rgb, ul_text, FS};
use super::{ColorSlot, Open, Picker, View};
use crate::ui::view::trigger_pages_spec as ps;
use crate::ui::view::trigger_spec::color as c;
use crate::ui::*;
use crate::ui::dialogs::triggers::model::{ActionKind, ColorChoice, LineStyle, SendCondition};

/// 菜单里的一行。
enum Item {
    /// 文本 + 助记符下划线所在的**字符**下标。
    Text(String, Option<usize>),
    /// 分隔线（仅颜色菜单用）。
    Sep,
}

/// 菜单皮肤的几何与配色。
struct Skin {
    bg: u32,
    border: u32,
    row_h: f32,
    pad_left: f32,
    /// 文本最宽处之外再留的右边距。
    pad_right: f32,
    /// 是否按内容定宽（`false` 时至少与锚点同宽）。
    fit_content: bool,
}

const LIST: Skin = Skin {
    bg: 0x1c1c1c,
    border: 0x2e2e2e,
    row_h: 20.0,
    pad_left: 8.5,
    pad_right: 10.0,
    fit_content: false,
};

const MENU: Skin = Skin {
    bg: 0x333333,
    border: 0x535353,
    row_h: 28.0,
    pad_left: 32.0,
    pad_right: 10.0,
    fit_content: true,
};

/// 分隔线高。
const SEP_H: f32 = 1.0;

/// 画当前打开的浮层。
pub(in crate::ui) fn show(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    view: &mut View,
) {
    let Some(open) = view.open else { return };
    let Some(anchor) = view.anchor else {
        view.dismiss();
        return;
    };
    let ctx = ui.ctx().clone();
    let idx = view.sel.filter(|i| *i < win.triggers.len());

    // 声音库是 47 项的可滚动列表（视觉基线 `22`），主 `draw` 的固定行式排布放不下，
    // 单独走一条滚动路径。
    if open == Open::SoundName {
        let cur = idx
            .map(|i| win.triggers[i].config.action.sound.clone())
            .unwrap_or_default();
        let (picked, menu) = sound_menu(&ctx, anchor, &cur);
        if let Some(name) = picked {
            if let Some(i) = idx {
                win.triggers[i].config.action.sound = name.to_owned();
            }
            view.dismiss();
            return;
        }
        if ctx.input(|i| i.pointer.any_click())
            && !ctx.pointer_interact_pos().is_some_and(|p| menu.contains(p) || anchor.contains(p))
        {
            view.dismiss();
        }
        return;
    }

    let color_menu = matches!(
        open,
        Open::MarkColor | Open::TextColor | Open::BgColor | Open::LineColor
    );
    let skin = if color_menu { &MENU } else { &LIST };
    let items = items_for(open);

    // `Captures` 是右对齐的小按钮（视觉基线 `17`）：菜单按内容定宽并贴右缘，
    // 否则 150 点的最小宽度会把菜单顶出对话框。
    let right_aligned = open == Open::Captures;
    // 视觉基线 `12` 里 "Solid" 那一行是浅一档的底：菜单会标出**当前取值**，
    // 不只是悬停行。
    let current = current_row(win, view, idx, open);
    let (picked, menu) = draw(&ctx, open_salt(open), anchor, &items, skin, right_aligned, current);
    let pointer = ctx.pointer_interact_pos();
    let clicked_any = ctx.input(|i| i.pointer.any_click());

    if let Some(row) = picked {
        // 行号 → 逻辑项下标（分隔线不占项）。
        let logical = items[..row].iter().filter(|i| matches!(i, Item::Text(..))).count();
        apply(win, view, idx, open, logical);
        view.dismiss();
        return;
    }
    if clicked_any {
        let inside = pointer.is_some_and(|p| menu.contains(p) || anchor.contains(p));
        if !inside {
            view.dismiss();
        }
    }
}

/// 某个浮层的菜单项。
fn items_for(open: Open) -> Vec<Item> {
    let text = |s: &str| Item::Text(s.to_owned(), None);
    match open {
        Open::Action => ActionKind::ALL.iter().map(|k| text(k.label())).collect(),
        Open::LineStyle => LineStyle::ALL.iter().map(|k| text(k.label())).collect(),
        Open::SendCondition => SendCondition::ALL.iter().map(|k| text(k.label())).collect(),
        // 视觉基线 `25`：`+` 下拉只有两项。
        Open::AddMenu => ps::custom_menu::ADD_ITEMS.iter().map(|s| text(s)).collect(),
        // 声音库走独立的滚动菜单（见 [`sound_menu`]），这里给空表占位。
        Open::SoundName => Vec::new(),
        // `${1}`~`${3}` 不在视觉基线 `17` 里，只保留 `${match}`；多出来的会被当成
        // 不存在的捕获组，选了也取不到值。
        Open::Captures => vec![text("${match}")],
        Open::MarkColor | Open::TextColor | Open::BgColor | Open::LineColor => {
            let mut v = vec![Item::Text("Color…".to_owned(), Some(0)), Item::Sep];
            for name in c::ANSI_LABELS.iter() {
                // `Ansi X` 的助记符是 X 首字母（下标 5）；`Ansi Bright X` 是 12。
                let at = if name.starts_with("Ansi Bright ") { 12 } else { 5 };
                v.push(Item::Text((*name).to_owned(), Some(at)));
            }
            v.push(Item::Sep);
            v.push(Item::Text("Empty".to_owned(), Some(0)));
            v
        }
    }
}

/// 当前取值在菜单里的**逻辑项下标**（不含分隔线）。
fn current_row(
    win: &TerminalWindow,
    view: &View,
    idx: Option<usize>,
    open: Open,
) -> Option<usize> {
    let i = idx?;
    let cfg = &win.triggers[i].config;
    match open {
        Open::Action => ActionKind::ALL.iter().position(|k| *k == cfg.action.kind),
        Open::LineStyle => {
            let cur = if view.hl_dialog {
                cfg.mark.line_style
            } else {
                cfg.mark.mark_style
            };
            LineStyle::ALL.iter().position(|k| *k == cur)
        }
        Open::SendCondition => SendCondition::ALL.iter().position(|k| *k == cfg.send.condition),
        Open::Captures => {
            if cfg.send.capture == "${match}" {
                Some(0)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn open_salt(open: Open) -> &'static str {
    match open {
        Open::Action => "trig_menu_action",
        Open::LineStyle => "trig_menu_style",
        Open::MarkColor => "trig_menu_mark",
        Open::TextColor => "trig_menu_tcol",
        Open::BgColor => "trig_menu_bg",
        Open::LineColor => "trig_menu_lcol",
        Open::SendCondition => "trig_menu_cond",
        Open::Captures => "trig_menu_cap",
        Open::SoundName => "trig_menu_sound",
        Open::AddMenu => "trig_menu_add",
    }
}

/// 把选择写回规则（或打开取色器）。
fn apply(
    win: &mut TerminalWindow,
    view: &mut View,
    idx: Option<usize>,
    open: Open,
    picked: usize,
) {
    let Some(i) = idx else {
        // 没有选中项时只处理「打开取色器」这类不需要落库的动作。
        if matches!(open, Open::MarkColor | Open::TextColor | Open::BgColor | Open::LineColor)
            && picked == 0
        {
            view.slot = slot_of(open);
            view.picker = Some(Default::default());
        }
        return;
    };
    let rule = &mut win.triggers[i];
    match open {
        Open::Action => {
            if let Some(k) = ActionKind::ALL.get(picked) {
                rule.config.action.kind = *k;
            }
        }
        Open::LineStyle => {
            if let Some(k) = LineStyle::ALL.get(picked) {
                // 主面板改 `mark_style`，Highlight 弹窗里的 Line Style 用
                // `line_style`——两者共用同一个菜单，按当前弹窗分流。
                if view.hl_dialog {
                    rule.config.mark.line_style = *k;
                } else {
                    rule.config.mark.mark_style = *k;
                }
            }
        }
        Open::SendCondition => {
            if let Some(k) = SendCondition::ALL.get(picked) {
                rule.config.send.condition = *k;
            }
        }
        Open::AddMenu => super::page_custom_menu::add_item(rule, picked == 1),
        // 声音库在 `show` 里提前分流，不会走到这里。
        Open::SoundName => {}
        Open::Captures => {
            rule.config.send.capture = "${match}".to_owned();
        }
        Open::MarkColor | Open::TextColor | Open::BgColor | Open::LineColor => {
            let choice = if picked == 0 {
                // 第一项调取色器：以该槽位的当前色为起点，不再恒从红色重来。
                view.slot = slot_of(open);
                let current = match open {
                    Open::MarkColor => &rule.config.mark.mark_color,
                    Open::TextColor => &rule.config.mark.text_color,
                    Open::BgColor => &rule.config.mark.background_color,
                    _ => &rule.config.mark.line_color,
                };
                view.picker = Some(Picker::from_choice(current));
                return;
            } else if picked <= 16 {
                ColorChoice::Ansi(c::ANSI_LABELS[picked - 1].to_owned())
            } else {
                ColorChoice::Empty
            };
            match open {
                Open::MarkColor => rule.config.mark.mark_color = choice,
                Open::TextColor => rule.config.mark.text_color = choice,
                Open::BgColor => rule.config.mark.background_color = choice,
                _ => rule.config.mark.line_color = choice,
            }
        }
    }
    super::sync_actions(rule);
}

fn slot_of(open: Open) -> ColorSlot {
    match open {
        Open::MarkColor => ColorSlot::Mark,
        Open::TextColor => ColorSlot::Text,
        Open::BgColor => ColorSlot::Bg,
        _ => ColorSlot::Line,
    }
}

/// 画菜单本体，返回 (被点的行下标, 菜单矩形)。
fn draw(
    ctx: &egui::Context,
    id: &'static str,
    anchor: egui::Rect,
    items: &[Item],
    skin: &Skin,
    right_aligned: bool,
    current: Option<usize>,
) -> (Option<usize>, egui::Rect) {
    let rows = items.iter().filter(|i| matches!(i, Item::Text(..))).count() as f32;
    let seps = items.len() as f32 - rows;
    let h = rows * skin.row_h + seps * SEP_H + 2.0;

    let widest = items
        .iter()
        .filter_map(|i| match i {
            Item::Text(s, _) => Some(s.chars().count() as f32 * FS * 0.602),
            Item::Sep => None,
        })
        .fold(0.0, f32::max);
    let content_w = widest + skin.pad_left + skin.pad_right;
    let w = if right_aligned {
        content_w.max(80.0)
    } else if skin.fit_content {
        content_w.max(120.0)
    } else {
        content_w.max(anchor.width()).max(150.0)
    };

    let mut y = anchor.bottom() + 1.0;
    let screen = ctx.screen_rect();
    if y + h > screen.bottom() - 6.0 {
        y = anchor.top() - h - 1.0;
    }
    // 比窗口还高的菜单（颜色菜单 18 行 ≈ 506 点）只能贴顶，否则会画到窗口外。
    let y = y.max(screen.top() + 4.0);
    let x = if right_aligned {
        anchor.right() - w
    } else {
        anchor.left()
    };
    let r = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h));
    let mut picked = None;
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Foreground)
        .fixed_pos(r.min)
        .fade_in(false)
        .show(ctx, |ui| {
            ui.set_min_size(r.size());
            let p = ui.painter().clone();
            p.rect_filled(r, egui::Rounding::ZERO, rgb(skin.bg));
            p.rect_stroke(
                r,
                egui::Rounding::ZERO,
                egui::Stroke::new(1.0, rgb(skin.border)),
            );
            let mut top = r.top() + 1.0;
            let mut logical = 0usize;
            for (i, it) in items.iter().enumerate() {
                match it {
                    Item::Sep => {
                        p.line_segment(
                            [
                                egui::pos2(r.left() + 1.0, top + 0.5),
                                egui::pos2(r.right() - 1.0, top + 0.5),
                            ],
                            egui::Stroke::new(SEP_H, rgb(0x555555)),
                        );
                        top += SEP_H;
                    }
                    Item::Text(s, ul) => {
                        let row = egui::Rect::from_min_size(
                            egui::pos2(r.left() + 1.0, top),
                            egui::vec2(r.width() - 2.0, skin.row_h),
                        );
                        let resp = ui.allocate_rect(row, egui::Sense::click());
                        if current == Some(logical) {
                            p.rect_filled(row, egui::Rounding::ZERO, rgb(c::SEL));
                        }
                        logical += 1;
                        // 悬停行是 Qt 的 Highlight 蓝 `#007cd2`（视觉基线 `17` 实测：
                        // `${match}` 那一行整块蓝底白字）；「当前取值」是浅一档的
                        // 灰底 `#282828`（视觉基线 `12` 的 `Solid`）。两者不同色，
                        // 别混成一个。
                        // `--screenshot` 没有真实指针，egui 的 `hovered()` 在
                        // `Area::show(ctx)` 独立 pass 里也拿不到注入的 pointer，
                        // 于是悬停蓝底永远截不出来。debug 夹具用
                        // `WINDTERM_SHOT_HOVER=<行号>` 钉住某一行（按可见文本行
                        // 计数，分隔线不算），生产构建里这段整块消失。
                        #[cfg(debug_assertions)]
                        let hot = resp.hovered()
                            || std::env::var("WINDTERM_SHOT_HOVER")
                                .ok()
                                .and_then(|s| s.parse::<usize>().ok())
                                == Some(logical - 1);
                        #[cfg(not(debug_assertions))]
                        let hot = resp.hovered();
                        if hot {
                            let fill = if skin.fit_content { 0x3d3d3d } else { 0x007cd2 };
                            p.rect_filled(row, egui::Rounding::ZERO, rgb(fill));
                        }
                        ul_text(
                            ui,
                            &p,
                            row.left() + skin.pad_left - 1.0,
                            row.center().y,
                            s,
                            rgb(if hot { c::TEXT } else { c::TEXT_2 }),
                            *ul,
                        );
                        if resp.clicked() {
                            picked = Some(i);
                        }
                        top += skin.row_h;
                    }
                }
            }
        });
    (picked, r)
}

/// 声音库下拉（视觉基线 `22`）：47 项、行高 20 点、可滚动。
///
/// 返回 `(被点的文件名, 菜单矩形)`。视觉基线里列表高到超出窗口（47 × 20 ≈ 940 点），
/// 用主菜单那套固定排布会画到屏幕外，所以这里限高并加滚动条——视觉基线右侧那条
/// 竖条就是同一个意思。
fn sound_menu(
    ctx: &egui::Context,
    anchor: egui::Rect,
    current: &str,
) -> (Option<&'static str>, egui::Rect) {
    let row_h = 20.0;
    let screen = ctx.screen_rect();
    let below = (screen.bottom() - anchor.bottom() - 8.0).max(row_h * 4.0);
    let full = ps::SOUND_NAMES.len() as f32 * row_h + 2.0;
    let h = full.min(below);
    let mut y = anchor.bottom() + 1.0;
    if y + h > screen.bottom() - 6.0 {
        y = (anchor.top() - h - 1.0).max(screen.top() + 4.0);
    }
    let r = egui::Rect::from_min_size(
        egui::pos2(anchor.left(), y),
        egui::vec2(anchor.width().max(180.0), h),
    );
    let mut picked = None;
    egui::Area::new(egui::Id::new("trig_sound_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(r.min)
        .fade_in(false)
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(rgb(c::BG))
                .stroke(egui::Stroke::new(1.0, rgb(0x2e2e2e)))
                .show(ui, |ui| {
                    ui.set_width(r.width() - 2.0);
                    egui::ScrollArea::vertical()
                        .max_height(h - 4.0)
                        .show(ui, |ui| {
                            for name in ps::SOUND_NAMES.iter() {
                                let (row, resp) = ui.allocate_exact_size(
                                    egui::vec2(r.width() - 2.0, row_h),
                                    egui::Sense::click(),
                                );
                                if resp.hovered() || *name == current {
                                    ui.painter().rect_filled(
                                        row,
                                        egui::Rounding::ZERO,
                                        rgb(c::SEL),
                                    );
                                }
                                ui.painter().text(
                                    egui::pos2(row.left() + 8.0, row.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    *name,
                                    super::fields::mono(FS),
                                    rgb(c::TEXT_2),
                                );
                                if resp.clicked() {
                                    picked = Some(*name);
                                }
                            }
                        });
                });
        });
    (picked, r)
}
