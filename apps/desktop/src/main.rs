//! WindTerm: a standalone terminal built on egui.
//!
//! A standalone terminal emulator built on egui. It keeps the parts the
//! original exhibits, and reworks
//! the rest around egui instead of Qt. The window chrome, the session tree,
//! the tab strip and the status bar all come from the same layout the
//! original draws: a left tree, a tab strip above the terminal, a vertical
//! strip of tool buttons on the right, and a status bar at the bottom.

// 单可执行文件 + 一批 v0.2 预留 API（会话管理、剪贴板读写、脏区方法、
// 配置常量、Action 变体、Palette 元信息）。binary crate 的 dead_code lint
// 会把这些全部报出来；逐项 allow 会淹没代码，因此在此统一允许，边界改由
// scripts/verify.sh 的 grep 断言 + 单测 + 金样本守住。
#![allow(dead_code)]

// ADR-0014 / 0016：core / pty / persist / session 在 term-core 与 term-runtime，
// 动作层在 term-app，本 crate 只保留投影层（ui / fonts / clipboard / theme 适配）。
mod bridge;
mod cli;
mod clipboard;
mod demo;
mod fonts;
// 原始字节 tee 的消费端（`Receiver` 这类资源留在 app 层，界面只拿句柄）。
mod hex_tap;
mod native_picker;
mod shot;
mod script_app;
mod script_clip;
mod script_env;
mod script_fs;
mod script_host;
mod script_human;
mod python_run;
mod script_proc;
mod script_recorder;
mod script_run;
mod script_output;
mod script_sftp;
// 控制面开会话的唯一入口（腿 A 脚本 / 腿 B 桥接共用）：内置 SSH 的通道在这层
// 注入，控制面本身不认识协议（ADR-0019）。
mod session_open;
mod sysopen;
// Sysmon 读数条的采样宿主：系统命令在 ui/ 之外跑，界面只读纯值。
mod sysmon_sample;
// 远端 Sysmon：独立 exec 通道取远端文本，复用同一套纯解析器；不碰用户 PTY。
mod sysmon_remote;
// 把文件交给系统默认程序（预览窗的「用外部程序打开」）与远端预览的
// 临时落点。放在 `ui/` 之外是要求：拉起进程不是画。
mod open_with;
// `Application.NewWindow`：再起一个完整应用实例（新进程）。与 `open_with.rs`
// 同一层——拉起进程不是画，门禁只扫 `ui/`，所以放这里。
mod new_window;
mod theme;
mod ui;
// 文件传输驱动（XMODEM / YMODEM 跑在后台线程上，界面只持不透明句柄）。
mod xfer_conflict;
mod xfer_run;
// 目录同步的组装层（两端清单 → `term_xfer::sync` 的计划）。放这里而不是
// 任何一个 crate 里：`term-xfer` 零依赖、`term-sftp` 不认识 `SyncOp`，映射只能在
// 认识两边的这一层做。纯逻辑，不碰 IO 也不画。
mod sync_job;
#[cfg(test)]
mod golden; // 金样本回归集，只在 cargo test 下编译

/// 进程级 `WINDTERM_PROFILE_DIR` 的全局锁。
///
/// 所有「设 → 读/写 → 还原」的用例都必须持它：环境变量是全进程共享的，
/// `with_temp_profile`（UI 夹具）和 `script_host` 等直改变量的用例若各持一把锁，
/// 并发时仍会互相看见对方的档案目录，表现为布局落盘断言偶发红。
#[cfg(test)]
pub(crate) static PROFILE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

use eframe::egui;

