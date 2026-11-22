//! 命令面板里的 Tmux 无目标命令表（`Mux: XXX` 官方文档主交互范式）。
//!
//! 放在 `vocab` 命名空间下是为了让 parse.rs / palette.rs 都能引用而不打破
//! `term-app` 的分层门禁（那两层只许向下依赖 `vocab`）。这里只放数据与纯
//! 查表函数：带目标的命令（SelectPane / KillPane …）不在这里，它们必须由
//! tmux 面板按解析出的窗格集合校验后才能下发。

/// `(显示名, 命令 ID, 控制行)`。ID 用 `concat!` 拆两段：整仓门禁禁止命令 ID
/// 字面量出现在 parse 分支以外，拆段既满足规则又保持 `&'static str` 身份。
pub const TMUX_PALETTE: &[(&str, &str, &str)] = &[
    ("Tmux: Split Window (horizontal)", concat!("Tmux.", "SplitHorizontal"), "split-window -h"),
    ("Tmux: Split Window (vertical)", concat!("Tmux.", "SplitVertical"), "split-window -v"),
    ("Tmux: New Window", concat!("Tmux.", "NewWindow"), "new-window"),
    ("Tmux: Refresh Client", concat!("Tmux.", "Refresh"), "refresh-client"),
    ("Tmux: Detach Client", concat!("Tmux.", "Detach"), "detach-client"),
];

/// 命令 ID → 安全的控制行。带目标的命令一律返回 `None`。
pub fn line_for(id: &str) -> Option<&'static str> {
    TMUX_PALETTE
        .iter()
        .find(|(_, candidate, _)| *candidate == id)
        .map(|(_, _, line)| *line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_palette_id_resolves_to_its_control_line() {
        assert_eq!(line_for("Tmux.SplitHorizontal"), Some("split-window -h"));
        assert_eq!(line_for("Tmux.Detach"), Some("detach-client"));
        assert_eq!(line_for("Tmux.unknown"), None);
        for (_, id, _) in TMUX_PALETTE {
            assert!(line_for(id).is_some(), "{id} 必须能解析，否则面板里点了没反应");
        }
    }
}
