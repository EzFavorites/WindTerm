# 多协议测试床

一个镜像、四种协议、零交互。用来回答一个问题：**容器里的协议实现是不是真的**
——真 OpenSSH sshd、真 GNU telnetd、真 socat、真 lrzsz，不是我们自己的 mock。

配套脚本：

| 脚本 | 验什么 | 依赖 |
|---|---|---|
| `scripts/protocol-verify.sh` | 协议与容器本身（32 项） | docker + python3 |
| `scripts/docker-verify.sh` | 产品代码与真服务端的互操作 | docker + cargo |

先跑前者。后者失败时，要先能排除「测试床坏了」这个可能。

```bash
bash scripts/protocol-verify.sh              # 全部
bash scripts/protocol-verify.sh telnet tcp   # 指定组
KEEP=1 bash scripts/protocol-verify.sh       # 跑完留容器
PAYLOAD_MB=8 bash scripts/protocol-verify.sh # 加大二进制载荷
```

## 1. 拓扑

| 服务 | 镜像角色 | 宿主端口 | 容器端口 | 内容 |
|---|---|---|---|---|
| `target` | `sshd` | 22022 | 22 | sshd + SFTP + SCP + tmux + sz/rz |
| `bastion` | `sshd` | 22023 | 22 | 跳板机，**独立 host key** |
| `client` | — | — | — | 一次性客户端容器，ProxyJump 用 |
| `telnet` | `telnetd` | 22024 | 2323 | GNU inetutils telnetd |
| `tcp` | `tcp` | 22025 / 22026 / 22027 | 9999 / 9998 / 9997 | 回显 / 汇聚 / 源端 |

端口全部在 22022–22027：高于 1024，不需要 root，也不会撞本机 sshd(22) 或常见的 2222。

## 2. 密钥模型

```
keys/
├── client/id_ed25519{,.pub}          # 唯一的客户端密钥
├── target/ssh_host_ed25519_key{,.pub}
├── bastion/ssh_host_ed25519_key{,.pub}
└── known_hosts                       # 从上面两把 host key 生成
```

三条规则：

1. **不碰系统密钥。** 客户端一律 `-F /dev/null -i keys/client/... -o IdentityAgent=none
   -o IdentitiesOnly=yes`。不读 `~/.ssh/config`，不问 `ssh-agent`。
   `IdentityAgent=none` 是关键：macOS 的钥匙串授权弹窗来自 agent，不来自 ssh。
2. **host key 固定且每服务不同。** 固定才能预置 known_hosts 做
   `StrictHostKeyChecking=yes` 的严格校验；不同才能让「换了钥匙要拒绝」有意义。
3. **能故意换。** `bash tests/docker/gen-keys.sh rotate target` 强制重生成，
   用于负向测试。固定不等于不可变。

密钥不入库（`.gitignore` 已排除 `tests/docker/keys/`），首次运行时由
`gen-keys.sh` 幂等生成。

## 3. 为什么这么设计

**为什么是 Debian 不是 Alpine。** Alpine 的 openssh 不带 PAM，
`KbdInteractiveAuthentication` 没有后端，服务端直接回 failure。那测的是
「这个镜像不支持」，不是「我们的实现对不对」。

**为什么 telnetd 用 socat 兜着。** Debian 的 telnetd 来自 GNU inetutils，
**没有** `-p` / `-debug`（那是 BSD 版参数），也从不自己监听端口——按 inetd 约定
它从 stdin/stdout 继承已建立的 socket。容器里没有 inetd，用 socat 扮演：
`TCP-LISTEN:2323,fork EXEC:/usr/sbin/telnetd`。

**为什么 SFTP 没有独立容器。** SFTP 不是独立守护进程，它是 sshd 的一个子系统。
单独起容器只会得到一个「不知道自己在测什么」的 sshd。

**为什么 chroot 用户单独一个。** `ChrootDirectory` 必须写在 `Match User` 块里。
把 tester 也锁进笼子，隧道与 ProxyJump 立刻失效。

**为什么 TCP 分三个端口。** 回显（原样）、汇聚（回 SHA256）、源端（发 SHA256
头 + 字节）。把「通道能通」和「字节没丢」拆成两条判据，出问题时能一次定位到
是哪一层。

## 4. 踩过的坑（改这个目录前先读）

| 症状 | 根因 | 处置 |
|---|---|---|
| `telnetd: invalid option -- 'd'` | GNU inetutils 用 `-D`，且不监听端口 | 用 socat 转发 |
| `No ED25519 host key is known ... strict checking` | `ssh -o` 的**值按空格分词**，仓库路径 `Craft Anything` 含空格，known_hosts 路径被劈成两段 | 值用 ssh 配置语法的双引号包住：`-o "UserKnownHostsFile=\"…\""` |
| bash 报 `out?: unbound variable` | macOS 自带 bash 3.2 对多字节字符边界判断有缺陷，`$out（` 把全角括号首字节并进变量名 | 所有紧邻中文的变量写成 `${out}` |
| telnet 二进制首字节是 `0x0a` | 发 `cmd\r\n` 时 `\r` 结束行，残留 `\n` 留在输入队列，`stty raw` 后被当数据读走 | 只发 `\r` |
| SFTP/SCP 能下载不能上传 | `/payload` 由 root 建，tester 无写权限 | 构建时 `chown tester:tester /payload` |
| chroot 用户「读不到 /etc/passwd」却通过 | 该用户没装公钥，根本没连上，负向断言假通过 | 装公钥，并先断言「确实登上去了」 |
| 改完 entrypoint 仍然失败 | `compose up` 不换已有容器，继续跑旧镜像 | `--force-recreate` |

## 5. 加一个新协议

1. 需要新守护进程 → 在 `Dockerfile` 装包，在 `entrypoint.sh` 加一个 `ROLE` 分支。
2. 需要新端口 → 在 `docker-compose.yml` 加映射，在 `protocol-verify.sh` 顶部加
   `P_XXX`，`wait_port` 列表补一条。
3. 客户端探针 → 放 `tests/docker/xxx_probe.py`。**不要依赖系统装的同名客户端**：
   它可能不存在，或者行为随版本漂移（telnet 就是例子）。
4. 断言要有**可对比的证据**，不是「没报错」。二进制用 SHA256，文本用明确的
   标记字符串，负向测试要断言错误消息而不只是非零退出码。

## 6. 与 CI 的关系

`protocol-verify.sh` 无编译依赖，适合做 PR 的前置门禁（秒级）。
`docker-verify.sh` 要编译 Rust，适合合入前跑。两者共用同一套容器与密钥，
不会出现「门禁过了但集成挂了」这种因环境分叉导致的假绿。
