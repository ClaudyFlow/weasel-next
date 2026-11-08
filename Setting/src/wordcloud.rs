//! 用户词云：取词 → 加权 → 映射 → 布局。
//!
//! 数据源就是 Rime 用户词典本身，**不需要分词**：`userdb/<方案>.userdb` 里的 key 是
//! `编码\t词`，value 是 `c=上屏次数 d=权重 t=tick`。词条本来就是词。
//!
//! 引擎运行时会独占 `LOCK`（`open_readonly` 也照样去拿锁），所以这里先把它复制到临时
//! 目录**跳过 LOCK 文件**再打开；一个都读不到就回退 `user_ngram.tsv` 的 `u` 行。
//!
//! 四个环节里只有布局是难的：阿基米德螺线试位 + 网格位图 mask + 积分图 O(1) 碰撞查询。
//! 布局只输出坐标，真正画字的是 Slint —— 它用系统字体，比自带字体文件更省事，也自带
//! 亚像素抗锯齿。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 画布逻辑尺寸。Slint 侧按容器宽度等比缩放，所以这里只关心比例。
pub const CANVAS_W: f32 = 1040.0;
pub const CANVAS_H: f32 = 400.0;

/// 词云里的一项。坐标是**左上角**（Slint `Text` 直接用）。
#[derive(Debug, Clone, PartialEq)]
pub struct CloudItem {
    pub word: String,
    /// 字号（画布坐标系下的 px）
    pub size: i32,
    pub x: i32,
    pub y: i32,
    /// 按字号估算的宽度（Slint 侧给 `Text` 固定宽度，保证渲染框和布局一致）
    pub w: i32,
    /// 颜色档 0..=4（越大越深，按排名分档）
    pub tint: i32,
    /// 上屏次数，悬停/说明用
    pub count: u32,
}

/// 词云的来源说明，给界面上那行灰字用。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CloudSummary {
    /// 清洗后的候选词数
    pub candidates: usize,
    /// 真正放下的词数
    pub placed: usize,
    /// `userdb` / `ngram` / `none`
    pub source: &'static str,
    /// 用的哪个方案（`luna_pinyin` …；`ngram` 路径为空）
    pub schema: String,
    /// 一共有几个方案的用户词典可选
    pub schemas: usize,
}

impl CloudSummary {
    /// 标题右边那行小字。说清「多少词、从哪来」，否则用户会以为这是全域统计。
    pub fn note(&self) -> String {
        match self.source {
            "userdb" => {
                let more = if self.schemas > 1 {
                    format!("（共 {} 个方案）", self.schemas)
                } else {
                    String::new()
                };
                format!(
                    "{} · {} 条学词 · 图上 {} 个{}",
                    self.schema,
                    thousands(self.candidates),
                    self.placed,
                    more
                )
            }
            "ngram" => format!("{} 个词 · 来自 user_ngram.tsv", thousands(self.candidates)),
            _ => "还没有学词数据，先打几个字吧".to_string(),
        }
    }
}

