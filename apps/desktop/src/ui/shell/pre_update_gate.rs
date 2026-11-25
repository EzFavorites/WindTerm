//! 每帧**统一前置阶段**：锁屏输入边界 / 模态键盘吞入 / 全局快捷键门控 /
//! 偏好版本号同步。
//!
//! 这些事过去散在 `mod.rs::draw` 顶部，顺序还不对——`handle_global_shortcuts`
//! 和 `sync_remote_sysmon` 都跑在 `lock.locked` 判断之前，锁着的时候快捷键照样
//! 开火、远端 Sysmon 照样采样。收口到一处之后，`draw` 顶部只剩一次调用，
//! 锁屏成为真正的输入边界（只让 `draw_lock` 处理自己的键）。

use eframe::egui;

use crate::ui::*;

impl TerminalWindow {
    /// 帧首的一次性前置处理。返回 `true` = 本帧已经画完（锁屏），`draw` 直接收尾。
    pub(in crate::ui) fn pre_update_gate(&mut self, ctx: &egui::Context) -> bool {
        // 空闲超时先判：这一帧刚派发的 Lock 立刻生效，不留一帧空窗。
        self.poll_lock_timeout(ctx);
        if self.view.lock.locked {
            // 锁屏是输入边界：不派发全局快捷键、不吞键、不采样远端 Sysmon。
            // 密码框需要的 Enter 与字符由 `draw_lock` 自己处理。
            self.draw_lock(ctx);
            return true;
        }
        self.sync_remote_sysmon(ctx);
        self.confirm_pre_update(ctx);
        self.paste_pre_update(ctx);
        self.handle_global_shortcuts(ctx);
        self.sync_prefs_rev(ctx);
        self.eat_modal_keys(ctx);
        false
    }

    /// 把「盘上偏好写过了」同步成窗口上的 `prefs_rev`。
    ///
    /// `save_prefs` 的调用点散在十几个文件里，逐处 `self.prefs_rev += 1` 既改不完
    /// 也会漏。写口 `term_profile::prefs::save` 在成功时推进全局 generation，
    /// 这里每帧只取一次——版本号因此仍是「每次成功写入 +1」，但只归正在绘制的
    /// 窗口所有：测试里直接 `app_snapshot()` 不会读到别的用例写盘留下的数字。
    pub(in crate::ui) fn sync_prefs_rev(&mut self, ctx: &egui::Context) {
        let generation = term_profile::prefs::write_generation();
        if generation != self.prefs_rev {
            let bumped = generation > self.prefs_rev;
            self.prefs_rev = generation;
            if bumped {
                ctx.request_repaint();
            }
        }
        // 启动期快捷键迁移丢弃项（越界旧下标 / 冲突用户绑定）只提示一次。
        if let Some(note) = crate::ui::dialogs::shortcuts::take_startup_migration_notice() {
            self.log_note = Some(note);
            ctx.request_repaint();
        }
        // 写盘失败也在这一处上屏：光返回 bool 没人看，等于冲突静默丢掉。
        // 取走即清空，下一次正常保存之后不会还挂着上次的「请重做」。
        if let Some(err) = term_profile::prefs::last_save_error() {
            self.log_note = Some(err.message().to_string());
            term_profile::prefs::clear_last_save_error();
            ctx.request_repaint();
        }
    }

    /// 模态期间的键盘收口：灰罩只吞鼠标，键盘在这里按**白名单**统一处理。
    ///
    /// 白名单里的弹窗带 `TextEdit`，键盘必须原样交给它们（连 Esc/Enter 都由
    /// 控件自己的 `DialogKeys` 契约解释）；不在白名单的纯确认弹窗独占键盘，
    /// 剩下的 `Key` / `Text` / `Paste` 一律吃掉，不让它穿过灰罩漏进终端。
    fn eat_modal_keys(&self, ctx: &egui::Context) {
        if !should_eat(self.modal_dialog_open(), keyboard_whitelist(self)) {
            return;
        }
        ctx.input_mut(|i| {
            i.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::Paste(_)
                )
            });
        });
    }
}

/// 吞键盘的唯一判据：有模态、且那个模态不在「键盘归自己」的白名单里。
fn should_eat(modal: bool, whitelisted: bool) -> bool {
    modal && !whitelisted
}

/// 带输入框 / 需要自己解释键盘的模态。漏一个，那个弹窗的打字就废了——
/// 所以清单按「有 TextEdit 或自定义键盘契约」逐项列全，而不是按 `DialogId` 猜。
fn keyboard_whitelist(win: &TerminalWindow) -> bool {
    let v = &win.view;
    v.show_settings
        || v.show_new_session
        || v.show_triggers
        || v.show_onekeys
        || v.show_quick_commands
        || v.show_shortcuts
        || v.show_log_settings
        || v.show_preview
        || v.show_profile_choice
        || win.pending_paste.is_some()
        || win.xfer_prompt.is_some()
        || win.script_ask.is_some()
        || win.connector.is_some()
        || win.tunnel_editor.is_some()
        || win.filer.has_modal()
        || win.script_manager.is_some()
        || win.script_editor.as_ref().is_some_and(|s| s.open)
        || win.onekeys_edit.as_ref().is_some_and(|e| e.new_scheme_modal)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 灰罩吞鼠标不吞键盘：只有「模态 + 不在白名单」这一格才吞键盘。
    /// 白名单覆盖不全时，带输入框的弹窗会被这一刀割掉——四格里那两格必须保持 false。
    #[test]
    fn only_a_non_whitelisted_modal_eats_the_keyboard() {
        assert!(should_eat(true, false));
        assert!(!should_eat(true, true));
        assert!(!should_eat(false, false));
        assert!(!should_eat(false, true));
    }
}
