//! 输入统计采集：按键数 + 上屏字数 + 有效时长 + 常用字，**单个二进制文件**，按天累加。
//!
//! 文件：`%APPDATA%\Rime-Next\stat\stat.bin`（注意 `stat` 单数）
//!
//! ## 格式（小端，v3）
//! ```text
//! 头 16 字节
//!   0..4    magic   = "WNST"
//!   4..8    version = 3u32
//!   8..12   days    = 天记录条数 u32
//!   12..16  chars   = 字记录条数 u32
//! 天记录 36 字节 × days（按 date 升序）
//!   0..4    date           = YYYYMMDD 的 u32
//!   4..12   keys           = u64  当天击键
//!   12..20  chars          = u64  当天上屏字数
//!   20..28  active_ms      = u64  当天有效输入毫秒（按键间隔 >5 秒算中断，不计）
//!   28..32  effective_keys = u32  Σ编辑量 = Σ按键 − 2×退格（负值收 0，见下）
//!   32..36  commits        = u32  当天上屏轮数（"一轮" = 一次上屏）
//! 字记录 12 字节 × chars（按次数降序，最多 [`KEEP_CHARS`] 个）
//!   0..4    codepoint = Unicode 标量值 u32
//!   4..12   count     = u64  该字累计上屏次数
//! ```
//!
//! ### 版本兼容（写侧与读侧必须一致）
//! * v3 = 现在的写法：36 字节天记录，多出 `effective_keys` / `commits`，为的是让
//!   「今日键准 = 有效按键 ÷ 击键」「今日平均词长 = 今日字数 ÷ 上屏轮数」有**当日**数据
//!   （尾部直方图是累计的，算不出今日）。
//! * v2（28 字节天记录、有字表）：**照样能读**，读出来 `effective_keys` 按「全有效」补齐
//!   （= keys）、`commits` 补 0，下次落盘自动升级成 v3 —— 老文件不会丢，只是老日子
//!   没有真正的编辑量/轮数。
//! * v1（20 字节天记录、没有时长与字表）：照样能读，读到就当 v2 的前三个字段用。
//! * **不认识的版本（>3）一律拒绝写入**：宁可不落盘（计数留在内存里下次重试），
//!   也不能按"空文件"覆盖掉别人的新格式 —— 这一条是防"旧进程把新文件清空"的护栏。
//!
//! ### effective_keys 的口径
//! 冰雪 rime-snow-pinyin 的 `docs/statistics.md`：键准 = 有效按键 ÷ 按键，
//! 有效按键 = 按键数 − 2×退格数（每个普通键编辑量 +1、退格 −1，
//! Σ编辑量 = Σnj − 2×退格）。这里按**每次落盘的那个窗口**算：窗口内按键数减去
//! 2×窗口内退格数，再累加到当天；窗口内可能为负（退格多），累加时收在 `0..=u32::MAX`。
//!
//! ## 读侧要能打开（重要）
//!
//! 打开时 `dwShareMode = FILE_SHARE_READ`：
//! * **能读** —— `Weasel-Next-Setting.exe /stat` 是另一个进程，靠它取数据；
//! * **不能写、不能删** —— 别人要写/删都得先拿到 `FILE_SHARE_WRITE` / `FILE_SHARE_DELETE`，
//!   这两样**没给**，所以输入法在跑的时候文件删不掉（`DeleteFile` 拿
//!   `ERROR_SHARING_VIOLATION`）；这是用户明确要求的「防删」。
//!   句柄存在静态 `Mutex` 里直到进程退出。
//!
//! ⚠️ 别把共享模式改回 `0`：那样连 `/stat` 都打不开文件（共享模式由**先打开的一方**决定，
//! 后开的一方请求多大都没用），统计窗口会永远显示 0。
//!
//! ## 不阻塞按键
//!
//! 计数全是 `AtomicU64`，攒够 `FLUSH_EVERY_COMMITS` 次上屏才落一次盘；失败静默并还回计数。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

type Handle = isize;
const INVALID_HANDLE_VALUE: Handle = -1;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const OPEN_ALWAYS: u32 = 4;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
const FILE_BEGIN: u32 = 0;

unsafe extern "system" {
    fn CreateFileW(name: *const u16, access: u32, share: u32, security: *mut core::ffi::c_void, disposition: u32, flags: u32, template: Handle) -> Handle;
    fn ReadFile(file: Handle, buffer: *mut u8, bytes: u32, read: *mut u32, overlapped: *mut core::ffi::c_void) -> i32;
    fn WriteFile(file: Handle, buffer: *const u8, bytes: u32, written: *mut u32, overlapped: *mut core::ffi::c_void) -> i32;
    fn SetFilePointer(file: Handle, distance: i32, high: *mut i32, method: u32) -> u32;
    fn SetEndOfFile(file: Handle) -> i32;
    fn CloseHandle(object: Handle) -> i32;
    fn GetTickCount64() -> u64;
}

