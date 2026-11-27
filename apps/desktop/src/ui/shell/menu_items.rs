//! 遍历一棵菜单树：`Item` 分发到「画一行 / 分隔线 / 子菜单」。
//!
//! 2026-11-08 从 `menu_bar.rs` 拆出。这里只有**遍历**——一行长什么样
//! 全在 `menu_style.rs`，本文件不碰任何颜色常量。

use crate::ui::*;
use super::menu_style::*;

/// 一条 Action 的状态：能力台账（接没接）× 写入端状态（此刻能不能用）。
///
/// 2026-11-17 放开给 `terminal::context_rows`：右键菜单 `Log ▸` 展开的命令
/// 与菜单栏是同一批，判态必须走**同一个**谓词，否则同一台机器上两处一亮一灰。
pub(in crate::ui) fn action_state(win: &TerminalWindow, id: &str) -> (MenuRowState, &'static str) {
    let implemented = capability::is_implemented(id);
    let enabled = implemented && win.log_command_enabled(id);
    let why = if implemented {
        "Not available in the current log state"
    } else {
        "Not wired (placeholder)"
    };
    let state = if enabled {
        MenuRowState::Enabled
    } else {
        MenuRowState::Disabled
    };
    (state, why)
}

/// 这一级菜单每一行的下划线位置：英文取参考图词位，中文取显示文字里的等效位置，
/// 同级撞字符时由纯层往后挪。一次算一整级，逐行拿——不能单行算，否则看到的
/// 只是「这一行本来该划哪」，并不知道同级已经占走了哪些字母。
fn row_positions(win: &TerminalWindow, items: &[Item]) -> Vec<Option<usize>> {
    let keys: Vec<&'static str> = items
        .iter()
        .filter_map(|item| match item {
            Item::Action { label, .. }
            | Item::Check { label, .. }
            | Item::Submenu { label, .. } => Some(*label),
            Item::Separator => None,
        })
        .collect();
    let shown: Vec<&'static str> = keys.iter().map(|key| win.tr(key)).collect();
    term_app::menu::access_positions_unique(&shown, &keys)
}

/// 一条 `CheckState::Live` 的勾**此刻**该不该画 —— 走完整的强类型链路，不认字符串。
///
/// 判据链（两类，共用 `Action::parse(id)` 入口）：
/// - 可视化开关：`Action::Shell(ShellAction::Marks(knob))` →
///   [`TerminalWindow::marks_knob_on`] → 活动标签的 `Tab.marks`；
/// - hex 互斥档位：`ShellAction::HexBase|HexColumns|HexGroupChars(value)` →
///   [`TerminalWindow::active_hex`] → 活动标签的 `Tab.hex`。选中哪一档就在
///   哪一档打勾，点另一档勾跟着挪过去。
///
/// 所以「菜单里那个勾」与「渲染器读的那份参数」是**同一个字段**，
/// 中间没有第二份镜像状态可以漂移。
///
/// 为什么落在这里而不是 `menu.rs`：菜单树是纯数据（`term-app` 不认识窗口）；
/// 为什么不是 `draw_action_row` 内联：那函数收 `&mut TerminalWindow`，
/// 在参数位置再借一次会撞借用检查——先算成 `bool` 再进去，一行都不用绕。
///
/// 兜底返回 `false` 而不是 panic：`Live` 只允许出现在已接线命令上
/// （`menu_check_tests.rs::a_wired_command_must_read_live_state` 双向守着），
/// 万一将来有人插了一条没接线的 `Live` 行，屏幕上会是一个可点的空框，
/// 而不是整窗崩掉。
pub(in crate::ui) fn live_check(win: &TerminalWindow, id: &str) -> bool {
    use term_app::action::Action;
    use term_app::shell::ShellAction;
    match Action::parse(id) {
        Some(Action::Shell(ShellAction::Marks(knob))) => win.marks_knob_on(knob),
        // Remote Mode 勾的是「当前可输入」= 不在本地只读态。与 Local Mode
        // 共用同一份 `Tab.local_mode`，中间没有第二份镜像状态。
        Some(Action::Shell(ShellAction::SetRemoteMode)) => !win.local_mode_on(),
        // Word Wrap 勾的是活动标签的 Tab.word_wrap，与 toggle_word_wrap 共用同一位。
        Some(Action::Shell(ShellAction::ToggleWordWrap)) => win.word_wrap_on(),
        // View ▸ Bars 的三条勾选行读的是同一份 `ViewState` 开关（reducer 的
        // `ToggleMenuBar/StatusBar/QuickBar` 写它，绘制也读它）。原先这里没有
        // 分支，三条 `Live` 全落兜底画空框——「有开关、没反馈」的假勾。
        Some(Action::Shell(ShellAction::ToggleMenuBar)) => win.view.show_menu_bar,
        Some(Action::Shell(ShellAction::ToggleStatusBar)) => win.view.show_status_bar,
        Some(Action::Shell(ShellAction::ToggleQuickBar)) => win.view.show_quick_bar,
        // `Tool ▸ Script ▸ Record Script` 的勾与面板按钮读同一个 `Recorder` 槽位。
        Some(Action::Shell(ShellAction::RecordScript)) => win.is_recording(),
        Some(Action::Shell(ShellAction::HexBase(base))) => win.active_hex().base == base,
        Some(Action::Shell(ShellAction::HexColumns(columns))) => {
            win.active_hex().columns == columns
        }
        Some(Action::Shell(ShellAction::HexGroupChars(group))) => win.active_hex().group == group,
        // 其余（`None` = 未接线 / 别的能力）画空框。这里刻意不穷举 `Action`：
        // 它二十几个变体在 `parse::feature()` 里已经被穷举过一次（那里加变体
        // 会直接编译失败），在这一处再抄一遍只会得到一张同长的噪音表。
        // 真正的护栏是 `menu_check_tests.rs::a_wired_command_must_read_live_state`：
        // `Live` 必须已接线，所以这条兜底在生产里取不到。
        _ => false,
    }
}

