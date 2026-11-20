//! 组件 Harness：把「独立渲染 + 可复现截图」的时序知识收成基础设施。
//!
//! 为什么单开一个文件：弹窗截图曾经连错 4 轮（方案文档 §1.1 的复盘）——
//! 先怀疑夹具，再怀疑样式，再怀疑 `fade_in`，最后才定位到 `sizing_pass`。
//! 那四轮的补丁是**手写在每个弹窗里**的，所以下一个弹窗还会再踩一遍。
//! 这里的三个函数就是那四轮的全部结论，业务组件从此不该再手写其中任何一行。
//!
//! # 三条结论（写死在代码里，不再靠记性）
//!
//! 1. **首次 show 是 sizing pass**：`Area::begin` 在 `state.is_none()` 时把所有
//!    形状换成 `Shape::Noop`，只为量尺寸。要一份逐字节可复现的图，必须先
//!    「空内容热身」一次把 `AreaState` 播种出来，再画真内容。
//! 2. **拍图时机由我们自己定**（`apps/desktop/src/shot.rs`，走
//!    `ViewportCommand::Screenshot` → `Event::Screenshot`）。eframe 自带的
//!    `EFRAME_SCREENSHOT_TO` 有两个坑：读屏在 `swap_buffers` 之后（拿到的
//!    是上一帧），以及一次 `request_discard` 就能让它存出一张全透明图。
//!    结论对这里的影响只有一条：**首帧的画面不作数**，热身仍必须做。
//! 3. **热身内容不能真的是空的尺寸**：空内容会播种一个极小的 size，真窗口
//!    首帧被压扁、右侧被裁没。所以热身里要 `set_min_size` 一个显式尺寸。
//!
//! # 边界
//!
//! 只做「怎么把东西稳定画出来」，不认识任何业务状态：没有 `TerminalWindow`，
//! 没有 `ViewState`，没有副作用。所以它能被组件夹具直接调用（P2）。

use eframe::egui;

/// 夹具模式：截图 / 组件出图时，布局必须一次到位，不能有动画与诊断噪音。
///
/// 判据只有环境变量：`WINDTERM_SCREENSHOT_TO`（`--screenshot`，见
/// `apps/desktop/src/shot.rs`）或 `WINDTERM_COMP_DEMO`（组件出图，见 [`show_canvas`]）。
pub(in crate::ui) fn fixture_mode() -> bool {
    fixture_flag(
        std::env::var_os("WINDTERM_SCREENSHOT_TO").is_some(),
        std::env::var_os("WINDTERM_COMP_DEMO").is_some(),
    )
}

/// 这个进程是不是 `Application.NewWindow` 拉起来的**第二个应用实例**。
///
/// 与 [`fixture_mode`] 同源（都是只看环境变量的纯判据，拿不到 argv）：新进程由
/// `new_window.rs` 打上 `WINDTERM_SECONDARY_WINDOW`。`construct.rs::init` 据此
/// **跳过 `restore_layout()`**——`layout.json` 是单份全局状态，照常恢复会让新窗口
/// 长成旧窗口的副本，那就不是「独立」了。跳过之后 `TerminalWindow::new` 本来
/// 就先建一条 live 本地 shell 标签，干净的出厂态自然成立。
///
/// 只判「在不在」，不判值：这个变量的唯一用途就是当角色标记，多一份解析就多
/// 一处能和 `new_window.rs` 说岔的地方。
pub fn secondary_window() -> bool {
    std::env::var_os("WINDTERM_SECONDARY_WINDOW").is_some()
}

/// [`fixture_mode`] 的纯函数形态：把两个环境变量读成布尔再合起来。
///
/// 单独留一个纯函数是为了能测——直接读环境变量进单测，红了只能怪「这台机器
/// 上碰巧 export 了什么」，那种测试等于没有。
fn fixture_flag(screenshot_to: bool, comp_demo: bool) -> bool {
    screenshot_to || comp_demo
}

