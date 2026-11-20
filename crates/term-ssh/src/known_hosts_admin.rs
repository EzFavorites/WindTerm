//! `known_hosts` 的管理面：列举 / 遗忘 / 换钥。
//!
//! 与 [`crate::known_hosts`] 分工：那边是**判定**（连上去的时候这把钥匙能不能信），
//! 这边是**维护账本**（人想知道自己信过谁、想忘掉谁、想在有把握时换一把）。
//!
//! 三条原则：
//!
//! 1. **不认识的行走原样保留**。注释、空行、`@cert-authority`、带 `!` 的反向匹配、
//!    哈希过的 `|1|…` —— 一律不改写。这个文件是用户和 OpenSSH 共用的，
//!    我们只是偶尔动一两行，动完别人还得能接着用。
//! 2. **只认精确匹配**。`forget("example.com", 22)` 只删 `example.com` 这一条，
//!    不删 `*.example.com`、不删 `[example.com]:2222`。通配符展开是 OpenSSH
//!    的匹配语义，在这里替用户猜等于删错行。
//! 3. **只改被命中的行**。没命中就**一个字节都不写**——连 mtime 都不动，
//!    否则备份脚本会看到一堆「什么都没改」的变更。
//!
//! 写文件是「同目录临时文件 + rename」：中途崩掉时旧文件还在，不会留下半截账本。
//!
//! # 谁在用
//!
//! 1. **SSH 连接对话框的「主机密钥变了」**（`apps/desktop/src/ui/dialogs/ssh_connect.rs`
//!    + `ssh.rs`）：连接被拒之后给一个独立窗口，用户确认过指纹才调
//!    [`trust_openssh`] 换掉旧记录，然后重连。刻意**没有**「要不要继续」这种一键
//!    确认——那会把中间人攻击变成默认按钮（见 `next()` 上的注释）。
//! 2. 会话库里的「忘掉这台机器」右键项：还没做，做的时候直接 [`forget`]。
//!
//! 本模块仍然**只有 API、没有独立 CLI**：`desk known-hosts` 那类子命令是给将来
//! 补第 3 个入口，不是给现在补第 1 个调用方。

use std::path::{Path, PathBuf};

use russh::keys::PublicKey;

use crate::error::SshError;
use crate::known_hosts::fingerprint;
use crate::options::DEFAULT_PORT;

/// `known_hosts` 里的一条记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostRecord {
    /// 行号（1 起，与编辑器里看到的一致）。
    pub line: usize,
    /// 这一行覆盖的主机名（`[host]:port` 形式保留方括号）。
    ///
    /// 哈希过的行（`|1|…`）**为空**：那种形式故意让人读不出主机名，
    /// 我们也不假装能读——`hashed` 会告诉你为什么。
    pub hosts: Vec<String>,
    /// 这一行的主机名是哈希形式。
    pub hashed: bool,
    /// `@cert-authority` / `@revoked` 这类标记。
    pub marker: Option<String>,
    /// 钥匙类型（`ssh-ed25519` …）。
    pub key_type: String,
    /// 钥匙指纹（`SHA256:…`）；base64 段解不开时为 `None`。
    pub fingerprint: Option<String>,
}

/// `trust` 的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustOutcome {
    /// 被替换掉的行数（0 表示这台主机之前没记录）。
    pub removed: usize,
    /// 新记下的那把钥匙的指纹。
    pub fingerprint: String,
}

/// 一条已知主机在文件里的键：默认端口写主机名，其余写 `[host]:port`。
///
/// 与 OpenSSH、与 [`crate::known_hosts::verify`] 走的是同一条规则——三处不一致
/// 的表现是「记下来了却校验不过」，那种 bug 极难查。
pub fn host_token(host: &str, port: u16) -> String {
    if port == DEFAULT_PORT {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

/// 读出一份账本。文件不存在 = 还没有账本，返回空表（不是错误）。
pub fn list(path: &Path) -> Result<Vec<HostRecord>, SshError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(SshError::HostKey(crate::error::HostKeyProblem::Unreadable(
            format!("读不了 {}：{err}", path.display()),
        ))),
    };
    Ok(text
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let parsed = parse_line(line)?;
            Some(HostRecord {
                line: index + 1,
                hosts: parsed.hosts,
                hashed: parsed.hashed,
                marker: parsed.marker,
                key_type: parsed.key_type,
                fingerprint: parsed.fingerprint,
            })
        })
        .collect())
}

