use super::*;
// `tests` 的 `super` 是 `filer::sync`，而 `Filer` 在上一层（`filer`）。
use super::super::Filer;

fn summary(upload: usize, download: usize, skip: usize) -> Summary {
    Summary {
        upload,
        download,
        skip,
        ..Summary::default()
    }
}

#[test]
fn an_all_clear_summary_says_so_instead_of_zeroes() {
    // 「0 项待上传 / 0 项待下载 / 5 项已一致」读起来像坏了；一致就直说。
    assert_eq!(summary_line(&summary(0, 0, 5)), "同步：两侧已一致（5 项）");
}

#[test]
fn the_line_only_mentions_the_directions_that_actually_have_work() {
    assert_eq!(
        summary_line(&summary(2, 0, 3)),
        "同步：2 项待上传（3 项已一致）"
    );
    assert_eq!(
        summary_line(&summary(2, 1, 0)),
        "同步：2 项待上传 / 1 项待下载（0 项已一致）"
    );
}

fn entry(rel: &str, is_dir: bool, size: u64, mtime: Option<i64>) -> ScannedEntry {
    ScannedEntry {
        rel: rel.to_string(),
        is_dir,
        size,
        mtime,
    }
}

fn review(local: Vec<ScannedEntry>, remote: Vec<ScannedEntry>) -> SyncReview {
    SyncReview::new(
        PathBuf::from("/local"),
        "/remote".to_string(),
        Scanned { local, remote },
    )
}

#[test]
fn switching_the_delete_policy_rebuilds_without_rescanning() {
    // 两份清单只采一次，三选一之间来回切都只重算计划——重扫一遍既慢，
    // 而且用户在这中间动了文件的话会扫到一棵不同的树。
    let local = vec![entry("only-local", false, 1, Some(10))];
    let remote = vec![entry("only-remote", false, 1, Some(10))];
    let mut r = review(local, remote);

    assert_eq!(r.policy, DeletePolicy::Keep, "默认绝不能是删");
    assert_eq!((r.summary.upload, r.summary.download), (1, 1));
    assert!(!r.summary.has_deletes());

    r.set_policy(DeletePolicy::DeleteRemote);
    assert_eq!((r.summary.upload, r.summary.delete_remote), (1, 1));
    assert!(r.summary.has_deletes());

    r.set_policy(DeletePolicy::DeleteLocal);
    assert_eq!((r.summary.download, r.summary.delete_local), (1, 1));

    // 切回默认值，计划回到最初那份。
    r.set_policy(DeletePolicy::Keep);
    assert_eq!((r.summary.upload, r.summary.download), (1, 1));
    assert_eq!(r.summary.mutations(), 2);
}

#[test]
fn the_executor_walks_the_plan_in_order_and_reports_progress() {
    // 只走记账那一半：`step()` / `record()` 都在 `term_xfer::sync` 里，
    // 这里锁的是「界面侧的 pump 不会漏项也不会重复」——所以用 `SyncExec`
    // 自己的形状跑一遍完整的顺序。
    let local = vec![
        entry("a", true, 0, Some(10)),
        entry("a/x.txt", false, 3, Some(10)),
        entry("b.txt", false, 5, Some(10)),
    ];
    let mut exec = SyncExec::new(
        build_plan(&local, &[], DeletePolicy::Keep),
        PathBuf::from("/local"),
        "/remote".to_string(),
    );
    assert_eq!(exec.progress().todo, 3, "三项都要动");
    assert_eq!(exec.progress().already, 0);

    // 依次取三步：目录在前、同层按路径升序。
    let mut seen = Vec::new();
    while let Some(item) = exec.run.step() {
        seen.push(item.entry.path.clone());
        exec.run.record(StepResult::Done);
    }
    assert_eq!(seen, ["a", "a/x.txt", "b.txt"]);
    let p = exec.progress();
    assert_eq!((p.done, p.failed, p.skipped), (3, 0, 0));
    assert_eq!(p.remaining(), 0);
}

