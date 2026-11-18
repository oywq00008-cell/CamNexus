//! 检测模型的清单、按需下载、以及按硬件推荐。
//!
//! 模型不进安装包：最小的也有 9.4 MB，多数用户根本不会开识别。
//! 用户只有在「设置 → 实验」里主动点下载，才会落到 `%LOCALAPPDATA%\CamNexus\models\`。
//!
//! 下载完必须校验 SHA-256：模型文件是直接喂给推理引擎的，被替换了不会有任何
//! 提示，只会「识别变差」——这种问题极难查。
//!
//! 许可：YOLO26 系列是 AGPL-3.0（模型文件里自带这条声明），因此要求先看须知
//! 并勾选同意；YOLOX 是 Apache-2.0，没有这个限制。

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::i18n::tr;
use crate::sys::Hardware;

/// 推荐预算：4 路 × 1 FPS = 单帧 250 ms，取一半给画面解码留余量。
pub const BUDGET_MS: f32 = 125.0;

/// 外推基准：9800X3D 的 Cinebench R23 多核分数。
const CPU_REF_MT: f32 = 23_163.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum License {
    Apache,
    Agpl,
}

impl License {
    pub fn label(self) -> &'static str {
        match self {
            License::Apache => "Apache-2.0",
            License::Agpl => "AGPL-3.0",
        }
    }
    /// AGPL 必须先看须知并勾选，其余不需要
    pub fn needs_notice(self) -> bool {
        self == License::Agpl
    }
}

pub struct Spec {
    pub id: &'static str,
    pub name: &'static str,
    pub file: &'static str,
    /// 官方源
    pub url: &'static str,
    /// 备用源（第三方 GitHub 代理）。内容由 SHA-256 兜底，走谁都不影响校验。
    pub fallback: &'static str,
    pub sha256: &'static str,
    pub size: u64,
    pub input: usize,
    pub license: License,
    pub vendor: &'static str,
    /// 9800X3D 上的 CPU 单帧耗时（实测），作为外推基准
    pub cpu_ref_ms: f32,
    /// 显卡拟合式 t = a + b / TFLOPS
    pub gpu_a: f32,
    pub gpu_b: f32,
}

pub const YOLOX_S: Spec = Spec {
    id: "yolox_s",
    name: "YOLOX-s",
    file: "yolox_s.onnx",
    url: "https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.0/yolox_s.onnx",
    fallback: "https://ghproxy.net/https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.0/yolox_s.onnx",
    sha256: "50f9715c9eb6831666a3c1f84c36cbcd34b2fc6db8c02c8af0f330b1b3fbb3ff",
    size: 35_858_697,
    input: 640,
    license: License::Apache,
    vendor: "Megvii",
    cpu_ref_ms: 46.0,
    gpu_a: 3.0,
    gpu_b: 88.0,
};

pub const YOLO26N: Spec = Spec {
    id: "yolo26n",
    name: "YOLO26n",
    file: "yolo26n.onnx",
    url: "https://github.com/ultralytics/assets/releases/download/v8.4.0/yolo26n.onnx",
    fallback: "https://ghproxy.net/https://github.com/ultralytics/assets/releases/download/v8.4.0/yolo26n.onnx",
    sha256: "2e947b787d9e787b93a16772a5f55b1d4d8c4d86f53146149c5d6a642442d6f7",
    size: 9_941_957,
    input: 640,
    license: License::Agpl,
    vendor: "Ultralytics",
    cpu_ref_ms: 17.0,
    gpu_a: 3.3,
    gpu_b: 32.6,
};

pub const YOLO26L: Spec = Spec {
    id: "yolo26l",
    name: "YOLO26l",
    file: "yolo26l.onnx",
    url: "https://github.com/ultralytics/assets/releases/download/v8.4.0/yolo26l.onnx",
    fallback: "https://ghproxy.net/https://github.com/ultralytics/assets/releases/download/v8.4.0/yolo26l.onnx",
    sha256: "d32e8d2b3e5c7c591865d372712e42a88ff5bbc518f3bd83ad3621668de89f7a",
    size: 99_613_335,
    input: 640,
    license: License::Agpl,
    vendor: "Ultralytics",
    cpu_ref_ms: 133.0,
    gpu_a: 5.9,
    gpu_b: 183.0,
};

