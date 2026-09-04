//! 硬件传感器服务 — GPU/CPU 温度/频率/功耗采集 + HTTP 接口
//!
//! 数据链路(按优先级降级,逐字段独立回退,无法采集的字段为 null):
//!   GPU 温度/频率/功耗: nvidia-smi(真实,NVIDIA 卡)→ LHM sidecar(温度,按名匹配)
//!   GPU 名称:           注册表 Class 键 DriverDesc(全部显卡,含核显/AMD/Intel)
//!   CPU 频率:           CallNtPowerInformation(powrprof.dll,每逻辑处理器)
//!   CPU 温度:           LHM sidecar(Package 温度回填;逐核温度无公开 API → null)
//!   CPU 功耗:           Windows 无公开 API → null
//!
//! 输出契约(前端渲染依据,任务书 JSON 形状):
//!   { gpus: [{ id, name, temperature, frequency, power }],
//!     cpus: [{ id, name, temperature, frequency, power }] }
//!   无法采集的字段一律 null;传感器不存在/权限不足返回明确错误码 + message。
//!
//! HTTP 接口:GET /api/hardware/sensors(127.0.0.1:17860,环境变量 SECM_HTTP_PORT 可覆盖)
//!   响应 < 500ms(nvidia-smi 冷启动约 100-300ms,其余为内存级查询)。
//!
//! Mock 模式:环境变量 SECM_HARDWARE_MOCK=1 时返回可配置模拟数据,
//!   用于无传感器环境(虚拟机/无独显/驱动未装)的前端联调;默认关闭。

use serde::Serialize;
use std::process::Command;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::{CommandExt, ExitStatusExt};

use secm_datasource::registry;

// ============================================================================
// 数据结构(任务书 JSON 契约)
// ============================================================================

/// 单个硬件传感器条目(GPU 卡或 CPU 逻辑处理器)。
#[derive(Debug, Clone, Serialize)]
pub struct HardwareSensorItem {
    /// 稳定标识,如 "gpu-0" / "cpu-0"
    pub id: String,
    /// 设备名称(GPU 为 DriverDesc/驱动名,CPU 为处理器品牌)
    pub name: String,
    /// 温度(℃);无法采集为 null
    pub temperature: Option<f64>,
    /// 频率(MHz);无法采集为 null
    pub frequency: Option<f64>,
    /// 功耗(W);无法采集为 null
    pub power: Option<f64>,
}

/// 硬件传感器整体响应(任务书 JSON 形状)。
#[derive(Debug, Clone, Serialize)]
pub struct HardwareSensorsResponse {
    pub gpus: Vec<HardwareSensorItem>,
    pub cpus: Vec<HardwareSensorItem>,
}

/// 错误码(HTTP 响应与 Tauri 命令共用)。
#[derive(Debug, Clone, Serialize)]
pub struct HardwareError {
    /// 机器可读错误码
    pub code: String,
    /// 人读错误消息(中文,含修复建议)
    pub message: String,
}

impl HardwareError {
    /// 传感器不存在(无 GPU/CPU 传感器或采集源缺失)
    pub fn not_found(detail: &str) -> Self {
        HardwareError {
            code: "HW_SENSOR_NOT_FOUND".into(),
            message: format!(
                "未找到可用的硬件传感器: {}(建议: 检查显卡驱动/nvidia-smi 是否安装,或启用 SECM_HARDWARE_MOCK=1 使用模拟数据)",
                detail
            ),
        }
    }

    /// 权限不足
    pub fn access_denied(detail: &str) -> Self {
        HardwareError {
            code: "HW_ACCESS_DENIED".into(),
            message: format!("硬件传感器访问被拒绝: {}(建议: 以管理员身份运行后重试)", detail),
        }
    }

    /// 内部采集失败
    pub fn internal(detail: &str) -> Self {
        HardwareError {
            code: "HW_INTERNAL_ERROR".into(),
            message: format!("硬件传感器采集失败: {}", detail),
        }
    }
}

// ============================================================================
// 常量与配置
// ============================================================================

