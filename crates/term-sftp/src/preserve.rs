//! 保留文件时间戳（SFTP 传输的「保留时间戳」需求）。
//!
//! 一句话：传完之后，把源文件的 mtime 写到目标上。
//!
//! - 下载：源是**远端**，目标是**本地**——`download` 开头 `metadata(remote)`
//!   已经拿到了远端 mtime，直接用它，传完 `set_times` 写回本地；
//! - 上传：源是**本地**，目标是**远端**——`upload` 开头 `fs::metadata(local)`
//!   已经拿到了本地 mtime，直接用它，传完 `set_metadata` 写到远端。
//!
//! 两处都**不再单独发一次 metadata / stat**：开头那次查询已经给了需要的东西，
//! 传完再查一遍既多一个往返，又可能读到「传输期间被别人改过」的新值——那正是
//! 要避免的。调用方把开头读到的 mtime 传进来即可。
//!
//! 服务端没报 mtime（`None`）就什么都不做：写 `0` 等于把时间设成 1970，
//! 比不设更糟。写回失败（只读挂载、远端拒绝 `setstat`）也不让整个传输失败——
//! 文件已经传好了，时间戳是锦上添花。
//!
//! 只写 mtime，不碰 atime：`FileTimes::new()` 默认不动没显式设置的时间，
//! 正好符合「保留修改时间」而不是「伪造访问时间」。

use std::fs::FileTimes;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileAttributes;

/// `SystemTime` → 秒（SFTP 的 mtime 是 `u32` 秒）。
///
/// 早于 1970（会溢出）或超出 `u32` 范围（2106 年后）返回 `None`——与其截断成一个
/// 错误的时间，不如不写。
pub fn to_epoch_secs(t: SystemTime) -> Option<u32> {
    let secs = t.duration_since(UNIX_EPOCH).ok()?.as_secs();
    u32::try_from(secs).ok()
}

/// 秒 → `SystemTime`。`u32` 秒一定能表示，不会失败。
pub fn from_epoch_secs(secs: u32) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(u64::from(secs))
}

/// 把 mtime 写到本地文件。`None` = 不做（远端没报）。
///
/// 打开一个可写句柄再 `set_times`：`FileTimes` 只改显式设置的时间，
/// atime 保持不动。文件只读导致打不开时，`set_times` 也就无从谈起，静默返回。
pub fn write_local_mtime(local: &str, mtime: Option<u32>) {
    let Some(secs) = mtime else {
        return;
    };
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(local) {
        let times = FileTimes::new().set_modified(from_epoch_secs(secs));
        let _ = file.set_times(times);
    }
}

/// 把 mtime 写到一个**本地目录**。目录不能用 `OpenOptions::write(true)`
/// 打开（Unix 上是 EISDIR），所以这里用只读句柄；`FileTimes` 只改显式设置的
/// 时间。失败静默——目录已经建好，时间是锦上添花。
pub fn write_local_dir_mtime(path: &std::path::Path, mtime: Option<u32>) {
    let Some(secs) = mtime else {
        return;
    };
    if let Ok(dir) = std::fs::File::open(path) {
        let times = FileTimes::new().set_modified(from_epoch_secs(secs));
        let _ = dir.set_times(times);
    }
}

/// 把 mtime 写到远端文件。`None` = 不做（本地没读到）。
///
/// 只塞 `mtime`，其余字段留 `None`——`setstat` 只更新带标志的字段，
/// 不会把 size / permissions 抹平。远端拒绝（权限、文件系统不支持）静默返回。
pub async fn write_remote_mtime(sftp: &SftpSession, remote: &str, mtime: Option<u32>) {
    write_remote_attrs(sftp, remote, None, mtime).await;
}

