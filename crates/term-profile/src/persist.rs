//! 会话持久化：SQLite 会话库（产品里只有这一条路）。
//!
//! 之前这里是「一个 `user.sessions` 明文 JSON 文件」。那个形状的三个问题
//! 在 `term-store` 的文档里写着（不可查、整写整读、没地方放凭据引用）；这一层
//! 现在只是把 `Store` 接到「profile 目录在哪」这件事上。
//!
//! **没有迁移代码**：项目还没有外部用户，旧 JSON 不再读（用户拍板「不留历史包袱」）。
//! 于是好处很直接：只有一种格式，读失败就是读失败，不存在「半迁移」状态。
//!
//! 两件事仍然归这一层：会话库文件的定位（`sessions_path`），以及保险库文件的
//! 读写（`vault.ztv`）——密码的**加解密**在 `term-vault`，**引用**在
//! `term-store`，只有「文件在哪」在这里。
//!
//! 2026-11-12 底层收敛：**profile 目录本身的解析不在这一层了**，搬到了
//! `term-paths`。原因是会话日志（`term-io::log`）也要那个目录，而它不该为了
//! 一个路径反过来依赖带 SQLite 的持久化层。`profile_dir` 在这里保留再导出，
//! 既有调用路径不变。
//!
//! # 原子写的临时文件名必须唯一
//!
//! 原子写是「同目录写临时文件再 `rename`」。此前五个写点都用固定的
//! `xxx.tmp` 名，单进程串行没问题；但 `Application.NewWindow` 之后会有**两个
//! 进程共享同一份 profile 目录**（`WINDTERM_PROFILE_DIR` 按产品口径保留继承），
//! 交错写盘时 A 的 `rename` 可能改名掉 B 刚写的临时文件，造成内容错乱或写丢失。
//! 现在统一走 [`unique_tmp`] / [`write_atomic`]：pid + 纳秒 + 进程内序号，
//! 照 `term-ssh/src/known_hosts_admin.rs::write_atomic` 的范式，零新依赖。
//! rename 失败会 `remove_file` 清掉临时文件，目录里不许留半截账本。
//!
//! # 跨进程 load-modify-save 的 best-effort 冲突提示
//!
//! 唯一临时名只保证「两次原子写不互相踩临时文件」，挡不住更常见的那次事故：
//! 两个进程都 load 了旧 `prefs.json`，各自改一个键再 save，后写的那份把先写
//! 的键整份盖回去（静默回退）。[`write_atomic_if_unchanged`] 在 rename 前再读
//! 一次文件指纹，与调用方 load 时记下的基线比对；不一致就返回
//! [`AtomicWriteError::Conflict`]，由 `prefs::save` 翻译成明确错误让 UI 提示用户
//! 重做，绝不静默覆盖。
//!
//! 这是 best-effort 的提示，**不是 CAS、也不是事务**：读指纹与 rename 之间
//! 仍留有一个极短的窗口，恰好挤进来的写仍可能被盖掉。它挡的是「别人已经
//! 改过盘、而我手里是陈旧快照」这类日常事故，不承诺串行化整段事务。
//!
//! # 落盘权限：凭据文件不能按 umask 创建
//!
//! `vault.ztv` 装着加密后的全部凭据，同目录的 `prefs.json` / `command_history.json`
//! / `layout.json` 也带用户数据。此前临时文件走 `std::fs::write`，权限是
//! `0666 & ~umask`（常见 `0644`），同机其它用户可读；`create_dir_all` 建的 profile
//! 目录同理（`0755`，别人能列目录）。现在 [`write_atomic`] 在 Unix 上用
//! `OpenOptionsExt::mode(0o600)` 建临时文件并显式 `set_permissions(0o600)`，
//! `rename` 把这个权限带到目标文件；[`save_vault_to`] 把 profile 目录收紧到 `0700`。
//! 显式 `set_permissions` 不是多余的：`mode` 只在**新建**时生效，umask 只能减权限，
//! 预置的同名临时文件也可能带着宽松权限。
//!
//! Windows 没有 Unix mode：profile 目录在当前用户目录（`%APPDATA%`）下，隔离由
//! 系统目录 ACL 承担，标准库也没有可移植的 ACL API，故此处不另做 ACL 操作。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use term_core::session::SessionConfig;

