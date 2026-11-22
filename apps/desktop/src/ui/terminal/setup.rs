//! 终端标签的构造与事实窗口计算（从 `ui/terminal.rs` 拆出）。
//!
//! 要求 `ui/` 单文件 ≤ 450 行；分屏把终端绘制参数化后 terminal.rs 涨到 466 行，
//! 于是把「谁构造 tab / 谁算事实窗口」这一组搬到这里。**只搬代码，不改行为**——
//! 五张视觉基线逐字节一致仍是判据。`ui/terminal.rs` 用 `pub(in crate::ui) use` 重新导出，
//! 所有调用点一处都不用改。

use super::hex_params::HexParams;
/// `MarksParams` / `WhitespaceMode` 走 `mod.rs` 的再导出（与 `HexParams` 同一
/// 条路径）：菜单那一侧（`ui/effects.rs`）也只认这一条，两处引用同一个名字，
/// 将来挪文件只需改 `mod.rs` 一行。
use super::{MarksParams, WhitespaceMode};
use crate::ui::*;
use term_core::session_runtime::SessionId;

/// ADR-0025：标签恒在身份铸造源。每条 tab 在 `tab_from` 一次性领取，
/// 与存活态（`session`）解耦——断连 / 重连都不回收、不改值。进程内单调递增，
/// 身份只在单次运行内比较（`restore_layout` 重建标签会重新领号，不保证跨运行稳定）。
static TAB_IDENTITY_SRC: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 给一条新建标签铸造恒在身份（见 [`TAB_IDENTITY_SRC`]，ADR-0025）。
pub(in crate::ui) fn mint_tab_identity() -> SessionId {
    TAB_IDENTITY_SRC.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// 统一的 tab 构造入口：起不来会话时仍给出空网格，UI 不会崩。
///
/// 会话由**控制面**起（ADR-0019）：`pool.open` 失败时 tab 照样成立，
/// 只是 `session = None`——界面上就是「断开」的那一副样子，而不是一个
/// 半死的句柄。
///
/// 这里顺手把**唤醒回调**注入会话——没有它，PTY 的输出到了也没人叫醒
/// 界面（`draw()` 只在有输入事件时才跑，`poll()` 也因此不会被调用）。
pub(in crate::ui) fn make_tab(
    pool: &std::sync::Arc<term_session::SessionPool>,
    config: SessionConfig,
    id: egui::Id,
    ctx: &egui::Context,
) -> Tab {
    let session = pool.open(&config, 80, 24).ok();
    tab_from(pool, config, session, id, ctx)
}

/// 通道已经建好（原生 SSH）时开 tab。
///
/// 与 `make_tab` 的唯一区别是「谁打开它」：PTY 由控制面自己拉，SSH 由调用方
/// 在后台线程里连好、经 `open_with_channel` 注入。这正是
/// `SessionPool::open_with_channel` 存在的理由——协议差异留在
/// 调用方，控制面只负责监管。
pub(in crate::ui) fn make_tab_with_channel(
    pool: &std::sync::Arc<term_session::SessionPool>,
    config: SessionConfig,
    ch: Box<dyn term_runtime::channel::TermChannel>,
    id: egui::Id,
    ctx: &egui::Context,
) -> Tab {
    let session = Some(pool.open_with_channel(&config, 80, 24, ch));
    tab_from(pool, config, session, id, ctx)
}

/// 新标签的 hex 初值：四格排版数 + 斑马线，**全部取自同一份 `Prefs`**。
///
/// 抽成函数而不是写在 `tab_from` 里的理由只有一条：`tab_from` 自己要从盘上
/// `load()`，测试没法把一份夹具 prefs 喂进去（改 `WINDTERM_PROFILE_DIR` 会和
/// 同进程并行的其它测试互相干扰，更别说可能覆盖真实用户目录）。有了这个入口，
/// 「新标签是不是真的按 prefs 取值」就能用一份手造的 `Prefs` 直接验证。
///
/// 它是**唯一**的建标签初值入口：`tab_from` 是它唯一的生产调用方。谁要在建
/// 标签路上再加一个参数（比如将来多一格开关），加在这里就自动被两条东西守住：
/// 这条测试与 `HexParams::from_prefs` 的夹取。绕开它直接在 `Tab { hex: … }`
/// 里写字面量，新参数就会静默退回出厂值——那正是「设置页改了没反应」的形状。
pub(in crate::ui) fn hex_defaults(prefs: &term_runtime::prefs::Prefs) -> HexParams {
    HexParams::from_prefs(
        prefs.hex_base,
        prefs.hex_columns,
        prefs.hex_group,
        prefs.hex_division,
        prefs.alternate_column_bg,
    )
}

/// 新标签的**空白与符号可视化**初值：六项全部取自同一份 `Prefs`。
///
/// 与 [`hex_defaults`] 并列而不是合并成一个 `TabParams`：两者的作用域虽然都是
/// 「每条会话一份」，但来源字段、夹值规则、菜单入口完全不同——合并只会造出一个
/// 谁都不敢删的大结构体。这里的「唯一入口」要求是一样的：`tab_from` 是它唯一的
/// 生产调用方，谁在建标签路上加了第七个开关，加在这里就自动被
/// [`MarksParams::from_prefs`] 的夹值与 `setup::tests` 那条用例守住。
///
/// 六项出厂全关，所以「老文件没有这些键」与「用户一个都没开」在屏幕上是同一种
/// 样子——这正是 `#[serde(default)]` 该给的结果。
pub(in crate::ui) fn marks_defaults(prefs: &term_runtime::prefs::Prefs) -> MarksParams {
    MarksParams::from_prefs(
        prefs.view_line_endings,
        prefs.view_column_marker,
        prefs.view_indent_guides,
        prefs.view_wrap_symbols,
        prefs.view_whitespace,
        prefs.text_highlight_caret_line,
    )
}

/// 三条建标签路径的共同落点：会话已经拿到（或没拿到）之后的全部装配。
///
/// `pub(in crate::ui)` 是给 ADR-0025 的**一条**额外调用路径开的：脚本线程
/// 已经建好会话（`session_open`），界面要绑到**同一条**——再 `pool.open` 一次
/// 就会多出一条谁也看不见的重复会话。所以 `absorb_script_opened` 直接调它、
/// 传 `Some(session)`，而不是另抄一份 `Tab { … }` 字面量：手抄的那份漏掉
/// 唤醒回调或 `hex` / `marks` 的 prefs 初值时，症状是「新标签页不刷新」。
pub(in crate::ui) fn tab_from(
    pool: &std::sync::Arc<term_session::SessionPool>,
    config: SessionConfig,
    session: Option<term_core::session_runtime::SessionId>,
    id: egui::Id,
    ctx: &egui::Context,
) -> Tab {
    let snapshot = session
        .and_then(|sid| pool.snapshot(sid))
        .unwrap_or_else(|| GridSnapshot::empty(80, 24));
    // ADR-0025：铸造恒在身份，与存活态无关——断连 / 重连都不改值。
    let identity = mint_tab_identity();
    // 建标签时从 prefs 拷一份**默认参数**：四个数与斑马线都由 设置 ▸ Binary 页
    // 写，`0` 哨兵与越界值在 `from_prefs` 一处夹掉。之后 `View ▸ Hex *` 改的是
    // 这份副本（`Tab.hex`），不落盘——所以下次开的标签又回到这里的默认值。
    //
    // 建标签是用户节奏的操作，为此读一次盘不值得另开一条缓存通路
    // （那要跟着 prefs 的每一次改动一起同步）。
    let prefs = term_runtime::prefs::load().unwrap_or_default();
    let hex = hex_defaults(&prefs);
    let marks = marks_defaults(&prefs);
    let tab = {
        let mut tab = Tab {
            pool: pool.clone(),
            session,
            identity,
            config,
            view: snapshot.to_screen_image(),
            snapshot,
            screen: None,
            viewport: Viewport::new(),
            selections: Selections::default(),
            last_request: None,
            preview_screen: None,
            preview_last_request: None,
            id,
            auth: None,
            input_line: term_core::input_line::InputLine::new(),
            // 新标签还没敲过键盘：`Hide cursor when typing` 一开就能正常画光标。
            last_input_at: std::cell::Cell::new(None),
            // 出厂恒为文本：模式不落盘，就没有「上次那个模式」可回读。
            mode: term_runtime::prefs::DisplayMode::Text,
            hex,
            // 六项可视化参数同样从 prefs 拷一份**默认值**：设置页写的是
            // `Prefs.view_*`，`View ▸ *` 改的是这份副本、不落盘。
            marks,
            trigger_highlight_rows: Vec::new(),
            // 调试覆盖层与只读查看都出厂关：与 `marks` 一样是「每条会话一份」的
            // 显示状态、不落盘。夹具与生产都从这里起步，视觉基线逐字节不变。
            debug: false,
            local_mode: false,
            raw: crate::ui::terminal::bin_spec::RawLog::new(),
            // 控制模式状态机归本 tab，跨块行缓冲从空开始。
            tmux: term_app::tmux::ControlMode::default(),
            tmux_pending: Vec::new(),
            // 触发器求值器随标签建立，空状态开始；存活会话在 `poll` 首帧
            // 经 Connect 跳变 reset（这里先置 false，让首帧识别成「刚连接」）。
            triggers: term_app::trigger::Matcher::new(),
            // 影子求值器出厂空态（signature 哨兵在 Shadow::new 内）。
            shadow: crate::automation::Shadow::new(),
            was_live: false,
            just_connected: false,
            auto_reconnect_at: None,
            auto_reconnect_attempts: 0,
            auto_reconnect_blocked: false,
            // 折叠：新建标签还没折过任何一段，输入行表也是空的（第一批要等用户
            // 真的敲一次回车）。与 `marks` 一样是「每条会话一份」的显示状态。
            folds: term_app::folds::Folds::new(),
            input_rows: std::cell::RefCell::new(Vec::new()),
            // 恢复运行态字段——默认 None，由 restore_layout 写入
            restore_scroll_top: None,
            restore_history_len: None,
        };
        // **无条件**挂字节 tee：不看 `mode`，也不看用户以后会不会切到 Bin。
        //
        // 「切到 Bin 时才挂」是上一版设计的错：那样屏幕要等新数据才有内容，
        // 而用户点 Hex Edit 想看的恰恰是刚才那一段。历史的起点只能是标签
        // 建立那一刻，所以挂 tap 是建标签的一部分，不是切模式的一部分。
        tab.attach_tap();
        // 存活会话（本地 shell / 系统 SSH）在构造后立即应用恢复态
        // 断开会话（内置 SSH）稍后在 poll_activation_connect 连上后应用
        if tab.session.is_some() {
            tab.maybe_apply_restore();
        }
        tab
    };
    tab.set_wakeup(ctx);
    tab
}

/// 算出本帧要画的事实窗口，必要时向监管者请求范围事实。
///
/// 贴底时不请求，直接用快照派生（内容就是当前屏）；回看时请求
/// `-scroll .. -scroll + rows`，拿到后用 `crop()` 裁出正好一屏——
/// 手上的事实不够画这一屏就返回空白帧，下一帧补上，绝不「猜」内容。
///
/// # 折叠：一个额外的前置分支，老逻辑一个字都没改
///
/// 下面这一整段（从 `if tab.viewport.is_bottom()` 起）是**本次改动之前逐字节
/// 相同**的代码：折叠集合为空时它就是全部行为，五张视觉基线、所有渲染夹具都靠
/// 这条早退守住。折叠分支写在 [`folded_view`] 里，与本函数通过一个 `return` 相连，
/// 两条路径**没有**被参数化合并成一条——合并就意味着要在老路径上多算几个值，
/// 而「空折叠时一屏一个像素都不能变」是比复用更硬的约束。
pub(in crate::ui) fn ensure_view(tab: &mut Tab, rows: usize) -> ScreenImage {
    // 空折叠 = 老路径（下面整段未改动）。只有真的有折叠才进新分支。
    if !tab.folds.is_empty() {
        return folded_view(tab, rows);
    }
    if tab.viewport.is_bottom() {
        return tab.snapshot.to_screen_image();
    }
    let desired = tab.viewport.rows(rows);
    let version = tab.snapshot.version;
    let stale = match &tab.last_request {
        Some((range, v)) => range != &desired || *v != version,
        None => true,
    };
    if stale {
        tab.request_range(desired.clone());
        tab.last_request = Some((desired.clone(), version));
    }
    // 往下仍然是视口行的世界：`crop_view()` / `blank_view()` 都收 `ViewRange`，
    // 擦成裸值只发生在 core 内部那一次委托里，UI 这一侧不再出现 `to_i64()`。
    // 回看时范围请求异步在路上，这一帧正文没内容可画，但 history 是已知事实。
    // 空白帧必须带上真 history：边栏按 `history + line.row` 取绝对行，若填 0，
    // 负的 line.row 会算出负数行号、时间戳整列查不到，下一帧真数据到了又恢复
    // ——拖动滚动条时左侧数字 / 时间一闪一闪还带负数。
    // `blank_view_with_history` 让空白帧与真数据帧的绝对行号逐值相等。
    let history = tab.snapshot.history;
    match &tab.screen {
        Some(img) => img.crop_view(desired.clone()).unwrap_or_else(|| {
            ScreenImage::blank_view_with_history(tab.snapshot.cols, version, desired.clone(), history)
        }),
        None => ScreenImage::blank_view_with_history(tab.snapshot.cols, version, desired, history),
    }
}

/// 折叠生效时的取窗口路径：**向上多要几行，再把被折的行丢掉**。
///
/// 折叠要满足的硬约束是「屏幕仍然是满 `rows` 行」：被折掉 H 行之后，底部那 H 行
/// 不能变成黑洞，得由更靠上的历史行补进来。所以窗口不是 `base`，而是
/// `base.start - extra .. base.end`——`extra` 由 [`extra_rows_needed`] 定点迭代求出。
///
/// # 贴底为什么也要发范围请求（本次改动的实质）
///
/// 贴底窗口是 `0..rows`，折掉 H 行后底部必须由 `-H..0` 那段补，而快照里**没有**
/// 历史。所以贴底的请求范围变成 `-extra..rows`。`SessionPool` /
/// `supervisor::slice_view` 本来就支持任意 `ViewRange`（回看路径一直在用），
/// 内核一行都不用改。
///
/// # 事实还没到时退回「不折的画面」
///
/// 范围请求是异步的：某一帧里 `tab.screen` 可能是 `None`、也可能覆盖不到新窗口
/// （`extra` 变了、窗口因此变长）。这时**不折**——贴底给快照整屏，回看给空白帧，
/// 下一帧图到了折叠自然生效。反过来「给一帧全黑」会让每一次折叠都闪一下黑屏，
/// 那比晚一帧生效糟得多。
fn folded_view(tab: &mut Tab, rows: usize) -> ScreenImage {
    use term_core::coords::{ViewRange, ViewRow};

    let folds = tab.fold_view_regions();
    let base = tab.viewport.rows(rows);
    let extra = extra_rows_needed(&folds, base.start().get(), base.end().get(), rows);
    // `saturating_sub`：`base.start()` 是 `-scroll`，正常远不到 `i64::MIN`；
    // 这里只是不肯让一个纯几何计算在极端输入上 panic（生产路径零 panic）。
    let needed = ViewRange::new(
        ViewRow::new(base.start().get().saturating_sub(extra)),
        base.end(),
    );

    let version = tab.snapshot.version;
    let stale = match &tab.last_request {
        Some((range, v)) => range != &needed || *v != version,
        None => true,
    };
    if stale {
        tab.request_range(needed.clone());
        tab.last_request = Some((needed.clone(), version));
    }

    let Some(img) = tab
        .screen
        .as_ref()
        .and_then(|screen| screen.crop_view(needed.clone()))
    else {
        // 这一帧的事实还没到：退回不折的画面，而不是给一帧全黑。
        return if tab.viewport.is_bottom() {
            tab.snapshot.to_screen_image()
        } else {
            // 同上：折叠路径的空白兜底也带真 history，避免拖动时边栏闪负数。
            ScreenImage::blank_view_with_history(
                tab.snapshot.cols,
                version,
                base,
                tab.snapshot.history,
            )
        };
    };
    // 过滤必须是**最后一次**变换：`crop_view` 依赖行号连续，`drop_rows` 之后
    // 行号就带洞了（见 `ScreenImage::drop_rows` 的文档）。
    img.drop_rows(|row| hidden_by_fold(&folds, row))
}

/// 折叠窗口要向上多要几行——**纯函数**，定点迭代求最小的那个数。
///
/// 不变式：窗口 `[start - extra, end)` 里被折掉的行数必须**正好等于** `extra`，
/// 这样丢掉 `extra` 行之后剩下的可见行数仍是 `rows`（少掉的行由上方补进来）。
///
/// # 为什么是迭代而不是一遍算完
///
/// 「窗口里藏了几行」取决于窗口从哪开始，而窗口起点又取决于 `extra`——两个未知数
/// 互相依赖。例：块 `[-5, 1]`（头行 `-5`）、窗口 `0..10`：先按 `extra = 0` 数出
/// 藏了 2 行（`0`、`1`），于是窗口上移到 `-2`；上移之后 `-2`、`-1` 也进了窗口，
/// 藏的行变成 4 行 → 再上移到 `-4` → 6 行 → 上移到 `-6`，此时窗口里该藏的就是
/// 那 6 行，收敛（`extra = 6`，`-4..1` 六行被藏，`16 - 6 = 10 = rows`）。
///
/// 单调性：窗口只会往上变长，`hidden` 只增不减，所以从 0 起步的迭代必收敛，
/// 且给出的是**最小的**可行 `extra`（不会为了收敛随便多要几行把屏滚过头）。
///
/// 上限 `rows` 与迭代次数 `rows + 1` 都是防呆：真到那一步说明整屏都在折叠里，
/// 多要也没有更多可见内容可补，返回 `rows` 即可。
fn extra_rows_needed(folds: &[(i64, i64)], base_start: i64, base_end: i64, rows: usize) -> i64 {
    // `rows == 0`：没有屏幕可补，且下面所有以 `rows` 为上限的比较都失去意义。
    if rows == 0 || folds.is_empty() {
        return 0;
    }
    let cap = i64::try_from(rows).unwrap_or(i64::MAX);
    // 窗口是半开区间 `[start, end)`，闭区间口径的尾行是 `end - 1`。
    let last_row = base_end.saturating_sub(1);

    let mut extra: i64 = 0;
    // `rows + 1` 次一定够（每轮要么收敛、要么严格增大）。
    for _ in 0..=rows {
        let win_start = base_start.saturating_sub(extra);
        let hidden = hidden_rows_in(folds, win_start, last_row);
        if hidden == extra {
            return extra;
        }
        if hidden >= cap {
            return cap;
        }
        extra = hidden;
    }
    extra
}

/// `[win_start, last_row]`（闭区间）里有多少行**被折掉**。
///
/// 逐块求交集，**块首行不算**：块首行是折叠头（命令行本身），永远可见
/// （`Folds::hides` 的规则）。所以交集的下界是 `block_start + 1`。
fn hidden_rows_in(folds: &[(i64, i64)], win_start: i64, last_row: i64) -> i64 {
    let mut hidden: i64 = 0;
    for &(block_start, block_end) in folds {
        let lo = block_start.saturating_add(1).max(win_start);
        let hi = block_end.min(last_row);
        if lo <= hi {
            hidden = hidden.saturating_add((hi - lo).saturating_add(1));
        }
    }
    hidden
}

/// 视口行 `row` 是否**被折掉**：落在某个块内，且不是该块的首行。
///
/// 与 [`hidden_rows_in`] 必须是同一条判据（一处给「窗口里藏了几行」，一处给
/// 「这一行藏不藏」），否则补进来的行数与实际丢掉的行数会对不上，屏幕会少画
/// 或露白。两处都从 `(start, end)` 闭区间读，且都把 `start` 排除在外。
fn hidden_by_fold(folds: &[(i64, i64)], row: i64) -> bool {
    folds
        .iter()
        .any(|&(block_start, block_end)| block_start < row && row <= block_end)
}

// ── 空白/符号可视化：菜单路径的唯一落地点 ──────────────────────────────
//
// 十一个开关（六项参数 + 五档空白模式）全都只改**活动标签**的 `Tab.marks`，
// 一个字节都不落盘——用户定死的行为是「View 可以切当前会话的显示方式，但是
// 不会保存」，与 `View ▸ Hex *` 同一条规矩。
//
// 「不落盘」不是靠自觉：这段代码手上只有 `self.tabs` 与 `self.ctx`，既没有
// `Prefs` 也没有写盘入口，做不到。写盘的只有设置页（它写 `Prefs.view_*`，
// 那是「默认参数」，只影响之后新开的标签）。
//
// 为什么放在 `setup.rs` 而不是 `ui/effects.rs`：`effects.rs` 是**分派**层
// （命令 ID → 效应 → 落地），而这里与 `hex_defaults` / `marks_defaults` 一样是
// 「每条会话一份渲染参数」的两侧之一——建标签取初值、菜单改当前标签。两侧都在
// 这一个文件里，下一个人找「谁改了 tab.marks」只需要看一处。
impl TerminalWindow {
    /// 当前活动标签的可视化参数。没有标签时给出厂值（菜单据此画勾，不会 panic）。
    ///
    /// 菜单需要它来决定哪几个项要打勾：勾的状态必须是**当前标签**的，而不是
    /// `Prefs` 里那份默认值——否则用户在新标签上关掉参考线，菜单里的勾还亮着。
    pub(in crate::ui) fn active_marks(&self) -> MarksParams {
        self.tabs
            .get(self.active_slot())
            .map(|tab| tab.marks)
            .unwrap_or_default()
    }

    /// 五档空白模式的唯一入口（`View ▸ White Spaces` 五个菜单项）。
    ///
    /// 模式是**互斥**的，所以这里收一个枚举而不是五个 bool：不存在「两档同时开」
    /// 的中间态，绘制层也就不需要写优先级。
    pub(in crate::ui) fn set_whitespace_mode(&mut self, mode: WhitespaceMode) {
        self.set_marks(|m| MarksParams {
            whitespace: mode,
            ..m
        });
    }

    /// `View ▸ Toggle Line Endings`：逻辑行行尾那个 `$`。
    pub(in crate::ui) fn toggle_line_endings(&mut self) {
        self.set_marks(|m| MarksParams {
            line_endings: !m.line_endings,
            ..m
        });
    }

    /// `View ▸ Toggle Indent Guides`：行首缩进参考线。
    pub(in crate::ui) fn toggle_indent_guides(&mut self) {
        self.set_marks(|m| MarksParams {
            indent_guides: !m.indent_guides,
            ..m
        });
    }

    /// `View ▸ Toggle Wrap Symbols`：自动换行续行的折行标记。
    pub(in crate::ui) fn toggle_wrap_symbols(&mut self) {
        self.set_marks(|m| MarksParams {
            wrap_symbols: !m.wrap_symbols,
            ..m
        });
    }

    /// `Text ▸ Highlight Caret Line`：光标行那层淡底。
    pub(in crate::ui) fn toggle_caret_line(&mut self) {
        self.set_marks(|m| MarksParams {
            caret_line: !m.caret_line,
            ..m
        });
    }

    /// `View ▸ Toggle Column Marker`：开 = 第 80 列（出厂列号），再点 = 关。
    ///
    /// 开关与列号合成一个动作：菜单上只有一格勾，没有「设成第几列」的输入框。
    /// 列号夹值仍走 [`MarksParams::with_column_marker`]（`8..=512`）。
    pub(in crate::ui) fn toggle_column_marker(&mut self) {
        self.set_marks(|m| m.with_column_marker(if m.column_marker.is_some() {
            None
        } else {
            Some(super::marks::COLUMN_MARKER_DEFAULT)
        }));
    }

    /// 直接指定参考列（`None` = 关）。给「把当前列号设成参考列」这类入口用。
    ///
    /// 越界由 `with_column_marker` 一处夹掉：菜单传 `Some(3)` 会落成第 8 列，
    /// 传 `Some(9999)` 会落成第 512 列。
    pub(in crate::ui) fn set_column_marker(&mut self, col: Option<usize>) {
        self.set_marks(|m| m.with_column_marker(col));
    }

    /// `Effect::SetMarks` 的落点：十条栅格可视化命令的**唯一**分流处。
    ///
    /// 为什么把「翻转」与「设档」收在一个函数里：菜单那一侧只有一条
    /// `Effect::SetMarks(knob)`（reducer 不认识 `Tab.marks`，它手上一份状态都
    /// 没有），所以「这个 knob 是翻转还是设档」必须在这里回答一次。
    /// 分成十个 `Effect` 变体只会得到十份一模一样的 `set_marks`。
    ///
    /// 五档空白模式的编号来自 `Prefs.view_whitespace` 与 `WhitespaceMode::ALL`
    /// 的同一张顺序表；越界（手改过的 `prefs.json` 写个 `9`）由
    /// [`WhitespaceMode::from_index`] 回落 `HideAll` —— 屏幕不会出现某种
    /// 没人定义过的画法。
    pub(in crate::ui) fn set_marks_knob(&mut self, knob: term_app::shell::MarksKnob) {
        use term_app::shell::MarksKnob;
        match knob {
            MarksKnob::LineEndings => self.toggle_line_endings(),
            MarksKnob::ColumnMarker => self.toggle_column_marker(),
            MarksKnob::IndentGuides => self.toggle_indent_guides(),
            MarksKnob::WrapSymbols => self.toggle_wrap_symbols(),
            MarksKnob::CaretLine => self.toggle_caret_line(),
            MarksKnob::Whitespace(index) => {
                self.set_whitespace_mode(WhitespaceMode::from_index(usize::from(index)));
            }
        }
    }

    /// 六个落地点共用的那一句：改**活动标签**，然后请一次重绘。
    ///
    /// 没有活动标签时静默——菜单能在没有标签的窗口里点到，那时候什么也不该发生
    /// （`tabs.get_mut` 返回 `None` 就结束，不 panic）。
    ///
    /// 必须 `request_repaint`：egui 空闲时不重画，只在有事件时才刷一帧。少了这句，
    /// 点完菜单要等下一次鼠标移动才看见效果——那与「点了没反应」是同一观感。
    pub(in crate::ui) fn set_marks(&mut self, edit: impl FnOnce(MarksParams) -> MarksParams) {
        let active = self.active_slot();
        let Some(tab) = self.tabs.get_mut(active) else {
            return;
        };
        tab.marks = edit(tab.marks);
        self.ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 「切过去屏幕是空的」那个 bug 的单测版：**Text 期间也在搬字节**。
    ///
    /// 这里不测绘制（那要窗口），测的是那个决定性判断——`Tab::poll` 每帧把
    /// tee 里的字节搬进环，且**不看 `mode`**。缺了这一条，用户切到 Bin 时
    /// 看到的历史会从「切过去那一刻」开始，而这恰恰是肉眼最容易误判的情形：
    /// 数据正在流动时，屏幕上照样有字，只是少了前面那一截。
    #[test]
    fn bytes_keep_arriving_in_text_mode_so_bin_is_never_empty() {
        use term_runtime::prefs::DisplayMode;

        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let ctx = egui::Context::default();
        let mut tab = tab_from(
            &pool,
            SessionConfig::new(Protocol::Shell),
            None,
            egui::Id::new("hex-tap-test"),
            &ctx,
        );
        // 夹具里没有会话可挂（`session` 传的是 `None`），于是手挂一个接收端：
        // 测的是同一条搬运路径（`RawLog::drain` ← `Tab::poll`），不是另一条。
        let (tx, rx) = std::sync::mpsc::channel();
        tab.raw.attach(crate::hex_tap::HexTap::new(rx));
        assert_eq!(tab.mode, DisplayMode::Text, "出厂是文本模式");

        tx.send(b"first chunk".to_vec()).expect("送第一块");
        assert!(tab.poll(&[], &mut Vec::new(), false), "环里进了字节，就该说「有新事实」");
        assert_eq!(tab.raw.ring.len(), 11, "Text 模式下字节也必须进环");

        // 切到 Bin：不清环、不重挂 tap，只换一种画法。历史必须接着算。
        tab.mode = DisplayMode::Bin;
        tx.send(b" more".to_vec()).expect("送第二块");
        assert!(tab.poll(&[], &mut Vec::new(), false));
        assert_eq!(tab.raw.ring.len(), 16, "切模式不清历史，新字节接着攒");
        assert_eq!(tab.raw.ring.take(0, 16), b"first chunk more".to_vec());
    }

    /// 新标签的两件事：排版初值**取自 prefs**，模式**恒为 Text**。
    ///
    /// 这是两层模型的另一半（另一半在 `hex_params::a_knob_moves_only_its_own_dimension`）：
    ///
    /// - 排版初值来自 `Prefs.hex_*`（设置 ▸ Binary 页写的「默认参数」），
    ///   所以「设置页改 24 列 → 新开标签是 24 列」成立；
    /// - `Tab.mode` 写死 `Text`：模式**不落盘**，就没有「上次那个模式」可回读。
    ///   这条曾经是 `Prefs::session_mode(&config.name)`，那张表已整条删掉。
    ///
    /// 不设 `WINDTERM_PROFILE_DIR`：这里拿**当前环境**的 prefs 当期望值，
    /// 于是无论测试跑在谁的用户目录下，比较的都是「读出来的那份」——
    /// 改环境变量会和同一进程里并行跑的其它测试互相干扰，那才是真的不稳。
    #[test]
    fn a_new_tab_takes_the_hex_defaults_from_prefs_and_starts_in_text_mode() {
        use term_runtime::prefs::DisplayMode;

        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let ctx = egui::Context::default();
        let tab = tab_from(
            &pool,
            SessionConfig::new(Protocol::Shell),
            None,
            egui::Id::new("hex-defaults-test"),
            &ctx,
        );

        let prefs = term_runtime::prefs::load().unwrap_or_default();
        // 初值 = 同一份 prefs 过同一个 `from_prefs`：0 哨兵与越界值都在那儿夹。
        // 走生产那条 `hex_defaults` 而不是在这里再拼一次五个参数——拼第二次
        // 就等于给「新标签取哪些参数」造了第二套定义，漏一项也照样绿。
        let want = hex_defaults(&prefs);
        assert_eq!(tab.hex, want, "排版初值必须来自 Prefs.hex_*");
        assert_eq!(
            tab.hex.zebra, prefs.alternate_column_bg,
            "斑马线也是默认参数：新标签取的是设置页写的值"
        );
        assert_eq!(tab.mode, DisplayMode::Text, "模式不落盘，新标签一律文本");
    }

    /// 新标签的六项可视化参数也**取自同一份 prefs**，且出厂全关。
    ///
    /// 与上一条同一个道理：建标签那条路只有 `marks_defaults` 一个入口，所以
    /// 「设置页改了 `view_indent_guides`，新标签就带参考线」不会因为漏一个字段
    /// 而静默失效。后半段断言「出厂全关」是视觉基线的前提——六项里任何一项默认
    /// 打开，五张基线都会当场变样。
    #[test]
    fn a_new_tab_takes_the_marks_defaults_from_prefs() {
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let ctx = egui::Context::default();
        let tab = tab_from(
            &pool,
            SessionConfig::new(Protocol::Shell),
            None,
            egui::Id::new("marks-defaults-test"),
            &ctx,
        );
        let prefs = term_runtime::prefs::load().unwrap_or_default();
        assert_eq!(
            tab.marks,
            marks_defaults(&prefs),
            "六项初值必须来自 Prefs.view_* / text_highlight_caret_line"
        );
        // 出厂 prefs（老文件缺键时读到的也是它）六项全关。
        let factory = marks_defaults(&term_runtime::prefs::Prefs::default());
        assert_eq!(factory, MarksParams::default(), "出厂六项全关");
        assert!(!factory.any_enabled());
    }

    /// 菜单那六个落地点只改**活动标签**，且都是「再点一次回到原样」。
    ///
    /// 这条钉的是两层模型里「菜单改当前会话、不落盘」那一半：`set_marks` 手上
    /// 没有 `Prefs`，所以这里能测的只是「值改对了、别的标签没被连坐」，
    /// 而「没写盘」由签名保证。
    #[test]
    fn the_marks_toggles_only_touch_the_active_tab() {
        use crate::ui::lifecycle_test_support::window_with;

        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let ctx = egui::Context::default();
        let first = tab_from(
            &pool,
            SessionConfig::new(Protocol::Shell),
            None,
            egui::Id::new("marks-toggle-1"),
            &ctx,
        );
        let second = tab_from(
            &pool,
            SessionConfig::new(Protocol::Shell),
            None,
            egui::Id::new("marks-toggle-2"),
            &ctx,
        );
        let mut window = window_with(pool, &ctx, vec![first, second]);
        let untouched = window.tabs[1].marks;

        window.toggle_line_endings();
        window.toggle_indent_guides();
        window.toggle_wrap_symbols();
        window.toggle_caret_line();
        window.toggle_column_marker();
        window.set_whitespace_mode(WhitespaceMode::InSelection);

        let marks = window.active_marks();
        assert!(marks.line_endings && marks.indent_guides && marks.wrap_symbols);
        assert!(marks.caret_line);
        assert_eq!(
            marks.column_marker,
            Some(super::super::marks::COLUMN_MARKER_DEFAULT),
            "开关打开时落在出厂列号"
        );
        assert_eq!(marks.whitespace, WhitespaceMode::InSelection);
        assert_eq!(window.tabs[1].marks, untouched, "另一个标签的六项一个字都不许动");

        // 再点一次全部回到关：勾掉与勾上是同一句话的反面。
        window.toggle_line_endings();
        window.toggle_indent_guides();
        window.toggle_wrap_symbols();
        window.toggle_caret_line();
        window.toggle_column_marker();
        window.set_whitespace_mode(WhitespaceMode::HideAll);
        assert_eq!(window.active_marks(), MarksParams::default());
    }

    // ── 折叠渲染：补行算法（`extra_rows_needed`）──────────────────────────
    //
    // 全部按「窗口 `start..end` 是视口行、`rows` 是一屏行数」的口径手算，
    // 期望值都写成算式旁边的注释，改算法的人能一眼对出是哪里变了。

    /// 没有折叠 → 一行都不用多要。这条挂了说明 `extra` 无中生有，屏幕会整体上滚。
    #[test]
    fn extra_rows_needed_is_zero_without_folds() {
        assert_eq!(extra_rows_needed(&[], 0, 10, 10), 0);
        // 回看窗口同理：起点是负数也一样，没有折叠就没有额外行。
        assert_eq!(extra_rows_needed(&[], -40, -30, 10), 0);
    }

    /// 块完全落在窗口内：藏几行就补几行，一轮收敛（头行 5 不算，只有 6、7 被藏）。
    #[test]
    fn extra_rows_needed_counts_a_block_inside_the_window() {
        // [5,7] → 窗口 0..10 内被藏的是 6、7（5 是折叠头）→ extra = 2。
        assert_eq!(extra_rows_needed(&[(5, 7)], 0, 10, 10), 2);
    }

    /// 块跨窗口**上边界**：一轮迭代算不够，必须迭代到不动点。
    ///
    /// 逐步（窗口 `0..10`、`rows = 10`、块 `[-5, 1]`，头行 `-5` 不算）：
    /// - `extra = 0` → 窗口 `0..10`，藏 `0`、`1` → `hidden = 2`；
    /// - `extra = 2` → 窗口 `-2..10`，藏 `-2..=1` → `hidden = 4`；
    /// - `extra = 4` → 窗口 `-4..10`，藏 `-4..=1` → `hidden = 6`；
    /// - `extra = 6` → 窗口 `-6..10`，藏 `-4..=1` 仍是 6 → 收敛。
    ///
    /// 这条是**定点迭代本身的**判据：一遍算完的实现会停在 2，屏幕底部就要露 4 行白。
    #[test]
    fn extra_rows_needed_iterates_when_a_block_crosses_the_top_edge() {
        assert_eq!(extra_rows_needed(&[(-5, 1)], 0, 10, 10), 6);
    }

    /// 块跨窗口**下边界**：只补窗口内那一截，窗口外的不算（一轮即可收敛）。
    #[test]
    fn extra_rows_needed_clips_at_the_bottom_edge() {
        // [5,20] → 窗口只到第 9 行，被藏的是 6..=9 → 4。
        assert_eq!(extra_rows_needed(&[(5, 20)], 0, 10, 10), 4);
    }

    /// 块首行是折叠头，**不算**被折：`[5,9]` 在窗口里只贡献 4 行，不是 5 行。
    ///
    /// 若这里算成 5，每折一个块就会多要一行，屏幕会整体上移一行内容。
    #[test]
    fn extra_rows_needed_never_counts_the_fold_header() {
        assert_eq!(extra_rows_needed(&[(5, 9)], 0, 10, 10), 4);
        // 头行正好在窗口首行时也一样：`[0,9]` → 藏 1..=9 → 9。
        assert_eq!(extra_rows_needed(&[(0, 9)], 0, 10, 10), 9);
    }

    /// 多个块：各自求交集再相加。
    #[test]
    fn extra_rows_needed_sums_every_block() {
        // [2,4] 藏 3、4（2 行）；[6,12] 被窗口下边界截到 9 → 藏 7..=9（3 行）→ 5。
        assert_eq!(extra_rows_needed(&[(2, 4), (6, 12)], 0, 10, 10), 5);
        // 完全在窗口之外的块一行都不贡献（一端在上方、一端在下方）。
        assert_eq!(extra_rows_needed(&[(-20, -10)], 0, 10, 10), 0);
        assert_eq!(extra_rows_needed(&[(30, 40)], 0, 10, 10), 0);
    }

    /// `rows == 0`：没有屏幕要补，直接 0（也避免下面以 `rows` 为上限的比较失去意义）。
    #[test]
    fn extra_rows_needed_is_zero_for_an_empty_screen() {
        assert_eq!(extra_rows_needed(&[(0, 5)], 0, 0, 0), 0);
        assert_eq!(extra_rows_needed(&[(-9, 99)], 0, 0, 0), 0);
    }

    /// 触到上限：整屏都在折叠里时最多补 `rows` 行，不再往历史里无限多要。
    #[test]
    fn extra_rows_needed_is_capped_at_one_screen() {
        // 一个横跨整屏的大块：窗口 0..10 内可见的行全在块里 → 10 ≥ rows → 钳到 10。
        assert_eq!(extra_rows_needed(&[(-1000, 1000)], 0, 10, 10), 10);
        // 多个块合计也会顶到上限：`[0,9]` 藏 9 行、`[-5,-1]` 上移后藏 4 行，
        // 合计 13 已经超过一屏 → 钳到 10，而不是继续往历史里要 13 行。
        assert_eq!(extra_rows_needed(&[(-5, -1), (0, 9)], 0, 10, 10), 10);
    }

    /// 不变量：补进来的行数 == 窗口里被藏的行数，两者相减**正好**是 `rows`。
    ///
    /// 这条把「补行」与「丢行」两张判据（[`hidden_rows_in`] 与 [`hidden_by_fold`]）
    /// 对在一起：它们只要有一处口径不同（比如一处把折叠头算进去），屏幕就会少画
    /// 或多画几行——而那种偏差在单条用例里很难被肉眼看出来。
    #[test]
    fn extra_rows_needed_leaves_exactly_one_screen_visible() {
        let (base_start, base_end, rows) = (0i64, 10i64, 10usize);
        let cases: [&[(i64, i64)]; 4] = [
            &[(5, 7)],
            &[(-5, 1)],
            &[(5, 20)],
            &[(2, 4), (6, 12)],
        ];
        for folds in cases {
            let extra = extra_rows_needed(folds, base_start, base_end, rows);
            assert!(extra <= rows as i64, "不得越过上限: {folds:?}");
            let win_start = base_start - extra;
            let visible = (win_start..base_end)
                .filter(|row| !hidden_by_fold(folds, *row))
                .count();
            let hidden = (win_start..base_end).filter(|row| hidden_by_fold(folds, *row)).count();
            assert_eq!(hidden, extra as usize, "藏的行数必须等于补进来的行数: {folds:?}");
            assert_eq!(visible, rows, "剩下的可见行必须正好一屏: {folds:?}");
        }
    }

    // ── 折叠渲染：`ensure_view` 两条路径 ─────────────────────────────────

    /// 空折叠 = 老路径：贴底时与快照派生**逐字段**相同，回看时给空白帧。
    ///
    /// 这是五张视觉基线的守卫在单测里的那一半：折叠集合为空时 `ensure_view` 走的
    /// 是本次改动**之前的原函数体**（`!is_empty()` 那个早退分支），这里断言的是
    /// 它的返回值没变——`View::range`、每一行的行号与格子、光标、历史行数。
    #[test]
    fn empty_folds_take_the_untouched_path() {
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let ctx = egui::Context::default();
        let mut tab = tab_from(
            &pool,
            SessionConfig::new(Protocol::Shell),
            None,
            egui::Id::new("fold-empty-path"),
            &ctx,
        );
        assert!(tab.folds.is_empty(), "新标签没有折叠");

        // 贴底：老路径直接 `snapshot.to_screen_image()`。
        let view = ensure_view(&mut tab, 24);
        assert_eq!(view, tab.snapshot.to_screen_image(), "空折叠贴底必须与快照派生同值");

        // 回看：夹具里没有会话（`session = None`），范围事实永远到不了手上，
        // 所以老路径给的就是视口那一截的空白帧——同样覆盖到了。
        tab.viewport.scroll_up(5, 100);
        let view = ensure_view(&mut tab, 24);
        assert_eq!(view.range, -5..19, "空白帧的区间就是视口区间");
        assert!(
            view.lines.iter().all(|line| line.text().is_empty()),
            "没有事实就画空白，绝不猜内容"
        );
    }

    /// 有折叠但**这一帧的事实还没到**：退回不折的画面，而不是一帧全黑。
    ///
    /// 同时钉住「贴底也要发范围请求」：请求范围必须是 `-extra..rows`（这里
    /// `-3..24`），而不是贴底窗口 `0..24`——底部的三行得由 `-3..0` 那段历史补。
    #[test]
    fn folds_without_arrived_facts_fall_back_to_the_unfolded_screen() {
        use term_app::folds::FoldRegion;
        use term_core::coords::HistoryRow;

        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let ctx = egui::Context::default();
        let mut tab = tab_from(
            &pool,
            SessionConfig::new(Protocol::Shell),
            None,
            egui::Id::new("fold-fallback"),
            &ctx,
        );
        // 历史绝对行 [0,3]（头行 0 可见）→ 视口行同值（`history == 0`）。
        let region = FoldRegion::new(
            HistoryRow::new(0).expect("行号 0 合法"),
            HistoryRow::new(3).expect("行号 3 合法"),
        )
        .expect("区间正序");
        tab.folds.add(region).expect("空集合插入必成功");

        assert!(tab.screen.is_none(), "夹具没有会话，范围事实永远不来");
        let view = ensure_view(&mut tab, 24);
        // [0,3] 在 0..24 里藏 1、2、3 → extra = 3 → 请求 -3..24。
        let requested = tab
            .last_request
            .clone()
            .expect("贴底也必须发范围请求");
        assert_eq!(requested.0.start().get(), -3, "向上多要三行");
        assert_eq!(requested.0.end().get(), 24, "窗口尾不动");
        assert_eq!(view, tab.snapshot.to_screen_image(), "事实没到 = 不折的画面");
    }
}
