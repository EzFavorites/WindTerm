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
            // 锁屏是凭据边界：后台隧道 / Filer 仍可调用 resolve_auth，
            // 必须把运行时明文与重建副本一并清掉，否则锁屏期间还能重建密码。
            crate::ui::dialogs::vault::clear_runtime_secrets();
            // 锁屏是**用户键盘输入**的边界，但不是「后台脚本与界面之间的
            // 非键盘往返」的边界：触发器脚本不会因为屏幕锁了就停。若把这几条
            // 队列连同整帧一起跳过，脚本的 app_* 会在 2 秒后误报 Timeout，
            // 脚本面板也会卡在「在跑」。它们不画模态、不吃按键，先消费掉。
            // `poll_script_ask` 仍留在解锁后的帧：ask/pick 需要用户应答，
            // 锁屏期间弹模态会破坏输入边界。
            self.drain_app_requests();
            self.drain_script_activated();
            self.poll_script();
            self.poll_script_panel();
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
        // 同一帧可能既有启动期迁移丢弃项，又有写盘失败（两者都只提示一次）。
        // 先都取到局部表里再拼成一条：过去后者直接覆盖前者，且前者已被 take
        // 掉，那条提示就永久丢了。两个槽都取走即清空，不会下一帧重复弹。
        let mut notices = Vec::new();
        if let Some(note) = crate::ui::dialogs::shortcuts::take_startup_migration_notice() {
            notices.push(note);
        }
        if let Some(err) = term_profile::prefs::last_save_error() {
            notices.push(err.message().to_string());
            term_profile::prefs::clear_last_save_error();
        }
        if !notices.is_empty() {
            // commit_note 同时抬到前景 Notice（错误类不会被普通提示冲掉）。
            self.commit_note(Some(notices.join("\n")));
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
