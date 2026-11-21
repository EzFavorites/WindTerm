# 文档索引

WindTerm 是一个用 Rust + egui 写的跨平台终端模拟器。本目录是随仓库发布的
公开文档；内部设计台账（需求矩阵、缺口审计、逐页对齐记录）不随公开仓库发布。

| 文档 | 内容 |
|---|---|
| [architecture.md](architecture.md) | 分层、crate 边界、构建强制的不变量 |
| [building.md](building.md) | 工具链、本地构建、打包、CI 与发布 |
| [user-guide.md](user-guide.md) | 会话、SSH、SFTP、隧道、脚本的使用说明 |
| [security.md](security.md) | 凭据保险库、主机密钥校验、网络面 |
| [scripting.md](scripting.md) | Rhai 脚本 API 与 JSON-RPC 桥 |

## 设计文档

按模块组织的设计文档在 [`design/README.md`](design/README.md)：会话与连接、终端核心、
选区查找与回滚、标签分屏与布局、文件传输、凭据安全、脚本与自动化、tmux、导入与引导、
崩溃防护、主题与视觉基线、能力面与菜单、窗口壳、范围与需求、持久化与文件布局、
会话 IO 与线程层。

源码注释里的 `docs/design/<篇目>.md` 指向这些文档。

## 架构决策记录

关键决策（语言、UI 框架、编译单元划分、能力面、会话模型等）记录在
[`adr/README.md`](adr/README.md)：一编号一决策，标出取代关系，每篇写背景、决策、
后果与备选否决理由。源码注释里的 `ADR-00NN` 指向这些文件。

## 代码地图

```
crates/term-core      纯 VT 与终端语义：无 UI、无 I/O、无 OS 句柄
crates/term-app       应用层：动作、reducer、菜单、能力注册
crates/term-runtime   兼容门面：旧路径再导出，无实现
crates/term-session   会话控制面
crates/term-script    Rhai 脚本引擎
crates/term-bridge    JSON-RPC 桥
crates/term-ssh       原生 SSH 客户端（russh）
crates/term-tunnel    SSH 隧道（-L / -R / -D）
crates/term-sftp      SFTP 客户端
crates/term-xfer      文件传输编排
crates/term-zmodem    X / Y / ZMODEM
crates/term-vault     加密凭据保险库
crates/term-store     SQLite 会话库
crates/term-import    会话导入（MobaXterm 导出）
crates/term-io        会话 IO 与线程层：PTY / 串口 / Telnet、监管线程、会话日志
crates/term-paths     路径规则与数据目录
crates/term-profile   用户档案：偏好、布局、快捷栏
apps/desktop          桌面二进制（term-desk，egui UI）
```

## 文档的边界

- 这里写的是**当前实现的事实**，不是路线图。未完成的能力不写进使用说明。
- 版本号、测试数这类会漂移的数字不写死在文档里：以 `cargo test` 的实际输出
  和 `term-desk --version` 为准。
- 内部排期、需求矩阵、逐页比对记录属于开发过程材料，不随仓库发布。