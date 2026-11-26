//! 目录树的递归传输（`-r` 下载 / 上传）。
//!
//! 分两半，和 [`crate::transfer`] 的分法一致：
//!
//! - **纯函数**：相对路径的拼装、本地/远端父目录推导。单测不需要连任何服务端。
//! - **干活的那一半**：展开一棵目录树成有序的「建目录 + 传文件」计划，然后按序
//!   执行。展开是 IO（`read_dir` / `fs::read_dir`），但**展开成什么样**是策略——
//!   策略写在这里，才好单测、才好在出错时报出「卡在哪一步」。
//!
//! 两条刻意的规则：
//!
//! 1. **符号链接一律跳过**，两个方向都是。SFTP 的 `readdir` 与本地 `fs::read_dir`
//!    的 file_type 都是 lstat 语义，链接显形为 symlink；跟着链接走，`a -> .`
//!    这种自指环能把遍历转成死循环。要传链接本体，先把它当文件单独传。
//! 2. **先建全部目录、再传文件**。SFTP 的 `mkdir` 不是 `mkdir -p`，逐级建才能
//!    保证深路径不因「父目录不存在」失败；目录先于文件，则文件的落点永远已存在。

use std::path::{Path, PathBuf};

use russh_sftp::client::SftpSession;

use crate::cancel::CancelToken;
use crate::dest::{local_dest_for, remote_dest_for};
use crate::entry::{self, collect};
use crate::transfer::{self, Direction, TransferError};

/// 一棵树的展开计划：相对根的条目列表，目录在前、文件在后，同层按名字排序。
///
/// `rel` 统一用 `/` 分隔——远端路径本来就是 POSIX，本地落盘时换成平台分隔符。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// 相对根的路径：`""` 是根本身，`"sub"`、`"sub/deep"` 是子目录。
    pub dirs: Vec<DirItem>,
    /// 要传的文件：相对路径 + 大小（进度用；服务端没报就是 0）。
    pub files: Vec<FileItem>,
}

/// 计划里的一个目录。`mtime` 是**源端**的修改时间，供 `preserve` 在树传完后
/// 写到对端；`None` = 服务端没报，不写（写 0 等于把时间设成 1970）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirItem {
    pub rel: String,
    pub mtime: Option<i64>,
}

/// 计划里的一个文件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileItem {
    pub rel: String,
    pub size: u64,
}

impl Plan {
    /// 目录 + 文件的总数，给「第 N/M 个」显示用。
    pub fn total_items(&self) -> usize {
        self.dirs.len() + self.files.len()
    }
}

/// 展开远端目录树。`root` 必须是目录——调用方（CLI）已经判定过，
/// 这里再判一次是因为事件路径上不该假设别人判过。
pub async fn plan_remote(sftp: &SftpSession, root: &str) -> Result<Plan, String> {
    let mut plan = Plan::default();
    // `to_string()` 只在这一层，与 `plan_local` 同一条理由：把结构化原因压成
    // 用户文案是**调用方的历史契约**，新接口原样把 `ScanError` 交出去。
    for (rel, e) in walk_remote(sftp, root).await.map_err(|e| e.to_string())? {
        if e.is_dir {
            plan.dirs.push(DirItem { rel, mtime: e.mtime });
        } else {
            plan.files.push(FileItem { rel, size: e.size });
        }
    }
    Ok(plan)
}