pub const YOLO26X: Spec = Spec {
    id: "yolo26x",
    name: "YOLO26x",
    file: "yolo26x.onnx",
    url: "https://github.com/ultralytics/assets/releases/download/v8.4.0/yolo26x.onnx",
    fallback: "https://ghproxy.net/https://github.com/ultralytics/assets/releases/download/v8.4.0/yolo26x.onnx",
    sha256: "88568299de91d4967f239a062c9f1619f695ebd05de73cd66b8f589591aaeb0a",
    size: 223_287_479,
    input: 640,
    license: License::Agpl,
    vendor: "Ultralytics",
    cpu_ref_ms: 269.0,
    gpu_a: 9.4,
    gpu_b: 336.0,
};

/// 界面里的展示顺序：从小到大
pub const MODELS: &[Spec] = &[YOLO26N, YOLO26L, YOLO26X, YOLOX_S];

/// 推荐阶梯：从高精度往低找，第一个装得下的就是推荐
const LADDER: &[&str] = &["yolo26x", "yolo26l", "yolo26n"];

pub fn by_id(id: &str) -> &'static Spec {
    MODELS.iter().find(|m| m.id == id).unwrap_or(&YOLO26N)
}

/// 预估单帧耗时（毫秒）。硬件太弱时报一个很大的值，推荐时自然会落到最小档。
pub fn estimate_ms(spec: &Spec, hw: &Hardware) -> f32 {
    if hw.gpu_usable() {
        spec.gpu_a + spec.gpu_b / hw.gpu_tflops
    } else if hw.cpu_mt > 0 {
        // 指数 0.52：推理没有 Cinebench 那么吃核，实测两点拟合出来的
        let ratio = CPU_REF_MT / hw.cpu_mt as f32;
        spec.cpu_ref_ms * ratio.powf(0.52)
    } else {
        9_999.0
    }
}


/// 单帧真正压在 **CPU** 上的时间。
///
/// 显卡模式下推理跑在显卡上，CPU 只负责 letterbox、NMS 和驱动调度，
/// 实测约 2 ms/帧，而且**基本不随模型变大**（模型越大是显卡那边更久）。
/// 早先直接用 `estimate_ms`（整帧端到端耗时）去乘帧率算 CPU 占用，
/// 把显卡的时间也算进了 CPU，同一个场景算出 82%，实测只有 10% 上下。
pub fn cpu_side_ms(spec: &Spec, hw: &Hardware) -> f32 {
    let est = estimate_ms(spec, hw);
    if hw.gpu_usable() {
        2.0f32.min(est)
    } else {
        est
    }
}

/// 按预算挑模型：装得下就往上抬，装不下就退一档。
pub fn recommend(hw: &Hardware) -> &'static Spec {
    // 没有可用独显时一律给最小档：CPU 上跑 l/x 在多数机器上喂不饱多路 1 FPS，
    // 与其推一个跑不动的档，不如给个体积最小、精度也够用的。
    if !hw.gpu_usable() {
        return &YOLO26N;
    }
    for id in LADDER {
        let m = by_id(id);
        if estimate_ms(m, hw) <= BUDGET_MS {
            return m;
        }
    }
    &YOLO26N
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Downloading { got: u64, total: u64 },
    Verifying,
    Done,
    Failed(String),
}

pub type Shared = Arc<Mutex<State>>;

pub fn shared() -> Shared {
    Arc::new(Mutex::new(State::Idle))
}

fn set(shared: &Shared, s: State) {
    if let Ok(mut g) = shared.lock() {
        *g = s;
    }
}

pub fn models_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|p| p.join("CamNexus").join("models"))
}

/// 已安装且大小对得上就返回路径（哈希在下载时已经校验过，这里只做粗筛）
pub fn installed_of(spec: &Spec) -> Option<PathBuf> {
    let p = models_dir()?.join(spec.file);
    let meta = std::fs::metadata(&p).ok()?;
    if meta.len() == spec.size {
        Some(p)
    } else {
        None
    }
}

