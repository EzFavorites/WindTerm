//! ADR-0030 二轮审查的三条判据：同一提示只应答一次、用户按键抢占独占、
//! 同帧按 `(priority, seq)` 排序落地。全部走真实 `Arbitrator` + `Tab` +
//! `DeskSurface`，而不是只测状态机。

use std::sync::{Arc, Mutex};

use eframe::egui;
use serde_json::json;
use term_core::automation::{Arbitrator, Priority};
use term_core::contract::Surface;
use term_core::session::{Protocol, SessionConfig};
use term_core::surface_error::ErrorKind;
use term_session::SessionPool;

use crate::bridge::surface::{shared_arbiter_arc, DeskSurface};
use crate::ui::Tab;

use crate::bridge::surface::{arbiter_session, shared_arbiter};
use crate::ui::dialogs::triggers::model::{Channel, SendCondition, UiAction};
use crate::ui::terminal::trigger_dispatch::{
    CredentialIntent, DispatchCtx, FrameIntent, IntentKind, SurfaceIntent,
};

#[derive(Clone, Default)]
struct Writes(Arc<Mutex<Vec<u8>>>);

impl term_runtime::channel::TermChannel for Writes {
    fn drain(&mut self) -> Vec<u8> { Vec::new() }
    fn write(&mut self, bytes: &[u8]) { self.0.lock().expect("写记录锁").extend_from_slice(bytes); }
    fn resize(&mut self, _cols: usize, _rows: usize) {}
    fn poll(&mut self) -> Option<String> { None }
    fn finished(&self) -> bool { false }
    fn close(&mut self) {}
    fn description(&self) -> &str { "arbiter regression channel" }
}

fn fixture(id: &str) -> (Tab, Writes) {
    let pool = Arc::new(term_session::SessionPool::new());
    let writes = Writes::default();
    let tab = crate::ui::terminal::make_tab_with_channel(
        &pool,
        SessionConfig::new(Protocol::Shell),
        Box::new(writes.clone()),
        egui::Id::new(id),
        &egui::Context::default(),
    );
    (tab, writes)
}

fn send_rule(id: &str, pattern: &str, text: &str) -> crate::ui::dialogs::triggers::model::Rule {
    let mut rule = crate::ui::dialogs::triggers::model::Rule::default();
    rule.id = id.into();
    rule.enabled = true;
    rule.on = Channel::Output;
    rule.pattern = pattern.into();
    rule.literal = true;
    rule.config.send.condition = SendCondition::AfterMatch;
    rule.actions = vec![UiAction::SendText(text.into())];
    rule
}

fn key_of(tab: &Tab) -> u64 {
    arbiter_session(&tab.pool, tab.session.expect("夹具带活会话"))
}

#[test]
fn two_rules_hitting_the_same_password_prompt_write_only_once() {
    let (mut tab, writes) = fixture("same-prompt-one-writer");
    let (tx, rx) = std::sync::mpsc::channel();
    tab.raw.attach(crate::hex_tap::HexTap::new(rx));
    let rules = [
        send_rule("onekey-credential", "password:", "secret-A"),
        send_rule("user-rule", "password:", "secret-B"),
    ];
    let key = key_of(&tab);

    tx.send(b"password:".to_vec()).expect("同一帧的提示符");
    let mut note = None;
    tab.poll_with_note(&rules, &mut Vec::new(), false, &mut note);

    crate::ui::lifecycle_test_support::eventually("只写一个应答", || {
        let bytes = writes.0.lock().expect("读写入").clone();
        bytes == b"secret-A" || bytes == b"secret-B"
    });
    let bytes = writes.0.lock().expect("读写入").clone();
    assert_eq!(bytes.len(), "secret-A".len(), "两条规则不能各答一次：{bytes:?}");
    assert!(
        note.as_deref().is_some_and(|n| n.contains("压制")),
        "被压制的规则必须留痕：{note:?}"
    );
    let guard = shared_arbiter().lock().expect("仲裁器锁");
    assert!(guard.suppressed_count(key) >= 1, "压制必须计数");
    assert_eq!(guard.owner(key).unwrap().source, "onekey-credential");
    drop(guard);
    shared_arbiter().lock().expect("仲裁器锁").clear(key);
}

