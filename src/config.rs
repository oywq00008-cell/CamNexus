//! 配置读写与 FFmpeg 定位

use crate::i18n::tr;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Channel {
    pub name: String,
    /// RTSP 地址。未填账号时形如 rtsp://192.168.1.10:554/Streaming/Channels/102
    pub url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// 这一路的录像设置
    #[serde(default)]
    pub record: RecordCfg,
    /// 子码流地址（可选）。只用于监看：旧机器解 704×576 比解 2560×1440 轻得多。
    /// 留空表示不用，监看也走主码流；录制永远用主码流，不受这里影响。
    #[serde(default)]
    pub sub_url: String,
    /// 预览旋转角度：0 / 90 / 180 / 270。
    /// **只影响显示**，录制始终是摄像机原始码流（copy），不会跟着转。
    #[serde(default)]
    pub rotate: u16,
    /// 预览水平翻转
    #[serde(default)]
    pub flip_h: bool,
    /// 预览垂直翻转
    #[serde(default)]
    pub flip_v: bool,
    /// 比例锁定：开时拖动缩放只能等比放大缩小，关时可随意拉伸
    #[serde(default = "yes")]
    pub aspect_lock: bool,
    /// 裁掉画面自带黑边（手机竖屏塞进 16:9 时两侧会有纯黑边）。
    /// 开启后自动检测内容区，按真实比例参与布局和旋转。
    #[serde(default)]
    pub crop_borders: bool,
}

/// 录像编码方式
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum RecCodec {
    /// H.265 / HEVC：同画质体积约省 40%，但部分播放器/系统需要额外解码包
    #[default]
    H265,
    /// H.264：兼容性最好，体积更大
    H264,
}

/// 录像质量档位：直接决定 CRF 与关键帧间隔
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum RecQuality {
    /// 高画质：crf 28 / 关键帧 50 帧（崩溃最多丢 4 秒）
    High,
    /// 均衡：crf 30 / 关键帧 100 帧（默认，体积约为高画质的 70%）
    #[default]
    Balanced,
    /// 省空间：crf 32 / 关键帧 100 帧（体积约为高画质的 53%）
    Small,
}

impl RecQuality {
    pub fn label(self) -> &'static str {
        match self {
            Self::High => tr("高画质"),
            Self::Balanced => tr("均衡"),
            Self::Small => tr("省空间"),
        }
    }

    /// (CRF, 关键帧间隔)
    pub fn params(self, codec: RecCodec) -> (u32, u32) {
        match (self, codec) {
            (Self::High, RecCodec::H265) => (28, 50),
            (Self::High, RecCodec::H264) => (23, 50),
            (Self::Balanced, RecCodec::H265) => (30, 100),
            (Self::Balanced, RecCodec::H264) => (25, 100),
            (Self::Small, RecCodec::H265) => (32, 100),
            (Self::Small, RecCodec::H264) => (27, 100),
        }
    }
}

/// 识别计算设备
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum DetectDevice {
    /// 纯 CPU，固定 1 帧/秒
    #[default]
    Cpu,
    /// 显卡（DirectML），1–5 帧/秒可调
    Gpu,
}

/// 智能识别设置
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct DetectCfg {
    pub device: DetectDevice,
    /// 检测频率。CPU 上限 10、显卡上限 25，实际用 `effective_fps()` 取夹紧后的值
    pub fps: u32,
    /// 夜视优化：CLAHE 对比度增强
    pub night_opt: bool,
    /// 实验功能：用户是否在设置里启用了识别。
    /// 关着的时候主界面不出现识别按钮，整个识别链路也不启动。
    #[serde(default)]
    pub enabled: bool,
    /// 选中的模型 id（见 `model::MODELS`）
    #[serde(default = "default_model")]
    pub model: String,
    /// 已阅读并同意许可的模型 id。
    /// AGPL 的模型必须出现在这里，否则不允许下载、也不允许启用识别。
    #[serde(default)]
    pub accepted: Vec<String>,
    /// 上次退出时识别是开着的。
    ///
    /// 与 `enabled` 分开：`enabled` 是「这个功能能不能用」（设置里的实验开关），
    /// `running` 是「现在开着没有」。崩溃后守护进程把程序拉起来时靠它恢复，
    /// 否则事件录像会因为识别没开而彻底失效。
    #[serde(default = "default_true")]
    pub running: bool,
}

