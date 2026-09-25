//! 冷启动延迟回归测试（Bug 1：硬件信息页初始化过久）。
//!
//! 症状（用户可见）：应用冷启动后，"硬件信息"页长时间停在骨架屏
//! 「正在加载硬件信息，首次采集需要预热性能计数器…」。生产日志实测：
//!   - 第 1 次 `get_sensor_data` 阻塞 **4073ms** 才返回，且返回的是
//!     `available:false` 的**空占位快照**（freq=0/none、GPU=none、temp=0）；
//!   - 第 1 份**含实质数据**的快照出现在应用启动后 **~12.1s**
//!     （`hasSubstantialData()` 为真的时刻）。
//!
//! 本测试把该症状固化为可测量的判据，断言两条：
//!   1. **首帧不阻塞**：第 1 次 `get_sensor_data()` 不得把 sidecar 冷启动
//!      （.NET 运行时启动 + 健康检查握手 + LHM 硬件树打开）串行阻塞在请求路径上；
//!   2. **出数及时**：从首次调用起，必须在预算内拿到**含实质数据**的快照
//!      （sidecar 侧信号：`gpu.len() > 0 || cpu.clock_mhz > 0 || cpu.temperature > 0`；
//!       前端 `hasSubstantialData` 另含 tier-1 首帧信号，见 `has_substantial` 注释）。
//!
//! 预热接缝（`lhm::warm_up`，本 Bug 的**主修复**）另有独立用例
//! `prewarm_removes_handshake_from_first_request_path`：它用**裸 TCP 探针**
//! 观察预热线程是否把 sidecar 拉起，而不是用 `get_sensor_data()` 轮询——
//! 后者会由测试自身触发 lazy spawn，使断言退化为同义反复、无法守护主修复。
//!
//! 真实 sidecar：本测试**必须**跑真实 `LhmSidecar.exe`（PDH/LHM 的行为无法用
//! mock 复刻，mock 只会让测试变成同义反复）。定位方式复用产品既有接缝
//! `SECM_LHM_DIR`（`lhm.rs::resolve_lhm_exe` 分支 1，仅 debug 构建生效）。
//! 端口用 `SECM_LHM_PORT` 指向一个专用高位端口，避免与开发机上常驻的
//! sidecar（45980）互相污染——那会让"冷启动"变成"连上一个热进程"，测试失去意义。
//!
//! 运行：`cd src-tauri; cargo test --test hardware_coldstart_latency -- --nocapture`
//!
//! 若 `LhmSidecar.exe` 不存在（未执行 `npm run stage:lhm` / 未 publish），
//! 测试显式 **skip**（打印原因并返回）而非静默通过——否则本护栏会在 CI 上
//! 变成永远绿的装饰品。

use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use secm_lib::sensor::SensorData;

// ============================================================================
// 配置
// ============================================================================

/// 专用测试端口：避开 45980/45981（真实 sidecar）与其它测试端口段。
const TEST_PORT: u16 = 46195;

/// 第 1 次 `get_sensor_data()` 的延迟预算（毫秒）。
///
/// 现状（修复前）：≈4000ms——`ensure_running → spawn_sidecar` 在请求路径里
/// 同步等待 `/health`（每次探测因 ureq 不约束连接超时 ≈2s + 1s 粒度重试；
/// 端口始终起不来时更糟，实测旧实现最坏 ≈16s）。
/// 修复后：请求路径不再同步等待握手，但**仍需付 sidecar 进程创建成本**
/// （.NET `CreateProcess`），实测 748–1019ms（本测试不调用 `warm_up()`，
/// 因此首帧就是完整的 spawn 路径）。
///
/// 取值说明：本预算必须覆盖无法被修复消除的 `CreateProcess` 成本（冷机可达
/// ~1s，杀软扫描时更久）。验证机复现过"给 sidecar 加 1s 启动延迟即 3/3 判红"
/// 的抖动，故从 1500ms 提到 2500ms。判红能力不受影响：模拟修复前实现时
/// 首帧实测 4117ms，稳定超过 2500ms。
const FIRST_CALL_BUDGET_MS: u128 = 2500;

/// 从首次调用起，到拿到**首份含实质数据快照**的预算（毫秒）。
///
/// 这部分由 sidecar 内部预热支配（LHM 硬件树打开 + PDH 双采样），SECM 侧无法消除。
/// 本机实测该阶段方差很大：冷机 8.4s，热机 4.0s（同一台机器、同一 sidecar）。
/// 预算取 12000ms 作为**收敛上界**而非判红阈值——它只回答"数据最终有没有出来"，
/// 避免把 sidecar 内部预热抖动误判为 SECM 回归。
///
/// 真正用于判红的是 `FIRST_CALL_BUDGET_MS`：修复前它稳定红（≈4.0s），
/// 修复后绿（≈0.8s），且不受 sidecar 内部预热方差影响。
///
/// 注：预热接缝（`warm_up`）的回归用例在独立文件 `lhm_prewarm.rs`——
/// `lhm.rs` 状态是进程级单例，同进程内两个用例会互相污染（前一个留下的
/// `ready=true` 会让 `ensure_running` 直接返回，掩盖预热是否真的生效）。
const FIRST_USABLE_BUDGET_MS: u128 = 12000;

/// 测试串行化：`lhm.rs` 的进程状态是全局单例，同进程内多个用例会互相污染。
static SERIAL: Mutex<()> = Mutex::new(());

// ============================================================================
// 环境护栏
// ============================================================================

