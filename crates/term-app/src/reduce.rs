//! 分发层：`Action` → 各能力的 reducer → `Effect`。
//!
//! 这是整个 crate 唯一**知道全部能力**的地方，所以它只能待在栈顶。
//! 2026-11-12 之前它和类型词表、字符串入口挤在同一个 `action.rs` 里，
//! 结果是 `action` 同时向 13 个能力模块射出依赖，而其中 7 个又反向依赖
//! `action` 的 `Effect`，形成 13 个节点的强连通分量。
//! 现在类型在 `vocab` / `effect`，入口在 `action`，分发在这里——单向。
//!
//! 规矩不变：本函数不能碰 PTY / 窗口 / 文件系统，副作用只以 `Effect` 值返回。

use crate::vocab::Action;
use crate::center::{self as center_feature};
use crate::clipboard::{self as clipboard_feature};
use crate::dock::{self};
use crate::effect::Effect;
use crate::find::{self as find_feature};
use crate::goto::{self as goto_feature};
use crate::log::{self as log_feature};
use crate::lock::{self as lock_feature};
use crate::margin::{self as margin_feature};
use crate::palette::{self as palette_feature};
use crate::shell::{self as shell_feature};
use crate::state::ViewState;
use term_core::grid::ScreenImage;
use crate::vocab::{CenterAction, CenterPage, Cmd, PaletteAction};

