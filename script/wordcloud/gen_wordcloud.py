#!/usr/bin/env python3
"""用户词云生成器：取词 -> 加权 -> 映射 -> 布局。

数据来自 Rime 用户词典（`export_user_words` 导出的 TSV），不需要分词 ——
userdb 里存的就是词条，这正是输入法做词云最省事的地方。

四个环节里只有布局是真正难的：阿基米德螺线试位 + 积分图 O(1) 碰撞查询。
这里的 mask 是**像素级**的（不是降采样网格），积分图由 numpy 全量重建，
所以“一个词框里有没有压到已放的词”是一次四则运算，而不是逐像素扫描。

用法：
  python script/wordcloud/gen_wordcloud.py \
      --tsv .tmp_wc/words.tsv --out out/wordcloud

产物：
  wordcloud.png   位图（PIL + FreeType 抗锯齿，字体显式指定微软雅黑）
  wordcloud.svg   矢量（text 元素，可无限放大、可用 Office 编辑）
  wordcloud.html  交互版（悬停看词频、可下载 PNG）
"""

from __future__ import annotations

import argparse
import collections
import html
import math
import re
import sys
import time
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

# ---------------------------------------------------------------- 字体

FONT_REGULAR = ("C:/Windows/Fonts/msyh.ttc", 0)  # 微软雅黑 Regular
FONT_BOLD = ("C:/Windows/Fonts/msyhbd.ttc", 0)  # 微软雅黑 Bold
FONT_CSS = "'Microsoft YaHei', 'PingFang SC', 'Noto Sans CJK SC', sans-serif"

# 5 档配色：越靠前词越大越深。按**排名**分档，不按绝对值 ——
# 长尾分布下绝对值分档会让 90% 的词挤在同一档。
TIERS = [
    (0.04, "#0B1F30"),
    (0.12, "#17455F"),
    (0.30, "#256A78"),
    (0.58, "#3F7D72"),
    (1.01, "#6E8B84"),
]
TIERS_DARK = [
    (0.04, "#EAF2F6"),
    (0.12, "#9FD3E0"),
    (0.30, "#6FBFB0"),
    (0.58, "#7FB28C"),
    (1.01, "#8C9E96"),
]

BG_LIGHT = "#FBFBF8"
BG_DARK = "#14181C"

# ---------------------------------------------------------------- 停用词

# 中文里真正的高频虚词。词云里留着它们只会把有信息量的词挤到角落 ——
# 实测这份用户词典里「的」216 次，不剔就是画面正中间一个巨大的「的」。
STOPWORDS = set(
    """
的 了 是 和 在 有 不 也 就 都 而 及 与 或 但 却 很 太 更 最 才 又 再 还 只 被 把 给 让 从 到 对 向 于 以 为 之 其 此
我 你 他 她 它 俺 咱 您 我们 你们 他们 她们 它们 咱们 自己 大家 别人
这 那 哪 这个 那个 哪个 这些 那些 哪些 这样 那样 怎样 这里 那里 哪里 这么 那么 怎么
一 二 三 一个 一些 一下 一点 一次 一直 一起 一样 一定 一般 其实 东西 事情 时候 地方 方面 问题
什么 为什么 怎么办 干嘛 如何 是否 是不是 有没有 能不能 可不可以
可以 可能 应该 需要 要有 还要 就是 不是 也是 还是 但是 因为 所以 如果 虽然 不过 只是 而且 然后 于是 因此 否则
已经 现在 刚才 马上 立刻 总是 经常 有时 有时候 大概 也许 差不多 基本上
没有 没用 不要 不用 不能 不会 别 吗 呢 吧 啊 呀 哦 嗯 哈 啦 嘛 呗 罢了
说 做 用 要 会 能 想 看 让 好 多 少 大 小 上 下 里 外 前 后 中
""".split()
)


# ---------------------------------------------------------------- 数据


@dataclass
class Word:
    text: str
    weight: float
    count: int
    dee: float
    size: int = 0
    color: str = ""
    # 布局结果：x 是中心、y 是**基线**（与 PIL 的 anchor="ms" 和 SVG 的默认基线一致）
    x: float = 0.0
    y: float = 0.0
    gx0: int = 0
    gy0: int = 0
    w: int = 0
    h: int = 0
    mask: object = None
    placed: bool = False


