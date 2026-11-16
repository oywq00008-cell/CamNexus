//! 单路视频流：FFmpeg 子进程解码 → rawvideo 管道 → 最新帧

use crate::i18n::{self, tr};
use crate::audio::AudioBuf;
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Connecting,
    Live,
    Reconnecting,
}

#[derive(Clone)]
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub seq: u64,
    pub data: Arc<Vec<u8>>,
}

#[derive(Clone)]
pub struct Status {
    pub state: State,
    pub fps: f32,
    pub msg: String,
    pub reconnects: u32,
    pub frames: u64,
    /// 最近一次收到帧的时间
    pub last_frame: Option<Instant>,
    /// 帧到达间隔的抖动（相邻间隔差值的 EMA，毫秒）
    pub jitter_ms: f32,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            state: State::Connecting,
            fps: 0.0,
            msg: tr("正在连接").into(),
            reconnects: 0,
            frames: 0,
            last_frame: None,
            jitter_ms: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct StreamCfg {
    pub url: String,
    pub ffmpeg: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub tcp: bool,
    /// 硬件解码后端名（"none" / "cuda" / "d3d11va" …），空或 none 表示软解
    pub hwdec: String,
    /// 是否让这条流顺带产出音频（省掉单独一路 RTSP 会话）
    pub audio: bool,
    /// 启动前等这么久再建连。多路同时拉起会给相机和 CPU 造成尖峰，
    /// 错开一点，界面也不用为此阻塞。
    pub start_delay: Duration,
}

/// 多久没收到帧就判定「卡住」。
///
/// 摄像机假死（NVR 卡住、无线丢包、RTSP 会话僵死）时 TCP 往往还连着，
/// ffmpeg 也不会主动退出，界面会一直显示「在线 + 上一帧」——
/// 必须自己发现并杀掉子进程让它重连。8 秒对 12.5fps 的常规流足够宽松。
const STALL_TIMEOUT: Duration = Duration::from_secs(8);

/// 线程名里不能带凭据：`rtsp://user:pass@host/...` 会进调试器和崩溃转储。
fn safe_tag(url: &str) -> String {
    match url.find("://") {
        Some(i) => {
            let (head, rest) = url.split_at(i + 3);
            match rest.find('@') {
                Some(at) => format!("{head}{}", &rest[at + 1..]),
                None => url.to_string(),
            }
        }
        None => url.to_string(),
    }
}

pub struct Stream {
    shared: Arc<Mutex<Option<Frame>>>,
    /// 音频缓冲（所有重连共用同一份，播放器不用跟着断）
    audio: AudioBuf,
    /// SDP 是否声明了音轨；探测失败或没有音轨时为 false
    audio_ok: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    child: Arc<Mutex<Option<Child>>>,
    cpu: Mutex<crate::sys::CpuMeter>,
}

impl Stream {
    pub fn spawn(cfg: StreamCfg) -> Self {
        let shared = Arc::new(Mutex::new(None));
        let audio = AudioBuf::default();
        let audio_ok = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(Status::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));

        let (s, st, sp, ch) = (
            shared.clone(),
            status.clone(),
            stop.clone(),
            child.clone(),
        );
        let (au, aok) = (audio.clone(), audio_ok.clone());
        let c = cfg.clone();
        let thread = std::thread::Builder::new()
            .name(format!("stream {}", safe_tag(&cfg.url)))
            .spawn(move || run_loop(c, s, st, sp, ch, au, aok))
            .ok();

        Self {
            shared,
            audio,
            audio_ok,
            status,
            stop,
            thread,
            child,
            cpu: Mutex::new(crate::sys::CpuMeter::default()),
        }
    }

