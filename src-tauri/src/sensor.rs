/// Hardware sensor module for SECM.
///
/// Pure Rust sensor architecture (no LHM bridge / no PawnIO driver):
///   CPU freq:   CallNtPowerInformation (powrprof.dll)
///   CPU usage:  sysinfo
///   GPU:        nvidia-smi (NVIDIA 卡; 非 NVIDIA/无驱动环境降级为空)
///   Memory:     GlobalMemoryStatusEx (kernel32) — authoritative
///   Storage:    sysinfo Disks
use serde::Serialize;
use std::mem::MaybeUninit;
use std::sync::Mutex;
use sysinfo::System;

use crate::debug_warn;

// ============================================================================
// Structures
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct CpuData {
    pub name: String,
    pub usage: f32,
    pub per_core: Vec<f32>,
    pub core_count: usize,
    pub clock_mhz: f32,
    /// 频率数据源: "ntapi" | "pdh" | "registry" | "sysinfo" | "none"（前端契约）
    pub freq_source: String,
    pub temperature: f32,
    pub power_w: f32,
    /// 温度数据源: "lhm" | "winring0" | "acpi" | "none"
    pub temp_source: String,
    /// 功耗数据源: "rapl" | "estimated" | "none"
    pub power_source: String,
    /// 功耗是否为估算值（true = 前端显示"估算"角标）
    pub power_estimated: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct GpuData {
    pub name: String,
    pub usage: f32,
    pub temperature: f32,
    pub memory_used: u64,
    pub memory_total: u64,
    pub clock_mhz: f32,
    pub power_w: f32,
}
#[derive(Debug, Clone, Serialize)]
pub struct MemoryData {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub usage_percent: f32,
    /// 内存型号（LHM SPD 一次性读取补充；无则空串，前端可不渲染）
    pub model_name: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct DiskData {
    pub name: String,
    pub total_space: u64,
    pub available_space: u64,
    pub used_space: u64,
    pub usage_percent: f32,
    pub read_mbps: f32,
    pub write_mbps: f32,
}
#[derive(Debug, Clone, Serialize)]
pub struct SensorData {
    pub cpu: CpuData,
    pub gpu: Vec<GpuData>,
    pub memory: MemoryData,
    pub disks: Vec<DiskData>,
    /// LHM 主板传感器（温度/风扇/电压分组；LHM 不可用或无数据时 None，前端不渲染）
    pub motherboard: Option<crate::lhm::LhmMotherboardData>,
    pub diag: String,
}

// ============================================================================
// WinAPI: CallNtPowerInformation (powrprof.dll) — CPU frequency
// ============================================================================

#[repr(C)]
struct ProcessorPowerInfo {
    number: u32,
    max_mhz: u32,
    current_mhz: u32,
    _limit: u32,
    _max_idle: u32,
    _cur_idle: u32,
}

/// 权威逻辑处理器数（`GetSystemInfo().dwNumberOfProcessors`）。
///
/// 替代 `std::thread::available_parallelism()`——后者在 Windows 上不受进程
/// affinity 影响，且可能低估 >64 逻辑处理器系统（F4 修复）。
/// 布局与 hardware.rs::authoritative_cpu_count 相同（已核实 x64 布局正确）。
fn authoritative_cpu_count() -> u32 {
    // SYSTEM_INFO 布局（x64）：union{dwOemId/struct{arch,reserved}} + 页大小 +
    // 最小/最大地址 + 活动处理器掩码 + dwNumberOfProcessors + ...
    #[repr(C)]
    struct SystemInfo {
        _union: u32,
        _page_size: u32,
        _min_address: *const u8,
        _max_address: *const u8,
        _active_mask: usize,
        dw_number_of_processors: u32,
        _processor_type: u32,
        _allocation_granularity: u32,
        _processor_level: u16,
        _processor_revision: u16,
    }
    // SAFETY: SystemInfo 为数值 POD，零值合法（指针字段为空且不 deref）
    let mut si: SystemInfo = unsafe { std::mem::zeroed() };
    extern "system" {
        fn GetSystemInfo(lpSystemInfo: *mut SystemInfo);
    }
    // SAFETY: GetSystemInfo 为无参导出，si 由 API 填充
    unsafe { GetSystemInfo(&mut si) };
    if si.dw_number_of_processors == 0 {
        1
    } else {
        si.dw_number_of_processors
    }
}

/// 通过 `CallNtPowerInformation(ProcessorInformation)` 采样 CPU 平均频率（MHz）。
///
/// 返回 `(平均当前频率, 平均最大频率)`；API 失败或频率全为 0 时返回 `(0.0, 0.0)`。
/// 平均 `max_mhz` 用于"疑似标称"判定（AMD 7800X3D 等平台 `current_mhz` 恒等于
/// 标称基频，此时 current ≈ max，见
/// `crate::datasource::cpu_freq::is_suspected_nominal`，v0.21.7 修复）。
fn cpu_freq_ntapi() -> (f32, f32) {
    // 缓冲区按 GetSystemInfo 权威逻辑处理器数分配（F4 修复）
    let count = authoritative_cpu_count();
    let sz = std::mem::size_of::<ProcessorPowerInfo>() * count as usize;
    let mut buf = vec![0u8; sz];
    let rc = unsafe {
        // SAFETY: input 为 NULL（无输入参数）；输出缓冲 buf 容量 = count × size_of::<ProcessorPowerInfo>，与 API 填充结构体数组匹配
        extern "system" {
            fn CallNtPowerInformation(
                info_level: i32,
                input: *const u8,
                input_sz: u32,
                output: *mut u8,
                output_sz: u32,
            ) -> i32;
        }
        CallNtPowerInformation(11, std::ptr::null(), 0, buf.as_mut_ptr(), sz as u32)
    };
    if rc != 0 {
        return (0.0, 0.0);
    }
    // SAFETY: 返回成功后缓冲含 count 个 ProcessorPowerInfo（缓冲区长度与结构体数组一致，由 count 保证）
    let infos: &[ProcessorPowerInfo] = unsafe {
        std::slice::from_raw_parts(buf.as_ptr() as *const ProcessorPowerInfo, count as usize)
    };
    let sum_current: u32 = infos.iter().map(|p| p.current_mhz).sum();
    if sum_current > 0 {
        // avg_max 与 avg_current 同源同除数，作为疑似标称判定的基准
        let sum_max: u32 = infos.iter().map(|p| p.max_mhz).sum();
        (
            sum_current as f32 / count as f32,
            sum_max as f32 / count as f32,
        )
    } else {
        (0.0, 0.0)
    }
}

// ============================================================================
// WinAPI: GlobalMemoryStatusEx (kernel32) — authoritative memory data
// ============================================================================

#[repr(C)]
struct MemoryStatusEx {
    length: u32,
    memory_load: u32,
    total_phys: u64,
    avail_phys: u64,
    _total_page: u64,
    _avail_page: u64,
    _total_virtual: u64,
    _avail_virtual: u64,
    _avail_ext: u64,
}

fn memory_status_ex() -> Option<MemoryData> {
    let mut ms = MaybeUninit::<MemoryStatusEx>::zeroed();
    unsafe {
        (*ms.as_mut_ptr()).length = std::mem::size_of::<MemoryStatusEx>() as u32;
        extern "system" {
            fn GlobalMemoryStatusEx(lpBuffer: *mut MemoryStatusEx) -> i32;
        }
        if GlobalMemoryStatusEx(ms.as_mut_ptr()) == 0 {
            return None;
        }
        let ms = ms.assume_init();
        Some(MemoryData {
            total: ms.total_phys,
            used: ms.total_phys - ms.avail_phys,
            available: ms.avail_phys,
            usage_percent: if ms.total_phys > 0 {
                ms.memory_load as f32
            } else {
                0.0
            },
            model_name: String::new(),
        })
    }
}

// ============================================================================
// CPU — WinAPI freq + sysinfo usage（温度由 lhm/WinRing0 数据源回填）
// ============================================================================

/// RAPL 功耗采样状态（上次计数 + 时间戳；进程级互斥，串行化差值计算）。
static CPU_POWER_STATE: Mutex<Option<(std::time::Instant, u64)>> = Mutex::new(None);

/// 经 WinRing0 RAPL 计算 CPU package 功耗（瓦；差值法）。
///
/// 首次采样仅记录基线返回 None；后续采样用能量计数差值 / 时间间隔计算功耗。
/// 通道不可用（驱动未加载/平台不支持）返回 None，调用方降级为 0。
fn cpu_power_rapl() -> Option<f32> {
    let (_energy, count, unit) =
        crate::driver_install::winring0::winring0_rapl_energy()?;
    let mut guard = CPU_POWER_STATE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, prev_count)) = guard.as_ref() {
        let dt = at.elapsed().as_secs_f64();
        if dt > 0.05 && dt < 10.0 {
            // 32 位能量计数回绕：wrapping_sub 取模差值（秒级采样最多回绕一次）
            let delta_count = (count.wrapping_sub(*prev_count)) & 0xFFFF_FFFF;
            let delta_j = delta_count as f64 * unit;
            *guard = Some((std::time::Instant::now(), count));
            return Some((delta_j / dt) as f32);
        }
    }
    *guard = Some((std::time::Instant::now(), count));
    None
}

