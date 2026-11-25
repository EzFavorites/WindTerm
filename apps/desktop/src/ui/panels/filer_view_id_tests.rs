//! `filer_view::row_id` 的唯一性回归：同名行跨目录 / 跨来源不得共用 id。

use super::row_id;

#[test]
fn a_name_alone_is_not_a_unique_row_id() {
    let same = row_id("README.md", "/srv/a", false);
    assert_eq!(same, row_id("README.md", "/srv/a", false), "同一行同一帧恒等");

    // 同名、不同目录：两个独立热区。
    assert_ne!(same, row_id("README.md", "/srv/b", false));
    // 同名、同目录、不同来源（本地树 vs 远端树）：也必须分开。
    assert_ne!(same, row_id("README.md", "/srv/a", true));
    // 两条 salt 同时变时自然也不相等。
    assert_ne!(same, row_id("README.md", "/srv/b", true));
}

#[test]
fn different_names_stay_different_within_one_directory() {
    assert_ne!(
        row_id("a.txt", "/srv/a", true),
        row_id("b.txt", "/srv/a", true)
    );
}
