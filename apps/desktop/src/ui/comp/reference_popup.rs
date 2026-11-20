//! 视觉基线夹具的统一入口：7 个变体按「快捷命令 / 右键菜单 + 未挂接弹层」
//! 分派到两个绘制文件。从 `comp/mod.rs` 拆出来是为了控制 500 代码行上限。

use eframe::egui;

use super::{quick_fixtures, CompId};
use crate::ui::TerminalWindow;

pub(in crate::ui) fn draw(win: &mut TerminalWindow, ctx: &egui::Context, comp: CompId) {
    if matches!(comp, CompId::QuickCommands | CompId::QuickConfig) {
        quick_fixtures::draw(win, ctx, comp);
    } else {
        super::context_fixtures::draw(win, ctx, comp);
    }
}
