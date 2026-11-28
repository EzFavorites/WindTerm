//! `term-desk script run <file>` —— 腿 A 脚本引擎的**第一个消费者**（ADR-0019 S-a）。
//!
//! 为什么这条命令值得存在：`term-script` 从写出来那天起就只有测试在调它。
//! 一个只有测试调用的库，它的 API 是被测试塑形的，不是被用法塑形的——
//! 于是「脚本能跑」这句话一直没有无人值守的证据。这里把它接到 `desk` 上，
//! 一次跑通就同时钉住三件事：控制面能开会话、控制面能借出会话、引擎能吃文件。
//!
//! 会话从哪来：本命令是**独立进程**，不共享 GUI 的会话池（ADR-0022
//! 记的同一个限制）。所以它自己开一个：
//!
//! - 默认开本地 `$SHELL`（脚本里 `send("ls\n")` 那种交互式用法）
//! - `--exec <cmd>` 改开一次性命令（CI 与排障现场：`--exec 'echo hi'`）
//! - `--session <名字>` 改开会话库里那条会话——包括**内置 SSH**（ADR-0019）。
//!   的开法只有一条：`crate::session_open::open_session`，与桥接共用。
//!
//! 密码认证的会话在这里打不开：密码在保险库里，而本命令是无窗口的、从不解锁
//! 保险库。遇到那一档就明说「去解锁」或「改用 Agent/私钥」，退出码 1——
//! **不**偷偷换成系统 `ssh`（那就是行为分叉）。
//!
//! 输出与退出码：脚本最后一个表达式的值打到 stdout（`--quiet` 关掉）；
//! 脚本失败 / 会话起不来 → 1，参数 / 文件错误 → 2。**不**把失败折成 0——自动化里
//! 「跑挂了但退出码是 0」比跑挂本身更贵。

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use term_core::session::SessionConfig;
use term_core::session_runtime::SessionId;
use term_runtime::persist;
use term_script::{BudgetKind, CancelToken, ScriptBudget, ScriptEngine, ScriptWorkspace};
use term_session::SessionPool;

use crate::script_env::{self, RunSpec, WindowPorts};
use crate::script_host::PooledSessions;
use crate::script_human::StdinHuman;
use crate::session_open::{open_session, OpenFailure};