/// profile 目录 —— 规则本体在 [`term_paths::profile_dir`]，这里只再导出。
pub use term_paths::profile_dir;

/// 会话库文件名（在 profile 目录下）。
pub const SESSIONS_FILE: &str = "sessions.db";

/// 保险库文件名（在 profile 目录下）。扩展名是自造的，用来和「普通数据文件」区分开。
pub const VAULT_FILE: &str = "vault.ztv";

/// 会话库的完整路径。
pub fn sessions_path() -> Option<PathBuf> {
    Some(profile_dir()?.join(SESSIONS_FILE))
}

/// 保险库的完整路径。
pub fn vault_path() -> Option<PathBuf> {
    Some(profile_dir()?.join(VAULT_FILE))
}

// --- 原子写：五个写点共用，别再各写一份 SystemTime ---

/// 进程内序号。纳秒在 macOS 上不是真纳秒分辨率，同一进程两个线程可能取到
/// 同一个值；加上序号才能保证「并发写同一路径」时临时名两两不同。
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 与 `path` **同目录**的唯一临时名：`.<文件名>.tmp-<pid>-<纳秒>-<序号>`。
///
/// 同目录是硬要求：跨文件系统的 `rename` 不是原子操作，会退化成「复制 + 删除」，
/// 断电就留半截文件。pid 区分两个进程，序号区分同一进程里的两个线程；单独靠
/// pid 或单独靠纳秒都不够（见模块头与 `TMP_SEQ` 的注释）。
pub(crate) fn unique_tmp(path: &Path) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let base = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    path.with_file_name(format!(
        ".{base}.tmp-{}-{nanos}-{seq}",
        std::process::id()
    ))
}

/// 把 `bytes` 原子地写进 `path`：先写唯一临时文件，再 `rename`。
///
/// 两条失败路径都 `remove_file` 清理：`write` 失败可能已经留下半截文件，
/// `rename` 失败（例如 Windows 上目标已存在）更不能把临时文件丢在目录里。
/// 返回 `bool` 与这一层既有的「尽力而为、失败不崩」口径一致。
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> bool {
    let tmp = unique_tmp(path);
    if !write_tmp_private(&tmp, bytes) {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    true
}

/// 写临时文件，并在 Unix 上把权限收紧到 `0600`。
///
/// `rename` 继承临时文件的权限，所以收紧临时文件即收紧最终文件。两条失败路径
/// 仍由 [`write_atomic`] 统一清理临时文件。
fn write_tmp_private(tmp: &Path, bytes: &[u8]) -> bool {
    #[cfg(unix)]
    {
        use std::io::Write;
        let written = (|| -> std::io::Result<()> {
            // 临时名含 pid + 纳秒 + 序号，但仍是「可预测」而不是密码学随机；
            // 拒绝符号链接，免得攻击者预置一个链接把内容写到别处。
            if let Ok(meta) = std::fs::symlink_metadata(tmp) {
                if meta.file_type().is_symlink() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "refusing to write through a symlink",
                    ));
                }
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(tmp)?;
            file.write_all(bytes)?;
            // mode 只管新建；已存在的同名文件或更严的 umask 都要靠这一步兜底。
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
        })();
        written.is_ok()
    }
    #[cfg(not(unix))]
    {
        // Windows 无 Unix mode，隔离靠 profile 目录的用户 ACL（见模块头）。
        std::fs::write(tmp, bytes).is_ok()
    }
}

/// 创建 `path` 的父目录并在 Unix 上收紧到 `0700`。
///
/// `create_dir_all` 给的是 `0777 & ~umask`（常见 `0755`）；已经存在的目录也收紧，
/// 这是安全加固，不是「尊重用户原有权限」。Windows 靠 `%APPDATA%` 的用户 ACL。
fn ensure_private_parent(path: &Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }
}

