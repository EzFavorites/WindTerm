//! Session Settings ▸ Logging 的**规格与状态**：视觉基线常量、协议/导航表、
//! 跨帧表单数据与 `prefs.json` 往返。
//!
//! 与 `log_settings.rs`（只画）分开是为了  的 450 行上限：数据面 + 测试
//! 和绘制面加起来 738 行，会一条门禁红掉整个阶段。
//!
//! 取色与几何逐点取自 视觉基线 `clipboard-…e44425d2.png`
//! （1280×1140 @2x ⇒ 640×570 逻辑点），与菜单三态同一套灰阶：
//! ```text
//! 顶部  Shell(选中 #1C1C1C) | SSH | Telnet | Tcp | Serial（未选中 #333333）
//! 左栏  Session / ▼Terminal(Bell,Mode,Keyboard,Security,Logging) /
//!       ▼Window(Selection) / ▼Shell(Process, X/Y/Z Modem)，Logging 整行 #282828
//! 右栏  Log types: 三单选 → Log options: 两置灰单选 → Log file name:
//!       → Log maximum size: ∞ → Log content pattern:
//! 页脚  Edit Default Settings...（蓝描边） | Cancel | Save
//! 正文 #1C1C1C / 文字 #BBBBBB / 禁用 #555555 / 树选中 #282828 /
//! 焦点环 #3EA0EC / 按钮 #262626
//! ```

use crate::ui::*;

/// 播种尺寸故意小于窗口内容高：`fixed_rect` 要能压住外框，否则
/// `set_min_size` 会把 570 的窗口撑到 620+，页脚跟着漂到画布下沿。
pub(in crate::ui) const CONTENT_MIN: egui::Vec2 = egui::Vec2::new(624.0, 480.0);
pub(in crate::ui) const TAB_H: f32 = 26.0;
pub(in crate::ui) const NAV_W: f32 = 212.0;
pub(in crate::ui) const ROW_H: f32 = 24.0;
pub(in crate::ui) const INDENT: f32 = 20.0;
pub(in crate::ui) const RADIO_H: f32 = 28.0;
/// 两栏内容区高度：右栏表单到底约 340，留足空档给视觉基线那片空白。
pub(in crate::ui) const BODY_H: f32 = 400.0;
/// 右栏「标签 + 控件」两行：标签列 152，控件列 248，列起点与视觉基线一致
/// （右栏 x≈228，控件从 x≈384 起、到 x≈632 收）。
pub(in crate::ui) const LABEL_W: f32 = 152.0;
pub(in crate::ui) const FIELD_W: f32 = 248.0;
/// 页脚按钮距**窗口顶边**的偏移（原绝对 y 616 减去窗口顶 80）。
/// 用相对量是因为窗口位置现在由屏幕中心推算，绝对 y 会留在原地。
pub(in crate::ui) const FOOTER_DY: f32 = 536.0;
pub(in crate::ui) const BTN_H: f32 = 24.0;

pub(in crate::ui) const TAB_BG: u32 = 0x333333;
pub(in crate::ui) const TAB_ACTIVE: u32 = 0x1C1C1C;
pub(in crate::ui) const TREE_SELECTED: u32 = 0x282828;
pub(in crate::ui) const TREE_HOVER: u32 = 0x242424;
pub(in crate::ui) const TEXT: u32 = 0xBBBBBB;
pub(in crate::ui) const TEXT_DISABLED: u32 = 0x555555;
pub(in crate::ui) const TEXT_STRONG: u32 = 0xE8E8E8;
pub(in crate::ui) const FIELD_FILL: u32 = 0x333333;
pub(in crate::ui) const FOCUS_BLUE: u32 = 0x3EA0EC;
pub(in crate::ui) const BUTTON_BG: u32 = 0x262626;

/// Log types 三选一。**类型本体在 `term_core::log_opts`**：写入端、无头桥接、
/// 偏好文件都要用同一份语义，这里只是把路径再导出一次，让 `super::log_settings_spec::LogType`
/// 这类既有引用照旧解析。曾经在这里有一份同形状的枚举，与 `LogOptions` 各解释
/// 一遍 `0/1/2`——那正是「两处各写一份迟早分叉」的老路。
pub(in crate::ui) use term_core::log_opts::LogType;