/// 遍历一棵远端目录树，按 `plan_remote` 的既定顺序吐出 `(相对路径, 条目)`。
///
/// 与 [`walk_local`] 对称：抽出来给两个消费者共用——传输计划（[`plan_remote`]，
/// 只要路径与大小）与目录同步的清单采集（[`scan_remote_entries`]，还要 `mtime`）。
/// 两个消费者共用同一趟遍历，产出的「同步看到的树」与「要传的树」才逐条一致。
///
/// 顺序与 `plan_remote` 原来完全一致（`collect` 排序后逆序入栈的深度优先），
/// 所以把 `plan_remote` 改成调用本函数是**零行为变化**的重构。
async fn walk_remote(
    sftp: &SftpSession,
    root: &str,
) -> Result<Vec<(String, crate::entry::Entry)>, ScanError> {
    let meta = sftp
        .metadata(root)
        .await
        .map_err(|source| ScanError::RemoteReadDir {
            path: root.to_string(),
            source,
        })?;
    if !meta.is_dir() {
        return Err(ScanError::RemoteNotADirectory {
            root: root.to_string(),
        });
    }
    let mut out: Vec<(String, crate::entry::Entry)> = Vec::new();
    // 显式栈做深度优先（async fn 不能直接递归，而且显式栈天然没有深度上限）。
    // 入栈时倒序，弹出顺序 = 排序顺序，产出的计划是确定性的。
    let mut stack: Vec<(String, String)> = vec![(root.to_string(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let rd = sftp
            .read_dir(&dir)
            .await
            .map_err(|source| ScanError::RemoteReadDir {
                path: dir.clone(),
                source,
            })?;
        for e in collect(rd).into_iter().rev() {
            if e.is_symlink {
                continue; // 规则 1：链接跳过，避免自指环把遍历转成死循环。
            }
            // 规则 2：只接受「干净的一段名字」。服务端在 `read_dir` 里返回什么
            // 都合法（`..`、`a/b`、空串都算），而这条名字会进 `rel`，再被
            // `local_path_for` 拼成本地落点——放进 `..` 就写出了用户指定的目录。
            // 宁可不传这一条，也不写到别处。
            if !is_plain_name(&e.name) {
                continue;
            }
            let child_rel = rel_child(&rel, &e.name);
            if e.is_dir {
                stack.push((entry::join(&dir, &e.name), child_rel.clone()));
            }
            out.push((child_rel, e));
        }
    }
    Ok(out)
}

/// 扫一棵树的失败。**分类而不是一句字符串**：调用方要能区分「根不对」与「读到一半
/// 失败」——前者该提示用户换目录，后者该提示权限 / 磁盘 / 网络。
///
/// 本地与远端各三个变体，语义一一对应（`NotADirectory` / `ReadDir` 对
/// `RemoteNotADirectory` / `RemoteReadDir`）；本地多一个 `Entry`，因为
/// `std::fs::metadata` 是单独一次调用，而远端每次 `read_dir` 的元数据随目录项一起来。
///
/// `plan_local` / `plan_remote` 是历史接口，仍返回 `String`（它们的调用方走
/// `Event::Failed { message }` 那条通道，整体改造属独立重构）。**新代码请用
/// [`ScanError`]**，别把「错误边界 String 化」的基线往上抬。
#[derive(Debug)]
pub enum ScanError {
    /// 根不是目录（含不存在、无权限、是文件）。
    NotADirectory {
        /// 用户给的根路径（原样保留，便于回显）。
        root: String,
    },
    /// 读目录失败。
    ReadDir {
        /// 出错的目录。
        path: PathBuf,
        /// 底层原因。
        source: std::io::Error,
    },
    /// 读单个条目的类型或元数据失败。
    Entry {
        /// 出错的条目。
        path: PathBuf,
        /// 底层原因。
        source: std::io::Error,
    },
    /// 远端根不是目录（含不存在、无权限、是文件）。
    RemoteNotADirectory {
        /// 用户给的根路径（原样保留，便于回显）。
        root: String,
    },
    /// 读远端目录失败。
    ///
    /// `source` 带的是 `russh_sftp` 的错误而不是 `io::Error`：远端失败的原因
    /// （服务端权限、协议状态码、超时）在这一层已经变成协议语义，压成字符串就
    /// 再也分不出「等会儿重试」与「这个目录就是没有」。
    RemoteReadDir {
        /// 出错的远端目录。
        path: String,
        /// 底层原因。
        source: russh_sftp::client::error::Error,
    },
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanError::NotADirectory { root } => {
                write!(f, "本地路径不是目录：{root}（-r 只传目录，单文件请去掉 -r）")
            }
            ScanError::ReadDir { path, source } => {
                write!(f, "读不到本地目录 {}：{source}", path.display())
            }
            ScanError::Entry { path, source } => {
                write!(f, "读不到 {} 的类型：{source}", path.display())
            }
            // 文案与早期 `plan_remote` 里那句逐字一致：它是对外可见的
            // （`Event::Failed { message }` 会原样显示），换个说法就是换行为。
            ScanError::RemoteNotADirectory { root } => {
                write!(f, "远端路径不是目录：{root}（-r 只传目录，单文件请去掉 -r）")
            }
            ScanError::RemoteReadDir { path, source } => {
                write!(f, "读不到远端目录 {path}：{source}")
            }
        }
    }
}