// ============================================================================
// CPU 功耗估算（D3：RAPL 不可用时的降级方案）
// ============================================================================

/// (型号关键词, TDP 瓦)。顺序敏感：长型号必须排在短型号之前（如
/// "7950X3D" 在 "7950X" 前、"13900KS" 在 "13900K" 前），否则短关键词
/// 会抢先命中长型号字符串。KF/KS 后缀与 K 同 TDP，无需单列。
const CPU_TDP_TABLE: &[(&str, u32)] = &[
    // AMD Ryzen 7000 系列（Zen4 桌面）
    ("7950X3D", 120),
    ("7950X", 170),
    ("7900X3D", 120),
    ("7900X", 170),
    ("7900", 65),
    ("7800X3D", 120),
    ("7700X", 105),
    ("RYZEN 7 7700", 65),
    ("7600X", 105),
    ("7600", 65),
    ("7500F", 65),
    // AMD Ryzen 5000 系列（Zen3 桌面）
    ("5950X", 105),
    ("5900X", 105),
    ("5900", 65),
    ("5800X3D", 105),
    ("5800X", 105),
    ("5700X", 65),
    ("5600X", 65),
    ("5600G", 65),
    ("5600", 65),
    ("5500", 65),
    // Intel 13 代（Raptor Lake 桌面）
    ("13900KS", 125),
    ("13900K", 125),
    ("13900", 65),
    ("13700K", 125),
    ("13700", 65),
    ("13600K", 125),
    ("13600", 65),
    ("13400", 65),
    ("13100", 60),
    // Intel 14 代（Raptor Lake Refresh 桌面）
    ("14900KS", 125),
    ("14900K", 125),
    ("14900", 65),
    ("14700K", 125),
    ("14700", 65),
    ("14600K", 125),
    ("14500", 65),
    ("14400", 65),
    // ==== v1.16.0 扩充：X99/X79/全 Intel/全 AMD 平台兼容 ====
    // 匹配策略：contains 大小写不敏感 + 顺序敏感（长关键词必在短关键词前；
    // Xeon v3/v4 → v2 → v1（" 0" 注册表名格式）→ 裸型号兜底）
    // TDP 来源：Intel ARK / AMD 官方数据，secm-dev 2026-08-13 逐条核对
    // Intel 12 代（Alder Lake 桌面）
    ("12900KS", 150),
    ("12900K", 125),
    ("12900", 65),
    ("12700K", 125),
    ("12700", 65),
    ("12600K", 125),
    ("12400", 65),
    ("12100", 60),
    // 老 Intel 桌面/至尊（Sandy Bridge-E ~ Coffee Lake，X79/X99 及更早）
    ("6950X", 140),
    ("6900K", 140),
    ("6850K", 140),
    ("6800K", 140),
    ("5960X", 140),
    ("5930K", 140),
    ("5820K", 140),
    ("4930K", 130),
    ("4820K", 130),
    ("3960X", 130),
    ("3930K", 130),
    ("I7-3820", 130), // 带前缀避免误匹配移动版 3820QM
    ("9900K", 95),
    ("9700K", 95),
    ("8700K", 95),
    ("7700K", 91),
    ("6700K", 91),
    ("4790K", 88),
    ("4770K", 84),
    ("3770K", 77),
    ("2600K", 95),
    // Xeon E5-1600 v1~v4（X79/X99 工作站；v1/v2 = 130W，v3/v4 = 140W）
    ("E5-1680 V4", 140),
    ("E5-1680 V3", 140),
    ("E5-1680 V2", 130),
    ("E5-1680", 140),
    ("E5-1660 V4", 140),
    ("E5-1660 V3", 140),
    ("E5-1660 V2", 130),
    ("E5-1660 0", 130),
    ("E5-1660", 130),
    ("E5-1650 V4", 140),
    ("E5-1650 V3", 140),
    ("E5-1650 V2", 130),
    ("E5-1650 0", 130),
    ("E5-1650", 130),
    ("E5-1620 V4", 140),
    ("E5-1620 V3", 140),
    ("E5-1620 V2", 130),
    ("E5-1620 0", 130),
    ("E5-1620", 130),
    // Xeon E5-2600 v1~v4（X79/X99 服务器；每型号 V4→V3→V2→V1(" 0")→裸）
    ("E5-2699 V4", 145),
    ("E5-2699 V3", 145),
    ("E5-2699 V2", 130),
    ("E5-2699", 145),
    ("E5-2697 V4", 145),
    ("E5-2697 V3", 145),
    ("E5-2697 V2", 130),
    ("E5-2697", 145),
    ("E5-2690 V4", 135),
    ("E5-2690 V3", 135),
    ("E5-2690 V2", 130),
    ("E5-2690 0", 135),
    ("E5-2690", 135),
    ("E5-2687W V4", 160),
    ("E5-2687W V3", 160),
    ("E5-2687W 0", 150),
    ("E5-2687W", 150),
    ("E5-2680 V4", 120),
    ("E5-2680 V3", 120),
    ("E5-2680 V2", 115),
    ("E5-2680 0", 130),
    ("E5-2680", 130),
    ("E5-2670 V4", 120),
    ("E5-2670 V3", 120),
    ("E5-2670 V2", 115),
    ("E5-2670 0", 115),
    ("E5-2670", 115),
    ("E5-2660 V4", 105),
    ("E5-2660 V3", 105),
    ("E5-2660 V2", 95),
    ("E5-2660 0", 95),
    ("E5-2660", 95),
    ("E5-2650 V4", 105),
    ("E5-2650 V3", 105),
    ("E5-2650 V2", 95),
    ("E5-2650 0", 95),
    ("E5-2650", 95),
    ("E5-2640 V4", 90),
    ("E5-2640 V3", 90),
    ("E5-2640 V2", 95),
    ("E5-2640 0", 95),
    ("E5-2640", 95),
    ("E5-2637 V4", 135),
    ("E5-2637 V3", 135),
    ("E5-2637 V2", 130),
    ("E5-2637 0", 80),
    ("E5-2637", 80),
    ("E5-2630 V4", 85),
    ("E5-2630 V3", 85),
    ("E5-2630 V2", 80),
    ("E5-2630 0", 95),
    ("E5-2630", 95),
    ("E5-2623 V4", 85),
    ("E5-2623 V3", 105),
    ("E5-2623", 85),
    ("E5-2620 V4", 85),
    ("E5-2620 V3", 85),
    ("E5-2620 V2", 80),
    ("E5-2620 0", 95),
    ("E5-2620", 95),
    ("E5-2603 V4", 85),
    ("E5-2603 V3", 85),
    ("E5-2603 V2", 80),
    ("E5-2603 0", 80),
    ("E5-2603", 80),
    // AMD FX（Zambezi/Vishera，AM3+）：ProcessorNameString 为
    // "AMD FX(tm)-8350 Eight-Core Processor"（CPUID brand string 带 (tm)），
    // 故每型号列两个关键词（带/不带 (TM)，带 TM 在前）；E 后缀低功耗版先于非 E
    ("FX(TM)-9590", 220),
    ("FX-9590", 220),
    ("FX(TM)-9370", 220),
    ("FX-9370", 220),
    ("FX(TM)-8370E", 95),
    ("FX-8370E", 95),
    ("FX(TM)-8370", 125),
    ("FX-8370", 125),
    ("FX(TM)-8350", 125),
    ("FX-8350", 125),
    ("FX(TM)-8320E", 95),
    ("FX-8320E", 95),
    ("FX(TM)-8320", 125),
    ("FX-8320", 125),
    ("FX(TM)-8300", 95),
    ("FX-8300", 95),
    ("FX(TM)-6350", 125),
    ("FX-6350", 125),
    ("FX(TM)-6300", 95),
    ("FX-6300", 95),
    ("FX(TM)-6100", 95),
    ("FX-6100", 95),
    ("FX(TM)-4300", 95),
    ("FX-4300", 95),
    ("FX(TM)-4130", 125),
    ("FX-4130", 125),
    // AMD Phenom II（K10，AM3）
    ("1100T", 125),
    ("1090T", 125),
    ("1055T", 95),
    ("1045T", 95),
    ("X4 980", 125),
    ("X4 975", 125),
    ("X4 970", 125),
    ("X4 965", 125),
];

