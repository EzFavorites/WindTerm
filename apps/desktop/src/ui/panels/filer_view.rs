//! Filer 的绘制：路径栏 / 表头 / 文件列表 / 底部状态行。
//!
//! 2026-11-08 从 `filer.rs` 拆出（那边 698 行顶破 450）。`filer.rs` 只留
//! 状态与数据源（本地目录 / SFTP 事件轮询），本文件只画——与
//! `sidebar_view.rs` / `sidebar_rows.rs` 同一分工。
//!
//! # 几何（视觉基线 2× 实测，逻辑点）
//!
//! ```text
//! ┌──────────────────────────────┐
//! │  /Users/unique/  × ↑ ↻ ⋮      │ PATH_H = 22，路径框左对齐
//! │  Name                 Date   │ HEAD_H = 18，左右两列
//! │  ▪ AITOSee      2026/11/07   │ ROW_H  = 19，蓝色文件夹方块 + 名字 + 日期
//! │  …                             │
//! │  21 items  4.3 KB      by Name│ FOOT_H = 20，贴着面板下沿
//! └──────────────────────────────┘
//! ```
//!
//! 列表**不用 `ScrollArea`**：它在这个 child Ui 里会把内容高度当成视口高度，
//! 21 行会一路画穿面板下沿、压到 Sender 上（2026-11-08 实测）。改按
//! `list_rect` 精确算出能放几行、只画这几行，滚轮按整行翻页，`set_clip_rect`
//! 是第二道保险。视觉基线也正是「21 items 只露出 4 行」。
//!
//! 路径栏四颗都有真实效果：`×` 清空输入、`↑` 上一级、`↻` 刷新、三点循环切排序；
//! 路径框回车跳到该目录；点目录行进入；状态行的条目数与总字节数都从当前列表现算。

use super::{sort_lines, up_target, FileLine, Source};
use crate::ui::*;
use eframe::egui;

/// 视觉基线里文件夹图标的亮青蓝。
const FOLDER_BLUE: egui::Color32 = egui::Color32::from_rgb(0x3F, 0xA9, 0xF5);
const PATH_H: f32 = 22.0;
const HEAD_H: f32 = 18.0;
const FOOT_H: f32 = 20.0;
const ROW_H: f32 = 19.0;

/// 滚轮像素量 → 整行位移。`pointer_in_list` 为假时一律 0。
///
/// 门为什么长在这里而不是调用处：`InputState.raw_scroll_delta` 是**整帧全局**
/// 的输入状态，跟「这个 child Ui 的矩形、裁剪、hover」全都无关。无条件读它，
/// 等于把窗口里每一次滚轮都记到本面板账上——指针停在终端上、停在 Explorer 上，
/// 动的永远是这一列文件列表（2026-11-14 用户实测）。
///
/// 判定用 `Ui::rect_contains_pointer` 而不是 `interact(..).hovered()`：列表是
/// 手绘的、没有 response，而临时 allocate 出来的 hover 热区会被下面那些
/// `Sense::click()` 的行抢走，时灵时不灵。终端那侧有真 response，所以它用
/// `Response::hovered()`——两处问的是同一个问题：指针在不在这块可视区里。
fn wheel_rows(wheel: f32, pointer_in_list: bool) -> i64 {
    if !pointer_in_list {
        return 0;
    }
    (wheel / ROW_H).round() as i64
}

/// 官网 page/20：Filter 行按文件名子串过滤当前目录，随时可改。
/// 大小写不敏感，显示名与真名都参与——目录尾斜杠不影响命中。
pub(super) fn filter_lines(lines: &mut Vec<FileLine>, filter: &str) {
    let needle = filter.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return;
    }
    lines.retain(|line| {
        line.name.to_ascii_lowercase().contains(&needle)
            || line.raw_name.to_ascii_lowercase().contains(&needle)
    });
}