impl std::error::Error for ScanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ScanError::NotADirectory { .. } | ScanError::RemoteNotADirectory { .. } => None,
            ScanError::ReadDir { source, .. } | ScanError::Entry { source, .. } => Some(source),
            ScanError::RemoteReadDir { source, .. } => Some(source),
        }
    }
}

/// 展开本地目录树。纯本地 IO，不需要连接。
pub fn plan_local(root: &str) -> Result<Plan, String> {
    let mut plan = Plan::default();
    // `to_string()` 只在这一层：把结构化原因压成用户文案是**调用方的历史契约**，
    // 新接口（`scan_local_entries`）原样把 `ScanError` 交出去。
    for (rel, e) in walk_local(root).map_err(|e| e.to_string())? {
        if e.is_dir {
            plan.dirs.push(DirItem { rel, mtime: e.mtime });
        } else {
            plan.files.push(FileItem { rel, size: e.size });
        }
    }
    Ok(plan)
}

/// 遍历一棵本地目录树，按 `plan_local` 的既定顺序吐出 `(相对路径, 条目)`。
///
/// 抽出来给两个消费者共用：传输计划（[`plan_local`]，只要路径与大小）与目录同步的
/// 清单采集（[`scan_local_entries`]，还要 `mtime`）。两处共用同一趟遍历，是为了
/// 保证「同步看到的树」与「传上去的树」在顺序与跳过规则上逐条一致——各写一遍迟早
/// 会在某条边界上分叉（比如软链跳过）。
///
/// 顺序与 `plan_local` 原来完全一致（`sort_entries` 排序后逆序入栈的深度优先），
/// 所以把它改成调用本函数是**零行为变化**的重构，不是重新排一遍。
fn walk_local(root: &str) -> Result<Vec<(String, crate::entry::Entry)>, ScanError> {
    let root_path = Path::new(root);
    if !root_path.is_dir() {
        return Err(ScanError::NotADirectory {
            root: root.to_string(),
        });
    }
    let mut out: Vec<(String, crate::entry::Entry)> = Vec::new();
    let mut stack: Vec<(PathBuf, String)> = vec![(root_path.to_path_buf(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let rd = std::fs::read_dir(&dir).map_err(|source| ScanError::ReadDir {
            path: dir.clone(),
            source,
        })?;
        let items: Vec<std::fs::DirEntry> = rd
            .collect::<Result<_, _>>()
            .map_err(|source| ScanError::ReadDir {
                path: dir.clone(),
                source,
            })?;
        // 与远端同一条排序规则（目录在前、忽略大小写按名），两个方向产出的
        // 计划形状一致。
        let mut entries: Vec<crate::entry::Entry> = items
            .iter()
            .map(|i| {
                let ty = i.file_type().map_err(|source| ScanError::Entry {
                    path: i.path(),
                    source,
                })?;
                let meta = i.metadata().ok();
                let size = if ty.is_file() {
                    meta.as_ref().map(|m| m.len()).unwrap_or(0)
                } else {
                    0
                };
                Ok(crate::entry::Entry {
                    name: i.file_name().to_string_lossy().to_string(),
                    is_dir: ty.is_dir(),
                    is_symlink: ty.is_symlink(),
                    size,
                    mtime: meta
                        .as_ref()
                        .and_then(|m| m.modified().ok())
                        .and_then(crate::preserve::to_epoch_secs)
                        .map(i64::from),
                })
            })
            .collect::<Result<_, ScanError>>()?;
        entry::sort_entries(&mut entries);
        for e in entries.into_iter().rev() {
            if e.is_symlink {
                continue; // 规则 1：与远端同一条规则。
            }
            let child_rel = rel_child(&rel, &e.name);
            if e.is_dir {
                stack.push((dir.join(&e.name), child_rel.clone()));
            }
            out.push((child_rel, e));
        }
    }
    Ok(out)
}

/// 同步清单里的一行：相对根的路径 + 类型 + 大小 + 秒级 mtime。
///
/// 与 [`crate::entry::Entry`] 的区别是**路径口径**：`Entry` 只有文件名（每次列一个
/// 目录），这里带的是相对同步根的整条路径，正是 `term_xfer::sync::SyncEntry` 要的形状。
/// 本 crate 不依赖 `term-xfer`（那是个刻意零依赖的协议层），所以两边各自定义，
/// 由 `apps/desktop` 做一次三行映射——比为了一个结构体在 crate 之间拉一条新边便宜。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannedEntry {
    /// 相对根的路径，`/` 分隔，无前导 `/`，根自身不出现（根两边都有）。
    pub rel: String,
    pub is_dir: bool,
    /// 文件字节数；目录恒为 `0`。
    pub size: u64,
    /// Unix 秒。`None` = 读不到（权限、时钟异常），**不是** 1970 年。
    pub mtime: Option<i64>,
}

impl ScannedEntry {
    /// 从一个目录项造清单行。两端共用，所以「目录 size 归零」只有这一处。
    ///
    /// 服务端报的目录大小可能是 4096（块大小）也可能是 0，而同步层拿目录大小
    /// 做不了任何判断。不收在这里，就得让 `term_xfer` 那边每次先判 `is_dir`
    /// ——规则散成两份，迟早在某一条路径上漏掉。
    fn of(rel: String, e: crate::entry::Entry) -> Self {
        ScannedEntry {
            rel,
            is_dir: e.is_dir,
            size: if e.is_dir { 0 } else { e.size },
            mtime: e.mtime,
        }
    }
}

/// 采一份本地清单，给目录同步用。
///
/// 规则与 `plan_local` 完全同一趟遍历：软链跳过（不跟随，避免环与超大树）、
/// 目录在前、忽略大小写按名。根目录自身不进清单。
pub fn scan_local_entries(root: &str) -> Result<Vec<ScannedEntry>, ScanError> {
    Ok(walk_local(root)?
        .into_iter()
        .map(|(rel, e)| ScannedEntry::of(rel, e))
        .collect())
}

/// 采一份远端清单，给目录同步用。与 [`scan_local_entries`] 对称。
///
/// 规则与 `plan_remote` 完全同一趟遍历：软链跳过、目录前后按名、根自身不进清单。
/// `mtime` 来自服务端每条目自带的 `FileAttributes.mtime`（`collect` 已经抽好），
/// **没有额外的 per-file `stat`**——一个几千条目的目录不会因此变成几千次往返。
///
/// 服务端没报 mtime 时该项是 `None`。这时候「谁更新」无从判断，该由调用方
/// （`term_xfer::sync::plan` 的组装层）决定策略，这一层不许把 `None` 写成 `0`。
pub async fn scan_remote_entries(
    sftp: &SftpSession,
    root: &str,
) -> Result<Vec<ScannedEntry>, ScanError> {
    Ok(walk_remote(sftp, root)
        .await?
        .into_iter()
        .map(|(rel, e)| ScannedEntry::of(rel, e))
        .collect())
}

/// 这个远端条目名能不能当作路径的一段。
///
/// 排除三类：空串（拼出来是空段）、`.` / `..`（改变层级）、含 `/` 或 `\`
/// （一个名字冒充多级）。SFTP 协议不禁止服务端返回这些，所以判定放在
/// 「收到的字节」这一侧，而不是指望服务端守规矩。
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
}

