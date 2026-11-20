//! 目录同步的**组装层**：把两端采集到的清单变成 `term_xfer::sync` 能吃的计划。
//!
//! # 为什么这一层在 `apps/desktop`
//!
//! 两边的清单类型各自定义，中间做一次映射：
//!
//! - `term_xfer::sync::SyncEntry`（决策层的输入）——那个 crate 是**刻意零依赖零 IO
//!   的协议层**（见其 `Cargo.toml` 头），让它反过来依赖 `term-sftp` 只为了一个
//!   结构体，会立刻破掉那条边。
//! - `term_sftp::ScannedEntry`（采集层的输出）——它连 `SyncOp` 都不认识。
//!
//! 字段一一对应，这三行映射比在两个 crate 之间拉一条新边便宜。这里也就自然成了
//! 「两端口径不一致」唯一要处理的地方。
//!
//! # 口径差一：`mtime` 是可空的
//!
//! `ScannedEntry.mtime` 是 `Option<i64>`：`None` 表示**对端没报**（服务端不带
//! `FileAttributes.mtime`，或者本地 `modified()` 失败），不是 1970 年。而
//! `SyncEntry.mtime` 是 `i64`——决策层要按大小比先后，没有「不知道」这一档。
//!
//! 怎么填这一格，决定了整份计划是否有效：
//!
//! | 填法 | 后果 |
//! |---|---|
//! | 填 `0` | `L.mtime > R.mtime` 恒成立 → **全量上传**，远端新改的内容被静默覆盖 |
//! | 填 `i64::MAX` | `R.mtime > L.mtime` 恒成立 → **全量下载**，本地新改的内容被静默覆盖 |
//! | 填**对侧的值**（[`effective_mtime`]） | 决策层看到「时间相同」，退到判定表最后一行：`size` 相同 → `Skip`，不同 → `Upload` |
//!
//! 第三种是唯一不撒谎的一种：它不假装知道时间，而是让判定退到**还在的那条依据**
//! （`size`）。语义上就是「这条不参与时间比较，按大小 + 本地优先处理」，与判定表
//! 既有的「`mtime` 相同但 `size` 不同 → `Upload`」是同一条规则，不新增语义。
//!
//! 代价写在这里，不藏着：远端 mtime 没报、且远端内容更新、且大小也变了——这一种
//! 组合会被判成上传（覆盖远端）。`DeletePolicy` 默认 `Keep`、本地优先是既有取舍，
//! 这里只是把它延伸到「时间未知」的情形。
//!
//! # 为什么要 `preserve`
//!
//! 执行阶段上传 / 下载都必须带 `preserve = true`（把源端 mtime 写到目标端）。
//! 否则传完之后目标端的 mtime 变成「现在」，下一趟扫描会看到「对端更新了」，
//! 于是同一份内容每次同步都来回传一次——计划永远收敛不到全 `Skip`。设计稿
//! §7 的那条「跑完重采 → 全 Skip」判据抓的就是这个。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use term_sftp::ScannedEntry;
use term_xfer::sync::{self, DeletePolicy, SyncEntry, SyncItem, SyncOp};

/// 一次本地扫盘的后台句柄。
///
/// 为什么必须后台：`scan_local_entries` 是同步阻塞的递归遍历。一个几千条目的目录
/// （`node_modules`、`target`）在 UI 线程上跑就是几百毫秒的假死，而且这期间面板
/// 连「扫描中」都画不出来——它自己卡在扫描里。设计稿 §6 的 Scanning 阶段要求
/// 「UI 每帧只查完成标志」，所以线程在这里、轮询在 [`LocalScan::poll`]。
///
/// 一次扫描一条线程，不池化：同步是用户按一下的偶发动作，线程起停的成本远小于
/// 维护一个池子的复杂度。扫完线程自己退出。
pub struct LocalScan {
    rx: Receiver<Result<Vec<ScannedEntry>, String>>,
}

