//! 一个打开的终端 tab 的全部状态。
//!
//! 为什么单独一个文件：`mod.rs` 已经顶到验收 4o 的 500 行上限（`scripts/verify.sh`），
//! 而 `Tab` 是它里面唯一「只描述数据、不带绘制逻辑」的块（R9 之后 `mod.rs` 只剩装配与
//! `draw`）。搬出来属于纯移动：字段一字未改，`mod.rs` 用 `pub use tab::Tab;` 保持
//! `crate::ui::Tab` 这个路径不变，20 处调用点一个都不用改。
//!
//! 注意字段可见性：`Tab` 的字段原本在 `ui` 模块内可见，搬到子模块后必须显式写成
//! `pub(in crate::ui)`，否则 `ui::terminal::*` / `ui::panels::*` 这些**兄弟**模块
//! 就读不到了（Rust 的私有是「本模块及后代」，不含兄弟）。

use std::sync::Arc;

use eframe::egui;
use term_app::selection::Selections;
use term_app::trigger::Matcher;
use term_app::viewport::Viewport;
use term_core::grid::{GridSnapshot, ScreenImage};
use term_core::session::SessionConfig;
use term_core::session_runtime::SessionId;
use term_session::SessionPool;

/// An open terminal tab. S2: 只留标识 + 只读快照，零 OS 资源。
pub struct Tab {
    /// 控制面（ADR-0019 档 1.1）。此前这里是 `Option<SupervisorHandle>`，tab 直接握
    /// 线程 / 队列 / 邮箱；现在动作一律经它落到同一个会话对象上。
    pub(in crate::ui) pool: Arc<SessionPool>,
    /// 本 tab 绑定的会话；`None` = 已断开或夹具要静态画面（对应旧 `handle.is_some()`）。
    pub(in crate::ui) session: Option<SessionId>,
    /// ADR-0025 阶段 0：本 tab 的**恒在逻辑身份**，独立于存活态铸造。
    ///
    /// 断连 / 重连都不改变；是窗口态 `open_order` / `active` / `pending_activation`
    /// 的身份键。`Tab.session` 是 `Option`（存活态）、`Tab.id` 是 egui 焦点 id
    /// （投影态），二者都随存活 / 重排漂移，不能稳定标识「这条逻辑会话」，故另立此字段。
    pub(in crate::ui) identity: SessionId,
    /// What this tab runs.
    pub(in crate::ui) config: SessionConfig,
    /// 只读屏幕快照，由 [`Tab::poll`] 更新。
    pub snapshot: GridSnapshot,
    /// R1：最近一次按范围请求的只读事实窗口（回看/选区/搜索共用通道）。
    /// F1 起它是回看时的渲染来源。
    pub screen: Option<ScreenImage>,
    /// F2：本帧真正画在屏上的事实窗口（贴底 = 快照派生，回看 = 范围窗口裁切）。
    ///
    /// 它是一份**派生值**，不是权威状态：每帧由 `ensure_view` 重算。
    /// 之所以留在 tab 上，是因为查找与 reducer 也要读同一个窗口——
    /// 「用户看到的那一屏」只能有一个定义，否则搜索与渲染会各算一屏。
    pub view: ScreenImage,
    /// F1：回看视口（贴底时跟随输出）。
    pub viewport: Viewport,
    /// F1：选区锚在内容行号上，滚动不跟着漂。
    ///
    /// W2 起这里是**一批**选区（`Selections`：若干段区间 + 活动下标 + 锁位）。
    /// 它不进 `ViewState`：选区是每帧从事实窗口推导的视图数据（拖动时每帧都变），
    /// 而 `ViewState` 只装领域语义、只能由 `reduce` 写。9 条集合命令改的正是它，
    /// 落地在 `ui/terminal/pane.rs`（那里才拿得到本帧的事实窗口）。
    pub selections: Selections,
    /// F1：最近一次范围请求与当时的版本，避免每帧重复上行。
    ///
    /// 存 [`ViewRange`](term_core::coords::ViewRange) 而不是裸 `Range<i64>`：
    /// 这个值唯一的作用就是和下一帧算出的 `viewport.rows()` 比较，
    /// 两边同一套类型，「拿绝对行号跟视口行比」这种错装配不进去。
    pub(in crate::ui) last_request: Option<(term_core::coords::ViewRange, u64)>,
    /// P：预览用事实窗口槽（悬停滚动条时的历史快照），与 `screen` **物理隔离**
    /// （ADR-0028）。绝不与 `screen` / `view` / `last_request` 共用：否则一帧内
    /// 两份 `Event::Screen` 只剩一份，真实视口会退化成空白帧。
    pub(in crate::ui) preview_screen: Option<ScreenImage>,
    /// P：预览请求的去重键（range + version），与 `last_request` **并列、独立判据**
    /// （ADR-0028）。不跟着 `last_request` 的复位点（goto / fold）走——预览是
    /// 临时 chrome，它有自己的 staleness 生命周期。
    pub(in crate::ui) preview_last_request: Option<(term_core::coords::ViewRange, u64)>,
    /// R13-f：这条会话连接时用过的凭据（内存里，不落盘）。
    ///
    /// 文件面板与隧道都是「连上之后还要再开一条连接」的消费者，没有它就得
    /// 再问用户一次密码——而那时用户正在看文件列表，问密码等于说「刚才那次不算」。
    /// 系统 `ssh` 会话没有这个值（凭据在子进程手里），面板据此显示原因而不是硬连。
    ///
    /// 类型是 `term_ssh::AuthMethod`（**带凭据**的那份），不是 `term_core` 里
    /// 同名的那个：后者只描述「打算用哪种方式认证」，不含密码与私钥路径。
    pub(in crate::ui) auth: Option<term_ssh::AuthMethod>,
    /// W5-I：本 tab 的输入行缓冲。把按键字节重建成「回车前那一行」，
    /// 回车时吐出，由窗口级 `CommandHistory` 记录。纯内存，不落盘（落盘见后续）。
    pub(in crate::ui) input_line: term_core::input_line::InputLine,
    /// Focus id so keyboard events reach this tab only.
    pub(in crate::ui) id: egui::Id,
    /// 最近一次往 PTY 写字（＝一次输入）的时刻。
    ///
    /// `Text ▸ Hide cursor when typing` 的判据：最近 500ms 内有输入就先不画光标。
    /// 记在**写入口**（[`Tab::write`]）而不是绘制时扫事件表：`write` 是键盘、
    /// 粘贴、同步输入、tmux、右键菜单那条粘贴路径的**唯一**漏斗，一处记下就
    /// 全都有了；而扫事件表既要知道哪些事件算输入，又会随浮层多出分支。
    ///
    /// 为什么是 `Cell` 而不是 `&mut self`：`write` 有九处调用点，其中
    /// `effects::send_command` / `PasteSelection` 手上只有 `&self.tabs.get(..)`。
    /// 「刚才敲过键盘」是一条瞬时观测值，不是会话状态——为了它把九处借用都改成
    /// 可变借用，等于让「读一下输入时刻」变成「必须独占整个标签」。
    pub(in crate::ui) last_input_at: std::cell::Cell<Option<std::time::Instant>>,
    /// 这条会话的显示方式：字符网格还是十六进制转储（`Mode ▸ Hex Edit`）。
    ///
    /// 归 tab 而不是 `ViewState`：同一个窗口里两个标签可以一个看日志、一个调
    /// 串口，要它们同时是 hex 没有道理。
    ///
    /// **出厂恒为 `Text`，切换不落盘**（用户定死的行为：「View 可以切当前会话的
    /// 显示方式，但是不会保存」）。所以没有「重开同名会话还是上次那个模式」——
    /// 那张按会话名索引的 `Prefs.session_modes` 已经删掉，留着就是第二个写者。
    /// 落地见 `ui/effects.rs::toggle_hex_mode`。
    pub(in crate::ui) mode: term_runtime::prefs::DisplayMode,
    /// 这条会话的 hex 显示参数：进制 / 列数 / 连写分组 / 加宽周期 / 斑马线。
    ///
    /// 两层模型里它属于**当前会话**这一层：初值在建标签时从
    /// `Prefs.hex_*` 与 `Prefs.alternate_column_bg`（设置 ▸ Binary 页，那份是
    /// 「默认参数」）拷一份，`View ▸ Hex *` 只改这一个标签、**不落盘**。
    ///
    /// 斑马线（`zebra`）也在这里，不是窗口上的一个 bool：它和那四个排版数
    /// 是同一个设置页上的同类参数，只能有一套语义——否则勾一下就会当场改掉
    /// 所有已开标签，而同页四个数字只影响新标签。
    ///
    /// 所以同一个窗口里两个标签可以各排各的：一个 16 进制 16 列看日志，
    /// 另一个 2 进制 8 列调串口。这也是它不放在 `TerminalWindow` 上的原因。
    pub(in crate::ui) hex: crate::ui::terminal::HexParams,
    /// 这条会话的**空白与符号可视化**参数：行尾 `$` / 参考列 / 缩进参考线 /
    /// 折行标记 / 五档空白符号 / 光标行底色。
    ///
    /// 与 [`Tab::hex`] 完全同一套两层模型：初值在建标签时从 `Prefs` 的
    /// `view_*` / `text_highlight_caret_line` 拷一份（设置页写的那份是「默认
    /// 参数」），`View ▸ *` / `Text ▸ HighlightCaretLine` 只改这一个标签、
    /// **不落盘**。所以同一个窗口里两个标签可以一个画参考线一个不画。
    ///
    /// 为什么不放 `ViewState`：它跟 `hex` / `mode` 一样是「每条会话一份」的显示
    /// 参数，进 `ViewState` 就变成整窗一份——分屏时两个窗格只能同开同关。
    /// 出厂六项全关，五张视觉基线靠这一条守住。
    pub(in crate::ui) marks: crate::ui::terminal::MarksParams,
    /// 这条会话的**调试覆盖层**开关（`Mode ▸ Debug Mode`）。
    ///
    /// 与 [`Tab::mode`] / [`Tab::hex`] / [`Tab::marks`] 完全同一套两层模型：归 tab
    /// 而不是 `ViewState`，**出厂 false，切换不落盘**——所以没有「上次那个状态」，
    /// 重启后新窗口一律是关的。落地见 `ui/effects.rs::toggle_debug_mode`。
    ///
    /// 它**不进** `DisplayMode`：那是**互斥排版**（`pane_body.rs` 的 if/else 二选
    /// 一），而调试覆盖是**叠加**——两者正交，能同时成立。`bin` 是一个被 13 处
    /// 分叉消费的派生 bool，往那个枚举加第三态等于让 13 处每一处重新回答「Debug
    /// 算不算 bin」，而调试覆盖层一处都不用改。
    ///
    /// 读者只有一个：`ui/terminal/pane_overlays.rs` 的 `if !bin && tab.debug`，
    /// 它去调 `debug_paint::paint`。`bin` 下整块跳过（hex 视图没有字符格坐标系）。
    pub(in crate::ui) debug: bool,
    /// 本标签是不是**只读查看**态（`Window ▸ Local Mode`，2026-11-19）。
    ///
    /// 只读 = 四条一起成立，缺一条就不叫只读：
    ///
    /// 1. **不写 PTY**：[`Tab::write`] 与 [`Tab::write_paste`] 直接早退，键盘 /
    ///    粘贴 / 鼠标上报 / 工具面板 / 快捷命令一条都发不出去；
    /// 2. **不自动滚动**：视口钉在 [`Tab::local_hold`] 那个绝对行上，新输出只进
    ///    历史、不把画面推走（用户自己滚动照常，滚到哪就冻在哪）；
    /// 3. **触发器不执行**：`poll` 跳过整个求值段，`SendText` / `RunCommand`
    ///    这类副作用一条都不落地；
    /// 4. **脚本不执行**：新脚本不让起，已经在跑的当场取消。
    ///
    /// 归 tab 而不是 `ViewState`：同一个窗口里可以一个标签冻着看日志、另一个
    /// 照常敲，进 `ViewState` 就变成整窗只读。**不落盘**——与 `debug` 同款：
    /// 只读是临时查看态，重启后一律回到可输入，否则用户会以为终端坏了。
    pub(in crate::ui) local_mode: bool,
    /// 本标签当前**折起来**的区间集合，坐标是历史绝对行（`HistoryRow`）。
    ///
    /// 归 tab 而不是 `ViewState`：与 `mode` / `hex` / `marks` 同一条理由——同一个
    /// 窗口里两个标签可以一个收着输出、一个摊开看，进 `ViewState` 就变成整窗一份。
    ///
    /// 折的是哪一段不由用户手选，而由**输入行**推导（[`Tab::fold_inputs`] +
    /// `term_app::folds::blocks_from_inputs`）：两次输入之间那段输出是一个块，
    /// 命令行本身是块的折叠头，永远可见。所以这里存的是「已经折了哪些块」，
    /// 不是「打算折哪些」——块是从输入行算出来的，本字段只记用户的实际动作。
    pub(in crate::ui) folds: term_app::folds::Folds,
    /// 用户在本标签里敲过回车的那些行（历史绝对行，升序去重）。
    ///
    /// [`Tab::fold_inputs`] 的第二数据源：`snapshot.blocks`（OSC 133）为空——也就是
    /// shell 没装提示符集成——时靠它兜底，否则折叠在默认配置下永远无块可折，
    /// 那九条命令就又变成按了没反应的假开关。
    ///
    /// 用 `RefCell` 的理由与 [`Tab::last_input_at`] 的 `Cell` 相同：[`Tab::write`]
    /// 有九处调用点，其中若干只有 `&self`。记一行输入不是会话状态，是一条观测值。
    pub(in crate::ui) input_rows: std::cell::RefCell<Vec<term_core::coords::HistoryRow>>,
    /// 这个标签从**创建那一刻**起收到的原始字节流（hex 视图的唯一数据源）。
    ///
    /// 与 `mode` 无关，这是本轮设计的关键：tap 在 `tab_from` 里无条件挂上，
    /// 环的寿命 = 标签的寿命。切 Text/Bin 只换渲染器——既不重挂 tap，也不清环。
    /// 想改成「切到 Bin 才挂 / 才清」的，先读 `bin_spec` 模块头那段：那正是
    /// 「切过去屏幕是空的」那个 bug 的成因。
    pub(in crate::ui) raw: crate::ui::terminal::bin_spec::RawLog,
    /// B2：本 tab 的 tmux 控制模式状态。只喂本会话的字节流，
    /// 普通会话全 Passthrough 不进 active，tmux 会话 `%begin` 自动置位。
    pub(in crate::ui) tmux: term_app::tmux::ControlMode,
    /// 跨 chunk 行缓冲：tmux 控制行可能被任意切成块，这里攒到 `\n` 才算一行。
    pub(in crate::ui) tmux_pending: Vec<u8>,
    /// D1：本 tab 的触发器求值器。与会话同生命周期（`poll` 里按 Connect/Disconnect
    /// 事件 reset），跨 chunk 的尾巴 / 冷却表 / 一次性事件都留在这里。规则本身
    /// 来自 `Prefs`（经窗口缓存成 `TerminalWindow.triggers`），不进 per-tab 状态。
    pub(in crate::ui) triggers: Matcher,
    /// R-M3：触发器双轨的影子求值器。默认只 eval 不 dispatch（零副作用），
    /// `WINDTERM_AUTOMATION_V2` 在场时新轨的 Effect 才会经 dispatcher 执行。
    /// 规则在 `poll` 首帧由旧规则转换装入，与会话同生命周期。
    pub(in crate::ui) shadow: crate::automation::Shadow,
    /// D1：上一帧会话是否存活。仅用于在 `poll` 里检测 Connect / Disconnect 跳变——
    /// 跳变时 reset 求值器，保证「重连后 Connect 再次触发、断开后 Disconnect 再次触发」。
    pub(in crate::ui) was_live: bool,
    /// 本帧是不是「刚刚连上」的那一帧。`poll` 在 Connect 跳变处置位，窗口层
    /// 用 `take_just_connected()` **取走并清掉**。
    ///
    /// 为什么不用 `log_status().phase == Off` 代替：用户手动 `Stop` 之后那一条
    /// 每一帧都是 `Off`，按状态判会立刻自动重开，变成界面与自己打架。
    /// 边沿只来一次，这才是「连上时按设置开一份」的正确时点。
    pub(in crate::ui) just_connected: bool,

