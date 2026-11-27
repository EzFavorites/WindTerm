# WindTerm

> A local-first desktop terminal workbench, written in Rust + egui.

WindTerm 是一个桌面终端工作台：左边是会话树和文件面板，中间是多标签终端，
下边是快捷栏、传输队列与 Sender。它用 Rust + egui 原生绘制界面，不依赖浏览器
或 WebView；终端语义放在纯 crate 里，没有 UI、I/O 与操作系统句柄。

支持本地 Shell、SSH、Telnet、TCP、串口和独立 SFTP 会话。SSH 会话上可以直接开
文件面板传文件，不必再连一次；SSH / Telnet / TCP 可以走 HTTP 或 SOCKS5 代理，
也可以配跳板链和三类端口转发。当前版本 **v0.0.1-beta3**。

> 本文只写已经实现的行为。命令面规模请用 `term-desk capability` 现算
> （写作时实测 256 条命令 / 252 已接线 / 4 置灰）；测试数量以 `cargo test`
> 的实际输出为准。架构、构建和实现细节见 [`docs/`](docs/README.md)。

## 界面一览

| 区域 | 用途 |
| --- | --- |
| **菜单栏与快捷栏** | 菜单、命令面板入口、常用动作；快捷栏成员与顺序可配置 |
| **会话树（Explorer）** | 保存的会话，按文件夹整理；双击连接，支持搜索与拖拽排序 |
| **终端区** | 多标签、左右/上下分屏与四宫格；标签悬停可看缩略图 |
| **Filer** | 当前标签所在机器的文件：本地直接读，远端走同一条 SFTP 连接 |
| **Sender / Transfer / Tmux / Script / Outline 面板** | 批量发送、传输队列、tmux 面板、Rhai 脚本库、输出大纲 |
| **Session 面板 / Shell 面板** | 已打开的会话列表、本地 Shell 入口 |
| **地址栏** | `Alt+D` 呼出；输入主机或会话名，能解析就直接连接，不留痕迹到会话库 |
| **状态栏** | 连接状态、行列、编码、主题、锁屏等 |

面板之间可以拖动分隔线调整大小；布局（分屏结构、标签顺序、面板位置）会保存，
恢复的是结构，不会恢复进程内状态——重开后的标签是断开的，激活时才重连。

## 安装

安装包在 GitHub Releases 页面，每个版本附带 `.sha256` 校验文件：

| 平台 | 文件 | 说明 |
| --- | --- | --- |
| macOS（Apple 芯片） | `WindTerm-<ver>-macos-aarch64.dmg` | 拖 `WindTerm.app` 到「应用程序」即装好 |
| macOS（Intel） | `WindTerm-<ver>-macos-x86_64.dmg` | 同上 |
| Linux（x86_64） | `WindTerm-<ver>-linux-x86_64.deb` | `sudo dpkg -i` 安装到 `/usr/bin/windterm` |
| Linux（x86_64） | `WindTerm-<ver>-linux-x86_64.zip` | 便携包：解压即用，不写系统目录 |
| Linux（aarch64） | — | 暂未提供预编译包，可从源码构建 |
| Windows（x86_64） | `WindTerm-<ver>-windows-x86_64.msi` | 安装器（开始菜单快捷方式） |
| Windows（x86_64） | `WindTerm-<ver>-windows-x86_64.zip` | 便携包：`WindTerm.exe` + 说明书 |

`<ver>` 取自 `apps/desktop/Cargo.toml`，按当前版本为 `0.0.1-beta3`（连字符）。
每个文件都附带同名 `.sha256`，例如：

```sh
shasum -a 256 -c WindTerm-0.0.1-beta3-macos-aarch64.dmg.sha256
# Linux：sha256sum -c WindTerm-0.0.1-beta3-linux-x86_64.deb.sha256
```

macOS 构建是 ad-hoc 签名，没有公证，首次启动会被 Gatekeeper 拦下：
右键 → *打开*，或执行

```sh
xattr -dr com.apple.quarantine WindTerm.app
```

### 从源码构建

需要 Rust **1.94** 或更新版本（`rust-toolchain.toml` 用 `stable`，CI 与发布产物
精确钉在 `1.94.0`）：

```sh
cargo check --workspace --locked          # 类型检查
cargo test --workspace --locked           # 全部测试
cargo build -p term-desk --locked         # 桌面程序（与 CI 同一份依赖锁）
```

提交前的项目验收脚本包含分层边界、能力注册、冻结基线、单测、金样本与可选视觉回归：

```sh
bash scripts/verify.sh
VERIFY_CLIPPY=1 bash scripts/verify.sh    # 额外跑 clippy -D warnings
```

## 无窗口命令

