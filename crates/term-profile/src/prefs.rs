//! 界面偏好的持久化：语言、两个关闭抑制位、Logging 配置页五项、光标样式、
//! 在线搜索模板、Filer 的隐藏项开关、Text 页三行与已移除 Behaviour 页的三个开关、
//! 关多标签的确认开关、两个粘贴确认开关、终端字号，以及 Tabs 页的三项标签行为。
//!
//! # 为什么单独一个文件，不塞进 `layout.json`
//!
//! `layout_state` 存的是**窗口结构**（标签 / 分屏 / 停靠位），它的语义是
//! 「上次这个窗口长什么样」；语言是**应用级偏好**，与窗口结构无关，也不该
//! 因为用户「重置布局」被一起丢掉。两者一个目录、两份文件，各自原子写。
//!
//! 语言偏好键名沿用 `application.language`（值形如 `zh-CN`），
//! 这里沿用同样的标签写法（见 `term_app::i18n::Lang::tag`），但文件格式是
//! JSON——本仓库的其余配置文件都是 JSON。
//!
//! # 读旧文件
//!
//! 字段全部 `#[serde(default)]`：老文件里少一个键（或者整份是手改坏的），
//! 最坏结果是「回到默认语言」，不是「启动失败」。默认语言是英文，见
//! `Lang::default` 的注释（基线契约）。
//!
//! # 读不出来 ≠ 可以覆盖它
//!
//! `prefs.json` 是用户可能在编辑器里手改的文件，而全仓十几处调用方都是
//! `load().unwrap_or_default()` → 改 → `save()`。这两件事凑在一起就是一条
//! **不可逆丢数据**的路：手改时少写一个逗号 → `load()` 返回 `None` → 调用方
//! 退回出厂值 → `save()` 把用户原来那份顶掉。文件在仓库之外，`git` 救不了，
//! 也没有第二份副本。
//!
//! 所以 [`save`] 不是「把 JSON 写出去」那么简单：覆盖之前先看旧文件读不读得
//! 出来，读不出来就把它原样另存一份 `prefs.json.bad-<时间戳>`（见
//! [`preserve_broken`]），再写新文件；连备份都做不到时**这一次就不写**。
//! 保全只发生在写路径上——读仍然是零副作用的一件事。

use crate::persist::{fingerprint_bytes, write_atomic_if_unchanged, AtomicWriteError};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use term_core::command_history::CommandHistory;
use term_core::onekey::OneKey;
use term_core::trigger::Rule;
use term_core::tunnel::TunnelProfile;

use crate::quickbar::QuickBarDisplay;

pub const PREFS_FILE: &str = "prefs.json";

/// 地址栏「最近使用」保留几条。
///
/// 20 条是**下拉高度与可用性**之间的折中：地址栏下拉一屏约十行，20 条刚好
/// 「翻两下能看到全部」；再多就变成一份需要搜索的清单，而它本来只是「顺手再用
/// 一次上一台机器」的快捷方式。也是写盘体积的上界（一条几十字节）。
pub const RECENT_TARGETS_CAP: usize = 20;

/// 终端字号的可选区间（点）。见 [`Prefs::font_size_value`]：必须与渲染层的
/// `MIN_FONT_SIZE` / `MAX_FONT_SIZE` 一致，否则设置页显示的值会被悄悄改掉。
pub const FONT_SIZE_MIN: f32 = 6.0;
pub const FONT_SIZE_MAX: f32 = 48.0;

/// `Tabs ▸ Activate the tab after closing:` 的档数（下一个 / 上一个 / 上次激活）。
pub const TAB_ACTIVATE_MODES: usize = 3;
/// `Tabs ▸ Double click:` 的档数（没有动作 / 关闭标签）。`Rename` 没有入口，不放。
pub const TAB_DOUBLE_CLICK_ACTIONS: usize = 2;
/// `Tabs ▸ Tab text elide mode:` 的档数（不省略 / 左 / 右 / 中）。
pub const TAB_ELIDE_MODES: usize = 4;
/// `Tabs ▸ Show the tab number on tabs:` 的档数（不显示 / 前九个 / 全部）。
pub const TAB_NUMBER_MODES: usize = 3;
/// `Tabs ▸ Mouse Actions ▸ Middle click:` 的档数（没有动作 / 关闭标签）。
pub const TAB_MIDDLE_CLICK_ACTIONS: usize = 2;
/// `Tabs ▸ Mouse Actions ▸ Right click:` 的档数
/// （标签菜单 / 关闭标签 / 没有动作）。
pub const TAB_RIGHT_CLICK_ACTIONS: usize = 3;
/// `Terminal ▸ Mouse Actions ▸ Right click:` 的档数
/// （显示菜单 / 粘贴 / 有选区就复制、否则粘贴）。
///
/// 三档的语义与判据在 `term_app::mouse::RightClickAction`：本 crate 只存下标，
/// 与 `TAB_DOUBLE_CLICK_ACTIONS` 同一套写法（文案在投影层，prefs 不装 UI 词）。
pub const RIGHT_CLICK_ACTIONS: usize = 3;

/// 会话显示方式：文本网格，还是十六进制转储。
///
/// 这是**当前会话的状态**，落在投影层的 `Tab.mode` 里，**不落盘**：用户要的是
/// 「View 可以切当前会话的显示方式，但是不会保存」。所以本层只留枚举本身
/// （`Tab` 要用它），不再有「会话名 → 模式」的映射表，也没有 tag 序列化
/// ——没有落盘就没有「认不出来的旧值」这回事。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DisplayMode {
    /// 字符网格（出厂态，也是每个新标签的样子）。
    Text,
    /// 十六进制转储。
    Bin,
}

/// 「最近跑过的脚本」最多留几条（ADR-0027 说「最近 10 条」）。
///
/// 超过之后丢最旧的一条。定死一个数而不是做成可配：这是一份**备忘**，不是
/// 历史记录，列表长了反而找不到。
pub const RECENT_SCRIPTS_MAX: usize = 10;

/// 落盘的偏好。字段只增不减，全部带 `default`。
///
/// 有 `f32` 字段（`font_size`），所以只派生 `PartialEq`：`Eq` 要求全序，
/// 而 `0.1 + 0.2 != 0.3` 那种事会让它在浮点上撒谎。
/// 脚本注册表里的一条：显示名 + 脚本文件路径 + 调用参数。
///
/// 只存元数据，不存脚本源码——源码始终在 `profile/scripts/*.rhai`，由编辑器
/// 负责读写；这里按**名字**唯一（与 OneKey 同款），因为脚本面板和触发器都按
/// 名字引用它。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptEntry {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// 一份 [`Prefs`] 快照「从哪份盘上字节读来」的基线指纹。
///
/// 这是**元数据，不是用户偏好**：不落盘（`#[serde(skip)]`），也不参与
/// [`Prefs`] 的相等比较——读回的快照和手写的期望值在调用方眼里必须相等。
/// 指纹绑在**实例**上而不是线程上：同线程里另一次无关的 `load()` 绝不能把
/// 一份已经过期的快照「洗白」成最新（那正是旧实现「明明冲突却通过」的漏洞）。
/// `Cell` 让 `save(&Prefs)` 在成功后把基线上移到刚写出的字节，而不必把全仓
/// 十几处写口全改成 `&mut`。
#[derive(Clone, Debug, Default)]
pub struct ExpectedFingerprint(Cell<Option<u64>>);

impl ExpectedFingerprint {
    fn get(&self) -> Option<u64> {
        self.0.get()
    }

    fn set(&self, fingerprint: Option<u64>) {
        self.0.set(fingerprint);
    }
}