    // ── N：恢复时注入的运行态（持久化只存，不主动读；restore_layout 里写入）──

    /// 待恢复的 scroll_top（0 = 历史顶部，history_len = 贴底）。
    /// `restore_layout` 从 `SavedTab.scroll_top` 读入；`Tab::poll` 中首帧应用后清 None。
    pub(in crate::ui) restore_scroll_top: Option<u64>,
    /// 待恢复的 history_len（行数）。
    /// `restore_layout` 从 `SavedTab.history_len` 读入；首帧应用时给 `viewport.clamp()` 用。
    pub(in crate::ui) restore_history_len: Option<u64>,
}

/// 「刚敲过键盘」的静默期：这段时间内不画光标。
///
/// 500ms 是「手在打字」与「手停下来看输出」的分界：短了打字时光标照闪（等于没
/// 开），长了每次停顿都要等半秒才看到光标回到眼前。
pub(in crate::ui) const TYPING_QUIET: std::time::Duration = std::time::Duration::from_millis(500);

impl Tab {
    /// 折叠算块用的输入行（历史绝对行，升序）。
    ///
    /// 两个数据源，**优先 shell 的 OSC 133**：提示符集成报上来的 `command_start`
    /// 是权威的，行号由内核在标记落点时记下，不受换行 / 重排影响；没装集成时
    /// `snapshot.blocks` 是空表，退回 [`Tab::note_input_lines`] 逐次攒下的那几行。
    ///
    /// 为什么不合并两份：两边都会给出「命令行所在行」，合并要靠行距去重，而
    /// 两条相邻输入行在折叠语义里是**合法**的（中间没有输出，不产生块），
    /// 去重时把真的相邻两条并掉，或者把同一条拆成两条，都是错的。取其一更稳：
    /// 有权威源就不猜。
    pub(in crate::ui) fn fold_inputs(&self) -> Vec<term_core::coords::HistoryRow> {
        let from_shell = term_app::blocks::command_rows(&self.snapshot.blocks);
        if from_shell.is_empty() {
            self.input_rows.borrow().clone()
        } else {
            from_shell
        }
    }

