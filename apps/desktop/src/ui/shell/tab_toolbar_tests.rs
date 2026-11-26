//! `tab_toolbar` 的纯几何 / 读数格式单测。
//!
//! 单独成文件：`tab_toolbar.rs` 的**生产段**（含 `impl` 与命中矩形）已近 500
//! 代码行门禁，测试留在这里让主文件合规。`#[path]` 引入，仍属同一模块树。

#![cfg(test)]

use super::*;

    use super::*;

    #[test]
    fn readout_and_button_share_the_same_right_edge() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, TAB_TOOLBAR_H));
        let strip = readout_rect(rect, rect.left(), READOUT_W);
        // 按钮宽度由字体决定，这里只要求读数条右沿贴住 EDGE_PAD。
        assert!((strip.right() - (rect.right() - EDGE_PAD)).abs() < 0.01);
        assert!(strip.width() <= READOUT_W);
    }

    #[test]
    fn longer_readouts_grow_left_ward_without_moving_the_right_edge() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, TAB_TOOLBAR_H));
        let fixture = readout_rect(rect, rect.left(), READOUT_W);
        let long = readout_rect(rect, rect.left(), READOUT_W + 40.0);
        assert!(long.left() < fixture.left(), "长读数要往左撑开");
        assert_eq!(long.right(), fixture.right(), "右沿必须钉死");
        assert!(long.width() <= rect.width());
    }

    #[test]
    fn remote_text_never_invents_a_reading() {
        let none = crate::sysmon_sample::Reading::default();
        assert_eq!(remote_text(false, Some(&none)), "—", "关着就不报数");
        assert_eq!(remote_text(true, None), "—", "采不到就是没有");
        assert_eq!(remote_text(true, Some(&none)), "0.0%", "真采到的 0 才是 0");
        let live = crate::sysmon_sample::Reading {
            cpu_pct: 37.5,
            ..none
        };
        assert_eq!(remote_text(true, Some(&live)), "37.5%");
    }

    #[test]
    fn mem_text_matches_the_reference_format() {
        let used = 24_117_248u64;
        let total = 24_776_704u64;
        assert_eq!(mem_text(used, total), "23.00/23.63 GiB");
        assert_eq!(mem_text(0, 0), "—/— GiB");
    }

    #[test]
    fn a_failed_remote_sample_shows_a_dash_not_zero_percent() {
        let failed = crate::sysmon_remote::from_exec(
            crate::sysmon_remote::Target::Linux,
            0,
            Err(term_ssh::SshError::Config("采样失败".to_string())),
        );
        assert!(failed.is_none());
        assert_eq!(remote_text(true, failed.as_ref()), "—");
    }

    #[test]
    fn remote_target_key_ignores_session_identity_but_separates_machines() {
        let a = SessionConfig::new(Protocol::Ssh);
        let mut renamed = a.clone();
        renamed.name = "另一条会话".to_string();
        assert_eq!(remote_target_key(&a), remote_target_key(&renamed));
        let mut other_host = a.clone();
        other_host.host = "10.0.0.2".to_string();
        assert_ne!(remote_target_key(&a), remote_target_key(&other_host));
        let mut other_user = a.clone();
        other_user.username = "root".to_string();
        assert_ne!(remote_target_key(&a), remote_target_key(&other_user));
        let mut other_port = a.clone();
        other_port.port = 2222;
        assert_ne!(remote_target_key(&a), remote_target_key(&other_port));
    }

    /// 切换远端源时，上一台机器的读数不能在新机器上继续显示。
    /// 同一台机器重复同步也不许清——否则每帧都归零，读数永远出不来。
    #[test]
    fn switching_remote_target_clears_the_previous_reading() {
        let stale = crate::sysmon_sample::Reading {
            cpu_pct: 77.7,
            ..Default::default()
        };
        set_remote_target(None);
        crate::sysmon_remote::clear();
        assert!(set_remote_target(Some("alice@10.0.0.1:22")));
        crate::sysmon_remote::seed_reading_for_test(Some(stale.clone()));
        assert_eq!(
            crate::sysmon_remote::reading().map(|r| r.cpu_pct),
            Some(77.7),
            "上一台机器的读数确实在快照里"
        );
        assert!(
            !set_remote_target(Some("alice@10.0.0.1:22")),
            "同一台不重置"
        );
        assert!(crate::sysmon_remote::reading().is_some());
        assert!(
            set_remote_target(Some("bob@10.0.0.2:2222")),
            "换机器必须重置"
        );
        assert!(
            crate::sysmon_remote::reading().is_none(),
            "上一台机器的读数不能留到新机器上"
        );
        crate::sysmon_remote::seed_reading_for_test(Some(stale));
        assert!(set_remote_target(None), "关掉开关也要重置");
        assert!(crate::sysmon_remote::reading().is_none());
        set_remote_target(None);
        crate::sysmon_remote::clear();
    }

