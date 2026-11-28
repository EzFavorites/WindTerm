//! 前台控制端点的薄客户端（`cli/api` 与 `cli/mcp` 共用）。
//!
//! 全平台一套：连 `127.0.0.1:<port>`，先跑传输级三帧挑战证明，再发一行
//! JSON-RPC，把 `{code, message, kind}` 错误还原成可读诊断。**不建会话池、
//! 不装配 HostEnv、不认识会话实现**。前台没起来就明确报错退出，绝不退化成
//! 「自己起一个后端」。
//!
//! # 握手时序（传输外壳，不是能力面的 `auth` 方法）
//!
//! 1. 客户端从 `~/.zterm/control-key.local-<port>` 读 key；
//! 2. `auth.challenge{"nonce": "<hex>"}` —— 客户端先发随机数；
//! 3. `auth.proof{"alg":"hmac-sha256","signature":"<hex>"}` —— 服务端用
//!    HMAC-SHA256 证明它持有同一把 key；
//! 4. 客户端验过证明**之后**才发 `auth.key{"key":"<hex>"}`；服务端比对一致
//!    才进入能力面。这三帧没有 `jsonrpc` / `id` 字段，不进 `describe`。
//!
//! # at-most-once
//!
//! 请求行一旦写出，就再也不重发：写失败或等回包时失联，一律报「可能已执行」。
//! 对 `wait` / `session_open` / `write` 这类有副作用的动词，重发会重复执行。

use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;

use serde_json::{json, Value};

use super::control::{
    self, constant_time_eq, hmac_hex, proof_message, read_frame, write_frame, CHALLENGE_FIELD,
    KEY_FIELD, PROOF_FIELD,
};

/// 客户端侧的失败。区分「前台没起」「准入失败」「可能已执行」与「请求失败」：
/// 各自给用户的动作不同。
#[derive(Debug)]
pub enum ClientError {
    NoHome,
    NotRunning { port: u16 },
    Io(io::Error),
    /// 传输级挑战证明失败（含服务端 HMAC 签名不匹配）。
    Auth(String),
    /// 请求行已写出但对端失联：可能已执行，绝不重发。
    PossiblyExecuted,
    /// 前台如实回上来的失败三元组，一位都不改。
    Rpc {
        code: i32,
        message: String,
        kind: String,
    },
    Malformed(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NoHome => write!(f, "定位不到家目录，无法读取控制 key 文件"),
            ClientError::NotRunning { port } => write!(
                f,
                "前台未运行：127.0.0.1:{port} 不可连接（或 key 文件不存在）。\
                  请先启动 WindTerm 窗口；本命令不会自行创建会话池"
            ),
            ClientError::Io(err) => write!(f, "控制端点 IO 失败：{err}"),
            ClientError::Auth(why) => write!(f, "控制端点准入失败：{why}"),
            ClientError::PossiblyExecuted => write!(
                f,
                "请求已写出但对端失联，可能已执行；本客户端不重发"
            ),
            ClientError::Rpc { code, message, kind } => {
                write!(f, "[{kind}:{code}] {message}")
            }
            ClientError::Malformed(why) => write!(f, "控制端点回包不合协议：{why}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<std::io::Error> for ClientError {
    fn from(err: std::io::Error) -> Self {
        ClientError::Io(err)
    }
}

pub struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
}

impl Client {
    /// 用调用方给的 key 连 `127.0.0.1:<port>` 并跑完挑战证明。
    /// 前台不在（连接被拒 / 没有 key 文件）时**明确**报「前台未运行」。
    pub fn connect(port: u16, key: &str) -> Result<Self, ClientError> {
        let writer = TcpStream::connect((control::LOOPBACK_IP, port)).map_err(|err| match err.kind() {
            io::ErrorKind::ConnectionRefused
            | io::ErrorKind::NotFound
            | io::ErrorKind::TimedOut => ClientError::NotRunning { port },
            _ => ClientError::Io(err),
        })?;
        // 握手与首条回包共用 30 秒上限；握手完成后解除（wait 可能等很久）。
        writer.set_read_timeout(Some(control::FIRST_REQUEST_TIMEOUT))?;
        let mut client = Self {
            reader: BufReader::new(writer.try_clone()?),
            writer,
            next_id: 0,
        };
        client.handshake(key)?;
        Ok(client)
    }

