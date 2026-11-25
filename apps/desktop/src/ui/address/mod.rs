//! 地址栏（Address Bar）—— 投影层。
//!
//! # 这是什么
//!
//! 工具栏面板段（视觉基线里那条 `(i) ▸ 分组 ▸ 会话` 的灰条）点一下，会从它的
//! 左下角展开一个**连接目标输入框**：敲 `user@host:2222` 回车就连过去，或者从
//! 下拉里挑「已打开的标签 / 会话库里配好的会话 / 最近用过的地址」。
//! 逻辑判定全在 `term_app::address`（字符串层）与本模块的 [`plan`]（映射层），
//! 本文件只做三件事：**开合**、**绘制**、**把激活意图落到窗口上**。
//!
//! # 文本选择由 `TextEdit` 原生能力满足
//!
//! 光标、拖动选择、双击选词、`Ctrl/Cmd + A`、`Ctrl/Cmd + C/V` 全部是
//! `egui::TextEdit` 自带的。自己画一个输入框去对齐这些行为，只会得到一份
//! 「看起来能选、其实选不了」的假输入框——需求写了「不自己造轮子」，这是
//! 那条要求的实现方式：**用真的控件**。
//!
//! # 为什么是 `Area` 而不是 `Window`
//!
//! 它锚在工具栏面板段上（一个随时会变的矩形），没有标题栏、不该能被拖走、
//! 打开即抢键盘焦点。`Window` 那一套（标题 / 折叠 / 记忆位置）全是负担。
//! `Order::Foreground` 让它盖在一切面板与普通浮层之上——它是直接输入焦点。
//!
//! **唯一压不住的东西是危险操作的模态**：关闭确认 / 粘贴确认挂着的时候整段
//! 不画（见 `draw_address_bar` 第一个早退）。理由很实在——那两个弹窗问的是
//! 「要不要把会话关掉」，此刻在上面盖一个能敲字的框，用户按下的 Enter 就
//! 可能被两边同时认领。

mod plan;
mod rows;

// 单测单独一个文件：`plan.rs` 的纯函数要逐条钉住（名字里的 `:port`、去重、
// 三种激活意图），而 `mod.rs` 已经贴着 500 行上限。挂载写法照
// `dialogs/session_form.rs` 的 `#[path = ...]`。
#[cfg(test)]
#[path = "address_tests.rs"]
mod address_tests;
#[cfg(test)]
#[path = "focus_gate_tests.rs"]
mod focus_gate_tests;

use crate::ui::shell::{apply_popup_style, draw_menu_row, CheckCell, MenuRowState, RowLook};
use crate::ui::*;
use term_app::address::{
    move_selection, parse_target, push_recent, render_target, window_start, AddressKind,
    AddressState, Candidate, Target,
};
use term_app::i18n::Lang;
use term_core::theme::CHROME_SCALE;

use plan::Activation;

/// 没有锚点时（工具行被关掉 / 还没画过）的浮层宽度。
const FALLBACK_W: f32 = 460.0;
/// 锚点面板被拖得很窄时的下限：再窄连一个 `user@host:22` 都看不全。
const MIN_W: f32 = 320.0;
/// 下拉一屏最多画几行，超出的靠 `window_start` 滚动。
const VISIBLE_ROWS: usize = 10;
/// 输入框右端让给 `▾` 的宽度。
const ARROW_W: f32 = 22.0;
/// `▾` 菜单的宽度（三行中文 + 快捷键列都装得下）。
const MENU_W: f32 = 168.0;

/// 地址栏的投影态。
///
/// 逻辑状态（开没开 / 输入 / 选中）在 `term_app::address::AddressState` 里，
/// 这里补三样本层才需要的东西：
///
/// - `anchor`：打开时工具栏面板段的矩形。**每一次开都重取**，因为面板可以随
///   窗口宽度变长、也可以被拖走——上一帧的矩形这一帧就可能是错的；
/// - `problem`：上一帧的解析错误，画在输入框下面一行。不进 `ViewState`
///   （那是领域状态），也不弹 toast（地址栏自己的报错应该长在自己身上）；
/// - `recent`：`prefs.recent_targets` 的**渲染期副本**。每帧都要拿它拼候选，
///   而拼候选在一个帧循环里——不能每帧读一次 `prefs.json`（空闲 0 CPU
///   会当场变成空话）。打开那一刻读一次，之后只在本模块内增删。
pub(in crate::ui) struct AddressBar {
    /// 开没开 / 输入 / 选中下标（判定都在 `term_app::address` 里）。
    pub state: AddressState,
    /// 打开时的锚点（工具栏面板段左下角）。`None` = 窗口左上角兜底。
    pub anchor: Option<egui::Rect>,
    /// 上一帧的解析错误，画在输入框下方（红色一行），不弹 toast。
    pub problem: Option<String>,
    /// `prefs.recent_targets` 的渲染期副本（见结构体注释）。
    pub recent: Vec<String>,
}

