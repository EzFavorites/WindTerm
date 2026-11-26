# 架构

## 总体形状

```
apps/desktop  (egui UI, 投影层)
      │
      ▼
crates/term-app  (动作 / reducer / 菜单 / 能力注册)
      │
      ▼
crates/term-core  (纯 VT 与终端语义：无 I/O、无 UI、无 OS 句柄)
```

依赖方向单向：上层可以依赖下层，下层不知道上层存在。副作用集中在少数叶子
crate，而不是散落在 UI 里。`term-runtime` 只是这些实现的兼容门面：旧导入路径
经它解析，新代码直接依赖实现 crate。

## crate 职责

| crate | 职责 | 边界 |
|---|---|---|
| `term-core` | VT 解析、屏幕模型、选择与回滚、主题与键位模型 | 零 `egui` / `eframe` / `epaint` / `winit`；零副作用 |
| `term-app` | 动作定义、reducer、菜单树、能力面注册、i18n | 不持有 OS 资源，不直接碰 UI 框架类型 |
| `term-runtime` | 兼容门面：旧路径的再导出，本身无实现 | 新代码不得依赖；依赖方向由构建门禁守住 |
| `term-session` | 会话控制面：创建、状态、销毁 | 只做控制，不做渲染 |
| `term-io` | 会话 IO 与线程层：PTY / 串口 / Telnet 通道、每会话监管线程、快照邮箱、会话日志、本地文件 IO | 持有 OS 句柄，但不依赖 UI、档案层与异步运行时 |
| `term-ssh` | SSH 客户端（russh）：认证、通道、SFTP 子系统 | `tokio` 允许出现 |
| `term-tunnel` | `-L` / `-R` / `-D` 隧道 | `tokio` 允许出现 |
| `term-sftp` | SFTP 客户端与连接池 | `tokio` 允许出现 |
| `term-xfer` | X / Y / ZMODEM 协议层与传输编排 | 零依赖的纯协议实现 |
| `term-zmodem` | ZMODEM 帧层与状态机 | 复用 `term-xfer` |
| `term-vault` | Argon2 加密凭据保险库 | 叶子 crate，不依赖终端核心 |
| `term-store` | SQLite 会话库 | 密码不以明文落盘 |
| `term-import` | 第三方会话导出导入 | 只读导入，不写回第三方格式 |
| `term-paths` | 数据目录与路径规则 | 只依赖 `dirs` |
| `term-profile` | 偏好、布局、快捷栏、搜索表 | 纯数据，无 UI |
| `term-script` | Rhai 脚本引擎与宿主 API | 脚本能力经能力面暴露 |
| `term-bridge` | JSON-RPC over stdio 桥 | 依赖白名单受限 |

## 构建强制的不变量

这些不是约定，而是 `scripts/verify.sh` 会逐条检查的门禁（CI 里跑的是
`cargo build` / `cargo test`，完整门禁在本地或发布前跑）：

- `term-core` 内不得出现 `egui` / `eframe` / `epaint` / `winit`。
- `term-core` 全目录零副作用（不读文件、不开 socket、不启动线程）。
- `tokio` 只允许出现在 `term-ssh` / `term-tunnel` / `term-sftp` 三个 crate；
  桌面 UI 源码里零 `.await`。
- `term-bridge` 的运行时依赖限定在白名单内。
- `term-app` 分层单向：词汇 → 副作用 → 状态 → 能力 → 入口 → 分发。
- 单文件代码行数（不含空行与注释）有上限；超限要拆模块而不是继续堆。
- 生产路径的 `panic` 与错误边界有冻结基线，只许减不许增。

### 硬边界判据表（改什么会红）

下表把上面的不变量映射到 `scripts/verify.sh` 的实际步骤。数字口径一律以代码为准：
能力面计数看 `crates/term-app/src/capability.rs` 的 `assert_eq!`，冻结基线看
`verify.sh` 里的常量；文档不抄一份会漂移的快照。

