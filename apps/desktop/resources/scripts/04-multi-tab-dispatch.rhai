// 多标签下发：开两条本地 shell，逐标签发命令（对应 CRT 的 GetTab/Screen.Send）。
// 运行：term-desk script run 04-multi-tab-dispatch.rhai --batch
// 改这里：把 session_open() 换成 session_open(#{profile: "你的会话"}) 或
//   #{config: #{host: ..., username: ..., auth: "agent"}} 就能下发到远端标签。

let first = session_open();
let second = session_open();

// CRT 的 GetTab 序号从 1 起；门面把每条标签钉死在自己的会话上，
// 所以这里不需要先 session_use，也不会因为当前绑定变化而发错标签。
let count = crt().get_tab_count();
for i in 1..=count {
    let tab = crt().get_tab(i);
    tab.screen.send("echo hello from tab " + i + "\r");
}

count
