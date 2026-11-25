//! Modem 自动触发与传输期的键盘取消（page/14 的两条产品缺口）。
//!
//! 为什么单独一个文件：`ui/mod.rs` 与 `ui/terminal/pane.rs` 都已经顶到验收的
//! 500 代码行上限（见 `scripts/verify.sh` 4j），而这两条逻辑既不属于绘制也不
//! 属于触发器求值——它们只消费「本帧输出字节」和「本帧按键事件」，然后调用
//! 既有入口（`Action::Xfer` / `XferHandle::cancel`）。放在这里，`mod.rs` 只留
//! 一行调用。
//!
//! # 分工
//!
//! - 输出检测在 `Tab::poll`（`terminal/session.rs`）喂进来，与本帧触发器读同一
//!   份字节；命中后只往 `TabState` 记一个请求，窗口级 `poll` 再落地。
//! - 键盘取消在窗口级 `poll` 里吃掉事件：它必须早于 `pane.rs` 的 PTY 写入，
//!   否则 `Ctrl+C` 会以 `\x03` 原样漏给对端。
//! - 命令 ID 一个都没新增：这里派发的 `Action::Xfer` 就是
//!   `Terminal.SendZModem` / `Terminal.ReceiveZModem` / `Terminal.CancelModem`
//!   那三条既有命令的强类型形态；取消直接调 `XferHandle::cancel`（与
//!   `Terminal.CancelModem` 同一个落点）。

use eframe::egui;
use std::path::PathBuf;
use term_app::action::{Action, XferCmd, XferProtocol};

use super::dialogs::xfer::XferDirection;
use super::TerminalWindow;

/// 一次 `Ctrl+X` 连按的窗口（秒）。egui 帧时钟在夹具里被钉死，所以这个值
/// 可复现，不读墙钟。
const DOUBLE_X_WINDOW: f64 = 1.0;
/// 跨块重组时保留的历史字符数：命令名最长不过几个字节，256 足够覆盖
/// 「提示符上一帧、命令这一帧」的分块边界，又不会让旧输出无限增长。
const TAIL_CHARS: usize = 256;

// ── 输出检测 ───────────────────────────────────────────────────

/// 一条会话的扫描态。滚动尾跨帧保留（命令可能被拆在两块输出里），请求由
/// 窗口级 `poll` 取走；取走即清，同一条命令不会连开两次弹层。
#[derive(Default)]
pub(in crate::ui) struct TabState {
    tail: String,
    request: Option<XferCmd>,
}

impl TabState {
    /// 喂本帧输出。命中只记第一个请求，并清掉尾部，避免下一帧把同一段
    /// 回显再认一次。
    pub(in crate::ui) fn note(&mut self, text: &str, commands: &[String; 3]) {
        if self.request.is_some() {
            return;
        }
        let visible = visible_text(text);
        if visible.is_empty() {
            return;
        }
        let mut combined = String::with_capacity(self.tail.len() + visible.len());
        combined.push_str(&self.tail);
        combined.push_str(&visible);
        if let Some(cmd) = detect(&combined, commands) {
            self.request = Some(cmd);
            self.tail.clear();
            return;
        }
        let keep: Vec<char> = combined.chars().rev().take(TAIL_CHARS).collect();
        self.tail = keep.into_iter().rev().collect();
    }

    /// 取走本帧请求（消费即清）。
    pub(in crate::ui) fn take(&mut self) -> Option<XferCmd> {
        self.request.take()
    }
}

/// 一条可能出现在远端输出里的命令及其对应方向 / 协议。
struct Candidate {
    name: String,
    cmd: XferCmd,
}

fn add_candidate(out: &mut Vec<Candidate>, raw: &str, cmd: XferCmd) {
    // 配置框里可能带着参数（"rz -be"），只取第一个词；空白 / 空串不认。
    let name = raw.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    if name.is_empty() || out.iter().any(|c| c.name == name) {
        return;
    }
    out.push(Candidate { name, cmd });
}

