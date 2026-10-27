#!/usr/bin/env python3
"""schema.yaml 标准化(Rime-Next 数据标准,2026-10-27)。

所有 `*.schema.yaml` 源文件统一为:
* 标准三行头:`# Rime Schema` / `# encode: utf-8` / `# newline: lf`
* `schema:` 块内单数 `id:`(旧式 `schema_id:` 改名;两键并存时保留 `id:` 删 `schema_id:`)
* CRLF → LF(声明与实测一致)

只动头部与 schema 块的键名,其余内容一字不改;`---` 之后的词典正文不碰。
build/ 下的编译产物不归本工具管(部署器 write_compiled_schema 自带标准形)。

用法:
    python script/normalize-schema-headers.py <目录> [...] [--dry]
"""

import sys
from pathlib import Path

HEADER = "# Rime Schema\n# encode: utf-8\n# newline: lf\n"


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

    # ① 找正文起点(第一个非注释、非空行)
    body_start = 0
    for i, line in enumerate(lines):
        if line.strip() == "" or line.lstrip().startswith("#"):
            body_start = i + 1
            continue
        body_start = i
        break

    # ② schema 块内 schema_id: → id:(保持缩进与行内其余部分;id: 已存在时删 schema_id: 行)
    changed_id = False
    in_schema = False
    out_lines = []
    for line in lines[body_start:]:
        stripped = line.lstrip()
        indent = len(line) - len(stripped)
        if indent == 0 and stripped.rstrip() == "schema:":
            in_schema = True
        elif indent == 0 and stripped.strip() and not stripped.startswith("#"):
            in_schema = False
        if in_schema and stripped.startswith("schema_id:"):
            if any(l.lstrip().startswith("id:") for l in out_lines if l.startswith(" ")):
                changed_id = True  # id: 已有,丢弃 schema_id: 行
                continue
            line = line.replace("schema_id:", "id:", 1)
            changed_id = True
        out_lines.append(line)

    head = lines[:body_start]
    # ③ 头是否已标准(三行逐字)
    head_ok = len(head) >= 3 and (
        head[0] == "# Rime Schema"
        and head[1] == "# encode: utf-8"
        and head[2] == "# newline: lf"
    )

    # 头已标准 ⇒ 原样保留整个头(标准三行之后的作者注释等也保留);
    # 只有头不标准才整段换成标准三行。
    new_text = ("\n".join(head) + "\n\n" if head_ok else HEADER) + "\n".join(out_lines)
    if not new_text.endswith("\n"):
        new_text += "\n"

    if new_text == text and not had_crlf:
        return None
    if not dry:
        path.write_bytes(new_text.encode("utf-8"))
    tags = []
    if not head_ok:
        tags.append("换头")
    if changed_id:
        tags.append("id 单数")
    if had_crlf:
        tags.append("CRLF→LF")
    return "改(" + ",".join(tags) + ")"


def main() -> None:
    args = [a for a in sys.argv[1:] if a != "--dry"]
    dry = "--dry" in sys.argv
    changed = skipped = same = 0
    for root in args:
        for path in sorted(Path(root).rglob("*.schema.yaml")):
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