/// `1260` → `1 260`（千位空格，跟统计页其它数字一致）。
fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// 中文里真正的高频虚词。留着它们，`的`（216 次）会占掉画面正中间。
const STOPWORDS: &[&str] = &[
    "的", "了", "是", "和", "在", "有", "不", "也", "就", "都", "而", "及", "与", "或", "但",
    "却", "很", "太", "更", "最", "才", "又", "再", "还", "只", "被", "把", "给", "让", "从",
    "到", "对", "向", "于", "以", "为", "之", "其", "此", "我", "你", "他", "她", "它", "咱们",
    "我们", "你们", "他们", "她们", "它们", "自己", "大家", "别人", "这", "那", "哪", "这个",
    "那个", "哪个", "这些", "那些", "哪些", "这样", "那样", "怎样", "这里", "那里", "哪里",
    "这么", "那么", "怎么", "一个", "一些", "一下", "一点", "一次", "一直", "一起", "一样",
    "一定", "一般", "其实", "东西", "事情", "时候", "地方", "方面", "问题", "什么", "为什么",
    "怎么办", "如何", "是否", "是不是", "有没有", "能不能", "可以", "可能", "应该", "需要",
    "还要", "就是", "不是", "也是", "还是", "但是", "因为", "所以", "如果", "虽然", "不过",
    "只是", "而且", "然后", "于是", "因此", "否则", "已经", "刚才", "马上", "总是", "经常",
    "有时", "大概", "也许", "差不多", "基本上", "没有", "不要", "不用", "不能", "不会", "吗",
    "呢", "吧", "啊", "呀", "哦", "嗯", "哈", "啦", "嘛", "呗", "说的", "看看", "觉得",
    // 繁体写法必须单独列：`luna_pinyin` 里的学词是繁体的，简体停用词表一个字都对不上，
    // 结果「這個 / 什麼 / 怎麼」全留在图里（2026-11-03 截图实测）。
    "這個", "那個", "哪個", "這些", "那些", "哪些", "這樣", "那樣", "怎樣", "這裡", "那裡",
    "哪裡", "這麼", "那麼", "怎麼", "什麼", "爲什麼", "為什麼", "怎麼辦", "我們", "你們",
    "他們", "她們", "它們", "一個", "一些", "一下", "一點", "一次", "一直", "一起", "一樣",
    "一定", "一般", "其實", "東西", "事情", "時候", "地方", "方面", "問題", "可以", "可能",
    "應該", "需要", "還要", "就是", "不是", "也是", "還是", "但是", "因為", "所以", "如果",
    "雖然", "不過", "只是", "而且", "然後", "於是", "因此", "否則", "已經", "剛才", "馬上",
    "總是", "經常", "有時", "大概", "也許", "差不多", "基本上", "沒有", "不要", "不用",
    "不能", "不會", "說的", "現在", "還有", "東西", "這個", "那種", "為了",
];

/// 最短/最长词长：单字排除（`的`/`是` 这类已进停用词，剩下的单字多是误上屏），
/// 超过 8 个字的基本是整句误学。
const MIN_LEN: usize = 2;
const MAX_LEN: usize = 8;

/// 词云最多放多少个词。100~200 之间最平衡：再多画面就糊了。
const MAX_WORDS: usize = 150;

/// 字号区间（画布 1040×340）。
const SIZE_MIN: f32 = 13.0;
const SIZE_MAX: f32 = 52.0;

/// 用户数据根（`%APPDATA%\Rime-Next`，安装时自选位置由 `userdir.txt` 覆盖）。
fn state_root() -> Option<PathBuf> {
    weasel_common::process::user_data_dir()
}

/// 生成词云：读词典 → 清洗 → 定字号 → 布局。返回 (词云项, 摘要)。
pub fn build() -> (Vec<CloudItem>, CloudSummary) {
    let (words, source, schema, schemas) = load_words();
    let candidates = words.len();
    let picked: Vec<(String, u64)> = rank(words).into_iter().take(MAX_WORDS).collect();
    let items = layout(&picked);
    let summary = CloudSummary {
        candidates,
        placed: items.len(),
        source,
        schema,
        schemas,
    };
    (items, summary)
}

// ---------------------------------------------------------------- ① 取词

/// 读学词。返回 (词, 次数)、来源标记、选中的方案名、可用方案数。
///
/// **只取一个方案**：`luna_pinyin` 存的是繁体（文件夾/這個），`pinyin` 存的是简体
/// （文件夹/这个），合并在一起图里就会同时出现「文件夾」和「文件夹」。选学词最多的
/// 那个方案，图内部就是统一的字形。要做真正的简繁归一，得引入 opencc，不值当。
fn load_words() -> (Vec<(String, u64)>, &'static str, String, usize) {
    if let Some((words, schema, schemas)) = load_from_userdb() {
        return (words, "userdb", schema, schemas);
    }
    let from_ngram = load_from_ngram();
    if !from_ngram.is_empty() {
        return (from_ngram, "ngram", String::new(), 0);
    }
    (Vec::new(), "none", String::new(), 0)
}