fn candidates(commands: &[String; 3]) -> Vec<Candidate> {
    let mut out = Vec::new();
    // 出厂命令恒认：即使会话配置写了别名，兼容终端键盘敲出的老命令仍要触发。
    add_candidate(&mut out, "rx", XferCmd::Send(XferProtocol::Xmodem));
    add_candidate(&mut out, "rb", XferCmd::Send(XferProtocol::Ymodem));
    add_candidate(&mut out, "rz", XferCmd::Send(XferProtocol::Zmodem));
    // sz 是对端「发」，本端才是接收方——与三条上传命令方向相反。
    add_candidate(&mut out, "sz", XferCmd::Receive(XferProtocol::Zmodem));
    // 自定义上传命令按下标覆盖协议（0=X / 1=Y / 2=Z，与 SessionConfig 同口径）。
    for (i, name) in commands.iter().enumerate() {
        let protocol = match i {
            0 => XferProtocol::Xmodem,
            1 => XferProtocol::Ymodem,
            _ => XferProtocol::Zmodem,
        };
        add_candidate(&mut out, name, XferCmd::Send(protocol));
    }
    out
}

/// 剥掉 ANSI 控制序列：提示符常带颜色，不剥就认不出 `$ rz` 前面的 `$`。
fn visible_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('[') => {
                // CSI：一直吃到结束字节（字母或 @ ` ~）。
                let _ = chars.next();
                while let Some(n) = chars.next() {
                    if n.is_ascii_alphabetic() || matches!(n, '@' | '`' | '~') {
                        break;
                    }
                }
            }
            Some(']') => {
                // OSC：BEL 或 ST 结束。
                let _ = chars.next();
                while let Some(n) = chars.next() {
                    if n == '\x07' {
                        break;
                    }
                    if n == '\x1b' && chars.peek() == Some(&'\\') {
                        let _ = chars.next();
                        break;
                    }
                }
            }
            Some(_) => {
                let _ = chars.next();
            }
            None => {}
        }
    }
    out
}

fn is_prompt_mark(c: char) -> bool {
    matches!(c, '$' | '#' | '>' | '%' | '❯' | 'λ' | '~')
}

/// 命令名必须独占一个词，且左边是行首 / 提示符，右边是空白或行尾。
/// 只认「提示符后或行首」是任务给的口径，也顺手挡掉 `size` / `rz.bin`
/// 这类正文里的偶然命中。
fn at_command_position(text: &str, at: usize, len: usize) -> bool {
    if let Some(next) = text[at + len..].chars().next() {
        if !(next.is_whitespace() || matches!(next, ';' | '&' | '|')) {
            return false;
        }
    }
    let before = text[..at].trim_end_matches(char::is_whitespace);
    match before.chars().last() {
        None => true,
        Some(c) if c == '\n' || c == '\r' => true,
        Some(c) => is_prompt_mark(c),
    }
}

/// 纯检测：返回本段输出里最早出现的那条命令。入口自己剥 ANSI，调用方即使
/// 直接喂原始 PTY 字节也不会漏掉带颜色的提示符。
fn detect(text: &str, commands: &[String; 3]) -> Option<XferCmd> {
    let lower = visible_text(text).to_ascii_lowercase();
    let mut best: Option<(usize, XferCmd)> = None;
    for candidate in candidates(commands) {
        let name = candidate.name.as_str();
        let mut from = 0usize;
        while let Some(rel) = lower[from..].find(name) {
            let at = from + rel;
            from = at + name.len();
            if !at_command_position(&lower, at, name.len()) {
                continue;
            }
            if best.map_or(true, |(pos, _)| at < pos) {
                best = Some((at, candidate.cmd));
            }
            break;
        }
    }
    best.map(|(_, cmd)| cmd)
}

// ── 键盘取消 ───────────────────────────────────────────────────

/// `Ctrl+X` 连按的判定状态。放在线程局部是因为 `poll` 拿不到窗口字段的
/// 可变借用余量（`mod.rs` 只留一行调用），而 egui 事件泵本来就是单线程。
#[derive(Default)]
pub(in crate::ui) struct CancelState {
    last_x: Option<f64>,
}

thread_local! {
    static CANCEL: std::cell::RefCell<CancelState> =
        std::cell::RefCell::new(CancelState::default());
}

