//! 一次性测量：逐帧 RGBA 四通道帧间差分（默认关闭）。
//!
//! 打开方式：`WEASEL_FRAME_DIFF=1`，输出路径 `WEASEL_FRAME_DIFF_OUT`（默认
//! `%TEMP%\weasel-frame-diff.csv`），最多记录 `WEASEL_FRAME_DIFF_MAX` 帧（默认 400）。
//! 只在 `--preview` 预览进程里用；关闭时每帧只多一次 env 缓存检查，不改变行为。
//!
//! 通道顺序按 **RGBA** 输出（交换链是 BGRA，读回时换序）。

use std::{
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

struct Config {
    out: PathBuf,
    max: u64,
}

static CONFIG: OnceLock<Option<Config>> = OnceLock::new();
static COUNT: AtomicU64 = AtomicU64::new(0);

fn config() -> Option<&'static Config> {
    CONFIG
        .get_or_init(|| {
            let on = matches!(
                std::env::var("WEASEL_FRAME_DIFF").as_deref(),
                Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes")
            );
            if !on {
                return None;
            }
            let out = std::env::var_os("WEASEL_FRAME_DIFF_OUT")
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::temp_dir().join("weasel-frame-diff.csv"));
            let max = std::env::var("WEASEL_FRAME_DIFF_MAX")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(400);
            let _ = std::fs::write(
                std::env::temp_dir().join("frame-diff-armed.txt"),
                out.display().to_string(),
            );
            Some(Config { out, max })
        })
        .as_ref()
}

pub(crate) fn enabled() -> bool {
    match config() {
        Some(cfg) => COUNT.load(Ordering::Relaxed) < cfg.max,
        None => false,
    }
}

/// 诊断标记：写 `%TEMP%\frame-diff-debug.txt`（追加，便于一次看清到哪一步失败）。
pub(crate) fn debug(message: impl std::fmt::Display) {
    use std::io::Write as _;
    let path = std::env::temp_dir().join("frame-diff-debug.txt");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{message}");
    }
}

struct State {
    prev: Option<(Vec<u8>, u32, u32)>,
    header_written: bool,
    failed: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    prev: None,
    header_written: false,
    failed: false,
});

const HEADER: &str = "frame,ts_us,frame_us,enddraw_us,capture_us,present_us,diff_us,w,h,\
changed_px,changed_pct,strict_px,strict_pct,bbox_l,bbox_t,bbox_r,bbox_b,bbox_pct,\
max_dr,max_dg,max_db,max_da,mean_dr,mean_dg,mean_db,mean_da,identical,size_changed\n";

/// 写一帧差分结果。`rgba` 为 RGBA 顺序；相对上一帧（同尺寸）逐像素逐通道取绝对差。
///
/// `strict` 阈值 3/255：预乘 alpha 与舍入会带来 1–2 的噪声，只有超过 3 才计入脏矩形。
pub(crate) fn observe(
    rgba: &[u8],
    width: u32,
    height: u32,
    ts_us: u64,
    frame_us: u64,
    enddraw_us: u64,
    capture_us: u64,
    present_us: u64,
) {
    let cfg = match config() {
        Some(cfg) => cfg,
        None => return,
    };
    let index = COUNT.fetch_add(1, Ordering::Relaxed);
    if index >= cfg.max {
        return;
    }
    let diff_start = super::presenter::now_us();
    let mut state = match STATE.lock() {
        Ok(state) => state,
        Err(_) => return,
    };
    if state.failed {
        return;
    }

    let pixels = (width as usize) * (height as usize);
    let mut changed = 0u64;
    let mut strict = 0u64;
    let (mut l, mut t, mut r, mut b) = (width, height, 0u32, 0u32);
    let mut maxc = [0u16; 4];
    let mut sumc = [0u64; 4];
    let mut size_changed = 0u8;

    match &state.prev {
        Some((prev, pw, ph)) if *pw == width && *ph == height && prev.len() == rgba.len() => {
            for i in 0..pixels {
                let base = i * 4;
                let mut hit = false;
                let mut hit_strict = false;
                for c in 0..4 {
                    let a = prev[base + c];
                    let b = rgba[base + c];
                    let d = a.abs_diff(b) as u16;
                    sumc[c] += d as u64;
                    if d > maxc[c] {
                        maxc[c] = d;
                    }
                    if d > 0 {
                        hit = true;
                    }
                    if d > 3 {
                        hit_strict = true;
                    }
                }
                if hit {
                    changed += 1;
                }
                if hit_strict {
                    strict += 1;
                    let x = (i as u32) % width;
                    let y = (i as u32) / width;
                    l = l.min(x);
                    t = t.min(y);
                    r = r.max(x + 1);
                    b = b.max(y + 1);
                }
            }
        }
        _ => {
            // 首帧或尺寸变化：整窗都算“新内容”。
            size_changed = 1;
            changed = pixels as u64;
            strict = pixels as u64;
            l = 0;
            t = 0;
            r = width;
            b = height;
            for c in 0..4 {
                maxc[c] = 255;
                sumc[c] = 255 * pixels as u64;
            }
        }
    }

    state.prev = Some((rgba.to_vec(), width, height));

    let diff_us = super::presenter::now_us().saturating_sub(diff_start);
    let total = pixels.max(1) as f64;
    let bbox_px = if strict == 0 {
        0.0
    } else {
        ((r - l) as f64) * ((b - t) as f64)
    };
    let line = format!(
        "{index},{ts_us},{frame_us},{enddraw_us},{capture_us},{present_us},{diff_us},{width},{height},\
{changed},{chg_pct:.3},{strict},{strict_pct:.3},{l},{t},{r},{b},{bbox_pct:.3},\
{},{},{},{},{:.3},{:.3},{:.3},{:.3},{},{size_changed}\n",
        maxc[0],
        maxc[1],
        maxc[2],
        maxc[3],
        sumc[0] as f64 / total,
        sumc[1] as f64 / total,
        sumc[2] as f64 / total,
        sumc[3] as f64 / total,
        u8::from(changed == 0),
        chg_pct = 100.0 * changed as f64 / total,
        strict_pct = 100.0 * strict as f64 / total,
        bbox_pct = 100.0 * bbox_px / total,
    );

    let write = || -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&cfg.out)?;
        if !state.header_written {
            file.write_all(HEADER.as_bytes())?;
        }
        file.write_all(line.as_bytes())?;
        Ok(())
    };
    if write().is_err() {
        state.failed = true;
        return;
    }
    state.header_written = true;
}
