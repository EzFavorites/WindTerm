//! tmux 控制模式（`-CC`）的**纯语义解析器**。
//!
//! 这一层只回答「这段控制模式输出说了什么」，不碰 PTY、不碰窗口、不碰时间：
//! `%begin` / `%end` 包一段通知，`%output %<pane> <data>` 把 pane 的字节交给
//! 本地窗格，`%layout-change` 描述窗格树，`%exit` 表示会话结束。
//!
//! **安全降级**是这里最重要的语义：控制模式不是一直开着的。遇到不是 `%`
//! 开头的行、或不认识的指令，一律 `Passthrough` 原样交给普通终端渲染，
//! 而不是「吞掉」——用户不会因为解析失败就看不到 shell 的输出。
//! `docs/design/08-tmux-integration.md` 的「未知指令与解析失败一律降级」由此落地。

#[path = "tmux_commands.rs"]
mod tmux_commands;

/// 一条控制模式事件。`data` 里的 tmux 八进制转义已还原成原始字节。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// `%begin <time> <command-id> <flags>`
    Begin { time: i64, id: u32 },
    /// `%end <time> <command-id> <flags>`
    End { time: i64, id: u32 },
    /// `%output %<pane-id> <data>`：某个窗格的输出。
    Output { pane: u32, data: Vec<u8> },
    /// `%layout-change <window> <layout>`：布局字符串交给布局树解析。
    LayoutChange { window: u32, layout: String },
    /// `%window-add <id>` / `%window-close <id>` / `%window-renamed <id> <name>`
    WindowAdd(u32),
    WindowClose(u32),
    WindowRenamed { window: u32, name: String },
    /// `%session-changed <id> <name>`
    SessionChanged { id: u32, name: String },
    /// `%client-detached <client>`：某个客户端断开（自己的客户端随后还会收到 `%exit`）。
    ClientDetached { client: String },
    /// `%pane-mode-changed <pane>`
    PaneModeChanged(u32),
    /// `%pause` / `%continue`：tmux 的流控开关，没有参数、也不改变会话活性。
    Pause,
    Continue,
    /// `%exit [reason]`：控制模式结束，本地应当回到普通终端。
    Exit { reason: String },
    /// 非控制模式行 / 未知指令：原样透传，**不吞**。
    Passthrough(String),
}

