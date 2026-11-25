//! 应用状态快照的**采集**（ADR-0027）。
//!
//! 形状在 `term_core::app_state`，采集只能在这里：面板停靠位来自 `DockLayout`
//! （`pub(in crate::ui)`）、可见性来自 `ViewState`、透明度来自 `WindowOpacity`——
//! 三样都只有 `ui` 里拿得到，`term-app` 拿不到（它不依赖 `egui`）。
//!
//! # 两个版本号
//!
//! `app_rev` 只由「界面真的变了」的写入推进（面板置位 / 将来的布局动作），
//! `prefs_rev` 只由偏好写入推进。合成一个的话，脚本「靠 rev 判断要不要重取」
//! 会漏掉偏好变化。
//!
//! # 偏好段读的是盘
//!
//! `prefs` 段回的是**落盘值**，不是设置页里未保存的编辑——运行期的界面真值在
//! `ViewState` / `WindowOpacity` 里，已经分别落在 `window` 与 `panels` 段。
//! 目前只暴露语言：它是唯一一条「改了能立刻生效」的偏好。

#[cfg(test)]
#[path = "app_state_prefs_rev_tests.rs"]
mod app_state_prefs_rev_tests;

use crate::ui::layout::Placement;
use crate::ui::*;
use term_app::action::Action;
use term_app::dock::{DockZone, LayoutAction, PanelId};
use term_app::shell::panel_visible;
use term_core::app_state::{AppSnapshot, PanelState, WindowState};
// 走真实 crate 路径而不是 `term_runtime::prefs` 门面：门面里那个模块只是
// `term_profile::prefs` 的再导出，经它会让计数 +1（基线只许减）。
use term_profile::prefs;

impl TerminalWindow {
    /// 本帧的一份应用状态快照。界面线程调用（读的都是本线程独占的字段）。
    pub(in crate::ui) fn app_snapshot(&self) -> AppSnapshot {
        let panels: Vec<PanelState> = PanelId::ALL
            .iter()
            .map(|id| {
                // `placement()` 对「表里没有」的 id 会退回 `default_zone()` + 0，
                // 所以这里不会出现 Option 分支——面板集合恒十条。
                let (zone, order, floating, rect) = match self.dock.placement(*id) {
                    Placement::Dock { zone, order } => (zone.name(), order, false, None),
                    Placement::Floating { rect } => (
                        id.default_zone().name(),
                        // 浮窗没有「带内位次」这回事；给 0 而不是随机值，
                        // 免得客户端把两次快照的 order 差异当成移动。
                        0,
                        true,
                        Some([
                            rect.min.x.round() as i32,
                            rect.min.y.round() as i32,
                            rect.width().round() as i32,
                            rect.height().round() as i32,
                        ]),
                    ),
                };
                PanelState {
                    id: id.name(),
                    visible: panel_visible(&self.view, *id),
                    zone,
                    order,
                    floating,
                    rect,
                }
            })
            .collect();

        let language = prefs::load()
            .map(|prefs| prefs.language)
            .unwrap_or_default();

        AppSnapshot {
            rev: self.app_rev,
            prefs_rev: self.prefs_rev,
            leaves: self.layout.leaf_count(),
            panels,
            window: WindowState {
                full_screen: self.view.full_screen,
                always_on_top: self.view.always_on_top,
                focus_mode: self.view.focus_mode,
                opacity: self.window_opacity.alpha(),
            },
            prefs_language: language,
        }
    }
}
impl TerminalWindow {
    /// 取一条应用控制请求、执行、回执。每帧最多一条（见调用点 `draw`）。
    ///
    /// 为什么读也走这里：共享槽只保证「上一帧」，脚本「写之后立刻读」会拿到
    /// 旧快照，于是把成功当成失败。往返一次一帧，同帧只服务一个请求。
    pub(in crate::ui) fn drain_app_requests(&mut self) {
        let pending = match self.app_queue.lock() {
            Ok(mut queue) => loop {
                if queue.is_empty() {
                    break None;
                }
                let next = queue.remove(0);
                // 已经被「停止」的请求直接丢：脚本侧早就报了 Cancelled，
                // 再执行 panel_set / layout 就是用户点了停止面板还自己动。
                if !next.is_stale() {
                    break Some(next);
                }
            },
            // 中毒 = 界面线程持锁时崩过。把积压的请求全部丢掉：`PendingApp` 一
            // drop，回信通道就断，脚本侧拿到 `Disconnected` → `Busy`，比让它
            // 空等到 2 秒超时更快说出真相（也免得那条通道一直吊着）。
            Err(poisoned) => {
                poisoned.into_inner().clear();
                None
            }
        };
        let Some(pending) = pending else {
            return;
        };
        let reply = match pending.request {
            crate::script_app::AppRequest::Snapshot => {
                crate::script_app::AppReply::Snapshot(self.app_snapshot())
            }
            crate::script_app::AppRequest::PanelSet { panel, visible } => {
                match PanelId::parse(&panel) {
                    None => crate::script_app::AppReply::Error(
                        // 名字不存在 ≠ 参数形状不对：前者要拿清单对一遍，
                        // 后者要改类型。压缩成 `BadArgument` 会让脚本少一条分支。
                        term_script::AppError::UnknownName {
                            kind: "停靠面板",
                            name: panel,
                        },
                    ),
                    Some(id) => {
                        // 先记现状：`changed` 按「真的变了没有」给，而不是按「调用成功」。
                        let before = panel_visible(&self.view, id);
                        self.dispatch_action(Action::Shell(term_app::vocab::ShellAction::SetPanel(
                            id, visible,
                        )));
                        if before == visible {
                            // 幂等：同值不推进版本，也不写 changed。
                            crate::script_app::AppReply::Delta(term_script::AppDelta::unchanged(
                                self.app_rev,
                                self.app_snapshot().panels,
                            ))
                        } else {
                            self.app_rev += 1;
                            crate::script_app::AppReply::Delta(term_script::AppDelta {
                                changed: vec![format!("panels.{}", id.name())],
                                rev: self.app_rev,
                                // 回执带写入**之后**的全景：脚本接着判断
                                // 现状时不必再发一次 `app_state`，也就不必赌时序。
                                panels: self.app_snapshot().panels,
                            })
                        }
                    }
                }
            }
            crate::script_app::AppRequest::Layout(op) => match self.apply_app_layout(&op) {
                Ok(delta) => crate::script_app::AppReply::Delta(delta),
                Err(err) => crate::script_app::AppReply::Error(err),
            },
        };
        let _ = pending.reply.send(reply);
        // 回执之后要一帧重绘：面板可见性变了，画面上必须看得见。
        self.ctx.request_repaint();
    }
}