/// 文件当前**字节内容**的指纹；文件不存在 / 读不出来都是 `None`。
///
/// 用固定种子的 `DefaultHasher`（不是 `RandomState`）：同一进程里两次读同一份
/// 字节必须得到同一个值，否则乐观锁永远判冲突。指纹只在本进程内比较，不落盘、
/// 不跨进程交换，因此哈希算法的跨版本稳定性不是契约。
pub(crate) fn file_fingerprint(path: &Path) -> Option<u64> {
    std::fs::read(path).ok().as_deref().map(fingerprint_bytes)
}

/// 同一份字节的指纹。调用方已经读过一轮字节时用它，不必再读第二次文件
/// （两次读之间的窗口正是乐观锁要防的东西）。
pub(crate) fn fingerprint_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// 只在 `path` 的当前内容等于 `expected` 时才原子写（best-effort）。
///
/// `expected = None` 表示调用方没有基线（例如第一次写、或手改文件读不出来），
/// 此时退化为普通原子写。返回写完后新内容的指纹，调用方把它记成下一次的
/// 基线；冲突与 IO 失败分开报，让 UI 能说出「有人改过，请重做」而不是
/// 笼统的「保存失败」。
///
/// 注意这是**尽力而为的检测**，不是 CAS / 事务：读指纹与 rename 之间仍有
/// 一个极短的窗口。对 `prefs.json` 这种「用户点一下才写一次」的文件足够；
/// 真要串行化整段 load-modify-save，得让调用方持有锁跨过 load 与 save，
/// 那是另一刀。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AtomicWriteError {
    /// 基线已过期：盘上内容在 load 之后被别的进程（或用户手改）动过。
    Conflict,
    /// 临时文件写失败或 rename 失败。
    Io,
}

pub(crate) fn write_atomic_if_unchanged(
    path: &Path,
    expected: Option<u64>,
    bytes: &[u8],
) -> Result<u64, AtomicWriteError> {
    if let Some(want) = expected {
        if file_fingerprint(path) != Some(want) {
            return Err(AtomicWriteError::Conflict);
        }
    }
    if !write_atomic(path, bytes) {
        return Err(AtomicWriteError::Io);
    }
    Ok(fingerprint_bytes(bytes))
}

/// 测试用：列出目录里所有临时文件残留。只在 `cfg(test)` 下编译。
///
/// 匹配 `.tmp` 而不只是 `.tmp-`：既要抓新方案 `*.tmp-<pid>-…` 的残留，也要抓
/// 退回旧固定名 `*.tmp` 的残留——否则「换回固定名」时这条断言反而看不见它。
#[cfg(test)]
pub(crate) fn tmp_leftovers(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().contains(".tmp") {
            out.push(entry.path());
        }
    }
    out
}

/// 读会话列表与分组。
///
/// `None` = 还没有库（第一次运行）或库读不出来——两种情况调用方的动作一样：
/// 用出厂默认值开一个窗口，等用户真的保存时才建库。
pub fn load() -> Option<(Vec<String>, Vec<SessionConfig>)> {
    load_from(&sessions_path()?)
}

/// 保存会话列表与分组。
///
/// 失败**不**让窗口崩：会话列表是可用性资产，不是数据资产（真丢了重新加两条）。
/// 但也不装作成功——错误打到 stderr，便于用户报障时看见。
pub fn save(groups: &[String], sessions: &[SessionConfig]) {
    let Some(path) = sessions_path() else { return };
    save_to(&path, groups, sessions);
}

/// 这条会话的密码存在保险库的哪一条里（`None` = 没存，连接时现问）。
pub fn vault_entry(session: &str) -> Option<String> {
    let path = sessions_path()?;
    if !path.is_file() {
        return None;
    }
    term_store::Store::open(&path)
        .ok()?
        .vault_entry(session)
        .ok()?
}

/// 记下（或清掉）一条会话的凭据引用。
///
/// 顺序上必须**先** `save()` 再调这个：`set_vault_entry` 是一次 `UPDATE`，
/// 行不存在就什么也不会发生（`Store` 的约定：缺行不算错）。
pub fn set_vault_entry(session: &str, entry: Option<&str>) -> bool {
    let Some(path) = sessions_path() else {
        return false;
    };
    term_store::Store::open(&path)
        .ok()
        .and_then(|mut store| store.set_vault_entry(session, entry).ok())
        .unwrap_or(false)
}

