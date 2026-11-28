# Changelog

本文件记录本项目面向用户的显著变更。格式沿用 [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)。
发布工作流直接把本文件当发布说明，所以 0.0.1 节排在 beta3 之前——读者不必从 beta3 读起。

## [0.0.1] - 2026-11-28

### Added

- ADR-0030 前台单一控制面：脚本、API（CLI / MCP）与规则共用同一张动词表，
  会话用名字寻址；脚本与 API 读写同一会话共享捕获游标，不再各持一份。
- 无窗口 `term-desk api describe` / `api call` / `mcp` 作为前台控制端点的外部客户端：
  前台未运行即报错退出，不自建会话池；锁屏时拒绝接入与调用（`Locked`）。
  准入是 0600 key 文件加传输级 `auth.challenge`（key + HMAC），不设 token 文件、
  不设能力面 `auth` 动词。
- 只读的 Script / API 设置页；脚本面板工具条重做，锁屏界面居中并冻结视觉基线。
- 跨平台锁屏探测：macOS 走 `ioreg`，Windows 查询 User32 输入桌面，Linux 查询
  freedesktop `ScreenSaver.GetActive`；保持三态与 fail-open 语义。
- `Tool ▸ Script` 子菜单：运行脚本 / 停止脚本 / 录制脚本 / 新建脚本 / 管理脚本；
  面板显隐仍归 `Window ▸ Panes ▸ Script`，两者互不带动。
- 会话树右键 `Logon Script…`：为已存会话配置登录脚本（下次连接生效），
  不改 1:1 会话表单与视觉基线。
- 快捷命令栏新增 `Run Script` 类型：只填路径，界面自动补 `Script.RunFile:` 前缀。
- 外部 Python 脚本：`.py` 走子进程 + stdio JSON-RPC 桥，提供从能力表派生的原生
  函数 API；运行需 `process` 授权，UI / CLI 均可运行，取消会 kill 子进程。
- `docs/scripting.md` 补录制脚本与命令行边界两节。

### Changed

- 触发器族统一到唯一内核 `term_core::automation`：用户规则、内置检测、OneKey
  连接期认证、登录脚本共用 `Pattern` / `ScanCursor` / `Evaluator` 与一套仲裁；
  用户键盘抢占、认证期独占写、多个写意图同帧排序后一次落到 PTY。
- 规则动作装载期校验（会话动词 / 应用动作 / 凭据引用 / 运行脚本四类），
  不合法即拒绝装载，不再命中后才变 `NoEffect`；`RunScript` 改为运行时窄授权。
- OneKey 收敛为纯连接期凭据方案：密码与键盘交互答案只进保险库条目
  `onekey:<name>` 与 `onekey:<name>:ki`，**键盘交互答案不再写进 `prefs.json`**；
  需要终端层登录自动化改用带 `CredentialRef` 的普通规则。
- 控制端点由 UDS / 命名管道改为全平台统一的 TCP loopback 端口（默认 `47653`，
  `WINDTERM_CONTROL_PORT` 覆盖）；key 文件按端口隔离为
  `~/.zterm/control-key.local-<port>`，并增加首请求超时与 1 MiB 行上限。
- 已登记脚本的参数与最近列表在运行窗口 / 停靠面板 / 编辑器三个入口统一。
- 脚本面板的录制按钮与单次授权折叠面板接入中文本地化。
- 脚本编辑器移除冗余 `open` 标志，模态开关只由 `Option` 决定。
- 脚本管理器的相对路径按脚本库解析，不再依赖进程当前目录。
- 消息框补齐 CRT 的 `MB_ABORTRETRYIGNORE` / `MB_RETRYCANCEL` 两组按钮：宿主能画出
  六个按钮组，并回传具体按下的按钮以映射到各自的 CRT ID（三按钮组不再被压成是/否）。

### Fixed

- 保险库与配置文件的原子写改为 Unix 0600 临时文件、profile 目录 0700，
  并拒绝符号链接临时名，不再短暂以 umask 默认权限落盘。
- Telnet 子协商加长度上限与背压（收集 / 排空按帧预算限额，未取完留在队列），
  不再让畸形协商无限占内存。
- 命令历史 TTL 计算修正溢出，超长保留期不再回绕成立即过期。
- X11 转发 cookie 改为随机生成，不再使用可预测值。
- XMODEM / YMODEM / ZMODEM 接收端在握手期消毒并拒绝恶意文件名（路径成分、控制字符）。
- SFTP 取消归入明确的错误分类，不再与其它 `Closed` 场景混同。
- `known_hosts` 管理中 `@directive` 行、CRLF 与末行无换行的条目原样保留。
- SFTP 续传逐块校验前缀内容，不再只比较长度就接着写。
- 控制面：断连即取消连接级 `HostEnv`；消除 Grant 检查的 TOCTOU；
  `Busy` / `Suppressed` 作为显式错误分类返回；后台命令执行接入统一仲裁。
