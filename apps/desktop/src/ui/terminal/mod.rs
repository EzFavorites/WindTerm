//! 终端窗格：绘制 / 逐行缓存 / margin 边栏 / 键盘选择 / 右键菜单 / 标签构造。
//!
//! 落点：这里只收 `&ScreenImage` 与只读快照，一切写入经 `Action`。

/// hex 视图的**绘制**：表头徽标、逐行 dump、回看药丸。只吃 `&RawLog` 与几何，
/// 不持 `&mut TerminalWindow`——绘制与窗口状态分开，`pane.rs` 才只留一个分叉点。
mod bin;
/// hex 视图的**状态与纯函数**（原始字节环、视口锚点、一行怎么排版）。
/// 零 `egui`，可单测；绘制在 `bin.rs`。
///
/// `pub(in crate::ui)`：`Tab` 要持有 `RawLog`（见 `ui/tab.rs`），而兄弟模块之间
/// 的可见性不互通，模块本身也得放开。
pub(in crate::ui) mod bin_spec;
/// 官网 #47/#48/#49 三个内置触发器的悬停浮层（hex / 时间戳 / 压缩包）：
/// 只展示换算结果，不写 PTY、不改选区。纯检测与换算在 `term_app::trigger_builtins`。
mod builtin_hover;
pub(in crate::ui) mod completion_popup;
mod context_menu;
/// 视觉基线两份右键菜单的逐字清单 + 与菜单栏同源的四栏手绘。
/// `pub(in crate::ui)` 让 `comp/context_fixtures.rs` 能直接复用出图。
pub(in crate::ui) mod context_rows;
/// `Mode ▸ Debug Mode` 的**绘制**：`Cell` → 属性位 / 色号种类 / 宽字符标记 → `painter` 调用。
///
/// 它收的是 `&ScreenImage` 与一格几何（`origin` + `cell`），**不碰任何状态**——
/// 与 `marks_paint::paint` 同一个形状，理由也同一条：`LineCache` 只缓存
/// `spans` / `bg_runs`，所有标记都追加在 span 循环之后，天然是「另一个文件的事」。
/// 单独一个文件还有第二个理由：九种颜色的常量 + 六个区的纯几何 + 七条单测，
/// `view.rs` 只剩 24 行余量（单文件上限 500），一行都放不下。本文件 404 代码行。
///
/// 开关本身是每条会话的 `Tab::debug`，唯一的调用点是
/// `pane_overlays.rs` 的第三与第四段之间（`if !bin && tab.debug`）。
/// 正文一格没动：它只让「SGR 属性 / 色号 / 宽字形」从不可见变成可读。
mod debug_paint;
/// Find All 结果按标签身份发布 / 校验（分屏时不再互相覆盖）。
pub(in crate::ui) mod find_results_slot;
/// 折叠列的指针几何命中：不依赖内容区 `Response`（margin 右键 interact 会抢走它）。
mod fold_hit;
/// hex 视图的**参数与排版**：三张档位表、`HexParams`、一行多宽、窗宽夹取。
/// 零 `egui`，可单测；`bin_spec.rs` 的 `row_text` 用它的宽度算法。
pub(in crate::ui) mod hex_params;
/// 空白与符号可视化：**参数**与**纯决策**（零 `egui`，可无头单测）。
///
/// `View ▸ Toggle Line Endings / Column Marker / Indent Guides / Wrap Symbols`、
/// `View ▸ White Spaces` 五档、`Text ▸ Highlight Caret Line` 一共十一个开关的
/// 「这一行该画哪些标记、画在第几列」全在这里回答；把它们变成像素的是
/// `marks_paint.rs`。拆成两个文件是单文件上限的老规矩：`view.rs` 已贴着单文件上限，
/// 而这些判据要能被没有窗口的单测钉死。
mod highlights;
mod history_scope;
/// 悬停浮层的帧间锚点锁存（不追指针，指针进浮层继续 show）。
mod hover_anchor;
/// 鼠标 → 选区的全部手势与它要的窗格几何（`pointer_selection` / `GridGeom`）。
///
/// 2026-11-13 从 `pane.rs` 拆出——栅格可视化接线往那边塞进两处
/// `apply_cmd_with_blocks`，把 `pane.rs` 顶到 527 代码行（单文件上限 500）。
/// 搬走的这一块本来就是自由函数，行为一行未改。
/// 悬停浮层的纯几何：文本偏移 → 屏幕 cell 列（宽字符前缀不能偏列）。
mod hover_hit;
mod ip_hover;
mod margin;
mod marks;
/// 上一条的**绘制**：`Mark` → `painter` 调用（空格点、Tab 箭头、参考线、`$`、折行勾）。
mod marks_paint;
/// 叠层标记的段内夹取（软折后按段而不按 `lines` 下标）。
mod overlay_segments;
mod pane;
/// 正文**二选一**：字符网格（`view::paint_rows`）还是 hex 转储（`bin::paint`），
/// 含 `Text ▸ Highlight Caret Line` 那个视口行号的算法。
///
/// 从 `pane.rs` 拆出——搬之前那边 492 代码行（单文件上限 500），
/// 而段驱动渲染 / reflow / 坐标系联动三批工作都要往 `draw_tab` 里加代码。
/// 本文件 65 代码行。它是自由函数：`draw_tab` 握着借自 `self.tabs` 的 `&mut Tab`，
/// 写成方法会同时要 `&mut self`（E0499），所以只收拆分好的借用，字形缓存那一句
/// 也由调用点算好再传。
mod pane_body;
/// 弹层点外收起那一击的终端侧闸门：折叠列点击 / xterm 鼠标报告两个分支共用。
/// `pub(in crate::ui)` 让 shell 侧的接线单测直接调到生产同一把判据。
pub(in crate::ui) mod pane_click_gate;
/// 正文之后的**全部叠层**：查找高亮 / 常驻词高亮 / 空白与符号标记 / 调试覆盖层 /
/// 回看药丸 / 光标。
///
/// 与 `pane_body` 同一因同一刀——从 492 代码行的 `pane.rs` 里切出去，
/// 拆完 `pane.rs` 降到 378 代码行。本文件 135 代码行。同样是自由函数（理由见上，
/// E0499），而且它只往 `painter` 上追加图形、一个状态都不写——这正是它能被整块
/// 搬走的原因。六段的**顺序决定像素**：搬过来的五段一段都没换位，
/// 调试覆盖层（`Mode ▸ Debug Mode`）是后来**插进**第三与第四段之间的第六段——
/// 它的几何全在 `debug_paint.rs`，本文件只多一个调用点。
mod pane_overlays;
mod pane_pointer;
/// 悬停预览窗：轨道 hover → 第二事实窗口（独立槽）→ 半透明历史快照框。
///
/// 第 7 段 chrome（在 `pane_scrollbar` 之后）；只读 `tab.preview_screen`，不动
/// `tab.view` / `tab.viewport` / `tab.last_request`（不变式）。
mod pane_preview;
/// 终端滚动条：egui 几何适配、轨道/滑块绘制、命中区、拖拽、点击跳转、滚轮补位。
///
/// 浮动叠加：不改 `avail_w` / `cols`，贴底视觉基线零 diff。纯几何在
/// `crates/term-app/src/scrollbar.rs`，这里只做像素投影与交互。
mod pane_scrollbar;
/// 粘贴的落地路径（`commit_paste`）与它的判据。从 `pane.rs` 拆出来：
/// 把这 129 行塞进那边，把 `pane.rs` 顶到 518（上限 500）。
mod paste;
/// 彩虹括号：按嵌套深度给括号字自身上前景色（改 `Cell.fg`，与字下淡色底不同）。
mod rainbow;
mod script;
/// 「运行脚本」窗口的两份清单：脚本库（读盘）与最近用过（读 prefs）。
///
/// `pub(in crate::ui)`：`panels/script_panel.rs` 的脚本下拉也要用同一个
/// `library()`——两处各写一份过滤规则的话，「面板里看不见、窗口里看得见」这类
/// 不一致迟早出现，而根因藏在两份 `.rhai` 判定里。
pub(in crate::ui) mod script_library;
mod select;
// `pub(in crate::ui)`：帧末派发层（`ui/trigger_intents.rs`）要复用这里的
// 转义解码与写 PTY 闸门，两点必须在同一个入口，不能各写一份编码规则。
pub(in crate::ui) mod session;
pub(in crate::ui) mod trigger_dispatch;
/// ④-0：新窗格身份铸造逻辑集中在 `crate::ui::layout::mint_pane_identity`
/// （定义在 `layout/split.rs`，与 `PaneId` 同处）。`setup` 模块放开给 `crate::ui`
/// 下兄弟模块调用，本身是终端构造入口的聚合点。
pub(in crate::ui) mod setup;
/// 「画哪一块终端」的槽位（主区标签页 / 底带 Shell 面板内的某一条）与按槽位取
/// 状态的那几个自由函数。2026-11-15：底带 Shell 面板不再有自己的渲染，改走
/// `pane::draw_tab`。
///
/// `pub(in crate::ui)`：`panels/shell_panel.rs` 要用 `TabSlot` 与过滤判据
/// （`shell_tab_matches`），它们与槽位是同一件事（面板里的标签就是槽位的一端）。
pub(in crate::ui) mod tab_slot;
/// 自定义菜单 `Run command` 的载荷判据：命令 ID 走 dispatch，正文走 SendLine。
mod trigger_menu;
mod view;
mod vim_apply;
/// Vim `p` / `P` 的粘贴漏斗（临时离开只读 → 落地 → 恢复）与帧末触发器派发。
pub(in crate::ui) mod vim_paste;
#[cfg(test)]
#[path = "vim_paste_tests.rs"]
mod vim_paste_tests;
/// 传输：进度状态行 + 起点接线（弹层确认 → `XferHandle::start` → 画一行）。
/// `pub(in crate::ui)` 让 `ui/mod.rs` 的 `draw()` 直接调 `terminal::xfer`。
pub(in crate::ui) mod xfer;
// `xfer.rs` 只画与接线，`xfer_spec.rs` 持有状态 / 措辞 / 单测。
mod xfer_spec;

