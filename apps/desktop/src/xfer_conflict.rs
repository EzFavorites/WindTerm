//! X/Y/ZMODEM 接收侧的同名冲突策略（纯函数）。
//!
//! 上游 WindTerm（官网 page/14）列了七档：overwrite / newer / newer and longer /
//! append / resume / rename / skip。这里只做判据：给一个目标路径与「对端声明的
//! 元信息」，返回该写哪个文件、以什么方式写，或跳过。
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
    /// 追加：保留既有内容，把收到的完整文件接到末尾。
    Append,
    /// 续传：从既有长度起算，请对端只发剩下的部分（ZMODEM 的 ZRPOS 协商）。
    Resume,
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

/// 落盘方式。判据除了「写哪个文件」，还要说清「怎么写」——append / resume
/// 与覆盖的差别全在这里；只返回路径的话，两档就会退化成假选项。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteMode {
    /// 从头写（截断旧内容）。overwrite / newer / newer-and-longer / rename 用这档。
    Overwrite,
    /// 追加：对端照常从 0 发，落盘从既有长度起算；最终 = 旧内容 + 完整新文件。
    Append,
    /// 续传：对端从 `offset` 发，落盘也从该偏移起；最终 = 完整新文件。
    ///
    /// `offset` 取既有文件长度。ZMODEM 协议层会把它钳到声明长度
    /// （见 `Receiver::resume_from` / `on_file_info`），这里不重复钳。
    Resume { offset: u64 },
}

pub(crate) enum Decision {
    Write(PathBuf, WriteMode),
    Skip,
    /// 判据本身没法安全落地（改名用尽候选）：报错，绝不静默覆盖。
    Error(String),
}

/// 接收弹层下拉的档位与文案。**唯一一份清单**：弹层按这个顺序画，判据按这个
/// 清单遍历，加了变体忘写名字会在单测里当场撞出来。
///
/// 顺序与文案照官网 page/14 的七档：overwrite / newer / newer-and-longer /
/// append / resume / rename / skip。前两档沿用原弹层的「Only if …」句式，
/// 新增两档直接用官网词 Append / Resume。
pub(crate) const POLICIES: [(ConflictPolicy, &str); 7] = [
    (ConflictPolicy::Overwrite, "Overwrite"),
    (ConflictPolicy::Newer, "Only if newer"),
    (ConflictPolicy::NewerAndLonger, "Only if newer and longer"),
    (ConflictPolicy::Append, "Append"),
    (ConflictPolicy::Resume, "Resume"),
    (ConflictPolicy::Rename, "Rename"),
    (ConflictPolicy::Skip, "Skip"),
];

/// 下拉里显示的文案；未知档位（反序列化越界）回落到默认档的文案。
pub(crate) fn label(policy: ConflictPolicy) -> &'static str {
    POLICIES
        .iter()
        .find(|(candidate, _)| *candidate == policy)
        .map(|(_, text)| *text)
        .unwrap_or(POLICIES[0].1)
}

/// 会话属性里存的下标 → 策略。`POLICIES` 是唯一真相源：越界（手改过 JSON、
/// 老版本写进过更大的档位）一律回落 Overwrite，绝不在下拉里显示空白档。
pub(crate) fn policy_from_index(index: usize) -> ConflictPolicy {
    POLICIES.get(index).map(|(policy, _)| *policy).unwrap_or_default()
}

/// 策略 → 会话属性里存的下标。未知档位同样回落到 0，保证写回的永远是 0..6。
pub(crate) fn policy_index(policy: ConflictPolicy) -> usize {
    POLICIES.iter().position(|(candidate, _)| *candidate == policy).unwrap_or(0)
}

/// 改名用的候选个数上限。`decide` 用 9999；测试用小值，不必真造一万个文件。
const RENAME_ATTEMPTS: u32 = 9999;

fn renamed(path: &Path) -> Option<PathBuf> {
    renamed_up_to(path, RENAME_ATTEMPTS)
}

