// 新脚本模板：把下面三处改成你的自动化即可。
// API 速查见 docs/scripting.md。入口是顶层语句，没有 main()。
// 要写本地文件时，在文件最前面加一行：// @cap fs=write:/tmp
// （授权只对本次运行有效；路径要和这行声明的根一致）。

let marker = "$ ";        // 改成你的命令提示符
let command = "uname -a"; // 改成你要执行的命令

write(command + "\r");
// 等待分两类：read_until 消费文本并返回到分隔符为止的内容；wait 只观察、不推进游标。
let out = read_until(marker, 15);

// 最后一条不带分号的表达式就是脚本返回值（CLI 会打印，GUI 显示在结果栏）。
out
