//! 开发期参考实现：把会话控制面 `term-session::SessionPool` 包成 `Surface`。
//!
//! 为什么它在 `tests/` 而不在 `src/`：本 crate 的运行时依赖表必须保持
//! 「只有 term-core」这条单向边界（派单红线）。把 `PoolSurface` 放进 `src/`
//! 就得让 `term-session` 变成运行时依赖，那会把 PTY、线程、监管者一路拖进
//! 任何想用桥接的编译单元——而桥接本体根本不需要它们，它只需要一个
//! 「认得这些方法名」的后端。
//!
//! 用 `#[cfg(feature = ...)]` + `required-features` 把实现挡在 feature 后面
//! 也不行：`cargo test --workspace` 在 feature 未启用时会**静默跳过**整个
//! 验收用例。宁可放在夹具里，也不要让 S-c 的验收在门禁里悄悄消失。
//!
//! 这八个方法的映射就是「能力面 ↔ 控制面」的全部翻译：没有任何业务逻辑，
//! 只有「叫这个名字、收这个参数、还这个形状」。任何一处需要写
//! `if something { ... }` 的地方，说明该回到 `term-session` 去加能力，
//! 而不是在桥接里补锅。
//!
//! 寻址一律是**会话名**（ADR-0024）：每个动词先把 `session` 解析成
//! `SessionId`（[`PoolSurface::resolve`]），解析不到就是 `not_found` 并点名
//! 那个名字——绝不「找不到就取第一条」。
//!
//! # 它只实现本用例驱动的那些动词（2026-11-14 明确）
//!
//! 曾经这里有第九个方法 `wait_idle`——写了判据、但派发表里没有分支，于是
//! **一次都没执行过**。这种「测试基建里的死参考实现」比不写更糟：它让覆盖
//! 看起来齐了。判据是**谁在跑它**——没被任何用例调用的一律删掉，不留着
//! 「将来可能有用」。
//!
//! 所以完整性的责任不在本夹具，而在两处真实现：
//! - 产品 `DeskSurface`（`apps/desktop/src/bridge/surface.rs`）由
//!   `every_remote_verb_is_dispatched_and_script_only_verbs_are_not` 逐动词核对；
//! - 流式四动词（`read` / `read_until` / `wait_any` / `wait_idle`）的真行为由
//!   `apps/desktop` 里**真 TCP + 真会话**的
//!   `streaming_verbs_consume_output_once_on_a_real_connection` 覆盖。

#![allow(dead_code)] // 八个方法都必须存在；每个用例只碰其中几个。

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use term_bridge::error::SurfaceError;
use term_bridge::handler::{
    describe_value, optional_string, optional_u64, required_session, required_string, Surface,
};
use term_core::session::{Protocol, SessionConfig};
use term_core::session_runtime::{FacetKind, SessionId, SessionState, WaitError};
use term_session::{OpenError, SessionPool};

/// `wait` 的缺省上限：与 `term_core::surface` 里 `wait.timeout_ms` 的说明一致。
pub const DEFAULT_WAIT_TIMEOUT_MS: u64 = 30_000;

/// `session_open` 的缺省尺寸（与 ADR 里的 80×24 对齐）。
const DEFAULT_COLS: u64 = 80;
const DEFAULT_ROWS: u64 = 24;

/// 包一个 `SessionPool` 的 `Surface`。
pub struct PoolSurface {
    pool: Arc<SessionPool>,
}

impl PoolSurface {
    pub fn new(pool: Arc<SessionPool>) -> Self {
        Self { pool }
    }

    /// 拿回控制面（测试收尾时直接查登记表用）。
    pub fn pool(&self) -> &Arc<SessionPool> {
        &self.pool
    }

    /// `session` 参数 → `(名字, id)`。
    ///
    /// 返回名字是为了让每条失败文案都能点名「是哪一条会话」——脚本与客户端
    /// 手里同时握着好几个名字，只说「不存在」等于让人一个个试。
    fn resolve(&self, params: &Value) -> Result<(String, SessionId), SurfaceError> {
        let name = required_session(params)?;
        match self.pool.resolve(&name) {
            Some(id) => Ok((name, id)),
            None => Err(SurfaceError::not_found(format!(
                "会话 {name:?} 不存在（见 session_list；已关闭的会话不会再关一次）"
            ))),
        }
    }

