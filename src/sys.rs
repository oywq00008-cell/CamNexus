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

// ---------------------------------------------------------------- 硬件画像

/// 本机硬件画像，用于给识别模型分档推荐。
///
/// 只做粗粒度判断：真正准的结论要在下载模型后实测。
pub struct Hardware {
    pub cpu: String,
    /// 逻辑核数
    pub cores: usize,
    /// 估算的 Cinebench R23 多核分数，0 表示测不到
    pub cpu_mt: u32,
    pub gpu: String,
    /// FP32 TFLOPS，0 表示核显/未知/没有可用独显
    pub gpu_tflops: f32,
    /// DirectML.dll 是否可加载
    pub directml: bool,
}

impl Hardware {
    /// 有独显、而且规格不低时才算「能用显卡」。
    /// 核显的 TFLOPS 数字看着不低，但实际跑推理会被内存带宽拖死，直接排除。
    pub fn gpu_usable(&self) -> bool {
        self.directml && self.gpu_tflops >= 1.5
    }
}

/// 进程内只探测一次：设置页每帧都会问
pub fn hardware() -> &'static Hardware {
    static HW: std::sync::OnceLock<Hardware> = std::sync::OnceLock::new();
    HW.get_or_init(probe_hardware)
}

#[cfg(not(windows))]
fn probe_hardware() -> Hardware {
    Hardware {
        cpu: String::new(),
        cores: 4,
        cpu_mt: 8_000,
        gpu: String::new(),
        gpu_tflops: 0.0,
        directml: false,
    }
}

#[cfg(windows)]
fn probe_hardware() -> Hardware {
    let cpu = read_reg_string(
        r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
        "ProcessorNameString",
    )
    .unwrap_or_else(|| "未知处理器".to_string());
    let logical = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    // available_parallelism 给的是逻辑核；R23 分数按物理核标定，粗略折半
    let physical = if logical >= 8 {
        (logical / 2).max(1)
    } else {
        logical
    };

    // 注册表不受会话影响；EnumDisplayDevices 在 session 0 里拿不到适配器，
    // 所以两者合并，谁有结果用谁。
    let mut names = gpu_names_registry();
    for n in gpu_names_enum() {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    let gpu = names.join(" / ");
    // 取最强的那个独显
    let tflops = names
        .iter()
        .filter(|n| !is_integrated_gpu(n))
        .filter_map(|n| lookup_tflops(n))
        .fold(0.0f32, f32::max);

    Hardware {
        cpu_mt: cpu_mt_estimate(&cpu, physical),
        cpu,
        cores: logical,
        gpu,
        gpu_tflops: tflops,
        directml: directml_available(),
    }
}

#[cfg(windows)]
fn read_reg_string(sub: &str, name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
        REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    let (sub, name) = (wide(sub), wide(name));
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, sub.as_ptr(), 0, KEY_QUERY_VALUE, &mut hkey) != 0 {
            return None;
        }
        let mut buf = vec![0u8; 4096];
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
        let len = (len as usize).min(buf.len());
        let u16s: Vec<u16> = buf[..len]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .take_while(|c| *c != 0)
            .collect();
        let s = String::from_utf16_lossy(&u16s).trim().to_string();
        (!s.is_empty()).then_some(s)
    }
}


/// 从注册表的显示适配器类枚举显卡名。
///
/// `EnumDisplayDevices` 只认当前会话，SSH / 服务这类 session 0 场景下会一个都
/// 拿不到（实测 Win10 上就是这样），所以主路径走注册表。
#[cfg(windows)]
fn gpu_names_registry() -> Vec<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    const CLASS: &str =
        r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";

    let mut out = Vec::new();
    let sub = wide(CLASS);
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, sub.as_ptr(), 0, KEY_READ, &mut hkey) != 0 {
            return out;
        }
        let mut i = 0u32;
        loop {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            let rc = RegEnumKeyExW(
                hkey,
                i,
                name.as_mut_ptr(),
                &mut len,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            if rc != 0 {
                break;
            }
            i += 1;
            let key = String::from_utf16_lossy(&name[..len as usize]);
            // 只有 0000/0001… 这些子键是适配器，其他（Properties 等）没有 DriverDesc
            if !key.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let path = format!("{CLASS}\\{key}");
            if let Some(desc) = read_reg_string(&path, "DriverDesc")
                && !out.contains(&desc)
            {
                out.push(desc);
            }
        }
        RegCloseKey(hkey);
    }
    out
}

