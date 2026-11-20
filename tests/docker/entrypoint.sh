#!/bin/bash
# 容器启动：按 ROLE 装成 sshd / telnetd / tcp 三种角色之一。
#
# 设计前提：客户端要能无人值守地连上来。这带来两条硬要求，都在这里落实：
#
#   · host key 必须可预测。挂了 `/keys/host/` 就用挂载的固定 key，客户端从而可以
#     预置 known_hosts 做严格校验；没挂才退回启动时生成（此时只能不校验）。
#   · 认证材料必须自带。`/keys/client/id_ed25519.pub` 装进 authorized_keys，
#     不读宿主机任何东西。
#
# 角色用环境变量选，不用三个 Dockerfile：三个角色共享同一套用户、路径与诊断
# 工具，拆开只会让约定散落多处。
set -euo pipefail

ROLE="${ROLE:-sshd}"

# ── 共用：装公钥、装 host key ────────────────────────────────────────────────

install_client_key() {
    local pub=/keys/client/id_ed25519.pub
    if [ ! -f "$pub" ]; then
        echo "warning: $pub 不存在，公钥认证不可用（只剩密码与交互认证）" >&2
        return 0
    fi
    install -o tester -g tester -m 600 "$pub" /home/tester/.ssh/authorized_keys

    # chroot 用户的公钥也要装。认证发生在 chroot **之前**，所以这里用的是真实
    # 路径（/sftp/sftponly/...），不是 chroot 内的 /。
    # 少了这一步的症状很有迷惑性：sftponly 连不上 → 「读不到 /etc/passwd」那条
    # 负向断言反而通过，看起来像是隔离生效了。
    install -d -o sftponly -g sftponly -m 700 /sftp/sftponly/.ssh
    install -o sftponly -g sftponly -m 600 "$pub" /sftp/sftponly/.ssh/authorized_keys
}

install_host_keys() {
    local dir=/keys/host found=0 f base
    for f in "$dir"/ssh_host_*_key; do
        [ -e "$f" ] || continue
        base=$(basename "$f")
        install -o root -g root -m 600 "$f" "/etc/ssh/$base"
        if [ -f "$f.pub" ]; then
            install -o root -g root -m 644 "$f.pub" "/etc/ssh/$base.pub"
        fi
        found=1
    done
    if [ "$found" = 0 ]; then
        echo "note: 未挂载固定 host key，改为启动时生成（known_hosts 无法预置）" >&2
    fi
    # 只为缺失的类型补，已存在的不覆盖。
    ssh-keygen -A >/dev/null
}

# ── 共用：二进制载荷 ────────────────────────────────────────────────────────

# 大小可调（PAYLOAD_MB），默认 1 MiB：足够跨过 SFTP 的分段边界与 TCP 的单次
# 写窗口，又不至于让 CI 每次都等 I/O。
ensure_payload() {
    if [ ! -s /payload/pattern.bin ]; then
        dd if=/dev/urandom of=/payload/pattern.bin bs=1M count="${PAYLOAD_MB:-1}" status=none
    fi
}

# ── sshd ────────────────────────────────────────────────────────────────────