/// hex 的参数类型与档位表：`ui/mod.rs` 的字段、`ui/construct.rs` 的初值、
/// `ui/effects.rs` 的落地点都要用它，所以在这里转一次手，调用方不必写长路径。
///
/// 它收的不止是排版数：斑马线（[`HexParams::zebra`]）也在里面，与四个数同一
/// 作用域——同一个设置页上同类参数必须只有一套语义。
pub(in crate::ui) use hex_params::HexParams;
/// 空白/符号可视化的参数类型与五档模式：`Tab` 要持有它（见 `ui/tab.rs`），
/// `ui/effects.rs` 的落地点、`setup.rs` 的初值入口、`pane.rs` 的绘制都要用它，
/// 所以在这里转一次手，调用方不必写长路径。
pub(in crate::ui) use marks::{MarksParams, WhitespaceMode};
pub(in crate::ui) use pane::draw_caret;
pub(in crate::ui) use script::ScriptDialog;
pub(in crate::ui) use setup::{ensure_view, make_tab, make_tab_with_channel, tab_from};
pub(in crate::ui) use tab_slot::TabSlot;
pub(in crate::ui) use view::LineCache;

/// 斑马线作用域的两条回归单测：设置页改默认值只影响**新**标签。
///
/// 单独一个文件是为了让「已开标签的画法」能被真的数出来（跑一帧
/// `draw_terminal_area_for` 再数底色块），而那需要 `lifecycle_test_support`
/// 的整份窗口夹具——塞进 `pane.rs` 会把那 648 行再顶高一截。
#[cfg(test)]
#[path = "zebra_scope_tests.rs"]
mod zebra_scope_tests;