- 连接与传输资源：SSH DNS 解析移入 worker 并加超时（超时分支发取消信号 + 有界 join，
  握后清零凭据）；隧道监听退出置 `alive=false` 并唤醒主任务释放 SSH；
  SOCKS5 greeting 循环读满、半包逐字节不丢；会话键按唯一池作用域隔离，
  关闭即清除。
- `ask` 模态的 Esc / Cancel 走 `NoInput`，不再冒充 `Bool(false)`；Confirm 的
  「否」仍是 `Ok(Bool(false))`，文件框取消仍是 `None`。
- 移除对象门面：不再提供 CRT 对象形状，改为借鉴其设计思想（录制节奏、
  消费式/观察式等待、按名字寻址、多入口、按运行授权），全部落到原生动词。
- 修正 `docs/scripting.md` 的偏差表：`wait` / `read_until` / `wait_any` 超时显式失败，
  只有排水用的 `read` 返回空串；补上多语言 / 两行头 / 每次运行授权 / 前后台模式 /
  录制器的 CRT 对照。
- 修正随附示例：`00-template.rhai` 与 `03-capture-output.rhai` 用 `write` + `read_until`，
  `04-multi-tab-dispatch.rhai` 改用 `session_list` / `session_use` 原生动词；
  `02-batch-inspection.rhai` 的口令提示索引改为 1 起的 3 / 4。
- 修正 `wait_any` 文档与两个随附示例的索引口径（1 起下标）。
- 修正 `secret`（交互式掩码输入，非保险库句柄）与 `term-desk capability --json`
  （菜单命令台账，非动词表）的文档描述。
- `Tool ▸ Script ▸ Stop Script` 叫停全部运行入口；面板重复运行不再堆积只读输出标签。
- 重冻 Tool 菜单相关的四张组件视觉基线。

### Removed

- OneKey 的 `expects` / `ExpectSend` / `ExpectFlags` / `expect_matches` 与输出帧应答块。
- 旧触发器内核：`term_core::trigger`、`term_app::trigger::Matcher`、
  `trigger_queue`、`trigger_regex` 与三元素 `DISPATCHABLE_METHODS`。
- 无头入口：`term-desk script run`、`desk serve`、`desk --headless --exec`。
- 各平台的 UDS / 命名管道控制端点及其平台专属权限复核代码。

### 破坏性变更

- 旧 `prefs.json`、vault 条目与规则数据**不迁移**：软件仍处于开发设计阶段，
  明确不做历史兼容，升级后按新形状重新配置。
- OneKey `expects` 删除；依赖它的登录自动化需改用带 `CredentialRef` 的普通规则
  （`once = Connection` + prompt 门控）。
- 无头入口删除；操作目标必须是前台实例持有的、用户可见的会话（不要求窗口有输入焦点）。
- 控制端点全平台改为 TCP loopback（默认端口 `47653`，`WINDTERM_CONTROL_PORT` 覆盖）；
  旧 UDS / 命名管道路径与 token 文件不再存在，客户端一律走 key 文件 +
  `auth.challenge`。

### 安全须知

- **主机 key 由客户端自动接受**：不校验服务端身份，连接可能被中间人冒充且界面无提示。
  这与参考版本一致，是有意取舍，**不是安全推荐**；MITM 风险由用户承担，严格校验
  场景请走 `term_ssh` 的 `Strict` / `Tofu` 策略或脚本路径。
- **loopback + key / HMAC 不是同用户恶意代码边界**：同用户进程能读 0600 key 文件，
  也能直接跑 CLI；这层防护挡的是误连、跨用户与沙箱隔离，不应被高估。
- **外部 Python 完全可信**：它是独立进程，本身就能读文件，cap 授权不构成真实约束；
  Rhai 仍是沙箱，`// @cap` 与界面勾选的按次授权保留。
- **`describe` 工具无推送**：MCP tools 全部由前台 `describe` 的 schema 生成，
  没有 resources / prompts / logging / 推送 / 订阅，脚本专用动词不暴露给 API。

## [0.0.1-beta4] - 2026-11-27

### Added

- 会话 tab 只显示会话名，不再拼退出状态或 OSC 动态标题。
- 首次启动按系统实际支持的 shell（`/etc/shells`、`$SHELL`、`%COMSPEC%`）生成
  `Shell sessions` 会话列表，替代写死的 `Local Shell` 与示例 SSH 条目。
- 会话字符集真正接线：表单里选的编码现在逐块解码远端字节（GBK / GB2312 /
  Big5 / Shift_JIS / EUC-JP / EUC-KR / 单字节编码），跨 chunk 的半个汉字不再丢；
  默认 UTF-8 与未知标签仍是零拷贝直传。
