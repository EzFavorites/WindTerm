//! 同步的 SFTP 客户端：一次连接 + 逐个请求，全部阻塞返回。
//!
//! # 为什么另起一套（而不是复用 [`crate::background`]）
//!
//! `background` 是**给界面**的：一条线程 + `Request`/`Event` + 每帧轮询。它的
//! 形状由「界面不能阻塞」决定。而脚本线程**本来就是阻塞的**——脚本写
//! `sftp_get("/var/log/syslog", "./syslog")` 之后下一句就该拿到结果或错误，
//! 套一层「发请求、等事件、自己配对」只会把一件同步的事写成异步。
//!
//! 两边共用同一条连接层（[`term_ssh::Authenticated`]）与同一份传输实现
//! （[`crate::transfer`]），所以「列出来的东西」与「传下去的字节」是同一套
//! 语义，不会出现「界面能传、脚本传不了」这类分叉。
//!
//! # tokio 笼子
//!
//! 异步只活在本文件内部的 `current_thread` 运行时里，对外零 `.await`
//! （ADR-0017 §决策 2 的边界：tokio 只许出现在 `term-ssh` / `term-tunnel` /
//! `term-sftp`，而 `apps/desktop` 不该看得见它）。脚本侧因此拿到的是一个普通的
//! `Send + Sync` 结构，与 `LocalFs` 那些端口一样好用。
//!
//! # 生命周期
//!
//! [`Client`] 持有一条已认证的连接。丢弃时尽力挂断（`Drop` 里 `block_on` 一次
//! 断开流程）；脚本跑完、`Client` 出作用域，服务端那边不会留一条半死的会话。

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use russh_sftp::client::SftpSession;
use term_core::session_runtime::Cancel;
use term_ssh::{Authenticated, SshOptions};

use crate::cancel::CancelToken;
use crate::entry::{self, Entry};
use crate::error::SftpError;
use crate::transfer::{self, Direction};

/// 单次阻塞传输的默认上限。脚本线程本来就阻塞；没有上限时一次卡死连
/// 「停止」都等不到响应，超时是取消之外的第二道保险。
pub const DEFAULT_TRANSFER_TIMEOUT: Duration = Duration::from_secs(60);

/// 桥接轮询间隔，与脚本的 50ms 切片同一量级。
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// 把只读取消源（脚本的取消令牌经 [`Cancel`] 抽象）桥成传输循环轮询的
/// [`CancelToken`]。
///
/// 为什么是轮询线程而不是回调：脚本侧只有一个原子布尔，没有通知机制；
/// 传输侧也只读一个原子布尔。两边都不该为了「取消」引入 channel 的所有权约定。
/// 返回的 guard 析构时停轮询并 join，避免一条传输结束后还留着线程。
pub fn bridge_cancel(source: Arc<dyn Cancel>) -> (CancelToken, CancelBridge) {
    let target = CancelToken::new();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let worker_target = target.clone();
    let handle = thread::spawn(move || {
        while !worker_stop.load(Ordering::SeqCst) {
            if source.is_cancelled() {
                worker_target.cancel();
                return;
            }
            thread::sleep(CANCEL_POLL);
        }
    });
    (
        target,
        CancelBridge {
            stop,
            handle: Some(handle),
        },
    )
}

