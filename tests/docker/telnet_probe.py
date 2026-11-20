#!/usr/bin/env python3
"""极简 telnet 探针：只做验证服务端 IAC 协商与 8-bit 传输所需的那点事。

为什么不用 `telnet` 命令：
  · macOS 从 10.13 起就不再随系统提供 telnet 客户端，本机这个是 Homebrew 装的，
    别人的机器上大概率没有；CI 里更不能假设它存在。
  · 各家 telnet 客户端对 IAC 的默认策略不同（有的默认不协商 BINARY），
    「测试通过」会变成「那天装的那个客户端的行为恰好合适」。

自己发 IAC 字节的好处是：协商序列是我们自己写的，断言就能精确到「服务端到底
回了 DO 还是 DONT」。这正是 telnet 层最容易错的地方。

三个子命令，对应三层判据：
  negotiate  只发 WILL BINARY/WILL SGA，看服务端回 DO
  login      走完 login/password，执行一条命令，看输出
  binary     登录后 `stty raw`，穿 0xFF 与高位字节，用 od 回读校验

退出码：0 通过，1 断言失败，2 连不上。
"""
import hashlib
import os
import re
import socket
import sys
import time

IAC, DONT, DO, WONT, WILL, SB, SE = 255, 254, 253, 252, 251, 250, 240
OPT_BINARY, OPT_SGA, OPT_ECHO = 0, 3, 1


class Telnet:
    def __init__(self, host, port, timeout=10.0):
        self.sock = socket.create_connection((host, int(port)), timeout=timeout)
        self.sock.settimeout(timeout)
        self.buf = b""
        self.negotiated = []          # 服务端发来的 (verb, option)
        self.sent = bytearray()       # 我们发出去的净荷（去掉 IAC 后）

    # ── 收 ────────────────────────────────────────────────────────────────
    def _fill(self):
        chunk = self.sock.recv(4096)
        if not chunk:
            raise EOFError("服务端关闭连接")
        self.buf += chunk

    def read_for(self, seconds, idle=1.5):
        """读到时间窗结束、或连续 idle 秒没有新数据为止。

        不能用 `read_until(rb'(?s).*')`：`.*` 能匹配空串，会在第一个字节就返回，
        拿到的永远是半截输出。raw 模式下也没有可靠的结束标记，只能按「静默多久
        算说完」来判断。
        """
        text = b""
        old = self.sock.gettimeout()
        self.sock.settimeout(0.5)
        deadline = time.time() + seconds
        last = time.time()
        try:
            while time.time() < deadline:
                try:
                    self._fill()
                except socket.timeout:
                    if time.time() - last > idle:
                        break
                    continue
                text += self._drain()
                last = time.time()
        finally:
            self.sock.settimeout(old)
        return text

    def _drain(self):
        """把 buf 里已完整到达的部分剥成净文本；不完整的 IAC 留在 buf 里。"""
        text = b""
        while self.buf:
            b = self.buf[0]
            if b != IAC:
                text += bytes([b])
                self.buf = self.buf[1:]
                continue
            if len(self.buf) < 2:
                break
            verb = self.buf[1]
            if verb == IAC:
                text += bytes([IAC])
                self.buf = self.buf[2:]
                continue
            if verb in (DO, DONT, WILL, WONT):
                if len(self.buf) < 3:
                    break
                opt = self.buf[2]
                self.negotiated.append((verb, opt))
                self._reply(verb, opt)
                self.buf = self.buf[3:]
                continue
            end = self.buf.find(bytes([IAC, SE]))
            if end == -1:
                break
            self.buf = self.buf[end + 2:]
        return text

    def read_until(self, pattern, deadline):
        """读到匹配 pattern 为止，返回期间收到的**净文本**（IAC 已剥离并应答）。

        剥离是必须的：telnet 服务端会把 IAC 序列夹在正常输出中间，直接把裸字节
        当文本匹配会得到「明明看见了 READY 却匹配不到」这种假阴性。
        """
        text = b""
        while time.time() < deadline:
            text += self._drain()
            if re.search(pattern, text):
                return text
            try:
                self._fill()
            except socket.timeout:
                break
        return text + self._drain()

    def _reply(self, verb, opt):
        # 只接受 BINARY / SGA / ECHO，其余一律拒绝。拒绝要显式回 DONT/WONT，
        # 沉默会让服务端重发，然后就变成了「超时」这种含糊症状。
        want = opt in (OPT_BINARY, OPT_SGA, OPT_ECHO)
        if verb == DO:
            self.sock.sendall(bytes([IAC, WILL if want else WONT, opt]))
        elif verb == WILL:
            self.sock.sendall(bytes([IAC, DO if want else DONT, opt]))

    # ── 发 ────────────────────────────────────────────────────────────────
    def send_line(self, s):
        # 只发 CR，不发 CRLF。实测：发 `cmd\r\n` 时，PTY 的 ICRNL 把 \r 变成 \n
        # 结束行，多出来的那个 \n 会留在输入队列；紧接着 `stty raw` 切模式后，
        # 子进程的第一次 read 就把这个 \n 当成数据（表现为「第一个字节是 0x0a」）。
        # 单发 \r 由 ICRNL 转成行结束，输入队列干净。
        self.sock.sendall(s.encode() + b"\r")

    def send_raw(self, data):
        """IAC 转义：数据里的 0xFF 必须双写，否则服务端会把它当命令起始吃掉。"""
        out = bytearray()
        for b in data:
            out += bytes([IAC, IAC]) if b == IAC else bytes([b])
        self.sock.sendall(bytes(out))

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass


def opt_name(opt):
    return {OPT_BINARY: "BINARY", OPT_SGA: "SGA", OPT_ECHO: "ECHO"}.get(opt, f"opt{opt}")


def cmd_negotiate(host, port):
    t = Telnet(host, port)
    try:
        # 主动表态：我们要 8-bit（BINARY）与 suppress-go-ahead（SGA）。
        # 服务端若同意会回 DO；回 DONT 说明它不支持 8-bit，二进制传输不可用。
        t.sock.sendall(bytes([IAC, WILL, OPT_BINARY, IAC, WILL, OPT_SGA]))
        deadline = time.time() + 8
        t.read_until(rb"login:|ogin:", deadline)
        got = {(v, o) for v, o in t.negotiated}
        print("  协商记录: " + ", ".join(f"{v}/{opt_name(o)}" for v, o in t.negotiated))
        if (DO, OPT_BINARY) not in got:
            print("  ✗ 服务端没有回 DO BINARY，8-bit 通道不可用")
            return 1
        print("  ✓ 服务端回 DO BINARY（8-bit 协商成功）")
        return 0
    finally:
        t.close()


def _login(t, user, password):
    deadline = time.time() + 15
    t.read_until(rb"login:|ogin:", deadline)
    t.send_line(user)
    t.read_until(rb"[Pp]assword:", deadline)
    t.send_line(password)
    # 登录成功的标志是出现 shell 提示符（$ 或 #）或我们主动发的标记被回显。
    out = t.read_until(rb"[$#] ?$", deadline)
    if b"Password" in out and b"incorrect" in out.lower():
        raise AssertionError("密码被拒")
    return out


def cmd_login(host, port, user, password):
    t = Telnet(host, port)
    try:
        t.sock.sendall(bytes([IAC, WILL, OPT_BINARY, IAC, WILL, OPT_SGA]))
        _login(t, user, password)
        marker = "TELNET_LOGIN_OK_9271"
        t.send_line(f"echo {marker}")
        out = t.read_until(marker.encode() + rb"[\s\S]*" + marker.encode(), time.time() + 10)
        if out.count(marker.encode()) < 2:
            # 一次是命令回显，一次是执行结果。只出现一次说明命令没被执行。
            print(f"  ✗ 只看到 {out.count(marker.encode())} 次标记，命令可能没执行")
            print("  实际输出: " + repr(out[-300:]))
            return 1
        print("  ✓ 登录成功且命令有回显")
        return 0
    finally:
        t.close()


def cmd_binary(host, port, user, password):
    """8-bit 传输：绕开 PTY 的特殊字符，只验证 0xFF 与高位字节不被剥离/吞掉。"""
    t = Telnet(host, port)
    try:
        t.sock.sendall(bytes([IAC, WILL, OPT_BINARY, IAC, WILL, OPT_SGA]))
        _login(t, user, password)
        # 选这组字节的理由：
        #   0x80–0xFF 覆盖高位；0xFF 是 IAC 本身，必须靠双写转义才能过去；
        #   0x00 验证 NUL 不被当字符串终止符。
        # 刻意避开 0x03/0x04/0x11/0x13/0x1A —— 即便在 raw 模式下，不同 tty
        # 实现对它们的处理也不一致，那属于 PTY 行为而不是 telnet 协议行为。
        payload = bytes(range(0x80, 0x100)) + b"\x00" + bytes([0xFF])
        n = len(payload)
        # 判据用服务端自己算的 SHA256，而不是 `od` 回读。
        # 理由：raw 模式下 od 的换行不再转成 CRLF，输出分行位置跟着变，
        # 靠正则抓最后 n 个十六进制串会抓到错位的内容（实测第一个字节就成了 0x0a）。
        # `head -c N | sha256sum` 只吐一个 64 位 hex，断言是逐字节严格的。
        t.send_line(f"stty raw -echo; head -c {n} | sha256sum")
        time.sleep(0.5)          # 等 head 真正开始读
        t.send_raw(payload)
        out = t.read_for(10)
        digests = re.findall(rb"\b[0-9a-f]{64}\b", out)
        want = hashlib.sha256(payload).hexdigest()
        if not digests:
            print(f"  ✗ 没有取到服务端摘要，实际输出: {out[-200:]!r}")
            return 1
        got = digests[-1].decode()
        if got != want:
            print(f"  ✗ 摘要不一致\n    服务端 {got}\n    本地   {want}")
            return 1
        print(f"  ✓ {n} 字节（含 0xFF 与高位）经 8-bit 通道后摘要一致")
        return 0
    finally:
        t.close()


def main():
    if len(sys.argv) < 4:
        print(__doc__)
        return 2
    action, host, port = sys.argv[1], sys.argv[2], sys.argv[3]
    user = os.environ.get("TELNET_USER", "tester")
    password = os.environ.get("TELNET_PASS", "testerpass")
    try:
        if action == "negotiate":
            return cmd_negotiate(host, port)
        if action == "login":
            return cmd_login(host, port, user, password)
        if action == "binary":
            return cmd_binary(host, port, user, password)
        print(f"未知子命令: {action}", file=sys.stderr)
        return 2
    except (ConnectionError, socket.timeout, EOFError) as e:
        print(f"  ✗ 连接/读取失败: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
