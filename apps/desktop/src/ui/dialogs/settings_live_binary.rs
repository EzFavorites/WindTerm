//! `Binary` 页：四个真数字框 + 一格斑马线开关。
//!
//! # 这一页写的是「默认参数」，不是当前会话
//!
//! 两层模型（用户 定死）：
//!
//! | 层 | 存在哪 | 落盘 | 写入口 |
//! |----|--------|------|--------|
//! | 默认参数 | `Prefs.hex_*` + `Prefs.alternate_column_bg` | 是 | **只有这一页** |
//! | 当前会话的显示方式 | `Tab.hex`（含 `zebra`）/ `Tab.mode` | 否 | `View ▸ Hex *` / `Mode ▸ Hex Edit` |
//!
//! 五格控件读写的都是**第一行**。第二行的 `Tab.hex.zebra` 只由建标签那一刻
//! 从第一行拷过去，之后设置页再怎么改都碰不到它——这正是「默认」的含义。
//!
//! # 为什么这一页的函数**看不见窗口**
//!
//! [`binary`] 只吃一份 [`HexParams`]（那份就是**默认参数**）、只吐 [`BinaryEdit`]，
//! 签名里既没有 `&mut TerminalWindow` 也没有 `Tab`。于是「设置页改默认值不追溯
//! 已开会话」不是一条靠自觉遵守的约定，而是**这个函数做不到**：它手上没有任何
//! 标签。写到 prefs 的那一步在调用方（`dialogs/settings.rs`），那边只在本页返回
//! `Some(edit)` 时才动盘。
//!
//! 参数的形状也是这条边界的一部分：斑马线**收在 [`HexParams`] 里**，不另开一个
//! `zebra: bool` 字段。曾经它是窗口上一个独立的 bool，于是同一页五个同类参数
//! 有两套语义——勾掉那一格当场改掉了所有已开标签的画法，而同页四个数字只影响
//! 新标签。合成一份之后，五个参数只有一条路径、一种作用域。
//!
//! 页底因此留了一行说明——用户改完 24 列发现眼前那个标签一点没变，没有这句话
//! 只会以为控件坏了。视觉基线这一页本来没有文字，这行是我们多出来的解释：
//! 宁可多一句，也不能让它看着像坏掉。
//!
//! # 为什么四个数取窗口上的副本而不每帧读盘
//!
//! 与 `settings_live_font` 同一条理由：调用方传进来的 [`HexParams`] 是**生效值**
//! （已按区间夹过的那一份）。每帧读 `prefs.json` 会多出一份可能落后的真相
//! ——拖动时盘上还是旧值，`DragValue` 每帧被喂回去就会跳回原位。
//!
//! # 为什么标签不是右对齐 + 省略号
//!
//! 视觉基线这一页的标签是**左对齐全称**（`Binary grouped characters:`），而数据表
//! 那些页共用 140px 的右对齐标签列，长标签会被截成 `Binary grouped char…`。
//! 这一页自己定列宽（[`LABEL_W`]），长短都写全——四个标签是本页唯一的信息，
//! 截掉一半等于没写。

use crate::ui::kit::field::{choice_row_resp, ChoiceKind, UnderlineStyle};
use crate::ui::kit::spec::BLOCK_GAP;
use super::settings_pages_view::{group_box, set_row_rhythm, spin_field_delta};
use crate::ui::terminal::HexParams;
use crate::ui::*;

/// 视觉基线里的行标签，逐字照抄——包括那个拼错的 `devision`。
///
/// 单独提出来是为了让单测能钉住它：拼写是保真的一部分，不是笔误。
pub(in crate::ui) const DEVISION_LABEL: &str = "Binary devision columns:";

/// 另外三条标签。同样逐字照抄视觉基线（全称，不是缩写）。
const BASE_LABEL: &str = "Binary base:";
const COLUMNS_LABEL: &str = "Binary columns:";
const GROUPED_LABEL: &str = "Binary grouped characters:";

/// 标签列宽：要装得下最长那条 `Binary grouped characters:`，**还要留出与数字框的留白**。
///
/// 184 是量出来的：正文 11pt 下最长那条的 ink 宽度是 181 逻辑像素。取 176 时
/// 冒号已经贴到数字框左边框上（溢出 5、缝只剩 3），图上看得见——余量是负的。
/// 184 = 181 + 3 的留白，两倍缩放的基线里第四条与框之间明显分开。
///
/// 不用 `settings_pages_view::right_label` 的 140：那一条会 `truncate`，长标签在这页
/// 会变成 `Binary grouped char…`——而视觉基线给的是全称。
const LABEL_W: f32 = 184.0;

