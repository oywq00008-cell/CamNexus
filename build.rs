//! 把 `versioninfo.rc`（图标 + 版本信息）编译成 `.res` 并随 exe 链接。
//!
//! 走的是 Windows SDK 官方 `rc.exe`，而不是自己拼 PE 资源结构——
//! 本机 SDK 装在 `H:\Windows Kits\10`，位置从注册表 `KitsRoot10` 读，
//! 找不到时只告警不失败（exe 会没有图标，但能正常构建）。

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let root = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
    );
    let rc_file = root.join("versioninfo.rc");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", rc_file.display());
    println!("cargo:rerun-if-changed=assets/camnexus.ico");

    let cargo_ver = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    check_rc_version(&rc_file, &cargo_ver);

    let Some(rc) = find_rc() else {
        println!("cargo:warning=未找到 rc.exe（Windows SDK），exe 将没有图标和版本信息");
        return;
    };

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("camnexus.res");
    let status = Command::new(&rc)
        .current_dir(&root)
        .arg("/nologo")
        .arg(format!("/fo{}", out.display()))
        .arg(&rc_file)
        .status();

    match status {
        Ok(s) if s.success() && out.exists() => {
            println!("cargo:rustc-link-arg-bins={}", out.display());
            // 成功不打印提示：每次构建都刷一行 cargo:warning 只会淹没真正的告警
        }
        Ok(s) => println!("cargo:warning=rc.exe 失败（{s}），exe 将没有图标和版本信息"),
        Err(e) => println!("cargo:warning=无法运行 rc.exe：{e}"),
    }
}

/// 找 Windows SDK 里的 rc.exe：环境变量 → 注册表 KitsRoot10 → 常见路径
fn find_rc() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CAMNEXUS_RC") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    // 注册表：KitsRoot10 = "H:\Windows Kits\10\"
    if let Ok(out) = Command::new("reg")
        .args([
            "query",
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots",
            "/v",
            "KitsRoot10",
        ])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if line.contains("KitsRoot10") && line.contains("REG_SZ")
                && let Some(v) = line.split("REG_SZ").nth(1) {
                    let v = v.trim().trim_end_matches('\\');
                    if !v.is_empty() {
                        roots.push(PathBuf::from(v));
                    }
                }
        }
    }
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Ok(p) = std::env::var(var) {
            roots.push(PathBuf::from(p).join("Windows Kits").join("10"));
        }
    }

    for root in roots {
        let bin = root.join("bin");
        let Ok(rd) = std::fs::read_dir(&bin) else {
            continue;
        };
        // 取版本号最大的那个 SDK
        let mut cands: Vec<PathBuf> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path().join("x64").join("rc.exe"))
            .filter(|p| p.is_file())
            .collect();
        cands.sort();
        if let Some(p) = cands.pop() {
            return Some(p);
        }
    }
    None
}

/// versioninfo.rc 里的 FILEVERSION 与 Cargo.toml 版本对不上时提醒
fn check_rc_version(rc_file: &Path, cargo_ver: &str) {
    let Ok(text) = std::fs::read_to_string(rc_file) else {
        return;
    };
    let Some(line) = text.lines().find(|l| l.trim_start().starts_with("FILEVERSION")) else {
        return;
    };
    let nums: Vec<u32> = line
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    if nums.len() < 3 {
        return;
    }
    let rc_ver = format!("{}.{}.{}", nums[0], nums[1], nums[2]);
    if rc_ver != cargo_ver {
        println!(
            "cargo:warning=versioninfo.rc 的版本 {rc_ver} 与 Cargo.toml 的 {cargo_ver} 不一致"
        );
    }
}
