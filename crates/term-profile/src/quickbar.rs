//! 快捷命令栏配置（`quickbar.config`）——命令表是**用户数据**，不是编译期常量。
//!
//! # 文件格式与参考实现逐字节兼容
//!
//! 落盘文件是 WindTerm 参考实现同名的 `quickbar.config`：**一个扁平的 JSON 数组**，
//! 每条命令 5 个参考实现键 `quick.group` / `quick.icon` / `quick.label` / `quick.type`
//! / `quick.uuid`。参考实现读这份文件即可直接打开本应用写出的配置。本应用额外写入
//! `send` / `label_zh` / `group_label_zh` 三个键，参考实现（serde 忽略未知键）自动
//! 跳过，不影响它读取命令表——这就是用户要的「同一份配置文件」。
//!
//! 组在扁平格式里是**隐式**的：靠 `quick.group` 同名聚合；组色编码进 `quick.icon`
//! 的 `session::<图形>-<颜色>`（颜色用 `#RRGGBB`，Qt/WindTerm 也认 hex）。
//! 内存里仍用嵌套的 `QuickBarConfig`（组 → 命令）方便编辑与渲染；`load` / `save`
//! 负责扁平 ↔ 嵌套的双向转换。
//!
//! # 三个显示开关不在这份文件里
//!
//! `Show multiple groups` / `Show group title` / `Show commands in alphabetical
//! order` 在参考实现存于 `user.config`（≈本应用的 `prefs.json`），**不**在
//! `quickbar.config`。本应用把它们放进 `Prefs`（见 `prefs.rs` 的 `quickbar_*`
//! 字段），渲染期由 [`QuickBarDisplay`] 携带。
//!
//! # 读一次 + 渲染期副本
//!
//! 与 `prefs.rs` 同一条规矩：构造时读盘一次，渲染期只读内存副本，不每帧读盘；
//! 拖动时的值和盘上的值不该各说一套。文件缺失 / 读不出来 / JSON 坏了都回退出厂
//! 默认，绝不拦住启动；覆盖坏文件之前先逐字节另存一份，不让用户手改的内容凭空消失。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 落盘文件名：与 WindTerm 参考实现同名，放在 profile 的 `terminal/` 子目录
/// （参考实现在 `profiles/<name>/terminal/`，本应用的 `profile_dir` 已是其根）。
pub const QUICKBAR_FILE: &str = "quickbar.config";

fn default_type() -> String {
    "Send Text".to_string()
}

fn default_color() -> String {
    "#1E90FF".into()
}

/// 渲染期携带的三个显示开关。来源是 `Prefs`（参考实现 `user.config` 的位置），不随
/// 命令表落盘，所以与 [`QuickBarConfig`] 分开。`Default` 即出厂态：显示多组与组
/// 标题、不按字母序（守住 main-dark 底栏 File 组的既有顺序）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuickBarDisplay {
    pub show_multiple_groups: bool,
    pub show_group_title: bool,
    pub alphabetical: bool,
}

impl Default for QuickBarDisplay {
    /// 出厂态 = 视觉基线底栏：四组全显示、每组带标题、不按字母序。
    /// 组件基线用 `QuickBarDisplay::default()` 即这份出厂态，守住现有像素基线。
    fn default() -> Self {
        Self {
            show_multiple_groups: true,
            show_group_title: true,
            alphabetical: false,
        }
    }
}

/// 一条快捷命令。`label` 是按钮逐字文字，`send` 是送进输入行的文本——两者不总
/// 相同（`Copy` → `cp `）。`label_zh` 为空时中文界面回退 `label`。`uuid` 是命令
/// 的稳定标识（参考实现必填，落盘进 `quick.uuid`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickCommandConfig {
    pub label: String,
    #[serde(default)]
    pub label_zh: String,
    pub send: String,
    /// 左侧彩色小图形：square / circle / cross / pin / arrow / star / heart。
    /// 用字符串而不是枚举，是为了让用户手改配置时写错一个词只回落默认图形，
    /// 不会让整个文件解析失败。
    #[serde(default)]
    pub mark: String,
    /// 命令的稳定 uuid（参考实现 `quick.uuid`）。出厂项用确定性哈希生成（可复现、
    /// 不依赖时钟/随机数，便于组件基线稳定）；用户新建项用 [`new_uuid`] 取唯一值。
    pub uuid: String,
}

