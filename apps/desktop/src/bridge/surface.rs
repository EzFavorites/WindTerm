//! 能力面 → `term-session::SessionPool` 的翻译层（`Surface` 的生产实现）。
//!
//! # 为什么不放在 `term-bridge` 里
//!
//! `term-bridge` 的运行时依赖表硬性只有 `term-core` + `serde_json`（verify 4k
//! 钉着）。把这份实现搬进去，就得让 `term-session`（→ `term-runtime` → PTY、
//! 线程、监管者）变成任何想用桥接的编译单元的依赖——而桥接本体根本不需要
//! 它们，它只需要一个「认得这些方法名」的后端。
//!
//! 也不做成新 crate：这份翻译的宿主是**会话所有者**（ADR-0019：desk 是唯一
//! 所有者），现在只有一个使用者。等真出现第二个宿主（比如将来的 broker），
//! 再抽出去，那时才有第二个证据。
//!
//! # 与 `crates/term-bridge/tests/support/mod.rs` 的关系
//!
//! 那边有一份 `PoolSurface`，是桥接自己的**兼容终端**（它不能依赖 bin crate，
//! 所以只能自己带一份）。两份的**形状必须一致**，防漂移靠三件事：
//!
//! 1. 方法名与 schema 的单一真相是 `term_core::surface::METHODS`，两边都经
//!    [`describe_value`] 生成，没有任何手工清单；
//! 2. 参数解析只用 `term_core::contract` 里的 `required_*` / `optional_*`
//!    （`term_bridge::handler` 只是它的再导出），
//!    不在本文件里自己拆 JSON（否则「缺参数」会被写成 panic 或默认值）；
//! 3. 本文件末尾的用例逐个断言线上形状，与夹具那侧的断言是同一组期望值。
//!
//! 真正该合并的时机是「第二个宿主出现」——那时抽 crate，两份变一份。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};

use term_core::automation::{Arbitrator, Grant, Priority};
use term_core::contract::{
    describe_value, optional_bool, optional_string, optional_u64, required_session, required_string,
    Surface,
};
use term_core::session::SessionConfig;
use term_core::session_runtime::{SessionId, SessionState, WaitError, WaitMatch, WaitRegistration};
use term_core::surface_error::SurfaceError;
use term_script::{CancelToken, ScriptSession, ScriptSessions};
use term_session::SessionPool;

use crate::session_open::{open_session, OpenFailure};

#[path = "surface_helpers.rs"]
mod surface_helpers;
use surface_helpers::{
    facet_name, key_bytes, open_failure, primitive_failure, required_string_array,
    session_descriptor,
};

/// `wait` 的缺省上限：与 `term_core::surface` 里 `wait.timeout_ms` 的说明一致。
const DEFAULT_WAIT_TIMEOUT_MS: u64 = 30_000;

/// `session_open` 的缺省尺寸（与 ADR-0019 的 80×24 对齐）。
const DEFAULT_COLS: u64 = 80;
const DEFAULT_ROWS: u64 = 24;

/// 连线游标的兜底预算，与 `term-script` 的 `DEFAULT_MAX_WAIT` 同口径：
/// 真正的超时由每个动词的 `timeout_ms` 收口，这里只是防止游标构造出一个
/// 瞬时到期的视图（ADR §4 的「断连即取消」由 `CancelToken` 负责）。
const CURSOR_MAX_WAIT: Duration = Duration::from_secs(24 * 60 * 60);

/// 前台进程里唯一一份仲裁器：GUI 的规则 / 键盘与所有 API 连接共用它，
/// 否则「显式调用 vs 用户规则」只会各查各的表，判据形同虚设。
static SHARED_ARBITER: OnceLock<Arc<Mutex<Arbitrator>>> = OnceLock::new();

pub fn shared_arbiter() -> &'static Mutex<Arbitrator> {
    SHARED_ARBITER.get_or_init(|| Arc::new(Mutex::new(Arbitrator::new()))).as_ref()
}

