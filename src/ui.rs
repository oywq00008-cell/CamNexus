//! 主界面

use crate::i18n::{self, tr};
use crate::audio::{AudioCfg, AudioStream};
use crate::config::{self, AppConfig, Channel};
use crate::detect::Detection;
use crate::diag::{Action, Evaluator, Snapshot, StreamMetrics};
use crate::discover::{self, Device, ScanMsg};
use crate::icon::VolumeIcons;
use crate::layout;
use crate::stream::{State, Stream, StreamCfg};
use crate::logs;
use crate::record::RecordManager;
use crate::tour::{self, StepId, Tour};
use crate::sys;
use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

const ACCENT: egui::Color32 = egui::Color32::from_rgb(76, 141, 255);
const OK: egui::Color32 = egui::Color32::from_rgb(61, 220, 132);
const WARN: egui::Color32 = egui::Color32::from_rgb(255, 197, 61);
const ERR: egui::Color32 = egui::Color32::from_rgb(255, 107, 107);
/// 叠在视频上的文字统一用浅色，与主题无关
const CHIP_TEXT: egui::Color32 = egui::Color32::from_rgb(240, 244, 250);

/// 主题调色板：所有界面颜色都从这里取，避免出现「浅色控件 + 深色外壳」
#[derive(Clone, Copy)]
pub struct Pal {
    pub bg: egui::Color32,
    pub panel: egui::Color32,
    pub card: egui::Color32,
    pub text: egui::Color32,
    pub muted: egui::Color32,
    pub warn_bg: egui::Color32,
    pub window: egui::Color32,
}

pub const PAL_DARK: Pal = Pal {
    bg: egui::Color32::from_rgb(12, 15, 20),
    panel: egui::Color32::from_rgb(20, 25, 33),
    card: egui::Color32::from_rgb(16, 20, 27),
    text: egui::Color32::from_rgb(230, 235, 244),
    muted: egui::Color32::from_rgb(138, 148, 165),
    warn_bg: egui::Color32::from_rgb(52, 42, 20),
    window: egui::Color32::from_rgb(22, 27, 35),
};

pub const PAL_LIGHT: Pal = Pal {
    bg: egui::Color32::from_rgb(243, 246, 250),
    panel: egui::Color32::from_rgb(252, 253, 255),
    card: egui::Color32::from_rgb(224, 229, 236),
    text: egui::Color32::from_rgb(28, 32, 40),
    muted: egui::Color32::from_rgb(98, 106, 120),
    warn_bg: egui::Color32::from_rgb(255, 244, 214),
    window: egui::Color32::from_rgb(255, 255, 255),
};

pub fn palette(dark: bool) -> Pal {
    if dark { PAL_DARK } else { PAL_LIGHT }
}

/// 可选送显分辨率档位（从低到高）
pub const RES_PRESETS: &[(u32, u32)] = &[
    (640, 360),
    (960, 540),
    (1280, 720),
    (1600, 900),
    (1920, 1080),
    (2560, 1440),
];

/// 某一路的「裁剪黑边」检测状态。
///
/// 检测不是每帧都做：2 秒一次，且要连续两次结果一致才生效，
/// 避免暗场景里闪一下就把画面裁歪。
#[derive(Default)]
struct CropWatch {
    next: Option<Instant>,
    pending: Option<[f32; 4]>,
    hits: u8,
    applied: Option<[f32; 4]>,
}

/// 画块右键菜单里选中的动作
#[derive(Clone, Copy)]
enum TileAct {
    Rotate(u16),
    FlipH,
    FlipV,
    ToggleLock,
    ToggleCrop,
    FitAspect,
    FullscreenOne,
    AutoLayout,
    Snapshot,
    Reconnect,
}

pub struct CamNexusApp {
    cfg: AppConfig,
    draft: AppConfig,
    streams: Vec<Option<Stream>>,
    textures: std::collections::HashMap<usize, egui::TextureHandle>,
    last_seq: std::collections::HashMap<usize, u64>,
    ffmpeg: Option<PathBuf>,
    show_settings: bool,
    show_scan: bool,
    solo: Option<usize>,
    toast: Option<(String, Instant)>,
    scan_rx: Option<Receiver<ScanMsg>>,
    scan_devices: Vec<Device>,
    scan_progress: (usize, usize),
    scan_running: bool,
    scan_prefix: String,
    scan_user: String,
    scan_pass: String,
    // ---- 运行时诊断 ----
    hwaccels: Vec<String>,
    diag: Evaluator,
    main_cpu: sys::CpuMeter,
    sys_cpu: sys::SysCpuMeter,
    last_sample: Instant,
    upload_ms: std::collections::HashMap<usize, f32>,
    show_details: bool,
    hint_dismissed: bool,
    // ---- 音频 ----
    audio_player: Option<AudioStream>,
    audio_idx: Option<usize>,
    volume: Arc<AtomicU32>,
    icons: VolumeIcons,
    /// 打开设置面板时的主题，用于「取消」时还原
    open_dark: bool,
    /// 打开设置时的语言，取消时用来还原
    open_lang: crate::i18n::Lang,
    /// 设置窗口当前标签页
    settings_tab: SetTab,
    /// 已绘制帧数，用于启动后补发标题栏主题
    frames: u64,
    // ---- 设置窗口 ----
    /// 打开设置后还需校准居中的帧数（首帧拿不到窗口尺寸）
    center_settings: u8,
    /// 打开扫描窗口后还需校准居中的帧数
    center_scan: u8,
    /// 当前视频流实际生效的参数指纹，用于提示「有改动待重连」
    active_sig: String,
    /// 启动时探测到的 FFmpeg 版本（「关于」直接用，避免每帧起子进程）
    ffmpeg_ver: String,
    /// 回放窗口的日期缓存（打开时算一次）
    replay_days: Option<Vec<Vec<String>>>,
    /// 目标检测
    detect: Option<DetectRuntime>,
    /// 实验功能：模型是否已就位（缓存，避免每帧 stat 文件）
    model_ready: bool,
    /// 模型下载进度
    model_state: crate::model::Shared,
    /// 是否处于（无边框）全屏
    fullscreen: bool,
    /// 全屏布局改过但还没落盘（拖动过程中别每帧写文件）
    tiles_dirty: bool,
    /// 命令行 --snapshot 触发的一次性抓拍（等首帧到达再执行）
    pending_snapshot: Option<(usize, Instant)>,
    /// 每路的裁剪黑边检测状态
    crops: std::collections::HashMap<usize, CropWatch>,
    /// 全屏下顶栏是否降下来了（鼠标靠近顶部才出）
    bar_revealed: bool,
    /// 录像管理
    records: RecordManager,
    // ---- 回放 / 日志 / 关于 ----
    show_replay: bool,
    /// 回放窗口里已选中的通道
    replay_channel: Option<usize>,
    show_logs: bool,
    show_about: bool,
    // ---- 新手引导 ----
    tour: Option<Tour>,
    /// 首次启动时的询问卡片
    welcome: bool,
    /// 欢迎卡片上的「演示模式」勾选
    tour_demo: bool,
    /// 上一帧气泡实测高度，用于下一帧定位
    tour_bubble_h: f32,
    /// 控件锚点，每帧重新登记，供引导高亮定位
    anchors: std::collections::HashMap<&'static str, egui::Rect>,
    // ---- FFmpeg 后端 ----
    fetch_state: Option<crate::fetch::Shared>,
    manual_ffmpeg: String,
    setup_msg: Option<String>,
}

impl CamNexusApp {
    // ---------------------------------------------------------------- 新手引导
    /// 开始引导。`demo` 为真时不打开真实窗口、不写任何配置。
    pub fn start_tour(&mut self, demo: bool) {
        self.welcome = false;
        self.show_settings = false;
        self.show_scan = false;
        self.tour = Some(Tour::new(demo));
        // 进了就不再自动询问
        if self.cfg.onboarding != config::Onboarding::Done {
            self.cfg.onboarding = config::Onboarding::Done;
            self.cfg.save().ok();
        }
        self.enter_step();
    }

    /// 直接跳到某一步（--tour-step，便于逐屏验收）
    pub fn start_tour_at(&mut self, step: usize) {
        self.start_tour(false);
        if let Some(t) = self.tour.as_mut() {
            t.step = step.min(tour::STEPS.len() - 1);
        }
        self.enter_step();
    }

    fn end_tour(&mut self) {
        self.tour = None;
        self.show_scan = false;
        self.show_settings = false;
    }

    /// 进入当前步骤时的副作用（打开对应窗口）
    fn enter_step(&mut self) {
        let (id, demo) = match self.tour.as_ref() {
            Some(t) => (t.current().id, t.demo),
            None => return,
        };
        if demo {
            return;
        }
        match id {
            StepId::Scan => {
                self.center_scan = 6;
                self.show_scan = true;
            }
            StepId::Manual => {
                self.draft = self.cfg.clone();
                self.open_dark = self.cfg.dark;
                self.open_lang = self.cfg.lang;
                self.center_settings = 6;
                self.show_settings = true;
            }
            StepId::Done => {
                self.show_scan = false;
                self.show_settings = false;
            }
            _ => {}
        }
    }