impl QuickCommandConfig {
    fn new(group: &str, label: &str, label_zh: &str, send: &str, mark: &str) -> Self {
        Self {
            label: label.to_owned(),
            label_zh: label_zh.to_owned(),
            send: send.to_owned(),
            mark: mark.to_owned(),
            uuid: stable_uuid(group, label),
        }
    }
}

/// 一个命令组。`color` 是 `#RRGGBB`；解析失败由渲染层回落到出厂调色板，
/// 不在这里判合法性（本层不认识颜色类型）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickGroupConfig {
    pub label: String,
    #[serde(default)]
    pub label_zh: String,
    pub color: String,
    /// 组可见性**不在这里**：它由栏左侧配置菜单切，属于会话内的 UI 状态
    /// （`ViewState.quick_groups_visible`）。放进来就成了两个写入者写一个
    /// 变量，重启后各说一套。
    #[serde(default)]
    pub commands: Vec<QuickCommandConfig>,
}

/// 整份配置 = 嵌套的组列表。落盘时由 [`save`] 拍平成 WindTerm 的扁平数组。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickBarConfig {
    /// 缺字段 → 出厂四组；显式写 `[]` 是用户故意清空，两者不能混。
    #[serde(default = "default_groups")]
    pub groups: Vec<QuickGroupConfig>,
}

impl Default for QuickBarConfig {
    fn default() -> Self {
        Self {
            groups: default_groups(),
        }
    }
}

/// 出厂词表：四组十五项，逐字取自界面视觉基线底部的快捷命令栏。
/// 中文只译五个当按钮标签用的英文动词与四个组名，命令行工具名一律不译。
pub fn default_groups() -> Vec<QuickGroupConfig> {
    vec![
        QuickGroupConfig {
            label: "Quick".into(),
            label_zh: "快捷".into(),
            color: "#1E90FF".into(),
            commands: vec![
                QuickCommandConfig::new("File", "Copy", "复制", "cp ", "square"),
                QuickCommandConfig::new("File", "Move", "移动", "mv ", "circle"),
                QuickCommandConfig::new("File", "Remove", "删除", "rm ", "cross"),
                QuickCommandConfig::new("File", "Rename", "重命名", "rename ", "pin"),
                QuickCommandConfig::new("File", "Property", "属性", "stat ", "arrow"),
            ],
        },
        QuickGroupConfig {
            label: "Network".into(),
            label_zh: "网络".into(),
            color: "#FF1493".into(),
            commands: vec![
                QuickCommandConfig::new("Network", "ping", "", "ping ", "square"),
                QuickCommandConfig::new("Network", "traceroute", "", "traceroute ", "circle"),
                QuickCommandConfig::new("Network", "mtr", "", "mtr ", "cross"),
                QuickCommandConfig::new("Network", "ifconfig", "", "ifconfig ", "star"),
                QuickCommandConfig::new("Network", "tcpdump", "", "tcpdump ", "pin"),
            ],
        },
        QuickGroupConfig {
            label: "Shell".into(),
            label_zh: "终端".into(),
            color: "#FFD700".into(),
            commands: vec![
                QuickCommandConfig::new("Shell", "ls", "", "ls ", "circle"),
                QuickCommandConfig::new("Shell", "cat", "", "cat ", "arrow"),
                QuickCommandConfig::new("Shell", "vi", "", "vi ", "star"),
            ],
        },
        QuickGroupConfig {
            label: "System".into(),
            label_zh: "系统".into(),
            color: "#228B22".into(),
            commands: vec![
                QuickCommandConfig::new("System", "reboot", "", "reboot ", "square"),
                QuickCommandConfig::new("System", "crontab", "", "crontab ", "heart"),
            ],
        },
    ]
}

