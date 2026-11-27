#!/usr/bin/env bash
# package-release.sh 校验和强制项的 shell 级自测（无新增依赖）。
# 用法：scripts/test-package-release-checksum.sh
#
# 从主脚本直接抽取 sha256_of / require_sha256_tool 的函数体来测，保证测到的
# 就是实际打包用的实现，不复制出第二份逻辑。
# 覆盖：工具全无硬失败、preflight、正常 PATH、三路工具的优先级、shasum -a 256
# 与 openssl dgst 两支各自的格式（64 hex + 两空格 + 文件名）。
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
pass_count=0
expect=6
pass() { pass_count=$((pass_count + 1)); echo "PASS: $1"; }
bad()  { echo "FAIL: $1" >&2; fail=1; }

# 三个 64 位十六进制标记：各自唯一，靠它判断实际走了哪一支工具。
HEX_SHA=$(printf '%064d' 1 | tr '0' '1')
HEX_SHASUM=$(printf '%064d' 2 | tr '0' '2')
HEX_OPENSSL=$(printf '%064d' 3 | tr '0' '3')

# 受控 PATH：只把非哈希工具（openssl 分支要用 sed）软链进来，系统里的
# sha256sum / shasum / openssl 一律不露面。
stub_dir() {
  local dir="$1" sed_path
  mkdir -p "$dir"
  sed_path="$(command -v sed || true)"
  [ -n "$sed_path" ] && ln -s "$sed_path" "$dir/sed"
}

make_sha256sum_stub() {
  cat > "$1/sha256sum" <<EOF
#!/bin/sh
printf '%s  %s\n' "$HEX_SHA" "\$1"
EOF
  chmod +x "$1/sha256sum"
}

make_shasum_stub() {
  # 真实调用形如 `shasum -a 256 file`：文件名是 $3。
  cat > "$1/shasum" <<EOF
#!/bin/sh
printf '%s  %s\n' "$HEX_SHASUM" "\$3"
EOF
  chmod +x "$1/shasum"
}

make_openssl_stub() {
  # 真实调用形如 `openssl dgst -sha256 file`，主脚本用 sed 's/.*= //' 取摘要。
  cat > "$1/openssl" <<EOF
#!/bin/sh
printf 'SHA2-256(%s)= %s\n' "\$3" "$HEX_OPENSSL"
EOF
  chmod +x "$1/openssl"
}

run_sha256_of() {
  bash -c '
    PATH="$1"; export PATH
    OUT="$2"; export OUT
    source "$3"
    sha256_of "$4"
  ' _ "$1" "$2" "$TMP/sha_fn.sh" "$3"
}

# ---- 1. 受控 PATH（空目录，无任何哈希工具）：sha256_of 硬失败且不落盘 ----
EMPTY_PATH="$TMP/empty-path"; mkdir -p "$EMPTY_PATH"
OUT1="$TMP/out1"; mkdir -p "$OUT1"
printf 'payload' > "$OUT1/payload.bin"
run_sha256_of "$EMPTY_PATH" "$OUT1" "payload.bin"
rc=$?
if [ "$rc" -ne 0 ] && [ ! -e "$OUT1/payload.bin.sha256" ]; then
  pass "无哈希工具时 sha256_of 硬失败且不落盘（rc=${rc}）"
else
  bad "无哈希工具时意外成功或落盘了 .sha256（rc=${rc}）"
fi

# ---- 2. 同样受控 PATH：打包前的 preflight 必须硬失败（先于任何产物落盘） ----
bash -c '
  PATH="$1"; export PATH
  source "$2"
  require_sha256_tool
' _ "$EMPTY_PATH" "$TMP/require_fn.sh"
rc=$?
if [ "$rc" -ne 0 ]; then
  pass "无哈希工具时 preflight 退出码非 0（实际 ${rc}）"
else
  bad "无哈希工具时 preflight 意外成功"
fi

