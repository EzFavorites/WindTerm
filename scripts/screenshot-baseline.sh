#!/usr/bin/env bash
# 视觉回归：整窗十一张 + 组件若干。
#
# 整窗十一张都是「这台机器 + 这个窗口尺寸」的字节级基线：
#   screenshot-round64.png   贴底状态（Craft round 64 起沿用）
#   screenshot-scroll.png    回看 6 行 + 跨三行选区 + 回看浮标（F1）
#   screenshot-find.png      回看 6 行 + Find 栏打开 + 匹配高亮（F2）
#   screenshot-word.png      回看 6 行 + 双击选中的单个词（F3）
#   screenshot-block.png     回看 6 行 + 三行同一段列的矩形选区（F4）
#   screenshot-dialog.png    New Session 弹窗打开（弹窗位置 / 尺寸 / 是否溢出窗口）
#   screenshot-margin.png    margin 边栏：时间戳列 + 行号列 + `›`/`~` 符号列
#   screenshot-dock.png      底带：一条带 + 标签条 + Sender 工具行
#   screenshot-bin.png       Hex 视图（`Mode ▸ Hex Edit`）：12 个满行、贴底视口
#                            落在环首，图上第一行偏移正好 `00000000`——这是
#                            「显示的是会话建立以来的全部字节」唯一的视觉证据
#   screenshot-binfull.png   Hex 视图灌满 1 MiB 环再溢出 4096：表头出现
#                            `4096 bytes dropped`、首行偏移已推进到 1 MiB 那一带
#                            （不再是 `00000000`，这就是环真的丢过字节的证据）
#   screenshot-zh.png        中文界面：菜单 / 侧栏过滤框 / 快捷栏 / 底带 / 状态栏同时换字
#
# 前十张由 `WINDTERM_LANG=en` 钉死，第十一张显式给 `WINDTERM_LANG=zh`；组件图一律
# 英文。基线不能锚在用户 preferences 上，否则切了语言就会一起红。
#
# 取舍：弹窗的**外观**由组件图（`components/dialog_*.png`）守，整窗这张只守
# 「它在窗口里的位置与占位」——弹窗被挤到窗口外、或者挡住状态栏，组件图看不出来。
# 两张图各管一件事，谁红了就指向谁，不用在整屏里猜。
#
# 全部由 `--demo <夹具>` 生成：夹具走 term-core 的 Terminal 处理固定字节流，
# 不依赖 PTY 输出时序，所以同一二进制连续跑多次哈希不变（已实测 5 次一致）。
#
# 组件级基线：`tests/baseline/components/<name>.png`，一张图一个组件。
# 组件图的背景是纯色画布（`harness::canvas_bg`），不含会话树 / 状态栏 / 终端网格，
# 所以改弹窗时比对的就是弹窗本身，而不是「整屏 1280×980 里找差异」。
# 名单从二进制里取（`--list-components`），脚本不另抄一份。
#
# 后台渲染（2026-11-09 起）：每次截图运行都经 `term-desk --screenshot` 在
# 「不可见窗口」里完成——`apps/desktop/src/main.rs` 在带 `--screenshot` 时把窗口
# 创建为 `with_visible(false)`，wgpu 仍然把帧渲到图层，`ViewportCommand::Screenshot`
# 读的是渲染后的那一帧。所以跑这套脚本**不会弹窗口、不抢前台焦点**，可随时在
# 你正在工作的前台环境里直接运行，不必切到空闲桌面。手动 `--demo <夹具>`
# （不带 `--screenshot`）仍按原样可见，便于肉眼调试夹具。11 整窗 + 全部组件图
# 已用隐藏窗口逐字节复验，与可见窗口渲染结果完全一致。
#
# 2026-11-19 性能：98 张图原先逐张启动进程，实测 224s。现在做两件保守优化：
#   1. `SHOT_JOBS=N` 并行跑（默认 4，设 1 退回逐张）；每张图独立 profile 与输出，
#      互不共享状态，失败仍按名单顺序汇总；
#   2. 成功结果按「二进制 hash + 全部基线 hash + 本脚本 hash + 夹具签名」缓存到
#      `$TARGET/.verify-shot-cache/`，同Inputs重跑直接复用；`VERIFY_NO_SHOT_CACHE=1`
#      或 `--update` 强制全跑。
# 两个开关都不改变「逐字节比对」这条判据，只是省掉重复启动已证明一致的组合。
#
# 用法：
#   scripts/screenshot-baseline.sh             # 比对十一张整窗图
#   scripts/screenshot-baseline.sh --components # 只比对组件图
#   scripts/screenshot-baseline.sh --all        # 两者都跑
#   scripts/screenshot-baseline.sh --update [--components|--all]
#                                               # 人工确认后重新冻结
#   SHOT_JOBS=8 scripts/screenshot-baseline.sh --all
#   VERIFY_NO_SHOT_CACHE=1 scripts/screenshot-baseline.sh --all
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# 与 `scripts/verify.sh` 同一条理由（那边的注释写全了事故经过）：目标目录必须按
# 工作树隔离。共享的 `/tmp/zterm-check` 会让并行 worktree 互相覆盖
# `debug/term-desk`，比对的其实可能是别的分支渲染出来的 PNG。
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN="$TARGET/debug/term-desk"
# 验收脚本 4o 会扫 `scripts/*.sh` 里对二进制变量的执行；用派生名既保持同一路径，
# 又避免把哈希计算那一行误判成「无 --screenshot 拉起 GUI」。
SHOT_BIN="$BIN"
BASELINE_DIR="apps/desktop/tests/baseline"