/// 配置页的跨帧控件态。默认值逐字取自视觉基线。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui) struct LogSettingsData {
    pub(in crate::ui) protocol: Protocol,
    pub(in crate::ui) page: usize,
    pub(in crate::ui) groups_open: [bool; 3],
    pub(in crate::ui) log_type: LogType,
    /// `None` = 视觉基线出厂态：两行都没选（且都禁用）。
    pub(in crate::ui) overwrite: Option<bool>,
    pub(in crate::ui) file_name: String,
    /// 0 = ∞（视觉基线）。
    pub(in crate::ui) max_size_mb: u32,
    pub(in crate::ui) pattern: String,
}

impl Default for LogSettingsData {
    fn default() -> Self {
        Self {
            protocol: Protocol::Shell,
            page: LOGGING_ROW,
            groups_open: [true; 3],
            log_type: LogType::Nothing,
            overwrite: None,
            file_name: "%n_%Y-%m-%d_%H-%M-%S.log".to_string(),
            max_size_mb: 0,
            pattern: "[%Y-%m-%d %H:%M:%S] %v".to_string(),
        }
    }
}

impl LogSettingsData {
    /// 界面态 → 领域选项。**手动 Start 走的就是这条路**：设置页里那五个字段
    /// 从此不再只是落盘，写入端真的按它们落盘。
    pub(in crate::ui) fn options(&self) -> term_core::log_opts::LogOptions {
        term_core::log_opts::LogOptions {
            log_type: self.log_type,
            overwrite: self.overwrite,
            file_name: self.file_name.clone(),
            max_size_mb: self.max_size_mb,
            content_pattern: self.pattern.clone(),
        }
    }

    /// 从 `prefs.json` 读回。空串 / 序号解释全在 `Prefs::log_options` 里——
    /// 那一份是 `prefs.json` 五个字段的唯一下游，这里不再抄第二遍。
    pub(in crate::ui) fn from_prefs(prefs: Option<&term_runtime::prefs::Prefs>) -> Self {
        let mut out = Self::default();
        let Some(p) = prefs else { return out };
        let opts = p.log_options();
        out.log_type = opts.log_type;
        out.overwrite = opts.overwrite;
        out.file_name = opts.file_name;
        out.max_size_mb = opts.max_size_mb;
        out.pattern = opts.content_pattern;
        out
    }

    /// load-modify-save：语言与两个关闭抑制位必须原样保留。
    pub(in crate::ui) fn save(&self) -> bool {
        let mut prefs = term_runtime::prefs::load().unwrap_or_default();
        prefs.set_log_options(&self.options());
        term_runtime::prefs::save(&prefs)
    }
}

/// 协议标签条：顺序与显示名逐字取自视觉基线（刻意不复用 `Protocol::label`）。
pub(in crate::ui) const TABS: [(Protocol, &str); 5] = [
    (Protocol::Shell, "Shell"),
    (Protocol::Ssh, "SSH"),
    (Protocol::Telnet, "Telnet"),
    (Protocol::Tcp, "Tcp"),
    (Protocol::Serial, "Serial"),
];

/// 左树的一行。`group = Some(i)` 表示受第 i 个折叠组管辖；`header` 是组头。
#[derive(Clone, Copy)]
pub(in crate::ui) struct NavRow {
    pub(in crate::ui) label: &'static str,
    pub(in crate::ui) indent: f32,
    pub(in crate::ui) group: Option<usize>,
    pub(in crate::ui) header: bool,
}

/// `Logging` 在**出厂态（Shell 树）**里的行号。`session_nav` 的树是唯一来源，
/// 这里只是一个「默认停在哪一行」的常量，由
/// `logging_row(Shell) == LOGGING_ROW` 守着不漂。
pub(in crate::ui) const LOGGING_ROW: usize = 6;