/// 从 `userdb/*.userdb` 读，挑学词最多的那个方案。
fn load_from_userdb() -> Option<(Vec<(String, u64)>, String, usize)> {
    let dir = state_root()?.join("userdb");
    let entries = std::fs::read_dir(&dir).ok()?;
    let tmp = tempfile::tempdir().ok()?;

    let mut per_schema: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();

    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.ends_with(".userdb").then_some(name)
        })
        .collect();
    names.sort();

    for name in names {
        let src = dir.join(&name);
        let dst = tmp.path().join(&name);
        // 复制时跳过 LOCK：引擎正占用它，连读都读不了
        if copy_db(&src, &dst).is_err() {
            continue;
        }
        let rows = read_db(&dst);
        if rows.is_empty() {
            continue;
        }
        let schema = name.trim_end_matches(".userdb").to_string();
        let bucket = per_schema.entry(schema).or_default();
        for (word, count) in rows {
            if word.is_empty() || word.starts_with('/') {
                continue; // meta 键 / 空词
            }
            *bucket.entry(word).or_default() += count;
        }
    }

    let schemas = per_schema.len();
    // 比的是**清洗后的词条数**，不是原始条数：某个方案可能塞了一堆单字
    let best = per_schema
        .into_iter()
        .max_by_key(|(_, m)| {
            rank(m.iter().map(|(k, v)| (k.clone(), *v)).collect()).len()
        })?;
    let (schema, bucket) = best;
    Some((bucket.into_iter().collect(), schema, schemas))
}

/// 复制 `.userdb` 目录，跳过 `LOCK`（被引擎独占）。
fn copy_db(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy() == "LOCK" {
            continue;
        }
        if !entry.file_type()?.is_file() {
            continue;
        }
        std::fs::copy(entry.path(), dst.join(&name))?;
    }
    Ok(())
}

/// 只读遍历一个 LevelDB：解析 `编码\t词` 与 `c=次数`。
fn read_db(path: &Path) -> Vec<(String, u64)> {
    use rusty_leveldb::LdbIterator;
    let options = rusty_leveldb::Options {
        create_if_missing: false,
        ..Default::default()
    };
    let Ok(mut db) = rusty_leveldb::DB::open(path, options) else {
        return Vec::new();
    };
    let Ok(mut iter) = db.new_iter() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    LdbIterator::seek_to_first(&mut iter);
    while let Some((key, value)) = LdbIterator::current(&mut iter) {
        let key = String::from_utf8_lossy(&key);
        let value = String::from_utf8_lossy(&value);
        if let Some((_code, word)) = key.split_once('\t') {
            if let Some(count) = parse_commits(&value) {
                out.push((word.to_string(), count));
            }
        }
        if !LdbIterator::advance(&mut iter) {
            break;
        }
    }
    out
}

/// `c=21 d=3.50902 t=3402` → 21
fn parse_commits(value: &str) -> Option<u64> {
    value
        .split(' ')
        .find_map(|kv| kv.strip_prefix("c="))
        .and_then(|n| n.parse::<i64>().ok())
        .map(|n| n.max(0) as u64)
}

/// 回退：`user_ngram.tsv` 的 `u` 行是 `u\t词\t次数`（已经是词条，不用分词）。
fn load_from_ngram() -> Vec<(String, u64)> {
    let Some(path) = state_root().map(|r| r.join("user_ngram.tsv")) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut acc: BTreeMap<String, u64> = BTreeMap::new();
    for line in text.lines() {
        let mut f = line.split('\t');
        if f.next() != Some("u") {
            continue;
        }
        let (Some(word), Some(count)) = (f.next(), f.next()) else {
            continue;
        };
        let Ok(n) = count.parse::<u64>() else {
            continue;
        };
        if word.is_empty() {
            continue;
        }
        *acc.entry(word.to_string()).or_default() += n;
    }
    acc.into_iter().collect()
}

/// 清洗 + 加权 + 排序。
fn rank(words: Vec<(String, u64)>) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = words
        .into_iter()
        .filter(|(word, count)| {
            let len = word.chars().count();
            *count > 0
                && (MIN_LEN..=MAX_LEN).contains(&len)
                && is_word_chars(word)
                && !STOPWORDS.contains(&word.as_str())
        })
        .collect();
    // 词频相同就按词排序，保证每次打开图都一样（不闪烁）
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// 纯汉字，或纯拉丁词（`trans` / `LoRA` 这类学词照样值得显示）。
fn is_word_chars(word: &str) -> bool {
    let mut han = false;
    let mut latin = false;
    for c in word.chars() {
        if ('\u{3400}'..='\u{9fff}').contains(&c) {
            han = true;
        } else if c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '#' | '-' | '_') {
            latin = true;
        } else {
            return false; // 标点、注音符号、emoji 一律不要
        }
    }
    if han && latin {
        return false; // `吗z` 这种混进来的噪音
    }
    true
}