impl LocalScan {
    /// 起一条线程扫 `root`。返回即返回，不等结果。
    pub fn start(root: PathBuf) -> Self {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            // 失败也**一定要发**：不发的话 UI 永远停在「扫描中」，而且没有任何
            // 迹象说明它卡在哪。`scan_local_entries` 给的是分类过的 `ScanError`，
            // 到这里压成文案——它要进状态行，那一侧只认一句话。
            let out = term_sftp::scan_local_entries(&root.to_string_lossy())
                .map_err(|e| e.to_string());
            let _ = tx.send(out);
        });
        LocalScan { rx }
    }

    /// 这一帧有没有结果。`None` = 还在扫。
    pub fn poll(&self) -> Option<Result<Vec<ScannedEntry>, String>> {
        match self.rx.try_recv() {
            Ok(v) => Some(v),
            Err(TryRecvError::Empty) => None,
            // 线程没发结果就没了（panic）：必须当成失败报出去，否则界面永远转圈。
            Err(TryRecvError::Disconnected) => {
                Some(Err("本地扫描线程意外退出".to_string()))
            }
        }
    }
}

/// 两侧用来喂决策层的 `mtime`。见模块头那张表。
///
/// 只对「两侧都有这一条」的情形有意义：单侧存在的条目走的是判定表的
/// 「只有 `L`」/「只有 `R`」两支，根本不看 `mtime`。
fn effective_mtime(local: Option<i64>, remote: Option<i64>) -> (i64, i64) {
    match (local, remote) {
        (Some(l), Some(r)) => (l, r),
        // 一侧未知：把它设成对侧的值，让决策层看到「时间相同」→ 退到比 size。
        (Some(l), None) => (l, l),
        (None, Some(r)) => (r, r),
        // 两侧都不知道：都设 0，同样落到「时间相同 → 比 size」。
        (None, None) => (0, 0),
    }
}

/// 一条采集结果 → 决策层的条目，`mtime` 用调用方算好的那个。
fn sync_entry(e: &ScannedEntry, mtime: i64) -> SyncEntry {
    SyncEntry {
        path: e.rel.clone(),
        is_dir: e.is_dir,
        size: e.size,
        mtime,
    }
}

/// 两份清单 → 一份计划。这是本文件唯一对外的主力函数。
///
/// 合并口径（同路径后出现者覆盖先出现者）与 `sync::plan` 内部一致：先把并集收成
/// 一张表、顺手把 `mtime` 归一，再交给决策层。**不在这里判方向**——方向是
/// `term_xfer::sync` 的事，两张表分开写迟早分叉。
///
/// 输出按路径升序（决策层用 `BTreeMap` 保证），包含 `Skip` 项，调用方自己按
/// [`SyncOp`] 分组。
pub fn build_plan(
    local: &[ScannedEntry],
    remote: &[ScannedEntry],
    deletes: DeletePolicy,
) -> Vec<SyncItem> {
    // 路径 → (本地项, 远端项)。用 `&str` 做键，避免把整棵树 clone 一份。
    let mut merged: BTreeMap<&str, (Option<&ScannedEntry>, Option<&ScannedEntry>)> = BTreeMap::new();
    for e in local {
        merged.entry(e.rel.as_str()).or_default().0 = Some(e);
    }
    for e in remote {
        merged.entry(e.rel.as_str()).or_default().1 = Some(e);
    }

    let mut left: Vec<SyncEntry> = Vec::with_capacity(merged.len());
    let mut right: Vec<SyncEntry> = Vec::with_capacity(merged.len());
    for (_, (l, r)) in merged {
        let (lm, rm) = effective_mtime(l.and_then(|e| e.mtime), r.and_then(|e| e.mtime));
        if let Some(e) = l {
            left.push(sync_entry(e, lm));
        }
        if let Some(e) = r {
            right.push(sync_entry(e, rm));
        }
    }
    sync::plan(&left, &right, deletes)
}