/// 保险库文件存在吗（决定解锁对话框是「创建」还是「解锁」）。
pub fn has_vault() -> bool {
    vault_path().map(|p| p.is_file()).unwrap_or(false)
}

/// 读保险库的全部字节（加密后的，不认识里面的内容）。
pub fn load_vault() -> Option<Vec<u8>> {
    std::fs::read(vault_path()?).ok()
}

/// 写回保险库（`Vault::seal()` 的输出）。
pub fn save_vault(bytes: &[u8]) -> bool {
    let Some(path) = vault_path() else {
        return false;
    };
    save_vault_to(&path, bytes)
}

/// `save_vault` 的路径无关形态：留出来是为了让原子写这一段真的能被单测打到，
/// 而不是在测试里手工模拟一遍 `write` + `rename`（那测的是测试自己）。
fn save_vault_to(path: &Path, bytes: &[u8]) -> bool {
    // 保险库所在目录只许属主进入；目录里所有文件也经由 write_atomic 建成 0600。
    ensure_private_parent(path);
    // 先写临时文件再改名：写到一半断电，用户丢的是「这次改动」，
    // 不是「整个保险库」——所有条目都在同一个文件里，这一点很重要。
    write_atomic(path, bytes)
}

// --- 路径无关的实现（测试直接调这两个，不用改全局环境变量）---

fn load_from(path: &Path) -> Option<(Vec<String>, Vec<SessionConfig>)> {
    // 先看文件在不在：`Store::open` 会顺手建库，那会让「第一次运行」
    // 和「读失败」变成同一件事（都会返回一个空列表）。
    if !path.is_file() {
        return None;
    }
    let store = term_store::Store::open(path).ok()?;
    let snapshot = store.load().ok()?;
    Some(normalize(snapshot.groups, snapshot.sessions))
}

fn save_to(path: &Path, groups: &[String], sessions: &[SessionConfig]) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match term_store::Store::open(path) {
        Ok(mut store) => {
            if let Err(e) = store.save(groups, sessions) {
                eprintln!("保存会话库失败：{e}");
            }
        }
        Err(e) => eprintln!("打开会话库失败：{e}"),
    }
}

/// 分组列表与「会话引用了但没声明的分组」对齐，空会话列表补一条本地 shell。
///
/// 这一层是**投影前的最后一次整理**：库里存的是用户操作的结果，可能有不一致
/// （删了分组但会话还在里面）。补齐而不是报错——树里少一个文件夹比少一条会话好。
fn normalize(
    groups: Vec<String>,
    mut sessions: Vec<SessionConfig>,
) -> (Vec<String>, Vec<SessionConfig>) {
    let mut out: Vec<String> = Vec::new();
    for g in groups {
        if !g.trim().is_empty() && !out.contains(&g) {
            out.push(g);
        }
    }
    for s in &sessions {
        if !s.group.trim().is_empty() && !out.contains(&s.group) {
            out.push(s.group.clone());
        }
    }
    if sessions.is_empty() {
        sessions.push(SessionConfig::new(term_core::session::Protocol::Shell));
    }
    (out, sessions)
}

/// 命令行历史文件（在 profile 目录下），与 `prefs.json` / `sessions.db` 同目录。
pub const COMMAND_HISTORY_FILE: &str = "command_history.json";

/// 命令行历史文件的完整路径。
pub fn command_history_path() -> Option<PathBuf> {
    Some(profile_dir()?.join(COMMAND_HISTORY_FILE))
}

/// 写命令行历史（尽力而为，失败不崩；与 `prefs::save` 同一条规矩）。
/// 宿主在退出时调用；序列化在 `term-core`，文件 IO 只在这一层。
pub fn save_command_history(history: &term_core::command_history::CommandHistory) -> bool {
    match command_history_path() {
        Some(path) => save_command_history_to(&path, history),
        None => false,
    }
}

fn save_command_history_to(
    path: &Path,
    history: &term_core::command_history::CommandHistory,
) -> bool {
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    write_atomic(path, history.to_json().as_bytes())
}