/// 后台下载 + 校验，进度写进 `shared`
pub fn spawn_download(spec: &'static Spec) -> Shared {
    let shared = shared();
    let st = shared.clone();
    std::thread::spawn(move || match download(spec, &st) {
        Ok(()) => set(&st, State::Done),
        Err(e) => set(&st, State::Failed(e)),
    });
    shared
}

/// 候选下载源：自建镜像（环境变量）→ 官方 → 第三方代理
pub fn sources(spec: &Spec) -> Vec<(&'static str, String)> {
    let mut v: Vec<(&'static str, String)> = Vec::new();
    if let Ok(custom) = std::env::var("CAMNEXUS_MODEL_MIRROR") {
        let custom = custom.trim();
        if !custom.is_empty() {
            // 支持两种写法：完整 URL，或一个目录前缀
            if custom.ends_with(".onnx") {
                v.push((tr("自定义镜像"), custom.to_string()));
            } else {
                v.push((
                    tr("自定义镜像"),
                    format!("{}/{}", custom.trim_end_matches('/'), spec.file),
                ));
            }
        }
    }
    v.push((tr("官方源"), spec.url.to_string()));
    v.push((tr("备用代理"), spec.fallback.to_string()));
    v
}

fn download(spec: &'static Spec, state: &Shared) -> Result<(), String> {
    let mut last = String::from(tr("没有可用的下载源"));
    for (name, url) in sources(spec) {
        match download_from(spec, &url, state) {
            Ok(()) => return Ok(()),
            Err(e) => last = format!("{name}：{e}"),
        }
    }
    Err(last)
}

fn download_from(spec: &Spec, url: &str, state: &Shared) -> Result<(), String> {
    let dir = models_dir().ok_or_else(|| tr("找不到可写的安装目录").to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| crate::i18n::err(tr("创建目录失败"), e))?;
    let dest = dir.join(spec.file);
    let tmp = dir.join(format!("{}.part", spec.file));

    let agent = ureq::Agent::config_builder()
        .timeout_global(None)
        .build()
        .new_agent();

    let mut resp = agent
        .get(url)
        .header("User-Agent", "CamNexus/0.6 (+https://github.com)")
        .call()
        .map_err(|e| crate::i18n::err(tr("请求失败"), e))?;

    let total = resp.body().content_length().unwrap_or(spec.size);
    let mut reader = resp.body_mut().as_reader();
    let mut file =
        std::fs::File::create(&tmp).map_err(|e| crate::i18n::err(tr("创建临时文件失败"), e))?;

    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut got: u64 = 0;
    let mut last = Instant::now();

    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| crate::i18n::err(tr("读取中断"), e))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| crate::i18n::err(tr("写入失败"), e))?;
        hasher.update(&buf[..n]);
        got += n as u64;
        if last.elapsed().as_millis() >= 200 {
            last = Instant::now();
            set(state, State::Downloading { got, total });
        }
    }
    file.flush()
        .map_err(|e| crate::i18n::err(tr("落盘失败"), e))?;
    drop(file);

    set(state, State::Verifying);
    let digest = format!("{:x}", hasher.finalize());
    if digest != spec.sha256 {
        let _ = std::fs::remove_file(&tmp);
        return Err(tr("模型校验失败（文件可能被篡改或下载不完整）").to_string());
    }
    if got != spec.size {
        let _ = std::fs::remove_file(&tmp);
        return Err(crate::i18n::file_too_small(got));
    }

    std::fs::rename(&tmp, &dest).map_err(|e| crate::i18n::err(tr("保存失败"), e))?;
    Ok(())
}

