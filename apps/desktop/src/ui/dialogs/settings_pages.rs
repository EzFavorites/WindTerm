//! Preferences 对话框里**尚未接线**的那些页：一张数据表 + 一个通用画法。
//!
//! # 为什么是数据表
//!
//! 19 张视觉基线截图里，十六个叶页 + 三个组头页的差别几乎只是「几组控件、
//! 每组的文字和取值」，页面结构是同一套（标题 + 圆角分组框 + 两列栅格）。
//! 把它们逐页写成 `if` 分支，等于把设计稿抄成十六段几乎一样的代码；
//! 写成表之后，**对照截图改数据**就行，画法只有一份。
//!
//! # 全部灰显，这是故意的
//!
//! 这些开关（代理类型、标签行为、光标闪烁、折行模式、补全列表高度……）
//! 在 `term-core` 里还没有对应字段，也没有任何地方读它们。按 I5：
//! **未接线的项必须灰显 + 说明**，不许画成能点却什么都不做（R5）。
//! 每个页面底部那一行小字就是这句说明。
//!
//! 灰显页上的勾选一律是**空框**（`Row::DisabledCheck`）：勾传达「这项设置
//! 已生效」，而它们零读者（S4 的假勾选）。W7 把剩下的 18 行「灰显却打勾」
//! 全改成了空框——Terminal 1（Enable local mode）、Auto Completion 6、
//! Mouse Tracking 5、Text 2、Binary 1、Quick Bar 3；Highlight / Terminal 的
//! 粘贴确认框 / Behaviour / Toolbar 此前已经改过。Quick Bar 那三项 2026-11-19
//! 接成真控件，已搬去 `settings_live_quickbar`，这里只留历史口径。
//!
//! 真接线的页不在这里：
//! - `Application`（两个关闭确认框绑 `confirm.suppress_*`；W1-a 起 Startup
//!   三档 + Session 1–4 绑 `win.startup`，都真的会生效）；
//! - `Appearance`（语言与主题是真的，落 prefs）；
//! - `Font`（2026-11-10 起 `Font size` 是真控件，绑 `prefs.font_size`，
//!   改完当场重装字形；字族 / 字重仍灰显。画法在 `settings_live_font.rs`）；
//! - `Caret`（W6：八种样式绑 `prefs.caret_style`，新建会话按它画光标）；
//! - `Command History`（W7：容量 / TTL / 落盘开关都有读者，改完当场重建
//!   历史表并写回 prefs）；
//! - `Search`（W8：引擎下拉 + 可编辑 URL 模板，落 `prefs.search_url`，
//!   `Text ▸ Search Online` 按它搜；自定义引擎列表仍无存储，故 `+ − ▲ ▼` 灰显）；
//! - `Filer`（W8：`Show hidden items` 绑 `prefs.filer_show_hidden`，
//!   `local_dir::list` 按它过滤，勾完当场重列）；
//! - `Text` / `Behaviour` / `ToolbarMenu`（2026-11-10：**部分真控件**——
//!   每页只有一两行接了字段，其余行仍按本文件的画法灰显。画法在
//!   `settings_live_text.rs`，它同时用 `settings_pages_view::draw_row` 画那些
//!   灰行，所以「灰行长什么样」仍然只有一份实现）；
//! - `Tabs`（2026-11-10：**部分真控件**——关闭后激活 / 双击动作 / 省略模式
//!   三行接了字段，其余五行灰显，逐行写清缺的是什么，见 `settings_live_tabs.rs`）。
//! - `Terminal`（2026-11-19：**整页真控件**——粘贴确认两勾 / 右键三档 /
//!   选中即复制 / 只读查看四项全接，见 `settings_live_terminal.rs`）。
//!
//! # 取值来源
//!
//! 每个 `value` 都是照着截图抄的**出厂态**（`English` / `dige-black` /
//! `Block` / `Wrap by character` …），不是我们编的默认值——所以页面看起来
//! 与原生一致，但控件是灰的，不会假装能改。