// ---------------------------------------------------------------- ③ 映射

/// 字号：长尾分布必须 log，线性会让首词吃满画面。
fn size_for(weight: u64, lo: u64, hi: u64) -> f32 {
    if hi <= lo {
        return (SIZE_MIN + SIZE_MAX) / 2.0;
    }
    let t = ((weight.max(1) as f32).ln() - (lo.max(1) as f32).ln())
        / ((hi.max(2) as f32).ln() - (lo.max(1) as f32).ln());
    (SIZE_MIN + (SIZE_MAX - SIZE_MIN) * t.clamp(0.0, 1.0)).round()
}

/// 颜色档：按**排名**分 5 档。按绝对值分档的话，长尾会把 90% 的词挤进同一档。
fn tint_for(index: usize, total: usize) -> i32 {
    let frac = index as f32 / (total.max(2) - 1) as f32;
    if frac < 0.04 {
        0
    } else if frac < 0.12 {
        1
    } else if frac < 0.30 {
        2
    } else if frac < 0.58 {
        3
    } else {
        4
    }
}

/// 估算文字宽度：中文字形是方块（宽度 ≈ 字号），ASCII 约 0.55 倍。
///
/// Slint 用系统字体渲染，这里拿不到它的度量，所以**宁可算宽一点** —— 多留的空白
/// 看不出来，算窄了就是压字。
fn text_width(word: &str, size: f32) -> f32 {
    let units: f32 = word
        .chars()
        .map(|c| if c.is_ascii() { 0.56 } else { 1.0 })
        .sum();
    // 1.03 是实测补的余量：Slint 用系统字体渲染，字形比「字数 × 字号」略宽，
    // 按 1.0 估算时大词边界会互相蹭到（2026-11-03 首次上机截图）。
    units * size * 1.03 + 3.0
}

// ---------------------------------------------------------------- ④ 布局

/// 网格位图 + 积分图。`cell` 取 2px：字形是矢量、留白本来就有几个像素，
/// 再细只是浪费；`allow` 是椭圆轮廓，让词云是个云而不是一块方砖。
struct Grid {
    cell: f32,
    cw: usize,
    ch: usize,
    mask: Vec<u8>,
    allow_sum: Vec<i32>,
    mask_sum: Vec<i32>,
}

impl Grid {
    fn new(w: f32, h: f32, cell: f32) -> Self {
        let cw = (w / cell).floor() as usize;
        let ch = (h / cell).floor() as usize;
        let mut allow = vec![0u8; cw * ch];
        let (cx, cy) = (cw as f32 / 2.0, ch as f32 / 2.0);
        for y in 0..ch {
            for x in 0..cw {
                let u = (x as f32 - cx) / (cx * 0.99);
                let v = (y as f32 - cy) / (cy * 0.93);
                if u * u + v * v <= 1.0 {
                    allow[y * cw + x] = 1;
                }
            }
        }
        let mask = vec![0u8; cw * ch];
        let allow_sum = integral(&allow, cw, ch);
        let mask_sum = integral(&mask, cw, ch);
        Self {
            cell,
            cw,
            ch,
            mask,
            allow_sum,
            mask_sum,
        }
    }

    fn idx(&self, gx: usize, gy: usize) -> usize {
        gy * (self.cw + 1) + gx
    }