```sh
term-desk doctor                 # 环境体检：$SHELL / profile / 主题 / 能力面
term-desk capability             # 能力面统计；--all 列全部，--json 给机器读
term-desk serve --port 0         # 把能力面暴露到 127.0.0.1（需 token 握手）
term-desk script run file.rhai   # 在独立会话里跑 Rhai 脚本
term-desk --version
term-desk --help
```

`serve` 只监听回环地址，第一条请求必须完成 token 握手。脚本与桥接共用同一张
动词表，用会话**名字**寻址（不是进程内自增 ID），所以脚本存下来明天还能跑。

## 功能

### 终端

- 本地 Shell、SSH、Telnet、TCP、串口；真彩色、Unicode 双宽、emoji、鼠标上报，
  支持 vim / htop / less 这类全屏程序。
- 多标签与分屏；标签可拖动排序，悬停出缩略图，缩放比例可调。
- 缓冲区内查找、行号/时间戳侧栏、命令块折叠与大纲；编码切换覆盖 UTF-8、
  GBK、GB2312、Big5、Shift_JIS、EUC-JP、EUC-KR、KOI8-R、ISO-8859-1/15。
  编码只影响切换之后的输出，已显示内容不重排。
- 选中即复制、右键动作可配、多行粘贴确认、bracketed paste；远端 OSC 52
  剪贴板请求可按询问 / 允许 / 禁止处理。
- 会话日志：`Session › Log` 起停、暂停/恢复、打开日志文件或目录。默认只记
  可打印文本，也可记录全部原始字节；同名日志自动加序号，不覆盖。
  `Mode › Debug Mode` 除了画单元格属性覆盖层，还会在日志槽空闲时起一份
  `debug_` 前缀的**原始字节排障日志**（含转义序列、有 16 MiB 轮转上限，
  同套凭据脱敏），写盘失败会在状态栏报出。
- tmux 控制模式：`Window › Panes › Tmux Control Mode`。对已经 `tmux -CC` 的
  会话，**解析并展示**远端的 tmux 状态，并可向选中的窗格/窗口下发经过目标校验的
  反向控制命令；**不把 tmux 的窗口/面板映射成本地标签与分屏**。解析不了的指令
  原样透传。

### 会话与连接

- 会话树支持分组、重命名、拖拽排序、过滤；可从 MobaXterm 导出文件批量导入
  （按会话名归并去重：同名在本次导入内做区分，不覆盖已有会话；没有「主机 + 端口
  相同即同一条」的判据）。**只导配置，不导密码**。
- 首次启动会探测 `~/.ssh/config`，勾选后导入到 `Imported` 分组。
- 内置 SSH 与系统 `ssh` 两种客户端，按会话选：内置用于 SFTP、隧道、跳板链
  和保险库口令自动注入，且已把 TCP_NODELAY 与 SSH keepalive 真正设到连接上
  （不再只是偏好里的开关）；系统 `ssh` 直接复用用户已有配置与 agent，只映射
  TCPKeepAlive——OpenSSH 没有 TCP_NODELAY 选项，不会往命令行塞这个参数。
- 认证支持密码、私钥、ssh-agent，以及可复用的 **OneKey** 方案（首选 + 回退链）。
  密码可以"不记住 / 本次记住 / 永久记住（写入保险库）"。
- 主机密钥按原版 1:1 在客户端内部处理并自动接受新的：连接链路只有一个
  用户名/密码弹窗，不弹主机密钥确认窗，也不持久化 accepted host key。
- 跳板机写成逗号分隔的 ProxyJump 链（`[user@]host[:port]`），每跳各自握手；
  agent 转发只作用于最终目标。
- 断线自动重连：在会话属性的 Session 页开启，可调初始延迟、退避倍数、单次封顶、
  总重试窗口（默认 180 秒，覆盖设备 reboot）与次数上限。默认关闭；只有"连接断开"
  才触发（网络与串口），密码错误立即停止，本地 Shell 不重连。连接状态在状态栏可见。

### 文件与传输

- Filer 顶部切换"对着哪台机器"，每台各自记住浏览位置。可浏览、下载、上传
  （拖拽即上传）、新建、重命名、删除、改权限，并查看修改日期。
- 远端文件用"下载到临时副本 → 系统默认程序打开 → 写回"的链路，写回保留
  原权限位；**没有内建通用文本编辑器**。
- 双击文件（或右键"预览"）开只读预览窗：按字节内容判类型，不靠扩展名；
  大文件只读开头，不会整个读进内存。会话断开时清掉预览快照、目录同步决策态
  与传输进度显示，旧连接的残留状态不会跟着切到新连接上。
- 传输队列有进度、速率与剩余时间，取消分"用户取消 / 网络中断 / 目标已存在"
  三种语义；目录同步有决策、执行记账与 Review 界面。