/// 配置文件路径（`profile_dir()/terminal/quickbar.config`）。
pub fn path() -> Option<PathBuf> {
    let base = term_paths::profile_dir()?;
    Some(base.join("terminal").join(QUICKBAR_FILE))
}

/// 读配置。任何失败都回退出厂默认——调用方不需要区分「没写过」和「写坏了」，
/// 两者对界面是同一件事。
pub fn load() -> QuickBarConfig {
    let Some(p) = path() else {
        return QuickBarConfig::default();
    };
    load_from(&p)
}

fn load_from(path: &Path) -> QuickBarConfig {
    let Ok(text) = std::fs::read_to_string(path) else {
        return QuickBarConfig::default();
    };
    let entries: Vec<FlatEntry> = match serde_json::from_str(&text) {
        Ok(e) => e,
        Err(_) => return QuickBarConfig::default(),
    };
    flat_to_config(&entries)
}

/// 把扁平条目聚合成嵌套配置。空标签的条目（手改坏的一半）直接跳过，不拦启动。
/// 合法但为空（用户显式清空，落盘成 `[]`）就返回空组表——不回落出厂，那是两种
/// 不同的意图（「没写过」vs「故意清空」）。
fn flat_to_config(entries: &[FlatEntry]) -> QuickBarConfig {
    let mut groups: Vec<QuickGroupConfig> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for e in entries {
        // 没有标签的命令既画不出来也发不出去——跳过它，别让同文件其余命令陪葬。
        if e.label.is_empty() {
            continue;
        }
        let (mark, color) = parse_icon(&e.icon);
        // 参考实现没有独立的「发送文本」字段，label 就是发送内容；我们读到 `send`
        // 缺省时回落 label，这样参考实现文件导入后按钮仍可用。自己写出去的 `send`
        // 非空（见 save），不会被这条回落改掉。
        let send = if e.send.is_empty() {
            e.label.clone()
        } else {
            e.send.clone()
        };
        let cmd = QuickCommandConfig {
            label: e.label.clone(),
            label_zh: e.label_zh.clone(),
            send,
            mark,
            uuid: e.uuid.clone(),
        };
        match index.get(&e.group) {
            Some(&i) => groups[i].commands.push(cmd),
            None => {
                index.insert(e.group.clone(), groups.len());
                groups.push(QuickGroupConfig {
                    label: e.group.clone(),
                    label_zh: e.group_label_zh.clone(),
                    color,
                    commands: vec![cmd],
                });
            }
        }
    }
    QuickBarConfig { groups }
}

/// 写配置。失败不崩；与 `prefs::save` 同一条「覆盖前先保全」的规矩。
pub fn save(config: &QuickBarConfig) -> bool {
    let Some(p) = path() else {
        return false;
    };
    save_to(&p, config)
}

fn save_to(path: &Path, config: &QuickBarConfig) -> bool {
    if !preserve_broken(path) {
        return false;
    }
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    // 拍平：组 → 命令，组色进 icon，组中文名进每条命令的 `group_label_zh`
    // （扁平格式没有组级字段，只能冗余到成员上；参考实现忽略它）。
    let entries: Vec<FlatEntry> = config
        .groups
        .iter()
        .flat_map(|g| {
            g.commands.iter().map(move |c| FlatEntry {
                group: g.label.clone(),
                icon: build_icon(&c.mark, &g.color),
                label: c.label.clone(),
                qtype: default_type(),
                uuid: c.uuid.clone(),
                send: c.send.clone(),
                label_zh: c.label_zh.clone(),
                group_label_zh: g.label_zh.clone(),
            })
        })
        .collect();
    let Ok(json) = serde_json::to_string_pretty(&entries) else {
        return false;
    };
    // 先写**唯一名**临时文件再原子改名：中途断电不会留下半个 JSON，
    // 两个进程共享 profile 目录时也不会互相 rename 掉对方的临时文件。
    crate::persist::write_atomic(path, json.as_bytes())
}

