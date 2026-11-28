// @cap fs=write:/tmp
// 批量巡检落盘：对多台主机跑同一组命令，结果追加到一份本地报告。
// 运行：在脚本窗口打开本文件，登记项里配好参数 host1 host2 host3，预算档选
//   Batch 后点「运行」。
// 改这里：user / commands / report；换输出目录时，上面的 @cap 根也要一起改。
// 认证用 agent；密码认证的会话要先在前台窗口里解锁保险库。

let report = "/tmp/windterm-inspection.txt";
let user = "ops";
let commands = ["uptime", "df -h"];
let hosts = args();

fs_write(report, "=== 巡检报告 ===\n");

for host in hosts {
    let info = session_open(#{config: #{
        name: host,
        host: host,
        username: user,
        protocol: "ssh",
        auth: "agent",
        builtin_ssh: true
    }});
    session_use(info["name"]);

    // 首个命中决定要不要答口令；index 是 1 起：密码提示是 3 / 4；wait_any 超时即显式失败。
    let login = wait_any(["$ ", "# ", "Password:", "password:"], 20);
    if login["index"] == 3 || login["index"] == 4 {
        write(secret("远端口令") + "\r");
        wait_any(["$ ", "# "], 20);
    }

    fs_append(report, "=== " + host + " ===\n");
    for command in commands {
        write(command + "\r");
        // 长输出当场落盘：捕获窗口只有 64 KiB，不要指望一次读回几 MB。
        let out = read_until("$ ", 10);
        fs_append(report, "$ " + command + "\n" + out + "\n");
    }
    session_close(info["name"]);
}

report