    /// 全屏遮罩 + 高亮挖空 + 气泡。遮罩带上挂交互区，高亮区放行。
    fn tour_overlay(&mut self, ctx: &egui::Context) {
        let Some(tour) = self.tour.as_ref() else {
            return;
        };
        let step_id = tour.current().id;
        let demo = tour.demo;
        let step_idx = tour.step;
        let step_title = tr(tour.current().title);
        let step_body = tr(tour.current().body);
        let step_primary = tr(tour.current().primary);
        let step_anchor = tour.current().anchor;

        let p = self.pal();
        let screen = ctx.content_rect();
        let hole = step_anchor
            .and_then(|id| self.anchors.get(id).copied())
            .filter(|r| r.is_positive())
            .map(|r| r.expand(6.0));

        let mask = if self.cfg.dark {
            egui::Color32::from_black_alpha(155)
        } else {
            egui::Color32::from_black_alpha(115)
        };
        let strips = mask_strips(screen, hole);

        // 遮罩与输入拦截
        egui::Area::new(egui::Id::unique("tour_mask"))
            .order(egui::Order::Foreground)
            .interactable(true)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.set_clip_rect(screen);
                let painter = ui.painter();
                for r in &strips {
                    painter.rect_filled(*r, egui::CornerRadius::ZERO, mask);
                }
                if let Some(h) = hole {
                    painter.rect_stroke(
                        h,
                        egui::CornerRadius::same(6),
                        egui::Stroke::new(2.0, ACCENT),
                        egui::StrokeKind::Outside,
                    );
                }
                // 除了高亮区，其余位置的点击都被这些不可见交互区吃掉
                for (i, r) in strips.iter().enumerate() {
                    if r.is_positive() {
                        let _ = ui.interact(
                            *r,
                            egui::Id::unique(("tour_block", i)),
                            egui::Sense::click_and_drag(),
                        );
                    }
                }
            });

        // 气泡
        let bw = 430.0;
        let gap = 16.0;
        let mut area = egui::Area::new(egui::Id::unique("tour_bubble"))
            .order(egui::Order::Foreground)
            .interactable(true);
        let est_h = self.tour_bubble_h.max(120.0);
        if let Some(h) = hole {
            let room_below = screen.bottom() - h.bottom();
            let cx = h.center().x.clamp(
                screen.left() + bw * 0.5 + 10.0,
                screen.right() - bw * 0.5 - 10.0,
            );
            let need = est_h + gap;
            // 只要下方还有一点空间就优先放下方（必要时压在区域下沿一点点），
            // 因为上方往往是工具栏，压住它更难看。
            if room_below >= need {
                area = area
                    .pivot(egui::Align2::CENTER_TOP)
                    .fixed_pos(egui::pos2(cx, h.bottom() + gap));
            } else if room_below >= 60.0 {
                // 留 20px 安全边距：气泡边框/阴影不计入实测高度
                let y = (screen.bottom() - est_h - 20.0).max(screen.top() + 8.0);
                area = area
                    .pivot(egui::Align2::CENTER_TOP)
                    .fixed_pos(egui::pos2(cx, y));
            } else {
                let y = (h.top() - gap).max(screen.top() + 8.0);
                area = area
                    .pivot(egui::Align2::CENTER_BOTTOM)
                    .fixed_pos(egui::pos2(cx, y));
            }
        } else {
            area = area
                .pivot(egui::Align2::CENTER_CENTER)
                .fixed_pos(screen.center());
        }

        let mut action = TourAction::None;
        let bubble_resp = area.show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(p.window)
                .inner_margin(egui::Margin::same(16))
                .show(ui, |ui| {
                    ui.set_width(bw - 32.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}/{}",
                                step_idx + 1,
                                tour::STEPS.len()
                            ))
                            .size(11.0)
                            .color(p.muted),
                        );
                        if demo {
                            ui.label(
                                egui::RichText::new(tr("演示模式 · 不会修改任何设置"))
                                    .size(11.0)
                                    .color(WARN),
                            );
                        }
                    });
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(step_title).size(16.0).strong().color(p.text));
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(step_body).size(13.0).color(p.muted));
                    ui.add_space(14.0);

                    ui.horizontal(|ui| {
                        match step_id {
                            StepId::ChooseAdd => {
                                if demo {
                                    if ui.button(tr("演示：走一遍添加流程")).clicked() {
                                        action = TourAction::GotoDone;
                                    }
                                } else {
                                    if ui.button(tr("自动扫描")).clicked() {
                                        action = TourAction::GotoScan;
                                    }
                                    if ui.button(tr("手动填写")).clicked() {
                                        action = TourAction::GotoManual;
                                    }
                                }
                                if ui.button(tr("跳过这段")).clicked() {
                                    action = TourAction::Next;
                                }
                            }
                            StepId::Done => {
                                if ui.button(tr("完成")).clicked() {
                                    action = TourAction::Finish;
                                }
                            }
                            _ => {
                                if step_idx > 0 && ui.button(tr("上一步")).clicked() {
                                    action = TourAction::Back;
                                }
                                if ui.button(step_primary).clicked() {
                                    action = TourAction::Next;
                                }
                            }
                        }
                        ui.add_space(4.0);
                        if ui.button(tr("退出引导")).clicked() {
                            action = TourAction::Exit;
                        }
                    });
                });
        });

        // 记住实际高度，供下一帧定位使用
        let measured = bubble_resp.response.rect.height();
        if measured > 10.0 {
            self.tour_bubble_h = measured;
        }

        match action {
            TourAction::None => {}
            TourAction::Next => {
                if let Some(t) = self.tour.as_mut() {
                    if t.is_last() {
                        self.end_tour();
                        return;
                    }
                    t.next();
                }
                self.enter_step();
            }
            TourAction::Back => {
                if let Some(t) = self.tour.as_mut() {
                    t.back();
                }
                self.enter_step();
            }
            TourAction::GotoScan => {
                if let Some(t) = self.tour.as_mut() {
                    t.branch = 1;
                    t.goto(StepId::Scan);
                }
                self.enter_step();
            }
            TourAction::GotoManual => {
                if let Some(t) = self.tour.as_mut() {
                    t.branch = 2;
                    t.goto(StepId::Manual);
                }
                self.enter_step();
            }
            TourAction::GotoDone => {
                if let Some(t) = self.tour.as_mut() {
                    t.goto(StepId::Done);
                }
                self.enter_step();
            }
            TourAction::Finish | TourAction::Exit => self.end_tour(),
        }
    }

    // ---------------------------------------------------------------- 回放 / 日志 / 关于
    fn replay_window(&mut self, ctx: &egui::Context) {
        if !self.show_replay {
            return;
        }
        let p = self.pal();
        let mut open = true;
        let root = crate::record::record_root(&self.cfg);
        let mut pick_channel: Option<usize> = None;

        // 每帧扫目录太浪费：打开窗口时算一次并缓存
        if self.replay_days.is_none() {
            self.replay_days = Some(
                (0..self.cfg.channels.len())
                    .map(|i| crate::record::dates_for(&root, &self.cfg.channels, i))
                    .collect(),
            );
        }
        let days_cache = self.replay_days.clone().unwrap_or_default();
        let mut open_day: Option<String> = None;
        let mut err: Option<String> = None;

        let win = egui::Window::new(tr("回放"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(520.0)
            .default_height(380.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO);

        win.show(ctx, |ui| match self.replay_channel {
            None => {
                ui.label(egui::RichText::new(tr("选择通道")).strong());
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(tr("录像按「日期 / 通道」存放，点开通道后选择日期即可查看"))
                        .size(11.5)
                        .color(p.muted),
                );
                ui.add_space(10.0);
                egui::ScrollArea::vertical()
                    .max_height(280.0)
                    .show(ui, |ui| {
                        for (i, ch) in self.cfg.channels.iter().enumerate() {
                            let days = days_cache.get(i).cloned().unwrap_or_default();
                            ui.horizontal(|ui| {
                                if ui.button(&ch.name).clicked() {
                                    pick_channel = Some(i);
                                }
                                let txt = if days.is_empty() {
                                    tr("录像不存在").to_string()
                                } else {
                                    i18n::days_recent(days.len(), &days[0])
                                };
                                ui.label(
                                    egui::RichText::new(txt)
                                        .size(11.5)
                                        .color(if days.is_empty() { p.muted } else { OK }),
                                );
                            });
                        }
                    });
            }
            Some(i) => {
                let name = self
                    .cfg
                    .channels
                    .get(i)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                let days = days_cache.get(i).cloned().unwrap_or_default();
                ui.horizontal(|ui| {
                    if ui.button(tr("← 返回")).clicked() {
                        self.replay_channel = None;
                    }
                    ui.label(egui::RichText::new(&name).strong());
                });
                ui.add_space(8.0);
                if days.is_empty() {
                    ui.label(egui::RichText::new(tr("录像不存在")).color(WARN));
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(i18n::archive_path(&root.display().to_string()))
                            .size(11.0)
                            .color(p.muted),
                    );
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            for day in &days {
                                if ui
                                    .add(
                                        egui::Button::new(format!("📁 {day}"))
                                            .min_size(egui::vec2(200.0, 26.0)),
                                    )
                                    .clicked()
                                {
                                    open_day = Some(day.clone());
                                }
                            }
                        });
                }
            }
        });

        if let Some(i) = pick_channel {
            self.replay_channel = Some(i);
        }
        if let Some(day) = open_day
            && let Some(i) = self.replay_channel {
                let dir = root
                    .join(&day)
                    .join(crate::record::channel_folder_pub(&self.cfg.channels, i));
                if let Err(e) = crate::record::open_folder(&dir) {
                    err = Some(e);
                }
            }
        if let Some(e) = err {
            logs::append("", &i18n::err(tr("回放打开失败"), &e));
            self.toast(e);
        }
        if !open {
            self.show_replay = false;
        }
    }

    fn logs_window(&mut self, ctx: &egui::Context) {
        if !self.show_logs {
            return;
        }
        let p = self.pal();
        let mut open = true;
        let mut pick: Option<String> = None;
        let days = logs::days();

        egui::Window::new(tr("日志"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(360.0)
            .default_height(320.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(tr("按天记录，Markdown 格式。点日期用系统默认程序打开。"))
                        .size(11.5)
                        .color(p.muted),
                );
                ui.add_space(8.0);
                if days.is_empty() {
                    ui.label(egui::RichText::new(tr("暂无日志")).color(p.muted));
                    return;
                }
                egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                    for d in &days {
                        if ui
                            .add(egui::Button::new(format!("📄 {d}")).min_size(egui::vec2(180.0, 26.0)))
                            .clicked()
                        {
                            pick = Some(d.clone());
                        }
                    }
                });
            });

        if let Some(d) = pick
            && let Some(path) = logs::file_for(&d) {
                logs::open(&path);
            }
        if !open {
            self.show_logs = false;
        }
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let p = self.pal();
        let mut open = true;
        let mut open_path: Option<std::path::PathBuf> = None;
        let root = crate::record::record_root(&self.cfg);
        let log_dir = logs::dir();

        egui::Window::new(tr("关于 CamNexus"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(440.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(tr("CamNexus · 监控中枢")).size(17.0).strong());
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(i18n::version(env!("CARGO_PKG_VERSION")))
                        .size(12.0)
                        .color(p.muted),
                );
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new(
                        tr("把支持 RTSP 的网络摄像机统一到一个窗口监看，支持录像、回放与运行日志。"),
                    )
                    .size(13.0)
                    .color(p.text),
                );
                ui.add_space(10.0);
                egui::Grid::new("about_grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        let mut row = |k: &str, v: String| {
                            ui.label(egui::RichText::new(k).size(12.0).color(p.muted));
                            ui.label(egui::RichText::new(v).size(12.0).color(p.text));
                            ui.end_row();
                        };
                        row(tr("作者"), "oywq0008-cell".to_string());
                        row(
                            tr("技术栈"),
                            "Rust · egui/eframe · FFmpeg".to_string(),
                        );
                        row(
                            "FFmpeg",
                            if self.ffmpeg_ver.is_empty() {
                                tr("未安装").to_string()
                            } else {
                                self.ffmpeg_ver.clone()
                            },
                        );
                        row(tr("日志目录"), log_dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default());
                    });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button(tr("打开存档目录")).clicked() {
                        open_path = Some(root.clone());
                    }
                    if let Some(d) = &log_dir
                        && ui.button(tr("打开日志目录")).clicked() {
                            open_path = Some(d.clone());
                        }
                });
            });

        if let Some(pth) = open_path {
            let _ = crate::record::open_folder(&pth);
        }
        if !open {
            self.show_about = false;
        }
    }

    /// 首次启动的询问卡片
    fn welcome_window(&mut self, ctx: &egui::Context) {
        if !self.welcome {
            return;
        }
        let p = self.pal();
        let mut start = false;
        let mut later = false;
        egui::Window::new(tr("欢迎使用 CamNexus"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_width(430.0);
                ui.add_space(4.0);
                ui.label(egui::RichText::new(tr("第一次使用？")).size(16.0).strong().color(p.text));
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "花一分钟带你认一遍界面，然后添加第一台摄像机。\
                         也可以先跳过，之后在「设置 → 新手引导」重新开始。",
                    )
                    .size(13.0)
                    .color(p.muted),
                );
                ui.add_space(12.0);
                ui.checkbox(&mut self.tour_demo, tr("演示模式（不连接真实设备，只看讲解）"));
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui.button(tr("开始引导")).clicked() {
                        start = true;
                    }
                    if ui.button(tr("以后再说")).clicked() {
                        later = true;
                    }
                });
            });

        if start {
            let demo = self.tour_demo;
            self.start_tour(demo);
        }
        if later {
            self.welcome = false;
            self.cfg.onboarding = config::Onboarding::Declined;
            self.cfg.save().ok();
        }
    }

    pub fn new(cc: &eframe::CreationContext<'_>, cfg: AppConfig) -> Self {
        apply_style(&cc.egui_ctx, cfg.dark);
        // 找到不等于能用：PATH 里可能是 4.x，缺 -fps_mode 等参数
        let mut setup_msg = None;
        let mut ffmpeg_ver = String::new();
        let ffmpeg = match config::find_ffmpeg(&cfg.ffmpeg_path) {
            Some(p) => match crate::fetch::check_usable(&p) {
                Ok(ver) => {
                    ffmpeg_ver = ver;
                    Some(p)
                }
                Err(e) => {
                    setup_msg = Some(i18n::err(tr("本机的 FFmpeg 无法使用"), e));
                    None
                }
            },
            None => None,
        };
        let hwaccels = ffmpeg
            .as_deref()
            .map(config::detect_hwaccels)
            .unwrap_or_default();
        let ip = discover::local_ip();
        let prefix = ip
            .rsplit_once('.')
            .map(|(p, _)| p.to_string())
            .unwrap_or(ip);
        // 只在「从未问过」且没有任何通道时打扰一次
        let welcome_prompt =
            cfg.onboarding == config::Onboarding::NotAsked && cfg.channels.is_empty();
        logs::prune(30);
        let mut app = Self {
            draft: cfg.clone(),
            volume: Arc::new(AtomicU32::new(cfg.volume.to_bits())),
            cfg,
            streams: Vec::new(),
            textures: Default::default(),
            last_seq: Default::default(),
            ffmpeg,
            show_settings: false,
            show_scan: false,
            solo: None,
            toast: None,
            scan_rx: None,
            scan_devices: Vec::new(),
            scan_progress: (0, 0),
            scan_running: false,
            scan_prefix: prefix,
            scan_user: "admin".into(),
            scan_pass: String::new(),
            hwaccels,
            diag: Evaluator::new(),
            main_cpu: sys::CpuMeter::default(),
            sys_cpu: sys::SysCpuMeter::default(),
            last_sample: Instant::now(),
            upload_ms: Default::default(),
            show_details: false,
            hint_dismissed: false,
            audio_player: None,
            audio_idx: None,
            icons: VolumeIcons::load(&cc.egui_ctx),
            open_dark: false,
            open_lang: crate::i18n::Lang::Zh,
            settings_tab: SetTab::default(),
            frames: 0,
            center_settings: 0,
            center_scan: 0,
            ffmpeg_ver,
            replay_days: None,
            detect: None,
            model_ready: crate::model::installed().is_some(),
            model_state: crate::model::shared(),
            fullscreen: false,
            tiles_dirty: false,
            pending_snapshot: None,
            crops: std::collections::HashMap::new(),
            bar_revealed: false,
            records: RecordManager::new(),
            show_replay: false,
            replay_channel: None,
            show_logs: false,
            show_about: false,
            tour: None,
            welcome: welcome_prompt,
            tour_demo: false,
            tour_bubble_h: 160.0,
            anchors: std::collections::HashMap::new(),
            active_sig: String::new(),
            fetch_state: None,
            manual_ffmpeg: String::new(),
            setup_msg,
        };
        app.rebuild_streams();
        logs::append(
            "",
            &i18n::started(
                env!("CARGO_PKG_VERSION"),
                &app.ffmpeg
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| tr("未找到").into()),
            ),
        );
        app
    }

    /// 启动时直接打开运行状态面板（--diag）
    pub fn open_details(&mut self) {
        self.show_details = true;
    }

    /// 启动时直接打开设置面板（--settings）
    pub fn open_settings(&mut self) {
        self.draft = self.cfg.clone();
        self.open_dark = self.cfg.dark;
                self.open_lang = self.cfg.lang;
        self.center_settings = 6;
        self.show_settings = true;
    }

    /// 启动即开始下载 FFmpeg（--fetch-ffmpeg，便于脚本化部署）
    pub fn start_fetch(&mut self) {
        if self.ffmpeg.is_none() {
            self.fetch_state = Some(crate::fetch::spawn_download());
        }
    }

    /// 启动即开录第 N 路（--record N，1 起）
    /// 设置窗口打开到指定标签页（0 起），给命令行/截图用
    pub fn set_settings_tab(&mut self, n: usize) {
        // 直接按 SET_TABS 的顺序取，改标签顺序时不用再同步改这里
        if let Some((tab, _)) = SET_TABS.get(n) {
            self.settings_tab = *tab;
        }
    }

    pub fn start_record(&mut self, idx0: usize) {
        let n = self.cfg.channels.len();
        if idx0 < n {
            self.records.toggle(idx0);
            logs::append("", &i18n::rec_from_cli(idx0 + 1));
        }
    }

    /// 命令行/自检用：直接改内存里的配置（不写盘）
    pub fn cfg_mut(&mut self) -> &mut AppConfig {
        &mut self.cfg
    }

    /// 启动后抓一张快照（--snapshot N，自检用）
    pub fn snapshot_now(&mut self, idx: usize) {
        self.pending_snapshot = Some((idx, Instant::now()));
    }

    /// 启动即进入全屏（--fullscreen，自检/截图用）
    pub fn enter_fullscreen(&mut self, ctx: &egui::Context) {
        self.set_fullscreen(ctx, true);
    }

    /// 启动即开启识别（--detect）
    pub fn enable_detect(&mut self, ctx: &egui::Context) {
        if self.detect.is_none() {
            self.detect = Some(DetectRuntime::spawn(ctx.clone(), self.cfg.detect));
        }
    }

    /// 启动即打开扫描窗口（--scan）
    pub fn open_scan_window(&mut self) {
        self.center_scan = 6;
        self.show_scan = true;
    }

    /// 启动时自动播放指定通道的音频（--audio N，N 从 1 开始）
    pub fn start_audio_index(&mut self, idx: usize) {
        if idx < self.cfg.channels.len() {
            self.toggle_audio(idx);
        }
    }

    /// 缺少 FFmpeg 时的引导页：自动下载，或指定本机已有的
    fn ffmpeg_setup(&mut self, ui: &mut egui::Ui) {
        let p = self.pal();

        // 轮询后台下载状态
        let mut state = crate::fetch::State::Idle;
        if let Some(sh) = &self.fetch_state
            && let Ok(g) = sh.lock() {
                state = g.clone();
            }
        if let crate::fetch::State::Done(path) = &state {
            match crate::fetch::check_usable(path) {
                Ok(ver) => {
                    self.ffmpeg = Some(path.clone());
                    self.cfg.ffmpeg_path = path.display().to_string();
                    self.cfg.save().ok();
                    self.fetch_state = None;
                    self.rebuild_streams();
                    self.setup_msg = None;
                    self.toast(i18n::ffmpeg_ready(&ver));
                }
                Err(e) => self.setup_msg = Some(e),
            }
            return;
        }

        ui.add_space(36.0);
        ui.heading(egui::RichText::new(tr("准备解码后端")).color(p.text));
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                tr("CamNexus 不自带 FFmpeg（完整包里九成功能用不上）。启动时会先在本机查找，找不到就下载精简版（约 79 MB，国内镜像，几秒到十几秒）。"),
            )
            .color(p.muted),
        );
        ui.add_space(16.0);

        match &state {
            crate::fetch::State::Downloading {
                mirror,
                got,
                total,
                speed,
            } => {
                let frac = if *total > 0 {
                    *got as f32 / *total as f32
                } else {
                    0.0
                };
                ui.add(
                    egui::ProgressBar::new(frac)
                        .desired_width(460.0)
                        .text(format!(
                            "{:.0} / {:.0} MB   ·   {:.1} MB/s   ·   {}",
                            *got as f64 / 1_048_576.0,
                            *total as f64 / 1_048_576.0,
                            *speed as f64 / 1_048_576.0,
                            mirror
                        )),
                );
                ui.add_space(8.0);
                ui.label(egui::RichText::new(tr("下载中，请勿关闭窗口…")).color(p.muted));
            }
            crate::fetch::State::Verifying => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new(tr("正在校验…")).color(p.muted));
                });
            }
            crate::fetch::State::Failed(e) => {
                ui.label(egui::RichText::new(i18n::err(tr("下载失败"), e)).color(ERR));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(tr("重试")).clicked() {
                        self.fetch_state = Some(crate::fetch::spawn_download());
                        self.setup_msg = None;
                    }
                    ui.hyperlink_to(tr("都打不开？手动下载"), crate::fetch::DOWNLOAD_PAGE);
                });
            }
            _ => {
                if ui.button(tr("自动下载并安装（约 79 MB）")).clicked() {
                    self.fetch_state = Some(crate::fetch::spawn_download());
                    self.setup_msg = None;
                }
            }
        }

        ui.add_space(18.0);
        ui.separator();
        ui.add_space(10.0);
        ui.label(egui::RichText::new(tr("已经装过 FFmpeg？直接指定路径：")).color(p.muted));
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.manual_ffmpeg)
                    .hint_text(r"C:\ffmpeg\bin\ffmpeg.exe")
                    .desired_width(420.0),
            );
            if ui.button(tr("使用")).clicked() {
                let path = std::path::PathBuf::from(self.manual_ffmpeg.trim());
                match crate::fetch::check_usable(&path) {
                    Ok(ver) => {
                        self.ffmpeg = Some(path.clone());
                        self.cfg.ffmpeg_path = path.display().to_string();
                        self.cfg.save().ok();
                        self.rebuild_streams();
                        self.setup_msg = None;
                        eprintln!("已切换 ffmpeg：{ver}");
                    }
                    Err(e) => self.setup_msg = Some(e),
                }
            }
        });

        if let Some(msg) = &self.setup_msg {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(msg).color(WARN));
        }
    }

    fn rebuild_streams(&mut self) {
        self.stop_audio();
        // 通道可能被删除或重排，录制槽位按索引对应，必须跟着停
        self.records.stop_all_async();
        self.active_sig = stream_sig(&self.cfg);
        self.streams.clear();
        self.textures.clear();
        self.last_seq.clear();
        // 通道可能被删掉、后面的索引整体前移；检测结果是按索引存的，
        // 不清掉的话旧索引的框会在 3 秒有效期内落到别的通道上
        if let Some(d) = self.detect.as_mut() {
            d.results.clear();
        }
        if self.ffmpeg.is_none() {
            self.streams = self.cfg.channels.iter().map(|_| None).collect();
            return;
        }
        let fresh: Vec<Option<Stream>> = (0..self.cfg.channels.len())
            .map(|i| self.spawn_one(i))
            .collect();
        self.streams = fresh;
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    /// 同一时刻只播一路音频；再次点击当前路则停止
    fn toggle_audio(&mut self, idx: usize) {
        if self.audio_idx == Some(idx) {
            self.stop_audio();
            return;
        }
        self.stop_audio();
        let Some(ffmpeg) = self.ffmpeg.clone() else {
            self.toast(tr("未找到 ffmpeg，无法播放音频"));
            return;
        };
        let Some(ch) = self.cfg.channels.get(idx) else {
            return;
        };
        if !ch.enabled || ch.url.trim().is_empty() {
            self.toast(tr("该通道未启用"));
            return;
        }
        let url = ch.rtsp();
        self.audio_player = Some(AudioStream::start(AudioCfg {
            url,
            ffmpeg,
            tcp: self.cfg.tcp,
            volume: self.volume.clone(),
        }));
        self.audio_idx = Some(idx);
        self.toast(tr("正在连接音频…"));
    }

    fn stop_audio(&mut self) {
        if let Some(a) = self.audio_player.take() {
            a.stop();
        }
        self.audio_idx = None;
    }

    fn pal(&self) -> Pal {
        palette(self.cfg.dark)
    }

    fn online_count(&self) -> (usize, usize) {
        let total = self.streams.iter().filter(|s| s.is_some()).count();
        let live = self
            .streams
            .iter()
            .filter_map(|s| s.as_ref())
            .filter(|s| s.status().state == State::Live)
            .count();
        (live, total)
    }

    fn update_textures(&mut self, ctx: &egui::Context) {
        for i in 0..self.streams.len() {
            let frame = self.streams[i].as_ref().and_then(|s| s.frame());
            let Some(f) = frame else { continue };
            if self.last_seq.get(&i) == Some(&f.seq) {
                continue;
            }
            let t0 = Instant::now();
            let img = egui::ColorImage::from_rgb([f.w, f.h], f.data.as_slice());
            match self.textures.get_mut(&i) {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => {
                    let t = ctx.load_texture(format!("ch{i}"), img, egui::TextureOptions::LINEAR);
                    self.textures.insert(i, t);
                }
            }
            // 单帧「构造 + 提交纹理」耗时，作为渲染侧压力的代理指标
            let dt = t0.elapsed().as_secs_f32() * 1000.0;
            let ema = self.upload_ms.entry(i).or_insert(dt);
            *ema = *ema * 0.8 + dt * 0.2;
            if let Some(d) = self.detect.as_mut() {
                d.maybe_submit(i, f.data.clone(), f.w, f.h);
            }
            self.last_seq.insert(i, f.seq);
        }
    }

    fn poll_scan(&mut self) {
        let mut msgs = Vec::new();
        if let Some(rx) = &self.scan_rx {
            while let Ok(m) = rx.try_recv() {
                msgs.push(m);
            }
        }
        for m in msgs {
            match m {
                ScanMsg::Progress(done, total) => self.scan_progress = (done, total),
                ScanMsg::Device(d) => self.scan_devices.push(d),
                ScanMsg::Done(list) => {
                    self.scan_devices = list;
                    self.scan_running = false;
                    self.scan_rx = None;
                    self.toast(tr("扫描完成"));
                }
            }
        }
    }

    fn start_scan(&mut self) {
        self.scan_devices.clear();
        self.scan_progress = (0, 254);
        self.scan_running = true;
        let (tx, rx) = std::sync::mpsc::channel();
        self.scan_rx = Some(rx);
        discover::scan_async(
            self.scan_prefix.clone(),
            self.scan_user.clone(),
            self.scan_pass.clone(),
            tx,
        );
    }

    // ---------------------------------------------------------------- 顶栏
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let p = self.pal();
        self.anchors.insert(tour::anchor::TOP_BAR, ui.max_rect());
        let (live, total) = self.online_count();
        let mut open_settings = false;
        let mut open_scan = false;
        let mut kick_all = false;
        let mut reconnect_now = false;
        let mut open_replay = false;
        let mut open_logs = false;
        let mut open_about = false;
        let mut toggle_detect = false;

        egui::containers::Sides::new().height(36.0).show(
            ui,
            |ui| {
                ui.label(egui::RichText::new("CamNexus").size(19.0).strong().color(p.text));
                ui.add_space(6.0);
                ui.label(egui::RichText::new(tr("监控中枢")).size(12.0).color(p.muted));
                ui.add_space(12.0);
                let (dot, label) = if total == 0 {
                    (p.muted, tr("未配置通道").to_string())
                } else if live == total {
                    (OK, i18n::online(live, total))
                } else if live == 0 {
                    (ERR, i18n::online(0, total))
                } else {
                    (WARN, i18n::online(live, total))
                };
                ui.label(egui::RichText::new("●").size(11.0).color(dot));
                ui.label(egui::RichText::new(label).size(12.5).color(p.muted));

                if let Some(i) = self.audio_idx {
                    ui.add_space(10.0);
                    let (txt, color) = match self.audio_player.as_ref() {
                        Some(a) if a.playing() => (
                            i18n::audio_level(i, (a.level() * 1000.0).clamp(0.0, 100.0)),
                            ACCENT,
                        ),
                        _ => (i18n::audio_connecting(i), WARN),
                    };
                    ui.label(egui::RichText::new(txt).size(12.0).color(color));
                }
            },
            |ui| {
                if ui.button(tr("设置")).clicked() {
                    open_settings = true;
                }
                if ui.button(tr("扫描设备")).clicked() {
                    open_scan = true;
                }
                if stream_sig(&self.cfg) != self.active_sig
                    && ui
                        .add(egui::Button::new(
                            egui::RichText::new(tr("重连以应用改动")).color(WARN),
                        ))
                        .on_hover_text(tr("设置里改动了通道或解码参数，点击按新设置重建全部视频流"))
                        .clicked()
                    {
                        reconnect_now = true;
                    }
                if ui.button(tr("全部重连")).clicked() {
                    kick_all = true;
                }
                // 这一组按「右→左」排布，源码顺序与视觉顺序相反
                ui.add_space(8.0);
                if ui.button(tr("关于")).clicked() {
                    open_about = true;
                }
                if ui.button(tr("日志")).clicked() {
                    open_logs = true;
                }
                // 识别是实验功能：只有用户在设置里启用、且模型已下载，主界面才出现这个按钮
                if self.cfg.detect.enabled && self.model_ready {
                    let detect_on = self.detect.as_ref().map(|d| d.enabled).unwrap_or(false);
                    let label = if detect_on {
                        egui::RichText::new(tr("识别 · 开")).color(OK)
                    } else {
                        egui::RichText::new(tr("识别"))
                    };
                    let dev = match self.cfg.detect.device {
                        crate::config::DetectDevice::Cpu => "CPU",
                        crate::config::DetectDevice::Gpu => tr("显卡"),
                    };
                    let tip = i18n::detect_tip(
                        self.cfg.detect.effective_fps(),
                        dev,
                        &i18n::static_grace(self.cfg.detect.static_grace_secs),
                    );
                    if ui.button(label).on_hover_text(tip).clicked() {
                        toggle_detect = true;
                    }
                }
                if ui.button(tr("回放")).clicked() {
                    open_replay = true;
                }
            },
        );

        if toggle_detect {
            match self.detect.as_mut() {
                None => {
                    let rt = DetectRuntime::spawn(ui.ctx().clone(), self.cfg.detect);
                    let status = rt.status.clone();
                    self.detect = Some(rt);
                    if !status.is_empty() && !status.contains(tr("加载")) {
                        self.toast(status);
                    }
                }
                Some(d) => {
                    d.enabled = !d.enabled;
                    if !d.enabled {
                        d.results.clear();
                    }
                }
            }
        }
        if open_replay {
            self.replay_channel = None;
            self.replay_days = None;
            self.show_replay = true;
        }
        if open_logs {
            self.show_logs = true;
        }
        if open_about {
            self.show_about = true;
        }
        if open_settings {
            self.draft = self.cfg.clone();
            self.open_dark = self.cfg.dark;
                self.open_lang = self.cfg.lang;
            self.center_settings = 6;
            self.show_settings = true;
        }
        if open_scan {
            self.center_scan = 6;
            self.show_scan = true;
        }
        if reconnect_now {
            self.rebuild_streams();
            self.toast(tr("已按新设置重连"));
        }
        if kick_all {
            for s in self.streams.iter().flatten() {
                s.kick();
            }
            self.toast(tr("已触发重连"));
        }
    }

    // ---------------------------------------------------------------- 视频区
    fn video_area(&mut self, ui: &mut egui::Ui) {
        let visible: Vec<usize> = if let Some(s) = self.solo {
            vec![s]
        } else {
            (0..self.streams.len())
                .filter(|i| self.streams[*i].is_some())
                .collect()
        };

        if visible.is_empty() {
            self.empty_hint(ui);
            return;
        }

        let avail = ui.available_rect_before_wrap();
        ui.allocate_space(avail.size());

        let gap = 8.0;
        let n = visible.len();
        let target = (self.cfg.render_width as f32 / self.cfg.render_height.max(1) as f32).max(0.1);

        // 在 1~3 列之间挑选「有效画面面积最大」的排布
        let mut best_cols = 1usize;
        let mut best_area = 0.0f32;
        for cols in 1..=3usize {
            let rows = n.div_ceil(cols);
            if rows > 3 {
                continue;
            }
            let cw = (avail.width() - gap * (cols as f32 - 1.0)) / cols as f32;
            let ch = (avail.height() - gap * (rows as f32 - 1.0)) / rows as f32;
            let cell_w = cw.min(ch * target);
            if cell_w <= 0.0 {
                continue;
            }
            let area = cell_w * (cell_w / target) * n as f32;
            if area > best_area {
                best_area = area;
                best_cols = cols;
            }
        }

        let cols = best_cols;
        let rows = n.div_ceil(cols);
        let cw = (avail.width() - gap * (cols as f32 - 1.0)) / cols as f32;
        let ch = (avail.height() - gap * (rows as f32 - 1.0)) / rows as f32;
        let cell_w = cw.min(ch * target).max(40.0);
        let cell_h = cell_w / target;

        let grid_w = cols as f32 * cell_w + gap * (cols as f32 - 1.0);
        let grid_h = rows as f32 * cell_h + gap * (rows as f32 - 1.0);
        let ox = avail.left() + (avail.width() - grid_w) * 0.5;
        let oy = avail.top() + (avail.height() - grid_h) * 0.5;

        // 全屏：自由布局（可拖动/缩放/吸附），不再走网格
        if self.fullscreen {
            self.free_canvas(ui, &visible, avail);
            self.fullscreen_button(ui, avail);
            return;
        }

        for (k, idx) in visible.iter().enumerate() {
            let r = k / cols;
            let c = k % cols;
            let rect = egui::Rect::from_min_size(
                egui::pos2(
                    ox + c as f32 * (cell_w + gap),
                    oy + r as f32 * (cell_h + gap),
                ),
                egui::vec2(cell_w, cell_h),
            );
            if k == 0 {
                self.anchors.insert(tour::anchor::TILE, rect);
            }
            self.draw_tile(ui, rect, *idx, egui::Sense::click(), None);
        }

        self.fullscreen_button(ui, avail);
    }

    /// 右下角全屏按钮。
    ///
    /// 用 Foreground 层的 Area：画块自带的音量/录制按钮就在右下角，
    /// 普通绘制顺序会被它们盖住，放前景层才点得到。
    fn fullscreen_button(&mut self, ui: &mut egui::Ui, area: egui::Rect) {
        // 全屏下按钮不再常驻（会压住画面右下角），鼠标靠近才浮现；
        // 窗口模式始终保持可见，它是进全屏的入口
        if self.fullscreen {
            let near = ui
                .ctx()
                .input(|i| i.pointer.hover_pos())
                .map(|p| p.x > area.right() - 170.0 && p.y > area.bottom() - 170.0)
                .unwrap_or(false);
            if !near {
                return;
            }
        }
        let size = 34.0;
        let mut clicked = false;
        egui::Area::new(egui::Id::unique("btn_fullscreen"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(
                area.right() - size - 10.0,
                area.bottom() - size - 10.0,
            ))
            .show(ui.ctx(), |ui| {
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
                let p = self.pal();
                let bg = if resp.hovered() {
                    p.card.gamma_multiply(1.8)
                } else {
                    egui::Color32::from_rgba_unmultiplied(20, 20, 20, 190)
                };
                let painter = ui.painter();
                painter.rect_filled(rect, egui::CornerRadius::same(7), bg);
                painter.rect_stroke(
                    rect,
                    egui::CornerRadius::same(7),
                    egui::Stroke::new(1.0, egui::Color32::from_white_alpha(40)),
                    egui::StrokeKind::Inside,
                );
                let tex = if self.fullscreen {
                    &self.icons.fullscreen_exit
                } else {
                    &self.icons.fullscreen
                };
                painter.image(
                    tex.id(),
                    rect.shrink(8.0),
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::from_rgb(235, 235, 235),
                );
                let tip = if self.fullscreen {
                    tr("退出全屏（Esc）")
                } else {
                    tr("全屏（画面可拖动 / 缩放）")
                };
                if resp.on_hover_text(tip).clicked() {
                    clicked = true;
                }
            });
        if clicked {
            self.set_fullscreen(ui.ctx(), !self.fullscreen);
        }
    }

    /// 全屏自由画布：每路一个可拖动、可缩放、带吸附的窗口
    fn free_canvas(&mut self, ui: &mut egui::Ui, visible: &[usize], canvas: egui::Rect) {
        // 布局数量对不上，或者某个可见通道还没有位置（新增/删除过通道），就重排
        let need_len = self.streams.len();
        let stale = self.cfg.tiles.len() != need_len
            || visible.iter().any(|i| {
                self.cfg
                    .tiles
                    .get(*i)
                    .map(|t| t[2] <= 0.0 || t[3] <= 0.0)
                    .unwrap_or(true)
            });
        if stale {
            self.reset_tiles(canvas, visible);
        }

        ui.allocate_space(canvas.size());

        // 单路全屏：这一路直接铺满画布，不参与拖动/吸附
        if let Some(only) = self.solo {
            let _ = self.draw_tile(ui, canvas, only, egui::Sense::click(), Some(canvas));
            return;
        }

        // 先把所有块的当前矩形算出来，供吸附参考
        let rects: Vec<egui::Rect> = (0..self.streams.len())
            .map(|i| {
                if i < self.cfg.tiles.len() {
                    layout::from_norm(self.cfg.tiles[i], canvas)
                } else {
                    egui::Rect::NOTHING
                }
            })
            .collect();

        let mut moved: Option<(usize, egui::Rect)> = None;
        let mut guides: Vec<layout::Guide> = Vec::new();

        for idx in visible {
            let idx = *idx;
            let rect = rects[idx];
            let others: Vec<egui::Rect> = rects
                .iter()
                .enumerate()
                .filter(|(i, r)| *i != idx && r.is_positive())
                .map(|(_, r)| *r)
                .collect();

            // 画块本体（拖拽手势交给它自己）
            let resp = self.draw_tile(ui, rect, idx, egui::Sense::click_and_drag(), Some(canvas));
            if resp.dragged() {
                let target = rect.translate(resp.drag_delta());
                let (snapped, g) = layout::snap(target, rect, &others, canvas, layout::SNAP);
                guides = g;
                moved = Some((idx, snapped));
            }

            // 右下角缩放柄
            let grip = egui::Rect::from_min_size(
                rect.max - egui::vec2(18.0, 18.0),
                egui::vec2(18.0, 18.0),
            );
            let gresp = ui.interact(grip, ui.make_persistent_id(("grip", idx)), egui::Sense::drag());
            if gresp.hovered() || gresp.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
            }
            if gresp.dragged() {
                let nr = match self.cfg.channels.get(idx) {
                    // 锁定比例：只能等比缩放（旋转 90/270 时用竖过来的比例）
                    Some(ch) if ch.aspect_lock => {
                        let aspect = layout::rotated_aspect(
                            self.cfg.render_width as f32,
                            self.cfg.render_height as f32,
                            ch.rotate,
                        );
                        layout::resize_locked(rect, gresp.drag_delta(), aspect, canvas)
                    }
                    // 没锁：随意拉伸，仍然带吸附
                    _ => layout::resize(rect, gresp.drag_delta(), &others, canvas, layout::SNAP),
                };
                moved = Some((idx, nr));
            }

            // 缩放柄的视觉提示（两道斜线）
            let painter = ui.painter_at(rect);
            for off in [4.0f32, 9.0] {
                painter.line_segment(
                    [
                        rect.max - egui::vec2(off + 10.0, off - 10.0),
                        rect.max - egui::vec2(off - 10.0, off + 10.0),
                    ],
                    egui::Stroke::new(1.5, egui::Color32::from_white_alpha(90)),
                );
            }
        }

        if let Some((idx, r)) = moved
            && idx < self.cfg.tiles.len() {
                self.cfg.tiles[idx] = layout::to_norm(r, canvas);
                self.tiles_dirty = true;
            }
        // 松手才落盘：拖动过程中每帧都写文件太浪费
        if self.tiles_dirty && ui.input(|i| i.pointer.any_released()) {
            self.cfg.save().ok();
            self.tiles_dirty = false;
        }

        // 吸附参考线
        let painter = ui.painter_at(canvas);
        for g in guides {
            match g.axis {
                layout::Axis::X => painter.line_segment(
                    [
                        egui::pos2(g.at, canvas.top()),
                        egui::pos2(g.at, canvas.bottom()),
                    ],
                    egui::Stroke::new(1.0, ACCENT.gamma_multiply(0.9)),
                ),
                layout::Axis::Y => painter.line_segment(
                    [
                        egui::pos2(canvas.left(), g.at),
                        egui::pos2(canvas.right(), g.at),
                    ],
                    egui::Stroke::new(1.0, ACCENT.gamma_multiply(0.9)),
                ),
            };
        }
    }

    /// 按当前通道数排一个初始网格（按画面比例，列宽不被拉长）
    /// 按**可见通道**排一个初始网格（禁用的通道不占格子）
    fn reset_tiles(&mut self, canvas: egui::Rect, visible: &[usize]) {
        let n = visible.len().max(1);
        let cols = if n <= 1 {
            1
        } else if n <= 4 {
            2
        } else {
            3
        };
        let rows = n.div_ceil(cols);
        let gap = 8.0;
        let target =
            (self.cfg.render_width as f32 / self.cfg.render_height.max(1) as f32).max(0.1);

        let cw = (canvas.width() - gap * (cols as f32 - 1.0)) / cols as f32;
        let chh = (canvas.height() - gap * (rows as f32 - 1.0)) / rows as f32;
        let cell_w = cw.min(chh * target).max(40.0);
        let cell_h = cell_w / target;

        let grid_w = cols as f32 * cell_w + gap * (cols as f32 - 1.0);
        let grid_h = rows as f32 * cell_h + gap * (rows as f32 - 1.0);
        let ox = canvas.left() + (canvas.width() - grid_w) * 0.5;
        let oy = canvas.top() + (canvas.height() - grid_h) * 0.5;

        // 整张表按通道数建，但只给可见通道写入位置；
        // 其余留 0，等它被启用时再补排
        let mut tiles = vec![[0.0f32; 4]; self.streams.len()];
        for (k, idx) in visible.iter().enumerate() {
            let r = k / cols;
            let c = k % cols;
            let rect = egui::Rect::from_min_size(
                egui::pos2(
                    ox + c as f32 * (cell_w + gap),
                    oy + r as f32 * (cell_h + gap),
                ),
                egui::vec2(cell_w, cell_h),
            );
            if *idx < tiles.len() {
                tiles[*idx] = layout::to_norm(rect, canvas);
            }
        }
        self.cfg.tiles = tiles;
        self.cfg.save().ok();
    }

    /// 一键布局：把所有可见通道整理成均分网格，贴满画布。
    ///
    /// 和 `reset_tiles` 的区别：那里格子会缩到画面比例再居中（四周留边），
    /// 这里格子就是均分的画布块（贴满四边），画面在格内等比缩。
    fn auto_layout(&mut self, canvas: egui::Rect) {
        let visible: Vec<usize> = (0..self.streams.len())
            .filter(|i| self.streams.get(*i).map(|s| s.is_some()).unwrap_or(false))
            .collect();
        if visible.is_empty() {
            return;
        }
        let target =
            (self.cfg.render_width as f32 / self.cfg.render_height.max(1) as f32).max(0.1);
        let (_, cells) = layout::grid_cells(visible.len(), canvas, 6.0, target);

        if self.cfg.tiles.len() != self.streams.len() {
            self.cfg.tiles = vec![[0.0f32; 4]; self.streams.len()];
        }
        for (k, idx) in visible.iter().enumerate() {
            if let (Some(cell), Some(slot)) = (cells.get(k), self.cfg.tiles.get_mut(*idx)) {
                *slot = layout::to_norm(*cell, canvas);
            }
        }
        self.cfg.save().ok();
    }

    /// 抓拍当前帧，存成 PNG。
    ///
    /// 存的是**原始画面**（与录像一致），不含预览的旋转/翻转——
    /// 旋转只影响显示，这点在菜单提示里也写明了。
    fn save_snapshot(&mut self, idx: usize) -> Result<std::path::PathBuf, String> {
        use resvg::tiny_skia::{Pixmap, PremultipliedColorU8};

        let frame = self
            .streams
            .get(idx)
            .and_then(|s| s.as_ref())
            .and_then(|s| s.frame())
            .ok_or_else(|| tr("这一路还没有画面").to_string())?;

        let (w, h) = (frame.w, frame.h);
        if frame.data.len() < w * h * 3 {
            return Err(tr("画面数据不完整").to_string());
        }
        let mut pm =
            Pixmap::new(w as u32, h as u32).ok_or_else(|| tr("无法分配图片缓冲").to_string())?;
        for (i, px) in pm.pixels_mut().iter_mut().enumerate() {
            let o = i * 3;
            *px = PremultipliedColorU8::from_rgba(
                frame.data[o],
                frame.data[o + 1],
                frame.data[o + 2],
                255,
            )
            .unwrap_or(PremultipliedColorU8::TRANSPARENT);
        }
        let png = pm
            .encode_png()
            .map_err(|e| i18n::err(tr("图片编码失败"), e))?;

        let root = crate::record::record_root(&self.cfg);
        let (y, mo, d, hh, mi, ss) = crate::sys::local_now();
        let dir = root.join("快照").join(format!("{y:04}-{mo:02}-{d:02}"));
        std::fs::create_dir_all(&dir).map_err(|e| i18n::err(tr("创建快照目录失败"), e))?;

        let folder = crate::record::channel_folder_pub(&self.cfg.channels, idx);
        let path = dir.join(format!("{folder} {hh:02}-{mi:02}-{ss:02}.png"));
        std::fs::write(&path, png).map_err(|e| i18n::err(tr("保存快照失败"), e))?;
        Ok(path)
    }

    /// 只重连某一路（其它通道不受影响）
    fn reconnect_one(&mut self, idx: usize) {
        if idx >= self.streams.len() {
            return;
        }
        // 先丢掉旧的：Drop 里会 kill + join 取流线程
        self.streams[idx] = None;
        self.textures.remove(&idx);
        self.last_seq.remove(&idx);
        if let Some(d) = self.detect.as_mut() {
            d.results.clear(); // 检测结果按索引存，重连后旧框不能留
        }
        let fresh = self.spawn_one(idx);
        self.streams[idx] = fresh;
        self.toast(tr("正在重新连接"));
    }

    /// 按索引启动单路取流（rebuild_streams 与单路重连共用）
    fn spawn_one(&self, idx: usize) -> Option<Stream> {
        let c = self.cfg.channels.get(idx)?;
        if !c.enabled || c.url.trim().is_empty() {
            return None;
        }
        let ffmpeg = self.ffmpeg.clone()?;
        Some(Stream::spawn(StreamCfg {
            // 监看/识别用子码流（若已配置），录制在 record.rs 里仍走主码流
            url: c.display_rtsp(),
            ffmpeg,
            width: self.cfg.render_width,
            height: self.cfg.render_height,
            fps: self.cfg.fps,
            tcp: self.cfg.tcp,
            hwdec: self.cfg.hwdec.clone(),
        }))
    }

    /// 进出全屏：无边框 + 全屏视口
    fn set_fullscreen(&mut self, ctx: &egui::Context, on: bool) {
        self.fullscreen = on;
        if on && self.solo.is_some() {
            self.solo = None;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(!on));
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
    }

    fn empty_hint(&mut self, ui: &mut egui::Ui) {
        let p = self.pal();
        let rect = ui.available_rect_before_wrap();
        ui.allocate_space(rect.size());
        let painter = ui.painter_at(rect);
        let center = rect.center();
        painter.text(
            center - egui::vec2(0.0, 18.0),
            egui::Align2::CENTER_CENTER,
            tr("还没有视频通道"),
            egui::FontId::proportional(20.0),
            p.text,
        );
        painter.text(
            center + egui::vec2(0.0, 14.0),
            egui::Align2::CENTER_CENTER,
            tr("点右上角「扫描设备」自动发现局域网摄像机，或到「设置」里手动填写 RTSP 地址"),
            egui::FontId::proportional(13.0),
            p.muted,
        );
    }

    fn draw_tile(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        idx: usize,
        sense: egui::Sense,
        canvas: Option<egui::Rect>,
    ) -> egui::Response {
        let p = self.pal();
        let resp = ui.interact(rect, ui.make_persistent_id(("tile", idx)), sense);

        // 画面变换状态先取出来：菜单闭包里要用，又不能再借 self
        let (rot, fh_flip, fv_flip, aspect_lock, crop_on) = self
            .cfg
            .channels
            .get(idx)
            .map(|c| {
                (
                    c.rotate % 360,
                    c.flip_h,
                    c.flip_v,
                    c.aspect_lock,
                    c.crop_borders,
                )
            })
            .unwrap_or((0, false, false, true, false));
        let hovered = resp.hovered();
        let painter = ui.painter_at(rect);

        painter.rect_filled(rect, egui::CornerRadius::ZERO, p.card);

        let name = self
            .cfg
            .channels
            .get(idx)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let status = self
            .streams
            .get(idx)
            .and_then(|s| s.as_ref())
            .map(|s| s.status())
            .unwrap_or_default();
        let tex = self.textures.get(&idx).map(|t| t.id());

        if let Some(tid) = tex {
            let (fw, fh) = (self.cfg.render_width as f32, self.cfg.render_height as f32);

            // 内容区：开了「裁剪黑边」就只取检测到的这块（手机竖屏塞进 16:9 时用得上）
            let crop = self.crops.get(&idx).and_then(|w| w.applied);
            let (cx, cy, cwn, chn) = crop
                .map(|c| (c[0], c[1], c[2], c[3]))
                .unwrap_or((0.0, 0.0, 1.0, 1.0));
            let uv_rect = egui::Rect::from_min_size(
                egui::pos2(cx, cy),
                egui::vec2(cwn.max(0.01), chn.max(0.01)),
            );
            // 按内容像素尺寸算显示比例，而不是整帧尺寸
            let (cw_px, ch_px) = (fw * cwn, fh * chn);
            let (dw, dh) = if rot == 90 || rot == 270 {
                (ch_px, cw_px) // 旋转 90/270 后外框宽高互换
            } else {
                (cw_px, ch_px)
            };
            let scale = (rect.width() / dw).min(rect.height() / dh);
            let img_rect = egui::Rect::from_center_size(
                rect.center(),
                egui::vec2(dw * scale, dh * scale),
            );

            if rot == 0 && !fh_flip && !fv_flip {
                // 无旋转/翻转：painter.image 支持任意 uv 子矩形，裁剪也能走这条快路
                painter.image(tid, img_rect, uv_rect, egui::Color32::WHITE);
            } else {
                // 旋转/翻转：painter.image() 只能贴轴对齐矩形，得自己建网格
                let mut mesh = egui::Mesh::with_texture(tid);
                let corners = [
                    img_rect.left_top(),
                    img_rect.right_top(),
                    img_rect.right_bottom(),
                    img_rect.left_bottom(),
                ];
                for (pos, (u, v)) in corners
                    .iter()
                    .zip(layout::corner_uvs(rot, fh_flip, fv_flip))
                {
                    // corner_uvs 给的是 0..1，要映射进内容区
                    let uu = uv_rect.left() + u * uv_rect.width();
                    let vv = uv_rect.top() + v * uv_rect.height();
                    mesh.vertices.push(egui::epaint::Vertex {
                        pos: *pos,
                        uv: egui::pos2(uu, vv),
                        color: egui::Color32::WHITE,
                    });
                }
                mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
                let _ = painter.add(egui::Shape::mesh(mesh));
            }
        }

        if status.state != State::Live {
            let (color, head) = match status.state {
                State::Connecting => (WARN, tr("连接中…")),
                State::Reconnecting => (ERR, tr("连接中断，重试中")),
                State::Live => (OK, ""),
            };
            painter.text(
                rect.center() - egui::vec2(0.0, 10.0),
                egui::Align2::CENTER_CENTER,
                head,
                egui::FontId::proportional(15.0),
                color,
            );
            if !status.msg.is_empty() {
                painter.text(
                    rect.center() + egui::vec2(0.0, 12.0),
                    egui::Align2::CENTER_CENTER,
                    truncate(&status.msg, 64),
                    egui::FontId::proportional(12.0),
                    p.muted,
                );
            }
        }

        let pad = 8.0;
        // 左下角只显示通道名称，不暴露 RTSP 地址（避免截图/录屏泄露）
        let label = name.clone();
        let galley = painter.layout_no_wrap(label, egui::FontId::proportional(12.0), CHIP_TEXT);
        let chip = egui::Rect::from_min_size(
            egui::pos2(rect.left() + pad, rect.bottom() - pad - galley.size().y - 6.0),
            galley.size() + egui::vec2(12.0, 6.0),
        );
        painter.rect_filled(chip, egui::CornerRadius::ZERO, egui::Color32::from_black_alpha(150));
        painter.galley(chip.min + egui::vec2(6.0, 3.0), galley, p.text);

        let (scolor, txt) = match status.state {
            State::Live => (OK, format!("{:.0} fps", status.fps)),
            State::Connecting => (WARN, tr("连接中").into()),
            State::Reconnecting => (ERR, i18n::reconnect_n(status.reconnects.max(1))),
        };
        let g2 = painter.layout_no_wrap(txt, egui::FontId::proportional(11.5), scolor);
        let chip2 = egui::Rect::from_min_size(
            egui::pos2(rect.right() - pad - g2.size().x - 16.0, rect.top() + pad),
            g2.size() + egui::vec2(16.0, 6.0),
        );
        painter.rect_filled(chip2, egui::CornerRadius::ZERO, egui::Color32::from_black_alpha(150));
        painter.circle_filled(chip2.left_center() + egui::vec2(6.0, 0.0), 3.0, scolor);
        painter.galley(chip2.min + egui::vec2(12.0, 3.0), g2, scolor);

        // 目标检测结果：画框 + 类别/置信度
        if let Some(dets) = self.detect.as_ref().and_then(|d| d.active(idx)) {
            let painter = ui.painter_at(rect);
            for det in dets {
                let x0 = rect.left() + det.x * rect.width();
                let y0 = rect.top() + det.y * rect.height();
                let bw = det.w * rect.width();
                let bh = det.h * rect.height();
                let r = egui::Rect::from_min_size(egui::pos2(x0, y0), egui::vec2(bw, bh));
                let color = if det.label == "人" { WARN } else { OK };
                painter.rect_stroke(
                    r,
                    egui::CornerRadius::same(2),
                    egui::Stroke::new(2.0, color),
                    egui::StrokeKind::Inside,
                );
                let txt = format!("{} {:.0}%", tr(det.label), det.score * 100.0);
                let g = painter.layout_no_wrap(
                    txt,
                    egui::FontId::proportional(11.0),
                    egui::Color32::BLACK,
                );
                let tag = egui::Rect::from_min_size(
                    egui::pos2(r.left(), (r.top() - g.size().y - 2.0).max(rect.top())),
                    g.size() + egui::vec2(6.0, 2.0),
                );
                painter.rect_filled(tag, egui::CornerRadius::same(2), color);
                painter.galley(tag.min + egui::vec2(3.0, 1.0), g, egui::Color32::BLACK);
            }
        }

        // 录制开关：喇叭左边。白=未录，红=录制中
        let rec_on = self.records.is_recording(idx);
        let rec_size = 24.0;
        let rec_anchor = egui::Rect::from_min_size(
            egui::pos2(
                rect.right() - 8.0 - rec_size,
                rect.bottom() - 8.0 - rec_size,
            ),
            egui::vec2(rec_size, rec_size),
        );
        let rec_rect = egui::Rect::from_min_size(
            egui::pos2(rec_anchor.left() - 8.0 - rec_size, rec_anchor.top()),
            egui::vec2(rec_size, rec_size),
        );
        let resp_rec = ui.interact(
            rec_rect.expand(7.0),
            ui.make_persistent_id(("record", idx)),
            egui::Sense::click(),
        );
        {
            let p = ui.painter_at(rect);
            if resp_rec.hovered() {
                p.circle_filled(
                    rec_rect.center(),
                    rec_size * 0.85,
                    egui::Color32::from_black_alpha(90),
                );
            }
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            p.image(
                self.icons.record.id(),
                rec_rect.translate(egui::vec2(1.0, 1.0)),
                uv,
                egui::Color32::from_black_alpha(150),
            );
            let tint = if rec_on { ERR } else { egui::Color32::WHITE };
            p.image(self.icons.record.id(), rec_rect, uv, tint);
        }
        if resp_rec.clicked() {
            let on = self.records.toggle(idx);
            logs::append(&name, if on { tr("开始录制（手动）") } else { tr("停止录制（手动）") });
        }

        // 音频开关（同一时刻只允许一路）：右下角图标，透明底
        let audible = self.audio_idx == Some(idx);
        let size = 24.0;
        let pad = 8.0;
        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(rect.right() - pad - size, rect.bottom() - pad - size),
            egui::vec2(size, size),
        );
        let resp_btn = ui.interact(
            icon_rect.expand(7.0),
            ui.make_persistent_id(("audio", idx)),
            egui::Sense::click(),
        );
        {
            let p = ui.painter_at(rect);
            if resp_btn.hovered() {
                p.circle_filled(
                    icon_rect.center(),
                    size * 0.85,
                    egui::Color32::from_black_alpha(90),
                );
            }
            let tex = if audible {
                self.icons.on.id()
            } else {
                self.icons.muted.id()
            };
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            // 阴影提升亮场景下的可读性
            p.image(
                tex,
                icon_rect.translate(egui::vec2(1.0, 1.0)),
                uv,
                egui::Color32::from_black_alpha(150),
            );
            let tint = if audible {
                egui::Color32::WHITE
            } else {
                egui::Color32::from_white_alpha(200)
            };
            p.image(tex, icon_rect, uv, tint);
        }
        let tip = if audible {
            tr("停止播放这一路的声音")
        } else {
            tr("播放这一路的声音")
        };
        if resp_btn.on_hover_text(tip).clicked() {
            self.toggle_audio(idx);
        }

        if hovered {
            painter.rect_stroke(
                rect,
                egui::CornerRadius::ZERO,
                egui::Stroke::new(1.0, ACCENT.gamma_multiply(0.75)),
                egui::StrokeKind::Inside,
            );
        }

        // 右键菜单：画面变换 / 视图 / 操作
        let mut act: Option<TileAct> = None;
        resp.context_menu(|ui| {
            ui.set_min_width(172.0);
            ui.label(egui::RichText::new(tr("画面")).size(11.0).color(p.muted));
            ui.menu_button(tr("旋转预览"), |ui| {
                for (deg, name) in [
                    (0u16, tr("不旋转")),
                    (90, tr("顺时针 90°")),
                    (180, tr("旋转 180°")),
                    (270, tr("顺时针 270°")),
                ] {
                    if ui.selectable_label(rot == deg, name).clicked() {
                        act = Some(TileAct::Rotate(deg));
                        ui.close();
                    }
                }
            });
            if ui.selectable_label(fh_flip, tr("水平翻转")).clicked() {
                act = Some(TileAct::FlipH);
                ui.close();
            }
            if ui.selectable_label(fv_flip, tr("垂直翻转")).clicked() {
                act = Some(TileAct::FlipV);
                ui.close();
            }
            if ui
                .selectable_label(aspect_lock, tr("比例锁定"))
                .on_hover_text(tr("开启后拖动只能等比缩放；关掉可随意拉伸"))
                .clicked()
            {
                act = Some(TileAct::ToggleLock);
                ui.close();
            }
            if ui
                .selectable_label(crop_on, tr("裁剪黑边"))
                .on_hover_text(tr("自动裁掉画面自带的纯黑边（手机竖屏塞进 16:9 时两侧会有）"))
                .clicked()
            {
                act = Some(TileAct::ToggleCrop);
                ui.close();
            }
            ui.add_enabled_ui(canvas.is_some(), |ui| {
                if ui
                    .button(tr("恢复原比例"))
                    .on_hover_text(tr("把框格调回画面本身的宽高比"))
                    .clicked()
                {
                    act = Some(TileAct::FitAspect);
                    ui.close();
                }
            });

            ui.separator();
            ui.add_enabled_ui(canvas.is_some(), |ui| {
                if ui
                    .button(tr("一键布局"))
                    .on_hover_text(tr("把所有画面整理成规整网格：等比缩放、贴满屏幕"))
                    .clicked()
                {
                    act = Some(TileAct::AutoLayout);
                    ui.close();
                }
            });
            if ui.button(tr("单路全屏")).clicked() {
                act = Some(TileAct::FullscreenOne);
                ui.close();
            }
            if ui
                .button(tr("抓拍快照"))
                .on_hover_text(tr("保存当前原始画面到录像目录下的「快照」"))
                .clicked()
            {
                act = Some(TileAct::Snapshot);
                ui.close();
            }
            if ui
                .button(tr("重新连接"))
                .on_hover_text(tr("只重连这一路，其它通道不受影响"))
                .clicked()
            {
                act = Some(TileAct::Reconnect);
                ui.close();
            }
        });
        if let Some(a) = act {
            self.apply_tile_act(ui.ctx(), idx, a, rect, canvas);
        }

        if !self.fullscreen && resp.double_clicked() {
            self.solo = if self.solo == Some(idx) { None } else { Some(idx) };
        }
        resp
    }

    /// 执行右键菜单选中的动作
    fn apply_tile_act(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
        act: TileAct,
        rect: egui::Rect,
        canvas: Option<egui::Rect>,
    ) {
        let (rw, rh) = (self.cfg.render_width as f32, self.cfg.render_height as f32);
        match act {
            TileAct::Rotate(d) => {
                if let Some(ch) = self.cfg.channels.get_mut(idx) {
                    ch.rotate = d % 360; // 只改预览，不碰取流
                }
            }
            TileAct::FlipH => {
                if let Some(ch) = self.cfg.channels.get_mut(idx) {
                    ch.flip_h = !ch.flip_h;
                }
            }
            TileAct::FlipV => {
                if let Some(ch) = self.cfg.channels.get_mut(idx) {
                    ch.flip_v = !ch.flip_v;
                }
            }
            TileAct::ToggleLock => {
                if let Some(ch) = self.cfg.channels.get_mut(idx) {
                    ch.aspect_lock = !ch.aspect_lock;
                }
            }
            TileAct::ToggleCrop => {
                if let Some(ch) = self.cfg.channels.get_mut(idx) {
                    ch.crop_borders = !ch.crop_borders;
                }
                // 关掉就立刻清掉已生效的裁剪，不用等下一轮检测
                let on = self
                    .cfg
                    .channels
                    .get(idx)
                    .map(|c| c.crop_borders)
                    .unwrap_or(false);
                if !on {
                    self.crops.remove(&idx);
                }
            }
            TileAct::FitAspect => {
                let (Some(cv), Some(ch)) = (canvas, self.cfg.channels.get(idx)) else {
                    return;
                };
                // 旋转 90/270 后「原比例」是竖的，这里必须用旋转后的比例
                let aspect = layout::rotated_aspect(rw, rh, ch.rotate);
                let fitted = layout::fit_aspect(rect, aspect, cv);
                if idx < self.cfg.tiles.len() {
                    self.cfg.tiles[idx] = layout::to_norm(fitted, cv);
                }
            }
            TileAct::AutoLayout => {
                let Some(cv) = canvas else { return };
                self.auto_layout(cv);
                return;
            }
            TileAct::FullscreenOne => {
                self.set_fullscreen(ctx, true);
                self.solo = Some(idx);
                return; // 全屏不动配置
            }
            TileAct::Snapshot => {
                match self.save_snapshot(idx) {
                    Ok(p) => self.toast(i18n::snapshot_saved(&p.to_string_lossy())),
                    Err(e) => self.toast(e),
                }
                return; // 快照不动配置
            }
            TileAct::Reconnect => {
                self.reconnect_one(idx);
                return;
            }
        }
        self.cfg.save().ok();
    }

    // ---------------------------------------------------------------- 设置
    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let mut save = false;
        let mut save_reconnect = false;
        let mut cancel = false;
        let mut tour_start = false;

        // 打开后头几帧持续校准：首帧窗口尺寸未知，pivot 会算偏
        let center = self.center_settings > 0;
        if center {
            self.center_settings -= 1;
        }

        let mut win = egui::Window::new(tr("设置"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(760.0)
            .default_height(640.0);
        if center {
            // anchor 每帧按窗口**实际**尺寸重新居中，不受首帧尺寸未知影响
            win = win.anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO);
        }
        let shown = win.show(ctx, |ui| {
                egui::Panel::bottom("set_actions")
                    .frame(
                        egui::Frame::NONE
                            .inner_margin(egui::Margin::symmetric(0, 8))
                            .fill(ui.visuals().window_fill),
                    )
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if ui
                                .button(tr("保存"))
                                .on_hover_text(
                                    tr("保存全部设置。主题、音量、提示立即生效；通道与解码参数需重连才生效"),
                                )
                                .clicked()
                            {
                                save = true;
                            }
                            if ui.button(tr("保存并重连")).clicked() {
                                save_reconnect = true;
                            }
                            if ui.button(tr("取消")).clicked() {
                                cancel = true;
                            }
                            ui.add_space(12.0);
                            if ui
                                .button(tr("新手引导"))
                                .on_hover_text(tr("重新播放界面导览（不会修改现有设置）"))
                                .clicked()
                            {
                                tour_start = true;
                            }
                            if stream_sig(&self.draft) != stream_sig(&self.cfg) {
                                ui.add_space(8.0);
                                ui.label(
                                    egui::RichText::new(tr("通道/解码设置已改动，需「保存并重连」"))
                                        .size(11.5)
                                        .color(WARN),
                                );
                            }
                        });
                    });

                // 标签页：把互不相关的设置分开，避免一条长滚动条
                egui::Panel::top("set_tabs")
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, 6)))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for (tab, name) in SET_TABS {
                                if ui
                                    .selectable_label(self.settings_tab == *tab, tr(name))
                                    .clicked()
                                {
                                    self.settings_tab = *tab;
                                }
                            }
                        });
                    });

                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
                        // auto_shrink([false,false])：内容再短也不收缩，
                        // 否则窗口宽度会随标签页内容跳来跳去
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                            match self.settings_tab {
                                SetTab::Channels => self.set_tab_channels(ui),
                                SetTab::Recording => self.set_tab_recording(ui),
                                SetTab::Detect => self.set_tab_lab(ui),
                                SetTab::Video => self.set_tab_video(ui),
                                SetTab::General => self.set_tab_general(ui, ctx),
                            }
                        });
                    });
            });

        if let Some(r) = shown {
            self.anchors
                .insert(tour::anchor::SETTINGS_WINDOW, r.response.rect);
        }

        if save || save_reconnect {
            let need_reconnect = stream_sig(&self.draft) != stream_sig(&self.cfg);
            let detect_changed = self.draft.detect != self.cfg.detect;
            self.cfg = self.draft.clone();
            if detect_changed {
                // 设备/频率/夜视/豁免变了：重建推理线程（显卡首次加载约 0.9 秒）
                let prev = self.detect.take();
                let was_on = prev.as_ref().map(|d| d.enabled).unwrap_or(false);
                drop(prev); // 丢掉 tx 后旧线程会在 recv 处退出
                // 关掉实验开关、或模型被删了，就不再拉起推理线程
                if was_on && self.cfg.detect.enabled && self.model_ready {
                    self.detect = Some(DetectRuntime::spawn(ctx.clone(), self.cfg.detect));
                }
            }
            self.volume
                .store(self.cfg.volume.to_bits(), Ordering::Relaxed);
            if self.ffmpeg.is_none() {
                self.ffmpeg = config::find_ffmpeg(&self.cfg.ffmpeg_path);
            }
            apply_style(ctx, self.cfg.dark);
            let saved = self.cfg.save();
            self.show_settings = false;

            if save_reconnect {
                self.rebuild_streams();
                match saved {
                    Ok(_) => self.toast(tr("已保存并重连")),
                    Err(e) => self.toast(i18n::err(tr("重连完成，但保存失败"), e)),
                }
            } else {
                match saved {
                    Ok(_) if need_reconnect => self.toast(
                        tr("已保存。通道/解码改动需点右上角「重连以应用改动」才生效"),
                    ),
                    Ok(_) => self.toast(tr("设置已保存")),
                    Err(e) => self.toast(i18n::err(tr("保存失败"), e)),
                }
            }
        } else if cancel || !open {
            if self.cfg.dark != self.open_dark {
                self.cfg.dark = self.open_dark;
                apply_style(ctx, self.cfg.dark);
            }
            if self.cfg.lang != self.open_lang {
                self.cfg.lang = self.open_lang;
                crate::i18n::set_lang(self.cfg.lang);
                apply_window_title(ctx);
            }
            self.show_settings = false;
        }

        if tour_start {
            self.start_tour(false);
        }
    }

    // ------------------------------------------------------------ 设置：标签页

    /// ① 通道：只放「连什么」，录像参数挪到录像页
    fn set_tab_channels(&mut self, ui: &mut egui::Ui) {
        let p = self.pal();
        let mut remove: Option<usize> = None;

        for (i, ch) in self.draft.channels.iter_mut().enumerate() {
            ui.push_id(("ch", i), |ui| {
                egui::Frame::NONE
                    .fill(p.card)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut ch.name)
                                    .hint_text(tr("名称"))
                                    .desired_width(170.0),
                            );
                            ui.checkbox(&mut ch.enabled, tr("启用"));
                            if ch.uses_substream() {
                                ui.label(
                                    egui::RichText::new(tr("子码流")).size(10.5).color(p.muted),
                                );
                            }
                            if ui.button(tr("删除")).clicked() {
                                remove = Some(i);
                            }
                        });
                        ui.add_space(6.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut ch.url)
                                .hint_text("rtsp://192.168.1.10:554/Streaming/Channels/102")
                                .desired_width(f32::INFINITY),
                        );
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut ch.username)
                                    .hint_text(tr("用户名"))
                                    .desired_width(140.0),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut ch.password)
                                    .hint_text(tr("密码"))
                                    .password(true)
                                    .desired_width(140.0),
                            );
                            help(ui, tr("留空则用地址里自带的账号"));
                        });
                        // 子码流（可选）：只影响监看，录制永远走主码流
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let w = (ui.available_width() - 96.0).max(160.0);
                            ui.add(
                                egui::TextEdit::singleline(&mut ch.sub_url)
                                    .hint_text(tr("子码流地址（可选，留空则用主码流）"))
                                    .desired_width(w),
                            );
                            if ui
                                .button(tr("自动"))
                                .on_hover_text(tr(
                                    "按常见规则从主码流推导：海康 101→102、201→202；大华 subtype=0→1",
                                ))
                                .clicked()
                                && let Some(d) = crate::config::derive_sub_url(&ch.url) {
                                    ch.sub_url = d;
                                }
                            help(
                                ui,
                                tr("填了之后监看走这一路：旧电脑解 704×576 比解 2560×1440 轻得多，内存和带宽也省；
录制仍用主码流，画质完全不受影响。代价是子码流分辨率低，放大看会糊、识别也会略降"),
                            );
                        });
                    });
            });
            ui.add_space(8.0);
        }

        if let Some(i) = remove {
            self.draft.channels.remove(i);
        }
        if ui.button(tr("+ 添加通道")).clicked() {
            self.draft.channels.push(Channel {
                record: Default::default(),
                rotate: 0,
                flip_h: false,
                flip_v: false,
                aspect_lock: true,
                crop_borders: false,
                name: i18n::channel_n(self.draft.channels.len() + 1),
                url: "rtsp://".into(),
                sub_url: String::new(),
                username: String::new(),
                password: String::new(),
                enabled: true,
            });
        }
    }

    /// ② 录像：存档位置 + 配额 + 每通道录制参数
    fn set_tab_recording(&mut self, ui: &mut egui::Ui) {
        let p = self.pal();
        let root = crate::record::record_root(&self.draft);

        ui.label(egui::RichText::new(tr("存档目录")).strong());
        ui.add_space(4.0);
        // 输入框单独占一行：把 desired_width(INFINITY) 塞进 horizontal 会让窗口被撑到全屏宽
        ui.add(
            egui::TextEdit::singleline(&mut self.draft.record_dir)
                .hint_text(tr("留空 = 用户目录\\Videos\\CamNexus"))
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(tr("打开目录")).clicked()
                && let Err(e) = crate::record::open_folder(&root) {
                    self.setup_msg = Some(e);
                }
            ui.label(
                egui::RichText::new(root.display().to_string())
                    .size(11.0)
                    .color(p.muted),
            );
        });

        ui.add_space(12.0);
        ui.label(egui::RichText::new(tr("占用上限")).strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            // 界面上按 GB 调，配置里仍存 MB
            let mut gb = (self.draft.record_max_mb / 1024) as u32;
            if ui
                .add(
                    egui::DragValue::new(&mut gb)
                        .range(0..=100_000)
                        .speed(1.0)
                        .suffix(" GB"),
                )
                .changed()
            {
                self.draft.record_max_mb = gb as u64 * 1024;
            }
            help(
                ui,
                tr("0 = 不限制；超出后自动删除最早的文件。\n实际生效值不会超过磁盘剩余空间（给系统留 512 MB）"),
            );
        });
        let used = self.records.dir_size_cached(&root);
        let used_mb = used as f64 / 1_048_576.0;
        let cap = self.draft.record_max_mb;
        let usage = if cap == 0 {
            i18n::disk_usage_free(used_mb)
        } else {
            let free = cap.saturating_sub(used / 1_048_576);
            i18n::disk_usage(used_mb, cap, free)
        };
        ui.label(egui::RichText::new(usage).size(11.0).color(p.muted));

        // 磁盘剩余：上限大于实际可用空间时给个明确提示，
        // 否则用户会以为「设了 500 GB 就能录 500 GB」
        if let Some(free) = crate::sys::free_bytes(&root) {
            let free_mb = free as f64 / 1_048_576.0;
            let over = cap > 0 && cap.saturating_mul(1_048_576) > free.saturating_add(used);
            let mut line = i18n::disk_free(free_mb);
            if over {
                line = format!(
                    "{} · {}",
                    line,
                    tr("已超过磁盘可用空间，实际按剩余空间控制")
                );
            }
            ui.label(
                egui::RichText::new(line)
                    .size(11.0)
                    .color(if over { WARN } else { p.muted }),
            );
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr("每通道录制参数")).strong());
            help(
                ui,
                tr("默认跟随送显分辨率；画质越高越清晰、占空间也越大"),
            );
        });
        ui.add_space(6.0);

        for (i, ch) in self.draft.channels.iter_mut().enumerate() {
            ui.push_id(("rec", i), |ui| {
                egui::Frame::NONE
                    .fill(p.card)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(ch.name.clone()).size(12.0).strong());
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            // 分辨率：源画质排第一，它是默认项，也是最省 CPU 的
                            let label = if ch.record.copy {
                                tr("源画质（不转码）").to_string()
                            } else if ch.record.width == 0 {
                                tr("跟随送显").to_string()
                            } else {
                                format!("{}×{}", ch.record.width, ch.record.height)
                            };
                            egui::ComboBox::from_id_salt(("rec_res", i))
                                .selected_text(label)
                                .width(150.0)
                                .show_ui(ui, |ui| {
                                    let mut pick: Option<(u32, u32)> = None;
                                    let mut pick_copy = false;
                                    if ui
                                        .selectable_label(
                                            ch.record.copy,
                                            tr("源画质（不转码，推荐）"),
                                        )
                                        .clicked()
                                    {
                                        pick_copy = true;
                                    }
                                    if ui
                                        .selectable_label(
                                            !ch.record.copy && ch.record.width == 0,
                                            tr("跟随送显"),
                                        )
                                        .clicked()
                                    {
                                        pick = Some((0, 0));
                                    }
                                    for (w, h) in [
                                        (640u32, 360u32),
                                        (960, 540),
                                        (1280, 720),
                                        (1920, 1080),
                                    ] {
                                        let sel = !ch.record.copy
                                            && ch.record.width == w
                                            && ch.record.height == h;
                                        if ui
                                            .selectable_label(sel, format!("{w} × {h}"))
                                            .clicked()
                                        {
                                            pick = Some((w, h));
                                        }
                                    }
                                    if let Some((w, h)) = pick {
                                        ch.record.copy = false;
                                        ch.record.width = w;
                                        ch.record.height = h;
                                    } else if pick_copy {
                                        ch.record.copy = true;
                                    }
                                });
                            help(
                                ui,
                                tr("「源画质」把摄像机的原始码流直接封装进 mp4，不解码也不转码，CPU 占用接近 0，文件保留摄像机自己的编码（多为 H.265）；\n选具体分辨率会实时转码，画质可控但很吃 CPU——旧电脑做录像机时建议保持源画质"),
                            );

                            // 下面这些只在转码时才有意义，不转码时置灰，
                            // 免得用户以为改了有用
                            let transcoding = !ch.record.copy;
                            ui.add_enabled_ui(transcoding, |ui| {
                                ui.horizontal(|ui| {
                                    // 画质档位
                                    let mut picked_q: Option<crate::config::RecQuality> = None;
                                    egui::ComboBox::from_id_salt(("rec_q", i))
                                        .selected_text(ch.record.quality.label())
                                        .width(88.0)
                                        .show_ui(ui, |ui| {
                                            use crate::config::RecQuality::*;
                                            for (q, tip) in [
                                                (
                                                    High,
                                                    tr("crf 28 / 每 4 秒一个关键帧\n体积最大，崩溃最多丢 4 秒"),
                                                ),
                                                (
                                                    Balanced,
                                                    tr("crf 30 / 每 8 秒一个关键帧\n约省 30%，日常推荐"),
                                                ),
                                                (
                                                    Small,
                                                    tr("crf 32 / 每 8 秒一个关键帧\n约省 47%，画质略降"),
                                                ),
                                            ] {
                                                if ui
                                                    .selectable_label(
                                                        ch.record.quality == q,
                                                        q.label(),
                                                    )
                                                    .on_hover_text(tip)
                                                    .clicked()
                                                {
                                                    picked_q = Some(q);
                                                }
                                            }
                                        });
                                    if let Some(q) = picked_q {
                                        ch.record.quality = q;
                                    }

                                    // 编码二选一（用 checkbox 的观感，行为互斥）
                                    let mut is265 =
                                        ch.record.codec == crate::config::RecCodec::H265;
                                    let mut is264 = !is265;
                                    if ui.checkbox(&mut is265, "H.265").clicked() {
                                        ch.record.codec = crate::config::RecCodec::H265;
                                    }
                                    if ui.checkbox(&mut is264, "H.264").clicked() {
                                        ch.record.codec = crate::config::RecCodec::H264;
                                    }
                                    help(
                                        ui,
                                        tr("H.265 同画质体积约省 40%，但部分系统需装 HEVC 解码包才能播；\nH.264 兼容性最好，任何播放器都能播，代价是文件更大"),
                                    );
                                });
                            });
                            ui.add_space(8.0);
                            ui.checkbox(&mut ch.record.audio, tr("录制声音"));
                        });
                    });
            });
            ui.add_space(8.0);
        }
    }

    /// ③ 实验：智能识别（默认关闭，模型按需下载）
    fn set_tab_lab(&mut self, ui: &mut egui::Ui) {
        let p = self.pal();
        let mut start_download = false;
        let mut remove_model = false;

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr("实验功能")).size(13.0).strong());
            help(
                ui,
                tr("这里的玩意儿还在验证阶段：可能不稳定，也可能在后续版本里改掉或直接移除。\n用不上就别开，开了也不会影响录像和监看"),
            );
        });
        ui.add_space(8.0);

        // ---------- 开关 ----------
        let ready = self.model_ready;
        egui::Frame::NONE
            .fill(p.card)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.add_enabled_ui(ready, |ui| {
                    if ui
                        .checkbox(&mut self.draft.detect.enabled, tr("启用智能识别"))
                        .changed()
                        && !self.draft.detect.enabled
                    {
                        // 关掉时顺手清空画面上残留的框
                        if let Some(d) = self.detect.as_mut() {
                            d.results.clear();
                        }
                    }
                });
                if !ready {
                    ui.label(
                        egui::RichText::new(tr("需要先下载模型才能启用"))
                            .size(11.0)
                            .color(p.muted),
                    );
                }
            });

        ui.add_space(12.0);
        ui.label(egui::RichText::new(tr("检测模型")).strong());
        ui.add_space(6.0);

        // ---------- 模型状态 ----------
        let state = self
            .model_state
            .lock()
            .map(|g| g.clone())
            .unwrap_or(crate::model::State::Idle);

        egui::Frame::NONE
            .fill(p.card)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| match &state {
                crate::model::State::Downloading { got, total } => {
                    let frac = if *total > 0 {
                        *got as f32 / *total as f32
                    } else {
                        0.0
                    };
                    ui.label(tr("正在下载模型…"));
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .show_percentage()
                            .desired_width(ui.available_width()),
                    );
                }
                crate::model::State::Verifying => {
                    ui.label(tr("正在校验…"));
                }
                crate::model::State::Failed(e) => {
                    ui.label(egui::RichText::new(e.clone()).size(11.5).color(ERR));
                    if ui.button(tr("重试")).clicked() {
                        start_download = true;
                    }
                }
                _ => {
                    if ready {
                        ui.horizontal(|ui| {
                            ui.label(tr("已安装"));
                            ui.label(
                                egui::RichText::new(format!(
                                    "{:.1} MB",
                                    crate::model::SIZE as f64 / 1_000_000.0
                                ))
                                .size(11.0)
                                .color(p.muted),
                            );
                            ui.add_space(6.0);
                            if ui.button(tr("删除")).clicked() {
                                remove_model = true;
                            }
                        });
                    } else {
                        ui.horizontal(|ui| {
                            ui.label(tr("未下载"));
                            ui.add_space(6.0);
                            if ui.button(tr("下载模型")).clicked() {
                                start_download = true;
                            }
                            help(
                                ui,
                                tr("约 35.9 MB，从 YOLOX 官方发布地址下载并校验 SHA-256；\n装好的文件放在 %LOCALAPPDATA%\\CamNexus\\models，删掉软件也就干净了"),
                            );
                        });
                    }
                }
            });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(tr("模型：YOLOX-s · Apache-2.0"))
                    .size(10.5)
                    .color(p.muted),
            );
        });

        if start_download {
            self.model_state = crate::model::spawn_download();
        }
        if remove_model {
            match crate::model::remove() {
                Ok(()) => {
                    self.model_ready = false;
                    self.draft.detect.enabled = false;
                    if let Ok(mut g) = self.model_state.lock() {
                        *g = crate::model::State::Idle;
                    }
                    self.toast(tr("模型已删除"));
                }
                Err(e) => self.toast(e),
            }
        }

        // ---------- 参数（启用后才可调） ----------
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
        ui.add_enabled_ui(self.draft.detect.enabled, |ui| {
            let gpu = self.draft.detect.device == crate::config::DetectDevice::Gpu;

            ui.label(egui::RichText::new(tr("计算设备")).strong());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.selectable_label(!gpu, "CPU").clicked() {
                    self.draft.detect.device = crate::config::DetectDevice::Cpu;
                }
                if ui.selectable_label(gpu, tr("显卡")).clicked() {
                    self.draft.detect.device = crate::config::DetectDevice::Gpu;
                }
                ui.add_space(6.0);
                help(
                    ui,
                    tr("CPU 单帧约 35 ms，显卡约 7 ms；CPU 模式固定 1 FPS，显卡可到 5 FPS"),
                );
            });

            ui.add_space(10.0);
            ui.label(egui::RichText::new(tr("检测频率")).strong());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let mut fps = self.draft.detect.fps.clamp(1, 5);
                ui.add_enabled_ui(gpu, |ui| {
                    ui.add(
                        egui::DragValue::new(&mut fps)
                            .range(1..=5)
                            .speed(0.2)
                            .suffix(" FPS"),
                    );
                });
                self.draft.detect.fps = fps.clamp(1, 5);
                if !gpu {
                    ui.label(
                        egui::RichText::new(tr("（CPU 固定 1 FPS）"))
                            .size(11.0)
                            .color(p.muted),
                    );
                } else {
                    help(ui, tr("频率越高响应越快，但更吃显卡；1–5 之间取整数"));
                }
            });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.draft.detect.night_opt, tr("夜视优化"));
                help(ui, tr("夜间识别不到目标时开启，会先增强对比度再推理"));
            });

            ui.add_space(10.0);
            ui.label(egui::RichText::new(tr("静止物体豁免")).strong());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let cur = match self.draft.detect.static_grace_secs {
                    0 => tr("关闭").to_string(),
                    30 => tr("30 秒").to_string(),
                    60 => tr("1 分钟").to_string(),
                    300 => tr("5 分钟").to_string(),
                    600 => tr("10 分钟").to_string(),
                    v => i18n::seconds(v),
                };
                let mut pick: Option<u32> = None;
                egui::ComboBox::from_id_salt("static_grace")
                    .selected_text(cur)
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for (v, label) in [
                            (0u32, tr("关闭")),
                            (30, tr("30 秒")),
                            (60, tr("1 分钟")),
                            (300, tr("5 分钟")),
                            (600, tr("10 分钟")),
                        ] {
                            if ui
                                .selectable_label(self.draft.detect.static_grace_secs == v, label)
                                .clicked()
                            {
                                pick = Some(v);
                            }
                        }
                    });
                if let Some(v) = pick {
                    self.draft.detect.static_grace_secs = v;
                }
                help(
                    ui,
                    tr("停放很久的车、站着不动的人会暂时不再画框，一旦移动立刻恢复"),
                );
            });
        });
    }

    /// ④ 画面：分辨率 / 解码 / 帧率 / 传输 / 音量
    fn set_tab_video(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new(tr("送显分辨率")).strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let cur = format!(
                "{} × {}",
                self.draft.render_width, self.draft.render_height
            );
            let mut picked: Option<(u32, u32)> = None;
            egui::ComboBox::from_id_salt("res")
                .selected_text(cur)
                .show_ui(ui, |ui| {
                    for (w, h) in RES_PRESETS {
                        let selected =
                            self.draft.render_width == *w && self.draft.render_height == *h;
                        if ui
                            .selectable_label(selected, format!("{w} × {h}"))
                            .clicked()
                        {
                            picked = Some((*w, *h));
                        }
                    }
                });
            if let Some((w, h)) = picked {
                self.draft.render_width = w;
                self.draft.render_height = h;
            }
            help(
                ui,
                tr("网格模式每格的实际像素远小于此值，调高只在双击放大单路时才有意义；\n目标较小的场景（远处的人/车）降低分辨率反而更容易被识别"),
            );
        });

        ui.add_space(10.0);
        ui.label(egui::RichText::new(tr("解码方式")).strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let is_sw = self.draft.hwdec.is_empty() || self.draft.hwdec == "none";
            let cur = if is_sw {
                tr("软件解码（CPU）").to_string()
            } else {
                i18n::hardware_dash(config::hwdec_label(&self.draft.hwdec))
            };
            let mut picked: Option<String> = None;
            egui::ComboBox::from_id_salt("hwdec")
                .selected_text(cur)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(is_sw, tr("软件解码（CPU）"))
                        .clicked()
                    {
                        picked = Some("none".into());
                    }
                    if self.hwaccels.is_empty() {
                        ui.label(
                            egui::RichText::new(tr("未检测到可用硬件后端"))
                                .size(11.0)
                                .color(self.pal().muted),
                        );
                    }
                    for hw in &self.hwaccels {
                        let sel = self.draft.hwdec == *hw;
                        if ui
                            .selectable_label(sel, format!("{}  ({hw})", config::hwdec_label(hw)))
                            .clicked()
                        {
                            picked = Some(hw.clone());
                        }
                    }
                });
            if let Some(picked) = picked {
                self.draft.hwdec = picked;
            }
            help(
                ui,
                tr("硬件解码只接管「解码」这一步，缩放仍在 CPU——实测比 GPU 内缩放快约 2.4 倍"),
            );
        });

        ui.add_space(10.0);
        ui.label(egui::RichText::new(tr("帧率")).strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let cur = if self.draft.fps == 0 {
                tr("跟随源（推荐）").to_string()
            } else {
                format!("{} fps", self.draft.fps)
            };
            let mut picked: Option<u32> = None;
            egui::ComboBox::from_id_salt("fps")
                .selected_text(cur)
                .show_ui(ui, |ui| {
                    for v in [0u32, 10, 12, 15, 20, 25, 30] {
                        let label = if v == 0 {
                            tr("跟随源（推荐）").to_string()
                        } else {
                            format!("{v} fps")
                        };
                        if ui.selectable_label(self.draft.fps == v, label).clicked() {
                            picked = Some(v);
                        }
                    }
                });
            if let Some(v) = picked {
                self.draft.fps = v;
            }
            help(
                ui,
                tr("跟随源可避免复制帧造成的节奏不均；源帧率低于目标时强制提帧只是重复画面"),
            );
        });

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.draft.tcp, tr("使用 TCP 传输"));
            help(ui, tr("更稳，推荐；UDP 延迟略低但容易丢包花屏"));
        });

        ui.add_space(10.0);
        ui.label(egui::RichText::new(tr("音量")).strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::Slider::new(&mut self.draft.volume, 0.0..=4.0).show_value(false),
            );
            ui.label(format!("{:.0}%", self.draft.volume * 100.0));
            help(ui, tr("摄像机麦克风通常很轻，可适当提高"));
        });
    }

    /// ⑤ 通用：外观、提示、诊断、FFmpeg
    fn set_tab_general(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let p = self.pal();

        ui.label(egui::RichText::new(tr("界面语言")).strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let cur = self.draft.lang;
            for l in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
                if ui.selectable_label(cur == l, l.label()).clicked() && cur != l {
                    self.draft.lang = l;
                    // 即时预览：立刻换文案，取消时再还原
                    self.cfg.lang = l;
                    crate::i18n::set_lang(l);
                    apply_window_title(ctx);
                }
            }
        });

        ui.add_space(10.0);
        if ui.checkbox(&mut self.draft.dark, tr("深色主题")).changed() {
            // 即时预览：勾选即生效，取消时再还原
            self.cfg.dark = self.draft.dark;
            apply_style(ctx, self.cfg.dark);
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.draft.show_hints, tr("运行提示"));
            help(
                ui,
                tr("画面不流畅或占用过高时给出提示（不会自动改配置）"),
            );
        });

        ui.add_space(6.0);
        if ui.button(tr("查看运行状态详情")).clicked() {
            self.show_details = true;
        }

        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("FFmpeg").strong());
            help(
                ui,
                tr("留空则每次启动自动查找；找不到会提示下载精简版（约 79 MB）"),
            );
        });
        ui.add_space(4.0);
        let fstate = match &self.ffmpeg {
            Some(path) => i18n::found_path(&path.display().to_string()),
            None => tr("未找到 ffmpeg.exe").to_string(),
        };
        ui.label(egui::RichText::new(fstate).size(11.0).color(p.muted));
        ui.add(
            egui::TextEdit::singleline(&mut self.draft.ffmpeg_path)
                .hint_text(tr("手动指定 ffmpeg.exe 完整路径（可留空自动查找）"))
                .desired_width(f32::INFINITY),
        );
    }

    // ---------------------------------------------------------------- 扫描
    fn scan_window(&mut self, ctx: &egui::Context) {
        let p = self.pal();
        if !self.show_scan {
            return;
        }
        let mut open = true;
        let mut add: Option<Channel> = None;

        // 打开后头几帧持续校准，其余同上
        let center = self.center_scan > 0;
        if center {
            self.center_scan -= 1;
        }

        let mut win = egui::Window::new(tr("扫描局域网设备"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(860.0)
            .default_height(560.0);
        if center {
            win = win.anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO);
        }
        let shown = win.show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr("网段"));
                    ui.add(egui::TextEdit::singleline(&mut self.scan_prefix).desired_width(110.0));
                    ui.label(tr("用户名"));
                    ui.add(egui::TextEdit::singleline(&mut self.scan_user).desired_width(90.0));
                    ui.label(tr("密码"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.scan_pass)
                            .password(true)
                            .desired_width(110.0),
                    );
                    if ui
                        .add_enabled(!self.scan_running, egui::Button::new(tr("开始扫描")))
                        .clicked()
                    {
                        self.start_scan();
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        tr("密码只在本机内存中使用，不会上传。注意：多数设备在密码连续错误若干次后会锁定账号一段时间，扫描前请确认密码正确；不确定时可先留空，只探测端口与厂商信息。"),
                    )
                    .size(11.0)
                    .color(p.muted),
                );

                if self.scan_running || self.scan_progress.0 > 0 {
                    let (d, t) = self.scan_progress;
                    ui.add(
                        egui::ProgressBar::new(d as f32 / t.max(1) as f32)
                            .text(format!("{d}/{t}")),
                    );
                }

                ui.add_space(8.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if self.scan_devices.is_empty() && !self.scan_running {
                        ui.label(egui::RichText::new(tr("暂无结果")).color(p.muted));
                    }
                    for dev in &self.scan_devices {
                        egui::Frame::NONE
                            .fill(p.card)
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(&dev.ip).strong().color(p.text));
                                    if !dev.hint.is_empty() {
                                        ui.label(
                                            egui::RichText::new(&dev.hint).size(12.0).color(ACCENT),
                                        );
                                    }
                                    let ports = dev
                                        .ports
                                        .iter()
                                        .map(|p| p.to_string())
                                        .collect::<Vec<_>>()
                                        .join(" ");
                                    ui.label(
                                        egui::RichText::new(i18n::ports(&ports))
                                            .size(11.5)
                                            .color(p.muted),
                                    );
                                });
                                for u in &dev.urls {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(if u.ok { "✔" } else { "·" })
                                                .color(if u.ok { OK } else { p.muted }),
                                        );
                                        ui.label(
                                            egui::RichText::new(&u.url)
                                                .size(12.0)
                                                .color(if u.ok { p.text } else { p.muted }),
                                        );
                                        ui.label(
                                            egui::RichText::new(&u.detail)
                                                .size(11.0)
                                                .color(if u.ok { OK } else { p.muted }),
                                        );
                                        if u.ok && ui.small_button(tr("添加")).clicked() {
                                            add = Some(Channel {
                                                record: Default::default(),
                                                rotate: 0,
                                                flip_h: false,
                                                flip_v: false,
                                                aspect_lock: true,
                                                crop_borders: false,
                                                name: format!(
                                                    "{} {}",
                                                    if dev.hint.is_empty() { tr("摄像头") } else { &dev.hint },
                                                    dev.ip.rsplit('.').next().unwrap_or("")
                                                ),
                                                url: strip_creds(&u.url),
                                                // 子码流不自动猜：猜错会让这一路直接连不上，
                                                // 需要省资源时在设置里点「自动」或手填
                                                sub_url: String::new(),
                                                username: self.scan_user.clone(),
                                                password: self.scan_pass.clone(),
                                                enabled: true,
                                            });
                                        }
                                    });
                                }
                            });
                        ui.add_space(4.0);
                    }
                });
            });

        if let Some(r) = shown {
            self.anchors
                .insert(tour::anchor::SCAN_WINDOW, r.response.rect);
        }

        if let Some(ch) = add {
            self.cfg.channels.push(ch);
            self.cfg.save().ok();
            self.draft = self.cfg.clone();
            self.rebuild_streams();
            self.toast(tr("已添加通道"));
        }
        if !open {
            self.show_scan = false;
        }
    }

    fn toast_layer(&mut self, ui: &egui::Ui) {
        let p = self.pal();
        let expired = match &self.toast {
            Some((_, at)) => at.elapsed().as_secs() >= 3,
            None => false,
        };
        if expired {
            self.toast = None;
            return;
        }
        if let Some((msg, _)) = &self.toast {
            let msg = msg.clone();
            let id = ui.make_persistent_id("toast");
            egui::Area::new(id)
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
                .show(ui.ctx(), |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(30, 36, 46))
                        .inner_margin(egui::Margin::symmetric(14, 8))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(msg).color(p.text));
                        });
                });
        }
    }

    // ---------------------------------------------------------------- 运行时诊断
    /// 当前是软解、且本机存在可用硬件后端时，优先建议开硬解而非降分辨率
    fn hw_upgradable(&self) -> bool {
        let sw = self.cfg.hwdec.is_empty() || self.cfg.hwdec == "none";
        sw && !self.hwaccels.is_empty()
    }

    fn sample_metrics(&mut self) {
        if self.last_sample.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_sample = Instant::now();

        let main_cpu = self.main_cpu.sample(sys::current_pid());
        let sys_cpu = self.sys_cpu.sample();

        let mut streams = Vec::new();
        let mut decode_total = 0.0f32;
        for (i, slot) in self.streams.iter().enumerate() {
            let Some(s) = slot else { continue };
            let st = s.status();
            let cpu = s.sample_cpu();
            decode_total += cpu;
            streams.push(StreamMetrics {
                live: st.state == State::Live,
                fps: st.fps,
                // 0 = 跟随源模式：没有帧率目标，帧率达成率不参与判定
                target_fps: self.cfg.fps as f32,
                jitter_ms: st.jitter_ms,
                cpu_pct: cpu,
                upload_ms: self.upload_ms.get(&i).copied().unwrap_or(0.0),
            });
        }

        self.diag.push(Snapshot {
            streams,
            main_cpu_pct: main_cpu,
            app_cpu_pct: decode_total + main_cpu,
            sys_cpu_pct: sys_cpu,
        });
    }

    fn apply_action(&mut self, action: Action) {
        match action {
            Action::LowerResolution => {
                let cur = (self.cfg.render_width, self.cfg.render_height);
                let idx = RES_PRESETS
                    .iter()
                    .position(|p| *p == cur)
                    .unwrap_or(RES_PRESETS.len() - 1);
                if idx == 0 {
                    self.toast(tr("已是最低档 640×360"));
                    self.hint_dismissed = true;
                    return;
                }
                let (w, h) = RES_PRESETS[idx - 1];
                self.cfg.render_width = w;
                self.cfg.render_height = h;
                self.draft = self.cfg.clone();
                self.cfg.save().ok();
                self.rebuild_streams();
                self.upload_ms.clear();
                self.hint_dismissed = true;
                self.toast(i18n::lowered(w, h));
            }
            Action::UseTcp => {
                self.cfg.tcp = true;
                self.draft = self.cfg.clone();
                self.cfg.save().ok();
                self.rebuild_streams();
                self.hint_dismissed = true;
                self.toast(tr("已切换为 TCP 传输"));
            }
            Action::UseHardware => {
                let pick = ["d3d11va", "cuda", "d3d12va", "qsv", "amf"]
                    .iter()
                    .find(|n| self.hwaccels.iter().any(|h| h == *n));
                match pick {
                    Some(hw) => {
                        self.cfg.hwdec = (*hw).to_string();
                        self.draft = self.cfg.clone();
                        self.cfg.save().ok();
                        self.rebuild_streams();
                        self.hint_dismissed = true;
                        self.toast(i18n::hw_enabled(config::hwdec_label(hw)));
                    }
                    None => {
                        self.toast(tr("本机 ffmpeg 未提供可用的硬件解码后端"));
                        self.hint_dismissed = true;
                    }
                }
            }
        }
    }

    fn hint_bar(&mut self, ui: &mut egui::Ui) {
        let p = self.pal();
        if !self.cfg.show_hints || self.hint_dismissed {
            return;
        }
        let hw_up = self.hw_upgradable();
        let Some(a) = self.diag.evaluate(hw_up) else {
            return;
        };
        let text = a.text.clone();
        let action = a.action;
        let action_label = a.action_label;

        let mut do_action = false;
        let mut dismiss = false;
        let mut details = false;

        // 用普通 Frame 条带而不是底部 Panel：Panel 嵌套在 CentralPanel 里
        // 会出现「预留空间与绘制区域错位」，表现为提示条被裁掉一半。
        egui::Frame::NONE
            .fill(p.warn_bg)
            .inner_margin(egui::Margin::symmetric(12, 9))
            .corner_radius(egui::CornerRadius::same(4))
            .show(ui, |ui| {
                // 单行显示：文字过长时省略，不要折行
                egui::containers::Sides::new()
                    .height(22.0)
                    .truncate()
                    .show(
                        ui,
                        |ui| {
                            ui.label(egui::RichText::new("⚠").size(14.0).color(WARN));
                            ui.add(
                                egui::Label::new(egui::RichText::new(text).size(12.5).color(p.text))
                                    .truncate(),
                            );
                        },
                        |ui| {
                            if action.is_some() && ui.button(action_label).clicked() {
                                do_action = true;
                            }
                            if ui.button(tr("详情")).clicked() {
                                details = true;
                            }
                            if ui.button(tr("忽略")).clicked() {
                                dismiss = true;
                            }
                        },
                    );
            });
        ui.add_space(6.0);

        if dismiss {
            self.hint_dismissed = true;
        }
        if details {
            self.show_details = true;
        }
        if do_action
            && let Some(act) = action {
                self.apply_action(act);
            }
    }

    fn details_window(&mut self, ctx: &egui::Context) {
        let p = self.pal();
        if !self.show_details {
            return;
        }
        let mut open = true;
        let report = self.diag.report();
        let hist: Vec<Snapshot> = self.diag.history().iter().cloned().collect();
        let target_label = if self.cfg.fps > 0 {
            format!("{} fps", self.cfg.fps)
        } else {
            tr("跟随源").to_string()
        };

        egui::Window::new(tr("运行状态详情"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(540.0)
            .show(ctx, |ui| {
                if let Some(last) = hist.last() {
                    ui.horizontal(|ui| {
                        ui.label(i18n::cpu_row(i18n::CpuRow::System, last.sys_cpu_pct));
                        ui.separator();
                        ui.label(i18n::cpu_row(i18n::CpuRow::Main, last.main_cpu_pct));
                        ui.separator();
                        ui.label(i18n::cpu_row(i18n::CpuRow::App, last.app_cpu_pct));
                    });
                    ui.add_space(6.0);

                    egui::Grid::new("diag_grid")
                        .num_columns(5)
                        .spacing([14.0, 6.0])
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(tr("通道")).color(p.muted));
                            ui.label(egui::RichText::new(tr("状态")).color(p.muted));
                            ui.label(egui::RichText::new(tr("帧率")).color(p.muted));
                            ui.label(egui::RichText::new(tr("抖动")).color(p.muted));
                            ui.label(egui::RichText::new(tr("解码CPU / 合成")).color(p.muted));
                            ui.end_row();
                            for (i, m) in last.streams.iter().enumerate() {
                                ui.label(i18n::channel_n(i + 1));
                                ui.label(if m.live { tr("在线") } else { tr("离线") });
                                if m.target_fps > 0.0 {
                                    ui.label(format!("{:.1}/{:.0}", m.fps, m.target_fps));
                                } else {
                                    ui.label(i18n::follow_source_fps(m.fps));
                                }
                                ui.label(format!("{:.0} ms", m.jitter_ms));
                                ui.label(format!("{:.0}% / {:.1} ms", m.cpu_pct, m.upload_ms));
                                ui.end_row();
                            }
                        });

                    ui.add_space(8.0);
                    // 有目标帧率时画「达成率」，跟随源时直接画实测帧率
                    let (fps_series, fps_max, fps_label): (Vec<f32>, f32, &str) =
                        if self.cfg.fps > 0 {
                            (
                                hist.iter()
                                    .map(|s| {
                                        s.streams
                                            .iter()
                                            .filter(|m| m.live && m.target_fps > 0.0)
                                            .map(|m| m.fps / m.target_fps * 100.0)
                                            .fold(100.0f32, f32::min)
                                    })
                                    .collect(),
                                100.0,
                                tr("帧率达成率 %（近 60 秒）"),
                            )
                        } else {
                            (
                                hist.iter()
                                    .map(|s| {
                                        s.streams
                                            .iter()
                                            .filter(|m| m.live)
                                            .map(|m| m.fps)
                                            .fold(0.0f32, f32::max)
                                    })
                                    .collect(),
                                30.0,
                                tr("帧率 fps（近 60 秒）"),
                            )
                        };
                    sparkline(ui, &fps_series, fps_max, OK, fps_label, p.muted);
                    let cpu: Vec<f32> = hist.iter().map(|s| s.app_cpu_pct).collect();
                    sparkline(ui, &cpu, 100.0, ACCENT, tr("软件 CPU 占用 %（近 60 秒）"), p.muted);

                    if let Some(a) = self.diag.evaluate(self.hw_upgradable()) {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(i18n::verdict(&a.text)).color(WARN));
                    }
                } else {
                    ui.label(egui::RichText::new(tr("刚开始采样，请稍候…")).color(p.muted));
                }

                ui.add_space(10.0);
                if ui.button(tr("复制诊断文本")).clicked() {
                    ui.output_mut(|o| {
                        o.commands
                            .push(egui::OutputCommand::CopyText(report.clone()))
                    });
                }
                ui.label(
                    egui::RichText::new(i18n::runtime_summary(
                        &target_label,
                        self.cfg.render_width,
                        self.cfg.render_height,
                        &if self.cfg.hwdec == "none" || self.cfg.hwdec.is_empty() {
                            tr("软件").to_string()
                        } else {
                            self.cfg.hwdec.clone()
                        },
                    ))
                    .size(11.0)
                    .color(p.muted),
                );
            });

        if !open {
            self.show_details = false;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TourAction {
    None,
    Next,
    Back,
    GotoScan,
    GotoManual,
    GotoDone,
    Finish,
    Exit,
}

/// 把「屏幕减去高亮矩形」拆成最多 4 条遮罩带
fn mask_strips(screen: egui::Rect, hole: Option<egui::Rect>) -> [egui::Rect; 4] {
    let Some(h) = hole else {
        return [screen, egui::Rect::NOTHING, egui::Rect::NOTHING, egui::Rect::NOTHING];
    };
    let h = h.intersect(screen);
    let top = egui::Rect::from_min_max(screen.min, egui::pos2(screen.max.x, h.min.y));
    let bottom = egui::Rect::from_min_max(egui::pos2(screen.min.x, h.max.y), screen.max);
    let left = egui::Rect::from_min_max(
        egui::pos2(screen.min.x, h.min.y),
        egui::pos2(h.min.x, h.max.y),
    );
    let right = egui::Rect::from_min_max(
        egui::pos2(h.max.x, h.min.y),
        egui::pos2(screen.max.x, h.max.y),
    );
    [top, bottom, left, right]
}

fn sparkline(
    ui: &mut egui::Ui,
    values: &[f32],
    max: f32,
    color: egui::Color32,
    label: &str,
    label_color: egui::Color32,
) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 48.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, egui::CornerRadius::ZERO, ui.visuals().extreme_bg_color);
    if values.len() >= 2 {
        let n = (values.len() - 1) as f32;
        let pts: Vec<egui::Pos2> = values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let x = rect.left() + rect.width() * (i as f32 / n);
                let y = rect.bottom() - rect.height() * (v / max).clamp(0.0, 1.0);
                egui::pos2(x, y)
            })
            .collect();
        painter.add(egui::Shape::line(pts, egui::Stroke::new(1.5, color)));
    }
    painter.text(
        rect.left_top() + egui::vec2(6.0, 4.0),
        egui::Align2::LEFT_TOP,
        label,
        egui::FontId::proportional(11.0),
        label_color,
    );


}

