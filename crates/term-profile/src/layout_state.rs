//! V1-1：窗口布局**结构**的持久化。
//!
//! `docs/05-feature-scope.md §2.2` 把边界写死了：恢复的是标签 / 分屏 / 会话引用，
//! **不是进程内状态**（回滚内容、运行中的进程、选区）。所以这里存的是会话名字
//! 与一棵二分树，快照与 PTY 句柄一个字节都不落盘。
//!
//! 文件放在 `term_paths::profile_dir()` 下（和 `sessions.db` 同目录），原子写：
//! 写到一半断电，丢的是「这次改动」，不是整个布局。截图基线用一次性 profile
//! 目录，所以这份文件不会污染视觉回归。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const LAYOUT_FILE: &str = "layout.json";

/// 布局文件的格式版本。ADR-0025 触发条件 #5 要求：**文件带版本，读的时候校验**。
///
/// 它只存在于文件里，不是 `SavedLayout` 的字段（理由见 [`save_to`]）。语义：
/// - 缺这个键 = `0`，即 V1-1 起「只有 `tabs` / `active` / `tree`、后加 `dock`」的那套；
/// - 比本程序新 → **不读**。宁可恢复成一条本地 shell，也不要按旧规则去解释新字段：
///   那种「读一半」会静默改掉标签顺序与分屏形状，用户以为记住了、其实记错了。
///
/// 各版本：
/// - `1`：只有 `tabs` / `active` / `tree`（叶子挂下标），后加 `dock`。分屏恒 50%。
/// - `2`：`Split` 节点新增 `ratio`（千分比，默认 `500` = 50%），分屏比例可拖动保存。
///
/// 注：按 `v1` 那套「纯新增 `#[serde(default)]` 字段不升版本」的惯例，`ratio` 本可
/// 不升——但产品上把它当作几何语义变化（老程序忽略比例、仍画 50% 即可拒绝误读），
/// 故显式升到 `2`，让「带比例的布局」与「不带比例的旧布局」在文件层面分清。
pub const LAYOUT_SCHEMA_VERSION: u64 = 2;

/// 分屏比例以「千分比」存储：`500` = 50%。与运行态 `Layout` 同一把标尺。
const SPLIT_RATIO_DEFAULT: u16 = 500;
const SPLIT_RATIO_MIN: u16 = 100;
const SPLIT_RATIO_MAX: u16 = 900;

/// `skip_serializing_if` 用：比例是默认 50% 时不写进 JSON，文件保持紧凑。
fn is_default_ratio(r: &u16) -> bool {
    *r == SPLIT_RATIO_DEFAULT
}

/// `serde(default)` 用：JSON 里缺比例时按 50% 理解（老文件恒 50%，不能让 `u16`
/// 的 `Default`（=0）进来——`sanitize` 会把 0 夹到 `SPLIT_RATIO_MIN`，读回就变 100‰）。
fn default_split_ratio() -> u16 {
    SPLIT_RATIO_DEFAULT
}

/// 二分布局树。`horizontal = true` = 左右并排，叶子挂 `tabs` 的下标。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SavedNode {
    Leaf(usize),
    Split {
        horizontal: bool,
        #[serde(default = "default_split_ratio", skip_serializing_if = "is_default_ratio")]
        ratio: u16,
        first: Box<SavedNode>,
        second: Box<SavedNode>,
    },
}