/// HTTP 服务默认监听端口(127.0.0.1 仅本机可访问,安全边界)。
pub const DEFAULT_HTTP_PORT: u16 = 17860;

/// nvidia-smi 查询超时(秒);驱动挂起时避免请求线程无限等待。
const NVIDIA_SMI_TIMEOUT_SECS: u64 = 3;

/// Mock 模式环境变量:SECM_HARDWARE_MOCK=1 启用。
const MOCK_ENV: &str = "SECM_HARDWARE_MOCK";

/// HTTP 端口环境变量:SECM_HTTP_PORT 覆盖默认端口。
const PORT_ENV: &str = "SECM_HTTP_PORT";

/// 当前生效的 HTTP 端口(启动时解析一次)。
fn http_port() -> u16 {
    std::env::var(PORT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .filter(|p| *p > 0)
        .unwrap_or(DEFAULT_HTTP_PORT)
}

/// Mock 模式是否启用。
pub fn mock_enabled() -> bool {
    matches!(
        std::env::var(MOCK_ENV).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes")
    )
}

// ============================================================================
// nvidia-smi 采集(NVIDIA 卡真实温度/频率/功耗)
// ============================================================================

/// 单张 NVIDIA 卡的实时数据(nvidia-smi 解析结果)。
#[derive(Debug, Clone)]
struct NvidiaGpuData {
    index: u32,
    name: String,
    temperature: Option<f64>,
    frequency: Option<f64>,
    power: Option<f64>,
}

/// 带超时的子进程输出捕获(避免 nvidia-smi 挂起拖死 HTTP 请求线程)。
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Option<std::process::Output> {
    let (tx, rx) = std::sync::mpsc::channel::<std::process::Output>();
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return None,
    };
    // 共享 child 引用:等待线程 move 一份,主线程超时路径可 kill 同一进程
    let shared = std::sync::Arc::new(std::sync::Mutex::new(Some(child)));
    let waiter = shared.clone();
    let handle = std::thread::spawn(move || {
        let mut guard = waiter.lock().unwrap_or_else(|p| p.into_inner());
        // 仅 waiter 线程 take 一次;若已被超时路径 kill 取走则直接返回失败输出
        let child = match guard.take() {
            Some(c) => c,
            None => {
                let _ = tx.send(std::process::Output {
                    status: std::process::ExitStatus::from_raw(1),
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                });
                return;
            }
        };
        drop(guard); // wait_with_output 期间释放锁,允许超时路径 kill
        let output = child.wait_with_output();
        let _ = tx.send(output.unwrap_or_else(|_| {
            std::process::Output {
                status: std::process::ExitStatus::from_raw(1),
                stdout: Vec::new(),
                stderr: Vec::new(),
            }
        }));
    });
    match rx.recv_timeout(timeout) {
        Ok(out) => {
            let _ = handle.join();
            Some(out)
        }
        Err(_) => {
            // 超时:kill 共享 child(锁竞争时等待,子进程退出后系统回收)
            if let Ok(mut guard) = shared.lock() {
                if let Some(mut c) = guard.take() {
                    let _ = c.kill();
                }
            }
            let _ = handle.join();
            None
        }
    }
}