impl eframe::App for CamNexusApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // 让 ffmpeg 写完 moov 再退出，避免录像文件损坏
        self.records.shutdown();
        logs::append("", tr("退出"));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let p = self.pal();

        // 原生标题栏主题：窗口创建时尚未显示，此时下发的 SetTheme 只改了配色，
        // 非客户区没有重绘（表现为「浅底白字」的标题栏），要等窗口交互才刷新。
        // 启动后补发几次，强制 Windows 重新计算标题栏。
        self.frames = self.frames.wrapping_add(1);
        if matches!(self.frames, 5 | 15 | 30 | 60 | 120) {
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(if self.cfg.dark {
                egui::SystemTheme::Dark
            } else {
                egui::SystemTheme::Light
            }));
            ctx.request_repaint();
        }

        // 下载完成后刷新可用状态（下载线程不会直接碰界面状态）
        if !self.model_ready {
            let done = matches!(
                self.model_state.lock().map(|g| g.clone()),
                Ok(crate::model::State::Done)
            );
            if done {
                self.model_ready = crate::model::installed().is_some();
            }
        }

        let detect_err = if let Some(d) = self.detect.as_mut() {
            d.poll();
            let e = d.error.take();
            if e.is_some() {
                // 模型缺失/加载失败：别让按钮显示「开」却什么也不做
                d.enabled = false;
                d.results.clear();
            }
            e
        } else {
            None
        };
        if let Some(e) = detect_err {
            // 线程已经退出了，留着一个「假开」的运行时只会让开关按了没反应，
            // 直接丢掉，下次点击会重新拉起一个
            self.detect = None;
            self.toast(i18n::detect_unavailable(&e));
        }
        if let Some(ff) = self.ffmpeg.clone() {
            self.records.tick(&ff, &self.cfg, &self.cfg.channels);
        }
        self.poll_scan();
        // 裁剪黑边：每 2 秒检测一次，连续两次一致才生效
        for i in 0..self.streams.len() {
            let on = self
                .cfg
                .channels
                .get(i)
                .map(|c| c.crop_borders)
                .unwrap_or(false);
            if !on {
                self.crops.remove(&i);
                continue;
            }
            let frame = self
                .streams
                .get(i)
                .and_then(|s| s.as_ref())
                .and_then(|s| s.frame());

            let now = Instant::now();
            let w = self.crops.entry(i).or_default();
            if w.next.map(|t| now < t).unwrap_or(false) {
                continue;
            }
            w.next = Some(now + Duration::from_secs(2));

            let Some(f) = frame else { continue };
            let found = layout::content_box(&f.data, f.w, f.h);
            match (&w.pending, found) {
                (Some(p), Some(d)) if {
                    // 和上一次检测足够接近才算稳定
                    let diff = (0..4).map(|k| (p[k] - d[k]).abs()).fold(0.0f32, f32::max);
                    diff < 0.02
                } =>
                {
                    w.hits = w.hits.saturating_add(1);
                    if w.hits >= 2 && w.applied != Some(d) {
                        w.applied = Some(d);
                    }
                    w.pending = Some(d);
                }
                (_, None) => {
                    // 没有黑边（或整帧全黑）→ 取消裁剪
                    w.pending = None;
                    w.hits = 0;
                    w.applied = None;
                }
                (_, Some(d)) => {
                    w.pending = Some(d);
                    w.hits = 1;
                }
            }
        }

        // 命令行抓拍：等这一路有画面了再存，最多等 20 秒
        if let Some((idx, since)) = self.pending_snapshot {
            let ready = self
                .streams
                .get(idx)
                .and_then(|s| s.as_ref())
                .and_then(|s| s.frame())
                .is_some();
            if ready {
                self.pending_snapshot = None;
                match self.save_snapshot(idx) {
                    Ok(p) => {
                        let msg = i18n::snapshot_saved(&p.to_string_lossy());
                        logs::append("", &msg);
                        self.toast(msg);
                    }
                    Err(e) => self.toast(e),
                }
            } else if since.elapsed() > Duration::from_secs(20) {
                self.pending_snapshot = None;
                self.toast(tr("这一路还没有画面"));
            }
        }

        // 看门狗：画面上次更新太久就杀掉取流进程，让它重连。
        // 摄像机假死时 TCP 常常还连着，不主动检测会一直显示冻结的旧画面。
        for s in self.streams.iter().flatten() {
            s.tick();
        }

        self.update_textures(&ctx);
        self.sample_metrics();

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.solo = None;
            if self.fullscreen {
                self.set_fullscreen(&ctx, false);
            }
        }

        if self.ffmpeg.is_none() {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(p.bg).inner_margin(egui::Margin::same(24)))
                .show(ui, |ui| self.ffmpeg_setup(ui));
            self.settings_window(&ctx);
            return;
        }

        if self.fullscreen {
            // 全屏下顶栏不占布局：鼠标靠近顶部才从上面降下来，
            // 这样画面始终是满屏，顶栏出现时也不会把格子挤动
            let ptr_y = ctx.input(|i| i.pointer.hover_pos()).map(|p| p.y);
            if let Some(y) = ptr_y {
                if y <= 44.0 {
                    self.bar_revealed = true;
                } else if y > 76.0 {
                    // 留一段回差，避免在边界上闪烁
                    self.bar_revealed = false;
                }
            }
            if self.bar_revealed {
                egui::Area::new(egui::Id::unique("top_bar_overlay"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(egui::pos2(0.0, 0.0))
                    .show(&ctx, |ui| {
                        ui.set_width(ctx.content_rect().width());
                        egui::Frame::NONE
                            .fill(p.panel)
                            .inner_margin(egui::Margin::symmetric(14, 10))
                            .show(ui, |ui| {
                                ui.set_height(36.0);
                                ui.set_width(ctx.content_rect().width() - 28.0);
                                self.top_bar(ui);
                            });
                    });
            }
        } else {
            egui::Panel::top("top")
                .exact_size(56.0)
                .frame(
                    egui::Frame::NONE
                        .fill(p.panel)
                        .inner_margin(egui::Margin::symmetric(14, 10)),
                )
                .show(ui, |ui| self.top_bar(ui));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.bg).inner_margin(egui::Margin::same(8)))
            .show(ui, |ui| {
                // 提示条排在视频区之前：占一行高度，视频区自动缩小，
                // 不参与底部面板的空间协商（Panel 嵌套在此处会错位裁切）。
                self.hint_bar(ui);
                self.video_area(ui);
            });

        self.settings_window(&ctx);
        self.scan_window(&ctx);
        self.details_window(&ctx);
        self.toast_layer(ui);
        self.replay_window(&ctx);
        self.logs_window(&ctx);
        self.about_window(&ctx);
        self.welcome_window(&ctx);
        self.tour_overlay(&ctx);

        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

fn strip_creds(url: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let (scheme, rest) = url.split_at(scheme_end + 3);
        if let Some(at) = rest.find('@') {
            return format!("{scheme}{}", &rest[at + 1..]);
        }
    }
    url.to_string()
}

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        format!("{}…", chars[..max].iter().collect::<String>())
    }
}