impl AddressBar {
    pub(in crate::ui) fn new() -> Self {
        Self::default()
    }
}

impl Default for AddressBar {
    fn default() -> Self {
        Self {
            state: AddressState::new(),
            anchor: None,
            problem: None,
            recent: Vec::new(),
        }
    }
}

/// `▾` 菜单里选了什么。三项各自对应一条真动作（不画点了没反应的假条目）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuPick {
    /// 打开新建会话对话框（走既有的 `ToolAction::NewSession`，不自己拼表单）。
    NewSession,
    /// 打开第 n 条会话的属性面板（**只保存不连接**）。
    Properties(usize),
    /// 清空「最近使用」并落盘。
    ClearRecent,
}

/// 一帧里在浮层内部收集到的意图。
///
/// 为什么不在闭包里直接改窗口：绘制期间 `self.address.state.input` 被
/// `TextEdit` 可变借着，闭包里再借 `self` 的别的字段就打架。收集成值、画完
/// 再落地，是 `tool_bar.rs` 的 `TailWork` 同一套写法。
#[derive(Default)]
struct Outcome {
    /// 点了第几行。
    row: Option<usize>,
    /// 菜单选了哪一项。
    menu: Option<MenuPick>,
}

impl TerminalWindow {
    /// 打开地址栏。`prefill` 为空 = 打开即览全部候选（点工具栏那一下就该这样）。
    pub(in crate::ui) fn open_address_bar(&mut self, prefill: &str, anchor: Option<egui::Rect>) {
        // 所有入口的统一门：dispatch 的 Alt+D 与工具栏那一下都走这里，
        // 专注模式下 AddressState 连 open 位都不置（不只是「不画」）。
        if self.view.focus_mode {
            return;
        }
        self.address.state.open_with(prefill);
        self.address.anchor = anchor;
        self.address.problem = None;
        // 「最近使用」只在打开这一刻读盘：它每帧都要参与候选拼接，而帧循环里
        // 不许碰文件（见 `AddressBar::recent` 的注释）。
        self.address.recent = term_runtime::prefs::load()
            .map(|p| p.recent_targets)
            .unwrap_or_default();
    }

    /// 关掉地址栏。输入**保留**（`AddressState::close` 的既定语义）：连错端口
    /// 之后重开，用户多半要继续改同一个地址。
    pub(in crate::ui) fn close_address_bar(&mut self) {
        self.address.state.close();
        self.address.anchor = None;
        self.address.problem = None;
    }

