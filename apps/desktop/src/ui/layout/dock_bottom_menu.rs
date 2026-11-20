//! 底带的齿轮菜单与三个图标：**能画就不排字**。
//!
//! 拆出来是因为 R9：`dock_bottom.rs` 只管带的结构（标签条 / 分组 / 内容），
//! 菜单与图标各占一块，两件事凑在一份文件里就会顶到 450 行。
//!
//! 这里的每一样都受同一条约束：**不许赌字体**。勾选是两段线画的，齿轮是一圈
//! 加八颗齿，`^` 是两条 45° 短线——与 `tool_bar_icons.rs` / `session_icons.rs`
//! 同一原则。

use eframe::egui;
use term_app::dock::{DockZone, PanelId};
use term_app::i18n::{self, Lang};
use term_core::theme::CHROME_SCALE;

use super::dock_bottom::{BTN_W, Cmd, TAB_H};
use crate::ui::shell;

/// 每颗齿轮一个独立 popup id：同一帧里有多颗齿轮都会调 `dock_menu`，若共用
/// 一个 id，点下一颗时其它齿轮的 `dock_menu` 会把它当成「点在外面」立刻收起，
/// 表现就是「齿轮点了菜单闪一下就没」。按 `zone + active + in_float_window`
/// 派发互不干扰（底带只有一颗，固定 id；侧带每颗齿轮属于不同面板，按面板名区分；
/// 浮窗与原停靠位空壳可能指向同一面板，再带一个位置位）。
pub(in crate::ui) fn menu_id(menu: &GearMenu) -> egui::Id {
    // 浮窗里的齿轮与原停靠位空壳的齿轮可能指向同一面板（active 相同），
    // 必须再带一个「是否在浮窗里」的位，否则两处菜单共用一个 popup id，
    // 点一处会同时弹两处（2026-11-16 K3-b 实测风险）。
    match menu.zone {
        DockZone::Bottom => egui::Id::new(("dock_gear_menu", "bottom", menu.in_float_window)),
        _ => egui::Id::new((
            "dock_gear_menu",
            menu.zone,
            menu.active.name(),
            menu.in_float_window,
        )),
    }
}

