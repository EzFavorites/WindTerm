//! 脚本的**本地文件端口**：把 `std::fs` 接到 `LocalFs`，并逐次过授权信封。
//!
//! # 判定点在哪儿
//!
//! **每一次**读写都重新问一次 `Capabilities`（[`term_core::script_caps`]），
//! 不在构造时判一次了事：脚本可以在中途拿到新授权（用户在文件框里手选了一个
//! 路径 → 工作区把它追加进信封），构造期判定会让那条路白拿。
//!
//! # 规范化
//!
//! 走 `canonicalize` 再比前缀。软链因此不会变成后门：`~/allowed/link-to-etc`
//! 规范到 `/etc` 之后落在白名单外，照样拒。**不存在的路径**没法规范化
//! （`canonicalize` 会失败），所以写新文件时退一步：规范父目录，再拼文件名。
//!
//! # 为什么读整文件返回 `Vec<u8>`
//!
//! 脚本会把内容当文本用（`text(data)`），但把二进制文件读成 `String` 再写回
//! 去会损坏字节。端口上保持字节，转不转文本由脚本决定。

use std::path::PathBuf;

use term_script::{normalize_for, DirEntry, FileStat, FsError, LocalFs};

/// 授权来源。工作区每次调用都传一份**当前**信封进来（见模块文档）。
///
/// 用 `Fn` 而不是 `Capabilities` 值：信封在运行中会被追加（`pick_file`），
/// 值拷贝会拿到一份过时的快照。
pub struct LocalFiles<F> {
    caps: F,
}

impl<F> LocalFiles<F>
where
    F: Fn() -> term_core::script_caps::Capabilities + Send + Sync,
{
    pub fn new(caps: F) -> Self {
        Self { caps }
    }

    /// 读：规范化 + 判读权限。
    fn allow_read(&self, path: &str) -> Result<PathBuf, FsError> {
        let resolved = normalize_for(path, false)?;
        (self.caps)()
            .check_read(&resolved)
            .map_err(|denied| FsError::Denied(denied.to_string()))?;
        Ok(resolved)
    }

    /// 写：规范化 + 判写权限。
    ///
    /// 目标可能还不存在，所以 `normalize_for(write = true)` 走「规范父目录」
    /// 那条退路。
    fn allow_write(&self, path: &str) -> Result<PathBuf, FsError> {
        let resolved = normalize_for(path, true)?;
        (self.caps)()
            .check_write(&resolved)
            .map_err(|denied| FsError::Denied(denied.to_string()))?;
        Ok(resolved)
    }

    /// 远端传输的本地端点：只判定与规范化，不碰文件（契约见 `term_script::localfs`）。
    fn authorize(&self, path: &str, write: bool) -> Result<PathBuf, FsError> {
        if write {
            self.allow_write(path)
        } else {
            self.allow_read(path)
        }
    }
}

