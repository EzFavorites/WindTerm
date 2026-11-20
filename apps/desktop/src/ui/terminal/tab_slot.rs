//! 「画哪一块终端」：主区的标签页，还是底带 Shell 面板内的一条标签。
//!
//! # 为什么需要这一层
//!
//! 2026-11-15 之前底带的 `Shell` 面板是**第二套**终端实现（`panels/tools.rs`
//! 自己 `pool.open` + `layout_no_wrap` 画一屏 + 手画光标块 + 只认六个键）。
//! 于是「面板里能用什么」与「标签页里能用什么」成了两份答案：面板里没有选区、
//! `Cmd+C`、滚轮回看、鼠标报告、hex、折叠、查找高亮、粘贴确认、margin、右键
//! 菜单。用户裁定面板**保留**，但内容与主区标签页一致——面板里是若干条**特殊的
//! session**（默认打开系统本地 shell），渲染与交互与标签页完全同一份代码。
//!
//! [`TabSlot`] 就是那份代码里唯一需要知道「现在画的是谁」的那个参数：
//! `pane.rs::draw_tab` 的每一处分叉都由它决定。
//!
//! # 为什么不把面板标签塞进 `tabs`
//!
//! `active_tab` 是**下标**型（分屏树、标签条、十几处赋值点全都按下标说话）。
//! 把一个不是主区标签页的东西混进同一个 `Vec`，只要有一处按下标取到它，症状
//! 就是「点 A 标签跳到 B」——那种错位会在整个 UI 里随机冒头，而且不指向根因。
//! 所以面板的标签是独立字段（`TerminalWindow::shell_tabs`），槽位是显式的两种
//! 取值，编译期就不存在「这个下标是哪个世界里的」的歧义。

use term_core::session::SessionConfig;
use term_core::session_runtime::SessionId;

use super::LineCache;
use crate::ui::*;

/// 面板内标签的 egui 键位都要与主区**错开**：置最高位。
///
/// ADR-0025 阶段 3：槽位本身已按恒在身份（`SessionId`）寻址，两块区域的身份
/// 由同一个铸造源发号、天然互异；这里的 tag 只为 `TabSlot::index()` 产出的
/// egui widget id 再上一道保险——同一帧里主区与面板的临时拖拽格子不会串。
const SHELL_KEY_TAG: usize = 1usize << (usize::BITS - 1);

/// 一块终端区域：主区的某条标签，或底带 Shell 面板里的某条特殊会话。
///
/// ADR-0025 阶段 3：槽位从「下标」改成「恒在身份」——与 `Tab.identity` 同一
/// `SessionId`。身份不随增删 / 重排漂移，所以 `Main(id)` 永远指向同一条标签，
/// 不存在「点 A 跳到 B」的越界错位。`Copy` 且只有两个变体。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::ui) enum TabSlot {
    /// 主区里 `identity == id` 的那条标签（分屏树的叶子说的就是这个）。
    Main(SessionId),
    /// 底带 Shell 面板里 `identity == id` 的那条特殊会话（`shell_tabs`）。
    Shell(SessionId),
}

impl TabSlot {
    /// 这块区域是不是主区的标签页。
    pub(in crate::ui) fn is_main(self) -> bool {
        matches!(self, TabSlot::Main(_))
    }

    /// 稳定键：给 `egui` 的 widget id、剪贴板派生值与字形缓存当槽位名用。
    ///
    /// 两块区域按同一个身份发号、天然互异；`Shell` 再置最高位，保证即便主区与
    /// 面板同时持有一条身份相同的标签（现实中不会发生），两块的 `Id` 也不撞。
    pub(in crate::ui) fn index(self) -> usize {
        match self {
            TabSlot::Main(id) => id as usize,
            TabSlot::Shell(id) => (id as usize) | SHELL_KEY_TAG,
        }
    }
}

