//! 工具按钮的出厂列表（从 `construct.rs` 拆出，4j：单文件 500 代码行上限）。
//! 只造数据，不碰窗口状态；公开调用点仍是 `construct.rs` 里的 `default_tools()`。

use crate::ui::{ToolAction, ToolButton};

/// 工具按钮的出厂列表（图标 + 提示 + 动作）。
pub(in crate::ui) fn default_tools() -> Vec<ToolButton> {
    vec![
        ToolButton {
            icon: "⬜",
            tooltip: "New Session",
            action: ToolAction::NewSession,
        },
        ToolButton {
            icon: "🔌",
            tooltip: "Connect",
            action: ToolAction::Connect,
        },
        ToolButton {
            icon: "🚫",
            tooltip: "Disconnect",
            action: ToolAction::Disconnect,
        },
        ToolButton {
            icon: "🔄",
            tooltip: "Refresh",
            action: ToolAction::Refresh,
        },
        ToolButton {
            icon: "⚙",
            tooltip: "Settings",
            action: ToolAction::Settings,
        },
        ToolButton {
            icon: "⌨",
            tooltip: "Terminal",
            action: ToolAction::Terminal,
        },
        ToolButton {
            icon: "📁",
            tooltip: "SFTP",
            action: ToolAction::Sftp,
        },
        ToolButton {
            icon: "📡",
            tooltip: "Serial",
            action: ToolAction::Serial,
        },
    ]
}