/// 标签条右端的一颗图标按钮。点按区是 `BTN_W` 见方、贴条顶。
pub(super) fn icon_button(ui: &mut egui::Ui, strip_top: f32, x: f32, id: egui::Id) -> egui::Response {
    let r = egui::Rect::from_min_size(egui::pos2(x, strip_top), egui::vec2(BTN_W, TAB_H));
    ui.interact(r, id, egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 齿轮菜单要「站在哪条带上说话」。
///
/// 三个带的齿轮弹的是**同一张菜单**（用户 2026-11-13：所有面板的设置按钮都该
/// 弹这个），差别只有三处，全都由它决定：
/// - `zone`：勾选框说的是「在不在**这条**带里」，`Split` 只对底带有意义；
/// - `band`：本条带此刻的成员，用来勾选；
/// - `active`：`Float` 要浮哪一块（侧带里就是这颗齿轮所属的面板）。
pub(in crate::ui) struct GearMenu<'a> {
    pub zone: DockZone,
    pub band: &'a [PanelId],
    pub active: PanelId,
    /// 这条带此刻是不是浮着。浮着时 `Float` 拆出当前 tab，而不是整带。
    pub band_floating: bool,
    /// 这颗齿轮是在浮窗里，还是在原停靠位的空壳里。只用于区分 popup id。
    pub in_float_window: bool,
}

/// 齿轮菜单的一行：分隔线 / 命令 / 面板勾选开关。
enum Entry {
    Sep,
    Row {
        label: String,
        /// 访问键在**英文 key**上的字节位；中文显示时划首字。
        underline: Option<usize>,
        check: Option<bool>,
        cmd: Cmd,
        disabled: bool,
    },
}

/// 兼容终端菜单每行第一个字母都带下划线（S̲plit / F̲loat / Ëxplorer / …）。
/// 位置复用主菜单那张访问键表；翻译成中文时按 `menu_items::underline_for`
/// 的同一口径划首字。
fn labelled(lang: Lang, key: &'static str) -> (String, Option<usize>) {
    let text = i18n::t(lang, key).to_string();
    let underline = if text == key {
        term_app::menu::access_char_pos(key)
    } else {
        Some(0)
    };
    (text, underline)
}

/// 齿轮菜单。
///
/// **不用 `popup_below_widget`**：那是 `Area` 包出来的，`Area` 默认淡入，而夹具
/// 把时钟钉在 0，整张菜单就停在淡入的第一帧——截图里是一片几乎透明的灰字
/// （实测）。本项目的浮动面板 / 工具栏都栽在同一个地方，所以这里显式
/// `.fade_in(false)`，并自己管「点外面收起」。
///
/// 展开方向按齿轮的实际屏幕位置：在窗口下半（底带收回态、窗口底部）**向上**弹，
/// 在上半（左右带标题栏、底带展开态标签条被推到菜单栏下）**向下**弹。不按 zone
/// 硬编码——底带展开时齿轮会跑到窗口顶部，照旧向上弹就出界了。
///
/// 2026-11-13：宽度不再写死 106，改由自己的行表按
/// [`crate::ui::shell::rows_width`] 算（与菜单栏 / 右键菜单同一条公式）；
/// 行也改用同一套 [`crate::ui::shell::draw_menu_row`]，三态色与勾选框几何
/// 从此和下拉菜单一模一样，不再各画一套。
/// 一次性把菜单的行表算出来（不碰 `Ui`，所以能单测）。
///
/// 三个带的菜单**内容同源**，差别只有三处，都在这里：
/// - `Split` 每条带都有，但左右带**置灰**：它们是上下均分的，切一刀没有语义。
///   兼容终端浮动窗那张图里 Split 就是灰的，直接少一行反而不像（R5 的「点了没反应」
///   用禁用态解决，不是用删行解决）；
/// - `Float` 行的勾表示「这条带是不是浮着」，兼容终端浮窗图里它带着勾；
/// - 勾选框说的是「在不在**这条**带里」，所以同一块面板在左带菜单里可能勾着、
///   在底带菜单里没勾——这正是「把面板从左带拖到底带」在菜单上的表达。
fn gear_entries(
    zone: DockZone,
    band: &[PanelId],
    active: PanelId,
    band_floating: bool,
    lang: Lang,
) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let (split, split_underline) = labelled(lang, "Split");
    entries.push(Entry::Row {
        label: split,
        underline: split_underline,
        check: None,
        cmd: Cmd::Split,
        // 左右带是上下均分，Split 没有语义；兼容终端浮动窗那一行就是灰的。
        disabled: zone != DockZone::Bottom,
    });
    // 文案恒为 "Float"（视觉基线两张菜单都是这一行）。停靠态浮整带；浮窗态
    // 拆出当前 tab——同一个词在不同位置做各自最直觉的那件事。
    let (float, float_underline) = labelled(lang, "Float");
    entries.push(Entry::Row {
        label: float,
        underline: float_underline,
        check: Some(band_floating),
        cmd: if band_floating {
            Cmd::Float(active)
        } else {
            Cmd::FloatBand(zone)
        },
        disabled: false,
    });
    entries.push(Entry::Sep);
    // 侧栏不在表里：它整高、占满一条带，塞不进底带（视觉基线那张表也没有它）。
    for id in PanelId::ALL.into_iter().filter(|p| *p != PanelId::Sidebar) {
        let on = band.contains(&id);
        let (label, underline) = labelled(lang, id.title());
        entries.push(Entry::Row {
            label,
            underline,
            check: Some(on),
            cmd: if on {
                Cmd::Uncheck(id, zone)
            } else {
                Cmd::Pull(id, zone)
            },
            disabled: false,
        });
    }
    entries
}

