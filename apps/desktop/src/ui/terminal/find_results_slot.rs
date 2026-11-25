//! Find All 结果列表的**按标签身份**快照（从 `pane_overlays.rs` 拆出）。
//!
//! 分屏时每个叶子都画一次叠层，原先大家往同一个 `egui::Id` 写 `FindResults`，
//! 最后画的（往往不是活动窗格）覆盖前面的，Find 栏于是读到别的标签的结果。
//! 这里写入只接受活动标签的身份，读取同时校验身份与 query：换标签或改查询后，
//! 上一帧的残留不会冒充当前答案。

use term_core::session_runtime::SessionId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) struct SlotFindResults {
    pub owner: SessionId,
    pub query: String,
    pub rows: Vec<term_app::find::FindResultRow>,
}

fn results_id() -> egui::Id {
    egui::Id::new("find_all_results")
}

fn owner_id() -> egui::Id {
    egui::Id::new("find_all_results_owner")
}

/// 只有活动标签写；非活动叶子直接返回，不碰快照（避免「最后画的覆盖」）。
pub(in crate::ui) fn publish(
    ctx: &egui::Context,
    owner: Option<SessionId>,
    results: term_app::find::FindResults,
) {
    let Some(owner) = owner else { return };
    ctx.data_mut(|d| {
        d.insert_temp(owner_id(), owner);
        d.insert_temp(results_id(), SlotFindResults {
            owner,
            query: results.query,
            rows: results.rows,
        });
    });
}

/// 读取时校验 id + query，任一不符都当没有结果。
pub(in crate::ui) fn read(ctx: &egui::Context, query: &str) -> Option<term_app::find::FindResults> {
    let owner = ctx.data(|d| d.get_temp::<SessionId>(owner_id()))?;
    let snapshot = ctx.data(|d| d.get_temp::<SlotFindResults>(results_id()))?;
    if snapshot.owner != owner || snapshot.query != query {
        return None;
    }
    Some(term_app::find::FindResults {
        query: snapshot.query,
        rows: snapshot.rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(n: usize) -> Vec<term_app::find::FindResultRow> {
        (0..n)
            .map(|i| term_app::find::FindResultRow {
                index: i,
                row: i as i64,
                col_start: 0,
                col_end: 1,
                summary: format!("row-{i}"),
            })
            .collect()
    }

    fn results(query: &str) -> term_app::find::FindResults {
        term_app::find::FindResults { query: query.to_string(), rows: rows(2) }
    }

    #[test]
    fn an_inactive_pane_never_overwrites_the_active_snapshot() {
        let ctx = egui::Context::default();
        publish(&ctx, Some(11), results("needle"));
        // 非活动叶子：owner=None，快照必须保持 11 那一份。
        publish(&ctx, None, results("other"));
        let found = read(&ctx, "needle").expect("活动标签的结果仍在");
        assert_eq!(found.rows.len(), 2);
        assert_eq!(found.rows[1].summary, "row-1");
    }

    #[test]
    fn a_changed_owner_or_query_rejects_the_stale_snapshot() {
        let ctx = egui::Context::default();
        publish(&ctx, Some(11), results("needle"));
        assert!(read(&ctx, "needle").is_some());
        assert!(read(&ctx, "needl").is_none(), "查询不同就丢弃");
        // 换标签后新活动标签写自己的身份与结果，旧的 11 不再匹配。
        publish(&ctx, Some(22), results("needle"));
        assert!(read(&ctx, "needle").is_some());
        ctx.data_mut(|d| d.insert_temp(owner_id(), 11u64));
        assert!(read(&ctx, "needle").is_none(), "身份不符就丢弃");
    }
}
