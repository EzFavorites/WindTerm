//! 三条交互运行入口共享的**一次性**运行配置（`RunConfig { grants, batch }`）。
//!
//! # 这是我们的设计，不是兼容终端的功能
//!
//! SecureCRT 既没有前 / 后台模式，也没有「本次运行授权」这一层。这里的两项
//! 都是本项目为自动化安全额外加的，注释里不声称与它等价：
//!
//! - `grants`：面板里手勾的额外能力。原先只有停靠面板会把它拼成 `--allow`，
//!   运行对话框与编辑器拿不到——同一份脚本从不同入口跑，权限却不一样。
//! - `batch`：原先只活在运行对话框的 `Long-running` 勾选里，面板与编辑器
//!   永远只能跑交互档。
//!
//! # 为什么挂在 `TerminalWindow` 而不是对话框状态上
//!
//! 三扇窗（运行对话框 / 停靠面板 / 编辑器）各有各的生命周期，谁都不该独占
//! 「这次给不给权限、跑哪一档预算」。放窗口上做**内存里的中转站**，三处
//! 都能读同一份；不写 prefs —— 一次勾选就只在这一程里有效，重启不带过去。
//! 更不写回脚本头：`// @cap` 是永久授权，手勾是临时授权，混在一起就等于
//! 把后门钉进文件里。
//!
//! 故意**不**在这里包 `script_run_options()`：登录脚本与触发器的后台运行
//! 也走那个构造器，它们既不该继承手勾授权，也不该继承批量档。两项只在
//! 三处显式 Run 的调用点贴上（见 `terminal/script.rs` / `panels/script_panel.rs`
//! / `overlays/script_editor.rs`）。

/// 面板上勾出来的额外授权。
#[derive(Default)]
pub(in crate::ui) struct Grants {
    pub process: bool,
    pub clipboard: bool,
    pub ui: bool,
    /// 文件根目录（空 = 不放行文件）。手打而不是选目录：面板窄，选目录要另开
    /// 一个模态框，而这里填的多半是 `/tmp` 这种一眼能打完的路径。
    pub dir: String,
    /// 给了目录之后，是只读还是可写。**独立一档**：能读不代表能写。
    pub write: bool,
}

impl Grants {
    /// 拼成 `--allow` 那一套语法（`fs=read:/a`、`fs=write:/a`、`process,ui`）。
    ///
    /// 拼出来再交给 `parse_allow` 解析，而不是直接构造 `Capabilities`：解析器
    /// 是唯一的事实源，两条路各拼一份迟早对不上（比如这里忘了规范化路径）。
    fn spec(&self) -> String {
        let mut items: Vec<String> = Vec::new();
        if self.process {
            items.push("process".to_string());
        }
        if self.clipboard {
            items.push("clipboard".to_string());
        }
        if self.ui {
            items.push("ui".to_string());
        }
        let dir = self.dir.trim();
        if !dir.is_empty() {
            let mode = if self.write { "write" } else { "read" };
            items.push(format!("fs={mode}:{dir}"));
        }
        items.join(",")
    }
}

/// 三条交互入口共用的一次性运行配置。只活在内存在，绝不落盘。
#[derive(Default)]
pub(in crate::ui) struct RunConfig {
    pub grants: Grants,
    /// 交互档 / 批量档。`false` = 30s / 100 万次；`true` = 10min / 5000 万次。
    pub batch: bool,
}

impl RunConfig {
    /// 本次运行的预算档位。唯一换算点，三处 Run 都用它。
    pub(in crate::ui) fn budget(&self) -> term_script::ScriptBudget {
        term_script::ScriptBudget::of(if self.batch {
            term_script::BudgetKind::Batch
        } else {
            term_script::BudgetKind::Interactive
        })
    }

    /// 本次手勾的额外授权（空串 = 没有）。
    pub(in crate::ui) fn allow(&self) -> String {
        self.grants.spec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_permission_checkboxes_turn_into_an_allow_spec() {
        // 面板手勾的每一项都要能在 `--allow` 语法里找到对应写法，否则「勾了没用」。
        let mut cfg = RunConfig::default();
        assert_eq!(cfg.allow(), "", "什么都没勾就什么都不放行");

        cfg.grants.process = true;
        cfg.grants.ui = true;
        assert_eq!(cfg.allow(), "process,ui");

        cfg.grants.dir = "  /tmp  ".to_string();
        assert_eq!(cfg.allow(), "process,ui,fs=read:/tmp", "只读是缺省档");

        cfg.grants.write = true;
        assert_eq!(cfg.allow(), "process,ui,fs=write:/tmp");

        // 拼出来的字符串必须真的能被解析器吃下去（两条路各拼一份是这类 bug 的温床）。
        let parsed = term_core::script_caps::parse_allow(&cfg.allow()).expect("应当能解析");
        assert_eq!(parsed.len(), 3);
    }
}