#[test]
fn a_user_keypress_preempts_and_releases_an_exclusive_api_owner() {
    let (tab, _writes) = fixture("key-preempts-exclusive");
    let key = key_of(&tab);
    let (grant, owner) = {
        let mut guard = shared_arbiter().lock().expect("仲裁器锁");
        let grant = guard
            .request(key, "api:write", Priority::P2Explicit, true)
            .expect("独占申请应放行");
        (grant, guard.owner(key).unwrap().source)
    };
    assert!(grant.owner);
    assert_eq!(owner, "api:write");
    assert!(shared_arbiter().lock().expect("仲裁器锁").is_valid(key, &grant));

    // 键盘路径只发信号，不排队、不留下 P0 owner。
    crate::ui::trigger_intents::user_key_preempt(&tab);
    {
        let guard = shared_arbiter().lock().expect("仲裁器锁");
        assert!(guard.owner(key).is_none(), "按键释放显式独占");
        assert!(!guard.is_valid(key, &grant), "已排队的写许可作废");
        assert!(guard.suppressed_count(key) >= 1, "释放/抢占必须留痕");
    }
    // 释放之后下一条规则可以正常拿 owner；写完按 source 释放。
    let mut guard = shared_arbiter().lock().expect("仲裁器锁");
    let next = guard.request(key, "rule-after", Priority::P3Rule, false).unwrap();
    assert!(next.owner);
    assert!(guard.release(key, "rule-after"));
    guard.clear(key);
}

/// 走 pane.rs 的真实键盘落地漏斗，而不是直接调抢占信号：egui `Event::Text`
/// 转成字节后写会话，必须先释放已有的独占 owner，字节也确实进了这个会话。
#[test]
fn a_key_event_sent_to_the_terminal_releases_an_exclusive_owner() {
    let (tab, writes) = fixture("key-event-preempts-exclusive");
    let key = key_of(&tab);
    let grant = {
        let mut guard = shared_arbiter().lock().expect("仲裁器锁");
        guard
            .request(key, "api:write", Priority::P2Explicit, true)
            .expect("独占申请应放行")
    };
    assert!(grant.owner);
    assert_eq!(
        shared_arbiter().lock().expect("仲裁器锁").owner(key).unwrap().source,
        "api:write"
    );

    // 与 pane.rs 的 Event::Text 分支同一套编码口径：普通字符 "x"。
    let text = "x";
    let bytes = term_core::keys::text_bytes_for_modifiers(
        text,
        false,
        false,
        tab.config.alt_modifier_sends,
        tab.config.meta_modifier_sends,
    )
    .unwrap_or_else(|| text.as_bytes().to_vec());
    crate::ui::terminal::pane::send_user_key_bytes(&tab, &bytes);

    {
        let guard = shared_arbiter().lock().expect("仲裁器锁");
        assert!(guard.owner(key).is_none(), "真实按键释放已有独占 owner");
        assert!(!guard.is_valid(key, &grant), "已排队的写许可作废");
        assert!(guard.suppressed_count(key) >= 1, "抢占必须留痕");
    }
    crate::ui::lifecycle_test_support::eventually("按键字节写进会话", || {
        writes.0.lock().expect("读写入").as_slice() == b"x"
    });
    shared_arbiter().lock().expect("仲裁器锁").clear(key);
}

