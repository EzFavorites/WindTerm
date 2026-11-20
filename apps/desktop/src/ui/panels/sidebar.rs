//! 侧栏：会话列表、分组、筛选、导入导出。
//!
//! 从 `ui/mod.rs` 拆出（2026-11-05）。**只搬代码，不改行为**——
//! 五张视觉基线逐字节一致是这次拆分唯一的判据。
//!
//! 2026-11-07：绘制部分再拆一次到 `sidebar_view.rs`（会话树对齐 WindTerm 视觉基线
//! 时新增了标题带 / 彩色协议图标 / 分组文件夹等一批绘图代码，`sidebar.rs` 会
//! 越过 450 行上限）。本文件只留**数据与落盘**：导入解析、排序、持久化、
//! 连接与断开；「画」全部在 `sidebar_view.rs`。

use crate::ui::*;

/// 这个会话该不该走进程内的 SSH 客户端。
///
/// 两个条件缺一不可：协议是 SSH，且会话上勾了「内置客户端」。老会话读出来是
/// `false`（字段不存在），所以升级不会悄悄换掉任何人在用的 SSH 实现。
pub(in crate::ui) fn uses_builtin_ssh(config: &SessionConfig) -> bool {
    config.protocol == Protocol::Ssh && config.builtin_ssh
}

/// 导入的**文本来源**由投影层决定（读哪个文件是 IO），解析全在
/// `term_runtime::import_sessions` 的纯函数里。这里只负责「找到文件」。
fn import_text(source: &str) -> Vec<String> {
    let home = dirs::home_dir();
    match source {
        "OpenSSH" => home
            .map(|h| h.join(".ssh").join("config"))
            .and_then(|p| std::fs::read_to_string(p).ok())
            .into_iter()
            .collect(),
        "WindTerm" => {
            // `profiles/` 目录里每份 `.wind` / `.json` 是一个会话导出。
            let mut out = Vec::new();
            let roots: Vec<std::path::PathBuf> = [
                home.as_ref().map(|h| h.join(".windterm").join("profiles")),
                home.as_ref().map(|h| h.join("Library/Application Support/WindTerm/profiles")),
                home.as_ref().map(|h| h.join(".config/WindTerm/profiles")),
            ]
            .into_iter()
            .flatten()
            .collect();
            for root in roots {
                let Ok(entries) = std::fs::read_dir(&root) else { continue };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str())
                        .is_some_and(|e| e == "wind" || e == "json")
                    {
                        if let Ok(raw) = std::fs::read_to_string(&path) {
                            out.push(raw);
                        }
                    }
                }
            }
            out
        }
        "PuTTY" => {
            // macOS 上的 PuTTY 会话落在 ~/.putty/sessions/<name>；Windows 注册表
            // 导出不在本机可读范围内（读不到就返回空，不伪造）。
            let mut out = Vec::new();
            if let Some(dir) = home.map(|h| h.join(".putty").join("sessions")) {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        if let Ok(raw) = std::fs::read_to_string(entry.path()) {
                            out.push(format!("[{}]\n{}", entry.file_name().to_string_lossy(), raw));
                        }
                    }
                }
            }
            out
        }
        "Shell Sessions" => std::fs::read_to_string("/etc/shells").ok().into_iter().collect(),
        _ => Vec::new(),
    }
}

impl TerminalWindow {
    /// SessionManager::saveUserSessions — flush the session
    /// list to `<config>/WindTerm/sessions.json` so a restart restores the tree.
    pub(in crate::ui) fn save_sessions(&self) {
        term_runtime::persist::save(&self.groups, &self.sessions);
    }

