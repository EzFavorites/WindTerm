#!/usr/bin/env bash
# M0 / G6：字形样张「出图 → 像素判据」的一键编排。
#
#   scripts/glyph-probe.sh                       # 结果落到 /tmp/zterm-glyph
#   GLYPH_OUT_DIR=docs/gate scripts/glyph-probe.sh   # 把报告与样张留档
#
# 两条纪律（与 screenshot-baseline.sh 同源）：
#   · 一次性 profile 目录，避免截图运行读用户的 sessions.db；
#   · 一律 `--screenshot`（不可见窗口 + Prohibited 激活策略），**不弹窗、
#     不抢前台**。门禁 4o 守着这一点。
#
# 分工：这一层只跑「一次出图 + 一次数值分析」，不进 verify.sh——它依赖
# Pillow/numpy 与 GUI 出图，而门禁要在没有这些的机器上也能跑完。
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN="$TARGET/debug/term-desk"
OUT_DIR="${GLYPH_OUT_DIR:-/tmp/zterm-glyph}"
PROBE="$ROOT/scripts/glyph-probe.py"

cd "$ROOT" || exit 1
mkdir -p "$OUT_DIR"

if ! cargo build -p term-desk --target-dir "$TARGET" --quiet; then
    echo "✗ 构建失败，无法出图" >&2
    exit 1
fi
if [ ! -x "$BIN" ]; then
    echo "✗ 找不到可执行文件：$BIN" >&2
    exit 1
fi

PROFILE="$(mktemp -d /tmp/zterm-glyph-profile.XXXXXX)"
trap 'rm -rf "$PROFILE"' EXIT

PNG="$OUT_DIR/sample.png"
if ! env WINDTERM_PROFILE_DIR="$PROFILE" WINDTERM_LANG=en \
     "$BIN" --demo glyph --screenshot "$PNG" >/dev/null 2>&1; then
    echo "✗ 样张出图失败" >&2
    exit 1
fi
if [ ! -f "$PNG" ]; then
    echo "✗ 没有产出 PNG" >&2
    exit 1
fi

python3 "$PROBE" "$PNG" \
    --json "$OUT_DIR/report.json" \
    --crop "$OUT_DIR/sample-zoom.png"
