#!/usr/bin/env python3
"""dict.yaml 标准头规范化(Rime-Next 数据标准,2026-10-27)。

所有 `*.dict.yaml` 的头部统一为三行(与方案文件的 `# Rime Schema` 头平行):

    # Rime Dictionary
    # encode: utf-8
    # newline: lf

规则:
* 剥掉文件开头的整段注释(含空行),换上标准三行;`---` 及其后内容一字不动
  (正文里的 `#` 注释行不归头部管,保持原样);
* CRLF 一并转 LF——声明了 `newline: lf` 就不许文件里真的有 CRLF
  (与方案准入的"声明与实测一致"同一条规矩);
* 非 UTF-8 文件跳过并报出来(标准要求 utf-8,转码不在本工具职责内)。

用法:
    python script/normalize-dict-headers.py <目录> [...]
对每个目录递归处理 `*.dict.yaml`,打印改动清单;`--dry` 只看不改。
"""

import sys
from pathlib import Path

HEADER = "# Rime Dictionary\n# encode: utf-8\n# newline: lf\n"


def normalize(path: Path, dry: bool) -> str | None:
    try:
        raw = path.read_bytes()
    except OSError as e:
        return f"读失败: {e}"
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        return "跳过:非 UTF-8"

    had_crlf = "\r\n" in text
    text = text.replace("\r\n", "\n")

    lines = text.split("\n")
    body_start = 0
    for i, line in enumerate(lines):
        if line.strip() == "" or line.lstrip().startswith("#"):
            body_start = i + 1
            continue
        body_start = i
        break
    body = "\n".join(lines[body_start:])

    new_text = HEADER + body
    if not new_text.endswith("\n"):
        new_text += "\n"
    if new_text == text and not had_crlf:
        return None  # 本来就标准
    if not dry:
        path.write_bytes(new_text.encode("utf-8"))
    return "改(含 CRLF→LF)" if had_crlf else "改(头)"


def main() -> None:
    args = [a for a in sys.argv[1:] if a != "--dry"]
    dry = "--dry" in sys.argv
    changed = skipped = same = 0
    for root in args:
        for path in sorted(Path(root).rglob("*.dict.yaml")):
            result = normalize(path, dry)
            if result is None:
                same += 1
            elif result.startswith("跳过") or result.startswith("读失败"):
                skipped += 1
                print(f"[{result}] {path}")
            else:
                changed += 1
                print(f"[{result}] {path}")
    print(f"合计: 改 {changed} / 原样 {same} / 跳过 {skipped}  ({'dry-run' if dry else '已写入'})")


if __name__ == "__main__":
    main()