/// 左树：**从 `session_nav` 的树派生**，不再自己写第二份。
///
/// 之前这里是独立的十二行枚举（SSH 组只有三项、Shell 组叫
/// `Process`），与新视觉基线集里 New Session 的树对不上——同一个产品里两棵
/// 「同一棵树」各自漂移，正是 `session_nav.rs` 模块头警告过的事。
/// 现在行数、文字、层级全部来自 `nav_for` + `PAGES`，两处不可能再分叉。
///
/// `group` 是折叠组的序号（`groups_open[i]` 的下标），按一级条目的出现
/// 顺序编号：`Terminal=0 / Window=1 / 协议组=2`。
pub(in crate::ui) fn nav_rows(protocol: Protocol) -> Vec<NavRow> {
    let mut rows = Vec::new();
    let mut group_index = 0usize;
    for entry in super::session_nav::nav_for(protocol) {
        match entry {
            super::session_nav::NavEntry::Leaf(l) => rows.push(NavRow {
                label: l.label,
                indent: 0.0,
                group: None,
                header: false,
            }),
            // `page` 是组头自己那一页的序号；这个列表只用 label / 层级，
            // 行号沿用「组头一行 + 子项各一行」的旧口径，故忽略。
            super::session_nav::NavEntry::Group {
                label,
                page: _,
                children,
            } => {
                rows.push(NavRow {
                    label,
                    indent: 0.0,
                    group: Some(group_index),
                    header: true,
                });
                rows.extend(children.iter().map(|c| NavRow {
                    label: c.label,
                    indent: INDENT,
                    group: Some(group_index),
                    header: false,
                }));
                group_index += 1;
            }
        }
    }
    rows
}

