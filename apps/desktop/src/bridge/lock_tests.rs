//! 回归：锁屏探测三态语义、fail-open 可审计性、1.0 平台声明。
//!
//! 单独成文件（由 `lock.rs` 的 `#[cfg(test)] #[path = "lock_tests.rs"]` 引入），
//! 是为了让「判据本身」与「怎么问系统」分开测：这里全部用注入的探针或纯函数，
//! 不依赖机器是否真的锁了屏，也不需要人工介入。
//!
//! 注意裁定：`None` 仍然**放行**（有意的产品决策），所以这些用例钉的是
//! 「放行 + 留痕 + 平台事实」，不是把 fail-open 翻成拒绝服务。

use super::*;

/// 固定结论的探针，三态都由测试直接给定。
struct Fixed(Option<bool>);

impl LockProbe for Fixed {
    fn locked(&self) -> Option<bool> {
        self.0
    }
}

fn long_window() -> Duration {
    Duration::from_secs(60)
}

#[test]
fn three_states_have_explicit_allow_deny_meanings() {
    assert!(!permits(Some(true)), "Some(true)：锁屏中，必须拒绝");
    assert!(permits(Some(false)), "Some(false)：没锁，必须放行");
    assert!(
        permits(None),
        "None：探测失败按 1.0 产品决策放行；不得把它翻成拒绝服务"
    );
    // 与 `bridge/mod.rs` 里 `handle` / `LockedGate` 现行的 `== Some(true)` 等价：
    // 收口函数不能悄悄改变线上语义。
    for value in [Some(true), Some(false), None] {
        assert_eq!(permits(value), value != Some(true), "三态={value:?}");
    }
}

#[test]
fn every_platform_judge_covers_the_three_states() {
    let locked = r#"+-o Root  <class IORegistryEntry>
  {
    "IOConsoleUsers" = ({"CGSSessionScreenIsLocked" = Yes})
  }"#;
    let unlocked = r#"+-o Root
  {
    "IOKitBuildVersion" = "..."
  }"#;
    // 三个平台的「怎么读 OS 的答复」都是纯函数，本机（macOS）一次就能全测完，
    // 不需要真的锁一次屏，也不需要 Windows / Linux 机器。
    assert_eq!(decide(true, Some(locked)), Some(true), "macOS 锁屏中");
    assert_eq!(decide(true, Some(unlocked)), Some(false), "macOS 没锁");
    assert_eq!(decide(true, None), None, "ioreg 没跑起来 = 不知道");
    assert_eq!(decide(true, Some("")), None, "空输出不算「没锁」");

    // Windows：输入桌面名与 OpenInputDesktop 失败码。
    assert_eq!(judge_windows_desktop("Winlogon"), Some(true));
    assert_eq!(judge_windows_desktop(" screen-saver "), Some(true));
    assert_eq!(judge_windows_desktop("Default"), Some(false));
    assert_eq!(
        judge_windows_desktop("SomeOtherDesktop"),
        None,
        "不认识的桌面名不许猜"
    );
    assert_eq!(
        judge_windows_error(5),
        Some(true),
        "锁屏时 ERROR_ACCESS_DENIED 视为锁屏"
    );
    assert_eq!(judge_windows_error(0), None, "其它错误是「问不出来」");

    // Linux：ScreenSaver.GetActive 的输出。
    assert_eq!(judge_screensaver("(<true>)"), Some(true));
    assert_eq!(judge_screensaver("(<false>)"), Some(false));
    assert_eq!(judge_screensaver("(true)"), Some(true));
    assert_eq!(
        judge_screensaver("method return time=... sender=... ( <true>, )"),
        Some(true),
        "gdbus 的 method-return 头不影响结论"
    );
    assert_eq!(judge_screensaver(""), None);
    assert_eq!(judge_screensaver("command not found"), None);
    assert_eq!(
        judge_screensaver("true false"),
        None,
        "自相矛盾的输出判「问不出来」"
    );
}

#[test]
fn an_unsupported_platform_never_claims_locked() {
    // 即便喂一份「锁屏中」的 ioreg 文本，没有实现的平台也必须明确返回 None：
    // 不能假装知道，更不能凭一份不属于本平台的输出把服务关掉。
    let locked = r#""CGSSessionScreenIsLocked" = Yes"#;
    assert_eq!(decide(false, Some(locked)), None);
    assert_eq!(decide(false, None), None);
    assert_eq!(
        platform_has_probe(),
        cfg!(any(target_os = "macos", target_os = "windows", target_os = "linux")),
        "平台声明：macOS / Windows / Linux 有实现，其余平台没有"
    );
}