/// 定位产品实际使用的 `LhmSidecar.exe`（注意目录名是 `lhm`，非 `lhm-sidecar`）。
fn resolve_sidecar_dir() -> Option<std::path::PathBuf> {
    let candidates = [
        // 开发运行布局（与 lhm.rs::resolve_lhm_exe 分支 4 同源）
        std::path::PathBuf::from("resources")
            .join("lhm")
            .join("publish"),
        // 从 src-tauri 目录运行
        std::path::PathBuf::from("src-tauri")
            .join("resources")
            .join("lhm")
            .join("publish"),
    ];
    candidates
        .into_iter()
        .find(|p| p.join("LhmSidecar.exe").exists())
        .and_then(|p| p.canonicalize().ok())
}

/// 环境变量护栏：退出作用域（含 panic）时恢复原值。
struct EnvGuard(&'static str, Option<String>);

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let old = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self(key, old)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.1 {
            Some(v) => std::env::set_var(self.0, v),
            None => std::env::remove_var(self.0),
        }
    }
}

/// 杀掉监听 `port` 的进程（只杀本测试 spawn 的 sidecar，绝不 `taskkill /IM`
/// 全量清理——那会误杀用户正在运行的应用所持有的 sidecar）。
fn kill_listener(port: u16) {
    let out = match Command::new("netstat").args(["-ano"]).output() {
        Ok(o) => o,
        Err(_) => return,
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let needle = format!(":{}", port);
    let mut pids: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.contains("LISTENING") && line.contains(&needle) {
            if let Some(pid) = line.split_whitespace().last() {
                if pid.chars().all(|c| c.is_ascii_digit()) && pid != "0" {
                    pids.push(pid.to_string());
                }
            }
        }
    }
    pids.sort();
    pids.dedup();
    for pid in pids {
        let _ = Command::new("taskkill").args(["/PID", &pid, "/F"]).output();
    }
}

/// 判据（与前端 `Dashboard.tsx::hasSubstantialData` 的**sidecar 侧**信号一致）：
/// `gpu.len() > 0 || cpu.clock_mhz > 0 || cpu.temperature > 0`。
///
/// 注意：前端该函数另含 `memory.total > 0 || core_count > 0` 两个 tier-1 首帧信号
/// （用于**尽早结束骨架屏**，见该函数注释）。本测试刻意**不**纳入它们：
/// 这里要观察的是"sidecar 的 PDH 预热何时出数"这一后端事实，而非前端骨架屏时机。
fn has_substantial(d: &SensorData) -> bool {
    !d.gpu.is_empty() || d.cpu.clock_mhz > 0.0 || d.cpu.temperature > 0.0
}

// ============================================================================
// 回归用例
// ============================================================================

/// 冷启动：首帧不得阻塞在 sidecar 握手上，且必须在预算内出数。
#[test]
fn cold_start_first_frame_is_not_blocked_by_sidecar_handshake() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());

    let Some(dir) = resolve_sidecar_dir() else {
        eprintln!(
            "[skip] 未找到 LhmSidecar.exe（期望 resources/lhm/publish/）。\
             先执行 sidecar 发布（scripts/stage-lhm.mjs / dotnet publish）再运行本测试。"
        );
        return;
    };

    kill_listener(TEST_PORT);

    // 复用产品既有调试接缝：指向真实 sidecar + 专用端口（避免连上常驻热进程）
    let _dir_guard = EnvGuard::set("SECM_LHM_DIR", &dir.to_string_lossy());
    let _port_guard = EnvGuard::set("SECM_LHM_PORT", &TEST_PORT.to_string());

    let t0 = Instant::now();

    // ---- 判据 1：第 1 次调用不得同步等待 sidecar 冷启动 ----
    let first = secm_lib::sensor::get_sensor_data();
    let first_ms = t0.elapsed().as_millis();
    eprintln!(
        "[hw-coldstart] 第 1 次 get_sensor_data: {}ms | freq={:.0}MHz({}) gpu={} temp={:.1}℃ | diag={}",
        first_ms,
        first.cpu.clock_mhz,
        first.cpu.freq_source,
        first.gpu.len(),
        first.cpu.temperature,
        first.diag
    );

    // ---- 判据 2：轮询直到首份含实质数据的快照 ----
    let mut usable_ms: Option<u128> = None;
    let mut polls = 0u32;
    while t0.elapsed() < Duration::from_millis(FIRST_USABLE_BUDGET_MS as u64) {
        polls += 1;
        let d = secm_lib::sensor::get_sensor_data();
        if has_substantial(&d) {
            usable_ms = Some(t0.elapsed().as_millis());
            eprintln!(
                "[hw-coldstart] 首份实质数据: {}ms（第 {} 次轮询）| freq={:.0}MHz({}) gpu={} temp={:.1}℃",
                usable_ms.unwrap(),
                polls,
                d.cpu.clock_mhz,
                d.cpu.freq_source,
                d.gpu.len(),
                d.cpu.temperature
            );
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    kill_listener(TEST_PORT);

    assert!(
        first_ms <= FIRST_CALL_BUDGET_MS,
        "第 1 次 get_sensor_data 阻塞了 {first_ms}ms（预算 {FIRST_CALL_BUDGET_MS}ms）——\
         sidecar 冷启动（.NET 启动 + 健康检查握手 + LHM 开树）被串行阻塞在请求路径上，\
         用户会先白等这段时间且拿到的是 available:false 空占位快照。\
         首帧应只触发启动、不同步等待握手。diag={}",
        first.diag
    );

    let usable = usable_ms.unwrap_or_else(|| {
        panic!(
            "{}ms 内未取到含实质数据的快照（轮询 {} 次，diag={}）",
            FIRST_USABLE_BUDGET_MS, polls, first.diag
        )
    });
    assert!(
        usable <= FIRST_USABLE_BUDGET_MS,
        "首份含实质数据快照耗时 {usable}ms（预算 {FIRST_USABLE_BUDGET_MS}ms）"
    );
}
