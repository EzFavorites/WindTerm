//! `script_human` 的行为断言：stdin 端口、UI 队列回信、取消与断开。
//!
//! 从 `script_human.rs` 原样搬来（只去掉外层 `mod tests` 的一级缩进），
//! 断言一个字没改；拆开的理由是主文件 550 代码行过 4j 的 500 上限。
use super::*;

/// stdin 不是终端时**不猜答案**：`NoInput` 而不是空串。
#[test]
fn the_stdin_channel_refuses_when_it_is_not_a_terminal() {
    use std::io::IsTerminal;
    // 这一条要钉的是「stdin 被重定向」那条分支，而 stdin 到底是不是终端
    // 由**跑测试的环境**决定，不是由这条测试决定。`cargo test` 在终端里
    // （`scripts/verify.sh` 第 103 行就是裸跑，没重定向）会继承那个终端，
    // 于是 `read_line` 一直等人敲字——实测挂死 18 分钟，门禁看起来是「卡住」
    // 而不是「红了」。CI 与 `cargo test < /dev/null` 下 stdin 是管道，正常断言。
    if std::io::stdin().is_terminal() {
        eprintln!("skip: stdin is a terminal; redirect it to exercise this branch");
        return;
    }
    let human = StdinHuman::new();
    assert_eq!(
        human.ask(&Ask::Confirm {
            title: "t".into(),
            text: "继续？".into()
        }),
        Err(AskError::NoInput)
    );
}

/// 关掉输入来源之后，即使有终端也不许问。
#[test]
fn without_input_closes_the_door_even_on_a_terminal() {
    let human = StdinHuman::new().without_input();
    assert!(!human.interactive());
    assert_eq!(human.pick_file(&PickFile::Open {
        title: "t".into(),
        filter: String::new(),
        default: String::new(),
    }), Err(AskError::Unsupported(
        "当前脚本运行通道没有文件选择框；请在脚本面板 ▸ Permissions 勾选本次 fs 授权后用 fs_*".to_string()
    )));
}

/// 界面那条：问题进队列、答案回到脚本线程；文件框同理。
#[test]
fn the_ui_channel_round_trips_through_the_queue() {
    let queue: AskQueue = Default::default();
    let picks = Arc::new(Mutex::new(Vec::new()));
    let human = Arc::new(UiHuman::new(
        Arc::clone(&queue),
        Arc::clone(&picks),
        None,
        None,
        CancelToken::new(),
    ));

    // ① 文本问题。
    let asker = {
        let human = Arc::clone(&human);
        std::thread::spawn(move || {
            human.ask(&Ask::Text {
                title: String::new(),
                prompt: "名字？".into(),
                default: String::new(),
            })
        })
    };
    let pending = pop_until(&queue);
    assert!(matches!(pending.request, Ask::Text { .. }));
    pending
        .reply
        .send(Ok(Answer::Text("abc".into())))
        .expect("回信");
    assert_eq!(
        asker.join().expect("脚本线程").expect("有答案"),
        Answer::Text("abc".into())
    );

    // ② 文件框：同上一条路，签名多一个 PathBuf。用户手选的路径要**当场**
    // 进授权信封（这里没信封，所以只验证「选中的路径带回去了」）。
    let picker = {
        let human = Arc::clone(&human);
        std::thread::spawn(move || {
            human.pick_file(&PickFile::Open {
                title: String::new(),
                filter: String::new(),
                default: String::new(),
            })
        })
    };
    let pending = pop_pick_until(&picks);
    pending
        .reply
        .send(Some(PathBuf::from("/tmp/picked.txt")))
        .expect("回信");
    assert_eq!(
        picker.join().expect("脚本线程").expect("有答案"),
        Some(PathBuf::from("/tmp/picked.txt"))
    );
}

/// 界面按了 Esc / Cancel：回 `Err(NoInput)`，脚本线程要看到「没人回答」，
/// 而不是 `Ok(Bool(false))`——后者会让 Confirm 的「取消」与「No」混成一件。
#[test]
fn a_cancelled_ask_reaches_the_script_as_no_input() {
    let queue: AskQueue = Default::default();
    let human = Arc::new(UiHuman::new(
        Arc::clone(&queue),
        Arc::new(Mutex::new(Vec::new())),
        None,
        None,
        CancelToken::new(),
    ));
    let asker = {
        let human = Arc::clone(&human);
        std::thread::spawn(move || {
            human.ask(&Ask::Confirm {
                title: String::new(),
                text: "继续？".into(),
            })
        })
    };
    let pending = pop_until(&queue);
    pending.reply.send(Err(AskError::NoInput)).expect("回信");
    assert_eq!(asker.join().expect("脚本线程"), Err(AskError::NoInput));
    assert!(queue.lock().expect("队列锁").is_empty());
}

