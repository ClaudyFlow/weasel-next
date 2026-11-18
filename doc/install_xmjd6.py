import os
import re
import shutil
from pathlib import Path

RAW = Path(r"C:\code\_work_xmjd6_20261118\raw")
DST = Path(r"C:\Program Files\Rime-Next\Weasel-Next\data")
BAK = Path(r"C:\code\_repo_cleanup_20261118\xmjd6-install-backup-20261118")
SCHEMAS = ["xmjd6", "xmjd6.cx", "xmjd6.o", "xmjd6.gbk", "pinyin_simp", "liangfen"]

log = []


def put(src: Path, dst: Path, backup=True):
    dst.parent.mkdir(parents=True, exist_ok=True)
    if dst.exists() and backup:
        rel = dst.relative_to(DST)
        target = BAK / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        if not target.exists():
            shutil.copy2(dst, target)
    shutil.copy2(src, dst)
    log.append(f"{src.name} -> {dst.relative_to(DST)}")


def schema_path(schema_id: str) -> Path:
    # id 按 '.' 分段，文件名是完整 id
    seg = "/".join(schema_id.split("."))
    return DST / "schema" / seg / f"{schema_id}.schema.yaml"


# 1. schemas
for sid in SCHEMAS:
    src = RAW / f"{sid}.schema.yaml"
    put(src, schema_path(sid))
# main custom patch
put(RAW / "xmjd6.custom.yaml", DST / "schema" / "xmjd6" / "xmjd6.custom.yaml")

# 2. dictionaries: 全部放 dict/xmjd6/ （分类器支持逐级退前缀+宽容搜索）
dicts = sorted(RAW.glob("*.dict.yaml"))
for src in dicts:
    name = src.name
    if name.startswith("pinyin_simp."):
        sub = "pinyin_simp"
    elif name.startswith("english."):
        sub = "english"
    elif name.startswith("liangfen."):
        sub = "liangfen"
    else:
        sub = "xmjd6"
    put(src, DST / "dict" / sub / name)

# 3. opencc: json + txt + ocd2 + Data/*.lua（保留现有 emoji.txt）
for src in sorted((RAW / "opencc").glob("*")):
    if src.is_file():
        put(src, DST / "opencc" / src.name)
for src in sorted((RAW / "opencc" / "Data").glob("*")):
    if src.is_file():
        put(src, DST / "opencc" / "Data" / src.name)

# 4. Lua scripts: lua/xmjd6/x.lua -> plugin/xmjd6/x/xmjd6.x.lua（恒等式布局）
for src in sorted((RAW / "lua" / "xmjd6").glob("*.lua")):
    stem = src.stem
    put(src, DST / "plugin" / "xmjd6" / stem / f"xmjd6.{stem}.lua")
for src in sorted((RAW / "lua" / "eng").glob("*.lua")):
    stem = src.stem
    put(src, DST / "plugin" / "eng" / stem / f"eng.{stem}.lua")

# 5. icon
if (RAW / "xmjd6.ico").is_file():
    put(RAW / "xmjd6.ico", DST / "icon" / "xmjd6.ico")

# 6. 从 schema 里抽取所有 lua_*@ 组件，生成全局注册片段
components = set()
for sid in SCHEMAS:
    p = RAW / f"{sid}.schema.yaml"
    text = p.read_text(encoding="utf-8-sig")
    for m in re.finditer(r"lua_(?:processor|translator|filter|segmentor)@(\*?[A-Za-z0-9_./@]+)", text):
        name = m.group(1).lstrip("*")
        components.add(name)

reg_lines = [
    "-- xmjd6 社区布局适配：把 @*xmjd6/名字 注册成引擎能解析的全局名",
    "-- 模块文件已按恒等式布局放到 plugin/xmjd6/<名字>/xmjd6.<名字>.lua",
    "local function xmjd6_reg(alias, module)",
    "  local ok, mod = pcall(require, module)",
    "  if ok then _G[alias] = mod else boot_log('xmjd6 require 失败 ' .. module .. ': ' .. tostring(mod)) end",
    "end",
]
for name in sorted(components):
    module = name.replace("/", ".")
    # @tag 后缀不进模块名，只进全局键
    module = re.sub(r"@.*$", "", module)
    reg_lines.append(f'xmjd6_reg("{name}", "{module}")')

reg = "\n".join(reg_lines) + "\n"
reg_path = Path(r"C:\code\_work_xmjd6_20261118\xmjd6_bootstrap_snippet.lua")
reg_path.write_text(reg, encoding="utf-8")
print("COMPONENTS", len(components))
print("REG_SNIPPET", reg_path)
print("FILES", len(log))
print("\n".join(log[:12]))
print("...")
print("\n".join(log[-8:]))
