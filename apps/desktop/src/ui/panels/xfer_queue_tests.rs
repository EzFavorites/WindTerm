//! `xfer_queue` 的单测：喂假事件把整条状态机走一遍，不需要真的 SSH。
//!
//! 单独一个文件是因为（`ui/` 单文件代码行 ≤ 500）：逻辑与用例放一起已经
//! 顶到边上，再往里加一条就得砍注释。这里测的都是「模型层 + 这张路径表」咬合处
//! 的判定，模型层自己的规则（退避算法、总进度分母）在 `crates/term-xfer` 里
//! 已经钉过了，不重复。
//!
//! 树传输的事件流照抄 `term-sftp::background` / `walk`：每个文件一组
//! `Progress` + `Done`（`path` 是文件的远端路径），最后由树根的 `Done` 收尾；
//! 失败 / 取消挂在树根上。这里拿假事件把「一行对一棵树」的归账走完。

use super::*;

fn queue() -> XferQueue {
    XferQueue::new()
}

#[test]
fn only_one_transfer_runs_at_a_time() {
    let mut q = queue();
    let first = q.enqueue_download("/r/a.txt", "/l/a.txt", 10).expect("入队");
    q.enqueue_download("/r/b.txt", "/l/b.txt", 10).expect("入队");

    assert_eq!(q.next_to_run(0).map(|t| t.id), Some(first));
    // 后台是一条线程串行消费的，界面上也就只能有一项在跑；这里起第二项的话，
    // 它会显示「传输中」却实际堵在通道里——假进度。
    assert!(q.next_to_run(0).is_none(), "第一项没结束前不许起第二项");

    q.on_done("/r/a.txt", 0);
    assert_eq!(
        q.next_to_run(0).map(|t| t.remote),
        Some("/r/b.txt".to_string()),
        "前一项结束后才轮到下一项"
    );
}

#[test]
fn trailing_slashes_do_not_create_a_second_task() {
    let mut q = queue();
    q.enqueue_upload_tree("/srv", "/tmp/src").expect("入队");
    assert!(
        q.enqueue_upload_tree("/srv/", "/tmp/src").is_none(),
        "/srv 与 /srv/ 是同一棵树，不能各占一行"
    );
    // 单文件路径同样要归一；根 '/' 保留，不能被当空串。
    let mut q = queue();
    q.enqueue_download("/r/a", "/l/a", 5).expect("入队");
    assert!(q.enqueue_download("/r/a/", "/l/a", 5).is_none());
    assert_eq!(queue_path("/"), "/");
    assert_eq!(queue_path("/srv//"), "/srv");
}

#[test]
fn progress_moves_the_overall_numerator() {
    let mut q = queue();
    q.enqueue_download("/r/a.bin", "/l/a.bin", 1000);
    assert_eq!(q.overall(), (0, 1000));
    q.next_to_run(0).expect("该跑");
    q.on_progress("/r/a.bin", 400);
    assert_eq!(q.overall(), (400, 1000));
    // 迟到的旧进度不许把条子拖回去。
    q.on_progress("/r/a.bin", 100);
    assert_eq!(q.overall(), (400, 1000));
    q.on_done("/r/a.bin", 0);
    assert_eq!(q.overall(), (1000, 1000), "Done 按 size 全额计入");
}

#[test]
fn a_failed_transfer_comes_back_after_the_backoff() {
    let mut q = queue();
    let id = q.enqueue_upload("/l/a", "/r/a", 10).expect("入队");
    q.next_to_run(0).expect("该跑");
    q.on_failed("/r/a", "Permission denied", 100);

    // retry_backoff = 2：失败在 100 秒，最早 102 秒才能再跑。
    assert!(q.next_to_run(100).is_none(), "退避没到点不许跑");
    assert_eq!(q.retry(101), Vec::<u64>::new(), "101 秒还没到点");
    assert_eq!(q.retry(102), vec![id], "到点后要退回来");

    let again = q.next_to_run(103).expect("退回 Pending 后能重新拿到");
    assert_eq!(again.id, id);
    assert!(!again.tree, "单文件任务不带 tree 标记");
    let row = &q.rows()[0];
    assert_eq!(row.attempts, 1, "重试次数要留痕，面板要显示「第 n 次重试」");
    assert_eq!(
        row.error.as_deref(),
        Some("Permission denied"),
        "服务端原话要留着，转述成「失败」等于把线索擦掉"
    );
}

