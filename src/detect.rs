//! 目标检测（人体 / 车辆）：YOLOX ONNX + ONNX Runtime
//!
//! 模型来自 YOLOX 官方 Releases（Apache-2.0）：
//! `https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.0/yolox_s.onnx`
//!
//! 两个必须记住的规格（官方文档没写，实测踩出来的）：
//! 1. 输入要归一化到 **0–1**（除以 255），喂 0–255 会得到一堆乱码结果；
//! 2. 输出 `[1, N, 85]` 里的框是**网格单位**，必须自己按 `(xy+grid)*stride`、
//!    `exp(wh)*stride` 解码；obj/cls 已在模型内做过 sigmoid，不要再 sigmoid。

use crate::i18n::{self, tr};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::DetectCfg;

/// 只关心这几类（COCO id → 中文名）
const CLASSES: &[(usize, &str)] = &[
    (0, "人"),
    (1, "自行车"),
    (2, "车"),
    (3, "摩托"),
    (5, "公交"),
    (7, "卡车"),
];

#[derive(Clone, Copy, Debug)]
pub struct Detection {
    pub label: &'static str,
    pub score: f32,
    /// 归一化坐标（0..1），相对原始画面
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub struct Detector {
    session: ort::session::Session,
    input: usize,
    /// 三个检测头的网格步长
    strides: [usize; 3],
    /// 预处理缓冲，避免每帧分配
    chw: Vec<f32>,
    /// 夜视优化：CLAHE 对比度增强
    night_opt: bool,
    /// CLAHE 用的亮度缓冲
    luma: Vec<u8>,
}

/// 进程级 ONNX Runtime 环境。
///
/// ort 限制「一个进程只能有一个 Environment」，而切显卡/CPU 会重建 Detector；
/// 如果每次都重新 `ort::init().build()`，第二次必定失败（实测报
/// `only one environment is allowed per process`），推理线程随即退出，
/// 表现就是「切回 CPU 后识别开关打不开」。
fn shared_env() -> Result<&'static ort::environment::Environment, String> {
    static ENV: std::sync::OnceLock<Result<ort::environment::Environment, String>> =
        std::sync::OnceLock::new();
    match ENV.get_or_init(|| {
        ort::init()
            .with_name("camnexus")
            .build()
            .map_err(|e| i18n::err(tr("初始化 ONNX Runtime 失败"), e))
    }) {
        Ok(env) => Ok(env),
        Err(msg) => Err(msg.clone()),
    }
}

impl Detector {
    /// 纯 CPU 加载（显卡路径统一走 `load_with`）。
    /// `input` 由模型决定（yolox_s = 640，yolox_tiny = 416）。
    #[allow(dead_code)]
    pub fn load(model: &Path, input: usize) -> Result<Self, String> {
        Self::load_with(model, input, false)
    }

    /// `gpu` 为真时用 DirectML 执行提供器；初始化失败会自动回退 CPU。
    pub fn load_with(model: &Path, input: usize, gpu: bool) -> Result<Self, String> {
        // ONNX Runtime 一个进程只允许一个 Environment（实测第二次 build 直接报
        // "only one environment is allowed per process"），所以环境做成进程级单例，
        // 显卡/CPU 的差异放到 **session** 上——EP 本来就可以按 session 注册。
        let env = shared_env()?;
        let new_builder = || {
            ort::session::Session::builder(env).map_err(|e| i18n::err(tr("加载模型失败"), e))
        };
        let mut builder = new_builder()?;
        if gpu {
            match builder.with_execution_providers([ort::ep::DirectML::default().build()]) {
                Ok(b) => builder = b,
                // 注册失败（驱动/DLL 不对）就重来一个纯 CPU 的，不让整次加载失败
                Err(_) => builder = new_builder()?,
            }
        }
        let session = builder
            .commit_from_file(model)
            .map_err(|e| i18n::err(tr("加载模型失败"), e))?;
        Ok(Self {
            session,
            input,
            strides: [8, 16, 32],
            chw: vec![0.0; 3 * input * input],
            night_opt: false,
            luma: Vec::new(),
        })
    }

    /// 设置夜视优化（CLAHE 3.0 / 8×8）
    pub fn set_night_opt(&mut self, on: bool) {
        self.night_opt = on;
    }

