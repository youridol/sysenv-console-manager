//! 预热接缝回归测试（Bug 1 主修复：`lib.rs setup → lhm::warm_up`）。
//!
//! 单独成文件的原因：`lhm.rs` 的进程状态是**进程级单例**
//! （`OnceLock<Mutex<LhmState>>`）。同文件内的多个用例共享该单例，前一个用例
//! 结束时遗留的 `ready=true` 会让 `ensure_running` 的 B 分支直接返回，
//! 从而掩盖"预热是否真的把 sidecar 拉起来"。每个集成测试文件是独立二进制，
//! 因此本用例在自己的进程里跑，拿到干净的单例状态。
//!
//! 症状（用户可见）：应用冷启动后，"硬件信息"页长时间停在骨架屏。
//! 生产日志实测第 1 次 `get_sensor_data` 阻塞 4073ms（其余会话 4030–4189ms），
//! 且返回 `available:false` 空占位快照；第 2 次起仅 1ms。
//!
//! 主修复：`setup()` 里 `warm_up()` 起后台线程，复用同一条 `lhm_snapshot()`
//! 把 sidecar 的 .NET 冷启动 + 握手 + LHM 开树移出请求路径。
//!
//! **判据为什么必须用裸 TCP 探针**：若用 `get_sensor_data()` 轮询等待"请求变快"，
//! 那个轮询本身就是 lazy spawn 的触发者——测试自己把 sidecar 拉起来了，于是即便
//! 把 `warm_up()` 整个删掉，超时后的计时请求仍是热的，用例恒绿（同义反复）。
//! 这里改为直接观察端口：只有预热线程会启动 sidecar，删掉 `warm_up()` 则端口
//! 始终无监听 → 判红。
//!
//! 运行：`cd src-tauri; cargo test --test lhm_prewarm -- --nocapture`
//! 若 `LhmSidecar.exe` 不存在则显式 skip（不静默通过）。

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::process::Command;
use std::time::{Duration, Instant};

/// 专用测试端口：避开 45980/45981（真实 sidecar）与其它测试端口段。
const TEST_PORT: u16 = 46194;

/// 预热观察窗口（毫秒）。
///
/// 只回答"预热线程最终有没有把 sidecar 拉起来"，不对预热耗时下断言
/// （那由 sidecar 内部 .NET 启动 + PDH 预热支配，非 SECM 契约）。
/// 本机实测 sidecar `/health` 就绪约 566–601ms；取 12s 留足冷机与杀软扫描余量。
const PREWARM_SETTLE_MS: u64 = 12000;

/// 预热完成后单次 `get_sensor_data()` 的上界（毫秒）。
///
/// 断言的是"**不再付 sidecar 握手代价**"，不是"请求是微秒级"：sidecar 刚就绪时
/// 第一次拉取快照仍可能等一次 PDH 采样（本机实测 484ms）。旧路径的握手代价是
/// ≥2s 的 ureq 连接超时 ×重试（实测首个请求 4000+ms），因此 1500ms 足以区分
/// 两种情形，又不会因"首个快照恰好慢一拍"而抖动。
const WARMED_REQUEST_BUDGET_MS: u128 = 1500;

/// 定位产品实际使用的 `LhmSidecar.exe`（目录名是 `lhm`）。
fn resolve_sidecar_dir() -> Option<std::path::PathBuf> {
    [
        std::path::PathBuf::from("resources").join("lhm").join("publish"),
        std::path::PathBuf::from("src-tauri")
            .join("resources")
            .join("lhm")
            .join("publish"),
    ]
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

/// 杀掉监听 `port` 的进程（只杀本测试 spawn 的 sidecar，绝不 taskkill /IM 全量）。
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

/// 裸 TCP 探针：不经过 `lhm` 状态机，纯粹观察端口是否被**预热线程**拉起。
fn port_up(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok()
}

/// `warm_up()` 必须把 sidecar 启动移出请求路径（Bug 1 主修复）。
#[test]
fn prewarm_starts_sidecar_without_any_request() {
    let Some(dir) = resolve_sidecar_dir() else {
        eprintln!(
            "[skip] 未找到 LhmSidecar.exe（期望 resources/lhm/publish/）。\
             先执行 sidecar 发布（scripts/stage-lhm.mjs / dotnet publish）再运行本测试。"
        );
        return;
    };

    kill_listener(TEST_PORT);

    // 复用产品既有调试接缝：指向真实 sidecar + 专用端口
    let _dir_guard = EnvGuard::set("SECM_LHM_DIR", &dir.to_string_lossy());
    let _port_guard = EnvGuard::set("SECM_LHM_PORT", &TEST_PORT.to_string());

    // 前置条件：此刻端口必须无监听（否则本用例无法区分"预热拉起的"与"本来就有的"）
    assert!(
        !port_up(TEST_PORT),
        "前置失败：端口 {} 在预热前已有监听，测试无法区分 sidecar 来源。\
         请确认没有其他进程占用该端口。",
        TEST_PORT
    );

    // 触发预热（与 lib.rs setup 同一调用）——**此过程绝不调用 get_sensor_data()**
    secm_lib::lhm::warm_up();

    let deadline = Instant::now() + Duration::from_millis(PREWARM_SETTLE_MS);
    let mut up = false;
    while Instant::now() < deadline {
        if port_up(TEST_PORT) {
            up = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // 预热完成后，首个请求不应再付握手代价
    let t = Instant::now();
    let _ = secm_lib::sensor::get_sensor_data();
    let warmed_ms = t.elapsed().as_millis();

    kill_listener(TEST_PORT);

    assert!(
        up,
        "预热窗口 {}ms 内 sidecar 端口 {} 始终无监听——`warm_up()` 没有把 sidecar \
         启动起来（主修复缺失/失效）。本判据用裸 TCP 探针观察，未调用 \
         `get_sensor_data()`，因此不会被测试自身的 lazy spawn 掩盖。",
        PREWARM_SETTLE_MS, TEST_PORT
    );

    assert!(
        warmed_ms <= WARMED_REQUEST_BUDGET_MS,
        "预热后单次 get_sensor_data 仍耗时 {warmed_ms}ms（预算 {WARMED_REQUEST_BUDGET_MS}ms）——\
         sidecar 虽已监听，请求路径仍在付握手/等待代价"
    );

    eprintln!(
        "[lhm-prewarm] 预热成功拉起 sidecar；预热后单次请求 {}ms（预算 {}ms）",
        warmed_ms, WARMED_REQUEST_BUDGET_MS
    );
}