/// `--screenshot <file.png>`: start normally, draw enough frames to settle the
/// layout, then save the window's picture to that PNG and exit. A build
/// without the option, or with a blank picture, does not pass.
///
/// 截图由我们自己的 `shot::Shot` 完成（command → event 两步），不走 eframe 的
/// `EFRAME_SCREENSHOT_TO`：那条路读的是上一帧、而且一次 pass 丢弃就能存出
/// 一张全透明图。两条实测证据见 `shot.rs` 的模块注释。
/// 环境变量 `WINDTERM_SCREENSHOT_TO` 只是给 `ui` 层当夹具判据用（它拿不到 argv）。
fn main() {
    // 启动期一次性整理（先于任何读配置的动作）：把旧版平铺在平台配置目录根下的
    // 数据复制进 `profile_dir()` 这个子目录，并清掉被强杀留下的原子写临时文件。
    // 两条都只做加法与「删自己写坏的东西」，且 `WINDTERM_PROFILE_DIR` 在场时整体
    // 跳过（测试/截图/便携模式不该碰真实目录）。
    let tidy = term_paths::startup_tidy();
    if tidy.migrated > 0 {
        eprintln!(
            "已把 {} 个旧配置项复制到 {}（旧文件原地保留）",
            tidy.migrated,
            term_paths::profile_dir().map(|p| p.display().to_string()).unwrap_or_default()
        );
    }
    if tidy.cleaned > 0 {
        eprintln!("清掉 {} 个上次异常退出留下的临时文件", tidy.cleaned);
    }
    let args: Vec<String> = std::env::args().collect();
    // ADR-0020：`doctor` / `capability` 是无窗口子命令，必须在 `eframe` 起来
    // 之前处理掉。放在这里而不是解析完 flags 之后，是为了让它们连显示环境都
    // 不需要——CI 与排障现场的第一条命令就是 `term-desk doctor`。
    if let Some(code) = cli::try_run(&args) {
        std::process::exit(code);
    }
    let mut headless_exec: Option<String> = None;
    let mut demo_spec: Option<demo::DemoSpec> = None;
    let mut comp_demo: Option<String> = None;
    let mut shot_path: Option<std::path::PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--screenshot" && i + 1 < args.len() {
            // 只设我们自己的变量：eframe 那条（`EFRAME_SCREENSHOT_TO`）会在
            // 第 2 个 pass 抢着存图并退出，两个机制同时在场必然打架。
            std::env::set_var("WINDTERM_SCREENSHOT_TO", &args[i + 1]);
            shot_path = Some(std::path::PathBuf::from(&args[i + 1]));
            i += 2;
        } else if args[i] == "--list-components" {
            // 给 `scripts/screenshot-baseline.sh --components` 取名单用：
            // 组件清单只有 `ui::comp` 一份，脚本不另抄。
            for (name, baseline) in ui::comp_list() {
                println!("{name}\t{}", if baseline { "gated" } else { "wip" });
            }
            return;
        } else if args[i] == "--headless" {
            i += 1;
        } else if args[i] == "--exec" && i + 1 < args.len() {
            headless_exec = Some(args[i + 1].clone());
            i += 2;
        } else if args[i] == "--demo" && i + 1 < args.len() {
            let value = args[i + 1].clone();
            // 两种夹具共用一个开关：`--demo <整窗夹具>` 与 `--demo comp:<组件>`。
            // 后者只画一个组件（「独立渲染验证」），前者画整窗。
            if let Some(name) = value.strip_prefix("comp:") {
                comp_demo = Some(name.to_string());
                // 夹具模式也得从环境变量看得出来：harness 与状态栏时钟都只看
                // 这一个判据，不看命令行（它们拿不到 argv）。
                std::env::set_var("WINDTERM_COMP_DEMO", name);
            } else {
                // 视觉基线夹具：静态画面，不依赖 PTY 输出时序。
                match demo::DemoSpec::parse(&value) {
                    Some(spec) => demo_spec = Some(spec),
                    None => {
                        eprintln!(
                            "unknown --demo value: {value}\n  \
                             整窗夹具: scroll | find | word | block | margin | dock | newsession | glyph\n  \
                             组件夹具: comp:<{}>",
                            ui::comp_names().join(" | ")
                        );
                        std::process::exit(2);
                    }
                }
            }
            i += 2;
        } else {
            i += 1;
        }
    }

    // 端到端验收入口：不起窗口，跑一条命令并打印最终网格文本。
    //
    // 退出码必须透传（ADR-0020：这是原 `term-cli local exec` 的能力之一，
    // 删掉 CLI 不等于这条能力可以消失）。状态走 stderr，stdout 只留屏幕文本，
    // 这样 `term-desk --headless --exec … | grep` 不会被状态行污染。
    if let Some(command) = headless_exec {
        match term_runtime::supervisor::run_headless(&command, 80, 24) {
            Ok((text, code)) => {
                print!("{text}");
                // `process::exit` 不走析构，stdout 的缓冲要自己冲掉。
                use std::io::Write;
                let _ = std::io::stdout().flush();
                let code = match code {
                    Some(c) => c,
                    // 没拿到退出码 = 命令没在超时内退出（`top` 之类）。如实说，
                    // 但不把它伪装成失败：交互式命令本身不是错误。
                    None => {
                        eprintln!("[{command}] 未在 5s 内退出，已断开（没有退出码）");
                        0
                    }
                };
                std::process::exit(code);
            }
            Err(e) => {
                eprintln!("headless failed: {e}");
                std::process::exit(1);
            }
        }
    }

    // 默认窗口的逻辑尺寸对齐 WindTerm 视觉基线主窗口（1280×760 @2×
    // 之前用框架默认的 800×600：底部快捷命令栏 4 组 15 项有一半被裁在
    // 窗口之外——不是画错了，是画布根本不够宽。改尺寸之后整窗与视觉基线同尺，
    // 基线整窗与本窗口同尺，无需再为对齐做任何缩放。
    let mut options = eframe::NativeOptions::default();
    // 窗口透明度（默认关，现状不变）。透明开关为真时让 OS 窗口背景透明——
    // eframe 0.29 经 `NativeOptions.viewport.transparent` 转 winit `with_transparency`
    // （`glow_integration.rs:926` / `wgpu_integration.rs:194` 已确认跨平台走通）。
    // 这是**建窗时**的那一半；运行期切换由 `ViewportCommand::Transparent(bool)`
    // 在 `ui/window_opacity.rs::sync_window_opacity` 里只在值变化时补发一次
    // （egui-winit 0.29 已实现 `window.set_transparent`；X11 只能在建窗时设，
    // 该平台下运行期切换需重启，需在 GUI 验收时肉眼确认）。
    // 数值型不透明度 egui 0.29 没有对应视口命令，由 `update()` 里把钳制后的
    // alpha 装到 `egui::Visuals` 背景色与终端区底色上（见 `ui/window_opacity.rs`）。
    let prefs_for_window = term_runtime::prefs::load().unwrap_or_default();
    options.viewport = options
        .viewport
        .with_inner_size(egui::vec2(1280.0, 760.0))
        .with_transparent(prefs_for_window.window_transparent);
    // 窗口图标。`include_bytes!` 而不是运行时读：发布产物是**单可执行文件**
    // （ADR-0020 把可执行目标收敛成一个），旁边不该再散落一个 png 才有图标。
    // 1024×1024 的 PNG 压进二进制，换来的是「拷走一个 exe 就能用」。
    if let Some(icon) = app_icon() {
        options.viewport = options.viewport.with_icon(std::sync::Arc::new(icon));
    }
    // 截图运行（`--screenshot`）必须是**完全无感**的：不显示、不激活、不抢前台。
    //
    // 三个开关缺一不可，这是 2026-11-09 实测踩出来的（`lsappinfo front` 采样：
    // 只加前两个时，80 次采样里仍有 22 次前台变成了 `term-desk`）：
    //
    // 1. `with_visible(false)`：窗口**不显示**。
    // 2. `with_active(false)`：建窗口时不要成为 key window（winit 的
    //    `WindowAttributes::with_active`）。
    // 3. macOS 的**激活策略改成 `Prohibited`**：这一条才是决定性的。前两条只管
    //    「窗口」这个对象，而抢焦点是 `NSApplication` 层的事——只要进程还是个
    //    正常 app，AppKit 就会在创建窗口时把自己 activate 起来。`Prohibited`
    //    让这个进程根本不具备成为前台应用的资格（也不出现 Dock 图标）。
    //
    // 手动 `--demo <夹具>`（不带 `--screenshot`）仍然可见可用，那是人在看。
    if shot_path.is_some() {
        options.viewport = options.viewport.with_visible(false).with_active(false);
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            options.event_loop_builder = Some(Box::new(|builder| {
                builder.with_activation_policy(ActivationPolicy::Prohibited);
            }));
        }
    }

    if let Err(e) = eframe::run_native(
        "WindTerm",
        options,
        Box::new(move |cc| {
            Ok(Box::new(WindTermApp::new(cc, demo_spec, comp_demo, shot_path)))
        }),
    ) {
        eprintln!("eframe failed: {e}");
        std::process::exit(1);
    }
}

