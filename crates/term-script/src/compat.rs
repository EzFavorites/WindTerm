//! `crt` 兼容门面（ADR-0027）：把 SecureCRT 的**对象形状**映射到本仓动词。
//!
//! # 它做什么、不做什么
//!
//! 只做「名字 → 动词」的转发：不缓存、不推断、不替脚本补参数。照着手册写的
//! `crt.Screen.WaitForString("$", 5)` 能直接跑，是因为这里把
//! `WaitForString` 转发到 **`read_until`**（消费式），而不是 `wait`
//! （不推进游标）——这是门面存在的第一个理由。
//!
//! 其余四条关键映射：
//!
//! | CRT | 门面转发到 | 为什么不能直译 |
//! |---|---|---|
//! | `Screen.Get/Get2` | `region` / `region_lines` | 坐标 1 起 vs 0 起 |
//! | `MessageBox` 的整数返回值 | 按钮标签 → CRT 返回 ID（`ID*` 常量） | 老代码 `if r = 6` 可改成 `IDYES()`；输入标志是 `MB_*`，二者不混用 |
//! | `Log(enabled, append, raw)` | `log_start(dir, #{append, raw})` | 目录与模板我们分开传 |
//! | `Session.Connect(cmd)` | `session_connect(name)` | 我们「会话是一等对象」→ 开新会话并绑定 |
//!
//! # 为什么注册在能力面之外
//!
//! `term_core::surface` 描述的是「会话能做什么」，而门面是**同一批动词的
//! 另一种拼写**。把它们塞进能力面会让 RPC 也长出 `CrtScreen::send` 这种
//! 协议/对象名（违反 §B.14）。所以门面单独注册，`engine.rs` 里对账
//! 「注册集合 == 能力面」的那条测试只跑 `register_primitives`。
//!
//! # 不做语言对齐
//!
//! CRT 的 VBScript / Python 我们不支持，Rhai 是既定决策
//! （`docs/design/07-scripting-and-automation.md`）。门面提供的是**形状**，不是解释器。

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, Position};

use crate::error::PrimitiveError;
use crate::human::{Ask, MessageButton, MessageIcon, PickFile};
use crate::sessions::SessionInfo;
use crate::workspace::ScriptWorkspace;

/// 门面里 WaitForString / ReadString 省略超时参数时用的秒数（与引擎的缺省一致）。
const DEFAULT_WAIT_SECS: f64 = 30.0;

/// `crt()` 的句柄共享这个槽：`LogFileName` 在 CRT 里是对象属性，而门面每次
/// 取属性都造一个新句柄，所以状态必须放在句柄外面共享。
type SharedTemplate = Arc<Mutex<Option<String>>>;

// `Screen.Synchronous` 不做第二份状态：它直接读写工作区里的那一位原子，
// 与全局 `set_synchronous(on)` 是同一个真源（见 `ScriptWorkspace::synchronous`）。

/// `crt` 对象本体（`version()` / `arguments()` / `get_tab(…)` / `sleep` / `quit`）。
#[derive(Clone)]
pub struct Crt {
    ws: ScriptWorkspace,
    log_template: SharedTemplate,
}

/// `crt.Screen`。
#[derive(Clone)]
pub struct CrtScreen {
    ws: ScriptWorkspace,
}

/// `crt.Session`。
#[derive(Clone)]
pub struct CrtSession {
    ws: ScriptWorkspace,
    log_template: SharedTemplate,
}

/// `crt.Dialog`。
#[derive(Clone)]
pub struct CrtDialog {
    ws: ScriptWorkspace,
}

/// `crt.Clipboard`。
#[derive(Clone)]
pub struct CrtClipboard {
    ws: ScriptWorkspace,
}

/// `crt.Window`。
#[derive(Clone)]
pub struct CrtWindow {
    ws: ScriptWorkspace,
}

/// `crt.GetTab(i)` 返回的标签对象（`screen` 与 `session` 都作用在那条会话上）。
#[derive(Clone)]
pub struct CrtTab {
    ws: ScriptWorkspace,
    name: String,
    log_template: SharedTemplate,
}

/// `crt.Arguments`：`Count` 是属性，`Arguments(i)` 仍是取第 i 项的方法。
#[derive(Clone)]
pub struct CrtArguments {
    ws: ScriptWorkspace,
}

fn fail(message: impl ToString) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        message.to_string().into(),
        Position::NONE,
    ))
}

fn ok_map(value: Dynamic) -> Result<Dynamic, Box<EvalAltResult>> {
    Ok(value)
}

fn session_map(info: &SessionInfo) -> Dynamic {
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
    Dynamic::from(map)
}