impl TerminalWindow {
    /// 画面板。四块区域按实测高度切矩形，互不挤占：路径栏 / 表头 / 列表 /
    /// 底部状态行。切 child Ui 而不是靠 `item_spacing` 堆叠，是为了让底部状态行
    /// 永远贴着面板下沿（视觉基线就是那样）。
    pub(in crate::ui) fn draw_filer(&mut self, ui: &mut egui::Ui) {
        // 拖放判定用的是上一帧矩形：modem_drop 在面板绘制之前轮询。
        self.filer_rect = Some(ui.max_rect());
        self.ensure_filer_session();

        let s = &term_core::theme::CHROME_SCALE;
        let text_col = crate::theme::chrome(s.text);
        let dim = text_col.gamma_multiply(0.62);
        let lang = self.view.lang;
        let name_head = self.tr("Name");
        let date_head = self.tr("Date");
        let items_word = t(lang, "items");
        let by_word = t(lang, "by");

        let mut lines = self.filer_lines();
        // 先按 Filter 行收窄，再排序：底部计数与总大小都只算当前可见的文件。
        let name_filter = self.panel_filter(PanelId::Filer).to_string();
        filter_lines(&mut lines, &name_filter);
        sort_lines(&mut lines, self.filer.sort_by);
        let total: u64 = lines.iter().map(|l| l.size).sum();
        let count = lines.len();
        let sort_by = self.filer.sort_by;
        let is_local = self.filer.source == Source::Local;
        let is_demo = self.filer.demo_rows.is_some();
        let current_path = if is_local {
            self.filer.local_path.to_string_lossy().into_owned()
        } else {
            self.filer.path.clone()
        };
        let mut path_input = std::mem::take(&mut self.filer.path_input);
        // 右键菜单的文案先取好：闭包里再调 `self.tr()` 会和后面那段
        // 「动作落地」的可变借用打架（同 `context_menu.rs` 的做法）。
        let new_folder_label = self.tr("New folder");
        let rename_label = self.tr("Rename");
        let permissions_label = self.tr("Permissions");
        let delete_label = self.tr("Delete");
        // 删整棵树是破坏性更大的一步：单独一项、只对目录出现、有自己的确认窗。
        let delete_tree_label = self.tr("Delete tree");
        // 传输两项走中文硬编码：翻译表在 `crates/term-app/src/i18n.rs`，本轮
        // 的分工不许动那个 crate（另一支在同仓并行改设置页）。工具面板的正文本来
        // 也是中文硬编码，i18n 第二阶段统一时再把这两句搬进 `tr()`。
        let download_label = "下载到本地当前目录";
        let upload_label = "上传到远端当前目录";
        // 传输判据的两项输入，画之前取一次：菜单里每行都要问「现在能不能传」。
        let connected = self.filer.handle.is_some();
        let remote_dir = self.filer.path.clone();
        // 目录同步的两项输入。本地根在远端视图下也一直是有效的
        // （`local_path` 跨帧保留），所以两个根都能拿到才有「同步」可言。
        let sync_local = self.filer.local_path.clone();
        let sync_local_display = if sync_local.as_os_str().is_empty() {
            String::new()
        } else {
            sync_local.to_string_lossy().into_owned()
        };
        let sync_label = "目录同步（本地 ↔ 远端）…";
        let sync_busy = self.filer.sync.is_some();
        // 执行阶段禁止对同一棵树做别的写操作：`SyncRun` 靠「同一时刻只有一步在飞」
        // 保证顺序与连坐，中途插进来的 mkdir / rename 会和同步那一步抢同一批事件
        // （`Done` / `Failed` 只带远端路径，认不出是谁的）。
        let sync_running = matches!(self.filer.sync, Some(super::sync::SyncState::Running(_)));
        let sync_blocker: Option<&'static str> = if !connected {
            Some("会话已断开")
        } else if sync_local_display.is_empty() {
            Some("本地目录不可用")
        } else if sync_busy {
            Some("正在扫描…")
        } else {
            None
        };

        // 收集这一帧的动作，画完再落地：`self.filer` 与 lines 同时要用。
        let mut goto: Option<String> = None;
        let mut clear = false;
        let mut up = false;
        let mut refresh = false;
        let mut cycle_sort = false;
        let mut goto_path = false;
        // 右键菜单里选中的写操作（画完再落地）。
        let mut op: Option<super::ops::FilerOp> = None;
        // 右键菜单里选中的传输（同样画完再落地）。
        let mut xfer: Option<super::xfer::XferPick> = None;
        // 右键菜单里点了「多选上传」（官网 page/21）：画完再开弹层。
        let mut multi_upload = false;
        // 右键菜单里点了「目录同步」（画完再落地）。
        let mut sync_now = false;
        // 双击 / 右键「预览」选中的行（排序后整张表的下标，画完再落地）。
        let mut open_at: Option<usize> = None;
        let preview_label = self.tr("Preview");