static KEYS: AtomicU64 = AtomicU64::new(0);
static CHARS: AtomicU64 = AtomicU64::new(0);
static ACTIVE_MS: AtomicU64 = AtomicU64::new(0);
/// 上一个按键的 tick；0 = 还没按过。只由 `record_key` 读写。
static LAST_KEY_TICK: AtomicU64 = AtomicU64::new(0);
static PENDING_COMMITS: AtomicU64 = AtomicU64::new(0);

/// 键盘与码长直方图（尾部扩展区的来源）。
struct KeyHist {
    /// `keys[vk]`：这个虚拟键按了多少次。
    keys: [u32; VK_LEN],
    /// `codelen[n]`：这一轮上屏一共敲了 n 键的次数。
    codelen: [u32; CODELEN_LEN],
    /// 本轮还没上屏的按键数（上屏时折算成码长后清零）。
    since_commit: u32,
    /// 今日 24 小时击键桶（时段统计，手机屏幕使用时间样式）。
    hour_keys: [u32; HOUR_LEN],
    /// 今日 24 小时有效毫秒桶（与 `active_ms` 同口径，按键间隔 >5 秒不计）。
    hour_active_ms: [u64; HOUR_LEN],
}
static KEY_HIST: OnceLock<Mutex<KeyHist>> = OnceLock::new();
fn key_hist() -> &'static Mutex<KeyHist> {
    KEY_HIST.get_or_init(|| {
        Mutex::new(KeyHist {
            keys: [0; VK_LEN],
            codelen: [0; CODELEN_LEN],
            since_commit: 0,
            hour_keys: [0; HOUR_LEN],
            hour_active_ms: [0; HOUR_LEN],
        })
    })
}

/// 跨日检测：日期变了就把小时桶清零（桶只描述"今天"）。
static LAST_HIST_DATE: AtomicU32 = AtomicU32::new(0);

/// 本地当前小时（0..=23）。
fn local_hour() -> usize {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime { year: u16, month: u16, day_of_week: u16, day: u16, hour: u16, minute: u16, second: u16, milliseconds: u16 }
    unsafe extern "system" { fn GetLocalTime(time: *mut SystemTime); }
    let mut now = SystemTime::default();
    unsafe { GetLocalTime(&mut now) };
    (now.hour as usize).min(HOUR_LEN - 1)
}

/// 攒够这么多次上屏就落一次盘。
///
/// 2026-10-26 用户要求从 20 降到 **5**：「每次按刷新键能及时计算今天的数据」——
/// 设置界面的刷新是整份重读 `stat.bin`，所以界面能看到的"最新"取决于这个阈值：
/// 20 次一轮意味着刚打完十几个词去按刷新，看到的还是「数据无变化」。
/// 代价：写盘次数 ×4（每次只写几 KB），崩溃/断电最多丢 5 轮上屏。
const FLUSH_EVERY_COMMITS: u64 = 5;
/// 按键间隔超过它就当「中断」，这一段不计入有效时长（与 lua 版 `gapThd = 5` 一致）。
const ACTIVE_GAP_MS: u64 = 5_000;
const HEADER_LEN: usize = 16;
const DAY_LEN_V1: usize = 20;
const DAY_LEN_V2: usize = 28;
/// v3：v2 的天记录 + `effective_keys: u32` + `commits: u32`。
const DAY_LEN_V3: usize = 36;
/// v4：v3 天记录 + 24×u32 当日逐时键数(时段图跨日历史,用户 2026-11-07)。
const DAY_LEN_V4: usize = 36 + 96;
const CHAR_LEN: usize = 12;
/// 键盘热力图：每个虚拟键一个 u32 计数（vk 0..=255）。
const VK_LEN: usize = 256;
/// `VK_BACK`（退格）在热力图里的下标：键准要扣掉它。
const VK_BACK: usize = 0x08;
/// 码长直方图：一轮上屏敲了 0..=32 键各多少次。
const CODELEN_LEN: usize = 33;
/// 时段统计桶数：一天 24 小时，每小时一格（手机屏幕使用时间样式）。
const HOUR_LEN: usize = 24;
/// 尾部扩展区（追加在字表之后）。
///
/// **为什么不升版本号**：旧读取器读到「字表」就停下，后面的字节它根本不看，
/// 所以把新数据追加在尾部不会让老版本的 `/stat` 页面报错 —— 升级是单向安全的。
/// 新读取器按「字表之后是否还有 `EXT_LEN` 字节」判断这份文件有没有扩展数据。
const EXT_LEN: usize = (VK_LEN + CODELEN_LEN) * 4;
/// 时段桶扩展区（追加在键盘/码长直方图之后）：每小时 `u32 keys + u64 active_ms`。
const HOUR_EXT_LEN: usize = HOUR_LEN * (4 + 8);
/// 字频表最多存多少个字（一天一天累积，十年也不会撑爆文件）。
const KEEP_CHARS: usize = 200;
const MAGIC: [u8; 4] = *b"WNST";
/// 落盘用的版本号（读侧认 1/2/3）。
const VERSION: u32 = 4;

/// 版本 → 天记录长度；不认识的版本给 `None`（读侧据此拒绝覆盖文件）。
fn day_len_of(version: u32) -> Option<usize> {
    match version {
        1 => Some(DAY_LEN_V1),
        2 => Some(DAY_LEN_V2),
        3 => Some(DAY_LEN_V3),
        4 => Some(DAY_LEN_V4),
        _ => None,
    }
}

