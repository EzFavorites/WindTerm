#!/usr/bin/env bash
# 文档数字对账：把 `docs/facts.txt` 里声明的数字与仓库实测值比对。
#
# 为什么需要它：改一行动辄要手改四五处文档（单测数、验收项数、能力面三元组、
# 基线张数），漏一处就是下一个「文档说一套、代码做一套」。历史上这些数字
# 漂移过很多次（171→158→160、17→19→20、23→17…），每次都靠人记住。
#
# 用法：
#   scripts/check-docs.sh          # 比对，不一致即非零退出
#   scripts/check-docs.sh --print  # 打印当前实测值（用来更新 facts.txt）
set -u
cd "$(dirname "$0")/.." || exit 1

PRINT=0
[ "${1:-}" = "--print" ] && PRINT=1

# ── 实测值 ──────────────────────────────────────────────────────────────
count_tests() {
  # 口径修正：`#[tokio::test]` 也是一条真实会跑的用例，此前只数
  # `#[test]`，于是「协议层 10 条真代理测试」全部落在台账之外。
  # 用 ERE 一次覆盖两种 attribute（本机 toybox grep 不认 BRE 的 `\|`）。
  grep -rcE '#\[(tokio::)?test\]' "$1/src" --include='*.rs' 2>/dev/null | awk -F: '{s+=$2} END {print s+0}'
}

tests_core=$(count_tests crates/term-core)
# 2026-11-12 底层收敛：term-runtime 拆出 term-paths（路径规则叶子）与
# term-profile（用户档案：会话库 / 布局 / 偏好 / 快捷栏 / 搜索表）。
# 两个新 crate 的单测都写在 src/ 里，没有 tests/ 目录。
tests_paths=$(count_tests crates/term-paths)
tests_profile=$(count_tests crates/term-profile)
# C3：会话 IO 与线程层（pty / serial / channel / supervisor / log / local_dir），
# 单测全在 src/ 里（`crates/term-runtime/tests/` 从来没有存在过）。
tests_io=$(count_tests crates/term-io)
# C4：独立配置导入模块（sessions / theme）。搬完之后 `term-runtime` 只剩再导出，
# 单测归零——所以 `tests_runtime` 这个键**被删掉了**，不再是一个恒为 0 的噪音项。
tests_import=$(count_tests crates/term-import)
tests_app=$(count_tests crates/term-app)
# 2026-11-12 app-split：`term-app` 的四个零生产读者模块收进了 `planned`
# feature，**默认不编译**。台账口径跟着改成「默认 `cargo test --workspace` 真的
# 会跑多少条」，另外单列 `tests_app_planned`。两个数相加仍是源码里的 `#[test]`
# 总数。只数文件不改 `count_tests`：那函数是「某目录下所有 .rs」，改它会影响
# 其余 16 个 crate。
#
# 2026-11-12 B2 Step 9 修正：`tmux_layout` 因有了真读者（`docker_tmux.rs` 直接
# 引用）已提回**默认编译面**，它的 12 条测试现在默认会跑。这里必须把它从扣减
# 名单里拿掉，否则 `tests_app` 会比真实少 12，且「两个数相加 = 总数」还成立、
# 自洽得看不出错。剩下三个仍在 planned 下：structure / trigger / trigger_regex。
tests_app_planned=$(grep -rcE '#\[(tokio::)?test\]' \
  crates/term-app/src/structure.rs \
  crates/term-app/src/trigger.rs crates/term-app/src/trigger_regex.rs \
  2>/dev/null | awk -F: '{s+=$2} END {print s+0}')
