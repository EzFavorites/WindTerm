//! 「脚本」停靠面板：从脚本库挑一个 Rhai 脚本，在**指定会话**上跑，结果日志就
//! 留在面板里。
//!
//! 与 `terminal/script.rs` 的 `ScriptDialog`（一次性「运行脚本」窗口）共用同一套执行
//! 线程（`apps/desktop/src/script_run.rs` 的 `ScriptRun`）——只是常驻在底带当面板，
//! 不另开窗口。开关（显不显示）走 `ViewState::show_pane_script`，由齿轮菜单的
//! `TogglePanel(Script)` 翻转。
//!
//! 为什么结果不写进终端：那是**伪造输出**。脚本 `send` 出去的字节应当出现在同一个
//! 屏幕上，但返回值是我们算出来的，不该冒充远端说的——所以日志只画在面板里。
//!
//! # 2026-11-17 重做：形态复用 Sender
//!
//! 上一版是 `egui::ComboBox` + 三个 `ui.button` + 一句 `session: {id:?}` 弱文本，
//! 和旁边的 Sender 面板长得完全不像（一个手绘 18 点输入框，一个 egui 默认控件）。
//! 现在工具行走同一套 `Row` 原语：`▶ / ■ / ×` ｜ 竖分隔线 ｜ `Script:` 下拉 ｜
//! `Session:` 下拉。**去掉** Sender 的内容控制（Text/Hex、By、Count、Interval），
//! 会话下拉是**单选**（圆点），不是 Sender 那种多选方块勾。
//!
//! 正文也从「一行状态文字」改成结果区：状态行单独上色，返回值 / 失败原因按
//! 等宽多行铺开（脚本的返回值就是给人看的东西，不该挤成一句话）。

use crate::script_run::{self, Outcome, ScriptRun};
use crate::ui::*;
use eframe::egui;
use term_core::session_runtime::SessionId;
use term_core::theme::CHROME_SCALE;

use super::list_popup::list_popup;
use super::script_panel_row::{script_row, ScriptCmd, SCRIPT_ROW_H};
use super::script_panel_tabs::{draw_tabs, TAB_STRIP_H};

/// 工具行与正文之间的空隙（与 Sender 的 `BODY_GAP` 同值）。
const BODY_GAP: f32 = 3.0;
/// 失败 / 本地错误的文字色。与 Sender 的提示同一支红。
const ERROR_RED: u32 = 0xE06060;

/// 面板上勾出来的额外授权（ADR-0027 的**第三个来源**）。
///
/// 三个来源取并集：脚本头 `// @cap`、命令行 `--allow`、**这里手勾**。手勾这一档
/// 的意义是「脚本我没改，但这次我确实想让它碰文件」——写进脚本头就等于把授权
/// 永久留在文件里，下次别人跑也带着，那是最容易被忘掉的一条后门。
///
/// 只放行**本次运行**：字段跟面板状态同寿命，不落盘、不进 prefs。
#[derive(Default)]
pub(in crate::ui) struct Grants {
    pub process: bool,
    pub clipboard: bool,
    pub ui: bool,
    /// 文件根目录（空 = 不放行文件）。手打而不是选目录：面板窄，选目录要另开
    /// 一个模态框，而这里填的多半是 `/tmp` 这种一眼能打完的路径。
    pub dir: String,
    /// 给了目录之后，是只读还是可写。**独立一档**：能读不代表能写（纪律 2）。
    pub write: bool,
}

impl Grants {
    /// 拼成 `--allow` 那一套语法（`fs=read:/a`、`fs=write:/a`、`process,ui`）。
    ///
    /// 拼出来再交给 `parse_allow` 解析，而不是直接构造 `Capabilities`：解析器
    /// 是**唯一**的事实源，两条路各拼一份迟早对不上（比如这里忘了规范化路径）。
    fn spec(&self) -> String {
        let mut items: Vec<String> = Vec::new();
        if self.process {
            items.push("process".to_string());
        }
        if self.clipboard {
            items.push("clipboard".to_string());
        }
        if self.ui {
            items.push("ui".to_string());
        }
        let dir = self.dir.trim();
        if !dir.is_empty() {
            let mode = if self.write { "write" } else { "read" };
            items.push(format!("fs={mode}:{dir}"));
        }
        items.join(",")
    }
}

