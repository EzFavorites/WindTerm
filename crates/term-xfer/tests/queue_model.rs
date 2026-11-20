//! 传输队列模型的行为测试。
//!
//! 全部用纯数据构造：不读盘、不看真实时钟（`now` 一律是手写的整数），所以每条策略
//! 都能单独钉死，失败时报的是「规则错了」而不是「这台机器的时钟精度不一样」。

use term_xfer::{Direction, FailReason, JobState, Policy, Queue};

/// 造策略：并行上限 / 重试上限 / 退避基准（秒）。
fn policy(max_parallel: usize, max_retries: u32, retry_backoff: u64) -> Policy {
    Policy {
        max_parallel,
        max_retries,
        retry_backoff,
    }
}

/// 造队列。
fn queue(max_parallel: usize, max_retries: u32, retry_backoff: u64) -> Queue {
    Queue::new(policy(max_parallel, max_retries, retry_backoff))
}

/// 取某项的状态（不存在就让测试失败）。
fn state_of(queue: &Queue, id: u64) -> JobState {
    queue
        .jobs()
        .iter()
        .find(|job| job.id == id)
        .unwrap_or_else(|| panic!("队列里没有 id={id}"))
        .state
        .clone()
}

/// 取某项的 `retry_at`（状态不是 `Failed` 就让测试失败）。
fn retry_at(queue: &Queue, id: u64) -> Option<u64> {
    match state_of(queue, id) {
        JobState::Failed { retry_at, .. } => retry_at,
        other => panic!("期望 Failed，实际 {other:?}"),
    }
}

#[test]
fn ids_start_at_one_and_only_grow() {
    let mut queue = queue(1, 0, 0);
    assert_eq!(queue.enqueue("a", 1, Direction::Upload), Some(1));
    assert_eq!(queue.enqueue("b", 2, Direction::Download), Some(2));
    assert_eq!(queue.enqueue("c", 3, Direction::Upload), Some(3));

    let ids: Vec<u64> = queue.jobs().iter().map(|job| job.id).collect();
    assert_eq!(ids, [1, 2, 3], "jobs() 按 id 升序");

    // 取消与结束都不回收编号。
    assert!(queue.cancel(1));
    assert_eq!(queue.enqueue("d", 4, Direction::Upload), Some(4));
    let ids: Vec<u64> = queue.jobs().iter().map(|job| job.id).collect();
    assert_eq!(ids, [1, 2, 3, 4], "id 单调递增、永不复用");
}

#[test]
fn duplicate_name_and_direction_rejected_while_unsettled() {
    let mut queue = queue(1, 0, 0);
    assert_eq!(queue.enqueue("a.txt", 10, Direction::Upload), Some(1));
    assert_eq!(
        queue.enqueue("a.txt", 10, Direction::Upload),
        None,
        "Pending 时同名同方向算重复"
    );
    assert_eq!(
        queue.enqueue("a.txt", 999, Direction::Upload),
        None,
        "size 不参与重复判定"
    );

    queue.start_next(0);
    assert_eq!(
        queue.enqueue("a.txt", 10, Direction::Upload),
        None,
        "Running 时仍算重复"
    );
    assert_eq!(queue.len(), 1, "被拒的项不入队");
}

#[test]
fn same_name_other_direction_is_not_duplicate() {
    let mut queue = queue(1, 0, 0);
    assert_eq!(queue.enqueue("a.txt", 10, Direction::Upload), Some(1));
    assert_eq!(
        queue.enqueue("a.txt", 10, Direction::Download),
        Some(2),
        "方向不同是两件事"
    );
}

#[test]
fn done_job_can_be_enqueued_again() {
    let mut queue = queue(1, 0, 0);
    let id = queue.enqueue("a.txt", 10, Direction::Upload).expect("首入队");
    assert_eq!(queue.start_next(0), [id]);
    assert!(queue.finish(id, Ok(()), 0));

    assert_eq!(
        queue.enqueue("a.txt", 10, Direction::Upload),
        Some(2),
        "传完之后同名同方向可以再入队（重传）"
    );
    assert_eq!(queue.len(), 2);
}

