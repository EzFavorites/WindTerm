# ADR-0030 · 前台单一控制面：脚本、API 与规则共用一套动词层

- 状态：已采纳（取代 ADR-0019 §5、ADR-0020 §2/§5、ADR-0022 全文；ADR-0023 的「两个前端 / Exposure 三档」表述、ADR-0027 的装配部分按本文调整；ADR-0024 / 0025 / 0026 / 0029 继续有效）
- 关系：软件处于开发设计阶段，**不考虑历史兼容**——旧 `prefs.json` / vault 条目形状不迁移，旧 ADR 按篇标注取代关系（ADR 索引已更新），不做行为兼容

## 背景

1. 现在所谓「脚本层 / API 层」是**假分层**：同一个动词在 `term-script/src/engine.rs` 的 `register_fn`、`apps/desktop/src/bridge/surface.rs` 的 `match method`、规则动作的 `Invocation` 里各实现一遍，靠集合相等断言兜底。游标也分裂：`ScriptWorkspace` 与 `DeskSurface.cursors` 各持一份 `ScriptSession`，同一会话被脚本与 API 交错读写时各走各的。
2. 事务层（UI 队列、`Capabilities`、`CancelToken`）只在脚本路径生效；桥接路径完全不查授权。结果是远程客户端比本地脚本权力更大。
3. 产品已定：**不支持无头模式，操作必须在前台**。但 `term-desk script run` / `desk serve` / `--headless --exec` 仍然自持会话池，与此直接冲突。
4. 外部控制是真实需求：CLI 与 MCP 都要以独立进程驱动前台应用，但现有桥接是为外部 Python 子进程设计的 stdio 管道，不是控制面。
5. 触发器不是单个对象，而是一族机制：用户规则、内置检测（URL/IP/Hex/时间戳/压缩包）、OneKey 登录、Modem 自动、登录脚本。当前有两套并存的内核（已接线的 `term_core::trigger` + `term_app::trigger::Matcher`，与未接线的 `term_core::automation`），以及三份模式编译器和一份三元素方法白名单 `DISPATCHABLE_METHODS`。
6. OneKey 的会话期 `expect → send` 使用泛化的 `password` 模式，经常匹配到非登录请求并误触发；原版软件上这是已知缺陷。
7. 同一输出（例如 `password:`）可被用户触发器与 OneKey 同时命中，两处各自 `Tab::write`，造成双重应答；并发外部连接写同一会话也没有仲裁。

## 决策

### 1. 第一原理：等待接收 → 触发执行

所有**自动化**都是同一条链：

```
登记判据 → 等待接收信号 → 命中 → 产出 Effect → 派发执行
register → receive → match → effect → dispatch
```

- **拉式等待**：调用方线程阻塞在登记上，命中后唤醒自己（`wait` / `wait_any` / `wait_idle` / `read_until`）。脚本与 API 共用同一套语义。
- **推式等待**：宿主持续喂信号，无人阻塞，命中后派发 Effect（规则、OneKey 连接期认证、登录脚本）。
- **按需检测**：事件到来才扫当前事实（悬停时的 URL/IP/Hex 检测）。

三者共用 `Pattern` / `ScanCursor` / 清洗文本，**不共用调度**。即时动词（`write` / `keys` / `screen` / `session_list` / `fs_*` / `app_command` 等）不走反应器，直接执行。

内核三条不变式：

1. **先登记后消费**：登记 ack 必须先于登记点之后到达的输出被匹配；推式规则必须在同一帧 feed 之前装入。
2. **零墙钟在内核**：冷却 / 空闲判定用注入的 `Ticks`；`Instant` 与 deadline 留在调用方适配器。
3. **观察式与消费式分开**：`ScanCursor::probe` 不推进游标（推式、悬停），`take` 推进游标（拉式读取），两者读同一 buffer 的不同指针，类型层面互不吞数据。

### 2. 前台唯一进程与入口

- 会话池只有一个持有者：前台 GUI 进程。`SessionPool` 与标签的关系沿用 ADR-0025（会话为真身，标签为投影，`active: Option<SessionId>`）。
- **删除**：`term-desk script run`、`desk serve`、`desk --headless --exec`。不存在任何自持会话池的无窗口进程。
- **保留**：`doctor` / `capability` 这类纯自检子命令——它们不创建会话、不驱动终端。
- **CLI / MCP**：`apps/desktop/src/cli/` 下的子命令，同一个可执行文件的另一种启动模式。启动时不初始化窗口 / GPU / 事件循环，只作为外部客户端连接前台实例的本地控制端点。前台未运行时明确报错退出，**绝不自己起一个池**。
- 「操作必须在前台」的精确含义：**操作目标必须是前台实例持有的、用户可见的会话**；不要求窗口有输入焦点。锁屏门保留（锁屏时拒绝接入与调用）。

