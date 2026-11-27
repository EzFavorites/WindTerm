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

其余常用参数：`--batch`（批处理预算档）、`--arg X`（可重复，脚本用 `args()` 读）、
`--allow fs=write:/tmp,process`（本次运行的额外授权）。完整列表见
`apps/desktop/src/cli/script.rs` 的 `USAGE`，无参数打印的用法错误里也会带上它。

窗口内的脚本编辑器使用同一套宿主 API，因此同一份脚本在 GUI 与 CLI 下行为
一致（可用的宿主能力随上下文不同而收敛）。

## 宿主 API

脚本动词与 JSON-RPC 桥共用**同一张能力描述表**（`term-core` 的 `surface`）：
一个动词只有一处定义，脚本侧与远程侧是它的两个投影。完整清单与参数结构以
`term_core::surface::METHODS`（脚本侧用脚本原语 `describe()`，远程侧用 RPC `describe`）
为准；`term-desk capability --json` 打印的是**菜单命令台账**（总数 / 已接线 / 置灰），
不是这张动词表。下面是按类别组织的概览。

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
| 安全 | `secret` | 弹掩码输入框向用户要口令（不是保险库查询；无窗口宿主不支持） |
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
`grep` / `grep_v`（标准正则的行级过滤）、`crt` 兼容门面。门面里
`crt().get_tab(n).index()` 是标签对象的方法，不是全局函数。

### `crt` 兼容门面与真实拼写

门面提供 SecureCRT 的**对象形状**，但注册名是 Rhai 的 snake_case：入口是函数调用
`crt()`，后面接小写属性与方法——`crt().screen.send(...)` 转发到 `write`、
`crt().screen.wait_for_string(...)` 转发到 `read_until`（消费式）、
`crt().screen.get(...)` 转发到 `region`。对象上的 PascalCase 别名也已注册
（`crt().Screen.Send(...)`、`crt().GetTab(1)` 可用），所以照 CRT 手册抄下来的脚本
大多能直接跑；注意入口必须写成函数调用 `crt()`，不是属性 `crt.`。下面的速查表
只是风格参考，不是必改清单。门面只提供形状，不提供 VBScript / Python 解释器。

| SecureCRT 写法 | WindTerm / Rhai 写法 | 说明 |
|---|---|---|
| `#$language = "VBScript"` / `#$language = "Python"` 两行头 | 无 | 只有 Rhai；授权另用单行 `// @cap ...` 头 |
| `Sub Main() ... End Sub` / `def main():` | 无 `main()`，顶层直接写语句 | 最后一条不带分号的表达式是脚本返回值 |
| `/SSH2 ...`（Open 会话配置行） | `session_open(#{config: #{host: "...", username: "...", auth: "agent"}})` | 也可用 `session_open(#{profile: "会话名"})` 开库里的会话 |
| `crt.Arguments` / `crt.Arguments(0)` | `args()` / `args(0)` | 0 起，越界给空串；门面对应 `crt().arguments()` / `crt().argument(i)` |
| `crt.GetLastErrorMessage()` | `last_error()` | 门面对应 `crt().last_error()` |
| `crt.Screen.WaitForString(p, secs)` | `crt().screen.wait_for_string(p, secs)` | 命中返回**到分隔符为止（含分隔符）**的文本并推进读游标；超时是显式错误，不返回「假装成功」的哨兵 |

全局没有 `send` / `wait_for_string` 这类裸函数：要么用全局动词 `write` /
`read_until`，要么走 `crt().screen.*` 门面。

## 执行环境

- 脚本在受限预算内运行：墙钟、操作数和语法嵌套深度都有上限
  （`crates/term-script/src/sandbox.rs`；交互档 30 秒 / 100 万操作，
  `--batch` 批处理档 600 秒 / 5000 万操作），超预算即显式失败。
  **没有输出字节配额这个旋钮**：输出侧的边界只有「每次取数只返回上次消费点
  之后的文本」与固定 64 KiB 捕获窗口（超出保尾裁旧）；长输出要循环取数并当场
  消费，不要指望一次 `read` 端回几 MB。
- `secret(prompt)` 弹一个掩码输入框，把用户输入**原文**作为字符串返回给脚本
  （不是保险库句柄，也不从配置里读）。无窗口宿主不支持，会显式失败；
  脚本日志与错误信息不回显口令。
- 脚本错误写进 `last_error`，不会让宿主进程崩溃。

## 示例

```rhai
// @cap fs=write:/tmp
// 开一个本地 shell、发一条命令、等提示符、把结果写进文件
let info = session_open();          // 返回会话行；宿主按会话配置定名
session_use(info["name"]);          // 绑定到这个名字
write("uname -a\r");                // 写动词是 write，不是 send；回车用 CR
let out = read_until("$ ", 10);     // 流式：吃到分隔符为止（秒）
fs_write("/tmp/uname.txt", out);
session_close(info["name"]);
```

