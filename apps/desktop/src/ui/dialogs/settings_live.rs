//! Preferences 里**真接线**的几页：Application / Appearance /
//! Command History / Filer（`Search` 同样真接线，但为了行数上限单开
//! `settings_live_search.rs`；`Font` / `Tabs` / `Caret` 同理，分别在
//! `settings_live_font.rs` / `settings_live_tabs.rs` / `settings_live_caret.rs`）。
//!
//! 其余页是纯排版（控件没有字段可绑），全部走 `settings_pages` 的数据表；
//! 这几页不行——它们的控件会真的改东西：
//!
//! - **Application ▸ Startup** ：三档单选 + Session 1–4 下拉，
//!   绑 `TerminalWindow::startup`，改完写 prefs；启动时由 `restore_layout`
//!   决定摆回哪些标签，被激活时才连（见 `term_app::startup` 模块文档）。
//! - **Application ▸ Close And Exit**：两个勾选就是关闭确认弹窗里「不再提示」
//!   的同两个位（`confirm.suppress_session` / `suppress_window`），改完写 prefs。
//! - **Appearance ▸ Language**：`SetLanguage` 走 reducer，界面当场换语言。
//! - **Appearance ▸ Window opacity** ：开关 + 滑杆是真控件，绑
//!   `Prefs.window_transparent` / `Prefs.window_opacity`，改完当场改窗口上的
//!   运行期副本并 `save_prefs` 落盘；`main.rs::update` 每帧按钳制后的 alpha
//!   把 `egui::Visuals` 背景与终端区底色画成半透明。
//! - **Command History**：容量 / TTL / 落盘开关三个字段都有读者（内存表与
//!   `on_exit`），改完当场 `reconfigure` 并写回 `prefs.json`；`Clear` 真的清空
//!   当前历史表。
//! - **Filer ▸ Show hidden items**：勾选绑 `win.filer.show_hidden`，列本地目录
//!   时按它过滤（`local_dir::list`），改完当场重列并落 prefs。
//!
//! 从 `settings.rs` 拆出来是因为 （`ui/` 下非 `mod.rs` 代码行 ≤ 500），
//! 顺便把「哪几页是真的」这件事集中到一个文件里，一眼看得完。

use crate::ui::kit::spec::BLOCK_GAP;
use crate::ui::kit::field::{choice_row_resp, ChoiceKind, UnderlineStyle};
use super::settings_pages_view::{
    field_height, field_label, group_box, label_col, set_row_rhythm, spin_field_delta,
};
use crate::ui::tr::t;
use crate::ui::*;
use term_app::i18n::Lang;

/// 视觉基线的强调色与控件宽（`settings.rs` 也用这几个，改一处就够）。
const STAR_RED: u32 = 0xE0554D;
const COMBO_W: f32 = 238.0;

/// 标签列宽度：视觉基线里「冒号」全部落在同一条竖线上（实测 ≈140）。
const LABEL_W: f32 = 110.0;

/// 一次 load-modify-save：读盘 → 让 `edit` 只改自己那几个字段 → 写回。
///
/// 抽出来是因为真心接线的页越来越多，而「整份重建 `Prefs`」会把语言 / 启动
/// 策略 / 光标样式 / 搜索模板一起清掉。每页各写一遍「先 load 再 save」，
/// 迟早有一页漏掉 load——那正好是最难查的一类 bug（用户设置好的东西自己没了）。
///
/// # 「文件读不出来」时这一串动作不会丢掉用户那份
///
/// 这条路里 `load ` 的 `None` 被 `unwrap_or_default ` 收成了出厂值，看起来
/// 下一步就要把磁盘上的原文件覆盖掉。**兜底在 `prefs::save` 里，不在这里**：
/// 它在覆盖前会先看一眼旧文件读不读得出来，读不出来就逐字节另存成
/// `prefs.json.bad-<时间戳>` 再写（连备份都做不到时这次就不写）。
///
/// 为什么不在这一处修：写 `prefs.json` 的入口有十几处（本文件的五页、
/// `settings.rs`、确认弹窗、日志设置页、搜索页…），每一处都写了同样的
/// `load .unwrap_or_default `；只在其中一处加保护，等于把同一个缺陷留在了
/// 其它十几条路上。所以保护放在唯一的写口 `prefs::save`，这一层保持
/// 「我只管改自己那几个字段」。
pub(in crate::ui) fn save_prefs(edit: impl FnOnce(&mut term_runtime::prefs::Prefs)) {
    let mut prefs = term_runtime::prefs::load().unwrap_or_default();
    edit(&mut prefs);
    term_runtime::prefs::save(&prefs);
}

