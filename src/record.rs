//! 录制：每路一个 ffmpeg 子进程，按整点分段写入 mp4
//!
//! 设计要点：
//! - 与显示链路完全解耦，另起进程直读 RTSP，主界面卡顿不影响录像；
//! - 分段对齐整点：16:23 开始 → 第一段 16-23_17-00，之后每小时一段；
//! - 停止时向 ffmpeg 的 stdin 写 `q` 让它自己收尾（写 moov），而不是 kill；
//! - 容量上限按「整文件」删除最早的文件，循环腾空间。

use crate::i18n::{self, tr};
use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::{AppConfig, Channel, RecCodec};
use crate::logs;

/// 停止宽限期：超过就强杀，避免界面卡住
const STOP_GRACE: Duration = Duration::from_secs(5);

/// Windows 作业对象：主程序退出（含被强杀）时，作业关闭 → 里面的 ffmpeg 一并终止。
/// 没有它，主程序被结束进程后会留下孤儿 ffmpeg 继续往文件里写。
#[cfg(windows)]
struct JobGuard(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl JobGuard {
    fn create() -> Option<Self> {
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        // SAFETY: 只创建/设置本进程私有的作业对象
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                return None;
            }
            Some(Self(job))
        }
    }

    fn assign(&self, child: &Child) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        // SAFETY: 句柄来自刚 spawn 的子进程，作业句柄有效
        unsafe {
            AssignProcessToJobObject(self.0, child.as_raw_handle());
        }
    }
}

#[cfg(windows)]
impl Drop for JobGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        // SAFETY: 关闭前先显式终止，确保子进程不会残留
        unsafe {
            TerminateJobObject(self.0, 0);
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

struct Recorder {
    child: Child,
    path: PathBuf,
    stop_deadline: Option<Instant>,
}

impl Recorder {
    /// 请求优雅停止：写 `q` 让 ffmpeg 写完 moov
    fn request_stop(&mut self) {
        if self.stop_deadline.is_some() {
            return;
        }
        if let Some(mut si) = self.child.stdin.take() {
            let _ = si.write_all(b"q\n");
            let _ = si.flush();
        }
        self.stop_deadline = Some(Instant::now() + STOP_GRACE);
    }

    /// 是否已退出（顺带处理超时强杀）
    fn poll(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(Some(_)) => true,
            Ok(None) => {
                if let Some(d) = self.stop_deadline
                    && Instant::now() >= d {
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        return true;
                    }
                false
            }
            Err(_) => true,
        }
    }
}

struct Slot {
    want: bool,
    rec: Option<Recorder>,
    /// 连续失败次数：防止 ffmpeg 秒退时每帧重试
    fails: u8,
}

pub struct RecordManager {
    slots: HashMap<usize, Slot>,
    last_cleanup: Instant,
    pub last_error: Option<String>,
    #[cfg(windows)]
    job: Option<JobGuard>,
    /// 目录占用缓存：(采样时刻, 字节数)
    size_cache: Option<(Instant, u64)>,
}