impl TerminalWindow {
    /// 名字 → `PanelId`。`term-core` 那边只有形状，名字表在这里
    /// （`PanelId::parse`），所以校验也在这里。
    pub(in crate::ui) fn app_panel_id(&self, name: &str) -> Result<PanelId, term_script::AppError> {
        PanelId::parse(name).ok_or_else(|| term_script::AppError::UnknownName {
            kind: "停靠面板",
            name: name.to_string(),
        })
    }

    /// 把一次布局请求落到窗口上。
    ///
    /// 校验（名字 / 停靠带 / 序号）全部在 `dispatch_action` **之前**完成，
    /// 所以失败路径没有副作用：布局文件、`rev`、`DockLayout` 一样都不动。
    pub(in crate::ui) fn apply_app_layout(
        &mut self,
        op: &term_script::LayoutOp,
    ) -> Result<term_script::AppDelta, term_script::AppError> {
        match op {
            term_script::LayoutOp::Reset => {
                self.dispatch_action(Action::Layout(LayoutAction::Reset));
                self.app_rev += 1;
                Ok(self.layout_delta(None))
            }
            term_script::LayoutOp::Redock { panel } => {
                let id = self.app_panel_id(panel)?;
                if !self.dock.is_floating(id) {
                    return Err(term_script::AppError::BadArgument(format!(
                        "{panel} 现在是停靠状态，redock 无从收起（它只收浮窗）"
                    )));
                }
                self.dispatch_action(Action::Layout(LayoutAction::Redock(id)));
                self.app_rev += 1;
                Ok(self.layout_delta(Some(id)))
            }
            term_script::LayoutOp::Dock { panel, zone, index } => {
                let id = self.app_panel_id(panel)?;
                let zone =
                    DockZone::parse(zone).ok_or_else(|| term_script::AppError::UnknownName {
                        kind: "停靠带",
                        name: zone.clone(),
                    })?;
                // 缺省 = 追加到末尾；**显式越界才是错**。投影层的 `insert` 会把越界
                // 夹成追加（服务拖拽），API 层不能继承那个夹取——否则
                // 「index: 99」会静默变成「末尾」，调用方以为位次照办了。
                let members = self.dock.band_members(zone).len();
                let index = match index {
                    None => members,
                    Some(raw) => {
                        let index = usize::from(*raw);
                        if index > members {
                            return Err(term_script::AppError::BadArgument(format!(
                                "index {index} 超过 {} 带的成员数 {members}（缺省即追加到末尾）",
                                zone.name()
                            )));
                        }
                        index
                    }
                };
                self.dispatch_action(Action::Layout(LayoutAction::Dock {
                    panel: id,
                    zone,
                    index,
                }));
                self.app_rev += 1;
                Ok(self.layout_delta(Some(id)))
            }
        }
    }

    /// 布局动作的回执：`changed` + `rev` + 写入之后的十块面板全景。
    ///
    /// `changed` 里的 `layout` 是**恒在**的那一项：布局改了，哪怕某块面板的
    /// `row` 看不出来（例如 `reset` 回的还是同一个位次），文件也重写了一次。
    pub(in crate::ui) fn layout_delta(&self, panel: Option<PanelId>) -> term_script::AppDelta {
        let mut changed = Vec::new();
        if let Some(id) = panel {
            changed.push(format!("panels.{}", id.name()));
        }
        changed.push("layout".to_string());
        term_script::AppDelta {
            changed,
            rev: self.app_rev,
            panels: self.app_snapshot().panels,
        }
    }
}
