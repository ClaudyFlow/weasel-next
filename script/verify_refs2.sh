#!/bin/bash
# 系列布局依赖验证器 v2：精确收集资源引用并按系列规则验证存在性。
#   import_tables / packs / dependencies 列表项：只取紧跟键名的缩进列表项
#   dictionary: / prism: 标量值
#   __include: <res>:/... 的资源部分（去掉 :/... 节点路径）
# id 规范化：剥掉 .schema/.dict 类型后缀后，其余按 '.' 分段做系列路径。
set -u
USER="$1"; SHARED="$2"
cd "$USER" || exit 1
: > /tmp/ref_ids2.txt
while IFS= read -r f; do
  awk '
    /^[ \t]*(import_tables|packs|dependencies):/ { inlist=1; next }
    inlist && /^[ \t]*-[ \t]*/ {
      v=$0; sub(/^[ \t]*-[ \t]*/,"",v); sub(/[ \t]*#.*/,"",v); sub(/[ \t]+$/,"",v)
      if (v != "") print v
      next
    }
    inlist && !/^[ \t]*-[ \t]*/ { inlist=0 }
    /^[ \t]*(dictionary|prism):[ \t]*[^ \t]/ {
      v=$0; sub(/^[ \t]*[a-z_]+:[ \t]*/,"",v); sub(/[ \t]*#.*/,"",v); sub(/[ \t]+$/,"",v)
      if (v != "") print v
    }
    /__include:[ \t]*[^:#]+:/ {
      v=$0; sub(/.*__include:[ \t]*/,"",v); sub(/:.*/,"",v); sub(/[ \t]+$/,"",v)
      if (v != "") print v
    }
  ' "$f" >> /tmp/ref_ids2.txt
done < <(find schema -name "*.yaml")
sort -u /tmp/ref_ids2.txt > /tmp/ref_ids2.u
miss=0
while IFS= read -r id; do
  [ -z "$id" ] && continue
  stem="$id"
  stem="${stem%.schema}"; stem="${stem%.dict}"
  found=""
  for root in "$USER" "$SHARED"; do
    [ -d "$root" ] || continue
    if [[ "$id" == */* ]]; then
      for sfx in .dict.yaml .schema.yaml .yaml ""; do
        [ -e "$root/$id$sfx" ] && found="$root/$id$sfx" && break
      done
    else
      series=$(echo "$stem" | tr '.' '/')
      for sfx in .dict.yaml .schema.yaml .yaml; do
        [ -e "$root/schema/$series/$id$sfx" ] && found="$root/schema/$series/$id$sfx" && break
      done
    fi
    [ -n "$found" ] && break
  done
  if [ -z "$found" ]; then echo "MISS  $id"; miss=$((miss+1)); fi
done < /tmp/ref_ids2.u
echo "--- 验证完成：$(wc -l < /tmp/ref_ids2.u) 个引用，缺失 $miss 个 ---"
