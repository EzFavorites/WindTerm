#!/bin/bash
# TCP 汇聚端：收完整个流，回一行可断言的摘要。
#
# 为什么不是 `cat > /dev/null`：那样只能证明「连接没断」，证明不了「字节没丢、
# 没重排、没截断」。回 SHA256 + 长度之后，「发出去 1 MiB」与「收到 1 MiB」之间
# 才有一条可以直接对比的证据。
set -euo pipefail

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT

cat >"$tmp"
printf 'SHA256 %s SIZE %s\n' \
    "$(sha256sum "$tmp" | cut -d' ' -f1)" \
    "$(stat -c%s "$tmp")"