#[test]
fn a_none_probe_is_audited_on_every_decision_including_cache_hits() {
    let probe = Cached::new(Fixed(None), long_window());
    assert_eq!(probe.unknown_hits(), 0, "新探针不该凭空有 fail-open 记录");
    assert_eq!(probe.locked(), None);
    assert_eq!(probe.locked(), None, "缓存命中也必须返回 None");
    assert_eq!(probe.locked(), None);
    assert_eq!(
        probe.unknown_hits(),
        3,
        "每次因探测失败而放行的判定都要留痕；缓存窗口里的命中同样算"
    );
}

#[test]
fn known_states_are_not_counted_as_fail_open() {
    let unlocked = Cached::new(Fixed(Some(false)), long_window());
    for _ in 0..3 {
        assert_eq!(unlocked.locked(), Some(false));
    }
    assert_eq!(unlocked.unknown_hits(), 0, "明确没锁不是探测失败");

    let locked = Cached::new(Fixed(Some(true)), long_window());
    for _ in 0..3 {
        assert_eq!(locked.locked(), Some(true));
    }
    assert_eq!(
        locked.unknown_hits(),
        0,
        "Some(true) 是拒绝服务，更不是探测失败"
    );
}

#[test]
fn the_audit_line_is_greppable_and_states_the_platform_decision() {
    let on_supported = audit_line(true, 7);
    assert!(
        on_supported.contains(UNKNOWN_MARK),
        "审计行必须可 grep：{on_supported}"
    );
    assert!(
        on_supported.contains("放行"),
        "必须写明是放行：{on_supported}"
    );
    assert!(
        on_supported.contains("loopback 对端")
            && on_supported.contains("0600 key 文件")
            && on_supported.contains("HMAC-SHA256 挑战证明"),
        "UDS / 命名管道已随 ADR-0030 改版删除；必须写清真正的主闸门：{on_supported}"
    );
    assert!(
        !on_supported.contains("主闸门仍是 token"),
        "不得再留下过时的 token 说法：{on_supported}"
    );
    assert!(
        on_supported.contains("7"),
        "必须带上判定次数：{on_supported}"
    );

    let off_supported = audit_line(false, 1);
    assert!(
        off_supported.contains(UNKNOWN_MARK),
        "审计行必须可 grep：{off_supported}"
    );
    assert!(
        off_supported.contains("仅 macOS / Windows / Linux 有实现"),
        "未实现的平台必须说清「本平台未做锁屏探测」：{off_supported}"
    );
    assert!(
        off_supported.contains("HMAC-SHA256 挑战证明"),
        "未实现的平台也要写清主闸门：{off_supported}"
    );
}

#[test]
fn the_real_probe_audit_matches_its_own_lock_result() {
    // 真探针在任何平台上都得自洽：结论是 None 就恰好记一次 fail-open，
    // 结论明确就一次都不记。本机（macOS）跑的是 `ioreg` 分支；Windows 的
    // User32 与 Linux 的 dbus 分支未在本机运行验证，只做了 cfg 隔离与编译检查。
    #[cfg(target_os = "windows")]
    eprintln!("Windows 输入桌面探测：真 OS 调用未在本机（macOS）运行验证");
    #[cfg(target_os = "linux")]
    eprintln!("Linux ScreenSaver 探测：真 OS 调用未在本机（macOS）运行验证");
    let probe = Cached::new(Ioreg, Duration::from_secs(2));
    let value = probe.locked();
    let expected = if value.is_none() { 1 } else { 0 };
    assert_eq!(
        probe.unknown_hits(),
        expected,
        "真实探针结论={value:?} 时 fail-open 计数必须吻合"
    );
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
#[test]
fn unsupported_platform_probe_explicitly_returns_none() {
    assert_eq!(
        Ioreg.locked(),
        None,
        "未实现探测的平台必须明确返回 None（放行），不得返回 Some"
    );
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
#[test]
fn the_real_probe_is_audited_when_the_platform_has_no_lock_probe() {
    let probe = Cached::new(Ioreg, Duration::from_secs(2));
    assert_eq!(probe.locked(), None);
    assert!(
        probe.unknown_hits() >= 1,
        "未实现探测的平台上必须在运行时有 fail-open 计数可查"
    );
    assert!(
        audit_line(platform_has_probe(), probe.unknown_hits())
            .contains("仅 macOS / Windows / Linux 有实现"),
        "审计行必须把平台事实写清楚"
    );
}

// Windows 的 User32 调用与 Linux 的 dbus 调用都整段 `#[cfg]` 隔离在 lock.rs 里，
// 本机（macOS）不编译、也没有真实交互桌面 / 会话总线可跑，因此**未在本机运行验证**；
// 这里只把两者的纯判据（桌面名 / 错误码 / GetActive 文本）拉到本机测死。
// 真 OS 调用的自洽性由 `the_real_probe_audit_matches_its_own_lock_result` 在对应
// 平台上覆盖：结论是 None 就恰好记一次 fail-open，否则一次都不记。
