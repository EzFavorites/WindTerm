//! `Mode ▸ Debug Mode` 附带的排障日志：开关与唯一的会话日志槽 reconcile。
//!
//! 文档定义的 Debug Mode 是「创建更详细的会话活动日志用于排障」，而本仓原来
//! 只做了单元格属性覆盖层。两者不冲突：覆盖层继续画，这里在打开时额外起一份
//! **原始字节（含转义序列）**日志。会话的日志槽只有一个，所以用户自己已经在
//! Logging 页记日志时不抢——抢了就是把他的日志悄悄换掉，比没有排障日志更糟。

use crate::ui::tab::Tab;

/// 按「Debug 开关 + 当前日志槽是否被占用」决定起 / 停排障日志。
/// 纯流程，不画像素、不碰 `ViewState`；`Tab` 的借由调用方拆开传进来。
pub(in crate::ui) fn sync_debug_log(
    pool: &term_session::SessionPool,
    tab: &mut Tab,
    on: bool,
    busy_log: bool,
) {
    if on && !busy_log {
        if let (Some(id), Some(dir)) = (tab.session, term_io::log::default_dir()) {
            let opts = term_core::debug_log::options();
            tab.debug_log_active = pool.start_log(id, dir, &tab.config.name, &opts);
        }
    } else if !on && tab.debug_log_active {
        if let Some(id) = tab.session {
            pool.stop_log(id);
        }
        tab.debug_log_active = false;
    }
}
