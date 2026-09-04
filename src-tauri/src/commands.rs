/// Tauri 命令处理器 — SECM
///
/// 原则：共性脚手架收拢到宏，差异化的自定义日志保留手动实现。
/// 4 个宏覆盖 ~30 条命令，剩余 ~10 条复杂命令保留详细日志。
use crate::AppState;
use chrono::Local;
use tauri::State;

// ============================================================================
// 命令宏
// ============================================================================

/// 同步命令 — 目标函数返回 Result<T, String>（已有错误类型）
macro_rules! cmd_result {
    ($state:expr, $label:literal, $func:expr) => {{
        let start = Local::now();
        let buf = $state.log_buffer.clone();
        let result = tokio::task::spawn_blocking(move || $func)
            .await
            .map_err(|e| format!("{} panicked: {}", $label, e))?;
        let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
        let ok = result.is_ok();
        buf.append(
            if ok { "Info" } else { "Error" },
            &format!("{}: ok={}, {}ms", $label, ok, elapsed),
        );
        result
    }};
}

/// 同步命令 — 目标函数返回普通值，自动包装为 Ok(value)
macro_rules! cmd_val {
    ($state:expr, $label:literal, $func:expr) => {{
        let start = Local::now();
        let buf = $state.log_buffer.clone();
        let val = tokio::task::spawn_blocking(move || $func)
            .await
            .map_err(|e| format!("{} panicked: {}", $label, e))?;
        let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
        buf.append("Info", &format!("{}: {}ms", $label, elapsed));
        Ok(val)
    }};
}

/// 无状态同步命令 — 目标函数返回普通值，spawn_blocking + 包装为 Ok
macro_rules! cmd_pure {
    ($func:expr) => {{
        let val: _ = tokio::task::spawn_blocking(move || $func)
            .await
            .map_err(|e| format!("Command panicked: {}", e))?;
        Ok(val)
    }};
}

/// 无状态同步命令 — 目标函数已返回 Result<T, String>，仅 spawn_blocking
macro_rules! cmd_pure_result {
    ($func:expr) => {{
        tokio::task::spawn_blocking(move || $func)
            .await
            .map_err(|e| format!("Command panicked: {}", e))?
    }};
}

/// 同步命令 — spawn_blocking + 计时 + 自定义日志 + 包装为 Ok
/// 日志闭包签名: |val: &T, elapsed_ms: i64| -> String
macro_rules! cmd_log {
    ($state:expr, $func:expr, $log_fn:expr) => {{
        let start = Local::now();
        let buf = $state.log_buffer.clone();
        let val = tokio::task::spawn_blocking(move || $func)
            .await
            .map_err(|e| format!("Command panicked: {}", e))?;
        let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
        buf.append("Info", &($log_fn)(&val, elapsed));
        Ok(val)
    }};
}

// ============================================================================
// 日志命令
// ============================================================================


/// 前端日志上报（统一 debug.log：前端 console/logger 的 WARN/ERROR 经此落盘）
///
/// 参数：level（TRACE/DEBUG/INFO/WARN/ERROR）、module（组件名）、message（日志内容）。
/// 校验：level 白名单 + message 长度上限（防滥用），失败静默（不影响前端）。
#[tauri::command]
pub fn append_frontend_log(
    state: State<'_, AppState>,
    level: String,
    module: String,
    message: String,
) -> Result<(), String> {
    let _ = state; // 仅校验+落盘，无需状态
    const MAX_LEN: usize = 4096;
    let level_ok = matches!(
        level.as_str(),
        "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR"
    );
    if !level_ok {
        return Err(format!("非法日志级别: {}", level));
    }
    let module_trim = module.trim();
    let module_ok = !module_trim.is_empty() && module_trim.len() <= 64;
    // 按 UTF-8 字符边界安全截断（message 长度按字节计；直接字节切片会切断多字节
    // 字符导致 panic —— S0 修复：len 超限时用 char_indices 定位安全边界）
    let mut msg = message;
    if msg.len() > MAX_LEN {
        let mut boundary = MAX_LEN;
        while boundary > 0 && !msg.is_char_boundary(boundary) {
            boundary -= 1;
        }
        msg.truncate(boundary);
    }
    crate::debug::append(
        &level,
        if module_ok { module_trim } else { "frontend" },
        &msg,
    );
    Ok(())
}

#[tauri::command]
pub async fn get_logs(state: State<'_, AppState>) -> Result<Vec<crate::log::LogEntry>, String> {
    let start = Local::now();
    let buf = &state.log_buffer;
    let entries = buf.get_all();
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    buf.append(
        "Info",
        &format!("get_logs: {} entries ({}ms)", entries.len(), elapsed),
    );
    Ok(entries)
}

#[tauri::command]
pub async fn clear_logs(state: State<'_, AppState>) -> Result<(), String> {
    let start = Local::now();
    let buf = &state.log_buffer;
    buf.clear();
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    buf.append("Info", &format!("clear_logs ({}ms)", elapsed));
    Ok(())
}

#[tauri::command]
pub async fn export_logs(state: State<'_, AppState>) -> Result<String, String> {
    let start = Local::now();
    let buf = &state.log_buffer;
    let content = buf.export_all_files()?;
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    buf.append(
        "Info",
        &format!("export_logs: {} bytes ({}ms)", content.len(), elapsed),
    );
    Ok(content)
}

// ============================================================================
// 基础信息（纯常量，无状态）
// ============================================================================

#[tauri::command]
pub async fn get_app_version() -> Result<String, String> {
    Ok(env!("CARGO_PKG_VERSION").to_string())
}

#[tauri::command]
pub async fn get_log_capacity() -> Result<usize, String> {
    Ok(crate::log::CAPACITY)
}

#[tauri::command]
pub async fn get_debug_logs() -> Result<Vec<crate::debug::DebugEntry>, String> {
    Ok(crate::debug::get_all())
}

// ============================================================================
// 传感器（特例 — 复杂诊断日志）
// ============================================================================

