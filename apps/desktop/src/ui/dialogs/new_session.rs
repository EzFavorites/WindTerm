//! New Session（兼 Session Properties）对话框：协议条 + 二级树 + 内容页 + 保存/连接。
//!
//! R9 从 `ui/mod.rs` 拆出；R13-d2 起这张表单同时当「会话属性」用
//! （见 `open_session_properties`）。表单与配置的双向映射在
//! `dialogs/session_form.rs`（纯函数，带自己的单测），页面内容在
//! `dialogs/session_pages.rs`。
//!
//! **窗口外框不在这个文件里**：尺寸 / 淡入 / 右上角 × / sizing-pass 热身
//! 全部走 `ui/harness.rs::StableWindow`。本文件只画内容（`draw_new_session_body`），
//! 所以同一份内容既能放进整窗的弹窗位置，也能被组件夹具单独出图。

use crate::ui::kit::window::{DialogId, DialogWindow};
use crate::ui::panels::tunnels;
use crate::ui::*;

/// 会话弹窗的四种调用方。每个入口参数不同、渲染不同：
///
/// - `New`：新建会话。对话框标题 `New Session`、主按钮 `Connect`，
///   保存时按名字新增 / 覆盖 `sessions` 并连接。
/// - `Edit { index }`：编辑已存在会话。标题 `Session Properties`、按钮
///   `Save & Connect`，保存时覆盖 `sessions[index]` 并连接。
/// - `Properties { index }`：属性面板。与 `Edit` 一样预填并写回**同一条**记录，
///   但保存后**不发起连接**（地址栏 / 右键的「属性」只是改参数）。
/// - `Default`：编辑默认会话模板（新增会话的默认配置）。标题
///   `Edit Default Session`、按钮 `Save`，保存时只写回 `default_session`、
///   **不进会话列表、不连接**。
///
/// 用一个枚举取代原先 `editing_session: Option<(usize, String)>` + `edit_target()`
/// 的布尔式判断：四个入口各带自己的参数（要编辑哪条、预置分组、是否连接），
/// 渲染时只读 `kind` 即可分派标题 / 按钮 / 保存语义，不用再散落 `if editing`。
#[derive(Clone)]
pub(in crate::ui) enum SessionDialogKind {
    New { group: Option<String> },
    Edit { index: usize },
    /// 属性面板：与 `Edit` 一样预填并写回同一条记录，但**保存后不发起连接**。
    ///
    /// 与 `Edit` 分开而不是给它加个 `reconnect: bool`：`Edit` 的调用方是
    /// 「会话设置…」菜单与侧边栏右键 `Properties`，语义是「改完就重连」；
    /// 而 D3 的地址栏 / 右键属性面板只是「改参数」。两者字段、预填、写回完全
    /// 相同，唯一的差别就是收尾那一步——所以差别放在这里、由
    /// `should_connect()` 一处回答，而不是让调用方各自记住「保存后别连」。
    Properties { index: usize },
    Default,
}