/// 相对路径的下一级：`"" + "a"` → `"a"`，`"a" + "b"` → `"a/b"`。
fn rel_child(rel: &str, name: &str) -> String {
    if rel.is_empty() {
        name.to_string()
    } else {
        format!("{rel}/{name}")
    }
}

/// 相对路径 → 本地落点。根自身（`rel == ""`）落回根目录本身。
///
/// `.` / `..` 段直接丢掉，不再 `push`。正常路径走不到这里（`walk_remote`
/// 已在入口过滤），但这是个 `pub` 接口，第二道闸门比「调用方都守规矩」可靠：
/// `local_path_for("/out", "../etc/passwd")` 必须落在 `/out` 里面。
pub fn local_path_for(root: &str, rel: &str) -> PathBuf {
    let mut p = PathBuf::from(root);
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            continue;
        }
        p.push(part);
    }
    p
}

/// 相对路径 → 远端落点。根自己落回根；子路径补一个 `/`。
pub fn remote_path_for(root: &str, rel: &str) -> String {
    if rel.is_empty() {
        return root.to_string();
    }
    entry::join(root, rel)
}


/// 建一个远端目录（已存在不算错——`mkdir -p` 的语义）。
///
/// 为什么吞掉错误：重复建、权限拒绝……这些情况文件传输那一步会给出**更准**的
/// 错误（带路径），在这里报只会重复一遍。唯一想拦的「父目录不存在」在计划
/// 顺序上不会发生（目录先于文件、父先于子）。
async fn ensure_remote_dir(sftp: &SftpSession, path: &str) {
    let _ = sftp.create_dir(path).await;
}