/// 等不到回信而通道断了（窗口关掉）：同样报 NoInput，不冒充默认答案。
#[test]
fn a_disconnected_ask_channel_means_no_input() {
    let queue: AskQueue = Default::default();
    let human = Arc::new(UiHuman::new(
        Arc::clone(&queue),
        Arc::new(Mutex::new(Vec::new())),
        None,
        None,
        CancelToken::new(),
    ));
    let asker = {
        let human = Arc::clone(&human);
        std::thread::spawn(move || {
            human.ask(&Ask::Text {
                title: String::new(),
                prompt: "名字？".into(),
                default: String::new(),
            })
        })
    };
    let pending = pop_until(&queue);
    drop(pending.reply);
    assert_eq!(asker.join().expect("脚本线程"), Err(AskError::NoInput));
}

/// 等到队列里有东西再取一个（界面那一侧的动作）。
fn pop_until(queue: &AskQueue) -> PendingAsk {
    loop {
        if let Some(ask) = queue.lock().expect("队列锁").pop() {
            return ask;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn pop_pick_until(queue: &Arc<Mutex<Vec<PendingPick>>>) -> PendingPick {
    loop {
        if let Some(pick) = queue.lock().expect("队列锁").pop() {
            return pick;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// 取消要能叫醒等在 `recv` 上的脚本线程（界面「停止」按钮那条路）。
#[test]
fn cancelling_wakes_a_waiting_ask() {
    let queue: AskQueue = Default::default();
    let cancel = CancelToken::new();
    let human = std::sync::Arc::new(UiHuman::new(
        Arc::clone(&queue),
        Arc::new(Mutex::new(Vec::new())),
        None,
        None,
        cancel.clone(),
    ));
    let worker = {
        let human = Arc::clone(&human);
        std::thread::spawn(move || {
            human.ask(&Ask::Confirm {
                title: String::new(),
                text: "继续？".into(),
            })
        })
    };
    // 等脚本排进队列，再叫停。
    while queue.lock().expect("队列锁").is_empty() {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cancel.cancel();
    assert_eq!(worker.join().expect("脚本线程"), Err(AskError::Cancelled));
    assert!(
        queue.lock().expect("队列锁").is_empty(),
        "取消之后问题不该留在队列里，下一帧再弹一次"
    );
}

#[test]
fn a_cancelled_pick_takes_itself_out_of_the_queue() {
    let picks = Arc::new(Mutex::new(Vec::new()));
    let cancel = CancelToken::new();
    let human = UiHuman::new(
        Default::default(),
        Arc::clone(&picks),
        None,
        None,
        cancel.clone(),
    );
    let worker = {
        let human = Arc::new(human);
        std::thread::spawn(move || {
            human.pick_file(&PickFile::Open {
                title: String::new(),
                filter: String::new(),
                default: String::new(),
            })
        })
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while picks.lock().expect("队列锁").is_empty() {
        assert!(std::time::Instant::now() < deadline, "文件框没能进队");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cancel.cancel();
    assert_eq!(worker.join().expect("脚本线程"), Err(AskError::Cancelled));
    assert!(picks.lock().expect("队列锁").is_empty());
}

#[test]
fn pending_requests_report_staleness_before_the_ui_drains_them() {
    let cancel = CancelToken::new();
    let (tx, _rx) = std::sync::mpsc::sync_channel(1);
    let ask = PendingAsk {
        seq: next_seq(),
        cancel: cancel.clone(),
        request: Ask::Alert {
            title: String::new(),
            text: String::new(),
        },
        reply: tx,
    };
    let (tx, _rx) = std::sync::mpsc::sync_channel(1);
    let pick = PendingPick {
        seq: next_seq(),
        cancel: cancel.clone(),
        request: PickFile::Open {
            title: String::new(),
            filter: String::new(),
            default: String::new(),
        },
        reply: tx,
    };
    assert!(!ask.is_stale() && !pick.is_stale());
    cancel.cancel();
    assert!(ask.is_stale() && pick.is_stale());
}