/// 读命令行历史。文件不存在 / 坏 / 解析失败都返回 `None`（调用方用出厂空历史），
/// 与 `load` 的「读不出来也不该拦住启动」同一条规矩。
pub fn load_command_history() -> Option<term_core::command_history::CommandHistory> {
    command_history_path().and_then(|p| load_command_history_from(&p))
}

fn load_command_history_from(path: &Path) -> Option<term_core::command_history::CommandHistory> {
    if !path.is_file() {
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    term_core::command_history::CommandHistory::from_json(&raw)
}

/// 历史迁进保险库后删掉明文文件（迁移是一次性的；文件不存在也算成功）。
pub fn delete_command_history() -> bool {
    command_history_path().map(|p| delete_history_file_at(&p)).unwrap_or(false)
}

fn delete_history_file_at(path: &Path) -> bool {
    if !path.is_file() {
        return true;
    }
    std::fs::remove_file(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use term_core::session::{AuthMethod, Protocol};

    /// 每个测试一个独立目录：不碰真实配置，也不会互相看到对方的库。
    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn tmp_file() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "windterm-persist-{}-{}-{}",
            std::process::id(),
            nanos,
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(SESSIONS_FILE)
    }

    fn sample() -> (Vec<String>, Vec<SessionConfig>) {
        let groups = vec!["Servers".to_string(), "Home".to_string()];
        let mut s = SessionConfig::new(Protocol::Ssh);
        s.name = "prod".to_string();
        s.host = "10.0.0.1".to_string();
        s.port = 2222;
        s.username = "admin".to_string();
        s.auth = AuthMethod::PublicKey;
        s.group = "Servers".to_string();
        s.forwards = vec!["L 8080:db.internal:5432".to_string()];
        (groups, vec![s])
    }

    #[test]
    fn round_trip_save_and_load() {
        let path = tmp_file();
        let (groups, sessions) = sample();
        assert!(load_from(&path).is_none(), "还没保存过就该是 None");
        save_to(&path, &groups, &sessions);
        assert!(path.is_file(), "sessions.db 没被建出来");
        let (g2, s2) = load_from(&path).expect("load 失败");
        assert_eq!(g2, groups);
        assert_eq!(s2.len(), 1);
        assert_eq!(s2[0].name, "prod");
        assert_eq!(s2[0].host, "10.0.0.1");
        assert_eq!(s2[0].port, 2222);
        assert_eq!(s2[0].auth, AuthMethod::PublicKey);
        // 转发规则跟着会话一起活下来。
        assert_eq!(s2[0].forwards, vec!["L 8080:db.internal:5432".to_string()]);
    }

    #[test]
    fn a_missing_store_is_none_not_an_empty_list() {
        let path = tmp_file();
        assert!(load_from(&path).is_none());
        assert!(!path.exists(), "读一次不该把库建出来");
    }

    #[test]
    fn a_corrupt_store_falls_back_instead_of_panicking() {
        let path = tmp_file();
        std::fs::write(&path, "this is not a sqlite file").unwrap();
        assert!(load_from(&path).is_none());
    }

    #[test]
    fn an_empty_store_gets_one_local_shell_so_a_fresh_window_opens_running() {
        let (groups, sessions) = normalize(Vec::new(), Vec::new());
        assert!(groups.is_empty());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].protocol, Protocol::Shell);
    }

    #[test]
    fn a_group_only_referenced_by_a_session_is_filled_in() {
        let mut s = SessionConfig::new(Protocol::Ssh);
        s.group = "Gone".to_string();
        let (groups, _) = normalize(vec!["Default".to_string()], vec![s]);
        assert_eq!(groups, vec!["Default".to_string(), "Gone".to_string()]);
    }

    #[test]
    fn the_credential_reference_is_kept_across_a_list_save() {
        let path = tmp_file();
        let (groups, sessions) = sample();
        save_to(&path, &groups, &sessions);
        {
            let mut store = term_store::Store::open(&path).unwrap();
            assert!(store.set_vault_entry("prod", Some("prod")).unwrap());
        }
        // 再存一次列表（用户改了个分组名）：引用必须还在。
        save_to(&path, &groups, &sessions);
        let store = term_store::Store::open(&path).unwrap();
        assert_eq!(store.vault_entry("prod").unwrap().as_deref(), Some("prod"));
    }

    #[test]
    fn the_vault_file_is_written_atomically_and_read_back() {
        // 走**生产**写路径（`save_vault` 只认全局 `vault_path()`，重定向不了，
        // 所以调它的路径无关形态）。断言不再绑死某个固定的 `.tmp` 名，而是扫
        // 目录里有没有残留的临时文件——原来那条测试手工模拟了一遍
        // `write` + `rename`，测的是测试自己，不是产品代码。
        let dir = tmp_file();
        let vault = dir.parent().unwrap().join(VAULT_FILE);
        assert!(save_vault_to(&vault, b"sealed"));
        assert!(vault.is_file());
        assert!(tmp_leftovers(vault.parent().unwrap()).is_empty(), "临时文件不该留下");
        assert_eq!(std::fs::read(&vault).unwrap(), b"sealed");

        // 写第二次也不许留下临时文件，且内容是最后一次（rename 没被抢走）。
        assert!(save_vault_to(&vault, b"sealed-again"));
        assert!(tmp_leftovers(vault.parent().unwrap()).is_empty());
        assert_eq!(std::fs::read(&vault).unwrap(), b"sealed-again");
    }

    #[cfg(unix)]
    #[test]
    fn the_vault_file_and_its_directory_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let path = tmp_file();
        let parent = path.parent().unwrap();
        // 故意先把目录放开，确认保存路径会把它收紧（不只是新建时才 0700）。
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        let vault = parent.join(VAULT_FILE);
        assert!(save_vault_to(&vault, b"sealed"));
        let mode = std::fs::metadata(&vault).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "vault 文件只能属主读写");
        let dir_mode = std::fs::metadata(parent).unwrap().permissions().mode();
        assert_eq!(dir_mode & 0o777, 0o700, "profile 目录只能属主进入");
        assert!(tmp_leftovers(parent).is_empty(), "临时文件不该留下");
    }

    #[cfg(unix)]
    #[test]
    fn a_preexisting_loose_tmp_file_is_tightened_to_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let path = tmp_file();
        let tmp = unique_tmp(&path);
        std::fs::write(&tmp, b"old").unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(write_tmp_private(&tmp, b"new"), "显式 chmod 不能失败");
        let mode = std::fs::metadata(&tmp).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "预置的宽松临时文件也必须收紧到 0600");
        assert_eq!(std::fs::read(&tmp).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_tmp_file_is_refused_instead_of_followed() {
        let path = tmp_file();
        let tmp = unique_tmp(&path);
        let victim = path.with_file_name("victim.txt");
        std::fs::write(&victim, b"keep").unwrap();
        std::os::unix::fs::symlink(&victim, &tmp).unwrap();
        assert!(!write_tmp_private(&tmp, b"overwrite"));
        assert_eq!(
            std::fs::read(&victim).unwrap(),
            b"keep",
            "不得穿透符号链接写别处"
        );
    }

    /// 命名规则本身的钉子：同一进程里连着两次取临时名，必须**不同**且**同目录**。
    /// 下面那条并发用例是概率性的（固定名偶尔也能碰巧跑绿），这条是确定性的——
    /// 把 `unique_tmp` 退回固定名，或删掉 `TMP_SEQ`，它立刻红。
    #[test]
    fn unique_tmp_differs_for_two_writers_in_one_process() {
        let path = tmp_file();
        let a = unique_tmp(&path);
        let b = unique_tmp(&path);
        assert_ne!(a, b, "两个写者不能抢同一个临时文件");
        assert_eq!(a.parent(), path.parent(), "必须与目标同目录，rename 才原子");
        assert!(a.file_name().unwrap().to_string_lossy().contains(".tmp-"));
        assert!(b.file_name().unwrap().to_string_lossy().contains(".tmp-"));
    }

    /// 共享 profile 目录下的**并发写**：两个线程对同一路径写不同内容，
    /// 最终文件必须是其中一份完整可解析的内容，目录里不许有 tmp 残留。
    ///
    /// 这是「固定 .tmp 名 → 唯一名」这条口径唯一真正被测试覆盖的地方：
    /// 固定名时 A 的 rename 会改名掉 B 刚写的临时文件，于是最终文件可能是
    /// 半截的、或干脆是另一份内容；唯一名让两次 rename 互不相干。
    #[test]
    fn deleting_the_plaintext_history_is_idempotent() {
        let dir = tmp_file();
        let path = dir.parent().unwrap().join(COMMAND_HISTORY_FILE);
        // 文件不存在也算成功（迁移可能重复触发）。
        assert!(delete_history_file_at(&path));
        std::fs::write(&path, b"[]").unwrap();
        assert!(path.is_file());
        assert!(delete_history_file_at(&path));
        assert!(!path.is_file());
    }

    #[test]
    fn two_writers_to_one_history_file_leave_one_complete_file() {
        let dir = tmp_file();
        let path = dir.parent().unwrap().join(COMMAND_HISTORY_FILE);
        let mut alpha = term_core::command_history::CommandHistory::new(100, 0);
        alpha.push("alpha", 1);
        let mut beta = term_core::command_history::CommandHistory::new(100, 0);
        beta.push("beta", 2);
        let json_alpha = alpha.to_json();
        let json_beta = beta.to_json();
        let (path_a, path_b) = (path.clone(), path.clone());
        let a = std::thread::spawn(move || write_atomic(&path_a, json_alpha.as_bytes()));
        let b = std::thread::spawn(move || write_atomic(&path_b, json_beta.as_bytes()));
        a.join().unwrap();
        b.join().unwrap();

        let back = load_command_history_from(&path).expect("并发写之后文件必须完整可解析");
        let recent = back.recent(10);
        assert!(
            recent == vec!["alpha"] || recent == vec!["beta"],
            "最终文件必须是两份之一，不能是拼坏的混合体：{recent:?}"
        );
        assert!(
            tmp_leftovers(path.parent().unwrap()).is_empty(),
            "并发写不许留下临时文件"
        );
    }

    #[test]
    fn command_history_round_trips_through_a_file() {
        let path = tmp_file().with_file_name(COMMAND_HISTORY_FILE);
        let mut h = term_core::command_history::CommandHistory::new(100, 0);
        h.push("ls", 1);
        h.push("pwd", 2);
        assert!(save_command_history_to(&path, &h));
        let back = load_command_history_from(&path).expect("应可读回");
        assert_eq!(back.recent(10), vec!["pwd", "ls"]);
    }

    #[test]
    fn a_broken_command_history_file_is_none() {
        let path = tmp_file().with_file_name(COMMAND_HISTORY_FILE);
        std::fs::write(&path, "not json").unwrap();
        assert!(load_command_history_from(&path).is_none());
    }

    #[test]
    fn a_best_effort_conflict_check_refuses_an_overwritten_baseline() {
        let path = tmp_file().with_file_name("prefs.json");
        let first = write_atomic_if_unchanged(&path, None, b"{\"language\":\"en\"}").unwrap();
        assert_eq!(Some(first), file_fingerprint(&path), "指纹必须对应刚写下的字节");

        // 基线吻合：正常推进。
        let second =
            write_atomic_if_unchanged(&path, Some(first), b"{\"language\":\"zh-CN\"}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"language\":\"zh-CN\"}");
        assert_ne!(second, first);

        // 基线过期（别人已经写进新内容）：拒写，且原文件内容一字不动。
        assert_eq!(
            write_atomic_if_unchanged(&path, Some(first), b"{\"font_size\":20.0}"),
            Err(AtomicWriteError::Conflict)
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"language\":\"zh-CN\"}");
        assert!(tmp_leftovers(path.parent().unwrap()).is_empty(), "拒写不许留下临时文件");

        // 文件被删掉也算基线过期（`Some` 期望的是那份内容，不是「不存在」）。
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            write_atomic_if_unchanged(&path, Some(second), b"{}"),
            Err(AtomicWriteError::Conflict)
        );
        assert!(!path.exists());
    }
}