#[test]
fn the_panel_line_counts_bytes_once_and_names_the_file_it_is_on() {
    // 三个陷阱，一个判据一条：
    // ① `Progress` 报的在途字节与随后 `Done` 报的文件大小是同一批——两边都加
    //    就会在「一个 3 字节的文件传完」之后显示 5 或 6；
    // ② 普通传输也在发同一批 `Progress`，认不上在途路径的必须丢掉；
    // ③ 目录项没有字节，算进分母会让百分比永远到不了 100%。
    let mut filer = running_with_one_in_flight();
    assert_eq!(exec_of(&filer).total_bytes(), 6, "两个 3 字节文件");

    filer.note_sync_progress("/remote/a.txt", 2);
    assert_eq!(exec_of(&filer).moved_bytes(), 2, "在途那份的进度也要算");

    filer.note_sync_progress("/remote/other.txt", 99);
    assert_eq!(exec_of(&filer).moved_bytes(), 2, "别人的进度不许算进来");

    filer.note_sync_done("/remote/a.txt", 3);
    assert_eq!(
        exec_of(&filer).moved_bytes(),
        3,
        "`Done` 报的是整个文件，加上在途那 2 字节就是重复计数"
    );

    let line = SyncLine::of(exec_of(&filer));
    assert!(line.headline.contains("1/2"), "{}", line.headline);
    assert!(line.detail.contains("3 B / 6 B"), "{}", line.detail);
    assert!(
        line.detail.contains("a.txt"),
        "要说清在动哪个文件：{}",
        line.detail
    );
}

#[test]
fn a_run_with_nothing_to_transfer_says_nothing_about_bytes() {
    // 全是目录 / 删除的一趟：`total_bytes == 0`。写「0 B / 0 B」既没有信息，
    // 又会让「估算中」跟着出现——那两句合起来像是卡住了。
    let local = vec![entry("a", true, 0, Some(10))];
    let exec = SyncExec::new(
        build_plan(&local, &[], DeletePolicy::Keep),
        PathBuf::from("/local"),
        "/remote".to_string(),
    );
    assert_eq!(exec.total_bytes(), 0, "目录不搬字节，不许进分母");
    let filer = Filer {
        sync: Some(SyncState::Running(Box::new(exec))),
        ..Filer::default()
    };
    let line = SyncLine::of(exec_of(&filer));
    assert_eq!(line.detail, "", "没有字节也没有速率时整段不画：{}", line.detail);
    assert!(!line.stopping);
}

#[test]
fn a_failed_directory_skips_its_children_and_only_counts_them_once() {
    let local = vec![
        entry("a", true, 0, Some(10)),
        entry("a/x.txt", false, 3, Some(10)),
        entry("a/y.txt", false, 3, Some(10)),
    ];
    let mut exec = SyncExec::new(
        build_plan(&local, &[], DeletePolicy::Keep),
        PathBuf::from("/local"),
        "/remote".to_string(),
    );
    // 第一步是目录 `a`，让它失败；后面两个子项应当被就地跳过。
    let first = exec.run.step().cloned().expect("有一步");
    assert_eq!(first.entry.path, "a");
    exec.run.record(StepResult::Failed);
    assert!(exec.run.step().is_none(), "子项全被连坐，序列到此为止");

    let p = exec.progress();
    assert_eq!((p.failed, p.done, p.skipped), (1, 0, 2));
}

/// 造一个「执行中、且已经有一步在飞」的 `Filer`。
fn running_with_one_in_flight() -> Filer {
    let local = vec![
        entry("a.txt", false, 3, Some(10)),
        entry("b.txt", false, 3, Some(10)),
    ];
    let mut exec = SyncExec::new(
        build_plan(&local, &[], DeletePolicy::Keep),
        PathBuf::from("/local"),
        "/remote".to_string(),
    );
    exec.inflight = Some(Inflight {
        path: "/remote/a.txt".to_string(),
        op: SyncOp::Upload,
    });
    // `pump_exec` 发请求时就会设这两个；手动摆出「已发出一步」的状态时也要跟上，
    // 否则测到的是一个真实运行里不存在的中间态。
    exec.current = Some("a.txt".to_string());
    Filer {
        sync: Some(SyncState::Running(Box::new(exec))),
        ..Filer::default()
    }
}

fn exec_of(filer: &Filer) -> &SyncExec {
    match &filer.sync {
        Some(SyncState::Running(exec)) => exec,
        _ => panic!("应该在执行阶段"),
    }
}

#[test]
fn the_pump_never_starts_a_second_step_while_one_is_in_flight() {
    // 「同一时刻只飞一步」是整个执行层的地基：并发会让「目录连坐」与
    // 「删除排最后」两条契约同时失效。这里锁住的是 pump 的前置条件——
    // 只要在途那一步没回来，哪怕给再多帧也不许发下一条。
    let mut filer = running_with_one_in_flight();
    for _ in 0..3 {
        assert!(filer.poll_sync(None, 0), "执行阶段要下一帧");
        let p = exec_of(&filer).progress();
        assert_eq!((p.done, p.failed, p.skipped), (0, 0, 0), "一步都没记账");
        assert!(exec_of(&filer).inflight.is_some(), "在途那一步仍在等事件");
    }
    // 传 `None` 也不该把在途那一步中止掉——没句柄只是发不出下一步，
    // 已经发出去那一步的结果还要收。
    assert!(filer.error.is_none());
}

