//! 脚本编辑器窗口（`docs/design/07-scripting-and-automation.md` 的第三步）。
//!
//! 三件事，各占一段：
//!
//! 1. **状态**（[`ScriptEditorState`]）：文档缓冲区（`term_script::ScriptDoc`）、
//!    当前路径、脏标记、运行句柄、上一次结果。打开 / 保存 / 运行都不在绘制闭包里
//!    做——闭包只收集 [`EditorAction`]，对外施加在窗外交给 `TerminalWindow`。
//! 2. **着色**（[`highlight_job`]）：把 `term_script::lex` 的 token 映射成
//!    `LayoutJob` 的分段颜色。词法在 crate 层（与引擎注册的原语同一份清单），
//!    这里只管上什么色。
//! 3. **绘制**（[`TerminalWindow::draw_script_editor`]）：固定矩形 + 固定 id，
//!    与其余对话框同一套「首帧即最终布局」。
//!
//! # 行号 gutter 为什么对得齐
//!
//! gutter 不自己算行高，而是读 `TextEdit` 返回的 `galley.rows`：每个 `Row` 的
//! `rect.top()` 就是它在屏幕上的纵坐标（`galley_pos` 已经含了滚动偏移）。所以
//! 行号与正文共享同一份排版几何，滚动 / 换字号都不会错位。
//!
//! # 「点错误跳到第 N 行」怎么落地
//!
//! 让 `ScriptError` 带上了结构化 `line()/column()`，`script_run::Outcome`
//! 又把它一路带到界面。点击错误行时用 [`term_script::ScriptDoc::byte_offset`]
//! 把 Rhai 的 1 起 `(行, 列)` 换成字节偏移，再写进 `TextEditState` 的光标——
//! 全程不碰 `Display` 字符串，也不在这里重新解析行号。

use crate::script_run::{self, Outcome, ScriptRun};
use crate::ui::kit::window::{DialogId, DialogWindow};
use crate::ui::*;
use term_app::i18n::Lang;
use term_core::session_runtime::SessionId;
use term_script::{ScriptDoc, TokenKind};

/// `TextEdit` 的稳定 id（光标状态按 id 存取）。
const EDITOR_ID: &str = "script_editor_text";

/// 编辑器的全部状态。
///
/// 放 `TerminalWindow` 上而不是 `ViewState`：里面有一个线程句柄（`ScriptRun`）
/// 与一份可编辑缓冲，reducer 不该持有这种东西。
pub(in crate::ui) struct ScriptEditorState {
    /// 窗口开着没有（`None` 的 Option 已表达「没有状态」，这个 bool 表达「关着但
    /// 状态还在」——目前关窗即丢弃状态，保留 bool 与其余对话框一致）。
    pub open: bool,
    /// 缓冲区（文本 + 行索引 + 脏标记）。
    pub doc: ScriptDoc,
    /// 当前文件路径；空串 = 还没保存过。
    pub path: String,
    /// 本地错误（读 / 写 / 起线程）。**不是**脚本自己的错。
    pub error: Option<String>,
    /// 在跑的那一次。
    pub run: Option<ScriptRun>,
    /// 上一次运行结果。
    pub outcome: Option<Outcome>,
    /// 脚本对着哪个会话跑。
    pub session: Option<SessionId>,
    /// 待跳转的字节偏移（错误行按钮设置，绘制正文那一帧消费）。
    jump: Option<usize>,
    /// 「另存为」输入缓冲。`None` = 没在另存为。
    save_as: Option<String>,
}

impl ScriptEditorState {
    fn closed() -> Self {
        ScriptEditorState {
            open: false,
            doc: ScriptDoc::default(),
            path: String::new(),
            error: None,
            run: None,
            outcome: None,
            session: None,
            jump: None,
            save_as: None,
        }
    }

