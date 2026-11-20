//! ProxyJump 的真实链路（Docker）：两台容器，目标机只能由跳板机连到。
//!
//! 这里跑的是 `SessionConfig::spawn_plan` 生成的**系统 ssh argv**——也就是用户
//! 选「用系统 ssh」时真正会执行的那串参数。所以它同时验两件事：
//!   1. `-J` / `-i` / `-l` 的拼法确实被 OpenSSH 接受；
//!   2. 跳板机能解析 `target`，链路真的通了（不是「参数看起来对」）。
//!
//! 为什么客户端跑在容器里而不是宿主机上：`ssh -J` 会再起一条
//! `ssh -W %h:%p` 作为 ProxyCommand，它**不继承**外层命令的 `-o` 选项。
//! 于是「首次信任这台跳板机」没法从外层传进去，而往宿主机 `~/.ssh/known_hosts`
//! 里塞测试密钥同样不行（那是在改用户的家目录）。容器里的 `HOME` 是一次性的，
//! 既隔离又能随便写。
//!
//! 因此这条测试验证的是「argv 正确 + 链路可通」；主机密钥的信任策略由
//! `crates/term-ssh/tests/docker_sshd.rs` 的 TOFU 用例覆盖（那条走我们自己的
//! 客户端，不经过系统 ssh）。

use std::process::Command;

use term_core::session::SessionConfig;

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("缺少环境变量 {name}：请用 scripts/docker-verify.sh 起容器后运行本测试")
    })
}

/// `docker compose run --rm --no-deps --entrypoint <entry> client <args…>`
fn run_in_client(entry: &str, args: &[String]) -> std::process::Output {
    let compose = var("WINDTERM_DOCKER_COMPOSE");
    let mut cmd = Command::new("docker");
    cmd.args([
        "compose",
        "-f",
        &compose,
        "run",
        "--rm",
        "--no-deps",
        "--entrypoint",
        entry,
        "client",
    ]);
    cmd.args(args);
    cmd.output().expect("docker compose run 起不来")
}

#[test]
#[ignore = "需要 Docker 里的两台容器：bash scripts/docker-verify.sh proxyjump"]
fn spawn_plan_proxy_jump_reaches_the_target_through_the_bastion() {
    let user = var("WINDTERM_DOCKER_USER");

    // 容器内的主机名就是 compose 服务名；`-J tester@bastion` 走默认 22 端口。
    let mut config = SessionConfig::new(term_core::session::Protocol::Ssh);
    config.host = "target".to_string();
    config.port = 22;
    config.username = user.clone();
    config.auth = term_core::session::AuthMethod::PublicKey;
    config.proxy_jump = format!("{user}@bastion");
    config.identity_file = "/keys/id_ed25519".to_string();
    config.keep_alive = 0;

    let argv = match config.spawn_plan().expect("应生成外部命令") {
        term_core::session::SpawnPlan::External(argv) => argv,
        _ => panic!("SSH 会话应当生成外部命令"),
    };
    let joined = argv.join(" ");
    assert!(
        joined.contains(&format!("-J {user}@bastion")),
        "argv 里没有 -J：{joined}"
    );
    assert!(joined.contains("-i /keys/id_ed25519"), "{joined}");
    assert_eq!(argv.last().map(String::as_str), Some("target"), "主机名应在最后：{joined}");

    // 为什么要写 `~/.ssh/config`：`-i` 只作用于**最终那一跳**。ProxyJump 会另起
    // `ssh -W` 连跳板机，那条子命令不继承外层的 `-i`（这是 OpenSSH 的行为，
    // 不是我们的 argv 少写了什么）。所以跳板那一跳的私钥得来自 config / agent /
    // 默认路径——这里用 config 把它显式指过去。
    //
    // 铺 known_hosts 与跑 ssh 必须在**同一个容器**里：`compose run` 每次都新建
    // 一个一次性容器，分两次跑的话第二次看不到第一次写的 /root/.ssh。
    // 做法：先用 `bash -lc` 铺账本，再用 `exec "$@"` 把原始 argv 原样交给 ssh
    // （不拼字符串，避免主机名 / 路径里的空格被 shell 解释）。
    let mut args: Vec<String> = vec![
        "-lc".to_string(),
        "mkdir -p /root/.ssh \
         && ssh-keyscan -p 22 bastion target 2>/dev/null > /root/.ssh/known_hosts \
         && chmod 600 /root/.ssh/known_hosts \
         && printf 'Host *\\n  IdentityFile /keys/id_ed25519\\n  IdentitiesOnly yes\\n' \
              > /root/.ssh/config \
         && chmod 600 /root/.ssh/config \
         && exec \"$@\""
            .to_string(),
        "bash".to_string(),
    ];
    args.extend(argv.iter().cloned());
    // ssh 把「主机名之后」的参数拼成远端命令，所以远端命令接在最后。
    args.push("echo".to_string());
    args.push("PROXY-JUMP-OK".to_string());
    let out = run_in_client("bash", &args);

    assert!(
        out.status.success(),
        "按 spawn_plan 的 argv 连不上：{}\nargv: {joined}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("PROXY-JUMP-OK"),
        "链路通了但命令没执行：{stdout:?}"
    );
}
