//! 第二/三批 Trigger 类型页的几何与文案（视觉基线 `19`~`31`，零 egui）。
//!
//! # 为什么单独一个文件
//!
//! `trigger_spec` 已经覆盖了骨架 + 前两批页面；新补的四个类型页（Custom menu /
//! Run command / Play sound / Show tooltip）与 `Action` 下拉的三个分支都是**独立
//! 页面**，各自的坐标表互不复用。放回 `trigger_spec` 会让那个文件同时承担
//! 「骨架」与「六个页面」两件事，也贴着 R9 的单文件上限。
//!
//! # 折算口径
//!
//! 这 13 张图都是 **2 px = 1 逻辑点**，证据是内容区边框的像素宽度：
//! `26` 的左右框在 x=17 / x=966（949 px），而骨架里 `BODY.2 = 475.8` 点；
//! 949 ÷ 2 = 474.5，与 475.8 只差边框取整。竖直方向同理：`23` 的 Match 框
//! 顶 y=110、`19` 的 Action 下拉顶 y=104，两者相差 6 px，正是裁剪起点不同。
//!
//! 本模块所有坐标都是 **`BODY` 相对**（`BODY` 左上为原点），与 `panels::body`
//! 的 `origin(o)` 对齐；不含骨头架（左栏 / 标签条 / 工具条 / 页脚）——那些仍在
//! `trigger_spec` 里。

/// `Action` 下拉某个取值对应的字段区（视觉基线 `19` / `20` / `21`）。
///
/// 三种取值各有自己的版式，**不是同一区块换标题**：
/// `Send text` / `Open hyperlink` 是一整块多行框 + 底部 `Text`/`Script` 选项卡；
/// `Play sound` 是一个分组框，里面是 `Sound Name` 下拉 + `Sound Duration` + `Test`。
pub mod action_extra {
    /// 字段区顶（`Action` 下拉底 22.5 之下留 6.5 点缝）。
    pub const FIELD_TOP: f32 = 29.0;
    /// 多行框底（`Text`/`Script` 选项卡条的上沿）。
    pub const FIELD_BOTTOM: f32 = 437.5;
    /// `Text`/`Script` 选项卡条 top / 高（视觉基线 `19` / `20`）。
    pub const MODE_TABS_TOP: f32 = 438.0;
    pub const MODE_TABS_H: f32 = 20.0;
    pub const MODES: [&str; 2] = ["Text", "Script"];

    /// `Send text` 分支的占位（视觉基线 `19` 两行）。
    pub const SEND_HINT: [&str; 2] = ["E.g.:", "echo command\\n"];
    /// `Open hyperlink` 分支的占位（视觉基线 `20`）。
    pub const HYPERLINK_HINT: [&str; 2] = ["E.g.:", "http://www.sample.com"];

    /// `Play sound` 分支的外层分组框：top / bottom（视觉基线 `21`，框几乎顶满
    /// 内容区，底边压到 `BODY` 底）。
    pub const SOUND_GROUP_TOP: f32 = 33.0;
    pub const SOUND_GROUP_BOTTOM: f32 = 457.5;
    /// `Sound Name:` 标签左缘，以及下拉（x / 宽 / top / 高）。
    ///
    /// 分组框左内边距实测 `10.25` 点（视觉基线 `21` 里标签墨迹左缘距 `BODY` 左）。
    pub const SOUND_LABEL_X: f32 = 10.25;
    pub const SOUND_COMBO_X: f32 = 123.5;
    pub const SOUND_COMBO_W: f32 = 337.0;
    pub const SOUND_COMBO_TOP: f32 = 44.0;
    pub const SOUND_COMBO_H: f32 = 22.5;
    /// `Sound Duration: 00:00:01.368` 这一行的中心 y。
    pub const DURATION_Y: f32 = 85.0;
    /// `Test` 按钮（x / top / 宽 / 高）。右缘与 `Sound Name` 下拉右缘齐平
    /// （视觉基线 `21`：两者右缘同在 `462.25` 点）。
    pub const TEST_BTN: (f32, f32, f32, f32) = (295.5, 72.5, 165.0, 25.0);
    /// 未选声音时的时长占位。
    pub const DURATION_ZERO: &str = "00:00:00.000";
}

