//! 会话日志的投影：执行日志动作 + 读出写入端状态。
//!
//! 这一层只做三件事：把 `Effect::Log` 落到 `SupervisorHandle` 上、用系统默认程序
//! 打开文件 / 目录、把状态交给状态栏与配置页。日志的**写入**在监管者线程里
//! （`term-runtime::log`），这里连字节都看不到——它只读 `handle.log` 那份状态。
//!
//! `Session ▸ Log ▸ Settings...` 打开的 **Session Settings ▸ Logging 配置页**
//! 在隔壁 `log_settings.rs`（2026-11-08 视觉基线重做）；旧版那个「状态三行 +
//! 两排按钮」的 Session Log 面板随之退场，运行时状态改由状态栏 `log_label()`
//! 与配置页底部的条件状态块呈现，Start / Stop / Pause / Resume 仍在 Log 菜单。
//!
//! 一个刻意的取舍：**暂停 / 恢复不从界面"猜"状态**。菜单项是否可点由
//! `LogStatus.phase` 决定，而那份状态是写入端报上来的（文件真的开了才算开始）。
//! 界面自己记一个 bool 会在「磁盘满、写入失败」时显示成正在记录。

use crate::ui::*;
use term_runtime::log::{LogPhase, LogStatus};

impl TerminalWindow {
    /// 执行一条日志动作。错误写进 `log_note`（对话框与状态栏都看它）。
    pub(in crate::ui) fn apply_log(&mut self, action: LogAction) {
        let outcome = self.run_log_action(action);
        match outcome {
            Ok(note) => self.log_note = note,
            Err(note) => self.log_note = Some(note),
        }
    }

    /// 返回 `Some(..)` = 需要显示的提示（成功路径也可能要，比如「没有日志文件」）。
    fn run_log_action(&self, action: LogAction) -> Result<Option<String>, String> {
        let tab = self.tabs.get(self.active_slot()).ok_or("没有活动会话")?;
        if !tab.is_live() {
            return Err("会话已断开，无法记录日志".to_string());
        }

        match action {
            LogAction::Start => {
                let dir =
                    term_runtime::log::default_dir().ok_or("找不到配置目录，日志无处可放")?;
                // 会话名进文件名，也进文件头。名字里有中文也照样能用——
                // 文件名里那部分会折成 `session`，真名记在头里。
                //
                // 配置页那五个字段（Log types / 覆盖追加 / 文件名模板 /
                // 大小上限 / 内容模板）在这里**真的被消费**：写入端按它们
                // 落盘。设置页因此不再是「改了没效果」。
                tab.log_start(dir, &tab.config.name, &self.log_settings.options());
                Ok(None)
            }
            LogAction::Stop => {
                tab.log_stop();
                Ok(None)
            }
            LogAction::Pause => {
                tab.log_pause();
                Ok(None)
            }
            LogAction::Resume => {
                tab.log_resume();
                Ok(None)
            }
            LogAction::OpenFile => {
                let path = tab
                    .log_status()
                    .path
                    .ok_or("这条会话还没有日志文件（先 Start Logger）")?;
                if !path.is_file() {
                    return Err(format!("日志文件不见了：{}", path.display()));
                }
                crate::sysopen::open(&path).map_err(|e| format!("打不开日志文件：{e}"))?;
                Ok(None)
            }
            LogAction::OpenFolder => {
                let dir =
                    term_runtime::log::default_dir().ok_or("找不到配置目录，日志无处可放")?;
                // 目录可能还不存在（从没记过日志）：先建出来再打开，
                // 否则用户点「Open Log Folder」只会得到一声没反应的沉默。
                std::fs::create_dir_all(&dir).map_err(|e| format!("建不了日志目录：{e}"))?;
                crate::sysopen::open(&dir).map_err(|e| format!("打不开日志目录：{e}"))?;
                Ok(None)
            }
            // 对话框开关由 reducer 写进 `ViewState`，这里没有额外副作用。
            LogAction::OpenSettings => Ok(None),
        }
    }

