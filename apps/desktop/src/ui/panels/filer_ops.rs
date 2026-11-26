//! Filer 的写操作：新建目录 / 改名 / 删除 / 改权限。
//!
//! 为什么单独一个文件：`filer.rs` 是「数据与数据源」，`filer_view.rs` 是绘制，
//! 这个文件是**对远端改东西**——三件事的失败方式完全不同（路径不合法会在本地
//! 被拦下、权限不对由服务端拒绝、删非空目录由服务端拒绝），混进前两个文件里
//! 会让「哪些错误是本地判的、哪些是服务端说的」变得看不出来。
//!
//! # 一条纪律：界面不替服务端编理由
//!
//! 本地只拦**看得出来**的错（空名字、带 `/` 的段、非八进制权限）。其余一律
//! 交给 `term-sftp`，失败时把服务端原话（`Permission denied` 之类）原样显示。
//! 转述成「操作失败」等于把用户唯一能拿到的线索擦掉。
//!
//! # 入口是右键菜单，不是工具栏按钮
//!
//! 面板的路径栏已经有四颗按钮，再加一排会让整窗基线漂移；而这些动作本来就
//! 是「对着某一行做」的，右键那一行才是自然的入口。代价是**不可见**——
//! 所以 `docs/` 与菜单里都要写清楚；这里选择的是「不为了可见性去改九张基线」。

use super::*;
use crate::ui::kit::window::{DialogId, DialogWindow};
use term_core::vt::paste_bytes_for;

/// 删整棵树的扫描文案：清单里目录数少算了一个根，`+1` 的换算单独放这里。
#[path = "filer_ops_tree_status.rs"]
mod tree_status;

/// 一次等着用户确认的写操作。
///
/// 全部用**绝对路径**存下来（而不是存行号 / 名字再回头查）：等用户点确认时
/// 列表可能已经刷新，按名字再查一次会指到别的东西上。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) enum FilerOp {
    /// 在当前目录下建一个目录。
    NewFolder { dir: String, name: String },
    /// 把 `from` 改名成同目录下的 `name`。
    Rename { from: String, name: String },
    /// 改权限位（八进制文本，如 `755`）。
    Chmod { path: String, mode: String },
    /// 删除一个文件或**空**目录。非空目录由服务端拒绝（我们不替它递归）。
    Delete { path: String, is_dir: bool },
    /// 删除**整棵树**：先扫描清单，用户显式确认后才逐条发 `RemoveFile` /
    /// `RemoveDir`。与 [`FilerOp::Delete`] 分开，是为了让破坏性更大的那一步
    /// 有自己的确认文案，且**不经过**「删空目录」那条轻量路径。
    DeleteTree { path: String },
}

/// 删整棵树前的扫描结果。跟着面板走，换会话 / 关面板时随 `Filer` 一起消失。
///
/// `entries: None` = 清单还没到（或扫描失败）；未到齐时确认按钮**必须不可点**，
/// 否则「先删文件后删目录」就没有依据，只剩一条删根目录的失败请求。
#[derive(Default)]
pub(in crate::ui) struct TreeDeleteScan {
    pub(in crate::ui) root: String,
    pub(in crate::ui) entries: Option<Vec<term_sftp::ScannedEntry>>,
    pub(in crate::ui) error: Option<String>,
}

impl TreeDeleteScan {
    fn begin(root: String) -> Self {
        Self {
            root,
            entries: None,
            error: None,
        }
    }
}

/// F7「把当前目录路径写进终端」的本地判据。纯值，便于单测。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum PathToTerminalBlock {
    /// 当前标签没有活着的会话——没地方写。
    NoSession,
    /// 只读查看（`Window ▸ Local Mode`）：键盘与工具面板都不许写 PTY。
    ReadOnly,
    /// 面板还不知道当前目录是什么。
    EmptyPath,
}

/// 能不能把路径送给当前终端。灰显理由随之一起给出来，不画假按钮。
pub(in crate::ui) fn path_to_terminal_blocker(
    path: &str,
    has_session: bool,
    read_only: bool,
) -> Option<PathToTerminalBlock> {
    if !has_session {
        Some(PathToTerminalBlock::NoSession)
    } else if read_only {
        Some(PathToTerminalBlock::ReadOnly)
    } else if path.trim().is_empty() {
        Some(PathToTerminalBlock::EmptyPath)
    } else {
        None
    }
}

