#!/usr/bin/env python3
"""字形样张的像素判据。

配套夹具是 `--demo glyph`（`apps/desktop/src/demo.rs`）：它往真实的
`Terminal::process` 里喂一段固定字节流，用 truecolor 绿色 `|` 当列标尺，
把 ASCII / CJK / emoji / Nerd Font 私用区 / 组合字符夹在标尺之间，
再由 `ui/terminal/view.rs::paint_rows` 正常画出来。**验的是产品渲染路径**，
不是另画一份假界面。

# 为什么不数 PNG 哈希

要量的是「占了几格、有没有颜色、有没有字形」。逐字节基线只能告诉你
「变了没有」，答不出这三件事，而且字体一升级就会全红。这里输出的是数值。

# 标尺为什么能自足算出宽度（不需要 Rust 告诉脚本 cell_w）

标尺 `|` 是 `Colour::Rgb(0,255,128)`，中间没有主题插手；按 RGB 距离就能把
它从画面里精确抠出来。相邻标尺的 x 间距 = (1 + 样例占格数) × cell_w，
所以只要有一个已知的「1 格」基准就能反推所有样例。

**基准取 ASCII 行，不取尺子行**（2026-11-09 实测踩过）：尺子行是一整个
span 连续 48 个 `|`，egui 按字体 advance 排版；样张行里每个 `|` 是独立
span，位置是 `origin.x + col*cell_w`。两条路径的取整不同，本机实测
尺子行间距 16.0px、ASCII 行间距 31.0px（不是 32）。拿 16 当 cell_w 去除
长序列，误差会随列号累积：mixed 行末段 139px 会被算成 8.69 格而不是 9 格。
改用 ASCII 行间距 A 作「2 格」基准后，`cells = round(2*span/A) - 1`，
所有样例都落在整数上，且与 ppp / 字体 / 窗口尺寸无关。

# 判据分两类

* **结构判据（hard fail）**：行带数、等宽、每个样例的占位格数、ASCII 真的
  画出了墨迹。这些是渲染管线的契约，坏了必须红。
* **能力清单（只报告）**：彩色 emoji、组合字符、grapheme cluster、Nerd Font
  完整度。当前实现里它们都是否定的，但那是**已知缺口**不是回归；将来哪天
  补上了，这个脚本不该因为「变好了」而变红。

依赖仅 Pillow + numpy（本机已有，与 `scripts/compare-golden.py` 一致），
不进 Cargo 依赖树。
"""

import argparse
import hashlib
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image

# 与 apps/desktop/src/demo.rs 的 RULER_ON 一致。屏幕上实际落的是抗锯齿后的
# (7,195,102) / (5,214,110)，所以阈值要留够，但 70 仍远小于主题里的任何绿。
RULER_RGB = np.array([0, 255, 128], dtype=np.int16)
RULER_TOL = 70

# 行序必须与 demo.rs::glyph_bytes 一致。脚本按「第几条有标尺的行带」贴标签。
ROW_LABELS = ["ruler", "ascii", "cjk", "emoji", "nerd", "mixed", "ruler"]

# 占位期望来自 Unicode East Asian Width（term-core::width::is_wide 的实现依据），
# 不是从这张图里读出来的。表若与夹具漂移，下面的结构判据会当场红。
EXPECTED_CELLS = {
    "ascii": [1, 1, 1, 1, 1, 1, 1, 1],
    "cjk": [2, 2, 2, 2, 2, 2, 2, 2],
    # 故意混宽：✅ / ⚡ / ❤ 是一格，其余是两格。
    "emoji": [2, 1, 2, 2, 1, 1, 2, 2],
    "nerd": [1, 1, 1, 1, 1, 1],
    # e+锐音符(逐码位) / 全角A / 半角片假名 / 全角空格 / ZWJ 家庭(逐码位)。
    "mixed": [2, 2, 1, 2, 8],
}