use super::settings::SettingsPage;

/// 一个页的全部内容：标题 + 若干分组。
pub(in crate::ui) struct PageSpec {
    pub title: &'static str,
    pub blocks: &'static [Block],
}

/// 页内的一块：分组框、框外的散行、或一张两列表。
pub(in crate::ui) enum Block {
    Group {
        title: &'static str,
        rows: &'static [Row],
    },
    /// 不在分组框里的行（视觉基线里 `Binary` / `Wrap` 都有这种半截控件）。
    Rows(&'static [Row]),
    /// 快捷键表。行不是字面量——要按 `DEFAULT_SHORTCUTS` + 运行期覆盖算出来，
    /// 所以数据表只放这个标记，具体画法在 `settings_pages_view`。
    Shortcuts,
}

/// 一行控件。`label` 为空串时画整宽控件（视觉基线 `Proxy` 页就是这样）。
pub(in crate::ui) enum Row {
    Label(&'static str),
    Check(&'static str, bool),
    /// 「未启用」的勾选行：控件形状照视觉基线，但**永远是空框**。
    ///
    /// W2：`Row::Check(text, true)` 会被画成一个灰显却打勾的框——
    /// 勾传达「这项设置已生效」，而它其实既没落盘也没读者（S4 的假勾选）。
    /// 这一行不假装生效：空框 + 灰显 + 悬停给出本页 `page_note` 的理由。
    /// 等 W4 把它接成真设置（粘贴前弹确认）时，再换回有状态的控件。
    DisabledCheck(&'static str),
    Radios {
        label: &'static str,
        items: &'static [&'static str],
        selected: usize,
    },
    Combo {
        label: &'static str,
        value: &'static str,
    },
    /// 标签在框上方独占一行、控件占满整行宽（视觉基线 Security/Auto Completion
    /// 的画法：带访问字母的标签紧跟其控件，不是「左标签右控件」两列）。
    ComboAbove {
        label: &'static str,
        value: &'static str,
    },
    Spin {
        label: &'static str,
        value: &'static str,
    },
    SpinAbove {
        label: &'static str,
        value: &'static str,
    },
    Text {
        label: &'static str,
        hint: &'static str,
    },
}

pub(in crate::ui) const DISABLED_NOTE: &str = "Not stored yet: these settings come from the reference \
     design and have no field in the config layer. They are shown disabled on purpose.";

// 下面八句是 2026-11-10 逐页写的：**每一句都点名缺的是哪一层**。
//
// 这一批的共同毛病不是「文案不够好看」，而是原来八页共用 `DISABLED_NOTE` 那句
// 「配置层没有字段」。那句话只说了「没字段」，没说缺的是**存储字段**、**状态机**、
// **渲染器**还是**编码通道**——读起来一律像「还没轮到做」。S4 要求灰掉的控件
// 同时交代缺口：用户看到灰控件，必须能判断是「真做不了」还是「还没做」。
// 所以每句都指到代码里的那一处（文件:行），而不是形容词。
//
// 长度上限照 `WRAP_NOTE` / `HIGHLIGHT_NOTE`：说明框在右栏定宽，再长会被截断。

/// `Proxy` 页。缺的是**跨 crate 的两层**，不是「加个字段」：
///
/// - 存储层：`Prefs`（`term-runtime`）里没有 proxy 字段，选中的类型无处可落。
/// - 应用层：`SessionConfig` / `Protocol` 的建连路径没有 proxy 的回落位置，
///   就算存下来也没有一处会把代理塞进连接。两层都缺，所以连「先存着」都做不到。
const PROXY_NOTE: &str = "Not wired: there is no proxy layer at all — no field in Prefs to store the \
     choice, and no fallback path in SessionConfig to apply it to a connection.";

/// `Security` 页。两个控件缺的是**两套不同的层**：
///
/// - Lock screen timeout：`term-app/src/lock.rs:16-19` 的 `LockState` 只有
///   `locked` / `error`，没有 Timeout 这个状态，也没有「窗口级最后活动时刻」
///   与计时器——锁屏只能手点，不会自己来。
/// - Username echo mode：没有登录向导这一层，也没有逐字回显的控制层；
///   回显是 PTY 那侧的行为，本项目没有可以按字符拦下的中间层。
const SECURITY_NOTE: &str = "Not wired: the timeout needs a window-level last-activity stamp and a \
     timer — the lock state is only locked/error and has no timeout. Username echo needs a login \
     wizard and per-character echo control; neither layer exists.";

/// `AutoCompletion` 页已在 2026-11-19 接成十行真控件（`settings_live_completion`），
/// 引擎在 `term-core::completion`。原先解释「整页灰显」的说明随之删除——
/// R5/I5 要求灰显理由只在确实没接线的项上保留，接线完成后不能再挂着旧理由。

/// `MouseTracking` 页。2026-11-19 校正：原先说「缺一条编码通道」是**过期说明**——
///
/// 底层通道早已接通：`term-core::mouse_report::encode_frame` 把指针事件编成
/// X10 / SGR / UTF-8 / Urxvt 报告，`pane.rs:389-415` 采集并写回 PTY，`vt.rs` 的
/// DECSET（?1000/1002/1003/1005/1006/1015h）也真的驱动了 mode / encoding。
///
/// 真正缺的是这**五个事件各一个偏好字段**，以及「用户勾选」与「远端 DECSET」
/// 两套控制源的优先级规则：底层是协议档位模型（总开关 + mode + encoding），
/// 不是「Move / Wheel / Click / Double / Middle 各一个 bool」。双击更特殊——
/// xterm 协议本身没有双击报告，本地双击只用于选词。所以这五行保持灰显，
/// 不是「没接线」，是「表达的开关模型在底层不存在」。
const MOUSE_TRACKING_NOTE: &str = "Not wired: mouse report bytes do reach the PTY (X10/SGR/UTF-8). \
     What is missing is a separate user preference for each of these five events and a rule for how \
     it combines with the program's DECSET mouse mode; the protocol models a mode, not five toggles.";

/// `Wrap` 页的说明。这一页不是「还没轮到」，是**接不了**：
///
/// - 内核在同一个列宽上已经硬折过行（`GridSnapshot.wraps` 就是那些续行），
///   所以逻辑行的显示宽度永远 ≤ 视口列宽，`term_app::wrap::wrap_lines` 对真实
///   内容就是恒等映射。`crates/term-app/src/wrap.rs` 的模块文档记着实测：整套
///   接线做完再回退，因为开与关的两张整窗截图**逐字节相同**。
/// - `Text.ToggleWordWrap` 仍在 `term_app::action` 的「点了没有效果」名单里
///   （`commands_without_a_visible_effect_stay_planned`），解析回来是 `None`。
/// - 除 `Wrap mode` 外的三项里，**只有 `Wrap column` 没有渲染器**：软换行本身
///   不存在，就没有「折到第几列」这个度量可画。另外两项 2026-11-13 已经接线——
///   `Wrap symbols` 走 `View.ToggleWrapSymbols` →
///   `ShellAction::Marks(MarksKnob::WrapSymbols)`（派发
///   `crates/term-app/src/parse.rs:99`，像素 `ui/terminal/marks.rs:386`：
///   `line.continuation` 为真时画 L 形勾，偏好落盘 `view_wrap_symbols`），
///   `Column markers` 走 `View.ToggleColumnMarker` → `MarksKnob::ColumnMarker`
///   （`parse.rs:97`，像素 `ui/terminal/marks_paint.rs:96` 的
///   `column_marker_col(image.cols)` 处画竖线，偏好 `view_column_marker`）。
///   两条都记在 `docs/menu-backlog.md:572,574`。
///
/// 按 I5：写清缺的是「把坐标系从一屏行改成一逻辑行 n 段」这一段，
/// 而不是画几个能点却什么都不做的开关。
const WRAP_NOTE: &str = "Not wired: the grid already hard-wraps at the pane width, so a soft-wrap \
     toggle would map every line onto itself (see term-app/src/wrap.rs). Only Wrap column has no \
     renderer — there is no wrap width to measure against until soft wrap exists. Wrap symbols and \
     column markers are already drawn either from the View menu or Preferences ▸ Wrap.";

/// `Highlight` 页 2026-11-19 搬去 live 页（`settings_live_highlight`）：
/// caret line 一行接成真勾选，原先「Text.HighlightCaretLine parses to no action」
/// 的说明已过期（该命令 2026-11-13 就接线了），随页搬走并改写。
/// 2026-11-19 同日：`incremental search` 也接了（那段的渲染本来就在画，补的是
/// 关掉它的字段），灰行的四行变三行。

/// `Shortcut Keys` 页的说明。这一页是**真数据、只读**，不是「没接线」：
///
/// 表里的每一行都是 `term_app::menu::DEFAULT_SHORTCUTS` 里的一对
/// （命令, 生效和弦），覆盖表有值时显示覆盖值。只读是因为编辑入口已经是
/// `Window ▸ Shortcuts` 那个对话框（`dialogs::shortcuts`）：它管着捕获态、
/// 重绑、恢复出厂，这里再摆一套就成了第二个编辑器和第二份真相。
const SHORTCUT_NOTE: &str = "Read-only: this list shows the bindings in effect \
     (factory table plus your rebinds). To change them, open Window ▸ Shortcuts — \
     that dialog owns rebinding and resetting to defaults.";

/// `Triggers` 页的说明。这一页**是真的接线**，不该退回到「没有配置字段」那句：
///
/// 规则编辑器直接读写 `Prefs.triggers`，每帧 `poll` 把 `TerminalWindow.triggers`
/// 喂给 `term_app::trigger::Matcher`。四种触发源（Output / Connect / Disconnect /
/// Idle）各自有消费者——`session.rs` 的 `apply_trigger` 把 `SendText` / `SendLine`
/// 写进 PTY、`Beep` 发 BEL、`RunCommand` 走命令通道。`Highlight` 这一项因为 v0.3
/// 才有行级高亮渲染基底，本页**不暴露**（不画也不假装画，R5）。
const TRIGGERS_NOTE: &str = "Live: rules are matched every frame against terminal \
     output and session events. Enabled rules can send text, ring the bell, or run a \
     menu command. Highlight is not offered yet (no line-highlight renderer).";

/// `OneKeys` 页的说明。凭据管理器（SSH / Account / Sessions 三标签）：
/// 用户名 / 密码 / 公钥（每个系统一份 Identity File）/ 键盘交互 Challenge-Response。
/// 编辑结果落 prefs，由 `auth_chain` 在连接时读取。锁屏门防止未解锁时改凭据。
const ONEKEYS_NOTE: &str = "Credentials stored here are used automatically by SSH \
     connections when a session's OneKey is selected. Click the lock to make changes.";

/// 页底那行小字。**有内容的灰页一律给自己的专句**：说明里得点出缺的那一节
/// （存储字段 / 状态机 / 渲染器 / 编码通道），「没接线」本身不是理由（S4 / W6）。
///
/// 2026-11-10：`Proxy` / `Security` / `AutoCompletion` / `MouseTracking`
/// 四页从通用那句换成专句，加上原有的 `Wrap` / `Shortcut Keys`
/// （`Highlight` 2026-11-19 搬去 live 页），数据表里再没有一页共用套话。（`QuickBar` 原先也有专句，
/// 但那页 2026-11-19 接成真控件，说明跟着搬去 `settings_live_quickbar`。）`DISABLED_NOTE` 只剩空页
/// `Widgets` 兜底——它 `blocks` 为空，
/// `draw` 在画说明之前就返回了，所以那句其实一个像素都不会上屏。
///
/// `Tabs` 同一天先写成专句（「五项缺口」），随后被接走了三项、整页改成手绘，
/// 那句就**删掉**而不是留着：一张八行里已经有三行能动的页，再说「五项全缺」
/// 是反向的谎。它现在的说明在 `settings_live_tabs.rs` 的 `TABS_NOTE`。
/// `Terminal` 走了同一条路（那句「三处缺口 / 粘贴门控没有闸」在粘贴确认
/// 接上之后已经是假话），说明搬去 `settings_live_terminal.rs` 的 `TERMINAL_NOTE`。
///
/// 2026-11-11：`Binary` 走同一条路。它原来那句专句（「没有 hex 视图、
/// `Text.ToggleHexEdit` 解析成 None」）在 hex 视图真的接上之后**两句都是假的**，
/// 于是整页搬去 `settings_live_binary.rs` 做成 live 页、专句跟着页走：
/// 一张有真读数的页，说明里不能再写「没接线」。
///
/// **手绘的 live 页不在这里**：它们每一页都有自己的说明（`CARET_PAGE_NOTE` /
/// `FILER_PAGE_NOTE` / `settings_live_text` 里那几条），因为「哪几行真、哪几行
/// 缺什么」是那一页自己的事，混进这张按页查的表里就成了按页抄的第二份文案。
pub(in crate::ui) fn page_note(page: SettingsPage) -> &'static str {
    match page {
        SettingsPage::Proxy => PROXY_NOTE,
        SettingsPage::Security => SECURITY_NOTE,
        SettingsPage::AutoCompletion => "", // live 页，无灰显说明
        SettingsPage::MouseTracking => MOUSE_TRACKING_NOTE,
        SettingsPage::Wrap => WRAP_NOTE,
        SettingsPage::Highlight => "", // live 页（caret line / incremental search 真，其余三行灰）
        SettingsPage::ShortcutKeys => SHORTCUT_NOTE,
        SettingsPage::Triggers => TRIGGERS_NOTE,
        SettingsPage::OneKeys => ONEKEYS_NOTE,
        _ => DISABLED_NOTE,
    }
}

// 2026-11-15：原先这里有一张 `footer_note(page)` 表 + `REFERENCE_FOOTER`，
// 负责给 Preferences 页脚那句橙色「什么时候生效」提示。参考实现
// `03-settings/01-application.png` 的页脚只有右下角 `Close`，而这句长文本
// 在 560 宽的对话框里会被 `Close` 压掉半截（用户反馈「糟乱」）。整条渲染与
// 这张表一起删除；各页自己的说明本来就在页内（`page_note` /
// `settings_live_text::*_NOTE`），信息没有丢。
//
// `Toolbar / Menu` 那条「工具行开关不落盘」的边界原先只写在页脚，现在并进
// `settings_live_text::TOOLBAR_NOTE` 的页内说明，仍然对用户可见。

/// 按运行期覆盖表算出生效的（命令, 和弦）列表——`Shortcut Keys` 页的全部数据。
///
/// 覆盖表的下标对应 `DEFAULT_SHORTCUTS` 的位置（`dialogs::shortcuts` 就是
/// 这么存的）。越界下标（手改过的文件、表缩短后的残留）**忽略而不是 panic**：
/// 页面只读，一条无效覆盖不该把整页打崩，也不该顶掉别人的行。
pub(in crate::ui) fn effective_chords(
    overrides: &[(usize, String)],
) -> Vec<(&'static str, String)> {
    term_app::menu::DEFAULT_SHORTCUTS
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let chord = overrides
                .iter()
                .find(|(j, _)| *j == i)
                .map(|(_, text)| text.clone())
                .unwrap_or_else(|| s.chord.to_string());
            (s.label, chord)
        })
        .collect()
}

