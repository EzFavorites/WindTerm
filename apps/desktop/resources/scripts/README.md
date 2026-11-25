# 随附脚本示例

这些 `.rhai` 文件随产品交付，是可直接复制修改的起点，不是「必须这样写」的答案。
真实注册名与语义见 [`docs/scripting.md`](../../../docs/scripting.md)；复制后把主机、
用户名、口令提示词、本地输出路径改成你的环境即可。

| 文件 | 做什么 | 运行要点 |
|---|---|---|
| `00-template.rhai` | 编辑器「新建」给出的起始模板 | 不需要额外授权 |
| `01-auto-login.rhai` | 自动登录（SSH 配置 + 口令提示应答） | `--arg <host> --arg <user>`；Agent 认证无需保险库 |
| `02-batch-inspection.rhai` | 批量巡检落盘 | `--batch --arg <host>...`；脚本头已声明 `/tmp` 写权限 |
| `03-capture-output.rhai` | 抓一条命令的输出并存文件 | 脚本头已声明 `/tmp` 写权限 |
| `04-multi-tab-dispatch.rhai` | 多标签逐一下发命令 | 两个本地 shell 标签，序号从 1 起 |
| `05-auto-reply.rhai` | 按首个命中提示自动应答 | `--batch`；提示语没出现时显式失败 |

带本地文件操作的示例用 `// @cap fs=write:/tmp` 脚本头授权；改成别的目录时，**头和路径
都要改**（或用 CLI `--allow fs=write:<你的目录>` 追加）。授权只对本次运行有效。

在 GUI 里：脚本库打开任一文件 →「运行」；在命令行里：

```sh
term-desk script run apps/desktop/resources/scripts/03-capture-output.rhai --batch
```
