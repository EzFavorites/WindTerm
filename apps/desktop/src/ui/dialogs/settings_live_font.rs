//! `Text ▸ Font`：视觉基线 `14-text-font.png` 的 `Default Font` 分组框。
//!
//! # 这一页现在五行全真
//!
//! - `Font family 1–4`：四槽字族，取自本机已装字面清单（`fonts::catalog`），
//!   拉下拉选，选完当场重装字形 + 重量 cell 盒，并写回 `prefs.font_family`。
//!   空 = 自动（槽 0 走内置等宽候选、槽 1 走内置 CJK 候选）。
//! - `Font weight`：主字族（槽 0）能选的字重，来自该字族真实装着的面
//!   （`Regular` / `Bold` / `Medium` …）。写回 `prefs.font_weight`。
//! - `Font size:`：绑 `Prefs.font_size`，改完当场 `TerminalFonts::set_size`。
//! - `Prefer theme fonts`：主题现在能声明可选字体（`Palette::theme_fonts`），
//!   勾上后优先用它；没声明的主题保持现状，不替用户换字体。
//!
//! # 为什么显示值取生效值而不是读盘
//!
//! `TerminalFonts` 里那一份就是**生效值**（清单解析过、文件读过、字重夹过），
//! 设置页要显示的正是它。每帧读 `prefs.json` 反而会多出一份可能落后的真相：
//! 拖动时盘上还是旧值，控件每帧被喂回去就会跳回原位。
//!
//! # 分文件的原因
//!
//! 从 `settings_live.rs` 拆出来是因为那个文件已经贴着  的代码行上限
//! （500），而本页要加下拉、过滤、读写 prefs、说明，塞回去就超了。

use super::settings_live::{recovery_notice, save_prefs};
use super::settings_pages_view::{field_label, field_row, group_box, label_col, spin_field, Slot};
use crate::ui::*;
use term_app::i18n::Lang;

/// 页底说明：这页现在哪些行真、哪一行还缺什么。
///
/// 显式换行而不是让 egui 自动折行：这一页的行比右栏宽（140 标签 + 字段），
/// 右栏的 `max_width` 会被这几行撑到比可见区域更宽，自动折行会把每行末尾
/// 裁在窗口外（实测「terminal g」「weight s」）。分行写死，每行都在可见宽度内。
const FONT_PAGE_NOTE: &str = "Family, weight, size and theme preference apply immediately and are \
     saved.\nInstalled families are listed here; fixed-width ones are shown in bold.";

fn persisted_fonts() -> [String; 4] {
    term_runtime::prefs::load()
        .map(|p| p.font_family_value())
        .unwrap_or_default()
}

fn persisted_weight() -> u16 {
    term_runtime::prefs::load()
        .map(|p| p.font_weight_value())
        .unwrap_or(400)
}

/// 一帧里被改动的那几项。`None` = 没动（load-modify-save 的 modify 半边）。
#[derive(Default)]
struct FontEdit {
    /// `(槽号, 字族名)`；空串 = 回到自动。
    family: Option<(usize, String)>,
    weight: Option<u16>,
    size: Option<f32>,
    prefer_theme: Option<bool>,
    /// 字号要不要落盘。拖动中每帧都在变，松手才写一次 JSON。
    persist_size: bool,
}

impl FontEdit {
    fn touched(&self) -> bool {
        self.family.is_some() || self.weight.is_some() || self.size.is_some() || self.prefer_theme.is_some()
    }

    /// 这一帧该不该写盘。拖字号的过程中不写（每帧一次 JSON 纯属噪声），
    /// 但选字族 / 选字重是「一次点击」，立刻写。
    fn should_save(&self) -> bool {
        self.persist_size || self.family.is_some() || self.weight.is_some() || self.prefer_theme.is_some()
    }

    /// 只写自己动过的那几项。
    fn write_into(&self, prefs: &mut term_profile::prefs::Prefs) {
        if let Some((slot, name)) = &self.family {
            if let Some(dst) = prefs.font_family.get_mut(*slot) {
                *dst = name.clone();
            }
        }
        if let Some(w) = self.weight {
            prefs.font_weight = w;
        }
        if let Some(s) = self.size {
            prefs.font_size = s;
        }
        if let Some(p) = self.prefer_theme {
            prefs.prefer_theme_fonts = p;
        }
    }
}