### 3. 分层：唯一动词层

```
L0 词汇   term-core::surface::METHODS —— 动词名/schema/类别/可见性（唯一真相）
L1 语义   term-core::automation —— Pattern/ScanCursor/Evaluator/Rule/Effect
          term-script::ScriptSession —— 拉式等待的语义核
L2 契约   HostEnv(七端口) + invoke(method, params) + Capabilities/Cancel + 仲裁器
          —— 动词只在这里实现一次；副作用只从这里出口
L3 实现   apps/desktop 的 DeskXxx 端口实现（UI 帧队列、vault、命令派发）
L4 装配   按前端装配 HostEnv：脚本 / API / 规则；按可见性过滤动词
L5 前端   GUI · Rhai 引擎 · 规则信号注入 · JSON-RPC(UDS) · CLI · MCP
```

- `Surface` trait 与 `SurfaceError` 从 `term-bridge` 上移到 `term-core`（纯 trait + 数据，零 IO）；`term-bridge` 降级为**传输适配器**，只保留编解码、分派与管道。
- `invoke` 是唯一的动词实现处。`DeskSurface::call` 与 Rhai 的 `register_fn` 都退化成薄壳，调用同一个 `invoke`。
- 捕获游标按会话一份 `ScriptSession`，归属在 L1/L2 的共享工作区；per-connection 的 `HostEnv` 只持有引用。因此脚本与 API 读写同一会话时共享同一游标，而不是各持一份。
- `describe` 仍由 `METHODS` 生成，是 CLI / MCP / 规则校验共用的一份自省答案。客户端不得硬编码方法名。
- `Exposure` 只做 L4 的可见性过滤，不产生第二套语义。纯脚本绑定 / 交互上下文的动词（`session_use` / `ask` / `notify` 等）保持 Script-only；UI 控制类动词（`session_active` / `session_activate` / `session_info` / `app_command` / `app_state` / `app_catalog` / `app_panel_set` / `app_layout`）对 API 可见——它们走端口，不新开写路径。

### 4. 契约、授权与生命周期

- `HostEnv` 七端口（会话 / 人机 / 本地文件 / 外部进程 / 剪贴板 / 窗口 / 应用控制）保持不变；`Capabilities` 与 `CancelToken` 属于一次运行。
- **脚本保留 caps**：脚本是可能来自外部的文件，`// @cap` + 界面勾选 / `--allow` 的按次授权是唯一还能挡一道的门。`Rhai` 是沙箱；**Python 完全可信**——它是独立进程，本身就能读文件，给它 caps 没有实际约束。
- **API 默认全开**：对已暴露给 API 的动词不做 caps 检查（授权边界是 UDS 连接本身）。这**不等于**把 Script-only 的本地副作用动词（`fs_*` / `proc_run` / `app_quit` / `window` / `clipboard_*`）暴露给 API；API 可见集合按 L4 显式过滤。
- **API 的 HostEnv 为 per-connection**：连接建立时装配一次，连接断开即取消（`CancelToken`）。
- `CredentialRef` 只能是宿主内部动作，**永不成为 API 动词**——否则等于提供「按方案名读密码」的接口。

### 5. 规则族统一与 OneKey 瘦身

- 只保留一套内核：`term-core::automation`。删除 `term_core::trigger` / `term_app::trigger::Matcher` / `term_app::trigger_queue` / `trigger_regex` / `DISPATCHABLE_METHODS`。旧数据不迁移。
- 术语定死：
  - **规则 Rule**：持久化数据（when / filter / guard / actions）；
  - **求值器 Evaluator**：纯函数，`Signal + Ticks → Effect`；
  - **反应器 Reactor**：宿主侧喂信号、派发 Effect 的运行时；
  - **触发器**：仅作 UI 产品名，不进架构描述。
