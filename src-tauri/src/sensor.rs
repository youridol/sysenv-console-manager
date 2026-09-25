/// 硬件传感器模块 —— 契约 v3 的**投影层**（sidecar 快照 → 前端 JSON）。
///
/// 硬件数据的唯一来源是 LhmSidecar（见 `lhm.rs` 与 `sidecar-lhm/CONTRACT-v3.md`）：
///   - **tier-1**（无需 UAC）：CPU 负载/频率（PDH 性能计数器）、GPU（厂商用户态通道）、
///     内存节点、每卷磁盘速率/占用、网络、电池 —— sidecar 进程内采集。
///   - **tier-2**（一次 UAC）：CPU 温度/功耗/电压、主板 SuperIO、磁盘温度。
///
/// SECM 侧不再有任何内联探测链路：`CallNtPowerInformation`、WinRing0、ACPI 热区、
/// `nvidia-smi`、硬编码 TDP 估算表均已删除。本模块只做三件事：
///   1. 用 `sysinfo` 取进程内权威的 CPU 占用率/每核占用与磁盘容量；
///   2. 用 `GlobalMemoryStatusEx` 取权威内存总量（sidecar 内存数值仅作型号补充）；
///   3. 把 sidecar 快照按契约 §7 映射为前端既有字段（字段名保持不变）。
///
/// **不伪造 0**：不可用指标一律 0 + `*_source="none"`，前端据 source 渲染 "—"。
/// `ntapi` / `sysinfo` / `winring0` / `acpi` / `rapl` 已不可能出现在 `*_source` 中。
use serde::Serialize;
use std::mem::MaybeUninit;
use std::sync::Mutex;
use sysinfo::System;

use crate::debug_warn;
use crate::lhm::{LhmDiskData, LhmGpuData, LhmSensorResponse};

// ============================================================================
// 契约收窄后的来源枚举（CONTRACT-v3.md §7；测试与映射断言共用）
// ============================================================================

/// `cpu.freq_source` 允许值（原 `ntapi | pdh | registry | sysinfo | none`）。
///
/// `lhm` 为契约 §7 修订新增：sidecar `clock_source="lhm"` 是 LHM 复合实时频率
/// （实测值），在 tier-1 PDH/registry 均缺失（计数器被裁剪的 Windows、或
/// `PerformanceCounterManager` 异步初始化窗口）时由 tier-2 提供，必须保留而非丢弃。
const FREQ_SOURCES: [&str; 4] = ["pdh", "registry", "lhm", "none"];
/// `cpu.temp_source` 允许值（原 `lhm | winring0 | acpi | none`）。
const TEMP_SOURCES: [&str; 2] = ["lhm", "none"];
/// `cpu.power_source` 允许值（原 `lhm | rapl | estimated | none`）。
const POWER_SOURCES: [&str; 3] = ["lhm", "estimated", "none"];

// ============================================================================
// Structures（字段名冻结：src/pages/Dashboard.tsx 直接依赖）
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct CpuData {
    pub name: String,
    pub usage: f32,
    pub per_core: Vec<f32>,
    pub core_count: usize,
    pub clock_mhz: f32,
    /// 频率数据源（契约 §7 收窄）: "pdh" | "registry" | "lhm" | "none"
    pub freq_source: String,
    pub temperature: f32,
    pub power_w: f32,
    /// 温度数据源（契约 §7 收窄）: "lhm" | "none"
    pub temp_source: String,
    /// 功耗数据源（契约 §7 收窄）: "lhm" | "estimated" | "none"
    pub power_source: String,
    /// 功耗是否为估算值（true = 前端显示"估算"角标）
    pub power_estimated: bool,
    /// CPU 占用率是否为**有效采样**（false = 进程内首次采样尚未完成）。
    ///
    /// 背景：sysinfo 的 `refresh_cpu()` 基于 PDH 计数器差分，**进程内第一次调用
    /// 必然拿到 0 空闲时间**，于是 `usage = 100 - 0 = 100%`（per_core 同样全 100）。
    /// 实测：独立探针第 1 次 100.00%、第 2 次 9.64%、第 3 次 16.78%；
    /// 生产 `chart_history.jsonl` 中 27 个会话有 23 个（85.2%）首采样为 100%。
    ///
    /// 该值本身是"预热中"的伪值，不是真实高负载。后端只负责如实标注，
    /// 由前端决定是否用「—」占位（见 Dashboard.tsx `cpuSampleReady`）。
    /// 字段为新增可选契约：旧版前端忽略它即可，不破坏兼容。
    pub usage_ready: bool,
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
    /// 内存型号（sidecar 内存节点名 / SPD 型号补充；无则空串，前端可不渲染）
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
    /// 主板传感器（温度/风扇/电压；tier-2 不可用或无数据时 None，前端不渲染卡片）
    pub motherboard: Option<crate::lhm::LhmMotherboardData>,
    /// tier-2 指标当前是否可读（sidecar 快照 `tier2_available`）
    pub tier2_available: bool,
    /// tier-2 不可读原因（sidecar 快照 `tier2_reason`；前端"需要一次管理员批准"提示）
    pub tier2_reason: Option<String>,
    /// tier-2 是否已生效（sidecar 快照 `elevated`，含提权伴生进程）
    pub elevated: bool,
    pub diag: String,
}

// ============================================================================
// WinAPI: GlobalMemoryStatusEx (kernel32) — 权威内存总量
//
// 契约 §3：sidecar 的 `memory` 是 LHM 推算值，SECM **仍以本 API 为权威**
// （内核用户态，零特权，覆盖全部物理内存）。故此处内联 FFI，不经过任何
// 中间抽象；sidecar 只补充 `model_name`（LHM 内存节点名）。
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