# 全角空格必然没有墨迹，但它仍占两格：这条把「占位不依赖墨迹」钉成判据。
BLANK_SAMPLE = ("mixed", 3)
COLOR_THRESHOLD = 0.05  # colorful_ratio 超过它才算「有彩色」


def ruler_mask(arr):
    d = np.linalg.norm(arr.astype(np.int16) - RULER_RGB, axis=2)
    return d < RULER_TOL


def consecutive_runs(on, min_len=1):
    """布尔数组里的连续 True 段，返回 (start, end) 左闭右开。"""
    out = []
    i = 0
    while i < len(on):
        if on[i]:
            j = i
            while j < len(on) and on[j]:
                j += 1
            if j - i >= min_len:
                out.append((i, j))
            i = j
        else:
            i += 1
    return out


def find_bands(mask, min_px=8, min_height=3):
    """按 y 投影找行带。夹具在每条内容行后留了一整行空白，所以带之间必然断开。"""
    ys = mask.sum(axis=1)
    return consecutive_runs(ys >= min_px, min_height)


def vertical_centers(mask, y0, y1):
    """一条行带里每一根绿色竖线的 x 中心。"""
    runs = consecutive_runs(mask[y0:y1, :].any(axis=0))
    return [(a + b - 1) / 2.0 for a, b in runs]


def background(arr, y0, y1, x_from):
    """从最后一根标尺右侧的空白区取背景色，避免把主题色写死。"""
    patch = arr[y0:y1, x_from : x_from + 120].reshape(-1, 3)
    if patch.size == 0:
        return np.array([0, 0, 0], dtype=np.int16)
    return np.median(patch, axis=0).astype(np.int16)


def sample_metrics(arr, mask, y0, y1, x0, x1, bg):
    """一个样例区间里的墨迹、彩色占比、中心空洞率、墨迹宽度。"""
    a, b = int(x0 + 3), int(x1 - 3)
    if b <= a:
        return dict(ink_px=0, colorful_ratio=0.0, hole_ratio=1.0, ink_bbox_w=0)
    reg = arr[y0:y1, a:b].astype(np.int16)
    ink = (np.linalg.norm(reg - bg, axis=2) > 35) & (~mask[y0:y1, a:b])
    px = reg[ink]
    colorful = 0
    if len(px):
        chroma = (px.max(axis=1) - px.min(axis=1)) > 40
        colorful = int((chroma & (px.max(axis=1) > 60)).sum())
    ys, xs = np.where(ink)
    bbox_w = int(xs.max() - xs.min() + 1) if len(xs) else 0
    # 空心方框（缺字 tofu）的特征：边框有墨、中间空。只作报告用。
    hole = 1.0
    if len(xs) and bbox_w > 8:
        iy0, iy1 = int(ys.min()), int(ys.max())
        inner = ink[iy0 + 3 : iy1 - 2, int(xs.min()) + 3 : int(xs.max()) - 2]
        if inner.size:
            hole = float(1.0 - inner.mean())
    return dict(
        ink_px=int(ink.sum()),
        colorful_ratio=round(colorful / max(1, int(ink.sum())), 4),
        hole_ratio=round(hole, 3),
        ink_bbox_w=bbox_w,
    )