/// 忘掉一台主机：删掉匹配 `host_token(host, port)` 的行。
///
/// 返回删掉的行数。没命中时不写文件（连 mtime 都不动）。
pub fn forget(path: &Path, host: &str, port: u16) -> Result<usize, SshError> {
    let token = host_token(host, port);
    let Some(text) = read_optional(path)? else {
        return Ok(0);
    };
    let (kept, removed) = filter_out(&text, &token);
    if removed > 0 {
        write_atomic(path, &kept)?;
    }
    Ok(removed)
}

/// 明确地换一把钥匙：删掉这台主机的旧记录，再写上新的。
///
/// 这是**人做决定**的路径（「我知道这台机器重装了，指纹是它给我的这个」），
/// 所以不检查新旧钥匙是否一致，也不打印警告——判定的那条路
/// （[`crate::known_hosts::verify`]）不会自己走到这里来。
pub fn trust(
    path: &Path,
    host: &str,
    port: u16,
    key: &PublicKey,
) -> Result<TrustOutcome, SshError> {
    let token = host_token(host, port);
    let text = read_optional(path)?.unwrap_or_default();
    let (mut kept, removed) = filter_out(&text, &token);

    let bare = PublicKey::new(key.key_data().clone(), String::new());
    let line = bare.to_openssh().map_err(|e| {
        SshError::HostKey(crate::error::HostKeyProblem::Unreadable(format!(
            "拼不出要写入的钥匙行：{e}"
        )))
    })?;
    if !kept.is_empty() && !kept.ends_with('\n') {
        kept.push('\n');
    }
    kept.push_str(&format!("{token} {line}\n"));
    write_atomic(path, &kept)?;

    Ok(TrustOutcome {
        removed,
        fingerprint: fingerprint(key),
    })
}

/// 同 [`trust`]，但钥匙以 openssh 单行文本给出。
///
/// 存在的理由：界面拿到新钥匙的来源是**错误值**（`HostKeyProblem::Changed` 里
/// 带着服务器这次给的公钥），那里已经是一个字符串；为了让界面能调 `trust` 就得
/// 让 `PublicKey` 穿过 UI，等于把 russh 的类型漏进投影层。改成文本进、文本出，
/// UI 只搬字符串。
///
/// 先解析再写：**解析失败时一个字节都不落盘**（不是先清后写）。
pub fn trust_openssh(
    path: &Path,
    host: &str,
    port: u16,
    key_line: &str,
) -> Result<TrustOutcome, SshError> {
    let key = PublicKey::from_openssh(key_line.trim()).map_err(|e| {
        SshError::HostKey(crate::error::HostKeyProblem::Unreadable(format!(
            "这不是一把能解析的公钥：{e}"
        )))
    })?;
    trust(path, host, port, &key)
}

/// 解析一行；注释、空行、看不懂的行都返回 `None`（调用方原样保留）。
struct Parsed {
    marker: Option<String>,
    hosts: Vec<String>,
    hashed: bool,
    key_type: String,
    fingerprint: Option<String>,
}

fn parse_line(line: &str) -> Option<Parsed> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let mut fields = trimmed.split_whitespace();
    let mut first = fields.next()?;
    let mut marker = None;
    if first.starts_with('@') {
        marker = Some(first.to_string());
        first = fields.next()?;
    }
    let key_type = fields.next()?.to_string();
    let blob = fields.next()?.to_string();
    // 注释（第 4 段起）忽略：它只是自由文本，指纹不含它。
    let hashed = first.starts_with('|');
    let hosts: Vec<String> = if hashed {
        Vec::new()
    } else {
        first.split(',').map(str::to_string).collect()
    };
    let fingerprint = PublicKey::from_openssh(&format!("{key_type} {blob}"))
        .ok()
        .map(|key| fingerprint(&key));
    Some(Parsed {
        marker,
        hosts,
        hashed,
        key_type,
        fingerprint,
    })
}

