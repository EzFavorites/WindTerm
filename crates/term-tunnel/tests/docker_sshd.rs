//! 经**真 sshd** 的隧道二进制完整性（Docker）。
//!
//! 与 `tests/e2e.rs` 的分工：那边用同库服务端验接缝（`direct-tcpip` 参数顺序、
//! SOCKS5 半包、`-R` 的回推）；这边验的是另一件事——**1 MiB 随机字节过隧道后
//! 是否逐字节相同**，对端是容器里真实的 `socat … EXEC:/bin/cat`。
//!
//! 为什么这条值得单独跑：终端里看文本「能用」不代表字节流没问题。压缩包、
//! 图片、二进制协议都会把「少一个字节」「多一次重传」放大成无法恢复的损坏，
//! 而这类问题在文本测试里完全看不出来。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use term_ssh::{AuthMethod, HostKeyPolicy, SshOptions};
use term_tunnel::{Forward, Tunnel};

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("缺少环境变量 {name}：请用 scripts/docker-verify.sh 起容器后运行本测试")
    })
}

fn opts() -> SshOptions {
    let mut opts = SshOptions::new(
        var("WINDTERM_DOCKER_HOST"),
        var("WINDTERM_DOCKER_USER"),
        AuthMethod::Password {
            password: var("WINDTERM_DOCKER_PASS"),
        },
    );
    opts.port = var("WINDTERM_DOCKER_PORT").parse().expect("端口");
    opts.host_key = HostKeyPolicy::Insecure;
    opts.connect_timeout = Duration::from_secs(10);
    opts.keepalive = None;
    opts
}

/// 确定性的伪随机字节（xorshift64*）。
///
/// 为什么不用 `rand`：这条测试要的是「不可压缩、不可预测」的字节流来暴露
/// 截断与错位，而不是密码学随机；固定种子还能在失败时复现同一份输入。
fn payload(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let v = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.truncate(len);
    out
}

fn connect_local(port: u16) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(30)))
                    .expect("设置读超时");
                stream
                    .set_write_timeout(Some(Duration::from_secs(30)))
                    .expect("设置写超时");
                return stream;
            }
            Err(e) if Instant::now() < deadline => {
                let _ = e;
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => panic!("本地转发端口 {port} 连不上：{e}"),
        }
    }
}

#[test]
#[ignore = "需要 Docker 里的真 sshd：bash scripts/docker-verify.sh tunnel"]
fn a_megabyte_of_random_bytes_survives_the_tunnel_byte_for_byte() {
    // `-L 127.0.0.1:0 → 127.0.0.1:9999`：容器里的 socat 把收到的字节原样回送。
    // 绑 0 端口让内核挑一个空闲端口，避免并发 / 重跑时撞端口。
    // `Forward` 明确拒绝端口 0（「用户没法知道连哪」），所以自己先借一个空闲端口：
    // 绑 0 让内核挑，拿到号就立刻放开，再用这个号建转发。
    let probe = TcpListener::bind("127.0.0.1:0").expect("借一个空闲端口");
    let local_port = probe.local_addr().expect("本地地址").port();
    drop(probe);
    let forwards = vec![Forward::local(
        format!("127.0.0.1:{local_port}"),
        "127.0.0.1",
        9999,
    )];
    let mut tunnel = Tunnel::start(opts(), forwards).expect("隧道应能起来");
    let ports = tunnel.ports();
    assert_eq!(ports.len(), 1, "一条规则应对应一个监听端口：{ports:?}");
    let local = ports[0];

    let sent = payload(1024 * 1024, 0x9E37_79B9_7F4A_7C15);
    let stream = connect_local(local);

    // 写在一个线程里：1 MiB 超过本地 socket 缓冲区，边写边读才不会互相等死。
    let to_send = sent.clone();
    let writer = std::thread::spawn(move || {
        let mut stream = stream;
        stream.write_all(&to_send).expect("写进隧道");
        stream.flush().ok();
        // 半关闭：告诉对端「我发完了」，`cat` 才会在读完时关闭连接。
        stream.shutdown(std::net::Shutdown::Write).ok();
        stream
    });

    let mut stream = writer.join().expect("写线程");
    let mut got = Vec::with_capacity(sent.len());
    let mut buf = [0u8; 64 * 1024];
    while got.len() < sent.len() {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => got.extend_from_slice(&buf[..n]),
            Err(e) => panic!("读回隧道失败（已收 {} 字节）：{e}", got.len()),
        }
    }

    assert_eq!(got.len(), sent.len(), "长度不一致：隧道截断了数据");
    assert!(got == sent, "字节不一致：隧道改动了数据");
    tunnel.close();
}
