//! 音量图标：把 SVG 栅格化成白色纹理
//!
//! 采用「渲染 alpha → 统一填白」的方式，不解析 SVG 里的填充色，
//! 这样换任何单色图标都能直接用，且能在暗背景上保持可见。

use eframe::egui;

/// 程序图标（窗口 / 任务栏 / Alt+Tab）。
///
/// exe 里已经通过 versioninfo.rc 嵌了图标，那管的是**资源管理器**里显示的那个；
/// 窗口标题栏和任务栏用的是 `WM_SETICON`，winit 默认会给一个自己的图标，
/// 所以必须显式把同一份 ICO 交给它，两处才一致。
///
/// ICO 里 128/256 是 PNG 压缩的（Windows 资源需要），64 及以下是无损 DIB。
/// 这里不解 PNG，只取最大的 DIB（64×64）——任务栏 32、Alt+Tab 48 缩下来够用。
pub fn app_icon() -> Option<egui::IconData> {
    let ico = include_bytes!("../assets/camnexus.ico");
    let count = u16::from_le_bytes([*ico.get(4)?, *ico.get(5)?]) as usize;
    let mut best: Option<(u32, egui::IconData)> = None;

    for i in 0..count {
        let off = 6 + i * 16;
        let e = ico.get(off..off + 16)?;
        let size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let start = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
        let data = ico.get(start..start + size)?;
        // 跳过 PNG 项（\x89PNG）
        if data.starts_with(&[0x89, b'P', b'N', b'G']) {
            continue;
        }
        let Some(img) = dib_to_rgba(data) else {
            continue;
        };
        if best.as_ref().is_none_or(|(w, _)| img.width > *w) {
            let w = img.width;
            best = Some((w, img));
        }
    }
    best.map(|(_, img)| img)
}

/// BITMAPINFOHEADER + 自底向上 BGRA + AND 掩码 → egui 的 RGBA（自顶向下）
fn dib_to_rgba(b: &[u8]) -> Option<egui::IconData> {
    let hdr = u32::from_le_bytes(b.get(0..4)?.try_into().ok()?) as usize;
    let w = i32::from_le_bytes(b.get(4..8)?.try_into().ok()?);
    let h = i32::from_le_bytes(b.get(8..12)?.try_into().ok()?) / 2; // 高度是两倍（XOR+AND）
    let bpp = u16::from_le_bytes(b.get(14..16)?.try_into().ok()?);
    if w <= 0 || h <= 0 || bpp != 32 || hdr < 40 {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    let px = b.get(hdr..)?;
    if px.len() < w * h * 4 {
        return None;
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in (0..h).rev() {
        let row = y * w * 4;
        for x in 0..w {
            let i = row + x * 4;
            rgba.extend_from_slice(&[px[i + 2], px[i + 1], px[i], px[i + 3]]);
        }
    }
    Some(egui::IconData {
        rgba,
        width: w as u32,
        height: h as u32,
    })
}

pub struct VolumeIcons {
    pub on: egui::TextureHandle,
    pub muted: egui::TextureHandle,
    /// 录制图标（单色 alpha，绘制时再着色：白=未录，红=录制中）
    pub record: egui::TextureHandle,
    /// 进入全屏
    pub fullscreen: egui::TextureHandle,
    /// 退出全屏
    pub fullscreen_exit: egui::TextureHandle,
}

impl VolumeIcons {
    pub fn load(ctx: &egui::Context) -> Self {
        Self {
            on: rasterize(
                ctx,
                include_str!("../assets/volume_on.svg"),
                "icon_volume_on",
            ),
            muted: rasterize(
                ctx,
                include_str!("../assets/volume_muted.svg"),
                "icon_volume_muted",
            ),
            record: rasterize(ctx, include_str!("../assets/record.svg"), "icon_record"),
            fullscreen: rasterize(
                ctx,
                include_str!("../assets/fullscreen.svg"),
                "icon_fullscreen",
            ),
            fullscreen_exit: rasterize(
                ctx,
                include_str!("../assets/fullscreen_exit.svg"),
                "icon_fullscreen_exit",
            ),
        }
    }
}

const PX: u32 = 72;

fn rasterize(ctx: &egui::Context, svg: &str, name: &str) -> egui::TextureHandle {
    let options = resvg::usvg::Options::default();
    let tree = match resvg::usvg::Tree::from_str(svg, &options) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("图标解析失败 {name}: {e}");
            return blank(ctx, name);
        }
    };

    let mut pixmap = match resvg::tiny_skia::Pixmap::new(PX, PX) {
        Some(p) => p,
        None => return blank(ctx, name),
    };

    let w = tree.size().width().max(1.0);
    let h = tree.size().height().max(1.0);
    // 等比缩放到 PX × PX，居中
    let scale = (PX as f32 / w).min(PX as f32 / h);
    let dx = (PX as f32 - w * scale) * 0.5;
    let dy = (PX as f32 - h * scale) * 0.5;
    let transform = resvg::tiny_skia::Transform::from_translate(dx, dy).pre_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // 只取 alpha，RGB 统一填白：任何单色图标都能着色
    let mut rgba = Vec::with_capacity((PX * PX * 4) as usize);
    for px in pixmap.pixels() {
        rgba.extend_from_slice(&[255, 255, 255, px.alpha()]);
    }
    let image = egui::ColorImage::from_rgba_unmultiplied([PX as usize, PX as usize], &rgba);
    ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
}

fn blank(ctx: &egui::Context, name: &str) -> egui::TextureHandle {
    let image = egui::ColorImage::filled([1, 1], egui::Color32::TRANSPARENT);
    ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
}