/// tmux 的 `%output` 数据用 `\ooo` 转义字节（尤其 `\015` 回车、换行、
/// 以及字面反斜杠）。这里只做这一件事，纯函数、可金样本验证。
pub fn unescape(data: &str) -> Vec<u8> {
    let bytes = data.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let (a, b, c) = (bytes[i + 1], bytes[i + 2], bytes[i + 3]);
            if (b'0'..=b'7').contains(&a) && (b'0'..=b'7').contains(&b) && (b'0'..=b'7').contains(&c) {
                // 三个八进制位最大 511，u8 乘法会在 debug 下直接 panic（release
                // 下则悄悄绕回成另一个字节）。先在 u32 里算完，超过 255 的
                // 畸形转义按原文透传：不吞用户输出，也不制造假字节。
                let value = ((a - b'0') as u32) * 64
                    + ((b - b'0') as u32) * 8
                    + ((c - b'0') as u32);
                if value <= u8::MAX as u32 {
                    out.push(value as u8);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// 解析一行控制模式输出。`line` 不含结尾换行。
/// tmux 进控制模式时会先发一个 **DCS**（`ESC P 1000 p`，iTerm2 用它识别控制模式），
/// 它没有终止符，会**粘在**第一条 `%begin` 前面。真实 tmux 就是这么发的：
/// 第一行是 `\x1bP1000p%begin 1793920481 264 0`，后面的行才干净。
///
/// 不剥掉的话，第一条 `%begin` 会被当成「不是控制模式的行」透传出去——
/// 于是「一次完整命令」永远等不到开括号（这条是被 Docker 里的真 tmux 打出来的，
/// 见 `apps/desktop/tests/docker_tmux.rs`）。
const DCS_CONTROL_PREAMBLE: &str = "\u{1b}P1000p";

pub fn parse_line(line: &str) -> Event {
    // 只有「剥掉 DCS 之后确实是一条控制模式行」时才用剥后的结果；其余原样透传，
    // 免得把用户程序自己输出的 ESC 序列吃掉。
    let payload = match line.strip_prefix(DCS_CONTROL_PREAMBLE) {
        Some(rest) if rest.starts_with('%') => rest,
        _ => line,
    };
    if !payload.starts_with('%') {
        return Event::Passthrough(line.to_string());
    }
    let line = payload;
    let mut it = line.splitn(2, ' ');
    let head = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("").trim_start();

    match head {
        "%begin" | "%end" => {
            let mut p = rest.split_whitespace();
            let time = p.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let id = p.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            if head == "%begin" {
                Event::Begin { time, id }
            } else {
                Event::End { time, id }
            }
        }
        "%output" => {
            let mut p = rest.splitn(2, ' ');
            let pane = p
                .next()
                .unwrap_or("")
                .trim_start_matches('%')
                .parse()
                .unwrap_or(0);
            let data = unescape(p.next().unwrap_or(""));
            Event::Output { pane, data }
        }
        "%layout-change" => {
            // tmux 真实输出：`%layout-change @0 <current-layout> <old-layout> *`
            // 只有第一个 layout（第 3 个字段）是当前的，其余是旧布局与激活标记。
            let mut parts = rest.split_whitespace();
            let window = parts
                .next()
                .unwrap_or("")
                .trim_start_matches('@')
                .parse()
                .unwrap_or(0);
            let layout = parts.next().unwrap_or("").to_string(); // 只取当前 layout
            Event::LayoutChange { window, layout }
        }
        "%window-add" => Event::WindowAdd(first_u32(rest)),
        "%window-close" => Event::WindowClose(first_u32(rest)),
        "%window-renamed" => {
            let mut p = rest.splitn(2, ' ');
            let window = p.next().unwrap_or("").trim_start_matches('@').parse().unwrap_or(0);
            let name = p.next().unwrap_or("").to_string();
            Event::WindowRenamed { window, name }
        }
        "%session-changed" => {
            let mut p = rest.splitn(2, ' ');
            let id = p.next().unwrap_or("").trim_start_matches('$').parse().unwrap_or(0);
            let name = p.next().unwrap_or("").to_string();
            Event::SessionChanged { id, name }
        }
        "%client-detached" => Event::ClientDetached { client: rest.to_string() },
        "%pane-mode-changed" => Event::PaneModeChanged(first_u32(rest)),
        "%pause" => Event::Pause,
        "%continue" => Event::Continue,
        "%exit" => Event::Exit { reason: rest.to_string() },
        // 未知控制指令：**不吞**，原样透传（安全降级）。
        _ => Event::Passthrough(line.to_string()),
    }
}

fn first_u32(s: &str) -> u32 {
    s.split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches(['%', '@', '$'])
        .parse()
        .unwrap_or(0)
}

/// 控制模式会话的状态机：只跟踪「是否在通知块里」，块内的 `%output`
/// 与块外的同样处理——tmux 的 begin/end 只是命令回显边界，不改变 pane 数据。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControlMode {
    pub active: bool,
    pub in_notification: bool,
    pub panes: Vec<u32>,
    /// 控制流里见过的窗口号（`@N`）。带目标的窗口命令只能从这张集合里选。
    pub windows: Vec<u32>,
    pub layout: String,
    pub exited: bool,
}

impl ControlMode {
    /// 喂一行，返回事件并用它更新状态。退出后置 `active = false`，
    /// 调用方据此把窗格还原成普通终端（降级路径）。
    pub fn feed(&mut self, line: &str) -> Event {
        let ev = parse_line(line);
        match &ev {
            Event::Begin { .. } => {
                self.active = true;
                self.in_notification = true;
            }
            Event::End { .. } => self.in_notification = false,
            Event::Output { pane, .. } => {
                self.active = true;
                if !self.panes.contains(pane) {
                    self.panes.push(*pane);
                }
            }
            Event::LayoutChange { window, layout } => {
                self.active = true;
                self.layout = layout.clone();
                if *window != 0 && !self.windows.contains(window) {
                    self.windows.push(*window);
                }
            }
            Event::WindowAdd(window) => {
                self.active = true;
                if *window != 0 && !self.windows.contains(window) {
                    self.windows.push(*window);
                }
            }
            Event::WindowClose(window) => {
                self.active = true;
                self.windows.retain(|w| w != window);
            }
            Event::WindowRenamed { .. } | Event::SessionChanged { .. }
            | Event::PaneModeChanged(_) => self.active = true,
            // `%pause` / `%continue` 是流控，`%client-detached` 说的是别的客户端：
            // 三者都不代表「本次控制模式还活着」，所以不动 `active`，
            // 真正的结束只认 `%exit`（降级路径唯一入口）。
            Event::Pause | Event::Passthrough(_) | Event::Continue | Event::ClientDetached { .. } => {}
            Event::Exit { .. } => {
                self.active = false;
                self.exited = true;
                self.in_notification = false;
            }
        }
        ev
    }
}

// 反向控制命令族拆到 `tmux_commands.rs`：本文件继续装解析与状态机，
// 命令表单独成文件以守住 500 代码行门禁（公共 API 经此 re-export 不变）。
pub use tmux_commands::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notification_block_parses_begin_output_layout_end() {
        let mut mode = ControlMode::default();
        let lines = [
            "%begin 1700000000 42 0",
            "%output %3 hello\\015\\012world\\015\\012",
            "%layout-change @1 9a3d,120x40,0,0[120x20,0,0{60x20,0,0,1,60x20,61,0,2}]",
            "%end 1700000000 42 0",
        ];
        let events: Vec<Event> = lines.iter().map(|l| mode.feed(l)).collect();
        assert_eq!(events[0], Event::Begin { time: 1700000000, id: 42 });
        assert_eq!(events[1], Event::Output { pane: 3, data: b"hello\r\nworld\r\n".to_vec() });
        assert!(matches!(events[2], Event::LayoutChange { window: 1, .. }));
        assert_eq!(events[3], Event::End { time: 1700000000, id: 42 });
        assert!(mode.active);
        assert!(!mode.in_notification);
        assert_eq!(mode.panes, vec![3]);
        assert!(mode.layout.starts_with("9a3d,"));
    }

    #[test]
    fn octal_escapes_and_literal_backslash_round_trip() {
        // tmux 把字面反斜杠编码成 `\134`；单独两个 `\` 原样保留（不是合法八进制）。
        assert_eq!(unescape("a\\015b\\012\\134"), b"a\rb\n\\".to_vec());
        assert_eq!(unescape("\\\\"), b"\\\\".to_vec());
        assert_eq!(unescape("\\101\\102"), b"AB".to_vec());
        assert_eq!(unescape("plain"), b"plain".to_vec());
    }

    #[test]
    fn malformed_octal_escapes_are_passed_through_without_overflowing() {
        // `\400` 及以上超过 u8；过去这里在 debug 构建里 panic，release 里绕回成
        // 另一个字节。两种都不是「安全降级」，现在原样透传整串。
        for raw in ["\\400", "\\777", "\\500"] {
            assert_eq!(unescape(raw), raw.as_bytes().to_vec(), "{raw}");
        }
        let mut mixed = vec![255];
        mixed.extend_from_slice(b"\\777");
        mixed.push(b'A');
        assert_eq!(unescape("\\377\\777\\101"), mixed);
        // 合法区间照旧解码，边界 \377 = 255 不丢。
        assert_eq!(unescape("\\377"), vec![255]);
        assert_eq!(unescape("\\000"), vec![0]);
        assert_eq!(parse_line("%output %0 \\777"), Event::Output {
            pane: 0,
            data: b"\\777".to_vec(),
        });
    }

    #[test]
    fn plain_shell_output_is_never_swallowed() {
        let mut mode = ControlMode::default();
        let ev = mode.feed("bash-3.2$ echo hi");
        assert_eq!(ev, Event::Passthrough("bash-3.2$ echo hi".to_string()));
        assert!(!mode.active);
    }

    #[test]
    fn an_unknown_directive_degrades_to_passthrough() {
        let ev = parse_line("%something-new foo bar");
        assert_eq!(ev, Event::Passthrough("%something-new foo bar".to_string()));
    }

    #[test]
    fn exit_closes_control_mode_and_marks_degraded() {
        let mut mode = ControlMode::default();
        mode.feed("%output %1 x");
        let ev = mode.feed("%exit detached");
        assert_eq!(ev, Event::Exit { reason: "detached".to_string() });
        assert!(!mode.active);
        assert!(mode.exited);
    }

    #[test]
    fn reverse_commands_are_well_formed_and_quoted_when_needed() {
        assert_eq!(Command::SplitHorizontal.line(), "split-window -h");
        assert_eq!(Command::SelectPane(7).line(), "select-pane -t %7");
        assert_eq!(Command::Detach.line(), "detach-client");
        assert_eq!(Command::RenameWindow("my window".into()).quoted_line(), "rename-window \"my window\"");
        assert_eq!(Command::RenameWindow("prod".into()).quoted_line(), "rename-window prod");
    }

    #[test]
    fn window_lifecycle_and_session_events_are_recognized() {
        assert_eq!(parse_line("%window-add @2"), Event::WindowAdd(2));
        assert_eq!(parse_line("%window-close @2"), Event::WindowClose(2));
        assert_eq!(parse_line("%pane-mode-changed %4"), Event::PaneModeChanged(4));
        assert_eq!(
            parse_line("%session-changed $3 work"),
            Event::SessionChanged { id: 3, name: "work".to_string() }
        );
    }

    #[test]
    fn a_dcs_preamble_glued_to_the_first_begin_is_still_a_begin() {
        // 真 tmux 的首行是 `\x1bP1000p%begin <time> <id> <flags>`：这条用例是
        // 被 Docker 里的真 tmux 打出来的（见 apps/desktop/tests/docker_tmux.rs）。
        let event = parse_line("\u{1b}P1000p%begin 1793920481 264 0");
        assert_eq!(event, Event::Begin { time: 1793920481, id: 264 });
    }

    #[test]
    fn a_program_escaped_line_is_still_passed_through() {
        // 不是控制模式的行，哪怕带 ESC 也不能被当成 DCS 吃掉。
        let line = "\u{1b}[31mred\u{1b}[0m";
        assert_eq!(parse_line(line), Event::Passthrough(line.to_string()));
    }

    #[test]
    fn window_rename_client_detach_and_flow_control_are_events() {
        assert_eq!(
            parse_line("%window-renamed @2 my window"),
            Event::WindowRenamed { window: 2, name: "my window".to_string() }
        );
        assert_eq!(
            parse_line("%client-detached /dev/ttys004"),
            Event::ClientDetached { client: "/dev/ttys004".to_string() }
        );
        assert_eq!(parse_line("%pause"), Event::Pause);
        assert_eq!(parse_line("%continue"), Event::Continue);
        // 近似但不同的指令仍然透传：不认识就**不许**假装认识。
        assert_eq!(parse_line("%paused"), Event::Passthrough("%paused".to_string()));
        assert_eq!(parse_line("%client-attached foo"), Event::Passthrough("%client-attached foo".to_string()));
    }

    #[test]
    fn flow_control_and_foreign_detach_do_not_end_the_session() {
        let mut mode = ControlMode::default();
        mode.feed("%window-add @1");
        assert!(mode.active);
        mode.feed("%pause");
        mode.feed("%continue");
        mode.feed("%client-detached /dev/ttys004");
        assert!(mode.active, "只有 %exit 能结束控制模式");
        assert!(!mode.exited);
        // 未知指令同样不能把 active 打回 false（安全降级只朝「继续渲染」的方向走）。
        mode.feed("%something-new");
        assert!(mode.active);
    }

    #[test]
    fn every_reverse_command_has_a_well_formed_line() {
        assert_eq!(Command::SplitHorizontal.line(), "split-window -h");
        assert_eq!(Command::SplitVertical.line(), "split-window -v");
        assert_eq!(Command::SelectPane(7).line(), "select-pane -t %7");
        assert_eq!(Command::KillPane(7).line(), "kill-pane -t %7");
        assert_eq!(Command::NewWindow.line(), "new-window");
        assert_eq!(Command::SelectWindow(3).line(), "select-window -t @3");
        assert_eq!(Command::RenameWindow("prod".into()).line(), "rename-window prod");
        assert_eq!(Command::RenameWindow("my window".into()).line(), "rename-window \"my window\"");
        // 单引号在 tmux 命令行里同样是引号；换行会截断命令，必须消掉。
        assert_eq!(Command::RenameWindow("it's".into()).line(), "rename-window \"it's\"");
        assert_eq!(Command::RenameWindow("a\nb; c".into()).line(), "rename-window \"a b; c\"");
        assert_eq!(Command::Detach.line(), "detach-client");
        assert_eq!(Command::Refresh.line(), "refresh-client");
        assert_eq!(Command::Resize { cols: 80, rows: 24 }.line(), "refresh-client -C 80x24");
    }

    #[test]
    fn the_four_documented_pane_features_have_their_control_lines() {
        // Kill Pane（page/2）：无目标即当前窗格；带目标仍走面板的窗格集合校验。
        assert_eq!(Command::KillPaneActive.line(), "kill-pane");
        assert_eq!(Command::KillPane(3).line(), "kill-pane -t %3");
        // Rename Pane（page/3）：名称与窗格号都不能成为第二条命令。
        assert_eq!(Command::RenamePane { pane: 2, name: "build".into() }.line(), "rename-pane -t %2 build");
        assert_eq!(
            Command::RenamePane { pane: 2, name: "a b; c\nd".into() }.line(),
            "rename-pane -t %2 \"a b; c d\"",
        );
        // Select Pane（page/4）：方向键不依赖解析出的窗格号。
        assert_eq!(Command::SelectPaneDirection(PaneDir::Left).line(), "select-pane -L");
        assert_eq!(Command::SelectPaneDirection(PaneDir::Right).line(), "select-pane -R");
        assert_eq!(Command::SelectPaneDirection(PaneDir::Up).line(), "select-pane -U");
        assert_eq!(Command::SelectPaneDirection(PaneDir::Down).line(), "select-pane -D");
        // Synchronize Panes（page/5）：省略值 = tmux 自己切换该 window 选项。
        assert_eq!(Command::SynchronizePanes.line(), "set-window-option synchronize-panes");
    }

    #[test]
    fn the_remaining_official_tmux_features_have_control_lines() {
        // 官网目录：Split / Zoom / Kill Window+Session / Swap / Move / Layout / Window 切换。
        assert_eq!(Command::SplitPaneLeft.line(), "split-window -h -b");
        assert_eq!(Command::SplitPaneUp.line(), "split-window -v -b");
        assert_eq!(Command::TogglePaneZoom.line(), "resize-pane -Z");
        assert_eq!(Command::KillWindow.line(), "kill-window");
        assert_eq!(Command::KillSession.line(), "kill-session");
        assert_eq!(Command::SwapPane.line(), "swap-pane");
        assert_eq!(Command::SwapWindow.line(), "swap-window");
        assert_eq!(Command::MovePaneToNewWindow.line(), "break-pane");
        assert_eq!(Command::SelectWindowPrevious.line(), "select-window -p");
        assert_eq!(Command::SelectWindowNext.line(), "select-window -n");
        assert_eq!(Command::SelectLayout("tiled").line(), "select-layout tiled");
        assert_eq!(
            Command::RenameSession("a b;c".into()).line(),
            "rename-session \"a b;c\"",
        );
    }

    #[test]
    fn send_keys_text_cannot_inject_a_second_command() {
        // 空格：不加引号会被拆成多个 key，丢掉空格本身。
        assert_eq!(send_text(1, "ls -l").line(), "send-keys -t %1 -l \"ls -l\"");
        // 分号：tmux 的命令分隔符，必须落在引号里。
        assert_eq!(
            send_text(2, "x; kill-server").line(),
            "send-keys -t %2 -l \"x; kill-server\""
        );
        // 引号与反斜杠：转义后才能表示「用户真的想输入这两个字符」。
        assert_eq!(
            send_text(3, "say \"hi\"; rm -rf /").line(),
            "send-keys -t %3 -l \"say \\\"hi\\\"; rm -rf /\""
        );
        assert_eq!(
            send_text(4, "C:\\tmp").line(),
            "send-keys -t %4 -l \"C:\\\\tmp\""
        );
        // 转义入口与 line() 一致：不存在「忘了走 quoted_line」的后门。
        assert_eq!(send_text(5, "a b").quoted_line(), send_text(5, "a b").line());
    }

    #[test]
    fn control_mode_tracks_windows_and_drops_closed_ones() {
        let mut mode = ControlMode::default();
        mode.feed("%window-add @1");
        mode.feed("%layout-change @2 main-horizontal main-vertical *");
        mode.feed("%window-close @1");
        assert_eq!(mode.windows, vec![2]);
        assert!(mode.active);
    }

    #[test]
    fn resize_pane_targets_the_pane_not_the_client() {
        assert_eq!(
            Command::ResizePane { pane: 7, cols: 120, rows: 40 }.line(),
            "resize-pane -t %7 -x 120 -y 40"
        );
    }

    #[test]
    fn a_newline_never_reaches_the_wire_inside_a_command() {
        // 换行会截断控制命令（等于注入），所以文本里出现控制字符时改走十六进制。
        assert_eq!(send_text(1, "a\nb").line(), "send-keys -t %1 -H 61 0a 62");
        assert_eq!(Command::SendText { pane: 2, text: "x\ry".into() }.line(), "send-keys -t %2 -H 78 0d 79");
    }

    #[test]
    fn raw_send_keeps_non_utf8_bytes_intact() {
        assert_eq!(send_keys_raw(1, b"hi").line(), "send-keys -t %1 -l \"hi\"");
        assert_eq!(
            send_keys_raw(2, b"h\xffi").line(),
            "send-keys -t %2 -H 68 ff 69"
        );
        assert_eq!(
            send_keys_raw(3, &[0x00, 0x1b, 0x7f]).line(),
            "send-keys -t %3 -H 00 1b 7f"
        );
        // 合法 UTF-8 仍然走文本路径，别把中文拆成字节。
        assert_eq!(send_keys_raw(4, "中文".as_bytes()).line(), "send-keys -t %4 -l \"中文\"");
        // 空输入不产生多余的尾随空格。
        assert_eq!(send_keys_raw(5, b"").line(), "send-keys -t %5 -l \"\"");
    }
}
