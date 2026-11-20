//! 命令行历史存储（W5-I 输入行模型的数据层，也是灰显 `CommandHistory` 设置页的支撑）。

//! # 设计边界（ADR-0014 / I3 / B1）
//!
//! 本模块是 `term-core` 的纯语义单元：**零 UI、零 IO、零线程、零墙钟**。
//! 因此这里**不读钟**——所有时间都以调用方注入的 `now: u64`（Unix 纪元秒）传入，
//! TTL 过期、顺序、去重全部基于注入值。真正的 `SystemTime::now()` 由宿主
//! （`term-runtime` / `desk`）在写入前递进，与本层无关。
//!
//! 持久化的「读盘 / 写盘」也不在本层：本层只负责内存态与 `serde` 序列化
//! （`to_json` / `from_json`），文件 IO 交给 `term-runtime::persist`。
//!
//! # 行为契约（对照 `settings_pages.rs` 灰显页出厂态）
//! - `History size` = 容量上限（默认 10000）：超出丢弃最旧；
//! - `Days to keep history` = TTL（默认 30 天）：超龄丢弃；`0` 表示永久保留；
//! - `Save command history to disk` = 是否落盘（序列化交由宿主）。
//!
//! 去重：连续两次相同命令行只更新时间戳，不产生重复条目（与终端历史惯例一致）。

use serde::{Deserialize, Serialize};

/// 一条历史记录。`ts` 为调用方注入的纪元秒（非本层读钟）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    ts: u64,
    line: String,
}

/// 命令行历史存储。内存态 + 序列化，无副作用。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CommandHistory {
    /// 按时间升序：最旧在前、最新在后。私有，序列化为整表。
    entries: Vec<Entry>,
    /// 容量上限（`History size`），`0` 表示不限制。
    capacity: usize,
    /// 保留天数 TTL（`Days to keep history`），`0` 表示永久保留。
    ttl_days: u64,
}

impl CommandHistory {
    /// 新建空历史。`capacity == 0` 视为不限制；`ttl_days == 0` 视为永久保留。
    pub fn new(capacity: usize, ttl_days: u64) -> Self {
        Self {
            entries: Vec::new(),
            capacity,
            ttl_days,
        }
    }

    /// 以既有条目重建（宿主从磁盘反序列化后调用）。TTL / 容量按参数重新生效。
    pub fn from_entries(entries: Vec<(u64, String)>, capacity: usize, ttl_days: u64) -> Self {
        let entries = entries
            .into_iter()
            .map(|(ts, line)| Entry { ts, line })
            .collect();
        let mut h = Self {
            entries,
            capacity,
            ttl_days,
        };
        let now = h.entries.last().map(|e| e.ts).unwrap_or(0);
        h.trim_ttl(now);
        h.trim_to_capacity();
        h
    }

    /// 记录一行输入。`now` 为调用方注入的纪元秒。
    ///
    /// - 空行（仅空白 / 仅回车）忽略；
    /// - 与最新一条相同则只更新其时间戳（去重），不新增条目；
    /// - 否则追加到末尾，随后按 TTL 与容量裁剪。
    pub fn push(&mut self, line: &str, now: u64) {
        let line = line.trim_end_matches(['\n', '\r']).to_string();
        if line.trim().is_empty() {
            return;
        }
        if let Some(last) = self.entries.last_mut() {
            if last.line == line {
                last.ts = now;
                return;
            }
        }
        self.entries.push(Entry { ts: now, line });
        self.trim_ttl(now);
        self.trim_to_capacity();
    }

    /// 丢弃超龄条目（基于注入的 `now`）。`ttl_days == 0` 不裁剪。
    fn trim_ttl(&mut self, now: u64) {
        if self.ttl_days == 0 {
            return;
        }
        let cutoff = now.saturating_sub(self.ttl_days * 86_400);
        self.entries.retain(|e| e.ts >= cutoff);
    }

    /// 超过容量丢弃最旧。`capacity == 0` 不裁剪。
    fn trim_to_capacity(&mut self) {
        if self.capacity == 0 {
            return;
        }
        while self.entries.len() > self.capacity {
            self.entries.remove(0);
        }
    }