/// 读取权威内存数据；`model_name` 由 `get_memory_data` 从 sidecar 快照补充。
pub fn get_memory_data(snap: Option<&LhmSensorResponse>) -> MemoryData {
    let mut ms = MaybeUninit::<MemoryStatusEx>::zeroed();
    // SAFETY: `ms` 按 MemoryStatusEx 大小分配且零初始化；length 由调用方填
    // size_of；返回非 0 表示整块结构体已被内核填充，assume_init 合法。
    let sampled = unsafe {
        (*ms.as_mut_ptr()).length = std::mem::size_of::<MemoryStatusEx>() as u32;
        extern "system" {
            fn GlobalMemoryStatusEx(lpBuffer: *mut MemoryStatusEx) -> i32;
        }
        if GlobalMemoryStatusEx(ms.as_mut_ptr()) == 0 {
            None
        } else {
            Some(ms.assume_init())
        }
    };

    let mut mem = match sampled {
        Some(ms) => MemoryData {
            total: ms.total_phys,
            used: ms.total_phys.saturating_sub(ms.avail_phys),
            available: ms.avail_phys,
            usage_percent: if ms.total_phys > 0 {
                (ms.memory_load as f32).clamp(0.0, 100.0)
            } else {
                0.0
            },
            model_name: String::new(),
        },
        None => {
            // 权威 API 不可用（理论上仅在非 Windows/极端受限环境）→ sysinfo 兜底，
            // 仍然保持"数值真实、不伪造"的语义。
            debug_warn!("sensor", "GlobalMemoryStatusEx 失败，内存数据回退 sysinfo");
            let mut sys = System::new();
            sys.refresh_memory();
            let t = sys.total_memory();
            MemoryData {
                total: t,
                used: sys.used_memory(),
                available: sys.available_memory(),
                usage_percent: if t > 0 {
                    ((sys.used_memory() as f64 / t as f64) * 100.0) as f32
                } else {
                    0.0
                },
                model_name: String::new(),
            }
        }
    };

    // 型号名：sidecar 内存节点（LHM 节点名 / SPD 型号，best-effort）。
    // v3 语义变化：不再由 LHM SPD 一次性读取单独填充，而是快照内存节点名直接投影。
    if let Some(name) = snap
        .and_then(|s| s.memory.as_ref())
        .and_then(|m| m.name.as_deref())
    {
        if !name.is_empty() {
            mem.model_name = name.to_string();
        }
    }
    mem
}

// ============================================================================
// sidecar 快照 → 前端来源映射（CONTRACT-v3.md §7，逐条冻结）
// ============================================================================

/// sidecar `cpu.clock_mhz` / `clock_source` → `(clock_mhz, freq_source)`。
///
/// 契约 §7：`"perfcounter"`（PDH `% Processor Performance`）→ `"pdh"`；
/// `"registry"`（标称）→ `"registry"`；`"lhm"`（LHM 复合实时频率，实测值）→ `"lhm"`；
/// 其余（`"none"` / 空串 / 值缺失或非正）→ `"none"`。
///
/// `"lhm"` 分支不可省略：sidecar 在 tier-1 PDH 与 registry `~MHz` 双双缺失
/// （计数器被裁剪的系统、或性能计数器异步初始化窗口）而 tier-2 已生效时会回退到
/// LHM 复合频率——那是真实测量值，丢弃会正好在"管理员场景"下丢数据。
/// `none` 时频率归零，维持「source=none ⇄ 值=0」不变式（不用 0 冒充有效数据）。
fn map_cpu_clock(snap: Option<&LhmSensorResponse>) -> (f32, &'static str) {
    let cpu = match snap {
        Some(s) => &s.cpu,
        None => return (0.0, "none"),
    };
    let valid = cpu.clock_mhz.filter(|v| v.is_finite() && *v > 0.0);
    let (clock, src) = match (cpu.clock_source.as_str(), valid) {
        ("perfcounter", Some(v)) => (v, "pdh"),
        ("registry", Some(v)) => (v, "registry"),
        ("lhm", Some(v)) => (v, "lhm"),
        _ => (0.0, "none"),
    };
    debug_assert!(FREQ_SOURCES.contains(&src), "freq_source 越界: {src}");
    (clock, src)
}

/// sidecar `cpu.package_temp_c` / `temp_source` → `(temperature, temp_source)`。
///
/// 契约 §7：sidecar `temp_source="lhm"` → `"lhm"`；否则 `"none"`。
/// tier-2 缺失时 sidecar 给 `null`，本函数输出 0 + `"none"`（前端 "—"）。
fn map_cpu_temp(snap: Option<&LhmSensorResponse>) -> (f32, &'static str) {
    let cpu = match snap {
        Some(s) => &s.cpu,
        None => return (0.0, "none"),
    };
    let valid = cpu.package_temp_c.filter(|v| v.is_finite() && *v > 0.0);
    let (temp, src) = match (cpu.temp_source.as_str(), valid) {
        ("lhm", Some(v)) => (v, "lhm"),
        _ => (0.0, "none"),
    };
    debug_assert!(TEMP_SOURCES.contains(&src), "temp_source 越界: {src}");
    (temp, src)
}

/// sidecar `cpu.power_w` / `power_source` → `(power_w, power_source, power_estimated)`。
///
/// 契约 §7：`power_source="lhm"` → `"lhm"` + `power_estimated=false`；否则若
/// `power_w` 有值 → `"estimated"` + `true`；否则 `"none"`。Rust 侧以 0 表示 null，
/// 故"有值"要求有限且为正。
fn map_cpu_power(snap: Option<&LhmSensorResponse>) -> (f32, &'static str, bool) {
    let cpu = match snap {
        Some(s) => &s.cpu,
        None => return (0.0, "none", false),
    };
    let valid = cpu.power_w.filter(|v| v.is_finite() && *v > 0.0);
    let (power, src, estimated) = if cpu.power_source == "lhm" {
        match valid {
            Some(v) => (v, "lhm", false),
            None => (0.0, "none", false),
        }
    } else {
        match valid {
            Some(v) => (v, "estimated", true),
            None => (0.0, "none", false),
        }
    };
    debug_assert!(POWER_SOURCES.contains(&src), "power_source 越界: {src}");
    (power, src, estimated)
}

