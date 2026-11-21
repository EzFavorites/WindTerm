//! 会话表单 ↔ `SessionConfig` 的双向映射（从 dialogs 拆出）。
//!
//! 拆的原因不只是行数上限：这套映射是**纯函数**（给它一个配置它吐一张
//! 表单，给它一张表单它吐一个配置），和对话框的绘制代码不是一个变化节奏——
//! 加一个会话字段时动这里，改交互时动 dialogs。混在一个文件里两边都难受。

use crate::ui::panels::tunnels;
use crate::ui::*;
use term_core::session::ProxyType;

/// 「Proxy Type」单选的四档下标（顺序照视觉基线的视觉基线顺序）。
///
/// 之后**协议层四档都有后端**：`0/1` 直连 / Jump 走内置客户端，
/// `2/3` HTTP CONNECT / SOCKS5 走 `term-ssh::outbound`。UI 控件仍保持灰显
/// （放开留到 Proxy 页逐屏比对一起做，避免单独重刷
/// `dialog_new_session_proxy` 组件基线）；但配置层已经能把这四档无损往返，
/// 于是只需把 `add_enabled_ui` 改成四档全放开，不必再动映射。
///
/// 写成常量而不是两侧各写一个数字：单选的下标与 `to_config` 的判定必须是
/// 同一个值，否则「选了 No proxy 却还在走跳板」这类错位迟早会发生。
pub(in crate::ui) const PROXY_NONE: usize = 0;
pub(in crate::ui) const PROXY_JUMP: usize = 1;
pub(in crate::ui) const PROXY_HTTP: usize = 2;
pub(in crate::ui) const PROXY_SOCKS5: usize = 3;

/// 单选下标 → 落盘 tag。未知下标当 `none`，不凭空造出一条代理。
pub(in crate::ui) fn proxy_tag(index: usize) -> &'static str {
    match index {
        PROXY_JUMP => ProxyType::Jump.tag(),
        PROXY_HTTP => ProxyType::Http.tag(),
        PROXY_SOCKS5 => ProxyType::Socks5.tag(),
        _ => ProxyType::None.tag(),
    }
}

/// 落盘档位 → 单选下标。`effective_proxy_type` 是唯一判定入口。
pub(in crate::ui) fn proxy_index(kind: ProxyType) -> usize {
    match kind {
        ProxyType::Jump => PROXY_JUMP,
        ProxyType::Http => PROXY_HTTP,
        ProxyType::Socks5 => PROXY_SOCKS5,
        ProxyType::None => PROXY_NONE,
    }
}

