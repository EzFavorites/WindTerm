//! 脚本两窗的组件夹具：`CompId::ScriptRun`（运行窗口）与 `CompId::ScriptEditor`
//! （编辑器）。
//!
//! 为什么单开一个文件：`ui/comp/mod.rs` 的代码行正好卡在 4j 上限（500），
//! 加一个 arm 就 501。与 `reference_popup.rs` / `menu_fixtures.rs` 同一条理由——
//! 夹具也是代码，也得被量。
//!
//! 两份夹具的共同点：**它们都只能从别处打开**（运行窗口来自右键菜单 / 工具栏，
//! 编辑器来自运行窗口的 `Edit`），而截图是一帧定格，点不到那两级入口。不单独
//! 出图，这两个窗口就永远没人看过——D8-3 的「视觉基线待确认」欠的就是这一张。

use crate::ui::comp::CompId;
use crate::ui::*;
use term_script::ScriptDoc;

/// 脚本库里的路径。用**真的**库位置而不是 `/tmp/x.rhai`：基线冻结的是「用户在
/// 自己机器上看到的样子」，路径长得像 `Application Support` 才对得上。
const LIB: &str = "/Users/you/Library/Application Support/WindTerm/scripts";

/// 编辑器夹具里的示例脚本。
///
/// 内容刻意覆盖五种着色：注释、关键字、字符串、数字、原语名。只放一句 `1 + 1`
/// 的话，这条基线上看不出词法着色到底有没有生效——而「着色挂错 token」正是
/// 编辑器最容易犯的错。
const DEMO_SCRIPT: &str = "\
// Wait for the prompt, then greet the remote host.
let host = ask(\"Host to check?\");
send(\"echo \" + host + \"\\r\");
wait(\"$\", 5);
\"ok\"
";

impl TerminalWindow {
    /// 按组件名摆好状态再画。调用方只负责「这是哪一个」。
    pub(in crate::ui) fn draw_script_fixture(&mut self, ctx: &egui::Context, comp: CompId) {
        match comp {
            CompId::ScriptEditor => self.fixture_script_editor(ctx),
            _ => self.fixture_script_run(ctx),
        }
    }

    /// 「运行脚本」窗口：**刚打开**的样子。
    ///
    /// 三份状态都得摆出来，否则基线记下的是一个用户永远看不到的形态：
    /// - `session: Some(7)` 是个占位 id（出图不会点 Run，不会真去池子里查它）；
    ///   不填的话按钮会画成灰的，图上就是一个「看起来坏了」的窗口。
    /// - `library` 三个文件：空清单那张图只能证明「空态长得对」，证明不了行高、
    ///   选中底色和滚动区宽度。
    /// - `path` 落在 `login.rhai` 上：选中行有底色，这是唯一能看出「哪一行被选中」
    ///   的静态证据。
    fn fixture_script_run(&mut self, ctx: &egui::Context) {
        let library = ["deploy.rhai", "health-check.rhai", "login.rhai"]
            .iter()
            .map(|name| (format!("{LIB}/{name}"), (*name).to_string()))
            .collect();
        self.script = Some(terminal::ScriptDialog {
            path: format!("{LIB}/login.rhai"),
            library,
            session: Some(7),
            ..terminal::ScriptDialog::default()
        });
        self.script_recent = vec![
            format!("{LIB}/login.rhai"),
            "/tmp/smoke.rhai".to_string(),
        ];
        self.draw_script_dialog(ctx);
    }

    /// 脚本编辑器：一份**没存过盘**的新脚本（标题是 `Script Editor`）。
    ///
    /// 走 `open_script_editor(None)` 而不是 `open_script_editor(Some(path))`：
    /// 后者会真去读盘，夹具里的路径不存在 → 窗口里先挂一条红错误，基线就冻结
    /// 了一个报错态。内容靠直接替换 `doc` 给，与「用户刚打完字」等价。
    fn fixture_script_editor(&mut self, ctx: &egui::Context) {
        self.open_script_editor(Some(7), None);
        if let Some(state) = self.script_editor.as_mut() {
            state.doc = ScriptDoc::new(DEMO_SCRIPT.to_string());
        }
        self.draw_script_editor(ctx);
    }
}
