//! 运行时诊断：把「画面卡 / 占用高」归因到具体环节，产出可执行的建议
//!
//! 设计原则：
//! - 只在持续满足条件时才提示，避免瞬时尖峰误报
//! - 必须能区分「算力不足」与「取流不稳」，否则建议会误导用户
//! - 结论携带可执行动作，而不只是一句警告

use crate::i18n::{self, tr};
use std::collections::VecDeque;
use std::time::Instant;

/// 预热时间：启动初期解码、缓冲、关键帧等待都会造成短时高占用
const WARMUP_SECS: u64 = 8;
/// 参与判定的采样数（1 秒 1 次）
const WINDOW: usize = 6;
/// 历史保留长度（画曲线用）
pub const HISTORY: usize = 60;

#[derive(Clone, Default, Debug)]
pub struct StreamMetrics {
    pub live: bool,
    pub fps: f32,
    pub target_fps: f32,
    /// 帧间隔抖动（相邻间隔差值的 EMA，毫秒）
    pub jitter_ms: f32,
    /// 该路解码进程占总算力的百分比
    pub cpu_pct: f32,
    /// 纹理上传耗时 EMA（毫秒）
    pub upload_ms: f32,
}

#[derive(Clone, Default, Debug)]
pub struct Snapshot {
    pub streams: Vec<StreamMetrics>,
    pub main_cpu_pct: f32,
    pub app_cpu_pct: f32,
    pub sys_cpu_pct: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    DecodeHeavy,
    RenderHeavy,
    SourceUnstable,
    LinkJitter,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    LowerResolution,
    UseHardware,
    UseTcp,
}

pub struct Advice {
    pub kind: Kind,
    pub text: String,
    pub action: Option<Action>,
    pub action_label: &'static str,
}

pub struct Evaluator {
    hist: VecDeque<Snapshot>,
    started: Instant,
}

impl Default for Evaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl Evaluator {
    pub fn new() -> Self {
        Self {
            hist: VecDeque::with_capacity(HISTORY),
            started: Instant::now(),
        }
    }

    pub fn push(&mut self, s: Snapshot) {
        if self.hist.len() >= HISTORY {
            self.hist.pop_front();
        }
        self.hist.push_back(s);
    }

    pub fn history(&self) -> &VecDeque<Snapshot> {
        &self.hist
    }

    fn window(&self) -> impl Iterator<Item = &Snapshot> {
        let n = self.hist.len().min(WINDOW);
        self.hist.iter().skip(self.hist.len() - n)
    }