/// 从未知 CPU 型号匹配 TDP 表（纯逻辑，可注入测试）。
///
/// 匹配前转大写；返回 None = 表中无此型号（调用方用 65W 默认）。
fn match_tdp(cpu_brand: &str) -> Option<u32> {
    let upper = cpu_brand.to_ascii_uppercase();
    CPU_TDP_TABLE
        .iter()
        .find(|(key, _)| upper.contains(key))
        .map(|(_, tdp)| *tdp)
}

/// 读取 CPU 型号名（注册表 ProcessorNameString，零特权；sysinfo brand() 同源）。
fn read_processor_name() -> Option<String> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    match RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0") {
        Ok(k) => k.get_value::<String, _>("ProcessorNameString").ok(),
        Err(_) => None,
    }
}

/// 功耗估算纯逻辑：TDP × usage%（usage 截断到 [0, 100]）。
fn estimate_power(usage: f32, tdp_w: f32) -> f32 {
    (tdp_w * (usage.clamp(0.0, 100.0) / 100.0)).max(0.0)
}

/// 估算 CPU 功耗：标称 TDP × usage%。
///
/// 返回 (功耗瓦, TDP 是否来自已知型号表)；型号匹配失败用 65W 默认并返回 false
/// （"标注未知"：前端/日志可区分精确型号与兜底默认）。
/// 仅作参考值（±30% 级），调用方必须标记为估算（power_estimated=true）。
fn estimate_cpu_power(usage: f32) -> (f32, bool) {
    let tdp_known = read_processor_name().and_then(|n| match_tdp(&n));
    match tdp_known {
        Some(tdp) => (estimate_power(usage, tdp as f32), true),
        None => (estimate_power(usage, 65.0), false),
    }
}

