//! 删整棵树的扫描文案：`entries` 不含根自身，文案却承诺「含根自身」。
//!
//! `term_sftp::ScannedEntry::rel` 的契约写得很清楚——根两边都不进清单。把清单里的
//! 目录数直接报给用户会少算一棵树的根，确认窗于是说「2 个目录」而实际要删 3 个。
//! 这条换算独立成一个纯函数，既方便钉断言，也不让 `filer_ops.rs` 继续变长。

/// 把一份整树扫描清单收成确认窗里的一行状态文案。
///
/// 目录数 `+1` 补的是根自身：清单里只有它的**子孙**，`plan_tree_delete` 却一定会
/// 发一条删根的请求（空树也一样）。文件数不加，因为根不是文件。
pub(in crate::ui) fn tree_scan_status(entries: &[term_sftp::ScannedEntry]) -> String {
    let files = entries.iter().filter(|e| !e.is_dir).count();
    let dirs = entries.len() - files + 1;
    format!("扫描完成：{files} 个文件、{dirs} 个目录（含根自身）")
}
