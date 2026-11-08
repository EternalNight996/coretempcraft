//! 实时 CPU 主频读取接口（Windows，用户态，无需安装驱动）。
//!
//! 原版程序通过自有内核驱动读取 MSR 0x198 的倍频，再乘以实测基频。
//! 这个模块用 Windows 自带的电源管理接口读取同一含义的“有效主频”：
//!   * `CallNtPowerInformation(ProcessorInformation)` -> 每个逻辑核心的 CurrentMhz；
//!   * `NtQuerySystemInformation(SystemProcessorPerformanceInformation)` -> 每核心负载；
//!   * CPUID / 注册表 -> 型号名、基准频率、最大频率。
//!
//! 用法：
//! ```no_run
//! let mut sampler = cpu::CpuSampler::new();
//! loop {
//!     let snap = sampler.sample();
//!     for core in &snap.cores {
//!         println!("core {}: {:.0} MHz  x{:.1}  {:.0}%",
//!             core.index, core.current_mhz, core.multiplier, core.load * 100.0);
//!     }
//!     std::thread::sleep(std::time::Duration::from_millis(cpu::DEFAULT_INTERVAL_MS));
//! }
//! ```
//!
//! 采样间隔取自原版程序配置里的 `ReadInt=1000`（毫秒）。
//! 采样器内部保存上一次的负载计数，因此请复用一个 `CpuSampler`，不要每次新建。

use std::ffi::c_void;

/// 原程序 `[General] ReadInt` 的默认值，单位毫秒。
pub const DEFAULT_INTERVAL_MS: u64 = 1_000;

/// 原程序允许的采样间隔范围，单位毫秒（见 FUN_14000cea0：10..9999）。
pub const MIN_INTERVAL_MS: u64 = 10;
pub const MAX_INTERVAL_MS: u64 = 9_999;

/// 原程序测量基频时使用的窗口（FUN_140064070 -> FUN_140062150 传入 5000 ms）。
/// 重建版在 CPUID 与注册表都拿不到基频时，用同一窗口测量 TSC 基频。
pub const BASE_CLOCK_MEASURE_WINDOW_MS: u64 = 5000;

#[derive(Clone, Debug, Default)]
pub struct CoreSample {
    /// 逻辑处理器编号，从 0 开始。
    pub index: usize,
    /// 当前有效主频，MHz。
    pub current_mhz: f32,
    /// 该核心报告的最大频率，MHz。
    pub max_mhz: f32,
    /// 倍频 = 当前频率 / 基准频率（原程序用 MSR 0x198 的倍频）。
    pub multiplier: f32,
    /// 自上次采样以来的核心负载，0.0..=1.0。
    pub load: f32,
}

#[derive(Clone, Debug)]
pub struct CpuSnapshot {
    pub cores: Vec<CoreSample>,
    /// 基准频率（外频 / 标称频率），MHz。
    pub base_mhz: f32,
    /// 全局最大频率，MHz。
    pub max_mhz: f32,
    /// 所有核心当前频率的平均值，MHz。
    pub avg_mhz: f32,
    /// 当前频率最高的核心，MHz。
    pub peak_mhz: f32,
    /// 所有核心负载的平均值，0.0..=1.0。
    pub avg_load: f32,
    /// 本次读取是否成功。false 时 current_mhz 为 0，界面应提示不可用。
    pub source_ok: bool,
    pub timestamp: std::time::SystemTime,
}

/// 一次创建，持续采样。内部保存负载计数，用来计算两次采样之间的占用率。
pub struct CpuSampler {
    cpu_name: String,
    vendor: String,
    logical_cores: usize,
    base_mhz: f32,
    max_mhz: f32,
    base_estimated: bool,
    previous_load: Vec<SystemProcessorPerformanceInformation>,
    vendor_id: [u32; 3],
}