/// 需要持有 `Arc` 的调用点（例如临时 `DeskSurface`）用这个入口。
pub fn shared_arbiter_arc() -> Arc<Mutex<Arbitrator>> {
    SHARED_ARBITER.get_or_init(|| Arc::new(Mutex::new(Arbitrator::new()))).clone()
}

/// 仲裁键：把池的进程内唯一 scope 号与会话 id 混成一个键，UI 与桥接共用。
///
/// 不能用 `Arc::as_ptr(pool)`：池 drop 后地址会被下一个池复用，旧池残留的
/// P3 owner 就会压住新池同号会话的写（跨测试 owner 串台的成因）。scope 号
/// 由 `AtomicU64` 单调分配、不回收，因此不同池 / 不同会话互不串；同一池
/// 同一会话仍拿到同一个键。
pub(crate) fn arbiter_session(pool: &Arc<SessionPool>, id: SessionId) -> SessionId {
    let scope = pool.scope_id(); scope.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ id
}

/// 把 `SessionPool` 包成桥接的后端。
pub struct DeskSurface {
    pool: Arc<SessionPool>,
    /// ADR-0030：API 写与 UI 规则写共用同一个仲裁器。
    arbiter: Arc<Mutex<Arbitrator>>,
    /// ADR-0023/0030：按会话一份 `ScriptSession`，登记处由宿主构造并同时交给
    /// `ScriptWorkspace`，所以脚本与 API 读写同一会话时续的是同一个读点。
    sessions: ScriptSessions,
    /// ADR §4「连接断开即取消」：每条控制连接一份令牌。注入到这里之后，
    /// 所有阻塞原语都走 `*_cancellable`（`wait` 显式分派，`ScriptSession`
    /// 家族由构造时的令牌接管），对端 EOF / IO 错误时由传输层 cancel。
    cancel: Option<CancelToken>,
}

impl DeskSurface {
    pub fn new(pool: Arc<SessionPool>) -> Self {
        Self::with_sessions(pool, ScriptSessions::shared().handle())
    }

    /// 夹具专用：用一份独立的仲裁器，避免不同测试互相抢占；游标登记处仍取宿主那一份。
    pub fn with_arbiter(pool: Arc<SessionPool>, arbiter: Arc<Mutex<Arbitrator>>) -> Self {
        Self {
            pool,
            arbiter,
            sessions: ScriptSessions::shared().handle(),
            cancel: None,
        }
    }

    /// 显式装配：调用方把与脚本侧同一份游标登记处交进来。
    pub fn with_sessions(pool: Arc<SessionPool>, sessions: ScriptSessions) -> Self {
        Self {
            pool,
            arbiter: shared_arbiter_arc(),
            sessions,
            cancel: None,
        }
    }

    /// 一条控制连接的装配：带上这条连接专属的取消令牌与游标登记处。
    /// 登记处按连接隔离，否则先建好游标的另一条连接会让令牌注入落空。
    pub fn with_cancel(
        pool: Arc<SessionPool>,
        sessions: ScriptSessions,
        cancel: CancelToken,
    ) -> Self {
        Self {
            pool,
            arbiter: shared_arbiter_arc(),
            sessions,
            cancel: Some(cancel),
        }
    }

    /// P2 显式调用的申请 / 回收。默认非独占；只有 `exclusive: true` 才抢 owner。
    fn arbitrate_write(&self, tab: SessionId, source: &str, exclusive: bool) -> Result<Grant, SurfaceError> {
        let key = arbiter_session(&self.pool, tab);
        let mut arbiter = self.arbiter.lock().unwrap_or_else(|e| e.into_inner());
        arbiter.request(key, source, Priority::P2Explicit, exclusive).map_err(|denied| {
            // Busy = 时序冲突，稍后重试；Suppressed = 本次写没产生。
            // 两者都不是「锁屏」——那一种是 SurfaceError::locked，码 -32003。
            let kind = if denied.is_busy() {
                term_core::surface_error::ErrorKind::Busy
            } else {
                term_core::surface_error::ErrorKind::Suppressed
            };
            SurfaceError::new(kind, denied.as_message())
        })
    }

