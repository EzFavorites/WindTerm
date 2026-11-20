# 多协议测试床 —— 一个镜像，三种角色（sshd / telnetd / tcp）。
#
# 为什么是一个镜像而不是三个：三个角色共享同一套「测试用户、密钥挂载、诊断
# 工具」。拆成三个 Dockerfile 会让 `tester:testerpass` 和路径约定散落三处，
# 改一处漏两处是最典型的样板债。角色差异只在 entrypoint 的 `ROLE` 分支里。
#
# 为什么是 Debian 而不是 Alpine：Alpine 的 openssh 不带 PAM，
# `KbdInteractiveAuthentication` 没有可用的交互后端，服务端直接回 failure。
# 那样测出来的只是「这个镜像不支持」，而不是「我们的实现对不对」。Debian 12
# 带 PAM，键盘交互会真的问一句 `Password:`。
#
# 装进来的东西都有明确用途，没有一个是「为了看起来完整」：
#   openssh-server / openssh-client  —— 被验证的服务端；也是 SFTP/SCP 的载体
#   inetutils-telnetd                —— 真 telnet 服务端（IAC 协商、PTY 分配）
#   socat                            —— TCP 回显 / 汇聚 / 源端三种角色
#   netcat-openbsd                   —— 容器内健康检查
#   tmux                             —— `tmux -CC` 控制模式的真实输出
#   lrzsz                            —— `sz`/`rz` 真实对端。term-xfer 与 term-zmodem
#                                       的互通测试只在宿主机装了 lrzsz 时才跑，镜像
#                                       里带一份可以把「远端跑 sz、本地收」这条链路
#                                       也纳入验证，而不是只跑本机管道。
FROM debian:12-slim

ENV DEBIAN_FRONTEND=noninteractive

# 两个测试用户：tester 走 ssh/sftp/scp 全权限；sftponly 被 chroot，只做 SFTP。
# 分开的原因是 `ChrootDirectory` 必须写在 `Match User` 块里，混用会把 tester
# 也锁进笼子，隧道与 ProxyJump 立刻失效。
#
# /payload 要 chown 给 tester：它由 root 建出来，而 SFTP/SCP 上传是 tester 在做。
# 不改属主的话，上传会以「Permission denied」失败而下载正常——这种一半能用的
# 症状最容易被误判成客户端问题。
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      openssh-server openssh-client \
      inetutils-telnetd \
      socat netcat-openbsd \
      tmux bash lrzsz ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -m -s /bin/bash tester \
 && echo 'tester:testerpass' | chpasswd \
 && useradd -m -d /sftp/sftponly -s /usr/sbin/nologin sftponly \
 && echo 'sftponly:sftppass' | chpasswd \
 && mkdir -p /home/tester/.ssh /run/sshd /payload /sftp/sftponly/incoming \
 && chown -R tester:tester /home/tester/.ssh \
 && chown tester:tester /payload \
 && chown root:root /sftp/sftponly \
 && chmod 755 /sftp/sftponly \
 && chown sftponly:sftponly /sftp/sftponly/incoming \
 # chroot 根必须是 root 拥有且组/其他人不可写，否则 sshd 拒绝该用户登录。
 && chmod 755 /sftp/sftponly/incoming \
 # telnetd 需要 utmp 记录会话，Debian 12 默认没有这个文件；缺了会让 telnetd
 # 在登录阶段退出，症状是「连上了但没有任何输出」。
 && touch /run/utmp && chmod 664 /run/utmp \
 # Debian 装包时就生成了 host key。留着会让所有容器共用同一把钥匙，
 # 「换了钥匙要拒绝」这条测试就永远测不出来（它真的这么骗过我一次）。
 # 删掉，由 entrypoint 决定用挂载的固定 key 还是临时生成。
 && rm -f /etc/ssh/ssh_host_*

COPY entrypoint.sh tcp-sink.sh tcp-source.sh /usr/local/bin/
RUN chmod 755 /usr/local/bin/entrypoint.sh \
            /usr/local/bin/tcp-sink.sh \
            /usr/local/bin/tcp-source.sh

# 22 ssh/sftp/scp · 2323 telnet · 9999 回显 · 9998 汇聚 · 9997 源端
EXPOSE 22 2323 9997 9998 9999

# 健康检查按角色在 compose 里定义；这里只留 sshd 的兜底，避免单跑容器时无探针。
HEALTHCHECK --interval=2s --timeout=2s --start-period=3s --retries=30 \
  CMD nc -z 127.0.0.1 22 || exit 1

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
