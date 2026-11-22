//! 可停靠面板：侧栏 / 文件 / 工具 / 隧道 / tmux。
//!
//! 与 `shell` 的区别是这一组**有开关**（`ViewState.show_*`），
//! 起还有停靠位与浮动位（见 `ui/layout/dock.rs`）。每个文件里都只有
//! 「画」与「发 Action」。
//!
//! 本文件补一张**面板表**：`PanelId` 与「默认宽度 / 内容」的对应关系。
//! 之前这三件事散在 `draw()` 的 `if` 里（开关）、三个 `default_width(200.0)`
//! （宽度）、三个不同的绘制函数（内容）；停靠循环要按 id 取它们，所以收了进来。
//! **开关**那一列已归还 `term_app::shell::panel_visible`（2026-11-12）：
//! 它是动作侧唯一知道「翻哪一位」的表，绘制侧不该再抄一份。

use term_core::theme::DESIGN_TOKENS;

pub(in crate::ui) mod filer;
pub(in crate::ui) mod sidebar;
pub(in crate::ui) mod sidebar_rows;
mod sidebar_view;
mod session_icons;
/// 底带 `Shell` 面板：面板内自己的标签条（多标签）+ 与主区同一份终端。
///
/// 2026-11-15 从 `tools.rs` 拆出来：面板要画标签条、`＋` / `≡` 两颗图标与
/// 标签过滤下拉，那些代码塞回 `tools.rs` 会把「六个只读工具面板」那个文件
/// 撑成两件事。
mod shell_panel;
/// 「脚本」停靠面板：从脚本库挑 Rhai 脚本，在当前活动标签页的会话上跑。
///
/// 复用 `apps/desktop/src/script_run.rs` 的执行线程；与 `terminal/script.rs` 的
/// `ScriptDialog`（一次性运行窗口）是同一套引擎，只是常驻在底带当面板。
mod list_popup;
mod script_panel;
mod script_panel_row;
pub(in crate::ui) use script_panel::ScriptPanelState;
mod tmux;
mod tools;
mod outline;
pub(in crate::ui) use outline::outline_items;
pub(in crate::ui) mod tools_sender;
mod tools_sender_menu;
mod tools_sender_row;
mod tools_sender_state;
mod tools_sender_tabs;
mod transfer_panel;
mod tunnel_panel;
pub(in crate::ui) mod tunnels;
mod xfer_queue;

use crate::ui::*;
use term_app::dock::PanelId;

pub(in crate::ui) use filer::Filer;
// 2026-11-17：`session_icons::paint_gear` 已并入 `shell::glyph`（全软件唯一一份
// 齿轮/叉画法），这里的再导出随之退役，调用点改从 `crate::ui::shell` 取。
pub(in crate::ui) use tools_sender_tabs::SenderPanel;
pub(in crate::ui) use tunnels::RunningTunnel;

impl TerminalWindow {
    /// 这个面板现在要不要画（领域状态的读者）。
    ///
    /// 专注模式不在这里判：它在 `draw()` 里一次性地把整条带压掉，
    /// 免得每个面板各写一遍 `!focus`。
    ///
    /// （2026-11-12）：**开关表只有一份，在 `term_app::shell::panel_visible`**。
    /// 这里原来抄了同样的 9 个分支，`ui` 和 `app` 各列一张；两张都是穷尽 match，
    /// 靠编译器挡「改了一边漏了另一边」。挡不住的是同一个开关有两条翻法，
    /// 所以现在只留一层委托：本方法退化成别名，调用点一行不用改。
    pub(in crate::ui) fn panel_visible(&self, id: PanelId) -> bool {
        term_app::shell::panel_visible(&self.view, id)
    }

