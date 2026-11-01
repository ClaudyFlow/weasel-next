//! 按键延迟采样（乙-1 插桩）。
//!
//! # 为什么需要它
//!
//! TSF 的 `ITfKeyEventSink::OnKeyDown` 跑在**应用的 UI 线程**上，
//! [`crate::rpc_worker::RpcWorker::process_key_event`] 会在这个线程上同步等待引擎应答。
//! 于是「引擎算多久」＝「应用卡多久」，用户感觉到的打字卡顿就是这个数字。
//! 光在引擎里打时间戳量不出端到端（还有队列、RPC、序列化），所以在这里量。
//!
//! # 开销
//!
//! 未开启时只有一次 `AtomicBool` 读（几纳秒）。开启后每键一次 `Instant::now()`
//! 加一次无竞争的 `Mutex` 写入，攒够 [`FLUSH_EVERY`] 条才落一次盘。
//!
//! # 开关
//!
//! 在用户目录放一个空文件 `key-latency.on`（`%APPDATA%\Rime-Next\key-latency.on`）
//! 即可开启，删掉即关闭；最多 [`RECHECK_INTERVAL`] 重新 stat 一次，不做每键系统调用。
//! 也可以设环境变量 `WEASEL_KEY_LATENCY=1`（供自动化测试用）。
//!
//! # 产出
//!
//! `%LOCALAPPDATA%\Rime-Next\Logs\key-latency.csv`，列：
//! `ms_since_start,wait_us,budget_ms,outcome,eaten`
//! * `outcome`：`ok` / `timeout` / `disconnect`
//! * `wait_us`：从入队到拿到应答（或放弃）的墙钟微秒

use std::fmt::Write as _;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 攒够多少条落一次盘。
const FLUSH_EVERY: usize = 256;
/// 重新检查开关文件的间隔（每键都 stat 太贵）。
const RECHECK_INTERVAL: Duration = Duration::from_secs(2);

/// 一条采样。故意保持得小（16 字节），环形缓冲是定长数组。
#[derive(Clone, Copy)]
struct Sample {
    /// 距进程启动的毫秒（采样时刻就取好，不等落盘时才算）。
    at_ms: u32,
    wait_us: u32,
    budget_ms: u16,
    /// 0=ok 1=timeout 2=disconnect
    outcome: u8,
    eaten: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    Ok,
    Timeout,
    Disconnect,
    /// 整段 TSF 回调的耗时（[`probe`] 记的），不是 RPC 等待。
    Total,
}

impl Outcome {
    fn as_u8(self) -> u8 {
        match self {
            Outcome::Ok => 0,
            Outcome::Timeout => 1,
            Outcome::Disconnect => 2,
            Outcome::Total => 3,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Timeout => "timeout",
            Outcome::Disconnect => "disconnect",
            Outcome::Total => "total",
        }
    }
}

struct Buffer {
    /// 进程启动时刻，CSV 首列用它算相对时间。
    start: Instant,
    enabled_at: AtomicBool,
    last_check: Mutex<Instant>,
    samples: Mutex<Vec<Sample>>,
    /// 落盘失败后就别再试了（例如目录不可写）。
    write_failed: AtomicBool,
    /// 已落盘的行数，只用于诊断。
    flushed: AtomicU64,
}

impl Buffer {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            enabled_at: AtomicBool::new(false),
            // 让第一次调用一定去检查一次
            last_check: Mutex::new(Instant::now() - RECHECK_INTERVAL),
            samples: Mutex::new(Vec::with_capacity(FLUSH_EVERY)),
            write_failed: AtomicBool::new(false),
            flushed: AtomicU64::new(0),
        }
    }

    /// 开关是否打开（带 2 秒缓存）。
    fn enabled(&self) -> bool {
        if self.write_failed.load(Ordering::Relaxed) {
            return false;
        }
        if std::env::var_os("WEASEL_KEY_LATENCY").is_some() {
            return true;
        }
        // 先看缓存
        if self.enabled_at.load(Ordering::Relaxed) {
            return true;
        }
        let mut last = match self.last_check.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if last.elapsed() < RECHECK_INTERVAL {
            return false;
        }
        *last = Instant::now();
        let on = flag_path().map(|path| path.exists()).unwrap_or(false);
        if on {
            self.enabled_at.store(true, Ordering::Relaxed);
        }
        on
    }

    fn push(&self, sample: Sample) {
        let mut flush = None;
        {
            let mut guard = match self.samples.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.push(sample);
            if guard.len() >= FLUSH_EVERY {
                flush = Some(std::mem::take(&mut *guard));
            }
        }
        if let Some(batch) = flush {
            self.flush(&batch);
        }
    }

    fn flush(&self, batch: &[Sample]) {
        let Some(path) = log_path() else {
            self.write_failed.store(true, Ordering::Relaxed);
            return;
        };
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                let _ = error;
                self.write_failed.store(true, Ordering::Relaxed);
                return;
            }
        }
        let mut text = String::with_capacity(batch.len() * 40);
        for sample in batch {
            let _ = writeln!(
                text,
                "{},{},{},{},{}",
                sample.at_ms,
                sample.wait_us,
                sample.budget_ms,
                Outcome::from_u8(sample.outcome).as_str(),
                if sample.eaten { 1 } else { 0 }
            );
        }
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| file.write_all(text.as_bytes()))
        {
            Ok(()) => {
                self.flushed
                    .fetch_add(batch.len() as u64, Ordering::Relaxed);
            }
            Err(_) => self.write_failed.store(true, Ordering::Relaxed),
        }
    }
}