/// 解析 `session::<图形>-<颜色>`（颜色可用 `#RRGGBB` 或 CSS 颜色名）。
fn parse_icon(icon: &str) -> (String, String) {
    let inner = icon.strip_prefix("session::").unwrap_or(icon);
    match inner.split_once('-') {
        Some((mark, color)) => (mark.to_owned(), named_to_hex(color)),
        None => (inner.to_owned(), default_color()),
    }
}

/// 组装 `session::<图形>-<颜色>`。
fn build_icon(mark: &str, color: &str) -> String {
    format!("session::{mark}-{color}")
}

/// CSS 颜色名 → `#RRGGBB`。只收一小撮常见名（含 WindTerm 样例里出现的
/// deeppink / rosybrown / coral），认不出的回落出厂蓝——颜色是装饰，读参考实现
/// 文件时丢了色也不该让命令表读不出来。
fn named_to_hex(name: &str) -> String {
    let trimmed = name.trim();
    let lower = trimmed.to_ascii_lowercase();
    let hex = match lower.as_str() {
        "black" => 0x000000,
        "white" => 0xFFFFFF,
        "red" => 0xFF0000,
        "lime" | "green" => 0x00FF00,
        "blue" => 0x0000FF,
        "yellow" => 0xFFFF00,
        "aqua" | "cyan" => 0x00FFFF,
        "fuchsia" | "magenta" => 0xFF00FF,
        "silver" => 0xC0C0C0,
        "gray" | "grey" => 0x808080,
        "maroon" => 0x800000,
        "olive" => 0x808000,
        "navy" => 0x000080,
        "teal" => 0x008080,
        "purple" => 0x800080,
        "orange" => 0xFFA500,
        "deeppink" => 0xFF1493,
        "rosybrown" => 0xBC8F8F,
        "coral" => 0xFF7F50,
        "dodgerblue" => 0x1E90FF,
        _ => {
            // 已经是 hex（带 #）：原样返回，保留大小写，只校验。
            // 大小写不一致会让「嵌套→扁平→嵌套」的往返失败。
            if let Some(rest) = lower.strip_prefix('#') {
                if u32::from_str_radix(rest, 16).is_ok() {
                    return trimmed.to_owned();
                }
                return default_color();
            }
            // 无 # 前缀的纯 hex 数字：补上 # 并大写归一。
            if let Ok(v) = u32::from_str_radix(lower.as_str(), 16) {
                return format!("#{v:06X}");
            }
            return default_color();
        }
    };
    format!("#{hex:06X}")
}

/// 覆盖前检查：文件不在 = 无损；能解析成扁平数组 = 无损；在但解析不了 = 逐字节
/// 另存 `quickbar.config.bad-<Unix 秒>` 再允许覆盖。备份做不到就**这一次不写**，
/// 宁可让用户改动不生效，也不要把他的手改内容换成出厂值。
fn preserve_broken(path: &Path) -> bool {
    if !path.is_file() {
        return true;
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return false,
    };
    if serde_json::from_slice::<Vec<FlatEntry>>(&bytes).is_ok() {
        return true;
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = path.with_extension(format!("config.bad-{ts}"));
    std::fs::write(&backup, &bytes).is_ok()
}

/// 落盘格式：WindTerm 参考实现 `quickbar.config` 的一条。serde 字段名即参考实现键；
/// 额外键 `send` / `label_zh` / `group_label_zh` 参考实现读时忽略（serde 默认填充）。
/// 空字符串不写，保持文件干净也避免读取时误触回落。
#[derive(Serialize, Deserialize)]
struct FlatEntry {
    #[serde(rename = "quick.group", default)]
    group: String,
    #[serde(rename = "quick.icon", default)]
    icon: String,
    #[serde(rename = "quick.label", default)]
    label: String,
    #[serde(rename = "quick.type", default = "default_type")]
    qtype: String,
    #[serde(rename = "quick.uuid", default)]
    uuid: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    send: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    label_zh: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    group_label_zh: String,
}

static UUID_CLOCK: AtomicU64 = AtomicU64::new(0);

/// 用户新建命令/组时取唯一 uuid（离线、无外部 crate：时间戳 + 序号 + PID 混出
/// 一个 v4 外观的串，足够当稳定键，不会与参考实现或其它新建项撞）。
pub fn new_uuid() -> String {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = UUID_CLOCK.fetch_add(1, Ordering::Relaxed) as u128;
    let mix = n ^ (seq << 64) ^ ((std::process::id() as u128) << 96);
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:08x}{:04x}",
        (mix >> 96) as u32,
        (mix >> 80) as u16,
        (mix >> 64) as u16,
        ((mix >> 48) as u16) | 0x8000,
        (mix >> 32) as u32,
        mix as u32
    )
}