/// Data for the new session dialog.
pub(in crate::ui) struct NewSessionData {
    pub(in crate::ui) protocol: Protocol,
    pub(in crate::ui) name: String,
    pub(in crate::ui) host: String,
    pub(in crate::ui) port: u16,
    pub(in crate::ui) username: String,
    pub(in crate::ui) password: String,
    pub(in crate::ui) auth: AuthMethod,
    /// 认证回退链（有序，空 = 不回退）。勾选里第一个是 `auth`，其余在这里。
    pub(in crate::ui) auth_chain: Vec<AuthMethod>,
    pub(in crate::ui) group: String,
    pub(in crate::ui) keep_alive: u32,
    pub(in crate::ui) connection_timeout: u32,
    pub(in crate::ui) tcp_keep_alive: bool,
    pub(in crate::ui) tcp_low_delay: bool,
    /// 走进程内 SSH 客户端（默认开；老会话没有这一项时为 false）。
    pub(in crate::ui) builtin_ssh: bool,
    /// window.caretStyle (createCaretStyleCombo).
    pub(in crate::ui) caret_style: usize,
    /// session.term (createTermCombo).
    pub(in crate::ui) term: usize,
    /// `session.description`.
    pub(in crate::ui) description: String,
    /// `session.charset`（WindTerm Session 页 `Charset` 下拉）。
    pub(in crate::ui) charset: String,
    /// `session.system`（createSystemCombo）。
    pub(in crate::ui) system: String,
    /// OneKey 宏名（WindTerm Session 页 `OneKey`，默认 `None`）。
    pub(in crate::ui) onekey: String,
    /// Sftp 子系统开关（WindTerm 默认 ON）。
    pub(in crate::ui) sftp: bool,
    /// 系统监控开关（WindTerm 默认 OFF）。
    pub(in crate::ui) sysmon: bool,
    /// 本地 Shell 启动命令（仅 Shell 协议）。
    pub(in crate::ui) shell_command: String,
    pub(in crate::ui) selected_page: usize,
    /// 转发规则（一行一条，`L` / `R` / `D` 开头）。表单存**文本**，
    /// 校验与解析在 `tunnels::parse_rules`——这里只负责搬。
    pub(in crate::ui) forwards: String,
    /// 规则写错时的提示（保存被拦下，对话框不关）。
    pub(in crate::ui) forward_error: Option<String>,
    // ── 10 页页条新增的可编辑字段 ──
    pub(in crate::ui) proxy_jump: String,
    pub(in crate::ui) identity_file: String,
    pub(in crate::ui) agent_forward: bool,
    /// X11 转发（`ssh -X`）。默认关——需要本机有 X server 才有意义。
    pub(in crate::ui) x11_forward: bool,
    /// X11 screen number，随 x11-req 下发给服务端。
    pub(in crate::ui) x11_screen: u32,
    pub(in crate::ui) proxy_type: usize,
    pub(in crate::ui) proxy_host: String,
    pub(in crate::ui) proxy_port: u16,
    pub(in crate::ui) proxy_command: String,
    pub(in crate::ui) working_dir: String,
    /// `Window ▸ Selection` 的两组选区分隔符；空串 = 未设置（页面显示出厂串）。
    pub(in crate::ui) word_separators: String,
    pub(in crate::ui) word_separators_alt: String,
    /// `Sftp ▸ Preserve original file modification time`（传输时写回 mtime）。
    pub(in crate::ui) sftp_preserve_mtime: bool,
    pub(in crate::ui) sftp_preserve_dir_mtime: bool,
    pub(in crate::ui) sftp_fix_invalid_path: bool,
    /// `Terminal ▾ Mode` 的两项初始值（DECAWM / DECCKM）。
    pub(in crate::ui) initial_auto_wrap: bool,
    pub(in crate::ui) initial_cursor_keys_application: bool,
    /// `Keyboard ▸ Backspace / Delete sends:` 的两个下标。
    pub(in crate::ui) backspace_sends: usize,
    pub(in crate::ui) delete_sends: usize,
    pub(in crate::ui) alt_modifier_sends: usize,
    pub(in crate::ui) meta_modifier_sends: usize,
    /// `Bell`：样式 / 自定义声音路径 / 去重窗口。
    pub(in crate::ui) bell_style: usize,
    pub(in crate::ui) bell_sound: String,
    pub(in crate::ui) bell_dedup_secs: u32,
    /// 登录脚本路径（ADR-0027）。空串 = 这条会话连上后不跑脚本。
    pub(in crate::ui) logon_script: String,
    pub(in crate::ui) cols: usize,
    pub(in crate::ui) rows: usize,
    pub(in crate::ui) scrollback: usize,
    pub(in crate::ui) wheel_lines: u32,
    pub(in crate::ui) baud_rate: u32,
    /// 串口 `Data Bits`（5/6/7/8）。
    pub(in crate::ui) data_bits: u8,
    /// 串口 `Stop Bits`（1/2）。
    pub(in crate::ui) stop_bits: u8,
    /// 串口 `Parity`（`None` / `Odd` / `Even`，界面显示串，落盘原样）。
    pub(in crate::ui) parity: String,
    /// 串口 `Flow Control`（`None` / `XON/XOFF` / `RTS/CTS`）。
    pub(in crate::ui) flow_control: String,
}

