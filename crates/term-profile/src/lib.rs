//! `term-profile` — 用户档案层：配置目录里的东西怎么读、怎么写。
//!
//! 2026-11-12 底层收敛从 `term-runtime` 拆出。原来那个 crate 同时装着
//! 「每会话一个线程 + 伪终端」与「用户偏好 + 会话库 + 窗口布局」两件毫不相干的事，
//! 于是改一行偏好要等 supervisor 重新编译，而 supervisor 的依赖（portable-pty、
//! nix）也跟着渗进所有想读偏好的地方。
//!
//! # 边界（依赖表即判据）
//!
//! | 有 | 没有 |
//! |---|---|
//! | `term-core`（纯语义）、`term-paths`（路径）、`term-store`（SQLite 会话库）、`serde` | portable-pty、nix、tokio、任何 UI 框架 |
//!
//! **起不了线程、开不了伪终端、连不上网**。这一层的全部副作用是「读写
//! 配置目录里的文件」。
//!
//! # 为什么 `search` 在这里
//!
//! 它不是「持久化」，但它是**偏好页的数据**：`Preferences ▸ Search` 的下拉与
//! 菜单的 `Effect::SearchOnline` 必须读同一份引擎表，而 `prefs` 依赖它。
//! 放进这一层，这条依赖是同 crate 内部的，不产生跨层边。
//!
//! - `persist`：会话库与保险库文件的定位与读写（内容格式在 `term-store` / `term-vault`）
//! - `layout_state`：标签 / 分屏 / 停靠结构的持久化（「上次这个窗口长什么样」）
//! - `prefs`：界面偏好（语言、日志页五项、光标、字号、粘贴确认…）
//! - `quickbar`：快捷栏条目
//! - `search`：在线搜索的引擎表与 URL 拼装（设置页与菜单命令共用的唯一一份数据）
//!
//! 读不出来**不等于**可以覆盖它：`prefs` 的模块文档里写着这条底线——配置文件是
//! 用户可能手改的，少一个逗号不能让整份配置被默认值盖掉。

// L3（O 设计稿）：`layout.json` 的落盘元信息（时间戳 / 上次退出形态）。
// 与 `layout_state` 分开是 R9 行数的要求，也是关注点不同；调用方经
// `layout_state` 的再导出访问，不必知道这一层。
pub mod layout_meta;
pub mod layout_state;
pub mod persist;
pub mod prefs;
pub mod quickbar;
pub mod search;
