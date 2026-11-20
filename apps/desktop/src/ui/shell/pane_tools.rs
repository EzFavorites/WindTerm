//! T3 / P5：窗格工具行——标签条**下面**那条独立的水平条（面包屑来源 + 发射点）。
//!
//! # 为什么要把它从标签条里拆出来
//!
//! 视觉基线 `reference/picture-1.png` / `picture-2.png` 里，终端区上方从来不是一行：
//!
//! ```text
//! ┌ 标签条 ────────────────────────────────────────────┐
//! │ Local SSH │ cmd │ powershell │ …            +  ≡  ●│
//! ├── 工具行 ──────────────────────────────────────────┤
//! │ ⠿ ▣▾ ▣× ▣○ ⓘ │ ▸ ssh ▸ putty sessions ▸ local ssh  │
//! ├── 终端内容 ────────────────────────────────────────┤
//! ```
//!
//! P4 之前（以及 P5 开工前）的 `draw()` 把标签与 8 个快捷图标塞进**同一条**
//! `TopBottomPanel::top("tabs_and_tools")`，图标还右对齐。整屏少一条，图标挤在
//! 标签右边，是全屏对照里最容易看出来的结构偏差。
//!
//! # 本文件现在只做两件事
//!
//! 1. **面包屑的段落推导**：[`breadcrumb`]，纯函数，`&SessionConfig` 进、
//!    `Vec<String>` 出（B3：只读当前活动标签的配置，不发 Action）。
//! 2. **发射点**：`TerminalWindow::draw_pane_tools`，`ui/mod.rs` 按名字调用。
//!
//! 真正的绘制在 [`super::tool_bar`]——P9-f 把这一行改成了视觉基线里那个
//! 「拖动抓手 + 三颗窗格动作 + 可拖走的工具栏面板」，并带上了拖放与回程逻辑。
//!
//! 分组按 `/` 拆成多段：视觉基线的 `ssh » putty sessions » local ssh` 正好是
//! 「两级分组 + 会话名」，而我们的出厂默认只有一级 `Servers`，于是显示两段。
//! **按数据有几级就画几段**，不硬凑三段。第一版的写法是
//! `Protocol::label() › group › name`，默认会画出
//! `Local Shell › Servers › Local Shell`（协议名与默认会话名撞车）——既重复也
//! 把「层级」说错了：协议不是分组的父节点。
//!
//! 分组为空时只画会话名，不画一个空的 `›`。
//!
//! # 右端三件（2026-11-08 补齐，见 `tool_bar_tail.rs`）
//!
//! 早期版本只在 `tool_bar.rs` 的模块头里记了一句「右端的假按钮一个都不画」：
//! R5 要求「已接线＝有效果」，画一个点了没反应的箭头比少画更糟。现在 `>` /
//! `∨` / `⋮` 各接了一颗真菜单（Goto / 会话列表 / 窗格动作），所以可以画了。
//! 本文件仍然只管面包屑段落推导与发射点，绘制全在 `tool_bar*.rs`。
//!
//! # 仍未做（见 `docs/p5-top-chrome-gap.md`）
//!
//! **每个分屏窗格都有自己的一对条**（picture-1 里三个窗格各一套）。要把窗格
//! chrome 搬进 `layout/split.rs` 的递归切分里，属于后续步骤。

use crate::ui::*;

/// 面包屑的段落。纯函数：`&SessionConfig` 进、`Vec<String>` 出，可单测。
///
/// `group` 允许是 `ssh/putty sessions` 这样的多级路径，按 `/` 拆开；空白段
/// 与全空分组都跳过，末尾永远补会话名。
pub(in crate::ui) fn breadcrumb(config: &term_core::session::SessionConfig) -> Vec<String> {
    let mut parts: Vec<String> = config
        .group
        .split('/')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect();
    parts.push(config.name.clone());
    parts
}

impl TerminalWindow {
    /// 画工具行。**发射点在 `draw()`**：它是标签条之后的第二条 `TopBottomPanel`，
    /// 声明顺序决定它压在终端区上方（而不是盖在上面）。
    ///
    /// P9-f 起真正的绘制搬到 [`super::tool_bar`]：这一行从「8 个按钮 + 面包屑」
    /// 变成了「抓手 + 三颗窗格动作 + 可拖走的工具栏面板」，含拖动/停靠逻辑与
    /// 七件几何图标，塞回本文件会顶破 R9 的 450 行。这里只留发射点，
    /// `ui/mod.rs` 的装配顺序与面板名（`pane_tools`）都不动。
    pub(in crate::ui) fn draw_pane_tools(&mut self, ui: &mut egui::Ui) {
        // 单窗格：全窗一条、可整块拖走。`session` 取旧代码同一个「活动槽位」，
        // 行为与像素都不变（ADR-0026 ④-1 把 `draw` 参数化后由这里显式给出）。
        let session = self
            .tabs
            .get(self.active_slot())
            .map(|t| t.identity)
            .unwrap_or(0);
        super::tool_bar::draw(self, ui, super::tool_bar::BarScope::single(session));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::session::Protocol;

    #[test]
    fn a_multi_level_group_expands_into_one_crumb_per_level() {
        let mut cfg = SessionConfig::new(Protocol::Ssh);
        cfg.group = "ssh/putty sessions".to_string();
        cfg.name = "local ssh".to_string();
        assert_eq!(breadcrumb(&cfg), vec!["ssh", "putty sessions", "local ssh"]);
    }

    #[test]
    fn the_default_group_produces_two_crumbs_without_a_duplicated_name() {
        let mut cfg = SessionConfig::new(Protocol::Shell);
        cfg.group = "Servers".to_string();
        cfg.name = "Local Shell".to_string();
        assert_eq!(breadcrumb(&cfg), vec!["Servers", "Local Shell"]);
    }

    #[test]
    fn an_empty_group_is_skipped_instead_of_leaving_a_leading_arrow() {
        let mut cfg = SessionConfig::new(Protocol::Shell);
        cfg.group = " /  / ".to_string();
        cfg.name = "my box".to_string();
        assert_eq!(breadcrumb(&cfg), vec!["my box"]);
    }
}