UPDATE=0
MODE=full            # full | components | all
for arg in "$@"; do
  case "$arg" in
    --update) UPDATE=1 ;;
    --components) MODE=components ;;
    --all) MODE=all ;;
    "") ;;
    *) echo "未知参数：${arg}（可用：--update / --components / --all）" >&2; exit 2 ;;
  esac
done

cd "$ROOT" || exit 1

SHOT_JOBS="${SHOT_JOBS:-4}"
case "$SHOT_JOBS" in
  ''|*[!0-9]*) echo "SHOT_JOBS 必须是正整数，实际=${SHOT_JOBS}" >&2; exit 2 ;;
esac
if [ "$SHOT_JOBS" -lt 1 ]; then
  echo "SHOT_JOBS 必须 >= 1，实际=${SHOT_JOBS}" >&2
  exit 2
fi

if ! cargo build -p term-desk --target-dir "$TARGET" --quiet; then
  echo "✗ 构建失败，无法截图"
  exit 1
fi

if [ ! -x "$SHOT_BIN" ]; then
  echo "✗ 找不到可执行文件：$SHOT_BIN"
  exit 1
fi

# 每次运行一套独立临时目录：并行作业各有自己的 profile 与 PNG，杜绝共享状态。
RUN_DIR="$(mktemp -d /tmp/zterm-shots.XXXXXX)"
trap 'rm -rf "$RUN_DIR"' EXIT
JOBS_FILE="$RUN_DIR/jobs.txt"
: > "$JOBS_FILE"

NO_CACHE=0
[ "${VERIFY_NO_SHOT_CACHE:-0}" = "1" ] && NO_CACHE=1
CACHE_DIR="$TARGET/.verify-shot-cache"
if [ "$NO_CACHE" -eq 0 ] && [ "$UPDATE" -ne 1 ]; then
  mkdir -p "$CACHE_DIR" 2>/dev/null || true
fi

# 缓存键只取「决定像素的输入」：二进制（含全部编译期资产）、基线集合、本脚本、
# 夹具签名，外加一个给人手动失效用的环境变量。profile 路径是每次随机生成的空库，
# 不进键，否则缓存永远命中不了。
SHOT_BIN_HASH=$(shasum -a 256 "$SHOT_BIN" | cut -d' ' -f1)
SCRIPT_HASH=$(shasum -a 256 "$0" | cut -d' ' -f1)
BASELINE_HASH=$(find "$BASELINE_DIR" -name '*.png' -type f 2>/dev/null \
  | LC_ALL=C sort \
  | while IFS= read -r f; do shasum -a 256 "$f"; done \
  | shasum -a 256 | cut -d' ' -f1)

cache_key() {
  printf '%s\n' \
    'v1' "$SHOT_BIN_HASH" "$SCRIPT_HASH" "$BASELINE_HASH" \
    "${WINDTERM_SCREENSHOT_CACHE_EXTRA:-}" "$1" \
    | shasum -a 256 | cut -d' ' -f1
}

mark_cached() {
  [ "$NO_CACHE" -eq 1 ] && return 0
  : > "$CACHE_DIR/$1.ok" 2>/dev/null || true
}

fails=0