/// 调用 nvidia-smi 查询所有 GPU 的 温度/频率/功耗(csv,noheader)。
///
/// 返回 Ok(Some(Vec)) = 成功解析;Ok(None) = nvidia-smi 不存在/不可执行
/// (非 NVIDIA 环境,属预期降级,不算错误);Err = 调用成功但解析失败。
fn query_nvidia_smi() -> Result<Option<Vec<NvidiaGpuData>>, String> {
    use std::process::Stdio;
    let mut cmd = Command::new("nvidia-smi");
    cmd.args([
        "--query-gpu=index,name,temperature.gpu,clocks.sm,power.draw",
        "--format=csv,noheader,nounits",
    ])
    // 显式 piped:spawn() 默认继承 stdout,wait_with_output 捕获不到内容,
    // 导致解析空文本而降级到注册表分支(实测 nvidia-smi 输出漏打到父进程 stdout)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    // CREATE_NO_WINDOW:禁止子进程创建控制台窗口(硬件信息页每秒轮询时防闪 cmd 窗)
    .creation_flags(0x08000000);
    let output = match run_with_timeout(&mut cmd, Duration::from_secs(NVIDIA_SMI_TIMEOUT_SECS)) {
        Some(out) => out,
        None => {
            crate::debug_warn!("hardware", "nvidia-smi 不可执行或超时,GPU 传感器降级为 null");
            return Ok(None);
        }
    };
    if !output.status.success() {
        // 退出码非 0:无 NVIDIA 驱动/无 NVIDIA GPU(常见于 AMD/Intel 平台)
        crate::debug_warn!(
            "hardware",
            "nvidia-smi 退出码 {:?},视为无 NVIDIA GPU",
            output.status.code()
        );
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut gpus = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // CSV 字段:index, name, temperature.gpu, clocks.sm, power.draw
        // 名称可能含逗号(如 "NVIDIA GeForce RTX 2080 Ti, 11GB"),故首字段取 index,
        // 末尾三个字段取数值,中间 join 为名称。
        let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if parts.len() < 5 {
            crate::debug_warn!("hardware", "nvidia-smi 输出格式异常(字段不足): {}", line);
            continue;
        }
        let index = parts[0].parse::<u32>().unwrap_or(0);
        let name = parts[1..parts.len() - 3].join(", ");
        let parse_f = |s: &str| s.parse::<f64>().ok();
        let temperature = parse_f(parts[parts.len() - 3]);
        let frequency = parse_f(parts[parts.len() - 2]);
        let power = parse_f(parts[parts.len() - 1]);
        gpus.push(NvidiaGpuData {
            index,
            name,
            temperature,
            frequency,
            power,
        });
    }
    if gpus.is_empty() {
        Ok(None)
    } else {
        Ok(Some(gpus))
    }
}

// ============================================================================
// 硬件采集辅助
// ============================================================================

/// 从 LHM sidecar 快照提取温度（替代原 hwmon 驱动回填；自研内核驱动已移除）。
///
/// 返回 (gpu 名小写 → 温度映射, CPU Package 温度)。
/// LHM 快照不可用或无匹配时返回空映射与 None（温度字段按契约允许为 null）。
fn lhm_temps() -> (std::collections::HashMap<String, Option<f64>>, Option<f64>) {
    let mut gpu_map = std::collections::HashMap::new();
    let mut cpu_temp: Option<f64> = None;
    let (snap, _diag) = crate::lhm::lhm_snapshot();
    let Some(snap) = snap else {
        return (gpu_map, cpu_temp);
    };
    cpu_temp = snap.cpu.package_temp_c.map(|t| t as f64);
    for g in &snap.gpu {
        if let Some(t) = g.temperature_c {
            gpu_map.insert(g.name.to_lowercase(), Some(t as f64));
        }
    }
    (gpu_map, cpu_temp)
}

// ============================================================================
// CPU 采集(CallNtPowerInformation 每逻辑处理器频率 + LHM Package 温度回填)
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

