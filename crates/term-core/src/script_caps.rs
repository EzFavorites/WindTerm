//! 脚本的**授权信封**（ADR-0027）：一次运行能碰哪些本地副作用。
//!
//! # 为什么默认全拒
//!
//! 脚本是「按文件跑的一段自动化」，它到达时用户可能已经忘了里面写了什么。
//! 文件、外部进程、剪贴板读、窗口控制、**应用级状态**（面板 / 布局 / 偏好，
//! 见 [`AppScope`]，ADR-0027）这五件事都不是「让终端干活」，而是
//! **以用户身份动用户的机器**——所以缺省值只能是拒绝，放行必须来自一次
//! 显式动作（脚本头声明 + 界面确认 / CLI `--allow` / 用户在文件框里手选）。
//!
//! 反过来说：授权**绑定单次运行**，不写进任何全局配置。一次「允许」等于
//! 永久放行的模型，在这类工具里是最容易被忘掉的一条后门。
//!
//! # 三条判定纪律
//!
//! 1. 路径先规范化再比前缀（`canonicalize` 在实现侧做，本模块只收规范形态）；
//!    `..` 与相对路径在**比较之前**就拒——先比再规范会让 `logs/../../etc` 通过。
//! 2. 写权限是**独立**的一档：给了读根不代表能写它。
//! 3. 拒绝要说清三件事：缺哪项能力、请求的是什么、已授予的是什么。
//!    只报「权限不足」的话，脚本没法改走另一条分支，用户也不知道该宽哪一格。
//!
//! 本模块是纯数据 + 纯函数：不读文件、不 `canonicalize`、不读墙钟
//! （`scripts/verify.sh` 第 4b / 4c 项）。

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// 本地文件能力的范围。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FsScope {
    /// 全拒（缺省）。
    #[default]
    Deny,
    /// 只读根与可写根**分开记**：`fs=read:/tmp` + `fs=write:/var/tmp` 时
    /// `/tmp` 仍然只读。一张带全局 `write` 布尔的根表做不到这一点——那会让
    /// 第一个写根把此前所有读根一起抬成可写。
    ///
    /// 根目录自身也算在内（`/tmp` 允许 `/tmp` 与 `/tmp/x`，不允许 `/tmpfoo`）。
    /// 写蕴含读，所以 `check_read` 查两张表，`check_write` 只查写表。
    Roots {
        read_roots: Vec<PathBuf>,
        write_roots: Vec<PathBuf>,
    },
}

impl FsScope {
    /// 并集（按声明顺序去重）：供展示 / 合并时用，**不用来判权限**。
    pub fn roots(&self) -> Vec<PathBuf> {
        match self {
            FsScope::Deny => Vec::new(),
            FsScope::Roots {
                read_roots,
                write_roots,
            } => {
                let mut out: Vec<PathBuf> =
                    Vec::with_capacity(read_roots.len() + write_roots.len());
                for root in read_roots.iter().chain(write_roots.iter()) {
                    if !out.contains(root) {
                        out.push(root.clone());
                    }
                }
                out
            }
        }
    }

    /// 只读根（按声明顺序）。
    pub fn read_roots(&self) -> &[PathBuf] {
        match self {
            FsScope::Deny => &[],
            FsScope::Roots { read_roots, .. } => read_roots,
        }
    }

    /// 可写根（按声明顺序）。
    pub fn write_roots(&self) -> &[PathBuf] {
        match self {
            FsScope::Deny => &[],
            FsScope::Roots { write_roots, .. } => write_roots,
        }
    }

    /// 是否授出了**任何**可写根。展示 / 兼容旧调用用；判定必须按根查 `write_roots`。
    pub fn writable(&self) -> bool {
        matches!(self, FsScope::Roots { write_roots, .. } if !write_roots.is_empty())
    }

    /// 加一个根。已经是 `Deny` 时升级成 `Roots`。
    fn grant(&mut self, root: PathBuf, write: bool) {
        if matches!(self, FsScope::Deny) {
            *self = FsScope::Roots {
                read_roots: Vec::new(),
                write_roots: Vec::new(),
            };
        }
        if let FsScope::Roots {
            read_roots,
            write_roots,
        } = self
        {
            if write {
                if !write_roots.contains(&root) {
                    write_roots.push(root.clone());
                }
                // 同根从「只读」升到「读写」：读表里那份留着也无害，但清掉更清楚。
                read_roots.retain(|r| r != &root);
            } else if !write_roots.contains(&root) && !read_roots.contains(&root) {
                read_roots.push(root);
            }
        }
    }
}