/// 一次树传输的全部输入：计划、两端根路径、选项与两个上报口。
///
/// 收成结构是因为参数比单文件传输多出「两端根 + 文件收尾回调」——平铺就是
/// 九个参数，收进来后 `run` 只剩三个，而且以后加选项（比如「跳过隐藏文件」）
/// 不用再改一串签名。
pub struct Job<'a> {
    pub plan: &'a Plan,
    pub remote_root: &'a str,
    pub local_root: &'a str,
    pub resume: bool,
    pub preserve: bool,
    /// 目录 mtime 是否也保留。与 `preserve`（文件）分开：默认关，因为
    /// `setstat` 目录在不少服务端上不被支持，不该悄悄多一次失败往返。
    pub preserve_dir: bool,
    /// 是否把目标路径每一段的非法字符换成 `_`。只作用于目标侧，源端路径不动。
    pub fix_invalid: bool,
    /// 取消旗子：与单文件传输**同一条**，所以「传一棵树传到第 3 个文件时按取消」
    /// 在第 3 个文件的分片边界上生效，第 4 个不会开始。
    pub cancel: &'a CancelToken,
    /// 进度：`progress(远端路径, 已传字节, 该文件总大小)`。
    pub progress: &'a mut dyn FnMut(&str, u64, u64),
    /// 一个文件收尾：`file_done(远端路径, 落盘字节)`。
    pub file_done: &'a mut dyn FnMut(&str, u64),
}

/// 把一棵计划跑完（下载方向）。返回传输的总字节。
///
/// 进度按**文件**上报：每个文件是独立的进度流，这样 UI / CLI 不需要把
/// 多个文件的字节混在一起算。
async fn run_download(sftp: &SftpSession, job: &mut Job<'_>) -> Result<u64, TransferError> {
    let (remote_root, local_root) = (job.remote_root, job.local_root);
    // 树根本身也不在 plan.dirs 里（根是调用方给的），先把它建出来——
    // 根下第一个文件的 `File::create` 需要它存在。
    std::fs::create_dir_all(local_root).map_err(|e| format!("建本地目录 {local_root} 失败：{e}"))?;
    // 规则 2：先建全部目录（本地 create_dir_all 自带递归，按序建即可）。
    for d in &job.plan.dirs {
        std::fs::create_dir_all(local_dest_for(local_root, &d.rel, job.fix_invalid))
            .map_err(|e| format!("建本地目录 {} 失败：{e}", d.rel))?;
    }
    let mut total = 0u64;
    for f in &job.plan.files {
        let remote = remote_path_for(remote_root, &f.rel);
        let local = local_dest_for(local_root, &f.rel, job.fix_invalid);
        let local_s = local.to_string_lossy().to_string();
        let (resume, preserve) = (job.resume, job.preserve);
        let progress = &mut *job.progress;
        let bytes = transfer::download(
            sftp,
            &remote,
            &local_s,
            resume,
            preserve,
            job.cancel,
            &mut |transferred, t| progress(&remote, transferred, t),
        )
        .await?;
        (job.file_done)(&remote, bytes);
        total += bytes;
    }
    // 目录 mtime 必须**最后**写：建目录 / 传文件都会更新父目录的时间，
    // 在文件传完之前写会被后面的创建动作覆盖。
    if job.preserve_dir {
        for d in &job.plan.dirs {
            let path = local_dest_for(local_root, &d.rel, job.fix_invalid);
            crate::preserve::write_local_dir_mtime(
                &path,
                d.mtime.and_then(|m| u32::try_from(m).ok()),
            );
        }
    }
    Ok(total)
}