    /// 生产入口：端口与 key 文件都从本机事实推导，没有 --socket 之类的参数。
    pub fn connect_default() -> Result<Self, ClientError> {
        let port = control::control_port();
        let path = control::key_path(port).ok_or(ClientError::NoHome)?;
        let key = control::read_key(&path).map_err(|err| match err.kind() {
            io::ErrorKind::NotFound => ClientError::NotRunning { port },
            _ => ClientError::Io(err),
        })?;
        Self::connect(port, &key)
    }

    /// 三帧传输外壳。服务端证明通过前绝不发 key —— 下面的 `auth.key` 写点
    /// 只在 HMAC 校验通过之后才执行到。
    fn handshake(&mut self, key: &str) -> Result<(), ClientError> {
        let nonce = control::random_hex().map_err(ClientError::Io)?;
        write_frame(
            &mut self.writer,
            &json!({CHALLENGE_FIELD: {"nonce": nonce}}),
        )?;
        let proof = read_frame(&mut self.reader)?.ok_or_else(|| {
            ClientError::Auth("服务端在挑战证明前关闭了连接".to_string())
        })?;
        let signature = proof
            .get(PROOF_FIELD)
            .and_then(|field| field.get("signature").or(Some(field)))
            .and_then(Value::as_str)
            .ok_or_else(|| ClientError::Auth("服务端没有回 HMAC-SHA256 证明".to_string()))?;
        let expected = hmac_hex(key.as_bytes(), &proof_message(&nonce))?;
        if !constant_time_eq(signature.as_bytes(), expected.as_bytes()) {
            return Err(ClientError::Auth("服务端 HMAC-SHA256 证明不匹配".to_string()));
        }
        // 证明通过之前绝不发 key：这一行绝对不能挪到上面的校验之前。
        write_frame(&mut self.writer, &json!({KEY_FIELD: {"key": key}}))?;
        self.writer.set_read_timeout(None)?;
        Ok(())
    }

    /// 发一条带 id 的请求并等它的回包。能力面没有 `auth` 方法：第一条就是普通方法。
    pub fn call(&mut self, method: &str, params: &Value) -> Result<Value, ClientError> {
        self.next_id += 1;
        let request = json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": method,
            "params": params,
        });
        let line = serde_json::to_string(&request)
            .map_err(|err| ClientError::Io(io::Error::other(err)))?;
        // at-most-once：写出之后无论发生什么都不会再写第二遍。
        if self.writer.write_all(line.as_bytes()).is_err()
            || self.writer.write_all(b"\n").is_err()
            || self.writer.flush().is_err()
        {
            return Err(ClientError::PossiblyExecuted);
        }
        let mut reply_line = String::new();
        match self.reader.read_line(&mut reply_line) {
            Ok(0) | Err(_) => return Err(ClientError::PossiblyExecuted),
            Ok(_) => {}
        }
        let reply: Value = serde_json::from_str(reply_line.trim())
            .map_err(|err| ClientError::Malformed(err.to_string()))?;
        if let Some(err) = reply.get("error") {
            return Err(ClientError::Rpc {
                code: err["code"].as_i64().unwrap_or(-32603) as i32,
                message: err["message"].as_str().unwrap_or("").to_string(),
                kind: err["kind"].as_str().unwrap_or("internal").to_string(),
            });
        }
        Ok(reply.get("result").cloned().unwrap_or(Value::Null))
    }
}

