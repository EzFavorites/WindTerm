//! Triggers 对话框：11 种触发器类型的 1:1 对齐。
//!
//! # 版式来源
//!
//! 窗口 768×562 逻辑点，骨架是「左列表 + 顶部动态类型标题 + Sessions 标签 +
//! 右内容区 + 两条底部工具条 + 页脚 Cancel/Ok」。
//!
//! # 标签的选中语义（易错点）
//!
//! 视觉基线里**选中的标签没有底色、未选中的标签填 `#333`**：
//! `01` 的内容是 Highlight Text 而该标签无底；`02` 的内容是 Sessions 而
//! Sessions 无底。按「选中=高亮」的直觉实现会整片反色。
//!
//! # 分层
//!
//! - 几何 / 文案 / 色板：`crate::ui::view::trigger_spec`（零 egui）
//! - 通用绘制件：本目录 `fields.rs`
//! - 页面：`panels.rs`
//! - 浮层：`dropdowns.rs`
//! - 弹窗：`popups.rs`（新建 / Highlight 设置）、`picker.rs`（取色器）

#[cfg(debug_assertions)]
mod demo;
mod dropdowns;
mod edit;
mod fields;
mod list;
pub(in crate::ui) mod model;
mod page_action;
mod page_custom_menu;
mod page_play_sound;
mod page_run_command;
mod page_tooltip;
mod panels;
mod picker;
mod popups;
mod ps_dialog;
mod strip;
mod style;
mod text;

#[cfg(test)]
#[path = "draft_tests.rs"]
mod draft_tests;

use crate::ui::*;
use crate::ui::dialogs::triggers::model::{Rule, UiAction};
use crate::ui::dialogs::triggers::model::{RuleConfig, TriggerKind};

/// 当前展开的浮层（同一时刻只可能有一个）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum Open {
    /// Action 下拉（Run command / Play sound / Open hyperlink / Send text）。
    Action,
    /// `Mark Style` / `Line Style` 下拉。
    LineStyle,
    /// `Mark Color` 下拉。
    MarkColor,
    /// Highlight 设置弹窗里的 Text Color。
    TextColor,
    /// Highlight 设置弹窗里的 Background Color。
    BgColor,
    /// Highlight 设置弹窗里的 Line Color。
    LineColor,
    /// `Send Condition` 下拉。
    SendCondition,
    /// `Captures` 下拉。
    Captures,
    /// `Play sound` 分支的 `Sound Name` 声音库（视觉基线 `22`，可滚动）。
    SoundName,
    /// `Custom menu` 工具条 `+` 的下拉：`Add Action` / `Add Separator`（图 `25`）。
    AddMenu,
}

impl Open {
    /// 该浮层是否属于 Highlight 设置弹窗（弹窗关闭时要一并收起）。
    fn in_highlight(self) -> bool {
        matches!(self, Open::LineStyle | Open::TextColor | Open::BgColor | Open::LineColor)
    }
}

/// 取色器的运行时状态（关掉即丢）。
#[derive(Clone, Debug)]
pub(in crate::ui) struct Picker {
    /// 当前 HSV。
    pub h: f32,
    pub s: f32,
    pub v: f32,
    /// 文本框草稿（HEX / RGB / HSV / Alpha）。
    pub hex: String,
    pub rgb: [String; 3],
    pub hsv: [String; 3],
    pub alpha: String,
    /// 自定义色板（Add to Custom Colors 的对象）。
    pub custom: Vec<u32>,
}

impl Default for Picker {
    fn default() -> Self {
        Self {
            h: 0.0,
            s: 255.0,
            v: 255.0,
            hex: "#ff0000".to_owned(),
            rgb: ["255".to_owned(), "0".to_owned(), "0".to_owned()],
            hsv: ["0".to_owned(), "255".to_owned(), "255".to_owned()],
            alpha: "255".to_owned(),
            custom: vec![0xffffff; 16],
        }
    }
}

