//! 界面文案的中英双语支持。
//!
//! 设计取舍：**以中文原文作为翻译表的 key**（gettext 的做法）。
//! 好处是替换处只多一层 `tr("…")`，改错的可能性降到最低，
//! 且漏翻时自动回退中文，不会出现空白或 panic。
//!
//! - 静态文案：`tr("设置")` —— 查表，中文模式下直接返回原文（不查表）
//! - 带参文案：本文件里的辅助函数，内部按语言选 `format!` 模板
//! - 反复调用的路径（每帧几十次）是 HashMap 查找，短字符串开销可忽略

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    Zh,
    En,
}

impl Lang {
    pub fn label(self) -> &'static str {
        match self {
            Lang::Zh => "简体中文",
            Lang::En => "English",
        }
    }
}

static CUR: AtomicU8 = AtomicU8::new(0);

pub fn lang() -> Lang {
    if CUR.load(Ordering::Relaxed) == 1 {
        Lang::En
    } else {
        Lang::Zh
    }
}

pub fn set_lang(l: Lang) {
    CUR.store(
        match l {
            Lang::Zh => 0,
            Lang::En => 1,
        },
        Ordering::Relaxed,
    );
}

fn table() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| PAIRS.iter().copied().collect())
}

/// 中文原文 → 当前语言。未收录的文案原样返回。
pub fn tr(zh: &'static str) -> &'static str {
    if lang() == Lang::Zh {
        return zh;
    }
    table().get(zh).copied().unwrap_or(zh)
}

/// 「前缀：错误」这种最常用的形式，中文用全角冒号，英文用半角冒号 + 空格
pub fn err(what: impl std::fmt::Display, e: impl std::fmt::Display) -> String {
    match lang() {
        Lang::Zh => format!("{what}：{e}"),
        Lang::En => format!("{what}: {e}"),
    }
}

// ============================================================ 带参文案

pub fn version(v: &str) -> String {
    match lang() {
        Lang::Zh => format!("版本 {v}"),
        Lang::En => format!("Version {v}"),
    }
}

pub fn started(ver: &str, ff: &str) -> String {
    match lang() {
        Lang::Zh => format!("启动 v{ver}（ffmpeg {ff}）"),
        Lang::En => format!("Started v{ver} (ffmpeg {ff})"),
    }
}

pub fn online(live: usize, total: usize) -> String {
    match lang() {
        Lang::Zh => format!("{live}/{total} 在线"),
        Lang::En => format!("{live}/{total} online"),
    }
}

pub fn audio_connecting(i: usize) -> String {
    match lang() {
        Lang::Zh => format!("音频 通道{} 连接中…", i + 1),
        Lang::En => format!("Audio ch{} connecting…", i + 1),
    }
}

pub fn audio_level(i: usize, pct: f32) -> String {
    match lang() {
        Lang::Zh => format!("音频 通道{}  电平 {pct:.0}%", i + 1),
        Lang::En => format!("Audio ch{}  level {pct:.0}%", i + 1),
    }
}

pub fn channel_n(n: usize) -> String {
    match lang() {
        Lang::Zh => format!("通道 {n}"),
        Lang::En => format!("Channel {n}"),
    }
}


pub fn reconnect_n(n: impl std::fmt::Display) -> String {
    match lang() {
        Lang::Zh => format!("重连 ×{n}"),
        Lang::En => format!("Reconnect ×{n}"),
    }
}

pub fn retry_n(n: impl std::fmt::Display) -> String {
    match lang() {
        Lang::Zh => format!("第 {n} 次重连"),
        Lang::En => format!("Reconnect #{n}"),
    }
}

pub fn days_recent(days: usize, last: &str) -> String {
    match lang() {
        Lang::Zh => format!("{days} 天有录像 · 最近 {last}"),
        Lang::En => format!("{days} day(s) with recordings · latest {last}"),
    }
}

pub fn archive_path(p: &str) -> String {
    match lang() {
        Lang::Zh => format!("存档位置：{p}"),
        Lang::En => format!("Archive: {p}"),
    }
}

pub fn found_path(p: &str) -> String {
    match lang() {
        Lang::Zh => format!("已找到：{p}"),
        Lang::En => format!("Found: {p}"),
    }
}

/// 内部到处用 MB 计数，展示时按大小切成 GB 更直观
fn size_hint(mb: f64) -> String {
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{:.0} MB", mb)
    }
}

pub fn disk_usage_free(used: f64) -> String {
    let u = size_hint(used);
    match lang() {
        Lang::Zh => format!("当前占用 {u}（未设上限）"),
        Lang::En => format!("Using {u} (no limit)"),
    }
}

pub fn disk_usage(used: f64, cap: u64, free: u64) -> String {
    let (u, c, f) = (size_hint(used), size_hint(cap as f64), size_hint(free as f64));
    match lang() {
        Lang::Zh => format!("当前占用 {u} / 上限 {c}，还可写约 {f}"),
        Lang::En => format!("Using {u} of {c} — about {f} left"),
    }
}

/// 快照保存成功
pub fn snapshot_saved(path: &str) -> String {
    match lang() {
        Lang::Zh => format!("快照已保存：{path}"),
        Lang::En => format!("Snapshot saved: {path}"),
    }
}

/// 磁盘剩余空间（跨盘符会不一样，所以单独显示）
pub fn disk_free(mb: f64) -> String {
    let v = size_hint(mb);
    match lang() {
        Lang::Zh => format!("所在磁盘剩余 {v}"),
        Lang::En => format!("{v} free on this disk"),
    }
}

pub fn hardware_dash(label: &str) -> String {
    match lang() {
        Lang::Zh => format!("硬件 · {label}"),
        Lang::En => format!("Hardware · {label}"),
    }
}

pub fn ports(p: &str) -> String {
    match lang() {
        Lang::Zh => format!("端口 {p}"),
        Lang::En => format!("Ports {p}"),
    }
}

pub fn lowered(w: u32, h: u32) -> String {
    match lang() {
        Lang::Zh => format!("已降到 {w}×{h}"),
        Lang::En => format!("Lowered to {w}×{h}"),
    }
}

pub fn hw_enabled(label: &str) -> String {
    match lang() {
        Lang::Zh => format!("已启用硬件解码：{label}"),
        Lang::En => format!("Hardware decoding on: {label}"),
    }
}

pub fn cpu_row(which: CpuRow, pct: f32) -> String {
    let name = tr(match which {
        CpuRow::System => "系统 CPU",
        CpuRow::Main => "主程序",
        CpuRow::App => "软件合计",
    });
    format!("{name}  {pct:.0}%")
}

pub enum CpuRow {
    System,
    Main,
    App,
}

pub fn follow_source_fps(fps: f32) -> String {
    match lang() {
        Lang::Zh => format!("{fps:.1}（跟随源）"),
        Lang::En => format!("{fps:.1} (source)"),
    }
}

pub fn verdict(text: &str) -> String {
    match lang() {
        Lang::Zh => format!("当前判断：{text}"),
        Lang::En => format!("Verdict: {text}"),
    }
}

pub fn detect_unavailable(e: &str) -> String {
    match lang() {
        Lang::Zh => format!("识别不可用：{e}"),
        Lang::En => format!("Detection unavailable: {e}"),
    }
}

pub fn model_ready(model: &str, dev: &str) -> String {
    match lang() {
        Lang::Zh => format!("就绪（{model}{dev}）"),
        Lang::En => format!("Ready ({model}{dev})"),
    }
}