pub(in crate::ui) fn dock_menu(
    ui: &mut egui::Ui,
    gear: &egui::Response,
    menu: GearMenu<'_>,
    cmds: &mut Vec<Cmd>,
    lang: Lang,
) {
    // 夹具入口（不入库）：`WINDTERM_GEAR_DEMO=<PanelId::name()>` 强制摆开那颗齿轮的
    // 菜单，供 `--demo dock --screenshot` 出图。静态图没有鼠标，否则永远拍不到。
    let forced = std::env::var("WINDTERM_GEAR_DEMO").is_ok_and(|v| v == menu.active.name());
    if !forced && !ui.memory(|m| m.is_popup_open(menu_id(&menu))) {
        return;
    }
    let entries = gear_entries(menu.zone, menu.band, menu.active, menu.band_floating, lang);
    // 格宽取最长标签；勾选框占一栏（`indent`），两行命令也跟着右移，
    // 于是整张菜单共用一条文字竖线（与 `reserve_check_column` 同一判据）。
    let cells = entries
        .iter()
        .map(|e| match e {
            Entry::Row { label, .. } => crate::ui::shell::text_cells(label),
            Entry::Sep => 0,
        })
        .max()
        .unwrap_or(0);

    let mut selected = None;
    // 展开方向只看齿轮**实际在哪半屏**，不按 zone 硬编码：底带收回时齿轮在
    // 窗口底部（向上弹），展开时标签条被推到菜单栏下方、齿轮到了窗口顶部
    // （必须向下弹，否则菜单越过窗口顶沿、第一行和菜单栏叠在一起）。左右带
    // 的齿轮在面板标题栏（上半屏），仍是向下弹——与旧判据一致。
    let pop_up = gear.rect.center().y > ui.ctx().screen_rect().center().y;
    let (pos, pivot) = if pop_up {
        (gear.rect.right_top(), egui::Align2::RIGHT_BOTTOM)
    } else {
        (gear.rect.right_bottom(), egui::Align2::RIGHT_TOP)
    };
    let area = egui::Area::new(menu_id(&menu).with("area"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .pivot(pivot)
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .fill(shell::popup_bg())
                .stroke(egui::Stroke::new(1.0, crate::theme::chrome(CHROME_SCALE.border)))
                .show(ui, |ui| {
                    // 设的是**行宽**（`Frame` 之内），必须在 Frame 的 ui 上设：
                    // 设在 Frame 外面会被当成总宽，右侧 11 的内边距会把文字顶到边框。
                    ui.set_width(crate::ui::shell::rows_width(
                        ui,
                        cells,
                        crate::ui::shell::CHECK_GUTTER,
                    ));
                    crate::ui::shell::apply_popup_style(ui);
                    let look = crate::ui::shell::RowLook {
                        state: crate::ui::shell::MenuRowState::Enabled,
                        check: crate::ui::shell::CheckCell::None,
                        indent: true,
                        cells,
                    };
                    for entry in &entries {
                        match entry {
                            Entry::Sep => crate::ui::shell::menu_separator(ui),
                            Entry::Row { label, underline, check, cmd, disabled } => {
                                let resp = crate::ui::shell::draw_menu_row(
                                    ui,
                                    label,
                                    *underline,
                                    "",
                                    crate::ui::shell::RowLook {
                                        state: if *disabled {
                                            crate::ui::shell::MenuRowState::Disabled
                                        } else {
                                            crate::ui::shell::MenuRowState::Enabled
                                        },
                                        check: match check {
                                            Some(on) => crate::ui::shell::CheckCell::Box(*on),
                                            None => crate::ui::shell::CheckCell::None,
                                        },
                                        ..look
                                    },
                                    false,
                                );
                                if resp.clicked() {
                                    selected = Some(*cmd);
                                }
                            }
                        }
                    }
                });
        });

    if let Some(cmd) = selected {
        cmds.push(cmd);
        ui.memory_mut(|m| m.close_popup());
        return;
    }
    // 点在外面的任意位置 → 收起。点在菜单里或齿轮上不算。
    let rect = area.response.rect;
    let out = ui.input(|i| {
        i.pointer.any_click()
            && !i
                .pointer
                .interact_pos()
                .is_some_and(|p| rect.contains(p) || gear.rect.contains(p))
    });
    if out {
        ui.memory_mut(|m| m.close_popup());
    }
}

// ── 图标：绘图原语，不赌字体 ────────────────────────────────────────────

pub(super) fn btn_center(x: f32, strip_top: f32) -> egui::Pos2 {
    egui::pos2(x + BTN_W / 2.0, strip_top + TAB_H / 2.0)
}

/// `^`（展开态画 `v`）。视觉基线里是两条 45° 短线。
pub(super) fn paint_chevron(p: &egui::Painter, k: egui::Pos2, up: bool, c: egui::Color32) {
    // 兼容终端箭头更小（此前用 5px 半宽显得过大 → 3.5），用户再反馈仍偏大，收到
    // 3.0（总宽 6px），与齿轮（外径 10）/ 叉（6.5）保持同一量级。
    let a = 3.0;
    let dy = if up { a } else { -a };
    let st = egui::Stroke::new(1.0, c);
    p.line_segment(
        [egui::pos2(k.x - a, k.y + dy), egui::pos2(k.x, k.y - dy)],
        st,
    );
    p.line_segment(
        [egui::pos2(k.x, k.y - dy), egui::pos2(k.x + a, k.y + dy)],
        st,
    );
}