随产品附带的 5 个可直接改的示例在
[`apps/desktop/resources/scripts/`](../apps/desktop/resources/scripts/)：
自动登录、批量巡检落盘、抓命令输出、多标签下发、自动应答。每个文件头部都写了
怎么运行、要改哪几处占位值，以及是否需要 `// @cap` 授权。

```
term-desk script run apps/desktop/resources/scripts/03-capture-output.rhai --batch
```

等一个不确定的提示语时用 `wait`（不推进游标，返回已匹配到的文本）或 `wait_any`
（多模式取首个命中，返回 `{index, text}`，`index` 是模式在数组里的 **1 起下标**）：

```rhai
// 超时直接报错（写进 last_error），不会返回一个「假装成功」的哨兵值
let hit = wait_any(["Password:", "password:", "密码"], 15);
// hit["index"] = 命中的模式下标（1 起，0 不会出现）；hit["text"] = 命中为止的文本
write("<口令>\n");
```

函数签名可能随版本演进；以 `describe()` / RPC `describe` 的实际输出与
`crates/term-script/src/*.rs` 的注册表为准。

## 录制脚本

录制器录的是**人敲进去的字节 + 屏幕什么时候安静下来**，不是「调了哪些原语」。
生成物与手写脚本同形（`write` 一段 → `read_until` 一个提示符 → 再 `write`），
所以能直接看懂、改路径就能复用。

- **入口**：`Tool ▸ Script ▸ Record Script`，或脚本面板工具行的 `Record` 按钮；
  两者是同一个开关，菜单项的勾读的就是录制器当前状态。
- **静默判定**：输入和屏幕最后一次变化之后 **400 ms** 算一段结束；每段落一条
  `write(...)` 加一条 `read_until(锚点, 10)`。屏幕还在刷时不落步。
- **只录人的输入**：设备应答（DSR / DA）、粘贴、本地只读会话的字节都不进录制。
- **产物**：停止时写到脚本库（`profile_dir()/scripts/`），文件名依次
  `recorded.rhai`、`recorded-2.rhai`…；状态栏会给出落地路径。空录制不留文件。
- **回放**：生成物就是普通 `.rhai`，从运行窗口 / 面板 / 命令面板跑都行。
  录制中的状态**不持久化**，退出应用会丢掉还没停的那一次。

实现位于 `apps/desktop/src/script_recorder.rs`；它的单测会把录出的源码丢回真实
引擎跑一遍，确保生成物不是只能看的字符串。

## 命令行的边界

`term-desk script run` 跑在独立进程里，**不共享**桌面窗口的会话池：

- 它自己开、自己关会话（`--exec` 一次性命令 / `--session` 会话库引用 / 默认本地 shell）。
- 无窗口进程不会解锁保险库，所以密码与 keyboard-interactive 认证的会话在这里
  打不开（显式报错、退出码 1）；改用 Agent / 私钥，或到桌面里跑。
- 脚本来自 stdin（`-`）时 `ask()` / `secret()` 不可用——输入源已被脚本占用。

## 与 CRT 的有意偏差

这些不是「还没做」，而是刻意不照搬；照 CRT 手册写的脚本要按上表改写。

| 偏差 | 现在的事实 |
|---|---|
| 无多语言解释器 | 只有 Rhai。门面提供 CRT 的**对象形状**，不解释 VBScript / Python，也不执行它们的语句 |
| 无两行头 | `#$language = ...` 不解析；脚本头只有可选的 `// @cap ...`（授权，非语言选择） |
| 超时显式失败 | `wait` / `wait_any` / `read_until` / `crt().screen.wait_for_string` 超时即报错，不返回哨兵值；原因可从 `last_error()` 读 |
| 部分 API 需 caps 授权 | 文件、外部进程、剪贴板读、窗口与应用级状态默认全拒；用脚本头 `// @cap fs=write:/tmp,process` 或 CLI `--allow ...` 放行，只作用于本次运行 |
| 无自定义对话框 / OLE | 不支持 CRT 的自定义窗体与 OLE 自动化；`crt().dialog` 只有 `message_box` / `prompt` / `file_open_dialog` / `file_save_dialog` 四种固定形态 |

## JSON-RPC 桥

`crates/term-bridge` 提供 JSON-RPC over stdio 的桥接，供外部工具以结构化方式
调用能力面。`term-desk serve` 把同一套能力暴露到 `127.0.0.1`：连接后的第一条
请求必须是 token 握手，之后按 JSON-RPC 调用。

token 从 `--token-file` 指定的文件读取；`serve` 不会在没有 token 的情况下
监听端口。