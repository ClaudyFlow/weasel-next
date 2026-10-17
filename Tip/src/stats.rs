//! 输入统计采集：按键数 + 上屏字数 + 有效时长 + 常用字，**单个二进制文件**，按天累加。
//!
//! 文件：`%APPDATA%\Rime-Next\stat\stat.bin`（注意 `stat` 单数）
//!
//! ## 格式（小端，v2）
//! ```text
//! 头 16 字节
//!   0..4    magic   = "WNST"
//!   4..8    version = 2u32
//!   8..12   days    = 天记录条数 u32
//!   12..16  chars   = 字记录条数 u32
//! 天记录 28 字节 × days（按 date 升序）
//!   0..4    date      = YYYYMMDD 的 u32
//!   4..12   keys      = u64  当天击键
//!   12..20  chars     = u64  当天上屏字数
//!   20..28  active_ms = u64  当天有效输入毫秒（按键间隔 >5 秒算中断，不计）
//! 字记录 12 字节 × chars（按次数降序，最多 [`KEEP_CHARS`] 个）
//!   0..4    codepoint = Unicode 标量值 u32
//!   4..12   count     = u64  该字累计上屏次数
//! ```
//!
//! v1（20 字节天记录、没有时长与字表）仍然**能读**，读到就当 v2 的前三个字段用，
//! 下次落盘自动升级成 v2 —— 老文件不会丢。
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
use std::sync::atomic::{AtomicU64, Ordering};
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

const FLUSH_EVERY_COMMITS: u64 = 20;
/// 按键间隔超过它就当「中断」，这一段不计入有效时长（与 lua 版 `gapThd = 5` 一致）。
const ACTIVE_GAP_MS: u64 = 5_000;
const HEADER_LEN: usize = 16;
const DAY_LEN_V1: usize = 20;
const DAY_LEN_V2: usize = 28;
const CHAR_LEN: usize = 12;
/// 字频表最多存多少个字（一天一天累积，十年也不会撑爆文件）。
const KEEP_CHARS: usize = 200;
const MAGIC: [u8; 4] = *b"WNST";
const VERSION: u32 = 2;

/// 待落盘的当天字频。落盘成功后清空；失败则原样还回去。
static CHAR_TABLE: OnceLock<Mutex<HashMap<char, u64>>> = OnceLock::new();
fn char_table() -> &'static Mutex<HashMap<char, u64>> {
    CHAR_TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

static LOCKED: OnceLock<Mutex<Option<Handle>>> = OnceLock::new();
fn locked() -> &'static Mutex<Option<Handle>> { LOCKED.get_or_init(|| Mutex::new(None)) }

/// 一个按键。顺手累计「有效输入时长」：与上一键间隔 ≤5 秒的那段算在打字。
#[inline]
pub fn record_key() {
    KEYS.fetch_add(1, Ordering::Relaxed);
    let now = unsafe { GetTickCount64() };
    let last = LAST_KEY_TICK.swap(now, Ordering::Relaxed);
    if last != 0 && now > last {
        let gap = now - last;
        if gap <= ACTIVE_GAP_MS {
            ACTIVE_MS.fetch_add(gap, Ordering::Relaxed);
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
    if keys == 0 && chars == 0 && active == 0 && pending_words.is_empty() { return Some(()); }

    let Some(path) = stat_path() else { return restore(keys, chars, active, pending_words) };
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }

    let guard = locked();
    let mut slot = guard.lock().ok()?;

    if slot.is_none() {
        match open_shared_read(&path) {
            Some(handle) => *slot = Some(handle),
            None => return restore(keys, chars, active, pending_words),
        }
    }
    let handle = match *slot { Some(handle) => handle, None => return restore(keys, chars, active, pending_words) };

    let today = today_code();
    let mut file = read_file(handle);
    match file.days.iter_mut().find(|record| record.date == today) {
        Some(record) => {
            record.keys = record.keys.saturating_add(keys);
            record.chars = record.chars.saturating_add(chars);
            record.active_ms = record.active_ms.saturating_add(active);
        }
        None => file.days.push(DayRecord { date: today, keys, chars, active_ms: active }),
    }
    file.days.sort_by_key(|record| record.date);
    for (ch, count) in &pending_words {
        *file.chars.entry(*ch).or_insert(0) += count;
    }

    if !write_file(handle, &file) {
        return restore(keys, chars, active, pending_words);
    }
    Some(())
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

fn stat_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(std::path::PathBuf::from(base).join("Rime-Next").join("stat").join("stat.bin"))
}

fn open_shared_read(path: &std::path::Path) -> Option<Handle> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let handle = unsafe {
        CreateFileW(wide.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ, std::ptr::null_mut(), OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, 0)
    };
    if handle == INVALID_HANDLE_VALUE || handle == 0 { None } else { Some(handle) }
}