impl NewSessionData {
    /// 把一条**既有**会话反向填进表单（`Properties` 用）。
    ///
    /// 密码留空：它从来不在 `SessionConfig` 里（那样会被写进 user.sessions），
    /// 所以这里也没有可回填的东西——想改密码就是「这次连接时重新输一遍」。
    pub(in crate::ui) fn from_config(config: &SessionConfig) -> Self {
        Self {
            protocol: config.protocol,
            name: config.name.clone(),
            host: config.host.clone(),
            port: config.port,
            username: config.username.clone(),
            password: String::new(),
            auth: config.auth,
            auth_chain: config.auth_chain.clone(),
            group: config.group.clone(),
            keep_alive: config.keep_alive,
            connection_timeout: config.connection_timeout,
            tcp_keep_alive: config.tcp_keep_alive,
            tcp_low_delay: config.tcp_low_delay,
            builtin_ssh: config.builtin_ssh,
            caret_style: config.caret_style,
            // 不在 `TERM_TYPES` 里的值（手改过 JSON）退回默认项，不把下标丢成 0 之外的东西。
            term: term_core::config::TERM_TYPES
                .iter()
                .position(|t| *t == config.term)
                .unwrap_or_else(|| {
                    term_core::config::TERM_TYPES
                        .iter()
                        .position(|t| *t == term_core::config::DEFAULT_TERM)
                        .unwrap_or(0)
                }),
            description: config.description.clone(),
            charset: config.charset.clone(),
            system: config.system.clone(),
            onekey: config.onekey.clone(),
            sftp: config.sftp,
            sftp_preserve_mtime: config.sftp_preserve_mtime,
            sftp_preserve_dir_mtime: config.sftp_preserve_dir_mtime,
            sftp_fix_invalid_path: config.sftp_fix_invalid_path,
            initial_auto_wrap: config.initial_auto_wrap,
            initial_cursor_keys_application: config.initial_cursor_keys_application,
            backspace_sends: config.backspace_sends,
            delete_sends: config.delete_sends,
            alt_modifier_sends: config.alt_modifier_sends,
            meta_modifier_sends: config.meta_modifier_sends,
            bell_style: config.bell_style,
            bell_sound: config.bell_sound.clone(),
            bell_dedup_secs: config.bell_dedup_secs,
            sysmon: config.sysmon,
            shell_command: config.shell_command.clone(),
            selected_page: 0,
            forwards: config.forwards.join("\n"),
            forward_error: None,
            proxy_jump: config.proxy_jump.clone(),
            identity_file: config.identity_file.clone(),
            agent_forward: config.agent_forward,
            x11_forward: config.x11_forward,
            x11_screen: config.x11_screen,
            proxy_type: proxy_index(config.effective_proxy_type()),
            proxy_host: config.proxy_host.clone(),
            // 0 在配置里表示「没填过」；控件显示一个能用的默认端口，
            // 保存时用户不改就等于填了 1080。
            proxy_port: if config.proxy_port == 0 { 1080 } else { config.proxy_port },
            proxy_command: config.proxy_command.clone(),
            working_dir: if config.working_dir.trim().is_empty() {
                "${HomeDir}".to_string()
            } else {
                config.working_dir.clone()
            },
            word_separators: config.word_separators.clone(),
            word_separators_alt: config.word_separators_alt.clone(),
            logon_script: config.logon_script.clone(),
            cols: if config.cols == 0 {
                term_core::config::DEFAULT_COLS
            } else {
                config.cols
            },
            rows: if config.rows == 0 {
                term_core::config::DEFAULT_ROWS
            } else {
                config.rows
            },
            scrollback: config.scrollback,
            wheel_lines: if config.wheel_lines == 0 {
                term_core::config::MOUSE_WHEEL_SCROLL_LINES
            } else {
                config.wheel_lines
            },
            baud_rate: config.baud_rate,
            data_bits: config.data_bits,
            stop_bits: config.stop_bits,
            parity: config.parity.clone(),
            flow_control: config.flow_control.clone(),
        }
    }