impl CpuSampler {
    pub fn new() -> Self {
        let logical_cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .clamp(1, 512);

        let regs = cpuid_registers();

        let mut cpu_name = reg_string(
            "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0",
            "ProcessorNameString",
        )
        .filter(|s| !s.trim().is_empty())
        .or_else(|| regs.as_ref().map(|r| r.brand.clone()).filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| "Unknown CPU".to_string());

        // 去掉多余空白，型号名里常有 "  " 或不可见字符。
        cpu_name = cpu_name.split_whitespace().collect::<Vec<_>>().join(" ");

        let vendor = regs.as_ref().map(|r| r.vendor.clone()).unwrap_or_default();
        let vendor_id = regs.as_ref().map(|r| r.vendor_id).unwrap_or([0; 3]);

        let mut base_mhz = regs.as_ref().map(|r| r.base_mhz).unwrap_or(0) as f32;
        let reg_base = reg_dword(
            "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0",
            "~MHz",
        )
        .unwrap_or(0) as f32;
        let cpuid_max = regs.as_ref().map(|r| r.max_mhz).unwrap_or(0) as f32;

        // 优先 CPUID 0x16 的基准频率；否则用注册表 ~MHz。
        let mut base_estimated = false;
        if !(50.0..=10_000.0).contains(&base_mhz) {
            base_mhz = reg_base;
        }

        let power = read_power_information(logical_cores);
        let power_max = power
            .iter()
            .map(|p| p.max_mhz as f32)
            .fold(0.0_f32, f32::max);

        let mut max_mhz = cpuid_max.max(power_max).max(base_mhz);
        if !(50.0..=20_000.0).contains(&base_mhz) || base_mhz <= 0.0 {
            // 没有任何可信的基频时，退回到最大频率；这属于估算。
            base_estimated = true;
            base_mhz = if max_mhz > 0.0 { max_mhz } else { 100.0 };
            max_mhz = max_mhz.max(base_mhz);
        }
        if max_mhz < base_mhz {
            max_mhz = base_mhz;
        }

        // 最后的兜底：用原程序同样的 5000 ms 窗口量一次 TSC 基频。
        // 正常机器上 CPUID/注册表已经有基频，不会走到这里，也不会拖慢启动。
        if base_estimated {
            if let Some(measured) = measure_base_clock_mhz(BASE_CLOCK_MEASURE_WINDOW_MS) {
                base_mhz = measured;
                base_estimated = false;
                max_mhz = max_mhz.max(base_mhz);
            }
        }

        let previous_load = query_processor_performance(logical_cores);

        Self {
            cpu_name,
            vendor,
            logical_cores,
            base_mhz,
            max_mhz,
            base_estimated,
            previous_load,
            vendor_id,
        }
    }

    pub fn cpu_name(&self) -> &str {
        &self.cpu_name
    }

    pub fn vendor(&self) -> &str {
        &self.vendor
    }

    pub fn vendor_id(&self) -> [u32; 3] {
        self.vendor_id
    }

    pub fn logical_cores(&self) -> usize {
        self.logical_cores
    }

    pub fn base_mhz(&self) -> f32 {
        self.base_mhz
    }

    pub fn max_mhz(&self) -> f32 {
        self.max_mhz
    }

    /// 基频是否为估算值（既没有 CPUID 0x16，也没有注册表 ~MHz）。
    pub fn base_estimated(&self) -> bool {
        self.base_estimated
    }

    /// 读取一次全核快照。建议每 1 秒调用一次。
    pub fn sample(&mut self) -> CpuSnapshot {
        let cores_n = self.logical_cores.max(1);
        let power = read_power_information(cores_n);

        let loads = self.compute_loads(cores_n);
        let mut cores = Vec::with_capacity(cores_n);
        let mut source_ok = !power.is_empty();

        for i in 0..cores_n {
            let p = power.get(i);
            let current = p.map(|p| p.current_mhz as f32).unwrap_or(0.0);
            let core_max = p
                .map(|p| p.max_mhz as f32)
                .filter(|v| *v > 0.0)
                .unwrap_or(self.max_mhz);
            let multiplier = if self.base_mhz > 0.0 {
                current / self.base_mhz
            } else {
                0.0
            };
            if current <= 0.0 {
                source_ok = false;
            }
            cores.push(CoreSample {
                index: i,
                current_mhz: current,
                max_mhz: core_max,
                multiplier,
                load: loads.get(i).copied().unwrap_or(0.0),
            });
        }

        let peak_mhz = cores.iter().map(|c| c.current_mhz).fold(0.0_f32, f32::max);
        let avg_mhz = if cores.is_empty() {
            0.0
        } else {
            cores.iter().map(|c| c.current_mhz).sum::<f32>() / cores.len() as f32
        };
        let avg_load = if cores.is_empty() {
            0.0
        } else {
            cores.iter().map(|c| c.load).sum::<f32>() / cores.len() as f32
        };

        CpuSnapshot {
            cores,
            base_mhz: self.base_mhz,
            max_mhz: self.max_mhz,
            avg_mhz,
            peak_mhz,
            avg_load,
            source_ok,
            timestamp: std::time::SystemTime::now(),
        }
    }