    /// 对一帧 RGB（`w*h*3`）做检测，返回原图归一化坐标
    pub fn detect(&mut self, rgb: &[u8], w: usize, h: usize, thr: f32) -> Result<Vec<Detection>, String> {
        if rgb.len() < w * h * 3 || w == 0 || h == 0 {
            return Ok(Vec::new());
        }
        // 夜视优化：先做 CLAHE，再进 letterbox。实测同一张红外画面
        // 置信度从 0.261 提到 0.478（全局直方图均衡反而更差，故不用）。
        let enhanced;
        let rgb = if self.night_opt {
            enhanced = clahe_rgb(rgb, w, h, &mut self.luma);
            enhanced.as_slice()
        } else {
            rgb
        };
        let size = self.input;

        // letterbox：等比缩放后居中，空白填 114/255
        let r = (size as f32 / w as f32).min(size as f32 / h as f32);
        let nw = ((w as f32 * r) as usize).max(1);
        let nh = ((h as f32 * r) as usize).max(1);
        let dx = (size - nw) / 2;
        let dy = (size - nh) / 2;
        let pad = 114.0 / 255.0;

        self.chw.fill(pad);
        let c_plane = size * size;
        for ty in 0..nh {
            let sy = ((ty as f32) / r) as usize;
            if sy >= h {
                break;
            }
            for tx in 0..nw {
                let sx = ((tx as f32) / r) as usize;
                if sx >= w {
                    break;
                }
                let si = (sy * w + sx) * 3;
                let di = (dy + ty) * size + (dx + tx);
                let v = [
                    rgb[si] as f32 / 255.0,
                    rgb[si + 1] as f32 / 255.0,
                    rgb[si + 2] as f32 / 255.0,
                ];
                self.chw[di] = v[0];
                self.chw[c_plane + di] = v[1];
                self.chw[2 * c_plane + di] = v[2];
            }
        }

        let input = ort::value::TensorRef::from_array_view((
            [1usize, 3, size, size],
            self.chw.as_slice(),
        ))
        .map_err(|e| i18n::err(tr("构造输入失败"), e))?;
        let outputs = self
            .session
            .run(ort::inputs![input])
            .map_err(|e| i18n::err(tr("推理失败"), e))?;
        let out = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| i18n::err(tr("读取输出失败"), e))?;

        // [1, N, 85] → 逐行解码
        let rows = out.shape()[1];
        let cols = out.shape()[2];
        let data = out.as_slice().ok_or(tr("输出内存不连续"))?;

        let mut cands: Vec<(f32, usize, [f32; 4])> = Vec::new(); // score, class, xywh(px)
        let mut offset = 0usize;
        for &s in &self.strides {
            let g = size / s;
            let cells = g * g;
            for i in 0..cells {
                let base = (offset + i) * cols;
                let obj = data[base + 4];
                if obj < thr {
                    continue;
                }
                // 取最大类别
                let mut best = 0usize;
                let mut best_p = 0.0f32;
                for c in 0..(cols - 5) {
                    let p = data[base + 5 + c];
                    if p > best_p {
                        best_p = p;
                        best = c;
                    }
                }
                let score = obj * best_p;
                if score < thr {
                    continue;
                }
                if !CLASSES.iter().any(|(id, _)| *id == best) {
                    continue;
                }
                let gx = (i % g) as f32;
                let gy = (i / g) as f32;
                let cx = (data[base] + gx) * s as f32;
                let cy = (data[base + 1] + gy) * s as f32;
                let bw = data[base + 2].exp() * s as f32;
                let bh = data[base + 3].exp() * s as f32;
                cands.push((
                    score,
                    best,
                    [cx - bw / 2.0, cy - bh / 2.0, bw, bh],
                ));
            }
            offset += cells;
        }
        let _ = rows;

        // 贪心 NMS
        cands.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut keep: Vec<(f32, usize, [f32; 4])> = Vec::new();
        for c in cands {
            if keep.iter().all(|k| iou(&k.2, &c.2) < 0.45) {
                keep.push(c);
            }
            if keep.len() >= 32 {
                break;
            }
        }