pub fn gpu_suffix() -> &'static str {
    match lang() {
        Lang::Zh => " · 显卡",
        Lang::En => " · GPU",
    }
}

pub fn cpu_suffix() -> &'static str {
    match lang() {
        Lang::Zh => " · CPU",
        Lang::En => " · CPU",
    }
}



/// 「3 段事件」
pub fn event_count(n: u32) -> String {
    match lang() {
        Lang::Zh => format!("{n} 段"),
        Lang::En => format!("{n} clips"),
    }
}

/// 检测负载估算：本机单帧耗时 + N 路并发占单核多少
pub fn detect_load(ms: f32, fps: u32, channels: u32, load: f32) -> String {
    match lang() {
        Lang::Zh => format!(
            "本机预估 {ms:.0} ms/帧 · {channels} 路 @ {fps} FPS 约占 CPU 单核 {load:.0}%"
        ),
        Lang::En => format!(
            "≈{ms:.0} ms/frame here · {channels} channels @ {fps} FPS ≈ {load:.0}% of one CPU core"
        ),
    }
}

pub fn detect_tip(fps: u32, dev: &str) -> String {
    match lang() {
        Lang::Zh => format!("按 {fps} FPS 抽帧做人体/车辆识别（{dev}），只标出在动的目标"),
        Lang::En => format!(
            "Sample frames at {fps} FPS for person/vehicle detection ({dev}); only moving targets are boxed"
        ),
    }
}

// ---------- 运行状态 / 诊断报告 ----------

pub fn runtime_summary(target: &str, w: u32, h: u32, dec: &str) -> String {
    match lang() {
        Lang::Zh => format!("帧率 {target}　送显 {w}×{h}　解码 {dec}"),
        Lang::En => format!("Rate {target} · Display {w}×{h} · Decoder {dec}"),
    }
}

pub fn diag_decode_ratio(ratio: f32, cpu: f32) -> String {
    match lang() {
        Lang::Zh => format!("解码算力吃紧：帧率仅达 {:.0}%，软件占用 {:.0}% CPU", ratio * 100.0, cpu),
        Lang::En => format!("Decoding is CPU-bound: only {:.0}% of the target rate at {:.0}% CPU", ratio * 100.0, cpu),
    }
}

pub fn diag_decode_cpu(cpu: f32) -> String {
    match lang() {
        Lang::Zh => format!("解码算力吃紧：软件占用 {:.0}% CPU", cpu),
        Lang::En => format!("Decoding is CPU-bound: {:.0}% CPU", cpu),
    }
}

pub fn diag_compose(ms: f32) -> String {
    match lang() {
        Lang::Zh => format!("画面合成偏重：单帧纹理上传 {:.1} ms", ms),
        Lang::En => format!("Compositing is heavy: {:.1} ms per texture upload", ms),
    }
}

pub fn diag_unstable(ratio: f32, cpu: f32) -> String {
    match lang() {
        Lang::Zh => format!(
            "取流不稳定：帧率仅达 {:.0}%，但 CPU 仅 {:.0}%（问题在链路或摄像机侧）",
            ratio * 100.0,
            cpu
        ),
        Lang::En => format!(
            "Unstable stream: {:.0}% of the target rate at only {:.0}% CPU (the bottleneck is the link or the camera)",
            ratio * 100.0,
            cpu
        ),
    }
}

pub fn diag_jitter(ms: f32) -> String {
    match lang() {
        Lang::Zh => format!("链路抖动明显：帧间隔波动约 {:.0} ms", ms),
        Lang::En => format!("Noticeable jitter: frame interval varies by about {:.0} ms", ms),
    }
}

pub fn diag_header(ver: &str) -> String {
    match lang() {
        Lang::Zh => format!("CamNexus 诊断报告  {ver} 版本\n"),
        Lang::En => format!("CamNexus diagnostics  version {ver}\n"),
    }
}

pub fn diag_cpu_line(sys: f32, main: f32, app: f32) -> String {
    match lang() {
        Lang::Zh => format!("系统 CPU {sys:.0}%   主程序 {main:.0}%   软件合计 {app:.0}%\n"),
        Lang::En => format!("System CPU {sys:.0}%   Main process {main:.0}%   App total {app:.0}%\n"),
    }
}

pub fn diag_stream_line(
    i: usize,
    live: bool,
    fps: f32,
    target: &str,
    jitter: f32,
    cpu: f32,
    compose: f32,
) -> String {
    let state = if live { tr("在线") } else { tr("离线") };
    match lang() {
        Lang::Zh => format!(
            "通道{i} {state:<4} fps {fps:.1}/{target}  抖动 {jitter:.0}ms  解码CPU {cpu:.0}%  合成 {compose:.1}ms\n"
        ),
        Lang::En => format!(
            "Channel {i} {state:<8} fps {fps:.1}/{target}  jitter {jitter:.0}ms  decode CPU {cpu:.0}%  compose {compose:.1}ms\n"
        ),
    }
}

pub fn diag_verdict(tag: &str, text: &str) -> String {
    match lang() {
        Lang::Zh => format!("当前判断：[{tag}] {text}\n"),
        Lang::En => format!("Verdict: [{tag}] {text}\n"),
    }
}

// ---------- 零散运行期消息 ----------

pub fn rec_from_cli(n: usize) -> String {
    match lang() {
        Lang::Zh => format!("按命令行参数开始录制第 {n} 路"),
        Lang::En => format!("Recording channel {n} (from the command line)"),
    }
}

pub fn ffmpeg_ready(ver: &str) -> String {
    match lang() {
        Lang::Zh => format!("FFmpeg {ver} 已就绪"),
        Lang::En => format!("FFmpeg {ver} is ready"),
    }
}

pub fn version_too_old(raw: &str, maj: u32, min: u32) -> String {
    match lang() {
        Lang::Zh => format!("版本过低（{raw}），需要 ≥ {maj}.{min}"),
        Lang::En => format!("Version too old ({raw}); {maj}.{min} or newer is required"),
    }
}

pub fn ffmpeg_installed(ver: &str, path: &str) -> String {
    match lang() {
        Lang::Zh => format!("ffmpeg {ver} 已安装到 {path}"),
        Lang::En => format!("ffmpeg {ver} installed to {path}"),
    }
}

pub fn file_too_small(got: u64) -> String {
    match lang() {
        Lang::Zh => format!("文件过小（{got} 字节），可能被拦截或重定向"),
        Lang::En => format!("File too small ({got} bytes) — possibly blocked or redirected"),
    }
}

pub fn describe_code(code: impl std::fmt::Display) -> String {
    match lang() {
        Lang::Zh => format!("DESCRIBE 返回 {code}"),
        Lang::En => format!("DESCRIBE returned {code}"),
    }
}

pub fn wrong_credentials(detail: &str) -> String {
    match lang() {
        Lang::Zh => format!("{detail}（账号或密码错误）"),
        Lang::En => format!("{detail} (wrong username or password)"),
    }
}

/// 录像启动失败，附带 ffmpeg 的真实报错（没有报错时退化成原来的提示）
/// 数据栏里的内存占用
pub fn bar_mem(pct: f32) -> String {
    match lang() {
        Lang::Zh => format!("内存 {pct:.0}%"),
        Lang::En => format!("RAM {pct:.0}%"),
    }
}

