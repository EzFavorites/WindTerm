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

use super::session_form_body::{form_row, heading, seconds_text, FIELD_W};
use crate::ui::kit::field::{choice_row_resp, choice_row_styled, ChoiceKind, UnderlineStyle};
use crate::ui::kit::form::field_box;
use crate::ui::*;

/// 一页的内容。`session_form_body::draw_session_form` 把非状态页委托到这里。
pub(in crate::ui) fn draw_page(win: &mut TerminalWindow, ui: &mut egui::Ui, page: usize) {
    match page {
        5 => proxy_page(win, ui),
        7..=11 => super::session_ssh_pages::draw_page(win, ui, page),
        13 => command_page(win, ui),
        14 => environment_page(ui),
        15 => serial_page(win, ui),
        16 => modem_page(win, ui),
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
/// # 这里只有两项是真的（已接线＝有效果）
///
/// | 控件 | 后端 | 处置 |
/// |---|---|---|
/// | `No proxy` / `Jump (ssh -J)` | `SshOptions::proxy_jump` | 可编辑，真的生效 |
/// | `Http proxy` / `Socks v5 proxy` | **没有**：出网代理还没写 | 灰显 + 说明 |
/// | `Host` / `Port` / `Proxy command` / `Working directory` | 只在 `NewSessionData` 里，从不写进 `SessionConfig` | 灰显 + 说明 |
///
/// 后两组以前是「能填、能存、不生效」：填了代理服务器、保存、连上去是直连，
/// 而且一句提示都没有。按  的做法把它们画成不可编辑并说明原因——假控件比
/// 少一个控件更坏。
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

    ui.label(egui::RichText::new("Proxy Type:").strong());
    egui::ComboBox::from_id_salt("proxy_type")
        .selected_text(name)
        .show_ui(ui, |ui| {
            for (i, t) in TYPES.iter().enumerate() {
                if ui.selectable_label(ptype == i, *t).clicked() {
                    ptype = i;
                }
            }
        });
    win.new_session.proxy_type = ptype;

    match ptype {
        PROXY_NONE => {
            placeholder_note(
                ui,
                "No proxy selected: the connection goes direct. \
                 HTTP / SOCKS5 proxies are configured on the other two entries.",
            );
        }
        PROXY_JUMP => {
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Jump & identity:").strong());
            grid(ui, "new_session_jump_grid", |ui| {
                form_row(ui, "ProxyJump chain:");
                ui.add(
                    TextEdit::singleline(&mut win.new_session.proxy_jump)
                        .hint_text("bastion.example.com,user@jump2:2222")
                        .desired_width(FIELD_W),
                );
                ui.end_row();
                // IdentityFile 与 agent 的完整控件在 `Public Keys` 页（页 10）；
                // 这里保留 Proxy 页需要的 Jump 链，不重复画同一个输入框。
                info_row(
                    ui,
                    "Agent forwarding:",
                    if win.new_session.agent_forward { "on (-A)" } else { "off" },
                );
            });
        }
        _ => {
            // HTTP / SOCKS5：出网代理未实现，Host / Port 灰显 + 说明。
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
                ui.add(DragValue::new(&mut win.new_session.proxy_port).range(1..=65535));
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

/// 13 `Shell ▾ Command`：启动目录（本地 Shell 的可编辑项）。
fn command_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    grid(ui, "new_session_command_grid", |ui| {
        form_row(ui, "Working directory:");
        ui.add(
            TextEdit::singleline(&mut win.new_session.working_dir)
                .hint_text("${HomeDir}")
                .desired_width(FIELD_W),
        );
        ui.end_row();
        info_row(ui, "Command:", "$SHELL -l");
        info_row(ui, "On exit:", "close the tab");
    });
    ui.add_space(8.0);
    ui.label("Runs the local user login shell; there is no remote host to configure.");
}

/// 14 `Environment`：登录 shell / TERM / 字符集。
fn environment_page(ui: &mut egui::Ui) {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    grid(ui, "new_session_environment_grid", |ui| {
        info_row(ui, "Login shell:", shell);
        info_row(ui, "TERM:", term_core::config::DEFAULT_TERM);
        info_row(ui, "Charset:", term_core::config::CHARSET);
    });
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
            .width(240.0) // 视觉基线下拉占满右列，不按内容收缩
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
            .show_ui(ui, |ui| {
                for f in ["None", "XON/XOFF", "RTS/CTS"] {
                    if ui.selectable_label(flow.as_str() == f, f).clicked() {
                        *flow = f.to_string();
                    }
                }
            });
        ui.end_row();
    });
    ui.add_space(8.0);
    placeholder_note(
        ui,
        "All five serial parameters are stored per session and passed to the \
         serial backend when the port is opened.",
    );
}

/// 视觉基线里「看起来是下拉、实际改不了」的那一行：灰底只读框，不是可点控件。
fn read_only_value(ui: &mut egui::Ui, value: &str) {
    // 统一字段面 + 正常亮度的只读文字：不再用 `add_enabled(false, TextEdit)`
    // （它把文字冲到几乎不可见）。
    field_box(ui, |ui| {
        crate::ui::kit::form::dead_value(ui, value, crate::ui::kit::form::Deco::None);
    });
}

/// 16 `X/Y/Z Modem`：视觉基线 `02-session-dialog/serial/02-serial-xymodem.png`。
///
/// 布局逐字照原图：**标签独占一行、输入框在下一行且占满整宽**（不是本文件
/// 其它页的两列网格）。顺序：说明 → 协议下拉(Z Modem) → Upload directory +
/// 路径框(带 `| ...` 浏览按钮) → Download directory + 框 → 默认勾选 →
/// X/Y/Z 三页签 → 选中页的字段。`Upload command:` 标签在上、命令框在下占满宽；
/// `Packet size:` 是标签在左、两个单选同一行。
///
/// 三个页签各有 补采的视觉基线真值（`03/04/05-serial-*modem-tab.png`）：
/// X=`rx`+128(XModem) 选中；Y=`rb`+1024(YModem-1K) 选中；Z=`rz` 且**没有**包长行。
///
/// 2026-11-22 起命令与包长是**真字段**（`SessionConfig::modem_commands` /
/// `modem_large_packets`），三页签各自可编辑；页面不再有「灰显说明」那段噪声。
/// 页签切换存 egui memory，不进领域状态；点得动且真换内容。
///
/// 所有几何都来自 `kit::field` / `kit::tabs` / `kit::form` 原语：通栏浏览框、
/// 相邻页签条、与选中页签相连的内容面板，不再由本页手写裸 egui 控件。
fn modem_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    use crate::ui::kit::field::{browse_field, choice_row, underlined_job, ChoiceKind, UnderlineStyle};
    use crate::ui::kit::form::field_box;
    use crate::ui::kit::spec::{FIELD_TO_ROW, LABEL_TO_FIELD};
    use crate::ui::kit::tabs::{panel, strip};

    const TABS: [&str; 3] = ["X Modem", "Y Modem", "Z Modem"];
    const TAB_ID: &str = "new_session_modem_tab";
    let tab: usize = ui.memory(|m| m.data.get_temp::<usize>(egui::Id::new(TAB_ID)).unwrap_or(0));
    let hint = "/Users/unique/Downloads";
    let mut protocol = win.new_session.modem_protocol;
    let mut use_default = win.new_session.modem_use_default;
    let mut up_dir = win.new_session.modem_upload_dir.clone();
    let mut down_dir = win.new_session.modem_download_dir.clone();
    let mut commands = win.new_session.modem_commands.clone();
    let mut large = win.new_session.modem_large_packets;

    // 本页是「标签在上、框在下」的堆叠布局（与同文件其它页的两列网格不同），
    // 节奏必须按金标准逐段给：标签→框 7pt、框→下一标签 9pt。用 scope 把
    // item_spacing 的修改关在本页内，不污染同一弹窗的其它页。
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = FIELD_TO_ROW;

        // 说明行 + 通栏协议下拉（视觉基线下拉框占满内容区宽度）。
        ui.small("Specify the protocol when dragging and dropping files:");
        egui::ComboBox::from_id_salt("modem_protocol")
            .selected_text(TABS[protocol.min(2)])
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for (i, name) in TABS.iter().enumerate() {
                    if ui.selectable_label(protocol == i, *name).clicked() {
                        protocol = i;
                    }
                }
            });

        // 段间留白：9(item_spacing) + 5 = 14px/2 = 7pt，与金标准
        // 「下拉框底 → Upload directory 标签顶」一致。
        ui.add_space(5.0);

        // 目录框：标签独占一行，框通栏、右端内嵌 `| ...` 浏览槽。
        ui.label(egui::WidgetText::from(underlined_job(
            ui, "Upload directory:", "Upload", UnderlineStyle::Dialog,
        )));
        ui.add_space(LABEL_TO_FIELD - FIELD_TO_ROW);
        let (_, browse) = browse_field(ui, &mut up_dir, hint, "...", 24.0, 40.0);
        if browse
            .on_hover_text("Browse for the upload directory")
            .clicked()
        {
            win.open_path_picker(crate::ui::path_picker::PathTarget::ModemUploadDir, &up_dir);
        }

        ui.label(egui::WidgetText::from(underlined_job(
            ui, "Download directory:", "Download", UnderlineStyle::Dialog,
        )));
        ui.add_space(LABEL_TO_FIELD - FIELD_TO_ROW);
        let (_, browse) = browse_field(ui, &mut down_dir, hint, "...", 24.0, 40.0);
        if browse
            .on_hover_text("Browse for the download directory")
            .clicked()
        {
            win.open_path_picker(crate::ui::path_picker::PathTarget::ModemDownloadDir, &down_dir);
        }

        choice_row(
            ui,
            ChoiceKind::Checkbox,
            "Use this as the default and do not ask again",
            "Use",
            &mut use_default,
            true,
        );

        // 勾选行 → 页签条：行距 9pt 已由 item_spacing 给出，不再另加空白。
        let outcome = strip(ui, &TABS, tab, 25.0, 8.0);
        if let Some(next) = outcome.clicked {
            ui.memory_mut(|m| m.data.insert_temp(egui::Id::new(TAB_ID), next));
        }
        // 页签与面板必须**紧贴**：horizontal 之后 egui 还会按 item_spacing.y
        // 推一次光标，这里把它抵消，选中页签才真的与面板顶框连成一条。
        ui.add_space(-FIELD_TO_ROW);

        // 页签下方的内容面板：选中页签与面板顶框相连。
        // 金标准 `02-serial-xymodem.png` @2x：面板 504..1038 → 267 逻辑点。
        panel(ui, outcome.selected_rect, 267.0, |ui| {
            ui.spacing_mut().item_spacing.y = FIELD_TO_ROW;
            let i = outcome.clicked.unwrap_or(tab).min(2);

            ui.label(egui::WidgetText::from(underlined_job(
                ui, "Upload command:", "Upload", UnderlineStyle::Dialog,
            )));
            ui.add_space(LABEL_TO_FIELD - FIELD_TO_ROW);
            field_box(ui, |ui| {
                ui.add_sized(
                    egui::vec2(ui.available_width(), 24.0),
                    egui::TextEdit::singleline(&mut commands[i]).frame(false),
                );
            });

            // 视觉基线 Z 页签没有 Packet size 行，不臆造。
            if i < 2 {
                ui.horizontal(|ui| {
                    ui.label("Packet size:");
                    let proto = if i == 0 { "XModem" } else { "YModem" };
                    let mut small = !large[i];
                    let mut big = large[i];
                    if choice_row(
                        ui,
                        ChoiceKind::Radio,
                        &format!("128 ({proto})"),
                        proto,
                        &mut small,
                        true,
                    ) {
                        large[i] = false;
                    }
                    if choice_row(
                        ui,
                        ChoiceKind::Radio,
                        &format!("1024 ({proto}-1K)"),
                        proto,
                        &mut big,
                        true,
                    ) {
                        large[i] = true;
                    }
                });
            }
        });
    });

    let cfg = &mut win.new_session;
    cfg.modem_protocol = protocol;
    cfg.modem_use_default = use_default;
    cfg.modem_upload_dir = up_dir;
    cfg.modem_download_dir = down_dir;
    cfg.modem_commands = commands;
    cfg.modem_large_packets = large;
}
/// 20 `SSH ▾ SFTP`：视觉基线 `shell/19-ssh-sftp.png`。
///
/// 两条权限现在是**真接线**的（`DEFAULT_FILE_MODE` / `DEFAULT_DIR_MODE`，
/// 新建文件 / 新建目录时由 `term-sftp` 落到对端）；三个复选框等 `SftpOptions`
/// 落地后才有字段。
fn sftp_page(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut fix_invalid = win.new_session.sftp_fix_invalid_path;
    let mut keep_dir_mtime = win.new_session.sftp_preserve_dir_mtime;
    let mut keep_mtime = win.new_session.sftp_preserve_mtime;
    if choice_row_resp(
        ui, ChoiceKind::Checkbox, "Automatically fix invalid path characters", "Automatically",
        &mut fix_invalid, true, UnderlineStyle::None,
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
        ui, ChoiceKind::Checkbox, "Preserve original file modification time", "Preserve",
        &mut keep_mtime, true, UnderlineStyle::None,
    ) {
        win.new_session.sftp_preserve_mtime = keep_mtime;
    }
    if choice_row_styled(
        ui, ChoiceKind::Checkbox, "Preserve original folder modification time", "Preserve",
        &mut keep_dir_mtime, true, UnderlineStyle::None,
    ) {
        win.new_session.sftp_preserve_dir_mtime = keep_dir_mtime;
    }
    ui.add_space(6.0);
    grid(ui, "new_session_sftp_grid", |ui| {
        form_row(ui, "Default file creation permissions:");
        // 常量是 mode 位（`0o644`），这里取八进制三位数显示 —— 视觉基线同样写 644。
        read_only_value(ui, &format!("{:o}", term_core::config::DEFAULT_FILE_MODE));
        ui.end_row();
        form_row(ui, "Default folder creation permissions:");
        read_only_value(ui, &format!("{:o}", term_core::config::DEFAULT_DIR_MODE));
        ui.end_row();
    });
    placeholder_note(
        ui,
        "Modes apply to created items; overwrites keep theirs. File modification time \
         is written back after each transfer; directory timestamps are written after \
         the whole tree arrives. Invalid destination characters become underscores.",
    );
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
        &mut nodelay, true, UnderlineStyle::None,
    ) {
        win.new_session.tcp_low_delay = nodelay;
    }
    ui.add_space(4.5);
    choice_row_resp(
        ui, ChoiceKind::Checkbox, "Enable TCP keepalives (SO_KEEPALIVE option)", "Enable",
        &mut keepalive, false, UnderlineStyle::None,
    )
    .on_hover_text(
        "The field is stored per session, but the Telnet socket layer does not \
         set SO_KEEPALIVE yet; the SSH path maps it to TCPKeepAlive.",
    );
    ui.add_space(6.0);
    heading(ui, "Sends null packet to keep session alive:");
    if ui
        .add(
            egui::DragValue::new(&mut null_pkt)
                .range(0..=3600)
                .custom_formatter(|n, _| seconds_text(n as u32)),
        )
        .changed()
    {
        win.new_session.keep_alive = null_pkt;
    }
    ui.add_space(6.0);
    heading(ui, "Maximum connection timeout:");
    if ui
        .add(
            egui::DragValue::new(&mut timeout)
                .range(1..=600)
                .custom_formatter(|n, _| seconds_text(n as u32)),
        )
        .changed()
    {
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