/// `Custom menu` 页（视觉基线 `23`）：Match 框 + `Actions` 表 + 工具条 + 两行 Mark。
pub mod custom_menu {
    /// Match 框高（不含 flag bar）。
    pub const MATCH_H: f32 = 164.0;
    /// `Actions:` 标签顶（`LEFT_TOP`）。
    pub const ACTIONS_LABEL_Y: f32 = 195.0;
    /// 表头 top / 高 / 表体底。
    pub const TABLE_TOP: f32 = 213.0;
    pub const HEAD_H: f32 = 20.5;
    pub const TABLE_BOTTOM: f32 = 378.0;
    /// `Label` 与 `Action` 两列的分界（相对 `BODY` 左缘）。
    pub const COL_SPLIT: f32 = 235.0;
    /// 表下工具条高（`+ - ▲▼` 与 `Modify` / `Remove All`）。
    pub const TOOLS_H: f32 = 20.0;
    /// `Mark Style` / `Mark Color` 两行的下拉 top 与左缘、高。
    pub const MARK_STYLE_TOP: f32 = 405.0;
    pub const MARK_COLOR_TOP: f32 = 434.0;
    pub const MARK_COMBO_LEFT: f32 = 86.0;
    pub const MARK_COMBO_H: f32 = 22.5;
    pub const HEADERS: [&str; 2] = ["Label", "Action"];
    pub const EMPTY: &str = "No action available";
    /// 左条工具条按钮与右端两个命令（顺序即绘制顺序）。
    pub const TOOLS_LEFT: [&str; 4] = ["+", "-", "\u{25b2}", "\u{25bc}"];
    pub const TOOLS_RIGHT: [&str; 2] = ["Modify", "Remove All"];
    /// `+` 的下拉（视觉基线 `25`）。
    pub const ADD_ITEMS: [&str; 2] = ["Add Action", "Add Separator"];
    /// 页脚主按钮文案：这一页是「下一步」，不是 `Ok`。
    pub const FOOTER_PRIMARY: &str = "Next";
}

/// `Run command` 页（视觉基线 `26` / `27`）：Match 框 + `Action:` 分 OS 三字段。
pub mod run_command {
    /// Match 框高（不含 flag bar）。
    pub const MATCH_H: f32 = 205.0;
    /// `Action:` 标签顶。
    pub const ACTION_LABEL_Y: f32 = 240.0;
    /// OS 选项卡条 top / 高。
    ///
    /// 视觉基线 `26` / `27` 里选项卡与第一行输入框之间还留着 15 点空档
    /// （选项卡 260.5..280.5、`Command:` 框 295.5 起）。早期把 top 写成 276，
    /// 选项卡底边正好压在 `Command:` 框顶边上。
    pub const TABS_TOP: f32 = 260.5;
    pub const TABS_H: f32 = 20.0;
    /// 三行输入框的 top（行距 30）与高。
    pub const ROWS: [f32; 3] = [295.0, 325.0, 355.0];
    pub const FIELD_H: f32 = 23.5;
    pub const FIELD_LEFT: f32 = 145.5;
    pub const FIELD_W: f32 = 319.5;
    /// Windows 页独有的 `Hide console window:` 一行（视觉基线 `27`）。
    pub const HIDE_LABEL_Y: f32 = 388.0;
    pub const HIDE_TOGGLE_X: f32 = 173.0;
    pub const HIDE_TOGGLE_W: f32 = 30.0;
    pub const HIDE_TOGGLE_H: f32 = 15.0;
    pub const HIDE_LABEL: &str = "Hide console window:";
    /// 字段右侧的两个内嵌按钮（`...` 与 `▶`）距框右缘的内缩。
    pub const BTN_INSET: f32 = 16.0;
    pub const FOOTER_PRIMARY: &str = "Next";
}

/// `Play sound` 页（视觉基线 `28`）：`Rules` 三列表（第三列是 `Sound`）。
pub mod play_sound {
    /// 表头三列（与 Highlight 的 Rules 表同列宽，第三列改叫 `Sound`）。
    pub const HEADERS: [&str; 3] = ["Pattern", "Description", "Sound"];
    pub const EMPTY: &str = "No sound available";
    pub const FOOTER_PRIMARY: &str = "Next";