pub fn rec_fail_detail(n: impl std::fmt::Display, detail: &str) -> String {
    let d = detail.trim();
    if d.is_empty() {
        return match lang() {
            Lang::Zh => format!("录像启动失败（第 {n} 次），ffmpeg 没有输出错误信息"),
            Lang::En => format!("Recording failed to start (attempt {n}), ffmpeg printed nothing"),
        };
    }
    match lang() {
        Lang::Zh => format!("录像启动失败（第 {n} 次）：{d}"),
        Lang::En => format!("Recording failed to start (attempt {n}): {d}"),
    }
}

pub fn rec_gave_up(name: &str) -> String {
    match lang() {
        Lang::Zh => format!("{name}：录像连续失败，已停止"),
        Lang::En => format!("{name}: recording kept failing and was stopped"),
    }
}

/// 网络类失败：说明多久后重试，避免看起来像卡死
pub fn rec_fail_retry(n: impl std::fmt::Display, secs: u64, detail: &str) -> String {
    let d = detail.trim();
    match lang() {
        Lang::Zh => format!("录像启动失败（第 {n} 次，{secs} 秒后重试）：{d}"),
        Lang::En => format!("Recording failed (attempt {n}, retrying in {secs}s): {d}"),
    }
}

/// 段中途断了（不是到点，也不是用户停的）
pub fn rec_cut_short(name: &str, actual_min: f64, expect_min: f64) -> String {
    match lang() {
        Lang::Zh => format!("{name} 提前中断：实录 {actual_min:.0} 分，预期 {expect_min:.0} 分"),
        Lang::En => format!("{name} ended early: {actual_min:.0} min of {expect_min:.0} min"),
    }
}

/// 写入心跳异常：进程活着但文件几乎没长
/// 写入停滞后的重试安排
pub fn rec_stalled_retry(n: impl std::fmt::Display, secs: u64) -> String {
    match lang() {
        Lang::Zh => format!("写入停滞（第 {n} 次，{secs} 秒后重试）"),
        Lang::En => format!("Recording stalled (attempt {n}, retrying in {secs}s)"),
    }
}

pub fn rec_stalled(detail: &str) -> String {
    match lang() {
        Lang::Zh => format!("录像没有在写盘（{detail}），已重启该段"),
        Lang::En => format!("Recording is not hitting the disk ({detail}), restarting"),
    }
}

pub fn rec_finished(name: &str, mb: f64) -> String {
    match lang() {
        Lang::Zh => format!("录像结束 {name}（{mb:.0} MB）"),
        Lang::En => format!("Recording finished {name} ({mb:.0} MB)"),
    }
}

pub fn rec_started(path: &str) -> String {
    match lang() {
        Lang::Zh => format!("开始录像 → {path}"),
        Lang::En => format!("Recording started → {path}"),
    }
}

pub fn quota_cleanup(deleted: usize, mb: f64) -> String {
    match lang() {
        Lang::Zh => format!("容量清理：删除 {deleted} 个最早的文件，释放 {mb:.0} MB"),
        Lang::En => format!("Quota cleanup: deleted {deleted} oldest file(s), freed {mb:.0} MB"),
    }
}

/// 窗口标题（原生标题栏那一串）
pub fn app_title() -> &'static str {
    tr("CamNexus · 监控中枢")
}

// ============================================================ 翻译表