/// 脚本面板体持有的全部跨帧状态。
///
/// 放 `TerminalWindow` 上而不是 `ViewState`：里面有一个线程句柄（`ScriptRun`），
/// reducer 不该持有这种东西；`ViewState` 只装「显不显示」那个 bool。
#[derive(Default)]
pub(in crate::ui) struct ScriptPanelState {
    /// 选中的脚本路径（跨帧输入缓冲）。
    pub path: String,
    /// 在跑的那一次（`None` = 没在跑）。
    pub run: Option<ScriptRun>,
    /// 上一次的结果。留着给人看，直到下一次运行 / 清掉。
    pub outcome: Option<Outcome>,
    /// 本地错误：读文件失败 / 起不了线程。**不是**脚本自己的错。
    pub error: Option<String>,
    /// 用户显式选的执行会话（`None` = 跟随主区活动标签）。
    ///
    /// 存 `SessionId` 而不是标签序号：关掉一个标签，后面所有序号整体前移，
    /// 存序号会让脚本跑到别人头上。会话没了就自然失效（见 `effective_session`）。
    pub target: Option<SessionId>,
    /// 本帧实际执行的会话（每帧由 `target` 与活动标签算出来，不是输入）。
    pub session: Option<SessionId>,
    /// 面板上勾出来的额外授权（与 `path` 一样是跨帧输入缓冲）。
    pub grants: Grants,
    /// 这次运行的只读输出标签（脚本日志与返回值走这里，不写真实会话）。
    pub output: Option<crate::script_output::ScriptOutputQueue>,
    /// 上一次输出标签的身份：下次运行时先关掉它，避免同名只读 tab 越堆越多。
    pub output_session: Option<SessionId>,
}

/// 脚本库里所有 `*.rhai`：`(完整路径, 展示名)`。
///
/// 判据在 `terminal::script_library`（运行窗口的两份清单也用它）——两边各写一份
/// `.rhai` 过滤，迟早出现「面板里看不见、窗口里看得见」而根因藏在两处判定里。
fn list_scripts() -> Vec<(String, String)> {
    crate::ui::terminal::script_library::library()
}

/// 本帧可选会话下拉里的名字。
///
/// 重名标签很常见（连着开三个 `local`），下拉里两行一模一样等于选不了。所以
/// 第二个同名的起加 ` (2)`、` (3)`——只在**这一格**里生效，不改标签自己的名字。
pub(in crate::ui) fn session_labels(names: &[String]) -> Vec<String> {
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    names
        .iter()
        .map(|n| {
            let c = seen.entry(n.as_str()).or_insert(0);
            *c += 1;
            if *c == 1 {
                n.clone()
            } else {
                format!("{n} ({c})")
            }
        })
        .collect()
}

/// 这次真正对哪个会话跑。
///
/// 显式选的优先，但那一条**必须仍然活着**——会话关掉之后 `SessionId` 不会变成
/// `None`，照旧选它只会得到「写不进去」。活不了就退回活动标签的会话。
pub(in crate::ui) fn effective_session(
    target: Option<SessionId>,
    bound: Option<SessionId>,
    alive: &[SessionId],
) -> Option<SessionId> {
    match target {
        Some(id) if alive.contains(&id) => Some(id),
        _ => bound,
    }
}

/// 状态行要显示什么。四种状态各自不同——「在跑」不能长成「跑完了」。
fn status(state: &ScriptPanelState) -> String {
    if let Some(err) = &state.error {
        return err.clone();
    }
    if state.run.is_some() {
        return "Running…".to_string();
    }
    match &state.outcome {
        Some(Outcome::Done(value)) if value.is_empty() => "Done (no value)".to_string(),
        Some(Outcome::Done(value)) => format!("Done: {value}"),
        Some(Outcome::Failed { message, .. }) => format!("Failed: {message}"),
        None => "Ready".to_string(),
    }
}

/// 状态行是报错还是正常。`Running…` 与 `Done` 都不算错。
fn status_is_error(state: &ScriptPanelState) -> bool {
    state.error.is_some()
        || matches!(
            state.outcome,
            Some(Outcome::Failed { .. })
        )
}

/// 结果区的正文（状态行下面那几行）。没有结果时给一句引导。
fn result_text(state: &ScriptPanelState) -> String {
    match &state.outcome {
        Some(Outcome::Done(value)) if !value.is_empty() => value.clone(),
        Some(Outcome::Done(_)) => String::new(),
        Some(Outcome::Failed { message, at }) => match at {
            Some((line, col)) => format!("{message}\n  at line {line}, column {col}"),
            None => message.clone(),
        },
        None if state.run.is_some() => String::new(),
        None => String::new(),
    }
}

