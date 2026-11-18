//! 音频：由**显示流**顺带产出，本模块只负责「缓冲 + 播放」
//!
//! 为什么不再单独起 ffmpeg：
//! 摄像机的并发会话是有限的（实测该型号 6 路），显示、录像、音频各占一路时，
//! 再算上手机 App 就会把额度打满。音轨本来就在显示那条会话里，
//! 单独再连一次纯属浪费——现在由显示进程把音频推到本地回环端口，
//! 一路会话同时供画面和声音使用。
//!
//! 结构：
//! - `AudioBuf`：生产者（stream.rs 的读取线程）写、消费者（播放线程）读的环形缓冲
//! - `AudioPlayer`：按需启动的播放线程；开关声音不再重连 RTSP

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 采样率统一到 48 kHz，避免声卡不支持 8000 Hz
pub const SAMPLE_RATE: u32 = 48_000;

/// 缓冲上限：2 秒。超过就丢最旧的，保证延迟不会越滚越大。
const MAX_SAMPLES: usize = (SAMPLE_RATE as usize) * 2;

/// 生产者与消费者共享的音频缓冲
#[derive(Clone, Default)]
pub struct AudioBuf {
    inner: Arc<Mutex<VecDeque<i16>>>,
    /// 是否收到过数据（用于界面判断"有声音"
    pub got_data: Arc<AtomicBool>,
    /// 最近一块音频的 RMS（f32 的 bit 形式），用于电平显示
    pub level: Arc<AtomicU32>,
}

impl AudioBuf {
    /// 生产者调用：追加一块采样，超出上限丢弃最旧的
    pub fn push(&self, samples: &[i16]) {
        if samples.is_empty() {
            return;
        }
        if let Ok(mut g) = self.inner.lock() {
            // 一次性推入超过上限时，只保留最后 MAX_SAMPLES 个
            let start = samples.len().saturating_sub(MAX_SAMPLES);
            g.extend(&samples[start..]);
            while g.len() > MAX_SAMPLES {
                g.pop_front();
            }
        }
        self.got_data.store(true, Ordering::Relaxed);

        let sum: f32 = samples
            .iter()
            .map(|s| {
                let v = *s as f32 / 32768.0;
                v * v
            })
            .sum();
        let rms = (sum / samples.len() as f32).sqrt();
        self.level.store(rms.to_bits(), Ordering::Relaxed);
    }

    /// 消费者调用：取走最多 want 个采样
    fn take(&self, want: usize) -> Vec<i16> {
        let Ok(mut g) = self.inner.lock() else {
            return Vec::new();
        };
        let n = want.min(g.len());
        g.drain(..n).collect()
    }

    /// 清空缓冲（关闭声音时调用，避免下次打开先放出旧声音）
    pub fn clear(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.clear();
        }
        self.got_data.store(false, Ordering::Relaxed);
        self.level.store(0f32.to_bits(), Ordering::Relaxed);
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn active(&self) -> bool {
        self.got_data.load(Ordering::Relaxed)
    }
}

/// 一次播放会话：把 AudioBuf 接到 rodio
pub struct AudioPlayer {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    buf: AudioBuf,
}

impl AudioPlayer {
    /// 启动播放线程。设备打不开时返回的实例会立刻结束（`playing()` 为 false）。
    pub fn start(buf: AudioBuf, volume: Arc<AtomicU32>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let b = buf.clone();
        let sp = stop.clone();
        let thread = std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || run(b, volume, sp))
            .ok();
        Self { stop, thread, buf }
    }

    pub fn playing(&self) -> bool {
        self.buf.active()
    }

    pub fn level(&self) -> f32 {
        self.buf.level()
    }

    /// 停止播放（不动 RTSP 连接）
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        // 关掉声音时清缓冲，下次打开不会先听到旧内容
        self.buf.clear();
    }
}

fn run(buf: AudioBuf, volume: Arc<AtomicU32>, stop: Arc<AtomicBool>) {
    // 播放设备只能在本线程创建：cpal 的流不是 Send
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

    // 每次取 100 ms，足够平滑又不至于空转
    let chunk = (SAMPLE_RATE as usize) / 10;

    while !stop.load(Ordering::SeqCst) {
        let vol = f32::from_bits(volume.load(Ordering::Relaxed));
        player.set_volume(vol.clamp(0.0, 4.0));

        let samples = buf.take(chunk);
        if samples.is_empty() {
            // 没数据就等一下，别空转烧 CPU
            sleep_stop(Duration::from_millis(40), &stop);
            continue;
        }

        let floats: Vec<f32> = samples.iter().map(|s| *s as f32 / 32768.0).collect();
        player.append(rodio::buffer::SamplesBuffer::new(channels, rate, floats));

        // 队列超过约 1 秒就清空，避免延迟累积（网络抖动或解码抖动都会积压）
        if player.len() > 10 {
            player.clear();
        }
    }

    player.stop();
    let _ = sink;
}

fn sleep_stop(dur: Duration, stop: &AtomicBool) -> bool {
    let step = Duration::from_millis(20);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_drops_oldest_when_full() {
        let b = AudioBuf::default();
        // 塞进 3 秒的数据，只应保留最后 2 秒
        let total = SAMPLE_RATE as usize * 3;
        let chunk: Vec<i16> = (0..total).map(|i| (i % 1000) as i16).collect();
        for part in chunk.chunks(480) {
            b.push(part);
        }
        let got = b.take(MAX_SAMPLES * 2);
        assert_eq!(got.len(), MAX_SAMPLES, "缓冲应被截断到上限");
    }

    #[test]
    fn buffer_take_drains_in_order() {
        let b = AudioBuf::default();
        b.push(&[1, 2, 3, 4, 5]);
        assert_eq!(b.take(3), vec![1, 2, 3]);
        assert_eq!(b.take(10), vec![4, 5]);
        assert!(b.take(1).is_empty());
    }

    #[test]
    fn level_reflects_amplitude() {
        let b = AudioBuf::default();
        b.push(&[16384i16; 100]); // 半幅
        let lv = b.level();
        assert!((lv - 0.5).abs() < 0.02, "RMS 应约等于 0.5，实际 {lv}");
    }

    #[test]
    fn clear_resets_state() {
        let b = AudioBuf::default();
        b.push(&[100, 200]);
        b.clear();
        assert!(!b.active());
        assert_eq!(b.level(), 0.0);
        assert!(b.take(10).is_empty());
    }
}