/// 待落盘的当天字频。落盘成功后清空；失败则原样还回去。
static CHAR_TABLE: OnceLock<Mutex<HashMap<char, u64>>> = OnceLock::new();
fn char_table() -> &'static Mutex<HashMap<char, u64>> {
    CHAR_TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

static LOCKED: OnceLock<Mutex<Option<Handle>>> = OnceLock::new();
fn locked() -> &'static Mutex<Option<Handle>> { LOCKED.get_or_init(|| Mutex::new(None)) }

/// 一个按键。顺手累计「有效输入时长」：与上一键间隔 ≤5 秒的那段算在打字。
///
/// `keycode` 用来累计**每个虚拟键的次数**（键盘热力图），只收 0..=255；
/// 其它值（如 0xFFE3 这类扩展键码）仍计入总击键数，只是不进热力图。
#[inline]
pub fn record_key(keycode: i64) {
    KEYS.fetch_add(1, Ordering::Relaxed);
    let now = unsafe { GetTickCount64() };
    let last = LAST_KEY_TICK.swap(now, Ordering::Relaxed);
    let mut gap_ms = 0u64;
    if last != 0 && now > last {
        let gap = now - last;
        if gap <= ACTIVE_GAP_MS {
            ACTIVE_MS.fetch_add(gap, Ordering::Relaxed);
            gap_ms = gap;
        }
    }
    if let Ok(mut hist) = key_hist().lock() {
        // 跨日清零：小时桶只描述"今天"（日期从文件侧拿不到，这里自己记）。
        let today = today_code();
        if LAST_HIST_DATE.swap(today, Ordering::Relaxed) != today {
            hist.hour_keys = [0; HOUR_LEN];
            hist.hour_active_ms = [0; HOUR_LEN];
        }
        let hour = local_hour();
        let slot = &mut hist.hour_keys[hour];
        *slot = slot.saturating_add(1);
        hist.hour_active_ms[hour] = hist.hour_active_ms[hour].saturating_add(gap_ms);
        hist.since_commit = hist.since_commit.saturating_add(1);
        if keycode >= 0 && (keycode as usize) < VK_LEN {
            let slot = &mut hist.keys[keycode as usize];
            *slot = slot.saturating_add(1);
        }
    }
}

#[inline]
pub fn record_commit(text: &str) {
    let count = text.chars().count() as u64;
    if count == 0 { return; }
    CHARS.fetch_add(count, Ordering::Relaxed);
    if let Ok(mut table) = char_table().lock() {
        for ch in text.chars() {
            if ch.is_control() { continue; }
            *table.entry(ch).or_insert(0) += 1;
        }
    }
    // 码长：这一轮上屏一共敲了几键（含选字键，口径与插件的 `code_len` 一致）。
    if let Ok(mut hist) = key_hist().lock() {
        let len = (hist.since_commit as usize).min(CODELEN_LEN - 1);
        let slot = &mut hist.codelen[len];
        *slot = slot.saturating_add(1);
        hist.since_commit = 0;
    }
    let pending = PENDING_COMMITS.fetch_add(1, Ordering::Relaxed) + 1;
    if pending >= FLUSH_EVERY_COMMITS {
        PENDING_COMMITS.store(0, Ordering::Relaxed);
        flush();
    }
}