impl PartialEq for ExpectedFingerprint {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    /// 语言标签：`en` / `zh-CN`。空串 = 没设置过（用默认语言）。
    #[serde(default)]
    pub language: String,
    /// 关闭**窗口**不再弹确认（确认弹窗里的「不再提示」勾选落到这里）。
    ///
    /// 与 `suppress_close_session` 分开：关窗口与关会话是两个问题，
    /// 一个勾选不该把另一个也静音。
    #[serde(default)]
    pub suppress_close_window: bool,
    /// 关闭**会话**（单条或全部）不再弹确认。
    #[serde(default)]
    pub suppress_close_session: bool,
    /// `Session ▸ Preferences ▸ Default Session Settings` 保存的默认模板。
    /// `None` = 用户没保存过，界面回落到出厂 `SessionConfig::new(Protocol::Ssh)`。
    /// 用 `Option` 而不是整份 `SessionConfig`，老文件缺键时 serde 缺省即「没设过」，
    /// 不会凭空造出一条用户的默认会话。
    #[serde(default)]
    pub default_session: Option<term_core::session::SessionConfig>,
    /// Session Settings ▸ Logging 配置页（2026-11-08）。空串 / 0 表示「老文件
    /// 没这项」，由 `apps/desktop` 的 `LogSettingsData::from_prefs` 回落到视觉基线
    /// 默认值；写回时一定是具体值。分开存而不是塞一个 JSON 字符串，是为了
    /// 让 `load-modify-save` 与其余字段一样可逐项核对。
    #[serde(default)]
    pub log_type: u8,
    /// 0 = 未选（视觉基线出厂态）/ 1 = 覆盖 / 2 = 追加。
    #[serde(default)]
    pub log_overwrite: u8,
    #[serde(default)]
    pub log_file_name: String,
    /// 0 = ∞。
    #[serde(default)]
    pub log_max_size_mb: u32,
    #[serde(default)]
    pub log_content_pattern: String,
    /// Application ▸ Startup 的三档策略。规范值 `none` / `last` /
    /// `specific`；**空串 = 老文件没有这项**，由 `term_app::startup::parse_tag`
    /// 回落到 `last`（视觉基线出厂态）。用字符串而非序号，是为了让 serde 的
    /// 缺省值（空串）天然等于「没设置过」，不需要 migration。
    #[serde(default)]
    pub startup_mode: String,
    /// 「A specific session or set of sessions」的 Session 1–4。空串 = 没指定。
    /// 与语言、日志设置同属一份 `prefs.json`，写回必须 load-modify-save。
    #[serde(default, deserialize_with = "de_sessions")]
    pub startup_sessions: [String; 4],
    /// 命令行历史 TTL（天）。`0` = 老文件没这项 / 用户未设，由
    /// `command_history_store` 回落出厂默认 30 天（与灰显设置页一致）。
    /// 不为 `0` 时直接采用该值；`0` 是「未设置」哨兵，不是「永久保留」
    /// （永久保留请设大数，如 36500）。
    #[serde(default)]
    pub command_history_days: u64,
    /// 命令行历史容量（条）。`0` = 回落出厂默认 10000（与灰显设置页一致）。
    #[serde(default)]
    pub command_history_size: usize,
    /// 是否把命令行历史落盘。灰显设置页出厂态为**开**，故反序列化默认 `true`
    /// （老文件缺键 = 沿用出厂态）；UI 接线后由设置页改写。落盘由 `term-runtime::persist` 负责。
    #[serde(default = "default_true")]
    pub command_history_save: bool,
    /// `Text ▸ Caret` 页选的光标样式，取值是
    /// `term_core::config::CARET_STYLES` 的下标（0 = Block … 7 = Blinking Underline）。
    ///
    /// **这是新会话的初值，不是所有会话的节目单**：每条会话带着自己的
    /// `SessionConfig.caret_style`，改这里不会回头改已经存在的会话（那样等于
    /// 悄悄改用户的会话属性）。`0` 同时是「老文件没这项」与出厂 Block，
    /// 两者要的效果一样，不需要 migration。
    #[serde(default)]
    pub caret_style: usize,
    /// `Text ▸ Caret ▸ Period`：光标闪烁周期（秒，「亮多久 = 灭多久」）。
    /// 与 `caret_style` 同属**新会话初值**，落进 `SessionConfig.caret_period`。
    /// 2026-11-19 起可调，此前是渲染层常数（1 Hz 方波）。
    #[serde(default = "default_caret_period")]
    pub caret_period: f32,
    /// `Text ▸ Caret ▸ Width`：`Underline` / `Beam` 的宽度（像素）。
    /// 落进 `SessionConfig.caret_width`；`Block` / `HollowBlock` 不用它。
    #[serde(default = "default_caret_width")]
    pub caret_width: f32,
    /// `Search` 页的查询 URL 模板：`{}` 是查询词占位符。
    ///
    /// 空串 = 没设置过 → 用 `search::default_url()`（DuckDuckGo）。用空串而不是
    /// 一开始就写满出厂模板，是为了让老文件（缺这个键）与「用户主动选了出厂
    /// 那条」在磁盘上仍然可区分：前者不需要我们替他做决定，后者是他选过的。
    #[serde(default)]
    pub search_url: String,
    /// `Filer ▸ Show hidden items`：列本地目录时是否列出 `.` 开头的条目。
    ///
    /// 出厂 `false`（与视觉基线设置页的空勾一致）。只有**本地**目录枚举读它；
    /// SFTP 列表由服务端给，远端怎么算隐藏项我们不知道，也不该假装知道。
    #[serde(default)]
    pub filer_show_hidden: bool,
    /// `Text ▸ Alternate line background color`：给隔行叠一层极淡的底色。
    ///
    /// 出厂 `false` = 保持现状（视觉基线设置页也是空勾）。消费者是
    /// `ui/terminal/view.rs::paint_rows`——它在画每行底色时按**绝对行号**的奇偶
    /// 决定要不要叠这一层，滚动时不会因为视口位置变化而闪。
    #[serde(default)]
    pub text_alternate_row_bg: bool,
    /// `Text ▸ Hide cursor when typing`：最近一次键盘输入后的 500ms 内不画光标。
    ///
    /// 出厂 `false` = 光标一直按 `snapshot.cursor_visible` 画（现状）。消费者是
    /// `ui/terminal/pane.rs` 画光标的那一段，判据来自 `Tab` 上记的输入时刻。
    #[serde(default)]
    pub text_hide_cursor_while_typing: bool,
    /// `prefs.focus_new_tab`（原 Behaviour 页已移除）：新开标签（分屏 / 从会话树连接 / 复制标签）
    /// 之后要不要把焦点切过去。
    ///
    /// 出厂 **`true`**：现在的行为就是「新标签立刻成为活动标签」，改默认值会让
    /// 整窗基线一起漂（分屏那张图的活动窗格会换）。关掉之后新标签照常建出来，
    /// 只是不抢焦点。
    #[serde(default = "default_true")]
    pub focus_new_tab: bool,
    /// `Text ▸ Font ▸ Font size:` 的终端字号（点）。
    ///
    /// **`0.0` = 老文件没这项**，不是「零号字」：生效值由
    /// [`Prefs::font_size_value`] 回落出厂 `term_core::config::DEFAULT_FONT_SIZE`，
    /// 与 `caret_style` / `command_history_*` 是同一套 0 哨兵写法。
    /// 落盘只写拖过之后的具体值，所以「没设置」与「设成出厂 13」在盘上仍可区分。
    #[serde(default)]
    pub font_size: f32,
    /// `Text ▸ Font ▸ Font family 1–4` 的四行字族（空串 = 自动）。
    ///
    /// 与 `font_size` 同一层：都是**全局显示项**，不是会话属性，改完立刻重装
    /// 字形、所有标签一起变。四行是固定长度而不是 `Vec`：视觉基线就是四行，
    /// 长度变化只会出现在「有人改了这个数组的字面量」这种事上，而那种改动
    /// 应该编译期就撞到 `Prefs` 字段类型上，而不是运行时读到第五个元素。
    ///
    /// 空串有确定含义：**自动**。槽 0 自动挑内置等宽候选，槽 1 自动挑内置
    /// CJK 候选，槽 2–3 不用。所以「没设过」与「设成空」是同一件事——
    /// 这与 `log_file_name` 那类「空串 = 没填」是一致的。
    #[serde(default)]
    pub font_family: [String; 4],
    /// `Text ▸ Font ▸ Font weight:` 的目标字重（OS/2 `usWeightClass`）。
    ///
    /// `0` = 老文件没这项，生效值由 [`Prefs::font_weight_value`] 回落 400。
    /// 存数值而不是枚举：字体的档位不止 Regular/Bold 两档（Medium 500、
    /// SemiBold 600 很常见），写死枚举会在遇到新字体时被迫改文件格式。
    #[serde(default)]
    pub font_weight: u16,
    /// `Tabs ▸ Activate the tab after closing:`：0 = 下一个（现状）/ 1 = 上一个 /
    /// 2 = 上一次激活的那条。`0` 同时是「老文件没这项」，两者要的效果一样。
    #[serde(default)]
    pub tab_activate_after_close: usize,
    /// `Tabs ▸ Double click:`：0 = 没有动作（现状）/ 1 = 关闭标签。
    ///
    /// 参考页还有 `Rename tab` 一档，本仓库没有标签重命名入口（改名走
    /// `Properties` 表单，改的是**会话**名字），所以下拉里只放两档。
    #[serde(default)]
    pub tab_double_click: usize,
    /// `Tabs ▸ Tab text elide mode:`：0 = 不省略（现状，标题想多长就多长）/
    /// 1 = 省略左侧 / 2 = 省略右侧 / 3 = 省略中间。
    #[serde(default)]
    pub tab_elide: usize,
    /// `Tabs ▸ Show the tab number on tabs:`：0 = 不显示（现状）/ 1 = 前九个 /
    /// 2 = 全部。编号画在标题前，宽度由同一段文本量出来——不是另算一列。
    #[serde(default)]
    pub tab_numbers: usize,
    /// `Tabs ▸ Mouse Actions ▸ Middle click:`：0 = 没有动作（现状）/ 1 = 关闭标签。
    #[serde(default)]
    pub tab_middle_click: usize,
    /// `Tabs ▸ Mouse Actions ▸ Right click:`：0 = 标签菜单（现状）/ 1 = 关闭标签 /
    /// 2 = 没有动作。
    #[serde(default)]
    pub tab_right_click: usize,
    /// hex 视图的**出厂参数**（设置 ▸ Binary 页那四个数）。
    ///
    /// 分界（用户 2026-11-11 定死）：这一层是「默认参数」，**只有设置页写**、
    /// 只对新会话生效；「当前会话的显示方式」在投影层的 `Tab` 上
    /// （`Tab.hex` / `Tab.mode`），`View ▸ Hex *` 改它、**不落盘**。
    /// 所以 `session_modes` 那张按会话名索引的表已经删掉——没有「记住上次模式」
    /// 这回事，表就是第二个写者。
    ///
    /// **`0` = 老文件没这一项**：生效值由 `apps/desktop` 的
    /// `HexParams::from_prefs` 回落出厂值（16 进制 / 16 列 / 每字节一组 /
    /// 每 4 字节加宽），与 `caret_style` / `command_history_*` 是同一套 0 哨兵写法。
    /// 四个字段分开存而不是塞一个字符串，是为了让 `load-modify-save` 与其余
    /// 字段一样可逐项核对；也为了让盘上写的越界值能被 `from_prefs` 一处夹掉，
    /// 而不是渲染器里各写一套兜底。
    #[serde(default)]
    pub hex_base: u32,
    #[serde(default)]
    pub hex_columns: usize,
    #[serde(default)]
    pub hex_group: usize,
    #[serde(default)]
    pub hex_division: usize,
    /// hex 视图里相邻的「加宽组」交替底色（视觉基线这一格出厂是勾上的）。
    ///
    /// 也是**默认参数**：勾选状态从 prefs 读，具体分几组由当前会话的
    /// `hex.division` 决定——所以换 division 时不需要再写一次这个开关。
    #[serde(default = "default_true")]
    pub alternate_column_bg: bool,
    /// `View ▸ Toggle Line Endings`：每个**逻辑行**行尾画一个 `$`。
    ///
    /// 出厂 `false` = 现状（屏幕上不画）。这一格是「默认参数」：只写盘、只影响
    /// 之后新开的标签；`View ▸ Toggle Line Endings` 改的是当前标签的
    /// `Tab.marks.line_endings`，**不落盘**（与 `hex_*` 同一套两层模型）。
    #[serde(default)]
    pub view_line_endings: bool,
    /// `View ▸ Toggle Column Marker` 的列号（1 起）。**`0` = 不画**。
    ///
    /// `0` 同时是「老文件没这一项」的哨兵（与本文件其余字段同一套写法）——两者
    /// 要的效果一样（不画线），所以不需要 `default_*` 兜底。
    ///
    /// 非零值时由 `apps/desktop` 的 `MarksParams::from_prefs` 夹进 `8..=512`：手改
    /// 过的文件写个 `3` 或 `9999` 都不该让渲染器去画一条没有意义的线。
    #[serde(default)]
    pub view_column_marker: usize,
    /// `View ▸ Toggle Indent Guides`：按行首连续空白每 4 列画一条竖参考线。
    ///
    /// 出厂 `false`。开关是 bool（没有区间可夹），落盘只记「用户开没开」。
    #[serde(default)]
    pub view_indent_guides: bool,
    /// `View ▸ Toggle Wrap Symbols`：自动换行的续行行尾画折行标记。
    ///
    /// 出厂 `false`。与 `mar` 那一列不同：这里画在正文的最右一列，不占 margin。
    #[serde(default)]
    pub view_wrap_symbols: bool,
    /// `Wrap ▸ Wrap mode`：0 = 按字符折行（出厂/接线前行为）、1 = 按词折行
    /// （折点回退到空白边界，不劈词）。越界值由 `WrapMode::from_u8` 回落 0。
    ///
    /// 这是**全局默认**：设置页改完立刻写回窗口副本供每帧段表读取，开/关软折
    /// 仍是每标签的 `Tab.word_wrap`（Mode ▸ Word Wrap）。
    #[serde(default)]
    pub wrap_mode: u8,
    /// `Wrap ▸ Wrap column`：0 = Window edge（出厂口径；开软折时回落默认 40），
    /// 非 0 = 用户指定的折行列宽（设置页夹进 8..=512，消费端再被内核列宽夹住）。
    ///
    /// 与 `wrap_mode` 同一层：设置页写盘 + 同步窗口副本，六个终端消费者读同一份。
    #[serde(default)]
    pub wrap_column: usize,
    /// `View ▸ White Spaces` 的五档**互斥**模式，按枚举文件序编号：
    ///
    /// | 值 | 模式 | 画什么 |
    /// |----|------|--------|
    /// | 0 | Hide All（出厂） | 不画 |
    /// | 1 | Show All | 整行所有空白格 |
    /// | 2 | Show In Selection | 选区内 |
    /// | 3 | Show In Line Start | 行首连续空白 |
    /// | 4 | Show In Line End | 行尾连续空白 |
    ///
    /// 存整数而不是五个 bool：五档互斥是**菜单的语义**（永远只有一个勾），
    /// 五个 bool 会出现「全勾上按谁算」这种没有答案的状态。越界的值（手改成
    /// `9`）由 `WhitespaceMode::from_index` 回落第 0 档，不会让整份 prefs 作废。
    #[serde(default)]
    pub view_whitespace: usize,
    /// `Text ▸ Highlight Caret Line`：光标所在行铺一层很淡的底色。
    ///
    /// 出厂 `false`（视觉基线设置页那一格是空的）。命名前缀是 `text_` 而不是
    /// `view_`：写它的菜单项在 `Text` 那一栏，与 `text_alternate_row_bg` 同组。
    /// 消费在 `ui/terminal/view.rs` 的逐行循环里——它必须压在字形**之下**。
    #[serde(default)]
    pub text_highlight_caret_line: bool,
    /// 高亮光标下的词 / 当前折叠块 / 当前括号对。出厂全关（视觉基线不变）。
    #[serde(default)]
    pub text_highlight_caret_word: bool,
    #[serde(default)]
    pub text_highlight_current_fold: bool,
    #[serde(default)]
    pub text_highlight_current_pair: bool,
    /// `Preferences ▸ Highlight ▸ Rainbow brackets`：按嵌套深度给 `()[]{}` 上色。
    /// 出厂关（视觉基线不变），与同页其它高亮同款落盘 / 每标签一份。
    #[serde(default)]
    pub text_highlight_rainbow_brackets: bool,
    /// `Preferences ▸ Highlight ▸ Highlight the incremental search`：查找命中铺底色。
    ///
    /// 出厂 **`true`**，与同页其余四项不同——那四项是**还没做**，这一项是
    /// **本来就在做、只是没有开关**：`ui/terminal/pane_overlays.rs` 一直
    /// 就无条件把命中画成底色（当前命中用前景色、其余金黄）。
    /// 出厂给 `false` 等于升级一次就把既有的查找高亮关掉，属于行为倒退。
    ///
    /// 读者是窗口级字段（`TerminalWindow::text_highlight_search`），
    /// 改完**当场对全部标签生效**——不像 `text_highlight_caret_line` 落在
    /// `Tab.marks` 上（每条标签各一份）。两者语义不同，别顺手合并。
    #[serde(default = "default_true")]
    pub text_highlight_search: bool,
    /// `prefs.confirm_close_multi_tabs`（原 Behaviour 页已移除）：还开着别的标签时，
    /// 关一条标签要不要先问一句。
    ///
    /// 出厂 **`true`**——现状就是「关标签一律确认」，改默认值等于升级一次
    /// 就把一张已经张开的网收掉。
    ///
    /// **只剩一条标签时不看这个字段**：它说的是 *multiple*，拿它去管「最后一条」
    /// 等于偷偷减掉现有的确认网（那一格仍由 `suppress_close_session` 决定）。
    /// 将来谁想「顺手统一」成一条判据，先看 `confirm::should_confirm_close`。
    #[serde(default = "default_true")]
    pub confirm_close_multi_tabs: bool,
    /// `Terminal ▸ Show paste dialog on the main screen`：主屏粘贴前先弹确认。
    ///
    /// 出厂 `false` = 现在的行为（粘贴立刻写进 PTY）。粘贴是把剪贴板里的字节
    /// 直接喂给远端 shell，所以这个开关的默认值必须是「不改变现状」。
    #[serde(default)]
    pub paste_confirm_main: bool,
    /// `Terminal ▸ Show paste dialog on the alternative screen`：备用屏粘贴前先确认。
    ///
    /// 与 `paste_confirm_main` 分开：全屏程序（vim / less / htop）里粘贴的风险
    /// 与在 shell 提示符下完全不同，一个开关替另一个做决定就是不诚实的粒度。
    #[serde(default)]
    pub paste_confirm_alt: bool,
    /// `Terminal ▸ Mouse Actions ▸ Right click:` 的行为档（0 起，见
    /// [`RIGHT_CLICK_ACTIONS`]）。
    ///
    /// 出厂 `0` = 显示菜单，即现在的行为——改默认值等于升级一次就把右键从
    /// 「看菜单」变成「直接粘贴」，那是会往远端 shell 里灌字节的副作用。
    #[serde(default)]
    pub right_click_action: usize,
    /// `Terminal ▸ Mouse Actions ▸ When I select text, copy it to clipboard`：
    /// 选中即复制。
    ///
    /// 出厂 `false` = 现在的行为（复制必须显式：`Cmd+C` / 右键 Copy）。
    /// 打开后**每一段新选区**在指针抬起时写一次剪贴板，会覆盖用户剪贴板里的
    /// 旧内容——所以它必须是一个用户亲手打开的开关，而不是默认值。
    #[serde(default)]
    pub copy_on_select: bool,
    /// `Preferences ▸ Mouse Tracking` 的四个**事件掩码**（出厂全开 = 现状）。
    ///
    /// 底层协议只有「总开关 + mode + encoding」三档（`DECSET ?1000/1002/1003`
    /// 等），没有「move / wheel / click / middle 各一个 bool」。这四个字段是
    /// **投影层的掩码**：远端 DECSET 仍然决定编码档位，掩码只决定某一类本地
    /// 指针事件要不要喂给编码器。双击没有对应协议事件（`term-core` 不报告
    /// 双击），所以那一栏不是缺字段，而是没语义。
    #[serde(default = "default_true")]
    pub mouse_move_event: bool,
    #[serde(default = "default_true")]
    pub mouse_wheel_event: bool,
    #[serde(default = "default_true")]
    pub mouse_click_event: bool,
    #[serde(default = "default_true")]
    pub mouse_middle_event: bool,
    /// 快捷命令栏的三个显示开关。兼容终端 WindTerm 把它们放在 `user.config` 的
    /// `quickbar.*` 一组——**不**在 `quickbar.config`（那份只有命令表）。本应用
    /// 照兼容终端的位置放进 `prefs.json`，渲染期由 [`QuickBarDisplay`] 从 `Prefs` 读
    /// 初值携带，与命令表（读 `quickbar.config`）分两条路、各写各的文件。
    ///
    /// 出厂：`show_multiple_groups` / `show_group_title` 为 `true`（视觉基线底栏
    /// 四组全显示、每组带标题）；`alphabetical` 为 `false`——main-dark 的 File
    /// 组是 `Copy / Move / Remove / Rename / Property` 而非字母序，默认勾上会让
    /// 整条基线变样。老文件缺这三个键时，serde 默认与 `Default` 一致（都回出厂）。
    #[serde(default = "default_true")]
    pub quickbar_show_multiple_groups: bool,
    #[serde(default = "default_true")]
    pub quickbar_show_group_title: bool,
    #[serde(default)]
    pub quickbar_alphabetical: bool,
    /// `prefs.show_address_bar`（原 Toolbar / Menu 页已移除）：工具行右段那块 `#333333` 的
    /// 面包屑面板（`(i) │ ▸ 分组 ▸ 会话`，点它开地址栏浮层）。
    ///
    /// 出厂 `true` = 现状（视觉基线里面板段一直在），所以升级不改外观；
    /// 关掉只是不画那一段，左端按钮组与右端三件菜单原样保留。
    #[serde(default = "default_true")]
    pub show_address_bar: bool,
    /// `prefs.auto_hide_menu_bar`（原 Toolbar / Menu 页已移除）：出厂关（现状常驻），
    /// 打开后菜单条收成一条细带，指针靠近顶端或按住 Alt 时展开。
    #[serde(default)]
    pub auto_hide_menu_bar: bool,
    /// `Preferences ▸ Security ▸ Lock screen timeout`（秒）。
    /// 出厂 1800 = 30 分钟，与官方文档默认一致；0 = 不自动锁。
    #[serde(default = "default_1800")]
    pub lock_timeout_secs: u32,
    /// `prefs.auto_reconnect`（原 Behaviour 页已移除）。
    /// 出厂 false：断线重连会往远端重新发包，不能替用户默认打开。
    #[serde(default)]
    pub auto_reconnect: bool,
    /// 整窗主题名，取值来自 `term_core::theme::BUILTIN_THEME_NAMES`。
    ///
    /// 这里必须写 `#[serde(default = "default_theme_name")]` 而不是裸
    /// `#[serde(default)]`：后者的缺省是 `String::new()`（空串），与
    /// `Prefs::default()` 的 `dige-black` 不一致。老 `prefs.json` 缺这个键时
    /// 空串一路走到 `load_palette_named("")`，拿到的是「没有主题名的兜底
    /// 16 色」，而不是 dige-black 整窗主题。两处默认值同一函数，就不会再漂。
    #[serde(default = "default_theme_name")]
    pub theme_name: String,
    /// `Text ▸ Font ▸ Prefer theme fonts`：出厂 false（主题通常没声明字体）。
    #[serde(default)]
    pub prefer_theme_fonts: bool,
    /// `Tabs ▸ Activate the tab after hovering`：出厂关（悬停切焦点会抢输入）。
    #[serde(default)]
    pub tab_hover_activate: bool,
    /// 悬停多久才切换（毫秒），出厂 200ms。
    #[serde(default = "default_200")]
    pub tab_hover_delay_ms: u32,
    /// Preferences ▸ Proxy 的全局默认：0 = 不用 / 1 = HTTP / 2 = SOCKS5。
    #[serde(default)]
    pub global_proxy_type: usize,
    #[serde(default)]
    pub global_proxy_host: String,
    #[serde(default)]
    pub global_proxy_port: u16,
    /// `Tabs ▸ Tab thumbnail radio`：缩略图相对标签宽度的百分比，0 = 关闭。
    #[serde(default)]
    pub tab_thumbnail_pct: usize,
    /// 触发器规则（事件 → 匹配 → 动作）。落盘在 `prefs.json` 的 `triggers`
    /// 数组里，运行时由每个 tab 的 `Matcher` 消费。空数组 = 出厂态（没有规则）。
    ///
    /// 规则类型定义在 `term_core::trigger::Rule`：数据层下放到最低 crate，
    /// `term-profile` 因此只依赖 `term-core`，不反向依赖 `term-app`（应用层），
    /// 守住 ADR-0014 的分层。`#[serde(default)]` 让老文件 / 缺键回落空规则集，
    /// 而不是整份 `prefs` 作废。
    #[serde(default)]
    pub triggers: Vec<Rule>,
    /// OneKey 登录方案（Tool ▸ OneKeys… 编辑器写这里）。落盘在 `prefs.json`
    /// 的 `onekeys` 数组；连接时按会话的 `onekey` 宏名取用（见
    /// `term_core::onekey`）。
    ///
    /// 与 `triggers` 同层纪律：类型定义在 `term_core::onekey`，`term-profile`
    /// 只依赖 `term-core`，不反向依赖 `term-app`。`#[serde(default)]` 让老
    /// 文件 / 缺键回落空集，而不是整份 `prefs` 作废。
    #[serde(default)]
    pub onekeys: Vec<OneKey>,
    /// Tunnels 设置对话框里的**命名隧道**（2026-11-17）。
    ///
    /// 与 `onekeys` 同层纪律：类型在 `term_core::tunnel`，`term-profile` 不
    /// 反向依赖 `term-app`。`#[serde(default)]` 让老文件 / 缺键回落空集——
    /// 出厂态就是「一条隧道都没配」。
    ///
    /// 它与「会话里的转发规则」（`SessionConfig::tunnels` 那一串文本）是
    /// **两件事**：那边是「连上就照这几行 `-L` 开转发」，这里是「一条有名字、
    /// 有启用开关、能被单独打开/关闭的隧道」，可以指向任意会话。两边的规则
    /// 最终都走同一个 `Forward::parse`。
    #[serde(default)]
    pub tunnels: Vec<TunnelProfile>,
    /// 隧道**总线开关**（菜单栏 `Tunnels` 下拉里那个蓝色开关）。
    ///
    /// 关掉 = 所有转发立刻停、配置一条不丢；再打开 = 按各自的 `status` 重起。
    /// 出厂 `true`：没配过隧道时它没有可关的东西，写成 `false` 只会让第一次
    /// 配好的人以为「为什么点了启用还是不通」。
    #[serde(default = "default_true")]
    pub tunnels_enabled: bool,
    /// 重绑后的快捷键落盘。元素是 `(DEFAULT_SHORTCUTS 下标, 新和弦文本)`，
    /// 空 = 全部用出厂绑定。
    ///
    /// 下标而非命令 ID：运行期派发本就按下标取 `DEFAULT_SHORTCUTS[i]`，
    /// 复用同一套下标就不需要再建一张「命令 ID → 下标」的反查表。下标越界
    /// （上游列表改过、或老文件残留一个不存在的条目）时，派发侧
    /// `DEFAULT_SHORTCUTS.get(i).is_some()` 会自然跳过，不会 panic 也不会
    /// 把旧和弦重新点亮。
    ///
    /// `#[serde(default)]`：老文件 / 缺键回落空覆盖集（全出厂绑定）；序列化形态
    /// 是 `[[3,"Cmd+K"], ...]`，与 `shortcut_overrides` 的运行期副本同构，
    /// 构造时一次 `clone()` 就能接上，不需要格式转换。
    #[serde(default)]
    pub shortcut_overrides: Vec<(usize, String)>,
    /// 新格式：按**命令 ID** 存用户重绑，避免出厂表增删项后下标漂移。
    /// 老文件的 `shortcut_overrides` 在构造窗口时由 `shortcut_map::migrate_legacy`
    /// 迁到这里；本字段一旦有值即以它为准。
    #[serde(default)]
    pub shortcut_bindings: Vec<(String, String)>,
    /// 窗口透明度 —— 透明开关。开 = 让 OS 窗口背景透明（eframe 0.29 经
    /// `NativeOptions.viewport.transparent` 转 winit `with_transparency`，
    /// `glow_integration.rs:926` / `wgpu_integration.rs:194` 已确认跨平台走通）。
    ///
    /// 仅当此开关为真、且 `update()` 里按 `window_opacity` 把 egui 背景 alpha
    /// 渲染出来时，窗口才会真的半透明。默认关：保持现状（不透明窗口），
    /// 现有行为 100% 不变；这是「功能优先、基线后置」策略下的安全默认值。
    #[serde(default)]
    pub window_transparent: bool,
    /// 窗口不透明度（1.0 = 完全不透明）。仅在 `window_transparent` 为真时
    /// 生效；设太低（<0.3）恐整窗不可读，故渲染侧钳制到 [0.3, 1.0]。
    ///
    /// 落盘允许任意 f32，钳制放在读取/渲染侧，避免「存了一个非法值就再也救不回」。
    /// 注意：serde 的 `default` 取的是**字段类型**的 `Default`（f32 = 0.0），
    /// 不是本 struct 的 `Default` impl（那是 1.0）。出厂不透明度必须是 1.0，
    /// 所以这里用 `default = "default_opacity_one"` 显式指定，老文件缺键回落 1.0。
    #[serde(default = "default_opacity_one")]
    pub window_opacity: f32,
    /// 地址栏的「最近使用」目标串（`user@host:port` 形态），MRU 在队首。
    ///
    /// # 为什么不放 `term-store`
    ///
    /// 会话库是用户**显式创建**的记录（有名字、分组、认证方式），最近使用是
    /// 随手敲下的痕迹。混在一起会污染会话树（「我什么时候建过一个叫
    /// `10.0.0.5:2222` 的会话？」），而且改库要动 schema 与迁移；
    /// 换一份 order-only 的偏好列表，读写都是 load-modify-save，没有迁移。
    ///
    /// # 为什么在这里裁剪而不是读的时候裁
    ///
    /// 上限（[`RECENT_TARGETS_CAP`]）由**写者**执行（`term_app::address::push_recent`
    /// 的 `cap`，投影层传这个常量）。读的时候不再裁一次：手改过的文件多写几条
    /// 也不致命，而「读时裁剪」会让 `load-modify-save` 静默改掉用户的文件。
    ///
    /// `#[serde(default)]`：老文件没有这个键时回落空列表（等于「还没用过地址栏」），
    /// 且不连坐同文件里的任何其它设置。
    #[serde(default)]
    pub recent_targets: Vec<String>,
    /// ADR-0027：最近跑过的脚本路径（MRU，最多 [`RECENT_SCRIPTS_MAX`] 条）。
    ///
    /// 与 `recent_targets` 同一套规矩：只留「最近用过的」，不存脚本内容。
    /// 空表不序列化——老 `prefs.json` 里没有这个键时读出来也是空表，两种情况
    /// 在磁盘上一样，不需要区分。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_scripts: Vec<String>,
    /// 脚本注册表（脚本配置管理页的落点）。空表 = 还没有注册任何脚本。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scripts: Vec<ScriptEntry>,
    /// 首次启动引导（Welcome → 探测 SSH 主机）是否已经走完，或者用户主动跳过。
    ///
    /// # 为什么是「已看过」而不是「首次运行」
    ///
    /// 判据不能是「`prefs.json` 在不在」：那个文件存在**不代表**用户看过引导，
    /// 他可能只改过一次设置；判据也不能是「`layout.json` 在不在」（见
    /// `apps/desktop/src/ui/construct.rs` 的 `init`）：删过一次布局会被误当成第一次用。
    /// 只有这一个显式的布尔位能区分「走过 / 没走过」。
    ///
    /// 老文件没有这个键时 `#[serde(default)]` 给 `false`，于是**升级上来的老用户
    /// 也会看一次引导**。这是有意的：引导里那句「可以把你 ssh config 里的主机导进
    /// 会话树」对老用户同样有用，而它只出现一次（走完与跳过都写回 `true`）。
    ///
    /// 字段只增不减、带 `default`（见本文件模块文档），所以加它不需要 migration，
    /// 也不会连坐同文件里的任何其它设置。
    #[serde(default)]
    pub onboarded: bool,
    /// I-AC：自动补全偏好。**六个勾出厂全空**、触发 1 字符、列表 7 行、
    /// 不预选——逐项对齐视觉基线 `08-terminal-auto-completion.png` 的出厂态。
    ///
    /// 类型直接复用 `term-core::completion::Settings`：引擎读的与实际落盘的
    /// 是同一份结构，设置页就不必在两套字段之间做转换（每个字段都有
    /// 真实读者）。`#[serde(default)]` 让老 `prefs.json` 缺这个键时回落到
    /// 上面的出厂值，不影响同文件里的其它设置。
    #[serde(default)]
    pub completion: term_core::completion::Settings,
    /// 这份快照的盘上基线指纹。**不要手写**：只由 [`load`] / 成功的
    /// [`save`] 维护，见 [`ExpectedFingerprint`]。
    #[serde(skip)]
    pub expected_fingerprint: ExpectedFingerprint,
}