#[test]
fn retryable_failure_blocks_duplicate_until_terminal() {
    // max_retries = 1：第一次失败安排重试（仍算未结束），第二次失败转终态。
    let mut queue = queue(1, 1, 10);
    let id = queue.enqueue("a.txt", 10, Direction::Upload).expect("首入队");
    queue.start_next(0);
    assert!(queue.finish(id, Err(FailReason::Io), 0));
    assert_eq!(
        queue.enqueue("a.txt", 10, Direction::Upload),
        None,
        "可重试的失败项仍在队列里语义上未结束"
    );

    assert_eq!(queue.retry_due(10), [id]);
    assert_eq!(queue.start_next(10), [id]);
    assert!(queue.finish(id, Err(FailReason::Io), 10));
    assert_eq!(
        state_of(&queue, id),
        JobState::Failed {
            reason: FailReason::Io,
            retry_at: None,
        },
        "额度用完即终态"
    );
    assert_eq!(queue.jobs()[0].attempts, 1, "终态那次不再加 attempts");
    assert!(
        queue.enqueue("a.txt", 10, Direction::Upload).is_some(),
        "终态失败后可再入队"
    );
}

#[test]
fn max_parallel_zero_behaves_as_one() {
    let mut queue = queue(0, 0, 0);
    for index in 0..3 {
        queue
            .enqueue(format!("f{index}"), 10, Direction::Upload)
            .expect("入队");
    }
    assert_eq!(queue.start_next(0), [1], "0 按 1 处理");
    assert_eq!(queue.running(), 1);
    assert!(queue.start_next(0).is_empty(), "名额已满");

    assert!(queue.finish(1, Ok(()), 0));
    assert_eq!(queue.start_next(0), [2], "释放名额后才能再启动");
}

#[test]
fn max_parallel_three_is_hard_limit() {
    let mut queue = queue(3, 0, 0);
    for index in 0..5 {
        queue
            .enqueue(format!("f{index}"), 10, Direction::Upload)
            .expect("入队");
    }
    assert_eq!(queue.start_next(0), [1, 2, 3]);
    assert_eq!(queue.running(), 3);
    assert!(queue.start_next(0).is_empty(), "不得越过上限");

    assert!(queue.finish(2, Ok(()), 0));
    assert_eq!(queue.start_next(0), [4], "空出一个位就只启动一项");
    assert_eq!(queue.running(), 3);
}

#[test]
fn start_next_follows_enqueue_order_and_skips_non_pending() {
    let mut queue = queue(2, 0, 0);
    for index in 0..4 {
        queue
            .enqueue(format!("f{index}"), 10, Direction::Upload)
            .expect("入队");
    }
    assert_eq!(queue.start_next(0), [1, 2], "按入队顺序");

    // 取消队首的等待项不该让后面的项插队。
    assert!(queue.cancel(3));
    assert!(queue.finish(1, Ok(()), 0));
    assert_eq!(queue.start_next(0), [4], "跳过 Cancelled");
}

#[test]
fn backoff_doubles_and_caps_at_300() {
    let mut queue = queue(1, 20, 10);
    let id = queue.enqueue("a", 100, Direction::Upload).expect("入队");

    let mut now = 0u64;
    let mut retry_ats = Vec::new();
    for _ in 0..7 {
        assert_eq!(queue.start_next(now), [id]);
        assert!(queue.finish(id, Err(FailReason::Io), now));
        let at = retry_at(&queue, id).expect("还有额度，应安排重试");
        assert_eq!(queue.retry_due(at), [id], "到点即捞回 Pending");
        retry_ats.push(at);
        now = at;
    }

    // 10、20、40、80、160 递增，之后被 300 秒封顶。
    assert_eq!(retry_ats, [10, 30, 70, 150, 310, 610, 910]);
    assert_eq!(queue.jobs()[0].attempts, 7);
}

#[test]
fn zero_backoff_is_immediately_retryable() {
    let mut queue = queue(1, 3, 0);
    let id = queue.enqueue("a", 100, Direction::Upload).expect("入队");
    queue.start_next(0);
    assert!(queue.finish(id, Err(FailReason::Timeout), 500));

    assert_eq!(retry_at(&queue, id), Some(500), "0 退避：now 当刻即可重试");
    assert!(queue.has_retryable());
    assert!(queue.retry_due(499).is_empty(), "差一秒都不行");
    assert_eq!(queue.retry_due(500), [id]);
}

