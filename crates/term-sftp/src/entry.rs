//! 一条远端目录项，以及**纯函数**的路径与展示规则。
//!
//! 这里没有 IO：`Sftp` 拿到服务端的 `DirEntry` 之后，把需要的东西抽成这个
//! 结构，之后排序、拼路径、格式化大小都只跟它打交道。这样「`/` 下面拼出来
//! 是不是双斜杠」「目录排不排在前面」这类问题全是单测问题，不需要起连接。

/// 目录里的一个条目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// 文件名（不含路径）。
    pub name: String,
    pub is_dir: bool,
    /// 符号链接：SFTP 的 `read_dir` 给的是 `lstat` 结果，链接本身不是目录，
    /// 但双击它多半想进去。所以这个标志只用于**显示**，不改变 `is_dir` 的语义。
    pub is_symlink: bool,
    /// 字节数；目录时服务端给的通常是 4096 或 0，一律显示成空。
    pub size: u64,
    /// 修改时间（Unix 纪元起的秒）。`None` = **对端没报**，不是「1970 年」。
    ///
    /// 区分这两件事是 XFER-06 目录同步的前提：`term_xfer::sync::plan` 拿它判
    /// 「该上传还是该下载」，把「不知道」写成 `0` 会让「本地更新」恒成立，整份
    /// 计划退化成全部上传。所以这一层允许 `None`，由调用方决定「不知道」怎么办。
    pub mtime: Option<i64>,
}

impl Entry {
    /// 在 `dir` 下这个条目的完整路径。
    pub fn path_in(&self, dir: &str) -> String {
        join(dir, &self.name)
    }
}

/// 把服务端的 `ReadDir` 收成一份排序好的列表，顺手丢掉 `.` 与 `..`。
///
/// 丢掉它们不是为了省一行显示：`..` 的 `metadata` 在多数服务端上是当前目录，
/// 点进去等于「刷新」；而「上一级」我们有 `entry::parent`，走的是一个**确定的**
/// 规则，不依赖服务端报什么。`.` 同理。
///
/// 排序规则见 [`sort_entries`]。放在这里是因为递归遍历（`walk`）也要用同一份
/// 收集逻辑——两边共用，就不会出现「浏览时排一种、下载时排另一种」。
pub fn collect(rd: russh_sftp::client::fs::ReadDir) -> Vec<Entry> {
    let mut entries: Vec<Entry> = rd
        .filter(|e| e.file_name() != "." && e.file_name() != "..")
        .map(|e| {
            let ty = e.file_type();
            let meta = e.metadata();
            Entry {
                name: e.file_name(),
                is_dir: ty.is_dir(),
                is_symlink: ty.is_symlink(),
                size: meta.size.unwrap_or(0),
                mtime: mtime_secs(&meta),
            }
        })
        .collect();
    sort_entries(&mut entries);
    entries
}

/// 服务端报的 mtime → 秒（`u32` 本来就是这个口径）。
///
/// 抽成纯函数是为了能单测：`collect` 收的是真 `ReadDir`，造不出来；而这层要做的事
/// 只是「有就带上、没有就留 `None`」，一条断言就能钉死。
pub fn mtime_secs(meta: &russh_sftp::client::fs::Metadata) -> Option<i64> {
    meta.mtime.map(i64::from)
}

/// 拼路径：`("/", "etc") → "/etc"`、`("/etc", "hosts") → "/etc/hosts"`。
///
/// 不做 `..` / `.` 归一——那是服务端的事（我们用的是 `canonicalize` 与 `read_dir`，
/// 路径都是服务端认得的形状）。在客户端归一反而会造出「本地拼的路径」与
/// 「服务端认的路径」两套真相。
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir == "." {
        return name.to_string();
    }
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// 上一级路径；到根（或本来就是相对路径的第一层）返回 `None`。
pub fn parent(dir: &str) -> Option<String> {
    let trimmed = dir.trim_end_matches('/');
    if trimmed.is_empty() {
        // "/" 或 "" —— 再往上没有了。
        return None;
    }
    match trimmed.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(i) => Some(trimmed[..i].to_string()),
        // "foo" 这种没有分隔符的相对路径：不知道相对谁，别猜。
        None => None,
    }
}

/// 目录在前、同类按名字（忽略大小写，再按原串）排序。
///
/// 为什么忽略大小写：远端常见 `README` / `build` / `Cargo.toml` 混在一起，
/// 区分大小写排序在用户眼里就是乱的（大写在 ASCII 里全排前面）。
pub fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| {
                a.name
                    .to_lowercase()
                    .cmp(&b.name.to_lowercase())
            })
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// 人类可读的大小。目录不显示（调用方用 `is_dir` 判断），这里只管字节数。
pub fn format_size(bytes: u64) -> String {
    const K: u64 = 1024;
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{:.1} KB", bytes as f64 / K as f64),
        1_048_576..=1_073_741_823 => format!("{:.1} MB", bytes as f64 / (K * K) as f64),
        _ => format!("{:.1} GB", bytes as f64 / (K * K * K) as f64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> Entry {
        Entry {
            name: name.to_string(),
            is_dir: true,
            is_symlink: false,
            size: 0,
            mtime: None,
        }
    }

    fn file(name: &str, size: u64) -> Entry {
        Entry {
            name: name.to_string(),
            is_dir: false,
            is_symlink: false,
            size,
            mtime: None,
        }
    }

    #[test]
    fn joining_under_root_does_not_double_the_slash() {
        assert_eq!(join("/", "etc"), "/etc");
        assert_eq!(join("/", "etc").matches('/').count(), 1);
    }

    #[test]
    fn joining_under_a_nested_dir() {
        assert_eq!(join("/var/log", "syslog"), "/var/log/syslog");
        // 末尾带斜杠也只有一个分隔符。
        assert_eq!(join("/var/log/", "syslog"), "/var/log/syslog");
    }

    #[test]
    fn parent_walks_up_and_stops_at_root() {
        assert_eq!(parent("/var/log/syslog").as_deref(), Some("/var/log"));
        assert_eq!(parent("/var").as_deref(), Some("/"));
        assert_eq!(parent("/"), None);
        // 相对路径没有分隔符：不知道相对谁，宁可不给上一级。
        assert_eq!(parent("srv"), None);
    }

    #[test]
    fn directories_sort_before_files_and_names_ignore_case() {
        let mut v = vec![
            file("Zebra", 1),
            dir("beta"),
            file("alpha", 2),
            dir("Alpha"),
        ];
        sort_entries(&mut v);
        let names: Vec<&str> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "beta", "alpha", "Zebra"]);
    }

    #[test]
    fn sizes_switch_units_at_the_right_boundaries() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn the_server_mtime_is_carried_over_and_unknown_stays_none() {
        use russh_sftp::client::fs::Metadata;
        let reported = Metadata {
            mtime: Some(1_700_000_000),
            ..Default::default()
        };
        assert_eq!(mtime_secs(&reported), Some(1_700_000_000));
        // 「服务端没报」必须留 None：写成 0 会让同步方向判定整份反转。
        assert_eq!(mtime_secs(&Metadata::default()), None);
        // 1970 年那一刻是**知道**的 0，与「不知道」不是一回事。
        let epoch = Metadata {
            mtime: Some(0),
            ..Default::default()
        };
        assert_eq!(mtime_secs(&epoch), Some(0));
    }

    #[test]
    fn an_entry_knows_its_own_path() {
        let e = dir("etc");
        assert_eq!(e.path_in("/"), "/etc");
        assert_eq!(e.path_in("/var"), "/var/etc");
    }
}
