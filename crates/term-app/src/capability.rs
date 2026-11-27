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

/// 全部真实可点入口及其状态：菜单树 + 影子入口 + 数据层的工具条 / 下拉，按菜单顺序。
///
/// 标签右键的 10 个动作不画在菜单栏里，但它们是**真实入口**。
/// 以前 `inventory()` 只遍历菜单树，于是「已接线 N 条」少算这一块；
/// 现在把 `tab::SHADOW_ENTRIES` 并进同一张表，判定仍只用 `Action::parse`，
/// 不维护第二份清单。加一条影子入口而不给它 `parse`，这里立刻置灰。
///
/// 同样的道理：`menu::bars` 里的 BAR_TOOLS / DOCK_ITEMS / TUNNEL_ITEMS /
/// GOTO_ITEMS / ZOOM_ITEMS 都是可点的命令入口，只是画在菜单栏**之外**
/// （图标带、布局下拉、Tunnels 下拉、地址栏 Goto、View ▸ Zoom）。此前它们
/// 从来没进过能力表——「菜单里点得到」的统计少了这块真实入口。
/// 这些 ID 大多与菜单树重复（同一能力两个入口），所以按 id 去重：统计的是
/// **不同命令**的数量，不是入口像素的数量；`menu_has_no_duplicate_command_ids`
/// 仍单独钉住菜单/影子之间那一处允许的双入口。
pub fn inventory() -> Vec<Capability<'static>> {
    let mut out = Vec::new();
    for menu in menu::menus() {
        collect(&menu.items, &mut out);
    }
    for entry in crate::tab::SHADOW_ENTRIES {
        out.push(describe(entry.id));
    }
    for tool in crate::menu::BAR_TOOLS {
        out.push(describe(tool.id));
    }
    for item in crate::menu::DOCK_ITEMS {
        out.push(describe(item.id));
    }
    for item in crate::menu::TUNNEL_ITEMS {
        out.push(describe(item.id));
    }
    for (_, id) in crate::menu::GOTO_ITEMS {
        out.push(describe(id));
    }
    for (_, id) in crate::menu::ZOOM_ITEMS {
        out.push(describe(id));
    }
    // 终端内容右键的四条命令与 tmux 浮层开关都不画在菜单栏里，但都是真实
    // 可点入口（右键 / comp 夹具 / 命令面板）。ID 只来自数据层常量表，这里
    // 不写第二个字面量。
    for (_, id) in crate::menu::context_ids::TERMINAL_CONTEXT_COMMANDS {
        out.push(describe(id));
    }
    out.push(describe(crate::menu::context_ids::TOGGLE_PANE_TMUX));
    let mut seen = std::collections::BTreeSet::new();
    out.retain(|cap| seen.insert(cap.id));
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
        // `inventory()` 现在按 id 去重（统计的是不同命令，不是入口像素），
        // 所以这里断言的是「去重后确实没有重复」。`Terminal.TabReopenClosed`
        // 在菜单栏与标签右键各有一个入口，去重前出现过两次——那是允许的
        // 双入口，但能力台账只算一条；`bars_entries_are_registered…` 另钉
        // 五个非菜单入口表也都在表里。
        let all = inventory();
        let mut seen = BTreeSet::new();
        let repeated: BTreeSet<&str> = all
            .iter()
            .map(|c| c.id)
            .filter(|id| !seen.insert(*id))
            .collect();
        assert!(repeated.is_empty(), "inventory 必须按 id 去重：{repeated:?}");
        assert!(
            all.iter().any(|c| c.id == "Terminal.TabReopenClosed"),
            "菜单栏 / 标签右键共用的 Reopen 仍要在表里，且只出现一次"
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
        // `sz` / `rz`，走完 `XferHandle::start` 之后逐字节比对落盘内容。该测试默认
        // `#[ignore]`，发布门显式 `--ignored` 才跑；缺 lrzsz 直接 FAIL，不再「跳过即绿」。
        // （`term-zmodem` 内核本身另有一份与真实 lrzsz 的互通测试。）
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
        // 当时已接线 215→216、总数 221→222，占位 6 不变（没动任何置灰项）。
        // 2026-11-19 只读查看：`Window.SetLocalMode` 从「置灰占位」变成真开关
        // → 当时已接线 216→217、占位 6→5（以下都是接线当时的流水号）。
        // 2026-11-23：`Window.GotoSymbol` 接线（符号表 = OSC 133 命令块）
        // → 当时已接线 217→218、占位 5→4。
        // 同日：`Window.SetRemoteMode` 接线（退出只读，勾改 Live）
        // → 当时已接线 218→219、占位 4→3。
        // 2026-11-24：`Text.ToggleWordWrap` 接线（软换行，段表按位现算）
        // → 当时已接线 219→220、占位 3→2。
        // 上面整段都是当时的历史流水（含旧总数 222），现行账本已冻结为
        // 已接线 222 / 占位 2 / 总数 224（见下面三条断言），别照抄旧口径。
        // 2026-11-25：把 `menu::bars` 的五个入口表并进 inventory 后，此前
        // 从未进过能力台账的四条命令终于登记：三条整带开关 + Tunnels 总线开关。
        // 同时 inventory 按 id 去重，菜单栏 / 标签右键共用的 Reopen 只算一条：
        // 已接线 214→217、占位 2 不变、总数 216→219（+4 新登记 −1 去重）。
        // 同日统一命令入口：终端内容右键四条（CopyCommand / ClearScreen /
        // ClearScrollback / ClearAll）从私有 Work 提升到 parse + 台账，孤儿命令
        // `Window.TogglePaneTmux` 也补进台账 → 已接线 217→222、占位 2 不变、
        // 总数 219→224。
        // 2026-11-26 C2 菜单 1:1：`Search ▸ Search Online` 的六个引擎、
        // `View ▸ Syntax Schemes` 的七个 scheme、`Selection` 补 5 条、
        // `Select To` 补 16 条、Toolbar 两行占位共 36 条新命令全部未接线，
        // 按 ADR-0015 置灰；同时原先占菜单位的四条（SearchOnline / SchemeList /
        // ToolbarList / ToolbarCustomize）退出菜单树。
        // → 已接线 222→218、占位 2→38、总数 224→256。
        // 2026-11-27 M1：把那 36 条灰项里的 34 条真正接线——
        //   · Search Online 六条按引擎名打开系统浏览器（选区 / 输入缓冲为查询词）；
        //   · Syntax Schemes 七条用 `builtin_scheme` 换运行中调色板并写回 scheme_name；
        //   · Select 5 + Select To 16 全部落到 selection 的纯函数（Fold 三条读折叠
        //     输入行表；其余复用既有 Op 边界规则）。
        // 两条 Toolbar 行仍置灰：本版没有自定义工具栏条目模型，原图里它们本来
        // 就是空态灰字，不摆点了没反应的假控件。
        // → 已接线 218→252、占位 38→4、总数 256 不变（一条菜单都没增删）。
        // 2026-11-27：`Tool ▸ Script` 新增 Run / Stop / Record / New / Manage
        // 五条并接线（面板显隐仍只有 `Window.TogglePaneScript` 一条）→ 已接线
        // 252→257、占位 4 不变、总数 256→261。
        let (implemented, planned) = counts();
        assert_eq!(implemented, 257, "已接线命令数变化需显式确认");
        assert_eq!(planned, 4, "占位命令数变化需显式确认");
        assert_eq!(inventory().len(), 261);
    }

