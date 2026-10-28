//! FFmpeg 后端：版本探测、自动下载、安装
//!
//! 发布包不再携带 ffmpeg.exe（96 MB 里九成用不到），改为首次启动时：
//!   1. 按优先级在本机找 ffmpeg（配置 → 程序目录 → PATH → 常见安装路径 → 上次安装位置）
//!   2. 找到后校验版本是否满足命令用法（需要 ≥ 5.1，`-fps_mode` 从 5.1 起）
//!   3. 没有或太旧则从镜像下载（默认国内源，79 MB，实测约 5 秒）

use crate::i18n::{self, tr};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// 最低可用版本（`-fps_mode` 自 5.1 起）
pub const MIN_VERSION: (u32, u32) = (5, 1);

/// 候选版本：主版本挂了可以退回旧版（命令兼容性要求仅 ≥ 5.1）
/// 已逐一验证：b6.1.1 与 b6.0 在两个源上均返回 200；5.x 的资产命名不同，暂不纳入
const TAGS: &[&str] = &["b6.1.1", "b6.0"];

/// 手动下载页（引导页里给链接）
pub const DOWNLOAD_PAGE: &str = "https://github.com/eugeneware/ffmpeg-static/releases";

/// 候选镜像列表：
///   1. 环境变量 `CAMNEXUS_FFMPEG_MIRROR` 指定的自建/内网源（若有）
///   2. 每个版本依次尝试 npmmirror（国内）与 GitHub Releases
pub fn mirror_list() -> Vec<(&'static str, String)> {
    let mut list = Vec::new();
    if let Ok(custom) = std::env::var("CAMNEXUS_FFMPEG_MIRROR") {
        let custom = custom.trim();
        if !custom.is_empty() {
            list.push((tr("自定义镜像"), custom.to_string()));
        }
    }
    for tag in TAGS {
        list.push((
            tr("npmmirror（国内）"),
            format!("https://registry.npmmirror.com/-/binary/ffmpeg-static/{tag}/ffmpeg-win32-x64"),
        ));
        list.push((
            "GitHub Releases",
            format!("https://github.com/eugeneware/ffmpeg-static/releases/download/{tag}/ffmpeg-win32-x64"),
        ));
    }
    list
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Downloading {
        mirror: &'static str,
        got: u64,
        total: u64,
        speed: f32,
    },
    Verifying,
    Done(PathBuf),
    Failed(String),
}

pub type Shared = Arc<Mutex<State>>;

/// 安装目录：`%LOCALAPPDATA%\CamNexus\bin`（可写，不污染程序目录）
pub fn install_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(
        base.join("CamNexus")
            .join("bin")
            .join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" }),
    )
}

/// 旧版（CamView 时期）的安装位置：改名后自动迁移，避免重新下载 79 MB
pub fn legacy_install_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(
        base.join("CamView")
            .join("bin")
            .join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" }),
    )
}

/// 把旧目录里的 ffmpeg 搬到新目录；失败则保留在原处（find_ffmpeg 会继续找到它）
pub fn migrate_legacy() {
    let (Some(old), Some(new)) = (legacy_install_path(), install_path()) else {
        return;
    };
    if !old.is_file() || new.is_file() {
        return;
    }
    if let Some(dir) = new.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::rename(&old, &new).is_err() && std::fs::copy(&old, &new).is_ok() {
        let _ = std::fs::remove_file(&old);
    }
}

/// 运行 `ffmpeg -version`，返回解析出的版本号字符串
pub fn probe_version(exe: &Path) -> Result<(u32, u32, String), String> {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("-hide_banner").arg("-version");
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd
        .output()
        .map_err(|e| i18n::err(format!("{} {}", tr("无法执行"), exe.display()), e))?;
    if !out.status.success() {
        return Err(tr("ffmpeg 返回非零退出码").into());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.lines().next().unwrap_or("");
    // 形如 "ffmpeg version 6.1.1-essentials_build-www.gyan.dev Copyright ..."
    let ver = first
        .split_whitespace()
        .nth(2)
        .ok_or_else(|| i18n::err(tr("无法解析版本行"), first))?;
    let mut parts = ver.split('.');
    let major: u32 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| i18n::err(tr("无法解析主版本号"), ver))?;
    let minor: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Ok((major, minor, ver.to_string()))
}

/// 检查某个 ffmpeg 是否可用；返回可读的版本描述
pub fn check_usable(exe: &Path) -> Result<String, String> {
    let (maj, min, raw) = probe_version(exe)?;
    if (maj, min) < MIN_VERSION {
        return Err(i18n::version_too_old(&raw, MIN_VERSION.0, MIN_VERSION.1));
    }
    Ok(raw)
}