/// 本帧的取消判据（纯函数，便于逐例测）。`Ctrl+C` 立即取消；`Ctrl+X`
/// 要在窗口内连按两次。两次之间隔得太久就把旧的那一击忘掉。
pub(in crate::ui) fn cancel_hit(
    events: &[egui::Event],
    now: f64,
    state: &mut CancelState,
    window: f64,
) -> bool {
    let mut ctrl_c = false;
    let mut ctrl_x = false;
    for event in events {
        match event {
            egui::Event::Text(text) => {
                ctrl_c |= text.contains('\x03');
                ctrl_x |= text.contains('\x18');
            }
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                if modifiers.ctrl {
                    ctrl_c |= *key == egui::Key::C;
                    ctrl_x |= *key == egui::Key::X;
                }
            }
            _ => {}
        }
    }
    if ctrl_c {
        state.last_x = None;
        return true;
    }
    if ctrl_x {
        let paired = state
            .last_x
            .is_some_and(|t| now >= t && now - t <= window);
        state.last_x = if paired { None } else { Some(now) };
        return paired;
    }
    if state.last_x.is_some_and(|t| now - t > window) {
        state.last_x = None;
    }
    false
}

fn is_cancel_event(event: &egui::Event) -> bool {
    match event {
        egui::Event::Text(text) => text.contains('\x03') || text.contains('\x18'),
        egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => modifiers.ctrl && (*key == egui::Key::C || *key == egui::Key::X),
        _ => false,
    }
}

/// 取消键只有在键盘真的会进终端窗格时才归我们处理。
///
/// 窗口失焦、对话框/浮层正拦着键盘时，`Ctrl+C` / `Ctrl+X` 是给那些控件的，
/// 无差别吞掉会让「在对话框里按取消」失效——焦点边界必须先判。
fn terminal_keyboard_focused(ctx: &egui::Context, win: &TerminalWindow) -> bool {
    let view = &win.view;
    ctx.input(|i| i.focused)
        && !view.command_palette.visible
        && !view.goto.visible
        && !view.center.visible
        && !view.show_shortcuts
        && !view.show_tmux
        && !view.lock.locked
        && win.pending_paste.is_none()
        && !win.modal_dialog_open()
}

/// 只摘出**匹配**的取消键，其余事件（包括同一条 Text 里的普通字符）原样留下。
///
/// 旧实现用 `retain` 把整条 `Event::Text("a\x03b")` 丢掉，连带吞了 `ab`；
/// 现在只抠掉控制字符，判据也只看真正摘出来的那些事件。
fn take_cancel_events(events: Vec<egui::Event>) -> (Vec<egui::Event>, Vec<egui::Event>) {
    let mut kept = Vec::with_capacity(events.len());
    let mut taken = Vec::new();
    for event in events {
        match event {
            egui::Event::Text(text) => {
                let (cancel, rest): (String, String) = text
                    .chars()
                    .partition(|c| *c == '\x03' || *c == '\x18');
                if !cancel.is_empty() {
                    taken.push(egui::Event::Text(cancel));
                }
                if !rest.is_empty() {
                    kept.push(egui::Event::Text(rest));
                }
            }
            event if is_cancel_event(&event) => taken.push(event),
            event => kept.push(event),
        }
    }
    (kept, taken)
}

fn cancel_pass(ctx: &egui::Context, win: &mut TerminalWindow) {
    let running = win.xfer.as_ref().is_some_and(|job| {
        let progress = job.handle.progress();
        !progress.finished && progress.error.is_none()
    });
    if !running || !terminal_keyboard_focused(ctx, win) {
        // 传输结束或焦点不在终端：别把半截连按（或失焦前的一击）带进下一次。
        CANCEL.with(|slot| slot.borrow_mut().last_x = None);
        return;
    }
    let (events, now) = ctx.input(|i| (i.events.clone(), i.time));
    let (kept, taken) = take_cancel_events(events);
    let hit = CANCEL.with(|slot| {
        let mut state = slot.borrow_mut();
        cancel_hit(&taken, now, &mut state, DOUBLE_X_WINDOW)
    });
    ctx.input_mut(|i| i.events = kept);
    if hit {
        if let Some(job) = win.xfer.as_ref() {
            job.handle.cancel();
        }
        ctx.request_repaint();
    }
}

