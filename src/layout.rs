//! 全屏自由布局的纯计算：限位、吸附、缩放。
//!
//! 单独拆出来是因为**拖拽没法自动测**——本机注入鼠标事件到不了应用。
//! 把这些几何逻辑做成纯函数，至少能保证「拖到边上会吸附」这类行为是对的。

use eframe::egui::{pos2, vec2, Rect, Vec2};

/// 吸附容差（屏幕像素）
pub const SNAP: f32 = 8.0;
/// 单个画块的最小尺寸
pub const MIN_W: f32 = 160.0;
pub const MIN_H: f32 = 90.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
}

/// 当前生效的吸附参考线（画出来给用户看）
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide {
    pub axis: Axis,
    pub at: f32,
}

/// 把矩形限制在画布内；比画布还大时先保证左上对齐
pub fn clamp(rect: Rect, canvas: Rect) -> Rect {
    let w = rect.width().min(canvas.width()).max(1.0);
    let h = rect.height().min(canvas.height()).max(1.0);
    let x = rect
        .left()
        .clamp(canvas.left(), (canvas.right() - w).max(canvas.left()));
    let y = rect
        .top()
        .clamp(canvas.top(), (canvas.bottom() - h).max(canvas.top()));
    Rect::from_min_size(pos2(x, y), vec2(w, h))
}

/// 拖动后吸附。
///
/// 参与吸附的特征线：左/中/右（上/中/下），候选线来自画布本身和**其他画块**。
/// 取偏差最小的那条，其它同等接近的也一并作为参考线返回（方便画多条）。
pub fn snap(
    rect: Rect,
    prev: Rect,
    others: &[Rect],
    canvas: Rect,
    tol: f32,
) -> (Rect, Vec<Guide>) {
    let mut guides = Vec::new();

    // 候选 X 线：画布左中右 + 其它块的左右边（不要中线，太密了）
    let mut xs: Vec<f32> = vec![canvas.left(), canvas.center().x, canvas.right()];
    let mut ys: Vec<f32> = vec![canvas.top(), canvas.center().y, canvas.bottom()];
    for o in others {
        xs.extend([o.left(), o.right()]);
        ys.extend([o.top(), o.bottom()]);
    }

    let dx = enter_delta(
        [rect.left(), rect.center().x, rect.right()],
        [prev.left(), prev.center().x, prev.right()],
        &xs,
        tol,
    );
    let dy = enter_delta(
        [rect.top(), rect.center().y, rect.bottom()],
        [prev.top(), prev.center().y, prev.bottom()],
        &ys,
        tol,
    );

    let mut out = rect.translate(vec2(dx.unwrap_or(0.0), dy.unwrap_or(0.0)));

    if let Some(d) = dx
        && d != 0.0 {
            for f in [out.left(), out.center().x, out.right()] {
                if xs.iter().any(|c| (c - f).abs() < 0.5) {
                    guides.push(Guide { axis: Axis::X, at: f });
                    break;
                }
            }
        }
    if let Some(d) = dy
        && d != 0.0 {
            for f in [out.top(), out.center().y, out.bottom()] {
                if ys.iter().any(|c| (c - f).abs() < 0.5) {
                    guides.push(Guide { axis: Axis::Y, at: f });
                    break;
                }
            }
        }

    out = clamp(out, canvas);
    (out, guides)
}

/// 只在「从区域外进入」时吸附。
///
/// 关键点：如果上一帧这条特征线**已经贴在候选线上**（prev 在容差内），就不要再吸了，
/// 否则画面一旦贴上边就再也拖不动——每帧位移不足 tol 都会被拉回去，
/// 手感就是「粘死在原地」。
fn enter_delta(
    features: [f32; 3],
    prev: [f32; 3],
    cands: &[f32],
    tol: f32,
) -> Option<f32> {
    let mut best: Option<f32> = None;
    for (f, pf) in features.iter().zip(prev.iter()) {
        for c in cands {
            let d = c - f;
            // 这一帧进入容差，且上一帧还在容差外 → 才吸附
            if d.abs() <= tol && (pf - c).abs() > tol
                && best.map(|b: f32| d.abs() < b.abs()).unwrap_or(true) {
                    best = Some(d);
                }
        }
    }
    best
}