/// 一条天记录（v1 文件读出来 `active_ms` 是 0）。
struct DayRecord {
    date: u32,
    keys: u64,
    chars: u64,
    active_ms: u64,
}

/// 磁盘上的全部内容。
struct StatFile {
    days: Vec<DayRecord>,
    chars: HashMap<char, u64>,
}

fn read_file(handle: Handle) -> StatFile {
    unsafe { SetFilePointer(handle, 0, std::ptr::null_mut(), FILE_BEGIN) };
    let mut buffer = vec![0u8; 256 * 1024];
    let mut read = 0u32;
    let ok = unsafe { ReadFile(handle, buffer.as_mut_ptr(), buffer.len() as u32, &mut read, std::ptr::null_mut()) };
    if ok == 0 { return StatFile { days: Vec::new(), chars: HashMap::new() }; }
    buffer.truncate(read as usize);
    parse_file(&buffer)
}

/// 纯函数：磁盘字节 → 结构。截断/坏魔数/未知版本都当「空的」，不 panic。
fn parse_file(buffer: &[u8]) -> StatFile {
    let empty = || StatFile { days: Vec::new(), chars: HashMap::new() };
    if buffer.len() < HEADER_LEN { return empty(); }
    if buffer[0..4] != MAGIC { return empty(); }
    let mut file = empty();
    let version = u32::from_le_bytes(buffer[4..8].try_into().unwrap());
    if version != 1 && version != VERSION { return empty(); }
    let count = u32::from_le_bytes(buffer[8..12].try_into().unwrap()) as usize;
    let char_count = if version >= 2 {
        u32::from_le_bytes(buffer[12..16].try_into().unwrap()) as usize
    } else {
        0
    };

    let day_len = if version >= 2 { DAY_LEN_V2 } else { DAY_LEN_V1 };
    let mut at = HEADER_LEN;
    for _ in 0..count {
        let Some(chunk) = buffer.get(at..at + day_len) else { return file };
        file.days.push(DayRecord {
            date: u32::from_le_bytes(chunk[0..4].try_into().unwrap()),
            keys: u64::from_le_bytes(chunk[4..12].try_into().unwrap()),
            chars: u64::from_le_bytes(chunk[12..20].try_into().unwrap()),
            active_ms: if day_len >= DAY_LEN_V2 {
                u64::from_le_bytes(chunk[20..28].try_into().unwrap())
            } else {
                0
            },
        });
        at += day_len;
    }
    for _ in 0..char_count {
        let Some(chunk) = buffer.get(at..at + CHAR_LEN) else { return file };
        let code = u32::from_le_bytes(chunk[0..4].try_into().unwrap());
        let count = u64::from_le_bytes(chunk[4..12].try_into().unwrap());
        if let Some(ch) = char::from_u32(code) {
            *file.chars.entry(ch).or_insert(0) += count;
        }
        at += CHAR_LEN;
    }
    file
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

    let mut buffer = Vec::with_capacity(HEADER_LEN + file.days.len() * DAY_LEN_V2 + words.len() * CHAR_LEN);
    buffer.extend_from_slice(&MAGIC);
    buffer.extend_from_slice(&VERSION.to_le_bytes());
    buffer.extend_from_slice(&(file.days.len() as u32).to_le_bytes());
    buffer.extend_from_slice(&(words.len() as u32).to_le_bytes());
    for record in &file.days {
        buffer.extend_from_slice(&record.date.to_le_bytes());
        buffer.extend_from_slice(&record.keys.to_le_bytes());
        buffer.extend_from_slice(&record.chars.to_le_bytes());
        buffer.extend_from_slice(&record.active_ms.to_le_bytes());
    }
    for (ch, count) in &words {
        buffer.extend_from_slice(&(*ch as u32).to_le_bytes());
        buffer.extend_from_slice(&count.to_le_bytes());
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
        StatFile {
            days: vec![
                DayRecord { date: 20261016, keys: 5, chars: 2, active_ms: 1_000 },
                DayRecord { date: 20261017, keys: 1234, chars: 567, active_ms: 180_000 },
            ],
            chars: HashMap::from([('的', 12), ('一', 3)]),
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
        assert_eq!(bytes.len(), HEADER_LEN + 2 * DAY_LEN_V2 + 2 * CHAR_LEN);
        let back = parse_file(&bytes);
        assert_eq!(back.days.len(), 2);
        assert_eq!(
            (back.days[1].date, back.days[1].keys, back.days[1].chars, back.days[1].active_ms),
            (20261017, 1234, 567, 180_000)
        );
        assert_eq!(back.chars.get(&'的'), Some(&12));
        assert_eq!(back.chars.get(&'一'), Some(&3));
    }

    /// v1（20 字节天记录、没有时长/字表）照样能读，下次落盘自动升级成 v2。
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
        let back = parse_file(&bytes);
        assert_eq!(back.days.len(), 2);
        assert_eq!(back.days[1].keys, 1234);
        assert_eq!(back.days[1].active_ms, 0);
        assert!(back.chars.is_empty());
    }

    /// 落盘过程中被读到一半（写侧先 SetEndOfFile 再写）不能 panic，只能少读到几条。
    #[test]
    fn truncated_file_reads_what_is_complete() {
        let bytes = encode(&scratch_file());
        for cut in [0usize, 3, HEADER_LEN - 1, HEADER_LEN + 1, HEADER_LEN + DAY_LEN_V2 + 5, bytes.len() - 1] {
            let back = parse_file(&bytes[..cut]);
            assert!(back.days.len() <= 2, "cut={cut}");
        }
        let partial = parse_file(&bytes[..HEADER_LEN + DAY_LEN_V2]);
        assert_eq!(partial.days.len(), 1);
        assert_eq!(partial.days[0].date, 20261016);
    }

    #[test]
    fn bad_magic_or_version_is_empty_not_a_panic() {
        let mut bytes = encode(&scratch_file());
        bytes[0] = b'X';
        assert!(parse_file(&bytes).days.is_empty());
        let mut bytes = encode(&scratch_file());
        bytes[4..8].copy_from_slice(&99u32.to_le_bytes());
        assert!(parse_file(&bytes).days.is_empty());
        assert!(parse_file(&[]).days.is_empty());
    }

    /// 字表最多留 KEEP_CHARS 个：十年也不会把文件撑大。
    #[test]
    fn char_table_is_capped() {
        let mut chars = HashMap::new();
        for code in 0x4e00u32..0x4e00 + 300 {
            chars.insert(char::from_u32(code).unwrap(), u64::from(code));
        }
        let file = StatFile { days: vec![], chars };
        let bytes = encode(&file);
        assert_eq!(bytes.len(), HEADER_LEN + KEEP_CHARS * CHAR_LEN);
        assert_eq!(parse_file(&bytes).chars.len(), KEEP_CHARS);
        assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), KEEP_CHARS as u32);
    }
}
