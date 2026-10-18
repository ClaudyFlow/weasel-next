#!/usr/bin/env python3
# 从 `Rime-Next\shuangpin\<方案>.toml` 生成 rime 认的 `.schema.yaml`。
#
# 为什么需要这一步：librime 的 ResourceResolver 写死只找 `.schema.yaml`，
# 它不认 TOML。所以约定是——
#     shuangpin\*.toml      ← 人编辑的配置（唯一源）
#     schema\<id>\*.schema.yaml  ← 本脚本生成的产物（别手改）
#
# 用法：
#     python script\gen-shuangpin.py                    # 生成全部 *.toml
#     python script\gen-shuangpin.py Rime-Next\shuangpin\xiaohe.toml
#
# 设计要点：**规则形态固定、键位字母从 TOML 取**。
# 双拼的 algebra 结构（哪个韵母用捕获组、哪些合并）是由 rime 的匹配语义决定的，
# 不是随便排的；所以这里不"推导"规则，只把 TOML 里的键位字母填进固定模板。
# 这样改键位（比如把 an 从 j 挪到别的键）能自动生效，又不会破坏匹配顺序。

import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SHUANGPIN_DIR = ROOT / "Rime-Next" / "shuangpin"
SCHEMA_DIR = ROOT / "Rime-Next" / "schema"

# (正则片段, 需要保留的捕获组前缀, 韵母键名)
# 顺序 = 官方 rime-double-pinyin 的顺序，不可乱：长韵母必须排在短的前面
# （否则 liang 会被 ang 规则先命中，变成 lih）。
ALGEBRA_TEMPLATE = [
    (r"iu$",                          "",   "iu"),
    (r"(.)ei$",                       "$1", "ei"),
    (r"uan$",                         "",   "uan"),
    (r"[uv]e$",                       "",   "ue"),      # ue / ve 同键
    (r"un$",                          "",   "un"),
    (r"uo$",                          "",   "uo"),
    (r"ie$",                          "",   "ie"),
    (r"i?ong$",                       "",   "ong"),     # ong / iong 同键
    (r"ing$|uai$",                    "",   "ing"),     # ing / uai 同键
    (r"(.)ai$",                       "$1", "ai"),
    (r"(.)en$",                       "$1", "en"),
    (r"(.)eng$",                      "$1", "eng"),
    (r"[iu]ang$",                     "",   "iang"),    # iang / uang 同键
    (r"(.)ang$",                      "$1", "ang"),
    (r"ian$",                         "",   "ian"),
    (r"(.)an$",                       "$1", "an"),
    (r"(.)ou$",                       "$1", "ou"),
    (r"[iu]a$",                       "",   "ia"),      # ia / ua 同键
    (r"iao$",                         "",   "iao"),
    (r"(.)ao$",                       "$1", "ao"),
    (r"ui$",                          "",   "ui"),      # ui / ü 同键
    (r"in$",                          "",   "in"),
]

# 零声母处理：两个字母的韵母直接全拼敲，其余「首字母 + 韵母键」
ZERO_FULL = ["ai", "an", "ao", "ei", "en", "er", "ou"]


def build_algebra(cfg) -> list[str]:
    finals = cfg["finals"]
    initials = cfg.get("initials", {})
    lines = []

    # ① jqxy + u → v（官方把这行放最前：jue/juan 这类要先变 jve/jvan，才走得到韵母规则）
    lines.append("derive/^([jqxy])u$/$1v/")

    # ② 双字母声母改成大写占位，避免被韵母规则误伤
    for initial in ("zh", "ch", "sh"):
        if initial in initials:
            lines.append(f"xform/^{initial}/{initials[initial].upper()}/")

    # ③ 零声母（必须在韵母规则之前）
    lines.append("xform/^([aoe])(ng)?$/$1$1$2/")       # a→aa, ang→ah, e→ee, eng→eg, o→oo
    lines.append("derive/^([aoe])([ioun])$/$1$1$2/")   # ai→aai, an→aan …（保留一条修正规则）

    # ④ 韵母（顺序固定，长的在前）
    for pattern, capture, key in ALGEBRA_TEMPLATE:
        letter = finals.get(key)
        if not letter:
            continue
        lines.append(f"xform/{pattern}/{capture}{letter.upper()}/")

    # ⑤ 全部大写中间标记转小写
    lines.append(
        "xlit/QWRTYUIOPSDFGHJKLZXCVBNM/qwrtyuiopsdfghjklzxcvbnm/"
    )
    return lines


SCHEMA_TEMPLATE = """# Rime schema
# encoding: utf-8
#
# ★ 本文件由 `script\\gen-shuangpin.py` 从 `Rime-Next\\shuangpin\\{source}` 生成。
#   **不要手改**：改键位请改那份 TOML，然后重新生成。
#
# 这是「转写层」方案：词典与明月拼音共用。

schema:
  schema_id: {schema_id}
  name: {name}
  version: "{version}"
  author:
    - {author}
  description: |
    {name}（明月拼音 + 双拼键位）。

switches:
  - name: ascii_mode
    reset: 0
    states: [ 中文, 西文 ]
  - name: full_shape
    states: [ 半角, 全角 ]
  - name: {simp_option}
    reset: 1
    states: [ 漢字, 汉字 ]
  - name: ascii_punct
    states: [ 。，, ．， ]

engine:
  processors:
    - ascii_composer
    - recognizer
    - key_binder
    - speller
    - punctuator
    - selector
    - navigator
    - express_editor
  segmentors:
    - ascii_segmentor
    - matcher
    - abc_segmentor
    - punct_segmentor
    - fallback_segmentor
  translators:
    - punct_translator
    - script_translator
  filters:
    - simplifier
    - uniquifier

speller:
  alphabet: zyxwvutsrqponmlkjihgfedcba
  delimiter: " '"
  algebra:
{algebra}

translator:
  dictionary: {dict}
  prism: {prism}

simplifier:
  option_name: {simp_option}

punctuator:
  import_preset: default

key_binder:
  import_preset: default

recognizer:
  import_preset: default
"""


def generate(toml_path: Path) -> Path:
    cfg = tomllib.loads(toml_path.read_text(encoding="utf-8"))
    meta = cfg["schema"]
    schema_id = meta["id"]

    algebra = "\n".join(f"    - {line}" for line in build_algebra(cfg))

    text = SCHEMA_TEMPLATE.format(
        source=toml_path.name,
        schema_id=schema_id,
        name=meta["name"],
        version=meta.get("version", "0.18"),
        author=meta.get("author", ""),
        dict=meta.get("dict", "luna_pinyin"),
        prism=meta.get("prism", schema_id),
        simp_option=meta.get("simp_option", "zh_simp"),
        algebra=algebra,
    )

    out_dir = SCHEMA_DIR / schema_id
    out_dir.mkdir(parents=True, exist_ok=True)
    out_path = out_dir / f"{schema_id}.schema.yaml"
    out_path.write_text(text, encoding="utf-8")
    return out_path


def main() -> int:
    args = sys.argv[1:]
    if args:
        targets = [Path(a) if Path(a).is_absolute() else (ROOT / a) for a in args]
    else:
        targets = sorted(SHUANGPIN_DIR.glob("*.toml"))

    if not targets:
        print(f"没有找到 {SHUANGPIN_DIR}/*.toml")
        return 1

    for toml_path in targets:
        if not toml_path.is_file():
            print(f"跳过（不存在）：{toml_path}")
            continue
        out = generate(toml_path)
        print(f"✓ {toml_path.name}  →  {out.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
