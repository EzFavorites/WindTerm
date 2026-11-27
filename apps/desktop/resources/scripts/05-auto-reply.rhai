// 自动应答：按首个命中的提示词回话（对应 CRT 的 WaitForStrings/MatchIndex）。
// 运行：term-desk script run 05-auto-reply.rhai --batch
// 改这里：prompts 与 replies 一一对应；期望三轮交互，轮数不对就显式失败，
// 不静默跳过——否则「以为自动应答了」比直接报错更难查。

let prompts = ["Password:", "yes/no", "Continue?"];

for n in 0..3 {
    let hit = wait_any(prompts, 20);
    let kind = hit["index"];
    // wait_any 的 index 是 1 起：Password: = 1，其余是 2/3。
    if kind == 1 {
        write(secret("远端口令") + "\r");
    } else {
        write("y\r");
    }
}

"自动应答完成"
