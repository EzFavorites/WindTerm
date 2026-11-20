//! `term-desk serve` 的参数解析与入口（ADR-0022）。
//!
//! 参数解析是纯函数（[`parse`]），因为 `run` 会一直阻塞——把两者混在一起，
//! 想测「`--port abc` 该报错」就得先起一个监听。

use std::path::PathBuf;

use crate::bridge::{self, Config};

const USAGE: &str = "\
term-desk serve [--port N] [--token-file PATH]

  把能力面暴露到 127.0.0.1（只回环，没有 --host）。每个连接的第一条请求
  必须握手：

    {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"auth\",\"params\":{\"token\":\"<token>\"}}

  --port N          默认 47653；0 表示让内核分配（真实端口打印到 stdout）
  --token-file PATH 默认 ~/.zterm/bridge.token（首次运行以 0600 生成）
                    权限宽于 0600 会拒绝启动，请先 chmod 600
";

/// 解析失败返回可读的原因（`run` 会连同用法一起打出来）。
pub fn parse(rest: &[String]) -> Result<Config, String> {
    let mut port = bridge::DEFAULT_PORT;
    let mut token_path: Option<PathBuf> = None;
    let mut index = 0;
    while index < rest.len() {
        match rest[index].as_str() {
            "--port" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--port 后面需要一个数字".to_string())?;
                // 解析失败只有一种结果：报错。不悄悄退回默认端口——那会让
                // 「我以为监听在 9000」变成一次静默的错配。
                port = raw
                    .parse::<u16>()
                    .map_err(|_| format!("--port 只接受 0..=65535 的整数，实际是 {raw:?}"))?;
                index += 2;
            }
            "--token-file" => {
                let raw = rest
                    .get(index + 1)
                    .ok_or_else(|| "--token-file 后面需要一个路径".to_string())?;
                token_path = Some(PathBuf::from(raw));
                index += 2;
            }
            other => return Err(format!("不认识的参数：{other}")),
        }
    }
    let token_path = match token_path {
        Some(path) => path,
        None => bridge::token::default_path()
            .ok_or_else(|| "定位不到家目录，请用 --token-file 显式指定".to_string())?,
    };
    Ok(Config::new(port, token_path))
}

/// 阻塞运行。返回退出码：0 正常收场，1 启动失败（token 权限 / 端口被占）。
pub fn run(rest: &[String]) -> i32 {
    let cfg = match parse(rest) {
        Ok(cfg) => cfg,
        Err(msg) => {
            eprintln!("{msg}\n\n{USAGE}");
            return 2;
        }
    };
    match bridge::serve(cfg) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("desk serve 起不来：{err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(rest: &[&str]) -> Vec<String> {
        rest.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_defaults_are_loopback_with_the_standard_port() {
        let cfg = parse(&argv(&[])).expect("默认参数必须能解析");
        assert_eq!(cfg.port, bridge::DEFAULT_PORT);
        assert!(cfg.token_path.ends_with("bridge.token"));
    }

    #[test]
    fn port_zero_is_allowed_for_ephemeral_binding() {
        assert_eq!(parse(&argv(&["--port", "0"])).expect("0 合法").port, 0);
    }

    #[test]
    fn a_bad_port_is_an_error_not_a_fallback() {
        assert!(parse(&argv(&["--port", "abc"])).is_err());
        assert!(parse(&argv(&["--port", "70000"])).is_err(), "u16 之外");
        assert!(parse(&argv(&["--port", "-1"])).is_err());
        assert!(parse(&argv(&["--port"])).is_err(), "缺值也要报错");
    }

    #[test]
    fn the_token_file_can_be_pointed_elsewhere() {
        let cfg = parse(&argv(&["--token-file", "/tmp/x.token"])).expect("解析");
        assert_eq!(cfg.token_path, PathBuf::from("/tmp/x.token"));
        assert!(parse(&argv(&["--token-file"])).is_err());
    }

    #[test]
    fn there_is_no_host_option_on_purpose() {
        // 远程访问走 SSH 隧道（见模块文档）。这里钉住「参数不存在」，
        // 免得将来有人顺手加一个 `--host` 把能力面挂到局域网上。
        assert!(parse(&argv(&["--host", "0.0.0.0"])).is_err());
        assert!(parse(&argv(&["--listen", "0.0.0.0"])).is_err());
    }
}
