//! 剪贴板能力：复制、粘贴各是一个可派发的动作（F1 起，F1-b 补齐粘贴）。
//!
//! egui 0.29 只暴露**写**剪贴板的接口（`Context::copy_text`），读取要走平台层。
//! 上一阶段据此把 `Text.Paste` 留在占位（菜单置灰）；F1-b 找到了不用新依赖的做法：
//! `ViewportCommand::RequestPaste` 会让 eframe 自己去读系统剪贴板，并在下一帧
//! 投递 `Event::Paste(String)`——和用户按 Cmd+V 是**同一条路径**。
//!
//! 所以本层只声明意图，两端都不碰平台：
//!
//! - `Text.Copy` / `Cmd+C` → `Effect::CopySelection`，UI 从当前事实窗口取选区文本写入剪贴板；
//! - `Text.Paste` / `Cmd+V` → `Effect::Paste`，UI 请框架读剪贴板，拿到文本后
//!   经 `term_core::vt::paste_bytes_for` 编码再写给 PTY；
//! - `Terminal.PasteSelectedText`（R14-b）→ `Effect::PasteSelection`，UI 取当前选区文本
//!   （与 `CopySelection` 同源），同样经 `paste_bytes_for` 编码后写给 PTY——
//!   「粘贴」这件事只差一个来源（剪贴板 vs 选区），编码与写出路径完全一致。
//!
//! 「怎么编码」不在这一层：换行归一、控制字符过滤、括号粘贴都取决于终端模式，
//! 属于语义平面（term-core）。这里连剪贴板内容都不看。

use crate::effect::Effect;
use crate::state::ViewState;

pub use crate::vocab::ClipboardAction;


/// 纯函数：三个动作都不改视图状态，只声明一个需要 UI 执行的副作用。
/// 命令 ID → 本能力的动作。未接线的 ID 返回 `None`。
///
/// 2026-11-12 R2：实现只有一份，在 [`crate::parse`]（入口层）。这里保留同名入口，
/// 一是因为「每个能力自带字符串入口」是这个 crate 对外的约定，二是因为
/// `apps/desktop` 有直接调 `term_app::log::parse` 的地方——把路径改掉没有架构价值。
pub fn parse(id: &str) -> Option<ClipboardAction> {
    crate::parse::clipboard_parse(id)
}

pub fn reduce(_state: &mut ViewState, action: ClipboardAction) -> Vec<Effect> {
    match action {
        ClipboardAction::Copy => vec![Effect::CopySelection],
        ClipboardAction::Paste => vec![Effect::Paste],
        ClipboardAction::PasteSelection => vec![Effect::PasteSelection],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_maps_both_clipboard_commands() {
        assert_eq!(parse("Text.Copy"), Some(ClipboardAction::Copy));
        assert_eq!(parse("Text.Paste"), Some(ClipboardAction::Paste));
        // R14-b：「Paste Selected Text」已接线——选区文本直接写给 PTY。
        assert_eq!(
            parse("Terminal.PasteSelectedText"),
            Some(ClipboardAction::PasteSelection)
        );
        assert_eq!(parse("Terminal.Save"), None);
    }

    #[test]
    fn each_action_emits_its_own_effect_and_leaves_state_untouched() {
        let mut state = ViewState::default();
        let before = state.clone();
        assert_eq!(
            reduce(&mut state, ClipboardAction::Copy),
            vec![Effect::CopySelection]
        );
        assert_eq!(
            reduce(&mut state, ClipboardAction::Paste),
            vec![Effect::Paste]
        );
        assert_eq!(
            reduce(&mut state, ClipboardAction::PasteSelection),
            vec![Effect::PasteSelection]
        );
        assert_eq!(state, before, "剪贴板动作不写视图状态");
    }
}
