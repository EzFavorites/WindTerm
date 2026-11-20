//! 能力注册表：命令「是否已接线」的唯一判定点。
//!
//! 这里不维护第二份清单。判定就是 `Action::parse(id).is_some()`：
//! - 能解析 → 该命令有对应能力动作，可以执行；
//! - 解析不了 → 占位，菜单置灰。
//!
//! 之所以做成一个模块而不是让 UI 直接调 `Action::parse`，是为了让
//! 「能力」这个概念有地方挂：查状态、列清单、统计、以及给验收脚本与
//! 测试一个稳定的名字。设计仓库的 12-crate 蓝图里这一步叫 plugin registry，
//! 当前阶段不需要动态加载，只需要一个真实的判定点。

use crate::vocab::{Action, FeatureId};
use crate::menu::{self, Item};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapState {
    /// 有动作、有 reducer，点了会真的改变状态。
    Implemented,
    /// 菜单里有位置，但没有任何能力接管；UI 必须置灰。
    Planned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capability<'a> {
    pub id: &'a str,
    pub feature: Option<FeatureId>,
    pub state: CapState,
}

impl Capability<'_> {
    pub fn is_implemented(&self) -> bool {
        self.state == CapState::Implemented
    }
}

/// 查一个命令 ID 的能力归属与状态。
pub fn describe(id: &str) -> Capability<'_> {
    let action = Action::parse(id);
    Capability {
        id,
        feature: action.as_ref().map(Action::feature),
        state: if action.is_some() {
            CapState::Implemented
        } else {
            CapState::Planned
        },
    }
}

/// 菜单里这个命令能不能点。
pub fn is_implemented(id: &str) -> bool {
    Action::parse(id).is_some()
}

/// 遍历菜单树，收集全部命令（含子菜单），供统计与测试使用。
///
/// 命令 ID 在菜单表里是 `&'static str`，所以结果可以脱离菜单树的所有权。
fn collect(items: &[Item], out: &mut Vec<Capability<'static>>) {
    for item in items {
        match item {
            // 可勾选行也是命令：`Item::Check` 的勾选态是菜单数据里的常量，
            // 不代表「已接线」——能不能点仍只看 `describe` 的结论。
            Item::Action { id, .. } | Item::Check { id, .. } => out.push(describe(id)),
            Item::Separator => {}
            Item::Submenu { items, .. } => collect(items, out),
        }
    }
}

/// 全部真实可点入口及其状态：菜单树 + 影子入口，按菜单顺序。
///
/// 标签右键的 10 个动作不画在菜单栏里，但它们是**真实入口**。
/// 以前 `inventory()` 只遍历菜单树，于是「已接线 N 条」少算这一块；
/// 现在把 `tab::SHADOW_ENTRIES` 并进同一张表，判定仍只用 `Action::parse`，
/// 不维护第二份清单。加一条影子入口而不给它 `parse`，这里立刻置灰。
pub fn inventory() -> Vec<Capability<'static>> {
    let mut out = Vec::new();
    for menu in menu::menus() {
        collect(&menu.items, &mut out);
    }
    for entry in crate::tab::SHADOW_ENTRIES {
        out.push(describe(entry.id));
    }
    out
}

