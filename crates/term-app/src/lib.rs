//! `term-app` — 无前端的应用层（从 `app/src/app/` 抽出，ADR-0016）。
//!
//! 这一层回答「按了某个命令之后应该发生什么」，但**不知道像素，也不知道 PTY**：
//! 它只依赖 `term-core`（纯语义），既没有 UI 框架，也没有 `term-runtime`。
//!
//! 边界由依赖表强制，比 grep 判据硬：
//!
//! - 没有 `egui` / `eframe` → 投影层无法把「绘制时的顺手一写」塞进来；
//! - 没有 `term-runtime` → 这一层**拿不到 PTY 与线程**，副作用只能以 `Effect` 值返回；
//! - `apps/desktop` 的 egui 壳与无窗口路径复用同一份判定，不会各写一套。
//!
//! 每个能力一个模块，独占自己的动作、自己的字符串入口和自己的 reducer：
//!
//! - `address`：地址栏（连接目标输入框）的纯逻辑——地址解析 / 展示串 /
//!   候选过滤与拼接 / 选中位移 / 最近使用 MRU。零 `egui`、零 IO；
//!   命令页（把一行当 shell 命令发出去）另有一页，改名 `Command Line`
//! - `vocab`：**词表层**——`Action` 与各能力的动作枚举、`FeatureId`、`PanelId`、`DockZone`。
//!   零函数零 IO，是整个 crate 的最低层（2026-11-12 从各模块下沉）
//! - `effect`：`Effect`（副作用以值返回）——纯层与执行层之间唯一的通道
//! - `parse`：入口层，命令 ID → `Action`，只依赖 `vocab`
//! - `reduce`：分发层，`Action` → 各能力 reducer → `Effect`，唯一知道全部能力的地方
//! - `state`：`ViewState` / `FindState` / `LockState` / `MarginState`，视图开关的纯数据集合
//!   （每个字段都有读者）
//! - `action`：**兼容门面**，只再导出 `Action` / `Effect` / `reduce`（保 `term_app::action::…` 路径）
//! - `shell`：外壳能力（窗口 chrome / 模式开关 / 对话框 / 连接）
//! - `find`：会话内查找（匹配是 `(query, 窗口)` 的纯函数值，不作为状态缓存）
//! - `clipboard`：复制 / 粘贴，只声明意图，编码留给 `term-core::vt`
//! - `selection`：选区几何 + 17 条选择命令的位移规则
//! - `log`：会话日志的七条命令，只翻状态 / 发意图，不碰文件
//! - `palette`：命令面板的注册表与过滤，条目取自菜单树，不另建清单
//! - `goto`：跳到回看绝对行 / 已打开会话，输入解析与状态都在纯层
//! - `center`：v0.1 工作台，15 条命令收敛成带页面的状态机
//! - `shortcuts`：快捷键和弦的匹配纯层
//! - `lock`：锁屏状态机（复用 Vault 主密码）
//! - `margin`：终端左侧边栏的五列开关（留白 / 行号 / 符号 / 时间戳 / 折叠），
//!   只管「哪列开着」；列宽与字形在投影层，行号与时刻来自内核快照
//! - `blocks`：OSC 133 命令块索引（前置）
//! - `dock`：停靠 / 浮动的词表——面板身份、停靠带、四个布局动作；
//!   布局状态本身在投影层（`ui/layout/dock.rs`），这一层没有一个字节的几何
//! - `tmux`：tmux -CC 控制模式解析
//! - `menu`：菜单树纯数据（114 项命令，10 个顶级菜单；删掉 106 条编辑器语义项）
//! - `capability`：命令「是否已接线」的唯一判定点，基线单测守数字
//! - `i18n`：界面语言与英中词表（key 就是英文原文，缺词退回英文）
//! - `viewport`：回看视口（上限来自内核报告的 history）
//! - `wrap`：软换行的行映射（逻辑行 + 视口列宽 → 屏幕行），纯函数；
//!   只管「折成几段、每段从第几列起、占多宽」，渲染留给投影层
//! - `startup`：启动会话三档策略与「激活标签才连」的判定；
//!   恢复结构不恢复进程，连接动作由投影层落地
//! - `tab`：标签右键 10 个动作的强类型意图与影子入口表；
//!   让「不在菜单栏里的真实入口」也进 `capability` 台账
//! - `onboarding`：首次启动引导——`~/.ssh/config` / WindTerm `profiles/`
//!   认出来的主机、`Welcome → Detect → Done` 三态机与勾选位。只有纯状态逻辑：
//!   找文件与文本解析都在投影层，那里复用全仓唯一那份 ssh config 解析器，
//!   本层只接收摊好的 `DetectedHost`，因此依赖表仍只有 `term-core`
//!
//! 以下三个**只在 `--features planned` 下编译**（理由见下方 `planned` 段）：
//!
//! - `structure`：结构领域模型——折叠区间 / 多光标 / 作用域栈的不变量
//! - `trigger`：触发器求值内核——事件 → 匹配 → 动作；跨 chunk 的尾巴、
//!   冷却与一次性事件都在纯层，`now` 由宿主注入
//!
//! `tmux_layout`（`%layout-change` 布局串 → 窗格矩形、PTY 尺寸双向换算）已于
//! 2026-11-12 因真读者提回**默认编译面**，见下方 `tmux_layout` 段的注释。