/// 标签格的高度。与 `number_row` 里数字框那一行取齐，别让行高跟着框的内容飘。
const LABEL_H: f32 = 20.0;

/// 页底那一行说明：这些数是**默认值**，当前会话去菜单改。
const DEFAULTS_NOTE: &str = "These are the defaults new sessions start with; the open session is \
     changed from View ▸ Hex *.";

/// 斑马线那一格的悬停说明：它画在哪、按什么分组。
const ZEBRA_HOVER: &str =
    "Tints alternate division groups in the hex view (division columns = the row above).";

/// 这一帧用户改了什么。两个字段分开是必要的：
///
/// - `apply`：拖动过程中每帧都要跟手（数字得走），所以只要有改动就非 `None`；
/// - `persist`：写盘只在松手 / 键盘改完时发生——每帧写一次 JSON 纯属噪声
///   （与 `settings_live_font` 同一判据）。
///
/// `apply` 里带的是整份 [`HexParams`]，斑马线也在里面：五格控件改的是**同一份**
/// 默认参数，分两个字段运回去只会让调用方多一次「先合再写」的机会。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::ui) struct BinaryEdit {
    pub apply: Option<HexParams>,
    pub persist: bool,
}

/// `Binary` 页：画一帧，返回用户这一帧改了什么。
///
/// `cur` 是**默认参数**的当前值（`TerminalWindow::hex_defaults`），不是当前
/// 标签的——这一页看不见标签（见模块头）。
pub(in crate::ui) fn binary(ui: &mut egui::Ui, cur: HexParams) -> BinaryEdit {
    // `base` 单独转成 `usize` 只为让四个框共用同一个 `number_row`
    // （`DragValue` 按类型泛型，混用 u32 / usize 要写两份一模一样的行）；
    // 交回 `from_prefs` 时再转回去。
    let mut base = cur.base as usize;
    let mut columns = cur.columns;
    let mut division = cur.division;
    let mut group = cur.group;
    let mut zebra = cur.zebra;
    let mut touched = false;
    let mut persist = false;

    ui.add_space(BLOCK_GAP);
    // 视觉基线 `12-text-binary.png`：内容区第一个字就是组标题 `Binary`（老实现
    // 还多画了一行灰页标题，同一个词出现两次、整页下移 26 点）。
    group_box(ui, Some("Binary"), |ui| {
        set_row_rhythm(ui);
        // 顺序照视觉基线逐行对齐：base / columns / devision / grouped characters。
        // `devision` 夹在第二、四行之间，别按语义重排。
        let (a, b) = number_row(ui, BASE_LABEL, &mut base, 2..=16);
        touched |= a;
        persist |= b;
        let (a, b) = number_row(ui, COLUMNS_LABEL, &mut columns, 1..=64);
        touched |= a;
        persist |= b;
        // 加宽周期与连写分组的上界都是**列数**：一组比一整行还宽，那一行就没有
        // 任何分隔符，「分组」这个设置等于没生效。改小列数时下一帧这两个框的
        // 上限跟着缩，越界值由 `HexParams::from_prefs` 夹回去。
        let (a, b) = number_row(ui, DEVISION_LABEL, &mut division, 1..=columns.max(1));
        touched |= a;
        persist |= b;
        let (a, b) = number_row(ui, GROUPED_LABEL, &mut group, 1..=columns.max(1));
        touched |= a;
        persist |= b;
    });
    ui.add_space(BLOCK_GAP);
    // 斑马线：真勾选框（视觉基线这一格是勾上的，出厂 `true`）。
    //
    // 它和上面四个数是**同一类**参数（都是默认值），所以勾完之后不会立刻看见
    // 变化——已开标签各带自己的 `Tab.hex.zebra`，新开的标签才用新值。页底那行
    // 说明管的就是这件事，别把这一格排除在外：同一个设置页里五个同类参数有
    // 两套语义（四个等下次、一个当场变）正是这一版修掉的东西。
    let r = choice_row_resp(ui, ChoiceKind::Checkbox, "Alternate column background color", "", &mut zebra, true, UnderlineStyle::None);
    if r.clicked() {
        touched = true;
        persist = true;
    }
    r.on_hover_text(ZEBRA_HOVER);
    // 视觉基线没有页脚说明；默认值的边界写在各自控件的 hover 上。

    if !touched {
        return BinaryEdit {
            apply: None,
            persist: false,
        };
    }
    // 整份过一遍 `from_prefs`：区间判断只有那一份实现（`group` / `division`
    // 的上界是列数），这里再夹一遍就等于第二套规则。斑马线原样穿过它。
    BinaryEdit {
        apply: Some(HexParams::from_prefs(
            base as u32,
            columns,
            group,
            division,
            zebra,
        )),
        persist,
    }
}