    /// 最近 `limit` 条（最新在前）。
    pub fn recent(&self, limit: usize) -> Vec<&str> {
        self.entries
            .iter()
            .rev()
            .take(limit)
            .map(|e| e.line.as_str())
            .collect()
    }

    /// 最近 `limit` 条，连时间戳一起给出（最新在前）。
    ///
    /// 自动补全的近因加权要用它：只给文本就没有「多久之前敲过」的信息。
    /// 返回借用，不复制；调用方要跨帧持有自行 `to_owned`。
    pub fn recent_with_ts(&self, limit: usize) -> Vec<(&str, u64)> {
        self.entries
            .iter()
            .rev()
            .take(limit)
            .map(|e| (e.line.as_str(), e.ts))
            .collect()
    }

    /// 子串匹配（最新在前）。大小写敏感，与终端历史检索惯例一致。
    pub fn search(&self, query: &str) -> Vec<&str> {
        if query.is_empty() {
            return self.recent(usize::MAX);
        }
        self.entries
            .iter()
            .rev()
            .filter(|e| e.line.contains(query))
            .map(|e| e.line.as_str())
            .collect()
    }

    /// 当前条目数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 清空（如「清除历史」动作）。
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// 按当前偏好重设容量 / TTL 并立即裁剪（宿主读盘后用：盘上的旧条目
    /// 可能超出当前配置，读回来必须先按现配置收敛一次）。
    pub fn reconfigure(&mut self, capacity: usize, ttl_days: u64, now: u64) {
        self.capacity = capacity;
        self.ttl_days = ttl_days;
        self.trim_ttl(now);
        self.trim_to_capacity();
    }