const USAGE: &str = "\
term-desk script run <file.rhai> [--exec CMD | --session NAME] [--cols N] [--rows N]
                                  [--timeout SECS] [--quiet] [--batch]
                                  [--arg VALUE]... [--allow SPEC]

  在独立会话里跑一段 Rhai 脚本（腿 A 引擎）。原语是全局函数：
    write(text) / keys([key...]) / keys(key)
    wait(pat, secs) / wait_any([pat...], secs[, filter]) / wait_idle(quiet_s[, secs])
    read_until(delim, secs[, filter]) / read([secs[, filter]]) / screen()
    cursor_pos() -> #{row, col} / terminal_size() -> #{cols, rows}
    state() -> #{alive, exit} / facets() -> [name...]
    log_start(dir) / log_stop() / log_status() -> #{phase, path, reason}
    set_synchronous(on) / notify(msg) / ask(prompt)
    grep(pat) / grep_v(pat)
    session_current() -> name / session_list() -> #{sessions:[#{name,alive,exit}]}
    session_open() / session_open(cmd[, cols, rows]) / session_use(name) / session_close(name)
  能力面清单与 JSON-RPC 桥共用一张表（term_core::surface::METHODS，ADR-0023）：
  脚本能调的动词 = 那张表里 exposure 含 script 的那些。

  write 与 keys 的分工是语义：write 送**原始字节**（要控制字符就自己拼 \\r \\u001b），
  keys 按**键名**送（enter / tab / esc / backspace / up / f5 / ctrl+c / alt+b …），
  所以 enter 是 CR、F5 是 ESC[15~、方向键按会话的 DECSET 1 选 ESC[A 或 ESC OA。
  认不出的键名是错误，不会被当成文本写出去。

  session_* 是「全局层」：无参原语作用在当前绑定的会话上，session_open 新开的
  那条**会自动成为当前绑定**；关当前绑定是错误（先 session_use 到别处）。每条
  会话的读游标是独立的，切过去不会看到上一条的残留。

  会话按**名字**寻址，不是数字 id：名字跨进程稳定，所以脚本存下来明天还能跑。
  重名会自动加后缀（cat → cat-2），`session_open` 返回的是**真名**——拿它
  去 session_use / session_close，别拿自己写进去的那个。

  五处会让人写错的地方（都是 Rhai 语义或阻塞原语的边界，不是本项目特有）：
    · `read()` 是**排水**：只取当下已到的，晚到的尾部留给下一次 read。
      要「等齐」请用 read_until(标记, 秒) / wait_any([...], 秒)，别靠一次 read。
    · `s.trim()` 是**原地改、返回 unit**。`print(s.trim())` 会打印空串；
      要先 `s.trim();` 再 `print(s)`。
    · Rhai 没有数组的 `join`。拼名字请用 for 循环累加，或直接 print(数组)
      （渲染成 `[a, b]`）。
    · `set_synchronous(true)` 不是「保证远端处理完了」：它只是把「写完立刻 read」
      的漏读窗口从 0 收到 120ms。要确定性就用 wait(pat, 秒) / wait_idle(秒)。
    · `ask(prompt)` 是**真的在等人回话**：终端里它会一直等，`--timeout` 拦不住
      （那是脚本的墙钟预算，不是人打字的速度）。无人值守的脚本不要问——用
      notify 记一笔，然后自己走默认分支。

  <file>            脚本路径；`-` 表示从 stdin 读（便于一行试跑）。此时 ask()
                    不可用——输入源已经被脚本本身占住了。stdin 不是终端
                    （管道 / 重定向 / 后台）时 ask() 同样显式失败，不会返回空串。
  --exec CMD        用一次性命令会话代替本地 shell。CMD 要活到脚本结束：
                    像 `echo hi` 这种秒退的命令，会话在脚本登记等待之前就结束了，
                    `wait` / `read_until` 会诚实报「会话已结束」（只有 `screen`
                    还能取到已经画上去的那一屏）。驱动交互请留空走本地 shell。
  --session NAME    用会话库里那条会话（可以是内置 SSH 会话）。与 --exec 互斥。
                    密码 / keyboard-interactive 认证的会话需要保险库，而这条命令
                    是无窗口的：请改用 Agent 或私钥认证，或先在窗口里解锁。
  --cols / --rows   会话尺寸，默认 80×24
  --timeout SECS    沙箱墙钟上限，默认 30（同时钳制单次 wait）
  --quiet           不打印脚本返回值，只看退出码
  --batch           批处理预算档：600 秒 / 5000 万操作（界面档是 30 秒 / 100 万）；
                     显式 --timeout 仍会盖过档位里的墙钟
  --arg VALUE       传给脚本的参数，`args()` / `args(0)` 读取（0 起）；可重复
  --allow SPEC      本次运行的额外授权，如 fs=write:/tmp,process,ui,app=configure；
                     与脚本头 `// @cap ...` 取并集，可重复（逗号拼接），不写进配置

退出码：0 = 脚本跑完；1 = 会话起不来或脚本报错；2 = 用法 / 读文件失败。";

/// 一条 `script run` 的全部输入。纯数据，便于把参数解析单独测。
#[derive(Debug)]
pub struct Options {
    /// 脚本路径；`-` = stdin。
    pub file: String,
    /// `Some(cmd)` = 一次性命令会话；`None` = 本地 shell 或会话库里那条。
    pub exec: Option<String>,
    /// `Some(name)` = 会话库里那条会话（可以是内置 SSH）。
    pub session: Option<String>,
    pub cols: usize,
    pub rows: usize,
    /// `None` = 按 `--batch` 与否取 `ScriptBudget` 的缺省值。
    pub timeout: Option<Duration>,
    pub quiet: bool,
    /// `--batch`：批处理预算（10 分钟 / 5000 万操作）而不是界面的 30 秒。
    pub batch: bool,
    /// `--arg X`（可重复）：脚本里 `args()` 读到的参数。
    pub args: Vec<String>,
    /// `--allow fs=read:/tmp,process`：本次运行的额外授权。
    pub allow: String,
}

/// 解析 `script` 之后的参数（即 `["run", "<file>", …]`）。
pub fn parse(rest: &[String]) -> Result<Options, String> {
    let Some(sub) = rest.first() else {
        return Err("script 后面需要一个动作，目前只有 `run`".to_string());
    };
    if sub != "run" {
        return Err(format!("不认识的 script 动作：{sub}（目前只有 run）"));
    }
    let mut opts = Options {
        // 先占位，下面必填项校验会覆盖它；这样参数顺序可以任意。
        file: String::new(),
        exec: None,
        session: None,
        cols: 80,
        rows: 24,
        timeout: None,
        quiet: false,
        batch: false,
        args: Vec::new(),
        allow: String::new(),
    };

    let mut index = 1;
    while index < rest.len() {
        match rest[index].as_str() {
            "--exec" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--exec 后面需要一条命令".to_string())?;
                opts.exec = Some(raw.clone());
                index += 2;
            }
            "--session" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--session 后面需要一个会话名".to_string())?;
                if raw.trim().is_empty() {
                    // 空名字只会变成一次「会话库里找不到」的误导性报错。
                    return Err("--session 的会话名不能是空的".to_string());
                }
                opts.session = Some(raw.clone());
                index += 2;
            }
            "--cols" => {
                opts.cols = number(rest.get(index + 1), "--cols")?;
                index += 2;
            }
            "--rows" => {
                opts.rows = number(rest.get(index + 1), "--rows")?;
                index += 2;
            }
            "--timeout" => {
                let secs: usize = number(rest.get(index + 1), "--timeout")?;
                if secs == 0 {
                    // 0 秒的字面意思是「立刻超时」，那只会让每条脚本都失败；
                    // 想要「不限时」的调用方其实想要的是「够长」，让他自己写。
                    return Err("--timeout 必须大于 0".to_string());
                }
                opts.timeout = Some(Duration::from_secs(secs as u64));
                index += 2;
            }
            "--quiet" => {
                opts.quiet = true;
                index += 1;
            }
            "--batch" => {
                opts.batch = true;
                index += 1;
            }
            "--arg" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--arg 后面需要一个值".to_string())?;
                opts.args.push(raw.clone());
                index += 2;
            }
            "--allow" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--allow 后面需要授权项（如 fs=read:/tmp,process）".to_string())?;
                if raw.trim().is_empty() {
                    return Err("--allow 不能是空的".to_string());
                }
                // 多条 --allow 用逗号接起来：`parse_allow` 认逗号分隔，
                // 这样两种写法（一次给全 / 分几次给）行为一致。
                if !opts.allow.is_empty() {
                    opts.allow.push(',');
                }
                opts.allow.push_str(raw);
                index += 2;
            }
            // `-` 是路径（stdin），不是参数——所以它在「以短横线开头 = 参数」
            // 这条规则里被单独放出来。
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("不认识的参数：{other}"));
            }
            path => {
                if !opts.file.is_empty() {
                    return Err(format!(
                        "只接受一个脚本路径，多出来的是：{path}（上一个已经是 {}）",
                        opts.file
                    ));
                }
                opts.file = path.to_string();
                index += 1;
            }
        }
    }

    if opts.file.is_empty() {
        return Err("缺少脚本路径（用 `-` 从 stdin 读）".to_string());
    }
    if opts.exec.is_some() && opts.session.is_some() {
        // 两个都给了就意味着「用哪条会话」有两个答案。随便挑一个等于静默丢掉
        // 用户的一半意图；驱动交互（本地 shell）就是不传这两项。
        return Err("--exec 与 --session 只能二选一（驱动本地 shell 就两项都不传）".to_string());
    }
    Ok(opts)
}