/// 一个勾选框的填充值：写死的常量，或按当前会话现读。
///
/// 类型写全路径（`term_app::menu::CheckState`）而不是靠 `use crate::ui::*`
/// 转手：`CheckState` 是这一刀新加的纯数据概念，只有本文件一个消费者，
/// 为它去动 `ui/mod.rs` 的公共导入列表不划算——那张列表是给「到处都在用」
/// 的东西准备的（`Item` / `Action` / `ViewState`）。
fn check_cell(
    win: &TerminalWindow,
    id: &str,
    state: term_app::menu::CheckState,
) -> CheckCell {
    use term_app::menu::CheckState;
    match state {
        CheckState::Fixed(on) => CheckCell::Box(on),
        CheckState::Live => CheckCell::Box(live_check(win, id)),
    }
}

/// 生产路径的一条命令行：查三态、画、点了派发。
///
/// `Action` 与 `Check` 的唯一区别是左边那个框，其余（置灰口径、快捷键、
/// 点击派发）完全一样，所以合成一处——两处各写一遍迟早只改一边。
fn draw_action_row(
    win: &mut TerminalWindow,
    ui: &mut egui::Ui,
    label: &'static str,
    id: &str,
    check: CheckCell,
    look: RowLook,
    underline: Option<usize>,
) {
    let (state, why) = action_state(win, id);
    let text = win.tr(label);
    let response = draw_menu_row(
        ui,
        text,
        underline,
        &win.shortcuts.display(id),
        RowLook {
            state,
            check,
            ..look
        },
        false,
    )
    .on_disabled_hover_text(win.tr(why));
    if response.clicked() {
        win.dispatch(id);
        ui.close_menu();
    }
}

/// 这棵子菜单里有没有**任何**能派发的命令（递归）。
///
/// 用来分辨「结构占位」与「真子菜单」：`items: vec![Item::Separator]` 与
/// `items: vec![]` 都属于前者。判据是「有没有 `Action` / `Check`」，而不是
/// 「`items` 是不是空」——将来若有人往里补了分隔线和一个未接线命令，仍然
/// 要按有命令处理（那条命令自己会走 `capability::is_implemented` 置灰）。
fn has_command(items: &[Item]) -> bool {
    items.iter().any(|item| match item {
        Item::Action { .. } | Item::Check { .. } => true,
        Item::Submenu { items, .. } => has_command(items),
        Item::Separator => false,
    })
}

