//! 终端区：事实窗口的绘制、键鼠输入、滚轮与拖选。
//!
//! 从 `ui/mod.rs` 拆出（2026-11-05）；又把「构造 tab / 算事实窗口」
//! 搬到 `ui/terminal_setup.rs`（单文件 ≤ 450 行）。**只搬代码，不改行为**——
//! 五张视觉基线逐字节一致是这次拆分唯一的判据。

use crate::ui::*;
use super::tab_slot::{line_cache, selection_text_mut, selection_text_ref, tab_of, TabSlot};
use super::completion_popup;
use super::{bin, ensure_view, margin, select};
use super::pane_click_gate;
// 正文二选一与正文之后的叠层各自成文件（单文件 500 行上限）。
// 两个都是**自由函数**——`draw_tab` 手上握着借自 `self.tabs` 的 `&mut Tab`，
// 方法调用会同时要 `&mut self`（E0499），所以那边只收拆分好的借用。
use super::history_scope::system_of; use super::pane_body::paint_body;
use super::pane_overlays::{paint_overlays, OverlayToggles}; use super::pane_preview::{box_rect as preview_box_rect, paint as preview_paint, preview_rows as preview_rows_fn};
use super::pane_pointer::{pointer_selection, visible_separators, GridGeom}; use super::pane_scrollbar::{interact as scrollbar_interact, paint as scrollbar_paint, track_rect as scrollbar_track_rect, ScrollbarView};
use term_app::scrollbar::preview_range; use term_app::selection; use term_core::keys; use term_core::session_runtime::SessionId;

/// 内容绝对行 → 屏内下标；窗口外的行返回 `None`（那一行根本画不出来）。
///
/// 用**查表**而不是减法（`row - view_range().start()`）：折叠把行收起来之后，
/// 事实窗口里的行号**不再连续**（`lines[i].row != start + i`），减法会指到相邻的
/// 另一行；查找天然兼容「有洞」的窗口，命中即该行本身。
///
/// 窗口最多几十行，线性查找比维护一张反查表便宜，也不必养第二份真相——反查表
/// 一旦与 `lines` 漂移，症状是标记画到别的行上，比慢几十次比较难查得多。
///
/// 可见性放到 `pub(super)`，`pane_overlays.rs` 的两层高亮要按同一条口径
/// 把绝对行号换成屏内下标——它们与查找高亮必须**同一套**换算法，多一份实现就是
/// 多一处「折叠之后高亮落到相邻行」的机会。行为零变化。
pub(super) fn screen_index_of(image: &term_core::grid::ScreenImage, row: i64) -> Option<usize> {
    image.lines.iter().position(|l| l.row == row)
}

/// 网格行（`0` = 屏首）→ `image.lines` 里的下标；那一行被折叠收起时是 `None`。
///
/// **不加 `view_range().start()`**：`ViewRow` 的原点定义是「贴底时屏幕首行」，
/// 不是「本次请求窗口的首行」。回看时窗口整体平移（`start` 是负数），而光标仍然
/// 在它原来那个网格行上——把窗口起点加上去，光标会整块浮到 `scroll` 行之上。
/// 折叠之后更不能加：窗口为了补行向上多要了几行，`start` 变成 `-extra`。
///
/// 查表而不是减法，是因为折叠后窗口内的行号不连续（`lines[i].row != start + i`）：
/// 命中就是那一行，落空就是「这一行现在看不见」。调用方据此**不画**：光标行底色
/// 与光标都落在这个下标上，硬画只会把它浮在相邻行上，等于指着另一行说「光标在这」。
pub(super) fn screen_index_of_grid_row(image: &term_core::grid::ScreenImage, grid_row: usize) -> Option<usize> { screen_index_of(image, grid_row as i64) }

/// 用户真实按键落到终端会话的唯一漏斗：先发 P0 抢占信号，再把字节写进 PTY。
///
/// 调用方只在收集到**确实要发往本会话**的 `key_bytes` 时才调它。所以 UI 自己的
/// 焦点导航（Tab / 方向键切面板）和只读查看标签都不会被误算成终端输入抢占。
/// 独立成函数是为了让这条生产路径能被单测直接复现，而不是只测抢占信号本身。
pub(in crate::ui) fn send_user_key_bytes(tab: &Tab, key_bytes: &[u8]) {
    if key_bytes.is_empty() || tab.local_mode {
        return;
    }
    // ADR-0030 P0：用户按键是抢占信号（不进队列），此前排队的
    // 规则 / API 写许可作废并被计数留痕，键盘字节随后落地。
    crate::ui::trigger_intents::user_key_preempt(tab);
    tab.write(key_bytes);
}

impl TerminalWindow {
    /// 单窗格路径：与分屏之前完全同一份绘制逻辑（五张视觉基线靠它守住）。
    pub(in crate::ui) fn draw_terminal_area(&mut self, ui: &mut egui::Ui) {
        // ADR-0025：单窗格按**活动身份**画；无活动标签时用永不存在身份 0，
        // `tab_of` 取不到就落到「No active session」，与旧「下标 0 越界」同一观感。
        self.draw_terminal_area_for(ui, self.active.unwrap_or(0));
    }

    /// 按标签**身份**画一个窗格。分屏时每个叶子各调一次。
    ///
    /// 2026-11-15 起只剩一层薄壳：真正的绘制在 [`Self::draw_tab`]，好让底带的
    /// `Shell` 面板走**同一份**代码。分屏树（`layout/split.rs`）的叶子现在挂的是
    /// `SessionId`，直接原样传进来，不再经下标。
    pub(in crate::ui) fn draw_terminal_area_for(&mut self, ui: &mut egui::Ui, main_id: SessionId) {
        self.draw_tab(ui, TabSlot::Main(main_id));
    }