#[test]
fn same_frame_writes_are_sorted_by_priority_then_sequence() {
    let (mut tab, writes) = fixture("priority-order-no-interleave");
    let key = key_of(&tab);
    let (p3, p2, p4) = {
        let mut guard = shared_arbiter().lock().expect("仲裁器锁");
        let p3 = guard.request(key, "rule", Priority::P3Rule, false).unwrap();
        let p2 = guard.request(key, "api", Priority::P2Explicit, false).unwrap();
        let p4 = guard.request(key, "hover", Priority::P4Passive, false).unwrap();
        (p3, p2, p4)
    };
    assert!(p2.seq > p3.seq, "排序键里 seq 才是同级到达顺序");

    let intent = |source: &str, priority, grant, text: &str| FrameIntent {
        ctx: DispatchCtx {
            session: key,
            source: source.into(),
            priority,
            owner: None,
            grant: Some(grant),
        },
        kind: IntentKind::Surface(SurfaceIntent {
            method: "write".into(),
            params: serde_json::json!({ "session": "current", "text": text }),
        }),
        // 故意逆序入队：落地时才按 (priority, seq) 排。
        release: term_core::automation::Release::Immediate,
        since: term_core::automation::Ticks::ZERO,
    };
    tab.pending_intents = vec![
        intent("rule", Priority::P3Rule, p3, "P3"),
        intent("api", Priority::P2Explicit, p2, "P2"),
        intent("hover", Priority::P4Passive, p4, "P4-只读"),
    ];
    // 排序发生在窗口级帧末派发（drain），不是单标签的 drain_tab。
    let win = crate::ui::lifecycle_test_support::window_with(
        tab.pool.clone(),
        &egui::Context::default(),
        vec![tab],
    );
    let mut win = win;
    let mut note = None;
    crate::ui::trigger_intents::drain(&mut win, &mut Vec::new(), &mut note);

    crate::ui::lifecycle_test_support::eventually("按优先级落地", || {
        writes.0.lock().expect("读写入").as_slice() == b"P2P3"
    });
    assert_eq!(
        writes.0.lock().expect("读写入").as_slice(),
        b"P2P3",
        "P2 先于 P3、P4 不写；每条文本完整不交错"
    );
    shared_arbiter().lock().expect("仲裁器锁").clear(key);
}

#[test]
fn a_stale_ui_grant_is_rechecked_at_landing_and_never_writes() {
    let (mut tab, writes) = fixture("ui-toctou-stale-grant");
    let key = key_of(&tab);
    let grant = shared_arbiter()
        .lock()
        .expect("仲裁器锁")
        .request(key, "rule", Priority::P3Rule, false)
        .expect("规则许可");
    tab.pending_intents = vec![FrameIntent {
        ctx: DispatchCtx {
            session: key,
            source: "rule".into(),
            priority: Priority::P3Rule,
            owner: None,
            grant: Some(grant),
        },
        kind: IntentKind::Surface(SurfaceIntent {
            method: "write".into(),
            params: json!({ "session": "current", "text": "STALE" }),
        }),
        release: term_core::automation::Release::Immediate,
        since: term_core::automation::Ticks::ZERO,
    }];
    // 排队之后、落地之前来了用户按键：epoch 前进，旧许可作废。
    crate::ui::trigger_intents::user_key_preempt(&tab);
    let mut win = crate::ui::lifecycle_test_support::window_with(
        tab.pool.clone(),
        &egui::Context::default(),
        vec![tab],
    );
    let mut note = None;
    crate::ui::trigger_intents::drain(&mut win, &mut Vec::new(), &mut note);
    assert!(
        writes.0.lock().expect("读写入").is_empty(),
        "失效的 UI 许可不许把字节写进 PTY"
    );
    assert!(
        note.as_deref().is_some_and(|n| n.contains("抢占") || n.contains("Busy")),
        "压制必须留痕给用户：{note:?}"
    );
    shared_arbiter().lock().expect("仲裁器锁").clear(key);
}