pub fn flush() -> Option<()> {
    let keys = KEYS.swap(0, Ordering::Relaxed);
    let chars = CHARS.swap(0, Ordering::Relaxed);
    let active = ACTIVE_MS.swap(0, Ordering::Relaxed);
    let pending_words: Vec<(char, u64)> = match char_table().lock() {
        Ok(mut table) => table.drain().collect(),
        Err(_) => Vec::new(),
    };
    let (key_counts, code_lens, hour_keys, hour_active_ms) = take_hist();
    let no_hist = key_counts.iter().all(|n| *n == 0) && code_lens.iter().all(|n| *n == 0);
    if keys == 0
        && chars == 0
        && active == 0
        && pending_words.is_empty()
        && no_hist
        && hour_keys.iter().all(|n| *n == 0)
    {
        return Some(());
    }

    let Some(path) = stat_path() else {
        put_hist(key_counts, code_lens, hour_keys, hour_active_ms);
        return restore(keys, chars, active, pending_words);
    };
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }

    let guard = locked();
    let mut slot = guard.lock().ok()?;

    if slot.is_none() {
        match open_shared_read(&path) {
            Some(handle) => *slot = Some(handle),
            None => {
                put_hist(key_counts, code_lens, hour_keys, hour_active_ms);
                return restore(keys, chars, active, pending_words);
            }
        }
    }
    let handle = match *slot {
        Some(handle) => handle,
        None => {
            put_hist(key_counts, code_lens, hour_keys, hour_active_ms);
            return restore(keys, chars, active, pending_words);
        }
    };

    let today = today_code();
    // 读不出来/版本不认识就**不写**（把计数还回去）：宁可晚一点落盘，也不能拿空文件覆盖掉
    // 一个还有历史记录的文件（旧版本 TIP 就是这样把 v3 文件清空的）。
    let Some(mut file) = read_file(handle) else {
        put_hist(key_counts, code_lens, hour_keys, hour_active_ms);
        return restore(keys, chars, active, pending_words);
    };
    // v3 的当日指标：这一轮 flush 窗口里的编辑量/上屏轮数。
    // 两者都能从同一窗口的直方图直接得出（窗口内每次上屏都往 `code_lens` 加 1，
    // 所以轮数 = 直方图之和；退格数 = 热力图里 VK_BACK 那一桶）。
    let effective = keys as i64 - 2 * i64::from(key_counts[VK_BACK]);
    let commits: u64 = code_lens.iter().map(|count| u64::from(*count)).sum();
    match file.days.iter_mut().find(|record| record.date == today) {
        Some(record) => {
            record.keys = record.keys.saturating_add(keys);
            record.chars = record.chars.saturating_add(chars);
            record.active_ms = record.active_ms.saturating_add(active);
            record.effective_keys = merge_effective(record.effective_keys, effective);
            record.commits = record.commits.saturating_add(clamp_u32(commits));
        }
        None => file.days.push(DayRecord {
            date: today,
            keys,
            chars,
            active_ms: active,
            effective_keys: merge_effective(0, effective),
            commits: clamp_u32(commits),
            hours: hour_keys,
        }),
    }
    file.days.sort_by_key(|record| record.date);
    for (ch, count) in &pending_words {
        *file.chars.entry(*ch).or_insert(0) += count;
    }
    for (index, count) in key_counts.iter().enumerate() {
        file.key_counts[index] = file.key_counts[index].saturating_add(*count);
    }
    for (index, count) in code_lens.iter().enumerate() {
        file.code_lens[index] = file.code_lens[index].saturating_add(*count);
    }
    // 小时桶只描述"今天":文件里的桶若来自别的日期,以本窗口的桶为起点清零重算。
    // （flush 窗口里的按键一定发生在"现在",文件里跨日的旧桶已由 record_key 侧清零。）
    for (index, count) in hour_keys.iter().enumerate() {
        file.hour_keys[index] = file.hour_keys[index].saturating_add(*count);
    }
    for (index, ms) in hour_active_ms.iter().enumerate() {
        file.hour_active_ms[index] = file.hour_active_ms[index].saturating_add(*ms);
    }

    if !write_file(handle, &file) {
        put_hist(key_counts, code_lens, hour_keys, hour_active_ms);
        return restore(keys, chars, active, pending_words);
    }
    Some(())
}

/// 取走待落盘的直方图与小时桶（失败时由 [`put_hist`] 还回去）。
fn take_hist() -> ([u32; VK_LEN], [u32; CODELEN_LEN], [u32; HOUR_LEN], [u64; HOUR_LEN]) {
    match key_hist().lock() {
        Ok(mut hist) => (
            std::mem::replace(&mut hist.keys, [0; VK_LEN]),
            std::mem::replace(&mut hist.codelen, [0; CODELEN_LEN]),
            std::mem::replace(&mut hist.hour_keys, [0; HOUR_LEN]),
            std::mem::replace(&mut hist.hour_active_ms, [0; HOUR_LEN]),
        ),
        Err(_) => ([0; VK_LEN], [0; CODELEN_LEN], [0; HOUR_LEN], [0; HOUR_LEN]),
    }
}

fn put_hist(
    keys: [u32; VK_LEN],
    code_lens: [u32; CODELEN_LEN],
    hour_keys: [u32; HOUR_LEN],
    hour_active_ms: [u64; HOUR_LEN],
) {
    if let Ok(mut hist) = key_hist().lock() {
        for (index, count) in keys.iter().enumerate() {
            hist.keys[index] = hist.keys[index].saturating_add(*count);
        }
        for (index, count) in code_lens.iter().enumerate() {
            hist.codelen[index] = hist.codelen[index].saturating_add(*count);
        }
        for (index, count) in hour_keys.iter().enumerate() {
            hist.hour_keys[index] = hist.hour_keys[index].saturating_add(*count);
        }
        for (index, ms) in hour_active_ms.iter().enumerate() {
            hist.hour_active_ms[index] = hist.hour_active_ms[index].saturating_add(*ms);
        }
    }
}

/// 落盘失败：把计数还回去，下一个 flush 再试。宁可晚一点写，不能丢数字。
fn restore(keys: u64, chars: u64, active: u64, words: Vec<(char, u64)>) -> Option<()> {
    KEYS.fetch_add(keys, Ordering::Relaxed);
    CHARS.fetch_add(chars, Ordering::Relaxed);
    ACTIVE_MS.fetch_add(active, Ordering::Relaxed);
    if let Ok(mut table) = char_table().lock() {
        for (ch, count) in words {
            *table.entry(ch).or_insert(0) += count;
        }
    }
    None
}

/// 把这一轮 flush 的「Σ编辑量」并进当天的累计值。
///
/// `delta` 可能为负（退格多于普通键），所以按有符号算、再收在 `0..=u32::MAX`：
/// 一天里累计为负只可能是"今天几乎全在退格"，那就当 0（键准 0%），不写成负数。
fn merge_effective(current: u32, delta: i64) -> u32 {
    (i64::from(current) + delta).clamp(0, i64::from(u32::MAX)) as u32
}

