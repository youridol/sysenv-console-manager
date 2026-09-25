//! 硬件传感器对外契约集成测试（契约 v3，替代已删除的 hardware_integration）。
//!
//! 背景：SECM 的 ad-hoc 硬件服务（`src-tauri/src/hardware.rs`、17860 HTTP 服务、
//! `nvidia-smi` 解析、`SECM_HARDWARE_MOCK`）已整体移除；硬件数据唯一来源是
//! `LhmSidecar`（`sidecar-lhm/`，契约见 `sidecar-lhm/CONTRACT-v3.md`），
//! `sensor.rs` 退化为该快照的投影层。
//!
//! 本文件是「不伪造 0」这一中心需求的回归护栏。验证：
//!   1. sidecar 缺失时 `secm_lib::sensor::get_sensor_data()` 仍返回结构合法的
//!      `SensorData`、不 panic、不可用指标 `*_source == "none"`；
//!   2. `*_source` 枚举收窄（CONTRACT-v3.md §7）：`ntapi` / `sysinfo` /
//!      `winring0` / `acpi` 不可能再出现；
//!   3. 序列化 JSON 字段名不变（前端 Dashboard 依赖）；
//!   4. 零值必须与 `"none"` 数据源严格绑定——不用 0 冒充有效数据，也不为
//!      不可用数据编造非零值。
//!
//! 运行：`cd src-tauri; cargo test --test hardware_contract`
//!
//! 确定性说明：测试把进程 CWD 切到空临时目录后再取快照，使 `lhm.rs`
//! `resolve_lhm_exe` 的 4 种布局（resource_dir / exe 同目录 / 当前目录 /
//! `src-tauri/resources/lhm/publish`）全部落空 → `lhm_snapshot()` 走
//! `ExeMissing` 确定性降级。这样既不依赖机器状态，也不会 spawn 真实 sidecar
//! （避免测试弹出 UAC 或阻塞健康检查重试）。CWD 变化由进程内互斥锁串行化，
//! 退出时（含 panic）无条件还原。

use std::path::PathBuf;
use std::sync::Mutex;

use secm_lib::sensor::SensorData;

mod common;

// ============================================================================
// sidecar 缺失的确定性测试环境
// ============================================================================

/// CWD 是进程级状态，测试默认并行 —— 所有切 CWD 的用例必须持锁串行。
static CWD_LOCK: Mutex<()> = Mutex::new(());

/// 退出作用域（含 panic 展开）时无条件还原 CWD。
struct CwdGuard(PathBuf);

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

/// `lhm.rs::resolve_lhm_exe` 在 debug 构建下会优先读取 `SECM_LHM_DIR`（测试注入用）。
/// 若宿主环境恰好设置了该变量，sidecar 会被判定为"存在"并真的被 spawn —— 那会
/// 弹 UAC / 阻塞健康检查。护栏显式清除它，退出时恢复原值。
struct LhmDirGuard(Option<String>);

impl Drop for LhmDirGuard {
    fn drop(&mut self) {
        match &self.0 {
            Some(v) => std::env::set_var("SECM_LHM_DIR", v),
            None => std::env::remove_var("SECM_LHM_DIR"),
        }
    }
}

/// 端口护栏：`lhm.rs::lhm_port()` 在 debug 构建下读取 `SECM_LHM_PORT`。
/// 退出作用域时恢复原值。
struct LhmPortGuard(Option<String>);

impl Drop for LhmPortGuard {
    fn drop(&mut self) {
        match &self.0 {
            Some(v) => std::env::set_var("SECM_LHM_PORT", v),
            None => std::env::remove_var("SECM_LHM_PORT"),
        }
    }
}