/// 真正送到 PTY 的文本：整条路径、**不回车**。
///
/// 上游 WindTerm 的 F7 只是把路径摆到命令行上让用户接着敲命令；替他按 Enter 会把
/// 路径当成命令执行。编码仍走 `paste_bytes_for`（括号粘贴 / 控制字符过滤
/// 与粘贴、Sender 同一份），不另造一条写 PTY 的漏斗。
pub(in crate::ui) fn path_to_terminal_text(path: &str) -> String {
    path.trim().to_string()
}

impl Filer {
    pub(in crate::ui) fn note_tree_delete_scan(
        &mut self,
        path: &str,
        entries: Vec<term_sftp::ScannedEntry>,
    ) {
        if let Some(scan) = self.tree_delete.as_mut() {
            if path == scan.root {
                scan.entries = Some(entries);
                scan.error = None;
            }
        }
    }

    pub(in crate::ui) fn fail_tree_delete_scan(&mut self, path: &str, message: &str) {
        if let Some(scan) = self.tree_delete.as_mut() {
            if path == scan.root {
                scan.error = Some(message.to_string());
            }
        }
    }
}

impl FilerOp {
    /// 窗口标题：用户得一眼看出自己正在改什么。
    pub(in crate::ui) fn heading(&self) -> &'static str {
        match self {
            FilerOp::NewFolder { .. } => "New folder",
            FilerOp::Rename { .. } => "Rename",
            FilerOp::Chmod { .. } => "Permissions",
            FilerOp::Delete { .. } => "Delete",
            FilerOp::DeleteTree { .. } => "Delete tree",
        }
    }

    /// 说明那一行：把**完整目标路径**摆出来（同名文件在不同目录里很常见）。
    pub(in crate::ui) fn body(&self) -> String {
        match self {
            FilerOp::NewFolder { dir, .. } => format!("Create a folder in {dir}"),
            FilerOp::Rename { from, .. } => format!("Rename {from}"),
            FilerOp::Chmod { path, .. } => format!("Change the mode of {path}"),
            FilerOp::Delete { path, is_dir } => {
                if *is_dir {
                    format!("Delete the empty folder {path}")
                } else {
                    format!("Delete the file {path}")
                }
            }
            FilerOp::DeleteTree { path } => {
                format!("Delete the whole tree at {path} and everything inside it")
            }
        }
    }

    /// 要不要输入框（删除只要确认，没有可填的东西）。
    pub(in crate::ui) fn wants_input(&self) -> bool {
        !matches!(self, FilerOp::Delete { .. } | FilerOp::DeleteTree { .. })
    }

    /// 确认按钮上写什么。删除必须写动词本身——「OK」在破坏性操作上太轻。
    pub(in crate::ui) fn confirm_label(&self) -> &'static str {
        match self {
            FilerOp::NewFolder { .. } => "Create",
            FilerOp::Rename { .. } => "Rename",
            FilerOp::Chmod { .. } => "Apply",
            FilerOp::Delete { .. } => "Delete",
            FilerOp::DeleteTree { .. } => "Delete tree",
        }
    }

    /// 输入框里预先填什么。
    pub(in crate::ui) fn seed(&self) -> String {
        match self {
            FilerOp::NewFolder { name, .. } | FilerOp::Rename { name, .. } => name.clone(),
            FilerOp::Chmod { mode, .. } => mode.clone(),
            FilerOp::Delete { .. } | FilerOp::DeleteTree { .. } => String::new(),
        }
    }
}

/// 名字的本地校验：只拦「一定错」的，不猜服务端的规则。
///
/// 返回的错误是给人看的一句话，不是错误码——这一层没有别的消费者。
pub(in crate::ui) fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("名字不能为空".to_string());
    }
    if name == "." || name == ".." {
        return Err("`.` 与 `..` 不是名字".to_string());
    }
    if name.contains('/') {
        return Err("名字里不能有 `/`（要换目录请用路径栏）".to_string());
    }
    if name.contains('\0') {
        return Err("名字里不能有 NUL".to_string());
    }
    Ok(())
}