/// 视频流相关参数的指纹：只有这些变了才需要重连。
/// 通道名称只是显示用，改名不触发重连。
fn stream_sig(c: &AppConfig) -> String {
    let mut s = format!(
        "{}|{}x{}|{}|{}|{}|",
        c.ffmpeg_path, c.render_width, c.render_height, c.fps, c.tcp, c.hwdec
    );
    for ch in &c.channels {
        s.push_str(&format!(
            "{}|{}|{}|{}|{};",
            ch.url, ch.sub_url, ch.username, ch.password, ch.enabled
        ));
    }
    s
}
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum SetTab {
    #[default]
    Channels,
    Recording,
    Detect,
    Video,
    General,
}

/// 标签页顺序与标题（标题走 i18n）
const SET_TABS: &[(SetTab, &str)] = &[
    (SetTab::Channels, "视频通道"),
    (SetTab::Recording, "录像"),
    (SetTab::Video, "画面"),
    (SetTab::General, "通用"),
    // 实验放最后：日常用不到，别占着靠前的位置
    (SetTab::Detect, "实验"),
];

/// 说明文案统一收进小问号：平时只占 14×14，鼠标悬停才展开
fn help(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    let color = ui.visuals().weak_text_color();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let c = rect.center();
        ui.painter()
            .circle_stroke(c, 6.5, egui::Stroke::new(1.0, color));
        ui.painter().text(
            c,
            egui::Align2::CENTER_CENTER,
            "?",
            egui::FontId::proportional(9.5),
            color,
        );
    }
    resp.on_hover_text(text).on_hover_cursor(egui::CursorIcon::Help)
}