/// `u64` 计数收进 `u32`（溢出只会发生在天文数字上，收在最大值即可）。
fn clamp_u32(value: u64) -> u32 {
    value.min(u64::from(u32::MAX)) as u32
}

fn stat_path() -> Option<std::path::PathBuf> {
    Some(weasel_common::process::user_data_dir()?.join("stat").join("stat.bin"))
}

fn open_shared_read(path: &std::path::Path) -> Option<Handle> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let handle = unsafe {
        CreateFileW(wide.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ, std::ptr::null_mut(), OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, 0)
    };
    if handle == INVALID_HANDLE_VALUE || handle == 0 { None } else { Some(handle) }
}

/// 一条天记录（v1 文件读出来 `active_ms` 是 0；v1/v2 文件读出来 `effective_keys`/`commits` 见
/// [`parse_file`] 的补齐规则）。
struct DayRecord {
    date: u32,
    keys: u64,
    chars: u64,
    active_ms: u64,
    /// Σ编辑量（Σ按键 − 2×退格），负值收 0；v3 起才有真正的当日值。
    effective_keys: u32,
    /// 当天上屏轮数；v3 起才有真正的当日值。
    commits: u32,
    /// 当日逐时键数(v4 起落盘;旧文件读入为 0)。时段图跨日历史的数据源。
    hours: [u32; HOUR_LEN],
}

/// 磁盘上的全部内容。
struct StatFile {
    days: Vec<DayRecord>,
    chars: HashMap<char, u64>,
    /// 键盘热力图：`key_counts[vk]`。
    key_counts: [u32; VK_LEN],
    /// 码长直方图：`code_lens[一轮敲了几键]`。
    code_lens: [u32; CODELEN_LEN],
    /// 时段统计：今天每小时的击键数（旧文件没有就是 0）。
    hour_keys: [u32; HOUR_LEN],
    /// 时段统计：今天每小时的有效毫秒（旧文件没有就是 0）。
    hour_active_ms: [u64; HOUR_LEN],
}

fn empty_stat() -> StatFile {
    StatFile {
        days: Vec::new(),
        chars: HashMap::new(),
        hour_keys: [0; HOUR_LEN],
        hour_active_ms: [0; HOUR_LEN],
        key_counts: [0; VK_LEN],
        code_lens: [0; CODELEN_LEN],
    }
}

/// 读整份文件。`None` = **不要写**（读失败、或版本比本程序新）。
fn read_file(handle: Handle) -> Option<StatFile> {
    unsafe { SetFilePointer(handle, 0, std::ptr::null_mut(), FILE_BEGIN) };
    let mut buffer = vec![0u8; 256 * 1024];
    let mut read = 0u32;
    let ok = unsafe { ReadFile(handle, buffer.as_mut_ptr(), buffer.len() as u32, &mut read, std::ptr::null_mut()) };
    if ok == 0 { return None; }
    buffer.truncate(read as usize);
    parse_file(&buffer)
}

