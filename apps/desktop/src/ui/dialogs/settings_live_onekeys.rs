//! `Terminal.ManageOneKeys` 的对话框内容：登录方案管理器（兼容终端 Tool ▸ OneKeys…）。
//!
//! # 1:1 对齐（用户要求「必须 1:1，不接受大概」）
//!
//! 所有几何值都是**实测**：视觉基线是一张 1408×1314 的 @2× 截图，
//! 物理像素 ÷2 得逻辑点。
//!
//! ```text
//! 逻辑点(相对窗口左上)                 物理像素(@2×)
//! 窗口 702 × 655                      2..1406 × 2..1312
//! 标签条 y 1..24（高 23）              2..48
//! 主体   y 34..594（高 560）           68..1188
//! 左栏框 x 9..266（宽 257）            19..533
//! 右栏   x 274..695（宽 421）          547..1389
//! 工具行 y 595..614（高 19）           1190..1228
//! 页脚   y 615..656（高 41）           1230..1312
//! ```
//!
//! 右栏各元素的 y（相对主体顶，实测）：
//!
//! ```text
//! Username 标签 3..11     输入框 21..45（高 24）
//! Password 标签 55..63    输入框 73..97
//! Public Key 标签 106..115
//! 表1 125..319（表头 18，三行各 19.5）
//! Keyboard Interactive 标签 348..357
//! 表2 367..560
//! ```
//!
//! 因此这一版**不用 flow 布局**：`add_space` 叠出来的位置会随字体度量漂移，
//! 全部改成「先算矩形、再往矩形里画」（见 [`place`]）。
//!
//! # 草稿模型（收口）
//!
//! 之前是**就地改** `TerminalWindow.onekeys`：每帧把选中项 clone 出来、改完再写
//! 回去。于是 `Cancel` 只关窗口不还原——用户点了 Cancel，改动留在内存里，下一次
//! 进来 `Ok` 就一起存了。语义上「取消」变成了「稍后保存」。
//!
//! 现在是一份 `draft: Vec<OneKey>`：整份编辑都发生在草稿上，`Ok` 才写回
//! `win.onekeys` 并落盘，`Cancel` 直接丢掉草稿。这也给 `Rename` 落了地——原先
//! 它只往 `ctx.data` 塞一个 `onekey_renaming = true`，而**全仓没有读者**，点了
//! 什么都不会发生（明令禁止这种没有效果的假按钮）。
//!
//! # 接线
//!
//! - 编辑结果只改 `OneKeysEdit::draft`；
//! - `Ok` 把草稿写回 `prefs.onekeys` 并关闭；`Cancel` 只关闭；
//! - 密码与键盘交互应答都属凭据，**明文不进 prefs.json**（vault 以
//!   `onekey:<name>` 为 id），锁着时 `Ok` 禁用。

use super::dialog_chrome::{framed_button, place, plain_button, rgb, tiny_button};
use super::settings_live_onekeys_pages::{account_editor, sessions_editor};
use super::settings_live_onekeys_ssh::ssh_editor;
use crate::ui::*;
use term_core::onekey::OneKey;

// ── 实测几何（逻辑点） ──
pub(in crate::ui) const DIALOG_W: f32 = 702.0;
pub(in crate::ui) const DIALOG_H: f32 = 655.0;
const PAD: f32 = 9.0;
const TAB_H: f32 = 23.0;
const TAB_GAP: f32 = 10.0;
const BODY_H: f32 = 560.0;
const LIST_W: f32 = 257.0;
const COL_GAP: f32 = 8.0;
pub(in crate::ui) const FIELD_W: f32 = 420.0;
const TOOL_H: f32 = 19.0;
const FOOT_H: f32 = 41.0;

/// 本对话框的三分类（顶部标签），与视觉基线「SSH / Account / Sessions」同序。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::ui) enum Tab {
    #[default]
    Ssh,
    Account,
    Sessions,
}

