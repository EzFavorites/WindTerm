//! 树传输的**目标**路径拼装。
//!
//! 与 `walk::local_path_for` / `remote_path_for` 的区别：那两个是源端也用的
//! 纯拼路径，这里只服务目标侧——只有目标侧才需要按会话开关把非法字符
//! 换成 `_`。放在独立文件是为了让 `walk.rs` 守住单文件行数门禁。

use std::path::PathBuf;

use crate::entry;

/// 目标侧本地路径：`fix_invalid` 为真时逐段替换非法字符。
pub(super) fn local_dest_for(root: &str, rel: &str, fix_invalid: bool) -> PathBuf {
    if !fix_invalid {
        return super::walk::local_path_for(root, rel);
    }
    let mut p = PathBuf::from(root);
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            continue;
        }
        p.push(crate::mutate::sanitize_component(part));
    }
    p
}

/// 目标侧远端路径：`fix_invalid` 为真时逐段替换非法字符。
pub(super) fn remote_dest_for(root: &str, rel: &str, fix_invalid: bool) -> String {
    if !fix_invalid {
        return super::walk::remote_path_for(root, rel);
    }
    if rel.is_empty() {
        return root.to_string();
    }
    let cleaned = rel
        .split('/')
        .map(crate::mutate::sanitize_component)
        .collect::<Vec<_>>()
        .join("/");
    entry::join(root, &cleaned)
}
