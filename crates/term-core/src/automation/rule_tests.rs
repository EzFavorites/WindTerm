//! 凭据字段的装载期拒绝判据。单独成文件：规则本体的测试已把 rule.rs 顶到
//! 单文件上限，回归不该逼生产代码删注释。

use super::*;

#[test]
fn credential_fields_are_a_closed_enum_at_load_time() {
    for (json, expected) in [
        (r#""username""#, CredentialField::Username),
        (r#""password""#, CredentialField::Password),
        (r#"{"keyboard_interactive":2}"#, CredentialField::KeyboardInteractive(2)),
    ] {
        let field: CredentialField = serde_json::from_str(json).expect("已知字段应装载");
        assert_eq!(field, expected);
        let back = serde_json::to_value(field).unwrap();
        let reparsed: CredentialField = serde_json::from_value(back).unwrap();
        assert_eq!(reparsed, expected);
    }
    // 未知字段（包括旧的「ki_first」潜规则）在装载期就拒，不进派发期。
    for bad in [r#""ki_first""#, r#""password:""#, r#"{"keyboard_interactive":"x"}"#] {
        assert!(serde_json::from_str::<CredentialField>(bad).is_err(), "{bad} 应被拒");
    }
    let bad_rule = r#"{
        "id":"r","when":"Connected",
        "actions":[{"CredentialRef":{"scheme":"prod","field":"ki_first"}}]
    }"#;
    assert!(serde_json::from_str::<Rule>(bad_rule).is_err());
}
