//! CPU 频率采集（纯 Rust 用户态，叶子模块）
//!
//! 数据源优先级降级链（设计文档 `docs/designs/2026-08-09-cpu-freq-fallback.md`）：
//!   1. `CallNtPowerInformation(ProcessorInformation)` —— 实时，powrprof.dll
//!      （缓冲区按 `GetSystemInfo().dwNumberOfProcessors` 权威逻辑处理器数分配，
//!      替代 `available_parallelism()`，F4 修复）
//!   2. PDH `\Processor Information(_Total)\% Processor Performance` × 注册表
//!      `~MHz` 标称 —— 实时估算，任务管理器同源，普通用户可读
//!      （注意：`\Processor(_Total)\...` 实测不可用，必须用 Processor Information 版本）
//!   3. 注册表 `HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0\~MHz`
//!      —— 标称基频，保底
//!   4. 全部失败 → `(None, "none")`
//!
//! 诊断透出（v0.21.6）：`get_cpu_freq_diag()` 返回 `(频率, 数据源, 各层失败
//! 原因字符串)`，失败原因含 API 名 + 错误码（如
//! `CallNtPowerInformation(0x80000005)`），供前端 FREQ=0 场景一次性诊断摘要
//! 使用；`get_cpu_freq()` 签名保持不变（兼容 hardware.rs / sensor.rs 调用）。
//!
//! 线程模型：同步阻塞 API，调用方须在 `spawn_blocking` 中执行（S8）。
//! 注意：PDH 层进程内第一次调用会阻塞补齐最小采样间隔（约 1.05s，盲区 A
//! 修复），保证 PDH 可用时首调也能返回有效估算值；此后不再阻塞。
//! 错误处理：采集降级语义，全部失败返回 `None` 而不抛错；失败路径输出
//! API 名 + 错误码（R8 日志完整）。

use std::mem::MaybeUninit;
use std::sync::Mutex;
use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue,
    PdhOpenQueryW, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE,
    PDH_FMT_DOUBLE, PDH_INVALID_DATA,
};
use windows_sys::Win32::System::Power::{CallNtPowerInformation, ProcessorInformation};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

/// PDH 计数器路径：Processor Information 类别 `_Total` 实例
/// （`\Processor(_Total)\...` 实测不可用，必须用 Processor Information 版本）
const COUNTER_PROCESSOR_PERF: &str = r"\Processor Information(_Total)\% Processor Performance";

/// PDH 两次采样最小间隔：PDH 要求两次 `PdhCollectQueryData` 间隔 ≥1s 才有
/// 稳定格式化值；首调 warmup 补齐到该间隔（略大于 1s，防时钟抖动导致
/// `PDH_CSTATUS_INVALID_DATA` 频繁出现）。
const PDH_MIN_SAMPLE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1050);

/// `CallNtPowerInformation(ProcessorInformation)` 输出结构（每逻辑处理器一条）。
#[repr(C)]
struct ProcessorPowerInfo {
    number: u32,
    max_mhz: u32,
    current_mhz: u32,
    _limit: u32,
    _max_idle: u32,
    _cur_idle: u32,
}

/// PDH 查询状态（进程级单例；查询句柄生命周期 = 进程生命周期，退出由 OS 回收）。
struct PdhFreqState {
    query: isize,
    counter: isize,
    /// 上次有效估算频率（MHz）；采样间隔不足/无效时沿用
    last_freq: Option<f32>,
    /// 初始化失败标记（计数器不存在等）——失败后不再重试
    failed: bool,
    /// 上次成功 `PdhCollectQueryData` 的时间戳（首调 warmup 补齐采样间隔用）
    baseline_at: Option<std::time::Instant>,
    /// 首调 warmup 是否完成；完成后不再阻塞等待采样间隔（仅进程内第一次阻塞）
    warmup_done: bool,
}

static PDH_FREQ_STATE: Mutex<Option<PdhFreqState>> = Mutex::new(None);

/// 字符串 → NUL 结尾 UTF-16。
fn to_utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// 采样一次 CPU 平均频率（MHz），返回 (频率, 数据源标识)。
///
/// 降级链：`ntapi（实时）→ pdh（实时估算）→ registry（标称保底）→ (None, "none")`。
/// 全部失败返回 `None` 而不抛错（采集降级语义，R8 日志完整）。
/// 数据源标识为前端契约：`"ntapi" | "pdh" | "registry" | "none"`。
pub fn get_cpu_freq() -> (Option<f32>, &'static str) {
    let (freq, src, _diag) = get_cpu_freq_diag();
    (freq, src)
}