/// 在一组特征值里找最近的候选，返回需要的位移
fn best_delta(features: [f32; 3], cands: &[f32], tol: f32) -> Option<f32> {
    let mut best: Option<f32> = None;
    for f in features {
        for c in cands {
            let d = c - f;
            if d.abs() <= tol && best.map(|b| d.abs() < b.abs()).unwrap_or(true) {
                best = Some(d);
            }
        }
    }
    best
}

/// 从右下角手柄缩放：先保证最小尺寸，再吸附右边/下边，最后限位
pub fn resize(
    rect: Rect,
    delta: Vec2,
    others: &[Rect],
    canvas: Rect,
    tol: f32,
) -> Rect {
    let w = (rect.width() + delta.x).max(MIN_W);
    let h = (rect.height() + delta.y).max(MIN_H);
    let mut out = Rect::from_min_size(rect.min, vec2(w, h));

    // 右边 / 下边吸附到画布和其他块
    let mut xs: Vec<f32> = vec![canvas.left(), canvas.center().x, canvas.right()];
    let mut ys: Vec<f32> = vec![canvas.top(), canvas.center().y, canvas.bottom()];
    for o in others {
        xs.extend([o.left(), o.right()]);
        ys.extend([o.top(), o.bottom()]);
    }
    if let Some(d) = best_delta([out.right(), out.right(), out.right()], &xs, tol) {
        out.set_right(out.right() + d);
    }
    if let Some(d) = best_delta([out.bottom(), out.bottom(), out.bottom()], &ys, tol) {
        out.set_bottom(out.bottom() + d);
    }

    let w = out.width().max(MIN_W);
    let h = out.height().max(MIN_H);
    out.set_width(w.min(canvas.right() - out.left()).max(1.0));
    out.set_height(h.min(canvas.bottom() - out.top()).max(1.0));
    clamp(out, canvas)
}

/// 旋转后的显示宽高比。
///
/// 90/270 度会把画面转成竖的，宽高互换——格子如果还是横的，
/// 画面会按新比例居中显示（两侧留黑边），由用户决定要不要「恢复原比例」。
pub fn rotated_aspect(w: f32, h: f32, rotate: u16) -> f32 {
    let (w, h) = (w.max(1.0), h.max(1.0));
    match rotate % 360 {
        90 | 270 => h / w,
        _ => w / h,
    }
}

/// 比例锁定下的缩放：只能等比放大缩小。
///
/// 把两个轴的位移都折算到宽度上，取影响更大的那个定标，
/// 这样横着拖和竖着拖都跟手，不会出现「往右拖没反应」。
pub fn resize_locked(rect: Rect, delta: Vec2, aspect: f32, canvas: Rect) -> Rect {
    let aspect = aspect.max(0.05);
    let dw = delta.x;
    let dh = delta.y * aspect;
    let dw = if dw.abs() >= dh.abs() { dw } else { dh };

    let mut w = (rect.width() + dw).max(MIN_W);
    let mut h = w / aspect;
    if h < MIN_H {
        h = MIN_H;
        w = h * aspect;
    }

    // 越出画布就整体等比缩回来（保持比例，不动左上角）
    let max_w = (canvas.right() - rect.left()).max(1.0);
    let max_h = (canvas.bottom() - rect.top()).max(1.0);
    let k = (max_w / w).min(max_h / h).min(1.0);
    Rect::from_min_size(rect.min, vec2((w * k).max(1.0), (h * k).max(1.0)))
}

/// 恢复成指定比例：保留宽度、以中心为基准调整高度，并保证不出画布。
pub fn fit_aspect(rect: Rect, aspect: f32, canvas: Rect) -> Rect {
    let aspect = aspect.max(0.05);
    let center = rect.center();

    let mut w = rect.width().max(MIN_W);
    let mut h = w / aspect;
    if h < MIN_H {
        h = MIN_H;
        w = h * aspect;
    }

    // 画布放不下就整体缩小
    let k = (canvas.width() / w).min(canvas.height() / h).min(1.0);
    w *= k;
    h *= k;

    clamp(Rect::from_center_size(center, vec2(w, h)), canvas)
}