    /// 按**槽位**画一块终端：主区的某个标签页，或底带的 `Shell` 面板。
    ///
    /// 2026-11-15：`Shell` 面板原先在 `panels/tools.rs::draw_shell` 里是**第二套**
    /// 终端实现（自己开池、`layout_no_wrap` 画一屏、手画光标块、只认六个键），
    /// 于是面板里没有选区 / `Cmd+C` / 滚轮回看 / 鼠标报告 / hex / 折叠 / 查找
    /// 高亮 / 粘贴确认 / margin / 右键菜单。用户裁定面板保留、内容与主区标签页
    /// 一致——它只是一条**特殊的 session**（默认系统本地 shell）。所以两者共用
    /// 这一整段：输入、PTY 尺寸、选区、滚轮、粘贴、右键菜单、光标全都只有一份
    /// 实现。
    ///
    /// # 分叉点清单
    ///
    /// 每一处只回答一个问题：「这件事属于**标签页**，还是属于**这块区域**？」
    /// ① 取 tab；② 写 `active_tab`（只有标签页有这个概念）；③ 还焦点（两块各记
    /// 一份标志）；④ 视觉基线夹具（整窗一份，只给活动标签页）；⑤ 菜单派来的选区
    /// 命令（作用在活动标签页上）；⑥ 鼠标拖拽的临时格子（按槽位取 id）；⑦ 剪贴板
    /// 派生值（两块各记一份）；⑧ 字形缓存（两块各一份）；⑨ 粘贴落点与两份右键
    /// 菜单（按槽位落地）。
    pub(in crate::ui) fn draw_tab(&mut self, ui: &mut egui::Ui, slot: TabSlot) {
        let rect = ui.available_rect_before_wrap();
        // 终端区底色调色板色按窗口 alpha 画；`window_transparent=false`
        // 时 `window_bg_color()` 原样返回 `to_color32(default_bg)`，与旧代码
        // 逐字节相同（十一张视觉基线不受影响）。
        // 预览底色先算：`tab` 借走 self 后再调 window_bg_color() 会撞 E0502。
        let window_bg = self.window_bg_color(); ui.painter().rect_filled(rect, 0.0, window_bg);
        // `tab` 马上从 `self` 借走；鼠标事件掩码是 Copy 值，先拷出来再用。
        let mouse_events = self.mouse_events; let mut sync_bytes: Option<Vec<u8>> = None;

        // 同步输入要写「其它」标签，而那一刻 `tab` 还借在手上，
        // 所以先把这批按键攒下来，等借用结束后再扇出去。

        // 本帧因回车完成的命令行先攒着——`tab` 借在手上时不能碰
        // `self.command_history`，等借用结束后统一写入窗口级历史。
        // 粘贴（Cmd+V / 右键 / 中键三条路径最后都落到 `Event::Paste`）连同
        // 「此刻在哪一屏」一起攒着：门控要读 `snapshot.alt`，而落地要等 `tab`
        // 的借用结束（`commit_paste` / `pending_paste` 都要借整个窗口）。
        // Vim p/P 与折叠列点击也各攒一个意图：前者复用 Cmd+V 漏斗，后者要借整个窗口。
        let mut paste: Option<(String, bool)> = None; let mut completed_lines: Vec<String> = Vec::new();
        let mut vim_paste: Option<super::vim_apply::VimPaste> = None; let mut pending_fold: Option<term_app::vocab::FoldCmd> = None;
        // 录制器那一格先克隆出来：下面 `tab` 会把 `self` 整块借走。
        // 补全的候选源在借用标签之前取好——历史 / QuickBar 都挂在窗口上，
        // 与 `tab` 借的 `self.tabs` 是不同字段，先克隆出来后面就不必再碰窗口。
        // 总开关关着时这两张表保持空，一帧只多两次 Vec::new()。
        let completion_enabled = self.completion.enabled; let completion_scope = system_of(self);
        let (completion_history, completion_quick) = completion_popup::prepare_sources(
            completion_enabled,
            &self.command_history,
            &self.quick_bar,
            &completion_scope,
        );

        let recorder = std::sync::Arc::clone(&self.recorder);
        let Some(tab) = tab_of(&mut self.tabs, &mut self.shell_tabs, slot) else {
            ui.centered_and_justified(|ui| {
                ui.label(t(self.view.lang, "No active session."));
            });
            return;
        };
        // 本帧走哪一套渲染。**只决定怎么画**：输入、PTY 尺寸、键盘都照旧——
        // 显示模式不是输入模式，切到 Bin 之后敲键盘仍然要发到会话里去。
        let bin = tab.mode == term_runtime::prefs::DisplayMode::Bin;
        // 2026-11-19：只读查看（`Window ▸ Local Mode`）下不弹补全——补全预测的是
        // 「用户接下来要敲什么」，而这里一个字都发不出去。判据收在 `completion_on`
        // 一处，弹层、按键归属、重算候选三处都读它，不必各加一遍。
        let completion_on = completion_enabled && !bin && !tab.local_mode;
        let completion_owner = tab.id;
        // owner 的绑定在 recompute 的 state.update 里做；切标签后的第一帧
        // 若只有方向键，旧 owner 会让 is_visible 为 false，不会误接受候选。

        // Compute cols and rows based on available rect size and measured font metrics
        let cell_w = self.fonts.cell_w.max(4.0);
        let cell_h = self.fonts.cell_h.max(8.0);
        let avail_h = (rect.height() - GRID_PAD * 2.0).max(cell_h * 4.0);
        let rows = (avail_h / cell_h) as usize;
        let rows = rows.clamp(2, term_core::config::MAX_ROWS);
        // margin 先占位，再算列数。顺序反了就会出现「pty 以为最后 6 格能用、
        // 其实被边栏压住」——那 6 列写进去的内容用户永远看不到。
        let margin_state = self.view.margin;
        let last_line = tab.snapshot.history as i64 + rows as i64;
        let margin_met = margin::MarginMetrics::new(&margin_state, cell_w, last_line);
        let avail_w = (rect.width() - GRID_PAD * 2.0 - margin_met.width).max(cell_w * 10.0);
        let cols = (avail_w / cell_w) as usize;
        let cols = cols.clamp(2, term_core::config::MAX_COLS);
        // hex 的列数按窗宽夹一次，**只算这一次**：滚轮（字节视口）、复制、绘制
        // 三处各算一次的话，边界上必然对不上（滚动按 16 列、绘制按 24 列，
        // 于是滚一屏会跳几行）。`32 Columns` 配 `base 2` 一行近 2000px，
        // 不夹就会画到右边的 Diff / Session 面板上。
        //
        // 参数取自**活动标签自己的** `hex`（`View ▸ Hex *` 只改它，不落盘）：
        // 同一个窗口里两个标签可以各排各的。
        let hex = tab.hex.fitted(cell_w, avail_w);

        // 控制面内部按尺寸去重，每帧调用也安全。
        tab.resize(cols, rows);

        // Allocate interactive area for the terminal so it receives keyboard focus
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        if resp.clicked() || resp.gained_focus() {
            resp.request_focus();
            // 分屏时点哪个窗格，哪个就是活动标签（键盘随之过去）。
            // `Shell` 面板**不是**标签页（它不在 `tabs` 里），点它只拿键盘焦点。
            if let TabSlot::Main(_) = slot {
                self.active = Some(tab.identity);
            }
        }
        // 菜单「粘贴」先由 `Effect::Paste` 请框架去读剪贴板，结果下一帧
        // 才作为 `Event::Paste` 回到输入队列；而点菜单已经把焦点带走了。
        // 在同一帧把焦点还给终端，粘贴就能走下面那条「有焦点才收键」的既有路径，
        // 不必再开一个「不看焦点」的输入分支。
        //
        // 2026-11-15：主区只把焦点还给**活动**窗格；`Shell` 面板还给自己。
        // 两块各记一份（`restore_focus` / `restore_focus_shell`）——混成一份的话，
        // 在面板里中键粘贴一次，焦点就跑到主区去了。
        let focus_back = match slot {
            TabSlot::Main(_) => {
                self.active == Some(tab.identity) && std::mem::take(&mut self.restore_focus)
            }
            TabSlot::Shell(_) => std::mem::take(&mut self.restore_focus_shell),
        };
        if focus_back {
            resp.request_focus();
        }

        // 视觉基线夹具：行列已知后才能构造画面，所以在这里一次性结算。
        // 只结算在活动窗格上，分屏时不重复注入。2026-11-15：也只给
        // **标签页**——`Shell` 面板如果把它取走，整窗夹具就注入不进去了
        // （底带比中央区先画）。
        if matches!(slot, TabSlot::Main(_) if self.active == Some(tab.identity)) {
            if let Some(spec) = self.demo.take() {
                // 弹窗夹具：不替换终端画面，改为打开
                // New Session 对话框——截图验证弹窗皮肤与左页签条重排。
                if spec.open_new_session {
                    #[cfg(debug_assertions)]
                    if std::env::var_os("WINDTERM_DEMO_DEBUG").is_some() {
                        eprintln!("[demo] newsession branch: show_new_session = true");
                    }
                    self.view.show_new_session = true;
                    // 视觉基线定格在 SSH / Proxy；只写夹具输入缓冲，不动领域状态。
                    let (p, page) = crate::ui::dialogs::session_nav::GOLDEN_FIXTURE;
                    self.new_session.protocol = p;
                    self.new_session.port = p.default_port();
                    self.new_session.selected_page = page;
                    // 静态画面：关掉真实会话，避免 shell 输出继续刷屏。
                    tab.close();
                } else {
                    let fixture = crate::demo::build(spec, cols, rows);
                    // 夹具也只描述「按了哪些」，不直接写 ViewState：走的还是 reduce。
                    // 查找动作要读窗口，所以必须在 `screen` 被 move 进 tab 之前派发。
                    for action in fixture.find_actions() {
                        reduce(&mut self.view, action, Some(&fixture.screen));
                    }
                    // 2026-11-07：margin 也走同一条路——夹具不写状态，只派发命令。
                    for action in fixture.margin_actions() {
                        reduce(&mut self.view, action, Some(&fixture.screen));
                    }
                    // 默认布局铺满后夹具不再改外壳状态；通道留着给将来的外壳夹具。
                    // 行过滤是每条会话状态：夹具派发的 Action::Filter 经 reduce
                    // 变成 Effect::Filter，在这里落到活动 tab，不写 ViewState。
                    let mut filter_actions: Vec<term_app::vocab::FilterAction> = Vec::new();
                    for action in fixture.shell_actions() {
                        for effect in reduce(&mut self.view, action, Some(&fixture.screen)) {
                            if let term_app::action::Effect::Filter(a) = effect {
                                filter_actions.push(a);
                            }
                        }
                    }
                    for a in filter_actions {
                        term_app::filter::reduce(&mut tab.filter, a);
                    }
                    tab.filter_input = tab.filter.query.clone();
                    tab.snapshot = fixture.snapshot;
                    tab.screen = Some(fixture.screen);
                    tab.viewport = fixture.viewport;
                    // 夹具注进来的是一份**全新**集合：整份状态刚被替换掉，
                    // 旧选区与锁位都不该留（`set_active` 会认锁，这里要用构造）。
                    tab.selections = Selections::from_active(fixture.selection);
                    // hex 夹具：把「会话刚才收到的那一段」直接灌进环里。真实路径
                    // 上这些字节是 tap 搬进来的，夹具没有会话可挂，于是走同一个
                    // `BinBuffer::push`——渲染器看到的是同一种数据，不是另一条岔路。
                    if !fixture.bin.is_empty() {
                        tab.raw.ring.push(&fixture.bin);
                    }
                    tab.mode = fixture.mode;
                    // 软换行夹具与 mode 同类：显示开关直接写 tab，不经过 reducer。
                    tab.word_wrap = fixture.word_wrap;
                    // 彩虹括号夹具：开关写在 tab.marks（与设置页同一份状态）。
                    tab.marks.rainbow_brackets = fixture.rainbow_brackets;
                    // hover 预览夹具把切好的历史快照灌进 tab 的预览槽，
                    // 并冻定轨道归一化位置（反算成绝对 y，稍后覆盖 `preview_hover_y`）。
                    tab.preview_screen = fixture.preview_screen;
                    // 冻定的轨道归一化位置反算成绝对 y 挂在窗口上（跨帧持久），
                    // 供每帧的预览框点亮；普通夹具 / 生产路径恒 `None`。
                    self.demo_preview_hover_y = fixture.preview_hover.map(|ratio| {
                        let tr = scrollbar_track_rect(rect);
                        tr.min.y + ratio * tr.height()
                    });
                    // 静态画面：关掉真实会话，避免 shell 输出把夹具覆盖掉。
                    // （`close` 只摘 tap，环里的字节留着——基线画的就是它们。）
                    tab.close();
                }
            }
        }

        // 滚轮回看。上限来自内核报告的历史行数（快照里的 history），不自己猜。
        // xterm 惯例：开了鼠标报告就把滚轮交给程序，Shift 是本地手势的豁免键；
        // 两者二选一，避免一次滚轮既滚程序又滚本地回看。
        let shift = ui.input(|i| i.modifiers.shift);
        let wheel_to_program = !bin && tab.snapshot.mouse_reporting && !shift && resp.hovered();
        if resp.hovered() && !wheel_to_program {
            let dy = ui.input(|i| i.raw_scroll_delta.y);
            if dy.abs() > f32::EPSILON {
                // 每格滚轮滚动 `wheel_lines` 行（规格默认 1，与旧行为一致，零回归）。
                let wheel = tab.config.wheel_lines.max(1) as f32;
                let lines = ((dy / cell_h) * wheel).round() as i64;
                if lines != 0 {
                    if bin {
                        // Bin 下滚的是**字节环的视口**，不是网格回看：两套行号
                        // 之间没有换算（hex 一行 16 字节，网格一行若干字符）。
                        let view = bin::view_rows(avail_h, cell_h);
                        tab.raw.scroll_rows(lines, view, hex);
                    } else {
                        let max = tab.snapshot.history as i64;
                        if lines > 0 {
                            tab.viewport.scroll_up(lines, max);
                        } else {
                            tab.viewport.scroll_down(-lines, max);
                        }
                    }
                }
            }
        }
        // 历史可能被清屏 / resize 缩短，视口不能停在空白里。
        tab.viewport.clamp(tab.snapshot.history as i64);

        // 滚动条（Text 模式）命中区 + 拖拽 / 点击跳转 / 滚轮补位；同时把
        // 轨道 hover（非拖拽）的 y 交回这里，用于触发悬停预览。浮动叠加，不改
        // `avail_w` / `cols`；直接改 `tab.viewport`。bin 模式坐标不同，首版不做
        // （见设计稿 后续）。
        let mut preview_hover_y: Option<f32> = None;
        if !bin {
            let view = ScrollbarView {
                rows,
                history: tab.snapshot.history as i64,
                cell_h,
                wheel_lines: tab.config.wheel_lines.max(1) as i64,
            };
            preview_hover_y = scrollbar_interact(
                ui,
                egui::Id::new(("term_scrollbar", slot.index())),
                rect,
                view,
                &mut tab.viewport,
            );
            // demo 夹具冻定的 hover y 覆盖真实指针（生产路径恒 None）。
            // 跨帧持久在 `self.demo_preview_hover_y` 上——否则 `demo.take()` 之后
            // 首帧外的帧都没有框，基线截图截到末帧就丢框。
            if self.demo_preview_hover_y.is_some() {
                preview_hover_y = self.demo_preview_hover_y;
            }
            // 轨道 hover（非拖拽）→ 第二事实窗口（独立槽）去重请求；框的绘制在
            // `paint_overlays` 之后。预览不改变 `tab.view` / `tab.viewport` / `last_request`
            // （不变式），只有 `preview_last_request` 与 `preview_screen` 变。
            if let Some(hover_y) = preview_hover_y {
                let history = tab.snapshot.history.max(0);
                let tr = scrollbar_track_rect(rect);
                let ratio = ((hover_y - tr.min.y) as f64 / tr.height() as f64).clamp(0.0, 1.0);
                let range = preview_range(history, ratio, preview_rows_fn(rect, cell_h));
                // 去重：与 `ensure_view` 同款判据（`range` 不等或 `version` 变→
                // 重发），避免每帧上行一份不变的预览请求（浪费预览通道）。
                // `preview_last_request` 是独立键，不随 goto / fold 的 `last_request`
                // 复位走（ADR-0028）。
                let version = tab.snapshot.version;
                let stale = match &tab.preview_last_request {
                    Some((prev, v)) => prev != &range || *v != version,
                    None => true,
                };
                if stale {
                    tab.request_preview_range(range.clone());
                    tab.preview_last_request = Some((range, version));
                }
            }
        }

        // 本帧的事实窗口落地到 tab 上——渲染、查找、reducer 读同一份，
        // 「用户看到的那一屏」只有一个定义。
        let view = ensure_view(tab, rows);
        tab.view = view;
        // 菜单派人送来的选区操作在这里落地——只有这一处手上有本帧的事实窗口。
        // 只落在活动窗格。位移与集合命令共用 `apply_cmd`。
        // 2026-11-15：菜单命令一律作用在**活动标签页**上（`effects` 改的也是
        // 它），所以 `Shell` 面板不消费这个槽位——否则「在面板里点全选，主区的
        // 选区变了」。
        if matches!(slot, TabSlot::Main(_) if self.active == Some(tab.identity)) {
            select::dispatch_pending_select(&mut self.pending_select, tab, bin);
        }
        let image = &tab.view;

        let start_x = rect.min.x + GRID_PAD + margin_met.width;
        let start_y = rect.min.y + GRID_PAD;
        let cell = Vec2::new(cell_w, cell_h);
        // 本帧的窗格几何算一次、三处共用（手势 / 正文 / 叠层）。三份各写一遍
        // 字面量时，改一处漏两处就是「标记与字形错开半格」那一类像素回归。
        let geom = GridGeom { start: egui::pos2(start_x, start_y), cell, rows, cols };

        // 鼠标手势 → 选区。整套手势（按下 / 拖动 / 双击 / 三击 /
        // shift 扩选 / 拖出边缘自动滚动）只在 Text 下成立：hex 视图没有字符坐标，
        // 能选的是「第几个字节」，那是另一种交互，本轮不做。
        // 关掉的是**手势**，不是焦点——切到 Bin 之后键盘照常发 PTY。
        // 报告开启且未按 Shift 时鼠标归程序；Shift 豁免下本地选区照旧。
        if !bin && (shift || !tab.snapshot.mouse_reporting) {
            pointer_selection(
                &mut tab.selections,
                &mut tab.viewport,
                tab.snapshot.history as i64,
                &resp,
                ui,
                image,
                geom,
                visible_separators(&tab.config, tab.snapshot.alt),
                &term_app::selection::Pairs::parse(&tab.config.bracket_pairs),
                self.wrap_settings.with(tab.word_wrap),
                mouse_events.double_event,
            );
        }

        if !bin && !tab.local_mode {
            // 浮层 Id 取自标签身份 + 槽位，跨帧恒等；`ui.unique_id()` 随帧内控件数漂移。
            let pane_id = super::hover_anchor::stable_pane_id(slot.index());
            if let Some(hit) = super::ip_hover::paint(ui, pane_id, image, geom, tab.word_wrap, &self.triggers) { self.pending_ip_hit = Some(hit); }
            super::builtin_hover::paint(ui, pane_id, image, geom, tab.word_wrap);
        }
        // 折叠列的鼠标点击：无修饰切本块 / Ctrl 全部开关（命中判据在 margin.rs）。
        // 2026-11-25：`clicked()` 边沿在帧首就算好了，点外收起只删 release 挡不住它，
        // 所以原始边沿还要过 `pane_click_gate`——收起弹层那一击不许顺手翻折叠。
        if !bin && !tab.local_mode {
            let (click, mods) = ui.input(|i| (i.pointer.button_clicked(egui::PointerButton::Primary).then(|| i.pointer.press_origin()).flatten(), i.modifiers));
            let allowed = pane_click_gate::fold_click_allowed(ui, click.is_some());
            pending_fold = allowed.then(|| margin_met.fold_click_cmd(cell_w, rect.min, geom.start, geom.cell, image, &tab.folds, &tab.fold_inputs(), click, mods)).flatten();
        }

        // xterm 鼠标报告：档位/编码读快照，字节由 term-core 纯函数产出后写回会话。
        // CellMotion 的「跨格判重」把上一格存在本窗格的临时数据里（ui.data_mut），
        // 不改 Tab / mod.rs 的结构；Bin 没有字符坐标，整段不发。
        // 同折叠列：点外收起那一击的 Primary release 边沿仍在帧首快照里，
        // 不挡住就会往会话编码一个本不该有的 click（`pane_click_gate`）。
        if !bin && tab.snapshot.mouse_reporting && !shift && !tab.local_mode
            && pane_click_gate::mouse_encoding_allowed(ui)
        {
            let pos = resp.interact_pointer_pos().or_else(|| resp.hover_pos()).map(|p| (p.x, p.y));
            let mut frame = ui.input(|i| {
                use egui::PointerButton as B;
                term_core::mouse_report::PointerFrame {
                    x: pos.map_or(0.0, |q| q.0), y: pos.map_or(0.0, |q| q.1), has_pos: pos.is_some(),
                    pressed: [i.pointer.button_pressed(B::Primary), i.pointer.button_pressed(B::Middle), i.pointer.button_pressed(B::Secondary)],
                    released: [i.pointer.button_released(B::Primary), i.pointer.button_released(B::Middle), i.pointer.button_released(B::Secondary)],
                    down: [i.pointer.button_down(B::Primary), i.pointer.button_down(B::Middle), i.pointer.button_down(B::Secondary)],
                    dragged: resp.dragged(), hovered: resp.hovered(), scroll: i.raw_scroll_delta.y,
                    modifiers: term_core::mouse_report::Modifiers { shift: i.modifiers.shift, alt: i.modifiers.alt, ctrl: i.modifiers.ctrl },
                }
            });
            // 设置页的四档掩码只清被关掉的事件；远端 DECSET 仍决定编码档位。
            mouse_events.apply(&mut frame); let cfg = term_core::mouse_report::MouseConfig { mode: tab.snapshot.mouse_mode, encoding: tab.snapshot.mouse_encoding };
            // 这个绑定本来也叫 `geom`，与上面那个 `GridGeom` 撞名——块内
            // 遮蔽是对的（两套几何的单位不同，混用就是「鼠标报告错一格」），但读
            // 起来像同一个东西。改名只是去歧义，一个字的行为都没动。
            let metrics = term_core::mouse_report::GridMetrics { start_x, start_y, cell_w, cell_h, cols, rows };
            let drag_id = ui.id().with(("mouse_drag_cell", slot.index()));
            // 状态只在帧间存活；释放时清空，下一次按下重新记起点。
            let (report_bytes, next) = term_core::mouse_report::encode_frame(&cfg, &frame, &metrics, ui.data_mut(|d| d.get_temp::<(u16, u16)>(drag_id)));
            ui.data_mut(|d| if let Some(cell) = next { d.insert_temp(drag_id, cell); } else { let _ = d.remove_temp::<(u16, u16)>(drag_id); });
            if !report_bytes.is_empty() { tab.write(&report_bytes); }
        }

        // 复制取「看得见的部分」：事实窗口之外的行不参与（见 selection::text）。
        // 剪贴板派生值只跟活动窗格走，别被非活动窗格的选区覆盖。
        // 整批高亮都参与（`Selections::text`），不是只取活动那一份。
        //
        // 2026-11-15：按**槽位**分两份（`selection_text` / `selection_text_shell`）。
        // 行为一致优先——面板里也得能 `Cmd+C`，而窗口级只有一份字段时两块区域
        // 会互相覆盖「现在复制什么」。读它的其余地方（`effects` 的复制/搜索、
        // 工作台、右键菜单）一律读**本槽位**那一份。
        let writes_selection = match slot {
            TabSlot::Main(_) => self.active == Some(tab.identity),
            TabSlot::Shell(_) => true,
        };
        if writes_selection {
            // 「Cmd+C 会复制什么」在两种模式下是同一句话：用户**看得见**的东西。
            // Text 下是选区文本；Bin 下是这一屏 hex 文本（没有字符坐标可选，
            // 但复制整屏 dump 正是这个视图最常见的用途）。
            let text = if bin {
                bin::copy_text(&tab.raw, avail_h, cell_h, hex)
            } else {
                // `wraps` 一并传：屏幕上看不出来的硬折行不能变成剪贴板里的换行符
                // （见 `Selection::text`）。这里传的是整个快照的续行表，与下面
                // `marks_paint::paint` 读的是同一份。
                tab.selections.text(image, &tab.snapshot.wraps)
            };
            // `Terminal ▸ Mouse Actions ▸ When I select text, copy it to clipboard`。
            //
            // 这是**唯一**的读点：判据、去重与「什么时候不写」都在
            // `term_app::mouse::should_copy_on_select` 里（纯函数、可单测），
            // 这里只提供两件事实——开关、以及指针此刻是不是还按着。
            // 指针按着时不写，于是拖动中的中间态不会一路覆盖用户剪贴板，
            // 松手那一帧才落一次；同一段选区只写一次，改选另一段才再写。
            let previous = match slot {
                TabSlot::Main(_) => self.copied_on_select.as_deref(),
                TabSlot::Shell(_) => self.copied_on_select_shell.as_deref(),
            };
            let pointer_down = ui.input(|i| i.pointer.any_down());
            if term_app::mouse::should_copy_on_select(
                self.copy_on_select,
                &text,
                pointer_down,
                previous,
            ) {
                match slot {
                    TabSlot::Main(_) => self.copied_on_select = Some(text.clone()),
                    TabSlot::Shell(_) => self.copied_on_select_shell = Some(text.clone()),
                }
                // `Context` 是 `Send + Sync` 的轻量句柄，克隆一份再写，
                // 免得 `&self.ctx` 与上面的 `&mut self.copied_on_select_*` 撞借用。
                let ctx = self.ctx.clone();
                ctx.copy_text(text.clone());
            } else if text.is_empty() {
                // 选区被清掉：下一段选区必须重新写一次（否则「再选同一段」不生效）。
                match slot {
                    TabSlot::Main(_) => self.copied_on_select = None,
                    TabSlot::Shell(_) => self.copied_on_select_shell = None,
                }
            }
            *selection_text_mut(&mut self.selection_text, &mut self.selection_text_shell, slot) =
                text;
        }

        // Handle keyboard input when focused.
        // 浮层打开时它们自己解释 ↑↓/Enter/Esc，同一批键不许再发给 PTY。
        if resp.has_focus()
            && !self.view.command_palette.visible
            && !self.view.goto.visible
            && !self.view.center.visible
            && !self.view.show_shortcuts
            && !self.view.show_tmux && !self.view.lock.locked
            && self.pending_paste.is_none()
        {
            // 本地模式的 Vim 导航键：先攒字符，循环结束后喂给纯解析层。
            let mut key_bytes = Vec::new(); let mut vim_keys: Vec<char> = Vec::new(); let mut want_copy = false;
            // Shift+方向键是外壳的选择手势，不转发给程序（见 `ui::select`）。
            let mut want_select: Option<Op> = None;
            // 弹层可见时 ↑↓/Tab/Esc 归补全，不再写进 PTY。
            let mut completion_action: Option<completion_popup::Action> = None; let mut completion_blocked = false;
            ui.input(|i| {
                for event in &i.events {
                    match event {
                        // 用 `Event::Text` 而不是 `Event::Key`：macOS 的
                        // Option+字母只在 Text 里给出最终字符。编码口径在纯层。
                        egui::Event::Text(text) => {
                            // 只读查看态：键盘不写 PTY，交给 Vim 导航解析层。
                            if tab.local_mode { vim_keys.extend(text.chars()); continue; }
                            let b = keys::text_bytes_for_modifiers(text, i.modifiers.alt, i.modifiers.command, tab.config.alt_modifier_sends, tab.config.meta_modifier_sends).unwrap_or_else(|| text.as_bytes().to_vec());
                            key_bytes.extend_from_slice(&b);
                        }
                        egui::Event::Copy => want_copy = true,
                        egui::Event::Paste(text) => {
                            // 只读查看：连「确认弹窗」都不该弹——它问的是
                            // 「要不要把这段写进去」，而这里写不进去。
                            if !tab.local_mode {
                                paste = Some((text.clone(), tab.snapshot.alt));
                            }
                        }
                        egui::Event::Key { key, pressed: true, modifiers, .. } => {
                            if let Some((action, blocked, consume)) = completion_popup::action_for_key(
                                key,
                                self.completion_state.is_visible(completion_owner),
                                completion_on,
                            ) {
                                completion_action = Some(action);
                                completion_blocked |= blocked;
                                if consume { continue; }
                            }
                            if let Some(op) = select::key_to_op(*key, *modifiers) {
                                want_select = Some(op);
                                continue;
                            }
                            let bytes = match key {
                                egui::Key::Enter => b"\r"[..].into(),
                                egui::Key::Backspace => keys::backspace_bytes(tab.config.backspace_sends).as_bytes(),
                                egui::Key::Delete => keys::delete_bytes(tab.config.delete_sends).as_bytes(),
                                egui::Key::Tab => b"\t"[..].into(),
                                egui::Key::Escape => b"\x1b"[..].into(),
                                egui::Key::ArrowUp => arrow_bytes_for(Arrow::Up, tab.snapshot.application_cursor_keys).as_bytes(),
                                egui::Key::ArrowDown => arrow_bytes_for(Arrow::Down, tab.snapshot.application_cursor_keys).as_bytes(),
                                egui::Key::ArrowRight => arrow_bytes_for(Arrow::Right, tab.snapshot.application_cursor_keys).as_bytes(),
                                egui::Key::ArrowLeft => arrow_bytes_for(Arrow::Left, tab.snapshot.application_cursor_keys).as_bytes(),
                                _ => &[],
                            };
                            if !bytes.is_empty() {
                                key_bytes.extend_from_slice(bytes);
                            }
                        }
                        _ => {}
                    }
                }
            });

            // Vim 本地模式：纯解析 → 已有视口 / 选区 API，不产生任何 PTY 字节。
            vim_paste = super::vim_apply::drain(&mut tab.viewport, &tab.view, &tab.snapshot, &mut tab.selections, &tab.config, &mut tab.vim, vim_keys, ui.ctx());

            // Cmd+C：写剪贴板。egui 不提供读接口，粘贴走 Event::Paste（Cmd+V）。
            // 读哪一份取决于**这块区域**：面板里复制的就是面板里选中的那段。
            let copy_text =
                selection_text_ref(&self.selection_text, &self.selection_text_shell, slot);
            if want_copy && !copy_text.is_empty() {
                let text = copy_text.to_string();
                self.ctx.copy_text(text);
            }
            if let Some(op) = want_select {
                // 借用拆分：`image` 借的是 `tab.view`，这里只写 `tab.selections`。
                // 与菜单那条同一条路：Shift+方向键走到的四条 Command/Scope 命令
                // 也要块表，否则键盘与菜单会得到两种结果。
                selection::apply_cmd_with_pairs(
                    Cmd::Move(op),
                    &tab.view,
                    &tab.snapshot.blocks,
                    &mut tab.selections,
                    visible_separators(&tab.config, tab.snapshot.alt),
                    &term_app::selection::Pairs::parse(&tab.config.bracket_pairs),
                );
            }
            // 补全手势先落地。Accept 把后缀并进 key_bytes，和真实按键
            // 走同一条 write/feed 路径——录制器、同步输入也都一致。
            if let Some(action) = completion_action {
                completion_popup::apply_action(
                    action,
                    completion_owner,
                    &mut self.completion_state,
                    &mut key_bytes,
                );
            }
            // 2026-11-19：只读查看。`Tab::write` 自己也会拦（唯一漏斗），但这里
            // 还要拦**派生状态**——输入行缓冲、折叠用的输入行表、同步输入的广播。
            // 只拦 `write` 的话，只读标签上敲的字仍会进命令历史与折叠表，
            // 还会经 `broadcast_input` 漏给别的标签，那就不是只读了。
            if !key_bytes.is_empty() && !tab.local_mode {
                // ADR-0027：录制器只在这里收字节——设备应答与粘贴都不经过
                // 这段，所以录到的永远是「人敲的」。
                super::script::note_recorded_input(&recorder, &key_bytes);
                // 抢占 + 写 PTY 只有这一条路；焦点导航不会产生 key_bytes。
                send_user_key_bytes(tab, &key_bytes);
                // 按键字节喂进输入行缓冲，回车时吐出命令行。
                let before = completed_lines.len();
                completed_lines.extend(tab.input_line.feed(&key_bytes));
                // 折叠的输入行表：回车那一行就是刚敲完的命令行（见
                // `Tab::note_input_line`）。只在真的完成了一行时才记。
                if completed_lines.len() > before { tab.note_input_line(); if let Some(r) = tab.cursor_abs_row() { for line in &completed_lines[before..] { tab.note_command_text(r.get(), line.clone()); } } }
                if self.view.sync_input {
                    sync_bytes = Some(key_bytes.clone());
                }
            }
            // 输入变了就重算候选。Esc / 左右方向键那一帧跳过，否则
            // 刚 dismiss 又立刻被重算弹回来。回车后输入行清空，自然没有候选。
            if completion_on && !completion_blocked {
                completion_popup::recompute(
                    completion_owner,
                    &tab.input_line.current(),
                    &completion_history,
                    &completion_quick,
                    &self.completion,
                    now_secs(),
                    &mut self.completion_state,
                );
            }
        }

        // 逐行 span 合批 + 行级缓存。典型行 80 次 text() → 1–3 次。
        let painter = ui.painter();
        let font_id = self.fonts.font_id();

        // 2026-11-07：margin 先画。当前行的整行淡底必须压在 cell 背景**之下**，
        // 否则用户设的 ANSI 背景色会被边栏抢走；而边栏自己的字永远不会和正文
        // 重叠（正文从 `start_x` 开始）。
        let current_row = if tab.viewport.is_bottom() {
            Some(tab.snapshot.history as i64 + tab.snapshot.cursor.0 as i64)
        } else {
            None
        };
        // margin 的三列（行号 / 时间戳 / 提示符符号）都是**网格坐标**的产物：
        // hex 下既没有「第几行」，时间戳也无处对齐，所以整块跳过。
        if !bin {
            margin::paint(
                painter,
                &margin_state,
                &margin_met,
                rect,
                start_y,
                cell_h,
                image,
                &tab.snapshot,
                // 折叠列（`Margin Fold Column`）的判据：折叠头的绝对行来自同一个
                // `Tab.folds`，与 `setup::folded_view` 藏掉的行是同一份集合。
                &tab.folds,
                &font_id,
                current_row,
                self.wrap_settings.with(tab.word_wrap),
            );
        }

        // 正文二选一：`bin` 下画 hex 转储，否则画字符网格。起整段在
        // `pane_body.rs`——这里只留分叉点的名字与本帧的几何。
        //
        // 字形缓存那句 `line_cache(...)` **在这里**算好再传：让被调方自己按
        // `slot` 去取，就等于把 `&mut self` 交出去，与 `tab` 的借用当场撞死。
        paint_body(
            painter,
            line_cache(&mut self.term_view, &mut self.term_view_shell, slot),
            &*tab,
            self.view.lang,
            &self.palette,
            &font_id,
            rect,
            geom,
            hex,
            self.alt_row_bg,
            self.wrap_settings.with(tab.word_wrap), bin,
        );

        // 正文之后的全部叠层，五段一条都不许换序（顺序决定像素）：
        // 查找高亮 → 常驻词高亮 → 空白/符号标记 → 调试覆盖层 → 光标。
        // 调试覆盖层（`Mode ▸ Debug Mode`）夹在空白标记与光标之间：它压在
        // 半透明高亮与装饰标记之上，又必须让光标永远在最上层。
        // 回看药丸已于 2026-11-19 按用户比对去掉（见 pane_overlays.rs 模块头）。
        // 起整段在 `pane_overlays.rs`：它只往 `painter` 上追加图形、
        // 不写任何状态，所以能拆成只吃只读借用的自由函数。
        paint_overlays(
            painter,
            ui.ctx(),
            rect,
            &*tab,
            &self.view,
            &self.fonts,
            &self.palette,
            geom,
            bin,
            if matches!(slot, TabSlot::Main(_)) && self.active == Some(tab.identity) { Some(tab.identity) } else { None },
            OverlayToggles { cursor_typing: self.hide_cursor_typing, search: self.highlight_search }, self.wrap_settings.with(tab.word_wrap),
        );

        // 第 7 段 chrome——滚动条滑块（只读 `tab.viewport`，不改任何状态）。
        // bin 模式无滚动条（见 interact 同一处注释）。
        if !bin {
            scrollbar_paint(painter, rect, rows, tab.snapshot.history as i64, &tab.viewport);
        }

        // 第 8 段 chrome——悬停预览窗（在滚动条之后）。只读 `tab.preview_screen`，
        // 不写任何状态（不变式）。只有当轨道**真正 hover**（非拖拽）且预览槽已
        // 拿到帧时才画；`preview_hover_y` 一旦为 `None`（移开 / 拖拽 / 滚动条不可见）
        // 整块不画，框随之消失（设计稿 事实 5）。bin 模式无事实窗口，跳过。
        if !bin {
            if let (Some(hover_y), Some(image)) =
                (preview_hover_y, tab.preview_screen.as_ref())
            {
                let br = preview_box_rect(rect, cell_h, hover_y);
                // 预览复用真实终端的 margin / 快照 / 折叠集合，数字逐列同源；
                // 底色与隔行底色跟随配置（2026-11-19 用户比对修正）。
                preview_paint(painter, br, image, cell, &self.palette, &font_id, &margin_state, &margin_met, &tab.snapshot, &tab.folds, window_bg, self.alt_row_bg, self.wrap_settings.with(tab.word_wrap));
            }
        }

        // `tab` 的可变借用到这里结束，右键菜单 / 中键粘贴才不会
        // 与事实窗口的绘制抢同一个 `self`（E0499）。
        // 同步输入 + 折叠列点击都要借整个窗口，攒到这里一次性落地。
        self.flush_deferred(sync_bytes, pending_fold);
        // 待触发命中、Vim p/P 与 Cmd+V 三条路都在新模块里收口：窗口级
        // pending_trigger（pending_ip_hit）取走 → RunCommand 真派发 → 粘贴漏斗。
        super::vim_paste::finish_actions(self, slot, vim_paste.map(|v| (v.text, v.alt)), paste);
        // 把本帧回车完成的命令行写入窗口级历史（`tab` 借用已结束）。
        let history_scope = system_of(self);
        for line in completed_lines {
            self.command_history.push_for(&line, &history_scope, now_secs());
        }
        // 左边缘（margin 栏）是**另一份**右键菜单：只在 margin 可见时挂，注册在
        // 内容 resp 之后（重叠区压在上面 → 右键命中它弹 margin 菜单）。
        // 2026-11-15：两块区域的菜单各挂各的，`Id` 按槽位取（壳面板用哨兵下标）。
        if let Some(mresp) = super::pane_pointer::margin_context_resp(
            ui, rect, start_x, slot.index(), margin_met.width,
        ) {
            self.draw_margin_context(ui, &mresp, slot);
        }
        // 右键三档（菜单 / 粘贴 / 有选区就复制否则粘贴）：分派在
        // `context_menu.rs::handle_right_click`，菜单档仍是原来那次挂载。
        self.handle_right_click(ui, &resp, slot);
        self.handle_middle_paste(ui, &resp, slot);

        // 候选弹层画在最后一层，压在正文与光标之上。状态记录 owner，
        // 切到别的标签后这里直接是 false，不会把上一行的建议画到新会话里。
        completion_popup::paint_if_visible(ui, rect, &self.completion_state, completion_on, completion_owner, self.completion.list_height);
    }
}

/// 当前 Unix 纪元秒。`term-core` 不读钟，时钟由宿主注入
/// `CommandHistory::push`。
///
/// 可见性放到 `pub(super)`：`paste.rs` 也要用它算命令行时间（那边的
/// `commit_paste` 是粘贴的落地点），所以放到 `ui::terminal` 这一层，
/// 但不再往外散——窗口级的历史写入只走这两处。
pub(super) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