- 建连前错误（凭据解析、ProxyJump 拼装等）写进与 Debug Mode 同源的排障日志，
  连接失败后可直接看文件定位。
- 主机 key 由客户端内部自动接受：连接链路只保留用户名 / 密码弹窗，界面不持久化
  accepted host key，也不提供 UNKNOWN / CHANGED / 忘记交互（见 `docs/security.md`
  的风险说明）。
- 接通 34 条原先置灰的占位菜单（Search 6 / Scheme 7 / Select 21）；
  Send Condition 默认不延迟，ZMODEM 冲突策略与传输后打开改为按会话可配。

### Changed

- 探测出的 shell 由会话的启动程序字段真正指定，选哪条就启动哪个 shell，不再一律退回登录 shell。
- 发布产物校验和改为强制项：任何平台打包工具或 SHA-256 工具缺失，都在产物落盘
  之前硬失败，不再留下没有 `.sha256` 的半成品发布包。
- 视觉基线按对齐后的默认值重冻：15 张整窗图（含中文）与组件图；门禁改用代码行
  计数与稳健断言。
- 对外口径统一为「参考版本 / 对齐目标」，清理残余内部代号。

### Fixed

- 切断 selection → folds 的架构回边，`fold_rows` 接收绝对行区间。
- 字符集解码不再让数据里的 BOM 改写显式标签；会话结束时未完结的多字节序列
  以替换字符收尾，不再静默丢弃。

## [0.0.1-beta3] - 2026-11-26

### Added

- 自动重连：会话属性 Session 页可配置初始延迟、退避倍数、单次封顶、总重试窗口
  （默认 180 秒，覆盖设备 reboot）与次数上限；默认关闭，仅"连接断开"触发（网络与串口），
  密码/主机密钥错误立即停止，本地 Shell 不重连。
- OneKey 自动登录改为内置语义：选中方案即登录，移除表单勾选框。

### Changed

- 发布产物按平台分发：macOS `.dmg`、Linux `.deb` + 便携 `.zip`、
  Windows `.msi` + 便携 `.zip`（均带 `.sha256`），直接发布 Pre-release。
- 原版对照台账修正 12 条错图，全页核对结论见 `reference-page-audit.md`；
  参考图暂不脱离归档，11 处设置页差异待逐项定夺。

### Fixed

- 会话层：修凭据一次性引用争用、SFTP 断线 `Closed` 语义、隧道活性与启动期拉起、
  telnet mock 偶发失败。
- 同帧多条自动重连改为每帧一条，避免全局 Connector 互相覆盖；锁屏时清运行时凭据。
- Bell 页文案回归原版完整标签；修正多处过期注释与文档失真。

## [0.0.1-beta2] - 2026-11-26

### Added

- 系统文件/目录对话框：设置页、会话属性等处的「浏览」按钮改用平台原生对话框，
  界面只持有不透明句柄；截图与无头运行不会弹出系统窗口。
- 脚本能力面：`app_command` / `app_catalog` 经统一命令入口暴露；发布包附带
  5 个 Rhai 任务示例。
- 锁屏密码框新增「显示明文」切换（绘制的小眼睛，不依赖 emoji 字体）。
- `Mode › Debug Mode` 在属性覆盖层之外还会产出 `debug_` 前缀的原始字节排障日志
  （含转义序列、16 MiB 轮转上限、与会话日志同一套凭据脱敏）；界面提示会同步
  写入当前会话日志，便于事后排查。

### Changed

- 菜单访问键按**同一级**去重，优先保留原有下划线位置；中文按显示的文字去重，
  右键菜单与菜单树使用同一套判据。
- 锁屏界面按视觉基线重做：暗底、蓝色标题、单密码框；无库时 `Esc` 为唯一退路。
- 脚本写按根目录记录授权；CRT 门面语义、超时与 `last_error` 行为对齐。

### Fixed

- 脚本 `Stop` 改为真取消，排队请求带取消令牌并在 UI 侧丢弃；后台脚本有独立租约上限。
- 系统 `ssh` 之外的脚本授权与 SFTP 本地端点接回同一条授权路径。
- 发布包 macOS 产物此前未包含 `LICENSE` / `README.md`，现已放入 `.app` 并在发布工作流加结构断言。
- README 命令面计数与代码断言对齐（224 条 / 222 已接线 / 2 置灰）。
- 会话日志写盘 / 刷新失败现在上报状态栏，不再让界面一直显示「Recording」。
- 脱敏改为字节级替换，原始字节日志不再被 `from_utf8_lossy` 改写。
- 重连边沿不再用自动日志覆盖 Debug Mode 的排障日志。

## [0.0.1-beta] - 2026-11-25

首轮公开 beta：Rust + egui 桌面终端工作台，支持本地 Shell / SSH / Telnet / TCP /
串口、内置 SFTP、跳板与端口转发、Rhai 脚本与能力面服务。详见 `README.md` 与
`docs/`。