/// 枚举显示适配器（不依赖 COM）。缺点：只列当前会话里的适配器。
#[cfg(windows)]
fn gpu_names_enum() -> Vec<String> {
    use windows_sys::Win32::Graphics::Gdi::{DISPLAY_DEVICEW, EnumDisplayDevicesW};

    let mut out: Vec<String> = Vec::new();
    for i in 0..8u32 {
        let mut dd: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
        dd.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
        let ok = unsafe { EnumDisplayDevicesW(std::ptr::null(), i, &mut dd, 0) };
        if ok == 0 {
            break;
        }
        let end = dd
            .DeviceString
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(dd.DeviceString.len());
        let name = String::from_utf16_lossy(&dd.DeviceString[..end])
            .trim()
            .to_string();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

#[cfg(windows)]
fn directml_available() -> bool {
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
    let name: Vec<u16> = "DirectML.dll"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe { !LoadLibraryW(name.as_ptr()).is_null() }
}

/// 核显/虚拟显卡：TFLOPS 数字不低，但共享内存带宽，实测远不如独显，一律按 CPU 处理
fn is_integrated_gpu(name: &str) -> bool {
    // "AMD Radeon(TM) Graphics" 这类名字里夹着商标符号，先去掉再匹配
    let n = name
        .to_ascii_lowercase()
        .replace("(tm)", "")
        .replace("(r)", "")
        .replace('®', "")
        .replace("  ", " ");
    [
        "uhd graphics",
        "hd graphics",
        "iris",
        "vega",
        "radeon graphics",
        "integrated",
        "microsoft basic",
        "remote",
        "virtual",
        "oray",
        "gameviewer",
    ]
    .iter()
    .any(|k| n.contains(k))
}

/// 显卡名 → FP32 TFLOPS。查不到返回 None（保守处理，不认这块卡）
fn lookup_tflops(name: &str) -> Option<f32> {
    let n = name.to_ascii_lowercase();
    // 从长到短匹配，避免 "rtx 4070" 抢走 "rtx 4070 ti super"
    const TABLE: &[(&str, f32)] = &[
        ("rtx 5090", 104.9),
        ("rtx 5080", 56.3),
        ("rtx 5070 ti", 44.1),
        ("rtx 5070", 30.8),
        ("rtx 5060 ti", 23.7),
        ("rtx 5060", 19.2),
        ("rtx 4090", 82.6),
        ("rtx 4080 super", 51.3),
        ("rtx 4080", 48.7),
        ("rtx 4070 ti super", 44.1),
        ("rtx 4070 ti", 40.1),
        ("rtx 4070 super", 35.5),
        ("rtx 4070", 29.1),
        ("rtx 4060 ti", 22.1),
        ("rtx 4060", 15.1),
        ("rtx 3090 ti", 40.0),
        ("rtx 3090", 35.6),
        ("rtx 3080 ti", 34.1),
        ("rtx 3080", 29.8),
        ("rtx 3070 ti", 21.8),
        ("rtx 3070", 20.3),
        ("rtx 3060 ti", 16.2),
        ("rtx 3060", 12.7),
        ("rtx 3050", 7.9),
        ("rtx 2080 ti", 14.3),
        ("rtx 2080 super", 11.2),
        ("rtx 2080", 10.6),
        ("rtx 2070 super", 9.1),
        ("rtx 2070", 7.9),
        ("rtx 2060 super", 7.2),
        ("rtx 2060", 6.45),
        ("gtx 1660 ti", 5.44),
        ("gtx 1660 super", 5.03),
        ("gtx 1660", 5.03),
        ("gtx 1650 super", 4.42),
        ("gtx 1650", 2.85),
        ("gtx 1630", 1.83),
        ("gtx 1080 ti", 11.3),
        ("gtx 1080", 8.87),
        ("gtx 1070 ti", 8.19),
        ("gtx 1070", 6.46),
        ("gtx 1060", 4.37),
        ("gtx 1050 ti", 2.14),
        ("gtx 1050", 1.86),
        ("gtx 980 ti", 5.6),
        ("gtx 970", 3.9),
        ("gtx 960", 2.37),
        ("gtx 750 ti", 1.31),
        ("gt 1030", 1.13),
        ("rx 7900 xtx", 61.4),
        ("rx 7900 xt", 51.6),
        ("rx 7900 gre", 46.0),
        ("rx 7800 xt", 37.3),
        ("rx 7700 xt", 35.2),
        ("rx 7600", 21.75),
        ("rx 6700 xt", 13.2),
        ("rx 6700", 11.3),
        ("rx 6650 xt", 10.8),
        ("rx 6600 xt", 10.6),
        ("rx 6600", 8.93),
        ("rx 6500 xt", 5.77),
        ("rx 5700 xt", 9.75),
        ("rx 5700", 7.95),
        ("rx 5600 xt", 7.19),
        ("rx 5600", 6.39),
        ("rx 5500 xt", 5.20),
        ("rx 590", 7.12),
        ("rx 580", 6.18),
        ("rx 570", 5.10),
        ("rx 560", 2.11),
        ("rx 550", 1.37),
        ("arc a770", 19.7),
        ("arc a750", 17.2),
        ("arc a580", 14.7),
        ("arc a380", 4.2),
        ("arc a310", 3.07),
    ];
    TABLE.iter().find(|(k, _)| n.contains(k)).map(|(_, v)| *v)
}

/// 用「核数 × 每核系数」粗估 Cinebench R23 多核分数。
///
/// 系数从公开数据反推：i5-10400 → 6×1350≈8100（实际 8219），
/// 9800X3D → 8×2600≈20800（实际 23163），误差都在 10% 以内。
fn cpu_mt_estimate(model: &str, cores: usize) -> u32 {
    let m = model.to_ascii_lowercase();
    let digits: Vec<u32> = m
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<u32>().ok())
        .collect();

    // 型号数字里有 i5 的 5、频率的 2.90，必须先过滤掉：型号本体一定 ≥ 1000
    let model_no = digits.iter().copied().find(|v| *v >= 1000);

    let per_core = if m.contains("ryzen") {
        let series = model_no.filter(|v| *v <= 9999).unwrap_or(5000);
        match series {
            9000..=9999 => 2600.0,
            8000..=8999 => 2200.0,
            7000..=7999 => 2400.0,
            5000..=5999 => 1750.0,
            4000..=4999 => 1500.0,
            3000..=3999 => 1450.0,
            2000..=2999 => 1200.0,
            _ => 1000.0,
        }
    } else if m.contains("atom") || m.contains("celeron") || m.contains("pentium") {
        600.0
    } else if m.contains("n100") || m.contains("n95") || m.contains("n5105") || m.contains("j4125")
    {
        700.0
    } else if m.contains("core") || m.contains("xeon") {
        // Intel Core i3/i5/i7/i9-NNNNN 或 Xeon E5-NNNNv1
        let num = model_no.unwrap_or(8000);
        let gen_n = num / 1000;
        match gen_n {
            0..=4 => 900.0,
            5..=7 => 1050.0,
            8..=9 => 1150.0,
            10..=11 => 1350.0,
            _ => 1900.0,
        }
    } else {
        1200.0
    };
    ((cores.max(1) as f32) * per_core) as u32
}



// ---------------------------------------------------------------- 打开链接

/// 用系统默认程序打开网址（手动下载时用）
#[cfg(windows)]
pub fn open_url(url: &str) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let op: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let target: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    };
    r as isize > 32
}

