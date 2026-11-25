//! 标签右键菜单的 10 个动作，从 `ui/` 的私有 `Work` 枚举提升到纯层。
//!
//! # 为什么要有这一层
//!
//! 以前 `apps/desktop/src/ui/shell/tab_menu.rs` 里有一个私有 `enum Work`，
//! 右键菜单收集 `Work`，画完循环里直接改 `TerminalWindow` 的字段。结果是
//! 10 个**真实可点入口**既没有命令 ID，也进不了 `capability` 台账——
//! 「已接线 120 条」这句话其实少算了它们，而「点了有效果」也
//! 没法在纯层被验证。语义归纯层 `Action`，`ui/` 只发 `Action`，
//! `capability` 用同一个 `Action::parse` 判定，台账与真实入口同源。
//!
//! # `target` 为什么是 `Option<usize>`
//!
//! 右键点中的标签不一定是活动标签。`Some(i)` 来自右键菜单的强类型构造，
//! `None` 来自命令 ID 路径（命令面板 / 快捷键），落地在投影层取
//! `active_tab`。不写 `usize::MAX` 之类的哨兵：一个能表达「未指定」的
//! 类型，比一个靠约定才能解释的魔数少一处会漂的口径。
//!
//! `Split` / `Color` 这类带参数的动作，命令 ID 路径给的是出厂默认
//! （水平分屏 / 恢复默认颜色）；右键菜单仍传真实参数。ID 只声明
//! 「要做哪一类动作」，不承担「把整个参数空间编码进字符串」的职责。

pub use crate::vocab::TabAction;

/// 一个「影子入口」：真实可点，但不在菜单栏里，所以菜单树里没有它。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShadowEntry {
    pub id: &'static str,
    pub label: &'static str,
}

/// 10 个影子入口的唯一数据表。`capability::inventory()` 把它并进能力面，
/// 于是「已接线」与「真实可点入口」可以逐条对上。
pub const SHADOW_ENTRIES: &[ShadowEntry] = &[
    ShadowEntry {
        id: "Terminal.TabActivate",
        label: "Activate",
    },
    ShadowEntry {
        id: "Terminal.TabClose",
        label: "Close",
    },
    ShadowEntry {
        id: "Terminal.TabCloseOthers",
        label: "Close Others",
    },
    ShadowEntry {
        id: "Terminal.TabCloseAll",
        label: "Close All Views",
    },
    ShadowEntry {
        id: "Terminal.TabReopenClosed",
        label: "Reopen Closed",
    },
    ShadowEntry {
        id: "Terminal.TabDuplicate",
        label: "Duplicate Session",
    },
    ShadowEntry {
        id: "Terminal.TabSplit",
        label: "Split Horizontally",
    },
    ShadowEntry {
        id: "Terminal.TabCopyName",
        label: "Copy Session Name",
    },
    ShadowEntry {
        id: "Terminal.TabCopyUrl",
        label: "Copy Session URL",
    },
    ShadowEntry {
        id: "Terminal.TabColor",
        label: "Tab Color",
    },
];

/// 只能从标签右键触发、**不能**从命令面板执行的影子入口。
///
/// 判据不是「感觉它没意义」，而是「它的 ID 路径构造上就是自赋值」：
/// 这些 ID 在 `parse()` 里拿到的是 `target: None`，而缺了 target 的动作
/// 语义上是空的（`Activate { target: None }` = 激活当前标签 = `active = active`）。
/// `palette::entries` 的 `Command` 模式用这份名单把它们剔除——只列「按下就有效果」的。
///
/// **这份名单有测试钉着**（`context_only_lists_exactly_the_commands_whose_id_path_is_a_noop`）：
/// 往里加一条，测试会要求那条的 ID 路径确实是空转；哪天某条命令的 ID 路径变得
/// 有真实效果了，测试同样会红，逼着把它从这里删掉。
pub const CONTEXT_ONLY: &[&str] = &["Terminal.TabActivate"];