def load_ngram(path: Path, min_len: int, max_len: int, stop: set[str],
               keep: set[str]) -> tuple[list[Word], dict[str, int], dict]:
    """直接读 `user_ngram.tsv`（词 + 频次，已经是词条，不需要分词）。

    这是最省事的路径：`u` 行是一元词，`b`/`t` 行是二元/三元组，词云只取 `u`。
    """
    han = re.compile(r"^[\u3400-\u9fff]+$")
    latin = re.compile(r"^[A-Za-z][A-Za-z0-9+.#_-]*$")
    drop = collections.Counter()
    kept: list[Word] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        f = line.split("\t")
        if len(f) < 3 or f[0] != "u":
            continue
        word, cnt = f[1].strip(), int(float(f[2] or 0))
        if not word:
            continue
        if word in keep:
            kept.append(Word(word, max(cnt, 1), cnt, 0.0))
            continue
        if len(word) < min_len:
            drop["太短"] += 1
        elif len(word) > max_len:
            drop["太长"] += 1
        elif not (han.match(word) or latin.match(word)):
            drop["字符合法性"] += 1
        elif word in stop:
            drop["停用词"] += 1
        elif cnt <= 0:
            drop["零频次"] += 1
        else:
            kept.append(Word(word, float(cnt), cnt, 0.0))
    return kept, dict(drop), {}


def load_tsv(path: Path, schema: str, min_len: int, max_len: int, stop: set[str],
             keep: set[str]) -> tuple[list[Word], dict[str, int], dict]:
    """取词 + 过滤。返回 (候选词, 各过滤环节剔除数量, 按方案的词频表)。"""
    raw = path.read_text(encoding="utf-8").splitlines()
    header = raw[0].split("\t")
    ix = {k: i for i, k in enumerate(header)}
    need = {"schema", "text", "simp", "commits", "dee"}
    if not need <= set(ix):
        sys.exit(f"表头缺列：{need - set(ix)}，实际 {header}")

    agg: dict[str, list[float]] = {}
    docs: dict[str, collections.Counter] = collections.defaultdict(
        collections.Counter
    )
    for line in raw[1:]:
        f = line.split("\t")
        if len(f) < len(header):
            continue
        sch = f[ix["schema"]]
        if schema != "all" and sch != schema:
            continue
        word = f[ix["simp"]].strip()
        if not word:
            continue
        commits = int(float(f[ix["commits"]] or 0))
        dee = float(f[ix["dee"]] or 0.0)
        slot = agg.setdefault(word, [0.0, 0.0])
        slot[0] += commits
        slot[1] += dee
        docs[sch][word] += commits

    han = re.compile(r"^[\u3400-\u9fff]+$")
    latin = re.compile(r"^[A-Za-z][A-Za-z0-9+.#_-]*$")
    drop = collections.Counter()
    kept: list[Word] = []
    for word, (commits, dee) in agg.items():
        if word in keep:
            kept.append(Word(word, max(commits, 1.0), int(commits), dee))
            continue
        if len(word) < min_len:
            drop["太短"] += 1
            continue
        if len(word) > max_len:
            drop["太长"] += 1
            continue
        if not (han.match(word) or latin.match(word)):
            # 混入标点/外文符号的学词噪音（如 `吗z`、`㕢ㄕ`）
            drop["字符合法性"] += 1
            continue
        if word in stop:
            drop["停用词"] += 1
            continue
        if commits <= 0:
            drop["零频次"] += 1
            continue
        kept.append(Word(word, float(commits), int(commits), dee))
    return kept, dict(drop), docs


def reweight(words: list[Word], mode: str, docs: dict[str, collections.Counter]):
    """加权：频次 -> 权重。"""
    if mode == "commits":
        for w in words:
            w.weight = max(w.count, 1)
    elif mode == "dee":
        # librime 的 `d` 值是 -log(p) 的近亲，越小越常用
        for w in words:
            w.weight = 1.0 / max(w.dee, 0.05)
    elif mode == "mix":
        for w in words:
            w.weight = max(w.count, 1) * (1.0 / max(w.dee, 0.05)) ** 0.5
    elif mode == "tfidf":
        # 单份语料算不了 IDF，这里用「按方案切文档」的对比变体：
        # 只有某个方案里出现的词，才说明它是**这个方案的特征词**。
        n_docs = max(len(docs), 1)
        for w in words:
            df = sum(1 for c in docs.values() if w.text in c)
            w.weight = max(w.count, 1) * math.log((n_docs + 1) / (df + 0.5))
    return words


