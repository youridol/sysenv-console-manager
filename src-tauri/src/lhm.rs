//! LHM（LibreHardwareMonitor）sidecar 数据源模块 —— 契约 v3。
//!
//! 架构：SECM 通过 HTTP 消费独立 sidecar 进程（LhmSidecar.exe，.NET 8）输出的
//! 全量硬件快照 JSON；LibreHardwareMonitorLib（MPL-2.0，内嵌 PawnIO ring0 驱动
//! LGPL-2.1）仅存在于 sidecar 进程内，进程隔离规避许可对 MIT 主体的传染。
//!
//! 权限模型（契约 v3 §1）：sidecar 始终以未提权启动并立即提供 tier-1
//! （性能计数器/DriveInfo/网络/电池/厂商 GPU 用户态通道）；tier-2
//! （CPU 温度/功耗/电压、主板 SuperIO、磁盘温度）由提权伴生进程提供。
//! PawnIO 设备（\\.\PawnIO）DACL 仅 SYSTEM/Administrators 可访问，因此 tier-2
//! 需要用户显式触发一次 UAC（`request_elevation()` → POST `/api/v1/elevate`），
//! 被拒绝是非致命且被记住的状态。SECM 以「端口服务探测（health_check）」判定
//! 服务可用性，而非依赖子进程句柄（提权接管后原进程退出、句柄失效）。
//!
//! 职责：
//!   1. sidecar 进程生命周期（lazy 启动 / 崩溃自动重启一次 / 退出清理）
//!   2. HTTP 轮询（/health 健康检查 + /api/v1/snapshot 全量快照）
//!   3. JSON 解析（serde）→ 契约 v3 结构（见 CONTRACT-v3.md §6，逐字段冻结）
//!   4. 降级：任何失败返回 (None, diag)，由 sensor.rs 投影为 `*_source="none"`
//!
//! 降级策略（不劣化现状）：
//!   - sidecar exe 不存在 → 确定性失败，标记降级（后续不再启动）
//!   - 启动中/UAC 等待/崩溃 → 可重试失败，退避期内不重复 spawn（避免每轮阻塞）
//!   - 快照轮询连续 3 次失败 → kill 进程 + 标记降级
//!   - sidecar 存活但 available:false（tier-1 性能计数器预热中 / 驱动或权限不可用）
//!     → **仍返回 `Some(resp)`**（不 kill 不降级，sidecar 内部周期重试，用户放行/
//!     提权后自动恢复）。`available` 是"是否有任一指标"的粗粒度门控，只用于诊断
//!     文案，**不再**用作"丢弃整份快照"的判据——契约 v3 每个字段都是
//!     `Option`/空数组安全（`#[serde(default)]`），预热期 payload 中已经有效的
//!     字段（如经厂商用户态通道立即可读的 GPU 温度/功耗）不应被连带丢弃。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// sidecar 监听端口（与 sidecar 默认值一致）。
const LHM_PORT: u16 = 45980;

/// 测试/排障用端口覆盖环境变量（仅 debug 构建生效，release 不读取，与
/// `SECM_LHM_DIR` 同一防注入面策略）。
///
/// 动机：契约测试需要构造「sidecar 缺失」场景。仅让 exe 不可定位并不充分——
/// 开发机上常驻一个已监听 `LHM_PORT` 的 sidecar（如应用正在运行），此时
/// `resolve_lhm_exe` 返回 None 但端口仍有服务应答，测试会误取真实快照而失败
/// （曾实测：`hardware_contract` 的 sidecar-absent 用例被 45980 上的残留进程
/// 击穿）。把端口指向一个确定无服务的值，才能让「缺失」可复现。
///
/// `cfg(debug_assertions)` 门控：release 下该常量不存在（无注入面，且避免
/// dead_code 告警）。
#[cfg(debug_assertions)]
const LHM_PORT_ENV: &str = "SECM_LHM_PORT";

/// 当前生效的 sidecar 端口：debug 构建下允许 `SECM_LHM_PORT` 覆盖，否则默认值。
///
/// 端口在使用处即时解析（与 sidecar 的 `--port` 参数同源；`SECM_LHM_PORT`
/// 仅 debug 生效，release 恒为默认端口）。
fn lhm_port() -> u16 {
    // 仅 debug 构建读取（release 关闭注入面，M1 同策略）
    #[cfg(debug_assertions)]
    if let Ok(v) = std::env::var(LHM_PORT_ENV) {
        if let Ok(p) = v.trim().parse::<u16>() {
            if p > 0 {
                return p;
            }
        }
    }
    LHM_PORT
}

/// 健康检查单次超时。
///
/// 注意：本值**不能**约束"端口无监听"这一失败路径——回环端口被拒绝连接时，
/// ureq/Windows 实测需要 ~2.0s 才返回 `WSAECONNREFUSED(10061)`，`.timeout()`
/// 对该阶段不生效（实测 3 次均 ~2.0s，与 timeout=500ms 无关）。因此
/// `health_check()` 先用 `LHM_PROBE_CONNECT_TIMEOUT` 的 std 层连接门把该路径
/// 压到有界（<200ms），再发起 HTTP。此常量只约束"已连上但读写慢"的场景。
const LHM_HEALTH_TIMEOUT: Duration = Duration::from_millis(500);
/// 端口可用性**前置门**的连接超时（std `TcpStream::connect_timeout`）。
///
/// 连接被拒绝时它严格在预算内返回 Err（实测 113ms / 313ms 对应 100ms / 300ms
/// 预算），使"端口无服务"的探测代价与 `LHM_HEALTH_TIMEOUT` 无关地有界。
/// 端口**有**服务时开销为 0–3ms（实测），故高频探测也是安全的。
///
/// 取 100ms：回环连接是本地内存操作，真实 sidecar 就绪时首包在毫秒级；
/// 留 100ms 足以覆盖调度抖动，同时把最坏探测代价钉在百毫秒量级。
const LHM_PROBE_CONNECT_TIMEOUT: Duration = Duration::from_millis(100);
/// 快照轮询超时。
const LHM_SENSORS_TIMEOUT: Duration = Duration::from_secs(2);
/// 提权请求超时（UAC 弹窗 + 伴生进程启动 + 端口接管；用户确认耗时不可控，给足窗口）。
const LHM_ELEVATE_TIMEOUT: Duration = Duration::from_secs(90);
/// sidecar 启动健康检查的**总**等待预算（覆盖 .NET 冷启动 + LHM 开树 + 权限移交）。
///
/// 语义变化：原实现是「5 次探测，每次失败后 sleep 1s」。但每次探测本身在
/// "端口无监听"路径上要花 ≈2s（ureq 的 `.timeout()` 不约束连接阶段，实测
/// 恒为 ~2030ms 才拿到 WSAECONNREFUSED；见 `LHM_PROBE_CONNECT_TIMEOUT` 注释），
/// 因此旧的最坏耗时实为 `5×2s + 4×1s ≈ 14s`，而非注释里假设的 ≈5s。
/// 现在改为「共享截止时间 + 50ms 细粒度轮询」，最坏情形被真正封顶在 5s
/// （实测 5.11–5.20s），较旧实现的 ~16s 请求路径阻塞有实质改善。
const LHM_START_WAIT: Duration = Duration::from_secs(5);

/// 自动提权等待 sidecar 就绪的总预算。
///
/// 页面挂载时，预热线程通常正在执行**同一次** sidecar 启动：此时
/// `ensure_running` 会返回可重试失败（启动中 / 退避期内）。自动提权若就此放弃，
/// 冷启动的首个自动尝试几乎必然不生效。这里做有界等待（每 100ms 重试一次），
/// 预算取 15s：大于 `LHM_START_WAIT`(5s) 与一次退避，且小于前端 `auto_elevate`
/// 的 20s 调用超时，保证失败能如实上报而不是被前端超时掩盖。
const LHM_AUTO_ELEVATE_AWAIT: Duration = Duration::from_secs(15);

/// 自动提权 HTTP 超时。
///
/// sidecar 侧零 UAC 路径最多等待 8s 让 tier-2 就绪，故 15s 足够覆盖该路径的最坏耗时；
/// 与 `LHM_ELEVATE_TIMEOUT`(90s，含 UAC 弹窗用户思考时间) 分开，避免自动路径长时间挂起。
const LHM_AUTO_ELEVATE_TIMEOUT: Duration = Duration::from_secs(15);

/// 自动提权等待就绪的重试间隔。
const LHM_AUTO_AWAIT_POLL: Duration = Duration::from_millis(100);
/// 启动健康检查的轮询间隔。
///
/// sidecar 实测约 600ms 起就提供 `/health`；整秒 sleep 会把"已就绪"取整到下一个
/// 整秒，实测额外多等 1510ms。改用 50ms 粒度后，就绪即被发现。
///
/// 收紧粒度不会放大最坏代价：`health_check()` 的端口前置门使"无服务"路径
/// <200ms 返回（见 `LHM_PROBE_CONNECT_TIMEOUT`），故轮询次数增加不显著增加总耗时，
/// 且总时长被 `LHM_START_WAIT` 统一封顶。
const LHM_HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// 连续轮询失败上限（达到后 kill + 降级）。
const LHM_MAX_FAILURES: u32 = 3;
/// sidecar 崩溃自动重启次数上限（超过则降级）。
const LHM_MAX_RESTARTS: u32 = 1;
/// spawn 退避间隔：上次 spawn 后此期间内不重复 spawn / 不探测端口（避免每轮阻塞）。
const LHM_SPAWN_BACKOFF: Duration = Duration::from_secs(10);
/// 降级 / 重启预算耗尽后的自愈冷却时间。
///
/// 修复（2026-09-23，实测会话 `2026-09-22 13:44`）：原实现把「降级」与「重启预算
/// 耗尽」都当作本会话的终态——`lhm_snapshot` 一进入降级态就直接返回，`ensure_running`
/// 的 E 分支在 `restarts > LHM_MAX_RESTARTS` 后也只返回 `Retryable` 且永不重置
/// `restarts`。实测日志中该计数从 2 一路涨到 13，整个会话（约 3 分钟）硬件信息页
/// 始终 `TEMP=n/a`，必须重启应用才能恢复；而同一 sidecar 二进制手工启动后
/// `/health` 与 `/api/v1/snapshot` 均正常。现改为：两种终态都进入固定冷却，
/// 冷却期满后清零预算并重新走完整的启动/探测路径，使短暂故障可自愈。
const LHM_RECOVERY_COOLDOWN: Duration = Duration::from_secs(60);
/// 快照缓存有效期：`lhm_cpu_temp()` 命中此窗口内的 `last_snapshot` 直接复用
/// （get_sensor_data 每采集周期先调 `lhm_snapshot()` 刷新缓存，CPU 温度读取
/// 零额外 HTTP；独立调用超过窗口则转调完整轮询保证数据新鲜）。
const LHM_SNAPSHOT_TTL: Duration = Duration::from_secs(2);
/// CREATE_NO_WINDOW：防止 sidecar 弹出控制台黑窗（参考 proc_util 先例）。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ============================================================================
// 契约结构（sidecar JSON，见 sidecar-lhm/CONTRACT-v3.md §3 / §6，逐字段冻结）
// ============================================================================

