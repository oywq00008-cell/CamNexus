//! 事件录像：常连主码流，只在内存里留最近若干秒，检测到目标才落盘。
//!
//! 为什么不「先录再删」：那样 24 小时都在写盘，事件录像省硬盘、护盘的初衷就没了。
//! 这里让 ffmpeg 以 `-c:v copy` 把码流吐到本地回环 TCP（不解码、不转码），
//! 主程序收字节流放进环形缓冲；触发时才把缓冲 + 后续直播写进文件。
//!
//! # 结构
//!
//! - [`Engine`]：纯逻辑内核，只管「什么时候开文件、写什么、什么时候收尾」，
//!   不碰网络也不读时钟——所有时间点由调用方传入，因此可以完整单测。
//! - `pump` / `run_session`：IO 外壳，负责 TCP、ffmpeg 子进程、看门狗与重连。
//! - `remux`：收尾后的重封装，由单个工作线程串行处理，避免事件频繁时线程爆掉。
//!
//! # 为什么要重封装
//!
//! 触发点必然落在 GOP 中间，直接落盘的 TS 头部解不了。用 `ffmpeg -c copy`
//! 重新封装一次，输出会从第一个关键帧开始，播放器不会花屏。代价是「前 5 秒」
//! 实际会多一点或少一点（取决于相机 GOP），监控场景无所谓。

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::i18n::tr;

/// 按字节的硬上限：码率再高也不能把内存吃爆
const BUF_BYTES: usize = 32 * 1024 * 1024;
/// 缓冲在配置值之外多留的秒数：起点要退到关键帧，多数相机 GOP 在 2~4 秒
const BUF_EXTRA_SECS: u64 = 3;
/// 这么久收不到数据判定连接假死，杀掉重连
const STALL_SECS: u64 = 10;
/// 单次读超时：让线程能定期检查停止/触发状态
const READ_TIMEOUT_MS: u64 = 300;
/// 打开文件失败后的重试间隔（磁盘满/权限问题时避免每块数据都试一次）
const OPEN_RETRY_SECS: u64 = 5;
/// 小于这个大小的片段视为没有内容，直接丢弃
const MIN_CLIP_BYTES: u64 = 4096;
/// 连接稳定这么久之后再断，说明是偶发断线，退避从头算
const HEALTHY_SECS: u64 = 30;
/// 重封装队列长度：超出的片段先留成 .ts，下次启动再处理
const REMUX_QUEUE: usize = 8;
/// 合并后的文件名里不再区分，沿用较早那一段的名字

#[derive(Clone, Debug, PartialEq)]
pub enum TapState {
    Connecting,
    Idle,
    Recording,
    Error(String),
}

#[derive(Clone, Debug)]
pub struct TapStatus {
    pub state: TapState,
    /// 缓冲覆盖的时长（秒），用于界面提示
    pub buffered_secs: f32,
    /// 本次运行累计存下的片段数
    pub clips: u32,
    pub last_error: Option<String>,
}

impl Default for TapStatus {
    fn default() -> Self {
        Self {
            state: TapState::Connecting,
            buffered_secs: 0.0,
            clips: 0,
            last_error: None,
        }
    }
}

/// 触发者类别。只影响一件事：单段到上限后要不要静默。
///
/// 车能停一整晚，不静默就会录到天亮；人一直在动说明确实有事，
/// 没有理由停下来，直接接着录下一段。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TriggerKind {
    Person,
    Vehicle,
}

/// 触发阈值（归一化宽度/秒）。
///
/// 人比车敏感一点：人走路本来就慢，而静止目标的抖动大约 1–2%/秒，4% 留了余量。
/// 车辆必须动得更明显，否则停着的车框边缘抖一下就能触发。
pub const PERSON_SPEED: f32 = 0.04;
pub const VEHICLE_SPEED: f32 = 0.08;

/// 该不该触发、以及这是人还是车。
pub fn trigger_kind(person: bool, motion: f32) -> Option<TriggerKind> {
    if person {
        (motion >= PERSON_SPEED).then_some(TriggerKind::Person)
    } else {
        (motion >= VEHICLE_SPEED).then_some(TriggerKind::Vehicle)
    }
}

pub struct Tap {
    /// 触发截止时刻（epoch 毫秒）；0 = 没触发。用原子量代替锁，热路径零竞争。
    trigger_ms: Arc<AtomicU64>,
    /// 最近一次触发是谁引起的（0 = 人，1 = 车）
    kind: Arc<AtomicU8>,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<TapStatus>>,
}

