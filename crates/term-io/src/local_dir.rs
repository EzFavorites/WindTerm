//! 本地目录枚举：Filer 在「本地 shell」会话里的数据源。
//!
//! 为什么放在 `term-io` 而不是 `apps/desktop/src/ui`：B2 要求 `ui/` 不持有
//! OS 资源，读目录是 IO；投影层只拿一组已经变成值的数据来画。
//!
//! 日期列要显示 `YYYY/MM/DD`，而 `std::fs::Metadata::modified()` 给的是
//! `SystemTime`。Unix 秒 → 公历日期的换算是纯算术（civil_from_days），写在这里
//! 并单测；**它不读墙钟**，也不给项目引入 chrono。
//!
//! 视觉基线不能钉在真实家目录上（每个人的目录都不同），所以 `--demo` 夹具
//! 改用 `apps/desktop/src/demo.rs` 里那组固定条目，本模块在夹具下不被调用。

use std::path::{Path, PathBuf};

/// 一行文件。投影层只认这个，不认 `DirEntry`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    /// `YYYY/MM/DD`；读不到修改时间时为空字符串。
    pub date: String,
}

/// 当前用户的家目录。读不到时用当前目录兜底，绝不返回空路径。
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 列一个本地目录：**目录在前**，其余按小写名字排；读不到就返回空列表
/// （面板显示 `0 items`，不假装成功）。
///
/// `show_hidden` 来自 `Preferences ▸ Filer ▸ Show hidden items`
/// （`Prefs.filer_show_hidden`）：`.git` / `.env` 这类以 `.` 开头的条目只在
/// 开关打开时列出。`.` 与 `..` 本来就不在 `read_dir` 的结果里，不必特判。
///
/// 过滤放在这一层而不是绘制层：过滤是本模块的**数据**语义，`ui/` 只该拿到
/// 「要画的那几行」；放绘制层的话，条目计数、排序、滚动上限都会先按未过滤的
/// 列表算一遍，状态行上的 `N items` 就会与实际行数不符。
pub fn list(path: &Path, show_hidden: bool) -> Vec<Row> {
    let Ok(rd) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut rows: Vec<Row> = rd
        .flatten()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let link = entry
                .file_type()
                .ok()
                .map(|t| t.is_symlink())
                .unwrap_or(false);
            // 跟随链接读元数据：链接指向目录时 `is_dir` 也该为真。
            let meta = entry.metadata().ok();
            let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let date = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| date_from_unix(d.as_secs()))
                .unwrap_or_default();
            Row {
                name,
                is_dir,
                is_symlink: link,
                size,
                date,
            }
        })
        .filter(|row| show_hidden || !row.name.starts_with('.'))
        .collect();
    sort_rows(&mut rows);
    rows
}

/// 目录在前、其余按小写名字排。抽成公开函数是为了让「同一种排法」也能用在
/// 夹具注入的那一组固定条目上，并且能单测——`list()` 读真实磁盘，测不了排序。
pub fn sort_rows(rows: &mut [Row]) {
    rows.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// Unix 秒 → `YYYY/MM/DD`（UTC，按整天取整）。
pub fn date_from_unix(secs: u64) -> String {
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}/{m:02}/{d:02}")
}

/// 1970-01-01 起的天数 → (年, 月, 日)。Howard Hinnant 的整数历法，
/// 没有查表、没有循环，负数（1970 之前）也成立。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y + if m <= 2 { 1 } else { 0 }, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch_and_the_next_day() {
        assert_eq!(date_from_unix(0), "1970/01/01");
        assert_eq!(date_from_unix(86_400), "1970/01/02");
    }

    #[test]
    fn a_leap_day_survives_the_integer_calendar() {
        assert_eq!(date_from_unix(1_709_164_800), "2024/02/29");
        assert_eq!(date_from_unix(1_709_251_200), "2024/03/01");
    }

    #[test]
    fn a_recent_date_matches_the_host_clock_conversion() {
        assert_eq!(date_from_unix(1_794_096_000), "2026/11/08");
    }

    #[test]
    fn listing_a_missing_directory_is_empty_not_an_error() {
        assert!(list(Path::new("/this/path/definitely/does/not/exist"), false).is_empty());
    }

    /// 临时目录：`.hidden` / `visible.txt` / 子目录 `.git`，三种形状各一条。
    fn hidden_fixture(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("windterm-localdir-{tag}-{nanos}"));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".hidden"), b"x").unwrap();
        std::fs::write(dir.join("visible.txt"), b"y").unwrap();
        dir
    }

    fn names(dir: &Path, show_hidden: bool) -> Vec<String> {
        list(dir, show_hidden)
            .into_iter()
            .map(|r| r.name)
            .collect()
    }

    #[test]
    fn the_hidden_switch_decides_whether_dot_entries_are_listed() {
        let dir = hidden_fixture("switch");
        let shown = names(&dir, false);
        assert_eq!(shown, ["visible.txt"], "关掉时 `.hidden` / `.git` 都不该出现");
        let all = names(&dir, true);
        // 目录恒在前（`.git` 是目录），其余按小写名字排。
        assert_eq!(all, [".git", ".hidden", "visible.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hidden_directory_is_still_sorted_as_a_directory() {
        // 过滤不能顺手把排序搞坏：打开开关后 `.git` 必须排在文件前面。
        let dir = hidden_fixture("sort");
        let rows = list(&dir, true);
        assert!(rows[0].is_dir && rows[0].name == ".git");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn directories_sort_before_files_regardless_of_case() {
        let mut rows = vec![
            Row { name: "alpha".into(), is_dir: false, is_symlink: false, size: 1, date: String::new() },
            Row { name: "Zeta".into(), is_dir: true, is_symlink: false, size: 0, date: String::new() },
            Row { name: "Beta".into(), is_dir: false, is_symlink: false, size: 1, date: String::new() },
        ];
        sort_rows(&mut rows);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Zeta", "alpha", "Beta"]);
    }
}