    /// 表单 → 会话配置。
    ///
    /// `base` 是**表单上没有画出来的那些字段**的来源：新建时是
    /// `SessionConfig::new(protocol)`，编辑时是那条会话本身（`charset`、
    /// `scrollback`、串口位宽……保存一次属性不该把它们悄悄抹成默认值）。
    pub(in crate::ui) fn to_config(&self, base: SessionConfig) -> SessionConfig {
        let mut c = base;
        c.protocol = self.protocol;
        c.name = self.name.clone();
        c.host = self.host.clone();
        c.port = self.port;
        c.username = self.username.clone();
        c.auth = self.auth;
        c.auth_chain = self.auth_chain.clone();
        c.group = self.group.clone();
        c.keep_alive = self.keep_alive;
        c.connection_timeout = self.connection_timeout;
        c.tcp_keep_alive = self.tcp_keep_alive;
        c.tcp_low_delay = self.tcp_low_delay;
        // 非 SSH 协议不该带着一个为真的「走内置 SSH 客户端」——
        // 它现在不生效（`uses_builtin_ssh` 要求 SSH），但落盘的配置得是诚实的。
        c.builtin_ssh = self.protocol == Protocol::Ssh && self.builtin_ssh;
        let term = term_core::config::TERM_TYPES
            .get(self.term)
            .map(|t| t.to_string())
            .unwrap_or_else(|| c.term.clone());
        c.term = term;
        c.description = self.description.clone();
        c.charset = self.charset.clone();
        c.system = self.system.clone();
        c.onekey = self.onekey.clone();
        c.sftp = self.sftp;
        c.sftp_preserve_mtime = self.sftp_preserve_mtime;
        c.sftp_preserve_dir_mtime = self.sftp_preserve_dir_mtime;
        c.sftp_fix_invalid_path = self.sftp_fix_invalid_path;
        c.initial_auto_wrap = self.initial_auto_wrap;
        c.initial_cursor_keys_application = self.initial_cursor_keys_application;
        c.backspace_sends = self.backspace_sends;
        c.delete_sends = self.delete_sends;
        c.alt_modifier_sends = self.alt_modifier_sends;
        c.meta_modifier_sends = self.meta_modifier_sends;
        c.bell_style = self.bell_style;
        c.bell_sound = self.bell_sound.clone();
        c.bell_dedup_secs = self.bell_dedup_secs;
        c.sysmon = self.sysmon;
        c.shell_command = self.shell_command.clone();
        // 转发规则：这里只拆行，**不**校验（校验会失败，而 `to_config` 是不会失败的——
        // 失败那一步由 `save_session_dialog` 先跑 `parse_rules` 拦下）。
        c.forwards = tunnels::split_rules(&self.forwards);
        // Proxy / Window / Serial 页的值真的落到配置里，
        // `spawn_plan` 据此生成 `-J` / `-i` / `-A`。
        //
        // 跳板链跟着单选走：单选停在 `No proxy` 时不把链写进配置。两个控件是
        // 同一件事的两半，只认一半就是「控件说直连、配置说走跳板」——那正是
        // 这一批要修的错位，只不过换了个方向。
        c.proxy_jump = if self.proxy_type == PROXY_JUMP {
            self.proxy_jump.clone()
        } else {
            String::new()
        };
        // 出网代理三字段现在有真实落点。四档互斥：选 Jump 就清掉
        // 出网代理，选出网代理就清掉跳板链，选 None 两个都清。此前这三项
        // 只活在表单里，`to_config` 一个都不读——「能填、能存、不生效」。
        c.proxy_type = proxy_tag(self.proxy_type).to_string();
        let outbound = self.proxy_type == PROXY_HTTP || self.proxy_type == PROXY_SOCKS5;
        c.proxy_host = if outbound { self.proxy_host.clone() } else { String::new() };
        c.proxy_port = if outbound { self.proxy_port } else { 0 };
        c.proxy_command = if outbound { self.proxy_command.clone() } else { String::new() };
        c.identity_file = self.identity_file.clone();
        c.agent_forward = self.agent_forward;
        c.x11_forward = self.x11_forward;
        c.x11_screen = self.x11_screen;
        c.scrollback = self.scrollback;
        c.baud_rate = self.baud_rate;
        // 串口五个参数全部落盘，消灭「能填、能存、不生效」。
        c.data_bits = self.data_bits;
        c.stop_bits = self.stop_bits;
        c.parity = self.parity.clone();
        c.flow_control = self.flow_control.clone();
        // 把 Selection / Keyboard / Command / Proxy 页已经能编辑但此前不落盘的
        // 字段真正写进会话配置，消灭「能填、看起来存了、实际没生效」的假控件。
        c.cols = self.cols;
        c.rows = self.rows;
        c.wheel_lines = self.wheel_lines;
        c.caret_style = self.caret_style;
        c.working_dir = self.working_dir.clone();
        c.word_separators = self.word_separators.clone();
        c.word_separators_alt = self.word_separators_alt.clone();
        c.logon_script = self.logon_script.clone();
        c
    }
}

/// 对话框此刻要覆盖哪一条会话。
///
/// 只认序号不够：树里改名、复制、移动分组都会重排 `sessions`，序号会指到别人身上。
/// 名字是那一条的稳定标识（`tabs` 也是按名字认会话的），所以先按序号核对名字，
/// 对不上再按名字找；找不到就当「没有编辑目标」，退化成新建。
pub(in crate::ui) fn editing_index(
    sessions: &[SessionConfig],
    editing: Option<&(usize, String)>,
) -> Option<usize> {
    let (idx, name) = editing?;
    if sessions.get(*idx).map(|s| &s.name) == Some(name) {
        return Some(*idx);
    }
    sessions.iter().position(|s| &s.name == name)
}

/// SessionPane::duplicate calls findUserSession for a free
/// name before adding the tree item, so a copy never collides with its source.
pub(in crate::ui) fn unique_name(sessions: &[SessionConfig], base: &str) -> String {
    let taken = |candidate: &str| sessions.iter().any(|s| s.name == candidate);
    let mut name = format!("{base} (copy)");
    let mut n = 2;
    while taken(&name) {
        name = format!("{base} (copy {n})");
        n += 1;
    }
    name
}

#[cfg(test)]
#[path = "session_form_serial_tests.rs"]
mod session_form_serial_tests;