#[test]
fn max_retries_zero_fails_terminally_on_first_error() {
    let mut queue = queue(1, 0, 10);
    let id = queue.enqueue("a", 100, Direction::Upload).expect("入队");
    queue.start_next(0);
    assert!(queue.finish(id, Err(FailReason::Protocol), 0));

    assert_eq!(
        state_of(&queue, id),
        JobState::Failed {
            reason: FailReason::Protocol,
            retry_at: None,
        }
    );
    assert_eq!(queue.jobs()[0].attempts, 0, "一次都没重试");
    assert!(queue.retry_due(10_000).is_empty(), "终态永不回 Pending");
    assert!(!queue.has_retryable());
    assert!(queue.all_settled());
}

#[test]
fn progress_is_monotonic_and_clamped() {
    let mut queue = queue(1, 0, 0);
    let id = queue.enqueue("a", 100, Direction::Upload).expect("入队");
    queue.start_next(0);

    assert!(queue.progress(id, 40));
    assert_eq!(state_of(&queue, id), JobState::Running { transferred: 40 });

    assert!(!queue.progress(id, 39), "回退被忽略");
    assert_eq!(
        state_of(&queue, id),
        JobState::Running { transferred: 40 },
        "回退不该改动状态"
    );

    assert!(queue.progress(id, 1_000), "超 size 接受");
    assert_eq!(
        state_of(&queue, id),
        JobState::Running { transferred: 100 },
        "钳到 size"
    );
    assert_eq!(queue.overall(), (100, 100));

    assert!(queue.progress(id, 100), "相等视为接受（不倒退）");
}

#[test]
fn progress_and_finish_rejected_outside_running() {
    let mut queue = queue(1, 0, 0);
    let id = queue.enqueue("a", 100, Direction::Upload).expect("入队");
    assert!(!queue.progress(id, 10), "Pending 不接受进度");
    assert!(!queue.finish(id, Ok(()), 0), "Pending 不接受 finish");

    assert!(!queue.progress(999, 10), "未知 id");
    assert!(!queue.finish(999, Ok(()), 0), "未知 id");

    queue.start_next(0);
    assert!(queue.finish(id, Ok(()), 0));
    assert!(!queue.progress(id, 10), "Done 之后不再接受进度");
    assert!(!queue.finish(id, Ok(()), 0), "重复 finish 返回 false");
}

#[test]
fn done_frees_parallel_slot() {
    let mut queue = queue(1, 0, 0);
    let first = queue.enqueue("a", 10, Direction::Upload).expect("入队");
    let second = queue
        .enqueue("b", 20, Direction::Download)
        .expect("入队");

    assert_eq!(queue.start_next(0), [first]);
    assert!(queue.start_next(0).is_empty(), "只有一个并行位");
    assert!(queue.finish(first, Ok(()), 0), "完成");
    assert_eq!(queue.start_next(0), [second], "释放的位给下一项");
    assert_eq!(queue.overall(), (10, 30), "Done 按 size 全额计入");
}

#[test]
fn retry_due_picks_only_due_jobs() {
    let mut queue = queue(2, 5, 100);
    let first = queue.enqueue("a", 10, Direction::Upload).expect("入队");
    let second = queue.enqueue("b", 10, Direction::Upload).expect("入队");
    assert_eq!(queue.start_next(0), [first, second]);

    assert!(queue.finish(first, Err(FailReason::Io), 0));
    assert!(queue.finish(second, Err(FailReason::Io), 50));

    assert_eq!(retry_at(&queue, first), Some(100));
    assert_eq!(retry_at(&queue, second), Some(150));

    assert!(queue.retry_due(99).is_empty(), "都还早");
    assert_eq!(queue.retry_due(100), [first], "只捞到点的");
    assert_eq!(state_of(&queue, first), JobState::Pending);
    assert_eq!(queue.retry_due(149), Vec::<u64>::new());
    assert_eq!(queue.retry_due(150), [second]);
}