/// 八进制权限文本 → 数值。接受 `755` 与 `0755`，拒绝一切别的。
///
/// 上限 `0o7777`：再大的位对 chmod 没有意义，而 `u32::from_str_radix` 会
/// 老老实实收下 `77777777`——写进协议只会被服务端拒，不如在本地就说清。
pub(in crate::ui) fn parse_mode(text: &str) -> Result<u32, String> {
    let trimmed = text.trim();
    let digits = trimmed.strip_prefix("0o").unwrap_or(trimmed);
    if digits.is_empty() || digits.len() > 4 || !digits.chars().all(|c| ('0'..='7').contains(&c)) {
        return Err(format!("`{trimmed}` 不是八进制权限位（例：755 / 0644）"));
    }
    let mode = u32::from_str_radix(digits, 8).map_err(|e| e.to_string())?;
    if mode > 0o7777 {
        return Err(format!("`{trimmed}` 超过 0o7777"));
    }
    Ok(mode)
}

/// 目录 + 名字 → 路径。根目录下不会拼出 `//name`。
pub(in crate::ui) fn join_path(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// 把用户填的值校验一遍，再变成「可以发给 `term-sftp` 的那一次调用」。
///
/// 抽成纯函数是为了**能测**：真正的发送要一条 SFTP 连接，而这里要守的是
/// 「坏输入在本地就被拦下，不打一个来回」。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) enum OpRequest {
    Mkdir { path: String },
    Rename { from: String, to: String },
    Chmod { path: String, mode: u32 },
    RemoveFile { path: String },
    RemoveDir { path: String },
}

/// 校验用户输入并给出请求；`Err` 是要显示在窗口里的那句话。
pub(in crate::ui) fn plan(op: &FilerOp, input: &str) -> Result<OpRequest, String> {
    match op {
        FilerOp::NewFolder { dir, .. } => {
            let name = input.trim();
            validate_name(name)?;
            Ok(OpRequest::Mkdir {
                path: join_path(dir, name),
            })
        }
        FilerOp::Rename { from, .. } => {
            let name = input.trim();
            validate_name(name)?;
            let parent = term_sftp::parent(from).unwrap_or_else(|| "/".to_string());
            Ok(OpRequest::Rename {
                from: from.clone(),
                to: join_path(&parent, name),
            })
        }
        FilerOp::Chmod { path, .. } => Ok(OpRequest::Chmod {
            path: path.clone(),
            mode: parse_mode(input)?,
        }),
        FilerOp::Delete { path, is_dir } => Ok(if *is_dir {
            OpRequest::RemoveDir { path: path.clone() }
        } else {
            OpRequest::RemoveFile { path: path.clone() }
        }),
        // 整棵树不走「填个名字就发一条」的快路径：它要等的清单不在这里。
        FilerOp::DeleteTree { .. } => {
            Err("删除整棵树要先等扫描完成，再在确认窗里点「Delete tree」".to_string())
        }
    }
}

/// 把扫描到的整棵树变成一串**已经排好序**的既有请求。
///
/// 纯函数：不碰 `SftpHandle`、不看弹层状态，所以「先文件后后序目录」能被单测
/// 钉死。调用方只有在用户点了确认之后才允许拿它去发请求——这里故意不返回
/// 「直接执行」的闭包，避免谁在右键回调里顺手绕过确认。
pub(in crate::ui) fn plan_tree_delete(
    root: &str,
    entries: &[term_sftp::ScannedEntry],
) -> Result<Vec<OpRequest>, term_sftp::delete::PlanError> {
    Ok(term_sftp::delete::plan_tree_delete(root, entries)?
        .into_iter()
        .map(|step| match step {
            term_sftp::DeleteStep::File(path) => OpRequest::RemoveFile { path },
            term_sftp::DeleteStep::Dir(path) => OpRequest::RemoveDir { path },
        })
        .collect())
}

/// 这次操作改的是哪个目录下的东西——做完要刷新它。
pub(in crate::ui) fn refresh_dir(op: &FilerOp) -> String {
    match op {
        FilerOp::NewFolder { dir, .. } => dir.clone(),
        FilerOp::Rename { from, .. } => term_sftp::parent(from).unwrap_or_else(|| "/".to_string()),
        FilerOp::Chmod { path, .. } => term_sftp::parent(path).unwrap_or_else(|| "/".to_string()),
        FilerOp::Delete { path, .. } | FilerOp::DeleteTree { path } => {
            term_sftp::parent(path).unwrap_or_else(|| "/".to_string())
        }
    }
}

/// 弹层里能点的东西。先收集、后执行——绘制期间已经借了 `self.filer`。
enum PromptAction {
    None,
    Cancel,
    Confirm,
}