def map_sizes(words: list[Word], smin: int, smax: int, scale: str):
    """映射：权重 -> 字号。长尾分布必须用 log/sqrt，线性会让首词吃满画面。"""
    lo = min(w.weight for w in words)
    hi = max(w.weight for w in words)
    if hi <= lo:
        for w in words:
            w.size = (smin + smax) // 2
        return words

    def f(v: float) -> float:
        if scale == "linear":
            return (v - lo) / (hi - lo)
        if scale == "sqrt":
            return (math.sqrt(v) - math.sqrt(lo)) / (math.sqrt(hi) - math.sqrt(lo))
        return (math.log(v) - math.log(lo)) / (math.log(hi) - math.log(lo))

    for w in words:
        w.size = int(round(smin + (smax - smin) * f(w.weight)))
    return words


def assign_colors(words: list[Word], dark: bool = False):
    palette = TIERS_DARK if dark else TIERS
    n = len(words)
    for i, w in enumerate(words):
        frac = i / max(n - 1, 1)
        for edge, color in palette:
            if frac < edge:
                w.color = color
                break
        else:
            w.color = TIERS[-1][1]
    return words


# ---------------------------------------------------------------- 布局


class Canvas:
    """占位图 + 积分图（O(1) 相交查询）。

    `mask` 记录每个格子是否已被词占用，`allow` 是形状约束（词云轮廓）。
    两者的积分图都**摊平成 Python list**：纯整数四则运算一次就出结果，
    比在 numpy 数组上取 4 个标量快一个数量级（这个词每个词要查几千次）。

    格子取 4px：字形是矢量、留白本来就 6px，再细的精度只是浪费算力。
    """

    CELL = 4

    def __init__(self, width: int, height: int, shape: str = "ellipse", pad: int = 6):
        self.w = width
        self.h = height
        self.pad = pad
        self.gw = width // self.CELL
        self.gh = height // self.CELL
        self.mask = np.zeros((self.gh, self.gw), dtype=np.int32)
        self.allow = self._shape_mask(shape)
        self.allow_flat = self._integral_flat(self.allow)
        self.mask_flat = self._integral_flat(self.mask)

    def _shape_mask(self, shape: str) -> np.ndarray:
        m = np.zeros((self.gh, self.gw), dtype=np.int32)
        yy, xx = np.mgrid[0 : self.gh, 0 : self.gw]
        cx, cy = self.gw / 2, self.gh / 2
        u = (xx - cx) / (cx * 0.98)
        v = (yy - cy) / (cy * 0.96)
        if shape == "rect":
            m[:] = 1
        elif shape == "circle":
            m[(u * u + v * v) <= 1.0] = 1
        elif shape == "heart":
            # 心形隐函数：(x²+y²-1)³ - x²y³ <= 0，y 轴翻转
            hx, hy = u * 1.15, -v * 1.15 + 0.12
            m[(hx * hx + hy * hy - 1) ** 3 - hx * hx * hy**3 <= 0] = 1
        else:  # ellipse
            m[(u * u + v * v) <= 1.0] = 1
        return m

    @staticmethod
    def _integral_flat(a: np.ndarray) -> list[int]:
        out = np.zeros((a.shape[0] + 1, a.shape[1] + 1), dtype=np.int32)
        np.cumsum(np.cumsum(a, axis=0), axis=1, out=out[1:, 1:])
        return out.ravel().tolist()

    def free(self, x0: int, y0: int, x1: int, y1: int) -> bool:
        c = self.CELL
        gx0, gy0 = x0 // c, y0 // c
        gx1 = -(-x1 // c)
        gy1 = -(-y1 // c)
        if gx0 < 1 or gy0 < 1 or gx1 >= self.gw - 1 or gy1 >= self.gh - 1:
            return False
        w = self.gw + 1
        area = (gx1 - gx0) * (gy1 - gy0)
        a = self.allow_flat
        if a[gy1 * w + gx1] - a[gy0 * w + gx1] - a[gy1 * w + gx0] + a[gy0 * w + gx0] < area:
            return False  # 越出形状轮廓
        m = self.mask_flat
        return m[gy1 * w + gx1] - m[gy0 * w + gx1] - m[gy1 * w + gx0] + m[gy0 * w + gx0] == 0

    def occupy(self, glyph: np.ndarray, x0: int, y0: int):
        c = self.CELL
        h, w = glyph.shape
        gx0, gy0 = x0 // c, y0 // c
        gx1 = (x0 + w + c - 1) // c
        gy1 = (y0 + h + c - 1) // c
        blk = np.zeros(((gy1 - gy0) * c, (gx1 - gx0) * c), dtype=np.int32)
        oy, ox = y0 - gy0 * c, x0 - gx0 * c
        blk[oy : oy + h, ox : ox + w] = glyph
        red = blk.reshape(gy1 - gy0, c, gx1 - gx0, c).max(axis=(1, 3))
        region = self.mask[gy0:gy1, gx0:gx1]
        np.maximum(region, red, out=region)
        self.mask_flat = self._integral_flat(self.mask)


FONT_CACHE: dict[tuple[str, int], ImageFont.FreeTypeFont] = {}


def get_font(size: int) -> ImageFont.FreeTypeFont:
    path, idx = FONT_BOLD if size >= 44 else FONT_REGULAR
    key = (path, size)
    if key not in FONT_CACHE:
        FONT_CACHE[key] = ImageFont.truetype(path, size, index=idx)
    return FONT_CACHE[key]


def layout(words: list[Word], canvas: Canvas, max_tries: int = 30000) -> tuple[int, float]:
    """阿基米德螺线试位。r 随 θ 线性增长，步长按弧长自适应（间距恒定 ~3px）。

    关键：**栅格化和最终渲染走同一份 glyph 位图**（`word.mask`），
    否则布局时认为"没压上"、渲染时却叠在一起 —— 这是自写词云最常见的坑。
    """
    cx, cy = canvas.w / 2, canvas.h / 2
    aspect = 0.62  # 画布宽高比压扁，让词云填成横幅而不是正圆
    pad = 5
    placed = 0
    t0 = time.time()

    for word in words:
        font = get_font(word.size)
        bx0, by0, bx1, by1 = font.getbbox(word.text, anchor="ms")
        gw, gh = bx1 - bx0 + 2 * pad, by1 - by0 + 2 * pad
        glyph = Image.new("L", (gw, gh), 0)
        # anchor="ms" 的坐标是**文字中心**：横向居中，纵向是基线
        ImageDraw.Draw(glyph).text(
            (gw / 2, pad - by0), word.text, font=font, anchor="ms", fill=255
        )
        arr = (np.asarray(glyph) > 0).astype(np.int32)
        hh, ww = arr.shape

        theta = 0.0
        r = 0.0
        for _ in range(max_tries):
            x = cx + r * math.cos(theta)
            y = cy + r * math.sin(theta) * aspect
            x0 = int(round(x - gw / 2))
            y0 = int(round(y + by0 - pad))
            if canvas.free(x0, y0, x0 + ww, y0 + hh):
                canvas.occupy(arr, x0, y0)
                word.x = x0 + gw / 2
                word.y = y0 + pad - by0
                word.gx0, word.gy0 = x0, y0
                word.w, word.h = ww, hh
                word.mask = glyph
                word.placed = True
                placed += 1
                break
            theta += max(0.035, 4.0 / max(r, 4.0))
            r = 2.1 * theta
        else:
            word.placed = False

    return placed, time.time() - t0


# ---------------------------------------------------------------- 渲染


def render_png(words: list[Word], canvas: Canvas, out: Path, bg: str):
    img = Image.new("RGB", (canvas.w, canvas.h), bg)
    for w in words:
        if not w.placed:
            continue
        img.paste(w.color, (w.gx0, w.gy0), w.mask)
    img.save(out, optimize=True)


def render_svg(words: list[Word], canvas: Canvas, out: Path, bg: str):
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{canvas.w}" height="{canvas.h}" '
        f'viewBox="0 0 {canvas.w} {canvas.h}">',
        f'<rect width="100%" height="100%" fill="{bg}"/>',
        f'<g font-family="{FONT_CSS}" text-anchor="middle">',
    ]
    for w in words:
        if not w.placed:
            continue
        parts.append(
            f'<text x="{w.x:.1f}" y="{w.y:.1f}" font-size="{w.size}" fill="{w.color}">'
            f"{html.escape(w.text)}</text>"
        )
    parts.append("</g></svg>")
    out.write_text("\n".join(parts), encoding="utf-8")


def render_html(words: list[Word], canvas: Canvas, out: Path, bg: str, meta: str):
    items = []
    for w in words:
        if not w.placed:
            continue
        items.append(
            f'<text x="{w.x:.1f}" y="{w.y:.1f}" font-size="{w.size}" fill="{w.color}" '
            f'data-n="{w.count}" data-w="{w.weight:.1f}">{html.escape(w.text)}</text>'
        )
    out.write_text(
        f"""<!DOCTYPE html>
<meta charset="utf-8">
<title>用户词云</title>
<style>
  html,body{{margin:0;background:{bg};font-family:{FONT_CSS}}}
  #wrap{{position:relative;max-width:{canvas.w}px;margin:24px auto}}
  svg{{width:100%;height:auto;text-anchor:middle}}
  text{{cursor:default;transition:opacity .12s}}
  text:hover{{opacity:.55}}
  #tip{{position:fixed;padding:6px 10px;border-radius:6px;background:#0E2233;color:#fff;
        font-size:13px;pointer-events:none;opacity:0;transition:opacity .1s}}
  #meta{{color:#8a8a80;font-size:12px;text-align:center;padding:8px 0 24px}}
</style>
<div id="wrap">
<svg viewBox="0 0 {canvas.w} {canvas.h}" xmlns="http://www.w3.org/2000/svg">
<rect width="100%" height="100%" fill="{bg}"/>
<g>{''.join(items)}</g>
</svg>
</div>
<div id="meta">{html.escape(meta)}</div>
<div id="tip"></div>
<script>
const tip = document.getElementById('tip');
for (const t of document.querySelectorAll('text')) {{
  t.addEventListener('mousemove', e => {{
    tip.textContent = t.textContent + ' · 上屏 ' + t.dataset.n + ' 次';
    tip.style.left = (e.clientX + 14) + 'px';
    tip.style.top = (e.clientY + 14) + 'px';
    tip.style.opacity = 1;
  }});
  t.addEventListener('mouseleave', () => tip.style.opacity = 0);
}}
</script>
""",
        encoding="utf-8",
    )


# ---------------------------------------------------------------- main


def main() -> int:
    ap = argparse.ArgumentParser(description="Rime 用户词典词云生成器")
    ap.add_argument("--tsv", type=Path, help="export_user_words 导出的 TSV")
    ap.add_argument("--ngram", type=Path, help="直接读 user_ngram.tsv（u 行）")
    ap.add_argument("--out", default=Path("out/wordcloud"), type=Path)
    ap.add_argument("--schema", default="all", help="只取某个方案的学词")
    ap.add_argument("--max-words", type=int, default=190)
    ap.add_argument("--min-len", type=int, default=2)
    ap.add_argument("--max-len", type=int, default=8)
    ap.add_argument("--size-min", type=int, default=16)
    ap.add_argument("--size-max", type=int, default=112)
    ap.add_argument("--scale", choices=["log", "sqrt", "linear"], default="log")
    ap.add_argument(
        "--weight", choices=["commits", "dee", "mix", "tfidf"], default="commits"
    )
    ap.add_argument("--width", type=int, default=1400)
    ap.add_argument("--height", type=int, default=860)
    ap.add_argument("--shape", default="ellipse")
    ap.add_argument("--bg", choices=["light", "dark"], default="light")
    ap.add_argument("--no-stopwords", action="store_true")
    ap.add_argument("--keep", default="", help="强制保留的词，逗号分隔")
    args = ap.parse_args()

    if not args.tsv and not args.ngram:
        sys.exit("要么给 --tsv（推荐，含 commit_count），要么给 --ngram")
    t0 = time.time()
    stop = set() if args.no_stopwords else STOPWORDS
    keep = {s.strip() for s in args.keep.split(",") if s.strip()}
    if args.ngram:
        words, drop, docs = load_ngram(
            args.ngram, args.min_len, args.max_len, stop, keep
        )
    else:
        words, drop, docs = load_tsv(
            args.tsv, args.schema, args.min_len, args.max_len, stop, keep
        )
    total_candidates = len(words)
    words = reweight(words, args.weight, docs)
    words.sort(key=lambda w: -w.weight)
    words = words[: args.max_words]
    map_sizes(words, args.size_min, args.size_max, args.scale)
    assign_colors(words, dark=args.bg == "dark")

    bg = BG_LIGHT if args.bg == "light" else BG_DARK
    canvas = Canvas(args.width, args.height, args.shape)
    placed, layout_s = layout(words, canvas)

    args.out.mkdir(parents=True, exist_ok=True)
    render_png(words, canvas, args.out / "wordcloud.png", bg)
    render_svg(words, canvas, args.out / "wordcloud.svg", bg)
    meta = (
        f"{placed}/{len(words)} 词 · 方案 {args.schema} · 权重 {args.weight} · "
        f"字号 {args.scale}"
    )
    render_html(words, canvas, args.out / "wordcloud.html", bg, meta)

    print(f"候选词 {total_candidates}，剔除：{drop}")
    print(f"取前 {len(words)} 词，放置 {placed} 个（{layout_s*1000:.0f} ms 布局）")
    print("Top 12：" + "  ".join(f"{w.text}({w.count})" for w in words[:12]))
    print(f"总耗时 {(time.time()-t0)*1000:.0f} ms → {args.out}/")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