/// 窗口标题跟着语言走（原生标题栏不会自动重绘，得主动发命令）
fn apply_window_title(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Title(
        i18n::app_title().to_string(),
    ));
}

fn apply_style(ctx: &egui::Context, dark: bool) {
    ctx.set_theme(if dark {
        egui::ThemePreference::Dark
    } else {
        egui::ThemePreference::Light
    });
    // 两套主题各自配置完整样式：即便系统主题或偏好被外部改动，
    // 落在哪一套上都是对的，不会出现「浅色控件 + 深色外壳」的错配。
    ctx.style_mut_of(egui::Theme::Dark, |style| customize_style(style, true));
    ctx.style_mut_of(egui::Theme::Light, |style| customize_style(style, false));
}

fn customize_style(style: &mut egui::Style, dark: bool) {
    let p = palette(dark);
    style.visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    style.visuals.panel_fill = p.panel;
    style.visuals.window_fill = p.window;
    style.visuals.faint_bg_color = if dark {
        egui::Color32::from_rgb(26, 32, 40)
    } else {
        egui::Color32::from_rgb(238, 242, 247)
    };
    style.visuals.extreme_bg_color = if dark {
        egui::Color32::from_rgb(10, 13, 18)
    } else {
        egui::Color32::from_rgb(255, 255, 255)
    };
    style.visuals.override_text_color = if dark { Some(p.text) } else { None };
    style.visuals.widgets.active.bg_fill = ACCENT;
    style.visuals.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
}

