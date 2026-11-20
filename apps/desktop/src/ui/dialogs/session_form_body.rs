//! New Session 右侧**表单本体**：按 `selected_page` 分派到各内容页。
//!
//! R9 / verify 4j 要求单文件代码行（非空非注释）≤ 500——**不是 450**，那个
//! 旧阈值早在 2026-11-09 就并成一档了（见 `scripts/verify.sh` 里 4j 的注释）。
//! 本文件现在 467 行。窗口外框 / 协议条 /
//! 导航树在 `new_session.rs`，纯静态与半静态页在 `session_pages.rs`，
//! SSH 算法页在 `session_ssh_pages.rs`；本文件只放**要写真实字段**的
//! 四页（Session / Connection / Authentication / Tunnels）。
//!
//! P8 之前这四页里的 Authentication 与 Tunnels 控件是硬塞在 Connection
//! 页底部的两段「extras」——视觉基线 `session-property.png` 里它们各自是
//! `SSH ▾` 下的独立二级项。现在一棵树一个叶子、一个下标一页，点进去
//! 不会再几项内容雷同，也没有任何已接线字段被删。

use crate::ui::*;
use super::SessionDialogKind;
use crate::ui::kit::switch::{trigger_switch, SwitchProps};

/// 右栏表单字段的统宽。视觉基线 `04-new-session/00-ssh-session.png` 与
/// `default-session-ssh-session.png` 两张图里输入框都是物理 x=642..1260
/// （弹窗左缘 0 起算）⇒ 逻辑 321..630，**画出来 309**。
///
/// 给 307 而不是 309：egui 把 1pt 描边画在 rect 边界上并各外扩半像素，
/// `add_sized(309)` 画出来是 311（右缘跑到 632，距弹窗右只剩 8）。
pub(in crate::ui) const FIELD_W: f32 = 307.0;

/// 右栏**内容**总宽。视觉基线 `02-session-dialog/ssh/01-ssh-connection.png` 量得
/// 整行控件左缘 x=228（分隔线 218 再留 10）、右缘 630（距弹窗右 10）⇒ 画出来 402。
/// 「标题在上、控件占满整行」那几页（Connection / Authentication）用它铺宽。
///
/// 同 `FIELD_W`：给 400 才画出 402。
pub(in crate::ui) const FORM_W: f32 = 400.0;

/// 标签列宽。视觉基线里标签右缘 x≈318.5、输入框左缘 321；右栏内容从 x=228
/// 起算 ⇒ 标签列 93（`form_row` 内部再右缩 2）。
///
/// 93 只放得下约 13 个等宽字符：`Description:` 刚好，再长就得截（`fit_label`
/// 内按像素宽度判定）。那些长标签页（`Word Separators For Alternate Screen:`…）
/// 在参考实现里根本不是两列表单，而是「标题在上、控件占满整行」——见 `heading`
/// 的注释；它们还等着按那个语法重排，在那之前先保证不压到左栏树上。
pub(in crate::ui) const LABEL_W: f32 = 93.0;

/// 行高与行距。视觉基线量得：**画出来的框高 23**（两条 1pt 边框外沿相距 23）、
/// 相邻框顶相距 29.375（8 行共 235pt）。egui 把 1pt 描边画在 rect 边界上、
/// 各外扩半像素，所以 rect 要给 22 才画出 23；缺口补 7.4 凑成 29.4 的行距
/// （29.4 与实测 29.375 相差 0.2pt，8 行累计 1.6pt，落在原图自身 ±1px 的
/// 取整噪声内）。
pub(in crate::ui) const ROW_H: f32 = 22.0;
pub(in crate::ui) const ROW_GAP: f32 = 7.4;

/// Grid 的标签列：右对齐到同一条竖线上（视觉基线 G4 的 `Host:` / `Port:` 就是右对齐）。
/// 用 `with_layout` 包一层而不是直接 `ui.label`——后者会左对齐，网格列宽再准也散。
pub(in crate::ui) fn form_row(ui: &mut egui::Ui, label: &str) {
    // 固定 LABEL_W 标签列并右对齐：只靠 with_layout 不指定宽度时 Grid 列宽
    // 由最长标签决定，短标签会落回左边、整列对不齐。
    form_row_colored(ui, label, crate::theme::chrome(term_core::theme::CHROME_SCALE.text));
}

