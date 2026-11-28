// @cap fs=write:/tmp
// 抓命令输出（借鉴 CRT「发一段、等提示符、再发一段」的节奏，用原生动词实现）。
// 运行：term-desk script run 03-capture-output.rhai --batch
// 改这里：marker 必须是你的提示符；command 与 path 换成你要抓的内容。

let marker = "$ ";
let command = "uname -a";
let path = "/tmp/command-output.txt";

write(command + "\r");
// read_until 是消费式的：命中返回到分隔符（含）为止的文本，超时显式失败。
let out = read_until(marker, 15);
fs_write(path, out);

out