#[test]
fn credential_fields_are_exhaustive_and_empty_values_are_errors() {
    use term_core::automation::CredentialField;

    let password: CredentialField = serde_json::from_str(r#""password""#).unwrap();
    assert_eq!(password, CredentialField::Password);
    let ki: CredentialField = serde_json::from_str(r#"{"keyboard_interactive":3}"#).unwrap();
    assert_eq!(ki, CredentialField::KeyboardInteractive(3));
    assert!(serde_json::from_str::<CredentialField>(r#""ki_first""#).is_err());

    let (tab, _writes) = fixture("credential-empty-value");
    let ctx = egui::Context::default();
    let win = crate::ui::lifecycle_test_support::window_with(tab.pool.clone(), &ctx, vec![tab]);
    for field in [
        CredentialField::Username,
        CredentialField::Password,
        CredentialField::KeyboardInteractive(0),
    ] {
        let cred = CredentialIntent { scheme: "absent".into(), field };
        let err = crate::ui::trigger_intents::credential_value(&win, &cred)
            .expect_err("空值 / 未知方案必须报错，不许静默写空串");
        assert!(!err.is_empty(), "{field:?} 的错误要说清原因");
    }
}

fn api_fixture() -> (DeskSurface, u64, Arc<Mutex<Arbitrator>>) {
    let pool = Arc::new(SessionPool::new());
    let id = pool.open_with_channel(
        &SessionConfig::new(Protocol::Shell),
        80,
        24,
        Box::new(Writes::default()),
    );
    let arbiter = Arc::new(Mutex::new(Arbitrator::new()));
    (
        DeskSurface::with_arbiter(pool.clone(), arbiter.clone()),
        arbiter_session(&pool, id),
        arbiter,
    )
}

#[test]
fn an_exclusive_api_write_is_busy_during_auth_and_takes_over_a_rule_later() {
    let (surface, key, arbiter) = api_fixture();
    {
        let mut guard = arbiter.lock().expect("仲裁器锁");
        guard.set_phase(key, term_core::automation::Phase::Connecting);
        guard.request(key, "auth", Priority::P1Auth, true).unwrap();
    }
    let err = surface
        .invoke(
            "write",
            &json!({ "session": "Local Shell", "text": "x", "exclusive": true }),
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Busy, "仲裁 Busy 有独立 kind，不能和锁屏混为一谈");
    assert_eq!(err.code, -32005);
    assert!(err.message.contains("Busy"), "{}", err.message);

    {
        let mut guard = arbiter.lock().expect("仲裁器锁");
        guard.set_phase(key, term_core::automation::Phase::Live);
        guard.request(key, "rule", Priority::P3Rule, false).unwrap();
    }
    let result = surface
        .invoke(
            "write",
            &json!({ "session": "Local Shell", "text": "x", "exclusive": true }),
        )
        .expect("P2 独占可以抢 P3 owner");
    assert_eq!(result["written"], true);
    let guard = arbiter.lock().expect("仲裁器锁");
    assert!(guard.owner(key).is_none(), "调用结束释放独占，不泄漏 owner");
    assert!(guard.suppressed_count(key) >= 1, "被抢的规则写要留痕");
}

#[test]
fn a_default_api_write_coexists_with_the_rule_owner() {
    let (surface, key, arbiter) = api_fixture();
    arbiter.lock().expect("仲裁器锁")
        .request(key, "rule", Priority::P3Rule, false).unwrap();
    let result = surface
        .invoke("keys", &json!({ "session": "Local Shell", "keys": ["enter"] }))
        .expect("非独占调用应放行");
    assert_eq!(result["written"], true);
    let guard = arbiter.lock().expect("仲裁器锁");
    assert_eq!(guard.owner(key).unwrap().source, "rule", "非独占不抢 owner");
    assert_eq!(guard.suppressed_count(key), 0, "非独占不算压制");
}

#[test]
fn an_unknown_action_id_is_reported_instead_of_silently_running_a_shell() {
    let (tab, _writes) = fixture("unknown-action-id");
    let mut win = crate::ui::lifecycle_test_support::window_with(
        tab.pool.clone(),
        &egui::Context::default(),
        vec![tab],
    );
    win.dispatch("Window.NoSuchCommand");
    assert!(
        win.log_note.as_deref().is_some_and(|n| n.contains("未知命令 ID")),
        "拼错的命令 ID 必须显式报错，不能静默丢给 /bin/sh：{:?}",
        win.log_note
    );
}

#[test]
fn shared_arbiter_is_the_one_both_ui_and_api_use() {
    let (tab, _writes) = fixture("shared-arbiter-identity");
    let key = key_of(&tab);
    let surface = DeskSurface::with_arbiter(tab.pool.clone(), shared_arbiter_arc());
    let mut guard = shared_arbiter().lock().expect("仲裁器锁");
    guard.set_phase(key, term_core::automation::Phase::Connecting);
    guard.request(key, "auth", Priority::P1Auth, true).unwrap();
    drop(guard);
    // 同一个共享实例：API 的独占申请必须看见 UI 侧记下的 P1 连接期 owner。
    let err = surface
        .invoke("write", &json!({ "session": "Local Shell", "text": "x", "exclusive": true }))
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Busy, "{err:?}");
    assert!(err.message.contains("Busy"));
    shared_arbiter().lock().expect("仲裁器锁").clear(key);
}
