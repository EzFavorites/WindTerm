# 脚本

WindTerm 内建 [Rhai](https://rhai.rs) 脚本引擎（`crates/term-script`），可以在
独立会话里驱动终端、操作会话、读写本地文件、跑外部进程。

## 入口

```sh
term-desk script run a.rhai              # 文件
term-desk script run - < a.rhai          # 标准输入
term-desk script run a.rhai --exec "ls"  # 用一次性命令会话代替本地 shell
term-desk script run a.rhai --timeout 30 --quiet
```

窗口内的脚本编辑器使用同一套宿主 API，因此同一份脚本在 GUI 与 CLI 下行为
一致（可用的宿主能力随上下文不同而收敛）。

## 宿主 API

能力按模块分组注册。函数清单以 `term-desk capability --json` 的实际输出为准，
下面是分组概览：

| 分组 | 代表函数 | 用途 |
|---|---|---|
| 会话 | `session_open` `session_connect` `session_close` `session_list` `session_use` `session_activate` `session_info` `session_at` `session_clone` | 创建、切换、查询会话 |
| 收发 | `send` `send_keys` `read` `read_until` `wait_for_string` `wait_for_strings` `wait_idle` `wait_any` `set_synchronous` | 向会话写数据、等输出 |
| 屏幕 | `screen` `selection` `cursor_pos` `current_row` `current_column` `rows` `columns` | 读当前屏幕与选区 |
| 过滤 | `grep` `grep_v` `index` `crt` `facets` | 对输出做行过滤与匹配 |
| 本地文件 | `fs_read` `fs_write` `fs_append` `fs_list` `fs_stat` `fs_mkdir` `fs_remove` `pick_file` | 本地文件系统 |
| 远端文件 | `sftp_list` `sftp_get` `sftp_mkdir` … | SFTP 操作 |
| 进程 | `proc_run` | 跑外部命令并取输出 |
| 剪贴板 | `clipboard_get` `clipboard_set` | 读写剪贴板 |
| 日志 | `log_start` `log_status` `log_stop` | 会话日志 |
| 偏好 | `profile_get` `profile_set` `profile_list` | 读写用户偏好 |
| 安全 | `secret` | 取保险库里的凭据（不落日志） |
| 交互 | `ask` `confirm` `notify` `message` | 向用户提问、确认、提示 |
| 应用 | `app_state` `app_quit` | 查询/控制应用状态 |
| 运行时 | `args` `script_path` `script_session` `last_error` `keys` | 脚本自身上下文 |

## 执行环境

- 脚本在受限预算内运行：执行时间、输出量、循环次数都有上限
  （`crates/term-script/src/sandbox.rs`），超预算即中止并报错。
- `secret` 只返回凭据的可用句柄语义，不把明文写进脚本变量以外的地方；
  脚本日志与错误信息不回显口令。
- 脚本错误写进 `last_error`，不会让宿主进程崩溃。

## 示例

```rhai
// 打开一个会话、发一条命令、等提示符、把结果写进文件
let s = session_open("demo");
session_use(s);
send("uname -a\n");
let out = read_until("$", 10);
fs_write("/tmp/uname.txt", out);
```

函数签名可能随版本演进；以 `term-desk capability --json` 与
`crates/term-script/src/*.rs` 的注册表为准。

## JSON-RPC 桥

`crates/term-bridge` 提供 JSON-RPC over stdio 的桥接，供外部工具以结构化方式
调用能力面。`term-desk serve` 把同一套能力暴露到 `127.0.0.1`：连接后的第一条
请求必须是 token 握手，之后按 JSON-RPC 调用。

token 从 `--token-file` 指定的文件读取；`serve` 不会在没有 token 的情况下
监听端口。