/// `command_history_save` 的出厂默认：开（视觉基线设置页出厂态是勾上的）。
fn default_true() -> bool {
    true
}
fn default_200() -> u32 {
    200
}
fn default_1800() -> u32 {
    1800
}

/// 整窗主题的缺省名。serde 与手写 `Default` 共用这一处，老文件缺键与没有文件
/// 两种处境必须拿到同一个主题（见 `theme_name` 字段注释）。
fn default_theme_name() -> String {
    term_core::theme::DEFAULT_THEME_NAME.to_string()
}

/// 窗口不透明度的出厂默认（1.0 = 完全不透明）。f32 的 `Default` 是 0.0，
/// 不能用在 serde `default` 上，否则老文件缺键会回落成「全透明」——那比「回落成
/// 出厂不透明」危险得多（整窗可能直接看不见）。见 `window_opacity` 字段注释。
fn default_opacity_one() -> f32 {
    1.0
}

/// 光标周期 / 宽度的反序列化缺省：老 prefs 文件没有这两条时回落
/// `term_core::config` 里的出厂值（0.5 秒 / 1.5 像素），行为与「此前是
/// 渲染层常数」时一致。与 `caret_style` 用 0 哨兵不同——这两个的 0 不是合法
/// 值，用显式 default 比哨兵更直白。
fn default_caret_period() -> f32 {
    term_core::config::DEFAULT_CARET_PERIOD
}
fn default_caret_width() -> f32 {
    term_core::config::DEFAULT_CARET_WIDTH
}

/// 渲染侧窗口不透明度的上下限。`window_opacity` 允许落盘任意 `f32`
/// （含用户手改的越界值），**钳制只发生在读取 / 渲染侧**，不回写盘：
/// 存了一个非法值就再也救不回来，比一时看不清更糟。见字段注释。
pub const WINDOW_OPACITY_MIN: f32 = 0.3;
pub const WINDOW_OPACITY_MAX: f32 = 1.0;

/// 把落盘值夹进整窗可读区间；`NaN` / 无穷一律按出厂不透明（1.0）处理。
///
/// 这是**唯一**的钳制函数：菜单三档预设、设置页滑杆、启动读盘、每帧渲染
/// 都经它，不存在第二份「0.3 还是 0.25」的口径。返回值只给调用方，
/// 调用方拿它改 `egui::Visuals` 的背景 alpha，不改 `Prefs` 本身。
pub fn clamp_window_opacity(value: f32) -> f32 {
    if !value.is_finite() {
        return WINDOW_OPACITY_MAX;
    }
    value.clamp(WINDOW_OPACITY_MIN, WINDOW_OPACITY_MAX)
}

impl Default for Prefs {
    /// 出厂态 = 视觉基线设置页上看到的那一份：保存命令行历史是**开**。
    ///
    /// 不能靠 `derive(Default)`：布尔默认是 `false`，而 `command_history_save`
    /// 的 serde 默认是 `true`。两处不一致会让「没有 `prefs.json`」与「文件里
    /// 缺这个键」走进两种行为——设置页的勾选态会随文件在不在而变。
    fn default() -> Self {
        Self {
            language: String::new(),
            suppress_close_window: false,
            suppress_close_session: false,
            // 没保存过默认模板：界面回落到出厂 SessionConfig。
            default_session: None,
            log_type: 0,
            log_overwrite: 0,
            log_file_name: String::new(),
            log_max_size_mb: 0,
            log_content_pattern: String::new(),
            startup_mode: String::new(),
            startup_sessions: Default::default(),
            command_history_days: 0,
            command_history_size: 0,
            command_history_save: true,
            caret_style: 0,
            caret_period: term_core::config::DEFAULT_CARET_PERIOD,
            caret_width: term_core::config::DEFAULT_CARET_WIDTH,
            search_url: String::new(),
            filer_show_hidden: false,
            text_alternate_row_bg: false,
            text_hide_cursor_while_typing: false,
            focus_new_tab: true,
            font_size: 0.0,
            font_family: Default::default(),
            font_weight: 0,
            tab_activate_after_close: 0,
            tab_double_click: 0,
            tab_elide: 0,
            // 编号 / 中键 / 右键三档的 0 = 接线前的行为（不编号、中键无事、
            // 右键弹标签菜单），老文件缺键时也是这个值。
            tab_numbers: 0,
            tab_middle_click: 0,
            tab_right_click: 0,
            // 全 0 = 「没设置过」，由 `HexParams::from_prefs` 回落出厂值。
            hex_base: 0,
            hex_columns: 0,
            hex_group: 0,
            hex_division: 0,
            alternate_column_bg: true,
            // 空白/符号可视化六项：出厂全关，屏幕上不画任何标记。
            // 与 serde 默认逐项对齐（都是 `false` / `0`）——两处给出不同画法
            // 会让「删掉 prefs.json」看起来像一个功能改动。
            view_line_endings: false,
            view_column_marker: 0,
            view_indent_guides: false,
            view_wrap_symbols: false,
            // Wrap 页两项真控件：出厂 char + 0（接线前行为，零像素变化）。
            wrap_mode: 0,
            wrap_column: 0,
            view_whitespace: 0,
            text_highlight_caret_line: false,
            text_highlight_caret_word: false,
            text_highlight_current_fold: false,
            text_highlight_current_pair: false,
            text_highlight_rainbow_brackets: false,
            // 查找命中高亮出厂开：它本来就是无条件在画的效果，这里只是给它
            // 一个能关的开关（见字段注释）。与 serde 的 `default_true` 对齐。
            text_highlight_search: true,
            // 与 serde 默认逐项对齐：出厂就问「关多标签」，粘贴不确认。
            confirm_close_multi_tabs: true,
            paste_confirm_main: false,
            paste_confirm_alt: false,
            // 右键出厂仍是「显示菜单」，选中即复制出厂关——两项都守现状。
            right_click_action: 0,
            copy_on_select: false,
            // 四类鼠标事件出厂全报，缺键时也是全报（不改升级前的行为）。
            mouse_move_event: true,
            mouse_wheel_event: true,
            mouse_click_event: true,
            mouse_middle_event: true,
            // 快捷命令栏显示开关：出厂「显示多组 + 显示组标题 + 不按字母序」，
            // 与 serde 默认逐项对齐，守住视觉基线底栏。
            quickbar_show_multiple_groups: true,
            quickbar_show_group_title: true,
            quickbar_alphabetical: false,
            // 地址栏面板出厂显示（与 serde `default_true` 对齐，守住视觉基线）。
            show_address_bar: true,
            auto_hide_menu_bar: false,
            lock_timeout_secs: 1800,
            auto_reconnect: false,
            theme_name: default_theme_name(),
            prefer_theme_fonts: false,
            tab_hover_activate: false,
            tab_hover_delay_ms: 200,
            global_proxy_type: 0,
            global_proxy_host: String::new(),
            global_proxy_port: 0,
            tab_thumbnail_pct: 0,
            // 出厂没有任何触发器规则。
            triggers: Vec::new(),
            onekeys: Vec::new(),
            // 2026-11-17：出厂没有命名隧道；总线开关默认开着（见字段注释）。
            tunnels: Vec::new(),
            tunnels_enabled: true,
            // 出厂没有任何重绑定的快捷键（全部用出厂和弦）。
            shortcut_overrides: Vec::new(),
            shortcut_bindings: Vec::new(),
            // 出厂是不透明窗口（透明开关默认关，现状不变）。
            window_transparent: false,
            window_opacity: 1.0,
            // 出厂没有任何「最近使用」的目标（等于「还没用过地址栏」）。
            recent_targets: Vec::new(),
            // ADR-0027：出厂 = 还没跑过脚本。
            recent_scripts: Vec::new(),
            scripts: Vec::new(),
            // 出厂 = 还没走过首次启动引导。**这一项必须是 `false`**：
            // 出厂值写成 `true` 等于给所有新装机器关掉引导，而引导是本刀唯一的
            // 入口；写成随 `Default` 派生的 `false` 与 serde 默认一致，不需要特判。
            onboarded: false,
            // I-AC：与 serde 默认逐项对齐——出厂时补全整体关闭，界面与
            // 视觉基线那张六勾全空、1 字符 / 7 行 / Nowhere 的截图一致。
            completion: term_core::completion::Settings::default(),
            // 出厂快照没有盘上基线：第一次写不比对，写完再记。
            expected_fingerprint: ExpectedFingerprint::default(),
        }
    }
}