/// 纯函数：磁盘字节 → 结构。
///
/// * 空文件/头不完整 → 空结构（新文件，正常）；
/// * 坏魔数、**不认识的版本** → `None`：调用方必须放弃这次落盘，不能覆盖别人的文件；
/// * 记录被截断 → 只交出已经完整的部分（写侧自己是"截断后重写"，可能读在半截上）。
fn parse_file(buffer: &[u8]) -> Option<StatFile> {
    let empty = empty_stat;
    if buffer.len() < HEADER_LEN { return Some(empty()); }
    if buffer[0..4] != MAGIC { return None; }
    let version = u32::from_le_bytes(buffer[4..8].try_into().unwrap());
    let day_len = day_len_of(version)?;
    let mut file = empty();
    let count = u32::from_le_bytes(buffer[8..12].try_into().unwrap()) as usize;
    let char_count = if version >= 2 {
        u32::from_le_bytes(buffer[12..16].try_into().unwrap()) as usize
    } else {
        0
    };

    let mut at = HEADER_LEN;
    for _ in 0..count {
        let Some(chunk) = buffer.get(at..at + day_len) else { return Some(file) };
        let keys = u64::from_le_bytes(chunk[4..12].try_into().unwrap());
        let chars = u64::from_le_bytes(chunk[12..20].try_into().unwrap());
        // v2 及更早没采过这两个数：
        //   * effective_keys 按"全有效"补齐（= keys）—— 只为了让升级后**当天**的键准不被
        //     升级前那半段凭空拉低；老日子本来也不参与任何"今日"计算。
        //   * commits 补 0 —— 没有轮数就算不出词长，读侧显示 0.00，不瞎猜。
        let (effective_keys, commits) = if day_len >= DAY_LEN_V3 {
            (
                u32::from_le_bytes(chunk[28..32].try_into().unwrap()),
                u32::from_le_bytes(chunk[32..36].try_into().unwrap()),
            )
        } else {
            (clamp_u32(keys), 0)
        };
        file.days.push(DayRecord {
            date: u32::from_le_bytes(chunk[0..4].try_into().unwrap()),
            keys,
            chars,
            active_ms: if day_len >= DAY_LEN_V2 {
                u64::from_le_bytes(chunk[20..28].try_into().unwrap())
            } else {
                0
            },
            effective_keys,
            commits,
            hours: if day_len >= DAY_LEN_V4 {
                let mut hs = [0u32; HOUR_LEN];
                for (h, slot) in hs.iter_mut().enumerate() {
                    let off = 36 + h * 4;
                    *slot = u32::from_le_bytes(chunk[off..off + 4].try_into().unwrap());
                }
                hs
            } else {
                [0; HOUR_LEN]
            },
        });
        at += day_len;
    }
    for _ in 0..char_count {
        let Some(chunk) = buffer.get(at..at + CHAR_LEN) else { return Some(file) };
        let code = u32::from_le_bytes(chunk[0..4].try_into().unwrap());
        let count = u64::from_le_bytes(chunk[4..12].try_into().unwrap());
        if let Some(ch) = char::from_u32(code) {
            *file.chars.entry(ch).or_insert(0) += count;
        }
        at += CHAR_LEN;
    }
    // 尾部扩展区（键盘热力图 + 码长直方图）：字表之后还有完整一段才读。
    if buffer.len() >= at + EXT_LEN {
        for index in 0..VK_LEN {
            let start = at + index * 4;
            file.key_counts[index] =
                u32::from_le_bytes(buffer[start..start + 4].try_into().unwrap());
        }
        let coded = at + VK_LEN * 4;
        for index in 0..CODELEN_LEN {
            let start = coded + index * 4;
            file.code_lens[index] =
                u32::from_le_bytes(buffer[start..start + 4].try_into().unwrap());
        }
        // 时段桶（2026-10-31）：热力图之后还有完整一段才读（老文件没有，保持 0）。
        let hours_at = at + EXT_LEN;
        if buffer.len() >= hours_at + HOUR_EXT_LEN {
            for index in 0..HOUR_LEN {
                let start = hours_at + index * 4;
                file.hour_keys[index] =
                    u32::from_le_bytes(buffer[start..start + 4].try_into().unwrap());
            }
            let ms_at = hours_at + HOUR_LEN * 4;
            for index in 0..HOUR_LEN {
                let start = ms_at + index * 8;
                file.hour_active_ms[index] =
                    u64::from_le_bytes(buffer[start..start + 8].try_into().unwrap());
            }
        }
    }
    Some(file)
}

fn write_file(handle: Handle, file: &StatFile) -> bool {
    let buffer = encode(file);
    unsafe {
        SetFilePointer(handle, 0, std::ptr::null_mut(), FILE_BEGIN);
        SetEndOfFile(handle);
        SetFilePointer(handle, 0, std::ptr::null_mut(), FILE_BEGIN);
    }
    let mut written = 0u32;
    let ok = unsafe { WriteFile(handle, buffer.as_ptr(), buffer.len() as u32, &mut written, std::ptr::null_mut()) };
    ok != 0 && (written as usize) == buffer.len()
}

/// 纯函数：结构 → 磁盘字节。与 [`parse_file`] 互为逆运算（单测里就是拿这一对做的往返）。
fn encode(file: &StatFile) -> Vec<u8> {
    let mut words: Vec<(char, u64)> = file.chars.iter().map(|(ch, count)| (*ch, *count)).collect();
    words.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    words.truncate(KEEP_CHARS);

    let mut buffer = Vec::with_capacity(HEADER_LEN + file.days.len() * DAY_LEN_V4 + words.len() * CHAR_LEN);
    buffer.extend_from_slice(&MAGIC);
    buffer.extend_from_slice(&VERSION.to_le_bytes());
    buffer.extend_from_slice(&(file.days.len() as u32).to_le_bytes());
    buffer.extend_from_slice(&(words.len() as u32).to_le_bytes());
    for record in &file.days {
        buffer.extend_from_slice(&record.date.to_le_bytes());
        buffer.extend_from_slice(&record.keys.to_le_bytes());
        buffer.extend_from_slice(&record.chars.to_le_bytes());
        buffer.extend_from_slice(&record.active_ms.to_le_bytes());
        buffer.extend_from_slice(&record.effective_keys.to_le_bytes());
        buffer.extend_from_slice(&record.commits.to_le_bytes());
        if VERSION >= 4 {
            // v4:天记录尾部带当日逐时键数(时段图跨日历史)。
            for h in &record.hours {
                buffer.extend_from_slice(&h.to_le_bytes());
            }
        }
    }
    for (ch, count) in &words {
        buffer.extend_from_slice(&(*ch as u32).to_le_bytes());
        buffer.extend_from_slice(&count.to_le_bytes());
    }
    // 尾部扩展区：固定 256 个 vk 计数 + 33 个码长计数，旧读取器读到字表就停，不受影响。
    for count in &file.key_counts {
        buffer.extend_from_slice(&count.to_le_bytes());
    }
    for count in &file.code_lens {
        buffer.extend_from_slice(&count.to_le_bytes());
    }
    // 时段桶（2026-10-31）：24 × (u32 keys + u64 active_ms)，同样追加式扩展。
    for count in &file.hour_keys {
        buffer.extend_from_slice(&count.to_le_bytes());
    }
    for ms in &file.hour_active_ms {
        buffer.extend_from_slice(&ms.to_le_bytes());
    }
    buffer
}