    /// 修改弹窗（视觉基线 `29`，宽 381 × 高 433 逻辑点）。
    ///
    /// 版式与 Highlight 设置弹窗（`trigger_spec::highlight`）同构：两个分组框，
    /// 上组 `Match`（Pattern + flag bar + Description），下组 `Sound`
    /// （Sound Name + Sound Duration + Test）。
    pub mod dialog {
        pub const W: f32 = 381.0;
        pub const H: f32 = 433.0;
        /// 两个分组框共用的左边与宽度（相对弹窗）。
        pub const GROUP_X: f32 = 5.0;
        pub const GROUP_W: f32 = 372.0;
        /// `Match` / `Sound` 两个标题（`LEFT_TOP`）。
        pub const MATCH_TITLE: (f32, f32) = (5.0, 11.5);
        pub const SOUND_TITLE: (f32, f32) = (5.0, 301.0);
        /// `Match` 组框 top / 高、`Sound` 组框 top / 高。
        pub const G1: (f32, f32) = (25.0, 266.0);
        pub const G2: (f32, f32) = (319.0, 75.0);
        /// 左标签列 x。
        pub const LABEL_X: f32 = 12.0;
        /// `Pattern:` 标签中心 y 与 Pattern 框（x / top / 宽 / 高）。
        pub const PATTERN_LABEL_Y: f32 = 47.0;
        pub const PATTERN: (f32, f32, f32, f32) = (106.0, 35.0, 259.0, 194.0);
        /// flag bar 高（贴在 Pattern 框内下沿）。
        pub const FLAG_H: f32 = 20.5;
        /// `Description:` 标签中心 y 与输入框。
        pub const DESC_LABEL_Y: f32 = 269.0;
        pub const DESC: (f32, f32, f32, f32) = (106.0, 258.0, 259.0, 22.0);
        /// `Sound Name:` 标签左缘 / 中心 y，下拉（x / top / 宽 / 高）。
        pub const SOUND_LABEL: (f32, f32) = (14.0, 340.0);
        pub const SOUND_COMBO: (f32, f32, f32, f32) = (128.0, 330.0, 237.0, 21.5);
        /// `Sound Duration:` 数值的左边与中心 y。
        ///
        /// 左边与 `Sound Name:` **标签列**对齐（视觉基线 `29` 里两行左缘同为
        /// 弹窗内 14 点），不是与下拉框对齐。
        pub const DURATION_X: f32 = 14.0;
        pub const DURATION_Y: f32 = 370.0;
        /// `Test` 按钮（x / top / 宽 / 高）。
        pub const TEST_BTN: (f32, f32, f32, f32) = (250.0, 359.0, 114.0, 23.0);
        /// 页脚按钮（宽 / 高 / y / 两个 x）。
        pub const BTN_W: f32 = 79.0;
        pub const BTN_H: f32 = 23.0;
        pub const BTN_Y: f32 = 400.0;
        pub const BTN_CANCEL_X: f32 = 211.0;
        pub const BTN_OK_X: f32 = 297.0;
        pub const LABELS: [&str; 2] = ["Pattern:", "Description:"];
    }
}

/// `Show tooltip` 页（视觉基线 `30` / `31`）：Match + `Tooltip` 框 + Mark 两行。
pub mod tooltip {
    /// Match 框高（不含 flag bar）。
    pub const MATCH_H: f32 = 164.5;
    /// `Tooltip:` 标签顶。
    pub const LABEL_Y: f32 = 197.0;
    /// Tooltip 多行框（x / top / 宽 / 高）。
    pub const BOX: (f32, f32, f32, f32) = (0.0, 213.0, 474.5, 165.5);
    /// `Text`/`Script` 选项卡条 top / 高；右端 `Captures` 宽。
    pub const MODE_TABS_TOP: f32 = 378.5;
    pub const MODE_TABS_H: f32 = 20.0;
    pub const CAPTURES_W: f32 = 74.0;
    pub const MODES: [&str; 2] = ["Text", "Script"];
    /// `Mark Style` / `Mark Color` 两行（与 Custom menu 同高同位）。
    pub const MARK_STYLE_TOP: f32 = 405.0;
    pub const MARK_COLOR_TOP: f32 = 434.0;
    pub const MARK_COMBO_LEFT: f32 = 86.0;
    pub const MARK_COMBO_H: f32 = 22.5;
    /// Text 模式的占位（视觉基线 `30`）。
    pub const HINT: [&str; 2] = ["E.g.:", "<font color=\"red\">${match}</font>"];
    /// Script 模式的占位（视觉基线 `31`）。
    pub const SCRIPT_HINT: &str = "E.g.:\n(captures) => {\n  return `<font color=\"red\">${captures.match}</font>`;\n}";
    pub const FOOTER_PRIMARY: &str = "Next";
}

