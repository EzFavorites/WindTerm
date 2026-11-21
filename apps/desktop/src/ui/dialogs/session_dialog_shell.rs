//! 会话弹窗的**外框**：入口分派、系统独立窗口、页脚按钮。
//!
//!  从 `new_session.rs` 拆出来 ：那个文件涨到 578 代码行，
//! 而这一屏的「外框」与「正文」变化节奏本来就不同——正文是表单字段，
//! 外框是窗口层级（应用内模态 vs 系统窗口）。拆开之后正文那一份
//! `draw_new_session_body` 不动，外框改起来只看这一个文件。
//!
//! # 两种外框，不要合成一种（用户 两句明确指令）
//!
//! | 入口 | 外框 | 遮罩 |
//! |---|---|---|
//! | 新建会话 `New` | **系统独立窗口**（窗管标题栏 / 红黄绿） | 无——它自带外框 |
//! | 会话属性 `Edit` / `Properties` | 应用内模态层，**没有**自绘标题条 | 有 |
//! | 默认模板 `Default` | 同属性 | 有 |
//!
//! 遮罩的开关不在这里，在 `ui/mod.rs::modal_dialog_open`——那里才看得全
//! 「谁打开着」，本文件只回答「这一扇窗怎么画」。

use super::new_session::BTN_H;
use crate::ui::kit::window::{DialogId, DialogWindow};
use crate::ui::*;

/// 页脚按钮。视觉基线 `default-session-ssh-session.png` 实测：高 23、2pt 圆角、
/// 文字左右各留 15，最窄 74；主按钮（Save / Connect）是 `#3EA0EC` 蓝框 +
/// `#333A42` 蓝灰底，次按钮（Cancel）是 `#3C3C3C` 灰框 + `#262626` 灰底。
pub(in crate::ui) fn footer_button(ui: &mut egui::Ui, label: &str, primary: bool, enabled: bool) -> egui::Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let text_w = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, egui::Color32::WHITE)
        .size()
        .x;
    let w = (text_w + 30.0).max(74.0);
    let fill = if primary {
        egui::Color32::from_rgb(0x33, 0x3A, 0x42)
    } else {
        egui::Color32::from_rgb(0x26, 0x26, 0x26)
    };
    let stroke = if primary {
        egui::Color32::from_rgb(0x3E, 0xA0, 0xEC)
    } else {
        egui::Color32::from_rgb(0x3C, 0x3C, 0x3C)
    };
    ui.add_enabled_ui(enabled, |ui| {
        ui.add_sized(
            egui::vec2(w, BTN_H),
            egui::Button::new(label)
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, stroke))
                .rounding(2.0),
        )
    })
    .inner
}

impl TerminalWindow {
    /// 弹窗入口：外框交给 harness，内容交给 `draw_new_session_body`。
    ///
    /// 这里曾经有 25 行手写的「空内容热身」补丁（sizing pass / fade_in /
    /// fixed_rect / ID 冲突四件事混在一起）。它们全在
    /// `harness::StableWindow::show` 里，只有一份——弹窗皮肤再改时不必
    /// 每处各补一遍，下一个弹窗也不用重新踩那四轮。
    pub(in crate::ui) fn draw_new_session_dialog(&mut self, ctx: &egui::Context) {
        // 标题 / 按钮 / 保存语义全由 `session_dialog` 这一个枚举分派：
        // New → "New Session" / Edit → "Session Properties" / Default →
        // "Edit Default Session"。
        let kind = self.session_dialog.clone();
        // **两种外框，不要合成一种**（用户对比图）：
        //
        // - 新建会话在兼容终端里是一扇**独立的系统窗口**：标题栏、红黄绿、最小化
        //   都归窗管，与本仓浮窗同一套多视口管线（`layout/dock_native.rs`）。
        // - 属性 / 默认模板是**应用内模态层**，与 Preferences 一样没有自绘标题条
        //   （没有 `‹Close›` 那种右上角 ×），唯一的关闭入口是页脚按钮。
        //
        // 此前两者共用 `DialogWindow` 的自绘标题条，于是「新建会话」缺了系统
        // 外框、「会话属性」多了一条兼容终端没有的标题栏——两边都不对。
        if matches!(kind, SessionDialogKind::New { .. }) {
            self.show_new_session_window(ctx, &kind);
            return;
        }
        let open = DialogWindow::new(DialogId::NewSession, kind.title())
            .without_title_bar()
            .tight()
            .show(ctx, |ui| self.draw_new_session_body(ui, &kind));
        if !open {
            self.close_session_dialog();
        }
    }

