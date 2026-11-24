//! Filer 面板单测（从 `filer.rs` 拆出，守 500 行门禁）。

use super::*;

    use super::*;

    fn entry(name: &str, is_dir: bool, is_symlink: bool) -> Entry {
        Entry {
            name: name.to_string(),
            is_dir,
            is_symlink,
            size: 0,
            // 夹具不关心 mtime：显示层不用它，只有目录同步的判定才用。
            mtime: None,
        }
    }

    fn line(name: &str, is_dir: bool, date: &str, size: u64) -> FileLine {
        FileLine {
            name: name.to_string(),
            raw_name: name.to_string(),
            is_dir,
            is_symlink: false,
            size,
            date: date.to_string(),
            target: name.to_string(),
        }
    }

    #[test]
    fn filer_uses_sftp_only_with_both_credentials_and_the_session_switch() {
        let mut config = SessionConfig::new(Protocol::Ssh);
        // 两个条件缺一不可：没凭据（本地 shell / 系统 ssh）看本地；
        // 有凭据但用户关掉 Sftp 子系统，也看本地用户目录。
        assert!(!filer_uses_sftp(&config, false), "无凭据不该起 SFTP");
        config.sftp = false;
        assert!(!filer_uses_sftp(&config, true), "关掉 Sftp 不该起 SFTP");
        config.sftp = true;
        assert!(filer_uses_sftp(&config, true), "两项都满足才起 SFTP");
    }

    #[test]
    fn directories_carry_a_trailing_slash_and_links_an_at_sign() {
        assert_eq!(row_label(&entry("etc", true, false)), "etc/");
        assert_eq!(row_label(&entry("notes.txt", false, false)), "notes.txt");
        // 链接指向目录时 `is_dir` 为假——两个标记都要在，信息才不丢。
        assert_eq!(row_label(&entry("latest", false, true)), "latest@");
    }

    #[test]
    fn the_up_button_knows_when_there_is_no_up() {
        assert_eq!(up_target("/home/demo").as_deref(), Some("/home"));
        assert_eq!(up_target("/home").as_deref(), Some("/"));
        assert_eq!(up_target("/"), None, "根目录没有上一级");
        assert_eq!(up_target(""), None, "还不知道路径时不要给方向");
    }

    #[test]
    fn sorting_keeps_directories_on_top_in_every_mode() {
        let mut lines = vec![
            line("a.txt", false, "2026/11/01", 900),
            line("Zeta", true, "2026/11/02", 0),
            line("b.txt", false, "2026/11/03", 10),
        ];
        for by in [SortBy::Name, SortBy::Date, SortBy::Size] {
            sort_lines(&mut lines, by);
            assert_eq!(lines[0].name, "Zeta", "{by:?} 把文件排到了目录前面");
        }
    }

    #[test]
    fn sorting_by_size_and_date_really_changes_the_order() {
        let mut by_date = vec![
            line("a", false, "2026/11/01", 1),
            line("b", false, "2026/11/09", 2),
        ];
        sort_lines(&mut by_date, SortBy::Date);
        assert_eq!(by_date[0].name, "b");
        let mut by_size = vec![
            line("a", false, "2026/11/01", 1),
            line("b", false, "2026/11/09", 2),
        ];
        sort_lines(&mut by_size, SortBy::Size);
        assert_eq!(by_size[0].name, "b");
        let mut by_name = vec![line("z", false, "", 9), line("a", false, "", 1)];
        sort_lines(&mut by_name, SortBy::Name);
        assert_eq!(by_name[0].name, "a");
    }

    #[test]
    fn the_sort_button_cycles_through_all_three_modes() {
        assert_eq!(SortBy::default().next(), SortBy::Date);
        assert_eq!(SortBy::Date.next(), SortBy::Size);
        assert_eq!(SortBy::Size.next(), SortBy::Name);
    }

    #[test]
    fn a_fresh_panel_starts_from_the_factory_hidden_switch() {
        // `Filer::default()` 是「面板还没绑会话」时的初值，它必须等于出厂偏好
        // （不显示隐藏项）；两边一旦分叉，勾上的开关会在重建后自己弹回去。
        assert!(!Filer::default().show_hidden);
        assert_eq!(
            Filer::default().show_hidden,
            term_runtime::prefs::Prefs::default().filer_show_hidden
        );
    }
