//! 无头 e2e（S-a 验收口径）：`SessionPool` 开本地会话，Rhai 脚本完成
//! 「登录 → 跑命令 → 取输出」等价物——write → wait / wait_any / read_until(+grep)
//! → read → screen 全通。

use std::sync::{Arc, Mutex};

use term_core::grid::GridSnapshot;
use term_core::session_runtime::{
    FacetKind, OutputReader, Session, SessionId, SessionState, WaitError, WaitRegistration,
};
use term_script::{SandboxConfig, ScriptEngine};
use term_session::SessionPool;

/// `SessionPool` → `dyn Session` 的适配器：控制面不暴露底层句柄，
/// 测试用它把池内会话喂给脚本（协议无关，零协议分支）。
struct PoolSession {
    pool: Arc<SessionPool>,
    id: SessionId,
}

impl Session for PoolSession {
    fn id(&self) -> SessionId {
        self.id
    }

    fn write(&self, bytes: &[u8]) {
        self.pool.send(self.id, bytes);
    }

    fn resize(&self, _cols: usize, _rows: usize) {
        // 控制面未暴露 resize，e2e 用不到。
    }

    fn screen(&self) -> Option<GridSnapshot> {
        self.pool.screen(self.id)
    }

    fn state(&self) -> SessionState {
        self.pool
            .state(self.id)
            .unwrap_or(SessionState::Finished(None))
    }

    fn facets(&self) -> Vec<FacetKind> {
        self.pool.facets(self.id).unwrap_or_default()
    }

    fn wait_register(&self, pattern: &str) -> Result<Box<dyn WaitRegistration>, WaitError> {
        self.pool.wait_register(self.id, pattern)
    }

    fn reader_register(&self) -> Result<Box<dyn OutputReader>, WaitError> {
        self.pool.reader_register(self.id)
    }

    fn close(&mut self) {
        self.pool.close(self.id);
    }
}

#[test]
fn a_script_logs_in_runs_a_command_and_takes_the_output() {
    // 「登录」等价物：起一个本地 sh 会话（exec 形态，POSIX 语法有保证）。
    let pool = Arc::new(SessionPool::new());
    let id = pool.open_exec("sh", 100, 30).expect("起本地 sh 会话");

    let session: Arc<Mutex<dyn Session>> = Arc::new(Mutex::new(PoolSession {
        pool: pool.clone(),
        id,
    }));
    let engine = ScriptEngine::new(SandboxConfig::default());
    // 「跑命令 → 取输出」等价物：write 不自动加换行（脚本自己拼 \n），
    // wait 走先登记后消费，命中后 screen 取屏幕全文。
    // '|' 分隔 wait 返回值与屏幕全文——text_upto 是多行（回显行 + 输出行），
    // 不能按 '\n' 切。
    let script = r#"
        write("echo MARK_$((1+1))\n");
        let hit = wait("MARK_2", 10.0);
        hit + "|" + screen()
    "#;
    let out = engine.run(script, session).expect("脚本端到端应跑通");
    let mut parts = out.splitn(2, '|');
    let hit = parts.next().unwrap_or("");
    let screen = parts.next().unwrap_or("");
    assert!(hit.contains("MARK_2"), "wait 命中文本应含 MARK_2：{hit:?}");
    assert!(hit.ends_with("MARK_2"), "text_upto 应以模式结尾：{hit:?}");
    assert!(screen.contains("MARK_2"), "屏幕全文应含 MARK_2：{screen:?}");
    assert!(pool.close(id), "关闭会话");
}

