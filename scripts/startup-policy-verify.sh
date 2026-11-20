#!/usr/bin/env bash
# W1-a：Startup 三档策略的实机回归（出图比哈希，不抢前台）。
#
#   scripts/startup-policy-verify.sh
#   CARGO_TARGET_DIR=/tmp/zterm-check scripts/startup-policy-verify.sh
#
# 纯层单测（`term-app::startup`）只能证明「策略函数算得对」，证明不了
# 「启动时真的按策略摆回标签」。这里造一个一次性 profile：layout.json 里
# 记两条本地 shell 标签 + 一棵分屏树，然后分别用 none / last / specific
# 三档各跑**两次** `--screenshot`，比 PNG 哈希。
#
# 判据（都不是绝对值，字体/主题升级不会误报）：
#   · 同档两次出图哈希一致（确定性）；
#   · none != last（NoSession 不恢复、LastSessions 恢复两条并分屏）；
#   · specific != last（指定档不照搬上次的分屏结构）；
#   · 跑完 layout.json 仍有两条 tabs —— 这条是 2026-11-09 实测抓到的 bug：
#     原先非 LastSessions 档退出时会把「此刻只有一条默认 shell」写回布局，
#     抹掉上次的会话记录，导致下一轮验证三张图全相同、假绿。
#
# 纪律：一次性 profile + 一律 `--screenshot`（不可见窗口 + Prohibited），
# 不弹窗、不抢前台；门禁 4o 守着这一点。不进 verify.sh：它需要 GUI 出图，
# 而门禁要在没有窗口系统的机器上也能跑完。
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN="$TARGET/debug/term-desk"
OUT_DIR="${STARTUP_VERIFY_OUT:-/tmp/zterm-startup-policy}"

cd "$ROOT" || exit 1
mkdir -p "$OUT_DIR"

if ! cargo build -p term-desk --target-dir "$TARGET" --quiet; then
    echo "✗ 构建失败，无法出图" >&2
    exit 1
fi
if [ ! -x "$BIN" ]; then
    echo "✗ 找不到 term-desk 构建产物（检查 CARGO_TARGET_DIR）" >&2
    exit 1
fi

PROFILE="$(mktemp -d /tmp/zterm-startup-profile.XXXXXX)"
trap 'rm -rf "$PROFILE"' EXIT

sha() { shasum -a 256 "$1" | cut -d' ' -f1; }

# 每次运行前重建输入：上一轮退出会写 layout.json，不隔离就测不出差异。
write_fixture() {
    MODE="$1" SESSION="$2" PROFILE="$PROFILE" python3 - <<'PY'
import json, os, pathlib
p = pathlib.Path(os.environ["PROFILE"])
(p / "layout.json").write_text(json.dumps({
    "tabs": [{"name": "Local Shell"}, {"name": "Local Shell"}],
    "active": 1,
    "tree": {"Split": {"horizontal": True,
                       "first": {"Leaf": 0}, "second": {"Leaf": 1}}},
    "dock": [],
}))
prefs = {"startup_mode": os.environ["MODE"]}
if os.environ["SESSION"]:
    prefs["startup_sessions"] = [os.environ["SESSION"]]
(p / "prefs.json").write_text(json.dumps(prefs))
PY
}

for spec in 'none|' 'last|' 'specific|Local Shell'; do
    mode="${spec%%|*}"
    session="${spec##*|}"
    for run in 1 2; do
        write_fixture "$mode" "$session" || exit 1
        png="$OUT_DIR/$mode-$run.png"
        if ! env WINDTERM_PROFILE_DIR="$PROFILE" WINDTERM_LANG=en \
             "$BIN" --screenshot "$png" >/dev/null 2>&1; then
            echo "✗ $mode run$run 出图失败" >&2
            exit 1
        fi
        printf '  %-9s run%s  %s\n' "$mode" "$run" "$(sha "$png" | cut -c1-16)"
    done
    # 不用关联数组：macOS 自带 bash 3.2 没有它，直接比文件更兼容。
    if [ "$(sha "$OUT_DIR/$mode-1.png")" != "$(sha "$OUT_DIR/$mode-2.png")" ]; then
        echo "✗ $mode 两次出图不一致（非确定性）" >&2
        exit 1
    fi
done

last="$(sha "$OUT_DIR/last-1.png")"
if [ "$(sha "$OUT_DIR/none-1.png")" = "$last" ]; then
    echo "✗ NoSession 与 LastSessions 画面相同：恢复策略没有生效" >&2
    exit 1
fi
if [ "$(sha "$OUT_DIR/specific-1.png")" = "$last" ]; then
    echo "✗ SpecificSessions 与 LastSessions 画面相同：指定档照搬了旧布局" >&2
    exit 1
fi

tabs=$(python3 -c "
import json, pathlib
print(len(json.loads(pathlib.Path('$PROFILE/layout.json').read_text())['tabs']))")
if [ "$tabs" != "2" ]; then
    echo "✗ 非 LastSessions 档覆写了布局（layout.json 只剩 $tabs 条），上次的会话记录被抹掉" >&2
    exit 1
fi

echo "✓ 三档策略实机验证通过：同档确定性 / none≠last / specific≠last / 布局未被覆写"
echo "  说明：本夹具里 specific 指定唯一一条 Local Shell，与 none 的默认单标签"
echo "        画面相同是预期的；「只开点名的会话」由纯层判据覆盖。"