/// [`bridge_cancel`] 的守卫。析构即停轮询。
pub struct CancelBridge {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for CancelBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 一条同步 SFTP 连接。
pub struct Client {
    /// `current_thread` 运行时：本结构的全部 async 都跑在它上面。
    rt: tokio::runtime::Runtime,
    /// 留着它才对：断开要经它（`SftpSession` 只关了子系统通道，认证会话还在）。
    auth: Option<Authenticated>,
    sftp: SftpSession,
    home: String,
}

impl Client {
    /// 连上并按 SFTP 子系统握手，问到家目录。
    ///
    /// 失败分三类（[`SftpError`]）：连不上/认证失败是 `Ssh`，连上了但子系统
    /// 谈不起来（服务端关了 sftp、`MaxSessions` 满了）是 `Session`。脚本据此
    /// 决定「换个网络」还是「换个做法」，所以不合并成一句「SFTP 失败」。
    pub fn connect(opts: SshOptions) -> Result<Self, SftpError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| SftpError::Session(format!("起不了 SFTP 运行时：{e}")))?;
        let (auth, sftp, home) = rt.block_on(async move {
            let auth = Authenticated::connect(&opts).await?;
            // 子系统请求不带 want_reply：可用性由下面的版本握手判定。
            let channel = auth.opener().subsystem("sftp").await?;
            let sftp = SftpSession::new(channel.into_stream())
                .await
                .map_err(|e| SftpError::Session(e.to_string()))?;
            let home = sftp
                .canonicalize(".")
                .await
                .map_err(|e| SftpError::Session(format!("问不到家目录：{e}")))?;
            Ok::<_, SftpError>((auth, sftp, home))
        })?;
        Ok(Self {
            rt,
            auth: Some(auth),
            sftp,
            home,
        })
    }

    /// 服务端认定的家目录（绝对路径）。
    pub fn home(&self) -> &str {
        &self.home
    }

    /// 列一层目录（已排序，目录在前；`.` 与 `..` 已丢弃）。
    pub fn list(&self, path: &str) -> Result<Vec<Entry>, SftpError> {
        let sftp = &self.sftp;
        self.rt.block_on(async move {
            let rd = sftp
                .read_dir(path)
                .await
                .map_err(|e| SftpError::Session(format!("列不出 {path}：{e}")))?;
            Ok(entry::collect(rd))
        })
    }

    /// 远端 → 本地。覆盖同名文件，不续传（脚本要续传就自己先看本地大小）。
    pub fn get(&self, remote: &str, local: &str) -> Result<u64, SftpError> {
        self.get_cancellable(remote, local, &CancelToken::new(), DEFAULT_TRANSFER_TIMEOUT)
    }

    /// 本地 → 远端。
    pub fn put(&self, local: &str, remote: &str) -> Result<u64, SftpError> {
        // 参数顺序以 `transfer` 的形参名为准：它先收远端、后收本地，与
        // 公开方法 `put(local, remote)` 相反。曾因这里照抄 `put` 的顺序把
        // 两端传反：`upload` 拿到的「本地」其实是远端路径，于是报
        // 「读不到本地文件 /root/xxx」。改顺序时先对一遍形参名。
        self.put_cancellable(local, remote, &CancelToken::new(), DEFAULT_TRANSFER_TIMEOUT)
    }

    /// 可取消 + 带上限的下载。脚本侧由 [`bridge_cancel`] 把「停止」接进 `cancel`。
    pub fn get_cancellable(
        &self,
        remote: &str,
        local: &str,
        cancel: &CancelToken,
        timeout: Duration,
    ) -> Result<u64, SftpError> {
        self.transfer(Direction::Download, remote, local, cancel, timeout)
    }

    /// 可取消 + 带上限的上传（公开参数顺序仍是先本地后远端）。
    pub fn put_cancellable(
        &self,
        local: &str,
        remote: &str,
        cancel: &CancelToken,
        timeout: Duration,
    ) -> Result<u64, SftpError> {
        self.transfer(Direction::Upload, remote, local, cancel, timeout)
    }

    /// 远端 / 本地的顺序**固定**为「先远端后本地」，与 `download` 的签名一致；
    /// `put` 这种公开参数顺序相反的方法要自己换位，不要在调用处直接搬参数。
    fn transfer(
        &self,
        direction: Direction,
        remote: &str,
        local: &str,
        cancel: &CancelToken,
        timeout: Duration,
    ) -> Result<u64, SftpError> {
        let sftp = &self.sftp;
        self.rt.block_on(async move {
            let mut no_progress = |_done: u64, _total: u64| {};
            let action = async {
                let result = match direction {
                    Direction::Download => {
                        transfer::download(
                            sftp,
                            remote,
                            local,
                            false,
                            false,
                            cancel,
                            &mut no_progress,
                        )
                        .await
                    }
                    Direction::Upload => {
                        transfer::upload(
                            sftp,
                            local,
                            remote,
                            false,
                            false,
                            cancel,
                            &mut no_progress,
                        )
                        .await
                    }
                };
                result.map_err(|e| match e {
                    transfer::TransferError::Cancelled { .. } => {
                        SftpError::Session("传输被取消".to_string())
                    }
                    transfer::TransferError::Failed(message) => SftpError::Session(message),
                })
            };
            bounded(timeout, action).await
        })
    }

    /// 建一层目录（父目录必须已存在；不做 `mkdir -p`）。
    pub fn mkdir(&self, path: &str) -> Result<(), SftpError> {
        let sftp = &self.sftp;
        self.rt.block_on(async move {
            sftp.create_dir(path)
                .await
                .map_err(|e| SftpError::Session(format!("建不了目录 {path}：{e}")))
        })
    }

    /// 改名 / 移动。目标已存在时**由服务端决定**（多数会覆盖）。
    pub fn rename(&self, from: &str, to: &str) -> Result<(), SftpError> {
        let sftp = &self.sftp;
        self.rt.block_on(async move {
            sftp.rename(from, to)
                .await
                .map_err(|e| SftpError::Session(format!("改不了名 {from} → {to}：{e}")))
        })
    }

    /// 删一个文件。目录会被服务端拒绝——要删空目录用 [`Client::remove_dir`]。
    pub fn remove_file(&self, path: &str) -> Result<(), SftpError> {
        let sftp = &self.sftp;
        self.rt.block_on(async move {
            sftp.remove_file(path)
                .await
                .map_err(|e| SftpError::Session(format!("删不掉文件 {path}：{e}")))
        })
    }

    /// 删一个**空**目录（不递归：一条命令清空整棵树的代价太大）。
    pub fn remove_dir(&self, path: &str) -> Result<(), SftpError> {
        let sftp = &self.sftp;
        self.rt.block_on(async move {
            sftp.remove_dir(path)
                .await
                .map_err(|e| SftpError::Session(format!("删不掉目录 {path}（必须先是空的）：{e}")))
        })
    }

    /// 一条路径的属性。`follow` 为真时跟随符号链接。
    ///
    /// 返回值里 `size` / `mtime` 允许「服务端没报」：这两个字段在协议里是
    /// `option`，把「不知道」写成 0 会让调用方以为「这是个 1970 年的空文件」。
    pub fn stat(&self, path: &str) -> Result<RemoteStat, SftpError> {
        let sftp = &self.sftp;
        self.rt.block_on(async move {
            let meta = sftp
                .metadata(path)
                .await
                .map_err(|e| SftpError::Session(format!("查不到 {path} 的属性：{e}")))?;
            Ok(RemoteStat {
                size: meta.size.unwrap_or(0),
                is_dir: meta.is_dir(),
                // 权限位：服务端没报时按「可写」处理——**不**凭空说成只读，
                // 那会让 `stat().readonly` 变成一个假事实。
                readonly: meta
                    .permissions
                    .map(|mode| mode & 0o200 == 0)
                    .unwrap_or(false),
                mtime: meta.mtime.map(u64::from),
            })
        })
    }

    /// 主动挂断。之后任何操作都会失败（`SftpSession` 已关）。
    ///
    /// 不需要调用：`Drop` 会做同一件事。但脚本在收尾时显式调一次，能让服务端
    /// 那边的会话早一点消失。
    pub fn close(mut self) {
        self.disconnect();
    }

    fn disconnect(&mut self) {
        let Some(auth) = self.auth.take() else {
            return;
        };
        let sftp = &self.sftp;
        self.rt.block_on(async move {
            let _ = sftp.close().await;
            auth.disconnect().await;
        });
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.disconnect();
    }
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 不打印任何连接细节（主机名在 `SshOptions` 里，本结构没存；
        // 而套接字里的东西本来就不该进日志）。
        f.debug_struct("sftp::Client")
            .field("home", &self.home)
            .finish()
    }
}

