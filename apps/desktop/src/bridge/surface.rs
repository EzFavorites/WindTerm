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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use term_core::contract::{
    describe_value, optional_string, optional_u64, required_session, required_string, Surface,
};
use term_core::session::SessionConfig;
use term_core::session_runtime::{FacetKind, SessionId, SessionState, WaitError};
use term_core::surface_error::SurfaceError;
use term_script::{PrimitiveError, ScriptSession};
use term_session::{OpenError, SessionPool};

use crate::session_open::{open_session, OpenFailure};

/// `wait` 的缺省上限：与 `term_core::surface` 里 `wait.timeout_ms` 的说明一致。
const DEFAULT_WAIT_TIMEOUT_MS: u64 = 30_000;

/// `session_open` 的缺省尺寸（与 ADR-0019 的 80×24 对齐）。
const DEFAULT_COLS: u64 = 80;
const DEFAULT_ROWS: u64 = 24;

/// 把 `SessionPool` 包成桥接的后端。
pub struct DeskSurface {
    pool: Arc<SessionPool>,
    /// ADR-0023：每个会话一份 `ScriptSession`，让远程的
    /// `read` / `read_until` / `wait_any` 与脚本共享**同一套捕获游标语义**。
    ///
    /// 为什么必须有状态：`read_until` 的承诺是「自上次取走点往后读到哨兵」。
    /// 无状态实现只能每次从屏幕重读——那就是洪水输出下漏读（ADR-0019）。
    /// 桥接是一条串行管道（`handler.rs` 硬约束 3），持游标不引入并发问题。
    cursors: Mutex<HashMap<SessionId, ScriptSession>>,
}

impl DeskSurface {
    pub fn new(pool: Arc<SessionPool>) -> Self {
        Self {
            pool,
            cursors: Mutex::new(HashMap::new()),
        }
    }

