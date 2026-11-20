//! bridge token：`desk serve` 对外暴露能力面时唯一的凭证。
//!
//! 为什么是文件而不是环境变量 / 命令行参数：token 要能被「人」交给客户端
//! （`cat ~/.zterm/bridge.token`），也要能在**重启后保持不变**（否则每次开
//! 服务所有客户端都要改配置）。环境变量随进程走、命令行参数会进 `ps`，
//! 两者都不合适。
//!
//! 三条硬规则：
//!
//! 1. **文件必须只有主人可读（0600）**。判断权在调用方，这里的 `load_or_create`
//!    只负责判：权限宽了就报错，**不擅自 chmod**——那会掩盖用户自己设的 ACL 意图，
//!    而且下次还会被改回去。**这条只是 unix 判据**：Windows 没有 unix 权限位，
//!    访问控制由 `%USERPROFILE%\.zterm` 的 NTFS ACL 继承决定，本进程既读不到
//!    也不该改（见 `file_mode` 的注释）。
//! 2. **不覆盖已有 token**。`create_new` 而不是 `create`：并发起两个 `serve`
//!    时，后一个不该悄悄换掉前一个已经交给客户端的凭证。
//! 3. **比对不早退**。token 是 64 个 hex 字符，逐字节异或累加，长度不同直接 false
//!    （长度不是秘密，定长 hex 也不泄露内容）。

use std::fs::{self, OpenOptions};
#[cfg(unix)]
use std::io::Read;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// token 的熵：32 字节 = 256 bit，hex 之后 64 个字符。
const TOKEN_BYTES: usize = 32;

/// 默认位置：`~/.zterm/bridge.token`。
pub fn default_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".zterm").join("bridge.token"))
}

/// token 文件的三种失败：读不了、权限太宽、内容不像 token。
///
/// 分开而不是揉成一个字符串：这三种的处理方式完全不同（查路径 / `chmod` /
/// 删了重生成），文案要直接告诉用户该做什么。
#[derive(Debug)]
pub enum TokenError {
    Io { path: PathBuf, detail: String },
    TooPermissive { path: PathBuf, mode: u32 },
    Malformed { path: PathBuf, why: &'static str },
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenError::Io { path, detail } => {
                write!(f, "读不了 token 文件 {}：{detail}", path.display())
            }
            TokenError::TooPermissive { path, mode } => write!(
                f,
                "token 文件 {} 的权限是 {:o}，同机其他用户也能读到——\
                 `desk serve` 拒绝启动。先跑 `chmod 600 {}`",
                path.display(),
                mode,
                path.display()
            ),
            TokenError::Malformed { path, why } => {
                write!(f, "token 文件 {} 不像 token（{why}）", path.display())
            }
        }
    }
}

impl std::error::Error for TokenError {}

/// 读取既有 token；不存在则以 0600 新建一个。
///
/// 「读」而不是「每次新生成」是有意的：服务重启不该让已配置好的客户端全部失效。
pub fn load_or_create(path: &Path) -> Result<String, TokenError> {
    if path.exists() {
        return read_existing(path);
    }
    let token = generate()?;
    write_new(path, &token)?;
    Ok(token)
}

fn read_existing(path: &Path) -> Result<String, TokenError> {
    let meta = fs::metadata(path).map_err(|err| TokenError::Io {
        path: path.to_path_buf(),
        detail: err.to_string(),
    })?;
    let mode = file_mode(&meta);
    if !perms_are_private(mode) {
        return Err(TokenError::TooPermissive {
            path: path.to_path_buf(),
            mode,
        });
    }
    let raw = fs::read_to_string(path).map_err(|err| TokenError::Io {
        path: path.to_path_buf(),
        detail: err.to_string(),
    })?;
    let token = raw.trim().to_string();
    if !looks_like_token(&token) {
        return Err(TokenError::Malformed {
            path: path.to_path_buf(),
            why: "应为 64 个 hex 字符（32 字节的 hex），删掉这个文件后会重新生成",
        });
    }
    Ok(token)
}