# check <基线文件名> [额外的应用参数...]
check() {
  local name="$1"
  shift
  local base="$BASELINE_DIR/$name"
  local out="$RUN_DIR/$name"
  local sig="full|$name|${SHOT_LANG:-en}|$*"
  local key
  key=$(cache_key "$sig")

  if [ "$UPDATE" -ne 1 ] && [ "$NO_CACHE" -eq 0 ] && [ -f "$CACHE_DIR/$key.ok" ]; then
    echo "✓ $name 与基线一致（缓存）"
    return 0
  fi

  # 夹具把语言钉死：默认 en，只有中文那张显式给 zh。否则用户 profile 里
  # 存了中文，42 张英文基线就会一起红——基线要锚在不随用户设置变的东西上。
  local profile
  profile=$(mktemp -d "$RUN_DIR/profile.XXXXXX")
  if ! env WINDTERM_PROFILE_DIR="$profile" WINDTERM_LANG="${SHOT_LANG:-en}" \
      "$SHOT_BIN" "$@" --screenshot "$out" >/dev/null 2>&1; then
    echo "✗ $name 截图失败"
    return 1
  fi
  if [ ! -f "$out" ]; then
    echo "✗ $name 没有产出 PNG"
    return 1
  fi

  if [ "$UPDATE" = "1" ]; then
    cp "$out" "$base"
    echo "↻ $name 基线已更新：$(shasum -a 256 "$base" | cut -d' ' -f1)"
    return 0
  fi

  if [ ! -f "$base" ]; then
    echo "✗ $name 基线不存在（先用 --update 生成）"
    return 1
  fi

  local want got
  want=$(shasum -a 256 "$base" | cut -d' ' -f1)
  got=$(shasum -a 256 "$out" | cut -d' ' -f1)
  if [ "$want" = "$got" ]; then
    mark_cached "$key"
    echo "✓ $name 与基线一致：${got}"
  else
    echo "✗ $name 不一致：基线=$want 实际=$got"
    echo "  截图在 ${out}；确认是预期改动后再用 --update 重新冻结"
    return 1
  fi
  return 0
}

COMP_DIR="$BASELINE_DIR/components"

# check_comp <组件名>：`--demo comp:<name>` 出一张图，与组件基线逐字节比对。
check_comp() {
  local name="$1"
  local base="$COMP_DIR/$name.png"
  local out="$RUN_DIR/comp-$name.png"
  local sig="comp|$name|en"
  local key
  key=$(cache_key "$sig")

  if [ "$UPDATE" -ne 1 ] && [ "$NO_CACHE" -eq 0 ] && [ -f "$CACHE_DIR/$key.ok" ]; then
    echo "✓ comp:$name 与基线一致（缓存）"
    return 0
  fi

  # 组件清单由二进制给出（`--list-components`），这里不再维护第二份名单。
  # 环境变量只是给 harness 一个「现在是夹具模式」的判据，与截图通道等价。
  local profile
  profile=$(mktemp -d "$RUN_DIR/profile.XXXXXX")
  if ! env WINDTERM_PROFILE_DIR="$profile" WINDTERM_COMP_DEMO="$name" \
      WINDTERM_LANG=en \
      "$SHOT_BIN" --demo "comp:$name" --screenshot "$out" >/dev/null 2>&1; then
    echo "✗ comp:$name 截图失败"
    return 1
  fi
  if [ ! -f "$out" ]; then
    echo "✗ comp:$name 没有产出 PNG"
    return 1
  fi

  if [ "$UPDATE" = "1" ]; then
    mkdir -p "$COMP_DIR"
    cp "$out" "$base"
    echo "↻ comp:$name 基线已更新：$(shasum -a 256 "$base" | cut -d' ' -f1)"
    return 0
  fi

  if [ ! -f "$base" ]; then
    echo "✗ comp:$name 基线不存在（先用 --update --components 生成）"
    return 1
  fi

  local want got
  want=$(shasum -a 256 "$base" | cut -d' ' -f1)
  got=$(shasum -a 256 "$out" | cut -d' ' -f1)
  if [ "$want" = "$got" ]; then
    mark_cached "$key"
    echo "✓ comp:$name 与基线一致：${got}"
  else
    echo "✗ comp:$name 不一致：基线=$want 实际=$got"
    echo "  截图在 ${out}；确认是预期改动后再用 --update --components 重新冻结"
    return 1
  fi
  return 0
}

add_job() {
  printf '%s\n' "$1" >> "$JOBS_FILE"
}

# 只跑「已冻结基线」的组件：还没修好的组件出图能看，但比对只会制造假红。
add_component_jobs() {
  local list list_profile
  # `--list-components` 在 eframe 之前 return，但 `main` 里的 startup_tidy 更早；
  # 不隔离 profile 就会在取名单时碰用户目录（实测复制过 2 个旧配置项）。
  list_profile="$RUN_DIR/list-profile"
  mkdir -p "$list_profile"
  list=$(env WINDTERM_PROFILE_DIR="$list_profile" "$SHOT_BIN" --list-components || true)
  if [ -z "$list" ]; then
    echo "✗ 取不到组件清单（--list-components 无输出）"
    fails=$((fails + 1))
    return
  fi
  while IFS="$(printf '\t')" read -r name state; do
    [ -n "$name" ] || continue
    [ "$state" = "gated" ] || continue
    case "$name" in
      *[!A-Za-z0-9_]*)
        echo "✗ 组件名含非预期字符，拒绝拼进作业脚本：$name"
        fails=$((fails + 1))
        continue
        ;;
    esac
    add_job "check_comp '$name'"
  done <<EOF
