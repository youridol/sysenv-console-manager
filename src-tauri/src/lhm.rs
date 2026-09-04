//! LHM（LibreHardwareMonitor）sidecar 数据源模块。
//!
//! 架构：SECM 通过 HTTP 消费独立 sidecar 进程（LhmSidecar.exe，.NET 8）输出的
//! CPU 温度/功耗 JSON；LibreHardwareMonitorLib（MPL-2.0，内嵌 PawnIO ring0 驱动
//! LGPL-2.1）仅存在于 sidecar 进程内，进程隔离规避许可对 MIT 主体的传染。
//!
//! 权限模型：PawnIO 设备（\\.\PawnIO）DACL 仅 SYSTEM/Administrators 可访问，
//! SECM 主进程以普通权限运行 → sidecar 启动时自提权（runas UAC 重启自身）。
//! 提权后原 sidecar 进程退出、提权子进程接管，SECM 失去子进程句柄——
//! 因此本模块以「端口服务探测（health_check）」判定服务可用性，而非依赖句柄。
//!
//! 职责：
//!   1. sidecar 进程生命周期（lazy 启动 / 崩溃自动重启一次 / 退出清理）
//!   2. HTTP 轮询（/health 健康检查 + /api/lhm/sensors 传感器 JSON）
//!   3. JSON 解析（serde）→ 回填 CpuData.temperature（temp_source="lhm"）
//!   4. 降级：任何失败返回 (None, diag)，由 sensor.rs 继续 winring0/acpi 链
//!
//! 降级策略（不劣化现状）：
//!   - sidecar exe 不存在 → 确定性失败，标记降级（后续不再启动）
//!   - 启动中/UAC 等待/崩溃 → 可重试失败，退避期内不重复 spawn（避免每轮阻塞）
//!   - 传感器轮询连续 3 次失败 → kill 进程 + 标记降级
//!   - sidecar 存活但 available:false（驱动/权限不可用）→ 返回 None（不 kill 不降级，
//!     sidecar 内部周期重试，用户放行/提权后自动恢复）

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// sidecar 监听端口（与 sidecar 默认值一致）。
const LHM_PORT: u16 = 45980;
/// 健康检查单次超时。
const LHM_HEALTH_TIMEOUT: Duration = Duration::from_millis(500);
/// 传感器轮询超时。
const LHM_SENSORS_TIMEOUT: Duration = Duration::from_secs(2);
/// 启动健康检查重试次数（间隔 1s，共约 5s；覆盖 sidecar 冷启动 + UAC 提权移交）。
const LHM_START_RETRIES: u32 = 5;
/// 连续轮询失败上限（达到后 kill + 降级）。
const LHM_MAX_FAILURES: u32 = 3;
/// sidecar 崩溃自动重启次数上限（超过则降级）。
const LHM_MAX_RESTARTS: u32 = 1;
/// spawn 退避间隔：上次 spawn 后此期间内不重复 spawn / 不探测端口（避免每轮阻塞）。
const LHM_SPAWN_BACKOFF: Duration = Duration::from_secs(10);
/// 快照缓存有效期：`lhm_cpu_temp()` 命中此窗口内的 `last_snapshot` 直接复用
/// （get_sensor_data 每采集周期先调 `lhm_snapshot()` 刷新缓存，CPU 温度读取
/// 零额外 HTTP；独立调用超过窗口则转调完整轮询保证数据新鲜）。
const LHM_SNAPSHOT_TTL: Duration = Duration::from_secs(2);
/// CREATE_NO_WINDOW：防止 sidecar 弹出控制台黑窗（参考 proc_util 先例）。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ============================================================================
// 契约结构（sidecar JSON，见 docs/designs/2026-08-09-lhm-datasource.md §5.1）
// ============================================================================

/// CPU 传感器数据（sidecar JSON 契约的 cpu 对象，v1.0 起结构不变）。
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmCpuData {
    /// CPU Package 温度（℃）
    pub package_temp_c: Option<f32>,
    /// 各核心/CCD 温度数组
    pub core_temps_c: Vec<f32>,
    /// CPU Package 功耗（W）
    pub power_w: Option<f32>,
    /// CPU 风扇转速（RPM，LHM 有则回填）
    pub fan_rpm: Option<f32>,
    /// CPU 核心电压（V，LHM 有则回填）
    pub voltage_v: Option<f32>,
}

/// GPU 传感器数据（v2.0 契约新增；可空字段用 Option，前端 "—" 兜底）。
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmGpuData {
    /// GPU 名称（厂商驱动命名，如 "NVIDIA GeForce GTX 1660 Ti"）
    pub name: String,
    /// 核心温度（℃）
    pub temperature_c: Option<f32>,
    /// 核心频率（MHz）
    pub core_clock_mhz: Option<f32>,
    /// 功耗（W；Intel 核显等可能无功耗，null 兜底）
    pub power_w: Option<f32>,
    /// 利用率（%）
    pub load_percent: Option<f32>,
    /// 显存已用（字节）
    pub memory_used_bytes: Option<u64>,
    /// 显存总量（字节）
    pub memory_total_bytes: Option<u64>,
    /// 风扇转速（RPM；无风扇/无转速读数时 null）
    pub fan_rpm: Option<f32>,
}

/// 主板单个传感器（SuperIO 温度/风扇/电压）。
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmMbSensor {
    /// 传感器名称（如 "System" / "Fan #1" / "+12V"）
    pub name: String,
    /// 传感器类型："temperature" | "fan" | "voltage"（LHM SensorType 映射）
    #[serde(rename = "type")]
    pub kind: String,
    /// 传感器数值（温度℃ / 风扇 RPM / 电压 V）
    pub value: f32,
}

/// 主板传感器数据（v2.0 契约新增；主板不可用时顶层为 null）。
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmMotherboardData {
    /// 主板名称（LHM 识别；无则 None）
    pub name: Option<String>,
    /// 传感器列表（无数据 → 空数组）
    pub sensors: Vec<LhmMbSensor>,
}