- 内置检测（`trigger_builtins::detect`、`looks_like_prompt`）保持**纯检测函数**，不包装成规则；悬停是 pull，规则匹配是 push，两者共用检测函数而不共用调度。
- 规则动作四类正交，装载期校验：

  ```
  Action =
    | SessionInvocation { method, params }   // 引用 METHODS，校验存在性/schema/可见性
    | AppAction { Mark | Link | Menu | Tooltip | Alert | Dispatch }
    | CredentialRef  { scheme, field }       // 宿主内部，运行时从 vault 取
    | RunScript      { path, args }          // 运行时窄授权
  ```

  任何一项校验不过就在装载期拒绝，**绝不命中后变 NoEffect**。`RunScript` 不携带 `ScriptGrant`——把宽授权写进规则文件等于持久化后门；运行时用窄授权（只给会话动词）。
- `SendText` 的转义解码（`\x03` / `\003` / `\cc`）归到 `write` 动词，规则层不再做第二遍文本变换。`Alert::Beep` 走 `HumanChannel`（提醒用户），终端响铃需求用 `SessionInvocation(write, "\u{7}")` 显式表达。
- **OneKey 退化为纯连接期凭据方案**：保留 `username` / `identity_files` / `keyboard_interactive` / `enabled`；删除 `expects` / `ExpectSend` / `ExpectFlags` / `expect_matches` 及输出帧应答块。密码与键盘交互答案存 vault 条目 `onekey:<name>` 与 `onekey:<name>:ki`。连接期认证链（`term_core::auth`）保持最高优先级；需要终端层登录自动化时，用带 `CredentialRef` 的普通规则，并配 `once = Connection` + prompt 门控。

### 6. 仲裁：所有权开关 + 优先级

- 仲裁是**事前互斥**，不做事后去重：每条会话同时只有一个自动化所有者（owner），只有 owner 产生写意图。
- 优先级（高 → 低）：

  | 级别 | 源 | 说明 |
  |---|---|---|
  | P0 | 用户键盘 | 不进队列；任何按键立即抢占并释放现有独占 |
  | P1 | 连接期认证（auth 链 + vault 凭据） | 阶段独占写 |
  | P2 | 显式调用（脚本 / API） | 仅 `exclusive: true` 时抢 owner；默认只进帧内排序 |
  | P3 | 用户规则 | 默认 owner |
  | P4 | 内置被动（高亮 / URL / IP） | 只读，不抢 owner |

- **求值与写分离**：所有源的匹配照常并行（纯函数、只读 buffer）；被压制的只是落向 PTY 的写意图，记 `suppressed` 留痕。若连求值都停掉，规则会错过 `AfterConnect` 这类一次性事件。
- 同帧多个写意图按 `(priority, seq)` 排序后一次 flush 到 PTY 唯一漏斗；不同文本的 `SendText` 绝不拼接。
- 冲突处置：显式调用拿到显式错误（`Busy` / `Suppressed`），用户规则丢弃并写状态行，被动检测只是不显示菜单；认证期动作处于 P1，不被压制。所有压制都计数留痕。
- 不变式：同一「会话 + 提示」一次连接最多一个应答者；认证期写独占；用户键盘优先；观察与等待不进仲裁器；冲突显式失败或留痕；独占在超时 / 取消 / 断连时释放。

### 7. 传输与安全

- 本地控制端点用 Unix domain socket `~/.zterm/control.sock`（Windows 用命名管道），权限 0600，加 peercred 校验。整个仓库只有一处监听绑定。
- **删除 token 文件与 `auth` 握手**：0600 socket 已挡住跨用户；同用户进程本来就能读 0600 token，token 不增加任何屏障。协议第一条即普通方法。
- **保留 stdio 桥**：`python_run.rs` 用 `term_bridge::python_api_shim()` 起外部 Python 子进程，是产品功能；与 UDS 一样只是 `invoke` 的一个传输适配器。
- 锁屏门保留：接入时与每次调用前判定，结果缓存 2 秒；探测失败放行并记录审计行。

### 8. 兼容

软件处于开发设计阶段，**不做历史兼容**：旧 `prefs.json` / vault 条目不迁移；旧的两套规则内核直接删除；旧 ADR 按篇标注取代关系。`scripts/verify.sh` 中关于第二可执行目标、`tools/cli`、无头路径的检查项同步改写为新的判据。

## 后果

**正面**

- 动词只实现一次，游标只归属一处，集合相等断言不再承担防漂移的主要责任。
- API 与脚本走同一条通路，远程不再比本地脚本权力更大；授权模型可解释。
- 触发器族收敛为一个内核、一套动作 schema、一条仲裁路径；OneKey 误触发源被删除。
- 前台唯一进程消除了「通过 API 开的标签不在窗口里」的整类问题。