/// 同一列但允许改色：视觉基线 Session 页只有 `OneKey:` 是橙色警示色。
fn form_row_colored(ui: &mut egui::Ui, label: &str, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(LABEL_W, ROW_H), egui::Sense::hover());
    ui.painter().text(
        rect.right_center() - egui::vec2(2.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        fit_label(ui, label),
        egui::TextStyle::Body.resolve(ui.style()),
        color,
    );
}

/// 把标签收进标签列：放得下就整条画，放不下才按**实际像素宽度**截断加省略号。
///
/// 从前这里按「超过 20 个字符」硬截，于是 `Sends null packet to keep session
/// alive:` 明明可以整条显示却还是变成 `Sends null packet t…`。按宽度判断之后，
/// Session / Connection 这类标签都能整条画出来；只有真的塞不下标签列的长标签
/// （`Word Separators For Alternate Screen:` 这类，参考实现里根本不是两列表单、
/// 而是「标题在上」）才截断——截断处既不压到左栏树上，也不把控件挤走。
fn fit_label(ui: &egui::Ui, label: &str) -> String {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let avail = LABEL_W - 4.0;
    let full = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), egui::Color32::WHITE)
        .size()
        .x;
    if full <= avail {
        return label.to_string();
    }
    let mut kept = String::new();
    for ch in label.chars() {
        let probe = format!("{kept}{ch}…");
        let w = ui
            .painter()
            .layout_no_wrap(probe.clone(), font.clone(), egui::Color32::WHITE)
            .size()
            .x;
        if w > avail {
            break;
        }
        kept.push(ch);
    }
    format!("{kept}…")
}

/// 视觉基线里 `OneKey:` 是唯一橙色标签（提示它是宏/需留意），其余标签都是正文灰。
fn form_row_accent(ui: &mut egui::Ui, label: &str) {
    form_row_colored(ui, label, egui::Color32::from_rgb(0xF5, 0x8C, 0x2E));
}

/// 「标题在上、控件占满整行」那几页的一行说明——参考实现 Session 的**大多数**
/// 二级页都是这个语法，不是两列网格：
/// `01-ssh-connection.png`（`Sends null packet…` 在框上方）、
/// `default-session-keyboard.png`（`Alt modifier key sends:` 在框上方）、
/// `default-session-mode.png`（`Cursor Keys mode (DECCKM):` 在单选行上方）
/// 三张图逐张量过，标题左缘都等于整行控件的左缘 x=228。
///
/// 标题文字顶到下面控件文字顶 18.5pt（视觉基线 127.5 → 146）。标题自身占
/// 11pt、`item_spacing.y` 再给 2.5，剩下的 5 由这里补——少补 3pt 时实测
/// 两个数值框整体比原图高 3~5pt。
pub(in crate::ui) fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(text);
    ui.add_space(5.0);
}

/// 整行 SpinBox：视觉基线 Connection 页那两个 `off` / `20 seconds` 是**占满
/// 整行**的数值框（402 宽、左对齐、右端上下箭头），不是两列网格里的短控件。
///
/// 交互仍由 `DragValue` 承担（点击输入、拖动步进），外观由 `spin_box`
/// 按视觉基线重画；`show` 决定非编辑态显示的文案（`0` 显示 `off`，
/// 其余显示 `N seconds`——与 `custom_formatter` 一致）。
pub(in crate::ui) fn full_spin(
    ui: &mut egui::Ui,
    value: &mut u32,
    show: fn(u32) -> String,
) -> egui::Response {
    let resp = ui
        .with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
            ui.add_sized(
                egui::vec2(FORM_W, ROW_H),
                DragValue::new(value)
                    .range(0..=3600)
                    .custom_formatter(|n, _| show(n as u32)),
            )
        })
        .inner;
    spin_box(ui, resp.rect, &show(*value), resp.hovered());
    resp
}

/// `0` = `off`，其余 `N seconds`。视觉基线两个框分别显示 `off` / `20 seconds`。
pub(in crate::ui) fn seconds_text(v: u32) -> String {
    if v == 0 {
        "off".to_string()
    } else {
        format!("{v} seconds")
    }
}