| 规则 | verify.sh 门禁 | 判定方式（自动 / 人工） |
|---|---|---|
| `term-core` 无 UI 框架依赖 | `4` / `4d` | 自动 grep + 依赖表 |
| `term-core` 全目录零副作用（文件、网络、进程、墙钟、环境） | `4b` | 自动 grep，例外为 0 |
| 事实流无共享写锁，事实窗口为值类型 | `4c` | 自动 grep + 纯函数单测 |
| 编译单元严格单向，UI 不越界进纯语义层 | `4d` / `4m` | 自动 grep + 编译器 |
| `term-app` 分层：词汇 → 副作用 → 状态 → 能力 → 入口 → 分发 | `4r` | 自动递归扫依赖 + 白名单 |
| 命令 ID 只出现在数据层与 `parse()` 入口 | `4e` | 自动 grep + capability 单测 |
| 投影只读事实窗口，写入经 `Action` | `4f` / `4g` | 自动 grep（类型与受控输入） |
| `term-core` 无 `std::env` / `SystemTime::now` 等宿主状态 | `4b` | 自动 grep（整行注释不算） |
| UI 五环分层，egui 触点白名单精确匹配 | `4m` | 自动 grep（越界与过期都红） |
| 无字符串派发（唯一 Action 入口） | `6` | 自动 grep |
| 每个 `ViewState` 标量字段都有读者 | `4h` | 自动 grep + capability 单测 |
| crate 边界错误分类，不退 `String` | `4i` | 自动 grep |
| 单文件代码行上限（不含空行与注释） | `4j` | 自动统计，欠账必须显式登记 |
| `tokio` 只在异步笼子三个 crate，桌面零 `.await` | `4k` | 自动 grep |
| 库真接进产品（不是只活在库里） | `8g` | 自动 grep 调用点 |
| 会话库是纯同步存储，不越界碰运行时 | `4d` | 自动依赖表 |
| 隧道 / SFTP 只认已认证连接，不反向依赖应用层 | `4d` / `4k` | 自动 grep |
| 日志写入端在监管线程，写入前做清洗 | `4b` / `8i` | 自动 grep + 端到端测试 |
| 生产路径 `panic` 基线只许减不许增 | `4p` | 自动计数（阈值见常量） |
| 错误边界 `Result<…, String>` 基线只许减不许增 | `4q` | 自动计数（阈值见常量） |
| 兼容门面 `term_runtime::` 引用只许减不许增 | `4s` | 自动计数（阈值见常量） |
| 无 Mach-O 地址字面 / 全仓地址字面 | `7` / `7b` | 自动 grep（阈值 0） |
| 置灰项必须有可读理由，与状态同源 | `4e` + 人工 | **部分人工**：数据层给理由，UI 文案靠 review |
| 选区形状归属 `Selection` 本身 | — | **人工**：`app::selection` 单测，无 grep 门禁 |
| 真机互操作需容器证据 | `scripts/docker-verify.sh` | **人工**：`#[ignore]` 测试显式跑 |
| 截图基线分整窗与组件两层，逐字节一致 | `8c`（`VERIFY_SCREENSHOT=1`） | 自动文件比对，基线变化须先归因 |

标"人工"的行没有 grep 兜底：改动时请连同对应单测与 review 一起做，不要假定门禁会拦住。

## 数据与状态

- 会话库：SQLite，位于用户数据目录（`term-paths` 决定具体位置）。
- 偏好与布局：可读的配置文件，与会话库同目录，便于备份和进版本控制。
- 凭据：只进保险库。库文件里不出现明文密码；会话库只存引用。
- 会话日志：默认关闭，开启后按会话落盘，写入前做转义清洗与 CRLF 归一。

目录只有一个出处、整文件写入走原子改名、文件属性（版本 / 写盘时间 / 退出形态）
与语义结构分开存放——这些规则的完整说明见
[持久化与文件布局](design/15-persistence-and-file-layout.md)；字节与线程如何落到
这些文件见[会话 IO 与线程层](design/16-io-and-threads.md)。

## UI 分层

`apps/desktop` 内部按职责分五层：

```
core    纯数据与投影计算
app     应用状态与动作派发
view    组件与页面
render  绘制与主题
platform 窗口、剪贴板、字体等 OS 触点
```

`egui` 类型只出现在 `ui/` 与平台白名单里；其余层对 UI 框架不可见。这样换渲染
后端时，终端语义与业务逻辑不需要改。