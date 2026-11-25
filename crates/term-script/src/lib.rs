//! `term-script` — 腿 A 脚本引擎（ADR-0019）。
//!
//! Rhai 沙箱 + 原语（`write` / `wait` / `wait_any` / `read_until` / `read` /
//! `screen`）与行级类 grep 过滤（`grep` / `grep_v`）。
//! 协议无关（§B.14 最高原则）：只认 [`term_core::session_runtime::Session`]，
//! 不出现 SSH / PTY 等协议专有分支——脚本驱动的是抽象会话，不是某类连接。
//!
//! 运行时依赖仅 `term-core`（单向）+ `regex`（只进本 crate，不进抽象层）；
//! 零 tokio、零 `.await`（ADR-0017 笼子）；不依赖 UI 框架：脚本线程绝不直写 UI 状态。
//!
//! # 引擎 API 形态
//!
//! **全局函数注入**（ZOC 的 `ZocSend` 等全局函数同手感）：`ScriptEngine::run`
//! 把原语绑到注入的会话上，脚本即任务验收示例的形状——
//!
//! ```rhai
//! write("echo hi\n");                // 不自动加换行，脚本自己拼 \n
//! let hit = wait("hi", 5.0);         // 命中返回 text_upto；超时抛错
//! let cap = read_until("#", 5.0);    // 读到哨兵（含 delimiter）；超时抛错
//! let tail = read(2.0);              // 排水取走剩余输出；超时返空串不报错
//! let f = grep_v("^noise");          // 行级过滤器句柄（grep / grep_v）
//! let body = read_until("@@", 5.0, f);
//! let m = wait_any(["OK", "ERROR"], 5.0);       // 等任意模式，先出现者胜
//! let m2 = wait_any(["OK", "ERROR"], 5.0, f);   // 也可带过滤器
//! m.index;                           // 1 起索引（超时不返回 0，直接抛错）
//! m.text;                            // 自游标到命中串结尾的文本
//! let screen = screen();
//! ```
//!
//! # 全局层
//!
//! 单会话原语作用在「当前绑定」上；`session_*` 五个动词负责选绑定与增删会话。
//! 一切按**会话名**寻址（ADR-0024）——名字跨进程稳定，脚本存下来明天还能跑：
//!
//! ```rhai
//! session_current();                 // 现在绑的是哪条会话（名字）
//! let a = session_open("cat");       // 新开一条命令会话，**并自动改绑**到它
//! write("hello\n");
//! let out = read_until("hello", 5);
//! session_close(a.name);             // 关掉它（关当前绑定会被拒绝）
//! for s in session_list().sessions { print(s.name); }
//! ```
//!
//! 一组会话（`session_list` 问谁要清单、`session_open` 让谁去开）由调用方实现
//! [`Sessions`]——`term-script` 不依赖应用层的会话登记表（单向）。
//!
//! # 语义选择
//!
//! - `wait` / `read_until` 走 [`term_core::session_runtime::Session::wait_register`]
//!   （先登记后消费），**超时显式失败**（抛 Rhai 错误），禁止静默空成功。
//!   两者的区别在返回值语义与游标：`wait` 用于同步（等到提示符，一次性自
//!   登记点）；`read_until` 用于取数据（游标续读，命中后游标推进到模式结尾、
//!   尾部文本不丢）。
//! - `read` / `read_until` / `wait_any` **共享捕获游标**（首次调用时登记，登记前输出不
//!   入账）：取走上次取走点以来的全部输出。有新数据即返；**超时返回空串**
//!   （排水不是等待事实，与 `read_until` 的超时显式失败是两种承诺）；
//!   会话死透显式报 Closed。
//! - `wait_any([patterns], timeout_secs[, filter])`（对标 SecureCRT
//!   `WaitForStrings` / `MatchIndex`）：等任意模式出现，返回最先出现者的
//!   [`WaitAnyHit`]（`index` **1 起**、`text` 为自游标到命中串结尾的文本）。
//!   同一位置多个模式命中时取数组靠前的。与 `read` / `read_until` **共享
//!   捕获游标**：命中后游标推进到命中串结尾，尾部文本不丢，留给后续
//!   `read` / `read_until` 续读。可选行级 grep 过滤器（同 `read_until`，
//!   作用于**匹配之前**）。**超时显式失败**——这里有意偏离 SecureCRT 的
//!   「索引 0 = 超时」：0 太容易被脚本忘判而静默继续（ADR-0019 禁止静默），
//!   显式抛错强制脚本处理超时分支。空模式列表或含空串直接拒绝。
//! - `read_until` / `read` / `wait_any` 接受行级类 grep 过滤器（[`LineFilter`]，由全局
//!   函数 `grep(pat)`（留匹配行）/ `grep_v(pat)`（丢匹配行）构造）：过滤
//!   作用于**匹配之前**（管道语义 `| grep -v noise` 再匹配哨兵）；不完整
//!   尾行与已判行的延续段按原样透传；无效正则显式报错。
//! - `write` 不自动追加换行；会话已结束时显式报错，不静默吞字节。
//! - `keys` 按**键名**送（`enter` 是 CR、`f5` 是 `ESC[15~`、方向键随会话的
//!   `DECSET 1` 选 `ESC[A` 或 `ESC OA`，`ctrl+c` / `alt+b` 同理）；整批键名
//!   **先全部翻译再写**，认不出任何一个就整批不发。键名表在
//!   [`term_core::keys`]，与界面按键路径同一张表——「界面里能按的键，脚本里
//!   也能按」靠的是同表，不是两份各自维护的 match。
//! - `screen` 返回 [`term_core::grid::GridSnapshot::screen_text`] 全文。
//! - `wait` 不加过滤器——它是同步原语；需要过滤时用 `read_until`。

mod cancel;
mod clip_ui;
mod compat;
mod editor;
mod engine;
mod env;
mod error;
mod filter;
mod human;
mod localfs;
mod proc;
mod sandbox;
mod sendkeys;
mod session;
mod sessions;
mod workspace;

#[cfg(test)]
mod testutil;

pub mod app_ui;

pub use app_ui::{AppControl, AppDelta, AppError, LayoutOp, NoApp};
pub use cancel::{CancelToken, Cancelled};
pub use clip_ui::{ClipError, Clipboard, NoClipboard, NoUi, Ui, UiError};
pub use editor::{lex, ScriptDoc, Token, TokenKind};
pub use engine::ScriptEngine;
pub use env::{HostEnv, ScriptContext};
pub use error::{PrimitiveError, ScriptError};
pub use filter::{GrepHandle, LineFilter};
pub use human::{
    Answer, Ask, AskError, HeadlessHuman, HumanChannel, MessageButton, MessageIcon, PickFile,
};
pub use localfs::{normalize_for, DirEntry, FileStat, FsError, LocalFs, NoFiles, RemoteFiles};
pub use proc::{NoProcess, ProcError, ProcOutput, ProcRun, Process};
pub use sandbox::{BudgetKind, SandboxConfig, ScriptBudget};
pub use sendkeys::Stroke;
pub use session::{ScriptSession, WaitAnyHit};
pub use sessions::{
    FixedSessions, SessionInfo, Sessions, SessionsError, PHASE_ALIVE, PHASE_CONNECTING,
    PHASE_FINISHED,
};
pub use workspace::{ScriptWorkspace, SessionError};
