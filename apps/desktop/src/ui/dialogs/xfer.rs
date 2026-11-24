//! 文件传输的输入弹层：发要本地文件路径，收要目标目录。
//!
//! # 为什么是弹层而不是原生文件对话框
//!
//! 原生 `FileDialog` 每个平台一套依赖，而且没法出基线图——窗口的像素由系统决定。
//! 这一层只要一段路径文本，`TextEdit` 就够；路径**是否存在**交给
//! [`XferHandle::start`](crate::xfer_run::XferHandle::start) 当场判（它本来就要
//! 读文件 / 看目录），UI 层不读盘：投影层做 IO 是一条要守的边，不是风格问题。
//!
//! # 安全承诺（Receive 侧）
//!
//! 对端在 YMODEM 头块里给的文件名**只取最后一段**，不会写出这个目录。
//! 这句话必须写在弹层上：用户看到的是一个目录输入框，他有权知道
//! 「对端能不能借这个名字跳到别处」——答案是不能。驱动侧的路径清洗在
//! `xfer_run::resolve_name`，这里只是把它说出来。
//!
//! # 状态归属
//!
//! [`XferPrompt`] 挂在 `TerminalWindow` 上（与 `connector` 同类）：它是
//! 「一次待填写的输入」，不进 `ViewState`——`ViewState` 里没有 `String` 输入框
//! 的位置，也不该有（那会让 reducer 认识键盘）。

use crate::ui::kit::window::{DialogId, DialogWindow};

/// 文件传输提示里的两个动作按钮：沿用裸 `ui.button` 的尺寸，
/// 绘制收在 `kit::footer`（与 SSH 流程那几个同源）。
fn action_button(ui: &mut egui::Ui, label: &str) -> bool {
    use crate::ui::kit::spec::FooterButton;
    let spec = FooterButton::Plain
        .spec()
        .with_size(crate::ui::kit::footer::text_button_size(ui, label));
    crate::ui::kit::footer::widget_button(ui, label, spec, true).clicked()
}
use crate::ui::*;
use std::path::PathBuf;
use term_app::action::{XferCmd, XferProtocol};

/// 这次是发还是收。决定标题、说明与输入框的语义（文件 vs 目录）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum XferDirection {
    /// 本端发送：输入是**本地文件**路径。
    Send,
    /// 本端接收：输入是**目标目录**，必须已存在。
    Receive,
}

/// 弹层本体（跨帧）。字段都是控件状态，不是领域状态。
pub(in crate::ui) struct XferPrompt {
    pub direction: XferDirection,
    /// 协议。存枚举而不是显示字符串：确认时要原样交给 `XferHandle::start_with_policy`，
    /// 存字符串就得再翻回来一次（那是一次没有收益的往返）。
    /// 显示名由 [`protocol_label`] 现算。
    pub protocol: XferProtocol,
    /// 路径输入缓冲（跨帧：输入法的预编辑文本要活过一帧）。
    pub path: String,
    /// 接收完成后是否用系统默认程序打开目标目录（X/Y/ZMODEM 文档都有此条）。
    pub open_after: bool,
    /// 接收侧同名冲突策略（官网 page/14 七档，见 `xfer_conflict::POLICIES`）。发送侧不用，
    /// 但同一张弹层结构保留这一格，确认时原样交给 `start_with_policy`。
    pub policy: crate::xfer_conflict::ConflictPolicy,
    /// 本地校验 / `start` 失败的原因。留着让用户改，弹层不关。
    pub error: Option<String>,
}

/// 一帧的结果。调用方据此决定「继续画 / 关掉 / 真的开始」。
#[derive(Debug, PartialEq, Eq)]
pub(in crate::ui) enum Outcome {
    /// 什么都没发生，继续画。
    Open,
    /// 用户取消了（Cancel / Esc / 关窗）。
    Cancelled,
    /// 用户确认了，带着这段路径去 `XferHandle::start`。
    Start(PathBuf),
}