/// 一份计划按操作分类的条数，给 Review 页的汇总行用。
///
/// 为什么不让 UI 每次自己 `filter().count()`：那些数字要在「文案模板」与「按钮
/// 是否可用」两处用，两处各数一遍迟早不一致（比如删除按钮按 `deletes` 判可用、
/// 汇总按实际条数算）。数一次、传一份。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// 待上传条数。
    pub upload: usize,
    /// 待下载条数。
    pub download: usize,
    /// 待删本地条数。
    pub delete_local: usize,
    /// 待删远端条数。
    pub delete_remote: usize,
    /// 已一致条数。
    pub skip: usize,
}

impl Summary {
    /// 数一份计划。
    pub fn of(items: &[SyncItem]) -> Self {
        let mut s = Summary::default();
        for item in items {
            match item.op {
                SyncOp::Upload => s.upload += 1,
                SyncOp::Download => s.download += 1,
                SyncOp::DeleteLocal => s.delete_local += 1,
                SyncOp::DeleteRemote => s.delete_remote += 1,
                SyncOp::Skip => s.skip += 1,
            }
        }
        s
    }

    /// 会产生写操作的条数（= 执行阶段要走的步数）。`SyncOp::is_mutation` 的镜像。
    pub fn mutations(&self) -> usize {
        self.upload + self.download + self.delete_local + self.delete_remote
    }

