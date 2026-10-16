"""把候选窗截图的几何量出来，用来和官方逐像素对照。

用法: python measure-candidate.py <png>

做法：先按颜色取掩码，再用连通域找**最大的一块**（避免把别处的浅色文字误当成面板），
然后量出可以直接和官方对比的几项：面板尺寸、高亮块尺寸、四边内缩、行带与行距。

关键量（都换算成"面板内坐标"，与 DPI 无关地可比）：
  * 面板宽高、高亮块宽高
  * 高亮块左边到面板左边 = margin_x + border（官方 content 矩形再内缩）
  * 上下内缩 = margin_y + border
  * 每行文字暗像素的起止 y → 行高、行距
"""

import sys
from collections import deque

import numpy as np
from PIL import Image


def largest_component(mask):
    """四连通的最大连通域，返回 (left, top, right, bottom, size)。"""
    height, width = mask.shape
    seen = np.zeros_like(mask, dtype=bool)
    best = None
    for start_y in range(height):
        row = np.flatnonzero(mask[start_y] & ~seen[start_y])
        for start_x in row:
            if seen[start_y, start_x]:
                continue
            queue = deque([(start_y, start_x)])
            seen[start_y, start_x] = True
            left = right = start_x
            top = bottom = start_y
            size = 0
            while queue:
                y, x = queue.popleft()
                size += 1
                left = min(left, x)
                right = max(right, x)
                top = min(top, y)
                bottom = max(bottom, y)
                for ny, nx in ((y - 1, x), (y + 1, x), (y, x - 1), (y, x + 1)):
                    if 0 <= ny < height and 0 <= nx < width and mask[ny, nx] and not seen[ny, nx]:
                        seen[ny, nx] = True
                        queue.append((ny, nx))
            if best is None or size > best[4]:
                best = (left, top, right, bottom, size)
    return best


def panel_mask(array):
    """面板底色 #eceeee（含边框 #e0e0e0 与抗锯齿过渡）。"""
    r = array[:, :, 0].astype(int)
    g = array[:, :, 1].astype(int)
    b = array[:, :, 2].astype(int)
    return (
        (abs(r - 0xEC) <= 14) & (abs(g - 0xEE) <= 14) & (abs(b - 0xEE) <= 14)
    ) | ((abs(r - 0xE0) <= 12) & (abs(g - 0xE0) <= 12) & (abs(b - 0xE0) <= 12))


def highlight_mask(array):
    """aqua 高亮块：#fa3a0a 按 ABGR 读 → RGB(0x0a,0x3a,0xfa)，允许抗锯齿过渡。"""
    r = array[:, :, 0].astype(int)
    g = array[:, :, 1].astype(int)
    b = array[:, :, 2].astype(int)
    return (b > 170) & (r < 120) & (g < 140) & (b - r > 80)


def text_bands(array, box, threshold=28, inset=6):
    """面板内"与面板底色明显不同"的行带（= 有内容的行），用来推行高与行距。

    比"暗像素"可靠：高亮行上的白字、注释灰字都算内容。
    `inset` 先切掉圆角/描边那一圈，否则每一行都有边缘像素、整块连成一条带。
    """
    left, top, right, bottom = box
    left, top, right, bottom = left + inset, top + inset, right - inset, bottom - inset
    sub = array[top : bottom + 1, left : right + 1].astype(int)
    diff = np.abs(sub - np.array([0xEC, 0xEE, 0xEE])).max(axis=2)
    rows = (diff > threshold).any(axis=1)
    bands = []
    current = None
    for index, filled in enumerate(rows):
        if filled and current is None:
            current = index
        elif not filled and current is not None:
            bands.append((current + inset, index - 1 + inset))
            current = None
    if current is not None:
        bands.append((current + inset, len(rows) - 1 + inset))
    return bands


def light_bands(array, box, threshold=200):
    """面板内"有亮像素"的行带（高亮块上的白字），用于定位高亮行里的文字。"""
    left, top, right, bottom = box
    sub = array[top : bottom + 1, left : right + 1]
    light = (sub[:, :, 0] > threshold) & (sub[:, :, 1] > threshold) & (sub[:, :, 2] > threshold)
    bands = []
    current = None
    for index, has_light in enumerate(light.any(axis=1)):
        if has_light and current is None:
            current = index
        elif not has_light and current is not None:
            bands.append((current, index - 1))
            current = None
    if current is not None:
        bands.append((current, len(light) - 1))
    return bands


def main():
    path = sys.argv[1]
    image = Image.open(path).convert("RGB")
    array = np.asarray(image)
    print(f"图片: {path} {image.width}x{image.height}")

    panel = largest_component(panel_mask(array))
    if panel is None:
        print("面板: 没找到 #eceeee 区域")
        return
    px0, py0, px1, py1 = panel[:4]
    print(f"面板: {px1 - px0 + 1}x{py1 - py0 + 1} @ ({px0},{py0})  像素数 {panel[4]}")

    highlight = None
    mask = highlight_mask(array)
    # 只在面板内部找高亮块（截图别处也可能有蓝色）
    inside = np.zeros_like(mask)
    inside[py0 : py1 + 1, px0 : px1 + 1] = True
    highlight = largest_component(mask & inside)
    if highlight is None:
        print("高亮块: 面板内没找到蓝色块")
    else:
        hx0, hy0, hx1, hy1 = highlight[:4]
        print(
            f"高亮块: {hx1 - hx0 + 1}x{hy1 - hy0 + 1} @ ({hx0},{hy0})"
            f"  内缩 左{hx0 - px0} 右{px1 - hx1} 上{hy0 - py0} 下{py1 - hy1}"
        )
        print(
            f"  高亮块中心色: #{array[(hy0 + hy1) // 2, (hx0 + hx1) // 2][0]:02x}"
            f"{array[(hy0 + hy1) // 2, (hx0 + hx1) // 2][1]:02x}"
            f"{array[(hy0 + hy1) // 2, (hx0 + hx1) // 2][2]:02x}"
        )

    bands = text_bands(array, (px0, py0, px1, py1))
    print(f"内容行带 {len(bands)} 条（面板内坐标）:")
    previous = None
    for start, end in bands:
        extra = "" if previous is None else f"  行距 {start - previous}"
        print(f"  y {start}..{end}  高 {end - start + 1}{extra}")
        previous = start

    lights = light_bands(array, (px0, py0, px1, py1))
    if lights:
        print(f"高亮行内亮色（白字）行带 {len(lights)} 条: " + ", ".join(f"{a}..{b}" for a, b in lights[:4]))

    print(
        f"面板底色样本: #{array[py0 + 8, px0 + 8][0]:02x}{array[py0 + 8, px0 + 8][1]:02x}{array[py0 + 8, px0 + 8][2]:02x}"
    )
    print(
        f"面板外样本: #{array[max(py0 - 4, 0), max(px0 - 4, 0)][0]:02x}"
        f"{array[max(py0 - 4, 0), max(px0 - 4, 0)][1]:02x}"
        f"{array[max(py0 - 4, 0), max(px0 - 4, 0)][2]:02x}"
    )


if __name__ == "__main__":
    main()