fn default_true() -> bool {
    true
}

fn default_model() -> String {
    "yolo26n".to_string()
}

impl Default for DetectCfg {
    fn default() -> Self {
        Self {
            device: DetectDevice::Cpu,
            fps: 3,
            night_opt: false,
            enabled: false,
            model: default_model(),
            accepted: Vec::new(),
            running: true,
        }
    }
}

impl DetectCfg {
    /// 实际生效的检测频率
    /// CPU 与显卡的上限不同：CPU 上每帧都要占一个核的时间片，
    /// 显卡是异步提交，能压得更密。上限只是护栏，实际能跑多少由模型决定。
    pub fn max_fps(&self) -> u32 {
        match self.device {
            DetectDevice::Cpu => 10,
            DetectDevice::Gpu => 25,
        }
    }

    pub fn effective_fps(&self) -> u32 {
        self.fps.clamp(1, self.max_fps())
    }
}

/// 单通道录像设置
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct RecordCfg {
    /// 录像分辨率；两个都为 0 表示跟随送显分辨率
    pub width: u32,
    pub height: u32,
    /// 是否录制声音
    pub audio: bool,
    /// 源画质：直接拷贝码流，不转码（分辨率跟随摄像机）
    pub copy: bool,
    /// 转码时使用的编码器
    pub codec: RecCodec,
    /// 画质档位
    pub quality: RecQuality,
}

impl Default for RecordCfg {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            audio: true,
            // 默认不转码：直接把摄像机码流封装进 mp4，CPU 占用接近 0。
            // 旧电脑做录像机时，转码是最容易把 CPU 吃满的一步，想要兼容性
            // 再进设置手动改（会明确提示代价）。
            copy: true,
            codec: RecCodec::H265,
            quality: RecQuality::Balanced,
        }
    }
}

fn yes() -> bool {
    true
}

/// 仅保留 URL 未保留字符，其余全部百分号编码
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl Channel {
    /// 主码流地址（录制、探测用），账号密码会先做百分号编码
    pub fn rtsp(&self) -> String {
        with_creds(&self.url, &self.username, &self.password)
    }

    /// 监看地址：填了子码流就用子码流，否则回落主码流。
    ///
    /// 这一路只影响画面预览和识别，**录制永远走主码流**（`rtsp()`），
    /// 所以旧机器上开子码流不会让录像变糊。
    pub fn display_rtsp(&self) -> String {
        let src = if self.sub_url.trim().is_empty() {
            &self.url
        } else {
            &self.sub_url
        };
        with_creds(src, &self.username, &self.password)
    }

    /// 是否在用独立的子码流
    pub fn uses_substream(&self) -> bool {
        !self.sub_url.trim().is_empty()
    }
}

fn with_creds(raw: &str, user: &str, pass: &str) -> String {
    let url = raw.trim().to_string();
    if user.is_empty() || url.contains('@') {
        return url;
    }
    match url.find("://") {
        Some(i) => {
            let (scheme, rest) = url.split_at(i + 3);
            format!("{scheme}{}:{}@{rest}", enc(user), enc(pass))
        }
        None => url,
    }
}

/// 按常见规则从主码流推导子码流地址，认不出来就返回 None。
///
/// - 海康：末段三位数且末位为奇数（101/201 → 102/202）
/// - 大华：`subtype=0` → `subtype=1`
///
/// 只覆盖这两家最常见的形式，其余让用户自己填——猜错了比不猜更糟。
pub fn derive_sub_url(main: &str) -> Option<String> {
    let u = main.trim();
    if u.is_empty() {
        return None;
    }
    // 大华：查询串里的 subtype
    if u.contains("subtype=0") {
        return Some(u.replacen("subtype=0", "subtype=1", 1));
    }
    // 海康：路径最后一段是三位数
    let pos = u.rfind('/')?;
    let (head, tail) = u.split_at(pos + 1);
    let tail = tail.trim_end_matches('/');
    if tail.len() == 3 && tail.bytes().all(|b| b.is_ascii_digit()) {
        let last = tail.as_bytes()[2];
        if (last - b'0') % 2 == 1 {
            let mut out = String::from(head);
            out.push_str(&tail[..2]);
            out.push((last + 1) as char);
            return Some(out);
        }
    }
    None
}