fn write_new(path: &Path, token: &str) -> Result<(), TokenError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let existed = dir.exists();
    fs::create_dir_all(dir).map_err(|err| TokenError::Io {
        path: dir.to_path_buf(),
        detail: err.to_string(),
    })?;
    let io_err = |p: &Path, err: std::io::Error| TokenError::Io {
        path: p.to_path_buf(),
        detail: err.to_string(),
    };
    // 只收紧**自己刚建出来的**目录；规则与理由见 `tighten_new_dir`。
    tighten_new_dir(dir, existed);

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    // unix 上要求「建出来就是 0600」，不给一个先 0644 再 chmod 的窗口。
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(|err| io_err(path, err))?;
    let written = file
        .write_all(format!("{token}\n").as_bytes())
        .and_then(|()| file.flush());
    if let Err(err) = written {
        // 半截文件比没有更糟：下次启动会读到空文件并报 Malformed，
        // 用户还得自己想「为什么要删」。这里直接收尾。
        let _ = fs::remove_file(path);
        return Err(io_err(path, err));
    }
    Ok(())
}

/// 文件的 unix 权限位（`0o777` 掩码）。
#[cfg(unix)]
fn file_mode(meta: &fs::Metadata) -> u32 {
    meta.permissions().mode() & 0o777
}

/// 非 unix 平台：没有 unix 权限位，**按「只有主人可读」返回**。
///
/// 为什么给一个定值而不是让上层分叉：判据（[`perms_are_private`]）与它的错误
/// 文案只有一处，分叉成两套会让 `desk serve` 在不同平台的诊断不一致。Windows 上
/// 真正的访问控制是 NTFS ACL——`%USERPROFILE%` 下新建的文件默认只对本人与
/// Administrators 可读，本进程既读不到 ACL 也不该去改。拿 `0o600` 表示「本平台
/// 无需额外判」是**诚实的占位**，不是"假装检查过了"。
#[cfg(not(unix))]
fn file_mode(_meta: &fs::Metadata) -> u32 {
    0o600
}

