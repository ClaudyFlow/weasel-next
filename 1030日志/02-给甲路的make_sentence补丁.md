# 给甲路的补丁建议：`make_sentence` 的 O(n²) 查库（2026-10-30 乙路）

> 乙路不碰 `librime-next`（你在改 `menu.rs` / `translation.rs` / `runtime.rs`），
> 这份只是**建议 + 可直接粘贴的改动**，你决定要不要收。
> 相关文件：`librime-next/src/engine/gear/table_translator.rs`

---

## 一、实测证据（乙路 02:00–02:10，release 探针）

喂 `tianqibucuo`（自然码双拼，11 键），在 `process_key → compose → translate_segments`
内打时间戳：

```
processors          0.03–0.17 ms
notify_update       ~0 ms
segmentation        0.01–0.03 ms
menu.prepare        0–0.03 ms
filters             0.08–0.21 ms
translate_segments  0.36–32.4 ms   ← 唯一大头
apply_commit        ~0 ms
```

逐键：`0.90 / 1.31 / 1.88 / 0.89 / 12.83 / 1.99 / 32.43 / 0.36 / 0.53 / 0.59 / 8.51`
→ **输入越长越慢，长句后半段每键 25 ms 上下**，这就是用户说的"打字卡顿"。
候选数上限实测 50（分工文档里写的 551/1062 那条口径不对，可忽略）。

---

## 二、嫌疑人：`make_sentence` 的三重循环

`table_translator.rs:524` 起：

```
for start_pos in 0..n                      // n = 输入长度
  if !vertices.contains(start_pos) continue
  ① 用户词典：for len in 1..=n → user_dict.lookup_words(input[start..start+len])   // n 次查库
  ② 编码器  ：for len in 1..=n → encoder.lookup_phrases(...)                        // n 次
  ③ 词表    ：prism.common_prefix_search(...) 逐条 lookup_words
```

查库次数 ≈ **可达起点数 × n**。对 `tianqibucuo` 大约 4×10 = 40 次用户词典 + 40 次编码器
+ 词表若干，按 0.1–0.3 ms/次算正好是 10–30 ms。

### 关键观察

这三段查库的结果**只取决于 `key` 本身**，跟 `start_pos` 一点关系都没有。
而打字过程中同一批 key 会被**逐键反复查**：

```
按键 1: t
按键 2: t, ti
按键 3: t, ti, tia
按键 4: t, ti, tia, tian
...
```

第 11 键查的那 11 个 key，前面 10 键**全都查过了**。

---

## 三、建议改动：按 key 记忆化（改动面只在 `table_translator.rs` 内）

### 3.1 加一个缓存字段

```rust
pub struct TableTranslator {
    // ... 现有字段 ...
    /// 组句查库的记忆化缓存（2026-10-30）：三段查库只取决于 key。
    /// 词表侧（prism）不可变，永久有效；用户词典侧在 `memorize` 写库后清空。
    sentence_cache: SentenceCache,
}

#[derive(Default)]
struct SentenceCache {
    /// key → 命中的词条（词表侧，不可变，不需要失效）
    table: HashMap<String, Vec<DictEntry>>,
    /// key → 命中的词条（用户词典侧，`memorize` 后 clear）
    user: HashMap<String, Vec<DictEntry>>,
}

impl SentenceCache {
    /// 容量上限：长句 + 长输入下 key 的数量是 O(n²)，必须封顶。
    const LIMIT: usize = 4096;
    fn get(&self, table_side: bool, key: &str) -> Option<&Vec<DictEntry>> {
        if table_side { self.table.get(key) } else { self.user.get(key) }
    }
    fn put(&mut self, table_side: bool, key: String, entries: Vec<DictEntry>) {
        let map = if table_side { &mut self.table } else { &mut self.user };
        if map.len() >= Self::LIMIT { return; }   // 满了就不再记，正确性不受影响
        map.insert(key, entries);
    }
    /// 用户词典被写过：这一侧的缓存全部作废
    fn invalidate_user(&mut self) { self.user.clear(); }
}
```

### 3.2 在 `make_sentence` 的三处查库前后套上缓存

① 用户词典（第 549–568 行附近）：

```rust
let key = active_input.get(..len).expect("len <= input.len()");
// 原来：
//   let mut uter = UserDictEntryIterator::new();
//   user_dict.lookup_words(&mut uter, key, false, 0, None);
//   ...
//   let entries = drain_user_entries(&mut uter);
// 改成：
let entries = match self.sentence_cache.get(false, key) {
    Some(cached) => cached.clone(),
    None => {
        let mut uter = UserDictEntryIterator::new();
        user_dict.lookup_words(&mut uter, key, false, 0, None);
        if self.filter_by_charset() {
            uter.add_filter(Box::new(charset_filter::filter_dict_entry));
        }
        let found = if uter.exhausted() { Vec::new() } else { drain_user_entries(&mut uter) };
        self.sentence_cache.put(false, key.to_string(), found.clone());
        found
    }
};
if entries.is_empty() { continue; }
// 后面照旧：vertices.insert(end_pos); split_entries(...)
```