- X / Y / ZMODEM 在 `Tool` 菜单里；也可以把文件拖进终端打开 XMODEM 发送弹层。
  传输走原始字节旁路，期间终端画面冻结是正常的，进度看传输面板。
- SFTP 传输基准可用 `scripts/transfer-bench.sh` 在容器里跑，报告落在
  [`docs/bench/transfer-report.md`](docs/bench/transfer-report.md)。

### 网络

- 隧道：`-L` 本地转发、`-R` 远程转发、`-D` 动态 SOCKS5（本地监听器当前只支持无认证客户端；出网代理的凭据另算）。
  每条隧道有名字和归属会话，状态与失败原因直接显示在面板里；监听端口必须
  显式指定，填 0 会被表单拒绝。
- 隧道用独立的 SSH 连接搬运字节，不复用终端那条；凭据已在内存里，不会二次询问。
  总开关之外每条还能单独设"自动启动"。启动自动拉起的判据是 `auto_open && status`：
  两个勾选都为真才会拉起；只勾「自动启动」而这条的 `Status` 没开，不会在启动时起。
- 出网代理支持 HTTP CONNECT 与 SOCKS5，可设全局默认，也可按会话覆盖；
  开会话前会做回落判断，不会带着无效代理配置去连。

### 自动化与外观

- Rhai 脚本引擎：内置会话 API、文件与剪贴板辅助；`term-desk script run`
  无窗口执行，界面里有脚本库和带行号、错误跳行的脚本编辑器。随附 5 个可直接改的
  示例（自动登录、批量巡检落盘、抓命令输出、多标签下发、自动应答）在
  `apps/desktop/resources/scripts/`，写法与 API 速查见
  [`docs/scripting.md`](docs/scripting.md)。
- 触发器：事件 → 匹配 → 动作，规则落盘到偏好；命中可执行发文本、发命令、
  响铃等。`Highlight text` 类型的关键字高亮**已实现**：命中行在终端字形下铺一层
  淡色底，只留最近 64 行，不进选区也不影响复制。
- Sender 把同一段文本发给多个会话，输出按一致性分组，危险命令强制确认。
- 命令面板（搜索框）把动作与菜单项拉平：`Command` 只列已接线命令，
  `Anything` 列出全部并标出置灰项。
- 深色/浅色主题、终端配色方案（内置七套 + 自家 `.theme` 文件；iTerm2 `.itermcolors`
  与 Windows Terminal `schemes` JSON 的解析尚未实现）、字体与字号、
  窗口透明度（30%–100%）、锁屏与空闲自动上锁、可重绑定的快捷键。

## 常用快捷键

`Cmd` 在 Windows / Linux 上归一为 `Ctrl`。

| 快捷键 | 功能 |
| --- | --- |
| `Cmd+Shift+P` | 命令面板（Command） |
| `Cmd+Shift+O` | 命令面板（Anything） |
| `Cmd+N` | 新建会话 |
| `Cmd+Shift+N` | 新建窗口 |
| `Cmd+Shift+F` / `Cmd+G` | 查找 / 查找下一个 |
| `Cmd+K` | 跳到指定行 |
| `Alt+D` | 地址栏 |
| `Cmd+Shift+L` | 锁屏 |
| `Cmd+W` | 关闭当前标签 |
| `Cmd+Shift+T` | 向右分屏 |
| `Cmd+Shift+I` | 窗口列表 |
| `Alt+W, Alt+H` / `Alt+W, Alt+V` | 横向 / 纵向分屏 |
| `Alt+W, Alt+G` | 关闭当前分屏组 |
| `Alt+M, Alt+H` | 切换十六进制显示 |
| `Meta+T, Meta+K` / `Meta+T, Meta+T` | OneKeys / Triggers |

用逗号隔开的是两段式按键（先按第一个，再按第二个）。`Meta` 与 `Cmd` 等价：
macOS 上是 Command，Windows / Linux 上归一为 Ctrl；设置里可以改绑定。菜单右侧展示的
文案来自视觉基线，个别项与出厂绑定不同，实际生效的是上表这一套。

## 设置与数据文件

- 设置中心：`Session › Preferences › Settings`。左栏覆盖应用、外观、代理、
  在线搜索、安全、标签、终端、文本、文件面板、快捷栏、触发器、OneKeys 等页。
  大多数页改的是**新会话默认值**，不追溯已打开的标签。