#[test]
fn a_cancelled_pending_job_never_runs() {
    let mut q = queue();
    q.enqueue_download("/r/a", "/l/a", 5);
    let second = q.enqueue_download("/r/b", "/l/b", 5).expect("入队");
    assert!(q.cancel(second));
    assert_eq!(q.next_to_run(0).map(|t| t.remote), Some("/r/a".to_string()));
    q.on_done("/r/a", 0);
    assert!(q.next_to_run(0).is_none(), "取消过的项永远不该被捞起来");
    assert_eq!(q.rows()[1].state, JobState::Cancelled);
}

#[test]
fn cancelling_a_running_job_keeps_the_path_reserved_until_the_event_lands() {
    // 跑着的时候取消：模型层立刻变 `Cancelled`，但后台还会回一条 `Cancelled`
    // 事件。这条路径在事件回来之前不能放开——否则重新入队后，那条迟到的事件
    // 会把新任务当成自己的。
    let mut q = queue();
    q.enqueue_download("/r/a", "/l/a", 5).expect("入队");
    q.next_to_run(0).expect("该跑");
    assert!(q.cancel(1));
    assert!(q.enqueue_download("/r/a", "/l/a", 5).is_none(), "事件还没回来，先别放行");
    q.on_cancelled("/r/a");
    assert!(q.enqueue_download("/r/a", "/l/a", 5).is_some(), "收尾之后可以再传一次");
}

#[test]
fn events_are_matched_by_the_remote_path_not_by_position() {
    let mut q = queue();
    let a = q.enqueue_download("/r/a", "/l/a", 100).expect("入队");
    let b = q.enqueue_download("/r/b", "/l/b", 200).expect("入队");
    assert_eq!(q.next_to_run(0).map(|t| t.id), Some(a));

    // 只有 a 在跑，给 b 的进度不该被任何人收下。
    q.on_progress("/r/b", 50);
    assert_eq!(q.overall(), (0, 300), "给没在跑的那一项报进度应当被忽略");
    q.on_progress("/r/a", 30);
    assert!(
        matches!(q.rows()[0].state, JobState::Running { transferred: 30 }),
        "a 的进度要落在 a 上"
    );
    assert_eq!(q.rows()[1].state, JobState::Pending, "b 不受影响");

    q.on_done("/r/a", 0);
    assert_eq!(q.next_to_run(0).map(|t| t.id), Some(b));
}

#[test]
fn an_upload_is_keyed_by_its_remote_target() {
    // 上传的 `Event::Progress { path }` 报的是**远端**路径
    // （`background.rs::transfer_and_report` 里 `let path = remote.to_string()`），
    // 所以这张表的键必须用远端目标；用本地源当键，进度永远对不上。
    let mut q = queue();
    let id = q
        .enqueue_upload("/Users/me/a.txt", "/srv/a.txt", 10)
        .expect("入队");
    let task = q.next_to_run(0).expect("该跑");
    assert_eq!(task.id, id);
    assert_eq!(task.local, "/Users/me/a.txt");
    assert_eq!(task.remote, "/srv/a.txt");

    q.on_progress("/srv/a.txt", 10);
    assert_eq!(q.overall(), (10, 10));
    assert_eq!(q.rows()[0].name, "a.txt", "画在屏上的是文件名，不是整条路径");
}

#[test]
fn the_same_path_is_refused_while_it_is_still_busy() {
    let mut q = queue();
    assert!(q.enqueue_download("/r/a", "/l/a", 1).is_some());
    assert!(
        q.enqueue_download("/r/a", "/l/a", 1).is_none(),
        "同一路径没结束前不许重复入队"
    );
    q.next_to_run(0).expect("该跑");
    q.on_done("/r/a", 0);
    assert!(
        q.enqueue_download("/r/a", "/l/a", 1).is_some(),
        "结束之后同一个文件可以再传一次"
    );
}