/// 内存数据（v2.0 契约新增；SPD 型号一次性读取，容量与 GlobalMemoryStatusEx 冗余）。
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmMemoryData {
    /// SPD 型号汇总（如 "DDR4-2400 2x8GB"；不可用时 None）
    pub name: Option<String>,
    /// 总容量（字节；与权威值冗余，前端仍用 GlobalMemoryStatusEx）
    pub total_bytes: Option<u64>,
    /// 已用容量（字节）
    pub used_bytes: Option<u64>,
}

/// sidecar /api/lhm/sensors 响应（v2.0 契约，顶层新增字段，cpu 结构不变）。
///
/// `#[serde(default)]` 保证旧 v1 sidecar（无 gpu/motherboard/memory/contract_version
/// 字段）与缺字段 JSON 解析不 panic——向后兼容（新 SECM 可消费旧 sidecar，旧 SECM
/// 忽略未知字段）。
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmSensorResponse {
    /// 采集是否可用
    pub available: bool,
    /// 不可用原因（含驱动/权限错误，供引导卡复用）
    pub error: Option<String>,
    /// sidecar 契约版本（v2.0 起=2；旧 v1 响应缺失时为 None）
    pub contract_version: Option<u32>,
    /// CPU 传感器数据（v1.0 结构不变）
    pub cpu: LhmCpuData,
    /// GPU 传感器列表（v2.0 新增；无 GPU 或不可用时为空数组）
    pub gpu: Vec<LhmGpuData>,
    /// 主板传感器数据（v2.0 新增；主板不可用时 None）
    pub motherboard: Option<LhmMotherboardData>,
    /// 内存数据（v2.0 新增；LHM 内存不可用时 None）
    pub memory: Option<LhmMemoryData>,
}

// ============================================================================
// 进程状态
// ============================================================================

/// sidecar 进程与降级状态（全局单例，Mutex 保护）。
struct LhmState {
    /// 最近一次 spawn 的子进程句柄（提权接管的进程退出后句柄失效，置 None）
    child: Option<Child>,
    /// 是否已成功启动过（区分首次启动与崩溃重启）
    started: bool,
    /// 是否已降级（确定性失败后不再尝试启动）
    degraded: bool,
    /// 服务是否已验证可用（提权接管后以端口服务为准，缓存避免每轮探测）
    ready: bool,
    /// 连续轮询失败计数
    failures: u32,
    /// sidecar 崩溃重启计数
    restarts: u32,
    /// 上次 spawn 尝试时间（退避控制）
    last_spawn: Option<Instant>,
    /// spawn 出的子进程是否正卡在 UAC 确认弹窗（存活但服务未就绪）：
    /// true 时不 kill 不重复 spawn（防 UAC 弹窗堆积），退避后只探测端口
    uac_pending: bool,
    /// 最近一次成功的全量快照缓存（`lhm_cpu_temp()` 薄封装复用，避免
    /// get_sensor_data 每周期先取全量快照再重复 HTTP 轮询）
    last_snapshot: Option<(Instant, LhmSensorResponse)>,
}

/// 全局状态单例。
fn state() -> &'static Mutex<LhmState> {
    static STATE: OnceLock<Mutex<LhmState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(LhmState {
            child: None,
            started: false,
            degraded: false,
            ready: false,
            failures: 0,
            restarts: 0,
            last_spawn: None,
            uac_pending: false,
            last_snapshot: None,
        })
    })
}

/// sidecar 可执行文件路径（init 时解析并缓存；未 init 时现场解析）。
static EXE_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// sidecar 启动错误：区分确定性失败（应降级）与可重试失败。
enum LhmStartError {
    /// sidecar 可执行文件不存在：确定性失败，应降级
    ExeMissing,
    /// 其他失败（spawn/健康检查/端口占用/崩溃超限）：可重试
    Retryable(String),
}

// ============================================================================
// 资源定位（4 种布局，参考 driver_install::resolve_driver_dir 模式）
// ============================================================================

/// 定位 sidecar 可执行文件：
/// 1. 环境变量 SECM_LHM_DIR（测试注入，指向 publish 目录）
/// 2. Tauri resource_dir()/lhm/publish（NSIS 安装布局）
/// 3. exe 同目录 resources/lhm/publish（便携版布局）
/// 4. 当前工作目录 src-tauri/resources/lhm/publish（开发运行布局）
fn resolve_lhm_exe(app: Option<&tauri::AppHandle>) -> Option<PathBuf> {
    // 1. 环境变量注入（测试/排障用）—— 仅 debug 构建生效，release 不读取（防注入面，M1）
    #[cfg(debug_assertions)]
    if let Ok(p) = std::env::var("SECM_LHM_DIR") {
        if !p.is_empty() {
            let exe = PathBuf::from(p).join("LhmSidecar.exe");
            if exe.exists() {
                return Some(exe);
            }
        }
    }
    // 2. Tauri resource_dir（NSIS 安装布局）
    if let Some(app) = app {
        use tauri::Manager;
        if let Ok(res) = app.path().resource_dir() {
            let exe = res.join("lhm").join("publish").join("LhmSidecar.exe");
            if exe.exists() {
                return Some(exe);
            }
        }
    }
    // 3. exe 同目录 / 当前目录（便携 + 开发布局）
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_default();
    for base in [exe_dir.clone(), PathBuf::from(".")] {
        let exe = base
            .join("resources")
            .join("lhm")
            .join("publish")
            .join("LhmSidecar.exe");
        if exe.exists() {
            return Some(exe);
        }
    }
    // 4. 开发运行目录（cargo run 场景）
    let exe = PathBuf::from(".")
        .join("src-tauri")
        .join("resources")
        .join("lhm")
        .join("publish")
        .join("LhmSidecar.exe");
    if exe.exists() {
        return Some(exe);
    }
    None
}

/// 获取已缓存或现场解析的 exe 路径。
fn lhm_exe() -> Option<PathBuf> {
    if let Some(cached) = EXE_PATH.get() {
        return cached.clone();
    }
    resolve_lhm_exe(None)
}

// ============================================================================
// HTTP 客户端（ureq，阻塞；在 sensor 查询链的 spawn_blocking 上下文内调用）
// ============================================================================