/// 一条标签的**引用**。存名字而不是整份 `SessionConfig`：会话库才是权威，
/// 恢复时按名字重新查，用户改过的密码 / 分组不会被旧布局覆盖回去。
///
/// # N：会话运行态（v0.2 起可选）
/// 三个字段全部 `Option` + `serde(default)`：旧 `layout.json`（只有 `name`）
/// 读入为 `None`，`restore_layout` 遇 `None` 走现有行为——不崩、不清旧文件。
/// 新文件里没变化的标签就不写这些字段（`skip_serializing_if`），文件保持紧凑。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedTab {
    pub name: String,
    /// 会话打开时的工作目录（shell 起始 cwd）。`None` = 走会话默认。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 视口顶端相对历史第一条的滚动 offset（0 = 历史顶部，`history_len` = 贴底）。
    /// `None` = 重开后回到贴底，与旧行为一致。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_top: Option<u64>,
    /// 该 tab 保留的回滚长度（行）。`None` = 不干预，由会话自己决定。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_len: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedLayout {
    pub tabs: Vec<SavedTab>,
    pub active: usize,
    pub tree: SavedNode,
    /// P4：停靠 / 浮动位置。**老文件里没有这一项**（V1-1 时就存在了），所以
    /// 必须 `default`——读旧布局不能因为少一个字段整份作废。
    #[serde(default)]
    pub dock: Vec<SavedPlacement>,
}

/// 一个面板停在哪。`zone` 是 `"left"` / `"right"` / `"bottom"` 或 `"float"`；
/// 停靠时看 `order`（带内位次），浮动时看 `rect`。
///
/// 用字符串而不是 `PanelId` 枚举：`term-runtime` 不依赖 `term-app`（依赖方向是
/// app → runtime 之外的另一棵树），而且布局文件要能容忍一个**已经删掉的面板名**
/// ——读到不认识的键就跳过那一条，不要整份布局作废。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPlacement {
    pub panel: String,
    pub zone: String,
    #[serde(default)]
    pub order: u16,
    /// 浮动时的矩形 `[x, y, w, h]`，单位是整点。停靠时为 `None`。
    ///
    /// 存整点而不是 `f32`：窗口位置没有亚像素意义，而整型让
    /// `SavedLayout` 继续能派生 `Eq`（`f32` 不能），比对与去重都好写。
    #[serde(default)]
    pub rect: Option<[i32; 4]>,
}

impl SavedLayout {
    /// 单窗格（新装 / 没有布局文件时用它）。
    ///
    /// N：新标签三个运行态字段省略——`Option::is_none` 让它不进 JSON，
    /// 与「旧文件只有 name」在序列化层面等价（读入都是 None）。
    pub fn single(name: &str) -> Self {
        Self {
            tabs: vec![SavedTab {
                name: name.to_string(),
                cwd: None,
                scroll_top: None,
                history_len: None,
            }],
            active: 0,
            tree: SavedNode::Leaf(0),
            dock: Vec::new(),
        }
    }

    /// 库里可能来自旧版本或 hand-edited 文件：下标越界就地收敛，不 panic。
    pub fn sanitize(&mut self) {
        self.dock = sanitize_dock(std::mem::take(&mut self.dock));
        if self.tabs.is_empty() {
            *self = SavedLayout::single("Local Shell");
            return;
        }
        if self.active >= self.tabs.len() {
            self.active = 0;
        }
        fn clamp(node: &mut SavedNode, n: usize) {
            match node {
                SavedNode::Leaf(i) => {
                    if *i >= n {
                        *i = 0;
                    }
                }
                SavedNode::Split {
                    first,
                    second,
                    ratio,
                    ..
                } => {
                    // 手改 / 损坏文件里可能是越界比例：夹回可用区间，避免一格消失。
                    *ratio = (*ratio).clamp(SPLIT_RATIO_MIN, SPLIT_RATIO_MAX);
                    clamp(first, n);
                    clamp(second, n);
                }
            }
        }
        clamp(&mut self.tree, self.tabs.len());
    }
}