def analyze(path):
    arr = np.asarray(Image.open(path).convert("RGB"))
    mask = ruler_mask(arr)
    bands = find_bands(mask)
    checks = []

    def check(name, passed, detail):
        checks.append(dict(name=name, passed=bool(passed), detail=detail))

    check("行带数 = 7", len(bands) == len(ROW_LABELS), f"实测 {len(bands)}：{bands}")
    if len(bands) != len(ROW_LABELS):
        return finalize(path, arr, checks, [], None, None)

    centers = [vertical_centers(mask, y0, y1) for y0, y1 in bands]
    steps = [np.diff(c) for c in centers]

    # 两把尺子：等宽字体的自检，且两次度量必须一致。
    r1, r2 = steps[0], steps[-1]
    ruler_ok = len(r1) >= 20 and len(r2) >= 20 and r1.std() < 0.6 and r2.std() < 0.6
    check(
        "两把尺子等宽且竖线数 ≥ 20",
        ruler_ok,
        f"首 {len(r1)} 段 std={r1.std():.3f}；尾 {len(r2)} 段 std={r2.std():.3f}",
    )
    check(
        "两把尺子步长一致",
        abs(float(np.median(r1)) - float(np.median(r2))) < 0.6,
        f"{np.median(r1):.2f}px vs {np.median(r2):.2f}px",
    )

    # ASCII 行是「2 格」的相对基准。它同时是被验对象（间距必须处处相等）。
    ascii_step = float(np.median(steps[1]))
    check(
        "ASCII 行标尺间距处处相等",
        float(steps[1].std()) < 0.6,
        f"{len(steps[1])} 段 std={steps[1].std():.3f}",
    )

    def cells_of(span):
        return int(round(2.0 * span / ascii_step)) - 1

    rows = []
    for idx, (label, (y0, y1)) in enumerate(zip(ROW_LABELS, bands)):
        cs = centers[idx]
        bg = background(arr, y0, y1, int(cs[-1]) + 64)
        entry = dict(
            label=label,
            y=[int(y0), int(y1)],
            ruler_count=len(cs),
            step_px=round(ascii_step if idx == 1 else float(np.median(steps[idx])), 2),
            background=bg.tolist(),
            samples=[],
        )
        expected = EXPECTED_CELLS.get(label, [])
        for k in range(len(cs) - 1):
            span = cs[k + 1] - cs[k]
            m = sample_metrics(arr, mask, y0, y1, cs[k], cs[k + 1], bg)
            m.update(index=k, span_px=round(span, 2), cells=cells_of(span))
            if k < len(expected):
                m["expected_cells"] = expected[k]
            entry["samples"].append(m)
        rows.append(entry)

    # 占位格数：每一行每个样例都要与 Unicode 宽度表对上。
    mismatches = []
    for row in rows:
        for s in row["samples"]:
            if "expected_cells" in s and s["cells"] != s["expected_cells"]:
                mismatches.append(
                    f"{row['label']}#{s['index']}: {s['cells']}≠{s['expected_cells']}"
                )
    check("每个样例的占位格数 = Unicode 宽度期望", not mismatches, "; ".join(mismatches) or "全部一致")

    ascii_row = next(r for r in rows if r["label"] == "ascii")
    no_ink = [s["index"] for s in ascii_row["samples"] if s["ink_px"] == 0]
    check("ASCII 样例真的画出了墨迹", not no_ink, f"无墨迹样例：{no_ink}" if no_ink else "8/8 有墨迹")

    blank_row = next(r for r in rows if r["label"] == BLANK_SAMPLE[0])
    blank = blank_row["samples"][BLANK_SAMPLE[1]]
    check(
        "全角空格无墨迹但仍占两格",
        blank["ink_px"] == 0 and blank["cells"] == 2,
        f"ink={blank['ink_px']} cells={blank['cells']}",
    )

    cap = capabilities(rows)
    return finalize(path, arr, checks, rows, ascii_step, cap)


def capabilities(rows):
    def get(label):
        return next(r for r in rows if r["label"] == label)["samples"]

    cjk, emoji, nerd, mixed = get("cjk"), get("emoji"), get("nerd"), get("mixed")
    return {
        # 结构：CJK 双宽是否真的实现。
        "cjk_double_width": all(s["cells"] == 2 and s["ink_px"] > 0 for s in cjk),
        # 能力：彩色 emoji。单色字体下全部 colorful_ratio 为 0。
        "color_emoji": any(s["colorful_ratio"] > COLOR_THRESHOLD for s in emoji),
        "emoji_max_colorful_ratio": max(s["colorful_ratio"] for s in emoji),
        # 能力：组合字符叠加（当前 e 与锐音符各占一格）。
        "combining_marks": mixed[0]["cells"] == 1,
        # 能力：grapheme cluster（当前 ZWJ 家庭被拆成 8 格）。
        "grapheme_clusters": mixed[4]["cells"] <= 2,
        "zwj_sequence_cells": mixed[4]["cells"],
        # 能力：私用区码位是否都有字形（空心方框 = 缺字）。
        "nerd_font_complete": all(s["ink_px"] > 0 and s["hole_ratio"] < 0.9 for s in nerd),
        "nerd_tofu_count": sum(1 for s in nerd if s["hole_ratio"] >= 0.9),
    }