/// 把 sidecar GPU 节点映射为 `GpuData`。
///
/// 过滤「无任何有效指标」的节点：原样输出会得到全 0 的占位卡片（伪造 0）。
/// 至少一个有效指标才映射；可空字段 → 0（前端据 0 渲染 "—"）。
fn map_lhm_gpu(g: &LhmGpuData) -> Option<GpuData> {
    let has_metric = g.temperature_c.is_some()
        || g.load_percent.is_some()
        || g.power_w.is_some()
        || g.core_clock_mhz.is_some()
        || g.memory_used_bytes.is_some()
        || g.memory_total_bytes.is_some();
    if !has_metric {
        return None;
    }
    Some(GpuData {
        name: g.name.clone(),
        usage: g.load_percent.unwrap_or(0.0),
        temperature: g.temperature_c.unwrap_or(0.0),
        memory_used: g.memory_used_bytes.unwrap_or(0),
        memory_total: g.memory_total_bytes.unwrap_or(0),
        clock_mhz: g.core_clock_mhz.unwrap_or(0.0),
        power_w: g.power_w.unwrap_or(0.0),
    })
}

/// 对外 tier-2 权限字段投影：`(tier2_available, elevated, tier2_reason)`。
///
/// sidecar 快照不可用（缺失/降级）→ `false/false` + lhm 诊断串（绝不 panic、
/// 绝不给出空原因）；快照可用但未提供 `tier2_reason` 而 tier-2 不可读时，
/// 同样回落诊断串，保证前端提示卡始终有可读原因。
fn tier2_fields(snap: Option<&LhmSensorResponse>, lhm_diag: &str) -> (bool, bool, Option<String>) {
    match snap {
        Some(r) => {
            let reason = match r.tier2_reason.clone() {
                Some(reason) => Some(reason),
                None if r.tier2_available => None,
                None if lhm_diag.is_empty() => None,
                None => Some(lhm_diag.to_string()),
            };
            (r.tier2_available, r.elevated, reason)
        }
        None => {
            let reason = if lhm_diag.is_empty() {
                "sidecar 快照不可用".to_string()
            } else {
                lhm_diag.to_string()
            };
            (false, false, Some(reason))
        }
    }
}

// ============================================================================
// CPU — sysinfo 占用 + sidecar 频率/温度/功耗投影
// ============================================================================

/// 获取 CPU 数据（契约 §7 投影）。
///
/// - 名称/占用率/每核占用：`sysinfo`（进程内用户态，权威，不依赖 sidecar）；
/// - 频率：sidecar `clock_mhz` + `clock_source` → `pdh` / `registry` / `none`；
/// - 温度：sidecar tier-2 `package_temp_c` + `temp_source` → `lhm` / `none`；
/// - 功耗：sidecar `power_w` + `power_source` → `lhm` / `estimated` / `none`。
///
/// `snap` 为本周期的全量快照（get_sensor_data 每周期只取 1 次，共享给
/// CPU/GPU/主板/内存/磁盘）；`lhm_diag_src` 为快照诊断串，原样透出到 diag 的
/// `[lhm:...]` 注记，便于用户排障（sidecar 缺失/降级原因）。
pub fn get_cpu_data(
    sys: &System,
    snap: Option<&LhmSensorResponse>,
    lhm_diag_src: &str,
    usage_ready: bool,
) -> (CpuData, String) {
    let cpus = sys.cpus();
    // CPU 名：sysinfo brand（与注册表 ProcessorNameString 同源）为主，
    // 极端环境下为空时回落 sidecar CPU 节点名，最后才是占位串。
    let name = cpus
        .first()
        .map(|c| c.brand().to_string())
        .filter(|n| !n.is_empty())
        .or_else(|| snap.map(|s| s.cpu.name.clone()).filter(|n| !n.is_empty()))
        .unwrap_or_else(|| "Unknown CPU".into());
    let core_count = cpus.len();
    let usage = sys.global_cpu_info().cpu_usage().clamp(0.0, 100.0);
    let per_core: Vec<f32> = cpus
        .iter()
        .map(|c| c.cpu_usage().clamp(0.0, 100.0))
        .collect();

    let (clock_mhz, freq_source) = map_cpu_clock(snap);
    let (temperature, temp_source) = map_cpu_temp(snap);
    let (power_w, power_source, power_estimated) = map_cpu_power(snap);

    // 诊断串：只报告 sidecar 实际提供的来源（无一内联降级链），并透出 lhm 诊断。
    let mut diag = String::new();
    if clock_mhz > 0.0 {
        diag.push_str(&format!("FREQ={:.0}({}) ", clock_mhz, freq_source));
    } else {
        diag.push_str("FREQ=0(none) ");
    }
    diag.push_str(&format!("USAGE={:.1}%", usage));
    if temperature > 0.0 {
        diag.push_str(&format!(" TEMP={:.1}℃({})", temperature, temp_source));
    } else {
        diag.push_str(" TEMP=n/a");
    }
    match power_source {
        "lhm" => diag.push_str(&format!(" PWR={:.1}W(lhm)", power_w)),
        "estimated" => diag.push_str(&format!(" PWR≈{:.1}W(estimated)", power_w)),
        _ => diag.push_str(" PWR=n/a"),
    }
    if !lhm_diag_src.is_empty() {
        diag.push_str(&format!(" [lhm:{}]", lhm_diag_src));
    }

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
            usage_ready,
        },
        diag,
    )
}

// ============================================================================
// Storage — 容量 sysinfo（DriveInfo 等价）+ 速率/温度 sidecar 每卷快照
//
// sysinfo v0.30 的 Disks::refresh_list() 在 Windows 上可能追加而非替换旧条目，
// 故每 DISKS_REBUILD_INTERVAL 次调用重建实例。
// ============================================================================

static DISKS_INSTANCE: std::sync::LazyLock<Mutex<(sysinfo::Disks, u32)>> =
    std::sync::LazyLock::new(|| Mutex::new((sysinfo::Disks::new_with_refreshed_list(), 0)));