        // 用「剩余」而不是 `max_rect()`：容器已经画掉标题带 / Filter 并推进了
        // 光标，`max_rect()` 仍是整块面板，从它起笔会把内容盖到标题上。
        let inner = ui
            .available_rect_before_wrap()
            .shrink2(egui::vec2(6.0, 2.0));
        let path_rect = egui::Rect::from_min_size(inner.min, egui::vec2(inner.width(), PATH_H));
        let head_rect = egui::Rect::from_min_size(
            egui::pos2(inner.left(), path_rect.bottom()),
            egui::vec2(inner.width(), HEAD_H),
        );
        let foot_rect =
            egui::Rect::from_min_max(egui::pos2(inner.left(), inner.bottom() - FOOT_H), inner.max);
        let list_rect = egui::Rect::from_min_max(
            egui::pos2(inner.left(), head_rect.bottom()),
            egui::pos2(inner.right(), foot_rect.top()),
        );

        // ── 路径栏 ──
        let mut path_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("filer_path")
                .max_rect(path_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        path_ui.set_clip_rect(path_rect);
        let btn_w = 16.0;
        let edit_w = (path_rect.width() - 4.0 * (btn_w + 4.0) - 6.0).max(30.0);
        let resp = path_ui.add_sized(
            egui::vec2(edit_w, PATH_H - 2.0),
            egui::TextEdit::singleline(&mut path_input)
                .id(egui::Id::new("filer_path_input"))
                .frame(false)
                .font(crate::ui::type_scale::small())
                .text_color(text_col),
        );
        if resp.lost_focus() && path_ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            goto_path = true;
        }
        for (glyph, hint, slot) in [
            ("×", "Clear path", &mut clear),
            ("↑", "Up", &mut up),
            ("↻", "Refresh", &mut refresh),
        ] {
            let r = path_ui.add_sized(
                egui::vec2(btn_w, 16.0),
                egui::Label::new(egui::RichText::new(glyph).color(dim)).sense(egui::Sense::click()),
            );
            if r.on_hover_text(self.tr(hint)).clicked() {
                *slot = true;
            }
        }
        // 第四颗是三个竖点。夹具字体里没有 `⋮` 这个字形（会渲染成方框），
        // 所以照 `tab_bar.rs` 的做法用绘图原语画，不碰字形表。
        let (sort_rect, sort_resp) =
            path_ui.allocate_exact_size(egui::vec2(btn_w, 16.0), egui::Sense::click());
        for k in 0..3 {
            path_ui.painter().circle_filled(
                egui::pos2(
                    sort_rect.center().x,
                    sort_rect.center().y + (k as f32 - 1.0) * 4.0,
                ),
                1.2,
                if sort_resp.hovered() { text_col } else { dim },
            );
        }
        if sort_resp.on_hover_text(self.tr("Sort")).clicked() {
            cycle_sort = true;
        }

        // ── 表头 ──
        let head_p = ui.painter();
        head_p.text(
            egui::pos2(head_rect.left() + 2.0, head_rect.center().y),
            egui::Align2::LEFT_CENTER,
            name_head,
            crate::ui::type_scale::micro(),
            dim,
        );
        head_p.text(
            egui::pos2(head_rect.right() - 2.0, head_rect.center().y),
            egui::Align2::RIGHT_CENTER,
            date_head,
            crate::ui::type_scale::micro(),
            dim,
        );
        head_p.line_segment(
            [
                egui::pos2(head_rect.left(), head_rect.bottom() - 0.5),
                egui::pos2(head_rect.right(), head_rect.bottom() - 0.5),
            ],
            egui::Stroke::new(1.0, crate::theme::chrome(s.border)),
        );

