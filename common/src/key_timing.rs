//! 按键耗时采样（乙-1 插桩的公共部分）。
//!
//! 三个进程各记各的，靠文件名区分，时间戳对齐后就能把"一次按键"拆成三段：
//!
//! ```text
//! 用户按键 ──► [Tip]  ──RPC──► [Engine] ──publish──► [Renderer] ──► 上屏
//!              key-latency.csv   engine-key-latency.csv   renderer-key-latency.csv
//! ```
//!
//! * Tip 那份（`Tip/src/key_latency.rs`）额外记 RPC 等待与整段 TSF 回调，
//!   因为它是**唯一跑在应用 UI 线程上**的一段，能不能跟手全看它。
//! * 这里给 Engine / Renderer 用：一个进程一个 CSV，格式相同便于对齐。
//!
//! # 开关
//!
//! `%APPDATA%\Rime-Next\key-latency.on`（放一个空文件即可）或环境变量
//! `WEASEL_KEY_LATENCY=1`。未开启时 [`record`] 只做一次原子读。
//!
//! # 产出
//!
//! `%LOCALAPPDATA%\Rime-Next\Logs\<component>-key-latency.csv`
//! 列：`at_ms,us,eaten,composing`

use std::fmt::Write as _;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 攒够多少条落一次盘。
const FLUSH_EVERY: usize = 256;
/// 重新检查开关文件的间隔（每键都 stat 太贵）。
const RECHECK_INTERVAL: Duration = Duration::from_secs(2);

struct Sample {
    at_ms: u32,
    us: u32,
    eaten: bool,
    composing: bool,
}

struct Buffer {
    start: Instant,
    on: AtomicBool,
    last_check: Mutex<Instant>,
    samples: Mutex<Vec<Sample>>,
    /// 落盘失败过一次就不再尝试（目录不可写之类），免得每键都去试。
    failed: AtomicBool,
}

impl Buffer {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            on: AtomicBool::new(false),
            // 让第一次调用一定去检查一次开关
            last_check: Mutex::new(Instant::now() - RECHECK_INTERVAL),
            samples: Mutex::new(Vec::with_capacity(FLUSH_EVERY)),
            failed: AtomicBool::new(false),
        }
    }

    fn enabled(&self) -> bool {
        if self.failed.load(Ordering::Relaxed) {
            return false;
        }
        if self.on.load(Ordering::Relaxed) {
            return true;
        }
        if std::env::var_os("WEASEL_KEY_LATENCY").is_some() {
            self.on.store(true, Ordering::Relaxed);
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
            self.on.store(true, Ordering::Relaxed);
        }
        on
    }

    fn push(&self, sample: Sample) {
        let mut batch = None;
        {
            let mut guard = match self.samples.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.push(sample);
            if guard.len() >= FLUSH_EVERY {
                batch = Some(std::mem::take(&mut *guard));
            }
        }
        if let Some(batch) = batch {
            self.flush(&batch);
        }
    }

    fn flush(&self, batch: &[Sample]) {
        let Some(path) = log_path() else {
            self.failed.store(true, Ordering::Relaxed);
            return;
        };
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                self.failed.store(true, Ordering::Relaxed);
                return;
            }
        }
        let mut text = String::with_capacity(batch.len() * 24);
        for sample in batch {
            let _ = writeln!(
                text,
                "{},{},{},{}",
                sample.at_ms,
                sample.us,
                u8::from(sample.eaten),
                u8::from(sample.composing)
            );
        }
        let result = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| file.write_all(text.as_bytes()));
        if result.is_err() {
            self.failed.store(true, Ordering::Relaxed);
        }
    }
}

fn buffer() -> &'static Buffer {
    static BUFFER: OnceLock<Buffer> = OnceLock::new();
    BUFFER.get_or_init(Buffer::new)
}

/// 开关文件：`%APPDATA%\Rime-Next\key-latency.on`。
fn flag_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from)?;
    Some(appdata.join("Rime-Next").join("key-latency.on"))
}

/// 输出 CSV 名（`<component>-key-latency.csv`）。
fn component() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.file_stem().map(|stem| stem.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "unknown".to_string())
    })
    .as_str()
}

fn log_path() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    Some(
        local
            .join("Rime-Next")
            .join("Logs")
            .join(format!("{}-key-latency.csv", component())),
    )
}

/// 记一次耗时。未开启采样时只做一次原子读 + 一次时间比较，可忽略。
pub fn record(elapsed: Duration, eaten: bool, composing: bool) {
    if !buffer().enabled() {
        return;
    }
    let at_ms = buffer().start.elapsed().as_millis().min(u32::MAX as u128) as u32;
    buffer().push(Sample {
        at_ms,
        us: elapsed.as_micros().min(u32::MAX as u128) as u32,
        eaten,
        composing,
    });
}

/// 退出前把残留采样刷出去（尽力而为，失败静默）。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_is_small() {
        assert!(std::mem::size_of::<Sample>() <= 16);
    }

    #[test]
    fn record_without_flag_file_is_a_noop() {
        // 测试进程里开关一般是关的；无论开关状态如何，这里都不能 panic。
        record(Duration::from_millis(1), true, false);
    }
}
