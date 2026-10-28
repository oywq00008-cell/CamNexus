//! 音频播放：独立 ffmpeg 进程抽音频 → rodio 播放
//!
//! 设计取舍：
//! - 音频不复用视频那条管道（std 只能给子进程一路 stdout），单独起一个轻量进程；
//! - 只在用户点开某一路声音时才启动，避免常驻多路 RTSP 连接；
//! - 同一时刻只播一路：两间屋子的声音混在一起没有意义；
//! - cpal 的音频流不是 Send，播放器必须在自己的线程里创建和销毁。

use crate::i18n::tr;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 采样率统一到 48 kHz，避免声卡不支持 8000 Hz
const SAMPLE_RATE: u32 = 48_000;
/// 每次读取 200 ms
const CHUNK_SAMPLES: usize = (SAMPLE_RATE as usize) / 5;

pub struct AudioCfg {
    pub url: String,
    pub ffmpeg: PathBuf,
    pub tcp: bool,
    /// f32 音量（以 bit 形式共享），允许 >1 做增益
    pub volume: Arc<AtomicU32>,
}

pub struct AudioStream {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// 是否已经成功取到过音频数据
    pub active: Arc<AtomicBool>,
    /// 最近一块音频的 RMS（f32 的 bit 形式），用于界面电平显示
    pub level: Arc<AtomicU32>,
}

impl AudioStream {
    pub fn start(cfg: AudioCfg) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let active = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0f32.to_bits()));

        let (sp, ch, ac, lv) = (stop.clone(), child.clone(), active.clone(), level.clone());
        let thread = std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || run(cfg, sp, ch, ac, lv))
            .ok();

        Self {
            stop,
            child,
            thread,
            active,
            level,
        }
    }

    pub fn playing(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut g) = self.child.lock()
            && let Some(c) = g.as_mut() {
                let _ = c.kill();
            }
    }
}

impl Drop for AudioStream {
    fn drop(&mut self) {
        self.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn build_command(cfg: &AudioCfg) -> Command {
    let mut cmd = Command::new(&cfg.ffmpeg);
    cmd.arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-rtsp_transport")
        .arg(if cfg.tcp { "tcp" } else { "udp" })
        // 音频没有关键帧概念，nobuffer 不会产生画面绿屏那类问题，
        // 只用于压低出声延迟（显示链路里已禁用，见 stream.rs 的说明）。
        .arg("-fflags")
        .arg("nobuffer")
        .arg("-flags")
        .arg("low_delay")
        .arg("-timeout")
        .arg("5000000")
        .arg("-i")
        .arg(&cfg.url)
        .arg("-vn")
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg(SAMPLE_RATE.to_string())
        .arg("-f")
        .arg("s16le")
        .arg("-");

    cmd.stdout(Stdio::piped()).stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    cmd
}

fn run(
    cfg: AudioCfg,
    stop: Arc<AtomicBool>,
    child_slot: Arc<Mutex<Option<Child>>>,
    active: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
) {
    // 播放设备只在本线程创建，cpal 流不能跨线程
    let sink = match rodio::DeviceSinkBuilder::open_default_sink() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("音频设备不可用：{e}");
            return;
        }
    };
    let player = rodio::Player::connect_new(sink.mixer());
    let channels = std::num::NonZeroU16::new(1).unwrap();
    let rate = std::num::NonZeroU32::new(SAMPLE_RATE).unwrap();

    let mut backoff: u64 = 1;
    while !stop.load(Ordering::SeqCst) {
        let spawned = build_command(&cfg).spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(_) => {
                sleep_stop(Duration::from_secs(backoff), &stop);
                backoff = (backoff * 2).min(8);
                continue;
            }
        };
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            continue;
        };
        if let Ok(mut slot) = child_slot.lock() {
            *slot = Some(child);
        }

        let mut buf = vec![0u8; CHUNK_SAMPLES * 2];
        loop {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            if read_full(&mut stdout, &mut buf).is_err() {
                break;
            }

            let vol = f32::from_bits(cfg.volume.load(Ordering::Relaxed));
            player.set_volume(vol.clamp(0.0, 4.0));

            let samples: Vec<f32> = buf
                .as_chunks::<2>().0.iter()
                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
                .collect();

            // 电平：RMS，供界面判断麦克风是否静音
            let rms = if samples.is_empty() {
                0.0
            } else {
                (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
            };
            level.store(rms.to_bits(), Ordering::Relaxed);

            player.append(rodio::buffer::SamplesBuffer::new(channels, rate, samples));
            active.store(true, Ordering::SeqCst);

            // 队列超过约 2 秒就清空，防止延迟越积越多
            if player.len() > 10 {
                player.clear();
            }
        }

        if let Ok(mut slot) = child_slot.lock() {
            if let Some(c) = slot.as_mut() {
                let _ = c.kill();
                let _ = c.wait();
            }
            *slot = None;
        }
        active.store(false, Ordering::SeqCst);

        if stop.load(Ordering::SeqCst) {
            break;
        }
        if !sleep_stop(Duration::from_secs(backoff), &stop) {
            break;
        }
        backoff = (backoff * 2).min(8);
    }

    player.stop();
    let _ = sink;
}

fn read_full<R: Read>(r: &mut R, buf: &mut [u8]) -> std::io::Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    tr("音频流结束"),
                ));
            }
            Ok(n) => filled += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn sleep_stop(dur: Duration, stop: &AtomicBool) -> bool {
    let step = Duration::from_millis(100);
    let mut left = dur;
    while left > Duration::ZERO {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        let s = left.min(step);
        std::thread::sleep(s);
        left -= s;
    }
    !stop.load(Ordering::SeqCst)
}