/// 删除已下载的模型（用户想收回空间时用）
pub fn remove_of(spec: &Spec) -> Result<(), String> {
    let Some(p) = models_dir().map(|d| d.join(spec.file)) else {
        return Ok(());
    };
    if p.exists() {
        std::fs::remove_file(&p).map_err(|e| crate::i18n::err(tr("删除失败"), e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hw(cpu_mt: u32, tflops: f32, directml: bool) -> Hardware {
        Hardware {
            cpu: String::new(),
            cores: 8,
            cpu_mt,
            gpu: String::new(),
            gpu_tflops: tflops,
            directml,
        }
    }

    #[test]
    fn catalog_meta_is_plausible() {
        // 官方 release 的固定大小，改错了这里会先炸
        assert_eq!(YOLOX_S.size, 35_858_697);
        assert_eq!(YOLO26N.size, 9_941_957);
        assert_eq!(YOLO26L.size, 99_613_335);
        assert_eq!(YOLO26X.size, 223_287_479);
        for m in MODELS {
            assert_eq!(m.sha256.len(), 64, "{} 的 SHA-256 长度不对", m.id);
            assert!(m.file.ends_with(".onnx"));
            assert!(m.url.starts_with("https://"));
        }
    }

    #[test]
    fn every_model_is_reachable_by_id() {
        for m in MODELS {
            assert_eq!(by_id(m.id).id, m.id);
        }
        // 未知 id 要有个确定的兜底，不能 panic
        assert_eq!(by_id("不存在").id, YOLO26N.id);
    }

    /// 两个实测锚点必须复现：估出来的值要和真机差不太多
    #[test]
    fn estimate_matches_measured_anchors() {
        // 9800X3D + 4070 Ti SUPER：显卡 4 / 10 / 17，CPU 17 / 133 / 269
        let fast = hw(23_163, 44.1, true);
        assert!((estimate_ms(&YOLO26N, &fast) - 4.0).abs() < 1.5);
        assert!((estimate_ms(&YOLO26L, &fast) - 10.0).abs() < 2.0);
        assert!((estimate_ms(&YOLO26X, &fast) - 17.0).abs() < 2.0);

        let fast_cpu = hw(23_163, 0.0, false);
        assert!((estimate_ms(&YOLO26N, &fast_cpu) - 17.0).abs() < 1.0);
        assert!((estimate_ms(&YOLO26L, &fast_cpu) - 133.0).abs() < 2.0);

        // i5-10400 + GTX 960：显卡 17 / 83 / 151，CPU 28 / 234 / 477
        let old_cpu = hw(8_219, 0.0, false);
        assert!((estimate_ms(&YOLO26N, &old_cpu) - 28.0).abs() < 3.0);
        assert!((estimate_ms(&YOLO26L, &old_cpu) - 234.0).abs() < 15.0);

        let old_gpu = hw(8_219, 2.37, true);
        assert!((estimate_ms(&YOLO26N, &old_gpu) - 17.0).abs() < 3.0);
        assert!((estimate_ms(&YOLO26L, &old_gpu) - 83.0).abs() < 8.0);
    }

    #[test]
    fn recommendation_follows_the_two_anchors() {
        // 强机 + 好显卡：直接给最高档
        assert_eq!(recommend(&hw(23_163, 44.1, true)).id, "yolo26x");
        // 老机器 + 老显卡：l 装得下（83ms），x 装不下（151ms）
        assert_eq!(recommend(&hw(8_219, 2.37, true)).id, "yolo26l");
        // 只有 CPU：一律退到 n
        assert_eq!(recommend(&hw(8_219, 0.0, false)).id, "yolo26n");
        assert_eq!(recommend(&hw(23_163, 0.0, false)).id, "yolo26n");
    }

    #[test]
    fn no_gpu_means_smallest_model() {
        // 核显也算「没有可用显卡」：DirectML 不可用就不认
        assert_eq!(recommend(&hw(30_000, 5.0, false)).id, "yolo26n");
        // 独显但规格太低（GT 1030 级别）：仍按 CPU 走，不引入显卡开销
        assert_eq!(recommend(&hw(23_163, 1.13, true)).id, "yolo26n");
    }

    #[test]
    fn weak_and_unknown_hardware_falls_back_to_smallest() {
        // 完全测不到硬件时不能推荐大模型
        let unknown = hw(0, 0.0, false);
        assert_eq!(recommend(&unknown).id, "yolo26n");
        assert!(estimate_ms(&YOLO26X, &unknown) > BUDGET_MS);
    }

    #[test]
    fn license_flags_are_right() {
        assert!(YOLO26N.license.needs_notice());
        assert!(YOLO26X.license.needs_notice());
        assert!(!YOLOX_S.license.needs_notice());
        assert_eq!(YOLO26N.license.label(), "AGPL-3.0");
        assert_eq!(YOLOX_S.license.label(), "Apache-2.0");
    }
}