/// `Text ▸ Font` 页。
pub(in crate::ui) fn font(win: &mut TerminalWindow, ui: &mut egui::Ui, lang: Lang) {
    // 字面清单第一次进来时扫一次（本机 745 个文件，热缓存约 1s），之后复用。
    // 放在页入口而不是启动路径上：默认安装一辈子不进这一页，就不该为它读 1.2 GB。
    win.fonts.ensure_catalog();
    let edit = draw(ui, win, lang);
    if !edit.touched() {
        return;
    }
    if let Some((slot, name)) = &edit.family {
        win.fonts.set_family(*slot, name);
    }
    if let Some(w) = edit.weight {
        win.fonts.set_weight(w);
    }
    if let Some(prefer) = edit.prefer_theme {
        win.prefer_theme_fonts = prefer;
        // 开：用主题声明覆盖槽 0 / 字重；关：从 prefs 恢复用户自己那套。
        let (families, weight) = if prefer {
            let (family, weight) = win.palette.theme_fonts();
            (
                family
                    .map(|f| [f, String::new(), String::new(), String::new()])
                    .unwrap_or_else(|| persisted_fonts()),
                weight.unwrap_or_else(|| persisted_weight()),
            )
        } else {
            (persisted_fonts(), persisted_weight())
        };
        let size = win.fonts.size();
        win.fonts = TerminalFonts::load_with(families, weight);
        win.fonts.set_size(size);
    }
    if let Some(size) = edit.size {
        // 只改目标值：`install` 每帧比较 size，变了才重装字形并让度量失效，
        // 再由 `measure_once` 用新尺寸量出 cell 盒。这里自己重装会撞上
        // 「定义下一帧才生效」的时序（见 `fonts/mod.rs::install`）。
        win.fonts.set_size(size);
        // 命令中心那行「默认字号」读的是 `settings.font_size` 这份副本，
        // 不同步就会在终端已经换成 18 之后还写着启动时的 13。
        win.settings.font_size = size;
    }
    // 有任意一项要落盘就整体写一次：三个字段在同一次 load-modify-save 里改，
    // 比各写各的少两次读盘，也不会出现「字族写进去了、字重还在旧文件」的
    // 半份状态。
    if edit.should_save() {
        save_prefs(|prefs| edit.write_into(prefs));
    }
}

/// 画这一页，返回「哪几项被改了」。拆出来只为让 [`font`] 的写回部分读起来
/// 是一段直线（画 → 判 → 写），不被几十行布局代码隔开。
fn draw(ui: &mut egui::Ui, win: &TerminalWindow, _lang: Lang) -> FontEdit {
    let mut edit = FontEdit::default();
    let mut size = win.fonts.size();
    let mut apply_size = false;
    let mut persist_size = false;

    recovery_notice(ui);
    ui.add_space(6.0);
    // 内容区第一个字就是组标题 `Default Font`（视觉基线 `14-text-font.png`）。
    // 标签列按本页最长的 `Font family N:` 量（`FAMILY_SLOTS` 那一档），三行对齐。
    let col = label_col(ui, &["Font size:", "Font family 1:", "Font weight:"]);
    group_box(ui, Some("Default Font"), |ui| {
        // 视觉基线原句。等宽字族在下拉里用粗体显示，就是这句话在起作用。
        ui.label(egui::RichText::new("Bold type indicates fixed-width fonts:").weak());
        ui.add_space(2.0);
        for slot in 0..crate::fonts::FAMILY_SLOTS {
            family_row(ui, win, slot, &mut edit, col);
        }
        weight_row(ui, win, &mut edit, col);
        ui.horizontal(|ui| {
            field_label(ui, "Font size:", col);
            // 0.5 的步长：13 → 13.5 → 14 是终端里最有用的那几档（整数跳一格
            // 在 13pt 附近显得太粗），区间与渲染层的夹取范围同一份常量。
            //
            // 套 `field_box`：`DragValue` 本身不画边框，裸放在这一页里和上面几行的
            // 灰字长得一样——「看起来能改」是这一页控件该有的样子；而且必须和
            // 上面三行下拉的左右缘齐平（同一个 `col` 标签列 + 同一个通栏余宽）。
            spin_field(ui, |ui| {
                let r = ui.add(
                    egui::DragValue::new(&mut size)
                        .range(term_profile::prefs::FONT_SIZE_MIN..=term_profile::prefs::FONT_SIZE_MAX)
                        .speed(0.5)
                        .suffix(" pixels"),
                );
                apply_size |= r.changed();
                // 松手（或者键盘改 / 点箭头）才落盘：拖动过程中每帧写一次
                // `prefs.json` 纯属噪声。
                persist_size |= r.drag_stopped() || (r.changed() && !r.dragged());
            });
        });
    });
    // 视觉基线 `14-text-font.png`：`Prefer theme fonts` 复选框在框**外**、框底之下
    // （与 `16-text-wrap.png` 的 `Show wrap symbols` 同一个位置关系）。
    ui.add_space(2.0);
    let mut prefer_theme = win.prefer_theme_fonts;
    if ui
        .checkbox(&mut prefer_theme, "Prefer theme fonts")
        .on_hover_text(
            "Use the font declared by the active theme (family / weight) instead of \
             the four slots above. Themes without a font declaration keep the current fonts.",
        )
        .changed()
    {
        edit.prefer_theme = Some(prefer_theme);
    }
    if apply_size {
        edit.size = Some(size);
    }
    edit.persist_size = persist_size;
    edit
}

