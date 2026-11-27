// @cap fs=write:/tmp
// 抓命令输出（CRT 里是 Screen.Send + 读输出 + 写文件；本仓用 send + 全局 read_until）。
// 运行：term-desk script run 03-capture-output.rhai --batch
// 改这里：marker 必须是你的提示符；command 与 path 换成你要抓的内容。

let marker = "$ ";
let command = "uname -a";
let path = "/tmp/command-output.txt";

crt().screen.send(command + "\r");
// read_until 是消费式的：命中返回到分隔符（含）为止的文本，超时显式失败。
// 门面的 wait_for_string 只返回布尔（true/false），拿不到文本，别用它来抓输出。
let out = read_until(marker, 15);
fs_write(path, out);

out