/// 纯函数：给定状态、动作与只读事实窗口，返回待执行副作用。
///
/// `window` 只读，是「用户此刻看得见的那一屏」——Find 的匹配集合由它推导
/// （F2：贴底是当前屏，回看就是历史）。传 `None` 等价于「当前没有活动标签页」，
/// 各 reducer 必须能处理这种情况。
pub fn reduce(state: &mut ViewState, action: Action, window: Option<&ScreenImage>) -> Vec<Effect> {
    match action {
        Action::Shell(a) => shell_feature::reduce(state, a),
        Action::Find(a) => match a {
            // R14-b：以选区为查询词的查找——reducer 打开栏并复位，把「填选区 + 跳一步」
            // 留给投影层（查询词来自 `selection_text`，本层没有选区）。
            find_feature::FindAction::SelectionNext => {
                find_feature::reduce(&mut state.find, a, window);
                vec![Effect::FindSelection(1)]
            }
            find_feature::FindAction::SelectionPrev => {
                find_feature::reduce(&mut state.find, a, window);
                vec![Effect::FindSelection(-1)]
            }
            _ => {
                find_feature::reduce(&mut state.find, a, window);
                Vec::new()
            }
        },
        Action::Clipboard(a) => clipboard_feature::reduce(state, a),
        Action::Select(op) => vec![Effect::Select(Cmd::Move(op))],
        Action::SelectSet(op) => vec![Effect::Select(Cmd::Set(op))],
        Action::Log(a) => log_feature::reduce(state, a),
        // R14-c：确认时把「选中命令」翻译成 Action，再走一次同一条纯函数路径。
        // 面板自己不执行任何副作用，也没有字符串派发的旁路（B4）；未接线项
        // 的 confirm_id 是 None，点了只会安静地关掉面板，不会假装成功（R5）。
        Action::Palette(a) => {
            let confirm = matches!(a, PaletteAction::Confirm);
            let id = if confirm {
                palette_feature::confirm_id(&state.command_palette).map(str::to_owned)
            } else {
                None
            };
            palette_feature::reduce(&mut state.command_palette, a);
            match id {
                Some(id) => match Action::parse(&id) {
                    Some(action) => reduce(state, action, window),
                    None => Vec::new(),
                },
                None => Vec::new(),
            }
        }
        Action::Goto(a) => goto_feature::reduce(&mut state.goto, a),
        // D3：地址栏。**没有 reducer、没有 Effect、一行 `ViewState` 都不写**：
        // 输入框的开合、候选与最近使用都活在投影层（`apps/desktop` 的
        // `dispatch_action` 认得这个变体就直接开地址栏）。
        //
        // 为什么不在这里翻一个 `show_address_bar` 位：那样这条命令就有两个写者
        // （reducer 写、投影层画），而地址栏不是「窗口 chrome 的一部分」，
        // 它是一个带自己输入缓冲的浮层——投影层本来就必须持有它的状态，
        // 纯层再放一个位只会多出一份要同步的真相。
        Action::AddressBar => Vec::new(),
        Action::Center(a) => {
            // `Default Session Settings…` 与 `Session Settings…` 都不打开中心面板
            // 占位页，而是改开对应的会话弹窗（与 New/Edit 共用），由 Effect 落到
            // 投影层。其余 Center 动作仍走中心面板的纯 reducer。
            if matches!(a, CenterAction::Open(CenterPage::DefaultSettings)) {
                return vec![Effect::OpenDefaultSessionSettings];
            }
            if matches!(a, CenterAction::Open(CenterPage::SessionSettings)) {
                return vec![Effect::OpenActiveSessionSettings];
            }
            center_feature::reduce(&mut state.center, a);
            Vec::new()
        }
        Action::Lock(a) => lock_feature::reduce(state, a),
        // P4：布局动作不写 `ViewState`——与分屏树同类，布局是投影层的视图结构。
        Action::Layout(a) => dock::reduce(a),
        // 2026-11-07：margin 只翻一位 bool，没有 Effect——列宽与颜色由投影层现算。
        Action::Margin(a) => {
            margin_feature::reduce(state, a);
            Vec::new()
        }
        // 2026-11-14：折叠族九条。与 `Margin` 正好相反——**一行 `ViewState` 都不写**，
        // 只把口令抛上去。折叠集合是每条会话的属性（`Tab.folds`），决策要的输入
        // （输入行 / 光标行 / 视口区间）全在投影层，纯层一样都没有。
        // 这里不解包 `FoldCmd` 去做任何判断：任何「折哪几个块」的推理若写在这一层，
        // 就得先往 `ViewState` 里塞一份光标行或输入行表——那是第二份真相。
        Action::Fold(cmd) => vec![Effect::Fold(cmd)],
        // 2026-11-09：传输只带口令，状态全在投影层（那里才有会话句柄）。
        Action::Xfer(cmd) => vec![Effect::Xfer(cmd)],
        // W2-b：标签右键只带意图，标签列表 / 最近关闭 / 颜色表全在投影层。
        Action::Tab(a) => vec![Effect::Tab(a)],
        // V1-1 / V1-2 / V1-4：投影层的 IO 意图，reducer 不碰状态。
        Action::ImportSessions(source) => vec![Effect::ImportSessions(source)],
        Action::SaveLayout => vec![Effect::SaveLayout],
        Action::OpenShortcuts => {
            state.show_shortcuts = true;
            vec![Effect::OpenShortcuts]
        }
        // 独立弹窗：纯 UI 状态，无 Effect；窗口内容由投影层读 quickbar.json 画。
        Action::OpenQuickCommands => {
            state.show_quick_commands = true;
            vec![]
        }
        Action::TmuxCommand(cmd) => vec![Effect::TmuxCommand(cmd)],
        Action::TmuxFeed(line) => vec![Effect::TmuxFeed(line)],
        // P9：纯 reducer 只把静态文本提升为 Effect；不碰 ViewState、不碰 PTY。
        Action::QuickCommand(text) => vec![Effect::SendCommand(text)],
        // P9：配置菜单切分组可见性——只翻一位 `bool`，无 Effect。index 越界时
        // 用取模兜底，菜单永远给的是合法下标，这里只是不让 reducer panic。
        Action::QuickGroupToggle(i) => {
            let slot = i % state.quick_groups_visible.len();
            state.quick_groups_visible[slot] = !state.quick_groups_visible[slot];
            vec![]
        }
    }
}