tests_app=$((tests_app - tests_app_planned))
tests_vault=$(count_tests crates/term-vault)
tests_session=$(count_tests crates/term-session)
# term-ssh 是唯一把核心行为放在 `tests/` 里的 crate：握手、认证、退出码这些
# 只能端到端测（进程内起一个 russh 服务端），只数 `src/` 会漏掉其中一半。
# 同类还有 tunnel / sftp / script / desk：真握手、真 PTY、Docker 集成用例都在
# `tests/` 里（desk 的 tests/ 既有金样本数据也有 docker_*.rs，2026-11-08 对账
# 才发现只数 src/ 漏了后者）。凡是 tests/ 里有 .rs 的目录都得数上。
tests_desk=$(( $(count_tests apps/desktop) + $(grep -rcE '#\[(tokio::)?test\]' apps/desktop/tests --include='*.rs' 2>/dev/null | awk -F: '{s+=$2} END {print s+0}') ))
tests_ssh=$(( $(count_tests crates/term-ssh) + $(grep -rcE '#\[(tokio::)?test\]' crates/term-ssh/tests --include='*.rs' 2>/dev/null | awk -F: '{s+=$2} END {print s+0}') ))
# 隧道的真握手与 SFTP 的端到端也是同一类：都在 `tests/` 里。
# （曾经有 5 个迁移用例，拍板「不留历史包袱」后整块删掉了。）
tests_store=$(count_tests crates/term-store)
tests_tunnel=$(( $(count_tests crates/term-tunnel) + $(grep -rcE '#\[(tokio::)?test\]' crates/term-tunnel/tests --include='*.rs' 2>/dev/null | awk -F: '{s+=$2} END {print s+0}') ))
tests_sftp=$(( $(count_tests crates/term-sftp) + $(grep -rcE '#\[(tokio::)?test\]' crates/term-sftp/tests --include='*.rs' 2>/dev/null | awk -F: '{s+=$2} END {print s+0}') ))
# term-script 的 e2e 在 tests/ 里（真 PTY 验收），单测在 src/ 里。
tests_script=$(( $(count_tests crates/term-script) + $(grep -rcE '#\[(tokio::)?test\]' crates/term-script/tests --include='*.rs' 2>/dev/null | awk -F: '{s+=$2} END {print s+0}') ))
# term-bridge（S-c 腿 B）同理：协议解析 / 错误映射 / 桩生成 / stdio 传输的单测在
# src/，而这一刀真正的验收在 tests/ —— 真 PTY 走一遍能力面，加上**外部 python3
# 进程**经 stdio 跑同一套流程。只数 src/ 等于把 S-c 的验收口径漏在对账之外。
tests_bridge=$(( $(count_tests crates/term-bridge) + $(grep -rcE '#\[(tokio::)?test\]' crates/term-bridge/tests --include='*.rs' 2>/dev/null | awk -F: '{s+=$2} END {print s+0}') ))
# term-xfer（XMODEM/YMODEM 协议层）也把大部分验收放在 tests/ 里：线格式在 src/，
# 而「任意切分等价」「收发闭环逐字节相同」「YMODEM 批处理时序」这些只能喂整段
# 字节流来验。只数 src/ 会漏掉 58 条里的 47 条。
tests_xfer=$(( $(count_tests crates/term-xfer) + $(grep -rcE '#\[(tokio::)?test\]' crates/term-xfer/tests --include='*.rs' 2>/dev/null | awk -F: '{s+=$2} END {print s+0}') ))
# term-zmodem 同理：帧层/扫描器在 src/，而握手时序、混流、错误恢复、联调全在 tests/。
# 联调那两条在没装 lrzsz 的机器上静默跳过，但仍计入条数（它们是真实存在的测试）。
tests_zmodem=$(( $(count_tests crates/term-zmodem) + $(grep -rcE '#\[(tokio::)?test\]' crates/term-zmodem/tests --include='*.rs' 2>/dev/null | awk -F: '{s+=$2} END {print s+0}') ))
tests_total=$((tests_core + tests_paths + tests_profile + tests_io + tests_import + tests_app + tests_vault + tests_ssh + tests_store + tests_tunnel + tests_sftp + tests_script + tests_bridge + tests_session + tests_desk + tests_xfer + tests_zmodem))

verify_steps=$(grep -c 'step "' scripts/verify.sh 2>/dev/null || echo 0)

# 能力面：以 capability.rs 里的显式断言为准（那里是「数字必须一起改」的地方）
menu_commands=$(grep -oE 'inventory\(\)\.len\(\), *[0-9]+' crates/term-app/src/capability.rs | grep -oE '[0-9]+$')
commands_wired=$(grep -oE 'assert_eq!\(implemented, *[0-9]+' crates/term-app/src/capability.rs | grep -oE '[0-9]+$')
commands_planned=$(grep -oE 'assert_eq!\(planned, *[0-9]+' crates/term-app/src/capability.rs | grep -oE '[0-9]+$')