    /// 光标所在的**历史绝对行**；快照的网格行先夹进本屏行数。
    ///
    /// 夹取是必须的：resize / 恢复的中间态里 `cursor.0` 可能指到比 `rows` 更远的
    /// 地方，那样算出来的绝对行会落在内容之外，折叠块就会凭空多出一段。
    ///
    /// 折叠族（`ui/effects/fold.rs`）也读这一份：它要判「光标落在哪个块里」，
    /// 换算口径必须与本文件逐字一致，所以那边调这个方法而不是自己再写一遍
    /// （两份 `history + min(cursor, rows - 1)` 迟早会漂）。
    pub(in crate::ui) fn cursor_abs_row(&self) -> Option<term_core::coords::HistoryRow> {
        let grid = self.snapshot.cursor.0.min(self.snapshot.rows.saturating_sub(1));
        term_core::coords::HistoryRow::new(self.snapshot.history as i64 + grid as i64)
    }

    /// 记下「用户刚敲了回车」——命令行就在光标现在这一行。
    ///
    /// 调用点在键盘漏斗里 `input_line.feed` 之后。一次回车最多完成一行输入
    /// （回车键只有一个字节），所以这里固定记一行，不需要按行数摊开。
    ///
    /// # 为什么记光标行
    ///
    /// 回车那一瞬间光标还停在刚敲完的命令行上（shell 的回应是之后才到的），
    /// 所以「输入行行号」就是此刻的光标绝对行。往后每来一字节输出，这一行只会
    /// 往历史里沉，绝对行号不变——这正是折叠块需要的性质。
    ///
    /// # 为什么跳过备用屏
    ///
    /// 备用屏（`snapshot.alt`）里跑的是一次全屏 TUI：`vim` 里按回车是换行，
    /// `less` 里是翻页，都不是「提交一条命令」。照记不误的话，用户在全屏程序里
    /// 敲一阵就会攒出一串以随机位置为头的假块，`Fold All` 一把折下去全是乱的。
    pub(in crate::ui) fn note_input_line(&self) {
        if self.snapshot.alt {
            return;
        }
        if let Some(row) = self.cursor_abs_row() {
            self.push_input_row(row.get());
        }
    }