// 勾选框已随 `row()` 一起退役：齿轮菜单改走 `shell::draw_menu_row`，
// 勾的几何（11×11 井 + 折线，2026-11-14 由原 14×14 收紧）只在
// `shell/menu_style.rs` 里有一份。
//
// 2026-11-17：齿轮与 `×` 也退役了——它们原先按 `BTN_ICON` 的比例各算一套
// （`0.30` / `0.35`），与标签条、Find 栏那份画出来差半个点到一点，同一个窗口
// 里能量出三种尺寸。现在三处共用 `shell::glyph` 的一份几何（⚙ 10.0、× 6.5、
// 线宽 1.0），`dock_container` 直接从 `shell` 取，不再经这里转发。

#[cfg(test)]
mod tests {
    use super::*;

    /// 行表里所有「面板行」的 (名字, 是否勾着)。
    /// 按**命令类型**筛，不按有没有勾——现在 Float 行也带勾了。
    fn panel_rows(zone: DockZone, band: &[PanelId]) -> Vec<(String, bool, Cmd)> {
        gear_entries(zone, band, PanelId::Explorer, false, Lang::En)
            .into_iter()
            .filter_map(|e| match e {
                Entry::Row { label, check, cmd: cmd @ (Cmd::Pull(_, _) | Cmd::Uncheck(_, _)), .. } => {
                    check.map(|on| (label, on, cmd))
                }
                _ => None,
            })
            .collect()
    }

    fn labels(zone: DockZone, band: &[PanelId]) -> Vec<String> {
        gear_entries(zone, band, PanelId::Explorer, false, Lang::En)
            .into_iter()
            .map(|e| match e {
                Entry::Row { label, .. } => label,
                Entry::Sep => "---".to_string(),
            })
            .collect()
    }

    /// 兼容终端菜单三条带都有 Split 行，但左右带那行是**灰的**——左右带上下均分，
    /// 切一刀没有语义；禁用态既保留了原图那一行，又不会点了没反应。
    #[test]
    fn split_is_present_everywhere_but_disabled_outside_the_bottom_band() {
        let rows = |zone| gear_entries(zone, &[], PanelId::Explorer, false, Lang::En);
        for zone in [DockZone::Bottom, DockZone::Left, DockZone::Right] {
            let first = rows(zone).into_iter().next().unwrap();
            match first {
                Entry::Row { label, disabled, cmd, .. } => {
                    assert_eq!(label, i18n::t(Lang::En, "Split"));
                    assert_eq!(cmd, Cmd::Split);
                    assert_eq!(disabled, zone != DockZone::Bottom, "{zone:?} 的 Split 禁用态不符原图");
                }
                Entry::Sep => panic!("第一行不该是分隔线"),
            }
        }
    }

    /// 兼容终端每行文字都带加速键下划线（S̲plit / F̲loat / Ëxplorer / …），
    /// 位置按英文 key 的首字母；中文显示时划首字。
    #[test]
    fn every_menu_row_carries_its_access_key_underline() {
        let entries = gear_entries(DockZone::Right, &[], PanelId::Explorer, true, Lang::En);
        for entry in &entries {
            if let Entry::Row { label, underline, .. } = entry {
                assert_eq!(
                    *underline,
                    term_app::menu::access_char_pos(label),
                    "{label} 的下划线不在首字母"
                );
            }
        }
        let zh = gear_entries(DockZone::Right, &[], PanelId::Explorer, false, Lang::Zh);
        for entry in &zh {
            if let Entry::Row { underline, .. } = entry {
                assert_eq!(*underline, Some(0), "中文菜单划首字");
            }
        }
    }

    /// 兼容终端浮动窗那张菜单里 Float 行带勾（表示这条带正浮着），停靠带空壳里
    /// 同一行没有勾。
    #[test]
    fn the_float_row_checks_the_floating_state() {
        let rows = |floating| gear_entries(DockZone::Right, &[], PanelId::Explorer, floating, Lang::En);
        let check = |entries: Vec<Entry>| -> Option<bool> {
            entries.into_iter().find_map(|e| match e {
                Entry::Row { label, check, .. } if label == "Float" => check,
                _ => None,
            })
        };
        assert_eq!(check(rows(false)), Some(false));
        assert_eq!(check(rows(true)), Some(true));
    }