    /// 上层（规则 / 凭据）已经拿过仲裁许可的写：走同一份动词实现，但**不**
    /// 二次申请 owner，也不把规则 owner 当成 API 独占那样释放掉。
    pub fn invoke_granted(&self, method: &str, params: &Value, grant: &Grant) -> Result<Value, SurfaceError> {
        match method {
            "write" => {
                let text = required_string(params, "text")?;
                let (session, tab) = self.resolve(params)?;
                self.send_granted(tab, &session, text.as_bytes(), grant, false)
            }
            "keys" => {
                let names = required_string_array(params, "keys")?;
                key_bytes(&names, false)?;
                let (session, tab) = self.resolve(params)?;
                let bytes = key_bytes(&names, self.app_cursor_keys(tab, &names))?;
                self.send_granted(tab, &session, &bytes, grant, true)
            }
            // 非写动词本来就不进仲裁器，直接落到唯一的动词实现。
            other => self.invoke(other, params),
        }
    }

    /// 拿到许可后的唯一写出点。`count` 只决定 `keys` 要不要回带字节数。
    ///
    /// # TOCTOU（P1-2）
    ///
    /// 「申请许可」与「写 PTY」之间可能发生 P0 按键 / 独占抢占，epoch 随之
    /// 前进。复核与落地必须在**同一把仲裁器锁**里：分开两次加锁，第二次锁
    /// 之前照样能被抢占，检查就只是摆设（这正是原实现的行为）。
    fn send_granted(
        &self,
        tab: SessionId,
        session: &str,
        bytes: &[u8],
        grant: &Grant,
        count: bool,
    ) -> Result<Value, SurfaceError> {
        let key = arbiter_session(&self.pool, tab);
        let written = {
            let mut arbiter = self.arbiter.lock().unwrap_or_else(|e| e.into_inner());
            if !arbiter.is_valid(key, grant) {
                // 不写、也不动计数；抢占方已经记过一次压制。这里再留一条
                // 可见痕迹：错误回到调用方（API 客户端 / UI 提示行），
                // stderr 给服务端排障——绝不静默吞掉这次未落地。
                eprintln!(
                    "Grant {}:{} 在落地前失效，会话 {session:?} 本次未写入（按键 / 独占抢占）",
                    grant.seq, grant.epoch
                );
                return Err(SurfaceError::busy(format!(
                    "Busy: 会话 {session:?} 的写在落地前被按键 / 独占抢占（seq {}），本次未写入，请稍后重试",
                    grant.seq
                )));
            }
            let written = self.pool.send(tab, bytes);
            arbiter.consume(key, grant);
            written
        };
        if !written {
            return Err(SurfaceError::not_found(format!("会话 {session:?} 不存在，没有可写入的会话")));
        }
        Ok(if count {
            json!({ "written": true, "bytes": bytes.len() })
        } else {
            json!({ "written": true })
        })
    }

    /// 取（或建立）该会话的捕获游标句柄。会话不在池里 → not_found。
    ///
    /// 参数仍是 `SessionId`：游标按 id 存（它是进程内的句柄），但调用方必须
    /// 已经用名字解析过了——这里不认识名字。
    fn cursor(&self, tab: SessionId) -> Result<ScriptSession, SurfaceError> {
        self.sessions.get_or_try_insert(tab, || {
            let handle = self
                .pool
                .session(tab)
                .ok_or_else(|| SurfaceError::not_found(format!("会话 {tab} 不在池子里")))?;
            // 连入的游标必须带上本连接的令牌：ScriptSession 的 read /
            // read_until / wait_any / wait_idle 据此走 *_cancellable 切片。
            let cursor = match &self.cancel {
                Some(cancel) => {
                    ScriptSession::with_cancel(handle, CURSOR_MAX_WAIT, Some(cancel.clone()))
                }
                None => ScriptSession::new(handle),
            };
            Ok(cursor)
        })
    }

