//! `term-core` — 纯语义编译单元（ADR-0014）。
//!
//! 字节 → 状态 → 事实的全部计算都在这里，且**零 UI 框架、零 IO、零线程、零墙钟、
//! 零 OS 句柄**。依赖表见 `Cargo.toml`：`vte` / `serde` / `serde_json` 仅做纯解析。
//!
//! 边界由编译器强制（`scripts/verify.sh` 第 4 / 4b / 4c / 4d 项复核）：
//! 本 crate 的 `Cargo.toml` 只声明纯解析依赖，不含任何 UI / PTY / 文件系统 crate。
//!
//! - `grid`：Cell / Rgb / Attrs / GridSnapshot / LineSlice / ScreenImage
//! - `vt`：`vte::ansi::Handler` 实现，唯一持有网格；`slice_range` 纯函数
//! - `ansi`：会话日志的文本清洗（剔除转义序列，复用同一个 `vte` 解析器）
//! - `width`：Unicode 双宽判定
//! - `find`：GridSnapshot 文本匹配
//! - `theme`：调色板数据与纯解析（文件读取在 `term-import::theme`）
//! - `config`：会话 / 窗口的默认值常量
//! - `coords`：三套坐标系的类型化边界（历史行 / 视口行 / 字节偏移）
//! - `command_history`：命令行历史存储（数据层，纯语义）
//! - `completion` / `completion_words`：自动补全引擎与静态词库（I-AC，纯语义）
//! - `input_line`：输入行模型（原始按键字节 → 回车前的命令行）
//! - `keys`：键名 → PTY 字节（脚本 / 远程按名字按键的唯一映射表）
//! - `session`：会话配置纯数据
//! - `script_caps`：脚本的授权信封（文件 / 进程 / 剪贴板 / 窗口，缺省全拒）
//! - `session_runtime`：运行时会话的协议无关抽象（ADR-0019）
//! - `log_opts`：会话日志的选项（Log type / 覆盖追加 / 文件名与内容模板）与
//!   两个展开函数。三处（写入端 / 配置界面 / 偏好）共用一份语义
//! - `osc133`：Shell 集成的 OSC 133 标记流（前置，纯字节语义）
//! - `surface`：会话控制面的描述层（方法名 / 说明 / 参数与结果 schema，纯数据）
//! - `telnet`：Telnet 选项协商纯状态机（IAC 转义 / ECHO / SGA / NAWS）
//! - `tunnel`：Tunnels 配置的纯数据与校验（一条命名隧道 → 一行 `L|R|D` 规则文本）
//! - `auth`：OneKey 凭据回退链（有序尝试 + 全失败诊断，纯状态机）
//! - `channel`：会话字节通道的契约（`TermChannel` trait）与启动失败分类
//!   （`SpawnError`）。纯类型，没有实现——实现方在 `term-runtime`（本地 PTY /
//!   外部进程）与 `term-ssh`（原生 SSH）。放在这里是为了让传输层不必依赖
//!   带 PTY 与线程的运行时层（2026-11-12 收敛）

pub mod ansi;
pub mod app_state;
pub mod automation;
pub mod auth;
pub mod channel;
pub mod command_history;
pub mod completion;
pub mod completion_words;
pub mod config;
pub mod coords;
pub mod find;
pub mod input_line;
pub mod keys;
pub mod log_opts;
pub mod mouse_report;
pub mod onekey;
pub mod grid;
pub mod osc133;
pub mod script_caps;
pub mod session;
pub mod session_runtime;
pub mod surface;
pub mod telnet;
pub mod tunnel;
pub mod theme;
mod theme_font;
mod theme_json;
pub mod trigger;
pub mod vt;
pub mod width;