def finalize(path, arr, checks, rows, ascii_step, cap):
    return dict(
        image=str(path),
        sha256=hashlib.sha256(Path(path).read_bytes()).hexdigest(),
        size=[int(arr.shape[1]), int(arr.shape[0])],
        ruler_rgb=RULER_RGB.tolist(),
        ruler_tol=RULER_TOL,
        ascii_step_px=ascii_step,
        ruler_step_px=rows[0]["step_px"] if rows else None,
        rows=rows,
        capabilities=cap or {},
        checks=checks,
        passed=all(c["passed"] for c in checks),
    )


def print_report(r):
    print(f"字形样张：{r['image']}  {r['size'][0]}×{r['size'][1]}  sha={r['sha256'][:12]}")
    print(f"标尺色 {tuple(r['ruler_rgb'])} ±{r['ruler_tol']}；ASCII 基准 {r['ascii_step_px']:.2f}px = 2 格；"
          f"整串尺子 {r['ruler_step_px']}px（两者本就不必成整数倍，见文件头）")
    print()
    for row in r["rows"]:
        # 两把尺子只用来标定等宽，它们之间没有「样例」，列出 47 个 0 只是噪音。
        if row["label"] == "ruler" or not row["samples"]:
            continue
        cells = ",".join(str(s["cells"]) for s in row["samples"])
        inks = ",".join(str(s["ink_px"]) for s in row["samples"])
        print(f"  {row['label']:<6} 格数 [{cells}]  墨迹 [{inks}]")
    print()
    print("能力清单（否定值是已知缺口，不判红）：")
    for k, v in r["capabilities"].items():
        print(f"  {k:<28} {v}")
    print()
    for c in r["checks"]:
        print(f"  {'✓' if c['passed'] else '✗'} {c['name']}：{c['detail']}")
    print()
    print(f"结构判据：{sum(c['passed'] for c in r['checks'])}/{len(r['checks'])} 通过")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("png", help="`--demo glyph --screenshot` 产出的 PNG")
    ap.add_argument("--json", dest="json_out", help="把完整数值写进 JSON")
    ap.add_argument("--crop", help="导出放大的人眼样张（供报告取证）")
    args = ap.parse_args()

    png = Path(args.png)
    if not png.is_file():
        sys.exit(f"找不到样张：{png}")
    r = analyze(png)

    if args.crop:
        im = Image.open(png).convert("RGB")
        bands = find_bands(ruler_mask(np.asarray(im)))
        if bands:
            y0, y1 = bands[0][0] - 6, bands[-1][1] + 6
            centers = vertical_centers(ruler_mask(np.asarray(im)), *bands[0])
            # 取所有行带里最右的一根标尺，右边再留一点空白。
            right = max(
                vertical_centers(ruler_mask(np.asarray(im)), a, b)[-1]
                for a, b in bands
            )
            crop = im.crop((max(0, int(centers[0]) - 10), max(0, y0),
                            min(im.width, int(right) + 20), min(im.height, y1)))
            crop.resize((crop.width * 2, crop.height * 2), Image.Resampling.NEAREST).save(args.crop)
            print(f"人眼样张：{args.crop}")

    print_report(r)
    if args.json_out:
        Path(args.json_out).write_text(json.dumps(r, ensure_ascii=False, indent=2) + "\n")
        print(f"数值报告：{args.json_out}")
    return 0 if r["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