/// 获取 CPU 数据；温度回退链：LHM → WinRing0 SMN（AMD Zen）→ ACPI 热区 → 0；
/// 功耗：LHM RAPL 实测 → WinRing0 RAPL 差值法（AMD Zen）→ TDP×usage 估算并标记。
/// 数据源标记（temp_source/power_source/power_estimated）为前端 T3 契约。
///
/// v1.17.0：LHM 温度/功耗改为参数注入（get_sensor_data 每周期先调 `lhm_snapshot()`
/// 取全量快照后传入，避免本函数内二次 HTTP 轮询）；`lhm_diag_src` 为快照诊断串
/// （LHM 不可用时的降级原因，透出到 diag 的 [lhm:...] 注记）。
pub fn get_cpu_data(
    sys: &System,
    lhm_temp: Option<f32>,
    lhm_power: Option<f32>,
    lhm_diag_src: &str,
) -> (CpuData, String) {
    let cpu = sys.cpus();
    let name = cpu
        .first()
        .map(|c| c.brand().to_string())
        .unwrap_or_else(|| "Unknown CPU".into());
    let core_count = cpu.len();
    let mut diag = String::new();

    // 频率：CallNtPowerInformation 为主（实时）；"疑似标称"判定（v0.21.7）——
    // AMD 7800X3D 等平台该 API 的 current_mhz 恒等于标称基频（不随负载变化），
    // 此时 current ≈ max_mhz，判定为疑似标称 → 不直接采用，继续走 datasource
    // 降级链（PDH 实时估算 → 注册表标称，与主路径不共用失败点），再失败用
    // sysinfo 当前频率兜底。freq_source 为前端契约（估算/标称角标区分）。
    // 频率主链路：ntapi 实时值；"疑似标称"判定（AMD 恒标称平台）→ datasource
    // 降级链（PDH 实时估算 → 注册表标称）；datasource 全失败 → sysinfo 兜底。
    // 返回 (频率, 数据源, 诊断原因串)——原因串仅在疑似标称兜底/全失败时透出。
    let (ntapi_freq, ntapi_max) = cpu_freq_ntapi();
    let (clock_mhz, freq_source, freq_diag): (f32, &str, String) =
        if ntapi_freq > 0.0
            && !crate::datasource::cpu_freq::is_suspected_nominal(ntapi_freq, ntapi_max)
        {
            // 真实实时值（Intel 平台主路径）：current 随负载变化，非疑似标称
            (ntapi_freq, "ntapi", String::new())
        } else {
            // 疑似标称（或主 API 失败）→ datasource 降级链 + 诊断透出（T2 契约）：
            // 一次调用同时取频率、数据源与各层失败原因。datasource 内部已做
            // "ntapi 疑似标称 → PDH 实时估算"判定，PDH 失败时回退 ntapi 标称值
            // 并带诊断标注（不劣化现状）。
            let (fallback, src, freq_diag) = crate::datasource::cpu_freq::get_cpu_freq_diag();
            match fallback {
                Some(f) => (f, src, freq_diag),
                None => {
                    // 注意：sysinfo 0.30.13 的 Windows 频率实现内部同样调用
                    // CallNtPowerInformation(ProcessorInformation)，与上方主路径同源——
                    // 主路径失败时此兜底大概率同样返回 0（伪兜底，仅防御实现差异/瞬时抖动）。
                    // 真正独立的兜底是 datasource 用户态降级链（PDH/registry，见上）。
                    let f = cpu.first().map(|c| c.frequency() as f32).unwrap_or(0.0);
                    if f > 0.0 {
                        (f, "sysinfo", String::new())
                    } else {
                        // 全链路失败：原因串（API 名+错误码）透出供前端诊断摘要
                        (0.0, "none", freq_diag)
                    }
                }
            }
        };
    // diag 透出：疑似标称兜底标注"标称"；全失败透出各层失败原因
    if clock_mhz > 0.0 {
        if freq_source == "ntapi" && !freq_diag.is_empty() {
            // datasource 疑似标称兜底路径（diag 带"ntapi疑似标称"原因）：
            // 标注"标称"，与真实 ntapi 实时值区分（不丢失诊断信息）
            diag.push_str(&format!("FREQ={:.0}(ntapi,标称) ", clock_mhz));
        } else {
            diag.push_str(&format!("FREQ={:.0}({}) ", clock_mhz, freq_source));
        }
    } else {
        diag.push_str("FREQ=0(none) ");
        if !freq_diag.is_empty() {
            diag.push_str(&format!("原因:{freq_diag}"));
        }
    }

    let usage = sys.global_cpu_info().cpu_usage();
    let per_core: Vec<f32> = cpu.iter().map(|c| c.cpu_usage()).collect();
    diag.push_str(&format!("USAGE={:.1}%", usage));

    // 温度：LHM sidecar（进程隔离，最高优先级）→ WinRing0 SMN（AMD Zen，毫℃）→
    // ACPI 热区（纯用户态兜底）→ 无数据
    let mut temperature = 0.0_f32;
    let mut temp_source = "none";
    // LHM sidecar：进程隔离的 LibreHardwareMonitor 数据源（PawnIO 驱动，需管理员提权）；
    // 温度由 get_sensor_data 每周期取全量快照后注入（避免本函数内二次 HTTP 轮询）；
    // 不可用时不劣化原链，失败原因附到 diag（lhm_note）
    let mut lhm_note = String::new();
    if let Some(t) = lhm_temp {
        if t > 0.0 {
            temperature = t;
            temp_source = "lhm";
            diag.push_str(&format!(" TEMP={:.1}℃(lhm)", temperature));
        } else if !lhm_diag_src.is_empty() {
            lhm_note = format!("[lhm:{}]", lhm_diag_src);
        }
    } else if !lhm_diag_src.is_empty() {
        lhm_note = format!("[lhm:{}]", lhm_diag_src);
    }
    // WinRing0 SMN（AMD Zen，毫℃）
    if temperature <= 0.0 {
        if let Some(mc) = crate::driver_install::winring0::winring0_cpu_temp() {
            temperature = mc as f32 / 1000.0;
            temp_source = "winring0";
            diag.push_str(&format!(" TEMP={:.1}℃(winring0)", temperature));
        }
    }
    // ACPI 热区（纯用户态）
    if temperature <= 0.0 {
        if let Some(c) = crate::driver_install::acpi::thermal_zone_temp() {
            temperature = c as f32;
            temp_source = "acpi";
            diag.push_str(&format!(" TEMP={:.1}℃(acpi)", temperature));
        }
    }
    if temperature <= 0.0 {
        diag.push_str(&format!(" TEMP=n/a{}", lhm_note));
        debug_warn!(
            "sensor",
            "CPU temp unavailable (lhm/WinRing0/ACPI 全通道不可用，可能被杀软拦截)"
        );
    } else if !lhm_note.is_empty() {
        // LHM 通道失败但温度已被兜底源（winring0/acpi）填补时，失败原因仍必须
        // 透出到 diag——否则用户无法感知 LHM 通道失效（目标机「未见 lhm 诊断」的
        // 直接原因，v1.17.0 修复：诊断信息不因降级成功而被静默）
        diag.push_str(&format!(" {}", lhm_note));
    }

    // 功耗：LHM RAPL 实测（Intel/AMD 全覆盖，power_source="lhm"，前端同 rapl 实测
    // 语义不显示「估算」角标）→ WinRing0 RAPL 差值法（AMD Zen package）→ 不可用时
    // TDP×usage 估算（D3）
    let (power_w, power_source, power_estimated) = match lhm_power {
        Some(p) if p > 0.0 => {
            diag.push_str(&format!(" PWR={:.1}W(lhm)", p));
            (p, "lhm", false)
        }
        _ => match cpu_power_rapl() {
            Some(p) if p > 0.0 => {
                diag.push_str(&format!(" PWR={:.1}W(rapl)", p));
                (p, "rapl", false)
            }
            _ => {
                let (est, tdp_known) = estimate_cpu_power(usage);
                if tdp_known {
                    diag.push_str(&format!(" PWR≈{:.1}W(estimated)", est));
                } else {
                    diag.push_str(&format!(" PWR≈{:.1}W(estimated,未知型号默认65W)", est));
                }
                (est, "estimated", true)
            }
        },
    };

    (
        CpuData {
            usage,
            per_core,
            name,
            core_count,
            clock_mhz,
            freq_source: freq_source.to_string(),
            temperature,
            power_w,
            temp_source: temp_source.to_string(),
            power_source: power_source.to_string(),
            power_estimated,
        },
        diag,
    )
}

// ============================================================================
// GPU — nvidia-smi 真实采集（NVIDIA 卡；非 NVIDIA/无驱动环境降级为空，不中断）
// ============================================================================

/// nvidia-smi 查询超时；驱动挂起时避免传感器轮询无限等待。
const NVIDIA_SMI_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// GPU 数据缓存有效期：Dashboard 每秒轮询，nvidia-smi 冷启动 100-300ms，
/// 缓存 2 秒显著降低子进程开销（温度/功耗秒级无感知差异）。
const GPU_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(2);