/// 注册整套门面。由 [`crate::ScriptEngine::run_in_workspace`] 在注册完原语后调。
pub fn register(engine: &mut Engine, ws: ScriptWorkspace) {
    engine.register_type_with_name::<Crt>("Crt");
    engine.register_type_with_name::<CrtScreen>("CrtScreen");
    engine.register_type_with_name::<CrtSession>("CrtSession");
    engine.register_type_with_name::<CrtDialog>("CrtDialog");
    engine.register_type_with_name::<CrtClipboard>("CrtClipboard");
    engine.register_type_with_name::<CrtWindow>("CrtWindow");
    engine.register_type_with_name::<CrtTab>("CrtTab");
    engine.register_type_with_name::<CrtArguments>("CrtArguments");

    // ── 入口 `crt()` ──────────────────────────────────────────────────
    {
        let ws_for_crt = ws.clone();
        let log_template: SharedTemplate = Arc::new(Mutex::new(None));
        let tpl_for_crt = Arc::clone(&log_template);
        engine.register_fn("crt", move || Crt {
            ws: ws_for_crt.clone(),
            log_template: Arc::clone(&tpl_for_crt),
        });
    }

    // ── Crt：顶层成员 ─────────────────────────────────────────────────
    // 非捕获闭包是 Copy：同一个实现注册两次即可给 CRT 手册的 PascalCase 名，
    // 小写名保持向后兼容（老脚本与门面内部都还在用）。
    {
        let version = |crt: &mut Crt| -> Result<Dynamic, Box<EvalAltResult>> {
            ok_map(Dynamic::from(crt.ws.env().context.version.clone()))
        };
        engine.register_fn("version", version);
        engine.register_fn("Version", version);
    }
    {
        let full_name = |crt: &mut Crt| -> Result<Dynamic, Box<EvalAltResult>> {
            ok_map(Dynamic::from(
                crt.ws
                    .env()
                    .context
                    .script_path
                    .clone()
                    .unwrap_or_default(),
            ))
        };
        engine.register_fn("script_full_name", full_name);
        engine.register_fn("ScriptFullName", full_name);
    }
    {
        let all_args = |crt: &mut Crt| -> Result<Dynamic, Box<EvalAltResult>> {
            let args: Array = crt
                .ws
                .env()
                .context
                .args
                .iter()
                .map(|a| Dynamic::from(a.clone()))
                .collect();
            ok_map(Dynamic::from(args))
        };
        engine.register_fn("arguments", all_args);
        engine.register_fn("Arguments", all_args);
    }
    {
        let arg_at = |crt: &mut Crt, index: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            // CRT 的 `Arguments(n)` 是 0 起的；越界返回空串（CRT 同）。
            if index < 0 {
                return Err(fail("Arguments 的序号不能为负（0 起）"));
            }
            let args = &crt.ws.env().context.args;
            Ok(Dynamic::from(
                args.get(index as usize).cloned().unwrap_or_default(),
            ))
        };
        engine.register_fn("argument", arg_at);
        engine.register_fn("Argument", arg_at);
        engine.register_fn("Arguments", arg_at);
    }
    {
        let count = |crt: &mut Crt| -> Result<Dynamic, Box<EvalAltResult>> {
            Ok(Dynamic::from(crt.ws.session_list().len() as i64))
        };
        engine.register_fn("get_tab_count", count);
        engine.register_fn("GetTabCount", count);
    }
    {
        let tab_at = |crt: &mut Crt, index: i64| -> Result<CrtTab, Box<EvalAltResult>> {
            if index < 1 {
                return Err(fail("GetTab 的序号从 1 起"));
            }
            let info = crt.ws.session_at(index as usize).map_err(fail)?;
            make_tab(crt, info)
        };
        engine.register_fn("get_tab", tab_at);
        engine.register_fn("GetTab", tab_at);
    }
    {
        let active = |crt: &mut Crt| -> Result<CrtTab, Box<EvalAltResult>> {
            make_tab(crt, crt.ws.session_active().map_err(fail)?)
        };
        engine.register_fn("get_active_tab", active);
        engine.register_fn("GetActiveTab", active);
    }
    {
        let script_tab = |crt: &mut Crt| -> Result<CrtTab, Box<EvalAltResult>> {
            // CRT 的 GetScriptTab 指的是「启动脚本的那条标签」，不随 session_use 改绑。
            let name = crt.ws.script_session_name().map_err(fail)?;
            let info = crt
                .ws
                .session_info(Some(&name))
                .map_err(|e| fail(format!("脚本启动时绑定的会话 {name:?} 取不到：{e}")))?;
            make_tab(crt, info)
        };
        engine.register_fn("get_script_tab", script_tab);
        engine.register_fn("GetScriptTab", script_tab);
    }
    {
        let nap = |crt: &mut Crt, millis: i64| -> Result<(), Box<EvalAltResult>> {
            if millis < 0 {
                return Err(fail("Sleep 的毫秒数不能为负"));
            }
            crt.ws.sleep(millis as u64).map_err(fail)
        };
        engine.register_fn("sleep", nap);
        engine.register_fn("Sleep", nap);
    }
    {
        let bye = |crt: &mut Crt| -> Result<(), Box<EvalAltResult>> {
            crt.ws.app_quit(true).map_err(fail)
        };
        engine.register_fn("quit", bye);
        engine.register_fn("Quit", bye);
    }
    {
        let last = |crt: &mut Crt| -> Result<Dynamic, Box<EvalAltResult>> {
            ok_map(Dynamic::from(crt.ws.env().context.last_error()))
        };
        engine.register_fn("last_error", last);
        engine.register_fn("GetLastErrorMessage", last);
    }

    // ── 属性：crt.Screen / Session / Dialog / Clipboard / Window ──────
    {
        let screen = |crt: &mut Crt| CrtScreen {
            ws: crt.ws.clone(),
        };
        engine.register_get("screen", screen);
        engine.register_get("Screen", screen);
    }
    {
        let session = |crt: &mut Crt| CrtSession {
            ws: crt.ws.clone(),
            log_template: Arc::clone(&crt.log_template),
        };
        engine.register_get("session", session);
        engine.register_get("Session", session);
    }
    {
        let dialog = |crt: &mut Crt| CrtDialog { ws: crt.ws.clone() };
        engine.register_get("dialog", dialog);
        engine.register_get("Dialog", dialog);
    }
    {
        let clipboard = |crt: &mut Crt| CrtClipboard { ws: crt.ws.clone() };
        engine.register_get("clipboard", clipboard);
        engine.register_get("Clipboard", clipboard);
    }
    {
        let window = |crt: &mut Crt| CrtWindow { ws: crt.ws.clone() };
        engine.register_get("window", window);
        engine.register_get("Window", window);
    }
    {
        let args = |crt: &mut Crt| CrtArguments {
            ws: crt.ws.clone(),
        };
        engine.register_get("Arguments", args);
        engine.register_get("arguments", args);
    }

    // ── CrtTab：标签对象转发到它自己的会话 ────────────────────────────
    {
        let screen = |tab: &mut CrtTab| CrtScreen {
            ws: tab.ws.clone(),
        };
        engine.register_get("screen", screen);
        engine.register_get("Screen", screen);
    }
    {
        let session = |tab: &mut CrtTab| CrtSession {
            ws: tab.ws.clone(),
            log_template: Arc::clone(&tab.log_template),
        };
        engine.register_get("session", session);
        engine.register_get("Session", session);
    }
    {
        let name = |tab: &mut CrtTab| tab.name.clone();
        engine.register_get("name", name);
        engine.register_get("Name", name);
    }
    {
        let index = |tab: &mut CrtTab| -> i64 {
            // 标签栏顺序里的 1 起序号；查不到给 0（CRT 没有 0 号，脚本据此能判断）。
            tab.ws
                .session_list()
                .iter()
                .position(|s| s.name == tab.name)
                .map(|i| i as i64 + 1)
                .unwrap_or(0)
        };
        engine.register_fn("index", index);
        engine.register_fn("Index", index);
        // CRT 里 `Tab.Index` 是属性；方法写法只是兼容旧脚本。
        engine.register_get("index", index);
        engine.register_get("Index", index);
    }
    {
        let activate = |tab: &mut CrtTab| -> Result<(), Box<EvalAltResult>> {
            tab.ws.session_activate(&tab.name).map_err(fail)
        };
        engine.register_fn("activate", activate);
        engine.register_fn("Activate", activate);
    }
    engine.register_get("Count", |args: &mut CrtArguments| -> i64 {
        args.ws.env().context.args.len() as i64
    });
    engine.register_get("count", |args: &mut CrtArguments| -> i64 {
        args.ws.env().context.args.len() as i64
    });

    // ── CrtScreen ─────────────────────────────────────────────────────
    {
        let send = |screen: &mut CrtScreen, text: &str| -> Result<(), Box<EvalAltResult>> {
            screen.ws.write(text).map_err(fail)
        };
        engine.register_fn("send", send);
        engine.register_fn("Send", send);
    }
    // CRT 的 WaitForString：命中 true、超时 false。核心 `read_until` 仍严格抛错，
    // 这里只吞 `Timeout` 一个变体；其它失败（会话结束、取消）照旧显式报错。
    {
        let wait = |screen: &mut CrtScreen, text: &str, secs: f64| -> Result<bool, Box<EvalAltResult>> {
            wait_for_bool(&screen.ws, text, secs)
        };
        let wait_1 = |screen: &mut CrtScreen, text: &str| -> Result<bool, Box<EvalAltResult>> {
            wait_for_bool(&screen.ws, text, DEFAULT_WAIT_SECS)
        };
        for name in ["wait_for_string", "WaitForString"] {
            engine.register_fn(name, wait);
            engine.register_fn(name, wait_1);
        }
    }
    // CRT 的 ReadString：命中返回文本、超时返回**空串**，而不是抛错。
    {
        let read = |screen: &mut CrtScreen, text: &str, secs: f64| -> Result<Dynamic, Box<EvalAltResult>> {
            read_string_value(&screen.ws, text, secs)
        };
        let read_1 = |screen: &mut CrtScreen, text: &str| -> Result<Dynamic, Box<EvalAltResult>> {
            read_string_value(&screen.ws, text, DEFAULT_WAIT_SECS)
        };
        for name in ["read_string", "ReadString"] {
            engine.register_fn(name, read);
            engine.register_fn(name, read_1);
        }
    }
    {
        let wait_many = |screen: &mut CrtScreen, patterns: Array, secs: f64| -> Result<Dynamic, Box<EvalAltResult>> {
            let list: Result<Vec<String>, _> = patterns
                .into_iter()
                .map(|v| v.into_string().map_err(|t| fail(format!("WaitForStrings 的项必须是字符串（实际 {t}）"))))
                .collect();
            let list = list?;
            let hit = screen.ws.wait_any(&list, secs, None).map_err(fail)?;
            Ok(Dynamic::from(hit.index as i64))
        };
        engine.register_fn("wait_for_strings", wait_many);
        engine.register_fn("WaitForStrings", wait_many);
    }
    {
        let rect = |screen: &mut CrtScreen, r1: i64, c1: i64, r2: i64, c2: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            let coords = checked_rect(r1, c1, r2, c2)?;
            screen
                .ws
                .region(coords.0, coords.1, coords.2, coords.3)
                .map(Dynamic::from)
                .map_err(fail)
        };
        engine.register_fn("get", rect);
        engine.register_fn("Get", rect);
    }
    {
        let lines = |screen: &mut CrtScreen, r1: i64, c1: i64, r2: i64, c2: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            let coords = checked_rect(r1, c1, r2, c2)?;
            screen
                .ws
                .region_lines(coords.0, coords.1, coords.2, coords.3)
                .map(Dynamic::from)
                .map_err(fail)
        };
        engine.register_fn("get2", lines);
        engine.register_fn("Get2", lines);
    }
    {
        let selection = |screen: &mut CrtScreen| -> Result<Dynamic, Box<EvalAltResult>> {
            screen.ws.selection().map(Dynamic::from).map_err(fail)
        };
        engine.register_fn("selection", selection);
        engine.register_fn("Selection", selection);
        engine.register_get("Selection", |screen: &mut CrtScreen| -> Result<String, Box<EvalAltResult>> {
            screen.ws.selection().map_err(fail)
        });
        engine.register_get("selection", |screen: &mut CrtScreen| -> Result<String, Box<EvalAltResult>> {
            screen.ws.selection().map_err(fail)
        });
    }
    {
        // `Screen.Synchronous = true` 是 CRT 的属性写法；旧的方法写法保留。
        // 读写都落在工作区那一位原子上，与全局 `set_synchronous(on)` 同源。
        let get = |screen: &mut CrtScreen| -> bool {
            screen.ws.synchronous().load(Ordering::Relaxed)
        };
        engine.register_get("synchronous", get);
        engine.register_get("Synchronous", get);
        let set = |screen: &mut CrtScreen, on: bool| {
            screen.ws.set_synchronous(on);
        };
        engine.register_set("synchronous", set);
        engine.register_set("Synchronous", set);
        let method = |screen: &mut CrtScreen, on: bool| -> Result<(), Box<EvalAltResult>> {
            screen.ws.set_synchronous(on);
            Ok(())
        };
        engine.register_fn("synchronous", method);
        engine.register_fn("Synchronous", method);
    }
    {
        let row = |screen: &mut CrtScreen| -> Result<i64, Box<EvalAltResult>> {
            screen.ws.cursor_pos().map(|(row, _)| row as i64 + 1).map_err(fail)
        };
        engine.register_fn("current_row", row);
        engine.register_fn("CurrentRow", row);
        engine.register_get("current_row", row);
        engine.register_get("CurrentRow", row);
    }
    {
        let column = |screen: &mut CrtScreen| -> Result<i64, Box<EvalAltResult>> {
            screen.ws.cursor_pos().map(|(_, col)| col as i64 + 1).map_err(fail)
        };
        engine.register_fn("current_column", column);
        engine.register_fn("CurrentColumn", column);
        engine.register_get("current_column", column);
        engine.register_get("CurrentColumn", column);
    }
    {
        let rows = |screen: &mut CrtScreen| -> Result<i64, Box<EvalAltResult>> {
            screen.ws.terminal_size().map(|(_, rows)| rows as i64).map_err(fail)
        };
        engine.register_fn("rows", rows);
        engine.register_fn("Rows", rows);
        engine.register_get("rows", rows);
        engine.register_get("Rows", rows);
    }
    {
        let cols = |screen: &mut CrtScreen| -> Result<i64, Box<EvalAltResult>> {
            screen.ws.terminal_size().map(|(cols, _)| cols as i64).map_err(fail)
        };
        engine.register_fn("columns", cols);
        engine.register_fn("Columns", cols);
        engine.register_get("columns", cols);
        engine.register_get("Columns", cols);
    }
    {
        let send_keys = |screen: &mut CrtScreen, expr: &str| -> Result<(), Box<EvalAltResult>> {
            screen.ws.send_keys(expr).map(|_| ()).map_err(fail)
        };
        engine.register_fn("send_keys", send_keys);
        engine.register_fn("SendKeys", send_keys);
    }

    // ── CrtSession ────────────────────────────────────────────────────
    {
        let connect = |session: &mut CrtSession, name: &str| -> Result<(), Box<EvalAltResult>> {
            session.ws.session_connect(name).map(|_| ()).map_err(fail)
        };
        engine.register_fn("connect", connect);
        engine.register_fn("Connect", connect);
    }
    {
        let connect_tab = |session: &mut CrtSession, name: &str| -> Result<Dynamic, Box<EvalAltResult>> {
            session
                .ws
                .session_connect(name)
                .map(|info| session_map(&info))
                .map_err(fail)
        };
        engine.register_fn("connect_in_tab", connect_tab);
        engine.register_fn("ConnectInTab", connect_tab);
    }
    {
        // 专用断连路径：CRT 的 `Session.Disconnect` 不是 `session_close`——后者
        // 明确拒绝关当前绑定（静默换目标最贵）。工作区先选好接棒者（没有就
        // 新开本地 shell），关掉旧会话后只改**这个句柄**的绑定，并显式返回
        // 新绑定名；钉死的标签不会把别的标签一起改绑。
        let disconnect = |session: &mut CrtSession| -> Result<Dynamic, Box<EvalAltResult>> {
            Ok(Dynamic::from(session.ws.disconnect_here().map_err(fail)?))
        };
        engine.register_fn("disconnect", disconnect);
        engine.register_fn("Disconnect", disconnect);
    }
    {
        let connected = |session: &mut CrtSession| -> Result<bool, Box<EvalAltResult>> {
            session
                .ws
                .state()
                .map(|state| matches!(state, term_core::session_runtime::SessionState::Alive))
                .map_err(fail)
        };
        engine.register_fn("connected", connected);
        engine.register_fn("Connected", connected);
    }
    {
        let log = |session: &mut CrtSession, enabled: bool, append: bool, raw: bool| -> Result<(), Box<EvalAltResult>> {
            if !enabled {
                session.ws.log_stop().map_err(fail)?;
                return Ok(());
            }
            let dir = session
                .ws
                .log_dir()
                .ok_or_else(|| fail("当前宿主没有默认日志目录：请用 log_start(dir) 自己指定"))?
                .display()
                .to_string();
            let opts = term_core::log_opts::LogOptions {
                overwrite: Some(!append),
                log_type: if raw {
                    term_core::log_opts::LogType::All
                } else {
                    term_core::log_opts::LogType::Printable
                },
                ..term_core::log_opts::LogOptions::default()
            };
            session
                .ws
                .log_start_opts(&dir, None, None, opts)
                .map_err(fail)
        };
        engine.register_fn("log", log);
        engine.register_fn("Log", log);
    }
    {
        // CRT 的 LogFileName 是**模板**（`%S` / `%H` / …）；存之前先翻成我们的
        // 花括号扩展，否则写进去的字面量永远不会展开。
        let set_name = |session: &mut CrtSession, name: &str| -> Result<(), Box<EvalAltResult>> {
            if session.ws.log_status().map(|s| s.is_recording()).unwrap_or(false) {
                return Err(fail("已经在记录日志：先 Log(False) 再改 LogFileName"));
            }
            let ours = term_core::log_opts::crt_template_to_ours(name);
            let opts = term_core::log_opts::LogOptions {
                file_name: ours.clone(),
                ..term_core::log_opts::LogOptions::default()
            };
            session.ws.remember_log_template(opts);
            if let Ok(mut slot) = session.log_template.lock() {
                *slot = Some(ours);
            }
            Ok(())
        };
        engine.register_fn("log_file_name", set_name);
        engine.register_fn("LogFileName", set_name);
        let get_name = |session: &mut CrtSession| -> Result<String, Box<EvalAltResult>> {
            match session.log_template.lock() {
                Ok(slot) => Ok(slot.clone().unwrap_or_default()),
                Err(_) => Err(fail("日志模板槽锁中毒")),
            }
        };
        engine.register_fn("get_log_file_name", get_name);
        engine.register_get("log_file_name", get_name);
        engine.register_get("LogFileName", get_name);
    }
    {
        let clone = |session: &mut CrtSession| -> Result<Dynamic, Box<EvalAltResult>> {
            let name = session.ws.current_name().map_err(fail)?;
            session
                .ws
                .session_clone(&name)
                .map(|info| session_map(&info))
                .map_err(fail)
        };
        engine.register_fn("clone", clone);
        engine.register_fn("Clone", clone);
    }
    {
        let sftp = |session: &mut CrtSession| -> Result<Dynamic, Box<EvalAltResult>> {
            // 句柄在能力面里是「每个 sftp_* 动词自己取」的；门面返回当前会话名，
            // 让脚本能把它当作后续 sftp_* 的第一个参数。
            Ok(Dynamic::from(session.ws.current_name().map_err(fail)?))
        };
        engine.register_fn("connect_sftp", sftp);
        engine.register_fn("ConnectSFTP", sftp);
    }

    // ── CrtDialog ─────────────────────────────────────────────────────
    {
        let simple = |dialog: &mut CrtDialog, text: &str, title: &str| -> Result<i64, Box<EvalAltResult>> {
            let answer = dialog
                .ws
                .ask_request(&Ask::Message {
                    title: title.to_string(),
                    text: text.to_string(),
                    icon: MessageIcon::None,
                    buttons: MessageButton::Ok,
                    default_button: Some("ok".to_string()),
                })
                .map_err(fail)?;
            button_id(answer, ButtonGroup::Ok)
        };
        engine.register_fn("message_box", simple);
        engine.register_fn("MessageBox", simple);
    }
    {
        let flagged = |dialog: &mut CrtDialog, text: &str, title: &str, flags: i64| -> Result<i64, Box<EvalAltResult>> {
            // CRT 把按钮组、图标、默认按钮组合成一个整数（按位或）；
            // group_from_id 只取低 4 位，其余位在这里分别解释。
            let group = group_from_id(flags).ok_or_else(|| fail(format!("不认识的按钮组编号 {flags}（可用 MB_OK / MB_OKCANCEL / MB_ABORTRETRYIGNORE / MB_YESNOCANCEL / MB_YESNO / MB_RETRYCANCEL）")))?;
            // 宿主的人机端口只实现了四组按钮；另两组必须显式报错，不能默默
            // 换成另一组（那会把 IDABORT/IDRETRY 变成 IDOK）。
            let buttons = group.message_button().ok_or_else(|| {
                fail(format!("当前宿主不支持 {} 按钮组（需要宿主的人机端口补齐这组按钮）", group.name()))
            })?;
            let icon = icon_from_id(flags).ok_or_else(|| fail(format!("不认识的图标编号 {flags}（可用 ICON_*）")))?;
            let default_button = default_button_from_id(flags, buttons);
            let answer = dialog
                .ws
                .ask_request(&Ask::Message {
                    title: title.to_string(),
                    text: text.to_string(),
                    icon,
                    buttons,
                    default_button,
                })
                .map_err(fail)?;
            button_id(answer, group)
        };
        engine.register_fn("message_box", flagged);
        engine.register_fn("MessageBox", flagged);
    }
    {
        let prompt = |dialog: &mut CrtDialog, prompt: &str, title: &str, default: &str, secret: bool| -> Result<Dynamic, Box<EvalAltResult>> {
            let ask = if secret {
                Ask::Secret {
                    title: title.to_string(),
                    prompt: prompt.to_string(),
                }
            } else {
                Ask::Text {
                    title: title.to_string(),
                    prompt: prompt.to_string(),
                    default: default.to_string(),
                }
            };
            match dialog.ws.ask_request(&ask).map_err(fail)? {
                crate::human::Answer::Text(text) => Ok(Dynamic::from(text)),
                _ => Err(fail("宿主对 Prompt 回的不是文本")),
            }
        };
        engine.register_fn("prompt", prompt);
        engine.register_fn("Prompt", prompt);
    }
    {
        let open = |dialog: &mut CrtDialog, title: &str, filter: &str| -> Result<Dynamic, Box<EvalAltResult>> {
            let picked = dialog
                .ws
                .pick_file(&PickFile::Open {
                    title: title.to_string(),
                    filter: filter.to_string(),
                    default: String::new(),
                })
                .map_err(fail)?;
            Ok(Dynamic::from(
                picked.map(|p| p.display().to_string()).unwrap_or_default(),
            ))
        };
        engine.register_fn("file_open_dialog", open);
        engine.register_fn("FileOpenDialog", open);
    }
    {
        let save = |dialog: &mut CrtDialog, title: &str, filter: &str| -> Result<Dynamic, Box<EvalAltResult>> {
            let picked = dialog
                .ws
                .pick_file(&PickFile::Save {
                    title: title.to_string(),
                    filter: filter.to_string(),
                    default: String::new(),
                })
                .map_err(fail)?;
            Ok(Dynamic::from(
                picked.map(|p| p.display().to_string()).unwrap_or_default(),
            ))
        };
        engine.register_fn("file_save_dialog", save);
        engine.register_fn("FileSaveDialog", save);
    }

    // ── CrtClipboard：`Text` 属性（读）+ `Set` 方法（写） ──────────────
    //
    // 读用属性（`crt.Clipboard.Text`，与手册一致）。写用方法：Rhai 的
    // `register_set` 只接受返回值恰好是 `()` 的闭包，而写入可能失败
    // （没有剪贴板 / 平台报错）——为了走属性而把失败吞掉，等于让脚本以为
    // 已经写好了。所以这一处形状有别：`crt.Clipboard.Set("x")`。
    {
        let text = |clip: &mut CrtClipboard| -> Result<String, Box<EvalAltResult>> {
            clip.ws.clipboard_get().map_err(fail)
        };
        engine.register_get("text", text);
        engine.register_get("Text", text);
    }
    {
        let set = |clip: &mut CrtClipboard, value: &str| -> Result<(), Box<EvalAltResult>> {
            clip.ws.clipboard_set(value).map_err(fail)
        };
        engine.register_fn("set", set);
        engine.register_fn("Set", set);
    }

    // ── CrtWindow：`Show` / `Hide` 方法 ──────────────────────────────
    // 同样理由：`Window.Visible = false` 要能失败，所以用两个方法表达。
    {
        let hide = |window: &mut CrtWindow| -> Result<(), Box<EvalAltResult>> {
            window.ws.set_window_visible(false).map_err(fail)
        };
        engine.register_fn("hide", hide);
        engine.register_fn("Hide", hide);
    }
    {
        let show = |window: &mut CrtWindow| -> Result<(), Box<EvalAltResult>> {
            window.ws.set_window_visible(true).map_err(fail)
        };
        engine.register_fn("show", show);
        engine.register_fn("Show", show);
    }

    // ── MessageBox 常量（CRT / Win32 的按位标志） ──────────────────────
    //
    // 按钮在低 4 位，图标在 0x70，默认按钮在 0x300；三者可按位或组合成一个
    // 整数传给 `MessageBox(text, title, flags)`。
    for (name, value) in [
        // 输入标志：Win32/CRT 的按钮组编号，**不是**返回值。
        ("MB_OK", 0_i64),
        ("MB_OKCANCEL", 1),
        ("MB_ABORTRETRYIGNORE", 2),
        ("MB_YESNOCANCEL", 3),
        ("MB_YESNO", 4),
        ("MB_RETRYCANCEL", 5),
        // BUTTON_* 是老脚本里输入标志的别名，跟着 MB_* 一起改。
        ("BUTTON_OK", 0),
        ("BUTTON_OKCANCEL", 1),
        ("BUTTON_YESNO", 4),
        ("BUTTON_YESNOCANCEL", 3),
        ("BUTTON_CANCEL", 2), // 历史遗留的 IDCANCEL；当输入标志用时同样显式报错。
        // 图标位（MB_ICON*）与默认按钮位（MB_DEFBUTTON*）。
        ("ICON_NONE", 0),
        ("ICON_STOP", 16),
        ("ICON_CRITICAL", 16),
        ("ICON_QUESTION", 32),
        ("ICON_EXCLAMATION", 48),
        ("ICON_WARNING", 48),
        ("ICON_INFORMATION", 64),
        ("ICON_INFO", 64),
        ("DEFBUTTON1", 0),
        ("DEFBUTTON2", 256),
        ("DEFBUTTON3", 512),
        // 返回 ID：MessageBox 的返回值，**不是**输入标志。
        ("IDOK", 1),
        ("IDCANCEL", 2),
        ("IDABORT", 3),
        ("IDRETRY", 4),
        ("IDIGNORE", 5),
        ("IDYES", 6),
        ("IDNO", 7),
    ] {
        engine.register_fn(name, move || value);
    }
}