fn scripts_menu_id() -> egui::Id {
    egui::Id::new("script_panel_scripts")
}

fn sessions_menu_id() -> egui::Id {
    egui::Id::new("script_panel_sessions")
}

impl TerminalWindow {
    /// 每帧收一次结果。在 `draw()` 的帧循环里调——无论面板可见与否都要收，
    /// 否则面板被关掉再打开会卡在「在跑」。
    pub(in crate::ui) fn poll_script_panel(&mut self) {
        for tab in &mut self.script_panel.tabs {
            let Some(run) = tab.state.run.as_mut() else {
                continue;
            };
            for note in run.drain_notes() {
                if let Some(out) = &tab.state.output {
                    out.push(&note);
                }
            }
            if let Some(outcome) = run.try_outcome() {
                let line = match &outcome {
                    crate::script_run::Outcome::Done(v) => format!("DONE: {v}"),
                    crate::script_run::Outcome::Failed { message, .. } => {
                        format!("FAILED: {message}")
                    }
                };
                if let Some(out) = &tab.state.output {
                    out.finish(&line);
                }
                tab.state.outcome = Some(outcome);
                tab.state.run = None;
            }
        }
        // 「关闭运行中的页签」是延迟到这里的：先 cancel，收到 Outcome 才真摘掉。
        self.script_panel.drop_finished_closings();
    }

    /// 读文件 → 起线程。失败留在 `error` 里，不弹第二个窗口。
    fn run_script_panel(&mut self) {
        let lang = self.view.lang;
        // 关闭中的页签只是还没收到旧脚本的 Outcome：在上面再起一条，旧结果一到
        // 页签就被摘掉，新句柄跟着丢，线程还在跑却没人管租约。
        let idx = self.script_panel.active.min(self.script_panel.tabs.len() - 1);
        if self.script_panel.tabs[idx].closing {
            self.script_panel.current_mut().error =
                Some(t(lang, "This tab is stopping; wait for it to finish").to_string());
            return;
        }
        let Some(id) = self.script_panel.current_mut().session else {
            self.script_panel.current_mut().error =
                Some(t(lang, "This tab has no live session").to_string());
            return;
        };
        if self
            .tabs
            .iter()
            .any(|t| t.session() == Some(id) && t.local_mode)
        {
            self.script_panel.current_mut().error = Some(
                t(lang, "This session is read-only (Local mode); scripts will not run")
                    .to_string(),
            );
            return;
        }
        let path = self.script_panel.current_mut().path.trim().to_string();
        if path.is_empty() {
            self.script_panel.current_mut().error =
                Some(t(lang, "Pick a script above first").to_string());
            return;
        }
        // 上一次的输出 tab 先收掉：每次运行都新建一条同名只读标签，跑十次
        // 就在标签栏里堆十份，而旧内容已经没有二次查看的价值。
        let previous = self.script_panel.current().output_session;
        if let Some(id) = previous {
            if let Some(index) = self.tabs.iter().position(|tab| tab.identity == id) {
                self.close_tab(index);
            }
        }
        self.script_panel.current_mut().output = None;
        self.script_panel.current_mut().output_session = None;
        let source = match script_run::read_source(&path) {
            Ok(text) => text,
            Err(message) => {
                self.script_panel.current_mut().error = Some(message);
                self.script_panel.current_mut().outcome = None;
                return;
            }
        };
        let allow = self.script_panel.current_mut().grants.spec();
        let options = self.script_run_options(&path).with_allow(allow);
        match ScriptRun::start(self.pool.clone(), id, source, options) {
            Ok(run) => {
                let tab_no = self.script_panel.next_no;
                self.script_panel.current_mut().run = Some(run);
                self.script_panel.current_mut().outcome = None;
                self.script_panel.current_mut().error = None;
                let queue = crate::script_output::ScriptOutputQueue::default();
                queue.push(&format!("RUN {path} on session {id:?}"));
                let out = queue.channel();
                let mut cfg = term_core::session::SessionConfig::new(term_core::session::Protocol::Shell);
                cfg.name = format!("Script Output — {}", path.rsplit('/').next().unwrap_or(&path));
                let out_id = self.pool.open_with_channel(&cfg, 80, 24, Box::new(out));
                let mut tab = crate::ui::terminal::tab_from(
                    &self.pool,
                    cfg,
                    Some(out_id),
                    egui::Id::new(("script_output", id, tab_no)),
                    &self.ctx,
                );
                tab.local_mode = true; // 只读：键盘 / 触发器都进不来
                self.add_tab(tab);
                self.script_panel.current_mut().output = Some(queue);
                self.script_panel.current_mut().output_session = Some(out_id);
                self.ctx.request_repaint();
                self.note_recent_script(&path);
            }
            Err(message) => self.script_panel.current_mut().error = Some(message),
        }
    }

