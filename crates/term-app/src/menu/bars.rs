//! 菜单栏右侧、标签条右端与布局下拉的四组条目。
//!
//! 从 `menu.rs` 搬出来的一段。放在数据层是为了 I5：`ui/` 里不许出现命令 ID
//! 字面量，而这些条目各自要派发一条命令 / 弹一个菜单，ID 必须由这里给。

// ── 菜单栏右侧的工具条与布局下拉 ────────────────────────────────────────────
//
// 2026-11-07：这两张表放在数据层，不是「为了对称」。I5 的判据是**投影层不许
// 出现命令 ID 字面量**（`scripts/verify.sh` 4e 用 `"[A-Z]+.[A-Z]+"` 正则扫
// `apps/desktop/src/ui`），而菜单栏右侧那四件正是命令的另一个入口。把它们留在
// `ui/` 里就要么违反 I5，要么在 UI 里再抄一份 ID→绘制 的映射——两条都比把
// 数据搬过来差。

/// 菜单栏右侧工具条的图标。最右的布局按钮**不在这里**：它是下拉容器，
/// 本身不对应任何命令。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarGlyph {
    Search,
    Tunnels,
    Focus,
}

/// 工具条的一件。
///
/// `label` 为空表示只有图标（视觉基线里放大镜就是这种）。`glyph` 只描述
/// 「画哪个形状」，具体怎么画由投影层决定——数据层不出现任何 UI 类型。
pub struct BarTool {
    pub label: &'static str,
    pub id: &'static str,
    pub glyph: BarGlyph,
}

/// **视觉顺序：从左到右**。投影层右对齐摆放，所以它按 `.rev()` 遍历。
///
/// 顺序依据是视觉基线（2348×60）逐像素量出来的：放大镜 → `Tunnels` →
/// `Focus Mode` → 布局按钮。
pub const BAR_TOOLS: &[BarTool] = &[
    BarTool {
        label: "",
        id: "Text.Find",
        glyph: BarGlyph::Search,
    },
    BarTool {
        label: "Tunnels",
        id: "Terminal.ManageTunnels",
        glyph: BarGlyph::Tunnels,
    },
    BarTool {
        label: "Focus Mode",
        id: "Window.ToggleFocusMode",
        glyph: BarGlyph::Focus,
    },
];

/// 标签条右端的三件（逐像素顺序：`group 名字` → `＋` → `≡`）。
///
/// 放在纯数据层是为了 I5：`ui/` 里不许出现命令 ID 字面量，而这三件各自要
/// 派发一条命令 / 弹一个菜单，命令 ID 必须由这里给。
///
/// 2026-11-15 夜语义更正：第一个字母**不是**「字体缩放菜单」，而是
/// **group 的名字**（`A/B/C/D`，每个 group 各不相同）。参考实现 `Merge To Group ▸`
/// 子菜单按名字列出目标组；缩放在 `View ▸ Zoom` 另有独立入口。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TabTool {
    /// group 名字（只读标识，不派发命令、不弹菜单）。
    GroupName,
    /// `＋`：新建会话。
    NewTab,
    /// `≡`：标签列表菜单入口。
    TabList,
}

/// **视觉顺序：从左到右**。投影层右对齐摆放，按 `.rev()` 遍历。
pub const TAB_TOOLS: &[TabTool] = &[TabTool::GroupName, TabTool::NewTab, TabTool::TabList];

/// `＋` 派发的命令。
pub const TAB_NEW_TAB_ID: &str = "Terminal.NewSession";

/// 缩放三条命令的文案与 ID（`View ▸ Zoom` 菜单与中文翻译共用这一份）。
///
/// 2026-11-15：这三条**不再**挂在标签条右端的字母上——那个字母是 group 名字，
/// 不是字体按钮。它们由 `menu/tree.rs` 的 `View ▸ Zoom` 独立消费；
/// `ui/` 侧只有工具行与菜单树读得到，标签条不弹这个菜单。
pub const ZOOM_ITEMS: &[(&str, &str)] = &[
    ("Zoom In", "Text.ZoomIn"),
    ("Zoom Out", "Text.ZoomOut"),
    ("Reset Zoom", "Text.ResetZoom"),
];

