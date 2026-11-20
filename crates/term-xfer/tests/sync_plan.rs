//! 目录同步计划器的行为测试。
//!
//! 全部用纯数据构造清单：不建临时目录、不读盘、不看时钟。这样每条判定规则都能
//! 单独钉死，失败时报的是「规则错了」而不是「这台机器的文件系统精度不一样」。

use term_xfer::{plan, DeletePolicy, SyncEntry, SyncItem, SyncOp};

fn file(path: &str, size: u64, mtime: i64) -> SyncEntry {
    SyncEntry::file(path, size, mtime)
}

fn dir(path: &str, mtime: i64) -> SyncEntry {
    SyncEntry::dir(path, mtime)
}

/// 按路径取出计划项（不存在就让测试失败，而不是静默跳过）。
fn op_of(items: &[SyncItem], path: &str) -> SyncOp {
    items
        .iter()
        .find(|item| item.entry.path == path)
        .unwrap_or_else(|| panic!("计划里没有 {path}：{items:?}"))
        .op
}

#[test]
fn local_only_uploads() {
    let items = plan(&[file("a.txt", 10, 100)], &[], DeletePolicy::Keep);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].op, SyncOp::Upload);
    assert_eq!(items[0].entry, file("a.txt", 10, 100));
}

#[test]
fn remote_only_downloads() {
    let items = plan(&[], &[file("b.txt", 20, 200)], DeletePolicy::Keep);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].op, SyncOp::Download);
    // 下载项携带的是远端条目本身。
    assert_eq!(items[0].entry, file("b.txt", 20, 200));
}

#[test]
fn local_newer_uploads() {
    // 大小相同、本地 mtime 更大 → 本地更新。
    let items = plan(
        &[file("a.txt", 10, 1_700_000_005)],
        &[file("a.txt", 10, 1_700_000_000)],
        DeletePolicy::Keep,
    );
    assert_eq!(op_of(&items, "a.txt"), SyncOp::Upload);
    // 大小变了也是同一判定：mtime 说了算。
    let items = plan(
        &[file("a.txt", 11, 1_700_000_005)],
        &[file("a.txt", 10, 1_700_000_000)],
        DeletePolicy::Keep,
    );
    assert_eq!(op_of(&items, "a.txt"), SyncOp::Upload);
}

#[test]
fn remote_newer_downloads() {
    let items = plan(
        &[file("a.txt", 10, 1_700_000_000)],
        &[file("a.txt", 10, 1_700_000_005)],
        DeletePolicy::Keep,
    );
    assert_eq!(op_of(&items, "a.txt"), SyncOp::Download);
    assert_eq!(items[0].entry, file("a.txt", 10, 1_700_000_005));
}

#[test]
fn identical_size_and_mtime_skips() {
    let items = plan(
        &[file("a.txt", 42, 1_700_000_000)],
        &[file("a.txt", 42, 1_700_000_000)],
        DeletePolicy::Keep,
    );
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].op, SyncOp::Skip);
    assert!(!items[0].op.is_mutation());
    // Skip 项取本地条目。
    assert_eq!(items[0].entry, file("a.txt", 42, 1_700_000_000));
}

#[test]
fn local_only_kept_when_policy_keeps() {
    // 默认策略：绝不因为「对端没有」就删本地文件，而是补传上去。
    let local = [file("a.txt", 10, 100), dir("sub", 100)];
    let items = plan(&local, &[], DeletePolicy::Keep);
    assert_eq!(op_of(&items, "a.txt"), SyncOp::Upload);
    assert_eq!(op_of(&items, "sub"), SyncOp::Upload, "目录也要在远端创建");
    assert_eq!(items.len(), 2);
}

#[test]
fn remote_only_deleted_when_policy_deletes_remote() {
    // 远端镜像本地：远端独有的条目删除。
    let items = plan(
        &[file("keep.txt", 1, 10)],
        &[file("keep.txt", 1, 10), file("stale.txt", 2, 20)],
        DeletePolicy::DeleteRemote,
    );
    assert_eq!(op_of(&items, "keep.txt"), SyncOp::Skip);
    assert_eq!(op_of(&items, "stale.txt"), SyncOp::DeleteRemote);
    // 删除项携带远端条目。
    assert_eq!(
        items
            .iter()
            .find(|item| item.entry.path == "stale.txt")
            .map(|item| &item.entry),
        Some(&file("stale.txt", 2, 20))
    );
}

