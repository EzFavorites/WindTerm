# Changelog

本文件记录本项目面向用户的显著变更。格式沿用 [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)。
版本号与发布日期在实际发布时写入；未发布的内容留在 `[Unreleased]`。
发布工作流直接把本文件当发布说明，所以本节排在 beta3 之前——读者不必从 beta3 读起。

## [Unreleased]

### Added

- `Tool ▸ Script` 子菜单：运行脚本 / 停止脚本 / 录制脚本 / 新建脚本 / 管理脚本；
  面板显隐仍归 `Window ▸ Panes ▸ Script`，两者互不带动。
- 会话树右键 `Logon Script…`：为已存会话配置登录脚本（下次连接生效），
  不改 1:1 会话表单与视觉基线。
- 快捷命令栏新增 `Run Script` 类型：只填路径，界面自动补 `Script.RunFile:` 前缀。
- `docs/scripting.md` 补录制脚本与命令行边界两节。
- 外部 Python 脚本：`.py` 走子进程 + stdio JSON-RPC 桥，提供从能力表派生的原生
  函数 API；运行需 `process` 授权，UI / CLI 均可运行，取消会 kill 子进程。

### Changed

- 已登记脚本的参数与最近列表在运行窗口 / 停靠面板 / 编辑器三个入口统一。
- 脚本面板的录制按钮与单次授权折叠面板接入中文本地化。
- 脚本编辑器移除冗余 `open` 标志，模态开关只由 `Option` 决定。
- 脚本管理器的相对路径按脚本库解析，不再依赖进程当前目录。

### Fixed

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
- 消息框补齐 CRT 的 `MB_ABORTRETRYIGNORE` / `MB_RETRYCANCEL` 两组按钮：宿主能画出
  六个按钮组，并回传具体按下的按钮以映射到各自的 CRT ID（三按钮组不再被压成是/否）。

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