/// 画面四个角（左上 / 右上 / 右下 / 左下）各自对应的纹理坐标。
///
/// `painter.image()` 只能贴轴对齐矩形，旋转 90/270 度必须自己建网格，
/// 所以这里把映射关系算成纯数据，方便单测。
///
/// 旋转角度按顺时针算。翻转在**显示空间**做，也就是旋转之后再左右/上下翻。
pub fn corner_uvs(rotate: u16, flip_h: bool, flip_v: bool) -> [(f32, f32); 4] {
    // 基准：不旋转时四角就是 (0,0) (1,0) (1,1) (0,1)
    let mut uv = match rotate % 360 {
        90 => [(0.0, 1.0), (0.0, 0.0), (1.0, 0.0), (1.0, 1.0)],
        180 => [(1.0, 1.0), (0.0, 1.0), (0.0, 0.0), (1.0, 0.0)],
        270 => [(1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)],
        _ => [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
    };
    // 左右翻：左上↔右上、左下↔右下
    if flip_h {
        uv.swap(0, 1);
        uv.swap(3, 2);
    }
    // 上下翻：左上↔左下、右上↔右下
    if flip_v {
        uv.swap(0, 3);
        uv.swap(1, 2);
    }
    uv
}

/// 把画布均分成 N 个尽量大的格子，用于「一键布局」。
///
/// 返回 (列数, 每格矩形)。格子是**均分**画布的：整体贴满四边，
/// 画面本身在格内按比例缩（由绘制端负责留黑边），格子之间只留 `gap`。
///
/// 列数在 1..=3 里挑「每格能放下的画面面积最大」的那个——
/// 和视频区网格用同一套判据，所以 1 路就是整屏、2 路并排、4 路 2×2。
pub fn grid_cells(n: usize, canvas: Rect, gap: f32, target: f32) -> (usize, Vec<Rect>) {
    let n = n.max(1);
    let target = target.max(0.05);

    let mut best_cols = 1usize;
    let mut best_area = 0.0f32;
    for cols in 1..=3usize {
        let rows = n.div_ceil(cols);
        if rows > 3 {
            continue;
        }
        let cw = (canvas.width() - gap * (cols as f32 - 1.0)) / cols as f32;
        let ch = (canvas.height() - gap * (rows as f32 - 1.0)) / rows as f32;
        if cw <= 0.0 || ch <= 0.0 {
            continue;
        }
        let w = cw.min(ch * target);
        let area = w * (w / target) * n as f32;
        if area > best_area {
            best_area = area;
            best_cols = cols;
        }
    }

    let cols = best_cols;
    let rows = n.div_ceil(cols);
    let cw = (canvas.width() - gap * (cols as f32 - 1.0)) / cols as f32;
    let ch = (canvas.height() - gap * (rows as f32 - 1.0)) / rows as f32;

    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let r = k / cols;
        let c = k % cols;
        out.push(Rect::from_min_size(
            pos2(
                canvas.left() + c as f32 * (cw + gap),
                canvas.top() + r as f32 * (ch + gap),
            ),
            vec2(cw, ch),
        ));
    }
    (cols, out)
}

/// 判定「黑边」的两个条件，缺一不可。
///
/// 只看亮度是不够的：夜景画面本身就接近全黑，很多列的最大亮度只有 9~12，
/// 按亮度阈值会把画面当成黑边裁掉。真正的黑边还有一个特征——**完全均匀**：
/// 编码器补的黑边极差恒为 0，而画面再暗也有传感器噪点（实测极差 5~207）。
const BAR_MAX_VALUE: u8 = 3;
const BAR_MAX_RANGE: u8 = 3;

/// 黑边左右（或上下）宽度允许的不对称比例。
/// 超过就说明检测很可能吃进了画面内容，宁可不裁也不要裁错。
const BAR_MAX_ASYMMETRY: f32 = 0.02;

