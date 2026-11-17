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

// ---------------------------------------------------------------- 网络与内存

/// 本机网络吞吐（所有已连接的非回环网卡合计）。
///
/// 数据栏里显示的是「这台机器当前收/发多少」，包含录像回放、
/// 系统更新等一切流量——它不是 CamNexus 自己的流量。
#[cfg(windows)]
#[derive(Default)]
pub struct NetMeter {
    last: Option<(std::time::Instant, u64, u64)>,
    down_bps: f64,
    up_bps: f64,
}

#[cfg(windows)]
impl NetMeter {
    /// 返回 (下行, 上行) 字节/秒。首次调用只建立基线，返回 0。
    pub fn sample(&mut self) -> (f64, f64) {
        let Some((rx, tx)) = if_counters() else {
            return (self.down_bps, self.up_bps);
        };
        let now = std::time::Instant::now();
        if let Some((t0, rx0, tx0)) = self.last {
            let dt = now.duration_since(t0).as_secs_f64();
            if dt >= 0.4 {
                // 计数器可能被系统重置，用 saturating 防止出现负数
                let d_rx = rx.saturating_sub(rx0) as f64 / dt;
                let d_tx = tx.saturating_sub(tx0) as f64 / dt;
                // 指数平滑：数字不跳，读数仍然及时
                self.down_bps = self.down_bps * 0.45 + d_rx * 0.55;
                self.up_bps = self.up_bps * 0.45 + d_tx * 0.55;
                self.last = Some((now, rx, tx));
            }
        } else {
            self.last = Some((now, rx, tx));
        }
        (self.down_bps, self.up_bps)
    }
}

#[cfg(windows)]
fn if_counters() -> Option<(u64, u64)> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetIfTable2, IF_TYPE_SOFTWARE_LOOPBACK, MIB_IF_TABLE2,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;

    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        // 返回 0 表示 NO_ERROR
        if GetIfTable2(&mut table) != 0 || table.is_null() {
            return None;
        }
        let n = (*table).NumEntries as usize;
        // Table 声明成长度 1 的数组，实际是变长结构，按 NumEntries 取
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), n);
        let mut rx = 0u64;
        let mut tx = 0u64;
        for r in rows {
            if r.Type == IF_TYPE_SOFTWARE_LOOPBACK || r.OperStatus != IfOperStatusUp {
                continue;
            }
            // 只要物理网卡：Hyper-V / VMware / 远程桌面的虚拟网卡会把流量重复计一遍，
            // 实测不筛选时上行会虚高到十几 MB/s（真实只有几百 KB/s）。
            // _bitfield 的最低位就是 HardwareInterface。
            if r.InterfaceAndOperStatusFlags._bitfield & 1 == 0 {
                continue;
            }
            rx += r.InOctets;
            tx += r.OutOctets;
        }
        FreeMibTable(table as *const core::ffi::c_void);
        Some((rx, tx))
    }
}

#[cfg(not(windows))]
#[derive(Default)]
pub struct NetMeter;

#[cfg(not(windows))]
impl NetMeter {
    pub fn sample(&mut self) -> (f64, f64) {
        (0.0, 0.0)
    }
}

/// 物理内存占用百分比（0-100）
#[cfg(windows)]
pub fn memory_load() -> Option<f32> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    let ok = unsafe { GlobalMemoryStatusEx(&mut m) };
    if ok != 0 {
        Some(m.dwMemoryLoad as f32)
    } else {
        None
    }
}

#[cfg(not(windows))]
pub fn memory_load() -> Option<f32> {
    None
}


