//! 意图分发：把命令 ID / 强类型 `Action` 翻译成对 `ViewState` 的纯改，并把
//! 产生的 `Effect` 落到各处副作用（见 `effects.rs`）。
//!
//! 从 `mod.rs` 拆出来是为了把「装配（draw 顺序 + 字段）」与「分发（Action →
//! reduce → Effect）」两件事分开，让 `mod.rs` 回到单文件 ≤ 500 行的约束内。
//! 本文件不持有字段、不画任何东西——只调用 `reduce` 与 `apply_effect`（后者在
//! `effects.rs` 的同一类型 impl 上）。

use serde_json::Value;
use eframe::egui;
use term_app::action::{reduce, Action};
use term_app::shell::ShellAction;
use term_core::automation::Priority;

use super::TerminalWindow;
use crate::bridge::surface::{arbiter_session, shared_arbiter, shared_arbiter_arc, DeskSurface};
use crate::ui::terminal::trigger_dispatch::{self, CommandCall};

impl TerminalWindow {
    /// 把 `full_screen` / `always_on_top` 落到窗口系统上。
    ///
    /// 只在值变化时发命令——`applied_window` 记「已经发过什么」。
    /// 每帧无条件发同一个请求会让部分平台反复调整窗口（抖动），也让
    /// 「空闲 0 CPU」变成空话。
    pub(super) fn sync_window_modes(&mut self, ctx: &egui::Context) {
        if self.applied_window.full_screen != Some(self.view.full_screen) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.view.full_screen));
            self.applied_window.full_screen = Some(self.view.full_screen);
        }
        if self.applied_window.always_on_top != Some(self.view.always_on_top) {
            let level = if self.view.always_on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
            self.applied_window.always_on_top = Some(self.view.always_on_top);
        }
    }

    /// 命令 ID 在这里被翻译成 `Action`，之后全链路强类型。
    ///
    /// 未接线的 ID 解析为 `None`：菜单已把它画成置灰，这里再兜一层，
    /// 任何来源的未知命令都不会被当成「执行成功但什么也没发生」。
    pub(super) fn dispatch(&mut self, action_id: &str) {
        // ADR-0027：`Script.RunFile:<路径>` 是映射键与按钮栏跑脚本的入口。
        // 它**不是** `Action` 的变体——路径是任意串，做成枚举会让 `Action` 认识
        // 文件系统。所以在这里先看一眼前缀（命令 ID 的字面量在 `script_run.rs`，
        // 门禁要求 `ui/` 下不出现 ID 字面量）。
        if let Some(path) = crate::script_run::run_file_target(action_id) {
            self.open_script_for_path_background(path);
            return;
        }
        if let Some(action) = Action::parse(action_id) {
            self.dispatch_action(action);
            return;
        }
        // 只有自由文本才是命令行。形状像命令 ID 却没登记的一律显式报错——
        // 旧实现把任何未知 ID 都丢给 /bin/sh，拼错的菜单 ID 会变成一次
        // 静默（甚至成功）的 shell 执行，用户完全看不出差别。
        match trigger_dispatch::classify_target(action_id) {
            trigger_dispatch::DispatchTarget::Command => {
                if let Err(why) = trigger_dispatch::spawn_command(action_id) {
                    self.commit_note(Some(why));
                }
            }
            trigger_dispatch::DispatchTarget::Action | trigger_dispatch::DispatchTarget::Unknown => {
                self.commit_note(Some(format!(
                    "未知命令 ID {action_id:?}：未登记的动作，不会当作 shell 执行"
                )));
            }
        }
    }

    /// 菜单项携带完整 `Invocation`：有参数且 method 是会话动词时，经仲裁器
    /// 与 `Surface::invoke` 落到活动会话；纯命令 ID 仍走原来的分发器。
    pub(super) fn dispatch_call(&mut self, call: &CommandCall) {
        let has_params = call
            .params
            .as_object()
            .is_some_and(|fields| !fields.is_empty());
        let remotely_callable = term_core::surface::find(&call.method)
            .is_some_and(|spec| spec.exposure.on_rpc());
        if !has_params || !remotely_callable {
            // `app_*` 这类 UI 控制动词由 `Action` 通道实现（对 API 不可见），
            // 不能误送进会话控制面；参数仍完整保留在 CommandCall 里。
            self.dispatch(&call.method);
            return;
        }
        let Some((id, name)) = self.tabs.get(self.active_slot()).map(|tab| {
            (tab.session.unwrap_or(tab.identity), tab.config.name.clone())
        }) else {
            self.commit_note(Some(format!("菜单调用 {:?} 需要一个活会话", call.method)));
            return;
        };
        let key = arbiter_session(&self.pool, id);
        let source = format!("menu:{}", call.method);
        let grant = {
            let mut arbiter = shared_arbiter().lock().unwrap_or_else(|e| e.into_inner());
            arbiter.request(key, &source, Priority::P2Explicit, false)
        };
        let grant = match grant {
            Ok(grant) => grant,
            Err(denied) => {
                self.commit_note(Some(denied.as_message()));
                return;
            }
        };
        let mut params = call.params.clone();
        if !params.get("session").is_some_and(Value::is_string) {
            params["session"] = Value::String(name);
        }
        // 这里**不再**在锁外做一次 is_valid 预检：那和 PTY 写入之间仍有
        // 窗口（P0 按键 / 独占抢占正好插在中间）。唯一的判据落在
        // `DeskSurface::send_granted` 的同一把 arbiter 锁里，失效就不写。
        let surface = DeskSurface::with_arbiter(self.pool.clone(), shared_arbiter_arc());
        let result = surface.invoke_granted(&call.method, &params, &grant);
        let mut arbiter = shared_arbiter().lock().unwrap_or_else(|e| e.into_inner());
        arbiter.release(key, &source);
        drop(arbiter);
        if let Err(err) = result {
            self.commit_note(Some(err.message));
        }
    }

    /// 已经有 `Action` 的入口（对话框里的按钮）。
    ///
    /// 为什么不给按钮也传命令 ID：命令 ID 只出现在 `menu.rs` 与各能力的
    /// `parse()`。对话框按钮本来就持有强类型动作，绕回字符串只会多一层错配机会。
    pub(super) fn dispatch_action(&mut self, action: Action) {
        // 同步输入的通道成员关系需要会话身份，reducer 手上没有；在这里补齐。
        // 总开关仍由 reducer 翻转，这里只决定「哪条标签加入/离开哪条通道」。
        let toggle_sync = matches!(action, Action::Shell(ShellAction::SyncInput));
        // `Action::AddressBar` 是**纯投影层**的意图（打开地址栏浮层），
        // 它在 `ViewState` 里没有对应的领域状态——地址栏开着还是关着是控件
        // 状态，和 `find_input` / `palette_input` 同一层。所以这里消费掉它，
        // 不往 `reduce` 里塞一个 reducer 管不了的变体。
        //
        // 这是唯一能拦住它的地方：命令 ID 在 `term_app::action::Action::parse`
        // 里就已经落成强类型变体，投影层不该再有第二条字符串分支（也查这条
        // ——命令 ID 字面量出现在 ui/ 里会被 verify.sh 判红）。
        if matches!(action, Action::AddressBar) {
            // 专注模式只留菜单栏：Alt+D 等地址栏入口在这里就被拦下，
            // `AddressState` 不会被置开，自然也没有浮层要画。
            if !self.view.focus_mode {
                self.open_address_bar("", None);
            }
            return;
        }
        // 命令面板里的「已登记脚本」与映射键 / 按钮栏共用这条 Action：
        // 这里拿到的是解析后的路径，不再做第二次字符串前缀判断。
        if let Action::RunScript(path) = action {
            self.open_script_for_path_background(&path);
            return;
        }
        // reducer 拿到的输入是「当前事实窗口」，与渲染同源——
        // 菜单里点「查找下一个」和屏幕上看到的那一屏是同一个坐标系。
        let window = self.tabs.get(self.active_slot()).map(|t| &t.view);
        let effects = reduce(&mut self.view, action, window);
        if toggle_sync {
            let ids: Vec<_> = self.tabs.iter().map(|t| t.identity).collect();
            if self.view.sync_input {
                // 开启：所有当前活着的标签加入通道 0，等价于旧「写给其它所有标签」；
                // 之后用户可以按会话把某些成员移出，这就是 4 通道模型的价值。
                for id in ids {
                    term_app::sync_input::join(&mut self.view, 0, id);
                }
            } else {
                for id in ids {
                    term_app::sync_input::leave_all(&mut self.view, id);
                }
            }
        }
        for effect in effects {
            self.apply_effect(effect);
        }
    }
}