    /// `wait` 的取消分派：有令牌走切片等待，没有则保持老语义。
    fn wait_with_cancel(
        &self,
        registration: Box<dyn WaitRegistration>,
        timeout: Duration,
    ) -> Result<WaitMatch, WaitError> {
        match &self.cancel {
            Some(cancel) => registration.wait_cancellable(timeout, cancel),
            None => registration.wait(timeout),
        }
    }

    /// `session` 参数 → `(名字, id)`。**所有**收会话参数的动词都从这里进。
    ///
    /// 为什么返回名字：失败文案必须点名「是哪一条会话」——客户端手里同时握着
    /// 好几个名字，只说「不存在」等于让人一个个试。
    ///
    /// 找不到就是 `not_found`，**绝不**退回第一条：那会把「名字写错」变成
    /// 「打中了另一条会话」，而这正是 ADR-0024 决策 4 要消灭的静默。
    ///
    /// # 调用顺序：参数形状先于会话存在性
    ///
    /// 每个动词都先校验自己的**其它**参数，最后才调这里解析名字。理由：
    /// 「缺 text」「patterns 里有非字符串」这类错与「会话在不在」无关，先报
    /// 它们客户端改起来更直接；反过来先查名字的话，一个明显写坏的请求会被
    /// 报成 `not_found`，客户端会去查会话列表——查了也没用。
    fn resolve(&self, params: &Value) -> Result<(String, SessionId), SurfaceError> {
        let name = required_session(params)?;
        match self.pool.resolve(&name) {
            Some(id) => Ok((name, id)),
            None => Err(SurfaceError::not_found(format!(
                "会话 {name:?} 不存在（见 session_list）"
            ))),
        }
    }

    fn session_list(&self) -> Result<Value, SurfaceError> {
        let sessions: Vec<Value> = self
            .pool
            .list()
            .into_iter()
            .map(|(_, name, state)| session_descriptor(state, &name))
            .collect();
        Ok(json!({ "sessions": sessions }))
    }

    fn session_open(&self, params: &Value) -> Result<Value, SurfaceError> {
        let command = optional_string(params, "command")?;
        let name = optional_string(params, "name")?;
        let cols = optional_u64(params, "cols")?.unwrap_or(DEFAULT_COLS) as usize;
        let rows = optional_u64(params, "rows")?.unwrap_or(DEFAULT_ROWS) as usize;

        let opened = match command {
            Some(command) => {
                // exec 会话在控制面里以命令文本为名；显式 name 不该撒谎覆盖它
                // （它描述的是「这条命令」，不是用户起的别名）。
                self.pool
                    .open_exec(&command, cols, rows)
                    .map_err(OpenFailure::Session)
            }
            None => {
                // 登录名由宿主注入（core 不读 `$USER`）。
                let mut config = SessionConfig::new_local_shell(&term_io::host::login());
                if let Some(explicit) = name {
                    config.name = explicit;
                }
                self.open_config(&config, cols, rows)
            }
        };

        let tab = opened.map_err(open_failure)?;
        // 立刻建好这条会话的读取游标。`ScriptSession` 是在**构造时**登记捕获器
        // 的（ADR-0019 先登记后消费），所以懒创建等于把「session_open 之后、
        // 第一次 read 之前」到达的输出**永久丢掉**：客户端典型写法正好是
        // `write` → `wait` → `read`，等它第一次 read 时游标才登记，前面那段
        // 已经不在它的窗口里了。脚本侧没有这个洞（游标在脚本开跑之前就建好），
        // 两侧语义要一致，就得在开会话这一点上对齐。
        //
        // 界面里自己开的会话不在此列：客户端是**后来才接上**的，从接上那一刻
        // 开始读本来就是它的语义。建失败不在这里报错（会话可能已经死了），
        // 留给首次读取显式报 `Closed`。
        let _ = self.cursor(tab);
        // 返回**真名**，不是调用方想要的 base：重名时池子会加后缀
        // （`cat` → `cat-2`），拿 base 回去寻址会打中另一条会话（ADR-0024）。
        let real = self.pool.name(tab).ok_or_else(|| {
            SurfaceError::internal(format!("会话 {tab} 刚建出来，池子却给不出它的名字"))
        })?;
        // 返回**完整的会话描述符**（与 session_list 里的一行同形）：客户端拿到
        // 之后可以直接塞回同一个列表、或原样转给 session_close，不需要记两套键名。
        //
        // 刚开的会话查不到状态只可能是池子在开完之后立刻丢了它；那时报
        // not_found 会让客户端以为「没开成」，而会话其实已经建出来了。
        // 退回 Alive，让客户端下一次 session_list 自己看到真相。
        let state = self.pool.state(tab).unwrap_or(SessionState::Alive);
        Ok(session_descriptor(state, &real))
    }