/// nvidia-smi 查询结果缓存（进程级；锁内串行化，避免并发多个 nvidia-smi 子进程）。
static GPU_CACHE: Mutex<Option<(std::time::Instant, Vec<GpuData>)>> = Mutex::new(None);

/// 带超时的 nvidia-smi 查询。
///
/// 查询字段: index,name,temperature.gpu,utilization.gpu,clocks.sm,power.draw,
/// memory.used,memory.total（csv,noheader,nounits；名称可含逗号，故首字段取 index、
/// 末尾 6 个数值字段从后向前解析，中间 join 为名称）。
///
/// 返回 Ok(Some(gpus)) = 成功;Ok(None) = nvidia-smi 不存在/不可执行/超时
/// （非 NVIDIA 环境属预期降级，不算错误）;Err = 调用/解析失败。
fn query_nvidia_gpus() -> Result<Option<Vec<GpuData>>, String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    let mut child = Command::new("nvidia-smi")
        .args([
            "--query-gpu=index,name,temperature.gpu,utilization.gpu,clocks.sm,power.draw,memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ])
        // 显式 piped：spawn() 默认继承 stdout，wait_with_output 捕获不到内容，
        // 导致解析空文本而降级（与 hardware.rs 同一坑，实测 nvidia-smi 输出漏打到父进程 stdout）
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // CREATE_NO_WINDOW：禁止子进程创建控制台窗口（硬件信息页每秒轮询时防闪 cmd 窗）
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|e| format!("nvidia-smi 启动失败: {e}"))?;

    // 轮询等待 + 超时 kill（驱动挂起时避免无限等待）
    let deadline = std::time::Instant::now() + NVIDIA_SMI_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(None); // 超时：视为无数据（降级）
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(format!("nvidia-smi 等待失败: {e}")),
        }
    };
    if !status.success() {
        // 退出码非 0：无 NVIDIA 驱动/无 NVIDIA GPU（AMD/Intel 平台常见），预期降级
        return Ok(None);
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("nvidia-smi 输出读取失败: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut gpus = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // CSV 字段: index, name, temperature, utilization, clocks.sm, power, mem.used, mem.total
        let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if parts.len() < 8 {
            debug_warn!("sensor", "nvidia-smi 输出格式异常(字段不足): {line}");
            continue;
        }
        let parse_f = |s: &str| s.parse::<f64>().ok().unwrap_or(0.0);
        let index = parts[0].parse::<u32>().unwrap_or(0);
        let name = parts[1..parts.len() - 6].join(", ");
        let temperature = parse_f(parts[parts.len() - 6]) as f32;
        let usage = parse_f(parts[parts.len() - 5]) as f32;
        let clock_mhz = parse_f(parts[parts.len() - 4]) as f32;
        let power_w = parse_f(parts[parts.len() - 3]) as f32;
        let mem_used_mib = parse_f(parts[parts.len() - 2]);
        let mem_total_mib = parse_f(parts[parts.len() - 1]);
        gpus.push(GpuData {
            name,
            usage,
            temperature,
            memory_used: (mem_used_mib * 1024.0 * 1024.0) as u64,
            memory_total: (mem_total_mib * 1024.0 * 1024.0) as u64,
            clock_mhz,
            power_w,
        });
        debug_warn!("sensor", "nvidia-smi 解析 GPU#{index} 成功");
    }
    if gpus.is_empty() {
        Ok(None)
    } else {
        Ok(Some(gpus))
    }
}

/// 获取 GPU 数据（nvidia-smi 真实采集；2 秒缓存；无 NVIDIA 环境返回空 = 降级）。
/// 返回 (gpus, 诊断来源标记)。
pub fn get_gpu_data() -> (Vec<GpuData>, &'static str) {
    let mut guard = GPU_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, cached)) = guard.as_ref() {
        if at.elapsed() < GPU_CACHE_TTL {
            return (cached.clone(), "nvidia-smi(cache)");
        }
    }
    match query_nvidia_gpus() {
        Ok(Some(gpus)) => {
            *guard = Some((std::time::Instant::now(), gpus.clone()));
            (gpus, "nvidia-smi")
        }
        Ok(None) => {
            guard.take();
            (Vec::new(), "none")
        }
        Err(e) => {
            debug_warn!("sensor", "GPU 采集失败: {e}");
            guard.take();
            (Vec::new(), "none")
        }
    }
}

/// 把 LHM GPU 传感器映射为 GpuData（数据源 "lhm"）。
///
/// 可空字段 → 0 兜底（前端 "—" 语义）；字段契约见 lhm.rs LhmGpuData。
/// 独立函数便于单测（字段映射为纯逻辑）。
fn map_lhm_gpu(g: &crate::lhm::LhmGpuData) -> GpuData {
    GpuData {
        name: g.name.clone(),
        usage: g.load_percent.unwrap_or(0.0),
        temperature: g.temperature_c.unwrap_or(0.0),
        memory_used: g.memory_used_bytes.unwrap_or(0),
        memory_total: g.memory_total_bytes.unwrap_or(0),
        clock_mhz: g.core_clock_mhz.unwrap_or(0.0),
        power_w: g.power_w.unwrap_or(0.0),
    }
}

// ============================================================================
// Memory — GlobalMemoryStatusEx — sysinfo fallback
// ============================================================================

pub fn get_memory_data() -> MemoryData {
    memory_status_ex().unwrap_or_else(|| {
        // System::new() 避免 new_all() 的完整进程枚举（~10-50MB），仅需内存数据
        let mut sys = System::new();
        sys.refresh_memory();
        let t = sys.total_memory();
        MemoryData {
            total: t,
            used: sys.used_memory(),
            available: sys.available_memory(),
            usage_percent: if t > 0 {
                (sys.used_memory() as f32 / t as f32) * 100.0
            } else {
                0.0
            },
            model_name: String::new(),
        }
    })
}

// ============================================================================
// Storage — sysinfo（每 60 次调用重建实例，防止 refresh_list 内部累积）
//
// sysinfo v0.30 的 Disks::refresh_list() 在 Windows 上可能追加而非替换旧条目
// ============================================================================

static DISKS_INSTANCE: std::sync::LazyLock<Mutex<(sysinfo::Disks, u32)>> =
    std::sync::LazyLock::new(|| Mutex::new((sysinfo::Disks::new_with_refreshed_list(), 0)));

/// Disks 实例重建间隔（调用次数），防止 refresh_list 内部条目累积
const DISKS_REBUILD_INTERVAL: u32 = 60;

