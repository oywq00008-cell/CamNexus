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

    // --guard 必须在最前面：守护进程不加载配置、不抢单实例锁，
    // 否则它自己写下的 PID 会让它误判"已有实例"而立刻退出。
    if args.iter().any(|a| a == "--guard") {
        guard_loop();
    }
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

    // --detect-test <图片>：用 ffmpeg 解码后跑一次检测，打印结果（自检用）
    // --net-test：打印各网卡 2 秒内的收发速率，用于核对数据栏的读数
    if args.iter().any(|a| a == "--net-test") {
        net_test();
        return Ok(());
    }

    // --audio-test N：验证第 N 路（1 起）的音频是否随显示流一起到达
    if let Some(i) = args.iter().position(|a| a == "--audio-test")
        && let Some(n) = args.get(i + 1).and_then(|s| s.parse::<usize>().ok())
    {
        audio_test(n);
        return Ok(());
    }

    if let Some(i) = args.iter().position(|a| a == "--detect-test")
        && let Some(img) = args.get(i + 1) {
            let gpu = args.iter().any(|a| a == "--gpu");
            let night = args.iter().any(|a| a == "--night");
            detect_test(img, gpu, night);
            return Ok(());
        }

    // 单实例：开了开机自启之后，登录时可能已有实例在跑，
    // 用户再双击图标就会起第二份——两套取流直接顶满相机连接数。
    if !single_instance() {
        return Ok(());
    }

    // 自启项自愈：程序被移动过、或注册表被清理过，这里补回来
    if cfg.autostart
        && let Ok(exe) = std::env::current_exe()
    {
        let want = sys::autostart_command(&exe, cfg.guard);
        if sys::autostart_read().as_deref() != Some(want.as_str()) {
            let _ = sys::set_autostart(&exe, cfg.guard, true);
        }
    }
    let open_diag = args.iter().any(|a| a == "--diag");
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
    let result = eframe::run_native(
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
    );

    // 正常关闭窗口：给守护留个"别再拉我"的标记
    if result.is_ok() {
        let _ = std::fs::write(clean_exit_marker(), b"ok");
    }
    result
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




/// 单实例锁：把 PID 写进数据目录，启动前检查那个进程还在不在。
///
/// 比命名互斥体好排查：出问题直接看文件就知道上次是谁占着。
/// 进程被杀后 PID 文件会残留，所以必须校验进程是否真的还活着。
#[cfg(windows)]
fn instance_pid_path() -> std::path::PathBuf {
    config::data_dir().join("instance.pid")
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW,
        PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let mut code: u32 = 0;
        let ok = GetExitCodeProcess(h, &mut code);
        if ok == 0 || code != STILL_ACTIVE as u32 {
            CloseHandle(h);
            return false;
        }
        // 系统会把 PID 复用给别的程序。只看到"活着"不够，
        // 还要确认它确实是 CamNexus，否则会出现"点了没反应"的怪现象。
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let named = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if named == 0 {
            return false;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.to_ascii_lowercase().ends_with("camnexus.exe")
    }
}

/// 有没有 CamNexus 正在运行
#[cfg(windows)]
fn instance_running() -> bool {
    let Ok(s) = std::fs::read_to_string(instance_pid_path()) else {
        return false;
    };
    let Ok(pid) = s.trim().parse::<u32>() else {
        return false;
    };
    pid != 0 && process_alive(pid)
}

#[cfg(not(windows))]
fn instance_running() -> bool {
    false
}

/// 抢占单实例锁。false 表示已有实例在跑。
#[cfg(windows)]
fn single_instance() -> bool {
    if instance_running() {
        return false;
    }
    let _ = std::fs::create_dir_all(config::data_dir());
    let _ = std::fs::write(instance_pid_path(), std::process::id().to_string());
    true
}

#[cfg(not(windows))]
fn single_instance() -> bool {
    true
}

/// 正常退出的标记文件。主程序正常关闭窗口时写它，守护据此判断是"用户关的"
/// 还是"崩溃了"——否则每次关窗口都会被守护重新拉起来。
pub fn clean_exit_marker() -> std::path::PathBuf {
    config::data_dir().join("clean_exit")
}