fn number(raw: Option<&String>, flag: &str) -> Result<usize, String> {
    let raw = raw.ok_or_else(|| format!("{flag} 后面需要一个数字"))?;
    // 不静默回退默认值：`--cols 8O`（字母 O）退回 80 会让脚本里的
    // 换行位置与预期不符，而回报只是一个看不出来的错位。
    raw.parse::<usize>()
        .map_err(|_| format!("{flag} 只接受非负整数，实际是 {raw:?}"))
}

/// 执行。返回退出码（0 / 1 / 2）。
pub fn run(rest: &[String]) -> i32 {
    let opts = match parse(rest) {
        Ok(opts) => opts,
        Err(msg) => {
            eprintln!("{msg}\n\n{USAGE}");
            return 2;
        }
    };

    let source = match read_script(&opts.file) {
        Ok(text) => text,
        Err(msg) => {
            eprintln!("{msg}");
            return 2;
        }
    };

    // 预算：`--batch` 选批处理档（10 分钟），否则界面档（30 秒）；`--timeout`
    // 显式给的墙钟盖过档位里的那两个数。
    let mut budget = ScriptBudget::of(if opts.batch {
        BudgetKind::Batch
    } else {
        BudgetKind::Interactive
    });
    if let Some(timeout) = opts.timeout {
        budget = ScriptBudget {
            wall: timeout,
            ..budget
        };
    }
    let sandbox = budget.sandbox();

    let pool = Arc::new(SessionPool::new());
    // 控制面本体认内置 SSH 的前提是把通道工厂装上；`open_session`
    // 也会幂等装一次，这里显式装是让「这个池子从一开始就是完整的」。
    crate::session_open::install_channel_factory(&pool);
    // 开出来的会话要连「它怎么连的」一起带出来：控制面的 `remote_files()`（脚本
    // `sftp_*` 端口）靠 `PooledSessions::remember` 那张表拿凭据，而这张表只由
    // `Sessions::open` 与界面播种填。CLI 这条路径直接调 `open_session`，**绕过**
    // 了 `Sessions::open`——曾经因此让 `desk script run --session X` 里的
    // `sftp_put` 一律报「不知道会话 X 是怎么连的（没有它的配置）」。
    // `--exec` 起来的是命令会话，没有可复用的连接参数，故为 `None`。
    let (opened, remembered): (Result<SessionId, OpenFailure>, Option<SessionConfig>) =
        match (&opts.exec, &opts.session) {
            (Some(command), _) => (
                pool.open_exec(command, opts.cols, opts.rows)
                    .map_err(OpenFailure::Session),
                None,
            ),
            (None, Some(name)) => match pick_session(name, persist::load()) {
                Ok(config) => (
                    open_session(&pool, &config, opts.cols, opts.rows),
                    Some(config),
                ),
                Err(msg) => {
                    // 会话库读不到 / 名字不存在都是**输入**问题（和读不到脚本同类），
                    // 所以是 2，不是「会话起不来」的 1。
                    eprintln!("{msg}");
                    return 2;
                }
            },
            (None, None) => {
                // 登录名由宿主注入（core 不读 `$USER`）。
                let mut config = SessionConfig::new_local_shell(&term_io::host::login());
                config.name = "script".to_string();
                (
                    open_session(&pool, &config, opts.cols, opts.rows),
                    Some(config),
                )
            }
        };
    let id = match opened {
        Ok(id) => id,
        Err(failure) => {
            eprintln!("{}", start_failure_line(&failure));
            return 1;
        }
    };

    // 借出会话之前先确认它在册：`session()` 返回 `None` 只有一种可能——
    // 登记表里没有这个 id，那是控制面的内伤，不该被说成「脚本报错」。
    let Some(session) = pool.session(id) else {
        eprintln!("控制面里查不到刚开的会话（id={id:?}）");
        pool.close(id);
        return 1;
    };

    let python_mode = opts.file.to_ascii_lowercase().ends_with(".py");
    let outcome: Result<String, String> = if python_mode {
        // 外部 Python：复用界面那套 ScriptRun（租约 / 取消 / Outcome），
        // 只是这边同步轮询到结果再退出。授权仍按 process 收，不偷偷放行。
        match super::python::run(Arc::clone(&pool), id, &opts.file, opts.args.clone(), opts.allow.clone(), budget) {
            Ok(value) => Ok(value),
            Err(message) => { eprintln!("{message}"); for (id, _, _) in pool.list() { pool.close(id); } return 1; }
        }
    } else {
        // 工作区 = 池级集合 + 起始会话。脚本因此能 `session_list` 看到这条会话、
        // `session_open` 再开新的（命令会话 / 本地 shell）、`session_use` 在它们之间切。
        let host = PooledSessions::new(Arc::clone(&pool));
        if let Some(config) = remembered {
            host.remember(id, config);
        }
        let sessions: Arc<dyn term_script::Sessions> = Arc::new(host);
        // 人机端口：进程级就是「提示写 stderr、回答读 stdin」。脚本源码本身来自
        // stdin 时关掉输入源——那份输入已经吃完了，再读只会拿到 EOF，而返回空串
        // 等于替用户按了回车。
        let mut human = StdinHuman::new();
        if opts.file == "-" {
            human = human.without_input();
        }
        let spec = RunSpec::new(source.clone())
            .with_args(opts.args.clone())
            .with_path(Some(opts.file.clone()))
            .with_budget(budget)
            .with_size(opts.cols, opts.rows)
            .with_allow(opts.allow.clone());
        let caps = match script_env::envelope(&spec) {
            Ok(caps) => caps,
            Err(msg) => {
                // 授权头写错是**输入**问题（和读不到脚本同类），所以是 2。
                eprintln!("脚本的授权声明有问题：{msg}");
                pool.close(id);
                return 2;
            }
        };
        let env = script_env::build(
            &spec,
            sessions,
            Arc::new(human),
            WindowPorts::default(),
            CancelToken::new(),
            caps,
        )
        .env;
        let workspace = Arc::new(ScriptWorkspace::new(
            env,
            session,
            sandbox.execution_timeout,
        ));
        ScriptEngine::new(sandbox)
            .run_in_workspace(&source, workspace)
            .map(|value| value.to_string())
            .map_err(|err| err.to_string())
    };

    // 先收会话再看结果：脚本跑到一半失败时，会话可能还在吐输出；而脚本也可能
    // 用 `session_open` 开过好几条——**全部**收掉，漏一条就是把进程留在不返回的状态。
    for (id, _, _) in pool.list() {
        pool.close(id);
    }

    match outcome {
        Ok(value) => {
            if !opts.quiet && !value.is_empty() {
                println!("{value}");
            }
            0
        }
        Err(err) => {
            // 只回显原因（`ScriptError` 的 Display 已经写了「第几行什么错」），
            // 不打整屏「脚本失败：」加提示——自动化里 stderr 少一行噪音都好。
            eprintln!("{err}");
            1
        }
    }
}