// 分开写两个 cfg（等价于 `all(test, unix)`）：4p 按第一个 `#[cfg(test)]`
// 截断生产段，合成一个属性时这一整块测试会被算进生产 panic 预算。
#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::time::Duration;

    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    /// 临时服务端：只跑三帧握手，然后按脚本回一条协议行。返回客户端连接到的端口。
    fn fake_server(
        proof: impl Fn(&str) -> String + Send + 'static,
        reply: &'static str,
    ) -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("临时端口");
        let port = listener.local_addr().expect("地址").port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream.set_read_timeout(Some(Duration::from_secs(5))).expect("超时");
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let challenge = read_frame(&mut reader).expect("读挑战").expect("有挑战");
            let nonce = challenge[CHALLENGE_FIELD]["nonce"].as_str().expect("nonce");
            write_frame(&mut stream, &json!({PROOF_FIELD: {"alg":"hmac-sha256","signature":proof(nonce)}}))
                .expect("回证明");
            // 假服务端不需要验证客户端 key，但要读完这一帧才能继续回协议行。
            let _ = read_frame(&mut reader).expect("读 key 帧");
            stream.write_all(reply.as_bytes()).expect("回包");
            stream.flush().expect("冲刷");
        });
        port
    }

    fn good_proof(nonce: &str) -> String {
        hmac_hex(KEY.as_bytes(), &proof_message(nonce)).expect("HMAC")
    }

    #[test]
    fn the_three_frames_come_in_proof_then_key_order() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("临时端口");
        let port = listener.local_addr().expect("地址").port();
        let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen_by_server = Arc::clone(&seen);
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let challenge = read_frame(&mut reader).expect("读挑战").expect("有挑战");
            seen_by_server.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(challenge[CHALLENGE_FIELD]["nonce"].as_str().unwrap().len(), 64);
            let nonce = challenge[CHALLENGE_FIELD]["nonce"].as_str().unwrap();
            write_frame(&mut stream, &json!({PROOF_FIELD: {"alg":"hmac-sha256","signature":good_proof(nonce)}}))
                .expect("回证明");
            assert_eq!(seen_by_server.load(std::sync::atomic::Ordering::SeqCst), 1,
                "客户端必须等证明到手才发 key");
            let key_frame = read_frame(&mut reader).expect("读 key").expect("有 key");
            assert_eq!(key_frame[KEY_FIELD]["key"].as_str(), Some(KEY));
            seen_by_server.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            stream.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n").expect("回包");
        });
        let mut client = Client::connect(port, KEY).expect("连接");
        assert_eq!(client.call("describe", &json!({})).expect("结果"), json!({"ok": true}));
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    fn a_wrong_proof_never_reveals_the_key() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("临时端口");
        let port = listener.local_addr().expect("地址").port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let _ = read_frame(&mut reader).expect("读挑战");
            write_frame(&mut stream, &json!({PROOF_FIELD: {"alg":"hmac-sha256","signature":"00"}}))
                .expect("回一个错的证明");
            stream.set_read_timeout(Some(Duration::from_millis(200))).expect("超时");
            let mut line = String::new();
            // 证明没通过，客户端绝不能发 key：这里应该什么也读不到。
            assert_eq!(reader.read_line(&mut line).unwrap_or(0), 0, "错证明后不得发 key：{line}");
        });
        assert!(matches!(Client::connect(port, KEY), Err(ClientError::Auth(_))));
    }

    #[test]
    fn an_rpc_error_keeps_code_message_and_kind_verbatim() {
    a_missing_front_end_is_reported_without_starting_a_pool();
        let port = fake_server(
            good_proof,
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32602,\"message\":\"坏参数\",\"kind\":\"invalid_params\"}}\n",
        );
        let mut client = Client::connect(port, KEY).expect("连接");
        match client.call("wait", &json!({})) {
            Err(ClientError::Rpc { code, message, kind }) => {
                assert_eq!(code, -32602);
                assert_eq!(message, "坏参数");
                assert_eq!(kind, "invalid_params");
            }
            other => panic!("错误三元组必须逐字带回来：{other:?}"),
        }
    }

    #[test]
    fn a_lost_connection_after_write_is_reported_as_possibly_executed() {
    a_wrong_proof_never_reveals_the_key();
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("临时端口");
        let port = listener.local_addr().expect("地址").port();
        let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen_by_server = Arc::clone(&seen);
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let challenge = read_frame(&mut reader).expect("读挑战").expect("有挑战");
            let nonce = challenge[CHALLENGE_FIELD]["nonce"].as_str().expect("nonce");
            write_frame(&mut stream, &json!({PROOF_FIELD: {"alg":"hmac-sha256","signature":good_proof(nonce)}}))
                .expect("回证明");
            let _ = read_frame(&mut reader).expect("读 key 帧");
            let mut line = String::new();
            reader.read_line(&mut line).expect("读请求");
            assert!(line.contains("\"method\":\"write\""), "读到的是：{line}");
            seen_by_server.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            drop(stream); // 请求写出了，但回包前失联
        });
        let mut client = Client::connect(port, KEY).expect("连接");
        assert!(matches!(
            client.call("write", &json!({})),
            Err(ClientError::PossiblyExecuted)
        ));
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1, "绝不重发");
    }

    fn a_missing_front_end_is_reported_without_starting_a_pool() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("占一个端口");
        let port = listener.local_addr().expect("地址").port();
        drop(listener);
        assert!(matches!(Client::connect(port, KEY), Err(ClientError::NotRunning { .. })));
    }
}