/// 应用图标：把编译期嵌进来的 PNG 解成 egui 要的 RGBA。
///
/// 解码失败返回 `None` 而不是 panic——图标是「皮肤」级的东西，显示不出来不该
/// 让整个终端起不来。真正的图标缺陷由 `assets/icon.png` 的重新生成解决，
/// 不该在启动路径上以崩溃的形式暴露。
fn app_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../../../assets/icon.png");
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = (img.width(), img.height());
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

struct WindTermApp {
    ui: ui::TerminalWindow,
    /// `Some` = 这次运行是来拍一张图的（见 `shot.rs`）。
    shot: Option<shot::Shot>,
}

impl WindTermApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        demo: Option<demo::DemoSpec>,
        comp_demo: Option<String>,
        shot: Option<std::path::PathBuf>,
    ) -> Self {
        let is_shot = shot.is_some();
        let prefs = term_runtime::prefs::load().unwrap_or_default();
        let mut app = Self {
            ui: ui::TerminalWindow::new(cc),
            shot: shot.map(shot::Shot::new),
        };
        app.ui.init(cc);
        // 把磁盘上的命令行历史接进窗口，按 `prefs` 的容量 / TTL 收敛一遍
        // （超出上限或过期的条目在读入时就裁掉，不必等下一次写入）。
        //
        // 拍图运行不接：基线要可复现，而 `command_history.json` 每台机器都不同。
        if !is_shot {
            if let Some(mut history) = term_runtime::persist::load_command_history() {
                history.reconfigure(
                    prefs.history_capacity(),
                    prefs.history_ttl_days(),
                    now_secs(),
                );
                app.ui.adopt_command_history(history);
            }
        }
        // 任何 `--screenshot` 都把文件面板钉在一组固定条目上：真实家目录每台
        // 机器都不同，逐字节基线会永远红。`screenshot-round64.png` 那张「贴底
        // 状态」根本不带 `--demo`，所以注入不能只挂在 `set_demo` 上（2026-11-08
        // 实测：不钉就是 8c 视觉回归红）。
        if is_shot {
            app.ui.use_demo_files();
        }
        if let Some(spec) = demo {
            app.ui.set_demo(spec);
        }
        // 组件夹具在 `init` 之后设置：`init` 会 restore_layout，但组件出图
        // 不画布局树，顺序上先后无关——放在后面是为了让「窗口先能跑起来」
        // 这件事不被一个拼错的名字挡住。
        if let Some(name) = comp_demo {
            if !app.ui.set_comp_demo(&name) {
                eprintln!(
                    "unknown --demo comp: {name}\n  可用组件: {}",
                    ui::comp_names().join(" | ")
                );
                std::process::exit(2);
            }
        }
        app
    }
}

