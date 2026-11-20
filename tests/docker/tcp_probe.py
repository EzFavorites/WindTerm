#!/usr/bin/env python3
"""裸 TCP 探针：回显 / 汇聚 / 源端。

为什么不用 shell 的 `/dev/tcp`：三件事它做不好——按固定长度读满（TCP 是字节流，
一次 read 可能只到一半）、`shutdown(SHUT_WR)` 让对端看到 EOF、以及对 1 MiB 载荷
算摘要。用 python 三十行就干净了，也不用装 curl。

退出码：0 通过，1 断言失败，2 连接失败。
"""
import hashlib
import socket
import sys

CHUNK = 65536


def read_exact(sock, n):
    """按长度读满。TCP 不保留消息边界，单次 recv 读到的可能远少于 n。"""
    buf = bytearray()
    while len(buf) < n:
        b = sock.recv(min(CHUNK, n - len(buf)))
        if not b:
            break
        buf += b
    return bytes(buf)


def read_all(sock):
    buf = bytearray()
    while True:
        b = sock.recv(CHUNK)
        if not b:
            return bytes(buf)
        buf += b


def sha(b):
    return hashlib.sha256(b).hexdigest()


def cmd_echo(host, port, path):
    data = open(path, "rb").read()
    s = socket.create_connection((host, port), timeout=30)
    try:
        s.sendall(data)
        s.shutdown(socket.SHUT_WR)      # 告诉对端「发完了」，否则 cat 一直等
        got = read_exact(s, len(data))
    finally:
        s.close()
    if got != data:
        print(f"  ✗ 回显不一致：发出 {len(data)} 字节，收到 {len(got)} 字节")
        return 1
    print(f"  ✓ {len(data)} 字节原样回显（{sha(data)[:16]}…）")
    return 0


def cmd_sink(host, port, path):
    data = open(path, "rb").read()
    s = socket.create_connection((host, port), timeout=30)
    try:
        s.sendall(data)
        s.shutdown(socket.SHUT_WR)
        line = read_all(s).decode(errors="replace").strip()
    finally:
        s.close()
    # 服务端回 `SHA256 <hex> SIZE <n>`；两段都要对，只看长度的话「偶发截断
    # 但长度恰好相同」会漏过去。
    parts = line.split()
    if len(parts) != 4 or parts[0] != "SHA256" or parts[2] != "SIZE":
        print(f"  ✗ 回执格式不对: {line!r}")
        return 1
    if parts[1] != sha(data) or parts[3] != str(len(data)):
        print(f"  ✗ 汇聚端摘要不符：服务端 {parts[1][:16]}…/{parts[3]}B，"
              f"本地 {sha(data)[:16]}…/{len(data)}B")
        return 1
    print(f"  ✓ 汇聚端收到 {len(data)} 字节且摘要一致")
    return 0


def cmd_source(host, port, out_path):
    s = socket.create_connection((host, port), timeout=30)
    try:
        buf = bytearray()
        while b"\n" not in buf:
            chunk = s.recv(CHUNK)
            if not chunk:
                print("  ✗ 源端在给出头部之前就关闭了")
                return 1
            buf += chunk
        head, _, rest = bytes(buf).partition(b"\n")
        parts = head.decode().split()
        if len(parts) != 4 or parts[0] != "SHA256" or parts[2] != "SIZE":
            print(f"  ✗ 头部格式不对: {head!r}")
            return 1
        want_sha, want_size = parts[1], int(parts[3])
        body = rest + read_all(s)
    finally:
        s.close()
    if len(body) != want_size:
        print(f"  ✗ 长度不符：头部声明 {want_size}，实收 {len(body)}")
        return 1
    if sha(body) != want_sha:
        print("  ✗ 摘要与头部声明不符")
        return 1
    open(out_path, "wb").write(body)
    print(f"  ✓ 下载 {want_size} 字节，摘要与头部一致")
    return 0


def main():
    if len(sys.argv) < 5:
        print(__doc__)
        return 2
    action, host, port, path = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
    try:
        if action == "echo":
            return cmd_echo(host, port, path)
        if action == "sink":
            return cmd_sink(host, port, path)
        if action == "source":
            return cmd_source(host, port, path)
        print(f"未知子命令: {action}", file=sys.stderr)
        return 2
    except OSError as e:
        print(f"  ✗ 连接失败: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