/// 「读不出来的 prefs.json 已经另存了一份」的提示条。
///
/// 数据还在盘上但用户不知道，在体验上和丢了没区别——他不会去翻 profile 目录。
/// 所以 `prefs::save` 保全成功时会留一条记录，这里把它画出来：**原文件没丢、
/// 存到哪儿了**，外加一个「看过就收起来」的按钮（`prefs::clear_recovery`）。
///
/// 画在本文件的这几页上，而不是整个对话框的框架里：框架在 `settings.rs`，
/// 而备份这件事发生在任何一页（写 prefs 的入口散在各页）。本文件这几页正是
/// 真接线的那几页——用户丢了设置之后最可能回去重新点的地方。
pub(in crate::ui) fn recovery_notice(ui: &mut egui::Ui) {
    let Some(backup) = term_runtime::prefs::last_recovery() else {
        return;
    };
    ui.add_space(BLOCK_GAP);
    group_box(ui, None, |ui| {
        ui.label(
            egui::RichText::new("prefs.json could not be read, so the original was kept as:")
                .color(col(STAR_RED)),
        );
        ui.label(
            egui::RichText::new(backup.display().to_string())
                .weak()
                .small(),
        );
        if ui
            .add(egui::Button::new("Dismiss").min_size(egui::vec2(64.0, 20.0)))
            .clicked()
        {
            term_runtime::prefs::clear_recovery();
        }
    });
}

/// 一行的「右对齐标签 + 控件」。`star` = 视觉基线的红色星号（要重启才生效）。
fn label_row(
    ui: &mut egui::Ui,
    text: &str,
    acc_word: &str,
    star: bool,
    add: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(LABEL_W, 20.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.label(egui::WidgetText::from(crate::ui::kit::field::underlined_job(ui, text, acc_word, crate::ui::kit::field::UnderlineStyle::Dialog)));
                if star {
                    ui.label(egui::RichText::new("* ").color(col(STAR_RED)));
                }
            },
        );
        add(ui);
    });
}

/// 控件宽度：占满标签列右边的余量，但不许超过视觉基线的 ≈238。
/// Application 页的 Session 1..4 标签按兼容终端**左对齐**（`01-application.png`）：
/// 「Session n:」起点与单选按钮文字对齐，下拉框从标签列右缘开始占满剩余宽度。
/// 与本文件其它页的右对齐标签不同，不能混用。
fn left_label(ui: &mut egui::Ui, text: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(104.0, 20.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add(egui::Label::new(text).truncate());
        },
    );
}

fn field_w(ui: &egui::Ui) -> f32 {
    ui.available_width()
}

use crate::ui::kit::spec::col;

/// `Appearance` 页一次绘制里被用户改过的东西（每一项 `None` = 没动）。
///
/// 拆成结构体而不是把 `&mut TerminalWindow` 传进来，是为了让这一页保持
/// 「只画控件、只回报意图」：落盘与运行期副本的写入只有
/// `TerminalWindow::set_window_opacity` 一个入口，菜单与设置页共用它。
pub(in crate::ui) struct AppearanceEdit {
    pub lang: Option<Lang>,
    pub window_opacity: Option<WindowOpacity>,
}