/// 对话框的编辑态（页面级局部状态，不是领域字段）。
#[derive(Default)]
pub(in crate::ui) struct OneKeysEdit {
    pub(in crate::ui) selected: Option<usize>,
    pub(in crate::ui) tab: Tab,
    /// 「显示密码」开关（视觉基线 Password 行右端那颗圆点）。
    pub(in crate::ui) show_password: bool,
    /// 方案的**草稿**。`None` = 本帧刚打开、尚未从 `win.onekeys` 拷过来。
    ///
    /// 放在窗口上而不是每帧重建：`TextEdit` 的光标与滚动位置要跨帧稳定。
    /// 生命周期与对话框一致——`Ok` 整份写回或 `Cancel` 整份丢弃之后都会把它清掉
    /// （`win.onekeys_edit = None`），下次进来重新从落盘值拷贝。
    pub(in crate::ui) draft: Option<Vec<OneKey>>,
    /// 左列表正在重命名（`Rename` 之后、提交之前）。
    pub(in crate::ui) renaming: bool,
    /// 重命名的输入缓冲。
    pub(in crate::ui) rename_buf: String,
    /// Account 页密码组 Send 框的明文开关（眼睛图标）。
    pub(in crate::ui) show_password_send: bool,
    /// Sessions 页 Filter 框的跨帧缓冲。
    pub(in crate::ui) session_filter: String,
    /// SSH 页两张表是否处于 `Modify` 编辑态（视觉基线默认是只读，只有链接）。
    pub(in crate::ui) key_editing: bool,
    pub(in crate::ui) ki_editing: bool,
}