#[cfg(test)]
mod sig_tests {
    use super::*;
    use crate::config::Channel;

    /// 问号必须有真正的可悬停区域——否则说明文案就永远弹不出来。
    /// 本机注入鼠标无效，只能靠无头 Context 模拟指针位置来验。
    #[test]
    fn help_mark_is_hoverable() {
        let ctx = egui::Context::default();
        let base = || {
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            ));
            input
        };
        // 固定位置，避免 Area 在两帧之间漂移导致指针落空
        let area = || {
            egui::Area::new(egui::Id::new("probe"))
                .fixed_pos(egui::pos2(40.0, 40.0))
                .order(egui::Order::Foreground)
        };

        let mut rect = egui::Rect::NOTHING;
        ctx.begin_pass(base());
        area().show(&ctx, |ui| {
            rect = help(ui, "说明文字").rect;
        });
        ctx.end_pass().textures_delta.clear();
        assert!(rect.width() > 0.0, "问号应有实际尺寸");

        // egui 的命中测试用的是上一帧的布局，所以要连续喂几帧指针位置
        let mut hovered = false;
        for _ in 0..3 {
            let mut input = base();
            input
                .events
                .push(egui::Event::PointerMoved(rect.center()));
            ctx.begin_pass(input);
            area().show(&ctx, |ui| {
                hovered = help(ui, "说明文字").hovered();
            });
            ctx.end_pass().textures_delta.clear();
        }
        assert!(hovered, "鼠标落在问号上时应判定为悬停");
    }

    fn ch(name: &str, url: &str) -> Channel {
        Channel {
            name: name.into(),
            url: url.into(),
            username: "admin".into(),
            password: "pw".into(),
            enabled: true,
            record: Default::default(),
            rotate: 0,
            flip_h: false,
            flip_v: false,
            aspect_lock: true,
            crop_borders: false,
            sub_url: String::new(),
        }
    }

    /// 子码流只影响监看：改它需要重连，但不该影响录制用的主码流地址
    #[test]
    fn substream_changes_require_reconnect() {
        let mut a = AppConfig::default();
        a.channels = vec![ch(tr("镜头1"), "rtsp://1.2.3.4/Streaming/Channels/101")];
        assert!(!stream_sig(&a).is_empty());

        let before = stream_sig(&a);
        let mut b = a.clone();
        b.channels[0].sub_url = "rtsp://1.2.3.4/Streaming/Channels/102".into();
        assert_ne!(before, stream_sig(&b), "改子码流应触发重连提示");
    }

    /// 录制必须永远拿主码流地址，不能被监看用的子码流污染
    #[test]
    fn recording_url_stays_on_main_stream() {
        let mut c = ch(tr("镜头1"), "rtsp://1.2.3.4/Streaming/Channels/101");
        c.sub_url = "rtsp://1.2.3.4/Streaming/Channels/102".into();
        assert!(c.rtsp().ends_with("/Streaming/Channels/101"));
        assert!(c.display_rtsp().ends_with("/Streaming/Channels/102"));
    }

    /// 常见子码流地址推导：海康 101→102、201→202，大华 subtype
    #[test]
    fn derive_substream_url() {
        use crate::config::derive_sub_url;
        assert_eq!(
            derive_sub_url("rtsp://192.168.1.10:554/Streaming/Channels/101").as_deref(),
            Some("rtsp://192.168.1.10:554/Streaming/Channels/102")
        );
        assert_eq!(
            derive_sub_url("rtsp://192.168.1.10:554/Streaming/Channels/201").as_deref(),
            Some("rtsp://192.168.1.10:554/Streaming/Channels/202")
        );
        assert_eq!(
            derive_sub_url("rtsp://1.2.3.4/cam/realmonitor?channel=1&subtype=0").as_deref(),
            Some("rtsp://1.2.3.4/cam/realmonitor?channel=1&subtype=1")
        );
        // 认不出来就不猜
        assert_eq!(derive_sub_url("rtsp://1.2.3.4/live"), None);
        assert_eq!(derive_sub_url("rtsp://1.2.3.4/Streaming/Channels/102"), None);
    }

    #[test]
    fn renaming_channel_does_not_require_reconnect() {
        let mut a = AppConfig::default();
        a.channels = vec![ch(tr("镜头1"), "rtsp://1.2.3.4/101")];
        let mut b = a.clone();
        b.channels[0].name = tr("大门").into();
        assert_eq!(stream_sig(&a), stream_sig(&b), "改通道名不应触发重连");
    }

    #[test]
    fn changing_url_or_decode_requires_reconnect() {
        let mut a = AppConfig::default();
        a.channels = vec![ch(tr("镜头1"), "rtsp://1.2.3.4/101")];

        let mut b = a.clone();
        b.channels[0].url = "rtsp://1.2.3.4/102".into();
        assert_ne!(stream_sig(&a), stream_sig(&b), "改 URL 必须重连");

        let mut c = a.clone();
        c.hwdec = "cuda".into();
        assert_ne!(stream_sig(&a), stream_sig(&c), "改解码方式必须重连");

        let mut d = a.clone();
        d.render_width += 320;
        assert_ne!(stream_sig(&a), stream_sig(&d), "改分辨率必须重连");
    }

    #[test]
    fn theme_and_volume_do_not_require_reconnect() {
        let a = AppConfig::default();
        let mut b = a.clone();
        b.dark = !a.dark;
        b.volume = 2.5;
        b.show_hints = !a.show_hints;
        assert_eq!(stream_sig(&a), stream_sig(&b), "主题/音量/提示不应触发重连");
    }
}

