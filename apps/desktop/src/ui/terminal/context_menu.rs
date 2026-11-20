//! 终端右键上下文菜单 + 中键粘贴（E-SELSCP / 13 §2.4）。
//!
//! 中键粘贴走和 `Cmd+V` **同一条路径**：`ViewportCommand::RequestPaste` →
//! 下一帧 `Event::Paste(text)` → `paste_bytes_for` 编码 → PTY。不在这一层
//! 自己读剪贴板（egui 0.29 也只暴露写接口），于是换行归一、控制字符过滤、
//! 括号粘贴三件事只有一份实现（F1-b）。
//!
//! 2026-11-11：**按视觉基线对齐右键菜单**。菜单主体（18 个文字行、一排七色
//! 圆点、9 条分隔线与 4 个子菜单箭头）由 `context_rows.rs::TERMINAL_ROWS`
//! 逐字描述，绘制复用 `shell::menu_style::draw_menu_row` 的四栏手绘——与
//! 菜单栏弹层同一套三态色、快捷键固定列、行高。截图里没有对应能力的项目
//! 一律灰显（R5），不摆空转开关；项目自有但视觉基线没有的能力（Margin /
//! Zoom / 脚本 / 激活窗格）收进末尾 `More ▸`，既不丢功能，也不破坏主菜单
//! 的 1:1。

use crate::ui::*;
use term_app::find::FindAction;
use term_app::margin::MarginAction;
use term_app::mouse::{RightClickAction, RightClickDecision};
use term_app::tab::TabAction;

use super::context_rows::{draw_context_rows, Work, TERMINAL_ROWS};
use super::tab_slot::{selection_text_ref, tab_of, TabSlot};

