"""把官方候选窗与我们的候选窗裁出来并排对比。

官方那张是用户截的图（整体约 0.8 缩放），所以把我们的截图缩到同一比例再拼，
行高、内缩、圆角、描边就能一眼对上。

用法: python compare-candidates.py <官方图> <官方面板 x y w h> <我们的图> <我们面板 x y w h> <输出>
"""

import sys

from PIL import Image, ImageDraw


def main():
    official_path, ox, oy, ow, oh, ours_path, px, py, pw, ph, out = sys.argv[1:12]
    ox, oy, ow, oh = int(ox), int(oy), int(ow), int(oh)
    px, py, pw, ph = int(px), int(py), int(pw), int(ph)

    official = Image.open(official_path).convert("RGB").crop((ox, oy, ox + ow, oy + oh))
    ours = Image.open(ours_path).convert("RGB").crop((px, py, px + pw, py + ph))

    # 官方图整体约 0.8 缩放（DSH 界面截图），把我们的也缩到同一比例再比
    scale = 0.8
    ours_scaled = ours.resize(
        (max(1, int(ours.width * scale)), max(1, int(ours.height * scale))),
        Image.LANCZOS,
    )

    gap = 16
    label = 24
    width = official.width + ours_scaled.width + gap * 3
    height = max(official.height, ours_scaled.height) + label + gap * 2
    canvas = Image.new("RGB", (width, height), (0x15, 0x15, 0x17))
    draw = ImageDraw.Draw(canvas)
    left_x = gap
    right_x = gap * 2 + official.width
    canvas.paste(official, (left_x, label + gap))
    canvas.paste(ours_scaled, (right_x, label + gap))
    draw.text((left_x, 6), f"OFFICIAL {official.width}x{official.height}", fill=(0xEC, 0xEE, 0xEE))
    draw.text(
        (right_x, 6),
        f"OURS {ours.width}x{ours.height} (x{scale} -> {ours_scaled.width}x{ours_scaled.height})",
        fill=(0xEC, 0xEE, 0xEE),
    )
    canvas.save(out)
    print(f"已生成 {out}: 官方 {official.size} · 我们 {ours.size} → {ours_scaled.size}")


if __name__ == "__main__":
    main()