/// 在「sidecar 确定性缺失」的环境下执行闭包。
///
/// 需要同时消除**两个**独立的"存在"信号，缺一不可：
///   1. 可执行文件不可定位（空 CWD + 清除 `SECM_LHM_DIR`）→ 不会 spawn；
///   2. **端口无服务**（`SECM_LHM_PORT` 指向一个确定无监听的端口）→ 不会复用
///      开发机上已常驻的 sidecar。
///
/// 第 2 点是必须的：`ensure_running` 的 D 分支只要 `health_check()` 成功就判定
/// 「端口有服务（历史提权子进程 / 其他实例）→ 视为可用」并直接返回 Ok，因此**仅**
/// 让 exe 缺失并不足以构造"无 sidecar"。实测：应用正在运行（45980 常驻 sidecar）
/// 时，本文件的 sidecar-absent 用例会取到真实快照而误报失败。
///
/// 端口选择一个高位且测试期间不会有人监听的固定值；该值同时用于断言"确实无服务"。
fn with_sidecar_absent<T>(f: impl FnOnce() -> T) -> T {
    /// 契约测试专用的"确定无服务"端口（高位，避开 45980/45981 与各测试端口段）。
    const ABSENT_PORT: u16 = 45997;

    let _lock = CWD_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let original = std::env::current_dir().expect("读取当前工作目录失败");
    // 夹具落在项目内 build/tmp/：受限完整性环境下系统 %TEMP% 不可写（实测 OS 错误 5）
    let tmp = common::fixture_dir(&format!("hw-contract-{}", std::process::id()));
    std::env::set_current_dir(&tmp).expect("切换工作目录失败");
    let _restore = CwdGuard(original);
    let _env = LhmDirGuard(std::env::var("SECM_LHM_DIR").ok());
    std::env::remove_var("SECM_LHM_DIR");
    let _port = LhmPortGuard(std::env::var("SECM_LHM_PORT").ok());
    std::env::set_var("SECM_LHM_PORT", ABSENT_PORT.to_string());

    // 前置条件自校验：端口确实无服务，否则本环境无法表达"sidecar 缺失"，
    // 用例结论无意义（宁可显式失败也不静默取到真实快照）。
    assert!(
        std::net::TcpStream::connect_timeout(
            &(std::net::Ipv4Addr::LOCALHOST, ABSENT_PORT).into(),
            std::time::Duration::from_millis(300),
        )
        .is_err(),
        "前置条件失败：127.0.0.1:{} 意外有服务监听，无法构造 sidecar 缺失场景",
        ABSENT_PORT
    );

    f()
}

/// 契约 §7 收窄后的取值集合（前端类型与角标语义同步）。
///
/// `freq_source` 含 `lhm`：sidecar `SnapshotBuilder.cs:148-155` 在 tier-1 的 PDH
/// 与注册表标称都不可用、而 tier-2 已生效时回退 LHM 复合 CPU 频率（真实测值），
/// 映射为 `"lhm"`（实测语义，前端不加"估算"/"标称"角标）。
const FREQ_SOURCES: [&str; 4] = ["pdh", "registry", "lhm", "none"];
const TEMP_SOURCES: [&str; 2] = ["lhm", "none"];
const POWER_SOURCES: [&str; 3] = ["lhm", "estimated", "none"];

/// CONTRACT-v3.md §7 明确不再可能出现的旧取值。
const FORBIDDEN_SOURCES: [&str; 4] = ["ntapi", "sysinfo", "winring0", "acpi"];

// ============================================================================
// 1. sidecar 缺失：结构合法 + 不 panic + 不伪造
// ============================================================================