/// L1 崩溃防护的**故障注入**：`WINDTERM_PANIC_FRAME=<n>` 让第 n 帧的 `draw`
/// 在画完提示条之后 panic 一次。
///
/// 为什么要这一格：L1 的承诺是「一帧崩了进程还在、会话还在」，这条承诺**只能**
/// 在真进程里验——状态机有单测，但「eframe 主循环会不会跟着退出」单测覆盖不到。
/// 注入点刻意放在 `draw` 的**第一行之后**：提示条已经画出去了，panic 之后的
/// 那一半不再执行，正好复现真实崩溃的形状。
///
/// 只在 debug 构建里存在，且必须显式给环境变量；生产路径零影响。
pub(in crate::ui) fn maybe_panic_on_frame() {
    #[cfg(debug_assertions)]
    {
        use std::sync::atomic::{AtomicU64, Ordering};
        static FRAME: AtomicU64 = AtomicU64::new(0);
        let Ok(spec) = std::env::var("WINDTERM_PANIC_FRAME") else {
            return;
        };
        let Ok(target) = spec.parse::<u64>() else {
            return;
        };
        // 计数放在解析成功之后：不注入的进程连一次自增都没有。
        let n = FRAME.fetch_add(1, Ordering::Relaxed) + 1;
        assert!(n != target, "注入的界面帧 panic（WINDTERM_PANIC_FRAME={target}）");
    }
}

/// 进入夹具模式时要关掉的诊断噪音，以及必须压掉的「多 pass 丢弃」。
///
/// 只关一样东西：热身窗口与真窗口复用同一批内部 widget id（同一个窗口 id 才能
/// 继承 sizing state），debug 构建默认打开的 `Options::warn_on_id_clash` 会把整张
/// 截图盖满红字。Release 构建本来就不警告，交互路径不受影响。
///
/// **不要在这里改 `max_passes`。** 试过：压成 1 能让 `Grid` 的首帧不再触发
/// 丢弃，看似「更稳」，但它同时改掉了 egui 的布局收敛方式，而且没有解决问题
/// ——真正的问题是拍图时机（见模块注释第 2 条），换由 `shot.rs` 处置之后，
/// 这里一行都不需要动。
pub(in crate::ui) fn quiet_diagnostics(ctx: &egui::Context) {
    if fixture_mode() {
        ctx.options_mut(|o| o.warn_on_id_clash = false);
    }
}

/// 夹具模式下把 `InputState::time` 钉在 0：所有「按时间动」的东西（spinner 的
/// 转角、光标闪烁、动画插值）都变成静态，否则同一份代码两次运行的字节不一样，
/// 逐字节基线永远红。
///
/// 实测触发点：`dialog_ssh_connecting` 里的 `ui.spinner()`——它按 `i.time`
/// 算转角，两次运行差几十毫秒，图就不同。冻时间比「夹具里把 spinner 换成静态
/// 标签」诚实：屏幕上画的还是那个 spinner，只是它不动了。
///
/// 每帧都要调（eframe 每帧重填 `RawInput::time`），所以放在 `draw()` 开头。
pub(in crate::ui) fn freeze_clock(ctx: &egui::Context) {
    if fixture_mode() {
        ctx.input_mut(|i| i.time = 0.0);
    }
}

/// 夹具模式下**假装窗口是聚焦的**（每帧都要调，与 `freeze_clock` 同处）。
///
/// 为什么需要：截图运行从 2026-11-09 起用 macOS 的
/// `ActivationPolicy::Prohibited` 建进程（见 `main.rs`），所以窗口**真的**
/// 永远不会被激活——这正是「不抢前台」的实现方式。副作用是 egui 会把
/// 「窗口未聚焦」如实画出来：命令面板与 Goto 这两个带输入框的浮层，提示文字
/// 会从 `#BBBBBB` 掉到 `#444444`，两张组件基线当场红。
///
/// 取舍：基线要录的是**用户真看到的样子**（用户打开面板时窗口是聚焦的），
/// 而不是截图机制的技术副作用。所以这里把 `InputState::focused` 钉成 `true`，
/// 让画面回到「有人在前台用它」的状态。这与 `freeze_clock` 同一条诚实原则：
/// 屏幕上画的还是那个控件，只是它对外部状态的依赖被固定了。
pub(in crate::ui) fn pretend_focused(ctx: &egui::Context) {
    if fixture_mode() {
        ctx.input_mut(|i| i.focused = true);
    }
}