    /// 矩形（像素坐标）是否可放：整块都在椭圆内，且没压到已放的词。
    fn free(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
        let gx0 = (x0 / self.cell).floor().max(0.0) as usize;
        let gy0 = (y0 / self.cell).floor().max(0.0) as usize;
        let gx1 = (x1 / self.cell).ceil() as usize;
        let gy1 = (y1 / self.cell).ceil() as usize;
        if gx1 >= self.cw || gy1 >= self.ch || gx1 <= gx0 || gy1 <= gy0 {
            return false;
        }
        let area = ((gx1 - gx0) * (gy1 - gy0)) as i32;
        let a = &self.allow_sum;
        let allow = a[self.idx(gx1, gy1)] - a[self.idx(gx0, gy1)] - a[self.idx(gx1, gy0)]
            + a[self.idx(gx0, gy0)];
        if allow < area {
            return false;
        }
        let m = &self.mask_sum;
        m[self.idx(gx1, gy1)] - m[self.idx(gx0, gy1)] - m[self.idx(gx1, gy0)] + m[self.idx(gx0, gy0)]
            == 0
    }

    /// 占用一块矩形，然后重建积分图（150 次要重建 150 次，88k 格/次，划得来）。
    fn occupy(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let gx0 = (x0 / self.cell).floor().max(0.0) as usize;
        let gy0 = (y0 / self.cell).floor().max(0.0) as usize;
        let gx1 = ((x1 / self.cell).ceil() as usize).min(self.cw);
        let gy1 = ((y1 / self.cell).ceil() as usize).min(self.ch);
        for y in gy0..gy1 {
            let row = y * self.cw;
            for x in gx0..gx1 {
                self.mask[row + x] = 1;
            }
        }
        self.mask_sum = integral(&self.mask, self.cw, self.ch);
    }

    /// 测试用：这一格在不在椭圆轮廓内（查 allow 积分图的 1×1 区间）。
    #[cfg(test)]
    fn allow_at(&self, x: f32, y: f32) -> bool {
        let gx = ((x / self.cell) as usize).min(self.cw - 1);
        let gy = ((y / self.cell) as usize).min(self.ch - 1);
        let a = &self.allow_sum;
        a[self.idx(gx + 1, gy + 1)] - a[self.idx(gx, gy + 1)] - a[self.idx(gx + 1, gy)]
            + a[self.idx(gx, gy)]
            == 1
    }
}

/// 二维前缀和（多一行一列 0，查区间就是四次加减）。
fn integral(a: &[u8], w: usize, h: usize) -> Vec<i32> {
    let mut out = vec![0i32; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0i32;
        for x in 0..w {
            row += a[y * w + x] as i32;
            out[(y + 1) * (w + 1) + x + 1] = out[y * (w + 1) + x + 1] + row;
        }
    }
    out
}