#[test]
fn sidecar_absent_snapshot_is_structurally_valid() {
    let data = with_sidecar_absent(secm_lib::sensor::get_sensor_data);

    // 内存权威值来自 GlobalMemoryStatusEx（不依赖 sidecar），Windows 上必须可用
    assert!(
        data.memory.total > 0,
        "权威内存总量应为正（GlobalMemoryStatusEx）: {:?}",
        data.memory
    );
    assert!(
        data.memory.used <= data.memory.total,
        "已用内存不得超过总量: used={} total={}",
        data.memory.used,
        data.memory.total
    );
    assert!(
        data.memory.available <= data.memory.total,
        "可用内存不得超过总量: available={} total={}",
        data.memory.available,
        data.memory.total
    );
    assert!(
        data.memory.usage_percent.is_finite() && (0.0..=100.0).contains(&data.memory.usage_percent),
        "内存使用率必须在 0..=100 且有限: {}",
        data.memory.usage_percent
    );

    // CPU：usage/per_core 来自 sysinfo，与 sidecar 无关
    assert!(data.cpu.core_count >= 1, "逻辑核心数应为正: {}", data.cpu.core_count);
    assert!(!data.cpu.per_core.is_empty(), "per_core 不应为空（前端逐核渲染）");
    assert!(
        data.cpu.usage.is_finite() && (0.0..=100.0).contains(&data.cpu.usage),
        "CPU 占用率必须在 0..=100 且有限: {}",
        data.cpu.usage
    );
    for (i, c) in data.cpu.per_core.iter().enumerate() {
        assert!(
            c.is_finite() && (0.0..=100.0).contains(c),
            "per_core[{i}] 非法: {c}"
        );
    }

    // 磁盘：结构合法、自洽、数值有限非负
    for d in &data.disks {
        assert!(!d.name.is_empty(), "磁盘名不应为空");
        assert!(
            d.used_space <= d.total_space,
            "磁盘 {0} 已用 > 总量: {1} > {2}",
            d.name,
            d.used_space,
            d.total_space
        );
        assert!(
            d.available_space <= d.total_space,
            "磁盘 {0} 可用 > 总量: {1} > {2}",
            d.name,
            d.available_space,
            d.total_space
        );
        if d.total_space > 0 {
            assert!(
                d.usage_percent.is_finite() && (0.0..=100.0).contains(&d.usage_percent),
                "磁盘 {0} 使用率非法: {1}",
                d.name,
                d.usage_percent
            );
        }
        assert!(
            d.read_mbps.is_finite() && d.read_mbps >= 0.0,
            "磁盘 {0} 读速度非法: {1}",
            d.name,
            d.read_mbps
        );
        assert!(
            d.write_mbps.is_finite() && d.write_mbps >= 0.0,
            "磁盘 {0} 写速度非法: {1}",
            d.name,
            d.write_mbps
        );
    }

    // GPU：sidecar 缺失 + nvidia-smi 链路已移除 → 空列表（不是含伪造 0 的占位项）
    assert!(
        data.gpu.is_empty(),
        "sidecar 缺失时不应编造 GPU 条目（nvidia-smi 链路已移除）: {:?}",
        data.gpu
    );

    // 诊断串必须可读（前端频率诊断卡依赖其非空）
    assert!(!data.diag.is_empty(), "diag 诊断串不应为空");
}

#[test]
fn sidecar_absent_reports_none_for_unavailable_metrics() {
    let data = with_sidecar_absent(secm_lib::sensor::get_sensor_data);

    // tier-2（LHM + PawnIO ring0）不可用：温度/功耗/主板一律"无数据"，绝不用 0 冒充
    assert_eq!(
        data.cpu.temp_source, "none",
        "sidecar 缺失时 temp_source 必须为 none（winring0/acpi 通道已删除）"
    );
    assert_eq!(
        data.cpu.temperature, 0.0,
        "无温度数据源时不得输出非零温度: {}",
        data.cpu.temperature
    );
    assert_eq!(
        data.cpu.power_source, "none",
        "sidecar 缺失时 power_source 必须为 none（TDP 估算表已删除）"
    );
    assert_eq!(
        data.cpu.power_w, 0.0,
        "无功耗数据源时不得输出非零功耗: {}",
        data.cpu.power_w
    );
    assert!(
        !data.cpu.power_estimated,
        "power_source=none 时 power_estimated 必须为 false"
    );
    assert!(
        data.motherboard.is_none(),
        "sidecar 缺失时主板传感器应为 None（而非空/伪造结构）"
    );

    // 频率：CONTRACT-v3 §7 允许 pdh/registry/lhm 或 none；
    // 但 none 时必须是真正的 0，不能是"有值却标 none"或"标 none 却给值"。
    assert!(
        FREQ_SOURCES.contains(&data.cpu.freq_source.as_str()),
        "sidecar 缺失时 freq_source 越界: {}",
        data.cpu.freq_source
    );
    if data.cpu.freq_source == "none" {
        assert_eq!(
            data.cpu.clock_mhz, 0.0,
            "freq_source=none 时不得输出非零频率: {}",
            data.cpu.clock_mhz
        );
    } else {
        assert!(
            data.cpu.clock_mhz > 0.0,
            "freq_source={} 却无有效频率: {}",
            data.cpu.freq_source,
            data.cpu.clock_mhz
        );
    }

    // CPU 型号名：sidecar 缺失时仍可由 sysinfo/注册表提供，但必须是有效字符串
    assert!(!data.cpu.name.is_empty(), "CPU 名称不应为空");
}