impl TerminalWindow {
    /// 右键菜单点了某一项：把弹层立起来。
    ///
    /// 删整棵树在这里**只发扫描请求**，一条删除都不发；清单回来后还要用户点
    /// 确认才会落地。扫描与确认分离是为了让「你到底要删多少东西」可见。
    pub(in crate::ui) fn begin_filer_op(&mut self, op: FilerOp) {
        self.filer.op_input = op.seed();
        self.filer.op_error = None;
        if let FilerOp::DeleteTree { path } = &op {
            match self.filer.handle.as_ref() {
                Some(handle) => {
                    handle.scan_tree(path);
                    self.filer.tree_delete = Some(TreeDeleteScan::begin(path.clone()));
                }
                None => self.filer.op_error = Some("文件面板还没连上，删不了整棵树".to_string()),
            }
        }
        self.filer.op = Some(op);
        self.ctx.request_repaint();
    }

    /// 画写操作的小弹层；用户确认就在这里真的发出去。
    pub(in crate::ui) fn draw_filer_prompt(&mut self, ctx: &egui::Context) {
        let Some(op) = self.filer.op.clone() else {
            return;
        };
        let mut action = PromptAction::None;
        // 输入框与错误提示都要可变借用，先摘出来，画完再放回去
        // （与 `draw_ssh_dialogs` 里「收集-后执行」同一个套路）。
        let mut input = std::mem::take(&mut self.filer.op_input);
        let error = self.filer.op_error.clone();
        // 标题只写动作名：正文已经把「改哪个路径」说清楚了，标题再抄一遍
        // 只会得到「New folder — Create a folder in /srv/www」这种重复。
        let title = op.heading().to_string();
        // 删整棵树要先看清规模：扫描没回来时确认按钮不可点，失败时把服务端
        // 原话留在窗里。判据只认当前 op 的那棵树，旧清单不能拿来删新目标。
        let (tree_status, tree_ready) = match (&op, self.filer.tree_delete.as_ref()) {
            (FilerOp::DeleteTree { path }, Some(scan)) if scan.root == *path => match &scan.error {
                Some(e) => (format!("扫描失败：{e}"), false),
                None => match &scan.entries {
                    Some(entries) => (tree_status::tree_scan_status(entries), true),
                    None => ("正在扫描整棵树…".to_string(), false),
                },
            },
            (FilerOp::DeleteTree { .. }, _) => ("正在扫描整棵树…".to_string(), false),
            _ => (String::new(), true),
        };

        DialogWindow::new(DialogId::FilerPrompt, &title).show(
            ctx,
            |ui| {
                ui.label(op.body());
                if matches!(op, FilerOp::DeleteTree { .. }) {
                    ui.label(&tree_status);
                }
                ui.add_space(6.0);
                let mut entered = false;
                if op.wants_input() {
                    let resp = ui.add(
                        TextEdit::singleline(&mut input)
                            .id(egui::Id::new("filer_op_input"))
                            .hint_text(match op {
                                FilerOp::Chmod { .. } => "755",
                                _ => "name",
                            }),
                    );
                    entered = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                }
                if let Some(e) = &error {
                    ui.add_space(4.0);
                    ui.colored_label(egui::Color32::from_rgb(0xE0, 0x6C, 0x6C), e);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        action = PromptAction::Cancel;
                    }
                    let confirm = ui.add_enabled(tree_ready, egui::Button::new(op.confirm_label()));
                    if confirm.clicked() || (entered && op.wants_input()) {
                        action = PromptAction::Confirm;
                    }
                });
            },
        );

        self.filer.op_input = input;
        match action {
            PromptAction::None => {}
            PromptAction::Cancel => {
                self.filer.op = None;
                self.filer.tree_delete = None;
                self.filer.op_input.clear();
                self.filer.op_error = None;
            }
            PromptAction::Confirm => {
                if let FilerOp::DeleteTree { path } = &op {
                    // 唯一的落地入口就是这一次点击：扫描、右键、绘制都不发删除。
                    let Some(mut scan) = self.filer.tree_delete.take() else {
                        self.filer.op_error = Some("树清单还没到，什么也没有删除".to_string());
                        return;
                    };
                    let Some(entries) = scan.entries.take() else {
                        self.filer.tree_delete = Some(scan);
                        self.filer.op_error = Some("树清单还没到，什么也没有删除".to_string());
                        return;
                    };
                    if scan.root != *path {
                        scan.entries = Some(entries);
                        self.filer.tree_delete = Some(scan);
                        self.filer.op_error = Some("树清单已经过期，什么也没有删除".to_string());
                        return;
                    }
                    match plan_tree_delete(path, &entries) {
                        Ok(reqs) => {
                            self.apply_filer_requests(reqs, refresh_dir(&op));
                            self.filer.op = None;
                            self.filer.op_input.clear();
                            self.filer.op_error = None;
                        }
                        Err(e) => {
                            scan.entries = Some(entries);
                            self.filer.tree_delete = Some(scan);
                            self.filer.op_error = Some(e.to_string());
                        }
                    }
                } else {
                    match plan(&op, &self.filer.op_input) {
                        Ok(req) => {
                            self.apply_filer_requests(vec![req], refresh_dir(&op));
                            self.filer.op = None;
                            self.filer.op_input.clear();
                            self.filer.op_error = None;
                        }
                        // 本地就说清楚的错，留在弹层上让人改，不打一个来回。
                        Err(message) => self.filer.op_error = Some(message),
                    }
                }
            }
        }
    }

    /// 把一次请求发给 `term-sftp`，随后重新列目录。
    ///
    /// 两条消息走**同一条队列**，所以 `list` 一定在写之后执行——不需要等
    /// `Done` 事件回来再发（那样还得记住「刚才那个 Done 是哪个操作的」）。
    fn apply_filer_requests(&mut self, reqs: Vec<OpRequest>, dir: String) {
        let Some(handle) = self.filer.handle.as_ref() else {
            self.filer.error = Some("文件面板还没连上，改动没有发出去".to_string());
            return;
        };
        // 全部走同一条会话的同一条请求通道，顺序即发出顺序；后台串行消费，
        // 所以「先文件后目录」不会在通道里被重排（见 `background.rs` 模块头）。
        for req in reqs {
            match req {
                OpRequest::Mkdir { path } => handle.mkdir(path),
                OpRequest::Rename { from, to } => handle.rename(from, to),
                OpRequest::Chmod { path, mode } => handle.chmod(path, mode),
                OpRequest::RemoveFile { path } => handle.remove_file(path),
                OpRequest::RemoveDir { path } => handle.remove_dir(path),
            }
        }
        handle.list(dir);
        self.filer.loading = true;
    }
}