#[test]
fn a_write_operation_done_event_does_not_touch_the_queue() {
    // mkdir / rename / remove / chmod 也用 `Done { bytes: 0 }` 收尾，它们不在
    // 这张表里，不许被当成某一项的完成信号。
    let mut q = queue();
    q.enqueue_download("/r/a", "/l/a", 10).expect("入队");
    q.next_to_run(0).expect("该跑");
    q.on_done("/r/some/other/dir", 0);
    assert!(
        matches!(q.rows()[0].state, JobState::Running { .. }),
        "别的路径的 Done 不该收尾这一项"
    );
    q.on_failed("/r/another", "Permission denied", 0);
    assert!(matches!(q.rows()[0].state, JobState::Running { .. }));
}

#[test]
fn retry_now_pulls_every_backoff_item_forward() {
    let mut q = queue();
    q.enqueue_download("/r/a", "/l/a", 5).expect("入队");
    q.next_to_run(0).expect("该跑");
    q.on_failed("/r/a", "boom", 1000);
    assert!(matches!(
        q.state_of(1),
        Some(JobState::Failed {
            retry_at: Some(_),
            ..
        })
    ));
    assert_eq!(q.retry_now(), 1);
    assert_eq!(q.state_of(1), Some(JobState::Pending), "提前之后回到排队");
    assert_eq!(q.next_to_run(1000).map(|t| t.remote), Some("/r/a".to_string()));
}

#[test]
fn an_empty_queue_says_so() {
    let q = queue();
    assert!(q.is_empty());
    assert!(q.all_settled(), "空队列视为全部结束，面板据此不再每帧叫醒自己");
    assert_eq!(q.overall(), (0, 0), "总字节为 0 时不许除零");
    assert!(q.rows().is_empty());
}

#[test]
fn the_last_segment_is_what_gets_drawn() {
    assert_eq!(file_name("/srv/www/a.txt"), "a.txt");
    assert_eq!(file_name("/srv/www/"), "www");
    assert_eq!(file_name("bare.txt"), "bare.txt");
    assert_eq!(file_name("C:\\Users\\me\\a.txt"), "a.txt");
}

// ── 目录树 = 队列里的一行 ────────────────────────────────────────

#[test]
fn a_directory_tree_runs_as_one_row_until_the_root_done_arrives() {
    let mut q = queue();
    let id = q
        .enqueue_download_tree("/r/www", "/l/www")
        .expect("目录入队");
    let task = q.next_to_run(0).expect("该跑");
    assert_eq!(task.id, id);
    assert!(
        task.tree,
        "目录任务要带 tree 标记，发起时改走 download_tree"
    );
    assert_eq!(task.remote, "/r/www");
    assert_eq!(task.local, "/l/www");
    assert_eq!(q.rows().len(), 1, "整棵树只占一行");

    // 文件 1：进度从 0 起，Done 挂在该文件的远端路径上。
    q.on_progress("/r/www/a.txt", 40);
    q.on_progress("/r/www/a.txt", 100);
    assert!(matches!(
        q.rows()[0].state,
        JobState::Running { transferred: 100 }
    ));
    q.on_done("/r/www/a.txt", 100);
    assert!(
        matches!(q.rows()[0].state, JobState::Running { .. }),
        "第一个文件的 Done 不许把整棵树标成完成"
    );
    assert_eq!(q.rows().len(), 1);

    // 文件 2：每个文件的进度从 0 重来，累计值仍要递增。
    q.on_progress("/r/www/sub/b.bin", 25);
    assert!(matches!(
        q.rows()[0].state,
        JobState::Running { transferred: 125 }
    ));
    q.on_progress("/r/www/sub/b.bin", 50);
    q.on_done("/r/www/sub/b.bin", 50);
    assert!(matches!(
        q.rows()[0].state,
        JobState::Running { transferred: 150 }
    ));

    // 文件 3：空文件也发一条 Done（bytes = 0），不许改变累计值。
    q.on_done("/r/www/empty", 0);
    assert!(matches!(
        q.rows()[0].state,
        JobState::Running { transferred: 150 }
    ));

    // 树根收尾：bytes 是整棵树的总字节，只有这一条才结算。
    q.on_done("/r/www", 150);
    assert_eq!(q.rows()[0].state, JobState::Done);
    assert_eq!(q.rows().len(), 1, "不许多出悬挂行");
    assert!(q.all_settled());
    assert_eq!(
        q.overall(),
        (150, 150),
        "树根 Done 的 bytes 成为这一行的分母"
    );
    assert!(q.by_path.is_empty(), "收尾后反查表要摘干净");
    assert!(q.tree_roots.is_empty(), "树根映射也要摘干净");

    // 迟到的后续事件不许污染终态，也不许再造出新行。
    q.on_progress("/r/www/sub/b.bin", 999);
    q.on_done("/r/www/another.txt", 10);
    q.on_failed("/r/www/late.txt", "too late", 999);
    assert_eq!(q.rows()[0].state, JobState::Done);
    assert_eq!(q.rows().len(), 1);
}

