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
//! egui 没有原生文件对话框，而仓库不引第三方对话框依赖（那会把平台相关的
//! 几百 KB 带进来）。所以 `pick_file` 落成一个「输入路径」的输入框：用户打的
//! 那个路径**就是**他同意脚本碰的那一个（ADR-0027 的「手选即授权」）。
//! 这比「假装有文件框但打不开」诚实，也比「静默返回空串」安全。
//!
//! # 取消
//!
//! 关窗 / 按 Esc / 按 Cancel 都把 `AskError::NoInput` 送回脚本线程——它据此走
//! 「没人回答」那条分支，而**不是**继续当成「用户点了确定」。

use term_app::i18n::Lang;

use crate::script_human::{PendingAsk, PendingPick};
use crate::ui::*;
use term_script::{Answer, Ask, PickFile};

/// 一个待答复的请求。
pub(in crate::ui) enum Pending {
    Ask(PendingAsk),
    Pick(PendingPick),
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

    /// 消息类问题的按钮（返回 `(标签, 是哪个 Answer)`）。
    fn buttons(&self, lang: Lang) -> Vec<(String, Answer)> {
        match &self.pending {
            Pending::Ask(ask) => match &ask.request {
                Ask::Alert { .. } => vec![(t(lang, "OK").to_string(), Answer::Ack)],
                Ask::Message { buttons, .. } => buttons
                    .labels()
                    .iter()
                    .map(|label| {
                        // `MessageButton` 给的标签是英文原文（OK / Cancel / Yes / No），
                        // 按它反推回结构化的答案——与 `contains` 同一套判定，
                        // 所以「点的哪个按钮」不会因为翻译而错位。
                        let answer = if label.eq_ignore_ascii_case("yes") {
                            Answer::Bool(true)
                        } else if label.eq_ignore_ascii_case("no")
                            || label.eq_ignore_ascii_case("cancel")
                        {
                            // 「否」与「取消」在这里是同一个答案（都是「不是」）：
                            // 脚本侧对 `Message` 只关心「是/不是」，取消的语义
                            // 与「否」一致——它不会因为用户按了哪个而改变分支。
                            Answer::Bool(false)
                        } else {
                            Answer::Ack
                        };
                        (t(lang, label).to_string(), answer)
                    })
                    .collect(),
                Ask::Confirm { .. } => vec![
                    (t(lang, "Yes").to_string(), Answer::Bool(true)),
                    (t(lang, "No").to_string(), Answer::Bool(false)),
                ],
                Ask::Text { .. } | Ask::Secret { .. } => {
                    vec![(t(lang, "OK").to_string(), Answer::Ack)]
                }
                Ask::Choose { .. } => {
                    vec![(t(lang, "OK").to_string(), Answer::Ack)]
                }
            },
            Pending::Pick(_) => vec![
                (t(lang, "Open").to_string(), Answer::Ack),
                (t(lang, "Cancel").to_string(), Answer::Bool(false)),
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
    if let Ok(mut queue) = asks.lock() {
        if !queue.is_empty() {
            return Some(AskModal::new(Pending::Ask(queue.remove(0))));
        }
    }
    if let Ok(mut queue) = picks.lock() {
        if !queue.is_empty() {
            return Some(AskModal::new(Pending::Pick(queue.remove(0))));
        }
    }
    None
}

/// 画一帧。返回 `Some(answer)` = 用户答复了（调用方负责送回脚本线程）。
///
/// 返回 `None` 时模态框继续立着；用户按 Cancel / Esc 时返回
/// `Some(Answer::Bool(false))` 由调用方翻译成「没有回答」（见 `mod.rs` 的用法）。
pub(in crate::ui) fn draw(
    ctx: &egui::Context,
    modal: &mut AskModal,
    lang: Lang,
) -> Option<Answer> {
    let (title, body) = modal.title_and_body(lang);
    let mut answer = None;
    let mut close = false;
    let mut window = egui::Window::new(if title.is_empty() {
        t(lang, "Script").to_string()
    } else {
        title
    })
    .collapsible(false)
    .resizable(false)
    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO);
    window = window.default_width(360.0);
    window.show(ctx, |ui| {
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
                    .desired_width(f32::INFINITY),
            );
            // 文件框回车 = 确定；文本问题也允许回车确认（省一次点击）。
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                close = true;
            }
        }
        ui.horizontal(|ui| {
            for (label, value) in modal.buttons(lang) {
                if ui.button(label).clicked() {
                    answer = Some(value);
                }
            }
        });
    });
    if close && answer.is_none() {
        // 回车：按「当前形态」组装答案。
        answer = Some(match &modal.pending {
            Pending::Ask(ask) => match &ask.request {
                Ask::Choose { .. } => Answer::Choice(modal.selected),
                Ask::Confirm { .. } => Answer::Bool(false),
                _ => Answer::Text(modal.text.clone()),
            },
            Pending::Pick(_) => Answer::Ack,
        });
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        // Esc = 取消（不是「确定」）。脚本会看到 NoInput。
        answer = Some(Answer::Bool(false));
    }
    answer
}

/// 答案 + 请求形态 → 真正送回脚本线程的值。
///
/// 抽出来是因为「按钮语义」与「线格式」不是一回事：`Confirm` 的按钮给的是
/// `Bool`，而 `Pick` 的 OK 给的是 `Ack` + 文本框里的路径。这一层负责翻译，
/// 画的那一层只管收集点击。
pub(in crate::ui) fn resolve(modal: &AskModal, answer: Answer) -> Resolved {
    match &modal.pending {
        Pending::Pick(_) => {
            if matches!(answer, Answer::Bool(false)) {
                // 取消：没有选中任何路径（CRT 的文件框取消也是空）。
                return Resolved::Picked(None);
            }
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
}

impl AskModal {
    /// 把答复送回等着的脚本线程。**取走自己**（一个请求只答一次）。
    ///
    /// 送不出去只有一种原因：脚本线程已经收摊（被取消 / 窗口要关了）。那不是
    /// 错误——用户答完时脚本可能已经走了，答案本来也没人接了。
    pub(in crate::ui) fn deliver(self, resolved: Resolved) {
        match (self.pending, resolved) {
            (Pending::Ask(ask), Resolved::Answer(answer)) => {
                let _ = ask.reply.send(answer);
            }
            (Pending::Ask(ask), Resolved::Picked(_)) => {
                let _ = ask.reply.send(Answer::Ack);
            }
            (Pending::Pick(pick), Resolved::Picked(path)) => {
                let _ = pick.reply.send(path);
            }
            (Pending::Pick(pick), Resolved::Answer(answer)) => {
                // 文件框收到的是「取消」：回 None（没有选中任何路径）。
                let _ = answer;
                let _ = pick.reply.send(None);
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