/// 底栏三颗按钮的点击结果。先收集、后落地：绘制期间 `self.filer` 还借着。
#[derive(Default)]
pub(in crate::ui) struct FilerFooterClicks {
    pub copy_path: bool,
    pub sync_now: bool,
}

/// 底栏右侧那一段的宽度：排序文案 + 三颗按钮。
const FOOT_ACTIONS_W: f32 = 78.0;
const FOOT_BTN_W: f32 = 16.0;
const FOOT_BTN_PITCH: f32 = 18.0;

/// 三颗图标都用绘图原语画，不依赖夹具字体里有没有这些字形（同路径栏的 `⋮`）。
fn paint_footer_icon(p: &egui::Painter, r: egui::Rect, kind: usize, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.0, color);
    let c = r.center();
    let line = |a: egui::Vec2, b: egui::Vec2| p.line_segment([c + a, c + b], stroke);
    let circle = |radius: f32| {
        p.circle(
            c + egui::vec2(0.0, -1.0),
            radius,
            egui::Color32::TRANSPARENT,
            stroke,
        )
    };
    match kind {
        // 文件夹 + 出箭头：Copy Directory Path To Terminal。
        0 => {
            p.rect_stroke(
                egui::Rect::from_center_size(c + egui::vec2(-2.0, 1.0), egui::vec2(8.0, 6.0)),
                1.0,
                stroke,
            );
            line(egui::vec2(-1.0, 1.0), egui::vec2(6.0, -4.0));
            line(egui::vec2(3.0, -4.0), egui::vec2(6.0, -4.0));
        }
        // 定位钉：Sync Terminal Folder。
        1 => {
            circle(4.0);
            line(egui::vec2(0.0, 3.0), egui::vec2(0.0, 7.0));
        }
        // 靶心钉：Automatically Sync Terminal Folder（当前恒灰）。
        _ => {
            circle(5.0);
            circle(2.0);
            line(egui::vec2(0.0, 4.0), egui::vec2(0.0, 7.0));
        }
    }
}