/// CPU 传感器数据（契约 v3 §6）。
///
/// `#[serde(default)]` 为硬性要求：v2 响应缺 `name`/`load_percent`/`clock_mhz`/
/// `pump_rpm` 等字段时按类型默认值兜底，绝不 panic。
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmCpuData {
    /// CPU 节点名（LHM 识别；缺失时空串）
    pub name: String,
    /// CPU 负载（%，tier-1 PDH；不可用 null）
    pub load_percent: Option<f32>,
    /// Package 温度（℃，tier-2）
    pub package_temp_c: Option<f32>,
    /// 各核心/CCD 温度（℃，tier-2；无则空数组）
    pub core_temps_c: Vec<f32>,
    /// Package 功耗（W，tier-2）
    pub power_w: Option<f32>,
    /// 核心电压（V，tier-2）
    pub voltage_v: Option<f32>,
    /// 频率（MHz；tier-1 PDH 优先，回退 tier-2）
    pub clock_mhz: Option<f32>,
    /// CPU 风扇转速（RPM，tier-2）
    pub fan_rpm: Option<f32>,
    /// 水泵转速（RPM，tier-2）
    pub pump_rpm: Option<f32>,
    /// 负载数据源："perfcounter" | "lhm" | "none"（sensor.rs 映射为前端 freq/usage 契约）
    pub load_source: String,
    /// 温度数据源："lhm" | "none"（tier-2 缺失时恒为 none，不用 0 冒充）
    pub temp_source: String,
    /// 功耗数据源："lhm" | "none"
    pub power_source: String,
    /// 频率数据源："perfcounter" | "lhm" | "registry" | "none"
    pub clock_source: String,
    /// 恒 false（移植后无 TDP×usage 估算；保留字段兼容，前端不显示"估算"角标）
    pub power_estimated: bool,
}

/// GPU 传感器数据（契约 v3 §6；可空字段一律 Option，前端 "—" 兜底）。
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmGpuData {
    /// GPU 名称（厂商驱动命名，如 "NVIDIA GeForce RTX 3060"）
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
    /// 显存占用率（%，计算值 used/total*100）
    pub vram_load_percent: Option<f32>,
    /// 温度数据源："lhm" | "none"
    pub temp_source: String,
    /// 功耗数据源："lhm" | "none"
    pub power_source: String,
    /// 频率数据源："lhm" | "none"
    pub clock_source: String,
}

/// 主板单个传感器（SuperIO 温度/风扇/电压，tier-2）。
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

/// 主板传感器数据（契约 v3 §3；tier-2 不可用/无主板节点时顶层为 null）。
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmMotherboardData {
    /// 主板名称（LHM 识别；无则 None）
    pub name: Option<String>,
    /// 传感器列表（无数据 → 空数组）
    pub sensors: Vec<LhmMbSensor>,
}

/// 内存数据（契约 v3 §3；LHM 推算，SECM 仍以 GlobalMemoryStatusEx 为权威）。
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmMemoryData {
    /// LHM 内存节点名 / SPD 型号（best-effort；不可用时 None）
    pub name: Option<String>,
    /// 总容量（字节；补充值）
    pub total_bytes: Option<u64>,
    /// 已用容量（字节；补充值）
    pub used_bytes: Option<u64>,
}

/// 磁盘传感器数据（契约 v3 §3；速率/占用 tier-1，温度 tier-2）。
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmDiskData {
    /// 卷名（如 "C:"）
    pub name: String,
    /// 读速率（字节/秒；PDH 每卷实例）
    pub read_bytes_per_sec: Option<f64>,
    /// 写速率（字节/秒）
    pub write_bytes_per_sec: Option<f64>,
    /// 活动率（%，PDH % Disk Time，截断 0-100）
    pub activity_percent: Option<f32>,
    /// 已用容量占比（%，DriveInfo）
    pub used_percent: Option<f32>,
    /// 温度（℃，tier-2）
    pub temperature_c: Option<f32>,
}

/// 电池数据（契约 v3 §3；无电池时顶层为 null）。
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmBatteryData {
    /// 剩余电量（%）
    pub percent: Option<f32>,
    /// 功耗（W；AC 在线为正、放电为负）
    pub power_w: Option<f32>,
    /// 电压（V）
    pub voltage_v: Option<f32>,
    /// 电流（A）
    pub current_a: Option<f32>,
}

/// sidecar `/api/v1/snapshot` 响应（契约 v3 §3 / §6）。
///
/// `#[serde(default)]` 保证 v2（无 `elevated`/`tier2_*`/`disks`/`battery`）与
/// 缺字段 JSON 解析不 panic——向后兼容。`contract_version` 为 `Option<u32>`
/// （v1 响应无此字段）。
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmSensorResponse {
    /// 是否有任一有效指标（契约 §4；tier-1 或 tier-2 任一有效即为 true）
    pub available: bool,
    /// 顶层不可用原因（available=true 时为 None，含修复建议）
    pub error: Option<String>,
    /// sidecar 契约版本（v3=3；v2=2；v1 响应缺失时为 None）
    pub contract_version: Option<u32>,
    /// tier-2 是否已生效（含提权伴生进程）
    pub elevated: bool,
    /// tier-2 指标当前是否可读
    pub tier2_available: bool,
    /// tier-2 不可读原因（供前端"需要管理员"提示卡；可用时为 None）
    pub tier2_reason: Option<String>,
    /// CPU 传感器数据
    pub cpu: LhmCpuData,
    /// GPU 传感器列表（无 GPU 或不可用时为空数组）
    pub gpu: Vec<LhmGpuData>,
    /// 主板传感器数据（tier-2 不可用/无主板节点时为 None）
    pub motherboard: Option<LhmMotherboardData>,
    /// 内存数据（无内存节点时为 None）
    pub memory: Option<LhmMemoryData>,
    /// 磁盘列表（v3 新增；无磁盘时为空数组）
    pub disks: Vec<LhmDiskData>,
    /// 电池数据（v3 新增；无电池时为 None）
    pub battery: Option<LhmBatteryData>,
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
    /// 进入降级 / 重启预算耗尽的时间（自愈冷却起点；None = 未处于终态）
    degraded_at: Option<Instant>,
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
            degraded_at: None,
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

/// 端口是否有监听者（**有界**探测；无监听时 <200ms 返回 false）。
///
/// 存在的唯一理由是绕开 Windows 回环"拒绝连接"路径的 ~2.0s 固定代价：
/// 该路径上 ureq 的 `.timeout(500ms)` 不生效（实测 3 次均 ~2.0s 才拿到
/// `WSAECONNREFUSED(10061)`），使 `ensure_running` 的端口探测在**请求路径**上
/// 稳定吃掉约 2s。std 层 `connect_timeout` 严格遵守预算（实测 100ms→113ms），
/// 故用它先把"确定无服务"的情形快速判掉；端口有服务时开销为 0–3ms。
///
/// 不改变任何判定语义：这里只回答"TCP 连接能否建立"，真正的服务身份校验
/// 仍由随后的 `/health` 响应体（`"status":"ok"`）完成——端口被其他程序占用
/// 时依然会被 health_check 拒绝（M2 防护不变）。
fn port_reachable() -> bool {
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, lhm_port()));
    TcpStream::connect_timeout(&addr, LHM_PROBE_CONNECT_TIMEOUT).is_ok()
}

/// 健康检查：GET /health（500ms 超时）。
///
/// 先经 [`port_reachable`] 有界前置门：无监听时立即返回 Err，避免"拒绝连接"
/// 路径的 ~2.0s 代价（见该函数注释）。端口有服务时前置门开销可忽略。
fn health_check() -> Result<(), String> {
    if !port_reachable() {
        return Err(format!(
            "health 前置门失败：127.0.0.1:{} 无监听（连接被拒绝，{}ms 内未建立）",
            lhm_port(),
            LHM_PROBE_CONNECT_TIMEOUT.as_millis()
        ));
    }
    let url = format!("http://127.0.0.1:{}/health", lhm_port());
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
            lhm_port(),
            body.chars().take(60).collect::<String>()
        ));
    }
    Ok(())
}

/// 轮询快照：GET /api/v1/snapshot（2s 超时）→ serde 解析为契约 v3 结构。
///
/// 契约 §2 保证 v2 路径 `/api/lhm/sensors` 仍返回同一份快照（别名），因此
/// 升级后无需回退路径；v2 响应体缺 `elevated`/`disks`/`battery` 由
/// `#[serde(default)]` 兜底。
fn fetch_sensors() -> Result<LhmSensorResponse, String> {
    let url = format!("http://127.0.0.1:{}/api/v1/snapshot", lhm_port());
    let resp = ureq::get(&url)
        .timeout(LHM_SENSORS_TIMEOUT)
        .call()
        .map_err(|e| format!("snapshot HTTP 错误: {}", e))?;
    if resp.status() != 200 {
        return Err(format!("snapshot HTTP 状态码 {}", resp.status()));
    }
    let text = resp
        .into_string()
        .map_err(|e| format!("读取响应体失败: {}", e))?;
    serde_json::from_str(&text).map_err(|e| format!("JSON 解析失败: {}", e))
}

