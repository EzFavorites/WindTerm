//! 命令行历史的分组键：当前活动标签的系统名。
//!
//! 抽出来是因为同一帧里补全候选源与回车落盘都要取它，各写一份很容易
//! 在某次重构后只剩一处更新，于是「补全只看 A 系统、记录写进 B 系统」。

use crate::ui::*;

pub(super) fn system_of(win: &TerminalWindow) -> String {
    win.tabs
        .get(win.active_slot())
        .map(|tab| tab.config.system.clone())
        .unwrap_or_default()
}
