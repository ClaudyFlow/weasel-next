//! 进度上报 —— 供 Weasel 设置界面显示。
//!
//! `--progress-file <路径>` 给出时，每个阶段进度以 **JSON 行**追加写进该文件
//! （每行一个事件，读取方取最后一行即可）。终端下用的是 indicatif 进度条，
//! 两者共存：这里只负责写文件，不碰终端。
//!
//! 格式：`{"phase":"sniff|clone|fetch|download|install|patch|done","pct":0.42,"detail":"..."}`

use std::path::{Path, PathBuf};
use std::sync::Mutex;

static SINK: Mutex<Option<PathBuf>> = Mutex::new(None);

/// 设置进度文件路径（启动时调用一次）；`None` 表示关闭。
pub fn set_sink(path: Option<&Path>) {
    let mut slot = SINK.lock().unwrap_or_else(|e| e.into_inner());
    *slot = path.map(|p| p.to_path_buf());
}

/// 追加一条进度事件。没设 sink 或写失败时静默跳过（进度是锦上添花，不能影响安装）。
pub fn emit(phase: &str, pct: f64, detail: &str) {
    let Some(path) = SINK.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    // pct 收敛到 [0,1]，防御上游传来的任意值
    let pct = pct.clamp(0.0, 1.0);
    let event = serde_json::json!({
        "phase": phase,
        "pct": pct,
        "detail": detail,
    });
    let line = format!("{}\n", event);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = write!(file, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_writes_json_lines_and_clamps_pct() {
        let dir = std::env::temp_dir().join(format!("plum-progress-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("progress.jsonl");
        set_sink(Some(&file));
        emit("sniff", 0.42, "探测镜像");
        emit("clone", 1.7, "拉超了");
        emit("done", -0.2, "拉负了");
        set_sink(None);
        let text = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        for l in &lines {
            let v: serde_json::Value = serde_json::from_str(l).expect("每行必须是合法 JSON");
            let pct = v["pct"].as_f64().unwrap();
            assert!((0.0..=1.0).contains(&pct), "pct 越界: {pct}");
        }
        assert_eq!(lines[0].contains("探测镜像"), true);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
