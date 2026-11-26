//! OneKeys 组件夹具的逐张出图状态（从 `mod.rs` 拆出，守住代码行上限）。
//!
//! 全仓只走 `harness::fixture_mode()`，生产路径不读这些环境变量。
//! 每张参考图 `07-dialogs/onekeys-*.png` 对应一个状态：SSH 空态 / 解锁 /
//! 选中 111 / Account Expect-Send / 新建模态框 / Sessions。

use crate::ui::dialogs::settings_live_onekeys::Tab;
use crate::ui::TerminalWindow;

/// 把环境变量指定的状态摆到 `win` 上。返回是否命中任一夹具。
pub(in crate::ui) fn apply(win: &mut TerminalWindow) {
    // 组件基线取金标准 `onekeys-sessions-page.png`：没显式指定时落 Sessions 页。
    let tab = std::env::var("WINDTERM_ONEKEYS_TAB")
        .unwrap_or_else(|_| "sessions".to_string());
    let want = match tab.as_str() {
        "account" => Some(Tab::Account),
        "sessions" => Some(Tab::Sessions),
        "ssh" => Some(Tab::Ssh),
        _ => None,
    };
    if let Some(want) = want {
        let edit = win.onekeys_edit.get_or_insert_with(Default::default);
        edit.tab = want;
        // 参考图 Sessions 图里 Filter 是聚焦态（蓝描边）、保险库已解锁
        // （页脚钥匙文案 + Ok 可点）。只在夹具里摆这两件事。
        if want == Tab::Sessions {
            edit.session_filter_focus = true;
            if win.vault.is_none() {
                if let Ok(vault) = term_vault::Vault::create("fixture-password-123") {
                    win.vault = Some(vault);
                }
            }
        }
    }

    // 新建方案模态框（参考图 onekeys-new-dialog.png）。
    if std::env::var("WINDTERM_ONEKEYS_MODAL").as_deref() == Ok("1") {
        let edit = win.onekeys_edit.get_or_insert_with(Default::default);
        edit.new_scheme_modal = true;
        edit.new_scheme_buf = "onekey_1".to_string();
    }

    // 选中 111 + 保险库解锁（参考图 onekeys-account-selected-111.png）。
    if std::env::var("WINDTERM_ONEKEYS_STATE").as_deref() == Ok("selected-111") {
        win.onekeys = vec![term_core::onekey::OneKey {
            name: "111".to_string(),
            identity_files: ["~/.ssh/id_linux".to_string(), String::new(), String::new()],
            ..Default::default()
        }];
        let edit = win.onekeys_edit.get_or_insert_with(Default::default);
        edit.draft = Some(win.onekeys.clone());
        edit.selected = Some(0);
        edit.tab = Tab::Ssh;
        // 夹具里密码写死、只用于摆出「已解锁」外观；weak-password 时保持锁定，
        // 不能在截图进程里 panic。
        if let Ok(vault) = term_vault::Vault::create("fixture-password-123") {
            win.vault = Some(vault);
        }
    }
}