/// 高单元格的标签：**顶对齐**而不是垂直居中。
///
/// 视觉基线 `Description:` 的标签中心在 y≈113.75，紧贴多行框的第一行文字；
/// Grid 默认把短标签在 95pt 高的行里居中（实测落到 150），看起来像浮在框中间。
fn form_row_top(ui: &mut egui::Ui, label: &str, height: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(LABEL_W, height), egui::Sense::hover());
    let font = egui::TextStyle::Body.resolve(ui.style());
    ui.painter().text(
        egui::pos2(rect.right() - 2.0, rect.top() + ROW_H / 2.0),
        egui::Align2::RIGHT_CENTER,
        fit_label(ui, label),
        font,
        crate::theme::chrome(term_core::theme::CHROME_SCALE.text),
    );
}

/// SpinBox 右端的上下箭头（视觉基线 `Port:` 那一格）。只画装饰：步进仍由
/// DragValue 的拖动承担，避免为两个小三角另起一套交互状态。
fn spin_arrows(ui: &egui::Ui, rect: egui::Rect) {
    let p = ui.painter();
    let c = crate::theme::chrome(term_core::theme::CHROME_SCALE.icon);
    let x = rect.right() - 9.0;
    let cy = rect.center().y;
    // 上三角 / 下三角各 4pt 高、7pt 宽。
    p.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(x - 3.5, cy - 2.0),
            egui::pos2(x + 3.5, cy - 2.0),
            egui::pos2(x, cy - 6.0),
        ],
        c,
        egui::Stroke::NONE,
    ));
    p.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(x - 3.5, cy + 2.0),
            egui::pos2(x + 3.5, cy + 2.0),
            egui::pos2(x, cy + 6.0),
        ],
        c,
        egui::Stroke::NONE,
    ));
}

/// 把 DragValue 画成视觉基线那种 SpinBox：数值**左对齐**、右端上下箭头。
///
/// egui 的非编辑态 DragValue 是一个 `Button`，文字恒居中；而视觉基线的 `Port`
/// 数值贴左、右侧留给箭头。这里保留 DragValue 的全部交互（点击输入、拖动
/// 步进），只把它画出来的中间区域按当前底色盖掉、再按视觉基线重画。
fn spin_box(ui: &egui::Ui, rect: egui::Rect, text: &str, hovered: bool) {
    let s = &term_core::theme::CHROME_SCALE;
    let fill = crate::theme::chrome(if hovered { s.hover } else { s.field });
    let p = ui.painter();
    p.rect_filled(rect.shrink(1.0), 1.0, fill);
    let font = egui::TextStyle::Body.resolve(ui.style());
    p.text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        font,
        crate::theme::chrome(s.text),
    );
    spin_arrows(ui, rect);
}

/// 下拉候选：终端系统类型（影响功能键 / 转义序列）。
const SYSTEM_TYPES: &[&str] = &[
    "linux", "macos", "windows", "freebsd", "openbsd", "netbsd", "sunos", "aix", "hpux",
];
/// 下拉候选：字符集。
const CHARSETS: &[&str] = &[
    "UTF-8", "GBK", "GB2312", "ISO-8859-1", "ISO-8859-15", "KOI8-R", "EUC-KR", "EUC-JP",
    "Shift_JIS", "Big5",
];
/// 下拉候选：OneKey 宏名。
const ONEKEYS: &[&str] = &[
    "None", "Ctrl+A", "Ctrl+E", "Ctrl+K", "Ctrl+U", "Ctrl+W", "Ctrl+Y", "Alt+B", "Alt+F",
    "Alt+D",
];

/// `&str` 候选的下拉框：选中值写回 `current`；当前值不在候选里也保留显示。
fn combo_str(ui: &mut egui::Ui, id: &str, options: &[&str], current: &mut String) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(current.clone())
        .width(FIELD_W)
        .show_ui(ui, |ui| {
            for opt in options {
                if ui
                    .selectable_label(*opt == current.as_str(), *opt)
                    .clicked()
                {
                    *current = (*opt).to_string();
                }
            }
            if !options.contains(&current.as_str()) && !current.is_empty() {
                // 当前值不在候选里（手改过 JSON）：仍显示为已选项，让用户看见自己的值。
                // 这是一个只读展示项，不需要处理点击。
                let _ = ui.selectable_label(true, current.as_str());
            }
        });
}

/// `Term` 下拉框：候选来自 `TERM_TYPES`，`current` 是下标。
fn combo_term(ui: &mut egui::Ui, current: &mut usize) {
    let text = term_core::config::TERM_TYPES
        .get(*current)
        .copied()
        .unwrap_or("?")
        .to_string();
    egui::ComboBox::from_id_salt("sess_term")
        .selected_text(text)
        .width(FIELD_W)
        .show_ui(ui, |ui| {
            for (i, t) in term_core::config::TERM_TYPES.iter().enumerate() {
                if ui.selectable_label(*current == i, *t).clicked() {
                    *current = i;
                }
            }
        });
}