impl Picker {
    /// 从规则当前颜色打开取色器（视觉基线 `06` 数值区是白色 `#ffffff`）。
    ///
    /// ANSI 色按标签在色表里查 hex；自定义色直接解析；`Empty` 没有明确色值，
    /// 取视觉基线默认白色。这样生产路径不再「改什么色都从红色重来」。
    pub(in crate::ui) fn from_choice(choice: &crate::ui::dialogs::triggers::model::ColorChoice) -> Self {
        use crate::ui::view::trigger_spec::color as c;
        let hex = match choice {
            crate::ui::dialogs::triggers::model::ColorChoice::Custom(s) => {
                u32::from_str_radix(s.trim().trim_start_matches('#'), 16).unwrap_or(0xffffff)
            }
            crate::ui::dialogs::triggers::model::ColorChoice::Ansi(label) => c::ANSI_LABELS
                .iter()
                .position(|l| *l == label)
                .map(|i| c::ANSI[i])
                .unwrap_or(0xffffff),
            crate::ui::dialogs::triggers::model::ColorChoice::Empty => 0xffffff,
        };
        picker::from_hex(hex)
    }
}

/// 取色器在给哪个槽位选色，以及选完之后的回填目标。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum ColorSlot {
    /// 主编辑面板的 `Mark Color`。
    Mark,
    /// Highlight 弹窗的 Text Color。
    Text,
    /// Highlight 弹窗的 Background Color。
    Bg,
    /// Highlight 弹窗的 Line Color。
    Line,
}

/// 本页的即时状态。放 egui 临时态：关窗即丢，不跨启动保留。
///
/// 注意它**不是**领域状态：`Rule` 才是。这里只装「此刻打开了哪个下拉 /
/// 哪个弹窗 / 列表选中第几条」这类投影态（ADR-0021 §3）。
#[derive(Clone, Debug)]
pub(in crate::ui) struct View {
    /// 0 = 类型页（标题随类型动态变化），1 = Sessions。
    pub tab: usize,
    /// 左列表选中的 `win.triggers` 下标。
    pub sel: Option<usize>,
    /// `Custom menu` 的 `Actions` 表里选中的行下标。
    ///
    /// 与 `sel`（左栏的触发器对象）是两个层次：一张表里有多行动作，选中的是
    /// 「当前要删 / 要移的那一行」。
    pub menu_sel: Option<usize>,
    /// 当前类型（无选中项时右栏按它渲染）。
    pub kind: TriggerKind,
    /// 展开中的浮层。
    pub open: Option<Open>,
    /// 展开中浮层的锚点矩形（决定菜单落在哪里）。
    pub anchor: Option<egui::Rect>,
    /// 当前选中的 OS 选项卡（0 macOS / 1 Linux / 2 Windows）。
    pub os: usize,
    /// Send 区当前是 Text 还是 Script。
    pub script: bool,
    /// 改名输入行是否开着。
    pub renaming: bool,
    /// 改名草稿。
    pub rename_buf: String,
    /// 新建触发器弹窗是否打开。
    pub new_dialog: bool,
    pub new_name: String,
    pub new_desc: String,
    pub new_kind: usize,
    /// Highlight 设置弹窗是否打开。
    pub hl_dialog: bool,
    /// `Play sound` 的修改弹窗是否打开（视觉基线 `29`）。
    pub ps_dialog: bool,
    /// `Play sound` 弹窗打开时该条规则的指纹（`OK` 可用性判据）。
    pub ps_base: Option<u64>,
    /// 弹窗打开时那条规则的指纹（判定 `OK` 是否可用；`None` = 本帧取基线）。
    pub hl_base: Option<u64>,
    /// 取色器。
    pub picker: Option<Picker>,
    /// 取色器在给哪个槽位选色。
    pub slot: ColorSlot,
    /// 取色器 `OK` 之后待写回的色值（由主循环消费）。
    pub pending_color: Option<crate::ui::dialogs::triggers::model::ColorChoice>,
    /// 新建弹窗的 `Type` 列表是否展开。
    pub type_menu: bool,
    /// 打开对话框那一刻的已提交规则快照。用来识别「关窗后重新打开」——
    /// 那一刻必须重新克隆草稿，不能把上次取消掉的编辑带回来。
    pub committed: Option<Vec<Rule>>,
    /// 编辑草稿。`show` 的每一帧临时把它换成 `win.triggers`，帧末换回来：
    /// 只有 Ok 才把草稿写进 prefs 并 `reload_triggers`，Cancel / Esc / 关窗
    /// 一律丢弃。过去直接改 `win.triggers`，Cancel 之后运行期规则已经变了。
    pub draft: Option<Vec<Rule>>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            tab: 0,
            sel: None,
            menu_sel: None,
            kind: TriggerKind::HighlightText,
            open: None,
            anchor: None,
            os: 0,
            script: false,
            renaming: false,
            rename_buf: String::new(),
            new_dialog: false,
            new_name: String::new(),
            new_desc: String::new(),
            new_kind: 6,
            hl_dialog: false,
            ps_dialog: false,
            ps_base: None,
            hl_base: None,
            picker: None,
            slot: ColorSlot::Mark,
            pending_color: None,
            type_menu: false,
            committed: None,
            draft: None,
        }
    }
}

