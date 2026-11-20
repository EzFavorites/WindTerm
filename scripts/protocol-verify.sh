#!/bin/bash
# 协议级验证：把「各种协议能不能通」变成一组可断言的用例。
#
# 与 `scripts/docker-verify.sh` 的分工：
#   · 本脚本验**协议与容器本身**（sshd / telnetd / socat / sftp / scp），不编译 Rust，
#     秒级出结果，用来判断「测试床是好的」。
#   · docker-verify.sh 验**产品代码与真服务端的互操作**（cargo test），依赖同一套
#     容器。测试床坏了会污染那边，所以先跑这个。
#
# ── 无人值守与「不碰系统密钥」的三条硬约束 ────────────────────────────────
#
# 1. 所有 SSH/SFTP/SCP 调用都带 `-F /dev/null`：不读用户的 ~/.ssh/config。
#    否则别人机器上一条 `Host *  IdentityFile ~/.ssh/id_rsa` 就能让测试行为漂移。
# 2. 都带 `-o IdentityAgent=none -o IdentitiesOnly=yes -i <仓库内私钥>`：
#    不询问 ssh-agent，因而不会触发 macOS 钥匙串授权弹窗。这是「不频繁请求权限」
#    的关键——弹窗来自 agent，不来自 ssh 本身。
# 3. host key 用仓库内 known_hosts 做**严格**校验（`StrictHostKeyChecking=yes`）。
#    既零交互，又真的验证了「连的是不是预期那台机器」。
#
# 用法：
#   bash scripts/protocol-verify.sh                 # 全部
#   bash scripts/protocol-verify.sh ssh telnet      # 指定组
#   KEEP=1 bash scripts/protocol-verify.sh          # 跑完留容器
#   SKIP_BUILD=1 bash scripts/protocol-verify.sh    # 复用已有镜像
#   PAYLOAD_MB=8 bash scripts/protocol-verify.sh    # 加大二进制载荷
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DOCKER_DIR="$ROOT/tests/docker"
COMPOSE="$DOCKER_DIR/docker-compose.yml"
KEYS="$DOCKER_DIR/keys"
CLIENT_KEY="$KEYS/client/id_ed25519"

HOST=127.0.0.1
P_SSH=22022
P_BASTION=22023
P_TELNET=22024
P_ECHO=22025
P_SINK=22026
P_SOURCE=22027
USER_NAME=tester
SFTP_USER=sftponly
SFTP_PASS=sftppass
PAYLOAD_MB=${PAYLOAD_MB:-1}