    /// 非 exec 会话的统一开法：**只经** [`crate::session_open::open_session`]。
    ///
    /// 为什么单独一个方法而不是在 `session_open` 里内联：能力面这八条方法**不许**
    /// 出现协议专有参数（ADR-0019），所以 `session_open` 的线上形状里永远没有
    /// 「用内置 SSH 连哪台机器」。协议注入点在控制面之下由
    /// `session_open` 决定，这一层既不认识 `SshOptions` 也不认识 russh。
    fn open_config(
        &self,
        config: &SessionConfig,
        cols: usize,
        rows: usize,
    ) -> Result<SessionId, OpenFailure> {
        open_session(&self.pool, config, cols, rows)
    }

    fn session_close(&self, params: &Value) -> Result<Value, SurfaceError> {
        let (session, tab) = self.resolve(params)?;
        // 先摘游标再关会话：否则关掉的会话会在登记处留一个孤儿句柄，
        // 它握着的 `Arc<Mutex<dyn Session>>` 让会话本体无法释放。
        self.sessions.remove(tab);
        // 会话没了，仲裁器里的 owner / 相位 / 留痕一并清掉。不清的话
        // 死会话会一直占着 owner，后续写被它压制（即便会话 id 不复用，
        // 旧状态也会在状态行里诈尸）。
        let key = arbiter_session(&self.pool, tab);
        self.arbiter.lock().unwrap_or_else(|e| e.into_inner()).clear(key);
        if !self.pool.close(tab) {
            return Err(SurfaceError::not_found(format!(
                "会话 {session:?} 不存在（已关闭的会话不会再关一次，也不会假装成功）"
            )));
        }
        Ok(json!({ "closed": true }))
    }

    fn write(&self, params: &Value) -> Result<Value, SurfaceError> {
        let text = required_string(params, "text")?;
        let exclusive = optional_bool(params, "exclusive")?.unwrap_or(false);
        let (session, tab) = self.resolve(params)?;
        let source = "api:write";
        let grant = self.arbitrate_write(tab, source, exclusive)?;
        let result = self.send_granted(tab, &session, text.as_bytes(), &grant, false);
        let key = arbiter_session(&self.pool, tab);
        self.arbiter.lock().unwrap_or_else(|e| e.into_inner()).release(key, source);
        result
    }