    /// 返回最需要提示的一条建议；无异常时返回 None
    ///
    /// `hw_upgradable`：当前是软解且本机存在可用硬件后端，此时优先建议开硬解，
    /// 而不是直接让用户降分辨率。
    pub fn evaluate(&self, hw_upgradable: bool) -> Option<Advice> {
        if self.started.elapsed().as_secs() < WARMUP_SECS || self.hist.len() < WINDOW {
            return None;
        }

        let win: Vec<&Snapshot> = self.window().collect();
        let n = win.len() as f32;

        // 只在有在线通道时判定
        let live_streams: Vec<&StreamMetrics> = win
            .iter()
            .flat_map(|s| s.streams.iter())
            .filter(|m| m.live)
            .collect();
        if live_streams.is_empty() {
            return None;
        }

        // 帧率达成率取最低的一路（单路拖后腿也算问题）。
        // 只在「有明确帧率目标」时才有意义；跟随源模式下 target=0，不参与判定。
        let has_target = win
            .iter()
            .flat_map(|s| s.streams.iter())
            .any(|m| m.live && m.target_fps > 0.0);
        let mut worst_ratio = 1.0f32;
        let mut worst_jitter = 0.0f32;
        let mut worst_upload = 0.0f32;
        let mut worst_cpu = 0.0f32;
        for s in &win {
            for m in &s.streams {
                // fps 还没测出来（<0.5）的采样不参与判定，避免启动期污染
                if !m.live || m.target_fps <= 0.0 || m.fps < 0.5 {
                    continue;
                }
                worst_ratio = worst_ratio.min(m.fps / m.target_fps);
            }
            worst_jitter = worst_jitter.max(s.streams.iter().map(|m| m.jitter_ms).fold(0.0, f32::max));
            worst_upload = worst_upload.max(s.streams.iter().map(|m| m.upload_ms).fold(0.0, f32::max));
            for m in &s.streams {
                if m.live {
                    worst_cpu = worst_cpu.max(m.cpu_pct);
                }
            }
        }
        let app_cpu = win.iter().map(|s| s.app_cpu_pct).sum::<f32>() / n;
        let ratio_bad = has_target && worst_ratio < 0.85;

        // 1) 算力不足：帧率不达标（或没有目标但 CPU 已高）且整体吃 CPU
        if (ratio_bad || app_cpu > 78.0) && app_cpu > 60.0 {
            let (action, label) = if hw_upgradable {
                (Some(Action::UseHardware), tr("开启硬件解码"))
            } else {
                (Some(Action::LowerResolution), tr("降低送显分辨率"))
            };
            return Some(Advice {
                kind: Kind::DecodeHeavy,
                text: if has_target {
                    i18n::diag_decode_ratio(worst_ratio, app_cpu)
                } else {
                    i18n::diag_decode_cpu(app_cpu)
                },
                action,
                action_label: label,
            });
        }

        // 2) 渲染过重：上传耗时超过单帧预算
        if worst_upload > 10.0 {
            return Some(Advice {
                kind: Kind::RenderHeavy,
                text: i18n::diag_compose(worst_upload),
                action: Some(Action::LowerResolution),
                action_label: tr("降低送显分辨率"),
            });
        }

        // 3) 取流不稳：帧率不达标但 CPU 富余 —— 问题在网络或摄像机侧
        if ratio_bad && app_cpu < 40.0 {
            return Some(Advice {
                kind: Kind::SourceUnstable,
                text: i18n::diag_unstable(worst_ratio, app_cpu),
                action: Some(Action::UseTcp),
                action_label: tr("改用 TCP 传输"),
            });
        }

        // 4) 链路抖动：帧到达间隔忽长忽短
        if worst_jitter > 70.0 && app_cpu < 50.0 {
            return Some(Advice {
                kind: Kind::LinkJitter,
                text: i18n::diag_jitter(worst_jitter),
                action: Some(Action::UseTcp),
                action_label: tr("改用 TCP 传输"),
            });
        }

        let _ = worst_cpu;
        None
    }

    /// 生成可直接粘贴到反馈里的诊断文本
    pub fn report(&self) -> String {
        let mut out = String::new();
        let Some(last) = self.hist.back() else {
            return tr("尚无采样数据").into();
        };
        out.push_str(&i18n::diag_header(env!("CARGO_PKG_VERSION")));
        out.push_str(&i18n::diag_cpu_line(last.sys_cpu_pct, last.main_cpu_pct, last.app_cpu_pct));
        for (i, m) in last.streams.iter().enumerate() {
            let target = if m.target_fps > 0.0 {
                format!("{:.0}", m.target_fps)
            } else {
                tr("跟随源").to_string()
            };
            out.push_str(&i18n::diag_stream_line(
                i + 1,
                m.live,
                m.fps,
                &target,
                m.jitter_ms,
                m.cpu_pct,
                m.upload_ms,
            ));
        }
        if let Some(a) = self.evaluate(false) {
            let tag = match a.kind {
                Kind::DecodeHeavy => tr("算力不足"),
                Kind::RenderHeavy => tr("渲染过重"),
                Kind::SourceUnstable => tr("取流不稳"),
                Kind::LinkJitter => tr("链路抖动"),
            };
            out.push_str(&i18n::diag_verdict(tag, &a.text));
        }
        out
    }
}
