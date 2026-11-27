//! `Tool ▸ Script` 五条命令的 Effect 落地。
//!
//! 单独成文件是为了让 `effects.rs` 守住 500 代码行门禁——脚本菜单一次加了
//! 五条命令，连同「停止 / 录制」两处需要说明的纪律，留在主文件会把行数顶过线。
//! 这里只做派发到已经存在的窗口方法，不新写任何一条执行路径：运行窗口、编辑器、
//! 管理器的入口与面板按钮 / 只读模式共用的是同一批方法。

use super::TerminalWindow;
use term_app::effect::Effect;

impl TerminalWindow {
    pub(super) fn apply_script_effect(&mut self, effect: Effect) -> bool {
        match effect {
            // 菜单只给入口；面板显隐仍归 `Window ▸ Panes ▸ Script`，这里不碰 ViewState。
            Effect::RunScript => self.open_script_dialog_for_active(),
            Effect::NewScript => self.open_new_script(self.active),
            Effect::ManageScripts => self.open_script_manager(),
            // Stop 与 Local Mode 共用同一处遍历；录制勾与面板按钮共用同一个槽位。
            Effect::StopScript => self.cancel_all_script_runs(),
            Effect::RecordScript => match self.toggle_recording() {
                Ok(Some(path)) => {
                    self.log_note = Some(format!("Script recorded: {}", path.display()));
                }
                Ok(None) => {}
                Err(msg) => self.log_note = Some(msg),
            },
            // 调用方只在这五条上转进来；其余原样退回，不在这里造 panic 或空成功。
            _ => return false,
        }
        true
    }
}