#[test]
fn cancel_pending_and_running_but_not_settled() {
    let mut queue = queue(1, 0, 0);
    let pending = queue.enqueue("a", 10, Direction::Upload).expect("入队");
    let running = queue.enqueue("b", 10, Direction::Upload).expect("入队");

    assert!(queue.cancel(pending), "Pending 直接取消");
    assert_eq!(state_of(&queue, pending), JobState::Cancelled);
    assert_eq!(queue.start_next(0), [running], "取消项不会被启动");

    assert!(queue.cancel(running), "Running 也能取消");
    assert_eq!(state_of(&queue, running), JobState::Cancelled);
    assert_eq!(queue.running(), 0, "取消 Running 释放并行位");
    assert!(!queue.finish(running, Ok(()), 0), "已取消项不接受 finish");

    assert!(!queue.cancel(pending), "已结束项返回 false");
    assert!(!queue.cancel(999), "未知 id 返回 false");
    assert!(
        queue.enqueue("a", 10, Direction::Upload).is_some(),
        "取消后可再入队"
    );
}

#[test]
fn cancel_retryable_failure_instead_of_retrying() {
    let mut queue = queue(1, 3, 10);
    let id = queue.enqueue("a", 10, Direction::Upload).expect("入队");
    queue.start_next(0);
    assert!(queue.finish(id, Err(FailReason::Io), 0));
    assert!(queue.cancel(id), "等待退避的项可以取消");
    assert_eq!(state_of(&queue, id), JobState::Cancelled);
    assert!(queue.retry_due(1_000).is_empty(), "取消后不再被捞回");
}

#[test]
fn finish_with_cancelled_reason_goes_to_cancelled() {
    let mut queue = queue(1, 3, 10);
    let id = queue.enqueue("a", 10, Direction::Upload).expect("入队");
    queue.start_next(0);
    assert!(queue.finish(id, Err(FailReason::Cancelled), 0));
    assert_eq!(state_of(&queue, id), JobState::Cancelled, "取消不是故障，不重试");
    assert!(!queue.has_retryable());
    assert!(queue.all_settled());
}

#[test]
fn overall_excludes_cancelled_and_handles_empty() {
    let mut queue = queue(2, 0, 0);
    assert!(queue.is_empty());
    assert!(queue.all_settled(), "空队列视为全部结束");
    assert_eq!(queue.overall(), (0, 0), "空队列不除零");

    let first = queue.enqueue("a", 100, Direction::Upload).expect("入队");
    let second = queue.enqueue("b", 300, Direction::Download).expect("入队");
    assert_eq!(queue.start_next(0), [first, second]);
    assert!(queue.progress(first, 50));
    assert!(queue.cancel(second));

    assert_eq!(queue.overall(), (50, 100), "取消项不进分母");
    assert!(queue.finish(first, Ok(()), 0));
    assert_eq!(queue.overall(), (100, 100), "进度能到 100%");
    assert_eq!(queue.len(), 2, "取消项仍留在列表里");
    assert!(queue.all_settled());
}

#[test]
fn zero_size_queue_reports_zero_over_zero() {
    let mut queue = queue(1, 0, 0);
    let id = queue.enqueue("empty", 0, Direction::Upload).expect("入队");
    assert_eq!(queue.overall(), (0, 0));
    queue.start_next(0);
    assert!(queue.finish(id, Ok(()), 0));
    assert_eq!(queue.overall(), (0, 0), "总字节为 0 时返回 (0, 0)");
}

#[test]
fn has_retryable_and_all_settled_track_retry_budget() {
    let mut queue = queue(1, 1, 5);
    let id = queue.enqueue("a", 10, Direction::Upload).expect("入队");
    assert!(!queue.has_retryable(), "还没失败");
    assert!(!queue.all_settled(), "Pending 未结束");

    queue.start_next(0);
    assert!(queue.finish(id, Err(FailReason::Timeout), 0));
    assert!(queue.has_retryable(), "有额度可用");
    assert!(!queue.all_settled());

    assert_eq!(queue.retry_due(5), [id]);
    assert_eq!(queue.start_next(5), [id]);
    assert!(queue.finish(id, Err(FailReason::Timeout), 5));
    assert!(!queue.has_retryable(), "额度用完");
    assert!(queue.all_settled());
}