#[cfg(not(windows))]
pub fn open_url(_url: &str) -> bool {
    false
}

/// 在资源管理器里打开目录
#[cfg(windows)]
pub fn open_path(path: &std::path::Path) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let op: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let target: Vec<u16> = path
        .display()
        .to_string()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    };
    r as isize > 32
}

#[cfg(not(windows))]
pub fn open_path(_path: &std::path::Path) -> bool {
    false
}

#[cfg(test)]
mod hw_tests {
    use super::*;

    #[test]
    fn cpu_estimate_tracks_published_scores() {
        // 公开的 Cinebench R23 多核分数，允许 15% 误差
        for (model, cores, actual) in [
            ("Intel(R) Core(TM) i5-10400 CPU @ 2.90GHz", 6, 8219u32),
            ("AMD Ryzen 7 9800X3D 8-Core Processor", 8, 23163),
            ("Intel(R) Core(TM) i5-12400", 6, 12374),
            ("Intel(R) Core(TM) i3-10105F", 4, 5615),
            ("AMD Ryzen 5 5600 6-Core Processor", 6, 11052),
        ] {
            let got = cpu_mt_estimate(model, cores);
            let dev = (got as f32 - actual as f32).abs() / actual as f32;
            assert!(
                dev < 0.15,
                "{model}: 估 {got}，实际 {actual}，偏差 {:.0}%",
                dev * 100.0
            );
        }
    }

    #[test]
    fn gpu_lookup_prefers_the_longer_match() {
        assert_eq!(lookup_tflops("NVIDIA GeForce RTX 4070 Ti SUPER"), Some(44.1));
        assert_eq!(lookup_tflops("NVIDIA GeForce RTX 4070"), Some(29.1));
        assert_eq!(lookup_tflops("NVIDIA GeForce GTX 960"), Some(2.37));
        assert_eq!(lookup_tflops("AMD Radeon RX 6600"), Some(8.93));
        // 未知型号必须返回 None，不能瞎猜一个高分
        assert_eq!(lookup_tflops("NVIDIA GeForce RTX 9090"), None);
    }

    #[test]
    fn integrated_gpus_are_excluded() {
        assert!(is_integrated_gpu("Intel(R) UHD Graphics 630"));
        assert!(is_integrated_gpu("AMD Radeon(TM) Graphics"));
        assert!(is_integrated_gpu("Intel Iris Xe Graphics"));
        assert!(is_integrated_gpu("GameViewer Virtual Display Adapter"));
        assert!(!is_integrated_gpu("NVIDIA GeForce GTX 960"));
    }
}
