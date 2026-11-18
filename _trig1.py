"""触发规则改为「在动才算事件」，静默只对车辆生效。"""

# ---------- 1) 跟踪器：记录最有代表性的一条运动（位移与尺寸变化取大） ----------
P = "src/detect.rs"
t = open(P, encoding="utf-8").read()


def sub(old, new, label=""):
    global t
    assert old in t, f"{label} 未找到：{old[:70]}"
    assert t.count(old) == 1, f"{label} 多处"
    t = t.replace(old, new, 1)


sub("""pub struct Tracker {
    tracks: Vec<Track>,""",
    """pub struct Tracker {
    tracks: Vec<Track>,
    /// 本轮最"像在动"的那条轨迹的类别与幅度（归一化宽度/秒）。
    /// 幅度取「中心位移」与「尺寸变化率」的较大值：朝镜头走过来时中心几乎不动，
    /// 只看位移会把人判成静止。
    last_motion: f32,
    last_label: &'static str,""", "字段")

sub("""        Self {
            tracks: Vec::new(),
            last_speed: 0.0,
        }""",
    """        Self {
            tracks: Vec::new(),
            last_speed: 0.0,
            last_motion: 0.0,
            last_label: "",
        }""", "初始化")

sub("""        self.last_speed = 0.0;
        let mut used = vec![false; self.tracks.len()];""",
    """        self.last_speed = 0.0;
        self.last_motion = 0.0;
        self.last_label = "";
        let mut used = vec![false; self.tracks.len()];""", "重置")

sub("""                        self.last_speed = self.last_speed.max(v);
                        if v > SPEED_EPS || scale_rate > SCALE_EPS {""",
    """                        self.last_speed = self.last_speed.max(v);
                        let m = v.max(scale_rate);
                        if m > self.last_motion {
                            self.last_motion = m;
                            self.last_label = t.label;
                        }
                        if v > SPEED_EPS || scale_rate > SCALE_EPS {""", "记录运动")

sub("""    /// 该通道最近一轮的最大运动速度（归一化宽度/秒）
    pub fn speed(&self, idx: usize) -> f32 {""",
    """    /// 该通道最近一轮「最像在动」的目标：(类别, 运动幅度)。
    ///
    /// 幅度是位移与尺寸变化率取大，朝镜头走也能算出来。
    /// 事件录制的触发行数就靠它——静止的模特、站岗的人幅度接近 0。
    pub fn motion(&self, idx: usize) -> Option<(&'static str, f32)> {
        let (t, _) = self.map.get(&idx)?;
        if t.last_label.is_empty() {
            return None;
        }
        Some((t.last_label, t.last_motion))
    }

    /// 该通道最近一轮的最大运动速度（归一化宽度/秒）
    pub fn speed(&self, idx: usize) -> f32 {""", "motion 方法")

open(P, "w", encoding="utf-8").write(t)
print("detect.rs 已改")

# ---------- 2) eventrec：触发带类别，静默只对车 ----------
P = "src/eventrec.rs"
t = open(P, encoding="utf-8").read()


def sub2(old, new, label=""):
    global t
    assert old in t, f"{label} 未找到：{old[:70]}"
    assert t.count(old) == 1, f"{label} 多处"
    t = t.replace(old, new, 1)


sub2("""pub struct Tap {
    /// 触发截止时刻（epoch 毫秒）；0 = 没触发。用原子量代替锁，热路径零竞争。
    trigger_ms: Arc<AtomicU64>,""",
    """/// 触发者类别。只影响一件事：单段到上限后要不要静默。
///
/// 车能停一整晚，不静默就会录到天亮；人一直在动说明确实有事，
/// 没有理由停，直接接着录下一段。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TriggerKind {
    Person,
    Vehicle,
}

/// 触发阈值（归一化宽度/秒）。
///
/// 人比车敏感一点：人走路的速度本来就慢，而静止目标的抖动大约 1–2%/秒，
/// 4% 留了余量。车辆必须动得更明显，否则停着的车框边缘抖动就能触发。
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
    kind: Arc<AtomicU8>,""", "结构")

sub2("""    /// 把触发窗口延长到 `now + post`。连续触发会不断续期。
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

sub2("""        Self {
            trigger_ms: Arc::new(AtomicU64::new(0)),""",
    """        Self {
            kind: Arc::new(AtomicU8::new(TriggerKind::Vehicle as u8)),
            trigger_ms: Arc::new(AtomicU64::new(0)),""", "Tap 初始化")

sub2("""    trigger_ms: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,""",
    """    trigger_ms: Arc<AtomicU64>,
    kind: Arc<AtomicU8>,
    stop: Arc<AtomicBool>,""", "TapCtx 字段")

sub2("""        let mut ctx = TapCtx {
            trigger_ms: Arc::new(AtomicU64::new(0)),""",
    """        let mut ctx = TapCtx {
            kind: Arc::new(AtomicU8::new(TriggerKind::Vehicle as u8)),
            trigger_ms: Arc::new(AtomicU64::new(0)),""", "TapCtx 初始化")

open(P, "w", encoding="utf-8").write(t)
print("eventrec.rs 结构已改")
