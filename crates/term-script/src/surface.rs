//! 应用级脚本动词的**注册面**（2026-11-25）。
//!
//! 与 `term_core::surface` 的分工：那张表描述**会话能做什么**（`write` /
//! `screen` / `app_state` …），并且是 RPC 与脚本两端共同的真相；这里只负责把
//! 其中「应用命令」那部分接到脚本引擎的全局函数上——`app_command(id)` 发一条
//! 命令，`app_catalog()` 列清单，`describe()` 答「有哪些动词」。
//!
//! # 为什么单独一层
//!
//! `engine.rs` 的原语注册是一大片手写类型转换（`f64`/`i64` 重载、`Map` 拼装），
//! 自省动词若塞在那里，等于把「发现能力」的实现藏进「执行能力」的文件。这里
//! 保持一件事只有一个入口：动词名取自 [`APP_COMMAND`] / [`APP_CATALOG`] /
//! [`DESCRIBE`]，三项均与 `term_core::surface::script_names()` 对账。
//!
//! # 纪律
//!
//! - `app_command` 只把 ID 交给宿主端口，本文件**没有命令 ID 字面量**
//!   （4e 门禁把它们限制在 `term-app` 的数据层）。
//! - 授权在调用端口前判：`app_command` 要 `app=view`，与 `app_state` 同档
//!   （它不写盘；会落盘的 `app_layout` 才要 `configure`）。
//! - 未知 ID 由宿主显式报 `dispatched: false`，不静默当成成功。

use rhai::{Array, Dynamic, Engine, EvalAltResult, Map};

use term_core::surface::{self, MethodKind};

use crate::{PrimitiveError, ScriptWorkspace};

/// 命令派发动词名（与 `term_core::surface` 的条目同名）。
pub const APP_COMMAND: &str = "app_command";
/// 命令清单动词名。
pub const APP_CATALOG: &str = "app_catalog";
/// 自省动词名（`term-core` 表里现在是 `Both`）。
pub const DESCRIBE: &str = "describe";

/// 把三条应用级动词注册到引擎。由 `engine::register_primitives` 在最后调一次；
/// 脚本侧对账因此天然包含它们（名字就在 `term_core::surface` 表里）。
pub fn register(engine: &mut Engine, ws: ScriptWorkspace) {
    {
        let s = ws.clone();
        engine.register_fn(APP_COMMAND, move |id: &str| app_command(&s, id));
    }
    {
        let s = ws.clone();
        engine.register_fn(APP_CATALOG, move || app_catalog(&s));
    }
    {
        let s = ws.clone();
        engine.register_fn(DESCRIBE, move || describe(&s));
    }
}

fn fail(message: String) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(message.into(), rhai::Position::NONE))
}

fn check_app_view(ws: &ScriptWorkspace) -> Result<(), Box<EvalAltResult>> {
    // `caps()` 现读 live 信封（缺省才退回快照），与工作区的 app_state 同一条
    // 判定路径——不再拿装配时的快照给运行中放行的授权判死。
    ws.caps()
        .check_app_view()
        .map_err(|e| fail(PrimitiveError::Denied(e.to_string()).to_string()))
}

/// `app_command(id)`：授权检查后把命令 ID 交给宿主端口。
///
/// 返回 `{"ok": true, "dispatched": bool}`。宿主拒绝 / 没有界面走 error 分支，
/// 不返回「执行成功」的空话。
pub fn app_command(ws: &ScriptWorkspace, id: &str) -> Result<Dynamic, Box<EvalAltResult>> {
    if id.trim().is_empty() {
        return Err(fail("app_command 需要非空命令 ID".to_string()));
    }
    check_app_view(ws)?;
    ws.check_cancel().map_err(|e| fail(e.to_string()))?;
    let ack = ws
        .env()
        .app
        .command(id)
        .map_err(|e| fail(e.to_string()))?;
    ws.check_cancel().map_err(|e| fail(e.to_string()))?;
    let mut map = Map::new();
    map.insert("ok".into(), Dynamic::from(true));
    map.insert("dispatched".into(), Dynamic::from(ack.dispatched));
    Ok(Dynamic::from(map))
}

/// `app_catalog()`：宿主把三处清单合成一份，脚本据此发现 ID。
pub fn app_catalog(ws: &ScriptWorkspace) -> Result<Dynamic, Box<EvalAltResult>> {
    check_app_view(ws)?;
    ws.check_cancel().map_err(|e| fail(e.to_string()))?;
    let entries = ws
        .env()
        .app
        .catalog()
        .map_err(|e| fail(e.to_string()))?;
    ws.check_cancel().map_err(|e| fail(e.to_string()))?;
    let rows: Array = entries
        .into_iter()
        .map(|entry| {
            let mut row = Map::new();
            row.insert("id".into(), Dynamic::from(entry.id));
            row.insert("summary".into(), Dynamic::from(entry.summary));
            row.insert("source".into(), Dynamic::from(entry.source));
            Dynamic::from(row)
        })
        .collect();
    let mut map = Map::new();
    map.insert("entries".into(), Dynamic::from(rows));
    Ok(Dynamic::from(map))
}

