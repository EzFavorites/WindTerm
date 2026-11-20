//! D2：Auth 页多勾选（首选 + 回退链）在「表单 ↔ 会话配置」之间的往返。
//!
//! 单独成文件是为了让 `session_form.rs` 的代码行（R9 上限 500，剥注释与空行）
//! 留在阈值内，与 `session_form_serial_tests.rs` 同一口径。
//!
//! 钉住的是「能勾、能存、不生效」这一批要消灭的病：勾了多个认证方式，保存一次
//! 属性不该只剩首选那一项。

use super::*;

#[test]
fn the_auth_chain_round_trips_through_the_form() {
    let mut c = SessionConfig::new(Protocol::Ssh);
    c.name = "chain".to_string();
    c.host = "h".to_string();
    c.username = "u".to_string();
    c.auth = AuthMethod::PublicKey;
    c.auth_chain = vec![AuthMethod::Agent, AuthMethod::Password];

    // 会话 → 表单：首选与回退链都要显示出来。
    let f = NewSessionData::from_config(&c);
    assert_eq!(f.auth, AuthMethod::PublicKey);
    assert_eq!(f.auth_chain, vec![AuthMethod::Agent, AuthMethod::Password]);

    // 表单 → 会话：一个字段都不许丢。
    let saved = f.to_config(SessionConfig::new(Protocol::Ssh));
    assert_eq!(saved.auth, AuthMethod::PublicKey);
    assert_eq!(
        saved.auth_chain,
        vec![AuthMethod::Agent, AuthMethod::Password],
        "回退链要能存回去"
    );
}
