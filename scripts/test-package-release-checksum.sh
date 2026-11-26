#!/usr/bin/env bash
# package-release.sh 校验和强制项的 shell 级自测（无新增依赖）。
# 用法：scripts/test-package-release-checksum.sh
#
# 从主脚本直接抽取 sha256_of / require_sha256_tool 的函数体来测，保证测到的
# 就是实际打包用的实现，不复制出第二份逻辑。
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT/scripts/package-release.sh"
[ -f "$SCRIPT" ] || { echo "找不到 $SCRIPT" >&2; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

sed -n '/^require_sha256_tool() {/,/^}/p' "$SCRIPT" > "$TMP/require_fn.sh"
sed -n '/^sha256_of() {/,/^}/p' "$SCRIPT" > "$TMP/sha_fn.sh"
grep -q 'require_sha256_tool() {' "$TMP/require_fn.sh" || { echo "抽取 require_sha256_tool 失败" >&2; exit 1; }
grep -q 'sha256_of() {' "$TMP/sha_fn.sh" || { echo "抽取 sha256_of 失败" >&2; exit 1; }

fail=0
pass() { echo "PASS: $1"; }
bad()  { echo "FAIL: $1" >&2; fail=1; }

# ---- 1. 受控 PATH（空目录，无任何哈希工具）：sha256_of 必须硬失败且不落盘 ----
EMPTY_PATH="$TMP/empty-path"; mkdir -p "$EMPTY_PATH"
OUT1="$TMP/out1"; mkdir -p "$OUT1"
printf 'payload' > "$OUT1/payload.bin"
set +e
bash -c '
  PATH="$1"; export PATH
  OUT="$2"; export OUT
  source "$3"
  sha256_of "payload.bin"
' _ "$EMPTY_PATH" "$OUT1" "$TMP/sha_fn.sh"
rc=$?
set -e
[ "$rc" -ne 0 ] && pass "无哈希工具时 sha256_of 退出码非 0（实际 ${rc}）" || bad "无哈希工具时 sha256_of 意外成功"
[ ! -e "$OUT1/payload.bin.sha256" ] && pass "无哈希工具时未产出 .sha256" || bad "无哈希工具时产出了 .sha256"

# ---- 2. 同样受控 PATH：打包前的 preflight 必须硬失败（先于任何产物落盘） ----
set +e
bash -c '
  PATH="$1"; export PATH
  source "$2"
  require_sha256_tool
' _ "$EMPTY_PATH" "$TMP/require_fn.sh"
rc=$?
set -e
[ "$rc" -ne 0 ] && pass "无哈希工具时 preflight 退出码非 0（实际 ${rc}）" || bad "无哈希工具时 preflight 意外成功"

# ---- 3. 正常 PATH：临时文件能生成 .sha256 ----
OUT3="$TMP/out3"; mkdir -p "$OUT3"
printf 'hello-checksum' > "$OUT3/artifact.bin"
tool=""
for c in sha256sum shasum openssl; do
  command -v "$c" >/dev/null 2>&1 && { tool="$c"; break; }
done
[ -n "$tool" ] || bad "当前 PATH 没有可用的 SHA-256 工具，无法跑正常用例"
set +e
bash -c '
  OUT="$1"; export OUT
  source "$2"
  sha256_of "artifact.bin"
' _ "$OUT3" "$TMP/sha_fn.sh"
rc=$?
set -e
if [ "$rc" -eq 0 ] && [ -s "$OUT3/artifact.bin.sha256" ]; then
  pass "正常 PATH（首个命中：${tool}）生成 .sha256：$(cat "$OUT3/artifact.bin.sha256")"
else
  bad "正常 PATH 下未生成 .sha256（rc=${rc}）"
fi

# ---- 4. 优先级：sha256sum 存在时优先于 shasum（保持既有 sha256sum → shasum → openssl） ----
STUB_PATH="$TMP/stub-path"; mkdir -p "$STUB_PATH"
printf '#!/bin/sh\nprintf "used-sha256sum\\n"\n' > "$STUB_PATH/sha256sum"
printf '#!/bin/sh\nprintf "used-shasum\\n"\n' > "$STUB_PATH/shasum"
chmod +x "$STUB_PATH/sha256sum" "$STUB_PATH/shasum"
OUT4="$TMP/out4"; mkdir -p "$OUT4"
printf 'x' > "$OUT4/prio.bin"
set +e
bash -c '
  PATH="$1"; export PATH
  OUT="$2"; export OUT
  source "$3"
  sha256_of "prio.bin"
' _ "$STUB_PATH" "$OUT4" "$TMP/sha_fn.sh"
rc=$?
set -e
if [ "$rc" -eq 0 ] && grep -q 'used-sha256sum' "$OUT4/prio.bin.sha256" 2>/dev/null; then
  pass "优先级正确：sha256sum 先于 shasum 被使用"
else
  bad "优先级异常（rc=${rc}，内容=$(cat "$OUT4/prio.bin.sha256" 2>/dev/null)）"
fi

[ "$fail" -eq 0 ] || { echo "自测失败" >&2; exit 1; }
echo "全部自测通过"