    /// 新建会话：**系统独立窗口**，正文没有我们自己的标题条。
    ///
    /// 两级回退与浮窗一致：
    /// 1. **夹具**（`--screenshot` / 组件出图）直接走应用内弹窗——多视口是真的
    ///    开第二扇系统窗口，而截图只拍 root，弹窗会整块从图里消失。
    /// 2. **后端不支持多视口**：`show_viewport_immediate` 会以
    ///    `ViewportClass::Embedded` 回调，同一条应用内路径接管。
    ///
    /// 两条回退都**保留自绘标题条**：那正是既有基线图拍到的样子，也是没有系统
    /// 外框时唯一能关窗的地方。
    fn show_new_session_window(&mut self, ctx: &egui::Context, kind: &SessionDialogKind) {
        let size = DialogId::NewSession.size();
        let embedded = |win: &mut TerminalWindow, ctx: &egui::Context| {
            let open = DialogWindow::new(DialogId::NewSession, kind.title())
                .tight()
                .show(ctx, |ui| win.draw_new_session_body(ui, kind));
            if !open {
                win.close_session_dialog();
            }
        };
        if harness::fixture_mode() {
            embedded(self, ctx);
            return;
        }

        let title = self.tr(kind.title());
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size(size)
            .with_min_inner_size(size)
            .with_max_inner_size(size)
            // 内容按 640×596 的固定网格排（`BODY_H` 等常量），窗口能拉大而
            // 内容不跟着重排，只会露出一圈空白，所以尺寸钉死。
            .with_resizable(false)
            .with_active(true)
            .with_position(crate::ui::kit::window::centered_outer_position(ctx, size));
        // 刚建出来的系统窗口在 macOS 上可能埋在主窗口后面（浮窗同一个实测
        // 问题），前几帧显式要一次前台；窗口关掉时把额度清零。
        //
        // 「OneKeys 刚关掉」也要补一次：会话页 `OneKey:` 行里的齿轮会把主窗口
        // 提到前面（见下），用户点完 Ok 之后该回来的是这扇窗。
        let focus_id = egui::Id::new("session_dialog_focus_tries");
        let was_id = egui::Id::new("session_dialog_onekeys_was_open");
        let back_from_onekeys = {
            let was = self.ctx.data(|d| d.get_temp::<bool>(was_id).unwrap_or(false));
            self.ctx
                .data_mut(|d| d.insert_temp(was_id, self.view.show_onekeys));
            was && !self.view.show_onekeys
        };
        let tries = if back_from_onekeys {
            // 额度重置：从 OneKeys 回来时这扇窗已经不在前台了，得重新要。
            self.ctx.data_mut(|d| d.insert_temp(focus_id, 0u8));
            0
        } else {
            self.ctx.data(|d| d.get_temp::<u8>(focus_id).unwrap_or(0))
        };
        let ask_focus = tries < 3;
        if ask_focus {
            self.ctx.data_mut(|d| d.insert_temp(focus_id, tries + 1));
        }

        let mut closed = false;
        let embedded_fallback = ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("session_dialog_new"),
            builder,
            |ctx, class| {
                if matches!(class, egui::ViewportClass::Embedded) {
                    return true;
                }
                if ask_focus {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                // 会话页 `OneKey:` 行的齿轮打开的是 OneKeys 编辑页，它画在
                // **主窗口**里：那一页内容 684×653，比这扇 640×596 的窗还大，
                // 塞进来会把页脚的 Cancel / Ok 裁掉——一个关不掉的弹窗。
                // 所以这里把主窗口提到前面，等它关掉再由上面的额度把这扇窗提回来。
                if self.view.show_onekeys {
                    ctx.send_viewport_cmd_to(
                        egui::ViewportId::ROOT,
                        egui::ViewportCommand::Focus,
                    );
                }
                closed = ctx.input(|i| i.viewport().close_requested());
                egui::CentralPanel::default()
                    .frame(crate::ui::kit::window::dialog_surface())
                    .show(ctx, |ui| {
                        ui.set_width(size.x);
                        self.draw_new_session_body(ui, kind);
                    });
                false
            },
        );
        if embedded_fallback {
            embedded(self, ctx);
            return;
        }
        if closed {
            self.close_session_dialog();
        }
    }
}
