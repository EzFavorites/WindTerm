//! New Session 的**非 SSH 算法页**内容（视觉基线的 Terminal / Window 两组、
//! Proxy，以及其余四个协议各自的专属页）。
//!
//! 显示名与分组不在这里，在 `session_nav.rs::PAGES`——导航是按协议分派的
//! 二级树，页面内容只认下标。旧版这里有个 `PAGES` 数组把
//! `SSH / SFTP / Serial / Shell` 当页名平铺到左侧，于是与顶部协议条重复、
//! 也没有层级；那个数组早已删除，导航文字由 `session_nav` 唯一提供。
//!
//! # 两列网格（与唯一的例外）
//!
//! 视觉基线里绝大多数表单是「标签右对齐 / 控件等宽左对齐」的两列结构，
//! 每页走 `form_row` + `Grid`。唯一例外是 `modem_page`：金标准
//! `02-session-dialog/serial/02-serial-xymodem.png` 明确是标签在上、通栏框
//! 在下，且带一条 X/Y/Z 页签条——它改用 `kit::field` / `kit::tabs` 的堆叠原语，
//! 不要按两列网格去「顺手统一」它。
//!
//! #
//!
//! 视觉基线一个叶子一个下标，所以这里每页都必须有**自己的**内容；
//! 旧版 `Modem` 与 `Transfer` 两页显示的是同一组信息行，已合并。
//! `Session / Connection / Authentication / Tunnels` 四页要写真实字段，
//! 在 `session_form_body.rs`；`Ciphers` 等 SSH 算法页在 `session_ssh_pages.rs`。

use super::session_form_body::{form_row, heading, seconds_text, FIELD_W, FORM_W, ROW_H};
use crate::ui::kit::form::{label_right_acc, read_only, Deco};
use crate::ui::kit::field::{choice_row_resp, choice_row_styled, ChoiceKind, UnderlineStyle};
use crate::ui::kit::form::{field_box, spin_field_delta};
use crate::ui::*;

/// 一页的内容。`session_form_body::draw_session_form` 把非状态页委托到这里。
pub(in crate::ui) fn draw_page(win: &mut TerminalWindow, ui: &mut egui::Ui, page: usize) {
    match page {
        5 => proxy_page(win, ui),
        7..=11 => super::session_ssh_pages::draw_page(win, ui, page),
        13 => super::session_shell_pages::shell_process_page(win, ui),
        14 => super::session_shell_pages::shell_process_page(win, ui),
        15 => serial_page(win, ui),
        16 => super::session_modem_pages::modem_page(win, ui),
        20 => sftp_page(win, ui),
        21 => tcp_connection_page(win, ui),
        // Terminal / Window 两组（1 / 2 / 3 / 17 / 18 / 19）在
        // `session_terminal_pages.rs`，不在这里。
        other => super::session_terminal_pages::draw_page(win, ui, other),
    }
}

/// 两列网格的公共骨架。`form_row` 负责把标签右对齐到同一条竖线。
///
/// 横向间距 0：`form_row` 的标签列宽（`LABEL_W` = 93）已经把「标签右缘 318.5 →
/// 字段左缘 321」那 2.5pt 让出来了，再叠一层 Grid 间距会把字段推到 323，
/// 于是同一对话框里 Session 页与子页的输入框差 2pt、上下对不齐。
pub(in crate::ui) fn grid(ui: &mut egui::Ui, id: &'static str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing(egui::vec2(0.0, 4.0))
        .show(ui, add);
}

/// 只读的一行（说明文字 / 默认值），也占两列，保持与输入行同一条起始线。
pub(in crate::ui) fn info_row(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
    form_row(ui, label);
    ui.label(value.into());
    ui.end_row();
}

/// 未接线的项不画成能点的样子，一律灰显 + 一行说明为什么灰。
pub(in crate::ui) fn placeholder_note(ui: &mut egui::Ui, text: &str) {
    // wrap 必须在 label 上显式开：父容器被钉宽后，默认 label 仍按内容算最小
    // 宽度，长说明会画到窗口外被裁（Keyboard 页实测 "these fi" 被截断）。
    ui.add(
        egui::Label::new(egui::RichText::new(text).weak()).wrap(),
    );
}