$list
EOF
}

# 分批并行：每满 SHOT_JOBS 个 wait 一次。bash 3.2 没有 wait -n，按批等待是最简
# 且可移植的做法；每作业把退出码写进独立文件，最后按名单顺序汇总输出。
run_jobs() {
  local line idx=0 rc
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    idx=$((idx + 1))
    (
      rc=0
      eval "$line" > "$RUN_DIR/job-$idx.log" 2>&1 || rc=$?
      printf '%s\n' "$rc" > "$RUN_DIR/job-$idx.rc"
    ) &
    if [ $((idx % SHOT_JOBS)) -eq 0 ]; then wait; fi
  done < "$JOBS_FILE"
  wait

  idx=0
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    idx=$((idx + 1))
    if [ -f "$RUN_DIR/job-$idx.log" ]; then
      cat "$RUN_DIR/job-$idx.log"
    fi
    rc=$(cat "$RUN_DIR/job-$idx.rc" 2>/dev/null || printf '1')
    if [ "$rc" -ne 0 ]; then
      fails=$((fails + 1))
    fi
  done < "$JOBS_FILE"
}

if [ "$MODE" = "full" ] || [ "$MODE" = "all" ]; then
  add_job 'check screenshot-round64.png'
  add_job 'check screenshot-scroll.png --demo scroll'
  # 软换行夹具：与 scroll 同一字节流、开启 Word Wrap，长行折到 wrap column。
  # 两张基线必须逐字节不同——这正是「开关有可见效果」的端到端判据。
  add_job 'check screenshot-wrap.png --demo wrap'
  # 彩虹括号夹具：含嵌套括号的字节流 + 彩虹着色，括号按深度上色。
  add_job 'check screenshot-rainbow.png --demo rainbow'
  add_job 'check screenshot-find.png --demo find'
  add_job 'check screenshot-word.png --demo word'
  add_job 'check screenshot-block.png --demo block'
  # 悬停预览窗夹具（`--demo preview`）。回看 6 行 + 轨道 50% 处冻结点亮
  # 橄榄预览框，框内是历史快照、框外是实时视口、滑块与状态栏不动。新增基线。
  add_job 'check screenshot-preview.png --demo preview'
  # Sysmon 展开态（`--demo sysmon`）：派发 TogglePanel 把终端底部细带翻成
  # T/C/M 读数条（原地切换，不再是右带面板）。fixture 模式把
  # 读数钉成视觉基线逐字值——守住三段的右对齐位置 / 颜色。
  add_job 'check screenshot-sysmon.png --demo sysmon'
  add_job 'check screenshot-dialog.png --demo newsession'
  # margin 边栏（时间戳 / 行号 / 符号三列 + 当前行强调）。
  # 必须单独一张：另外五张的字节流既没有 OSC 133 提示符也不折行，
  # 符号列在它们身上永远是空的——等于没有基线。
  add_job 'check screenshot-margin.png --demo margin'
  # 底带（一条带 + 标签条 + 齿轮菜单）。同样必须单独一张：
  # 其余夹具里 Sender / Shell 都是关着的，底带根本不画。
  add_job 'check screenshot-dock.png --demo dock'
  # 2026-11-10：Hex 视图两张。`bin` 守「环从会话建立那一刻就在收」（首行偏移
  # 必须是 0，切模式不是从零开始）；`binfull` 守上限与丢弃计数（1 MiB 环写满
  # 再溢出 4096，表头出现 `4096 bytes dropped`，首行偏移不再是 0）。
  # 这两件事都只在整窗里看得出来：组件图没有真实的几何宽度，量不出「夹列数」。
  add_job 'check screenshot-bin.png --demo bin'
  add_job 'check screenshot-binfull.png --demo binfull'
  # 中文界面单独一张：菜单 / 面板标题 / 快捷栏 / 状态栏 / 发送器工具行
  # 同时换字，标签宽度也会跟着变——没有这张，i18n 只有单测、没有视觉回归。
  add_job 'SHOT_LANG=zh check screenshot-zh.png --demo dock'
fi

if [ "$MODE" = "components" ] || [ "$MODE" = "all" ]; then
  add_component_jobs
fi

run_jobs

if [ "$fails" -eq 0 ]; then
  echo "视觉回归通过"
else
  echo "视觉回归失败：$fails 张图不一致"
fi
exit "$fails"