fn today_code() -> u32 {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime { year: u16, month: u16, day_of_week: u16, day: u16, hour: u16, minute: u16, second: u16, milliseconds: u16 }
    unsafe extern "system" { fn GetLocalTime(time: *mut SystemTime); }
    let mut now = SystemTime::default();
    unsafe { GetLocalTime(&mut now) };
    u32::from(now.year) * 10_000 + u32::from(now.month) * 100 + u32::from(now.day)
}

/// 卸载 TIP 时调用：先尽量落一次盘，再关句柄（关句柄后文件就能删了）。
pub fn release() {
    let _ = flush();
    if let Some(slot) = LOCKED.get() {
        if let Ok(mut slot) = slot.lock() {
            if let Some(handle) = slot.take() { unsafe { CloseHandle(handle) }; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_file() -> StatFile {
        let mut hour_keys = [0u32; HOUR_LEN];
        hour_keys[9] = 120;
        hour_keys[21] = 7;
        let mut hour_active_ms = [0u64; HOUR_LEN];
        hour_active_ms[9] = 480_000;
        hour_active_ms[21] = 30_000;
        StatFile {
            hour_keys,
            hour_active_ms,
            days: vec![
                DayRecord {
                    date: 20261016,
                    keys: 5,
                    chars: 2,
                    active_ms: 1_000,
                    effective_keys: 5,
                    hours: [0; HOUR_LEN],
                    commits: 1,
                },
                DayRecord {
                    date: 20261017,
                    keys: 1234,
                    chars: 567,
                    active_ms: 180_000,
                    effective_keys: 1_180,
                    commits: 321,
                    hours: [0; HOUR_LEN],
                },
            ],
            chars: HashMap::from([('的', 12), ('一', 3)]),
            key_counts: [0; VK_LEN],
            code_lens: [0; CODELEN_LEN],
        }
    }

    #[test]
    fn today_code_shape() {
        let code = today_code();
        assert!(code >= 20_000_101, "year too small: {code}");
    }

    #[test]
    fn empty_commit_ignored() {
        let before = CHARS.load(Ordering::Relaxed);
        record_commit("");
        assert_eq!(CHARS.load(Ordering::Relaxed), before);
    }

    /// 写出来的字节必须能被同一个解析器原样读回来 —— 这是两边唯一的契约。
    #[test]
    fn encode_then_parse_roundtrips() {
        let file = scratch_file();
        let bytes = encode(&file);
        assert_eq!(
        bytes.len(),
        HEADER_LEN + 2 * DAY_LEN_V4 + 2 * CHAR_LEN + EXT_LEN + HOUR_EXT_LEN,
    );
        // 头里的版本必须是 4：读侧（Setting）认 2/3/4,写侧只写 4。
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 4);
        let back = parse_file(&bytes).expect("自己的输出必须读得回来");
        // 时段桶往返:9 点与 21 点的打字量原样回来。
        assert_eq!(back.hour_keys[9], 120);
        assert_eq!(back.hour_keys[21], 7);
        assert_eq!(back.hour_active_ms[9], 480_000);
        assert_eq!(back.hour_active_ms[21], 30_000);
        assert_eq!(back.days.len(), 2);
        assert_eq!(
            (
                back.days[1].date,
                back.days[1].keys,
                back.days[1].chars,
                back.days[1].active_ms,
                back.days[1].effective_keys,
                back.days[1].commits,
            ),
            (20261017, 1234, 567, 180_000, 1_180, 321)
        );
        assert_eq!(back.chars.get(&'的'), Some(&12));
        assert_eq!(back.chars.get(&'一'), Some(&3));
    }

    /// v1（20 字节天记录、没有时长/字表）照样能读，下次落盘自动升级成 v3。
    #[test]
    fn version_one_files_still_read() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        for (date, keys, chars) in [(20261016u32, 5u64, 2u64), (20261017, 1234, 567)] {
            bytes.extend_from_slice(&date.to_le_bytes());
            bytes.extend_from_slice(&keys.to_le_bytes());
            bytes.extend_from_slice(&chars.to_le_bytes());
        }
        assert_eq!(bytes.len(), HEADER_LEN + 2 * DAY_LEN_V1);
        let back = parse_file(&bytes).expect("v1 必须能读");
        assert_eq!(back.days.len(), 2);
        assert_eq!(back.days[1].keys, 1234);
        assert_eq!(back.days[1].active_ms, 0);
        assert_eq!(back.days[1].commits, 0);
        assert_eq!(back.days[1].effective_keys, 1234, "老记录按全有效补齐");
        assert!(back.chars.is_empty());
    }

    /// v2（28 字节天记录）也要能读：编辑量按 keys 补齐、轮数补 0，写回去就是 v3。
    #[test]
    fn version_two_files_still_read_and_upgrade() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        for (date, keys, chars, active) in [(20261016u32, 5u64, 2u64, 1_000u64), (20261017, 1234, 567, 180_000)] {
            bytes.extend_from_slice(&date.to_le_bytes());
            bytes.extend_from_slice(&keys.to_le_bytes());
            bytes.extend_from_slice(&chars.to_le_bytes());
            bytes.extend_from_slice(&active.to_le_bytes());
        }
        bytes.extend_from_slice(&('的' as u32).to_le_bytes());
        bytes.extend_from_slice(&12u64.to_le_bytes());
        assert_eq!(bytes.len(), HEADER_LEN + 2 * DAY_LEN_V2 + CHAR_LEN);

        let back = parse_file(&bytes).expect("v2 必须能读");
        assert_eq!(back.days.len(), 2);
        assert_eq!((back.days[1].keys, back.days[1].chars, back.days[1].active_ms), (1234, 567, 180_000));
        assert_eq!((back.days[1].effective_keys, back.days[1].commits), (1234, 0));
        assert_eq!(back.chars.get(&'的'), Some(&12));

        // 下一次落盘 = v3：字节长度按 36 字节天记录重排，老日子不丢。
        let upgraded = encode(&back);
        assert_eq!(
            upgraded.len(),
            HEADER_LEN + 2 * DAY_LEN_V4 + CHAR_LEN + EXT_LEN + HOUR_EXT_LEN,
        );
        assert_eq!(u32::from_le_bytes(upgraded[4..8].try_into().unwrap()), VERSION);
        let again = parse_file(&upgraded).unwrap();
        assert_eq!(again.days[1].keys, 1234);
        assert_eq!(again.days[1].chars, 567);
    }

    /// 落盘过程中被读到一半（写侧先 SetEndOfFile 再写）不能 panic，只能少读到几条。
    #[test]
    fn truncated_file_reads_what_is_complete() {
        let bytes = encode(&scratch_file());
        for cut in [0usize, 3, HEADER_LEN - 1, HEADER_LEN + 1, HEADER_LEN + DAY_LEN_V4 + 5, bytes.len() - 1] {
            let back = parse_file(&bytes[..cut]).expect("头完整就该能读（只有坏魔数/未知版本才拒绝）");

            assert!(back.days.len() <= 2, "cut={cut}");
        }
        let partial = parse_file(&bytes[..HEADER_LEN + DAY_LEN_V4]).expect("头还在");
        assert_eq!(partial.days.len(), 1);
        assert_eq!(partial.days[0].date, 20261016);
    }

    /// 坏魔数/不认识的版本 → `None`（调用方据此**不写**，绝不拿空文件覆盖）。
    #[test]
    fn bad_magic_or_unknown_version_refuses_to_write() {
        let mut bytes = encode(&scratch_file());
        bytes[0] = b'X';
        assert!(parse_file(&bytes).is_none(), "坏魔数不该被当成空文件");
        let mut bytes = encode(&scratch_file());
        bytes[4..8].copy_from_slice(&99u32.to_le_bytes());
        assert!(parse_file(&bytes).is_none(), "未来版本不该被当成空文件");
        // 空文件 = 还没采集，正常；头都不完整也一样（新文件就是这个样子）。
        assert_eq!(parse_file(&[]).map(|file| file.days.len()), Some(0));
    }

    /// 字表最多留 KEEP_CHARS 个：十年也不会把文件撑大。
    #[test]
    fn char_table_is_capped() {
        let mut chars = HashMap::new();
        for code in 0x4e00u32..0x4e00 + 300 {
            chars.insert(char::from_u32(code).unwrap(), u64::from(code));
        }
        let file = StatFile {
            code_lens: [0; CODELEN_LEN],
            key_counts: [0; VK_LEN],
            hour_keys: [0; HOUR_LEN],
            hour_active_ms: [0; HOUR_LEN],
            days: vec![],
            chars,
        };
        let bytes = encode(&file);
        assert_eq!(
            bytes.len(),
            HEADER_LEN + KEEP_CHARS * CHAR_LEN + EXT_LEN + HOUR_EXT_LEN
        );
        assert_eq!(parse_file(&bytes).unwrap().chars.len(), KEEP_CHARS);
        assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), KEEP_CHARS as u32);
    }

    /// 键准口径：窗口内 Σ编辑量 = 按键 − 2×退格；累加到当天时负数收 0。
    #[test]
    fn effective_keys_follow_the_snow_formula() {
        assert_eq!(merge_effective(0, 100), 100, "没有退格 = 全有效");
        assert_eq!(merge_effective(0, 100 - 2 * 5), 90, "5 次退格扣 10");
        assert_eq!(merge_effective(10, -30), 0, "当天累计为负就收 0");
        assert_eq!(merge_effective(u32::MAX, 1), u32::MAX, "溢出收在 u32::MAX");
        assert_eq!(merge_effective(7, -7), 0);
        // 手算一遍写侧会写进天记录的两个数：300 键、其中 20 次退格、40 轮上屏。
        let keys = 300u64;
        let backspace = 20u32;
        assert_eq!(merge_effective(0, keys as i64 - 2 * i64::from(backspace)), 260);
    }
}
