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

脚本动词与 JSON-RPC 桥共用**同一张能力描述表**（`term-core` 的 `surface`）：
一个动词只有一处定义，脚本侧与远程侧是它的两个投影。完整清单与参数结构以
`term-desk capability --json` 的实际输出为准，下面是按类别组织的概览。

**读**（只取事实，不改变会话状态）：

| 分组 | 函数 | 用途 |
|---|---|---|
| 会话 | `session_list` `session_current` `session_info` `session_at` `session_index` `session_active` | 列举、定位、查询会话 |
| 屏幕 | `screen` `state` `selection` `cursor_pos` `terminal_size` `region` `region_lines` | 读屏幕、选区、光标与任意矩形 |
| 等待 | `wait` | 等某个模式出现（不推进游标） |
| 日志 | `log_status` | 查询日志状态 |
| 过滤 | `facets` | 读取可用的行级过滤器 |
| 本地文件 | `fs_read` `fs_list` `fs_stat` `pick_file` | 本地文件系统 |
| 远端文件 | `sftp_list` `sftp_stat` | SFTP 读取 |
| 偏好 | `profile_get` `profile_list` | 读用户偏好 |
| 安全 | `secret` | 取保险库里的凭据（不落日志） |
| 交互 | `ask` `confirm` `message` `choose` `text_input` `pick_file` | 向用户提问、确认、选择 |
| 剪贴板 | `clipboard_get` | 读剪贴板 |
| 运行时 | `args` `version` `script_path` `script_session` `last_error` `sleep` | 脚本自身上下文 |
| 应用 | `app_state` | 查询应用状态 |

**写**（改变会话或应用状态）：

| 分组 | 函数 | 用途 |
|---|---|---|
| 收发 | `write` `keys` `send_keys` `set_synchronous` | 写文本、按键名发按键 |
| 日志 | `log_start` `log_stop` | 起停会话日志 |
| 本地文件 | `fs_write` `fs_append` `fs_mkdir` `fs_remove` | 改本地文件系统 |
| 远端文件 | `sftp_get` `sftp_put` `sftp_mkdir` `sftp_rename` `sftp_remove` | SFTP 操作 |
| 进程 | `proc_run` | 跑外部命令并取输出 |
| 偏好 | `profile_set` | 写用户偏好 |
| 剪贴板 | `clipboard_set` | 写剪贴板 |
| 交互 | `notify` | 弹提示 |
| 应用 | `app_quit` `window` `app_panel_set` `app_layout` | 关应用、控制窗口与面板布局 |

**会话生命周期**（改动控制面的登记表）：`session_open` `session_close` `session_use`
`session_connect` `session_clone`。

**流**（推进捕获游标，重复调用会吃掉上次没读完的数据，不可当幂等读重试）：
`read` `read_until` `wait_any` `wait_idle`。

> `session_open` 返回的是一个**会话行**（含 `name` / `alive` / `exit` / `phase` /
> `local_addr` 字段），不是句柄；要绑定会话请用行里的 `name` 调 `session_use`。
> 重名时宿主会加后缀，返回值里带的是真名。

脚本另有几个**脚本专属**原语，不在能力面上（因此不出现于 RPC 动词表）：
`grep` / `grep_v`（标准正则的行级过滤）、`index`、以及 `crt` 兼容门面。

`crt` 门面提供的是 SecureCRT 的**对象形状**，转发到本仓动词：`crt.Screen.Send(...)`
转发到 `write`、`crt.Screen.WaitForString(...)` 转发到 `read_until`（消费式）、
`crt.Screen.Get(...)` 转发到 `region`。它们是**对象方法**，只能在 `crt.Screen`
上调用——全局没有 `send` 或 `wait_for_string`。门面只提供形状，不提供 VBScript /
Python 解释器。

## 执行环境

- 脚本在受限预算内运行：执行时间、输出量、循环次数都有上限
  （`crates/term-script/src/sandbox.rs`），超预算即中止并报错。
- `secret` 只返回凭据的可用句柄语义，不把明文写进脚本变量以外的地方；
  脚本日志与错误信息不回显口令。
- 脚本错误写进 `last_error`，不会让宿主进程崩溃。

## 示例

```rhai
// 开一个本地 shell、发一条命令、等提示符、把结果写进文件
let info = session_open();          // 返回会话行；宿主按会话配置定名
session_use(info["name"]);          // 绑定到这个名字
write("uname -a\n");                // 写动词是 write，不是 send
let out = read_until("$ ", 10);     // 流式：吃到分隔符为止（秒）
fs_write("/tmp/uname.txt", out);
session_close(info["name"]);
```

等一个不确定的提示语时用 `wait`（不推进游标，返回已匹配到的文本）或 `wait_any`
（多模式取首个命中，返回 `{index, text}`，`index` 是模式在数组里的 0 起下标）：

```rhai
// 超时直接报错（写进 last_error），不会返回一个「假装成功」的哨兵值
let hit = wait_any(["Password:", "password:", "密码"], 15);
// hit["index"] = 命中的模式下标（0 起）；hit["text"] = 命中为止的文本
write("<口令>\n");
```

函数签名可能随版本演进；以 `term-desk capability --json` 与
`crates/term-script/src/*.rs` 的注册表为准。

## JSON-RPC 桥

`crates/term-bridge` 提供 JSON-RPC over stdio 的桥接，供外部工具以结构化方式
调用能力面。`term-desk serve` 把同一套能力暴露到 `127.0.0.1`：连接后的第一条
请求必须是 token 握手，之后按 JSON-RPC 调用。

token 从 `--token-file` 指定的文件读取；`serve` 不会在没有 token 的情况下
监听端口。