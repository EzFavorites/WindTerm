//! 浮层：命令面板 / Goto / 工作台 / 锁屏 / 首次启动引导 / 预览窗。
//!
//! 这一组没有停靠位——它们是**覆盖在终端之上**的临时层，
//! 关掉即消失，默认全关。绘制顺序在 `ui::mod` 的末尾统一排。

// 脚本提问的模态框（`ask` / `pick_file` 的界面落点）。与其余浮层同级：
// 默认没有待答复的请求时一行都不画。
mod ask;
mod center;
mod goto;
mod lock;
// 模态对话框的整屏灰罩：画在 Background 之上、对话框之下，
// 让弹窗从主窗口里显式浮出来（吞掉穿透点击）。
mod modal;
// D6：首次启动引导。它是一次性的（写完 `prefs.onboarded` 就再不出现），
// 生命周期与「临时覆盖层」一致：不属于固定框架、不参与停靠、默认不画。
mod onboarding;
mod palette;
// D6：预览窗（Filer 双击 / 右键「预览」）。同属临时覆盖层：不参与停靠、
// 默认不画。它与引导的区别是同一个窗口会被反复打开，所以状态（内容 / 纹理）
// 留在 `PreviewState` 里，绘制只读。
mod preview;
// D8-3：脚本编辑器窗口。与预览窗同属临时覆盖层：不参与停靠、默认不画，
// 状态（文档缓冲 / 运行句柄）留在 `ScriptEditorState` 里。
mod script_editor;

// 窗口结构体要持有一份预览状态（内容 + 纹理），所以类型要在 `ui` 这一层可见。
// 只再导出**类型**，`content_of` / `human_bytes` 这些纯函数仍是 `preview` 私有——
// 它们只属于那个模块，`ui/mod.rs` 不该顺手借来用。
pub(in crate::ui) use ask::AskModal;
pub(in crate::ui) use modal::draw_backdrop;

/// 取一个待答复的请求（队列空 = `None`）。
pub(in crate::ui) fn poll_ask(
    asks: &crate::script_human::AskQueue,
    picks: &std::sync::Arc<std::sync::Mutex<Vec<crate::script_human::PendingPick>>>,
) -> Option<AskModal> {
    ask::poll(asks, picks)
}

/// 画一帧模态框；`Some(answer)` = 用户答复了。
pub(in crate::ui) fn draw_ask(
    ctx: &egui::Context,
    modal: &mut AskModal,
    lang: term_app::i18n::Lang,
) -> Option<term_script::Answer> {
    ask::draw(ctx, modal, lang)
}

/// 答案 + 请求形态 → 真正送回脚本线程的值。
pub(in crate::ui) fn resolve_ask(modal: &AskModal, answer: term_script::Answer) -> ask::Resolved {
    ask::resolve(modal, answer)
}
pub(in crate::ui) use preview::PreviewState;
pub(in crate::ui) use script_editor::ScriptEditorState;
