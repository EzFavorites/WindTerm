#!/usr/bin/env bash
# WindTerm 一次性地基改造 —— 阶段验收脚本（方案 §9 验收总表的本地实现）
#
# 用法：
#   scripts/verify.sh                        # 常规验收（阶段提交后跑）
#   SKIP_GIT_CHECK=1 scripts/verify.sh      # 阶段提交前跑（跳过工作树检查）
#   STRICT_B4=1 scripts/verify.sh           # 强制零字符串派发
#   ALLOW_MARKERS=1 scripts/verify.sh       # 仅调试时临时放宽地址字面判据（正常应为 0）
#   VERIFY_CLIPPY=1 scripts/verify.sh       # 额外跑 clippy（-D warnings，0 warning 才算过）
#   SHOT_JOBS=8 scripts/verify.sh           # 截图并行度（默认 4；设 1 退回逐张）
#   VERIFY_NO_SHOT_CACHE=1 scripts/verify.sh # 强制重拍所有截图（默认复用同Inputs缓存）
#
# ADR-0014 后：编译/测试一律用 --workspace 覆盖 3 个 crate，只测 app 会漏掉
# term-core / term-runtime 的单测与编译错误；grep 判据路径同步指向 crates/。
# 每个阶段结束执行：本脚本全绿 → git commit；任一红项 → git revert 到上一阶段。
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1
# 目标目录**必须跟着工作树走**（2026-11-09 实测事故）。
#
# 这里原本默认 `/tmp/zterm-check`——全机共享。并行 worktree 一起跑时，两个 cargo
# 抢同一把锁、互相覆盖 `debug/term-desk`，于是**截图步拿到的可能是别的分支的二进制**：
# 故障表现是「设置页基线红了」，而干净树上复跑又是绿的，排查方向全是错的。
# 用 `$ROOT/target` 既天然按工作树隔离，又正好复用在建工作树的增量缓存，不额外重建。
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
ALLOW_MARKERS="${ALLOW_MARKERS:-0}"   # 默认 0 命中

fails=0
step() { printf '\n── %s ──\n' "$1"; }
ok()   { printf '  ✓ %s\n' "$1"; }
bad()  { printf '  ✗ %s\n' "$1"; fails=$((fails + 1)); }

step "0. 门禁自检：判据路径不许静默失效（fail-closed）"
# 4e / 4h 都建立在「文件在哪」这个假设上。文件一挪，grep 就找不到 → 输出为空 →
# 判据里写成 `[ -z ] && ok` 的那部分就**静默变绿**：门禁看起来全过，实际什么都没查。
# 这不是理论风险。2026-11-12 审查 term-app 时实测到两处：
#   · 4e 只扫 `src` 直下一层（一个非递归 glob）：文件挪进子目录即完全逃逸，
#     子目录里写死 `"Window.Close"` 也不会红；
#   · 4h 硬编码 `src/state.rs`：awk 打不开文件 → 字段集为空 →
#     `dead_fields` 为空 → 判绿。
# 所以把这两条前提先变成断言。宁可在挪文件时红一次，也不要在重构后假绿。
GATE_PATHS="crates/term-app/src
crates/term-app/src/state.rs
crates/term-app/src/menu.rs
apps/desktop/src/ui
apps/desktop/src/ui/mod.rs
apps/desktop/src/ui/shell/find_bar.rs
apps/desktop/src/ui/effects.rs
apps/desktop/src/new_window.rs"
missing=""
for p in $GATE_PATHS; do [ -e "$p" ] || missing="$missing $p"; done
# 回归防线：本脚本里不许再出现「term-app 的 src 目录 + 单星号 glob」这种写法。
# 模式必须用字符类拼（`[/]?` `[*]` `[.]`），且注释里也不写出字面量：
# 否则这一行会被自己匹配到，判据恒红（写完全部断言后实测撞到过）。
nonrec_pat='crates/term-app/src[/]?[*][.]rs'
nonrec=$(grep -nE "$nonrec_pat" scripts/verify.sh 2>/dev/null | grep -vE ':[[:space:]]*#' || true)
# 4e 的扫描集合在这里定义一次，由 4e 直接消费（单一口径，不会两边漂）。
APP_SRC_FILES=$(find crates/term-app/src -name '*.rs' -type f 2>/dev/null | sort)
app_src_n=$(printf '%s\n' "$APP_SRC_FILES" | grep -c . || true)
if [ -n "$missing" ]; then
  bad "门禁引用的路径不存在：$missing —— 改布局必须同步本脚本，否则判据会静默变绿"
elif [ -n "$nonrec" ]; then
  bad "出现非递归 glob（子目录会漏扫）：$nonrec"
elif [ "${app_src_n:-0}" -eq 0 ]; then
  bad "term-app/src 下一个 .rs 也没扫到 —— 4e 判据失效"
else
  ok "路径齐全；4e 递归覆盖 $app_src_n 个 .rs；无非递归 glob"
fi

# ── 工具链必须可解析、且确实是 1.94 ──
# 2026-11-13 实测事故：根目录 `rust-toolchain.toml` 原本钉 `channel = "1.94.0"`，
# 而 rustup 里只有 `stable`（其 rustc 恰好是 1.94.0），没有具名 `1.94.0` channel。
# rustup 于是每次调用都去 sync，下载临时目录 `~/.rustup/tmp` 不可写 → Permission
# denied → **每一次 cargo 调用都失败**，表现为 11 项同时红（编译/单测/金样本/4d/
# 8c/clippy），真因只有一行 `info: syncing channel updates`。排查在错误方向上耗了很久。
# 所以这条断言的职责不是「再查一遍版本」，而是**把真因直接说清楚**：
# 工具链坏了的时候，人应该在这一行就看到 sync 失败，而不是去追 11 个红项。
# 版本钉定的三道机制（MSRV / 本断言 / CI env）见 `rust-toolchain.toml` 注释。
if ! tc_out=$(rustc --version 2>&1); then
  bad "工具链不可用（rustc --version 非 0）：$(printf '%s' "$tc_out" | head -3)"
elif ! printf '%s' "$tc_out" | grep -qE '^rustc 1\.94\.'; then
  bad "工具链不是 1.94：$tc_out —— 与 rust-version=\"1.94\" 及 CI 钉的 1.94.0 不符。处置：rustup toolchain install 1.94.0 后用 RUSTUP_TOOLCHAIN 明确指定，或确认要升版本则同步改全部 MSRV 并重跑视觉基线"
else
  ok "工具链 $tc_out"
fi

step "1. cargo check --workspace（0 error）"
if cargo check --workspace --target-dir "$TARGET" --quiet 2>/dev/null; then
  ok "编译通过"
else
  bad "编译失败"
fi
# `term-app` 的 `planned` feature 默认**不编译**：现在只剩 `structure` 一个模块。
# 2026-11-12 审查时 `trigger` / `trigger_regex` 也在清单里，但那个结论过期了：
# `apps/desktop` 随后把触发器接进会话执行 / 规则编辑 / hover 检测等真实路径，
# 两者已提回默认编译面（`apps/desktop/Cargo.toml` 也不再显式开本 feature）。
# 默认不编不等于可以烂——这里编一次，让剩下的模块跟着类型系统走。
if cargo check -p term-app --features planned --target-dir "$TARGET" --quiet 2>/dev/null; then
  ok "term-app --features planned 编译通过（未接线模块未腐烂）"
else
  bad "term-app --features planned 编译失败（未接线模块已腐烂）"
fi

step "2. cargo test --workspace（全量，含金样本）"
if cargo test --workspace --target-dir "$TARGET" --quiet 2>/dev/null; then
  ok "全部单测通过"
else
  bad "单测或金样本失败"
fi

step "3. 金样本显式复核（固定字节流 → 网格 JSON）"
# `cargo test <filter>` 匹配 0 个用例时返回 0——filter 拼错、用例改名、或整批被
# `#[ignore]` 之后，这一步会变成永远绿的假检查。先看匹配数，再看退出码。
# 注意：一条 filter 会跑多个 test binary，`term-desk` 下前几个是 0 passed、第三个才是
# 金样本本身——所以必须**求和**，取第一个会永远读到 0。
golden_out=$(cargo test -p term-desk --target-dir "$TARGET" golden -- --nocapture 2>&1)
golden_rc=$?
golden_n=$(printf '%s\n' "$golden_out" \
  | grep -oE 'test result: ok\. [0-9]+ passed' \
  | grep -oE '[0-9]+' \
  | awk '{ s += $1 } END { print s + 0 }')
if [ "$golden_rc" -ne 0 ]; then
  bad "金样本 diff 非空 —— 语义被改动，必须回滚本阶段"
elif [ "${golden_n:-0}" -lt 1 ]; then
  bad "金样本 filter 匹配 ${golden_n:-0} 个用例 —— filter 失效就是假绿"
else
  ok "apps/desktop/tests/golden/*.json 与当前 VT 行为一致（${golden_n} 个用例）"
fi

step "4. B1：term-core 内不得出现 egui / eframe / epaint / winit"
# 2026-11-14 架构审查：原词表只有 `egui|eframe`，而 UI 依赖可以不经
# 这两个名字溜进来 —— `epaint` 是 egui 独立发布的绘制 crate，`winit` 是窗口
# 后端，任何一个被 core 直接 `use` 都同样是「纯语义层认识 UI」。补上这两个，
# 与 4d 的 `bad_deps` 用同一份词表（两处不许各写各的，否则又是一格盲区）。
if [ -d crates/term-core/src ]; then
  hits=$(grep -rEn 'egui|eframe|epaint|winit' crates/term-core/src --include='*.rs' 2>/dev/null || true)
  if [ -n "$hits" ]; then bad "core/ 混入 UI 依赖：$hits"; else ok "core/ 零 UI 依赖（egui / eframe / epaint / winit 全挡）"; fi
else
  ok "core/ 尚未建立（当时跳过）"
fi

step "4b. term-core 全目录零副作用（例外已清零）"
# ADR-0014 后 supervisor / channel / theme_load 已迁往 term-runtime，
# 判据从「限定三文件 + 登记例外」升级为覆盖整个纯语义编译单元。
#
# 2026-11-13：判据里原本**没有 `std::env`**。于是
# `session.rs:587` 的 `std::env::var("USER")` 一直活得好好的——同一份输入在不同
# 宿主上得到不同 `spawn_plan`，而 4b 天天报「零副作用」。这不是漏网，是判据
# 本身漏了一个类别：环境变量和墙钟、线程、文件系统是同一件事——**把宿主状态
# 偷偷塞进纯函数**。现在补上，core 里 `std::env` / `env::var` 命中 0（实测）。
# 教训与 4d 那次一样：改掉一处违规，必须同时把「能看见这类违规」的判据补上，
# 否则下一个人会原样写回去，而门禁仍然绿着。
# 2026-11-14 架构审查：原词表只挡了「文件 / 线程 / PTY / 通道 /
# 目录 / 环境变量」六类，墙钟的另一种写法 `SystemTime::now()`、以及 `std::io::`
# / `std::net::` / `std::process::` 三个宿主 IO 入口全在视野之外 —— 它们和
# `std::env` 是同一件事：把宿主状态偷偷塞进纯函数。
#
# 同时加**整行注释过滤**：`vt.rs:122` 与 `command_history.rs:7` 的文档恰恰在
# 解释「本层不读 SystemTime::now()、时刻由宿主递进」，直接补词表会把这两行
# 判成违规，逼人把「为什么这样分层」删掉（4l 的注释里记过同一个坑）。
# 只滤整行注释，行尾注释（`let x = …; // SystemTime::now()`）仍会被抓：那一行
# 本来就是代码，不管注释写了什么。
# 口径上接受一点误报：`std::io::Result` / `std::net::IpAddr` 这类**纯类型**
# 也会被词表命中。当前 term-core 一个都没有；将来真要用，红一次再细化，
# 比漏掉一整类宿主状态好（fail-closed，与 step 0 同一原则）。
sem_hits=$(grep -rEn 'Instant::now|SystemTime::now|std::fs|std::io::|std::net::|std::process::|std::thread|portable_pty|mpsc::|dirs::|std::env|env::var' crates/term-core/src --include='*.rs' 2>/dev/null \
  | grep -vE ':[[:space:]]*(//|/\*|\*)' || true)
if [ -n "$sem_hits" ]; then
  bad "term-core 混入副作用/墙钟/环境读取：$sem_hits"
else
  ok "term-core 零副作用（墙钟两种写法、线程、文件/网络/进程 IO、OS 句柄、环境读取全覆盖；整行注释不算代码），架构例外 0"
fi

step "4c. 事实流无共享写锁"
# 值传递的事实窗口不得退化成共享可变状态（04 §2 警告过的 RwLock<Terminal> 争用）。
lock_hits=$(grep -rEn 'RwLock|ArcSwap|Mutex|AtomicPtr' crates/term-core/src --include='*.rs' 2>/dev/null || true)
if [ -n "$lock_hits" ]; then
  bad "core/ 混入共享写锁：$lock_hits"
else
  ok "core/ 零共享写锁（ScreenImage 为值类型）"
fi