/// 按槽位取标签：`Main` 在 `tabs` 里按身份找，`Shell` 在 `shell_tabs` 里按身份找。
///
/// 收两个**字段借用**而不是写成 `&mut TerminalWindow` 上的方法：`pane.rs`
/// 在持有 `tab` 的同时还要读 `self.fonts` / `self.palette` / `self.view`，
/// 方法调用会独占总个 `self`（E0499）；两个字段分开借，编译器就能看出
/// 它们互不相干。
pub(in crate::ui) fn tab_of<'a>(
    tabs: &'a mut [Tab],
    shell: &'a mut [Tab],
    slot: TabSlot,
) -> Option<&'a mut Tab> {
    match slot {
        TabSlot::Main(id) => tabs.iter_mut().find(|t| t.identity == id),
        TabSlot::Shell(id) => shell.iter_mut().find(|t| t.identity == id),
    }
}

/// 按槽位取**字形缓存**：两块区域各一份。
///
/// `LineCache` 的键是「行号 + 内容 hash」（见 `view.rs`）。共用一份时两块区域
/// 交替写同一下标，每一帧都会把对方的那条判成失效——画得没错，但缓存等于没养
/// （每行每帧重新 `build_spans`）。这是「看起来一切正常、只是慢慢变卡」的
/// 那一类问题，所以宁可多一个字段。
pub(in crate::ui) fn line_cache<'a>(
    main: &'a mut LineCache,
    shell: &'a mut LineCache,
    slot: TabSlot,
) -> &'a mut LineCache {
    match slot {
        TabSlot::Main(_) => main,
        TabSlot::Shell(_) => shell,
    }
}

/// 按槽位取**可写**的剪贴板派生值（`Cmd+C` 的来源）。
///
/// 分两份的理由与字形缓存不同：这一份是**行为**而不是性能。窗口级只有一份
/// 字段时，主区与面板会互相覆盖「现在复制什么」——用户在面板里选好一段，
/// 主区同一帧的选区就把它冲掉了。行为一致优先，所以两块各记一份。
pub(in crate::ui) fn selection_text_mut<'a>(
    main: &'a mut String,
    shell: &'a mut String,
    slot: TabSlot,
) -> &'a mut String {
    match slot {
        TabSlot::Main(_) => main,
        TabSlot::Shell(_) => shell,
    }
}

/// 按槽位**读**剪贴板派生值。与 [`selection_text_mut`] 是同两个字段的两个方向。
pub(in crate::ui) fn selection_text_ref<'a>(
    main: &'a str,
    shell: &'a str,
    slot: TabSlot,
) -> &'a str {
    match slot {
        TabSlot::Main(_) => main,
        TabSlot::Shell(_) => shell,
    }
}

// ── 面板内标签的生命周期 ────────────────────────────────────────────────
//
// 三条入口（首次绘制 / `+` / `×`）都走这里，不在绘制代码里各写一遍：
// 「关到 0 个要补一条」这条约定只要漏一处，面板就会变成一块点不动的死区。

/// 面板标签名：`序号.壳名`（视觉基线里的 `1.zsh`）。
///
/// 序号是**这个面板里**的顺序号（从 1 起），不是主区标签的下标——两者混用会
/// 出现「面板标签叫 5.zsh，主区只有两条」这种读不懂的编号。
///
/// 抽成纯函数是为了能单测：名字是用户唯一能用来区分几条 shell 的线索。
pub(in crate::ui) fn shell_tab_name(ordinal: usize, shell: &str) -> String {
    let shell = shell.trim();
    let shell = if shell.is_empty() { "shell" } else { shell };
    format!("{ordinal}.{shell}")
}

/// 过滤语义：按标签名做**大小写不敏感**的包含匹配，空词 = 全部。
///
/// 与 `Explorer` / `Sidebar` 的 Filter 是同一条（`contains` + `to_lowercase`）：
/// 同一个界面上两块过滤框给两种匹配规则，用户没法形成手感。
pub(in crate::ui) fn shell_tab_matches(name: &str, filter: &str) -> bool {
    let needle = filter.trim();
    needle.is_empty() || name.to_lowercase().contains(&needle.to_lowercase())
}