/// 只收紧**自己刚建出来的**目录（`~/.zterm` 的常态）。
///
/// 对已存在的目录一律不动：`--token-file ~/Documents/x.token` 这种用法下把
/// `~/Documents` 改成 0700 是不可接受的副作用，而 `let _ =` 会把它藏起来。
#[cfg(unix)]
fn tighten_new_dir(dir: &Path, existed: bool) {
    if !existed {
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
}

/// 非 unix 平台：什么都不做。
///
/// `std::fs::set_permissions` 在 Windows 上只能改**只读位**，对访问控制没有
/// 作用；新建目录的 ACL 由父目录继承，这正是想要的结果。
#[cfg(not(unix))]
fn tighten_new_dir(_dir: &Path, _existed: bool) {}

/// 权限是否「只有主人可读」：组与其他人的读写执行位全为 0。
pub fn perms_are_private(mode: u32) -> bool {
    mode & 0o077 == 0
}

/// 64 个 hex 字符。不校验大小写之外的花样——生成处只会写小写。
pub fn looks_like_token(token: &str) -> bool {
    token.len() == TOKEN_BYTES * 2 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 恒定时间比对。
pub fn tokens_match(expected: &str, got: &str) -> bool {
    if expected.len() != got.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in expected.bytes().zip(got.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// 32 字节来自操作系统的随机源。
///
/// unix 走 `/dev/urandom`：不引 `getrandom` / `rand`，为一次 32 字节的读取加一棵
/// 依赖树不划算，而 `/dev/urandom` 在 macOS 与 Linux 上都存在且是加密安全的。
///
/// 非 unix 没有 `/dev/urandom`，改走 `getrandom`——Windows 上它的后端是
/// `RtlGenRandom`（`advapi32`），正好是系统提供的那个加密随机源。
fn generate() -> Result<String, TokenError> {
    let mut buf = [0u8; TOKEN_BYTES];
    fill_random(&mut buf)?;
    let mut hex = String::with_capacity(TOKEN_BYTES * 2);
    for byte in buf {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(hex)
}

#[cfg(unix)]
fn fill_random(buf: &mut [u8]) -> Result<(), TokenError> {
    fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(buf))
        .map_err(|err| TokenError::Io {
            path: PathBuf::from("/dev/urandom"),
            detail: err.to_string(),
        })
}

#[cfg(not(unix))]
fn fill_random(buf: &mut [u8]) -> Result<(), TokenError> {
    getrandom::getrandom(buf).map_err(|err| TokenError::Io {
        // 没有路径可报（随机源是系统 API），这一行只是让诊断不丢信息。
        path: PathBuf::from("<os rng>"),
        detail: err.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    /// 每个用例一个独立临时目录（不并行撞名）。
    fn scratch() -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("zterm-token-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    #[test]
    fn only_owner_readable_counts_as_private() {
        assert!(perms_are_private(0o600));
        assert!(perms_are_private(0o400));
        assert!(!perms_are_private(0o640), "组可读不行");
        assert!(!perms_are_private(0o604), "其他人可读不行");
        assert!(!perms_are_private(0o660));
        assert!(!perms_are_private(0o644), "默认 umask 就是这个");
    }

    #[test]
    fn generated_token_is_32_bytes_of_hex() {
        let token = generate().expect("读 /dev/urandom");
        assert_eq!(token.len(), 64);
        assert!(looks_like_token(&token));
        // 两次生成不该撞（256 bit，撞了说明随机源有问题）。
        assert_ne!(token, generate().expect("读 /dev/urandom"));
        assert!(!looks_like_token(""), "空串不是 token");
        assert!(!looks_like_token(&token[..63]), "短一位不是 token");
        assert!(!looks_like_token(&"z".repeat(64)), "非 hex 不是 token");
    }

    #[test]
    fn created_file_is_0600_and_reused_on_second_call() {
        let dir = scratch();
        let path = dir.join("nested").join("bridge.token");
        let first = load_or_create(&path).expect("首次生成");
        // 权限位这条判据只有 unix 有（见 `file_mode` 的注释）：Windows 上要验的是
        // 「文件真的建出来了」，不是 unix 权限。
        #[cfg(unix)]
        {
            let mode = fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "新建的 token 必须只有主人可读");
        }
        #[cfg(not(unix))]
        assert!(path.exists(), "新建的 token 文件必须存在");
        let second = load_or_create(&path).expect("二次读取");
        assert_eq!(first, second, "重启不该换掉已交给客户端的凭证");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_world_readable_token_is_refused_not_fixed() {
        let dir = scratch();
        let path = dir.join("bridge.token");
        fs::write(&path, "a".repeat(64)).expect("写文件");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod");
        match load_or_create(&path) {
            Err(TokenError::TooPermissive { mode, .. }) => assert_eq!(mode, 0o644),
            other => panic!("0644 必须被拒，实际：{other:?}"),
        }
        // 而且不许偷偷改掉权限——用户得自己决定。
        let mode = fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "拒绝是对的，擅自 chmod 是错的");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_or_short_token_is_refused_with_a_readable_reason() {
        let dir = scratch();
        let path = dir.join("bridge.token");
        fs::write(&path, "").expect("写空文件");
        // 先把权限调到「合格」，确保失败原因只能是内容而不是权限（unix 才有这条）。
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod");
        match load_or_create(&path) {
            Err(TokenError::Malformed { why, .. }) => assert!(why.contains("64")),
            other => panic!("空文件必须被拒，实际：{other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_directory_is_not_tightened_behind_the_users_back() {
        let dir = scratch();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("chmod 0755");
        let path = dir.join("bridge.token");
        load_or_create(&path).expect("生成");
        let mode = fs::metadata(&dir).expect("stat dir").permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o755,
            "只许收紧自己新建的目录；改用户已有目录的权限是不可接受的副作用"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn comparison_is_exact() {
        let a = "ab".repeat(32);
        assert!(tokens_match(&a, &a));
        assert!(!tokens_match(&a, &"ab".repeat(31)), "长度不同直接 false");
        let mut other = a.clone();
        other.replace_range(63..64, "c");
        assert!(!tokens_match(&a, &other), "最后一位不同也必须 false");
    }
}
