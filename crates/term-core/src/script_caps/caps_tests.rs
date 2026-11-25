use super::*;

#[test]
fn nothing_is_allowed_by_default() {
    let caps = Capabilities::NONE;
    assert!(caps.check_read(Path::new("/tmp/x")).is_err());
    assert!(caps.check_write(Path::new("/tmp/x")).is_err());
    assert!(caps.check_process().is_err());
    assert!(caps.check_clipboard().is_err());
    assert!(caps.check_ui().is_err());
}

#[test]
fn a_granted_root_allows_itself_and_descendants_but_not_siblings() {
    let mut caps = Capabilities::NONE;
    caps.grant(&CapRequest::FsRead(PathBuf::from("/tmp/logs")));
    assert!(caps.check_read(Path::new("/tmp/logs")).is_ok());
    assert!(caps.check_read(Path::new("/tmp/logs/a/b.txt")).is_ok());
    // 前缀不是字符串前缀：`/tmp/logs2` 不在 `/tmp/logs` 下。
    assert!(caps.check_read(Path::new("/tmp/logs2/x")).is_err());
    assert!(caps.check_read(Path::new("/etc/passwd")).is_err());
    // 只授了读：写仍然拒。
    assert!(caps.check_write(Path::new("/tmp/logs/a.txt")).is_err());
}

#[test]
fn write_is_a_separate_tier_from_read() {
    let mut caps = Capabilities::NONE;
    caps.grant(&CapRequest::FsWrite(PathBuf::from("/tmp/out")));
    assert!(caps.check_write(Path::new("/tmp/out/report.txt")).is_ok());
    // 写授权同时意味着可读（要写一个文件总得能看见它）。
    assert!(caps.check_read(Path::new("/tmp/out/report.txt")).is_ok());
    assert!(caps.fs.writable());
}

/// 两个根各带各的可写性：`fs=read:/tmp` + `fs=write:/var/tmp` 时，
/// 旧实现的一张表 + 全局 `write` 布尔会让 `/tmp` 也跟着可写。
#[test]
fn writability_is_recorded_per_root_not_for_the_whole_envelope() {
    let caps = capabilities_from(&[
        CapRequest::FsRead(PathBuf::from("/tmp")),
        CapRequest::FsWrite(PathBuf::from("/var/tmp")),
    ]);
    assert_eq!(
        caps.fs.roots(),
        vec![PathBuf::from("/tmp"), PathBuf::from("/var/tmp")],
        "roots() 保留并集供展示"
    );
    assert!(caps.check_read(Path::new("/tmp/a.txt")).is_ok());
    assert!(caps.check_read(Path::new("/var/tmp/a.txt")).is_ok());
    assert!(caps.check_write(Path::new("/var/tmp/a.txt")).is_ok());
    let err = caps.check_write(Path::new("/tmp/a.txt")).unwrap_err();
    assert_eq!(err.capability, "fs.write");
    assert!(err.granted.contains("只授了读"), "{err}");
    assert!(err.granted.contains("/tmp"), "{err}");
}

/// 合并两个来源时，写根不能把另一来源的只读根一起抬成可写。
#[test]
fn merging_keeps_each_roots_own_write_flag() {
    let mut caps = capabilities_from(&[CapRequest::FsRead(PathBuf::from("/tmp"))]);
    caps.merge(&capabilities_from(&[CapRequest::FsWrite(PathBuf::from(
        "/var/tmp",
    ))]));
    assert!(caps.check_write(Path::new("/var/tmp/a")).is_ok());
    assert!(
        caps.check_write(Path::new("/tmp/a")).is_err(),
        "合并不提权只读根"
    );
    assert!(caps.check_read(Path::new("/tmp/a")).is_ok());

    // 同根先读后写会升级；先写后读不降档。
    let mut upgraded = capabilities_from(&[CapRequest::FsRead(PathBuf::from("/data"))]);
    upgraded.grant(&CapRequest::FsWrite(PathBuf::from("/data")));
    assert!(upgraded.check_write(Path::new("/data/a")).is_ok());
    let mut stable = capabilities_from(&[CapRequest::FsWrite(PathBuf::from("/data"))]);
    stable.grant(&CapRequest::FsRead(PathBuf::from("/data")));
    assert!(stable.check_write(Path::new("/data/a")).is_ok());
    assert_eq!(
        stable.fs.roots(),
        vec![PathBuf::from("/data")],
        "并集不重复"
    );
}

#[test]
fn relative_and_dotdot_paths_are_refused_before_the_prefix_check() {
    let mut caps = Capabilities::NONE;
    caps.grant(&CapRequest::FsWrite(PathBuf::from("/tmp/logs")));
    for bad in [
        Path::new("logs/a.txt"),
        Path::new("/tmp/logs/../../etc/passwd"),
        Path::new("/tmp/../etc/passwd"),
    ] {
        let err = caps.check_write(bad).unwrap_err();
        assert_eq!(err.capability, "fs.write");
        assert!(err.requested.contains("etc") || err.requested.contains("logs"));
    }
}