    /// 记下「刚粘贴进来 `lines` 行」。
    ///
    /// 与 [`Tab::note_input_line`] 的分工只在**方向**：键盘回车时内容已经回显完，
    /// 光标在命令行之**后**；粘贴时字节刚写进 PTY、回显还没回来，光标还在粘贴段
    /// 的第一行上。所以这里从光标行**往下**数。
    ///
    /// 数错方向的后果不是少记一行，而是整段记到上一屏去：多行粘贴后 `Fold All`
    /// 会折出一堆与内容对不上的块。
    pub(in crate::ui) fn note_pasted_lines(&self, lines: usize) {
        if lines == 0 || self.snapshot.alt {
            return;
        }
        let Some(first) = self.cursor_abs_row() else {
            return;
        };
        for i in 0..lines as i64 {
            let Some(raw) = first.get().checked_add(i) else {
                break;
            };
            self.push_input_row(raw);
        }
    }

    /// 追加一行输入行号：只在**严格变大**时追加。
    ///
    /// `input_rows` 必须升序去重——折叠算块靠相邻两行的差，重复或倒序会让
    /// `blocks_from_inputs` 跳过整对，块就少了一个。
    fn push_input_row(&self, raw: i64) {
        let Some(row) = term_core::coords::HistoryRow::new(raw) else {
            return;
        };
        let mut rows = self.input_rows.borrow_mut();
        if rows.last().is_none_or(|prev| *prev < row) {
            rows.push(row);
        }
    }

