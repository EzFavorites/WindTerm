//! 远端写操作的规则（纯函数）：路径与权限模式的判据。
//!
//! 为什么单独一个模块：`create_dir` / `rename` / `remove` / `chmod` 四条请求的
//! **协议部分**没有可测的东西（就是一次调用），值得测的恰恰是「什么算合法参数」。
//! 把判据抽出来，UI 侧的输入就可以在不连服务器的情况下被钉住——
//! 否则「权限填了 9999」这类错误要等到远端回一句含糊的失败才知道。

/// 权限位的上限：`0o7777` 覆盖 setuid / setgid / sticky 与 rwx 九位。
///
/// 超过这个数说明调用方把别的东西（文件类型位、十进制 755）传进来了。
/// 两种都可能被服务端静默截断，也可能被当成非法——都不该由我们替用户猜。
pub const MAX_MODE: u32 = 0o7777;

/// 文件类型位（`S_IFMT`）与权限位的分界。
const TYPE_MASK: u32 = 0o170000;
const PERM_MASK: u32 = 0o7777;

/// 路径必须是绝对或相对的**非空**路径。
///
/// 空串在多数服务端会被解释成「当前目录」，`rmdir("")` 于是变成一次
/// 对家目录的删除请求——这种「参数错误升级成破坏性操作」必须先在这里挡住。
pub fn check_path(path: &str) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("路径不能是空串（空路径在服务端会被当成当前目录）".to_string());
    }
    Ok(())
}

/// 一段文件名里在所有常见文件系统上都不安全的字符。
///
/// 保守集：`< > : " / \ | ? *` 与 ASCII 控制字符。`/` 与 `\` 在这里只是
/// 名字里的字符（路径分隔符由调用方按段拆开后才会拼接），出现在名字里就
/// 会把一段变成两段，必须替换。Unicode、空格、`.` 都保留。
const INVALID_NAME_CHARS: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// 把一段文件名里的非法字符换成 `_`。
///
/// 只动**文件名**，不碰路径分隔符（调用方按 `/` 拆好逐段调用）。
/// 两个不同源名替换后可能撞成同一个目标名，这里不发明后缀策略——
/// 交给文件系统的既有覆盖语义，并在设置页说明这一条。
pub fn sanitize_component(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_control() || INVALID_NAME_CHARS.contains(&ch) {
                '_'
            } else {
                ch
            }
        })
        .collect()
}

/// 权限模式必须落在 `0o0000`–`0o7777`。
pub fn check_mode(mode: u32) -> Result<(), String> {
    if mode > MAX_MODE {
        return Err(format!(
            "权限模式 {mode:#o} 超出范围：只接受 0–0o7777（十进制 755 会被误读成 0o1363）"
        ));
    }
    Ok(())
}

/// 合并出要写回的完整 `mode`。
///
/// `chmod` 在 SFTP 上是 `SSH_FXP_SETSTAT`：服务端拿到的是一整个 `mode`，
/// 里面**既有**文件类型位（`S_IFREG` / `S_IFDIR` / 符号链接）**也有**权限位。
/// 只写 `0o755` 会把类型位清零，有的服务端就此把文件当成未知类型。
///
/// 所以：保留已知的类型位，替换权限位。`existing` 为 `None` 说明服务端没报，
/// 那就只写权限位（这时没什么可保留的）。
pub fn merge_permissions(existing: Option<u32>, requested: u32) -> u32 {
    let kept = existing.unwrap_or(0) & TYPE_MASK;
    kept | (requested & PERM_MASK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_path_is_rejected_before_it_reaches_the_server() {
        assert!(check_path("").is_err());
        assert!(check_path("   ").is_err());
        assert!(check_path("/").is_ok());
        assert!(check_path("sub/file.txt").is_ok());
    }

    #[test]
    fn invalid_name_characters_become_underscores() {
        assert_eq!(sanitize_component("a/b"), "a_b");
        assert_eq!(sanitize_component("a\\b"), "a_b");
        assert_eq!(sanitize_component("a:b?c*d"), "a_b_c_d");
        assert_eq!(sanitize_component("a\nb"), "a_b");
        assert_eq!(sanitize_component("你好.txt"), "你好.txt");
        // 路径分隔符不在这里处理：调用方按段拆开后逐段调用。
        assert_eq!(sanitize_component("sub/deep/a.txt"), "sub_deep_a.txt");
    }

    #[test]
    fn the_mode_must_fit_in_the_low_four_octal_digits() {
        assert!(check_mode(0o644).is_ok());
        assert!(check_mode(0o7777).is_ok());
        assert!(check_mode(0).is_ok());
        // 十进制 755：用户以为写的是 rwxr-xr-x，其实是 0o1363。
        // 这里**不替他想**，只保证上限检查会挡住更大的值。
        assert!(check_mode(0o10000).is_err());
        assert!(check_mode(0o177777).is_err());
    }

    #[test]
    fn chmod_keeps_the_file_type_bits() {
        // 0o100644 = 普通文件 + 0644
        assert_eq!(merge_permissions(Some(0o100644), 0o600), 0o100600);
        // 0o040755 = 目录 + 0755
        assert_eq!(merge_permissions(Some(0o040755), 0o700), 0o040700);
    }

    #[test]
    fn chmod_without_reported_permissions_still_writes_the_requested_bits() {
        assert_eq!(merge_permissions(None, 0o600), 0o600);
    }

    #[test]
    fn the_merge_only_takes_permission_bits_from_the_request() {
        // 请求里若混进了类型位（不该发生），也只取低 12 位。
        assert_eq!(merge_permissions(Some(0o040755), 0o100600), 0o040600);
    }
}