/// `Sound Name` 下拉里的音频名（视觉基线 `22`，47 项）。
///
/// **只有文件名，不附带音频文件**：本仓不发布参考实现资源，`Test` 的试听能力见
/// （含视觉基线里的拼写 `Delection.mp3`）。
pub const SOUND_NAMES: [&str; 47] = [
    "Confirmation.mp3",
    "Delection.mp3",
    "Dige's Cat.mp3",
    "Dog.mp3",
    "Doorbell Ring 1.mp3",
    "Doorbell Ring 2.mp3",
    "Failure 1.mp3",
    "Failure 2.mp3",
    "Failure 3.mp3",
    "Failure 4.mp3",
    "Failure 5.mp3",
    "Failure 6.mp3",
    "Failure 7.mp3",
    "Failure 8.mp3",
    "Finish.mp3",
    "Notification 1.mp3",
    "Notification 2.mp3",
    "Notification 3.mp3",
    "Notification 4.mp3",
    "Notification 5.mp3",
    "Notification 6.mp3",
    "Notification 7.mp3",
    "Notification 8.mp3",
    "Pause.mp3",
    "Rejection 1.mp3",
    "Rejection 2.mp3",
    "Restart 1.mp3",
    "Restart 2.mp3",
    "Resume.mp3",
    "Ring 1.mp3",
    "Ring 2.mp3",
    "Ring 3.mp3",
    "Ring 4.mp3",
    "Ring 5.mp3",
    "Ring 6.mp3",
    "Ring 7.mp3",
    "Ring 8.mp3",
    "Ring 9.mp3",
    "Success 1.mp3",
    "Success 2.mp3",
    "Success 3.mp3",
    "Success 4.mp3",
    "Success 5.mp3",
    "Telephone Ring 1.mp3",
    "Telephone Ring 2.mp3",
    "Timeout.mp3",
    "Waiting.mp3",
];