    /// `keys`：按键名写字节（`write` 的姊妹动词）。
    ///
    /// 与 `write` 分开的理由不是风格：`write("ls\n")` 是**原样字节**，`keys(["ls","enter"])`
    /// 是**按键语义**——后者要知道 `enter` 是 CR 而不是 LF、方向键要不要按
    /// `DECSET 1` 发 `ESC O A`。把两件事塞进一个动词，就得在运行时猜这串字符
    /// 到底是文本还是键名，猜错时静默发错字节。
    fn keys(&self, params: &Value) -> Result<Value, SurfaceError> {
        let names = required_string_array(params, "keys")?;
        // 先用「非 application cursor」翻一遍，只为了**校验键名**：认不出的键名
        // 是客户端 bug，与「会话在不在」无关，不该被 not_found 挡住。方向键在
        // 两种模式下都在键名表里，所以这一遍不会误判。
        key_bytes(&names, false)?;
        let (session, tab) = self.resolve(params)?;
        // 方向键的编码取决于会话当前的 `DECSET 1`；其余键名与终端状态无关。
        // 只在真的出现方向键时问一次屏幕，避免每次按键都多一次往返。
        let bytes = key_bytes(&names, self.app_cursor_keys(tab, &names))?;
        let exclusive = optional_bool(params, "exclusive")?.unwrap_or(false);
        let source = "api:keys";
        let grant = self.arbitrate_write(tab, source, exclusive)?;
        let result = self.send_granted(tab, &session, &bytes, &grant, true);
        let key = arbiter_session(&self.pool, tab);
        self.arbiter.lock().unwrap_or_else(|e| e.into_inner()).release(key, source);
        result
    }

    /// 这条会话是否处于 `DECSET 1`（application cursor keys）模式。
    ///
    /// 只有方向键关心它；没有方向键时**不问屏幕**，省掉每次按键一次往返。
    fn app_cursor_keys(&self, tab: SessionId, names: &[String]) -> bool {
        if names.iter().any(|name| term_core::keys::is_arrow(name)) {
            self.pool
                .screen(tab)
                .is_some_and(|snapshot| snapshot.application_cursor_keys)
        } else {
            false
        }
    }

    fn wait(&self, params: &Value) -> Result<Value, SurfaceError> {
        let pattern = required_string(params, "pattern")?;
        if pattern.is_empty() {
            // 空模式会立刻命中任何东西——那是调用方的 bug，直接拒绝而不是
            // 让它「成功」返回一段随机文本。
            return Err(SurfaceError::invalid_params(
                "参数 pattern 不能是空串：空模式会立刻命中任何输出",
            ));
        }
        let timeout_ms = optional_u64(params, "timeout_ms")?.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
        let (_, tab) = self.resolve(params)?;

        // 先登记、后匹配（ADR-0019）：wait_register 返回即已生效，
        // 之后的输出保证被匹配；登记点之前的输出不入账。
        let registration = self.pool.wait_register(tab, &pattern)?;
        let matched = self
            .wait_with_cancel(registration, Duration::from_millis(timeout_ms))
            .map_err(|err| match err {
                WaitError::Timeout => SurfaceError::timeout(format!(
                    "{timeout_ms}ms 内没等到 {pattern:?}（会话仍在，可加大 timeout_ms 重试）"
                )),
                other => SurfaceError::from(other),
            })?;
        Ok(json!({ "text_upto": matched.text_upto }))
    }

    fn wait_idle(&self, params: &Value) -> Result<Value, SurfaceError> {
        // `quiet_ms` 是必填：给个缺省值等于替调用方决定「多安静算安静」，
        // 而这个数因场景差别很大（本地命令 50ms、跨网 500ms）。
        let quiet_ms = optional_u64(params, "quiet_ms")?.ok_or_else(|| {
            SurfaceError::invalid_params("缺少必填参数 quiet_ms（多少毫秒没有新输出算「停下来」）")
        })?;
        if quiet_ms == 0 {
            return Err(SurfaceError::invalid_params(
                "quiet_ms 必须大于 0：0 毫秒的安静窗口会让它立刻返回",
            ));
        }
        let timeout_ms = optional_u64(params, "timeout_ms")?.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
        let (_, tab) = self.resolve(params)?;
        let text = self
            .cursor(tab)?
            .wait_idle(quiet_ms as f64 / 1000.0, timeout_ms as f64 / 1000.0)
            .map_err(primitive_failure)?;
        Ok(json!({ "text": text }))
    }