    /// 画地址栏浮层（含键盘解释、下拉、`▾` 菜单、错误行）。
    ///
    /// 默认关闭时一行都不画——五张整窗视觉基线因此完全不受影响。
    pub(in crate::ui) fn draw_address_bar(&mut self, ctx: &egui::Context) {
        // 调用点已有 `!focus` 门控；这里再兜一层，避免将来新调用点漏掉时
        // 专注模式里凭空冒出输入框。
        if self.view.focus_mode || !self.address.state.open {
            return;
        }
        // 危险操作的模态优先（见模块文档末段）：它挂着的时候地址栏整段不画，
        // 于是「Enter 该归谁」永远不会成为一个问题。
        if self.confirm.pending.is_some() || self.pending_paste.is_some() {
            return;
        }

        let lang = self.lang();
        let list = plan::candidates(
            &self.open_tab_rows(),
            &self.sessions,
            &self.address.recent,
            &self.address.state.input,
            plan::CANDIDATE_CAP,
        );
        let total = list.len();

        // ── 键盘 ──────────────────────────────────────────────────────
        // 浮层里统一解释 ↑↓/Enter/Esc；终端区那条路有 `resp.has_focus()` 兜着，
        // 输入框拿着焦点时同一批键不会第二次发给 PTY（`terminal/pane.rs`）。
        let (up, down, enter, escape) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });

        if escape {
            self.close_address_bar();
            return;
        }
        // ↑↓ 在**夹紧后**的下标上移动：候选刚变少时旧下标可能越界，拿它去
        // `move_selection` 会得到「跳到末尾」这种莫名其妙的结果。
        let current = if total == 0 {
            0
        } else {
            self.address.state.selected.min(total - 1)
        };
        self.address.state.selected = if up && !down {
            move_selection(current, -1, total)
        } else if down && !up {
            move_selection(current, 1, total)
        } else {
            current
        };
        let selected = self.address.state.selected;
        if enter {
            // 输入能解析就**直接连**，优先级高于选中项。地址栏的
            // 主动作是「我敲了一个地址，回车」；下拉只是顺手的第二条路。
            match parse_target(&self.address.state.input) {
                Ok(target) => {
                    self.quick_connect(&target);
                    return;
                }
                Err(problem) => {
                    if let Some(candidate) = list.get(selected).cloned() {
                        self.activate_candidate(&candidate);
                        return;
                    }
                    self.address.problem = Some(plan::problem_text(lang, &problem));
                }
            }
        }

        // ── 几何 ──────────────────────────────────────────────────────
        let width = self
            .address
            .anchor
            .map(|rect| rect.width().max(MIN_W))
            .unwrap_or(FALLBACK_W);
        // 锚点左下角；没有锚点就用中央区左上角（工具行被关掉时的兜底）。
        let pos = match self.address.anchor {
            Some(rect) => rect.left_bottom(),
            None => ctx.available_rect().left_top(),
        };
        let properties = self.properties_index(&list, selected);
        let has_recent = !self.address.recent.is_empty();

        let mut outcome = Outcome::default();
        draw_popup(
            ctx,
            lang,
            &mut self.address.state,
            &mut self.address.problem,
            DrawInput {
                list: &list,
                selected,
                width,
                pos,
                properties,
                has_recent,
            },
            &mut outcome,
        );

        if let Some(index) = outcome.row {
            if let Some(candidate) = list.get(index).cloned() {
                self.activate_candidate(&candidate);
            }
        }
        match outcome.menu {
            Some(MenuPick::NewSession) => {
                self.close_address_bar();
                self.handle_tool_action(ToolAction::NewSession);
            }
            Some(MenuPick::Properties(index)) => {
                self.close_address_bar();
                self.open_session_properties_only(index);
            }
            Some(MenuPick::ClearRecent) => self.clear_recent_targets(),
            None => {}
        }
    }

    /// 已打开标签的（标题，目标串）。`Opened` 段只有投影层拿得到。
    fn open_tab_rows(&self) -> Vec<(String, String)> {
        self.tabs
            .iter()
            .map(|tab| {
                (
                    tab.config.name.clone(),
                    plan::config_target(&tab.config),
                )
            })
            .collect()
    }

    /// 「Session Properties…」这一行该打开哪条会话（`None` = 该置灰）。
    ///
    /// 选中项是 `Session` → 就是它；是 `Opened` → 按标签的**会话名**回查会话库
    /// （标签名下标与会话库下标是两个空间，不能互相借）；是 `Host` 或者会话
    /// 已经被删掉 → `None`，那一行画成 `Disabled`。
    ///
    /// 这里必须真的返回 `None` 而不是猜一个下标：猜错就是「点属性，改的是
    /// 另一条会话」，而那种事故在界面上完全看不出来。
    fn properties_index(&self, list: &[Candidate], selected: usize) -> Option<usize> {
        let candidate = list.get(selected)?;
        match candidate.kind {
            AddressKind::Session => candidate.index.filter(|i| *i < self.sessions.len()),
            AddressKind::Opened => {
                let name = self.tabs.get(candidate.index?)?.config.name.clone();
                self.sessions.iter().position(|cfg| cfg.name == name)
            }
            AddressKind::Host => None,
        }
    }

    /// 一条候选被激活（回车或点击）。
    fn activate_candidate(&mut self, candidate: &Candidate) {
        match plan::activation_for(candidate) {
            Some(Activation::SwitchTab(index)) => {
                if index < self.tabs.len() {
                    self.set_active_slot(index);
                    self.close_address_bar();
                }
            }
            Some(Activation::ConnectSession(index)) => {
                self.selected_session = index;
                self.connect_selected_session();
                self.close_address_bar();
            }
            Some(Activation::QuickConnect(target)) => self.quick_connect(&target),
            // 过期的那一行（会话被删 / 手改过的 prefs 里有一条坏地址）：
            // 什么都不做。拿下标去索引才是真事故。
            None => {}
        }
    }

    /// 「输入即连接」：记住这次的目标、关掉浮层、起连接。
    fn quick_connect(&mut self, target: &Target) {
        // 先记 MRU 再关浮层：关掉之后输入框就读不到了，目标串得在这里定下来。
        self.push_recent_target(&render_target(target));
        self.close_address_bar();
        self.connect_target(target);
    }

    /// 目标 → 连接。**瞬时**：不进会话库、不 `save_sessions()`。
    fn connect_target(&mut self, target: &Target) {
        let config = plan::config_for(target, &plan::name_hint(target));
        if crate::ui::panels::sidebar::uses_builtin_ssh(&config) {
            // SSH：握手是异步的（十几秒），必须先起后台线程再等结果回来建标签。
            self.begin_builtin_ssh(config, None);
            return;
        }
        let index = self.tabs.len();
        let tab = make_tab(
            &self.pool,
            config,
            egui::Id::new(format!("tab_{index}")),
            &self.ctx,
        );
        let index = self.add_tab(tab);
        self.activate_new_tab(index);
    }

    /// 把目标串推进「最近使用」并落盘（`push_recent` 负责去重 + 裁剪）。
    fn push_recent_target(&mut self, rendered: &str) {
        push_recent(&mut self.address.recent, rendered, plan::RECENT_CAP);
        self.save_recent_prefs();
    }

    /// 清空「最近使用」并落盘。
    ///
    /// `pub(in crate::ui)`：菜单命令 `Terminal.ClearRecentUrls` 也从这里落地
    /// （经 `Effect::ClearRecentUrls`，见 `ui/effects.rs`）。地址栏右键的
    /// 「Clear Recent」是第一个调用方——两条入口共用一次清理，避免出现
    /// 「菜单清了缓存但盘上还在，重开又冒出来」的半截实现。
    pub(in crate::ui) fn clear_recent_targets(&mut self) {
        self.address.recent.clear();
        self.save_recent_prefs();
    }

    /// 「最近使用」落盘：load-modify-save。
    ///
    /// 整份重建会把语言与其余几十项设置一起清掉（`prefs` 字段只增不减，这个
    /// 坑 `confirm.rs::save_confirm_prefs` 里记过一次）。语言顺带写回当前值：
    /// 与那份实现同一个口径，谁落盘都带上当前语言。
    fn save_recent_prefs(&self) {
        let mut prefs = term_runtime::prefs::load().unwrap_or_default();
        prefs.language = self.view.lang.tag().to_string();
        prefs.recent_targets = self.address.recent.clone();
        term_runtime::prefs::save(&prefs);
    }
}

