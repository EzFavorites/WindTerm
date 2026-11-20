//! 同步执行器的行为测试。
//!
//! 全部用纯数据：不建目录、不读盘、不看时钟。这一层唯一会「真出错」的地方就是顺序
//! 与连坐，而顺序错了报的是对端的一句话（`ENOTEMPTY` / `No such file or directory`），
//! 看不出根因——所以每一条顺序契约都单独钉死。

use term_xfer::{
    execution_order, plan, DeletePolicy, StepResult, SyncEntry, SyncItem, SyncOp, SyncRun,
};

fn file(path: &str, size: u64, mtime: i64) -> SyncEntry {
    SyncEntry::file(path, size, mtime)
}

fn dir(path: &str, mtime: i64) -> SyncEntry {
    SyncEntry::dir(path, mtime)
}

/// 计划里各条目的路径，按执行顺序排好——断言顺序时看这个，比下标好读。
fn order_paths(items: &[SyncItem]) -> Vec<&str> {
    execution_order(items)
        .into_iter()
        .map(|index| items[index].entry.path.as_str())
        .collect()
}

/// 一个被上传的条目，只给路径。
fn upload(path: &str, is_dir: bool) -> SyncEntry {
    if is_dir {
        dir(path, 100)
    } else {
        file(path, 10, 100)
    }
}

#[test]
fn uploads_create_the_parent_directory_before_the_file_inside_it() {
    // 本地独有 = 全上传。`a` 必须在 `a/b.txt` 之前，否则上传文件时父目录还不存在。
    let items = plan(
        &[
            upload("a", true),
            upload("a/b.txt", false),
            upload("a-b.txt", false),
        ],
        &[],
        DeletePolicy::Keep,
    );
    assert_eq!(order_paths(&items), vec!["a", "a-b.txt", "a/b.txt"]);
}

#[test]
fn deletes_remove_the_child_before_the_parent_and_run_after_every_write() {
    // 远端独有 + `DeleteRemote` = 全删除。父目录必须先留着，等子项删完才轮到它——
    // 反过来会拿到「目录非空」。同时删除整体排在写操作之后。
    let local = [file("keep.txt", 5, 100)];
    let remote = [dir("a", 100), file("a/b.txt", 10, 100), file("z.txt", 3, 100)];
    let items = plan(&local, &remote, DeletePolicy::DeleteRemote);

    let paths = order_paths(&items);
    assert_eq!(paths, vec!["keep.txt", "z.txt", "a/b.txt", "a"]);
    // 最后一条是那个目录，且它在子项之后。
    assert_eq!(items[execution_order(&items)[3]].op, SyncOp::DeleteRemote);
}

#[test]
fn already_in_sync_items_stay_in_the_plan_but_never_enter_the_run() {
    // 两份清单完全一致 → 全是 Skip。它们要留在计划里（界面要显示「共 2 项，已一致
    // 2 项」），但不该被执行器往外发。
    let items = plan(
        &[file("a.txt", 10, 100), dir("d", 100)],
        &[file("a.txt", 10, 100), dir("d", 100)],
        DeletePolicy::Keep,
    );
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|item| item.op == SyncOp::Skip));

    let mut run = SyncRun::new(items);
    assert!(run.step().is_none(), "没有可执行项");
    assert!(run.is_finished());
    let progress = run.progress();
    assert_eq!((progress.total, progress.already, progress.todo), (2, 2, 0));
    assert_eq!(progress.remaining(), 0);
}

#[test]
fn a_failed_directory_skips_its_children_of_the_same_operation() {
    // 目录 `a` 传不上去（权限 / 对端拒绝），它下面的 `a/b.txt` 注定失败：挨个撞一遍
    // 只会用两条一模一样的错误盖住真正的原因。`a-c.txt` 不在 `a` 底下，照常执行。
    let items = plan(
        &[
            upload("a", true),
            upload("a/b.txt", false),
            upload("a-c.txt", false),
        ],
        &[],
        DeletePolicy::Keep,
    );
    let mut run = SyncRun::new(items);

    assert_eq!(run.step().map(|i| i.entry.path.as_str()), Some("a"));
    run.record(StepResult::Failed);

    assert_eq!(run.step().map(|i| i.entry.path.as_str()), Some("a-c.txt"));
    run.record(StepResult::Done);

    assert!(run.step().is_none(), "子项应被连坐跳过");
    let progress = run.progress();
    assert_eq!(progress.todo, 3);
    assert_eq!(progress.done, 1);
    assert_eq!(progress.failed, 1);
    assert_eq!(progress.skipped, 1);
    assert_eq!(progress.remaining(), 0);
}

#[test]
fn a_failed_file_does_not_block_anything_else() {
    // 只是这一个文件传不上去，与后面无关。
    let items = plan(
        &[upload("a.txt", false), upload("b.txt", false)],
        &[],
        DeletePolicy::Keep,
    );
    let mut run = SyncRun::new(items);
    assert_eq!(run.step().map(|i| i.entry.path.as_str()), Some("a.txt"));
    run.record(StepResult::Failed);
    assert_eq!(run.step().map(|i| i.entry.path.as_str()), Some("b.txt"));
    run.record(StepResult::Done);
    assert!(run.step().is_none());
    assert_eq!(run.progress().skipped, 0, "文件失败不该连坐");
}

#[test]
fn a_failed_upload_directory_does_not_block_a_download_below_it() {
    // 连坐只限「同一操作」：上传失败说明的是远端写不进去，与「从远端取下来」无关。
    let items = vec![
        SyncItem {
            entry: dir("a", 100),
            op: SyncOp::Upload,
        },
        SyncItem {
            entry: file("a/x.txt", 10, 100),
            op: SyncOp::Download,
        },
    ];
    let mut run = SyncRun::new(items);
    assert_eq!(run.step().map(|i| i.op), Some(SyncOp::Upload));
    run.record(StepResult::Failed);
    assert_eq!(
        run.step().map(|i| i.entry.path.as_str()),
        Some("a/x.txt"),
        "下载不该被上传的失败连坐"
    );
}

#[test]
fn recording_a_step_after_the_run_is_finished_changes_nothing() {
    // 记账疏漏不该变成崩溃：多记一次就是什么都不做。
    let items = plan(&[upload("a.txt", false)], &[], DeletePolicy::Keep);
    let mut run = SyncRun::new(items);
    let _ = run.step();
    run.record(StepResult::Done);
    assert!(run.is_finished());
    let before = run.progress();
    run.record(StepResult::Done);
    run.record(StepResult::Failed);
    assert_eq!(run.progress(), before);
}

#[test]
fn an_empty_plan_finishes_immediately_with_a_zero_progress() {
    let mut run = SyncRun::new(Vec::new());
    assert!(run.step().is_none());
    assert!(run.is_finished());
    assert_eq!(run.progress(), term_xfer::Progress::default());
    assert!(run.items().is_empty());
}