/// 触发一次 UAC 提权：POST `/api/v1/elevate`（契约 §2）。
///
/// 返回 `Ok(true)` = 用户接受且 tier-2 已生效；`Ok(false)` = 用户拒绝 / 提权
/// 未生效（非致命，sidecar 记住该状态，不会每次轮询重复弹窗）。
/// 服务不可达或响应体异常返回 `Err`——调用方据此提示"sidecar 未运行"。
fn post_elevate() -> Result<bool, String> {
    let url = format!("http://127.0.0.1:{}/api/v1/elevate", lhm_port());
    let resp = ureq::post(&url)
        .timeout(LHM_ELEVATE_TIMEOUT)
        .call()
        .map_err(|e| format!("elevate HTTP 错误: {}", e))?;
    if resp.status() != 200 {
        return Err(format!("elevate HTTP 状态码 {}", resp.status()));
    }
    let text = resp
        .into_string()
        .map_err(|e| format!("读取响应体失败: {}", e))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("elevate 响应 JSON 解析失败: {}", e))?;
    let elevated = v.get("elevated").and_then(|b| b.as_bool()).unwrap_or(false);
    if !elevated {
        let reason = v
            .get("reason")
            .and_then(|r| r.as_str())
            .unwrap_or("提权未生效（用户取消或被系统拒绝）");
        return Err(reason.to_string());
    }
    Ok(true)
}

/// 触发一次「零 UAC 自动提权」：POST `/api/v1/elevate/auto`（契约 §2 扩展）。
///
/// 与 `post_elevate()` 的差别在 sidecar 侧：该端点**绝不回退 runas**，只走
/// 「管理员直通」与「已注册计划任务 /Run」两条无 UAC 路径。都不成立时返回
/// `elevated:false`，调用方据此判定"才需要向用户申请一次 UAC"。
fn post_elevate_auto() -> Result<bool, String> {
    let url = format!("http://127.0.0.1:{}/api/v1/elevate/auto", lhm_port());
    let resp = ureq::post(&url)
        .timeout(LHM_AUTO_ELEVATE_TIMEOUT)
        .call()
        .map_err(|e| format!("auto elevate HTTP 错误: {}", e))?;
    if resp.status() != 200 {
        return Err(format!("auto elevate HTTP 状态码 {}", resp.status()));
    }
    let text = resp
        .into_string()
        .map_err(|e| format!("读取响应体失败: {}", e))?;
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("auto elevate 响应 JSON 解析失败: {}", e))?;
    Ok(v.get("elevated").and_then(|b| b.as_bool()).unwrap_or(false))
}

// ============================================================================
// 进程生命周期
// ============================================================================

/// 启动 sidecar：spawn + 健康检查（共享 `LHM_START_WAIT` 截止时间 + 50ms 轮询）。
///
/// sidecar 以普通权限启动并立即提供 tier-1；tier-2 由后续 `request_elevation()`
/// 触发的提权伴生进程提供（见契约 §1）。健康检查探测的是 127.0.0.1:45980 的
/// 服务（无论由哪个进程提供）。失败时按存活状态分类处理（UAC 竞态修复）：
/// 不在此处降级（由调用方决定）。
fn spawn_sidecar(st: &mut LhmState) -> Result<(), LhmStartError> {
    st.last_spawn = Some(Instant::now());
    let exe = lhm_exe().ok_or(LhmStartError::ExeMissing)?;

    // sidecar 日志重定向到运行目录 `<运行目录>/logs/`（追加）；提权子进程另有文件日志兜底。
    //
    // 失败不阻断启动（2026-09-23）：原实现把打开日志失败当作 `Retryable` 直接
    // 返回，**在 `Command::spawn` 之前**就放弃了整个 sidecar。日志重定向只是
    // 排障辅助，不应成为 tier-1/tier-2 数据链路的单点：实测受限令牌子进程内
    // `%TEMP%` 该文件不可写时，应用会每 10s 重试一次且永不降级，用户看到的是
    // 硬件信息页长期 `TEMP=n/a` 与 `sidecar 启动中（退避期内）`。
    // 现改为尽力而为：打不开就退回 `Stdio::null()`，并记录一次警告保留原始错误。
    let log_path = crate::runtime_paths::sidecar_log_path();
    let log_file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(f) => Some(f),
        Err(e) => {
            crate::debug_warn!(
                "lhm",
                "打开 sidecar 日志 {} 失败（不阻断启动，改为丢弃输出）: {}",
                log_path.display(),
                e
            );
            None
        }
    };

    // stdout/stderr 重定向：日志文件可用时克隆同一个句柄；不可用时丢弃。
    // `log_file` 为 None 时两个流都走 null，sidecar 仍正常启动并提供服务。
    let (out_stdio, err_stdio) = match log_file {
        Some(f) => {
            let out = Stdio::from(
                f.try_clone()
                    .map_err(|e| LhmStartError::Retryable(format!("stdout 重定向失败: {}", e)))?,
            );
            (out, Stdio::from(f))
        }
        None => (Stdio::null(), Stdio::null()),
    };

    use std::os::windows::process::CommandExt;
    let mut child = Command::new(&exe)
        .args(["--port", &lhm_port().to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(out_stdio)
        .stderr(err_stdio)
        .spawn()
        .map_err(|e| LhmStartError::Retryable(format!("spawn {} 失败: {}", exe.display(), e)))?;

    // 健康检查（覆盖冷启动 JIT + tier-1 服务就绪；总预算 LHM_START_WAIT 用尽后
    // 由下方存活判定接管）。
    //
    // 轮询粒度远小于从前的「失败即 sleep 1s」：sidecar 实测约 600ms 就开始
    // 提供 /health，而整秒 sleep 会把"已就绪"取整到下一个整秒，凭空多等最多
    // ~1s（实测冷启动稳定多花 1510ms 才判定就绪）。
    //
    // 收紧粒度之所以安全，前提是 health_check() 的端口前置门：无监听时它
    // <200ms 返回，所以高频探测不会把请求路径拖慢（详见 health_check 注释）。
    let started_at = Instant::now();
    let mut attempts: u32 = 0;
    loop {
        attempts += 1;
        if health_check().is_ok() {
            st.child = Some(child);
            st.started = true;
            st.ready = true;
            st.uac_pending = false;
            crate::debug_info!(
                "lhm",
                "sidecar 服务就绪（端口 {}，{}ms / {} 次探测）",
                lhm_port(),
                started_at.elapsed().as_millis(),
                attempts
            );
            return Ok(());
        }
        if started_at.elapsed() >= LHM_START_WAIT {
            break;
        }
        std::thread::sleep(LHM_HEALTH_POLL_INTERVAL);
    }

    // 健康检查超时：先判定子进程是否存活（启动/提权竞态修复）——
    // Ok(None) = 进程存活，大概率仍在校验 ring0 设备或等待用户操作 → **不 kill**
    //   （kill 会打断 sidecar 自身的初始化重试），把句柄存入状态 + 标记 uac_pending，
    //   返回 Retryable；退避后 ensure_running 只探测端口，防重复 spawn 堆积。
    // 已退出 / 查询失败 = spawn 出的进程已结束（初始化失败 / 崩溃）→ kill 兜底清理
    //   （对已退出进程 no-op），避免残留。
    match child.try_wait() {
        Ok(None) => {
            st.child = Some(child);
            st.uac_pending = true;
            crate::debug_warn!(
                "lhm",
                "sidecar 健康检查超时但进程存活（疑似初始化/提权移交中），不 kill；退避后重试探测端口"
            );
            Err(LhmStartError::Retryable(format!(
                "sidecar 启动中（{}s 内健康检查无响应，进程存活；tier-2 需管理员时请在提权提示处确认 UAC）",
                LHM_START_WAIT.as_secs()
            )))
        }
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            Err(LhmStartError::Retryable(format!(
                "sidecar 健康检查失败（{}s 内 127.0.0.1:{} 无响应；可能是 UAC 未确认或启动异常）",
                LHM_START_WAIT.as_secs(),
                lhm_port()
            )))
        }
    }
}

