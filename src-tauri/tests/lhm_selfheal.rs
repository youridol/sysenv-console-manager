//! sidecar 自愈回归测试（v1.21.2 缺陷 2 修复）。
//!
//! 症状（用户可见）：硬件信息页长期 `温度源: none | 功耗源: none | 频率源: none`、
//! `TEMP=n/a PWR=n/a mb=n/a`，`diag` 先是 `sidecar 启动中（退避期内）`，随后变为
//! `sidecar 服务不可达已超过重启上限（N 次），降级` 并整场会话不再恢复。
//!
//! 根因：`lhm.rs` 的状态机把两种状态当成会话终态——`lhm_snapshot` 在连续 3 次
//! 轮询失败后置 `degraded` 并永久短路；`ensure_running` 的 E 分支在
//! `restarts > LHM_MAX_RESTARTS` 后只返回可重试错误且从不重置计数。实测日志
//! （`%APPDATA%\SECM\logs\2026-09-22.log`）中该计数从 2 一路涨到 13。
//!
//! 本用例的判据是**崩溃恢复契约**：sidecar 被外部杀死后，持续轮询必须能让
//! SECM 自行把服务拉回来，而不是永久停在没有数据的状态。用真实 sidecar +
//! 专用端口，不依赖前端（前端在窗口不可见时会被浏览器节流，无法驱动轮询）。
//!
//! 单独成文件的原因与 `lhm_prewarm.rs` 一致：`lhm` 的进程状态是进程级单例
//! （`OnceLock<Mutex<LhmState>>`），同文件内的用例会相互污染；每个集成测试
//! 文件是独立二进制，因此本用例在自己的进程里拿到干净的单例状态。
//!
//! 运行：`cd src-tauri; cargo test --test lhm_selfheal -- --nocapture`
//! 若 `LhmSidecar.exe` 不存在则显式 skip（不静默通过）。

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::process::Command;
use std::time::{Duration, Instant};

/// 专用测试端口：避开 45980/45981（真实 sidecar）与 lhm_prewarm 的 46194。
const TEST_PORT: u16 = 46196;

/// 等待 sidecar 首次就绪的窗口（毫秒）。
const START_SETTLE_MS: u64 = 20000;

/// 杀进程后等待自愈的窗口（毫秒）。
///
/// 修复后的路径代价：一次退避（`LHM_SPAWN_BACKOFF` = 10s）+ sidecar 冷启动
/// （实测约 0.6s）。取 60s 覆盖"首批轮询正好落在退避窗口内"与冷机余量；
/// 关键是**必须远小于**修复前的"永久不恢复"。
const RECOVERY_WINDOW_MS: u64 = 60000;

/// 轮询间隔：贴近前端 1s 的传感器轮询节奏。
const POLL_INTERVAL: Duration = Duration::from_millis(1000);

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

/// 裸 TCP 探针：不经过 `lhm` 状态机，纯粹观察端口是否有监听。
fn port_up(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok()
}

/// 自愈核心：sidecar 被杀死后，持续轮询必须让它恢复，而不是永久无数据。
#[test]
fn sidecar_recovers_after_being_killed() {
    let Some(dir) = resolve_sidecar_dir() else {
        eprintln!(
            "[skip] 未找到 LhmSidecar.exe（期望 resources/lhm/publish/）。\
             先执行 sidecar 发布（scripts/npm lhm:publish）再运行本测试。"
        );
        return;
    };

    kill_listener(TEST_PORT);

    // 复用产品既有调试接缝：指向真实 sidecar + 专用端口
    let _dir_guard = EnvGuard::set("SECM_LHM_DIR", &dir.to_string_lossy());
    let _port_guard = EnvGuard::set("SECM_LHM_PORT", &TEST_PORT.to_string());

    assert!(
        !port_up(TEST_PORT),
        "前置失败：端口 {} 测试前已有监听，无法区分 sidecar 来源。\
         请确认没有其他进程占用该端口。",
        TEST_PORT
    );

    // ---- 1. 先让 SECM 把 sidecar 拉起来 ----
    let start_deadline = Instant::now() + Duration::from_millis(START_SETTLE_MS);
    let mut started = false;
    while Instant::now() < start_deadline {
        let (snap, _diag) = secm_lib::lhm::lhm_snapshot();
        if snap.is_some() || port_up(TEST_PORT) {
            started = true;
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    assert!(
        started,
        "前置失败：{}ms 内 SECM 未能启动 sidecar（端口 {} 始终无监听）",
        START_SETTLE_MS, TEST_PORT
    );
    eprintln!("[lhm-selfheal] 初始启动成功，端口 {} 已监听", TEST_PORT);

    // ---- 2. 外部杀死 sidecar，模拟崩溃 ----
    kill_listener(TEST_PORT);
    assert!(
        !port_up(TEST_PORT),
        "前置失败：杀死 sidecar 后端口 {} 仍被监听",
        TEST_PORT
    );
    eprintln!("[lhm-selfheal] 已杀死 sidecar，开始观察自愈（窗口 {}ms）", RECOVERY_WINDOW_MS);

    // ---- 3. 持续轮询，等待自愈 ----
    let recovery_deadline = Instant::now() + Duration::from_millis(RECOVERY_WINDOW_MS);
    let mut recovered_at: Option<Duration> = None;
    let mut last_diag = String::new();
    let began = Instant::now();
    while Instant::now() < recovery_deadline {
        let (snap, diag) = secm_lib::lhm::lhm_snapshot();
        last_diag = diag;
        if snap.is_some() {
            recovered_at = Some(began.elapsed());
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    kill_listener(TEST_PORT);

    match recovered_at {
        Some(elapsed) => eprintln!(
            "[lhm-selfheal] 自愈成功：杀进程后 {}ms 恢复服务（最后诊断: {}）",
            elapsed.as_millis(),
            last_diag
        ),
        None => panic!(
            "自愈失败：杀进程后 {}ms 内 sidecar 始终未恢复（端口 {} 无监听）。\
             最后诊断: {}。这是 v1.21.2 修复的回归——状态机把降级/重启预算耗尽\
             当成了会话终态，用户必须重启应用才能恢复数据。",
            RECOVERY_WINDOW_MS, TEST_PORT, last_diag
        ),
    }
}
