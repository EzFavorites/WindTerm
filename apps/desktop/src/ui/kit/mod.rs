//! UI 工具箱：**函数式绘制原语**，不定义 widget trait。
//!
//! ADR-0021 明确不做 retained 组件抽象（理由见该 ADR「明确不做 1」：那等于
//! 在本仓库里再造一个小框架）。所以这里全部是 `fn draw_xxx(ui, &Props)`：
//! 输出只由入参决定，夹具能定格，视觉基线逐字节可比。
//!
//! 分层：这一层只认识 `egui::Ui` / `egui::Painter` / 主题色号，不认识任何
//! 业务状态（没有 `ViewState`、没有 `TerminalWindow`）。所以它既能被生产
//! 路径调用，也能被组件夹具直接调用。

pub(in crate::ui) mod checkbox;
pub(in crate::ui) mod footer;
pub(in crate::ui) mod menu_row;
pub(in crate::ui) mod spec;
pub(in crate::ui) mod switch;
pub(crate) mod type_scale;
pub(in crate::ui) mod window;