    /// 工具行与两个下拉的落地。**唯一**改脚本面板状态的地方。
    fn apply_script_cmd(
        &mut self,
        ui: &egui::Ui,
        cmd: ScriptCmd,
        scripts: &[(String, String)],
        choices: &[(SessionId, String)],
    ) {
        match cmd {
            ScriptCmd::Run => self.run_script_panel(),
            ScriptCmd::Stop => {
                // 只叫停，不摘句柄：句柄要留到 `poll_script_panel` 收到 Outcome
                // 再置空，否则脚本线程还在跑、租约却已放行，下一条马上插进来。
                // 与 `terminal/script.rs` 的 `ScriptAction::Stop` 同一套语义。
                if let Some(run) = self.script_panel.current().run.as_ref() {
                    run.cancel();
                }
            }
            ScriptCmd::NewTab => self.script_panel.add(),
            ScriptCmd::CloseTab => self.script_panel.close_active(),
            ScriptCmd::OpenConfig => self.open_script_manager(),
            ScriptCmd::OpenScripts => ui.memory_mut(|m| m.toggle_popup(scripts_menu_id())),
            ScriptCmd::OpenSessions => ui.memory_mut(|m| m.toggle_popup(sessions_menu_id())),
            ScriptCmd::PickScript(i) => {
                if let Some((full, _)) = scripts.get(i) {
                    self.script_panel.current_mut().path = full.clone();
                }
                ui.memory_mut(|m| m.close_popup());
            }
            ScriptCmd::PickSession(i) => {
                // 下标 0 是 `Current Session`（= 跟随活动标签），其余对应 choices。
                self.script_panel.current_mut().target = if i == 0 {
                    None
                } else {
                    choices.get(i - 1).map(|(id, _)| *id)
                };
                ui.memory_mut(|m| m.close_popup());
            }
        }
        ui.ctx().request_repaint();
    }
}