/// 画浮层本体要的全部输入。攒成一个结构是因为参数已经六个——散在签名里，
/// 调用处会变成一串没法读的位置参数。
struct DrawInput<'a> {
    list: &'a [Candidate],
    selected: usize,
    width: f32,
    pos: egui::Pos2,
    /// 「Session Properties…」那行指向的会话下标（`None` = 置灰）。
    properties: Option<usize>,
    /// 「Clear Recent」那行能不能点（最近使用为空时置灰）。
    has_recent: bool,
}

/// 浮层：输入框 + `▾` + 下拉 + 错误行。
///
/// 是个自由函数而不是 `TerminalWindow` 的方法：它只需要一个
/// `&mut AddressState`、一条待显示的报错与一份候选表，拿到整个窗口反而会
/// 诱使后来的人在这里顺手改领域状态（守住边界）。
///
/// `problem` 是 `&mut`：用户一改动输入就把上一帧的报错擦掉——「刚刚那句
/// 报错说的是我改之前那半截地址」，留着它只会误导。
fn draw_popup(
    ctx: &egui::Context,
    lang: Lang,
    state: &mut AddressState,
    problem: &mut Option<String>,
    input: DrawInput<'_>,
    out: &mut Outcome,
) {
    let DrawInput {
        list,
        selected,
        width,
        pos,
        properties,
        has_recent,
    } = input;
    egui::Area::new(egui::Id::new("address_bar"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .pivot(egui::Align2::LEFT_TOP)
        // 与工具栏 / 浮动面板同一个坑：`Area` 默认淡入，而夹具把 `i.time` 钉在
        // 0，整块会停在淡入第一帧（看起来像没用设计令牌）。
        .fade_in(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(crate::theme::chrome(CHROME_SCALE.tab))
                .stroke(egui::Stroke::new(
                    1.0,
                    crate::theme::chrome(CHROME_SCALE.border),
                ))
                .show(ui, |ui| {
                    // `Area` 里的 `available_width` 是整屏宽，必须钉死。
                    ui.set_width(width);
                    draw_input_row(ui, lang, state, problem, has_recent, properties, out);
                    ui.add_space(2.0);
                    draw_list(ui, lang, list, selected, out);
                    if let Some(text) = problem.as_deref() {
                        // 与 `overlays/goto.rs` / `panels/tools_sender_row.rs` 的
                        // 既有错误红同值（本仓没有 `CHROME_SCALE` 的警示色令牌）。
                        ui.colored_label(egui::Color32::from_rgb(0xE0, 0x60, 0x60), text);
                    }
                });
        });
}

/// 输入行：无边框 `TextEdit`（`field` 底）+ 右端 `▾` 菜单。
fn draw_input_row(
    ui: &mut egui::Ui,
    lang: Lang,
    state: &mut AddressState,
    problem: &mut Option<String>,
    has_recent: bool,
    properties: Option<usize>,
    out: &mut Outcome,
) {
    egui::Frame::none()
        .fill(crate::theme::chrome(CHROME_SCALE.field))
        .inner_margin(egui::Margin::symmetric(4.0, 3.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let edit_w = (ui.available_width() - ARROW_W).max(80.0);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut state.input)
                        .frame(false)
                        .font(egui::TextStyle::Monospace.resolve(ui.style()))
                        .hint_text(t(lang, "Host, user@host, or pick a session"))
                        .desired_width(edit_w)
                        .min_size(egui::vec2(edit_w, 0.0)),
                );
                // 打开期间输入框始终持有焦点（照 `palette.rs` 的写法）：敲字即
                // 过滤，下拉跟着变。焦点已经是它时不会触发重绘。
                if !response.has_focus() {
                    response.request_focus();
                }
                if response.changed() {
                    // 输入变了：选中下标归零。候选集是输入的纯函数值，改一个字
                    // 就可能换一批，留着旧下标就是「回车连到了别的机器」。
                    // 同时擦掉上一帧的报错（它说的是改之前那半截地址）。
                    state.selected = 0;
                    *problem = None;
                }
                draw_menu_button(ui, lang, has_recent, properties, out);
            });
        });
}