baselines=$(ls apps/desktop/tests/baseline/screenshot-*.png 2>/dev/null | wc -l | tr -d ' ')
golden_cases=$(ls apps/desktop/tests/golden/*.json 2>/dev/null | wc -l | tr -d ' ')
# 按目录数，不按 workspace 成员数：`apps/` 也在 members 里，
# 用引号数会把 6 个成员全算成 crate，于是「4 个 crate」这句话就假了。
crates=$(ls -d crates/*/ 2>/dev/null | wc -l | tr -d ' ')
apps=$(ls -d apps/*/ 2>/dev/null | wc -l | tr -d ' ')

# 能力矩阵的任务条目数（`| 数字 |` 开头的行）。数字本身不重要，重要的是它别被删空——
# 这张表是「用户视角」的唯一口径，删掉就等于没人再看这个维度。
matrix_tasks=$(grep -cE '^\| [0-9]+ \|' docs/design/12-capability-surface-and-menus.md 2>/dev/null || echo 0)

# 与原程序的功能对齐度条目数（同样是「别被删空」的意思）
parity_items=$(grep -cE '^\| [0-9]+ \|' docs/design/14-scope-and-requirements.md 2>/dev/null || echo 0)

# 菜单未完成清单的明细行数（必须等于「菜单总数 - 已接线」）
menu_backlog_rows=$(grep -cE '^\| `[A-Za-z]+\.[A-Za-z0-9]+` \|' docs/design/12-capability-surface-and-menus.md 2>/dev/null || echo 0)

actual="tests_core=$tests_core
tests_paths=$tests_paths
tests_profile=$tests_profile
tests_io=$tests_io
tests_import=$tests_import
tests_app=$tests_app
tests_app_planned=$tests_app_planned
tests_vault=$tests_vault
tests_ssh=$tests_ssh
tests_store=$tests_store
tests_tunnel=$tests_tunnel
tests_sftp=$tests_sftp
tests_script=$tests_script
tests_bridge=$tests_bridge
tests_session=$tests_session
tests_desk=$tests_desk
tests_xfer=$tests_xfer
tests_zmodem=$tests_zmodem
tests_total=$tests_total
verify_steps=$verify_steps
menu_commands=$menu_commands
commands_wired=$commands_wired
commands_planned=$commands_planned
baselines=$baselines
golden_cases=$golden_cases
crates=$crates
apps=$apps
matrix_tasks=$matrix_tasks
parity_items=$parity_items
menu_backlog_rows=$menu_backlog_rows"

if [ "$PRINT" = "1" ]; then
  printf '%s\n' "$actual"
  exit 0
fi

# ── 比对 ────────────────────────────────────────────────────────────────
FACTS="docs/facts.txt"
if [ ! -f "$FACTS" ]; then
  echo "缺少 ${FACTS}（用 scripts/check-docs.sh --print 生成后人工确认）" >&2
  exit 1
fi

# 逐项比对，产出「哪一项、文档写的是什么、实测是什么」——只说「不一致」没用。
mismatch=""
while IFS= read -r line; do
  case "$line" in ''|\#*) continue;; esac
  key=${line%%=*}
  want=${line#*=}
  have=$(printf '%s\n' "$actual" | grep -E "^${key}=" | cut -d= -f2)
  if [ -z "$have" ]; then
    mismatch="$mismatch ${key}(文档=$want 实测=无此项)"
  elif [ "$want" != "$have" ]; then
    mismatch="$mismatch ${key}(文档=$want 实测=$have)"
  fi
done < "$FACTS"

# 反向：实测里有、facts 里漏写的项也算不一致（新增统计维度时容易漏）
for kv in $actual; do
  key=${kv%%=*}
  grep -qE "^${key}=" "$FACTS" || mismatch="$mismatch ${key}(实测=${!key} 文档=未登记)"
done

if [ -z "$mismatch" ]; then
  echo "文档事实与实测一致（$(printf '%s\n' "$actual" | wc -l | tr -d ' ') 项）"
  exit 0
fi

echo "文档事实与实测不一致：$mismatch" >&2
echo "" >&2
echo "当前实测值（确认是预期变化后再更新 docs/facts.txt）：" >&2
printf '%s\n' "$actual" | sed 's/^/  /' >&2
exit 1