    /// 有没有「要删东西」的项——Review 页据此决定确认按钮是否需要二次确认文案。
    pub fn has_deletes(&self) -> bool {
        self.delete_local + self.delete_remote > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(rel: &str, size: u64, mtime: Option<i64>) -> ScannedEntry {
        ScannedEntry {
            rel: rel.to_string(),
            is_dir: false,
            size,
            mtime,
        }
    }

    fn d(rel: &str, mtime: Option<i64>) -> ScannedEntry {
        ScannedEntry {
            rel: rel.to_string(),
            is_dir: true,
            size: 0,
            mtime,
        }
    }

    fn ops(items: &[SyncItem]) -> Vec<(&str, SyncOp)> {
        items
            .iter()
            .map(|i| (i.entry.path.as_str(), i.op))
            .collect()
    }

    #[test]
    fn a_first_sync_uploads_everything_and_a_second_run_skips_everything() {
        // 这是最有价值的一条判据：它同时证明采集（mtime 到手）、归一
        // （两侧同一口径）、决策（判定表）三处都对。
        //
        // 它只喂合成清单——真机上那条链由三段拼起来，每一段都有自己的判据：
        //   1. `term-sftp` 的 e2e 证**采集**带得回 mtime，且两端顺序/口径逐条一致；
        //   2. 同文件的 e2e 证 **`preserve`** 双向把 mtime 写过去（上传后远端
        //      mtime == 本地 mtime，下载反之）——没有它第二趟永远收敛不到 Skip；
        //   3. 本测试证 **mtime 一致时判定表给 Skip**。
        // 三段各自可独立复现，比一条「起服务端 + 传完再采」的巨型用例更好定位。
        let local = vec![f("a.txt", 3, Some(1_700_000_000)), d("sub", Some(1_700_000_000))];
        // 空远端 = 第一次同步。
        let first = build_plan(&local, &[], DeletePolicy::Keep);
        assert_eq!(
            ops(&first),
            [("a.txt", SyncOp::Upload), ("sub", SyncOp::Upload)],
            "空远端时全部上传，顺序按路径升序"
        );
        assert_eq!(Summary::of(&first).mutations(), 2);

        // 跑完之后远端就是本地那份（`preserve` 让 mtime 也一致）。重采 = 全 Skip。
        let second = build_plan(&local, &local, DeletePolicy::Keep);
        assert_eq!(
            ops(&second),
            [("a.txt", SyncOp::Skip), ("sub", SyncOp::Skip)],
            "第二趟必须全部 Skip——有哪条还在动，就说明 mtime 没落回去"
        );
        assert_eq!(Summary::of(&second).mutations(), 0);
    }

    #[test]
    fn the_newer_side_wins_and_ties_fall_back_to_size() {
        let local = vec![
            f("l-newer", 3, Some(1_700_000_100)),
            f("r-newer", 3, Some(1_700_000_000)),
            f("tie-same-size", 3, Some(1_700_000_000)),
            f("tie-diff-size", 9, Some(1_700_000_000)),
        ];
        let remote = vec![
            f("l-newer", 3, Some(1_700_000_000)),
            f("r-newer", 3, Some(1_700_000_100)),
            f("tie-same-size", 3, Some(1_700_000_000)),
            f("tie-diff-size", 3, Some(1_700_000_000)),
        ];
        let items = build_plan(&local, &remote, DeletePolicy::Keep);
        assert_eq!(
            ops(&items),
            [
                ("l-newer", SyncOp::Upload),
                ("r-newer", SyncOp::Download),
                ("tie-diff-size", SyncOp::Upload),
                ("tie-same-size", SyncOp::Skip),
            ]
        );
    }

    #[test]
    fn an_unknown_mtime_falls_back_to_size_instead_of_claiming_a_direction() {
        // 远端没报 mtime：既不能当 0（会全量上传），也不能当 MAX（会全量下载）。
        // 归一之后决策层看到「时间相同」，于是：内容大小一致就 Skip，不一致按本地优先。
        let local = vec![f("same", 5, Some(1_700_000_000)), f("differ", 5, Some(1_700_000_000))];
        let remote = vec![f("same", 5, None), f("differ", 7, None)];
        let items = build_plan(&local, &remote, DeletePolicy::Keep);
        assert_eq!(
            ops(&items),
            [("differ", SyncOp::Upload), ("same", SyncOp::Skip)],
            "未知时间不许伪造方向：大小相同就跳过，不同才动手"
        );
    }

    #[test]
    fn both_sides_unknown_still_compares_sizes() {
        let local = vec![f("a", 5, None)];
        let remote = vec![f("a", 5, None)];
        assert_eq!(
            build_plan(&local, &remote, DeletePolicy::Keep)[0].op,
            SyncOp::Skip
        );
    }

    #[test]
    fn the_delete_policy_only_affects_one_sided_entries() {
        let local = vec![f("only-local", 1, Some(1)), f("both", 1, Some(1))];
        let remote = vec![f("only-remote", 1, Some(1)), f("both", 1, Some(1))];

        // 默认 Keep：单侧的都往对面复制，一项都不删。
        let keep = Summary::of(&build_plan(&local, &remote, DeletePolicy::Keep));
        assert_eq!((keep.upload, keep.download, keep.skip), (1, 1, 1));
        assert!(!keep.has_deletes());

        let del_remote = Summary::of(&build_plan(&local, &remote, DeletePolicy::DeleteRemote));
        assert_eq!((del_remote.upload, del_remote.delete_remote), (1, 1));

        let del_local = Summary::of(&build_plan(&local, &remote, DeletePolicy::DeleteLocal));
        assert_eq!((del_local.download, del_local.delete_local), (1, 1));
    }

    #[test]
    fn a_summary_counts_what_the_plan_says_and_nothing_else() {
        let items = vec![
            SyncItem {
                entry: SyncEntry::file("a", 1, 1),
                op: SyncOp::Upload,
            },
            SyncItem {
                entry: SyncEntry::file("b", 1, 1),
                op: SyncOp::Download,
            },
            SyncItem {
                entry: SyncEntry::file("c", 1, 1),
                op: SyncOp::Skip,
            },
            SyncItem {
                entry: SyncEntry::file("d", 1, 1),
                op: SyncOp::DeleteRemote,
            },
        ];
        let s = Summary::of(&items);
        assert_eq!(s.mutations(), 3, "Skip 不算一步");
        assert!(s.has_deletes());
    }
}