/// 应用级状态的授权档（ADR-0027，2026-11-19）。
///
/// 为什么不并进 `ui`：`ui` 管的是**窗口**（显隐 / 退出 / 取选区），
/// 而这里管的是**应用级状态**（面板可见性、布局、偏好）。偏好写入是
/// **持久副作用**（落 `prefs.json`、跨会话存活），与「看一眼界面」不是一档风险。
///
/// 三档只有「更宽」一个方向，没有交集问题：`Configure` 蕴含 `Tune`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum AppScope {
    /// 全拒（缺省）。
    #[default]
    Deny,
    /// 读快照 + 改面板可见性（内存态，不落盘）。
    Tune,
    /// 再加偏好写入（落 `prefs.json`）。
    Configure,
}

impl AppScope {
    /// 能否读应用状态快照（`Tune` 起即可）。
    pub fn allows_view(self) -> bool {
        self >= AppScope::Tune
    }

    /// 能否写偏好（只有 `Configure`）。
    pub fn allows_configure(self) -> bool {
        self >= AppScope::Configure
    }

    /// 取更宽的一档。两个来源（脚本头 / `--allow`）合并时用。
    pub fn union(self, other: AppScope) -> AppScope {
        if self >= other {
            self
        } else {
            other
        }
    }

    /// 写进拒绝信息 / `summary()` 的名字。
    pub fn name(self) -> &'static str {
        match self {
            AppScope::Deny => "deny",
            AppScope::Tune => "tune",
            AppScope::Configure => "configure",
        }
    }
}

/// 一次运行的授权集合。字段都是「放行」而不是「禁止」——加能力必须是显式动作。
/// 唯一例外是 [`Capabilities::app`]：它不是 bool 而是三档 [`AppScope`]，
/// 因为「读状态」与「写持久偏好」的风险不同级（同一条理由见 `ui` 为什么不并进它）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub fs: FsScope,
    /// 允许起外部进程。
    pub process: bool,
    /// 允许**读**剪贴板（写不需要授权：脚本往剪贴板放一段文本不是读取用户秘密）。
    pub clipboard: bool,
    /// 允许控制窗口（显隐 / 退出 / 取选区）。
    pub ui: bool,
    /// 应用级状态（面板 / 布局 / 偏好）。见 [`AppScope`]。
    pub app: AppScope,
}

impl Capabilities {
    /// 全拒。脚本没有声明、入口没有给 `--allow` 时就是这个。
    pub const NONE: Capabilities = Capabilities {
        fs: FsScope::Deny,
        process: false,
        clipboard: false,
        ui: false,
        app: AppScope::Deny,
    };

    /// 加一项授权（幂等）。
    pub fn grant(&mut self, request: &CapRequest) {
        match request {
            CapRequest::FsRead(path) => self.fs.grant(path.clone(), false),
            CapRequest::FsWrite(path) => self.fs.grant(path.clone(), true),
            CapRequest::Process => self.process = true,
            CapRequest::Clipboard => self.clipboard = true,
            CapRequest::Ui => self.ui = true,
            CapRequest::App(scope) => self.app = self.app.union(*scope),
        }
    }

    /// 合并另一份授权（脚本头 + 入口参数 + 用户手选三个来源）。
    ///
    /// **按根**合并：另一份授权里只读的根不能因为它同时还有别的写根就变成可写。
    pub fn merge(&mut self, other: &Capabilities) {
        for root in other.fs.read_roots() {
            self.fs.grant(root.clone(), false);
        }
        for root in other.fs.write_roots() {
            self.fs.grant(root.clone(), true);
        }
        self.process |= other.process;
        self.clipboard |= other.clipboard;
        self.ui |= other.ui;
        self.app = self.app.union(other.app);
    }