/// 诊断用：列出每个网络接口的累计收发字节，以及它是否被计入速率统计
#[cfg(windows)]
pub fn net_ifaces() -> Vec<(String, u64, u64, bool)> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetIfTable2, IF_TYPE_SOFTWARE_LOOPBACK, MIB_IF_TABLE2,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;

    let mut out = Vec::new();
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table) != 0 || table.is_null() {
            return out;
        }
        let n = (*table).NumEntries as usize;
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), n);
        for r in rows {
            let name = String::from_utf16_lossy(
                &r.Alias[..r.Alias.iter().position(|c| *c == 0).unwrap_or(r.Alias.len())],
            );
            let up = r.OperStatus == IfOperStatusUp;
            let hw = r.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
            let counted = up && hw && r.Type != IF_TYPE_SOFTWARE_LOOPBACK;
            if up {
                out.push((name, r.InOctets, r.OutOctets, counted));
            }
        }
        FreeMibTable(table as *const core::ffi::c_void);
    }
    out
}

#[cfg(not(windows))]
pub fn net_ifaces() -> Vec<(String, u64, u64, bool)> {
    Vec::new()
}

/// 字节/秒 → 便于阅读的短字符串（B/s、KB/s、MB/s）
pub fn human_rate(bytes_per_sec: f64) -> String {
    let v = bytes_per_sec.max(0.0);
    if v < 1024.0 {
        format!("{v:.0} B/s")
    } else if v < 1024.0 * 1024.0 {
        format!("{:.0} KB/s", v / 1024.0)
    } else {
        format!("{:.1} MB/s", v / (1024.0 * 1024.0))
    }
}

// ---------------------------------------------------------------- 开机自启

/// Run 键下的值名
const RUN_NAME: &str = "CamNexus";

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 自启命令行：路径带空格必须加引号，否则 Run 项解析会断
pub fn autostart_command(exe: &std::path::Path, guard: bool) -> String {
    let p = exe.display().to_string();
    if guard {
        format!("\"{p}\" --guard")
    } else {
        format!("\"{p}\"")
    }
}

/// 读回 Run 项当前值（没有则 None），用于判断是否与设置一致
#[cfg(windows)]
pub fn autostart_read() -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
        REG_SZ,
    };

    let sub = wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let name = wide(RUN_NAME);
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut hkey,
        ) != 0
        {
            return None;
        }
        let mut buf = vec![0u8; 1024];
        let mut len = buf.len() as u32;
        let mut ty = REG_SZ;
        let rc = RegQueryValueExW(
            hkey,
            name.as_ptr(),
            std::ptr::null(),
            &mut ty,
            buf.as_mut_ptr(),
            &mut len,
        );
        RegCloseKey(hkey);
        if rc != 0 {
            return None;
        }
        // 值比缓冲区长时，系统会把 len 设成所需字节数——直接切片会越界
        let len = (len as usize).min(buf.len());
        let u16s: Vec<u16> = buf[..len]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .take_while(|c| *c != 0)
            .collect();
        Some(String::from_utf16_lossy(&u16s))
    }
}

#[cfg(not(windows))]
pub fn autostart_read() -> Option<String> {
    None
}

/// 写入/删除自启项。`on = false` 时删除。
#[cfg(windows)]
pub fn set_autostart(exe: &std::path::Path, guard: bool, on: bool) -> Result<(), String> {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, RegCloseKey, RegCreateKeyExW, RegDeleteValueW,
        RegSetValueExW, REG_SZ,
    };

    let sub = wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let name = wide(RUN_NAME);
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut hkey,
            std::ptr::null_mut(),
        );
        if rc != 0 {
            return Err(format!("打开注册表失败（错误码 {rc}）"));
        }
        let out = if on {
            let cmd = wide(&autostart_command(exe, guard));
            let bytes = (cmd.len() * 2) as u32;
            RegSetValueExW(
                hkey,
                name.as_ptr(),
                0,
                REG_SZ,
                cmd.as_ptr() as *const u8,
                bytes,
            )
        } else {
            RegDeleteValueW(hkey, name.as_ptr())
        };
        RegCloseKey(hkey);
        // 2 = 值不存在，删除时属正常
        if out != 0 && !(out == 2 && !on) {
            return Err(format!("写入注册表失败（错误码 {out}）"));
        }
        Ok(())
    }
}

#[cfg(not(windows))]
pub fn set_autostart(_exe: &std::path::Path, _guard: bool, _on: bool) -> Result<(), String> {
    Err("当前系统不支持开机自启".into())
}