fn read_script(path: &str) -> Result<String, String> {
    if path == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| format!("读不了 stdin：{e}"))?;
        return Ok(text);
    }
    std::fs::read_to_string(path).map_err(|e| format!("读不了脚本 {path}：{e}"))
}

/// 从会话库里挑出 `--session` 点名的那一条。
///
/// 纯函数（会话库从外面递进来）有两个理由：单测不用碰真实 profile 目录；
/// 而「库里有什么」这件事不该由这个命令自己去读——读库在 [`run`] 里一行完成。
fn pick_session(
    name: &str,
    loaded: Option<(Vec<String>, Vec<SessionConfig>)>,
) -> Result<SessionConfig, String> {
    let Some((_groups, sessions)) = loaded else {
        return Err(format!(
            "读不到会话库，所以拿不到会话 {name:?}；\
             先用 `term-desk` 打开一次窗口并保存会话，\
             或者不用 --session（走本地 shell / --exec）"
        ));
    };
    if let Some(config) = sessions.iter().find(|s| s.name == name) {
        return Ok(config.clone());
    }
    // 报错要报出**现有的名字**：打错一个字时，列表比一句「不存在」有用得多。
    let mut available: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
    available.sort_unstable();
    let list = if available.is_empty() {
        "（库是空的）".to_string()
    } else {
        available.join("、")
    };
    Err(format!("会话库里没有叫 {name:?} 的会话；现有：{list}"))
}