/// 把一棵计划跑完（上传方向）。返回传输的总字节。
async fn run_upload(sftp: &SftpSession, job: &mut Job<'_>) -> Result<u64, TransferError> {
    let (remote_root, local_root) = (job.remote_root, job.local_root);
    // 树根 + 全部子目录逐级建（SFTP 的 mkdir 不递归，计划的顺序保证父先于子）。
    ensure_remote_dir(sftp, remote_root).await;
    for d in &job.plan.dirs {
        ensure_remote_dir(sftp, &remote_dest_for(remote_root, &d.rel, job.fix_invalid)).await;
    }
    let mut total = 0u64;
    for f in &job.plan.files {
        let local = local_path_for(local_root, &f.rel);
        let local_s = local.to_string_lossy().to_string();
        let remote = remote_dest_for(remote_root, &f.rel, job.fix_invalid);
        let (resume, preserve) = (job.resume, job.preserve);
        let progress = &mut *job.progress;
        let bytes = transfer::upload(
            sftp,
            &local_s,
            &remote,
            resume,
            preserve,
            job.cancel,
            &mut |transferred, t| progress(&remote, transferred, t),
        )
        .await?;
        (job.file_done)(&remote, bytes);
        total += bytes;
    }
    // 与下载同一条理由：文件传完再写目录时间，否则会被 mkdir / 写文件覆盖。
    if job.preserve_dir {
        for d in &job.plan.dirs {
            let remote = remote_dest_for(remote_root, &d.rel, job.fix_invalid);
            crate::preserve::write_remote_mtime(
                sftp,
                &remote,
                d.mtime.and_then(|m| u32::try_from(m).ok()),
            )
            .await;
        }
    }
    Ok(total)
}