pub fn get_disk_data() -> Vec<DiskData> {
    let mut guard = DISKS_INSTANCE.lock().unwrap_or_else(|p| p.into_inner());
    let (disks, call_count) = &mut *guard;
    *call_count += 1;

    // 周期性重建 Disks 实例，防止 refresh_list 内部条目累积
    if *call_count >= DISKS_REBUILD_INTERVAL {
        *disks = sysinfo::Disks::new_with_refreshed_list();
        *call_count = 0;
    } else {
        disks.refresh_list();
    }
    // 各卷磁盘读写速度（PDH 通配计数器按卷独立采样，任务管理器同源）；
    // 首次调用为采样基线返回空映射，此后每秒返回最近 ~1s 平均速率。
    // 键为盘符（"C:" 大写），与 mount_point() 归一化后按卷匹配。
    let io_map = crate::datasource::disk_io::get_disk_io_speed_map();
    disks
        .list()
        .iter()
        .map(|d| {
            let t = d.total_space();
            let a = d.available_space();
            let u = t.saturating_sub(a);
            // mount_point 形如 "D:\\" → 归一化为 "D:" 与 PDH 实例匹配
            let mount = d.mount_point().to_string_lossy();
            let drive_key = mount.trim_end_matches('\\').to_ascii_uppercase();
            let (read_mbps, write_mbps) =
                io_map.get(&drive_key).copied().unwrap_or((0.0, 0.0));
            DiskData {
                name: d.name().to_string_lossy().to_string(),
                total_space: t,
                available_space: a,
                used_space: u,
                usage_percent: if t > 0 {
                    (u as f32 / t as f32) * 100.0
                } else {
                    0.0
                },
                read_mbps,
                write_mbps,
            }
        })
        .collect()
}

// ============================================================================
// Snapshot
// ============================================================================

/// 仅用 System::new() 而非 new_all() — 避免持久化全量进程快照（10-50MB 浪费）
/// 我们只需要 refresh_cpu() / refresh_memory()，不需要进程列表
static SYS_INSTANCE: Mutex<Option<System>> = Mutex::new(None);