impl Default for RecordManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordManager {
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
            last_cleanup: Instant::now(),
            last_error: None,
            #[cfg(windows)]
            job: JobGuard::create(),
            size_cache: None,
        }
    }

    pub fn is_recording(&self, idx: usize) -> bool {
        self.slots.get(&idx).map(|s| s.want).unwrap_or(false)
    }

    /// 切换录制状态，返回切换后的目标状态
    pub fn toggle(&mut self, idx: usize) -> bool {
        let slot = self.slots.entry(idx).or_insert(Slot {
            want: false,
            rec: None,
            fails: 0,
        });
        if !slot.want {
            slot.fails = 0;
        }
        slot.want = !slot.want;
        slot.want
    }

    /// 非阻塞地停止全部：通道/参数变更时用，避免界面卡住
    pub fn stop_all_async(&mut self) {
        for slot in self.slots.values_mut() {
            slot.want = false;
            if let Some(r) = slot.rec.as_mut() {
                r.request_stop();
            }
        }
    }

    /// 退出程序时调用：全部请求停止并等待收尾
    pub fn shutdown(&mut self) {
        self.stop_all_async();
        let deadline = Instant::now() + STOP_GRACE;
        loop {
            let mut pending = false;
            for slot in self.slots.values_mut() {
                if let Some(r) = slot.rec.as_mut() {
                    if r.poll() {
                        slot.rec = None;
                    } else {
                        pending = true;
                    }
                }
            }
            if !pending || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// 带缓存的目录占用（设置面板每帧都会问，不能真每帧都扫盘）
    pub fn dir_size_cached(&mut self, root: &Path) -> u64 {
        const TTL: Duration = Duration::from_secs(3);
        if let Some((t, v)) = self.size_cache
            && t.elapsed() < TTL {
                return v;
            }
        let v = dir_size(root);
        self.size_cache = Some((Instant::now(), v));
        v
    }

    /// 每帧驱动：启停、整点分段、容量清理
    pub fn tick(&mut self, ffmpeg: &Path, cfg: &AppConfig, channels: &[Channel]) {
        let root = record_root(cfg);

        // 通道被删除/重排后，超出的旧槽位不再有效，直接停掉，避免张冠李戴
        let stale: Vec<usize> = self
            .slots
            .keys()
            .copied()
            .filter(|i| *i >= channels.len())
            .collect();
        for i in stale {
            if let Some(mut slot) = self.slots.remove(&i) {
                slot.want = false;
                if let Some(r) = slot.rec.as_mut() {
                    r.request_stop();
                }
            }
        }

        for (idx, ch) in channels.iter().enumerate() {
            let slot = self.slots.entry(idx).or_insert(Slot {
                want: false,
                rec: None,
                fails: 0,
            });

            if let Some(r) = slot.rec.as_mut() {
                if r.poll() {
                    // 本段结束（到点或被停止）
                    let size = std::fs::metadata(&r.path).map(|m| m.len()).unwrap_or(0);
                    let name = r.path.file_name().unwrap_or_default().to_string_lossy();
                    if size < 64 * 1024 {
                        // 秒退：地址不通、编码器缺失、参数错误……不能无限重试
                        let _ = std::fs::remove_file(&r.path);
                        slot.fails = slot.fails.saturating_add(1);
                        logs::append(
                            &ch.name,
                            &i18n::rec_fail_n(slot.fails),
                        );
                        if slot.fails >= 3 {
                            slot.want = false;
                            slot.fails = 0;
                            self.last_error = Some(i18n::rec_gave_up(&ch.name));
                        }
                    } else {
                        slot.fails = 0;
                        logs::append(&ch.name, &i18n::rec_finished(&name, size as f64 / 1_048_576.0));
                    }
                    slot.rec = None;
                    self.size_cache = None;
                } else if !slot.want {
                    // 用户点了停止：只发一次
                    if let Some(r) = slot.rec.as_mut() {
                        r.request_stop();
                    }
                }
            }

            if slot.want && slot.rec.is_none() {
                #[cfg(windows)]
                let spawned = spawn_segment(ffmpeg, cfg, channels, idx, &root, self.job.as_ref());
                #[cfg(not(windows))]
                let spawned = spawn_segment(ffmpeg, cfg, channels, idx, &root);
                match spawned {
                    Ok(rec) => {
                        logs::append(
                            &ch.name,
                            &i18n::rec_started(
                                &rec.path
                                    .strip_prefix(&root)
                                    .unwrap_or(&rec.path)
                                    .display()
                                    .to_string(),
                            ),
                        );
                        slot.rec = Some(rec);
                        self.last_error = None;
                    }
                    Err(e) => {
                        slot.want = false;
                        self.last_error = Some(e.clone());
                        logs::append(&ch.name, &i18n::err(tr("录像启动失败"), e));
                    }
                }
            }
        }

        // 每 60 秒检查一次容量
        if self.last_cleanup.elapsed() >= Duration::from_secs(60) {
            self.last_cleanup = Instant::now();
            let active: Vec<PathBuf> = self
                .slots
                .values()
                .filter_map(|s| s.rec.as_ref().map(|r| r.path.clone()))
                .collect();
            // 按「用户设的上限」和「磁盘实际能放多少」里更小的那个来清
            let used_mb = self.dir_size_cached(&root) / 1_048_576;
            let free_mb = crate::sys::free_bytes(&root).map(|b| b / 1_048_576);
            let limit_mb = effective_limit_mb(cfg.record_max_mb, used_mb, free_mb);
            if let Some((deleted, freed)) = cleanup(&root, limit_mb, &active) {
                self.size_cache = None;
                if deleted > 0 {
                    logs::append(
                        "",
                        &i18n::quota_cleanup(deleted, freed as f64 / 1_048_576.0),
                    );
                }
            }
        }
    }
}

/// 存档根目录：配置为空时用 `用户目录\Videos\CamNexus`
pub fn record_root(cfg: &AppConfig) -> PathBuf {
    let raw = cfg.record_dir.trim();
    if !raw.is_empty() {
        return PathBuf::from(raw);
    }
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Videos").join("CamNexus")
}

/// 通道目录名：去掉 Windows 非法字符，并与前面的通道去重
/// 对外暴露：回放窗口拼日期目录时用
pub fn channel_folder_pub(channels: &[Channel], idx: usize) -> String {
    channel_folder(channels, idx)
}

fn channel_folder(channels: &[Channel], idx: usize) -> String {
    // 逐个分配，已占用的名字累加比较——只比原始名会撞车（前门 / 前门 / 前门_2）
    let mut used: Vec<String> = Vec::new();
    for c in channels.iter().take(idx + 1) {
        let base = sanitize(&c.name);
        let mut name = base.clone();
        let mut n = 2;
        while used.contains(&name) {
            name = format!("{base}_{n}");
            n += 1;
        }
        used.push(name);
    }
    used[idx].clone()
}

fn sanitize(name: &str) -> String {
    let mut s: String = name
        .trim()
        .chars()
        .map(|c| {
            if matches!(c, '"' | '<' | '>' | ':' | '|' | '?' | '*' | '/' | '\\') {
                '_'
            } else {
                c
            }
        })
        .collect();
    // Windows 不允许结尾的空格和点
    while s.ends_with(' ') || s.ends_with('.') {
        s.pop();
    }
    if s.trim().is_empty() {
        s = tr("通道").to_string();
    }
    s
}

/// 计算本段时长与结束时间（对齐整点）
fn segment_plan() -> (u32, String) {
    let (_, _, _, h, mi, s) = crate::sys::local_now();
    // 到下一个整点还有多少秒
    let mut secs = (60 - mi) * 60 - s;
    if secs < 60 {
        // 马上就到整点：干脆录满一小时，避免产生几秒钟的碎片文件
        secs = 3600;
    }
    // 必须向上取整：secs 已对齐到整点，向下取整会把 17:00 算成 16:59
    let end_min = h * 60 + mi + secs.div_ceil(60);
    let eh = (end_min / 60) % 24;
    let em = end_min % 60;
    (secs, format!("{eh:02}-{em:02}"))
}

#[cfg(windows)]
fn spawn_segment(
    ffmpeg: &Path,
    cfg: &AppConfig,
    channels: &[Channel],
    idx: usize,
    root: &Path,
    job: Option<&JobGuard>,
) -> Result<Recorder, String> {
    spawn_segment_inner(ffmpeg, cfg, channels, idx, root, job)
}

#[cfg(not(windows))]
fn spawn_segment(
    ffmpeg: &Path,
    cfg: &AppConfig,
    channels: &[Channel],
    idx: usize,
    root: &Path,
) -> Result<Recorder, String> {
    spawn_segment_inner(ffmpeg, cfg, channels, idx, root)
}

fn spawn_segment_inner(
    ffmpeg: &Path,
    cfg: &AppConfig,
    channels: &[Channel],
    idx: usize,
    root: &Path,
    #[cfg(windows)] job: Option<&JobGuard>,
) -> Result<Recorder, String> {
    let ch = &channels[idx];
    let (y, mo, d, h, mi, _) = crate::sys::local_now();
    let date = format!("{y:04}-{mo:02}-{d:02}");

    let (secs, end_hm) = segment_plan();
    let dir = root.join(&date).join(channel_folder(channels, idx));
    std::fs::create_dir_all(&dir).map_err(|e| i18n::err(tr("创建录像目录失败"), e))?;

    let path = dir.join(format!("{date} {h:02}-{mi:02}_{end_hm}.mp4"));

    // 分辨率：0 = 跟随送显
    let (mut w, mut hgt) = if ch.record.width > 0 && ch.record.height > 0 {
        (ch.record.width, ch.record.height)
    } else {
        (cfg.render_width.max(2), cfg.render_height.max(2))
    };
    // H.264 + yuv420p 要求偶数
    w -= w % 2;
    hgt -= hgt % 2;

    let mut cmd = Command::new(ffmpeg);
    cmd.arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostats")
        .arg("-rtsp_transport")
        .arg(if cfg.tcp { "tcp" } else { "udp" })
        .arg("-timeout")
        .arg("5000000")
        .arg("-i")
        .arg(ch.rtsp())
        .arg("-t")
        .arg(secs.to_string())
        .arg("-map")
        .arg("0:v:0");

    if ch.record.audio {
        cmd.arg("-map").arg("0:a:0?");
    }

    if ch.record.copy {
        // 源画质：不转码，分辨率跟随摄像机主码流
        cmd.arg("-c:v").arg("copy");
    } else {
        cmd.arg("-vf").arg(format!("scale={w}:{hgt}"));
        let (crf, gop) = ch.record.quality.params(ch.record.codec);
        match ch.record.codec {
            RecCodec::H265 => {
                // `hvc1` 标记让 Windows/苹果系播放器认得出 HEVC
                cmd.arg("-c:v")
                    .arg("libx265")
                    .arg("-preset")
                    .arg("veryfast")
                    .arg("-crf")
                    .arg(crf.to_string())
                    .arg("-tag:v")
                    .arg("hvc1");
            }
            RecCodec::H264 => {
                cmd.arg("-c:v")
                    .arg("libx264")
                    .arg("-preset")
                    .arg("veryfast")
                    .arg("-crf")
                    .arg(crf.to_string());
            }
        }
        cmd.arg("-pix_fmt").arg("yuv420p");
        // 关键帧间隔：分片 mp4 只在关键帧处落盘，间隔越短崩溃丢得越少、但体积越大。
        cmd.arg("-g").arg(gop.to_string());
    }

    // 分片 mp4：写入被中断（崩溃/强杀）时文件依然可播，
    // 代价是容器结构不同，极个别老播放器可能不认。
    cmd.arg("-movflags")
        .arg("+frag_keyframe+empty_moov+default_base_moof");

    if ch.record.audio {
        cmd.arg("-c:a").arg("aac").arg("-b:a").arg("96k");
    } else {
        cmd.arg("-an");
    }

    // 录像进程不能用 -nostdin，否则停不下来
    cmd.arg("-y")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let child = cmd.spawn().map_err(|e| i18n::err(tr("启动 ffmpeg 失败"), e))?;
    #[cfg(windows)]
    if let Some(job) = job {
        job.assign(&child);
    }
    Ok(Recorder {
        child,
        path,
        stop_deadline: None,
    })
}

/// 目录总大小（字节）
pub fn dir_size(root: &Path) -> u64 {
    walk_files(root).iter().map(|(_, s, _)| *s).sum()
}

/// 递归列出所有 mp4：(路径, 大小, 修改时间)
fn walk_files(root: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .extension()
                .map(|x| x.eq_ignore_ascii_case("mp4"))
                .unwrap_or(false)
                && let Ok(md) = e.metadata() {
                    let t = md.modified().unwrap_or(std::time::UNIX_EPOCH);
                    out.push((p, md.len(), t));
                }
        }
    }
    out
}