    /// Log 子菜单的可用性由**写入端状态**决定，不是能力台账。
    /// 视觉基线出厂态：`Start...` / `Open Log Folder` / `Settings...` 可用，
    /// `Stop` / `Pause` / `Resume` / `Open Log File` 全灰。能力台账回答
    /// 「这条命令接没接」，这里回答「此刻点了有没有意义」——两者是 AND。
    pub(in crate::ui) fn log_command_enabled(&self, id: &str) -> bool {
        let status = self
            .tabs
            .get(self.active_slot())
            .map(|t| t.log_status())
            .unwrap_or_default();
        // 命令 ID → 语义动作**只经** `term_app::log::parse`（ID 字符串不许
        // 出现在 `ui/` 里）。`None` = 这条 ID 不是日志命令，本谓词管不着，
        // 交给能力台账去判，所以这里返回 `true`（即「不因日志状态而灰」）。
        match term_app::log::parse(id) {
            None => true,
            Some(action) => log_action_enabled(action, &status),
        }
    }

    /// `Log types` 的**触发面**：会话刚连上时按设置自动开一份日志。
    ///
    /// 只认 `Tab::poll` 报的「刚连上」边沿，认完即清。三条边界：
    ///
    /// - **不能拿 `phase == Off` 当判据**：用户手动 Stop 之后那一条每帧都是
    ///   `Off`，会自动重开，界面与自己打架；
    /// - `Log nothing` 时边沿照样要**取走**（否则之后打开设置会捡到一条老边沿，
    ///   给一条早就连上的会话补开日志）；
    /// - 底带 Shell 面板里那些会话**不自动记**：它们是工具条会话，不是用户的
    ///   工作会话，边沿没人取，天然不会开。
    pub(in crate::ui) fn poll_auto_log(&mut self) {
        let opts = self.log_settings.options();
        let dir = if opts.log_type.enabled() {
            term_runtime::log::default_dir()
        } else {
            None
        };
        for i in 0..self.tabs.len() {
            if !self.tabs[i].take_just_connected() {
                continue;
            }
            let Some(dir) = dir.clone() else { continue };
            let tab = &self.tabs[i];
            tab.log_start(dir, &tab.config.name, &opts);
        }
    }

    /// 状态栏上的日志标记。没在记录时返回 `None`——空转的「Logging: off」
    /// 只会占位置（不留没有信息量的开关）。
    pub(in crate::ui) fn log_label(&self) -> Option<String> {
        let tab = self.tabs.get(self.active_slot())?;
        if !tab.is_live() {
            return None;
        }
        let status = tab.log_status();
        match status.phase {
            LogPhase::Off => None,
            LogPhase::Recording => Some("Logging".to_string()),
            LogPhase::Paused => Some("Logging (paused)".to_string()),
            LogPhase::Failed(e) => Some(format!("Log failed: {e}")),
        }
    }

}

/// 配置页底部的条件状态块要读这两个纯函数（`dialogs::log_settings`）。
pub(in crate::ui) fn phase_text(phase: &LogPhase) -> String {
    match phase {
        LogPhase::Off => "off".to_string(),
        LogPhase::Recording => "recording".to_string(),
        LogPhase::Paused => "paused".to_string(),
        LogPhase::Failed(e) => format!("failed — {e}"),
    }
}

/// 一条日志动作此刻能不能点——**只看写入端状态**，能力台账是另一层（见
/// `log_command_enabled` 的注释）。
///
/// 抽成自由函数是为了能单测：判据全在 `(LogAction, LogStatus)` 这张表里，
/// 不需要真的起一条会话。
pub(in crate::ui) fn log_action_enabled(action: LogAction, status: &LogStatus) -> bool {
    match action {
        // 只有「没在写」时才允许开始。Recording / Paused 下再 Start 会另开一个
        // 文件，前一段日志就此断在半路（不是关闭，是不再被追加）。
        LogAction::Start => matches!(status.phase, LogPhase::Off | LogPhase::Failed(_)),
        // Recording 与 Paused 都算「在写」：暂停只是不落盘，文件还开着。
        LogAction::Stop => status.is_recording(),
        LogAction::Pause => status.phase == LogPhase::Recording,
        LogAction::Resume => status.phase == LogPhase::Paused,
        // 没有真文件就不许点：点了只能报错，等于把失败当交互。
        LogAction::OpenFile => status.path.as_ref().map(|p| p.is_file()).unwrap_or(false),
        // 目录可能还不存在（`OpenFolder` 自己会建），设置页任何时候都能开。
        LogAction::OpenFolder | LogAction::OpenSettings => true,
    }
}