/// 下一个标签序号：**沿着已经用过的最大序号往上走**。
///
/// 不能用 `len + 1`：关掉面板里的第一条（`1.zsh`）之后长度回到 1，再点一次 `+`
/// 又会得到一个 `2.zsh`——面板里出现两条同名标签，而名字是用户唯一能区分它们
/// 的线索（视觉基线里那一条就叫 `1.zsh`）。
///
/// 面板被关空（`names` 为空）时从 1 重新开始：那时没有任何名字会撞车，而
/// 「关光了再开」在用户看来就是从头开始。
pub(in crate::ui) fn next_shell_ordinal<'a>(names: impl Iterator<Item = &'a str>) -> usize {
    let max = names
        .filter_map(|name| name.split('.').next()?.trim().parse::<usize>().ok())
        .max()
        .unwrap_or(0);
    max.saturating_add(1)
}

impl TerminalWindow {
    /// 面板第一次要画的时候：一条标签都没有就开一条系统本地 shell。
    ///
    /// 惰性建一次：面板从头到尾没打开过，就一个进程都不起。
    pub(in crate::ui) fn ensure_shell_tabs(&mut self) {
        if self.shell_tabs.is_empty() {
            self.open_shell_tab();
        }
    }

    /// `+` 的落地点：开一条系统本地 shell 并切过去，返回它的下标。
    ///
    /// 与主区标签页走**同一条**构造路（`setup::make_tab`）：唤醒回调、字节 tee、
    /// prefs 里的显示默认值都跟着装上，所以「面板里的终端」与「标签页里的终端」
    /// 从构造那一刻起就是同一种东西。
    pub(in crate::ui) fn open_shell_tab(&mut self) -> usize {
        // P1-5：登录名与壳名都由宿主注入（core 不读环境）。
        let mut config = SessionConfig::new_local_shell(&term_io::host::login());
        // 序号沿着已用过的最大值 +1：关掉中间一条之后再来一条，名字不会重复。
        let ordinal = next_shell_ordinal(self.shell_tabs.iter().map(|t| t.config.name.as_str()));
        config.name = shell_tab_name(ordinal, &term_io::host::shell_basename());
        // `id` 只在本窗口里当一个稳定标识（面板标签不进分屏树、不进标签条），
        // 下标即序号足够；`draw_tab` 的 widget id 由 `ui` 自己生成，不读它。
        let id = egui::Id::new(("tab_shell_panel", self.shell_tabs.len()));
        let ctx = self.ctx.clone();
        let tab = super::make_tab(&self.pool, config, id, &ctx);
        let new_id = tab.identity;
        self.shell_tabs.push(tab);
        self.shell_active = Some(new_id);
        self.ctx.request_repaint();
        // 返回新标签在面板里的序号（调用方/测试按「第几条」用）。
        self.shell_tabs.len() - 1
    }

    /// `×` 的落地点：关掉面板内第 `index` 条，并保证面板**不会空着**。
    ///
    /// 必须走 [`Tab::close`]（先摘 tee 再 `pool.close`）：直接 `Vec::remove` 丢掉
    /// 一个 `Tab` 只是丢了句柄，PTY 子进程与监管者线程照旧活着。
    ///
    /// 关到 0 条时立刻补一条新的：面板空着就是一块点不动的死区——用户连
    /// 「再开一条」的按钮都找不到（那颗 `+` 画在标签条上，而标签条是面板自己的
    /// 内容）。补上之后它仍然是一个可用终端，与「主区永远至少有一条标签」
    /// （`ensure_tab`）是同一条约定。
    pub(in crate::ui) fn close_shell_tab(&mut self, index: usize) {
        if index >= self.shell_tabs.len() {
            return;
        }
        let mut closed = self.shell_tabs.remove(index);
        closed.close();
        if self.shell_tabs.is_empty() {
            self.shell_active = None;
            self.open_shell_tab();
            return;
        }
        // 身份稳定：活动那条还在就不动（关掉它前面的条目不再让它「挪号」）；
        // 关掉的正是活动那条时才退回最后一条。
        if !self
            .shell_tabs
            .iter()
            .any(|t| Some(t.identity) == self.shell_active)
        {
            self.shell_active = self.shell_tabs.last().map(|t| t.identity);
        }
        self.ctx.request_repaint();
    }