/// `Appearance` 页。
///
/// - Language：选了立刻生效（reducer + 落盘都由调用方接手）；
/// - Theme：只有 `dige-black` 一项，是真数据源但没有第二种选择；
/// - Window opacity ：**真控件**——开关管 OS 窗口能不能透，滑杆管
///   `[0.3, 1.0]` 的数值，改完调用方一并写回 `Prefs` 与运行期副本。
///   滑杆两端直接用 `prefs::WINDOW_OPACITY_MIN/MAX`，与渲染侧钳制同一份口径。
pub(in crate::ui) fn appearance(
    ui: &mut egui::Ui,
    lang: Lang,
    window: WindowOpacity,
) -> AppearanceEdit {
    let mut picked = None;
    let mut opacity_edit = None;
    let mut current = window;
    recovery_notice(ui);
    ui.add_space(BLOCK_GAP);
    // 视觉基线 `02-appearance.png`：`Application` 是**组标题**（画在框外上方），
    // 不是页级灰标题——框里直接就是 Language / Theme / Window opacity 三行。
    group_box(ui, Some(t(lang, "Application")), |ui| {
        set_row_rhythm(ui);
        // 标签列固定 140 并右对齐到同一条竖线（视觉基线里三行的冒号在同一直线上）；
        // 星号画在文字**左侧**，所以按 right_to_left 先放文字再放星。
        label_row(ui, t(lang, "Language"), "Language", true, |ui| {
            egui::ComboBox::from_id_salt("prefs_language")
                .selected_text(lang.native_name())
                .width(field_w(ui))
                .wrap_mode(egui::TextWrapMode::Truncate)
                .show_ui(ui, |ui| {
                    for l in Lang::ALL {
                        if ui.selectable_label(l == lang, l.native_name()).clicked() {
                            picked = Some(l);
                        }
                    }
                });
        });
        label_row(ui, t(lang, "Theme"), "Theme", false, |ui| {
            egui::ComboBox::from_id_salt("prefs_theme")
                .selected_text("dige-black")
                .width(field_w(ui))
                .wrap_mode(egui::TextWrapMode::Truncate)
                .show_ui(ui, |ui| {
                    // theme_load 只有 dige-black 一项：列表如实只有它；
                    // 选中即当前主题，不需要任何后续动作。
                    let _ = ui.selectable_label(true, "dige-black");
                });
        });
        //  Window opacity：真控件。视觉基线 `02-appearance.png` 这一行是
        // 「标签 + 通栏数值框 `1.00`」，与上面 Language / Theme 两行同宽同位。
        //
        // 原先把透明开关压成 18×18 的**空标签复选框**塞在数值框右边：数值框因此
        // 窄 22 点、右缘缩进，而那个没有文字的方块孤零零浮在框右侧——用户
        // 报的「右侧有个孤立方块」就是它。现在数值框按视觉基线占满整行（`spin_field`
        // 补上 ▲▼），透明开关单独占一行，与 `Close And Exit` 组里的复选框同一种样式。
        label_row(ui, t(lang, "Window opacity"), "Window", false, |ui| {
            let min = term_runtime::prefs::WINDOW_OPACITY_MIN;
            let max = term_runtime::prefs::WINDOW_OPACITY_MAX;
            let mut value = current.opacity.clamp(min, max);
            let (_, stepped) = spin_field_delta(ui, |ui| {
                let drag = egui::DragValue::new(&mut value)
                    .range(min..=max)
                    .speed(0.01)
                    .fixed_decimals(2);
                if ui.add(drag).changed() {
                    current.opacity = value;
                    opacity_edit = Some(current);
                }
            });
            if let Some(d) = stepped {
                value = (value + d as f32 * 0.01).clamp(min, max);
                current.opacity = value;
                opacity_edit = Some(current);
            }
        });
        let mut transparent = current.transparent;
        if choice_row_resp(ui, ChoiceKind::Checkbox, t(lang, "Transparent window"), "", &mut transparent, true, UnderlineStyle::None)
            .on_hover_text("Let the OS window background show through")
            .clicked()
        {
            current.transparent = transparent;
            opacity_edit = Some(current);
        }
    });
    AppearanceEdit {
        lang: picked,
        window_opacity: opacity_edit,
    }
}