        // ── 文件列表 ──
        //
        // 不用 `ScrollArea`：它在这个 child Ui 里会把内容高度当成视口高度，
        // 21 行会一路画穿面板下沿、压到 Sender 上（2026-11-08 实测）。这里按
        // `list_rect` 精确算出能放几行、只画这几行，滚轮按整行翻页；
        // `set_clip_rect` 是第二道保险。视觉基线也正是「21 items 只露出 4 行」。
        let mut list_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("filer_list")
                .max_rect(list_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        list_ui.set_clip_rect(list_rect);
        list_ui.spacing_mut().item_spacing.y = 0.0;
        let wheel = wheel_rows(
            list_ui.input(|i| i.raw_scroll_delta.y),
            list_ui.rect_contains_pointer(list_rect),
        );
        let visible = (list_rect.height() / ROW_H).floor().max(0.0) as usize;
        let extra =
            usize::from(self.filer.error.is_some()) + usize::from(self.filer.progress.is_some());
        let file_slots = visible.saturating_sub(extra);
        let max_scroll = lines.len().saturating_sub(file_slots);
        if wheel != 0 {
            let next = self.filer.scroll as i64 + wheel;
            self.filer.scroll = next.clamp(0, max_scroll as i64) as usize;
        }
        let start = self.filer.scroll.min(max_scroll);
        let shown: Vec<&FileLine> = lines.iter().skip(start).take(file_slots).collect();

        if let Some(err) = &self.filer.error {
            list_ui.colored_label(egui::Color32::from_rgb(0xE0, 0x6C, 0x6C), err);
        }
        if let Some(progress) = &self.filer.progress {
            // 单行截断，不换行：上面那「多出来的槽位」是按 `is_some()` 扣的一行，
            // 文案一换行就会把下面那行文件挤掉一格。目录同步的汇总文案
            // （「本地 → 远端 ｜ 2 项待上传…」）比早期的「path: done」长得多。
            list_ui.add(egui::Label::new(egui::RichText::new(progress).weak().small()).truncate());
        }
        for (offset, line) in shown.iter().enumerate() {
            let (rect, _) = list_ui
                .allocate_exact_size(egui::vec2(list_rect.width(), ROW_H), egui::Sense::hover());
            // 蓝色文件夹方块：绘图原语，不依赖字体里有没有这个字形。
            let icon = egui::Rect::from_center_size(
                egui::pos2(rect.left() + 8.0, rect.center().y),
                egui::vec2(9.0, 9.0),
            );
            if line.is_dir {
                list_ui.painter().rect_filled(icon, 1.0, FOLDER_BLUE);
            } else if line.is_symlink {
                list_ui
                    .painter()
                    .rect_stroke(icon, 1.0, egui::Stroke::new(1.0, dim));
            }
            // 名字左对齐、按可用宽度按字符截断。用 `add_sized(Label)` 会把整条
            // 文字在给定宽度里居中（实测 `AITOSee` 跑到了图标与日期中间），
            // 所以这里直接 painter 画字 + `interact` 注册点击热区。
            let text_rect = egui::Rect::from_min_max(
                egui::pos2(rect.left() + 18.0, rect.min.y),
                egui::pos2(rect.right() - 72.0, rect.max.y),
            );
            let text_resp = list_ui.interact(
                text_rect,
                egui::Id::new(("filer_row", &line.name)),
                egui::Sense::click(),
            );
            let max_chars = (text_rect.width() / 6.6).floor().max(2.0) as usize;
            let mut chars = line.name.chars();
            let shown_name: String = chars.by_ref().take(max_chars - 1).collect();
            let shown_name = if chars.next().is_some() {
                format!("{shown_name}…")
            } else {
                shown_name
            };
            list_ui.painter().text(
                egui::pos2(text_rect.left(), text_rect.center().y),
                egui::Align2::LEFT_CENTER,
                shown_name,
                crate::ui::type_scale::small(),
                text_col,
            );
            if text_resp.clicked() && line.is_dir {
                goto = Some(line.target.clone());
            }
            // 双击是**打开**——目录进去、本地文件立刻预览、远端文件先下载。
            // 单击仍然只做「目录进入」（既有行为，视觉基线与老用户的手感都钉着
            // 它，不在这里顺手改成「单击即预览」）。
            if text_resp.double_clicked() {
                open_at = Some(start + offset);
            }
            // 入口是右键：面板里已经有一排路径按钮，再加一排会让整窗基线漂移；
            // 而这些动作本来就「对着某一行」做。
            //
            // 传输一项两条源都有，但方向相反：远端视图只能「下载」（本地视图只能
            // 「上传」）——反过来的那条在这个面板里无处可选源，画上去就是假入口。
            // 写操作仍然只有 SFTP 源有：本地目录的写操作（新建 / 改名 / 删除）还没
            // 做，给一个点了没反应的菜单比不给更糟。
            let pick = if is_local {
                super::xfer::XferPick::Upload {
                    local: line.target.clone(),
                    name: line.raw_name.clone(),
                    size: line.size,
                    is_dir: line.is_dir,
                }
            } else {
                super::xfer::XferPick::Download {
                    remote: line.target.clone(),
                    name: line.raw_name.clone(),
                    size: line.size,
                    is_dir: line.is_dir,
                }
            };
            let label = if is_local { upload_label } else { download_label };
            let blocker = super::xfer::transfer_blocker(connected, &remote_dir, line.is_dir);
            text_resp.context_menu(|ui| {
                // 预览。双击也能开，但双击不可发现——菜单里必须有一条
                // 看得见的入口（能力不能只有一个隐藏手势）。目录不给这一项：
                // 目录没有内容可预览，画上去就是点了没反应的假入口。
                if !line.is_dir {
                    if ui.button(preview_label).clicked() {
                        open_at = Some(start + offset);
                        ui.close_menu();
                    }
                    ui.separator();
                }
                // 灰项下面必须跟一句原因：只灰不说，用户会以为面板坏了。
                if ui
                    .add_enabled(blocker.is_ok() && !sync_running, egui::Button::new(label))
                    .clicked()
                {
                    xfer = Some(pick);
                    ui.close_menu();
                }
                // 官网 page/21：一次勾选多个本地文件/目录上传。入口放在右键菜单
                // 而不是路径栏——那四颗按钮的几何是视觉基线，多一颗就得重冻。
                // 两种源都能用：本地视图本就列着本地文件，远端视图也可以用本地
                // 文件填远端当前目录（源由弹层自己列，不依赖这一行）。
                ui.separator();
                let pick = ui.add_enabled(blocker.is_ok() && !sync_running, egui::Button::new("选择多个本地文件上传…"));
                if pick.on_hover_text("勾选多个本地文件/目录，逐个上传到远端当前目录").clicked() { multi_upload = true; ui.close_menu(); }
                if let Err(why) = blocker {
                    ui.label(egui::RichText::new(why).weak().small());
                } else if sync_running {
                    ui.label(egui::RichText::new("正在同步…").weak().small());
                }
                if !is_local {
                    let here = current_path.clone();
                    // 目录同步。它与下面的 `New folder` 同类——作用对象是
                    // **当前目录**，不是右键点中的那一行。所以只在远端视图出现
                    // （本地视图里没有远端根），文案里点名「目录」。
                    ui.separator();
                    if ui
                        .add_enabled(sync_blocker.is_none(), egui::Button::new(sync_label))
                        .on_hover_text(format!("{sync_local_display} → {here}"))
                        .clicked()
                    {
                        sync_now = true;
                        ui.close_menu();
                    }
                    if let Some(why) = sync_blocker {
                        ui.label(egui::RichText::new(why).weak().small());
                    }
                    ui.separator();
                    if ui
                        .add_enabled(!sync_running, egui::Button::new(new_folder_label))
                        .clicked()
                    {
                        op = Some(super::ops::FilerOp::NewFolder {
                            dir: here.clone(),
                            name: String::new(),
                        });
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(!sync_running, egui::Button::new(rename_label))
                        .clicked()
                    {
                        op = Some(super::ops::FilerOp::Rename {
                            from: line.target.clone(),
                            // 用真名，不是屏上那个（带 `/` 或 `@` 的标签）：
                            // 拿标签去改名会把尾斜杠一起写进去。
                            name: line.raw_name.clone(),
                        });
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(!sync_running, egui::Button::new(permissions_label))
                        .clicked()
                    {
                        op = Some(super::ops::FilerOp::Chmod {
                            path: line.target.clone(),
                            mode: String::new(),
                        });
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(!sync_running, egui::Button::new(delete_label))
                        .clicked()
                    {
                        op = Some(super::ops::FilerOp::Delete {
                            path: line.target.clone(),
                            is_dir: line.is_dir,
                        });
                        ui.close_menu();
                    }
                    if line.is_dir
                        && ui
                            .add_enabled(!sync_running, egui::Button::new(delete_tree_label))
                            .clicked()
                    {
                        // 这里只立确认窗；真正的删除要等扫描结果到齐、用户再点一次。
                        op = Some(super::ops::FilerOp::DeleteTree {
                            path: line.target.clone(),
                        });
                        ui.close_menu();
                    }
                }
            });
            let date = if line.date.is_empty() {
                "—".to_string()
            } else {
                line.date.clone()
            };
            list_ui.painter().text(
                egui::pos2(rect.right() - 2.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                date,
                crate::ui::type_scale::micro(),
                dim,
            );
        }

        // ── 底部状态行 ──
        let foot_p = ui.painter();
        foot_p.text(
            egui::pos2(foot_rect.left() + 2.0, foot_rect.center().y),
            egui::Align2::LEFT_CENTER,
            format!("{count} {items_word}  {}", term_sftp::format_size(total)),
            crate::ui::type_scale::micro(),
            dim,
        );
        foot_p.text(
            egui::pos2(foot_rect.right() - 2.0, foot_rect.center().y),
            egui::Align2::RIGHT_CENTER,
            format!("{by_word} {}", t(lang, sort_by.label())),
            crate::ui::type_scale::micro(),
            dim,
        );

        // ── 动作落地 ──
        if clear {
            path_input.clear();
        }
        if cycle_sort {
            self.filer.sort_by = sort_by.next();
        }
        let mut local_target: Option<String> = None;
        let mut sftp_target: Option<String> = None;
        if let Some(p) = goto {
            if is_local {
                local_target = Some(p);
            } else {
                sftp_target = Some(p);
            }
        } else if up {
            if is_local {
                local_target = self
                    .filer
                    .local_path
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned());
            } else {
                sftp_target = up_target(&current_path);
            }
        } else if refresh || goto_path {
            let p = if goto_path {
                path_input.clone()
            } else {
                current_path.clone()
            };
            if is_local {
                local_target = Some(p);
            } else {
                sftp_target = Some(p);
            }
        }
        if let Some(p) = local_target {
            if !is_demo {
                // 「设路径 + 重列」只有 `enter_filer_dir` 一份实现（双击也走它）。
                if self.enter_filer_dir(&p) {
                    path_input = p;
                }
            } else {
                path_input = p;
            }
        }
        if let Some(p) = sftp_target {
            if let Some(handle) = self.filer.handle.as_ref() {
                handle.list(p);
                self.filer.loading = true;
                self.filer.error = None;
            }
        }
        self.filer.path_input = path_input;
        // 右键菜单选完再落地：绘制期间 `lines` / `self.filer` 都借着。
        if let Some(pick) = xfer {
            // 回执写进已有的状态行（`progress` 槽）：新开一个提示位就要在别处
            // 记得清它，那是第二个真相。排上了 / 已经在队列里，都有话说。
            let note = self.start_filer_transfer(pick);
            self.filer.progress = Some(note);
        }
        if let Some(op) = op {
            self.begin_filer_op(op);
        }
        if multi_upload {
            // 起点取 Filer 的本地当前目录（面板可切 Source::Local），与用户眼前
            // 那份列表同一处；空路径时退回家目录，不给一个打不开的弹层。
            let start = if self.filer.local_path.as_os_str().is_empty() { term_io::local_dir::home() } else { self.filer.local_path.clone() };
            self.open_upload_picker(start);
        }
        // 只发起扫描，不改任何文件。汇总在下一帧由 `poll_sync` 写进状态行。
        if sync_now {
            self.filer.begin_sync();
        }
        // 双击 / 右键「预览」的落地放在最后：`open_filer_entry` 会重算整张
        // 表并按排序后的下标取行，所以它必须在`lines`（本帧的快照）用完、
        // 且 `self.filer` 的路径栏缓冲已经写回之后再跑。
        if let Some(index) = open_at {
            self.open_filer_entry(index);
        }
    }
}

impl TerminalWindow {
    /// 多选上传的汇总回执。与右键单项上传共用 Filer 状态行那一个 `progress` 槽，
    /// 不另开提示位；`view` 是 `filer` 的子模块，能写它的私有字段。
    pub(in crate::ui) fn note_multi_upload(&mut self, notes: Vec<String>) {
        self.filer.progress = Some(if notes.is_empty() { "所选路径都已不存在，未入队".to_string() } else { notes.join("；") });
    }
}

#[cfg(test)]
mod filter_tests {
    use super::*;

    fn row(name: &str) -> FileLine {
        FileLine {
            name: name.to_string(),
            raw_name: name.trim_end_matches('/').to_string(),
            is_dir: name.ends_with('/'),
            is_symlink: false,
            size: 0,
            date: String::new(),
            target: name.to_string(),
        }
    }

    #[test]
    fn filter_matches_file_names_case_insensitively_and_keeps_empty_query_a_noop() {
        let mut rows = vec![row("readme.md"), row("src/"), row("notes.txt")];
        filter_lines(&mut rows, "");
        assert_eq!(rows.len(), 3, "空过滤词不该改列表");
        filter_lines(&mut rows, "MD");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "readme.md");
        let mut dirs = vec![row("src/"), row("notes.txt")];
        filter_lines(&mut dirs, "src"); // 目录显示名带尾斜杠仍要命中
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].name, "src/");
    }
}

#[cfg(test)]
#[path = "filer_wheel_tests.rs"]
mod wheel_tests;