    /// 采样本路解码进程的 CPU 占用（占总算力百分比），约 1 秒调用一次
    pub fn sample_cpu(&self) -> f32 {
        let pid = self
            .child
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|c| c.id()));
        match pid {
            Some(pid) => self
                .cpu
                .lock()
                .map(|mut m| m.sample(pid))
                .unwrap_or(0.0),
            None => 0.0,
        }
    }

    /// 音频缓冲：供播放器消费
    pub fn audio(&self) -> AudioBuf {
        self.audio.clone()
    }

    /// 这条流是否真的带音轨（探测结果）
    pub fn audio_available(&self) -> bool {
        self.audio_ok.load(Ordering::Relaxed)
    }

    /// 取最新一帧（Arc 克隆，代价极低）
    pub fn frame(&self) -> Option<Frame> {
        self.shared.lock().ok().and_then(|g| g.clone())
    }

    pub fn status(&self) -> Status {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// 手动重连：杀掉当前子进程，线程会自动重启
    pub fn kick(&self) {
        if let Ok(mut g) = self.child.lock()
            && let Some(c) = g.as_mut() {
                let _ = c.kill();
            }
        if let Ok(mut s) = self.status.lock() {
            s.msg = tr("手动重连").into();
        }
    }

    /// 每帧调一次：发现画面长时间没更新就杀掉 ffmpeg，交给重连逻辑处理。
    ///
    /// 不这么做的话，摄像机假死时 `State::Live` 会一直保持，
    /// 界面显示「在线 + 旧帧率」和一张冻结的旧画面，等于在骗人。
    pub fn tick(&self) {
        let stalled = match self.status.lock() {
            Ok(s) => {
                s.state == State::Live
                    && s.last_frame
                        .map(|t| t.elapsed() > STALL_TIMEOUT)
                        .unwrap_or(false)
            }
            Err(_) => false,
        };
        if !stalled {
            return;
        }
        // 先改状态，避免同一帧内重复触发（stalled 的判断依赖 Live）
        if let Ok(mut s) = self.status.lock() {
            s.state = State::Reconnecting;
            s.fps = 0.0;
            s.msg = tr("画面卡住，正在重连").into();
        }
        if let Ok(mut g) = self.child.lock()
            && let Some(c) = g.as_mut() {
                let _ = c.kill(); // 管道关闭后读线程会自行退出并重连
            }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut g) = self.child.lock()
            && let Some(c) = g.as_mut() {
                let _ = c.kill();
            }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn build_command(cfg: &StreamCfg, audio_port: u16) -> Command {
    let mut cmd = Command::new(&cfg.ffmpeg);
    cmd.arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin");

    // 硬件解码：只把「解码」交给显卡，缩放仍走 CPU。
    // 实测 scale_cuda + hwdownload 比纯软件解码慢 141%，故不使用 GPU 内缩放。
    let hw = cfg.hwdec.trim();
    if !hw.is_empty() && hw != "none" {
        cmd.arg("-hwaccel").arg(hw);
    }

    cmd.arg("-rtsp_transport")
        .arg(if cfg.tcp { "tcp" } else { "udp" })
        // 丢弃损坏包；但**不能用 nobuffer**：
        // 它让 ffmpeg 不等关键帧就开始喂解码器，解码出来全是空帧
        // （YUV 0,0,0 → RGB 纯绿），启动时绿屏几秒。实测去掉后
        // 首帧反而更快（733ms → 630ms），零代价。
        .arg("-fflags")
        .arg("discardcorrupt")
        .arg("-flags")
        .arg("low_delay")
        .arg("-probesize")
        .arg("32768")
        .arg("-analyzeduration")
        .arg("0")
        // FFmpeg 8.x 用 -timeout（微秒）；旧版本的 -rw_timeout 已被移除
        .arg("-timeout")
        .arg("5000000")
        .arg("-i")
        .arg(&cfg.url)
        .arg("-an")
        .arg("-vf")
        .arg(format!(
            "scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1",
            w = cfg.width,
            h = cfg.height
        ));

    // 帧率策略：
    //   fps = 0 → 跟随源（passthrough）。强行把 12.5fps 转成 30fps 会由 ffmpeg 复制帧，
    //   造成帧到达间隔忽长忽短，既影响观感也让「链路抖动」指标失真。
    //   fps > 0 → 按目标帧率重采样（低于源帧率时用于降负载）。
    if cfg.fps > 0 {
        cmd.arg("-r").arg(cfg.fps.to_string());
    } else {
        cmd.arg("-fps_mode").arg("passthrough");
    }

    cmd.arg("-map").arg("0:v:0");

    cmd.arg("-f")
        .arg("rawvideo")
        .arg("-pix_fmt")
        .arg("rgb24")
        .arg("-");

    // 第二路输出：音频推到本地回环。
    // 与视频共用同一个 RTSP 会话，摄像机只看到 1 个客户端。
    if audio_port > 0 {
        cmd.arg("-map")
            .arg("0:a:0")
            .arg("-f")
            .arg("s16le")
            .arg("-ar")
            .arg("48000")
            .arg("-ac")
            .arg("1")
            .arg(format!("tcp://127.0.0.1:{audio_port}"));
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    cmd
}

fn run_loop(
    cfg: StreamCfg,
    shared: Arc<Mutex<Option<Frame>>>,
    status: Arc<Mutex<Status>>,
    stop: Arc<AtomicBool>,
    child_slot: Arc<Mutex<Option<Child>>>,
    audio: AudioBuf,
    audio_ok: Arc<AtomicBool>,
) {
    let frame_len = (cfg.width as usize) * (cfg.height as usize) * 3;

    // 错峰建连：延迟放在取流线程里，不阻塞界面
    if !cfg.start_delay.is_zero() && !sleep_stop(cfg.start_delay, &stop) {
        return;
    }

    // 只探测一次：SDP 里有没有 m=audio。
    // 注意该型号的 SDP 里 m=audio 的端口字段恒为 0，不能拿端口判断可用性。
    let mut want_audio = cfg.audio && crate::discover::stream_has_audio(&cfg.url).unwrap_or(false);
    audio_ok.store(want_audio, Ordering::Relaxed);

    let mut backoff: u64 = 1;
    let mut reconnects: u32 = 0;
    // 带音频启动却一帧都没出的次数；连续两次就退回纯视频
    let mut audio_fails: u8 = 0;
    let mut seq: u64 = 0;

    while !stop.load(Ordering::SeqCst) {
        {
            let mut s = status.lock().unwrap();
            s.state = if reconnects == 0 {
                State::Connecting
            } else {
                State::Reconnecting
            };
            s.msg = if reconnects == 0 {
                tr("正在连接").into()
            } else {
                i18n::retry_n(reconnects)
            };
        }

        // 每次启动都新建监听：临时端口，避免重连时旧端口被占
        let (audio_port, audio_listener) = if want_audio {
            match TcpListener::bind(("127.0.0.1", 0)) {
                Ok(l) => match l.local_addr() {
                    Ok(a) => (a.port(), Some(l)),
                    Err(_) => (0, None),
                },
                Err(_) => (0, None),
            }
        } else {
            (0, None)
        };

        let spawned = build_command(&cfg, audio_port).spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                if let Ok(mut s) = status.lock() {
                    s.state = State::Reconnecting;
                    s.msg = i18n::err(tr("无法启动 ffmpeg"), e);
                }
                if !sleep_stop(Duration::from_secs(backoff), &stop) {
                    break;
                }
                backoff = (backoff * 2).min(8);
                reconnects += 1;
                continue;
            }
        };

        // 等 ffmpeg 连上来，持续把音频写进缓冲
        if let Some(l) = audio_listener {
            let buf = audio.clone();
            let sp = stop.clone();
            let _ = std::thread::Builder::new()
                .name("audio-read".into())
                .spawn(move || read_audio(l, buf, sp));
        }

        // 收集 stderr，便于定位 401 / 超时等问题
        let errbuf = Arc::new(Mutex::new(String::new()));
        if let Some(mut err) = child.stderr.take() {
            let eb = errbuf.clone();
            std::thread::spawn(move || {
                let mut chunk = [0u8; 1024];
                while let Ok(n) = err.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut g) = eb.lock() {
                        g.push_str(&String::from_utf8_lossy(&chunk[..n]));
                        if g.len() > 2000 {
                            let cut = g.len() - 1200;
                            let tail: String = g[cut..].to_string();
                            *g = tail;
                        }
                    }
                }
            });
        }

        // 首帧看门狗：ffmpeg 有时会"连着但不吐数据"，
        // 这种状态既不退出也不出错，光靠 -timeout 不一定兜得住。
        let got_first = Arc::new(AtomicBool::new(false));
        let child_pid = child.id();
        {
            let flag = got_first.clone();
            let slot = child_slot.clone();
            let sp = stop.clone();
            std::thread::Builder::new()
                .name("first-frame".into())
                .spawn(move || {
                    let deadline = Instant::now() + Duration::from_secs(15);
                    while Instant::now() < deadline {
                        if sp.load(Ordering::SeqCst) || flag.load(Ordering::SeqCst) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(200));
                    }
                    if flag.load(Ordering::SeqCst) {
                        return;
                    }
                    // 只杀自己那一个：期间可能已经重连过，别误伤新进程
                    if let Ok(mut g) = slot.lock()
                        && let Some(c) = g.as_mut()
                        && c.id() == child_pid
                    {
                        let _ = c.kill();
                    }
                })
                .ok();
        }

        if let Ok(mut slot) = child_slot.lock() {
            *slot = Some(child);
        }

        // stdout 由本线程独占，避免每帧加锁取句柄
        let mut stdout = {
            let mut g = match child_slot.lock() {
                Ok(g) => g,
                Err(_) => break,
            };
            match g.as_mut().and_then(|c| c.stdout.take()) {
                Some(s) => s,
                None => break,
            }
        };

        // 缓冲池：正常情况下一帧 11 MB（1440p RGB），
        // 每帧重新 vec! 等于每秒几十 MB 的分配 + 清零，全浪费。
        // 上一帧没人引用时就把它收回来复用。
        let mut spare: Vec<Vec<u8>> = Vec::new();
        let mut fps_window = Instant::now();
        let mut fps_count = 0u32;
        let mut got_any = false;
        let mut last_arrival: Option<Instant> = None;
        let mut last_gap: Option<f32> = None;
        let mut jitter = 0.0f32;

        loop {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let mut buf = spare.pop().unwrap_or_else(|| vec![0u8; frame_len]);
            if read_exact_into(&mut stdout, &mut buf).is_err() {
                break;
            }
            // 帧到达间隔抖动：相邻间隔的差值，反映链路稳定性而非帧率高低
            let arrived = Instant::now();
            if let Some(prev) = last_arrival {
                let gap = arrived.duration_since(prev).as_secs_f32() * 1000.0;
                if let Some(pg) = last_gap {
                    let d = (gap - pg).abs();
                    jitter = if jitter == 0.0 { d } else { jitter * 0.8 + d * 0.2 };
                }
                last_gap = Some(gap);
            }
            last_arrival = Some(arrived);
            got_any = true;
            got_first.store(true, Ordering::SeqCst);
            seq += 1;

            let frame = Frame {
                w: cfg.width as usize,
                h: cfg.height as usize,
                seq,
                data: Arc::new(buf),
            };
            if let Ok(mut g) = shared.lock() {
                // 换下来的旧帧如果只有我们持有，就把它的内存收回池子
                if let Some(old) = g.replace(frame)
                    && let Ok(v) = Arc::try_unwrap(old.data) {
                        // 只留一块备用：池子越大越省分配，但每块就是 11 MB 常驻内存，
                        // 旧电脑上内存比这点分配开销更值钱
                        if spare.is_empty() {
                            spare.push(v);
                        }
                    }
            }

            fps_count += 1;
            let elapsed = fps_window.elapsed();
            let fps_ready = elapsed >= Duration::from_millis(900);
            let fps_now = if fps_ready {
                // 先取值再清零，顺序反了就永远是 0
                let v = fps_count as f32 / elapsed.as_secs_f32();
                fps_count = 0;
                fps_window = Instant::now();
                v
            } else {
                0.0
            };
            // 每帧都刷新时间戳和帧计数（看门狗依赖 last_frame）；
            // fps 每 900ms 才算一次，避免抖动
            if let Ok(mut s) = status.lock() {
                s.state = State::Live;
                s.last_frame = Some(arrived);
                s.frames = s.frames.saturating_add(1);
                s.jitter_ms = jitter;
                if fps_ready {
                    s.fps = fps_now;
                    s.msg = String::new();
                }
            }
            backoff = 1;
        }

        if let Ok(mut slot) = child_slot.lock() {
            if let Some(c) = slot.as_mut() {
                let _ = c.kill();
                let _ = c.wait();
            }
            *slot = None;
        }

        // 带着音频却一帧都没出：多半是音轨把整条流拖死了。
        // 连续两次就关掉音频重连——宁可没声音，也不能没画面。
        if want_audio && !got_any {
            audio_fails += 1;
            if audio_fails >= 2 {
                want_audio = false;
                audio_ok.store(false, Ordering::Relaxed);
                if let Ok(mut s) = status.lock() {
                    s.msg = tr("音频不可用，已切回纯画面").into();
                }
            }
        }

        if stop.load(Ordering::SeqCst) {
            break;
        }

        reconnects += 1;
        let detail = errbuf
            .lock()
            .map(|g| g.lines().last().unwrap_or("").trim().to_string())
            .unwrap_or_default();
        if let Ok(mut s) = status.lock() {
            s.state = State::Reconnecting;
            s.reconnects = reconnects;
            s.fps = 0.0;
            s.msg = if detail.is_empty() {
                tr("连接中断，正在重试").into()
            } else {
                detail
            };
        }
        // 首帧都没拿到说明地址/认证有问题，退避更久一些
        let wait = if got_any { 1 } else { backoff };
        if !sleep_stop(Duration::from_secs(wait), &stop) {
            break;
        }
        if !got_any {
            backoff = (backoff * 2).min(10);
        }
    }
}