/// 在 RGB24 帧里找出内容区，返回归一化的 `[x, y, w, h]`。
///
/// 用于裁掉推流端塞进来的黑边（手机竖屏塞进 16:9 是最常见的场景）。
/// 判据：某一列/行**又暗又均匀**才算黑边，其余都算画面。
///
/// 以下情况返回 None（不裁）：
/// - 整帧都是黑边（没画面）
/// - 内容几乎占满（没有黑边可裁）
/// - 内容小得离谱（小于 5%，多半是噪点或误判）
pub fn content_box(data: &[u8], w: usize, h: usize) -> Option<[f32; 4]> {
    if w == 0 || h == 0 || data.len() < w * h * 3 {
        return None;
    }
    // 降采样扫描：1920×1080 全扫是 600 万次，这里压到约 13 万次
    let step_x = (w / 480).max(1);
    let step_y = (h / 270).max(1);
    let cols = w.div_ceil(step_x);
    let rows = h.div_ceil(step_y);

    let mut col_min = vec![255u8; cols];
    let mut col_max = vec![0u8; cols];
    let mut row_min = vec![255u8; rows];
    let mut row_max = vec![0u8; rows];

    let mut y = 0;
    let mut ri = 0;
    while y < h && ri < rows {
        let mut x = 0;
        let mut ci = 0;
        while x < w && ci < cols {
            let o = (y * w + x) * 3;
            let v = data[o].max(data[o + 1]).max(data[o + 2]);
            if v < col_min[ci] {
                col_min[ci] = v;
            }
            if v > col_max[ci] {
                col_max[ci] = v;
            }
            if v < row_min[ri] {
                row_min[ri] = v;
            }
            if v > row_max[ri] {
                row_max[ri] = v;
            }
            x += step_x;
            ci += 1;
        }
        y += step_y;
        ri += 1;
    }

    // 又暗又均匀 = 黑边
    let is_bar_col = |i: usize| col_max[i] <= BAR_MAX_VALUE && col_max[i] - col_min[i] <= BAR_MAX_RANGE;
    let is_bar_row = |i: usize| row_max[i] <= BAR_MAX_VALUE && row_max[i] - row_min[i] <= BAR_MAX_RANGE;

    // 整帧全黑时 find 返回 None，直接放弃裁剪
    let first_c = (0..cols).find(|i| !is_bar_col(*i))?;
    let last_c = (0..cols).rev().find(|i| !is_bar_col(*i))?;
    let first_r = (0..rows).find(|i| !is_bar_row(*i))?;
    let last_r = (0..rows).rev().find(|i| !is_bar_row(*i))?;

    let left = first_c * step_x;
    let right = ((last_c + 1) * step_x).min(w);
    let top = first_r * step_y;
    let bottom = ((last_r + 1) * step_y).min(h);

    // 对称性检查：编码器补的黑边一定左右（上下）等宽。
    // 不等就说明检测多半已经吃进画面，直接放弃裁剪。
    let asym_x = (left as f32 - (w - right) as f32).abs() / w as f32;
    let asym_y = (top as f32 - (h - bottom) as f32).abs() / h as f32;
    if asym_x > BAR_MAX_ASYMMETRY || asym_y > BAR_MAX_ASYMMETRY {
        return None;
    }

    let bw = (right - left) as f32 / w as f32;
    let bh = (bottom - top) as f32 / h as f32;

    // 几乎占满 = 没有黑边可裁
    if bw > 0.98 && bh > 0.98 {
        return None;
    }
    // 太小的框多半是噪点，别把画面裁没了
    if bw < 0.05 || bh < 0.05 {
        return None;
    }

    Some([left as f32 / w as f32, top as f32 / h as f32, bw, bh])
}

/// 归一化（0..1，相对画布），用于写进配置
pub fn to_norm(rect: Rect, canvas: Rect) -> [f32; 4] {
    let w = canvas.width().max(1.0);
    let h = canvas.height().max(1.0);
    [
        (rect.left() - canvas.left()) / w,
        (rect.top() - canvas.top()) / h,
        rect.width() / w,
        rect.height() / h,
    ]
}