#[test]
fn repeated_snapshots_stay_well_formed() {
    // 降级是记忆化状态（degraded=true 后短路）—— 连续调用必须稳定、不 panic、
    // 不因缓存/状态机漂移而开始输出伪造值。
    let samples = with_sidecar_absent(|| {
        (0..3)
            .map(|_| secm_lib::sensor::get_sensor_data())
            .collect::<Vec<_>>()
    });
    for (i, d) in samples.iter().enumerate() {
        assert_eq!(d.cpu.temp_source, "none", "第 {i} 次采样 temp_source 漂移");
        assert_eq!(d.cpu.temperature, 0.0, "第 {i} 次采样温度漂移");
        assert!(d.memory.total > 0, "第 {i} 次采样内存总量丢失");
        assert!(!d.diag.is_empty(), "第 {i} 次采样诊断串为空");
    }
}

// ============================================================================
// 2. 核心回归：不伪造 0（任何环境下都必须成立的不变式）
// ============================================================================

/// 校验单份快照的「零值 ⇄ none」不变式。
///
/// 该不变式不依赖 sidecar 是否存在，因此在任何机器上都成立：
/// - 数据源为 `none` ⇒ 对应数值必须为 0（Rust 侧用 0 表示 null，前端据 source 渲染 "—"）；
/// - 数据源有效（lhm/pdh/registry/estimated）⇒ 数值必须为正，且不得自称 none。
fn assert_no_fabricated_zeros(data: &SensorData) {
    // ---- CPU 频率 ----
    match data.cpu.freq_source.as_str() {
        "none" => assert_eq!(
            data.cpu.clock_mhz, 0.0,
            "freq_source=none 却给出了频率 {}（伪造）",
            data.cpu.clock_mhz
        ),
        "pdh" | "registry" | "lhm" => assert!(
            data.cpu.clock_mhz > 0.0 && data.cpu.clock_mhz.is_finite(),
            "freq_source={} 却给出非正频率 {}（伪造/缺失）",
            data.cpu.freq_source,
            data.cpu.clock_mhz
        ),
        other => panic!("freq_source 越界（契约 §7）: {other}"),
    }

    // ---- CPU 温度 ----
    match data.cpu.temp_source.as_str() {
        "none" => assert_eq!(
            data.cpu.temperature, 0.0,
            "temp_source=none 却给出了温度 {}（伪造）",
            data.cpu.temperature
        ),
        "lhm" => assert!(
            data.cpu.temperature > 0.0 && data.cpu.temperature.is_finite(),
            "temp_source=lhm 却给出非正温度 {}",
            data.cpu.temperature
        ),
        other => panic!("temp_source 越界（契约 §7）: {other}"),
    }

    // ---- CPU 功耗 ----
    match data.cpu.power_source.as_str() {
        "none" => {
            assert_eq!(
                data.cpu.power_w, 0.0,
                "power_source=none 却给出了功耗 {}（伪造）",
                data.cpu.power_w
            );
            assert!(
                !data.cpu.power_estimated,
                "power_source=none 不得标记 power_estimated"
            );
        }
        "lhm" => {
            assert!(
                data.cpu.power_w > 0.0 && data.cpu.power_w.is_finite(),
                "power_source=lhm 却给出非正功耗 {}",
                data.cpu.power_w
            );
            assert!(
                !data.cpu.power_estimated,
                "lhm 实测功耗不得标记为估算（前端不显示角标）"
            );
        }
        "estimated" => {
            assert!(
                data.cpu.power_w > 0.0 && data.cpu.power_w.is_finite(),
                "power_source=estimated 却给出非正功耗 {}",
                data.cpu.power_w
            );
            assert!(
                data.cpu.power_estimated,
                "estimated 功耗必须标记 power_estimated=true（前端显示「估算」角标）"
            );
        }
        other => panic!("power_source 越界（契约 §7）: {other}"),
    }

    // ---- GPU：条目本身不得是伪造占位（全 0 且无名称） ----
    for g in &data.gpu {
        assert!(!g.name.is_empty(), "GPU 名称不应为空: {:?}", g);
        for (label, v) in [
            ("usage", g.usage),
            ("temperature", g.temperature),
            ("clock_mhz", g.clock_mhz),
            ("power_w", g.power_w),
        ] {
            assert!(
                v.is_finite() && v >= 0.0,
                "GPU {} 的 {label} 非法: {v}",
                g.name
            );
        }
        assert!(
            g.memory_used <= g.memory_total || g.memory_total == 0,
            "GPU {} 显存已用 > 总量: {} > {}",
            g.name,
            g.memory_used,
            g.memory_total
        );
    }

    // ---- 内存 ----
    assert!(
        data.memory.usage_percent.is_finite() && (0.0..=100.0).contains(&data.memory.usage_percent),
        "内存使用率非法: {}",
        data.memory.usage_percent
    );
    assert!(
        data.memory.used <= data.memory.total,
        "内存已用 > 总量: {} > {}",
        data.memory.used,
        data.memory.total
    );

    // ---- 磁盘 ----
    for d in &data.disks {
        assert!(d.read_mbps.is_finite() && d.read_mbps >= 0.0, "磁盘读速度非法");
        assert!(d.write_mbps.is_finite() && d.write_mbps >= 0.0, "磁盘写速度非法");
        assert!(d.used_space <= d.total_space, "磁盘已用 > 总量: {}", d.name);
    }

    // ---- 主板（tier-2）：有则结构完整，无则必须为 None，不允许"空壳"占位 ----
    if let Some(mb) = &data.motherboard {
        for s in &mb.sensors {
            assert!(!s.name.is_empty(), "主板传感器名称不应为空");
            assert!(
                ["temperature", "fan", "voltage"].contains(&s.kind.as_str()),
                "主板传感器类型越界（契约 §3.1）: {}",
                s.kind
            );
            assert!(s.value.is_finite(), "主板传感器 {} 数值非法", s.name);
        }
    }
}

