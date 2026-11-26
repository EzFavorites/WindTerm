//! Sender 面板：**一条工具行 + 一块无边框多行文本区**（2026-11-07 重做）。
//!
//! # 上一版错在哪
//!
//! 上一版把「发什么」做成了六个带框按钮（`pwd` / `ls -la` / …）加一行说明
//! 文字。视觉基线里这两样都不存在：Sender 是一个**简单的自动化工具**——上面一行
//! 配置，下面一块**没有外框**的多行文本区。用户原话是「输入框不是一个
//! textEdit，怎么文字还有包围」——所以文本区必须是真的 `TextEdit::multiline`
//! + `frame(false)`，文字四周不许出现任何框。
//!
//! # 逐像素（视觉基线 1280 宽 @1x，工具行 y 32..56）
//!
//! | 元素 | 视觉基线 x | 备注 |
//! |---|---|---|
//! | `▶` 运行 | 12..19 | 饱和绿 `#008200`（全图唯一的绿） |
//! | `■` 停止 | 40..47 | `#646464` |
//! | `＋` `−` `×` | 69..76 / 97..104 / 126..132 | `#A1A1A1` |
//! | 竖分隔线 | 145 | 单列 `#444444`，整行高 |
//! | `◉ Text` `○ Hex` | 162..217 / 228..302 | 圆 13 点，实心点只在选中的那颗 |
//! | `By:` | 289..308 | 它管的是后面那组 |
//! | `◉ Line` `○ Char` | 328..383 / 394..449 | |
//! | `Count:` + 数字框 | 462..528 / 519..594 | 框内 `1` + 右侧上下箭头 |
//! | `Interval:` + 数字框 | 605..673 / 685..784 | 框内 `1.00s` |
//! | `Targets:` + 下拉 | 691..728 / 868..1097 | 展开是五行单选 |
//! | `Selected Sessions` 右边的多选下拉 | 紧随 `Targets` 之后 | 只有这一档才出现；点开是可勾选的标签列表 |
//!
//! 文本区（y 57 以下）是纯 `#1C1C1C`、**无外框**，左上角只有一个行号 `1`
//! （x 29..34，品红 `#FF0095`）。行号列因此是本文件唯一不来自 `CHROME_SCALE`
//! 的颜色——与 `dock_bottom::dot_color` 的实测色同一种处理。
//!
//! # 工具行上的每一项都是真的
//!
//! - `▶` 按当前配置切片并**真的写进 PTY**：文本走 `paste_bytes_for`（与粘贴、
//!   预设命令同一条编码路径），Hex 走原始字节。有间隔时由 `pump_sender` 按
//!   `ctx.input(|i| i.time)` 逐片发送。
//! - `■` 停掉正在跑的定时发送（`run = None`）。
//! - `＋ / − / ×` 对**文本区**生效：末尾新起一行 / 删掉最后一行 / 清空；三者
//!   都把光标挪到末尾并抢回焦点，点完能直接接着敲。
//! - `Count` 是「每一片发几次」，`Interval` 是「两片之间等多久」——两者一起
//!   决定 `run` 的节奏；间隔为 0 或只有一片时**一次性写完**，不装定时器。
//! - `Targets` 决定写给哪些标签（五项见 `Target`）。单标签时与粘贴等价。
//! - `Selected Sessions` 右边多一格**多选下拉**：勾哪些标签就发给哪些，一个都不勾
//!   就一个字节都不发（并给出提示，而不是「发送成功、对面没收到」）。它是显式的
//!   ——这一档不再偷偷把侧栏选中那条并进来（那等于指定不了）。
//!
//! # 状态住在哪
//!
//! 全部在投影层（`TerminalWindow::sender`），与 `find_input` 同一类：文本缓冲、
//! 选择项、勾选集、定时器都是**控件状态**，不是领域语义，一个字段都不进 `ViewState`。
//! 勾选用 `Tab::identity` 键控，不是名字也不是序号：同名标签是两条会话，序号会随
//! 关标签整体前移。发送走既有的 `SupervisorHandle::write`，没有第二条写 PTY 的路。
//!
//! 文件三分的理由只有一个：本文件管**发送与编辑器**，工具行在
//! `tools_sender_row.rs`，类型与纯函数在 `tools_sender_state.rs`。