impl eframe::App for WindTermApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 按钳制后的 `window_opacity` 让 egui 背景 alpha 生效。只在值变化时
        // 应用（`applied_window` 模式），`window_transparent=false` 时整段是
        // no-op——所有既有视觉基线一个字节都不变。放在 `draw` 之前：
        // `draw` 里的 `apply_ui_style` 每帧重建 `Visuals`，会把当前 alpha
        // 读回来原样装回去，所以两处顺序不会互相抹掉。
        self.ui.sync_window_opacity(ctx);
        // Sysmon 展开面板的采样线程只起一次：top / vm_stat 在 ui/ 之外跑，
        // 面板只读纯值快照（夹具模式下也起线程，但面板不展开、不影响基线）。
        sysmon_sample::ensure_started();
        // （`docs/design/10-crash-hardening.md`）：**界面单帧 panic
        // 不带走进程**。egui 主循环里一次 `unwrap` 炸掉，在此之前是「窗口直接消失，
        // 十条会话连同未保存的输出一起没」。现在它只丢这一帧。
        //
        // `AssertUnwindSafe` 是这里唯一的取舍点（设计稿 §5 开放问题 1）：包住之后
        // 上下文可能停在一个不一致的中间态——某一处 `take()` 了没放回去、某个控件
        // 的 `Response` 没被读。**宁可错帧也不死进程**：错帧下一帧自己会重算，
        // 而进程死了用户的会话是真的没了。提示条会把次数与原因说清楚，用户点 ×
        // 或重启都有据可依。
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.ui.draw(ctx)
        }));
        match outcome {
            Ok(()) => self.ui.note_frame_ok(),
            Err(payload) => {
                let reason = term_io::panic_message(&*payload);
                eprintln!("ui frame panicked（已跳过该帧，进程继续）: {reason}");
                self.ui.note_ui_panic(ctx, &reason);
            }
        }
        // 拍图放在 `draw` 之后：要拍的正是这一帧画出来的东西。
        if let Some(shot) = &mut self.shot {
            shot.poll(ctx);
        }
    }

    /// 退出时把本次运行积累的命令行历史写回磁盘，并收口所有会话。
    ///
    /// 三条边界：
    /// - 拍图运行不写——那是一次「只该出现一帧」的运行，不该改动用户的 profile；
    ///   会话也不必在这里收口，进程随即退出，会话随进程一起走。
    /// - **退出时重新读一次 prefs**，而不是用启动快照：设置页的「保存命令行
    ///   历史」开关现在真的会写 `prefs.json`，用户完全可能「打开设置关掉它、
    ///   再关窗口」——读快照会让这条路径失效，文件照样被写。
    /// - 落盘失败不拦退出（`save_command_history` 返回 `false` 就算了）。
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.shot.is_some() {
            return;
        }
        // 关会话放在读 prefs **之前**，理由只有两条，都不是风格问题：
        //
        // 1. 下面读 prefs 有一条 `return`——文件在但读不出来时按「不写」处理。
        //    关会话若放在它后面，一次坏掉的 `prefs.json` 就会让本次运行的所有
        //    会话失去收口机会。放前面则任何「非拍图」的退出路径都必然走到。
        // 2. 两个方向都没有数据依赖，所以顺序完全由「谁会被早退跳过」决定：
        //    `Tab::close` 只把 `Command::Close` 入队就返回（不等监管者 join，
        //    也不 flush 屏幕），`save_command_history` 读的是命令历史、与标签无关。
        //
        // 用 `close_all_sessions()` 而不是 `close_all_tabs()`：后者末尾会调
        // `ensure_tab()` 现起一条本地 shell，退出时没有意义。退一步说，即使将来
        // 有人换回 `close_all_tabs()`，`Drop for Tab` 也会把那条补出来的会话一起
        // 收口，不会泄漏——两层都堵上了。
        self.ui.close_all_sessions();
        // 给这次运行盖一个「正常退出」的戳。理由与做法都见
        // `TerminalWindow::mark_clean_exit`——**只改元信息，不重写布局**：上面刚
        // 把所有会话收口，此刻的 `SavedLayout` 是一份空布局。
        self.ui.mark_clean_exit();
        let save = match term_runtime::prefs::load() {
            Some(prefs) => prefs.command_history_save,
            // 文件在、但读不出来（坏了 / 权限）：不知道用户选了什么，按「不写」
            // 处理——宁可丢一次历史，也不要在用户明确关掉之后还写盘。
            None if term_runtime::prefs::exists() => return,
            // 从没写过 `prefs.json`：用户没有机会表达「关掉」，按出厂态。
            // 这与设置页上那个出厂就是勾上的框是同一条事实（`Prefs::default`）。
            None => term_runtime::prefs::Prefs::default().command_history_save,
        };
        if save {
            // 保险库已解锁时历史作为加密条目落盘（明文文件随之删除）；
            // 没有主密码模型可用时才回落明文文件——那是没有保险库的用户。
            if !self.ui.save_history_to_vault() {
                term_runtime::persist::save_command_history(self.ui.command_history());
            }
        }
    }
}

/// 当前 Unix 秒。墙钟只在宿主（本 crate）里读，`term-core` 保持纯语义
/// （ADR-0014：core 零墙钟，时间一律由宿主注入）。
///
/// `pub(crate)`：设置页改历史容量 / TTL 时要用同一个口径（`ui::dialogs::
/// settings_live` 调 `reconfigure`），不要再抄第三份 `SystemTime::now()`。
pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
