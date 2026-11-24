//! 递归删除的**纯计划**：把一棵树的清单收成「先删文件、后序删目录」的步骤序列。
//!
//! 这里不碰 `SftpSession`，也不新增请求类型：产出的 [`DeleteStep`] 与
//! [`crate::Request::RemoveFile`] / [`crate::Request::RemoveDir`] 一一对应，由
//! **调用方在显式确认之后**逐条发给已有的后台会话（见
//! `apps/desktop/src/ui/panels/filer_ops.rs`）。因此「删整棵树」与单条删除走的是
//! 同一条串行通道、同一套 `Event::Done` / `Event::Failed` 语义，不另开并行路径。
//!
//! 树的来源复用 [`crate::walk::scan_remote_entries`]：软链跳过、根自身不在清单里、
//! 每段名字经过过滤。但计划函数仍自己校验一次 `rel`——它是 `pub` 接口，调用方
//! 手里可能已经有脏数据；宁可整棵拒绝，也不让一条 `../` 变成删到树外。
//!
//! 顺序的两条规则：
//!
//! 1. **文件先删完**：任何目录的删除请求发出去时，它一定是空的（服务端不需要
//!    替我们递归，也不会回一句莫名其妙的 `Directory not empty`）。
//! 2. **目录后序**：子路径一定以父路径为前缀，按路径**降序**发就保证最深的先删。
//!    同层顺序不敏感，用排序换确定性，便于单测钉住与复现。

use std::fmt;

use crate::walk::{ScannedEntry, remote_path_for};

/// 一次删除动作，与后台的两条既有请求一一对应。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeleteStep {
    /// 对应 `Request::RemoveFile`。
    File(String),
    /// 对应 `Request::RemoveDir`（服务端只删空目录）。
    Dir(String),
}

impl DeleteStep {
    /// 这一步要删的远端绝对路径。
    pub fn path(&self) -> &str {
        match self {
            DeleteStep::File(path) | DeleteStep::Dir(path) => path,
        }
    }
}

/// 计划阶段就能判定的错误。分类而不是一句 `String`：根不对与清单里有脏路径
/// 是两件事，前者提示用户换目标，后者说明扫描结果被污染了。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    /// 根路径本身不合法（空串尤其危险：服务端会把空路径当成当前目录）。
    BadRoot(String),
    /// 清单里的相对路径不安全。
    UnsafeEntry {
        rel: String,
        why: &'static str,
    },
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::BadRoot(why) => write!(f, "删除树的根路径不合法：{why}"),
            PlanError::UnsafeEntry { rel, why } => {
                write!(f, "树清单里有不安全的相对路径 {rel:?}：{why}")
            }
        }
    }
}

impl std::error::Error for PlanError {}

/// 相对路径必须是「一段段干净的名字」。与 `walk` 的入口过滤同一套判据，
/// 但这里再守一次：计划函数是公开的，调用方传进来的清单未必来自 `walk`。
fn check_rel(rel: &str) -> Result<(), PlanError> {
    let fail = |why| PlanError::UnsafeEntry {
        rel: rel.to_string(),
        why,
    };
    if rel.is_empty() {
        return Err(fail("空路径"));
    }
    if rel.starts_with('/') {
        return Err(fail("绝对路径"));
    }
    if rel.contains('\0') {
        return Err(fail("含 NUL"));
    }
    for part in rel.split('/') {
        if part.is_empty() {
            return Err(fail("含空段"));
        }
        if part == "." || part == ".." {
            return Err(fail("含 `.` / `..` 段"));
        }
        if part.contains('\\') {
            return Err(fail("含反斜杠"));
        }
    }
    Ok(())
}