/// 健康检查：GET /health（500ms 超时）。
fn health_check() -> Result<(), String> {
    let url = format!("http://127.0.0.1:{}/health", LHM_PORT);
    let resp = ureq::get(&url)
        .timeout(LHM_HEALTH_TIMEOUT)
        .call()
        .map_err(|e| format!("health HTTP 错误: {}", e))?;
    if resp.status() != 200 {
        return Err(format!("health HTTP 状态码 {}", resp.status()));
    }
    // 校验响应体为 LHM sidecar 服务标识（防止端口被其他程序占用时误判，M2）
    let body = resp
        .into_string()
        .map_err(|e| format!("health 响应读取失败: {}", e))?;
    if !body.contains("\"status\":\"ok\"") {
        return Err(format!(
            "health 响应异常（疑似端口 {} 被其他程序占用）: {}",
            LHM_PORT,
            body.chars().take(60).collect::<String>()
        ));
    }
    Ok(())
}

/// 轮询传感器：GET /api/lhm/sensors（2s 超时）→ serde 解析。
fn fetch_sensors() -> Result<LhmSensorResponse, String> {
    let url = format!("http://127.0.0.1:{}/api/lhm/sensors", LHM_PORT);
    let resp = ureq::get(&url)
        .timeout(LHM_SENSORS_TIMEOUT)
        .call()
        .map_err(|e| format!("sensors HTTP 错误: {}", e))?;
    if resp.status() != 200 {
        return Err(format!("sensors HTTP 状态码 {}", resp.status()));
    }
    let text = resp.into_string().map_err(|e| format!("读取响应体失败: {}", e))?;
    serde_json::from_str(&text).map_err(|e| format!("JSON 解析失败: {}", e))
}

// ============================================================================
// 进程生命周期
// ============================================================================