/// 日志文件大小的可读形式。日志动辄几百 KB，显示位数太多没有意义。
pub(in crate::ui) fn format_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.1} KB", n as f64 / KB as f64)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_sizes_read_like_sizes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn every_phase_has_a_distinct_label() {
        assert_eq!(phase_text(&LogPhase::Off), "off");
        assert_eq!(phase_text(&LogPhase::Recording), "recording");
        assert_eq!(phase_text(&LogPhase::Paused), "paused");
        // 失败要把原因带出来：「日志写不进去」时只说 failed 等于没说。
        assert_eq!(phase_text(&LogPhase::Failed("磁盘满".into())), "failed — 磁盘满");
    }

    fn status(phase: LogPhase, path: Option<&str>) -> LogStatus {
        LogStatus {
            phase,
            path: path.map(std::path::PathBuf::from),
        }
    }

    #[test]
    fn stop_pause_resume_follow_the_writer_state_not_the_ui() {
        // 出厂态：只有 Start 亮着。
        let off = status(LogPhase::Off, None);
        assert!(log_action_enabled(LogAction::Start, &off));
        assert!(!log_action_enabled(LogAction::Stop, &off));
        assert!(!log_action_enabled(LogAction::Pause, &off));
        assert!(!log_action_enabled(LogAction::Resume, &off));

        let recording = status(LogPhase::Recording, Some("/tmp/x.log"));
        assert!(!log_action_enabled(LogAction::Start, &recording), "已在记录时再 Start 会另开文件");
        assert!(log_action_enabled(LogAction::Stop, &recording));
        assert!(log_action_enabled(LogAction::Pause, &recording));
        assert!(!log_action_enabled(LogAction::Resume, &recording));

        let paused = status(LogPhase::Paused, Some("/tmp/x.log"));
        assert!(!log_action_enabled(LogAction::Start, &paused));
        // 暂停时文件仍开着，所以 Stop 仍然有意义。
        assert!(log_action_enabled(LogAction::Stop, &paused));
        assert!(!log_action_enabled(LogAction::Pause, &paused));
        assert!(log_action_enabled(LogAction::Resume, &paused));
    }

    #[test]
    fn failed_writing_allows_a_restart_but_keeps_stop_reachable() {
        // 磁盘满导致的失败：允许重新开始（换目录 / 腾空间后），
        // 但此时已经不在写，所以 Stop 是灰的——两个判断来自同一份状态。
        let failed = status(LogPhase::Failed("磁盘满".into()), None);
        assert!(log_action_enabled(LogAction::Start, &failed));
        assert!(!log_action_enabled(LogAction::Stop, &failed));
        assert!(!log_action_enabled(LogAction::Pause, &failed));
    }

    #[test]
    fn open_file_needs_a_file_that_is_really_there() {
        // 路径存在但文件不在 → 灰。否则点下去只能弹一条错误。
        let missing = status(LogPhase::Recording, Some("/nonexistent/zzz-windterm.log"));
        assert!(!log_action_enabled(LogAction::OpenFile, &missing));
        let none = status(LogPhase::Off, None);
        assert!(!log_action_enabled(LogAction::OpenFile, &none));
        // 目录与设置页任何时候都能开：前者自己会 create_dir_all。
        assert!(log_action_enabled(LogAction::OpenFolder, &none));
        assert!(log_action_enabled(LogAction::OpenSettings, &none));
    }
}
