# 构建与发布

## 工具链

- Rust **1.94** 或更新（各 crate 声明 `rust-version = "1.94"`）。
- 发布门禁与 CI 钉 **1.94.0**：`.github/workflows/ci.yml` 与 `release.yml` 都用
  `RUSTUP_TOOLCHAIN=1.94.0` 覆盖，避免 runner 预装的 `stable` 悄悄换版本。
- 编辑器/诊断工具（`rustfmt`、`clippy`）不是构建必需，但提交前建议跑。

## 系统依赖

### Linux

```sh
sudo apt-get install -y --no-install-recommends \
  build-essential pkg-config libssl-dev \
  libxkbcommon-dev libwayland-dev libx11-dev libxi-dev \
  libxcursor-dev libxrandr-dev libgl1-mesa-dev \
  libfontconfig1-dev \
  openssh-server openssh-sftp-server lrzsz
```

- X11 / Wayland 头文件：`eframe` 的窗口后端。
- `libssl-dev` 与 `build-essential`：`russh` 的 ring 后备需要 C 工具链。
- `openssh-sftp-server` / `lrzsz`：部分端到端测试用真实 `sftp-server` 与
  `lrz` / `lsz` 跑握手与传输。

### macOS / Windows

- macOS：装 Xcode Command Line Tools 即可（`xcode-select --install`）。
- Windows：装 MSVC 工具链（Visual Studio Build Tools，勾选 C++ 生成工具）。

## 本地构建

```sh
cargo check --workspace          # 类型检查
cargo test --workspace           # 全部测试
cargo build -p term-desk         # 桌面应用（产物在 target/debug/）
cargo build -p term-desk --release
```

桌面二进制叫 `term-desk`（crate 名）。发布包会把它重命名为产品名
`WindTerm` / `windterm`（见下节）。

## 无窗口子命令

`term-desk` 在窗口起来之前先处理这些子命令，适合 CI 与排障现场；它们不建
会话池，`api` / `mcp` 只是前台 UDS 控制端点的薄客户端：

```sh
term-desk doctor                  # 环境体检（shell / profile / 主题 / 能力面）
term-desk capability              # 能力面统计（--json / --all）
term-desk api describe            # 问前台要动词清单（需前台已运行）
term-desk api call <method> '{"json":"params"}'
term-desk mcp                     # stdio MCP server（tools 来自 describe）
term-desk --version
```

## 完整门禁（可选）

`scripts/verify.sh` 是项目自己的验收脚本：分层与依赖边界、能力注册、冻结基线、
单测、金样本、可选的视觉回归都在里面。CI 只跑 `cargo build` 与 `cargo test`；
完整门禁在发布前跑：

```sh
bash scripts/verify.sh                    # 常规验收
SKIP_GIT_CHECK=1 bash scripts/verify.sh   # 提交前跑（跳过工作树干净检查）
VERIFY_CLIPPY=1 bash scripts/verify.sh    # 额外跑 clippy（-D warnings）
```

公开检出应包含内部台账 `docs/facts.txt`；只有缺失时才跳过引用它的那一步（旧版「内部台账不随公开仓库发布」的口径已作废）。

## 打包

`scripts/package-release.sh` 产出发布包，并被 release workflow 直接调用：

```sh
bash scripts/package-release.sh <target-triple> <version> <binary> [out-dir]
```

| 平台 | 产物 |
|---|---|
| macOS | `WindTerm-<ver>-macos-<arch>.tar.gz`，内含 `WindTerm.app/` |
| Linux | `WindTerm-<ver>-linux-x86_64.tar.gz`，内含 `windterm`、`.desktop`、图标 |
| Windows | `WindTerm-<ver>-windows-x86_64.zip`，内含 `WindTerm.exe` |

每个包旁都有 `.sha256`。macOS 的 `.app` 会做 ad-hoc 签名（`codesign -s -`），
不做公证；首次启动需要右键 → 打开，或去掉 quarantine 属性。

## CI

`.github/workflows/ci.yml`：每次 push 到 `main` 或 PR 时，在 `ubuntu-22.04` 上
跑 `cargo build --workspace --locked` 与 `cargo test --workspace --locked`。

## 发布

`.github/workflows/release.yml`：

1. 推 `v*` 标签（或手动派发）触发。
2. `meta` job 从 `apps/desktop/Cargo.toml` 读版本号；tag 与版本不一致只告警。
3. `build` job 四平台矩阵：macOS arm64 / macOS x86_64（交叉编译）/ Linux
   x86_64 / Windows MSVC。
4. 每个平台构建、打包、校验包内结构、上传 artifact。
5. tag 触发时创建 **draft** Release（先人工看一眼再发布）。

版本号的唯一来源是 `apps/desktop/Cargo.toml` 的 `version`，不要在 workflow 里
另写一份。