    /// 一句话描述当前授予面（界面勾选框与错误信息共用）。
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.push(self.fs_summary());
        parts.push(format!("进程：{}", yn(self.process)));
        parts.push(format!("剪贴板读：{}", yn(self.clipboard)));
        parts.push(format!("窗口：{}", yn(self.ui)));
        parts.push(format!("应用：{}", self.app.name()));
        parts.join("；")
    }

    /// 读一个路径是否被允许。`path` 必须是**已规范化**的绝对路径。
    pub fn check_read(&self, path: &Path) -> Result<(), Denied> {
        self.check_fs(path, false)
    }

    /// 写一个路径是否被允许。
    pub fn check_write(&self, path: &Path) -> Result<(), Denied> {
        self.check_fs(path, true)
    }

    fn check_fs(&self, path: &Path, write: bool) -> Result<(), Denied> {
        let requested = path.display().to_string();
        let capability = if write { "fs.write" } else { "fs.read" };
        // 相对路径 / 含 `..` 的路径：**比较之前**拒。先比再规范会让
        // `logs/../../etc/passwd` 在 `logs` 根下通过。
        if !path.is_absolute() {
            return Err(Denied {
                capability,
                requested,
                granted: format!("{}（路径必须是绝对路径）", self.fs_summary()),
            });
        }
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(Denied {
                capability,
                requested,
                granted: format!("{}（路径不得含 ..）", self.fs_summary()),
            });
        }
        let granted = self.fs_summary();
        let FsScope::Roots {
            read_roots,
            write_roots,
        } = &self.fs
        else {
            return Err(Denied {
                capability,
                requested,
                granted,
            });
        };
        if write {
            if write_roots.iter().any(|root| path.starts_with(root)) {
                return Ok(());
            }
            // 落在读根里但不在写根里：说清「只授了读」，让脚本能改走别的分支。
            let only_read = read_roots.iter().any(|root| path.starts_with(root));
            return Err(Denied {
                capability,
                requested,
                granted: if only_read {
                    format!("{granted}（只授了读）")
                } else {
                    granted
                },
            });
        }
        // 写授权同时蕴含读（写一个文件总得能看见它）。
        if read_roots
            .iter()
            .chain(write_roots.iter())
            .any(|root| path.starts_with(root))
        {
            Ok(())
        } else {
            Err(Denied {
                capability,
                requested,
                granted,
            })
        }
    }

    fn fs_summary(&self) -> String {
        match &self.fs {
            FsScope::Deny => "文件：拒绝".to_string(),
            FsScope::Roots {
                read_roots,
                write_roots,
            } => {
                let mut parts: Vec<String> = Vec::new();
                if !write_roots.is_empty() {
                    parts.push(format!("读写 {}", display_roots(write_roots.iter())));
                }
                let read_only: Vec<&PathBuf> = read_roots
                    .iter()
                    .filter(|root| !write_roots.contains(root))
                    .collect();
                if !read_only.is_empty() {
                    parts.push(format!("只读 {}", display_roots(read_only.into_iter())));
                }
                format!("文件：{}", parts.join("；"))
            }
        }
    }

    pub fn check_process(&self) -> Result<(), Denied> {
        self.flag(self.process, "process", "起外部进程")
    }

    pub fn check_clipboard(&self) -> Result<(), Denied> {
        self.flag(self.clipboard, "clipboard.read", "读剪贴板")
    }

    pub fn check_ui(&self) -> Result<(), Denied> {
        self.flag(self.ui, "ui", "控制窗口")
    }

    /// 读应用状态快照（`Tune` 起放行）。
    pub fn check_app_view(&self) -> Result<(), Denied> {
        if self.app.allows_view() {
            Ok(())
        } else {
            Err(Denied {
                capability: "app",
                requested: "读应用状态快照（面板 / 布局 / 窗口 / 语言）".to_string(),
                granted: format!("app={}", self.app.name()),
            })
        }
    }

    /// 写偏好（只有 `Configure` 放行）。
    pub fn check_app_configure(&self) -> Result<(), Denied> {
        if self.app.allows_configure() {
            Ok(())
        } else {
            Err(Denied {
                capability: "app",
                requested: "写偏好（落 prefs.json）".to_string(),
                granted: format!("app={}", self.app.name()),
            })
        }
    }

    fn flag(&self, granted: bool, capability: &'static str, what: &str) -> Result<(), Denied> {
        if granted {
            Ok(())
        } else {
            Err(Denied {
                capability,
                requested: what.to_string(),
                granted: self.summary(),
            })
        }
    }
}

fn display_roots<'a>(roots: impl Iterator<Item = &'a PathBuf>) -> String {
    roots
        .map(|root| root.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn yn(value: bool) -> &'static str {
    if value {
        "允许"
    } else {
        "拒绝"
    }
}

/// 一次授权请求（三个来源解析出来的同一种形状）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapRequest {
    /// 读某个目录 / 文件（`fs=read:<path>`）。
    FsRead(PathBuf),
    /// 写某个目录 / 文件（`fs=write:<path>`）。
    FsWrite(PathBuf),
    /// 起外部进程（`process`）。
    Process,
    /// 读剪贴板（`clipboard`）。
    Clipboard,
    /// 控制窗口（`ui`）。
    Ui,
    /// 应用级状态（`app` / `app=configure`）。
    App(AppScope),
}

/// 拒绝。`Display` 给脚本作者看，所以三件事按顺序说：缺什么、请求什么、有什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Denied {
    /// 能力名（`fs.read` / `fs.write` / `process` / `clipboard.read` / `ui` / `app`）。
    pub capability: &'static str,
    /// 请求的对象（路径 / 动作）。
    pub requested: String,
    /// 当前已授予的面。
    pub granted: String,
}

impl fmt::Display for Denied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "没有 {} 授权：请求 {}；本次运行已授予（{}）",
            self.capability, self.requested, self.granted
        )
    }
}

impl std::error::Error for Denied {}

/// 授权声明的语法错误。
///
/// 为什么不是 `String`：`term-core` 是语义层，跨 crate 的错误要能让调用方按
/// 类别反应，而 `scripts/verify.sh` 4i 也要求这一层不出现 `Result<_, String>`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapError(pub String);

