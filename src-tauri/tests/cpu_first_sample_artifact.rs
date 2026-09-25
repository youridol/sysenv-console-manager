//! Verifies the sysinfo first-sample 100% artifact and that the new
//! `usage_ready` flag correctly marks it, so the frontend can avoid showing
//! a bogus "CPU 100%".
//!
//! Run: cargo test --test cpu_first_sample_artifact -- --nocapture
//!
//! 端口隔离：`get_sensor_data()` 会经 `lhm_snapshot()` 拉起 sidecar。本测试把它
//! 指到专用端口（`SECM_LHM_PORT`），结束时杀掉自己启动的监听者——既不干扰开发机
//! 上常驻的真实 sidecar，也避免遗留孤儿进程占住 cargo 的 stdout 管道
//! （那会让 `cargo test` 本身挂住不返回）。

use std::process::Command;
use std::time::Duration;

/// 专用测试端口：避开 45980/45981（真实 sidecar）与其它测试端口段。
const TEST_PORT: u16 = 46193;

/// 杀掉监听 `port` 的进程（只杀本测试 spawn 的 sidecar，绝不全量 taskkill）。
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

#[test]
fn first_sample_is_100_percent_and_usage_ready_flags_it() {
    // 隔离到专用端口：不干扰真实 sidecar，且结束时可精确清理
    let _port_guard = EnvGuard::set("SECM_LHM_PORT", &TEST_PORT.to_string());

    // Snapshot 1: should report the bogus 100% but be marked NOT ready.
    let first = secm_lib::sensor::get_sensor_data();
    eprintln!(
        "[cpu-artifact] #1 usage={:.2}% per_core_len={} usage_ready={}",
        first.cpu.usage,
        first.cpu.per_core.len(),
        first.cpu.usage_ready
    );

    assert!(
        !first.cpu.usage_ready,
        "first snapshot must be flagged usage_ready=false (got {:?}); \
         the frontend uses this to withhold the bogus first-sample value",
        first.cpu.usage_ready
    );

    // Document the underlying artifact: assert the tell-tale 100% so that if
    // sysinfo ever changes behaviour this test reports that the workaround is
    // moot, rather than silently passing for the wrong reason.
    assert!(
        (first.cpu.usage - 100.0).abs() < f32::EPSILON,
        "expected the known first-sample artifact (100.0), got {}. \
         If sysinfo fixed this, the usage_ready workaround can be revisited.",
        first.cpu.usage
    );

    // Snapshot 2: must be real and marked ready.
    std::thread::sleep(Duration::from_millis(300));
    let second = secm_lib::sensor::get_sensor_data();
    eprintln!(
        "[cpu-artifact] #2 usage={:.2}% usage_ready={}",
        second.cpu.usage, second.cpu.usage_ready
    );

    assert!(
        second.cpu.usage_ready,
        "second snapshot must be usage_ready=true"
    );
    assert!(
        (0.0..=100.0).contains(&second.cpu.usage),
        "second sample out of range: {}",
        second.cpu.usage
    );

    // The JSON the frontend receives must carry the field.
    let json = serde_json::to_string(&second).expect("serialize SensorData");
    assert!(
        json.contains("\"usage_ready\":true"),
        "SensorData JSON must expose usage_ready for the frontend"
    );

    // Clean up the sidecar this test caused to be spawned (see module docs).
    kill_listener(TEST_PORT);
}