/// 从管道读满一帧
/// 接受 ffmpeg 的音频连接并持续读取，转成 i16 写入缓冲。
///
/// 设备没有音轨时 ffmpeg 不会连接，这里最多等 10 秒就退出，不留挂起线程。
fn read_audio(listener: TcpListener, buf: AudioBuf, stop: Arc<AtomicBool>) {
    let _ = listener.set_nonblocking(true);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut conn: Option<TcpStream> = None;
    while Instant::now() < deadline {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match listener.accept() {
            Ok((s, _)) => {
                conn = Some(s);
                break;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return,
        }
    }
    let Some(mut conn) = conn else {
        return; // 没有音频流
    };
    let _ = conn.set_nonblocking(false);
    let _ = conn.set_read_timeout(Some(Duration::from_millis(500)));

    let mut raw = [0u8; 4096];
    // TCP 不保证按采样边界切分，奇数个字节要留到下次
    let mut carry: Option<u8> = None;
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        match conn.read(&mut raw) {
            Ok(0) => break,
            Ok(n) => {
                let mut samples = Vec::with_capacity(n / 2 + 1);
                let mut i = 0;
                if let Some(hi) = carry.take() {
                    samples.push(i16::from_le_bytes([raw[0], hi]));
                    i = 1;
                }
                while i + 1 < n {
                    samples.push(i16::from_le_bytes([raw[i], raw[i + 1]]));
                    i += 2;
                }
                if i < n {
                    carry = Some(raw[i]);
                }
                buf.push(&samples);
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => break,
        }
    }
}

fn read_exact_into<R: Read>(r: &mut R, buf: &mut [u8]) -> std::io::Result<()> {
    let mut filled = 0usize;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    tr("ffmpeg 已退出"),
                ));
            }
            Ok(n) => filled += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// 分片睡眠，便于快速响应停止信号
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_tag_strips_credentials() {
        // 线程名会出现在调试器/崩溃转储里，绝不能带密码
        assert_eq!(
            safe_tag("rtsp://admin:secret@192.168.1.10:554/Streaming/Channels/101"),
            "rtsp://192.168.1.10:554/Streaming/Channels/101"
        );
        // 没有凭据时原样返回
        assert_eq!(
            safe_tag("rtsp://192.168.1.10:554/live"),
            "rtsp://192.168.1.10:554/live"
        );
        assert_eq!(safe_tag("nonsense"), "nonsense");
    }
}