/// 把命中 `token` 的行丢掉，其余（含注释与空行）原样保留。
fn filter_out(text: &str, token: &str) -> (String, usize) {
    let mut kept = String::with_capacity(text.len());
    let mut removed = 0;
    for line in text.lines() {
        let hit = parse_line(line).is_some_and(|parsed| parsed.hosts.iter().any(|h| h == token));
        if hit {
            removed += 1;
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    (kept, removed)
}

fn read_optional(path: &Path) -> Result<Option<String>, SshError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(SshError::HostKey(crate::error::HostKeyProblem::Unreadable(
            format!("读不了 {}：{err}", path.display()),
        ))),
    }
}

/// 同目录写临时文件再 `rename`：崩溃时旧文件完好，不会留下半截账本。
fn write_atomic(path: &Path, text: &str) -> Result<(), SshError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| {
        SshError::HostKey(crate::error::HostKeyProblem::Unreadable(format!(
            "建不了目录 {}：{e}",
            dir.display()
        )))
    })?;
    let temp: PathBuf = dir.join(format!(
        ".known_hosts.tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&temp, text).map_err(|e| {
        SshError::HostKey(crate::error::HostKeyProblem::Unreadable(format!(
            "写不了临时文件 {}：{e}",
            temp.display()
        )))
    })?;
    if let Err(e) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(SshError::HostKey(crate::error::HostKeyProblem::Unreadable(
            format!("替换 {} 失败：{e}", path.display()),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    const HOST_PUB: &str = include_str!("../testdata/host_ed25519.pub");
    const CLIENT_PUB: &str = include_str!("../testdata/id_ed25519.pub");

    fn key(pub_openssh: &str) -> PublicKey {
        PublicKey::from_openssh(pub_openssh.trim()).expect("测试公钥")
    }

    /// 每个用例一个独立目录。
    ///
    /// 名字里**必须带纳秒**，不能只靠 `pid + 计数器`：这里好几个用例的前提是
    /// 「文件还不存在」（例如 `trusting_on_a_missing_file_creates_the_ledger`
    /// 断言 `removed == 0`），而 `temp_dir()` 是跨进程共用的、谁也不清理。
    /// 进程号会被系统回收复用，计数器又只在本进程内递增，两者一撞上，用例就会
    /// 读到上一轮留下的账本，"缺文件"变成了"有文件"，于是报 `removed: 1`。
    /// 这不是 flaky 可以被重跑掩盖的那种——它报的是**别的进程写下的**状态。
    /// 同文件的 [`write_atomic`] 与 `known_hosts::tests::scratch` 都带了纳秒，
    /// 这里当初漏了。
    fn scratch(tag: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "term-ssh-khadmin-{}-{}-{tag}-{n}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).expect("建临时目录");
        dir.join("known_hosts")
    }

    #[test]
    fn the_host_key_uses_brackets_only_for_non_default_ports() {
        assert_eq!(host_token("example.com", 22), "example.com");
        assert_eq!(host_token("example.com", 2222), "[example.com]:2222");
    }

    #[test]
    fn a_missing_file_is_an_empty_ledger_not_an_error() {
        let path = scratch("missing");
        assert!(list(&path).expect("不存在不是错误").is_empty());
        assert_eq!(forget(&path, "example.com", 22).expect("忘掉不存在的"), 0);
        assert!(!path.exists(), "没命中就不该建文件");
    }

    #[test]
    fn listing_skips_comments_and_reports_markers_and_fingerprints() {
        let path = scratch("list");
        let host_line = key(HOST_PUB).to_openssh().expect("拼一行");
        fs::write(
            &path,
            format!(
                "# 这是我自己的账本\n\n\
                 example.com,www.example.com {host_line}\n\
                 @revoked old.example.com {host_line}\n\
                 |1|abc=|def= {host_line}\n"
            ),
        )
        .expect("写账本");

        let records = list(&path).expect("列账本");
        assert_eq!(records.len(), 3, "注释与空行不算记录：{records:#?}");
        assert_eq!(records[0].line, 3, "行号要能对上编辑器");
        assert_eq!(records[0].hosts, ["example.com", "www.example.com"]);
        assert!(!records[0].hashed);
        assert_eq!(records[0].marker, None);
        assert!(records[0].fingerprint.as_deref().is_some_and(|f| f.starts_with("SHA256:")));

        assert_eq!(records[1].marker.as_deref(), Some("@revoked"));

        assert!(records[2].hashed, "|1|… 是哈希行");
        assert!(records[2].hosts.is_empty(), "哈希行读不出主机名，不假装能读");
        assert!(records[2].fingerprint.is_some(), "指纹只跟钥匙有关，仍可算");
    }

    #[test]
    fn forgetting_removes_only_the_exact_host_and_keeps_everything_else() {
        let path = scratch("forget");
        let host_line = key(HOST_PUB).to_openssh().expect("拼一行");
        let original = format!(
            "# 保留这行注释\n\
             example.com {host_line}\n\
             [example.com]:2222 {host_line}\n\
             *.example.com {host_line}\n\
             other.example.com {host_line}\n"
        );
        fs::write(&path, &original).expect("写账本");

        assert_eq!(forget(&path, "example.com", 22).expect("忘掉"), 1);
        let after = fs::read_to_string(&path).expect("读回");
        // 逐行比对，**不能**用 `after.contains("example.com <key>")`：
        // `other.example.com <key>` 是它的超串，断言会永远为真。
        let target = format!("example.com {host_line}");
        assert!(
            !after.lines().any(|line| line.trim() == target),
            "目标行该没了：{after}"
        );
        assert!(after.contains("# 保留这行注释"), "注释不该被动");
        assert!(after.contains("[example.com]:2222"), "另一个端口是另一条记录");
        assert!(after.contains("*.example.com"), "通配行不是同一台主机");
        assert!(after.contains("other.example.com"));
    }

    #[test]
    fn a_non_default_port_only_forgets_that_port() {
        let path = scratch("port");
        let host_line = key(HOST_PUB).to_openssh().expect("拼一行");
        fs::write(
            &path,
            format!("example.com {host_line}\n[example.com]:2222 {host_line}\n"),
        )
        .expect("写账本");

        assert_eq!(forget(&path, "example.com", 2222).expect("忘掉 2222"), 1);
        let after = fs::read_to_string(&path).expect("读回");
        assert!(after.contains(&format!("example.com {host_line}")));
        assert!(!after.contains(":2222"));
    }

    #[test]
    fn forgetting_a_host_that_is_not_there_does_not_touch_the_file() {
        let path = scratch("noop");
        let host_line = key(HOST_PUB).to_openssh().expect("拼一行");
        let original = format!("other.example.com {host_line}\n");
        fs::write(&path, &original).expect("写账本");
        let before = fs::metadata(&path).expect("stat").modified().expect("mtime");

        assert_eq!(forget(&path, "example.com", 22).expect("没命中"), 0);
        assert_eq!(fs::read_to_string(&path).expect("读回"), original);
        let after = fs::metadata(&path).expect("stat").modified().expect("mtime");
        assert_eq!(before, after, "没命中就不该重写文件");
    }

    #[test]
    fn trusting_a_new_key_replaces_the_old_line_and_reports_the_fingerprint() {
        let path = scratch("trust");
        let old_line = key(CLIENT_PUB).to_openssh().expect("拼一行");
        fs::write(
            &path,
            format!(
                "# 别人的记录\nother.example.com {}\nexample.com {old_line}\n",
                key(HOST_PUB).to_openssh().unwrap()
            ),
        )
        .expect("写账本");

        let outcome = trust(&path, "example.com", 22, &key(HOST_PUB)).expect("换钥");
        assert_eq!(outcome.removed, 1, "旧记录该被替换掉");
        assert_eq!(outcome.fingerprint, fingerprint(&key(HOST_PUB)));

        let after = fs::read_to_string(&path).expect("读回");
        assert!(after.contains("# 别人的记录"), "别的行不该被动");
        assert!(after.contains("other.example.com"));
        assert_eq!(
            after.matches("example.com ").count(),
            2,
            "other.example.com 与新的 example.com 各一次：{after}"
        );

        // 换完之后校验应当认这把新钥匙。
        let decision = crate::known_hosts::verify(
            crate::options::HostKeyPolicy::Strict,
            "example.com",
            22,
            Some(&path),
            &key(HOST_PUB),
        )
        .expect("换钥后应当被信任");
        assert_eq!(decision, crate::known_hosts::Decision::Trusted);
    }

    #[test]
    fn trusting_on_a_missing_file_creates_the_ledger() {
        let path = scratch("trust-new");
        // 前提写成断言：本模块只有这一个用例依赖「账本还不存在」（其余都自己
        // 先 `fs::write` 铺好内容），所以路径一旦被复用，只有它会炸，而且炸出来
        // 的是 `removed: 1` 这种看不出原因的读数。这里直接点出真正的问题。
        assert!(
            !path.exists(),
            "前提不成立：账本已经存在，说明 scratch 交出了别人用过的路径 {}",
            path.display()
        );
        let outcome = trust(&path, "example.com", 2222, &key(HOST_PUB)).expect("建账本");
        assert_eq!(outcome.removed, 0);
        let text = fs::read_to_string(&path).expect("读回");
        assert!(text.starts_with("[example.com]:2222 "), "{text}");
        assert!(text.ends_with('\n'), "结尾要有换行，否则下一条记录会粘上来");
    }

    #[test]
    fn replacing_by_text_lands_the_same_line_as_replacing_by_key() {
        // 界面拿到的新钥匙是**字符串**（`HostKeyProblem::Changed.presented`），
        // 所以这里验的是「文本进、账本出」这条路与 `trust` 完全等价。
        let by_text = scratch("trust-text");
        let by_key = scratch("trust-key");
        let old = key(CLIENT_PUB).to_openssh().expect("拼旧行");
        for p in [&by_text, &by_key] {
            fs::write(p, format!("example.com {old}\n")).expect("写账本");
        }

        let presented = key(HOST_PUB).to_openssh().expect("拼新行");
        let a = trust_openssh(&by_text, "example.com", 22, &presented).expect("换钥（文本）");
        let b = trust(&by_key, "example.com", 22, &key(HOST_PUB)).expect("换钥（钥匙）");
        assert_eq!(a, b);
        assert_eq!(
            fs::read_to_string(&by_text).expect("读回"),
            fs::read_to_string(&by_key).expect("读回"),
            "两条路写出来的字节必须一样，否则「界面换的」和「API 换的」会分叉"
        );

        let decision = crate::known_hosts::verify(
            crate::options::HostKeyPolicy::Strict,
            "example.com",
            22,
            Some(&by_text),
            &key(HOST_PUB),
        )
        .expect("换完应当被信任");
        assert_eq!(decision, crate::known_hosts::Decision::Trusted);
    }

    #[test]
    fn a_garbage_key_line_is_rejected_before_anything_is_written() {
        let path = scratch("trust-garbage");
        let original = format!("example.com {}\n", key(CLIENT_PUB).to_openssh().unwrap());
        fs::write(&path, &original).expect("写账本");

        let err = trust_openssh(&path, "example.com", 22, "这不是一把钥匙").expect_err("应当报错");
        assert!(err.to_string().contains("公钥"), "{err}");
        assert_eq!(
            fs::read_to_string(&path).expect("读回"),
            original,
            "解析失败时一个字节都不该落盘——先清后写会留下空账本"
        );
    }
}