#[tauri::command]
pub async fn get_sensor_data(
    state: State<'_, AppState>,
) -> Result<crate::sensor::SensorData, String> {
    let start = Local::now();
    let data = tokio::task::spawn_blocking(|| crate::sensor::get_sensor_data())
        .await
        .map_err(|e| format!("get_sensor_data panicked: {}", e))?;
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    let buf = &state.log_buffer;
    let gpu0 = data.gpu.first();
    // 记录到硬盘：图表历史持久化（每秒采样点，跨重启恢复）
    crate::temp_data::chart_append(&format!(
        "{{\"type\":\"hw\",\"t\":\"{}\",\"cpu\":{:.1},\"gpu\":{:.1},\"mem\":{:.1}}}",
        Local::now().format("%Y-%m-%d %H:%M:%S"),
        data.cpu.usage,
        gpu0.map_or(0.0, |g| g.usage),
        data.memory.usage_percent
    ));
    buf.append("Info", &format!(
        "[硬件传感器] CPU={:.1}% | freq={:.0}MHz | temp={:.1}°C | GPU={} | gpu_temp={:.1}°C | gpu_power={:.1}W | Mem={:.1}% | {}ms",
        data.cpu.usage, data.cpu.clock_mhz, data.cpu.temperature,
        gpu0.map_or("none".into(), |g| g.name.clone()),
        gpu0.map_or(0.0, |g| g.temperature), gpu0.map_or(0.0, |g| g.power_w),
        data.memory.usage_percent, elapsed,
    ));
    buf.append("Info", &format!("[硬件传感器诊断] {}", data.diag));
    Ok(data)
}