#[test]
fn an_upload_tree_is_also_one_row_and_keeps_its_direction() {
    let mut q = queue();
    let id = q.enqueue_upload_tree("/l/www", "/r/www").expect("目录入队");
    let task = q.next_to_run(0).expect("该跑");
    assert_eq!(task.id, id);
    assert!(task.tree);
    assert_eq!(task.direction, Direction::Upload);
    assert_eq!(task.remote, "/r/www", "事件口径仍是远端路径");

    q.on_progress("/r/www/x.txt", 30);
    q.on_done("/r/www/x.txt", 30);
    q.on_done("/r/www", 30);
    assert_eq!(q.rows()[0].state, JobState::Done);
    assert_eq!(q.rows()[0].direction, Direction::Upload);
    assert!(q.by_path.is_empty() && q.tree_roots.is_empty());
}

#[test]
fn a_file_failure_fails_the_whole_tree_and_later_events_do_not_pollute_it() {
    let mut q = queue();
    q.enqueue_download_tree("/r/www", "/l/www").expect("入队");
    q.next_to_run(0).expect("该跑");
    q.on_progress("/r/www/a.txt", 30);
    q.on_done("/r/www/a.txt", 30);

    // 任一文件失败，后台把 Failed 挂在树根上（tree_and_report::stop_event）。
    q.on_failed("/r/www", "Permission denied", 100);
    assert!(matches!(
        q.rows()[0].state,
        JobState::Failed {
            retry_at: Some(_),
            ..
        }
    ));
    assert_eq!(q.rows()[0].error.as_deref(), Some("Permission denied"));
    assert_eq!(q.rows().len(), 1, "失败不许多出一行");

    // 退避窗口里这一行没结算，但后续事件不许把它改回 Running 或 Done。
    q.on_progress("/r/www/b.txt", 999);
    q.on_done("/r/www/b.txt", 999);
    q.on_done("/r/www", 999);
    assert!(
        matches!(q.rows()[0].state, JobState::Failed { .. }),
        "退避里的失败态不许被迟到的进度 / Done 污染"
    );
    assert_eq!(q.rows().len(), 1);

    // 到点重跑：仍走树传输，且上一次的树进度清零，重新从 0 起。
    assert_eq!(q.retry(102), vec![1]);
    let again = q.next_to_run(103).expect("退回 Pending 后能重跑");
    assert!(again.tree, "重试仍走 download_tree");
    assert!(matches!(
        q.rows()[0].state,
        JobState::Running { transferred: 0 }
    ));
    q.on_progress("/r/www/a.txt", 10);
    assert!(matches!(
        q.rows()[0].state,
        JobState::Running { transferred: 10 }
    ));
}