> ⚠️ 借用：`self.sentence_cache` 与 `self.user_dict` 是**不同字段**，直接字段访问
> Rust 允许同时借（`&self.user_dict` + `&mut self.sentence_cache`）。
> 若 `user_dict` 是从方法里取的（不是直接字段），先把 `Arc`/引用拎出来再改缓存。

② 编码器（第 578–604 行）同理，用同一张 `user` 表（它也读用户词典）。

③ 词表（第 620–645 行）用 `table` 表，**永远有效**，收益最直接：

```rust
let key = active_input.get(..m.length).expect("checked");
let entries = match self.sentence_cache.get(true, key) {
    Some(cached) => cached.clone(),
    None => {
        let mut iter = DictEntryIterator::new();
        self.dict.lookup_words(&mut iter, key, false, 0, Some(&blacklist));
        if self.filter_by_charset() { iter.add_filter(Box::new(charset_filter::filter_dict_entry)); }
        let found = if iter.exhausted() { Vec::new() } else { drain_table_entries(&mut iter) };
        self.sentence_cache.put(true, key.to_string(), found.clone());
        found
    }
};
```

顺带：`prism.common_prefix_search(active_input)` 本身也可以按 `active_input` 缓存
（纯函数，且同一个 `active_input` 在同一帧里只出现一次，收益有限，可不做）。

### 3.3 失效点

`memorize()`（第 387 行）写用户词典后：

```rust
// 写库之后：用户词典侧的缓存作废
self.sentence_cache.invalidate_user();
```

放在 `memorize` 的**函数开头或结尾都行** —— 只要在真正 `update_entry` 之后。
`encoder.encode_phrase` / `encode_from_history` 也写库，同样在这之后清一次。

### 3.4 预期收益

* 逐键输入时，除了"新冒出来的那一个 key"，其余全部命中缓存
  → 每键的查库次数从 `可达起点数 × n` 降到 **`可达起点数 × 1`**（只剩新 key）
  → 预计 10–30 ms 降到 1–3 ms。
* 内存代价：4 096 条 × 平均几条 `DictEntry`，可控（上限封顶）。
* 语义不变：缓存只是"同一个 key 的查询结果"，词表侧不可变，用户侧写库即失效。

---

## 四、如果你想先量一遍（5 分钟的插桩）

`librime-next/src/engine/prof.rs`（新文件）：

```rust
use std::time::Instant;
pub fn enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("WEASEL_PROFILE").map(|v| v != "0").unwrap_or(false))
}
pub fn start() -> Instant { Instant::now() }
pub fn mark(label: &str, t0: Instant) {
    if enabled() { eprintln!("[prof] {:<24} {:>7.2} ms", label, t0.elapsed().as_secs_f64()*1000.0); }
}
pub fn mark_ms(label: &str, ms: f64) {
    if enabled() && ms > 0.05 { eprintln!("[prof] {:<24} {:>7.2} ms", label, ms); }
}
```

`engine.rs` 加 `pub mod prof;`。
`runtime.rs`：`process_key` 开头 `mark("processors")`；`compose` 里分段
`mark("segmentation")` / `mark("translate_segments")`；`translate_segments` 里
`mark("menu.prepare")` / `mark("filters")`。

`table_translator.rs`：`make_sentence` 里给 ①②③④ 各加一个 `f64` 累加器，循环外用
`mark_ms("  ms.user_dict" / "  ms.encoder" / "  ms.prism" / "  ms.poet", acc)` 报出来。

然后：

```bash
cargo build -p librime-next --release --example probe_sp_step
WEASEL_PROFILE=1 ./target/release/examples/probe_sp_step.exe \
  "C:/Users/CloudyFlow/AppData/Roaming/Rime-Next/build" \
  "C:/Users/CloudyFlow/AppData/Roaming/Rime-Next" \
  "C:/Program Files/Rime-Next/Weasel-Next-0.18.5/Rime-Next" tianqibucuo
```

看 ① ② ③ 谁占大头，再决定先缓存哪一侧。

---

## 五、顺带两条（不紧急）

1. `make_sentence` 里 `let blacklist = self.options.blacklist.clone();`
   和 `let delimiters = self.options.delimiters.clone();` **每键都 clone 一次**
   （HashSet + String）。改成借 `&self.options.blacklist` 或缓存起来，能省一点。
2. `query()` 里 `lookup_words(..., false, 0, ...)` 的 `limit = 0` 表示"不限"，
   非补全模式下会把**整码命中的全部词条**都 drain 出来再排序。C++ 是懒式 peek，
   只取到首屏。这个正好是你正在做的懒化管线能覆盖到的地方。
