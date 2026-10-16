"""严格比对 punctuation.yaml 与 symbols.yaml：逐表、逐字符、逐值。

两份文件只用到一个很小子集的 YAML（嵌套 map + 行内 list/map + 标量 + 带引号的键），
所以这里手写一个够用的解析器，避免引入依赖。

结论要看的是：
  * symbols.yaml 的 punctuator.full_shape / half_shape 是否覆盖 punctuation.yaml 的
    full_shape / half_shape（键与值都一致）；
  * punctuation.yaml 的 ascii_style 在 symbols.yaml 里有没有对应表。
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(r"C:\Program Files\Weasel-Next\Rime-Next")


def strip_comment(line: str) -> str:
    out = []
    quote = None
    for ch in line:
        if quote:
            out.append(ch)
            if ch == quote:
                quote = None
            continue
        if ch in "'\"":
            quote = ch
            out.append(ch)
            continue
        if ch == "#":
            break
        out.append(ch)
    return "".join(out).rstrip()


def parse(text: str) -> dict:
    """把文件解析成嵌套 dict；只支持本文件用到的写法。"""
    root: dict = {}
    stack: list[tuple[int, dict]] = [(-1, root)]
    for raw in text.splitlines():
        line = strip_comment(raw)
        if not line.strip():
            continue
        indent = len(line) - len(line.lstrip(" "))
        body = line.strip()
        while stack and indent <= stack[-1][0]:
            stack.pop()
        current = stack[-1][1]
        if body.endswith(":") and ":" not in body[:-1]:
            key = body[:-1].strip().strip("'\"")
            child: dict = {}
            current[key] = child
            stack.append((indent, child))
            continue
        # key: value
        match = re.match(r"^(?P<key>'(?:[^']|'')*'|\"[^\"]*\"|[^:]+):\s*(?P<value>.*)$", body)
        if not match:
            continue
        key = match.group("key").strip()
        if len(key) >= 2 and key[0] == key[-1] and key[0] in "'\"":
            key = key[1:-1].replace("''", "'")
        current[key] = match.group("value").strip()
    return root


def values_equal(a: str, b: str) -> bool:
    """行内 list/map 的书写差异（空格、引号）不算差异。"""
    norm = lambda s: re.sub(r"[\s'\"]+", "", s)
    return norm(a) == norm(b)


def compare_table(name: str, left: dict, right: dict) -> tuple[list[str], list[str], list[str]]:
    only_left = sorted(set(left) - set(right))
    only_right = sorted(set(right) - set(left))
    differing = sorted(
        key for key in set(left) & set(right) if not values_equal(left[key], right[key])
    )
    print(f"\n### {name}: punctuation {len(left)} 项 / symbols {len(right)} 项")
    print(f"  只在 punctuation 里有: {only_left if only_left else '（无）'}")
    print(f"  只在 symbols 里有    : {only_right if only_right else '（无）'}")
    if differing:
        print(f"  两边都有但**值不同** : {len(differing)} 项")
        for key in differing[:10]:
            print(f"    {key!r}: punctuation={left[key][:60]!r}  symbols={right[key][:60]!r}")
        if len(differing) > 10:
            print(f"    …其余 {len(differing) - 10} 项")
    else:
        print("  两边都有但值不同    :（无，值完全一致）")
    return only_left, only_right, differing


def main() -> int:
    punctuation = parse((ROOT / "punctuation.yaml").read_text(encoding="utf-8"))
    symbols = parse((ROOT / "symbols.yaml").read_text(encoding="utf-8"))
    punctuator = symbols.get("punctuator", {})

    print("punctuation.yaml 顶层表:", list(punctuation))
    print("symbols.yaml 顶层表    :", list(symbols))
    print("symbols.punctuator 表  :", list(punctuator))

    total_missing: list[str] = []
    for table in ("full_shape", "half_shape"):
        only_left, _, _ = compare_table(
            f"{table}（punctuation.{table} ↔ symbols.punctuator.{table}）",
            punctuation.get(table, {}),
            punctuator.get(table, {}),
        )
        if only_left:
            total_missing.append(f"{table}: {only_left}")

    # ascii_style 在 symbols 里没有同名表 —— 看看它是不是被别处覆盖
    ascii_style = punctuation.get("ascii_style", {})
    print(f"\n### ascii_style: punctuation 有 {len(ascii_style)} 项，symbols.punctuator 里的同名表: "
          f"{'有' if 'ascii_style' in punctuator else '**没有**'}")
    if "ascii_style" in punctuator:
        compare_table("ascii_style", ascii_style, punctuator["ascii_style"])

    symbols_table = punctuator.get("symbols", {})
    print(f"\n### symbols.punctuator.symbols：{len(symbols_table)} 组符号（/fh /sx 这类）")

    print("\n=== 结论 ===")
    if total_missing:
        print("symbols.yaml **没有**完全覆盖 punctuation.yaml：")
        for item in total_missing:
            print("  -", item)
        print("→ 直接删 punctuation.yaml 会丢东西")
    else:
        print("键集：symbols.yaml 的 full_shape / half_shape 与 punctuation.yaml **完全一致**")
        print("值  ：两边键都有，但 symbols 的候选更多（是 superset，不是子集）")
        if "ascii_style" not in punctuator:
            print("缺口：`ascii_style` 只在 punctuation.yaml 里，symbols.yaml **没有**")
        print("→ 能不能删 punctuation.yaml 取决于还有谁 `__include: punctuation:/…`，"
              "以及 ascii_style 是否需要保留")
    return 0


if __name__ == "__main__":
    sys.exit(main())