impl TerminalWindow {
    /// `Terminal ▸ Mouse Actions ▸ Right click:` 的三档分派（2026-11-18）。
    ///
    /// 菜单档原样交给 [`Self::draw_terminal_context`]——挂 `resp.context_menu`
    /// 就是接线之前的行为，一个字不改。另两档**不挂** egui 的浮层，改为自己
    /// 判 `Secondary` 按下，再把动作接到既有的两条落地上：
    ///
    /// - 粘贴 → [`Self::request_paste`]（`ViewportCommand::RequestPaste`），
    ///   与 `Cmd+V` / 中键**同一条** `Event::Paste` 通道，括号粘贴与换行归一
    ///   只有一份实现；
    /// - 复制 → `ctx.copy_text`，与右键菜单里的 `Copy` 同一句实现。
    ///
    /// 为什么判据只看「选区非空」：第三档的文案就是
    /// `Copy text if selected, otherwise paste text`，文案即判据
    /// （判决表在 `term_app::mouse::decide`，三档 × 有无选区的六种组合有单测）。
    pub(in crate::ui) fn handle_right_click(
        &mut self,
        ui: &mut egui::Ui,
        resp: &egui::Response,
        slot: TabSlot,
    ) {
        let action = RightClickAction::from_index(self.right_click);
        if action == RightClickAction::Menu {
            self.draw_terminal_context(ui, resp, slot);
            return;
        }
        // 与中键同一条规矩：只认指针落在本窗格里的右键（`InputState` 是整帧
        // 全局的，不看 hover 会让分屏的每个窗格各粘贴/复制一次）。
        if !resp.hovered() {
            return;
        }
        let pressed = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary));
        if !pressed {
            return;
        }
        let selection =
            selection_text_ref(&self.selection_text, &self.selection_text_shell, slot).to_string();
        let decision = term_app::mouse::decide(action, !selection.is_empty());
        match decision {
            // 菜单档在函数开头就返回了；这条分支只是把 `decide` 的三种结果对齐，
            // 不在这里再挂一次菜单（挂两次会弹两个浮层）。
            RightClickDecision::Menu => {}
            RightClickDecision::Paste => self.request_paste(),
            RightClickDecision::Copy => {
                if !selection.is_empty() {
                    self.ctx.copy_text(selection);
                }
            }
        }
        if resp.has_focus() {
            if slot.is_main() {
                self.restore_focus = true;
            } else {
                self.restore_focus_shell = true;
            }
        }
    }

    /// 在终端区的 response 上挂右键菜单。`slot` 让分屏 / 底带面板各自作用在
    /// 正确的窗格上（2026-11-15：参数从「标签下标」换成槽位，`Shell` 面板因此
    /// 拿到了与兼容终端同一份菜单）。
    pub(in crate::ui) fn draw_terminal_context(&mut self, _ui: &mut egui::Ui, resp: &egui::Response, slot: TabSlot) {
        // 状态与文案先拷出来：闭包里不能借 `self`（`resp.context_menu` 的闭包与
        // `&mut self` 的方法体共用同一个 `self`），`MarginState` 是五个 bool，
        // 拷一份比绕 borrow 更清楚。
        let mut work: Vec<Work> = Vec::new();
        let lang = self.lang();
        let tr = |en: &'static str| t(lang, en);
        let activate = tr("Activate this pane");
        let run_script = tr("Run Script");
        let more = tr("More");
        resp.context_menu(|ui| {
            draw_context_rows(self, lang, ui, TERMINAL_ROWS, &mut work, None);
            // 视觉基线之外的项目自有能力：只多一个子菜单入口。
            ui.separator();
            ui.menu_button(more, |ui| {
                // Margin / Zoom 已挪到「左边缘专用菜单」（`draw_margin_context`）：
                // 兼容终端里左边缘与内容区是两份菜单，不该把 margin 塞进内容菜单。
                // 脚本入口本来就是上下文相关动作，菜单栏没有同义路径。
                if ui.button(run_script).clicked() {
                    work.push(Work::RunScript);
                    ui.close_menu();
                }
                if ui.button(activate).clicked() {
                    work.push(Work::Focus);
                    ui.close_menu();
                }
            });
        });

        for w in work {
            match w {
                Work::Copy => {
                    let text =
                        selection_text_ref(&self.selection_text, &self.selection_text_shell, slot);
                    if !text.is_empty() {
                        let text = text.to_string();
                        self.ctx.copy_text(text);
                    }
                }
                // 2026-11-19：复制整条命令（OSC 133 边界），与 `Copy` 不同。
                Work::CopyCommand => self.copy_command(),
                Work::Paste => self.request_paste(),
                // 「粘贴选中」粘的是**屏幕上选中的那几个字**，与剪贴板无关
                // （2026-11-17 用户校正：它不是 Paste 的别名）。所以不走
                // `RequestPaste`（那条路读的是平台剪贴板），直接把选区文本交给
                // `commit_paste`——粘贴的**唯一落地点**：括号粘贴、换行归一、
                // 输入行、命令行历史与 Cmd+V 完全同一份。
                //
                // 没有选区时静默（与 `Copy` 的空选区行为同一条约定）：菜单行是
                // 静态表，判不了「此刻有没有选区」，把判据放到这里比把整张表
                // 改成每帧重建更划算。
                Work::PasteSelection => {
                    let text = selection_text_ref(
                        &self.selection_text,
                        &self.selection_text_shell,
                        slot,
                    )
                    .to_string();
                    if !text.is_empty() {
                        self.commit_paste(slot, &text);
                    }
                }
                // 标签页级的动作只对主区生效：`Shell` 面板不是标签页（它不在
                // `tabs` 里），把「全选 / 分屏 / 激活窗格」派给活动标签，等于
                // 用户在面板里点一下、主区的选区或布局跟着变。
                Work::SelectAll => {
                    if slot.is_main() {
                        self.dispatch_action(Action::Select(term_app::selection::Op::All));
                    }
                }
                Work::Find => self.dispatch_action(Action::Find(FindAction::Open)),
                Work::ClearScreen => {
                    if let Some(tab) = tab_of(&mut self.tabs, &mut self.shell_tabs, slot) {
                        tab.clear();
                    }
                }
                Work::ClearScrollback => {
                    if let Some(tab) = tab_of(&mut self.tabs, &mut self.shell_tabs, slot) {
                        tab.clear_scrollback();
                    }
                }
                Work::ClearAll => {
                    if let Some(tab) = tab_of(&mut self.tabs, &mut self.shell_tabs, slot) {
                        tab.clear_all();
                    }
                }
                // 与菜单栏 `Text.GotoPreviousCommand` / `Text.GotoNextCommand`
                // 走同一条 `Op`：两处入口一份位移规则。
                Work::GotoCommand { next } => {
                    if slot.is_main() {
                        let op = if next {
                            term_app::selection::Op::CommandNext
                        } else {
                            term_app::selection::Op::CommandPrev
                        };
                        self.dispatch_action(Action::Select(op));
                    }
                }
                Work::ManageTriggers => self.dispatch_action(Action::Shell(
                    term_app::shell::ShellAction::ManageTriggers,
                )),
                // 与菜单栏 `Window.ToggleFullScreen` 同一条命令（窗口系统在
                // `update()` 里按 `view.full_screen` 同步）。
                Work::FullScreen => self.dispatch_action(Action::Shell(
                    term_app::shell::ShellAction::ToggleFullScreen,
                )),
                Work::Zoom(dir) => self.dispatch_action(Action::Shell(term_app::shell::ShellAction::ZoomIn).zoom(dir)),
                Work::Focus => {
                    // ADR-0025 阶段 3：槽位已是恒在身份，直接登记为活动身份。
                    if let TabSlot::Main(id) = slot {
                        self.active = Some(id);
                    }
                }
                Work::Split(horizontal) => {
                    if slot.is_main() {
                        self.dispatch_action(Action::Tab(TabAction::Split { horizontal }));
                    }
                }
                // 与菜单栏 `View › Margin` 走**同一条** `reduce`——两处入口一份状态。
                Work::Margin(part) => {
                    self.dispatch_action(Action::Margin(MarginAction::Toggle(part)))
                }
                Work::RunScript => self.open_script_dialog_for(slot),
                // 菜单树里的命令（`Log ▸` 的七条）：与菜单栏点同一条命令是
                // 同一条落地路径——判态用同一个 `action_state`，派发用同一个
                // `dispatch`，两处不存在第二份实现。
                Work::Command(id) => self.dispatch(id),
            }
        }
    }

    /// 终端**左边缘（margin 栏）专用**右键菜单。兼容终端里左边缘与内容区是**两份**
    /// 不同的右键菜单：左边缘给「边距 / 符号 / 空白 / 折叠 / 缩放」，内容区给
    /// 复制/粘贴/查找那一套。这里走**与内容菜单同一套**手绘行（`draw_context_rows`），
    /// 风格与宽度自适应完全一致。
    ///
    /// 菜单里的动作没有一条是标签页级的（改的是窗口的 margin / 缩放状态），
    /// 所以槽位只用来对齐签名——底带面板的左边缘与兼容终端是同一份菜单。
    pub(in crate::ui) fn draw_margin_context(
        &mut self,
        _ui: &mut egui::Ui,
        resp: &egui::Response,
        _slot: TabSlot,
    ) {
        let mut work: Vec<Work> = Vec::new();
        let lang = self.lang();
        let rows = super::context_rows::margin_rows(&self.view.margin, lang);
        resp.context_menu(|ui| {
            draw_context_rows(self, lang, ui, &rows, &mut work, None);
        });
        for w in work {
            match w {
                Work::Margin(part) => {
                    self.dispatch_action(Action::Margin(MarginAction::Toggle(part)))
                }
                Work::Zoom(dir) => self.dispatch_action(
                    Action::Shell(term_app::shell::ShellAction::ZoomIn).zoom(dir),
                ),
                _ => {}
            }
        }
    }

    /// 中键按下 = 请求平台剪贴板。焦点丢失时记得还回去（F1-b）。
    ///
    /// **只认指针落在本窗格里的中键**：`InputState` 是整帧全局的，不看 hover
    /// 就等于「在文件面板里中键一按，终端跟着粘贴」；分屏时每个窗格都会各粘
    /// 一次。与文件面板滚轮串面板是同一个根因（2026-11-14 一并收口）：面板内
    /// 的手势必须先问「指针在不在这块区域里」，焦点只管键盘，不管指针事件。
    ///
    /// 2026-11-15：还焦点的那面旗按**槽位**记（`restore_focus` /
    /// `restore_focus_shell`）。混成一份的话，在底带面板里中键粘贴一次，
    /// 焦点就被主区抢走了——而粘贴的文字属于面板。
    pub(in crate::ui) fn handle_middle_paste(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        slot: TabSlot,
    ) {
        if !resp.hovered() {
            return;
        }
        let middle = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Middle));
        if middle {
            self.request_paste();
            if resp.has_focus() {
                if slot.is_main() {
                    self.restore_focus = true;
                } else {
                    self.restore_focus_shell = true;
                }
            }
        }
    }

    fn request_paste(&self) {
        self.ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
    }
}

/// 把缩放方向映射成强类型动作（避免投影层拼命令 ID）。
trait ZoomAction {
    fn zoom(self, dir: i8) -> Action;
}
impl ZoomAction for Action {
    fn zoom(self, dir: i8) -> Action {
        let _ = self;
        let a = match dir {
            d if d > 0 => term_app::shell::ShellAction::ZoomIn,
            d if d < 0 => term_app::shell::ShellAction::ZoomOut,
            _ => term_app::shell::ShellAction::ZoomReset,
        };
        Action::Shell(a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_directions_map_to_three_distinct_actions() {
        let base = Action::Shell(term_app::shell::ShellAction::ZoomIn);
        assert_ne!(base.clone().zoom(1), base.clone().zoom(-1));
        assert_ne!(base.clone().zoom(-1), base.zoom(0));
    }
}