/// 给一次传输套上限。超时与业务失败分开报，免得脚本把「卡住」当成「文件不存在」。
async fn bounded<T, F>(timeout: Duration, future: F) -> Result<T, SftpError>
where
    F: Future<Output = Result<T, SftpError>>,
{
    match tokio::time::timeout(timeout, future).await {
        Ok(result) => result,
        Err(_) => Err(SftpError::Session(format!(
            "传输超过 {timeout:?} 仍未完成（已中断，可重试）"
        ))),
    }
}

/// [`Client::stat`] 的结果。字段口径与 `term_script::FileStat` 对齐。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteStat {
    pub size: u64,
    pub is_dir: bool,
    pub readonly: bool,
    /// 修改时间（Unix 秒）；服务端没报时为 `None`。
    pub mtime: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// 测试用的只读取消源（脚本侧令牌在本 crate 里只以 [`Cancel`] 形态出现）。
    #[derive(Clone, Default)]
    struct Source(Arc<AtomicBool>);

    impl Source {
        fn cancel(&self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    impl Cancel for Source {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn a_cancelled_script_token_reaches_the_transfer_flag() {
        let source = Arc::new(Source::default());
        let (token, bridge) = bridge_cancel(Arc::clone(&source) as Arc<dyn Cancel>);
        assert!(!token.is_cancelled());
        source.cancel();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !token.is_cancelled() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(token.is_cancelled(), "脚本按停止后传输循环要看见取消");
        drop(bridge);
    }

    #[test]
    fn dropping_the_bridge_stops_relaying_cancellation() {
        let source = Arc::new(Source::default());
        let (token, bridge) = bridge_cancel(Arc::clone(&source) as Arc<dyn Cancel>);
        drop(bridge);
        source.cancel();
        thread::sleep(Duration::from_millis(100));
        assert!(
            !token.is_cancelled(),
            "传输结束后桥必须停：否则下一次传输会一上来就被取消"
        );
    }

    #[tokio::test]
    async fn a_transfer_past_its_deadline_reports_a_timeout() {
        let err = bounded(Duration::from_millis(20), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok::<_, SftpError>(())
        })
        .await
        .expect_err("超时应当显式失败");
        assert!(
            matches!(&err, SftpError::Session(msg) if msg.contains("未完成")),
            "{err:?}"
        );
    }
}