/// 顶栏数据栏显示哪些项目
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BarCfg {
    /// 在线通道数（默认）
    pub online: bool,
    /// 本机时间
    pub clock: bool,
    /// 本机上行/下行速率
    pub net: bool,
    /// 整机 CPU 占用
    pub cpu: bool,
    /// 物理内存占用
    pub mem: bool,
}

impl Default for BarCfg {
    fn default() -> Self {
        // 默认只显示在线数：数据栏是给人用的，一上来塞满反而看不清
        Self {
            online: true,
            clock: false,
            net: false,
            cpu: false,
            mem: false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct AppConfig {
    pub channels: Vec<Channel>,
    pub ffmpeg_path: String,
    /// 解码输出尺寸（送显分辨率，非拉流分辨率）
    pub render_width: u32,
    pub render_height: u32,
    pub fps: u32,
    /// true = RTSP over TCP
    pub tcp: bool,
    pub dark: bool,
    /// 硬件解码后端：none / cuda / d3d11va / qsv / amf / d3d12va …
    pub hwdec: String,
    /// 是否显示运行时性能提示条
    pub show_hints: bool,
    /// 音量倍数，1.0 = 原音量；摄像机麦克风通常很轻，允许 >1 做增益
    pub volume: f32,
    /// 新手引导状态：只自动询问一次
    pub onboarding: Onboarding,
    /// 录像存档根目录，空 = 用户目录\Videos\CamNexus
    pub record_dir: String,
    /// 录像占用上限（MB），0 = 不限
    pub record_max_mb: u64,
    /// 全屏自由布局：每路一个归一化矩形 [x, y, w, h]（相对画面区）。
    /// 数量对不上通道数时会被重排。
    #[serde(default)]
    pub tiles: Vec<[f32; 4]>,
    /// 智能识别设置
    pub detect: DetectCfg,
    /// 界面语言
    pub lang: crate::i18n::Lang,
    /// 顶栏数据栏显示项
    pub bar: BarCfg,
    /// 开机自启（写 HKCU 的 Run 项）
    pub autostart: bool,
    /// 进程守护：崩溃后由守护进程自动拉起
    pub guard: bool,
    /// 启动软件后自动开始录像（只在摄像机在线时才真正开录）
    #[serde(default)]
    pub auto_record: bool,
    /// 录像方式：7×24 连续录，还是只在检测到人/车时存盘
    #[serde(default = "default_record_mode")]
    pub record_mode: RecordMode,
    /// 事件录像：触发前保留多少秒（内存缓冲，不落盘）
    #[serde(default = "default_event_pre")]
    pub event_pre_secs: u32,
    /// 事件录像：目标消失后继续录多少秒
    #[serde(default = "default_event_post")]
    pub event_post_secs: u32,
    /// 事件录像：多少置信度以上才算触发（比画框门槛高，压误报）
    #[serde(default = "default_event_conf")]
    pub event_conf: f32,
    /// 事件录像：同一路两段间隔小于这个秒数就合并成一个文件；0 = 不合并
    #[serde(default = "default_merge_gap")]
    pub event_merge_gap_secs: u32,
    /// 事件录像：单段最长多少秒，到点强制收尾并等目标离开再恢复；0 = 不限
    ///
    /// 没有这个上限，一辆一直停在画面里的车会被持续检出、一直录下去，
    /// 事件录像就退化成了 24 小时录制。
    #[serde(default = "default_max_clip")]
    pub event_max_clip_secs: u32,
}

fn default_merge_gap() -> u32 {
    30
}

fn default_max_clip() -> u32 {
    600
}

fn default_event_conf() -> f32 {
    0.45
}

fn default_record_mode() -> RecordMode {
    RecordMode::Continuous
}
fn default_event_pre() -> u32 {
    5
}
fn default_event_post() -> u32 {
    5
}

/// 录像方式
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordMode {
    /// 7×24 连续录制（原有行为）
    #[default]
    Continuous,
    /// 事件录制：常连但只在检测到目标时落盘
    Event,
}

/// 新手引导状态
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Onboarding {
    /// 还没问过（首次启动）
    #[default]
    NotAsked,
    /// 问过了，用户选择跳过
    Declined,
    /// 走完了引导
    Done,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            channels: vec![],
            ffmpeg_path: String::new(),
            render_width: 960,
            render_height: 540,
            // 0 = 跟随源帧率，避免复制帧造成的节奏不均
            fps: 0,
            tcp: true,
            dark: true,
            hwdec: "none".into(),
            show_hints: true,
            volume: 1.5,
            onboarding: Onboarding::NotAsked,
            record_dir: String::new(),
            // 500 GB：监控录像按天攒，默认给宽一点，
            // 真正写多少由磁盘剩余空间兜底（见 record::effective_limit_mb）
            record_max_mb: 512_000,
            tiles: Vec::new(),
            detect: DetectCfg::default(),
            lang: crate::i18n::Lang::Zh,
            bar: BarCfg::default(),
            autostart: false,
            guard: false,
            auto_record: false,
            record_mode: RecordMode::Continuous,
            event_pre_secs: 5,
            event_post_secs: 5,
            event_conf: 0.45,
            event_merge_gap_secs: 30,
            event_max_clip_secs: 600,
        }
    }
}

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 数据目录：日志、守护标记、模型缓存都放这里
pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(|p| PathBuf::from(p).join("CamNexus"))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn config_path() -> PathBuf {
    exe_dir().join("config.json")
}

impl AppConfig {
    pub fn load() -> Self {
        let p = config_path();
        match std::fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                eprintln!("配置文件解析失败（{e}），使用默认配置");
                AppConfig::default()
            }),
            Err(_) => {
                // 首次运行：在 exe 同级生成一份默认配置，方便用户直接编辑
                // （发布包中不含 config.json，避免携带任何真实凭据）
                let cfg = AppConfig::default();
                if let Ok(json) = serde_json::to_string_pretty(&cfg) {
                    let _ = std::fs::write(&p, json);
                }
                cfg
            }
        }
    }

    pub fn save(&self) -> anyhow::Result<PathBuf> {
        let p = config_path();
        std::fs::write(&p, serde_json::to_string_pretty(self)?)?;
        Ok(p)
    }
}