/// 停靠项的清洗：去重 + 丢掉小到看不见的浮动窗。
///
/// **不校验带名**：`left` / `right` / `bottom` 这份表只有 `term_app::dock::DockZone`
/// 一份，这里再抄一遍就是第二个真相源。看不懂的带名由投影层的 `from_saved` 跳过
/// ——它本来就认识那张表。这一层只保证「一条坏记录不会让整份布局作废」。
///
/// 为什么「小到看不见」等于丢掉：一个 8×8 的浮动窗在屏幕上就是一个抓不到的点，
/// 用户既找不回也不可能再拖动它。宁可让这一条退回默认停靠位。
fn sanitize_dock(items: Vec<SavedPlacement>) -> Vec<SavedPlacement> {
    let mut out: Vec<SavedPlacement> = Vec::new();
    for item in items {
        if out.iter().any(|p| p.panel == item.panel) {
            continue;
        }
        if item.zone == "float" {
            match item.rect {
                Some([_, _, w, h]) if w >= 80 && h >= 60 => {}
                _ => continue,
            }
        }
        out.push(item);
    }
    out
}

pub(crate) fn layout_path() -> Option<PathBuf> {
    Some(term_paths::profile_dir()?.join(LAYOUT_FILE))
}

// L3 的元信息（时间戳 / 退出形态）单独成文件：`layout_state` 已经到 R9 的单文件
// 行数上限，而它们是**另一个关注点**（文件属性 vs 布局语义）。这里再导出一遍，
// 是为了让调用方只认 `layout_state` 一个入口，不必知道它内部拆成了几块。
pub use crate::layout_meta::{
    clear_exit, load_meta, mark_exit, LayoutMeta, EXIT_CLEAN, EXIT_CRASH, RECENT_WINDOW_SECS,
};

/// 保存失败不让窗口崩（布局是可用性资产，不是数据资产），但也不装作成功。
pub fn save(layout: &SavedLayout) -> bool {
    match layout_path() {
        Some(path) => save_to(&path, layout),
        None => false,
    }
}

/// 运行期落盘：写布局并**盖上时间戳**（`save` 的带戳版本）。
///
/// 只有运行期的三条写入路径（周期留存 / 事件驱动 / 正常退出）该盖戳；测试与
/// 手工构造的落盘不需要，走 [`save`] 即可。
pub fn save_run(layout: &SavedLayout, saved_at: u64) -> bool {
    match layout_path() {
        Some(path) => save_to_stamped(&path, layout, Some(saved_at)),
        None => false,
    }
}

pub fn load() -> Option<SavedLayout> {
    layout_path().and_then(|p| load_from(&p))
}

fn save_to(path: &Path, layout: &SavedLayout) -> bool {
    save_to_stamped(path, layout, None)
}

pub(crate) fn save_to_stamped(
    path: &Path,
    layout: &SavedLayout,
    saved_at: Option<u64>,
) -> bool {
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    // 版本**只写在文件里**，不进 `SavedLayout`：它是「文件的属性」，不是布局的语义
    // 字段。塞进结构体就要在每个构造点写一个自己根本没有意见的数字，而那个数字
    // 一旦被某个调用点写成旧值，文件就带着错版本落盘——比现在这样更难守。
    let Ok(mut value) = serde_json::to_value(layout) else {
        return false;
    };
    if let Some(map) = value.as_object_mut() {
        map.insert(
            "schema_version".to_string(),
            serde_json::Value::from(LAYOUT_SCHEMA_VERSION),
        );
        // 时间戳与版本同一层：它描述「这份文件什么时候写的」，不是布局语义。
        if let Some(at) = saved_at {
            map.insert("last_saved_at".to_string(), serde_json::Value::from(at));
        }
    }
    let Ok(json) = serde_json::to_string_pretty(&value) else {
        return false;
    };
    crate::persist::write_atomic(path, json.as_bytes())
}