/// 把本帧检测到的请求落成弹层 / 直启。
///
/// `modem_use_default` 只在「接收」一侧直启：收到的 `sz` 用 `Download directory`
/// 预填并跳过询问。上传命令（`rz` / `rx` / `rb`）仍要用户挑具体文件——
/// `SessionConfig` 对这一位的定义就是「目录取默认值，文件仍要选」。
fn requests_pass(win: &mut TerminalWindow) {
    let mut found: Vec<(XferCmd, bool, String, String)> = Vec::new();
    for tab in win.tabs.iter_mut().chain(win.shell_tabs.iter_mut()) {
        if let Some(cmd) = tab.modem.take() {
            found.push((
                cmd,
                tab.config.modem_use_default,
                tab.config.modem_download_dir.clone(),
                tab.config.modem_upload_dir.clone(),
            ));
        }
    }
    for (cmd, use_default, download, upload) in found {
        // 复用的就是 `Terminal.ReceiveZModem` / `Terminal.SendZModem` 那条
        // 既有 Xfer 链路（Cancel 走上面的 `XferHandle::cancel`）。
        win.dispatch_action(Action::Xfer(cmd));
        if !use_default {
            continue;
        }
        let start_dir = {
            let Some(prompt) = win.xfer_prompt.as_mut() else {
                continue;
            };
            match prompt.direction {
                XferDirection::Receive => {
                    let dir = download.trim().to_string();
                    if !dir.is_empty() {
                        prompt.path = dir.clone();
                    }
                    dir
                }
                XferDirection::Send => {
                    let dir = upload.trim().to_string();
                    if !dir.is_empty() {
                        prompt.path = dir;
                    }
                    String::new()
                }
            }
        };
        if !start_dir.is_empty() {
            win.start_xfer(PathBuf::from(start_dir));
        }
    }
}