    fn screen(&self, params: &Value) -> Result<Value, SurfaceError> {
        let (session, tab) = self.resolve(params)?;
        match self.pool.screen(tab) {
            Some(snapshot) => Ok(json!({
                "text": snapshot.screen_text(),
                "cols": snapshot.cols,
                "rows": snapshot.rows,
                // ADR-0023：`cursor_pos` 是脚本专用动词，远程从这两个字段取
                // 同样的两个数——不必为它多开一次往返。
                "cursor_row": snapshot.cursor.0,
                "cursor_col": snapshot.cursor.1,
            })),
            // `screen()` 返回 None 有两种含义：tab 不在册，或会话已结束到没有
            // 事实可取。查一次 state 把两种分开——回一个空屏会让调用方以为
            // 「屏幕就是空的」。
            None => match self.pool.state(tab) {
                None => Err(SurfaceError::not_found(format!("会话 {session:?} 不存在"))),
                Some(SessionState::Finished(_)) => Err(SurfaceError::closed(format!(
                    "会话 {session:?} 已结束，没有屏幕事实可取"
                ))),
                Some(SessionState::Alive) => Err(SurfaceError::internal(format!(
                    "会话 {session:?} 存活却取不到屏幕快照——桥接侧的故障"
                ))),
            },
        }
    }

    fn state(&self, params: &Value) -> Result<Value, SurfaceError> {
        let (session, tab) = self.resolve(params)?;
        match self.pool.state(tab) {
            None => Err(SurfaceError::not_found(format!("会话 {session:?} 不存在"))),
            Some(SessionState::Alive) => Ok(json!({ "alive": true, "exit": Value::Null })),
            Some(SessionState::Finished(exit)) => Ok(json!({ "alive": false, "exit": exit })),
        }
    }

    fn facets(&self, params: &Value) -> Result<Value, SurfaceError> {
        let (session, tab) = self.resolve(params)?;
        match self.pool.facets(tab) {
            None => Err(SurfaceError::not_found(format!("会话 {session:?} 不存在"))),
            Some(kinds) => {
                let names: Vec<&'static str> = kinds.into_iter().map(facet_name).collect();
                Ok(json!({ "facets": names }))
            }
        }
    }

    // ── ADR-0023：流式与日志动词（与脚本共用 ScriptSession 的语义） ──────
    //
    // 这六个方法不是「给远程单独写一套」：read / read_until / wait_any 直接
    // 委托给 per-tab 的 `ScriptSession`，日志三件套委托给 `Session` 的
    // log_* 门面。语义只有一份，两侧测试共享同一组期望。

    fn read(&self, params: &Value) -> Result<Value, SurfaceError> {
        // 缺省 0 = 只排水不等（与脚本 `read()` 零参重载同一语义）。
        let timeout_ms = optional_u64(params, "timeout_ms")?.unwrap_or(0);
        let (_, tab) = self.resolve(params)?;
        let text = self
            .cursor(tab)?
            .read(timeout_ms as f64 / 1000.0, None)
            .map_err(primitive_failure)?;
        Ok(json!({ "text": text }))
    }

    fn read_until(&self, params: &Value) -> Result<Value, SurfaceError> {
        let delimiter = required_string(params, "delimiter")?;
        if delimiter.is_empty() {
            return Err(SurfaceError::invalid_params(
                "参数 delimiter 不能是空串：空哨兵会立刻命中任何输出",
            ));
        }
        let timeout_ms = optional_u64(params, "timeout_ms")?.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
        let (_, tab) = self.resolve(params)?;
        let text = self
            .cursor(tab)?
            .read_until(&delimiter, timeout_ms as f64 / 1000.0, None)
            .map_err(primitive_failure)?;
        Ok(json!({ "text": text }))
    }

    fn wait_any(&self, params: &Value) -> Result<Value, SurfaceError> {
        let patterns = required_string_array(params, "patterns")?;
        let timeout_ms = optional_u64(params, "timeout_ms")?.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
        let (_, tab) = self.resolve(params)?;
        let hit = self
            .cursor(tab)?
            .wait_any(&patterns, timeout_ms as f64 / 1000.0, None)
            .map_err(primitive_failure)?;
        Ok(json!({ "index": hit.index, "text": hit.text }))
    }