/// 认证方式在勾选块里的文案。逐字取自视觉基线
/// `03-ssh-authentication.png`（`Attempt ssh-agent authentication` 等五条），
/// 不用 `AuthMethod::to_string()`——那是日志与配置里的短名（`Password`、
/// `GSSAPI with MIC`），放进界面既不是参考实现措辞也读不出「尝试」的含义。
fn attempt_label(m: AuthMethod) -> &'static str {
    match m {
        AuthMethod::Agent => "Attempt ssh-agent authentication",
        AuthMethod::Password => "Attempt password authentication",
        AuthMethod::PublicKey => "Attempt public-key authentication",
        AuthMethod::KeyboardInteractive => "Attempt keyboard-interactive authentication",
        AuthMethod::GssapiWithMic => "Attempt gssapi-with-mic authentication",
    }
}

impl TerminalWindow {
    /// 内容页分派入口。下标与显示名的唯一来源是 `session_nav::PAGES`。
    pub(in crate::ui) fn draw_session_form(&mut self, ui: &mut egui::Ui) {
        // 视觉基线所有字段/下拉/开关的高度都是 23（行距 29）。
        ui.spacing_mut().interact_size.y = ROW_H;
        // 视觉基线里输入框、下拉框、数值框的底**都是** `#1C1C1C`（与弹窗底同色，
        // 只靠 1pt `#444444` 边框区分）。egui 的 `ComboBox` / `DragValue` 是非编辑
        // 态的 Button，取 `widgets.inactive.weak_bg_fill` = 按钮色 `#262626`，
        // 于是同一列里「文本框更暗、下拉框更亮」，比原图多出一档灰。
        // 只改本表单子树，不动全局（工具栏与菜单的按钮仍是按钮色）。
        let s = &term_core::theme::CHROME_SCALE;
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.weak_bg_fill = crate::theme::chrome(s.field);
        w.hovered.weak_bg_fill = crate::theme::chrome(s.hover);
        w.active.weak_bg_fill = crate::theme::chrome(s.field);
        w.open.weak_bg_fill = crate::theme::chrome(s.field);
        let page = self.new_session.selected_page;
        match page {
            0 => self.session_page(ui),
            4 => self.connection_page(ui),
            6 => self.authentication_page(ui),
            12 => self.tunnels_page(ui),
            other => super::session_pages::draw_page(self, ui, other),
        }
    }

