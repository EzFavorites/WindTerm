# 脚本

WindTerm 内建 [Rhai](https://rhai.rs) 脚本引擎（`crates/term-script`），可以在
前台会话里驱动终端、操作会话、读写本地文件、跑外部进程。

## 入口

脚本只在**前台 GUI 进程**里跑：脚本窗口选文件点「运行」，或在脚本编辑器里
打开后运行。脚本 `session_open` 出来的标签就在用户眼前，与手动打开的会话
属于同一个池。脚本用 `args(0)` 读的参数来自脚本登记项；预算档位与本次授权
（`// @cap` 声明、脚本面板 ▸ Permissions）都在界面里选，不靠命令行开关。

命令行**不执行脚本文件**：`term-desk script run` 已删除（ADR-0030）。剩下的
无窗口子命令只有 `doctor` / `capability` / `api describe` / `api call` / `mcp`，
它们都是前台 TCP loopback 控制端点的薄客户端，要求前台已经在运行，绝不自建会话池。

脚本窗口与脚本编辑器使用同一套宿主 API，因此同一份脚本在两处行为一致
（可用的宿主能力随上下文不同而收敛）。

## 外部 Python 脚本

`.py` 文件走**外部 Python 子进程 + stdio JSON-RPC 桥**，不是内嵌 Rhai，同样
从界面里运行。起进程必须放行 `process`：

```sh
WINDTERM_PYTHON=/opt/python3.12/bin/python3        # 指定解释器（可选）
# @cap process                                       # 写在 .py 文件头（或面板勾选）
```

借鉴 CRT 的思路——写 Python、在会话里发命令等提示符——但机制不同：别家在进程内
加载 Python DLL，我们把 Python 放在子进程里，靠一行一个 JSON 的桥接通信。好处是
崩溃、GIL、第三方库都关在进程外；代价是每次 API 调用是一次 IPC。

- **解释器**：默认找 PATH 上的 `python3` / `python`（Windows 上 `python` 优先），
  可用 `WINDTERM_PYTHON` 指定绝对路径。
- **授权**：起进程属于高风险能力，默认拒绝。脚本头写 `# @cap process`，或在
  脚本面板勾选「运行程序」；只作用于本次运行。
- **原生 API**：`crates/term-bridge::python_api_shim()` 从能力表派生出与动词同名的
  Python 一等函数：`write(text)`、`read_until(pattern, timeout)`、`wait(...)`、
  `wait_any(...)`、`screen()`、`session_list()`、`session_open(...)`、`session_use(name)`、
  `log_start(dir)` 等。函数名与参数全部来自 `rpc_specs()` 的 schema，不会和桥接漂移。
  返回值就是 RPC 结果对象（例如 `read_until(...)` 给 `{"text": ...}`）。
- **上下文**：启动会话由 `ZTERM_SESSION` 注入并作为各函数的默认 `session` 参数；
  `args()` / `script_path()` / `version()` / `last_error()` 是本地小助手，不走 RPC。
- **不支持**：剪贴板、文件选择框等需要界面队列的能力在外部 Python 里显式失败，
  不会返回空对象冒充成功。
- **输出**：用户的 `print()` 被改道到 stderr（stdout 是 RPC 管道）；失败时 stderr
  的异常栈会带进结果信息。
- **取消与预算**：点「停止」或墙钟到点会 kill 并回收 Python 子进程，与 Rhai 脚本
  共用同一套 `ScriptRun` 句柄。

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
| 安全 | `secret` | 弹掩码输入框向用户要口令（不是保险库查询；非交互宿主不支持） |
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
`grep` / `grep_v`（标准正则的行级过滤）。

### 从 CRT 借鉴的设计，以及刻意不照搬的地方

我们借鉴的是那套终端自动化的**节奏与语义**，不是对象形状：录「人敲的字节 +
屏幕静默点」生成「发一段 → 等提示符 → 再发一段」的脚本；区分「消费式读取」
与「只观察不推进游标」；按会话名寻址并钉住目标；同一个脚本能从菜单、映射键、
按钮栏、会话登录钩子和自动化规则（`RunScript` 动作）多种入口启动。实现上全部
落到 WindTerm 自己的全局动词，没有对象门面，也不解释别家脚本语言。

| 借鉴的设计点 | WindTerm 的原生写法 |
|---|---|
| 两行语言头 | 无；只有 Rhai，脚本头用单行 `// @cap ...` 声明本次授权 |
| `Sub Main()` / `def main():` | 无 `main()`，顶层直接写语句；最后一条不带分号的表达式是返回值 |
| 打开会话的配置行 | `session_open(#{config: #{host: "...", username: "...", auth: "agent"}})` 或 `session_open(#{profile: "会话名"})` |
| 启动参数 | `args()` / `args(0)`，0 起，越界给空串 |
| 等一个模式并取走输出 | `read_until(pattern, secs)`：命中返回到分隔符（含）为止的文本并推进游标；超时显式报错 |
| 只等模式、不取走输出 | `wait(pattern, secs)`：返回已匹配文本，游标不动 |
| 多模式取首个命中 | `wait_any([...], secs)`：返回 `{index, text}`，`index` 是 **1 起**下标 |
| 取屏幕矩形 | `region(row1, col1, row2, col2)`，行列 1 起、含端点 |
| 问用户 / 选文件 | `ask(...)` / `confirm` / `message` / `text_input` / `secret` / `pick_file` |
| 多按钮消息框 | `ask(#{kind:"message", buttons:"abort_retry_ignore"})`，还支持 `retry_cancel` / `yes_no_cancel`；宿主回传具体按钮再映射成 ID |

**刻意不同**：只支持 Rhai 一种语言，不解析别家的两行头；等待超时是显式错误，
不返回「假装成功」的哨兵；危险能力按运行授权，不全局放行；不做对象门面，
所以照别家手册抄来的对象链不能直接跑——按上表改成动词即可。

## 执行环境

- 脚本在受限预算内运行：墙钟、操作数和语法嵌套深度都有上限
  （`crates/term-script/src/sandbox.rs`；交互档 30 秒 / 100 万操作，
  批处理档 600 秒 / 5000 万操作），超预算即显式失败。
  **没有输出字节配额这个旋钮**：输出侧的边界只有「每次取数只返回上次消费点
  之后的文本」与固定 64 KiB 捕获窗口（超出保尾裁旧）；长输出要循环取数并当场
  消费，不要指望一次 `read` 端回几 MB。
- `secret(prompt)` 弹一个掩码输入框，把用户输入**原文**作为字符串返回给脚本
  （不是保险库句柄，也不从配置里读）。不能提问的宿主不支持，会显式失败；
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
要改哪几处占位值，以及是否需要 `// @cap` 授权；在脚本窗口打开后点「运行」即可。

等一个不确定的提示语时用 `wait`（不推进游标，返回已匹配到的文本）或 `wait_any`
（多模式取首个命中，返回 `{index, text}`，`index` 是模式在数组里的 **1 起下标**）：

```rhai
// 超时直接报错（写进 last_error），不会返回一个「假装成功」的哨兵值
let hit = wait_any(["Password:", "password:", "密码"], 15);
// hit["index"] = 命中的模式下标（1 起，0 不会出现）；hit["text"] = 命中为止的文本
write("<口令>\n");
```

函数签名可能随版本演进；以 `describe()` / `term-desk api describe` 的实际输出与
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

## 会话与凭据的边界

脚本跑在前台进程里，会话就是窗口里用户可见的标签（ADR-0030）：

- 脚本开、关的会话都在同一个池里；不存在「自己开池、跑完就退」的脚本入口，
  也没有 `--exec` 一次性命令或 stdin（-）脚本形态。
- 需要密码 / keyboard-interactive 认证的会话要先在窗口里解锁保险库；脚本本身
  不会替用户解锁，拿不到凭据就显式失败。
- `ask()` / `secret()` 要有界面队列才能回答；不能提问的宿主显式报错，不会
  用空串冒充用户输入。

## 与 CRT 的有意偏差

这些不是「还没做」，而是刻意不照搬；照 CRT 手册写的脚本要按上表改写。

| 偏差 | CRT 的做法 | WindTerm 现在的事实 |
|---|---|---|
| 无多语言解释器 | 支持 VBScript / Python 等多种语言，由两行 `#$Language` 头选定 | 只有 Rhai（另有外部 Python 子进程通道）。不解释 VBScript / Python 语句，也不提供任何对象门面 |
| 无两行头 | 开头两行 `#$language = "VBScript"` 之类用于选语言 | 不解析两行头；脚本头只有可选的**单行** `// @cap ...`，它是本次运行的授权声明，不是语言选择 |
| 超时显式失败 | `WaitForString` 超时给 `false`、`ReadString` 给哨兵、`WaitForStrings` / `MatchIndex` 给 0，都不抛错 | `wait` / `wait_any` / `read_until` 超时即显式失败（原因可从 `last_error()` 读），不返回哨兵；只有排水用的 `read` 在超时时返回空串。要「布尔命中」就写 `wait` 后自己判空，别指望哨兵 |
| 无每次运行的授权 | CRT 没有 per-run capability grant：脚本能做什么取决于脚本与宿主环境，没有按次收紧的开关 | `// @cap` 与脚本面板 ▸ Permissions 是刻意加的**安全增强**，只作用于本次运行；文件、外部进程、剪贴板读、窗口与应用级状态默认全拒 |
| 无前后台模式 | CRT 不区分脚本的前台 / 后台运行模式 | 不做该区分；脚本只能操作前台实例持有的可见会话（ADR-0030），不存在自持会话池的无窗口模式 |
| 录制器形似而神不同 | 录人敲的键 + 观察到的输出，生成 `Send` / `WaitForString` 对 | 同样从**按键 + 屏幕安静锚点**录制，但生成 `write` / `read_until` 对（见「录制脚本」节） |
| 无自定义对话框 / OLE | 支持自定义窗体与 OLE 自动化 | 不支持；交互只有 `ask` / `confirm` / `message` / `text_input` / `secret` / `pick_file` 六种固定形态 |

## JSON-RPC 桥

`crates/term-bridge` 提供 JSON-RPC over stdio 的桥接，供外部 Python 子进程以
结构化方式调用能力面。`term-desk mcp` 也用 stdio 对接 MCP 客户端，但它连前台
走的是本地控制端点。外部 CLI / MCP 客户端连前台 GUI 的 loopback 控制端点
（全平台统一，默认端口 `47653`，`WINDTERM_CONTROL_PORT` 覆盖），用
`term-desk api describe` / `term-desk api call <method> [params-json]` 发请求。

前台没运行时 `api` / `mcp` 明确报错退出，绝不自己起一个会话池。没有 token
文件，也没有能力面 `auth` 动词——连接首条消息是传输级 `auth.challenge`
（key + HMAC，key 文件 `~/.zterm/control-key.local-<port>`，0600），只做连接
准入，不进动词表；另有首请求超时、1 MiB 行限与 loopback 对端校验（ADR-0030）。