if [ $# -eq 0 ]; then
    set -- ssh hostkey sftp scp telnet tcp transfer xfer tmux
fi

if [ -t 1 ]; then
    B=$'\033[1m'; G=$'\033[32m'; R=$'\033[31m'; Y=$'\033[33m'; Z=$'\033[0m'
else
    B=''; G=''; R=''; Y=''; Z=''
fi

n_pass=0; n_fail=0; n_skip=0
ok()   { n_pass=$((n_pass + 1)); printf '  %s✓%s %s\n' "$G" "$Z" "$1"; }
bad()  { n_fail=$((n_fail + 1)); printf '  %s✗%s %s\n' "$R" "$Z" "$1"; [ -n "${2:-}" ] && printf '      %s\n' "$2"; return 0; }
skip() { n_skip=$((n_skip + 1)); printf '  %s-%s %s（跳过：%s）\n' "$Y" "$Z" "$1" "$2"; }
say()  { printf '\n%s── %s ──%s\n' "$B" "$1" "$Z"; }

# ── 公共 SSH 选项 ───────────────────────────────────────────────────────────

# ssh 的 `-o` 值会被按空格分词。工作区路径里只要有空格（本仓库是
# "Craft Anything"），UserKnownHostsFile 就会被劈成两截，ssh 于是报
# 「No ED25519 host key is known」并拒绝连接——看起来像密钥不匹配，实际是
# 参数被切断了。用 ssh 配置语法自己的双引号把值包住，路径含空格才是安全的。
KH_Q="UserKnownHostsFile=\"$KEYS/known_hosts\""

SSH_OPTS=(
    -F /dev/null
    -i "$CLIENT_KEY"
    -o IdentitiesOnly=yes
    -o IdentityAgent=none
    -o BatchMode=yes
    -o PreferredAuthentications=publickey
    -o StrictHostKeyChecking=yes
    -o "$KH_Q"
    -o ConnectTimeout=5
    -o ServerAliveInterval=5
    -o LogLevel=ERROR
)
# sftp/scp 用同一组 -o（它们内部就是 ssh），端口参数名不同：sftp/scp 是 -P。
SFTP_OPTS=("${SSH_OPTS[@]}")

ssh_run() { ssh "${SSH_OPTS[@]}" -p "$P_SSH" "$USER_NAME@$HOST" "$@"; }
sftp_batch() { sftp "${SFTP_OPTS[@]}" -P "$P_SSH" -b - "$1@$HOST"; }

# 生成一份临时 ssh_config，供子进程（python）直接 `ssh -F <cfg>` 使用。
#
# 为什么需要它：`-o Key=Value` 的值会被按空格分词，路径里一有空格（本仓库是
# "Craft Anything"）就会被切断；而且把一长串选项拼进 python 的 argv 也很脆。
# 配置文件里用双引号是标准语法，路径和工作参数都只写一次。
write_ssh_config() {
    local kh=$1 out=$2
    cat >"$out" <<EOF
Host windterm-target
    HostName $HOST
    Port $P_SSH
    User $USER_NAME
    IdentityFile "$CLIENT_KEY"
    IdentitiesOnly yes
    IdentityAgent none
    BatchMode yes
    StrictHostKeyChecking yes
    UserKnownHostsFile "$kh"
    ConnectTimeout 5
    LogLevel ERROR
EOF
    chmod 600 "$out"
}

wait_port() {
    local port=$1 n=0
    while [ "$n" -lt 60 ]; do
        nc -z "$HOST" "$port" 2>/dev/null && return 0
        n=$((n + 1)); sleep 0.5
    done
    return 1
}

# ── 前置：镜像、密钥、容器 ─────────────────────────────────────────────────

say "0. 准备测试床"
command -v docker >/dev/null || { echo "没有 docker" >&2; exit 2; }
docker info >/dev/null 2>&1 || { echo "docker 没在跑（OrbStack/Docker Desktop 未启动）" >&2; exit 2; }
ok "docker $(docker version --format '{{.Server.Version}}' 2>/dev/null)"

command -v python3 >/dev/null || { echo "探针需要 python3" >&2; exit 2; }

bash "$DOCKER_DIR/gen-keys.sh" >/dev/null
[ -f "$CLIENT_KEY" ] && ok "仓库内客户端密钥就绪（不依赖 ~/.ssh）" || { bad "密钥生成失败"; exit 2; }

if [ "${SKIP_BUILD:-0}" != "1" ]; then
    if docker build -q -t windterm-test-sshd:latest "$DOCKER_DIR" >/dev/null 2>&1; then
        ok "镜像 windterm-test-sshd 构建完成"
    else
        bad "镜像构建失败（需要能拉取 debian:12-slim 与 apt 源）"
        exit 2
    fi
else
    ok "复用已有镜像（SKIP_BUILD=1）"
fi

# --force-recreate：镜像重建后必须换容器，否则会继续用旧镜像里的 entrypoint，
# 于是「刚改完却仍失败」——这类假阴性排查起来最费时间。
docker compose -f "$COMPOSE" up -d --wait --force-recreate >/dev/null 2>&1
for p in "$P_SSH" "$P_BASTION" "$P_TELNET" "$P_ECHO" "$P_SINK" "$P_SOURCE"; do
    if ! wait_port "$p"; then
        bad "端口 $p 未就绪"
        docker compose -f "$COMPOSE" ps
        exit 2
    fi
done
ok "target / bastion / telnet / tcp 全部就绪"

cleanup() {
    if [ "${KEEP:-0}" = "1" ]; then
        printf '\n（KEEP=1：容器保留。清理：docker compose -f %s down -v）\n' "$COMPOSE"
    else
        docker compose -f "$COMPOSE" down -v >/dev/null 2>&1 || true
    fi
    [ -n "${MARKER:-}" ] && rm -f "$MARKER"
    [ -n "${TMPDIR_TEST:-}" ] && rm -rf "$TMPDIR_TEST"
}
MARKER=$(mktemp)
TMPDIR_TEST=$(mktemp -d)
trap cleanup EXIT

# ── 1. SSH ─────────────────────────────────────────────────────────────────

g_ssh() {
    say "1. SSH（真 OpenSSH 服务端）"
    local out rc msg m

    out=$(ssh_run 'printf SSH_CHANNEL_OK' 2>&1)
    if [ "$out" = "SSH_CHANNEL_OK" ]; then
        ok "公钥认证 + 命令通道"
    else
        bad "公钥认证失败" "$out"
    fi

    ssh_run 'exit 42' >/dev/null 2>&1; rc=$?
    if [ "$rc" -eq 42 ]; then ok "远端退出码透传（42）"; else bad "退出码透传" "得到 $rc"; fi

    out=$(ssh "${SSH_OPTS[@]}" -tt -p "$P_SSH" "$USER_NAME@$HOST" 'tty' 2>/dev/null | tr -d '\r\n')
    case "$out" in
    /dev/pts/*) ok "PTY 分配（${out}）" ;;
    *) bad "PTY 分配" "得到 '$out'" ;;
    esac

    # 用「不发任何认证」探测服务端支持的方法列表。这比尝试密码登录更适合
    # 无人值守：不需要把密码喂给交互式提示，也不会因重试策略差异而抖动。
    # 注意 -o 重复时 ssh 取**第一个**出现的值，所以这个 none 必须放在最前面。
    msg=$(ssh -F /dev/null -o PreferredAuthentications=none -o BatchMode=yes \
        -o StrictHostKeyChecking=yes -o "$KH_Q" \
        -o ConnectTimeout=5 -o LogLevel=ERROR \
        -p "$P_SSH" "$USER_NAME@$HOST" true 2>&1)
    for m in publickey password keyboard-interactive; do
        if printf '%s' "$msg" | grep -qi "$m"; then
            ok "服务端提供 $m 认证"
        else
            bad "服务端未提供 $m" "$msg"
        fi
    done
}

# ── 2. host key 严格校验（含负向）──────────────────────────────────────────

g_hostkey() {
    say "2. 主机密钥（严格校验 + 变更必须拒绝）"
    local out rc bad_known

    if ssh_run 'true' >/dev/null 2>&1; then
        ok "预置 known_hosts 严格校验通过（StrictHostKeyChecking=yes）"
    else
        bad "严格校验模式下连不上（known_hosts 与 host key 不匹配？）"
    fi

    # 负向：把 22022 那一条换成跳板机的公钥。真实场景里这就是「同一地址换了机器」。
    bad_known="$TMPDIR_TEST/known_hosts.wrong"
    printf '[127.0.0.1]:%s %s\n' "$P_SSH" \
        "$(cut -d' ' -f1,2 "$KEYS/bastion/ssh_host_ed25519_key.pub")" >"$bad_known"
    out=$(ssh -F /dev/null -i "$CLIENT_KEY" -o IdentitiesOnly=yes -o IdentityAgent=none \
        -o BatchMode=yes -o StrictHostKeyChecking=yes \
        -o "UserKnownHostsFile=\"$bad_known\"" \
        -o ConnectTimeout=5 -o LogLevel=ERROR \
        -p "$P_SSH" "$USER_NAME@$HOST" true 2>&1); rc=$?
    # 用 -E 而不是 BRE 的 `\|`：后者是 GNU 扩展，BSD / toybox grep 不认，
    # 于是断言恒为假——「拒绝成功」被报成「没有拒绝」。这个坑已经踩过一次。
    if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -qiE 'host key verification failed|REMOTE HOST IDENTIFICATION HAS CHANGED'; then
        ok "host key 不匹配时被拒绝（exit ${rc}）"
    else
        bad "host key 不匹配却没有被拒绝" "exit=$rc out=$out"
    fi

    # 证据性检查：整轮测试没有写过宿主机 ~/.ssh。
    if [ ! -d "$HOME/.ssh" ]; then
        skip "宿主机 ~/.ssh 未被写入" "目录不存在"
    else
        local touched
        touched=$(find "$HOME/.ssh" -maxdepth 2 -type f -newer "$MARKER" 2>/dev/null | wc -l | tr -d ' ')
        if [ "$touched" -eq 0 ]; then
            ok "宿主机 ~/.ssh 未被写入"
        else
            bad "宿主机 ~/.ssh 被写入了 $touched 个文件（测试应完全隔离）"
        fi
    fi
}

# ── 3. SFTP ────────────────────────────────────────────────────────────────

g_sftp() {
    say "3. SFTP（上传 / 下载 / 列举 / chroot）"
    local up="$TMPDIR_TEST/sftp-up.bin" down="$TMPDIR_TEST/sftp-down.bin"
    local lsha rsha out

    dd if=/dev/urandom of="$up" bs=1048576 count="${PAYLOAD_MB}" 2>/dev/null
    if sftp_batch "$USER_NAME" >/dev/null 2>&1 <<EOF
put "$up" /payload/sftp-up.bin
EOF
    then
        lsha=$(shasum -a 256 "$up" | cut -d' ' -f1)
        rsha=$(ssh_run 'sha256sum /payload/sftp-up.bin' 2>/dev/null | cut -d' ' -f1)
        if [ "$lsha" = "$rsha" ]; then
            ok "上传 ${PAYLOAD_MB} MiB 逐字节一致（sha256 ${lsha:0:16}…）"
        else
            bad "上传后摘要不一致" "local=$lsha remote=$rsha"
        fi
    else
        bad "SFTP 上传失败"
    fi

    if sftp_batch "$USER_NAME" >/dev/null 2>&1 <<EOF
get /payload/pattern.bin "$down"
EOF
    then
        rsha=$(ssh_run 'sha256sum /payload/pattern.bin' 2>/dev/null | cut -d' ' -f1)
        lsha=$(shasum -a 256 "$down" | cut -d' ' -f1)
        if [ "$lsha" = "$rsha" ]; then
            ok "下载服务端随机载荷逐字节一致"
        else
            bad "下载后摘要不一致" "local=$lsha remote=$rsha"
        fi
    else
        bad "SFTP 下载失败"
    fi

    out=$(sftp_batch "$USER_NAME" 2>/dev/null <<EOF
ls /payload
EOF
    )
    if printf '%s' "$out" | grep -q 'pattern.bin'; then
        ok "目录列举可用"
    else
        bad "目录列举没看到 pattern.bin" "$out"
    fi

    # chroot 用户：根被换到 /sftp/sftponly，看不到真实 /etc。
    #
    # 必须先确认「确实登上去了」。否则认证失败时下面那条负向断言会**假通过**：
    # 读不到 /etc/passwd 不是隔离生效，而是根本没连上。这个坑踩过一次。
    out=$(printf 'pwd\n' | sftp "${SFTP_OPTS[@]}" -P "$P_SSH" -b - "$SFTP_USER@$HOST" 2>&1)
    if printf '%s' "$out" | grep -qiE 'permission denied|connection closed'; then
        bad "chroot 用户认证失败" "$out"
        return
    fi
    if printf '%s' "$out" | grep -qE 'Remote working directory: /[[:space:]]*$'; then
        ok "chroot 用户工作目录被限制在 /"
    else
        bad "chroot 用户的工作目录不对" "$out"
    fi
    if printf 'get /etc/passwd %s/leak\n' "$TMPDIR_TEST" | sftp "${SFTP_OPTS[@]}" -P "$P_SSH" -b - "$SFTP_USER@$HOST" >/dev/null 2>&1; then
        bad "chroot 用户读到了 /etc/passwd（隔离失效）"
    else
        ok "chroot 用户无法访问真实 /etc/passwd"
    fi
}

# ── 4. SCP（两种协议）──────────────────────────────────────────────────────

g_scp() {
    say "4. SCP（SFTP 协议与 legacy -O）"
    local up="$TMPDIR_TEST/scp-up.bin" down="$TMPDIR_TEST/scp-down.bin" lsha rsha

    dd if=/dev/urandom of="$up" bs=1048576 count="${PAYLOAD_MB}" 2>/dev/null
    lsha=$(shasum -a 256 "$up" | cut -d' ' -f1)

    if scp "${SSH_OPTS[@]}" -P "$P_SSH" "$up" "$USER_NAME@$HOST:/payload/scp-modern.bin" >/dev/null 2>&1; then
        rsha=$(ssh_run 'sha256sum /payload/scp-modern.bin' 2>/dev/null | cut -d' ' -f1)
        if [ "$lsha" = "$rsha" ]; then ok "scp（默认 SFTP 协议）上传一致"; else bad "scp 默认协议上传摘要不符"; fi
    else
        bad "scp（默认协议）上传失败"
    fi

    if scp -O "${SSH_OPTS[@]}" -P "$P_SSH" "$up" "$USER_NAME@$HOST:/payload/scp-legacy.bin" >/dev/null 2>&1; then
        rsha=$(ssh_run 'sha256sum /payload/scp-legacy.bin' 2>/dev/null | cut -d' ' -f1)
        if [ "$lsha" = "$rsha" ]; then ok "scp -O（legacy 协议）上传一致"; else bad "scp -O 上传摘要不符"; fi
    else
        bad "scp -O（legacy 协议）上传失败（OpenSSH 需 >= 9.0）"
    fi

    if scp "${SSH_OPTS[@]}" -P "$P_SSH" "$USER_NAME@$HOST:/payload/pattern.bin" "$down" >/dev/null 2>&1; then
        rsha=$(ssh_run 'sha256sum /payload/pattern.bin' 2>/dev/null | cut -d' ' -f1)
        lsha=$(shasum -a 256 "$down" | cut -d' ' -f1)
        if [ "$lsha" = "$rsha" ]; then ok "scp 下载一致"; else bad "scp 下载摘要不符"; fi
    else
        bad "scp 下载失败"
    fi
}

# ── 5. Telnet ──────────────────────────────────────────────────────────────

g_telnet() {
    say "5. Telnet（IAC 协商 / 登录 / 8-bit）"
    local out rc
    for step in negotiate login binary; do
        out=$(python3 "$DOCKER_DIR/telnet_probe.py" "$step" "$HOST" "$P_TELNET" 2>&1); rc=$?
        if [ "$rc" -eq 0 ]; then
            ok "telnet:$step — $(printf '%s' "$out" | tail -1 | sed 's/^ *✓ *//')"
        else
            bad "telnet:$step 失败" "$out"
        fi
    done
}

# ── 6. 裸 TCP ──────────────────────────────────────────────────────────────

g_tcp() {
    say "6. 裸 TCP（回显 / 汇聚 / 源端）"
    local f="$TMPDIR_TEST/tcp-payload.bin" out rc
    dd if=/dev/urandom of="$f" bs=1048576 count="${PAYLOAD_MB}" 2>/dev/null

    for step in echo sink; do
        out=$(python3 "$DOCKER_DIR/tcp_probe.py" "$step" "$HOST" \
            "$([ "$step" = echo ] && echo "$P_ECHO" || echo "$P_SINK")" "$f" 2>&1); rc=$?
        if [ "$rc" -eq 0 ]; then ok "tcp:$step — $(printf '%s' "$out" | tail -1 | sed 's/^ *✓ *//')"; else bad "tcp:$step 失败" "$out"; fi
    done

    out=$(python3 "$DOCKER_DIR/tcp_probe.py" source "$HOST" "$P_SOURCE" "$TMPDIR_TEST/tcp-download.bin" 2>&1); rc=$?
    if [ "$rc" -eq 0 ]; then
        ok "tcp:source — $(printf '%s' "$out" | tail -1 | sed 's/^ *✓ *//')"
    else
        bad "tcp:source 失败" "$out"
    fi
}

# ── 7. 经加密通道的二进制完整性 ────────────────────────────────────────────

g_transfer() {
    say "7. 经 SSH 通道的二进制完整性"
    local out="$TMPDIR_TEST/via-ssh.bin" rsha lsha

    ssh "${SSH_OPTS[@]}" -p "$P_SSH" "$USER_NAME@$HOST" 'cat /payload/pattern.bin' >"$out" 2>/dev/null
    rsha=$(ssh_run 'sha256sum /payload/pattern.bin' 2>/dev/null | cut -d' ' -f1)
    lsha=$(shasum -a 256 "$out" | cut -d' ' -f1)
    if [ "$lsha" = "$rsha" ]; then
        ok "SSH 通道搬运 ${PAYLOAD_MB} MiB 逐字节一致"
    else
        bad "SSH 通道搬运后摘要不符" "local=$lsha remote=$rsha"
    fi

    # 全双工：一边写一边读，验证通道不会因窗口/缓冲而死锁。
    # 用 python 是因为 shell 做不到「边写边读」；选项走 ssh_config，避免把带
    # 空格的路径拼进 argv。
    local cfg="$TMPDIR_TEST/ssh_config"
    write_ssh_config "$KEYS/known_hosts" "$cfg"
    if python3 - "$cfg" <<'PY'
import subprocess, sys

# 用 ssh 的 stdin 灌 256 KiB，同时读它 stdout 的回显（cat 把字节原样送回）。
p = subprocess.Popen(["ssh", "-F", sys.argv[1], "windterm-target", "cat"],
                     stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.DEVNULL)
payload = bytes(range(256)) * 1024
got = bytearray()


def writer():
    try:
        p.stdin.write(payload)
        p.stdin.close()
    except (BrokenPipeError, ValueError):
        pass


import threading
t = threading.Thread(target=writer)
t.start()
while len(got) < len(payload):
    b = p.stdout.read(65536)
    if not b:
        break
    got += b
t.join(timeout=10)
p.wait(timeout=30)
sys.exit(0 if bytes(got) == payload else 1)
PY
    then
        ok "SSH 全双工回显 256 KiB（边写边读不死锁）"
    else
        bad "SSH 全双工回显失败"
    fi
}

# ── 8. lrzsz 真对端（供 term-xfer / term-zmodem 端到端用）──────────────────

g_xfer() {
    say "8. 远端 lrzsz（X/Y/Z Modem 真对端）"
    local out

    if ssh_run 'command -v sz >/dev/null && command -v rz >/dev/null' 2>/dev/null; then
        ok "远端 sz / rz 可用"
    else
        skip "远端 sz / rz" "镜像未装 lrzsz"
        return
    fi

    # sz 要求 stdin 是 tty，所以必须 -tt。这里只取前 128 字节看帧头：
    # ZMODEM 的 ZRQINIT 帧固定以 "**\x18B00" 起头，出现它就说明是真正的
    # lrzsz 在说话，而不是我们的模拟端。
    out=$(ssh "${SSH_OPTS[@]}" -tt -p "$P_SSH" "$USER_NAME@$HOST" \
        'sz -b /payload/pattern.bin 2>/dev/null | head -c 128 | od -An -tx1' 2>/dev/null | tr -d '\r\n')
    if printf '%s' "$out" | grep -q '2a 2a 18 42'; then
        ok "远端 sz 输出 ZRQINIT 帧头（**\\x18B）"
    else
        bad "远端 sz 没有输出 ZMODEM 帧头" "$out"
    fi
}

# ── 9. tmux -CC ────────────────────────────────────────────────────────────

g_tmux() {
    say "9. tmux 控制模式（-CC）"
    local f="$TMPDIR_TEST/tmux.out" pid

    ssh_run 'tmux kill-session -t probe' >/dev/null 2>&1
    ssh "${SSH_OPTS[@]}" -tt -p "$P_SSH" "$USER_NAME@$HOST" 'tmux -CC new -A -s probe' \
        </dev/null >"$f" 2>&1 &
    pid=$!
    sleep 3
    kill "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null

    if grep -qE '%begin|%session-changed|%output' "$f"; then
        ok "收到 tmux 控制模式输出"
    else
        bad "没有收到控制模式输出" "$(head -5 "$f")"
    fi
    ssh_run 'tmux kill-session -t probe' >/dev/null 2>&1
}

# ── 执行 ───────────────────────────────────────────────────────────────────

for g in "$@"; do
    case "$g" in
    ssh) g_ssh ;;
    hostkey) g_hostkey ;;
    sftp) g_sftp ;;
    scp) g_scp ;;
    telnet) g_telnet ;;
    tcp) g_tcp ;;
    transfer) g_transfer ;;
    xfer) g_xfer ;;
    tmux) g_tmux ;;
    all)
        g_ssh; g_hostkey; g_sftp; g_scp; g_telnet; g_tcp; g_transfer; g_xfer; g_tmux
        ;;
    *) bad "不认识的组: $g" "可选 ssh hostkey sftp scp telnet tcp transfer xfer tmux all" ;;
    esac
done

say "汇总"
printf '  通过 %s%d%s · 失败 %s%d%s · 跳过 %s%d%s\n' \
    "$G" "$n_pass" "$Z" "$R" "$n_fail" "$Z" "$Y" "$n_skip" "$Z"
if [ "$n_fail" -eq 0 ]; then
    printf '  %s全部通过%s：这一层验的是「容器里的协议实现是真的」，不是我们自己的 mock。\n' "$G" "$Z"
    exit 0
fi
printf '  %s有失败项%s（上面每项都留了实际输出）\n' "$R" "$Z"
exit 1