impl SessionDialogKind {
    /// 窗口标题（StableWindow 的 id 固定为 `new_session`，标题随 kind 变，
    /// 窗口状态不跟着变）。
    fn title(&self) -> &'static str {
        match self {
            SessionDialogKind::New { .. } => "New Session",
            // 属性面板与 `Edit` 同标题：用户看到的都是「这条会话的属性」，
            // 差别只在保存后连不连——那是收尾行为，不该体现在窗口标题上。
            SessionDialogKind::Edit { .. } | SessionDialogKind::Properties { .. } => {
                "Session Properties"
            }
            SessionDialogKind::Default => "Edit Default Session",
        }
    }

    /// 右下角主按钮文案。带连接的模式（New / Edit）把「连接」写在按钮上，
    /// 不带连接的（Default / Properties）只写 `Save`——沿用文件里既有的动词，
    /// 不另造 `Apply` 之类的词（这里的按钮按下去就关窗落盘，不是「留着窗生效」）。
    fn primary_button(&self) -> &'static str {
        match self {
            SessionDialogKind::New { .. } => "Connect",
            SessionDialogKind::Edit { .. } => "Save & Connect",
            SessionDialogKind::Properties { .. } | SessionDialogKind::Default => "Save",
        }
    }

    /// 保存之后要不要发起连接。
    ///
    /// `New` / `Edit` 为真（老行为逐字节不变：新建即连、改完重连），
    /// `Properties` / `Default` 为假。属性面板改的只是参数，用户没要求重连——
    /// 把正在跑的会话踢掉再连一遍是副作用，不是功能。
    ///
    /// 这条判据此前是死代码（`save_session_dialog` 尾部无条件连接），D3 起是
    /// 那一段唯一的开关。
    fn should_connect(&self) -> bool {
        // 白名单，刻意不写成 `!matches!(self, Default)`：后者会让后加的变体
        // 默认继承「连接」，每加一个模式都得记得回来改这一行。白名单则是
        // 加变体时编译器不会提醒、但语义上的默认答案是「不连」，出错更显眼。
        matches!(
            self,
            SessionDialogKind::New { .. } | SessionDialogKind::Edit { .. }
        )
    }

    /// 正在编辑的已存在会话下标（`None` = 新建 / 默认模板）。
    ///
    /// `Properties` 也算「编辑已存在会话」：它保存时同样写回 `sessions[index]`，
    /// 所以这里返回 `Some(index)`，`save_session_dialog` 的写回分支不用另写一套。
    fn edit_index(&self) -> Option<usize> {
        match self {
            SessionDialogKind::Edit { index } | SessionDialogKind::Properties { index } => {
                Some(*index)
            }
            _ => None,
        }
    }
}

/// 两栏内容区高度。视觉基线 `default-session-ssh-session.png` / `04-new-session/
/// 00-ssh-session.png` 量得内容总高 570pt，扣掉协议条 24、页脚上留白 19、
/// 按钮 23、下留白 10，两栏区就是 494。
///
/// 行的节奏全部按视觉基线实测：树行 24pt（20 行 = 480，加顶部 10pt 内边距正好
/// 494 不滚动）、右栏首行距协议条 18pt、字段 23pt 高 / 29pt 行距。
const BODY_H: f32 = 494.0;

/// 协议条高度。视觉基线 48 物理像素 = 24 逻辑点，且**通栏**（tab 从弹窗左缘
/// 铺到右缘，没有左右内边距）。
const STRIP_H: f32 = 24.0;

/// 左栏宽度。视觉基线分隔线在物理 x=435（640 宽的弹窗里 = 逻辑 217.5）。
const NAV_W: f32 = 218.0;

/// 弹窗内容宽。视觉基线两张会话图都是 640。
const CONTENT_W: f32 = 640.0;

/// 右栏内容的左边距与宽度。视觉基线 `02-session-dialog/ssh/01-ssh-connection.png`
/// 量得整行控件左缘 x=228（分隔线 218 再留 10）、右缘 630（距弹窗右 10）
/// ⇒ 内容宽 402。Session 页的「标签列 93 + 字段 309」正好也是 402。
const FORM_PAD_L: f32 = 10.0;
const FORM_W: f32 = 402.0;

/// 页脚按钮高与右边距/底边距（视觉基线 Cancel/Save 实测 23 高、距右 10、距底 10）。
const BTN_H: f32 = 23.0;
const FOOT_PAD: f32 = 10.0;