    /// 0 `Session`：WindTerm 的集大成第一页——连接字段（Host/Port · Serial Port ·
    /// Shell 命令，按协议不同）+ OneKey / Label / Group + 终端默认（Term/System/
    /// Charset）+ Description + Sftp/Sysmon 开关。`Protocol` 是顶层标签，不在页内重复。
    /// 0 `Session`：按对话框类型分两套字段集。
    ///
    /// - `New` / `Edit`：完整字段（协议条件首行 + OneKey + Label + Group +
    ///   Term + System + Charset + Description + 协议条件 Sftp/Sysmon 行）。
    /// - `Default`：默认会话是**模板**，按 WindTerm
    ///   `default-session-*-session.png` 视觉基线只有 `Term` / `Charset` /
    ///   `Description` / `Sysmon` 四项——没有 Host/Port/Label/Group/System/Sftp
    ///   （这些是具体连接目标，模板不该有；Sftp 在默认会话里是左侧独立分类，
    ///   不是会话页字段）。
    fn session_page(&mut self, ui: &mut egui::Ui) {
        let is_default = matches!(self.session_dialog, SessionDialogKind::Default);
        egui::Grid::new("new_session_session_grid")
            .num_columns(2)
            // 横向 0：标签列 93 之后紧接字段，视觉基线标签右缘 318.5 → 字段左缘
            // 321（`form_row` 自己右缩 2，所以列宽 93 正好落到 321）。
            // 纵向 6.5：23 高 + 6.5 = 29.5 行距。
            .spacing(egui::vec2(0.0, ROW_GAP))
            .show(ui, |ui| {
                if !is_default {
                    match self.new_session.protocol {
                        Protocol::Shell => {
                            form_row(ui, "Shell:");
                            ui.add_sized(
                                egui::vec2(FIELD_W, ROW_H),
                                TextEdit::singleline(&mut self.new_session.shell_command)
                                    .hint_text("bash"),
                            );
                            ui.end_row();
                        }
                        Protocol::Serial => {
                            form_row(ui, "Serial Port:");
                            // 提示按平台给：Windows 上串口叫 `COM3`，unix 上才叫
                            // `/dev/...`。让 Windows 用户照着 `/dev/tty.usbserial`
                            // 去敲，等于给了一条必然失败的示例。
                            ui.add_sized(
                                egui::vec2(FIELD_W, ROW_H),
                                TextEdit::singleline(&mut self.new_session.host).hint_text(
                                    if cfg!(windows) {
                                        "COM3"
                                    } else {
                                        "/dev/tty.usbserial"
                                    },
                                ),
                            );
                            ui.end_row();
                        }
                        _ => {
                            form_row(ui, "Host:");
                            ui.add_sized(
                                egui::vec2(FIELD_W, ROW_H),
                                TextEdit::singleline(&mut self.new_session.host)
                                    .hint_text("[user@]hostname"),
                            );
                            ui.end_row();
                            form_row(ui, "Port:");
                            // 视觉基线的 Port 是占满字段列的 SpinBox：数值**左对齐**、
                            // 右端两个上下箭头。egui 的 DragValue 在居中的单元格里
                            // 会把字画正中，所以这里包一层左对齐布局，箭头自己补画。
                            let resp = ui
                                .with_layout(
                                    egui::Layout::left_to_right(egui::Align::Min),
                                    |ui| {
                                        ui.add_sized(
                                            egui::vec2(FIELD_W, ROW_H),
                                            DragValue::new(&mut self.new_session.port)
                                                .range(1..=65535),
                                        )
                                    },
                                )
                                .inner;
                            let port_text = self.new_session.port.to_string();
                            spin_box(ui, resp.rect, &port_text, resp.hovered());
                            ui.end_row();
                        }
                    }
                    form_row_accent(ui, "OneKey:");
                    combo_str(ui, "sess_onekey", ONEKEYS, &mut self.new_session.onekey);
                    ui.end_row();
                    // WindTerm 的「Label」即本实现的显示名（内部仍是 `new_session.name`）。
                    form_row(ui, "Label:");
                    ui.add_sized(
                        egui::vec2(FIELD_W, ROW_H),
                        TextEdit::singleline(&mut self.new_session.name),
                    );
                    ui.end_row();
                    form_row(ui, "Group:");
                    ui.add_sized(
                        egui::vec2(FIELD_W, ROW_H),
                        TextEdit::singleline(&mut self.new_session.group)
                            .hint_text("[nesting groups] a>b>c"),
                    );
                    ui.end_row();
                    // 参考实现 `04-new-session/00-ssh-session.png` 的 Session 页
                    // 没有 Logon Script；CRT 那一项不属于 WindTerm 会话表单，
                    // 字段仍留在数据层供脚本侧读取，但不再画进 1:1 页面。
                }
                form_row(ui, "Term:");
                combo_term(ui, &mut self.new_session.term);
                ui.end_row();
                if !is_default {
                    form_row(ui, "System:");
                    combo_str(ui, "sess_system", SYSTEM_TYPES, &mut self.new_session.system);
                    ui.end_row();
                }
                form_row(ui, "Charset:");
                combo_str(ui, "sess_charset", CHARSETS, &mut self.new_session.charset);
                ui.end_row();
                // 视觉基线 Description 框高 95（物理 190）：固定高度而不是
                // desired_rows，因为 egui 的多行正文行高只有 ~14，凑行数
                // 永远压不到 95（4 行只有 61）。
                form_row_top(ui, "Description:", 94.0);
                ui.add_sized(
                    egui::vec2(FIELD_W, 94.0),
                    TextEdit::multiline(&mut self.new_session.description)
                        .desired_width(FIELD_W),
                );
                ui.end_row();
            });
        // Sftp / Sysmon：WindTerm 按协议显隐（`04-new-session/<proto>/00-session` 实测）：
        // SSH → 两者都有（Sftp 默认 ON / Sysmon 默认 OFF）；Shell → 仅 Sysmon；
        // Telnet / Tcp / Serial → 两者都不显示。默认会话只显 Sysmon（视觉基线
        // `default-session-*-session.png` 里没有 Sftp 字段）。
        // 视觉基线 Description 框底到 Sftp 轨道顶是 7pt——Grid 末行自带的那点
        // 行距已经吃掉大半，这里只补 0。
        ui.add_space(0.0);
        // 参考实现这两个是拨动开关，不是复选框（`04-new-session/00-ssh-session.png`）：
        // 标签右对齐到同一列，轨道 31×16，开态 #007ACC。统一走 kit::switch。
        let sftp_on = self.new_session.sftp;
        let sysmon_on = self.new_session.sysmon;
        let mut toggle_sftp = false;
        let mut toggle_sysmon = false;
        if is_default {
            toggle_sysmon = trigger_switch(
                ui,
                &SwitchProps::labeled("sess_sysmon", "Sysmon:", LABEL_W, sysmon_on),
            );
        } else {
            match self.new_session.protocol {
                Protocol::Ssh => {
                    toggle_sftp = trigger_switch(
                        ui,
                        &SwitchProps::labeled("sess_sftp", "Sftp:", LABEL_W, sftp_on),
                    );
                    toggle_sysmon = trigger_switch(
                        ui,
                        &SwitchProps::labeled("sess_sysmon", "Sysmon:", LABEL_W, sysmon_on),
                    );
                }
                Protocol::Shell => {
                    toggle_sysmon = trigger_switch(
                        ui,
                        &SwitchProps::labeled("sess_sysmon", "Sysmon:", LABEL_W, sysmon_on),
                    );
                }
                _ => {}
            }
        }
        if toggle_sftp {
            self.new_session.sftp = !sftp_on;
        }
        if toggle_sysmon {
            self.new_session.sysmon = !sysmon_on;
        }
    }

