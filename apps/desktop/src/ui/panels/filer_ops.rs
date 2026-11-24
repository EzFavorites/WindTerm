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
                    Some(entries) => {
                        let files = entries.iter().filter(|e| !e.is_dir).count();
                        let dirs = entries.len() - files;
                        (
                            format!("扫描完成：{files} 个文件、{dirs} 个目录（含根自身）"),
                            true,
                        )
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn op_new(dir: &str) -> FilerOp {
        FilerOp::NewFolder {
            dir: dir.to_string(),
            name: String::new(),
        }
    }

    #[test]
    fn a_name_with_a_slash_is_refused_before_anything_is_sent() {
        assert!(validate_name("logs").is_ok());
        assert!(validate_name("my logs 2026").is_ok());
        for bad in ["", "   ", ".", "..", "a/b"] {
            assert!(validate_name(bad).is_err(), "{bad:?} 应当被拒");
        }
        // 中文与空格是合法名字——不要顺手把宽字符也拦了。
        assert!(validate_name("日志 备份").is_ok());
    }

    #[test]
    fn the_mode_parser_takes_three_and_four_digits_and_nothing_else() {
        assert_eq!(parse_mode("755"), Ok(0o755));
        assert_eq!(parse_mode("0755"), Ok(0o755));
        assert_eq!(parse_mode(" 644 "), Ok(0o644));
        assert_eq!(parse_mode("0o750"), Ok(0o750));
        for bad in ["", "8", "abc", "7555x", "77777", "0x1f"] {
            assert!(parse_mode(bad).is_err(), "{bad:?} 应当被拒");
        }
    }

    #[test]
    fn joining_keeps_a_single_slash_even_at_the_root() {
        assert_eq!(join_path("/home/deploy", "logs"), "/home/deploy/logs");
        assert_eq!(join_path("/", "logs"), "/logs");
        assert_eq!(join_path("/home/deploy/", "logs"), "/home/deploy/logs");
    }

    #[test]
    fn planning_a_new_folder_turns_the_typed_name_into_one_absolute_path() {
        let op = op_new("/srv/www");
        assert_eq!(
            plan(&op, " logs ").expect("合法名字"),
            OpRequest::Mkdir {
                path: "/srv/www/logs".to_string()
            }
        );
        // 坏名字**不产生请求**——这样调用方就没机会先发出去再报错。
        assert!(plan(&op, "a/b").is_err());
    }

    #[test]
    fn renaming_stays_inside_the_same_directory() {
        let op = FilerOp::Rename {
            from: "/srv/www/old.conf".to_string(),
            name: String::new(),
        };
        assert_eq!(
            plan(&op, "new.conf").expect("合法名字"),
            OpRequest::Rename {
                from: "/srv/www/old.conf".to_string(),
                to: "/srv/www/new.conf".to_string(),
            }
        );
    }

    #[test]
    fn deleting_a_folder_asks_for_remove_dir_and_a_file_for_remove_file() {
        let dir = FilerOp::Delete {
            path: "/srv/emptydir".to_string(),
            is_dir: true,
        };
        let file = FilerOp::Delete {
            path: "/srv/a.txt".to_string(),
            is_dir: false,
        };
        assert_eq!(
            plan(&dir, "").expect("删除不需要输入"),
            OpRequest::RemoveDir {
                path: "/srv/emptydir".to_string()
            }
        );
        assert_eq!(
            plan(&file, "").expect("删除不需要输入"),
            OpRequest::RemoveFile {
                path: "/srv/a.txt".to_string()
            }
        );
        // 两个按钮文案必须不同：「Delete」对文件和空目录做的事不一样，
        // 用户点之前该看得出来（非空目录会被服务端拒，这是有意的）。
        assert!(dir.body().contains("empty folder"));
        assert!(file.body().contains("file"));
    }

    #[test]
    fn every_operation_refreshes_the_directory_it_touched() {
        assert_eq!(
            refresh_dir(&FilerOp::NewFolder {
                dir: "/srv/www".to_string(),
                name: String::new()
            }),
            "/srv/www"
        );
        assert_eq!(
            refresh_dir(&FilerOp::Delete {
                path: "/srv/www/a.txt".to_string(),
                is_dir: false
            }),
            "/srv/www"
        );
        assert_eq!(
            refresh_dir(&FilerOp::DeleteTree {
                path: "/srv/www/tree".to_string()
            }),
            "/srv/www"
        );
    }
}

/// 删整棵树的单测拆到单独文件：`filer_ops.rs` 的**代码行**已经贴着 500 的门禁，
/// 与其删注释 / 压缩断言，不如把这一块整体挪出来（与 `filer_sync_tests.rs` 同套路）。
#[cfg(test)]
#[path = "filer_ops_tree_tests.rs"]
mod tree_tests;
