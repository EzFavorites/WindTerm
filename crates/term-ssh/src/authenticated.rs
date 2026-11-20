//! 给 `term-tunnel` 用的接缝：只握手 + 认证，**不开**会话通道。
//!
//! 为什么放在 `term-ssh` 而不是 `term-tunnel`：主机密钥校验（`HostKeyHandler`）、
//! 三种认证、known_hosts 记账都在这一层，隧道只是「借这条连接开别的通道」。
//! 在隧道里复制一份握手，等于把 known_hosts 与认证逻辑写两遍——两边迟早不一致。
//!
//! 这条 API 是**异步的**，与 `connect()` 的同步形状不同，这是故意的：
//! `connect()` 面向 UI（「连上了没有」必须立刻回答），而这里面向另一个库——
//! 隧道要同时开多个通道、跑多个超时，用 `Result` 把异步折叠成同步只会让它
//! 再拆开一次。两者都在 ADR-0017 的笼子里，笼子外看到的仍然只有同步接口。

use std::sync::Arc;

use russh::client::{self, Handle};
use russh::Channel;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::error::SshError;
use crate::handler::HostKeyHandler;
use crate::options::SshOptions;

/// 服务端送进来的一条转发连接（`-R` 用）。
pub struct Forwarded {
    pub channel: Channel<client::Msg>,
    /// 服务端上被连的那个地址 / 端口——多路 `-R` 时用它决定往哪条转发。
    pub connected_address: String,
    pub connected_port: u32,
    pub originator_address: String,
    pub originator_port: u32,
}

/// 一条「连上了、但还没开会话」的 SSH 连接。
pub struct Authenticated {
    opener: ChannelOpener,
    forwarded: UnboundedReceiver<Forwarded>,
}

/// 开通道的手柄。可以 `clone()` 到多个任务里并发开通道——
/// 连接本身是共享的（`Arc<Handle>`），关掉它只发生一次。
#[derive(Clone)]
pub struct ChannelOpener {
    handle: Arc<Handle<HostKeyHandler>>,
}

impl Authenticated {
    /// 握手 + 认证。**不做**任何通道级的事（PTY / shell 都不碰）。
    ///
    /// `opts.proxy_jump` 在这里被兑现：整条链（含最终目标）都是这一层的事，
    /// 调用方拿到的句柄和「直连」时一模一样。
    pub async fn connect(opts: &SshOptions) -> Result<Self, SshError> {
        opts.validate()?;
        let (tx, rx) = unbounded_channel();
        let handle = crate::proxy::connect(opts, Some(tx)).await?;
        Ok(Self {
            opener: ChannelOpener {
                handle: Arc::new(handle),
            },
            forwarded: rx,
        })
    }

    /// 开通道用的手柄（克隆出去之后本对象仍可继续用）。
    pub fn opener(&self) -> ChannelOpener {
        self.opener.clone()
    }

    /// 等一条服务端送进来的转发连接。连接断了返回 `None`。
    pub async fn next_forwarded(&mut self) -> Option<Forwarded> {
        self.forwarded.recv().await
    }

    /// 交出「服务端推来的转发连接」队列。开通道手柄仍可用（它自己持有连接）。
    pub fn into_forwarded(self) -> UnboundedReceiver<Forwarded> {
        self.forwarded
    }

    /// 主动挂断。不发这一下，服务端会留一个半死的会话直到超时。
    pub async fn disconnect(&self) {
        let _ = self
            .opener
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;
    }

    /// 连接是否还活着（用于「隧道断了」的判定）。
    pub fn is_closed(&self) -> bool {
        self.opener.handle.is_closed()
    }
}

impl ChannelOpener {
    /// 让服务器连到 `host:port`，把通道开回来（`-L` 与 SOCKS5 共用）。
    pub async fn direct_tcpip(
        &self,
        host: &str,
        port: u16,
        originator: (&str, u16),
    ) -> Result<Channel<client::Msg>, SshError> {
        self.handle
            .channel_open_direct_tcpip(host, u32::from(port), originator.0, u32::from(originator.1))
            .await
            .map_err(|e| SshError::Channel(format!("服务器拒绝转发到 {host}:{port} —— {e}")))
    }

    /// 请服务器在 `bind_addr:bind_port` 上监听，把进来的连接推给我们（`-R`）。
    /// 返回服务器**实际**绑定的端口（`bind_port = 0` 时由服务器挑）。
    pub async fn remote_forward(&self, bind_addr: &str, bind_port: u16) -> Result<u16, SshError> {
        let reply = self
            .handle
            .tcpip_forward(bind_addr, u32::from(bind_port))
            .await
            .map_err(|e| {
                SshError::Channel(format!(
                    "服务器不允许在 {bind_addr}:{bind_port} 上监听（服务端的 AllowTcpForwarding 关着？）—— {e}"
                ))
            })?;
        // 协议只在「请求的是 0 端口」时才会在应答里带端口（RFC 4254 §7.1）；
        // 其余情况 russh 给的是 0，实际端口就是请求的那个。这一条是 `-R` 的
        // 端到端测试抓出来的：直接用应答值会让所有规则记成「监听 0 端口」，
        // 于是服务端推来的连接一条都匹配不上。
        Ok(if reply == 0 && bind_port != 0 {
            bind_port
        } else {
            reply as u16
        })
    }