    /// 4 `Connection`：纯 TCP 选项（与 WindTerm 一致）。Host/Port 已上移到
    /// Session 页；`Built-in SSH client` 移到 Authentication 页（SSH 专属）。
    ///
    /// 整页按视觉基线 `02-session-dialog/ssh/01-ssh-connection.png` 重排：参考实现
    /// **不是两列网格**，而是「两句说明 + 两个勾选 + 两个占满整行的数值框」。
    /// 量得：说明行 45.5、勾选 71 / 101、标题 127.5 / 180，数值框 146 / 198
    /// （高 23、宽 402），标题与框之间 4.5。
    fn connection_page(&mut self, ui: &mut egui::Ui) {
        ui.label("TCP connection options:");
        ui.add_space(5.0);
        ui.checkbox(
            &mut self.new_session.tcp_low_delay,
            "Disable Nagle's algorithm (TCP_NODELAY option)",
        );
        ui.add_space(4.5);
        ui.checkbox(
            &mut self.new_session.tcp_keep_alive,
            "Enable TCP keepalives (SO_KEEPALIVE option)",
        );
        ui.add_space(6.0);
        heading(ui, "Sends null packet to keep session alive:");
        full_spin(ui, &mut self.new_session.keep_alive, seconds_text);
        ui.add_space(6.0);
        heading(ui, "Maximum connection timeout:");
        full_spin(ui, &mut self.new_session.connection_timeout, seconds_text);
    }