/// 导航树的叶子：**整行**占宽、整行高亮。
///
/// 视觉基线 `default-session-ssh-session.png` 里选中行是 x=20..429、y=68..115
/// （物理）的一整条 `#282828`：逻辑 x=10..214.5、高 24，无描边、文字仍是
/// `#BBBBBB`。`egui::SelectableLabel` 只按文字宽度画底，还继承
/// `widgets.inactive.bg_stroke` 画一圈边框，于是旧版的选中项看起来像个按钮。
/// 这里自己占满整行再画：底色只铺「左缩进 10、右留 4」，文字位置不动，
/// 选中/悬停都只换底色、绝不描边。
fn nav_leaf(ui: &mut egui::Ui, selected: bool, label: &str, indent: f32) -> egui::Response {
    let s = &term_core::theme::CHROME_SCALE;
    let width = ui.available_width();
    // 24pt：视觉基线 20 行树（4 个折叠头 + 16 个叶子）行距实测 48 物理像素。
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, 24.0), egui::Sense::click());
    let fill = if selected {
        Some(s.selected)
    } else if response.hovered() {
        Some(s.hover)
    } else {
        None
    };
    if let Some(fill) = fill {
        let band = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 10.0, rect.top()),
            egui::pos2(rect.right() - 4.0, rect.bottom()),
        );
        ui.painter()
            .rect_filled(band, egui::Rounding::ZERO, crate::theme::chrome(fill));
    }
    let font = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Body)
        .cloned()
        .unwrap_or_else(|| crate::ui::type_scale::heading());
    ui.painter().text(
        egui::pos2(rect.min.x + indent, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        crate::theme::chrome(s.text),
    );
    response
}

/// 页脚按钮。视觉基线 `default-session-ssh-session.png` 实测：高 23、2pt 圆角、
/// 文字左右各留 15，最窄 74；主按钮（Save / Connect）是 `#3EA0EC` 蓝框 +
/// `#333A42` 蓝灰底，次按钮（Cancel）是 `#3C3C3C` 灰框 + `#262626` 灰底。
fn footer_button(ui: &mut egui::Ui, label: &str, primary: bool, enabled: bool) -> egui::Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let text_w = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, egui::Color32::WHITE)
        .size()
        .x;
    let w = (text_w + 30.0).max(74.0);
    let fill = if primary {
        egui::Color32::from_rgb(0x33, 0x3A, 0x42)
    } else {
        egui::Color32::from_rgb(0x26, 0x26, 0x26)
    };
    let stroke = if primary {
        egui::Color32::from_rgb(0x3E, 0xA0, 0xEC)
    } else {
        egui::Color32::from_rgb(0x3C, 0x3C, 0x3C)
    };
    ui.add_enabled_ui(enabled, |ui| {
        ui.add_sized(
            egui::vec2(w, BTN_H),
            egui::Button::new(label)
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, stroke))
                .rounding(2.0),
        )
    })
    .inner
}

impl TerminalWindow {
    /// 弹窗入口：外框交给 harness，内容交给 `draw_new_session_body`。
    ///
    /// 这里曾经有 25 行手写的「空内容热身」补丁（sizing pass / fade_in /
    /// fixed_rect / ID 冲突四件事混在一起）。P2 起它们全在
    /// `harness::StableWindow::show` 里，只有一份——弹窗皮肤再改时不必
    /// 每处各补一遍，下一个弹窗也不用重新踩那四轮。
    pub(in crate::ui) fn draw_new_session_dialog(&mut self, ctx: &egui::Context) {
        // 标题 / 按钮 / 保存语义全由 `session_dialog` 这一个枚举分派：
        // New → "New Session" / Edit → "Session Properties" / Default →
        // "Edit Default Session"。StableWindow 的 id 固定，标题变窗口状态不变。
        let kind = self.session_dialog.clone();
        // `tight()`：视觉基线的协议条是**通栏**的（tab 从弹窗左缘铺到右缘），
        // 12pt 的内容内边距会让五个 tab 整体缩进、左右各留一条空隙。
        // 页脚按钮的 10pt 边距改由 `draw_new_session_body` 自己给。
        let open = DialogWindow::new(DialogId::NewSession, kind.title())
            .tight()
            .show(ctx, |ui| self.draw_new_session_body(ui, &kind));
        // 右上角 × 与「Cancel」走同一条关闭路径（清编辑目标，防止下一次
        // 「＋ New Session」误改上一条会话）。
        if !open {
            self.close_session_dialog();
        }
    }