impl Tap {
    pub fn status(&self) -> TapStatus {
        self.status.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// 把触发窗口延长到 `now + post`。连续触发会不断续期。
    pub fn trigger(&self, post: Duration, kind: TriggerKind) {
        let until = epoch_ms() + post.as_millis() as u64;
        self.trigger_ms.fetch_max(until, Ordering::SeqCst);
        self.kind.store(kind as u8, Ordering::SeqCst);
    }

    pub fn is_recording(&self) -> bool {
        matches!(self.status().state, TapState::Recording)
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        // 线程会在退出前把当前片段收尾
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// 当前 epoch 毫秒。时间戳类判断（触发截止、音频空闲）都用它，
/// 免得各处各写一套、单位还容易搞混。
pub fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------- 环形缓冲

/// 按时间与字节双重裁剪的字节环形缓冲。
struct Ring {
    chunks: VecDeque<(Instant, Vec<u8>)>,
    bytes: usize,
    max_age: Duration,
}

impl Ring {
    fn new(pre_secs: u32) -> Self {
        Self {
            chunks: VecDeque::new(),
            bytes: 0,
            max_age: Duration::from_secs(pre_secs as u64 + BUF_EXTRA_SECS),
        }
    }

    fn push(&mut self, at: Instant, data: Vec<u8>) {
        self.bytes += data.len();
        self.chunks.push_back((at, data));
        self.trim(at);
    }

    fn trim(&mut self, now: Instant) {
        while let Some((t, d)) = self.chunks.front() {
            let too_old = now.duration_since(*t) > self.max_age;
            let too_big = self.bytes > BUF_BYTES;
            if !too_old && !too_big {
                break;
            }
            self.bytes = self.bytes.saturating_sub(d.len());
            self.chunks.pop_front();
        }
    }

    /// 取出全部并清空（触发瞬间用）
    fn drain(&mut self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.bytes);
        for (_, d) in self.chunks.drain(..) {
            out.extend_from_slice(&d);
        }
        self.bytes = 0;
        out
    }

    fn clear(&mut self) {
        self.chunks.clear();
        self.bytes = 0;
    }

    /// 缓冲覆盖的时长（秒）
    fn span_secs(&self) -> f32 {
        match (self.chunks.front(), self.chunks.back()) {
            (Some((a, _)), Some((b, _))) => b.duration_since(*a).as_secs_f32(),
            _ => 0.0,
        }
    }
}

// ---------------------------------------------------------------- 逻辑内核

/// 正在写的片段
struct Clip {
    path: PathBuf,
    file: std::fs::File,
    started: bool,
    /// 片段开始写入的时刻，用于单段时长上限
    opened_at: Instant,
    /// 真正写进去的时长（不含续接窗口里的空等），日志和报告都用它
    written: Duration,
    /// 上一块数据的时间戳，用来累加 written
    last_data: Instant,
}

impl Clip {
    fn open(root: &Path, channel: &str, now: Instant) -> Option<Self> {
        let (y, mo, d, h, mi, s) = crate::sys::local_now();
        let dir = root.join(format!("{y:04}-{mo:02}-{d:02}")).join(channel);
        std::fs::create_dir_all(&dir).ok()?;
        let stem = format!("{y:04}-{mo:02}-{d:02} {h:02}-{mi:02}-{s:02}");
        // 两个通道同名、或同一秒内开了两次，文件名会撞；
        // File::create 是截断式的，撞上会把对方正在写的片段清空
        let mut path = dir.join(format!("{stem}.part.ts"));
        let mut n = 1;
        while path.exists() && n < 100 {
            n += 1;
            path = dir.join(format!("{stem}-{n}.part.ts"));
        }
        let file = std::fs::File::create(&path).ok()?;
        Some(Self {
            path,
            file,
            started: false,
            written: Duration::ZERO,
            last_data: now,
            opened_at: now,
        })
    }
}

/// 纯逻辑内核：所有时间点由调用方传入，便于单测。
struct Engine {
    ring: Ring,
    writer: Option<Clip>,
    open_retry_at: Option<Instant>,
    root: PathBuf,
    channel: String,
    status: Arc<Mutex<TapStatus>>,
    /// 触发窗口结束后，等这么久才真正收尾。
    ///
    /// 期间目标重新出现就继续写同一个文件——比「先各自落盘、事后拼接」省得多：
    /// 拼接要读旧文件再整个重写，文件越长越贵，反复拼就是 O(n²)。
    /// 续接窗口只要保持文件不关，代价为零。
    gap_since: Option<Instant>,
    merge_gap: Duration,
    /// 单段最长时长，0 = 不限
    max_clip: Duration,
    /// 当前是否真的在往文件里写。续接窗口内文件是开着的，但不再写。
    writing: bool,
    /// 达到上限后的静默截止时间（**只对车生效**）。
    ///
    /// 车能停一整晚，不静默就会录到天亮；人不一样——一直在动说明确实有事，
    /// 到上限就收尾、立刻接着录下一段。静默期满会自动重新武装。
    suppress_until: Option<Instant>,
    /// 最近一次触发的来源，只用来决定单段到上限后要不要静默
    kind: TriggerKind,
    /// 测试里关掉，避免真的起 ffmpeg
    remux: bool,
    /// 测试里关掉：否则单元测试会往用户的真实日志里写内容
    log_enabled: bool,
}

impl Engine {
    fn new(
        root: PathBuf,
        channel: String,
        pre_secs: u32,
        merge_gap: Duration,
        max_clip: Duration,
        status: Arc<Mutex<TapStatus>>,
    ) -> Self {
        Self {
            ring: Ring::new(pre_secs),
            writer: None,
            open_retry_at: None,
            root,
            channel,
            status,
            gap_since: None,
            merge_gap,
            max_clip,
            writing: false,
            suppress_until: None,
            kind: TriggerKind::Vehicle,
            remux: true,
            log_enabled: true,
        }
    }

    /// 收到一段码流。`active` 表示当前是否处于触发窗口内。
    ///
    /// 「已经开着文件」就继续写，哪怕这一帧 `active` 刚好变成 false——
    /// 收尾由 `tick` 按续接窗口决定，否则窗口内的最后一两帧会掉进环形缓冲里。
    fn on_data(&mut self, data: &[u8], active: bool, now: Instant) {
        // 强制收尾后的静默期：目标还在，就不再开新片段
        if self.suppress_until.is_some() && self.writer.is_none() {
            self.ring.push(now, data.to_vec());
            return;
        }
        if !active && self.writer.is_none() {
            self.ring.push(now, data.to_vec());
            self.sync_buffered();
            return;
        }
        if !active {
            // 触发窗口结束了。
            //
            // 文件**先不收尾**——续接窗口内再来目标就继续写同一个文件，
            // 但这段空等时间**不再写入**：没人没车的几十秒没有任何价值，
            // 写进去只会让每段录像白白变长，指示灯也一直红着。
            // 数据照常进环形缓冲，留给下一段做预录。
            if self.writing {
                self.writing = false;
                self.set_state(TapState::Idle);
            }
            self.ring.push(now, data.to_vec());
            self.sync_buffered();
            return;
        }
        self.writing = true;
        if let Some(clip) = self.writer.as_mut() {
            if clip.started {
                clip.written += now.saturating_duration_since(clip.last_data);
            }
            clip.last_data = now;
        }

        if self.writer.is_none() && self.may_open(now) {
            match Clip::open(&self.root, &self.channel, now) {
                Some(c) => {
                    self.writer = Some(c);
                    self.open_retry_at = None;
                }
                None => {
                    self.open_retry_at = Some(now);
                    self.note_error(tr("无法创建录像文件").to_string());
                }
            }
        }

        // 先处理写入，再同步状态：日志/状态方法借的是整个 self，
        // 放在 writer 的可变借用里会冲突
        let mut just_started = false;
        if let Some(clip) = self.writer.as_mut() {
            if !clip.started {
                // 第一笔：把缓冲里的「过去」先倒进去，这就是触发前那几秒
                let past = self.ring.drain();
                let _ = clip.file.write_all(&past);
                clip.started = true;
                just_started = true;
            }
            let _ = clip.file.write_all(data);
        }
        if just_started {
            self.log(tr("事件片段开始录制"));
            self.set_state(TapState::Recording);
        }
    }

    /// 每轮循环都要调：维护「触发结束后的续接窗口」。
    fn tick(&mut self, active: bool, now: Instant) {
        if active {
            self.gap_since = None;
            // 静默期满就重新武装：万一是个赖着不走的人，
            // 不能让这个通道从此再也不录
            if self.suppress_until.map(|t| now >= t).unwrap_or(false) {
                self.suppress_until = None;
            }
            // 单段到点就收尾，然后进入静默：一辆一直停着的车不能录到天亮
            let too_long = self
                .writer
                .as_ref()
                .map(|c| {
                    !self.max_clip.is_zero() && now.duration_since(c.opened_at) >= self.max_clip
                })
                .unwrap_or(false);
            if too_long {
                if let Some(clip) = self.writer.take() {
                    let mins = self.max_clip.as_secs() / 60;
                    let msg = if self.kind == TriggerKind::Vehicle {
                        tr("单段已达上限，暂停直到车辆离开")
                    } else {
                        tr("单段已达上限，接着录下一段")
                    };
                    self.log(&format!("{}（{mins} 分钟）", msg));
                    self.finish(clip);
                }
                // 只有车才静默：车能停一整晚，人一直在动说明确实有事。
                // 人不停 → 下一帧数据到来就开新片段，前后靠环形缓冲接得上。
                if self.kind == TriggerKind::Vehicle {
                    self.suppress_until = Some(now + self.max_clip.max(Duration::from_secs(60)));
                }
            }
            return;
        }
        // 目标消失：解除静默，下次再来就能正常开录
        self.suppress_until = None;
        let since = *self.gap_since.get_or_insert(now);
        if now.duration_since(since) >= self.merge_gap {
            if let Some(clip) = self.writer.take() {
                self.finish(clip);
            }
            self.gap_since = None;
        }
        self.ring.trim(now);
        self.sync_buffered();
    }

    /// 线程退出前调用：保证不留半截文件
    fn shutdown(&mut self) {
        if let Some(clip) = self.writer.take() {
            self.finish(clip);
        }
    }

    /// 重连后缓冲必须清空：跨连接的字节拼起来是解不了的
    fn on_reconnect(&mut self) {
        self.ring.clear();
        self.sync_buffered();
    }

    fn may_open(&self, now: Instant) -> bool {
        self.open_retry_at
            .map(|t| now.duration_since(t) >= Duration::from_secs(OPEN_RETRY_SECS))
            .unwrap_or(true)
    }

    fn finish(&mut self, clip: Clip) {
        // 收尾即代表这一段结束：状态复位，指示灯跟着灭
        self.writing = false;
        self.set_state(TapState::Idle);
        let Clip { path, file, .. } = clip;
        drop(file);
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if len < MIN_CLIP_BYTES {
            let _ = std::fs::remove_file(&path);
            return;
        }
        if let Ok(mut g) = self.status.lock() {
            g.clips += 1;
        }

        let secs = clip.written.as_secs_f32().max(0.0);
        self.log(&format!("{}（{secs:.0} 秒）", tr("事件片段已保存")));
        if self.remux {
            enqueue_finish(path);
        }
    }

    fn set_state(&self, s: TapState) {
        if let Ok(mut g) = self.status.lock() {
            g.state = s;
        }
    }

    fn sync_buffered(&self) {
        let secs = self.ring.span_secs();
        if let Ok(mut g) = self.status.lock() {
            g.buffered_secs = secs;
        }
    }

    fn note_error(&self, msg: String) {
        if let Ok(mut g) = self.status.lock() {
            g.state = TapState::Error(msg.clone());
            g.last_error = Some(msg);
        }
    }

    fn log(&self, msg: &str) {
        if self.log_enabled {
            crate::logs::append(&self.channel, msg);
        }
    }
}

// ---------------------------------------------------------------- IO 外壳

/// 起一路事件录像需要的东西。做成结构体而不是六个位置参数：
/// 位置参数一多，调用点就读不出哪个是哪个了。
#[derive(Clone)]
pub struct TapSpec {
    pub ffmpeg: PathBuf,
    pub url: String,
    pub channel: String,
    pub root: PathBuf,
    pub tcp: bool,
    /// 触发前在内存里保留多少秒
    pub pre_secs: u32,
    /// 同一路两段间隔小于这个值就合并成一个文件；0 = 不合并
    pub merge_gap: Duration,
    /// 单段最长时长，0 = 不限
    pub max_clip: Duration,
    /// 启动延迟：多路同时建连会瞬间打满相机，错开一点
    pub start_delay: Duration,
}

/// 一路 tap 的共享状态
struct Ctx {
    spec: TapSpec,
    port: u16,
    trigger_ms: Arc<AtomicU64>,
    kind: Arc<AtomicU8>,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<TapStatus>>,
}

/// 起一路事件录像。拿不到端口时返回 None，调用方按「未启用」处理。
pub fn start(spec: TapSpec) -> Option<Tap> {
    let listener = TcpListener::bind("127.0.0.1:0").ok()?;
    let port = listener.local_addr().ok()?.port();

    let ctx = Ctx {
        spec,
        port,
        trigger_ms: Arc::new(AtomicU64::new(0)),
        kind: Arc::new(AtomicU8::new(TriggerKind::Vehicle as u8)),
        stop: Arc::new(AtomicBool::new(false)),
        status: Arc::new(Mutex::new(TapStatus::default())),
    };

    let tap = Tap {
        trigger_ms: ctx.trigger_ms.clone(),
        kind: ctx.kind.clone(),
        stop: ctx.stop.clone(),
        status: ctx.status.clone(),
    };

    let name = format!("evtap-{}", ctx.spec.channel.chars().take(8).collect::<String>());
    let spawned = std::thread::Builder::new()
        .name(name)
        .spawn(move || serve(listener, ctx))
        .ok();

    spawned.map(|_| tap)
}

/// 外层循环：一条连接断了就退避重连，直到被要求停止。
fn serve(listener: TcpListener, ctx: Ctx) {
    // 错峰：第 n 路延迟 n×120ms，和显示流的节奏保持一致
    sleep_stop(&ctx.stop, ctx.spec.start_delay);

    let mut backoff = 1u64;
    let mut last_log: Option<Instant> = None;
    while !ctx.stop.load(Ordering::SeqCst) {
        set_state(&ctx.status, TapState::Connecting);
        let began = Instant::now();
        run_session(&listener, &ctx);
        if ctx.stop.load(Ordering::SeqCst) {
            break;
        }
        // 稳定跑过一阵子之后再断，说明是偶发断线，退避从头算
        let healthy = began.elapsed() >= Duration::from_secs(HEALTHY_SECS);
        if healthy {
            backoff = 1;
        }
        // 连接一直起不来（相机满额、地址错误）在过去是完全静默的，
        // 用户只会看到「事件录像从不触发」。这里每分钟记一条。
        let due = last_log
            .map(|t: Instant| t.elapsed() >= Duration::from_secs(60))
            .unwrap_or(true);
        if !healthy && due {
            let why = ctx
                .status
                .lock()
                .ok()
                .and_then(|g| g.last_error.clone())
                .unwrap_or_else(|| tr("连接未能建立").to_string());
            crate::logs::append(&ctx.spec.channel, &format!("{}：{why}", tr("事件录像取流失败")));
            last_log = Some(Instant::now());
        }
        sleep_stop(&ctx.stop, Duration::from_secs(backoff));
        backoff = (backoff * 2).min(10);
    }
}

fn set_state(status: &Arc<Mutex<TapStatus>>, s: TapState) {
    if let Ok(mut g) = status.lock() {
        g.state = s;
    }
}

/// 等待一段时间，期间随时可被停止打断。
///
/// 参数用 `Duration` 而不是裸数字：之前按「秒」传参、调用方却给的是毫秒，
/// 结果第 2 路起要等 120 秒才开始连相机，表现为「只有第一路在录」。
fn sleep_stop(stop: &Arc<AtomicBool>, dur: Duration) {
    let end = Instant::now() + dur;
    while Instant::now() < end && !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 一次完整的连接周期：起 ffmpeg → 接收 → 泵数据 → 收尾。
fn run_session(listener: &TcpListener, ctx: &Ctx) {
    let mut cmd = std::process::Command::new(&ctx.spec.ffmpeg);
    cmd.arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostats")
        .arg("-rtsp_transport")
        .arg(if ctx.spec.tcp { "tcp" } else { "udp" })
        .arg("-timeout")
        .arg("5000000")
        // 与显示流一致：不加 nobuffer，否则中途接入会喂空帧给解码器
        .arg("-fflags")
        .arg("discardcorrupt")
        .arg("-i")
        .arg(&ctx.spec.url)
        .arg("-map")
        .arg("0:v:0")
        .arg("-c:v")
        .arg("copy")
        .arg("-map")
        .arg("0:a:0?")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("64k")
        .arg("-ac")
        .arg("1")
        .arg("-f")
        .arg("mpegts")
        .arg(format!("tcp://127.0.0.1:{}", ctx.port))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    // 不加这个，每路取流都会弹一个黑色控制台窗口。
    // stream.rs / record.rs 一直都有，这里当初漏了。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            note_error(&ctx.status, crate::i18n::err(tr("启动取流进程失败"), e));
            return;
        }
    };

    let Some(stream) = accept_within(listener, Duration::from_secs(15), &ctx.stop) else {
        let _ = child.kill();
        let _ = child.wait();
        note_error(&ctx.status, tr("等待取流超时").to_string());
        return;
    };
    // 关键：accept 出来的连接会继承监听套接字的非阻塞属性。
    // 不显式改回阻塞，read() 会立刻返回 WouldBlock，读循环变成纯空转——
    // 实测 4 路能把主程序烧掉 4 个核。stream.rs 的音频连接同样在这里复位。
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(READ_TIMEOUT_MS)));

    let mut engine = Engine::new(
        ctx.spec.root.clone(),
        ctx.spec.channel.clone(),
        ctx.spec.pre_secs,
        ctx.spec.merge_gap,
        ctx.spec.max_clip,
        ctx.status.clone(),
    );
    pump(stream, &mut engine, ctx, &mut child);
    engine.shutdown();
    if !matches!(ctx.status.lock().map(|g| g.state.clone()), Ok(TapState::Error(_))) {
        set_state(&ctx.status, TapState::Idle);
    }

    let _ = child.kill();
    let _ = child.wait();
}