/// 5 `SSH ▾ Proxy`：视觉基线的两段式，`Proxy Type` radio + `Proxy Settings` 网格。
///
/// # 五项都已接线
///
/// | 控件 | 后端 |
/// |---|---|
/// | `No proxy` / `Jump (ssh -J)` | `SshOptions::proxy_jump` |
/// | `Http proxy` / `Socks v5 proxy` | `OutboundProxy`（内置 SSH 与系统 ssh 两条路径） |
/// | `Host` / `Port` / `Proxy command` / `DNS lookup` | `SessionConfig` → `SpawnPlan` / `OutboundProxy` |
///
/// Working directory 那一行已删除：它与 `Shell ▾ Command` 页是同一个字段，
/// 放在代理页只是重复入口。
/// 5 `SSH ▾ Proxy`：视觉基线 `ssh/13-ssh-proxy.png` 的 `Proxy Type` 下拉框。
///
/// 视觉基线里 Proxy Type 是**下拉框**（不是单选）；且选 `No proxy` 时不显示任何
/// 设置行。选 `Jump (ssh -J)` 显示跳板链（已接线的真控件）；选 HTTP / SOCKS5
/// 显示出网代理的 Host / Port——2026-11-21 起这两个字段也是真控件：内置 SSH
/// 与系统 `ssh` 两条建连路径都读它们（`SessionConfig::outbound_proxy_spec` /
/// 生成 `ProxyCommand`）。
///
/// Proxy command 也接了真：系统 ssh 路径用它覆盖由 Host/Port 生成的 `nc`
/// 等价式（`%h` / `%p` 由 ssh 展开）；内置 SSH 客户端仍只认 Host/Port。
/// 同页的 Working directory 仍灰着：那是本地 Shell 的启动目录，已有独立入口
/// （见 `Shell ▾ Command`），不是代理自己的字段。
fn proxy_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    use super::session_form::{PROXY_JUMP, PROXY_NONE};
    const TYPES: &[&str] = &["No proxy", "Jump (ssh -J)", "Http proxy", "Socks v5 proxy"];
    let mut ptype = win.new_session.proxy_type;
    let name = TYPES.get(ptype).copied().unwrap_or(TYPES[0]);

    // 金标准 `ssh/13-ssh-proxy.png`：普通字重的标签（不是强体、没有下划线），
    // 下拉框通栏 402 宽。选 No proxy 时下面什么都没有——不要补说明文字。
    ui.label("Proxy Type:");
    egui::ComboBox::from_id_salt("proxy_type")
        .selected_text(name)
        .width(FORM_W)
        .show_ui(ui, |ui| {
            for (i, t) in TYPES.iter().enumerate() {
                if ui.selectable_label(ptype == i, *t).clicked() {
                    ptype = i;
                }
            }
        });
    win.new_session.proxy_type = ptype;

    match ptype {
        PROXY_NONE => {}
        PROXY_JUMP => {
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Jump & identity:").strong());
            let mut hops: Vec<String> = win
                .new_session
                .proxy_jump
                .split(',')
                .map(|s| s.trim().to_string())
                .collect();
            hops.resize(4, String::new());
            grid(ui, "new_session_jump_grid", |ui| {
                for i in 0..4 {
                    form_row(ui, &format!("Jump {}:", i + 1));
                    ui.add(
                        TextEdit::singleline(&mut hops[i])
                            .hint_text(if i == 0 { "bastion.example.com" } else { "user@jump2:2222" })
                            .desired_width(FIELD_W),
                    );
                    ui.end_row();
                }
                // IdentityFile 与 agent 的完整控件在 `Public Keys` 页（页 10）；
                // 这里保留 Proxy 页需要的 Jump 链，不重复画同一个输入框。
                info_row(
                    ui,
                    "Agent forwarding:",
                    if win.new_session.agent_forward { "on (-A)" } else { "off" },
                );
            });
            win.new_session.proxy_jump = hops
                .into_iter()
                .map(|h| h.trim().to_string())
                .filter(|h| !h.is_empty())
                .collect::<Vec<_>>()
                .join(",");
        }
        _ => {
            // HTTP / SOCKS5：Host / Port / Proxy command / DNS lookup 都是真控件。
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Proxy Settings:").strong());
            grid(ui, "new_session_proxy_grid", |ui| {
                form_row(ui, "Host:");
                ui.add(
                    TextEdit::singleline(&mut win.new_session.proxy_host)
                        .hint_text("127.0.0.1")
                        .desired_width(FIELD_W),
                );
                ui.end_row();
                form_row(ui, "Port:");
                let (drag, stepped) = spin_field_delta(ui, |ui| {
                    ui.add(DragValue::new(&mut win.new_session.proxy_port).range(1..=65535));
                });
                let _ = drag;
                if let Some(d) = stepped {
                    let port = win.new_session.proxy_port as i64 + d as i64;
                    win.new_session.proxy_port = port.clamp(1, 65535) as u16;
                }
                ui.end_row();
                // Working directory 那一行**删掉**了：它和 Shell ▾ Command 页的
                // `Working directory` 是同一个字段（form.working_dir），在代理页
                // 灰着只是在展示一个不属于代理的重复入口。
                form_row(ui, "Proxy command:");
                ui.add(
                    TextEdit::singleline(&mut win.new_session.proxy_command)
                        .hint_text("nc -X connect -x %h:%p %h %p")
                        .desired_width(FIELD_W),
                );
                ui.end_row();
                form_row(ui, "DNS lookup:");
                choice_row_styled(
                    ui, ChoiceKind::Checkbox, "Perform DNS lookup with proxy", "Perform",
                    &mut win.new_session.dns_over_proxy, true, UnderlineStyle::None,
                );
                ui.end_row();
            });
            placeholder_note(
                ui,
                "Host and Port are used by both the built-in SSH client and the \
                 system ssh path. Proxy command overrides the generated nc command \
                 only on the system ssh path; the built-in client uses Host/Port.",
            );
        }
    }
}