#[test]
fn only_the_step_in_flight_claims_a_done_or_failed_event() {
    // `Done` / `Failed` 只带远端路径，而普通传输、写操作（mkdir / rename）
    // 也走同一批事件。认错一步会让记账错位：真正的失败被当成别人成功。
    let mut filer = running_with_one_in_flight();
    filer.note_sync_done("/remote/b.txt", 3);
    assert_eq!(exec_of(&filer).progress().done, 0, "别人的 Done 不许记账");

    filer.note_sync_done("/remote/a.txt", 3);
    assert_eq!(exec_of(&filer).progress().done, 1);
    assert!(
        exec_of(&filer).inflight.is_none(),
        "收下之后要清在途，否则永远发不出下一步"
    );
}

#[test]
fn a_failure_keeps_the_server_words_and_frees_the_step() {
    let mut filer = running_with_one_in_flight();
    filer.note_sync_failed("/remote/a.txt", "Permission denied");
    let exec = exec_of(&filer);
    assert_eq!(exec.progress().failed, 1);
    assert_eq!(
        exec.last_error(),
        Some("/remote/a.txt: Permission denied"),
        "服务端原话要原样留下——转述成「操作失败」等于把线索擦掉"
    );
    assert!(exec.inflight.is_none());
}

#[test]
fn a_stop_request_waits_for_the_step_in_flight_then_reports_real_numbers() {
    // 停止 = 不再发下一步，已做的保留（不回滚）。但**不许当场走人**：在途那一步
    // 已经发到服务端，它的结果迟早会到。当场清状态的话界面会写「3/10」而实际
    // 可能已经传到第 4 个——数字对不上比慢一步糟。
    let mut filer = running_with_one_in_flight();
    filer.request_sync_stop();
    assert!(filer.poll_sync(None, 0), "还在跑，要下一帧");
    assert!(
        matches!(&filer.sync, Some(SyncState::Running(e)) if e.is_stopping()),
        "在途那一步没回来之前，状态还在且标成「停止中」"
    );

    // 在手那一步回来了（成功），停止这时才真正兑现。
    filer.note_sync_done("/remote/a.txt", 3);
    assert!(filer.poll_sync(None, 0));
    assert!(filer.sync.is_none(), "停下来之后没有状态可推进");
    let line = filer.progress.as_deref().expect("要有收尾文案");
    assert!(line.contains("已停止"), "{line}");
    assert!(line.contains("1 项已完成"), "在途那一步的成功要算进去：{line}");
    assert!(line.contains("1 项没动"), "剩下的项数要说清：{line}");
}

#[test]
fn a_dropped_session_aborts_the_run_instead_of_leaving_it_spinning() {
    // 会话断了，执行阶段没法继续。状态必须清掉并说明原因，否则界面会停在
    // 「3 / 12」上再也不动，看起来像卡死。
    let items = build_plan(
        &[entry("a.txt", false, 3, Some(10))],
        &[],
        DeletePolicy::Keep,
    );
    let mut filer = Filer {
        sync: Some(SyncState::Running(Box::new(SyncExec::new(
            items,
            PathBuf::from("/local"),
            "/remote".to_string(),
        )))),
        ..Filer::default()
    };
    filer.abort_sync("连接被关闭".to_string());
    assert!(filer.sync.is_none(), "状态要清掉");
    assert!(
        filer.error.as_deref().is_some_and(|e| e.contains("连接被关闭")),
        "要说明为什么停了：{:?}",
        filer.error
    );
}

#[test]
fn kept_errors_are_capped_so_a_total_failure_does_not_grow_without_bound() {        let mut exec = SyncExec::new(Vec::new(), PathBuf::from("/local"), "/r".to_string());
    assert!(exec.last_error().is_none());
    for i in 0..(MAX_KEPT_ERRORS + 3) {
        exec.note_error(format!("e{i}"));
    }
    assert_eq!(exec.errors.len(), MAX_KEPT_ERRORS);
    assert_eq!(
        exec.last_error(),
        Some(format!("e{}", MAX_KEPT_ERRORS - 1).as_str()),
        "留的是最早的那几条，不是最新的——第一句通常才是根因"
    );
}