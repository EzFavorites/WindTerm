# 设计文档

按模块组织的设计说明。每篇回答「这一层负责什么、关键取舍是什么、边界在哪」，不记录开发过程与进度。

| 篇目 | 主题 |
|---|---|
| [01-sessions-and-connection.md](01-sessions-and-connection.md) | 会话模型、SSH 认证、主机密钥、跳板、隧道 |
| [02-terminal-core.md](02-terminal-core.md) | VT 语义、屏幕模型、字符宽度、字体与渲染分层 |
| [03-selection-find-scrollback.md](03-selection-find-scrollback.md) | 选区、复制粘贴、查找、滚动条与悬停预览 |
| [04-tabs-split-layout.md](04-tabs-split-layout.md) | 标签与分屏、每窗格分组、布局持久化与恢复 |
| [05-files-and-transfer.md](05-files-and-transfer.md) | SFTP 面板、传输队列、取消三态、续传、ZMODEM |
| [06-security-credentials.md](06-security-credentials.md) | 保险库、密码策略、锁屏、回环服务准入 |
| [07-scripting-and-automation.md](07-scripting-and-automation.md) | Rhai 引擎、宿主 API、执行预算、JSON-RPC 桥 |
| [08-tmux-integration.md](08-tmux-integration.md) | 控制模式解析、事件映射、安全降级 |
| [09-session-import-and-onboarding.md](09-session-import-and-onboarding.md) | 会话导入、宽松解析、首次启动引导 |
| [10-crash-hardening.md](10-crash-hardening.md) | 三层崩溃防护、落盘与恢复 |
| [11-theme-and-visual-baseline.md](11-theme-and-visual-baseline.md) | 主题与配色、控件尺寸、自有截图基线 |
| [12-capability-surface-and-menus.md](12-capability-surface-and-menus.md) | 能力面三态、命令标识、菜单结构与命令面板 |
| [13-ui-shell.md](13-ui-shell.md) | 窗口壳、状态栏、快捷栏、通知条、命令面板 |
| [14-scope-and-requirements.md](14-scope-and-requirements.md) | 功能范围、明确不做、需求条目格式、版本阶段 |
| [15-persistence-and-file-layout.md](15-persistence-and-file-layout.md) | profile 目录、原子写与多进程、启动期搬迁、档案层边界、各文件职责 |
| [16-io-and-threads.md](16-io-and-threads.md) | 会话 IO 与线程层：通道、监管线程、快照邮箱、字节分流、会话日志、本地文件 IO |

## 阅读顺序

- 想理解整体结构：先读 [`../architecture.md`](../architecture.md)，再按模块读本目录。
- 想参与某个模块：读对应篇目 + 该模块源码；`docs/building.md` 里有构建与门禁。
- 想提需求：先读 [14](14-scope-and-requirements.md) 的分级与「明确不做」，再对照具体模块篇目。

## 约定

- 设计文档描述**当前实现与其取舍**，不写未落地的设想；设想留在 issue。
- 会漂移的数字（版本、测试条数、命令条数）不写死，以命令输出为准。
- 源码注释引用设计文档时，路径指向本目录下的篇目。