impl std::fmt::Display for CapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CapError {}

impl From<String> for CapError {
    fn from(message: String) -> Self {
        CapError(message)
    }
}

/// 脚本头里的授权声明前缀：`// @cap …`（也接受 `# @cap`）。
pub const CAP_HEADER: &str = "@cap";

/// 从脚本源码头部（第一条非空非注释语句之前）解析 `// @cap` 声明。
///
/// 只认**头部连续注释块**里的声明：脚本正文里出现 `// @cap` 时不再授予任何
/// 东西——否则一段被 `include` 进来的文本就能悄悄提权。
///
/// 错误带**行号与原文**：写错一行 `// @cap` 时，用户需要知道是哪一行哪一处，
/// 而这条错误会在脚本启动前就抛出来（跑起来之前就该拦住）。
pub fn parse_header(script: &str) -> Result<Vec<CapRequest>, CapError> {
    let mut out = Vec::new();
    for (index, line) in script.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(comment) = trimmed
            .strip_prefix("//")
            .or_else(|| trimmed.strip_prefix('#'))
        else {
            // 头部结束：第一条真正的语句。
            break;
        };
        let body = comment.trim();
        let Some(rest) = body.strip_prefix(CAP_HEADER) else {
            continue;
        };
        out.extend(
            parse_allow(rest)
                .map_err(|e| CapError(format!("第 {} 行的 @cap 声明有误：{e}", index + 1)))?,
        );
    }
    Ok(out)
}

/// 解析「授权规格」文本。两种来源共用：脚本头 `@cap` 之后的尾巴、CLI `--allow`。
///
/// 语法（逗号分隔，空项忽略）：
/// - `fs=read:<path>` / `fs=write:<path>`（可重复）
/// - `process` / `clipboard` / `ui`
/// - `app`（= `AppScope::Tune`）/ `app=configure`（写偏好，ADR-0027）
///
/// 缺省或未知项一律报错（不静默忽略）：`--allow proces` 静默变成「什么都没授」
/// 时，用户会以为是脚本不听话。
pub fn parse_allow(spec: &str) -> Result<Vec<CapRequest>, CapError> {
    let mut out = Vec::new();
    for raw in spec.split(',') {
        let item = raw.trim();
        if item.is_empty() {
            continue;
        }
        if item == "process" {
            out.push(CapRequest::Process);
        } else if item == "clipboard" {
            out.push(CapRequest::Clipboard);
        } else if item == "ui" {
            out.push(CapRequest::Ui);
        } else if item == "app" {
            // `app` 单独一项 = 最窄的可用档（读快照 + 面板置位）。
            out.push(CapRequest::App(AppScope::Tune));
        } else if let Some(rest) = item.strip_prefix("app=") {
            // 只认一个值：`app=configure`。写错档名报错，不回落成 `Tune`——
            // 静默降档会让「我明明要写偏好」变成「只读」，而脚本侧看不出差别。
            match rest.trim() {
                "configure" => out.push(CapRequest::App(AppScope::Configure)),
                other => {
                    return Err(CapError(format!(
                        "不认识的 app 档 {other:?}（只支持 app=configure；只读用单项 app）"
                    )))
                }
            }
        } else if let Some(rest) = item.strip_prefix("fs=") {
            // `read:/a` / `write:/a:/b`? 只接受单个模式 + 单个路径；
            // 多个根就写多项（`fs=read:/a,fs=read:/b`），不发明分隔符。
            let (mode, path) = rest.split_once(':').ok_or_else(|| {
                CapError(format!(
                    "{item:?} 缺少路径（应为 fs=read:<路径> 或 fs=write:<路径>）"
                ))
            })?;
            let path = path.trim();
            if path.is_empty() {
                return Err(CapError(format!("{item:?} 的路径是空的")));
            }
            match mode {
                "read" => out.push(CapRequest::FsRead(PathBuf::from(path))),
                "write" => out.push(CapRequest::FsWrite(PathBuf::from(path))),
                other => {
                    return Err(CapError(format!(
                        "不认识的 fs 模式 {other:?}（只支持 read / write）"
                    )))
                }
            }
        } else {
            return Err(CapError(format!(
                "不认识的授权项 {item:?}（可用：fs=read:<路径>、fs=write:<路径>、process、clipboard、ui、app、app=configure）"
            )));
        }
    }
    Ok(out)
}

/// 把一串请求变成一份授权集合。
pub fn capabilities_from(requests: &[CapRequest]) -> Capabilities {
    let mut caps = Capabilities::NONE;
    for request in requests {
        caps.grant(request);
    }
    caps
}

#[cfg(test)]
#[path = "script_caps/caps_tests.rs"]
mod tests;
