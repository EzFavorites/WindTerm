//! 对话框：会话表单 / 设置 / 关于 / SSH / 日志 / 快捷键。
//!
//! 所有弹窗都经 `crate::ui::harness::show_stable_window` 绘制，
//! 保证截图夹具与交互路径共用同一套 sizing / fade_in / 关闭按钮处理。

mod about;
pub(in crate::ui) mod confirm;
// ：粘贴确认弹窗。**不**并进 `confirm`：那个枚举的文案与高度是
// 按参考截图逐像素钉死的，而这里要放一段可滚动的文本预览。
pub(in crate::ui) mod confirm_paste;
mod log;
mod log_settings;
mod log_settings_spec;
mod new_session;
// 会话弹窗的外框（入口分派 / 系统独立窗口 / 页脚按钮）： 从 `new_session.rs`
// 拆出，正文仍在那一个文件里。
mod session_dialog_shell;
mod session_form;
mod session_auth_page;
mod session_form_body;
pub(in crate::ui) mod session_nav;
pub(in crate::ui) mod script_manager;
// 会话页 `OneKey:` 行：候选来自 `prefs.onekeys` + 行内齿轮。
mod session_onekey_row;
mod session_modem_pages;
mod session_pages;
mod session_shell_pages;
mod session_ssh_pages;
mod session_terminal_pages;
mod settings;
// ：独立的快捷命令配置弹窗（命令表不放在 Preferences 那一页）。
mod quick_commands;
mod settings_live;
// ：`settings_live.rs` 已贴着  的代码行上限，Font 与 Tabs 各开一个
// 文件（前者接字号，后者接三项标签行为）。
mod settings_live_binary;
mod settings_live_font;
mod settings_live_search;
mod settings_live_proxy;
mod settings_live_security;
mod settings_live_tabs;
// ：Quick Bar 页三项从灰显占位接成真控件（读写 QuickBarDisplay / Prefs）。
mod settings_live_quickbar;
mod settings_live_completion;
// ：Wrap 页三项接成真控件（wrap symbols / column markers / 列号）。
mod settings_live_wrap;
// ：Highlight 页 caret line 接成真勾选；同日再接 incremental search，
// 其余三行仍灰（caret word / current fold / current pair）。
mod settings_live_highlight;
// ：Caret 页 Period / Width 接成真数字框后，那一页自带写回，
// 从 `settings_live.rs` 拆出来（那个文件已贴着  的 500 代码行上限）。
mod settings_live_caret;
// ：Text 页的「部分真控件」画法（Behaviour / Toolbar / Menu 三页已移除）。
mod settings_live_text;
// ：Terminal 页从数据表改手绘——两个粘贴确认开关接了字段，
// 其余两行由同一份 `draw_row` 画成灰的。
mod settings_live_terminal;
mod settings_live_mouse;
// ： 触发器规则编辑器（事件 → 匹配 → 动作）。这一页是真的接线——
// 编辑结果直接写回 `prefs.triggers`，每帧 `poll` 喂给求值器。
mod settings_live_triggers;
// ：OneKeys 登录方案编辑器（Tool ▸ OneKeys…）。编辑结果写回
// `prefs.onekeys`，连接流程按会话的 `onekey` 宏名消费；凭据走 vault。
// `pub(in crate::ui)`：`OneKeysEdit` 是窗口上的字段类型，`ui/mod.rs` 要声明。
pub(in crate::ui) mod settings_live_onekeys;
mod settings_live_onekeys_modal;
// ：OneKeys 的 Account / Sessions 两页拆出来（ 单文件行数），顺手把
// 它们从 egui 默认控件换成对话框那一套（`dark_field` / `toggle` / `tiny_button`）。
mod settings_live_onekeys_pages;
// ：SSH 页同理拆出（本轮新增 readonly / Modify 编辑态后主文件越线）。
mod settings_live_onekeys_ssh;
// ：Triggers / OneKeys 的**独立对话框宿主**。内容仍由上面两个
// `settings_live_*` 提供（组件夹具也走旧入口），这里只管窗口壳。
pub(in crate::ui) mod dialog_chrome;
mod onekeys_dialog;
mod triggers_dialog;
// ：11 种触发器类型的完整复刻（主窗 + 页面 + 浮层 + 弹窗 + 取色器）。
mod triggers;
// ：Tunnels 设置对话框（视觉基线 `clipboard-…-b2ab71d2.png`）。
// `pub(in crate::ui)`：`Editor` 是窗口上的字段类型，`ui/mod.rs` 要声明它。
pub(in crate::ui) mod tunnel_settings;
mod settings_pages;
mod settings_pages_view;
// ：隐藏末页 `Script & API`——只读状态 + 既有管理入口，不新增任何编辑器。
mod settings_live_script_api;
// ：`shortcuts` 从 `mod` 提到 `pub(in crate::ui)`——两段式键的运行期表
// 由它构造（`runtime_sequence_table`），`construct.rs` 与 `lifecycle_test_support.rs`
// 都要在造窗口时拿到同一份（各自现算会变成两份真相）。
pub(in crate::ui) mod shortcuts;
mod ssh;
// Sysmon 的远端采样要复用这份「SessionConfig + 凭据 → SshOptions」拼装，
// 所以纯逻辑模块对 ui/ 内可见；绘制 / 状态仍在 ssh.rs、ssh_connect.rs。
pub(in crate::ui) mod ssh_ask;
pub(in crate::ui) mod ssh_connect;
pub(in crate::ui) mod vault;
/// ：文件传输的路径输入弹层（发要文件、收要目录）。
pub(in crate::ui) mod xfer;

// `log_settings.rs` 只画，`log_settings_spec.rs` 持有规格 / 状态 / 单测。
// `LogSettingsData` 从后者导出，`ui/mod.rs` 与 `construct.rs` 的引用路径不变。
pub(in crate::ui) use log_settings_spec::LogSettingsData;
// 一次 load-modify-save 的唯一入口（`settings_live::save_prefs`）。
//
// 曾经在这里再导出一份给「设置页之外的落点」用（那时的例子是 `Mode ▸ Hex Edit`
// 写 `session_modes`）。两层模型定下来之后**不存在设置页之外的落点**：
// `View ▸ Hex *` / `Mode ▸ Hex Edit` 改的是 `Tab` 上的内存状态、不落盘，
// 所以那个再导出没了读者，删掉——留着会让人以为菜单那条路也写盘。
//
// D6：**那句话又被打破了，这次是有真读者的**。首次启动引导
// （`ui/overlays/onboarding.rs`）走完 / 跳过时要写 `prefs.onboarded`，它不是
// 设置页上的一项，也不可能塞进设置页（引导要先于任何设置存在）。
//  又添一例：窗口透明度三档预设从菜单落下时也要写
// `prefs.window_transparent` / `window_opacity`（`ui/window_opacity.rs`）。
// 规矩不变：load-modify-save 仍然只有这一个实现，这里只是把它再开出去，
// 而不是在引导 / 菜单落地里另写一遍「读盘 → 改一个字段 → 写盘」。
pub(in crate::ui) use settings_live::save_prefs;
pub(in crate::ui) use session_form::{unique_name, NewSessionData};
// 会话弹窗的三种调用方（新建 / 编辑已有 / 编辑默认模板）。`editing_session`
// 的布尔式判断被它取代：标题 / 按钮 / 保存语义全由 `kind` 分派。
pub(in crate::ui) use new_session::SessionDialogKind;
pub(in crate::ui) use settings::{SettingsData, SettingsPage};
pub(in crate::ui) use ssh_connect::Connector;
