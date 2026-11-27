//! Rhai 脚本引擎：沙箱装配 + 原语注册 + 执行入口。
//!
//! API 形态：**全局函数注入**（`write("x")` / `wait("y", 5)` / …），闭包在
//! `run` 时捕获 [`ScriptWorkspace`]。与 ZOC 的 `ZocSend` 等全局函数同手感；
//! 比自定义类型少一层 `session.` 前缀，脚本即任务验收示例的形状。
//!
//! 捕获的是**工作区**而不是单条会话：单会话原语经
//! [`ScriptWorkspace`] 的门面转给「当前绑定」，于是 `session_use` 改绑之后
//! 同一批闭包自动作用到新会话上（`register_primitives` 只为每个动词注册一次）。
//!
//! `run` 在调用线程上同步执行（设计上跑在独立脚本线程）；引擎每次执行
//! 现搭——函数注册是微秒级开销，换来零共享状态、`ScriptEngine` 天然
//! Send + Sync（配置是纯数据）。

use std::sync::{Arc, Mutex};
use std::time::Instant;

use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, Position, Scope};

use term_core::app_state::{AppSnapshot, PanelState};
use term_core::session::{AuthMethod, OpenRequest, Protocol, SessionConfig};
use term_core::session_runtime::{LogPhase, LogStatus, Session, SessionState};

use crate::app_ui::{AppDelta, LayoutOp};
use crate::error::{PrimitiveError, ScriptError};
use crate::filter::GrepHandle;
use crate::human::{Answer, Ask, MessageButton, MessageIcon, PickFile};
use crate::localfs::{DirEntry, FileStat};
use crate::sessions::SessionsError;
use crate::proc::{ProcOutput, ProcRun};
use crate::sessions::SessionInfo;
use crate::sandbox::SandboxConfig;
use crate::session::WaitAnyHit;
use crate::workspace::{ScriptWorkspace, SessionError};

/// 脚本的 `session_open` 不带尺寸时的缺省值——与能力面 `session_open` 的
/// `params_schema`（cols/rows `minimum: 1`、说明里写「缺省 80 列 / 24 行」）
/// 是同一个数：脚本与远程各写一套默认值迟早对不上。
const DEFAULT_OPEN_COLS: usize = 80;
const DEFAULT_OPEN_ROWS: usize = 24;

/// 省掉超时参数时用的秒数（30s，与能力面 `timeout_ms` 的缺省 30000 同一个数）。
const DEFAULT_WAIT_SECS: f64 = 30.0;

/// 脚本引擎。持有沙箱配置；`&self` 可跨线程共享。
pub struct ScriptEngine {
    sandbox: SandboxConfig,
}

impl ScriptEngine {
    pub fn new(sandbox: SandboxConfig) -> Self {
        Self { sandbox }
    }

    /// 执行一段脚本，原语以全局函数注入：`write(text)`、
    /// `wait(pattern, timeout_secs)`、`wait_any([patterns], timeout_secs[, filter])`、
    /// `read_until(delimiter, timeout_secs[, filter])`、
    /// `read([timeout_secs[, filter]])`、`screen()`；过滤器构造
    /// `grep(pat)` / `grep_v(pat)` 返回 [`GrepHandle`] 句柄；`wait_any` 返回
    /// 一张含 `index`（1 起）与 `text` 的 Map。
    ///
    /// 返回值 = 脚本最后一个表达式的字符串形式（无表达式 → 空串）。
    /// 失败一律显式报 [`ScriptError`]：语法错 → `Syntax`；原语超时、沙箱
    /// 超限、执行超时、运行时异常 → `Runtime`。
    pub fn run(
        &self,
        script: &str,
        session: Arc<Mutex<dyn Session>>,
    ) -> Result<String, ScriptError> {
        let workspace = ScriptWorkspace::single(session, self.sandbox.execution_timeout);
        self.run_in_workspace(script, Arc::new(workspace))
    }

    /// 在给定工作区里执行（正式入口：多会话脚本走这条）。
    ///
    /// 与 [`run`](Self::run) 的区别只有一处：**谁能被看见**。`run` 把调用方
    /// 递来的那条会话包成单会话工作区（`session_open` / `session_close` 会报
    /// 「这组会话不支持」），这里的 [`ScriptWorkspace`] 带着调用方给的
    /// [`crate::Sessions`]，于是 `session_list` / `session_open` / `session_use`
    /// 才有意义。
    ///
    /// 工作区由调用方以 `Arc` 递进来：脚本跑完后调用方可能还想问「最后绑在
    /// 哪条会话上」（`workspace.current_name()`），而那需要共享同一份状态。
    pub fn run_in_workspace(
        &self,
        script: &str,
        workspace: Arc<ScriptWorkspace>,
    ) -> Result<String, ScriptError> {
        let start = Instant::now();
        let budget = self.sandbox.execution_timeout;
        let mut engine = Engine::new();
        engine.set_max_operations(self.sandbox.max_operations);
        engine.set_max_expr_depths(self.sandbox.max_expr_depth, self.sandbox.max_global_depth);
        // 墙钟超时：进度回调在语句之间查钟，到期即中断执行。
        // （中断不了已阻塞的 wait——单次等待上限由 with_max_wait 钳住。）
        engine.on_progress(move |_ops| {
            if start.elapsed() >= budget {
                Some(Dynamic::UNIT)
            } else {
                None
            }
        });
        // 工作区是 Arc 共享的句柄，克隆一次只是加一次引用计数。
        Self::register_primitives(&mut engine, workspace.as_ref().clone());
        // CRT 兼容门面（`crt.Screen` / `crt.Session` …）单独注册：它属于
        // 迁移层，不是能力面的一部分，因此不参与
        // `registered_primitives_match_the_script_projection_of_the_surface_table`
        // 的对账，也不出现在 ADR-0019 的 schema 里。
        crate::compat::register(&mut engine, workspace.as_ref().clone());

        let ast = engine.compile(script).map_err(|e| {
            let error = ScriptError::Syntax(e.to_string(), Self::position_of(e.position()));
            // `last_error()` / `crt.GetLastErrorMessage()` 必须能看见失败原因；
            // 两处失败都在这里落账，别让调用方只拿到一个 Err 却读不到文本。
            workspace.env().context.record_error(error.to_string());
            error
        })?;
        let mut scope = Scope::new();
        let value = engine
            .eval_ast_with_scope::<Dynamic>(&mut scope, &ast)
            .map_err(|e| {
                let error = Self::to_script_error(*e);
                workspace.env().context.record_error(error.to_string());
                error
            })?;
        // Display：String 原样、数字转十进制、unit → 空串（调用方最常见的
        // 是脚本末尾放 screen()）。
        Ok(if value.is_unit() {
            String::new()
        } else {
            value.to_string()
        })
    }

