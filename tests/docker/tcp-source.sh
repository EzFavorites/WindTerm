#!/bin/bash
# TCP 源端：一行 ASCII 头 + 原始字节。
#
# 头里带 SHA256 与长度，客户端读完自己算一遍就能判定「下载到的内容是不是
# 服务端那一次随机生成的那份」。把摘要放在同一连接里，避免再开一条 SSH
# 通道去取期望值——那样两条通道之间就有了时间窗，判据不再自洽。
set -euo pipefail

f=${PAYLOAD_FILE:-/payload/pattern.bin}
if [ ! -s "$f" ]; then
    echo "tcp-source: $f 不存在或为空" >&2
    exit 1
fi

printf 'SHA256 %s SIZE %s\n' \
    "$(sha256sum "$f" | cut -d' ' -f1)" \
    "$(stat -c%s "$f")"
exec cat "$f"
