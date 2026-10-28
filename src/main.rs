#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod config;
mod detect;
mod diag;
mod discover;
mod fetch;
mod fonts;
mod i18n;
mod icon;
mod layout;
mod logs;
mod model;
mod record;
mod stream;
mod sys;
mod tour;
mod ui;

use eframe::egui;

fn main() -> eframe::Result<()> {
    // 控制台诊断模式：camnexus --probe rtsp://...
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--probe") {
        if let Some(url) = args.get(pos + 1) {
            let r = discover::probe_url(url);
            println!("地址: {}", r.url);
            println!("结果: {}", if r.ok { "可用" } else { "不可用" });
            println!("详情: {}", r.detail);
        } else {
            eprintln!("用法: camnexus --probe rtsp://user:pass@ip:554/path");
        }
        return Ok(());
    }

    fetch::migrate_legacy();
    let cfg = config::AppConfig::load();
    // 语言必须在任何界面文案之前确定：i18n 的 tr() 全靠这个全局状态
    i18n::set_lang(cfg.lang);
    let open_diag = args.iter().any(|a| a == "--diag");
    // --detect-test <图片>：用 ffmpeg 解码后跑一次检测，打印结果（自检用）
    if let Some(i) = args.iter().position(|a| a == "--detect-test")
        && let Some(img) = args.get(i + 1) {
            let gpu = args.iter().any(|a| a == "--gpu");
            let night = args.iter().any(|a| a == "--night");
            detect_test(img, gpu, night);
            return Ok(());
        }

    // --settings-tab N：直接打开设置并切到第 N 个标签页（0 起），方便截图核对
    let settings_tab = args
        .iter()
        .position(|a| a == "--settings-tab")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok());
    let open_settings = args.iter().any(|a| a == "--settings") || settings_tab.is_some();
    // --fullscreen：启动即进入无边框全屏（自检/截图用）
    let start_fullscreen = args.iter().any(|a| a == "--fullscreen");
    // --snapshot N：等这一路出画面后抓一张（自检用）
    let snapshot_idx = args
        .iter()
        .position(|a| a == "--snapshot")
        .and_then(|i| args.get(i + 1));
    let fetch_ffmpeg = args.iter().any(|a| a == "--fetch-ffmpeg");
    let open_scan = args.iter().any(|a| a == "--scan");
    let start_tour = args.iter().any(|a| a == "--tour");
    let auto_detect = args.iter().any(|a| a == "--detect");
    let record_idx = args
        .iter()
        .position(|a| a == "--record")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok());
    let tour_step = args
        .iter()
        .position(|a| a == "--tour-step")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok());
    let audio_start = args
        .iter()
        .position(|a| a == "--audio")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok());
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 780.0])
        .with_min_inner_size([880.0, 540.0])
        .with_title(i18n::app_title());
    // 窗口 / 任务栏图标：不设的话 winit 会用自己的默认图标，和 exe 资源里的不一致
    if let Some(icon) = icon::app_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "CamNexus",
        options,
        Box::new(move |cc| {
            fonts::install(&cc.egui_ctx);
            let mut app = ui::CamNexusApp::new(cc, cfg);
            if open_diag {
                app.open_details();
            }
            if open_settings {
                app.open_settings();
                if let Some(n) = settings_tab {
                    app.set_settings_tab(n);
                }
            }
            if fetch_ffmpeg {
                app.start_fetch();
            }
            if open_scan {
                app.open_scan_window();
            }
            if auto_detect {
                // --detect 是自检参数：临时把实验开关打开（不写盘），
                // 这样主界面按钮和正常流程保持一致
                app.cfg_mut().detect.enabled = true;
                app.enable_detect(&cc.egui_ctx);
            }
            if start_fullscreen {
                app.enter_fullscreen(&cc.egui_ctx);
            }
            if let Some(n) = snapshot_idx.and_then(|s| s.parse::<usize>().ok()) {
                app.snapshot_now(n.saturating_sub(1));
            }
            if let Some(n) = record_idx {
                app.start_record(n.saturating_sub(1));
            }
            if let Some(n) = tour_step {
                app.start_tour_at(n.saturating_sub(1));
            } else if start_tour {
                app.start_tour(false);
            }
            if let Some(n) = audio_start {
                app.start_audio_index(n.saturating_sub(1));
            }
            Ok(Box::new(app))
        }),
    )
}