/// 全部协议，按菜单 `Transfer Binary` 里的出现顺序（X → Y → Z）。
///
/// 这不是「第二份清单」：菜单里的命令 ID 仍由 `term-app::menu` 拥有。这张表只
/// 回答「弹层该按什么顺序列协议」，且它现在是 `every_protocol_has_a_distinct_label`
/// 的遍历源——**加了变体却忘了给它一个名字**会当场红，而不是在弹层上显示一片
/// 空白。`Xmodem1k` 紧跟它的母变体（菜单里没有独立命令，它是 XMODEM 的块长选项）。
pub(in crate::ui) const PROTOCOLS: [XferProtocol; 4] = [
    XferProtocol::Xmodem,
    XferProtocol::Xmodem1k,
    XferProtocol::Ymodem,
    XferProtocol::Zmodem,
];

/// 协议的可读名。写全大写是有意的：终端用户认的是 `XMODEM` 这个写法。
pub(in crate::ui) fn protocol_label(protocol: XferProtocol) -> &'static str {
    match protocol {
        XferProtocol::Xmodem => "XMODEM",
        XferProtocol::Xmodem1k => "XMODEM-1K",
        XferProtocol::Ymodem => "YMODEM",
        XferProtocol::Zmodem => "ZMODEM",
    }
}

impl XferPrompt {
    /// `Send` / `Receive` 各开一张空弹层；`Cancel` 没有东西可填，返回 `None`。
    ///
    /// `Cancel` 也在这里出现（而不是只靠调用点的上一臂）是为了让这个函数
    /// 对 `XferCmd` 是**全函数**：将来谁把它接到别处，也不会漏掉一个变体。
    pub(in crate::ui) fn from(cmd: XferCmd) -> Option<Self> {
        let (direction, protocol) = match cmd {
            XferCmd::Send(p) => (XferDirection::Send, p),
            XferCmd::Receive(p) => (XferDirection::Receive, p),
            XferCmd::Cancel => return None,
        };
        Some(Self {
            direction,
            protocol,
            path: String::new(),
            open_after: false,
            policy: Default::default(),
            error: None,
        })
    }

    /// 窗口标题。
    fn title(&self) -> String {
        let protocol = protocol_label(self.protocol);
        match self.direction {
            XferDirection::Send => format!("Send file — {protocol}"),
            XferDirection::Receive => format!("Receive files into — {protocol}"),
        }
    }

    /// 输入框上方的说明（一行主轴 + 可选的一行安全承诺）。
    fn hint_text(&self) -> (&'static str, &'static str) {
        match self.direction {
            XferDirection::Send => (
                "Local file path. It is read when the transfer starts.",
                "/path/to/file  |  /path/to/dir  |  a.bin;b.bin (ZMODEM batch)",
            ),
            XferDirection::Receive => (
                "Target directory. It must already exist.",
                "/path/to/directory",
            ),
        }
    }

    /// ZMODEM 发送支持多文件批处理：把「怎么写」摆在输入框上方，别让用户猜。
    ///
    /// X/YMODEM 没有这个能力，所以只有「Send + ZMODEM」才出现这一行；接收侧
    /// 的目标目录本来就是单个路径，也不该提示。
    fn batch_hint(&self) -> Option<&'static str> {
        (self.direction == XferDirection::Send && self.protocol == XferProtocol::Zmodem)
            .then_some("ZMODEM batch: a directory, or paths separated by ';'.")
    }
}

// ── 视觉常量（逻辑点）────────────────────────────────────────────────
//
// 位置钉在视觉基线主窗口（1280×760）的正中，与 `dialogs::confirm` 同一条规矩：
// 夹具出图与交互路径共用同一个矩形，逐字节可复现。

/// 弹层宽。够放下 40 个字符的路径提示，又不至于挡住整个终端。
// 几何已进 `DialogId::XferPrompt`（kit::spec），这里是它的实测出处备注。
const WIN_W: f32 = 460.0;
/// 弹层高：说明一行 + 输入框 + 承诺行 + 错误行 + 按钮行。
const WIN_H: f32 = 168.0;

impl TerminalWindow {
    /// 画弹层。`None` 时一行都不画（空闲路径零像素变化）。
    pub(in crate::ui) fn draw_xfer_prompt(&mut self, ctx: &egui::Context) {
        if self.xfer_prompt.is_none() {
            return;
        }
        // 借用只活到 `draw` 返回：结果先落地成值，再改 `self`。
        let outcome = {
            let Some(prompt) = self.xfer_prompt.as_mut() else {
                return;
            };
            draw_prompt(ctx, prompt)
        };
        match outcome {
            Outcome::Open => {}
            Outcome::Cancelled => self.xfer_prompt = None,
            Outcome::Start(path) => self.start_xfer(path),
        }
    }
}