pub(crate) fn load_from(path: &Path) -> Option<SavedLayout> {
    if !path.is_file() {
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    // **先看版本，再看内容**。比自己新的格式不猜：按旧规则去解释新字段的结果是
    // 「读一半」——标签顺序、分屏形状会静默变成别的东西，用户以为程序记住了，
    // 其实记错了。直接回「没有布局」（于是恢复一条本地 shell）是可解释的降级。
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let version = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if version > LAYOUT_SCHEMA_VERSION {
        return None;
    }
    let mut layout: SavedLayout = serde_json::from_value(value).ok()?;
    layout.sanitize();
    Some(layout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn tmp() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "windterm-layout-{}-{}-{}",
            std::process::id(),
            nanos,
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(LAYOUT_FILE)
    }

    #[test]
    fn a_split_layout_round_trips_without_losing_the_tree() {
        let path = tmp();
        let layout = SavedLayout {
            tabs: vec![
                SavedTab { name: "a".into(), cwd: None, scroll_top: None, history_len: None },
                SavedTab { name: "b".into(), cwd: None, scroll_top: None, history_len: None },
                SavedTab { name: "c".into(), cwd: None, scroll_top: None, history_len: None },
            ],
            active: 2,
            tree: SavedNode::Split {
                horizontal: true,
                ratio: 500,
                first: Box::new(SavedNode::Leaf(0)),
                second: Box::new(SavedNode::Split {
                    horizontal: false,
                    ratio: 500,
                    first: Box::new(SavedNode::Leaf(1)),
                    second: Box::new(SavedNode::Leaf(2)),
                }),
            },
            dock: Vec::new(),
        };
        assert!(save_to(&path, &layout));
        assert_eq!(load_from(&path), Some(layout));
    }

    #[test]
    fn missing_file_is_none_not_a_fake_layout() {
        let path = tmp();
        assert_eq!(load_from(&path), None);
        assert!(!path.exists(), "读一次不该把文件建出来");
    }

    #[test]
    fn a_corrupt_layout_falls_back_to_none() {
        let path = tmp();
        std::fs::write(&path, "{ this is not json").unwrap();
        assert_eq!(load_from(&path), None);
    }

    /// ADR-0025 触发条件 #5：文件带版本，且**比自己新的不读**。
    ///
    /// 这条盯的是「静默读一半」：新版本可能改了字段含义（比如 `tree` 的叶子从下标
    /// 改成身份），旧程序照旧规则去解释，标签顺序与分屏形状就会变成别的东西——
    /// 用户以为记住了、其实记错了，这比「没恢复」更难发现。
    #[test]
    fn a_layout_from_a_newer_build_is_refused_instead_of_misread() {
        let path = tmp();
        let raw = format!(
            r#"{{"schema_version": {}, "tabs": [{{"name": "a"}}], "active": 0, "tree": {{"Leaf": 0}}}}"#,
            LAYOUT_SCHEMA_VERSION + 1
        );
        std::fs::write(&path, raw).unwrap();
        assert_eq!(
            load_from(&path),
            None,
            "比本程序新的格式必须拒绝，不能按旧规则猜"
        );
    }

    /// 落盘的文件必须**带着**当前版本；老文件（没有这个键）读成 0 照常可用。
    #[test]
    fn saving_stamps_the_current_schema_version_and_old_files_still_load() {
        let path = tmp();
        let layout = SavedLayout {
            tabs: vec![SavedTab {
                name: "only".into(),
                cwd: None,
                scroll_top: None,
                history_len: None,
            }],
            active: 0,
            tree: SavedNode::Leaf(0),
            dock: Vec::new(),
        };
        assert!(save_to(&path, &layout));
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains(&format!("\"schema_version\": {LAYOUT_SCHEMA_VERSION}")),
            "落盘必须盖章：{raw}"
        );
        assert_eq!(load_from(&path), Some(layout), "盖了章也照样是个可读的布局");

        // 老文件（V1-1 那套，没有 `schema_version` 键）读成 0，不该被新校验挡掉。
        let old = r#"{"tabs": [{"name": "only"}], "active": 0, "tree": {"Leaf": 0}}"#;
        std::fs::write(&path, old).unwrap();
        let loaded = load_from(&path).expect("老文件必须能读");
        assert_eq!(loaded.tabs.len(), 1);
        assert_eq!(loaded.tabs[0].name, "only");
    }

    /// V2：分屏比例必须落盘并能原值读回；缺比例的旧文件（V1-1）读成默认 50%。
    #[test]
    fn v2_split_ratio_round_trips_and_old_files_default_to_half() {
        let path = tmp();
        let layout = SavedLayout {
            tabs: vec![
                SavedTab { name: "a".into(), cwd: None, scroll_top: None, history_len: None },
                SavedTab { name: "b".into(), cwd: None, scroll_top: None, history_len: None },
            ],
            active: 0,
            tree: SavedNode::Split {
                horizontal: true,
                ratio: 300,
                first: Box::new(SavedNode::Leaf(0)),
                second: Box::new(SavedNode::Leaf(1)),
            },
            dock: Vec::new(),
        };
        assert!(save_to(&path, &layout));
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains("\"ratio\": 300"),
            "比例必须落盘（非默认值时）: {raw}"
        );
        // 落盘必须是 V2。
        assert!(
            raw.contains(&format!("\"schema_version\": {LAYOUT_SCHEMA_VERSION}")),
            "落盘必须盖 V2 章: {raw}"
        );
        let loaded = load_from(&path).expect("带比例的 V2 必须能读");
        assert_eq!(loaded.tree, layout.tree, "比例必须逐值保持");

        // 老文件（V1-1，没有 ratio，分屏恒 50%）：读入后比例取默认 500。
        let old = r#"{"tabs":[{"name":"a"},{"name":"b"}],"active":0,"tree":{"Split":{"horizontal":true,"first":{"Leaf":0},"second":{"Leaf":1}}}}"#;
        std::fs::write(&path, old).unwrap();
        let old_loaded = load_from(&path).expect("老文件必须能读");
        match &old_loaded.tree {
            SavedNode::Split { ratio, .. } => {
                assert_eq!(*ratio, 500, "缺省比例应为 50%（500‰）")
            }
            _ => panic!("应为分屏树"),
        }
    }

    #[test]
    fn sanitize_clamps_out_of_range_indices_and_active_tab() {
        let mut layout = SavedLayout {
            tabs: vec![SavedTab {
                name: "only".into(),
                cwd: None,
                scroll_top: None,
                history_len: None,
            }],
            active: 9,
            tree: SavedNode::Split {
                horizontal: false,
                ratio: 500,
                first: Box::new(SavedNode::Leaf(7)),
                second: Box::new(SavedNode::Leaf(0)),
            },
            dock: Vec::new(),
        };
        layout.sanitize();
        assert_eq!(layout.active, 0);
        assert_eq!(
            layout.tree,
            SavedNode::Split {
                horizontal: false,
                ratio: 500,
                first: Box::new(SavedNode::Leaf(0)),
                second: Box::new(SavedNode::Leaf(0)),
            }
        );
    }

    #[test]
    fn empty_tabs_become_one_local_shell() {
        let mut layout = SavedLayout {
            tabs: vec![],
            active: 3,
            tree: SavedNode::Leaf(0),
            dock: Vec::new(),
        };
        layout.sanitize();
        assert_eq!(layout, SavedLayout::single("Local Shell"));
    }

    #[test]
    fn saving_twice_leaves_no_temp_file() {
        let path = tmp();
        assert!(save_to(&path, &SavedLayout::single("x")));
        assert!(save_to(&path, &SavedLayout::single("y")));
        // 不绑死具体的临时文件名：唯一名带 pid / 纳秒 / 序号，扫目录看残留。
        assert!(
            crate::persist::tmp_leftovers(path.parent().unwrap()).is_empty(),
            "两次原子写都不该留下 tmp 文件"
        );
        assert_eq!(load_from(&path), Some(SavedLayout::single("y")));
    }

    // ── P4：停靠位置 ──

    #[test]
    fn a_layout_file_from_before_p4_still_loads() {
        // V1-1 时代写的文件里没有 `dock` 这一项。它必须还能读——否则升级一次
        // 用户就丢掉全部标签与分屏，而丢的原因只是少了一个新字段。
        let path = tmp();
        std::fs::write(
            &path,
            r#"{"tabs":[{"name":"a"}],"active":0,"tree":{"Leaf":0}}"#,
        )
        .unwrap();
        let loaded = load_from(&path).expect("旧布局要能读");
        assert_eq!(loaded.tabs.len(), 1);
        assert!(loaded.dock.is_empty(), "没有停靠项 = 用默认布局");
    }

    #[test]
    fn dock_placements_round_trip() {
        let path = tmp();
        let mut layout = SavedLayout::single("a");
        layout.dock = vec![
            SavedPlacement {
                panel: "sidebar".into(),
                zone: "float".into(),
                order: 0,
                rect: Some([120, 90, 260, 420]),
            },
            SavedPlacement {
                panel: "filer".into(),
                zone: "left".into(),
                order: 1,
                rect: None,
            },
        ];
        assert!(save_to(&path, &layout));
        assert_eq!(load_from(&path), Some(layout));
    }

    #[test]
    fn sanitize_drops_tiny_floats_and_duplicate_panels() {
        let mut layout = SavedLayout::single("a");
        layout.dock = vec![
            SavedPlacement {
                panel: "sidebar".into(),
                zone: "float".into(),
                order: 0,
                rect: Some([10, 10, 12, 12]),
            },
            SavedPlacement {
                panel: "filer".into(),
                zone: "right".into(),
                order: 0,
                rect: None,
            },
            SavedPlacement {
                panel: "filer".into(),
                zone: "left".into(),
                order: 0,
                rect: None,
            },
        ];
        layout.sanitize();
        assert_eq!(
            layout.dock.len(),
            1,
            "抓不到的浮动窗要丢，重复的面板只留第一条"
        );
        assert_eq!(layout.dock[0].panel, "filer");
    }

    // ── N：状态留存 —— 旧格式兼容与新格式 round-trip ──

    #[test]
    fn n_old_layout_without_runtime_fields_still_loads() {
        // N-A2: 旧 layout.json（只有 name，无 cwd/scroll_top/history_len）读入不崩、
        // 行为同旧版：三个新字段都是 None，恢复时走现有回退路径。
        let path = tmp();
        std::fs::write(
            &path,
            r#"{"tabs":[{"name":"a"},{"name":"b"}],"active":1,"tree":{"Leaf":0},"dock":[]}"#,
        )
        .unwrap();
        let loaded = load_from(&path).expect("旧布局要能读");
        assert_eq!(loaded.tabs.len(), 2);
        assert_eq!(loaded.tabs[0].cwd, None);
        assert_eq!(loaded.tabs[0].scroll_top, None);
        assert_eq!(loaded.tabs[0].history_len, None);
        assert_eq!(loaded.tabs[1].cwd, None);
    }

    #[test]
    fn n_new_layout_with_runtime_fields_round_trips() {
        // N-A5: 新格式（含 cwd/scroll_top/history_len）序列化 -> 反序列化逐值相同。
        let path = tmp();
        let layout = SavedLayout {
            tabs: vec![
                SavedTab {
                    name: "remote".into(),
                    cwd: Some("/home/user/project".into()),
                    scroll_top: Some(120),
                    history_len: Some(10000),
                },
                SavedTab {
                    name: "local".into(),
                    cwd: None,
                    scroll_top: None,
                    history_len: None,
                },
            ],
            active: 0,
            tree: SavedNode::Leaf(0),
            dock: Vec::new(),
        };
        assert!(save_to(&path, &layout));
        let loaded = load_from(&path).expect("新布局要能读");
        assert_eq!(loaded, layout, "新字段必须逐值保持");
    }
}