/// 启动 sidecar：spawn + 健康检查（至多 5 次 × 1s）。
///
/// sidecar 会自提权（UAC）——spawn 出的进程可能立即退出、由提权子进程接管，
/// 健康检查探测的是 127.0.0.1:45980 的服务（无论由哪个进程提供）。
/// 失败时按存活状态分类处理（UAC 竞态修复，见下方）；不在此处降级（由调用方决定）。
fn spawn_sidecar(st: &mut LhmState) -> Result<(), LhmStartError> {
    st.last_spawn = Some(Instant::now());
    let exe = lhm_exe().ok_or(LhmStartError::ExeMissing)?;

    // sidecar 日志重定向到系统临时目录（追加）；提权子进程另有文件日志兜底
    let log_path = std::env::temp_dir().join("secm-lhm-sidecar.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| LhmStartError::Retryable(format!("打开 sidecar 日志 {} 失败: {}", log_path.display(), e)))?;

    use std::os::windows::process::CommandExt;
    let mut child = Command::new(&exe)
        .args(["--port", &LHM_PORT.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::from(
            log_file
                .try_clone()
                .map_err(|e| LhmStartError::Retryable(format!("stdout 重定向失败: {}", e)))?,
        ))
        .stderr(Stdio::from(log_file))
        .spawn()
        .map_err(|e| LhmStartError::Retryable(format!("spawn {} 失败: {}", exe.display(), e)))?;

    // 健康检查（覆盖冷启动 JIT + 提权移交；UAC 确认等待超出 5s 由下方存活判定接管）
    for i in 0..LHM_START_RETRIES {
        if health_check().is_ok() {
            st.child = Some(child);
            st.started = true;
            st.ready = true;
            st.uac_pending = false;
            crate::debug_info!("lhm", "sidecar 服务就绪（端口 {}）", LHM_PORT);
            return Ok(());
        }
        if i + 1 < LHM_START_RETRIES {
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    // 健康检查超时：先判定子进程是否存活（UAC 提权竞态修复）——
    // Ok(None) = 进程存活，大概率卡在 UAC 确认弹窗（Program.cs TryElevate 同步
    //   阻塞等待用户）→ **不 kill**（kill 会取消 UAC 请求导致永远起不来），把句柄
    //   存入状态 + 标记 uac_pending，返回 Retryable；退避后 ensure_running 只探测
    //   端口（用户确认 UAC 后提权子进程接管端口，服务自动恢复），防弹窗堆积。
    // 已退出 / 查询失败 = spawn 出的进程已结束（提权移交成功 / 崩溃 / 用户拒绝
    //   UAC）→ kill 兜底清理（对已退出进程 no-op），避免残留。
    match child.try_wait() {
        Ok(None) => {
            st.child = Some(child);
            st.uac_pending = true;
            crate::debug_warn!(
                "lhm",
                "sidecar 健康检查超时但进程存活（疑似等待 UAC 确认），不 kill；退避后重试探测端口"
            );
            Err(LhmStartError::Retryable(format!(
                "sidecar 等待 UAC 确认（{} 次健康检查无响应，进程存活；请在 UAC 弹窗点击「是」）",
                LHM_START_RETRIES
            )))
        }
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            Err(LhmStartError::Retryable(format!(
                "sidecar 健康检查失败（{} 次重试后 127.0.0.1:{} 无响应；可能是 UAC 未确认或启动异常）",
                LHM_START_RETRIES, LHM_PORT
            )))
        }
    }
}

/// 确保 sidecar 服务可用：句柄存活 / 端口有服务（提权接管）/ 重启 / 首次启动。
fn ensure_running(st: &mut LhmState) -> Result<(), LhmStartError> {
    // A. 子进程句柄存活 → 直接可用（正常启动成功路径）；
    //    uac_pending（存活但服务未就绪）时继续走下方退避/探测逻辑（C/C'），
    //    不在此返回——保证 UAC 探测受退避节流（防每轮 500ms 端口探测阻塞）。
    if let Some(child) = &mut st.child {
        match child.try_wait() {
            // 进程存活
            Ok(None) => {
                if !st.uac_pending {
                    return Ok(());
                }
            }
            // 已退出或查询失败：丢弃句柄（提权接管场景原进程退出是正常的；
            // 用户拒绝 UAC 后进程退出亦在此清理，uac_pending 一并清除）
            _ => {
                st.child = None;
                st.uac_pending = false;
            }
        }
    }
    // B. 服务已验证可用（提权子进程接管后 SECM 无句柄，以端口服务为准）
    if st.ready {
        return Ok(());
    }
    // C. 退避期内：不重复 spawn / 不探测端口（last_spawn 同时充当 UAC 探测节流，
    //    避免 UAC 等待期间每轮 500ms HTTP 阻塞传感器轮询）
    if let Some(t) = st.last_spawn {
        if t.elapsed() < LHM_SPAWN_BACKOFF {
            return Err(LhmStartError::Retryable("sidecar 启动中（退避期内）".to_string()));
        }
    }
    // C'. UAC 挂起（退避已结束）：只探测端口，不重复 spawn——用户确认 UAC 后
    //     提权子进程接管端口即命中；未确认则重置节流时间戳，下个窗口再探测
    //     （防弹窗堆积 + 防每轮阻塞）
    if st.uac_pending {
        if health_check().is_ok() {
            st.uac_pending = false;
            st.ready = true;
            return Ok(());
        }
        st.last_spawn = Some(Instant::now());
        return Err(LhmStartError::Retryable(
            "sidecar 等待 UAC 确认（进程存活，服务未就绪；退避后重试探测端口）".to_string(),
        ));
    }
    // D. 端口有服务（可能是历史提权子进程 / 其他实例）：视为可用
    if health_check().is_ok() {
        st.ready = true;
        return Ok(());
    }
    // E. 需要启动/重启
    if st.started {
        st.restarts += 1;
        if st.restarts > LHM_MAX_RESTARTS {
            return Err(LhmStartError::Retryable(format!(
                "sidecar 服务不可达已超过重启上限（{} 次），降级原链",
                st.restarts
            )));
        }
        crate::debug_warn!("lhm", "sidecar 服务不可达，自动重启（第 {} 次）", st.restarts);
    }
    // 启动 sidecar 前确保 PawnIO 驱动服务就绪（LHM 0.9.6 不装驱动，依赖系统 PawnIO
    // 服务，见 docs/designs/2026-08-09-pawnio-bundle.md §2.2）。幂等：pawnio.rs 内部
    // 30s 缓存节流；失败仅记日志——sidecar 仍会启动并报 available:false，SECM 自动
    // 降级原链（不劣化现状）。
    if should_ensure_pawnio(st) {
        if let Err(e) = crate::driver_install::pawnio::ensure_pawnio_ready() {
            crate::debug_warn!(
                "lhm",
                "PawnIO 驱动未就绪（继续启动 sidecar，将按 available:false 降级处理）: {}",
                e
            );
        }
    }
    spawn_sidecar(st)
}

/// 判定是否需要在启动 sidecar 前触发 PawnIO 部署检查：未降级时才有意义
/// （已降级后不再 spawn，部署无消费者）。纯逻辑，可单测。
fn should_ensure_pawnio(st: &LhmState) -> bool {
    !st.degraded
}

/// 终止 SECM 直接 spawn 的 sidecar 子进程（kill + wait 回收）。
fn kill_child(st: &mut LhmState) {
    if let Some(mut child) = st.child.take() {
        st.uac_pending = false;
        let _ = child.kill();
        let _ = child.wait();
    }
}

// ============================================================================
// 对外接口
// ============================================================================

/// 初始化：定位并缓存 sidecar 可执行文件路径（lib.rs setup 时调用一次）。
pub fn init(app: Option<&tauri::AppHandle>) {
    let exe = resolve_lhm_exe(app);
    match &exe {
        Some(p) => crate::debug_info!("lhm", "sidecar 可执行文件: {}", p.display()),
        None => crate::debug_warn!(
            "lhm",
            "未找到 LhmSidecar.exe（资源目录缺失？LHM 温度源不可用，走原降级链）"
        ),
    }
    let _ = EXE_PATH.set(exe);
}

/// 构造 CPU 温度诊断串（与既有 `lhm_cpu_temp` diag 语义一致，供快照与薄封装共用）。
fn cpu_temp_diag(resp: &LhmSensorResponse) -> String {
    match resp.cpu.package_temp_c {
        Some(t) if t > 0.0 => {
            if let Some(pw) = resp.cpu.power_w {
                format!("lhm(pkg {:.1}℃, {:.1}W)", t, pw)
            } else {
                format!("lhm(pkg {:.1}℃)", t)
            }
        }
        _ => {
            let err = resp.error.as_deref().unwrap_or("无有效温度");
            format!("lhm(无有效温度:{})", err)
        }
    }
}

/// 唯一 HTTP 轮询入口：取全量快照（CPU/GPU/主板/内存），供 sensor.rs 每采集
/// 周期调用 1 次并共享结果（避免 CPU/GPU/主板各 1 次 HTTP 的 3 倍开销）。
///
/// 返回 (快照, 诊断串)。进程生命周期（降级/退避/重启/PawnIO 前置）与既有
/// `lhm_cpu_temp` 完全共用同一状态机，零破坏复用；成功且 available 时缓存
/// `last_snapshot` 供 `lhm_cpu_temp()` 薄封装复用（不重复 spawn/轮询）。
pub fn lhm_snapshot() -> (Option<LhmSensorResponse>, String) {
    let mut st = state().lock().unwrap_or_else(|p| p.into_inner());

    // 已降级：直接返回 None，不再尝试启动
    if st.degraded {
        return (None, "lhm(已降级:此前确定性失败)".to_string());
    }

    // 确保服务可用（启动 / 提权接管 / 崩溃重启 / UAC 等待）
    if let Err(e) = ensure_running(&mut st) {
        return match e {
            LhmStartError::ExeMissing => {
                st.degraded = true;
                crate::debug_warn!("lhm", "sidecar exe 缺失，降级原链");
                (None, "lhm(exe 缺失)".to_string())
            }
            LhmStartError::Retryable(msg) => {
                // 可重试失败（启动中/UAC 等待/崩溃）：不降级，退避控制重试频率
                (None, format!("lhm({})", msg))
            }
        };
    }

    // 轮询传感器（全量快照）
    match fetch_sensors() {
        Ok(resp) => {
            st.failures = 0;
            if resp.available {
                // 缓存全量快照（供 lhm_cpu_temp 薄封装复用，避免二次 HTTP）
                st.last_snapshot = Some((Instant::now(), resp.clone()));
                let diag = cpu_temp_diag(&resp);
                (Some(resp), diag)
            } else {
                // sidecar 存活但不可用（驱动/权限问题）：不 kill 不降级，
                // sidecar 内部周期重试，用户放行/提权后自动恢复
                let err = resp.error.as_deref().unwrap_or("未知原因");
                (None, format!("lhm(不可用:{})", err))
            }
        }
        Err(e) => {
            // 轮询失败 = sidecar 服务可能已崩溃/退出：复位 ready，
            // 使 ensure_running 走 E 分支自动重启（修复：ready 一旦为 true
            // 永不复位导致崩溃重启路径不可达、连续失败后直接降级的问题）
            st.ready = false;
            st.failures += 1;
            if st.failures >= LHM_MAX_FAILURES {
                crate::debug_warn!(
                    "lhm",
                    "连续 {} 次轮询失败，终止 sidecar 并降级: {}",
                    st.failures,
                    e
                );
                kill_child(&mut st);
                st.degraded = true;
            } else {
                crate::debug_warn!("lhm", "轮询失败({}/{}): {}", st.failures, LHM_MAX_FAILURES, e);
            }
            (None, format!("lhm(轮询失败:{})", e))
        }
    }
}

/// sensor.rs 调用入口：返回 (温度℃, 诊断串)。
///
/// 优先级：lhm 为温度链最高优先级；返回 (None, diag) 表示 LHM 不可用，
/// 由 sensor.rs 继续 winring0/acpi 链（不劣化现状）。
///
/// v1.17.0 起为薄封装：优先复用 `lhm_snapshot()` 的近时快照缓存（get_sensor_data
/// 每周期已先取全量快照，本函数直接命中缓存，零额外 HTTP）；缓存过期/缺失时
/// 转调 `lhm_snapshot()` 走完整进程生命周期与轮询。
pub fn lhm_cpu_temp() -> (Option<f32>, String) {
    // 快照缓存新鲜且可用 → 直接取 CPU 温度（避免重复 spawn/轮询）
    {
        let st = state().lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, resp)) = st.last_snapshot.as_ref() {
            if at.elapsed() < LHM_SNAPSHOT_TTL && resp.available {
                let diag = cpu_temp_diag(resp);
                let t = resp.cpu.package_temp_c.filter(|t| *t > 0.0);
                return (t, diag);
            }
        }
    }
    // 缓存缺失/过期：完整快照轮询（含降级/退避/重启/PawnIO 前置）
    let (snap, diag) = lhm_snapshot();
    match snap {
        Some(resp) if resp.available => {
            (resp.cpu.package_temp_c.filter(|t| *t > 0.0), diag)
        }
        _ => (None, diag),
    }
}

/// SECM 退出时清理：kill 直接 spawn 的子进程 + 按映像名兜底清理提权孤儿。
pub fn shutdown() {
    let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
    kill_child(&mut st);
    // 提权子进程（SECM 无句柄）按映像名兜底清理；失败（权限不足）则留待下次
    // 启动时端口探测复用，功能不受影响
    use std::os::windows::process::CommandExt;
    let _ = Command::new("taskkill")
        .args(["/IM", "LhmSidecar.exe", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    crate::debug_info!("lhm", "sidecar 清理完成（taskkill /IM LhmSidecar.exe）");
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试串行化互斥锁：本模块测试共享全局 `state()` 单例，并行运行时
    /// 相互污染（flaky，如退避态/降级态被并发测试改写）。触碰共享状态的
    /// 测试必须在测试体开头持有本锁，测试结束随 `_guard` 析构自动释放
    /// （自实现串行化，零依赖，语义与项目既有 Mutex 模式一致）。
    static TEST_STATE_LOCK: Mutex<()> = Mutex::new(());

    /// 重置全局状态（测试间隔离）。
    fn reset_state() {
        let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
        st.child = None;
        st.started = false;
        st.degraded = false;
        st.ready = false;
        st.failures = 0;
        st.restarts = 0;
        st.last_spawn = None;
        st.uac_pending = false;
        st.last_snapshot = None;
    }

    #[test]
    fn test_parse_sensor_json() {
        // 契约 §5.1 正常响应：snake_case 字段映射 + Option 解析（v1 字段）
        let json = r#"{"available":true,"error":null,"cpu":{"package_temp_c":45.2,"core_temps_c":[40.1,42.3,41.5,43.0],"power_w":38.4,"fan_rpm":812.0,"voltage_v":1.19}}"#;
        let resp: LhmSensorResponse = serde_json::from_str(json).expect("契约 JSON 应可解析");
        assert!(resp.available);
        assert!(resp.error.is_none());
        assert_eq!(resp.contract_version, None, "v1 响应无 contract_version 字段");
        assert_eq!(resp.cpu.package_temp_c, Some(45.2));
        assert_eq!(resp.cpu.core_temps_c.len(), 4);
        assert_eq!(resp.cpu.core_temps_c[1], 42.3);
        assert_eq!(resp.cpu.power_w, Some(38.4));
        assert_eq!(resp.cpu.fan_rpm, Some(812.0));
        assert_eq!(resp.cpu.voltage_v, Some(1.19));
        assert!(resp.gpu.is_empty(), "v1 响应 gpu 应为空数组");
        assert!(resp.motherboard.is_none());
        assert!(resp.memory.is_none());
    }

    #[test]
    fn test_parse_v2_full_json() {
        // v2.0 契约完整响应：contract_version + gpu[] + motherboard + memory 全字段解析
        let json = r#"{
            "available": true, "error": null, "contract_version": 2,
            "cpu": { "package_temp_c": 45.2, "core_temps_c": [40.1,42.3], "power_w": 38.4, "fan_rpm": 812.0, "voltage_v": 1.19 },
            "gpu": [
                { "name": "NVIDIA GeForce GTX 1660 Ti", "temperature_c": 52.3,
                  "core_clock_mhz": 1500.0, "power_w": 85.2, "load_percent": 45.0,
                  "memory_used_bytes": 1073741824, "memory_total_bytes": 6442450944, "fan_rpm": 1200.0 }
            ],
            "motherboard": {
                "name": "HUANANZHI X99",
                "sensors": [
                    { "name": "System", "type": "temperature", "value": 34.2 },
                    { "name": "Fan #1",  "type": "fan",         "value": 812.0 },
                    { "name": "+12V",    "type": "voltage",     "value": 12.1 }
                ]
            },
            "memory": { "name": "DDR4-2400 2x8GB", "total_bytes": 17179869184, "used_bytes": 8589934592 }
        }"#;
        let resp: LhmSensorResponse = serde_json::from_str(json).expect("v2 契约 JSON 应可解析");
        assert!(resp.available);
        assert_eq!(resp.contract_version, Some(2));
        // GPU
        assert_eq!(resp.gpu.len(), 1);
        let g = &resp.gpu[0];
        assert_eq!(g.name, "NVIDIA GeForce GTX 1660 Ti");
        assert_eq!(g.temperature_c, Some(52.3));
        assert_eq!(g.core_clock_mhz, Some(1500.0));
        assert_eq!(g.power_w, Some(85.2));
        assert_eq!(g.load_percent, Some(45.0));
        assert_eq!(g.memory_used_bytes, Some(1073741824));
        assert_eq!(g.memory_total_bytes, Some(6442450944));
        assert_eq!(g.fan_rpm, Some(1200.0));
        // 主板：type 字段 rename 映射到 kind
        let mb = resp.motherboard.as_ref().expect("motherboard 应存在");
        assert_eq!(mb.name.as_deref(), Some("HUANANZHI X99"));
        assert_eq!(mb.sensors.len(), 3);
        assert_eq!(mb.sensors[0].kind, "temperature");
        assert_eq!(mb.sensors[1].kind, "fan");
        assert_eq!(mb.sensors[2].kind, "voltage");
        assert_eq!(mb.sensors[2].value, 12.1);
        // 内存
        let mem = resp.memory.as_ref().expect("memory 应存在");
        assert_eq!(mem.name.as_deref(), Some("DDR4-2400 2x8GB"));
        assert_eq!(mem.total_bytes, Some(17179869184));
        assert_eq!(mem.used_bytes, Some(8589934592));
    }

    #[test]
    fn test_parse_v2_edge_cases() {
        // 边界：motherboard:null / gpu 空数组 / gpu 元素全 null 字段 / memory:null
        let json = r#"{
            "available": true, "error": null, "contract_version": 2,
            "cpu": { "package_temp_c": 45.2, "core_temps_c": [], "power_w": null, "fan_rpm": null, "voltage_v": null },
            "gpu": [
                { "name": "AMD Radeon RX 580", "temperature_c": null, "core_clock_mhz": null,
                  "power_w": null, "load_percent": null, "memory_used_bytes": null,
                  "memory_total_bytes": null, "fan_rpm": null }
            ],
            "motherboard": null,
            "memory": null
        }"#;
        let resp: LhmSensorResponse = serde_json::from_str(json).expect("边界 JSON 应可解析");
        assert_eq!(resp.contract_version, Some(2));
        assert_eq!(resp.gpu.len(), 1);
        let g = &resp.gpu[0];
        // 全 null 字段 → Option None（前端 "—" 兜底）
        assert!(g.temperature_c.is_none());
        assert!(g.power_w.is_none());
        assert!(g.load_percent.is_none());
        assert!(g.memory_used_bytes.is_none());
        assert_eq!(g.name, "AMD Radeon RX 580");
        assert!(resp.motherboard.is_none(), "motherboard:null 应解析为 None");
        assert!(resp.memory.is_none(), "memory:null 应解析为 None");

        // gpu 空数组 + 缺字段（仅顶层 available）
        let json2 = r#"{"available":true,"gpu":[]}"#;
        let resp2: LhmSensorResponse = serde_json::from_str(json2).expect("缺字段 JSON 应可解析");
        assert!(resp2.gpu.is_empty());
        assert!(resp2.motherboard.is_none());
        assert_eq!(resp2.cpu.package_temp_c, None);

        // 主板 sensors 元素缺 type/value 字段（serde(default) 兜底）
        let json3 = r#"{"available":true,"motherboard":{"sensors":[{"name":"System"}]}}"#;
        let resp3: LhmSensorResponse = serde_json::from_str(json3).expect("缺字段传感器应可解析");
        let mb = resp3.motherboard.expect("motherboard 应存在");
        assert_eq!(mb.sensors.len(), 1);
        assert_eq!(mb.sensors[0].name, "System");
        assert!(mb.sensors[0].kind.is_empty(), "缺 type 时 kind 应为空串");
        assert_eq!(mb.sensors[0].value, 0.0);
    }

    #[test]
    fn test_parse_v1_response_backward_compat() {
        // 旧 v1 sidecar 响应（无 contract_version/gpu/motherboard/memory 顶层字段）：
        // serde(default) 兜底，新 SECM 解析不 panic（向后兼容）
        let json = r#"{"available":true,"error":null,"cpu":{"package_temp_c":51.7,"core_temps_c":[50.2,51.1],"power_w":22.3}}"#;
        let resp: LhmSensorResponse = serde_json::from_str(json).expect("v1 响应应可解析");
        assert!(resp.available);
        assert_eq!(resp.contract_version, None);
        assert_eq!(resp.cpu.package_temp_c, Some(51.7));
        assert!(resp.gpu.is_empty());
        assert!(resp.motherboard.is_none());
        assert!(resp.memory.is_none());
    }

    #[test]
    fn test_parse_unavailable_json() {
        // 契约失败响应：available:false + error 透传（驱动/权限不可用）
        let json = r#"{"available":false,"error":"PawnIO 设备不可访问（CreateFileW \\\\.\\PawnIO 失败：驱动未安装）","cpu":{"package_temp_c":null,"core_temps_c":[],"power_w":null,"fan_rpm":null,"voltage_v":null}}"#;
        let resp: LhmSensorResponse = serde_json::from_str(json).expect("契约 JSON 应可解析");
        assert!(!resp.available);
        let err = resp.error.as_deref().expect("error 字段应存在");
        assert!(err.contains("PawnIO"), "错误应含 PawnIO 字样: {}", err);
        assert_eq!(resp.cpu.package_temp_c, None);
        assert!(resp.cpu.core_temps_c.is_empty());
    }

    #[test]
    fn test_parse_missing_fields_defaults() {
        // 字段缺失时 serde(default) 兜底，不 panic
        let json = r#"{"available":true}"#;
        let resp: LhmSensorResponse = serde_json::from_str(json).expect("缺字段 JSON 应可解析");
        assert!(resp.available);
        assert!(resp.error.is_none());
        assert_eq!(resp.cpu.package_temp_c, None);
    }

    #[test]
    fn test_degraded_short_circuit() {
        // 降级态直接返回 None，不启动进程（测试确定性，不依赖真实 sidecar）
        let _guard = TEST_STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        {
            let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
            st.degraded = true;
        }
        let (t, diag) = lhm_cpu_temp();
        assert!(t.is_none(), "降级后温度应为 None");
        assert!(diag.contains("降级"), "诊断串应说明降级: {}", diag);
        reset_state();
    }

    #[test]
    fn test_invalid_temp_filtered() {
        // package_temp_c <= 0 时视为无有效温度（LHM 静默 0 值防护）
        let _guard = TEST_STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
        st.ready = true; // 模拟服务可用
        drop(st);
        // 无法直接注入 fetch_sensors 返回值；此处验证契约解析层：
        // 0 值 package 在解析层保留，过滤逻辑在 lhm_cpu_temp 分支（t > 0.0 判断）
        let json = r#"{"available":true,"error":null,"cpu":{"package_temp_c":0.0,"core_temps_c":[],"power_w":null}}"#;
        let resp: LhmSensorResponse = serde_json::from_str(json).expect("解析成功");
        assert_eq!(resp.cpu.package_temp_c, Some(0.0));
        // 若直接以该响应走 lhm_cpu_temp 的过滤分支，0 值应落入「无有效温度」分支
        // （该分支由 Some(t) if t > 0.0 守卫，0.0 不匹配 → 走 _ 分支返回 None）
    }

    #[test]
    fn test_parse_invalid_json_errors() {
        // 场景 C（非法 JSON）：serde 解析失败应返回 Err（Result 传播，无 panic）
        let bad = r#"{"available":true,"cpu":{"package_temp_c":"oops"}}"#;
        let r1: Result<LhmSensorResponse, _> = serde_json::from_str(bad);
        assert!(r1.is_err(), "字段类型错误应解析失败: {:?}", r1.err());

        let r2: Result<LhmSensorResponse, _> = serde_json::from_str(r#"{"not json""#);
        assert!(r2.is_err(), "语法错误 JSON 应解析失败");

        let r3: Result<LhmSensorResponse, _> = serde_json::from_str("");
        assert!(r3.is_err(), "空 body 应解析失败");
    }

    #[test]
    fn test_parse_top_level_null_errors() {
        // 顶层 null：serde(default) 仅作用于字段级，顶层 null 应报错而非静默默认
        let r: Result<LhmSensorResponse, _> = serde_json::from_str("null");
        assert!(r.is_err(), "顶层 null 不应被当作默认结构体: {:?}", r.err());
    }

    #[test]
    fn test_parse_unavailable_missing_error() {
        // 契约边界：available=false 但 error 字段缺失 → error 应为 None
        // （lhm_cpu_temp 分支以 unwrap_or("未知原因") 兜底，不 panic）
        let json = r#"{"available":false,"cpu":{"package_temp_c":null,"core_temps_c":[],"power_w":null}}"#;
        let resp: LhmSensorResponse = serde_json::from_str(json).expect("解析成功");
        assert!(!resp.available);
        assert!(resp.error.is_none(), "error 缺失时应为 None");
    }

    #[test]
    fn test_should_ensure_pawnio() {
        // 未降级 → 启动 sidecar 前应触发 PawnIO 部署检查；
        // 已降级 → 不再 spawn，部署无消费者（不应触发）
        let _guard = TEST_STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
        st.degraded = false;
        assert!(should_ensure_pawnio(&st), "未降级时应触发 PawnIO 检查");
        st.degraded = true;
        assert!(!should_ensure_pawnio(&st), "已降级后不应触发 PawnIO 检查");
        drop(st);
        reset_state();
    }

    #[test]
    fn test_backoff_prevents_spawn() {
        // 场景 A（sidecar 不可用）的快速失败路径：spawn 退避期内不应重复
        // spawn / 探测端口（避免每轮 500ms 阻塞），直接返回 Retryable 诊断。
        // 该分支在 health_check 之前命中，因此本测试不依赖网络与真实进程。
        let _guard = TEST_STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        {
            let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
            st.last_spawn = Some(Instant::now());
        }
        let (t, diag) = lhm_cpu_temp();
        assert!(t.is_none(), "退避期内不应产生温度");
        assert!(
            diag.contains("退避"),
            "诊断串应说明退避重试: {}",
            diag
        );
        // 退避失败不降级：degraded 保持 false，退避结束后仍可重试启动
        let st = state().lock().unwrap_or_else(|p| p.into_inner());
        assert!(!st.degraded, "退避失败不应降级");
        drop(st);
        reset_state();
    }

    #[test]
    fn test_snapshot_degraded_short_circuit() {
        // lhm_snapshot 降级态直接返回 (None, 降级诊断)，不启动进程不轮询
        let _guard = TEST_STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        {
            let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
            st.degraded = true;
        }
        let (snap, diag) = lhm_snapshot();
        assert!(snap.is_none(), "降级后快照应为 None");
        assert!(diag.contains("降级"), "诊断串应说明降级: {}", diag);
        reset_state();
    }

    #[test]
    fn test_snapshot_does_not_cache_when_degraded() {
        // 降级路径不应写入 last_snapshot 缓存（保证后续恢复逻辑不被脏数据干扰）
        let _guard = TEST_STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        {
            let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
            st.degraded = true;
        }
        let _ = lhm_snapshot();
        let st = state().lock().unwrap_or_else(|p| p.into_inner());
        assert!(st.last_snapshot.is_none(), "降级路径不应写缓存");
        drop(st);
        reset_state();
    }

    #[test]
    fn test_lhm_cpu_temp_reuses_fresh_snapshot() {
        // lhm_cpu_temp 薄封装：命中新鲜 last_snapshot 缓存时直接返回 CPU 温度，
        // 不依赖真实 sidecar / 网络（缓存注入方式验证「薄封装优先读缓存」契约）
        let _guard = TEST_STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        {
            let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
            let resp = LhmSensorResponse {
                available: true,
                error: None,
                contract_version: Some(2),
                cpu: LhmCpuData {
                    package_temp_c: Some(61.5),
                    power_w: Some(42.7),
                    ..Default::default()
                },
                ..Default::default()
            };
            st.last_snapshot = Some((std::time::Instant::now(), resp));
        }
        let (t, diag) = lhm_cpu_temp();
        assert_eq!(t, Some(61.5), "应命中缓存返回温度");
        assert!(diag.contains("61.5") && diag.contains("42.7"), "diag 应含温度与功耗: {}", diag);
        // 缓存路径不改动状态机（不触发 spawn/降级）
        let st = state().lock().unwrap_or_else(|p| p.into_inner());
        assert!(!st.degraded, "缓存命中不应降级");
        assert!(st.last_snapshot.is_some(), "缓存应保留");
        drop(st);
        reset_state();
    }

    #[test]
    fn test_lhm_cpu_temp_filters_nonpositive_cache() {
        // 缓存中 package_temp_c <= 0 时薄封装返回 None（过滤无效温度），diag 透出原因
        let _guard = TEST_STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        {
            let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
            let resp = LhmSensorResponse {
                available: true,
                error: Some("传感器无有效温度".to_string()),
                cpu: LhmCpuData {
                    package_temp_c: Some(0.0),
                    ..Default::default()
                },
                ..Default::default()
            };
            st.last_snapshot = Some((std::time::Instant::now(), resp));
        }
        let (t, diag) = lhm_cpu_temp();
        assert!(t.is_none(), "非正温度应过滤为 None");
        assert!(diag.contains("无有效温度"), "diag 应说明无有效温度: {}", diag);
        reset_state();
    }

    #[test]
    fn test_uac_pending_probe_only() {
        // UAC 竞态修复核心路径：进程存活 + uac_pending 时 ensure_running 不 kill
        // 不重复 spawn，只探测端口（本测试端口无服务 → 返回 Retryable 而非降级）
        let _guard = TEST_STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        // 无法构造真实存活子进程，改为验证状态机契约：uac_pending 标记下
        // 退避返回 Retryable（不降级），且 child 为空时 uac_pending 自动清除
        {
            let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
            st.uac_pending = true;
            st.last_spawn = Some(std::time::Instant::now());
        }
        // 退避期内：返回 Retryable，不 spawn（uac_pending 保持，等待退避结束探测）
        let (snap, diag) = lhm_snapshot();
        assert!(snap.is_none());
        assert!(diag.contains("退避"), "退避期内诊断应说明退避: {}", diag);
        let st = state().lock().unwrap_or_else(|p| p.into_inner());
        assert!(!st.degraded, "UAC 等待/退避不应降级");
        assert!(st.uac_pending, "退避期不应清除 uac_pending");
        drop(st);
        reset_state();
    }

    #[test]
    fn test_uac_pending_without_child_probes_port() {
        // child 句柄丢失（进程已退出/句柄失效）+ uac_pending 残留 + 退避已过：
        // ensure_running 应走 C' 端口探测（本测试环境 45980 无服务 → 探测失败），
        // 返回 Retryable 而非 Ok/降级，且不 panic（探测节流时间戳被重置）
        let _guard = TEST_STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
        st.child = None;
        st.uac_pending = true;
        st.ready = false;
        st.last_spawn = None;
        let r = ensure_running(&mut st);
        assert!(r.is_err(), "无服务时 uac_pending 探测应失败");
        // 探测失败后节流时间戳被重置（下个窗口再探测）
        assert!(st.last_spawn.is_some(), "探测失败应重置节流时间戳");
        assert!(st.uac_pending, "探测失败不应清除 uac_pending");
        drop(st);
        reset_state();
    }

    #[test]
    fn test_cpu_temp_diag_variants() {
        // cpu_temp_diag 辅助：有温度+功耗 / 仅温度 / 无有效温度 三种 diag 形态
        let with_power = LhmSensorResponse {
            available: true,
            cpu: LhmCpuData {
                package_temp_c: Some(45.2),
                power_w: Some(38.4),
                ..Default::default()
            },
            ..Default::default()
        };
        let d1 = cpu_temp_diag(&with_power);
        assert!(d1.contains("45.2") && d1.contains("38.4"), "应含温度与功耗: {}", d1);

        let no_power = LhmSensorResponse {
            available: true,
            cpu: LhmCpuData {
                package_temp_c: Some(45.2),
                ..Default::default()
            },
            ..Default::default()
        };
        let d2 = cpu_temp_diag(&no_power);
        assert!(d2.contains("45.2") && !d2.contains("W"), "无功耗时 diag 不应含 W: {}", d2);

        let invalid = LhmSensorResponse {
            available: true,
            error: Some("传感器未就绪".to_string()),
            cpu: LhmCpuData {
                package_temp_c: Some(0.0),
                ..Default::default()
            },
            ..Default::default()
        };
        let d3 = cpu_temp_diag(&invalid);
        assert!(d3.contains("无有效温度") && d3.contains("传感器未就绪"), "应透出错误原因: {}", d3);
    }
}