/// 15 `Serial ▾ Protocol`：视觉基线 `serial/01-serial-protocol.png` 的五个下拉框。
///
/// 五个参数现在都是真字段：Baud Rate / Data Bits / Parity / Stop Bits /
/// Flow Control 全部绑定到 `new_session` 并经由 `to_config` 落盘到
/// `SessionConfig`（C2 把原先的  假控件接通）。候选取 WindTerm 出厂集合。
fn serial_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let ns = &mut win.new_session;
    let baud = &mut ns.baud_rate;
    let data_bits = &mut ns.data_bits;
    let stop_bits = &mut ns.stop_bits;
    let parity = &mut ns.parity;
    let flow = &mut ns.flow_control;
    let baud_str = baud.to_string();
    grid(ui, "new_session_serial_grid", |ui| {
        form_row(ui, "Baud Rate:");
        egui::ComboBox::from_id_salt("serial_baud")
            .selected_text(baud_str)
            .width(FIELD_W) // 与同页其余四个下拉等宽，铺满字段列
            .show_ui(ui, |ui| {
                for r in term_core::config::BAUD_RATES {
                    if ui.selectable_label(*baud == r, r.to_string()).clicked() {
                        *baud = r;
                    }
                }
            });
        ui.end_row();
        form_row(ui, "Data Bits:");
        let sel = data_bits.to_string();
        egui::ComboBox::from_id_salt("serial_databits")
            .selected_text(sel)
            .width(FIELD_W)
            .show_ui(ui, |ui| {
                for b in ["5", "6", "7", "8"] {
                    if ui.selectable_label(data_bits.to_string() == b, b).clicked() {
                        *data_bits = b.parse().unwrap_or(term_core::config::SERIAL_DATA_BITS);
                    }
                }
            });
        ui.end_row();
        form_row(ui, "Parity:");
        let sel = parity.to_string();
        egui::ComboBox::from_id_salt("serial_parity")
            .selected_text(sel)
            .width(FIELD_W)
            .show_ui(ui, |ui| {
                for p in ["None", "Odd", "Even"] {
                    if ui.selectable_label(parity.as_str() == p, p).clicked() {
                        *parity = p.to_string();
                    }
                }
            });
        ui.end_row();
        form_row(ui, "Stop Bits:");
        let sel = stop_bits.to_string();
        egui::ComboBox::from_id_salt("serial_stopbits")
            .selected_text(sel)
            .width(FIELD_W)
            .show_ui(ui, |ui| {
                for s in ["1", "2"] {
                    if ui.selectable_label(stop_bits.to_string() == s, s).clicked() {
                        *stop_bits = s.parse().unwrap_or(term_core::config::SERIAL_STOP_BITS);
                    }
                }
            });
        ui.end_row();
        form_row(ui, "Flow Control:");
        let sel = flow.to_string();
        egui::ComboBox::from_id_salt("serial_flow")
            .selected_text(sel)
            .width(FIELD_W)
            .show_ui(ui, |ui| {
                for f in ["None", "XON/XOFF", "RTS/CTS"] {
                    if ui.selectable_label(flow.as_str() == f, f).clicked() {
                        *flow = f.to_string();
                    }
                }
            });
        ui.end_row();
    });
}