impl View {
    pub(in crate::ui) fn load(ctx: &egui::Context) -> Self {
        ctx.data(|d| d.get_temp::<Self>(egui::Id::new("trig_view")).unwrap_or_default())
    }

    pub(in crate::ui) fn store(&self, ctx: &egui::Context) {
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("trig_view"), self.clone()));
    }

    /// 打开 / 收起某个浮层，并记住锚点。
    pub(in crate::ui) fn toggle_at(&mut self, which: Open, anchor: egui::Rect) {
        if self.open == Some(which) {
            self.open = None;
            self.anchor = None;
        } else {
            self.open = Some(which);
            self.anchor = Some(anchor);
        }
    }

    /// 收起浮层。
    pub(in crate::ui) fn dismiss(&mut self) {
        self.open = None;
        self.anchor = None;
    }
}

/// 生成一个不与现有规则冲突的 id。
fn new_rule_id(win: &TerminalWindow) -> String {
    let mut n = win.triggers.len() + 1;
    loop {
        let candidate = format!("rule_{n}");
        if !win.triggers.iter().any(|r| r.id == candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// 一条空白规则。
fn blank_rule(win: &TerminalWindow, kind: TriggerKind, name: &str) -> Rule {
    Rule {
        id: new_rule_id(win),
        name: name.to_owned(),
        description: String::new(),
        kind,
        config: RuleConfig::default(),
        on: kind.trigger_on(),
        pattern: String::new(),
        literal: true,
        case_sensitive: true,
        cooldown_secs: 0,
        idle_secs: 0,
        actions: Vec::new(),
        enabled: true,
    }
}

/// 切换类型时同步事件通道与求值动作（数据层的两处派生字段）。
pub(in crate::ui) fn set_kind(rule: &mut Rule, kind: TriggerKind) {
    rule.kind = kind;
    rule.on = kind.trigger_on();
    sync_actions(rule);
}

/// 把编辑配置投影成求值内核认识的 `actions`。
///
/// 求值器只认 `UiAction`，编辑面板改的是 `config`；两者必须同步，
/// 否则「面板上改了发送文本，跑起来还是旧内容」。
///
/// # 四种 `Action` 取值各自的去向
///
/// `Send text` → `SendText`；`Run command` → `RunCommand`（走既有命令通道）；
/// `Play sound` → `Beep`（本层能做到的真实副作用，见 `session.rs` 的
/// `apply_trigger`）。`Open hyperlink` **目前没有执行通道**（要调平台 opener，
/// 的待办里有记录。
pub(in crate::ui) fn sync_actions(rule: &mut Rule) {
    use crate::ui::dialogs::triggers::model::ActionKind;
    let action_of = |kind: ActionKind, cfg: &RuleConfig, text: &str| match kind {
        ActionKind::SendText => vec![UiAction::SendText(text.to_owned())],
        ActionKind::PlaySound => vec![UiAction::Beep],
        ActionKind::OpenHyperlink => Vec::new(),
        ActionKind::RunCommand => {
            vec![UiAction::RunCommand(cfg.action.os[0].command.clone())]
        }
    };
    rule.actions = match rule.kind {
        // 命中的行要被标出来：菜单 / 链接 / 高亮都靠它锚定。
        TriggerKind::HighlightText | TriggerKind::CustomLink | TriggerKind::CustomMenu => {
            vec![UiAction::Highlight]
        }
        TriggerKind::PlaySound => vec![UiAction::Beep],
        TriggerKind::SendText => vec![UiAction::SendText(rule.config.send.text.clone())],
        // `Show tooltip` 的提示文本只落在本地视图，**不能**当发送文本写进终端
        // （旧实现就是这么干的，选一次工具提示等于往远端敲一段 HTML）。
        TriggerKind::ShowTooltip => vec![UiAction::Highlight],
        TriggerKind::RunCommand => {
            vec![UiAction::RunCommand(rule.config.action.os[0].command.clone())]
        }
        TriggerKind::AfterConnect
        | TriggerKind::AfterDisconnect
        | TriggerKind::BeforeConnect
        | TriggerKind::BeforeDisconnect => {
            let cfg = &rule.config;
            action_of(cfg.action.kind, cfg, &cfg.send.text)
        }
    };
}

/// 画整窗内容（`o` = 对话框左上角，窗口坐标）。
///
/// 返回 `true` 表示要关闭对话框。
pub(in crate::ui) fn show(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    o: egui::Pos2,
) -> bool {
    let mut view = View::load(ui.ctx());

    // 这一帧的所有编辑都发生在草稿上：临时把草稿塞进 `win.triggers`，
    // 让原有绘制代码一个字不用改；帧末按 Ok / Cancel 决定保留还是丢弃。
    let mut draft = begin_draft(&mut view, &win.triggers);
    std::mem::swap(&mut win.triggers, &mut draft);

    // 选中项可能被删：先校正。左列表**不按页过滤**——视觉基线 `13` 里
    // 事件类（计时器图标）、Custom link、Highlight 三类同列在一个列表中。
    if view.sel.is_some_and(|i| i >= win.triggers.len()) {
        view.sel = None;
    }
    // 有选中项时，右栏类型跟随它；否则用 view.kind。
    if let Some(i) = view.sel {
        view.kind = win.triggers[i].kind;
    }
    // 列表非空但未选中 → 默认选第一条（兼容终端打开就有内容）。
    if view.sel.is_none() {
        view.sel = if win.triggers.is_empty() { None } else { Some(0) };
        if let Some(i) = view.sel {
            view.kind = win.triggers[i].kind;
        }
    }

    let mut act = Actions::default();

    // 仅调试：直接把某个浮层 / 弹窗摆出来截图核对（`WINDTERM_TRIG_OPEN`）。
    #[cfg(debug_assertions)]
    demo::preset(&mut view, ui.ctx(), o);

    list::left_pane(win, ui, o, &mut view);
    list::tab_bar(ui, o, &mut view);
    list::label_row(ui, o, &mut view, win);
    panels::body(win, ui, o, &mut view);
    strip::strips(win, ui, o, &mut view, &mut act);
    strip::footer(ui, o, &view, &mut act);

    if view.renaming {
        strip::rename_row(win, ui, o, &mut view);
    }

    // 浮层最后画，保证盖在内容之上。
    dropdowns::show(win, ui, &mut view);
    popups::show(win, ui, o, &mut view, &mut act);
    picker::show(ui, o, &mut view);
    // 弹窗关掉后基线作废：下次打开重新取，否则「改完关掉再开」会带着旧基线。
    if !view.hl_dialog {
        view.hl_base = None;
    }
    if !view.ps_dialog {
        view.ps_base = None;
    }

    let close = act.close;
    let saved = act.save;
    apply(win, &mut view, act);
    // Ok：`apply` 已把草稿写回 prefs 并 reload，`win.triggers` 就是新的已提交
    // 版本——丢掉换出来的旧版本。Cancel / 关窗：换回已提交版本，草稿作废。
    finish_draft(&mut view, draft, &mut win.triggers, saved, close);
    view.store(ui.ctx());
    close
}

/// 帧首：取出这份对话框的草稿（必要时重新克隆已提交规则）。
pub(in crate::ui) fn begin_draft(view: &mut View, committed: &[Rule]) -> Vec<Rule> {
    if view.draft.is_none() || view.committed.as_deref() != Some(committed) {
        view.committed = Some(committed.to_vec());
        view.draft = Some(committed.to_vec());
    }
    view.draft.take().unwrap_or_default()
}

/// 帧末：`saved` 时草稿已经被 `apply` 提交；其余路径一律把已提交版本换回来。
pub(in crate::ui) fn finish_draft(
    view: &mut View,
    mut draft: Vec<Rule>,
    live: &mut Vec<Rule>,
    saved: bool,
    close: bool,
) {
    if saved {
        view.committed = Some(live.clone());
        view.draft = None;
    } else {
        // swap 之后：`live` 是已提交版本，`draft` 是这一帧编辑过的草稿。
        std::mem::swap(live, &mut draft);
        view.committed = Some(live.clone());
        view.draft = if close { None } else { Some(draft) };
    }
}

/// 一次绘制里累积的结构性动作，绘制结束后统一落地（避免边遍历边改）。
#[derive(Default)]
pub(in crate::ui) struct Actions {
    pub add: bool,
    pub del: bool,
    pub duplicate: bool,
    pub remove_all: bool,
    pub save: bool,
    pub close: bool,
    pub modify: bool,
    /// 新建弹窗确认后的 (名字, 描述, 类型下标)。
    pub create: Option<(String, String, usize)>,
}

fn apply(win: &mut TerminalWindow, view: &mut View, mut act: Actions) {
    // 取色器结果优先落地（它可能刚落在一个已选中的规则上）。
    if let Some(choice) = view.pending_color.take() {
        if let Some(i) = view.sel.filter(|i| *i < win.triggers.len()) {
            let m = &mut win.triggers[i].config.mark;
            match view.slot {
                ColorSlot::Mark => m.mark_color = choice,
                ColorSlot::Text => m.text_color = choice,
                ColorSlot::Bg => m.background_color = choice,
                ColorSlot::Line => m.line_color = choice,
            }
        }
    }
    if act.create.is_some() {
        act.add = false;
    }
    if act.add {
        let kind = view.kind;
        let n = win.triggers.iter().filter(|r| r.kind == kind).count() + 1;
        win.triggers
            .push(blank_rule(win, kind, &format!("Trigger {n}")));
        view.sel = Some(win.triggers.len() - 1);
    }
    if let Some((name, desc, ki)) = act.create.take() {
        let kind = TriggerKind::ALL[ki.min(10)];
        let mut r = blank_rule(win, kind, &name);
        r.description = desc;
        win.triggers.push(r);
        view.tab = 0;
        view.sel = Some(win.triggers.len() - 1);
        view.kind = kind;
    }
    if act.del {
        if let Some(i) = view.sel {
            if i < win.triggers.len() {
                win.triggers.remove(i);
                view.sel = None;
            }
        }
    }
    if act.remove_all {
        // 只清当前类型的行：`Rules` 表按类型过滤，清全部会把别的类型的规则一起删。
        win.triggers.retain(|r| r.kind != view.kind);
        view.sel = None;
    }
    if act.duplicate {
        if let Some(i) = view.sel.filter(|i| *i < win.triggers.len()) {
            let mut copy = win.triggers[i].clone();
            copy.id = new_rule_id(win);
            copy.name = format!("{} copy", list::display_name(&win.triggers[i], i));
            win.triggers.push(copy);
            view.sel = Some(win.triggers.len() - 1);
        }
    }
    if act.modify {
        // `Modify` 在两类「表」页面里各自打开自己的弹窗：Highlight 是图 `11` 的
        // 样式弹窗，Play sound 是图 `29` 的声音弹窗。
        match view.kind {
            TriggerKind::PlaySound => view.ps_dialog = true,
            _ => view.hl_dialog = true,
        }
    }
    if act.save {
        // 11 种类型的编辑投影在此展开成正交规则，落盘的唯一真相是 automation::Rule。
        // 无效模式不能被空模式顶替：那会让这条规则匹配每一行。宁可整批不落盘，
        // 并把原因留在状态栏，也不写一条语义已经变形的规则。
        let snapshot: Result<Vec<term_core::automation::Rule>, String> =
            win.triggers.iter().map(|r| r.to_core()).collect();
        match snapshot {
            Ok(snapshot) => {
                super::settings_live::save_prefs(|p| p.triggers = snapshot);
                win.reload_triggers();
            }
            Err(msg) => win.log_note = Some(format!("触发器未保存：{msg}")),
        }
    }
    if act.close {
        win.view.show_triggers = false;
        win.ctx.request_repaint();
    }
}