fn checked_rect(
    r1: i64,
    c1: i64,
    r2: i64,
    c2: i64,
) -> Result<(usize, usize, usize, usize), Box<EvalAltResult>> {
    if [r1, c1, r2, c2].iter().any(|v| *v < 1) {
        return Err(fail("Screen.Get 的坐标从 1 起（0 或负数无效）"));
    }
    Ok((r1 as usize, c1 as usize, r2 as usize, c2 as usize))
}

/// 把一条会话钉进标签对象。共享槽跟着句柄一起走，`tab.screen` 才认得
/// `crt().Screen.Synchronous` 设过的开关与 LogFileName 模板。
fn make_tab(crt: &Crt, info: SessionInfo) -> Result<CrtTab, Box<EvalAltResult>> {
    Ok(CrtTab {
        ws: crt.ws.scoped_to(info.id).map_err(fail)?,
        name: info.name,
        log_template: Arc::clone(&crt.log_template),
    })
}

/// CRT `WaitForString`：命中 true / 超时 false。只吞 `Timeout`，其它错误照旧抛。
fn wait_for_bool(
    ws: &ScriptWorkspace,
    text: &str,
    secs: f64,
) -> Result<bool, Box<EvalAltResult>> {
    match ws.read_until(text, secs, None) {
        Ok(_) => Ok(true),
        Err(PrimitiveError::Timeout) => Ok(false),
        Err(other) => Err(fail(other)),
    }
}