    /// 6 `Authentication`：认证方式（勾选即回退链）+ 凭据。
    ///
    /// 视觉基线 `02-session-dialog/ssh/03-ssh-authentication.png` 的勾选块原样
    /// 照抄（标题 `Automatic authentication methods:` + 五条 `Attempt …`，
    /// 顺序 ssh-agent / password / public-key / keyboard-interactive /
    /// gssapi-with-mic）。用户名与密码是本实现多出来的两项——参考实现把这两样
    /// 推迟到连接时的密码提示里问，而我们要在「保存即连接」的一次点击里
    /// 就把凭据备齐；它们按同一套「标题在上」的语法接在勾选块后面。
    fn authentication_page(&mut self, ui: &mut egui::Ui) {
        if self.new_session.protocol != Protocol::Ssh {
            ui.label("Authentication is only applicable to SSH sessions.");
            return;
        }
        ui.label("Automatic authentication methods:");
        // D2：多勾选 = OneKey 回退链。勾选里排第一的是首选（`auth`），其余进
        // `auth_chain`；连接时按这个顺序逐个尝试（例如密码被拒后自动改试公钥）。
        // 至少保留一个——取消最后一个不生效，否则这条会话没有任何认证方式。
        let all = [
            AuthMethod::Password,
            AuthMethod::PublicKey,
            AuthMethod::Agent,
            AuthMethod::GssapiWithMic,
            AuthMethod::KeyboardInteractive,
        ];
        // 用「首选 + 回退链」重建当前勾选集，并按 `all` 的显示顺序排列：勾选
        // 顺序不会漂移成每次重开表单都换个次序。
        let mut selected: Vec<AuthMethod> = all
            .iter()
            .copied()
            .filter(|a| *a == self.new_session.auth || self.new_session.auth_chain.contains(a))
            .collect();
        // 勾选块的**显示顺序**照视觉基线，与 `all` 的链路顺序无关：链路只看
        // `selected` 的排序（仍由 `all` 决定），这里只决定哪一行画在哪。
        const SHOWN: [AuthMethod; 5] = [
            AuthMethod::Agent,
            AuthMethod::Password,
            AuthMethod::PublicKey,
            AuthMethod::KeyboardInteractive,
            AuthMethod::GssapiWithMic,
        ];
        ui.add_space(5.5);
        for (i, auth) in SHOWN.iter().copied().enumerate() {
            if i > 0 {
                ui.add_space(5.5);
            }
            let mut on = selected.contains(&auth);
            if ui.checkbox(&mut on, attempt_label(auth)).changed() {
                if on {
                    if !selected.contains(&auth) {
                        selected.push(auth);
                    }
                } else {
                    selected.retain(|m| *m != auth);
                    // 不许一个都不剩。
                    if selected.is_empty() {
                        selected.push(auth);
                    }
                }
            }
        }
        if selected.is_empty() {
            // 理论上到不了这里；兜底保证会话永远有一种认证方式。
            selected.push(AuthMethod::Password);
        }
        // 归一：首选尽量保持不变（仍被勾选就继续当首选），否则取第一个勾选项；
        // 其余进回退链。
        let primary = if selected.contains(&self.new_session.auth) {
            self.new_session.auth
        } else {
            selected[0]
        };
        self.new_session.auth = primary;
        self.new_session.auth_chain = selected
            .into_iter()
            .filter(|a| *a != primary)
            .collect();
        // 只要勾选里含密码 / 键盘交互，就展示密码框（两者共用同一个密码字段）。
        let needs_password = matches!(
            self.new_session.auth,
            AuthMethod::Password | AuthMethod::KeyboardInteractive
        ) || self
            .new_session
            .auth_chain
            .iter()
            .any(|m| matches!(m, AuthMethod::Password | AuthMethod::KeyboardInteractive));
        ui.add_space(8.0);
        heading(ui, "Username:");
        ui.add_sized(
            egui::vec2(FORM_W, ROW_H),
            TextEdit::singleline(&mut self.new_session.username)
                .hint_text("root")
                .desired_width(FORM_W),
        );
        if needs_password {
            ui.add_space(6.0);
            heading(ui, "Password:");
            ui.add_sized(
                egui::vec2(FORM_W, ROW_H),
                TextEdit::singleline(&mut self.new_session.password)
                    .password(true)
                    .desired_width(FORM_W),
            );
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    "If you tick “Save in vault” in the password prompt, the password \
                     goes into the credentials vault and this session only keeps an \
                     entry id.",
                )
                .wrap(),
            );
        }
        // R13-b2：凭据保险库的解锁 / 创建。放在这里而不是弹窗，
        // 是因为用完密码就要连，嵌套两个模态窗口最容易「关不掉」。
        self.draw_vault_section(ui);
    }

    /// 12 `Tunnels`：转发规则。原先同样挤在 Connection 页底部。
    fn tunnels_page(&mut self, ui: &mut egui::Ui) {
        if self.new_session.protocol != Protocol::Ssh {
            ui.label("Tunnels need an SSH session.");
            return;
        }
        ui.label("Forwarding rules (one per line, started when the session connects):");
        ui.add(
            TextEdit::multiline(&mut self.new_session.forwards)
                .desired_rows(6)
                .hint_text("L 8080:db.internal:5432\nR 9000:localhost:3000\nD 1080"),
        );
        ui.label(
            "L = listen here, connect from the server; R = listen on the server, \
             connect from here; D = a SOCKS5 proxy on this machine. \
             The bind address defaults to 127.0.0.1.",
        );
        if let Some(error) = &self.new_session.forward_error {
            ui.colored_label(egui::Color32::from_rgb(220, 90, 90), error);
        }
    }
}