use crate::ui::*;
use eframe::egui;
use term_core::theme::CHROME_SCALE;
use term_core::vt::paste_bytes_for;

use super::tools_sender_menu::{
    picked_menu_id, picked_popup, targets_menu_id, targets_popup, PickRow,
};
use super::tools_sender_row::{tool_row, Cmd, ROW_H};
use super::tools_sender_state::{
    picked_indices, plan, plan_then_write, repeat, Piece, PlanError, Run, SenderState, Target,
};
use super::tools_sender_tabs::{self as tabs, TAB_STRIP_H};

/// 工具行与文本区之间的空隙。
const BODY_GAP: f32 = 3.0;
/// 行号列宽：数字右缘落在「正文左缘 + 26」（视觉基线的 x 34），正文从 +32 起排。
const GUTTER_W: f32 = 32.0;
/// 行号色。**实测色**，与 `dock_bottom::dot_color` 同一种处理。
const GUTTER: u32 = 0xFF0095;
/// Interval 的步进（秒）。
const INTERVAL_STEP: f32 = 0.25;
/// Interval 的上下限：上游最小 20ms（见 kingtoolbox-docs/Sender-Pane/Send-Chars），
/// 不是 0——「一次写完」由 Hex/连续模式自己表达，低于 20ms 只是忙等。
const INTERVAL_MIN: f32 = 0.02;
const INTERVAL_MAX: f32 = 60.0;

/// 满足谓词的标签序号。
fn indices(tabs: &[Tab], f: impl Fn(&Tab) -> bool) -> Vec<usize> {
    tabs.iter()
        .enumerate()
        .filter(|(_, t)| f(t))
        .map(|(i, _)| i)
        .collect()
}

/// 一片写进一个标签。**编码只有这一处**。
///
/// 两个变体走两条完全不同的路，这正是 `Piece` 存在的理由：
/// - `Text`：与粘贴 / 预设命令同一条路径（`paste_bytes_for`：换行归一 + 括号粘贴）；
/// - `Bytes`：原样上线，**不**做任何归一——它就是用户逐字节写下来的东西，
///   替他把 `0x0a` 改成 `0x0d` 等于改了他的报文。
///
/// 取 `&Tab` 而不是 `&mut Tab`：真正写 PTY 的是控制面（`Tab::write` 内部走
/// `SessionPool`），所以「一片发给多个标签」不需要可变借用，也就不用
/// 先把片序列克隆出来再写。
fn write_piece(tab: &Tab, piece: &Piece) {
    match piece {
        // `Tab::write` 的返回值是「有没有真的写进会话」；已断开的标签写不进去
        // 是正常处境（目标里可能混着一个断开的标签），不该中断整批发送。
        Piece::Bytes(bytes) => {
            tab.write(bytes);
        }
        Piece::Text(text) => {
            let bytes = paste_bytes_for(text, tab.snapshot.bracketed_paste);
            tab.write(bytes.as_bytes());
        }
    }
}

impl TerminalWindow {
    /// 当前页的配置（只读）。工具行、提示文字、目标判定都从这里取。
    pub(in crate::ui) fn sender_state(&self) -> &SenderState {
        self.sender.state()
    }

    /// 按当前配置算出**解析好**的片序列（含 `Count` 重复）。
    ///
    /// 返回 `Err` = 一个字节都还没写，`note()` 是给用户看的那句话。
    fn sender_pieces(&self) -> Result<Vec<Piece>, PlanError> {
        let s = self.sender.state();
        // `plan` 只负责「切 + 解析」，重复次数是发送语义，分开算更清楚。
        let pieces = plan(&s.buf, s.hex, s.by_char)?;
        Ok(repeat(&pieces, s.count))
    }

    /// 这一片要发给哪些标签（按标签序号，去重保序）。
    ///
    /// 取 `&SenderState` 入参而不是读 `self.sender.state()`：多页之后每个
    /// 页的目标可以不同，定时发送要按**各页自己的**配置算目标。
    pub(in crate::ui) fn sender_targets(&self, s: &SenderState) -> Vec<usize> {
        let active = self.active_slot();
        let same_group = self
            .tabs
            .get(active)
            .map(|t| t.config.group.clone())
            .unwrap_or_default();
        match s.target {
            Target::Current => vec![active],
            Target::Group => indices(&self.tabs, |t| t.config.group == same_group),
            Target::Window | Target::AllWindows => (0..self.tabs.len()).collect(),
            // 勾选集是**显式**的：以前这里偷偷把侧栏选中那条并进来，于是
            // 「指定会话」其实指定不了——用户勾了 A，B 也收到。现在只有勾上的发。
            // 已关掉的标签不在册，`picked_indices` 自然跳过（不需要清理 `picked`）。
            Target::Selected => {
                let ids: Vec<_> = self.tabs.iter().map(|t| t.identity).collect();
                picked_indices(&s.picked, &ids)
            }
        }
    }