/// 启动后台下载。返回共享状态供界面轮询。
pub fn spawn_download() -> Shared {
    let state: Shared = Arc::new(Mutex::new(State::Downloading {
        mirror: tr("准备中"),
        got: 0,
        total: 0,
        speed: 0.0,
    }));

    let st = state.clone();
    std::thread::Builder::new()
        .name("ffmpeg-download".into())
        .spawn(move || run(st))
        .ok();

    state
}

fn set(state: &Shared, s: State) {
    if let Ok(mut g) = state.lock() {
        *g = s;
    }
}

fn run(state: Shared) {
    let Some(target) = install_path() else {
        set(&state, State::Failed(tr("找不到可写的安装目录").into()));
        return;
    };
    if let Some(dir) = target.parent()
        && let Err(e) = std::fs::create_dir_all(dir) {
            set(&state, State::Failed(i18n::err(tr("创建目录失败"), e)));
            return;
        }
    let part = target.with_extension("part");

    let mut last_err = String::from(tr("没有可用的下载源"));
    for (name, url) in mirror_list() {
        match download(&url, &part, &state, name) {
            Ok(()) => {
                set(&state, State::Verifying);
                if let Err(e) = std::fs::rename(&part, &target) {
                    // 跨卷或占用时退回复制
                    if let Err(e2) = std::fs::copy(&part, &target) {
                        last_err = format!("{} / {e2}", i18n::err(tr("保存失败"), e));
                        continue;
                    }
                    let _ = std::fs::remove_file(&part);
                }
                match check_usable(&target) {
                    Ok(ver) => {
                        set(&state, State::Done(target.clone()));
                        eprintln!("{}", i18n::ffmpeg_installed(&ver, &target.display().to_string()));
                        return;
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&target);
                        last_err = i18n::err(tr("下载到的文件不可用"), e);
                        continue;
                    }
                }
            }
            Err(e) => {
                last_err = format!("{name}：{e}");
                let _ = std::fs::remove_file(&part);
            }
        }
        if let Ok(mut g) = state.lock() {
            *g = State::Downloading {
                mirror: tr("切换备用源…"),
                got: 0,
                total: 0,
                speed: 0.0,
            };
        }
    }

    set(&state, State::Failed(last_err));
}

fn download(url: &str, dest: &Path, state: &Shared, mirror: &'static str) -> Result<(), String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(None)
        .build()
        .new_agent();

    let mut resp = agent
        .get(url)
        .header("User-Agent", "CamNexus/0.5 (+https://github.com)")
        .call()
        .map_err(|e| i18n::err(tr("请求失败"), e))?;

    let total = resp.body().content_length().unwrap_or(0);
    let mut reader = resp.body_mut().as_reader();
    let mut file = std::fs::File::create(dest).map_err(|e| i18n::err(tr("创建临时文件失败"), e))?;

    let mut buf = vec![0u8; 64 * 1024];
    let mut got: u64 = 0;
    let mut last_report = Instant::now();
    let mut last_bytes: u64 = 0;

    loop {
        let n = reader.read(&mut buf).map_err(|e| i18n::err(tr("读取中断"), e))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| i18n::err(tr("写入失败"), e))?;
        got += n as u64;

        if last_report.elapsed().as_millis() >= 200 {
            let dt = last_report.elapsed().as_secs_f32();
            let speed = (got - last_bytes) as f32 / dt;
            last_report = Instant::now();
            last_bytes = got;
            set(
                state,
                State::Downloading {
                    mirror,
                    got,
                    total,
                    speed,
                },
            );
        }
    }

    file.flush().map_err(|e| i18n::err(tr("落盘失败"), e))?;
    if got < 20 * 1024 * 1024 {
        return Err(i18n::file_too_small(got));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_list_has_fallbacks() {
        let list = mirror_list();
        assert_eq!(list.len(), 4, "两个版本 × 两个源");
        // 镜像名是翻译后的结果，而语言是全局状态、i18n 的测试会切它。
        // 这里只断言两种语言下都有的专有名词，避免并行测试互相干扰。
        assert!(list[0].0.contains("npmmirror"));
        assert_eq!(list[1].0, "GitHub Releases");
        assert!(list[0].1.contains("b6.1.1"));
        assert!(list[2].1.contains("b6.0"));
    }

    #[test]
    fn install_path_is_under_appdata() {
        let p = install_path().expect(tr("应能定位安装目录"));
        assert!(p.to_string_lossy().to_lowercase().contains("camnexus"));
        assert!(p.to_string_lossy().ends_with("ffmpeg.exe") || !cfg!(windows));
    }
}
