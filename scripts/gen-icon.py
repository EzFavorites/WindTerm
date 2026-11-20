#!/usr/bin/env python3
"""生成 WindTerm 应用图标（PNG / ICO / ICNS）。

为什么用程序画而不是拿一张位图：应用图标要在 16×16 的 Dock / 任务栏里仍然
可辨，笔画宽度、留白、圆角半径都得按比例算。位图缩放出来在小尺寸下会糊成
一团色块。这里按 1024 画、4× 超采样后缩，保证每个尺寸都是干净边缘。

风格跟项目的 UI 取向一致：粗黑描边 + 高饱和底色 + 无渐变（Neo-Brutalism）。
符号用 `>_`——终端的通用记号，比任何抽象图形在 16px 下都更容易认。

用法：
    python3 scripts/gen-icon.py            # 写到 assets/
    python3 scripts/gen-icon.py --out DIR

产物：
    assets/icon.png      1024×1024（源，也用于窗口图标）
    assets/icon.ico      Windows（16/24/32/48/64/128/256 多尺寸）
    assets/icon.icns     macOS（iconutil 生成；非 macOS 上跳过并打印提示）
"""
from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw

# 配色：糖果黄底 + 近黑描边。不用纯黑（#000）是因为纯黑在深色 Dock 背景上
# 会和背景糊在一起；#1A1A1A 同样够重，但保留一点边界。
BG = (255, 217, 61, 255)
INK = (26, 26, 26, 255)

SIZE = 1024
SS = 4  # 超采样倍数


def draw(size: int) -> Image.Image:
    s = size * SS
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    # 外框：圆角方块 + 粗描边。圆角半径按 18% 走，接近 macOS 大图标的观感，
    # 但不用 iOS 那种「超椭圆」——直角系更贴 Neo-Brutalism。
    radius = int(s * 0.18)
    border = int(s * 0.052)
    d.rounded_rectangle(
        [0, 0, s - 1, s - 1], radius=radius, fill=BG, outline=INK, width=border
    )

    # `>` 与 `_`。整体范围要落在 0.25–0.75，即**几何居中**——第一版把重心放到了
    # 0.29–0.79，右侧留白比左侧多一截，缩小到 32px 时看起来像整体偏左。
    stroke = int(s * 0.082)
    half_w = int(s * 0.105)
    half_h = int(s * 0.150)
    cx = int(s * 0.355)
    cy = int(s * 0.500)

    # `>`：两条斜线。joint="curve" 让拐角是圆的，否则尖角在高倍缩小时会出现
    # 一个突兀的缺角（PIL 的 line 在拐点处不做连接处理）。
    d.line(
        [(cx - half_w, cy - half_h), (cx + half_w, cy)],
        fill=INK, width=stroke, joint="curve",
    )
    d.line(
        [(cx + half_w, cy), (cx - half_w, cy + half_h)],
        fill=INK, width=stroke, joint="curve",
    )
    # 两端补圆头，避免斜线端点是方切口（与 joint 的圆角不一致）。
    for px, py in (
        (cx - half_w, cy - half_h),
        (cx + half_w, cy),
        (cx - half_w, cy + half_h),
    ):
        d.ellipse(
            [px - stroke // 2, py - stroke // 2, px + stroke // 2, py + stroke // 2],
            fill=INK,
        )

    # `_`：右下角的一条粗横线，与 `>` 的下端对齐在同一条基线上。
    #
    # 左端要从 0.560 起，不是 0.505：`>` 的**视觉**右端是 `cx + half_w + stroke/2`
    # = 0.501（笔画是圆的，会往外鼓出半个线宽）。按坐标算 0.505 看似留了缝，
    # 实际两个圆头已经叠在一起，缩到 32px 时糊成一个「⌐」形。
    ux0 = int(s * 0.560)
    ux1 = int(s * 0.800)
    uy = cy + half_h
    d.line([(ux0, uy), (ux1, uy)], fill=INK, width=stroke)
    for px in (ux0, ux1):
        d.ellipse(
            [px - stroke // 2, uy - stroke // 2, px + stroke // 2, uy + stroke // 2],
            fill=INK,
        )

    return img.resize((size, size), Image.LANCZOS)


def build_iconset(out_dir: Path, base: Image.Image) -> Path:
    """macOS iconset：iconutil 只认这套固定文件名。"""
    iconset = out_dir / "WindTerm.iconset"
    if iconset.exists():
        shutil.rmtree(iconset)
    iconset.mkdir(parents=True)
    for px in (16, 32, 128, 256, 512):
        base.resize((px, px), Image.LANCZOS).save(iconset / f"icon_{px}x{px}.png")
        base.resize((px * 2, px * 2), Image.LANCZOS).save(
            iconset / f"icon_{px}x{px}@2x.png"
        )
    return iconset


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="assets", help="输出目录（默认 assets/）")
    args = ap.parse_args()

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    base = draw(SIZE)
    png = out / "icon.png"
    base.save(png)
    print(f"  {png}  {SIZE}×{SIZE}")

    ico = out / "icon.ico"
    base.save(
        ico,
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    print(f"  {ico}  多尺寸")

    iconset = build_iconset(out, base)
    icns = out / "icon.icns"
    if shutil.which("iconutil"):
        subprocess.run(
            ["iconutil", "-c", "icns", str(iconset), "-o", str(icns)], check=True
        )
        shutil.rmtree(iconset)
        print(f"  {icns}")
    else:
        print(f"  跳过 icon.icns：本机没有 iconutil（留 {iconset} 供下次用）")
        print("  macOS 的 .app 打包脚本会在 macos runner 上用 iconutil 现生成。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