configure_sshd() {
    local cfg=/etc/ssh/sshd_config key kv
    # sshd 对同一关键字取**第一次**出现的值。Debian 默认配置里已经有显式的
    # `KbdInteractiveAuthentication no`，往文件尾追加 `yes` 不生效（`sshd -T` 仍报 no）。
    # 所以必须就地改，不能只 append。
    sed -i -E 's/^#?[[:space:]]*KbdInteractiveAuthentication[[:space:]].*/KbdInteractiveAuthentication yes/' "$cfg"
    sed -i -E 's/^#?[[:space:]]*PasswordAuthentication[[:space:]].*/PasswordAuthentication yes/' "$cfg"
    sed -i -E 's/^#?[[:space:]]*UsePAM[[:space:]].*/UsePAM yes/' "$cfg"
    for kv in 'PermitRootLogin no' 'AllowTcpForwarding yes' 'X11Forwarding no' 'PrintMotd no' \
              'ClientAliveInterval 15' 'ClientAliveCountMax 4'; do
        key=${kv%% *}
        if grep -qiE "^#?[[:space:]]*${key}[[:space:]]" "$cfg"; then
            sed -i -E "s/^#?[[:space:]]*${key}[[:space:]].*/${kv}/" "$cfg"
        else
            echo "$kv" >>"$cfg"
        fi
    done

    # SFTP 子系统：Debian 的 openssh-server 默认就有这一行，但依赖发行版默认值
    # 是不可靠的（换个基础镜像就没了），显式确认一次。
    grep -qiE '^[[:space:]]*Subsystem[[:space:]]+sftp' "$cfg" \
        || echo 'Subsystem sftp /usr/lib/openssh/sftp-server' >>"$cfg"

    # chroot 的 SFTP 用户。`Match` 块必须位于文件末尾——它之后的关键字会被
    # 当作 Match 条件的一部分，sshd 直接拒绝启动。
    if ! grep -q '^Match User sftponly' "$cfg"; then
        cat >>"$cfg" <<'EOF'

# 测试用：被锁在 /sftp/sftponly 里的纯 SFTP 用户。
# 用来验证 ChrootDirectory + internal-sftp 这条路径（真实世界里最常见的
# 「只能传文件不能登录」配置）。
Match User sftponly
    ChrootDirectory /sftp/sftponly
    ForceCommand internal-sftp
    AllowTcpForwarding no
    X11Forwarding no
    PasswordAuthentication yes
EOF
    fi

    # 起之前自检：配置没生效就让容器直接失败，而不是让测试给出「认证失败」这种
    # 指向客户端的错结论。
    if [ "$(sshd -T 2>/dev/null | grep -i '^kbdinteractiveauthentication' | awk '{print $2}')" != "yes" ]; then
        echo "sshd 配置自检失败：KbdInteractiveAuthentication 没生效" >&2
        sshd -T 2>&1 | grep -iE 'kbd|usepam|passwordauth|subsystem' >&2 || true
        exit 1
    fi
}

start_sshd() {
    install_client_key
    install_host_keys
    configure_sshd
    ensure_payload
    start_echo
    exec /usr/sbin/sshd -D -e
}

# ── 共用：9999 回显 ─────────────────────────────────────────────────────────

# `term-tunnel` 的二进制完整性测试把 1 MiB 写进 `-L` 隧道、落到目标机的 9999，
# 期望原样回来。所以**目标角色必须提供它**，不是只有 tcp 角色才提供。
# 漏掉的症状是一句 `Broken pipe`——看着像隧道写坏了，实际是对端端口没人监听。
start_echo() {
    socat TCP-LISTEN:9999,reuseaddr,fork EXEC:/bin/cat &
}

# ── telnetd ─────────────────────────────────────────────────────────────────

start_telnetd() {
    # Debian 的 telnetd 来自 GNU inetutils，**没有** `-debug` / `-p`（那是 BSD 版
    # 参数），它也从不自己监听端口——按 inetd 约定，它从 stdin/stdout 继承一个
    # 已建立的 socket。容器里没有 inetd，所以用 socat 扮演 inetd 的角色：
    # listen + accept + fork，再把连接接到 telnetd 的标准流上。
    # socat 传的是 socketpair，telnetd 只用 read/write 与 getpeername，实测可用。
    exec socat TCP-LISTEN:2323,reuseaddr,fork EXEC:/usr/sbin/telnetd
}

# ── tcp（socat 三种角色）────────────────────────────────────────────────────

start_tcp() {
    ensure_payload
    # 9999 回显：原样回送字节，不解释内容。用于隧道/全双工完整性。
    start_echo
    # 9998 汇聚：收完整个流后回一行 `SHA256 <hex> SIZE <n>`，让「发出去的字节
    # 到底有没有被完整收到」变成可断言的判据，而不是「没报错就算过」。
    socat TCP-LISTEN:9998,reuseaddr,fork EXEC:/usr/local/bin/tcp-sink.sh &
    # 9997 源端：先发一行 `SHA256 <hex> SIZE <n>` 头，再发原始字节。客户端据此
    # 校验下载内容，不需要额外通道。
    socat TCP-LISTEN:9997,reuseaddr,fork EXEC:/usr/local/bin/tcp-source.sh &
    trap 'kill 0' TERM INT
    wait -n
}

case "$ROLE" in
sshd) start_sshd ;;
telnetd) start_telnetd ;;
tcp) start_tcp ;;
*)
    echo "未知 ROLE: $ROLE（可选 sshd / telnetd / tcp）" >&2
    exit 2
    ;;
esac
