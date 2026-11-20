#!/bin/bash
# 测试专用密钥生成器（幂等）。
#
# 为什么要有这个脚本，而不是直接 `ssh-keygen` 了事：
#
# 1. **不碰系统密钥。** 测试只认 `tests/docker/keys/` 下的密钥。客户端调用一律
#    `-F /dev/null -i <这里的私钥> -o IdentityAgent=none -o IdentitiesOnly=yes`，
#    宿主机的 `~/.ssh/config`、`ssh-agent`、macOS 钥匙串都被排除在外。否则第一次
#    跑测试就会弹「ssh 想使用钥匙串中的密钥」授权框，无人值守直接破功。
#
# 2. **host key 要固定，才能预置 known_hosts。** 每个服务一把独立的 host key
#    （target / bastion 互不相同）。固定下来之后客户端可以用
#    `StrictHostKeyChecking=yes` + 仓库内 known_hosts 做严格校验，全程零交互，
#    而且「连的是不是我预期的机器」这件事真的被验证了。
#
# 3. **同时要能故意换钥匙。** `gen-keys.sh rotate <service>` 强制重生成，用于
#    「host key 变了必须拒绝」这条负向测试。固定不等于不可变。
#
# 用法：
#   bash tests/docker/gen-keys.sh                # 补齐缺失的密钥
#   bash tests/docker/gen-keys.sh rotate target  # 重生成 target 的 host key
#   bash tests/docker/gen-keys.sh show           # 打印指纹，便于排查
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
KEYS="$ROOT/keys"

# 服务名 → host key 目录。新增一个服务就在这里加一行，别在别处硬编码路径。
SERVICES=(target bastion)

mkdir -p "$KEYS"
chmod 700 "$KEYS"

# ed25519 而非 rsa：生成快（rsa 4096 在 CI 上要几百毫秒）、体积小、OpenSSH 8+
# 默认优先。测试不关心算法矩阵，只关心「认证能不能过」。
gen_key() {
    local path=$1 comment=$2
    mkdir -p "$(dirname "$path")"
    if [ ! -f "$path" ]; then
        ssh-keygen -q -t ed25519 -N '' -C "$comment" -f "$path" >/dev/null
    fi
    chmod 600 "$path"
    chmod 644 "$path.pub"
}

ensure_all() {
    gen_key "$KEYS/client/id_ed25519" 'windterm-test-client'
    local svc
    for svc in "${SERVICES[@]}"; do
        mkdir -p "$KEYS/$svc"
        chmod 700 "$KEYS/$svc"
        gen_key "$KEYS/$svc/ssh_host_ed25519_key" "windterm-test-host-$svc"
    done
}

# known_hosts 的键必须写成 `[host]:port`，因为端口不是 22。写成裸 host 的话
# ssh 会去匹配 22 端口那一条，然后在严格校验模式下报「找不到主机」。
write_known_hosts() {
    local out=$1 tmp
    tmp=$(mktemp)
    local svc port
    for svc in "${SERVICES[@]}"; do
        case "$svc" in
        target) port=22022 ;;
        bastion) port=22023 ;;
        *) continue ;;
        esac
        # 只取类型与 base64，丢掉注释尾巴：注释里带服务名会让 diff 噪声变大。
        printf '[127.0.0.1]:%s %s\n' "$port" \
            "$(cut -d' ' -f1,2 "$KEYS/$svc/ssh_host_ed25519_key.pub")" >>"$tmp"
    done
    mv "$tmp" "$out"
    chmod 644 "$out"
}

case "${1:-ensure}" in
ensure)
    ensure_all
    write_known_hosts "$KEYS/known_hosts"
    echo "keys: $KEYS （client + ${SERVICES[*]}，known_hosts 已同步）"
    ;;
rotate)
    svc=${2:-}
    if [ -z "$svc" ] || [ ! -d "$KEYS/$svc" ]; then
        echo "用法：gen-keys.sh rotate <${SERVICES[*]}>" >&2
        exit 2
    fi
    rm -f "$KEYS/$svc/ssh_host_ed25519_key" "$KEYS/$svc/ssh_host_ed25519_key.pub"
    gen_key "$KEYS/$svc/ssh_host_ed25519_key" "windterm-test-host-$svc"
    echo "已重生成 $svc 的 host key（known_hosts 尚未更新，负向测试正需要这个错位）"
    ;;
show)
    for f in "$KEYS"/client/id_ed25519.pub "$KEYS"/*/ssh_host_ed25519_key.pub; do
        [ -f "$f" ] || continue
        printf '%-52s %s\n' "${f#"$KEYS"/}" "$(ssh-keygen -lf "$f" | awk '{print $2}')"
    done
    ;;
*)
    echo "用法：gen-keys.sh [ensure|rotate <service>|show]" >&2
    exit 2
    ;;
esac