    /// 停靠时的默认宽度。与三个写死值保持一致（侧栏 200 / 文件 260 /
    /// 工具 220）——`egui` 只在**第一次**见到这个 panel id 时用它，所以改这里
    /// 不会影响已经记住宽度的用户，但会改全新 profile 下的基线。
    ///
    /// **2026-11-12 token 接线**：侧栏宽度改取 `DESIGN_TOKENS.sidebar_width`
    /// （200.0），不再用实测值 196.0 硬编码。原先 196 是对视觉基线 2× 量出的
    /// 392px 换算，但 `sidebar_width` 令牌从未被消费，属于死令牌；用户拍板
    /// 「先完成功能、后统一修基线」，故此处直接接线，4px 差异在后续基线
    /// 评审时一并裁定（见 `p0-2-menu-deviation.md` §3 口径冲突）。
    pub(in crate::ui) fn panel_width(id: PanelId) -> f32 {
        match id {
            PanelId::Sidebar => DESIGN_TOKENS.sidebar_width,
            PanelId::Filer => 260.0,
            _ => 220.0,
        }
    }

    /// 面板内容。标题由停靠容器补（见 `layout/dock.rs`），这里只管身体：
    /// 侧栏与文件面板自带工具行，六个工具面板的 `heading` 由容器画。
    pub(in crate::ui) fn draw_panel_body(&mut self, id: PanelId, ui: &mut egui::Ui) {
        match id {
            PanelId::Sidebar => self.draw_sidebar(ui),
            PanelId::Filer => self.draw_filer(ui),
            PanelId::Explorer => tools::draw_explorer(self, ui),
            PanelId::Outline => tools::draw_outline(self, ui),
            PanelId::Sender => self.draw_sender(ui),
            PanelId::Session => tools::draw_session(self, ui),
            PanelId::Shell => shell_panel::draw_shell(self, ui),
            PanelId::Transfer => transfer_panel::draw_transfer(self, ui),
            PanelId::Sysmon => tools::draw_sysmon(self, ui),
            PanelId::Script => script_panel::draw_script_panel(self, ui),
        }
    }
}

#[cfg(test)]
mod scroll_owner_tests {
    use super::*;

    /// 每个面板的滚动必须**恰好有一个**归属：要么由停靠容器统一包一层
    /// `ScrollArea`（`scrolls_in_dock` 为真），要么自己实现
    /// （`Filer` 是手绘列表 + 自己的 `scroll` 字段，`Sidebar` 是树区单独一层
    /// `ScrollArea`，`Shell` 面板里是**终端**——滚轮归 `viewport.scroll_up/down`
    /// 那条回看路径）。
    ///
    /// 挡的是 2026-11-14 之前的状态：`Sidebar` 被 `is_tool_pane` 排除（它的
    /// 标题带与 Filter 不能跟着滚），但树区忘了自己接滚动，于是会话一多就直接
    /// 溢出、滚不动——一直没人发现，因为它「看起来像设计如此」。以后再有面板
    /// 想排除出容器的统一滚动，这条会立刻红。
    ///
    /// 2026-11-15：`Shell` 进 `by_itself` 那一侧。底带那块面板装的是**终端**
    /// （与主区同一份渲染），外面套 `ScrollArea` 有两个实打实的后果：滚轮回看
    /// 被容器吃掉、`available_rect_before_wrap` 变成无穷高（PTY 的行数算不出来）。
    /// 所以容器那两处（`layout/dock_bottom.rs` / `layout/dock_paint.rs`）对
    /// `PanelId::Shell` 不包滚动——这条断言因此把 `Shell` 记在「自己实现」一侧，
    /// 否则它会以「两边都真」的形式红，而真实原因恰恰是它两边都不该真。
    #[test]
    fn every_panel_owns_its_scroll() {
        for id in PanelId::ALL {
            let by_dock = id.scrolls_in_dock();
            let by_itself = matches!(id, PanelId::Filer | PanelId::Sidebar | PanelId::Shell);
            assert!(
                by_dock ^ by_itself,
                "{} 的滚动归属不唯一：容器统一={by_dock} 自己实现={by_itself}",
                id.name()
            );
        }
    }
}