/// 超出上限时删除最早的文件。返回（删除数量，释放字节）
/// 给系统留的余量（MB）：写满磁盘会让配置、日志、临时文件全都失败
pub const DISK_RESERVE_MB: u64 = 512;

/// 实际生效的录制上限（MB）。
///
/// 用户填的上限可能远大于磁盘：比如默认 500 GB 放在一块只剩 80 GB 的盘上。
/// 这种情况下如果只按 `max_mb` 判断，永远触发不了清理，最后把盘写满、
/// ffmpeg 写入失败、录制连续报错停止。所以这里取两者更小的那个。
///
/// `free_mb` 为 None（读不到磁盘信息）时不做限制，保持原行为。
pub fn effective_limit_mb(max_mb: u64, used_mb: u64, free_mb: Option<u64>) -> u64 {
    if max_mb == 0 {
        return 0; // 0 = 不限制，尊重用户选择
    }
    let Some(free) = free_mb else {
        return max_mb;
    };
    // 已用的 + 还能写的 − 预留 = 这块盘最多能放多少
    let hard = used_mb.saturating_add(free.saturating_sub(DISK_RESERVE_MB));
    // 但不能比已用量还小，否则每次清理都想删到 0
    max_mb.min(hard).max(used_mb)
}

pub fn cleanup(root: &Path, max_mb: u64, active: &[PathBuf]) -> Option<(usize, u64)> {
    if max_mb == 0 {
        return None;
    }
    let limit = max_mb * 1_048_576;
    let mut files = walk_files(root);
    let mut total: u64 = files.iter().map(|(_, s, _)| *s).sum();
    if total <= limit {
        return None;
    }
    // 最早的在前面；正在写的文件不能删
    files.sort_by_key(|(_, _, t)| *t);
    let mut deleted = 0usize;
    let mut freed = 0u64;
    for (path, size, _) in files {
        if total <= limit {
            break;
        }
        if active.iter().any(|a| a == &path) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
            freed += size;
            deleted += 1;
        }
    }
    if deleted > 0 {
        prune_empty_dirs(root);
    }
    Some((deleted, freed))
}

