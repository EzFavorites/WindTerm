# 架构决策记录（ADR）

一编号一决策。状态分两类：**已采纳**、**已被取代**（正文注明由哪一篇取代）。编号按历史保留，未使用的编号不补（0010–0013 空号）。

| 编号 | 决策 | 状态 |
|---|---|---|
| [0001](0001-language-rust.md) | 实现语言选用 Rust | 已采纳 |
| [0002](0002-gui-tauri.md) | 桌面壳选用 Tauri 2 | 已被取代（0007） |
| [0003](0003-vt-engine-boundary.md) | VT 解析先放在前端 xterm.js | 已被取代（0008） |
| [0004](0004-ssh-stack.md) | SSH 栈选用 russh | 已采纳 |
| [0005](0005-license-apache2.md) | 自有代码采用 Apache-2.0 | 已采纳 |
| [0006](0006-plugin-runtime-wasm.md) | 插件运行时选用 WASM | 已采纳 |
| [0007](0007-gui-egui-native.md) | 桌面 UI 改用 egui / eframe | 已采纳（取代 0002） |
| [0008](0008-vt-core-alacritty.md) | VT 内核与自研网格渲染 | 已采纳（取代 0003） |
| [0009](0009-single-crate-boundaries.md) | 单 crate 三层边界 | 已被取代（0014） |
| [0014](0014-compilation-units.md) | 按副作用划分编译单元、拆分 workspace | 已采纳（取代 0009） |
| [0015](0015-capability-registration.md) | 能力注册：动作按能力分区，未接线命令显式置灰 | 已采纳 |
| [0016](0016-app-layer-and-apps-layout.md) | 应用层独立成 `term-app`，可执行目标进 `apps/` | 已采纳 |
| [0017](0017-backend-staging.md) | 后端分期：保险库 / 会话库 / 导入 | 已采纳 |
| [0018](0018-single-connection-path.md) | 同一会话名只有一条连接路径 | 已采纳 |
| [0019](0019-capability-surface-session-ownership.md) | 能力面与会话归属 | 已采纳 |
| [0020](0020-executable-target-convergence.md) | 可执行目标收敛 | 已采纳 |
| [0021](0021-desk-ui-layering.md) | 桌面 UI 分层 | 已采纳 |
| [0022](0022-desk-serve.md) | 无窗口 `serve` 与回环准入 | 已采纳 |
| [0023](0023-script-and-rpc-share-one-surface.md) | 脚本与 RPC 共用一套能力面 | 已采纳 |
| [0024](0024-session-addressing.md) | 会话寻址 | 已采纳 |
| [0025](0025-session-as-unit-tab-as-projection.md) | 会话为单元、标签为投影 | 已采纳 |
| [0026](0026-per-pane-tab-groups.md) | 每窗格一组标签 | 已采纳 |
| [0027](0027-script-platform-architecture.md) | 脚本平台目标架构 | 已采纳 |
| [0028](0028-scrollbar-preview-window.md) | 滚动条悬停预览的第二事实窗口 | 已采纳 |
| [0029](0029-runtime-layer-convergence.md) | 副作用会话按职责收敛：IO、档案、路径各自成层，运行时退化为兼容门面 | 已采纳（细化 0014） |

## 读法

- 想知道**现在**为什么这样分层：读状态为「已采纳」且未被取代的篇目。
- 想知道**为什么曾经那样选**：读被取代的两篇（0002 Tauri、0003 前端 VT）与取代它们的 0007、0008；演进链本身是设计资料。
- 每篇结构一致：背景 → 决策 → 后果 → 备选方案与否决理由。后果写代价，不只写收益。

## 与设计文档的关系

- ADR 记录**为什么这样定**（一次性决策，不随实现细节变）。
- `docs/design/` 记录**现在怎么实现**（模块职责、数据结构、边界）。
- 两者冲突时：实现层细节以 `docs/design/` 为准，决策本身以 ADR 为准；冲突需要在 PR 里指出并改其中一份。