#[test]
fn no_fabricated_zeros() {
    let data = with_sidecar_absent(secm_lib::sensor::get_sensor_data);
    assert_no_fabricated_zeros(&data);
}

// ============================================================================
// 3. `*_source` 枚举收窄（CONTRACT-v3.md §7）
// ============================================================================

#[test]
fn source_enums_are_narrowed() {
    let data = with_sidecar_absent(secm_lib::sensor::get_sensor_data);

    for (field, value, allowed) in [
        ("freq_source", data.cpu.freq_source.as_str(), &FREQ_SOURCES[..]),
        ("temp_source", data.cpu.temp_source.as_str(), &TEMP_SOURCES[..]),
        ("power_source", data.cpu.power_source.as_str(), &POWER_SOURCES[..]),
    ] {
        assert!(
            allowed.contains(&value),
            "cpu.{field} 越界: {value}（允许: {allowed:?}）"
        );
        for forbidden in FORBIDDEN_SOURCES {
            assert_ne!(
                value, forbidden,
                "cpu.{field} 出现契约 §7 已删除的取值 {forbidden}"
            );
        }
    }

    // 旧取值必须不可能：显式覆盖四个字段的完整组合而不是只测当前值
    for f in FORBIDDEN_SOURCES {
        assert!(!FREQ_SOURCES.contains(&f), "freq_source 集合混入 {f}");
        assert!(!TEMP_SOURCES.contains(&f), "temp_source 集合混入 {f}");
        assert!(!POWER_SOURCES.contains(&f), "power_source 集合混入 {f}");
    }
    // power_source 中 `rapl` 也不复存在（v3 仅 lhm/estimated/none）
    assert!(!POWER_SOURCES.contains(&"rapl"), "power_source 不应包含 rapl");
}

#[test]
fn source_enum_values_round_trip_through_json() {
    // 枚举收窄后仍必须原样序列化（前端按字符串比较）
    let data = with_sidecar_absent(secm_lib::sensor::get_sensor_data);
    let json = serde_json::to_value(&data).expect("SensorData 应可序列化为 JSON");
    let cpu = json.get("cpu").expect("cpu 字段缺失");
    for field in ["freq_source", "temp_source", "power_source"] {
        let v = cpu
            .get(field)
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("cpu.{field} 缺失或非字符串: {cpu}"));
        assert!(!v.is_empty(), "cpu.{field} 不应为空串");
    }
    assert_eq!(
        cpu.get("power_estimated").and_then(|v| v.as_bool()),
        Some(data.cpu.power_estimated),
        "power_estimated 必须序列化为 JSON 布尔值"
    );
}

// ============================================================================
// 4. 对外 JSON 字段名不变（前端 Dashboard 依赖）
// ============================================================================

