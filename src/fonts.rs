//! 字体装配
//!
//! 两个原则：
//! 1. **拉丁字母/数字用系统 UI 字体**（Segoe UI），中文再回退到 CJK 字体。
//!    之前把中文字体插在第 0 位，导致英文和数字也用雅黑渲染，字形偏方、基线偏挤，
//!    是界面"不够精致"的主要来源之一。
//! 2. 字体加载失败不能让程序起不来——任何一步失败都退回 egui 自带字体。

use eframe::egui;
use std::sync::Arc;

/// 拉丁字母与数字：优先系统 UI 字体
fn latin() -> &'static [&'static str] {
    if cfg!(windows) {
        &[r"C:\Windows\Fonts\segoeui.ttf"]
    } else if cfg!(target_os = "macos") {
        &["/System/Library/Fonts/Helvetica.ttc"]
    } else {
        &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        ]
    }
}

/// 等宽：日志、时间戳、参数值
fn mono() -> &'static [&'static str] {
    if cfg!(windows) {
        &[r"C:\Windows\Fonts\consola.ttf"]
    } else if cfg!(target_os = "macos") {
        &["/System/Library/Fonts/Menlo.ttc"]
    } else {
        &["/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"]
    }
}

/// 中文：作为回退字体挂在后面，缺字时才用到
fn cjk() -> &'static [&'static str] {
    if cfg!(windows) {
        &[
            r"C:\Windows\Fonts\Deng.ttf",
            r"C:\Windows\Fonts\msyh.ttc",
            r"C:\Windows\Fonts\simhei.ttf",
            r"C:\Windows\Fonts\simsun.ttc",
        ]
    } else if cfg!(target_os = "macos") {
        &[
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/STHeiti Medium.ttc",
        ]
    } else {
        &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        ]
    }
}

fn load_first(paths: &[&str], key: &str, fonts: &mut egui::FontDefinitions) -> bool {
    for path in paths {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert(key.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
            return true;
        }
    }
    false
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let has_latin = load_first(latin(), "latin", &mut fonts);
    let has_mono = load_first(mono(), "mono", &mut fonts);
    let has_cjk = load_first(cjk(), "cjk", &mut fonts);

    // 比例字体：拉丁优先，中文兜底
    {
        let prop = fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default();
        if has_cjk {
            prop.push("cjk".to_owned());
        }
        if has_latin {
            prop.insert(0, "latin".to_owned());
        }
    }

    // 等宽字体：日志里数字要对齐
    {
        let mono_fam = fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default();
        if has_cjk {
            mono_fam.push("cjk".to_owned());
        }
        if has_mono {
            mono_fam.insert(0, "mono".to_owned());
        }
    }

    ctx.set_fonts(fonts);
}
