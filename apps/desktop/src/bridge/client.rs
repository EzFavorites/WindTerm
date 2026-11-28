//! 前台控制端点的薄客户端（`cli/api` 与 `cli/mcp` 共用）。
//!
//! 只做三件事：连固定路径的 UDS、发一行 JSON-RPC、把 `{code, message, kind}`
//! 错误还原成可读诊断。**不建会话池、不装配 HostEnv、不认识会话实现**。
//! 前台没起来就明确报错退出，绝不退化成「自己起一个后端」。

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::net::UnixStream;

use serde_json::{json, Value};

/// 客户端侧的失败。区分「前台没起」与「请求失败」：两种情况给用户的动作不同。
#[derive(Debug)]
pub enum ClientError {
    NoHome,
    Unsupported(&'static str),
    NotRunning { path: PathBuf },
    Io(std::io::Error),
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
            ClientError::NoHome => write!(f, "定位不到家目录，无法确定控制端点路径"),
            ClientError::Unsupported(why) => write!(f, "当前平台不支持控制端点：{why}"),
            ClientError::NotRunning { path } => write!(
                f,
                "前台未运行：{} 不存在。请先启动 WindTerm 窗口；\
                 本命令不会自行创建会话池",
                path.display()
            ),
            ClientError::Io(err) => write!(f, "控制端点 IO 失败：{err}"),
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

#[cfg(unix)]
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
}

#[cfg(not(unix))]
pub struct Client;

impl Client {
    /// 连固定路径。socket 不存在时**明确**报「前台未运行」，不尝试别的路径。
    #[cfg(unix)]
    pub fn connect(path: &Path) -> Result<Self, ClientError> {
        if !path.exists() {
            return Err(ClientError::NotRunning {
                path: path.to_path_buf(),
            });
        }
        let writer = UnixStream::connect(path).map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                ClientError::NotRunning {
                    path: path.to_path_buf(),
                }
            }
            _ => ClientError::Io(err),
        })?;
        let reader = BufReader::new(writer.try_clone()?);
        Ok(Self {
            reader,
            writer,
            next_id: 0,
        })
    }

    #[cfg(not(unix))]
    pub fn connect(_path: &Path) -> Result<Self, ClientError> {
        Err(ClientError::Unsupported(
            "Windows 命名管道尚未实现；当前只支持 unix domain socket",
        ))
    }

    pub fn connect_default() -> Result<Self, ClientError> {
        let path = super::default_socket_path().ok_or(ClientError::NoHome)?;
        Self::connect(&path)
    }

    /// 发一条带 id 的请求并等它的回包。`auth` 不存在：第一条就是普通方法。
    #[cfg(unix)]
    pub fn call(&mut self, method: &str, params: &Value) -> Result<Value, ClientError> {
        self.next_id += 1;
        let request = json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": method,
            "params": params,
        });
        serde_json::to_writer(&mut self.writer, &request)
            .map_err(|err| ClientError::Io(std::io::Error::other(err)))?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;

        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err(ClientError::Malformed(
                "控制端点在回包前关闭了连接".to_string(),
            ));
        }
        let reply: Value = serde_json::from_str(line.trim())
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

    #[cfg(not(unix))]
    pub fn call(&mut self, _method: &str, _params: &Value) -> Result<Value, ClientError> {
        Err(ClientError::Unsupported(
            "Windows 命名管道尚未实现；当前只支持 unix domain socket",
        ))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    fn scratch() -> (std::path::PathBuf, UnixListener) {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("zterm-client-{}-{n}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("control.sock");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("绑定临时 socket");
        (path, listener)
    }

    #[test]
    fn a_missing_socket_is_reported_as_front_end_not_running() {
        let (path, _listener) = scratch();
        // 把路径改成一个绝不存在的文件名：listener 占着 path 本身。
        let absent = path.with_file_name("absent.sock");
        assert!(matches!(
            Client::connect(&absent),
            Err(ClientError::NotRunning { .. })
        ));
    }

    #[test]
    fn an_rpc_error_keeps_code_message_and_kind_verbatim() {
        let (path, listener) = scratch();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            use std::io::BufRead;
            let mut line = String::new();
            BufReader::new(&mut stream).read_line(&mut line).unwrap();
            stream
                .write_all(
                    "{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32602,\"message\":\"坏参数\",\"kind\":\"invalid_params\"}}\n".as_bytes(),
                )
                .unwrap();
        });
        let mut client = Client::connect(&path).expect("连接");
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
    fn a_result_comes_back_without_any_auth_handshake() {
        let (path, listener) = scratch();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut line = String::new();
            BufReader::new(&mut stream).read_line(&mut line).unwrap();
            assert!(line.contains("\"method\":\"describe\""), "首条就是普通方法：{line}");
            assert!(!line.contains("auth"), "不得有 auth 握手：{line}");
            stream
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"methods\":[]}}\n")
                .unwrap();
        });
        let mut client = Client::connect(&path).expect("连接");
        assert_eq!(
            client.call("describe", &json!({})).expect("结果"),
            json!({"methods": []})
        );
    }
}