    /// 面板当前该画哪一条的**序号**：由 `shell_active` 身份反查，查不到（空面板 /
    /// 陈旧身份）退化成 0。
    ///
    /// 单独给一个入口而不是让调用方自己反查：关标签、切标签、首次构造三条路径
    /// 都会读它，口径必须只有一份（越界一次就是「点了没反应」，而不是崩溃——
    /// `tab_of` 会返回 `None`）。
    pub(in crate::ui) fn shell_active_index(&self) -> usize {
        match self.shell_active {
            Some(id) => self
                .shell_tabs
                .iter()
                .position(|t| t.identity == id)
                .unwrap_or(0),
            None => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::lifecycle_test_support::{fake_tab, window_with, CloseCount};

    /// 槽位的稳定键**不会与主区标签下标相撞**。
    ///
    /// 挡的是「两块区域共用一个 `Id` / 一份临时拖拽状态」：撞了以后症状是
    /// 在面板里拖动选区，主区的拖拽锚点跟着乱跳。
    #[test]
    fn a_shell_slot_never_collides_with_a_tab_index() {
        for index in 0..64usize {
            assert_ne!(
                TabSlot::Shell(index as SessionId).index(),
                TabSlot::Main(index as SessionId).index()
            );
        }
        assert_eq!(TabSlot::Main(3).index(), 3);
        assert!(TabSlot::Main(0).is_main());
        assert!(!TabSlot::Shell(0).is_main());
        assert_ne!(TabSlot::Shell(0), TabSlot::Shell(1));
    }

    /// 取标签只看槽位：`Main` 拿到 `tabs[i]`，`Shell` 拿到 `shell_tabs[i]`。
    ///
    /// 这条钉的是「两块区域取的是各自的标签」——取错一次，键盘字节就会写进
    /// 另一条会话（屏幕上看不出，远端会看到串台）。
    #[test]
    fn tab_of_reaches_the_slot_it_names() {
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let ctx = egui::Context::default();
        let closes = CloseCount::default();
        let mut tabs = vec![
            fake_tab(&pool, &ctx, 0, &closes),
            fake_tab(&pool, &ctx, 1, &closes),
        ];
        let mut shell = vec![
            fake_tab(&pool, &ctx, 2, &closes),
            fake_tab(&pool, &ctx, 3, &closes),
        ];
        // 每条标签的 `id` 都不同，据此区分「拿到的是哪一个」。
        let first = tabs[0].id;
        let shell_second = shell[1].id;
        let first_identity = tabs[0].identity;
        let shell_second_identity = shell[1].identity;

        assert_eq!(
            tab_of(&mut tabs, &mut shell, TabSlot::Main(first_identity)).map(|t| t.id),
            Some(first)
        );
        assert_eq!(
            tab_of(&mut tabs, &mut shell, TabSlot::Shell(shell_second_identity)).map(|t| t.id),
            Some(shell_second)
        );
        // 身份不存在给 `None`（不 panic）——用一个永不会被铸造源发出的 id，
        // 避免与全局自增的其它测试夹具撞号（否则断言会偶发）。
        assert!(tab_of(&mut tabs, &mut shell, TabSlot::Main(SessionId::MAX)).is_none());
        assert!(tab_of(&mut tabs, &mut shell, TabSlot::Shell(SessionId::MAX)).is_none());
    }

    /// 面板标签名：`序号.壳名`，壳名缺失时退化成 `shell`，序号从 1 起。
    #[test]
    fn a_shell_tab_is_named_ordinal_dot_shell() {
        assert_eq!(shell_tab_name(1, "zsh"), "1.zsh");
        assert_eq!(shell_tab_name(3, "bash"), "3.bash");
        assert_eq!(shell_tab_name(2, ""), "2.shell", "取不到壳名也要有名字");
        assert_eq!(shell_tab_name(2, "  "), "2.shell", "空白壳名同样退化");
    }

    /// 过滤：大小写不敏感包含匹配，空词 = 全部。
    #[test]
    fn the_filter_is_a_case_insensitive_contains() {
        assert!(shell_tab_matches("1.zsh", ""));
        assert!(shell_tab_matches("1.zsh", "  "));
        assert!(shell_tab_matches("1.zsh", "zs"));
        assert!(shell_tab_matches("1.ZSH", "zsh"));
        assert!(!shell_tab_matches("1.zsh", "bash"));
    }

    /// 序号沿着用过的最大值往上走：关掉中间一条之后再来一条**不会同名**。
    ///
    /// `len + 1` 的写法在这里必挂：`["1.zsh", "3.zsh"]` 的长度是 2，再开一条会
    /// 得到第二个 `3.zsh`（或者关到只剩一条时得到第二个 `2.zsh`）。
    #[test]
    fn the_next_ordinal_never_repeats_a_name() {
        assert_eq!(next_shell_ordinal(std::iter::empty()), 1, "空面板从 1 开始");
        let names = ["1.zsh".to_string(), "2.zsh".to_string()];
        assert_eq!(next_shell_ordinal(names.iter().map(String::as_str)), 3);
        // 关掉中间那条：剩下 1、3，下一条是 4 而不是 3。
        let names = ["1.zsh".to_string(), "3.zsh".to_string()];
        assert_eq!(next_shell_ordinal(names.iter().map(String::as_str)), 4);
        // 名字被手改过 / 没有序号前缀时也不能 panic，按 1 起步。
        let names = ["zsh".to_string()];
        assert_eq!(next_shell_ordinal(names.iter().map(String::as_str)), 1);
    }

    /// 首次绘制建出一条、且**不进 `tabs`**；`+` 再加一条并切过去。
    ///
    /// 两条都是硬要求：进了 `tabs` 就会让 `active_tab` 的下标世界多出一个不是
    /// 主区标签页的元素（「点 A 跳到 B」）；`+` 之后激活位不跟过去，用户按了
    /// 新建还得自己点一下新标签。
    #[test]
    fn ensure_and_open_keep_the_panel_out_of_the_main_tabs() {
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let ctx = egui::Context::default();
        let closes = CloseCount::default();
        let tab = fake_tab(&pool, &ctx, 0, &closes);
        let mut win = window_with(pool, &ctx, vec![tab]);
        assert!(win.shell_tabs.is_empty(), "夹具前提：面板还没开过");

        win.ensure_shell_tabs();
        assert_eq!(win.shell_tabs.len(), 1, "第一次打开面板要开一条");
        assert_eq!(win.shell_active_index(), 0);
        assert_eq!(win.tabs.len(), 1, "面板的标签不进 tabs");

        // 再 ensure 一次不重复开。
        win.ensure_shell_tabs();
        assert_eq!(win.shell_tabs.len(), 1);

        assert_eq!(win.open_shell_tab(), 1, "`+` 之后切到新那条");
        assert_eq!(win.shell_tabs.len(), 2);
        assert_eq!(win.shell_active_index(), 1);
        assert_eq!(win.tabs.len(), 1);
    }

    /// 关标签：激活位夹回来；**关到 0 条时立刻补一条**（面板不能变成死区）。
    #[test]
    fn closing_a_shell_tab_never_leaves_the_panel_empty() {
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let ctx = egui::Context::default();
        let closes = CloseCount::default();
        let tab = fake_tab(&pool, &ctx, 0, &closes);
        let mut win = window_with(pool, &ctx, vec![tab]);

        win.ensure_shell_tabs();
        win.open_shell_tab();
        assert_eq!(win.shell_tabs.len(), 2);
        assert_eq!(win.shell_active_index(), 1);

        // 关掉当前这条：长度减一，激活位夹到仍然存在的最后一条。
        win.close_shell_tab(1);
        assert_eq!(win.shell_tabs.len(), 1);
        assert_eq!(win.shell_active_index(), 0);

        // 关掉最后一条：立刻补一条新的，面板仍然可用。
        win.close_shell_tab(0);
        assert_eq!(win.shell_tabs.len(), 1, "关到 0 条要补一条");
        assert_eq!(win.shell_active_index(), 0);
        // 补出来的那条按「第 1 条」重新编号（名字是给用户看的顺序号）。
        assert_eq!(
            win.shell_tabs[0].config.name,
            shell_tab_name(1, &term_io::host::shell_basename())
        );

        // 越界下标是空操作，不 panic。
        win.close_shell_tab(9);
        assert_eq!(win.shell_tabs.len(), 1);
    }
}