/// 定宽只读框：SFTP 页那两个权限值是后端常量，不能装成可编辑字段。
fn read_only_value(ui: &mut egui::Ui, value: &str, width: f32) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, ROW_H),
        egui::Layout::left_to_right(egui::Align::Min),
        |ui| {
            field_box(ui, |ui| {
                read_only(
                    ui,
                    value,
                    crate::theme::chrome(term_core::theme::CHROME_SCALE.text),
                    Deco::None,
                )
            })
        },
    )
    .inner
}

/// 20 `SSH ▾ SFTP`：视觉基线 `shell/19-ssh-sftp.png`。
///
/// 两条权限是真接线（`DEFAULT_FILE_MODE` / `DEFAULT_DIR_MODE`）；三个复选框
/// 分别绑 `sftp_fix_invalid_path` / `sftp_preserve_mtime` /
/// `sftp_preserve_dir_mtime`，经树传输落到对端。
fn sftp_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut fix_invalid = win.new_session.sftp_fix_invalid_path;
    let mut keep_dir_mtime = win.new_session.sftp_preserve_dir_mtime;
    let mut keep_mtime = win.new_session.sftp_preserve_mtime;
    if choice_row_resp(
        ui, ChoiceKind::Checkbox, "Automatically fix invalid path characters", "characters",
        &mut fix_invalid, true, UnderlineStyle::Dialog,
    )
    .on_hover_text(
        "Replace unsafe characters in each destination path segment with '_'. \
         Two different names that collapse to the same target keep the filesystem's \
         overwrite behaviour.",
    )
    .changed()
    {
        win.new_session.sftp_fix_invalid_path = fix_invalid;
    }
    if choice_row_styled(
        ui, ChoiceKind::Checkbox, "Preserve original file modification time", "file",
        &mut keep_mtime, true, UnderlineStyle::Dialog,
    ) {
        win.new_session.sftp_preserve_mtime = keep_mtime;
    }
    if choice_row_styled(
        ui, ChoiceKind::Checkbox, "Preserve original folder modification time", "folder",
        &mut keep_dir_mtime, true, UnderlineStyle::Dialog,
    ) {
        win.new_session.sftp_preserve_dir_mtime = keep_dir_mtime;
    }
    // 金标准 `ssh/19-ssh-sftp.png`：三句勾选之后是两行「右对齐标签 + 145 宽
    // 数值框」。标签列比 93 宽得多（253），否则这两行长标签会被截成 `Default…`。
    ui.add_space(10.0);
    egui::Grid::new("new_session_sftp_grid")
        .num_columns(2)
        .spacing(egui::vec2(5.0, 4.0))
        .show(ui, |ui| {
            label_right_acc(ui, "Default file creation permissions:", 260.0, Some("file"));
            read_only_value(ui, &format!("{:o}", term_core::config::DEFAULT_FILE_MODE), 137.0)
                .on_hover_text("Created files use mode 0644; overwrites keep their own mode.");
            ui.end_row();
            label_right_acc(ui, "Default folder creation permissions:", 260.0, Some("folder"));
            read_only_value(ui, &format!("{:o}", term_core::config::DEFAULT_DIR_MODE), 137.0)
                .on_hover_text("Created folders use mode 0755; existing folders are unchanged.");
            ui.end_row();
        });
}

