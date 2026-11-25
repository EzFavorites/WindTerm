//! 脚本的**远端文件端口**：把 `term_sftp::BlockingClient` 接成 `RemoteFiles`。
//!
//! # 为什么在 desk
//!
//! `term-script` 只声明「远端文件长什么样」；「拿哪条会话的凭据去连」是应用
//! 策略（配置、OneKey 方案、保险库都在这一侧）。所以实现在这里，与
//! `session_open` / `script_host` 同一层。
//!
//! # 懒连接 + 一次一条
//!
//! 第一个 `sftp_*` 动作才建连接，之后复用同一条：一条脚本里连列三个目录，
//! 不该付三次认证的钱（几百毫秒 × 3）。连接失败**不缓存**——下次动作会重试，
//! 因为「刚才连不上」不等于「现在连不上」。
//!
//! # 只对内置 SSH 会话可用
//!
//! 系统 `ssh` 客户端的会话（`builtin_ssh == false`）里跑的是**别人的进程**，
//! 我们手上没有能开 sftp 子系统的连接对象。那种会话报 `Unsupported` 并说清
//! 原因，而不是悄悄改用另一套连接参数——「同一个会话名，两条不同的连接路径」
//! 正是 ADR-0018 修掉的那类分叉。

use std::sync::{Arc, Mutex};

use term_core::session::SessionConfig;
use term_core::session_runtime::Cancel;
use term_script::{DirEntry, FileStat, FsError, RemoteFiles, SessionsError};
use term_session::SessionPool;
use term_sftp::blocking::{bridge_cancel, DEFAULT_TRANSFER_TIMEOUT};
use term_sftp::{BlockingClient, Direction};

use crate::session_open::ssh_options_for;

/// 为某条会话建一个远端文件端口。**不在这里连接**（见模块文档的懒连接）。
///
/// `pool` 只用来做一件事：把会话选的 OneKey 方案应用到配置上
/// （[`SessionPool::resolve_onekey`]）。控制面是这件事的唯一出处——
/// 脚本这条路径要是自己再查一次 `prefs`，就会出现「终端连得上、SFTP 传不了」
/// 那种最难解释的分歧。
pub(crate) fn for_session(
    pool: &SessionPool,
    config: &SessionConfig,
) -> Result<std::sync::Arc<dyn RemoteFiles>, SessionsError> {
    if !config.builtin_ssh {
        return Err(SessionsError::Unsupported(format!(
            "会话 {:?} 用的是系统 ssh 客户端，脚本拿不到它的连接句柄，\
             开不了 SFTP；把这条会话改成「内置 SSH 客户端」再试",
            config.name
        )));
    }
    if config.host.trim().is_empty() {
        return Err(SessionsError::Unsupported(format!(
            "会话 {:?} 没有填主机名，无从建立 SFTP 连接",
            config.name
        )));
    }
    Ok(std::sync::Arc::new(SftpRemote {
        name: config.name.clone(),
        config: pool.resolve_onekey(config),
        client: Mutex::new(None),
    }))
}

/// 一条会话的远端文件端口。内部持一条懒建立的连接。
pub(crate) struct SftpRemote {
    /// 会话名：错误信息里要说清「是哪条会话传不了文件」。
    name: String,
    config: SessionConfig,
    client: Mutex<Option<BlockingClient>>,
}

impl SftpRemote {
    /// 拿现成的连接，没有就建一条。
    ///
    /// **持锁做连接**：同一脚本里两个线程同时传文件时，第二条会等第一条连完
    /// 再复用同一条连接——比各连一条更接近 SFTP 的实际用法（多路请求本来就
    /// 跑在一条会话上）。
    fn client(&self) -> Result<std::sync::MutexGuard<'_, Option<BlockingClient>>, FsError> {
        let mut slot = match self.client.lock() {
            Ok(slot) => slot,
            // 中毒只说明别的线程持锁时崩过；连接本身还能用就继续用。
            Err(poisoned) => poisoned.into_inner(),
        };
        if slot.is_none() {
            let opts = ssh_options_for(&self.config, 80, 24).map_err(|failure| {
                FsError::Unsupported(format!("{:?} 连不上：{failure}", self.name))
            })?;
            let client = BlockingClient::connect(opts).map_err(|e| {
                FsError::Io(format!("会话 {:?} 的 SFTP 连接失败：{e:?}", self.name))
            })?;
            *slot = Some(client);
        }
        Ok(slot)
    }

    /// 用一次连接执行一个动作；动作失败**不重连**（重连会把真正的错误原因
    /// 换成「第二次也失败了」）。
    fn with<T>(
        &self,
        what: &str,
        action: impl FnOnce(&BlockingClient) -> Result<T, term_sftp::SftpError>,
    ) -> Result<T, FsError> {
        let slot = self.client()?;
        let client = slot
            .as_ref()
            .ok_or_else(|| FsError::Unsupported("远端连接没有建立起来".to_string()))?;
        action(client).map_err(|e| FsError::Io(format!("{what}：{e:?}")))
    }

    /// 一次可取消的传输：把脚本的「停止」桥到传输循环，并套上默认超时。
    ///
    /// `bridge` 必须在 `with` 之后才析构：`Drop` 会 join 轮询线程，放在锁内
    /// 意义不大，但放在动作之前会让取消在传输期间失去中继。
    fn transfer_cancellable(
        &self,
        what: &str,
        direction: Direction,
        remote: &str,
        local: &str,
        cancel: Arc<dyn Cancel>,
    ) -> Result<u64, FsError> {
        // 人已经按了停止：连连接都不建，也不碰目标文件。
        if cancel.is_cancelled() {
            return Err(FsError::Cancelled);
        }
        let (token, bridge) = bridge_cancel(cancel.clone());
        let outcome = self.with(what, |client| match direction {
            Direction::Download => {
                client.get_cancellable(remote, local, &token, DEFAULT_TRANSFER_TIMEOUT)
            }
            Direction::Upload => {
                client.put_cancellable(local, remote, &token, DEFAULT_TRANSFER_TIMEOUT)
            }
        });
        drop(bridge);
        match outcome {
            // 传输侧看见旗子后报的是 Session("传输被取消")；源令牌才是判据。
            Err(_) if cancel.is_cancelled() => Err(FsError::Cancelled),
            other => other,
        }
    }
}