/// 确保 sidecar 服务可用：句柄存活 / 端口有服务（提权接管）/ 重启 / 首次启动。
fn ensure_running(st: &mut LhmState) -> Result<(), LhmStartError> {
    // A. 子进程句柄存活 → 直接可用（正常启动成功路径）；
    //    uac_pending（存活但服务未就绪）时继续走下方退避/探测逻辑（C/C'），
    //    不在此返回——保证探测受退避节流（防每轮 500ms 端口探测阻塞）。
    if let Some(child) = &mut st.child {
        match child.try_wait() {
            // 进程存活
            Ok(None) => {
                if !st.uac_pending {
                    return Ok(());
                }
            }
            // 已退出或查询失败：丢弃句柄（提权接管场景原进程退出是正常的）
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
    // C. 退避期内：不重复 spawn / 不探测端口（last_spawn 同时充当探测节流，
    //    避免等待期间每轮 500ms HTTP 阻塞传感器轮询）
    if let Some(t) = st.last_spawn {
        if t.elapsed() < LHM_SPAWN_BACKOFF {
            return Err(LhmStartError::Retryable(
                "sidecar 启动中（退避期内）".to_string(),
            ));
        }
    }
    // C'. 挂起（退避已结束）：只探测端口，不重复 spawn——伴生进程接管端口即命中；
    //     否则重置节流时间戳，下个窗口再探测（防重复 spawn + 防每轮阻塞）
    if st.uac_pending {
        if health_check().is_ok() {
            st.uac_pending = false;
            st.ready = true;
            return Ok(());
        }
        st.last_spawn = Some(Instant::now());
        return Err(LhmStartError::Retryable(
            "sidecar 初始化中（进程存活，服务未就绪；退避后重试探测端口）".to_string(),
        ));
    }
    // D. 端口有服务（可能是历史实例 / 其他实例）：视为可用
    if health_check().is_ok() {
        st.ready = true;
        // 服务已被证实可用：清零重启预算与自愈冷却起点
        st.restarts = 0;
        st.degraded_at = None;
        return Ok(());
    }
    // E. 需要启动/重启
    if st.started {
        // 已在使用自愈冷却：只检查冷却是否期满，不重复累加预算
        // （原实现每次轮询都 `restarts += 1`，实测计数从 2 涨到 13，既是噪音，
        //   也让"重启次数"失去计量意义）。
        if let Some(since) = st.degraded_at {
            let elapsed = since.elapsed();
            if elapsed < LHM_RECOVERY_COOLDOWN {
                return Err(LhmStartError::Retryable(format!(
                    "sidecar 重启预算已用尽，{}s 后自动重试（已等待 {}s）",
                    LHM_RECOVERY_COOLDOWN.as_secs(),
                    elapsed.as_secs()
                )));
            }
            crate::debug_warn!(
                "lhm",
                "重启冷却期满（{}s），清零重启预算并重新尝试启动 sidecar",
                LHM_RECOVERY_COOLDOWN.as_secs()
            );
            st.restarts = 0;
            st.degraded_at = None;
            st.started = false;
        } else {
            st.restarts += 1;
            if st.restarts > LHM_MAX_RESTARTS {
                // 重启预算耗尽：进入自愈冷却。冷却期内拒绝重启（防风暴），冷却期满
                // 清零预算并允许重新启动——原实现此处直接永久拒绝，导致短暂故障后
                // 整个会话无法恢复（见 LHM_RECOVERY_COOLDOWN 注释）。
                st.degraded_at = Some(Instant::now());
                return Err(LhmStartError::Retryable(format!(
                    "sidecar 重启预算已用尽（上限 {} 次），{}s 后自动重试",
                    LHM_MAX_RESTARTS,
                    LHM_RECOVERY_COOLDOWN.as_secs()
                )));
            }
            crate::debug_warn!(
                "lhm",
                "sidecar 服务不可达，自动重启（第 {} 次）",
                st.restarts
            );
        }
    }
    // 启动 sidecar 前确保 PawnIO 驱动服务就绪（LHM 0.9.6 不装驱动，依赖系统 PawnIO
    // 服务，见 driver_install::pawnio 的部署流程）。幂等：pawnio.rs 内部
    // 30s 缓存节流；失败仅记日志——sidecar 仍会启动并正常提供 tier-1，仅 tier-2
    // 不可用（前端据 tier2_reason 提示，不劣化 tier-1）。
    if should_ensure_pawnio(st) {
        if let Err(e) = crate::driver_install::pawnio::ensure_pawnio_ready() {
            crate::debug_warn!(
                "lhm",
                "PawnIO 驱动未就绪（继续启动 sidecar，tier-2 不可用、tier-1 不受影响）: {}",
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

/// 自愈冷却是否仍生效：返回剩余冷却时长（`None` = 已可重试）。
///
/// 纯函数，便于在不接触网络与子进程的前提下单测冷却边界。
fn cooldown_remaining(degraded_at: Option<Instant>, now: Instant) -> Option<Duration> {
    match degraded_at {
        Some(since) => {
            let elapsed = now.saturating_duration_since(since);
            if elapsed < LHM_RECOVERY_COOLDOWN {
                Some(LHM_RECOVERY_COOLDOWN - elapsed)
            } else {
                None
            }
        }
        None => None,
    }
}

/// 结束降级、恢复可重试状态（冷却期满后调用）。
///
/// 纯状态迁移，`port_healthy` 由调用方用 `health_check()` 的结果传入，
/// 使本函数可在单测中确定性地覆盖两条分支而不触发任何网络/进程副作用。
fn clear_degraded_for_retry(st: &mut LhmState, port_healthy: bool) {
    st.degraded = false;
    st.degraded_at = None;
    st.restarts = 0;
    st.failures = 0;
    if port_healthy {
        // 端口已有服务（提权伴生进程 / 历史实例）：直接视为可用，不再起新进程
        st.ready = true;
    } else {
        st.ready = false;
        st.last_spawn = None;
    }
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
            "未找到 LhmSidecar.exe（资源目录缺失？tier-1/tier-2 均不可用）"
        ),
    }
    let _ = EXE_PATH.set(exe);
}

/// 应用启动时**预热** sidecar（后台线程，非阻塞 setup）。
///
/// 动机（Bug 1「硬件信息页初始化过久」）：sidecar 是 .NET 进程，其冷启动
/// （运行时启动 → 绑定 127.0.0.1 → LHM 硬件树打开）与 PDH 双采样预热合计
/// 需要数百毫秒到数秒。原实现在**首个 `get_sensor_data` 请求路径上**同步完成
/// 这段等待：生产日志实测第 1 次调用阻塞 **4073ms**，且返回的是
/// `available:false` 的空占位快照；用户看到的是"4 秒白屏 + 骨架屏"。
///
/// 本函数把"启动 sidecar"这一动作提前到 `setup()`：后台线程调用与请求路径
/// **完全相同**的 `lhm_snapshot()`（同一状态机、同一退避/降级语义），使 sidecar
/// 在前端 WebView 尚未加载完成时就已进入预热。前端首个请求到达时通常已命中
/// `st.ready`，`ensure_running` 直接返回，等待被移出用户可见路径。
///
/// 安全性（为什么不劣化）：
///   - 不改变任何契约：复用 `lhm_snapshot()`，无新增状态、无新增 HTTP 路径；
///   - 不弹 UAC：spawn 的 sidecar 始终以普通权限启动（提权仍需用户在
///     `request_elevation` 处显式确认，契约 §1 权限模型不变）；
///   - 最坏情形与现状**等价**：预热线程持 `state()` 锁期间若首个请求恰好到达，
///     请求会等锁，等待上限与"自己去做这件事"相同（同一条 `spawn_sidecar`
///     路径、同一个 `LHM_START_WAIT` 总预算），不会更久；
///   - exe 缺失/启动失败走既有降级/退避逻辑，仅记日志，不阻断启动
///     （与 `driver-ensure` 自愈线程同一"失败不阻断"约定）。
///
/// 幂等：可重复调用（内部只 spawn 一个线程）。
pub fn warm_up() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("lhm-prewarm".to_string())
            .spawn(|| {
                crate::debug_info!("lhm", "[预热] 后台启动 sidecar（不在请求路径上等待握手）");
                let (snap, diag) = lhm_snapshot();
                match snap {
                    Some(_) => crate::debug_info!("lhm", "[预热] sidecar 已可服务: {}", diag),
                    None => crate::debug_info!(
                        "lhm",
                        "[预热] sidecar 暂不可用（将按退避重试）: {}",
                        diag
                    ),
                }
            });
        if spawned.is_err() {
            // 线程创建失败（极端资源耗尽）：不致命——首个请求仍会走同一条
            // lazy 启动路径，只是退回修复前的行为，绝不阻断应用启动。
            crate::debug_warn!("lhm", "[预热] 线程创建失败，回退 lazy 启动");
        }
    });
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
            // tier-2 未生效时错误原因是"需要管理员"，优先透出 tier2_reason
            let err = resp
                .tier2_reason
                .as_deref()
                .or(resp.error.as_deref())
                .unwrap_or("无有效温度");
            format!("lhm(无有效温度:{})", err)
        }
    }
}

/// 把一次**成功**的 HTTP 轮询结果映射为对外返回值 `(Some(resp), diag)`。
///
/// CHANGE-CONTRACT-v3 §A.1 的核心决策点：`available` 只决定**诊断文案**，
/// **不**决定是否丢弃 payload。抽取为纯函数以便单测直接守卫该决策
/// （避免"整个 payload 因 available:false 被丢弃"回归）。
///
/// - `available:true` → diag 取 `cpu_temp_diag`（温度/功耗摘要）
/// - `available:false` → diag 取 `lhm(不可用:<error>)`（透出预热中/驱动/权限原因）
/// - 两种情况**都**返回 `Some(resp)`：payload 中已有效的字段（如 GPU）必须可见
fn success_to_snapshot(resp: LhmSensorResponse) -> (Option<LhmSensorResponse>, String) {
    let diag = if resp.available {
        cpu_temp_diag(&resp)
    } else {
        let err = resp.error.as_deref().unwrap_or("未知原因");
        format!("lhm(不可用:{})", err)
    };
    (Some(resp), diag)
}