    /// 「Float」文案在视觉基线两张菜单里都一样；差别只在命令：停靠态浮整带，
    /// 浮窗态把当前 tab 单独拆出去。
    #[test]
    fn the_float_row_keeps_its_label_but_changes_scope_in_a_floating_band() {
        let docked = gear_entries(DockZone::Right, &[], PanelId::Explorer, false, Lang::En);
        let floating = gear_entries(DockZone::Right, &[], PanelId::Explorer, true, Lang::En);
        let row = |entries: Vec<Entry>| -> (String, Cmd) {
            entries
                .into_iter()
                .find_map(|e| match e {
                    Entry::Row { label, cmd, .. } if label == "Float" => Some((label, cmd)),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(row(docked), ("Float".to_string(), Cmd::FloatBand(DockZone::Right)));
        assert_eq!(row(floating), ("Float".to_string(), Cmd::Float(PanelId::Explorer)));
    }

    /// 勾选框说的是「在不在**这条**带里」，不是「显不显示」——
    /// 同一块面板在左带菜单里勾着、在底带菜单里没勾，才是「换个带」的表达。
    #[test]
    fn a_checkbox_follows_band_membership_not_visibility() {
        let in_left = panel_rows(DockZone::Left, &[PanelId::Explorer, PanelId::Filer]);
        let explorer = in_left
            .iter()
            .find(|(l, _, _)| l == i18n::t(Lang::En, "Explorer"))
            .expect("Explorer 必在表里");
        assert!(explorer.1, "Explorer 在左带，左带菜单里应勾着");
        let filer = in_left
            .iter()
            .find(|(l, _, _)| l == i18n::t(Lang::En, "Filer"))
            .expect("Filer 必在表里");
        assert!(filer.1);
        let session = in_left
            .iter()
            .find(|(l, _, _)| l == i18n::t(Lang::En, "Session"))
            .expect("Session 必在表里");
        assert!(!session.1, "Session 在右带，左带菜单里不该勾着");

        // 同一次问法换成底带：左带那两块都该是没勾的。
        let in_bottom = panel_rows(DockZone::Bottom, &[PanelId::Sender]);
        let explorer_b = in_bottom
            .iter()
            .find(|(l, _, _)| l == i18n::t(Lang::En, "Explorer"))
            .expect("Explorer 必在表里");
        assert!(!explorer_b.1, "Explorer 不在底带，底带菜单里不该勾着");
    }

    /// 勾着的行点下去是「离开这条带」，没勾的是「进这条带」，而且都带着
    /// 发命令的那条带——不然同一个命令在三条带里没法落地到不同目标。
    #[test]
    fn checking_pulls_and_unchecking_leaves_that_band() {
        for zone in [DockZone::Bottom, DockZone::Left, DockZone::Right] {
            let rows = panel_rows(zone, &[PanelId::Explorer]);
            for (_, on, cmd) in rows {
                match (on, cmd) {
                    (true, Cmd::Uncheck(id, z)) => {
                        assert_eq!(z, zone, "取消勾选要落在本条带");
                        assert!(id != PanelId::Sidebar, "侧栏不该出现在表里");
                    }
                    (false, Cmd::Pull(id, z)) => {
                        assert_eq!(z, zone, "勾上要拉进本条带");
                        assert!(id != PanelId::Sidebar, "侧栏不该出现在表里");
                    }
                    other => panic!("{zone:?} 里出现意外组合：{other:?}"),
                }
            }
        }
    }

    /// 侧栏永远不在表里：它整高、占满一条带，拉进底带会把它压成一条。
    ///
    /// 按**命令里的 `PanelId`** 判，不按标签判：`Sidebar.title()` 与
    /// `Session.title()` 都是 "Session"（会话树那块面板就叫 Session），
    /// 用标签会误判。
    #[test]
    fn the_sidebar_is_never_in_the_panel_list() {
        let all = panel_rows(DockZone::Left, &[]);
        for (_, _, cmd) in &all {
            let id = match cmd {
                Cmd::Pull(id, _) | Cmd::Uncheck(id, _) => *id,
                other => panic!("面板行里出现了非停靠命令：{other:?}"),
            };
            assert_ne!(id, PanelId::Sidebar, "侧栏不该出现在面板清单里");
        }
        assert_eq!(all.len(), PanelId::ALL.len() - 1);
    }
}