# ---- 3. 正常 PATH：生成 .sha256，且是 64 hex + 两个空格 + 文件名 ----
OUT3="$TMP/out3"; mkdir -p "$OUT3"
printf 'hello-checksum' > "$OUT3/artifact.bin"
tool=""
for c in sha256sum shasum openssl; do
  command -v "$c" >/dev/null 2>&1 && { tool="$c"; break; }
done
[ -n "$tool" ] || bad "当前 PATH 没有可用的 SHA-256 工具，无法跑正常用例"
run_sha256_of "$PATH" "$OUT3" "artifact.bin"
rc=$?
line3="$(cat "$OUT3/artifact.bin.sha256" 2>/dev/null)"
if [ "$rc" -eq 0 ] && printf '%s\n' "$line3" | grep -Eq '^[0-9a-f]{64}  artifact\.bin$'; then
  pass "正常 PATH（首个命中：${tool}）生成两空格格式 .sha256：$line3"
else
  bad "正常 PATH 下未生成合规 .sha256（rc=${rc}，内容=$line3）"
fi

# ---- 4. 优先级：sha256sum > shasum > openssl，三支同时在场时只用第一支 ----
ALL_PATH="$TMP/all-path"; stub_dir "$ALL_PATH"
make_sha256sum_stub "$ALL_PATH"
make_shasum_stub "$ALL_PATH"
make_openssl_stub "$ALL_PATH"
OUT4="$TMP/out4"; mkdir -p "$OUT4"
printf 'x' > "$OUT4/prio.bin"
run_sha256_of "$ALL_PATH" "$OUT4" "prio.bin"
rc=$?
line4="$(cat "$OUT4/prio.bin.sha256" 2>/dev/null)"
if [ "$rc" -eq 0 ] && [ "$line4" = "$HEX_SHA  prio.bin" ]; then
  pass "优先级正确：sha256sum > shasum > openssl（两空格格式）"
else
  bad "优先级异常（rc=${rc}，内容=$line4）"
fi

# ---- 5. shasum -a 256 单独在场：格式与两空格 ----
SHASUM_PATH="$TMP/shasum-path"; stub_dir "$SHASUM_PATH"
make_shasum_stub "$SHASUM_PATH"
OUT5="$TMP/out5"; mkdir -p "$OUT5"
printf 'y' > "$OUT5/shasum.bin"
run_sha256_of "$SHASUM_PATH" "$OUT5" "shasum.bin"
rc=$?
line5="$(cat "$OUT5/shasum.bin.sha256" 2>/dev/null)"
if [ "$rc" -eq 0 ] && [ "$line5" = "$HEX_SHASUM  shasum.bin" ]; then
  pass "shasum -a 256 分支：64 hex + 两空格 + 文件名"
else
  bad "shasum 分支格式异常（rc=${rc}，内容=$line5）"
fi

# ---- 6. openssl dgst 单独在场：格式与两空格（含 sed 抽摘要） ----
OPENSSL_PATH="$TMP/openssl-path"; stub_dir "$OPENSSL_PATH"
make_openssl_stub "$OPENSSL_PATH"
OUT6="$TMP/out6"; mkdir -p "$OUT6"
printf 'z' > "$OUT6/openssl.bin"
run_sha256_of "$OPENSSL_PATH" "$OUT6" "openssl.bin"
rc=$?
line6="$(cat "$OUT6/openssl.bin.sha256" 2>/dev/null)"
if [ "$rc" -eq 0 ] && [ "$line6" = "$HEX_OPENSSL  openssl.bin" ]; then
  pass "openssl dgst 分支：64 hex + 两空格 + 文件名"
else
  bad "openssl 分支格式异常（rc=${rc}，内容=$line6）"
fi

[ "$fail" -eq 0 ] || { echo "自测失败（${pass_count}/${expect} PASS）" >&2; exit 1; }
echo "全部自测通过（${pass_count}/${expect} PASS）"