/// 绘制菜单项列表。子菜单递归展开，行为与上游 WindTerm 的 `Menu::popup` 一致。
/// 点击先入队再关闭菜单，避免在弹出层里改状态。
pub(in crate::ui) fn draw_menu_items(win: &mut TerminalWindow, ui: &mut egui::Ui, items: &[Item]) {
    apply_popup_style(ui);
    // 宽度**在这里钉死**，生产路径与夹具共用同一句：`ui.menu_button` 的弹层是
    // 一个 `Area`，egui 用 `spacing.menu_width`（默认 400）当它的 `default_width`，
    // 不设宽每行都会摊满这 400 点（＝一堵右侧全空的灰墙）。夹具此前各自
    // `set_width` 掩盖了这个洞。
    ui.set_width(menu_rows_width(win, items, ui));
    let look = RowLook {
        // 每行的三态与勾选框由下面逐行填，这里只给「整菜单级」的两项。
        state: MenuRowState::Enabled,
        check: CheckCell::None,
        indent: reserve_check_column(items),
        cells: widest_cells(win, items),
    };
    let mut positions = row_positions(win, items).into_iter();
    for item in items {
        match item {
            Item::Action { label, id } => {
                draw_action_row(win, ui, label, id, CheckCell::None, look, positions.next().flatten())
            }
            Item::Check { label, id, checked } => {
                // 先算勾再进 `&mut` 那一侧：`live_check` 只借 `&TerminalWindow`，
                // 在实参位置调用会与外层的 `&mut` 撞车（E0502）。
                let cell = check_cell(win, id, *checked);
                draw_action_row(win, ui, label, id, cell, look, positions.next().flatten())
            }
            Item::Separator => menu_separator(ui),
            // 一条命令都没有的子菜单不画成能点开的样子。
            //
            // 这些父级（`Select Column To` / `Word` / `Page` / … / `Hex Base`）
            // 在视觉基线里是有的，删掉会让菜单结构与视觉基线对不上，所以**留着**；
            // 但点开只看见一条横线，是比缺一项更坏的体验——画成灰行，悬停
            // 说明原因，与未接线命令行（`Not wired (placeholder)`）同一口径。
            Item::Submenu { label, items } if !has_command(items) => {
                let underline = positions.next().flatten();
                draw_menu_row(
                    ui,
                    win.tr(label),
                    underline,
                    "",
                    RowLook {
                        state: MenuRowState::Disabled,
                        ..look
                    },
                    true,
                )
                .on_disabled_hover_text(win.tr("Not wired (placeholder)"));
            }
            Item::Submenu { label, items } => {
                // 真子菜单的标题也要有访问字母下划线。夹具一侧早在
                // `draw_menu_items_static` 里经 `draw_menu_row` 画了，生产此前把
                // `_underline` 丢掉，正是「夹具画、生产不画」的分叉。这里把同一份
                // 位置烤进 LayoutJob 的 section format，再交给 `ui.menu_button`；
                // 夹具分支一个字节没动，组件基线因此不受影响。
                let underline = positions.next().flatten();
                let title = win.tr(label);
                let font = egui::TextStyle::Button.resolve(ui.style());
                // 颜色留 PLACEHOLDER：egui 0.29.1 `menu.rs::SubMenuButton::show`
                // 用 `painter.galley(pos, galley, visuals.text_color())` 画标题，
                // PLACEHOLDER 的 glyph 与下划线都由 tessellator 填成同一个当前色，
                // hover / open 三态才不会一条亮一条灰。
                let title_job = text_job(&title, underline, egui::Color32::PLACEHOLDER, &font);
                // 2026-11-15 用户反馈「二级菜单要停一下还得点、展开位置也不在
                // 右侧」的根因：上一版用 `egui::menu::menu_custom_button`。那是
                // **顶级菜单** API（源码注释写着 "Responds to primary clicks"），
                // 而且在弹层里另起一份 `BarState`，与父菜单的开合状态互不认识。
                //
                // `Ui::menu_button` 在已有菜单上下文里会分派到
                // `menu::submenu_button`（"Opens on hover"），由
                // `MenuState::submenu_button_interaction` 在**父菜单右缘**
                // 打开（pos.x = 父菜单 rect.right() + menu_spacing），
                // 收起 / 点外面关闭 / 递归子菜单全由同一份 MenuState 管。
                //
                // 外观对齐：egui 的 `set_menu_style` 每帧把 `button_padding`
                // 压成 (2, 0)，而 `draw_menu_row` 用钉死的 `MENU_ROW_PAD_*`；
                // 调用前后把它对齐，父行才与其它行等高、文字同一条竖线。
                // `menu_popup` 用 `top_down_justified`，`SubMenuButton` 的矩形
                // 会被拉到整行宽，它自己画的展开箭头自然落在右缘——不再需要
                // 我们按标题宽度补一个 `▸`（那正是上一版箭头压住文字的由来）。
                // egui 0.29.1 源码核对（换成 LayoutJob 不会动交互与几何）：
                // · `Ui::menu_button` 在菜单上下文里转 `menu::submenu_button`
                //   → `SubMenu::show` → `SubMenuButton::show`，sense 固定 `click()`；
                // · `MenuState::submenu_button_interaction` 只看 `button.hovered()` /
                //   `button.rect`，决定 hover 展开与父菜单右缘定位，不读 galley；
                // · 展开箭头是另一枚独立 icon galley（`"⏵"`），仍贴按钮右缘；
                // · `WidgetText::into_galley` 对 `LayoutJob` 只改写 `job.wrap`，
                //   而弹层已 `ui.set_width(menu_rows_width(..))`，标题宽度必然小于
                //   `text_available_width`，不换行、`Galley::size()` 不变。
                let saved = ui.spacing().button_padding;
                ui.spacing_mut().button_padding = egui::vec2(MENU_ROW_PAD_X, MENU_ROW_PAD_Y);
                let _ = ui.menu_button(egui::WidgetText::from(title_job), |ui| {
                    draw_menu_items(win, ui, items);
                });
                ui.spacing_mut().button_padding = saved;
            }
        }
    }
}