    fn compute_loads(&mut self, cores: usize) -> Vec<f32> {
        let now = query_processor_performance(cores);
        if now.is_empty() {
            return vec![0.0; cores];
        }
        let previous = &self.previous_load;
        let mut loads = vec![0.0_f32; cores];
        if previous.len() == now.len() {
            for i in 0..cores.min(now.len()) {
                let idle = now[i].idle_time.saturating_sub(previous[i].idle_time);
                let total_now = now[i].kernel_time
                    + now[i].user_time
                    + now[i].dpc_time
                    + now[i].interrupt_time;
                let total_prev = previous[i].kernel_time
                    + previous[i].user_time
                    + previous[i].dpc_time
                    + previous[i].interrupt_time;
                let total = total_now.saturating_sub(total_prev);
                if total > 0 {
                    let busy = (total - idle).max(0) as f32;
                    loads[i] = (busy / total as f32).clamp(0.0, 1.0);
                }
            }
        }
        self.previous_load = now;
        loads
    }
}

impl Default for CpuSampler {
    fn default() -> Self {
        Self::new()
    }
}

/// 用时间戳计数器（TSC）在给定窗口内测量基频，单位 MHz。
///
/// 现代 x86 的 TSC 以标称基频（constant TSC）计数，因此这就是 原版程序
/// “Measured base clock” 在用户态可用的等价做法。窗口默认 5000 ms，
/// 与原程序 FUN_140062150 的测量窗口一致。
pub fn measure_base_clock_mhz(window_ms: u64) -> Option<f32> {
    if window_ms < 50 {
        return None;
    }
    #[cfg(target_arch = "x86_64")]
    {
        let tsc0 = unsafe { std::arch::x86_64::_rdtsc() };
        let started = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(window_ms));
        let elapsed = started.elapsed().as_secs_f64();
        if elapsed <= 0.0 {
            return None;
        }
        let tsc1 = unsafe { std::arch::x86_64::_rdtsc() };
        let mhz = ((tsc1 - tsc0) as f64 / elapsed / 1_000_000.0) as f32;
        if (50.0..=20_000.0).contains(&mhz) {
            Some(mhz)
        } else {
            None
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = window_ms;
        None
    }
}

// ---------------------------------------------------------------------------
// Windows 用户态接口
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct ProcessorPowerInformation {
    number: u32,
    max_mhz: u32,
    current_mhz: u32,
    mhz_limit: u32,
    max_idle_state: u32,
    current_idle_state: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct SystemProcessorPerformanceInformation {
    idle_time: i64,
    kernel_time: i64,
    user_time: i64,
    dpc_time: i64,
    interrupt_time: i64,
    interrupt_count: u32,
}

const PROCESSOR_INFORMATION: u32 = 11;
const SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION: u32 = 8;

#[link(name = "powrprof")]
extern "system" {
    fn CallNtPowerInformation(
        information_class: u32,
        input_buffer: *mut c_void,
        input_buffer_length: u32,
        output_buffer: *mut c_void,
        output_buffer_length: u32,
    ) -> i32;
}

#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(
        system_information_class: u32,
        system_information: *mut c_void,
        system_information_length: u32,
        return_length: *mut u32,
    ) -> i32;
}

#[link(name = "advapi32")]
extern "system" {
    fn RegGetValueW(
        hkey: isize,
        sub_key: *const u16,
        value: *const u16,
        flags: u32,
        value_type: *mut u32,
        data: *mut c_void,
        data_size: *mut u32,
    ) -> i32;
}

const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as isize;
const RRF_RT_REG_SZ: u32 = 0x0000_0002;
const RRF_RT_REG_DWORD: u32 = 0x0000_0010;