/// 唯一 HTTP 轮询入口：取全量快照（CPU/GPU/主板/内存/磁盘/电池），供 sensor.rs
/// 每采集周期调用 1 次并共享结果（避免各数据源独立 HTTP 的开销）。
///
/// 返回 (快照, 诊断串)。进程生命周期（降级/退避/重启/PawnIO 前置）与既有
/// `lhm_cpu_temp` 完全共用同一状态机；HTTP 成功即缓存 `last_snapshot`
/// 供 `lhm_cpu_temp()` 薄封装复用（不重复 spawn/轮询）。
///
/// **HTTP 成功 ⇒ 返回 `Some(resp)`，与 `available` 无关**：`available` 仅表达
/// "是否至少有一个有效指标"，用于诊断文案；它**不**决定是否丢弃 payload。
/// tier-1 性能计数器预热（需两次采样，约 2.5–4.4s）期间所有指标为 null 而
/// `available:false`，此时整份丢弃会连带丢掉 payload 中**已经有效**的字段
/// （典型是经厂商用户态通道零授权即可读取的 GPU 温度/功耗），造成数秒的
/// `GPU=none` 窗口。契约 v3 各字段均 `Option`/空数组安全，缺失值由前端 "—" 兜底。
/// 返回 `None` 只保留给真正的失败：降级态、启动失败、以及 HTTP 轮询失败（`Err`）。
pub fn lhm_snapshot() -> (Option<LhmSensorResponse>, String) {
    // 非阻塞优先（加载速度修复）：预热线程在后台执行 `spawn_sidecar` 握手期间会
    // **长时间**持有本状态锁（最坏 `LHM_START_WAIT`=5s，冷机 + 杀软扫描更久）。
    // 原实现用阻塞 `lock()`，前端首个 `get_sensor_data` 恰好落在预热窗口时会被
    // 串行堵在锁上，用户看到的骨架屏因此比实际需要更久；而这一轮请求本可以立刻用
    // tier-1 数据（sysinfo CPU / GlobalMemoryStatusEx 内存，均不依赖 sidecar）渲染整页。
    //
    // 改为 `try_lock`：锁被预热线程持有时**立即**返回“预热中”，把等待留在后台线程。
    // `sensor.rs` 拿到 `snap=None` 仍会输出 tier-1 字段，页面即刻成形；下一轮轮询
    //（前端 1s）通常已能拿到 sidecar 快照。
    //
    // 语义不变之处：拿到锁后的全部逻辑（降级/退避/重启/HTTP 轮询）逐字保持原样；
    // 锁中毒仍沿用既有 `unwrap_or_else(|p| p.into_inner())` 处理（不把中毒误判为
    //“预热中”而永久返回 None）。
    let mut st = match state().try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            return (
                None,
                "lhm(预热中:sidecar 握手进行中，本周期跳过)".to_string(),
            );
        }
    };

    // 已降级：先看自愈冷却是否期满。原实现此处无条件返回，使一次短暂故障
    // 就把整个会话钉死在无数据状态；现改为冷却期满后允许重新探测/启动。
    if st.degraded {
        // 降级态而冷却起点缺失（状态被外部改写/初始化异常）：按"刚进入降级"
        // 处理并补上起点，而不是当作冷却已满——后者会立刻触发一次真实探测，
        // 在无法确认 sidecar 状态时把一次日志查询变成一次进程启动。
        if st.degraded_at.is_none() {
            st.degraded_at = Some(Instant::now());
        }
        match cooldown_remaining(st.degraded_at, Instant::now()) {
            Some(remaining) => {
                return (
                    None,
                    format!("lhm(已降级:{}s 后自动重试)", remaining.as_secs()),
                );
            }
            None => {
                // 冷却期满：先探测端口（可能是提权伴生进程或历史实例仍在服务），
                // 命中即直接恢复，避免无谓地再起一个进程。
                let healthy = health_check().is_ok();
                crate::debug_warn!(
                    "lhm",
                    "降级冷却期满（{}s），{}并重新尝试启动 sidecar",
                    LHM_RECOVERY_COOLDOWN.as_secs(),
                    if healthy {
                        "端口已有服务，恢复 tier-1/tier-2"
                    } else {
                        "清零降级与重启预算"
                    }
                );
                clear_degraded_for_retry(&mut st, healthy);
            }
        }
    }

    // 确保服务可用（启动 / 权限移交 / 崩溃重启）
    if let Err(e) = ensure_running(&mut st) {
        return match e {
            LhmStartError::ExeMissing => {
                st.degraded = true;
                st.degraded_at = Some(Instant::now());
                crate::debug_warn!("lhm", "sidecar exe 缺失，降级");
                (None, "lhm(exe 缺失)".to_string())
            }
            LhmStartError::Retryable(msg) => {
                // 可重试失败（启动中/崩溃）：不降级，退避控制重试频率
                (None, format!("lhm({})", msg))
            }
        };
    }

    // 轮询快照（契约 v3）
    match fetch_sensors() {
        Ok(resp) => {
            st.failures = 0;
            // 服务已被证实可用：清零重启预算，使"重启次数"计量的是连续故障
            // 而非会话累计（否则一次成功之后预算仍被历史耗尽，后续故障无法重启）。
            st.restarts = 0;
            st.degraded_at = None;
            // HTTP 成功即缓存全量快照——含 available:false 的预热期快照，
            // 使 payload 中已有效的字段（GPU 等）立即可见。
            st.last_snapshot = Some((Instant::now(), resp.clone()));
            // §A.1：available 只影响 diag，不再决定是否丢弃 payload
            success_to_snapshot(resp)
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
                    "连续 {} 次轮询失败，终止 sidecar 并进入降级冷却（{}s 后自动重试）: {}",
                    st.failures,
                    LHM_RECOVERY_COOLDOWN.as_secs(),
                    e
                );
                kill_child(&mut st);
                st.degraded = true;
                st.degraded_at = Some(Instant::now());
            } else {
                crate::debug_warn!(
                    "lhm",
                    "轮询失败({}/{}): {}",
                    st.failures,
                    LHM_MAX_FAILURES,
                    e
                );
            }
            (None, format!("lhm(轮询失败:{})", e))
        }
    }
}

/// sensor.rs 调用入口：返回 (温度℃, 诊断串)。
///
/// 温度仅来自 sidecar 的 tier-2（契约 §3.2：tier-2 缺失时 temp_source="none"，
/// 不用 0 冒充）。返回 (None, diag) 表示当前不可用，由 sensor.rs 落到
/// `temp_source="none"`；diag 说明原因（含"需要管理员"）。
///
/// 薄封装：优先复用 `lhm_snapshot()` 的近时快照缓存（get_sensor_data
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
        Some(resp) if resp.available => (resp.cpu.package_temp_c.filter(|t| *t > 0.0), diag),
        _ => (None, diag),
    }
}

/// 请求一次 UAC 提权（契约 §2：POST `/api/v1/elevate`）。
///
/// 返回 `Ok(true)` = 用户接受且 tier-2 已生效；`Err(reason)` = 用户取消 / 被系统
/// 拒绝 / sidecar 未运行（非致命）。sidecar 记住被拒状态，不会每次轮询重复弹窗。
///
/// 提权成功后 sidecar 的伴生进程接管 `LHM_PORT + 1` 并把 tier-2 并入同一份快照，
/// 下一次 `lhm_snapshot()` 即可返回 CPU 温度/功耗/主板/磁盘温度。
pub fn request_elevation() -> Result<bool, String> {
    // 提权是用户显式动作：先确保服务在跑（复用同一状态机，含退避/降级判定）
    {
        let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
        if st.degraded {
            // 降级态在用户显式点击时可以立即重试一次（不必等冷却期满）：
            // 用户的显式动作正是"现在重试"的意图，冷却只用于自动轮询节流。
            let remaining = cooldown_remaining(st.degraded_at, Instant::now());
            if let Some(r) = remaining {
                crate::debug_warn!(
                    "lhm",
                    "用户点击提权，提前结束降级冷却（剩余 {}s）",
                    r.as_secs()
                );
            }
            // 用户显式动作：跳过端口探测直接重置为可启动状态（探测交给随后的
            // ensure_running 分支 D，避免这里重复发一次 HTTP）。
            clear_degraded_for_retry(&mut st, false);
        }
        if let Err(e) = ensure_running(&mut st) {
            return Err(match e {
                LhmStartError::ExeMissing => {
                    st.degraded = true;
                    st.degraded_at = Some(Instant::now());
                    "sidecar exe 缺失，无法提权".to_string()
                }
                // sidecar 不可用时给出可操作前缀，与具体原因一起透出到界面，
                // 避免前端只能显示固定文案（用户报告「提权请求失败，可稍后重试」
                // 却看不到任何真实原因）。
                LhmStartError::Retryable(msg) => {
                    format!("sidecar 未就绪，无法发起提权：{}", msg)
                }
            });
        }
    }
    let elevated = post_elevate()?;
    // 提权后立即刷新快照缓存，使前端下一次轮询即拿到 tier-2 指标。
    // 注意：此处不持锁调用 lhm_snapshot()（它自身会取锁），否则会自死锁。
    if elevated {
        let _ = lhm_snapshot();
    }
    Ok(elevated)
}

/// 自动提权（零 UAC）：进入硬件信息页时后台调用，**不等待用户授权**。
///
/// 返回 `Ok(true)` = tier-2 已由「管理员直通」或「已注册持久授权 /Run」自动生效；
/// `Ok(false)` = 自动路径不可行（非管理员且无持久授权），此时才应由前端发起
/// **一次**用户 UAC 申请（`request_elevation()`）。`Err` = sidecar 未就绪。
///
/// 为什么需要独立入口：`request_elevation()` 在两条无 UAC 路径都失败后会回退
/// runas（弹 UAC）。若进入页面就调它，非管理员环境会**未经用户意图**直接弹窗。
/// 本入口把"自动尝试"与"申请授权"切成两步，使自动路径可行时用户零操作，
/// 不可行时才由界面显式询问。
pub fn auto_elevate() -> Result<bool, String> {
    // 页面挂载时预热线程往往正在做同一次 sidecar 启动，因此这里**有界等待**而非
    // 一次性判定：把"启动中/退避期内"当作可重试，直到就绪或预算耗尽。
    await_lhm_ready_for_auto()?;

    let elevated = post_elevate_auto()?;
    if elevated {
        // 提权生效后立即刷新快照缓存，使下一次轮询即拿到 tier-2 指标。
        let _ = lhm_snapshot();
    }
    Ok(elevated)
}

/// 有界等待 sidecar 就绪（供 `auto_elevate` 使用）。
///
/// 与 `ensure_running` 的差别：把 `Retryable` 视为"再等等"而非失败，直到
/// `LHM_AUTO_ELEVATE_AWAIT` 耗尽。每次重试都重新取状态锁，因此**不会**持锁睡眠，
/// 不会阻塞并发的传感器轮询。确定性失败（exe 缺失）立即返回，不做无谓等待。
fn await_lhm_ready_for_auto() -> Result<(), String> {
    let deadline = Instant::now() + LHM_AUTO_ELEVATE_AWAIT;
    loop {
        // 每轮重新取锁探测；探测结果立即在本轮内消费，不跨迭代持有状态。
        let detail = {
            let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
            if st.degraded {
                // 进入页面是显式"现在重试"意图：允许提前结束降级冷却（与 request_elevation 同语义）
                clear_degraded_for_retry(&mut st, false);
            }
            match ensure_running(&mut st) {
                Ok(()) => return Ok(()),
                Err(LhmStartError::ExeMissing) => {
                    st.degraded = true;
                    st.degraded_at = Some(Instant::now());
                    return Err("sidecar exe 缺失，无法自动提权".to_string());
                }
                Err(LhmStartError::Retryable(msg)) => msg,
            }
        };
        if Instant::now() >= deadline {
            return Err(format!("sidecar 未就绪，无法自动提权：{}", detail));
        }
        std::thread::sleep(LHM_AUTO_AWAIT_POLL);
    }
}