/// 弹层本体的绘制。独立函数（不借 `self`）：弹层只认自己那四个字段，
/// 翻译、会话、句柄都不在它的视野里。
pub(in crate::ui) fn draw_prompt(ctx: &egui::Context, prompt: &mut XferPrompt) -> Outcome {
    let title = prompt.title();
    let (line, placeholder) = prompt.hint_text();
    let batch_hint = prompt.batch_hint();
    let receive = prompt.direction == XferDirection::Receive;
    let mut outcome = Outcome::Open;

    let closed = DialogWindow::new(DialogId::XferPrompt, &title).show(
        ctx,
        |ui| {
            ui.label(line);
            if let Some(hint) = batch_hint {
                // 多文件写法只对 ZMODEM 发送成立，常驻会让 X/Y 的用户以为能用。
                ui.weak(hint);
            }
            if receive {
                // 安全承诺：这句不能省，见文件头。
                ui.weak("File names from the peer are reduced to their last component;");
                ui.weak("nothing is written outside this directory.");
            }
            ui.add_space(6.0);
            let resp = ui.add(
                egui::TextEdit::singleline(&mut prompt.path)
                    .id(egui::Id::new("xfer_path_input"))
                    .hint_text(placeholder)
                    .desired_width(f32::INFINITY),
            );
            // 首次打开把焦点放进来；已经有控件拿着焦点就不抢（否则点 Cancel 的
            // 那一帧会被拉回输入框）。
            if ui.memory(|m| m.focused().is_none()) {
                resp.request_focus();
            }
            let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if receive {
                ui.checkbox(&mut prompt.open_after, "Open download folder when done");
                // 同名冲突策略：七档与 `xfer_conflict.rs` 共用一个清单，确认时
                // 原样交给 `XferHandle::start_with_policy`。
                egui::ComboBox::from_id_salt("xfer_conflict_policy")
                    .selected_text(crate::xfer_conflict::label(prompt.policy))
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (policy, text) in crate::xfer_conflict::POLICIES {
                            if ui.selectable_label(prompt.policy == policy, text).clicked() {
                                prompt.policy = policy;
                            }
                        }
                    });
            }

            ui.add_space(4.0);
            // 错误行常驻占位：没有错误时也占一行，否则出错误那一刻整窗会跳。
            match &prompt.error {
                Some(err) => {
                    ui.add(egui::Label::new(
                        egui::RichText::new(err).color(ui.visuals().error_fg_color),
                    ));
                }
                None => {
                    ui.label(" ");
                }
            }

            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if action_button(ui, "Cancel") {
                    outcome = Outcome::Cancelled;
                }
                if action_button(ui, "Start") || entered {
                    outcome = confirm(prompt);
                }
            });
            if DialogId::XferPrompt.keys().poll(ui.ctx()).cancel {
                outcome = Outcome::Cancelled;
            }
        },
    );

    // 标题条右上角的 ×：与 Cancel 同一条路（不是「确认」）。
    if !closed {
        return Outcome::Cancelled;
    }
    outcome
}