/// 会话起不来时打到 stderr 的那一行。
///
/// `NeedsVault` 不加「起会话失败：」前缀：它要说的是一件**具体可做的事**
/// （去解锁保险库，或改用 Agent/私钥），压成一句泛泛的失败会让人以为会话坏了。
fn start_failure_line(failure: &OpenFailure) -> String {
    match failure {
        OpenFailure::NeedsVault { .. } => failure.to_string(),
        other => format!("起会话失败：{other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // 之后默认 shell 走 `new_local_shell`，bin 目标不再需要 `Protocol`；
    // 只有这里的夹具按协议造会话，所以导入落在测试模块里。
    use term_core::session::Protocol;

    fn argv(rest: &[&str]) -> Vec<String> {
        rest.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn usage_lists_every_flag_the_parser_accepts() {
        // 参数早就实现了，USAGE 没跟上会让 `--arg/--allow/--batch` 只能用「猜」发现。
        for flag in ["--arg", "--allow", "--batch"] {
            assert!(USAGE.contains(flag), "USAGE 漏了 {flag}");
        }
    }

    #[test]
    fn the_minimal_invocation_is_run_plus_a_path() {
        let opts = parse(&argv(&["run", "smoke.rhai"])).expect("最小调用要能解析");
        assert_eq!(opts.file, "smoke.rhai");
        assert!(opts.exec.is_none(), "默认开本地 shell，不是 exec");
        assert_eq!((opts.cols, opts.rows), (80, 24));
        assert!(opts.timeout.is_none(), "不传就用沙箱默认 30 秒");
        assert!(!opts.quiet);
    }

    #[test]
    fn the_flags_are_order_independent() {
        let opts = parse(&argv(&[
            "run", "--quiet", "a.rhai", "--cols", "100", "--exec", "echo hi", "--timeout", "5",
        ]))
        .expect("参数可乱序");
        assert_eq!(opts.file, "a.rhai");
        assert!(opts.quiet);
        assert_eq!(opts.cols, 100);
        assert_eq!(opts.exec.as_deref(), Some("echo hi"));
        assert_eq!(opts.timeout, Some(Duration::from_secs(5)));
    }

    #[test]
    fn stdin_is_spelled_as_a_single_dash() {
        // `-` 会走进 `other.starts_with('-')` 之前的分支（先判 path 还是先判
        // 短横线），这里钉住顺序：`-` 是路径，不是未知参数。
        let opts = parse(&argv(&["run", "-"])).expect("- 是合法路径");
        assert_eq!(opts.file, "-");
    }

    #[test]
    fn bad_input_is_an_error_with_a_reason_not_a_default() {
        assert!(parse(&argv(&[])).is_err(), "缺动作");
        assert!(parse(&argv(&["walk", "x.rhai"])).is_err(), "只认 run");
        assert!(parse(&argv(&["run"])).is_err(), "缺路径");
        assert!(parse(&argv(&["run", "a.rhai", "b.rhai"])).is_err(), "两个路径");
        assert!(parse(&argv(&["run", "a.rhai", "--cols"])).is_err(), "缺值");
        assert!(parse(&argv(&["run", "a.rhai", "--cols", "8O"])).is_err(), "字母 O");
        assert!(parse(&argv(&["run", "a.rhai", "--timeout", "0"])).is_err(), "0 秒无意义");
        assert!(parse(&argv(&["run", "a.rhai", "--force"])).is_err(), "未知参数");
    }

    #[test]
    fn reading_the_script_reports_the_path_that_failed() {
        let err = read_script("/nonexistent/definitely-not-here.rhai").unwrap_err();
        assert!(err.contains("definitely-not-here"), "缺的不是消息而是路径：{err}");
    }

    // ── `--session`：控制面（脚本侧）开内置 SSH 会话的入口 ──────────────

    fn saved(name: &str, protocol: Protocol) -> SessionConfig {
        let mut config = SessionConfig::new(protocol);
        config.name = name.to_string();
        config.host = "127.0.0.1".to_string();
        config.username = "qa".to_string();
        config
    }

    #[test]
    fn a_named_session_is_taken_from_the_store_as_is() {
        let loaded = Some((
            vec!["Servers".to_string()],
            vec![saved("prod", Protocol::Ssh), saved("local", Protocol::Shell)],
        ));
        let config = pick_session("prod", loaded.clone()).expect("库里有 prod");
        // 关键断言：内置 SSH 那条会话**原样**交出去，没有被改成本地 shell。
        assert_eq!(config.name, "prod");
        assert_eq!(config.protocol, Protocol::Ssh);
        assert!(config.builtin_ssh, "新会话默认走内置客户端");
        // 换个名字拿到的是另一条，不是第一条。
        assert_eq!(
            pick_session("local", loaded).expect("库里有 local").protocol,
            Protocol::Shell
        );
    }

    #[test]
    fn a_missing_session_name_lists_what_is_there() {
        let loaded = Some((
            Vec::new(),
            vec![saved("prod", Protocol::Ssh), saved("alpha", Protocol::Shell)],
        ));
        let err = pick_session("prodd", loaded).unwrap_err();
        assert!(err.contains("prodd"), "要点名打错的那个：{err}");
        assert!(
            err.contains("alpha") && err.contains("prod"),
            "要把现有的名字列出来：{err}"
        );
    }

    #[test]
    fn no_session_store_is_an_input_error_with_a_way_out() {
        let err = pick_session("prod", None).unwrap_err();
        assert!(err.contains("会话库"), "{err}");
        assert!(err.contains("--session"), "要给出不依赖会话库的路：{err}");
    }

    #[test]
    fn session_and_exec_cannot_both_be_given() {
        let err = parse(&argv(&[
            "run", "a.rhai", "--session", "prod", "--exec", "echo hi",
        ]))
        .unwrap_err();
        assert!(err.contains("--exec") && err.contains("--session"), "{err}");
        // 单给一个则都合法。
        assert!(parse(&argv(&["run", "a.rhai", "--session", "prod"])).is_ok());
        assert!(parse(&argv(&["run", "a.rhai", "--session"])).is_err(), "缺值");
        assert!(
            parse(&argv(&["run", "a.rhai", "--session", "  "])).is_err(),
            "空名字只会变成一次误导性的「找不到」"
        );
    }

    #[test]
    fn a_session_that_needs_the_vault_says_so_without_the_generic_prefix() {
        // 脚本侧唯一的呈现点：这一行就是脚本作者能看到的全部信息。
        let line = start_failure_line(&OpenFailure::NeedsVault {
            session: "prod".to_string(),
        });
        assert!(!line.contains("起会话失败："), "别把可操作的指令压成泛泛失败：{line}");
        assert!(line.contains("prod"), "{line}");
        assert!(line.contains("解锁保险库"), "{line}");
        // 其余三档仍然沿用「起会话失败：」的口径（与这一层出现之前一致）。
        for other in [
            OpenFailure::Unsupported("x".to_string()),
            OpenFailure::Connect("y".to_string()),
            OpenFailure::Session(term_session::OpenError::BuiltinChannelNeedsInjection),
        ] {
            assert!(start_failure_line(&other).starts_with("起会话失败："));
        }
    }
}