/// 一行「左对齐标签 + 数字框」。返回 `(跟手, 该落盘)`。
///
/// 数字框套一层框：`DragValue` 自己不画边框，裸放着与上面几行的灰字长得一样，
/// 而「看起来能改」正是这一页每一行该有的样子（Font 页同一条先例）。
fn number_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut usize,
    range: std::ops::RangeInclusive<usize>,
) -> (bool, bool) {
    let mut changed = false;
    let mut persist = false;
    ui.horizontal(|ui| {
        // 标签列宽固定，「右缘对齐」靠它；返回值只在单测里读，这里不看。
        let _ = label_cell(ui, label);
        // 占满标签列之后的整段：视觉基线 `12-text-binary.png` 的数字框一路到框右
        // 边（数字靠左、▲▼ 贴右缘），不是标签旁边一个小方框。
        let lo = *range.start();
        let hi = *range.end();
        let (_, stepped) = spin_field_delta(
            ui,
            |ui| {
                let r = ui.add(egui::DragValue::new(value).range(range).speed(0.2));
                changed |= r.changed();
                // 与 Font 页同一判据：拖动过程中不写盘，松手或键盘改完才写。
                persist |= r.drag_stopped() || (r.changed() && !r.dragged());
            },
        );
        if let Some(d) = stepped {
            if d > 0 { *value = (*value + 1).min(hi); }
            else { *value = (*value - 1).max(lo); }
            changed = true;
            persist = true;
        }
    });
    (changed, persist)
}