pub mod action;
// `address` 留在**默认编译面**上，不进 `--features planned`。
//
// 本 crate 的规矩是「没有生产读者的模块只配 `planned`」（见下方 `planned` 段
// 的说明）。这一条不适用于 `address`：它的读者是同一刀的另一半——投影层的
// 地址栏浮层（`apps/desktop`）与 `dispatch_action` 对 `Action::AddressBar` 的处理，
// 与 `tmux_layout` 因双向测试提回默认面是同一类情形。等 UI 接上，
// `parse_target` / `session_candidates` / `push_recent` 各有真实调用点。
pub mod address;
// 菜单命令与应用面动词的分类表。留在**默认编译面**：它的读者是
// `capability`（同一份清单）与 `apps/desktop` 的菜单置灰文案，不是 `planned` 特性。
pub mod api_class;
pub mod blocks;
pub mod capability;
pub mod center;
pub mod clipboard;
pub mod dock;
pub mod effect;
pub mod find;
// 折叠功能的唯一领域模型，因此**必须在默认编译面**上：`apps/desktop` 的 `Tab` 要
// 真正读取它（`Folds` + `blocks_from_inputs`），有生产读者就不属于 `planned`。
// 与之配对的 `FoldRegion` / `StructureError` 原本在 `structure` 里，2026-11-14
// 随读者一起搬到了本模块；`structure` 反过来 `pub use` 回去，老路径不断。
pub mod folds;
pub mod goto;
pub mod i18n;
pub mod lock;
pub mod log;
pub mod margin;
pub mod menu;
pub mod mouse;
pub mod onboarding;
pub mod palette;
pub mod parse;
pub mod reduce;
pub mod selection;
pub mod shell;
pub mod shortcuts;
pub mod startup;
pub mod state;
pub mod tab;
pub mod tmux;
pub mod vocab;
pub mod viewport;
// P：终端滚动条与悬停预览窗的纯几何层（零 egui、零 IO、可单测）。
// 只做「数值 → 数值」换算：滑块 ↔ scroll_top 互算、hover ratio → 预览 ViewRange、
// 预览框钳制。像素与绘制在投影层（`apps/desktop` 的 `pane_scrollbar.rs` /
// `pane_preview.rs`），本模块一个像素常量都没有。配套 ADR-0028。
pub mod scrollbar;
// 纯几何层的单测文件（`scrollbar_tests.rs`）放在 crate 根同级，避免 `scrollbar/`
// 子目录；测试模块在此声明（设计稿 §6.1 列为配套文件）。
#[cfg(test)]
mod scrollbar_tests;
pub mod wrap;

// ── `planned`：已实现、已测试，但**没有任何生产读者** ──────────────────────
//
// 这两个模块共约 800 行生产代码 + 34 条单测。2026-11-12 审查实测：
// `term_app::structure` / `term_app::trigger` 两个模块路径，加上 crate 根那些
// 再导出符号（`Matcher` / `Rule` / `Carets` …），在 `apps/` 与其余
// `crates/` 里的外部引用**全为 0**。
//
// 2026-11-14 订正：`FoldRegion` 与 `StructureError` 已搬去默认面上的 `folds`
// （折叠有真实读者），因此它们不再属于这份清单；`structure` 里剩下的
// `Carets` / `ScopeStack` / `StructureModel` 仍然零读者，继续挂在 `planned`。
//
// `tmux_layout` 原本是第四个，2026-11-12 因 Step 9 双向闭环测试
// （`apps/desktop/tests/docker_tmux.rs` 直接 `use term_app::tmux_layout`）有了真读者，
// 已提回默认编译面——它的 12 条单测现在默认会跑。
//
// 留在默认编译面上时，`cargo test` 的绿色在为无人调用的代码担保——而本 crate
// 自己的规矩是「没有读者的字段不是预留，是第二步的假接口」（见 `state.rs` 开头）。
// 所以用 feature 把「已接线」与「已实现但未接线」在**编译期**分开，与本 crate
// 处理未接线命令的方式一致（返回 `None` 而不是占位变体）。
//
// 这不是删除：触发器与 tmux `%layout-change` 都还在路线图上，`--features
// planned` 时全部回来。`scripts/verify.sh` 第 1 项会用该 feature 编一次，防烂。

#[cfg(feature = "planned")]
pub mod structure;

// `tmux_layout` 于 2026-11-12 提回默认编译面：Step 9 的双向闭环测试
// `apps/desktop/tests/docker_tmux.rs` 直接 `use term_app::tmux_layout`（真机验证
// `split-window -h` 后能解析出 2-pane 布局），有了真读者就不再是「无人调用」。
// 后续产品侧 auto-feed / 尺寸双向也走这里。

pub mod tmux_layout;

#[cfg(feature = "planned")]
pub mod trigger;
pub mod trigger_builtins;

// Trigger 的简化正则（分组 / 选择都不支持）；只在 `trigger` 内部使用。
#[cfg(feature = "planned")]
mod trigger_regex;

pub use tmux_layout::{layout_to_size, parse_layout, resize_command, Layout, Node};

// 地址栏的常用项在 crate 根再导出一次，与 `tmux_layout` 同一条规矩——
// 投影层写 `use term_app::{parse_target, AddressState, …}` 就够，
// 不必记住「这个类型落在哪个子模块」（模块路径仍然可用，两者不冲突）。
pub use address::{
    merge_candidates, move_selection, parse_target, push_recent, render_target,
    session_candidates, window_start, AddressKind, AddressProblem, AddressState, Candidate, Target,
};

#[cfg(feature = "planned")]
pub use trigger::{validate, Hit, Matcher, Rule, TriggerAction, TriggerOn};
