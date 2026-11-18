"""eventrec：触发带类别，单段上限后只有车静默。"""

P = "src/eventrec.rs"
t = open(P, encoding="utf-8").read()


def sub(old, new, label=""):
    global t
    assert old in t, f"{label} 未找到：{old[:70]}"
    assert t.count(old) == 1, f"{label} 多处"
    t = t.replace(old, new, 1)


sub("use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};",
    "use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};", "导入")

sub("""pub struct Tap {""",
    """/// 触发者类别。只影响一件事：单段到上限后要不要静默。
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

pub struct Tap {""", "枚举与阈值")

sub("""    trigger_ms: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<TapStatus>>,
}""",
    """    trigger_ms: Arc<AtomicU64>,
    /// 最近一次触发是谁引起的（0 = 人，1 = 车）
    kind: Arc<AtomicU8>,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<TapStatus>>,
}""", "Tap 字段")

sub("""    /// 把触发窗口延长到 `now + post`。连续触发会不断续期。
    pub fn trigger(&self, post: Duration) {
        let until = epoch_ms() + post.as_millis() as u64;
        self.trigger_ms.fetch_max(until, Ordering::SeqCst);
    }""",
    """    /// 把触发窗口延长到 `now + post`。连续触发会不断续期。
    pub fn trigger(&self, post: Duration, kind: TriggerKind) {
        let until = epoch_ms() + post.as_millis() as u64;
        self.trigger_ms.fetch_max(until, Ordering::SeqCst);
        self.kind.store(kind as u8, Ordering::SeqCst);
    }""", "trigger")

# Ctx（共享给线程的状态）
sub("""struct Ctx {
    spec: TapSpec,
    port: u16,
    trigger_ms: Arc<AtomicU64>,""",
    """struct Ctx {
    spec: TapSpec,
    port: u16,
    trigger_ms: Arc<AtomicU64>,
    kind: Arc<AtomicU8>,""", "Ctx 字段")

sub("""        port,
        trigger_ms: Arc::new(AtomicU64::new(0)),""",
    """        port,
        trigger_ms: Arc::new(AtomicU64::new(0)),
        kind: Arc::new(AtomicU8::new(TriggerKind::Vehicle as u8)),""", "Ctx 初始化")

sub("""    let tap = Tap {
        trigger_ms: ctx.trigger_ms.clone(),""",
    """    let tap = Tap {
        trigger_ms: ctx.trigger_ms.clone(),
        kind: ctx.kind.clone(),""", "Tap 初始化")

# 引擎：记住类别
sub("""    /// 测试里关掉，避免真的起 ffmpeg
    remux: bool,""",
    """    /// 最近一次触发的来源，只用来决定单段到上限后要不要静默
    kind: TriggerKind,
    /// 测试里关掉，避免真的起 ffmpeg
    remux: bool,""", "Engine 字段")

sub("""            suppress_until: None,
            remux: true,""",
    """            suppress_until: None,
            kind: TriggerKind::Vehicle,
            remux: true,""", "Engine 初始化")

# 单段上限：只有车才静默
sub("""            if too_long {
                if let Some(clip) = self.writer.take() {
                    let mins = self.max_clip.as_secs() / 60;
                    self.log(&format!("{}（{mins} 分钟）", tr("单段已达上限，暂停直到目标离开")));
                    self.finish(clip);
                }
                // 静默一个上限时长后重新武装
                self.suppress_until = Some(now + self.max_clip.max(Duration::from_secs(60)));
            }""",
    """            if too_long {
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
            }""", "上限处理")

# 线程里把类别同步进引擎
sub("""        engine.tick(active, Instant::now());""",
    """        let kind = if ctx.kind.load(Ordering::SeqCst) == TriggerKind::Person as u8 {
            TriggerKind::Person
        } else {
            TriggerKind::Vehicle
        };
        engine.kind = kind;
        engine.tick(active, Instant::now());""", "同步类别")

open(P, "w", encoding="utf-8").write(t)
print("eventrec.rs 已改")