/// `▾` 按钮与它的三行菜单。
fn draw_menu_button(
    ui: &mut egui::Ui,
    lang: Lang,
    has_recent: bool,
    properties: Option<usize>,
    out: &mut Outcome,
) {
    let button = egui::Button::new("▾")
        .frame(false)
        .min_size(egui::vec2(ARROW_W, 0.0));
    egui::menu::menu_custom_button(ui, button, |ui| {
        apply_popup_style(ui);
        ui.set_width(MENU_W);
        if menu_row(ui, t(lang, "New Session"), true) {
            out.menu = Some(MenuPick::NewSession);
            ui.close_menu();
        }
        if menu_row(ui, t(lang, "Session Properties"), properties.is_some()) {
            if let Some(index) = properties {
                out.menu = Some(MenuPick::Properties(index));
            }
            ui.close_menu();
        }
        if menu_row(ui, t(lang, "Clear Recent"), has_recent) {
            out.menu = Some(MenuPick::ClearRecent);
            ui.close_menu();
        }
    });
}

/// 菜单里的一行。`enabled == false` 时画成 `Disabled`——**点了没反应的空转
/// 开关**是明确禁止的，宁可让它看起来就不能点。
fn menu_row(ui: &mut egui::Ui, text: &str, enabled: bool) -> bool {
    let look = RowLook {
        state: if enabled {
            MenuRowState::Enabled
        } else {
            MenuRowState::Disabled
        },
        check: CheckCell::None,
        indent: false,
        cells: 0,
    };
    draw_menu_row(ui, text, None, "", look, false).clicked()
}

/// 候选列表：最多 [`VISIBLE_ROWS`] 行，超出靠 `window_start` 围绕选中项滚动。
fn draw_list(
    ui: &mut egui::Ui,
    lang: Lang,
    list: &[Candidate],
    selected: usize,
    out: &mut Outcome,
) {
    if list.is_empty() {
        ui.label(t(lang, "No matches"));
        return;
    }
    let start = window_start(list.len(), Some(selected), VISIBLE_ROWS);
    for (index, candidate) in list.iter().enumerate().skip(start).take(VISIBLE_ROWS) {
        if rows::draw_candidate_row(ui, lang, candidate, index == selected).clicked() {
            out.row = Some(index);
        }
    }
}