/// 组件画布的底色：面板底 `panel_bg`（与 `DESIGN_TOKENS` 同源，不另写魔数）。
pub(in crate::ui) fn canvas_bg() -> egui::Color32 {
    let t = &term_core::theme::DESIGN_TOKENS;
    crate::theme::to_color32(term_core::theme::rgb(t.panel_bg))
}

/// 一块只有底色的画布，`--demo comp:<name>` 的舞台。
///
/// 组件之外的一切（菜单栏、会话树、终端网格）都是噪音：出图要能一眼看出
/// 「这个组件长什么样」，也要能逐字节比对——底色是固定的，不依赖终端内容。
pub(in crate::ui) fn show_canvas(ctx: &egui::Context, add: impl FnOnce(&mut egui::Ui)) {
    egui::CentralPanel::default()
        .frame(egui::Frame::none().fill(canvas_bg()))
        .show(ctx, |ui| add(ui));
}

/// P9：**横栏组件**的单独舞台。整宽、定高，钉在画布左上角。
///
/// 弹窗类组件用窗口、浮层类组件用锚定窗口，而状态栏 / 快捷命令栏是「一条
/// 贴着窗口边缘的整宽横栏」——放进弹窗里就不再是那个组件了。这里用固定
/// 位置的 `Area` 而不是 `TopBottomPanel`：面板声明顺序依赖整窗的几何契约，
/// 组件图不该把「谁先声明」也冻结进基线。宽高都写死，逐字节可复现。
pub(in crate::ui) fn show_bar_canvas(
    ctx: &egui::Context,
    height: f32,
    add: impl FnOnce(&mut egui::Ui),
) {
    let width = ctx.screen_rect().width() - 16.0;
    egui::Area::new(egui::Id::new("component_bar_canvas"))
        .fixed_pos(egui::pos2(0.0, 0.0))
        .movable(false)
        // 与 `tool_bar.rs` 的 pane_toolbar 同一个坑：Area 默认 fade_in=true，
        // 而夹具把 `i.time` 钉在 0，整条横栏永远停在淡入第一帧（实测文字
        // 只剩约 30% 亮度）。关掉淡入，交互路径只是少一个 100ms 动画。
        .fade_in(false)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(canvas_bg())
                .inner_margin(egui::Margin::symmetric(8.0, 3.0))
                .show(ui, |ui| {
                    ui.set_width(width);
                    ui.set_min_height(height - 6.0);
                    add(ui);
                });
        });
}

/// 锚定在画布中央、**定宽不定高**的浮层。命令面板 / Goto / 命令中心 / 隧道 /
/// tmux 五个用它。
///
/// 对话框（固定尺寸、居中）走 `kit::window::DialogWindow`；这里只留「内容多高
/// 就多高」的浮层。它们的确定性来自——`shot.rs` 要的是
/// 布局稳定后的第 3 帧，而内容（空查询、空历史）在夹具里是固定的，
/// 所以自适应高度本身就是确定的。固定尺寸只适用于**本来就有确定尺寸**的对话框。
///
/// 仍然保留的两条纪律：稳定 id（标题会随页签换）与 `fade_in(false)`。
pub(in crate::ui) struct AnchoredWindow<'a> {
    id: &'a str,
    title: String,
    width: f32,
    /// 内容超出时的滚动上限。给 `f32::INFINITY` 表示不限。
    max_height: f32,
    close_button: bool,
}

