//! Tunnels 表单校验的判据（由 `#[path]` 挂回 `tunnel_settings.rs`）。
//!
//! 审查发现的两处静默：坏端口 `parse().unwrap_or(0)` 变成「动态端口」，
//! 重名也不拦。端口的 u16 边界与名字唯一性都在这里钉死。

use super::*;

#[test]
fn port_text_is_accepted_only_inside_u16() {
    let ok = |s: &str| s.trim().parse::<u16>();
    assert_eq!(ok("0"), Ok(0), "0 = 动态端口，是合法的");
    assert_eq!(ok(" 22 "), Ok(22));
    assert_eq!(ok("65535"), Ok(u16::MAX));
    for bad in ["65536", "-1", "abc", "1.5", ""] {
        assert!(ok(bad).is_err(), "{bad:?} 不该被当成合法端口");
    }
}

#[test]
fn names_must_be_present_and_unique() {
    let rows = vec![
        TunnelProfile::new("alpha".to_string()),
        TunnelProfile::new("beta".to_string()),
    ];
    assert_eq!(validate_rows(&rows), None);

    let empty = vec![TunnelProfile::new(String::new())];
    assert!(validate_rows(&empty).is_some_and(|e| e.contains("名字")));

    let dup = vec![
        TunnelProfile::new("alpha".to_string()),
        TunnelProfile::new("alpha".to_string()),
    ];
    assert!(validate_rows(&dup).is_some_and(|e| e.contains("重复")));
}

#[test]
fn a_fresh_editor_has_no_pending_validation_error() {
    let ed = Editor::new(vec![TunnelProfile::new("alpha".to_string())]);
    assert!(ed.error.is_none());
    assert!(!ed.port_invalid);
    assert_eq!(ed.sel, Some(0));
}
