# 容器里的 Rust 构建环境（`scripts/rust-in-docker.sh` 用）。
#
# 为什么要有它：宿主上同时跑两个工位的 `cargo test` 会抢同一个 `target/` 的
# 文件锁，一抢就是十几分钟串行，还容易把 `verify.sh` 拖过超时线。容器里给
# 独立的 `CARGO_TARGET_DIR`（命名卷，落在 OrbStack 的 Linux 侧，不进 APFS），
# 两边就互不相干了。
#
# 为什么要装这一堆 X11 / Wayland / GL 库：`apps/desktop` 依赖 eframe + wgpu，
# 它们的 `build.rs` 与 pkg-config 探针在裸 `rust` 镜像上会直接失败。装完能
# **编译与跑单测**；**截图仍然只能在 macOS 宿主跑**（要真窗口与 Metal 后备）。
#
# `lrzsz` 顺带装上：`crates/term-zmodem/tests/interop.rs` 需要真的 `sz`/`rz`，
# 容器里这份是 0.12 线，宿主 brew 那份是 0.13 线——两代都过才算稳。
#
# `openssh-sftp-server` 同理：`crates/term-sftp/tests/e2e.rs` 把 SFTP 通道接到
# 真实的 `/usr/lib/openssh/sftp-server`。宿主 macOS 那份在
# `/usr/libexec/sftp-server`，两代 OpenSSH 都过才算「能跟真的服务端谈」；
# 也顺带让没有 macOS 那套路径的机器跑得了这条真机门。装的是 sftp-server
# 而不是整个 openssh-server：这条 e2e 自己起 russh 服务端，不需要 sshd。
FROM rust:1-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        libxkbcommon-dev \
        libwayland-dev \
        libx11-dev \
        libxi-dev \
        libxcursor-dev \
        libxrandr-dev \
        libxinerama-dev \
        libgl1-mesa-dev \
        libvulkan-dev \
        libasound2-dev \
        libudev-dev \
        lrzsz \
        openssh-sftp-server \
        socat \
    && rm -rf /var/lib/apt/lists/*

# 时区与语言钉死：测试里有比对本地化字符串的用例，容器默认 UTC / POSIX
# 会让它们与宿主结果不一致，且失败原因看起来与代码无关。
ENV TZ=Asia/Shanghai \
    LANG=C.UTF-8