    fn session_list(&self) -> Result<Value, SurfaceError> {
        let sessions: Vec<Value> = self
            .pool
            .list()
            .into_iter()
            .map(|(id, name, state)| {
                let (alive, exit) = split_state(state);
                // 名字取自池（寻址名），不是 `Tab.config.name`（显示名）：
                // 改名只改显示，不改寻址（ADR-0024 决策 3）。
                let _ = id;
                json!({"name": name, "alive": alive, "exit": exit})
            })
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
                // （它描述的是"这条命令"，不是用户起的别名）。
                self.pool.open_exec(&command, cols, rows)
            }
            None => {
                let mut config = SessionConfig::new(Protocol::Shell);
                if let Some(explicit) = name {
                    config.name = explicit;
                }
                self.pool.open(&config, cols, rows)
            }
        };

        let id = opened.map_err(open_error)?;
        // 返回**池里的真名**，不是调用方想要的 base：重名时池会加后缀
        // （`cat` → `cat-2`），拿 base 回去寻址会打中另一条会话（ADR-0024 §2）。
        let real = self.pool.name(id).ok_or_else(|| {
            SurfaceError::internal("池刚报开会话成功，转头又给不出它的名字")
        })?;
        Ok(json!({ "name": real, "alive": true, "exit": null }))
    }

    fn session_close(&self, params: &Value) -> Result<Value, SurfaceError> {
        let (name, id) = self.resolve(params)?;
        if !self.pool.close(id) {
            return Err(SurfaceError::not_found(format!(
                "会话 {name:?} 不存在（已关闭的会话不会再关一次，也不会假装成功）"
            )));
        }
        Ok(json!({ "closed": true }))
    }

    fn write(&self, params: &Value) -> Result<Value, SurfaceError> {
        let (name, id) = self.resolve(params)?;
        let text = required_string(params, "text")?;
        if !self.pool.send(id, text.as_bytes()) {
            return Err(SurfaceError::not_found(format!(
                "会话 {name:?} 不存在，没有可写入的会话"
            )));
        }
        Ok(json!({ "written": true }))
    }

    /// `keys`：键名 → 字节（与 `term-script` / 界面共用的那张表）。
    fn keys(&self, params: &Value) -> Result<Value, SurfaceError> {
        let (name, id) = self.resolve(params)?;
        let names: Vec<String> = params
            .get("keys")
            .and_then(Value::as_array)
            .ok_or_else(|| SurfaceError::invalid_params("缺少必填参数 keys（字符串数组）"))?
            .iter()
            .map(|item| {
                item.as_str().map(str::to_string).ok_or_else(|| {
                    SurfaceError::invalid_params(format!("keys 里每一项都必须是字符串（实际：{item}）"))
                })
            })
            .collect::<Result<_, _>>()?;
        if names.is_empty() {
            return Err(SurfaceError::invalid_params("参数 keys 至少要有 1 项"));
        }
        let app_cursor = if names.iter().any(|name| term_core::keys::is_arrow(name)) {
            self.pool
                .screen(id)
                .is_some_and(|snapshot| snapshot.application_cursor_keys)
        } else {
            false
        };
        let mut bytes = Vec::new();
        for name in &names {
            let Some(text) = term_core::keys::key_bytes(name, app_cursor) else {
                return Err(SurfaceError::invalid_params(format!(
                    "未知键名 {name:?}；可用 {}",
                    term_core::keys::known_names()
                )));
            };
            bytes.extend_from_slice(text.as_bytes());
        }
        if !self.pool.send(id, &bytes) {
            return Err(SurfaceError::not_found(format!(
                "会话 {name:?} 不存在，没有可写入的会话"
            )));
        }
        Ok(json!({ "written": true, "bytes": bytes.len() }))
    }

    fn wait(&self, params: &Value) -> Result<Value, SurfaceError> {
        let (_, id) = self.resolve(params)?;
        let pattern = required_string(params, "pattern")?;
        if pattern.is_empty() {
            // 空模式会立刻命中任何东西——那是调用方的 bug，直接拒绝而不是
            // 让它"成功"返回一段随机文本。
            return Err(SurfaceError::invalid_params(
                "参数 pattern 不能是空串：空模式会立刻命中任何输出",
            ));
        }
        let timeout_ms = optional_u64(params, "timeout_ms")?.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);

        // 先登记、后匹配（§B.13）：wait_register 返回即已生效，
        // 之后的输出保证被匹配；登记点之前的输出不入账。
        let registration = self.pool.wait_register(id, &pattern)?;
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

    fn screen(&self, params: &Value) -> Result<Value, SurfaceError> {
        let (name, id) = self.resolve(params)?;
        match self.pool.screen(id) {
            Some(snapshot) => Ok(json!({
                "text": snapshot.screen_text(),
                "cols": snapshot.cols,
                "rows": snapshot.rows,
            })),
            // `screen()` 返回 None 有两种含义：会话不在册，或会话已结束到没有
            // 事实可取。查一次 state 把两种分开——回一个空屏会让调用方以为
            // "屏幕就是空的"。
            None => match self.pool.state(id) {
                None => Err(SurfaceError::not_found(format!("会话 {name:?} 不存在"))),
                Some(SessionState::Finished(_)) => Err(SurfaceError::closed(format!(
                    "会话 {name:?} 已结束，没有屏幕事实可取"
                ))),
                Some(SessionState::Alive) => Err(SurfaceError::internal(format!(
                    "会话 {name:?} 存活却取不到屏幕快照——桥接侧的故障"
                ))),
            },
        }
    }

    fn state(&self, params: &Value) -> Result<Value, SurfaceError> {
        let (name, id) = self.resolve(params)?;
        match self.pool.state(id) {
            None => Err(SurfaceError::not_found(format!("会话 {name:?} 不存在"))),
            Some(SessionState::Alive) => Ok(json!({ "alive": true, "exit": Value::Null })),
            Some(SessionState::Finished(exit)) => Ok(json!({ "alive": false, "exit": exit })),
        }
    }

    fn facets(&self, params: &Value) -> Result<Value, SurfaceError> {
        let (name, id) = self.resolve(params)?;
        match self.pool.facets(id) {
            None => Err(SurfaceError::not_found(format!("会话 {name:?} 不存在"))),
            Some(kinds) => {
                let names: Vec<&'static str> = kinds.into_iter().map(facet_name).collect();
                Ok(json!({ "facets": names }))
            }
        }
    }
}