    /// 已勾选、且**仍在册**的会话名（按标签顺序）——多选下拉未展开时显示它。
    pub(super) fn sender_picked_names(&self) -> Vec<String> {
        let picked = &self.sender.state().picked;
        self.tabs
            .iter()
            .filter(|t| picked.contains(&t.identity))
            .map(|t| t.config.name.clone())
            .collect()
    }

    /// 多选弹层的候选行：**开着的标签**，不是会话库。
    ///
    /// 能发的前提是有一条活着的会话通道，而「开着的标签」正是它的全部：
    /// 会话库里那些没连上的条目发不出任何字节，列进来只会让用户勾了也白勾。
    pub(super) fn sender_pick_rows(&self) -> Vec<PickRow> {
        let picked = &self.sender.state().picked;
        self.tabs
            .iter()
            .map(|t| PickRow {
                name: t.config.name.clone(),
                on: picked.contains(&t.identity),
            })
            .collect()
    }

    /// 把一片发给所有目标标签。
    fn send_piece(&self, s: &SenderState, piece: &Piece) {
        for i in self.sender_targets(s) {
            if let Some(tab) = self.tabs.get(i) {
                write_piece(tab, piece);
            }
        }
    }

    /// 定时发送的推进。每帧调一次；没有任何页在跑时是空操作（基线零成本）。
    ///
    /// **遍历所有页**，不是只推进当前页：每页都有自己的节奏，切走一页
    /// 不该让它的定时发送停掉——那正是「一页一份 `Run`」的理由。
    pub(in crate::ui) fn pump_sender(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        for i in 0..self.sender.tabs().len() {
            // 先把这一帧到期的片收完再落地：`send_piece` 要借 `self`，
            // 与 `state.run` 的借用不能重叠。片在装定时器之前就解析完了，
            // 所以这里没有「解析失败」这一支。
            let Some(mut run) = self.sender.tabs_mut()[i].state.run.take() else {
                continue;
            };
            let mut due: Vec<Piece> = Vec::new();
            while run.at < run.pieces.len() && now + 1e-6 >= run.due {
                due.push(run.pieces[run.at].clone());
                run.at += 1;
                run.due += run.gap;
            }
            if !due.is_empty() {
                // 目标按**这一页自己的** Targets / 勾选集算。
                let state = &self.sender.tabs()[i].state;
                for piece in &due {
                    self.send_piece(state, piece);
                }
            }
            if run.at < run.pieces.len() {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(run.gap.max(0.01)));
                self.sender.tabs_mut()[i].state.run = Some(run);
            }
        }
    }

    /// `▶`：按计划发送。没有间隔就一次性写完，有间隔才装定时器。
    ///
    /// 两条路都**先整体校验**：解析错误在这里就返回，没写出去的字节一个都
    /// 不会写（定时发送也一样——它跑起来之后不再做任何解析）。
    fn sender_start(&mut self) {
        // 「指定会话」一个都没勾时**一个字节都不发**：这时候往下走只会得出
        // 「发送成功、对面没收到」，比直接说清楚差得多。
        if self.sender.state().target == Target::Selected
            && self.sender_targets(self.sender.state()).is_empty()
        {
            let note = self.tr("No target session is selected").to_string();
            self.sender.state_mut().note = Some(note);
            return;
        }
        let gap = self.sender.state().interval as f64;
        if gap > 0.0 {
            // 有间隔：先把整份片序列算好，再决定是装定时器还是一次写完。
            let pieces = match self.sender_pieces() {
                Ok(p) => p,
                Err(e) => {
                    self.sender.state_mut().note = Some(e.note());
                    return;
                }
            };
            if pieces.is_empty() {
                self.sender.state_mut().note = Some("文本区是空的".to_string());
                return;
            }
            if pieces.len() > 1 {
                let now = self.ctx.input(|i| i.time);
                self.sender.state_mut().run = Some(Run {
                    pieces,
                    at: 0,
                    due: now,
                    gap,
                });
                return;
            }
            // 只有一片：落到下面一次性写完，不装定时器（间隔对它没有意义）。
        }
        // 一次性写完。`plan_then_write` 把「先校验、后动笔」写死在顺序里：
        // 写错时闭包一次都不会被调用。
        let (hex, by_char, count) = {
            let s = self.sender.state();
            (s.hex, s.by_char, s.count)
        };
        let targets = self.sender_targets(self.sender.state());
        let outcome = plan_then_write(&self.sender.state().buf, hex, by_char, count, |piece| {
            for &i in &targets {
                if let Some(tab) = self.tabs.get(i) {
                    write_piece(tab, piece);
                }
            }
        });
        match outcome {
            Ok(0) => self.sender.state_mut().note = Some("文本区是空的".to_string()),
            Ok(_) => {}
            Err(e) => self.sender.state_mut().note = Some(e.note()),
        }
    }

    /// 工具行 + 文本区。
    pub(in crate::ui) fn draw_sender(&mut self, ui: &mut egui::Ui) {
        // 在 ScrollArea 里，`available_rect_before_wrap` 给出内容区的高，
        // 可视高只有 `clip_rect` 知道——文本区要按可视高铺满。
        let view_h = ui.clip_rect().height();
        let full = ui.available_rect_before_wrap();
        let row = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), ROW_H));
        let out = tool_row(self, ui, row);
        if let Some(cmd) = out.cmd {
            self.apply_cmd(ui, cmd);
        }
        if let Some(t) = targets_popup(ui, out.anchor, self.sender.state().target, self.lang()) {
            self.apply_cmd(ui, Cmd::Target(t));
        }
        // 多选弹层要用本帧的标签列表：名字与勾选态都是从标签现算的，
        // 所以它必须在 `tool_row` 之后画（锚点是那一格的矩形）。
        if let Some(anchor) = out.picked {
            let rows = self.sender_pick_rows();
            let empty = self.tr("No open sessions");
            if let Some(i) = picked_popup(ui, anchor, &rows, empty) {
                self.apply_cmd(ui, Cmd::PickToggle(i));
            }
        }
        // 多于一项才画 tab 条（用户原话：只有一个的时候不显示 tab）。
        //
        // 2026-11-17 视觉基线纠正：**条在内容区最下沿**，不是工具行下面。
        // 所以先扣掉条高定出正文下界，再画身份条的矩形；正文吃掉剩下的全部高度。
        let view_bottom = (full.top() + view_h).max(row.bottom() + BODY_GAP + 40.0);
        let mut body_bottom = view_bottom;
        if !self.sender.single() {
            let strip_rect = egui::Rect::from_min_max(
                egui::pos2(full.left(), view_bottom - TAB_STRIP_H),
                egui::pos2(full.right(), view_bottom),
            );
            let nums = self.sender.nums();
            if let Some(i) = tabs::strip(ui, strip_rect, &nums, self.sender.active()) {
                self.sender.select(i);
            }
            body_bottom = strip_rect.top();
        }
        let body = egui::Rect::from_min_max(
            egui::pos2(full.left(), row.bottom() + BODY_GAP),
            egui::pos2(full.right(), body_bottom),
        );
        self.sender_editor(ui, body);
        self.pump_sender(ui.ctx());
    }

    /// 真正的多行输入区：`frame(false)`，底色就是面板底，没有外框。
    fn sender_editor(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        // 先把「下一帧把光标挪到末尾」这个一次性请求取出来：后面借了
        // `state_mut().buf`，同一个 `sender` 不能再有第二次可变借用。
        let caret_end = std::mem::take(&mut self.sender.state_mut().caret_end);
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
        let rows = ((rect.height() / row_h).floor() as usize).clamp(2, 40);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        // 每页一份编辑状态：`id_salt` 换成页号，否则切页之后光标 / 滚动 /
        // undo 栈会跟着串到另一页的文本上。
        let salt = ("sender_editor", self.sender.active_no());
        let buf = &mut self.sender.state_mut().buf;
        let mut out = egui::TextEdit::multiline(buf)
            .id_salt(salt)
            .font(egui::TextStyle::Monospace)
            .text_color(crate::theme::chrome(CHROME_SCALE.text))
            // 用户点名的就是这一条：文字不许被框住。
            .frame(false)
            .desired_width((rect.width() - GUTTER_W).max(0.0))
            .desired_rows(rows)
            .margin(egui::Margin {
                left: GUTTER_W,
                right: 0.0,
                top: 0.0,
                bottom: 0.0,
            })
            .show(&mut child);

        // 行号：一个视觉行一段 y，只有「逻辑行首」那一行才编号——折行续行不编号。
        let p = child.painter();
        let font = crate::ui::type_scale::font(row_h * 0.82);
        let c = crate::theme::chrome(GUTTER);
        let mut n = 1usize;
        let mut at_start = true;
        for r in &out.galley.rows {
            if at_start {
                let g = p.layout_no_wrap(format!("{n}"), font.clone(), c);
                p.galley(
                    egui::pos2(
                        rect.left() + GUTTER_W - 6.0 - g.size().x,
                        out.galley_pos.y + r.rect.center().y - g.size().y / 2.0,
                    ),
                    g,
                    c,
                );
            }
            at_start = r.ends_with_newline;
            if at_start {
                n += 1;
            }
        }

        if caret_end {
            let len = out.galley.text().chars().count();
            let end = egui::text::CCursor::new(len);
            out.state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(end)));
            out.state.store(ui.ctx(), out.response.id);
            out.response.request_focus();
        }
    }

    /// 工具行的落地。**唯一**改 Sender 状态的地方。
    fn apply_cmd(&mut self, ui: &egui::Ui, cmd: Cmd) {
        self.sender.state_mut().note = None;
        match cmd {
            Cmd::Run => self.sender_start(),
            Cmd::Stop => self.sender.state_mut().run = None,
            // `＋` = 多开一页（编号 +1），`−` = 关掉当前那页，`×` = 清空当前页的
            // 文本。三颗按钮 2026-11-17 之前都是对文本行生效（加行 / 删行 / 清空），
            // 用户点名它们原本是管多 sender 页的，所以加行删行这两档去掉了。
            Cmd::NewTab => {
                self.sender.add();
            }
            Cmd::CloseTab => {
                // 只剩一页时是死键：`close_active` 自己拒绝，这里不用再判一次。
                self.sender.close_active();
            }
            Cmd::Clear => {
                let s = self.sender.state_mut();
                s.buf.clear();
                s.caret_end = true;
            }
            Cmd::Encoding(hex) => self.sender.state_mut().hex = hex,
            Cmd::By(char_mode) => self.sender.state_mut().by_char = char_mode,
            Cmd::Count(d) => {
                let s = self.sender.state_mut();
                s.count = (s.count as i32 + d).clamp(1, 999) as u32;
            }
            Cmd::Interval(d) => {
                let s = self.sender.state_mut();
                let v = s.interval + d as f32 * INTERVAL_STEP;
                s.interval = v.clamp(INTERVAL_MIN, INTERVAL_MAX);
            }
            Cmd::Target(t) => {
                let active_id = self.tabs.get(self.active_slot()).map(|t| t.identity);
                let s = self.sender.state_mut();
                s.target = t;
                // 首次切到「指定会话」时把当前标签预勾上：不预勾的话 `▶` 立刻
                // 变成一个什么都不发的键，用户得先猜到右边那格才是入口。
                if t == Target::Selected && s.picked.is_empty() {
                    if let Some(id) = active_id {
                        s.picked.push(id);
                    }
                }
                ui.memory_mut(|m| m.close_popup());
            }
            Cmd::OpenTargets => ui.memory_mut(|m| m.toggle_popup(targets_menu_id())),
            Cmd::OpenPicked => ui.memory_mut(|m| m.toggle_popup(picked_menu_id())),
            Cmd::PickToggle(i) => {
                // 弹层给的是**本帧的标签序号**，只在这一刻有效：换算成恒在身份
                // （`identity`）再存，标签关掉/重排都不会把勾选挪到别人头上。
                let id = self.tabs.get(i).map(|t| t.identity);
                if let Some(id) = id {
                    let s = self.sender.state_mut();
                    match s.picked.iter().position(|&x| x == id) {
                        Some(at) => {
                            s.picked.remove(at);
                        }
                        None => s.picked.push(id),
                    }
                }
            }
        }
        ui.ctx().request_repaint();
    }
}