/// 手改过的文件可能把 Session 列表写成 2 个或 9 个。数组默认反序列化会因此
/// **整份 prefs 作废**（语言、两个抑制位、日志设置一起丢），代价远大于这一项。
/// 这里收成固定四位：缺的补空、多的丢掉。
fn de_sessions<'de, D>(d: D) -> Result<[String; 4], D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<String>::deserialize(d)?;
    let mut out: [String; 4] = Default::default();
    for (slot, value) in out.iter_mut().zip(values) {
        *slot = value;
    }
    Ok(out)
}

impl Prefs {
    /// 日志配置 → 领域选项。**这是 `prefs.json` 那五个字段的唯一下游**：
    /// 配置界面、无头桥接、写入端都从这里取，别各自再解释一遍
    /// `1 = 覆盖 / 2 = 追加` 之类的序号。
    ///
    /// 空串回落：老文件没有这两项，`term_core::log_opts` 的 `normalized()`
    /// 会换成视觉基线出厂模板。
    pub fn log_options(&self) -> term_core::log_opts::LogOptions {
        term_core::log_opts::LogOptions {
            log_type: term_core::log_opts::LogType::from_u8(self.log_type),
            overwrite: match self.log_overwrite {
                1 => Some(true),
                2 => Some(false),
                _ => None,
            },
            file_name: self.log_file_name.clone(),
            max_size_mb: self.log_max_size_mb,
            content_pattern: self.log_content_pattern.clone(),
        }
        .normalized()
    }

    /// 领域选项 → 五个字段（load-modify-save 的一半，调用方负责读盘与写盘）。
    pub fn set_log_options(&mut self, opts: &term_core::log_opts::LogOptions) {
        self.log_type = opts.log_type.as_u8();
        self.log_overwrite = match opts.overwrite {
            Some(true) => 1,
            Some(false) => 2,
            None => 0,
        };
        self.log_file_name = opts.file_name.clone();
        self.log_max_size_mb = opts.max_size_mb;
        self.log_content_pattern = opts.content_pattern.clone();
    }

    /// 由偏好构造命令行历史存储（读取三字段）。
    ///
    /// `0` 是「未设置」哨兵：回落出厂默认（`days = 30`、`size = 10000`），
    /// 与灰显 `CommandHistory` 设置页一致；老文件读到 `0` 不会变成「不保留」。
    /// `command_history_save` 仅影响落盘（由 `term-runtime::persist` 负责），
    /// 不影响内存结构，故此处不消费。
    pub fn command_history_store(&self) -> CommandHistory {
        CommandHistory::new(self.history_capacity(), self.history_ttl_days())
    }

    /// `command_history_size` 的生效值：`0` 哨兵回落出厂默认 10000 条。
    pub fn history_capacity(&self) -> usize {
        if self.command_history_size == 0 {
            10000
        } else {
            self.command_history_size
        }
    }

    /// `command_history_days` 的生效值：`0` 哨兵回落出厂默认 30 天。
    pub fn history_ttl_days(&self) -> u64 {
        if self.command_history_days == 0 {
            30
        } else {
            self.command_history_days
        }
    }

    /// `caret_style` 的生效值。越界（手改过的文件写了个 `9`）回落 `0` =
    /// Block——与 `term_core::config::caret_style_to_shape_blink` 的兜底一致，
    /// 读取方不必各自再夹一次。
    pub fn caret_style_index(&self) -> usize {
        if self.caret_style < term_core::config::CARET_STYLES.len() {
            self.caret_style
        } else {
            0
        }
    }

    /// 光标周期的生效值：夹进 `CARET_PERIOD_RANGE`，`NaN` 回落出厂 0.5。
    /// 与渲染层共用 `clamp_caret_period`，两处不会各夹一套。
    pub fn caret_period_value(&self) -> f32 {
        term_core::config::clamp_caret_period(self.caret_period)
    }

    /// 光标宽度的生效值：夹进 `CARET_WIDTH_RANGE`，`NaN` 回落出厂 1.5。
    pub fn caret_width_value(&self) -> f32 {
        term_core::config::clamp_caret_width(self.caret_width)
    }

    /// `font_size` 的生效值：`0.0` 哨兵回落出厂 13pt，越界夹回可读区间。
    ///
    /// 区间与渲染层（`apps/desktop/src/fonts.rs` 的 `MIN_FONT_SIZE` /
    /// `MAX_FONT_SIZE`）**必须一致**：设置页按 6–48 画滑块，渲染层若只认 8–40，
    /// 用户拖到 44 会看到 44、实际画成 40。`term-runtime` 不依赖 `apps/desktop`，
    /// 两个 crate 各写一份口径，靠这条注释与那两行常量对齐；单测钉住两端。
    pub fn font_size_value(&self) -> f32 {
        if self.font_size <= 0.0 {
            term_core::config::DEFAULT_FONT_SIZE
        } else {
            self.font_size.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX)
        }
    }

    /// `font_weight` 的生效值：`0` 哨兵回落 400（Regular），越界夹到
    /// OS/2 的合法档位区间。
    ///
    /// 区间与渲染层（`apps/desktop/src/fonts/mod.rs` 的 `normalize_weight`）
    /// **必须一致**：两处给出不同口径时，设置页显示 100、实际装成 400 这种
    /// 「改了没反应」最难查。单测钉住两端。
    pub fn font_weight_value(&self) -> u16 {
        if self.font_weight == 0 {
            400
        } else {
            self.font_weight.clamp(100, 900)
        }
    }

    /// `font_family` 的生效值：去掉每行首尾空白。
    ///
    /// 单独一个函数是因为四行会被三个地方读（启动装字、设置页显示、
    /// 写回时比对是否变化），三处都自己 `trim()` 迟早在某处漏掉，
    /// 于是「名字后面多一个空格」变成一个查不出来的「字族没装」。
    pub fn font_family_value(&self) -> [String; 4] {
        [
            self.font_family[0].trim().to_string(),
            self.font_family[1].trim().to_string(),
            self.font_family[2].trim().to_string(),
            self.font_family[3].trim().to_string(),
        ]
    }

    /// `tab_activate_after_close` 的生效值。越界（手改文件写了 7）回落 0 =
    /// 「下一个」，也就是这一项接线之前的行为——不是随便挑一档。
    pub fn tab_activate_after_close_kind(&self) -> usize {
        if self.tab_activate_after_close < TAB_ACTIVATE_MODES {
            self.tab_activate_after_close
        } else {
            0
        }
    }

    /// `tab_double_click` 的生效值。越界回落 0 = 没有动作（现状）。
    pub fn tab_double_click_action(&self) -> usize {
        if self.tab_double_click < TAB_DOUBLE_CLICK_ACTIONS {
            self.tab_double_click
        } else {
            0
        }
    }

    /// `right_click_action` 的生效值。越界（手改文件写了 9）回落 0 =
    /// 「显示菜单」，即这一项接线之前的行为——右脚本来的样子。
    pub fn right_click_kind(&self) -> usize {
        if self.right_click_action < RIGHT_CLICK_ACTIONS {
            self.right_click_action
        } else {
            0
        }
    }

    /// `tab_elide` 的生效值。越界回落 0 = 不省略（现状）。
    pub fn tab_elide_mode(&self) -> usize {
        if self.tab_elide < TAB_ELIDE_MODES {
            self.tab_elide
        } else {
            0
        }
    }

    /// `tab_numbers` 的生效值。越界回落 0 = 不显示（现状）。
    pub fn tab_number_mode(&self) -> usize {
        if self.tab_numbers < TAB_NUMBER_MODES {
            self.tab_numbers
        } else {
            0
        }
    }

    /// `tab_middle_click` 的生效值。越界回落 0 = 没有动作（现状）。
    pub fn tab_middle_click_action(&self) -> usize {
        if self.tab_middle_click < TAB_MIDDLE_CLICK_ACTIONS {
            self.tab_middle_click
        } else {
            0
        }
    }

    /// `tab_right_click` 的生效值。越界回落 0 = 标签菜单（现状）。
    pub fn tab_right_click_action(&self) -> usize {
        if self.tab_right_click < TAB_RIGHT_CLICK_ACTIONS {
            self.tab_right_click
        } else {
            0
        }
    }

    /// `search_url` 的生效值：空串 / 只有空白 → 出厂模板（`search::default_url`）。
    ///
    /// 收成 `&str` 而不是让每个读者各写一次三元表达式：设置页要显示它、
    /// `Effect::SearchOnline` 要用它拼 URL，两处必须看到同一条模板。
    pub fn search_url_template(&self) -> &str {
        if self.search_url.trim().is_empty() {
            crate::search::default_url()
        } else {
            &self.search_url
        }
    }

    /// Reset 快捷键时调用：**两份字段一起清**。只清新字段时，下次启动
    /// `migrate_legacy` 会把旧下标重新点亮，用户看到的是「Reset 后又复活」。
    pub fn reset_shortcut_bindings(&mut self) {
        self.shortcut_bindings.clear();
        self.shortcut_overrides.clear();
    }

    /// 快捷命令栏的渲染开关，从 `prefs` 读初值（兼容终端 `user.config` 的位置）。
    /// 三个老文件缺键 / 刚装的用户都回落出厂态（多组 + 组标题 + 不字母序）。
    pub fn quick_bar_display(&self) -> QuickBarDisplay {
        QuickBarDisplay {
            show_multiple_groups: self.quickbar_show_multiple_groups,
            show_group_title: self.quickbar_show_group_title,
            alphabetical: self.quickbar_alphabetical,
        }
    }
}

fn prefs_path() -> Option<PathBuf> {
    Some(term_paths::profile_dir()?.join(PREFS_FILE))
}

// 最近一次写盘失败的原因。只是一条「给 UI 看的一次性提示」，不是冲突判据：
// 判据绑在每份 Prefs 实例的 `expected_fingerprint` 上（见上文）。
thread_local! {
    static LAST_SAVE_ERROR: Cell<Option<SaveError>> = const { Cell::new(None) };
}

/// 写偏好失败的原因。`save()` 的 `bool` 只够调用方知道「没写成」，
/// 这一种类型才说得出**为什么**——冲突时 UI 要提示用户重做那一项修改，
/// 而不是笼统地说「保存失败」或（更糟）谎报成功。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveError {
    /// 拿不到 profile 目录。
    NoProfileDir,
    /// 旧文件读不出来又备份不下，按既有规矩拒写。
    PreserveFailed,
    /// `load()` 之后盘上内容被别的进程 / 编辑器改过，这次改动需要重做。
    Conflict,
    /// 序列化失败或原子写失败。
    Io,
}

impl SaveError {
    /// 给用户看的一句话。刻意不写具体键名：我们只发现了「版本变了」，
    /// 不知道对方改了哪一项，不能装作知道。
    pub fn message(self) -> &'static str {
        match self {
            Self::NoProfileDir => "拿不到配置目录，偏好没有保存",
            Self::PreserveFailed => "旧配置读不出来且备份失败，为避免丢数据本次没有写入",
            Self::Conflict => "配置已被其它进程或编辑器改过，请重新打开设置页再改一次",
            Self::Io => "写盘失败，偏好没有保存",
        }
    }
}

/// 最近一次 [`save`] / [`save_to_path`] / [`save_checked`] 的失败原因；
/// 成功时清空。本线程本地——UI 在自己的线程上写完就能取到，用来决定是否
/// 弹「请重做」的提示。
pub fn last_save_error() -> Option<SaveError> {
    LAST_SAVE_ERROR.with(|e| e.get())
}

/// UI 已经把 [`last_save_error`] 画成提示后清空它，免得下一次正常保存之前
/// 一直挂着上次的「请重做」。每帧的首置阶段会取一次（见 `pre_update_gate`）。
pub fn clear_last_save_error() {
    LAST_SAVE_ERROR.with(|e| e.set(None));
}

/// 读偏好。文件不存在 / 读不出来 / JSON 坏了都返回 `None`——调用方据此
/// 保持默认值，不需要区分是哪一种失败（都不该拦住启动）。
///
/// **读的宽容不等于写的宽容**：这三种失败对「能不能覆盖」的答案完全不同，
/// 写路径会用 [`read_disk`] 再问一次（见 [`save`]）。读到这里时还会把这份
/// 字节的指纹记在**返回的实例上**，作为随后那次 save 的冲突基线。
pub fn load() -> Option<Prefs> {
    prefs_path().and_then(|p| load_from(&p))
}

/// `prefs.json` 是否**已经存在**。
///
/// `load()` 的 `None` 是两种完全不同的处境，退出路径要区分：
/// - 文件不存在 = 用户从没表达过偏好，按出厂态；
/// - 文件在但读不出来（坏了 / 权限）= 不知道用户选了什么，按保守态。
///
/// 合并成一种会让「从没设置过」与「设置过但文件坏了」走进同一条分支，
/// 前者不该被当成「用户关掉了保存」。
pub fn exists() -> bool {
    prefs_path().is_some_and(|p| p.is_file())
}