/// 窗口级唯一入口（`ui/mod.rs` 只调用这一行）。
pub(in crate::ui) fn poll(ctx: &egui::Context, win: &mut TerminalWindow) {
    cancel_pass(ctx, win);
    requests_pass(win);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> [String; 3] {
        ["rx".into(), "rb".into(), "rz".into()]
    }

    fn key_ctrl(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        }
    }

    #[test]
    fn upload_commands_open_the_matching_send_prompt() {
        let cfg = defaults();
        assert_eq!(
            detect("last login: Tue\n$ rx\n", &cfg),
            Some(XferCmd::Send(XferProtocol::Xmodem))
        );
        assert_eq!(
            detect("[host ~]$ rb -be\r\n", &cfg),
            Some(XferCmd::Send(XferProtocol::Ymodem))
        );
        // 行首、没有提示符也算。
        assert_eq!(
            detect("rz\r\n", &cfg),
            Some(XferCmd::Send(XferProtocol::Zmodem))
        );
    }

    #[test]
    fn sz_is_the_receive_side_and_zmodem_only() {
        let cfg = defaults();
        assert_eq!(
            detect("user@host:~$ sz report.pdf\r\n", &cfg),
            Some(XferCmd::Receive(XferProtocol::Zmodem))
        );
    }

    #[test]
    fn configured_aliases_are_honoured_but_plain_words_are_not() {
        let mut cfg = defaults();
        cfg[2] = "upload-z".into();
        assert_eq!(
            detect("> upload-z file.bin\n", &cfg),
            Some(XferCmd::Send(XferProtocol::Zmodem))
        );
        // 词中间出现不是命令：`size` 里有 `sz` 的后半、`rz.bin` 也不是独立词。
        assert_eq!(detect("the size of rz.bin is 3\n", &cfg), None);
        assert_eq!(detect("describe rz", &cfg), None);
    }

    #[test]
    fn ansi_prompts_are_stripped_before_matching() {
        let cfg = defaults();
        let text = "\x1b[32muser@host\x1b[0m:\x1b[34m~\x1b[0m$ \x1b[1mrz\x1b[0m\r\n";
        assert_eq!(
            detect(text, &cfg),
            Some(XferCmd::Send(XferProtocol::Zmodem))
        );
    }

    #[test]
    fn a_command_split_across_two_chunks_is_still_found() {
        let cfg = defaults();
        let mut state = TabState::default();
        state.note("prompt$ r", &cfg);
        assert!(state.take().is_none());
        state.note("z -be\r\n", &cfg);
        assert_eq!(
            state.take(),
            Some(XferCmd::Send(XferProtocol::Zmodem))
        );
        // 请求消费掉之后不会从旧尾巴里再冒一次。
        assert!(state.take().is_none());
    }

    #[test]
    fn ctrl_c_cancels_immediately_and_resets_the_x_streak() {
        let mut state = CancelState::default();
        // 第一击 X 只起手，不取消。
        assert!(!cancel_hit(&[key_ctrl(egui::Key::X)], 1.0, &mut state, 1.0));
        assert!(cancel_hit(&[key_ctrl(egui::Key::C)], 1.2, &mut state, 1.0));
        // C 之后旧的 X 清了：再来一击 X 不算连按。
        assert!(!cancel_hit(&[key_ctrl(egui::Key::X)], 1.3, &mut state, 1.0));
    }

    #[test]
    fn double_ctrl_x_within_the_window_cancels_once() {
        let mut state = CancelState::default();
        assert!(!cancel_hit(&[key_ctrl(egui::Key::X)], 10.0, &mut state, 1.0));
        assert!(cancel_hit(&[key_ctrl(egui::Key::X)], 10.9, &mut state, 1.0));
        assert!(!cancel_hit(&[key_ctrl(egui::Key::X)], 10.91, &mut state, 1.0));
    }

    #[test]
    fn control_characters_in_text_events_are_recognised() {
        let mut state = CancelState::default();
        let text = egui::Event::Text("\x03".into());
        assert!(cancel_hit(&[text], 0.0, &mut state, 1.0));
        let mut state = CancelState::default();
        assert!(!cancel_hit(&[egui::Event::Text("\x18".into())], 3.0, &mut state, 1.0));
        assert!(cancel_hit(&[egui::Event::Text("\x18".into())], 3.5, &mut state, 1.0));
    }

    #[test]
    fn cancel_consumption_respects_the_terminal_focus_boundary() {
        let ctx = egui::Context::default();
        let mut win = crate::ui::lifecycle_test_support::window_with(
            std::sync::Arc::new(term_session::SessionPool::new()),
            &ctx,
            vec![],
        );
        ctx.input_mut(|i| i.focused = true);
        assert!(terminal_keyboard_focused(&ctx, &win));
        ctx.input_mut(|i| i.focused = false);
        assert!(!terminal_keyboard_focused(&ctx, &win), "窗口失焦时不能吞键");
        ctx.input_mut(|i| i.focused = true);
        win.view.show_settings = true;
        assert!(
            !terminal_keyboard_focused(&ctx, &win),
            "设置对话框正拦着键盘时不能吞键"
        );
    }

    #[test]
    fn only_matching_cancel_events_are_removed() {
        let (kept, taken) = take_cancel_events(vec![
            egui::Event::Text("a\x03b".to_string()),
            key_ctrl(egui::Key::C),
            key_ctrl(egui::Key::V),
            egui::Event::Text("plain".to_string()),
        ]);
        assert_eq!(taken.len(), 2, "只摘出 Ctrl+C 与 Text 里的 \\x03");
        assert!(matches!(&taken[0], egui::Event::Text(t) if t == "\x03"));
        assert_eq!(kept.len(), 3, "普通键与同一条 Text 的其余字符必须留下");
        assert!(matches!(&kept[0], egui::Event::Text(t) if t == "ab"));
        assert!(matches!(&kept[1], egui::Event::Key { key: egui::Key::V, .. }));
    }

    #[test]
    fn plain_keys_and_other_modifiers_are_ignored() {
        let mut state = CancelState::default();
        let plain = egui::Event::Key {
            key: egui::Key::C,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        assert!(!cancel_hit(&[plain], 1.0, &mut state, 1.0));
        assert!(!cancel_hit(&[], 1.0, &mut state, 1.0));
    }
}
