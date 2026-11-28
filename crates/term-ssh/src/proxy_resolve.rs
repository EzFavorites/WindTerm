//! 连接链的第一段解析：DNS 必须阻塞在有界的工作线程上，不能钉住 async worker。

use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

use crate::error::SshError;

/// `connect_timeout = 0` 的语义是「不设超时」。但 `tokio::time::timeout(0, …)`
/// 是**立刻超时**——不统一映射的话，用户在表单里填 0 会在第一跳就失败。
/// 所有阶段（TCP / 代理协商 / KEX / 开通道）都经这里换算成一个长兜底，
/// 认证阶段另有自己的 120 秒下限（见 `handshake`）。
pub(crate) const NO_TIMEOUT_FALLBACK: Duration = Duration::from_secs(120);

/// 每段共用的超时换算：0 映射成兜底值，其余原样返回。
pub(crate) fn stage_timeout(timeout: Duration) -> Duration {
    if timeout.is_zero() {
        NO_TIMEOUT_FALLBACK
    } else {
        timeout
    }
}

/// 主机 + 端口 → 一个具体地址。DNS 失败与「连上了但被拒」是两类错误。
pub(crate) fn resolve_host(host: &str, port: u16) -> Result<SocketAddr, SshError> {
    (host, port)
        .to_socket_addrs()
        .map_err(|e| SshError::Resolve(format!("{host}:{port} —— {e}")))?
        .next()
        .ok_or_else(|| SshError::Resolve(format!("{host} 没有解析出任何地址")))
}

/// 带预算的解析：`getaddrinfo` 是阻塞调用，直接放在 async 线程里会把整条
/// 连接链钉住（DNS 卡住时连 `connect_timeout` 都没机会生效）。搬进
/// `spawn_blocking` 再套一层超时，时间到就返回 `Resolve`，工作线程自己收摊。
pub(crate) async fn resolve_host_async(
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<SocketAddr, SshError> {
    let owned = host.to_string();
    let task = tokio::task::spawn_blocking(move || resolve_host(&owned, port));
    await_resolve(host, port, timeout, task).await
}

/// 等解析任务但预算有界。单独抽出来是为了让「超时」分支能在单测里用一条
/// 保证不返回的 future 钉住——直接拿 `Duration::ZERO` 去赌本机 DNS 没跑完
/// 是会 flaky 的（回环解析有时真的能抢在计时器之前完成）。
async fn await_resolve(
    host: &str,
    port: u16,
    timeout: Duration,
    task: tokio::task::JoinHandle<Result<SocketAddr, SshError>>,
) -> Result<SocketAddr, SshError> {
    match tokio::time::timeout(timeout, task).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => Err(SshError::Resolve(format!(
            "{host}:{port} —— DNS 工作线程退出：{e}"
        ))),
        Err(_) => Err(SshError::Resolve(format!(
            "{host}:{port} —— {timeout:?} 内没有解析出地址"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_stage_timeout_never_means_an_immediate_timeout() {
        assert_eq!(stage_timeout(Duration::ZERO), NO_TIMEOUT_FALLBACK);
        assert_eq!(stage_timeout(Duration::from_secs(5)), Duration::from_secs(5));
    }

    /// 用一条确定不返回的 future 钉住超时分支：DNS 卡住时必须在预算内
    /// 得到 Resolve 错误，而不是把整条连接链永久挂住。
    #[tokio::test]
    async fn dns_resolution_is_bounded_by_the_stage_timeout() {
        let stuck = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok("127.0.0.1:22".parse().unwrap())
        });
        let expired = await_resolve("example.com", 22, Duration::from_millis(30), stuck)
            .await
            .expect_err("预算一到必须返回，不能跟着解析一起卡住");
        assert!(matches!(expired, SshError::Resolve(_)), "{expired:?}");
        assert!(expired.to_string().contains("没有解析出地址"), "{expired}");

        let ok = resolve_host_async("127.0.0.1", 22, Duration::from_secs(2))
            .await
            .expect("本机回环应当解析成功");
        assert_eq!(ok.ip().to_string(), "127.0.0.1");
    }
}