/// 组件夹具的一条命令行：只画不派发，`highlighted` 命中时摆成选中态。
fn draw_static_row(
    win: &TerminalWindow,
    ui: &mut egui::Ui,
    label: &'static str,
    id: &str,
    check: CheckCell,
    look: RowLook,
    highlighted: &[&str],
    underline: Option<usize>,
) -> egui::Response {
    let (real, _) = action_state(win, id);
    let selected = ui_was_selected(label, id, highlighted);
    let state = if selected {
        MenuRowState::Selected
    } else {
        real
    };
    let text = win.tr(label);
    draw_menu_row(
        ui,
        text,
        underline,
        &win.shortcuts.display(id),
        RowLook {
            state,
            check,
            ..look
        },
        false,
    )
}

/// 组件夹具专用：不弹层、不派发，把 `highlighted` 里的标签 / 命令 ID 摆成
/// 选中态（视觉基线里 `Log` 行与 `Settings...` 行同时是蓝块）。三态颜色与
/// 生产共用 [`draw_menu_row`]，避免「夹具好看、真菜单不对」。
///
/// 返回第一个选中行的**屏幕矩形**：子菜单要对齐父菜单里 `Log` 那一行，
/// 靠数行数乘行高去估位置会在改行距时错位（首版实测差 32pt），直接量最稳。
pub(in crate::ui) fn draw_menu_items_static(
    win: &TerminalWindow,
    ui: &mut egui::Ui,
    items: &[Item],
    highlighted: &[&str],
) -> Option<egui::Rect> {
    apply_popup_style(ui);
    // 与生产同一个行宽（夹具的 `Area` 已经按 `menu_popup_width` 设过总宽，
    // 这里设的是 `Frame` 之内的行宽，两者正好差两侧 4）。
    ui.set_width(menu_rows_width(win, items, ui));
    let look = RowLook {
        state: MenuRowState::Enabled,
        check: CheckCell::None,
        indent: reserve_check_column(items),
        cells: widest_cells(win, items),
    };
    let mut positions = row_positions(win, items).into_iter();
    let mut picked = None;
    for item in items {
        match item {
            Item::Action { label, id } => {
                let response = draw_static_row(
                    win,
                    ui,
                    label,
                    id,
                    CheckCell::None,
                    look,
                    highlighted,
                    positions.next().flatten(),
                );
                if ui_was_selected(label, id, highlighted) && picked.is_none() {
                    picked = Some(response.rect);
                }
            }
            Item::Check { label, id, checked } => {
                let cell = check_cell(win, id, *checked);
                let response = draw_static_row(
                    win,
                    ui,
                    label,
                    id,
                    cell,
                    look,
                    highlighted,
                    positions.next().flatten(),
                );
                if ui_was_selected(label, id, highlighted) && picked.is_none() {
                    picked = Some(response.rect);
                }
            }
            Item::Separator => menu_separator(ui),
            Item::Submenu { label, .. } => {
                let text = win.tr(label);
                let selected = highlighted.contains(label);
                let state = if selected {
                    MenuRowState::Selected
                } else {
                    MenuRowState::Enabled
                };
                // 子菜单箭头经 `submenu=true` 贴右边缘（视觉基线实测贴右），
                // 不再把 ▸ 当快捷键挤在中间列。
                let response = draw_menu_row(
                    ui,
                    text,
                    positions.next().flatten(),
                    "",
                    RowLook { state, ..look },
                    true,
                );
                if selected && picked.is_none() {
                    picked = Some(response.rect);
                }
            }
        }
    }
    picked
}