        // 映射回原图并归一化
        let mut out_v = Vec::with_capacity(keep.len());
        for (score, cls, b) in keep {
            let label = CLASSES
                .iter()
                .find(|(id, _)| *id == cls)
                .map(|(_, n)| *n)
                .unwrap_or(tr("目标"));
            let x0 = ((b[0] - dx as f32) / r / w as f32).clamp(0.0, 1.0);
            let y0 = ((b[1] - dy as f32) / r / h as f32).clamp(0.0, 1.0);
            let x1 = ((b[0] + b[2] - dx as f32) / r / w as f32).clamp(0.0, 1.0);
            let y1 = ((b[1] + b[3] - dy as f32) / r / h as f32).clamp(0.0, 1.0);
            if x1 - x0 < 0.01 || y1 - y0 < 0.01 {
                continue;
            }
            out_v.push(Detection {
                label,
                score,
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            });
        }
        Ok(out_v)
    }
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    let inter = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

/// 模型搜索路径：先看程序目录 models/，再看 %LOCALAPPDATA%\CamNexus\models\
pub fn find_model(name: &str) -> Option<PathBuf> {
    let mut cands = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(d) = exe.parent() {
            cands.push(d.join("models").join(name));
            cands.push(d.join(name));
        }
    if let Some(base) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        cands.push(base.join("CamNexus").join("models").join(name));
    }
    cands.into_iter().find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ONNX Runtime 环境必须是进程级单例：第二次 build 会直接失败，
    /// 一旦失败推理线程就退出，界面上表现为「切显卡/CPU 后识别开关打不开」。
    #[test]
    fn environment_is_a_process_singleton() {
        let a = shared_env().expect("第一次初始化应该成功");
        let b = shared_env().expect("第二次必须复用同一个环境，不能报错");
        assert!(std::ptr::eq(a, b), "两次拿到的应是同一个 Environment");
    }

    /// 手动检查（需要模型 + 显卡）：真机验证「先显卡、再切 CPU」能连续加载。
    /// 跑法：cargo test -- --ignored probe
    #[test]
    #[ignore]
    fn probe_gpu_then_cpu() {
        let Some(path) = find_model("yolox_s.onnx") else {
            println!("没有模型文件，跳过");
            return;
        };
        Detector::load_with(&path, 640, true).expect("显卡模式应能加载");
        Detector::load_with(&path, 640, false).expect("切到 CPU 后应能加载");
        Detector::load_with(&path, 640, true).expect("再切回显卡应能加载");
    }


    /// 回归：通道 1 的目标不许出现在通道 2 上。
    ///
    /// 曾经所有通道共用一份 Tracker，`update()` 会把「本帧没匹配上但还没到丢失上限」
    /// 的轨迹一并输出，于是 A 的框被画到了 B 的画面上。
    #[test]
    fn tracks_never_leak_between_channels() {
        let cfg = DetectCfg::default(); // 静止豁免 60 秒
        let mut ts = Trackers::new(&cfg);
        let now = Instant::now();
        let person = Detection {
            label: "人",
            score: 0.9,
            x: 0.1,
            y: 0.1,
            w: 0.1,
            h: 0.2,
        };

        // 通道 0 连续两帧看到人 → 达到 hits ≥ 2，开始输出
        assert!(ts.update(0, &[person], now).is_empty(), "第一帧只登记不显示");
        assert_eq!(ts.update(0, &[person], now).len(), 1, "通道 0 应输出目标");

        // 通道 1 一个人都没有 → 绝不能冒出通道 0 的框
        let out = ts.update(1, &[], now);
        assert!(out.is_empty(), "通道 1 不该看到通道 0 的目标，实际 {:?}", out.len());

        // 真实时序是两路交替来帧：通道 0 每帧都有人，通道 1 每帧都空。
        // 共用 Tracker 时，通道 0 的轨迹会被通道 1 的「空帧」判成丢失，
        // 但丢失不足 3 帧仍会输出，于是交替跑下去通道 1 永远在显示别人的框。
        for _ in 0..8 {
            assert_eq!(ts.update(0, &[person], now).len(), 1, "通道 0 应一直有人");
            assert!(
                ts.update(1, &[], now).is_empty(),
                "交替提交时通道 1 也必须保持干净"
            );
        }

        // 通道 0 自己那份还在（这是防闪框的平滑，属于正常行为）
        assert_eq!(ts.update(0, &[person], now).len(), 1);
    }

    /// 空闲通道的跟踪器会被回收，删通道后不会无限堆积
    #[test]
    fn idle_channel_trackers_are_pruned() {
        let cfg = DetectCfg::default();
        let mut ts = Trackers::new(&cfg);
        let now = Instant::now();
        let det = Detection {
            label: "车",
            score: 0.9,
            x: 0.1,
            y: 0.1,
            w: 0.2,
            h: 0.2,
        };
        ts.update(0, &[det], now);
        ts.update(1, &[det], now);
        assert_eq!(ts.map.len(), 2);
        // 11 分钟后再来一帧，只有正在用的那路留下
        ts.update(0, &[det], now + std::time::Duration::from_secs(660));
        assert_eq!(ts.map.len(), 1, "超过 10 分钟没帧的通道应被回收");
    }

    #[test]
    fn iou_basic() {
        let a = [0.0, 0.0, 10.0, 10.0];
        assert!((iou(&a, &a) - 1.0).abs() < 1e-6);
        let b = [20.0, 20.0, 10.0, 10.0];
        assert_eq!(iou(&a, &b), 0.0);
        let c = [5.0, 0.0, 10.0, 10.0];
        assert!((iou(&a, &c) - 1.0 / 3.0).abs() < 1e-5);
    }

    #[test]
    fn tracker_hides_static_after_grace() {
        let cfg = DetectCfg {
            static_grace_secs: 1,
            ..Default::default()
        };
        let mut t = Tracker::new(&cfg);
        let det = Detection { label: "车", score: 0.9, x: 0.1, y: 0.1, w: 0.2, h: 0.2 };
        let now = Instant::now();
        // 第一帧只登记，不显示（要求连续两帧）
        assert!(t.update(&[det], now).is_empty());
        // 第二帧仍然显示
        assert_eq!(t.update(&[det], now + std::time::Duration::from_millis(500)).len(), 1);
        // 超过豁免时长且没动 → 隐藏
        assert!(t.update(&[det], now + std::time::Duration::from_secs(2)).is_empty());
        // 小幅度移动（真实场景里车是一点点挪的）→ 立刻恢复显示
        let moved = Detection { x: 0.32, ..det };
        assert_eq!(t.update(&[moved], now + std::time::Duration::from_secs(3)).len(), 1);
    }

    #[test]
    fn tracker_zero_grace_never_hides() {
        let cfg = DetectCfg {
            static_grace_secs: 0,
            ..Default::default()
        };
        let mut t = Tracker::new(&cfg);
        let det = Detection { label: "人", score: 0.8, x: 0.1, y: 0.1, w: 0.1, h: 0.2 };
        let now = Instant::now();
        t.update(&[det], now);
        assert_eq!(t.update(&[det], now + std::time::Duration::from_secs(600)).len(), 1);
    }

    #[test]
    fn only_person_and_vehicle_kept() {
        assert!(CLASSES.iter().all(|(id, _)| matches!(id, 0 | 1 | 2 | 3 | 5 | 7)));
    }
}