// ---------------------------------------------------------------- 目标检测运行时
/// 检测任务：通道下标 + 帧（Arc 共享，零拷贝）+ 尺寸
type DetectJob = (usize, std::sync::Arc<Vec<u8>>, usize, usize);

pub struct DetectRuntime {
    pub enabled: bool,
    pub cfg: crate::config::DetectCfg,
    pub status: String,
    /// 模型加载失败的说明，界面取走后清空（用于弹一次提示）
    pub error: Option<String>,
    tx: std::sync::mpsc::SyncSender<DetectJob>,
    rx: std::sync::mpsc::Receiver<(usize, Vec<Detection>)>,
    /// 后台线程回报的错误（空串 = 加载成功）
    st_rx: std::sync::mpsc::Receiver<String>,
    /// 后台线程随时更新的状态文案
    status_slot: std::sync::Arc<std::sync::Mutex<String>>,
    last_sent: std::collections::HashMap<usize, Instant>,
    /// 每路的检测结果与时间戳
    pub results: std::collections::HashMap<usize, (Vec<Detection>, Instant)>,
}

impl DetectRuntime {
    /// 起一个后台线程加载模型并常驻推理。
    /// 返回值里的 `status` 会先显示"加载中"，加载结果通过 channel 回传。
    pub fn spawn(ctx: egui::Context, cfg: crate::config::DetectCfg) -> Self {
        // 有界队列：一帧是整张原图（2560×1440 ≈ 11MB），多路高帧率下如果
        // 推理跟不上，无界 channel 会把内存吃掉；满了就丢帧，绝不阻塞 UI。
        let (tx, job_rx) = std::sync::mpsc::sync_channel::<DetectJob>(2);
        let (res_tx, rx) = std::sync::mpsc::channel::<(usize, Vec<Detection>)>();
        let (st_tx, st_rx) = std::sync::mpsc::channel::<String>();
        let status_slot = std::sync::Arc::new(std::sync::Mutex::new(String::from(tr("正在加载模型…"))));
        let slot = status_slot.clone();

        std::thread::Builder::new()
            .name("detect".into())
            .spawn(move || {
                // yolox_s(640) 优先，其次 yolox_tiny(416)
                let (path, size) = match crate::detect::find_model("yolox_s.onnx") {
                    Some(p) => (p, 640usize),
                    None => match crate::detect::find_model("yolox_tiny.onnx") {
                        Some(p) => (p, 416),
                        None => {
                            if let Ok(mut g) = slot.lock() {
                                *g = tr("未找到模型文件（models/yolox_s.onnx）").to_string();
                            }
                            let _ = st_tx.send(tr("未找到模型文件").to_string());
                            ctx.request_repaint();
                            return;
                        }
                    },
                };
                let mut det = match crate::detect::Detector::load_with(
                    &path,
                    size,
                    cfg.device == crate::config::DetectDevice::Gpu,
                ) {
                    Ok(d) => d,
                    Err(e) => {
                        if let Ok(mut g) = slot.lock() {
                            *g = e.clone();
                        }
                        let _ = st_tx.send(e);
                        ctx.request_repaint();
                        return;
                    }
                };
                det.set_night_opt(cfg.night_opt);
                if let Ok(mut g) = slot.lock() {
                    *g = i18n::model_ready(
                        &path.file_name().unwrap_or_default().to_string_lossy(),
                        match cfg.device {
                            crate::config::DetectDevice::Gpu => i18n::gpu_suffix(),
                            crate::config::DetectDevice::Cpu => i18n::cpu_suffix(),
                        },
                    );
                }
                let _ = st_tx.send(String::new());

                // 每路一份跟踪器：共用会让 A 的框漂到 B 的画面上
                let mut trackers = crate::detect::Trackers::new(&cfg);
                while let Ok((idx, data, w, h)) = job_rx.recv() {
                    if let Ok(raw) = det.detect(&data, w, h, 0.35) {
                        // 静止豁免在这里生效：长时间不动的目标不再输出
                        let shown = trackers.update(idx, &raw, std::time::Instant::now());
                        let _ = res_tx.send((idx, shown));
                        ctx.request_repaint();
                    }
                }
            })
            .ok();

        Self {
            enabled: true,
            cfg,
            status: String::from(tr("正在加载模型…")),
            error: None,
            tx,
            rx,
            st_rx,
            status_slot,
            last_sent: std::collections::HashMap::new(),
            results: std::collections::HashMap::new(),
        }
    }