/// CRT `ReadString`：命中返回文本 / 超时返回**空串**。核心 `read_until` 的
/// 严格抛错不变；与 `WaitForString` 的布尔假值不要混成同一个返回值。
fn read_string_value(
    ws: &ScriptWorkspace,
    text: &str,
    secs: f64,
) -> Result<Dynamic, Box<EvalAltResult>> {
    match ws.read_until(text, secs, None) {
        Ok(text) => Ok(Dynamic::from(text)),
        Err(PrimitiveError::Timeout) => Ok(Dynamic::from("")),
        Err(other) => Err(fail(other)),
    }
}

/// `MB_*` 输入的按钮组（低 4 位）：六个 Win32/CRT 组全部认识并支持。
///
/// 宿主的人机端口已经补齐 abort/retry/ignore 与 retry/cancel 两组，
/// 每组都按 `Answer::Text(标签)` 回传具体按钮，由 `button_id` 映射到 CRT ID。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ButtonGroup {
    Ok,
    OkCancel,
    AbortRetryIgnore,
    YesNoCancel,
    YesNo,
    RetryCancel,
}

impl ButtonGroup {
    fn name(self) -> &'static str {
        match self {
            Self::Ok => "MB_OK",
            Self::OkCancel => "MB_OKCANCEL",
            Self::AbortRetryIgnore => "MB_ABORTRETRYIGNORE",
            Self::YesNoCancel => "MB_YESNOCANCEL",
            Self::YesNo => "MB_YESNO",
            Self::RetryCancel => "MB_RETRYCANCEL",
        }
    }

    /// 六组按钮宿主都能画出来；返回 `None` 只可能是新增了未登记的组。
    fn message_button(self) -> Option<MessageButton> {
        match self {
            Self::Ok => Some(MessageButton::Ok),
            Self::OkCancel => Some(MessageButton::OkCancel),
            Self::YesNoCancel => Some(MessageButton::YesNoCancel),
            Self::YesNo => Some(MessageButton::YesNo),
            Self::AbortRetryIgnore => Some(MessageButton::AbortRetryIgnore),
            Self::RetryCancel => Some(MessageButton::RetryCancel),
        }
    }

    /// 宿主用 `Answer::Bool` 作答时（假宿主与部分 CLI 会这样），肯定/否定
    /// 分别落到这组的第一个/最后一个按钮上。
    fn affirmative(self) -> &'static str {
        match self {
            Self::Ok | Self::OkCancel => "ok",
            Self::YesNo | Self::YesNoCancel => "yes",
            Self::AbortRetryIgnore => "abort",
            Self::RetryCancel => "retry",
        }
    }

    fn negative(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::OkCancel | Self::YesNoCancel | Self::RetryCancel => "cancel",
            Self::YesNo => "no",
            Self::AbortRetryIgnore => "ignore",
        }
    }

    /// 按钮标签 → CRT 返回 ID。
    fn id(self, label: &str) -> Option<i64> {
        match (self, label) {
            (Self::Ok, "ok") => Some(1), // IDOK
            (Self::OkCancel, "ok") => Some(1),
            (Self::OkCancel, "cancel") => Some(2), // IDCANCEL
            (Self::YesNo, "yes") => Some(6),       // IDYES
            (Self::YesNo, "no") => Some(7),        // IDNO
            (Self::YesNoCancel, "yes") => Some(6),
            (Self::YesNoCancel, "no") => Some(7),
            (Self::YesNoCancel, "cancel") => Some(2),
            (Self::AbortRetryIgnore, "abort") => Some(3), // IDABORT
            (Self::AbortRetryIgnore, "retry") => Some(4),  // IDRETRY
            (Self::AbortRetryIgnore, "ignore") => Some(5), // IDIGNORE
            (Self::RetryCancel, "retry") => Some(4),
            (Self::RetryCancel, "cancel") => Some(2),
            _ => None,
        }
    }
}