/// 一行字族：右对齐标签 + 一个占满余宽的下拉。
///
/// 下拉里第一项是 `(Default)`（回到自动），其余是本机全部字族，等宽的用粗体。
/// 顶部带一个过滤框：本机三百多个字族，没有过滤就得滚很久——这不是「加分项」，
/// 是让这个下拉可用的最低要求。
fn family_row(
    ui: &mut egui::Ui,
    win: &TerminalWindow,
    slot: usize,
    edit: &mut FontEdit,
    col: f32,
) {
    // 走 `field_row`：下拉必须套进 `field_box`，与同一页的文本框同一个底与描边。
    field_row(ui, &format!("Font family {}:", slot + 1), Slot::Inline, col, "", |ui| {
        let info = win.fonts.slot_info(slot);
        // 字段里只写生效的字族名（与视觉基线一致）：`(automatic)` 这类注解
        // 挂在悬停上——那一行本身是「你现在的字」，不是状态栏。
        let shown = if info.fell_back {
            // 存着一个本机没装的名字：字段里必须看得见，否则用户会以为
            // 「选了不变」是程序坏了。
            format!("{} (not installed)", info.requested)
        } else {
            // 空槽留空（视觉基线里第 3/4 行就是空框）：写 `(None)` 反而比
            // 视觉基线多出四个字符，把字段撑成另一种样子。
            info.family.clone()
        };
        let hover = if info.requested.is_empty() {
            "Automatic: follows the built-in default for this slot.".to_string()
        } else if info.fell_back {
            format!(
                "{} is not installed on this machine — using {} instead.",
                info.requested, info.family
            )
        } else {
            format!("{} (weight {})", info.family, info.weight)
        };
        let combo = egui::ComboBox::from_id_salt(("prefs_font_family", slot))
            .selected_text(shown)
            .width(ui.available_width())
            .wrap_mode(egui::TextWrapMode::Truncate)
            .show_ui(ui, |ui| {
                // 过滤串存在 egui 的临时数据里：它纯属「这个下拉开着的时候
                // 的输入状态」，关掉弹层就没有意义，不该进窗口状态（那会多出
                // 一个没人写、没人清的字段）。按槽分开存，换一行不会继承上一行
                // 的过滤词。
                let id = ui.id().with(("font_family_filter", slot));
                let mut filter: String = ui.memory(|m| m.data.get_temp(id).unwrap_or_default());
                ui.add(
                    egui::TextEdit::singleline(&mut filter)
                        .hint_text("Filter families")
                        .desired_width(ui.available_width().min(240.0)),
                );
                ui.memory_mut(|m| m.data.insert_temp(id, filter.clone()));
                let needle = filter.trim().to_lowercase();
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        // 第一项始终在（哪怕在过滤）：它是「回到自动」的唯一
                        // 出口，被过滤词藏起来就再也回不去了。
                        if ui
                            .selectable_label(
                                info.requested.is_empty(),
                                format!("(Default: {})", auto_label(&info)),
                            )
                            .clicked()
                        {
                            edit.family = Some((slot, String::new()));
                        }
                        for family in win.fonts.families() {
                            if !needle.is_empty() && !family.name.to_lowercase().contains(&needle) {
                                continue;
                            }
                            let selected = family.name.eq_ignore_ascii_case(&info.requested);
                            let mut text = egui::RichText::new(&family.name);
                            if family.monospace {
                                text = text.strong();
                            }
                            if ui.selectable_label(selected, text).clicked() {
                                edit.family = Some((slot, family.name.clone()));
                            }
                        }
                    });
            });
        combo.response.on_hover_text(hover);
    });
}