/// `Widgets ▸ Filer`：视觉基线 `dialog_settings_filer.png` 的 `Features` 分组框。
///
/// 唯一那个勾选绑 `win.filer.show_hidden`——列本地目录时的过滤开关
/// （`local_dir::list` 的第二个参数）。改完同一帧做三件事：写窗口字段、落
/// `prefs.json`、**立即重列**当前本地目录。只写不重列的话，用户勾完要等下一次
/// 刷新或重开面板才看得到效果——那正是「控件像假的」的来源。
///
/// 过滤只作用于**本地**目录：SFTP 列表由服务端给，远端怎么算隐藏项我们不知道，
/// 所以不对远端条目做本地猜测（页底写着这句）。
pub(in crate::ui) fn filer(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let mut on = win.filer.show_hidden;
    recovery_notice(ui);
    ui.add_space(BLOCK_GAP);
    // 视觉基线 `18-widgets-filer.png`：内容区第一个字就是组标题 `Features`，
    // 没有页级灰标题（老实现多画一行 `Filer`，整页因此整体下移 26 点）。
    group_box(ui, Some("Features"), |ui| {
        if choice_row_resp(ui, ChoiceKind::Checkbox, "Show hidden items", "", &mut on, true, UnderlineStyle::None).clicked() {
            win.filer.show_hidden = on;
            // 先重列再落盘：列表与勾选态同一帧对上，写盘慢一点也看不出来。
            win.reload_local_dir();
            // load-modify-save：只动这一个位，语言 / 历史 / 搜索模板原样保留。
            let mut prefs = term_runtime::prefs::load().unwrap_or_default();
            prefs.filer_show_hidden = on;
            term_runtime::prefs::save(&prefs);
        }
    });
    // 视觉基线没有页脚说明；灰行理由挂在 hover 上。
}

/// Filer 页底部说明：这页有一半是真的——真过滤只在本地目录上发生。
const FILER_PAGE_NOTE: &str =
    "Filters local directory listings immediately (names starting with '.'). \
     SFTP listings come from the server, so remote entries are shown as sent.";

/// 「刚清空过」提示在 egui 临时记忆里的键。
fn cleared_id() -> egui::Id {
    egui::Id::new("prefs_history_cleared")
}

/// 页面显示的 TTL：`Prefs` 里的 `0` 是「老文件没这项」的哨兵，不是「当天清空」。
///
/// 抽成纯函数是因为这是本页最容易写错的一处：把 `0` 原样画出来，用户会以为
/// 历史只留 0 天（实际出厂是 30 天）；而写回时又要写**具体值**、不能再写哨兵。
/// 单测钉住 `0 → 30` 与 `45 → 45`（容量同一个道理）。
pub(in crate::ui) fn shown_days(prefs: &term_runtime::prefs::Prefs) -> u64 {
    prefs.history_ttl_days()
}

/// 页面显示的容量，语义同 [`shown_days`]：`0 → 10000`、显式值原样显示。
pub(in crate::ui) fn shown_size(prefs: &term_runtime::prefs::Prefs) -> usize {
    prefs.history_capacity()
}

