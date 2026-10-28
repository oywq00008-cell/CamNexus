//! 检测模型的按需下载（实验功能）。
//!
//! 模型不进安装包：35.8 MB 的体积对「绿色版双击就能用」太重，而且大多数用户
//! 根本不会开识别。用户只有在「设置 → 实验」里主动点下载，才会落到
//! `%LOCALAPPDATA%\CamNexus\models\`。
//!
//! 下载完必须校验 SHA-256：模型文件是直接喂给推理引擎的，被替换了不会有任何
//! 提示，只会「识别变差」——这种问题极难查。

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::i18n::tr;

/// 模型文件名（`detect::find_model` 按这个名字找）
pub const FILE: &str = "yolox_s.onnx";
/// 官方发布地址（Megvii YOLOX 0.1.0 release，Apache-2.0）
pub const URL: &str =
    "https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.0/yolox_s.onnx";
/// 官方文件的 SHA-256，用于校验下载完整性 / 防止被替换
pub const SHA256: &str = "50f9715c9eb6831666a3c1f84c36cbcd34b2fc6db8c02c8af0f330b1b3fbb3ff";
/// 官方文件大小，用于下载前预估和完整性粗筛
pub const SIZE: u64 = 35_858_697;

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Downloading { got: u64, total: u64 },
    Verifying,
    Done,
    Failed(String),
}

pub type Shared = Arc<Mutex<State>>;

pub fn shared() -> Shared {
    Arc::new(Mutex::new(State::Idle))
}

fn set(shared: &Shared, s: State) {
    if let Ok(mut g) = shared.lock() {
        *g = s;
    }
}

pub fn models_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|p| p.join("CamNexus").join("models"))
}

/// 已安装且大小对得上就返回路径（哈希在下载时已经校验过，这里只做粗筛）
pub fn installed() -> Option<PathBuf> {
    let p = models_dir()?.join(FILE);
    let meta = std::fs::metadata(&p).ok()?;
    if meta.len() == SIZE {
        Some(p)
    } else {
        None
    }
}

/// 后台下载 + 校验，进度写进 `shared`
pub fn spawn_download() -> Shared {
    let shared = shared();
    let st = shared.clone();
    std::thread::spawn(move || {
        match download(&st) {
            Ok(()) => set(&st, State::Done),
            Err(e) => set(&st, State::Failed(e)),
        }
    });
    shared
}

fn download(state: &Shared) -> Result<(), String> {
    let dir = models_dir().ok_or_else(|| tr("找不到可写的安装目录").to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| crate::i18n::err(tr("创建目录失败"), e))?;
    let dest = dir.join(FILE);
    let tmp = dir.join("yolox_s.onnx.part");

    let agent = ureq::Agent::config_builder()
        .timeout_global(None)
        .build()
        .new_agent();

    let mut resp = agent
        .get(URL)
        .header("User-Agent", "CamNexus/0.5 (+https://github.com)")
        .call()
        .map_err(|e| crate::i18n::err(tr("请求失败"), e))?;

    let total = resp.body().content_length().unwrap_or(SIZE);
    let mut reader = resp.body_mut().as_reader();
    let mut file =
        std::fs::File::create(&tmp).map_err(|e| crate::i18n::err(tr("创建临时文件失败"), e))?;

    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut got: u64 = 0;
    let mut last = Instant::now();

    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| crate::i18n::err(tr("读取中断"), e))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| crate::i18n::err(tr("写入失败"), e))?;
        hasher.update(&buf[..n]);
        got += n as u64;
        if last.elapsed().as_millis() >= 200 {
            last = Instant::now();
            set(state, State::Downloading { got, total });
        }
    }
    file.flush()
        .map_err(|e| crate::i18n::err(tr("落盘失败"), e))?;
    drop(file);

    set(state, State::Verifying);
    let digest = format!("{:x}", hasher.finalize());
    if digest != SHA256 {
        let _ = std::fs::remove_file(&tmp);
        return Err(tr("模型校验失败（文件可能被篡改或下载不完整）").to_string());
    }
    if got != SIZE {
        let _ = std::fs::remove_file(&tmp);
        return Err(crate::i18n::file_too_small(got));
    }

    std::fs::rename(&tmp, &dest).map_err(|e| crate::i18n::err(tr("保存失败"), e))?;
    Ok(())
}

/// 删除已下载的模型（用户想收回空间时用）
pub fn remove() -> Result<(), String> {
    let Some(p) = models_dir().map(|d| d.join(FILE)) else {
        return Ok(());
    };
    if p.exists() {
        std::fs::remove_file(&p).map_err(|e| crate::i18n::err(tr("删除失败"), e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机下载一次（需要网络）。跑法：
    ///   set LOCALAPPDATA=%TEMP%\modeltest && cargo test -- --ignored download_model
    #[test]
    #[ignore]
    fn download_model_roundtrip() {
        let shared = spawn_download();
        let deadline = Instant::now() + std::time::Duration::from_secs(180);
        loop {
            let st = shared.lock().map(|g| g.clone()).unwrap_or(State::Idle);
            match st {
                State::Done => break,
                State::Failed(e) => panic!("下载失败: {e}"),
                _ => {}
            }
            assert!(Instant::now() < deadline, "下载超时");
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let p = installed().expect("下载完成后应能定位到模型");
        assert_eq!(std::fs::metadata(&p).unwrap().len(), SIZE);
    }

    #[test]
    fn model_meta_is_plausible() {
        // 官方 release 的固定大小，改错了这里会先炸
        assert_eq!(SIZE, 35_858_697);
        assert_eq!(SHA256.len(), 64);
        assert!(URL.starts_with("https://"));
        assert!(FILE.ends_with(".onnx"));
    }
}