#[test]
fn local_only_deleted_when_policy_deletes_local() {
    // 本地镜像远端：本地独有的条目删除，远端独有的照常下载。
    let items = plan(
        &[file("stale.txt", 2, 20), file("only_local.txt", 3, 30)],
        &[file("fresh.txt", 4, 40)],
        DeletePolicy::DeleteLocal,
    );
    assert_eq!(op_of(&items, "stale.txt"), SyncOp::DeleteLocal);
    assert_eq!(op_of(&items, "only_local.txt"), SyncOp::DeleteLocal);
    assert_eq!(op_of(&items, "fresh.txt"), SyncOp::Download);
}

#[test]
fn mtime_second_granularity_boundary() {
    // 1 秒差就算「更新」：本模块不做容差，精度归一化属于采集阶段。
    let items = plan(
        &[file("a.txt", 10, 1_700_000_001)],
        &[file("a.txt", 10, 1_700_000_000)],
        DeletePolicy::Keep,
    );
    assert_eq!(op_of(&items, "a.txt"), SyncOp::Upload);

    let items = plan(
        &[file("a.txt", 10, 1_700_000_000)],
        &[file("a.txt", 10, 1_700_000_001)],
        DeletePolicy::Keep,
    );
    assert_eq!(op_of(&items, "a.txt"), SyncOp::Download);

    // 秒数相等但大小不同：时间戳分不出先后，约定以本地为准。
    let items = plan(
        &[file("a.txt", 10, 1_700_000_000)],
        &[file("a.txt", 11, 1_700_000_000)],
        DeletePolicy::Keep,
    );
    assert_eq!(op_of(&items, "a.txt"), SyncOp::Upload);

    // 极端值（含 1970 年前与 i64 上界）在精确比较下仍按相等处理。
    let items = plan(
        &[file("old.txt", 0, -1), file("max.txt", 0, i64::MAX)],
        &[file("old.txt", 0, -1), file("max.txt", 0, i64::MAX)],
        DeletePolicy::Keep,
    );
    assert_eq!(op_of(&items, "old.txt"), SyncOp::Skip);
    assert_eq!(op_of(&items, "max.txt"), SyncOp::Skip);
}

#[test]
fn empty_listings_yield_empty_plan() {
    let items = plan(&[], &[], DeletePolicy::Keep);
    assert!(items.is_empty());
    // 删除策略不该凭空造出条目。
    let items = plan(&[], &[], DeletePolicy::DeleteRemote);
    assert!(items.is_empty());
}

#[test]
fn dirs_skip_and_type_mismatch_uploads() {
    // 两侧都是目录：直接跳过，目录 mtime 不参与比较（子项增删都会改它）。
    let items = plan(
        &[dir("sub", 1_700_000_000)],
        &[dir("sub", 1_700_009_999)],
        DeletePolicy::Keep,
    );
    assert_eq!(op_of(&items, "sub"), SyncOp::Skip);

    // 一边目录一边文件：类型冲突，以本地为准。
    let items = plan(
        &[dir("weird", 100)],
        &[file("weird", 7, 200)],
        DeletePolicy::Keep,
    );
    assert_eq!(op_of(&items, "weird"), SyncOp::Upload);
    assert!(items[0].entry.is_dir, "冲突时携带本地条目");
}

#[test]
fn plan_covers_union_sorted_by_path() {
    let items = plan(
        &[file("z.txt", 1, 10), file("b.txt", 2, 20), dir("m", 30)],
        &[file("b.txt", 2, 20), file("c.txt", 3, 30)],
        DeletePolicy::Keep,
    );
    let paths: Vec<&str> = items.iter().map(|item| item.entry.path.as_str()).collect();
    // 并集完整且按路径升序（BTreeMap 的顺序），Skip 项也在计划里。
    assert_eq!(paths, ["b.txt", "c.txt", "m", "z.txt"]);
    assert_eq!(op_of(&items, "b.txt"), SyncOp::Skip);
    assert_eq!(op_of(&items, "c.txt"), SyncOp::Download);
    assert_eq!(op_of(&items, "m"), SyncOp::Upload);
    assert_eq!(op_of(&items, "z.txt"), SyncOp::Upload);
    assert_eq!(items.iter().filter(|i| i.op.is_mutation()).count(), 3);
}