/// 采样 CPU 平均频率（MHz）并透出各层失败原因（诊断契约，v0.21.6）。
///
/// 返回 `(频率, 数据源, 失败原因字符串)`：
/// - 某层成功：原因串为空（成功路径无需诊断）；
/// - 全失败：原因串以 `|` 分隔各层失败原因，含 API 名 + 错误码
///   （如 `CallNtPowerInformation(0x80000005)|PDH不可用|registry~MHz missing`）。
/// 数据源与 `get_cpu_freq()` 一致（`"ntapi" | "pdh" | "registry" | "none"`）。
pub fn get_cpu_freq_diag() -> (Option<f32>, &'static str, String) {
    // 1. ntapi：实时（powrprof，CallNtPowerInformation）
    //    疑似标称判定（v0.21.7）：AMD 7800X3D 等平台 `current_mhz` 恒等于标称
    //    基频（不随负载变化），此时 current ≈ max → 不直接采用，继续走 PDH
    //    实时估算层；PDH 失败时回退 ntapi 标称值并带诊断标注（不劣化现状）。
    let (ntapi_freq, ntapi_reason, avg_max) = ntapi_avg_freq_inner();
    let mut reasons: Vec<String> = Vec::new();
    // 疑似标称判定仅在 ntapi 成功且 avg_max 有效时有意义（防御分支返回 false）
    let ntapi_suspected = match (ntapi_freq, avg_max) {
        (Some(f), m) if m > 0.0 => is_suspected_nominal(f, m),
        _ => false,
    };
    // ntapi 成功且非疑似标称 → 直接采用（真实实时值，Intel 平台主路径）
    if ntapi_freq.is_some() && !ntapi_suspected {
        return (ntapi_freq, "ntapi", String::new());
    }
    if let Some(r) = ntapi_reason {
        reasons.push(r);
    }
    if ntapi_suspected {
        reasons.push("ntapi疑似标称".to_string());
    }
    // 2. pdh：实时估算（% Processor Performance × ~MHz）
    let (pdh_freq, pdh_reason) = pdh_perf_freq_inner();
    if pdh_freq.is_some() {
        return (pdh_freq, "pdh", String::new());
    }
    if let Some(r) = pdh_reason {
        reasons.push(r);
    }
    // ntapi 疑似标称兜底：PDH 不可用时回退 ntapi 标称值（diag 标注，不劣化现状）
    if ntapi_suspected {
        if let Some(f) = ntapi_freq {
            return (
                Some(f),
                "ntapi",
                format!("ntapi疑似标称(PDH不可用:{})", reasons.join("|")),
            );
        }
    }
    // 3. registry：标称基频保底
    match nominal_freq_mhz() {
        Some(n) if n > 0 => return (Some(n as f32), "registry", String::new()),
        _ => reasons.push("registry~MHz missing".to_string()),
    }
    (None, "none", reasons.join("|"))
}

/// 判定 ntapi 平均当前频率是否"疑似标称（非实时）"。
///
/// AMD 7800X3D 等平台 `CallNtPowerInformation(ProcessorInformation)` 的
/// `current_mhz` 恒等于标称基频（不随负载变化），此时 current ≈ max_mhz。
/// 规则：`current_avg >= max_avg * 0.98`（2% 容差，防平台浮点/舍入差异）
/// 判定为疑似标称 → 不直接采用，继续走 PDH 实时估算层。
/// 满载边界：CPU 满载时 current≈max 为真值，但 PDH≈100%×标称≈max，
/// 数值一致，用 PDH 同样正确（不劣化）。
/// 防御：`max_avg <= 0` 或 `current_avg <= 0` 时无法判定 → 返回 false
/// （沿用"非零即成功"原行为，不误伤）。
pub fn is_suspected_nominal(current_avg: f32, max_avg: f32) -> bool {
    if max_avg <= 0.0 || current_avg <= 0.0 {
        return false;
    }
    current_avg >= max_avg * 0.98
}