/// 出厂项的确定性 uuid：用 (组, 标签) 的 FNV-1a 双盐哈希拼出，可复现、
/// 不依赖时钟，让组件基线稳定。不同 (组, 标签) 几乎不撞。
fn stable_uuid(group: &str, label: &str) -> String {
    let h = fnv1a(&format!("{group}\0{label}"));
    let h2 = fnv1a(&format!("{label}\0{group}"));
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:08x}{:04x}",
        (h >> 32) as u32,
        (h >> 16) as u16,
        (h & 0xffff) as u16,
        ((h2 >> 48) as u16) | 0x8000,
        (h2 >> 32) as u32,
        h2 as u32
    )
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "zterm-quickbar-{name}-{}-{}.config",
            std::process::id(),
            UUID_CLOCK.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn factory_default_is_the_four_groups_fifteen_commands() {
        let c = QuickBarConfig::default();
        assert_eq!(c.groups.len(), 4);
        assert_eq!(c.groups.iter().map(|g| g.commands.len()).sum::<usize>(), 15);
        assert_eq!(c.groups[0].label, "Quick");
        assert_eq!(c.groups[0].label_zh, "快捷");
        assert_eq!(c.groups[0].color, "#1E90FF");
        assert_eq!(c.groups[0].commands[0].label, "Copy");
        assert_eq!(c.groups[0].commands[0].send, "cp ");
        assert_eq!(c.groups[0].commands[0].mark, "square");
        assert!(!c.groups[0].commands[0].uuid.is_empty(), "出厂项也要有 uuid");
    }

    #[test]
    fn every_command_sends_plain_text_without_a_newline() {
        for g in default_groups() {
            for c in g.commands {
                assert!(c.send.ends_with(' '), "{} 尾部要留空格", c.label);
                assert!(!c.send.contains('\r') && !c.send.contains('\n'));
            }
        }
    }

    #[test]
    fn factory_uuid_is_deterministic_and_unique() {
        let a = QuickBarConfig::default();
        let b = QuickBarConfig::default();
        // 两次构造出的出厂 uuid 必须一致（可复现基线）。
        assert_eq!(a.groups[0].commands[0].uuid, b.groups[0].commands[0].uuid);
        // 不同命令的 uuid 不得撞。
        let mut seen = std::collections::HashSet::new();
        for g in &a.groups {
            for c in &g.commands {
                assert!(seen.insert(c.uuid.clone()), "uuid 撞了: {}", c.uuid);
            }
        }
    }

    #[test]
    fn the_flat_file_matches_windterm_shape() {
        // 写出去的文件必须能被参考实现逐字节读懂：5 个参考实现键 + 扁平数组。
        let p = temp("shape");
        assert!(save_to(&p, &QuickBarConfig::default()));
        let text = std::fs::read_to_string(&p).unwrap();
        let entries: Vec<FlatEntry> = serde_json::from_str(&text).unwrap();
        assert_eq!(entries.len(), 15);
        let e = &entries[0];
        assert_eq!(e.group, "Quick");
        assert!(e.icon.starts_with("session::square-#1E90FF"), "组色进 icon：{}", e.icon);
        assert_eq!(e.label, "Copy");
        assert_eq!(e.qtype, "Send Text");
        assert!(!e.uuid.is_empty());
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn round_trip_through_the_flat_file_is_lossless() {
        let p = temp("roundtrip");
        let mut c = QuickBarConfig::default();
        c.groups[0].commands[0].label = "My Copy".into();
        c.groups[0].label_zh = "文档".into();
        assert!(save_to(&p, &c));
        assert_eq!(load_from(&p), c, "嵌套→扁平→嵌套必须原样回来");
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn an_explicit_empty_group_list_is_not_replaced_by_defaults() {
        // 扁平格式里「空数组」是用户故意清空。
        let p = temp("empty");
        assert!(save_to(&p, &QuickBarConfig { groups: vec![] }));
        let loaded = load_from(&p);
        assert!(loaded.groups.is_empty(), "显式清空是用户意图");
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn a_real_windterm_file_reads_with_label_as_send() {
        // 参考实现文件没有 `send` 键，读进来 send 必须回落成 label，按钮才可用。
        let p = temp("windterm");
        std::fs::write(
            &p,
            r#"[
                {"quick.group":"Quick","quick.icon":"session::square-deeppink",
                 "quick.label":"Untitled","quick.type":"Send Text",
                 "quick.uuid":"1bddafdd-c94c-4d0d-be1d-7c2cbe5cd7d6"},
                {"quick.group":"2","quick.icon":"session::arrow-coral",
                 "quick.label":"ls","quick.type":"Send Text",
                 "quick.uuid":"41aa879d-984d-4337-bc20-917410420096"}
            ]"#,
        )
        .unwrap();
        let c = load_from(&p);
        assert_eq!(c.groups.len(), 2);
        let quick = &c.groups[0];
        assert_eq!(quick.label, "Quick");
        assert_eq!(quick.commands[0].label, "Untitled");
        assert_eq!(quick.commands[0].send, "Untitled", "参考实现无 send → 用 label");
        assert_eq!(quick.commands[0].mark, "square", "deeppink 颜色忽略，图形取 square");
        // coral 认得出 → 组色是 coral 的 hex。
        assert_eq!(c.groups[1].color, "#FF7F50");
        assert_eq!(c.groups[1].commands[0].send, "ls");
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn an_unknown_mark_or_color_falls_back_without_panicking() {
        let p = temp("fallback");
        std::fs::write(
            &p,
            r#"[{"quick.group":"G","quick.icon":"session::nope-#ZZZ",
                 "quick.label":"x","quick.uuid":"u1"}]"#,
        )
        .unwrap();
        let c = load_from(&p);
        assert_eq!(c.groups[0].commands[0].mark, "nope", "未知图形就原样留着，渲染层回落");
        assert_eq!(c.groups[0].color, default_color(), "坏颜色回落出厂蓝");
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn malformed_file_falls_back_without_panicking() {
        let p = temp("bad");
        std::fs::write(&p, "{ not json").unwrap();
        assert_eq!(load_from(&p), QuickBarConfig::default());
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn round_trip_and_broken_file_is_preserved_before_overwrite() {
        let p = temp("roundtrip");
        // ui-align 已将 show_group_title 等三开关从 QuickBarConfig 迁到 Prefs，
        // 故此处直接用 default()，不再做结构体更新语法。
        let mut c = QuickBarConfig::default();
        c.groups[0].commands[0].label = "My Copy".into();
        assert!(save_to(&p, &c));
        assert_eq!(load_from(&p), c);

        // 写坏之后再覆盖：原件要被逐字节保全，新的值要生效。
        std::fs::write(&p, "{ oops").unwrap();
        let next = QuickBarConfig::default();
        assert!(save_to(&p, &next));
        assert_eq!(load_from(&p), next);
        let preserved: Vec<_> = std::fs::read_dir(p.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|q| q.to_string_lossy().contains("bad-"))
            .collect();
        assert!(!preserved.is_empty(), "坏文件必须留一份副本");
        for q in preserved {
            std::fs::remove_file(q).ok();
        }
        std::fs::remove_file(p).ok();
    }
}
