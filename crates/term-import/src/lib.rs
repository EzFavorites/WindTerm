//! `term-import` — 独立的配置导入模块：把**别人家的配置**变成我们的领域类型。
//!
//! 2026-11-12 底层收敛时从 `term-runtime` 单独立出来（用户明确要求
//! 「配置导入最后我重新安排，需要实现独立的配置导入模块」）。这个 crate 就是
//! 那个模块的**物理边界**：它的依赖表里没有会话运行时、没有配置写入、没有 UI，
//! 所以「导入」这件事不可能偷偷顺手做别的事。
//!
//! # 三步流水线（这是留给下一步的接口面）
//!
//! ```text
//!   ① 发现                 ② 解析                     ③ 落库
//!   外部配置在哪            文本 → 领域类型             领域类型 → 我们的存储
//!   （本 crate 已实现）     （本 crate 已实现）          （调用方做）
//! ```
//!
//! - **① 发现**：[`discovery`] 只**枚举候选根目录**（macOS app 包内
//!   `Contents/MacOS/.wind/profiles`、`profiles.config` 目录指针、三处用户根、
//!   Windows/Linux 常见位置、以及手动指定的目录），并定位 `user.config`。
//!   只枚举、不扫描全盘、不读会话内容；要不要用哪一个是产品决定，由调用方做。
//! - **② 解析**：[`sessions`] 的 `parse_*` 与 [`user_config`] 的 `parse_user_config`
//!   都是纯函数，输入是**文本**，输出是领域类型。谁读的文件不归这里管——所以
//!   每条都能用固定字符串写单测，跑多少次结果都一样。
//! - **③ 落库**：`ImportedSession` → `SessionConfig`，以及
//!   `UserConfigDelta` → `Prefs`（在 `term-profile::prefs_import`）。写盘不在本
//!   crate：那一步要决定「重名怎么办 / 失败要不要回滚 / 坏文件怎么保全」，
//!   全是产品决策，而且它需要写权限——本 crate 刻意没有。
//!
//! # 三条纪律
//!
//! 1. **永不读密码。** `docs/design/09-session-import-and-onboarding.md` 把「密码不迁移」写成产品
//!    底线：各家加密方案不公开，本应用不尝试解密：不可靠，也不合规。`ImportedSession`
//!    类型里根本没有密码字段——这是类型层面的保证，不是约定。
//! 2. **近乎纯函数。** `parse_*` 不读文件、不看时钟；唯一的**环境读取**是
//!    `sessions::expand_home()` 里的 `dirs::home_dir()`——`parse_ssh_config` 必须
//!    把 `IdentityFile ~/.ssh/id_ed25519` 的 `~` 展开成绝对路径，所以它的输出随
//!    宿主的 `HOME` 变化。这条曾被写成「不读环境」，2026-11-13 架构审查
//!    证伪并订正；要完全消除它，得把「展开 `~`」上移到调用方（`term-paths` 已有
//!    `home()`），属 1.0.x 的收敛项。
//!    其余副作用是 [`theme::load_palette`] 找并读**我们自己的**主题文件，
//!    不是外部配置。
//! 3. **宽松解析，整份不丢。** 未知格式（WindTerm 的 `profiles/`）同时认 JSON 与
//!    INI 两种形态，认不出的**行**忽略、认不出的**整份**才返回空——不因为一行
//!    怪格式就把整份配置判死。
//!
//! # 模块
//!
//! - `sessions`：四种来源的会话导入纯解析（OpenSSH `config` / WindTerm /
//!   PuTTY / shell 历史），外加上游 WindTerm 的 `user.sessions` 解析 `parse_windterm_sessions`。
//!   **有真实调用方**：`apps/desktop/src/ui/panels/sidebar.rs` 的导入按钮经
//!   `Effect::ImportSessions` 调它。
//! - `user_config`：上游 WindTerm `user.config` 的 13 键白名单解析（哨兵两键不定义）。
//! - `discovery`：候选配置目录枚举 + `user.config` 定位，UI 两处候选路径的
//!   唯一来源（消除侧栏 / 引导的双写）。
//! - `theme`：主题文件的定位与读取（解析在 `term_core::theme` 的纯函数）。
//!   内置**三套**整窗主题（`dige-black` / `dige-light` / `dige-green`，见
//!   `term_core::theme::BUILTIN_THEME_NAMES`）；设置页的下拉逐项登记这三套，
//!   缺磁盘主题文件时也能切色。此前的「只有一个内置主题」是旧实现的陈旧登记。
//!
//! # 下一步要在这里做的事（不属于本轮）
//!
//! - ③ 落库层的接线：把 `UserConfigDelta` 经 `term-profile::prefs_import` 并进
//!   `Prefs` 的入口做成用户显式动作（本轮只提供纯函数，不自动导入、不改主题）；
//! - 重名策略（覆盖 / 跳过 / 改名）、分组映射、导入前预览的差异展示；
//! - 主题导入：外部终端主题格式（iTerm `.itermcolors` / Windows Terminal JSON）
//!   到 `term_core::theme::Palette` 的映射——注意 `Palette` 的字段是固定的，
//!   映射不上的项要有明确的回退，不能悄悄用黑色。

pub mod discovery;
pub mod sessions;
pub mod theme;
pub mod user_config;