fn kind_name(kind: MethodKind) -> &'static str {
    match kind {
        MethodKind::Introspect => "introspect",
        MethodKind::Read => "read",
        MethodKind::Write => "write",
        MethodKind::SessionLifecycle => "session_lifecycle",
        MethodKind::Stream => "stream",
    }
}

/// `describe()`：脚本侧的自省回答**直接取自 term-core 的同一张表**，不抄第二份。
///
/// 形状与 RPC 的 `describe` 结果 schema 一致（`methods[]`：name / summary /
/// kind / params_schema / result_schema），因此两端拿到的动词集合同源。
pub fn describe(_ws: &ScriptWorkspace) -> Result<Dynamic, Box<EvalAltResult>> {
    let methods: Array = surface::script_specs()
        .map(|spec| {
            let mut row = Map::new();
            row.insert("name".into(), Dynamic::from(spec.name));
            row.insert("summary".into(), Dynamic::from(spec.summary));
            row.insert("kind".into(), Dynamic::from(kind_name(spec.kind)));
            row.insert("params_schema".into(), Dynamic::from(spec.params_schema));
            row.insert("result_schema".into(), Dynamic::from(spec.result_schema));
            Dynamic::from(row)
        })
        .collect();
    let mut map = Map::new();
    map.insert("methods".into(), Dynamic::from(methods));
    Ok(Dynamic::from(map))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    use term_core::script_caps::{AppScope, Capabilities};

    use crate::testutil::{FakeApp, FakeSessions};
    use crate::Sessions;

    /// 与 `workspace.rs` 的 `app_workspace` 同形：假会话 + 假应用端口 + 授权档。
    fn workspace(scope: AppScope, app: FakeApp) -> ScriptWorkspace {
        let fake = FakeSessions::new();
        fake.mount(1, "one");
        let sessions: Arc<dyn Sessions> = fake.clone();
        let current = sessions.attach(1).expect("预挂的会话 1");
        let env = fake
            .env()
            .with_app(Arc::new(app))
            .with_caps(Capabilities {
                app: scope,
                ..Capabilities::NONE
            });
        ScriptWorkspace::new(env, current, Duration::from_secs(5))
    }

    #[test]
    fn the_three_verbs_are_declared_on_the_shared_surface() {
        for name in [APP_COMMAND, APP_CATALOG, DESCRIBE] {
            let spec = surface::find(name).unwrap_or_else(|| panic!("{name} 不在能力面上"));
            assert!(spec.exposure.on_script(), "{name} 脚本侧不可见");
        }
        // describe 从 Rpc-only 改成 Both：远程与脚本问的是同一张表。
        assert_eq!(surface::find(DESCRIBE).unwrap().exposure, surface::Exposure::Both);
    }

    #[test]
    fn app_command_requires_authorization_and_reports_unknown_ids() {
        let denied = workspace(AppScope::Deny, FakeApp::new());
        let err = app_command(&denied, "Terminal.ClearScreen").unwrap_err();
        assert!(err.to_string().contains("授权"), "拒绝要说清原因：{err}");

        let app = FakeApp::new();
        let ws = workspace(AppScope::Tune, app.clone());
        let value = app_command(&ws, "Terminal.ClearScreen").expect("已授权");
        assert_eq!(field_bool(&value, "ok"), Some(true));
        assert_eq!(field_bool(&value, "dispatched"), Some(true));
        // 未知 ID 必须显式报「没派发」，不能静默当成成功。
        let unknown = app_command(&ws, "No.SuchCommand").expect("调用本身合法");
        assert_eq!(field_bool(&unknown, "dispatched"), Some(false));
        assert_eq!(
            app.commands(),
            vec!["Terminal.ClearScreen".to_string(), "No.SuchCommand".to_string()]
        );
    }

    #[test]
    fn app_catalog_and_describe_come_from_the_shared_sources() {
        let app = FakeApp::new();
        let ws = workspace(AppScope::Tune, app);
        let catalog = app_catalog(&ws).expect("清单");
        let rows = field_array(&catalog, "entries").expect("entries 数组");
        assert!(rows.iter().any(|r| field_str(r, "source").as_deref() == Some("app")));

        let described = describe(&ws).expect("自省");
        let methods = field_array(&described, "methods").expect("methods 数组");
        let names: Vec<String> = methods
            .iter()
            .filter_map(|m| field_str(m, "name"))
            .collect();
        for name in [APP_COMMAND, APP_CATALOG, DESCRIBE] {
            assert!(names.iter().any(|n| n == name), "自省漏了 {name}");
        }
    }

    fn field_bool(value: &Dynamic, key: &str) -> Option<bool> {
        value
            .clone()
            .try_cast::<Map>()
            .and_then(|m| m.get(key).cloned())
            .and_then(|v| v.as_bool().ok())
    }

    fn field_str(value: &Dynamic, key: &str) -> Option<String> {
        value
            .clone()
            .try_cast::<Map>()
            .and_then(|m| m.get(key).cloned())
            .and_then(|v| v.try_cast::<String>())
    }

    fn field_array(value: &Dynamic, key: &str) -> Option<Array> {
        value
            .clone()
            .try_cast::<Map>()
            .and_then(|m| m.get(key).cloned())
            .and_then(|v| v.try_cast::<Array>())
    }
}