    /// 取（或建立）该会话的捕获游标句柄。会话不在池里 → not_found。
    ///
    /// 参数仍是 `SessionId`：游标按 id 存（它是进程内的句柄），但调用方必须
    /// 已经用名字解析过了——这里不认识名字。
    fn cursor(&self, tab: SessionId) -> Result<ScriptSession, SurfaceError> {
        let mut map = self.cursors.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = map.get(&tab) {
            return Ok(existing.clone());
        }
        let handle = self
            .pool
            .session(tab)
            .ok_or_else(|| SurfaceError::not_found(format!("会话 {tab} 不在池子里")))?;
        let session = ScriptSession::new(handle);
        map.insert(tab, session.clone());
        Ok(session)
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
        // 先摘游标再关会话：否则关掉的会话会在 cursors 里留一个孤儿句柄，
        // 它握着的 `Arc<Mutex<dyn Session>>` 让会话本体无法释放。
        self.cursors.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&tab);
        if !self.pool.close(tab) {
            return Err(SurfaceError::not_found(format!(
                "会话 {session:?} 不存在（已关闭的会话不会再关一次，也不会假装成功）"
            )));
        }
        Ok(json!({ "closed": true }))
    }

    fn write(&self, params: &Value) -> Result<Value, SurfaceError> {
        let text = required_string(params, "text")?;
        let (session, tab) = self.resolve(params)?;
        if !self.pool.send(tab, text.as_bytes()) {
            return Err(SurfaceError::not_found(format!(
                "会话 {session:?} 不存在，没有可写入的会话"
            )));
        }
        Ok(json!({ "written": true }))
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
        let written = self.pool.send(tab, &bytes);
        if !written {
            return Err(SurfaceError::not_found(format!(
                "会话 {session:?} 不存在，没有可写入的会话"
            )));
        }
        Ok(json!({ "written": true, "bytes": bytes.len() }))
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
        let matched = registration
            .wait(Duration::from_millis(timeout_ms))
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
        // 不另立一套无头默认值：否则同样的会话，界面里点 Start 与 agent
        // 调 `log_start` 会写出两种格式，事后没法比对。
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

/// `PrimitiveError` → 线上失败。
///
/// 分档的理由与 `term-script` 的 `Display` 文案一致：客户端要看得出「再等等」
/// （timeout）与「改参数」（invalid_params）与「状态不对」（invalid_request）的区别。
fn primitive_failure(err: PrimitiveError) -> SurfaceError {
    match err {
        PrimitiveError::EmptyPattern => SurfaceError::invalid_params(err.to_string()),
        PrimitiveError::Timeout => SurfaceError::timeout(err.to_string()),
        PrimitiveError::Closed => SurfaceError::closed(err.to_string()),
        PrimitiveError::AlreadyRecording | PrimitiveError::NotRecording => {
            SurfaceError::invalid_request(err.to_string())
        }
        // 全局层那三类。远程侧目前**没有**走这条路的动词（`session_*` 由
        // `session_open` / `session_close` 自己管，参数校验也在那一层），所以这里
        // 不是「将来会用」，而是「脚本层与远程层共用同一个错误枚举」的必然：
        // 枚举加了变体，这里的 match 就得表态。按语义归类——点错名字是参数
        // 问题，没有绑定是状态问题。
        PrimitiveError::NoSuchSession(_) | PrimitiveError::BadArgument(_) => {
            SurfaceError::invalid_params(err.to_string())
        }
        PrimitiveError::CloseCurrentSession(_) | PrimitiveError::NoBinding => {
            SurfaceError::invalid_request(err.to_string())
        }
        // 键名不认识是参数问题（改键名就好）；`NoInput` 走不到这里——`ask`
        // 是脚本专有动词，远程侧没有那条调用路径。留一臂是为了让「枚举加了
        // 变体，映射得表态」这条规矩继续成立。
        PrimitiveError::UnknownKey(_) => SurfaceError::invalid_params(err.to_string()),
        // 名字不在某一层的表里（停靠面板名），同样是「改名字就好」= 参数问题。
        // 远程侧目前走不到（`app_*` 是 Script-only），留一臂的理由同上。
        PrimitiveError::UnknownName { .. } => SurfaceError::invalid_params(err.to_string()),
        PrimitiveError::NoInput => SurfaceError::invalid_request(err.to_string()),
        // ADR-0027 新增的四类。远程侧同样走不到它们（取消 / 授权 / 端口不支持
        // 都是**脚本运行时**的概念，RPC 那头没有脚本线程），但枚举加了变体，
        // 映射就得表态——按语义归类，别让将来真接上时临时拍脑袋：
        // 「有人叫停」是独立的终态，客户端不该自动重试；
        // 「没授权」与「宿主没这个能力」都属于「这条路走不通」，且**重试无用**
        // （要改的是授权声明或换宿主），所以归到 locked 而不是 internal。
        PrimitiveError::Cancelled => SurfaceError::cancelled(err.to_string()),
        PrimitiveError::Denied(_) | PrimitiveError::Unsupported(_) => {
            SurfaceError::new(term_core::surface_error::ErrorKind::Locked, err.to_string())
        }
        PrimitiveError::Failed(_) => SurfaceError::internal(err.to_string()),
    }
}

/// 按键名表把一串键名翻成字节。未知键名 → `invalid_params`（不是静默丢弃：
/// 丢掉一个键会让脚本在后面某一步等一个永远不会来的提示）。
///
/// `app_cursor` 只影响方向键（`ESC [ A` vs `ESC O A`），其余键名与终端状态无关。
fn key_bytes(names: &[String], app_cursor: bool) -> Result<Vec<u8>, SurfaceError> {
    if names.is_empty() {
        return Err(SurfaceError::invalid_params(
            "参数 keys 不能是空数组：一次按键都没有",
        ));
    }
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let Some(bytes) = term_core::keys::key_bytes(name, app_cursor) else {
            return Err(SurfaceError::invalid_params(format!(
                "未知键名 {name:?}；可用 {}",
                term_core::keys::known_names()
            )));
        };
        out.extend_from_slice(bytes.as_bytes());
    }
    Ok(out)
}

/// 取必填的字符串数组参数（`wait_any` 的 `patterns`、`keys` 的 `keys`）。
///
/// 非字符串元素显式报错并点名是第几项——静默 `to_string()` 会把
/// `["OK", 7]` 变成「在等 '7'」，脚本作者永远查不出来。
fn required_string_array(params: &Value, key: &str) -> Result<Vec<String>, SurfaceError> {
    let value = match params.get(key) {
        None | Some(Value::Null) => {
            return Err(SurfaceError::invalid_params(format!(
                "缺少必填参数 {key}（字符串数组）"
            )))
        }
        Some(value) => value,
    };
    let items = value
        .as_array()
        .ok_or_else(|| SurfaceError::invalid_params(format!("参数 {key} 必须是数组（实际：{value}）")))?;
    if items.is_empty() {
        return Err(SurfaceError::invalid_params(format!("参数 {key} 至少要有 1 项")));
    }
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            item.as_str().map(str::to_string).ok_or_else(|| {
                SurfaceError::invalid_params(format!(
                    "参数 {key} 第 {} 项必须是字符串（实际：{item}）",
                    i + 1
                ))
            })
        })
        .collect()
}