    /// 每帧收结果 + 刷新状态
    pub fn poll(&mut self) {
        while let Ok((idx, dets)) = self.rx.try_recv() {
            self.results.insert(idx, (dets, Instant::now()));
        }
        // 加载是异步的：状态文案每帧同步一次，否则会永远停在「正在加载模型…」
        if let Ok(g) = self.status_slot.lock()
            && !g.is_empty() && *g != self.status {
                self.status = g.clone();
            }
        while let Ok(msg) = self.st_rx.try_recv() {
            if !msg.is_empty() && self.error.is_none() {
                self.error = Some(msg);
            }
        }
    }

    /// 到点就送一帧去检测（频率由设置决定：CPU 1 fps，显卡 1–5 fps）
    pub fn maybe_submit(&mut self, idx: usize, data: std::sync::Arc<Vec<u8>>, w: usize, h: usize) {
        if !self.enabled || w == 0 || h == 0 {
            return;
        }
        let interval = std::time::Duration::from_millis(1000 / self.cfg.effective_fps().max(1) as u64);
        let due = self
            .last_sent
            .get(&idx)
            .map(|t| t.elapsed() >= interval)
            .unwrap_or(true);
        if !due {
            return;
        }
        // try_send：队列满（推理跟不上）就跳过这一帧，不排队也不阻塞界面
        match self.tx.try_send((idx, data, w, h)) {
            Ok(()) => {
                self.last_sent.insert(idx, Instant::now());
            }
            Err(std::sync::mpsc::TrySendError::Full(_)) => {}
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                // 推理线程已退出（模型加载失败等），别再空转
                self.enabled = false;
            }
        }
    }

    /// 某一路当前有效的检测结果（3 秒内）
    pub fn active(&self, idx: usize) -> Option<&Vec<Detection>> {
        self.results
            .get(&idx)
            .filter(|(_, at)| at.elapsed() < std::time::Duration::from_secs(3))
            .map(|(v, _)| v)
    }
}
