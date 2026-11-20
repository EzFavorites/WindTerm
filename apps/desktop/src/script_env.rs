//! 组装一次脚本运行的宿主接缝（[`term_script::HostEnv`]）。
//!
//! # 为什么要一个装配点
//!
//! 两个入口（`desk script run` 与界面里的「运行脚本」）用的是同一套端口、
//! 同一套授权规则，差别只在「谁来实现人机 / 剪贴板 / 窗口」。装配写两遍的结果
//! 一定是「命令行能 `fs_read`、界面里报 Denied」这种只在某一侧出现的怪事。
//!
//! # 授权的两个来源
//!
//! 1. 脚本头：`// @cap fs=read:/tmp`、`// @cap process` 之类（第一段连续注释）。
//! 2. 命令行：`--allow fs=read:/tmp,process`。
//!
//! 两者取并集。**没有第三个来源是「配置文件」**——一次授权只对一次运行有效，
//! 写进全局配置等于永久放行（ADR-0027 D2 的理由）。
//!
//! 用户在文件框里手选的那一条走的是运行中的追加（`pick_file` 之后由界面把
//! 路径塞回信封），不在这里。
//!
//! # 预算
//!
//! `Interactive`（界面）30 秒 / 100 万操作，`Batch`（命令行）10 分钟 / 5000 万。
//! 界面里跑的东西必须能被打断，而命令行里的批处理脚本常常要等远端登录——
//! 同一个上限会让其中一边不可用。

use std::sync::Arc;

use std::sync::Mutex;

use term_core::script_caps::{parse_allow, parse_header, Capabilities, CapRequest};
use term_script::{
    CancelToken, Clipboard, HostEnv, HumanChannel, NoClipboard, NoUi, ScriptBudget, ScriptContext,
    Sessions, Ui,
};

use crate::script_fs::LocalFiles;
use crate::script_proc::LocalProcess;

/// 入口给的东西：源码、参数、`--allow`、预算、终端尺寸。
pub struct RunSpec {
    /// 脚本源码。**要它**是因为授权头得在源码里读（`// @cap …`）。
    pub source: String,
    pub script_path: Option<String>,
    pub args: Vec<String>,
    /// `--allow` 的原始串（`fs=read:/tmp,process`）；空串 = 没给。
    pub allow: String,
    pub budget: ScriptBudget,
    pub cols: usize,
    pub rows: usize,
}

impl RunSpec {
    /// 只给源码的最小形态（单测与嵌入式调用用）。
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            script_path: None,
            args: Vec::new(),
            allow: String::new(),
            budget: ScriptBudget::of(term_script::BudgetKind::Interactive),
            cols: 80,
            rows: 24,
        }
    }

    pub fn with_allow(mut self, allow: impl Into<String>) -> Self {
        self.allow = allow.into();
        self
    }

    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    pub fn with_path(mut self, path: Option<String>) -> Self {
        self.script_path = path;
        self
    }

    pub fn with_budget(mut self, budget: ScriptBudget) -> Self {
        self.budget = budget;
        self
    }

    pub fn with_size(mut self, cols: usize, rows: usize) -> Self {
        self.cols = cols;
        self.rows = rows;
        self
    }

    /// 这次运行的授权集合：脚本头 ∪ `--allow`。
    ///
    /// 解析失败是**硬错误**（不是「忽略那一条」）：一条写错的 `@cap` 被静默
    /// 忽略，脚本后面那次 `fs_write` 会以 Denied 的形式报出来，而作者会以为是
    /// 授权范围写错了——真正的原因（语法）就此埋掉。
    pub fn capabilities(&self) -> Result<Capabilities, String> {
        let mut caps = Capabilities::NONE;
        for request in parse_header(&self.source).map_err(|e| e.0)? {
            caps.grant(&request);
        }
        for request in parse_allow(&self.allow).map_err(|e| e.0)? {
            caps.grant(&request);
        }
        Ok(caps)
    }
}

/// 界面的那套端口。命令行路径传 `None` 就退化成「显式不支持」。
///
/// **不含文件与进程**：那两个永远经授权信封走 [`LocalFiles`] / [`LocalProcess`]，
/// 由 [`build`] 自己装——让调用方各自装一份的结果是「某一侧忘了过授权」。
#[derive(Default)]
pub struct WindowPorts {
    pub clipboard: Option<Arc<dyn Clipboard>>,
    pub ui: Option<Arc<dyn Ui>>,
    /// 应用控制面（ADR-0027 §9）。`None` = 无头 / 没有窗口 → 落到 `NoApp`。
    pub app: Option<Arc<dyn term_script::AppControl>>,
}

/// 授权信封的共享句柄。
///
/// 为什么是共享的：用户在文件框里手选一个路径之后要**当场**把它加进信封
/// （ADR-0027 D2.3），而读信封的两处（工作区 / 文件端口）各有各的时机。
/// 值拷贝会让「刚授的权」在下一句里还看不见。
pub type Envelope = Arc<Mutex<Capabilities>>;

/// 解析授权头 + `--allow`，装进共享信封。
pub fn envelope(spec: &RunSpec) -> Result<Envelope, String> {
    Ok(Arc::new(Mutex::new(spec.capabilities()?)))
}