fn read_power_information(count: usize) -> Vec<ProcessorPowerInformation> {
    let count = count.max(1);
    let mut buffer = vec![ProcessorPowerInformation::default(); count];
    let bytes = (count * std::mem::size_of::<ProcessorPowerInformation>()) as u32;
    let status = unsafe {
        CallNtPowerInformation(
            PROCESSOR_INFORMATION,
            std::ptr::null_mut(),
            0,
            buffer.as_mut_ptr().cast::<c_void>(),
            bytes,
        )
    };
    if status == 0 {
        // 个别系统会返回比请求更多的条目，按实际进程数截断。
        buffer.truncate(count);
        buffer
    } else {
        Vec::new()
    }
}

fn query_processor_performance(count: usize) -> Vec<SystemProcessorPerformanceInformation> {
    let count = count.max(1);
    let mut buffer = vec![SystemProcessorPerformanceInformation::default(); count];
    let bytes = (count * std::mem::size_of::<SystemProcessorPerformanceInformation>()) as u32;
    let mut returned = 0u32;
    let status = unsafe {
        NtQuerySystemInformation(
            SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION,
            buffer.as_mut_ptr().cast::<c_void>(),
            bytes,
            &mut returned,
        )
    };
    if status == 0 {
        buffer
    } else {
        Vec::new()
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn reg_string(sub_key: &str, value: &str) -> Option<String> {
    let sub_key_w = wide(sub_key);
    let value_w = wide(value);
    let mut size = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            sub_key_w.as_ptr(),
            value_w.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if status != 0 || size == 0 || size > 64 * 1024 {
        return None;
    }
    let mut buffer = vec![0u16; (size as usize / 2) + 1];
    let mut size2 = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            sub_key_w.as_ptr(),
            value_w.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast::<c_void>(),
            &mut size2,
        )
    };
    if status != 0 {
        return None;
    }
    let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..len]))
}

fn reg_dword(sub_key: &str, value: &str) -> Option<u32> {
    let sub_key_w = wide(sub_key);
    let value_w = wide(value);
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            sub_key_w.as_ptr(),
            value_w.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast::<c_void>(),
            &mut size,
        )
    };
    if status == 0 {
        Some(data)
    } else {
        None
    }
}

struct CpuRegisters {
    vendor: String,
    vendor_id: [u32; 3],
    brand: String,
    base_mhz: u32,
    max_mhz: u32,
}

#[cfg(target_arch = "x86_64")]
fn cpuid_registers() -> Option<CpuRegisters> {
    use std::arch::x86_64::{__cpuid, __cpuid_count};

    let leaf0 = unsafe { __cpuid(0) };
    let vendor = String::from_utf8_lossy(
        &[
            leaf0.ebx.to_le_bytes(),
            leaf0.edx.to_le_bytes(),
            leaf0.ecx.to_le_bytes(),
        ]
        .concat(),
    )
    .trim()
    .to_string();

    let max_basic = leaf0.eax;
    let max_ext = unsafe { __cpuid(0x8000_0000) }.eax;

    let mut brand = String::new();
    if max_ext >= 0x8000_0004 {
        let mut bytes = Vec::with_capacity(48);
        for leaf in 0x8000_0002..=0x8000_0004 {
            let r = unsafe { __cpuid(leaf) };
            bytes.extend_from_slice(&r.eax.to_le_bytes());
            bytes.extend_from_slice(&r.ebx.to_le_bytes());
            bytes.extend_from_slice(&r.ecx.to_le_bytes());
            bytes.extend_from_slice(&r.edx.to_le_bytes());
        }
        brand = String::from_utf8_lossy(&bytes)
            .replace('\0', " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
    }

    let (mut base_mhz, mut max_mhz) = (0u32, 0u32);
    if max_basic >= 0x16 {
        let r = unsafe { __cpuid(0x16) };
        let base = r.eax & 0xffff;
        let max = r.ebx & 0xffff;
        if (50..=10_000).contains(&base) {
            base_mhz = base;
        }
        if (50..=20_000).contains(&max) {
            max_mhz = max;
        }
    }

    // 部分 AMD 处理器在 0x80000008 里给出 APIC 相关频率，这里只用 CPUID 0x16。
    let _ = unsafe { __cpuid_count(0, 0) };

    Some(CpuRegisters {
        vendor,
        vendor_id: [leaf0.ebx, leaf0.edx, leaf0.ecx],
        brand,
        base_mhz,
        max_mhz,
    })
}

#[cfg(not(target_arch = "x86_64"))]
fn cpuid_registers() -> Option<CpuRegisters> {
    None
}
