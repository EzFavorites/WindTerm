//! `filer_view::row_salt` 的唯一性回归：显示名碰撞不许共用行热区 id。

use super::{row_salt, FileLine};

fn line(name: &str, raw_name: &str, target: &str) -> FileLine {
    FileLine {
        name: name.to_string(),
        raw_name: raw_name.to_string(),
        is_dir: false,
        is_symlink: false,
        size: 0,
        date: String::new(),
        target: target.to_string(),
    }
}

#[test]
fn a_plain_file_and_a_symlink_with_the_same_display_name_get_different_ids() {
    // 普通文件真的叫 `foo@`；链接指向 `foo`，`row_label` 给它加 `@` 后屏上同名。
    let plain = line("foo@", "foo@", "/srv/foo@");
    let link = line("foo@", "foo", "/srv/foo");
    assert_eq!(plain.name, link.name, "前提：显示名确实撞了");
    let salt = row_salt(&plain);
    assert_ne!(salt, row_salt(&link), "真名 + target 必须把两行分开");
    assert_eq!(salt, row_salt(&plain), "同一行恒等");
}

#[test]
fn the_same_raw_name_in_two_directories_is_two_rows() {
    let a = line("README.md", "README.md", "/srv/a/README.md");
    let b = line("README.md", "README.md", "/srv/b/README.md");
    assert_ne!(row_salt(&a), row_salt(&b), "同名不同目录也要分开");
}

#[test]
fn the_salt_never_uses_the_display_name() {
    // 只改显示名（比如以后给它加别的标签后缀）不该影响 salt。
    let a = line("foo/", "foo", "/srv/foo");
    let b = line("foo@", "foo", "/srv/foo");
    assert_eq!(row_salt(&a), row_salt(&b));
}