    /// 每帧收一次结果与提示。返回脚本 `notify` 送来的最后一句（交给状态栏）。
    fn poll(&mut self) -> Option<String> {
        let run = self.run.as_mut()?;
        let notes = run.drain_notes();
        if let Some(outcome) = run.try_outcome() {
            self.outcome = Some(outcome);
            self.run = None;
        }
        notes.last().cloned()
    }

    /// 窗口标题：有路径就用文件名，没有就用通用名。
    fn title(&self, lang: Lang) -> String {
        if self.path.trim().is_empty() {
            t(lang, "Script Editor").to_string()
        } else {
            file_label(&self.path)
        }
    }
}

/// 路径最后一段（`a/b/c.rhai` → `c.rhai`）。空串给原文。
fn file_label(path: &str) -> String {
    path.trim()
        .rsplit(['/', '\\'])
        .next()
        .filter(|leaf| !leaf.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// 绘制期间收集的动作。闭包外统一施加（与 `overlays/preview.rs` 同一套）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditorAction {
    None,
    Close,
    Run,
    /// 存到当前路径（没有路径则退化为「另存为」）。
    Save,
    /// 打开「另存为」输入行。
    SaveAs,
    /// 把「另存为」输入行的路径落盘。
    SaveAsWrite,
}

impl TerminalWindow {
    /// 打开编辑器并绑定到 `session`；`path` 给了就先读进来。
    ///
    /// 读文件是 IO，走 `script_run::read_source`（`ui/` 只调，不自己开文件）。
    pub(in crate::ui) fn open_script_editor(&mut self, session: Option<SessionId>, path: Option<&str>) {
        let path = path.unwrap_or("").trim().to_string();
        let (text, error) = if path.is_empty() {
            (String::new(), None)
        } else {
            match script_run::read_source(&path) {
                Ok(text) => (text, None),
                Err(message) => (String::new(), Some(message)),
            }
        };
        let mut state = ScriptEditorState::closed();
        state.open = true;
        state.session = session;
        state.path = path;
        state.doc = ScriptDoc::new(text);
        state.error = error;
        self.script_editor = Some(state);
        self.ctx.request_repaint();
    }

    /// 新建一个空白脚本：路径空着，但「另存为」预填到**脚本库**目录。
    ///
    /// 预填的理由：`path` 空着时按保存只会先弹一行空白输入框，用户随手打一个
    /// `test.rhai` 就落进了当前工作目录——那不是他的脚本库，下次启动就找不到了。
    /// 先把落点摆出来，用户想换随时能改。
    pub(in crate::ui) fn open_new_script(&mut self, session: Option<SessionId>) {
        self.open_script_editor(session, None);
        if let Some(state) = self.script_editor.as_mut() {
            state.save_as = script_run::default_script_path();
        }
    }

    /// 画编辑器。默认关闭时一行都不画。
    pub(in crate::ui) fn draw_script_editor(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.script_editor.take() else {
            return;
        };
        if !state.open {
            self.script_editor = Some(state);
            return;
        }
        // `notify` 的话落到窗口的状态栏（`log_note` 是既有的一次性反馈通道）。
        // 函数返回 `Option` 就是为了这一句：`ScriptEditorState` 自己够不到窗口。
        if let Some(note) = state.poll() {
            self.log_note = Some(note);
        }
        let lang = self.view.lang;
        let title = state.title(lang);
        let mut action = EditorAction::None;
        // 状态只读、动作收集：闭包持有 `state` 的可变借用，施加动作要等它结束。
        let still_open =
            DialogWindow::new(DialogId::ScriptEditor, &title).show(ctx, |ui| {
                draw_editor(ui, lang, &mut state, &mut action);
            });
        if !still_open {
            action = EditorAction::Close;
        }
        self.script_editor = Some(state);
        match action {
            EditorAction::None => {}
            EditorAction::Close => {
                if let Some(state) = self.script_editor.as_mut() {
                    state.open = false;
                }
            }
            EditorAction::Run => self.run_editor_script(),
            EditorAction::Save => self.save_editor(false),
            EditorAction::SaveAs => {
                if let Some(state) = self.script_editor.as_mut() {
                    state.save_as = Some(state.path.clone());
                }
            }
            EditorAction::SaveAsWrite => self.save_editor(true),
        }
    }

    /// 在当前会话上跑缓冲区。起线程复用 `script_run::ScriptRun`。
    fn run_editor_script(&mut self) {
        let pool = self.pool.clone();
        // ADR-0025：编辑器跑出来的脚本同样能 `session_open`，开出来的会话
        // 同样要出现在标签栏——所以三条运行入口（运行窗口 / 停靠面板 / 编辑器）
        // 都交同一个队列。先克隆出来，下面 `self.script_editor.as_mut()` 借走整块。
        // 先取「谁在跑」的名字（编辑器还没存过盘时是空串，运行侧会兜成「未命名脚本」）。
        let who = self
            .script_editor
            .as_ref()
            .map(|state| state.path.clone())
            .unwrap_or_default();
        let options = self.script_run_options(&who);
        let lang = self.view.lang;
        let Some(state) = self.script_editor.as_mut() else {
            return;
        };
        let Some(id) = state.session else {
            state.error = Some(t(lang, "This tab has no live session").to_string());
            return;
        };
        match ScriptRun::start(pool, id, state.doc.text().to_string(), options) {
            Ok(run) => {
                state.run = Some(run);
                state.outcome = None;
                state.error = None;
                self.ctx.request_repaint();
            }
            Err(message) => state.error = Some(message),
        }
    }

    /// 落盘。`as_new` 为真时取「另存为」输入的路径。
    ///
    /// 写文件是 OS 资源，走 `script_run::save_source`（`ui/` 不碰文件系统）。
    fn save_editor(&mut self, as_new: bool) {
        let Some(state) = self.script_editor.as_mut() else {
            return;
        };
        let path = if as_new {
            state.save_as.take().unwrap_or_default()
        } else {
            state.path.clone()
        };
        if path.trim().is_empty() {
            // 没路径就打开「另存为」输入行，别在错误栏里丢一句「保存失败」。
            if state.save_as.is_none() {
                // 预填脚本库路径，让「新建 → 直接保存」落进
                // `profile_dir()/scripts/`，而不是随启动目录漂移的 CWD。
                state.save_as = Some(script_run::default_script_path().unwrap_or_default());
            }
            return;
        }
        match script_run::save_source(&path, state.doc.text()) {
            Ok(()) => {
                state.path = path;
                state.doc.mark_saved();
                state.error = None;
                state.save_as = None;
            }
            Err(error) => state.error = Some(error.to_string()),
        }
    }
}

/// 一帧的全部绘制。可见文案一律经 `t(lang, …)`（i18n 门禁）。
fn draw_editor(
    ui: &mut egui::Ui,
    lang: Lang,
    state: &mut ScriptEditorState,
    action: &mut EditorAction,
) {
    let dim = ui.visuals().weak_text_color();
    let bad = egui::Color32::from_rgb(220, 96, 96);
    let good = egui::Color32::from_rgb(120, 190, 120);

    ui.horizontal(|ui| {
        let running = state.run.is_some();
        ui.add_enabled_ui(!running, |ui| {
            if ui.button(t(lang, "Run")).clicked() {
                *action = EditorAction::Run;
            }
        });
        if ui.button(t(lang, "Save")).clicked() {
            *action = EditorAction::Save;
        }
        if ui.button(t(lang, "Save As")).clicked() {
            *action = EditorAction::SaveAs;
        }
        if running {
            ui.spinner();
        }
        if state.doc.is_dirty() {
            ui.label(egui::RichText::new(t(lang, "● Unsaved")).small().color(dim));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(t(lang, "Close")).clicked() {
                *action = EditorAction::Close;
            }
            let label = if state.path.trim().is_empty() {
                t(lang, "(unsaved)").to_string()
            } else {
                state.path.clone()
            };
            ui.label(egui::RichText::new(label).small().color(dim));
        });
    });

    if state.save_as.is_some() {
        // 取消要在借用 `save_as` 结束后再清，否则与输入框的可变借用打架。
        let mut cancel = false;
        if let Some(buffer) = state.save_as.as_mut() {
            ui.horizontal(|ui| {
                ui.label(t(lang, "Save as:"));
                ui.add(
                    egui::TextEdit::singleline(buffer)
                        .id(egui::Id::new("script_editor_save_as"))
                        .hint_text("/path/to/script.rhai")
                        .desired_width(420.0),
                );
                if ui.button(t(lang, "Save")).clicked() {
                    *action = EditorAction::SaveAsWrite;
                }
                if ui.button(t(lang, "Cancel")).clicked() {
                    cancel = true;
                }
            });
        }
        if cancel {
            state.save_as = None;
        }
    }

    if let Some(error) = &state.error {
        ui.label(egui::RichText::new(error).small().color(bad));
    }

    // 上一次结果。失败且带结构化位置时，给一个可点的「跳到第 N 行」。
    let failed_at = match &state.outcome {
        Some(Outcome::Failed { at, .. }) => *at,
        _ => None,
    };
    let mut want_jump = false;
    match &state.outcome {
        Some(Outcome::Failed { message, .. }) => {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{}: {message}", t(lang, "Failed")))
                        .small()
                        .color(bad),
                );
                if let Some((line, _)) = failed_at {
                    let label = t(lang, "Go to line {n}").replace("{n}", &line.to_string());
                    if ui.small_button(label).clicked() {
                        // 只记意图：`state.outcome` 还在被借用，借用结束后再动 `state`。
                        want_jump = true;
                    }
                }
            });
        }
        Some(Outcome::Done(value)) => {
            let text = if value.is_empty() {
                t(lang, "Done").to_string()
            } else {
                format!("{}: {value}", t(lang, "Done"))
            };
            ui.label(egui::RichText::new(text).small().color(good));
        }
        None => {}
    }
    if want_jump {
        if let Some((line, column)) = failed_at {
            state.jump = state.doc.byte_offset(line, column);
        }
    }

    draw_body(ui, state);
}