/// 确认：只校验「非空」。
///
/// 路径存在性**不在这里查**——`XferHandle::start` 会给出更准的原因
/// （「接收目录 X 不可用」「读不到要发送的文件 X」），重复查一遍只会
/// 让两条错误文案随时间漂开。
fn confirm(prompt: &mut XferPrompt) -> Outcome {
    let path = prompt.path.trim();
    if path.is_empty() {
        prompt.error = Some(match prompt.direction {
            XferDirection::Send => "Enter the file to send.".to_string(),
            XferDirection::Receive => "Enter the directory to receive into.".to_string(),
        });
        return Outcome::Open;
    }
    prompt.error = None;
    Outcome::Start(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_and_receive_open_different_prompts_and_cancel_opens_none() {
        let send = XferPrompt::from(XferCmd::Send(XferProtocol::Xmodem)).unwrap();
        assert_eq!(send.direction, XferDirection::Send);
        assert_eq!(protocol_label(send.protocol), "XMODEM");
        assert!(send.path.is_empty());

        let receive = XferPrompt::from(XferCmd::Receive(XferProtocol::Ymodem)).unwrap();
        assert_eq!(receive.direction, XferDirection::Receive);
        assert_eq!(protocol_label(receive.protocol), "YMODEM");

        // 取消没有东西可填：调用点的上一臂会直接 cancel，不该开空弹层。
        assert!(XferPrompt::from(XferCmd::Cancel).is_none());

        // 冲突策略出厂 Overwrite（与内核默认一致），七档都能通过下拉选到。
        assert_eq!(receive.policy, crate::xfer_conflict::ConflictPolicy::Overwrite);
        assert_eq!(receive.policy, Default::default());
        assert_eq!(crate::xfer_conflict::POLICIES.len(), 7);
        // 下拉的七档顺序照官网 page/14：Append / Resume 夹在 newer-and-longer 与 Rename 之间。
        let labels: Vec<_> = crate::xfer_conflict::POLICIES
            .iter()
            .map(|(_, text)| *text)
            .collect();
        assert_eq!(
            labels,
            [
                "Overwrite",
                "Only if newer",
                "Only if newer and longer",
                "Append",
                "Resume",
                "Rename",
                "Skip"
            ]
        );
    }

    #[test]
    fn only_a_non_empty_path_starts_the_transfer() {
        let mut prompt = XferPrompt::from(XferCmd::Send(XferProtocol::Xmodem)).unwrap();
        // 空 / 全空白都拦下来，且弹层留在原地（`Open` 而不是 `Cancelled`）。
        assert_eq!(confirm(&mut prompt), Outcome::Open);
        assert!(prompt.error.is_some());
        prompt.path = "   ".to_string();
        assert_eq!(confirm(&mut prompt), Outcome::Open);

        // 确认后把上一次的错误清掉：留着会让用户以为新路径也错。
        prompt.path = "  /tmp/report.pdf  ".to_string();
        assert_eq!(
            confirm(&mut prompt),
            Outcome::Start(PathBuf::from("/tmp/report.pdf")),
            "首尾空白要去掉"
        );
        assert!(prompt.error.is_none());
    }

    #[test]
    fn every_protocol_has_a_distinct_label() {
        // 遍历 [`PROTOCOLS`] 而不是手写四条：加了变体忘写名字，或两条撞名，
        // 都会在这里炸——比在弹层标题上显示一片空白强。
        let mut seen = std::collections::BTreeSet::new();
        for protocol in PROTOCOLS {
            let label = protocol_label(protocol);
            assert!(!label.is_empty(), "{protocol:?} 没有可读名");
            assert!(label.to_uppercase() == label, "{label} 应当全大写");
            assert!(seen.insert(label), "{label} 与另一个协议撞名");
        }
        assert_eq!(protocol_label(XferProtocol::Zmodem), "ZMODEM");
        assert_eq!(PROTOCOLS.len(), seen.len());
    }

    #[test]
    fn only_the_zmodem_send_prompt_documents_the_batch_syntax() {
        // 写法必须「可发现」：输入框的 hint_text 里给出分隔符，弹层上还有一行
        // 说明。两者都只该出现在 ZMODEM 发送——X/Y 与接收侧不支持批处理。
        let prompt = XferPrompt::from(XferCmd::Send(XferProtocol::Zmodem)).unwrap();
        let (_, placeholder) = prompt.hint_text();
        assert!(placeholder.contains(';'), "hint_text 里要给出多路径分隔符");
        assert!(
            placeholder.contains("ZMODEM") && placeholder.contains("batch"),
            "hint_text 要说明这是 ZMODEM 批处理：{placeholder}"
        );
        let hint = prompt.batch_hint().expect("ZMODEM 发送应有批处理提示");
        assert!(hint.contains(';') && hint.contains("directory"), "{hint}");

        for protocol in [
            XferProtocol::Xmodem,
            XferProtocol::Xmodem1k,
            XferProtocol::Ymodem,
        ] {
            assert!(
                XferPrompt::from(XferCmd::Send(protocol))
                    .unwrap()
                    .batch_hint()
                    .is_none(),
                "{protocol:?} 发送不该显示批处理提示"
            );
        }
        assert!(
            XferPrompt::from(XferCmd::Receive(XferProtocol::Zmodem))
                .unwrap()
                .batch_hint()
                .is_none(),
            "接收侧不该显示批处理提示"
        );
    }
}
