//! X/Y/ZMODEM 接收侧的同名冲突策略（纯函数）。
//!
//! 文档列了 overwrite / newer / newer and longer / rename / skip 五档。这里只做
//! 判据：给一个目标路径与「对端声明的元信息」，返回该写哪个文件或跳过。
//! 不碰 IO、不碰时间：调用方负责 stat 并把结果传进来。

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum ConflictPolicy {
    /// 覆盖（当前行为，也是旧版唯一行为）。
    #[default]
    Overwrite,
    /// 源端更新才覆盖。
    Newer,
    /// 源端更新且更长才覆盖。
    NewerAndLonger,
    /// 改名写入（`file-1.bin`、`file-2.bin`…）。
    Rename,
    /// 目标已存在就跳过（数据照收但丢弃）。
    Skip,
}

/// 目标文件的已知事实。`None` 表示文件或时间未知。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Existing {
    pub modified_secs: Option<u64>,
    pub len: u64,
}

pub(crate) enum Decision {
    Write(PathBuf),
    Skip,
}

fn renamed(path: &Path) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    for n in 1..=9999u32 {
        let candidate = parent.join(format!("{stem}-{n}{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // 极端情况：一万次都撞名，回落到覆盖原路径，绝不 panic。
    path.to_path_buf()
}

/// `None` 的 existing = 文件不存在，所有档位都直接写。
pub(crate) fn decide(
    path: &Path,
    existing: Option<Existing>,
    source_mtime: Option<u32>,
    source_size: Option<u64>,
    policy: ConflictPolicy,
) -> Decision {
    let Some(existing) = existing else {
        return Decision::Write(path.to_path_buf());
    };
    match policy {
        ConflictPolicy::Overwrite => Decision::Write(path.to_path_buf()),
        ConflictPolicy::Skip => Decision::Skip,
        ConflictPolicy::Rename => Decision::Write(renamed(path)),
        ConflictPolicy::Newer => {
            let newer = match (source_mtime, existing.modified_secs) {
                (Some(src), Some(dst)) => u64::from(src) > dst,
                // 缺时间就不要猜：宁可覆盖也不把新文件悄悄丢掉。
                _ => true,
            };
            if newer { Decision::Write(path.to_path_buf()) } else { Decision::Skip }
        }
        ConflictPolicy::NewerAndLonger => {
            let newer_longer = match (source_mtime, existing.modified_secs, source_size) {
                (Some(src), Some(dst), Some(len)) => u64::from(src) > dst && len > existing.len,
                _ => true,
            };
            if newer_longer { Decision::Write(path.to_path_buf()) } else { Decision::Skip }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_file_is_always_written() {
        let path = Path::new("/tmp/a.bin");
        let existing = None;
        for policy in [
            ConflictPolicy::Overwrite,
            ConflictPolicy::Newer,
            ConflictPolicy::NewerAndLonger,
            ConflictPolicy::Rename,
            ConflictPolicy::Skip,
        ] {
            assert!(matches!(
                decide(path, existing, Some(100), Some(10), policy),
                Decision::Write(p) if p == path
            ));
        }
    }

    #[test]
    fn newer_and_newer_and_longer_compare_the_same_mtime_differently() {
        let path = Path::new("/tmp/a.bin");
        let old = Some(Existing { modified_secs: Some(100), len: 100 });
        assert!(matches!(
            decide(path, old, Some(101), Some(10), ConflictPolicy::Newer),
            Decision::Write(_)
        ));
        assert!(matches!(
            decide(path, old, Some(101), Some(10), ConflictPolicy::NewerAndLonger),
            Decision::Skip
        ));
        assert!(matches!(
            decide(path, old, Some(101), Some(101), ConflictPolicy::NewerAndLonger),
            Decision::Write(_)
        ));
        // 对端没给时间时不猜，保持覆盖。
        assert!(matches!(
            decide(path, old, None, None, ConflictPolicy::Newer),
            Decision::Write(_)
        ));
    }

    #[test]
    fn skip_and_rename_take_effect_only_when_the_file_exists() {
        let dir = std::env::temp_dir().join(format!("zterm-conflict-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("a.bin");
        std::fs::write(&path, b"old").unwrap();
        let existing = Some(Existing { modified_secs: Some(10), len: 3 });
        assert!(matches!(
            decide(&path, existing, None, None, ConflictPolicy::Skip),
            Decision::Skip
        ));
        match decide(&path, existing, None, None, ConflictPolicy::Rename) {
            Decision::Write(p) => assert_eq!(p.file_name().unwrap(), "a-1.bin"),
            Decision::Skip => panic!("rename 不该跳过"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