/// 硬件传感器结构化接口(GPU/CPU 温度/频率/功耗,任务书 JSON 契约)
///
/// 与 HTTP GET /api/hardware/sensors 同一采集链路,供前端 Tauri invoke 直连;
/// 错误返回携带明确错误码(message 含修复建议)。
#[tauri::command]
pub async fn get_hardware_sensors(
    state: State<'_, AppState>,
) -> Result<crate::hardware::HardwareSensorsResponse, String> {
    let start = Local::now();
    let result = tokio::task::spawn_blocking(|| crate::hardware::collect_sensors())
        .await
        .map_err(|e| format!("get_hardware_sensors panicked: {}", e))?;
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    let buf = &state.log_buffer;
    match &result {
        Ok(resp) => {
            let gpu_summary = resp
                .gpus
                .iter()
                .map(|g| {
                    format!(
                        "{} t={} f={} p={}",
                        g.name,
                        g.temperature
                            .map(|v| format!("{:.0}", v))
                            .unwrap_or_else(|| "null".into()),
                        g.frequency
                            .map(|v| format!("{:.0}", v))
                            .unwrap_or_else(|| "null".into()),
                        g.power
                            .map(|v| format!("{:.1}", v))
                            .unwrap_or_else(|| "null".into())
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ");
            buf.append(
                "Info",
                &format!(
                    "[硬件传感器] GPU={} CPU={}核 | {}ms",
                    resp.gpus.len(),
                    resp.cpus.len(),
                    elapsed
                ),
            );
            buf.append("Info", &format!("[硬件传感器] GPU 明细: {}", gpu_summary));
        }
        Err(e) => {
            buf.append(
                "Error",
                &format!("[硬件传感器] 采集失败: code={} message={}", e.code, e.message),
            );
        }
    }
    result.map_err(|e| format!("{} (code={})", e.message, e.code))
}

// ============================================================================
// 清理命令 — 统一 cmd_log! + 闭包格式化
// ============================================================================

#[tauri::command]
pub async fn flush_dns(
    state: State<'_, AppState>,
) -> Result<crate::cleanup::CleanupResult, String> {
    cmd_log!(
        state,
        crate::cleanup::flush_dns(),
        |r: &crate::cleanup::CleanupResult, ms| format!("flush_dns: ok={}, {}ms", r.success, ms)
    )
}

#[tauri::command]
pub async fn clean_shader_cache(
    state: State<'_, AppState>,
) -> Result<crate::cleanup::CleanupResult, String> {
    cmd_log!(
        state,
        crate::cleanup::clean_shader_cache(),
        |r: &crate::cleanup::CleanupResult, ms| format!(
            "clean_shader_cache: ok={}, freed={}, {}ms",
            r.success, r.bytes_freed, ms
        )
    )
}

#[tauri::command]
pub async fn clean_nvidia_cache(
    state: State<'_, AppState>,
) -> Result<crate::cleanup::CleanupResult, String> {
    cmd_log!(
        state,
        crate::cleanup::clean_nvidia_cache(),
        |r: &crate::cleanup::CleanupResult, ms| format!(
            "clean_nvidia_cache: ok={}, freed={}, {}ms",
            r.success, r.bytes_freed, ms
        )
    )
}

#[tauri::command]
pub async fn clean_amd_cache(
    state: State<'_, AppState>,
) -> Result<crate::cleanup::CleanupResult, String> {
    cmd_log!(
        state,
        crate::cleanup::clean_amd_cache(),
        |r: &crate::cleanup::CleanupResult, ms| format!(
            "clean_amd_cache: ok={}, freed={}, {}ms",
            r.success, r.bytes_freed, ms
        )
    )
}

#[tauri::command]
pub async fn clean_directx_cache(
    state: State<'_, AppState>,
) -> Result<crate::cleanup::CleanupResult, String> {
    cmd_log!(
        state,
        crate::cleanup::clean_directx_cache(),
        |r: &crate::cleanup::CleanupResult, ms| format!(
            "clean_directx_cache: ok={}, freed={}, {}ms",
            r.success, r.bytes_freed, ms
        )
    )
}

#[tauri::command]
pub async fn clean_steam_cache(
    state: State<'_, AppState>,
) -> Result<crate::cleanup::CleanupResult, String> {
    cmd_log!(
        state,
        crate::cleanup::clean_steam_cache(),
        |r: &crate::cleanup::CleanupResult, ms| format!(
            "clean_steam_cache: ok={}, freed={}, {}ms",
            r.success, r.bytes_freed, ms
        )
    )
}

#[tauri::command]
pub async fn clean_temp_files(
    state: State<'_, AppState>,
) -> Result<crate::cleanup::CleanupResult, String> {
    cmd_log!(
        state,
        crate::cleanup::clean_temp_files(),
        |r: &crate::cleanup::CleanupResult, ms| format!(
            "clean_temp_files: ok={}, freed={}, {}ms",
            r.success, r.bytes_freed, ms
        )
    )
}

#[tauri::command]
pub async fn trim_process_working_set(
    state: State<'_, AppState>,
) -> Result<crate::cleanup::CleanupResult, String> {
    cmd_log!(
        state,
        crate::cleanup::trim_process_working_set(),
        |r: &crate::cleanup::CleanupResult, ms| format!(
            "trim_process_working_set: ok={}, {}ms",
            r.success, ms
        )
    )
}

#[tauri::command]
pub async fn set_priority(
    state: State<'_, AppState>,
    pid: u32,
    priority: String,
) -> Result<crate::cleanup::CleanupResult, String> {
    let p = priority.clone();
    cmd_log!(
        state,
        crate::cleanup::set_process_priority(pid, &p),
        |r: &crate::cleanup::CleanupResult, ms| format!(
            "set_priority: pid={}, pri={}, ok={}, {}ms",
            pid, priority, r.success, ms
        )
    )
}

#[tauri::command]
pub async fn list_processes(
    state: State<'_, AppState>,
) -> Result<Vec<crate::cleanup::ProcessInfo>, String> {
    cmd_log!(
        state,
        crate::cleanup::list_processes(),
        |v: &Vec<crate::cleanup::ProcessInfo>, ms| format!(
            "list_processes: {} entries ({}ms)",
            v.len(),
            ms
        )
    )
}

// ============================================================================
// 网络流式命令（模式各异，保留原始实现）
// ============================================================================

#[tauri::command]
pub async fn ping_stream(
    channel: tauri::ipc::Channel<crate::network::StreamEvent>,
    state: State<'_, AppState>,
    target: String,
    count: u32,
    interval_ms: u64,
    cmd_id: String,
    continuous: bool,
    ip_version: String,
    proto: String,
    size: u32,
    ttl: u32,
    deadline: u32,
) -> Result<crate::network::PingSummary, String> {
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let t = target.clone();
    let cid = cmd_id.clone();
    let ipv = ip_version.clone();
    let proto2 = proto.clone();
    let emitter = {
        let ch = channel.clone();
        move |ev: crate::network::StreamEvent| {
            let _ = ch.send(ev);
        }
    };

    let result = crate::network::ping_streaming(
        &t,
        count,
        interval_ms,
        &cid,
        continuous,
        &ipv,
        &proto2,
        size,
        ttl,
        deadline,
        emitter,
    )
    .await;

    if let Err(ref e) = result {
        let _ = channel.send(crate::network::StreamEvent {
            kind: "error".into(),
            data: format!("{}", e),
        });
    }
    crate::cancel::clear_cancel(&cid);

    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    match &result {
        Ok(summary) => buf.append(
            "Info",
            &format!(
                "ping {}: sent={} recv={} loss={:.1}% avg={:.1}ms ({}ms)",
                target,
                summary.sent,
                summary.received,
                summary.loss_percent,
                summary.avg_rtt_ms,
                elapsed
            ),
        ),
        Err(e) => buf.append("Error", &format!("ping {}: {} ({}ms)", target, e, elapsed)),
    }
    result
}

#[tauri::command]
pub async fn traceroute_stream(
    channel: tauri::ipc::Channel<crate::network::StreamEvent>,
    state: State<'_, AppState>,
    target: String,
    max_hops: u8,
    cmd_id: String,
    ip_version: String,
    proto: String,
    dns_server: Option<String>,
) -> Result<crate::network::TraceResult, String> {
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let cid = cmd_id.clone();
    let ipv = ip_version.clone();
    let proto2 = proto.clone();
    let dns = dns_server.clone();
    let emitter = {
        let ch = channel.clone();
        move |ev: crate::network::StreamEvent| {
            let _ = ch.send(ev);
        }
    };

    let result = crate::network::traceroute_streaming(
        &target,
        max_hops,
        &cid,
        &ipv,
        &proto2,
        dns.as_deref(),
        emitter,
    )
    .await;

    if let Err(ref e) = result {
        let _ = channel.send(crate::network::StreamEvent {
            kind: "error".into(),
            data: format!("{}", e),
        });
    }
    crate::cancel::clear_cancel(&cid);

    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    match &result {
        Ok(tr) => buf.append(
            "Info",
            &format!(
                "traceroute {}: {} hops ({}ms)",
                target,
                tr.hops.len(),
                elapsed
            ),
        ),
        Err(e) => buf.append(
            "Error",
            &format!("traceroute {}: {} ({}ms)", target, e, elapsed),
        ),
    }
    result
}

#[tauri::command]
pub async fn nslookup_stream(
    channel: tauri::ipc::Channel<crate::network::StreamEvent>,
    state: State<'_, AppState>,
    domain: String,
    record_type: String,
    cmd_id: String,
) -> Result<crate::network::DnsResult, String> {
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let d = domain.clone();
    let rt = record_type.clone();
    let cid_val = cmd_id.clone();
    let cid_ref = cmd_id.clone();
    let emitter = {
        let ch = channel.clone();
        move |ev: crate::network::StreamEvent| {
            let _ = ch.send(ev);
        }
    };

    let result = tokio::task::spawn_blocking(move || {
        crate::network::nslookup_streaming(&d, &rt, &cid_val, emitter)
    })
    .await
    .map_err(|e| {
        let _ = channel.send(crate::network::StreamEvent {
            kind: "error".into(),
            data: format!("Nslookup panicked: {}", e),
        });
        format!("Nslookup panicked: {}", e)
    })?;

    if let Err(ref e) = result {
        let _ = channel.send(crate::network::StreamEvent {
            kind: "error".into(),
            data: format!("{}", e),
        });
    }
    crate::cancel::clear_cancel(&cid_ref);

    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    match &result {
        Ok(dr) => buf.append(
            "Info",
            &format!(
                "nslookup {} ({}): {} records ({}ms)",
                domain,
                record_type,
                dr.records.len(),
                elapsed
            ),
        ),
        Err(e) => buf.append(
            "Error",
            &format!("nslookup {}: {} ({}ms)", domain, e, elapsed),
        ),
    }
    result
}

#[tauri::command]
pub async fn detect_nat_type_stream(
    channel: tauri::ipc::Channel<crate::network::StreamEvent>,
    state: State<'_, AppState>,
    server: String,
    cmd_id: String,
) -> Result<crate::network::NatDetectionResult, String> {
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let s = server.clone();
    let cid_val = cmd_id.clone();
    let cid_ref = cmd_id.clone();
    let emitter = {
        let ch = channel.clone();
        move |ev: crate::network::StreamEvent| {
            let _ = ch.send(ev);
        }
    };

    let result = tokio::task::spawn_blocking(move || {
        crate::network::detect_nat_streaming(&s, &cid_val, emitter)
    })
    .await
    .map_err(|e| {
        let _ = channel.send(crate::network::StreamEvent {
            kind: "error".into(),
            data: format!("NAT detection panicked: {}", e),
        });
        format!("NAT detection panicked: {}", e)
    })?;

    if let Err(ref e) = result {
        let _ = channel.send(crate::network::StreamEvent {
            kind: "error".into(),
            data: format!("{}", e),
        });
    }
    crate::cancel::clear_cancel(&cid_ref);

    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    match &result {
        Ok(nat) => buf.append(
            "Info",
            &format!(
                "detect_nat_type: {} → {} ({}ms)",
                server, nat.nat_label, elapsed
            ),
        ),
        Err(e) => buf.append(
            "Error",
            &format!("detect_nat_type: {} — {} ({}ms)", server, e, elapsed),
        ),
    }
    result
}

#[tauri::command]
pub async fn iperf3_stream(
    channel: tauri::ipc::Channel<crate::network::StreamEvent>,
    state: State<'_, AppState>,
    host: String,
    port: u16,
    duration: u32,
    cmd_id: String,
) -> Result<String, String> {
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let h = host.clone();
    let p = port;
    let d = duration;
    let cid = cmd_id.clone();
    let cid2 = cid.clone();

    let result = tokio::task::spawn_blocking(move || {
        use std::io::BufRead;
        use std::os::windows::process::CommandExt;
        use std::process::Stdio;
        let mut child = std::process::Command::new("iperf3")
            .args([
                "-c",
                &h,
                "-p",
                &p.to_string(),
                "-t",
                &d.to_string(),
                "-f",
                "m",
            ])
            // CREATE_NO_WINDOW：iperf3 持续运行，防止黑窗常驻
            .creation_flags(0x08000000)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("iperf3 未安装或无法启动: {}", e))?;

        let stdout = child.stdout.take().unwrap();
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            if let Ok(l) = line {
                if crate::cancel::is_cancelled(&cid2) {
                    let _ = child.kill();
                    break;
                }
                let _ = channel.send(crate::network::StreamEvent {
                    kind: "info".into(),
                    data: l,
                });
            }
        }
        let output = child.wait_with_output().map_err(|e| format!("{}", e))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stderr).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    })
    .await
    .map_err(|e| format!("iperf3 panicked: {}", e))??;

    crate::cancel::clear_cancel(&cid);
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    buf.append(
        "Info",
        &format!(
            "iperf3 {}:{} duration={}s ({}ms)",
            host, port, duration, elapsed
        ),
    );
    Ok(result)
}