impl<F> LocalFs for LocalFiles<F>
where
    F: Fn() -> term_core::script_caps::Capabilities + Send + Sync,
{
    fn read(&self, path: &str) -> Result<Vec<u8>, FsError> {
        let resolved = self.allow_read(path)?;
        std::fs::read(&resolved)
            .map_err(|e| FsError::Io(format!("读不到 {}：{e}", resolved.display())))
    }

    fn write(&self, path: &str, data: &[u8], append: bool) -> Result<u64, FsError> {
        use std::io::Write;
        let resolved = self.allow_write(path)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true);
        if append {
            options.append(true).create(true);
        } else {
            // `truncate` 必须与 `create` 一起给：只给 `truncate(true)` 时文件
            // 不存在会报 NotFound，脚本得自己先判断「有没有」——多一次竞态。
            options.truncate(true).create(true);
        }
        let mut file = options
            .open(&resolved)
            .map_err(|e| FsError::Io(format!("写不到 {}：{e}", resolved.display())))?;
        file.write_all(data)
            .and_then(|()| file.flush())
            .map_err(|e| FsError::Io(format!("写到 {} 时失败：{e}", resolved.display())))?;
        Ok(data.len() as u64)
    }

    fn list(&self, path: &str) -> Result<Vec<DirEntry>, FsError> {
        let resolved = self.allow_read(path)?;
        let reader = std::fs::read_dir(&resolved)
            .map_err(|e| FsError::Io(format!("列不出 {}：{e}", resolved.display())))?;
        let mut rows = Vec::new();
        for entry in reader {
            let entry = entry.map_err(|e| {
                FsError::Io(format!("读 {} 的目录项时失败：{e}", resolved.display()))
            })?;
            // `metadata` 跟着软链走：列表里显示的大小/类型以目标为准，
            // 与文件管理器一致。
            let meta = std::fs::metadata(entry.path()).ok();
            rows.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                dir: meta.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            });
        }
        // 稳定顺序：目录在前、同组按名字。脚本据此可以做「取第一个 .log」
        // 这类事而不必自己排。
        rows.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));
        Ok(rows)
    }

    fn stat(&self, path: &str) -> Result<FileStat, FsError> {
        let resolved = self.allow_read(path)?;
        let meta = std::fs::metadata(&resolved)
            .map_err(|e| FsError::Io(format!("查不到 {} 的属性：{e}", resolved.display())))?;
        Ok(FileStat {
            path: resolved.display().to_string(),
            size: meta.len(),
            dir: meta.is_dir(),
            readonly: meta.permissions().readonly(),
            modified: meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|since| since.as_secs()),
        })
    }

    fn mkdir(&self, path: &str) -> Result<(), FsError> {
        let resolved = self.allow_write(path)?;
        // 递归建：脚本写 `fs_mkdir("out/a/b")` 时中间层没建是常态，
        // 一层一层报错只会逼作者写循环。
        std::fs::create_dir_all(&resolved)
            .map_err(|e| FsError::Io(format!("建不了目录 {}：{e}", resolved.display())))
    }

    /// 删文件或**空**目录。不递归：一条命令清空整棵树的代价太大。
    fn remove(&self, path: &str) -> Result<(), FsError> {
        let resolved = self.allow_write(path)?;
        let meta = std::fs::symlink_metadata(&resolved)
            .map_err(|e| FsError::Io(format!("查不到 {}：{e}", resolved.display())))?;
        let result = if meta.is_dir() {
            std::fs::remove_dir(&resolved)
        } else {
            std::fs::remove_file(&resolved)
        };
        result.map_err(|e| FsError::Io(format!("删不掉 {}：{e}", resolved.display())))
    }

    fn authorize_read(&self, path: &str) -> Result<PathBuf, FsError> {
        self.authorize(path, false)
    }

    fn authorize_write(&self, path: &str) -> Result<PathBuf, FsError> {
        self.authorize(path, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use term_core::script_caps::{CapRequest, Capabilities};

    /// 一个临时目录 + 只在测试范围内有效的授权。
    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zterm-fs-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        std::fs::canonicalize(&dir).expect("规范化临时目录")
    }

    fn caps_for(root: &Path, write: bool) -> Capabilities {
        let mut caps = Capabilities::NONE;
        caps.grant(&CapRequest::FsRead(root.to_path_buf()));
        if write {
            caps.grant(&CapRequest::FsWrite(root.to_path_buf()));
        }
        caps
    }

    #[test]
    fn read_and_write_stay_inside_the_declared_root() {
        let root = sandbox("inside");
        let caps = caps_for(&root, true);
        let files = LocalFiles::new(|| caps.clone());
        let target = root.join("a.txt").display().to_string();

        assert_eq!(files.write(&target, b"hello", false).expect("写"), 5);
        assert_eq!(files.read(&target).expect("读"), b"hello");
        // 追加。
        files.write(&target, b"!", true).expect("追加");
        assert_eq!(files.read(&target).expect("读"), b"hello!");

        let listing = files.list(root.to_str().expect("utf8")).expect("列");
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].name, "a.txt");
        assert!(!listing[0].dir);

        let stat = files.stat(&target).expect("属性");
        assert_eq!(stat.size, 6);
        assert!(!stat.dir);
        assert!(stat.modified.is_some(), "刚写的文件该有 mtime");

        files.remove(&target).expect("删");
        assert!(files.read(&target).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    /// 只授了读：写必须被拒，且错误里说清是「只授了读」。
    #[test]
    fn a_read_only_grant_refuses_writes() {
        let root = sandbox("readonly");
        let caps = caps_for(&root, false);
        let files = LocalFiles::new(|| caps.clone());
        let target = root.join("b.txt").display().to_string();
        match files.write(&target, b"x", false) {
            Err(FsError::Denied(why)) => assert!(why.contains("只授了读"), "{why}"),
            other => panic!("期望被拒，实得 {other:?}"),
        }
        let _ = std::fs::remove_dir_all(root);
    }

    /// 根外的一律拒——包括「看着在根里、其实经 .. 绕出去」的路径。
    #[test]
    fn paths_outside_the_root_are_refused_before_touching_the_disk() {
        let root = sandbox("outside");
        let caps = caps_for(&root, true);
        let files = LocalFiles::new(|| caps.clone());

        let escaped = format!("{}/../../etc/passwd", root.display());
        match files.read(&escaped) {
            Err(FsError::BadArgument(why)) => assert!(why.contains(".."), "{why}"),
            other => panic!("期望先于规范就拒，实得 {other:?}"),
        }
        match files.read("/etc/passwd") {
            Err(FsError::Denied(_)) => {}
            other => panic!("根外必须拒，实得 {other:?}"),
        }
        // 相对路径也拒：脚本没有「当前目录」这个概念。
        assert!(matches!(files.read("a.txt"), Err(FsError::BadArgument(_))));
        let _ = std::fs::remove_dir_all(root);
    }

    /// 软链不能变成白名单逃逸口：写新文件时也不能跟着一个「指向外部」的软链。
    #[cfg(unix)]
    #[test]
    fn a_symlink_cannot_escape_the_write_allowlist_even_when_dangling() {
        let root = sandbox("symlink");
        let outside = std::env::temp_dir().join(format!(
            "zterm-fs-outside-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&outside).unwrap();
        let outside = std::fs::canonicalize(&outside).unwrap();
        let caps = caps_for(&root, true);
        let files = LocalFiles::new(|| caps.clone());

        // 指向白名单外**已存在**文件：normalize_for 的 canonicalize 会成功，
        // 但真实目标在白名单外，allow_write 必须拒。
        let secret = outside.join("secret.txt");
        std::fs::write(&secret, b"secret").unwrap();
        let link = root.join("link.txt");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        match files.write(&link.to_string_lossy(), b"X", false) {
            Err(FsError::Denied(_)) => {}
            other => panic!("指向外部的软链必须被拒，实得 {other:?}"),
        }
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret", "目标不能被改写");

        // 悬空软链：canonicalize(raw) 失败会走「写新文件」退路，但落点不能是
        // 软链本身（open 会跟到白名单外创建目标）。
        let dangling = root.join("dangling.txt");
        let outside_target = outside.join("created-by-escape.txt");
        std::os::unix::fs::symlink(&outside_target, &dangling).unwrap();
        assert!(
            files
                .write(&dangling.to_string_lossy(), b"X", false)
                .is_err(),
            "悬空软链必须拒绝写入"
        );
        assert!(!outside_target.exists(), "白名单外不能被创建出文件");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// 授权是**每次现问**的：构造之后信封变宽，下一次调用就该放行。
    ///
    /// 这条钉住的是「用户在文件框里手选了一个路径 → 工作区把它追加进信封」
    /// 那条路；构造期判定会让它白拿。
    #[test]
    fn a_widened_envelope_is_honoured_without_rebuilding_the_port() {
        let root = sandbox("widen");
        let granted = std::sync::Arc::new(AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&granted);
        let root_for_caps = root.clone();
        let files = LocalFiles::new(move || {
            if flag.load(Ordering::SeqCst) {
                caps_for(&root_for_caps, true)
            } else {
                Capabilities::NONE
            }
        });
        let target = root.join("c.txt").display().to_string();
        assert!(matches!(
            files.write(&target, b"x", false),
            Err(FsError::Denied(_))
        ));
        granted.store(true, Ordering::SeqCst);
        assert_eq!(files.write(&target, b"x", false).expect("放行"), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    /// `fs_mkdir("out/a/b")` 的中间层都不存在：规范化要从最长存在的祖先
    /// （沙箱根）拼起，再交给 `create_dir_all`，不能因为在 `out/a` 上
    /// `canonicalize` 失败就报 Io。
    #[test]
    fn mkdir_creates_the_whole_missing_chain_inside_the_granted_root() {
        let root = sandbox("mkdir-chain");
        let files = LocalFiles::new(|| caps_for(&root, true));
        let target = root.join("out/a/b");
        files.mkdir(&target.to_string_lossy()).expect("递归建成");
        assert!(target.is_dir());
        assert!(root.join("out/a").is_dir());

        let read_only = LocalFiles::new(|| caps_for(&root, false));
        match read_only.mkdir(&root.join("out/c").to_string_lossy()) {
            Err(FsError::Denied(why)) => assert!(why.contains("只授了读"), "{why}"),
            other => panic!("只读根不该能建目录，实得 {other:?}"),
        }
        assert!(!root.join("out/c").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn mkdir_cannot_follow_a_symlink_to_outside_the_granted_root() {
        let root = sandbox("mkdir-symlink");
        let outside = std::env::temp_dir().join(format!(
            "zterm-fs-mkdir-outside-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&outside).expect("外部目录");
        let outside = std::fs::canonicalize(&outside).expect("规范外部目录");
        let files = LocalFiles::new(|| caps_for(&root, true));

        // 中间层是软链：规范到白名单外，check_write 必须拒。
        let link = root.join("escape");
        std::os::unix::fs::symlink(&outside, &link).expect("软链");
        let target = link.join("made-by-script");
        match files.mkdir(&target.to_string_lossy()) {
            Err(FsError::Denied(_)) => {}
            other => panic!("软链逃逸必须拒，实得 {other:?}"),
        }
        assert!(!outside.join("made-by-script").exists(), "外部不能被建目录");
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }
}