impl RemoteFiles for SftpRemote {
    fn list(&self, path: &str) -> Result<Vec<DirEntry>, FsError> {
        if path.is_empty() {
            return Err(FsError::BadArgument("sftp_list 的路径不能为空".to_string()));
        }
        let entries = self.with("列目录", |client| client.list(path))?;
        Ok(entries
            .into_iter()
            .map(|entry| DirEntry {
                name: entry.name,
                dir: entry.is_dir,
                size: entry.size,
            })
            .collect())
    }

    fn get(&self, remote: &str, local: &str) -> Result<u64, FsError> {
        check_paths("sftp_get", remote, local)?;
        self.with("下载", |client| client.get(remote, local))
    }

    fn put(&self, local: &str, remote: &str) -> Result<u64, FsError> {
        check_paths("sftp_put", local, remote)?;
        self.with("上传", |client| client.put(local, remote))
    }

    fn get_cancellable(
        &self,
        remote: &str,
        local: &str,
        cancel: Arc<dyn Cancel>,
    ) -> Result<u64, FsError> {
        check_paths("sftp_get", remote, local)?;
        self.transfer_cancellable("下载", Direction::Download, remote, local, cancel)
    }

    fn put_cancellable(
        &self,
        local: &str,
        remote: &str,
        cancel: Arc<dyn Cancel>,
    ) -> Result<u64, FsError> {
        check_paths("sftp_put", local, remote)?;
        self.transfer_cancellable("上传", Direction::Upload, remote, local, cancel)
    }

    fn mkdir(&self, path: &str) -> Result<(), FsError> {
        if path.is_empty() {
            return Err(FsError::BadArgument(
                "sftp_mkdir 的路径不能为空".to_string(),
            ));
        }
        self.with("建目录", |client| client.mkdir(path))
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), FsError> {
        check_paths("sftp_rename", from, to)?;
        self.with("改名", |client| client.rename(from, to))
    }

    /// 删文件或**空**目录：按 `stat` 的结果分派。
    ///
    /// 不做递归删除——一条命令清空整棵树的代价太大，脚本要删树就自己走
    /// `list` + `remove`。目录非空时服务端会拒绝，错误原样带回来。
    fn remove(&self, path: &str) -> Result<(), FsError> {
        if path.is_empty() {
            return Err(FsError::BadArgument(
                "sftp_remove 的路径不能为空".to_string(),
            ));
        }
        let stat = self.with("查属性", |client| client.stat(path))?;
        if stat.is_dir {
            self.with("删目录", |client| client.remove_dir(path))
        } else {
            self.with("删文件", |client| client.remove_file(path))
        }
    }

    fn stat(&self, path: &str) -> Result<FileStat, FsError> {
        if path.is_empty() {
            return Err(FsError::BadArgument("sftp_stat 的路径不能为空".to_string()));
        }
        let stat = self.with("查属性", |client| client.stat(path))?;
        Ok(FileStat {
            path: path.to_string(),
            size: stat.size,
            dir: stat.is_dir,
            readonly: stat.readonly,
            modified: stat.mtime,
        })
    }
}

/// 两个路径参数都不能空。名字进错误信息：脚本里同一个动作可能出现好几次，
/// 只说「参数为空」等于让作者猜是哪一句。
fn check_paths(what: &str, first: &str, second: &str) -> Result<(), FsError> {
    if first.is_empty() || second.is_empty() {
        return Err(FsError::BadArgument(format!("{what} 的两个路径都不能为空")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use term_core::session::Protocol;

    #[derive(Default)]
    struct Flag(Arc<AtomicBool>);

    impl Cancel for Flag {
        fn is_cancelled(&self) -> bool {
            self.0.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// 已经按了停止就不建连接：否则取消测试会卡在认证上，看不到真正的判据。
    #[test]
    fn a_pre_cancelled_script_transfer_is_rejected_before_connecting() {
        let remote = SftpRemote {
            name: "test".to_string(),
            config: SessionConfig::new(Protocol::Ssh),
            client: Mutex::new(None),
        };
        let flag = Arc::new(Flag::default());
        flag.0.store(true, std::sync::atomic::Ordering::SeqCst);
        match remote.get_cancellable("remote.txt", "local.txt", flag) {
            Err(FsError::Cancelled) => {}
            other => panic!("已取消的传输必须返回 Cancelled，实得 {other:?}"),
        }
    }
}