    /// 弹窗内容：协议标签条 + 左侧二级树 + 右侧两列表单 + 右下按钮。
    ///
    /// 与窗口外框分开：尺寸 / 淡入 / 右上角 × / sizing-pass 热身都是
    /// `harness::StableWindow` 的事。分开之后同一份内容既能放进整窗的
    /// 弹窗位置，也能被组件夹具单独画出来——这正是 P2 要的「独立渲染」。
    ///
    /// 结构差别见 `session_nav.rs` 的模块头：横排是协议、左侧是折叠树、
    /// 右侧是两列网格、按钮在右下。左侧树的文字逐字来自视觉基线，内容页
    /// 下标与显示名由 `session_nav::PAGES` 唯一提供。
    fn draw_new_session_body(&mut self, ui: &mut egui::Ui, kind: &SessionDialogKind) {
        // 通栏布局：协议条贴弹窗左缘，块与块之间不额外加间距，全部由常量控制。
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        self.draw_protocol_strip(ui);

        ui.horizontal_top(|ui| {
            // 三块严格按视觉基线切分：左栏 218 + 分隔线 1 + 右栏 421 = 640。
            // 用 allocate 钉死宽度，内容页里未限宽的长 label 才不会把整行撑爆。
            ui.spacing_mut().item_spacing.x = 0.0;
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(CONTENT_W, BODY_H),
                egui::Sense::hover(),
            );
            let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            let ui = &mut body_ui;
            ui.set_max_height(BODY_H);
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

            // 左：二级树（树顶距协议条 10pt，与视觉基线选中块 y=34 对齐）。
            ui.allocate_ui_with_layout(
                egui::vec2(NAV_W, BODY_H),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.add_space(10.0);
                    egui::ScrollArea::vertical()
                        .max_height(BODY_H - 10.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.draw_nav_tree(ui));
                },
            );

            // 中：1pt 分隔线（视觉基线 x=217.5，色 #161616，比弹窗底压暗一档）。
            let (sep, _) = ui.allocate_exact_size(
                egui::vec2(1.0, BODY_H),
                egui::Sense::hover(),
            );
            ui.painter()
                .rect_filled(sep, egui::Rounding::ZERO, egui::Color32::from_rgb(0x16, 0x16, 0x16));

            // 右：表单。首行框顶距协议条 18.5pt（视觉基线 Term 框顶 y=42，边框中心在 42.5）。
            // 左缘再让 10pt：视觉基线右栏内容从 x=228 起（分隔线在 217.5），
            // 内容宽 402、右缘 630。
            let right_w = CONTENT_W - NAV_W - 1.0;
            ui.add_space(FORM_PAD_L);
            ui.allocate_ui_with_layout(
                egui::vec2(right_w - FORM_PAD_L, BODY_H),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    // 右栏内的垂直节奏按视觉基线：Sftp / Sysmon 两个开关的轨道
                    // 顶相距 18（轨道 16 + 2.5 缝）。Grid 用自己的 spacing，
                    // 不受这里影响；其它页的 label/checkbox 也只差几分。
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 2.5);
                    egui::ScrollArea::vertical()
                        .max_width(right_w - FORM_PAD_L)
                        .max_height(BODY_H)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(FORM_W);
                            ui.add_space(19.5);
                            self.draw_session_form(ui);
                        });
                },
            );
        });

        // 视觉基线：两栏区底 → 按钮顶 19pt，按钮 23 高、距右/底各 10。
        ui.add_space(19.5);
        ui.horizontal(|ui| {
            // 视觉基线 04-new-session/00-ssh-session.png 页脚左侧还有一颗
            // `Edit Default Settings...`：打开同一份表单的默认模板视图。
            if kind.primary_button() != "Save"
                && footer_button(ui, "Edit Default Settings...", false, true).clicked()
            {
                self.open_default_session_settings();
            }
            ui.spacing_mut().item_spacing.x = 9.0;
            let rest = ui.available_width();
            ui.allocate_ui_with_layout(
                egui::vec2(rest, BTN_H),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    // right_to_left 的第一颗按钮落在最右：视觉基线无论 New
                    // （Cancel + Connect）还是 Default（Cancel + Save），主按钮
                    // 都在最右，Cancel 在它左边。右端 10pt 由先占的空白让出。
                    ui.add_space(FOOT_PAD);
                    // 视觉基线 New Session 在 Host 为空时 `Connect` 是灰禁用的；
                    // Default / Properties 的 `Save` 不依赖连接目标，始终可用。
                    let can_connect = match self.new_session.protocol {
                        Protocol::Shell => !self.new_session.shell_command.trim().is_empty(),
                        _ => !self.new_session.host.trim().is_empty(),
                    };
                    let enabled = !kind.should_connect() || can_connect;
                    let primary = footer_button(ui, kind.primary_button(), true, enabled);
                    if primary.clicked() {
                        self.save_session_dialog();
                    }
                    if footer_button(ui, "Cancel", false, true).clicked() {
                        self.close_session_dialog();
                    }
                },
            );
        });
        ui.add_space(FOOT_PAD);
    }

    /// G2：整行五个等宽协议标签。旧版是 `Protocol:` + 一排裸文字挤在左上，
    /// 既没标签条的形状，也让「协议」看起来像左侧导航的同类项。
    ///
    /// 视觉基线（`default-session-ssh-session.png`）：条高 24、**通栏**铺满
    /// 640pt，五个 tab 各 128 宽、彼此用 1pt `#474747` 竖线分开；未选中底
    /// `#333333`，选中的那个反而是压暗的 `#1C1C1C`（与弹窗底同色，看起来
    /// 像「挖」进了内容区），文字 `#E8E8E8`。
    fn draw_protocol_strip(&mut self, ui: &mut egui::Ui) {
        use super::session_nav::PROTOCOL_TABS;
        let s = &term_core::theme::CHROME_SCALE;
        let to_color = crate::theme::chrome;
        egui::Frame::none()
            .fill(to_color(s.tab))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                ui.horizontal(|ui| {
                    let w = ui.available_width() / PROTOCOL_TABS.len() as f32;
                    for (i, (proto, tab_label)) in PROTOCOL_TABS.iter().enumerate() {
                        let selected = self.new_session.protocol == *proto;
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(w, STRIP_H), egui::Sense::click());
                        let fill = if selected {
                            s.nav
                        } else if response.hovered() {
                            s.hover
                        } else {
                            s.tab
                        };
                        ui.painter()
                            .rect_filled(rect, egui::Rounding::ZERO, to_color(fill));
                        // tab 之间的 1pt 分界线：只在右侧画，最后一个不画。
                        if i + 1 < PROTOCOL_TABS.len() && !selected {
                            ui.painter().vline(
                                rect.right() - 0.5,
                                rect.y_range(),
                                egui::Stroke::new(1.0, egui::Color32::from_rgb(0x47, 0x47, 0x47)),
                            );
                        }
                        let font = ui
                            .style()
                            .text_styles
                            .get(&egui::TextStyle::Button)
                            .cloned()
                            .unwrap_or_else(|| crate::ui::type_scale::heading());
                        let colour = to_color(if selected { s.text_strong } else { s.text });
                        let painter = ui.painter().clone();
                        let galley =
                            painter.layout_no_wrap(tab_label.to_string(), font.clone(), colour);
                        let text_pos = rect.center() - galley.size() / 2.0;
                        painter.galley(text_pos, galley.clone(), colour);
                        // 视觉基线每个 tab 的首字母下都有一条访问键下划线
                        // （y=38 处 5 段 14px 的亮线，正好压在 S/S/T/T/S 下）。
                        let first = tab_label.chars().next().unwrap_or(' ');
                        let first_w = painter
                            .layout_no_wrap(first.to_string(), font, colour)
                            .size()
                            .x;
                        let uy = text_pos.y + galley.size().y + 2.0;
                        painter.line_segment(
                            [
                                egui::pos2(text_pos.x, uy),
                                egui::pos2(text_pos.x + first_w, uy),
                            ],
                            egui::Stroke::new(1.0, colour),
                        );
                        if response.clicked() {
                            self.new_session.protocol = *proto;
                            self.new_session.port = proto.default_port();
                            if !super::session_nav::contains(
                                *proto,
                                self.new_session.selected_page,
                            ) {
                                self.new_session.selected_page =
                                    super::session_nav::default_page(*proto);
                            }
                            if self.new_session.name.starts_with("New ")
                                || self.new_session.name == "Local Shell"
                            {
                                self.new_session.name = match proto {
                                    Protocol::Shell => "Local Shell".to_string(),
                                    other => format!("New {}", other.label()),
                                };
                            }
                        }
                    }
                });
            });
    }

    /// G3：一级设置域 + 可折叠二级页。折叠状态由 `id_salt` 存在 egui Memory，
    /// 不给表单模型加视图字段。
    fn draw_nav_tree(&mut self, ui: &mut egui::Ui) {
        use super::session_nav::{nav_for, NavEntry};
        // 行距按视觉基线：20 行树、每行 24pt、行间零缝隙。
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.spacing_mut().interact_size.y = 24.0;
        // `CollapsingHeader` 用 `spacing().indent` 定位三角与组名
        // (collapsing_header.rs:519)：三角中心在 `left + indent/2`、组名在
        // `left + indent`。视觉基线组名 x≈36（egui 还会再加 3pt 图标间距）⇒
        // indent = 33，此时三角中心落在 16.5（视觉基线 19.75，差 3pt，可接受）。
        // body 用 `show_unindented()`，子项缩进由 `nav_leaf` 的 indent 自己给。
        ui.spacing_mut().indent = 33.0;
        let selected = self.new_session.selected_page;
        let tree = nav_for(self.new_session.protocol);
        for (index, entry) in tree.iter().enumerate() {
            match entry {
                NavEntry::Leaf(leaf) => {
                    // 一级独立项与折叠组名同一条起始线（视觉基线里 Session /
                    // Terminal / Window / SSH 左对齐：都在 x≈33）。
                    if nav_leaf(ui, selected == leaf.page, leaf.label, 33.0).clicked() {
                        self.new_session.selected_page = leaf.page;
                    }
                }
                NavEntry::Group { label, children } => {
                    egui::CollapsingHeader::new(egui::RichText::new(*label).strong())
                        .id_salt(("new_session_nav", index))
                        .default_open(true)
                        .show_unindented(ui, |ui| {
                            for leaf in children.iter() {
                                if nav_leaf(ui, selected == leaf.page, leaf.label, 54.0).clicked()
                                {
                                    self.new_session.selected_page = leaf.page;
                                }
                            }
                        });
                }
            }
        }
    }

    // 右侧表单本体（G4 两列网格）现在在 `session_form_body.rs`
    // （R9 单文件 ≤ 450 行）。`self.draw_session_form(ui)` 是同一个 impl
    // 上的固有方法，无需 import。

    /// 打开「会话属性」：把既有会话反向填进同一个表单。
    ///
    /// 为什么复用 New Session 而不是另做一个对话框：字段完全一样，两份表单
    /// 迟早会不同步。区别只有三处——标题、按钮文案、保存时覆盖原条目，
    /// 现在统一由 `session_dialog` 这一个枚举分派。
    pub(in crate::ui) fn open_session_properties(&mut self, i: usize) {
        let Some(config) = self.sessions.get(i).cloned() else {
            return;
        };
        self.new_session = NewSessionData::from_config(&config);
        self.session_dialog = SessionDialogKind::Edit { index: i };
        self.view.show_new_session = true;
    }

    /// 打开「属性面板」：预填方式与 `open_session_properties` 逐字相同，
    /// 唯一的差别是保存后**不连接**（`SessionDialogKind::Properties`）。
    ///
    /// 为什么单开一个入口而不是给 `open_session_properties` 加参数：那个函数
    /// 已被「会话设置…」菜单（`effects.rs`）与侧边栏右键（`sidebar_view.rs`）
    /// 占了，语义是「改完就重连」，两处调用都依赖它；属性面板要的是「只保存」。
    /// 一个函数两套收尾语义必然要靠调用方记得传参，所以分成两个名字——
    /// 名字本身说明收尾行为，调用点读起来不会误会。
    pub(in crate::ui) fn open_session_properties_only(&mut self, i: usize) {
        let Some(config) = self.sessions.get(i).cloned() else {
            return;
        };
        self.new_session = NewSessionData::from_config(&config);
        self.session_dialog = SessionDialogKind::Properties { index: i };
        self.view.show_new_session = true;
    }

    /// 打开一张新会话表单。**必须走这里**：它把 dialog 重置成 `New`，
    /// 否则在「属性 / 默认会话」之后点「＋ New Session」会把上一条会话改掉。
    ///
    /// 注意：这里**不**用 `default_session` 重填表单——保持与历史行为一致，
    /// 避免改动新建会话的初始表单、连带冲掉会话弹窗的组件基线。把「默认会话
    /// 模板作用于新建」接到 `open_new_session` 是后续一步（需重冻基线）。
    pub(in crate::ui) fn open_new_session(&mut self, group: Option<String>) {
        self.session_dialog = SessionDialogKind::New { group: group.clone() };
        if let Some(group) = group {
            self.new_session.group = group;
        }
        self.view.show_new_session = true;
    }

    /// 打开「默认会话设置」：把默认模板反向填进同一个表单。
    ///
    /// 这是 WindTerm `Session ▸ Preferences ▸ Default Session Settings…` 的落地：
    /// 标题 `Edit Default Session`、按钮 `Save`、保存时只写回 `default_session`、
    /// 不进会话列表、不连接。窗口类型与 New/Edit 完全相同，仅参数与渲染不同。
    pub(in crate::ui) fn open_default_session_settings(&mut self) {
        self.new_session = NewSessionData::from_config(&self.default_session);
        self.new_session.selected_page = 0;
        self.session_dialog = SessionDialogKind::Default;
        self.view.show_new_session = true;
    }

    // 原 Authentication / Tunnel 控件也随表单一起搬到 `session_form_body.rs`。

    fn close_session_dialog(&mut self) {
        self.view.show_new_session = false;
        self.session_dialog = SessionDialogKind::New { group: None };
    }

    /// 保存：行为由 `session_dialog` 分派。
    ///
    /// - `Edit { index }`：覆盖 `sessions[index]` 并连接。
    /// - `Properties { index }`：覆盖 `sessions[index]`，**不连接**。
    /// - `New { .. }`：按名字新增 / 覆盖并连接。
    /// - `Default`：只写回 `default_session`，不进会话列表、不连接。
    fn save_session_dialog(&mut self) {
        // R13-e2：规则先校验——写错的转发不该等到连接时才报，那时对话框已经关了，
        // 用户也没法把光标放回那一行。
        if let Err(message) = tunnels::parse_rules(&self.new_session.forwards) {
            self.new_session.forward_error = Some(message);
            return;
        }
        self.new_session.forward_error = None;
        // 编辑已存在会话时以那条会话为底稿：表单没画的字段（charset、scrollback、
        // 串口位宽）得原样留下，不能被 `SessionConfig::new` 重置成默认值。
        let base = match self.session_dialog.edit_index() {
            Some(i) => self.sessions[i].clone(),
            None => SessionConfig::new(self.new_session.protocol),
        };
        let mut config = self.new_session.to_config(base);
        // W1-b：出网代理选了却填不全时，在关对话框之前拦下。复用表单级的
        // `forward_error` 提示位（W4 逐页比对时再拆成独立字段），避免为了
        // 一个错误位改动 Proxy 页的组件基线。
        if let Some(why) = config.validate_proxy() {
            self.new_session.forward_error = Some(why);
            return;
        }
        // R13-d2：密码只在这一次连接里用，不进 `SessionConfig`
        // （那会写进 user.sessions）。原生路径直接把它交给连接流程。
        let password = match (config.builtin_ssh, config.auth) {
            (true, AuthMethod::Password) if !self.new_session.password.is_empty() => {
                Some(self.new_session.password.clone())
            }
            _ => None,
        };

        // `Default` 模式：写回默认模板、不进会话列表、不连接。
        if let SessionDialogKind::Default = self.session_dialog {
            self.default_session = config;
            self.persist_default_session();
            self.close_session_dialog();
            return;
        }

        let s_idx = match self.session_dialog {
            // `Properties` 与 `Edit` 走**同一段**写回：撞名静默改名、覆盖
            // `sessions[index]`、选中它。两者只差收尾连不连（`should_connect()`），
            // 所以不为 `Properties` 特判——两份写回逻辑迟早会不同步。
            SessionDialogKind::Edit { index } | SessionDialogKind::Properties { index } => {
                // 名字撞上别人会让「按名字认 tab」失效（`tabs` 与
                // `connect_selected_session` 都是按名字对的），
                // 所以像 Duplicate 一样先取一个不撞的。
                let clash = self
                    .sessions
                    .iter()
                    .enumerate()
                    .any(|(j, s)| j != index && s.name == config.name);
                if clash {
                    config.name = unique_name(&self.sessions, &config.name);
                }
                self.sessions[index] = config.clone();
                index
            }
            SessionDialogKind::New { .. } => {
                match self.sessions.iter().position(|s| s.name == config.name) {
                    Some(i) => {
                        self.sessions[i] = config.clone();
                        i
                    }
                    None => {
                        let i = self.sessions.len();
                        self.sessions.push(config.clone());
                        i
                    }
                }
            }
            SessionDialogKind::Default => unreachable!(),
        };
        self.selected_session = s_idx;
        self.save_sessions();
        // `should_connect()` 必须在 `close_session_dialog()` **之前**读：关闭会把
        // `session_dialog` 复位成 `New { group: None }`（下一张新建表单的初值），
        // 那之后读就永远是 true，属性面板会被连上。
        let connect = self.session_dialog.should_connect();
        self.close_session_dialog();
        if !connect {
            return;
        }
        if crate::ui::panels::sidebar::uses_builtin_ssh(&config) {
            self.begin_builtin_ssh(config, password);
        } else {
            self.connect_selected_session();
        }
    }

    /// 把 `default_session` 模板落盘（目前为内存态占位，见 `mod.rs` 字段注释）。
    ///
    /// 接 prefs 持久化时在这里做唯一一处写入；现在先留空，保证 `Default` 模式
    /// 在「不改动既有持久化格式」的前提下就能跑通完整保存链路。
    fn persist_default_session(&mut self) {}
}

// 属性面板（`Properties`）的分派语义用单测钉住：标题 / 按钮 / 是否连接 / 写回哪条
// 都是**行为**，靠整窗截图看不出来（截图只拍到「按了之后长什么样」，拍不到
// 「按了之后连没连」）。测试另开一个文件，与 `session_form.rs` 同一惯例——
// 本文件已经贴着自己的代码行预算，不值得为了几十行断言再动一次。
#[cfg(test)]
#[path = "new_session_props_tests.rs"]
mod new_session_props_tests;