/// 秒数 → `00:00:01.368` 形式的时长文本。
///
/// 视觉基线 `21` / `29` 是**三段**加毫秒的定宽写法（`时:分:秒.毫秒`），不是
/// `分:秒`——`1.368` 秒在视觉基线里显示成四个字段而不是三个。负数与 NaN 都按 0
/// 处理：配置里出现负数只可能是手改 JSON，不该让界面显示 `-00:00:01.-1`。
pub fn duration_text(secs: f32) -> String {
    let s = if secs.is_finite() && secs > 0.0 { secs } else { 0.0 };
    let total_ms = (s * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        total_s / 3600,
        (total_s / 60) % 60,
        total_s % 60,
        ms
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 常量之间的比较走一层函数。
    ///
    /// `clippy::assertions_on_constants` 会把 `assert!(A > B)`（两侧都是 `const`）
    /// 判成「条件恒定」；可这些断言要证明的**正是**常量之间不重叠——把常量当
    /// 运行时值传进来，既保留断言强度，又不触发那条 lint。
    fn check(cond: bool, what: &str) {
        assert!(cond, "{what}");
    }

    #[test]
    fn duration_text_matches_reference_format() {
        assert_eq!(duration_text(1.368), "00:00:01.368");
        assert_eq!(duration_text(0.0), "00:00:00.000");
        assert_eq!(duration_text(-3.0), "00:00:00.000");
        assert_eq!(duration_text(f32::NAN), "00:00:00.000");
        assert_eq!(duration_text(61.5), "00:01:01.500");
        assert_eq!(duration_text(3725.0), "01:02:05.000");
    }

    /// 声音库必须按视觉基线 `22` 的字典序排列：`Delection` 在 `Dige's Cat` 之前，
    /// `Ring 10` 这类「数字不进位」的项在这份列表里不存在（最多到 9）。
    #[test]
    fn sound_names_keep_the_reference_order() {
        assert_eq!(SOUND_NAMES.len(), 47);
        assert_eq!(SOUND_NAMES[0], "Confirmation.mp3");
        assert_eq!(SOUND_NAMES[2], "Dige's Cat.mp3");
        assert_eq!(SOUND_NAMES[46], "Waiting.mp3");
        let mut sorted = SOUND_NAMES;
        sorted.sort_unstable();
        assert_eq!(sorted, SOUND_NAMES, "视觉基线的列表本身就是字典序");
    }

    /// 四个页面在竖直方向上不能互相压盖：上一块的下沿必须落在下一块上沿之前。
    #[test]
    fn page_blocks_do_not_overlap() {
        use action_extra as a;
        check(a::FIELD_TOP > 22.5, "字段区要在 Action 下拉下方");
        check(a::MODE_TABS_TOP >= a::FIELD_BOTTOM, "a::MODE_TABS_TOP >= a::FIELD_BOTTOM");
        check(a::SOUND_COMBO_TOP > a::FIELD_TOP, "a::SOUND_COMBO_TOP > a::FIELD_TOP");
        check(a::SOUND_GROUP_BOTTOM > a::TEST_BTN.1 + a::TEST_BTN.3, "a::SOUND_GROUP_BOTTOM > a::TEST_BTN.1 + a::TEST_BTN.3");

        use custom_menu as cm;
        check(cm::MATCH_H < cm::ACTIONS_LABEL_Y, "cm::MATCH_H < cm::ACTIONS_LABEL_Y");
        check(cm::ACTIONS_LABEL_Y < cm::TABLE_TOP, "cm::ACTIONS_LABEL_Y < cm::TABLE_TOP");
        check(cm::TABLE_BOTTOM > cm::TABLE_TOP + cm::HEAD_H, "cm::TABLE_BOTTOM > cm::TABLE_TOP + cm::HEAD_H");
        check(cm::TABLE_BOTTOM + cm::TOOLS_H <= cm::MARK_STYLE_TOP, "cm::TABLE_BOTTOM + cm::TOOLS_H <= cm::MARK_STYLE_TOP");
        check(cm::MARK_COLOR_TOP > cm::MARK_STYLE_TOP + cm::MARK_COMBO_H, "cm::MARK_COLOR_TOP > cm::MARK_STYLE_TOP + cm::MARK_COMBO_H");

        use run_command as rc;
        check(rc::MATCH_H < rc::ACTION_LABEL_Y, "rc::MATCH_H < rc::ACTION_LABEL_Y");
        check(rc::ROWS[0] >= rc::TABS_TOP + rc::TABS_H - 1.0, "rc::ROWS[0] >= rc::TABS_TOP + rc::TABS_H - 1.0");
        check(rc::HIDE_LABEL_Y > rc::ROWS[2] + rc::FIELD_H, "rc::HIDE_LABEL_Y > rc::ROWS[2] + rc::FIELD_H");

        use tooltip as tt;
        check(tt::LABEL_Y > tt::MATCH_H, "tt::LABEL_Y > tt::MATCH_H");
        check(tt::BOX.1 >= tt::LABEL_Y, "tt::BOX.1 >= tt::LABEL_Y");
        check(tt::BOX.1 + tt::BOX.3 <= tt::MODE_TABS_TOP + 1.0, "tt::BOX.1 + tt::BOX.3 <= tt::MODE_TABS_TOP + 1.0");
        check(tt::MARK_STYLE_TOP > tt::MODE_TABS_TOP + tt::MODE_TABS_H, "tt::MARK_STYLE_TOP > tt::MODE_TABS_TOP + tt::MODE_TABS_H");
    }

    /// 修改弹窗内部：两个组框都在弹窗里，Pattern / Description / 声音控件落在
    /// 各自的组框内，页脚按钮不越界。
    #[test]
    fn play_sound_dialog_fits_its_groups() {
        use play_sound::dialog as d;
        check(d::G1.0 + d::G1.1 < d::G2.0, "Match 组要在 Sound 组之上");
        check(d::G2.0 + d::G2.1 < d::BTN_Y, "Sound 组要在页脚之上");
        check(d::BTN_Y + d::BTN_H <= d::H, "页脚按钮不越出弹窗底");
        check(d::GROUP_X + d::GROUP_W <= d::W, "组框不越出弹窗右缘");
        for (x, top, w, h) in [d::PATTERN, d::DESC] {
            assert!(x > d::GROUP_X && x + w <= d::GROUP_X + d::GROUP_W);
            assert!(top > d::G1.0 && top + h <= d::G1.0 + d::G1.1);
        }
        let (sx, st, sw, sh) = d::SOUND_COMBO;
        check(sx > d::GROUP_X && sx + sw <= d::GROUP_X + d::GROUP_W, "声音下拉在 Sound 组内");
        check(st > d::G2.0 && st + sh <= d::G2.0 + d::G2.1, "声音下拉不越出 Sound 组");
        check(d::BTN_CANCEL_X + d::BTN_W < d::BTN_OK_X, "Cancel 在 OK 左边");
        check(d::BTN_OK_X + d::BTN_W <= d::W, "OK 不越出弹窗右缘");
    }
}