/// `CallNtPowerInformation(ProcessorInformation)` 的处理器平均当前频率（MHz）。
///
/// 缓冲区按 `GetSystemInfo().dwNumberOfProcessors`（权威逻辑处理器数）分配，
/// 替代 `std::thread::available_parallelism()`——后者在 Windows 上不受进程
/// affinity 影响，且可能低估 >64 逻辑处理器系统（F4 修复）。
/// 失败返回 None（日志输出 API 名 + NTSTATUS 错误码）。
pub fn ntapi_avg_freq() -> Option<f32> {
    ntapi_avg_freq_inner().0
}

/// `ntapi_avg_freq` 内部实现：返回 `(频率, 失败原因, 平均最大频率)`。
/// 失败原因（API 名 + 错误码）供 `get_cpu_freq_diag` 透出诊断；第 3 元素为
/// 平均 `max_mhz`（`ProcessorPowerInfo.max_mhz`，Windows 报告的处理器最大
/// 频率，AMD 平台 ≈ 标称基频），供疑似标称判定使用，失败时为 0.0。
fn ntapi_avg_freq_inner() -> (Option<f32>, Option<String>, f32) {
    // SAFETY: SYSTEM_INFO 为数值 POD，零值合法（指针字段为空且不 deref）
    let mut si: SYSTEM_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: GetSystemInfo 为无参导出，si 由 API 填充
    unsafe { GetSystemInfo(&mut si) };
    let count = si.dwNumberOfProcessors;
    if count == 0 {
        log::warn!("[cpu_freq] GetSystemInfo returned 0 logical processors");
        return (None, Some("GetSystemInfo returned 0 processors".to_string()), 0.0);
    }
    let sz = std::mem::size_of::<ProcessorPowerInfo>() * count as usize;
    let mut buf = vec![0u8; sz];
    // SAFETY: input 为 NULL（无输入参数）；输出缓冲 buf 容量与处理器数匹配
    let rc = unsafe {
        CallNtPowerInformation(
            ProcessorInformation,
            std::ptr::null(),
            0,
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            sz as u32,
        )
    };
    if rc != 0 {
        // R8：输出 API 名 + 错误码（NTSTATUS），并透出失败原因供诊断
        log::warn!(
            "[cpu_freq] CallNtPowerInformation failed: status=0x{:08X}, freq falls back to PDH/registry",
            rc as u32
        );
        return (None, Some(format!("CallNtPowerInformation(0x{:08X})", rc as u32)), 0.0);
    }
    // SAFETY: 返回成功后缓冲含 count 个 ProcessorPowerInfo（布局对齐一致）
    let infos: &[ProcessorPowerInfo] = unsafe {
        std::slice::from_raw_parts(buf.as_ptr() as *const ProcessorPowerInfo, count as usize)
    };
    let sum: u32 = infos.iter().map(|p| p.current_mhz).sum();
    let sum_max: u32 = infos.iter().map(|p| p.max_mhz).sum();
    if sum > 0 {
        // avg_max 为疑似标称判定的基准（与 avg_current 同源同除数）
        (Some(sum as f32 / count as f32), None, sum_max as f32 / count as f32)
    } else {
        (None, Some("CallNtPowerInformation returned 0 frequency".to_string()), 0.0)
    }
}

/// 初始化 PDH 查询（打开查询 + 添加 `% Processor Performance` 计数器 + 首次采样基线）。
///
/// 计数器不存在（虚拟机/被禁用）时返回 Err，调用方标记不可用。
fn pdh_freq_init() -> Result<PdhFreqState, String> {
    let mut query: isize = 0;
    // SAFETY: NULL 数据源 = 本地实时性能数据；query 由 API 写入
    let rc = unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut query) };
    if rc != 0 {
        return Err(format!("PdhOpenQueryW failed win32=0x{:08X}", rc));
    }
    let path = to_utf16(COUNTER_PROCESSOR_PERF);
    let mut counter: isize = 0;
    // SAFETY: 计数器路径为 NUL 结尾 UTF-16；句柄由 API 写入
    let rc = unsafe { PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) };
    if rc != 0 {
        // SAFETY: query 为 PdhOpenQueryW 返回的有效句柄
        unsafe { PdhCloseQuery(query) };
        return Err(format!("PdhAddEnglishCounterW failed win32=0x{:08X}", rc));
    }
    // 首次收集建立采样基线（PDH 需两次采样间隔 ≥1s 才有稳定格式化值）
    // SAFETY: query 为有效句柄
    let rc = unsafe { PdhCollectQueryData(query) };
    // R8：收集失败（对象/计数器不存在、无数据等）时记录 API 名 + 错误码并视为
    // 初始化失败——首次基线未建立，后续 read_perf_percent 将无法得到有效值。
    // PDH_CSTATUS_NEW_DATA(0x00000001) 为成功码（首次收集返回新数据）。
    if rc != 0 && rc != PDH_CSTATUS_NEW_DATA {
        // SAFETY: query 为 PdhOpenQueryW 返回的有效句柄
        unsafe { PdhCloseQuery(query) };
        log::warn!(
            "[cpu_freq] PdhCollectQueryData failed at init: win32=0x{:08X}",
            rc
        );
        return Err(format!("PdhCollectQueryData failed win32=0x{:08X}", rc));
    }
    Ok(PdhFreqState {
        query,
        counter,
        last_freq: None,
        failed: false,
        // 记录基线采样时间戳：首调 warmup 据此补齐最小采样间隔
        baseline_at: Some(std::time::Instant::now()),
        warmup_done: false,
    })
}