/// 命令 ID → 本能力的动作。未接线的 ID 返回 `None`。
///
/// 2026-11-12：实现只有一份，在 [`crate::parse`]（入口层）。这里保留同名入口，
/// 一是因为「每个能力自带字符串入口」是这个 crate 对外的约定，二是因为
/// `apps/desktop` 有直接调 `term_app::log::parse` 的地方——把路径改掉没有架构价值。
pub fn parse(id: &str) -> Option<TabAction> {
    crate::parse::tab_parse(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// 表里每一条都必须能被 `parse` 认出来，反过来也成立：
    /// 多一个 ID 或少一个 ID 都会让台账与真实入口重新分叉。
    #[test]
    fn shadow_entries_and_parse_are_the_same_ten_commands() {
        assert_eq!(SHADOW_ENTRIES.len(), 10);
        let ids: BTreeSet<&str> = SHADOW_ENTRIES.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), 10, "影子入口里有重复命令 ID");
        for entry in SHADOW_ENTRIES {
            assert!(
                parse(entry.id).is_some(),
                "{} 在表里却没有 parse 入口",
                entry.id
            );
        }
        let labels: BTreeSet<&str> = SHADOW_ENTRIES.iter().map(|e| e.label).collect();
        assert_eq!(labels.len(), 10, "标签文案重复会让右键菜单无法区分");
    }

    /// `None` 是「投影层取活动标签」，不是「第 0 个标签」。
    #[test]
    fn the_id_path_never_pretends_to_know_which_tab_was_clicked() {
        for entry in SHADOW_ENTRIES {
            match parse(entry.id).unwrap() {
                TabAction::Activate { target }
                | TabAction::Close { target }
                | TabAction::CloseOthers { target }
                | TabAction::Duplicate { target } => {
                    assert_eq!(target, None, "{} 不该硬编码标签下标", entry.id)
                }
                TabAction::Color { target, slot } => {
                    assert_eq!(target, None);
                    assert_eq!(slot, None);
                }
                TabAction::Split { horizontal } => assert!(horizontal),
                _ => {}
            }
        }
        // 强类型路径仍然能把「右键点中的是第 7 个标签」带进来。
        assert_eq!(
            TabAction::Close { target: Some(7) },
            TabAction::Close { target: Some(7) }
        );
        assert_ne!(
            TabAction::Close { target: Some(7) },
            TabAction::Close { target: None }
        );
    }

    // ── 缺陷修复：命令面板不许列「ID 路径必然空转」的上下文命令 ──────

    /// `CONTEXT_ONLY` 不能烂：名单里每一条的 ID 路径都必须真的是空转。
    ///
    /// 两头都钉住了——往里塞一条其实是有效果的命令，这条会红（`parse` 给的不是
    /// `Activate { target: None }`）；哪天 `parse("Terminal.TabActivate")` 变得
    /// 有真实效果了，这条同样会红，逼着把它从名单里删掉。
    #[test]
    fn context_only_lists_exactly_the_commands_whose_id_path_is_a_noop() {
        assert_eq!(CONTEXT_ONLY.len(), 1, "名单长度变了就要重新审视每条的理由");
        assert_eq!(CONTEXT_ONLY, &["Terminal.TabActivate"]);
        for id in CONTEXT_ONLY {
            assert_eq!(
                parse(id),
                Some(TabAction::Activate { target: None }),
                "{} 的 ID 路径不是空转，不该留在 CONTEXT_ONLY 里",
                id
            );
        }
    }

    /// 上一条只证明了名单的**内容**对；这一条证明名单真的被命令面板用上了。
    #[test]
    fn the_command_palette_does_not_offer_context_only_commands() {
        let wired = crate::palette::entries(crate::palette::PaletteMode::Command);
        let wired_ids: BTreeSet<&str> = wired.iter().map(|e| e.id.as_str()).collect();
        assert!(
            !wired_ids.is_empty(),
            "可执行列表被过滤空了，下面的断言会恒真，这条测试就失去意义"
        );
        for id in CONTEXT_ONLY {
            assert!(
                !wired_ids.contains(id),
                "{} 的 ID 路径是自赋值，不该出现在命令面板的可执行列表里",
                id
            );
        }
        // 但它不能从面板里彻底消失：`Anything`（全量）范围仍要能查到，
        // 否则用户就再也看不到「有这么一条命令」了。
        let all = crate::palette::entries(crate::palette::PaletteMode::Anything);
        let all_ids: BTreeSet<&str> = all.iter().map(|e| e.id.as_str()).collect();
        assert!(
            all_ids.contains("Terminal.TabActivate"),
            "上下文命令应仍在全量列表里可见"
        );
    }
}