/// 守护模式：主程序退出后决定是否重启。
fn guard_loop() -> ! {
    use std::time::{Duration, Instant};

    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("camnexus.exe"));
    let marker = clean_exit_marker();
    let mut fast_exits = 0u32;

    loop {
        // 已有实例在跑就别插手，否则每 3 秒拉一个抢不到锁的进程
        if instance_running() {
            std::process::exit(0);
        }
        let _ = std::fs::remove_file(&marker);
        let started = Instant::now();
        let status = std::process::Command::new(&exe)
            .env("CAMNEXUS_GUARDED", "1")
            .spawn()
            .and_then(|mut c| c.wait());

        // 正常退出：守护也退出
        if marker.exists() {
            let _ = std::fs::remove_file(&marker);
            std::process::exit(0);
        }

        let ran = started.elapsed();
        if ran < Duration::from_secs(15) {
            // 秒退：多半是启动即崩，别在几秒内刷几十次
            fast_exits += 1;
        } else {
            fast_exits = 0;
        }

        let wait = if fast_exits >= 5 {
            fast_exits = 0;
            Duration::from_secs(60)
        } else {
            Duration::from_secs(3)
        };
        if let Err(e) = status {
            eprintln!("守护：拉起主程序失败 {e}");
        }
        std::thread::sleep(wait);
    }
}

/// 自检：把每个网络接口的实际速率打出来，用来核对数据栏读数。
fn net_test() {
    use std::time::Duration;

    let a = sys::net_ifaces();
    std::thread::sleep(Duration::from_secs(2));
    let b = sys::net_ifaces();

    println!("{:<34} {:>12} {:>12}  计入", "网卡", "下行", "上行");
    let mut tot_dn = 0.0;
    let mut tot_up = 0.0;
    for (name, in0, out0, counted) in &a {
        let Some((_, in1, out1, _)) = b.iter().find(|(n, _, _, _)| n == name) else {
            continue;
        };
        let dn = in1.saturating_sub(*in0) as f64 / 2.0;
        let up = out1.saturating_sub(*out0) as f64 / 2.0;
        if *counted {
            tot_dn += dn;
            tot_up += up;
        }
        println!(
            "{:<34} {:>12} {:>12}  {}",
            name,
            sys::human_rate(dn),
            sys::human_rate(up),
            if *counted { "是" } else { "否" }
        );
    }
    println!();
    println!("统计合计：下行 {}  上行 {}", sys::human_rate(tot_dn), sys::human_rate(tot_up));

}

/// 自检：起一路流，观察 8 秒，报告画面与音频是否都到达。
fn audio_test(idx1: usize) {
    use std::time::{Duration, Instant};

    let cfg = config::AppConfig::load();
    let Some(ch) = cfg.channels.get(idx1.saturating_sub(1)) else {
        println!("没有第 {idx1} 路通道");
        return;
    };
    let Some(ff) = config::find_ffmpeg(&cfg.ffmpeg_path) else {
        println!("未找到 ffmpeg");
        return;
    };

    println!("通道: {}  {}", ch.name, ch.display_rtsp());
    println!("探测 SDP 音轨…");
    let has_audio = discover::stream_has_audio(&ch.display_rtsp());
    println!("  SDP 含 m=audio: {has_audio:?}");

    let stream = stream::Stream::spawn(stream::StreamCfg {
        url: ch.display_rtsp(),
        ffmpeg: ff,
        width: cfg.render_width,
        height: cfg.render_height,
        fps: cfg.fps,
        tcp: cfg.tcp,
        hwdec: cfg.hwdec.clone(),
        audio: true,
        start_delay: std::time::Duration::ZERO,
    });

    let started = Instant::now();
    let mut printed_frame = false;
    while started.elapsed() < Duration::from_secs(8) {
        std::thread::sleep(Duration::from_millis(250));
        if !printed_frame
            && let Some(f) = stream.frame()
        {
            println!("首帧到达: {}x{}", f.w, f.h);
            printed_frame = true;
        }
    }

    let st = stream.status();
    let buf = stream.audio();
    println!("--- 结果 ---");
    println!("画面状态: {:?}  帧数 {}  帧率 {:.1}", st.state, st.frames, st.fps);
    println!("音轨可用: {}", stream.audio_available());
    println!("收到音频数据: {}", buf.active());
    println!("音量电平: {:.3}", buf.level());
    println!("连接次数: {}", st.reconnects);
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
