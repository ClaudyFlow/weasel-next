#!/bin/bash
# 系列文件夹迁移 v2（备份式，可重复执行）：
#   归位  → mv 到系列目标
#   重复  → mv 到备份区（不删）
#   冲突  → park 成 <目标>.yuhao-folder-variant（不覆盖）
# 已执行过的行 src 不存在会自动跳过，可安全重跑。
set -u
ROOT="$1"
MOVES="$2"
BACKUP="$ROOT/../migration-backup-20261025-$(basename "$ROOT")"
cd "$ROOT" || exit 1
mkdir -p "$BACKUP"
moved=0; duped=0; parked=0; n=0
while IFS= read -r line; do
  [ -z "$line" ] && continue
  n=$((n+1))
  src="schema/$(echo "$line" | awk '{print $2}')"
  dst=$(echo "$line" | awk '{print $4}')
  [ -e "$src" ] || continue
  if [ -e "$dst" ]; then
    if diff -q "$src" "$dst" >/dev/null 2>&1; then
      mkdir -p "$BACKUP/$(dirname "$src")"
      mv "$src" "$BACKUP/$src"; duped=$((duped+1))     # 纯重复 → 备份区
    else
      mv "$src" "$dst.yuhao-folder-variant"            # 冲突 → park
      echo "PARK  $src -> $dst.yuhao-folder-variant"
      parked=$((parked+1))
    fi
  else
    mkdir -p "$(dirname "$dst")"
    mv "$src" "$dst"; moved=$((moved+1))
  fi
  [ $((n % 10)) -eq 0 ] && echo "...已处理 $n 行"
done < "$MOVES"
for i in 1 2 3; do find schema -type d -empty -delete 2>/dev/null; done
refs=0
while IFS= read -r f; do
  if grep -q "schema/yuhao/" "$f" 2>/dev/null; then
    sed -i 's|schema/yuhao/||g' "$f"; refs=$((refs+1))
  fi
done < <(find schema -name "*.yaml")
echo "完成: 归位 $moved, 重复入备份 $duped, 冲突park $parked, 改写引用 $refs 个文件"
echo "备份区: $BACKUP"