#[cfg(test)]
#[path = "session_form_auth_tests.rs"]
mod session_form_auth_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_form_opens_on_the_general_page() {
        // 打开 New Session 时停在哪一页，是**行为**，不是渲染细节：
        // 停下标不对，用户第一步就要多点一次。这条以前靠整窗截图「肉眼看」，
        // 现在由单测钉住——组件图只拍得到「当前页」，拍不到「本该是哪页」。
        let data = NewSessionData::from_config(&SessionConfig::new(Protocol::Ssh));
        assert_eq!(data.selected_page, 0);
        assert_eq!(super::super::session_nav::label_for(data.selected_page), "Session",
            "默认停在导航树第一页；页名只由 session_nav 提供");
    }

    fn a_saved_ssh_session() -> SessionConfig {
        let mut c = SessionConfig::new(Protocol::Ssh);
        c.name = "Prod".to_string();
        c.host = "10.0.0.5".to_string();
        c.port = 2222;
        c.username = "deploy".to_string();
        c.group = "Servers".to_string();
        c.auth = AuthMethod::PublicKey;
        c.description = "db box".to_string();
        c.keep_alive = 30;
        // 表单上没有的字段，用来验证「保存不改它们」。
        c.charset = "GBK".to_string();
        c.scrollback = 5000;
        c.baud_rate = 115200;
        c
    }

    #[test]
    fn the_form_shows_what_the_session_says() {
        let f = NewSessionData::from_config(&a_saved_ssh_session());
        assert_eq!(f.name, "Prod");
        assert_eq!(f.host, "10.0.0.5");
        assert_eq!(f.port, 2222);
        assert_eq!(f.username, "deploy");
        assert_eq!(f.group, "Servers");
        assert_eq!(f.auth, AuthMethod::PublicKey);
        assert_eq!(f.description, "db box");
        assert_eq!(f.keep_alive, 30);
        assert!(f.builtin_ssh, "新会话默认走内置客户端，属性页也得显示它是开的");
        // 密码从来不在 SessionConfig 里，只可能留空。
        assert!(f.password.is_empty());
    }

    #[test]
    fn saving_the_form_keeps_the_fields_it_never_showed() {
        let old = a_saved_ssh_session();
        let mut f = NewSessionData::from_config(&old);
        f.host = "10.0.0.6".to_string();
        let saved = f.to_config(old.clone());
        assert_eq!(saved.host, "10.0.0.6");
        // 这三项表单没画：一次「属性」保存不该把它们抹成默认值。
        assert_eq!(saved.charset, "GBK");
        assert_eq!(saved.scrollback, 5000);
        assert_eq!(saved.baud_rate, 115200);
    }

    #[test]
    fn a_form_for_a_telnet_session_does_not_carry_the_builtin_client() {
        let mut f = NewSessionData::from_config(&a_saved_ssh_session());
        f.protocol = Protocol::Telnet;
        let saved = f.to_config(SessionConfig::new(Protocol::Telnet));
        assert_eq!(saved.protocol, Protocol::Telnet);
        assert!(!saved.builtin_ssh, "非 SSH 协议落盘的配置不能声称自己在用 SSH 客户端");
    }

    #[test]
    fn to_config_persists_window_and_shell_fields() {
        // Selection / Keyboard / Command 页能编辑的字段必须落盘，
        // 否则就是「能填、看起来存了、实际没生效」的假控件。
        // 此前 cols/rows/wheel_lines/caret_style/working_dir 在 to_config 里
        // 一个都不写——保存或连接后这些值静默丢失。
        let mut f = NewSessionData::from_config(&SessionConfig::new(Protocol::Ssh));
        f.cols = 120;
        f.rows = 40;
        f.wheel_lines = 3;
        f.caret_style = 4; // Blinking Block
        f.working_dir = "/tmp/work".to_string();
        let saved = f.to_config(SessionConfig::new(Protocol::Ssh));
        assert_eq!(saved.cols, 120, "Selection 页列数必须落盘");
        assert_eq!(saved.rows, 40, "Selection 页行数必须落盘");
        assert_eq!(saved.wheel_lines, 3, "Selection 页滚轮行数必须落盘");
        assert_eq!(saved.caret_style, 4, "Keyboard 页光标样式必须落盘");
        assert_eq!(saved.working_dir, "/tmp/work", "Command 页工作目录必须落盘");
    }

    #[test]
    fn from_config_round_trips_window_and_shell_fields() {
        // 编辑一个存过这些字段的会话时，表单要回显保存的值（0 表示未设时退回默认）。
        let mut c = SessionConfig::new(Protocol::Ssh);
        c.cols = 132;
        c.rows = 43;
        c.wheel_lines = 5;
        c.caret_style = 7; // Blinking Underline
        c.working_dir = "/srv/app".to_string();
        let f = NewSessionData::from_config(&c);
        assert_eq!(f.cols, 132);
        assert_eq!(f.rows, 43);
        assert_eq!(f.wheel_lines, 5);
        assert_eq!(f.caret_style, 7);
        assert_eq!(f.working_dir, "/srv/app");
        // 未设置的字段退回默认值，不把 0 当真值。
        let blank = NewSessionData::from_config(&SessionConfig::new(Protocol::Ssh));
        assert_eq!(blank.cols, term_core::config::DEFAULT_COLS);
        assert_eq!(blank.wheel_lines, term_core::config::MOUSE_WHEEL_SCROLL_LINES);
    }

    #[test]
    fn the_dialog_finds_its_session_after_the_tree_was_resorted() {
        let sessions = vec![
            SessionConfig::new(Protocol::Shell),
            a_saved_ssh_session(),
        ];
        // 打开时它是 1 号；树重排之后序号变成 0，名字还是那条。
        let editing = (1usize, "Prod".to_string());
        assert_eq!(editing_index(&sessions, Some(&editing)), Some(1));
        let sessions = vec![
            a_saved_ssh_session(),
            SessionConfig::new(Protocol::Shell),
        ];
        assert_eq!(editing_index(&sessions, Some(&editing)), Some(0));
    }

    #[test]
    fn the_dialog_gives_up_when_the_session_is_gone() {
        let sessions = vec![SessionConfig::new(Protocol::Shell)];
        let editing = (0usize, "Prod".to_string());
        assert_eq!(editing_index(&sessions, Some(&editing)), None);
        assert_eq!(editing_index(&sessions, None), None);
    }

    #[test]
    fn a_copy_never_collides_with_its_source() {
        let mut taken = SessionConfig::new(Protocol::Shell);
        taken.name = "Local Shell".to_string();
        let sessions = vec![taken];
        assert_eq!(unique_name(&sessions, "Local Shell"), "Local Shell (copy)");
    }

    #[test]
    fn forwarding_rules_go_through_the_form_as_text() {
        let mut old = a_saved_ssh_session();
        old.forwards = vec!["L 8080:db:5432".to_string(), "D 1080".to_string()];
        let mut f = NewSessionData::from_config(&old);
        assert_eq!(f.forwards, "L 8080:db:5432\nD 1080");
        // 改一条、加一条：落盘的应当是干净的两行。
        f.forwards = "  L 8081:db:5432 \n\n D 1080 \n".to_string();
        let saved = f.to_config(old.clone());
        assert_eq!(
            saved.forwards,
            vec!["L 8081:db:5432".to_string(), "D 1080".to_string()]
        );
    }

    #[test]
    fn an_empty_tunnel_page_means_no_forwards_at_all() {
        let mut old = a_saved_ssh_session();
        old.forwards = vec!["L 8080:db:5432".to_string()];
        let mut f = NewSessionData::from_config(&old);
        f.forwards = "   \n".to_string();
        assert!(f.to_config(old).forwards.is_empty());
    }

    #[test]
    fn a_jump_chain_makes_the_form_open_on_the_jump_radio() {
        // 单选与链是同一件事的两半：有链就必须显示 Jump，否则用户看到
        // 「No proxy」而实际在走跳板（或反过来保存时被抹掉）。
        let mut old = a_saved_ssh_session();
        old.proxy_jump = "deploy@bastion:2222".to_string();
        let f = NewSessionData::from_config(&old);
        assert_eq!(f.proxy_type, PROXY_JUMP);
        assert_eq!(f.proxy_jump, "deploy@bastion:2222");
    }

    #[test]
    fn the_proxy_radio_and_the_chain_are_saved_together() {
        let old = a_saved_ssh_session();
        let mut f = NewSessionData::from_config(&old);
        f.proxy_type = PROXY_JUMP;
        f.proxy_jump = "deploy@bastion".to_string();
        assert_eq!(f.to_config(old.clone()).proxy_jump, "deploy@bastion");

        // 单选退回 No proxy：链不能再落盘，否则「点了直连还在走跳板」。
        let mut f = NewSessionData::from_config(&old);
        f.proxy_type = PROXY_NONE;
        f.proxy_jump = "deploy@bastion".to_string();
        assert!(f.to_config(old).proxy_jump.is_empty());
    }

    #[test]
    fn outbound_proxy_fields_survive_a_form_round_trip() {
        // 核心判据：以前 host/port/type 只活在表单里，`to_config`
        // 一个都不读，保存完连上去还是直连。
        let mut old = a_saved_ssh_session();
        old.proxy_type = ProxyType::Http.tag().to_string();
        old.proxy_host = "gw.example".to_string();
        old.proxy_port = 3128;
        old.proxy_command = "nc -X connect -x gw:3128 %h %p".to_string();
        let f = NewSessionData::from_config(&old);
        assert_eq!(f.proxy_type, PROXY_HTTP);
        assert_eq!(f.proxy_host, "gw.example");
        assert_eq!(f.proxy_port, 3128);
        assert_eq!(f.proxy_command, "nc -X connect -x gw:3128 %h %p");

        let saved = f.to_config(old);
        assert_eq!(saved.proxy_type, "http");
        assert_eq!(saved.proxy_host, "gw.example");
        assert_eq!(saved.proxy_port, 3128);
        assert_eq!(saved.proxy_command, "nc -X connect -x gw:3128 %h %p");
        assert_eq!(saved.effective_proxy_type(), ProxyType::Http);
        assert!(saved.validate_proxy().is_none());
        assert_eq!(
            saved.outbound_proxy_spec(),
            Some((ProxyType::Http, "gw.example", 3128))
        );
    }

    #[test]
    fn the_four_proxy_radios_are_mutually_exclusive_in_the_saved_config() {
        let old = a_saved_ssh_session();
        // Socks5：链必须清掉，否则「单选说出网代理、配置却还带 -J」。
        let mut f = NewSessionData::from_config(&old);
        f.proxy_type = PROXY_SOCKS5;
        f.proxy_jump = "bastion".to_string();
        f.proxy_host = "127.0.0.1".to_string();
        f.proxy_port = 1080;
        let saved = f.to_config(old.clone());
        assert_eq!(saved.effective_proxy_type(), ProxyType::Socks5);
        assert!(saved.proxy_jump.is_empty());
        assert_eq!(saved.outbound_proxy_spec().unwrap().2, 1080);

        // 退回 No proxy：出网代理与链都清零，不留下一次「幽灵代理」。
        let mut f = NewSessionData::from_config(&saved);
        f.proxy_type = PROXY_NONE;
        let cleared = f.to_config(saved);
        assert_eq!(cleared.effective_proxy_type(), ProxyType::None);
        assert!(cleared.proxy_host.is_empty());
        assert_eq!(cleared.proxy_port, 0);
        assert!(cleared.outbound_proxy_spec().is_none());
    }

    #[test]
    fn a_jump_only_old_session_opens_on_the_jump_radio_and_stays_a_jump() {
        // 老配置没有 proxy_type：efective 判定必须把它认成 Jump，
        // 表单保存一次后补上显式 tag，行为不变。
        let mut old = a_saved_ssh_session();
        old.proxy_type = String::new();
        old.proxy_jump = "deploy@bastion".to_string();
        let f = NewSessionData::from_config(&old);
        assert_eq!(f.proxy_type, PROXY_JUMP);
        let saved = f.to_config(old);
        assert_eq!(saved.proxy_type, "jump");
        assert_eq!(saved.proxy_jump, "deploy@bastion");
        assert!(saved.outbound_proxy_spec().is_none());
    }

    #[test]
    fn choosing_an_outbound_proxy_without_a_host_is_caught_by_the_form() {
        let old = a_saved_ssh_session();
        let mut f = NewSessionData::from_config(&old);
        f.proxy_type = PROXY_HTTP;
        f.proxy_host = String::new();
        f.proxy_port = 3128;
        let saved = f.to_config(old);
        assert!(saved.validate_proxy().is_some(), "保存前必须能拦下空主机");
    }

    #[test]
    fn the_identity_file_and_agent_flag_are_carried_through_the_form() {
        // Public Keys 页这两个字段有后端（`-i` / `-A`，内置客户端也读），
        // 所以往返必须无损。
        let mut old = a_saved_ssh_session();
        old.identity_file = "~/.ssh/id_work".to_string();
        old.agent_forward = true;
        let mut f = NewSessionData::from_config(&old);
        assert_eq!(f.identity_file, "~/.ssh/id_work");
        assert!(f.agent_forward);

        f.identity_file = "/keys/other.pem".to_string();
        let saved = f.to_config(old);
        assert_eq!(saved.identity_file, "/keys/other.pem");
        assert!(saved.agent_forward);
    }

    #[test]
    fn the_session_page_fields_round_trip_through_the_form() {
        // 重排：Session 页新增的 OneKey/System/Charset/Sftp/Sysmon/Shell
        // 字段必须无损往返，否则「能填、看起来存了、实际没生效」的假控件会回来。
        let mut old = a_saved_ssh_session();
        old.system = "windows".to_string();
        old.charset = "GBK".to_string();
        old.onekey = "Ctrl+A".to_string();
        old.sftp = false;
        old.sftp_preserve_mtime = false;
        old.sftp_preserve_dir_mtime = true;
        old.sftp_fix_invalid_path = false;
        old.initial_auto_wrap = false;
        old.initial_cursor_keys_application = true;
        old.backspace_sends = 1;
        old.delete_sends = 2;
        old.alt_modifier_sends = 1;
        old.meta_modifier_sends = 0;
        old.bell_style = 2;
        old.bell_sound = "/tmp/beep.wav".to_string();
        old.bell_dedup_secs = 5;
        old.sysmon = true;
        let mut f = NewSessionData::from_config(&old);
        assert_eq!(f.system, "windows");
        assert_eq!(f.charset, "GBK");
        assert_eq!(f.onekey, "Ctrl+A");
        assert!(!f.sftp);
        assert!(!f.sftp_preserve_mtime);
        assert!(f.sftp_preserve_dir_mtime);
        assert!(!f.sftp_fix_invalid_path);
        assert!(!f.initial_auto_wrap);
        assert!(f.initial_cursor_keys_application);
        assert_eq!(f.backspace_sends, 1);
        assert_eq!(f.delete_sends, 2);
        assert_eq!(f.alt_modifier_sends, 1);
        assert_eq!(f.meta_modifier_sends, 0);
        assert_eq!(f.bell_style, 2);
        assert_eq!(f.bell_sound, "/tmp/beep.wav");
        assert_eq!(f.bell_dedup_secs, 5);
        assert!(f.sysmon);

        // 改一遍再落盘，确认 to_config 写的是表单值而非 base 的旧值。
        f.system = "macos".to_string();
        f.charset = "Big5".to_string();
        f.onekey = "None".to_string();
        f.sftp = true;
        f.sftp_preserve_mtime = true;
        f.sftp_preserve_dir_mtime = false;
        f.sftp_fix_invalid_path = true;
        f.initial_auto_wrap = true;
        f.initial_cursor_keys_application = false;
        f.backspace_sends = 2;
        f.delete_sends = 0;
        f.alt_modifier_sends = 0;
        f.meta_modifier_sends = 1;
        f.bell_style = 0;
        f.bell_sound.clear();
        f.bell_dedup_secs = 0;
        f.sysmon = false;
        let saved = f.to_config(old);
        assert_eq!(saved.system, "macos");
        assert_eq!(saved.charset, "Big5");
        assert_eq!(saved.onekey, "None");
        assert!(saved.sftp);
        assert!(saved.sftp_preserve_mtime);
        assert!(!saved.sftp_preserve_dir_mtime);
        assert!(saved.sftp_fix_invalid_path);
        assert!(saved.initial_auto_wrap);
        assert!(!saved.initial_cursor_keys_application);
        assert_eq!(saved.backspace_sends, 2);
        assert_eq!(saved.delete_sends, 0);
        assert_eq!(saved.alt_modifier_sends, 0);
        assert_eq!(saved.meta_modifier_sends, 1);
        assert_eq!(saved.bell_style, 0);
        assert!(saved.bell_sound.is_empty());
        assert_eq!(saved.bell_dedup_secs, 0);
        assert!(!saved.sysmon);
    }

    #[test]
    fn a_fresh_shell_session_has_sane_session_page_defaults() {
        // WindTerm 默认 Term/System/Charset/OneKey/Sftp/Sysmon 必须落到合理初值，
        // 否则新建会话首屏就会出现空标签或错位的开关状态。
        let f = NewSessionData::from_config(&SessionConfig::new(Protocol::Shell));
        assert_eq!(f.system, "linux");
        assert_eq!(f.charset, term_core::config::CHARSET);
        assert_eq!(f.onekey, "None");
        assert!(f.sftp, "WindTerm 默认 Sftp 开");
        assert!(f.sftp_preserve_mtime, "WindTerm 默认保留文件 mtime");
        assert!(!f.sftp_preserve_dir_mtime, "目录 mtime 默认关（setstat 非通用）");
        assert!(f.sftp_fix_invalid_path, "非法路径修正默认开");
        assert!(f.initial_auto_wrap, "默认自动换行与内核出厂态一致");
        assert!(!f.initial_cursor_keys_application, "默认方向键是 Normal");
        assert_eq!(f.alt_modifier_sends, 0, "Alt 默认 8-bit characters");
        assert_eq!(f.meta_modifier_sends, 1, "Meta 默认 ESC 前缀");
        assert_eq!(f.bell_style, 1, "Bell 默认 Beep");
        assert!(f.bell_sound.is_empty());
        assert_eq!(f.bell_dedup_secs, 2, "去重窗口默认 2 秒");
        assert!(!f.sysmon, "WindTerm 默认 Sysmon 关");
        assert!(f.shell_command.is_empty(), "Shell 协议未填命令时为空");
    }
}