#[test]
fn a_script_filters_noise_reads_to_a_sentinel_and_drains_the_tail() {
    // 类 grep 过滤 + 游标续读的真 PTY 验收。2026-11-19 裁定后 grep 是结果
    // 后处理：哨兵在噪声行里也命中（匹配不受 grep 影响），命中文本按 §4f-3
    // 透传不判；read 排水取走余段并按 grep_v 清掉噪声行。
    let pool = Arc::new(SessionPool::new());
    let id = pool.open_exec("sh", 100, 30).expect("起本地 sh 会话");

    let session: Arc<Mutex<dyn Session>> = Arc::new(Mutex::new(PoolSession {
        pool: pool.clone(),
        id,
    }));
    let engine = ScriptEngine::new(SandboxConfig::default());
    // '\\' 在 Rhai 字符串里是单个反斜杠 → printf 收到 \n 转义。
    // 哨兵经 printf %s 参数拼出：回显的命令行文本（"END_%s … MARK"）不含
    // 连续的 "END_MARK"。新语义下最早命中是噪声行 "noise END_MARK decoy"，
    // grep_v 不参与匹配；body 止于命中串（无换行）按 §4f-3 透传，
    // tail 由 read 排水、再按 grep_v 清掉噪声行。
    let script = r#"
        write("printf 'noise END_%s decoy\\nkeep1\\nEND_%s tail-after\\n' MARK MARK\n");
        let f = grep_v("^noise");
        let body = read_until("END_MARK", 10.0, f);
        let tail = read(2.0);
        body + "|" + tail
    "#;
    let out = engine.run(script, session).expect("过滤 + 排水脚本应跑通");
    let mut parts = out.splitn(2, '|');
    let body = parts.next().unwrap_or("");
    let tail = parts.next().unwrap_or("");
    assert!(body.ends_with("END_MARK"), "body 应止于命中串：{body:?}");
    assert!(
        tail.contains("tail-after"),
        "read 应取走命中后的余段与后续输出：{tail:?}"
    );
    assert!(
        tail.contains("keep1"),
        "未过滤行应在排水尾部保留：{tail:?}"
    );
    assert!(pool.close(id), "关闭会话");
}

#[test]
fn a_script_waits_on_multiple_branches_and_picks_the_first_one_seen() {
    // wait_any 的真 PTY 验收：1 起索引 + 「最先出现者胜」+ 共享游标续读。
    let pool = Arc::new(SessionPool::new());
    let id = pool.open_exec("sh", 100, 30).expect("起本地 sh 会话");

    let session: Arc<Mutex<dyn Session>> = Arc::new(Mutex::new(PoolSession {
        pool: pool.clone(),
        id,
    }));
    let engine = ScriptEngine::new(SandboxConfig::default());
    // 哨兵经 printf %s 拼出：回显的命令行文本（"BRANCH_%s … B A"）不含
    // 连续的 "BRANCH_A" / "BRANCH_B"，wait_any 不会在回显行误命中。
    // 输出里 B 先于 A，但模式数组把 A 放前面 → 命中 index 2 才算
    // 「最先出现者胜」（而非按数组顺序）。
    let script = r#"
        write("printf 'BRANCH_%s\\nBRANCH_%s\\n' B A\n");
        let hit = wait_any(["BRANCH_A", "BRANCH_B"], 10.0);
        // 显式等到后一行，别用 read：read 是「有存量立即返回」的排水语义，
        // 若 PTY 把两行切到不同 chunk，命中 B 后只剩 "\n"，read 会立刻返回、
        // 拿不到还在路上的 A（QA 用 sleep 0.3 确定性复现过）。read_until 对
        // chunk 切分免疫；游标若分叉 / 尾部丢失会显式超时，仍守住「命中后
        // 尾部文本不丢」的验收意图——不是放宽断言。
        let tail = read_until("BRANCH_A", 2.0);
        hit.index.to_string() + "|" + hit.text + "|" + tail
    "#;
    let out = engine.run(script, session).expect("wait_any 真 PTY 脚本应跑通");
    let mut parts = out.splitn(3, '|');
    let index = parts.next().unwrap_or("");
    let text = parts.next().unwrap_or("");
    let tail = parts.next().unwrap_or("");
    assert_eq!(index, "2", "B 先出现 → 1 起索引的 2：{out:?}");
    assert!(
        text.ends_with("BRANCH_B"),
        "命中文本应以最先出现的命中串结尾：{text:?}"
    );
    assert!(
        !text.lines().any(|l| l.starts_with("BRANCH_A")),
        "A 是后出现的，不该进首次命中文本：{text:?}"
    );
    assert!(
        tail.lines().any(|l| l.starts_with("BRANCH_A")),
        "命中后游标续读，read_until 显式等到后出现的 A：{tail:?}"
    );
    assert!(
        !tail.contains("BRANCH_B"),
        "已消费的命中串不该在续读里复活：{tail:?}"
    );
    assert!(pool.close(id), "关闭会话");
}