/// 每逻辑处理器的当前频率(MHz)。
///
/// 主路径 `CallNtPowerInformation` 成功但平均频率"疑似标称"（AMD 7800X3D 等
/// 平台 `current_mhz` 恒等于标称基频，current ≈ max，不随负载变化）时，与
/// sensor.rs 主链路语义一致回退 datasource 用户态降级链（PDH 实时估算 →
/// 注册表标称，同值广播到每核）；API 失败时同样回退；仍失败才返回空 Vec
/// （F3 修复：不再因主 API 失败而全 None；v0.21.7 新增疑似标称判定）。
fn cpu_freq_per_core() -> Vec<f32> {
    // 缓冲区按 GetSystemInfo 权威逻辑处理器数分配（F4）
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
    if rc == 0 {
        // SAFETY: 缓冲区长度与结构体数组长度一致,由 count 保证
        let infos: &[ProcessorPowerInfo] = unsafe {
            std::slice::from_raw_parts(buf.as_ptr() as *const ProcessorPowerInfo, count as usize)
        };
        // 疑似标称判定（v0.21.7）：平均 current ≈ max 时判定为疑似标称
        // （AMD 恒标称平台），不直接采用，整体回退 datasource 降级链；
        // 非疑似标称（Intel 平台真实实时值）才直接返回每核 current_mhz。
        let sum_current: u32 = infos.iter().map(|p| p.current_mhz).sum();
        if sum_current > 0 {
            let sum_max: u32 = infos.iter().map(|p| p.max_mhz).sum();
            let avg_current = sum_current as f32 / count as f32;
            let avg_max = sum_max as f32 / count as f32;
            if !crate::datasource::cpu_freq::is_suspected_nominal(avg_current, avg_max) {
                // 真实实时值 → 直接采用每核 current_mhz
                return infos.iter().map(|p| p.current_mhz as f32).collect();
            }
            // 疑似标称 → 落入下方 datasource 降级链（PDH 估算同值广播到每核）
        } else {
            // API 成功但频率全为 0：无有效数据，按失败路径处理（落入下方降级链）
            crate::debug_warn!(
                "hardware",
                "CallNtPowerInformation returned 0 frequency, CPU freq falls back to datasource chain"
            );
        }
    }
    // 回退：datasource 用户态降级链（PDH 实时估算 → 注册表标称），同值广播到每核
    let (freq, src) = crate::datasource::cpu_freq::get_cpu_freq();
    match freq {
        Some(f) if f > 0.0 => {
            crate::debug_warn!(
                "hardware",
                "CallNtPowerInformation suspected nominal or failed, CPU freq fallback {src} = {:.0} MHz",
                f
            );
            vec![f; count as usize]
        }
        _ => {
            crate::debug_warn!(
                "hardware",
                "CallNtPowerInformation and fallback chain failed, CPU freq empty"
            );
            Vec::new()
        }
    }
}

// ============================================================================
// Mock 数据(无传感器环境联调用;SECM_HARDWARE_MOCK=1 启用)
// ============================================================================

/// 生成可配置的模拟传感器数据。
///
/// 可配置项(环境变量):
///   SECM_HARDWARE_MOCK_GPUS  模拟 GPU 数量(默认 2)
///   SECM_HARDWARE_MOCK_CORES 模拟 CPU 逻辑处理器数量(默认 8,0=用本机核数)
fn mock_response() -> HardwareSensorsResponse {
    let gpu_count = std::env::var("SECM_HARDWARE_MOCK_GPUS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(2)
        .clamp(1, 4);
    let cores_env = std::env::var("SECM_HARDWARE_MOCK_CORES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let core_count = if cores_env > 0 {
        cores_env
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8)
    };

    let mock_gpu_names = ["NVIDIA GeForce RTX 4080 (Mock)", "AMD Radeon RX 7800 XT (Mock)"];
    let gpus = (0..gpu_count)
        .map(|i| HardwareSensorItem {
            id: format!("gpu-{}", i),
            name: mock_gpu_names[i % mock_gpu_names.len()].to_string(),
            temperature: Some(45.0 + (i as f64 * 6.0) + 3.0),
            frequency: Some(2100.0 - (i as f64 * 300.0)),
            power: Some(180.0 - (i as f64 * 60.0)),
        })
        .collect();

    let cpus = (0..core_count)
        .map(|i| HardwareSensorItem {
            id: format!("cpu-{}", i),
            name: format!("Intel Core i7-14700K (Mock) Core #{}", i),
            temperature: Some(38.0 + (i as f64 % 8.0) * 2.5),
            frequency: Some(4200.0 - (i as f64 * 120.0)),
            power: Some(35.0 + (i as f64 % 4.0) * 8.0),
        })
        .collect();

    HardwareSensorsResponse { gpus, cpus }
}

// ============================================================================
// 采集主入口
// ============================================================================