/// 写偏好。失败不崩，与 `layout_state::save` 同一条规矩。
///
/// # 覆盖之前一定先保全
///
/// 这个函数**不**假定盘上那份 `prefs.json` 是它自己写的：用户可能在编辑器里
/// 改过。旧文件读不出来时（手改少了逗号、括号没闭合、甚至是非 UTF-8 字节），
/// 先把它逐字节另存成 `prefs.json.bad-<Unix 秒>` 再覆盖，见
/// [`preserve_broken`]——这一份是「用户原来那份内容」唯一的副本，而
/// `load()` 的 `None` 已经让调用方退回出厂值了，不先备份就等于删掉它。
///
/// 保全做不到时（既读不出来、又存不下一份，如磁盘满 / 目录只读）**这一次不写**
/// 并返回 `false`：宁可让用户这一下改动不生效（下次打开设置页还看到旧值，
/// 改动再点一次就是了），也不要把原文件换成出厂值。
///
/// # 跨进程的 load-modify-save：best-effort 冲突提示，不是事务
///
/// 光保全坏文件还不够：两个进程都读到同一份**合法**的旧文件，各改一个键，
/// 后写者会把先写者那一项整份盖回去，而且没有任何报错。所以写前再读一次
/// 指纹，与**这份实例自己**在 `load()` / 上次成功 `save()` 记下的基线比对——
/// 不一致就是 [`SaveError::Conflict`]，写被拒绝，用户重做即可，绝不静默回退。
///
/// 这是 best-effort 的提示，**不是 CAS、也不是事务**：读指纹与 rename 之间
/// 仍有一个极短的窗口，并行到那一步的写仍可能被覆盖；它挡的是「用户手改 /
/// 另一个进程已经落盘，而我手里是一份陈旧快照」这种常见事故。基线绑在实例
/// 上，同线程另一次无关的 `load()` 不会把过期快照洗白。
///
/// 这一步的代价是每次写多读一遍这个几百字节的文件——设置页的写只在
/// 「松手 / 点一下」时发生，不在每帧。
pub fn save(prefs: &Prefs) -> bool {
    save_checked(prefs).is_ok()
}

/// [`save`] 的显式错误版：调用方要按失败原因给不同提示时用这个
/// （例如冲突要提示「请重新打开设置页改一次」，IO 失败只要重试）。
pub fn save_checked(prefs: &Prefs) -> Result<(), SaveError> {
    let outcome = match prefs_path() {
        Some(path) => save_to_checked(&path, prefs),
        None => Err(SaveError::NoProfileDir),
    };
    match &outcome {
        Ok(_) => {
            LAST_SAVE_ERROR.with(|e| e.set(None));
            // 偏好版本号：每次**成功**写盘推进一格。界面层每帧读一次差额，得到
            // `AppSnapshot.prefs_rev`——脚本据此判断要不要重取偏好，不再拿到死状态。
            WRITE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Err(e) => LAST_SAVE_ERROR.with(|last| last.set(Some(*e))),
    }
    outcome.map(|_| ())
}

static WRITE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 成功写盘的次数。只用来推进界面层的 `prefs_rev`，不落盘、不跨进程。
pub fn write_generation() -> u64 {
    WRITE_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

/// 显式路径版的写盘，语义与 [`save`] 完全一致（同一套覆盖前保全与冲突检测）。
///
/// 只给「解析 → 合并 → 写盘」这条链路的单测用：导入链路要验证坏文件被另存 /
/// 写不进去时拒写，但单测不能去碰用户真实的 profile 目录。生产调用方仍然走
/// [`save`]，保全逻辑全仓只有 [`save_to_checked`] 这一份。
pub fn save_to_path(path: &Path, prefs: &Prefs) -> bool {
    save_to(path, prefs)
}

fn save_to(path: &Path, prefs: &Prefs) -> bool {
    match save_to_checked(path, prefs) {
        Ok(_) => {
            LAST_SAVE_ERROR.with(|e| e.set(None));
            true
        }
        Err(e) => {
            LAST_SAVE_ERROR.with(|last| last.set(Some(e)));
            false
        }
    }
}

/// 一次读盘同时拿到解析结果与字节指纹：冲突检测与「覆盖前保全」必须看
/// **同一份快照**，否则两次读之间正好被别人改掉时又会漏掉冲突。
fn read_disk(path: &Path) -> (Option<u64>, Result<Prefs, LoadError>) {
    if !path.is_file() {
        return (None, Err(LoadError::Missing));
    }
    let Ok(raw) = std::fs::read(path) else {
        return (None, Err(LoadError::Unreadable));
    };
    let fingerprint = fingerprint_bytes(&raw);
    let parsed = std::str::from_utf8(&raw)
        .map_err(|_| LoadError::Unreadable)
        .and_then(|text| serde_json::from_str(text).map_err(|_| LoadError::Malformed));
    (Some(fingerprint), parsed)
}

fn save_to_checked(path: &Path, prefs: &Prefs) -> Result<u64, SaveError> {
    let (fingerprint, parsed) = read_disk(path);

    // best-effort 冲突提示：盘上的内容必须还是**这份实例**见过的那一份。
    // 没基线（第一次写、手改后从未 load 过的显式路径写）才跳过——那种调用方
    // 没有可过期的快照。判据不在线程全局表里，所以同线程别的 load/save 改
    // 不到这份快照的基线，「明明冲突却通过」不再可能。
    if let Some(want) = prefs.expected_fingerprint.get() {
        if fingerprint != Some(want) {
            return Err(SaveError::Conflict);
        }
    }

    // 覆盖前的保全沿用旧规矩：坏文件先逐字节另存，存不下就不写。
    if matches!(parsed, Err(LoadError::Unreadable) | Err(LoadError::Malformed)) {
        match preserve_broken(path) {
            Some(backup) => record_recovery(&backup),
            None => return Err(SaveError::PreserveFailed),
        }
    }
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return Err(SaveError::Io);
        }
    }
    let Ok(json) = serde_json::to_string_pretty(prefs) else {
        return Err(SaveError::Io);
    };
    match write_atomic_if_unchanged(path, fingerprint, json.as_bytes()) {
        Ok(new_fingerprint) => {
            // 这份实例下一次 save 时，基线上移到这次写出的字节。
            prefs.expected_fingerprint.set(Some(new_fingerprint));
            Ok(new_fingerprint)
        }
        Err(AtomicWriteError::Conflict) => Err(SaveError::Conflict),
        Err(AtomicWriteError::Io) => Err(SaveError::Io),
    }
}

// ---- 覆盖前的保全 ----
//
// 上面这些函数只关心「能不能覆盖」。要回答它，必须知道 `load_from` 的 `None`
// 是哪一种失败，所以这里把三件事分开：文件不在 / 文件在但读失败 / 读到了但
// serde 不认。第一件无损（没有东西可以丢），后两件都有东西可以丢。

/// 读 `prefs.json` 失败的原因。
///
/// 三种失败的**处置完全不同**，合并成一个 `None` 只够界面用：
/// - [`LoadError::Missing`]：用户没写过偏好，覆盖是**无损**的；
/// - [`LoadError::Unreadable`]：文件在，但 `read_to_string` 失败（权限 / 非
///   UTF-8 字节 / 磁盘错误）。内容是还在盘上的，只是我们看不到；
/// - [`LoadError::Malformed`]：字节读出来了，但 `serde_json` 不认（少逗号、
///   括号没闭合、字段类型写错…）。用户的字还在，只是解析器读不懂。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoadError {
    /// 文件不存在（或那一条路径不是普通文件）。
    Missing,
    /// 文件在，但读不出字节。
    Unreadable,
    /// 字节读到了，但不是一份合法的 [`Prefs`]。
    Malformed,
}

/// 宽容版：界面只要「读到就用，读不到就用默认值」这一个动作。
///
/// 顺手把这次**亲眼看到**的字节指纹记在**读出来的这份实例上**：随后
/// `load-modify-save` 里的 save 就是拿它跟盘上内容比，发现别人动过就拒写
/// （见 [`save_checked`]）。文件不存在时新实例天然没有基线，不会误判。
/// 关键在于这里不写任何线程全局表——同线程另一次 load 不会洗白旧快照。
fn load_from(path: &Path) -> Option<Prefs> {
    let (fingerprint, mut parsed) = read_disk(path);
    if let Ok(prefs) = &mut parsed {
        prefs.expected_fingerprint.set(fingerprint);
    }
    parsed.ok()
}

/// 同一份坏内容最多试几个后缀名。正常情况下第一个就成功（或复用已有那份），
/// 这个上限只挡住「同一秒里反复写同一份读不出来的文件」这种病态循环。
const BACKUP_ATTEMPTS: u32 = 8;