/// 把一棵树的清单展开成删除步骤：所有文件在前，目录 deepest-first 在后。
///
/// `root` 是用户右键点中的那棵树的根；`entries` 里**不含根自身**（与扫描一致），
/// 所以最后一条 `DeleteStep::Dir` 才是删根的那一刀。
///
/// 空清单也是合法输入：返回一条删根目录的步骤（空目录本身也要删掉）。
pub fn plan_tree_delete(
    root: &str,
    entries: &[ScannedEntry],
) -> Result<Vec<DeleteStep>, PlanError> {
    crate::mutate::check_path(root).map_err(PlanError::BadRoot)?;
    let mut files: Vec<String> = Vec::with_capacity(entries.len());
    let mut dirs: Vec<String> = Vec::new();
    for e in entries {
        check_rel(&e.rel)?;
        let path = remote_path_for(root, &e.rel);
        if e.is_dir {
            dirs.push(path);
        } else {
            files.push(path);
        }
    }
    files.sort();
    dirs.sort();
    let mut steps: Vec<DeleteStep> = files.into_iter().map(DeleteStep::File).collect();
    // 降序：子路径以父路径为前缀，字典序里排在父后面，倒过来就是后序。
    steps.extend(dirs.into_iter().rev().map(DeleteStep::Dir));
    // 扫描清单不含根自身，但「删整棵树」必须连根删掉——漏了这一步，用户会
    // 看到一个空壳目录留在原处，以为删除失败了。
    steps.push(DeleteStep::Dir(root.to_string()));
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(rel: &str, is_dir: bool) -> ScannedEntry {
        ScannedEntry {
            rel: rel.to_string(),
            is_dir,
            size: 0,
            mtime: None,
        }
    }

    fn paths(steps: &[DeleteStep]) -> Vec<&str> {
        steps.iter().map(DeleteStep::path).collect()
    }

    /// 核心契约：先把文件删干净，再按后序删目录。把任一目录提到文件之前，
    /// 服务端就会回 `Directory not empty`，整棵树半死不活。
    #[test]
    fn files_come_before_directories() {
        let steps = plan_tree_delete(
            "/srv/tree",
            &[
                e("sub", true),
                e("sub/inner.txt", false),
                e("a.txt", false),
                e("empty", true),
            ],
        )
        .expect("合法清单");
        assert_eq!(
            paths(&steps),
            [
                "/srv/tree/a.txt",
                "/srv/tree/sub/inner.txt",
                "/srv/tree/sub",
                "/srv/tree/empty",
                "/srv/tree",
            ]
        );
        let first_dir = steps.iter().position(|s| matches!(s, DeleteStep::Dir(_))).unwrap();
        assert!(steps[..first_dir].iter().all(|s| matches!(s, DeleteStep::File(_))));
    }

    /// 空目录没有文件可删，但仍要把根目录本身删掉。
    #[test]
    fn an_empty_tree_is_one_remove_dir() {
        let steps = plan_tree_delete("/srv/empty", &[]).expect("空清单合法");
        assert_eq!(paths(&steps), ["/srv/empty"]);
        assert!(matches!(steps[0], DeleteStep::Dir(_)));
    }

    /// 深树：每一层的父目录都必须排在自己的孩子后面。
    #[test]
    fn a_deep_tree_deletes_directories_in_post_order() {
        let steps = plan_tree_delete(
            "/r",
            &[
                e("a", true),
                e("a/b", true),
                e("a/b/c", true),
                e("a/b/c/deep.txt", false),
                e("a/top.txt", false),
            ],
        )
        .expect("深树合法");
        assert_eq!(
            paths(&steps),
            [
                "/r/a/b/c/deep.txt",
                "/r/a/top.txt",
                "/r/a/b/c",
                "/r/a/b",
                "/r/a",
                "/r",
            ]
        );
        for parent in ["/r/a", "/r/a/b", "/r/a/b/c"] {
            let at = paths(&steps).iter().position(|p| *p == parent).unwrap();
            assert!(steps[at..].iter().all(|s| !s.path().starts_with(&format!("{parent}/"))));
        }
    }

    /// 计划不认识「相对根」以外的路径：一条 `../` 就能把删除写到树外。
    #[test]
    fn unsafe_relative_paths_are_rejected_whole() {
        for bad in ["", "/abs", "../evil", "a/../../b", "a//b", "a/./b", "a\\b", "a\0b"] {
            assert!(
                plan_tree_delete("/r", &[e("ok.txt", false), e(bad, false)]).is_err(),
                "{bad:?} 应当被拒"
            );
        }
        // 合法名字照常通过：中文、空格、点开头的文件名都不该误伤。
        assert!(plan_tree_delete("/r", &[e(".hidden", false), e("中文 名", false)]).is_ok());
    }

    #[test]
    fn an_empty_root_is_refused_before_any_request_can_be_sent() {
        assert!(matches!(plan_tree_delete("", &[]), Err(PlanError::BadRoot(_))));
        assert!(plan_tree_delete("/r", &[]).is_ok());
    }

    #[test]
    fn a_trailing_slash_on_the_root_does_not_double_the_separator() {
        let steps = plan_tree_delete("/r/", &[e("a.txt", false)]).expect("合法");
        assert_eq!(paths(&steps), ["/r/a.txt", "/r/"]);
    }
}