    /// `SessionPane::duplicate` finishes with `sortByColumn(0, Ascending)`: the
    /// tree is always alphabetical by label inside its group.
    pub(in crate::ui) fn sort_sessions(&mut self) {
        let keep = self
            .sessions
            .get(self.selected_session)
            .map(|s| (s.group.clone(), s.name.clone()));
        self.sessions.sort_by(|a, b| {
            a.group
                .to_lowercase()
                .cmp(&b.group.to_lowercase())
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.selected_session = match keep {
            Some((group, name)) => self
                .sessions
                .iter()
                .position(|s| s.group == group && s.name == name)
                .unwrap_or(0),
            None => 0,
        };
    }

    /// 导入外部会话。四个来源都走真解析器，PuTTY / Shell Sessions
    /// 不再是空壳；WindTerm `profiles/` 也接上了。密码永不迁移。
    pub(in crate::ui) fn import_sessions(&mut self, source: &str) {
        let texts = import_text(source);
        let mut found: Vec<term_runtime::import_sessions::ImportedSession> = Vec::new();
        for text in &texts {
            let mut batch = match source {
                "OpenSSH" => term_runtime::import_sessions::parse_ssh_config(text),
                "WindTerm" => term_runtime::import_sessions::parse_windterm(text),
                "PuTTY" => term_runtime::import_sessions::parse_putty(text),
                "Shell Sessions" => {
                    let shell = std::env::var("SHELL").unwrap_or_default();
                    term_runtime::import_sessions::parse_shell_sessions(text, &shell)
                }
                _ => Vec::new(),
            };
            found.append(&mut batch);
        }

        let mut added = 0;
        for item in found {
            if self.sessions.iter().any(|s| s.name == item.name) {
                continue;
            }
            let group = item.group.clone();
            if !self.groups.contains(&group) {
                self.groups.push(group);
            }
            self.sessions.push(item.into_config());
            added += 1;
        }
        self.sort_sessions();
        self.save_sessions();
        // 导入结果要看得见：侧栏没有别的地方放一次性提示，状态栏的
        // log_note 是既有的一次性反馈通道（不新增空转开关）。
        self.log_note = Some(format!("Imported {source}: {added} session(s)"));
    }

    pub(in crate::ui) fn connect_selected_session(&mut self) {
        let Some(config) = self.sessions.get(self.selected_session).cloned() else {
            return;
        };
        // Check if a tab for this config already exists
        if let Some(idx) = self.tabs.iter().position(|t| t.config.name == config.name) {
            self.set_active_slot(idx);
            return;
        }
        // 标了 builtin_ssh 的 SSH 会话走进程内客户端。
        //
        // 放在这里而不是 `make_tab` 里面：`make_tab` 是同步的（拉起 PTY 是毫秒级），
        // 而 SSH 握手可能等十几秒，必须先起后台线程、再等结果回来才建 tab。
        // 从侧栏点「连接」时手上一份密码也没有，所以这一步通常是「弹密码框」。
        if uses_builtin_ssh(&config) {
            self.begin_builtin_ssh(config, None);
            return;
        }
        let idx = self.tabs.len();
        let tab = make_tab(
            &self.pool,
            config,
            egui::Id::new(format!("tab_{idx}")),
            &self.ctx,
        );
        let idx = self.add_tab(tab);
        // `Behaviour ▸ Focus the new tab`：关掉时新标签照常建出来，但不抢焦点。
        self.activate_new_tab(idx);
    }

    pub(in crate::ui) fn disconnect_active_tab(&mut self) {
        let active = self.active_slot();
        let Some(tab) = self.tabs.get_mut(active) else {
            return;
        };
        let name = tab.config.name.clone();
        // 关会话必须经控制面：句柄现在在池里，丢一个 `SessionId` 什么都不会发生。
        tab.close();
        // 隧道跟着会话走——终端断了，转发没有理由还留着。
        // 否则用户看到的是「一个没人能关的端口还开着」。
        self.close_tunnels_of(&name);
        // 文件面板比它的会话活得久没有意义（那条 SFTP 连接也连着同一台机器）。
        self.close_filer_of(&name);
    }
}