/// 删除空的日期/通道目录（非空会失败，安全）
fn prune_empty_dirs(root: &Path) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for day in rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        if let Ok(inner) = std::fs::read_dir(&day) {
            for sub in inner.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
                let _ = std::fs::remove_dir(&sub);
            }
        }
        let _ = std::fs::remove_dir(&day);
    }
}

/// 某通道已有录像的日期列表（倒序）
pub fn dates_for(root: &Path, channels: &[Channel], idx: usize) -> Vec<String> {
    // 目录结构是「根/日期/通道」，所以遍历根下的日期，再看该日期里有没有这个通道
    let chan = channel_folder(channels, idx);
    let Ok(rd) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut v: Vec<String> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|s| s.len() == 10 && s.as_bytes()[4] == b'-')
        .filter(|day| has_video(&root.join(day).join(&chan)))
        .collect();
    v.sort();
    v.reverse();
    v
}

/// 目录里有没有 mp4
fn has_video(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    rd.flatten().any(|e| {
        e.path()
            .extension()
            .map(|x| x.eq_ignore_ascii_case("mp4"))
            .unwrap_or(false)
    })
}

/// 用资源管理器打开目录（不存在则创建）
pub fn open_folder(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| i18n::err(tr("打开失败"), e))?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("explorer")
            .arg(path)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| i18n::err(tr("打开资源管理器失败"), e))?;
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|e| i18n::err(tr("打开目录失败"), e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_limit_follows_disk_when_cap_is_huge() {
        // 上限 500 GB，但盘上只剩 80 GB、已用 10 GB → 最多 10+80-0.5 ≈ 89 GB
        let cap = 512_000u64;
        let used = 10_240u64;
        let free = 81_920u64;
        let lim = effective_limit_mb(cap, used, Some(free));
        assert_eq!(lim, used + free - DISK_RESERVE_MB);
        assert!(lim < cap);
    }

    #[test]
    fn effective_limit_keeps_user_cap_when_disk_is_big() {
        // 盘很大 → 用户设的上限说了算
        assert_eq!(effective_limit_mb(4_096, 100, Some(10_000_000)), 4_096);
    }

    #[test]
    fn effective_limit_zero_means_unlimited() {
        // 0 = 不限制，不要因为磁盘还算出个值来
        assert_eq!(effective_limit_mb(0, 100, Some(5_000)), 0);
    }

    #[test]
    fn effective_limit_never_below_used() {
        // 盘快满了（可用 0）也不能算出比已用更小的上限，否则会疯狂删
        assert_eq!(effective_limit_mb(512_000, 3_000, Some(0)), 3_000);
    }

    #[test]
    fn effective_limit_without_disk_info() {
        // 读不到磁盘信息 → 退回用户设置
        assert_eq!(effective_limit_mb(512_000, 3_000, None), 512_000);
    }
    use crate::config::RecordCfg;

    fn ch(name: &str, url: &str) -> Channel {
        Channel {
            name: name.into(),
            url: url.into(),
            username: "admin".into(),
            password: "pw".into(),
            enabled: true,
            record: RecordCfg::default(),
            rotate: 0,
            flip_h: false,
            flip_v: false,
            aspect_lock: true,
            crop_borders: false,
            sub_url: String::new(),
        }
    }

    #[test]
    fn sanitize_strips_illegal_chars() {
        assert_eq!(sanitize("a/b:c*d?"), "a_b_c_d_");
        assert_eq!(sanitize("  前门.  "), "前门");
        assert_eq!(sanitize("   "), "通道");
    }

    #[test]
    fn duplicate_names_get_suffix() {
        let list = vec![ch(tr("前门"), "u"), ch(tr("前门"), "u"), ch(tr("前门"), "u")];
        assert_eq!(channel_folder(&list, 0), "前门");
        assert_eq!(channel_folder(&list, 1), "前门_2");
        assert_eq!(channel_folder(&list, 2), "前门_3");
    }

    #[test]
    fn segment_ends_on_the_hour() {
        let (secs, end) = segment_plan();
        assert!(secs >= 60 && secs <= 3600, "分段时长应在 1 分钟到 1 小时之间");
        // 结束时间的分钟位必须是整点（除非走了 3600 的兜底分支）
        if secs != 3600 {
            assert_eq!(&end[3..], "00", "应对齐整点，实际 {end}");
        }
    }

    #[test]
    fn dates_lookup_uses_date_then_channel() {
        let tmp = std::env::temp_dir().join("camnexus_dates_test");
        let _ = std::fs::remove_dir_all(&tmp);
        let chan = tmp.join("2026-10-27").join(tr("前门"));
        std::fs::create_dir_all(&chan).unwrap();
        std::fs::write(chan.join("2026-10-27 16-00_17-00.mp4"), b"x").unwrap();
        // 另一天只有目录没有文件，不应计入
        std::fs::create_dir_all(tmp.join("2026-10-28").join(tr("前门"))).unwrap();

        let list = vec![ch(tr("前门"), "u")];
        let days = dates_for(&tmp, &list, 0);
        assert_eq!(days, vec!["2026-10-27".to_string()]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn default_root_is_under_videos() {
        let cfg = AppConfig::default();
        let root = record_root(&cfg);
        assert!(root.ends_with("CamNexus"));
    }
}