/// 工具行地址栏右端 `>` 的 Goto 菜单。三条都在 `capability` 的已接线集合里，
/// 点下去真的会打开对应的 Goto 浮层，不是装饰箭头。
///
/// D3 后 `Window.GotoAddressBar` 指的就是地址栏本身（此前它点开的是一个
/// 「把一行当 shell 命令发出去」的工作台页，那是语义冲突的源头）。
/// 那条命令页现在叫 `Window.GotoCommandLine`，**没有**加进这张下拉：
/// 这一列是「跳到窗格 / 主机」的三个入口，而命令行页不是「跳」——
/// 加进来会让这一个箭头同时装着两种语义，又绕回 D3 要解的问题。
pub const GOTO_ITEMS: &[(&str, &str)] = &[
    ("Goto Line", "Window.GotoLine"),
    ("Goto Session", "Window.GotoSession"),
    ("Goto Address Bar", "Window.GotoAddressBar"),
];

/// 布局下拉的一项。
///
/// `id` 用来派发，`zone` 用来读勾选状态（投影层要知道「左带还亮着几块」）。
/// 两个字段都留着：从 `zone` 推不出 `id` 的命名，从 `id` 反推 `zone` 又要在
/// 投影层写一次字符串比较——那正是 I5 要挡的事。
pub struct DockItem {
    pub label: &'static str,
    pub id: &'static str,
    pub zone: crate::dock::DockZone,
}

pub const DOCK_ITEMS: &[DockItem] = &[
    DockItem {
        label: "Left Docks",
        id: "Window.ToggleLeftDocks",
        zone: crate::dock::DockZone::Left,
    },
    DockItem {
        label: "Right Docks",
        id: "Window.ToggleRightDocks",
        zone: crate::dock::DockZone::Right,
    },
    // 2026-11-17 用户点名补第三条：截图里这个下拉**只有左/右两项**，
    // 而底带（Sender / Shell / Script）是三条带里唯一没有菜单入口的。
    // `DockZone::ALL` 一直是三条，缺的只是这里的枚举项与下面三处映射。
    DockItem {
        label: "Bottom Docks",
        id: "Window.ToggleBottomDocks",
        zone: crate::dock::DockZone::Bottom,
    },
];

/// 菜单栏 `Tunnels` 下拉的一行。
///
/// 这两行原先在 `ui/shell/menu_tools.rs` 里写成元组字面量（`(标签, 访问字母下标,
/// 有没有开关, 命令 ID)`），被 4e 的 I5 判据抓了个正着：**`ui/` 里出现
/// `"Window.ToggleTunnels"` 这种命令 ID 就算硬编码**，哪怕它只是从别处抄了一遍。
/// 表挪到这里之后，投影层只认 `TUNNEL_ITEMS` 这个名单，ID 只活在数据层。
pub struct TunnelItem {
    /// 英文标签——与 `Tool` 菜单里那两条用**同一个** i18n key（中文由 `i18n/zh.rs` 给）。
    pub label: &'static str,
    /// 访问字母的字符下标：`T̲unnels` 划首字母，`Tunnel S̲ettings` 划 `S`。
    pub access: usize,
    /// `true` = 这一行右端带那个胶囊开关（画的是 `tunnels_enabled` 的当前值）。
    /// 视觉基线里只有第一行有开关，所以这一位不是「有没有值」而是**哪一行装开关**。
    pub switch: bool,
    /// 派发用。开关那一行点下去派 `Window.ToggleTunnels`，另一行派设置对话窗。
    pub id: &'static str,
}

pub const TUNNEL_ITEMS: &[TunnelItem] = &[
    TunnelItem {
        label: "Tunnels",
        access: 0,
        switch: true,
        id: "Window.ToggleTunnels",
    },
    TunnelItem {
        label: "Tunnel Settings",
        access: 7,
        switch: false,
        id: "Terminal.TunnelSettings",
    },
];