pub fn get_sensor_data() -> SensorData {
    let mut g = SYS_INSTANCE.lock().unwrap_or_else(|p| p.into_inner());
    // System::new() 仅分配 CPU + 内存组件，不枚举全部进程（节省 10-50MB）
    let s = g.get_or_insert_with(System::new);
    s.refresh_cpu();
    s.refresh_memory();

    // LHM 全量快照：唯一 HTTP 轮询入口（每采集周期 1 次，CPU/GPU/主板/内存共享，
    // 避免各数据源独立 HTTP 的 3 倍开销）。快照内含 CPU 温度/功耗，直接注入
    // get_cpu_data（其内部 lhm_cpu_temp() 薄封装命中 last_snapshot 缓存，零额外 HTTP）。
    let (lhm_snap, lhm_diag) = crate::lhm::lhm_snapshot();
    let lhm_temp = lhm_snap
        .as_ref()
        .and_then(|r| r.cpu.package_temp_c)
        .filter(|t| *t > 0.0);
    let lhm_power = lhm_snap
        .as_ref()
        .and_then(|r| r.cpu.power_w)
        .filter(|p| *p > 0.0);

    let (cpu, cpu_diag) = get_cpu_data(s, lhm_temp, lhm_power, &lhm_diag);

    // GPU 主源：LHM gpu[]（NVAPI/ADL 用户态，覆盖 NVIDIA/AMD/Intel）→ 空/不可用
    // 时降级 nvidia-smi（既有 get_gpu_data 保留，AMD/Intel 卡无数据返回空）。
    let (gpu, gpu_src): (Vec<GpuData>, &'static str) = match lhm_snap.as_ref() {
        Some(r) if !r.gpu.is_empty() => {
            let mapped: Vec<GpuData> = r.gpu.iter().map(map_lhm_gpu).collect();
            (mapped, "lhm")
        }
        _ => get_gpu_data(),
    };

    // 主板传感器：LHM 快照透出（温度/风扇/电压分组）；LHM 不可用/无数据 → None
    let motherboard = lhm_snap.as_ref().and_then(|r| r.motherboard.clone());

    drop(g); // release sys

    let mut mem = get_memory_data();
    // 内存型号：LHM SPD 补充（sidecar 一次性读取缓存，不每轮 SMBus；无则保持空串）
    if mem.model_name.is_empty() {
        if let Some(m) = lhm_snap.as_ref().and_then(|r| r.memory.as_ref()) {
            if let Some(n) = m.name.as_deref() {
                if !n.is_empty() {
                    mem.model_name = n.to_string();
                }
            }
        }
    }
    let disks = get_disk_data();

    let mb_tag = match motherboard.as_ref() {
        Some(mb) => format!("mb={}传感器", mb.sensors.len()),
        None => "mb=n/a".to_string(),
    };
    let diag = format!("GPU={} {} mem={:.0}% {}", gpu_src, cpu_diag, mem.usage_percent, mb_tag);
    crate::temp_data::sensor_diag(&diag);

    SensorData {
        cpu,
        gpu,
        memory: mem,
        disks,
        motherboard,
        diag,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_cpu() {
        let mut s = System::new_all();
        s.refresh_all();
        let (c, _) = get_cpu_data(&s, None, None, "");
        assert!(!c.name.is_empty());
        // 降级语义：数据源不可用时不 panic、温度为非负值
        // （WinRing0 回退链可能提供真实温度；
        //   v0.17.5 驱动自愈后本机 WinRing0 可用，温度不再恒为 0）
        assert!(c.temperature >= 0.0);
        assert!(c.clock_mhz >= 0.0);
        assert!(c.power_w >= 0.0);
        // freq_source 枚举契约（前端依赖）：固定值集合
        assert!(
            ["ntapi", "pdh", "registry", "sysinfo", "none"].contains(&c.freq_source.as_str()),
            "freq_source 非法: {}",
            c.freq_source
        );
        // 数据源契约（前端 T3 依赖）：枚举值固定、温度有值时 temp_source 非 none
        assert!(
            ["lhm", "winring0", "acpi", "none"].contains(&c.temp_source.as_str()),
            "temp_source 非法: {}",
            c.temp_source
        );
        assert!(
            ["lhm", "rapl", "estimated", "none"].contains(&c.power_source.as_str()),
            "power_source 非法: {}",
            c.power_source
        );
        if c.temperature > 0.0 {
            assert_ne!(c.temp_source, "none", "有温度时数据源不应为 none");
        }
    }

    #[test]
    fn test_cpu_lhm_power_source() {
        // LHM 功耗注入：power_w>0 时 power_source="lhm" 且 power_estimated=false
        // （前端同 rapl 实测语义，不显示「估算」角标）
        let mut s = System::new_all();
        s.refresh_all();
        let (c, diag) = get_cpu_data(&s, Some(61.5), Some(42.7), "");
        assert_eq!(c.temp_source, "lhm", "LHM 温度注入后 temp_source 应为 lhm");
        assert_eq!(c.power_source, "lhm", "LHM 功耗注入后 power_source 应为 lhm");
        assert!(!c.power_estimated, "LHM 实测功耗不应标记估算");
        assert_eq!(c.temperature, 61.5);
        assert_eq!(c.power_w, 42.7);
        assert!(diag.contains("(lhm)"), "diag 应标注 lhm 数据源: {}", diag);

        // LHM 功耗为 0/None 时降级到既有 rapl/estimated 链（不 panic、不强制 lhm）
        let (c2, _) = get_cpu_data(&s, None, None, "");
        assert!(
            ["rapl", "estimated", "none"].contains(&c2.power_source.as_str()),
            "LHM 缺失时功耗源应为降级值: {}",
            c2.power_source
        );
    }

    #[test]
    fn test_cpu_lhm_diag_note() {
        // LHM 不可用诊断串透出：lhm_diag_src 非空且无温度时，diag 应含 [lhm:...] 注记
        let mut s = System::new_all();
        s.refresh_all();
        let (c, diag) = get_cpu_data(&s, None, None, "lhm(轮询失败:timeout)");
        assert!(c.temp_source != "lhm" || c.temperature <= 0.0);
        assert!(diag.contains("[lhm:lhm(轮询失败:timeout)]"), "diag 应透出 LHM 失败原因: {}", diag);
    }

    #[test]
    fn test_lhm_gpu_mapping() {
        // LHM GPU 传感器 → GpuData 字段映射（load_percent→usage、core_clock_mhz→clock_mhz 等）
        let g = crate::lhm::LhmGpuData {
            name: "NVIDIA GeForce GTX 1660 Ti".to_string(),
            temperature_c: Some(52.3),
            core_clock_mhz: Some(1500.0),
            power_w: Some(85.2),
            load_percent: Some(45.0),
            memory_used_bytes: Some(1073741824),
            memory_total_bytes: Some(6442450944),
            fan_rpm: Some(1200.0),
        };
        let out = map_lhm_gpu(&g);
        assert_eq!(out.name, "NVIDIA GeForce GTX 1660 Ti");
        assert_eq!(out.usage, 45.0);
        assert_eq!(out.temperature, 52.3);
        assert_eq!(out.clock_mhz, 1500.0);
        assert_eq!(out.power_w, 85.2);
        assert_eq!(out.memory_used, 1073741824);
        assert_eq!(out.memory_total, 6442450944);

        // 可空字段 → 0 兜底（前端 "—" 语义，Intel 核显无功耗等场景）
        let g2 = crate::lhm::LhmGpuData {
            name: "AMD Radeon Graphics".to_string(),
            ..Default::default()
        };
        let out2 = map_lhm_gpu(&g2);
        assert_eq!(out2.usage, 0.0);
        assert_eq!(out2.temperature, 0.0);
        assert_eq!(out2.power_w, 0.0);
        assert_eq!(out2.memory_used, 0);
        assert!(out2.name.contains("AMD"));
    }

    #[test]
    fn test_memory_model_name_default() {
        // MemoryData.model_name 默认空串（无 LHM SPD 时前端不渲染）
        let m = get_memory_data();
        assert!(m.total > 0);
        assert!(m.model_name.is_empty(), "无 LHM 注入时型号应为空串");
    }

    #[test]
    fn tdp_table_known_models() {
        // 常见型号精确匹配（AMD Ryzen 7000/5000 + Intel 13/14 代）
        assert_eq!(match_tdp("AMD Ryzen 9 7950X3D 16-Core Processor"), Some(120));
        assert_eq!(match_tdp("AMD Ryzen 9 7950X 16-Core Processor"), Some(170));
        assert_eq!(match_tdp("AMD Ryzen 9 7900X 12-Core Processor"), Some(170));
        assert_eq!(match_tdp("AMD Ryzen 7 7800X3D 8-Core Processor"), Some(120));
        assert_eq!(match_tdp("AMD Ryzen 5 7600X 6-Core Processor"), Some(105));
        assert_eq!(match_tdp("AMD Ryzen 9 5950X 16-Core Processor"), Some(105));
        assert_eq!(match_tdp("AMD Ryzen 7 5800X3D 8-Core Processor"), Some(105));
        assert_eq!(match_tdp("AMD Ryzen 5 5600X 6-Core Processor"), Some(65));
        assert_eq!(
            match_tdp("Intel(R) Core(TM) i9-14900K CPU @ 3.20GHz"),
            Some(125)
        );
        assert_eq!(match_tdp("Intel(R) Core(TM) i7-14700K CPU @ 3.40GHz"), Some(125));
        assert_eq!(match_tdp("Intel(R) Core(TM) i5-13600K CPU @ 3.50GHz"), Some(125));
        assert_eq!(match_tdp("Intel(R) Core(TM) i9-13900 CPU @ 2.00GHz"), Some(65));
        assert_eq!(match_tdp("Intel(R) Core(TM) i3-13100 CPU @ 3.40GHz"), Some(60));
    }

    #[test]
    fn tdp_table_order_sensitive() {
        // 长型号必须优先：7950X3D 不能被 7950X 抢先；13900KS 不能被 13900K 抢先
        assert_eq!(match_tdp("AMD Ryzen 9 7950X3D"), Some(120), "7950X3D 应匹配 120");
        assert_eq!(
            match_tdp("Intel(R) Core(TM) i9-13900KS CPU @ 3.20GHz"),
            Some(125),
            "13900KS 应匹配 125"
        );
        assert_eq!(match_tdp("Intel(R) Core(TM) i9-13900KF CPU @ 3.00GHz"), Some(125));
    }

    #[test]
    fn tdp_table_unknown_defaults() {
        // 未知/非桌面型号 → None（调用方用 65W 默认）
        assert_eq!(match_tdp("Unknown CPU"), None);
        assert_eq!(match_tdp("Apple Silicon"), None);
        assert_eq!(match_tdp(""), None);
    }

    #[test]
    fn tdp_table_xeon_e5_models() {
        // Xeon E5 v1~v4（X79/X99）：各代独立 TDP（Intel ARK 官方数据）
        assert_eq!(
            match_tdp("Intel(R) Xeon(R) CPU E5-2690 v4 @ 2.10GHz"),
            Some(135)
        );
        assert_eq!(
            match_tdp("Intel(R) Xeon(R) CPU E5-2690 v3 @ 2.60GHz"),
            Some(135)
        );
        // v2 顺序敏感：必须命中 V2（130W）而非 V3/V4/裸词
        assert_eq!(
            match_tdp("Intel(R) Xeon(R) CPU E5-2690 v2 @ 3.00GHz"),
            Some(130),
            "E5-2690 v2 应匹配 130W（V2 关键词必须在 V3 后、裸词前）"
        );
        // v1 注册表名格式带 " 0"：命中 v1 条目（135W）
        assert_eq!(
            match_tdp("Intel(R) Xeon(R) CPU E5-2690 0 @ 2.90GHz"),
            Some(135)
        );
        // 裸型号兜底（135W）
        assert_eq!(match_tdp("Intel(R) Xeon(R) CPU E5-2690"), Some(135));
        // E5-1650：v3/v4 = 140W，v1/v2 = 130W（X79 平台）
        assert_eq!(
            match_tdp("Intel(R) Xeon(R) CPU E5-1650 v4 @ 3.60GHz"),
            Some(140)
        );
        assert_eq!(
            match_tdp("Intel(R) Xeon(R) CPU E5-1650 v3 @ 3.50GHz"),
            Some(140)
        );
        assert_eq!(
            match_tdp("Intel(R) Xeon(R) CPU E5-1650 0 @ 3.20GHz"),
            Some(130)
        );
        // 低端型号代表
        assert_eq!(
            match_tdp("Intel(R) Xeon(R) CPU E5-2603 v4 @ 3.30GHz"),
            Some(85)
        );
        assert_eq!(
            match_tdp("Intel(R) Xeon(R) CPU E5-2630 0 @ 2.30GHz"),
            Some(95)
        );
    }

    #[test]
    fn tdp_table_xeon_order_sensitive() {
        // 长关键词优先验证：2697（v2=130 vs v3/v4=145）
        assert_eq!(
            match_tdp("Intel(R) Xeon(R) CPU E5-2697 v4 @ 2.30GHz"),
            Some(145)
        );
        assert_eq!(
            match_tdp("Intel(R) Xeon(R) CPU E5-2697 v2 @ 2.70GHz"),
            Some(130)
        );
        // 2687W：v3=160 vs v1(" 0")=150
        assert_eq!(
            match_tdp("Intel(R) Xeon(R) CPU E5-2687W v3 @ 3.10GHz"),
            Some(160)
        );
        assert_eq!(
            match_tdp("Intel(R) Xeon(R) CPU E5-2687W 0 @ 3.10GHz"),
            Some(150)
        );
    }

    #[test]
    fn tdp_table_amd_legacy_models() {
        // AMD FX（AM3+）
        assert_eq!(match_tdp("AMD FX(tm)-8350 Eight-Core Processor"), Some(125));
        assert_eq!(match_tdp("AMD FX(tm)-9590 Eight-Core Processor"), Some(220));
        assert_eq!(match_tdp("AMD FX(tm)-4130 Quad-Core Processor"), Some(125));
        // E 后缀顺序敏感：8370E(95W) 必须命中 E 条目而非 8370(125W)
        assert_eq!(match_tdp("AMD FX(tm)-8370E Eight-Core Processor"), Some(95));
        assert_eq!(match_tdp("AMD FX(tm)-8320E Eight-Core Processor"), Some(95));
        assert_eq!(match_tdp("AMD FX(tm)-8370 Eight-Core Processor"), Some(125));
        // Phenom II X6 / X4（K10）
        assert_eq!(match_tdp("AMD Phenom(tm) II X6 1090T Processor"), Some(125));
        assert_eq!(match_tdp("AMD Phenom(tm) II X6 1055T Processor"), Some(95));
        assert_eq!(match_tdp("AMD Phenom(tm) II X4 965 Processor"), Some(125));
    }

    #[test]
    fn tdp_table_intel_12th_gen() {
        // 12 代顺序敏感：KS(150) → K(125) → 裸(65)
        assert_eq!(
            match_tdp("Intel(R) Core(TM) i9-12900KS CPU @ 3.40GHz"),
            Some(150)
        );
        assert_eq!(
            match_tdp("Intel(R) Core(TM) i9-12900K CPU @ 3.20GHz"),
            Some(125)
        );
        assert_eq!(match_tdp("Intel(R) Core(TM) i9-12900 CPU @ 2.40GHz"), Some(65));
        assert_eq!(
            match_tdp("Intel(R) Core(TM) i7-12700K CPU @ 3.60GHz"),
            Some(125)
        );
        assert_eq!(
            match_tdp("Intel(R) Core(TM) i5-12600K CPU @ 3.70GHz"),
            Some(125)
        );
        assert_eq!(match_tdp("Intel(R) Core(TM) i5-12400 CPU @ 2.50GHz"), Some(65));
        assert_eq!(match_tdp("Intel(R) Core(TM) i3-12100 CPU @ 3.30GHz"), Some(60));
    }

    #[test]
    fn tdp_table_intel_legacy_models() {
        // 老 Intel 桌面/至尊（X79/X99 及更早）
        assert_eq!(match_tdp("Intel(R) Core(TM) i7-5960X CPU @ 3.00GHz"), Some(140));
        assert_eq!(match_tdp("Intel(R) Core(TM) i7-5820K CPU @ 3.30GHz"), Some(140));
        assert_eq!(match_tdp("Intel(R) Core(TM) i7-3960X CPU @ 3.30GHz"), Some(130));
        assert_eq!(match_tdp("Intel(R) Core(TM) i7-4790K CPU @ 4.00GHz"), Some(88));
        assert_eq!(match_tdp("Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz"), Some(91));
        assert_eq!(match_tdp("Intel(R) Core(TM) i9-9900K CPU @ 3.60GHz"), Some(95));
        assert_eq!(match_tdp("Intel(R) Core(TM) i7-2600K CPU @ 3.40GHz"), Some(95));
    }

    #[test]
    fn estimate_power_formula() {
        // 功耗 = TDP × usage/100；usage 截断 [0,100]
        assert!((estimate_power(50.0, 170.0) - 85.0).abs() < 1e-6);
        assert!((estimate_power(0.0, 65.0) - 0.0).abs() < 1e-6);
        assert!((estimate_power(100.0, 120.0) - 120.0).abs() < 1e-6);
        // usage 越界截断
        assert!((estimate_power(150.0, 65.0) - 65.0).abs() < 1e-6);
        assert!((estimate_power(-10.0, 65.0) - 0.0).abs() < 1e-6);
        // 未知型号默认 65W
        assert!((estimate_power(50.0, 65.0) - 32.5).abs() < 1e-6);
    }

    #[test]
    fn cpu_data_serialize_sources() {
        // CpuData 序列化契约：新增三个字段必须出现在 JSON 中（前端 T3 消费）
        let c = CpuData {
            name: "Test CPU".to_string(),
            usage: 10.0,
            per_core: vec![10.0],
            core_count: 1,
            clock_mhz: 3000.0,
            freq_source: "ntapi".to_string(),
            temperature: 60.0,
            power_w: 30.0,
            temp_source: "winring0".to_string(),
            power_source: "estimated".to_string(),
            power_estimated: true,
        };
        let json = serde_json::to_string(&c).expect("CpuData 序列化不应失败");
        assert!(json.contains("\"temp_source\":\"winring0\""));
        assert!(json.contains("\"power_source\":\"estimated\""));
        assert!(json.contains("\"power_estimated\":true"));
        assert!(json.contains("\"freq_source\":\"ntapi\""));
    }

    #[test]
    fn test_mem() {
        let m = get_memory_data();
        assert!(m.total > 0);
    }

    /// 真机端到端验证（默认忽略）：get_sensor_data 快照中磁盘 IO 速度字段
    /// 应为非负有限值（PDH 通道；空闲可为 0）。
    #[test]
    #[ignore]
    fn real_machine_disk_io_in_snapshot() {
        let data = get_sensor_data();
        assert!(!data.disks.is_empty(), "应有磁盘条目");
        for d in &data.disks {
            assert!(
                d.read_mbps.is_finite() && d.read_mbps >= 0.0,
                "读速度非法: {}",
                d.read_mbps
            );
            assert!(
                d.write_mbps.is_finite() && d.write_mbps >= 0.0,
                "写速度非法: {}",
                d.write_mbps
            );
        }
        eprintln!(
            "[真机] disks={} read={:.2} write={:.2} MB/s(首盘)",
            data.disks.len(),
            data.disks[0].read_mbps,
            data.disks[0].write_mbps
        );
    }
}