/// 左对齐的固定宽标签列，返回它**实际占用**的宽度。
///
/// 不 `truncate`：这一页的标签必须写全（见模块头）。宽度取 [`LABEL_W`]，
/// 装不下时宁可让文字溢出，也不截成 `…`——那时该改的是 [`LABEL_W`]。
///
/// # 为什么是 `allocate_exact_size` 而不是 `allocate_ui_with_layout`
///
/// 后者只把 [`LABEL_W`] 当**上限**：子 UI 的 `min_rect` 是内容尺寸，外层按内容
/// 收缩，于是标签列的实际宽度跟着文字长度跑，右边四个数字框左右参差（实测
/// 四行右缘 624.8 / 642.5 / 695.7 / 707.5，同一个逻辑列画成了四级台阶）。
/// 要**占满**一列就得精确预留一块矩形，然后在这块矩形里自己画字。
///
/// 返回值是 `rect.width `（预留出来的那块，不是 [`LABEL_W`] 常量本身）：
/// 单测据此断言「四行都是同一列」。谁把它换回按内容收缩的 API，返回值立刻
/// 变小，测试就红——只看图守不住这件事。
fn label_cell(ui: &mut egui::Ui, text: &str) -> f32 {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(LABEL_W, LABEL_H), egui::Sense::hover());
    ui.painter().text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        text,
        egui::TextStyle::Body.resolve(ui.style()),
        ui.visuals().text_color(),
    );
    rect.width()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 四条标签都得是视觉基线里的**全称**：截成 `…` 等于没写。
    #[test]
    fn the_labels_are_the_full_reference_names() {
        for label in [BASE_LABEL, COLUMNS_LABEL, DEVISION_LABEL, GROUPED_LABEL] {
            assert!(label.starts_with("Binary "), "{label} 少了前缀");
            assert!(label.ends_with(':'), "{label} 少了冒号");
            assert!(!label.contains('…'), "{label} 不许用省略号");
        }
        assert_eq!(BASE_LABEL, "Binary base:");
        assert_eq!(COLUMNS_LABEL, "Binary columns:");
        // 视觉基线自己拼成 `devision`：逐字照抄的证据。改成 `division` 就红。
        assert_eq!(DEVISION_LABEL, "Binary devision columns:");
        assert_eq!(GROUPED_LABEL, "Binary grouped characters:");
        // 标签列必须真的装得下最长那条，并且留出与数字框的留白：
        // 184 = 实测 ink 宽度 181 + 余量 3。缩到 140（数据表页那条右对齐列）
        // 会把第四条截成 `Binary grouped char…`，缩回 176 则冒号贴到框上。
        assert_eq!(
            LABEL_W, 184.0,
            "184 = 实测 ink 181 + 留白 3；缩回 176 冒号会贴到数字框上"
        );
    }

    /// 页底那行必须点明「这页是默认值、当前会话在菜单」，否则改完眼前不变会被
    /// 当成坏掉。这是视觉基线没有、我们多加的一句。
    #[test]
    fn the_note_separates_the_defaults_from_the_open_session() {
        assert!(DEFAULTS_NOTE.contains("defaults"), "要点明写的是默认值");
        assert!(
            DEFAULTS_NOTE.contains("new sessions"),
            "要点明对谁生效（新会话）"
        );
        assert!(
            DEFAULTS_NOTE.contains("View ▸ Hex *"),
            "要给当前会话的改法指路"
        );
    }

    /// **四行数字框必须落在同一列**：标签列是固定宽的预留，不是「最多 176」。
    ///
    /// 这条挡的是我犯过的错：`allocate_ui_with_layout` 只把 176 当上限、外层按
    /// 内容收缩，于是四行数字框的右缘跟着标签长度跑（实测 624.8 / 642.5 /
    /// 695.7 / 707.5，同一个逻辑列画成了四级台阶）。只靠看图守不住——下一轮
    /// 换个人写又会参差，所以这里量的是**真布局出来的数字**：
    ///
    /// - 标签格自己占的宽度，四条都必须正好是 [`LABEL_W`]；
    /// - 走真的 [`number_row`] 铺四行，`ui.horizontal` 的外框右缘必须相同
    ///   （数字框是那行的最后一个控件，外框右缘相同 ⇒ 框的右缘相同）。
    ///
    /// `Context::run` 不需要窗口，所以它不弹任何东西、也不抢焦点。
    #[test]
    fn the_four_number_boxes_share_one_right_edge() {
        let labels = [BASE_LABEL, COLUMNS_LABEL, DEVISION_LABEL, GROUPED_LABEL];
        let ctx = egui::Context::default();
        let mut widths: Vec<f32> = Vec::new();
        let mut rights: Vec<f32> = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                // 一、标签格自己的宽度。
                for label in labels {
                    ui.horizontal(|ui| {
                        widths.push(label_cell(ui, label));
                    });
                }
                // 二、四行走真实现，量每行的右缘。
                for label in labels {
                    let mut value = 4usize;
                    let row = ui.horizontal(|ui| {
                        let _ = number_row(ui, label, &mut value, 1..=16);
                    });
                    rights.push(row.response.rect.right());
                }
            });
        });

        assert_eq!(widths.len(), 4, "四条标签都要画出来");
        for (label, w) in labels.iter().zip(&widths) {
            assert_eq!(
                *w, LABEL_W,
                "{label} 的标签格占了 {w}px，不是 {LABEL_W}px —— 标签列不再是固定宽，右侧数字框会参差"
            );
        }

        assert_eq!(rights.len(), 4, "四行都要画出来");
        let first = rights[0];
        for (label, right) in labels.iter().zip(&rights) {
            assert!(
                (right - first).abs() < 0.01,
                "{label} 那一行的数字框右缘在 {right}，第一行在 {first} —— 四行没有对齐"
            );
        }
    }

    /// **无窗口**跑一帧：页面能画出来，且没人点它时返回「没有改动」。
    ///
    /// 这是替代肉眼验证的那一条：真的走一遍 `egui` 的布局（四个 `DragValue`、
    /// 一个勾选框、页底说明），而不是只断言常量。`Context::run` 不需要窗口，
    /// 所以它不会弹出任何东西、也不抢焦点。
    ///
    /// 「没有改动」这一半同时也是**不写盘**的闸：调用方只有拿到 `apply` 才会
    /// 写 prefs，而这一页每帧都跑，一帧误报就是每秒 60 次磁盘写。
    #[test]
    fn the_page_draws_headlessly_and_reports_no_edit() {
        let ctx = egui::Context::default();
        let cur = HexParams::default();
        let mut got: Option<BinaryEdit> = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                got = Some(binary(ui, cur));
            });
        });
        let edit = got.expect("面板跑起来了");
        assert_eq!(edit, BinaryEdit::default(), "没人碰它，应该是空改动");
        assert!(edit.apply.is_none(), "没人碰它，不该报改动");
        assert!(!edit.persist, "没人碰它，就不该写盘");
    }
}