impl TerminalWindow {
    /// 画底栏右侧：排序文案 + 三颗图标按钮。
    ///
    /// 提示文案用金图上的英文原文走 `tr()`。键名**不**写进 hover：F7/F8/F9
    /// 并没有绑进 `shortcut_map`，写出来就是「按了没反应」的假提示；命令面
    /// 不为一个悬停文案扩张。灰掉的那颗把原因写进 hover——只灰不说，用户会
    /// 以为面板坏了。
    pub(in crate::ui) fn draw_filer_footer_actions(
        &self,
        ui: &mut egui::Ui,
        foot_rect: egui::Rect,
        sort_label: &str,
        path_block: Option<PathToTerminalBlock>,
        sync_ready: bool,
        sync_reason: Option<&'static str>,
    ) -> FilerFooterClicks {
        let s = &term_core::theme::CHROME_SCALE;
        let dim = crate::theme::chrome(s.text).gamma_multiply(0.62);
        let rect = egui::Rect::from_min_size(
            egui::pos2(foot_rect.right() - FOOT_ACTIONS_W, foot_rect.top() + 2.0),
            egui::vec2(FOOT_ACTIONS_W - 2.0, foot_rect.height() - 4.0),
        );
        ui.painter().text(
            egui::pos2(rect.left() - 4.0, foot_rect.center().y),
            egui::Align2::RIGHT_CENTER,
            sort_label,
            crate::ui::type_scale::micro(),
            dim,
        );
        let copy_reason = path_block.map(|b| match b {
            PathToTerminalBlock::NoSession => self.tr("No session"),
            PathToTerminalBlock::ReadOnly | PathToTerminalBlock::EmptyPath => {
                self.tr("Not available")
            }
        });
        // F9 恒灰：没有后台轮询 / 偏好开关，画成可点的开关就是假开关。
        let rows = [
            (
                self.tr("Copy Directory Path To Terminal"),
                path_block.is_none(),
                copy_reason,
            ),
            (self.tr("Sync Terminal Folder"), sync_ready, sync_reason),
            (
                self.tr("Automatically Sync Terminal Folder"),
                false,
                Some(self.tr("Not wired (placeholder)")),
            ),
        ];
        let mut clicks = FilerFooterClicks::default();
        for (i, (label, enabled, reason)) in rows.into_iter().enumerate() {
            let x = rect.right() - (rows.len() - i) as f32 * FOOT_BTN_PITCH;
            let r = egui::Rect::from_min_size(
                egui::pos2(x, rect.center().y - FOOT_BTN_W / 2.0),
                egui::vec2(FOOT_BTN_W, FOOT_BTN_W),
            );
            paint_footer_icon(
                ui.painter(),
                r,
                i,
                if enabled { dim } else { dim.gamma_multiply(0.45) },
            );
            let hint = match reason {
                Some(why) if !enabled => format!("{label} — {why}"),
                _ => label.to_string(),
            };
            let resp = ui
                .interact(r, egui::Id::new(("filer_footer_btn", i)), egui::Sense::click())
                .on_hover_text(hint);
            if enabled && resp.clicked() {
                if i == 0 {
                    clicks.copy_path = true;
                } else if i == 1 {
                    clicks.sync_now = true;
                }
            }
        }
        clicks
    }

    /// F7 落地：把当前 Filer 目录路径写进当前标签的 PTY。
    /// 没有活会话 / 只读查看时一个字节也不发，返回 `false`。
    pub(in crate::ui) fn copy_filer_path_to_terminal(&self, path: &str) -> bool {
        let Some(tab) = self.tabs.get(self.active_slot()) else {
            return false;
        };
        if path_to_terminal_blocker(path, tab.session.is_some(), tab.local_mode).is_some() {
            return false;
        }
        let bytes = paste_bytes_for(&path_to_terminal_text(path), tab.snapshot.bracketed_paste);
        tab.write(bytes.as_bytes())
    }
}

#[cfg(test)]
#[path = "filer_ops_tests.rs"]
mod tests;

/// 删整棵树的单测拆到单独文件：`filer_ops.rs` 的**代码行**已经贴着 500 的门禁，
/// 与其删注释 / 压缩断言，不如把这一块整体挪出来（与 `filer_sync_tests.rs` 同套路）。
#[cfg(test)]
#[path = "filer_ops_tree_tests.rs"]
mod tree_tests;
