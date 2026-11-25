//! 删整棵树的 UI 侧单测：确认语义、步骤顺序、扫描收件箱的认领规则。
//!
//! 这里是 `filer_ops.rs` 的 `#[cfg(test)]` 子模块；所有断言都不连 SSH，
//! 只验「用户没点确认就什么都不会发」这件事的可测部分。

use super::*;

fn scanned(rel: &str, is_dir: bool) -> term_sftp::ScannedEntry {
    term_sftp::ScannedEntry {
        rel: rel.to_string(),
        is_dir,
        size: 0,
        mtime: None,
    }
}

/// 整棵树不是「删空目录」的一条快路径：它有自己的确认文案，而且 `plan`
/// 对它是**拒绝**的——唯一的落地口是确认窗里那次 `plan_tree_delete`。
#[test]
fn deleting_a_tree_has_its_own_explicit_confirmation() {
    let op = FilerOp::DeleteTree {
        path: "/srv/tree".to_string(),
    };
    assert_eq!(op.heading(), "Delete tree");
    assert_eq!(op.confirm_label(), "Delete tree");
    assert!(!op.wants_input());
    assert!(op.body().contains("whole tree"));
    assert!(plan(&op, "").is_err(), "不能靠填名字绕过扫描与确认");
}

/// UI 这一层只把 crate 的计划映射到既有的两种请求，顺序本身由 `term_sftp::delete`
/// 的单测钉住；这里守的是「不会冒出第三种请求」。
#[test]
fn a_tree_becomes_existing_remove_requests_files_first() {
    let entries = [
        scanned("sub", true),
        scanned("sub/deep/a.txt", false),
        scanned("b.txt", false),
        scanned("empty", true),
    ];
    let reqs = plan_tree_delete("/srv/tree", &entries).expect("合法清单");
    assert_eq!(
        reqs,
        vec![
            OpRequest::RemoveFile {
                path: "/srv/tree/b.txt".to_string()
            },
            OpRequest::RemoveFile {
                path: "/srv/tree/sub/deep/a.txt".to_string()
            },
            OpRequest::RemoveDir {
                path: "/srv/tree/sub".to_string()
            },
            OpRequest::RemoveDir {
                path: "/srv/tree/empty".to_string()
            },
            OpRequest::RemoveDir {
                path: "/srv/tree".to_string()
            },
        ]
    );
    // 空树也要给出删根的那一刀，否则用户以为删了、其实留了个空壳。
    assert_eq!(
        plan_tree_delete("/srv/tree", &[]).expect("空清单"),
        vec![OpRequest::RemoveDir {
            path: "/srv/tree".to_string()
        }]
    );
    // 脏 rel 整棵拒绝：宁可让用户重试，也不能删到树外。
    assert!(plan_tree_delete("/srv/tree", &[scanned("../evil", false)]).is_err());
}

/// 确认窗说「含根自身」，就真得把根算进去：`ScannedEntry` 里只有子孙，
/// 根自身那条删请求是 `plan_tree_delete` 额外补的。少报一个会让用户低估规模。
#[test]
fn tree_scan_status_counts_the_root_directory_itself() {
    let entries = [
        scanned("sub", true),
        scanned("sub/deep/a.txt", false),
        scanned("b.txt", false),
        scanned("empty", true),
    ];
    assert_eq!(
        tree_status::tree_scan_status(&entries),
        "扫描完成：2 个文件、3 个目录（含根自身）",
        "清单里 2 个子目录 + 根自身 = 3"
    );
    // 空树也得报 1 个目录：那是唯一的一条删根请求。
    assert_eq!(
        tree_status::tree_scan_status(&[]),
        "扫描完成：0 个文件、1 个目录（含根自身）"
    );
}

/// 扫描收件箱按根路径认领：旧清单 / 别的树的清单都不能顶替当前确认的那棵。
#[test]
fn a_stale_or_failed_scan_cannot_arm_the_confirmation() {
    let mut filer = Filer {
        tree_delete: Some(TreeDeleteScan::begin("/srv/tree".to_string())),
        ..Filer::default()
    };
    filer.note_tree_delete_scan("/srv/other", vec![scanned("x", false)]);
    filer.fail_tree_delete_scan("/srv/other", "denied");
    assert!(
        filer
            .tree_delete
            .as_ref()
            .is_some_and(|s| s.entries.is_none() && s.error.is_none())
    );
    filer.note_tree_delete_scan("/srv/tree", vec![scanned("x", false)]);
    assert!(filer.tree_delete.as_ref().is_some_and(|s| s.entries.is_some()));
    // 失败挂在自己这棵树上时，清单仍缺位——确认按钮不会因此被点亮。
    let mut filer = Filer {
        tree_delete: Some(TreeDeleteScan::begin("/srv/tree".to_string())),
        ..Filer::default()
    };
    filer.fail_tree_delete_scan("/srv/tree", "Permission denied");
    assert!(filer.tree_delete.as_ref().is_some_and(|s| s.entries.is_none() && s.error.is_some()));
}