/// 正文：行号 gutter + 着色 TextEdit，共享同一个滚动区。
fn draw_body(ui: &mut egui::Ui, state: &mut ScriptEditorState) {
    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    let colors = TokenColors::from_ui(ui);
    // 等宽字体一个字符的近似宽度（只用于 gutter 宽度与横向预留，不参与排版）。
    let char_w = font_id.size * 0.6;
    let row_h = font_id.size * 1.4;
    let line_count = state.doc.line_count().max(1);
    let digits = line_count.to_string().len().max(2) as f32;
    let gutter_w = digits * char_w + 10.0;
    let longest = state
        .doc
        .text()
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0)
        .max(1);
    let mut text = state.doc.text().to_string();

    egui::ScrollArea::both()
        .id_salt("script_editor_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                let (gutter_rect, _) = ui.allocate_exact_size(
                    egui::vec2(gutter_w, line_count as f32 * row_h),
                    egui::Sense::hover(),
                );
                let desired_w =
                    ((longest as f32) * char_w + 24.0).max(ui.available_width() - gutter_w);
                let layout_font = font_id.clone();
                let mut layouter = move |ui: &egui::Ui, source: &str, _wrap: f32| {
                    let job = highlight_job(source, &layout_font, &colors);
                    ui.fonts(|fonts| fonts.layout_job(job))
                };
                let output = egui::TextEdit::multiline(&mut text)
                    .id(egui::Id::new(EDITOR_ID))
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(line_count)
                    .desired_width(desired_w)
                    .layouter(&mut layouter)
                    .frame(false)
                    .show(ui);

                let galley = output.galley.clone();
                let galley_pos = output.galley_pos;
                let changed = output.response.changed();
                // 行号：每个 Row 的 rect.top() 就是屏上纵坐标（含滚动偏移）。
                let painter = ui.painter();
                for (index, row) in galley.rows.iter().enumerate() {
                    painter.text(
                        egui::pos2(gutter_rect.right() - 4.0, galley_pos.y + row.rect.top()),
                        egui::Align2::RIGHT_TOP,
                        (index + 1).to_string(),
                        font_id.clone(),
                        colors.comment,
                    );
                }

                // 跳转：把字节偏移换成字符光标，写进 TextEdit 的状态。
                if let Some(byte) = state.jump.take() {
                    let character = text[..byte.min(text.len())].chars().count();
                    let mut edit_state = output.state;
                    edit_state.cursor.set_char_range(Some(egui::text::CCursorRange::one(
                        egui::text::CCursor::new(character),
                    )));
                    edit_state.store(ui.ctx(), egui::Id::new(EDITOR_ID));
                    output.response.request_focus();
                    ui.ctx().request_repaint();
                }

                if changed {
                    state.doc.set_text(text);
                }
            });
        });
}

