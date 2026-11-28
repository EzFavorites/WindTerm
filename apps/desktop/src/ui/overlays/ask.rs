//! 脚本提问的模态框（`ask` / `pick_file` 的界面落点）。
//!
//! # 它解决的是「谁在等」
//!
//! 脚本跑在自己的线程上，问一句话之后**阻塞**在回信通道上（`script_human`）。
//! 界面这一侧每帧从队列里取一个请求、画成一个模态框，用户点完把答案送回去，
//! 脚本线程才继续。所以这个文件只有两件事：**取一个、画一个、回一个**。
//!
//! # 为什么一次只画一个
//!
//! 队列里可能排着三四个问题（脚本可以连问几次）。同时画出来会让「回车」落到
//! 哪一个是随机的；一个一个来则是确定的顺序——脚本本来就是顺序执行的，界面
//! 跟着它的顺序走最不意外。
//!
//! # 文件框为什么是手打路径
//!
//! 普通配置页的浏览按钮已改用系统对话框（`native_picker.rs`），但脚本的
//! `pick_file` 仍落成「输入路径」的输入框：用户亲手打的**那一个**路径就是他
//! 同意脚本碰的那一个（ADR-0027 的「手选即授权」）。系统对话框的过滤器与默认
//! 目录由平台改写，授权边界反而模糊；这里保持手打，比「静默返回空串」安全。
//!
//! # 取消
//!
//! 关窗 / 按 Esc / 按 Cancel 都不生成普通答案：`Ask` 走 `AskError::NoInput`
//! （脚本据此走「没人回答」分支，而不是当成「用户点了确定」），文件框回 `None`。
//! 取消与真实回答在画框这一层就分开（[`AskOutcome`]）：Confirm 的「No」是真的
//! `Bool(false)`，Esc 则连 `Answer` 都不生成。

use term_app::i18n::Lang;

// 回信口是不透明句柄（apps 层定义）；界面既看不到通道类型，也不持有通道端。
use crate::script_human::{AskRequest, PendingPick, PickRequest};
use crate::ui::*;
use term_script::{Answer, Ask, AskError, PickFile};

/// 一帧画框的结局：**取消**与**真实回答**从窗口层就分开。
///
/// 不能把取消塞进 `Answer::Bool(false)`：Confirm 里它和「用户点了 No」同形，
/// Text/Secret/Choose 里它还是类型错误的 bool。
#[derive(Debug)]
pub(in crate::ui) enum AskOutcome {
    /// 用户给了真实答案（按钮点击 / 回车提交）。
    Answer(Answer),
    /// 关窗 / Esc / Cancel：没有答案，脚本应走 `NoInput`。
    Cancel,
}

/// 一个待答复的请求（回信口都是不透明句柄）。
pub(in crate::ui) enum Pending {
    Ask(AskRequest),
    Pick(PickRequest),
}

/// 模态框的跨帧状态（输入缓冲与选中项必须存活，否则输入法逐帧重置）。
pub(in crate::ui) struct AskModal {
    pending: Pending,
    /// 文本类问题的输入缓冲 / 文件框的路径缓冲。
    text: String,
    /// 选择类问题当前高亮的项（0 起）。
    selected: usize,
}

impl AskModal {
    pub(in crate::ui) fn new(pending: Pending) -> Self {
        let selected = 0;
        let text = match &pending {
            Pending::Ask(ask) => match &ask.request {
                Ask::Text { default, .. } => default.clone(),
                _ => String::new(),
            },
            Pending::Pick(pick) => match &pick.request {
                PickFile::Open { default, .. } | PickFile::Save { default, .. } => {
                    default.clone()
                }
            },
        };
        Self {
            pending,
            text,
            selected,
        }
    }

    /// 已经立着的模态框所属运行被取消时，也要撤掉它（不能等用户去点）。
    pub(in crate::ui) fn is_stale(&self) -> bool {
        match &self.pending {
            Pending::Ask(ask) => ask.is_stale(),
            Pending::Pick(pick) => pick.is_stale(),
        }
    }

