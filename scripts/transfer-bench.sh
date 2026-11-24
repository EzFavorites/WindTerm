#!/usr/bin/env bash
# 传输基准驱动：起真 sshd 容器 → 用本项目 term-sftp 客户端成批跑本地 / put / get → 出对比报告。
#
# 为什么不用系统 `sftp` 命令：那条路测的是 OpenSSH 客户端的吞吐，与本仓代码无关。
# 被测实现是 `crates/term-sftp/examples/transfer-bench.rs`，它调用的是产品同款
# `term_sftp::blocking::Client`（与脚本端口 `sftp_get` / `sftp_put` 共用传输层）。
#
# 用法：
#   bash scripts/transfer-bench.sh                          # 8 MiB × 3 轮，报告落 /tmp
#   bash scripts/transfer-bench.sh --size-mb 32 --rounds 5  # 加大载荷 / 轮次
#   bash scripts/transfer-bench.sh --small-count 256        # 小文件组的 N
#   bash scripts/transfer-bench.sh --out docs/bench/transfer-report.md
#   KEEP=1 bash scripts/transfer-bench.sh                   # 跑完留容器
#   SKIP_BUILD=1 bash scripts/transfer-bench.sh             # 复用已有镜像
#
# 不挂 `scripts/verify.sh` 的 step：那会把 verify_steps 从 38 改到 39，必须同步 facts.txt。
# 本脚本人工运行；`scripts/check-docs.sh` 也不统计 scripts/ 下的脚本数量。
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DOCKER_DIR="$ROOT/tests/docker"
COMPOSE="$DOCKER_DIR/docker-compose.yml"
# 密钥每次现生成到一次性目录，与 `docker-verify.sh` 同一口径：不碰系统密钥、
# 不留长期凭据、不受上一次残留影响。`KEEP=1` 时保留并打印路径。
KEYS=$(mktemp -d "${TMPDIR:-/tmp}/1zterm-docker-keys.XXXXXX")
export WINDTERM_TEST_KEYS_DIR="$KEYS"
CLIENT_KEY="$KEYS/client/id_ed25519"
# compose 的挂载源跟着一次性目录走（默认值仍是仓库内 keys/）。
export TEST_CLIENT_KEYS="$KEYS/client"
export TEST_TARGET_KEYS="$KEYS/target"
export TEST_BASTION_KEYS="$KEYS/bastion"

HOST=127.0.0.1
PORT=22022
USER_NAME=tester
PASS=testerpass

SIZE_MB=8
ROUNDS=3
SMALL_COUNT=128
OUT=/tmp/windterm-transfer-report.md
WORK=/tmp/windterm-transfer-bench
while [ $# -gt 0 ]; do
    case "$1" in
        --size-mb) SIZE_MB=$2; shift 2 ;;
        --rounds)  ROUNDS=$2;  shift 2 ;;
        --small-count) SMALL_COUNT=$2; shift 2 ;;
        --out)     OUT=$2;     shift 2 ;;
        --work-dir) WORK=$2;   shift 2 ;;
        -h|--help) sed -n '2,18p' "$0"; exit 0 ;;
        *) printf '未知参数: %s\n' "$1" >&2; exit 2 ;;
    esac
done

if ! command -v docker >/dev/null 2>&1; then
    echo "需要 docker（本脚本起的是发行版 sshd + sftp-server 真对端）" >&2
    exit 2
fi
if ! command -v nc >/dev/null 2>&1; then
    echo "需要 nc（等待容器端口就绪）" >&2
    exit 2
fi

# 密钥在运行时生成，不入库（公开仓库不得含私钥材料）。
bash "$DOCKER_DIR/gen-keys.sh" >/dev/null || { echo "gen-keys.sh 失败" >&2; exit 2; }
[ -f "$CLIENT_KEY" ] || { echo "客户端密钥没生成" >&2; exit 2; }

if [ "${SKIP_BUILD:-0}" != 1 ]; then
    docker build -q -t windterm-test-sshd:latest "$DOCKER_DIR" >/dev/null || {
        echo "镜像构建失败（需要能拉取 debian:12-slim 与 apt 源）" >&2; exit 2;
    }
fi

# --force-recreate：镜像重建后必须换容器，否则 entrypoint 还是旧的，
# 于是「刚改完却测不出变化」——这类假阴性排查起来最费时间。
# 这里的 `|| true` 是必须的：compose 里有一个一次性 client 容器，跑完就退出 0，
# 于是 `up --wait` 会返回 1，即使四个 sshd 服务全部 healthy（实测）。
# 所以判据用下面的端口探测，而不是 compose 的退出码。
docker compose -f "$COMPOSE" up -d --wait --force-recreate >/dev/null 2>&1 || true

n=0
until nc -z "$HOST" "$PORT" 2>/dev/null; do
    n=$((n + 1))
    if [ "$n" -ge 60 ]; then
        echo "端口 $PORT 60 次探测后仍未就绪" >&2
        docker compose -f "$COMPOSE" ps >&2
        exit 2
    fi
    sleep 0.5
done

# 与 crates/term-ssh/tests/docker_sshd.rs 同一组变量名，避免基准与真机测试各有口径。
export WINDTERM_DOCKER_HOST=$HOST
export WINDTERM_DOCKER_PORT=$PORT
export WINDTERM_DOCKER_USER=$USER_NAME
export WINDTERM_DOCKER_PASS=$PASS
export WINDTERM_DOCKER_KEY=$CLIENT_KEY

cleanup() {
    if [ "${KEEP:-0}" != 1 ]; then
        docker compose -f "$COMPOSE" down >/dev/null 2>&1
        # 只删自己 mktemp 出来的目录（前缀判断防变量为空时误删）。
        case "$KEYS" in
        */1zterm-docker-keys.*) rm -rf -- "$KEYS" ;;
        esac
    else
        echo "（KEEP=1：容器与临时密钥保留，密钥在 $KEYS）" >&2
    fi
}
trap cleanup EXIT INT TERM

echo "跑基准：载荷 ${SIZE_MB} MiB × ${ROUNDS} 轮 + 小文件 ${SMALL_COUNT} 个（本项目 term-sftp 客户端）" >&2
# 必须 --release：debug 下 SFTP 的分块加密与拷贝没优化，测出来的是
# 「编译器没做内联」而不是「传输层快不快」（实测 debug 会低一个数量级）。
# 首次跑要先编译 release，慢是正常的；换代码后别用 SKIP_BUILD 绕过重建。
cargo run --release --quiet --manifest-path "$ROOT/crates/term-sftp/Cargo.toml" \
    --example transfer-bench -- \
    --size-mb "$SIZE_MB" --rounds "$ROUNDS" --small-count "$SMALL_COUNT" \
    --out "$OUT" --work-dir "$WORK" || {
    echo "基准失败" >&2
    exit 1
}

echo "报告：$OUT" >&2
cat "$OUT"