/// 阿基米德螺线试位：`r = 2.1θ`，步长按弧长自适应（间距约 3px）。
fn layout(words: &[(String, u64)]) -> Vec<CloudItem> {
    const PAD: f32 = 4.0; // 词与词之间的呼吸（渲染框比估算宽，留够）
    const ASPECT: f32 = 0.62; // 纵向压扁，填成横幅而不是正圆
    let (Some((_top, hi)), Some((_, lo))) = (words.first(), words.last()) else {
        return Vec::new();
    };
    let (hi, lo) = (*hi, *lo);

    let mut grid = Grid::new(CANVAS_W, CANVAS_H, 2.0);
    let mut out = Vec::with_capacity(words.len());
    let (cx, cy) = (CANVAS_W / 2.0, CANVAS_H / 2.0);

    for (index, (word, count)) in words.iter().enumerate() {
        let size = size_for(*count, lo, hi);
        let w = text_width(word, size);
        let h = size; // 视觉高度就是字号，Slint 侧 Text 的高度也设成字号
        let mut theta = 0.0f32;
        let mut r = 0.0f32;
        let mut placed = None;
        for _ in 0..30_000 {
            let x = cx + r * theta.cos();
            let y = cy + r * theta.sin() * ASPECT;
            let (x0, y0) = (x - w / 2.0 - PAD, y - h / 2.0 - PAD);
            let (x1, y1) = (x + w / 2.0 + PAD, y + h / 2.0 + PAD);
            if grid.free(x0, y0, x1, y1) {
                placed = Some((x0 + PAD, y0 + PAD));
                grid.occupy(x0, y0, x1, y1);
                break;
            }
            theta += (4.0 / r.max(4.0)).max(0.035);
            r = 2.1 * theta;
        }
        let Some((left, top)) = placed else {
            continue; // 放不下就丢，绝不重叠
        };
        out.push(CloudItem {
            word: word.clone(),
            size: size as i32,
            x: left.round() as i32,
            y: top.round() as i32,
            w: w.ceil() as i32,
            tint: tint_for(index, words.len()),
            count: (*count).min(u32::MAX as u64) as u32,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commits_from_user_db_value() {
        assert_eq!(parse_commits("c=21 d=3.50902 t=3402"), Some(21));
        assert_eq!(parse_commits("c=1 d=1 t=175"), Some(1));
        assert_eq!(parse_commits("d=1 t=175"), None);
        assert_eq!(parse_commits("garbage"), None);
    }

    #[test]
    fn filters_stopwords_single_chars_and_noise() {
        let raw = vec![
            ("的".to_string(), 216),
            ("是".to_string(), 95),
            ("文件夹".to_string(), 62),
            ("吗z".to_string(), 3),
            ("㕢ㄕ".to_string(), 1),
            ("一段特别长的误学整句子内容".to_string(), 2),
            ("trans".to_string(), 10),
            ("零".to_string(), 0),
        ];
        let ranked = rank(raw);
        let words: Vec<&str> = ranked.iter().map(|(w, _)| w.as_str()).collect();
        assert_eq!(words, vec!["文件夹", "trans"], "只剩真词，按频次降序");
    }

    #[test]
    fn size_uses_log_and_stays_in_range() {
        // 频次差 100 倍，字号不该差 100 倍
        let big = size_for(100, 1, 100);
        let small = size_for(1, 1, 100);
        assert_eq!(big, SIZE_MAX.round());
        assert_eq!(small, SIZE_MIN.round());
        let mid = size_for(10, 1, 100);
        assert!(mid > small && mid < big, "10 次的字号要落在中间");
        assert!(mid <= SIZE_MIN + (SIZE_MAX - SIZE_MIN) * 0.6);
    }

    #[test]
    fn tint_follows_rank_not_absolute_count() {
        assert_eq!(tint_for(0, 100), 0, "第一名最深");
        assert_eq!(tint_for(99, 100), 4, "末位最浅");
    }

    #[test]
    fn ellipse_mask_excludes_corners() {
        let g = Grid::new(400.0, 200.0, 2.0);
        assert!(!g.allow_at(2.0, 2.0), "左上角在椭圆外");
        assert!(g.allow_at(200.0, 100.0), "中心在椭圆内");
    }

    #[test]
    fn layout_never_overlaps_and_fits_canvas() {
        let words: Vec<(String, u64)> = [
            ("你好", 139),
            ("文件夹", 62),
            ("方案", 36),
            ("打包", 33),
            ("识别", 28),
            ("界面", 19),
            ("部署", 20),
            ("统计", 18),
            ("速度", 12),
            ("压缩包", 9),
        ]
        .iter()
        .map(|(w, c)| (w.to_string(), *c))
        .collect();
        let items = layout(&words);
        assert!(!items.is_empty());

        for it in &items {
            assert!(it.x as f32 >= 0.0 && it.y as f32 >= 0.0);
            assert!(
                it.x as f32 + it.w as f32 <= CANVAS_W + 1.0,
                "{} 右边越界：x={} w={}",
                it.word,
                it.x,
                it.w
            );
            assert!(
                it.y as f32 + it.size as f32 <= CANVAS_H + 1.0,
                "{} 下面越界",
                it.word
            );
        }
        for (i, a) in items.iter().enumerate() {
            for b in &items[i + 1..] {
                let overlap = a.x < b.x + b.w
                    && b.x < a.x + a.w
                    && a.y < b.y + b.size
                    && b.y < a.y + a.size;
                assert!(!overlap, "「{}」和「{}」重叠了", a.word, b.word);
            }
        }
    }

    #[test]
    fn layout_is_deterministic() {
        let words: Vec<(String, u64)> = (0..20)
            .map(|i| (format!("词{i}号"), (100 - i) as u64))
            .collect();
        let a = layout(&words);
        let b = layout(&words);
        assert_eq!(a, b, "同样的输入必须给出同样的图，否则每次打开都在跳");
    }
}