    /// 标题与正文（两种请求共用一套文案位置）。
    fn title_and_body(&self, lang: Lang) -> (String, String) {
        match &self.pending {
            Pending::Ask(ask) => match &ask.request {
                Ask::Alert { title, text } => (auto(title), auto(text)),
                Ask::Message { title, text, .. } => (auto(title), auto(text)),
                Ask::Confirm { title, text } => (auto(title), auto(text)),
                Ask::Text { title, prompt, .. } => (auto(title), auto(prompt)),
                Ask::Secret { title, prompt } => (auto(title), auto(prompt)),
                Ask::Choose {
                    title,
                    prompt,
                    options,
                } => (
                    auto(title),
                    format!("{}\n{}", auto(prompt), options.join(" / ")),
                ),
            },
            Pending::Pick(pick) => match &pick.request {
                PickFile::Open { title, filter, .. } => (
                    auto(title),
                    pick_help(filter, lang),
                ),
                PickFile::Save { title, filter, .. } => (
                    auto(title),
                    pick_help(filter, lang),
                ),
            },
        }
    }

    /// 这个问题要不要文本框。
    fn wants_text(&self) -> bool {
        match &self.pending {
            Pending::Ask(ask) => matches!(ask.request, Ask::Text { .. } | Ask::Secret { .. }),
            Pending::Pick(_) => true,
        }
    }

    /// 这个问题要不要选择列表。
    fn options(&self) -> Option<&[String]> {
        match &self.pending {
            Pending::Ask(ask) => match &ask.request {
                Ask::Choose { options, .. } => Some(options),
                _ => None,
            },
            Pending::Pick(_) => None,
        }
    }

    /// 消息类问题的按钮（返回 `(标签, 点它的结局)`）。
    fn buttons(&self, lang: Lang) -> Vec<(String, AskOutcome)> {
        let answer = |value| AskOutcome::Answer(value);
        match &self.pending {
            Pending::Ask(ask) => match &ask.request {
                Ask::Alert { .. } => vec![(t(lang, "OK").to_string(), answer(Answer::Ack))],
                Ask::Message { buttons, .. } => buttons
                    .labels()
                    .iter()
                    // 消息框返回的是「按了哪个按钮」的原文标签：`Ask::Message`
                    // 的契约就是 `Answer::Text(label)`。早期这里把 no/cancel
                    // 都压成 Bool(false)，三按钮组里根本分不出 IDNO 与 IDCANCEL，
                    // 也让 abort/retry/ignore 无从映射。显示文字仍走 `t()`，
                    // 结构化答案取未翻译的标签，点哪个按钮都不会因语言错位。
                    .map(|label| {
                        (
                            t(lang, label).to_string(),
                            answer(Answer::Text((*label).to_string())),
                        )
                    })
                    .collect(),
                Ask::Confirm { .. } => vec![
                    // 「No」是真实回答，必须与 Esc 的取消截然不同。
                    (t(lang, "Yes").to_string(), answer(Answer::Bool(true))),
                    (t(lang, "No").to_string(), answer(Answer::Bool(false))),
                ],
                Ask::Text { .. } | Ask::Secret { .. } => {
                    vec![(t(lang, "OK").to_string(), answer(Answer::Ack))]
                }
                Ask::Choose { .. } => {
                    vec![(t(lang, "OK").to_string(), answer(Answer::Ack))]
                }
            },
            Pending::Pick(_) => vec![
                (t(lang, "Open").to_string(), answer(Answer::Ack)),
                // 文件框的 Cancel 没有选中任何路径，走取消分支（送 None）。
                (t(lang, "Cancel").to_string(), AskOutcome::Cancel),
            ],
        }
    }
}

/// 取一个待答复的请求（队列空 = `None`）。
pub(in crate::ui) fn poll(
    asks: &crate::script_human::AskQueue,
    picks: &std::sync::Arc<std::sync::Mutex<Vec<PendingPick>>>,
) -> Option<AskModal> {
    // 问题优先于文件框：脚本是顺序执行的，先排进来的先答。
    // 取之前先看取消令牌：被「停止」的请求连弹窗都不该再弹。
    if let Ok(mut queue) = asks.lock() {
        while !queue.is_empty() {
            let next = queue.remove(0);
            if !next.is_stale() {
                // 从这里起界面只碰到不透明回信句柄：转换发生在 apps 层。
                return Some(AskModal::new(Pending::Ask(next.into())));
            }
        }
    }
    if let Ok(mut queue) = picks.lock() {
        while !queue.is_empty() {
            let next = queue.remove(0);
            if !next.is_stale() {
                return Some(AskModal::new(Pending::Pick(next.into())));
            }
        }
    }
    None
}