/// `highlighted` 是否点名了这一行（标签或命令 ID 任一命中）。
fn ui_was_selected(label: &str, id: &str, highlighted: &[&str]) -> bool {
    highlighted.contains(&label) || highlighted.contains(&id)
}

/// 2026-11-15 二级菜单的防回归：生产路径只能用 `Ui::menu_button`。
///
/// `egui::menu::menu_custom_button` 是**顶级菜单**（只响应点击、弹层在按钮
/// 下方），用它当二级菜单就是「要点一下 + 位置不对」两个毛病的直接来源。
/// 组件夹具走 `draw_menu_items_static`（手绘行），不会碰这条路径。
#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Pos2, PointerButton, Rect, Shape};

    /// 把一帧里所有文字连同位置取出来（弹层的文字是手绘 `Shape::Text`）。
    fn texts(out: &egui::FullOutput) -> Vec<(String, Pos2)> {
        texts_with_galleys(out)
            .into_iter()
            .map(|(text, pos, _)| (text, pos))
            .collect()
    }

    /// 同上，但保留 galley：形状级测试要看的是**实际画出来的** section
    /// format，不是源码里有没有写 `underline` 这个词。
    fn texts_with_galleys(out: &egui::FullOutput) -> Vec<(String, Pos2, std::sync::Arc<egui::text::Galley>)> {
        fn walk(shape: &Shape, out: &mut Vec<(String, Pos2, std::sync::Arc<egui::text::Galley>)>) {
            match shape {
                Shape::Text(t) => out.push((t.galley.text().to_string(), t.pos, t.galley.clone())),
                Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        for clipped in &out.shapes {
            walk(&clipped.shape, &mut found);
        }
        found
    }

    /// 一帧：把一棵真实菜单树挂在 `menu_custom_button` 的弹层里。顶级那颗按钮
    /// 只是用来造出 egui 的 `MenuState`，被测的是它里面的 `draw_menu_items`。
    fn frame(
        ctx: &egui::Context,
        win: &mut TerminalWindow,
        items: &[Item],
        screen: Rect,
        events: Vec<Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |ctx| {
                // 用真实的 `TopBottomPanel` + `menu::bar` 当宿主：Area 在没有
                // 背景图层的裸 `ctx.run` 里 hit-test 不可靠（按钮永远拿不到
                // hovered/clicked），而生产菜单就挂在菜单栏里。
                egui::TopBottomPanel::top("submenu_hover_probe_bar")
                    .frame(egui::Frame::none())
                    .show(ctx, |ui| {
                        egui::menu::bar(ui, |ui| {
                            let _ = egui::menu::menu_custom_button(
                                ui,
                                egui::Button::new("Root"),
                                |ui| draw_menu_items(win, ui, items),
                            );
                        });
                    });
            },
        )
    }

    /// 两条形状测试共用的铺垫：造窗口、点开顶级菜单、稳定两帧。
    /// 每帧都要先 `PointerMoved` 让按钮 hovered，按下与松开要分两帧才 `clicked()`；
    /// 弹层首帧是 sizing pass（形状 Noop），所以要再跑两帧才拿到能看的形状。
    fn opened_menu(ctx: &egui::Context) -> (TerminalWindow, Vec<term_app::menu::Item>, Rect, egui::FullOutput) {
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let mut win = crate::ui::lifecycle_test_support::window_with(pool, ctx, Vec::new());
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let items = term_app::menu::menus().remove(0).items; // Session：含 `Log` 子菜单
        frame(ctx, &mut win, &items, screen, vec![Event::PointerMoved(Pos2::new(400.0, 400.0))]);
        for pressed in [true, false] {
            frame(ctx, &mut win, &items, screen, vec![
                Event::PointerMoved(Pos2::new(15.0, 9.0)),
                Event::PointerButton {
                    pos: Pos2::new(15.0, 9.0), button: PointerButton::Primary,
                    pressed, modifiers: Default::default(),
                },
            ]);
        }
        let mut out = egui::FullOutput::default();
        for _ in 0..2 {
            out = frame(ctx, &mut win, &items, screen, vec![Event::PointerMoved(Pos2::new(400.0, 400.0))]);
        }
        (win, items, screen, out)
    }

    /// 2026-11-15 用户反馈「鼠标停留不自动展开、还需要点一下，展开位置也不在
    /// 右侧」。这条不点第二下，只把指针移到 `Log` 行上，然后要求子菜单里的
    /// `Start` 出现在**父菜单右边、与 Log 行同高**。旧实现用顶级菜单 API，
    /// 这一步只会得到 `None`（要点一下才开）。
    #[test]
    fn hovering_a_nested_menu_opens_it_to_the_right_without_a_click() {
        let ctx = egui::Context::default();
        let (mut win, items, screen, before) = opened_menu(&ctx);
        let log = texts(&before)
            .into_iter()
            .find(|(s, _)| s == "Log")
            .map(|(_, p)| p)
            .expect("父菜单里应画出了 Log 行");
        assert!(
            !texts(&before).iter().any(|(s, _)| s == "Start"),
            "还没悬停，Log 子菜单就不该开着"
        );

        // 只移动指针，不产生任何按键事件。
        let hovered = frame(
            &ctx,
            &mut win,
            &items,
            screen,
            vec![Event::PointerMoved(Pos2::new(log.x + 4.0, log.y + 6.0))],
        );
        // 子菜单自己的 popup 首帧也可能还是 sizing pass，再跑一帧稳定。
        let settled = frame(&ctx, &mut win, &items, screen, Vec::new());
        let start = texts(&hovered)
            .into_iter()
            .chain(texts(&settled))
            .find(|(s, _)| s == "Start")
            .map(|(_, p)| p);
        let Some(start) = start else {
            panic!("悬停 Log 行后，子菜单的 Start 必须出现（不需要点击）");
        };
        assert!(
            start.x > log.x + 100.0,
            "子菜单要在父菜单右侧展开：Log.x={:.1} Start.x={:.1}",
            log.x,
            start.x
        );
        assert!(
            (start.y - log.y).abs() < 24.0,
            "子菜单首行要与父行 Log 对齐：Log.y={:.1} Start.y={:.1}",
            log.y,
            start.y
        );
    }

    /// 2026-11-26「夹具画、生产不画」的防回归。夹具一侧走 `draw_menu_row`，
    /// 早就有下划线；这里从**生产路径实际画出的形状**里捞出标题 galley，
    /// 逐节断言 `format.underline`，而不是只在源码里搜关键词。
    /// 同一条还钉住 egui 0.29.1 的两条几何事实：箭头仍是独立的第二枚 galley，
    /// 下划线的加入不改变 `Galley::size()`（不再包一层、不换行）。
    #[test]
    fn a_real_submenu_title_galley_underlines_its_access_letter() {
        let ctx = egui::Context::default();
        let (_, _, _, out) = opened_menu(&ctx);
        let painted = texts_with_galleys(&out);
        let title = painted.iter().find(|(text, _, _)| text == "Log")
            .map(|(_, _, galley)| galley.clone()).expect("生产菜单里应画出 Log 标题");
        let underlined: Vec<&str> = title.job.sections.iter()
            .filter(|s| s.format.underline != egui::Stroke::NONE)
            .map(|s| &title.text()[s.byte_range.clone()]).collect();
        assert_eq!(underlined, vec!["L"], "只有访问字母那节该带下划线");
        // 箭头是独立的 icon galley；换标题载体不该把它吃掉。
        assert!(painted.iter().any(|(text, _, _)| text == "⏵"), "箭头仍贴按钮右缘");
        // galley 尺寸与参考图纯文本一模一样：下划线只加 mesh，不改排版。
        let font = egui::TextStyle::Button.resolve(&ctx.style());
        let plain = ctx.fonts(|f| f.layout_job(egui::text::LayoutJob::simple(
            "Log".into(), font, egui::Color32::PLACEHOLDER, f32::INFINITY)).size());
        assert_eq!(title.size(), plain, "下划线不许改变标题 galley 尺寸");
    }

    #[test]
    fn nested_menus_use_the_hover_aware_ui_api() {
        let src = include_str!("menu_items.rs");
        // 只看生产段：本文件下方的测试说明里也会提到那个 API 的名字。
        let production = src.split("#[cfg(test)]").next().unwrap_or(src);
        assert!(
            production.contains("ui.menu_button(egui::WidgetText::from(title_job), |ui| {"),
            "二级菜单必须走 Ui::menu_button（hover 展开 + 右侧定位），标题带下划线"
        );
        // 注释里会提到这个 API 的名字，所以只钉「真的调用了它」。
        assert!(
            !production.contains("menu_custom_button("),
            "顶级菜单 API 不许回到菜单树里"
        );
    }

    /// 视觉基线 `view/02`、`view/03`、`view/05`：hex 三张子菜单是互斥单选组。
    /// 这条同时守住用户报的两个症状——「默认没有勾选」与「点了没有勾选行为」：
    /// 出厂档位必须正好一个勾；点另一档后勾必须跟着挪过去，且只改活动标签。
    #[test]
    fn hex_option_rows_are_checked_on_the_current_value_and_follow_clicks() {
        use crate::ui::lifecycle_test_support::{fake_tab, window_with, CloseCount};
        use crate::ui::terminal::HexParams;

        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let closes = CloseCount::default();
        let first = fake_tab(&pool, &ctx, 0, &closes);
        let second = fake_tab(&pool, &ctx, 1, &closes);
        let mut win = window_with(pool, &ctx, vec![first, second]);
        // 不依赖用户目录里的 prefs：直接把活动标签摆到出厂档位。
        win.tabs[0].hex = HexParams::default();
        let other_before = win.tabs[1].hex;

        // (出厂该打勾的 ID, 这一组全部 ID)。三组同构，循环里逐档点一遍。
        let groups: [(&str, &[&str]); 3] = [
            (
                "View.HexBase16",
                &[
                    "View.HexBase2",
                    "View.HexBase7",
                    "View.HexBase8",
                    "View.HexBase10",
                    "View.HexBase16",
                ],
            ),
            (
                "View.HexColumns16",
                &[
                    "View.HexColumns8",
                    "View.HexColumns16",
                    "View.HexColumns24",
                    "View.HexColumns32",
                ],
            ),
            (
                "View.HexGroupChars1",
                &[
                    "View.HexGroupChars1",
                    "View.HexGroupChars2",
                    "View.HexGroupChars4",
                    "View.HexGroupChars8",
                ],
            ),
        ];

        for (default_id, rows) in groups {
            let checked: Vec<&str> = rows
                .iter()
                .copied()
                .filter(|id| live_check(&win, id))
                .collect();
            assert_eq!(
                checked,
                vec![default_id],
                "{default_id}：出厂态应只有这一档打勾"
            );

            for id in rows {
                win.dispatch(id);
                let checked: Vec<&str> = rows
                    .iter()
                    .copied()
                    .filter(|probe| live_check(&win, probe))
                    .collect();
                assert_eq!(
                    checked,
                    vec![*id],
                    "点 {id} 之后应只有它自己打勾（勾读活动标签的 Tab.hex）"
                );
            }

            // 收尾回到出厂档，下一组从干净状态开始。
            win.dispatch(default_id);
        }

        assert_eq!(
            win.tabs[1].hex, other_before,
            "View ▸ Hex * 只改活动标签，另一个标签的排版一个字都不许动"
        );
    }

    /// 没有标签时菜单仍点得开：勾画在出厂档上，不 panic（与 `active_marks` 同口径）。
    #[test]
    fn hex_option_rows_without_a_tab_show_the_factory_default() {
        use crate::ui::lifecycle_test_support::window_with;

        let ctx = egui::Context::default();
        let pool = std::sync::Arc::new(term_session::SessionPool::new());
        let win = window_with(pool, &ctx, Vec::new());
        for id in ["View.HexBase16", "View.HexColumns16", "View.HexGroupChars1"] {
            assert!(live_check(&win, id), "{id}：没有标签时勾应落在出厂档");
        }
        for id in ["View.HexBase2", "View.HexColumns8", "View.HexGroupChars8"] {
            assert!(!live_check(&win, id), "{id}：没有标签时不该打勾");
        }
    }
}