/// `Terminal ▸ Command History`：视觉基线 `09-command-history.png` 的两个分组框。
///
/// 三个字段**都有读者**，所以这页不该灰显：`command_history_size` /
/// `command_history_days` 建的是内存历史表（`construct.rs` 的
/// `command_history_store `），`command_history_save` 决定退出时写不写盘
/// （`main.rs::on_exit`）。但三处读者原先都只在启动那一刻取一次值，数据表里
/// 的控件改了也没用。现在改容量 / TTL 会当场 `reconfigure`（超限的旧条目同一帧
/// 就掉），并把**具体值**写回 `prefs.json`。
///
/// # 为什么每帧读一次 prefs
///
/// 页面必须显示**生效值**（`0` 哨兵 → 30 天 / 10000 条），而窗口上没有第二份
/// `Prefs` 缓存可用：`command_history` 只存内部容量且不暴露 getter（core 这轮
/// 不动），给 `SettingsData` 加字段又要改窗口构造。所以停在这一页时读一次
/// `prefs.json`——几百字节，且写回仍走 load-modify-save，不会覆盖别的字段。
pub(in crate::ui) fn command_history(win: &mut TerminalWindow, ui: &mut egui::Ui) {
    let prefs = term_runtime::prefs::load().unwrap_or_default();
    let mut days = shown_days(&prefs);
    let mut size = shown_size(&prefs);
    let mut save = prefs.command_history_save;
    // 两个「脏」标志分开：`apply` 决定要不要重建历史表（拖动时每一帧都该跟手），
    // `persist` 决定要不要写盘（每帧写一次 JSON 纯属噪声，松手再落）。
    let mut apply = false;
    let mut persist = false;

    recovery_notice(ui);
    ui.add_space(BLOCK_GAP);
    // 视觉基线 `09-terminal-command-history.png`：内容区第一个字是 `Capacity`（组标题），
    // 没有页级灰标题。标签列按本页最长的 `Saved command history:` 量，两个框里的
    // 控件左缘因此对齐（用户口径：同一页输入框宽度一致）。
    let col = label_col(ui, &[
        "Days to keep history:",
        "History size:",
        "Saved command history:",
    ]);
    group_box(ui, Some("Capacity"), |ui| {
        ui.horizontal(|ui| {
            field_label(ui, "Days to keep history:", col);
            // 下限是 1 而不是 0：`0` 在 prefs 里是「没设置」哨兵，不是「永久保留」
            // （要永久就拖到 36500 天，事实上的永久）。
            // 字段框占满标签之后整段：视觉基线的数字靠左、▲▼ 贴右缘，不是一个小框。
            let (_, stepped) = spin_field_delta(ui, |ui| {
                ui.add(egui::DragValue::new(&mut days).range(1..=36_500).speed(1.0));
            });
            if let Some(d) = stepped {
                days = (days as i64 + d as i64).clamp(1, 36_500) as u64;
                apply = true;
                persist = true;
            }
        });
        ui.horizontal(|ui| {
            field_label(ui, "History size:", col);
            let (drag, stepped) = spin_field_delta(ui, |ui| {
                ui.add(
                    egui::DragValue::new(&mut size)
                        .range(1..=1_000_000)
                        .speed(10.0),
                );
            });
            apply |= drag.changed();
            persist |= drag.drag_stopped() || (drag.changed() && !drag.dragged());
            if let Some(d) = stepped {
                size = (size as i64 + d as i64 * 10).clamp(1, 1_000_000) as usize;
                apply = true;
                persist = true;
            }
        });
    });

    let mut cleared = None;
    ui.add_space(BLOCK_GAP);
    group_box(ui, Some("Storage"), |ui| {
        persist |= choice_row_resp(ui, ChoiceKind::Checkbox, "Save command history to disk", "", &mut save, true, UnderlineStyle::None).clicked();
        ui.horizontal(|ui| {
            field_label(ui, "Saved command history:", col);
            // Clear 与上面三个字段同宽同高：视觉基线里它是一个通栏按钮（文字居中）。
            // 高度取 `field_height` 而不是写死 20——同页的真字段是那个公式算出来的
            // 22 点，写死 20 就比旁边每一行都矮一档。
            if ui
                .add_sized(
                    egui::vec2(ui.available_width(), field_height(ui)),
                    egui::Button::new("Clear"),
                )
                .clicked()
            {
                cleared = Some(win.command_history.len());
                win.command_history.clear();
            }
        });
    });

    if let Some(n) = cleared {
        // 一次性提示放进 egui 的临时记忆：它不是设置值，是「刚才发生了什么」。
        // 记下时间戳让它在 4 秒后自己消失，免得一直挂着像某种状态。
        let now = ui.input(|i| i.time);
        ui.data_mut(|d| d.insert_temp(cleared_id(), (n, now)));
    }
    // `Clear` 只动内存。若「保存历史」开着，退出时会把空表写回文件——这正是
    // 点 Clear 想要的结果（文件里也不该再留着旧命令）；关着则文件保持原样。
    if let Some((n, at)) = ui.data(|d| d.get_temp::<(usize, f64)>(cleared_id())) {
        let age = ui.input(|i| i.time) - at;
        if age < 4.0 {
            ui.label(
                egui::RichText::new(format!("Cleared · {n} entries removed"))
                    .weak()
                    .small(),
            );
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(4.0 - age));
        } else {
            ui.data_mut(|d| d.remove::<(usize, f64)>(cleared_id()));
        }
    }

    if apply {
        // 墙钟只在宿主读（`term-core` 零墙钟）：与 `main.rs` 那个 `now_secs `
        // 同一口径，core 只收注入值。
        win.command_history
            .reconfigure(size, days, crate::now_secs());
    }
    if persist {
        // load-modify-save：只动这三个字段。整份重建会把语言 / 启动策略 /
        // 光标样式悄悄清掉。
        let mut prefs = term_runtime::prefs::load().unwrap_or_default();
        prefs.command_history_days = days;
        prefs.command_history_size = size;
        prefs.command_history_save = save;
        term_runtime::prefs::save(&prefs);
    }
}