/// 读取 `% Processor Performance` 单计数器当前值（百分比）。
///
/// 返回 None = 采样间隔不足 / CStatus 无效 / 非有限值。
fn read_perf_percent(counter: isize) -> Option<f32> {
    let mut ty: u32 = 0;
    let mut value = MaybeUninit::<PDH_FMT_COUNTERVALUE>::zeroed();
    // SAFETY: value 为对齐正确的输出缓冲；ty 由 API 写入
    let rc =
        unsafe { PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, &mut ty, value.as_mut_ptr()) };
    if rc != PDH_CSTATUS_VALID_DATA && rc != PDH_CSTATUS_NEW_DATA {
        return None;
    }
    // SAFETY: rc 校验通过后 CStatus 与值已由 API 填充
    let v = unsafe { value.assume_init() };
    if v.CStatus == PDH_INVALID_DATA {
        // 两次采样间隔不足（<1s）——调用方沿用上次值
        return None;
    }
    // SAFETY: CStatus 有效时 doubleValue 已由 API 写入
    let perf = unsafe { v.Anonymous.doubleValue };
    if !perf.is_finite() {
        return None;
    }
    Some(perf as f32)
}

/// PDH `% Processor Performance` × 注册表 `~MHz` 标称 → 实时估算频率（MHz）。
///
/// 进程级 Mutex 单例（查询句柄仅初始化一次）；首次调用初始化查询并阻塞补齐
/// 最小采样间隔（约 1.05s，warmup），PDH 可用时首调即返回有效估算值；此后
/// 每次调用返回最近采样估算值，间隔不足/无效时沿用上次有效值（避免抖动
/// 归零）。标称频率缺失或 PDH 初始化失败时返回 None（不抛错，降级链继续）。
pub fn pdh_perf_freq() -> Option<f32> {
    pdh_perf_freq_inner().0
}