- 偏好、会话库、布局、保险库、日志、脚本都放在同一个 profile 目录里，便于整体
  备份或搬走：

  | 平台 | 目录 |
  | --- | --- |
  | macOS | `~/Library/Application Support/WindTerm/` |
  | Linux | `~/.config/WindTerm/` |
  | Windows | `%APPDATA%\WindTerm\` |

- 用 `WINDTERM_PROFILE_DIR` 可以整体重定向（便携配置与测试隔离都靠它）：

  ```sh
  WINDTERM_PROFILE_DIR=/tmp/wt-profile ./windterm
  ```

- 主要文件：`sessions.db`（会话库）、`prefs.json`（偏好）、`layout.json`
  （窗口与标签结构）、`vault.ztv`（加密保险库）、`scripts/`（Rhai 脚本）。
  保险库条目不以明文落盘。

## 已知限制

- 4 条命令仍然置灰：`Text.ToggleVirtualSpace`、`Text.ToggleMultipleCaret`
  是终端行编辑行为（不是内建文本编辑器语义），尚未实现，列为待办并按对齐后的视觉基线先画勾选态；
  `Window.ToolbarListEmpty`、
  `Window.ToolbarCustomizePlaceholder` 是原图里 Toolbar 子菜单的两行空态灰字，
  本版没有自定义工具栏条目模型，不摆点了没反应的假控件。以下已接线：
  `Window.GotoSymbol`（符号表 = OSC 133 命令块）、`Window.SetRemoteMode`
  （退出只读）、`Text.ToggleWordWrap`（Mode ▸ Word Wrap，按 wrap column 软折）。
  `Search Online` 六条、`Syntax Schemes` 七条与 `Select` / `Select To` 二十一条
  已于 2026-11-27 全部接通（详见能力面台账）。
- 菜单里个别项的快捷键文案与出厂绑定不同（为保真视觉基线保留），按上表按键。
- 窗口透明目前是纯 alpha；macOS vibrancy / Windows acrylic 的平台模糊还没做。
- 不做云同步、账号体系、团队协作、远程桌面与通用文本编辑器，范围见
  [`docs/design/14-scope-and-requirements.md`](docs/design/14-scope-and-requirements.md)。

## 与上游 WindTerm 的关系

原版 WindTerm 项目：[github.com/kingToolbox/WindTerm](https://github.com/kingToolbox/WindTerm)。

本项目是独立实现的 clean-room 重建：只参考可观测行为与公开文档，不复制上游的
二进制、专有 `.term` 配置、图标、主题、截图或翻译资产，也不与其作者或发布方
存在关联。文档里提到的第三方产品名归各自所有者所有。

上游 README 把产品定位为 "A Quicker and better SSH/Telnet/Serial/Shell/Sftp
client for DevOps"，并给出下列公开自述指标。**这些数字来自 2020 年的
WindTerm 1.x / 1.72（Windows 10，2.3 GHz i5，8 GB），原文也注明仅供参考，
不是本项目的实测或承诺**：

| 上游自述项 | 数值 |
| --- | --- |
| SFTP 5 GB 随机文件下载 / 上传 | 216.3 MB/s / 247.0 MB/s |
| Telnet / SSH 终端吞吐 | 52.1 MB/s / 41.8 MB/s |
| `seq 1 1000万` | 10.674 s / 133.3 MB（对比客户端多数 OOM） |
| 动态内存压缩 | 工作内存降低 20%–90% |
| 终端兼容 | vt100/220/340/420/520、xterm-256color；vttest 除 Tektronix 4014 全通过 |

上游还提到本地/远程模式配合 vim 键位（用 Shift+Enter 切换）。本项目已有本地模式
的 vim 键位解析层（`term_app::vim`：26 个导航键与 `y`/`p` 复制粘贴），本地/远端模式
均已接线：`Window.SetLocalMode` 与 `Window.SetRemoteMode` 共用 `Tab.local_mode`，
后者的勾走 `CheckState::Live`，不再置灰；切换入口是 Mode 菜单，不照抄上游的
Shift+Enter。本项目的性能口径以
[`docs/bench/transfer-report.md`](docs/bench/transfer-report.md) 的可复跑基准为准。

## 文档

- [`docs/architecture.md`](docs/architecture.md) — 分层、crate 边界与编译期不变量
- [`docs/building.md`](docs/building.md) — 工具链、构建、打包、CI 与发布
- [`docs/user-guide.md`](docs/user-guide.md) — 会话、SSH、SFTP、隧道、脚本
- [`docs/security.md`](docs/security.md) — 保险库、主机密钥与网络面
- [`docs/scripting.md`](docs/scripting.md) — Rhai API 与 JSON-RPC 桥
- [`docs/design/README.md`](docs/design/README.md) — 逐模块设计文档
- [`docs/adr/README.md`](docs/adr/README.md) — 架构决策记录
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — 提交前的门禁
- [原版 WindTerm 上游仓库](https://github.com/kingToolbox/WindTerm) — 本项目参考的原版项目

## 许可证

[Apache License 2.0](LICENSE)。
