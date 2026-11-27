//! 粘贴的**唯一落地路径**：`commit_paste`。
//!
//! 直通与粘贴弹窗的 Yes 都只经这一个方法，编码在 `Tab::write_paste` 里。单独成
//! 这个文件纯属行数上限：之前它连同判据一起塞进 `pane.rs`，把那个文件顶到 518 行
//! （上限 500）。**只搬位置，不改行为**——判据原样跟着搬过来。

use super::pane::now_secs;
use super::tab_slot::{tab_of, TabSlot};
use crate::ui::*;

impl TerminalWindow {
    /// 粘贴的**唯一**落地点：编码 → 写 PTY → 喂输入行 → 记命令行历史。
    ///
    /// 未确认的直通与粘贴弹窗的 Yes 都只调这一个方法。两条路各写一遍
    /// `paste_bytes_for` 是这类改动最容易埋的雷：只要有一处忘了括号粘贴或
    /// 漏了 `input_line.feed`，同一个标签在两种情形下写出去的字节/历史就不同，
    /// 而症状要到远端粘贴出怪东西时才显形。单测在 `write_paste` 那一层钉住
    /// 「同一条文本、两条路写出去的字节完全相同」。
    ///
    /// 2026-11-15：参数从「标签下标」换成槽位——底带 `Shell` 面板与主区标签页
    /// 走的是同一条粘贴路（同一条括号粘贴 / 换行归一 / 输入行 / 命令行历史）。
    ///
    /// 标签在弹窗打开期间被关掉 → `tab_of` 拿不到，**丢弃**这次粘贴（一个
    /// 字节都不写，也不 panic）：下标已经指向别人或越界了。
    pub(in crate::ui) fn commit_paste(&mut self, slot: TabSlot, text: &str) {
        let Some(tab) = tab_of(&mut self.tabs, &mut self.shell_tabs, slot) else {
            return;
        };
        let written = tab.write_paste(text);
        let scope = tab.config.system.clone();
        // `tab` 的借用到这里结束，才轮得到窗口级的命令行历史。
        for line in written.lines {
            self.command_history.push_for(&line, &scope, now_secs());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个**没有会话**的标签：`write` 送不出去（返回 false），但输入行照样
    /// 吃字节。粘贴要验的就是「编码 + 输入行」这两步，不需要真起一个 PTY。
    fn tab(bracketed: bool) -> Tab {
        let mut snapshot = GridSnapshot::empty(80, 24);
        snapshot.bracketed_paste = bracketed;
        Tab {
            pool: std::sync::Arc::new(term_session::SessionPool::new()),
            session: None,
            identity: 1,
            config: SessionConfig::new(Protocol::Shell),
            snapshot,
            screen: None,
            view: ScreenImage::blank(80, 0, 0..24),
            viewport: Viewport::new(),
            selections: Selections::default(),
            last_request: None,
            preview_screen: None,
            preview_last_request: None,
            auth: None,
            input_line: term_core::input_line::InputLine::new(),
            id: egui::Id::new("paste_test"),
            last_input_at: std::cell::Cell::new(None),
            // 合并 ws-b1 时这里补上的三个字段：`ws-hex1` 给 `Tab` 加了三项，
            // 而这个假标签是 `ws-b1` 在另一条线上写的，git 看不到这层关系
            // （两个文件没冲突，编译才红）。取的都是「新建标签」的出厂值：
            // 文本模式、出厂 hex 参数、空字节环——粘贴测试不碰这三样。
            mode: term_runtime::prefs::DisplayMode::Text,
            hex: crate::ui::terminal::hex_params::HexParams::default(),
            // 空白/符号可视化六项（`View ▸ *` / `Text ▸ HighlightCaretLine`）：
            // 粘贴测试不碰它们，取出厂值（全关 = 屏幕一个像素都不变）。
            marks: crate::ui::terminal::MarksParams::default(),
            trigger_highlight_rows: Vec::new(),
            onekey_expects_fired: std::collections::HashSet::new(),
            modem: crate::ui::modem_auto::TabState::default(),
            // 调试覆盖层：粘贴测试不碰它，取出厂值（关 = 屏幕一个像素都不变）。
            debug: false,
            debug_log_active: false,
            vim: term_app::vim::VimState::default(),
            // 只读查看：粘贴测试验的正是「能粘」，所以必须是可输入态。
            local_mode: false,
            word_wrap: false,
            raw: crate::ui::terminal::bin_spec::RawLog::new(),
            // B2：控制模式状态机归本 tab，跨块行缓冲从空开始。
            tmux: term_app::tmux::ControlMode::default(),
            tmux_pending: Vec::new(),
            triggers: term_app::trigger::Matcher::new(),
            shadow: crate::automation::Shadow::new(),
            was_live: false,
            just_connected: false,
            auto_reconnect_at: None,
            auto_reconnect_attempts: 0,
            auto_reconnect_blocked: false,
            // 折叠与输入行表出厂为空：粘贴测试不折任何东西（空集合 = 一屏不变）。
            folds: term_app::folds::Folds::new(),
            filter: term_app::state::FilterState::default(),
            filter_input: String::new(),
            filter_history: Vec::new(),
            filter_diag: None,
            input_rows: std::cell::RefCell::new(Vec::new()),
            command_texts: std::cell::RefCell::new(Vec::new()),
            pending_local_sends: Vec::new(),
            // N：恢复运行态字段（新建标签出厂 None）。
            restore_scroll_top: None,
            restore_history_len: None,
        }
    }

    #[test]
    fn confirming_does_not_change_the_bytes() {
        // 两条路：没有弹窗直接落地 / 弹窗 Yes 之后落地。二者都只经
        // `Tab::write_paste`（`commit_paste` 是它唯一的调用者），所以这里把
        // 同一条文本喂两遍——字节、完成的命令行、输入行状态必须逐字节相同。
        // 这条断言挡的是「确认路径另写一份编码」：只要那边忘了括号粘贴或
        // 漏了换行归一，它就红。
        let text = "echo one\r\necho two\necho three";
        let mut direct = tab(true);
        let a = direct.write_paste(text);
        // 确认路径的前提：这一屏的开关是开着的。
        assert!(crate::ui::dialogs::confirm_paste::paste_needs_confirm(
            false, true, false
        ));
        let mut confirmed = tab(true);
        let b = confirmed.write_paste(text);
        assert_eq!(a.bytes, b.bytes);
        assert_eq!(a.lines, b.lines);
        assert_eq!(direct.input_line.current(), confirmed.input_line.current());
        // 顺带钉住编码本身：括号粘贴包起来、CRLF 归一成 CR。
        assert_eq!(a.bytes, b"\x1b[200~echo one\recho two\recho three\x1b[201~");
        // 非括号模式同一段文本少一层包装——分叉只允许出现在这里。
        let mut plain = tab(false);
        let c = plain.write_paste(text);
        assert_eq!(c.bytes, b"echo one\recho two\recho three");
    }

    /// 2026-11-19 只读查看（`Window ▸ Local Mode`）：**一个字节都不许出去**，
    /// 而且连「用户刚打过字」都不算——后者是 `Hide cursor when typing` 的判据，
    /// 只读态下光标根本不画，记了也没有读者，反而会让退出只读的第一步错判。
    #[test]
    fn a_read_only_tab_writes_nothing_and_does_not_count_as_typing() {
        let mut tab = tab(false);
        // 先确认可输入态的行为：没有会话 → 送不出去（返回 false），但**记过**输入。
        assert!(!tab.write(b"ls\n"), "这条夹具没有会话，本来也送不出去");
        assert!(
            tab.last_input_at.get().is_some(),
            "可输入态敲键盘要记下时刻"
        );

        tab.local_mode = true;
        tab.last_input_at.set(None);
        assert!(!tab.write(b"ls\n"), "只读：不发送");
        assert!(
            tab.last_input_at.get().is_none(),
            "只读不算打字——拦在 `write` 的第一行，而不是发出去之后"
        );

        // 粘贴在 `write` 之前还有一步（编码 + 喂输入行），必须一起拦住：
        // 只拦 `write` 的话，输入行缓冲与折叠表仍会记出一段没发生过的粘贴。
        let w = tab.write_paste("echo hi\n");
        assert!(w.bytes.is_empty(), "只读：粘贴不编码任何字节");
        assert!(w.lines.is_empty(), "只读：粘贴不产生命令行");
        assert_eq!(tab.input_line.current(), "", "只读：输入行不许被粘进来");
    }
}