/// 采集 GPU/CPU 传感器数据(逐字段独立降级,无法采集为 null)。
///
/// 错误语义:
/// - 返回 Err(HardwareError) = 采集源整体不可用(权限/内部错误),HTTP 层映射 4xx/5xx;
/// - 返回 Ok(Response) = 结构完整(字段可能为 null),HTTP 层返回 200。
pub fn collect_sensors() -> Result<HardwareSensorsResponse, HardwareError> {
    // Mock 模式:直接返回模拟数据(用于无传感器环境联调)
    if mock_enabled() {
        crate::debug_info!("hardware", "SECM_HARDWARE_MOCK=1,mock 模式返回模拟传感器数据");
        return Ok(mock_response());
    }

    // ---- GPU ----
    let cards = registry::enum_gpu_cards().map_err(|e| {
        crate::debug_error!("hardware", "枚举 GPU 失败: {}", e);
        HardwareError::access_denied(&format!("注册表 GPU 枚举失败({})", e))
    })?;
    // 过滤虚拟显示适配器(远程桌面/模拟器/基础显示回退等非物理 GPU):
    //   - Microsoft Basic Display Adapter(无驱动回退)
    //   - 名称含 Virtual(虚拟显示适配器,如 MuMu/Todesk/GameViewer)
    let physical_cards: Vec<&registry::GpuCard> = cards
        .iter()
        .filter(|c| {
            let lower = c.name.to_lowercase();
            !lower.contains("microsoft basic display")
                && !lower.contains("virtual display")
                && !lower.contains("virtual adapter")
        })
        .collect();
    if physical_cards.is_empty() && !cards.is_empty() {
        crate::debug_warn!(
            "hardware",
            "注册表枚举 {} 张显卡但全部为虚拟适配器,视为无物理 GPU",
            cards.len()
        );
    }

    // nvidia-smi 真实数据(可用的 NVIDIA 卡);其余卡温度尽力从 LHM 快照匹配
    let nvidia = query_nvidia_smi().map_err(|e| HardwareError::internal(&e))?;
    // LHM 快照 GPU 温度映射（按驱动名近似匹配；快照不可用/无匹配时温度 null）
    let (lhm_gpu_map, lhm_cpu_temp) = lhm_temps();

    let mut gpus: Vec<HardwareSensorItem> = Vec::new();
    // nvidia-smi 优先:以驱动枚举为准(权威的 NVIDIA GPU 列表)
    if let Some(nv_gpus) = &nvidia {
        for g in nv_gpus {
            gpus.push(HardwareSensorItem {
                id: format!("gpu-{}", g.index),
                name: g.name.clone(),
                temperature: g.temperature,
                frequency: g.frequency,
                power: g.power,
            });
        }
        // 追加注册表中非 NVIDIA 的物理卡(核显/AMD/Intel),温度尽力从 LHM 匹配,频率/功耗 null
        for card in &physical_cards {
            if card.vendor == "nvidia" {
                continue; // nvidia-smi 已覆盖
            }
            gpus.push(HardwareSensorItem {
                id: format!("gpu-{}", card.key),
                name: card.name.clone(),
                temperature: lhm_gpu_map.get(&card.name.to_lowercase()).copied().flatten(),
                frequency: None,
                power: None,
            });
        }
    } else {
        // 无 nvidia-smi:NVIDIA/AMD/Intel 全部走注册表 + LHM(温度)/null
        for card in &physical_cards {
            gpus.push(HardwareSensorItem {
                id: format!("gpu-{}", card.key),
                name: card.name.clone(),
                temperature: lhm_gpu_map.get(&card.name.to_lowercase()).copied().flatten(),
                frequency: None,
                power: None,
            });
        }
    }

    // ---- CPU ----
    let freqs = cpu_freq_per_core();
    let core_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(freqs.len().max(1));

    let mut cpus: Vec<HardwareSensorItem> = Vec::new();
    // CPU 名称:用 sysinfo 读取品牌(仅一次,失败则通用名)
    let brand = {
        let mut sys = sysinfo::System::new();
        sys.refresh_cpu();
        sys.cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .unwrap_or_else(|| "Unknown CPU".into())
    };
    for i in 0..core_count {
        // 每核温度:Windows 无公开逐核温度 API——尽力用 LHM CPU Package 温度回填,
        // 不可用时 null(字段契约允许)
        let temp = lhm_cpu_temp;
        cpus.push(HardwareSensorItem {
            id: format!("cpu-{}", i),
            name: format!("{} Core #{}", brand, i),
            temperature: temp,
            frequency: freqs.get(i).copied().map(|f| f as f64),
            power: None, // Windows 无公开 CPU 功耗 API
        });
    }

    // 整体不可用判定:GPU 枚举失败且 CPU 无任何频率/温度数据 → 明确错误码
    let any_gpu_data = gpus.iter().any(|g| {
        g.temperature.is_some() || g.frequency.is_some() || g.power.is_some()
    });
    let any_cpu_data = cpus.iter().any(|c| {
        c.temperature.is_some() || c.frequency.is_some() || c.power.is_some()
    });
    if !any_gpu_data && !any_cpu_data {
        return Err(HardwareError::not_found(
            "GPU 无驱动枚举数据且 CPU 无频率/温度数据",
        ));
    }

    Ok(HardwareSensorsResponse { gpus, cpus })
}