#[cfg(test)]
#[path = "capability_readers_tests.rs"]
mod capability_readers_tests;

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
        // `Window.GotoSymbol` 2026-11-23 已接线（符号表 = OSC 133 命令块）；
        // `Window.SetRemoteMode` 同日接线；`Text.ToggleWordWrap` 2026-11-24
        // 接线。这里改用仍未接线的 `Text.ToggleVirtualSpace`（待实现的终端
        // 行编辑行为）钉住置灰这一侧。
        assert_eq!(describe("Text.ToggleVirtualSpace").state, CapState::Planned);
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

    /// 源码级读者判据：`every_implemented_command_has_a_reader` 的两张白名单
    /// 相等只能证明「登记一致」，证明不了「点了有效果」。对**新增命令**再要求
    /// 一次硬证据：命令 ID 必须字面出现在 `apps/desktop/src/ui` 的某个 .rs 里
    /// （那一层才有像素 / 对话框 / 窗口副作用）。老命令多数经 `Action::parse`
    /// 统一派发，没有字面 ID，所以不在这一组里；新登记的命令必须过这一关。
    /// fail-closed：目录读不到 / 文件读不到 / 一个 .rs 都没有，一律 FAIL。
    #[test]
    fn newly_wired_commands_have_a_literal_ui_reader() {
        const NEW_WITH_LITERAL_READER: &[&str] = &[
            "View.HexBase2", "View.HexBase7", "View.HexBase8", "View.HexBase10", "View.HexBase16",
            "View.HexColumns8", "View.HexColumns16", "View.HexColumns24", "View.HexColumns32",
            "View.HexGroupChars1", "View.HexGroupChars2", "View.HexGroupChars4", "View.HexGroupChars8",
            "View.ToggleLineEndings", "View.ToggleColumnMarker", "View.ToggleIndentGuides", "View.ToggleWrapSymbols",
            "View.WhiteSpacesHideAll", "View.WhiteSpacesShowAll", "View.WhiteSpacesShowInSelection",
            "View.WhiteSpacesShowInLineStart", "View.WhiteSpacesShowInLineEnd",
            "Application.SetWindowOpaque", "Application.SetWindowTranslucent", "Application.SetWindowTransparent",
            "Application.SetWindowOpacity", "Window.ToggleDebugMode", "Text.ToggleHexEdit", "Text.ToggleWordWrap",
        ];
        fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let entries = std::fs::read_dir(dir)
                .unwrap_or_else(|e| panic!("读者目录必须可读（fail-closed）: {}: {e}", dir.display()));
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect_rs(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/src/ui");
        let mut files = Vec::new();
        collect_rs(&root, &mut files);
        assert!(!files.is_empty(), "没有 UI 源码可查，不能算通过");
        let mut source = String::new();
        for file in &files {
            source.push_str(&std::fs::read_to_string(file).unwrap_or_else(|e| {
                panic!("读者源码必须可读（fail-closed）: {}: {e}", file.display())
            }));
        }
        for id in NEW_WITH_LITERAL_READER {
            assert!(is_implemented(id), "{id} 必须已接线");
            assert!(source.contains(id), "{id} 没有 apps/desktop/src/ui 的字面读者");
        }
    }

    /// 工具条 / 下拉的每条入口都在能力表里（按 id 去重后同一个命令只算一次）。
    /// 这条门禁补的是「菜单树之外的真实入口漏统计」这一块。
    #[test]
    fn bars_entries_are_registered_without_inflating_command_counts() {
        let ids: BTreeSet<&str> = inventory().iter().map(|c| c.id).collect();
        for tool in crate::menu::BAR_TOOLS {
            assert!(ids.contains(tool.id), "{} 漏进能力表", tool.id);
        }
        for item in crate::menu::DOCK_ITEMS {
            assert!(ids.contains(item.id), "{} 漏进能力表", item.id);
        }
        for item in crate::menu::TUNNEL_ITEMS {
            assert!(ids.contains(item.id), "{} 漏进能力表", item.id);
        }
        for (_, id) in crate::menu::GOTO_ITEMS {
            assert!(ids.contains(id), "{} 漏进能力表", id);
        }
        for (_, id) in crate::menu::ZOOM_ITEMS {
            assert!(ids.contains(id), "{} 漏进能力表", id);
        }
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