/// 探测 ffmpeg 编入的硬件解码后端
pub fn detect_hwaccels(ffmpeg: &Path) -> Vec<String> {
    use std::process::Command;
    let Ok(out) = Command::new(ffmpeg)
        .args(["-hide_banner", "-hwaccels"])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut list = Vec::new();
    let mut started = false;
    for line in text.lines() {
        if !started {
            if line.contains("Hardware acceleration methods") {
                started = true;
            }
            continue;
        }
        let name = line.trim();
        if name.is_empty() || name.contains(':') {
            continue;
        }
        list.push(name.to_string());
    }
    list
}

/// 后端名的中文说明
pub fn hwdec_label(name: &str) -> &'static str {
    match name {
        "cuda" => "NVIDIA CUDA（NVDEC）",
        "d3d11va" => tr("Direct3D 11（通用）"),
        "d3d12va" => "Direct3D 12",
        "qsv" => "Intel Quick Sync",
        "amf" => "AMD AMF",
        "dxva2" => tr("DXVA2（旧）"),
        "vaapi" => "VAAPI（Linux）",
        "videotoolbox" => "VideoToolbox（macOS）",
        "vulkan" => "Vulkan",
        _ => tr("其他"),
    }
}

/// 按优先级定位 ffmpeg：配置 → exe 同级 → PATH → 常见安装路径
pub fn find_ffmpeg(preferred: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };

    if !preferred.trim().is_empty() {
        let p = PathBuf::from(preferred.trim());
        if p.is_file() {
            return Some(p);
        }
    }

    // 自动下载安装的位置（%LOCALAPPDATA%\CamNexusin）
    if let Some(p) = crate::fetch::install_path()
        && p.is_file() {
            return Some(p);
        }

    let dir = exe_dir();
    for cand in [
        dir.join(exe),
        dir.join("ffmpeg").join(exe),
        dir.join("ffmpeg").join("bin").join(exe),
    ] {
        if cand.is_file() {
            return Some(cand);
        }
    }

    if let Some(paths) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&paths) {
            let p = d.join(exe);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    #[cfg(windows)]
    {
        let mut roots: Vec<PathBuf> = vec![
            PathBuf::from(r"C:\ffmpeg\bin").join(exe),
            PathBuf::from(r"C:\Program Files\ffmpeg\bin").join(exe),
        ];
        // 兼容 C:\ffmpeg\ffmpeg-x.y-xxx\bin\ffmpeg.exe 这类带版本号的目录
        if let Ok(rd) = std::fs::read_dir(r"C:\ffmpeg") {
            for e in rd.flatten() {
                roots.push(e.path().join("bin").join(exe));
            }
        }
        for c in roots {
            if c.is_file() {
                return Some(c);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        for c in ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg"] {
            let p = PathBuf::from(c);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for c in ["/usr/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/snap/bin/ffmpeg"] {
            let p = PathBuf::from(c);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_url_with_encoded_credentials() {
        let c = Channel {
            name: "t".into(),
            url: "rtsp://192.168.1.10:554/live".into(),
            username: "admin".into(),
            password: "p@ss:1/2".into(),
            enabled: true,
            record: Default::default(),
            rotate: 0,
            flip_h: false,
            flip_v: false,
            aspect_lock: true,
            crop_borders: false,
            sub_url: String::new(),
        };
        assert_eq!(c.rtsp(), "rtsp://admin:p%40ss%3A1%2F2@192.168.1.10:554/live");
    }

    #[test]
    fn keeps_url_when_no_credentials() {
        let c = Channel {
            name: "t".into(),
            url: "rtsp://192.168.1.10:554/live".into(),
            username: String::new(),
            password: String::new(),
            enabled: true,
            record: Default::default(),
            rotate: 0,
            flip_h: false,
            flip_v: false,
            aspect_lock: true,
            crop_borders: false,
            sub_url: String::new(),
        };
        assert_eq!(c.rtsp(), "rtsp://192.168.1.10:554/live");
    }

    /// 帧率：CPU 上限 10、显卡上限 25，越界一律夹回
    #[test]
    fn detect_fps_is_clamped_per_device() {
        let mut c = DetectCfg {
            device: DetectDevice::Cpu,
            fps: 6,
            ..Default::default()
        };
        assert_eq!(c.max_fps(), 10);
        assert_eq!(c.effective_fps(), 6, "CPU 模式允许高于 1 FPS");
        for (input, want) in [(0, 1), (1, 1), (10, 10), (11, 10), (99, 10)] {
            c.fps = input;
            assert_eq!(c.effective_fps(), want, "CPU fps={input} 应夹到 {want}");
        }

        c.device = DetectDevice::Gpu;
        assert_eq!(c.max_fps(), 25);
        for (input, want) in [(1, 1), (8, 8), (25, 25), (26, 25), (100, 25)] {
            c.fps = input;
            assert_eq!(c.effective_fps(), want, "显卡 fps={input} 应夹到 {want}");
        }
    }

    /// 老配置文件没有 detect 字段时，要落回默认值
    #[test]
    fn old_config_without_detect_falls_back() {
        let cfg: AppConfig = serde_json::from_str(r#"{"channels":[],"dark":true}"#).unwrap();
        assert_eq!(cfg.detect.device, DetectDevice::Cpu);
        assert_eq!(cfg.detect.effective_fps(), 3, "默认帧率");
        assert!(!cfg.detect.night_opt);
        assert!(!cfg.detect.enabled);
    }
}