// ============================================================================
// HTTP 服务(tiny_http,127.0.0.1 仅本机)
// ============================================================================

/// 启动 HTTP 服务(独立线程,不阻塞主线程)。
///
/// 路由:
///   GET /api/hardware/sensors → 200 JSON(字段 null 表达不可采集)
///                               403 { error: { code: HW_ACCESS_DENIED, ... } }
///                               404 { error: { code: HW_SENSOR_NOT_FOUND, ... } }
///                               500 { error: { code: HW_INTERNAL_ERROR, ... } }
///   其他路径 → 404 { error: { code: HW_NOT_FOUND, ... } }
///   非 GET  → 405 { error: { code: HW_METHOD_NOT_ALLOWED, ... } }
///
/// 返回监听端口(用于日志);绑定失败返回 Err(调用方记日志,不中断主程序)。
pub fn start_http_server() -> Result<u16, String> {
    let port = http_port();
    let addr = format!("127.0.0.1:{}", port);
    let server = tiny_http::Server::http(&addr)
        .map_err(|e| format!("HTTP 服务绑定 {} 失败: {}", addr, e))?;
    crate::debug_info!(
        "hardware",
        "硬件传感器 HTTP 服务已启动: http://{}{}",
        addr,
        "/api/hardware/sensors"
    );

    std::thread::Builder::new()
        .name("hardware-http".into())
        .spawn(move || {
            for request in server.incoming_requests() {
                // 每个请求独立线程处理:采集耗时(≤300ms)不阻塞其他请求
                std::thread::spawn(move || handle_request(request));
            }
        })
        .map_err(|e| format!("HTTP 服务线程启动失败: {}", e))?;

    Ok(port)
}

