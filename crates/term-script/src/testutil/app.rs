//! 假的应用控制面（`AppControl`）。
//!
//! 与 [`super::FakeSessions`] 同一个用途：让「脚本调 → 参数错 → 错误文案」这条
//! 链路在没有窗口的情况下可测。**语义跟真端口一致**，不是返回固定值的桩：
//! 认不出名字报 `UnknownName`、设成同值 `changed` 为空且 `rev` 不动、
//! 回执带写入之后的十块面板全景——这些正是刀2 的验收项，桩要是「永远成功」，
//! 那几条断言等于没测。

use std::sync::{Arc, Mutex};

use term_core::app_state::{AppSnapshot, PanelState, WindowState};

use crate::app_ui::{AppControl, AppDelta, AppError, CatalogEntry, CommandAck, LayoutOp};

/// 十块面板的 id，与 `PanelId::ALL` 同序同名（`crates/term-app/src/vocab.rs`）。
///
/// `term-script` 只依赖 `term-core`，拿不到 `term-app` 的 `PanelId`，所以这里
/// 用一份字面量镜像；名字对不上的风险由 `apps/desktop` 的采集层测试兜住
/// （那边能拿到真的 `PanelId::ALL`）。`zone` / `order` 同理是假值——真实停靠位
/// 只有 `DockLayout` 知道，假端口只需要「形状是十条」。
const PANEL_COUNT: usize = 10;

const PANEL_IDS: [&str; PANEL_COUNT] = [
    "pane_explorer",
    "filer",
    "sidebar",
    "pane_outline",
    "pane_sender",
    "pane_session",
    "pane_shell",
    "pane_transfer",
    "pane_sysmon",
    "pane_script",
];

/// 假端口的状态。`Clone` 共享同一份（`Arc<Mutex<_>>`），脚本线程与断言线程
/// 看的是同一个对象——与 `FakeSessions` 一致。
#[derive(Clone)]
pub struct FakeApp {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    /// 十块面板的可见性，下标与 [`PANEL_IDS`] 对齐。初值全 false（出厂态）。
    visible: [bool; PANEL_COUNT],
    /// 收到的 `panel_set` 调用，顺序即到达顺序（含「设成同值」的空操作）。
    sets: Vec<(String, bool)>,
    /// 收到的 `layout` 调用，顺序即到达顺序。
    layouts: Vec<LayoutOp>,
    /// 收到的 `command` 调用，顺序即到达顺序。
    commands: Vec<String>,
    rev: u64,
    language: String,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            visible: [false; PANEL_COUNT],
            sets: Vec::new(),
            layouts: Vec::new(),
            commands: Vec::new(),
            rev: 0,
            language: "zh-CN".to_string(),
        }
    }
}

impl Default for FakeApp {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }
}

impl Inner {
    fn panels(&self) -> Vec<PanelState> {
        PANEL_IDS
            .iter()
            .enumerate()
            .map(|(index, id)| PanelState {
                id,
                visible: self.visible[index],
                zone: "left",
                order: index as u16,
                floating: false,
                rect: None,
            })
            .collect()
    }
}

impl FakeApp {
    pub fn new() -> Self {
        Self::default()
    }

    /// 预置快照里的语言（`app_state().prefs.language`）。
    pub fn with_language(self, language: &str) -> Self {
        if let Ok(mut inner) = self.inner.lock() {
            inner.language = language.to_string();
        }
        self
    }

    /// 到目前为止收到的 `panel_set` 调用（顺序即到达顺序）。
    pub fn sets(&self) -> Vec<(String, bool)> {
        self.inner.lock().map(|i| i.sets.clone()).unwrap_or_default()
    }

    /// 到目前为止收到的 `layout` 调用（顺序即到达顺序）。
    pub fn layouts(&self) -> Vec<LayoutOp> {
        self.inner.lock().map(|i| i.layouts.clone()).unwrap_or_default()
    }

    /// 当前界面状态版本（断言「同值不推进」时读它）。
    pub fn rev(&self) -> u64 {
        self.inner.lock().map(|i| i.rev).unwrap_or_default()
    }

    /// 到目前为止收到的 `command` 调用（顺序即到达顺序）。
    pub fn commands(&self) -> Vec<String> {
        self.inner.lock().map(|i| i.commands.clone()).unwrap_or_default()
    }
}

impl AppControl for FakeApp {
    fn snapshot(&self) -> Result<AppSnapshot, AppError> {
        let inner = self.inner.lock().map_err(|_| AppError::Busy)?;
        Ok(AppSnapshot {
            rev: inner.rev,
            prefs_rev: 0,
            leaves: 1,
            panels: inner.panels(),
            window: WindowState {
                full_screen: false,
                always_on_top: false,
                focus_mode: false,
                opacity: 1.0,
            },
            prefs_language: inner.language.clone(),
        })
    }

    fn panel_set(&self, panel: &str, visible: bool) -> Result<AppDelta, AppError> {
        let mut inner = self.inner.lock().map_err(|_| AppError::Busy)?;
        let Some(index) = PANEL_IDS.iter().position(|id| *id == panel) else {
            return Err(AppError::UnknownName {
                kind: "停靠面板",
                name: panel.to_string(),
            });
        };
        inner.sets.push((panel.to_string(), visible));
        if inner.visible[index] == visible {
            // 幂等：同值不推进版本、不写 changed，但回执照样带全景。
            return Ok(AppDelta::unchanged(inner.rev, inner.panels()));
        }
        inner.visible[index] = visible;
        inner.rev += 1;
        Ok(AppDelta {
            changed: vec![format!("panels.{panel}")],
            rev: inner.rev,
            panels: inner.panels(),
        })
    }

    fn layout(&self, op: &LayoutOp) -> Result<AppDelta, AppError> {
        let mut inner = self.inner.lock().map_err(|_| AppError::Busy)?;
        // 名字校验与真端口同一口径：认不出就报 `UnknownName`，不静默无操作。
        let panel = match op {
            LayoutOp::Reset => None,
            LayoutOp::Dock { panel, .. } | LayoutOp::Redock { panel } => {
                if !PANEL_IDS.contains(&panel.as_str()) {
                    return Err(AppError::UnknownName {
                        kind: "停靠面板",
                        name: panel.clone(),
                    });
                }
                Some(panel.clone())
            }
        };
        inner.layouts.push(op.clone());
        inner.rev += 1;
        let mut changed = Vec::new();
        if let Some(name) = panel {
            changed.push(format!("panels.{name}"));
        }
        changed.push("layout".to_string());
        Ok(AppDelta {
            changed,
            rev: inner.rev,
            panels: inner.panels(),
        })
    }

    fn command(&self, id: &str) -> Result<CommandAck, AppError> {
        let mut inner = self.inner.lock().map_err(|_| AppError::Busy)?;
        inner.commands.push(id.to_string());
        // 假端口只认一小组 ID：未知 ID 必须显式报「没派发」，不能一律成功。
        let known = matches!(
            id,
            "Terminal.ClearScreen" | "Terminal.ClearScrollback" | "Terminal.ClearAll"
        );
        Ok(CommandAck::dispatched(known))
    }

    fn catalog(&self) -> Result<Vec<CatalogEntry>, AppError> {
        Ok(vec![CatalogEntry {
            id: "Terminal.ClearScreen".to_string(),
            summary: "清屏".to_string(),
            source: "app".to_string(),
        }])
    }
}