/// `Application` 页一帧里改了什么。两个位分开报，由 `settings.rs` 合并成
/// **一次** load-modify-save——同一帧各写一次 prefs 会让后写那份覆盖先写的。
pub(in crate::ui) struct SettingsChange {
    pub(in crate::ui) startup: bool,
    pub(in crate::ui) suppress: bool,
}

/// `Application` 页：视觉基线 `01-application.png` 的两个分组框。
/// 此前这里六个控件全是 `add_enabled(false)`：样子像兼容终端，点下去什么也
/// 不发生。现在三档与四个下拉都绑 `win.startup`，真正决定下次启动摆回哪些
/// 会话；填的是**会话库里的名字**，不是自由文本——填一个库里不存在的名字
/// 在恢复时会被跳过，那才是「静默失效」。
///
/// 下拉只在 `SpecificSessions` 档可用（其余档灰显），免得用户以为在
/// 「Last sessions」下指定的会话也会生效。
pub(in crate::ui) fn application(win: &mut TerminalWindow, ui: &mut egui::Ui) -> SettingsChange {
    use term_app::startup::{StartupMode, StartupPolicy};

    // 闭包里画控件时不能同时可变借用整个 win：先把策略复制出来，画完再写回。
    // 会话名也先收成一份只读快照，ComboBox 的选项与策略互不牵连。
    let before: StartupPolicy = win.startup.clone();
    let mut startup = before.clone();
    let names: Vec<String> = win.sessions.iter().map(|s| s.name.clone()).collect();

    recovery_notice(ui);
    ui.add_space(BLOCK_GAP);
    group_box(ui, Some("Startup"), |ui| {
        // 视觉基线三档单选的行距约 49 物理像素；只靠默认 item_spacing 会挤到
        // 36，后续四个下拉跟着整体上移。按逐页比对结果在第一、二档后各补 13 物理
        // 像素（7.5 逻辑点），最后一档不补，框底才不会被多余空白顶开。
        for (i, mode) in StartupMode::ALL.iter().enumerate() {
            if ui
                .add(egui::RadioButton::new(startup.mode == *mode, mode.label()))
                .clicked()
            {
                startup.mode = *mode;
            }
            if i + 1 < StartupMode::ALL.len() {
                ui.add_space(7.5);
            }
        }
        // 单选末行到 Session 1：视觉基线比默认值多约 22 物理像素（11 逻辑点）。
        ui.add_space(11.0);
        let enabled = startup.mode == StartupMode::SpecificSessions;
        let slot_count = startup.specific.len();
        for (i, slot) in startup.specific.iter_mut().enumerate() {
            let shown = if slot.is_empty() {
                "—".to_string()
            } else {
                slot.clone()
            };
            ui.horizontal(|ui| {
                left_label(ui, &format!("Session {}:", i + 1));
                ui.add_enabled_ui(enabled, |ui| {
                    egui::ComboBox::from_id_salt(format!("prefs_startup_session_{i}"))
                        .selected_text(shown)
                        // 与 `settings_pages_view` 同一条规矩：视觉基线的 230 不能闭眼用，
                        // 140 标签 + 230 组合会超出右栏可用宽度，把窗口顶宽后
                        // `Close` 按钮就被裁在窗口外面。会话名可能很长，`width`
                        // 又只是下限，所以还要 Truncate 把文字锁在框内。
                        .width(ui.available_width())
                        .wrap_mode(egui::TextWrapMode::Truncate)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(slot.is_empty(), "—").clicked() {
                                *slot = String::new();
                            }
                            for name in &names {
                                if ui.selectable_label(slot.as_str() == name, name).clicked() {
                                    *slot = name.clone();
                                }
                            }
                        });
                });
            });
            // 视觉基线四个下拉的填充块起点相隔 58 物理像素；当前控件本身高
            // 40，默认行距再叠上去会走到 78。前三个之后只补 10 物理像素
            // （5 逻辑点），第四个之后不补。
            if i + 1 < slot_count {
                ui.add_space(5.0);
            }
        }
        // 视觉基线第四行下拉底到框底还有约 21 物理像素；当前 wrap 后只剩 3，
        // 框比参考矮一截。这里补 26.5 逻辑点，只为把框底推到参考位置。
        ui.add_space(26.5);
    });

    let startup_changed = startup != before;
    if startup_changed {
        win.startup = startup;
    }

    // 第一个框的底填充加厚后，两框之间的留白要同步收回来，Close And Exit
    // 的标题才不会跟着下移（视觉基线两框间距约 29 物理像素）。
    ui.add_space(1.0);
    let mut suppress = false;
    group_box(ui, Some("Close And Exit"), |ui| {
        let mut show_tab = !win.confirm.suppress_session;
        if choice_row_resp(ui, ChoiceKind::Checkbox, "Show close tab confirmation dialog", "", &mut show_tab, true, UnderlineStyle::None).clicked()
        {
            win.confirm.suppress_session = !show_tab;
            suppress = true;
        }
        // 两行确认框同样按视觉基线补一行间距（兼容终端中心距约 64 物理像素）。
        ui.add_space(12.0);
        let mut show_window = !win.confirm.suppress_window;
        if choice_row_resp(ui, ChoiceKind::Checkbox, "Show close window confirmation dialog", "", &mut show_window, true, UnderlineStyle::None).clicked()
        {
            win.confirm.suppress_window = !show_window;
            suppress = true;
        }
    });
    SettingsChange {
        startup: startup_changed,
        suppress,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_runtime::prefs::Prefs;

    #[test]
    fn zero_sentinel_shows_the_factory_defaults() {
        // 老文件 / 没设置过 → 两个字段都是 0。页面必须显示 30 / 10000，
        // 把 0 原样画出来会让用户以为历史当天就清空。
        let prefs = Prefs::default();
        assert_eq!(shown_days(&prefs), 30, "0 是哨兵，页面要显示出厂 30 天");
        assert_eq!(
            shown_size(&prefs),
            10_000,
            "0 是哨兵，页面要显示出厂 10000 条"
        );
    }

    #[test]
    fn explicit_values_are_shown_as_written() {
        // 用户拖到 45 / 500 之后，页面显示的就是 45 / 500——不是被吞回哨兵，
        // 也不是被夹到出厂值。
        let prefs = Prefs {
            command_history_days: 45,
            command_history_size: 500,
            ..Prefs::default()
        };
        assert_eq!(shown_days(&prefs), 45);
        assert_eq!(shown_size(&prefs), 500);
    }
}