/// 单请求处理:路由 + 采集 + JSON 响应。
fn handle_request(request: tiny_http::Request) {
    use tiny_http::{Method, Response, StatusCode};

    let respond_json = |status: StatusCode, body: String| -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
        let mut resp = Response::from_string(body).with_status_code(status);
        // R2: Header 由硬编码 ASCII 字面量构建(理论不可能失败)，失败时静默降级为
        // 不带该头，绝不 unwrap；浏览器侧缺失 Content-Type 时按 MIME 嗅探仍可解析 JSON。
        if let Ok(h) = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]) {
            resp = resp.with_header(h);
        }
        if let Ok(h) = tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]) {
            resp = resp.with_header(h);
        }
        resp
    };

    let path = request.url().split('?').next().unwrap_or("").to_string();
    let method = request.method().clone();

    // 路由分发
    let (status, body): (StatusCode, String) = match (method, path.as_str()) {
        (Method::Get, "/api/hardware/sensors") | (Method::Get, "/api/hardware/sensors/") => {
            // 采集在请求线程内执行(独立线程,不阻塞 GUI;nvidia-smi ≤3s 超时兜底)
            match collect_sensors() {
                Ok(resp) => {
                    let json = serde_json::to_string(&resp).unwrap_or_else(|e| {
                        format!(r#"{{"error":{{"code":"HW_INTERNAL_ERROR","message":"JSON 序列化失败: {}"}}}}"#, e)
                    });
                    (StatusCode(200), json)
                }
                Err(e) => {
                    let json = serde_json::to_string(&e).unwrap_or_else(|_| {
                        r#"{"code":"HW_INTERNAL_ERROR","message":"错误序列化失败"}"#.into()
                    });
                    let status = match e.code.as_str() {
                        "HW_ACCESS_DENIED" => StatusCode(403),
                        "HW_SENSOR_NOT_FOUND" => StatusCode(404),
                        _ => StatusCode(500),
                    };
                    (status, json)
                }
            }
        }
        (_, _) if path == "/api/hardware/sensors" || path == "/api/hardware/sensors/" => {
            let body = r#"{"error":{"code":"HW_METHOD_NOT_ALLOWED","message":"仅支持 GET 请求"}}"#;
            (StatusCode(405), body.to_string())
        }
        _ => {
            let body = format!(
                r#"{{"error":{{"code":"HW_NOT_FOUND","message":"接口不存在: {} (可用: GET /api/hardware/sensors)"}}}}"#,
                path
            );
            (StatusCode(404), body)
        }
    };

    let response = respond_json(status, body);
    let _ = request.respond(response);
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 环境变量为进程级,并行单测改 env 会互相干扰,统一串行锁。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_mock_response_shape() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // 临时设置 mock 环境变量,验证响应结构与字段类型
        unsafe {
            std::env::set_var(MOCK_ENV, "1");
        }
        let resp = mock_response();
        assert!(!resp.gpus.is_empty());
        assert!(!resp.cpus.is_empty());
        for g in &resp.gpus {
            assert!(g.id.starts_with("gpu-"));
            assert!(!g.name.is_empty());
            assert!(g.temperature.is_some());
            assert!(g.frequency.is_some());
            assert!(g.power.is_some());
        }
        for c in &resp.cpus {
            assert!(c.id.starts_with("cpu-"));
            assert!(!c.name.is_empty());
        }
        unsafe {
            std::env::remove_var(MOCK_ENV);
        }
    }

    #[test]
    fn test_error_codes() {
        let e = HardwareError::not_found("测试");
        assert_eq!(e.code, "HW_SENSOR_NOT_FOUND");
        assert!(e.message.contains("建议"));
        let e = HardwareError::access_denied("测试");
        assert_eq!(e.code, "HW_ACCESS_DENIED");
        assert!(e.message.contains("管理员"));
        let e = HardwareError::internal("测试");
        assert_eq!(e.code, "HW_INTERNAL_ERROR");
        assert!(!e.message.is_empty());
    }

    #[test]
    fn test_http_port_env() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            std::env::set_var(PORT_ENV, "19999");
            assert_eq!(http_port(), 19999);
            std::env::remove_var(PORT_ENV);
        }
        assert_eq!(http_port(), DEFAULT_HTTP_PORT);
    }

    #[test]
    fn test_cpu_freq_shape() {
        // 每逻辑处理器频率数组:长度应与权威逻辑处理器数一致(或为空)
        let freqs = cpu_freq_per_core();
        if !freqs.is_empty() {
            let count = authoritative_cpu_count();
            assert_eq!(freqs.len(), count as usize);
        }
    }

    #[test]
    fn test_collect_shape() {
        // 真实采集(本机有 nvidia-smi):GPU/CPU 结构完整,字段可为 null
        match collect_sensors() {
            Ok(resp) => {
                for g in &resp.gpus {
                    assert!(g.id.starts_with("gpu-"));
                    assert!(!g.name.is_empty());
                }
                for c in &resp.cpus {
                    assert!(c.id.starts_with("cpu-"));
                    assert!(!c.name.is_empty());
                }
            }
            Err(e) => {
                // 极端环境(无 GPU 无 CPU 数据)允许明确错误码
                assert!(["HW_SENSOR_NOT_FOUND", "HW_ACCESS_DENIED", "HW_INTERNAL_ERROR"]
                    .contains(&e.code.as_str()));
            }
        }
    }
}
