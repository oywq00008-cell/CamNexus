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
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct DetectCfg {
    pub device: DetectDevice,
    /// 检测频率（仅显卡模式生效；CPU 固定 1）
    pub fps: u32,
    /// 夜视优化：CLAHE 对比度增强
    pub night_opt: bool,
    /// 静止物体豁免秒数，0 = 不豁免
    pub static_grace_secs: u32,
    /// 实验功能：用户是否在设置里启用了识别。
    /// 关着的时候主界面不出现识别按钮，整个识别链路也不启动。
    #[serde(default)]
    pub enabled: bool,
}

impl Default for DetectCfg {
    fn default() -> Self {
        Self {
            device: DetectDevice::Cpu,
            fps: 1,
            night_opt: false,
            static_grace_secs: 60,
            enabled: false,
        }
    }
}

impl DetectCfg {
    /// 实际生效的检测频率
    pub fn effective_fps(&self) -> u32 {
        match self.device {
            DetectDevice::Cpu => 1,
            DetectDevice::Gpu => self.fps.clamp(1, 5),
        }
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
        }
    }
}

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
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

    /// CPU 恒为 1 FPS；显卡模式只允许 1–5 的整数，越界一律夹回
    #[test]
    fn detect_fps_is_one_to_five_integers() {
        let mut c = DetectCfg {
            device: DetectDevice::Cpu,
            fps: 5,
            ..Default::default()
        };
        assert_eq!(c.effective_fps(), 1, "CPU 模式必须固定 1 FPS");

        c.device = DetectDevice::Gpu;
        for (input, want) in [(1, 1), (2, 2), (3, 3), (4, 4), (5, 5), (0, 1), (9, 5)] {
            c.fps = input;
            assert_eq!(c.effective_fps(), want, "fps={input} 应夹到 {want}");
        }
    }

    /// 老配置文件没有 detect 字段时，要落回 CPU / 1 FPS / 不增强 / 1 分钟豁免
    #[test]
    fn old_config_without_detect_falls_back() {
        let cfg: AppConfig = serde_json::from_str(r#"{"channels":[],"dark":true}"#).unwrap();
        assert_eq!(cfg.detect.device, DetectDevice::Cpu);
        assert_eq!(cfg.detect.effective_fps(), 1);
        assert!(!cfg.detect.night_opt);
        assert_eq!(cfg.detect.static_grace_secs, 60);
    }
}