/// 一次 `setstat` 把**权限**与 mtime 写到远端。两个都给 `None` 就什么都不做。
///
/// 为什么合并成一次：最常见的情形（新建文件 + 保留时间戳，也就是目录同步的
/// 每一条上传）两者都要写，分两次就是每个文件多一个往返，而动辄几千个文件。
///
/// `mode` 传 `None` 是**「不碰权限」**而不是「设成 0」：`setstat` 只更新带标志的
/// 字段。覆盖已有文件时正是靠这一点保住对端原本的 `0600`——把别人的私钥文件
/// 放宽成 `0644` 是数据事故，不是保真度。
///
/// 失败一律静默（与 [`write_remote_mtime`] 同一条纪律）：文件已经传好、目录已经
/// 建好，权限和时间是锦上添花，不该让一次成功的传输因为 `setstat` 被拒而报错。
pub async fn write_remote_attrs(
    sftp: &SftpSession,
    remote: &str,
    mode: Option<u32>,
    mtime: Option<u32>,
) {
    if mode.is_none() && mtime.is_none() {
        return;
    }
    let attrs = FileAttributes {
        permissions: mode,
        mtime,
        ..Default::default()
    };
    let _ = sftp.set_metadata(remote, attrs).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::MetadataExt;

    /// 造一个 mtime 明确（秒级）的本地文件，返回它的路径字符串。
    fn touch(dir: &std::path::Path, name: &str, mtime_secs: u32) -> String {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).expect("建文件");
        f.write_all(b"payload").expect("写字节");
        let times = FileTimes::new().set_modified(from_epoch_secs(mtime_secs));
        f.set_times(times).expect("设 mtime");
        path.to_string_lossy().to_string()
    }

    fn temp(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("term-sftp-preserve-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建目录");
        dir
    }

    #[test]
    fn a_directory_mtime_can_be_written_back() {
        // 目录不能用 OpenOptions::write(true) 打开（Unix EISDIR），所以这条
        // 走的是只读句柄；否则「目录 mtime」这个开关永远只是个摆设。
        let dir = temp("dir-mtime");
        let want = 1_612_345_678u32;
        write_local_dir_mtime(&dir, Some(want));
        let got = std::fs::metadata(&dir)
            .and_then(|m| m.modified())
            .ok()
            .and_then(to_epoch_secs);
        assert_eq!(got, Some(want));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn epoch_seconds_round_trip() {
        let t = from_epoch_secs(1_700_000_000);
        assert_eq!(to_epoch_secs(t), Some(1_700_000_000));
    }

    #[test]
    fn times_before_the_epoch_have_no_mtime() {
        // SFTP 的 mtime 是无符号秒，早于 1970 表达不了——返回 None，别写个负数。
        let before = UNIX_EPOCH - Duration::from_secs(10);
        assert_eq!(to_epoch_secs(before), None);
    }

    #[test]
    fn seconds_past_2106_overflow_u32_and_are_dropped() {
        let far = UNIX_EPOCH + Duration::from_secs(u64::from(u32::MAX) + 1);
        assert_eq!(to_epoch_secs(far), None);
        // 恰好在边界内的仍然可用。
        let edge = UNIX_EPOCH + Duration::from_secs(u64::from(u32::MAX));
        assert_eq!(to_epoch_secs(edge), Some(u32::MAX));
    }

    #[test]
    fn writing_a_local_mtime_changes_the_file_and_reads_back() {
        let dir = temp("write");
        let p = touch(&dir, "a.txt", 1_600_000_000);
        assert_eq!(std::fs::metadata(&p).unwrap().mtime(), 1_600_000_000);
        write_local_mtime(&p, Some(1_700_000_000));
        assert_eq!(std::fs::metadata(&p).unwrap().mtime(), 1_700_000_000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn passing_none_leaves_the_mtime_alone() {
        let dir = temp("none");
        let p = touch(&dir, "b.txt", 1_500_000_000);
        write_local_mtime(&p, None);
        assert_eq!(std::fs::metadata(&p).unwrap().mtime(), 1_500_000_000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_local_file_is_no_error() {
        // 写不存在的文件不该 panic 或拖垮传输：静默返回。
        write_local_mtime("/definitely/not/here/xyz.txt", Some(1_700_000_000));
    }

    #[test]
    fn from_metadata_gives_the_same_seconds() {
        // 本地文件的 mtime（经 MetadataExt）与 from_epoch_secs 造出来的时刻一致，
        // 确认上传方向「读本地 mtime」的取法是对的。
        let dir = temp("meta");
        let p = dir.join("c.txt");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(b"x").unwrap();
        f.set_times(FileTimes::new().set_modified(from_epoch_secs(1_400_000_000)))
            .unwrap();
        let secs = to_epoch_secs(std::fs::metadata(&p).unwrap().modified().unwrap());
        assert_eq!(secs, Some(1_400_000_000));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