#[test]
fn denial_says_what_is_missing_what_was_asked_and_what_is_granted() {
    let caps = Capabilities::NONE;
    let err = caps.check_read(Path::new("/tmp/x")).unwrap_err();
    let text = err.to_string();
    assert!(text.contains("fs.read"), "{text}");
    assert!(text.contains("/tmp/x"), "{text}");
    assert!(text.contains("文件：拒绝"), "{text}");
}

#[test]
fn the_header_parser_stops_at_the_first_statement() {
    let script = "\
// 一段说明
// @cap fs=read:/tmp/in,process
let x = 1;
// @cap ui
";
    let requests = parse_header(script).expect("头部合法");
    assert_eq!(
        requests,
        vec![
            CapRequest::FsRead(PathBuf::from("/tmp/in")),
            CapRequest::Process
        ],
        "第一条语句之后的 @cap 不该生效（否则 include 进来的文本能提权）"
    );
}

#[test]
fn a_malformed_header_points_at_the_line() {
    let err = parse_header("// @cap fs=rw:/tmp\n").unwrap_err();
    assert!(err.0.contains("第 1 行"), "{err}");
    assert!(err.0.contains("read"), "{err}");
}

/// `app` 档的三条判据：语法、门禁、合并方向。
///
/// 这三条就是敢让 `app_state` / `app_panel_set` 只看一个字段的全部依据。
#[test]
fn the_app_scope_has_three_levels_and_a_strict_parser() {
    // 语法：单项 `app` = Tune；`app=configure` = Configure；写错档名报错。
    assert_eq!(
        parse_allow("app").expect("app 合法"),
        vec![CapRequest::App(AppScope::Tune)]
    );
    assert_eq!(
        parse_allow("app=configure").expect("app=configure 合法"),
        vec![CapRequest::App(AppScope::Configure)]
    );
    assert!(parse_allow("app=tune").is_err(), "只认 configure，不猜档名");
    assert!(parse_allow("app=").is_err());

    // 门禁：缺省全拒；`Tune` 只放读；`Configure` 两样都放。
    let none = Capabilities::NONE;
    assert!(none.check_app_view().is_err());
    assert!(none.check_app_configure().is_err());
    let tune = Capabilities {
        app: AppScope::Tune,
        ..Capabilities::NONE
    };
    assert!(tune.check_app_view().is_ok());
    assert!(tune.check_app_configure().is_err(), "Tune 不许写偏好");
    let configure = Capabilities {
        app: AppScope::Configure,
        ..Capabilities::NONE
    };
    assert!(configure.check_app_view().is_ok());
    assert!(configure.check_app_configure().is_ok());

    // 合并：更宽的一档胜出，且与顺序无关。
    let mut merged = tune.clone();
    merged.merge(&configure);
    assert_eq!(merged.app, AppScope::Configure);
    let mut reversed = configure;
    reversed.merge(&tune);
    assert_eq!(reversed.app, AppScope::Configure, "合并与顺序无关");
    // `grant` 幂等：同档授两次不改变结果。
    let mut twice = tune.clone();
    twice.grant(&CapRequest::App(AppScope::Tune));
    assert_eq!(twice.app, AppScope::Tune);
}

#[test]
fn allow_specs_round_trip_and_reject_unknown_items() {
    let requests = parse_allow("fs=read:/tmp/in, fs=write:/tmp/out ,ui").expect("合法");
    assert_eq!(
        requests,
        vec![
            CapRequest::FsRead(PathBuf::from("/tmp/in")),
            CapRequest::FsWrite(PathBuf::from("/tmp/out")),
            CapRequest::Ui
        ]
    );
    for bad in ["proces", "fs=read", "fs=rw:/tmp", "fs=read:", ""] {
        if bad.is_empty() {
            assert!(parse_allow(bad).expect("空串合法（无授权）").is_empty());
        } else {
            assert!(parse_allow(bad).is_err(), "{bad:?} 应当报错");
        }
    }
}

#[test]
fn merging_keeps_the_stronger_of_each_tier() {
    let mut caps = capabilities_from(&[CapRequest::FsRead(PathBuf::from("/a"))]);
    caps.merge(&capabilities_from(&[
        CapRequest::FsWrite(PathBuf::from("/b")),
        CapRequest::Ui,
    ]));
    assert!(caps.ui);
    assert!(caps.fs.writable());
    assert!(caps.check_read(Path::new("/a/x")).is_ok());
    assert!(caps.check_write(Path::new("/b/x")).is_ok());
}
