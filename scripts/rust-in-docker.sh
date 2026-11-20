#!/usr/bin/env bash
# 在容器里跑 cargo：宿主 `target/` 不被占用，多个工位可以同时编译与测试。
#
#   bash scripts/rust-in-docker.sh test -p term-xfer
#   bash scripts/rust-in-docker.sh clippy -p term-desk --all-targets -- -D warnings
#   bash scripts/rust-in-docker.sh check --workspace
#
# # 为什么要有这个壳
#
# 宿主上两个 `cargo test` 会抢同一个 `target/` 的文件锁：后到的那个静静排队，
# 直到前面那个跑完。平时只是慢，赶上 `verify.sh` 就是超时被杀——2026-11-09
# 那次 13 分钟被杀掉就是三个工位同时在编译。容器里给独立的
# `CARGO_TARGET_DIR`（命名卷，落在 OrbStack 的 Linux 侧，不写 APFS），互不相干。
#
# # 真机对端测试也走这里
#
# 凡是要在测试里拉起**真实外部程序**当对端的，默认在容器里跑，不在宿主跑：
#
#   bash scripts/rust-in-docker.sh test -p term-zmodem --test interop -- --nocapture
#   bash scripts/rust-in-docker.sh test -p term-sftp --test e2e -- --ignored --test-threads=1
#
# 镜像自带 `lrzsz`（0.12 线）、`socat` 与 Debian 的
# `/usr/lib/openssh/sftp-server`；宿主 macOS 那份在 `/usr/libexec/sftp-server`。
# 两代真服务端都过，才算「能跟真的 SFTP 谈」——`sftp_server_binary()` 认的
# 就是这两条路径，`--ignored` 时缺任何一份都由 `require_sftp_server` 硬失败。
# 注意这条 e2e 自己起 russh 服务端，不需要 `tests/docker` 那个 sshd 测试床；
# 容器在这里提供的是**服务端二进制**，不是网络连接。
# PTY 由内核提供，跑法与宿主完全等价。
# 好处不只是躲开 target 锁：这类测试会拉子进程、占 PTY、对端不说话时挂十几秒，
# 跑挂了就 `docker rm`，宿主上不会留孤儿 `sz`/`rz`。宿主那边是 0.13 线，两代
# 都过才算稳。
#
# 实测（2026-11-09，同一份代码）：
#
#   term-zmodem --test interop      容器 lrzsz 0.12.21rc → 2 passed
#                                   宿主 lrzsz 0.13.1     → 2 passed
#   term-desk --test xfer_lifecycle 容器                  → 11 passed
#   term-desk --test xfer_run       容器                  → 14 passed
#
# # 一个必须知道的差异：容器的 rustc 比宿主新
#
#   宿主 rustc 1.94.0 ／ 容器 rustc 1.99.1（`rust:1-bookworm` 是滚动 tag）
#
# 后果：容器里编既有代码会多出宿主没有的 lint（当前是 70 条 `1.0` 浮点推断，
# 即 `f32: From<f64>` 那条未来硬错误的前瞻告警）。所以：
#
#   · **测试**在容器跑（行为与宿主一致，结论可采信）；
#   · **`clippy -D warnings` 以宿主为准**，容器跑出来的新 lint 不是回归。
#
# 要彻底消掉这个差异，把 Dockerfile 的 `FROM` 钉到 `rust:1.94-bookworm` 与宿主
# 同代即可（代价是所有依赖在卷里重编一遍，故留作独立决定）。
#
# # 什么能放进来
#
# **能**：任何 `crates/*` 的 check / build / test / clippy。纯逻辑，无窗口。
# **也能**：`apps/desktop` 的 check 与单测（镜像里装了 X11 / Wayland / GL 开发库，
#       eframe + wgpu 的 `build.rs` 探针能过），以及用真 PTY 的端到端（`sz`/`rz`）。
# **不能**：`scripts/screenshot-baseline.sh` 与 `verify.sh` 的视觉回归——它们要
#       真窗口与 Metal 后备，只能在 macOS 宿主跑。别把整条 `verify.sh` 塞进来。
#
# # 第一次会慢
#
# 首次运行要拉 `zterm-rust:dev` 镜像并在命名卷里从头编译一遍依赖（egui/wgpu
# 那一坨，几分钟）。之后每次只编改动的 crate。宿主 `target/` 一个字节没动过，
# 所以两条路各有一份依赖产物——这是刻意的：共用一份就要抢锁，等于没解决。
#
# 环境变量：`ZTERM_RUST_IMAGE` 换镜像名、`ZTERM_CARGO_VOL` 换 target 卷名、
# `ZTERM_CARGO_HOME_VOL` 换 CARGO_HOME 卷名（改卷名 = 从零编一次）。
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${ZTERM_RUST_IMAGE:-zterm-rust:dev}"
TARGET_VOL="${ZTERM_CARGO_VOL:-zterm-cargo-target}"
CARGO_HOME_VOL="${ZTERM_CARGO_HOME_VOL:-zterm-cargo-home}"

if ! command -v docker >/dev/null 2>&1; then
  echo "✗ 找不到 docker。容器路需要 OrbStack / Docker Desktop 在跑。" >&2
  exit 1
fi

if [ "$#" -eq 0 ]; then
  echo "用法：bash scripts/rust-in-docker.sh <cargo 子命令> [参数…]" >&2
  echo "例如：bash scripts/rust-in-docker.sh test -p term-xfer" >&2
  exit 2
fi

# 镜像不在就现建。用 `inspect` 而不是 `images | grep`：后者会被同名前缀骗。
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  echo "· 首次使用：构建 ${IMAGE}（apt 装 GUI 开发库 + lrzsz，之后走缓存）" >&2
  docker build -t "$IMAGE" -f "$REPO/scripts/docker/rust.Dockerfile" "$REPO/scripts/docker" >/dev/null
fi

# 宿主路径含空格（`…/Craft Anything/…`），`-v` 的每一处都要引号。
#
# `--init`：cargo test 会起子进程（PTY、`sz`/`rz`、`sh -c`），没有 init 时
# 被杀掉的容器会留一堆孤儿。`-i` 不加 `-t`：非交互，避免 "the input device
# is not a TTY" 之类的噪音。
exec docker run --rm -i --init \
  -v "$REPO":/src \
  -w /src \
  -v "$TARGET_VOL":/target \
  -v "$CARGO_HOME_VOL":/usr/local/cargo \
  -e CARGO_TARGET_DIR=/target \
  -e CARGO_HOME=/usr/local/cargo \
  -e CARGO_TERM_COLOR=never \
  -e CARGO_INCREMENTAL=0 \
  "$IMAGE" cargo "$@"