fn renamed_up_to(path: &Path, attempts: u32) -> Option<PathBuf> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    for n in 1..=attempts {
        let candidate = parent.join(format!("{stem}-{n}{ext}"));
        if !candidate.exists() {
            return Some(candidate);
        }
    }
    // 候选全被占：覆盖原路径就是数据丢失，宁可明确报错。
    None
}

fn rename_error(path: &Path, attempts: u32) -> String {
    format!(
        "改名失败：{} 的 -1..-{attempts} 全部存在，已拒绝覆盖原文件",
        path.display()
    )
}

/// `None` 的 existing = 文件不存在：七档都从头写，append / resume 没有可续的旧内容。
pub(crate) fn decide(
    path: &Path,
    existing: Option<Existing>,
    source_mtime: Option<u32>,
    source_size: Option<u64>,
    policy: ConflictPolicy,
) -> Decision {
    let Some(existing) = existing else {
        return Decision::Write(path.to_path_buf(), WriteMode::Overwrite);
    };
    match policy {
        ConflictPolicy::Overwrite => Decision::Write(path.to_path_buf(), WriteMode::Overwrite),
        ConflictPolicy::Skip => Decision::Skip,
        ConflictPolicy::Rename => match renamed(path) {
            Some(target) => Decision::Write(target, WriteMode::Overwrite),
            None => Decision::Error(rename_error(path, RENAME_ATTEMPTS)),
        },
        ConflictPolicy::Newer => {
            let newer = match (source_mtime, existing.modified_secs) {
                (Some(src), Some(dst)) => u64::from(src) > dst,
                // 缺时间就不要猜：宁可覆盖也不把新文件悄悄丢掉。
                _ => true,
            };
            if newer {
                Decision::Write(path.to_path_buf(), WriteMode::Overwrite)
            } else {
                Decision::Skip
            }
        }
        ConflictPolicy::NewerAndLonger => {
            let newer_longer = match (source_mtime, existing.modified_secs, source_size) {
                (Some(src), Some(dst), Some(len)) => u64::from(src) > dst && len > existing.len,
                _ => true,
            };
            if newer_longer {
                Decision::Write(path.to_path_buf(), WriteMode::Overwrite)
            } else {
                Decision::Skip
            }
        }
        ConflictPolicy::Append => Decision::Write(path.to_path_buf(), WriteMode::Append),
        ConflictPolicy::Resume => Decision::Write(
            path.to_path_buf(),
            WriteMode::Resume { offset: existing.len },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用：只关心「写哪个文件 + 怎么写」，Skip 记成 `None`。
    fn outcome(
        path: &Path,
        existing: Option<Existing>,
        mtime: Option<u32>,
        size: Option<u64>,
        policy: ConflictPolicy,
    ) -> Option<(PathBuf, WriteMode)> {
        match decide(path, existing, mtime, size, policy) {
            Decision::Write(p, mode) => Some((p, mode)),
            Decision::Skip | Decision::Error(_) => None,
        }
    }

    /// 每档一个独立临时目录（`RUST_TEST_THREADS=1` 下也避免撞名）。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zterm-conflict-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn seven_policies_match_the_official_page14_order_and_labels() {
        let expected = [
            (ConflictPolicy::Overwrite, "Overwrite"),
            (ConflictPolicy::Newer, "Only if newer"),
            (ConflictPolicy::NewerAndLonger, "Only if newer and longer"),
            (ConflictPolicy::Append, "Append"),
            (ConflictPolicy::Resume, "Resume"),
            (ConflictPolicy::Rename, "Rename"),
            (ConflictPolicy::Skip, "Skip"),
        ];
        assert_eq!(POLICIES, expected);
        assert_eq!(POLICIES.len(), 7);
        // 七档各自的文案都要能从 label() 取回，且与官网词一致。
        for &(policy, text) in &POLICIES {
            assert_eq!(label(policy), text);
        }
    }

    #[test]
    fn policy_indexes_round_trip_through_the_single_policy_list() {
        for (i, &(policy, _)) in POLICIES.iter().enumerate() {
            assert_eq!(policy_from_index(i), policy, "下标 {i} 应取回列表里的那一档");
            assert_eq!(policy_index(policy), i, "这一档应写回下标 {i}");
        }
    }

    #[test]
    fn out_of_range_policy_indexes_fall_back_to_overwrite() {
        assert_eq!(policy_from_index(POLICIES.len()), ConflictPolicy::Overwrite);
        assert_eq!(policy_from_index(usize::MAX), ConflictPolicy::Overwrite);
        // 列表本身不变：回落不会凭空多出第八档。
        assert_eq!(POLICIES.len(), 7);
        assert_eq!(policy_index(policy_from_index(usize::MAX)), 0);
    }

    #[test]
    fn seven_policy_decision_table_with_an_existing_target() {
        let dir = temp_dir("table");
        let path = dir.join("a.bin");
        std::fs::write(&path, b"old").unwrap();
        let existing = Some(Existing { modified_secs: Some(100), len: 3 });

        // 源更新且更长：覆盖系三档写原路径；append/resume 走各自方式；rename 改名；skip 跳过。
        let table = [
            (ConflictPolicy::Overwrite, Some((path.clone(), WriteMode::Overwrite))),
            (ConflictPolicy::Newer, Some((path.clone(), WriteMode::Overwrite))),
            (ConflictPolicy::NewerAndLonger, Some((path.clone(), WriteMode::Overwrite))),
            (ConflictPolicy::Append, Some((path.clone(), WriteMode::Append))),
            (ConflictPolicy::Resume, Some((path.clone(), WriteMode::Resume { offset: 3 }))),
            (ConflictPolicy::Rename, Some((dir.join("a-1.bin"), WriteMode::Overwrite))),
            (ConflictPolicy::Skip, None),
        ];
        for (policy, want) in table {
            assert_eq!(
                outcome(&path, existing, Some(101), Some(9), policy),
                want,
                "{policy:?} 的判据与七档决策表不一致"
            );
        }

        // 源更旧：覆盖系两档跳过；append/resume 不受时间影响，仍旧按既有长度落盘。
        assert_eq!(outcome(&path, existing, Some(99), Some(2), ConflictPolicy::Newer), None);
        assert_eq!(outcome(&path, existing, Some(99), Some(2), ConflictPolicy::NewerAndLonger), None);
        assert_eq!(
            outcome(&path, existing, Some(99), Some(2), ConflictPolicy::Append),
            Some((path.clone(), WriteMode::Append))
        );
        assert_eq!(
            outcome(&path, existing, Some(99), Some(2), ConflictPolicy::Resume),
            Some((path.clone(), WriteMode::Resume { offset: 3 }))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_and_resume_boundaries() {
        let path = Path::new("/tmp/a.bin");

        // 目标不存在：七档都从头写，没有可续 / 可接的旧内容。
        for &(policy, _) in &POLICIES {
            assert_eq!(
                outcome(path, None, Some(100), Some(10), policy),
                Some((path.to_path_buf(), WriteMode::Overwrite)),
                "{policy:?} 在目标不存在时应从头写"
            );
        }

        // 空文件：Append 落盘从 0 起，Resume 的偏移也是 0（等价于从头写）。
        let empty = Some(Existing { modified_secs: Some(10), len: 0 });
        assert_eq!(
            outcome(path, empty, Some(11), Some(10), ConflictPolicy::Append),
            Some((path.to_path_buf(), WriteMode::Append))
        );
        assert_eq!(
            outcome(path, empty, Some(11), Some(10), ConflictPolicy::Resume),
            Some((path.to_path_buf(), WriteMode::Resume { offset: 0 }))
        );

        // 目标比源小 / 比源大：Append 与源长度无关；Resume 的偏移恒取既有长度，
        // 「源更小」时协议层会把它钳到声明长度（这里只是判据，不替协议层做决定）。
        for &(len, size) in &[(5u64, 100u64), (500u64, 100u64)] {
            let existing = Some(Existing { modified_secs: Some(10), len });
            assert_eq!(
                outcome(path, existing, Some(11), Some(size), ConflictPolicy::Append),
                Some((path.to_path_buf(), WriteMode::Append))
            );
            assert_eq!(
                outcome(path, existing, Some(11), Some(size), ConflictPolicy::Resume),
                Some((path.to_path_buf(), WriteMode::Resume { offset: len }))
            );
        }

        // 对端没给长度：Append 仍追加；Resume 用既有长度（协议层按无长度处理）。
        let existing = Some(Existing { modified_secs: Some(10), len: 42 });
        assert_eq!(
            outcome(path, existing, None, None, ConflictPolicy::Append),
            Some((path.to_path_buf(), WriteMode::Append))
        );
        assert_eq!(
            outcome(path, existing, None, None, ConflictPolicy::Resume),
            Some((path.to_path_buf(), WriteMode::Resume { offset: 42 }))
        );
    }

    #[test]
    fn an_absent_file_is_always_written() {
        let path = Path::new("/tmp/a.bin");
        // 七档逐一遍历（不再手写清单），目标不存在时一律从头写。
        for &(policy, _) in &POLICIES {
            assert_eq!(
                outcome(path, None, Some(100), Some(10), policy),
                Some((path.to_path_buf(), WriteMode::Overwrite))
            );
        }
    }

    #[test]
    fn newer_and_newer_and_longer_compare_the_same_mtime_differently() {
        let path = Path::new("/tmp/a.bin");
        let old = Some(Existing { modified_secs: Some(100), len: 100 });
        assert!(matches!(
            decide(path, old, Some(101), Some(10), ConflictPolicy::Newer),
            Decision::Write(_, WriteMode::Overwrite)
        ));
        assert!(matches!(
            decide(path, old, Some(101), Some(10), ConflictPolicy::NewerAndLonger),
            Decision::Skip
        ));
        assert!(matches!(
            decide(path, old, Some(101), Some(101), ConflictPolicy::NewerAndLonger),
            Decision::Write(_, WriteMode::Overwrite)
        ));
        // 对端没给时间时不猜，保持覆盖。
        assert!(matches!(
            decide(path, old, None, None, ConflictPolicy::Newer),
            Decision::Write(_, WriteMode::Overwrite)
        ));
    }

    #[test]
    fn rename_never_falls_back_to_overwriting_the_original() {
        let dir = temp_dir("rename-exhausted");
        let path = dir.join("a.bin");
        std::fs::write(&path, b"old").unwrap();
        // 两个候选都被占：不再回落覆盖原路径，而是给出明确错误。
        std::fs::write(dir.join("a-1.bin"), b"one").unwrap();
        std::fs::write(dir.join("a-2.bin"), b"two").unwrap();
        assert!(renamed_up_to(&path, 2).is_none(), "候选全被占时不再回落覆盖");
        let err = rename_error(&path, 2);
        assert!(err.contains("改名失败"), "{err}");
        assert!(err.contains("拒绝覆盖"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), b"old", "原文件必须原样保留");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_policy_has_a_distinct_dropdown_label() {
        let mut seen = std::collections::BTreeSet::new();
        for &(policy, text) in &POLICIES {
            assert!(!text.is_empty(), "{policy:?} 缺下拉文案");
            assert!(seen.insert(text), "{text} 与另一档撞名");
            assert_eq!(label(policy), text);
        }
        assert_eq!(seen.len(), POLICIES.len());
    }

    #[test]
    fn skip_and_rename_take_effect_only_when_the_file_exists() {
        let dir = temp_dir("skip-rename");
        let path = dir.join("a.bin");
        std::fs::write(&path, b"old").unwrap();
        let existing = Some(Existing { modified_secs: Some(10), len: 3 });
        assert_eq!(outcome(&path, existing, None, None, ConflictPolicy::Skip), None);
        match outcome(&path, existing, None, None, ConflictPolicy::Rename) {
            Some((p, mode)) => {
                assert_eq!(p.file_name().unwrap(), "a-1.bin");
                assert_eq!(mode, WriteMode::Overwrite);
            }
            None => panic!("rename 不该跳过"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