    /// 把原语注册成全局函数（闭包捕获工作区）。超时参数收 f64 与 i64
    /// 两种重载（脚本写 `wait("x", 10)` 不该撞「Function not found」）；
    /// `read_until` / `read` / `wait_any` 另有收 [`GrepHandle`] 过滤器的
    /// 重载（Rhai 按参数个数 + 类型分派）。返回 `Result` 的闭包由
    /// `register_fn` 直接收（rhai 1.21+）。
    pub(crate) fn register_primitives(engine: &mut Engine, ws: ScriptWorkspace) {
        // 过滤器句柄类型：grep(pat) / grep_v(pat) 的返回值，可传给
        // read_until / read 的过滤重载。无效正则在构造时显式报错。
        engine.register_type_with_name::<GrepHandle>("GrepFilter");
        engine.register_fn("grep", |pattern: &str| {
            GrepHandle::grep(pattern).map_err(string_to_rhai)
        });
        engine.register_fn("grep_v", |pattern: &str| {
            GrepHandle::grep_v(pattern).map_err(string_to_rhai)
        });
        {
            let s = ws.clone();
            engine.register_fn("write", move |text: &str| {
                s.write(text).map_err(primitive_to_rhai)
            });
        }
        {
            // keys：按**键名**发按键。两种重载——单个键名与键名数组；返回写入的
            // 字节数（`keys("f5")` 是 5 个字节，脚本据此能自己核对）。
            let s = ws.clone();
            engine.register_fn("keys", move |name: &str| {
                s.keys(&[name]).map(|n| n as i64).map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("keys", move |names: Array| {
                let names = array_to_strings(names, "keys")?;
                let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
                s.keys(&borrowed).map(|n| n as i64).map_err(primitive_to_rhai)
            });
        }
        {
            // wait_idle：读到输出停下来。这是「等安静」而不是「等某个事实」，
            // 与 wait / wait_any 并列使用（提示语不确定、只知道命令会结束）。
            let s = ws.clone();
            engine.register_fn("wait_idle", move |quiet_secs: f64| {
                s.wait_idle(quiet_secs, DEFAULT_WAIT_SECS)
                    .map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("wait_idle", move |quiet_secs: f64, timeout_secs: f64| {
                s.wait_idle(quiet_secs, timeout_secs)
                    .map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("wait_idle", move |quiet_ms: i64, timeout_secs: i64| {
                s.wait_idle(quiet_ms as f64 / 1000.0, timeout_secs as f64)
                    .map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("wait_idle", move |quiet_ms: i64| {
                s.wait_idle(quiet_ms as f64 / 1000.0, DEFAULT_WAIT_SECS)
                    .map_err(primitive_to_rhai)
            });
        }
        {
            // 同步写开关：开启后每次 write / keys 都等输出安静再返回。
            let s = ws.clone();
            engine.register_fn("set_synchronous", move |on: bool| s.set_synchronous(on));
        }
        {
            // 面向人的两条。notify 不返回东西（提示不是可确认的通知）；
            // ask 拿不到输入时显式报错，不返回空串冒充「用户按了回车」。
            let s = ws.clone();
            engine.register_fn("notify", move |message: &str| {
                s.notify(message).map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("ask", move |prompt: &str| {
                s.ask(prompt).map_err(primitive_to_rhai)
            });
            // `ask(#{kind: …})` 重载：与 [`Ask`] 的六种形态一一对应。Rhai 按
            // 参数类型分派，因此与上面那个收字符串的 `ask` 共存。能力面里
            // `ask` 的 schema 只描述简单形态（收字符串、答一行），这里是
            // 脚本侧多出来的一层——`alert`（单向通报）只在 `message` 的
            // 「只有一个 OK」之外才需要单独成形，故不另立动词。
            let s = ws.clone();
            engine.register_fn("ask", move |spec: Map| -> Result<Dynamic, Box<EvalAltResult>> {
                let request = ask_from_map(&spec)?;
                let answer = s.ask_request(&request).map_err(primitive_to_rhai)?;
                Ok(answer_to_dynamic(answer))
            });
        }
        {
            let s = ws.clone();
            engine.register_fn("wait", move |pattern: &str, timeout_secs: f64| {
                s.wait(pattern, timeout_secs).map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("wait", move |pattern: &str, timeout_secs: i64| {
                s.wait(pattern, timeout_secs as f64)
                    .map_err(primitive_to_rhai)
            });
        }
        {
            // read_until 两参（无过滤）+ 三参（行级过滤，作用于匹配之前）。
            let s = ws.clone();
            engine.register_fn("read_until", move |delimiter: &str, timeout_secs: f64| {
                s.read_until(delimiter, timeout_secs, None)
                    .map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("read_until", move |delimiter: &str, timeout_secs: i64| {
                s.read_until(delimiter, timeout_secs as f64, None)
                    .map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn(
                "read_until",
                move |delimiter: &str, timeout_secs: f64, filter: GrepHandle| {
                    s.read_until(delimiter, timeout_secs, Some(filter.0.as_ref()))
                        .map_err(primitive_to_rhai)
                },
            );
            let s = ws.clone();
            engine.register_fn(
                "read_until",
                move |delimiter: &str, timeout_secs: i64, filter: GrepHandle| {
                    s.read_until(delimiter, timeout_secs as f64, Some(filter.0.as_ref()))
                        .map_err(primitive_to_rhai)
                },
            );
        }
        {
            // read 零参（纯排水、不等待）/ 一参（等新数据至多 timeout）/
            // 两参（加行级过滤）。超时无数据返回空串，不报错。
            let s = ws.clone();
            engine.register_fn("read", move || {
                s.read(0.0, None).map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("read", move |timeout_secs: f64| {
                s.read(timeout_secs, None).map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("read", move |timeout_secs: i64| {
                s.read(timeout_secs as f64, None).map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn(
                "read",
                move |timeout_secs: f64, filter: GrepHandle| {
                    s.read(timeout_secs, Some(filter.0.as_ref()))
                        .map_err(primitive_to_rhai)
                },
            );
            let s = ws.clone();
            engine.register_fn(
                "read",
                move |timeout_secs: i64, filter: GrepHandle| {
                    s.read(timeout_secs as f64, Some(filter.0.as_ref()))
                        .map_err(primitive_to_rhai)
                },
            );
        }
        {
            // wait_any 四重载：(Array, f64/i64) 与带 GrepHandle 过滤器的
            // (Array, f64/i64, GrepHandle)。过滤先于匹配；命中返回 Map。
            let s = ws.clone();
            engine.register_fn("wait_any", move |patterns: Array, timeout_secs: f64| {
                let patterns = array_to_patterns(patterns)?;
                s.wait_any(&patterns, timeout_secs, None)
                    .map(wait_any_hit_to_map)
                    .map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("wait_any", move |patterns: Array, timeout_secs: i64| {
                let patterns = array_to_patterns(patterns)?;
                s.wait_any(&patterns, timeout_secs as f64, None)
                    .map(wait_any_hit_to_map)
                    .map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn(
                "wait_any",
                move |patterns: Array, timeout_secs: f64, filter: GrepHandle| {
                    let patterns = array_to_patterns(patterns)?;
                    s.wait_any(&patterns, timeout_secs, Some(filter.0.as_ref()))
                        .map(wait_any_hit_to_map)
                        .map_err(primitive_to_rhai)
                },
            );
            let s = ws.clone();
            engine.register_fn(
                "wait_any",
                move |patterns: Array, timeout_secs: i64, filter: GrepHandle| {
                    let patterns = array_to_patterns(patterns)?;
                    s.wait_any(&patterns, timeout_secs as f64, Some(filter.0.as_ref()))
                        .map(wait_any_hit_to_map)
                        .map_err(primitive_to_rhai)
                },
            );
        }
        {
            let s = ws.clone();
            engine.register_fn("screen", move || {
                s.screen().map_err(primitive_to_rhai)
            });
        }
        {
            let s = ws.clone();
            engine.register_fn("cursor_pos", move || {
                s.cursor_pos()
                    .map(|(row, col)| {
                        let mut map = Map::new();
                        map.insert("row".into(), Dynamic::from(row as i64));
                        map.insert("col".into(), Dynamic::from(col as i64));
                        map
                    })
                    .map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("terminal_size", move || {
                s.terminal_size()
                    .map(|(cols, rows)| {
                        let mut map = Map::new();
                        map.insert("cols".into(), Dynamic::from(cols as i64));
                        map.insert("rows".into(), Dynamic::from(rows as i64));
                        map
                    })
                    .map_err(primitive_to_rhai)
            });
        }
        {
            // 日志三件套：目录由脚本显式给（term-script 不读配置目录——
            // 运行时依赖只有 term-core，不引 term-paths）。
            // 会话名在这是占位 "script"：文件名里的标识由会话侧拼，
            // 脚本没有「会话名」这个概念（它只持一个 Session 句柄）。
            let s = ws.clone();
            engine.register_fn("log_start", move |dir: &str| {
                s.log_start(dir).map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("log_stop", move || {
                s.log_stop().map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("state", move || -> Result<Map, Box<EvalAltResult>> {
                let mut map = Map::new();
                // 工作区的 `state()` 会先解析当前绑定（可能一次都没有），
                // 所以这里必须处理 `Result`，不能像单会话那样直接匹配。
                match s.state().map_err(primitive_to_rhai)? {
                    SessionState::Alive => {
                        map.insert("alive".into(), Dynamic::from(true));
                        map.insert("exit".into(), Dynamic::UNIT);
                    }
                    SessionState::Finished(exit) => {
                        map.insert("alive".into(), Dynamic::from(false));
                        map.insert("exit".into(), Dynamic::from(exit));
                    }
                }
                Ok(map)
            });
            let s = ws.clone();
            engine.register_fn("facets", move || -> Result<Dynamic, Box<EvalAltResult>> {
                let names: Array = s
                    .facets()
                    .map_err(primitive_to_rhai)?
                    .into_iter()
                    .map(Dynamic::from)
                    .collect();
                Ok(Dynamic::from(names))
            });
            let s = ws.clone();
            engine.register_fn("log_status", move || {
                s.log_status().map(|status| log_status_to_map(&status)).map_err(primitive_to_rhai)
            });
        }
        // ── 全局层：选绑定 / 增删会话 ────────────────────────────
        //
        // 一律按**会话名**寻址（ADR-0024）：`SessionId` 是进程内自增，存下来的
        // 脚本明天再跑就失效了；名字跨进程稳定，且池内唯一。
        {
            let s = ws.clone();
            engine.register_fn("session_current", move || {
                s.current_name().map_err(primitive_to_rhai)
            });

            let s = ws.clone();
            engine.register_fn("session_list", move || {
                let rows: Array = s.session_list().iter().map(session_row_to_map).collect();
                let mut map = Map::new();
                map.insert("sessions".into(), Dynamic::from(rows));
                map
            });

            // session_open 四种重载：命令 / 命令+尺寸 / 无参（本地 shell）/ 尺寸。
            // 与能力面 session_open 的三个可选参数（command / cols / rows）对应，
            // 缺省尺寸取 80×24（与 RPC 侧缺省一致）。
            //
            // 没有「本地 shell + 自定义名字」的重载：名字由宿主按会话配置定，
            // 脚本指定一个宿主不认的名字只会让它自己的 session_use 打空。
            // 重名时宿主会加后缀，返回值里带的是**真名**。
            let s = ws.clone();
            engine.register_fn("session_open", move |command: &str| {
                s.session_open(Some(command), DEFAULT_OPEN_COLS, DEFAULT_OPEN_ROWS)
                    .map(|info| session_row_to_map(&info))
                    .map_err(session_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("session_open", move |command: &str, cols: i64, rows: i64| {
                let (cols, rows) = checked_size(cols, rows)?;
                s.session_open(Some(command), cols, rows)
                    .map(|info| session_row_to_map(&info))
                    .map_err(session_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("session_open", move || {
                s.session_open(None, DEFAULT_OPEN_COLS, DEFAULT_OPEN_ROWS)
                    .map(|info| session_row_to_map(&info))
                    .map_err(session_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("session_open", move |cols: i64, rows: i64| {
                let (cols, rows) = checked_size(cols, rows)?;
                s.session_open(None, cols, rows)
                    .map(|info| session_row_to_map(&info))
                    .map_err(session_to_rhai)
            });

            let s = ws.clone();
            engine.register_fn("session_use", move |name: &str| {
                s.session_use(name)
                    .map(|info| session_row_to_map(&info))
                    .map_err(session_to_rhai)
            });

            let s = ws.clone();
            engine.register_fn("session_close", move |name: &str| {
                s.session_close(name)
                    .map_err(session_to_rhai)
                    // `session_close` 的远程结果是 `{"closed": true}`；脚本这边
                    // 返回同形状，让「一个动词两个前端一种形状」成立。
                    .map(|()| {
                        let mut map = Map::new();
                        map.insert("closed".into(), Dynamic::from(true));
                        map
                    })
            });
            // ── 上下文动词（ADR-0027） ────────────────────────────
            let s = ws.clone();
            engine.register_fn("version", move || -> Dynamic {
                Dynamic::from(s.env().context.version.clone())
            });
            let s = ws.clone();
            engine.register_fn("script_path", move || -> Dynamic {
                Dynamic::from(s.env().context.script_path.clone().unwrap_or_default())
            });
            let s = ws.clone();
            engine.register_fn("args", move || -> Dynamic {
                let args: Array = s
                    .env()
                    .context
                    .args
                    .iter()
                    .map(|a| Dynamic::from(a.clone()))
                    .collect();
                Dynamic::from(args)
            });
            let s = ws.clone();
            engine.register_fn("args", move |index: i64| -> Result<Dynamic, Box<EvalAltResult>> {
                if index < 0 {
                    return Err(string_to_rhai("args 的序号不能为负（0 起）".to_string()));
                }
                Ok(Dynamic::from(
                    s.env()
                        .context
                        .args
                        .get(index as usize)
                        .cloned()
                        .unwrap_or_default(),
                ))
            });
            let s = ws.clone();
            engine.register_fn("last_error", move || -> Dynamic {
                Dynamic::from(s.env().context.last_error())
            });
            let s = ws.clone();
            engine.register_fn("script_session", move || -> Result<Dynamic, Box<EvalAltResult>> {
                s.script_session_name().map(Dynamic::from).map_err(primitive_to_rhai)
            });

            // ── 会话面扩展 ───────────────────────────────────
            let s = ws.clone();
            engine.register_fn("session_at", move |index: i64| -> Result<Dynamic, Box<EvalAltResult>> {
                if index < 1 {
                    return Err(session_to_rhai(SessionError::Primitive(
                        PrimitiveError::BadArgument("session_at 的序号从 1 起"),
                    )));
                }
                s.session_at(index as usize)
                    .map(|info| session_row_to_map(&info))
                    .map_err(session_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("session_index", move || -> Result<Dynamic, Box<EvalAltResult>> {
                s.session_index()
                    .map(|i| count_map(i as i64))
                    .map_err(session_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("session_active", move || -> Result<Dynamic, Box<EvalAltResult>> {
                s.session_active()
                    .map(|info| session_row_to_map(&info))
                    .map_err(session_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("session_info", move || -> Result<Dynamic, Box<EvalAltResult>> {
                s.session_info(None)
                    .map(|info| session_row_to_map(&info))
                    .map_err(session_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("session_info", move |name: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                s.session_info(Some(name))
                    .map(|info| session_row_to_map(&info))
                    .map_err(session_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("session_connect", move |name: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                s.session_connect(name)
                    .map(|info| session_row_to_map(&info))
                    .map_err(session_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("session_clone", move |name: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                s.session_clone(name)
                    .map(|info| session_row_to_map(&info))
                    .map_err(session_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("session_activate", move |name: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                s.session_activate(name).map(|()| ok_map()).map_err(session_to_rhai)
            });
            // `session_open(#{profile|config|command|local})`：ADR-0027 的脚本形态。
            let s = ws.clone();
            engine.register_fn("session_open", move |spec: Map| -> Result<Dynamic, Box<EvalAltResult>> {
                let request = open_request_from_map(&spec, DEFAULT_OPEN_COLS, DEFAULT_OPEN_ROWS)?;
                s.session_open_request(request)
                    .map(|info| session_row_to_map(&info))
                    .map_err(session_to_rhai)
            });

            // ── 睡眠 / 按键 / 区域 ────────────────────────────────
            let s = ws.clone();
            engine.register_fn("sleep", move |millis: i64| -> Result<Dynamic, Box<EvalAltResult>> {
                if millis < 0 {
                    return Err(primitive_to_rhai(PrimitiveError::BadArgument(
                        "sleep 的毫秒数不能为负",
                    )));
                }
                s.sleep(millis as u64).map(|()| ok_map()).map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("sleep", move |secs: f64| -> Result<Dynamic, Box<EvalAltResult>> {
                if secs < 0.0 {
                    return Err(primitive_to_rhai(PrimitiveError::BadArgument(
                        "sleep 的秒数不能为负",
                    )));
                }
                s.sleep((secs * 1000.0) as u64)
                    .map(|()| ok_map())
                    .map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("send_keys", move |expr: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                s.send_keys(expr).map(|n| count_map(n as i64)).map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn(
                "region",
                move |r1: i64, c1: i64, r2: i64, c2: i64| -> Result<Dynamic, Box<EvalAltResult>> {
                    let (r1, c1, r2, c2) = checked_rect(r1, c1, r2, c2)?;
                    s.region(r1, c1, r2, c2).map(Dynamic::from).map_err(primitive_to_rhai)
                },
            );
            let s = ws.clone();
            engine.register_fn(
                "region_lines",
                move |r1: i64, c1: i64, r2: i64, c2: i64| -> Result<Dynamic, Box<EvalAltResult>> {
                    let (r1, c1, r2, c2) = checked_rect(r1, c1, r2, c2)?;
                    s.region_lines(r1, c1, r2, c2)
                        .map(Dynamic::from)
                        .map_err(primitive_to_rhai)
                },
            );

            // ── 人机 ────────────────────────────────────────
            let s = ws.clone();
            engine.register_fn("confirm", move |text: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                let answer = s
                    .ask_request(&Ask::Confirm {
                        title: String::new(),
                        text: text.to_string(),
                    })
                    .map_err(primitive_to_rhai)?;
                match answer {
                    Answer::Bool(value) => Ok(flag_map(value)),
                    other => Err(string_to_rhai(format!(
                        "宿主对 confirm 回了 {other:?}（实现与询问形态不一致）"
                    ))),
                }
            });
            let s = ws.clone();
            engine.register_fn("message", move |spec: Map| -> Result<Dynamic, Box<EvalAltResult>> {
                let ask = message_from_map(&spec)?;
                let answer = s.ask_request(&ask).map_err(primitive_to_rhai)?;
                match answer {
                    Answer::Text(text) => Ok(Dynamic::from(text)),
                    other => Err(string_to_rhai(format!("宿主对 message 回了 {other:?}"))),
                }
            });
            let s = ws.clone();
            engine.register_fn(
                "text_input",
                move |prompt: &str, default: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                    let answer = s
                        .ask_request(&Ask::Text {
                            title: String::new(),
                            prompt: prompt.to_string(),
                            default: default.to_string(),
                        })
                        .map_err(primitive_to_rhai)?;
                    match answer {
                        Answer::Text(text) => Ok(Dynamic::from(text)),
                        other => Err(string_to_rhai(format!("宿主对 text_input 回了 {other:?}"))),
                    }
                },
            );
            let s = ws.clone();
            engine.register_fn("secret", move |prompt: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                let answer = s
                    .ask_request(&Ask::Secret {
                        title: String::new(),
                        prompt: prompt.to_string(),
                    })
                    .map_err(primitive_to_rhai)?;
                match answer {
                    Answer::Text(text) => Ok(Dynamic::from(text)),
                    other => Err(string_to_rhai(format!("宿主对 secret 回了 {other:?}"))),
                }
            });
            let s = ws.clone();
            engine.register_fn(
                "choose",
                move |prompt: &str, options: Array| -> Result<Dynamic, Box<EvalAltResult>> {
                    let list = array_to_strings(options, "choose")?;
                    if list.is_empty() {
                        return Err(primitive_to_rhai(PrimitiveError::BadArgument(
                            "choose 至少要给一个选项",
                        )));
                    }
                    let answer = s
                        .ask_request(&Ask::Choose {
                            title: String::new(),
                            prompt: prompt.to_string(),
                            options: list,
                        })
                        .map_err(primitive_to_rhai)?;
                    match answer {
                        Answer::Choice(index) => Ok(count_map(index as i64)),
                        other => Err(string_to_rhai(format!("宿主对 choose 回了 {other:?}"))),
                    }
                },
            );
            let s = ws.clone();
            engine.register_fn("pick_file", move |spec: Map| -> Result<Dynamic, Box<EvalAltResult>> {
                let request = pick_file_from_map(&spec)?;
                let picked = s.pick_file(&request).map_err(primitive_to_rhai)?;
                Ok(Dynamic::from(
                    picked.map(|p| p.display().to_string()).unwrap_or_default(),
                ))
            });

            // ── 剪贴板 / 选区 / 窗口 ────────────────────────
            let s = ws.clone();
            engine.register_fn("clipboard_get", move || -> Result<Dynamic, Box<EvalAltResult>> {
                s.clipboard_get().map(Dynamic::from).map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("clipboard_set", move |text: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                s.clipboard_set(text).map(|()| ok_map()).map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("selection", move || -> Result<Dynamic, Box<EvalAltResult>> {
                s.selection().map(Dynamic::from).map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("app_quit", move |save: bool| -> Result<Dynamic, Box<EvalAltResult>> {
                s.app_quit(save).map(|()| ok_map()).map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("window", move |visible: bool| -> Result<Dynamic, Box<EvalAltResult>> {
                s.set_window_visible(visible)
                    .map(|()| ok_map())
                    .map_err(primitive_to_rhai)
            });

            // ── 应用面（ADR-0027） ─────────────────────
            //
            // 读也走往返（`script_app.rs` 的队列），所以这两个闭包拿到的是
            // **界面线程刚算出来的**状态，不是某个共享槽里可能晚一帧的副本。
            let s = ws.clone();
            engine.register_fn("app_state", move || -> Result<Dynamic, Box<EvalAltResult>> {
                let snapshot = s.app_state().map_err(primitive_to_rhai)?;
                Ok(app_snapshot_map(snapshot))
            });
            let s = ws.clone();
            engine.register_fn(
                "app_panel_set",
                move |panel: &str, visible: bool| -> Result<Dynamic, Box<EvalAltResult>> {
                    let delta = s.app_panel_set(panel, visible).map_err(primitive_to_rhai)?;
                    Ok(app_delta_map(delta))
                },
            );
            let s = ws.clone();
            engine.register_fn(
                "app_layout",
                move |spec: Map| -> Result<Dynamic, Box<EvalAltResult>> {
                    let op = layout_op_from_map(&spec)?;
                    let delta = s.app_layout(&op).map_err(primitive_to_rhai)?;
                    Ok(app_delta_map(delta))
                },
            );

            // ── 本地文件 / 进程 ─────────────────────────────
            let s = ws.clone();
            engine.register_fn("fs_read", move |path: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                s.fs_read(path)
                    .map(|bytes| Dynamic::from(String::from_utf8_lossy(&bytes).into_owned()))
                    .map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn(
                "fs_write",
                move |path: &str, data: &str, append: bool| -> Result<Dynamic, Box<EvalAltResult>> {
                    s.fs_write(path, data.as_bytes(), append)
                        .map(|n| count_map(n as i64))
                        .map_err(primitive_to_rhai)
                },
            );
            let s = ws.clone();
            engine.register_fn("fs_write", move |path: &str, data: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                s.fs_write(path, data.as_bytes(), false)
                    .map(|n| count_map(n as i64))
                    .map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("fs_append", move |path: &str, data: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                s.fs_write(path, data.as_bytes(), true)
                    .map(|n| count_map(n as i64))
                    .map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("fs_list", move |path: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                let entries = s.fs_list(path).map_err(primitive_to_rhai)?;
                Ok(Dynamic::from(dir_entries_map(entries)))
            });
            let s = ws.clone();
            engine.register_fn("fs_stat", move |path: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                let stat = s.fs_stat(path).map_err(primitive_to_rhai)?;
                Ok(file_stat_map(stat))
            });
            let s = ws.clone();
            engine.register_fn("fs_mkdir", move |path: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                s.fs_mkdir(path).map(|()| ok_map()).map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("fs_remove", move |path: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                s.fs_remove(path).map(|()| ok_map()).map_err(primitive_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("proc_run", move |program: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                let output = s.proc_run(&ProcRun::new(program)).map_err(primitive_to_rhai)?;
                Ok(proc_output_map(output))
            });
            let s = ws.clone();
            engine.register_fn(
                "proc_run",
                move |program: &str, args: Array| -> Result<Dynamic, Box<EvalAltResult>> {
                    let mut request = ProcRun::new(program);
                    request.args = array_to_strings(args, "proc_run")?;
                    let output = s.proc_run(&request).map_err(primitive_to_rhai)?;
                    Ok(proc_output_map(output))
                },
            );

            // ── 远端文件 ────────────────────────────────────
            let s = ws.clone();
            engine.register_fn("sftp_list", move |path: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                let entries = s.sftp_list(None, path).map_err(session_to_rhai)?;
                Ok(Dynamic::from(dir_entries_map(entries)))
            });
            let s = ws.clone();
            engine.register_fn("sftp_get", move |remote: &str, local: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                s.sftp_get(None, remote, local)
                    .map(|n| count_map(n as i64))
                    .map_err(session_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("sftp_put", move |local: &str, remote: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                s.sftp_put(None, local, remote)
                    .map(|n| count_map(n as i64))
                    .map_err(session_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("sftp_mkdir", move |path: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                s.sftp_mkdir(None, path).map(|()| ok_map()).map_err(session_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("sftp_rename", move |from: &str, to: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                s.sftp_rename(None, from, to)
                    .map(|()| ok_map())
                    .map_err(session_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("sftp_remove", move |path: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                s.sftp_remove(None, path).map(|()| ok_map()).map_err(session_to_rhai)
            });
            let s = ws.clone();
            engine.register_fn("sftp_stat", move |path: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                let stat = s.sftp_stat(None, path).map_err(session_to_rhai)?;
                Ok(file_stat_map(stat))
            });

            // ── 会话库 ──────────────────────────────────────
            let s = ws.clone();
            engine.register_fn("profile_list", move || -> Result<Dynamic, Box<EvalAltResult>> {
                let sessions = s.profile_list().map_err(session_to_rhai)?;
                let rows: Array = sessions.iter().map(profile_row_map).collect();
                let mut map = Map::new();
                map.insert("sessions".into(), Dynamic::from(rows));
                Ok(Dynamic::from(map))
            });
            let s = ws.clone();
            engine.register_fn("profile_get", move |name: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                let config = s.profile_get(name).map_err(session_to_rhai)?;
                Ok(profile_config_map(&config))
            });
            let s = ws.clone();
            engine.register_fn("profile_set", move |name: &str, spec: Map| -> Result<Dynamic, Box<EvalAltResult>> {
                // 只有「库里没有这条」才新建。读库失败（权限 / 冲突 / 宿主不支持）
                // 必须原样报出来——否则脚本以为在改配置，其实往一条全新的空配置里写。
                let mut config = match s.profile_get(name) {
                    Ok(config) => config,
                    Err(SessionError::Sessions(SessionsError::NotFound(_))) => {
                        term_core::session::SessionConfig::new(term_core::session::Protocol::Ssh)
                    }
                    Err(other) => return Err(session_to_rhai(other)),
                };
                config.name = name.to_string();
                apply_profile_map(&mut config, &spec, &[])
                    .map_err(|e| string_to_rhai(e.0))?;
                let total = s.profile_save(config).map_err(session_to_rhai)?;
                Ok(count_map(total as i64))
            });

            // ── 日志 ───────────────────────────────────
            let s = ws.clone();
            engine.register_fn("log_start", move |dir: &str, spec: Map| -> Result<Dynamic, Box<EvalAltResult>> {
                let (name, host, opts) = log_spec_from_map(&spec)?;
                s.log_start_opts(dir, name.as_deref(), host.as_deref(), opts)
                    .map(|()| ok_map())
                    .map_err(primitive_to_rhai)
            });
        }
        // 应用级动词：命令派发 / 自省 / describe。名字与实现都在 surface.rs，
        // 这里只接一根线——注册集合与 term_core::surface 的脚本投影同源。
        crate::surface::register(engine, ws.clone());
    }

    /// Rhai 的 `Position` → 我们的 `(行, 列)`，无位置给 `None`。
    /// 行、列都是 Rhai 原生 1 起坐标。
    fn position_of(pos: Position) -> Option<(usize, usize)> {
        match (pos.line(), pos.position()) {
            (Some(line), Some(column)) => Some((line, column)),
            _ => None,
        }
    }

    /// Rhai 执行错误 → [`ScriptError`]。进度回调中断（= 执行超时）与
    /// 操作数超限给出面向脚本作者的中文文案。位置取自 `e.position()`，
    /// 所以抛在脚本第 N 行的运行时错误也能被编辑器定位到那一行。
    fn to_script_error(e: EvalAltResult) -> ScriptError {
        let pos = Self::position_of(e.position());
        match e {
            EvalAltResult::ErrorTerminated(..) => {
                ScriptError::Runtime("执行超时：超出沙箱墙钟上限".to_string(), pos)
            }
            EvalAltResult::ErrorTooManyOperations(..) => {
                ScriptError::Runtime("超过沙箱操作数上限（死循环？）".to_string(), pos)
            }
            other => ScriptError::Runtime(other.to_string(), pos),
        }
    }
}

/// 原语失败 → Rhai 运行时错误（脚本可 catch；不 catch 就炸出来，显式失败）。
fn primitive_to_rhai(e: PrimitiveError) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        e.to_string().into(),
        Position::NONE,
    ))
}

/// 过滤器构造失败（无效正则）→ Rhai 运行时错误，文案原样给脚本作者看。
fn string_to_rhai(e: String) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(e.into(), Position::NONE))
}

/// Rhai `Array` → 模式列表。**非字符串元素显式报错**（写清第几项与实际
/// 类型），绝不静默 `to_string()` 或跳过——脚本写错类型必须当场可见。
/// 空数组原样透传给 [`ScriptSession::wait_any`] 判 `EmptyPattern`。
fn array_to_patterns(patterns: Array) -> Result<Vec<String>, Box<EvalAltResult>> {
    array_to_strings(patterns, "wait_any")
}

/// Rhai 数组 → `Vec<String>`，非字符串元素报「第几项错了」。
///
/// `verb` 只进错误文案：`wait_any 第 2 项…` 比 `第 2 项…` 更容易定位到
/// 是哪一句原语调用写错了。
fn array_to_strings(values: Array, verb: &str) -> Result<Vec<String>, Box<EvalAltResult>> {
    let mut out = Vec::with_capacity(values.len());
    for (i, value) in values.into_iter().enumerate() {
        let text = value.into_string().map_err(|actual_type| {
            string_to_rhai(format!(
                "{verb} 第 {} 项必须是字符串（实际类型：{actual_type}）",
                i + 1
            ))
        })?;
        out.push(text);
    }
    Ok(out)
}

/// `WaitAnyHit` → Rhai `Map`（脚本里用 `hit.index` / `hit.text`）。
fn wait_any_hit_to_map(hit: WaitAnyHit) -> Map {
    let mut map = Map::new();
    map.insert("index".into(), Dynamic::from(hit.index as i64));
    map.insert("text".into(), Dynamic::from(hit.text));
    map
}

/// `LogStatus` → Rhai `Map`（脚本里用 `.phase` / `.path` / `.reason`）。
///
/// `phase` 用线上一字不差的四个值（与 `term_core::surface` 的
/// `log_status.result_schema` 里的 enum 同一套字面量）——脚本与远程客户端
/// 判同一组字符串，不需要各记一张翻译表。
fn log_status_to_map(status: &LogStatus) -> Map {
    let mut map = Map::new();
    let phase = match &status.phase {
        LogPhase::Off => "off",
        LogPhase::Recording => "recording",
        LogPhase::Paused => "paused",
        LogPhase::Failed(_) => "failed",
    };
    map.insert("phase".into(), Dynamic::from(phase));
    let path = status.path.as_ref().map(|p| p.display().to_string());
    map.insert("path".into(), Dynamic::from(path));
    let reason = match &status.phase {
        LogPhase::Failed(why) => Some(why.clone()),
        _ => None,
    };
    map.insert("reason".into(), Dynamic::from(reason));
    map
}

/// `SessionInfo` → `session_list` 里的一行。
///
/// 键与能力面 `session_list.result_schema` 的 item 一一对应（`name` /
/// `alive` / `exit`）——**不含 id**：id 是进程内自增的绑定句柄，脚本拿它
/// 存盘明天就失效（ADR-0024）。`exit` 存活时给 unit（打印为空）：Rhai 没有
/// `null`，`state()` 早就是这么做的；schema 里写的 `["string","null"]`
/// 对应的正是「存活 → 空 / 已结束 → 状态文本」。
fn session_row_to_map(info: &SessionInfo) -> Dynamic {
    let mut map = Map::new();
    map.insert("name".into(), Dynamic::from(info.name.clone()));
    map.insert("alive".into(), Dynamic::from(info.alive));
    map.insert(
        "exit".into(),
        match &info.exit {
            Some(text) => Dynamic::from(text.clone()),
            None => Dynamic::UNIT,
        },
    );
    map.insert("phase".into(), Dynamic::from(info.phase.clone()));
    map.insert(
        "local_addr".into(),
        match &info.local_addr {
            Some(addr) => Dynamic::from(addr.clone()),
            None => Dynamic::UNIT,
        },
    );
    Dynamic::from(map)
}

/// `{"ok": true}`（写类动词的统一成功形状）。
fn ok_map() -> Dynamic {
    let mut map = Map::new();
    map.insert("ok".into(), Dynamic::from(true));
    Dynamic::from(map)
}

fn count_map(value: i64) -> Dynamic {
    let mut map = Map::new();
    map.insert("value".into(), Dynamic::from(value));
    Dynamic::from(map)
}

fn flag_map(value: bool) -> Dynamic {
    let mut map = Map::new();
    map.insert("value".into(), Dynamic::from(value));
    Dynamic::from(map)
}

/// 一块面板 → Rhai `Map`。`app_state` 的 `panels` 一项与 `app_panel_set` 回执的
/// `panels` 一项共用这一个函数——两处形状本就该一模一样，
/// 各写一份的结果是「写完接着按快照的键读」时才发现的键名漂移。
fn app_panel_map(panel: &PanelState) -> Dynamic {
    let mut row = Map::new();
    row.insert("id".into(), Dynamic::from(panel.id.to_string()));
    row.insert("visible".into(), Dynamic::from(panel.visible));
    row.insert("zone".into(), Dynamic::from(panel.zone.to_string()));
    row.insert("order".into(), Dynamic::from(panel.order as i64));
    row.insert("floating".into(), Dynamic::from(panel.floating));
    row.insert(
        "rect".into(),
        match panel.rect {
            // 落盘形态就是 `[x, y, w, h]`（整数逻辑点），照原样给：客户端
            // 再记一套 `{x, y, w, h}` 只会多一条要对齐的映射。
            Some([x, y, w, h]) => {
                let mut cells = Array::new();
                for value in [x, y, w, h] {
                    cells.push(Dynamic::from(value as i64));
                }
                Dynamic::from(cells)
            }
            None => Dynamic::UNIT,
        },
    );
    Dynamic::from(row)
}

/// `app_state` 的结果（形状）：`panels` / `rev` / `prefs_rev` 平铺，
/// `layout` / `window` / `prefs` 各成一段。分段的理由见设计稿：三层数据的
/// 变化频率不同，需要靠 `rev` / `prefs_rev` 分开判断要不要重取。
fn app_snapshot_map(snapshot: AppSnapshot) -> Dynamic {
    let panels: Array = snapshot.panels.iter().map(app_panel_map).collect();
    let mut layout = Map::new();
    layout.insert("leaves".into(), Dynamic::from(snapshot.leaves as i64));
    let mut window = Map::new();
    window.insert(
        "full_screen".into(),
        Dynamic::from(snapshot.window.full_screen),
    );
    window.insert(
        "always_on_top".into(),
        Dynamic::from(snapshot.window.always_on_top),
    );
    window.insert("focus_mode".into(), Dynamic::from(snapshot.window.focus_mode));
    window.insert(
        "opacity".into(),
        Dynamic::from(snapshot.window.opacity as f64),
    );
    let mut prefs = Map::new();
    prefs.insert("language".into(), Dynamic::from(snapshot.prefs_language));
    let mut map = Map::new();
    map.insert("rev".into(), Dynamic::from(snapshot.rev as i64));
    map.insert("panels".into(), Dynamic::from(panels));
    map.insert("layout".into(), Dynamic::from(layout));
    map.insert("window".into(), Dynamic::from(window));
    map.insert("prefs".into(), Dynamic::from(prefs));
    map.insert("prefs_rev".into(), Dynamic::from(snapshot.prefs_rev as i64));
    Dynamic::from(map)
}

/// 应用面写类动词的回执：`changed` + `rev` + 写入**之后**的 `panels` 全景。
fn app_delta_map(delta: AppDelta) -> Dynamic {
    let panels: Array = delta.panels.iter().map(app_panel_map).collect();
    let changed: Array = delta.changed.into_iter().map(Dynamic::from).collect();
    let mut map = Map::new();
    map.insert("changed".into(), Dynamic::from(changed));
    map.insert("rev".into(), Dynamic::from(delta.rev as i64));
    map.insert("panels".into(), Dynamic::from(panels));
    Dynamic::from(map)
}

/// `#{action: "dock", …}` → [`LayoutOp`]。
///
/// 与 `session_open` 同一套纪律：未知 `action`、缺必填、**多给键**都报错。
/// 多给键尤其要拦：`dock` 传了 `index: 3` 但 `redock` 只认 `panel` 时，
/// 静默忽略会让人以为位次生效了。
///
/// 这里只做**形状**校验；`panel` / `zone` 的名字由宿主校验（`PanelId` /
/// `DockZone` 住在 `term-app`，这层拿不到）。
fn layout_op_from_map(spec: &Map) -> Result<LayoutOp, Box<EvalAltResult>> {
    let action = spec
        .get("action")
        .and_then(|value| value.clone().into_string().ok())
        .ok_or_else(|| {
            string_to_rhai("app_layout 需要 action（reset / dock / redock）".to_string())
        })?;
    let allowed: &[&str] = match action.as_str() {
        "reset" => &["action"],
        "dock" => &["action", "panel", "zone", "index"],
        "redock" => &["action", "panel"],
        other => {
            return Err(string_to_rhai(format!(
                "app_layout 不认识 action {other:?}（可用 reset / dock / redock）"
            )))
        }
    };
    for key in spec.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(string_to_rhai(format!(
                "app_layout 的 {action} 不接受键 {key:?}（可用：{}）",
                allowed.join(" / ")
            )));
        }
    }
    let panel_name = |spec: &Map| -> Result<String, Box<EvalAltResult>> {
        spec.get("panel")
            .and_then(|value| value.clone().into_string().ok())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| string_to_rhai("app_layout 需要 panel（面板名）".to_string()))
    };
    match action.as_str() {
        "reset" => Ok(LayoutOp::Reset),
        "redock" => Ok(LayoutOp::Redock {
            panel: panel_name(spec)?,
        }),
        _ => {
            let zone = spec
                .get("zone")
                .and_then(|value| value.clone().into_string().ok())
                .ok_or_else(|| {
                    string_to_rhai("app_layout 的 dock 需要 zone（left / right / bottom）".to_string())
                })?;
            let index = match spec.get("index") {
                None => None,
                Some(value) => {
                    let raw = value.clone().as_int().map_err(|_| {
                        string_to_rhai("app_layout 的 index 必须是整数（0 起）".to_string())
                    })?;
                    if raw < 0 {
                        return Err(string_to_rhai(format!(
                            "app_layout 的 index {raw} 不能为负"
                        )));
                    }
                    Some(u16::try_from(raw).map_err(|_| {
                        string_to_rhai(format!("app_layout 的 index {raw} 太大"))
                    })?)
                }
            };
            Ok(LayoutOp::Dock {
                panel: panel_name(spec)?,
                zone,
                index,
            })
        }
    }
}

fn dir_entries_map(entries: Vec<DirEntry>) -> Map {
    let rows: Array = entries
        .into_iter()
        .map(|entry| {
            let mut row = Map::new();
            row.insert("name".into(), Dynamic::from(entry.name));
            row.insert("dir".into(), Dynamic::from(entry.dir));
            row.insert("size".into(), Dynamic::from(entry.size as i64));
            Dynamic::from(row)
        })
        .collect();
    let mut map = Map::new();
    map.insert("entries".into(), Dynamic::from(rows));
    map
}

fn file_stat_map(stat: FileStat) -> Dynamic {
    let mut map = Map::new();
    map.insert("path".into(), Dynamic::from(stat.path));
    map.insert("size".into(), Dynamic::from(stat.size as i64));
    map.insert("dir".into(), Dynamic::from(stat.dir));
    map.insert("readonly".into(), Dynamic::from(stat.readonly));
    map.insert(
        "modified".into(),
        match stat.modified {
            Some(secs) => Dynamic::from(secs as i64),
            None => Dynamic::UNIT,
        },
    );
    Dynamic::from(map)
}

fn proc_output_map(output: ProcOutput) -> Dynamic {
    let mut map = Map::new();
    map.insert(
        "code".into(),
        match output.code {
            Some(code) => Dynamic::from(code as i64),
            None => Dynamic::UNIT,
        },
    );
    map.insert("stdout".into(), Dynamic::from(output.stdout));
    map.insert("stderr".into(), Dynamic::from(output.stderr));
    Dynamic::from(map)
}

fn profile_row_map(config: &SessionConfig) -> Dynamic {
    let mut map = Map::new();
    map.insert("name".into(), Dynamic::from(config.name.clone()));
    map.insert("protocol".into(), Dynamic::from(protocol_tag(config.protocol)));
    map.insert("host".into(), Dynamic::from(config.host.clone()));
    map.insert("group".into(), Dynamic::from(config.group.clone()));
    Dynamic::from(map)
}

fn profile_config_map(config: &SessionConfig) -> Dynamic {
    let mut map = profile_row_map(config).cast::<Map>();
    map.insert("port".into(), Dynamic::from(config.port as i64));
    map.insert("username".into(), Dynamic::from(config.username.clone()));
    map.insert("auth".into(), Dynamic::from(auth_tag(config.auth)));
    map.insert("term".into(), Dynamic::from(config.term.clone()));
    map.insert("charset".into(), Dynamic::from(config.charset.clone()));
    map.insert(
        "shell_command".into(),
        Dynamic::from(config.shell_command.clone()),
    );
    map.insert("builtin_ssh".into(), Dynamic::from(config.builtin_ssh));
    map.insert(
        "identity_file".into(),
        Dynamic::from(config.identity_file.clone()),
    );
    Dynamic::from(map)
}

/// `profile_set` 的未知字段错误。用新类型而非裸 `String`：这条只在引擎内部
/// 流转，没必要为它再破一次「全仓错误边界 String 化只许减不许增」的基线。
struct UnknownProfileKeys(String);

/// `profile_set` 里允许改的键。**未知键收集后统一报错**：写错字段名静默无效
/// 最难查——脚本会拿着「保存成功」的回执，以为配置已经改了。
fn apply_profile_map(
    config: &mut SessionConfig,
    spec: &Map,
    extra_allowed: &[&str],
) -> Result<(), UnknownProfileKeys> {
    let allowed = "host / port / username / group / term / charset / description / protocol \
                   / auth / shell_command / identity_file / builtin_ssh";
    let mut unknown = Vec::new();
    for (key, value) in spec {
        match key.as_str() {
            "host" => config.host = value.clone().into_string().unwrap_or_default(),
            "port" => config.port = value.as_int().unwrap_or(config.port as i64) as u16,
            "username" => config.username = value.clone().into_string().unwrap_or_default(),
            "group" => config.group = value.clone().into_string().unwrap_or_default(),
            "term" => config.term = value.clone().into_string().unwrap_or_default(),
            "charset" => config.charset = value.clone().into_string().unwrap_or_default(),
            "description" => {
                config.description = value.clone().into_string().unwrap_or_default()
            }
            "protocol" => {
                if let Ok(text) = value.clone().into_string() {
                    if let Some(protocol) = protocol_from_tag(&text) {
                        config.protocol = protocol;
                    }
                }
            }
            "auth" => {
                if let Ok(text) = value.clone().into_string() {
                    if let Some(auth) = auth_from_tag(&text) {
                        config.auth = auth;
                    }
                }
            }
            "shell_command" => {
                config.shell_command = value.clone().into_string().unwrap_or_default()
            }
            "identity_file" => {
                config.identity_file = value.clone().into_string().unwrap_or_default()
            }
            "builtin_ssh" => config.builtin_ssh = value.as_bool().unwrap_or(config.builtin_ssh),
            other if extra_allowed.contains(&other) => {}
            // 其余键不在本动词的可改范围里（profile_set 里连 name 也不收）。
            other => unknown.push(other.to_string()),
        }
    }
    if !unknown.is_empty() {
        return Err(UnknownProfileKeys(format!(
            "profile_set 不认识键 {}（可用：{allowed}）",
            unknown.join(", ")
        )));
    }
    Ok(())
}

/// `Protocol` → 脚本里的短标签（小写，与 [`protocol_from_tag`] 互为逆）。
fn protocol_tag(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Shell => "shell",
        Protocol::Ssh => "ssh",
        Protocol::Telnet => "telnet",
        Protocol::Serial => "serial",
        Protocol::Tcp => "tcp",
    }
}

fn protocol_from_tag(tag: &str) -> Option<Protocol> {
    match tag.to_ascii_lowercase().as_str() {
        "shell" => Some(Protocol::Shell),
        "ssh" => Some(Protocol::Ssh),
        "telnet" => Some(Protocol::Telnet),
        "serial" => Some(Protocol::Serial),
        "tcp" => Some(Protocol::Tcp),
        _ => None,
    }
}

fn auth_tag(auth: AuthMethod) -> &'static str {
    match auth {
        AuthMethod::Password => "password",
        AuthMethod::PublicKey => "publickey",
        AuthMethod::Agent => "agent",
        AuthMethod::GssapiWithMic => "gssapi",
        AuthMethod::KeyboardInteractive => "keyboard",
    }
}

fn auth_from_tag(tag: &str) -> Option<AuthMethod> {
    match tag.to_ascii_lowercase().as_str() {
        "password" => Some(AuthMethod::Password),
        "publickey" | "public_key" | "key" => Some(AuthMethod::PublicKey),
        "agent" => Some(AuthMethod::Agent),
        "gssapi" => Some(AuthMethod::GssapiWithMic),
        "keyboard" | "keyboard-interactive" | "kbdint" => {
            Some(AuthMethod::KeyboardInteractive)
        }
        _ => None,
    }
}

/// `session_open(#{…})` 的 map → [`OpenRequest`]。
///
/// 未知键报错：`#{profiel: "x"}` 静默变成「开本地 shell」是最坏的一种错——
/// 脚本会以为连上了，然后在本地 shell 里发远程命令。
fn open_request_from_map(
    spec: &Map,
    default_cols: usize,
    default_rows: usize,
) -> Result<OpenRequest, Box<EvalAltResult>> {
    let cols = spec
        .get("cols")
        .and_then(|v| v.as_int().ok())
        .map(|v| v.max(1) as usize)
        .unwrap_or(default_cols);
    let rows = spec
        .get("rows")
        .and_then(|v| v.as_int().ok())
        .map(|v| v.max(1) as usize)
        .unwrap_or(default_rows);
    let allowed = ["profile", "config", "command", "local", "cols", "rows", "name"];
    for key in spec.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(string_to_rhai(format!(
                "session_open 不认识键 {key:?}（可用：profile / config / command / local / cols / rows / name）"
            )));
        }
    }
    let has = |key: &str| spec.contains_key(key);
    let count = ["profile", "config", "command", "local"]
        .iter()
        .filter(|key| has(key))
        .count();
    if count != 1 {
        return Err(string_to_rhai(
            "session_open 需要且只能给一个来源：profile / config / command / local".to_string(),
        ));
    }
    if let Some(profile) = spec.get("profile") {
        let name = profile.clone().into_string().map_err(|_| {
            string_to_rhai("session_open 的 profile 必须是字符串".to_string())
        })?;
        return Ok(OpenRequest::Profile { name });
    }
    if let Some(config) = spec.get("config") {
        let map = config.clone().try_cast::<Map>().ok_or_else(|| {
            string_to_rhai("session_open 的 config 必须是 Map".to_string())
        })?;
        let mut base = SessionConfig::new(Protocol::Ssh);
        apply_profile_map(&mut base, &map, &["name"])
            .map_err(|e| string_to_rhai(e.0))?;
        if let Some(name) = map.get("name").and_then(|v| v.clone().into_string().ok()) {
            base.name = name;
        }
        return Ok(OpenRequest::Config(Box::new(base)));
    }
    if let Some(command) = spec.get("command") {
        let command = command.clone().into_string().map_err(|_| {
            string_to_rhai("session_open 的 command 必须是字符串".to_string())
        })?;
        let name = spec
            .get("name")
            .and_then(|v| v.clone().into_string().ok())
            .filter(|s| !s.is_empty());
        return Ok(OpenRequest::Command {
            command,
            name,
            cols,
            rows,
        });
    }
    Ok(OpenRequest::LocalShell { cols, rows })
}

/// `#{kind: …}` → [`Ask`]。`kind` 必填；未知 `kind` 与未知键都报错。
/// `Answer` → Rhai 值。**形态跟着问题的种类走**，不做统一包装：
/// 通报类（`alert`）给 `true`、二选一给 `bool`、填字类给 `string`、
/// 选择类给落选下标 `int`。脚本写 `let n = ask(#{kind: 'choose', …})`
/// 拿到的就是数字，不必再拆一层 `#{value: …}`。
fn answer_to_dynamic(answer: Answer) -> Dynamic {
    match answer {
        Answer::Ack => flag_map(true),
        Answer::Bool(value) => flag_map(value),
        Answer::Text(text) => Dynamic::from(text),
        Answer::Choice(index) => count_map(index as i64),
    }
}

fn ask_from_map(spec: &Map) -> Result<Ask, Box<EvalAltResult>> {
    let kind = spec
        .get("kind")
        .and_then(|v| v.clone().into_string().ok())
        .ok_or_else(|| string_to_rhai("ask 需要 kind（message / confirm / text / secret / choose / alert）".to_string()))?;
    let text = |key: &str| -> String {
        spec.get(key)
            .and_then(|v| v.clone().into_string().ok())
            .unwrap_or_default()
    };
    let ask = match kind.as_str() {
        "alert" => Ask::Alert {
            title: text("title"),
            text: text("text"),
        },
        "confirm" => Ask::Confirm {
            title: text("title"),
            text: text("text"),
        },
        "text" => Ask::Text {
            title: text("title"),
            prompt: text("prompt"),
            default: text("default"),
        },
        "secret" => Ask::Secret {
            title: text("title"),
            prompt: text("prompt"),
        },
        "choose" => {
            let options = spec
                .get("options")
                .and_then(|v| v.clone().try_cast::<Array>())
                .ok_or_else(|| string_to_rhai("ask(#{kind: 'choose'}) 需要 options 数组".to_string()))?;
            Ask::Choose {
                title: text("title"),
                prompt: text("prompt"),
                options: array_to_strings(options, "choose")?,
            }
        }
        "message" => message_from_map(spec)?,
        other => {
            return Err(string_to_rhai(format!(
                "ask 不认识的 kind {other:?}（可用：alert / message / confirm / text / secret / choose）"
            )))
        }
    };
    Ok(ask)
}

fn message_from_map(spec: &Map) -> Result<Ask, Box<EvalAltResult>> {
    let text = |key: &str| -> String {
        spec.get(key)
            .and_then(|v| v.clone().into_string().ok())
            .unwrap_or_default()
    };
    let buttons = match text("buttons").as_str() {
        "" | "ok" => MessageButton::Ok,
        "ok_cancel" | "okcancel" => MessageButton::OkCancel,
        "yes_no" | "yesno" => MessageButton::YesNo,
        "yes_no_cancel" | "yesnocancel" => MessageButton::YesNoCancel,
        "abort_retry_ignore" | "abortretryignore" => MessageButton::AbortRetryIgnore,
        "retry_cancel" | "retrycancel" => MessageButton::RetryCancel,
        other => {
            return Err(string_to_rhai(format!(
                "message 的 buttons {other:?} 不认识（可用：ok / ok_cancel / yes_no / yes_no_cancel / abort_retry_ignore / retry_cancel）"
            )))
        }
    };
    let icon = match text("icon").as_str() {
        "" | "none" => MessageIcon::None,
        "info" => MessageIcon::Info,
        "warning" | "warn" => MessageIcon::Warning,
        "error" => MessageIcon::Error,
        "question" => MessageIcon::Question,
        other => {
            return Err(string_to_rhai(format!(
                "message 的 icon {other:?} 不认识（可用：none / info / warning / error / question）"
            )))
        }
    };
    let default = spec
        .get("default")
        .and_then(|v| v.clone().into_string().ok())
        .filter(|s| !s.is_empty());
    if let Some(button) = &default {
        if !buttons.contains(button) {
            return Err(string_to_rhai(format!(
                "message 的 default {button:?} 不在这一组按钮里"
            )));
        }
    }
    Ok(Ask::Message {
        title: text("title"),
        text: text("text"),
        icon,
        buttons,
        default_button: default,
    })
}

fn pick_file_from_map(spec: &Map) -> Result<PickFile, Box<EvalAltResult>> {
    let text = |key: &str| -> String {
        spec.get(key)
            .and_then(|v| v.clone().into_string().ok())
            .unwrap_or_default()
    };
    let title = text("title");
    let filter = text("filter");
    let default = text("default");
    match text("mode").as_str() {
        "open" => Ok(PickFile::Open {
            title,
            filter,
            default,
        }),
        "save" => Ok(PickFile::Save {
            title,
            filter,
            default,
        }),
        "" => Err(string_to_rhai(
            "pick_file 需要 mode（open / save）".to_string(),
        )),
        other => Err(string_to_rhai(format!(
            "pick_file 的 mode {other:?} 不认识（可用：open / save）"
        ))),
    }
}

/// `log_start(dir, #{name, host, append, raw, max_mb})` 的参数。
fn log_spec_from_map(
    spec: &Map,
) -> Result<(Option<String>, Option<String>, term_core::log_opts::LogOptions), Box<EvalAltResult>> {
    for key in spec.keys() {
        if !["name", "host", "append", "raw", "max_mb"].contains(&key.as_str()) {
            return Err(string_to_rhai(format!(
                "log_start 不认识键 {key:?}（可用：name / host / append / raw / max_mb）"
            )));
        }
    }
    let name = spec
        .get("name")
        .and_then(|v| v.clone().into_string().ok())
        .filter(|s| !s.trim().is_empty());
    let host = spec
        .get("host")
        .and_then(|v| v.clone().into_string().ok())
        .filter(|s| !s.trim().is_empty());
    let mut opts = term_core::log_opts::LogOptions::default();
    if let Some(append) = spec.get("append").and_then(|v| v.as_bool().ok()) {
        // append=true → overwrite=Some(false)（追加）；false → 覆盖。
        opts.overwrite = Some(!append);
    }
    if spec.get("raw").and_then(|v| v.as_bool().ok()).unwrap_or(false) {
        opts.log_type = term_core::log_opts::LogType::All;
    }
    if let Some(mb) = spec.get("max_mb").and_then(|v| v.as_int().ok()) {
        opts.max_size_mb = mb.clamp(0, u32::MAX as i64) as u32;
    }
    Ok((name, host, opts))
}

/// `region` 的 1 起坐标校验（0 与负数显式报错，不静默夹到 1）。
fn checked_rect(
    r1: i64,
    c1: i64,
    r2: i64,
    c2: i64,
) -> Result<(usize, usize, usize, usize), Box<EvalAltResult>> {
    if [r1, c1, r2, c2].iter().any(|v| *v < 1) {
        return Err(primitive_to_rhai(PrimitiveError::BadArgument(
            "region 的坐标从 1 起（0 或负数无效）",
        )));
    }
    Ok((r1 as usize, c1 as usize, r2 as usize, c2 as usize))
}

/// 全局层失败 → Rhai 运行时错误。两类原因都原样透出：脚本作者看到的是
/// 「没有名为 "prod-db" 的会话」或「会话起不来：命令不存在」，而不是笼统的失败。
fn session_to_rhai(e: SessionError) -> Box<EvalAltResult> {
    string_to_rhai(e.to_string())
}

/// `session_open` 的尺寸参数：≥ 1。
///
/// 不静默夹到 1 或换成默认 80：那会让脚本里的换行位置与预期不符，而回报只是
/// 一个看不出来的错位（与 `--cols 8O` 不接受字母 O 是同一个理由）。
fn checked_size(cols: i64, rows: i64) -> Result<(usize, usize), Box<EvalAltResult>> {
    if cols < 1 || rows < 1 {
        return Err(session_to_rhai(SessionError::Primitive(
            PrimitiveError::BadArgument("终端尺寸必须 ≥ 1：cols / rows 至少为 1"),
        )));
    }
    Ok((cols as usize, rows as usize))
}

/// 引擎级单测：真 Rhai 编译执行 + 假会话（`testutil`）+ 沙箱旋钮。
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{fake_session, Manip};
    use std::collections::BTreeSet;
    use std::thread;
    use std::time::Duration;

    fn fast_sandbox() -> SandboxConfig {
        SandboxConfig {
            max_operations: 10_000,
            max_expr_depth: 16,
            max_global_depth: 32,
            execution_timeout: Duration::from_secs(5),
        }
    }

    /// ADR-0023 §决策 3 的脚本侧防线：**注册了什么，就必须等于表上投影出来的**。
    ///
    /// 为什么不是「从表里自动生成注册」：Rhai 的闭包是手写的类型适配
    /// （`f64`/`i64` 双重载、`GrepHandle` 参数、返回 Map 的字段拼装），
    /// 生成器会把「漏写一个重载」变成运行期才发现。所以保留手写，
    /// 但用这条断言把「漏注册 / 多注册」变成编译测试期的红。
    ///
    /// 取名字的方式是问 Rhai 自己（`gen_fn_signatures`），不是读一份常量表——
    /// 常量表会和注册代码一起漂移，那就等于没有防线。
    #[test]
    fn registered_primitives_match_the_script_projection_of_the_surface_table() {
        let (session, _manip) = fake_session(1);
        let mut engine = Engine::new();
        engine.set_max_operations(10_000);
        ScriptEngine::register_primitives(&mut engine, ScriptWorkspace::single(session, Duration::from_secs(5)));

        // 签名形如 `write(&str)` / `read(i64)`；取左括号前的部分，去掉命名空间。
        let registered: BTreeSet<String> = engine
            .gen_fn_signatures(false)
            .into_iter()
            .filter_map(|sig| {
                let name = sig.split('(').next()?.trim().to_string();
                Some(name.rsplit("::").next()?.to_string())
            })
            .collect();

        let declared: BTreeSet<String> = term_core::surface::script_names()
            .into_iter()
            .map(str::to_string)
            .collect();

        let missing: Vec<&String> = declared.difference(&registered).collect();
        assert!(
            missing.is_empty(),
            "能力面声明了但引擎没注册：{missing:?}（脚本调用会报 Function not found）"
        );

        // 注册集合 = 能力面动词 ∪ 语言辅助函数。
        //
        // `grep` / `grep_v` 是**过滤器构造函数**，不是会话动词：它们不碰会话、
        // 没有远程等价物（`term_core::surface` 描述的是「会话能做什么」），
        // 结果是一个只能传给 read / read_until / wait_any 的句柄。所以它们
        // 不该进能力面表，但确实是脚本词汇的一部分，必须能在此处被点名。
        //
        // 显式列出而不是放宽断言：任何**新增**的未声明函数都会落进 `extra`
        // 并让这条测试红——想消红只有两条路：进表，或承认它是辅助函数并写在这里。
        const SCRIPT_LANGUAGE_HELPERS: &[&str] = &["grep", "grep_v"];
        let extras: BTreeSet<String> = registered.difference(&declared).cloned().collect();
        let helpers: BTreeSet<String> = SCRIPT_LANGUAGE_HELPERS.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            extras, helpers,
            "注册集合里有既不在能力面、也不在辅助函数名单里的函数：{}",
            extras.difference(&helpers).cloned().collect::<Vec<_>>().join(", ")
        );
        for helper in SCRIPT_LANGUAGE_HELPERS {
            assert!(
                !declared.contains(*helper),
                "{helper} 是语言辅助函数，不该出现在会话能力面表里"
            );
        }
    }

    /// 生产者反复推同一段输出：无论脚本何时登记，登记后必有一段到达，
    /// 消除「输出抢在登记前」的竞态（假会话不回显，无别的输出源）。
    fn spawn_producer(manip: Manip, text: &'static str) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            for _ in 0..20 {
                thread::sleep(Duration::from_millis(50));
                manip.push_output(text);
            }
        })
    }

    /// wait_any 单测用：登记之后才推一次输出（§B.13 先登记后消费），
    /// 单次推送让「最先出现者胜」的断言完全确定。
    fn push_once_delayed(manip: Manip, text: &'static str) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            manip.push_output(text);
        })
    }

    #[test]
    fn a_script_drives_the_primitives_end_to_end() {
        // Send 友好：整个脚本跑在独立线程，证明引擎可搬家。
        let handle = thread::spawn(|| {
            let (session, manip) = fake_session(1);
            let producer = spawn_producer(manip.clone(), "welcome, guest\n@@DATA\n");
            let engine = ScriptEngine::new(SandboxConfig::default());
            // '|' 分隔三个原语的返回值，避免与多行文本混淆；哨兵用 "@@"
            // （"###" 会与 raw string 的 # 收尾符打架）。
            let script = r#"
                write("login\n");
                let w = wait("welcome", 3.0);
                let r = read_until("@@", 3.0);
                w + "|" + r + "|" + screen()
            "#;
            let out = engine.run(script, session).expect("脚本应跑通");
            producer.join().expect("生产者");
            (out, manip.written())
        });
        let (out, written) = handle.join().expect("脚本线程");
        assert_eq!(written, b"login\n".to_vec(), "write 应原样写入、不自动加换行");
        let mut parts = out.split('|');
        assert_eq!(parts.next(), Some("welcome"), "wait 命中返回 text_upto");
        let read = parts.next().unwrap_or("");
        assert!(read.ends_with("@@"), "read_until 返回文本含 delimiter：{read:?}");
        let screen = parts.next().unwrap_or("");
        assert!(screen.contains("welcome, guest"), "screen 全文应在册：{screen:?}");
    }

    #[test]
    fn script_level_grep_v_filters_after_matching_and_read_takes_the_tail() {
        // 2026-11-19 裁定：过滤是结果后处理。噪声行里的哨兵**照样命中**，
        // body 的返回文本里那行被 grep_v 清掉；消费停在命中结尾，read 取尾部。
        let (session, manip) = fake_session(1);
        let producer = {
            let manip = manip.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(30));
                manip.push_output("noise END_MARK decoy\nkeep1\nEND_MARK tail-after\n");
            })
        };
        let engine = ScriptEngine::new(SandboxConfig::default());
        let script = r#"
            let f = grep_v("^noise");
            let body = read_until("END_MARK", 5.0, f);
            let tail = read(1.0);
            body + "|" + tail
        "#;
        let out = engine.run(script, session).expect("过滤脚本应跑通");
        producer.join().expect("生产者");
        let mut parts = out.splitn(2, '|');
        let body = parts.next().unwrap_or("");
        let tail = parts.next().unwrap_or("");
        // 最早命中在噪声行 "noise END_MARK decoy"：命中照常发生，
        // 命中文本止于命中串（无换行）按约定透传；消费推到命中结尾，
        // read 取走该行余段 + 后续全部（并按 grep_v 清掉噪声行）。
        assert_eq!(body, "noise END_MARK", "命中不受 grep 影响：{body:?}");
        assert!(tail.contains("keep1"), "消费停在命中结尾，其余在尾部：{tail:?}");
        assert!(tail.contains("tail-after"), "read 取走剩余全部：{tail:?}");
    }

    #[test]
    fn invalid_regex_in_grep_fails_explicitly() {
        // 无效正则必须显式报错（脚本作者可见），不许静默当空过滤器。
        let (session, _m) = fake_session(1);
        let engine = ScriptEngine::new(SandboxConfig::default());
        let err = engine
            .run(r#"grep("[")"#, session)
            .expect_err("无效正则必须显式报错");
        let msg = match err {
            ScriptError::Runtime(m, ..) => m,
            other => panic!("应是运行时错误：{other:?}"),
        };
        assert!(
            msg.contains("无效模式") || msg.contains("无效正则"),
            "文案应指明模式无效：{msg}"
        );
    }

    #[test]
    fn wait_any_script_returns_one_based_index_and_match_text() {
        // 基本命中：模式数组里第 2 个先出现 → index = 2（1 起），
        // text 是过滤视图里自游标到命中串结尾的那段（以命中串结尾）。
        let (session, manip) = fake_session(1);
        let producer = push_once_delayed(manip, "banner\nBBB comes first\nAAA later\n");
        let engine = ScriptEngine::new(SandboxConfig::default());
        // Map 直接返给 run 会走 to_string()，所以脚本内部拼成字符串。
        let script = r#"
            let hit = wait_any(["AAA", "BBB"], 5.0);
            hit.index.to_string() + "|" + hit.text
        "#;
        let out = engine.run(script, session).expect("wait_any 脚本应跑通");
        producer.join().expect("生产者");
        let mut parts = out.splitn(2, '|');
        assert_eq!(parts.next(), Some("2"), "1 起索引、最先出现者胜：{out:?}");
        let text = parts.next().unwrap_or("");
        assert!(text.ends_with("BBB"), "text 应以命中串结尾：{text:?}");
    }

    #[test]
    fn wait_any_i64_timeout_overload_dispatches() {
        // i64 超时重载：脚本写 5（INT）不该撞「Function not found」。
        let (session, manip) = fake_session(1);
        let producer = push_once_delayed(manip, "banner\nX_MARK here\n");
        let engine = ScriptEngine::new(SandboxConfig::default());
        let script = r#"
            let hit = wait_any(["X_MARK"], 5);
            hit.index.to_string() + "|" + hit.text
        "#;
        let out = engine.run(script, session).expect("i64 超时重载应可执行");
        producer.join().expect("生产者");
        let mut parts = out.splitn(2, '|');
        assert_eq!(parts.next(), Some("1"), "单模式命中 index = 1：{out:?}");
        assert!(
            parts.next().unwrap_or("").ends_with("X_MARK"),
            "text 应以命中串结尾：{out:?}"
        );
    }

    #[test]
    fn wait_any_filter_overload_filters_after_matching() {
        // 新语义：过滤在结果之后。噪声行里的 "HIT"（数组第 1 项）照常命中
        // → index = 1；返回文本里那行被 grep_v 清掉。
        let (session, manip) = fake_session(1);
        let producer = push_once_delayed(manip, "noise: HIT decoy\nkeep\nREAL_HIT done\n");
        let engine = ScriptEngine::new(SandboxConfig::default());
        let script = r#"
            let f = grep_v("^noise");
            let hit = wait_any(["HIT", "REAL_HIT"], 5.0, f);
            hit.index.to_string() + "|" + hit.text
        "#;
        let out = engine.run(script, session).expect("过滤重载应可执行");
        producer.join().expect("生产者");
        let mut parts = out.splitn(2, '|');
        assert_eq!(parts.next(), Some("1"), "噪声行里的 HIT 也命中：{out:?}");
        assert_eq!(
            parts.next().unwrap_or(""),
            "noise: HIT",
            "命中在噪声行照常返回，grep 不参与匹配"
        );
    }

    #[test]
    fn wait_any_i64_timeout_with_filter_overload_dispatches() {
        // 第四个重载（Array, i64, GrepHandle）也要能编译执行。
        let (session, manip) = fake_session(1);
        let producer = push_once_delayed(manip, "noise: HIT decoy\nREAL_HIT done\n");
        let engine = ScriptEngine::new(SandboxConfig::default());
        let script = r#"
            let f = grep_v("^noise");
            let hit = wait_any(["REAL_HIT"], 5, f);
            hit.index.to_string() + "|" + hit.text
        "#;
        let out = engine.run(script, session).expect("i64+过滤重载应可执行");
        producer.join().expect("生产者");
        let mut parts = out.splitn(2, '|');
        assert_eq!(parts.next(), Some("1"), "index = 1：{out:?}");
        assert!(
            parts.next().unwrap_or("").ends_with("REAL_HIT"),
            "text 应以命中串结尾：{out:?}"
        );
    }

    #[test]
    fn wait_any_non_string_array_items_fail_explicitly() {
        // 非字符串元素必须显式报 Rhai 运行时错误，写清第几项；
        // 不许静默 to_string() 后当成模式串去等。
        let (session, _manip) = fake_session(1);
        let engine = ScriptEngine::new(SandboxConfig::default());
        let err = engine
            .run(r#"wait_any([1, 2], 1.0)"#, session)
            .expect_err("非字符串元素必须显式报错");
        let msg = match err {
            ScriptError::Runtime(m, ..) => m,
            other => panic!("应是运行时错误：{other:?}"),
        };
        assert!(
            msg.contains("第 1 项必须是字符串"),
            "错误文案应指明第几项：{msg}"
        );
    }

    #[test]
    fn wait_any_empty_array_fails_explicitly() {
        // 空数组透传给 ScriptSession::wait_any → EmptyPattern，显式报错。
        let (session, _manip) = fake_session(1);
        let engine = ScriptEngine::new(SandboxConfig::default());
        let err = engine
            .run(r#"wait_any([], 1.0)"#, session)
            .expect_err("空模式列表必须显式报错");
        assert!(matches!(err, ScriptError::Runtime(_, _)), "{err:?}");
    }

    #[test]
    fn op_limit_blocks_an_infinite_loop_script() {
        let (session, _m) = fake_session(1);
        let engine = ScriptEngine::new(SandboxConfig {
            max_operations: 5_000,
            ..fast_sandbox()
        });
        let err = engine
            .run("while true { let x = 1; }", session)
            .expect_err("死循环必须被沙箱拦下");
        assert!(matches!(err, ScriptError::Runtime(_, _)), "{err:?}");
    }

    #[test]
    fn expr_depth_limit_blocks_a_runaway_expression() {
        let (session, _m) = fake_session(1);
        let engine = ScriptEngine::new(fast_sandbox());
        // 24 层嵌套括号超过 max_expr_depth = 16，编译期即拒。
        let script = "((((((((((((((((((((((((1))))))))))))))))))))))))";
        assert!(engine.run(script, session).is_err(), "超深表达式必须被拦");
    }

    #[test]
    fn wall_clock_timeout_stops_a_spin_loop() {
        let (session, _m) = fake_session(1);
        let engine = ScriptEngine::new(SandboxConfig {
            max_operations: u64::MAX, // 操作数放行，只留墙钟
            max_expr_depth: 16,
            max_global_depth: 32,
            execution_timeout: Duration::from_millis(300),
        });
        let err = engine
            .run("while true { let x = 1; }", session)
            .expect_err("墙钟超时必须中断执行");
        assert!(matches!(err, ScriptError::Runtime(_, _)), "{err:?}");
    }

    #[test]
    fn a_wait_timeout_surfaces_as_an_explicit_script_error() {
        let (session, _manip) = fake_session(1);
        let engine = ScriptEngine::new(SandboxConfig::default());
        let err = engine
            .run("wait(\"never\", 0.05)", session)
            .expect_err("超时必须显式失败，不许静默空成功");
        assert!(matches!(err, ScriptError::Runtime(_, _)), "{err:?}");
    }

    #[test]
    fn a_syntax_error_carries_its_line_and_column_from_rhai() {
        // 错误必须能被定位到具体行——这是脚本编辑器「点错误跳行」的前提。
        let (session, _manip) = fake_session(1);
        let engine = ScriptEngine::new(SandboxConfig::default());
        // 三个语句，语法错在「第 3 行」。
        let script = "let a = 1;\nlet b = 2;\nlet = ;\n";
        let err = engine
            .run(script, session)
            .expect_err("语法错必须显式失败（且带位置）");
        match err {
            ScriptError::Syntax(_, pos) => {
                let (line, column) = pos.expect("语法错必须带位置");
                assert_eq!(line, 3, "错误应在第 3 行，实际 {line}");
                assert!(column >= 1, "列应为 1 起正数，实际 {column}");
            }
            other => panic!("应是语法错误：{other:?}"),
        }
    }

    #[test]
    fn a_runtime_throw_carries_its_line_from_rhai() {
        // 运行期错误也要能定位——Rhai 的 `throw` 会带上抛出点的行号。
        let (session, _manip) = fake_session(1);
        let engine = ScriptEngine::new(SandboxConfig::default());
        let script = "let a = 1;\nthrow \"boom\";\n";
        let err = engine
            .run(script, session)
            .expect_err("throw 必须显式失败（且带位置）");
        match err {
            ScriptError::Runtime(_, pos) => {
                let (line, column) = pos.expect("throw 必须带位置");
                assert_eq!(line, 2, "throw 在第 2 行，实际 {line}");
                assert!(column >= 1, "列应为 1 起正数，实际 {column}");
            }
            other => panic!("应是运行时错误：{other:?}"),
        }
    }

    #[test]
    fn a_primitive_timeout_runtime_error_carries_its_call_site_position() {
        // 原语错误（等待超时）由闭包返回 Err——Rhai 会把调用点位置补上，
        // 所以即便不是脚本作者手写的语法错，编辑器也能跳到 `wait` 那一行。
        let (session, _manip) = fake_session(1);
        let engine = ScriptEngine::new(SandboxConfig::default());
        let err = engine
            .run("wait(\"never\", 0.05)", session)
            .expect_err("超时必须显式失败");
        match err {
            ScriptError::Runtime(_, pos) => {
                let (line, column) = pos.expect("原语错误应带上调用点位置");
                assert_eq!(line, 1, "单行脚本的 wait 调用在第 1 行，实际 {line}");
                assert!(column >= 1, "列应为 1 起正数，实际 {column}");
            }
            other => panic!("应是运行时错误：{other:?}"),
        }
    }

    /// `last_error()` / `crt.GetLastErrorMessage()` 的 e2e：编译失败与执行失败
    /// 都要在共享 `ScriptContext` 上留一句可读文本，不能只把 Err 丢给调用方。
    #[test]
    fn compile_and_eval_failures_are_recorded_in_the_shared_context() {
        use crate::{HostEnv, ScriptContext, Sessions};
        use crate::testutil::FakeSessions;

        let fake = FakeSessions::new();
        fake.mount(1, "one");
        let sessions: Arc<dyn Sessions> = fake.clone();
        let current = sessions.attach(1).expect("会话 1");
        let context = Arc::new(ScriptContext::new(None, Vec::new()));
        let env = HostEnv::new(sessions).with_context(Arc::clone(&context));
        let ws = Arc::new(ScriptWorkspace::new(env, current, Duration::from_secs(2)));
        let engine = ScriptEngine::new(fast_sandbox());

        assert!(context.last_error().is_empty(), "干净启动不该有错误文本");
        engine
            .run_in_workspace("let = ;", Arc::clone(&ws))
            .expect_err("语法错必须失败");
        let syntax = context.last_error();
        assert!(syntax.contains("语法"), "语法错文案应可展示：{syntax}");

        engine
            .run_in_workspace("throw \"boom\";", ws)
            .expect_err("运行期错必须失败");
        let runtime = context.last_error();
        assert!(runtime.contains("boom"), "运行期错文案应可展示：{runtime}");
        assert_ne!(runtime, syntax, "后一次失败应覆盖前一次");
    }

    // ── profile_set：未知键 / NotFound 专用路径 ──────────────────────────

    struct TestCatalog(Arc<Mutex<Vec<term_core::session::SessionConfig>>>);

    impl term_core::session::ProfileCatalog for TestCatalog {
        fn sessions(
            &self,
        ) -> Result<Vec<term_core::session::SessionConfig>, term_core::session::CatalogError> {
            Ok(self.0.lock().expect("catalog 锁").clone())
        }

        fn save(
            &self,
            rows: &[term_core::session::SessionConfig],
        ) -> Result<(), term_core::session::CatalogError> {
            *self.0.lock().expect("catalog 锁") = rows.to_vec();
            Ok(())
        }
    }

    struct FakeWithCatalog {
        inner: Arc<crate::testutil::FakeSessions>,
        catalog: Arc<TestCatalog>,
    }

    impl crate::Sessions for FakeWithCatalog {
        fn list(&self) -> Vec<crate::sessions::SessionInfo> {
            self.inner.list()
        }
        fn resolve(&self, name: &str) -> Option<term_core::session_runtime::SessionId> {
            self.inner.resolve(name)
        }
        fn name(&self, id: term_core::session_runtime::SessionId) -> Option<String> {
            self.inner.name(id)
        }
        fn open(
            &self,
            request: &term_core::session::OpenRequest,
        ) -> Result<crate::sessions::SessionInfo, SessionsError> {
            self.inner.open(request)
        }
        fn close(&self, name: &str) -> Result<(), SessionsError> {
            self.inner.close(name)
        }
        fn attach(
            &self,
            id: term_core::session_runtime::SessionId,
        ) -> Option<Arc<Mutex<dyn term_core::session_runtime::Session>>> {
            self.inner.attach(id)
        }
        fn profiles(&self) -> Option<Arc<dyn term_core::session::ProfileCatalog>> {
            Some(self.catalog.clone())
        }
    }

    fn workspace_with_catalog() -> (Arc<ScriptWorkspace>, Arc<TestCatalog>) {
        use crate::HostEnv;

        let fake = crate::testutil::FakeSessions::new();
        fake.mount(1, "one");
        let catalog = Arc::new(TestCatalog(Arc::new(Mutex::new(Vec::new()))));
        let sessions: Arc<dyn crate::Sessions> = Arc::new(FakeWithCatalog {
            inner: fake.clone(),
            catalog: Arc::clone(&catalog),
        });
        let current = sessions.attach(1).expect("会话 1");
        let ws = ScriptWorkspace::new(HostEnv::new(sessions), current, Duration::from_secs(2));
        (Arc::new(ws), catalog)
    }

    #[test]
    fn profile_set_rejects_unknown_keys_instead_of_silently_ignoring() {
        let (ws, catalog) = workspace_with_catalog();
        let engine = ScriptEngine::new(fast_sandbox());
        let err = engine
            .run_in_workspace(
                r#"profile_set("new", #{host: "h", hostname: "typo"})"#,
                ws,
            )
            .expect_err("未知键必须报错");
        let msg = match err {
            ScriptError::Runtime(msg, _) => msg,
            other => panic!("应是运行时错误：{other:?}"),
        };
        assert!(msg.contains("hostname"), "错误要点名未知键：{msg}");
        assert!(
            catalog.0.lock().expect("catalog 锁").is_empty(),
            "失败不该留下半截写盘"
        );
    }

    #[test]
    fn profile_set_only_creates_a_new_config_when_not_found() {
        use term_core::session::{Protocol, SessionConfig};

        let (ws, catalog) = workspace_with_catalog();
        catalog.0.lock().expect("catalog 锁").push(SessionConfig {
            name: "keep".to_string(),
            host: "old-host".to_string(),
            ..SessionConfig::new(Protocol::Ssh)
        });
        let engine = ScriptEngine::new(fast_sandbox());
        let out = engine
            .run_in_workspace(
                r#"profile_set("keep", #{port: 2222}).value.to_string()"#,
                ws,
            )
            .expect("保存已有配置");
        assert_eq!(out, "1", "回执给保存后的条数");
        let rows = catalog.0.lock().expect("catalog 锁");
        assert_eq!(rows.len(), 1, "只更新、不追加");
        assert_eq!(rows[0].host, "old-host", "已有配置不能被空壳覆盖");
        assert_eq!(rows[0].port, 2222);
    }
}
