// 多标签下发：开两条本地 shell，逐会话发命令（借鉴 CRT 的多标签下发节奏）。
// 运行：在脚本窗口打开本文件点「运行」。
// 改这里：把 session_open() 换成 session_open(#{profile: "你的会话"}) 或
//   #{config: #{host: ..., username: ..., auth: "agent"}} 就能下发到远端标签。

let first = session_open();
let second = session_open();

// 会话名是稳定寻址键（ADR-0024）：按名字切换绑定，写动词就落到那条标签。
// 用 session_use 而非「全局当前标签」，避免脚本开新会话后发错目标。
let rows = session_list()["sessions"];
for row in rows {
    let name = row["name"].to_string();
    session_use(name);
    write("echo hello from " + name + "\r");
}

rows.len()