/// 把读不出来的 `prefs.json` 原样另存一份，返回备份路径。
///
/// 三个实现细节都不是风格问题：
/// - 用 `std::fs::copy`（按字节）而不是 `read_to_string` + 写回：坏文件可能
///   连 UTF-8 都不是，而那正是 [`LoadError::Unreadable`] 的成因——一条要求
///   「先解码再备份」的路会在最需要它的时候失败。
/// - **不 `rename`**：原文件留在原地，后面那次原子写失败时整件事退化成
///   「什么都没发生」；改名则会让文件跑到别处去，设置页的 load 也跟着变。
/// - 同一份内容只留一份：设置页每点一个开关都是一次 load-modify-save，
///   若每次都另存，写失败几轮之后用户的 profile 目录就被同名备份铺满了。
///   先按内容比对已有备份，比中了直接认那一份。
fn preserve_broken(path: &Path) -> Option<PathBuf> {
    for existing in existing_backups(path) {
        if same_bytes(path, &existing) {
            return Some(existing);
        }
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = path.file_name()?.to_string_lossy().into_owned();
    for attempt in 0..BACKUP_ATTEMPTS {
        let candidate = if attempt == 0 {
            path.with_file_name(format!("{name}.bad-{stamp}"))
        } else {
            path.with_file_name(format!("{name}.bad-{stamp}-{attempt}"))
        };
        if !candidate.exists() {
            return std::fs::copy(path, &candidate).ok().map(|_| candidate);
        }
    }
    None
}

/// 同一目录下已经存在的 `prefs.json.bad-*`（排序后返回，便于去重与测试）。
fn existing_backups(path: &Path) -> Vec<PathBuf> {
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return Vec::new();
    };
    let prefix = format!("{name}.bad-");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// 两个文件逐字节相同。任何一步读失败都算「不同」——宁可多存一份备份。
fn same_bytes(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

// ---- 保全之后要让人知道 ----
//
// 「文件还在盘上但用户不知道」和「丢了」在体验上是同一件事：他不会去找。
// 界面在 `apps/desktop`，这一层不认识 egui，所以只留一条进程级记录，由设置页
// 画成一行提示。

/// 最近一次「坏文件被另存」的备份路径。
static LAST_RECOVERY: Mutex<Option<PathBuf>> = Mutex::new(None);

fn record_recovery(backup: &Path) {
    if let Ok(mut slot) = LAST_RECOVERY.lock() {
        *slot = Some(backup.to_path_buf());
    }
}

/// 覆盖坏文件时留下的备份路径（`None` = 本次运行没有保全过什么）。
///
/// 只在备份**真的**做成了之后才有值；读它不会清掉它，界面可以每帧问一次。
pub fn last_recovery() -> Option<PathBuf> {
    LAST_RECOVERY.lock().ok().and_then(|slot| slot.clone())
}

/// 用户已经看过提示：清掉记录，下一次保全再重新提示。
pub fn clear_recovery() {
    if let Ok(mut slot) = LAST_RECOVERY.lock() {
        *slot = None;
    }
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
            "windterm-prefs-{}-{}-{}",
            std::process::id(),
            nanos,
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(PREFS_FILE)
    }

    #[test]
    fn a_language_choice_round_trips() {
        let path = tmp();
        let prefs = Prefs {
            language: "zh-CN".to_string(),
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        assert_eq!(load_from(&path), Some(prefs));
    }

    /// 2026-11-17 Tunnels：设置对话框按 `Ok` 写的就是这两个字段。规则本身要
    /// 逐字段过一遍 JSON（类型 / 端口 / 两个勾选都不能丢），而总线开关是
    /// **独立**的一格——关掉不等于把规则删了，这正是「停转发不删配置」的形态。
    #[test]
    fn tunnel_profiles_round_trip_and_the_bus_switch_is_separate() {
        use term_core::tunnel::{TunnelKind, TunnelProfile};

        let mut local = TunnelProfile::new("pg");
        local.kind = TunnelKind::Local;
        local.session = "prod".into();
        local.listen_port = 5432;
        local.dest_host = "db.internal".into();
        local.dest_port = 5432;
        local.status = true;
        local.auto_open = true;
        let mut socks = TunnelProfile::new("socks");
        socks.kind = TunnelKind::Socks5;
        socks.session = "prod".into();
        socks.listen_port = 1080;
        socks.local_only = false;
        socks.status = false;

        let path = tmp();
        let prefs = Prefs {
            tunnels: vec![local, socks],
            tunnels_enabled: false,
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).expect("刚写下的文件必须读得回来");
        assert_eq!(back.tunnels, prefs.tunnels);
        assert!(
            !back.tunnels_enabled,
            "总线开关关着要读回 false，不能当成缺键回落到 true"
        );
        assert_eq!(back.tunnels.len(), 2, "关掉开关不删规则");
    }

    /// 老 `prefs.json`（这一刀之前写的）没有 `tunnels` / `tunnels_enabled`
    /// 两个键：必须读成「没有规则 + 总线开着」，不能整份作废，也不能把
    /// 出厂状态读成「关着」——那会让升级后的用户以为隧道坏了。
    #[test]
    fn an_older_file_without_tunnels_reads_as_none_and_the_bus_on() {
        let path = tmp();
        std::fs::write(&path, "{}").unwrap();
        let back = load_from(&path).expect("空对象应可反序列化");
        assert!(back.tunnels.is_empty());
        assert!(back.tunnels_enabled);
    }

    #[test]
    fn suppress_flags_round_trip_independently() {
        // 两个抑制位互不牵连：只开窗口那个，会话那个必须还是 false。
        let path = tmp();
        let prefs = Prefs {
            language: "en".to_string(),
            suppress_close_window: true,
            suppress_close_session: false,
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).unwrap();
        assert!(back.suppress_close_window);
        assert!(!back.suppress_close_session);
        assert_eq!(back.language, "en");
    }

    #[test]
    fn an_older_file_without_the_key_still_loads() {
        // 老文件 / 手改过的文件少一个键：必须回落成默认值，不能整份作废。
        // `command_history_save` 的手写 `Default` 与 serde 默认一致（都是开），
        // 逐字段断言是为了守住这一点。
        let path = tmp();
        std::fs::write(&path, "{}").unwrap();
        let back = load_from(&path).expect("空对象应可反序列化");
        assert_eq!(back.language, "");
        assert!(!back.suppress_close_window);
        assert_eq!(
            back.command_history_save,
            Prefs::default().command_history_save
        );
        assert!(back.command_history_save, "Save history 出厂默认开");
        assert_eq!(back.command_history_days, 0);
        assert_eq!(back.command_history_size, 0);
    }

    /// 老 `prefs.json` 没有 `theme_name` 时不能读到空串：空串会绕过三套内置
    /// 整窗主题的查找，拿到的调色板连主题名都是空的。serde 缺省必须与
    /// `Prefs::default()` 逐字相同。
    #[test]
    fn an_older_file_without_theme_name_gets_the_factory_theme() {
        let path = tmp();
        std::fs::write(&path, r#"{"language":"en"}"#).unwrap();
        let back = load_from(&path).expect("缺键不该让整份文件读不出来");
        assert_eq!(back.theme_name, term_core::theme::DEFAULT_THEME_NAME);
        assert_eq!(back.theme_name, Prefs::default().theme_name);
        assert_eq!(back.language, "en", "其余字段不许被连坐");
    }

    /// 显式写空串是用户手改的结果，不该被 migration 改写成出厂主题——
    /// 「缺键」与「写了键」在磁盘上仍可区分。
    #[test]
    fn an_explicit_empty_theme_name_is_preserved() {
        let path = tmp();
        std::fs::write(&path, r#"{"theme_name":""}"#).unwrap();
        assert_eq!(load_from(&path).unwrap().theme_name, "");
    }

    #[test]
    fn startup_prefs_round_trip_without_touching_other_fields() {
        let path = tmp();
        let prefs = Prefs {
            language: "zh-CN".to_string(),
            suppress_close_window: true,
            startup_mode: "specific".to_string(),
            startup_sessions: ["db".into(), String::new(), "web".into(), String::new()],
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).unwrap();
        assert_eq!(back.startup_mode, "specific");
        assert_eq!(back.startup_sessions, prefs.startup_sessions);
        assert_eq!(back.language, "zh-CN");
        assert!(back.suppress_close_window);
    }

    #[test]
    fn an_old_prefs_file_defaults_to_the_factory_startup_mode() {
        // 空串由 term_app::startup::parse_tag 解释成 LastSessions；这里只守
        // 「缺键确实落成空串」，不把另一层的判定抄进来。
        let path = tmp();
        std::fs::write(&path, r#"{"language":"en"}"#).unwrap();
        let back = load_from(&path).unwrap();
        assert_eq!(back.startup_mode, "");
        assert!(back.startup_sessions.iter().all(String::is_empty));
        assert_eq!(back.language, "en");
    }

    #[test]
    fn a_hand_edited_session_list_of_the_wrong_length_does_not_kill_the_file() {
        let path = tmp();
        std::fs::write(
            &path,
            r#"{"language":"zh-CN","startup_sessions":["db","web"]}"#,
        )
        .unwrap();
        let back = load_from(&path).expect("两个元素也该读得出来");
        assert_eq!(back.language, "zh-CN", "其余字段不许被连坐");
        assert_eq!(back.startup_sessions[0], "db");
        assert_eq!(back.startup_sessions[1], "web");
        assert!(back.startup_sessions[2].is_empty());
        assert!(back.startup_sessions[3].is_empty());
    }

    #[test]
    fn a_broken_file_is_none_not_a_panic() {
        let path = tmp();
        std::fs::write(&path, "not json at all").unwrap();
        assert_eq!(load_from(&path), None);
    }

    #[test]
    fn a_missing_file_is_none() {
        assert_eq!(load_from(&tmp().with_file_name("nope.json")), None);
    }

    #[test]
    fn command_history_fields_round_trip() {
        let path = tmp();
        let prefs = Prefs {
            command_history_days: 7,
            command_history_size: 500,
            command_history_save: true,
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).unwrap();
        assert_eq!(back.command_history_days, 7);
        assert_eq!(back.command_history_size, 500);
        assert!(back.command_history_save);
    }

    #[test]
    fn command_history_store_applies_factory_defaults_for_zero() {
        // 老文件 / 未设置 → 0 哨兵，回落出厂默认 30 天 / 10000 条。
        let prefs = Prefs::default();
        let mut store = prefs.command_history_store();
        for i in 0..20 {
            store.push(&format!("cmd{i}"), i as u64);
        }
        // 容量 10000 远大于 20，不应裁剪；TTL 默认 30 天、now 远小于，也不裁剪。
        assert_eq!(store.len(), 20);
    }

    #[test]
    fn command_history_store_honors_explicit_capacity() {
        let prefs = Prefs {
            command_history_size: 3,
            ..Prefs::default()
        };
        let mut store = prefs.command_history_store();
        for i in 0..5 {
            store.push(&format!("cmd{i}"), i as u64);
        }
        assert_eq!(store.len(), 3, "显式容量应覆盖出厂默认");
        assert_eq!(store.recent(10), vec!["cmd4", "cmd3", "cmd2"]);
    }

    #[test]
    fn caret_style_round_trips_without_touching_other_fields() {        // W6：Caret 页的「改选即落盘」是 load-modify-save，只能动这一个字段。
        // 语言、抑制位、启动策略、历史容量都得原样留着。
        let path = tmp();
        let prefs = Prefs {
            language: "zh-CN".to_string(),
            suppress_close_window: true,
            startup_mode: "specific".to_string(),
            command_history_size: 500,
            caret_style: 5,
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).unwrap();
        assert_eq!(back.caret_style, 5);
        assert_eq!(back.language, "zh-CN");
        assert!(back.suppress_close_window);
        assert_eq!(back.startup_mode, "specific");
        assert_eq!(back.command_history_size, 500);
    }

    #[test]
    fn an_old_prefs_file_without_caret_style_falls_back_to_block() {
        // 老文件缺键 → 0 = Block（出厂态），且不会把同文件里的其它设置连坐掉。
        let path = tmp();
        std::fs::write(&path, r#"{"language":"en"}"#).unwrap();
        let back = load_from(&path).expect("缺键不该让整份文件读不出来");
        assert_eq!(back.caret_style, 0);
        assert_eq!(back.caret_style_index(), 0);
        assert_eq!(back.language, "en");
    }

    #[test]
    fn an_out_of_range_caret_style_reads_as_block() {
        // 手改文件写了个 9：读取方拿到 Block，而不是 panic 或越界索引。
        let prefs = Prefs {
            caret_style: 9,
            ..Prefs::default()
        };
        assert_eq!(prefs.caret_style_index(), 0);
    }

    #[test]
    fn caret_period_and_width_round_trip_on_their_own() {
        // Caret 页把 Period / Width 从灰文本改成真 DragValue，落盘同样是
        // load-modify-save——拖这两根滑条不许把光标样式或语言改回默认值。
        let path = tmp();
        let prefs = Prefs {
            language: "zh-CN".to_string(),
            caret_style: 5,
            caret_period: 1.25,
            caret_width: 3.0,
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).unwrap();
        assert_eq!(back.caret_period, 1.25);
        assert_eq!(back.caret_width, 3.0);
        assert_eq!(back.caret_style, 5, "改周期不许顺手改形状");
        assert_eq!(back.language, "zh-CN");
    }

    #[test]
    fn an_old_prefs_file_without_caret_timing_keeps_todays_caret() {
        // 老文件没有这两个键：必须回落到「与改动前逐帧等价」的出厂值，
        // 而不是 `f32` 的 `0.0`（那会把光标闪成永远熄灭、宽度闪成 0 像素）。
        let path = tmp();
        std::fs::write(&path, r#"{"language":"en"}"#).unwrap();
        let back = load_from(&path).expect("缺键不该让整份文件读不出来");
        assert_eq!(back.caret_period, term_core::config::DEFAULT_CARET_PERIOD);
        assert_eq!(back.caret_width, term_core::config::DEFAULT_CARET_WIDTH);
        assert_eq!(back.language, "en");
    }

    #[test]
    fn hand_edited_caret_timing_reads_back_inside_the_range() {
        // 手改文件写了 0 与 99：读取方拿到区间内的值，渲染层不必自己再防一次。
        let prefs = Prefs {
            caret_period: 0.0,
            caret_width: 99.0,
            ..Prefs::default()
        };
        assert_eq!(
            prefs.caret_period_value(),
            *term_core::config::CARET_PERIOD_RANGE.start()
        );
        assert_eq!(
            prefs.caret_width_value(),
            *term_core::config::CARET_WIDTH_RANGE.end()
        );
    }

    #[test]
    fn an_empty_search_url_falls_back_to_the_factory_template() {
        // 没设置过（空串）与只有空白都算「没设置」——设置页与网搜读到同一条。
        let mut prefs = Prefs::default();
        assert_eq!(prefs.search_url_template(), crate::search::default_url());
        prefs.search_url = "   ".to_string();
        assert_eq!(prefs.search_url_template(), crate::search::default_url());
    }

    #[test]
    fn a_custom_search_url_is_used_verbatim() {
        let prefs = Prefs {
            search_url: "https://example.com/?q={}&lang=en".to_string(),
            ..Prefs::default()
        };
        assert_eq!(
            prefs.search_url_template(),
            "https://example.com/?q={}&lang=en"
        );
        // 生效值就是拼 URL 时用的那一条：不是被吞回出厂模板。
        assert_eq!(
            crate::search::build_url(prefs.search_url_template(), "a b"),
            "https://example.com/?q=a%20b&lang=en"
        );
    }

    #[test]
    fn the_new_fields_do_not_disturb_the_zero_sentinels() {
        // 老文件里只有新加的 search_url：历史容量的 `0` 仍是哨兵、光标仍是 Block、
        // 保存历史仍是出厂的开。加字段不该让任何既有默认值跟着变。
        let path = tmp();
        std::fs::write(&path, r#"{"search_url":"https://example.com/?q={}"}"#).unwrap();
        let back = load_from(&path).expect("缺键不该让整份文件读不出来");
        assert_eq!(back.search_url, "https://example.com/?q={}");
        assert_eq!(back.history_capacity(), 10000);
        assert_eq!(back.history_ttl_days(), 30);
        assert_eq!(back.caret_style_index(), 0);
        assert!(back.command_history_save);
        assert!(!back.filer_show_hidden);
    }

    #[test]
    fn the_filer_hidden_switch_round_trips_and_defaults_off() {
        // 出厂不显示隐藏项（视觉基线是空勾）；勾上之后必须能从文件读回来，
        // 否则「面板重建后开关自己弹回去」。
        assert!(!Prefs::default().filer_show_hidden);
        let path = tmp();
        let prefs = Prefs {
            filer_show_hidden: true,
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        assert!(load_from(&path).unwrap().filer_show_hidden);
    }

    #[test]
    fn the_partially_wired_switches_default_to_todays_behaviour() {
        // 三个新开关里有两个是「加了新效果」，出厂必须关着——否则升级一次，
        // 终端外观与光标行为就自己变了（整窗基线也会一起红）。
        let prefs = Prefs::default();
        assert!(!prefs.text_alternate_row_bg);
        assert!(!prefs.text_hide_cursor_while_typing);
        // `focus_new_tab` 相反：现状就是新标签抢焦点，出厂必须是 `true`。
        assert!(prefs.focus_new_tab, "新标签默认切过去，这是现状");
        // 老文件缺这三个键时走 serde 默认，必须与 `Default` 一模一样：
        // 两边不一致会让「没有 prefs.json」与「文件里缺这一项」变成两种行为。
        let path = tmp();
        std::fs::write(&path, r#"{"language":"en"}"#).unwrap();
        let back = load_from(&path).expect("缺键不该让整份文件读不出来");
        assert!(!back.text_alternate_row_bg);
        assert!(!back.text_hide_cursor_while_typing);
        assert!(back.focus_new_tab, "老文件缺键要沿用现状");
        assert_eq!(back.language, "en", "其余字段不许被连坐");
    }

    #[test]
    fn the_three_new_switches_round_trip_on_their_own() {
        // 设置页是 load-modify-save：写这三个位不能顺手把光标样式 / 搜索模板
        // / 历史容量改回默认值。
        let path = tmp();
        let prefs = Prefs {
            text_alternate_row_bg: true,
            text_hide_cursor_while_typing: true,
            focus_new_tab: false,
            caret_style: 5,
            search_url: "https://example.com/?q={}".to_string(),
            command_history_size: 500,
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).unwrap();
        assert!(back.text_alternate_row_bg);
        assert!(back.text_hide_cursor_while_typing);
        assert!(!back.focus_new_tab);
        assert_eq!(back.caret_style, 5);
        assert_eq!(back.search_url, "https://example.com/?q={}");
        assert_eq!(back.command_history_size, 500);
    }

    #[test]
    fn a_zero_font_size_means_the_factory_size_not_zero_points() {
        // 老文件 / 从没拖过 → 0.0。把它当字号画出来是零号字，终端会整片空白；
        // 生效值必须回到出厂 13pt（就是 `fonts.rs` 里 `TERMINAL_FONT_SIZE` 那一份）。
        let prefs = Prefs::default();
        assert_eq!(prefs.font_size, 0.0);
        assert_eq!(
            prefs.font_size_value(),
            term_core::config::DEFAULT_FONT_SIZE
        );
        assert_eq!(prefs.font_size_value(), 13.0);
    }

    #[test]
    fn an_out_of_range_font_size_is_clamped_to_the_renderable_range() {
        // 手改文件写出来的 3 / 200：页面与渲染层都要看到同一个夹过的值，
        // 否则会出现「设置页显示 200、终端画 48」这种两套真相。
        let mut prefs = Prefs {
            font_size: 3.0,
            ..Prefs::default()
        };
        assert_eq!(prefs.font_size_value(), FONT_SIZE_MIN);
        prefs.font_size = 200.0;
        assert_eq!(prefs.font_size_value(), FONT_SIZE_MAX);
        // 区间内的值原样生效——不是被吞回出厂 13。
        prefs.font_size = 18.0;
        assert_eq!(prefs.font_size_value(), 18.0);
    }

    #[test]
    fn the_font_size_round_trips_without_touching_other_fields() {
        let path = tmp();
        let prefs = Prefs {
            font_size: 18.0,
            caret_style: 5,
            language: "zh-CN".to_string(),
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).unwrap();
        assert_eq!(back.font_size, 18.0);
        assert_eq!(back.font_size_value(), 18.0);
        assert_eq!(back.caret_style, 5, "load-modify-save 不许连坐别的字段");
        assert_eq!(back.language, "zh-CN");
    }

    #[test]
    fn an_unset_font_weight_means_regular_and_out_of_range_is_clamped() {
        // 与字号同一套 0 哨兵：`0` 是「老文件没这项」，不是 0 号字重。
        let mut prefs = Prefs::default();
        assert_eq!(prefs.font_weight, 0);
        assert_eq!(prefs.font_weight_value(), 400);
        prefs.font_weight = 700;
        assert_eq!(prefs.font_weight_value(), 700, "合法档位原样生效");
        prefs.font_weight = 50;
        assert_eq!(prefs.font_weight_value(), 100, "低于 Thin 的夹回下限");
        prefs.font_weight = 5000;
        assert_eq!(prefs.font_weight_value(), 900, "高于 Black 的夹回上限");
    }

    #[test]
    fn an_empty_font_family_means_automatic_and_names_are_trimmed() {
        // 空串 = 自动（内置候选），所以「没设过」与「设成空」是同一件事。
        // 空白名字不 trim 的话，「Menlo 」会变成一个查不到的家族名，
        // 用户看到的是「字族没装」——一个只差一个空格的悬案。
        let mut prefs = Prefs::default();
        assert_eq!(prefs.font_family_value(), ["", "", "", ""].map(String::from));
        prefs.font_family = [
            "  Menlo  ".to_string(),
            "Hiragino Sans GB".to_string(),
            String::new(),
            " ".to_string(),
        ];
        let v = prefs.font_family_value();
        assert_eq!(v[0], "Menlo");
        assert_eq!(v[1], "Hiragino Sans GB");
        assert!(v[2].is_empty());
        assert!(v[3].is_empty(), "只有一个空格的名字等于没填");
    }

    #[test]
    fn the_font_family_and_weight_round_trip() {
        let path = tmp();
        let prefs = Prefs {
            font_family: [
                "JetBrains Mono".to_string(),
                "Hiragino Sans GB".to_string(),
                String::new(),
                String::new(),
            ],
            font_weight: 600,
            font_size: 15.0,
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).unwrap();
        assert_eq!(back.font_family[0], "JetBrains Mono");
        assert_eq!(back.font_family[1], "Hiragino Sans GB");
        assert_eq!(back.font_family[2], "");
        assert_eq!(back.font_weight_value(), 600);
        assert_eq!(back.font_size_value(), 15.0, "字号不许被字族写入连坐");
    }

    #[test]
    fn an_old_file_without_the_font_keys_still_loads() {
        // 只写了字号的旧 prefs.json：新加的两个键必须走 serde 默认，
        // 而不是让整份文件读不出来（那会触发「另存坏文件」的保护路径）。
        let path = tmp();
        std::fs::write(&path, r#"{"language":"zh-CN","font_size":16.0}"#).unwrap();
        let back = load_from(&path).expect("旧文件仍要能读");
        assert_eq!(back.font_family_value(), ["", "", "", ""].map(String::from));
        assert_eq!(back.font_weight_value(), 400);
        assert_eq!(back.font_size_value(), 16.0);
    }

    #[test]
    fn the_tab_prefs_default_to_todays_behaviour() {
        // 六个字段的默认值都必须是**现状**：关标签夹到末位、双击没有动作、
        // 标题不省略、不编号、中键无事、右键弹菜单。改默认值等于升级一次
        // 就把用户的标签栏换了个样子。
        let prefs = Prefs::default();
        assert_eq!(prefs.tab_activate_after_close, 0);
        assert_eq!(prefs.tab_double_click, 0);
        assert_eq!(prefs.tab_elide, 0);
        assert_eq!(prefs.tab_numbers, 0);
        assert_eq!(prefs.tab_middle_click, 0);
        assert_eq!(prefs.tab_right_click, 0);
        // 老文件缺这三个键：serde 默认与 `Default` 必须一致，否则「没有
        // prefs.json」与「文件里缺这一项」会走进两种行为。
        let path = tmp();
        std::fs::write(&path, r#"{"language":"en"}"#).unwrap();
        let back = load_from(&path).expect("缺键不该让整份文件读不出来");
        assert_eq!(back.tab_activate_after_close_kind(), 0);
        assert_eq!(back.tab_double_click_action(), 0);
        assert_eq!(back.tab_elide_mode(), 0);
        assert_eq!(back.tab_number_mode(), 0);
        assert_eq!(back.tab_middle_click_action(), 0);
        assert_eq!(back.tab_right_click_action(), 0);
        // `show_address_bar` 相反：出厂就是显示，缺键也必须是显示（否则
        // 升级一次地址栏就消失了）。
        assert!(Prefs::default().show_address_bar, "地址栏面板出厂显示");
        assert!(back.show_address_bar, "老文件缺这个键 = 沿用出厂态");
        assert!(!Prefs::default().auto_hide_menu_bar, "菜单栏出厂常驻");
        assert!(!back.auto_hide_menu_bar, "老文件缺这个键 = 菜单栏仍常驻");
        assert_eq!(Prefs::default().lock_timeout_secs, 1800, "默认 30 分钟");
        assert_eq!(back.lock_timeout_secs, 1800, "老文件缺键 = 文档默认 30 分钟");
        assert!(!Prefs::default().auto_reconnect, "出厂不自动重连");
        assert!(!back.auto_reconnect, "老文件缺键 = 仍不自动重连");
        assert_eq!(back.language, "en", "其余字段不许被连坐");
    }

    #[test]
    fn an_out_of_range_tab_index_reads_as_the_current_behaviour() {
        // 手改文件写了 9：读取方拿到「下一个 / 没有动作 / 不省略 / 不编号 /
        // 没有动作 / 标签菜单」，而不是 panic 或越界下标去索引标签列表。
        let prefs = Prefs {
            tab_activate_after_close: 9,
            tab_double_click: 9,
            tab_elide: 9,
            tab_numbers: 9,
            tab_middle_click: 9,
            tab_right_click: 9,
            ..Prefs::default()
        };
        assert_eq!(prefs.tab_activate_after_close_kind(), 0);
        assert_eq!(prefs.tab_double_click_action(), 0);
        assert_eq!(prefs.tab_elide_mode(), 0);
        assert_eq!(prefs.tab_number_mode(), 0);
        assert_eq!(prefs.tab_middle_click_action(), 0);
        assert_eq!(prefs.tab_right_click_action(), 0);
    }

    #[test]
    fn the_tab_prefs_round_trip_on_their_own() {
        let path = tmp();
        let prefs = Prefs {
            tab_activate_after_close: 2,
            tab_double_click: 1,
            tab_elide: 3,
            tab_numbers: 2,
            tab_middle_click: 1,
            tab_right_click: 2,
            font_size: 17.5,
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).unwrap();
        assert_eq!(back.tab_activate_after_close_kind(), 2);
        assert_eq!(back.tab_double_click_action(), 1);
        assert_eq!(back.tab_elide_mode(), 3);
        assert_eq!(back.tab_number_mode(), 2);
        assert_eq!(back.tab_middle_click_action(), 1);
        assert_eq!(back.tab_right_click_action(), 2);
        assert_eq!(back.font_size_value(), 17.5);
        assert!(back.focus_new_tab, "出厂 true 的字段不许被写坏");
    }

    // ---- hex 的出厂参数（设置 ▸ Binary 页那四个数 + 斑马线） ----

    #[test]
    fn hex_defaults_round_trip_through_the_file() {
        // 四项都是「默认参数」：设置页写、新会话用。它们必须能原样往返，
        // 否则设置页改完重启就丢，用户会以为那个框是假的。
        let path = tmp();
        let mut prefs = Prefs::default();
        // 缺省 = 0 哨兵 / 斑马线出厂勾上（视觉基线这一格是勾着的）。
        assert_eq!(
            (
                prefs.hex_base,
                prefs.hex_columns,
                prefs.hex_group,
                prefs.hex_division
            ),
            (0, 0, 0, 0),
            "0 = 老文件没这一项，由 HexParams::from_prefs 回落出厂值"
        );
        assert!(prefs.alternate_column_bg, "斑马线出厂是勾上的");
        prefs.hex_base = 2;
        prefs.hex_columns = 24;
        prefs.hex_group = 4;
        prefs.hex_division = 8;
        prefs.alternate_column_bg = false;
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).expect("读回");
        assert_eq!(
            (
                back.hex_base,
                back.hex_columns,
                back.hex_group,
                back.hex_division
            ),
            (2, 24, 4, 8)
        );
        assert!(!back.alternate_column_bg);
    }

    #[test]
    fn a_file_without_the_hex_keys_reads_as_unset_not_as_garbage() {
        // 老 prefs.json 里一个 hex 键都没有：读出来必须是 0 哨兵 + 斑马线默认开，
        // 而不是「随便挑一个数」。这条是升级路径唯一的判据。
        let path = tmp();
        std::fs::write(&path, r#"{"language":"en"}"#).unwrap();
        let back = load_from(&path).expect("缺键不该让整份文件读不出来");
        assert_eq!((back.hex_base, back.hex_columns), (0, 0));
        assert_eq!((back.hex_group, back.hex_division), (0, 0));
        assert!(back.alternate_column_bg, "serde 默认必须与出厂一致");
    }

    #[test]
    fn the_close_and_paste_switches_default_to_todays_behaviour() {
        // 关多标签的确认是**现状**（出厂问），两个粘贴确认是**新效果**
        // （出厂不问）。两边的默认值都不是「随便挑一个」：前者收窄会把用户的
        // 确认网拿掉，后者放宽会让升级一次就开始弹窗。
        let prefs = Prefs::default();
        assert!(prefs.confirm_close_multi_tabs, "出厂就问「关多标签」");
        assert!(!prefs.paste_confirm_main);
        assert!(!prefs.paste_confirm_alt);
        // 老文件缺这三个键：serde 默认必须与 `Default` 一模一样，
        // 否则「没有 prefs.json」与「文件里缺这一项」会走进两种行为。
        let path = tmp();
        std::fs::write(&path, r#"{"language":"en"}"#).unwrap();
        let back = load_from(&path).expect("缺键不该让整份文件读不出来");
        assert!(back.confirm_close_multi_tabs, "老文件缺键要沿用现状（问）");
        assert!(!back.paste_confirm_main);
        assert!(!back.paste_confirm_alt);
        assert_eq!(back.language, "en", "其余字段不许被连坐");
    }

    #[test]
    fn the_close_and_paste_switches_round_trip_on_their_own() {
        // 设置页是 load-modify-save：写这三个位不能顺手把别的开关改回默认值。
        let path = tmp();
        let prefs = Prefs {
            confirm_close_multi_tabs: false,
            paste_confirm_main: true,
            paste_confirm_alt: true,
            focus_new_tab: false,
            tab_elide: 2,
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).unwrap();
        assert!(!back.confirm_close_multi_tabs);
        assert!(back.paste_confirm_main);
        assert!(back.paste_confirm_alt);
        assert!(!back.focus_new_tab);
        assert_eq!(back.tab_elide_mode(), 2);
    }

    #[test]
    fn the_quickbar_switches_default_to_todays_behaviour() {
        // 三个开关出厂都是「现状」：多组 + 组标题显示、不字母序。改默认值等于
        // 升级一次就把底栏变样，所以必须守住。
        let prefs = Prefs::default();
        assert!(prefs.quickbar_show_multiple_groups, "出厂显示多组");
        assert!(prefs.quickbar_show_group_title, "出厂显示组标题");
        assert!(!prefs.quickbar_alphabetical, "出厂不按字母序，守住视觉基线底栏");
        // 老文件缺这三个键：serde 默认与 `Default` 必须一致，否则「没有
        // prefs.json」与「文件里缺这一项」会走进两种行为。
        let path = tmp();
        std::fs::write(&path, r#"{"language":"en"}"#).unwrap();
        let back = load_from(&path).expect("缺键不该让整份文件读不出来");
        assert!(back.quickbar_show_multiple_groups);
        assert!(back.quickbar_show_group_title);
        assert!(!back.quickbar_alphabetical);
        assert_eq!(back.language, "en", "其余字段不许被连坐");
        // 派生出的渲染开关也必须等于出厂态。
        assert_eq!(back.quick_bar_display(), QuickBarDisplay::default());
    }

    #[test]
    fn the_quickbar_switches_round_trip_on_their_own() {
        // 设置页是 load-modify-save：写这三个位不能顺手把别的开关改回默认值。
        let path = tmp();
        let prefs = Prefs {
            quickbar_show_multiple_groups: false,
            quickbar_show_group_title: false,
            quickbar_alphabetical: true,
            focus_new_tab: false,
            tab_elide: 2,
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).unwrap();
        assert!(!back.quickbar_show_multiple_groups);
        assert!(!back.quickbar_show_group_title);
        assert!(back.quickbar_alphabetical);
        assert!(!back.focus_new_tab, "出厂 true 的字段不许被写坏");
        assert_eq!(back.tab_elide_mode(), 2);
        let d = back.quick_bar_display();
        assert!(!d.show_multiple_groups && !d.show_group_title && d.alphabetical);
    }

    // ---- 坏文件不许被静默覆盖（本仓唯一一条「写一下就不可逆丢数据」的路） ----
    //
    // 这几条是回归用例，不是既有行为的描述：把 `save_to` 开头那句
    // `save_to_checked` 里的坏文件保全去掉，第一条会以「备份 0 份」失败 —— 那正是
    // 修复之前用户丢掉整份 `prefs.json` 的时刻。

    #[test]
    fn a_broken_file_is_copied_aside_before_the_new_one_lands() {
        // 用户手改 `prefs.json` 时少写了一个逗号，然后去设置页点了一个开关：
        // `load()` 给出 `None` → 设置页用出厂值 → `save()`。原文件必须还在。
        let path = tmp();
        let broken = "{\n  \"language\": \"zh-CN\"\n  \"font_size\": 18.0\n}\n";
        std::fs::write(&path, broken).unwrap();
        assert_eq!(load_from(&path), None, "前提：这份文件确实读不出来");
        let prefs = Prefs {
            font_size: 18.0,
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let backups = existing_backups(&path);
        assert_eq!(backups.len(), 1, "坏文件必须留一份备份，实际 {backups:?}");
        assert_eq!(
            std::fs::read_to_string(&backups[0]).unwrap(),
            broken,
            "备份必须是用户原来那份字节，不是我们重写过的"
        );
        assert_eq!(load_from(&path), Some(prefs), "新文件照常写进去");
    }

    #[test]
    fn a_non_utf8_prefs_file_is_preserved_too() {
        // 「文件在但读不出来」不止 JSON 坏：非 UTF-8 的字节也一样，而这一种
        // 连 `read_to_string` 都失败（`Unreadable` 而不是 `Malformed`）。
        // 备份因此必须走字节拷贝，不能先解码再存。
        let path = tmp();
        let raw: [u8; 5] = [b'{', b'"', 0xFF, b'"', b'}'];
        std::fs::write(&path, raw).unwrap();
        assert_eq!(
            read_disk(&path).1,
            Err(LoadError::Unreadable),
            "前提：非 UTF-8 走的是「读不出来」这一支"
        );
        assert!(save_to(&path, &Prefs::default()));
        let backups = existing_backups(&path);
        assert_eq!(backups.len(), 1, "实际 {backups:?}");
        assert_eq!(std::fs::read(&backups[0]).unwrap(), raw);
    }

    #[test]
    fn a_read_failure_is_not_folded_into_a_missing_file() {
        // 三种失败必须分得开：混成一种，写路径就只能二选一 —— 要么把
        // 「文件坏了」当成「没有文件」照写（丢数据），要么把「没有文件」当成
        // 「文件坏了」拒写（第一次保存就永远存不下来）。
        let missing = tmp().with_file_name("nope.json");
        assert_eq!(read_disk(&missing).1, Err(LoadError::Missing));
        let broken = tmp();
        std::fs::write(&broken, "not json at all").unwrap();
        assert_eq!(read_disk(&broken).1, Err(LoadError::Malformed));
        let unreadable = tmp();
        std::fs::write(&unreadable, [0xFF_u8, 0xFE]).unwrap();
        assert_eq!(read_disk(&unreadable).1, Err(LoadError::Unreadable));
    }

    #[test]
    fn a_fresh_profile_gets_no_backup_file() {
        // 从没写过 prefs 的用户不该在第一次保存时看到一堆 `.bad-*`。
        let path = tmp();
        assert!(!path.exists());
        assert!(save_to(&path, &Prefs::default()));
        assert!(
            existing_backups(&path).is_empty(),
            "没有旧文件就没有可保全的东西"
        );
    }

    #[test]
    fn the_same_broken_content_is_kept_once_not_once_per_click() {
        // 设置页每点一个开关都是一次 load-modify-save。反复失败时若每次都
        // 另存，用户的 profile 目录会被同名备份铺满——那是垃圾场，不是保全。
        let path = tmp();
        std::fs::write(&path, "{ oops").unwrap();
        let first = preserve_broken(&path).expect("第一份备份");
        let second = preserve_broken(&path).expect("第二次要能指出同一份");
        assert_eq!(first, second, "同一份内容只留一份，不许另起");
        assert_eq!(existing_backups(&path).len(), 1);
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "{ oops");
    }

    #[test]
    fn the_kept_copy_is_reported_so_the_ui_can_say_where_it_went() {
        // 数据还在盘上但用户不知道，在体验上和丢了是同一件事。保全成功时
        // 留一条记录，设置页据此画一行提示。
        let path = tmp();
        std::fs::write(&path, "{ broken").unwrap();
        assert!(save_to(&path, &Prefs::default()));
        let reported = last_recovery().expect("保全之后必须能报出备份路径");
        assert!(reported.is_file(), "报出去的路径必须真的存在");
        assert_eq!(std::fs::read_to_string(&reported).unwrap(), "{ broken");
    }

    /// Reset 必须清掉旧下标字段；否则重启时迁移会把它重新写成新绑定。
    #[test]
    fn resetting_shortcuts_clears_both_the_new_and_the_legacy_field() {
        let mut prefs = Prefs {
            shortcut_bindings: vec![("Text.Find".into(), "Cmd+F".into())],
            shortcut_overrides: vec![(3, "Cmd+K".into())],
            ..Prefs::default()
        };
        prefs.reset_shortcut_bindings();
        assert!(prefs.shortcut_bindings.is_empty());
        assert!(prefs.shortcut_overrides.is_empty());
    }

    #[test]
    fn shortcut_bindings_by_command_id_survive_a_round_trip() {
        // 新格式以命令 ID 为键：出厂表增删项不再让用户的重绑漂移。
        let path = tmp();
        let prefs = Prefs {
            shortcut_bindings: vec![
                ("Text.Find".to_string(), "Cmd+Shift+F".to_string()),
                ("Text.Paste".to_string(), "Cmd+V".to_string()),
            ],
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).expect("读回");
        assert_eq!(back.shortcut_bindings, prefs.shortcut_bindings);
    }

    #[test]
    fn shortcut_overrides_round_trip_and_default_to_empty() {
        // 重绑定的快捷键必须能从 prefs 读回来，否则「改完重启就丢」，
        // 用户会以为那个编辑器是假的。出厂态是空覆盖集（全部用出厂和弦）。
        assert!(Prefs::default().shortcut_overrides.is_empty());
        let path = tmp();
        let prefs = Prefs {
            shortcut_overrides: vec![(3usize, "Cmd+K".to_string()), (7usize, "Alt+H".to_string())],
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).expect("读回");
        assert_eq!(
            back.shortcut_overrides,
            vec![(3usize, "Cmd+K".to_string()), (7usize, "Alt+H".to_string())]
        );
        // 落盘形态必须是 `[[3,"Cmd+K"],[7,"Alt+H"]]`（顺序稳定、可被下游直接读）。
        // serde 走漂亮打印（带换行 / 缩进），先去掉所有空白再比对，避开格式噪声；
        // 顺序之所以稳定，是因为 `shortcut_overrides` 是 `Vec` 而不是 `HashMap`
        // —— 序列化顺序由写入顺序决定，不随哈希随机化漂。
        let json = std::fs::read_to_string(&path).unwrap();
        let compact = json.split_whitespace().collect::<String>();
        assert!(
            compact.contains("\"shortcut_overrides\":[[3,\"Cmd+K\"],[7,\"Alt+H\"]]"),
            "{json}"
        );
    }

    #[test]
    fn an_old_prefs_file_without_shortcut_overrides_reads_as_unset() {
        // 老文件缺这个键：必须回落空覆盖集，不连坐其它字段（如语言）。
        let path = tmp();
        std::fs::write(&path, r#"{"language":"en"}"#).unwrap();
        let back = load_from(&path).expect("缺键不该让整份文件读不出来");
        assert!(back.shortcut_overrides.is_empty(), "老文件缺键 = 全出厂绑定");
        assert_eq!(back.language, "en", "其余字段不许被连坐");
    }

    #[test]
    fn an_old_prefs_file_without_recent_targets_reads_as_empty() {
        // 地址栏「最近使用」。老文件根本没有这个键，`#[serde(default)]` 让
        // 它回落成空列表（= 还没用过地址栏），并且不许连坐同文件里的其它设置。
        let path = tmp();
        std::fs::write(&path, r#"{"language":"zh-CN","caret_style":5}"#).unwrap();
        let back = load_from(&path).expect("缺键不该让整份文件读不出来");
        assert!(back.recent_targets.is_empty(), "缺键 = 空列表");
        assert_eq!(back.language, "zh-CN", "其余字段不许被连坐");
        assert_eq!(back.caret_style, 5);
        assert_eq!(Prefs::default().recent_targets, Vec::<String>::new());
    }

    #[test]
    fn recent_targets_round_trip_and_keep_their_mru_order() {
        // MRU 的顺序就是列表顺序（队首 = 最近一条），落盘必须是同一个顺序：
        // 换一次顺序，下拉里「第一行就是上次那台机器」这句话就不成立了。
        let path = tmp();
        let prefs = Prefs {
            recent_targets: vec![
                "ssh://root@10.0.0.5:22".to_string(),
                "user@db:2222".to_string(),
                "shell:///bin/bash".to_string(),
            ],
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).expect("读回");
        assert_eq!(back.recent_targets, prefs.recent_targets);
        assert_eq!(back.recent_targets[0], "ssh://root@10.0.0.5:22", "队首 = 最近");
        // 上限常量与「写者裁剪」的口径一起钉住：常量本身是 20，不是随手写的数。
        assert_eq!(RECENT_TARGETS_CAP, 20);
    }

    #[test]
    fn window_transparency_defaults_to_opaque_and_survives_a_missing_key() {
        // 出厂是不透明窗口（透明关 + 不透明度 1.0）。老文件缺这两个键时
        // 必须回落出厂值，不能因为「角落里多两个字段」就让整窗突然变透明。
        assert!(!Prefs::default().window_transparent, "默认不透明窗口");
        assert_eq!(Prefs::default().window_opacity, 1.0);
        let path = tmp();
        std::fs::write(&path, r#"{"language":"en"}"#).unwrap();
        let back = load_from(&path).expect("缺键不该让整份文件读不出来");
        assert!(!back.window_transparent, "老文件缺键 = 出厂不透明");
        assert_eq!(back.window_opacity, 1.0, "老文件缺键 = 出厂不透明度 1.0");
        assert_eq!(back.language, "en", "其余字段不许被连坐");
        // round-trip：写回去还能读回来。
        let path2 = tmp();
        let prefs = Prefs {
            window_transparent: true,
            window_opacity: 0.6,
            ..Prefs::default()
        };
        assert!(save_to(&path2, &prefs));
        let back2 = load_from(&path2).expect("读回");
        assert!(back2.window_transparent);
        assert_eq!(back2.window_opacity, 0.6);
    }

    /// 钳制边界：下限 / 上限 / 区间内 / 非有限值四档逐项钉死。
    ///
    /// 为什么这里而不是渲染层：钳制是**读取侧的语义**，与 egui 无关，
    /// 放在 `term-profile` 才能让纯测试不拉起任何 UI 依赖。
    #[test]
    fn window_opacity_clamps_into_the_readable_range() {
        assert_eq!(WINDOW_OPACITY_MIN, 0.3);
        assert_eq!(WINDOW_OPACITY_MAX, 1.0);
        assert_eq!(clamp_window_opacity(0.0), 0.3, "0.0 不能让整窗消失");
        assert_eq!(clamp_window_opacity(0.05), 0.3, "下限 0.3");
        assert_eq!(clamp_window_opacity(0.3), 0.3, "下边界本身合法");
        assert_eq!(clamp_window_opacity(0.6), 0.6, "区间内原样返回");
        assert_eq!(clamp_window_opacity(1.0), 1.0, "上边界本身合法");
        assert_eq!(clamp_window_opacity(4.0), 1.0, "上限 1.0");
        assert_eq!(clamp_window_opacity(f32::NAN), 1.0, "坏值按出厂不透明兜底");
        assert_eq!(clamp_window_opacity(f32::INFINITY), 1.0);
        assert_eq!(clamp_window_opacity(f32::NEG_INFINITY), 1.0);
    }

    /// 钳制**不改落盘值**：手改过的 `prefs.json` 里写一个 0.05，读回来仍是
    /// 0.05（渲染侧用夹过的 0.3 画），写回去也还是 0.05。否则一次设置页的
    /// 「保存」就会把用户自己写的值悄悄抹掉。
    #[test]
    fn clamping_never_rewrites_the_saved_value() {
        let mut prefs = Prefs {
            window_transparent: true,
            window_opacity: 0.05,
            ..Prefs::default()
        };
        assert_eq!(clamp_window_opacity(prefs.window_opacity), 0.3);
        assert_eq!(prefs.window_opacity, 0.05, "钳制只产出返回值，不写回字段");
        let path = tmp();
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).expect("读回");
        assert_eq!(back.window_opacity, 0.05, "往返后仍是用户写的那个值");
        prefs.window_opacity = clamp_window_opacity(prefs.window_opacity);
        assert_eq!(prefs.window_opacity, 0.3, "只有显式赋值才改");
    }

    #[test]
    fn onboarded_is_false_for_an_older_file_without_the_key() {
        // 老文件里没有 `onboarded`。回落值必须是 `false`（= 还没走过引导），
        // 不是 `true`。差别是这个字段唯一的语义：`true` 会让升级上来的用户
        // **永远看不到**引导，而那正是本刀唯一新增的入口。
        let path = tmp();
        std::fs::write(&path, r#"{"language":"zh-CN","font_size":15.0}"#).unwrap();
        let back = load_from(&path).expect("缺 onboarded 键不该让整份文件读不出来");
        assert!(!back.onboarded, "老文件缺键 = 还没走过引导");
        assert_eq!(back.language, "zh-CN", "其余字段不许被连坐");
        assert_eq!(back.font_size, 15.0, "其余字段不许被连坐");
        // 出厂态同样必须是「没走过」。
        assert!(!Prefs::default().onboarded);
    }

    #[test]
    fn onboarded_round_trips() {
        // 走完 / 跳过之后写回的 `true` 必须能被下一次启动读回来，
        // 否则每次启动都会再弹一次引导——那是比不做引导更烦人的形态。
        let path = tmp();
        let prefs = Prefs {
            onboarded: true,
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        let back = load_from(&path).expect("读回");
        assert!(back.onboarded, "写进去的 true 必须读回来");
        assert_eq!(back, prefs, "往返不该改动任何其它字段");
    }

    /// 空白/符号可视化六项：老文件缺六个键时**逐项**回落到厂值。
    ///
    /// 这条挡的是两种事故：
    ///
    /// 1. 少写一个 `#[serde(default)]` → 整份 `prefs.json` 反序列化失败，
    ///    用户的会话库、语言、日志设置跟着一起丢；
    /// 2. 默认值写成 `true` → 升级上来的用户屏幕当场多出六种标记，而五张视觉
    ///    基线正是靠「出厂全关」成立的。
    #[test]
    fn an_older_file_without_the_marks_keys_falls_back_to_all_off() {
        let path = tmp();
        std::fs::write(&path, r#"{"language":"zh-CN","font_size":15.0}"#).unwrap();
        let back = load_from(&path).expect("缺六个键不该让整份文件读不出来");
        assert!(!back.view_line_endings);
        assert_eq!(back.view_column_marker, 0, "0 = 不画参考列");
        assert!(!back.view_indent_guides);
        assert!(!back.view_wrap_symbols);
        assert_eq!(back.view_whitespace, 0, "0 = Hide All");
        assert!(!back.text_highlight_caret_line);
        // 其余字段不许被连坐。
        assert_eq!(back.language, "zh-CN");
        assert_eq!(back.font_size, 15.0);
        // 出厂态与「缺键」必须是同一种画法，否则删掉 prefs.json 会像功能改动。
        let factory = Prefs::default();
        assert!(!factory.view_line_endings);
        assert_eq!(factory.view_column_marker, 0);
        assert!(!factory.view_indent_guides);
        assert!(!factory.view_wrap_symbols);
        assert_eq!(factory.view_whitespace, 0);
        assert!(!factory.text_highlight_caret_line);
    }

    /// 六项逐个往返：都是 `load-modify-save` 能逐项核对的普通字段。
    ///
    /// 特别是 `view_whitespace`——它是**整数编号**而不是枚举，所以「写 4 读回 4」
    /// 走的是 serde 的默认整数通道；档位名与编号的映射在 `apps/desktop` 的
    /// `WhitespaceMode::index` 一处，与这里存的编号必须同一套。
    #[test]
    fn the_marks_keys_round_trip_independently() {
        let path = tmp();
        let prefs = Prefs {
            view_line_endings: true,
            view_column_marker: 100,
            view_indent_guides: true,
            view_wrap_symbols: true,
            view_whitespace: 4,
            text_highlight_caret_line: true,
            // 这一项出厂是 `true`，所以往返要钉的是**能存成 false**——
            // 默认值方向的那个断言在 `an_older_file_still_highlights_find_matches`。
            text_highlight_search: false,
            ..Prefs::default()
        };
        assert!(save_to(&path, &prefs));
        assert_eq!(load_from(&path).expect("读回"), prefs);
    }

    /// 查找命中高亮：老文件**没有这个键**时必须当「开着」，不能当「关着」。
    ///
    /// 这是本仓唯一一个「出厂 `true` 的渲染开关」——它出厂开不是因为视觉基线
    /// 画着勾，而是因为效果本来就无条件在画（`pane_overlays.rs` 的那段）。
    /// 少写 `#[serde(default = "default_true")]` 的后果是**升级即关**：
    /// 用户会以为「查找高亮坏了」，而代码里没有任何东西会报错。
    #[test]
    fn an_older_file_still_highlights_find_matches() {
        let path = tmp();
        std::fs::write(&path, r#"{"language":"zh-CN","font_size":15.0}"#).unwrap();
        let back = load_from(&path).expect("缺少新键不该让整份文件读不出来");
        assert!(
            back.text_highlight_search,
            "老文件没有这个键 → 保持既有的查找高亮，不许升级即关"
        );
        assert!(Prefs::default().text_highlight_search, "出厂与缺键同一种画法");
        // 其余字段不连坐。
        assert_eq!(back.font_size, 15.0);
    }

    /// 两个进程各自 load 了同一份合法 prefs、各改一个键：后写者不许把先写者
    /// 的键整份盖回去，也不许假装成功。本线程扮演进程 B（先读、后写），
    /// 另一个线程扮演进程 A（中间读改并写）——两份实例各带各的基线，互不干扰。
    #[test]
    fn two_writers_on_different_keys_do_not_silently_regress() {
        let path = tmp();
        let base = Prefs {
            language: "en".to_string(),
            font_size: 12.0,
            ..Prefs::default()
        };
        assert!(save_to(&path, &base));

        // 进程 B 先读到基线（记下此刻的内容指纹）。
        let mut b = load_from(&path).expect("B 读到了基线");
        assert_eq!(b.language, "en");

        // 进程 A 用自己那份快照改 language 并成功写盘。
        let path_a = path.clone();
        let a = std::thread::spawn(move || {
            let mut a = load_from(&path_a).expect("A 读到了同一份基线");
            a.language = "zh-CN".to_string();
            assert!(save_to(&path_a, &a), "A 的基线是最新的，应当写成");
        });
        a.join().expect("A 线程不能 panic");

        // B 改的只是另一个键，但它手里那份 prefs 已经过期：必须拒写并报出
        // 明确原因，不能把 A 的 language 静默回退成 en。
        b.font_size = 20.0;
        assert!(!save_to(&path, &b), "过期基线必须拒写");
        assert_eq!(last_save_error(), Some(SaveError::Conflict));
        assert!(!SaveError::Conflict.message().is_empty(), "UI 要有可显示的原因");
        let disk = load_from(&path).expect("盘上仍是 A 写下的合法偏好");
        assert_eq!(disk.language, "zh-CN", "先写者的键不许被静默回退");
        assert_ne!(disk.font_size, 20.0, "后写者的改动没有偷偷生效");
        assert_eq!(disk.font_size, 12.0);
    }

    /// 冲突是显式错误；成功后错误必须清空，否则 UI 会在下一次正常的
    /// 保存之后还挂着上次的「请重做」提示。
    #[test]
    fn save_error_is_explicit_and_cleared_by_a_success() {
        let path = tmp();
        let prefs = Prefs { language: "en".to_string(), ..Prefs::default() };
        assert!(save_to(&path, &prefs));
        assert_eq!(last_save_error(), None);
        // 外部改了文件（模拟手改 / 另一个进程），本线程的基线过期。
        std::fs::write(&path, r#"{"language":"zh-CN"}"#).unwrap();
        assert!(!save_to(&path, &prefs));
        assert_eq!(last_save_error(), Some(SaveError::Conflict));
        assert_eq!(
            SaveError::Conflict.message(),
            "配置已被其它进程或编辑器改过，请重新打开设置页再改一次"
        );
        // 重新读一次再写：成功，错误清空。
        let fresh = load_from(&path).unwrap();
        assert!(save_to(&path, &fresh));
        assert_eq!(last_save_error(), None);
        clear_last_save_error();
    }

    /// 旧实现的「明明冲突却通过」：基线放在线程本地表里，同线程任何一次
    /// 无关 `load()` 都会把它刷成最新指纹，于是手里那份陈旧快照反而通过
    /// 了冲突检查。判据移到实例上以后这件事不能再发生。
    #[test]
    fn an_unrelated_load_cannot_make_a_stale_snapshot_pass() {
        let path = tmp();
        let base = Prefs { language: "en".to_string(), ..Prefs::default() };
        assert!(save_to(&path, &base));

        let mut stale = load_from(&path).expect("旧快照读到基线");
        // 另一个进程 / 编辑器改了盘上内容。
        std::fs::write(&path, r#"{"language":"zh-CN"}"#).unwrap();
        // 同一线程又读了一次（例如渲染另一页时）：旧表在这里把基线洗白。
        let _fresh = load_from(&path).expect("另一次 load 是无关动作");

        stale.font_size = 20.0;
        assert!(!save_to(&path, &stale), "陈旧快照不得因为同线程另一次 load 而通过");
        assert_eq!(last_save_error(), Some(SaveError::Conflict));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"language":"zh-CN"}"#,
            "被拒的写不能改动盘上内容"
        );
        clear_last_save_error();
    }
}