/// 21 `Telnet ▾ Connection` / `Tcp ▾ Connection`：视觉基线里这两页截图逐字一致，
/// 画的是 TCP 选项（不是 SSH 的 Host/Port）。
///
/// 四个控件里三个已经有读者，一个还没有——所以这一页**部分真**，理由逐行写：
///
/// - `Disable Nagle's algorithm` → `SessionConfig.tcp_low_delay`，建连时
///   `TcpStream::set_nodelay`（`term-io/telnet_channel.rs`）。
/// - `Maximum connection timeout` → `SessionConfig.connection_timeout`，
///   同一个建连循环里逐个地址 `connect_timeout`。
/// - `Enable TCP keepalives` → `SessionConfig.tcp_keep_alive` 有值，但 Telnet
///   的套接字层还没有 `SO_KEEPALIVE` 的落地（SSH 路径已映射成
///   `TCPKeepAlive=yes/no`）。
/// - `Sends null packet` → `SessionConfig.keep_alive`。SSH 路径用它做
///   `ServerAliveInterval`；Telnet 路径在读线程里按同一秒数写一个 NUL（NOOP），
///   0 = 不发。两条协议同一个字段，语义一致。
fn tcp_connection_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut nodelay = win.new_session.tcp_low_delay;
    let mut keepalive = win.new_session.tcp_keep_alive;
    let mut null_pkt = win.new_session.keep_alive;
    let mut timeout = win.new_session.connection_timeout;
    // 视觉基线 `default-session-tcp-connection.png` 与 SSH ▸ Connection 是同一套
    // 「说明在上、控件占满整行」的语法（标题左缘 = 整行控件左缘 x=228）。
    ui.label("TCP connection options:");
    ui.add_space(5.0);
    if choice_row_styled(
        ui, ChoiceKind::Checkbox, "Disable Nagle's algorithm (TCP_NODELAY option)", "Disable",
        &mut nodelay, true, UnderlineStyle::Dialog,
    ) {
        win.new_session.tcp_low_delay = nodelay;
    }
    ui.add_space(4.5);
    choice_row_resp(
        ui, ChoiceKind::Checkbox, "Enable TCP keepalives (SO_KEEPALIVE option)", "Enable",
        &mut keepalive, false, UnderlineStyle::Dialog,
    )
    .on_hover_text(
        "The field is stored per session, but the Telnet socket layer does not \
         set SO_KEEPALIVE yet; the SSH path maps it to TCPKeepAlive.",
    );
    ui.add_space(6.0);
    heading(ui, "Sends null packet to keep session alive:");
    let (drag, stepped) = spin_field_delta(ui, |ui| {
        ui.add(
            egui::DragValue::new(&mut null_pkt)
                .range(0..=3600)
                .custom_formatter(|n, _| seconds_text(n as u32)),
        );
    });
    if drag.changed() {
        win.new_session.keep_alive = null_pkt;
    }
    if let Some(d) = stepped {
        null_pkt = (null_pkt as i64 + d as i64).clamp(0, 3600) as u32;
        win.new_session.keep_alive = null_pkt;
    }
    ui.add_space(6.0);
    heading(ui, "Maximum connection timeout:");
    let (drag, stepped) = spin_field_delta(ui, |ui| {
        ui.add(
            egui::DragValue::new(&mut timeout)
                .range(1..=600)
                .custom_formatter(|n, _| seconds_text(n as u32)),
        );
    });
    if drag.changed() {
        win.new_session.connection_timeout = timeout;
    }
    if let Some(d) = stepped {
        timeout = (timeout as i64 + d as i64).clamp(1, 600) as u32;
        win.new_session.connection_timeout = timeout;
    }
    ui.add_space(6.0);
    placeholder_note(
        ui,
        "Nagle's algorithm, the connection timeout and the null-packet heartbeat \
         apply immediately. SO_KEEPALIVE still needs a Telnet socket layer.",
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_modes_are_the_shipped_644_and_755() {
        // 断言 mode 位本身，不是它十进制看起来像什么：`0o644` 的十进制是 420，
        // 从前写成 `644u16` 时这里也是绿的——那正是它危险的地方。
        assert_eq!(term_core::config::DEFAULT_FILE_MODE, 0o644);
        assert_eq!(term_core::config::DEFAULT_DIR_MODE, 0o755);
        assert_eq!(
            format!("{:o}", term_core::config::DEFAULT_FILE_MODE),
            "644",
            "对话框里显示的三位数"
        );
    }

    #[test]
    fn modem_three_tab_defaults_match_the_reference_capture() {
        // 金标准 `serial/03..05-*-modem-tab.png` 的逐字真值：
        // X=rx+128、Y=rb+1024、Z=rz 且无包长行。出厂值必须在领域模型里，
        // 不能靠页面内临时字面量——否则保存/重开后会漂移。
        let cfg =
            term_core::session::SessionConfig::new(term_core::session::Protocol::Serial);
        assert_eq!(cfg.modem_commands, ["rx", "rb", "rz"]);
        assert_eq!(cfg.modem_large_packets, [false, true, false]);
    }

    #[test]
    fn page_labels_come_from_the_nav_tree_not_a_second_table() {
        // 页名只有一份来源；这里钉住「0 号页」与「越界回退」两件事，
        // 免得有人再把协议名抄回一张平铺数组里。
        use super::super::session_nav::{label_for, PAGE_COUNT};
        assert_eq!(label_for(0), "Session");
        assert_eq!(label_for(99), "Session");
        assert_eq!(label_for(6), "Authentication");
        assert_eq!(label_for(12), "Tunnels");
        // 末页是 追加的 `Window` 组头页（组头自己也是一页）。
        assert_eq!(label_for(PAGE_COUNT - 1), "Window");
    }
}