#[test]
fn serialized_json_field_names_unchanged() {
    let data = with_sidecar_absent(secm_lib::sensor::get_sensor_data);
    let json = serde_json::to_value(&data).expect("SensorData 应可序列化为 JSON");
    let obj = json.as_object().expect("SensorData 应序列化为 JSON 对象");

    // 顶层字段：契约 §7 的六个字段必须存在且名称不变（前端 Dashboard 直接消费）。
    // 允许 task-2 追加 tier-2 权限状态三元组（CONTRACT-v3 §3 的 elevated /
    // tier2_available / tier2_reason）——前端已将其声明为可选字段并容忍 undefined，
    // 因此这里断言"必需字段齐全 + 无意外字段"，而不是严格等集，避免把
    // 契约允许的追加误判为字段漂移。
    let required_top = ["cpu", "gpu", "memory", "disks", "motherboard", "diag"];
    for key in required_top {
        assert!(obj.contains_key(key), "SensorData 缺少必需字段: {key}");
    }
    let optional_top = ["elevated", "tier2_available", "tier2_reason"];
    for key in obj.keys() {
        assert!(
            required_top.contains(&key.as_str()) || optional_top.contains(&key.as_str()),
            "SensorData 出现非契约顶层字段（疑似重命名/泄漏）: {key}"
        );
    }

    // cpu 字段（§7：字段名保持不变）
    let cpu = obj["cpu"].as_object().expect("cpu 应为对象");
    for key in [
        "usage",
        "per_core",
        "core_count",
        "clock_mhz",
        "temperature",
        "power_w",
        "freq_source",
        "temp_source",
        "power_source",
        "power_estimated",
    ] {
        assert!(cpu.contains_key(key), "cpu 缺少字段: {key}");
    }
    // CPU 名称与前端频率诊断卡共用的字段
    assert!(cpu.contains_key("name"), "cpu 缺少字段: name");

    // gpu / disks / memory 的形状
    assert!(obj["gpu"].is_array(), "gpu 应为数组");
    assert!(obj["disks"].is_array(), "disks 应为数组");
    assert!(obj["diag"].is_string(), "diag 应为字符串");

    let mem = obj["memory"].as_object().expect("memory 应为对象");
    for key in ["total", "used", "available", "usage_percent", "model_name"] {
        assert!(mem.contains_key(key), "memory 缺少字段: {key}");
    }

    if let Some(gpu0) = obj["gpu"].as_array().and_then(|a| a.first()).and_then(|g| g.as_object()) {
        for key in [
            "name",
            "usage",
            "temperature",
            "memory_used",
            "memory_total",
            "clock_mhz",
            "power_w",
        ] {
            assert!(gpu0.contains_key(key), "gpu[] 缺少字段: {key}");
        }
    }
    if let Some(disk0) = obj["disks"].as_array().and_then(|a| a.first()).and_then(|d| d.as_object()) {
        for key in [
            "name",
            "total_space",
            "available_space",
            "used_space",
            "usage_percent",
            "read_mbps",
            "write_mbps",
        ] {
            assert!(disk0.contains_key(key), "disks[] 缺少字段: {key}");
        }
    }
}

// ============================================================================
// 5. 已删除的旧硬件栈不得回归
// ============================================================================

#[test]
fn removed_hardware_stack_is_absent_from_snapshot() {
    let data = with_sidecar_absent(secm_lib::sensor::get_sensor_data);
    let json = serde_json::to_value(&data).expect("SensorData 应可序列化");
    let obj = json.as_object().expect("SensorData 应序列化为 JSON 对象");

    // 旧 hardware.rs 契约的顶层字段名（gpus/cpus）不得回归
    for ghost_key in ["gpus", "cpus", "sensors"] {
        assert!(
            !obj.contains_key(ghost_key),
            "顶层出现已删除 hardware.rs 的字段名: {ghost_key}"
        );
    }

    // 旧 HTTP 服务端口与错误码不得再出现在任何对外输出中
    let text = serde_json::to_string(&data).expect("SensorData 应可序列化");
    for ghost in [
        "17860",
        "HW_SENSOR_NOT_FOUND",
        "HW_ACCESS_DENIED",
        "HW_INTERNAL_ERROR",
        "HW_NOT_FOUND",
        "HW_METHOD_NOT_ALLOWED",
    ] {
        assert!(
            !text.contains(ghost),
            "对外 JSON 出现已删除硬件栈的残留标记 {ghost}: {text}"
        );
    }
}
