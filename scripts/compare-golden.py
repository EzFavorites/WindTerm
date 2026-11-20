#!/usr/bin/env python3
"""并排比对：我们的截图 vs WindTerm 原程序视觉基线。

用法见 docs/design/11-theme-and-visual-baseline.md。设计要点：

* 视觉基线**只读**，哈希写死在下面；被人改过就直接报错退出，防止「把标准
  往自己这边挪」——这正是过去只有回归基线时永远不像 WindTerm 的根因。
* 数字只用来看**趋势**，不做 hard fail：视觉基线与我们的会话名、当前页本来
  就不可能逐像素相同。结构是否达标由规格表（docs/design/11-theme-and-visual-baseline.md）
  逐项人工打勾。
* 依赖仅 Pillow + numpy（本机已有），不进 Cargo 依赖树。
"""

import argparse
import hashlib
import sys
from pathlib import Path

from PIL import Image, ImageDraw
import numpy as np

ROOT = Path(__file__).resolve().parent.parent

# 2026-11-07 从主工作区 reference/ 复制时的字节哈希。改图 = 改标准 = 作弊。
GOLDEN_SHA256 = {
    "breadcrumb": "b82d91f74d33acc314a7b4463e40282608bb960278afdaca00154b824ca29a9d",
    "main-dark": "0290f26ddf89ca8be99b8ffbdbde8b088233a4c71373b2be0e3ed835a8bd23ac",
    "main-light": "f2c6686ca6debd56d095a30bbd554e295b27cacb26c303f625c4803c54a0c160",
    "main-split": "ad1b30bcd6e1a00cbe78a00e870458f115037ba249036a6457e8bb1b0741c9ce",
    "main-window": "5ee0bdb9aaa9d116084a2610d4a23aa7c2c5414a87396416a2df0d4e3f91a395",
    "session-property": "9aa2116b043fd214cac2fdf940e341d9020e155b7dbfda5f5091e097364a7e22",
}


def load_golden(name: str) -> Image.Image:
    path = GOLDEN_DIR / f"{name}.png"
    if not path.exists():
        sys.exit(f"找不到视觉基线：{path}")
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    expected = GOLDEN_SHA256.get(name)
    if expected is None:
        sys.exit(f"视觉基线 {name} 未登记哈希；先确认来源再加进 GOLDEN_SHA256")
    if digest != expected:
        sys.exit(
            f"视觉基线被改动了！\n  {path}\n  期望 {expected}\n  实际 {digest}\n"
            "L2 视觉基线只读，不允许重冻结（见 docs/design/11-theme-and-visual-baseline.md）。"
        )
    return Image.open(path).convert("RGB")


def metrics(a: Image.Image, b: Image.Image) -> dict:
    """归一到同一尺寸后的三个可解释数字。"""
    b = b.resize(a.size, Image.LANCZOS)
    x = np.asarray(a, dtype=np.int16)
    y = np.asarray(b, dtype=np.int16)
    mad = float(np.abs(x - y).mean())

    lum_x = x.mean(axis=2)
    lum_y = y.mean(axis=2)
    hx = np.histogram(lum_x, bins=32, range=(0, 256))[0].astype(float)
    hy = np.histogram(lum_y, bins=32, range=(0, 256))[0].astype(float)
    hx /= hx.sum()
    hy /= hy.sum()
    hist_l1 = float(np.abs(hx - hy).sum() / 2)  # 0..1

    edge = lambda g: float(np.abs(np.diff(g, axis=0)).mean() + np.abs(np.diff(g, axis=1)).mean()) / 2
    return {"mad": mad, "hist_l1": hist_l1, "edge_golden": edge(lum_x), "edge_ours": edge(lum_y)}


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--golden", required=True, choices=sorted(GOLDEN_SHA256))
    p.add_argument("--shot", required=True, help="我们的截图路径")
    p.add_argument("--crop", help="先裁我们的画布：x,y,w,h（组件图只占画布中间一块）")
    p.add_argument("--out", default="/tmp/golden-compare.png", help="并排图输出路径")
    args = p.parse_args()

    golden = load_golden(args.golden)
    shot_path = Path(args.shot)
    if not shot_path.exists():
        sys.exit(f"找不到我们的截图：{shot_path}")
    ours = Image.open(shot_path).convert("RGB")
    if args.crop:
        x, y, w, h = map(int, args.crop.split(","))
        ours = ours.crop((x, y, x + w, y + h))

    m = metrics(golden, ours)
    print(f"视觉基线 {args.golden}: {golden.size}   我们: {ours.size} (裁切后)")
    print(f"  平均绝对色差 MAD        {m['mad']:7.2f} / 255   （越低越接近）")
    print(f"  亮度直方图 L1           {m['hist_l1']:7.4f} / 1     （明暗分布差）")
    print(f"  边缘密度 视觉基线/我们      {m['edge_golden']:7.2f} / {m['edge_ours']:7.2f}  （控件密度差）")
    print("  注：数字只看趋势，结构判据见 docs/design/11-theme-and-visual-baseline.md")

    # 并排：左视觉基线、右我们，各缩到同高
    H = 620
    def fit(im):
        return im.resize((round(im.width * H / im.height), H), Image.LANCZOS)
    g, o = fit(golden), fit(ours)
    pad, head = 16, 40
    W = g.width + o.width + pad * 3
    sheet = Image.new("RGB", (W, H + head + pad * 2), (24, 24, 24))
    d = ImageDraw.Draw(sheet)
    sheet.paste(g, (pad, head + pad))
    sheet.paste(o, (pad * 2 + g.width, head + pad))
    d.text((pad, 12), f"GOLDEN  WindTerm  {args.golden}  (read-only)", fill=(255, 220, 90))
    d.text((pad * 2 + g.width, 12), "OURS  current build", fill=(120, 220, 255))
    out = Path(args.out)
    sheet.save(out)
    print(f"  并排图 → {out}")


if __name__ == "__main__":
    main()