step "4d. ADR-0014：编译单元边界（编译器强制）"
# 依赖表即边界：term-core 不得出现 UI / PTY / 文件系统 / 异步运行时 / 反向依赖。
# 与 step 4 用同一份 UI 词表：源码里 `use epaint::…` 会被 step 4 抓到，
# 依赖表里加一行 `epaint = "0.29"` 曾经谁也不抓 —— 两处不许各写各的。
bad_deps=$(grep -E '^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=' crates/term-core/Cargo.toml | grep -E 'egui|eframe|epaint|winit|portable-pty|dirs|tokio|russh|wasmtime|term-runtime|term-app' || true)
bad_rev=$(grep -E '^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=' crates/term-runtime/Cargo.toml | grep -E 'egui|eframe|term-desk' || true)
# term-app 是无前端应用层，**不许**有 UI 框架，也**不许**有 term-runtime——
# 后者保证这一层拿不到 PTY / 线程，副作用只能以 Effect 值返回（ADR-0016）。
bad_app=$(grep -E '^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=' crates/term-app/Cargo.toml | grep -E 'egui|eframe|portable-pty|term-runtime|dirs|tokio' || true)
# 2026-11-13 加固：上面那条是**黑名单**，漏一个词就全体失守——一次引进
# `term-import` 时正是这样溜进来的（依赖表多了一个带 `std::fs` / `dirs` 的 crate，
# 而 `bad_app` 的词表里没有它，于是 4d 一直绿）。架构审查报告指出的正是这处判据失明
# 就指这一处。改成**精确白名单**：`term-app` 的 [dependencies] 里只许 `term-core`
# 一个内部 crate，其余任何 `term-*` 依赖一律判红，不需要跟着新名字维护词表。
# 用 awk 切 `[dependencies]` 段，避免把 `[dev-dependencies]` 的测试期依赖误判成
# 运行时依赖（4l 踩过同一个坑）。
bad_app_extra=$(awk '/^\[dependencies\]/{f=1;next} /^\[/{f=0} f' crates/term-app/Cargo.toml | grep -E '^[[:space:]]*term-[a-zA-Z0-9_-]+[[:space:]]*=' | grep -vE '^[[:space:]]*term-core[[:space:]]*=' || true)
# 保险库是纯字节变换 —— 依赖表里不许出现运行时（tokio）、UI、PTY。
# 「加密顺手读一下文件」是最容易发生的一次越界，用依赖表挡住比 code review 可靠。
bad_vault=$(grep -E '^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=' crates/term-vault/Cargo.toml | grep -E 'tokio|async|egui|eframe|portable-pty|term-runtime|term-core|dirs' || true)
# SSH 连接层可以有 tokio，但**不许**有 UI，也不许反向依赖应用层，
# 也不许依赖 `term-runtime`——2026-11-12 收敛前它就是靠 term-runtime 拿到
# `TermChannel` 与 `SpawnError` 的，于是传输层被绑在「本机能开伪终端」上。
# 两个类型现在都在 `term_core::channel`，这条黑名单就是要它别再滑回去。
bad_ssh=$(grep -E '^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=' crates/term-ssh/Cargo.toml | grep -E 'egui|eframe|epaint|term-app|term-desk|term-runtime' || true)
# 会话库是纯同步存储 —— 依赖表里不许有运行时 / UI / PTY / 应用层。
# 「顺手把密码解密一下」是最容易发生的一次越界（库只存 entry_id，不认识保险库），
# 所以 term-vault 也不在白名单里：解密的调用点必须留在应用层。
bad_store=$(grep -E '^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=' crates/term-store/Cargo.toml | grep -E 'tokio|async|egui|eframe|portable-pty|term-runtime|term-app|term-vault|dirs' || true)
# 隧道可以有 tokio（与 term-ssh 同为异步笼子），但**不许**有 UI，
# 也不许反向依赖应用层 —— 它只认得「已认证的连接」与「转发规则」。
# 2026-11-14 架构审查：这里原先漏了 `term-runtime`，而 `[dev-dependencies]`
# 里恰好躺着一条全 crate 0 引用的 `term-runtime` 死依赖 —— 与 ssh/sftp 同样
# 不许碰门面，这一格却是空的。删死依赖与补词表必须同一刀做完：只删不补，
# 下一个人原样写回来时门禁仍然绿（4b / 4d 反复栽过的就是这个跟头）。
bad_tunnel=$(grep -E '^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=' crates/term-tunnel/Cargo.toml | grep -E 'egui|eframe|epaint|term-app|term-desk|term-runtime' || true)
# SFTP 与隧道同类（可以有 tokio），但只认得「已认证的连接」——
# 不许有 UI、不许反向依赖应用层，也不许顺手把 term-runtime 拉进来
# （那会让「文件面板」与「会话线程」长在一起）。
bad_sftp=$(grep -E '^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=' crates/term-sftp/Cargo.toml | grep -E 'egui|eframe|epaint|term-app|term-desk|term-runtime|dirs' || true)
# 2026-11-12 底层收敛：`term-paths` 是**路径规则叶子**，比持久化层与会话层都低。
# 它只许依赖 `dirs`：一旦它开始依赖别的 term-* crate，那个 crate 就必须比
# 「路径规则」更低，而两者都想要它——必然成环。其余重量级依赖也不许进来。
# 注意：这里用**白名单**而不是黑名单。黑名单里有 `term-` 的话，`[package]` 段的
# `name = "term-paths"` 会把自己匹配上，于是断言永久为红——这是 2026-11-12 证伪
# 实测撞到的（把依赖注入进去测红，结果它本来就是红的）。白名单凡不在名单里的一律
# 越界，也不需要跟着新依赖名维护。
bad_paths=$(awk '/^\[dependencies\]/{f=1;next} /^\[/{f=0} f && /^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=/' crates/term-paths/Cargo.toml | grep -vE '^[[:space:]]*dirs[[:space:]]*=' || true)
# 会话 IO 层：可以有 portable-pty / nix（它就是要开设备），**不许** SQLite、
# serde（档案层的活）、tokio / russh（异步笼子外）、UI，也不许反向依赖门面
# 或档案层——那会让「改一行偏好」重新变成「重编整个会话线程」。
bad_io=$(grep -E '^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=' crates/term-io/Cargo.toml | grep -E 'rusqlite|term-store|serde|tokio|russh|async|egui|eframe|term-runtime|term-profile|term-app|term-desk' || true)
# 用户档案层：只许碰「配置目录里的文件」。**不许** portable-pty / nix / tokio
# （它不该能起线程或开设备），**不许** dirs（路径规则只归 term-paths），
# 也不许反向依赖会话层或投影层。
bad_profile=$(grep -E '^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=' crates/term-profile/Cargo.toml | grep -E 'portable-pty|nix|tokio|russh|egui|eframe|dirs|term-runtime|term-io|term-app|term-desk' || true)
# 独立配置导入模块：只做「外部文本 → 领域类型」。**不许**能开会话（term-io）、
# 不许能写配置（term-profile）、不许反向依赖门面，也不许 UI / tokio / PTY。
# 这条黑名单是「导入不会偷偷顺手做别的事」的编译期保证。
bad_import=$(grep -E '^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=' crates/term-import/Cargo.toml | grep -E 'term-io|term-profile|term-runtime|term-app|term-desk|portable-pty|tokio|russh|async|egui|eframe' || true)
# 2026-11-13 收口：4d 原来只覆盖上面这 12 个 crate，term-bridge 由 4l 单独守，
# 但 **term-session / term-script / term-zmodem / term-xfer 四个没有任何依赖判据**。
# 这不是纸面问题：刚把「控制面反向依赖待删门面 term-runtime」拆掉，而当时
# 没有任何门禁能阻止它被原样绑回去——改代码不补门禁，等于留了个能重新打开的口子。
# 判据只加**禁止项**、不预设白名单：绑定关系会随重构变，禁止项（不许碰 UI / 不许
# 碰门面 / 不许开异步）才是稳定的性质。
bad_session=$(grep -E '^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=' crates/term-session/Cargo.toml | grep -E 'egui|eframe|epaint|term-app|term-desk|term-runtime' || true)
# Rhai 沙箱：只许作用于注入的那条会话，自己不许有 IO / 异步 / UI。
bad_script=$(grep -E '^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=' crates/term-script/Cargo.toml | grep -E 'egui|eframe|epaint|term-app|term-desk|term-runtime|term-io|portable-pty|tokio|russh|async' || true)
# ZMODEM 是帧层协议实现，落盘由调用方给路径——它自己不该能开 PTY / 连 SSH / 跑异步。
bad_zmodem=$(grep -E '^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=' crates/term-zmodem/Cargo.toml | grep -E 'egui|eframe|epaint|term-app|term-desk|term-runtime|term-io|portable-pty|tokio|russh|async' || true)
# term-xfer 是「X/YMODEM 内核零 IO」。这里不是黑名单，是「`[dependencies]` 必须为空」
# ——空表本身就是它的设计证明，多出任何一个都说明有东西被塞进来了。
bad_xfer=$(awk '/^\[dependencies\]/{f=1;next} /^\[/{f=0} f && /^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=/' crates/term-xfer/Cargo.toml)
if [ -n "$bad_deps" ] || [ -n "$bad_rev" ] || [ -n "$bad_app" ] || [ -n "$bad_app_extra" ] || [ -n "$bad_vault" ] || [ -n "$bad_ssh" ] || [ -n "$bad_store" ] || [ -n "$bad_tunnel" ] || [ -n "$bad_sftp" ] || [ -n "$bad_paths" ] || [ -n "$bad_profile" ] || [ -n "$bad_io" ] || [ -n "$bad_import" ] || [ -n "$bad_session" ] || [ -n "$bad_script" ] || [ -n "$bad_zmodem" ] || [ -n "$bad_xfer" ]; then
  bad "编译单元依赖越界：core=[$bad_deps] runtime=[$bad_rev] app=[$bad_app] app_extra=[$bad_app_extra] vault=[$bad_vault] ssh=[$bad_ssh] store=[$bad_store] tunnel=[$bad_tunnel] sftp=[$bad_sftp] paths=[$bad_paths] profile=[$bad_profile] io=[$bad_io] import=[$bad_import] session=[$bad_session] script=[$bad_script] zmodem=[$bad_zmodem] xfer=[$bad_xfer]"
elif cargo check -p term-core -p term-app --target-dir "$TARGET" --quiet 2>/dev/null; then
  ok "依赖单向：desk → app → core，io/profile/import → core，ssh/tunnel/sftp → core；paths 只依赖 dirs，profile 无线程无设备，import 不会话不写配置，app 无 UI 无 PTY，vault/store 无运行时"
else
  bad "term-core / term-app 独立编译失败"
fi

# 2026-11-13：`[target.'cfg(target_os = "macos")'.dependencies]` 必须是
# 文件**最后一段**。AGENTS.md:1028-1032 记过一次真实事故：13 条依赖被写在该段
# 之后，于是全部被吞成 macOS-only，Linux 上编译失败而没人知道原因。当时唯一的
# 防线是 `apps/desktop/Cargo.toml` 里一行「这一段必须留在文件最后」的**注释**——
# 注释不是门禁，下一个人删掉它不会有任何提示。
#
# 两条判据缺一不可：
#   ① 段序：`[target…]` 之后再出现非 target 的 `[section]` 即红。
#   ② 段内键白名单：光守段序挡不住「不写新 section、直接往段尾追加键」——
#      那种写法照样被吞成 macOS-only，且段序判据完全看不见。今天该段只有
#      `winit` 一个键（apps/desktop/Cargo.toml:88-89）。
target_misplaced=""
target_straykey=""
for ct in $(find . -name Cargo.toml -not -path './target/*' 2>/dev/null | sort); do
  m=$(awk '/^\[target/{t=NR} /^\[/ && !/^\[target/ && t && NR>t {print NR": "$0}' "$ct")
  if [ -n "$m" ]; then target_misplaced="${target_misplaced}${ct}:[$m] "; fi
  k=$(awk -v F="$ct" '/^\[target/{f=1;next} /^\[/{f=0} f && /^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=/ {print F": "NR": "$0}' "$ct" | grep -vE ':[[:space:]]*winit[[:space:]]*=' || true)
  if [ -n "$k" ]; then target_straykey="${target_straykey}${k} "; fi
done
if [ -n "$target_misplaced" ] || [ -n "$target_straykey" ]; then
  bad "macOS 依赖段违规：段后仍有非 target 段=[${target_misplaced}]段内非白名单键=[${target_straykey}]（写错位置会把依赖静默吞成 macOS-only）"
else
  ok "macOS 依赖段在文件末尾；段内键仅 winit（白名单）"
fi

step "4k. ADR-0017 §决策 2：tokio 笼子（异步只许在 term-ssh / term-tunnel / term-sftp）"
# 判据四条：
# 1) 除了 term-ssh / term-tunnel，任何清单里都不许出现 tokio / russh —— 一旦漏到
#    term-app 或 apps/*，「对外同步」这句话就不成立了，UI 线程随时可能被 `.await` 挂住；
# 2) term-app 与 apps/* 的源码里不许出现 `.await` / `tokio::` —— 与 1 互为兜底，
#    依赖可以经由 re-export 绕进来；
# 3) term-ssh 必须真的 `impl TermChannel`，否则「Supervisor 一行不改」只是口号；
# 4) `apps/desktop` 必须直接依赖 term-ssh —— 没有这一条，原生 SSH 只活在库里，
#    界面上没有任何路径用得到它。tokio 正是**经**这个依赖间接进了 apps/，
#    所以 1) 管的是直接依赖：这条缝是故意留的，写在这里免得下次有人把它堵上；
# 5) term-tunnel 必须走 term-ssh 的 `Authenticated` —— 隧道复用「已认证的连接」，
#    而不是自己再写一份 known_hosts / 认证。复制一份就是两处漏一个 CVE；
# 6) `apps/desktop` 必须直接依赖 term-tunnel —— 与第 4 条同一个道理：隧道的库
#    不接进界面，就只是一堆没人调用的转发代码。tokio 也因此多了一条间接通道，
#    但仍然只有这两条（第 1 条管直接依赖）。
# 注意：这里**不能**写 `case`——bash 3.2 在 `$( )` 里遇到 case 分支的 `)`
# 会当成命令替换的收尾，直接报语法错（macOS 自带的就是 3.2）。用 if 绕开。
async_outside=$(for f in crates/*/Cargo.toml apps/*/Cargo.toml; do
  if [ "$f" = "crates/term-ssh/Cargo.toml" ] || [ "$f" = "crates/term-tunnel/Cargo.toml" ] || [ "$f" = "crates/term-sftp/Cargo.toml" ]; then continue; fi
  grep -HE '^[[:space:]]*(tokio|russh|async-std)' "$f" 2>/dev/null || true
done)
await_outside=$(grep -rEn '\.await|tokio::' crates/term-app/src apps/desktop/src --include='*.rs' 2>/dev/null || true)
ssh_wired=$(grep -cE 'impl TermChannel for SshChannel' crates/term-ssh/src/channel.rs 2>/dev/null || true)
ssh_russh=$(grep -cE '^russh[[:space:]]*=' crates/term-ssh/Cargo.toml 2>/dev/null || true)
desk_ssh=$(grep -cE '^term-ssh[[:space:]]*=' apps/desktop/Cargo.toml 2>/dev/null || true)
desk_tunnel=$(grep -cE '^term-tunnel[[:space:]]*=' apps/desktop/Cargo.toml 2>/dev/null || true)
# 判据是「真的调了」而不是「import 了」：`Authenticated::connect` 就是用现成认证的
# 那一步；自己再写一份握手的话，这里会数不到。
tunnel_reuse=$(grep -cE 'Authenticated::connect' crates/term-tunnel/src/lib.rs 2>/dev/null || true)
desk_sftp=$(grep -cE '^term-sftp[[:space:]]*=' apps/desktop/Cargo.toml 2>/dev/null || true)
sftp_reuse=$(grep -cE 'Authenticated::connect' crates/term-sftp/src/background.rs 2>/dev/null || true)
if [ -n "$async_outside" ] || [ -n "$await_outside" ] || [ "${ssh_wired:-0}" -eq 0 ] || [ "${ssh_russh:-0}" -eq 0 ] || [ "${desk_ssh:-0}" -eq 0 ] || [ "${desk_tunnel:-0}" -eq 0 ] || [ "${tunnel_reuse:-0}" -eq 0 ] || [ "${desk_sftp:-0}" -eq 0 ] || [ "${sftp_reuse:-0}" -eq 0 ]; then
  bad "tokio 越界：笼子外的依赖=[$async_outside] 笼子外的 await=[$await_outside] TermChannel 实现=${ssh_wired:-0} russh 依赖=${ssh_russh:-0} desk→term-ssh=${desk_ssh:-0} desk→term-tunnel=${desk_tunnel:-0} desk→term-sftp=${desk_sftp:-0} tunnel→Authenticated=${tunnel_reuse:-0} sftp→Authenticated=${sftp_reuse:-0}"