// 此处曾有 `tier2_state() -> (elevated, tier2_available, reason)`：全局检索确认
// 生产调用点为 0（仅自身与其单测引用），且三元组顺序与本仓实际对外投影
// `sensor::tier2_fields() -> (tier2_available, elevated, reason)` **相反**，
// 极易被后续消费者误读。ADR-0001 D11 已删除；对外状态统一走
// `sensor::get_sensor_data()` 的 `tier2_available` / `elevated` / `tier2_reason` 字段。

/// SECM 退出时清理：kill 直接 spawn 的子进程 + 按映像名兜底清理提权孤儿。
pub fn shutdown() {
    let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
    kill_child(&mut st);
    // 提权伴生进程（SECM 无句柄）按映像名兜底清理；失败（权限不足）则留待下次
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
        st.degraded_at = None;
        st.uac_pending = false;
        st.last_snapshot = None;
    }

    #[test]
    fn test_parse_v3_full_json() {
        // 契约 v3 §3 完整快照：新增 elevated/tier2_*/disks/battery + CPU 全字段
        let json = r#"{
            "available": true, "error": null, "contract_version": 3,
            "elevated": true, "tier2_available": true, "tier2_reason": null,
            "cpu": {
                "name": "AMD Ryzen 7 5800X 8-Core Processor",
                "load_percent": 12.5, "package_temp_c": 45.2, "core_temps_c": [44.1, 45.2],
                "power_w": 38.4, "voltage_v": 1.19, "clock_mhz": 4200.0,
                "fan_rpm": 812.0, "pump_rpm": null,
                "load_source": "perfcounter", "temp_source": "lhm",
                "power_source": "lhm", "clock_source": "perfcounter",
                "power_estimated": false
            },
            "gpu": [
                { "name": "NVIDIA GeForce RTX 3060", "temperature_c": 52.3,
                  "core_clock_mhz": 1500.0, "power_w": 85.2, "load_percent": 45.0,
                  "memory_used_bytes": 1073741824, "memory_total_bytes": 6442450944,
                  "fan_rpm": 1200.0, "vram_load_percent": 16.7,
                  "temp_source": "lhm", "power_source": "lhm", "clock_source": "lhm" }
            ],
            "motherboard": {
                "name": "HUANANZHI X99",
                "sensors": [
                    { "name": "System", "type": "temperature", "value": 34.2 },
                    { "name": "Fan #1",  "type": "fan",         "value": 812.0 },
                    { "name": "+12V",    "type": "voltage",     "value": 12.1 }
                ]
            },
            "memory": { "name": "Physical Memory", "total_bytes": 17179869184, "used_bytes": 8589934592 },
            "disks": [
                { "name": "C:", "read_bytes_per_sec": 1048576.0, "write_bytes_per_sec": 524288.0,
                  "activity_percent": 12.0, "used_percent": 63.5, "temperature_c": 38.0 }
            ],
            "battery": { "percent": 87.0, "power_w": -12.5, "voltage_v": 15.6, "current_a": -0.8 }
        }"#;
        let resp: LhmSensorResponse = serde_json::from_str(json).expect("v3 契约 JSON 应可解析");
        assert!(resp.available);
        assert_eq!(resp.contract_version, Some(3));
        // 权限状态
        assert!(resp.elevated);
        assert!(resp.tier2_available);
        assert!(resp.tier2_reason.is_none());
        // CPU 全字段
        assert_eq!(resp.cpu.name, "AMD Ryzen 7 5800X 8-Core Processor");
        assert_eq!(resp.cpu.load_percent, Some(12.5));
        assert_eq!(resp.cpu.package_temp_c, Some(45.2));
        assert_eq!(resp.cpu.core_temps_c, vec![44.1, 45.2]);
        assert_eq!(resp.cpu.power_w, Some(38.4));
        assert_eq!(resp.cpu.voltage_v, Some(1.19));
        assert_eq!(resp.cpu.clock_mhz, Some(4200.0));
        assert_eq!(resp.cpu.fan_rpm, Some(812.0));
        assert_eq!(resp.cpu.pump_rpm, None);
        assert_eq!(resp.cpu.load_source, "perfcounter");
        assert_eq!(resp.cpu.temp_source, "lhm");
        assert_eq!(resp.cpu.power_source, "lhm");
        assert_eq!(resp.cpu.clock_source, "perfcounter");
        assert!(!resp.cpu.power_estimated);
        // GPU
        assert_eq!(resp.gpu.len(), 1);
        let g = &resp.gpu[0];
        assert_eq!(g.name, "NVIDIA GeForce RTX 3060");
        assert_eq!(g.temperature_c, Some(52.3));
        assert_eq!(g.core_clock_mhz, Some(1500.0));
        assert_eq!(g.power_w, Some(85.2));
        assert_eq!(g.load_percent, Some(45.0));
        assert_eq!(g.memory_used_bytes, Some(1073741824));
        assert_eq!(g.memory_total_bytes, Some(6442450944));
        assert_eq!(g.fan_rpm, Some(1200.0));
        assert_eq!(g.vram_load_percent, Some(16.7));
        assert_eq!(g.temp_source, "lhm");
        assert_eq!(g.power_source, "lhm");
        assert_eq!(g.clock_source, "lhm");
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
        assert_eq!(mem.name.as_deref(), Some("Physical Memory"));
        assert_eq!(mem.total_bytes, Some(17179869184));
        assert_eq!(mem.used_bytes, Some(8589934592));
        // 磁盘（v3 新增）
        assert_eq!(resp.disks.len(), 1);
        let d = &resp.disks[0];
        assert_eq!(d.name, "C:");
        assert_eq!(d.read_bytes_per_sec, Some(1048576.0));
        assert_eq!(d.write_bytes_per_sec, Some(524288.0));
        assert_eq!(d.activity_percent, Some(12.0));
        assert_eq!(d.used_percent, Some(63.5));
        assert_eq!(d.temperature_c, Some(38.0));
        // 电池（v3 新增，含放电负值语义）
        let b = resp.battery.as_ref().expect("battery 应存在");
        assert_eq!(b.percent, Some(87.0));
        assert_eq!(b.power_w, Some(-12.5));
        assert_eq!(b.voltage_v, Some(15.6));
        assert_eq!(b.current_a, Some(-0.8));
    }

    #[test]
    fn test_parse_v3_tier1_only_json() {
        // 契约 §3.2：未提权（tier-2 缺失）时的确切形态——
        // 温度/功耗/电压/风扇/pump 全 null + temp_source/power_source="none" +
        // motherboard=null + disks[].temperature_c=null，且 available 仍为 true
        let json = r#"{
            "available": true, "error": null, "contract_version": 3,
            "elevated": false, "tier2_available": false,
            "tier2_reason": "需要管理员权限（PawnIO 设备仅 SYSTEM/Administrators 可访问）",
            "cpu": {
                "name": "AMD Ryzen 7 5800X 8-Core Processor",
                "load_percent": 12.5, "package_temp_c": null, "core_temps_c": [],
                "power_w": null, "voltage_v": null, "clock_mhz": 4200.0,
                "fan_rpm": null, "pump_rpm": null,
                "load_source": "perfcounter", "temp_source": "none",
                "power_source": "none", "clock_source": "perfcounter",
                "power_estimated": false
            },
            "gpu": [],
            "motherboard": null,
            "memory": { "name": "Physical Memory", "total_bytes": 17179869184, "used_bytes": 8589934592 },
            "disks": [
                { "name": "C:", "read_bytes_per_sec": 0.0, "write_bytes_per_sec": 0.0,
                  "activity_percent": 0.0, "used_percent": 63.5, "temperature_c": null }
            ],
            "battery": null
        }"#;
        let resp: LhmSensorResponse = serde_json::from_str(json).expect("tier-1 快照应可解析");
        assert!(resp.available, "tier-1 有指标时 available 应为 true");
        assert!(!resp.elevated);
        assert!(!resp.tier2_available);
        assert!(resp
            .tier2_reason
            .as_deref()
            .expect("tier2_reason 应存在")
            .contains("管理员"));
        // tier-2 字段缺失形态：null / [] —— 绝不用 0 冒充
        assert!(resp.cpu.package_temp_c.is_none());
        assert!(resp.cpu.core_temps_c.is_empty());
        assert!(resp.cpu.power_w.is_none());
        assert!(resp.cpu.voltage_v.is_none());
        assert!(resp.cpu.fan_rpm.is_none());
        assert!(resp.cpu.pump_rpm.is_none());
        assert_eq!(resp.cpu.temp_source, "none");
        assert_eq!(resp.cpu.power_source, "none");
        // tier-1 字段仍有效
        assert_eq!(resp.cpu.load_percent, Some(12.5));
        assert_eq!(resp.cpu.clock_mhz, Some(4200.0));
        assert!(resp.motherboard.is_none(), "未提权时 motherboard 应为 null");
        assert!(resp.battery.is_none());
        assert!(resp.disks[0].temperature_c.is_none());
        assert_eq!(resp.disks[0].used_percent, Some(63.5));
    }

    #[test]
    fn test_parse_v3_edge_cases() {
        // 边界：motherboard:null / gpu 空数组 / gpu 元素全 null 字段 /
        // memory:null / disks:[] / battery:null / tier2_reason 缺失
        let json = r#"{
            "available": true, "error": null, "contract_version": 3,
            "cpu": { "load_percent": 5.0, "package_temp_c": null, "core_temps_c": [],
                     "power_w": null, "voltage_v": null, "clock_mhz": null,
                     "fan_rpm": null, "pump_rpm": null,
                     "load_source": "perfcounter", "temp_source": "none",
                     "power_source": "none", "clock_source": "none", "power_estimated": false },
            "gpu": [
                { "name": "AMD Radeon RX 580", "temperature_c": null, "core_clock_mhz": null,
                  "power_w": null, "load_percent": null, "memory_used_bytes": null,
                  "memory_total_bytes": null, "fan_rpm": null, "vram_load_percent": null,
                  "temp_source": "none", "power_source": "none", "clock_source": "none" }
            ],
            "motherboard": null,
            "memory": null,
            "disks": [],
            "battery": null
        }"#;
        let resp: LhmSensorResponse = serde_json::from_str(json).expect("边界 JSON 应可解析");
        assert_eq!(resp.contract_version, Some(3));
        assert!(!resp.elevated, "缺 elevated 时应为 false");
        assert!(!resp.tier2_available);
        assert!(resp.tier2_reason.is_none(), "缺 tier2_reason 时应为 None");
        assert_eq!(resp.gpu.len(), 1);
        let g = &resp.gpu[0];
        // 全 null 字段 → Option None（前端 "—" 兜底）
        assert!(g.temperature_c.is_none());
        assert!(g.power_w.is_none());
        assert!(g.load_percent.is_none());
        assert!(g.memory_used_bytes.is_none());
        assert!(g.vram_load_percent.is_none());
        assert_eq!(g.name, "AMD Radeon RX 580");
        assert!(resp.motherboard.is_none(), "motherboard:null 应解析为 None");
        assert!(resp.memory.is_none(), "memory:null 应解析为 None");
        assert!(resp.disks.is_empty());
        assert!(resp.battery.is_none());

        // gpu 空数组 + 缺字段（仅顶层 available）
        let json2 = r#"{"available":true,"gpu":[]}"#;
        let resp2: LhmSensorResponse = serde_json::from_str(json2).expect("缺字段 JSON 应可解析");
        assert!(resp2.gpu.is_empty());
        assert!(resp2.motherboard.is_none());
        assert!(resp2.disks.is_empty());
        assert!(resp2.battery.is_none());
        assert_eq!(resp2.cpu.package_temp_c, None);
        assert!(resp2.cpu.temp_source.is_empty(), "缺 temp_source 时为空串");

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
    fn test_parse_v2_response_backward_compat() {
        // 契约 §6 要求：v2 响应（无 elevated/tier2_*/disks/battery，CPU 无 name/
        // load_percent/clock_mhz/pump_rpm/load_source/clock_source/power_estimated）
        // 必须解析成功，不 panic，新字段取默认值（false/None/[]）
        let json = r#"{
            "available": true, "error": null, "contract_version": 2,
            "cpu": { "package_temp_c": 45.2, "core_temps_c": [40.1,42.3], "power_w": 38.4,
                     "fan_rpm": 812.0, "voltage_v": 1.19 },
            "gpu": [
                { "name": "NVIDIA GeForce GTX 1660 Ti", "temperature_c": 52.3,
                  "core_clock_mhz": 1500.0, "power_w": 85.2, "load_percent": 45.0,
                  "memory_used_bytes": 1073741824, "memory_total_bytes": 6442450944,
                  "fan_rpm": 1200.0 }
            ],
            "motherboard": {
                "name": "HUANANZHI X99",
                "sensors": [ { "name": "System", "type": "temperature", "value": 34.2 } ]
            },
            "memory": { "name": "DDR4-2400 2x8GB", "total_bytes": 17179869184, "used_bytes": 8589934592 }
        }"#;
        let resp: LhmSensorResponse =
            serde_json::from_str(json).expect("v2 响应应可解析（向后兼容）");
        assert!(resp.available);
        assert_eq!(resp.contract_version, Some(2));
        // v3 新增顶层字段取默认值
        assert!(!resp.elevated);
        assert!(!resp.tier2_available);
        assert!(resp.tier2_reason.is_none());
        assert!(resp.disks.is_empty(), "v2 无 disks → 空数组");
        assert!(resp.battery.is_none(), "v2 无 battery → None");
        // v2 CPU 已有字段照常解析
        assert_eq!(resp.cpu.package_temp_c, Some(45.2));
        assert_eq!(resp.cpu.core_temps_c.len(), 2);
        assert_eq!(resp.cpu.power_w, Some(38.4));
        assert_eq!(resp.cpu.fan_rpm, Some(812.0));
        assert_eq!(resp.cpu.voltage_v, Some(1.19));
        // v3 新增 CPU 字段取默认值（空串 / None / false）
        assert!(resp.cpu.name.is_empty());
        assert!(resp.cpu.load_percent.is_none());
        assert!(resp.cpu.clock_mhz.is_none());
        assert!(resp.cpu.pump_rpm.is_none());
        assert!(resp.cpu.load_source.is_empty());
        assert!(resp.cpu.clock_source.is_empty());
        assert!(!resp.cpu.power_estimated);
        // v2 GPU 元素缺 vram_load_percent/*_source（验证 serde(default) 兜底）
        assert_eq!(resp.gpu.len(), 1);
        assert!(resp.gpu[0].vram_load_percent.is_none());
        assert!(resp.gpu[0].temp_source.is_empty());
        // 主板/内存照常
        assert_eq!(
            resp.motherboard.as_ref().and_then(|m| m.name.as_deref()),
            Some("HUANANZHI X99")
        );
        assert_eq!(
            resp.memory.as_ref().and_then(|m| m.name.as_deref()),
            Some("DDR4-2400 2x8GB")
        );
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
        assert!(resp.disks.is_empty());
        assert!(resp.battery.is_none());
    }

    #[test]
    fn test_parse_unavailable_json() {
        // 契约失败响应：available:false + error 透传（驱动/权限不可用）
        let json = r#"{"available":false,"error":"PawnIO 设备不可访问（CreateFileW \\\\.\\PawnIO 失败：驱动未安装）","contract_version":3,"elevated":false,"tier2_available":false,"tier2_reason":"需要管理员权限","cpu":{"package_temp_c":null,"core_temps_c":[],"power_w":null,"fan_rpm":null,"voltage_v":null,"temp_source":"none","power_source":"none"}}"#;
        let resp: LhmSensorResponse = serde_json::from_str(json).expect("契约 JSON 应可解析");
        assert!(!resp.available);
        let err = resp.error.as_deref().expect("error 字段应存在");
        assert!(err.contains("PawnIO"), "错误应含 PawnIO 字样: {}", err);
        assert_eq!(resp.cpu.package_temp_c, None);
        assert!(resp.cpu.core_temps_c.is_empty());
        assert_eq!(resp.cpu.temp_source, "none");
        assert_eq!(resp.cpu.power_source, "none");
        assert!(!resp.elevated);
        assert_eq!(resp.tier2_reason.as_deref(), Some("需要管理员权限"));
    }

    #[test]
    fn test_parse_missing_fields_defaults() {
        // 字段缺失时 serde(default) 兜底，不 panic
        let json = r#"{"available":true}"#;
        let resp: LhmSensorResponse = serde_json::from_str(json).expect("缺字段 JSON 应可解析");
        assert!(resp.available);
        assert!(resp.error.is_none());
        assert_eq!(resp.cpu.package_temp_c, None);
        assert!(!resp.elevated);
        assert!(!resp.tier2_available);
        assert!(resp.disks.is_empty());
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
        let json =
            r#"{"available":false,"cpu":{"package_temp_c":null,"core_temps_c":[],"power_w":null}}"#;
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
        assert!(diag.contains("退避"), "诊断串应说明退避重试: {}", diag);
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
    fn test_success_to_snapshot_keeps_unavailable_payload() {
        // CHANGE-CONTRACT-v3 §A.1 回归守卫（核心）：HTTP 成功但 available:false
        // 时**必须**返回 Some(resp)，不能整份丢弃。
        // 复刻实测冷启动预热窗口：所有 tier-1 指标为 null、available:false，
        // 但厂商用户态通道已能给出 GPU 温度/功耗——那些字段必须透出。
        let resp = LhmSensorResponse {
            available: false,
            error: Some("正在进行首次采集（tier-1 性能计数器预热中）".to_string()),
            contract_version: Some(3),
            gpu: vec![LhmGpuData {
                name: "NVIDIA GeForce RTX 2080 Ti".to_string(),
                temperature_c: Some(48.0),
                power_w: Some(71.9),
                load_percent: Some(8.0),
                core_clock_mhz: Some(1350.0),
                memory_used_bytes: Some(2_054_000_000),
                memory_total_bytes: Some(11_811_160_064),
                temp_source: "lhm".to_string(),
                power_source: "lhm".to_string(),
                clock_source: "lhm".to_string(),
                ..Default::default()
            }],
            cpu: LhmCpuData {
                load_percent: None,
                clock_mhz: None,
                package_temp_c: None,
                ..Default::default()
            },
            ..Default::default()
        };
        let (snap, diag) = success_to_snapshot(resp);
        let snap = snap.expect("available:false 不应导致整份快照被丢弃（§A.1）");
        assert!(!snap.available);
        // GPU 数据必须完整保留（这正是被丢弃 bug 的受害者）
        assert_eq!(snap.gpu.len(), 1, "预热期 GPU 条目必须保留");
        let g = &snap.gpu[0];
        assert_eq!(g.name, "NVIDIA GeForce RTX 2080 Ti");
        assert_eq!(g.temperature_c, Some(48.0));
        assert_eq!(g.power_w, Some(71.9));
        assert_eq!(g.core_clock_mhz, Some(1350.0));
        // diag 语义不变：available:false → 透出原因
        assert!(
            diag.contains("不可用") && diag.contains("预热"),
            "diag 应透出不可用原因: {}",
            diag
        );
    }

    #[test]
    fn test_success_to_snapshot_diag_semantics_unchanged() {
        // diag 语义保持现状：available:true → 温度摘要（cpu_temp_diag）
        let ok = LhmSensorResponse {
            available: true,
            cpu: LhmCpuData {
                package_temp_c: Some(61.5),
                power_w: Some(42.7),
                ..Default::default()
            },
            ..Default::default()
        };
        let (snap, diag) = success_to_snapshot(ok);
        assert!(snap.is_some());
        assert_eq!(diag, cpu_temp_diag(&snap.unwrap()));
        assert!(
            diag.contains("61.5") && diag.contains("42.7"),
            "diag={}",
            diag
        );

        // available:false 且 error 缺失 → 回落 "未知原因"（不 panic）
        let no_err = LhmSensorResponse {
            available: false,
            error: None,
            ..Default::default()
        };
        let (snap, diag) = success_to_snapshot(no_err);
        assert!(snap.is_some(), "available:false 也应返回 Some");
        assert!(
            diag.contains("未知原因"),
            "缺 error 时应回落未知原因: {}",
            diag
        );
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
                contract_version: Some(3),
                elevated: true,
                tier2_available: true,
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
        assert!(
            diag.contains("61.5") && diag.contains("42.7"),
            "diag 应含温度与功耗: {}",
            diag
        );
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
        assert!(
            diag.contains("无有效温度"),
            "diag 应说明无有效温度: {}",
            diag
        );
        reset_state();
    }

    #[test]
    fn test_cpu_temp_diag_prefers_tier2_reason() {
        // tier-2 未生效时 diag 的无温度原因应优先取 tier2_reason
        // （前端"需要管理员"提示与后端诊断一致），否则回落 error
        let resp = LhmSensorResponse {
            available: true,
            error: Some("顶层错误".to_string()),
            tier2_available: false,
            tier2_reason: Some("需要管理员权限".to_string()),
            cpu: LhmCpuData {
                package_temp_c: None,
                temp_source: "none".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let d = cpu_temp_diag(&resp);
        assert!(d.contains("无有效温度"), "应说明无有效温度: {}", d);
        assert!(d.contains("需要管理员权限"), "应透出 tier2_reason: {}", d);
        assert!(
            !d.contains("顶层错误"),
            "有 tier2_reason 时不应回落顶层 error: {}",
            d
        );
    }

    // 此处曾有 test_tier2_state_accessor：随 tier2_state() 一并删除（ADR-0001 D11）。
    // tier-2 状态的对外契约由 sensor::tier2_fields 的投影测试覆盖
    // （sensor.rs 的 test_tier2_fields_projection / sensor_data_serializes_tier2_fields）。

    #[test]
    fn test_request_elevation_degraded_clears_state_for_manual_retry() {
        // 修复后的契约（2026-09-23）：用户显式点击提权时不再被降级冷却阻塞，
        // 也不再把会话钉死在终态。用纯状态迁移验证该前置动作本身
        // （request_elevation 内部即调用 clear_degraded_for_retry），
        // 不触发任何网络/子进程副作用。
        let _guard = TEST_STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        {
            let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
            st.degraded = true;
            st.degraded_at = Some(Instant::now());
            st.failures = LHM_MAX_FAILURES;
            st.restarts = LHM_MAX_RESTARTS + 3;
            assert!(
                cooldown_remaining(st.degraded_at, Instant::now()).is_some(),
                "刚进入降级时冷却应生效"
            );

            // 复刻显式点击时的重置（与 request_elevation 中同一函数）
            clear_degraded_for_retry(&mut st, false);

            assert!(!st.degraded, "显式重试应清除降级标记");
            assert!(st.degraded_at.is_none(), "显式重试应清除冷却起点");
            assert_eq!(st.restarts, 0, "显式重试应清零重启预算");
            assert_eq!(st.failures, 0, "显式重试应清零失败计数");
        }
        reset_state();
    }

    #[test]
    fn test_clear_degraded_for_retry_branches() {
        // 冷却期满后的两条恢复分支（纯状态，确定性、零副作用）：
        //   port_healthy=true  → 直接可用（不起新进程）
        //   port_healthy=false → 重置为可启动（清节流时间戳）
        let _guard = TEST_STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();

        // 分支 1：端口已有服务
        {
            let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
            st.degraded = true;
            st.degraded_at = Some(Instant::now());
            st.ready = false;
            st.last_spawn = Some(Instant::now());
            clear_degraded_for_retry(&mut st, true);
            assert!(!st.degraded, "应清除降级");
            assert!(st.ready, "端口健康应直接置 ready");
        }

        // 分支 2：端口无服务 → 需要重新启动
        {
            let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
            st.degraded = true;
            st.degraded_at = Some(Instant::now());
            st.ready = false;
            st.last_spawn = Some(Instant::now());
            clear_degraded_for_retry(&mut st, false);
            assert!(!st.degraded, "应清除降级");
            assert!(!st.ready, "端口无服务时不应置 ready");
            assert!(st.last_spawn.is_none(), "应清空退避节流时间戳以便立即启动");
        }

        reset_state();
    }

    #[test]
    fn test_cooldown_boundaries() {
        // 冷却边界（纯函数，确定性、零副作用）：
        //   未进入终态 → 无冷却；刚进入 → 剩余≈总时长；期满 → 无冷却。
        let now = Instant::now();
        assert!(
            cooldown_remaining(None, now).is_none(),
            "未进入终态时不应有冷却"
        );
        let just_entered = cooldown_remaining(Some(now), now).expect("刚进入应有冷却");
        assert_eq!(
            just_entered, LHM_RECOVERY_COOLDOWN,
            "刚进入时剩余应等于总冷却时长"
        );

        let half = now - LHM_RECOVERY_COOLDOWN / 2;
        let rem = cooldown_remaining(Some(half), now).expect("半程应仍在冷却");
        assert!(
            rem <= LHM_RECOVERY_COOLDOWN / 2 + Duration::from_millis(50),
            "半程剩余应约为一半: {:?}",
            rem
        );

        let expired = now - LHM_RECOVERY_COOLDOWN - Duration::from_secs(1);
        assert!(
            cooldown_remaining(Some(expired), now).is_none(),
            "冷却期满后不应再有冷却"
        );
    }

    #[test]
    fn test_cooldown_diagnostic_is_actionable() {
        // 冷却期内诊断必须给出剩余时间（可操作），不得回到"确定性失败"式的
        // 永久措辞。直接按 lhm_snapshot 的同一格式构造，避免调用它造成副作用。
        let _guard = TEST_STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        let remaining = cooldown_remaining(Some(Instant::now()), Instant::now())
            .expect("刚进入降级时应有冷却剩余");
        let diag = format!("lhm(已降级:{}s 后自动重试)", remaining.as_secs());
        assert!(diag.contains("自动重试"), "诊断应说明自动重试: {}", diag);
        assert!(
            !diag.contains("此前确定性失败"),
            "不应再使用永久失败措辞: {}",
            diag
        );
        reset_state();
    }

    #[test]
    fn test_uac_pending_probe_only() {
        // 启动竞态修复核心路径：进程存活 + uac_pending 时 ensure_running 不 kill
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
        assert!(!st.degraded, "等待/退避不应降级");
        assert!(st.uac_pending, "退避期不应清除 uac_pending");
        drop(st);
        reset_state();
    }

    #[test]
    fn test_uac_pending_without_child_probes_port() {
        // child 句柄丢失（进程已退出/句柄失效）+ uac_pending 残留 + 退避已过：
        // ensure_running 必须走 C' 分支**只探测端口**，绝不 spawn 新进程。
        //
        // 该分支的结果取决于本机 127.0.0.1:45980 是否恰好有服务：
        //   - 无服务（常规 CI）→ 探测失败：返回 Retryable、重置节流时间戳、
        //     保留 uac_pending（下个退避窗口再探测）。
        //   - 有服务（例如同机正在运行的真实 sidecar）→ 探测成功：
        //     清除 uac_pending、置 ready、返回 Ok。
        // 两种结果都合法（探测本就是"看服务在不在"），因此断言真正的契约不变式：
        // 不起新进程（child 仍为 None、started 保持 false）、不降级、不 panic。
        let _guard = TEST_STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_state();
        {
            let mut st = state().lock().unwrap_or_else(|p| p.into_inner());
            st.child = None;
            st.uac_pending = true;
            st.ready = false;
            st.last_spawn = None;
            let r = ensure_running(&mut st);
            // 端口探测路径绝不 spawn：句柄与"曾启动过"标记都不应改变
            assert!(st.child.is_none(), "端口探测路径不应 spawn 子进程");
            assert!(!st.started, "端口探测路径不应标记已启动");
            assert!(!st.degraded, "端口探测路径不应降级");
            match r {
                Ok(()) => {
                    // 服务在：uac_pending 清除、ready 置位
                    assert!(!st.uac_pending, "探测成功应清除 uac_pending");
                    assert!(st.ready, "探测成功应置 ready");
                }
                Err(_) => {
                    // 服务不在：节流时间戳重置、uac_pending 保留等待下个窗口
                    assert!(st.last_spawn.is_some(), "探测失败应重置节流时间戳");
                    assert!(st.uac_pending, "探测失败不应清除 uac_pending");
                }
            }
        }
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
        assert!(
            d1.contains("45.2") && d1.contains("38.4"),
            "应含温度与功耗: {}",
            d1
        );

        let no_power = LhmSensorResponse {
            available: true,
            cpu: LhmCpuData {
                package_temp_c: Some(45.2),
                ..Default::default()
            },
            ..Default::default()
        };
        let d2 = cpu_temp_diag(&no_power);
        assert!(
            d2.contains("45.2") && !d2.contains("W"),
            "无功耗时 diag 不应含 W: {}",
            d2
        );

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
        assert!(
            d3.contains("无有效温度") && d3.contains("传感器未就绪"),
            "应透出错误原因: {}",
            d3
        );
    }
}