/// **菜单路径**的判据：十条栅格可视化命令 `dispatch(id)` 之后，活动标签的
/// `Tab.marks` 真的变了、且菜单的勾（`marks_knob_on`）说的是同一句话。
///
/// 单独成文件的两个理由：`marks_tests.rs` 已到 476 代码行（单文件上限 500），
/// 而这条用例要的是真窗口夹具（`lifecycle_test_support`），与那边零 `egui`
/// 的纯函数夹具不是一类东西。
#[cfg(test)]
#[path = "marks_menu_tests.rs"]
mod marks_menu_tests;

/// 行过滤器「每条会话独立」：两个 tab 互不串味，`Action::Filter` 不写 `ViewState`。
#[cfg(test)]
#[path = "filter_session_tests.rs"]
mod filter_session_tests;

/// 控制行经 tap 字节流自动 feed 的回归判据。单独成文件是因为
/// `bin_spec.rs` 已顶到上限附近，新测试再塞进去会踩 500 行红线。
#[cfg(test)]
#[path = "tmux_feed_tests.rs"]
mod tmux_feed_tests;

/// ADR-0025 的对账判据：被外部（脚本 / 桥接的 `session_close`）关掉的会话，
/// 标签必须自己认出来并转成断开形态。
///
/// 单独成文件：`session.rs` 是事件泵本体，判据要的是「真会话 + 关掉再看」的夹具
/// （`lifecycle_test_support`），与那边纯状态机的读法不是一类东西。
#[cfg(test)]
#[path = "session_tests.rs"]
mod session_tests;