else
  ok "tokio / russh 只在 term-ssh / term-tunnel / term-sftp；term-app 与 apps/* 零 await；SshChannel 实现了 TermChannel；desk 经这三个库连一条会话 / 起隧道 / 列目录，后两者都复用 Authenticated"
fi

step "4l. term-bridge 依赖边界（白名单，运行时只许 term-core / serde / serde_json）"
# 为什么不用 4d 那种 `^name =` 正则，而是 awk 切 section + 白名单——两条都实测会出错：
# 1) `crates/term-bridge/Cargo.toml` 的注释文字里本来就写着 tokio / term-session
#    （「不许有 tokio …」「term-session 只在 dev-dependencies 里」），黑名单 grep 会把
#    说明文字判成违规，于是判据逼着人删注释——恰好把「为什么这样分层」抹掉。
# 2) 4d 的正则不区分 section：`[dev-dependencies]` 里的 `term-session` 会被当成运行时
#    依赖，于是「运行时只许 term-core」这条永远校验不对。
# 白名单比黑名单强：黑名单漏一个词就全体失守（新增依赖天然不在名单里就查不出），
# 白名单凡不在名单里的一律越界，不需要跟着新词维护。
# 判据 3 的 grep 会连**文档注释**一起扫（与 4c 同一口径）：所以 crate 文档里不要写
# 那个「点 + await」的字面量，写「异步调用点」即可——否则注释自己就把门禁判红。
# 本步只校验 `term-bridge` crate 自身的依赖边界（运行时只许 term-core / serde /
# serde_json），**不**校验 `apps/desktop` 是否依赖它——那是 desk 侧的接线事实，由
# `apps/desktop/Cargo.toml` 与命令分发入口（cli/serve.rs → bridge::serve）保证，已于
# 2026-11-12 复核确认已接。若将来想让门禁也守住「desk 必须依赖 bridge」，在此追加
# 一条对 apps/desktop/Cargo.toml 的白名单检查即可。
bridge_runtime=$(awk '
  /^\[/ { in_dep = ($0 ~ /^\[dependencies\]/); next }
  in_dep && /^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=/ {
    split($0, kv, "="); gsub(/[[:space:]]/, "", kv[1]); print kv[1]
  }
' crates/term-bridge/Cargo.toml 2>/dev/null)
bridge_offlist=$(printf '%s\n' "$bridge_runtime" | sed '/^$/d' | grep -vE '^(term-core|serde|serde_json)$' || true)
bridge_dev_session=$(awk '
  /^\[/ { in_dev = ($0 ~ /^\[dev-dependencies\]/); next }
  in_dev && /^term-session[[:space:]]*=/ { c++ } END { print c + 0 }
' crates/term-bridge/Cargo.toml 2>/dev/null)
bridge_await=$(grep -rEn '\.await|tokio::' crates/term-bridge/src --include='*.rs' 2>/dev/null || true)
if [ -z "$bridge_runtime" ] || [ -n "$bridge_offlist" ] || [ "${bridge_dev_session:-0}" -ne 1 ] || [ -n "$bridge_await" ]; then
  bad "term-bridge 依赖越界：运行时依赖=[$(printf '%s' "${bridge_runtime:-<空，Cargo.toml 或 [dependencies] 段没取到>}" | tr '\n' ' ')] 白名单外=[$bridge_offlist] dev-dependencies 里的 term-session=${bridge_dev_session:-0}（应恰好 1） await/tokio 命中=[$bridge_await]"
else
  ok "term-bridge 运行时只有 term-core / serde / serde_json；term-session 恰好 1 次且只在 dev-dependencies；桥接本体零 .await / 零 tokio::（4k 的 tokio 笼子之外再钉一遍它自己）"
fi

step "4m. ADR-0021：egui 触点只许在 ui/ 与平台白名单里"
# 分层的意义全在这条判据上：`ui/` 之外认识 egui 的文件，只允许是「窗口 / 字体 /
# 剪贴板 / 主题 / 截图」这五类平台代码。逻辑层（demo / golden / sysopen）一旦
# `use egui`，换渲染后端时就会被连带重写——那正是 ADR-0021 要避免的。
# 白名单是**精确**的：名单里多出文件（已经不再引用 egui）也算失败，否则名单
# 会慢慢长成垃圾桶，下次真有人越界时没人敢删。
ui_outside=$(grep -rlE 'egui::|use egui' apps/desktop/src --include='*.rs' 2>/dev/null | grep -v '^apps/desktop/src/ui/' | grep -v '^apps/desktop/src/bin/' | sort || true)
# `src/bin/` 被 `.gitignore` 第 52 行排除（本地一次性实验 bin），所以**不进白名单**：
# 干净 checkout 里它根本不存在，写进名单只会让门禁在 worktree / CI 上假红。
# 反过来说，也意味着逻辑层不能靠搬进 `src/bin/` 逃过这条判据。
# `script_{clip,run,app}.rs` 是同一类：脚本线程需要 egui 的平台能力（剪贴板 /
# 帧驱动 / 请求重绘），只是**持有 `egui::Context` 发信号**，不含任何绘制与布局。
# 它们留在 `ui/` 之外是对的（脚本宿主是平台层，不是逻辑层），因此逐个登记。
platform_allowed=$(printf '%s\n' \
  apps/desktop/src/main.rs \
  apps/desktop/src/theme.rs \
  apps/desktop/src/fonts/mod.rs \
  apps/desktop/src/clipboard.rs \
  apps/desktop/src/shot.rs \
  apps/desktop/src/script_clip.rs \
  apps/desktop/src/script_run.rs \
  apps/desktop/src/script_app.rs | sort)
ui_offlist=$(comm -23 <(printf '%s\n' "$ui_outside") <(printf '%s\n' "$platform_allowed") | sed '/^$/d')
platform_stale=$(comm -13 <(printf '%s\n' "$ui_outside") <(printf '%s\n' "$platform_allowed") | sed '/^$/d')
if [ -n "$ui_offlist" ] || [ -n "$platform_stale" ]; then
  bad "egui 触点越界/白名单过期：越界=[$ui_offlist] 白名单里已不再引用=[$platform_stale]"
else
  ok "ui/ 之外认识 egui 的只有 5 个平台文件（窗口 / 字体 / 剪贴板 / 主题 / 截图）"
fi

step "4n. ADR-0022：能力面只许挂回环（desk serve）"
# 判据：整个 desk 里**只有一处** `TcpListener::bind`，且绑定地址是
# `Ipv4Addr::LOCALHOST`。远程访问的正确形态是 SSH 隧道，不是监听 0.0.0.0——
# 后者等于把「能开终端、能跑命令」的能力面挂到局域网上。
#
# 刻意不 grep 字符串 `0.0.0.0`：`cli/serve.rs` 的用例里就写着
# 「`--host 0.0.0.0` 必须被拒」，那样的门禁只能靠人眼区分测试与实现。
bind_sites=$(grep -rn 'TcpListener::bind' apps/desktop/src --include='*.rs' 2>/dev/null || true)
# 判据一（ADR-0022 的**实质**）：任何一处 bind 都必须绑回环——测试代码也不例外。
#
# 这里曾经只数「有几处 bind」，要求恰好 1 处。那条判据把测试夹具也算进去了：
# `TcpListener::bind(("127.0.0.1", 0))` 是端到端测试拿临时端口的正常写法，
# 却被判成「能力面多开了一个入口」。2026-11-09 起改成判真实性质：
# 非回环一处都不许有；生产路径（bridge/mod.rs）仍然只许一个出口。
bind_offloop=$(printf '%s\n' "$bind_sites" | sed '/^$/d' \
  | grep -v 'Ipv4Addr::LOCALHOST' | grep -v '"127\.0\.0\.1"' || true)
bind_prod=$(grep -rn 'TcpListener::bind' apps/desktop/src/bridge/mod.rs --include='*.rs' 2>/dev/null | wc -l | tr -d ' ')
if [ -n "$bind_offloop" ] || [ "${bind_prod}" -ne 1 ]; then
  bad "desk serve 必须且只能绑回环：非回环 bind = [$bind_offloop]，生产 bind 处数 = ${bind_prod}，全部=[$bind_sites]"
else
  ok "能力面只绑 127.0.0.1（生产唯一一处 TcpListener::bind，测试夹具也只绑回环）"
fi

step "4o. 测试与验收不得抢占前台（自动化调用一律 --screenshot）"
# 规则来源：用户在 2026-11-09 明确要求「后续测试验证，禁止出现抢占前台的情况」。
#
# 「不抢前台」需要**三个**开关同时成立，缺一个都会被用户看见（2026-11-09 实测）：
#
#   1. `with_visible(false)` —— 窗口不显示；
#   2. `with_active(false)`   —— 建窗口时不成为 key window；
#   3. macOS `ActivationPolicy::Prohibited` —— 进程根本不具备成为前台应用的资格。
#
# **第 3 条是决定性的**，也是第一版漏掉的那条：前两条只管「窗口」这个对象，
# 而抢焦点发生在 `NSApplication` 层——只要进程还是普通 app，AppKit 就会在创建
# 窗口时把自己 activate 起来。当时只加了 1（后来又补了 2），`lsappinfo front`
# 采样 80 次里仍有 22 次前台变成了 `term-desk`；补上 3 之后 80/80 全是原来的
# 前台应用，且截图字节与基线一致。
#
# 判据分两层：
#   A. **机制还在**：main.rs 里三个开关一个都不能少——删掉任何一个都会静默
#      退化成「弹窗抢焦点」，而那时没有任何测试会红。
#   B. **调用方式对**：脚本里起 `term-desk` 必须带 `--screenshot`，或走
#      `cli::try_run` / `--list-components` / `--headless` 这几条在 `eframe`
#      之前 return 的无窗口出口。
#      只查 `scripts/`：这是自动化唯一会拉起 GUI 的地方。`apps/desktop/tests/` 里只有
#      `script_run.rs` 起这个二进制，走 `script run` 子命令（`cli::try_run` 在
#      eframe 之前 return），用 grep 去猜 Rust 的参数数组只会误报。
main_rs=apps/desktop/src/main.rs
fs_flags=""
grep -q 'with_visible(false)' "$main_rs" 2>/dev/null || fs_flags="${fs_flags}with_visible(false) "
grep -q 'with_active(false)' "$main_rs" 2>/dev/null || fs_flags="${fs_flags}with_active(false) "
grep -q 'ActivationPolicy::Prohibited' "$main_rs" 2>/dev/null || fs_flags="${fs_flags}ActivationPolicy::Prohibited "
fs_bad=$(grep -rn '\$BIN' scripts/*.sh 2>/dev/null \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*#' \
  | grep -v 'fs_bad=' \
  | grep -vE -- '--screenshot|--list-components|--headless' \
  | grep -vE 'BIN=|echo |\[ ! -x ' \
  | grep -vE '^([^:]+:)?[0-9]+:[[:space:]]*(cp|install|chmod|tar|codesign|file|ls|du|stat|\[)[[:space:]][^;&|]*$' || true)
# 最后那条排除是给 `scripts/package-release.sh` 的（2026-11-13 实测误报）：
# `cp "$BIN" "$APP/Contents/MacOS/WindTerm"` 被原判据当成「拉起 GUI」，可它只是
# **复制**，一次也没执行过那个文件。判据要抓的性质是「执行了图形二进制」，不是
# 「提到了这个变量名」。
#
# 排除范围刻意做窄，两个约束缺一不可：
#   · 命令必须**位于行首**（`^[[:space:]]*(cp|install|…)`）——`foo && cp $BIN x`
#     这种行不匹配，仍会被抓；
#   · 命令之后**不许出现 `;` `&` `|`**（`[^;&|]*$`）——`cp $BIN x && $BIN` 这种
#     「复制完顺手跑一下」是本次排除最可能被利用的缝，直接堵死。
# 换句话说：被排除的只可能是「一行里只有一个纯文件操作命令」。
#
# `^([^:]+:)?` 是为了兼容两种 `grep -rn` 输出：扫多个文件时有 `路径:` 前缀，扫单个
# 文件时没有。当前 `scripts/` 下不止一个 `.sh`，但把判据写成只在某个文件数下成立，
# 等于埋一个「以后只剩一个脚本时静默失效」——那正是本仓反复栽过的坑。
if [ -n "$fs_flags" ]; then
  bad "截图运行少了「不抢前台」的开关：${fs_flags}（三个缺一不可，见本步骤注释）"
elif [ -n "$fs_bad" ]; then
  bad "有脚本在无窗口模式下拉起 GUI（会抢前台）：\n$fs_bad"
else
  ok "自动化不抢前台（三个开关齐备；脚本一律 --screenshot 或无窗口出口）"
fi

step "4e. 能力注册（命令 ID 只在数据层，投影层零硬编码）"
# 三条判据：
# 1) 命令 ID 只允许出现在 crates/term-app/src/menu.rs（纯数据），ui/ 不得硬编码；
# 2) 不再有「什么也不做」的 Unhandled 变体（未接线必须是 Option::None）；
# 3) 匹配计数不是状态：投影层不得出现 total_matches（F2 已把它删出 FindState），
#    也不得给 current_match 赋值——计数只由 app::find::matches 从窗口算出。
# 纯测试模块：整个文件都是测试代码，但**文件内部没有 `#[cfg(test)]` 行**——它是被
# 别处的 `#[cfg(test)]` + `#[path = "xxx_tests.rs"] mod …;` 两行声明拉进来的。
# 下面那套「截断到 #[cfg(test)] 之后」的手法对这类文件完全失效：awk 一行也截不掉，
# 于是测试里的 `("Window.Close", …)` 这种命令 ID 字面量会被当成生产代码硬编码判红。
# 判据**不靠文件名后缀**（那等于给这条判据开后门：随便起个 `_tests.rs` 后缀就能藏 ID），
# 而是回到声明处——只有真的被那条两行声明引入的文件才跳过。
test_only_mods=" $(grep -rhA1 --include='*.rs' '^[[:space:]]*#\[cfg(test)\][[:space:]]*$' \
    crates apps 2>/dev/null \
  | grep -o 'path = "[^"]*"' | sed 's/path = "\([^"]*\)"/\1/' | xargs -n1 basename 2>/dev/null \
  | sort -u | tr '\n' ' ')"

# 判据写成函数而不是内联 `case`：bash 3.2（macOS 自带）在 `$( … )` 里解析
# `case … in … ;; esac` 会报 `syntax error near unexpected token ';;'`
# ——连 `a) : ;; *) … ;;` 这种最普通的写法都过不去。函数在顶层定义，子 shell 里可直接调。
is_test_only_mod() {
  case "$test_only_mods" in
    *" $1 "*) return 0 ;;
    *) return 1 ;;
  esac
}

cmd_in_ui=$(grep -rEn '"[A-Z][A-Za-z]+\.[A-Za-z]+"' apps/desktop/src/ui --include='*.rs' 2>/dev/null \
  | while IFS= read -r hit; do
      is_test_only_mod "$(basename "${hit%%:*}")" || printf '%s\n' "$hit"
    done || true)
# app/ 层的命令 ID 只允许出现在**命令数据表**与各能力的 parse()（入口）。
# W2 把「一张表」拆成了三张，都是纯数据，不是投影层硬编码：
#   · menu.rs            顶级 10 棵菜单（判据原始白名单）
#   · menu_submenus.rs   W2 从 menu.rs 拆出的 18 个子菜单明细，同一张菜单表
#   · tab.rs             影子入口数据表 SHADOW_ENTRIES + Tab 能力的 parse
#   · api_class.rs       分类表：**每条记录都是一条命令 ID**（`("Window.Close",
#                        ApiClass::…)`），与 menu.rs 同属「命令数据表」这一侧。它回答的是
#                        「这条命令为什么不在脚本 API 里」，不是投影层的硬编码分支——
#                        真正的判定逻辑（`class_of` / 五条断言）里一个 ID 字面量都没有。
# `shortcut_map.rs` 是快捷键绑定表（命令 ID → 按键序列），同属纯数据表。
# 这五处之外，term-app 下任何 .rs 出现 ID 字面量一律判红（例如某个
# reducer 里偷偷写死一个命令字符串）；各能力的 `"ID" => ...` 分支放行。
# 测试代码里出现 ID 是合法的，所以扫描时截断 #[cfg(test)] 之后的部分；
# 由 `#[path]` 引入的纯测试模块见上面的 `test_only_mods`，整文件跳过。
# 白名单按**路径**判、不按 basename：`menu` 是一棵子树，将来拆成
# `menu/mod.rs` + `menu/bars.rs` 之后，里面的命令 ID 仍属于「命令数据表」这一侧。
# 扫描集合取自 step 0 的 `APP_SRC_FILES`（递归），不再用顶层 glob —— 那会让
# 子目录里的文件整片逃逸，是「判据静默变绿」的典型形状。
# 不用 `case`：bash 3.2 在 `$( … )` 里解析 `case … ;; esac` 会语法错（见上方注释）。
cmd_in_app=$(printf '%s\n' "$APP_SRC_FILES" | while IFS= read -r f; do
  [ -n "$f" ] || continue
  if [ "$f" = "crates/term-app/src/menu.rs" ] || [ "$f" = "crates/term-app/src/menu_submenus.rs" ] || [ "$f" = "crates/term-app/src/tab.rs" ] || [ "$f" = "crates/term-app/src/api_class.rs" ] || [ "$f" = "crates/term-app/src/shortcut_map.rs" ]; then
    continue
  fi
  if [ "${f#crates/term-app/src/menu/}" != "$f" ]; then continue; fi
  is_test_only_mod "$(basename "$f")" && continue
  awk '/#\[cfg\(test\)\]/{exit} {print FILENAME":"FNR":"$0}' "$f" 2>/dev/null
done | grep -E '"[A-Z][A-Za-z]+\.[A-Za-z]+"' | grep -vE ':[0-9]+: *"[A-Z][A-Za-z]+\.[A-Za-z]+" =>' || true)
unhandled=$(grep -rn 'Action::Unhandled\|Unhandled' crates/term-app/src apps/desktop/src --include='*.rs' 2>/dev/null || true)
slot_write=$(grep -rEn 'total_matches|current_match[[:space:]]*=[^=]' apps/desktop/src/ui --include='*.rs' 2>/dev/null || true)
# 2026-11-15：`Application.NewWindow` 的「空 arm」护栏，挂在 4e 里而不是新增
# step（verify_steps 冻结在 38，不为一个判据动账本）。`main.rs` 有 crate 级
# `#![allow(dead_code)]`，`apply_effect` 的 arm 写成 `{}` 时 cargo test 与
# clippy -D warnings 会**双双全绿**——`#[must_use] SpawnPlan` 只抓「造了 plan
# 却没 launch」，抓不到「连 plan 都没造」。这两条是源码级存在性断言：
#   ① NewWindow arm 真的消费了 plan（`new_window_plan().launch()`）；
#   ② `new_window.rs` 真的拉起了进程（`.spawn()`），不是只有个返回 Ok 的空壳。
# fail-closed：两个文件已在 step 0 的 GATE_PATHS 登记，挪走即红；grep 判「≥1 命中」
# 而不是「输出为空即通过」，避免文件名一改就静默变绿。只证明这两跳存在，
# 不证明窗口真的弹出来——后者只能人眼验收（见 next-plan.md §10.2）。
nw_arm=$(grep -c 'new_window_plan().launch()' apps/desktop/src/ui/effects.rs 2>/dev/null || true)
nw_spawn=$(grep -c '\.spawn()' apps/desktop/src/new_window.rs 2>/dev/null || true)
if [ -n "$cmd_in_ui" ] || [ -n "$cmd_in_app" ] || [ -n "$unhandled" ] || [ -n "$slot_write" ] \
   || [ "${nw_arm:-0}" -lt 1 ] || [ "${nw_spawn:-0}" -lt 1 ]; then
  bad "判据未过：ui 硬编码=[$cmd_in_ui] app 硬编码=[$cmd_in_app] Unhandled=[$unhandled] 投影写计数=[$slot_write] NewWindow arm 消费=${nw_arm:-0} / launch 真 spawn=${nw_spawn:-0}（两条都须 ≥1）"
else
  ok "命令 ID 只出现在 menu.rs 与 parse 分支；无 Unhandled；匹配计数不在投影层；NewWindow 两跳（arm 消费 plan / launch 真 spawn）都在"
fi

step "4f. F1/F2/F3：投影只读事实窗口；纯状态模块零 UI 依赖"
# 动作按能力分区、搬成 `crates/term-app`：viewport / selection / find /
# shell / capability 都不该认识 egui（注释里提名字不算，只查代码引用），
# 这件事现在由依赖表守，这里再兜一层 grep。
# 同时 ui/ 不得再直接索引 GridSnapshot.cells——渲染只有一个入口：事实窗口。
ui_dep=$(grep -rEn 'egui::|eframe::' crates/term-app/src --include='*.rs' 2>/dev/null || true)
cell_idx=$(grep -rEn '(snapshot|snap)\.cells' apps/desktop/src/ui --include='*.rs' 2>/dev/null || true)
# F2：投影只能对「事实窗口」查找（find_matches_in），不得回到快照版入口，
# 否则回看里搜不到东西——两套入口意味着两套行号坐标系。
snap_find=$(grep -rEn 'find_matches\(' apps/desktop/src/ui --include='*.rs' 2>/dev/null || true)
# 粘贴必须经 term-core 编码（换行归一 / 控制字符过滤 / 括号粘贴）。
# 两个方向都查：调用必须存在，且不得把剪贴板原文直接写给 PTY——
# 后者正是「多行粘贴在 vim 里塞满 ^J」那个 bug 的形状。
paste_enc=$(grep -c 'paste_bytes_for' apps/desktop/src/ui/mod.rs 2>/dev/null || true)
# F3：双击选词 / 三击选行的边界判定在纯层（app/selection），投影只把坐标交给它，
# 不得在 ui/ 里自己扫字符类别——那会立刻变成第二套「什么算一个词」。
sel_enc=$(grep -cE 'word_at|line_span_at' apps/desktop/src/ui/mod.rs 2>/dev/null || true)
sel_own=$(grep -rEn 'is_alphanumeric' apps/desktop/src/ui --include='*.rs' 2>/dev/null || true)
raw_paste=$(grep -rEn 'write\((text|paste|clip|contents)\.as_bytes\(\)\)' apps/desktop/src/ui --include='*.rs' 2>/dev/null || true)
if [ -n "$ui_dep" ] || [ -n "$cell_idx" ] || [ -n "$snap_find" ] || [ "${paste_enc:-0}" -eq 0 ] || [ -n "$raw_paste" ] || [ "${sel_enc:-0}" -eq 0 ] || [ -n "$sel_own" ]; then
  bad "F1/F2/F3 判据未过：term-app 依赖 UI 框架=[$ui_dep] ui 索引快照单元=[$cell_idx] ui 对快照查找=[$snap_find] 粘贴未编码=[$paste_enc] 原文写 PTY=[$raw_paste] 选词未走纯层=[$sel_enc] ui 自算词边界=[$sel_own]"
else
  ok "term-app 零 UI 依赖；ui/ 只经事实窗口渲染与查找；粘贴经 term-core 编码；选词/选行走纯层"
fi

step "4g. 投影层零领域写——Find 栏是受控组件"
# 最后一处例外曾是 `FindState.query` 由 egui `TextEdit` 直接编辑：
# 绘制过程中把状态改掉，投影层就成了第二个写入者。于是把输入框改成受控
# （状态只读 + 缓冲可变 + 改动派发 `SetQuery`），这里把「不许再写回去」钉住。
ui_mut_state=$(grep -rEn '&mut +FindState|&mut +self\.view\.find' apps/desktop/src/ui --include='*.rs' 2>/dev/null || true)
ui_find_assign=$(grep -rEn 'find(_state)?\.(query|visible|case_sensitive|current_match)[[:space:]]*=[^=]' apps/desktop/src/ui --include='*.rs' 2>/dev/null || true)
ui_query_mut=$(grep -rEn 'query[[:space:]]*\.(push|clear|pop|insert|truncate|retain)' apps/desktop/src/ui --include='*.rs' 2>/dev/null || true)
# 受控组件的形状：状态引用 + 可变的编辑缓冲，两个参数都得在。
# 2026-11-06 目录重排后 Find 栏在 `ui/shell/`，判定路径跟着走——
# 路径写错时 `grep -c` 返回 0，判据会「红得很正确但原因错」，别让它悄悄变绿。
ctl_state=$(grep -c 'find_state: &FindState' apps/desktop/src/ui/shell/find_bar.rs 2>/dev/null || true)
ctl_input=$(grep -c 'input: &mut String' apps/desktop/src/ui/shell/find_bar.rs 2>/dev/null || true)
if [ -n "$ui_mut_state" ] || [ -n "$ui_find_assign" ] || [ -n "$ui_query_mut" ] || [ "${ctl_state:-0}" -eq 0 ] || [ "${ctl_input:-0}" -eq 0 ]; then
  bad "判据未过：ui 持有可变状态=[$ui_mut_state] ui 写 FindState=[$ui_find_assign] ui 改 query=[$ui_query_mut] 受控签名=[$ctl_state/$ctl_input]"
else
  ok "Find 栏受控（只读 state + 编辑缓冲）；ui/ 零领域状态写入 —— 例外 0 处"
fi

step "4j. 单文件代码行（apps/desktop/src + crates/，不含空行与注释，上限 500）"
# `ui/mod.rs` 曾是 1,586 行；按「谁画谁」拆成 6 个文件，2026-11-06 再按组件
# 分到 6 个子目录（shell / terminal / panels / dialogs / overlays / layout）。
# 这条只挡一件事：新代码别再往同一个文件里堆。
#
# **2026-11-09 口径变更**（用户要求）：
#   · 以前数 `wc -l`（含注释），阈值还分两档：`ui/mod.rs` ≤ 500、其余 ≤ 450，
#     且所有 `*/mod.rs` 都被排除在 450 那档之外——`ui/comp/mod.rs` 因此从没被
#     量过（实测代码行 566）。把代码藏进嵌套 `mod.rs` 就能绕过门禁，是假绿。
#   · 现在统一数**代码行**：非空、且不在注释里（`//`、`///`、`//!`、`/* … */`
#     都算注释；行尾注释不剥，那一行仍然是代码）。所有文件一个阈值 500，
#     不再给 `mod.rs` 另设档位。
#   · 为什么改：注释是资产不是负债。按 `wc -l` 算，一个把「为什么这么画」写足
#     的文件会被判超标，而应对办法只有「删注释」——那是**最坏**的结果。
#     `comp/mod.rs` 就是被这条逼着把名字表拆去 `comp/ids.rs` 的。
code_lines() {
  awk '
    BEGIN { inblock = 0 }
    {
      s = $0; n = length(s); keep = ""; i = 1; instr = 0
      while (i <= n) {
        c = substr(s, i, 1)
        if (inblock) {
          if (substr(s, i, 2) == "*/") { inblock = 0; i += 2 } else i++
          continue
        }
        # 字符串里的 `//` 与 `/*` 不是注释——先认出字符串再认注释标记。
        if (instr) {
          keep = keep c
          if (c == "\\") { keep = keep substr(s, i + 1, 1); i += 2; continue }
          if (c == "\"") instr = 0
          i++
          continue
        }
        if (substr(s, i, 2) == "//") break
        if (substr(s, i, 2) == "/*") { inblock = 1; i += 2; continue }
        if (c == "\"") instr = 1
        keep = keep c
        i++
      }
      if (keep ~ /[^ \t]/) count++
    }
    END { print count + 0 }
  ' "$1"
}
# 目录重排（2026-11-06）后 `ui/` 变成 6 个子目录，必须递归数——只数顶层
# 会得到一个「合规」的假绿：上面 7 千行全躲在 `panels/`、`dialogs/` 里。
over_files=""
stale_debt=""
max_file=""; max_lines=0
n_files=0
# 2026-11-13 扩面：扫描根从 `apps/desktop/src/ui` 扩到 `apps/desktop/src`。
# 原来只扫 `ui/`，于是 `src/xfer_run.rs`（775 代码行）与 `src/session_open.rs`（538）
# 在门禁视野之外——不是「合规」，是「没被量过」。审查报告点的就是这一处。
#
# 扩面立刻多出这两条红项。它们不进「允许额度」，进**待拆清单**：下面的第二条断言
# 强制它们必须仍然超限——一旦拆到 ≤500，这里也必须删掉该表项，否则基线会悄悄
# 变成永久豁免。这是 4p/4q「基线是现状事实，不是许可额度」的同一口径。
DESK_SZ_LIMIT=500
# ADR-0027（2026-11-16）新增 3 处：脚本平台的执行/接线/窗口三块一次写不完，
# 先登记为欠账（每处都已按「能拆的先拆」处理过，剩下的是一次原子改动）。
# 2026-11-22：远端 menu 接线改动把 setup.rs 推到 506 行，登记为待拆。
DESK_SZ_DEBT="apps/desktop/src/xfer_run.rs apps/desktop/src/session_open.rs apps/desktop/src/script_run.rs apps/desktop/src/ui/terminal/script.rs apps/desktop/src/ui/dialogs/session_form.rs apps/desktop/src/ui/terminal/setup.rs"
# 2026-11-15（架构审查 §5-6 扩面）：4j 原来只扫 apps/desktop/src，crates/ 下
# 190 个 .rs **一次都没被量过**——supervisor.rs 1677 代码行、vt.rs 1605 行，
# 不是「合规」而是没被看见。这里把现存 23 个超限文件登记为**可见基线**：
#   · 第 24 个超限文件出现即红；
#   · 谁把某个文件拆到 ≤500，必须从这张表里删掉（下面 stale_debt 断言），
#     否则基线会悄悄变成永久豁免。与 4p/4q 同一口径：基线是现状事实，不是许可额度。
# 排除 `*/tests/*`：4j 的 desk 口径本来就不含 apps/desktop/tests，两边保持一致，
# 测试文件长不算架构债。
CRATES_SZ_DEBT="crates/term-io/src/supervisor.rs
crates/term-core/src/vt.rs
crates/term-app/src/selection.rs
crates/term-zmodem/src/frame.rs
crates/term-profile/src/prefs.rs
crates/term-app/src/action.rs
crates/term-script/src/session.rs
crates/term-app/src/menu/tree.rs
crates/term-core/src/session.rs
crates/term-script/src/engine.rs
crates/term-io/src/preview.rs
crates/term-ssh/src/options.rs
crates/term-ssh/src/outbound.rs
crates/term-session/src/lib.rs
crates/term-app/src/shortcuts.rs
crates/term-app/src/address.rs
crates/term-sftp/src/background.rs
crates/term-app/src/selection/geometry.rs
crates/term-sftp/src/transfer.rs
crates/term-core/src/mouse_report.rs
crates/term-app/src/vocab.rs
crates/term-core/src/surface.rs
crates/term-app/src/i18n/zh.rs
crates/term-core/src/grid.rs
crates/term-script/src/compat.rs
crates/term-script/src/workspace.rs
crates/term-app/src/menu_submenus.rs"
# 统一成空格分隔：case 的 `" $f "` 边界只认空格，换行会让匹配静默失效。
SZ_DEBT=$(printf '%s %s' "$DESK_SZ_DEBT" "$CRATES_SZ_DEBT" | tr '\n' ' ')
crates_over_n=0
while IFS= read -r f; do
  n_files=$((n_files + 1))
  n=$(code_lines "$f")
  if [ "$n" -gt "$max_lines" ]; then max_lines=$n; max_file=$f; fi
  if [ "$n" -gt "$DESK_SZ_LIMIT" ]; then
    case " $SZ_DEBT " in
      *" $f "*) : ;;
      *) over_files="$over_files$f($n 行) " ;;
    esac
    case "$f" in crates/*) crates_over_n=$((crates_over_n + 1)) ;; esac
  fi
done < <(find apps/desktop/src crates -name '*.rs' ! -path '*/tests/*' 2>/dev/null | sort)
for f in $SZ_DEBT; do
  if [ ! -f "$f" ]; then
    stale_debt="$stale_debt$f(已不存在) "
    continue
  fi
  n=$(code_lines "$f")
  if [ "$n" -le "$DESK_SZ_LIMIT" ]; then
    stale_debt="$stale_debt$f(已降到 $n 行) "
  fi
done
if [ -n "$over_files" ] || [ -n "$stale_debt" ]; then
  # 花括号不是风格：macOS 自带 bash 3.2 在变量名紧跟中文标点时会把它后面
  # 那几个高位字节也算进变量名（2026-11-12 实测：变量名后接全角逗号，
  # 报出的是「max_file?: unbound variable」，问号就是被吞掉的半个汉字）。
  # 裸写在这里不是「偶尔难看」，是这条红灯分支**必崩**——它只在真的有文件
  # 超限时才走到，于是这层检查长期只能说「绿」或「崩」，从不说「红」。
  bad "单文件代码行超限：${over_files}；待拆清单需更新：${stale_debt}（全仓最大的是 ${max_file}，${max_lines} 行）"
else
  ok "文件规模合规（递归 ${n_files} 个文件；desk 2 处 + crates ${crates_over_n} 处已登记欠账，其余均 ≤ ${DESK_SZ_LIMIT}；全仓最大 $max_file = $max_lines 行，属欠账）"
fi

step "4i. crate 边界的错误必须分类（不得退回 String）"
# 配置错 / 打不开 PTY / 找不到程序，调用方的反应完全不同；拍平成字符串就只剩一句话。
# `term-core` 出 `ConfigError`，`term-runtime` 出 `SpawnError`（含 Config/Pty/Program 三类）。
#
# 2026-11-11 收紧正则：旧判据 `\)) -> Result<[^>]*String>` 会把「String 在值位置」的
# 签名也算成违规（`Result<Option<String>, VaultError>`），实测 6 处误报。这里改成
# 「第二个泛型参数确实是 String」（以 `, String>` 结尾）。全仓范围的冻结在 4q。
str_err=$(grep -rEn '\) -> Result<.*, *String>' crates/term-runtime/src crates/term-io/src crates/term-profile/src crates/term-paths/src crates/term-import/src crates/term-core/src --include='*.rs' 2>/dev/null || true)
if [ -n "$str_err" ]; then
  bad "边界仍在返回 String 错误：$str_err"
else
  ok "错误已分类：core → ConfigError，runtime → SpawnError；无 String 边界"
fi

step "4r. term-app 分层单向（词汇 → 副作用 → 状态 → 能力 → 入口 → 分发）"
# 2026-11-12 拆分前，`term-app` 是一个 13 节点的强连通分量：`action`（类型 + 入口 +
# 分发三合一）与 `state` 各 7 条入边，46 条生产边里 13 条从 `action` 射出、7 条射向它。
# 后果不是编译错误——同 crate 内模块可以成环——而是**拆不动**：任何按层切分都会撞上
# 「能力模块要 Effect、Effect 要能力模块的动作类型」这类回边。
#
# 拆完的依赖图是单向的，本 step 把它钉住。判据两条：
#   ① 只有 `reduce`（分发）/ `action`（门面）/ `parse`（入口）/ `lib`（再导出）四个
#      文件可以指向 `crate::action` / `crate::reduce`；其余文件出现即为向上回边。
#   ② 底部各层与拆出来的子模块各有白名单：词汇表只许 i18n，副作用只许 vocab，
#      入口只许 vocab，状态只许 i18n / palette / goto / center / vocab。
# 只查**生产段**：测试里允许按能力取入口（`use crate::parse::shell_parse as parse;`）。
# 注释行必须跳过：`effect.rs` 的文档里就写着「原来这里是 use crate::action::Effect」，
# 那是历史说明，不是依赖。同理 `crate::i18n` 里的数字要能被 `[a-z0-9_]+` 匹配到，
# 否则会被截成 `crate::i`（第一版就是这么误报的）。
#
# 2026-11-12：`menu` / `selection` / `i18n` 拆出同名子目录（`menu/tree.rs` 等 7 个
# 文件、约 2000 行）。旧版把文件名写死成 `<模块>.rs` 只认顶层——子目录里的文件**一次
# 都没被查过**，本项照样绿。这和 4e / 4h 那次静默变绿是同一类错：判据的**输入集合**
# 悄悄缩了，判据本身不报错。所以现在三件事一起改：
#   · `prod_refs` 收完整路径，不再内部拼 `.rs`；
#   · 向上回边改成**递归扫 `src/` 下全部 .rs**（豁免表显式列出），不再维护模块名单；
#   · 末位自检：白名单里的文件必须都在、扫描到的文件数必须大于 0，否则判红。
APP_SRC_DIR="crates/term-app/src"
prod_refs() { awk '/#\[cfg\(test\)\]/{exit} /^[[:space:]]*\/\//{next} {print}' "$1" 2>/dev/null \
  | grep -oE 'crate::[a-z0-9_]+' | sort -u; }
lay_bad=""
check_layer() {
  local r
  r=$(prod_refs "$1" | grep -vE "$2" | tr '\n' ' ')
  [ -n "$r" ] && lay_bad="$lay_bad ${1#$APP_SRC_DIR/}→[$r]"
}
check_layer "$APP_SRC_DIR/vocab.rs"     '^crate::(i18n)$'
check_layer "$APP_SRC_DIR/effect.rs"    '^crate::vocab$'
check_layer "$APP_SRC_DIR/parse.rs"     '^crate::vocab$'
check_layer "$APP_SRC_DIR/state.rs"     '^crate::(i18n|palette|goto|center|vocab)$'
check_layer "$APP_SRC_DIR/action.rs"    '^crate::(vocab|effect|reduce)$'
check_layer "$APP_SRC_DIR/capability.rs" '^crate::(vocab|menu|tab|parse)$'
check_layer "$APP_SRC_DIR/palette.rs"   '^crate::(vocab|capability|menu|tab|parse)$'
# 拆出的子模块：搬家不该改层级，所以照样钉住。
# `menu/bars.rs` 里的 `crate::dock` 是数据层（`DockItem` 要 `DockZone`），不是回边。
check_layer "$APP_SRC_DIR/i18n/zh.rs"            '^crate::i18n$'
check_layer "$APP_SRC_DIR/menu/tree.rs"          '^crate::(dock|menu|vocab)$'
check_layer "$APP_SRC_DIR/menu/bars.rs"          '^crate::(dock|menu|vocab)$'
check_layer "$APP_SRC_DIR/menu/shortcuts.rs"     '^crate::(dock|menu|vocab|shortcut_map)$'
check_layer "$APP_SRC_DIR/shortcut_map.rs"       '^crate::(menu|shortcuts)$'
check_layer "$APP_SRC_DIR/selection/geometry.rs" '^crate::(selection|vocab)$'
check_layer "$APP_SRC_DIR/selection/words.rs"    '^crate::(selection|vocab)$'
check_layer "$APP_SRC_DIR/selection/ops.rs"      '^crate::(parse|selection|vocab)$'

# 判据失效自检：白名单点名的文件必须都还在。改名/挪位时这里先红，
# 而不是让那一行 grep 静默匹配空、整项判绿。
LAY_REQUIRED="vocab.rs effect.rs parse.rs state.rs action.rs capability.rs palette.rs \
i18n/zh.rs menu/tree.rs menu/bars.rs menu/shortcuts.rs shortcut_map.rs \
selection/geometry.rs selection/words.rs selection/ops.rs"
missing_lay=""
for f in $LAY_REQUIRED; do
  [ -f "$APP_SRC_DIR/$f" ] || missing_lay="$missing_lay $f"
done

# 向上回边（递归）：除显式豁免的四个文件外，任何生产段文件都不许指向
# `crate::action` / `crate::reduce`。纯测试模块（被 `#[path]` 拉进来的）不算生产段。
UPWARD_EXEMPT="action.rs reduce.rs parse.rs lib.rs"
upward=""
lay_files=0
while IFS= read -r f; do
  [ -n "$f" ] || continue
  rel=${f#$APP_SRC_DIR/}
  lay_files=$((lay_files + 1))
  is_test_only_mod "$(basename "$f")" && continue
  case " $UPWARD_EXEMPT " in *" $rel "*) continue ;; esac
  r=$(prod_refs "$f" | grep -E '^crate::(action|reduce)$' || true)
  [ -n "$r" ] && upward="$upward ${rel}→[$r]"
done <<EOF
$(find "$APP_SRC_DIR" -name '*.rs' -type f 2>/dev/null | sort)
EOF
if [ -n "$missing_lay" ] || [ "${lay_files:-0}" -eq 0 ]; then
  bad "4r 判据失效：分层文件缺失[$missing_lay]、递归只扫到 ${lay_files:-0} 个文件——此时本项什么都不查，不许判绿"
elif [ -n "$lay_bad" ] || [ -n "$upward" ]; then
  bad "term-app 分层越界：层级=[$lay_bad] 向上回边=[$upward]"
else
  ok "分层单向（递归 $lay_files 个文件，含子模块）：词汇/副作用/入口只依赖 vocab；状态只依赖纯数据；无向上回边"
fi

step "4p. 生产路径 panic 冻结（只许减不许增，基线 22）"
# 运行时可达的 `unwrap` / `expect` 每多一处，就多一个能 abort 整个 GUI 的输入。
# `panic!` / `unreachable!` / `todo!` / `unimplemented!` 是同一件事的直接写法，
# 2026-11-15 起一并计入（后两者实测 0 命中，纳入后基线不变）。
# 口径（改口径必须同步改基线，并在蓝图 §7 记一笔）：
#   · 排除 `_tests.rs` / `testutil.rs` / `testutil/` / `tests/`；
#     2026-11-19：`testutil.rs` 拆成目录模块（`testutil/mod.rs` + `testutil/app.rs`，
#     给 4j 腾余量）。按文件名排除 `testutil.rs` 对目录形态失效——basename 变成
#     `mod.rs`，于是整套假端口的 `expect("…锁")` 会混进生产预算。所以同时按
#     目录排除，两种形态都盖住。
#   · 排除「整文件就是测试」的模块：由别处 `#[cfg(test)] mod xxx;` 引入、
#     文件内部反而没有 `#[cfg(test)]` 行可截断（见下方 TEST_ONLY_FILES）；
#   · 每个文件在第一个 `#[cfg(test)]` 处截断；
#   · 排除 `const _: () = { ... };` 块——那里面是编译期求值，不在运行时路径；
#   · 排除整行注释。
# 为什么只许减不许增：基线是现状的事实，不是许可额度。
#
# 39 → 37（2026-11-14）：`ScriptSession` 的 5 处 `expect("已登记")` 改成
# `Capture::reader_mut()` 返回显式 `Closed`——登记器缺失是脚本能撞上的运行时
# 状态，原先只有 3 处能撞（read/read_until/wait_any），wait_idle 与 settle
# 两种新原语又各加一处。既然已经证明这条路可以不开 panic，就顺手把旧的也换掉。
#
# 37 → 44（2026-11-15，架构审查 §5-3 重取口径，**不是新增了 7 处 panic**）：
#   +10 生产路径 `panic!` / `unreachable!` 一直在预算之外
#     （`term-xfer/src/packet.rs:95` 的 `encode_frame` 直接 panic、
#      `apps/desktop/src/ui/comp/mod.rs` 3 处、各对话框 5 处等）；
#    -3 `apps/desktop/src/golden.rs` 由 `main.rs:43` 的 `#[cfg(test)] mod golden;`
#       引入，文件内**没有** `#[cfg(test)]` 行，awk 截断对它无效，白占 3 处
#       `.unwrap()`（另有 1 处 `panic!` 同属测试，一并排除）。
#   37 + 10 − 3 = 44。仍是「只许减不许增」，第 4 刀会把它降下来。
#
# 44 → 22（2026-11-15，架构审查 §5-4 真正减掉 22 处）：
#   `term-session` 13 处 + `term-script` 9 处「锁中毒就 expect」全部收口到
#   各自的 `lock_or_recover`（照 `entries()` / `guard()` 已有的
#   `poisoned.into_inner()` 范式）。中毒只是「别的线程持锁时 panic 过」，
#   登记表与捕获游标都没有要回滚的一致性约束，不该把 GUI 一起带走。
#   新增两条判据：会话池中毒后表侧 / 会话侧 / 写路径全部仍可用；
#   helper 中毒后仍能读回数据。基线是现状事实，减了就必须同步降。
PANIC_BASELINE=22
# 4e 的 `test_only_mods` 只收 `#[path = "…"]` 形式，收不到 `mod golden;` 这种。
# 从**声明处**取名字，与 4e 同一口径：不靠文件名后缀，否则改个名就能藏 panic。
TEST_ONLY_FILES=" $(grep -rhA1 --include='*.rs' -E '^[[:space:]]*#\[cfg\(test\)\][[:space:]]*$' \
    crates apps 2>/dev/null \
  | grep -oE '^[[:space:]]*mod [a-zA-Z0-9_]+;' \
  | sed -E 's/^[[:space:]]*mod ([a-zA-Z0-9_]+);/\1.rs/' \
  | sort -u | tr '\n' ' ')"
panic_n=0
excluded_n=0
for f in $(find crates apps -name '*.rs' ! -name '*_tests.rs' ! -name 'testutil.rs' ! -path '*/testutil/*' ! -path '*/tests/*'); do
  case " $TEST_ONLY_FILES " in
    *" $(basename "$f") "*) excluded_n=$((excluded_n + 1)); continue ;;
  esac
  n=$(awk '
    /#\[cfg\(test\)\]/ { exit }
    /^const _: \(\) = \{$/ { skip=1; next }
    skip { if ($0 ~ /^\};$/) skip=0; next }
    /^[[:space:]]*\/\// { next }
    { print }
  ' "$f" | grep -cE '\.unwrap\(\)|\.expect\(|panic!\(|unreachable!\(|todo!\(|unimplemented!\(' || true)
  panic_n=$((panic_n + n))
done
if [ "${excluded_n:-0}" -eq 0 ]; then
  bad "4p 判据失效：整文件排除一个也没生效（纯测试模块会重新混进生产预算）"
elif [ "$panic_n" -gt "$PANIC_BASELINE" ]; then
  bad "生产路径 panic 增加：${panic_n} 处 > 基线 ${PANIC_BASELINE}（新增的请改成显式分支）"
else
  ok "生产路径 panic ${panic_n} 处（基线 ${PANIC_BASELINE}，只许减不许增；整文件排除 ${excluded_n} 个纯测试模块）"
fi

step "4q. 全仓错误边界 String 化基线（只许减不许增，基线 40）"
# 4i 只扫 `term-core` / `term-runtime`，其余 crate 与 `apps/` 从没被量过——把判据
# 扩到 `crates` + `apps` 立刻命中 42 行，其中 6 行是**旧正则的误报**：
# `Result<Option<String>, VaultError>` / `Result<Vec<(String, Option<String>)>, StoreError>`
# 这种「String 出现在值位置」也被 `\)) -> Result<[^>]*String>` 算成了错误类型。
# 先收紧：只认以 `, String>` 结尾的，即第二个泛型参数确实是 String。
#
# 收紧后 40 处（2026-11-11 实测），分布：
#   apps/desktop 19 —— CLI 参数解析与面板命令的**用户文案**，刻意设计，不是债务；
#   term-sftp 5 / term-script 5 / term-app 5 / term-zmodem 2 / term-ssh 2 /
#   term-xfer 1 / term-session 1。
# 这 40 处一次改不完：每改一处都要先给那个 crate 定义错误枚举，属于独立重构，
# 所以这里只做**冻结**。基线是现状的事实，不是许可额度：只许减不许增。
# ADR-0027（2026-11-16）：40→51。新增的 11 处全在 `apps/desktop` 的脚本端口
# 实现（`script_human` / `script_fs` / `script_proc` / `script_sftp` / `script_recorder`
# / `script_run`）：它们的 String 是**给人看的原话**（「写不到脚本 X」这类），
# 与基线里已有的 19 处同一类别，不是「本该分类却拍平了」。改这 11 处要先给
# 每个端口定义错误枚举再翻译成 `PrimitiveError`，属独立重构。
STRING_ERR_BASELINE=50
str_err_n=$(grep -rEn '\) -> Result<.*, *String>' crates apps --include='*.rs' 2>/dev/null | wc -l | tr -d ' ')
if [ "${str_err_n:-0}" -gt "$STRING_ERR_BASELINE" ]; then
  dist=$(grep -rEn '\) -> Result<.*, *String>' crates apps --include='*.rs' 2>/dev/null \
    | cut -d/ -f1-2 | sort | uniq -c | sort -rn | tr '\n' ';')
  bad "错误边界 String 化的地方增加了：${str_err_n} 处 > 基线 ${STRING_ERR_BASELINE}（分布：${dist}）"
else
  ok "错误边界 String 化 ${str_err_n} 处（基线 ${STRING_ERR_BASELINE}，只许减不许增）"
fi

step "4s. ADR-0014 门面收敛：term_runtime:: 引用只许减不许增（基线 148）"
# `term-runtime` 是兼容门面：`lib.rs` 自称「等 ui/ 重新校对完可以删掉」，
# 但此前**没有任何机制让这个数字下降**。2026-11-14 架构审查实测
# apps/desktop/src 下 145 行 / 50 文件引用（含文档注释），比文档记录时更多——
# 临时兼容层永久化的典型形态。照 4p/4q 的「只许减不许增」基线范式钉住它。
#
# 口径：
#   · 只数 apps/desktop/src 的 .rs（门面的消费者在这一层）；crates/ 下那几处
#     `term_runtime::` 全是文档注释里的旧路径说明，零真引用，不计入也不约束；
#   · 排除整行注释：文档里提一句旧路径，不等于还留着一条依赖；
#   · 行尾注释所在的代码行仍算（那一行本来就是代码）。
# 基线 138 = 145 行命中 − 7 行整行注释（文件数相应 50 → 49）。降到更少时**同步下调基线并记账**，
# 别把差额留成隐形额度（第 4 刀把 4p 从 44 降到 22 就是这么做的）。
FACADE_REF_BASELINE=148
facade_hits=$(grep -rn 'term_runtime::' apps/desktop/src --include='*.rs' 2>/dev/null \
  | grep -vE ':[[:space:]]*(//|/\*|\*)' || true)
facade_n=$(printf '%s\n' "$facade_hits" | grep -c . || true)
facade_files=$(printf '%s\n' "$facade_hits" | cut -d: -f1 | sort -u | grep -c . || true)
desk_rs_n=$(find apps/desktop/src -name '*.rs' -type f 2>/dev/null | wc -l | tr -d ' ')
if [ "${desk_rs_n:-0}" -eq 0 ]; then
  bad "4s 判据失效：apps/desktop/src 下一个 .rs 也没有——此时本项什么都不查，不许判绿"
elif [ "${facade_n:-0}" -gt "$FACADE_REF_BASELINE" ]; then
  bad "门面引用回升：${facade_n} 行 / ${facade_files} 文件 > 基线 ${FACADE_REF_BASELINE}（新代码请用 term_core / term_io / term_session 的真实路径，别再经门面）"
else
  ok "term_runtime:: 引用 ${facade_n} 行 / ${facade_files} 文件（基线 ${FACADE_REF_BASELINE}，只许减不许增）"
fi

step "4h. ViewState 的每个字段都要有读者"
# 教训：`ViewState` 里 19 个 bool 有 13 个 `ui/` 一次都没读过——菜单是亮的、
# 命令能 parse、点了只翻一个没人看的 bool。这种「接了线但没效果」比置灰更糟：
# 置灰是诚实地表示没做，空转是让人以为坏了。
# （`FindState` 的字段经 `app::find` 读取，不在此列。）
#
# 判据 2026-11-11 收紧。旧判据是「字段名在 ui/ 出现一次即绿」，它有四个洞，其中两个
# 实测能把真缺陷骗过去：
#   ① 注释算数：把 `show_toolbar` 的**全部代码行**抹掉、只留 4 行提到它的注释，
#      旧判据仍绿——而那个开关已经彻底空转了。
#   ② 短名误命中：把 `view.find` 的 6 处真读者全抹掉、留下 `sessions.iter().find(…)`
#      与 `egui::Frame::margin(…)` 这类同名调用，旧判据仍绿。4h 管辖的 32 个字段里，
#      `find` / `center` / `margin` / `lang` 四个都是这种短名，误命中比注释更容易真的漏。
#      （另两个洞：`#[cfg(test)]` 算数、纯赋值 `self.view.x = true` 也算数。）
# 新判据：必须有一行**非注释代码**以 `view.<字段>` / `state.<字段>` 的形式读到它，
# 且不在 `*_tests.rs` 里。两个反例在新判据下都红，32 个字段实测全绿（不假红）。
# 代价（有意的）：将来若用别名读（`let v = &self.view; v.x`）会红——那时把该处写成
# `view.<字段>` 即可。别名会让这条检查彻底失效，宁可逼一次显式访问。
#
# 2026-11-12 起的补充：面板开关表合并到 `term_app::shell::panel_visible`，
# `ui/panels/mod.rs` 不再自己列 9 个分支，于是这 8 个字段在 `ui/` 里的**直接读点
# 消失了**。判据不放松，改成承认**一处受检的间接读**（"桥"）：
#   ① 桥的实现真的读了它：`crates/term-app/src/shell.rs` 生产段里有一行 `state.<字段>`；
#   ② 桥真的被绘制侧调用：`apps/desktop/src/ui` 里有一行 `shell::panel_visible`。
# 两条**同时**成立才算有读者，缺一条就照旧判红。所以删掉 `ui` 那一行调用，
# 这批字段会一起红——白名单不会像旧判据那样无声失效。
# `show_filer` 故意**不**进这张桥：它在 `ui/panels/filer.rs` 还有真实直接读点，
# 那里的读点必须留着（它决定"要不要去连 SFTP"），不能靠桥兜过去。
#
# **新面板开关必须同步登记到这里**。判据本身不看"有没有人登记"，只看"有没有读者"；
# 漏登记的表现不是报"未登记"，而是报"无读者"——于是会出现下面这种假红：
# 桥的实现（`shell.rs` 的 `PanelId::Script => state.show_pane_script`）和
# 桥的调用（`ui/` 里的 `shell::panel_visible`）**两条都成立**，白名单却没跟上。
# 2026-11-13 合并 `feat/script-api` 时就吃过这一记：脚本面板支线加了字段没加白名单。
STATE_RS="crates/term-app/src/state.rs"
BRIDGE_SRC="crates/term-app/src/shell.rs"
BRIDGE_FIELDS=" show_sidebar show_pane_explorer show_pane_outline show_pane_sender \
show_pane_session show_pane_shell show_pane_transfer show_pane_sysmon show_pane_script "
bridge_call=$(grep -rnE 'shell::panel_visible' apps/desktop/src/ui --include='*.rs' 2>/dev/null \
  | grep -v '_tests\.rs:' | grep -vE ':[[:space:]]*(//|/\*|\*)' || true)
bridge_n=0
view_fields=$(awk '/pub struct ViewState \{/,/^}/' "$STATE_RS" 2>/dev/null \
  | grep -oE 'pub [a-z_]+:' | awk '{print $2}' | tr -d ':')
field_n=$(printf '%s\n' "$view_fields" | grep -c . || true)
# 「读」与「写」的区别：`&mut state.x` 与 `state.x = …` 都是写点，不算读者。
# 旧判据只在注释里声称「纯赋值不算」，实际 grep 会把 `self.view.x = true;` 算成读者
# ——那正是 4h 自己那份「旧判据有四个洞」清单里的第 5 个（注释只列了 4 个）。
# 2026-11-12 这一刀顺手堵上：不加这两条过滤，桥的实现侧也会被
# `set_panel_visible` 里的 `&mut state.<字段>` 骗过（实测：把 `panel_visible` 的
# Sidebar 分支删掉，判据仍绿）。`= [^=]` 放过 `==`，所以比较式仍算读。
reads_only() {
  grep -vE "&mut[[:space:]]+(view|state)\.${1}([^A-Za-z0-9_]|$)" \
    | grep -vE "(view|state)\.${1}[[:space:]]*=[^=]" || true
}
dead_fields=""
for f in $view_fields; do
  # `grep -v` 的 `:[[:space:]]*(//|/\*|\*)` 只去掉整行注释；行尾注释（`x; // …`）
  # 仍须靠前缀判据挡——因为行尾注释很少单独出现，前缀才是主防线。
  # 标识符边界用 `([^A-Za-z0-9_]|$)`：BSD grep 不支持 \b。
  #
  # **不要**写成 `… | grep -q .`：本脚本开头有 `set -o pipefail`，而 `grep -q`
  # 一命中就退出，上游 `grep -r` 收到 SIGPIPE 会以 141 结束，pipefail 于是把整条
  # 管道判成失败——32 个字段会全被误报成「无读者」（实测踩过）。取值再判空才是对的。
  hits=$(grep -rnE "(^|[^A-Za-z0-9_])(view|state)\.${f}([^A-Za-z0-9_]|$)" \
           apps/desktop/src/ui --include='*.rs' 2>/dev/null \
         | grep -v '_tests\.rs:' \
         | grep -vE ':[[:space:]]*(//|/\*|\*)' \
         | reads_only "$f")
  if [ -z "$hits" ]; then
    case "$BRIDGE_FIELDS" in
      *" $f "*)
        # 桥的两半都要在。取值再判空——不要 `awk … | grep -q`：pipefail 之下
        # grep 一命中就退出，awk 收 SIGPIPE 以 141 结束，整条管道会被判成失败
        # （本文件 4h 上面那段注释记的就是这个坑）。
        impl_hit=$(awk '/#\[cfg\(test\)\]/{exit} /^[[:space:]]*\/\//{next} {print}' "$BRIDGE_SRC" 2>/dev/null \
          | grep -E "(^|[^A-Za-z0-9_])(view|state)\.${f}([^A-Za-z0-9_]|$)" \
          | reads_only "$f")
        if [ -n "$impl_hit" ] && [ -n "$bridge_call" ]; then
          bridge_n=$((bridge_n + 1))
        else
          dead_fields="$dead_fields $f(桥缺一半：impl=${#impl_hit}B call=${#bridge_call}B)"
        fi
        ;;
      *)
        dead_fields="$dead_fields $f"
        ;;
    esac
  fi
done
if [ "${field_n:-0}" -eq 0 ]; then
  bad "4h 判据失效：$STATE_RS 里一个 ViewState 字段都没解析到（文件挪位或字段改名？）——此时本项什么都不查，不许判绿"
elif [ -n "$dead_fields" ]; then
  bad "ViewState 存在无读者的字段（要么实现效果，要么删掉）：$dead_fields
  判据：必须有非注释代码以 \`view.<字段>\` / \`state.<字段>\` 读到它；注释、*_tests.rs、纯赋值都不算。
  面板类字段允许经 \`term_app::shell::panel_visible\` 间接读，但必须「桥的实现读了它」且
  「ui/ 里有一行调它」同时成立（缺一半就是这个括号里的 \`impl=…B call=…B\`）。"
else
  ok "ViewState 每个标量字段都有真读者（非注释、非测试、经 view./state. 读；其中 $bridge_n 个经 shell::panel_visible 的受检间接读，共 $field_n 个字段）"
fi

step "5. B2：ui/ 不得持有 OS 资源"
# 2026-11-13 加固：原判据是四个**字面量**黑名单，包一层就逃逸。
# 实证：`ui/terminal/session.rs:198` 的 `std::sync::mpsc::channel()` 把 Receiver
# 交给 `HexTap`（apps/desktop/src/hex_tap.rs:24-26），ui 侧字段只写 `HexTap`，
# 四个字面量一个都不命中——门禁绿着，东西已经进去了。
#
# 加固分两层，选型原则是「加进去今天必须仍然是绿的」，否则门禁第一天就在喊狼来了：
#   ① 语法层：`Receiver<` / `Sender<` 裸类型进 ui/ 即红。今天实测 0 命中。
#      注意 **BSD grep -E 不支持 `\b`**（本轮实测过：加了 `\b` 会静默 0 命中，
#      把判据变成永远绿的假检查——这项目已经栽过一次 `\|` 的同类跟头）。
#   ② 句柄层：冻结基线 3 处。只匹配**字段声明**形态，不数 `use` 与注释——否则
#      `filer.rs` 的文档注释会把总命中抬到 13 处，基线就退化成「随便加，只要
#      不加文档」。第 4 个持有者出现即红，与 4p/4q 同一口径：基线是现状事实。
HANDLE_BASELINE=3
if [ -d apps/desktop/src/ui ]; then
  hits=$(grep -rEn 'MasterPty|mpsc::Receiver|portable_pty|std::process::Child' apps/desktop/src/ui --include='*.rs' 2>/dev/null || true)
  chan_hits=$(grep -rEn '(Receiver|Sender)[[:space:]]*<' apps/desktop/src/ui --include='*.rs' 2>/dev/null || true)
  handle_hits=$(grep -rEn '^[[:space:]]*(pub([(][^)]*[)])?[[:space:]]+)?[a-zA-Z_][a-zA-Z0-9_]*:[[:space:]]*Option<(SftpHandle|TunnelHandle|ConnectHandle)>' apps/desktop/src/ui --include='*.rs' 2>/dev/null || true)
  handle_n=$(printf '%s\n' "$handle_hits" | grep -c . 2>/dev/null || true)
  if [ -n "$hits" ]; then
    bad "ui/ 混入 OS 资源：$hits"
  elif [ -n "$chan_hits" ]; then
    bad "ui/ 直接持有 channel 端（应经不透明句柄包一层）：$chan_hits"
  elif [ "${handle_n:-0}" -gt "$HANDLE_BASELINE" ]; then
    bad "ui/ 不透明句柄持有者增至 ${handle_n} 处 > 基线 ${HANDLE_BASELINE}：$(printf '%s' "$handle_hits" | tr '\n' ' ')"
  else
    ok "ui/ 零 OS 资源；零裸 channel 端；不透明句柄 ${handle_n:-0} 处（基线 ${HANDLE_BASELINE}，只许减不许增）"
  fi
else
  ok "ui/ 尚未建立（当时跳过）"
fi

step "6. 禁止字符串派发（强制）"
hits=$(grep -rn 'last_action' apps/desktop/src crates --include='*.rs' 2>/dev/null || true)
if [ -n "$hits" ]; then
  c=$(printf '%s\n' "$hits" | wc -l | tr -d ' ')
  if [ "${STRICT_B4:-0}" = "1" ]; then
    bad "仍有字符串派发：$hits"
  else
    ok "已知待办：$c 处，待清除（当前不阻断）；之后用 STRICT_B4=1 强制"
  fi
else
  ok "无 last_action"
fi

step "7. 合规标记（Mach-O 地址注释）"
n=$(grep -rEo '0x[0-9a-fA-F]{8,}' apps/desktop/src crates --include='*.rs' 2>/dev/null | wc -l | tr -d ' ')
if [ "$n" -le "${ALLOW_MARKERS}" ]; then
  ok "${n} 处，阈值 ${ALLOW_MARKERS}；详见仓库内的合规标记说明"
else
  bad "${n} 处超过阈值 ${ALLOW_MARKERS}"
fi

step "7b. 全仓库地址字面（含未跟踪的新文件）"
# 防御性写法：`docs` 现在是实体目录，但这个仓库曾被改成过符号链接，
# 未来也可能再被谁换掉。BSD grep 的三种行为都实测过：
#   grep -r docs   → 若 docs 是链接："Is a directory"、空结果（被 || true 吞掉 = 假绿）
#   grep -R docs   → 起始参数是符号链接时仍不跟随
#   grep -R docs/  → 带尾斜杠才强制当作目录并跟随
# 所以固定用 -R + 尾斜杠；不要把尾斜杠"清理"掉。
hits=$(grep -REn '0x[0-9a-fA-F]{8,}' apps/desktop/src crates docs/ scripts --include='*.rs' --include='*.md' --include='*.sh' 2>/dev/null || true)
if [ -n "$hits" ]; then
  bad "仓库内仍有地址字面：$hits"
elif [ ! -d docs ]; then
  bad "docs 不可用（不存在或断链）：文档无法被检查"
elif [ -z "$(ls -A docs 2>/dev/null)" ]; then
  bad "docs 是空目录：文档丢了"
else
  ok "全仓库 0 命中（docs 可读，$(ls docs | wc -l | tr -d ' ') 份文档）"
fi

step "8. 截图基准存档"
# 六张整窗 + 每组件一张：整窗管位置，组件管外观（见 8c）。
shots=apps/desktop/tests/baseline/screenshot-*.png
n_shots=$(ls $shots 2>/dev/null | wc -l | tr -d ' ')
n_comp=$(ls apps/desktop/tests/baseline/components/*.png 2>/dev/null | wc -l | tr -d ' ')
if [ "${n_shots:-0}" -ge 6 ] && [ "${n_comp:-0}" -ge 2 ]; then
  ok "基线齐备：整窗 ${n_shots} 张（贴底 / 回看 / 查找 / 选词 / 列选 / 弹窗）+ 组件 ${n_comp} 张"
else
  bad "截图基准缺失：整窗 ${n_shots:-0} 张（应 ≥6）、组件 ${n_comp:-0} 张（应 ≥2）"
fi

step "8c. 视觉回归（可选，VERIFY_SCREENSHOT=1）：整窗十一张（含中文）+ 组件若干"
# 一次跑完 `--all`：整窗图守「面板/弹窗在窗口里的位置与占位」，组件图守
# 「这个组件自己长什么样」。原先分成两个开关，是因为整窗基线还停在皮肤接管
# 之前（一开就红）；2026-11-06 基线全量重采集之后这个理由不存在了。
# 仍默认不跑：截图需要真实窗口环境，无头环境会失败——那是环境问题，
# 不该伪装成回归失败。
# 2026-11-19：底层已改为 `SHOT_JOBS` 并行 + 成功结果缓存；缓存键含二进制、
# 基线与脚本哈希，任何决定像素的输入变了都会重拍，`VERIFY_NO_SHOT_CACHE=1`
# 可强制全跑。判据仍是「每张输出各占一行且含『与基线一致』」，没有放宽。
if [ "${VERIFY_SCREENSHOT:-0}" = "1" ]; then
  shot_out=$(scripts/screenshot-baseline.sh --all 2>&1)
  if [ $? -eq 0 ]; then
    n=$(printf '%s\n' "$shot_out" | grep -c '与基线一致')
    ok "整窗 + 组件共 ${n} 张图与基线字节一致"
  else
    bad "截图与基线不一致：$(printf '%s' "$shot_out" | grep '✗' | head -3)"
  fi
else
  ok "跳过（用 VERIFY_SCREENSHOT=1 开启）"
fi

step "8d. term-ssh 端到端（进程内 mock 服务端，真握手）"
# 这一层最容易错的不是逻辑而是接缝：`check_server_key` 的返回值、`request_pty`
# 的参数顺序、`ExitStatus` 的到达时机。所以必测的不是 mock，是**真握手**——
# `tests/e2e.rs` 里起一个 russh 服务端（同库的 server 端），只监听 127.0.0.1，
# 不依赖 sshd、不依赖网络。
if [ -f crates/term-ssh/tests/e2e.rs ]; then
  ssh_out=$(cargo test -p term-ssh --target-dir "$TARGET" --quiet 2>&1)
  ssh_status=$?
  if [ "$ssh_status" -eq 0 ]; then
    ssh_n=$(printf '%s\n' "$ssh_out" | grep -oE 'test result: ok\. [0-9]+' | awk '{s+=$4} END {print s+0}')
    ok "单测 + 真握手全过（${ssh_n} 个用例）"
  else
    bad "term-ssh 测试失败：$(printf '%s' "$ssh_out" | grep -E 'FAILED|panicked at' | head -3)"
  fi
else
  bad "crates/term-ssh/tests/e2e.rs 缺失"
fi

step "8e. term-store 恢复测试（SQLite 往返 + 库文件里没有明文密码）"
# 判据三条：
# 1) 会话列表过一遍库还能原样回来（身份列 + 整份 JSON 都不许丢字段）；
# 2) 凭据引用（vault_entry_id）不会被「保存列表」抹掉；
# 3) 库文件（含 -wal）的原始字节里找不到密码——密码只在保险库里。
# 还有一条**接线**判据：产品的持久化必须真的走这个库，
# 否则它又变回「只活在库里、界面用不到」。
if [ -d crates/term-store/src ]; then
  store_out=$(cargo test -p term-store --target-dir "$TARGET" --quiet 2>&1)
  store_status=$?
  wired=$(grep -c 'term_store::Store' crates/term-profile/src/persist.rs 2>/dev/null || true)
  if [ "$store_status" -ne 0 ]; then
    bad "term-store 测试失败：$(printf '%s' "$store_out" | grep -E 'FAILED|panicked at' | head -3)"
  elif [ "${wired:-0}" -eq 0 ]; then
    bad "会话库没接进产品：crates/term-profile/src/persist.rs 里没有 term_store::Store"
  else
    store_n=$(printf '%s\n' "$store_out" | grep -oE 'test result: ok\. [0-9]+' | awk '{s+=$4} END {print s+0}')
    ok "会话库往返 + 凭据引用 + 无明文密码全过（${store_n} 个用例），persist 已改走 Store"
  fi
else
  bad "crates/term-store/src 缺失"
fi

step "8f. term-tunnel 进程内真握手（-L / -D / -R 各搬一次真字节）"
# 隧道最容易错的也是接缝：`direct-tcpip` 的目标参数顺序、SOCKS5 的应答字节、
# `tcpip-forward` 的应答端口、`close()` 之后监听是否真的放掉。所以四个用例都起来
# 真握手，并断言**服务端收到的目标地址 / 端口**就是命令行里写的那个——写反会直接红。
if [ -f crates/term-tunnel/tests/e2e.rs ]; then
  tunnel_out=$(cargo test -p term-tunnel --target-dir "$TARGET" --quiet 2>&1)
  tunnel_status=$?
  if [ "$tunnel_status" -eq 0 ]; then
    tunnel_n=$(printf '%s\n' "$tunnel_out" | grep -oE 'test result: ok\. [0-9]+' | awk '{s+=$4} END {print s+0}')
    ok "转发规则 + SOCKS5 协议 + 真握手全过（${tunnel_n} 个用例）"
  else
    bad "term-tunnel 测试失败：$(printf '%s' "$tunnel_out" | grep -E 'FAILED|panicked at' | head -3)"
  fi
else
  bad "crates/term-tunnel/tests/e2e.rs 缺失"
fi

step "8g. 库真的接进产品了（不是「只活在库里」）"
# 这一项存在的理由是本项目反复踩过的坑：库做完、单测全绿、界面上一个入口都没有。
# 四个判据都是「调用点存在」这种可 grep 的事实：
# 1) 会话持久化走 Store（persist 里不再有把会话序列化成 JSON 的代码）；
# 2) 连之前会问保险库要密码（`stored_password`）；
# 3) 连上之后按配置起隧道（`start_session_tunnel`）；
# 4) 会话配置里有转发规则字段（`SessionConfig.forwards`）——没有它前两条无从谈起；
# 5) 文件面板真的会去开 SFTP 会话（`term_sftp::start_in_background` 的调用点），
#    并且那个开关在 `ViewState` 里（否则菜单亮了却没有读者，正是修过的那类假接线）。
json_left=$(grep -cE 'serde_json::to_(string|vec)' crates/term-profile/src/persist.rs 2>/dev/null || true)
vault_used=$(grep -rl 'stored_password' apps/desktop/src 2>/dev/null | wc -l | tr -d ' ')
tunnel_used=$(grep -rl 'start_session_tunnel' apps/desktop/src 2>/dev/null | wc -l | tr -d ' ')
forwards_field=$(grep -c 'pub forwards: Vec<String>' crates/term-core/src/session.rs 2>/dev/null || true)
sftp_used=$(grep -rl 'term_sftp::start_in_background' apps/desktop/src 2>/dev/null | wc -l | tr -d ' ')
filer_field=$(grep -c 'pub show_filer: bool' crates/term-app/src/state.rs 2>/dev/null || true)
if [ "${json_left:-0}" -ne 0 ] || [ "${vault_used:-0}" -eq 0 ] || [ "${tunnel_used:-0}" -eq 0 ] || [ "${forwards_field:-0}" -eq 0 ] || [ "${sftp_used:-0}" -eq 0 ] || [ "${filer_field:-0}" -eq 0 ]; then
  bad "接线不全：persist 里残留 JSON=[$json_left] 用保险库的文件数=${vault_used:-0} 起隧道的文件数=${tunnel_used:-0} forwards 字段=${forwards_field:-0} 开 SFTP 的文件数=${sftp_used:-0} show_filer 字段=${filer_field:-0}"
else
  ok "persist 只走 Store；desk 连接前查保险库、连上后起隧道、开面板时起 SFTP；会话配置带转发规则"
fi

step "8h. term-sftp 端到端（进程内 SSH 服务端 + 真 sftp-server 子系统）"
# 客户端最容易错的地方全在协议细节上（版本交换、realpath、readdir 的 EOF、
# 失败目录的应答码）。自己实现一遍服务端只能测「我们对协议的理解」，
# 所以这里把通道接到系统自带的 `sftp-server` 上——测的是能不能和真的实现谈成。
#
# 2026-11-13：这 7 条真机用例改成 `#[ignore]` + `require_prog`。原文写的是
# 「没有 sftp-server 时用例会 skip 并在输出里写明，不当失败处理」——那句话正是
# 假绿的入口：缺依赖的机器上 7 条 `return` 之后 libtest 报 `passed`，于是
# 「从没和真服务端谈成过」与「谈成了」在输出里长得一模一样。ZMODEM 的同一句
# 写法已被审查指出并修掉，这里补齐，不留双标准。
#
# 所以这一项跑两次：默认那次确认 7 条**确实**被 ignored（若某天有人把
# `#[ignore]` 删了，这里会红），`--ignored` 那次才真跑。两次都判，因为
# `0 passed` 的退出码同样是 0——只看退出码会再假绿一次（4j/第 3 项都栽过）。
# 条数是 8 不是 7：`closing_the_handle_hangs_up_the_server_side_session` 也需要真
# `sftp-server`，与另外 7 条同一套前提，所以用同一个 `[SFTP-E2E]` 标记（一个文件
# 里两个标记各数各的，迟早数错）。`closing_twice_is_idempotent_and_never_hangs`
# 连的是没人听的端口、不需要外部依赖，因此默认就跑、不算在这 8 条里。
SFTP_E2E_N=8
if [ -f crates/term-sftp/tests/e2e.rs ]; then
  sftp_out=$(cargo test -p term-sftp --test e2e --target-dir "$TARGET" --quiet 2>&1)
  sftp_status=$?
  sftp_ign=$(printf '%s\n' "$sftp_out" | grep -oE '[0-9]+ ignored' | grep -oE '[0-9]+' | head -1 || true)
  # `--test-threads=1`：这些用例各自起 mock 服务端占端口，串行跑才是人工验证时
  # 的那个条件；门里换条件等于换了被测对象。
  # `--nocapture` **必须带**：libtest 默认捕获 stdout，成功时不打印测试里的
  # `println!`，于是下面数 `[SFTP-E2E] PASS` 会恒得 0、这条判据必然红（或者更糟：
  # 如果哪天有人把留痕断言删了，它会静默地永远「通过」）。加它才能真正数到留痕。
  sftp_real=$(cargo test -p term-sftp --test e2e --target-dir "$TARGET" --quiet -- --ignored --test-threads=1 --nocapture 2>&1)
  sftp_real_status=$?
  sftp_real_passed=$(printf '%s\n' "$sftp_real" | grep -oE 'test result: ok\. [0-9]+' | awk '{s+=$4} END {print s+0}')
  sftp_pass_n=$(printf '%s\n' "$sftp_real" | grep -c '\[SFTP-E2E\] PASS' || true)
  if [ "$sftp_status" -ne 0 ]; then
    bad "term-sftp 默认测试失败：$(printf '%s' "$sftp_out" | grep -E 'FAILED|panicked at' | head -3)"
  elif [ "${sftp_ign:-0}" -lt "$SFTP_E2E_N" ]; then
    bad "真机用例没有被 ignored（${sftp_ign:-0} < ${SFTP_E2E_N}）——静默跳过会伪装成 passed"
  elif [ "$sftp_real_status" -ne 0 ]; then
    bad "真 sftp-server 对谈失败：$(printf '%s' "$sftp_real" | grep -E 'FAILED|panicked at|SFTP-E2E\] FAIL' | head -3)"
  elif [ "${sftp_real_passed:-0}" -lt "$SFTP_E2E_N" ] || [ "${sftp_pass_n:-0}" -lt "$SFTP_E2E_N" ]; then
    bad "真机用例跑数不足：passed=${sftp_real_passed:-0} 留痕=${sftp_pass_n:-0} 行（各要 ≥ ${SFTP_E2E_N}）"
  else
    ok "SFTP 列目录 / 子目录 / 失败目录 / 错误密码四类全过（真 sftp-server，${sftp_real_passed} 条 passed + ${sftp_pass_n} 行 PASS 留痕）"
  fi
else
  bad "crates/term-sftp/tests/e2e.rs 缺失"
fi

step "8i. 会话日志端到端（清洗器 + 监管者线程真落盘）"
# 两个判据：
# 1) 清洗器（`term_core::ansi`）—— 转义序列被吃掉、CRLF 折成一个换行、
#    跨 chunk 的 UTF-8 不被切坏。这几条是「日志能不能 grep」的全部前提；
# 2) 监管者线程真的把字节写进文件（`supervisor::logging_captures_output_through_the_supervisor`）——
#    库做好了但产品没接，是这个项目反复踩过的坑（见 8g）。
# 日志写错不会让程序崩，只会让用户排障时拿到一份没用的文件，所以单独守一条。
log_clean=$(cargo test -p term-core --target-dir "$TARGET" --quiet ansi 2>&1)
log_clean_status=$?
log_wire=$(cargo test -p term-io --target-dir "$TARGET" --quiet log 2>&1)
log_wire_status=$?
if [ "$log_clean_status" -eq 0 ] && [ "$log_wire_status" -eq 0 ]; then
  log_n=$(printf '%s\n%s\n' "$log_clean" "$log_wire" | grep -oE 'test result: ok\. [0-9]+' | awk '{s+=$4} END {print s+0}')
  ok "转义清洗 / CRLF 归一 / 跨 chunk UTF-8 / 真落盘全过（${log_n} 个用例）"
else
  bad "会话日志测试失败：$(printf '%s\n%s' "$log_clean" "$log_wire" | grep -E 'FAILED|panicked at' | head -3)"
fi

step "9. 工作树干净"
if [ "${SKIP_GIT_CHECK:-0}" = "1" ]; then
  ok "提交前跳过（提交后请不带该变量重跑）"
elif git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  dirty=$(git status --porcelain 2>/dev/null)
  if [ -z "$dirty" ]; then ok "git status 干净"; else bad "存在未提交改动：\n$dirty"; fi
else
  bad "未初始化 git"
fi

if [ "${VERIFY_CLIPPY:-0}" = "1" ]; then
  step "10. cargo clippy（可选，-D warnings 强制 0 warning；含 --all-targets）"
  # 旧版只看 exit code，warning 会被 --quiet + 2>/dev/null 吞掉，属于验收假绿。
  #
  # `--all-targets`（2026-11-15 补）：不带它时 `#[cfg(test)]` 里的代码**从来没被
  # lint 过**，于是测试代码里的 lint 债可以无限欠着——实测就欠着 3 条
  # `clippy::useless_vec`（`terminal/tab_slot.rs`）。这和 4j 当初「只用 wc -l
  # 数行数、注释和空行也算」是同一类假绿：判据看似存在，实际量不到它声称的量。
  clippy_out=$(cargo clippy --workspace --all-targets --target-dir "$TARGET" --quiet -- -D warnings 2>&1)
  if [ $? -eq 0 ]; then
    ok "clippy 通过，0 warning（-D warnings）"
  else
    bad "clippy 失败或存在 warning：$(printf '%s' "$clippy_out" | tail -20)"
  fi
fi

step "11. 文档事实对账（docs/facts.txt ↔ 实测）"
# 数字散落在四五处文档里（单测数、验收项数、能力面三元组、基线张数），
# 靠人肉同步必然漏。facts.txt 是「机器认的那一份」，正文里的数字以它为准。
# 公开检出里没有内部台账 docs/facts.txt 时跳过本项，不把「文件不存在」当失败。
if [ ! -f docs/facts.txt ]; then
  ok "docs/facts.txt 不在本检出中，跳过（内部台账不随公开仓库发布）"
else
  doc_out=$(scripts/check-docs.sh 2>&1)
  if [ $? -eq 0 ]; then
    ok "$doc_out"
  else
    bad "$(printf '%s' "$doc_out" | head -1)"
  fi
fi

printf '\n══════════════════════════════\n'
if [ "$fails" -eq 0 ]; then
  printf '验收通过：全部检查项通过，可以提交本阶段。\n'
else
  printf '验收失败：%d 项未通过，禁止进入下一阶段。\n' "$fails"
fi
exit "$fails"