fn note_error(status: &Arc<Mutex<TapStatus>>, msg: String) {
    if let Ok(mut g) = status.lock() {
        g.state = TapState::Error(msg.clone());
        g.last_error = Some(msg);
    }
}

/// 等 ffmpeg 连过来，超时或收到停止信号就返回 None
fn accept_within(
    listener: &TcpListener,
    limit: Duration,
    stop: &Arc<AtomicBool>,
) -> Option<TcpStream> {
    let deadline = Instant::now() + limit;
    if listener.set_nonblocking(true).is_err() {
        return None;
    }
    let mut out = None;
    while !stop.load(Ordering::SeqCst) && Instant::now() < deadline {
        match listener.accept() {
            Ok((s, _)) => {
                out = Some(s);
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    let _ = listener.set_nonblocking(false);
    out
}

fn pump(
    mut stream: TcpStream,
    engine: &mut Engine,
    ctx: &Ctx,
    child: &mut std::process::Child,
) {
    let mut buf = vec![0u8; 128 * 1024];
    let mut last_data = Instant::now();
    // 连上之后状态一直显示「连接中」会让人以为没通，收到第一块数据就转 Idle
    let mut announced = false;
    engine.on_reconnect();

    loop {
        if ctx.stop.load(Ordering::SeqCst) {
            break;
        }
        let active = ctx.trigger_ms.load(Ordering::SeqCst) > epoch_ms();

        match stream.read(&mut buf) {
            Ok(0) => break, // 对端关闭：相机断开或 ffmpeg 退出
            Ok(n) => {
                last_data = Instant::now();
                if !announced {
                    announced = true;
                    if !active {
                        set_state(&ctx.status, TapState::Idle);
                    }
                }
                engine.on_data(&buf[..n], active, Instant::now());
            }
            Err(e) => match e.kind() {
                std::io::ErrorKind::WouldBlock
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::Interrupted => {}
                _ => {
                    note_error(&ctx.status, crate::i18n::err(tr("读取中断"), e));
                    break;
                }
            },
        }

        engine.kind = if ctx.kind.load(Ordering::SeqCst) == TriggerKind::Person as u8 {
            TriggerKind::Person
        } else {
            TriggerKind::Vehicle
        };
        engine.tick(active, Instant::now());

        if last_data.elapsed() > Duration::from_secs(STALL_SECS) {
            note_error(&ctx.status, tr("取流停顿，正在重连").to_string());
            break;
        }
        if let Ok(Some(_)) = child.try_wait() {
            note_error(&ctx.status, tr("取流进程已退出").to_string());
            break;
        }
    }
}

// ---------------------------------------------------------------- 重封装

/// 重封装工作线程：串行处理；队列满了就把片段留成 .ts 等下次启动收拾。
///
/// 这里不做拼接——续接在写入阶段就完成了（`Engine::tick` 的续接窗口）。
/// 事后拼接要读旧文件再整个重写，文件越长越贵，反复拼是 O(n²)：
/// 实测 4 路事件录像能把主程序 CPU 顶到 400%。
fn remux_queue() -> &'static SyncSender<PathBuf> {
    static Q: OnceLock<SyncSender<PathBuf>> = OnceLock::new();
    Q.get_or_init(|| {
        let (tx, rx) = sync_channel::<PathBuf>(REMUX_QUEUE);
        std::thread::Builder::new()
            .name("ev-remux".into())
            .spawn(move || {
                for part in rx {
                    if remux_one(&part).is_none() {
                        keep_as_ts(&part);
                    }
                }
            })
            .ok();
        tx
    })
}

fn enqueue_finish(part: PathBuf) {
    if remux_queue().try_send(part.clone()).is_err() {
        // 队列满：先把素材保住，下次启动会统一重封装
        keep_as_ts(&part);
    }
}

/// `.part.ts` 对应的最终 mp4 路径
fn to_mp4(part: &Path) -> PathBuf {
    part.with_extension("").with_extension("mp4")
}

/// `.part.ts` → `.mp4`。用 `-c copy` 重新封装，输出从头对齐到关键帧。
fn remux_one(part: &Path) -> Option<PathBuf> {
    // 源文件可能已经被别的任务处理掉了。这时**什么都不做**——
    // 早先会继续跑一遍注定失败的 ffmpeg，然后把成品 mp4 删掉。
    if !part.is_file() {
        return None;
    }
    let ff = crate::config::find_ffmpeg("")?;
    let mp4 = to_mp4(part);
    let mut cmd = std::process::Command::new(ff);
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
        .arg("-i")
        .arg(part)
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(&mp4);
    // 收尾时也不能弹窗，否则每段录像结束都闪一个黑框
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let ok = cmd
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let good = ok
        && std::fs::metadata(&mp4)
            .map(|m| m.len() > MIN_CLIP_BYTES)
            .unwrap_or(false);
    if good {
        let _ = std::fs::remove_file(part);
        Some(mp4)
    } else {
        // 只在确认是本次失败产生的残次品时才删；源文件不在的早退不会走到这里
        let _ = std::fs::remove_file(&mp4);
        None
    }
}

/// 重封装失败也要留住素材：改名成 .ts，任何播放器都能直接播
fn keep_as_ts(part: &Path) {
    let ts = part.with_extension("").with_extension("ts");
    let _ = std::fs::rename(part, &ts);
}

/// 清掉旧版本留下的合并临时文件（现在的实现不再拼接，这里只是收尾）
fn sweep_merge_leftovers(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            if name == ".merge.list" || name == ".merge.tmp.mp4" {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

/// 启动时收拾上次强杀留下的半截文件：能重封装就重封装，别直接删素材。
pub fn recover_parts(root: &Path) {
    // 合并是在收尾时做的，启动时不可能正在进行，残留一律清掉
    sweep_merge_leftovers(root);

    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let is_part = p
                .file_name()
                .map(|n| n.to_string_lossy().ends_with(".part.ts"))
                .unwrap_or(false);
            if !is_part {
                continue;
            }
            let len = e.metadata().map(|m| m.len()).unwrap_or(0);
            if len < MIN_CLIP_BYTES {
                let _ = std::fs::remove_file(&p);
            } else {
                enqueue_finish(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cnx-ev-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// 测试用：默认续接窗口 30 秒，且不跑 ffmpeg、不写真实日志
    fn engine(dir: &Path, pre: u32) -> Engine {
        engine_with_gap(dir, pre, Duration::from_secs(30))
    }

    fn engine_with_gap(dir: &Path, pre: u32, gap: Duration) -> Engine {
        engine_full(dir, pre, gap, Duration::ZERO)
    }

    fn engine_full(dir: &Path, pre: u32, gap: Duration, max_clip: Duration) -> Engine {
        let st = Arc::new(Mutex::new(TapStatus::default()));
        let mut e = Engine::new(dir.to_path_buf(), "测试".into(), pre, gap, max_clip, st);
        e.remux = false;
        e.log_enabled = false;
        e
    }

    #[test]
    fn idle_data_only_fills_the_buffer() {
        let dir = tmp_root("idle");
        let mut e = engine(&dir, 5);
        e.on_data(&[7u8; 1024], false, Instant::now());
        assert!(e.writer.is_none(), "待机时不能创建文件");
        assert_eq!(e.ring.bytes, 1024);
    }

    #[test]
    fn trigger_writes_buffer_first_then_live_data() {
        let dir = tmp_root("trigger");
        let mut e = engine(&dir, 5);
        let now = Instant::now();
        e.on_data(&[1u8; 3000], false, now);
        e.on_data(&[2u8; 3000], false, now);
        e.on_data(&[3u8; 3000], true, now);

        assert!(e.writer.is_some(), "触发后必须有文件在写");
        assert_eq!(e.ring.bytes, 0, "缓冲倒进文件后要清空");
        let path = e.writer.as_ref().unwrap().path.clone();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 9000, "过去 6000 + 当前 3000");
    }

    #[test]
    fn window_end_finalizes_the_clip() {
        let dir = tmp_root("finish");
        let mut e = engine_with_gap(&dir, 5, Duration::ZERO);
        let now = Instant::now();
        e.on_data(&[5u8; 8192], true, now);
        let path = e.writer.as_ref().unwrap().path.clone();
        e.tick(false, now);
        assert!(e.writer.is_none());
        assert!(path.exists(), "收尾后文件应保留");
    }

    #[test]
    fn tiny_clip_is_discarded() {
        let dir = tmp_root("tiny");
        let mut e = engine_with_gap(&dir, 5, Duration::ZERO);
        let now = Instant::now();
        e.on_data(&[5u8; 100], true, now);
        let path = e.writer.as_ref().unwrap().path.clone();
        e.tick(false, now);
        assert!(!path.exists(), "过小的片段应被丢弃");
    }

    #[test]
    fn reconnect_clears_the_buffer() {
        let dir = tmp_root("reconn");
        let mut e = engine(&dir, 5);
        e.on_data(&[9u8; 2048], false, Instant::now());
        assert!(e.ring.bytes > 0);
        e.on_reconnect();
        assert_eq!(e.ring.bytes, 0, "跨连接的字节拼不起来，必须清空");
    }

    #[test]
    fn shutdown_finalizes_open_clip() {
        let dir = tmp_root("shutdown");
        let mut e = engine(&dir, 5);
        e.on_data(&[4u8; 8192], true, Instant::now());
        let path = e.writer.as_ref().unwrap().path.clone();
        e.shutdown();
        assert!(e.writer.is_none());
        assert!(path.exists());
    }

    #[test]
    fn buffer_is_capped_by_age() {
        let mut r = Ring::new(5);
        let now = Instant::now();
        r.push(now - Duration::from_secs(30), vec![0u8; 100]);
        r.push(now, vec![0u8; 50]);
        r.trim(now);
        assert_eq!(r.bytes, 50, "超过 max_age 的块必须裁掉");
    }

    #[test]
    fn buffer_is_capped_by_bytes() {
        let mut r = Ring::new(5);
        let now = Instant::now();
        let big = vec![0u8; BUF_BYTES / 2 + 1];
        r.push(now - Duration::from_millis(10), big.clone());
        r.push(now, big.clone());
        r.push(now, big);
        assert!(r.bytes <= BUF_BYTES);
    }

    #[test]
    fn drain_is_ordered_and_clears() {
        let mut r = Ring::new(5);
        let now = Instant::now();
        r.push(now, vec![1, 2, 3]);
        r.push(now, vec![4, 5]);
        assert_eq!(r.drain(), vec![1, 2, 3, 4, 5]);
        assert_eq!(r.bytes, 0);
        assert!(r.drain().is_empty());
    }


    /// 静止的人（模特、海报、站岗）不触发；动起来才算事件
    #[test]
    fn still_person_never_triggers() {
        assert_eq!(trigger_kind(true, 0.0), None, "完全静止的人不能触发");
        assert_eq!(trigger_kind(true, 0.02), None, "抖动不算动");
        assert_eq!(
            trigger_kind(true, PERSON_SPEED),
            Some(TriggerKind::Person),
            "走起来就该触发"
        );
        // 车比人迟钝：同样的速度，人算动、车不算
        assert_eq!(trigger_kind(false, 0.05), None, "慢速车不触发");
        assert_eq!(trigger_kind(false, VEHICLE_SPEED), Some(TriggerKind::Vehicle));
    }

    /// 人触发的单段到上限后要接着录，不能静默
    #[test]
    fn person_keeps_recording_past_max_clip() {
        let dir = tmp_root("maxclip_person");
        let mut e = engine_full(&dir, 5, Duration::from_secs(30), Duration::from_secs(1));
        e.kind = TriggerKind::Person;
        let t0 = Instant::now();
        e.on_data(&[1u8; 8192], true, t0);

        e.tick(true, t0 + Duration::from_millis(1100));
        assert!(e.writer.is_none(), "到上限仍要收尾，文件不能无限长");
        assert!(e.suppress_until.is_none(), "人触发的不该静默");

        e.on_data(&[2u8; 8192], true, t0 + Duration::from_secs(2));
        assert!(e.writer.is_some(), "应立刻开下一段，中间不停");
    }

    #[test]
    fn max_clip_forces_stop_and_waits_for_target_to_leave() {
        let dir = tmp_root("maxclip");
        // 续接窗口 30 秒、单段上限 1 秒
        let mut e = engine_full(&dir, 5, Duration::from_secs(30), Duration::from_secs(1));
        let t0 = Instant::now();
        e.on_data(&[1u8; 8192], true, t0);
        let path = e.writer.as_ref().unwrap().path.clone();

        // 目标一直在：到点强制收尾并进入静默
        e.tick(true, t0 + Duration::from_millis(1100));
        assert!(e.writer.is_none(), "超过上限应当收尾");
        assert!(e.suppress_until.is_some(), "收尾后应进入静默");
        assert!(path.exists());

        // 静默期间即使还有数据，也不能开新片段
        e.on_data(&[2u8; 8192], true, t0 + Duration::from_secs(2));
        assert!(e.writer.is_none(), "目标没离开就不该再开新片段");

        // 目标离开（active=false）后解除静默
        e.tick(false, t0 + Duration::from_secs(3));
        assert!(e.suppress_until.is_none(), "目标消失后应解除静默");
        e.on_data(&[3u8; 8192], true, t0 + Duration::from_secs(4));
        assert!(e.writer.is_some(), "解除后应能重新开录");
    }

    #[test]
    fn gap_pauses_writing_but_keeps_the_file() {
        let dir = tmp_root("gap");
        let mut e = engine(&dir, 5); // 续接窗口 30 秒
        let t0 = Instant::now();

        e.on_data(&[1u8; 4096], true, t0);
        let path = e.writer.as_ref().unwrap().path.clone();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 4096);

        // 触发结束：文件不关，但不再写——空等的几十秒没有价值
        e.tick(false, t0);
        e.on_data(&[2u8; 4096], false, t0 + Duration::from_secs(1));
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            4096,
            "续接窗口内的空画面不该写进文件"
        );
        assert!(e.writer.is_some(), "文件要留着，供续接");
        assert!(!e.writing, "指示灯应当熄灭");

        // 目标又出现：继续写同一个文件，不另开
        e.tick(true, t0 + Duration::from_secs(2));
        e.on_data(&[3u8; 4096], true, t0 + Duration::from_secs(2));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 8192);
        assert!(e.writing);
    }

    #[test]
    fn gap_expiry_finalizes_once() {
        let dir = tmp_root("gapexp");
        let mut e = engine(&dir, 5);
        let t0 = Instant::now();
        e.on_data(&[7u8; 8192], true, t0);
        let path = e.writer.as_ref().unwrap().path.clone();

        // 30 秒续接窗口（测试里 merge_gap 取 30 秒）
        e.tick(false, t0);
        e.tick(false, t0 + Duration::from_secs(10));
        assert!(e.writer.is_some(), "窗口内不该收尾");

        e.tick(false, t0 + Duration::from_secs(31));
        assert!(e.writer.is_none(), "窗口到期应当收尾");
        assert!(path.exists());
    }

    #[test]
    fn zero_gap_closes_immediately() {
        let dir = tmp_root("gapzero");
        let mut e = engine_with_gap(&dir, 5, Duration::ZERO);
        let t0 = Instant::now();
        e.on_data(&[9u8; 8192], true, t0);
        e.tick(false, t0);
        assert!(e.writer.is_none(), "gap=0 表示不续接，立刻收尾");
    }
}
