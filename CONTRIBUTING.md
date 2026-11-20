# 参与开发

## 环境

- Rust 1.94 或更新（见根目录 `rust-toolchain.toml`）。
- Linux 还需要 X11/Wayland 头文件与 C 工具链，清单见
  [`docs/building.md`](docs/building.md)。

```sh
cargo build --workspace
cargo test --workspace
```

## 提交前

最小要求：

```sh
cargo test --workspace
```

更完整的门禁（分层边界、能力注册、冻结基线、金样本、可选视觉回归）：

```sh
SKIP_GIT_CHECK=1 bash scripts/verify.sh
VERIFY_CLIPPY=1 bash scripts/verify.sh   # 额外跑 clippy -D warnings
```

门禁是判据，不是建议：任一红项先修，不要用「看起来差不多」放过。

## 代码边界

这些约束由 `scripts/verify.sh` 检查，改代码时请遵守：

- `term-core` 不引入 `egui` / `eframe` / `epaint` / `winit`，也不做 I/O。
- `tokio` 只出现在 `term-ssh` / `term-tunnel` / `term-sftp`；桌面 UI 里零 `.await`。
- 依赖方向单向：`apps/desktop → term-app → term-core`。
- 单文件代码行数（不含空行与注释）有上限，超了就拆模块。
- 生产路径的 `panic` 与错误边界有冻结基线，只许减不许增。

## 提交信息

提交信息用一句话说明「为什么改」，而不是「改了哪个文件」。行为变化、修复的
回归、以及被否掉的方案都值得写进正文。示例：

```
fix(ssh): 多跳链顺序排反，跳板会话选了不生效

链的顺序按「离本机由近到远」，而建连顺序正好相反。
顺带把跳板复用扩到整条链。
```

## Pull Request

1. 从 `main` 切分支，保持一个 PR 只做一件事。
2. 附上验证方式：跑了哪些测试 / 门禁，真实场景怎么复现。
3. 涉及界面变化时附截图；涉及协议或安全行为时说明边界情况。
4. CI 必须绿。

## 文档

- 用户可见的行为变化同步更新 `docs/user-guide.md`。
- 架构约束变化同步更新 `docs/architecture.md`。
- 模块设计变化同步更新 `docs/design/` 下对应篇目；源码注释引用设计文档时指向
  `docs/design/<篇目>.md`。
- 不要在文档里写死会漂移的数字（版本号、测试条数），以命令输出为准。