/// 会话描述符：**所有**返回会话的动词都用这一个形状（`name` / `alive` / `exit`）。
///
/// `session_list` 的一行、`session_open` 的结果、脚本侧 `session_open` /
/// `session_use` 的结果完全同形。为什么值得单独一个函数：两个前端各拼一次
/// 就是漂移的起点（ADR-0023 的「一个动词两个前端一种形状」）。
///
/// **不含 id**：线上形状里没有进程内自增的句柄（ADR-0024）。客户端要再指同一条
/// 会话，用的就是这里的 `name`。
fn session_descriptor(state: SessionState, name: &str) -> Value {
    let (alive, exit) = split_state(state);
    json!({"name": name, "alive": alive, "exit": exit})
}

/// 会话状态 → `(alive, exit)`。
fn split_state(state: SessionState) -> (bool, Option<String>) {
    match state {
        SessionState::Alive => (true, None),
        SessionState::Finished(exit) => (false, exit),
    }
}

/// 能力 facet 的线上名字。与 `term_core::surface` 的 `facets` 结果
/// schema 里的 `enum` 必须一一对应。
fn facet_name(kind: FacetKind) -> &'static str {
    match kind {
        FacetKind::FileTransfer => "file_transfer",
        FacetKind::PortForward => "port_forward",
    }
}

/// `OpenFailure` → 线上失败。四档分开，因为客户端要做的动作不同：
///
/// - `NeedsVault` → `locked`（-32003）。这不是「内部故障」，也不是「参数错了」：
///   它是**策略**拒绝——密码在保险库里，而 `desk serve` 从来不解锁保险库
///   （`serve_on` 只建 `SessionPool`）。与锁屏门用同一个码，客户端只需处理
///   「现在做不了，等条件变了再试」这一种情形。
/// - `Unsupported` / `Session::Spawn` / `Session::BuiltinChannelNeedsInjection`
///   → `invalid_request`：用户改会话配置或环境就能修。
/// - `Connect` → `internal`（-32603）：DNS / TCP / 握手 / 主机密钥都不是调用方
///   改参数能修的，重试同样一串参数结果一样。这不是「桥接自身的 bug」，但**码**
///   只能取这几个——`internal` 在这里的含义是「调用方无从修复」，比起把它
///   塞进 `invalid_params`（暗示「改参数就好了」）更不容易误导。
/// - `Session::Config` 是唯一走 `invalid_params` 的一档：那是**配置里缺东西**，
///   调用方补上就行（与这一层出现之前的映射逐字一致）。
fn open_failure(err: OpenFailure) -> SurfaceError {
    match err {
        OpenFailure::NeedsVault { session } => SurfaceError::locked(format!(
            "会话 {session} 需要解锁保险库才能连接；desk serve 是无窗口的，\
             请改用 Agent / 私钥认证的会话"
        )),
        OpenFailure::Unsupported(why) => SurfaceError::invalid_request(why),
        OpenFailure::Connect(why) => SurfaceError::internal(format!("内置 SSH 连接失败：{why}")),
        OpenFailure::Session(OpenError::Config(detail)) => {
            SurfaceError::invalid_params(detail.to_string())
        }
        OpenFailure::Session(OpenError::Spawn(detail)) => {
            SurfaceError::invalid_request(format!("起会话失败：{detail}"))
        }
        OpenFailure::Session(OpenError::BuiltinChannelNeedsInjection) => {
            // 走不到：内置 SSH 已经被 `open_session` 接住了。留着这一臂是为了
            // 「新增 OpenError 变体时编译器会提醒你回来想清楚映射」。
            SurfaceError::invalid_request(OpenError::BuiltinChannelNeedsInjection.to_string())
        }
        OpenFailure::Session(OpenError::Channel(failure)) => {
            // 工厂的失败分类在 `session_open::open_failure` 已经翻成
            // NeedsVault / Unsupported / Connect 三档，正常走不到这里；
            // 留一臂是为了控制面新增分类时不会静默丢进「起会话失败」。
            match failure {
                term_session::ChannelFailure::NeedsVault { session } => SurfaceError::locked(
                    format!("会话 {session} 需要解锁保险库才能建立通道"),
                ),
                term_session::ChannelFailure::Unsupported(why) => {
                    SurfaceError::invalid_request(why)
                }
                term_session::ChannelFailure::Connect(why) => {
                    SurfaceError::internal(format!("内置 SSH 连接失败：{why}"))
                }
            }
        }
    }
}

// 夹具与断言较多（14 条），单独成文件：单文件上限管的是**代码**行，
// 而这些用例只是回归，不该把翻译层顶出上限。见 `lock_tests.rs` 的同款处理。
#[cfg(test)]
#[path = "surface_tests.rs"]
mod surface_tests;