/// 只取低 4 位的按钮组；图标位与默认按钮位由另两个函数解释。这样
/// `MB_YESNO | ICON_QUESTION | DEFBUTTON2` 这样的组合号也能被接受。
fn group_from_id(id: i64) -> Option<ButtonGroup> {
    match id & 0x0F {
        0 => Some(ButtonGroup::Ok),
        1 => Some(ButtonGroup::OkCancel),
        2 => Some(ButtonGroup::AbortRetryIgnore),
        3 => Some(ButtonGroup::YesNoCancel),
        4 => Some(ButtonGroup::YesNo),
        5 => Some(ButtonGroup::RetryCancel),
        _ => None,
    }
}

fn icon_from_id(id: i64) -> Option<MessageIcon> {
    match id & 0x70 {
        0 => Some(MessageIcon::None),
        16 => Some(MessageIcon::Error),
        32 => Some(MessageIcon::Question),
        48 => Some(MessageIcon::Warning),
        64 => Some(MessageIcon::Info),
        _ => None,
    }
}

fn default_button_from_id(id: i64, buttons: MessageButton) -> Option<String> {
    let which = match id & 0x300 {
        0 => 0,
        256 => 1,
        512 => 2,
        _ => return None,
    };
    buttons.labels().get(which).map(|label| (*label).to_string())
}

/// 按钮答案 → CRT 的整数 ID。答非所问一律显式报错，**不猜 IDOK**。
fn button_id(
    answer: crate::human::Answer,
    group: ButtonGroup,
) -> Result<i64, Box<EvalAltResult>> {
    let label = match answer {
        crate::human::Answer::Text(text) => text,
        crate::human::Answer::Bool(true) => group.affirmative().to_string(),
        crate::human::Answer::Bool(false) => group.negative().to_string(),
        crate::human::Answer::Ack => "ok".to_string(),
        crate::human::Answer::Choice(_) => {
            return Err(fail("消息框收到了 Choice 序号，不是按钮答案"));
        }
    };
    group.id(&label).ok_or_else(|| {
        fail(format!(
            "宿主对 {} 消息框回了 {:?}，这一组按钮里没有这个答案（不猜 IDOK）",
            group.name(),
            label
        ))
    })
}