fn new_name(existing: &[OneKey]) -> String {
    let mut n = 1;
    loop {
        let candidate = format!("onekey_{n}");
        if !existing.iter().any(|k| k.name == candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// 方案名合法吗：非空、无首尾空白、不与**其它**方案重名。
///
/// 重名必须挡掉：连接时是按 `SessionConfig.onekey` 的**名字**找方案的，两个同名
/// 方案等于其中一个永远取不到（`iter().find()` 只命中第一个）。
fn name_ok(draft: &[OneKey], at: usize, name: &str) -> bool {
    let n = name.trim();
    !n.is_empty()
        && n == name
        && !draft
            .iter()
            .enumerate()
            .any(|(i, k)| i != at && k.name == name)
}

/// OneKeys 对话框的入口：标签条 + 左列表 + 右字段 + 页脚，全部按实测坐标摆。
pub(in crate::ui) fn onekeys(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    // 字号对齐视觉基线：实测正文墨迹高约 8.5 逻辑点 → 12px 字体。
    // egui 默认 Button/Body 是 14，整块看起来比兼容终端粗大一圈。
    // 统一后字号取自 type_scale::SIZE_BODY，字体族走那里定义的全 UI 等宽。
    crate::ui::kit::type_scale::install_sized_styles(
        ui.style_mut(),
        crate::ui::kit::type_scale::SIZE_BODY,
    );
    let mut edit = win.onekeys_edit.take().unwrap_or_default();
    // 首次进来从落盘值拷一份草稿；之后每帧都在这份草稿上改。
    if edit.draft.is_none() {
        edit.draft = Some(win.onekeys.clone());
    }
    let mut save = false;
    let mut close = false;
    let mut toggle_lock = false;
    let mut duplicate: Option<usize> = None;

    let content_w = DIALOG_W - PAD * 2.0;
    let origin = ui.cursor().min;

    // ── 标签条：三等分；选中块比未选更暗（与内容区同色） ──
    // 标签条**贴满窗口**（兼容终端物理 x 2..1406，没有左右内边距），所以起点要
    // 退回到 PAD 之外、宽度补回两侧。
    let tab_w = (content_w + PAD * 2.0) / 3.0;
    place(
        ui,
        egui::Rect::from_min_size(
            origin - egui::vec2(PAD, 0.0),
            egui::vec2(content_w + PAD * 2.0, TAB_H),
        ),
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.horizontal(|ui| {
                for (tab, label) in [
                    (Tab::Ssh, "SSH"),
                    (Tab::Account, "Account"),
                    (Tab::Sessions, "Sessions"),
                ] {
                    if super::settings_pages_view::dialog_tab(ui, tab_w, TAB_H, label, edit.tab == tab)
                        .clicked()
                    {
                        edit.tab = tab;
                    }
                }
            });
        },
    );

    // ── 主体 ──
    // Sessions 页在视觉基线里是**整宽**表格，没有左栏列表和工具行；
    // SSH / Account 两页才是「左列表 + 右字段 + 工具行」。
    let is_sessions = edit.tab == Tab::Sessions;
    let body_y = if is_sessions { TAB_H + 3.0 } else { TAB_H + TAB_GAP };
    let body_h = if is_sessions { 614.0 - body_y } else { BODY_H };
    let body = egui::Rect::from_min_size(
        origin + egui::vec2(0.0, body_y),
        egui::vec2(content_w, body_h),
    );
    if is_sessions {
        place(ui, body, |ui| {
            let key = edit
                .selected
                .and_then(|i| edit.draft.as_ref()?.get(i))
                .cloned()
                .unwrap_or_default();
            sessions_editor(win, ui, &key, &mut edit.session_filter);
        });
    } else {
        write_body(ui, body, win, &mut edit);
    }

    let tool_y = body.max.y;
    // ── 左栏工具行：**独立的一个框**（上下各一条横线把按钮组框住），
    // 与 Triggers 的两条工具条同形；不再被左侧大框吞掉。
    let strip = egui::Rect::from_min_size(egui::pos2(body.min.x, tool_y), egui::vec2(LIST_W, TOOL_H));
    if !is_sessions {
        ui.painter()
            .rect_stroke(strip, egui::Rounding::ZERO, egui::Stroke::new(0.5, rgb(0x444444)));
        let can_edit = edit
            .selected
            .zip(edit.draft.as_ref())
            .is_some_and(|(i, d)| i < d.len());
        place(ui, strip.shrink2(egui::vec2(10.0, 0.0)), |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                if tiny_button(ui, "+").clicked() {
                    if let Some(draft) = edit.draft.as_mut() {
                        let name = new_name(draft);
                        draft.push(OneKey { name, ..OneKey::default() });
                        edit.selected = Some(draft.len() - 1);
                        edit.renaming = false;
                    }
                }
                // `-` 删的是**当前选中**项；没有选中就置灰可辨（`tiny_button` 在
                // disabled 的 ui 里自动只收 hover，见它的实现）。
                let del = place(
                    ui,
                    egui::Rect::from_min_size(ui.cursor().min, egui::vec2(19.0, 16.0)),
                    |ui| {
                        ui.add_enabled_ui(can_edit, |ui| tiny_button(ui, "-")).inner
                    },
                );
                if del.clicked() {
                    if let Some(draft) = edit.draft.as_mut() {
                        if let Some(i) = edit.selected.filter(|i| *i < draft.len()) {
                            draft.remove(i);
                            edit.selected = None;
                            edit.renaming = false;
                        }
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // 两个纯文字按钮必须留缝：`plain_button` 的响应矩形就是文字宽度，
                    // 间距为 0 时渲染出来是 `DuplicateRename` 连成一串。
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if plain_button(ui, "Rename", can_edit).clicked() {
                        if let Some(name) = edit
                            .selected
                            .and_then(|i| edit.draft.as_ref()?.get(i))
                            .map(|k| k.name.clone())
                        {
                            edit.renaming = true;
                            edit.rename_buf = name;
                        }
                    }
                    if plain_button(ui, "Duplicate", can_edit).clicked() {
                        // 结构变更不能在这里直接 push：`edit.draft` 还被本帧的列表
                        // 迭代借着，统一攒到帧末（与 `-` 的删一致）。
                        duplicate = edit.selected;
                    }
                });
            });
        });
    }

    // ── 页脚 ──
    let foot_y = if is_sessions { tool_y } else { tool_y + TOOL_H };
    place(
        ui,
        egui::Rect::from_min_size(
            egui::pos2(origin.x, foot_y),
            egui::vec2(content_w, FOOT_H),
        ),
        |ui| write_footer(ui, win, &mut save, &mut close, &mut toggle_lock),
    );
    // 页脚的黄色锁是真入口：锁着 → 复用锁屏问主密码；开着 → 直接锁回去。
    if toggle_lock {
        if win.vault_unlocked() {
            win.lock_vault();
        } else {
            win.dispatch_action(Action::Lock(term_app::vocab::LockAction::Lock));
        }
    }

    // ── 帧末：草稿上的复制（删除已在按钮处就地做掉） ──
    if let (Some(i), Some(draft)) = (duplicate, edit.draft.as_mut()) {
        if i < draft.len() {
            let mut copy = draft[i].clone();
            copy.name = new_name(draft);
            draft.push(copy);
            edit.selected = Some(draft.len() - 1);
        }
    }

    if close {
        // 草稿要么**落地**（Ok），要么**整份丢弃**（Cancel）。之前两者都只关窗口，
        // 于是 Cancel 之后改动还留在 `win.onekeys` 里，下次 Ok 一起存。
        if save {
            if let Some(draft) = edit.draft.take() {
                win.onekeys = draft;
            }
            win.save_onekeys();
        } else {
            // Cancel 必须把**密码草稿**也丢掉。它是 `win` 上的字段，不在这份
            // `edit` 里；留着的话，下次任意一次 Ok 都会 `save_onekeys()`，
            // 把这次已经取消的输入写进保险库——用户以为取消了，凭据却落了盘。
            win.onekeys_password_draft = None;
        }
        win.view.show_onekeys = false;
        // 丢掉整份编辑态：下次打开重新从落盘值拷贝草稿。
        win.onekeys_edit = None;
        win.ctx.request_repaint();
    } else {
        win.onekeys_edit = Some(edit);
    }
    // 内容全是绝对定位，不占流式高度——把光标推到整块内容之下。
    ui.advance_cursor_after_rect(egui::Rect::from_min_size(
        origin,
        egui::vec2(
            content_w,
            body_y + body_h + if is_sessions { 0.0 } else { TOOL_H } + FOOT_H,
        ),
    ));
}