/// (简体中文, English)
const PAIRS: &[(&str, &str)] = &[
    // ---------- 顶栏 / 通用 ----------
    ("监控中枢", "Control Hub"),
    ("CamNexus · 监控中枢", "CamNexus · Control Hub"),
    ("界面语言", "Language"),
    ("裁剪黑边", "Trim black borders"),
    ("自动裁掉画面自带的纯黑边（手机竖屏塞进 16:9 时两侧会有）", "Automatically trim the pure-black borders baked into the stream (common when a phone streams portrait inside a 16:9 frame)"),
    ("一键布局", "Auto layout"),
    ("把所有画面整理成规整网格：等比缩放、贴满屏幕", "Tidy all views into an even grid: scaled to fit and spread across the screen"),
    ("画面卡住，正在重连", "Picture stalled — reconnecting"),
    ("这一路还没有画面", "This channel has no picture yet"),
    ("画面数据不完整", "Frame data is incomplete"),
    ("无法分配图片缓冲", "Cannot allocate image buffer"),
    ("图片编码失败", "Image encoding failed"),
    ("创建快照目录失败", "Cannot create snapshot folder"),
    ("保存快照失败", "Cannot save snapshot"),
    ("旋转预览", "Rotation (preview)"),
    ("不旋转", "None"),
    ("顺时针 90°", "90° clockwise"),
    ("旋转 180°", "180°"),
    ("顺时针 270°", "270° clockwise"),
    ("水平翻转", "Flip horizontally"),
    ("垂直翻转", "Flip vertically"),
    ("比例锁定", "Lock aspect ratio"),
    ("开启后拖动只能等比缩放；关掉可随意拉伸", "When on, resizing keeps the aspect ratio; turn off to stretch freely"),
    ("恢复原比例", "Restore original ratio"),
    ("把框格调回画面本身的宽高比", "Resize this view back to the picture's own aspect ratio"),
    ("单路全屏", "Fullscreen this channel"),
    ("抓拍快照", "Snapshot"),
    ("保存当前原始画面到录像目录下的「快照」", "Save the current raw frame into the “快照” folder under the recording directory"),
    ("重新连接", "Reconnect"),
    ("只重连这一路，其它通道不受影响", "Reconnect only this channel; others keep running"),
    ("0 = 不限制；超出后自动删除最早的文件。\n实际生效值不会超过磁盘剩余空间（给系统留 512 MB）", "0 = no limit; the oldest files are deleted when exceeded.\nThe effective limit never exceeds free disk space (512 MB is kept for the system)"),
    ("已超过磁盘可用空间，实际按剩余空间控制", "exceeds free disk space — the limit is capped to what's left"),
    ("退出全屏（Esc）", "Exit fullscreen (Esc)"),
    ("全屏（画面可拖动 / 缩放）", "Fullscreen — drag and resize the views"),
    ("实验", "Lab"),
    ("实验功能", "Experimental"),
    ("这里的玩意儿还在验证阶段：可能不稳定，也可能在后续版本里改掉或直接移除。\n用不上就别开，开了也不会影响录像和监看", "Everything here is still being validated: it may be unstable, and may change or disappear in a later version.\nLeave it off if you don't need it — recording and live view are unaffected either way."),
    ("启用智能识别", "Enable smart detection"),
    ("需要先下载模型才能启用", "Download the model first"),
    ("检测模型", "Detection model"),
    ("正在下载模型…", "Downloading the model…"),
    ("已安装", "Installed"),
    ("未下载", "Not downloaded"),
    ("下载模型", "Download model"),
    ("约 35.9 MB，从 YOLOX 官方发布地址下载并校验 SHA-256；\n装好的文件放在 %LOCALAPPDATA%\\CamNexus\\models，删掉软件也就干净了", "About 35.9 MB, downloaded from the official YOLOX release and verified with SHA-256.\nStored under %LOCALAPPDATA%\\CamNexus\\models, so uninstalling is always clean."),
    ("模型：YOLOX-s · Apache-2.0", "Model: YOLOX-s · Apache-2.0"),
    ("模型已删除", "Model deleted"),
    ("模型校验失败（文件可能被篡改或下载不完整）", "Model verification failed — the file may be corrupted or tampered with"),
    ("删除失败", "Delete failed"),
    ("子码流", "Substream"),
    ("子码流地址（可选，留空则用主码流）", "Substream URL (optional; empty = main stream)"),
    ("自动", "Auto"),
    ("按常见规则从主码流推导：海康 101→102、201→202；大华 subtype=0→1", "Derive it from the main stream: Hikvision 101→102, 201→202; Dahua subtype=0→1"),
    ("填了之后监看走这一路：旧电脑解 704×576 比解 2560×1440 轻得多，内存和带宽也省；\n录制仍用主码流，画质完全不受影响。代价是子码流分辨率低，放大看会糊、识别也会略降", "When set, preview uses this stream: decoding 704×576 is far cheaper than 2560×1440, saving memory and bandwidth too.\nRecording still uses the main stream, so file quality is untouched. The trade-off: lower resolution looks soft when zoomed, and detection is slightly weaker."),
    ("源画质（不转码，推荐）", "Source quality (no transcode, recommended)"),
    ("「源画质」把摄像机的原始码流直接封装进 mp4，不解码也不转码，CPU 占用接近 0，文件保留摄像机自己的编码（多为 H.265）；\n选具体分辨率会实时转码，画质可控但很吃 CPU——旧电脑做录像机时建议保持源画质", "“Source quality” wraps the camera's original stream into an mp4 without decoding or re-encoding — near-zero CPU, and the file keeps the camera's own codec (usually H.265).\nPicking a specific resolution transcodes in real time: more control, much more CPU. On an older PC keep source quality."),
    ("画面", "Display"),
    ("通用", "General"),
    ("夜视优化", "Night vision boost"),
    ("运行提示", "Performance hints"),
    ("使用 TCP 传输", "Use TCP transport"),
    ("每通道录制参数", "Per-channel recording"),
    ("默认跟随送显分辨率；画质越高越清晰、占空间也越大", "Follows the display resolution by default; higher quality looks better and takes more space"),
    ("H.265 同画质体积约省 40%，但部分系统需装 HEVC 解码包才能播；\nH.264 兼容性最好，任何播放器都能播，代价是文件更大", "H.265: about 40% smaller at the same quality, but some systems need the HEVC codec pack to play it.\nH.264: plays anywhere, at the cost of larger files"),
    ("显卡把推理交给显卡做，上限 25 FPS；CPU 每帧要占一个核的时间片，上限 10 FPS。每帧实际耗时随模型不同，下面的负载估算会按本机算出来",
     "The GPU does the inference (up to 25 FPS); on CPU each frame takes a slice of a core (up to 10 FPS). The real per-frame cost depends on the model — the load estimate below is computed for this machine"),
    ("夜间识别不到目标时开启，会先增强对比度再推理", "Enable if targets are missed at night — contrast is boosted before inference"),
    ("停放很久的车、站着不动的人会暂时不再画框，一旦移动立刻恢复", "Cars parked or people standing still stop being boxed after a while; the box returns as soon as they move"),
    ("网格模式每格的实际像素远小于此值，调高只在双击放大单路时才有意义；\n目标较小的场景（远处的人/车）降低分辨率反而更容易被识别", "Each tile is far smaller than this in grid mode; a higher value only helps when you zoom in.\nFor small targets (distant people or cars) a LOWER resolution actually detects better"),
    ("更稳，推荐；UDP 延迟略低但容易丢包花屏", "More stable — recommended. UDP has slightly lower latency but drops packets"),
    ("留空则每次启动自动查找；找不到会提示下载精简版（约 79 MB）", "Leave empty to auto-detect at startup; if missing, a slim download (≈79 MB) is offered"),
    ("留空 = 用户目录\\Videos\\CamNexus", "Empty = <home>\\Videos\\CamNexus"),
    ("回放打开失败", "Could not open playback"),
    ("退出", "Exit"),
    ("已保存。通道/解码改动需点右上角「重连以应用改动」才生效", "Saved. Channel/decoder changes take effect after “Reconnect to apply”."),
    ("200 · 无需认证", "200 · no auth required"),
    ("200 · 认证通过", "200 · authenticated"),
    ("CamNexus 不自带 FFmpeg（完整包里九成功能用不上）。启动时会先在本机查找，找不到就下载精简版（约 79 MB，国内镜像，几秒到十几秒）。", "CamNexus doesn't bundle FFmpeg (most of it would go unused). At startup it looks for a local copy, and downloads a slim build (≈79 MB from a domestic mirror, a few seconds) if there isn't one."),
    ("回放", "Playback"),
    ("日志", "Logs"),
    ("关于", "About"),
    ("设置", "Settings"),
    ("识别", "Detect"),
    ("识别 · 开", "Detect · On"),
    ("扫描设备", "Scan devices"),
    ("全部重连", "Reconnect all"),
    ("重连以应用改动", "Reconnect to apply"),
    ("设置里改动了通道或解码参数，点击按新设置重建全部视频流", "Channel or decoder settings changed. Click to rebuild every stream."),
    ("未配置通道", "No channels yet"),
    ("摄像头", "Camera"),
    ("加载", "Load"),
    // ---------- 画面格 ----------
    ("连接中", "Connecting"),
    ("连接中…", "Connecting…"),
    ("连接中断，重试中", "Disconnected, retrying"),
    ("还没有视频通道", "No video channels yet"),
    ("点右上角「扫描设备」自动发现局域网摄像机，或到「设置」里手动填写 RTSP 地址", "Use “Scan devices” at the top right to find cameras on your network, or add an RTSP address in Settings."),
    ("开始录制（手动）", "Start recording (manual)"),
    ("停止录制（手动）", "Stop recording (manual)"),
    ("播放这一路的声音", "Play this channel's audio"),
    ("停止播放这一路的声音", "Stop this channel's audio"),
    ("识别不可用", "Detection unavailable"),
    // ---------- 设置：底部按钮 ----------
    ("保存", "Save"),
    ("保存并重连", "Save & reconnect"),
    ("取消", "Cancel"),
    ("新手引导", "Onboarding"),
    ("保存全部设置。主题、音量、提示立即生效；通道与解码参数需重连才生效", "Save every setting. Theme, volume and hints apply immediately; channel and decoder changes need a reconnect."),
    ("重新播放界面导览（不会修改现有设置）", "Replay the interface tour (settings are untouched)"),
    ("通道/解码设置已改动，需「保存并重连」", "Channel/decoder settings changed — use “Save & reconnect”"),
    // ---------- 设置：通道 ----------
    ("视频通道", "Video channels"),
    ("名称", "Name"),
    ("启用", "Enabled"),
    ("删除", "Remove"),
    ("用户名", "Username"),
    ("密码", "Password"),
    ("留空则用地址里自带的账号", "Leave empty to use the credentials in the URL"),
    ("+ 添加通道", "+ Add channel"),
    ("录制", "Recording"),
    ("录像", "Recording"),
    ("源画质（不转码）", "Source quality (no transcode)"),
    ("源画质（不转码，省 CPU）", "Source quality (no transcode, saves CPU)"),
    ("跟随送显", "Match display"),
    ("高画质", "High"),
    ("均衡", "Balanced"),
    ("省空间", "Small size"),
    ("crf 28 / 每 4 秒一个关键帧\n体积最大，崩溃最多丢 4 秒", "crf 28, keyframe every 4 s\nLargest files; a crash loses at most 4 s"),
    ("crf 30 / 每 8 秒一个关键帧\n约省 30%，日常推荐", "crf 30, keyframe every 8 s\n~30% smaller — recommended"),
    ("crf 32 / 每 8 秒一个关键帧\n约省 47%，画质略降", "crf 32, keyframe every 8 s\n~47% smaller, slightly softer"),
    ("录制声音", "Record audio"),
    ("同画质体积约省 40%；部分系统需装 HEVC 解码包才能播", "~40% smaller at the same quality; some systems need the HEVC codec pack to play it"),
    ("兼容性最好，任何播放器都能播，但文件更大", "Plays in any player, but the files are larger"),
    // ---------- 设置：录像 ----------
    ("存档目录", "Archive folder"),
    ("打开目录", "Open folder"),
    ("占用上限", "Storage limit"),
    ("0 = 不限制；超出后自动删除最早的文件", "0 = unlimited; the oldest files are deleted once the limit is hit"),
    ("打开存档目录", "Open archive folder"),
    // ---------- 设置：智能识别 ----------
    ("智能识别", "Smart detection"),
    ("计算设备", "Compute device"),
    ("检测频率", "Detection rate"),
    ("夜视优化（夜间识别不到目标时开启）", "Night vision boost (enable if targets are missed at night)"),
    ("关闭", "Off"),
    ("30 秒", "30 s"),
    ("1 分钟", "1 min"),
    ("5 分钟", "5 min"),
    ("10 分钟", "10 min"),
    ("停放很久的车/站着不动的人会暂时不再画框，一旦移动立刻恢复", "Cars parked or people standing still stop being boxed after a while; the box returns as soon as they move"),
    // ---------- 设置：解码与性能 ----------
    ("解码与性能", "Decoding & performance"),
    ("送显分辨率", "Display resolution"),
    ("解码方式", "Decoder"),
    ("硬件解码只接管「解码」这一步，缩放仍在 CPU——实测比 GPU 内缩放快约 2.4 倍", "Hardware decoding only covers the decode step; scaling stays on the CPU (measured ~2.4× faster than scaling on the GPU)"),
    ("软件解码（CPU）", "Software (CPU)"),
    ("未检测到可用硬件后端", "No hardware backend detected"),
    ("帧率", "Frame rate"),
    ("跟随源（推荐）", "Follow source (recommended)"),
    ("跟随源", "Follow source"),
    ("跟随源可避免复制帧造成的节奏不均；源帧率低于目标时强制提帧只是重复画面", "Following the source avoids uneven pacing; forcing a higher rate only duplicates frames"),
    ("使用 TCP 传输（更稳，推荐）", "Use TCP transport (more stable, recommended)"),
    ("深色主题", "Dark theme"),
    ("画面不流畅或占用过高时给出提示（不会自动改配置）", "Show hints when playback stutters or usage is high (settings are never changed)"),
    ("查看运行状态详情", "View runtime details"),
    ("音量", "Volume"),
    ("摄像机麦克风通常很轻，可适当提高", "Camera microphones are usually quiet — raise this if needed"),
    ("已找到", "Found"),
    ("未找到 ffmpeg.exe", "ffmpeg.exe not found"),
    ("手动指定 ffmpeg.exe 完整路径（可留空自动查找）", "Full path to ffmpeg.exe (leave empty to auto-detect)"),
    ("已保存并重连", "Saved and reconnected"),
    ("设置已保存", "Settings saved"),
    ("重连完成，但保存失败", "Reconnected, but saving failed"),
    // ---------- 扫描窗口 ----------
    ("扫描局域网设备", "Scan the local network"),
    ("网段", "Subnet"),
    ("开始扫描", "Start scan"),
    ("密码只在本机内存中使用，不会上传。注意：多数设备在密码连续错误若干次后会锁定账号一段时间，扫描前请确认密码正确；不确定时可先留空，只探测端口与厂商信息。", "The password stays in local memory and is never uploaded. Note: most cameras lock the account for a while after several wrong passwords, so double-check before scanning; if unsure, leave it empty to only probe ports and vendor info."),
    ("暂无结果", "No results yet"),
    ("添加", "Add"),
    ("已添加通道", "Channel added"),
    ("地址无效", "Invalid address"),
    ("地址格式无法解析", "Cannot parse this address"),
    ("无法解析认证方式", "Cannot determine the auth method"),
    ("无需认证", "no auth required"),
    ("认证通过", "authenticated"),
    ("海康", "Hikvision"),
    ("海康威视", "Hikvision"),
    ("海康 SADP", "Hikvision SADP"),
    ("大华", "Dahua"),
    ("Web 服务", "Web service"),
    ("自动扫描", "Auto scan"),
    ("手动填写", "Enter manually"),
    // ---------- 回放 ----------
    ("选择通道", "Choose a channel"),
    ("录像按「日期 / 通道」存放，点开通道后选择日期即可查看", "Recordings are stored by date / channel. Pick a channel, then a date."),
    ("录像不存在", "No recordings"),
    ("← 返回", "← Back"),
    // ---------- 日志 ----------
    ("按天记录，Markdown 格式。点日期用系统默认程序打开。", "One Markdown file per day. Click a date to open it in your default editor."),
    ("暂无日志", "No logs yet"),
    ("日志目录", "Log folder"),
    ("打开日志目录", "Open log folder"),
    // ---------- 关于 ----------
    ("关于 CamNexus", "About CamNexus"),
    ("把支持 RTSP 的网络摄像机统一到一个窗口监看，支持录像、回放与运行日志。", "Bring every RTSP camera into one window — with recording, playback and runtime logs."),
    ("作者", "Author"),
    ("技术栈", "Built with"),
    ("未安装", "Not installed"),
    // ---------- 欢迎 / 引导 ----------
    ("欢迎使用 CamNexus", "Welcome to CamNexus"),
    ("第一次使用？", "First time here?"),
    ("花一分钟带你认一遍界面，然后添加第一台摄像机。", "A one-minute tour of the interface, then we'll add your first camera."),
    ("演示模式（不连接真实设备，只看讲解）", "Demo mode (no real device, narration only)"),
    ("开始引导", "Start the tour"),
    ("以后再说", "Maybe later"),
    ("演示模式 · 不会修改任何设置", "Demo mode · nothing is changed"),
    ("演示：走一遍添加流程", "Demo: walk through adding a camera"),
    ("跳过这段", "Skip this step"),
    ("完成", "Done"),
    ("上一步", "Back"),
    ("下一步", "Next"),
    ("退出引导", "Exit tour"),
    ("引导语言", "Tour language"),
    ("先花三分钟，之后随便点", "Three minutes now, then click around"),
    ("CamNexus 把局域网里支持 RTSP 的摄像机收进一个窗口。日常只做三件事：看画面、录像、找人找车。这一遍只讲界面怎么用，不会改动你的任何设置。", "CamNexus brings the RTSP cameras on your network into one window. Day to day you only do three things: watch, record, and find people or vehicles. This tour only explains the interface and never changes your settings."),
    ("左上角：一眼看清状态", "Top left: status at a glance"),
    ("默认显示在线通道数（绿点表示全部在线）、当前时间、网络收发、CPU 与内存。点右边那三个点可以逐项开关——勾选后菜单不会关，可以连着勾几个，点空白处才收起。", "Shows live channels (a green dot means all online), the clock, network throughput, CPU and memory. Click the three dots to toggle each item -- the menu stays open while you tick several boxes and closes when you click elsewhere."),
    ("顶栏：一排入口", "Top bar: every entry point"),
    ("从左到右：「回放」打开录像目录；「今日事件」看今天哪些时段有人有车；「识别」开关人体/车辆检测（实验功能，需先在设置里下载模型）；「日志」「关于」；最后是「全部重连」「扫描设备」「设置」。画面卡住时先点「全部重连」，多数情况能自己恢复。", "Left to right: Playback opens the recording folder, Today shows when people and vehicles appeared, Detect toggles person/vehicle detection (experimental -- download a model first), then Logs and About, and finally Reconnect all, Scan devices and Settings. If a picture freezes, hit Reconnect all first."),
    ("画面格子：三处交互", "Video tiles: three interactions"),
    ("双击任意一路放大单看，Esc 退出。右下角两个图标：喇叭控制这一路的声音（同一时刻只播一路）；方块是录制开关——连续录制模式下点一下开、再点一下停，录制中变红；事件录制模式下它只是指示灯，亮了说明正在落盘。右上角圆点是连接状态。开启识别后，人被黄框框住、车辆是绿框。", "Double-click a tile to view it alone; Esc goes back. Two icons sit at the bottom right: the speaker plays this channel's audio (one at a time); the square toggles recording -- in continuous mode click to start or stop and it turns red while recording, in event mode it is only an indicator that lights up while writing. The dot at the top right is the connection state. With detection on, people get yellow boxes and vehicles green ones."),
    ("全屏：格子自己排", "Fullscreen: arrange the tiles yourself"),
    ("鼠标移到画面右下角会出现全屏按钮。进去之后每路都能拖动、拉伸，靠近边缘会吸附对齐；排乱了点「重置布局」。布局会自动记住，下次进来还是这个样子。", "Move the pointer to the bottom-right corner and a fullscreen button appears. Inside, every tile can be dragged and resized and snaps to the edges; if it gets messy, hit Reset layout. The arrangement is remembered next time."),
    ("录像：连续录，还是只留证据", "Recording: keep everything, or only what matters"),
    ("在「设置 → 录像」里切换两种模式。7×24 连续录制：一直写盘，超出容量上限删最旧的，什么都不会漏。事件录制只在有东西动的时候录——人走动、车开过，每段前后各留 5 秒，30 秒内的并成一段；站着不动的模特、海报不会触发，平时一个字节都不写。两种模式占用的相机会话数相同。", "Two modes live in Settings > Recording. 24/7 continuous writes all the time and deletes the oldest clips once the quota is full -- nothing is missed. Event recording runs only while something moves: a person walking, a vehicle driving past. It keeps 5 seconds before and after and merges events less than 30 seconds apart; a mannequin or a poster standing still triggers nothing, and nothing is written in between. Both modes use the same number of camera sessions."),
    ("今日事件：今天录了多少", "Today: how much was recorded"),
    ("「今日事件」列出每段的通道、起止和时长，底部是合计。显示「进行中」表示此刻正在录；显示「中断」说明那段被程序退出打断了——文件还在，只是没写完。", "Today lists each clip's channel, start, end and length with a total at the bottom. In progress means it is being written right now; Interrupted means the app exited mid-clip -- the file is still there, just unfinished."),
    ("添加第一台摄像机", "Add your first camera"),
    ("只要设备支持 RTSP 就能接进来，不限品牌。自动扫描会找出局域网里的设备并试出可用地址，适合不知道地址的情况；手动填写适合你已知地址的场景。", "Any device that speaks RTSP works, whatever the brand. Auto scan finds cameras on your network and works out a usable URL when you don't know it; enter it manually when you do."),
    ("填写网段和账号后点「开始扫描」。密码只在本机内存中使用，不会上传。注意多数设备在密码连续错误若干次后会锁定账号，不确定时可先留空只探端口。", "Fill in the subnet and credentials, then hit Start scan. The password stays in memory and is never uploaded. Note that most cameras lock the account after a few wrong passwords, so leave it blank to only probe ports if unsure."),
    ("在「通道」页填名称、RTSP 地址、账号和密码，地址不确定时可参考右侧常见路径（海康 101/102、大华 subtype=0/1）。填完点「保存并重连」。同一台相机的第二路可以填到「子码流」，只影响看画面时的带宽，录像仍然走主码流。", "Fill in the name, RTSP URL, username and password on the Channels tab; common paths are listed on the right (Hikvision 101/102, Dahua subtype=0/1). Then hit Save & reconnect. The second stream of the same camera can go into Sub stream -- it only saves bandwidth while watching; recording always uses the main stream."),
    ("可以开始用了", "You're all set"),
    ("引导随时能从「设置」底部的「新手引导」重放。录像按「日期/通道」存放，顶栏「回放」直接打开目录。画面卡顿时底部会给出诊断建议。想让软件开机就录，去「设置 → 通用」把「开机自启」和「启动后自动录像」都打开。", "Replay this tour any time from the Onboarding button at the bottom of Settings. Recordings are stored by date and channel, and Playback opens that folder. If a picture stutters, a diagnostic hint appears at the bottom. To record from boot, turn on Start with Windows and Start recording on launch in Settings > General."),
    // ---------- 诊断 ----------
    ("开启硬件解码", "Enable hardware decoding"),
    ("降低送显分辨率", "Lower the display resolution"),
    ("改用 TCP 传输", "Switch to TCP transport"),
    ("尚无采样数据", "No samples yet"),
    ("算力不足", "CPU-bound"),
    ("渲染过重", "Heavy compositing"),
    ("取流不稳", "Unstable stream"),
    ("链路抖动", "Jitter"),
    ("详情", "Details"),
    ("忽略", "Dismiss"),
    ("运行状态详情", "Runtime details"),
    ("系统 CPU", "System CPU"),
    ("主程序", "Main process"),
    ("软件合计", "App total"),
    ("状态", "State"),
    ("抖动", "Jitter"),
    ("解码CPU / 合成", "Decode CPU / Compose"),
    ("在线", "Online"),
    ("离线", "Offline"),
    ("帧率达成率 %（近 60 秒）", "Frame rate achieved % (last 60 s)"),
    ("帧率 fps（近 60 秒）", "Frame rate fps (last 60 s)"),
    ("软件 CPU 占用 %（近 60 秒）", "App CPU % (last 60 s)"),
    ("刚开始采样，请稍候…", "Sampling just started, please wait…"),
    ("复制诊断文本", "Copy diagnostics"),
    ("软件", "Software"),
    // ---------- 运行期提示 ----------
    ("未找到 ffmpeg，无法播放音频", "ffmpeg not found — cannot play audio"),
    ("该通道未启用", "This channel is disabled"),
    ("正在连接音频…", "Connecting audio…"),
    ("扫描完成", "Scan finished"),
    ("未找到", "Not found"),
    ("FFmpeg 已就绪", "FFmpeg is ready"),
    ("准备解码后端", "Preparing the decoding backend"),
    ("下载中，请勿关闭窗口…", "Downloading — don't close the window…"),
    ("正在校验…", "Verifying…"),
    ("下载失败", "Download failed"),
    ("重试", "Retry"),
    ("都打不开？手动下载", "Can't reach any mirror? Download manually"),
    ("自动下载并安装（约 79 MB）", "Download and install automatically (≈79 MB)"),
    ("已经装过 FFmpeg？直接指定路径：", "Already have FFmpeg? Point to it:"),
    ("使用", "Use"),
    ("已切换 ffmpeg", "Switched to ffmpeg"),
    ("本机的 FFmpeg 无法使用", "The local FFmpeg is unusable"),
    ("已是最低档 640×360", "Already at the lowest, 640×360"),
    ("已切换为 TCP 传输", "Switched to TCP transport"),
    ("本机 ffmpeg 未提供可用的硬件解码后端", "This ffmpeg build has no usable hardware decoder"),
    ("已按新设置重连", "Reconnected with the new settings"),
    ("已触发重连", "Reconnect triggered"),
    ("正在加载模型…", "Loading model…"),
    ("未找到模型文件（models/yolox_s.onnx）", "Model file not found (models/yolox_s.onnx)"),
    ("未找到模型文件", "Model file not found"),
    ("显卡", "GPU"),
    // ---------- 识别类别 ----------
    ("人", "Person"),
    ("自行车", "Bicycle"),
    ("车", "Car"),
    ("摩托", "Motorcycle"),
    ("公交", "Bus"),
    ("卡车", "Truck"),
    ("目标", "Object"),
    // ---------- 模块内提示 ----------
    ("正在连接", "Connecting"),
    ("手动重连", "Manual reconnect"),
    ("连接中断，正在重试", "Connection lost, retrying"),
    ("ffmpeg 已退出", "ffmpeg exited"),
    ("无法启动 ffmpeg", "Cannot start ffmpeg"),
    ("在线通道数", "Live channels"),
    ("整机所有物理网卡的合计收发，含系统更新、云同步等后台流量", "Total traffic of all physical adapters, including OS updates and cloud sync"),
    ("数据栏显示", "Show in the data bar"),
    ("数据栏", "Data bar"),
    ("网络速率", "Network rate"),
    ("内存占用", "Memory usage"),
    ("本机", "This machine"),
    ("可用显卡", "Usable GPU"),
    ("按处理器估算", "Estimated on CPU"),
    ("推荐", "Recommended"),
    ("本机预估", "estimated here"),
    ("4 路约", "~4 channels"),
    ("这个模型采用 AGPL-3.0，启用前需要先阅读须知", "This model is AGPL-3.0 — read the notice before enabling"),
    ("阅读许可须知", "Read the licence notice"),
    ("手动下载", "Download manually"),
    ("下载完成", "Download complete"),
    ("打开文件夹", "Open folder"),
    ("下载", "Download"),
    ("下载失败可手动放到：", "If the download fails, put the file in:"),
    ("文件名必须是：", "The file must be named:"),
    ("来源", "Source"),
    ("来源：", "Source: "),
    ("备用源", "fallback"),
    ("已配置", "configured"),
    ("无", "none"),
    ("需要先同意该模型的许可才能启用", "Accept the licence for this model before enabling"),
    ("使用第三方识别模型", "Third-party detection model"),
    ("模型：", "Model: "),
    ("许可证：", "Licence: "),
    ("该模型不属于本程序：我们不打包、不镜像、也不随软件分发它", "This model is not part of this app: we do not bundle it, mirror it or ship it"),
    ("下载地址指向模型作者与官方发布页，由你本人发起下载", "The download goes to the author's official release page and is started by you"),
    ("AGPL-3.0 要求：分发基于该模型的衍生作品时需以 AGPL-3.0 开源", "AGPL-3.0 requires that derivative works you distribute are released under AGPL-3.0"),
    ("将本模型用于闭源商业产品或服务，需向模型作者购买商业授权", "Using this model in a closed-source commercial product or service needs a commercial licence from the author"),
    ("个人使用、学习研究、内部自用不受影响，可随时在设置里删除", "Personal, research and internal use are unaffected; you can delete it at any time"),
    ("下载后会校验 SHA-256，文件被篡改或不完整都会被拒绝。", "The download is verified against a SHA-256 digest; tampered or truncated files are rejected."),
    ("我已阅读并理解上述许可条款", "I have read and understood the licence terms above"),
    ("同意并下载", "Agree and download"),
    ("查看许可证", "View licence"),
    ("模型输出维度不支持", "Unsupported model output shape"),
    ("自定义镜像", "Custom mirror"),
    ("官方源", "Official source"),
    ("备用代理", "Fallback proxy"),
    ("没有可用的下载源", "No usable download source"),
    ("录像方式", "Recording mode"),
    ("7×24 连续录制", "24/7 continuous"),
    ("事件录制（识别到人/车才存盘）", "On event (save only when a person or vehicle is detected)"),
    ("一直在录，超出容量上限后删最旧的。占空间，但不会漏任何画面", "Always recording; the oldest files are removed once the cap is reached. Uses space, misses nothing"),
    ("常连相机但不写盘：画面只在内存里留几秒，识别到目标才把前后几秒存成片段。\n省硬盘、护盘，依赖智能识别", "Stays connected but writes nothing: the last few seconds live in memory and are only saved when a target appears. Saves disk wear and space; depends on detection"),
    ("触发后继续录", "Keep recording for"),
    ("内存缓冲", "Memory buffer"),
    ("缓冲这几秒只占内存（每路约数 MB），不写盘；实际片段起点会对齐到关键帧，可能多几秒", "The buffered seconds stay in RAM (a few MB per channel) and never touch the disk; clips start on a keyframe, so they may run a little longer"),
    ("事件录制模式下由识别自动开关，不需要手动点", "In event mode recording is driven by detection, no manual toggle needed"),
    ("事件片段开始录制", "Event clip started"),
    ("事件片段已保存", "Event clip saved"),
    ("无法创建录像文件", "Cannot create the recording file"),
    ("启动取流进程失败", "Failed to start the stream process"),
    ("等待取流超时", "Timed out waiting for the stream"),
    ("取流停顿，正在重连", "Stream stalled, reconnecting"),
    ("取流进程已退出", "Stream process exited"),
    ("事件录制依赖智能识别：请先到「实验」页启用识别",
     "Event recording depends on detection: enable it on the Experiments tab first"),
    ("事件录制依赖智能识别：请先到「实验」页下载模型并同意许可",
     "Event recording depends on detection: download a model and accept its licence on the Experiments tab first"),
    ("当前配置是事件录制，但识别不可用，因此不会录制任何画面",
     "Event recording is configured but detection is unavailable, so nothing will be recorded"),
    ("识别不可用，已切回 7×24 连续录制",
     "Detection unavailable — switched back to 24/7 continuous recording"),
    ("频率越高框越跟手，但更吃显卡；上限 25（多数相机只有 12–25 帧/秒，再高也是重复帧）",
     "Higher rates track motion more smoothly but cost GPU time; 25 is the ceiling (most cameras emit 12–25 fps, beyond that frames repeat)"),
    ("CPU 每帧都要占一个核的时间片，上限 10；老机器建议 2–3",
     "On CPU each frame takes a slice of a core; the ceiling is 10, and 2–3 is sensible on older machines"),
    ("按这个频率跑不满 4 路，画面会丢帧；建议降低频率或换更小的模型",
     "At this rate four channels cannot keep up and frames will be dropped; lower the rate or pick a smaller model"),
    ("相邻片段合并", "Merge clips closer than"),
    ("同一路两段间隔小于这个值时拼成一个文件，避免遮挡、走走停停切出一堆碎片；0 = 不合并",
     "Two clips on the same channel closer than this are joined into one file, so occlusions and stop-and-go targets don't shatter the timeline; 0 disables merging"),
    ("事件片段已保存（并入上一段）", "Event clip saved (joined to the previous one)"),
    ("全天录像", "Continuous"),
    ("事件片段", "Event clips"),
    ("全天目录", "Open continuous folder"),
    ("事件目录", "Open events folder"),
    ("整点分段的连续录制文件", "Hourly segmented continuous recordings"),
    ("识别到人/车才存下来的短片", "Short clips saved only when a person or vehicle shows up"),
    ("今日事件", "Today's events"),
    ("看今天录了多久、哪些时段有人或车经过", "See how much was recorded today and when something moved"),
    ("今日报告", "Today's report"),
    ("今天没有录像", "No recording today"),
    ("今天还没有检测到人/车", "No person or vehicle detected today"),
    ("进行中", "in progress"),
    ("中断", "interrupted"),
    ("正在录制的通道", "Recording now:"),
    ("报告根据日志生成，日志每天一个文件", "Built from the log, one file per day"),
    ("通道", "Channel"),
    ("开始", "Start"),
    ("结束", "End"),
    ("时长", "Duration"),
    ("检出", "Detected"),
    ("开始事件录像", "started event recording"),
    ("设置已更改", "Settings changed"),
    ("画面已恢复", "Picture restored"),
    ("正在重新连接", "Reconnecting"),
    ("同意并启用", "Agree and enable"),
    ("已同意许可，模型已在本机，可以直接启用",
     "Licence accepted — the model is already on this machine and can be enabled right away"),
    ("已在主界面开启识别", "Detection turned on from the main window"),
    ("已在主界面关闭识别", "Detection turned off from the main window"),
    ("启动时自动开启识别", "Detection resumed on startup"),
    ("识别未自动开启（模型或许可不可用）",
     "Detection not resumed (model or licence unavailable)"),
    ("单段最长", "Max clip length"),
    ("到点强行收尾，等目标离开画面才恢复录制；防止一辆一直停着的车把事件录像变成 24 小时录制。0 = 不限",
     "Force a cut at this length and wait for the target to leave before recording again, so a parked car cannot turn event recording into 24/7 capture. 0 = unlimited"),
    ("单段已达上限，暂停直到车辆离开", "Clip hit the length cap, paused until the vehicle leaves"),
    ("单段已达上限，接着录下一段", "Clip hit the length cap, starting the next one"),
    ("事件录像已接入", "Event recording connected"),
    ("事件录像接入失败（端口或线程）", "Event recording could not start (port or thread)"),
    ("事件录像取流失败", "Event recording stream failed"),
    ("未接入：识别未开启", "Not connected: detection is off"),
    ("未接入：ffmpeg 未就绪", "Not connected: ffmpeg not ready"),
    ("连接未能建立", "connection could not be established"),
    ("「CPU 单核」指处理器的一个逻辑核，和显卡的 CUDA 核心无关；显卡模式下 CPU 只做预处理，单帧约 2 ms",
     "\"One CPU core\" means one logical CPU thread — nothing to do with CUDA cores. On GPU the CPU only does preprocessing, about 2 ms per frame"),
    ("速度", "speed"),
    ("启动与运行", "Startup & runtime"),
    ("开机自启", "Start with Windows"),
    ("进程守护", "Process watchdog"),
    ("需先开启开机自启", "Needs startup enabled"),
    ("启动后自动录像", "Start recording on launch"),
    ("这一路没有画面，无法开始录像", "No picture on this channel — cannot start recording"),
    ("无法定位程序路径，开机自启未生效", "Cannot resolve the executable path; startup entry not written"),
    ("开机自启设置失败", "Failed to update the startup entry"),
    ("开机自启写在当前用户的启动项里，不需要管理员权限", "Written to the current user's startup entries; no admin rights needed"),
    ("登录 Windows 后自动运行本程序，不弹窗、不请求管理员权限", "Launches after you sign in — no window prompt, no admin request"),
    ("由守护进程看住主程序：异常退出会立刻重新拉起；正常关窗口则一起退出", "A watchdog relaunches the app if it crashes; closing the window stops both"),
    ("程序启动后自动开始录制所有已启用通道；画面未连上的通道会等它上线后再开录", "Records every enabled channel on launch; channels that are offline start once they come online"),
    ("CPU 占用", "CPU usage"),
    ("时间", "Clock"),
    ("手动添加", "Add manually"),
    ("扫描局域网自动发现摄像机，或者手动填写一个 RTSP 地址", "Scan the network for cameras, or enter an RTSP address yourself"),
    ("该通道未连接", "This channel is not connected"),
    ("这一路没有音轨", "This channel has no audio track"),
    ("音频不可用，已切回纯画面", "Audio unavailable — switched back to video only"),
    ("录像启动失败", "Recording failed to start"),
    ("录像连续失败，已停止", "recording kept failing and was stopped"),
    ("录像结束", "Recording finished"),
    ("开始录像", "Recording started"),
    ("容量清理：删除", "Quota cleanup: deleted"),
    ("创建录像目录失败", "Cannot create the recording folder"),
    ("启动 ffmpeg 失败", "Cannot start ffmpeg"),
    ("打开失败", "Cannot open"),
    ("打开资源管理器失败", "Cannot open Explorer"),
    ("打开目录失败", "Cannot open the folder"),
    ("音频设备不可用", "Audio device unavailable"),
    ("音频流结束", "Audio stream ended"),
    ("初始化 ONNX Runtime 失败", "Cannot initialise ONNX Runtime"),
    ("加载模型失败", "Cannot load the model"),
    ("构造输入失败", "Cannot build the input tensor"),
    ("推理失败", "Inference failed"),
    ("读取输出失败", "Cannot read the output"),
    ("输出内存不连续", "Output buffer is not contiguous"),
    ("Direct3D 11（通用）", "Direct3D 11 (generic)"),
    ("DXVA2（旧）", "DXVA2 (legacy)"),
    ("其他", "Other"),
    ("npmmirror（国内）", "npmmirror (China)"),
    ("无法执行", "Cannot run"),
    ("ffmpeg 返回非零退出码", "ffmpeg exited with a non-zero code"),
    ("无法解析版本行", "Cannot parse the version line"),
    ("无法解析主版本号", "Cannot parse the major version"),
    ("版本过低", "Version too old"),
    ("准备中", "Preparing"),
    ("找不到可写的安装目录", "No writable install folder"),
    ("创建目录失败", "Cannot create the folder"),
    ("保存失败", "Save failed"),
    ("下载到的文件不可用", "The downloaded file is unusable"),
    ("切换备用源…", "Switching to a backup mirror…"),
    ("请求失败", "Request failed"),
    ("创建临时文件失败", "Cannot create a temporary file"),
    ("读取中断", "Read interrupted"),
    ("写入失败", "Write failed"),
    ("落盘失败", "Flush failed"),
    ("文件过小", "File too small"),
    ("可能被拦截或重定向", "possibly blocked or redirected"),
    ("连接失败", "Connection failed"),
    ("DESCRIBE 返回", "DESCRIBE returned"),
    ("账号或密码错误", "wrong username or password"),
    ("配置文件解析失败", "Cannot parse the config file"),
    ("使用默认配置", "using defaults"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zh_returns_source() {
        set_lang(Lang::Zh);
        assert_eq!(tr("设置"), "设置");
        set_lang(Lang::En);
        assert_eq!(tr("设置"), "Settings");
        set_lang(Lang::Zh);
    }

    #[test]
    fn unknown_falls_back_to_source() {
        set_lang(Lang::En);
        assert_eq!(tr("这句没有翻译"), "这句没有翻译");
        set_lang(Lang::Zh);
    }

    #[test]
    fn table_has_no_duplicate_keys() {
        let mut seen = std::collections::HashSet::new();
        for (zh, _) in PAIRS {
            assert!(seen.insert(*zh), "重复的中文 key：{zh}");
        }
    }

    #[test]
    fn every_pair_is_translated() {
        for (zh, en) in PAIRS {
            assert!(!zh.is_empty() && !en.is_empty(), "空条目：{zh}");
        }
    }
}