/// 画一帧。返回 `Some(outcome)` 时这一帧有结论，调用方负责送回脚本线程。
///
/// - `Some(AskOutcome::Answer(..))`：用户点了按钮 / 回车提交，是真的回答。
/// - `Some(AskOutcome::Cancel)`：Esc / Cancel，没有回答（脚本走 `NoInput`）。
/// - `None`：模态框继续立着。
pub(in crate::ui) fn draw(
    ctx: &egui::Context,
    modal: &mut AskModal,
    lang: Lang,
) -> Option<AskOutcome> {
    let (title, body) = modal.title_and_body(lang);
    let mut clicked: Option<AskOutcome> = None;
    let mut close = false;
    let mut focus_in_modal = false;
    let window_id = egui::Id::new("script_ask_modal");
    let mut window = egui::Window::new(if title.is_empty() {
        t(lang, "Script").to_string()
    } else {
        title
    })
    .id(window_id)
    .collapsible(false)
    .resizable(false)
    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO);
    window = window.default_width(360.0);
    let shown = window.show(ctx, |ui| {
        if !body.is_empty() {
            ui.label(body.clone());
        }
        if let Some(options) = modal.options() {
            let options = options.to_vec();
            for (index, option) in options.iter().enumerate() {
                if ui
                    .selectable_label(modal.selected == index, option)
                    .clicked()
                {
                    modal.selected = index;
                }
            }
        }
        if modal.wants_text() {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut modal.text)
                    .id(egui::Id::new("script_ask_text"))
                    .desired_width(f32::INFINITY),
            );
            focus_in_modal |= edit.has_focus();
            // 文件框回车 = 确定；文本问题也允许回车确认（省一次点击）。
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                close = true;
            }
        }
        ui.horizontal(|ui| {
            for (label, outcome) in modal.buttons(lang) {
                let button = ui.button(label);
                focus_in_modal |= button.has_focus();
                if button.clicked() {
                    clicked = Some(outcome);
                }
            }
        });
    });
    // 弹窗刚立起来时里面往往没有控件持有焦点。主动请求焦点给窗口本体，
    // 让「Esc 在这个模态框里按的」有确定的判据，也避免把别处的 Esc 吞掉。
    if let Some(shown) = &shown {
        if !focus_in_modal {
            shown.response.request_focus();
        }
        focus_in_modal |= shown.response.has_focus();
    }
    // 同帧按钮点击优先：Esc 绝不覆盖用户真的点下去的答案。
    if let Some(outcome) = clicked {
        return Some(outcome);
    }
    if close {
        // 回车：按「当前形态」组装答案。
        return Some(AskOutcome::Answer(match &modal.pending {
            Pending::Ask(ask) => match &ask.request {
                Ask::Choose { .. } => Answer::Choice(modal.selected),
                Ask::Confirm { .. } => Answer::Bool(true),
                _ => Answer::Text(modal.text.clone()),
            },
            Pending::Pick(_) => Answer::Ack,
        }));
    }
    // Esc 只属于拥有焦点的这个模态框，且只在没有按钮点击 / 回车提交的帧生效。
    if focus_in_modal && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        return Some(AskOutcome::Cancel);
    }
    None
}

/// 画框结局 + 请求形态 → 真正送回脚本线程的值。
///
/// 抽出来是因为「按钮语义」与「线格式」不是一回事：`Confirm` 的按钮给的是
/// `Bool`，而 `Pick` 的 OK 给的是 `Ack` + 文本框里的路径。这一层负责翻译，
/// 画的那一层只管收集点击。取消在这里变成 `Resolved::Cancel`，绝不降级成
/// 某个普通的 `Answer`。
pub(in crate::ui) fn resolve(modal: &AskModal, outcome: AskOutcome) -> Resolved {
    let answer = match outcome {
        AskOutcome::Cancel => return Resolved::Cancel,
        AskOutcome::Answer(answer) => answer,
    };
    match &modal.pending {
        Pending::Pick(_) => {
            let path = modal.text.trim();
            if path.is_empty() {
                return Resolved::Picked(None);
            }
            Resolved::Picked(Some(std::path::PathBuf::from(path)))
        }
        Pending::Ask(ask) => match (&ask.request, answer) {
            (Ask::Choose { .. }, Answer::Ack) => Resolved::Answer(Answer::Choice(modal.selected)),
            (Ask::Text { .. }, Answer::Ack) => Resolved::Answer(Answer::Text(modal.text.clone())),
            (Ask::Secret { .. }, Answer::Ack) => Resolved::Answer(Answer::Text(modal.text.clone())),
            (Ask::Confirm { .. }, Answer::Ack) => Resolved::Answer(Answer::Bool(true)),
            (_, other) => Resolved::Answer(other),
        },
    }
}

