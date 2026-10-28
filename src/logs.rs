//! 按天追加的 Markdown 日志
//!
//! 位置：`%LOCALAPPDATA%\CamNexus\logs\2026-10-27.md`
//! 格式：每天一个文件，逐行追加 `- **16:05:30** [通道] 内容`

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

/// 日志目录
pub fn dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("CamNexus").join("logs"))
}

/// 今天：`2026-10-27`
pub fn today() -> String {
    let (y, m, d, _, _, _) = crate::sys::local_now();
    format!("{y:04}-{m:02}-{d:02}")
}

/// 当前时刻：`16:05:30`
fn clock() -> String {
    let (_, _, _, h, mi, s) = crate::sys::local_now();
    format!("{h:02}:{mi:02}:{s:02}")
}

/// 追加一行。`tag` 为通道名或模块名，可为空。
pub fn append(tag: &str, msg: &str) {
    let Some(mut path) = dir() else { return };
    let day = today();
    path.push(format!("{day}.md"));

    let _guard = LOCK.lock();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let fresh = !path.exists() || std::fs::metadata(&path).map(|m| m.len() == 0).unwrap_or(false);
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    if fresh {
        let _ = writeln!(f, "# CamNexus 日志 {day}\n");
    }
    if tag.is_empty() {
        let _ = writeln!(f, "- **{}** {}", clock(), msg);
    } else {
        let _ = writeln!(f, "- **{}** [{}] {}", clock(), tag, msg);
    }
}

/// 已有日志的日期，倒序（最新在前）
pub fn days() -> Vec<String> {
    let Some(d) = dir() else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(&d) else {
        return Vec::new();
    };
    let mut v: Vec<String> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_suffix(".md").map(|s| s.to_string())
        })
        .filter(|s| s.len() == 10 && s.as_bytes()[4] == b'-')
        .collect();
    v.sort();
    v.reverse();
    v
}

/// 某一天的日志文件路径
pub fn file_for(day: &str) -> Option<PathBuf> {
    let mut p = dir()?;
    p.push(format!("{day}.md"));
    p.is_file().then_some(p)
}

/// 只保留最近 `keep` 天
pub fn prune(keep: usize) {
    for old in days().into_iter().skip(keep) {
        if let Some(p) = file_for(&old) {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// 用系统默认程序打开（失败则用记事本兜底）
pub fn open(path: &std::path::Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn today_has_iso_shape() {
        let d = today();
        assert_eq!(d.len(), 10);
        assert_eq!(&d[4..5], "-");
        assert_eq!(&d[7..8], "-");
    }

    #[test]
    fn clock_is_hh_mm_ss() {
        let c = clock();
        assert_eq!(c.len(), 8);
        assert_eq!(&c[2..3], ":");
        assert_eq!(&c[5..6], ":");
    }
}
