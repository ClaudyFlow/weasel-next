"""量候选窗里"文字墨迹"的行高与行距（把两张图的 DPI 差异折算后再比）。

用法: python ink-bands.py <png> <x> <y> <w> <h> [--scale 1.0] [--threshold 110] [--inset 8]

`--scale` 是"这张图相对实际像素的比例"：官方那张截图整体被缩到 0.8，
传 0.8 就能把测得的高度换算回实际像素，从而和我们 1:1 的截图直接比。
"""

import sys

import numpy as np
from PIL import Image


def main():
    path = sys.argv[1]
    x, y, w, h = (int(v) for v in sys.argv[2:6])
    scale = 1.0
    threshold = 110
    inset = 8
    args = sys.argv[6:]
    for index, arg in enumerate(args):
        if arg == "--scale":
            scale = float(args[index + 1])
        elif arg == "--threshold":
            threshold = int(args[index + 1])
        elif arg == "--inset":
            inset = int(args[index + 1])

    image = Image.open(path).convert("RGB")
    crop = image.crop((x, y, x + w, y + h))
    array = np.asarray(crop).astype(int)
    dark = (
        (array[:, :, 0] < threshold)
        & (array[:, :, 1] < threshold)
        & (array[:, :, 2] < threshold)
    )
    rows = dark[:, inset : max(inset + 1, w - inset)].any(axis=1)
    bands = []
    current = None
    for index, filled in enumerate(rows):
        if filled and current is None:
            current = index
        elif not filled and current is not None:
            bands.append((current, index - 1))
            current = None
    if current is not None:
        bands.append((current, len(rows) - 1))

    print(f"{path} 裁剪 {w}x{h} @({x},{y}) 缩放系数 {scale}")
    previous = None
    for start, end in bands:
        actual_top = round(start / scale, 1)
        actual_height = round((end - start + 1) / scale, 1)
        pitch = "" if previous is None else f"  行距 {round((start - previous) / scale, 1)} px(实际)"
        print(f"  图内 y {start}..{end} 高 {end - start + 1} → 实际 顶 {actual_top} 高 {actual_height}{pitch}")
        previous = start


if __name__ == "__main__":
    main()