**负面**

- 改动面很大：`term-bridge` 的依赖方向、能力面可见性、UI 端口接入、规则数据、shell 子命令删除都互相牵连。
- 默认 owner 为规则，意味着显式调用不声明 `exclusive` 时仍可能与规则同帧写；靠排序与留痕而非硬隔离。
- 删除 `script run` 后，命令行批处理入口消失；CI 与排障改由单测 / 集成测试承担。
- API 全开的前提是「连得上 UDS 即用户本人授权」；这个假设在共享同一用户账户的机器上不成立，属于已接受的风险。

**判据**

- 全仓库只有一处 `SessionPool` 的持有者（前台进程），无自持池的子命令。
- `Surface` / `SurfaceError` 在 `term-core`；`term-bridge` 运行时依赖表不含会话实现。
- 方法名只在 `METHODS` 出现；`describe`、脚本注册、规则校验都从它派生；`DISPATCHABLE_METHODS` 不存在。
- 同一会话的脚本游标与 API 游标是同一份实例。
- `onekey::expects` 与输出帧 OneKey 应答块不存在；凭据只从 vault 取。
- 同一 `password:` 提示同帧命中 OneKey 凭据规则与用户规则时，只写一次。
- 未声明 `exclusive` 的 API 调用与用户规则同帧写时，按优先级排序且不交错。
- `auth` 握手与 token 文件不存在；socket 为 0600 且校验 peercred。

**复查触发**

- 出现第二个自持会话池的进程 → 决策 2 重写。
- 需要跨机器访问 → 先写认证与审计方案，不得以任何形式把控制端点暴露到本机之外。
- API 需要调用本地副作用动词（`fs_*` / `proc_run`）→ 决策 4 重写，不能靠放宽可见性悄悄放行。
- 需要服务端主动推送事件 → 决策 1 与 7 重写（当前协议无订阅 / 无推送）。
- 出现第二个需要宿主实现的自动化内核 → 决策 5 重写。

## 备选方案与否决理由

| 方案 | 否决理由 |
|---|---|
| 保留 `desk serve` 作为无头后端 | 与「操作必须在前台」直接冲突；两个进程各自持池，API 开的标签用户看不见 |
| 保留 `script run` 自持池 | 同上；且它无法解锁保险库，密码会话本来就打不开 |
| CLI / MCP 放进 `tools/cli` 独立仓库 | 同一可执行文件多模式即可，独立仓库会重新长出第二个宿主 |
| 用 span 重叠 / 提示分类器做事后去重 | 要造第四套模式编译器；改用事前所有权开关更简单可预测 |
| 把 OneKey expects 提到高优先级 | 泛化 `password` 模式误触发；直接废弃，改用带凭据引用的普通规则 |
| API 与脚本统一按 caps 授权 | 客户端已由 UDS + 显式配置准入，再加一层协商只增加配置面；脚本文件仍需 caps |
| 把 `fs_*` / `proc_run` 暴露给 API | 等于给本地控制端点开任意文件读与进程执行；API 全开只针对已暴露动词 |
| token 文件保留 | UDS 0600 已挡跨用户；同用户进程能读 0600 token，第二道门无效 |
| 两套触发器内核长期并存 | 两个 matcher、两份规则数据、两份动作枚举必然漂移；开发阶段直接删旧 |
| 规则里携带 `ScriptGrant` | 把宽授权写进规则文件等于持久化后门；授权只属于一次运行 |

## 落地顺序

1. `Surface` / `SurfaceError` 上移 `term-core`，`term-bridge` 改为纯适配器；`invoke` 收口。
2. 删除旧触发器内核与 `DISPATCHABLE_METHODS`，`automation` 成为唯一内核；规则动作四类 + 装载期校验。
3. OneKey 删 `expects`，Account 页改画已有字段；vault 条目收敛到 `onekey:<name>` / `:ki`。
4. 仲裁器（per-session owner + 优先级）接入 PTY 写漏斗；求值与写分离。
5. UDS 控制端点接入前台启动路径；删除 `serve` / `script run` / headless exec / token。
6. `apps/desktop/src/cli/` 增加 `api` / `mcp` 薄子命令；tools 由 `describe` 生成。
7. 删旧文档与 `verify.sh` 旧检查项，更新 ADR 索引。