    /// 折叠区间换算成**视口行**（`ViewRow` 的裸值），渲染与导航共用这一份。
    ///
    /// 折叠存的是历史绝对行（滚动不漂），而渲染、鼠标、`viewport.jump_to_row`
    /// 说的都是视口行（`0` = 贴底时的屏首）。换算只走 `term_core::coords` 这一个
    /// 出口——本函数存在的意义就是不让第二份 `row - history` 出现在 UI 里。
    ///
    /// 返回 `(start, end)` 闭区间对，升序（`Folds` 保证区间有序不重叠）。
    pub(in crate::ui) fn fold_view_regions(&self) -> Vec<(i64, i64)> {
        let history = self.snapshot.history as u64;
        self.folds
            .regions()
            .iter()
            .map(|r| {
                (
                    term_core::coords::history_to_view(r.start(), history).get(),
                    term_core::coords::history_to_view(r.end(), history).get(),
                )
            })
            .collect()
    }

    /// 光标因「刚敲过键盘」还要藏多久。`None` = 现在就该画。
    ///
    /// 用 `Instant` 而不是 egui 的 `i.time`：写入口（`Tab::write`）手上没有
    /// `ui`，而这个差值只需要「两个时刻之间的差」，不参与任何需要可复现的
    /// 决策（截图夹具里没有输入，永远是 `None`）。
    pub(in crate::ui) fn typing_pause_left(&self) -> Option<std::time::Duration> {
        let at = self.last_input_at.get()?;
        let elapsed = at.elapsed();
        (elapsed < TYPING_QUIET).then(|| TYPING_QUIET - elapsed)
    }

