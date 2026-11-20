//! `bridge/mod.rs` 的测试：握手判据、锁屏门、以及真 TCP / 真进程端到端。
//!
//! 单独成文件（与 `surface_tests.rs` / `lock_tests.rs` 同一条理由）：夹具与
//! 断言占的篇幅不该顶爆单文件上限，而 `#[path]` 引入的纯测试模块
//! 不会污染门禁对生产代码的扫描。

    use super::*;
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};
    use term_bridge::error::ErrorKind;

    struct NeverLocked;
    impl lock::LockProbe for NeverLocked {
        fn locked(&self) -> Option<bool> {
            Some(false)
        }
    }

    struct AlwaysLocked;
    impl lock::LockProbe for AlwaysLocked {
        fn locked(&self) -> Option<bool> {
            Some(true)
        }
    }

    fn scratch_token() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zterm-serve-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("时钟")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("临时目录");
        dir.join("bridge.token")
    }

    fn token() -> String {
        "a1".repeat(32)
    }

    // ---- 握手判据（纯函数） ----

    #[test]
    fn a_correct_token_is_accepted() {
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"auth","params":{{"token":"{}"}}}}"#,
            token()
        );
        let (reply, ok) = auth_reply(&line, &token());
        assert!(ok, "正确 token 必须通过：{reply}");
        let value: Value = serde_json::from_str(&reply).expect("回包是 JSON");
        assert_eq!(value["result"]["ok"], json!(true));
        assert_eq!(value["id"], json!(1), "id 必须原样回显");
    }

    #[test]
    fn a_wrong_token_is_refused_with_the_locked_kind() {
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":9,"method":"auth","params":{{"token":"{}"}}}}"#,
            "ff".repeat(32)
        );
        let (reply, ok) = auth_reply(&line, &token());
        assert!(!ok);
        let value: Value = serde_json::from_str(&reply).expect("回包是 JSON");
        assert_eq!(value["error"]["kind"], json!("locked"));
        assert_eq!(value["error"]["code"], json!(-32003));
    }

    #[test]
    fn a_first_request_that_is_not_auth_is_refused() {
        let line = r#"{"jsonrpc":"2.0","id":2,"method":"session_list"}"#;
        let (reply, ok) = auth_reply(line, &token());
        assert!(!ok, "未认证就调能力面必须被拒：{reply}");
        let value: Value = serde_json::from_str(&reply).expect("回包是 JSON");
        assert_eq!(value["error"]["kind"], json!("locked"));
        assert_eq!(
            value["error"]["code"],
            json!(-32003),
            "读的是 ErrorKind::Locked 的码"
        );
    }

    #[test]
    fn garbage_is_a_parse_error_not_a_silent_close() {
        let (reply, ok) = auth_reply("not json", &token());
        assert!(!ok);
        let value: Value = serde_json::from_str(&reply).expect("回包是 JSON");
        assert_eq!(value["error"]["kind"], json!("parse_error"));
        assert_eq!(value["error"]["code"], json!(-32700));
    }

    #[test]
    fn an_empty_token_in_the_request_never_matches() {
        let line = r#"{"jsonrpc":"2.0","id":3,"method":"auth","params":{}}"#;
        assert!(!auth_reply(line, &token()).1);
    }

    // ---- 锁屏门 ----

    #[test]
    fn a_locked_screen_turns_every_call_into_locked() {
        let gate = LockedGate {
            inner: surface::DeskSurface::new(Arc::new(SessionPool::new())),
            probe: Arc::new(AlwaysLocked),
        };
        let err = gate.call("session_list", &json!({})).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Locked);
        assert_eq!(err.code, -32003);
    }

    #[test]
    fn a_locked_screen_also_refuses_to_open_new_sessions() {
        // ADR-0022 的锁屏门要挡的是**新建**会话，不只是读：锁屏期间让一条
        // `session_open` 过去，等于把「屏幕锁了」变成「只是看不见而已」。
        // 门在每一帧调用之前都问一次（`LockedGate::call`），所以 `session_open`
        // 与 `session_list` 同一条口径——这里把它钉住，免得将来有人把它挪进
        // `DeskSurface` 之后只剩握手处那一道。
        let pool = Arc::new(SessionPool::new());
        let gate = LockedGate {
            inner: surface::DeskSurface::new(Arc::clone(&pool)),
            probe: Arc::new(AlwaysLocked),
        };
        for params in [
            json!({ "command": "echo hi" }),
            json!({ "name": "prod" }),
        ] {
            let err = gate.call("session_open", &params).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Locked, "session_open({params}) 应被拒");
            assert_eq!(err.code, -32003);
        }
        assert!(pool.list().is_empty(), "锁屏态下不许留下任何会话");
    }

    // ---- 真 TCP 端到端 ----

    /// 起一个真监听，返回 `(端口, token)`。
    fn spawn_server(probe: Arc<dyn lock::LockProbe>) -> (u16, String) {
        let cfg = Config::new(0, scratch_token());
        let bound = bind(&cfg).expect("绑定 127.0.0.1:0");
        let port = bound.port;
        let token = bound.token.clone();
        std::thread::spawn(move || {
            let _ = serve_on(bound, probe);
        });
        (port, token)
    }

    fn read_line(reader: &mut BufReader<TcpStream>) -> String {
        let mut line = String::new();
        reader.read_line(&mut line).expect("读一行");
        line
    }

    #[test]
    fn a_real_connection_authenticates_then_drives_a_session() {
        let (port, token) = spawn_server(Arc::new(NeverLocked));
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("连接");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("超时");
        let mut reader = BufReader::new(stream.try_clone().expect("克隆"));
        let mut writer = stream;

        let reply = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":1,"method":"auth","params":{"token": token}}),
        );
        assert_eq!(reply["result"]["ok"], json!(true), "握手：{reply}");

        let opened = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":2,"method":"session_open",
                    "params":{"command":"sh -c 'sleep 0.3; printf ready'"}}),
        );
        let session = opened["result"]["name"].as_str().expect("会话名");

        let waited = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":3,"method":"wait",
                    "params":{"session":session,"pattern":"ready","timeout_ms":10_000}}),
        );
        assert!(
            waited["result"]["text_upto"]
                .as_str()
                .unwrap_or("")
                .contains("ready"),
            "wait 应拿到 ready：{waited}"
        );

        let closed = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":4,"method":"session_close","params":{"session":session}}),
        );
        assert_eq!(closed["result"]["closed"], json!(true));
    }

    #[test]
    fn an_unauthenticated_connection_is_closed_after_one_reply() {
        let (port, _token) = spawn_server(Arc::new(NeverLocked));
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("连接");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("超时");
        let mut reader = BufReader::new(stream.try_clone().expect("克隆"));
        let mut writer = stream;

        let reply = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":1,"method":"session_list"}),
        );
        assert_eq!(reply["error"]["code"], json!(-32003));

        // 服务端应已收线：下一次读拿到 EOF。
        let mut rest = String::new();
        let _ = reader.read_to_string(&mut rest);
        assert!(rest.is_empty(), "拒绝之后不该再有字节：{rest:?}");
    }

    /// 流式四动词必须在**真连接 + 真会话**上验一次行为，而不只是验参数。
    ///
    /// 为什么单独一条：`read` / `read_until` / `wait_any` / `wait_idle` 在
    /// ADR-0023 里是「**消耗**输出的流式动词」，客户端不许把它们当
    /// 幂等读重试。这条把「同一个 tab 的两次 `read` 共用一个游标」钉死——
    /// 在此之前，远程侧这四个动词只有「空 delimiter 报 invalid_params」
    /// 「tab 99 报 not_found」两类断言，行为零覆盖。
    #[test]
    fn streaming_verbs_consume_output_once_on_a_real_connection() {
        let (port, token) = spawn_server(Arc::new(NeverLocked));
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("连接");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("超时");
        let mut reader = BufReader::new(stream.try_clone().expect("克隆"));
        let mut writer = stream;

        let reply = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":1,"method":"auth","params":{"token": token}}),
        );
        assert_eq!(reply["result"]["ok"], json!(true), "握手：{reply}");

        // 小写进、大写出：`tr` 真的在处理我们的字节，不是 tty 回显。
        let opened = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":2,"method":"session_open",
                    "params":{"command":"sleep 0.2; exec tr a-z A-Z"}}),
        );
        let session = opened["result"]["name"].as_str().expect("会话名");

        // ① read 是消耗的：同一段输出不许读两遍。
        request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":3,"method":"write",
                    "params":{"session":session,"text":"alpha\n"}}),
        );
        let waited = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":4,"method":"wait",
                    "params":{"session":session,"pattern":"ALPHA","timeout_ms":10_000}}),
        );
        assert!(
            waited["result"]["text_upto"]
                .as_str()
                .unwrap_or("")
                .contains("ALPHA"),
            "wait 该先命中：{waited}"
        );
        let first = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":5,"method":"read",
                    "params":{"session":session,"timeout_ms":1000}}),
        );
        let first_text = first["result"]["text"].as_str().expect("text 是字符串");
        assert!(first_text.contains("ALPHA"), "第一次 read 该拿到它：{first_text:?}");
        // 关键：只断言「不再包含 ALPHA」，而不是「必须是空串」——输出可能还在
        // 路上被切成两段，后者会偶发红；前者才是「游标被推进了」这件事本身。
        let second = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":6,"method":"read",
                    "params":{"session":session,"timeout_ms":200}}),
        );
        let second_text = second["result"]["text"].as_str().expect("text 是字符串");
        assert!(
            !second_text.contains("ALPHA"),
            "第二次 read 又拿到了同一段——游标没有推进：{second_text:?}"
        );

        // ② read_until 认哨兵，且接着同一个游标续读。
        request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":7,"method":"write",
                    "params":{"session":session,"text":"beta\n"}}),
        );
        let until = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":8,"method":"read_until",
                    "params":{"session":session,"delimiter":"BETA","timeout_ms":10_000}}),
        );
        let until_text = until["result"]["text"].as_str().expect("text 是字符串");
        assert!(until_text.contains("BETA"), "read_until 该拿到哨兵：{until_text:?}");
        assert!(
            !until_text.contains("ALPHA"),
            "read_until 不该把上一轮已消费的文本再吐一遍：{until_text:?}"
        );

        // ③ wait_any 给 1 起的 index。
        request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":9,"method":"write",
                    "params":{"session":session,"text":"gamma\n"}}),
        );
        let any = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":10,"method":"wait_any",
                    "params":{"session":session,"patterns":["NOPE","GAMMA"],"timeout_ms":10_000}}),
        );
        assert_eq!(any["result"]["index"], json!(2), "两个模式里第二个命中：{any}");
        assert!(
            any["result"]["text"]
                .as_str()
                .unwrap_or("")
                .contains("GAMMA"),
            "命中文本该带 GAMMA：{any}"
        );

        // ④ wait_idle 等「没事了」，不猜提示符长相。
        request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":11,"method":"write",
                    "params":{"session":session,"text":"delta\n"}}),
        );
        let idle = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":12,"method":"wait_idle",
                    "params":{"session":session,"quiet_ms":150,"timeout_ms":10_000}}),
        );
        assert!(
            idle["result"]["text"]
                .as_str()
                .unwrap_or("")
                .contains("DELTA"),
            "wait_idle 该把安静前的输出带回来：{idle}"
        );
        // quiet_ms 是必填：少给一个数就是替调用方决定「多安静算安静」。
        let missing = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":13,"method":"wait_idle","params":{"session":session}}),
        );
        assert_eq!(missing["error"]["kind"], json!("invalid_params"));

        // ⑤ screen 是只读的，能把四段输出全看到。
        let screen = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":14,"method":"screen","params":{"session":session}}),
        );
        let text = screen["result"]["text"].as_str().expect("text 是字符串");
        for needle in ["ALPHA", "BETA", "GAMMA", "DELTA"] {
            assert!(text.contains(needle), "屏幕上缺 {needle}：\n{text}");
        }
        // ADR-0023：远程不该有 cursor_pos，那两个数在 screen 的结果里。
        assert!(
            screen["result"]["cursor_row"].is_u64() && screen["result"]["cursor_col"].is_u64(),
            "screen 必须带光标位置（脚本专用 cursor_pos 的远程替代）：{screen}"
        );

        let closed = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":15,"method":"session_close","params":{"session":session}}),
        );
        assert_eq!(closed["result"]["closed"], json!(true));
    }

    /// `state` 的**退出分支**：活着时 `exit` 是 null，死了必须给退出原因。
    ///
    /// 在此之前只验过 `alive == true` / `exit == null` 那一半，于是「进程退出
    /// 之后 `exit` 里到底是什么」无人钉住——而这正是客户端据以判断「正常结束
    /// 还是崩了」的唯一事实。
    #[test]
    fn state_reports_the_exit_reason_after_the_process_ends() {
        let (port, token) = spawn_server(Arc::new(NeverLocked));
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("连接");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("超时");
        let mut reader = BufReader::new(stream.try_clone().expect("克隆"));
        let mut writer = stream;

        let reply = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":1,"method":"auth","params":{"token": token}}),
        );
        assert_eq!(reply["result"]["ok"], json!(true), "握手：{reply}");

        let opened = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":2,"method":"session_open",
                    "params":{"command":"exit 7"}}),
        );
        let session = opened["result"]["name"].as_str().expect("会话名");

        // 轮询到进程真的退出为止（不睡固定时长：快机器上立刻就好，
        // 慢机器上也不该假红）。
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut state = json!(null);
        loop {
            let reply = request(
                &mut writer,
                &mut reader,
                &json!({"jsonrpc":"2.0","id":3,"method":"state","params":{"session":session}}),
            );
            state = reply["result"].clone();
            if state["alive"] == json!(false) {
                break;
            }
            assert!(Instant::now() < deadline, "进程一直没退出：{state}");
            std::thread::sleep(Duration::from_millis(20));
        }
        // 文案由 `term-io` 的 PTY 通道定义（`exited` / `exited with code N`），
        // 这里钉的是**整条链**：进程退出码 → 监管者状态 → 会话池 → 能力面。
        assert_eq!(
            state["exit"],
            json!(" exited with code 7"),
            "退出原因必须如实报出退出码，而不是一个笼统的「已结束」：{state}"
        );

        // 死掉的会话仍在册：能关，且不是 not_found（它是「已结束」不是「不存在」）。
        let closed = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":4,"method":"session_close","params":{"session":session}}),
        );
        assert_eq!(closed["result"]["closed"], json!(true));
        let after = request(
            &mut writer,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":5,"method":"state","params":{"session":session}}),
        );
        assert_eq!(after["error"]["kind"], json!("not_found"));
    }

    #[test]
    fn a_locked_screen_refuses_the_connection_without_any_reply() {
        let (port, _token) = spawn_server(Arc::new(AlwaysLocked));
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("连接");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("超时");
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        assert!(
            buf.is_empty(),
            "锁屏时应直接关连接，一个字节都不回：{buf:?}"
        );
    }

    /// 一问一答，返回解析后的 JSON。
    fn request(writer: &mut TcpStream, reader: &mut BufReader<TcpStream>, value: &Value) -> Value {
        writer
            .write_all(format!("{value}\n").as_bytes())
            .expect("写请求");
        writer.flush().expect("冲刷");
        let line = read_line(reader);
        assert!(!line.is_empty(), "服务端没有回包（请求：{value}）");
        serde_json::from_str(&line).unwrap_or_else(|err| panic!("回包不是 JSON：{err} / {line}"))
    }