/// Disks 实例重建间隔（调用次数），防止 refresh_list 内部条目累积
const DISKS_REBUILD_INTERVAL: u32 = 60;

/// 卷标识归一化：`"C:\\"` / `"c:"` / `"C"` → `"C:"`（与 sidecar `disks[].name` 对齐）。
fn normalize_volume_key(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('\\');
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.ends_with(':') {
        trimmed.to_ascii_uppercase()
    } else {
        format!("{}:", trimmed).to_ascii_uppercase()
    }
}

/// 按卷标识在 sidecar 磁盘列表中查找对应节点（sidecar 名称形如 `"C:"`）。
fn find_lhm_disk<'a>(disks: &'a [LhmDiskData], key: &str) -> Option<&'a LhmDiskData> {
    if key.is_empty() {
        return None;
    }
    disks.iter().find(|d| normalize_volume_key(&d.name) == key)
}

/// sidecar 字节/秒 → MB/s（前端 `read_mbps` / `write_mbps` 单位）。
/// 缺失/非法值 → 0（前端渲染 0 MB/s，不伪造速率）。
fn bytes_per_sec_to_mbps(v: Option<f64>) -> f32 {
    match v {
        Some(b) if b.is_finite() && b >= 0.0 => (b / (1024.0 * 1024.0)) as f32,
        _ => 0.0,
    }
}