    /// 开一条会话通道并请求子系统（`term-sftp` 用 `"sftp"`）。
    ///
    /// 与 `connect()` 里那条路（PTY + shell）是同一族，只是不带 PTY：
    /// 子系统不需要终端尺寸，也不该要——要了反而会让服务端以为有人在敲键盘。
    /// `want_reply` 传 `false`：子系统的可用性由协议内部握手判定
    /// （SFTP 版本交换谈不拢会超时），比一个「收到了」的空应答有意义。
    pub async fn subsystem(&self, name: &str) -> Result<Channel<client::Msg>, SshError> {
        let channel = self.handle.channel_open_session().await.map_err(|e| {
            SshError::Channel(format!("服务器不给开会话通道（要 {name} 子系统）—— {e}"))
        })?;
        channel
            .request_subsystem(false, name)
            .await
            .map_err(|e| SshError::Channel(format!("请求 {name} 子系统失败—— {e}")))?;
        Ok(channel)
    }

    /// 开一条会话通道并执行一条远端命令（不带 PTY），返回**裸 channel** 让调用方
    /// 自己收数据。第二 exec 通道 / 远端 Sysmon 的异步接缝就是这里；同步形状见
    /// [`crate::exec_capture`]。
    ///
    /// russh 0.65 的方法名是 `Channel::exec`（不是 `request_exec`），签名
    /// `exec<A: Into<Vec<u8>>>(want_reply: bool, command: A)`。
    /// 命令按 UTF-8 原始字节上送：SSH 线上携带的是不透明字节串，显式转换比依赖
    /// `&str → Vec<u8>` 的隐式 `Into` 更清楚地钉住了「发出去的就是这些字节」。
    ///
    /// `want_reply` 取 `true`，与 `subsystem()` 的 `false` 是**有意的差别**：
    /// - `Channel::exec()` 只是把 `ChannelMsg::Exec` 塞进发送队列（`send_msg`），
    ///   `await` 返回 `Ok` 仅代表「排进去了」，**不代表服务端受理**；
    /// - RFC 4254 §6.5 规定 `want_reply=true` 时服务端必须回
    ///   `SSH_MSG_CHANNEL_SUCCESS` / `SSH_MSG_CHANNEL_FAILURE`，russh 把它们映射成
    ///   `ChannelMsg::Success` / `ChannelMsg::Failure` 送进 `channel.wait()`；
    /// - 这是「服务端拒绝 exec」（例如 `AllowUsers` 限制、子系统未启用、命令被禁）
    ///   唯一的可靠信号。若传 `false`，服务端按协议**不回任何东西**，调用方只能看到
    ///   EOF/Close，无法把「命令被拒」和「命令跑完但没输出」区分开，
    ///   错误分类也就无从谈起（W6-C3 明确要求这两类不同）；
    /// - `subsystem()` 之所以能用 `false`，是因为 SFTP 之后还有自己的版本握手来判活；
    ///   `exec` 没有第二层协商，只能靠这个回执。
    ///
    /// 官方范例 `examples/client_exec_simple.rs` 同样用 `exec(true, command)`。
    /// 代价是 `wait()` 的第一条消息会是 Success/Failure，收数据的一侧必须显式处理
    /// 这两个变体——[`crate::exec_capture`] 已经处理。
    pub async fn exec(&self, command: &str) -> Result<Channel<client::Msg>, SshError> {
        let channel = self.handle.channel_open_session().await.map_err(|e| {
            SshError::Channel(format!("服务器不给开会话通道（要执行 {command}）—— {e}"))
        })?;
        channel
            .exec(true, command.as_bytes().to_vec())
            .await
            .map_err(|e| SshError::Channel(format!("请求执行 {command} 失败—— {e}")))?;
        Ok(channel)
    }

    /// 挂断连接（与 `Authenticated::disconnect` 同一件事，只是从手柄那边调）。
    pub async fn disconnect(&self) {
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;
    }

    /// 撤销一条 `-R` 监听。
    pub async fn cancel_remote_forward(&self, bind_addr: &str, bind_port: u16) {
        let _ = self
            .handle
            .cancel_tcpip_forward(bind_addr, u32::from(bind_port))
            .await;
    }
}

/// `UnboundedSender<Forwarded>` 的类型别名，`handler.rs` 用它接住服务端推来的通道。
pub(crate) type ForwardSink = UnboundedSender<Forwarded>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_connection_reports_itself_closed() {
        // 不连网络：用一个空 options 直接验证 `validate` 先把错误挡在前面。
        let mut opts = SshOptions::new(
            "",
            "root",
            crate::options::AuthMethod::Password {
                password: "x".to_string(),
            },
        );
        opts.host.clear();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let e = match rt.block_on(Authenticated::connect(&opts)) {
            Ok(_) => panic!("空主机名不该连上"),
            Err(e) => e,
        };
        assert!(matches!(e, SshError::Config(_)), "{e:?}");
    }
}