/// 翻译后的答复。
pub(in crate::ui) enum Resolved {
    Answer(Answer),
    Picked(Option<std::path::PathBuf>),
    /// 取消：Ask 回 `Err(NoInput)`，Pick 回 `None`。
    Cancel,
}

impl AskModal {
    /// 把答复送回等着的脚本线程。**取走自己**（一个请求只答一次）。
    ///
    /// 送不出去只有一种原因：脚本线程已经收摊（被取消 / 窗口要关了）。那不是
    /// 错误——用户答完时脚本可能已经走了，答案本来也没人接了。
    pub(in crate::ui) fn deliver(self, resolved: Resolved) {
        match (self.pending, resolved) {
            (Pending::Ask(ask), Resolved::Answer(answer)) => {
                let _ = ask.reply.reply(Ok(answer));
            }
            // 取消走错误通道：脚本看到 NoInput，不会把「没回答」当成答案。
            (Pending::Ask(ask), Resolved::Cancel) => {
                let _ = ask.reply.reply(Err(AskError::NoInput));
            }
            (Pending::Ask(ask), Resolved::Picked(_)) => {
                let _ = ask.reply.reply(Ok(Answer::Ack));
            }
            (Pending::Pick(pick), Resolved::Picked(path)) => {
                let _ = pick.reply.reply(path);
            }
            // 文件框的 Cancel / Esc：回 None（没有选中任何路径），与 CRT 一致。
            (Pending::Pick(pick), Resolved::Answer(_)) | (Pending::Pick(pick), Resolved::Cancel) => {
                let _ = pick.reply.reply(None);
            }
        }
    }
}

/// 脚本给的文案**原样显示**，不翻译。
///
/// `t()` 只吃 `&'static str`（界面自己的那几条固定文案），而脚本的提示语是
/// 用户数据——把它塞进翻译表既做不到（生命周期）也不该做（谁写的就照谁的话显示）。
/// 翻译只发生在界面自带的按钮与说明上（`buttons()` / `pick_help`）。
fn auto(text: &str) -> String {
    text.to_string()
}