/// (已接线, 占位) 的命令数。
pub fn counts() -> (usize, usize) {
    let all = inventory();
    let implemented = all.iter().filter(|c| c.is_implemented()).count();
    (implemented, all.len() - implemented)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn menu_has_no_duplicate_command_ids() {
        // 同一个 surface 里一条命令只能出现一次——那是抄菜单时漏改 ID 的典型症状。
        // 但**两个 surface 各有一个入口**是正常的：起
        // `Session › Open Recent › Reopen Closed View` 与标签右键的「Reopen Closed」
        // 共用一个 ID（`Terminal.TabReopenClosed`），因为它们是同一件事
        // （读者都是 `ui/shell/tab_menu.rs::reopen_closed`），只是入口在菜单栏与
        // 标签栏两处。以前菜单里挂的是第二个 ID `Terminal.ReopenClosedView`，
        // `parse` 不认识它，于是同一能力被算成「一条能点 + 一条置灰」。
        //
        // 所以这里不看「有没有重复」，而看「重复的是哪些」：清单之外多一条就红。
        let all = inventory();
        let mut seen = BTreeSet::new();
        let repeated: BTreeSet<&str> = all
            .iter()
            .map(|c| c.id)
            .filter(|id| !seen.insert(*id))
            .collect();
        assert_eq!(
            repeated,
            BTreeSet::from(["Terminal.TabReopenClosed"]),
            "只有菜单栏 / 标签右键共用的 Reopen 允许出现两次"
        );
    }

    #[test]
    fn inventory_matches_the_r3_baseline() {
        // 115 个菜单项、93 个已接线、22 个占位。
        // 沿革：220 项 / 21 接线 → Text.Copy 22 → Text.Paste 23 →
        // 审计后 -6（空转开关回退，见 `commands_without_a_visible_effect_stay_planned`）→
        // **删掉 106 条编辑器语义项**（220 → 114）→
        // **接通 17 条选择命令**（Selection 能力，鼠标/键盘/菜单共用一条纯函数路径）→
        // **接通 Window.TogglePaneFiler**（文件面板，右侧远端目录浏览）→
        // **接通 7 条日志命令**（Session › Log 整组）→
        // **接通 12 条 A 类便宜货**（循环切标签 / 复制会话名·URL / 帮助 / 在线搜索 /
        // 字号缩放 / 从选区查找 / 粘贴选区）→
        // **接通 2 条命令面板命令 + 3 条 E 类**（Exit / Close / CloseAllViews）→
        // **接通 Goto 两条 + 分屏六条**（共 8 条）→
        // **接通六个工具面板开关** → **接通 v0.1 工作台 15 条** →
        // **接通 Window.LockScreen**（F 类占位改真实锁屏，复用 Vault 主密码）→
        // **接通 Window.ResetLayout**（停靠 / 浮动面板全部收回默认位置）→
        // 2026-11-07 **接通 Window.ToggleSidebar**（会话树标题栏的 × 与可逆回程）→
        // 同日 **接通 4 条 margin 命令**（Blank / Number / Symbol / Timestamp；
        // Fold 当时仍占位——折叠要改视口行映射，v0.3。2026-11-14 已接通，
        // 见下面「接线折叠族九条」那一段）。
        // 同日 **接通 2 条整带开关**（Left / Right Docks，菜单栏布局按钮的下拉）。
        // 增删能力时这组数字必须一起改：它是「能力面」的显式确认，
        // 不是随手可漂的统计——发现「接了线但没效果」能把这数字撑起来，
        // 则发现「菜单项本身」也能撑起来：220 项里一半是编辑器功能。
        // 2026-11-08 **UI 对齐**：按视觉基线补齐菜单结构，新增 13 条
        // 未接线项（Selection 9 条平铺项 + Goto 4 条光标移动），一律 Planned
        // （灰显占位）。已接线数不变，只增占位。
        // 2026-11-08 **UI 对齐**：View 菜单补回 `Text.HighlightCaretLine`
        // （视觉基线里它是全菜单唯一的勾选行）→ 占位 36→37、总数 136→137。
        // 它属于删掉的编辑器语义，所以只是占位，勾选框画空框 + 灰显。
        // 2026-11-09 **接通 13 条**：Selection 的 9 条集合命令（激活下一份 /
        // 锁 / 丢 / 取消 / 拆分 / 反选 / 流列互换 / 交换端点）+ Goto 的 4 条光标
        // → 已接线 100→113、占位 37→24、总数 137 不变。
        // 2026-11-09 **接通 5 条传输命令**（Transfer Binary：Send/Receive X Modem、
        // Send/Receive Y Modem、Cancel Modem）→ 已接线 113→118、占位 24→19、总数
        // 137 不变。
        // 同日 **接通 ZMODEM 两条**（Send/Receive Z Modem）→ 已接线 118→120、
        // 占位 19→17、总数 137 不变。这一刀的证据不是「`parse` 返回 `Some`」而是
        // **文件真的过去了**：`apps/desktop/tests/xfer_zmodem.rs` 在真 PTY 里起真的
        // `sz` / `rz`，走完 `XferHandle::start` 之后逐字节比对落盘内容
        // （`term-zmodem` 内核本身另有一份与真实 lrzsz 的互通测试）。
        // 所以这次是 +7 而不是 +5：X/Y 三条 + Z 两条，剩下的 17 个占位与传输无关。
        // 2026-11-09 **结构对齐**：18 个空子菜单逐行补齐，
        // 新增 68 条命令**全部是占位**（一条都没假装接线，判据在
        // `menu_submenus.rs::the_new_rows_are_unique_and_none_is_fake_wired`）。
        // 所以这一刀已接线数一位不涨：120 不变、占位 17→85、总数 137→205。
        // 这不是能力倒退：这 68 条原本连「灰项」都算不上，是点开只有
        // 一条横线的结构性缺口；现在逐条入册，才有一个可清零的清单。
        // 2026-11-09 **影子入口归册**：标签右键 10 个动作提升为
        // 纯层 `Action::Tab`，由 `inventory()` 并进同一张能力表。
        // 它们本来就在工作（只是没被统计），所以已接线 120→130、
        // 占位保持 85、总数 205→215。三个数字一起改，缺一个都会红。
        // 2026-11-09 **tmux 入口**：`Window › Panes › Tmux Control Mode`
        // 是第 131 条已接线命令（读者 `ui/panels/tmux.rs::draw_tmux_panel`），
        // 菜单里多一条 → 总数 215→216，占位仍 85。
        // 2026-11-10 **接通 11 条**：Goto 的 10 条光标移动（词首 / 词尾 / 翻页 /
        // 行首行尾 / 文首文末）映射到 `selection.rs` 既有的 `Op`，与 Shift+方向键
        // 同一条位移；Open Recent 的 `Reopen Closed View` 改挂 `Terminal.TabReopenClosed`
        // ——它与标签右键那条是同一个实现，所以 `parse` 里**没有**第二个 arm。
        // → 已接线 131→142、占位 85→74，总数 216 不变（菜单项一条没增没删，
        // 只是其中 11 条从「置灰」变成「可点」）。
        // 2026-11-10 **接通 `Text.ToggleHexEdit`**：`Mode ▸ Hex Edit` 从
        // 「点了没效果」变成真开关（`Effect::ToggleHex` → 投影层翻 `Tab.mode`）
        // → 已接线 142→143、占位 74→73、总数 216 不变。数字只动两位而总数
        // 不动，正是「接线」与「加菜单项」的区别。
        // （2026-11-11 更正：那一版还顺手把模式写进了 `Prefs.session_modes`，
        // 用户随后定死「View 可以切当前会话的显示方式，但是不会保存」，那张表
        // 整条删掉了。落地只剩 `Tab.mode`，一个字节都不落盘。）
        // 2026-11-11 **③ 接通 hex 三个参数子菜单的 13 条档位命令**：
        // `View ▸ Hex Base{2,7,8,10,16}` / `Hex Columns{8,16,24,32}` /
        // `Hex Group Chars{1,2,4,8}`。读者是投影层的 `ui/effects.rs::set_tab_hex`
        // ——改**活动标签**的 `Tab.hex`（不落盘）；渲染器（`ui/terminal/bin.rs`）
        // 与表头读数都从那里取。设置页写的是另一层（`Prefs.hex_*`，默认参数）。
        // → 已接线 143→156、占位 73→60，总数 216 不变（菜单项一条没增没删）。
        // 2026-11-12 **拆开「地址栏」这一个名字**：`Window.GotoAddressBar`
        // 保留（现在真的是地址栏），原来的工作台页改名成 `Window.GotoCommandLine`
        // 并在 Goto 菜单里占一行 → 已接线 156→157、占位仍 60、总数 216→217。
        // 与上面几次不同，这一刀**真的加了一条菜单项**：两件事本来就是两件事，
        // 挤在一条命令上正是要解的语义冲突。
        // 2026-11-13 **接线 26 条光标 / 选区命令**：10 条 `Select Column To` +
        // 词段 / 半页 / 段落 / 命令 / 作用域 / 缩进 / 括号对各 2 条 + 括号对另 2 条
        // → 已接线 157→183、占位 60→34、总数 217 不变（菜单项一条没增没删）。
        // 当时唯一留灰的是 `Text.GotoPreviousFold` / `Text.GotoNextFold`：折叠引擎
        // （块 → 折叠视图）还没建，接了只会算出「折叠前的行号」。
        // 2026-11-14 折叠引擎建好了（`folds.rs` 的 `Folds` + `blocks_from_inputs`
        // 已在默认编译面上，读者是 `Tab.folds`），这条理由失效。
        // 2026-11-13 **接线 10 条栅格可视化命令**：`View ▸ Symbols` 四项 +
        // `View ▸ White Spaces` 五档 + `Text ▸ Highlight Caret Line`
        // → 已接线 183→193、占位 34→24、总数 217 不变（菜单项一条没增没删，
        // 十条从「置灰」变成「点了屏幕上真的多出东西」）。
        // 与 hex 那 13 条的区别：那批换的是「怎么看字节」，这批是**多画了什么**
        // ——空格点、行尾 `$`、参考列、折行勾、光标行淡底。落地在
        // `Tab.marks`（不落盘），读者 `ui/effects.rs::set_marks_knob`，
        // 像素在 `ui/terminal/marks_paint.rs` / `view.rs`。
        // 2026-11-13 **接线 2 条「直达」命令**：`Terminal.ManageTriggers` 与
        // `Terminal.ClearRecentUrls` → 已接线 193→195、占位 24→22、总数 217 不变。
        // 这两条原本卡在「点了打开哪一页 / 清哪一份列表」——现在前者落到
        // `Effect::FocusTriggersPage`（设置页翻到 Triggers），后者复用地址栏
        // 右键早就在用的 `clear_recent_targets`，都没有新造第二条路径。
        // 2026-11-13 **接线 `Terminal.ManageOneKeys`**：与 `ManageTriggers` 同形
        // ——`show_settings = true` + `Effect::FocusOneKeysPage`（设置页翻到
        // OneKeys）。凭据管理器（SSH / Account / Sessions 三标签）在设置页落地。
        // → 已接线 195→196、占位 22→21、总数 217 不变。
        // 2026-11-13 **补完：接线 4 条窗口透明度命令** → 已接线 196→200、
        // 占位 21→17、总数 217 不变（菜单项一条没增没删，四条从「置灰」变成
        // 「点了真的改 Prefs + 屏幕」）。三档预设（Opaque / Translucent /
        // Transparent）共用一条带参数的 `Effect::SetWindowOpacity`；第四条
        // `SetWindowOpacity` 打开设置并翻到 `SettingsPage::Appearance`，用开关 +
        // 滑杆自己调。四条都**真的落盘**（load-modify-save），重启仍生效；
        // `window_transparent=false` 时渲染侧一个像素都不改（视觉基线不变）。
        // 2026-11-14 **接线折叠族九条**：`View ▸ Folds` 六条（Fold / Unfold /
        // Fold Inside / Unfold Inside / Fold All / Unfold All）+ `Text ▸ Margin Fold`
        // + `Goto ▸ Fold` 两条 → 已接线 200→209、占位 17→8、总数 217 不变
        // （菜单项一条没增没删，九条从「置灰」变成「点了屏幕上的行真的少掉」）。
        // 读者是 `ui/effects.rs::apply_fold`（唯一落地点），决策是它旁边那个纯函数
        // `decide_fold`，渲染侧是 `terminal/setup.rs::folded_view`——折叠一旦非空，
        // `ensure_view` 就走另一条取窗口路径，把被折的行丢掉、向上多要几行补齐，
        // 所以「屏幕还是满的」。九条**同一个** `Effect::Fold` 变体，不各开一条。
        // 2026-11-14 **接线 `Window.ToggleDebugMode`**：调试覆盖层从「置灰的
        // 占位」变成真开关 → 已接线 209→210、占位 8→7、总数 217 不变（菜单项
        // 一条没增没删）。读者是 `ui/terminal/pane_overlays.rs` 的
        // `if !bin && tab.debug` → `ui/terminal/debug_paint.rs::paint`：每格的
        // `Attrs` 八位、`fg`/`bg` 色号种类、`Cell.wide` 变成像素标记。
        // 归 `Shell`（命令 ID 本身是 `Window.*` 前缀），**不新增 `FeatureId`**。
        // 2026-11-15 **接线 `Application.NewWindow`**：再起一个完整应用实例（新
        // 进程），不恢复布局 → 已接线 210→211、占位 7→6、总数 217 不变。
        // 归 `Shell`（`Action::Shell(_)` 的兜底归属），同样不新增 `FeatureId`。
        // 2026-11-17 补齐 `Window.TogglePaneScript`：脚本面板早就在 `PanelId::ALL`
        // 里（齿轮菜单遍历那张表），但公共的 `Window ▸ Panes` 漏了它——菜单栏
        // 打不开。补一条专用动作指向同一个 `TogglePanel(PanelId::Script)`，读的是
        // `ViewState::show_pane_script`（`ui/panels` 每帧按它决定画不画）。
        // 已接线 211→212、占位 6 不变、总数 218。
        // 2026-11-17 补 `Window.ToggleBottomDocks`：布局下拉只有左/右两项，
        // 底带（Sender / Shell / Script）没有整带入口 → 已接线 212→213、
        // 占位 6 不变、总数 218→219。与上一条的区别：上一条是「菜单漏了既有
        // 面板」，这一条是**真的新增一条命令**（`DockZone::Bottom` 原先没有
        // 对应的 `Window.*` 名字）。`Window ▸ Panes` 同时补同一行，两个入口
        // 指向同一条命令。
        // 2026-11-17 Tunnels 设置与总线开关：`Terminal.TunnelSettings` +
        // `Window.ToggleTunnels` → 已接线 213→215、占位 6 不变、总数 219→221。
        // 前者打开配置对话框（`ui/dialogs/tunnel_settings.rs`），后者真的停/起
        // 转发（`set_tunnels_enabled`）。
        // 2026-11-18：`Text.MarginFoldColumn` 新增一行且真接线 →
        // 已接线 215→216、总数 221→222，占位 6 不变（没动任何置灰项）。
        // 2026-11-19 只读查看：`Window.SetLocalMode` 从「置灰占位」变成真开关
        // → 已接线 216→217、占位 6→5、总数 222 不变（菜单项一条没增没删）。
        let (implemented, planned) = counts();
        assert_eq!(implemented, 217, "已接线命令数变化需显式确认");
        assert_eq!(planned, 5, "占位命令数变化需显式确认");
        assert_eq!(inventory().len(), 222);
    }

    #[test]
    fn every_implemented_command_has_a_reader() {
        // 教训：命令「能被 parse」不等于「点了有效果」。
        // 这条测试列出**允许**出现在已接线集合里的命令；新增一条必须同时回答
        // 「谁读它」。目前分三类：
        //   1) 打开对话框 / 触发副作用（NewSession / OpenSettings / About / 断开 / 重连 / 关闭）；
        //   2) 直接决定绘制分支（MenuBar / StatusBar / QuickBar / FocusMode）；
        //   3) 每帧同步到窗口系统（FullScreen / AlwaysOnTop）。
        let expected: BTreeSet<&str> = [
            "Terminal.NewSession",
            "Window.OpenSettings",
            "Help.About",
            "Terminal.DisconnectSession",
            "Terminal.ReconnectSession",
            "Window.CloseActiveView",
            "Window.ToggleMenuBar",
            "Window.ToggleStatusBar",
            "Window.ToggleQuickBar",
            // 2026-11-07：margin 边栏四列。读者是 `ui/terminal/margin.rs`——
            // 每一列都真的改边栏像素（宽度、行号、符号、时间戳），
            // 不是「翻了个没人看的 bool」。`Text.MarginFold` **不在**这里：
            // 它翻的是「折哪个块」而不是「画不画那一列」，2026-11-14 起归
            // 下面折叠族那一段（`Action::Fold(FoldCmd::Toggle)`）。
            "Text.MarginBlank",
            "Text.MarginNumber",
            "Text.MarginSymbol",
            "Text.MarginTimestamp",
            // 2026-11-18：第五列。读者与上面四列同一处（`margin.rs`），
            // 但它读的是**折叠集合**：`is_fold_head(tab.folds, abs)` 决定在折叠头
            // 那一行画 `▸`。与 `Text.MarginFold` 的差别只有一格语义——
            // 那条折块，这条画「哪里折着」。
            "Text.MarginFoldColumn",
            // 2026-11-07：会话树开关。读者是 `crate::shell::panel_visible`
            // （`PanelId::Sidebar` 的 `show_sidebar` 分支，`ui` 直接调它）；
            // `ui/panels/sidebar_view.rs` 标题栏的 × 是它的写入点，
            // 菜单 Window › Panes 是回程。
            "Window.ToggleSidebar",
            // 2026-11-07：菜单栏右侧布局按钮的勾选项（整带开关）。
            // 读者仍是 `crate::shell::panel_visible`——`ToggleDocks` 只是
            // 一次写一组 `show_*`，没有第二条可见性通路。
            // 2026-11-17 补底带：归属表 `PanelId::default_zone()` 里
            // Bottom 一直是 Sender / Shell / Script 三块，缺的只是菜单入口。
            "Window.ToggleLeftDocks",
            "Window.ToggleRightDocks",
            "Window.ToggleBottomDocks",
            // 2026-11-17：Tunnels 两条。读者分别是
            // `ui/effects.rs` 的 `Effect::ToggleTunnels` → `set_tunnels_enabled`
            // （真停/起连接）与 `Effect::OpenTunnelSettings` → `tunnel_editor`
            // （真对话框）。都有屏幕上的后果，不是空转。
            "Window.ToggleTunnels",
            "Terminal.TunnelSettings",
            "Window.ToggleFocusMode",
            "Window.ToggleFullScreen",
            "Window.ToggleAlwaysOnTop",
            // 读者是 `ui/filer.rs` 的面板分支——开关一开就画，
            // 并在第一次显示时为当前标签开一条 SFTP 会话。
            "Window.TogglePaneFiler",
            // 7 条日志命令。读者是 `ui/logger.rs`——开始/停止/暂停/恢复
            // 直接落在 `SupervisorHandle` 上，打开文件/目录走系统默认程序。
            "Terminal.StartLogger",
            "Terminal.StopLogger",
            "Terminal.PauseLogger",
            "Terminal.ResumeLogger",
            "Terminal.OpenLogFile",
            "Terminal.OpenLogFolder",
            "Terminal.OpenLogSettings",
            // 12 条 A 类便宜货。`ui/mod.rs` 的 `apply_effect` 装配分派，
            // 落地在 `ui/effects.rs`——切标签改 `active_tab`、复制取 `tab.config`、
            // 帮助 / 在线搜索用 `sysopen`、字号调 `fonts.zoom`；从选区查找把
            // `SetQuery` + `Next/Previous` 派发回 reducer（投影层只给派生值，
            // 不写 `FindState`）；粘贴选区经 `paste_bytes_for` 写给 PTY。
            "Window.ActivateNextView",
            "Window.ActivatePreviousView",
            "Terminal.CopySessionName",
            "Terminal.CopySessionUrl",
            "Help.Help",
            "Text.SearchOnline",
            "Text.ZoomIn",
            "Text.ZoomOut",
            "Text.ResetZoom",
            "Text.FindNextSelection",
            "Text.FindPreviousSelection",
            // 粘贴选区，与 `Text.Paste` 同一读者（`Effect::PasteSelection`）。
            "Terminal.PasteSelectedText",
            // 2 条命令面板命令。读者是 `ui/palette.rs`：面板只在
            // `ViewState.command_palette.visible` 时画，条目取自菜单树，确认后
            // 由顶层 `reduce` 把选中命令递归派发回同一条纯函数路径。
            "Window.ShowPaletteCommand",
            "Window.ShowPaletteAnything",
            // 3 条 E 类。读者是 `ui/effects.rs`——Exit 发
            // `ViewportCommand::Close`；Close / CloseAllViews 经 `ui/tabs.rs`
            // 的 `close_tab` / `close_all_tabs`，与标签栏 × 同一条路径。
            "Application.Exit",
            "Window.Close",
            "Window.CloseAllViews",
            // Goto 两条。读者是 `ui/goto.rs` 与 `ui/effects.rs`——
            // 行号经 `Viewport::jump_to_row` 落到活动标签的视口，会话索引
            // 落到 `active_tab`；解析与失败提示都在 `term-app/goto.rs`。
            "Window.GotoLine",
            "Window.GotoSession",
            // 分屏六条。读者是 `ui/layout.rs` 的布局树与递归渲染——
            // SplitView 新建本地 shell、SplitGroup 复制当前会话配置，
            // 方向决定左右 / 上下；CloseActiveGroup 收起当前窗格，
            // MoveGroupToGroup 把当前会话并入下一个窗格。
            "Window.SplitViewHorizontally",
            "Window.SplitViewVertically",
            "Window.SplitGroupHorizontally",
            "Window.SplitGroupVertically",
            "Window.CloseActiveGroup",
            "Window.MoveGroupToGroup",
            // 六个工具面板。读者是 `ui/panes.rs`——每个开关开着就画一个
            // 有真实内容的面板（会话树 / 当前屏行摘要 / 预设命令 / 会话列表 /
            // 会话状态 / 在跑的隧道），不是空转 bool。
            "Window.TogglePaneExplorer",
            "Window.TogglePaneOutline",
            "Window.TogglePaneSender",
            "Window.TogglePaneSession",
            "Window.TogglePaneShell",
            "Window.TogglePaneScript",
            "Window.TogglePaneTransfer",
            // v0.1 工作台 15 条。读者是 `ui/center.rs` 的对应页面——
            // 每条命令打开一个有真实内容/动作的页面，不是同义开关；
            // `Text.HighlightText` 的读者还包括 `ui/terminal.rs` 的常驻高亮渲染。
            "Terminal.OpenSession",
            "Terminal.Save",
            "Terminal.SaveSelectionAs",
            "Terminal.OpenSessionSettings",
            "Terminal.OpenDefaultSessionSettings",
            "Application.OpenProfileDialog",
            "Window.ToolbarList",
            "Window.ToolbarCustomize",
            // 这两条此前是同一条命令。现在 `GotoAddressBar` 是地址栏
            // （连接目标输入框：解析 / 候选 / 最近使用在 `term_app::address`，
            // 浮层与 `dispatch_action` 在投影层），`GotoCommandLine` 是那个
            // 「把一行当 shell 命令发出去」的工作台页。两条都算已接线：
            // 各有各的读者，不是同一个效果的别名。
            "Window.GotoAddressBar",
            "Window.GotoCommandLine",
            "Terminal.CompleteHistory",
            "Terminal.SchemeList",
            "Window.WindowList",
            "Window.WindowManager",
            "Window.OpenFocusModeSettings",
            "Text.HighlightText",
            // 锁屏。读者是 `ui/lock.rs` 的全屏遮罩 + `term-vault` 的主密码校验。
            "Window.LockScreen",
            // 重置布局。读者是 `ui/layout/dock_paint.rs::apply_layout_action`
            // ——把拖过 / 浮过的面板全部收回默认停靠位（`DockLayout::reset`）。
            "Window.ResetLayout",
            // 三条。读者分别是——同步输入：`ui/terminal.rs` 的按键扇出
            // 与状态栏 SYNC 徽标；选中命令：`ui/effects.rs` 按内核 OSC 133
            // 的行号取 `B..C` 那段；隧道管理：`ui/tunnel_manager.rs` 的列表 +
            // 增删规则（规则语法仍只有 `tunnels::parse_rule` 一处）。
            "Terminal.SyncInput",
            "Text.SelectCommand",
            "Terminal.ManageTunnels",
            // 2026-11-13：设置页与「最近使用」的两条直达。
            //
            // `ManageTriggers` 一分为二：`show_settings` 由 reducer 置位（与
            // `Window.OpenSettings` 同一句），落到屏幕上是
            // `ui/effects.rs::Effect::FocusTriggersPage` → 把
            // `TerminalWindow::settings_page` 翻到 `SettingsPage::Triggers`，
            // 那一页的真实现是 `ui/dialogs/settings_live_triggers.rs`。
            // `ClearRecentUrls` 的读者是 `ui/address/mod.rs::clear_recent_targets`
            // ——清空 `address.recent` 并 load-modify-save 写回
            // `Prefs.recent_targets`，地址栏右键「Clear Recent」走的是同一个函数。
            "Terminal.ManageTriggers",
            // `ManageOneKeys` 与 `ManageTriggers` 同形：reducer 置 `show_settings`
            // + `Effect::FocusOneKeysPage` → 设置页翻到 `SettingsPage::OneKeys`，
            // 真实现是 `ui/dialogs/settings_live_onekeys.rs`。
            "Terminal.ManageOneKeys",
            "Terminal.ClearRecentUrls",
            "Text.Find",
            "Text.FindNext",
            "Text.FindPrevious",
            "Text.Copy",
            "Text.Paste",
            // 17 条选择命令。读者是 `ui/selection` 的落点（`Effect::Select`
            // → `selection::apply`），鼠标拖动、Shift+方向键、菜单点选共用它。
            "Text.SelectAll",
            "Text.SelectWord",
            "Text.SelectLine",
            "Text.SelectToDocumentStart",
            "Text.SelectToDocumentEnd",
            "Text.SelectToLineHome",
            "Text.SelectToLineEnd",
            "Text.SelectToPreviousChar",
            "Text.SelectToNextChar",
            "Text.SelectToPreviousLine",
            "Text.SelectToNextLine",
            "Text.SelectToPreviousPage",
            "Text.SelectToNextPage",
            "Text.SelectToPreviousWordStart",
            "Text.SelectToNextWordStart",
            "Text.SelectToPreviousWordEnd",
            "Text.SelectToNextWordEnd",
            // 2026-11-13：26 条光标 / 选区命令。读者与上面 17 条**同一个**
            // （`Effect::Select(Cmd::Move)` → `selection::apply_with_blocks`），
            // 但这次不是纯映射——26 条各有自己的位移规则，边界判定在
            // `selection/geometry.rs` / `selection/words.rs`，逐条单测在
            // `selection.rs`（`every_cursor_command_parses_to_its_own_op` 钉 ID，
            // 其后每族一条落点断言）。`Command` / `Scope` 两条需要 OSC 133 块表，
            // 空表时前者不动、后者退化到窗口首末行。
            "Text.SelectColumnToPreviousChar",
            "Text.SelectColumnToNextChar",
            "Text.SelectColumnToPreviousLine",
            "Text.SelectColumnToNextLine",
            "Text.SelectColumnToPreviousPage",
            "Text.SelectColumnToNextPage",
            "Text.SelectColumnToPreviousParagraph",
            "Text.SelectColumnToNextParagraph",
            "Text.SelectColumnToLineHome",
            "Text.SelectColumnToLineEnd",
            "Text.GotoPreviousWordPart",
            "Text.GotoNextWordPart",
            "Text.GotoPreviousStutteredPage",
            "Text.GotoNextStutteredPage",
            "Text.GotoPreviousParagraph",
            "Text.GotoNextParagraph",
            "Text.GotoPreviousCommand",
            "Text.GotoNextCommand",
            "Text.GotoPreviousScope",
            "Text.GotoNextScope",
            "Text.GotoPreviousIndentation",
            "Text.GotoNextIndentation",
            "Text.GotoPairStart",
            "Text.GotoPairEnd",
            "Text.GotoPreviousPair",
            "Text.GotoNextPair",
            // 9 条集合命令。读者是 `ui/terminal/pane.rs` 的落点——它们改的是
            // 标签上的 `Selections`（活动下标 / 成员 / 锁 / 形状 / 端点），
            // 纯函数在 `term-app/selection.rs::apply_cmd`；非活动选区在
            // `ui/terminal/view.rs` 里淡一档画出来，所以「切到哪一份」看得见。
            "Text.ActivateNextSelection",
            "Text.ActivatePreviousSelection",
            "Text.LockSelection",
            "Text.DropSelection",
            "Text.CancelSelection",
            "Text.SplitSelection",
            "Text.InvertSelection",
            "Text.SwitchStreamAndColumn",
            "Text.ExchangeCaretAndAnchor",
            // Goto 的 4 条光标移动。读者与 17 条选择命令同一条路径
            // （`Effect::Select(Cmd::Move)` → `selection::apply`），
            // 语义是「只挪活动选区的 head」，不是另开一套光标。
            "Text.CharLeft",
            "Text.CharRight",
            "Text.LineUp",
            "Text.LineDown",
            // Goto 另外 10 条。读者与上面四条**同一个**：`parse` 只把它们映射到
            // 既有的 `Op`（词首 / 词尾 / 翻页 / 行首行尾 / 文首文末），落地仍是
            // `selection::apply` → `Effect::Select(Cmd::Move)`。没有新增位移规则，
            // 也没有第二套光标——`selection.rs::goto_commands_land_where_the_menu_says`
            // 逐条验落点（行尾列号 / 词首列号 / 翻页夹取 / 文末行号）。
            "Text.GotoPreviousWord",
            "Text.GotoPreviousWordEnd",
            "Text.GotoNextWord",
            "Text.GotoNextWordEnd",
            "Text.GotoPreviousPage",
            "Text.GotoNextPage",
            "Text.GotoLineHome",
            "Text.GotoLineEnd",
            "Text.GotoDocumentStart",
            "Text.GotoDocumentEnd",
            // 2026-11-09：Transfer Binary 的 5 条。读者分两段——口令由
            // `apps/desktop/src/xfer_run.rs` 的驱动线程消费（它才持有会话通道），
            // 进度与结果由 `apps/desktop/src/ui/terminal/xfer.rs::draw` 画成终端底部
            // 的一行状态（在跑 / 跑完没值 / 失败 / 空闲四种说法互不混淆）。
            "Terminal.SendXModem",
            "Terminal.ReceiveXModem",
            "Terminal.SendYModem",
            "Terminal.ReceiveYModem",
            "Terminal.CancelModem",
            // 同日：ZMODEM 两条，读者是同一条驱动线程的另一台状态机
            // （`term-zmodem` 的 `Sender` / `Receiver`，经 `xfer_run.rs` 的
            // `ZTxEngine` / `ZRxEngine` 两个壳接进同一套 `Engine` 循环）。
            // 「点了有效果」的证据在 `apps/desktop/tests/xfer_zmodem.rs`：
            // 真 PTY 里起真 `sz`/`rz`，落盘内容与源文件逐字节相同。
            "Terminal.SendZModem",
            "Terminal.ReceiveZModem",
            // 标签右键 10 个影子入口。读者是
            // `ui/effects.rs::apply_tab_action`——它把意图落到
            // `close_tab_with_history` / `reopen_closed` / `duplicate_tab` /
            // 分屏 / 复制 / 上色这些**既有**方法上，不是空转开关。
            // `ui/shell/tab_menu.rs` 不再直接改 `TerminalWindow`，
            // 只发 `Action::Tab(...)`，所以「真实入口」与这张表同源。
            "Terminal.TabActivate",
            "Terminal.TabClose",
            "Terminal.TabCloseOthers",
            "Terminal.TabCloseAll",
            "Terminal.TabReopenClosed",
            "Terminal.TabDuplicate",
            "Terminal.TabSplit",
            "Terminal.TabCopyName",
            "Terminal.TabCopyUrl",
            "Terminal.TabColor",
            // tmux 控制模式浮层。读者是
            // `ui/panels/tmux.rs::draw_tmux_panel`（`ui/mod.rs` 每帧调用），
            // 开关由 `ShellAction::TogglePaneTmux` 翻 `ViewState.show_tmux`。
            "Window.TogglePaneTmux",
            // 2026-11-10：显示模式切换。读者是投影层的 `Tab.mode`——本层只吐
            // `Effect::ToggleHex`，真正的落地（翻 `Tab.mode`、不落盘）在
            // `apps/desktop`。归 `FeatureId::Bin`：它换的是「怎么看字节」，
            // 不是窗口 chrome。
            "Text.ToggleHexEdit",
            // 2026-11-14：调试覆盖层。读者是 `apps/desktop/src/ui/terminal/
            // pane_overlays.rs` 的 `if !bin && tab.debug` —— 它去调
            // `ui/terminal/debug_paint.rs::paint`，点了屏幕上的像素**真的变**：
            // 每格的 `Attrs` 八位变成 pip 架与两条贯穿线、`fg`/`bg` 的
            // `Indexed`/`Rgb` 变成 1 / 3 个小方块、`Cell.wide` 变成右缘竖线。
            // 归 `FeatureId::Shell`（走 `Action::Shell(_)` 的兜底归属）：
            // 命令 ID 本身就是 `Window.*` 前缀，与 `Window.ToggleFocusMode` 一族。
            "Window.ToggleDebugMode",
            // 2026-11-19：只读查看（`Window ▸ Local Mode`）。**四位读者**全在
            // 投影层，每一位都是一条独立判据：`Tab::write` / `write_paste`
            // （不写 PTY）、`Tab::poll`（钉住视口 + 跳过触发器求值）、
            // `ui/terminal/pane.rs`（不发鼠标上报 / 不弹补全 / 不落粘贴）、
            // `ui/terminal/view.rs::paint_cursor`（不画光标）。归
            // `FeatureId::Shell`（`Window.*` 前缀 + `Action::Shell(_)` 兜底）。
            "Window.SetLocalMode",
            // 2026-11-15：`Application.NewWindow`。读者是
            // `apps/desktop/src/new_window.rs` 的 `SpawnPlan::launch`——它拉起一个
            // 完整的**新进程**（窗口态独立、不恢复布局），「效果」落在窗口
            // 系统而不是像素上，与 `Effect::Exit` 同一类。归 `FeatureId::Shell`，
            // 走 `Action::Shell(_)` 的兜底归属，不新增 `FeatureId`。
            "Application.NewWindow",
            // 2026-11-11：hex 三个参数子菜单的 13 条档位命令。读者全是
            // `ui/effects.rs::set_tab_hex`——它只改**活动标签**的 `Tab.hex`
            // 一个旋钮、整份过一遍 `HexParams::from_prefs` 夹一次（不落盘；
            // 落盘的那一层是设置 ▸ Binary 页的 `Prefs.hex_*`）；
            // `bin.rs`（绘制）与 hex 视图表头读同一份值。设置页 Binary 页那四行
            // 是**另一层**（默认参数），不读 `Tab.hex`。
            // 归 `FeatureId::Bin` 的理由与上面那条相同：换的是怎么看字节。
            "View.HexBase2",
            "View.HexBase7",
            "View.HexBase8",
            "View.HexBase10",
            "View.HexBase16",
            "View.HexColumns8",
            "View.HexColumns16",
            "View.HexColumns24",
            "View.HexColumns32",
            "View.HexGroupChars1",
            "View.HexGroupChars2",
            "View.HexGroupChars4",
            "View.HexGroupChars8",
            // 2026-11-13：栅格可视化 10 条。读者是投影层的 `effect::SetMarks` →
            // `ui/effects.rs::set_marks_knob` → `TerminalWindow::set_marks_knob`
            // （四个翻转 + 五档设档，改**活动标签**的 `Tab.marks`，不落盘）。
            // 「点了屏幕上哪一像素会变」逐个写在这里，不是「翻了个没人看的 bool」：
            //   · LineEndings → 逻辑行末尾多一个 `$` 字形
            //   · ColumnMarker → 第 N 列一条贯穿竖线
            //   · IndentGuides → 行首每 4 列一条 1px 竖线
            //   · WrapSymbols → 自动换行续行右下角一个 L 形勾
            //   · Whitespace(*) → 空格小方点 / Tab 箭头（五档范围不同）
            //   · CaretLine → 光标行一层 7% 淡底（压在字形之下）
            // 归 `FeatureId::Marks`：既不换「怎么看字节」，也不动窗口 chrome。
            "View.ToggleLineEndings",
            "View.ToggleColumnMarker",
            "View.ToggleIndentGuides",
            "View.ToggleWrapSymbols",
            "View.WhiteSpacesHideAll",
            "View.WhiteSpacesShowAll",
            "View.WhiteSpacesShowInSelection",
            "View.WhiteSpacesShowInLineStart",
            "View.WhiteSpacesShowInLineEnd",
            "Text.HighlightCaretLine",
            // 2026-11-13：窗口透明度四条。读者分两处——
            //   · 三档预设 `SetWindow{Opaque,Translucent,Transparent}`：
            //     `ui/effects.rs` 落地到 `TerminalWindow::set_window_opacity`
            //     （改运行期副本 + `save_prefs` 写回 `Prefs.window_transparent`
            //     / `window_opacity`）；`main.rs::update` 每帧按钳制后的值把
            //     alpha 装到窗口表面色与终端区底色上，并在开关
            //     变化时只发一次 `ViewportCommand::Transparent`。
            //   · 第四条 `SetWindowOpacity`：`show_settings` 由 reducer 置位，
            //     `Effect::FocusAppearancePage` 把设置页翻到 `Appearance`，
            //     那一页的开关 + 滑杆是真控件（`ui/dialogs/settings_live.rs`）。
            // 归 `FeatureId::Shell`：它改的是整窗 chrome，不是栅格上的像素。
            // 语义假设（可在 GUI 验收后调整）见 `vocab.rs::WindowOpacityPreset`。
            "Application.SetWindowOpaque",
            "Application.SetWindowTranslucent",
            "Application.SetWindowTransparent",
            "Application.SetWindowOpacity",
            // 2026-11-14：折叠族九条。读者全部是 `ui/effects.rs::apply_fold`——
            // 它把 `FoldCmd` 交给纯函数 `decide_fold`，再把结果写回
            // `Tab.folds` / `Tab.viewport` / `last_request`。
            // 「点了屏幕上哪一行会变」逐条写在这里（渲染在
            // `terminal/setup.rs::folded_view`，它在 `folds` 非空时把被折的行丢出窗口）：
            //   · Fold / `MarginFold`(Toggle) → 光标所在的块收起，屏幕上少掉
            //     `b - a - 1` 行输出，折叠头（那条命令行）留着
            //   · Unfold → 那一块摊开，行数长回来
            //   · FoldInside / UnfoldInside → 当前视口内所有块一起折 / 解
            //     （本仓的块平坦不嵌套，见 `vocab::FoldCmd::FoldInside` 的取舍）
            //   · FoldAll / UnfoldAll → 全部块 / 整个集合
            //   · GotoPreviousFold / GotoNextFold → 视口跳到上一个 / 下一个折叠头
            // 归 `FeatureId::Margin`：折叠就是 margin 折叠标记列的内容。
            "View.Fold",
            "View.Unfold",
            "View.FoldInside",
            "View.UnfoldInside",
            "View.FoldAll",
            "View.UnfoldAll",
            "Text.MarginFold",
            "Text.GotoPreviousFold",
            "Text.GotoNextFold",
        ]
        .into_iter()
        .collect();
        let actual: BTreeSet<&str> = inventory()
            .iter()
            .filter(|c| c.is_implemented())
            .map(|c| c.id)
            .collect();
        assert_eq!(actual, expected, "已接线的命令集合变化需显式确认");
    }

    /// 影子入口不能只「进了表」而没有动作。
    /// 这条与 `tab.rs::shadow_entries_and_parse_are_the_same_ten_commands`
    /// 从两头夹住同一件事：表、parse、能力归属、真实落地四处都齐。
    #[test]
    fn shadow_entries_are_all_wired_and_owned_by_tab() {
        assert_eq!(crate::tab::SHADOW_ENTRIES.len(), 10);
        for entry in crate::tab::SHADOW_ENTRIES {
            let cap = describe(entry.id);
            assert_eq!(
                cap.state,
                CapState::Implemented,
                "{} 是真实可点入口，不能被置灰",
                entry.id
            );
            assert_eq!(cap.feature, Some(FeatureId::Tab), "{}", entry.id);
        }
    }

    #[test]
    fn every_implemented_command_belongs_to_a_declared_feature() {
        for cap in inventory().iter().filter(|c| c.is_implemented()) {
            let feature = cap.feature.expect("已接线命令必须有归属能力");
            assert!(FeatureId::ALL.contains(&feature), "{:?}", feature);
        }
    }

    #[test]
    fn planned_commands_have_no_feature() {
        for cap in inventory().iter().filter(|c| !c.is_implemented()) {
            assert_eq!(cap.feature, None, "{} 不该有归属能力", cap.id);
        }
    }

    #[test]
    fn each_declared_feature_owns_at_least_one_command() {
        let all = inventory();
        for feature in FeatureId::ALL {
            let n = all.iter().filter(|c| c.feature == Some(feature)).count();
            assert!(n > 0, "{:?} 没有任何命令接管", feature);
        }
    }

    /// 新窗口：能点、有归属、归 `Shell`（`Action::Shell(_)` 的兜底归属）。
    /// 单开一个 `FeatureId` 会为这一条命令把 `ALL` 与 `name()` 各改一遍，零收益。
    #[test]
    fn new_window_is_wired_and_owned_by_shell() {
        assert!(is_implemented("Application.NewWindow"));
        let cap = describe("Application.NewWindow");
        assert_eq!(cap.state, CapState::Implemented);
        assert_eq!(cap.feature, Some(FeatureId::Shell));
    }

    #[test]
    fn hex_edit_is_wired_and_owned_by_bin() {
        // `Mode ▸ Hex Edit` 不再是灰的。三件事一起钉：能点、有归属、
        // 归属是 `Bin` 而不是「因为 ID 以 `Text.` 开头就归 Text」——
        // 它是换一种看法，不是编辑器语义。
        let cap = describe("Text.ToggleHexEdit");
        assert_eq!(cap.state, CapState::Implemented);
        assert_eq!(cap.feature, Some(FeatureId::Bin));
    }

    #[test]
    fn find_is_registered_as_a_feature() {
        assert_eq!(describe("Text.Find").feature, Some(FeatureId::Find));
        assert_eq!(describe("Text.Find").state, CapState::Implemented);
        // 后 `Terminal.Save` 已接线；ZMODEM 也在 2026-11-09 接通了，
        // 所以换一条仍明确属于 v0.3+ 的占位命令来钉「置灰这一侧还有东西」。
        // 2026-11-14：原来钉的是 `Text.MarginFold`，它这一刀接通了；改用
        // `Window.GotoSymbol`——仍然解析不了（符号表要先有「符号」这个概念），
        // 且它不在任何已接线的能力族里。
        assert_eq!(describe("Window.GotoSymbol").state, CapState::Planned);
    }

    /// 2026-11-14：折叠族九条**逐条**钉住「可点 + 归属」。
    ///
    /// 逐条写、不用循环：九条里只要有一条漏了 `parse` 分支，循环会跟着实现一起
    /// 歪（拿实现当期望值），而写死的语句不会；而且循环写法的反例消息只有
    /// `"{id}"` 一句，真红的时候还要回去数第几条。
    #[test]
    fn the_nine_fold_commands_are_wired_and_owned_by_margin() {
        let cap = |id: &str| (describe(id).state, describe(id).feature);
        let wired = (CapState::Implemented, Some(FeatureId::Margin));
        assert_eq!(cap("View.Fold"), wired);
        assert_eq!(cap("View.Unfold"), wired);
        assert_eq!(cap("View.FoldInside"), wired);
        assert_eq!(cap("View.UnfoldInside"), wired);
        assert_eq!(cap("View.FoldAll"), wired);
        assert_eq!(cap("View.UnfoldAll"), wired);
        assert_eq!(cap("Text.MarginFold"), wired);
        assert_eq!(cap("Text.GotoPreviousFold"), wired);
        assert_eq!(cap("Text.GotoNextFold"), wired);
    }

    /// 折叠九条接通之后，四条 margin **列**开关不许被顺手改道。
    ///
    /// 十条 ID 里五条带 `Margin`，五条里四条是列开关、一条是折叠内容。判据放在
    /// 这里而不是 `margin.rs`，因为「ID 看起来像」正是最容易混的量：`MarginFold`
    /// 若被解析成 `Toggle(MarginPart::Fold)`，`capability` 照样说「已接线」，
    /// 但点下去屏幕上什么都不会变——假开关。
    #[test]
    fn the_four_margin_columns_still_toggle_a_column() {
        assert_eq!(
            Action::parse("Text.MarginBlank"),
            Some(Action::Margin(crate::vocab::MarginAction::Toggle(
                crate::vocab::MarginPart::Blank
            )))
        );
        assert_eq!(
            Action::parse("Text.MarginNumber"),
            Some(Action::Margin(crate::vocab::MarginAction::Toggle(
                crate::vocab::MarginPart::Number
            )))
        );
        assert_eq!(
            Action::parse("Text.MarginSymbol"),
            Some(Action::Margin(crate::vocab::MarginAction::Toggle(
                crate::vocab::MarginPart::Symbol
            )))
        );
        assert_eq!(
            Action::parse("Text.MarginTimestamp"),
            Some(Action::Margin(crate::vocab::MarginAction::Toggle(
                crate::vocab::MarginPart::Timestamp
            )))
        );
        // 而 `MarginFold` 走的是折叠，**不是** `MarginPart::Fold`（那一格没有读者）。
        assert_eq!(
            Action::parse("Text.MarginFold"),
            Some(Action::Fold(crate::vocab::FoldCmd::Toggle))
        );
    }

    #[test]
    fn both_clipboard_commands_are_wired() {
        // 接复制（写剪贴板）；接粘贴（`ViewportCommand::RequestPaste`
        // 让框架读，不需要平台依赖）；接「粘贴选区」（选区文本直接写给 PTY，
        assert_eq!(describe("Text.Copy").state, CapState::Implemented);
        assert_eq!(describe("Text.Paste").state, CapState::Implemented);
        assert_eq!(describe("Terminal.PasteSelectedText").state, CapState::Implemented);
        assert_eq!(describe("Text.Copy").feature, Some(FeatureId::Clipboard));
        assert_eq!(describe("Text.Paste").feature, Some(FeatureId::Clipboard));
        assert_eq!(
            describe("Terminal.PasteSelectedText").feature,
            Some(FeatureId::Clipboard)
        );
    }
}