/// 磁盘投影：容量/占用来自 `sysinfo`（DriveInfo 等价，零特权、与 sidecar 无关），
/// 读写速率来自 sidecar 每卷快照（tier-1 PDH）。
///
/// 结构保持 `DiskData` 不变（契约 §7：字段名冻结，不新增前端可见字段）；
/// sidecar 的 `activity_percent` / `temperature_c` 无对应对外字段，故不透出。
pub fn get_disk_data(snap: Option<&LhmSensorResponse>) -> Vec<DiskData> {
    let mut guard = DISKS_INSTANCE.lock().unwrap_or_else(|p| p.into_inner());
    let (disks, call_count) = &mut *guard;
    *call_count += 1;

    if *call_count >= DISKS_REBUILD_INTERVAL {
        *disks = sysinfo::Disks::new_with_refreshed_list();
        *call_count = 0;
    } else {
        disks.refresh_list();
    }

    disks
        .list()
        .iter()
        .map(|d| {
            let total = d.total_space();
            let available = d.available_space();
            let used = total.saturating_sub(available);
            // mount_point 形如 "D:\\" → 归一化为 "D:" 与 sidecar 卷名匹配
            let mount = d.mount_point().to_string_lossy();
            let drive_key = normalize_volume_key(&mount);
            let lhm_disk = snap.and_then(|s| find_lhm_disk(&s.disks, &drive_key));
            let (read_mbps, write_mbps) = match lhm_disk {
                Some(x) => (
                    bytes_per_sec_to_mbps(x.read_bytes_per_sec),
                    bytes_per_sec_to_mbps(x.write_bytes_per_sec),
                ),
                None => (0.0, 0.0),
            };
            // 卷标为空时回落盘符，保证前端始终有可读名称
            let label = d.name().to_string_lossy().to_string();
            let name = if label.is_empty() {
                drive_key.clone()
            } else {
                label
            };
            DiskData {
                name,
                total_space: total,
                available_space: available,
                used_space: used,
                usage_percent: if total > 0 {
                    (used as f64 / total as f64 * 100.0) as f32
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

/// 进程内已完成的 CPU 采样次数。
///
/// sysinfo 的首次 `refresh_cpu()` 拿不到差分基准，必然返回 100% 伪值
/// （见 [`CpuData::usage_ready`]）。用它区分"预热伪值"与真实采样：
/// 第 1 次置 `usage_ready=false`，第 2 次起为 true。
static CPU_SAMPLE_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 传感器总快照（前端 `get_sensor_data` 命令的唯一实现）。
///
/// 每周期只向 sidecar 轮询 1 次全量快照，CPU/GPU/主板/内存/磁盘共享同一份
/// （避免各数据源独立 HTTP 的开销）；tier-1 由 sidecar 立即提供，tier-2 需
/// 用户在提权提示处确认一次 UAC，前端据 `tier2_available` / `tier2_reason` 提示。
pub fn get_sensor_data() -> SensorData {
    let mut g = SYS_INSTANCE.lock().unwrap_or_else(|p| p.into_inner());
    // System::new() 仅分配 CPU + 内存组件，不枚举全部进程（节省 10-50MB）
    let s = g.get_or_insert_with(System::new);
    s.refresh_cpu();
    s.refresh_memory();
    // 本次采样是进程内第几次（+1 后使用）；第 1 次的结果是伪值
    let sample_index = CPU_SAMPLE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;

    // 唯一 HTTP 轮询入口（含 sidecar 进程生命周期：启动/退避/重启/降级）
    let (lhm_snap, lhm_diag) = crate::lhm::lhm_snapshot();
    let snap = lhm_snap.as_ref();

    let (cpu, cpu_diag) = get_cpu_data(s, snap, &lhm_diag, sample_index > 1);

    // GPU：唯一来源是 sidecar（nvidia-smi 链路已删除）；快照缺失 → 空列表，
    // 绝不编造占位条目。
    let gpu: Vec<GpuData> = match snap {
        Some(r) => r.gpu.iter().filter_map(map_lhm_gpu).collect(),
        None => Vec::new(),
    };

    // 主板传感器：sidecar 透出（tier-2；温度/风扇/电压分组）；不可用/无数据 → None
    let motherboard = snap.and_then(|r| r.motherboard.clone());

    drop(g); // release sys

    let mem = get_memory_data(snap);
    let disks = get_disk_data(snap);
    let (tier2_available, elevated, tier2_reason) = tier2_fields(snap, &lhm_diag);

    let gpu_src = if gpu.is_empty() { "none" } else { "lhm" };
    let mb_tag = match motherboard.as_ref() {
        Some(mb) => format!("mb={}传感器", mb.sensors.len()),
        None => "mb=n/a".to_string(),
    };
    let tier2_tag = if tier2_available {
        "tier2=ready".to_string()
    } else {
        match tier2_reason.as_deref() {
            Some(reason) => format!("tier2=n/a({})", reason),
            None => "tier2=n/a".to_string(),
        }
    };
    let diag = format!(
        "GPU={} {} mem={:.0}% {} {}",
        gpu_src, cpu_diag, mem.usage_percent, mb_tag, tier2_tag
    );
    crate::temp_data::sensor_diag(&diag);

    SensorData {
        cpu,
        gpu,
        memory: mem,
        disks,
        motherboard,
        tier2_available,
        tier2_reason,
        elevated,
        diag,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lhm::{LhmCpuData, LhmMemoryData};

    /// 构造一份最小可用快照（tier-1 无来源、tier-2 缺失）。
    fn snapshot_with(cpu: LhmCpuData, memory: Option<LhmMemoryData>) -> LhmSensorResponse {
        LhmSensorResponse {
            available: true,
            contract_version: Some(3),
            cpu,
            memory,
            ..Default::default()
        }
    }

    fn sysinfo_all() -> System {
        let mut s = System::new_all();
        s.refresh_all();
        s
    }

    #[test]
    fn test_cpu() {
        let s = sysinfo_all();
        let (c, _) = get_cpu_data(&s, None, "", true);
        assert!(!c.name.is_empty());
        // 降级语义：数据源不可用时不 panic、数值非负且不伪造
        assert!(c.temperature >= 0.0);
        assert!(c.clock_mhz >= 0.0);
        assert!(c.power_w >= 0.0);
        // 收窄后的来源枚举契约（CONTRACT-v3.md §7）——旧值不可能出现
        assert!(
            FREQ_SOURCES.contains(&c.freq_source.as_str()),
            "freq_source 越界: {}",
            c.freq_source
        );
        assert!(
            TEMP_SOURCES.contains(&c.temp_source.as_str()),
            "temp_source 越界: {}",
            c.temp_source
        );
        assert!(
            POWER_SOURCES.contains(&c.power_source.as_str()),
            "power_source 越界: {}",
            c.power_source
        );
        // source=none ⇄ 值=0（不伪造）
        if c.temp_source == "none" {
            assert_eq!(c.temperature, 0.0, "temp_source=none 却给出温度");
        } else {
            assert!(c.temperature > 0.0, "temp_source 有效却无温度");
        }
        if c.freq_source == "none" {
            assert_eq!(c.clock_mhz, 0.0, "freq_source=none 却给出频率");
        } else {
            assert!(c.clock_mhz > 0.0, "freq_source 有效却无频率");
        }
        if c.power_source == "none" {
            assert_eq!(c.power_w, 0.0);
            assert!(!c.power_estimated);
        }
    }

    #[test]
    fn test_cpu_projection_perfcounter_and_lhm() {
        // 契约 §7 主路径：clock_source=perfcounter → freq_source=pdh；
        // temp_source=lhm → temp_source=lhm；power_source=lhm → lhm + 非估算
        let s = sysinfo_all();
        let snap = snapshot_with(
            LhmCpuData {
                name: "AMD Ryzen 7 5800X 8-Core Processor".to_string(),
                load_percent: Some(12.5),
                package_temp_c: Some(61.5),
                power_w: Some(42.7),
                clock_mhz: Some(4200.0),
                clock_source: "perfcounter".to_string(),
                temp_source: "lhm".to_string(),
                power_source: "lhm".to_string(),
                ..Default::default()
            },
            None,
        );
        let (c, diag) = get_cpu_data(&s, Some(&snap), "lhm(pkg 61.5℃, 42.7W)", true);
        assert_eq!(c.freq_source, "pdh");
        assert_eq!(c.clock_mhz, 4200.0);
        assert_eq!(c.temp_source, "lhm");
        assert_eq!(c.temperature, 61.5);
        assert_eq!(c.power_source, "lhm");
        assert_eq!(c.power_w, 42.7);
        assert!(!c.power_estimated, "lhm 实测功耗不得标记估算");
        assert!(diag.contains("(pdh)"), "diag 应标注 pdh 来源: {}", diag);
        assert!(diag.contains("(lhm)"), "diag 应标注 lhm 来源: {}", diag);
        assert!(
            diag.contains("[lhm:lhm(pkg 61.5℃, 42.7W)]"),
            "diag 应透出 lhm 诊断: {}",
            diag
        );
    }

    #[test]
    fn test_cpu_projection_registry_and_estimated_power() {
        // clock_source=registry → freq_source=registry（"标称"角标）；
        // power_source≠lhm 但 power_w 有值 → estimated + power_estimated=true
        let s = sysinfo_all();
        let snap = snapshot_with(
            LhmCpuData {
                clock_mhz: Some(3600.0),
                clock_source: "registry".to_string(),
                power_w: Some(30.0),
                power_source: "none".to_string(),
                temp_source: "none".to_string(),
                ..Default::default()
            },
            None,
        );
        let (c, diag) = get_cpu_data(&s, Some(&snap), "", true);
        assert_eq!(c.freq_source, "registry");
        assert_eq!(c.clock_mhz, 3600.0);
        assert_eq!(c.power_source, "estimated");
        assert_eq!(c.power_w, 30.0);
        assert!(c.power_estimated, "estimated 功耗必须标记 power_estimated");
        assert!(diag.contains("(estimated)"), "diag 应标注估算: {}", diag);
        assert_eq!(c.temp_source, "none");
        assert_eq!(c.temperature, 0.0);
    }

    #[test]
    fn test_cpu_projection_lhm_clock_source_preserved() {
        // 契约 §7 修订：sidecar clock_source="lhm"（LHM 复合实时频率，实测值）
        // → freq_source="lhm" 且频率保留。该分支覆盖 tier-1 PDH/registry 均缺失
        // 而 tier-2 已生效的场景（计数器被裁剪的系统 / 性能计数器初始化窗口），
        // 值必须保留而不能丢弃；同时绝不出现 ntapi/sysinfo/winring0/acpi/rapl。
        let s = sysinfo_all();
        let snap = snapshot_with(
            LhmCpuData {
                clock_mhz: Some(4100.0),
                clock_source: "lhm".to_string(),
                temp_source: "none".to_string(),
                power_source: "none".to_string(),
                ..Default::default()
            },
            None,
        );
        let (c, diag) = get_cpu_data(&s, Some(&snap), "lhm(无有效温度:需要管理员权限)", true);
        assert_eq!(
            c.freq_source, "lhm",
            "clock_source=lhm 应映射为 freq_source=lhm"
        );
        assert_eq!(c.clock_mhz, 4100.0, "lhm 实时频率必须保留，不得归零");
        assert!(c.clock_mhz > 0.0, "freq_source=lhm 必须配有效频率");
        assert!(diag.contains("(lhm)"), "diag 应标注 lhm 频率来源: {}", diag);
        // 其余通道不受影响（本用例未提供 tier-2 数据）
        assert_eq!(c.temp_source, "none");
        assert_eq!(c.temperature, 0.0);
        assert_eq!(c.power_source, "none");
        assert_eq!(c.power_w, 0.0);
        assert!(!c.power_estimated);
        // 旧取值必须仍然不可能（收窄集合的负向断言保持不变）
        for legacy in ["ntapi", "sysinfo", "winring0", "acpi", "rapl"] {
            assert_ne!(c.freq_source, legacy);
            assert_ne!(c.temp_source, legacy);
            assert_ne!(c.power_source, legacy);
        }
        // 正向：新取值在收窄集合内
        assert!(
            FREQ_SOURCES.contains(&c.freq_source.as_str()),
            "freq_source 应在收窄集合内: {}",
            c.freq_source
        );
    }

    #[test]
    fn test_cpu_projection_lhm_clock_source_requires_positive_value() {
        // "lhm" 分支同样受「值有效」守卫约束：clock_mhz 缺失/为 0/非有限时
        // 回落 "none" + 0，维持「source=none ⇄ 值=0」不变式（不伪造频率）
        let s = sysinfo_all();
        for bad in [
            None,
            Some(0.0),
            Some(-1.0),
            Some(f32::NAN),
            Some(f32::INFINITY),
        ] {
            let snap = snapshot_with(
                LhmCpuData {
                    clock_mhz: bad,
                    clock_source: "lhm".to_string(),
                    ..Default::default()
                },
                None,
            );
            let (c, _) = get_cpu_data(&s, Some(&snap), "", true);
            assert_eq!(
                c.freq_source, "none",
                "clock_mhz={bad:?} 时不应标记 lhm 来源"
            );
            assert_eq!(c.clock_mhz, 0.0, "freq_source=none 时频率必须为 0");
        }
    }

    #[test]
    fn test_cpu_projection_invalid_values_are_filtered() {
        // 侧车给出 0 / 非有限值（等价于"无数据"）时不得标记有效来源
        let s = sysinfo_all();
        let snap = snapshot_with(
            LhmCpuData {
                package_temp_c: Some(0.0),
                power_w: Some(0.0),
                clock_mhz: Some(0.0),
                clock_source: "perfcounter".to_string(),
                temp_source: "lhm".to_string(),
                power_source: "lhm".to_string(),
                ..Default::default()
            },
            None,
        );
        let (c, _) = get_cpu_data(&s, Some(&snap), "", true);
        assert_eq!(c.freq_source, "none");
        assert_eq!(c.clock_mhz, 0.0);
        assert_eq!(c.temp_source, "none");
        assert_eq!(c.temperature, 0.0);
        assert_eq!(c.power_source, "none");
        assert_eq!(c.power_w, 0.0);
        assert!(!c.power_estimated);
    }

    #[test]
    fn test_gpu_projection_and_metricless_filter() {
        // sidecar GPU → GpuData 字段映射；无任何有效指标的节点被丢弃（不伪造占位）
        let s = sysinfo_all();
        let snap = LhmSensorResponse {
            available: true,
            contract_version: Some(3),
            gpu: vec![
                LhmGpuData {
                    name: "NVIDIA GeForce RTX 3060".to_string(),
                    temperature_c: Some(52.3),
                    core_clock_mhz: Some(1500.0),
                    power_w: Some(85.2),
                    load_percent: Some(45.0),
                    memory_used_bytes: Some(1073741824),
                    memory_total_bytes: Some(6442450944),
                    fan_rpm: Some(1200.0),
                    vram_load_percent: Some(16.7),
                    temp_source: "lhm".to_string(),
                    power_source: "lhm".to_string(),
                    clock_source: "lhm".to_string(),
                },
                // 全空节点（LHM 识别到设备但无任何传感器）→ 丢弃
                LhmGpuData {
                    name: "Empty Placeholder".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mapped: Vec<GpuData> = snap.gpu.iter().filter_map(map_lhm_gpu).collect();
        assert_eq!(mapped.len(), 1, "无指标节点应被过滤，避免伪造 0 占位");
        let g = &mapped[0];
        assert_eq!(g.name, "NVIDIA GeForce RTX 3060");
        assert_eq!(g.usage, 45.0);
        assert_eq!(g.temperature, 52.3);
        assert_eq!(g.clock_mhz, 1500.0);
        assert_eq!(g.power_w, 85.2);
        assert_eq!(g.memory_used, 1073741824);
        assert_eq!(g.memory_total, 6442450944);
        // 与原 CPU 数据正交：不 panic、不使用任何其它数据源
        let (_c, _) = get_cpu_data(&s, Some(&snap), "", true);
    }

    #[test]
    fn test_memory_authoritative_total_and_sidecar_model_name() {
        // 权威总量来自 GlobalMemoryStatusEx（与 sidecar 无关）；型号名来自
        // sidecar 内存节点名（v3 语义：投影而非 SPD 一次性读取）
        let without = get_memory_data(None);
        assert!(without.total > 0, "权威内存总量应为正");
        assert!(
            without.model_name.is_empty(),
            "无 sidecar 时型号应为空串（前端不渲染）"
        );
        assert!(without.used <= without.total);
        assert!(without.available <= without.total);
        assert!((0.0..=100.0).contains(&without.usage_percent));

        let snap = snapshot_with(
            LhmCpuData::default(),
            Some(LhmMemoryData {
                name: Some("Physical Memory".to_string()),
                total_bytes: Some(1),
                used_bytes: Some(1),
            }),
        );
        let with = get_memory_data(Some(&snap));
        assert_eq!(with.model_name, "Physical Memory");
        assert_eq!(
            with.total, without.total,
            "sidecar 数值只作补充，不得覆盖权威总量"
        );

        // 空名 / 缺失 name → 保持空串（不伪造型号）
        let empty = snapshot_with(
            LhmCpuData::default(),
            Some(LhmMemoryData {
                name: Some(String::new()),
                ..Default::default()
            }),
        );
        assert!(get_memory_data(Some(&empty)).model_name.is_empty());
        let no_mem = snapshot_with(LhmCpuData::default(), None);
        assert!(get_memory_data(Some(&no_mem)).model_name.is_empty());
    }

    #[test]
    fn test_memory_status_shape() {
        let m = get_memory_data(None);
        assert!(m.total > 0);
    }

    #[test]
    fn test_disk_projection_overlays_sidecar_rates() {
        let snap = LhmSensorResponse {
            available: true,
            contract_version: Some(3),
            disks: vec![
                LhmDiskData {
                    name: "C:".to_string(),
                    read_bytes_per_sec: Some(1024.0 * 1024.0),
                    write_bytes_per_sec: Some(2.0 * 1024.0 * 1024.0),
                    activity_percent: Some(12.0),
                    used_percent: Some(63.5),
                    temperature_c: Some(38.0),
                },
                LhmDiskData {
                    name: "Z:".to_string(),
                    read_bytes_per_sec: Some(999.0),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let disks = get_disk_data(Some(&snap));
        for d in &disks {
            assert!(!d.name.is_empty(), "磁盘名不应为空");
            assert!(d.used_space <= d.total_space);
            assert!(d.read_mbps.is_finite() && d.read_mbps >= 0.0);
            assert!(d.write_mbps.is_finite() && d.write_mbps >= 0.0);
            // sidecar 未提供该卷速率时必须是 0（不伪造），且不可能是 sidecar
            // 中不存在的卷（Z:）的值泄漏过来
            assert_ne!(d.read_mbps, 999.0, "未匹配卷的速率不应泄漏: {}", d.name);
        }
        // 无快照 → 速率全 0（不伪造），容量仍可用
        let bare = get_disk_data(None);
        assert_eq!(bare.len(), disks.len(), "快照有无不应改变磁盘条目数");
        for d in &bare {
            assert_eq!(d.read_mbps, 0.0);
            assert_eq!(d.write_mbps, 0.0);
        }
        // 卷匹配纯逻辑：与挂载点归一化后的键一致
        assert_eq!(
            find_lhm_disk(&snap.disks, "C:").map(|d| d.name.as_str()),
            Some("C:")
        );
        assert!(find_lhm_disk(&snap.disks, "").is_none());
        assert!(find_lhm_disk(&snap.disks, "Q:").is_none());
    }

    #[test]
    fn test_normalize_volume_key() {
        assert_eq!(normalize_volume_key("C:\\"), "C:");
        assert_eq!(normalize_volume_key("c:"), "C:");
        assert_eq!(normalize_volume_key("D"), "D:");
        assert_eq!(normalize_volume_key(""), "");
        assert_eq!(normalize_volume_key("  E:  "), "E:");
    }

    #[test]
    fn test_bytes_per_sec_to_mbps() {
        assert!((bytes_per_sec_to_mbps(Some(1024.0 * 1024.0)) - 1.0).abs() < 1e-4);
        assert_eq!(bytes_per_sec_to_mbps(None), 0.0);
        assert_eq!(bytes_per_sec_to_mbps(Some(-1.0)), 0.0);
        assert_eq!(bytes_per_sec_to_mbps(Some(f64::NAN)), 0.0);
    }

    #[test]
    fn test_tier2_fields_projection() {
        // sidecar 缺失：false/false + lhm 诊断串（绝不 None、绝不 panic）
        let (avail, elevated, reason) = tier2_fields(None, "lhm(exe 缺失)");
        assert!(!avail);
        assert!(!elevated);
        assert_eq!(reason.as_deref(), Some("lhm(exe 缺失)"));

        let (avail, _elevated, reason) = tier2_fields(None, "");
        assert!(!avail);
        assert_eq!(reason.as_deref(), Some("sidecar 快照不可用"));

        // 已提权：透出 true/true + 无原因
        let ok = LhmSensorResponse {
            available: true,
            elevated: true,
            tier2_available: true,
            tier2_reason: None,
            ..Default::default()
        };
        let (avail, elevated, reason) = tier2_fields(Some(&ok), "");
        assert!(avail);
        assert!(elevated);
        assert!(reason.is_none());

        // 未提权且有原因：原样透出
        let no = LhmSensorResponse {
            available: true,
            elevated: false,
            tier2_available: false,
            tier2_reason: Some(
                "需要管理员权限（PawnIO 设备仅 SYSTEM/Administrators 可访问）".to_string(),
            ),
            ..Default::default()
        };
        let (avail, elevated, reason) = tier2_fields(Some(&no), "lhm(pkg…)");
        assert!(!avail);
        assert!(!elevated);
        assert!(reason.as_deref().unwrap_or("").contains("管理员"));

        // 未提权但 sidecar 未给原因：回落 lhm 诊断串（前端提示卡始终可读）
        let no_reason = LhmSensorResponse {
            available: true,
            tier2_available: false,
            tier2_reason: None,
            ..Default::default()
        };
        let (_a, _e, reason) = tier2_fields(Some(&no_reason), "lhm(不可用:驱动未安装)");
        assert_eq!(reason.as_deref(), Some("lhm(不可用:驱动未安装)"));
    }

    #[test]
    fn cpu_data_serialize_sources() {
        // CpuData 序列化契约：新增三个字段必须出现在 JSON 中（前端消费）
        let c = CpuData {
            name: "Test CPU".to_string(),
            usage: 10.0,
            per_core: vec![10.0],
            core_count: 1,
            clock_mhz: 3000.0,
            freq_source: "registry".to_string(),
            temperature: 60.0,
            power_w: 30.0,
            temp_source: "lhm".to_string(),
            power_source: "estimated".to_string(),
            power_estimated: true,
            usage_ready: true,
        };
        let json = serde_json::to_string(&c).expect("CpuData 序列化不应失败");
        assert!(json.contains("\"temp_source\":\"lhm\""));
        assert!(json.contains("\"power_source\":\"estimated\""));
        assert!(json.contains("\"power_estimated\":true"));
        assert!(json.contains("\"freq_source\":\"registry\""));
        // usage_ready 必须序列化给前端（决定首帧是否用「—」占位 CPU 占用率）
        assert!(json.contains("\"usage_ready\":true"));

        // 契约 §7 修订新增的 "lhm" 频率来源必须原样往返（前端按字符串比较）
        let c_lhm = CpuData {
            freq_source: "lhm".to_string(),
            clock_mhz: 4100.0,
            temp_source: "none".to_string(),
            power_source: "none".to_string(),
            power_estimated: false,
            ..c
        };
        let json_lhm = serde_json::to_string(&c_lhm).expect("CpuData 序列化不应失败");
        assert!(
            json_lhm.contains("\"freq_source\":\"lhm\""),
            "freq_source=lhm 必须原样序列化: {json_lhm}"
        );
        let back: serde_json::Value = serde_json::from_str(&json_lhm).expect("序列化结果应可解析");
        assert_eq!(
            back.get("freq_source").and_then(|v| v.as_str()),
            Some("lhm"),
            "freq_source=lhm 应往返一致"
        );
        assert_eq!(
            back.get("clock_mhz").and_then(|v| v.as_f64()),
            Some(4100.0),
            "lhm 来源的频率值应随 JSON 保留"
        );
    }

    #[test]
    fn sensor_data_serializes_tier2_fields() {
        // 新增外的 tier-2 字段名必须是契约冻结的 snake_case（前端按名读取）
        let data = get_sensor_data();
        let json = serde_json::to_value(&data).expect("SensorData 应可序列化");
        let obj = json.as_object().expect("SensorData 应为对象");
        for key in ["tier2_available", "tier2_reason", "elevated"] {
            assert!(obj.contains_key(key), "SensorData 缺少字段: {key}");
        }
        assert!(obj["tier2_available"].is_boolean());
        assert!(obj["elevated"].is_boolean());
        assert!(obj["tier2_reason"].is_string() || obj["tier2_reason"].is_null());
        assert!(!data.diag.is_empty(), "diag 诊断串不应为空");
    }

    /// 真机端到端验证（默认忽略）：`get_sensor_data` 投影必须落在契约 §7 的
    /// 收窄枚举内，且 tier-1 指标（频率/内存/磁盘容量）真实可用。
    ///
    /// 运行：`cargo test --lib -- --ignored real_machine_projection_contract`
    /// （需本机 127.0.0.1:45980 有 sidecar；无 sidecar 时全部降级为 none 亦通过，
    /// 因为本测试断言的是不变式而非"必须有数据"。）
    #[test]
    #[ignore]
    fn real_machine_projection_contract() {
        let data = get_sensor_data();
        // 枚举收窄（契约 §7）：旧取值不可能出现
        assert!(FREQ_SOURCES.contains(&data.cpu.freq_source.as_str()));
        assert!(TEMP_SOURCES.contains(&data.cpu.temp_source.as_str()));
        assert!(POWER_SOURCES.contains(&data.cpu.power_source.as_str()));
        // none ⇄ 0 不变式
        if data.cpu.freq_source == "none" {
            assert_eq!(data.cpu.clock_mhz, 0.0);
        }
        if data.cpu.temp_source == "none" {
            assert_eq!(data.cpu.temperature, 0.0);
        }
        if data.cpu.power_source == "none" {
            assert_eq!(data.cpu.power_w, 0.0);
            assert!(!data.cpu.power_estimated);
        }
        // 权威内存始终可用
        assert!(data.memory.total > 0);
        assert!(data.cpu.core_count >= 1);
        assert!(!data.diag.is_empty());
        // tier-2 字段必须自洽：可读时不应给出"无原因"以外的矛盾
        if data.tier2_available {
            eprintln!("[真机] tier-2 已生效 elevated={}", data.elevated);
        } else {
            assert!(
                !data.elevated,
                "tier2_available=false 时 elevated 应为 false"
            );
            assert!(
                data.tier2_reason.as_deref().is_some_and(|r| !r.is_empty()),
                "tier-2 不可用时应给出可读原因"
            );
        }
        eprintln!(
            "[真机] freq={:.0}MHz({}) temp={:.1}℃({}) pwr={:.1}W({}) gpu={} disks={} tier2={} reason={:?}",
            data.cpu.clock_mhz,
            data.cpu.freq_source,
            data.cpu.temperature,
            data.cpu.temp_source,
            data.cpu.power_w,
            data.cpu.power_source,
            data.gpu.len(),
            data.disks.len(),
            data.tier2_available,
            data.tier2_reason
        );
    }

    /// 真机端到端验证（默认忽略）：get_sensor_data 快照中磁盘吞吐字段
    /// 应为非负有限值（sidecar tier-1/tier-2 提供；缺失时全 0；不伪造 0 契约保证）。
    #[test]
    #[ignore]
    fn real_machine_disk_throughput_in_snapshot() {
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
            "[真机] disks={} read={:.2} write={:.2} MB/s(首盘) tier2_available={} reason={:?}",
            data.disks.len(),
            data.disks[0].read_mbps,
            data.disks[0].write_mbps,
            data.tier2_available,
            data.tier2_reason
        );
    }
}