/// `pdh_perf_freq` 内部实现：返回 `(估算频率, 失败原因)`。
///
/// 进程级 Mutex 单例（查询句柄仅初始化一次）；首次调用初始化查询并阻塞补齐
/// 最小采样间隔（约 1.05s），保证 PDH 可用时首调也能返回有效估算值——此前
/// 首调仅建立基线返回 None，若轮询间隔与 PDH 最小间隔边界抖动，PDH 层可能
/// 长期不可用而降级 registry（盲区 A 修复）。warmup 完成后不再阻塞；采样间隔
/// 不足/无效时沿用上次有效值（避免抖动归零）。
///
/// 注册表标称频率缺失时返回 `(None, None)`——无法估算属 registry 层缺失，
/// 失败原因由上层 `get_cpu_freq_diag` 的 registry 层记录，避免重复。
fn pdh_perf_freq_inner() -> (Option<f32>, Option<String>) {
    // 注册表标称频率缺失时无法估算 → 直接 None（原因归 registry 层）
    let nominal = match nominal_freq_mhz() {
        Some(n) => n as f32,
        None => return (None, None),
    };
    if !nominal.is_finite() || nominal <= 0.0 {
        return (None, None);
    }
    let mut guard = PDH_FREQ_STATE.lock().unwrap_or_else(|p| p.into_inner());
    // 首次调用：初始化 PDH 查询（打开查询 + 添加计数器 + 建立采样基线）
    if guard.is_none() {
        match pdh_freq_init() {
            Ok(state) => {
                *guard = Some(state);
            }
            Err(e) => {
                // R8：初始化失败输出原因并标记不可用（避免每次调用重复尝试）
                log::warn!("[cpu_freq] PDH init failed, freq falls back to registry: {e}");
                *guard = Some(PdhFreqState {
                    query: 0,
                    counter: 0,
                    last_freq: None,
                    failed: true,
                    baseline_at: None,
                    warmup_done: true,
                });
                return (None, Some(format!("PDH不可用({e})")));
            }
        }
    }
    let Some(s) = guard.as_mut() else {
        // 防御：上方 init 分支必然置入 state，此处不可能为 None
        return (None, Some("PDH状态异常".to_string()));
    };
    if s.failed {
        return (None, Some("PDH不可用".to_string()));
    }
    // 首调 warmup：距基线采样不足最小间隔时阻塞补齐，保证首调返回有效估算值
    // （仅进程内第一次调用阻塞；此后 last_freq 有值，间隔不足时沿用不降级）
    if !s.warmup_done {
        if let Some(b) = s.baseline_at {
            let elapsed = b.elapsed();
            if elapsed < PDH_MIN_SAMPLE_INTERVAL {
                std::thread::sleep(PDH_MIN_SAMPLE_INTERVAL - elapsed);
            }
        }
        s.warmup_done = true;
    }
    // 每次调用收集新样本（返回码检查：R8 输出 API 名 + 错误码，盲区 B 修复）
    // SAFETY: query 为 PdhOpenQueryW 返回的有效句柄
    let rc = unsafe { PdhCollectQueryData(s.query) };
    if rc != 0 && rc != PDH_CSTATUS_NEW_DATA {
        log::warn!(
            "[cpu_freq] PdhCollectQueryData failed: win32=0x{:08X}, reuse last value or fallback",
            rc
        );
        if s.last_freq.is_none() {
            // 从未采样成功且本次收集失败：标记不可用，避免每次调用重复告警
            s.failed = true;
            return (None, Some(format!("PdhCollectQueryData(0x{:08X})", rc)));
        }
        // 已有有效值：沿用（瞬时失败不降级）
        return (s.last_freq, None);
    }
    s.baseline_at = Some(std::time::Instant::now());
    match read_perf_percent(s.counter) {
        Some(perf) => {
            let f = estimate_freq(nominal, perf);
            s.last_freq = Some(f);
            (Some(f), None)
        }
        // 间隔不足/无效：沿用上次有效估算值（避免抖动归零）
        None => match s.last_freq {
            Some(f) => (Some(f), None),
            None => (None, Some("PDH采样无有效数据".to_string())),
        },
    }
}

/// 注册表 `HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0\~MHz` 标称频率。
///
/// 缺失/非法时返回 None（降级链继续）。
pub fn nominal_freq_mhz() -> Option<u32> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    let key = match RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0")
    {
        Ok(k) => k,
        Err(e) => {
            log::warn!("[cpu_freq] open registry CentralProcessor\\0 failed: {e}");
            return None;
        }
    };
    match key.get_value::<u32, _>("~MHz") {
        Ok(v) if v > 0 => Some(v),
        Ok(_) => None,
        Err(e) => {
            log::warn!("[cpu_freq] read registry ~MHz failed: {e}");
            None
        }
    }
}

