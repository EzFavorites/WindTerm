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
//!   ① 发现                 ② 解析                  ③ 落库
//!   外部配置在哪            文本 → 领域类型           领域类型 → 我们的存储
//!   （crate 层未做；        （本 crate 已实现）        （调用方做）
//!     产品层已有探测）
//! ```
//!
//! - **① 发现**：`~/.ssh/config`、WindTerm 的 `profiles/`、PuTTY 的注册表导出、
//!   `~/.bash_history` 之类的位置枚举。**本 crate 刻意不实现**——路径策略要跟着
//!   产品决定走（要不要扫描、要不要问用户、要不要记住上次的选择），先划出边界
//!   比先写死一套规则更有价值。产品层已有入口：首次启动向导
//!   `apps/desktop/src/ui/overlays/onboarding.rs::detect_hosts` 会读 `~/.ssh/config`
//!   与三处 WindTerm `profiles/`，再调本 crate 的 `parse_*`；那是产品侧的发现，
//!   不是 `term-import` 自己的发现器。
//! - **② 解析**：[`sessions`] 四个 `parse_*` 纯函数，输入是**文本**，输出是
//!   [`sessions::ImportedSession`]。谁读的文件不归这里管——所以每条都能用固定
//!   字符串写单测，跑多少次结果都一样。
//! - **③ 落库**：`ImportedSession` → `SessionConfig` → `term-profile` 的存储。
//!   不在本 crate：那一步要决定「重名怎么办 / 放到哪个分组 / 失败要不要回滚」，
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
//!   PuTTY / shell 历史）。**有真实调用方**：`apps/desktop/src/ui/panels/sidebar.rs`
//!   的导入按钮经 `Effect::ImportSessions` 调它。
//! - `theme`：主题文件的定位与读取（解析在 `term_core::theme` 的纯函数）。
//!   目前只有一个内置主题 `dige-black`，所以设置页的主题下拉只有一项——如实
//!   反映现状，不放假选项。
//!
//! # 下一步要在这里做的事（不属于本轮）
//!
//! - ① 发现层：crate 层仍空缺；若要收进来，先对齐产品层 `detect_hosts` 的边界
//!   （扫哪些目录、怎么问用户、怎么记住选择），当前仍由产品层负责；
//! - ③ 落库层：重名策略（覆盖 / 跳过 / 改名）、分组映射、导入前预览的差异展示；
//! - 主题导入：外部终端主题格式（iTerm `.itermcolors` / Windows Terminal JSON）
//!   到 `term_core::theme::Palette` 的映射——注意 `Palette` 的字段是固定的，
//!   映射不上的项要有明确的回退，不能悄悄用黑色。

pub mod sessions;
pub mod theme;
