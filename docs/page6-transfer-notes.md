# page/6 Sftp Transfer：本轮补齐与保留差异

本轮只做两件事：**递归 / 批量删除**与**传输基准补「大量小文件」载荷**。
不新增命令 ID、不改 `scripts/verify.sh`、不改 `xfer_queue` 的并行上限，
也不改其它 worker 负责的文件（`prefs.rs`、`wrap.rs`、`ui/overlays/center.rs`、
`xfer_run.rs` 及其 UI、`ui/mod.rs` 热点）。

## 1. 删除整棵树（真实操作）

- **计划层**：新增 `crates/term-sftp/src/delete.rs`。纯函数
  `plan_tree_delete(root, entries)` 把一棵树的清单（相对根路径）展开成
  `DeleteStep::{File, Dir}`：所有文件先删，目录按路径降序（子路径一定以父路径
  为前缀，降序即后序）删，最后补一刀删根节点。空清单也会得到「删根」一步。
  脏相对路径（`..`、绝对路径、空段、反斜杠、NUL）整棵拒绝，不猜服务端行为。
- **复用既有请求**：UI 侧的 `plan_tree_delete` 只把 `DeleteStep` 映射成已有的
  `OpRequest::RemoveFile` / `OpRequest::RemoveDir`，再经 `SftpHandle` 当前的
  `Request::RemoveFile` / `Request::RemoveDir` 发出；没有新请求类型、没有新的
  传输通道。后台仍是一条会话串行消费请求，删除步骤的发出顺序就是执行顺序。
- **显式确认，无法绕过**：右键「Delete tree」只调 `scan_tree` 并立起确认窗，
  一条删除都不发；清单到齐之前确认按钮是灰的。只有 `draw_filer_prompt` 收到
  `Confirm` 才调用计划函数并发送；扫描失败 / 清单过期 / 会话断开时一律不发。
  单测钉住「`plan()` 对 DeleteTree 拒绝」与「收件箱按根路径认领」。
- **清单来源**：复用 `walk::scan_remote_entries`（软链跳过、根自身不在清单中），
  所以「看到什么就删什么」与同步 / 树传输共用同一套遍历口径。

## 2. 基准：N 个小文件 put / get / tree-delete

- `scripts/transfer-bench.sh` 新增 `--small-count`（默认 128），仍不挂
  `verify.sh`，`verify_steps` 不变。
- `crates/term-sftp/examples/transfer-bench.rs` 增加一组 4 KiB × N 的载荷：
  逐文件 `put` / `get` 并做 FNV-1a 核对，再用**产品同款**
  `plan_tree_delete` + 既有 `remove_file` / `remove_dir` 做完 tree-delete，
  最后确认树根已不存在。报告新增独立小节，分别给 files/s、MB/s、ops/s
  （小文件组的瓶颈是每步一次往返，MB/s 不适合用来判回归）。
- 实测数据已写入 `docs/bench/transfer-report.md`（128 个 × 4 KiB，put 82.1
  files/s、get 308.7 files/s、tree-delete 2083.6 ops/s，校验全绿）。

## 3. 保留差异（本轮刻意不做）

- **并行化不做**：要真正并行的小文件传输必须改 `xfer_queue` 的
  `max_parallel`（现在冻结在 1）并同步设计文档；后台是一条会话串行消费
  （见 `background.rs` 模块头），填 2 只会画出假进度。本轮保持**串行契约**
  不变：`xfer_queue::POLICY.max_parallel`、其单测与 `docs/design/` 均未改动。
- **不提供「一条请求删整棵树」的服务端接口**：`RemoveDir` 仍然只删空目录；
  递归的语义完全在纯计划里，由调用方确认后逐条发。这是刻意保留的安全边界。
- **不新增命令 ID / 脚本端口**：删整棵树是文件面板右键的一条入口，不进
  命令表面；`verify.sh` 与 `facts.txt` 口径不动。
- **基准仍为人工运行**：与上一轮一致，不进 `verify_steps`；报告是本机回环
  Docker 对端的实测值，只在本机内有比较意义。