/// 把 `PrimitiveError` 透出成脚本错误（门面内部用）。
fn _primitive(e: PrimitiveError) -> Box<EvalAltResult> {
    fail(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FakeSessions;
    use crate::{Sessions, ScriptEngine, ScriptWorkspace, SandboxConfig, HostEnv};
    use std::sync::Arc;
    use std::time::Duration;

    /// 造一个带门面的引擎 + 假会话集合。返回 (引擎, 工作区, 假集合)。
    fn facade() -> (Engine, ScriptWorkspace, Arc<FakeSessions>) {
        let fake = FakeSessions::new();
        fake.mount(1, "one");
        fake.mount(2, "two");
        let sessions: Arc<dyn Sessions> = fake.clone();
        let current = sessions.attach(1).expect("会话 1");
        let ws = ScriptWorkspace::new(fake.env(), current, Duration::from_secs(2));
        let mut engine = Engine::new();
        engine.set_max_operations(100_000);
        // 门面里的动词直接调工作区方法，不需要先注册原语；但 `crt.Screen.Send`
        // 之外的手册例子会混着用全局函数，所以两边都注册上。
        ScriptEngine::register_primitives(&mut engine, ws.clone());
        register(&mut engine, ws.clone());
        (engine, ws, fake)
    }

    fn run(engine: &Engine, script: &str) -> Result<Dynamic, Box<EvalAltResult>> {
        let mut scope = rhai::Scope::new();
        engine.eval_ast_with_scope::<Dynamic>(
            &mut scope,
            &engine.compile(script).expect("语法正确"),
        )
    }

    /// 带窗口语义的假集合：`Activate` 在裸 FakeSessions 上是默认 Unsupported，
    /// 冒烟测试要验 `Tab.Activate()` 能真的走到宿主，所以补一份激活事实。
    struct WindowSessions {
        inner: Arc<FakeSessions>,
        active: Arc<Mutex<Option<term_core::session_runtime::SessionId>>>,
    }

    impl Sessions for WindowSessions {
        fn list(&self) -> Vec<SessionInfo> {
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
        ) -> Result<SessionInfo, crate::sessions::SessionsError> {
            self.inner.open(request)
        }
        fn close(&self, name: &str) -> Result<(), crate::sessions::SessionsError> {
            self.inner.close(name)
        }
        fn attach(
            &self,
            id: term_core::session_runtime::SessionId,
        ) -> Option<Arc<Mutex<dyn term_core::session_runtime::Session>>> {
            self.inner.attach(id)
        }
        fn activate(
            &self,
            id: term_core::session_runtime::SessionId,
        ) -> Result<(), crate::sessions::SessionsError> {
            *self.active.lock().expect("激活槽") = Some(id);
            Ok(())
        }
        fn active(&self) -> Option<term_core::session_runtime::SessionId> {
            *self.active.lock().expect("激活槽")
        }
    }

    fn window_facade() -> (Engine, Arc<Mutex<Option<term_core::session_runtime::SessionId>>>) {
        let fake = FakeSessions::new();
        fake.mount(1, "one");
        fake.mount(2, "two");
        let active = Arc::new(Mutex::new(None));
        let sessions: Arc<dyn Sessions> = Arc::new(WindowSessions {
            inner: fake.clone(),
            active: Arc::clone(&active),
        });
        let current = sessions.attach(1).expect("会话 1");
        let ws = ScriptWorkspace::new(HostEnv::new(sessions), current, Duration::from_secs(2));
        let mut engine = Engine::new();
        register(&mut engine, ws);
        (engine, active)
    }

    #[test]
    fn send_forwards_to_write_and_wait_for_string_is_consuming() {
        let (engine, _ws, fake) = facade();
        let _ = run(&engine, r#"crt().screen.send("ls\n")"#).expect("发送");
        assert_eq!(
            String::from_utf8_lossy(&fake.manip(1).expect("会话").written()),
            "ls\n"
        );
        // WaitForString 走 read_until（消费式）：推一段含哨兵的输出即可命中，
        // 门面按 CRT 语义返回 true（核心 read_until 仍严格抛错）。
        fake.manip(1).expect("会话").push_output("banner$ ");
        let hit = run(&engine, r#"crt().screen.wait_for_string("$", 1.0)"#).expect("命中");
        assert!(hit.as_bool().expect("布尔"), "命中应为 true");
        // 哨兵后面的空格留在缓冲区里，下一次读还能拿到——消费停在命中点。
        let tail = run(&engine, r#"crt().screen.read_string(" ", 0.1)"#).expect("尾段");
        assert_eq!(tail.to_string(), " ");
    }

    #[test]
    fn get_uses_one_based_coordinates_and_returns_the_rectangle() {
        let (engine, _ws, fake) = facade();
        fake.manip(1).expect("会话").set_screen("abc\ndef");
        let text = run(&engine, r#"crt().screen.get(1, 1, 2, 2)"#).expect("取区域");
        assert_eq!(text.to_string(), "ab\r\nde");
        // Get2 用 \n 分隔。
        let text = run(&engine, r#"crt().screen.get2(1, 1, 2, 2)"#).expect("取区域");
        assert_eq!(text.to_string(), "ab\nde");
    }

    #[test]
    fn tabs_are_one_based_and_screen_of_a_tab_reaches_that_session() {
        let (engine, ws, fake) = facade();
        let name = run(&engine, r#"crt().get_tab(2).name"#).expect("第二条");
        assert_eq!(name.to_string(), "two");
        let _ = run(&engine, r#"crt().get_tab(2).screen.send("hi\n")"#).expect("发给第二条");
        assert_eq!(
            String::from_utf8_lossy(&fake.manip(2).expect("会话").written()),
            "hi\n"
        );
        // 绑定没被门面改动（GetTab 只借对象，不改当前绑定）。
        assert_eq!(ws.current_name().expect("绑定"), "one");
    }

    #[test]
    fn get_script_tab_stays_pinned_to_the_launch_session() {
        let (engine, ws, fake) = facade();
        ws.session_use("two").expect("全局改绑");
        assert_eq!(
            run(&engine, "crt().GetScriptTab().Name")
                .expect("启动标签")
                .to_string(),
            "one",
            "GetScriptTab 不随 session_use 改绑"
        );
        let _ = run(&engine, r#"crt().GetScriptTab().Screen.Send("launch\n")"#)
            .expect("发到启动标签");
        assert_eq!(
            String::from_utf8_lossy(&fake.manip(1).expect("会话 1").written()),
            "launch\n"
        );
        assert!(fake.manip(2).expect("会话 2").written().is_empty());
    }

    #[test]
    fn scoped_disconnect_does_not_touch_another_tabs_binding() {
        let (engine, ws, fake) = facade();
        let next = run(
            &engine,
            r#"let t = crt().GetTab(2); t.Session.Disconnect()"#,
        )
        .expect("钉死的 Disconnect");
        assert_eq!(next.to_string(), "one", "显式返回接棒的绑定名");
        assert_eq!(ws.current_name().expect("全局绑定"), "one", "另一条标签不被改绑");
        assert!(fake.manip(1).is_some(), "one 仍活着");
        assert!(fake.manip(2).is_none(), "钉住的 two 被关掉");
    }

    #[test]
    fn scoped_connect_in_tab_rebinds_only_that_tab() {
        let (engine, ws, fake) = facade();
        let _ = run(
            &engine,
            r#"let t = crt().GetTab(2); t.Session.ConnectInTab("fresh"); t.Screen.Send("scoped\n")"#,
        )
        .expect("钉死的 ConnectInTab");
        assert_eq!(ws.current_name().expect("全局绑定"), "one", "one 的绑定不动");
        assert!(fake.manip(2).is_some(), "two 没有被关掉");
        let fresh_id = *fake.ids().last().expect("新开的会话");
        assert!(fake.manip(fresh_id).is_some(), "新会话在册");
        assert_eq!(
            String::from_utf8_lossy(&fake.manip(1).expect("one").written()),
            "",
            "发命令不能落到 one"
        );
        assert_eq!(
            String::from_utf8_lossy(
                &fake
                    .manip(fresh_id)
                    .expect("新会话")
                    .written()
            ),
            "scoped\n",
            "写跟随 scope 改绑到新会话"
        );
    }

    #[test]
    fn synchronous_is_one_atomic_shared_by_property_and_global_verb() {
        let (engine, ws, _fake) = facade();
        let _ = run(&engine, "set_synchronous(true)").expect("全局动词写");
        assert!(ws.synchronous().load(Ordering::Relaxed), "全局写落到唯一真源");
        let on = run(&engine, "crt().Screen.Synchronous").expect("属性读");
        assert!(on.as_bool().expect("布尔"), "属性读同一位");

        let _ = run(&engine, "let c = crt(); c.Screen.Synchronous = false;").expect("属性写");
        assert!(!ws.synchronous().load(Ordering::Relaxed), "属性写也落到同一位");
        let off = run(&engine, "set_synchronous(false)").expect("全局动词读回");
        assert!(!off.as_bool().expect("布尔"));
    }

    #[test]
    fn get_tab_index_and_count_share_one_tab_bar_order() {
        let fake = FakeSessions::new();
        // 故意让登记序（id 1, 2）与标签栏的名字序相反。
        fake.mount(1, "zulu");
        fake.mount(2, "alpha");
        let sessions: Arc<dyn Sessions> = fake.clone();
        let current = sessions.attach(1).expect("会话 1");
        let ws = ScriptWorkspace::new(fake.env(), current, Duration::from_secs(2));
        let mut engine = Engine::new();
        register(&mut engine, ws);
        assert_eq!(
            run(&engine, "crt().GetTabCount()")
                .expect("数量")
                .as_int()
                .expect("整数"),
            2
        );
        assert_eq!(
            run(&engine, "crt().GetTab(1).Name").expect("第一标签").to_string(),
            "alpha",
            "GetTab 按名字序，不按 id 序"
        );
        assert_eq!(
            run(&engine, "crt().GetTab(2).Name").expect("第二标签").to_string(),
            "zulu"
        );
        assert_eq!(
            run(&engine, "crt().GetTab(1).Index()")
                .expect("Index 方法")
                .as_int()
                .expect("整数"),
            1,
            "GetTab(n).Index() == n"
        );
        assert_eq!(
            run(&engine, "crt().GetTab(2).Index")
                .expect("Index 属性")
                .as_int()
                .expect("整数"),
            2
        );
    }

    #[test]
    fn screen_and_tab_properties_work_without_parentheses() {
        let (engine, _ws, fake) = facade();
        let manip = fake.manip(1).expect("会话");
        manip.set_cursor(2, 3);
        manip.resize(90, 30);
        let property = |script: &str| -> i64 {
            run(&engine, script)
                .expect("属性")
                .as_int()
                .expect("整数")
        };
        assert_eq!(property("crt().Screen.CurrentRow"), 3, "行 1 起");
        assert_eq!(property("crt().Screen.CurrentColumn"), 4, "列 1 起");
        assert_eq!(property("crt().Screen.Rows"), 30);
        assert_eq!(property("crt().Screen.Columns"), 90);
        assert_eq!(property("crt().GetTab(1).Index"), 1);
        // 方法形式仍在（兼容老脚本）。
        assert_eq!(property("crt().Screen.CurrentRow()"), 3);
    }

    #[test]
    fn pascal_case_file_dialogs_are_registered() {
        let (engine, _ws, fake) = facade();
        fake.pick(Some("/tmp/open.txt"));
        fake.pick(Some("/tmp/save.txt"));
        assert_eq!(
            run(&engine, r#"crt().Dialog.FileOpenDialog("t", "*")"#)
                .expect("打开")
                .to_string(),
            "/tmp/open.txt"
        );
        assert_eq!(
            run(&engine, r#"crt().Dialog.FileSaveDialog("t", "*")"#)
                .expect("保存")
                .to_string(),
            "/tmp/save.txt"
        );
    }

    #[test]
    fn arguments_are_zero_based_and_out_of_range_is_empty() {
        let fake = FakeSessions::new();
        fake.mount(1, "one");
        let sessions: Arc<dyn Sessions> = fake.clone();
        let current = sessions.attach(1).expect("会话");
        let env = HostEnv::new(sessions).with_context(Arc::new(crate::ScriptContext::new(
            Some("/tmp/demo.rhai".to_string()),
            vec!["a".to_string(), "b".to_string()],
        )));
        let ws = ScriptWorkspace::new(env, current, Duration::from_secs(2));
        let mut engine = Engine::new();
        register(&mut engine, ws.clone());
        assert_eq!(
            run(&engine, "crt().argument(1)").expect("第二个参数").to_string(),
            "b"
        );
        assert_eq!(
            run(&engine, "crt().argument(9)").expect("越界给空串").to_string(),
            ""
        );
        assert_eq!(
            run(&engine, "crt().Arguments(1)").expect("CRT 手册形状").to_string(),
            "b"
        );
        assert_eq!(
            run(&engine, "crt().script_full_name()").expect("路径").to_string(),
            "/tmp/demo.rhai"
        );
    }

    #[test]
    fn message_box_maps_button_labels_back_to_crt_ids() {
        let (engine, _ws, fake) = facade();
        fake.answer("yes");
        let id = run(
            &engine,
            r#"crt().dialog.message_box("continue?", "t", MB_YESNO())"#,
        )
        .expect("问");
        assert_eq!(id.as_int().expect("整数"), 6, "yes → IDYES=6");
        // 输入标志（4）与返回 ID（6）是两个不同的数，常量必须分得清。
        assert_eq!(
            run(&engine, "MB_YESNO()").expect("常量").as_int().expect("整数"),
            4
        );
        assert_eq!(
            run(&engine, "IDYES()").expect("返回 ID").as_int().expect("整数"),
            6
        );
    }

    #[test]
    fn wait_for_string_and_read_string_return_crt_values_on_timeout() {
        let (engine, _ws, _fake) = facade();
        let hit = run(&engine, r#"crt().Screen.WaitForString("never", 0.01)"#)
            .expect("CRT 超时返回 false，不抛错");
        assert_eq!(hit.as_bool().expect("布尔"), false, "WaitForString → bool");
        let value = run(&engine, r#"crt().Screen.ReadString("never", 0.01)"#)
            .expect("CRT 超时返回空串，不抛错");
        assert_eq!(value.to_string(), "", "ReadString → 空串，不是整数 0");
    }

    #[test]
    fn read_string_timeout_is_an_empty_string_not_a_number() {
        let (engine, _ws, _fake) = facade();
        let value = run(&engine, r#"crt().Screen.ReadString("never", 0.01)"#)
            .expect("超时不抛错");
        assert!(value.is_string(), "空串是字符串，不能是数字 0");
        assert_eq!(value.as_string().expect("字符串"), "");
    }

    #[test]
    fn one_argument_wait_overload_hits_with_the_default_timeout() {
        let (engine, _ws, fake) = facade();
        fake.manip(1).expect("会话").push_output("ready$");
        let hit = run(&engine, r#"crt().Screen.WaitForString("$")"#).expect("1 参重载");
        assert!(hit.as_bool().expect("布尔"));
    }

    #[test]
    fn pascal_case_shapes_smoke_test_the_whole_facade() {
        let (engine, active) = window_facade();
        let _ = run(&engine, r#"crt().Screen.Send("hi\n")"#).expect("PascalCase Send");
        assert_eq!(
            run(&engine, "crt().GetTab(1).Index()")
                .expect("PascalCase GetTab / Index")
                .as_int()
                .expect("整数"),
            1
        );
        let _ = run(&engine, "crt().GetTab(1).Activate()").expect("Tab.Activate");
        assert!(active.lock().expect("激活槽").is_some(), "Activate 到了宿主");
        assert_eq!(
            run(&engine, "crt().GetScriptTab().Name")
                .expect("GetScriptTab")
                .to_string(),
            "one"
        );
        assert_eq!(
            run(&engine, "crt().Arguments.Count")
                .expect("Arguments.Count")
                .as_int()
                .expect("整数"),
            0
        );
        let _ = run(&engine, "let c = crt(); c.Screen.Synchronous = true;")
            .expect("属性写入");
        let on = run(&engine, "crt().Screen.Synchronous").expect("属性读回");
        assert!(on.as_bool().expect("布尔"), "Synchronous 是可读写的属性");
        // 小写形态必须还在。
        let _ = run(&engine, r#"crt().screen.send("x")"#).expect("小写 send");
    }

    #[test]
    fn log_file_name_is_translated_and_reads_back() {
        let (engine, _ws, _fake) = facade();
        let out = run(
            &engine,
            r#"let c = crt(); c.Session.LogFileName("%S-%H.log"); c.Session.LogFileName"#,
        )
        .expect("模板写后可读回");
        assert_eq!(out.to_string(), "%{session}-%{host}.log", "CRT 模板被翻译");
    }

    #[test]
    fn get_last_error_reads_the_shared_context() {
        let (engine, ws, _fake) = facade();
        ws.env().context.record_error("第 2 行：等待超时");
        let msg = run(&engine, "crt().GetLastErrorMessage()").expect("读错误");
        assert!(msg.to_string().contains("第 2 行"));
    }

    #[test]
    fn disconnect_rebinds_to_another_session_and_returns_its_name() {
        let (engine, ws, fake) = facade();
        let name = run(&engine, "crt().Session.Disconnect()").expect("断连要成功");
        assert_eq!(name.to_string(), "two", "显式返回新绑定名");
        assert_eq!(ws.current_name().expect("新绑定"), "two");
        assert!(fake.manip(1).is_none(), "旧会话已关闭");
    }

    #[test]
    fn message_box_accepts_combined_crt_flags() {
        let (engine, _ws, fake) = facade();
        fake.answer("no");
        let id = run(
            &engine,
            r#"crt().Dialog.MessageBox("q", "t", MB_YESNO() | ICON_QUESTION() | DEFBUTTON2())"#,
        )
        .expect("组合标志");
        assert_eq!(id.as_int().expect("整数"), 7, "no → IDNO=7");
    }

    #[test]
    fn message_box_input_flags_use_win32_values_and_return_ids_are_separate() {
        let (engine, _ws, _fake) = facade();
        let value = |name: &str| {
            run(&engine, &format!("{name}()"))
                .expect("常量")
                .as_int()
                .expect("整数")
        };
        // 输入标志：0..5，不是老代码里误用的返回 ID 1/2/6/7。
        assert_eq!(value("MB_OK"), 0);
        assert_eq!(value("MB_OKCANCEL"), 1);
        assert_eq!(value("MB_ABORTRETRYIGNORE"), 2);
        assert_eq!(value("MB_YESNOCANCEL"), 3);
        assert_eq!(value("MB_YESNO"), 4);
        assert_eq!(value("MB_RETRYCANCEL"), 5);
        // 图标位与默认按钮位。
        assert_eq!(value("ICON_STOP"), 16);
        assert_eq!(value("ICON_QUESTION"), 32);
        assert_eq!(value("ICON_EXCLAMATION"), 48);
        assert_eq!(value("ICON_INFORMATION"), 64);
        assert_eq!(value("DEFBUTTON1"), 0);
        assert_eq!(value("DEFBUTTON2"), 256);
        assert_eq!(value("DEFBUTTON3"), 512);
        // 返回 ID：1..7。
        assert_eq!(value("IDOK"), 1);
        assert_eq!(value("IDCANCEL"), 2);
        assert_eq!(value("IDABORT"), 3);
        assert_eq!(value("IDRETRY"), 4);
        assert_eq!(value("IDIGNORE"), 5);
        assert_eq!(value("IDYES"), 6);
        assert_eq!(value("IDNO"), 7);
    }

    #[test]
    fn each_supported_button_group_maps_answers_to_crt_ids() {
        let (engine, _ws, fake) = facade();
        let ask = |flags: &str, answer: &str| -> i64 {
            fake.answer(answer);
            run(
                &engine,
                &format!(r#"crt().Dialog.MessageBox("q", "t", {flags})"#),
            )
            .expect("问")
            .as_int()
            .expect("整数")
        };
        fake.answer("ok");
        assert_eq!(
            run(&engine, r#"crt().Dialog.MessageBox("hi", "t")"#)
                .expect("两参形态")
                .as_int()
                .expect("整数"),
            1,
            "两参消息框只有一个 IDOK"
        );
        assert_eq!(ask("MB_OK()", "ok"), 1, "MB_OK → IDOK");
        assert_eq!(ask("MB_OKCANCEL()", "ok"), 1, "MB_OKCANCEL → IDOK");
        assert_eq!(ask("MB_OKCANCEL()", "cancel"), 2, "→ IDCANCEL");
        assert_eq!(ask("MB_YESNO()", "yes"), 6, "MB_YESNO → IDYES");
        assert_eq!(ask("MB_YESNO()", "no"), 7, "→ IDNO");
        assert_eq!(ask("MB_YESNOCANCEL()", "yes"), 6, "MB_YESNOCANCEL → IDYES");
        assert_eq!(ask("MB_YESNOCANCEL()", "no"), 7, "→ IDNO");
        assert_eq!(ask("MB_YESNOCANCEL()", "cancel"), 2, "→ IDCANCEL（不再是 IDNO）");
        // 以前两组「宿主不支持」现在也通了：三个按钮各自映射到独立 CRT ID。
        assert_eq!(ask("MB_ABORTRETRYIGNORE()", "abort"), 3, "→ IDABORT");
        assert_eq!(ask("MB_ABORTRETRYIGNORE()", "retry"), 4, "→ IDRETRY");
        assert_eq!(ask("MB_ABORTRETRYIGNORE()", "ignore"), 5, "→ IDIGNORE");
        assert_eq!(ask("MB_RETRYCANCEL()", "retry"), 4, "→ IDRETRY");
        assert_eq!(ask("MB_RETRYCANCEL()", "cancel"), 2, "→ IDCANCEL");
    }

    #[test]
    fn every_mb_group_has_a_host_button_set() {
        for group in [
            ButtonGroup::Ok,
            ButtonGroup::OkCancel,
            ButtonGroup::YesNo,
            ButtonGroup::YesNoCancel,
            ButtonGroup::AbortRetryIgnore,
            ButtonGroup::RetryCancel,
        ] {
            assert!(
                group.message_button().is_some(),
                "{} 没有宿主按钮组",
                group.name()
            );
            if let Some(buttons) = group.message_button() {
                for label in buttons.labels() {
                    assert!(group.id(label).is_some(), "{} 应含 {label}", group.name());
                }
            }
        }
    }

    #[test]
    fn an_unexpected_message_box_answer_is_not_mapped_to_idok() {
        use crate::human::Answer;
        assert!(
            button_id(Answer::Text("maybe".to_string()), ButtonGroup::YesNo).is_err(),
            "不在按钮组里的答案必须报错"
        );
        assert!(
            button_id(Answer::Ack, ButtonGroup::YesNo).is_err(),
            "Ack 不是 YesNo 组的答案"
        );
        assert!(
            button_id(Answer::Choice(1), ButtonGroup::OkCancel).is_err(),
            "Choice 不是按钮答案"
        );
        assert_eq!(
            button_id(
                Answer::Text("abort".to_string()),
                ButtonGroup::AbortRetryIgnore
            )
            .expect("组本身可译"),
            3,
            "IDABORT 仍是 3"
        );
        assert_eq!(
            button_id(Answer::Text("no".to_string()), ButtonGroup::YesNoCancel)
                .expect("三按钮组的 no"),
            7,
            "MB_YESNOCANCEL 的 no → IDNO"
        );
    }

    #[test]
    fn the_facade_members_are_registered_under_their_crt_names() {
        // 这不是「函数存在」的检查，而是「名字拼写与手册一致」的检查：
        // 少一个名字，照着文档改写的脚本就会在那一行报 Function not found。
        let (engine, _ws, _fake) = facade();
        let sigs: Vec<String> = engine.gen_fn_signatures(false);
        for expected in [
            "crt()",
            "screen",
            "session",
            "dialog",
            "clipboard",
            "window",
            "version(",
            "script_full_name(",
            "arguments(",
            "argument(",
            "get_tab(",
            "get_tab_count(",
            "get_active_tab(",
            "send(",
            "wait_for_string(",
            "read_string(",
            "wait_for_strings(",
            "get(",
            "get2(",
            "selection(",
            "synchronous(",
            "current_row(",
            "current_column(",
            "rows(",
            "columns(",
            "send_keys(",
            "connect(",
            "connect_in_tab(",
            "disconnect(",
            "connected(",
            "log(",
            "log_file_name(",
            "clone(",
            "connect_sftp(",
            "message_box(",
            "prompt(",
            "file_open_dialog(",
            "file_save_dialog(",
            "FileOpenDialog(",
            "FileSaveDialog(",
            "text",
            "set(",
            "show(",
            "hide(",
            // PascalCase 手册形状：少了任何一个，照着文档改写的脚本都会在
            // 那一行报 Function not found。
            "Screen",
            "Session",
            "Dialog",
            "Clipboard",
            "Window",
            "Version(",
            "ScriptFullName(",
            "Arguments(",
            "Argument(",
            "GetTab(",
            "GetTabCount(",
            "GetActiveTab(",
            "GetScriptTab(",
            "GetLastErrorMessage(",
            "Send(",
            "WaitForString(",
            "ReadString(",
            "WaitForStrings(",
            "Get(",
            "Get2(",
            "Selection(",
            "Synchronous",
            "CurrentRow(",
            "CurrentColumn(",
            "Rows(",
            "Columns(",
            "SendKeys(",
            "Connect(",
            "Disconnect(",
            "Connected(",
            "Log(",
            "LogFileName",
            "Clone(",
            "MessageBox(",
            "Prompt(",
            "Count",
            "Set(",
            "Show(",
            "Hide(",
            "Activate(",
            // 属性形状（同时保留方法形式）。
            "Index",
            "CurrentRow",
            "CurrentColumn",
            "Rows",
            "Columns",
            // 输入标志与返回 ID 是两套常量。
            "MB_YESNO(",
            "MB_ABORTRETRYIGNORE(",
            "MB_RETRYCANCEL(",
            "BUTTON_YESNO(",
            "ICON_QUESTION(",
            "DEFBUTTON2(",
            "IDOK(",
            "IDABORT(",
            "IDRETRY(",
            "IDIGNORE(",
            "IDYES(",
            "IDNO(",
        ] {
            assert!(
                sigs.iter().any(|sig| sig.contains(expected)),
                "门面缺少 {expected}（手册里写着这个名字）"
            );
        }
    }

    #[test]
    fn sandbox_config_still_compiles_with_the_facade() {
        // 门面注册不该把沙箱搞坏：死循环仍然要被操作数上限拦住。
        let (mut engine, _ws, _fake) = facade();
        engine.set_max_operations(1_000);
        let err = run(&engine, "loop { crt().version(); }").unwrap_err();
        assert!(matches!(*err, EvalAltResult::ErrorTooManyOperations(..)));
        let _ = SandboxConfig::default();
    }
}