impl Surface for PoolSurface {
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
            other => Err(SurfaceError::method_not_found(format!(
                "未知方法 {other}；可用方法见 describe"
            ))),
        }
    }
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

/// `OpenError` → 线上失败。四类分开：配置不足是调用方的问题（可修），
/// 起不来是环境问题（也可能是命令打错），没装工厂是「客户端漏了一步」，
/// 工厂建通道失败按它自己的分类走——都不是 internal。
fn open_error(err: OpenError) -> SurfaceError {
    match err {
        OpenError::Config(detail) => SurfaceError::invalid_params(detail.to_string()),
        OpenError::Spawn(detail) => SurfaceError::invalid_request(format!("起会话失败：{detail}")),
        OpenError::BuiltinChannelNeedsInjection => SurfaceError::invalid_request(err.to_string()),
        // W1-c：分类仍要送到调用方，别拍平成一句「起不来」。
        OpenError::Channel(term_session::ChannelFailure::NeedsVault { session }) => {
            SurfaceError::locked(format!("会话 {session} 需要解锁保险库"))
        }
        OpenError::Channel(term_session::ChannelFailure::Unsupported(why)) => {
            SurfaceError::invalid_request(why)
        }
        OpenError::Channel(term_session::ChannelFailure::Connect(why)) => {
            SurfaceError::internal(format!("通道连接失败：{why}"))
        }
    }
}