/// 高亮配色。深浅主题各一套，取 `ui.visuals()` 判断。
#[derive(Clone, Copy)]
struct TokenColors {
    text: egui::Color32,
    keyword: egui::Color32,
    primitive: egui::Color32,
    string: egui::Color32,
    number: egui::Color32,
    comment: egui::Color32,
}

impl TokenColors {
    fn from_ui(ui: &egui::Ui) -> Self {
        let dark = ui.visuals().dark_mode;
        let (keyword, primitive, string, number) = if dark {
            (
                egui::Color32::from_rgb(96, 160, 220),
                egui::Color32::from_rgb(225, 180, 95),
                egui::Color32::from_rgb(115, 195, 125),
                egui::Color32::from_rgb(200, 145, 225),
            )
        } else {
            (
                egui::Color32::from_rgb(30, 90, 170),
                egui::Color32::from_rgb(150, 95, 10),
                egui::Color32::from_rgb(35, 120, 55),
                egui::Color32::from_rgb(120, 60, 160),
            )
        };
        TokenColors {
            text: ui.visuals().text_color(),
            keyword,
            primitive,
            string,
            number,
            comment: ui.visuals().weak_text_color(),
        }
    }

    fn of(self, kind: TokenKind) -> egui::Color32 {
        match kind {
            TokenKind::Keyword => self.keyword,
            TokenKind::Primitive => self.primitive,
            TokenKind::String => self.string,
            TokenKind::Number => self.number,
            TokenKind::Comment => self.comment,
        }
    }
}

fn text_format(font: &egui::FontId, color: egui::Color32) -> egui::TextFormat {
    egui::TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    }
}

/// 把源码切成「默认色 + 若干上色段」的排版任务。
///
/// **不折行**（`max_width = INFINITY`）：折行会让一个逻辑行占多行，行号 gutter
/// 就得做 row→line 映射；不折行时 `galley.rows.len() == line_count`，行号即序号。
/// 长行的横向滚动交给外层 `ScrollArea::both`。
fn highlight_job(
    source: &str,
    font: &egui::FontId,
    colors: &TokenColors,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut cursor = 0;
    for token in term_script::lex(source) {
        if token.start > cursor {
            job.append(
                &source[cursor..token.start],
                0.0,
                text_format(font, colors.text),
            );
        }
        job.append(
            &source[token.start..token.end],
            0.0,
            text_format(font, colors.of(token.kind)),
        );
        cursor = token.end;
    }
    if cursor < source.len() {
        job.append(&source[cursor..], 0.0, text_format(font, colors.text));
    }
    job.wrap.max_width = f32::INFINITY;
    job
}

#[cfg(test)]
#[path = "script_editor_tests.rs"]
mod tests;