// ---------------------------------------------------------------- 夜视优化（CLAHE）
/// 对亮度做 CLAHE（限制对比度的自适应直方图均衡），再按增益回写 RGB。
///
/// 参数取实测最优：clip limit 3.0、8×8 网格。只在亮度上做，避免偏色。
fn clahe_rgb(rgb: &[u8], w: usize, h: usize, luma_buf: &mut Vec<u8>) -> Vec<u8> {
    const TILES: usize = 8;
    const CLIP: f32 = 3.0;

    luma_buf.resize(w * h, 0);
    for i in 0..w * h {
        let r = rgb[i * 3] as u32;
        let g = rgb[i * 3 + 1] as u32;
        let b = rgb[i * 3 + 2] as u32;
        // Rec.601 亮度
        luma_buf[i] = ((r * 77 + g * 150 + b * 29) >> 8) as u8;
    }

    // 每块的直方图 + 均衡后的查找表
    let tw = w.div_ceil(TILES);
    let th = h.div_ceil(TILES);
    let area = (tw * th) as f32;
    let clip = (CLIP * area / 256.0).max(1.0) as u32;
    let mut luts = vec![[0u8; 256]; TILES * TILES];

    for ty in 0..TILES {
        for tx in 0..TILES {
            let mut hist = [0u32; 256];
            let y0 = ty * th;
            let y1 = ((ty + 1) * th).min(h);
            let x0 = tx * tw;
            let x1 = ((tx + 1) * tw).min(w);
            for y in y0..y1 {
                for x in x0..x1 {
                    hist[luma_buf[y * w + x] as usize] += 1;
                }
            }
            // 截断并重分配
            let mut excess = 0u32;
            for hcount in hist.iter_mut() {
                if *hcount > clip {
                    excess += *hcount - clip;
                    *hcount = clip;
                }
            }
            let per = excess / 256;
            let mut rest = excess % 256;
            for (i, hcount) in hist.iter_mut().enumerate() {
                *hcount += per;
                if rest > 0 && i < 256 {
                    *hcount += 1;
                    rest -= 1;
                }
            }
            // CDF → LUT
            let mut cdf = 0u32;
            let mut lut = [0u8; 256];
            let total: u32 = hist.iter().sum();
            for i in 0..256 {
                cdf += hist[i];
                lut[i] = ((cdf as f32 / total as f32) * 255.0).clamp(0.0, 255.0) as u8;
            }
            luts[ty * TILES + tx] = lut;
        }
    }

    // 双线性插值取每个像素的映射值
    let mut out = vec![0u8; w * h * 3];
    for y in 0..h {
        let gy = (y as f32 / th as f32 - 0.5).max(0.0);
        let ty0 = (gy as usize).min(TILES - 1);
        let ty1 = (ty0 + 1).min(TILES - 1);
        let fy = gy - ty0 as f32;
        for x in 0..w {
            let gx = (x as f32 / tw as f32 - 0.5).max(0.0);
            let tx0 = (gx as usize).min(TILES - 1);
            let tx1 = (tx0 + 1).min(TILES - 1);
            let fx = gx - tx0 as f32;

            let v = luma_buf[y * w + x] as usize;
            let a = luts[ty0 * TILES + tx0][v] as f32;
            let b = luts[ty0 * TILES + tx1][v] as f32;
            let c = luts[ty1 * TILES + tx0][v] as f32;
            let d = luts[ty1 * TILES + tx1][v] as f32;
            let top = a + (b - a) * fx;
            let bot = c + (d - c) * fx;
            let new_luma = (top + (bot - top) * fy).clamp(0.0, 255.0);

            let old = luma_buf[y * w + x] as f32;
            let gain = if old > 1.0 { new_luma / old } else { 1.0 };
            let i = (y * w + x) * 3;
            for k in 0..3 {
                out[i + k] = ((rgb[i + k] as f32) * gain).clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

// ---------------------------------------------------------------- 静止物体豁免
/// 一条目标轨迹
pub struct Track {
    pub label: &'static str,
    pub score: f32,
    /// 归一化框
    pub rect: [f32; 4],
    last_move: Instant,
    hits: u32,
    missed: u32,
}

impl Track {
    fn center(&self) -> (f32, f32) {
        (
            self.rect[0] + self.rect[2] / 2.0,
            self.rect[1] + self.rect[3] / 2.0,
        )
    }
}

/// 轻量跟踪器：按 IoU 关联，位移小视为静止；静止超时后不再输出
pub struct Tracker {
    tracks: Vec<Track>,
    /// 判定「动了」的最小位移（归一化，约等于画面宽度的 1.5%）
    move_eps: f32,
    /// 静止多久后隐藏
    grace_secs: u32,
}

impl Tracker {
    pub fn new(cfg: &DetectCfg) -> Self {
        Self {
            tracks: Vec::new(),
            move_eps: 0.015,
            grace_secs: cfg.static_grace_secs,
        }
    }

    /// 输入本帧检测结果，返回应当显示的目标
    pub fn update(&mut self, dets: &[Detection], now: Instant) -> Vec<Detection> {
        let mut used = vec![false; self.tracks.len()];

        for d in dets {
            let mut best: Option<(usize, f32)> = None;
            for (i, t) in self.tracks.iter().enumerate() {
                if used[i] || t.label != d.label {
                    continue;
                }
                let iou = iou_rect(&t.rect, &[d.x, d.y, d.w, d.h]);
                // 除了重叠，还允许「中心足够近」——物体移动快时框会跳，
                // 只靠 IoU 会把它当成新目标，导致要等两帧才重新显示
                let (tcx, tcy) = t.center();
                let dcx = d.x + d.w / 2.0;
                let dcy = d.y + d.h / 2.0;
                let dist = ((dcx - tcx).powi(2) + (dcy - tcy).powi(2)).sqrt();
                let near = dist < t.rect[2].max(t.rect[3]).max(0.05) * 1.5;
                if (iou > 0.4 || near) && best.map(|(_, b)| iou > b).unwrap_or(true) {
                    best = Some((i, iou));
                }
            }
            match best {
                Some((i, _)) => {
                    used[i] = true;
                    let t = &mut self.tracks[i];
                    let (cx, cy) = t.center();
                    let ncx = d.x + d.w / 2.0;
                    let ncy = d.y + d.h / 2.0;
                    let moved = (ncx - cx).abs() > self.move_eps || (ncy - cy).abs() > self.move_eps;
                    if moved {
                        t.last_move = now;
                    }
                    t.rect = [d.x, d.y, d.w, d.h];
                    t.score = d.score;
                    t.hits = t.hits.saturating_add(1);
                    t.missed = 0;
                }
                None => {
                    self.tracks.push(Track {
                        label: d.label,
                        score: d.score,
                        rect: [d.x, d.y, d.w, d.h],
                        last_move: now,
                        hits: 1,
                        missed: 0,
                    });
                    used.push(true);
                }
            }
        }

        // 没匹配上的轨迹累加丢失计数，连续 3 帧未见则移除
        for (i, t) in self.tracks.iter_mut().enumerate() {
            if !used.get(i).copied().unwrap_or(false) {
                t.missed += 1;
            }
        }
        self.tracks.retain(|t| t.missed < 3);

        // 只输出「稳定存在且未静止超时」的目标
        let grace = self.grace_secs;
        self.tracks
            .iter()
            .filter(|t| {
                if t.hits < 2 {
                    return false;
                }
                if grace == 0 {
                    return true;
                }
                now.duration_since(t.last_move).as_secs() < grace as u64
            })
            .map(|t| Detection {
                label: t.label,
                score: t.score,
                x: t.rect[0],
                y: t.rect[1],
                w: t.rect[2],
                h: t.rect[3],
            })
            .collect()
    }
}

/// 每个通道一份跟踪器。
///
/// **轨迹绝不能跨通道**：`Tracker::update` 除了本帧匹配到的目标，还会输出
/// 「这一帧没匹配上、但还没到丢失上限」的轨迹（防闪框的平滑策略）。
/// 如果多个通道共用一个 Tracker，通道 1 的目标在通道 2 里匹配不上，
/// 只会变成「丢失 1 帧」，下一帧照样被输出 —— 表现就是 A 的框画到了 B 上。
pub struct Trackers {
    map: std::collections::HashMap<usize, (Tracker, std::time::Instant)>,
    cfg: DetectCfg,
}

impl Trackers {
    pub fn new(cfg: &DetectCfg) -> Self {
        Self {
            map: std::collections::HashMap::new(),
            cfg: *cfg,
        }
    }

    /// 按通道号更新，返回该通道应当显示的目标
    pub fn update(
        &mut self,
        idx: usize,
        dets: &[Detection],
        now: std::time::Instant,
    ) -> Vec<Detection> {
        // 清掉十分钟没来过帧的通道（删通道 / 换配置后的残留），避免无限增长
        self.map
            .retain(|_, (_, last)| now.duration_since(*last).as_secs() < 600);

        let cfg = self.cfg;
        let entry = self
            .map
            .entry(idx)
            .or_insert_with(|| (Tracker::new(&cfg), now));
        entry.1 = now;
        entry.0.update(dets, now)
    }
}

fn iou_rect(a: &[f32; 4], b: &[f32; 4]) -> f32 {    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    let inter = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}