    // ── N：恢复运行态（首帧应用，仅执行一次）──

    /// 若有待恢复的 scroll_top/history_len，应用到 viewport 并清空。
    ///
    /// - `restore_scroll_top`：视口顶端在历史中的绝对行号（0 = 顶部，history_len = 贴底）
    /// - `restore_history_len`：该 tab 保留的历史行数
    /// - 调用时机：存活会话在 `tab_from` 后立即调用；断开会话在 `poll_activation_connect` 连上后调用
    /// - 只执行一次：应用后把两个字段设为 `None`，防止重复应用
    pub(in crate::ui) fn maybe_apply_restore(&mut self) {
        let Some(scroll_top) = self.restore_scroll_top.take() else {
            return;
        };
        let history_len = self.restore_history_len.take().unwrap_or(0);
        // 从 scroll_top + history_len 换算 scroll
        self.viewport.apply_scroll_top(scroll_top, history_len);
        // history_len 作为上限给 viewport.clamp() 用（内核历史可能比保存时短）
        self.viewport.clamp(history_len as i64);
    }
}

/// RAII 兜底：`Tab` 被丢弃时把会话一起收口。
///
/// # 为什么现在才加
///
/// 蓝图 §4.2 定的是「先保留可证伪回归、后上 RAII」。P0-1 / P0-1b / P0-1c(D) 已经
/// 把四条显式关闭路径（`close_tab` / `close_all_tabs` / `delete_session_config` /
/// `replace_tabs`）逐一改成「先 `close()` 再丢」，每条都有自己的可证伪测试，前提
/// 因此成立：这一层只负责「以后谁写了第五条路径、忘了 `close()`」。
///
/// # 为什么不会重复关
///
/// [`Tab::close`] 是幂等的：`session.take()` 之后 `session` 是 `None`，再调不会
/// 走到 `pool.close`。所以「显式 close + Drop」双保险的代价只有一次
/// `raw.detach()`（它自己也是幂等的）。`pool.close` 对不在册的 id 返回 `false`，
/// 不会 panic。
///
/// # 为什么要先看 `panicking()`
///
/// `SessionPool::detach_tap` / `close` 内部对表锁 `.expect("sessions 锁")`。panic
/// 展开期间若那把锁已被毒化（panic 正是在持锁的临界区里发生的），Drop 里的
/// `.expect` 会二次 panic——**Drop 中的二次 panic 是 `abort`**，等于把一次可恢复
/// 的 panic 升级成整个进程硬死，正在写的日志也跟着毁。进程本来就要退了，漏掉这
/// 一次回收是可接受的代价，换成 abort 则不是。
impl Drop for Tab {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        self.close();
    }
}

// `Drop for Tab` 的回归单测单独放一个文件，与 `shell/tab_strip_lifecycle_tests.rs`
// 同一套做法（`#[path]` + `lifecycle_test_support` 共享夹具）：本文件只描述数据、
// 不装测试帮手，四条用例连同注释塞进来会让它读起来像两件事。
#[cfg(test)]
#[path = "tab_drop_tests.rs"]
mod drop_tests;
