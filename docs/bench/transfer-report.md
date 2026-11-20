# 传输基准报告

本文件是 `scripts/transfer-bench.sh` 的报告落点。基准由脚本在容器里跑，结果按实测填写，不在文档里预置数字。

## 怎么跑

```sh
# 默认：8 MiB × 3 轮，报告落 /tmp
bash scripts/transfer-bench.sh

# 加大载荷与轮次，并直接写回本文件
bash scripts/transfer-bench.sh --size-mb 32 --rounds 5 --out docs/bench/transfer-report.md

# 复用已有镜像 / 跑完保留容器
SKIP_BUILD=1 bash scripts/transfer-bench.sh
KEEP=1 bash scripts/transfer-bench.sh
```

## 测什么

| 维度 | 口径 |
| --- | --- |
| 上传 | 本地 → SFTP 会话，固定块大小，统计墙钟与吞吐 |
| 下载 | SFTP → 本地，同上 |
| 协议内传输 | X / Y / ZMODEM 链路，经 PTY 通道 |
| 取消 | 传输中途取消，确认分片状态与队列标记 |

## 环境

跑基准时记录：CPU 型号、内存、容器镜像、磁盘类型、SSH 加密算法。不同环境的数字不可横向比较，只用于同一台机器上的回归对比。

## 结果

运行 `--out docs/bench/transfer-report.md` 后由脚本写入下表。未跑过的环境不要填数字。

| 日期 | 环境 | 方向 | 载荷 | 轮次 | 吞吐 | 备注 |
| --- | --- | --- | --- | --- | --- | --- |
| — | — | — | — | — | — | 待实测 |

## 判读

- 只看同一环境下的趋势，不跨机器比较绝对值。
- 吞吐下降超过 20% 视为回归，先查加密算法、块大小与磁盘缓存，再怀疑代码。
- 取消与续传的正确性由测试覆盖，不以基准结果代替。