/// 主体：左栏带边框列表 + 右栏字段区。
fn write_body(ui: &mut egui::Ui, body: egui::Rect, win: &mut TerminalWindow, edit: &mut OneKeysEdit) {
    let list = egui::Rect::from_min_size(body.min, egui::vec2(LIST_W, BODY_H));
    // 列表边框实测 (68,68,68)，比 `line` 亮一档。
    ui.painter()
        .rect_stroke(list, egui::Rounding::ZERO, egui::Stroke::new(1.0, rgb(0x444444)));
    place(ui, list.shrink(2.0), |ui| {
        let Some(draft) = edit.draft.as_mut() else {
            return;
        };
        if draft.is_empty() {
            ui.add_space(BODY_H / 2.0 - 14.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("No onekey available").color(rgb(0x555555)));
            });
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("onekeys_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for i in 0..draft.len() {
                    // 重命名态：这一行换成输入框；回车/失焦提交，Esc 放弃。
                    if edit.renaming && edit.selected == Some(i) {
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut edit.rename_buf)
                                .desired_width(LIST_W - 8.0)
                                .hint_text("name"),
                        );
                        if !r.has_focus() && !r.lost_focus() {
                            r.request_focus();
                        }
                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            edit.renaming = false;
                        } else if r.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            // 名字不合法（空/重名）就**保持编辑态**，别静默丢掉用户
                            // 刚敲的字——静默失败是这类内联改名最常见的坑。
                            if name_ok(draft, i, &edit.rename_buf) {
                                draft[i].name = edit.rename_buf.clone();
                                edit.renaming = false;
                            }
                        }
                        continue;
                    }
                    if ui
                        .selectable_label(edit.selected == Some(i), &draft[i].name)
                        .clicked()
                    {
                        edit.selected = Some(i);
                        edit.renaming = false;
                    }
                }
            });
    });

    let right = egui::Rect::from_min_size(
        egui::pos2(list.max.x + COL_GAP, body.min.y),
        egui::vec2(FIELD_W, BODY_H),
    );
    place(ui, right, |ui| {
        // 视觉基线里**列表为空时右栏照样是完整字段区**（`No onekey available` 只在
        // 左栏）。所以这里永远渲染字段：没有选中就用一份临时 `OneKey` 并把整块
        // 置灰——视觉与视觉基线一致，同时不假装可编辑。
        let idx = edit
            .selected
            .filter(|i| edit.draft.as_ref().is_some_and(|d| *i < d.len()));
        let mut key = idx
            .and_then(|i| edit.draft.as_ref().map(|d| d[i].clone()))
            .unwrap_or_default();
        // 不再用 `add_enabled_ui(false)` 置灰：视觉基线在「没有选中方案」时字段
        // 仍是正常亮度。无选中时这里改的是临时 `key`，帧末只写回 draft[i]，
        // idx 为 None 就直接丢弃——不可持久，但观感与视觉基线一致。
        match edit.tab {
            Tab::Ssh => ssh_editor(win, ui, &mut key, edit),
            Tab::Account => account_editor(win, ui, &mut key, &mut edit.show_password_send),
            Tab::Sessions => sessions_editor(win, ui, &key, &mut edit.session_filter),
        }
        // 「Click the lock to make changes」是硬约束：锁着时页面可以照常看，
        // 但不能把改动落到草稿里。没有方案时例外——那时要能填出第一条。
        let can_change = win.vault_unlocked() || win.onekeys.is_empty();
        if can_change {
            if let Some(i) = idx {
                if let Some(draft) = edit.draft.as_mut() {
                    if key != draft[i] {
                        draft[i] = key;
                    }
                }
            }
        }
    });
}