#[tauri::command]
pub async fn cancel_net_cmd(cmd_id: String) -> Result<(), String> {
    crate::cancel::cancel_command(&cmd_id);
    Ok(())
}

// ============================================================================
// 设置命令
// ============================================================================

#[tauri::command]
pub async fn get_hags_state(
    state: State<'_, AppState>,
) -> Result<crate::settings::SettingState, String> {
    cmd_log!(
        state,
        crate::settings::get_hags_state(),
        |s: &crate::settings::SettingState, ms| format!(
            "get_hags_state: enabled={}, msg={}, {}ms",
            s.enabled, s.message, ms
        )
    )
}

#[tauri::command]
pub async fn set_hags_state(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<crate::settings::SettingState, String> {
    cmd_result!(
        state,
        "set_hags_state",
        crate::settings::set_hags_state(enabled)
    )
}

/// 读取 Windows 游戏模式开关状态（HKCU GameBar）
#[tauri::command]
pub async fn get_game_mode_state(
    state: State<'_, AppState>,
) -> Result<crate::settings::SettingState, String> {
    cmd_log!(
        state,
        crate::settings::get_game_mode_state(),
        |s: &crate::settings::SettingState, ms| format!(
            "get_game_mode_state: enabled={}, msg={}, {}ms",
            s.enabled, s.message, ms
        )
    )
}

/// 设置 Windows 游戏模式开关
#[tauri::command]
pub async fn set_game_mode_state(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<crate::settings::SettingState, String> {
    cmd_result!(
        state,
        "set_game_mode_state",
        crate::settings::set_game_mode_state(enabled)
    )
}

#[tauri::command]
pub async fn get_game_optimization_state(
    state: State<'_, AppState>,
) -> Result<crate::settings::SettingState, String> {
    cmd_log!(
        state,
        crate::settings::get_game_optimization_state(),
        |s: &crate::settings::SettingState, ms| format!(
            "get_game_optimization_state: enabled={}, msg={}, {}ms",
            s.enabled, s.message, ms
        )
    )
}

#[tauri::command]
pub async fn set_game_optimization(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<crate::settings::SettingState, String> {
    cmd_result!(
        state,
        "set_game_opt",
        crate::settings::set_game_optimization(enabled)
    )
}

#[tauri::command]
pub async fn get_mouse_precision_state(
    state: State<'_, AppState>,
) -> Result<crate::settings::SettingState, String> {
    cmd_log!(
        state,
        crate::settings::get_mouse_precision_state(),
        |s: &crate::settings::SettingState, ms| format!(
            "get_mouse_precision: enabled={}, msg={}, {}ms",
            s.enabled, s.message, ms
        )
    )
}

#[tauri::command]
pub async fn set_mouse_precision(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<crate::settings::SettingState, String> {
    cmd_result!(
        state,
        "set_mouse_precision",
        crate::settings::set_mouse_precision(enabled)
    )
}

// ============================================================================
// NVIDIA 显卡电源管理模式（NVAPI DRS）
// ============================================================================

/// 读取 NVIDIA 显卡电源管理模式（0=自适应 1=最高性能优先 5=最佳功率）
#[tauri::command]
pub async fn get_nvidia_power_mode(
    state: State<'_, AppState>,
) -> Result<crate::nvidia_drs::NvidiaPowerMode, String> {
    cmd_result!(
        state,
        "get_nvidia_power_mode",
        crate::nvidia_drs::get_power_mode()
    )
}

/// 设置 NVIDIA 显卡电源管理模式
#[tauri::command]
pub async fn set_nvidia_power_mode(
    state: State<'_, AppState>,
    mode: crate::nvidia_drs::NvidiaPowerMode,
) -> Result<(), String> {
    // 记录写入前后值对比（R8 日志完整：设置目标 + 实际结果）
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let before = crate::nvidia_drs::get_power_mode().ok();
    let result = tokio::task::spawn_blocking(move || crate::nvidia_drs::set_power_mode(mode))
        .await
        .map_err(|e| format!("set_nvidia_power_mode panicked: {}", e))?;
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    match (&result, before) {
        (Ok(()), Some(b)) => buf.append(
            "Info",
            &format!(
                "set_nvidia_power_mode: {} → {}, {}ms",
                b.label_cn(),
                mode.label_cn(),
                elapsed
            ),
        ),
        (Ok(()), None) => buf.append(
            "Info",
            &format!(
                "set_nvidia_power_mode: ? → {}, {}ms",
                mode.label_cn(),
                elapsed
            ),
        ),
        (Err(e), _) => buf.append(
            "Error",
            &format!(
                "set_nvidia_power_mode: 设置 {} 失败: {}, {}ms",
                mode.label_cn(),
                e,
                elapsed
            ),
        ),
    }
    result
}

#[tauri::command]
pub async fn get_vrr_state(
    state: State<'_, AppState>,
) -> Result<crate::settings::SettingState, String> {
    cmd_log!(
        state,
        crate::settings::get_vrr_state(),
        |s: &crate::settings::SettingState, ms| format!(
            "get_vrr_state: enabled={}, msg={}, {}ms",
            s.enabled, s.message, ms
        )
    )
}

#[tauri::command]
pub async fn set_vrr_state(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<crate::settings::SettingState, String> {
    cmd_result!(
        state,
        "set_vrr_state",
        crate::settings::set_vrr_state(enabled)
    )
}

#[tauri::command]
pub async fn get_power_plans(
    state: State<'_, AppState>,
) -> Result<Vec<crate::settings::PowerPlan>, String> {
    cmd_result!(state, "get_power_plans", crate::settings::get_power_plans())
}

#[tauri::command]
pub async fn set_power_plan(state: State<'_, AppState>, guid: String) -> Result<(), String> {
    let g = guid.clone();
    cmd_result!(state, "set_power_plan", crate::settings::set_power_plan(&g))
}

#[tauri::command]
pub async fn delete_power_plan(state: State<'_, AppState>, guid: String) -> Result<(), String> {
    let g = guid.clone();
    cmd_result!(state, "delete_power_plan", crate::settings::delete_power_plan(&g))
}

/// 读取活动电源方案下异类线程/短运行线程调度策略（注册表）
#[tauri::command]
pub async fn get_hetero_policies(
    state: State<'_, AppState>,
) -> Result<crate::settings::HeteroPolicies, String> {
    cmd_result!(
        state,
        "get_hetero_policies",
        crate::settings::get_hetero_policies()
    )
}

/// 设置异类线程/短运行线程调度策略（AC/DC 同步，powercfg）
#[tauri::command]
pub async fn set_hetero_policy(
    state: State<'_, AppState>,
    kind: String,
    value: u32,
) -> Result<(), String> {
    let k = kind.clone();
    cmd_result!(
        state,
        "set_hetero_policy",
        crate::settings::set_hetero_policy(&k, value)
    )
}

#[tauri::command]
pub async fn enable_ultimate_performance(state: State<'_, AppState>) -> Result<String, String> {
    cmd_result!(
        state,
        "enable_ultimate_perf",
        crate::settings::enable_ultimate_performance()
    )
}

#[tauri::command]
pub async fn get_services(
    state: State<'_, AppState>,
) -> Result<Vec<crate::settings::ServiceInfo>, String> {
    cmd_val!(state, "get_services", crate::settings::get_services())
}

#[tauri::command]
pub async fn list_all_services(
    state: State<'_, AppState>,
) -> Result<Vec<crate::settings::ServiceInfo>, String> {
    cmd_val!(
        state,
        "list_all_services",
        crate::settings::list_all_services()
    )
}

#[tauri::command]
pub async fn start_service(state: State<'_, AppState>, name: String) -> Result<String, String> {
    let n = name.clone();
    cmd_result!(state, "start_service", crate::settings::start_service(&n))
}

#[tauri::command]
pub async fn stop_service(state: State<'_, AppState>, name: String) -> Result<String, String> {
    let n = name.clone();
    cmd_result!(state, "stop_service", crate::settings::stop_service(&n))
}

#[tauri::command]
pub async fn set_service_start_type(
    state: State<'_, AppState>,
    name: String,
    start_type: String,
) -> Result<String, String> {
    let n = name.clone();
    let t = start_type.clone();
    cmd_result!(
        state,
        "set_service_start_type",
        crate::settings::set_service_start_type(&n, &t)
    )
}

// ============================================================================
// 环境检测（无状态）
// ============================================================================

#[tauri::command]
pub async fn check_directx() -> Result<crate::environment::DirectXInfo, String> {
    cmd_pure!(crate::environment::check_directx())
}

#[tauri::command]
pub async fn check_vc_runtimes() -> Result<crate::environment::VcRuntimeInfo, String> {
    cmd_pure!(crate::environment::check_vc_runtimes())
}

#[tauri::command]
pub async fn check_ai_tools() -> Result<crate::environment::AiToolsInfo, String> {
    cmd_pure!(crate::environment::check_ai_tools())
}

#[tauri::command]
pub async fn install_ai_tool(npm_package: String) -> Result<String, String> {
    cmd_pure_result!(crate::environment::install_or_upgrade_tool(&npm_package))
}

#[tauri::command]
pub async fn fetch_ai_latest_versions() -> Result<Vec<(String, String)>, String> {
    cmd_pure!(crate::environment::fetch_ai_latest_versions())
}

/// npm 运行环境检测（node/npm 版本 + 全局目录 + 包统计）
#[tauri::command]
pub async fn check_npm_environment() -> Result<crate::environment::NpmEnvironment, String> {
    cmd_pure!(crate::environment::check_npm_environment())
}

/// 卸载 AI 工具（npm uninstall -g，白名单校验）
#[tauri::command]
pub async fn uninstall_ai_tool(npm_package: String) -> Result<String, String> {
    cmd_pure_result!(crate::environment::uninstall_ai_tool(&npm_package))
}

/// 列出 MCP 服务器安装状态
#[tauri::command]
pub async fn list_mcp_servers() -> Result<Vec<crate::environment::McpServerInfo>, String> {
    cmd_pure!(crate::environment::list_mcp_servers())
}

/// 安装 MCP 服务器（npm install -g，白名单校验）
#[tauri::command]
pub async fn install_mcp_server(package: String) -> Result<String, String> {
    cmd_pure_result!(crate::environment::install_mcp_server(&package))
}

/// 卸载 MCP 服务器（npm uninstall -g，白名单校验）
#[tauri::command]
pub async fn uninstall_mcp_server(package: String) -> Result<String, String> {
    cmd_pure_result!(crate::environment::uninstall_mcp_server(&package))
}

/// 列出各 AI 工具的 skills / 插件
#[tauri::command]
pub async fn list_extensions() -> Result<Vec<crate::environment::AiExtension>, String> {
    cmd_pure!(crate::environment::list_extensions())
}


#[tauri::command]
pub async fn get_game_presets() -> Result<Vec<crate::game_env::GamePreset>, String> {
    cmd_pure!(crate::game_env::get_game_presets())
}

// ============================================================================
// 系统信息（自定义日志）
// ============================================================================

#[tauri::command]
pub async fn get_net_stats(
    state: State<'_, AppState>,
) -> Result<crate::net_stats::NetStatsData, String> {
    cmd_log!(
        state,
        crate::net_stats::get_net_stats(),
        |r: &crate::net_stats::NetStatsData, ms| {
            // 记录到硬盘：图表历史持久化（每秒采样点，跨重启恢复）
            crate::temp_data::chart_append(&format!(
                "{{\"type\":\"net\",\"t\":\"{}\",\"dl\":{:.1},\"ul\":{:.1}}}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                r.total_download_speed,
                r.total_upload_speed
            ));
            format!(
            "get_net_stats: ifaces={} dl_total={} ul_total={} dl_spd={:.0} ul_spd={:.0} conns={}, {}ms",
            r.interfaces.len(), r.total_download, r.total_upload,
            r.total_download_speed, r.total_upload_speed, r.active_connections, ms)
        }
    )
}

/// 读取图表历史（持久化数据，前端启动时“再一次读取”恢复图表）
#[tauri::command]
pub async fn get_chart_history(limit: usize) -> Result<Vec<String>, String> {
    cmd_pure!(crate::temp_data::chart_load(limit))
}

/// 获取各网卡链路协商速度（准静态数据，前端挂载/网卡集合变化时一次性查询）
#[tauri::command]
pub async fn get_link_speeds(
    state: State<'_, AppState>,
) -> Result<std::collections::HashMap<String, String>, String> {
    cmd_log!(
        state,
        crate::net_stats::get_link_speeds(),
        |r: &std::collections::HashMap<String, String>, ms| format!(
            "get_link_speeds: entries={}, {}ms",
            r.len(),
            ms
        )
    )
}

#[tauri::command]
pub async fn get_ip_info(state: State<'_, AppState>) -> Result<crate::ip_info::IpInfo, String> {
    cmd_log!(
        state,
        crate::ip_info::get_ip_info(),
        |r: &crate::ip_info::IpInfo, ms| format!(
            "get_ip_info: local={} ipv4_pub={} ipv6_pub={} dom={} dom_loc={} city={}, {}ms",
            r.local_ipv4,
            r.public_ipv4,
            r.public_ipv6,
            r.domestic_ip,
            r.domestic_location,
            r.city,
            ms
        )
    )
}

/// 检测本地子网内的 DHCP 服务器（主动 DHCP Discover 探测 + 注册表基线合并）
///
/// 说明：cmd_log! 宏适用于目标函数返回普通值的场景；本命令目标返回
/// Result<DhcpProbeResult, String>，故手动 spawn_blocking + 计时 + 日志
/// （语义等价 cmd_result!，但日志额外携带服务器数）。
#[tauri::command]
pub async fn check_dhcp_servers(
    state: State<'_, AppState>,
) -> Result<crate::dhcp_probe::DhcpProbeResult, String> {
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let result = tokio::task::spawn_blocking(crate::dhcp_probe::probe_dhcp_servers)
        .await
        .map_err(|e| format!("check_dhcp_servers panicked: {}", e))?;
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    let ok = result.is_ok();
    let summary = match &result {
        Ok(r) => format!(
            "servers=[{}] count={} healthy={} baseline={:?}",
            r.servers.join(","),
            r.count,
            r.healthy,
            r.baseline_server
        ),
        Err(e) => format!("error={}", e),
    };
    buf.append(
        if ok { "Info" } else { "Error" },
        &format!("check_dhcp_servers: {} {}ms", summary, elapsed),
    );
    result
}

/// 获取本地网络配置详情（MAC / IPv4+掩码 / 网关 / DNS / 链路本地 IPv6）
///
/// 纯 Rust 驱动（secm-datasource::netif，GetAdaptersAddresses，与 ipconfig /all 同源），
/// 返回全部非回环适配器，前端按需筛选活动接口。
#[tauri::command]
pub async fn get_network_config(
    state: State<'_, AppState>,
) -> Result<Vec<crate::datasource::netif::AdapterConfig>, String> {
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let result = tokio::task::spawn_blocking(crate::datasource::netif::adapter_configs)
        .await
        .map_err(|e| format!("get_network_config panicked: {}", e))?
        .map_err(|e| crate::ds_util::log_err("netif", "获取网络配置", &e))?;
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    let count = result.len();
    buf.append(
        "Info",
        &format!("get_network_config: adapters={} {}ms", count, elapsed),
    );
    Ok(result)
}

/// 应用网络配置（静态 IPv4/IPv6 地址、掩码、网关、DNS；DHCP/静态切换）
///
/// 通过 netsh 应用（需管理员权限，命令内先检查 is_admin 并给出明确提示）；逐项执行并返回
/// 分步结果，单项失败不影响其他项。R8：非管理员时给出明确修复建议。
#[tauri::command]
pub async fn apply_network_config(
    state: State<'_, AppState>,
    req: crate::net_config::NetworkConfigRequest,
) -> Result<crate::net_config::NetworkConfigApplyResult, String> {
    if !crate::cleanup::is_admin() {
        return Err(
            "apply_network_config: 需要管理员权限才能修改网络配置（建议：以管理员身份重新运行本程序）"
                .to_string(),
        );
    }
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let ifname_log = req.ifname.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::net_config::apply_network_config(&req)
    })
    .await
    .map_err(|e| format!("apply_network_config panicked: {}", e))?;
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    let ok_count = result.steps.iter().filter(|s| s.ok).count();
    let fail_count = result.steps.len() - ok_count;
    buf.append(
        if fail_count == 0 { "Info" } else { "Error" },
        &format!(
            "apply_network_config: ifname={} ok={}/{} fail={} {}ms",
            ifname_log,
            ok_count,
            result.steps.len(),
            fail_count,
            elapsed
        ),
    );
    Ok(result)
}

/// 独立应用 DNS 配置（只动 DNS，不改 IP 地址；需管理员权限）
///
/// 契约：DnsConfigRequest { ifname, mode_dns4, dns4, mode_dns6, dns6 } →
/// NetworkConfigApplyResult（分步结果，与 apply_network_config 同构，前端渲染零改动）。
/// 共享 apply_network_config 的 DNS 步骤实现（apply_dns_steps），行为一致。
#[tauri::command]
pub async fn set_dns(
    state: State<'_, AppState>,
    req: crate::net_config::DnsConfigRequest,
) -> Result<crate::net_config::NetworkConfigApplyResult, String> {
    if !crate::cleanup::is_admin() {
        return Err(
            "set_dns: 需要管理员权限才能修改 DNS 配置（建议：以管理员身份重新运行本程序）"
                .to_string(),
        );
    }
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let ifname_log = req.ifname.clone();
    let result = tokio::task::spawn_blocking(move || crate::net_config::apply_dns_config(&req))
        .await
        .map_err(|e| format!("set_dns panicked: {}", e))?;
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    let ok_count = result.steps.iter().filter(|s| s.ok).count();
    let fail_count = result.steps.len() - ok_count;
    buf.append(
        if fail_count == 0 { "Info" } else { "Error" },
        &format!(
            "set_dns: ifname={} ok={}/{} fail={} {}ms",
            ifname_log,
            ok_count,
            result.steps.len(),
            fail_count,
            elapsed
        ),
    );
    Ok(result)
}

/// 查询接口 DoH 配置（Get-DnsClientDohServerAddress，Win10 2004+；只读无需管理员）
///
/// 返回该接口当前启用 DoH 的 DNS 服务器（{ip, template}），template=None 表示未启用。
#[tauri::command]
pub async fn get_doh_config(
    state: State<'_, AppState>,
    ifname: String,
) -> Result<Vec<crate::net_config::DohEntry>, String> {
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let ifname_log = ifname.clone();
    let result = tokio::task::spawn_blocking(move || crate::net_config::get_doh_config(&ifname))
        .await
        .map_err(|e| format!("get_doh_config panicked: {}", e))?;
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    let ok = result.is_ok();
    let count = result.as_ref().map(|v| v.len()).unwrap_or(0);
    buf.append(
        if ok { "Info" } else { "Error" },
        &format!(
            "get_doh_config: ifname={} entries={} {}ms",
            ifname_log,
            count,
            elapsed
        ),
    );
    result
}

/// 应用接口 DoH 配置（Set/Remove-DnsClientDohServerAddress；需管理员权限）
///
/// 前置防御：非接口当前 DNS 的 IP 直接标记失败步骤；移除幂等（无记录视为成功）；
/// 空 entries 视为清空接口全部 DoH。逐 IP 分步返回，前端逐行渲染。
#[tauri::command]
pub async fn set_doh_config(
    state: State<'_, AppState>,
    ifname: String,
    entries: Vec<crate::net_config::DohEntryInput>,
) -> Result<crate::net_config::NetworkConfigApplyResult, String> {
    if !crate::cleanup::is_admin() {
        return Err(
            "set_doh_config: 需要管理员权限才能修改 DoH 配置（建议：以管理员身份重新运行本程序）"
                .to_string(),
        );
    }
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let ifname_log = ifname.clone();
    let entries_log = entries.len();
    let result = tokio::task::spawn_blocking(move || {
        crate::net_config::set_doh_config(&ifname, &entries)
    })
    .await
    .map_err(|e| format!("set_doh_config panicked: {}", e))?;
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    let ok_count = result.steps.iter().filter(|s| s.ok).count();
    let fail_count = result.steps.len() - ok_count;
    buf.append(
        if fail_count == 0 { "Info" } else { "Error" },
        &format!(
            "set_doh_config: ifname={} entries={} ok={}/{} fail={} {}ms",
            ifname_log,
            entries_log,
            ok_count,
            result.steps.len(),
            fail_count,
            elapsed
        ),
    );
    Ok(result)
}

/// 修改网卡物理地址（MAC，注册表 NetworkAddress + 重启网卡，需管理员权限）
///
/// 应用前备份原值；重启网卡瞬间可能短暂断网。返回消息含原值备份与恢复指引。
#[tauri::command]
pub async fn set_network_mac(
    state: State<'_, AppState>,
    ifname: String,
    mac: String,
    guid: Option<String>,
) -> Result<String, String> {
    if !crate::cleanup::is_admin() {
        return Err(
            "set_network_mac: 需要管理员权限才能修改物理地址（建议：以管理员身份重新运行本程序）"
                .to_string(),
        );
    }
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let ifname_log = ifname.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::net_config::set_network_mac(&ifname, &mac, guid.as_deref())
    })
    .await
    .map_err(|e| format!("set_network_mac panicked: {}", e))?;
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    let ok = result.is_ok();
    buf.append(
        if ok { "Info" } else { "Error" },
        &format!(
            "set_network_mac: ifname={} ok={} {}ms",
            ifname_log,
            ok,
            elapsed
        ),
    );
    result
}

/// DHCP 深度检查：探测服务器 + 逐台 ping 可达性 + 同网段/网关拓扑判定 + 综合诊断
///
/// 与 check_dhcp_servers 的区别：额外验证每台服务器的可达性与网络拓扑关系，
/// 覆盖“检测到跨网段 DHCP 服务器但 ping 不通”的场景（如服务器 192.168.11.1、
/// 默认网关 192.168.3.254），输出中文诊断与建议。
#[tauri::command]
pub async fn deep_check_dhcp_servers(
    state: State<'_, AppState>,
) -> Result<crate::dhcp_probe::DhcpDeepCheckResult, String> {
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let result = crate::dhcp_probe::deep_check_dhcp_servers().await;
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    let ok = result.is_ok();
    let summary = match &result {
        Ok(r) => format!(
            "servers=[{}] checks={} summary={}",
            r.servers.join(","),
            r.checks.len(),
            r.summary
        ),
        Err(e) => format!("error={}", e),
    };
    buf.append(
        if ok { "Info" } else { "Error" },
        &format!("deep_check_dhcp_servers: {} {}ms", summary, elapsed),
    );
    result
}

#[tauri::command]
pub async fn get_system_info(
    state: State<'_, AppState>,
) -> Result<crate::sysinfo::SystemInfo, String> {
    cmd_log!(
        state,
        crate::sysinfo::get_system_info(),
        |r: &crate::sysinfo::SystemInfo, ms| format!(
            "get_system_info: edition={} build={} arch={} boot={}, {}ms",
            r.edition, r.build_number, r.arch, r.boot_mode, ms
        )
    )
}

// ============================================================================
// 驱动安装/状态命令（设计文档 §4.5.3；S8：全部走 spawn_blocking 不阻塞 UI）
// ============================================================================

/// 查询温度采集通道状态（幂等：内部 30s 缓存；不阻塞 UI）。
#[tauri::command]
pub async fn driver_status(app: tauri::AppHandle) -> Result<crate::driver_install::DriverStatus, String> {
    tokio::task::spawn_blocking(move || crate::driver_install::ensure_temp_channel(&app))
        .await
        .map_err(|e| format!("driver_status panicked: {}", e))
}

/// 手动触发驱动安装/修复（后台执行，幂等；返回最新通道状态）。
#[tauri::command]
pub async fn driver_install(app: tauri::AppHandle) -> Result<crate::driver_install::DriverStatus, String> {
    tokio::task::spawn_blocking(move || {
        // 手动触发：清缓存后重新执行完整安装链
        crate::driver_install::invalidate_cache();
        crate::driver_install::ensure_temp_channel(&app)
    })
    .await
    .map_err(|e| format!("driver_install panicked: {}", e))
}

/// 卸载本模块自建的 WinRing0 驱动服务（幂等；需管理员权限）。
#[tauri::command]
pub async fn driver_uninstall() -> Result<(), String> {
    tokio::task::spawn_blocking(crate::driver_install::driver_uninstall)
        .await
        .map_err(|e| format!("driver_uninstall panicked: {}", e))?
}

// ============================================================================
// 磁盘枚举 / S.M.A.R.T 命令（GET /api/disks ↔ list_disks；GET /api/disks/:id/smart ↔ get_disk_smart）
// 设计文档：docs/designs/2026-08-07-disk-smart.md
// S8：磁盘 IOCTL 为同步阻塞采集，MUST spawn_blocking 不阻塞 UI
// ============================================================================

/// 磁盘简要列表（含健康度摘要与告警标记）— 前端硬件检测页卡片渲染
#[tauri::command]
pub async fn list_disks(
    state: State<'_, AppState>,
) -> Result<Vec<crate::disk_info::DiskSummary>, String> {
    cmd_log!(
        state,
        crate::disk_info::list_disks(),
        |r: &Vec<crate::disk_info::DiskSummary>, ms| {
            let alert_count = r.iter().filter(|d| d.health.alert).count();
            format!(
                "list_disks: disks={} alerts={}, {}ms",
                r.len(),
                alert_count,
                ms
            )
        }
    )
}

/// 单块磁盘完整 S.M.A.R.T 数据（含健康分析 + 属性表）— 前端卡片详情弹窗
///
/// 读取失败不返回 Err（前端页面不崩），详情内 `error` 字段携带可读原因。
#[tauri::command]
pub async fn get_disk_smart(
    id: String,
    state: State<'_, AppState>,
) -> Result<crate::disk_info::DiskSmartDetail, String> {
    cmd_log!(
        state,
        crate::disk_info::get_disk_smart(&id),
        |r: &crate::disk_info::DiskSmartDetail, ms| format!(
            "get_disk_smart: id={} status={} attrs={} err={}, {}ms",
            r.id,
            r.health.status,
            r.attributes.len(),
            r.error.is_some(),
            ms
        )
    )
}

// ============================================================================
// 高精度计时器（HPET）开关
// ============================================================================

/// 读取高精度计时器关闭状态（enabled = 已关闭，需重启生效）
#[tauri::command]
pub async fn get_hpt_state(
    state: State<'_, AppState>,
) -> Result<crate::settings::SettingState, String> {
    cmd_log!(
        state,
        crate::hpt::get_hpt_state(),
        |s: &crate::settings::SettingState, ms| format!(
            "get_hpt_state: enabled={}, msg={}, {}ms",
            s.enabled, s.message, ms
        )
    )
}

/// 设置高精度计时器：enabled=true 关闭，false 恢复默认（均需重启生效）
#[tauri::command]
pub async fn set_hpt_state(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<crate::settings::SettingState, String> {
    cmd_result!(
        state,
        "set_hpt_state",
        crate::hpt::set_hpt_state(enabled)
    )
}