/// 文件框的说明行：把「过滤」原样带上，让用户知道该写什么后缀。
fn pick_help(filter: &str, lang: Lang) -> String {
    if filter.trim().is_empty() {
        t(lang, "Type a path:").to_string()
    } else {
        format!("{} ({filter})", t(lang, "Type a path:"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // 测试在 ui/ 里，但仍只经不透明句柄回信：构造句柄的口子开在 apps 层。
    use crate::script_human::AskReply;
    use term_script::CancelToken;

    fn modal(request: Ask) -> AskModal {
        let (tx, _rx) = std::sync::mpsc::sync_channel(1);
        AskModal::new(Pending::Ask(AskRequest {
            cancel: CancelToken::new(),
            request,
            reply: AskReply::new(tx),
        }))
    }

    fn confirm() -> AskModal {
        modal(Ask::Confirm {
            title: "确认".into(),
            text: "继续？".into(),
        })
    }

    fn text_modal() -> AskModal {
        modal(Ask::Text {
            title: String::new(),
            prompt: "名字？".into(),
            default: String::new(),
        })
    }

    fn secret_modal() -> AskModal {
        modal(Ask::Secret {
            title: String::new(),
            prompt: "密码？".into(),
        })
    }

    fn raw() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        }
    }

    fn esc_event() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    /// 画两帧：首帧立窗并让 `draw` 把焦点请求给窗口本体，第二帧送 Esc。
    fn esc_frame(m: &mut AskModal) -> Option<AskOutcome> {
        let ctx = egui::Context::default();
        let mut result = None;
        let _ = ctx.run(raw(), |ctx| {
            result = draw(ctx, m, Lang::En);
        });
        assert!(result.is_none(), "首帧只是立窗，不该有结论");

        let mut input = raw();
        input.events.push(esc_event());
        let _ = ctx.run(input, |ctx| {
            result = draw(ctx, m, Lang::En);
        });
        result
    }

    /// 从一帧输出里找出按钮的矩形（按 x 排序）。按钮 frame 是个小 Rect，
    /// 窗口底色是 360 宽的大 Rect，文本输入框是长条 Rect——按尺寸过滤即可。
    fn button_rects(out: &egui::FullOutput) -> Vec<egui::Rect> {
        let mut rects = Vec::new();
        for clipped in &out.shapes {
            if let egui::Shape::Rect(r) = &clipped.shape {
                let (w, h) = (r.rect.width(), r.rect.height());
                // 按钮高 18 左右；窗口标题栏高 33，按高度排除掉。
                if (18.0..90.0).contains(&w) && (14.0..26.0).contains(&h) {
                    rects.push(r.rect);
                }
            }
        }
        rects.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
        rects
    }

    /// 量出第二颗按钮（Confirm 的 No）的位置；先跑一帧让布局稳定。
    fn no_button_center(ctx: &egui::Context, m: &mut AskModal) -> egui::Pos2 {
        let _ = ctx.run(raw(), |ctx| {
            let _ = draw(ctx, m, Lang::En);
        });
        let out = ctx.run(raw(), |ctx| {
            let _ = draw(ctx, m, Lang::En);
        });
        let rects = button_rects(&out);
        assert!(
            rects.len() >= 2,
            "Confirm 应画出 Yes / No 两个按钮：{rects:?}"
        );
        rects[1].center()
    }

    fn click_events(at: egui::Pos2) -> Vec<egui::Event> {
        let mut events = vec![egui::Event::PointerMoved(at)];
        for pressed in [true, false] {
            events.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
        }
        events
    }

    fn deliver_via(reply: AskReply) -> AskModal {
        AskModal::new(Pending::Ask(AskRequest {
            cancel: CancelToken::new(),
            request: Ask::Confirm {
                title: String::new(),
                text: String::new(),
            },
            reply,
        }))
    }

    /// Confirm 上按 Esc：窗口层给 Cancel，送到脚本线程是 `Err(NoInput)`，
    /// 绝不是 `Ok(Bool(false))`——那是「用户点了 No」的形状。
    #[test]
    fn escape_on_confirm_is_no_input_not_bool_false() {
        let mut m = confirm();
        let outcome = esc_frame(&mut m);
        assert!(matches!(outcome, Some(AskOutcome::Cancel)), "{outcome:?}");

        let resolved = resolve(&m, outcome.expect("Esc 有结论"));
        assert!(matches!(resolved, Resolved::Cancel));

        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        deliver_via(AskReply::new(tx)).deliver(resolved);
        assert_eq!(rx.try_recv(), Ok(Err(AskError::NoInput)));
    }

    /// Text / Secret 上的 Esc 同样只有取消，不返回类型错误的 bool。
    #[test]
    fn escape_on_text_and_secret_never_returns_a_bool() {
        for mut m in [text_modal(), secret_modal()] {
            let outcome = esc_frame(&mut m);
            assert!(
                matches!(outcome, Some(AskOutcome::Cancel)),
                "Esc 只能是取消：{outcome:?}"
            );
            let resolved = resolve(&m, AskOutcome::Cancel);
            assert!(matches!(resolved, Resolved::Cancel));

            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            deliver_via(AskReply::new(tx)).deliver(resolved);
            assert_eq!(rx.try_recv(), Ok(Err(AskError::NoInput)));
        }
    }

    /// 点「No」仍是真实回答 `Bool(false)`，走 Ok 通道，不被当成取消。
    #[test]
    fn clicking_no_still_resolves_to_bool_false() {
        let ctx = egui::Context::default();
        let mut m = confirm();
        let at = no_button_center(&ctx, &mut m);

        let mut input = raw();
        input.events.extend(click_events(at));
        let mut result = None;
        let _ = ctx.run(input, |ctx| {
            result = draw(ctx, &mut m, Lang::En);
        });
        let outcome = result.expect("点了 No 就该有结论");
        assert!(
            matches!(&outcome, AskOutcome::Answer(Answer::Bool(false))),
            "No 按钮必须是 Bool(false)：{outcome:?}"
        );

        let resolved = resolve(&m, outcome);
        assert!(matches!(&resolved, Resolved::Answer(Answer::Bool(false))));
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        deliver_via(AskReply::new(tx)).deliver(resolved);
        assert_eq!(rx.try_recv(), Ok(Ok(Answer::Bool(false))));
    }

    /// Esc 不许吞掉同帧按钮答案：同一帧「点 No + Esc」仍以点击为准。
    #[test]
    fn escape_does_not_override_a_button_click_in_the_same_frame() {
        let ctx = egui::Context::default();
        let mut m = confirm();
        let at = no_button_center(&ctx, &mut m);

        let mut input = raw();
        input.events.extend(click_events(at));
        input.events.push(esc_event());
        let mut result = None;
        let _ = ctx.run(input, |ctx| {
            result = draw(ctx, &mut m, Lang::En);
        });
        assert!(
            matches!(result, Some(AskOutcome::Answer(Answer::Bool(false)))),
            "同帧的 Esc 不得覆盖点 No：{result:?}"
        );
    }
}