/// 从配置还原成屏幕坐标
pub fn from_norm(n: [f32; 4], canvas: Rect) -> Rect {
    let w = canvas.width().max(1.0);
    let h = canvas.height().max(1.0);
    Rect::from_min_size(
        pos2(canvas.left() + n[0] * w, canvas.top() + n[1] * h),
        vec2((n[2] * w).max(1.0), (n[3] * h).max(1.0)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 900.0))
    }

    #[test]
    fn clamps_inside_canvas() {
        let c = canvas();
        let r = Rect::from_min_size(pos2(-50.0, -20.0), vec2(400.0, 300.0));
        let out = clamp(r, c);
        assert_eq!(out.left(), 0.0);
        assert_eq!(out.top(), 0.0);

        let r2 = Rect::from_min_size(pos2(1500.0, 850.0), vec2(400.0, 300.0));
        let out2 = clamp(r2, c);
        assert!(out2.right() <= c.right() + 0.01);
        assert!(out2.bottom() <= c.bottom() + 0.01);
    }

    #[test]
    fn snaps_to_left_edge() {
        let c = canvas();
        // 上一帧在容差外（x=30），这一帧进到 5px → 应贴到 0
        let prev = Rect::from_min_size(pos2(30.0, 100.0), vec2(400.0, 300.0));
        let r = Rect::from_min_size(pos2(5.0, 100.0), vec2(400.0, 300.0));
        let (out, guides) = snap(r, prev, &[], c, SNAP);
        assert_eq!(out.left(), 0.0);
        assert!(guides.iter().any(|g| g.axis == Axis::X && g.at == 0.0));
    }

    /// 回归：已经贴边的画面必须能被拖走。
    /// 之前每帧都重新吸附，位移不足容差就被拉回去，手感像粘死。
    #[test]
    fn does_not_stick_once_snapped() {
        let c = canvas();
        let prev = Rect::from_min_size(pos2(0.0, 100.0), vec2(400.0, 300.0));
        let now = Rect::from_min_size(pos2(5.0, 100.0), vec2(400.0, 300.0));
        let (out, guides) = snap(now, prev, &[], c, SNAP);
        assert_eq!(out.left(), 5.0, "贴边之后小位移不该再被吸回去");
        assert!(guides.is_empty());
    }

    #[test]
    fn does_not_snap_beyond_tolerance() {
        let c = canvas();
        let prev = Rect::from_min_size(pos2(60.0, 100.0), vec2(400.0, 300.0));
        let r = Rect::from_min_size(pos2(40.0, 100.0), vec2(400.0, 300.0));
        let (out, guides) = snap(r, prev, &[], c, SNAP);
        assert_eq!(out.left(), 40.0);
        assert!(guides.is_empty());
    }

    #[test]
    fn snaps_center_of_canvas() {
        let c = canvas(); // 中线 x = 800
        // 块中心 795 → 距中线 5px（容差内），上一帧在 760（容差外）
        let prev = Rect::from_min_size(pos2(560.0, 100.0), vec2(400.0, 200.0));
        let r = Rect::from_min_size(pos2(595.0, 100.0), vec2(400.0, 200.0));
        let (out, _) = snap(r, prev, &[], c, SNAP);
        assert!((out.center().x - 800.0).abs() < 0.01, "中心应对齐画布中线");
    }

    #[test]
    fn snaps_to_neighbour_edge() {
        let c = canvas();
        let other = Rect::from_min_size(pos2(500.0, 100.0), vec2(400.0, 300.0));
        // 我这块右边 503，邻居左边 500；上一帧右边在 480（容差外）
        let prev = Rect::from_min_size(pos2(80.0, 100.0), vec2(400.0, 300.0));
        let me = Rect::from_min_size(pos2(103.0, 100.0), vec2(400.0, 300.0));
        let (out, guides) = snap(me, prev, &[other], c, SNAP);
        assert!((out.right() - 500.0).abs() < 0.01, "应吸到邻居左边");
        assert!(guides.iter().any(|g| g.axis == Axis::X && (g.at - 500.0).abs() < 0.01));
    }

    #[test]
    fn snap_never_leaves_canvas() {
        let c = canvas();
        let prev = Rect::from_min_size(pos2(-500.0, -500.0), vec2(400.0, 300.0));
        let r = Rect::from_min_size(pos2(-500.0, -500.0), vec2(400.0, 300.0));
        let (out, _) = snap(r, prev, &[], c, SNAP);
        assert!(out.left() >= c.left() - 0.01);
        assert!(out.top() >= c.top() - 0.01);
    }

    #[test]
    fn resize_keeps_min_size() {
        let c = canvas();
        let r = Rect::from_min_size(pos2(100.0, 100.0), vec2(400.0, 300.0));
        let out = resize(r, vec2(-9999.0, -9999.0), &[], c, SNAP);
        assert!(out.width() >= MIN_W);
        assert!(out.height() >= MIN_H);
    }

    #[test]
    fn resize_snaps_to_canvas_right() {
        let c = canvas();
        let r = Rect::from_min_size(pos2(100.0, 100.0), vec2(1494.0, 300.0)); // right = 1594，容差内
        let out = resize(r, vec2(0.0, 0.0), &[], c, SNAP);
        assert!((out.right() - c.right()).abs() < 0.01, "右边应吸到画布右边");
    }


    #[test]
    fn rotated_aspect_swaps_on_quarter_turns() {
        let (w, h) = (2560.0, 1440.0);
        assert!((rotated_aspect(w, h, 0) - 16.0 / 9.0).abs() < 0.001);
        assert!((rotated_aspect(w, h, 180) - 16.0 / 9.0).abs() < 0.001);
        assert!((rotated_aspect(w, h, 90) - 9.0 / 16.0).abs() < 0.001);
        assert!((rotated_aspect(w, h, 270) - 9.0 / 16.0).abs() < 0.001);
        // 非法角度按 0 处理
        assert!((rotated_aspect(w, h, 45) - 16.0 / 9.0).abs() < 0.001);
    }

    #[test]
    fn locked_resize_keeps_aspect() {
        let c = canvas();
        let r = Rect::from_min_size(pos2(100.0, 100.0), vec2(640.0, 360.0));
        let aspect = 16.0 / 9.0;
        for d in [
            vec2(80.0, 0.0),
            vec2(0.0, 80.0),
            vec2(50.0, -30.0),
            vec2(-100.0, 0.0),
        ] {
            let out = resize_locked(r, d, aspect, c);
            assert!(
                (out.width() / out.height() - aspect).abs() < 0.01,
                "delta {d:?} 把比例拉成了 {}",
                out.width() / out.height()
            );
        }
    }

    #[test]
    fn locked_resize_respects_min_size() {
        let c = canvas();
        let r = Rect::from_min_size(pos2(100.0, 100.0), vec2(640.0, 360.0));
        let out = resize_locked(r, vec2(-9999.0, -9999.0), 16.0 / 9.0, c);
        assert!(out.width() >= MIN_W - 0.01);
        assert!(out.height() >= MIN_H - 0.01);
    }

    #[test]
    fn locked_resize_stays_inside_canvas() {
        let c = canvas();
        let r = Rect::from_min_size(pos2(1200.0, 600.0), vec2(320.0, 180.0));
        let out = resize_locked(r, vec2(4000.0, 4000.0), 16.0 / 9.0, c);
        assert!(out.right() <= c.right() + 0.01, "右边越界");
        assert!(out.bottom() <= c.bottom() + 0.01, "下边越界");
        assert!((out.width() / out.height() - 16.0 / 9.0).abs() < 0.01);
    }

    #[test]
    fn fit_aspect_produces_requested_ratio() {
        let c = canvas();
        let r = Rect::from_min_size(pos2(300.0, 100.0), vec2(300.0, 700.0));
        let out = fit_aspect(r, 16.0 / 9.0, c);
        assert!((out.width() / out.height() - 16.0 / 9.0).abs() < 0.01);
        assert!(out.left() >= c.left() - 0.01 && out.right() <= c.right() + 0.01);
        assert!(out.top() >= c.top() - 0.01 && out.bottom() <= c.bottom() + 0.01);
    }

    #[test]
    fn fit_aspect_keeps_center() {
        let c = canvas();
        let r = Rect::from_min_size(pos2(300.0, 100.0), vec2(300.0, 700.0));
        let before = r.center();
        let out = fit_aspect(r, 16.0 / 9.0, c);
        assert!((out.center().x - before.x).abs() < 0.01);
        assert!((out.center().y - before.y).abs() < 0.01);
    }


    #[test]
    fn corner_uvs_identity() {
        assert_eq!(
            corner_uvs(0, false, false),
            [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
        );
    }

    #[test]
    fn corner_uvs_rotations() {
        // 90 度：画面左上角应出现在格子的右上角
        assert_eq!(
            corner_uvs(90, false, false),
            [(0.0, 1.0), (0.0, 0.0), (1.0, 0.0), (1.0, 1.0)]
        );
        // 180 度：纹理左上角应落到格子右下角（写成镜像表就会翻车）
        assert_eq!(
            corner_uvs(180, false, false),
            [(1.0, 1.0), (0.0, 1.0), (0.0, 0.0), (1.0, 0.0)]
        );
        assert_eq!(
            corner_uvs(270, false, false),
            [(1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)]
        );
        // 非法角度等同不旋转
        assert_eq!(corner_uvs(45, false, false), corner_uvs(0, false, false));
    }

    #[test]
    fn corner_uvs_flips() {
        // 左右翻 = 交换左列和右列
        assert_eq!(
            corner_uvs(0, true, false),
            [(1.0, 0.0), (0.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
        );
        // 上下翻 = 交换上排和下排
        assert_eq!(
            corner_uvs(0, false, true),
            [(0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)]
        );
        // 同时翻 = 180 度旋转的效果
        assert_eq!(corner_uvs(0, true, true), corner_uvs(180, false, false));
    }


    #[test]
    fn grid_cells_single_fills_canvas() {
        let c = canvas();
        let (cols, cells) = grid_cells(1, c, 0.0, 16.0 / 9.0);
        assert_eq!(cols, 1);
        assert_eq!(cells.len(), 1);
        assert!((cells[0].width() - c.width()).abs() < 0.01);
        assert!((cells[0].height() - c.height()).abs() < 0.01);
    }

    #[test]
    fn grid_cells_two_side_by_side() {
        let c = canvas(); // 1600×900
        let (cols, cells) = grid_cells(2, c, 4.0, 16.0 / 9.0);
        assert_eq!(cols, 2, "16:9 画布上两路应该并排");
        assert_eq!(cells.len(), 2);
        // 贴满四边：左边贴左、右边贴右
        assert!((cells[0].left() - c.left()).abs() < 0.01);
        assert!((cells[1].right() - c.right()).abs() < 0.01);
        // 上下也贴满
        assert!((cells[0].top() - c.top()).abs() < 0.01);
        assert!((cells[0].bottom() - c.bottom()).abs() < 0.01);
    }

    #[test]
    fn grid_cells_four_is_two_by_two() {
        let c = canvas();
        let (cols, cells) = grid_cells(4, c, 4.0, 16.0 / 9.0);
        assert_eq!(cols, 2);
        assert_eq!(cells.len(), 4);
        assert!((cells[3].right() - c.right()).abs() < 0.01);
        assert!((cells[3].bottom() - c.bottom()).abs() < 0.01);
    }

    #[test]
    fn grid_cells_never_overlap() {
        let c = canvas();
        for n in 1..=6 {
            let (_, cells) = grid_cells(n, c, 4.0, 16.0 / 9.0);
            for (i, a) in cells.iter().enumerate() {
                for b in cells.iter().skip(i + 1) {
                    assert!(!a.intersects(*b), "n={n} 时格子重叠");
                }
            }
        }
    }


    /// 造一张 w×h 的帧，中间放一块亮区，用来验证内容区检测
    fn frame_with_box(w: usize, h: usize, bx: usize, by: usize, bw: usize, bh: usize) -> Vec<u8> {
        let mut d = vec![0u8; w * h * 3];
        for y in by..(by + bh).min(h) {
            for x in bx..(bx + bw).min(w) {
                let o = (y * w + x) * 3;
                d[o] = 200;
                d[o + 1] = 200;
                d[o + 2] = 200;
            }
        }
        d
    }

    #[test]
    fn content_box_finds_pillarboxed_phone() {
        // 模拟手机竖屏：1920×1080 里中间 608 宽的竖条
        let (w, h) = (1920, 1080);
        let d = frame_with_box(w, h, 656, 0, 608, h);
        let b = content_box(&d, w, h).expect("应该检测出内容区");
        assert!((b[0] - 656.0 / 1920.0).abs() < 0.01, "左边不对: {}", b[0]);
        assert!((b[2] - 608.0 / 1920.0).abs() < 0.02, "宽度不对: {}", b[2]);
        assert!((b[3] - 1.0).abs() < 0.02, "高度应占满");
    }

    #[test]
    fn content_box_finds_letterboxed() {
        let (w, h) = (1280, 720);
        let d = frame_with_box(w, h, 0, 90, w, 540);
        let b = content_box(&d, w, h).expect("应该检测出内容区");
        assert!((b[1] - 90.0 / 720.0).abs() < 0.02, "上边不对: {}", b[1]);
        assert!((b[2] - 1.0).abs() < 0.02, "宽度应占满");
    }

    #[test]
    fn content_box_ignores_full_frame() {
        let (w, h) = (640, 360);
        let d = vec![120u8; w * h * 3];
        assert!(content_box(&d, w, h).is_none(), "占满时不该裁");
    }

    #[test]
    fn content_box_ignores_all_black() {
        let (w, h) = (640, 360);
        let d = vec![0u8; w * h * 3];
        assert!(content_box(&d, w, h).is_none(), "全黑时不该裁");
    }

    #[test]
    fn content_box_rejects_tiny_speck() {
        let (w, h) = (1920, 1080);
        // 一个 3×3 的亮点，占不到 5%
        let d = frame_with_box(w, h, 900, 500, 3, 3);
        assert!(content_box(&d, w, h).is_none(), "噪点不该触发裁剪");
    }


    #[test]
    fn content_box_handles_night_scene() {
        // 夜景：画面本身接近全黑，只有零星亮窗。
        // 旧判据（够亮才算画面）会把暗部当黑边裁掉；新判据看「是否均匀」。
        let (w, h) = (1600, 900);
        let mut d = vec![0u8; w * h * 3];
        // 内容区 x 500..1100：整体亮度 6，另有几个亮点
        for y in 0..h {
            for x in 500..1100 {
                let o = (y * w + x) * 3;
                d[o] = 6;
                d[o + 1] = 6;
                d[o + 2] = 6;
            }
        }
        for (x, y) in [(700, 300), (800, 500), (900, 700)] {
            let o = (y * w + x) * 3;
            d[o] = 200;
            d[o + 1] = 200;
            d[o + 2] = 200;
        }
        let b = content_box(&d, w, h).expect("夜景也应该能找出内容区");
        assert!((b[0] - 500.0 / 1600.0).abs() < 0.01, "左边不对: {}", b[0]);
        assert!((b[2] - 600.0 / 1600.0).abs() < 0.02, "宽度不对: {}", b[2]);
    }

    #[test]
    fn content_box_tolerates_slightly_noisy_bars() {
        // 黑边可能带极轻微噪声（编码振铃），只要不超范围仍应判为黑边
        let (w, h) = (1600, 900);
        let mut d = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..500 {
                let o = (y * w + x) * 3;
                d[o] = (x % 4) as u8; // 0..3 的抖动
            }
            for x in 500..1100 {
                let o = (y * w + x) * 3;
                d[o] = 180;
            }
        }
        let b = content_box(&d, w, h).expect("应找出内容区");
        assert!((b[0] - 500.0 / 1600.0).abs() < 0.01, "抖动黑边没被忽略: {}", b[0]);
    }

    #[test]
    fn content_box_handles_short_buffer() {
        let d = vec![255u8; 10]; // 数据不够一帧
        assert!(content_box(&d, 640, 360).is_none());
    }

    #[test]
    fn norm_roundtrip() {
        let c = canvas();
        let r = Rect::from_min_size(pos2(160.0, 90.0), vec2(800.0, 450.0));
        let n = to_norm(r, c);
        let back = from_norm(n, c);
        assert!((back.left() - r.left()).abs() < 0.01);
        assert!((back.width() - r.width()).abs() < 0.01);
    }
}
