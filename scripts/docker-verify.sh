#!/bin/bash
# Docker 真机验证：真 OpenSSH 服务端 + 真 tmux + 真二进制回显。
#
# 为什么需要这一层：`cargo test` 里的 e2e 用同一份 russh 起服务端，证明的是
# 「我们和自己兼容」；而用户连的是发行版自带的 sshd。握手、认证方法协商、
# `tmux -CC` 的控制模式输出、以及「1 MiB 随机字节过隧道后是否逐字节相同」
# 只能在真实进程里验。
#
# 用法：
#   bash scripts/docker-verify.sh                # 全跑
#   KEEP=1 bash scripts/docker-verify.sh         # 跑完留着容器，方便手工看
#   bash scripts/docker-verify.sh ssh control    # 只跑指定组（ssh / proxyjump / tunnel / tmux / control）
#
# 依赖：docker（或 OrbStack）在跑；本机有 ssh / ssh-keygen。
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMPOSE="$ROOT/tests/docker/docker-compose.yml"
KEYS="$ROOT/tests/docker/keys"
KEY="$KEYS/client/id_ed25519"
SELECTED=("$@")
[ ${#SELECTED[@]} -eq 0 ] && SELECTED=(ssh proxyjump tunnel tmux control)

say() { printf '\n\033[1m── %s ──\033[0m\n' "$1"; }
fail=0
note() { printf '  \033[31m✗\033[0m %s\n' "$1"; fail=$((fail + 1)); }
pass() { printf '  \033[32m✓\033[0m %s\n' "$1"; }

say "0. 前置检查"
if ! docker info >/dev/null 2>&1; then
    echo "  docker 没在跑（或当前用户没有权限）。这一层验证需要真的容器。" >&2
    exit 2
fi
pass "docker $(docker version --format '{{.Server.Version}}' 2>/dev/null || echo ok)"
command -v ssh >/dev/null && pass "ssh $(ssh -V 2>&1 | cut -d, -f1)" || { note "没有 ssh 客户端"; exit 2; }

# 密钥统一由 gen-keys.sh 负责：客户端密钥 + 每个服务独立的 host key + known_hosts。
# 不由本脚本自己 ssh-keygen，是为了和 protocol-verify.sh 共用同一份材料——
# 两套生成逻辑迟早会分叉（路径、注释、host key 数量）。
bash "$ROOT/tests/docker/gen-keys.sh" >/dev/null
pass "仓库内测试密钥就绪（${KEY}）"

cleanup() {
    if [ "${KEEP:-0}" = "1" ]; then
        echo "  （KEEP=1：容器保留。手工清理：docker compose -f $COMPOSE down -v）"
    else
        docker compose -f "$COMPOSE" down -v >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

say "1. 构建并启动 target / bastion"
# 直接用 `docker build`：BuildKit 会往 ~/.docker/buildx 写活动时间戳，受限沙箱里
# 这一步可能被拒；而 `compose build` 在退回旧构建器时还会去碰钥匙串。
# 镜像建好之后 `compose up` 只会用它，不再触发构建。
IMAGE=windterm-test-sshd:latest
if ! docker build -t "$IMAGE" "$ROOT/tests/docker"; then
    echo "  BuildKit 失败，退回旧构建器重试……"
    if ! DOCKER_BUILDKIT=0 docker build -t "$IMAGE" "$ROOT/tests/docker"; then
        echo "  构建失败：这一步需要能拉取 alpine 镜像。" >&2
        exit 2
    fi
fi
if ! docker compose -f "$COMPOSE" up -d --wait --force-recreate target bastion; then
    note "容器没能在 60 秒内就绪"
    docker compose -f "$COMPOSE" logs --tail 30
    exit 2
fi
pass "target / bastion 已 healthy"

wait_port() {
    local port=$1 name=$2
    for _ in $(seq 1 60); do
        if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
            exec 3<&- 3>&-
            pass "$name 就绪（127.0.0.1:${port}）"
            return 0
        fi
        sleep 0.5
    done
    note "$name 在 30 秒内没有监听 127.0.0.1:${port}"
    docker compose -f "$COMPOSE" logs --tail 20
    return 1
}
wait_port 22022 target || exit 2
wait_port 22023 bastion || exit 2

export WINDTERM_DOCKER_SSH=1
export WINDTERM_DOCKER_HOST=127.0.0.1
export WINDTERM_DOCKER_PORT=22022
export WINDTERM_DOCKER_BASTION_PORT=22023
export WINDTERM_DOCKER_USER=tester
export WINDTERM_DOCKER_PASS=testerpass
export WINDTERM_DOCKER_KEY="$KEY"
export WINDTERM_DOCKER_TARGET_NAME=target
export WINDTERM_DOCKER_COMPOSE="$COMPOSE"

run_group() {
    local group=$1
    case "$group" in
    ssh)
        say "2. SSH 握手与认证（真 sshd）"
        if cargo test -p term-ssh --test docker_sshd --target-dir "$ROOT/target" \
            -- --ignored --test-threads=1 --nocapture; then
            pass "term-ssh：密码 / 公钥 / 交互认证 / TOFU 全过"
        else
            note "term-ssh docker_sshd 失败"
        fi
        ;;
    proxyjump)
        say "3. ProxyJump 真实链路（系统 ssh 的 -J）"
        if cargo test -p term-desk --test docker_proxyjump --target-dir "$ROOT/target" \
            -- --ignored --test-threads=1 --nocapture; then
            pass "spawn_plan 的 argv 经跳板机连上了目标机"
        else
            note "docker_proxyjump 失败"
        fi
        ;;
    tunnel)
        say "4. 隧道二进制完整性（1 MiB 随机字节）"
        if cargo test -p term-tunnel --test docker_sshd --target-dir "$ROOT/target" \
            -- --ignored --test-threads=1 --nocapture; then
            pass "term-tunnel：经真 sshd 的本地转发逐字节一致"
        else
            note "term-tunnel docker_sshd 失败"
        fi
        ;;
    tmux)
        say "5. tmux -CC 真实控制模式"
        if cargo test -p term-desk --test docker_tmux --target-dir "$ROOT/target" -F term-app/planned \
            -- --ignored --test-threads=1 --nocapture; then
            pass "term-app::tmux 解析器吃到了真的控制模式输出"
        else
            note "docker_tmux 失败"
        fi
        ;;
    control)
        say "6. W1-c 控制面本体：desk script 经通道工厂连真 sshd"
        if cargo test -p term-desk --test docker_control_plane --target-dir "$ROOT/target" \
            -- --ignored --test-threads=1 --nocapture; then
            pass "会话库 → SessionPool::open → 工厂 → 真 sshd 回显全链路通过"
        else
            note "docker_control_plane 失败"
        fi
        # ADR-0027 判据 ③：脚本的 sftp_* 原语真传一个文件（同一批会话/凭据，
        # 只是端口换成 SFTP）。2026-11-18 `Client::put` 参数传反就是这一步漏掉
        # 才没被发现，所以单独留一条真机判据，别只靠内存假实现。
        say "7. ADR-0027 ③：脚本 sftp_put / sftp_get 真机往返"
        if cargo test -p term-desk --test docker_script_sftp --target-dir "$ROOT/target" \
            -- --ignored --test-threads=1 --nocapture; then
            pass "sftp_put / sftp_stat / sftp_get / sftp_remove 在真 sftp-server 上通过"
        else
            note "docker_script_sftp 失败"
        fi
        ;;
    *)
        note "不认识的分组：${group}（可选 ssh / proxyjump / tunnel / tmux / control）"
        ;;
    esac
}

for g in "${SELECTED[@]}"; do run_group "$g"; done

say "汇总"
if [ "$fail" -eq 0 ]; then
    printf '  \033[32m全部通过\033[0m：这一层验证的是「与真实 sshd / tmux 互操作」，不是我们自己的 mock。\n'
    exit 0
fi
printf '  \033[31m%s 组失败\033[0m（上面每组都留了 cargo 输出）\n' "$fail"
exit 1