    /// 序列化为 JSON（宿主落盘用）。
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("CommandHistory 序列化不会失败")
    }

    /// 从 JSON 反序列化（宿主读盘用）。失败返回空历史，不 panic。
    pub fn from_json(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_recent_newest_first() {
        let mut h = CommandHistory::new(100, 0);
        h.push("ls", 1);
        h.push("pwd", 2);
        h.push("echo hi", 3);
        assert_eq!(h.recent(10), vec!["echo hi", "pwd", "ls"]);
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn empty_and_whitespace_lines_ignored() {
        let mut h = CommandHistory::new(100, 0);
        h.push("", 1);
        h.push("\n", 2);
        h.push("   \r\n", 3);
        h.push("   ", 4);
        assert!(h.is_empty(), "纯空白行不应进入历史");
    }

    #[test]
    fn trailing_newline_stripped() {
        let mut h = CommandHistory::new(100, 0);
        h.push("ls\n", 1);
        h.push("pwd\r\n", 2);
        assert_eq!(h.recent(10), vec!["pwd", "ls"]);
        assert_eq!(h.search("ls").len(), 1);
    }

    #[test]
    fn consecutive_duplicates_dedupe_and_bump_ts() {
        let mut h = CommandHistory::new(100, 0);
        h.push("ls", 1);
        h.push("ls", 5); // 连续相同：不新增，只更新时间戳
        assert_eq!(h.len(), 1, "连续重复不应产生重复条目");
        let json = h.to_json();
        let back = CommandHistory::from_json(&json).unwrap();
        assert_eq!(back.recent(1), vec!["ls"]);
        // 反序列化后时间戳应保留为 5（去重时的更新值）。
        let restored: CommandHistory =
            serde_json::from_str(&json).unwrap();
        assert_eq!(restored.entries.last().unwrap().ts, 5);
    }

    #[test]
    fn non_consecutive_same_line_kept_twice() {
        let mut h = CommandHistory::new(100, 0);
        h.push("ls", 1);
        h.push("pwd", 2);
        h.push("ls", 3); // 隔了一条，应保留两条 ls
        assert_eq!(h.len(), 3);
        assert_eq!(h.search("ls").len(), 2);
    }

    #[test]
    fn capacity_drops_oldest() {
        let mut h = CommandHistory::new(3, 0);
        for i in 1..=5 {
            h.push(&format!("cmd{i}"), i as u64);
        }
        assert_eq!(h.len(), 3, "超出容量应丢弃最旧");
        assert_eq!(h.recent(10), vec!["cmd5", "cmd4", "cmd3"]);
    }

    #[test]
    fn capacity_zero_means_unbounded() {
        let mut h = CommandHistory::new(0, 0);
        for i in 1..=50 {
            h.push(&format!("cmd{i}"), i as u64);
        }
        assert_eq!(h.len(), 50);
    }

    #[test]
    fn ttl_expires_old_entries() {
        // 容量不限，TTL = 2 天（172800 秒）。
        let mut h = CommandHistory::new(0, 2);
        h.push("old", 1_000_000);
        h.push("mid", 1_000_000 + 100_000);
        // now 比最旧超出 2 天，比 mid 也超出（mid+172800 = 1_272_800 < 1_300_000）。
        let now = 1_300_000;
        h.push("new", now);
        h.trim_ttl(now);
        assert_eq!(h.len(), 1, "超龄条目应被 TTL 裁剪");
        assert_eq!(h.recent(10), vec!["new"]);
    }

    #[test]
    fn ttl_zero_keeps_forever() {
        let mut h = CommandHistory::new(0, 0); // ttl_days == 0 → 永久
        h.push("ancient", 1);
        h.trim_ttl(1_000_000_000);
        assert_eq!(h.len(), 1, "ttl_days=0 不应裁剪任何条目");
    }

    #[test]
    fn ttl_exact_boundary_kept() {
        let mut h = CommandHistory::new(0, 1); // 1 天 = 86400 秒
        h.push("edge", 1_000_000);
        let now = 1_000_000 + 86_400; // 恰好 1 天，应保留
        h.trim_ttl(now);
        assert_eq!(h.len(), 1, "恰好等于 TTL 边界的条目应保留");
    }

    #[test]
    fn recent_with_ts_keeps_timestamps_newest_first() {
        let mut h = CommandHistory::new(10, 0);
        h.push("ls", 11);
        h.push("pwd", 22);
        assert_eq!(h.recent_with_ts(10), vec![("pwd", 22), ("ls", 11)]);
    }

    #[test]
    fn search_substring_case_sensitive() {
        let mut h = CommandHistory::new(0, 0);
        h.push("git status", 1);
        h.push("git push", 2);
        h.push("ls -la", 3);
        let hits = h.search("git");
        assert_eq!(hits, vec!["git push", "git status"]);
        assert!(h.search("GIT").is_empty(), "子串匹配大小写敏感");
        assert_eq!(h.search(""), h.recent(usize::MAX), "空查询返回全部");
    }

    #[test]
    fn clear_empties() {
        let mut h = CommandHistory::new(10, 0);
        h.push("a", 1);
        h.push("b", 2);
        h.clear();
        assert!(h.is_empty());
    }

    #[test]
    fn reconfigure_applies_new_limits() {
        let mut h = CommandHistory::new(0, 0);
        for i in 1..=10 {
            h.push(&format!("c{i}"), i as u64);
        }
        assert_eq!(h.len(), 10);
        h.reconfigure(3, 0, 10); // 容量收紧到 3，TTL 不限
        assert_eq!(h.len(), 3, "重设容量后应裁剪到 3");
        assert_eq!(h.recent(10), vec!["c10", "c9", "c8"]);
    }

    #[test]
    fn json_round_trip_preserves_order_and_count() {
        let mut h = CommandHistory::new(100, 30);
        h.push("ls", 10);
        h.push("pwd", 20);
        let json = h.to_json();
        let restored = CommandHistory::from_json(&json).expect("应可反序列化");
        assert_eq!(restored.len(), 2);
        assert_eq!(restored.recent(10), vec!["pwd", "ls"]);
        assert_eq!(restored.capacity, 100);
        assert_eq!(restored.ttl_days, 30);
    }

    #[test]
    fn from_entries_reapplies_ttl_and_capacity() {
        // 反序列化时若当前 TTL/容量比入库时更紧，应立刻生效。
        let raw = vec![
            (1_000_000, "old".to_string()),
            (2_000_000, "new".to_string()),
        ];
        let h = CommandHistory::from_entries(raw, 0, 1); // TTL=1 天
        // now 在 new 之后 100 秒（远小于 1 天 TTL）：new 应保留、old 应被裁剪。
        let now = 2_000_000 + 100;
        let mut h = h;
        h.trim_ttl(now);
        assert_eq!(h.len(), 1, "重建后应按当前 TTL 裁剪超龄条目");
        assert_eq!(h.recent(10), vec!["new"]);
    }
}