/// `(Default: …)` 括号里写什么：自动生效的那个字族名；该槽本来就不自动时
/// 写 `None`（槽 2–3 默认不用）。
fn auto_label(info: &crate::fonts::SlotInfo) -> String {
    if info.family.is_empty() {
        "None".to_string()
    } else {
        info.family.clone()
    }
}

/// `Font weight` 一行：可选字重来自主字族真实装着的面。
fn weight_row(ui: &mut egui::Ui, win: &TerminalWindow, edit: &mut FontEdit, col: f32) {
    field_row(ui, "Font weight:", Slot::Inline, col, "", |ui| {
        let info = win.fonts.slot_info(0);
        let choices = win.fonts.weight_choices();
        let current = choices
            .iter()
            .find(|(w, _)| *w == info.weight)
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| crate::fonts::catalog::canonical_weight(info.weight).to_string());
        egui::ComboBox::from_id_salt("prefs_font_weight")
            .selected_text(current)
            .width(ui.available_width())
            .wrap_mode(egui::TextWrapMode::Truncate)
            .show_ui(ui, |ui| {
                for (weight, label) in &choices {
                    // 下拉里带上数值：`Regular` 与 `Medium` 光看名字分不出
                    // 「这字体只有一档」还是「两档都在」。
                    let text = format!("{label} ({weight})");
                    if ui.selectable_label(*weight == info.weight, text).clicked() {
                        edit.weight = Some(*weight);
                    }
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_no_longer_claims_only_the_size_is_live() {
        // 反向的谎和灰行一样糟：接着说「只有字号是真的」会让用户以为
        // 字族下拉是个装饰。
        assert!(
            !FONT_PAGE_NOTE.contains("Only Font size"),
            "接真后不能再说只有字号可用：{FONT_PAGE_NOTE}"
        );
        assert!(FONT_PAGE_NOTE.contains("Family"), "要点名字族");
        assert!(FONT_PAGE_NOTE.contains("weight"), "要点名字重");
        assert!(
            FONT_PAGE_NOTE.contains("bold"),
            "要说清「粗体 = 等宽」这条列表规则"
        );
        assert!(
            FONT_PAGE_NOTE.contains("saved"),
            "这四项都会落盘，页底要交代"
        );
    }

    #[test]
    fn the_page_note_claims_all_five_rows() {
        assert!(FONT_PAGE_NOTE.contains("theme preference"));
        assert!(FONT_PAGE_NOTE.contains("saved"));
    }

    #[test]
    fn each_edit_writes_only_its_own_field() {
        // load-modify-save 的 modify 半边：选字族不许把字号与字重回退。
        let mut prefs = term_profile::prefs::Prefs {
            font_family: ["Menlo".to_string(), String::new(), String::new(), String::new()],
            font_weight: 700,
            font_size: 15.0,
            language: "zh-CN".to_string(),
            ..Default::default()
        };
        FontEdit {
            family: Some((1, "Hiragino Sans GB".to_string())),
            ..Default::default()
        }
        .write_into(&mut prefs);
        assert_eq!(prefs.font_family[1], "Hiragino Sans GB");
        assert_eq!(prefs.font_family[0], "Menlo", "别的槽不许被连坐");
        assert_eq!(prefs.font_weight, 700, "字重不许被字族连坐");
        assert_eq!(prefs.font_size, 15.0, "字号不许被连坐");

        FontEdit {
            weight: Some(400),
            ..Default::default()
        }
        .write_into(&mut prefs);
        assert_eq!(prefs.font_weight, 400);
        assert_eq!(prefs.font_family[1], "Hiragino Sans GB", "反过来也一样");
        assert_eq!(prefs.language, "zh-CN");
    }

    #[test]
    fn clearing_a_slot_writes_an_empty_name() {
        // `(Default)` 写的是空串而不是「当前生效的那个名字」：后者会让
        // 「自动」变成「钉死在今天这台机器的 Menlo 上」。
        let mut prefs = term_profile::prefs::Prefs::default();
        prefs.font_family[2] = "Monaco".to_string();
        FontEdit {
            family: Some((2, String::new())),
            ..Default::default()
        }
        .write_into(&mut prefs);
        assert_eq!(prefs.font_family[2], "");
    }
}