#[test]
fn a_cancelled_tree_lands_on_cancelled_without_a_dangling_row() {
    let mut q = queue();
    q.enqueue_upload_tree("/l/www", "/r/www").expect("入队");
    q.next_to_run(0).expect("该跑");
    q.on_progress("/r/www/a.txt", 10);

    // 用户点取消：模型层立刻 Cancelled，后台随后回一条树根 Cancelled 事件。
    assert!(q.cancel(1));
    q.on_cancelled("/r/www");
    assert_eq!(q.rows()[0].state, JobState::Cancelled);
    assert_eq!(q.rows().len(), 1);
    assert!(q.all_settled());
    assert!(
        q.by_path.is_empty() && q.tree_roots.is_empty(),
        "取消后两张反查表都要摘干净，不留悬挂映射"
    );
    assert!(
        q.enqueue_upload_tree("/l/www", "/r/www").is_some(),
        "取消之后同一个目录可以重新发起"
    );
}

#[test]
fn a_terminal_tree_failure_releases_the_root() {
    let mut q = queue();
    q.enqueue_download_tree("/r/www", "/l/www").expect("入队");
    // max_retries = 3：第四次 Failed 是终态（retry_at = None）。
    for now in [0, 10, 20, 30] {
        q.next_to_run(now).expect("该跑");
        q.on_failed("/r/www", "Permission denied", now);
        q.retry(now + 100);
    }
    assert!(matches!(
        q.rows()[0].state,
        JobState::Failed { retry_at: None, .. }
    ));
    assert_eq!(q.rows().len(), 1);
    assert!(
        q.by_path.is_empty() && q.tree_roots.is_empty(),
        "终态失败也要把树根映射摘掉"
    );
    assert!(
        q.enqueue_download_tree("/r/www", "/l/www").is_some(),
        "终态失败后同一个目录可以重新发起"
    );
}

#[test]
fn only_true_descendants_of_a_tree_root_are_matched_by_prefix() {
    let mut q = queue();
    q.enqueue_download_tree("/r/www", "/l/www").expect("入队");
    q.next_to_run(0).expect("该跑");

    // 只差一个字符的兄弟目录不是后代：/r/www2/a.txt 不许被 /r/www 吃掉。
    q.on_progress("/r/www2/a.txt", 500);
    q.on_done("/r/www2/a.txt", 0);
    assert!(matches!(
        q.rows()[0].state,
        JobState::Running { transferred: 0 }
    ));
    // 单文件写操作的 Done 也不在这棵树的范围里。
    q.on_done("/r/www-other", 0);
    assert!(matches!(
        q.rows()[0].state,
        JobState::Running { transferred: 0 }
    ));

    q.on_progress("/r/www/a.txt", 20);
    assert!(matches!(
        q.rows()[0].state,
        JobState::Running { transferred: 20 }
    ));
    // 根目录做树根时，绝对路径都算后代。
    assert!(is_under("/etc/hosts", "/"));
    assert!(!is_under("relative/hosts", "/"));
    assert!(!is_under("/r/www", "/r/www"));
}

#[test]
fn overlapping_paths_are_refused_while_a_tree_is_live() {
    let mut q = queue();
    q.enqueue_download_tree("/r/www", "/l/www").expect("入队");

    assert!(
        q.enqueue_download("/r/www/a.txt", "/l/www/a.txt", 10)
            .is_none(),
        "树里的文件不许另起一行（事件会被前缀与精确两条路抢）"
    );
    assert!(
        q.enqueue_download_tree("/r/www/sub", "/l/www/sub")
            .is_none(),
        "与树根重叠的子树也不许"
    );
    assert!(
        q.enqueue_download_tree("/r", "/l").is_none(),
        "新树把已有路径罩进去同样不许"
    );
    assert!(
        q.enqueue_download("/r/other.txt", "/l/other.txt", 10)
            .is_some(),
        "树外的文件照常入队"
    );
    assert_eq!(q.rows().len(), 2, "被拒的三条都没进队列");
}