/// 页脚那把锁：几何画出（视觉基线是单色 `#FF8400` 铜锁，不是彩色 emoji），
/// 整行可点。下划线落在 `lock` 上，和视觉基线的键盘加速器写法一致。
fn lock_hint(ui: &mut egui::Ui, unlocked: bool) -> bool {
    let tail = if unlocked { "prevent" } else { "make" };
    let color = rgb(if unlocked { 0x6BBF59 } else { 0xFF8400 });
    let font = egui::TextStyle::Button.resolve(ui.style());
    let plain = egui::TextFormat::simple(font.clone(), color);
    let mut job = egui::text::LayoutJob::default();
    job.append("Click the ", 0.0, plain.clone());
    job.append(
        "lock",
        0.0,
        egui::TextFormat { underline: egui::Stroke::new(1.0, color), ..plain.clone() },
    );
    job.append(&format!(" to {tail} changes."), 0.0, plain);
    let galley = ui.fonts(|f| f.layout_job(job));
    let rect = egui::Rect::from_min_size(
        ui.cursor().min,
        egui::vec2(28.0 + galley.size().x, FOOT_H - 8.0),
    );
    let resp = ui.allocate_rect(rect, egui::Sense::click());
    let p = ui.painter_at(rect);
    let center = rect.min + egui::vec2(11.0, rect.height() / 2.0);
    // 锁身：实心圆角方块；锁梁：开口朝右的圆环（解锁态挪一点表示弹开）。
    p.rect_filled(
        egui::Rect::from_center_size(center + egui::vec2(0.0, 2.5), egui::vec2(11.0, 8.0)),
        egui::Rounding::same(1.0),
        color,
    );
    p.circle_stroke(
        center + egui::vec2(if unlocked { 2.0 } else { 0.0 }, -3.0),
        3.5,
        egui::Stroke::new(1.0, color),
    );
    p.galley(
        rect.min + egui::vec2(28.0, (rect.height() - galley.size().y) / 2.0),
        galley,
        color,
    );
    resp.on_hover_text(if unlocked { "Lock to prevent changes" } else { "Unlock to make changes" })
        .clicked()
}

/// 页脚：可点击的锁 + 提示居左，`Cancel` `Ok` 居右。
fn write_footer(
    ui: &mut egui::Ui,
    win: &TerminalWindow,
    save: &mut bool,
    close: &mut bool,
    toggle_lock: &mut bool,
) {
    let unlocked = win.vault_unlocked();
    ui.horizontal_centered(|ui| {
        if lock_hint(ui, unlocked) {
            *toggle_lock = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let can_save = unlocked || win.onekeys.is_empty();
            if framed_button(ui, "Ok", can_save, true).clicked() {
                *save = true;
                *close = true;
            }
            if framed_button(ui, "Cancel", true, false).clicked() {
                *close = true;
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> OneKey {
        OneKey {
            name: name.to_string(),
            ..OneKey::default()
        }
    }

    /// 重命名是**按名字**找方案的入口，重名必须挡；自己改自己（名字没变）要放行。
    #[test]
    fn a_rename_only_lands_on_a_unique_non_blank_name() {
        let draft = vec![key("alpha"), key("beta")];
        // 撞到隔壁方案：拒绝。
        assert!(!name_ok(&draft, 0, "beta"));
        // 改成自己的名字：放行（不算重名）。
        assert!(name_ok(&draft, 0, "alpha"));
        // 空 / 全空白 / 带首尾空白：拒绝——名字是持久化主键，空白会漂。
        assert!(!name_ok(&draft, 0, ""));
        assert!(!name_ok(&draft, 0, "   "));
        assert!(!name_ok(&draft, 0, " gamma"));
        // 全新且合法的名字：放行。
        assert!(name_ok(&draft, 0, "gamma"));
    }

    /// `+` 与 `Duplicate` 都靠它起名：必须跳过已占用的 `onekey_N`，否则新方案
    /// 一生成就是重名（重名方案里第二个永远取不到密码）。
    #[test]
    fn a_generated_name_skips_the_ones_already_taken() {
        assert_eq!(new_name(&[]), "onekey_1");
        assert_eq!(new_name(&[key("onekey_1"), key("onekey_2")]), "onekey_3");
        // 中间空出来的号可以复用：列表里没有就算没占用。
        assert_eq!(new_name(&[key("onekey_1"), key("onekey_3")]), "onekey_2");
    }
}