/// 估算频率纯逻辑：标称频率 × 性能百分比 / 100（MHz）。
///
/// PDH `% Processor Performance` 可 >100%（P 状态增强 / Turbo），理论上限
/// 400%（4× 标称）防荒谬值；NaN 按 0、负值按 0 处理。
fn estimate_freq(nominal_mhz: f32, perf_percent: f32) -> f32 {
    let perf = if perf_percent.is_nan() {
        0.0
    } else {
        perf_percent.clamp(0.0, 400.0)
    };
    (nominal_mhz * perf / 100.0).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// is_suspected_nominal 纯逻辑：疑似标称判定（设计 §3.2 语义）
    #[test]
    fn test_is_suspected_nominal() {
        // current == max → 疑似标称（AMD 恒标称场景）
        assert!(is_suspected_nominal(4200.0, 4200.0));
        // current < max × 0.98 → 非疑似（真实波动值，Intel 主路径不受影响）
        assert!(!is_suspected_nominal(4000.0, 4200.0));
        // 边界：current == max × 0.98 → 疑似（含边界，两侧为同一浮点运算位相等）
        assert!(is_suspected_nominal(4200.0 * 0.98, 4200.0));
        // 防御：max <= 0 → 非疑似（沿用"非零即成功"原行为）
        assert!(!is_suspected_nominal(4200.0, 0.0));
        assert!(!is_suspected_nominal(4200.0, -1.0));
        // 防御：current <= 0 → 非疑似
        assert!(!is_suspected_nominal(0.0, 4200.0));
        assert!(!is_suspected_nominal(-1.0, 4200.0));
    }

    /// estimate_freq 纯逻辑：标称 × perf/100；perf 越界截断；0/NaN 处理
    #[test]
    fn test_estimate_freq() {
        // 标称 × 100% = 标称
        assert!((estimate_freq(4200.0, 100.0) - 4200.0).abs() < 1e-3);
        // 降频：50%
        assert!((estimate_freq(4200.0, 50.0) - 2100.0).abs() < 1e-3);
        // Boost：109.5% → 4599（E5 实证场景）
        assert!((estimate_freq(4200.0, 109.5) - 4599.0).abs() < 1e-3);
        // perf 越界：负值 → 0
        assert!((estimate_freq(4200.0, -10.0) - 0.0).abs() < 1e-3);
        // perf 越界：>400% 截断到 400%（4× 标称）
        assert!((estimate_freq(4200.0, 500.0) - 16800.0).abs() < 1e-3);
        // 标称 0 → 0
        assert!((estimate_freq(0.0, 100.0) - 0.0).abs() < 1e-3);
        // NaN → 0
        assert!((estimate_freq(4200.0, f32::NAN) - 0.0).abs() < 1e-3);
        // 负数标称防御 → 0
        assert!((estimate_freq(-100.0, 50.0) - 0.0).abs() < 1e-3);
    }

    /// 真机 PDH 链路验证（默认忽略）：`% Processor Performance` × ~MHz 估算。
    /// v0.21.6 起首调会 warmup 补齐采样间隔，PDH 可用时首调即返回有效估算值
    /// （盲区 A 修复验证）。
    /// 运行：`cargo test -p secm-datasource -- --ignored real_machine_pdh_perf`
    #[test]
    #[ignore]
    fn real_machine_pdh_perf() {
        let nominal = nominal_freq_mhz();
        eprintln!("[真机] ~MHz 标称={nominal:?}");
        // 首调：warmup 补齐采样间隔后应直接返回有效估算值
        let first = pdh_perf_freq();
        eprintln!("[真机] pdh 首调={first:?} MHz（warmup 后应为 Some）");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let second = pdh_perf_freq();
        eprintln!("[真机] pdh 次调={second:?} MHz");
        if let Some(f) = second {
            assert!(f.is_finite() && f > 0.0, "估算频率非法: {f}");
        } else {
            // 虚拟机/计数器禁用场景：预期降级（不中断）
            eprintln!("[真机] PDH 通道不可用（虚拟机/禁用），预期降级");
        }
    }

    /// 真机降级链验证（默认忽略）：本机至少一个数据源可用
    /// 运行：`cargo test -p secm-datasource -- --ignored real_machine_cpu_freq_chain`
    #[test]
    #[ignore]
    fn real_machine_cpu_freq_chain() {
        let (freq, src) = get_cpu_freq();
        eprintln!("[真机] cpu_freq source={src} freq={freq:?} MHz");
        assert!(freq.is_some(), "降级链全失败: source={src}");
        assert!(
            ["ntapi", "pdh", "registry"].contains(&src),
            "数据源标识非法: {src}"
        );
    }

    /// 真机降级链诊断透出验证（默认忽略）：全失败时原因串非空且含各层 API 名；
    /// 成功路径原因串为空（诊断契约）。
    /// 运行：`cargo test -p secm-datasource -- --ignored real_machine_cpu_freq_diag`
    #[test]
    #[ignore]
    fn real_machine_cpu_freq_diag() {
        let (freq, src, diag) = get_cpu_freq_diag();
        eprintln!("[真机] diag source={src} freq={freq:?} reasons={diag:?}");
        if freq.is_none() {
            assert!(!diag.is_empty(), "全失败时必须透出失败原因: source={src}");
        } else {
            assert!(diag.is_empty(), "成功路径原因串应为空: {diag}");
        }
    }
}
