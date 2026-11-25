// @cap fs=write:/tmp
// 抓命令输出（对应 CRT 的 Screen.Send + WaitForString + 写文件）。
// 运行：term-desk script run 03-capture-output.rhai --batch
// 改这里：marker 必须是你的提示符；command 与 path 换成你要抓的内容。

let marker = "$ ";
let command = "uname -a";
let path = "/tmp/command-output.txt";

crt().screen.send(command + "\r");
// wait_for_string 是消费式的：命中返回到分隔符（含）为止，超时显式失败。
let out = crt().screen.wait_for_string(marker, 15);
fs_write(path, out);

out
