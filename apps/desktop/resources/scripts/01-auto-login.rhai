// 自动登录（对应 CRT 的 /SSH2 … + WaitForString 脚本）。
// 运行：在脚本窗口打开本文件，登记项里配好参数 <host> <user> 后点「运行」。
// 改这里：认证方式默认 agent（无需保险库）；若你的 SSH 用密码，
//   请先在窗口里解锁保险库，或改用会话库中的会话：session_open(#{profile: "名字"})。
// 口令提示出现时才向人询问，不把口令写进脚本文件。

let host = args(0);
let user = args(1);

let info = session_open(#{config: #{
    name: "auto-login",
    host: host,
    username: user,
    protocol: "ssh",
    auth: "agent",
    builtin_ssh: true
}});
session_use(info["name"]);

// 命中返回到提示符为止（含提示符）；超时显式失败，不会假装登录成功。
let hit = wait_any(["Password:", "password:", "$ ", "# "], 20);
// wait_any 的 index 是 1 起：Password: = 1、password: = 2、提示符 = 3/4。
if hit["index"] == 1 || hit["index"] == 2 {
    write(secret("远端口令") + "\r");
    wait_any(["$ ", "# "], 20);
}

write("uname -a\r");
let out = read_until("$ ", 10);
out
