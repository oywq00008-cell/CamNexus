//! 系统指标采集：进程 CPU 时间、系统 CPU 占用
//!
//! 只依赖 windows-sys，不引入第三方运行时。

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessTimes, GetSystemTimes, OpenProcess,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    fn to_u64(ft: FILETIME) -> u64 {
        ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
    }


    /// 指定路径所在磁盘的可用字节数（给录制配额做上限保护用）。
    /// 路径可能还没创建，这里向上找第一个存在的目录。
    pub fn free_bytes(path: &std::path::Path) -> Option<u64> {
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

        let mut p: &std::path::Path = path;
        loop {
            if p.exists() {
                break;
            }
            p = p.parent()?;
        }
        let wide: Vec<u16> = p
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut free: u64 = 0;
        let ok = unsafe {
            GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut())
        };
        if ok == 0 {
            None
        } else {
            Some(free)
        }
    }

    /// 进程累计 CPU 时间（秒）。进程不存在或无权限时返回 None。
    pub fn process_cpu_seconds(pid: u32) -> Option<f64> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let mut creation = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let mut exit = creation;
            let mut kernel = creation;
            let mut user = creation;
            let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
            CloseHandle(handle);
            if ok == 0 {
                return None;
            }
            // FILETIME 单位是 100ns
            Some((to_u64(kernel) + to_u64(user)) as f64 / 10_000_000.0)
        }
    }

    /// 系统累计 (idle, kernel, user) 时间，单位 100ns
    pub fn system_times() -> Option<(u64, u64, u64)> {
        unsafe {
            let mut idle = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let mut kernel = idle;
            let mut user = idle;
            if GetSystemTimes(&mut idle, &mut kernel, &mut user) == 0 {
                return None;
            }
            Some((to_u64(idle), to_u64(kernel), to_u64(user)))
        }
    }

    pub fn current_pid() -> u32 {
        std::process::id()
    }

    #[allow(dead_code)]
    pub fn _current_process() {
        unsafe {
            let _ = GetCurrentProcess();
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn process_cpu_seconds(_pid: u32) -> Option<f64> {
        None
    }
    pub fn system_times() -> Option<(u64, u64, u64)> {
        None
    }
    pub fn current_pid() -> u32 {
        std::process::id()
    }
}

pub use imp::{current_pid, free_bytes, process_cpu_seconds, system_times};

/// 逻辑处理器数量
pub fn logical_cores() -> f32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as f32)
        .unwrap_or(1.0)
}

/// 采样器：把「累计 CPU 时间」换算成「占总算力的百分比」
#[derive(Default)]
pub struct CpuMeter {
    last: Option<(std::time::Instant, f64)>,
    pub pct: f32,
}

impl CpuMeter {
    pub fn sample(&mut self, pid: u32) -> f32 {
        let Some(total) = process_cpu_seconds(pid) else {
            self.pct = 0.0;
            return 0.0;
        };
        let now = std::time::Instant::now();
        if let Some((t0, c0)) = self.last {
            let dt = now.duration_since(t0).as_secs_f32();
            if dt > 0.2 {
                let dc = (total - c0).max(0.0) as f32;
                let pct = dc / dt / logical_cores() * 100.0;
                // 平滑，避免单次采样抖动
                self.pct = if self.pct == 0.0 {
                    pct
                } else {
                    self.pct * 0.6 + pct * 0.4
                };
                self.last = Some((now, total));
            }
        } else {
            self.last = Some((now, total));
        }
        self.pct
    }
}

/// 全局 CPU 占用（%）
#[derive(Default)]
pub struct SysCpuMeter {
    last: Option<(u64, u64, u64)>,
    pub pct: f32,
}

impl SysCpuMeter {
    pub fn sample(&mut self) -> f32 {
        let Some((idle, kernel, user)) = system_times() else {
            return 0.0;
        };
        if let Some((i0, k0, u0)) = self.last {
            let di = idle.saturating_sub(i0) as f32;
            let dk = kernel.saturating_sub(k0) as f32;
            let du = user.saturating_sub(u0) as f32;
            let total = dk + du;
            if total > 0.0 {
                let pct = (total - di) / total * 100.0;
                self.pct = self.pct * 0.5 + pct * 0.5;
            }
        }
        self.last = Some((idle, kernel, user));
        self.pct
    }
}

// ---------------------------------------------------------------- 本地时间
/// 本机时间：(年, 月, 日, 时, 分, 秒)
#[cfg(windows)]
pub fn local_now() -> (i32, u32, u32, u32, u32, u32) {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: GetLocalTime 只写入我们提供的栈上结构体
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    (
        st.wYear as i32,
        st.wMonth as u32,
        st.wDay as u32,
        st.wHour as u32,
        st.wMinute as u32,
        st.wSecond as u32,
    )
}

/// 非 Windows 退化为 UTC（仅用于编译通过，录制定位仍按 UTC）
#[cfg(not(windows))]
pub fn local_now() -> (i32, u32, u32, u32, u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    (
        y,
        m as u32,
        d as u32,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

/// 由「1970-01-01 起的天数」反推公历日期（Howard Hinnant 算法）
#[cfg(not(windows))]
fn civil_from_days(z: i64) -> (i32, i32, i32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        (if m <= 2 { y + 1 } else { y }) as i32,
        m as i32,
        d as i32,
    )
}