impl<'a> AnchoredWindow<'a> {
    pub(in crate::ui) fn new(id: &'a str, title: impl Into<String>, width: f32) -> Self {
        Self {
            id,
            title: title.into(),
            width,
            max_height: f32::INFINITY,
            close_button: false,
        }
    }

    /// 画右上角 ×（返回 `false` = 被点掉了）。默认不画：命令面板这类
    /// 「Esc 就走」的浮层多一个 × 只是多一处要维护的关闭路径。
    pub(in crate::ui) fn with_close_button(mut self) -> Self {
        self.close_button = true;
        self
    }

    /// 限高。超出的内容进滚动区——不限制的话，命令中心的长列表会把窗口
    /// 顶到屏幕外。
    pub(in crate::ui) fn with_max_height(mut self, max_height: f32) -> Self {
        self.max_height = max_height;
        self
    }

    pub(in crate::ui) fn show(
        self,
        ctx: &egui::Context,
        mut add: impl FnMut(&mut egui::Ui),
    ) -> bool {
        let mut open = true;
        // 同 StableWindow：window_fill 让给了菜单弹层，窗口显式回填 panel_bg。
        let mut frame = egui::Frame::window(&ctx.style().clone());
        frame.fill = canvas_bg();
        let mut window = egui::Window::new(self.title.clone())
            .id(egui::Id::new(self.id))
            .collapsible(false)
            .resizable(false)
            .fade_in(false)
            .frame(frame)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO);
        if self.close_button {
            window = window.open(&mut open);
        }
        window.show(ctx, |ui| {
            // 宽度两头都钉：只给 min 时，一句长提示会把窗口撑宽，
            // 同一份代码两次出图就可能差几个像素。
            ui.set_min_width(self.width);
            ui.set_max_width(self.width);
            let mut body = |ui: &mut egui::Ui| {
                ui.set_min_width(self.width);
                ui.set_max_width(self.width);
                add(ui);
            };
            if self.max_height.is_finite() {
                egui::ScrollArea::vertical()
                    .max_height(self.max_height)
                    // 竖向必须允许收缩：`[false, false]` 会让视口吃掉整个
                    // `max_height`，内容只有三行也留一大片空白（实测命令中心
                    // 底部空了半个窗口）。横向不收缩是因为宽度已经被钉死。
                    .auto_shrink([false, true])
                    .show(ui, body);
            } else {
                body(ui);
            }
        });
        open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_colour_comes_from_the_design_tokens() {
        // 底色不许是随手写的魔数：它必须等于 `panel_bg`（#1C1C1C），
        // 否则组件图的背景与整窗图对不上，「同内容」就无从谈起。
        let t = &term_core::theme::DESIGN_TOKENS;
        assert_eq!(canvas_bg(), crate::theme::to_color32(term_core::theme::rgb(t.panel_bg)));
        assert_eq!(t.panel_bg, 0x1C1C1C);
    }

    #[test]
    fn anchored_window_defaults_to_no_close_button_and_no_height_cap() {
        // 默认值就是「命令面板那种浮层」：Esc 关闭、高度跟着内容。
        // 谁改了默认值，五个浮层会一起变形。
        let w = AnchoredWindow::new("x", "T", 400.0);
        assert!(!w.close_button);
        assert!(!w.max_height.is_finite());
    }

    #[test]
    fn anchored_window_options_are_opt_in() {
        let w = AnchoredWindow::new("x", "T", 400.0)
            .with_close_button()
            .with_max_height(300.0);
        assert!(w.close_button);
        assert_eq!(w.max_height, 300.0);
    }

    #[test]
    fn fixture_flag_needs_one_of_the_two_channels() {
        // 两个通道任一存在即夹具模式；都没有就是常规交互路径——这正是
        // 「热身只该出现在取证时」的那条线。
        assert!(!fixture_flag(false, false));
        assert!(fixture_flag(true, false));
        assert!(fixture_flag(false, true));
        assert!(fixture_flag(true, true));
    }
}