/// 页 → 内容。手绘的 live 页返回 `None`，由 `settings.rs` 自己画。
///
/// 现在是十七页：`settings_live.rs` 五页（Application / Appearance / Caret /
/// CommandHistory / Filer）+ `settings_live_search.rs` 一页 +
/// `settings_live_font.rs` 一页 + `settings_live_binary.rs` 一页 +
/// `settings_live_tabs.rs` 一页 + `settings_live_terminal.rs` 一页 +
/// `settings_live_quickbar.rs` 一页（2026-11-19，三项真控件）+
/// `settings_live_wrap.rs` 一页（同日，折行符号 / 参考列 / 列号三项真控件）+
/// `settings_live_highlight.rs` 一页（同日，caret line 真勾选）+
/// `settings_live_completion.rs` 一页（同日，十行全真控件）+
/// `settings_live_text.rs` 三页（后几页每页只有几行是真控件，见那边的模块文档）。
pub(in crate::ui) fn page_spec(page: SettingsPage) -> Option<PageSpec> {
    let spec = match page {
        SettingsPage::Application
        | SettingsPage::Appearance
        | SettingsPage::Font
        | SettingsPage::Caret
        | SettingsPage::CommandHistory
        | SettingsPage::Search
        | SettingsPage::Binary
        | SettingsPage::Filer
        |         SettingsPage::Text
        | SettingsPage::Behaviour
        | SettingsPage::ToolbarMenu
        | SettingsPage::Tabs
        | SettingsPage::Terminal
        | SettingsPage::QuickBar
        | SettingsPage::Wrap
        | SettingsPage::Highlight
        | SettingsPage::AutoCompletion
        | SettingsPage::Triggers
        | SettingsPage::OneKeys => return None,
        SettingsPage::Proxy => PageSpec {
            title: "Proxy Type",
            blocks: &[Block::Rows(&[Row::Combo { label: "", value: "No proxy" }])],
        },
        SettingsPage::Security => PageSpec {
            title: "Security",
            blocks: &[
                Block::Group {
                    title: "Lock Screen",
                    rows: &[Row::ComboAbove { label: "Lock screen timeout:", value: "Off" }],
                },
                Block::Group {
                    title: "Login Wizard",
                    rows: &[Row::ComboAbove {
                        label: "Username echo mode:",
                        value: "Do not echo characters",
                    }],
                },
            ],
        },
        SettingsPage::MouseTracking => PageSpec {
            title: "Mouse Tracking",
            blocks: &[Block::Group {
                title: "Tracking Events",
                rows: &[
                    Row::DisabledCheck("Move event"),
                    Row::DisabledCheck("Wheel event"),
                    Row::DisabledCheck("Click event"),
                    Row::DisabledCheck("Double click event"),
                    Row::DisabledCheck("Middle click event"),
                    Row::Check("Right click event", false),
                ],
            }],
        },
        // Highlight 页 2026-11-19 接成真控件（settings_live_highlight）：
        // caret line 一行有完整读者（Prefs + Tab.marks + 渲染器），接真；
        // 其余三行仍按数据表画法灰显，说明跟着页搬到 live 文件
        // （2026-11-19 同日：incremental search 已接真，四行灰变三行）。
        // Wrap 页 2026-11-19 接成真控件（settings_live_wrap），不再在此表。
        // Quick Bar 页 2026-11-19 接成真控件（settings_live_quickbar），不再在此表。
        // §2.6 点名的三页里，`Behaviour` / `Toolbar / Menu` 已搬去
        // `settings_live_text.rs`（部分真控件）；只剩 `Shortcut Keys` 还在表里，
        // 而且它不是灰显占位：表里是**真绑定**（出厂表 + 运行期覆盖），只是
        // 只读——编辑入口在 `Window ▸ Shortcuts`，见 `SHORTCUT_NOTE`。
        SettingsPage::ShortcutKeys => PageSpec {
            title: "Keyboard Shortcuts",
            blocks: &[Block::Shortcuts],
        },
        // 视觉基线里点 `Widgets` 组头是个空页（右侧一片空白）。
        SettingsPage::Widgets => PageSpec { title: "Widgets", blocks: &[] },
    };
    Some(spec)
}

#[cfg(test)]
#[path = "settings_pages_tests.rs"]
mod tests;