/// 装配好的一次运行。
pub struct BuiltEnv {
    pub env: HostEnv,
    /// 共享信封（`pick_file` 成功之后往里加根）。
    pub caps: Envelope,
}

/// 当前信封的快照。
///
/// 每次调用现读：脚本运行中授权会变宽（用户手选了文件），拿旧快照就等于
/// 把那次授权吞掉。
pub fn snapshot(caps: &Envelope) -> Capabilities {
    match caps.lock() {
        Ok(guard) => guard.clone(),
        // 中毒只说明别的线程持锁时崩过；信封里的内容还能用。
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// 往信封里加一项授权（`pick_file` 成功之后调）。
pub fn grant(caps: &Envelope, request: &CapRequest) {
    match caps.lock() {
        Ok(mut guard) => guard.grant(request),
        Err(poisoned) => poisoned.into_inner().grant(request),
    }
}

/// 装配一次运行的 [`HostEnv`]。
///
/// 没给的端口一律落到 `term_script` 的「显式不支持」实现上——**绝不静默成功**：
/// 脚本调 `fs_write` 时要么真的写、要么拿到一句说得清原因的 Denied/Unsupported。
pub fn build(
    spec: &RunSpec,
    sessions: Arc<dyn Sessions>,
    human: Arc<dyn HumanChannel>,
    ports: WindowPorts,
    cancel: CancelToken,
    caps: Envelope,
) -> BuiltEnv {
    let context = Arc::new(ScriptContext::new(
        spec.script_path.clone(),
        spec.args.clone(),
    ));
    let mut env = HostEnv::new(sessions)
        .with_human(human)
        .with_caps(snapshot(&caps))
        .with_cancel(cancel)
        .with_context(context)
        .with_size(spec.cols, spec.rows);
    env = env.with_clipboard(ports.clipboard.unwrap_or_else(|| Arc::new(NoClipboard)));
    env = env.with_ui(ports.ui.unwrap_or_else(|| Arc::new(NoUi)));
    env = env.with_app(
        ports
            .app
            .unwrap_or_else(|| Arc::new(term_script::NoApp) as Arc<dyn term_script::AppControl>),
    );

    let files_env = Arc::clone(&caps);
    env = env.with_files(Arc::new(LocalFiles::new(move || snapshot(&files_env))));
    let process_env = Arc::clone(&caps);
    env = env.with_process(Arc::new(LocalProcess::new(move || snapshot(&process_env))));
    BuiltEnv { env, caps }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_declares_capabilities() {
        let spec = RunSpec::new(
            "// @cap fs=read:/tmp\n// @cap process\nprint(1);\n",
        );
        let caps = spec.capabilities().expect("合法");
        assert_eq!(caps.fs.roots(), vec![std::path::PathBuf::from("/tmp")]);
        assert!(!caps.fs.writable(), "只声明了 read");
        assert!(caps.process);
        assert!(!caps.clipboard);
    }

    #[test]
    fn the_command_line_widens_the_header_but_never_narrows_it() {
        let spec = RunSpec::new("// @cap fs=read:/tmp\n1;\n")
            .with_allow("fs=write:/var/tmp,clipboard");
        let caps = spec.capabilities().expect("合法");
        assert_eq!(caps.fs.roots().len(), 2, "读根 ∪ 写根");
        assert!(caps.fs.writable(), "写授权生效");
        assert!(caps.clipboard);
        // 根的集合按「声明顺序」：头里的 /tmp 先、命令行里的 /var/tmp 后。
        assert_eq!(
            caps.fs.roots(),
            vec![
                std::path::PathBuf::from("/tmp"),
                std::path::PathBuf::from("/var/tmp")
            ]
        );
    }

    /// 一条写错的 `@cap` 是硬错误——静默忽略会让后面的 Denied 变成假线索。
    #[test]
    fn a_malformed_header_is_an_error_not_a_silent_skip() {
        let spec = RunSpec::new("// @cap fs=bogus:/tmp\n1;\n");
        let error = spec.capabilities().expect_err("该报错");
        assert!(error.contains("bogus") && error.contains("第 1 行"), "{error}");
    }

    #[test]
    fn without_any_declaration_everything_is_denied() {
        let caps = RunSpec::new("1;\n").capabilities().expect("合法");
        assert_eq!(caps, Capabilities::NONE);
    }

    /// 装配出来的环境：没给的端口是显式的「不支持」，不是静默成功。
    #[test]
    fn unconnected_ports_refuse_explicitly() {
        let sessions: Arc<dyn Sessions> = Arc::new(crate::script_host::PooledSessions::new(
            Arc::new(term_session::SessionPool::new()),
        ));
        let spec = RunSpec::new("1;\n");
        let caps = envelope(&spec).expect("授权头合法");
        let env = build(
            &spec,
            sessions,
            Arc::new(term_script::HeadlessHuman),
            WindowPorts::default(),
            CancelToken::new(),
            caps,
        )
        .env;
        assert!(env.files.read("/etc/passwd").is_err(), "文件端口必须拒");
        assert!(env.process.run(&term_script::ProcRun {
            program: "/bin/echo".to_string(),
            args: Vec::new(),
            cwd: None,
            timeout: None,
            stdin: None,
        }).is_err());
        assert!(env.clipboard.get().is_err());
        assert!(env.ui.selection(1).is_err());
    }
}