/// 从文件头读尺寸（支持 JPEG / PNG），比解析 ffmpeg 输出可靠
fn image_size(buf: &[u8]) -> Option<(usize, usize)> {
    // PNG：文件头 8 字节签名 + IHDR 块

    if buf.len() > 24 && buf.starts_with(&[0x89, b'P', b'N', b'G']) {
        let w = u32::from_be_bytes([buf[16], buf[17], buf[18], buf[19]]) as usize;
        let h = u32::from_be_bytes([buf[20], buf[21], buf[22], buf[23]]) as usize;
        return Some((w, h));
    }
    // JPEG: 找 SOF0..SOF3 标记
    if buf.len() > 4 && buf[0] == 0xFF && buf[1] == 0xD8 {
        let mut i = 2;
        while i + 9 < buf.len() {
            if buf[i] != 0xFF {
                i += 1;
                continue;
            }
            let m = buf[i + 1];
            if (0xC0..=0xC3).contains(&m) {
                let h = u16::from_be_bytes([buf[i + 5], buf[i + 6]]) as usize;
                let w = u16::from_be_bytes([buf[i + 7], buf[i + 8]]) as usize;
                return Some((w, h));
            }
            if m == 0xD8 || m == 0xD9 || (0xD0..=0xD7).contains(&m) {
                i += 2;
                continue;
            }
            let len = u16::from_be_bytes([buf[i + 2], buf[i + 3]]) as usize;
            if len < 2 {
                break;
            }
            i += 2 + len;
        }
    }
    None
}

/// 自检：图片 → RGB24 → 检测 → 打印，用于验证解码/后处理是否正确
fn detect_test(image: &str, gpu: bool, night: bool) {
    let Ok(bytes) = std::fs::read(image) else {
        eprintln!("读不到图片：{image}");
        return;
    };
    let Some((w, h)) = image_size(&bytes) else {
        eprintln!("无法识别图片格式（只支持 JPEG/PNG）");
        return;
    };
    let Some(ff) = config::find_ffmpeg("") else {
        eprintln!("未找到 ffmpeg");
        return;
    };
    let Ok(out) = std::process::Command::new(ff)
        .args(["-hide_banner", "-loglevel", "error", "-i", image])
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
    else {
        eprintln!("ffmpeg 解码失败");
        return;
    };
    if out.stdout.len() < w * h * 3 {
        eprintln!("解码数据不足：{} < {}", out.stdout.len(), w * h * 3);
        return;
    }

    let (path, size) = match detect::find_model("yolox_s.onnx") {
        Some(p) => (p, 640usize),
        None => match detect::find_model("yolox_tiny.onnx") {
            Some(p) => (p, 416usize),
            None => {
                eprintln!("未找到模型文件（应放在 %LOCALAPPDATA%\\CamNexus\\models）");
                return;
            }
        },
    };
    let t0 = std::time::Instant::now();
    match detect::Detector::load_with(&path, size, gpu) {
        Ok(mut d) => {
            let load_ms = t0.elapsed().as_millis();
            d.set_night_opt(night);
            let t1 = std::time::Instant::now();
            match d.detect(&out.stdout, w, h, 0.35) {
                Ok(v) => {
                    println!(
                        "模型 {}（{}×{}，{}）  加载 {} ms  推理 {} ms  画面 {}x{}",
                        path.file_name().unwrap_or_default().to_string_lossy(),
                        size,
                        size,
                        match (gpu, night) {
                            (false, false) => "CPU",
                            (false, true) => "CPU + 夜视优化",
                            (true, false) => "显卡 DirectML",
                            (true, true) => "显卡 DirectML + 夜视优化",
                        },
                        load_ms,
                        t1.elapsed().as_millis(),
                        w,
                        h
                    );
                    if v.is_empty() {
                        println!("未检测到人体/车辆");
                    }
                    for det in v {
                        println!(
                            "  {:<6} {:3.0}%  位置 {:.2},{:.2}  尺寸 {:.2}x{:.2}",
                            det.label,
                            det.score * 100.0,
                            det.x,
                            det.y,
                            det.w,
                            det.h
                        );
                    }
                }
                Err(e) => eprintln!("推理失败：{e}"),
            }
        }
        Err(e) => eprintln!("加载失败：{e}"),
    }
}
