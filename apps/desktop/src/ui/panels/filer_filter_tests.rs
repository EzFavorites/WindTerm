use super::*;

fn row(name: &str) -> FileLine {
    FileLine {
        name: name.to_string(),
        raw_name: name.trim_end_matches('/').to_string(),
        is_dir: name.ends_with('/'),
        is_symlink: false,
        size: 0,
        date: String::new(),
        target: name.to_string(),
    }
}

#[test]
fn filter_matches_file_names_case_insensitively_and_keeps_empty_query_a_noop() {
    let mut rows = vec![row("readme.md"), row("src/"), row("notes.txt")];
    filter_lines(&mut rows, "");
    assert_eq!(rows.len(), 3, "空过滤词不该改列表");
    filter_lines(&mut rows, "MD");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "readme.md");
    let mut dirs = vec![row("src/"), row("notes.txt")];
    filter_lines(&mut dirs, "src"); // 目录显示名带尾斜杠仍要命中
    assert_eq!(dirs.len(), 1);
    assert_eq!(dirs[0].name, "src/");
}