    fn log_start(&self, params: &Value) -> Result<Value, SurfaceError> {
        let dir = required_string(params, "dir")?;
        let (name, tab) = self.resolve(params)?;
        // 会话名进文件名——就是寻址名本身，不必再向清单要一遍：
        // 名字在开会话时定下、之后不再变（ADR-0024）。
        //
        // 格式选项取设置页那一份（同一个用户在 `prefs.json` 里配的），
        // 不另立一套客户端默认值：否则同样的会话，界面里点 Start 与 API
        // 客户端调 `log_start` 会写出两种格式，事后没法比对。
        let opts = term_runtime::prefs::load()
            .unwrap_or_default()
            .log_options();
        self.cursor(tab)?
            .log_start_with(&dir, &name, &opts)
            .map_err(primitive_failure)?;
        let status = self.pool.log_status(tab).unwrap_or_default();
        Ok(json!({
            "recording": true,
            "path": status.path.map(|p| p.display().to_string()),
        }))
    }

    fn log_stop(&self, params: &Value) -> Result<Value, SurfaceError> {
        let (_, tab) = self.resolve(params)?;
        self.cursor(tab)?.log_stop().map_err(primitive_failure)?;
        Ok(json!({ "recording": false }))
    }

    fn log_status(&self, params: &Value) -> Result<Value, SurfaceError> {
        let (session, tab) = self.resolve(params)?;
        if self.pool.state(tab).is_none() {
            return Err(SurfaceError::not_found(format!("会话 {session:?} 不存在")));
        }
        let status = self.pool.log_status(tab).unwrap_or_default();
        let phase = match &status.phase {
            term_core::session_runtime::LogPhase::Off => "off",
            term_core::session_runtime::LogPhase::Recording => "recording",
            term_core::session_runtime::LogPhase::Paused => "paused",
            term_core::session_runtime::LogPhase::Failed(_) => "failed",
        };
        let reason = match &status.phase {
            term_core::session_runtime::LogPhase::Failed(why) => Value::String(why.clone()),
            _ => Value::Null,
        };
        Ok(json!({
            "phase": phase,
            "path": status.path.map(|p| p.display().to_string()),
            "reason": reason,
        }))
    }
}

impl Surface for DeskSurface {
    fn call(&self, method: &str, params: &Value) -> Result<Value, SurfaceError> {
        match method {
            // describe 是协议内建方法（`jsonrpc::dispatch` 会直接拦截），
            // 这里仍然实现一遍：直连 `call` 的调用方（测试、将来进程内的
            // agent）必须拿到与线上**完全相同**的答案。
            "describe" => Ok(describe_value()),
            "session_list" => self.session_list(),
            "session_open" => self.session_open(params),
            "session_close" => self.session_close(params),
            "write" => self.write(params),
            "keys" => self.keys(params),
            "wait" => self.wait(params),
            "screen" => self.screen(params),
            "state" => self.state(params),
            "facets" => self.facets(params),
            // ADR-0023：流式与日志动词。`cursor_pos` / `terminal_size` 是
            // 脚本专用，故意**不**在这里出现——远程从 screen 的结果字段取。
            "read" => self.read(params),
            "read_until" => self.read_until(params),
            "wait_any" => self.wait_any(params),
            "wait_idle" => self.wait_idle(params),
            "log_start" => self.log_start(params),
            "log_stop" => self.log_stop(params),
            "log_status" => self.log_status(params),
            other => Err(SurfaceError::method_not_found(format!(
                "未知方法 {other}；可用方法见 describe"
            ))),
        }
    }
}

// 夹具与断言较多（14 条），单独成文件：单文件上限管的是**代码**行，
// 而这些用例只是回归，不该把翻译层顶出上限。见 `lock_tests.rs` 的同款处理。
#[cfg(test)]
#[path = "surface_tests.rs"]
mod surface_tests;
