//! `term-runtime` — 兼容门面（2026-11-12 底层收敛后的落点）。
//!
//! 这个 crate 曾经是 16 个模块 7579 行的「副作用杂货箱」：会话线程、伪终端、
//! 串口、会话日志、会话库、界面偏好、窗口布局、主题文件、外部会话导入、本地
//! 目录枚举全挤在一起。后果很具体——改一行偏好要等 Supervisor 重新编译；
//! 而任何想读偏好的地方会连带把 `portable-pty` 与 `nix` 拖进依赖树；
//! `term-ssh` 更是为了一个 trait 就把整个 PTY 栈拖了进来。
//!
//! 现在它只剩两件事：
//!
//! 1. **还没搬走的两个纯解析模块**（`import_sessions` / `theme_load`）；
//! 2. **对所有旧路径的再导出**——`term_runtime::supervisor::Supervisor`、
//!    `term_runtime::persist::save`、`term_runtime::log::default_dir` 全部照旧解析。
//!
//! 第 2 条是刻意的：`apps/desktop/src` 下仍有大量 `term_runtime::` 引用（2026-11-13
//! 实测 142 行 / 48 文件；这个数字会随重构下降，别在注释里写死，用
//! `grep -rn 'term_runtime::' apps/desktop/src | wc -l` 现取），而那一层正等用户
//! 重新校对采集——搬文件会制造一堆无意义的 import 冲突。所以 UI 侧一行不改，
//! 收敛全部在它下面完成。
//!
//! # 依赖方向
//!
//! ```text
//! term-runtime (facade)
//!   ├── term-io       会话 IO 与线程：pty / serial / channel / supervisor / log / local_dir
//!   ├── term-profile  用户档案：persist / layout_state / prefs / quickbar / search
//!   ├── term-paths    路径规则：profile_dir（只依赖 dirs）
//!   └── term-import   外部配置导入的纯解析（C4 之后）
//! ```
//!
//! 反向边一条都没有，四条都由 `scripts/verify.sh` 第 4d 项的依赖黑名单守着。
//!
//! **新代码请直接依赖上面对应的 crate，不要依赖本门面。** 门面存在的唯一目的
//! 是让旧路径继续解析；等 `ui/` 重新校对完，它可以退化成纯 `pub use`。
//!
//! # 已经全搬完了
//!
//! C4 把最后两个模块（外部会话导入 / 主题文件读取）搬到了 `term-import`，
//! 那是用户要求的「独立配置导入模块」。本 crate 现在**没有任何实现**，
//! 全部内容都是 `pub use`。
//!
//! 它不再是「副作用会话编译单元」，也不再对应某个 ADR 里的编译边界——
//! 它只有一条存在理由：让 `apps/desktop/src` 里那些旧路径继续解析（数量以实测为准，
//! 见上文第 2 条）。
//! 等那一层重新校对采集完，可以把它删掉，把 import 改成新 crate 名。

// ── 配置导入层：实现已搬到 `term-import`（2026-11-12 收敛 C4）───────────────
//
// 模块改名了（`import_sessions` → `sessions`，`theme_load` → `theme`），
// 这里用旧名再导出，旧路径照旧解析。
pub mod import_sessions {
    pub use term_import::sessions::*;
}
pub mod theme_load {
    pub use term_import::theme::*;
}

// ── 会话 IO 与线程层：实现已搬到 `term-io`（2026-11-12 收敛 C3）─────────────
//
// 保留同名模块做再导出，调用路径不变。trait `TermChannel` 与 `SpawnError`
// 的类型本体更下一层，在 `term_core::channel`（C1）——传输层不该为了它们
// 依赖带 PTY 与线程的 crate。
pub mod channel {
    pub use term_io::channel::*;
}
pub mod error {
    pub use term_io::error::*;
}
pub mod local_dir {
    pub use term_io::local_dir::*;
}
pub mod log {
    pub use term_io::log::*;
}
pub mod pty {
    pub use term_io::pty::*;
}
pub mod serial {
    pub use term_io::serial::*;
}
pub mod serial_channel {
    pub use term_io::serial_channel::*;
}
pub mod supervisor {
    pub use term_io::supervisor::*;
}

// ── 用户档案层：实现已搬到 `term-profile`（2026-11-12 收敛 C2）──────────────
pub mod layout_state {
    pub use term_profile::layout_state::*;
}
pub mod persist {
    pub use term_profile::persist::*;
}
pub mod prefs {
    pub use term_profile::prefs::*;
}
pub mod quickbar {
    pub use term_profile::quickbar::*;
}
pub mod search {
    pub use term_profile::search::*;
}
/// profile 目录的规则本体在 `term-paths`，这里只是入口。
pub mod paths {
    pub use term_paths::*;
}
