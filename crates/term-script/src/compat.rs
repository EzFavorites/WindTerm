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
//! | `MessageBox` 的整数返回值 | 字符串结果 → 整数（`MB_*` 常量） | 老代码 `if r = 6` 可改成常量而无歧义 |
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

use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, Position};

use crate::error::PrimitiveError;
use crate::human::{Ask, MessageButton, MessageIcon, PickFile};
use crate::sessions::SessionInfo;
use crate::workspace::ScriptWorkspace;

/// `crt` 对象本体（`version()` / `arguments()` / `get_tab(…)` / `sleep` / `quit`）。
#[derive(Clone)]
pub struct Crt {
    ws: ScriptWorkspace,
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

    // ── 入口 `crt()` ──────────────────────────────────────────────────
    {
        let ws = ws.clone();
        engine.register_fn("crt", move || Crt { ws: ws.clone() });
    }

    // ── Crt：顶层成员 ─────────────────────────────────────────────────
    engine.register_fn("version", |crt: &mut Crt| -> Result<Dynamic, Box<EvalAltResult>> {
        ok_map(Dynamic::from(crt.ws.env().context.version.clone()))
    });
    engine.register_fn(
        "script_full_name",
        |crt: &mut Crt| -> Result<Dynamic, Box<EvalAltResult>> {
            ok_map(Dynamic::from(
                crt.ws
                    .env()
                    .context
                    .script_path
                    .clone()
                    .unwrap_or_default(),
            ))
        },
    );
    engine.register_fn(
        "arguments",
        |crt: &mut Crt| -> Result<Dynamic, Box<EvalAltResult>> {
            let args: Array = crt
                .ws
                .env()
                .context
                .args
                .iter()
                .map(|a| Dynamic::from(a.clone()))
                .collect();
            ok_map(Dynamic::from(args))
        },
    );
    engine.register_fn(
        "argument",
        |crt: &mut Crt, index: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            // CRT 的 `Arguments(n)` 是 0 起的；越界返回空串（CRT 同）。
            if index < 0 {
                return Err(fail("Arguments 的序号不能为负（0 起）"));
            }
            let args = &crt.ws.env().context.args;
            Ok(Dynamic::from(
                args.get(index as usize).cloned().unwrap_or_default(),
            ))
        },
    );
    engine.register_fn(
        "get_tab_count",
        |crt: &mut Crt| -> Result<Dynamic, Box<EvalAltResult>> {
            Ok(Dynamic::from(crt.ws.session_list().len() as i64))
        },
    );
    engine.register_fn(
        "get_tab",
        |crt: &mut Crt, index: i64| -> Result<CrtTab, Box<EvalAltResult>> {
            if index < 1 {
                return Err(fail("GetTab 的序号从 1 起"));
            }
            let info = crt
                .ws
                .session_at(index as usize)
                .map_err(fail)?;
            Ok(CrtTab {
                // 钉死在这条会话上：`get_tab(2).screen.send(…)` 打的是 2 号
                // 标签，而不是拍完这一句之后全局还绑在哪一条。
                ws: crt.ws.scoped_to(info.id).map_err(fail)?,
                name: info.name,
            })
        },
    );
    engine.register_fn(
        "get_active_tab",
        |crt: &mut Crt| -> Result<CrtTab, Box<EvalAltResult>> {
            let info = crt.ws.session_active().map_err(fail)?;
            Ok(CrtTab {
                ws: crt.ws.scoped_to(info.id).map_err(fail)?,
                name: info.name,
            })
        },
    );
    engine.register_fn(
        "sleep",
        |crt: &mut Crt, millis: i64| -> Result<(), Box<EvalAltResult>> {
            if millis < 0 {
                return Err(fail("Sleep 的毫秒数不能为负"));
            }
            crt.ws.sleep(millis as u64).map_err(fail)
        },
    );
    engine.register_fn(
        "quit",
        |crt: &mut Crt| -> Result<(), Box<EvalAltResult>> {
            crt.ws.app_quit(true).map_err(fail)
        },
    );
    engine.register_fn("last_error", |crt: &mut Crt| -> Result<Dynamic, Box<EvalAltResult>> {
        ok_map(Dynamic::from(crt.ws.env().context.last_error()))
    });

    // ── 属性：crt.Screen / Session / Dialog / Clipboard / Window ──────
    engine.register_get("screen", |crt: &mut Crt| CrtScreen { ws: crt.ws.clone() });
    engine.register_get("session", |crt: &mut Crt| CrtSession { ws: crt.ws.clone() });
    engine.register_get("dialog", |crt: &mut Crt| CrtDialog { ws: crt.ws.clone() });
    engine.register_get("clipboard", |crt: &mut Crt| CrtClipboard { ws: crt.ws.clone() });
    engine.register_get("window", |crt: &mut Crt| CrtWindow { ws: crt.ws.clone() });

    // ── CrtTab：标签对象转发到它自己的会话 ────────────────────────────
    engine.register_get("screen", |tab: &mut CrtTab| CrtScreen {
        ws: tab.ws.clone(),
    });
    engine.register_get("session", |tab: &mut CrtTab| CrtSession {
        ws: tab.ws.clone(),
    });
    engine.register_get("name", |tab: &mut CrtTab| tab.name.clone());
    engine.register_fn("index", |tab: &mut CrtTab| -> i64 {
        // 标签栏顺序里的 1 起序号；查不到给 0（CRT 没有 0 号，脚本据此能判断）。
        tab.ws
            .session_list()
            .iter()
            .position(|s| s.name == tab.name)
            .map(|i| i as i64 + 1)
            .unwrap_or(0)
    });

    // ── CrtScreen ─────────────────────────────────────────────────────
    engine.register_fn(
        "send",
        |screen: &mut CrtScreen, text: &str| -> Result<(), Box<EvalAltResult>> {
            screen.ws.write(text).map_err(fail)
        },
    );
    engine.register_fn(
        "wait_for_string",
        |screen: &mut CrtScreen, text: &str, secs: f64| -> Result<Dynamic, Box<EvalAltResult>> {
            // **不是 wait**：CRT 的 WaitForString 是消费式的，映射到 read_until。
            screen
                .ws
                .read_until(text, secs, None)
                .map(Dynamic::from)
                .map_err(fail)
        },
    );
    engine.register_fn(
        "read_string",
        |screen: &mut CrtScreen, text: &str, secs: f64| -> Result<Dynamic, Box<EvalAltResult>> {
            screen
                .ws
                .read_until(text, secs, None)
                .map(Dynamic::from)
                .map_err(fail)
        },
    );
    engine.register_fn(
        "wait_for_strings",
        |screen: &mut CrtScreen, patterns: Array, secs: f64| -> Result<Dynamic, Box<EvalAltResult>> {
            let list: Result<Vec<String>, _> = patterns
                .into_iter()
                .map(|v| v.into_string().map_err(|t| fail(format!("WaitForStrings 的项必须是字符串（实际 {t}）"))))
                .collect();
            let list = list?;
            let hit = screen.ws.wait_any(&list, secs, None).map_err(fail)?;
            Ok(Dynamic::from(hit.index as i64))
        },
    );
    engine.register_fn(
        "get",
        |screen: &mut CrtScreen, r1: i64, c1: i64, r2: i64, c2: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            let coords = checked_rect(r1, c1, r2, c2)?;
            screen
                .ws
                .region(coords.0, coords.1, coords.2, coords.3)
                .map(Dynamic::from)
                .map_err(fail)
        },
    );
    engine.register_fn(
        "get2",
        |screen: &mut CrtScreen, r1: i64, c1: i64, r2: i64, c2: i64| -> Result<Dynamic, Box<EvalAltResult>> {
            let coords = checked_rect(r1, c1, r2, c2)?;
            screen
                .ws
                .region_lines(coords.0, coords.1, coords.2, coords.3)
                .map(Dynamic::from)
                .map_err(fail)
        },
    );
    engine.register_fn(
        "selection",
        |screen: &mut CrtScreen| -> Result<Dynamic, Box<EvalAltResult>> {
            screen.ws.selection().map(Dynamic::from).map_err(fail)
        },
    );
    engine.register_fn(
        "synchronous",
        |screen: &mut CrtScreen, on: bool| -> Result<(), Box<EvalAltResult>> {
            screen.ws.set_synchronous(on);
            Ok(())
        },
    );
    engine.register_fn("current_row", |screen: &mut CrtScreen| -> Result<i64, Box<EvalAltResult>> {
        screen
            .ws
            .cursor_pos()
            .map(|(row, _)| row as i64 + 1)
            .map_err(fail)
    });
    engine.register_fn("current_column", |screen: &mut CrtScreen| -> Result<i64, Box<EvalAltResult>> {
        screen
            .ws
            .cursor_pos()
            .map(|(_, col)| col as i64 + 1)
            .map_err(fail)
    });
    engine.register_fn("rows", |screen: &mut CrtScreen| -> Result<i64, Box<EvalAltResult>> {
        screen
            .ws
            .terminal_size()
            .map(|(_, rows)| rows as i64)
            .map_err(fail)
    });
    engine.register_fn("columns", |screen: &mut CrtScreen| -> Result<i64, Box<EvalAltResult>> {
        screen
            .ws
            .terminal_size()
            .map(|(cols, _)| cols as i64)
            .map_err(fail)
    });
    engine.register_fn(
        "send_keys",
        |screen: &mut CrtScreen, expr: &str| -> Result<(), Box<EvalAltResult>> {
            screen.ws.send_keys(expr).map(|_| ()).map_err(fail)
        },
    );

    // ── CrtSession ────────────────────────────────────────────────────
    engine.register_fn(
        "connect",
        |session: &mut CrtSession, name: &str| -> Result<(), Box<EvalAltResult>> {
            session.ws.session_connect(name).map(|_| ()).map_err(fail)
        },
    );
    engine.register_fn(
        "connect_in_tab",
        |session: &mut CrtSession, name: &str| -> Result<Dynamic, Box<EvalAltResult>> {
            session
                .ws
                .session_connect(name)
                .map(|info| session_map(&info))
                .map_err(fail)
        },
    );
    engine.register_fn(
        "disconnect",
        |session: &mut CrtSession| -> Result<(), Box<EvalAltResult>> {
            let name = session.ws.current_name().map_err(fail)?;
            session.ws.session_close(&name).map_err(fail)
        },
    );
    engine.register_fn(
        "connected",
        |session: &mut CrtSession| -> Result<bool, Box<EvalAltResult>> {
            session
                .ws
                .state()
                .map(|state| matches!(state, term_core::session_runtime::SessionState::Alive))
                .map_err(fail)
        },
    );
    engine.register_fn(
        "log",
        |session: &mut CrtSession, enabled: bool, append: bool, raw: bool| -> Result<(), Box<EvalAltResult>> {
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
        },
    );
    engine.register_fn(
        "log_file_name",
        |session: &mut CrtSession, name: &str| -> Result<(), Box<EvalAltResult>> {
            // CRT 的 LogFileName 要在开始记录之前设置；我们把它当作「下一次
            // log_start 的模板」——现在已经在记录时报错（改模板不能换文件）。
            if session.ws.log_status().map(|s| s.is_recording()).unwrap_or(false) {
                return Err(fail("已经在记录日志：先 Log(False) 再改 LogFileName"));
            }
            let opts = term_core::log_opts::LogOptions {
                file_name: name.to_string(),
                ..term_core::log_opts::LogOptions::default()
            };
            // 记下模板：用 log_start 的下一次调用生效。
            session.ws.remember_log_template(opts);
            Ok(())
        },
    );
    engine.register_fn(
        "clone",
        |session: &mut CrtSession| -> Result<Dynamic, Box<EvalAltResult>> {
            let name = session.ws.current_name().map_err(fail)?;
            session
                .ws
                .session_clone(&name)
                .map(|info| session_map(&info))
                .map_err(fail)
        },
    );
    engine.register_fn(
        "connect_sftp",
        |session: &mut CrtSession| -> Result<Dynamic, Box<EvalAltResult>> {
            // 句柄在能力面里是「每个 sftp_* 动词自己取」的；门面返回当前会话名，
            // 让脚本能把它当作后续 sftp_* 的第一个参数。
            let name = session.ws.current_name().map_err(fail)?;
            Ok(Dynamic::from(name))
        },
    );

    // ── CrtDialog ─────────────────────────────────────────────────────
    engine.register_fn(
        "message_box",
        |dialog: &mut CrtDialog, text: &str, title: &str| -> Result<i64, Box<EvalAltResult>> {
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
            Ok(button_id(answer))
        },
    );
    engine.register_fn(
        "message_box",
        |dialog: &mut CrtDialog, text: &str, title: &str, buttons: i64| -> Result<i64, Box<EvalAltResult>> {
            let group = group_from_id(buttons).ok_or_else(|| fail(format!("不认识的按钮组编号 {buttons}（可用 MB_OK / MB_OKCANCEL / MB_YESNO / MB_YESNOCANCEL）")))?;
            let answer = dialog
                .ws
                .ask_request(&Ask::Message {
                    title: title.to_string(),
                    text: text.to_string(),
                    icon: MessageIcon::None,
                    buttons: group,
                    default_button: None,
                })
                .map_err(fail)?;
            Ok(button_id(answer))
        },
    );
    engine.register_fn(
        "prompt",
        |dialog: &mut CrtDialog, prompt: &str, title: &str, default: &str, secret: bool| -> Result<Dynamic, Box<EvalAltResult>> {
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
        },
    );
    engine.register_fn(
        "file_open_dialog",
        |dialog: &mut CrtDialog, title: &str, filter: &str| -> Result<Dynamic, Box<EvalAltResult>> {
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
        },
    );
    engine.register_fn(
        "file_save_dialog",
        |dialog: &mut CrtDialog, title: &str, filter: &str| -> Result<Dynamic, Box<EvalAltResult>> {
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
        },
    );

    // ── CrtClipboard：`Text` 属性（读）+ `Set` 方法（写） ──────────────
    //
    // 读用属性（`crt.Clipboard.Text`，与手册一致）。写用方法：Rhai 的
    // `register_set` 只接受返回值恰好是 `()` 的闭包，而写入可能失败
    // （没有剪贴板 / 平台报错）——为了走属性而把失败吞掉，等于让脚本以为
    // 已经写好了。所以这一处形状有别：`crt.Clipboard.Set("x")`。
    engine.register_get("text", |clip: &mut CrtClipboard| -> Result<String, Box<EvalAltResult>> {
        clip.ws.clipboard_get().map_err(fail)
    });
    engine.register_fn(
        "set",
        |clip: &mut CrtClipboard, value: &str| -> Result<(), Box<EvalAltResult>> {
            clip.ws.clipboard_set(value).map_err(fail)
        },
    );

    // ── CrtWindow：`Show` / `Hide` 方法 ──────────────────────────────
    // 同样理由：`Window.Visible = false` 要能失败，所以用两个方法表达。
    engine.register_fn(
        "hide",
        |window: &mut CrtWindow| -> Result<(), Box<EvalAltResult>> {
            window.ws.set_window_visible(false).map_err(fail)
        },
    );
    engine.register_fn(
        "show",
        |window: &mut CrtWindow| -> Result<(), Box<EvalAltResult>> {
            window.ws.set_window_visible(true).map_err(fail)
        },
    );

    // ── MessageBox 常量（CRT 的 1/2/6/7） ─────────────────────────────
    for (name, value) in [
        ("MB_OK", 1_i64),
        ("MB_OKCANCEL", 2),
        ("MB_YESNO", 6),
        ("MB_YESNOCANCEL", 7),
        ("IDOK", 1),
        ("IDCANCEL", 2),
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

fn group_from_id(id: i64) -> Option<MessageButton> {
    match id {
        1 => Some(MessageButton::Ok),
        2 => Some(MessageButton::OkCancel),
        6 => Some(MessageButton::YesNo),
        7 => Some(MessageButton::YesNoCancel),
        _ => None,
    }
}

/// 按钮标签 → CRT 的整数 ID。
fn button_id(answer: crate::human::Answer) -> i64 {
    let label = match answer {
        crate::human::Answer::Text(text) => text,
        crate::human::Answer::Bool(true) => "yes".to_string(),
        crate::human::Answer::Bool(false) => "no".to_string(),
        _ => "ok".to_string(),
    };
    match label.as_str() {
        "ok" => 1,
        "cancel" => 2,
        "yes" => 6,
        "no" => 7,
        _ => 1,
    }
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

    #[test]
    fn send_forwards_to_write_and_wait_for_string_is_consuming() {
        let (engine, _ws, fake) = facade();
        let _ = run(&engine, r#"crt().screen.send("ls\n")"#).expect("发送");
        assert_eq!(
            String::from_utf8_lossy(&fake.manip(1).expect("会话").written()),
            "ls\n"
        );
        // WaitForString 走 read_until（消费式）：推一段含哨兵的输出即可命中。
        fake.manip(1).expect("会话").push_output("banner$ ");
        let hit = run(&engine, r#"crt().screen.wait_for_string("$", 1.0)"#).expect("命中");
        // read_until 返回到**命中点为止**（含哨兵），哨兵后面的空格留在
        // 缓冲区里等下一次读——与 CRT 的 WaitForString 一致。
        assert_eq!(hit.to_string(), "banner$");
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
            run(&engine, "crt().script_full_name()").expect("路径").to_string(),
            "/tmp/demo.rhai"
        );
    }

    #[test]
    fn message_box_maps_button_labels_back_to_crt_ids() {
        let (engine, _ws, fake) = facade();
        fake.answer("yes");
        let id = run(&engine, r#"crt().dialog.message_box("continue?", "t", 6)"#).expect("问");
        assert_eq!(id.as_int().expect("整数"), 6, "yes → IDYES=6");
        // 常量本身也可用。
        assert_eq!(
            run(&engine, "MB_YESNO()").expect("常量").as_int().expect("整数"),
            6
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
            "text",
            "set(",
            "show(",
            "hide(",
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