/// 面板体。标题由停靠容器补（`PanelId::Script::title()` = "Script"）。
///
/// 与 `tools::draw_explorer` / `shell_panel::draw_shell` 同一格式：自由函数
/// `(win, ui)`，由 `panels/mod.rs::draw_panel_body` 按 `id` 分发调用。
pub(in crate::ui) fn draw_script_panel(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    // 有活会话的标签 = 会话下拉的候选。关掉的标签不在册，也就发不出字节。
    let choices: Vec<(SessionId, String)> = win
        .tabs
        .iter()
        .filter_map(|t| t.session().map(|id| (id, t.config.name.clone())))
        .collect();
    let alive: Vec<SessionId> = choices.iter().map(|(id, _)| *id).collect();
    let bound = win.tabs.get(win.active_slot()).and_then(|t| t.session());
    let effective = effective_session(win.script_panel.current_mut().target, bound, &alive);
    win.script_panel.current_mut().session = effective;

    let view_h = ui.clip_rect().height();
    let full = ui.available_rect_before_wrap();
    let row_rect = egui::Rect::from_min_size(
        full.min,
        egui::vec2(full.width(), SCRIPT_ROW_H),
    );

    let scripts = list_scripts();
    let names = session_labels(&choices.iter().map(|(_, n)| n.clone()).collect::<Vec<_>>());
    let script_label = match scripts.iter().find(|(full, _)| full == &win.script_panel.current_mut().path) {
        Some((_, name)) => name.as_str(),
        None => win.tr("— pick a script —"),
    };
    // 会话格显示：显式选中 → 那个标签的名字；没选 → `Current Session`；
    // 选中的会话已经关了 → 明说「已关闭 → 当前」，不再只写「已关闭」让人以为
    // 点 Run 什么都不会发生（实际它会按 `effective_session` 退回活动标签）。
    let session_label = match win.script_panel.current().target {
        None => win.tr("Current Session").to_string(),
        Some(id) => match choices.iter().position(|(x, _)| *x == id) {
            Some(i) => names[i].clone(),
            None if win.script_panel.current().session.is_some() => format!(
                "{} → {}",
                win.tr("Closed Session"),
                win.tr("Current Session")
            ),
            None => win.tr("Closed Session").to_string(),
        },
    };

    let out = script_row(win, ui, row_rect, script_label, &session_label);
    if let Some(cmd) = out.cmd {
        win.apply_script_cmd(ui, cmd, &scripts, &choices);
    }

    // 脚本下拉：第一项是「还没选」的占位，所以下标要往右挪一格。
    let script_items: Vec<String> = std::iter::once(win.tr("— pick a script —").to_string())
        .chain(scripts.iter().map(|(_, n)| n.clone()))
        .collect();
    let script_sel = scripts
        .iter()
        .position(|(full, _)| full == &win.script_panel.current_mut().path)
        .map(|i| i + 1);
    if let Some(i) = list_popup(
        ui,
        scripts_menu_id(),
        out.script_anchor,
        &script_items,
        script_sel,
        win.tr("no scripts in library"),
    ) {
        // 选回占位项 = 清空选择。
        if i == 0 {
            win.script_panel.current_mut().path.clear();
            ui.memory_mut(|m| m.close_popup());
        } else {
            win.apply_script_cmd(ui, ScriptCmd::PickScript(i - 1), &scripts, &choices);
        }
    }

    let session_items = std::iter::once(win.tr("Current Session").to_string())
        .chain(names.iter().cloned())
        .collect::<Vec<_>>();
    let session_sel = win
        .script_panel
        .current()
        .target
        .and_then(|id| choices.iter().position(|(x, _)| *x == id))
        .map(|i| i + 1)
        .or(Some(0));
    if let Some(i) = list_popup(
        ui,
        sessions_menu_id(),
        out.session_anchor,
        &session_items,
        session_sel,
        win.tr("No open sessions"),
    ) {
        win.apply_script_cmd(ui, ScriptCmd::PickSession(i), &scripts, &choices);
    }

    // ── 结果区（多页时给底部数字页签条留位置）──
    let strip_h = if win.script_panel.tabs.len() > 1 { TAB_STRIP_H } else { 0.0 };
    let body = egui::Rect::from_min_max(
        egui::pos2(full.left(), row_rect.bottom() + BODY_GAP),
        egui::pos2(
            full.right(),
            ((full.top() + view_h).max(row_rect.bottom() + BODY_GAP + 40.0)) - strip_h,
        ),
    );
    // 录制器 + 单次授权条：控件与文案在 `script_panel_strip` 里，这里只留高度。
    let toolbar_h = super::script_panel_strip::draw(win, ui, body);
    let body = egui::Rect::from_min_max(
        egui::pos2(body.left(), body.top() + toolbar_h),
        body.max,
    );
    let child = ui.new_child(egui::UiBuilder::new().max_rect(body));
    let p = child.painter();
    let font = crate::ui::type_scale::body();
    let line_h = child.text_style_height(&egui::TextStyle::Monospace).max(1.0);
    let mut y = body.top();
    // 状态行：报错才上红，`Ready` / `Running…` / `Done` 都用正文色。
    let sc = if status_is_error(win.script_panel.current()) {
        crate::theme::chrome(ERROR_RED)
    } else {
        crate::theme::chrome(CHROME_SCALE.text)
    };
    let sg = p.layout(
        status(win.script_panel.current()),
        font.clone(),
        sc,
        body.width(),
    );
    p.galley(egui::pos2(body.left(), y), sg.clone(), sc);
    y += sg.size().y.max(line_h);
    // 后台脚本不占会话名额，也就不会把「运行」按钮灰掉——那更要说一声有人在跑。
    let bg = win.script_runs.background_count();
    if bg > 0 {
        let c = crate::theme::chrome(CHROME_SCALE.hint);
        let g = p.layout_no_wrap(
            format!("Background scripts running ({bg})"),
            crate::ui::type_scale::small(),
            c,
        );
        p.galley(egui::pos2(body.left(), y), g.clone(), c);
        y += g.size().y.max(line_h);
    }
    // 结果正文：返回值 / 失败原因，等宽多行（脚本自己排好的格式要保住）。
    let text = result_text(win.script_panel.current());
    if !text.is_empty() {
        let c = crate::theme::chrome(CHROME_SCALE.text);
        let g = p.layout(text, font, c, body.width());
        p.galley(egui::pos2(body.left(), y), g, c);
    }
    // 没有活会话、也没在跑：给一句为什么按不动 ▶，而不是一个死键。
    if win.script_panel.current_mut().session.is_none() && win.script_panel.current_mut().run.is_none() {
        let hint = win.tr("This tab has no live session");
        let c = crate::theme::chrome(CHROME_SCALE.hint);
        let g = p.layout_no_wrap(hint.to_string(), crate::ui::type_scale::small(), c);
        p.galley(
            egui::pos2(body.left(), body.bottom() - g.size().y - 2.0),
            g,
            c,
        );
    }
    if strip_h > 0.0 {
        draw_tabs(
            win,
            ui,
            egui::Rect::from_min_size(
                egui::pos2(full.left(), body.bottom()),
                egui::vec2(full.width(), TAB_STRIP_H),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_permission_checkboxes_turn_into_an_allow_spec() {
        // 面板手勾的每一项都要能在 `--allow` 语法里找到对应写法，否则「勾了没用」。
        let mut g = Grants::default();
        assert_eq!(g.spec(), "", "什么都没勾就什么都不放行");

        g.process = true;
        g.ui = true;
        assert_eq!(g.spec(), "process,ui");

        g.dir = "  /tmp  ".to_string();
        assert_eq!(g.spec(), "process,ui,fs=read:/tmp", "只读是缺省档");

        g.write = true;
        assert_eq!(g.spec(), "process,ui,fs=write:/tmp");

        // 拼出来的字符串必须真的能被解析器吃下去（两条路各拼一份是这类 bug 的温床）。
        let parsed = term_core::script_caps::parse_allow(&g.spec()).expect("应当能解析");
        assert_eq!(parsed.len(), 3);
    }

    #[test]
    fn the_status_line_tells_apart_idle_running_done_and_failed() {
        let mut s = ScriptPanelState::default();
        assert_eq!(status(&s), "Ready");

        // 「跑完了但没有返回值」不能说成「跑完了：」——那个冒号会让人以为值丢了。
        s.outcome = Some(Outcome::Done(String::new()));
        assert_eq!(status(&s), "Done (no value)");

        s.outcome = Some(Outcome::Done("ok".to_string()));
        assert!(status(&s).contains("ok"));
        assert!(!status_is_error(&s));

        s.outcome = Some(Outcome::Failed {
            message: "第 3 行：超时".to_string(),
            at: Some((3, 1)),
        });
        assert!(status(&s).starts_with("Failed"), "{}", status(&s));
        assert!(status(&s).contains("第 3 行"), "{}", status(&s));
        assert!(status_is_error(&s), "失败状态要上红");
    }

    /// 一帧真绘制：布局算错、借用写错、裁剪区为负——都会在这里炸出来。
    ///
    /// 手绘工具行 + 两个下拉 + 结果区都不是 egui 的现成控件，没有这条冒烟测试，
    /// 这些错误只会在真机上表现为「面板空白」。
    #[test]
    fn the_panel_really_draws_a_frame() {
        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let mut win = crate::ui::lifecycle_test_support::window_with(pool, &ctx, Vec::new());
        win.script_panel.current_mut().outcome = Some(Outcome::Failed {
            message: "第 3 行：超时".to_string(),
            at: Some((3, 1)),
        });
        // 两帧：第一帧定尺寸，第二帧才是「知道宽高之后」的真实布局。
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw_script_panel(&mut win, ui);
                });
            });
        }
    }

    /// 重名标签在下拉里必须能分清：「local」连着开三个，选哪个得看得见。
    #[test]
    fn duplicate_session_names_get_a_suffix_so_they_can_be_told_apart() {
        let names: Vec<String> = ["local", "local", "db", "local"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            session_labels(&names),
            vec!["local", "local (2)", "db", "local (3)"]
        );
        // 不重名时一个字都不改。
        let one: Vec<String> = vec!["a".into(), "b".into()];
        assert_eq!(session_labels(&one), vec!["a", "b"]);
    }
}

#[cfg(test)]
#[path = "script_panel_stop_tests.rs"]
mod stop_tests;
