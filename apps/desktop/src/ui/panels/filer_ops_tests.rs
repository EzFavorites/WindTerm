use super::*;

fn op_new(dir: &str) -> FilerOp {
    FilerOp::NewFolder {
        dir: dir.to_string(),
        name: String::new(),
    }
}

#[test]
fn a_name_with_a_slash_is_refused_before_anything_is_sent() {
    assert!(validate_name("logs").is_ok());
    assert!(validate_name("my logs 2026").is_ok());
    for bad in ["", "   ", ".", "..", "a/b"] {
        assert!(validate_name(bad).is_err(), "{bad:?} 应当被拒");
    }
    // 中文与空格是合法名字——不要顺手把宽字符也拦了。
    assert!(validate_name("日志 备份").is_ok());
}

#[test]
fn the_mode_parser_takes_three_and_four_digits_and_nothing_else() {
    assert_eq!(parse_mode("755"), Ok(0o755));
    assert_eq!(parse_mode("0755"), Ok(0o755));
    assert_eq!(parse_mode(" 644 "), Ok(0o644));
    assert_eq!(parse_mode("0o750"), Ok(0o750));
    for bad in ["", "8", "abc", "7555x", "77777", "0x1f"] {
        assert!(parse_mode(bad).is_err(), "{bad:?} 应当被拒");
    }
}

#[test]
fn joining_keeps_a_single_slash_even_at_the_root() {
    assert_eq!(join_path("/home/deploy", "logs"), "/home/deploy/logs");
    assert_eq!(join_path("/", "logs"), "/logs");
    assert_eq!(join_path("/home/deploy/", "logs"), "/home/deploy/logs");
}

#[test]
fn planning_a_new_folder_turns_the_typed_name_into_one_absolute_path() {
    let op = op_new("/srv/www");
    assert_eq!(
        plan(&op, " logs ").expect("合法名字"),
        OpRequest::Mkdir {
            path: "/srv/www/logs".to_string()
        }
    );
    // 坏名字**不产生请求**——这样调用方就没机会先发出去再报错。
    assert!(plan(&op, "a/b").is_err());
}

#[test]
fn renaming_stays_inside_the_same_directory() {
    let op = FilerOp::Rename {
        from: "/srv/www/old.conf".to_string(),
        name: String::new(),
    };
    assert_eq!(
        plan(&op, "new.conf").expect("合法名字"),
        OpRequest::Rename {
            from: "/srv/www/old.conf".to_string(),
            to: "/srv/www/new.conf".to_string(),
        }
    );
}

#[test]
fn deleting_a_folder_asks_for_remove_dir_and_a_file_for_remove_file() {
    let dir = FilerOp::Delete {
        path: "/srv/emptydir".to_string(),
        is_dir: true,
    };
    let file = FilerOp::Delete {
        path: "/srv/a.txt".to_string(),
        is_dir: false,
    };
    assert_eq!(
        plan(&dir, "").expect("删除不需要输入"),
        OpRequest::RemoveDir {
            path: "/srv/emptydir".to_string()
        }
    );
    assert_eq!(
        plan(&file, "").expect("删除不需要输入"),
        OpRequest::RemoveFile {
            path: "/srv/a.txt".to_string()
        }
    );
    // 两个按钮文案必须不同：「Delete」对文件和空目录做的事不一样，
    // 用户点之前该看得出来（非空目录会被服务端拒，这是有意的）。
    assert!(dir.body().contains("empty folder"));
    assert!(file.body().contains("file"));
}

#[test]
fn every_operation_refreshes_the_directory_it_touched() {
    assert_eq!(
        refresh_dir(&FilerOp::NewFolder {
            dir: "/srv/www".to_string(),
            name: String::new()
        }),
        "/srv/www"
    );
    assert_eq!(
        refresh_dir(&FilerOp::Delete {
            path: "/srv/www/a.txt".to_string(),
            is_dir: false
        }),
        "/srv/www"
    );
    assert_eq!(
        refresh_dir(&FilerOp::DeleteTree {
            path: "/srv/www/tree".to_string()
        }),
        "/srv/www"
    );
}

#[test]
fn copying_a_path_refuses_without_a_session_on_a_read_only_tab_or_with_no_path() {
    assert_eq!(path_to_terminal_blocker("/srv/www", true, false), None);
    assert_eq!(
        path_to_terminal_blocker("/srv/www", false, false),
        Some(PathToTerminalBlock::NoSession)
    );
    assert_eq!(
        path_to_terminal_blocker("/srv/www", true, true),
        Some(PathToTerminalBlock::ReadOnly)
    );
    assert_eq!(
        path_to_terminal_blocker("", true, false),
        Some(PathToTerminalBlock::EmptyPath)
    );
}

#[test]
fn the_path_sent_to_the_terminal_is_the_whole_path_without_an_enter() {
    assert_eq!(path_to_terminal_text("/srv/www with space/"), "/srv/www with space/");
    for text in ["/srv/www", "中文目录", "/Users/unique/"] {
        let sent = path_to_terminal_text(text);
        assert!(!sent.contains('\r'), "F7 不替用户按回车：{text:?}");
        assert!(!sent.contains('\n'), "F7 不吐进命令历史：{text:?}");
    }
}

#[test]
fn filer_chrome_keys_are_translated_with_the_tsv_wording() {
    use term_app::i18n::{t, Lang};
    for (key, zh) in [
        ("Download", "下载"),
        ("Upload To Current Folder", "上传到当前文件夹"),
        ("Sync Terminal Folder", "同步终端文件夹"),
        ("Copy Directory Path To Terminal", "将目录路径复制到终端"),
        ("Automatically Sync Terminal Folder", "自动同步终端文件夹"),
        ("No session", "无会话"),
        ("Not available", "不可用"),
    ] {
        assert_eq!(t(Lang::Zh, key), zh, "{key} 的中文没有按 TSV 登记");
        assert_eq!(t(Lang::En, key), key, "英文界面必须显示原文");
    }
}