impl Outcome {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Outcome::Timeout,
            2 => Outcome::Disconnect,
            3 => Outcome::Total,
            _ => Outcome::Ok,
        }
    }
}

fn buffer() -> &'static Buffer {
    static BUFFER: OnceLock<Buffer> = OnceLock::new();
    BUFFER.get_or_init(Buffer::new)
}

/// 开关文件路径：`%APPDATA%\Rime-Next\key-latency.on`。
fn flag_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from)?;
    Some(appdata.join("Rime-Next").join("key-latency.on"))
}

/// 输出 CSV：`%LOCALAPPDATA%\Rime-Next\Logs\key-latency.csv`。
fn log_path() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    Some(local.join("Rime-Next").join("Logs").join("key-latency.csv"))
}

/// 在 `process_key_event` 的等待前调一次；未开启采样时得到的是一个**空壳**，
/// `finish` 什么也不做，所以调用方不必到处判空。
pub fn scoped(budget: Duration) -> Scope {
    if !buffer().enabled() {
        return Scope::disabled();
    }
    Scope {
        begin: Some(Instant::now()),
        budget_ms: budget.as_millis().min(u16::MAX as u128) as u16,
    }
}

/// 量一个不经过 RPC 的区间（整段 TSF 回调）。
///
/// 和 [`scoped`] 写入同一个 CSV，靠 `outcome=total` 区分。
pub fn probe() -> Scope {
    if !buffer().enabled() {
        return Scope::disabled();
    }
    Scope {
        begin: Some(Instant::now()),
        budget_ms: 0,
    }
}

/// 一次按键等待的计时器。
pub struct Scope {
    /// `None` = 采样未开启，[`Scope::finish`] 直接返回。
    begin: Option<Instant>,
    budget_ms: u16,
}

impl Scope {
    fn disabled() -> Self {
        Self {
            begin: None,
            budget_ms: 0,
        }
    }

    /// 结束计时并采样。`eaten` 记的是这一键最终有没有被输入法吃下。
    pub fn finish(self, outcome: Outcome, eaten: bool) {
        let Some(begin) = self.begin else {
            return;
        };
        let wait = begin.elapsed();
        let at_ms = (buffer().start.elapsed().as_millis().min(u32::MAX as u128)) as u32;
        buffer().push(Sample {
            at_ms,
            wait_us: wait.as_micros().min(u32::MAX as u128) as u32,
            budget_ms: self.budget_ms,
            outcome: outcome.as_u8(),
            eaten,
        });
    }
}

/// 进程退出前把残留的采样刷出去（尽力而为，失败静默）。
pub fn flush_pending() {
    let batch = {
        let mut guard = match buffer().samples.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.is_empty() {
            return;
        }
        std::mem::take(&mut *guard)
    };
    buffer().flush(&batch);
}

/// 已落盘条数（诊断/测试用）。
#[allow(dead_code)]
pub fn flushed_count() -> u64 {
    buffer().flushed.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_roundtrip() {
        for value in [0u8, 1, 2, 99] {
            let outcome = Outcome::from_u8(value);
            assert_eq!(outcome.as_u8(), Outcome::from_u8(value).as_u8());
        }
        assert_eq!(Outcome::from_u8(1), Outcome::Timeout);
        assert_eq!(Outcome::from_u8(2), Outcome::Disconnect);
        assert_eq!(Outcome::from_u8(0), Outcome::Ok);
    }

    #[test]
    fn sample_size_is_small() {
        assert!(std::mem::size_of::<Sample>() <= 16);
    }
}