pub(in crate::ui) fn logging_row(protocol: Protocol) -> usize {
    nav_rows(protocol)
        .iter()
        .position(|r| r.label == "Logging")
        .unwrap_or(LOGGING_ROW)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 出厂态（Shell 协议）左树 13 行：文字与层级逐字钉住。
    /// 行数与 `session_nav::nav_for(Shell)` 完全同源，这里钉的是「派生的形状对不对」。
    #[test]
    fn shell_nav_matches_the_reference_thirteen_rows() {
        let rows = nav_rows(Protocol::Shell);
        let shape: Vec<(&str, f32, bool)> = rows
            .iter()
            .map(|r| (r.label, r.indent, r.header))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("Session", 0.0, false),
                ("Terminal", 0.0, true),
                ("Bell", INDENT, false),
                ("Mode", INDENT, false),
                ("Keyboard", INDENT, false),
                ("Security", INDENT, false),
                ("Logging", INDENT, false),
                ("Window", 0.0, true),
                ("Selection", INDENT, false),
                ("Shell", 0.0, true),
                ("Command", INDENT, false),
                ("Environment", INDENT, false),
                ("X/Y/Z Modem", INDENT, false),
            ]
        );
        assert_eq!(logging_row(Protocol::Shell), LOGGING_ROW);
    }

    /// 两个入口（New Session 弹窗 / Session Settings 窗口）用的是**同一棵树**。
    /// 这条守的是「别再出现第二份手写枚举」——上一版 SSH 组只有三项就是那么来的。
    #[test]
    fn both_entry_points_share_one_tree() {
        for protocol in Protocol::ALL {
            let rows = nav_rows(protocol);
            let mut flat = Vec::new();
            for entry in crate::ui::dialogs::session_nav::nav_for(protocol) {
                match entry {
                    crate::ui::dialogs::session_nav::NavEntry::Leaf(l) => flat.push(l.label),
                    crate::ui::dialogs::session_nav::NavEntry::Group {
                        label,
                        page: _,
                        children,
                    } => {
                        flat.push(label);
                        flat.extend(children.iter().map(|c| c.label));
                    }
                }
            }
            let our: Vec<&str> = rows.iter().map(|r| r.label).collect();
            assert_eq!(our, flat, "{protocol} 的两棵树不一致");
            // 组头恰好三个，且编号连续（0..2），否则折叠状态会串组。
            let groups: Vec<usize> = rows.iter().filter(|r| r.header).filter_map(|r| r.group).collect();
            assert_eq!(groups, vec![0, 1, 2], "{protocol} 的折叠组编号不连续");
            // 每个叶子都归属某个组，只有 Session 是独立项。
            let orphans: Vec<&str> = rows
                .iter()
                .filter(|r| !r.header && r.group.is_none())
                .map(|r| r.label)
                .collect();
            assert_eq!(orphans, vec!["Session"]);
        }
    }

    #[test]
    fn protocol_tabs_match_the_reference_order() {
        let labels: Vec<&str> = TABS.iter().map(|(_, l)| *l).collect();
        assert_eq!(labels, vec!["Shell", "SSH", "Telnet", "Tcp", "Serial"]);
    }

    #[test]
    fn factory_defaults_are_the_reference_state() {
        let d = LogSettingsData::default();
        assert_eq!(d.protocol, Protocol::Shell);
        assert_eq!(d.page, LOGGING_ROW);
        assert_eq!(d.log_type, LogType::Nothing);
        assert_eq!(d.overwrite, None, "视觉基线出厂态两行 Log options 都没选");
        assert_eq!(d.file_name, "%n_%Y-%m-%d_%H-%M-%S.log");
        assert_eq!(d.max_size_mb, 0);
        assert_eq!(d.pattern, "[%Y-%m-%d %H:%M:%S] %v");
        assert!(d.groups_open.iter().all(|g| *g));
    }

    #[test]
    fn log_options_only_exist_when_something_is_logged() {
        assert!(!LogType::Nothing.enabled());
        assert!(LogType::Printable.enabled());
        assert!(LogType::All.enabled());
        // 与视觉基线出厂态一致：Log nothing ⇒ 两行 Overwrite / Append 全灰。
        assert!(!LogSettingsData::default().log_type.enabled());
    }

    #[test]
    fn prefs_round_trip_keeps_the_form_without_touching_other_prefs() {
        let d = LogSettingsData {
            log_type: LogType::All,
            overwrite: Some(false),
            file_name: "x.log".to_string(),
            max_size_mb: 8,
            pattern: "%v".to_string(),
            ..LogSettingsData::default()
        };
        let mut p = term_runtime::prefs::Prefs {
            language: "zh-CN".to_string(),
            suppress_close_window: true,
            ..Default::default()
        };
        // 走生产用的那条转换（`set_log_options` / `log_options`），不手抄
        // 一遍 `1 = 覆盖 / 2 = 追加`——手抄的那份正是这次要拆掉的。
        p.set_log_options(&d.options());
        let back = LogSettingsData::from_prefs(Some(&p));
        assert_eq!(back.log_type, LogType::All);
        assert_eq!(back.overwrite, Some(false));
        assert_eq!(back.file_name, "x.log");
        assert_eq!(back.max_size_mb, 8);
        assert_eq!(back.pattern, "%v");
        assert_eq!(p.language, "zh-CN", "load-modify-save 不许清掉语言");
        assert!(p.suppress_close_window, "也不许清掉关闭抑制位");
    }

    #[test]
    fn the_form_is_what_the_writer_actually_receives() {
        // 「设置页 vs 写入端」之间只允许有一次转换：表单 → `LogOptions`。
        // 这条钉住它，因为上一轮正是这里断的（五个字段都落了盘，没人消费）。
        let d = LogSettingsData {
            log_type: LogType::Printable,
            overwrite: Some(true),
            file_name: "%n.log".to_string(),
            max_size_mb: 4,
            pattern: "[%H:%M] %v".to_string(),
            ..LogSettingsData::default()
        };
        let opts = d.options();
        assert_eq!(opts.log_type, LogType::Printable);
        assert_eq!(opts.overwrite, Some(true));
        assert_eq!(opts.max_size_mb, 4);
        assert_eq!(opts.max_bytes(), Some(4 * 1024 * 1024));
        // 用户写的模板要真的生效：文件名与每行格式都按它走。
        assert_eq!(opts.file_name_for("web", 1_793_867_052), "web.log");
        assert_eq!(
            opts.line("web", "hi", 1_793_867_052),
            "[08:24] hi"
        );
    }

    #[test]
    fn an_old_prefs_file_falls_back_to_reference_defaults() {
        let back = LogSettingsData::from_prefs(Some(&term_runtime::prefs::Prefs::default()));
        assert_eq!(back, LogSettingsData::default());
    }
}