/// 按方向跑完一棵树。`background` 拿它分发，CLI 不需要知道内部分几支。
pub async fn run(
    sftp: &SftpSession,
    direction: Direction,
    job: &mut Job<'_>,
) -> Result<u64, TransferError> {
    match direction {
        Direction::Download => run_download(sftp, job).await,
        Direction::Upload => run_upload(sftp, job).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(rel: &str, size: u64) -> FileItem {
        FileItem {
            rel: rel.to_string(),
            size,
        }
    }

    #[test]
    fn relative_paths_nest_under_the_root() {
        assert_eq!(rel_child("", "a"), "a");
        assert_eq!(rel_child("a", "b"), "a/b");
        assert_eq!(rel_child("a/b", "c.txt"), "a/b/c.txt");
    }

    /// 远端条目名只接受「一段干净的名字」：`..` / 含分隔符 / 空串一律不是。
    ///
    /// 威胁面是真实的：`read_dir` 返回的 filename 由服务端决定，恶意或被控的
    /// 服务端可以给一条 `../evil`，拼进本地落点就写到目标根之外。
    #[test]
    fn remote_entry_names_must_be_a_single_plain_segment() {
        assert!(is_plain_name("a.txt"));
        assert!(is_plain_name("中文 名"));
        assert!(is_plain_name(".hidden"));

        assert!(!is_plain_name(""), "空名字不能当一段路径");
        assert!(!is_plain_name("."));
        assert!(!is_plain_name(".."), "`..` 会退到上一级");
        assert!(!is_plain_name("../evil"), "带 `..` 前缀同样要挡");
        assert!(!is_plain_name("a/b"), "一个名字不能冒充两级");
        assert!(!is_plain_name("a\\b"), "反斜杠在 Windows 上同样是分隔符");
    }

    /// 就算调用方手里已经有了脏 `rel`，本地落点也不许跑出根目录。
    #[test]
    fn local_targets_cannot_escape_the_root() {
        for rel in ["../evil", "a/../../evil", "..", "./../x"] {
            let p = local_path_for("/tmp/out", rel);
            assert!(
                p.starts_with("/tmp/out"),
                "{rel} 落到了根目录之外：{}",
                p.display()
            );
        }
    }

    #[test]
    fn local_targets_land_inside_the_root() {
        assert_eq!(
            local_path_for("/tmp/out", "sub/nested.txt"),
            PathBuf::from("/tmp/out/sub/nested.txt")
        );
        // 根自己落回根目录本身（建根目录时用）。
        assert_eq!(local_path_for("/tmp/out", ""), PathBuf::from("/tmp/out"));
    }

    #[test]
    fn remote_targets_join_with_a_single_separator() {
        assert_eq!(remote_path_for("/srv", "sub/a.txt"), "/srv/sub/a.txt");
        assert_eq!(remote_path_for("/srv/", "sub/a.txt"), "/srv/sub/a.txt");
        // 根自己落回根——`create_dir` 根目录是多余且有风险的（可能没权限）。
        assert_eq!(remote_path_for("/srv", ""), "/srv");
    }

    #[test]
    fn destination_paths_sanitize_each_segment_only_when_asked() {
        assert_eq!(
            local_dest_for("/tmp/out", "a/b?c.txt", false),
            PathBuf::from("/tmp/out/a/b?c.txt")
        );
        assert_eq!(
            local_dest_for("/tmp/out", "a/b?c.txt", true),
            PathBuf::from("/tmp/out/a/b_c.txt")
        );
        assert_eq!(remote_dest_for("/srv", "a/b?c", true), "/srv/a/b_c");
        // 根自己不带 rel，不加任何后缀（根目录名由调用方在入队前处理）。
        assert_eq!(remote_dest_for("/srv", "", true), "/srv");
    }

    #[test]
    fn empty_plan_reports_zero_items() {
        let plan = Plan::default();
        assert_eq!(plan.total_items(), 0);
        let plan = Plan {
            dirs: vec![
                DirItem { rel: "a".into(), mtime: None },
                DirItem { rel: "a/b".into(), mtime: Some(1_700_000_000) },
            ],
            files: vec![item("a/x", 1), item("a/b/y", 2)],
        };
        assert_eq!(plan.total_items(), 4);
    }

    /// 造一棵真目录树：`scratch/sub/a.txt`（3 字节）+ `scratch/b.txt`（1 字节）。
    ///
    /// 用真文件系统是因为这一层的行为就是「读目录」，用假数据测不出权限、mtime、
    /// 软链这些只有在真 FS 上才有的东西。目录名带 pid，跑完自己删。
    fn scratch_tree(tag: &str) -> PathBuf {
        let mut root = std::env::temp_dir();
        root.push(format!("zterm-walk-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).expect("建临时目录");
        std::fs::write(root.join("sub").join("a.txt"), b"abc").expect("写 a");
        std::fs::write(root.join("b.txt"), b"x").expect("写 b");
        root
    }

    #[test]
    fn scanning_a_local_tree_carries_relative_paths_sizes_and_mtimes() {
        let root = scratch_tree("scan");
        let got = scan_local_entries(root.to_str().unwrap()).expect("扫盘");
        let paths: Vec<&str> = got.iter().map(|e| e.rel.as_str()).collect();
        // 顺序就是遍历顺序本身：每个目录内按 `sort_entries` 排好后**逆序**出栈，
        // 深度优先。与 `plan_local` 共用同一趟遍历，所以这里断言的顺序同时就是
        // 「传输计划的 dirs / files 顺序」的守卫。
        assert_eq!(paths, ["b.txt", "sub", "sub/a.txt"]);

        let sub = got.iter().find(|e| e.rel == "sub").expect("子目录");
        assert!(sub.is_dir);
        assert_eq!(sub.size, 0, "目录的 size 恒为 0");
        assert!(sub.mtime.is_some(), "目录也要带 mtime，否则同步判不出结构变化");

        let a = got.iter().find(|e| e.rel == "sub/a.txt").expect("子文件");
        assert!(!a.is_dir);
        assert_eq!(a.size, 3);
        assert!(a.mtime.is_some_and(|t| t > 1_600_000_000), "秒级 Unix 时间戳");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_transfer_plan_carries_directory_mtimes_for_preserve() {
        let root = scratch_tree("plan-dir-mtime");
        let plan = plan_local(root.to_str().unwrap()).expect("展开计划");
        let sub = plan.dirs.iter().find(|d| d.rel == "sub").expect("子目录在计划里");
        assert!(
            sub.mtime.is_some_and(|t| t > 1_600_000_000),
            "目录 mtime 必须随计划一起走，否则目录时间戳开关没有数据源"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_non_directory_root_is_refused_instead_of_returning_an_empty_list() {
        // 空清单会被上层读成「本地什么都没有」，进而在同步里把远端全删/全下——
        // 宁可报错也不要给出一个看起来正常的空结果。
        assert!(scan_local_entries("/definitely/not/here").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_skipped_by_the_scan_too() {
        let root = scratch_tree("link");
        std::os::unix::fs::symlink(root.join("b.txt"), root.join("link.txt")).expect("建软链");
        let got = scan_local_entries(root.to_str().unwrap()).expect("扫盘");
        assert!(
            got.iter().all(|e| e.rel != "link.txt"),
            "软链必须跳过，否则跟随它可能造出环"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
