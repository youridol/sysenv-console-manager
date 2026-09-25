/// Tauri 命令处理器 — SECM
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
        buf.append_scoped(
            if ok { "Info" } else { "Error" },
            &format!("{}: ok={}, {}ms", $label, ok, elapsed),
            crate::log::scope_for_command($label),
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
        buf.append_scoped(
            "Info",
            &format!("{}: {}ms", $label, elapsed),
            crate::log::scope_for_command($label),
        );
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

/// 同步命令 — spawn_blocking + 计时 + 自定义日志 + 包装为 Ok
/// 日志闭包签名: |val: &T, elapsed_ms: i64| -> String
///
/// 本宏签名保持 3 个参数不变（契约 §B.3「宏签名不变」）。日志闭包的首行约定为
/// `<命令标签>: <详情>`，故命令标签在运行时取自闭包输出（冒号前的前缀），
/// 再经 `crate::log::scope_for_command` 解析来源页面 scope。
macro_rules! cmd_log {
    ($state:expr, $func:expr, $log_fn:expr) => {{
        let start = Local::now();
        let buf = $state.log_buffer.clone();
        let val = tokio::task::spawn_blocking(move || $func)
            .await
            .map_err(|e| format!("Command panicked: {}", e))?;
        let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
        let msg = ($log_fn)(&val, elapsed);
        let label = msg.split(':').next().unwrap_or("").trim();
        buf.append_scoped("Info", &msg, crate::log::scope_for_command(label));
        Ok(val)
    }};
}

// ============================================================================
// 日志命令
// ============================================================================

/// 前端日志上报（双通道：debug.log 开发者诊断 + 用户可见日志缓冲）
///
/// 参数：level（TRACE/DEBUG/INFO/WARN/ERROR）、module（组件名）、message（日志内容）。
/// 校验：level 白名单 + module 长度 + message 长度上限（防滥用），失败静默（不影响前端）。
///
/// 落点：
/// 1. `crate::debug::append` → 运行目录 diag/debug.log（开发者通道，保持不变）；
/// 2. `crate::log::append_frontend_entry` → 用户日志缓冲，按 module 解析来源页面 scope。
///    前端页面动作（如网络诊断的 HEAD/端口探测）据此出现在日志卡片的「按页输出」中。
#[tauri::command]
pub fn append_frontend_log(
    state: State<'_, AppState>,
    level: String,
    module: String,
    message: String,
) -> Result<(), String> {
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
    // 非法/缺失 module 统一回落 "frontend"（无法归属页面 → scope_for_module 解析为 system）
    let effective_module = if module_ok { module_trim } else { "frontend" };
    // 通道 1：开发者诊断（保持原有行为）
    crate::debug::append(&level, effective_module, &msg);
    // 通道 2：用户可见日志（带来源页面 scope）
    crate::log::append_frontend_entry(&state.log_buffer, &level, effective_module, &msg);
    Ok(())
}

#[tauri::command]
pub async fn get_logs(state: State<'_, AppState>) -> Result<Vec<crate::log::LogEntry>, String> {
    let start = Local::now();
    let buf = &state.log_buffer;
    let entries = buf.get_all();
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    buf.append_scoped(
        "Info",
        &format!("get_logs: {} entries ({}ms)", entries.len(), elapsed),
        crate::log::scope_for_command("get_logs"),
    );
    Ok(entries)
}

#[tauri::command]
pub async fn clear_logs(state: State<'_, AppState>) -> Result<(), String> {
    let start = Local::now();
    let buf = &state.log_buffer;
    buf.clear();
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    buf.append_scoped(
        "Info",
        &format!("clear_logs ({}ms)", elapsed),
        crate::log::scope_for_command("clear_logs"),
    );
    Ok(())
}

#[tauri::command]
pub async fn export_logs(state: State<'_, AppState>) -> Result<String, String> {
    let start = Local::now();
    let buf = &state.log_buffer;
    let content = buf.export_all_files()?;
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    buf.append_scoped(
        "Info",
        &format!("export_logs: {} bytes ({}ms)", content.len(), elapsed),
        crate::log::scope_for_command("export_logs"),
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

/// 运行数据目录信息（前端展示真实路径，避免在界面文案中硬编码路径）。
///
/// 返回值与 `runtime_paths` 解析结果同源，是运行数据落点的唯一真源。
#[tauri::command]
pub async fn get_runtime_paths() -> Result<crate::runtime_paths::RuntimePathsInfo, String> {
    Ok(crate::runtime_paths::info())
}

// ============================================================================
// 传感器（特例 — 复杂诊断日志）
// ============================================================================

/// 传感器采集主命令（数据源：LHM sidecar 快照 + sysinfo 投影）。
///
/// 同时写入图表历史（每秒采样点，跨重启恢复），并把采样摘要与诊断写入用户日志。
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
    buf.append_scoped("Info", &format!(
        "[硬件传感器] CPU={:.1}% | freq={:.0}MHz({}) | temp={:.1}°C({}) | GPU={} | gpu_temp={:.1}°C | gpu_power={:.1}W | Mem={:.1}% | {}ms",
        data.cpu.usage, data.cpu.clock_mhz, data.cpu.freq_source, data.cpu.temperature,
        data.cpu.temp_source,
        gpu0.map_or("none".into(), |g| g.name.clone()),
        gpu0.map_or(0.0, |g| g.temperature), gpu0.map_or(0.0, |g| g.power_w),
        data.memory.usage_percent, elapsed,
    ), crate::log::scope_for_command("get_sensor_data"));
    buf.append_scoped(
        "Info",
        &format!("[硬件传感器诊断] {}", data.diag),
        crate::log::scope_for_command("get_sensor_data"),
    );
    Ok(data)
}

// ============================================================================
// tier-2 提权（sidecar 特权伴生进程；on-demand UAC）
// ============================================================================

/// 请求 sidecar tier-2 提权（POST /api/v1/elevate；阻塞 HTTP，置于 spawn_blocking）。
///
/// 返回 `true` = 提权伴生进程已生效（CPU 温度/功耗与主板传感器可用）；
/// `false` = 用户取消 UAC 或提权未生效（前端据此保持 tier-1 并给出说明）。
/// 被拒绝的提权由 sidecar 记忆，不会每次轮询重复弹窗。
#[tauri::command]
pub async fn request_elevation() -> Result<bool, String> {
    tokio::task::spawn_blocking(crate::lhm::request_elevation)
        .await
        .map_err(|e| format!("request_elevation panicked: {}", e))?
}

/// 自动提权（零 UAC）：进入硬件信息页时后台调用，**不等待用户授权**。
///
/// 返回 `true` = tier-2 已由管理员直通或持久授权自动生效（用户零操作）；
/// `false` = 自动路径不可行（非管理员且无持久授权），前端据此才发起一次 UAC 申请。
#[tauri::command]
pub async fn auto_elevate() -> Result<bool, String> {
    tokio::task::spawn_blocking(crate::lhm::auto_elevate)
        .await
        .map_err(|e| format!("auto_elevate panicked: {}", e))?
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

    // 入口注册取消标志：`cancel_command` 只置位已存在的键（cancel.rs:30-35），
    // 未注册则全部 `is_cancelled` 检查点恒为 false，取消永不生效。
    // 出口的 `clear_cancel` 与之严格配对，标志生命周期由本命令界定。
    crate::cancel::cancel_flag(&cid);
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
        Ok(summary) => buf.append_scoped(
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
            crate::log::scope_for_command("ping_stream"),
        ),
        Err(e) => buf.append_scoped(
            "Error",
            &format!("ping {}: {} ({}ms)", target, e, elapsed),
            crate::log::scope_for_command("ping_stream"),
        ),
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

    // 入口注册取消标志：`cancel_command` 只置位已存在的键（cancel.rs:30-35），
    // 未注册则全部 `is_cancelled` 检查点恒为 false，取消永不生效。
    // 出口的 `clear_cancel` 与之严格配对，标志生命周期由本命令界定。
    crate::cancel::cancel_flag(&cid);
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
        Ok(tr) => buf.append_scoped(
            "Info",
            &format!(
                "traceroute {}: {} hops ({}ms)",
                target,
                tr.hops.len(),
                elapsed
            ),
            crate::log::scope_for_command("traceroute_stream"),
        ),
        Err(e) => buf.append_scoped(
            "Error",
            &format!("traceroute {}: {} ({}ms)", target, e, elapsed),
            crate::log::scope_for_command("traceroute_stream"),
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

    // 入口注册取消标志：`cancel_command` 只置位已存在的键（cancel.rs:30-35），
    // 未注册则全部 `is_cancelled` 检查点恒为 false，取消永不生效。
    // 出口的 `clear_cancel` 与之严格配对，标志生命周期由本命令界定。
    crate::cancel::cancel_flag(&cid_ref);
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
        Ok(dr) => buf.append_scoped(
            "Info",
            &format!(
                "nslookup {} ({}): {} records ({}ms)",
                domain,
                record_type,
                dr.records.len(),
                elapsed
            ),
            crate::log::scope_for_command("nslookup_stream"),
        ),
        Err(e) => buf.append_scoped(
            "Error",
            &format!("nslookup {}: {} ({}ms)", domain, e, elapsed),
            crate::log::scope_for_command("nslookup_stream"),
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

    // 入口注册取消标志：`cancel_command` 只置位已存在的键（cancel.rs:30-35），
    // 未注册则全部 `is_cancelled` 检查点恒为 false，取消永不生效。
    // 出口的 `clear_cancel` 与之严格配对，标志生命周期由本命令界定。
    crate::cancel::cancel_flag(&cid_ref);
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
        Ok(nat) => buf.append_scoped(
            "Info",
            &format!(
                "detect_nat_type: {} → {} ({}ms)",
                server, nat.nat_label, elapsed
            ),
            crate::log::scope_for_command("detect_nat_type_stream"),
        ),
        Err(e) => buf.append_scoped(
            "Error",
            &format!("detect_nat_type: {} — {} ({}ms)", server, e, elapsed),
            crate::log::scope_for_command("detect_nat_type_stream"),
        ),
    }
    result
}

/// iperf3 客户端带宽测试（流式）。
///
/// 参数由 `crate::iperf::Iperf3ClientOptions` 承载——覆盖 iperf3 官方手册中
/// **在 Windows 上有意义**的全部客户端选项（协议/方向/结束条件/并行流/omit/
/// 限速/缓冲/MSS/Nagle/网络栈/间隔/单位/TOS·DSCP/零拷贝/标题/服务端输出/
/// 连接超时/亲和性/绑定/源端口/重复载荷/DF/64 位计数/收发超时/文件）。
///
/// 返回值是**结构化结果**（`Iperf3Result`），不再是原始 stderr 文本：
/// 旧实现成功时返回 stderr（通常为空串），UI 只能显示"完成"，无法给出吞吐数字。
///
/// 参数经 `build_client_args` 白名单校验后才落到命令行；全程 `Command::args`
/// 数组传参，不经 shell。
#[tauri::command]
pub async fn iperf3_stream(
    channel: tauri::ipc::Channel<crate::network::StreamEvent>,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    options: crate::iperf::Iperf3ClientOptions,
    cmd_id: String,
) -> Result<crate::iperf::Iperf3Result, String> {
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let cid = cmd_id.clone();
    let cid_inner = cmd_id.clone();
    crate::cancel::cancel_flag(&cid);
    let desc = format!(
        "{}:{} {} {} 并行{} 条件={}",
        options.host,
        options.port,
        match options.protocol {
            crate::iperf::Protocol::Tcp => "TCP",
            crate::iperf::Protocol::Udp => "UDP",
        },
        if options.bidir {
            "双向"
        } else if options.reverse {
            "反向"
        } else {
            "正向"
        },
        options.parallel,
        crate::iperf::end_condition_label(&options),
    );

    let result = tokio::task::spawn_blocking(move || {
        let Some(exe) = crate::iperf::iperf3_exe(Some(&app)) else {
            return Err(
                "未找到 iperf3（已尝试随包内置 resources/iperf3 与系统 PATH）。\
                 请确认安装包完整，或将 iperf3.exe 放入 resources/iperf3/ 目录。"
                    .to_string(),
            );
        };
        let version = crate::iperf::iperf3_version(Some(&app));
        let probe = crate::iperf::Iperf3VersionProbe {
            version,
            raw: crate::iperf::tool_info(Some(&app)).version_raw,
        };
        crate::iperf::run_client_streaming(&options, &exe, &probe, &cid_inner, move |ev| {
            let _ = channel.send(ev);
        })
    })
    .await
    .map_err(|e| format!("iperf3 panicked: {}", e))?;

    crate::cancel::clear_cancel(&cid);
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    let scope = crate::log::scope_for_command("iperf3_stream");

    match &result {
        Ok(r) => {
            // 单行结果摘要（右侧栏「按页输出」）：吞吐/丢包/抖动按方向呈现
            let fmt_dir = |label: &str, d: &crate::iperf::Iperf3DirectionSummary| {
                if d.is_empty() {
                    String::new()
                } else {
                    let mbps = d.mbps.map(|v| format!("{:.1}Mbps", v)).unwrap_or_default();
                    let extra = if let Some(lp) = d.lost_percent {
                        format!(" 丢包{:.2}%", lp)
                    } else if let Some(j) = d.jitter_ms {
                        format!(" 抖动{:.2}ms", j)
                    } else {
                        String::new()
                    };
                    format!(" | {}{}{}", label, mbps, extra)
                }
            };
            let summary = format!(
                "{}{}{}{}",
                fmt_dir(
                    if r.direction == "反向" {
                        "收"
                    } else {
                        "发"
                    },
                    &r.sent
                ),
                fmt_dir("收", &r.received),
                r.bidir_reverse_sent
                    .as_ref()
                    .map(|d| fmt_dir("反向发", d))
                    .unwrap_or_default(),
                r.bidir_reverse_received
                    .as_ref()
                    .map(|d| fmt_dir("反向收", d))
                    .unwrap_or_default(),
            );
            let cancelled_note = if r.cancelled {
                "（用户取消）"
            } else {
                ""
            };
            buf.append_scoped(
                "Info",
                &format!(
                    "iperf3_stream: {} {} 区间{} 条{}{} ({}ms)",
                    desc,
                    r.protocol,
                    r.intervals.len(),
                    summary,
                    cancelled_note,
                    elapsed
                ),
                scope,
            );
        }
        Err(e) => {
            buf.append_scoped(
                "Error",
                &format!("iperf3_stream: {} — {} ({}ms)", desc, e, elapsed),
                scope,
            );
        }
    }

    result
}

/// 查询随包/系统 iperf3 的可用性与版本能力（UI 用于门控与缺失提示）。
///
/// 有意**不记日志**（走 `cmd_pure!`）：页面会周期性刷新该信息，
/// 每次都记一行会灌满 200 条环形缓冲，挤掉用户真正关心的操作日志。
#[tauri::command]
pub async fn iperf3_tool_info(
    app: tauri::AppHandle,
) -> Result<crate::iperf::Iperf3ToolInfo, String> {
    cmd_pure!(crate::iperf::tool_info(Some(&app)))
}

/// 启动本机 iperf3 服务端（长驻进程，供对端测试）。
#[tauri::command]
pub async fn iperf3_server_start(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    port: u16,
    one_off: Option<bool>,
) -> Result<crate::iperf::Iperf3ServerStatus, String> {
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let scope = crate::log::scope_for_command("iperf3_server_start");
    let once = one_off.unwrap_or(false);

    let result = tokio::task::spawn_blocking(move || {
        let Some(exe) = crate::iperf::iperf3_exe(Some(&app)) else {
            return Err("未找到 iperf3（已尝试随包内置 resources/iperf3 与系统 PATH）".to_string());
        };
        crate::iperf::server_start(&exe, port, once)
    })
    .await
    .map_err(|e| format!("iperf3 server start panicked: {}", e))?;

    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    match &result {
        Ok(s) => buf.append_scoped(
            "Info",
            &format!(
                "iperf3_server_start: 端口={} pid={:?} one_off={} ({}ms)",
                s.port.unwrap_or(port),
                s.pid,
                once,
                elapsed
            ),
            scope,
        ),
        Err(e) => buf.append_scoped(
            "Error",
            &format!("iperf3_server_start: 端口={} — {} ({}ms)", port, e, elapsed),
            scope,
        ),
    }
    result
}

/// 停止本机 iperf3 服务端（幂等）。
#[tauri::command]
pub async fn iperf3_server_stop(
    state: State<'_, AppState>,
) -> Result<crate::iperf::Iperf3ServerStatus, String> {
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let scope = crate::log::scope_for_command("iperf3_server_stop");

    let result = tokio::task::spawn_blocking(crate::iperf::server_stop)
        .await
        .map_err(|e| format!("iperf3 server stop panicked: {}", e))?;

    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    buf.append_scoped(
        "Info",
        &format!("iperf3_server_stop: 已停止 ({}ms)", elapsed),
        scope,
    );
    Ok(result)
}

/// 查询本机 iperf3 服务端状态（含最近输出行；UI 展开时轮询）。
///
/// 有意不记日志：轮询会以 1Hz 频率调用，记日志会灌满环形缓冲。
#[tauri::command]
pub async fn iperf3_server_status(
    one_off: Option<bool>,
) -> Result<crate::iperf::Iperf3ServerStatus, String> {
    let once = one_off.unwrap_or(false);
    cmd_pure!(crate::iperf::server_status(once))
}

/// 增删 Windows 防火墙入站规则（让对端能连上本机 iperf3 服务端）。
///
/// 需要管理员权限；规则名带固定前缀 `SECM iperf3 <port> <PROTO>`，便于成对移除。
#[tauri::command]
pub async fn iperf3_firewall_rule(
    state: State<'_, AppState>,
    action: String,
    port: u16,
    protocol: String,
) -> Result<String, String> {
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let scope = crate::log::scope_for_command("iperf3_firewall_rule");
    let act = action.clone();
    let proto = protocol.clone();

    let result =
        tokio::task::spawn_blocking(move || crate::iperf::apply_firewall_rule(&act, port, &proto))
            .await
            .map_err(|e| format!("iperf3 firewall panicked: {}", e))?;

    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    match &result {
        Ok(msg) => buf.append_scoped(
            "Info",
            &format!("iperf3_firewall_rule: {} ({}ms)", msg, elapsed),
            scope,
        ),
        Err(e) => buf.append_scoped(
            "Error",
            &format!(
                "iperf3_firewall_rule: action={} port={} — {} ({}ms)",
                action, port, e, elapsed
            ),
            scope,
        ),
    }
    result
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
        (Ok(()), Some(b)) => buf.append_scoped(
            "Info",
            &format!(
                "set_nvidia_power_mode: {} → {}, {}ms",
                b.label_cn(),
                mode.label_cn(),
                elapsed
            ),
            crate::log::scope_for_command("set_nvidia_power_mode"),
        ),
        (Ok(()), None) => buf.append_scoped(
            "Info",
            &format!(
                "set_nvidia_power_mode: ? → {}, {}ms",
                mode.label_cn(),
                elapsed
            ),
            crate::log::scope_for_command("set_nvidia_power_mode"),
        ),
        (Err(e), _) => buf.append_scoped(
            "Error",
            &format!(
                "set_nvidia_power_mode: 设置 {} 失败: {}, {}ms",
                mode.label_cn(),
                e,
                elapsed
            ),
            crate::log::scope_for_command("set_nvidia_power_mode"),
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
    cmd_result!(
        state,
        "delete_power_plan",
        crate::settings::delete_power_plan(&g)
    )
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

/// HTTP 延迟探测（**服务端**执行，绕开 WebView CSP）。
///
/// 网络诊断页的「HTTP 延迟测试」与「端口检测」原本用浏览器 `fetch` 直连，
/// v1.19.0 收紧 CSP `connect-src` 后发行版全部被判为"不可达"（详见
/// `crate::network::probe_url` 注释）。改由 Rust 侧探测，CSP 无需放开通配、
/// 且支持用户自定义的任意站点。
///
/// ## 唯一耗时口径：HTTP 延迟
///
/// 返回的 `ms` 是**应用层 HTTP 延迟**（DNS + TCP + TLS 握手 + 请求/响应），
/// 由 `crate::network::probe_url` 测量。历史上这里曾并列两个口径：裸 HTTP
/// 耗时与「隐藏 WebView2 真实加载页面的耗时」`page_ms`；本版本起统一为
/// HTTP 延迟，原因见下。
///
/// 为什么砍掉浏览器页面测速：首页「HTTP 延迟测试」的自动检测会按
/// `autoTestInterval`（默认 5s）对全部站点轮询，而浏览器测速要为**每个站点**
/// 开一次隐藏 WebView2 引擎并下载全套子资源——开销与轮询频率完全不匹配，
/// 读数也随子资源/脚本执行波动，不适合做"延迟"这类可反复对比的指标。
///
/// 因此本命令**不再接受 `measure_page` 开关，也不再返回 `page_ms`**：
/// 只给一个明确口径，避免 UI 在两种口径之间回退显示。
///
/// 注意本口径**不是 ICMP ping**：ICMP 是网络层往返，HTTP 延迟额外包含
/// TCP/TLS 与应用层往返，两者数值不可互换（见 `probe_url_live.rs` 守卫）。
///
/// **有意不在后端记日志**（走 `cmd_pure!`/手工实现而非 `cmd_log!`）：
/// 自动检测会按 `autoTestInterval`（默认 5s）对全部站点轮询，每次都记一行
/// 会迅速灌满 200 条环形缓冲，把用户真正关心的操作日志挤掉，使右侧栏
/// 「按页输出」失效。可达性结果由**前端**在用户显式触发时上报
/// （见 Network.tsx `testSite` 的 `report` 参数），自动轮询静默。
#[tauri::command]
pub async fn probe_url(
    url: String,
    timeout_ms: Option<u64>,
) -> Result<crate::network::UrlProbeResult, String> {
    // 超时上限收敛到 30s，避免前端传入异常大值导致一次探测长时间占住线程
    let timeout = timeout_ms.unwrap_or(8000).min(30_000);

    // ureq 是阻塞客户端 → 放 spawn_blocking，不占住 tokio worker 线程
    let probe = tokio::task::spawn_blocking(move || crate::network::probe_url(&url, timeout)).await;

    match probe {
        Ok(r) => Ok(r),
        // spawn_blocking 内部 panic：当作"探测失败"如实上报，不伪装成不可达
        Err(e) => Ok(crate::network::UrlProbeResult {
            reachable: false,
            ms: 0,
            status: None,
            error: Some(format!("探测任务异常终止: {}", e)),
        }),
    }
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
    buf.append_scoped(
        if ok { "Info" } else { "Error" },
        &format!("check_dhcp_servers: {} {}ms", summary, elapsed),
        crate::log::scope_for_command("check_dhcp_servers"),
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
    buf.append_scoped(
        "Info",
        &format!("get_network_config: adapters={} {}ms", count, elapsed),
        crate::log::scope_for_command("get_network_config"),
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
    let result = tokio::task::spawn_blocking(move || crate::net_config::apply_network_config(&req))
        .await
        .map_err(|e| format!("apply_network_config panicked: {}", e))?;
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    let ok_count = result.steps.iter().filter(|s| s.ok).count();
    let fail_count = result.steps.len() - ok_count;
    buf.append_scoped(
        if fail_count == 0 { "Info" } else { "Error" },
        &format!(
            "apply_network_config: ifname={} ok={}/{} fail={} {}ms",
            ifname_log,
            ok_count,
            result.steps.len(),
            fail_count,
            elapsed
        ),
        crate::log::scope_for_command("apply_network_config"),
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
    buf.append_scoped(
        if fail_count == 0 { "Info" } else { "Error" },
        &format!(
            "set_dns: ifname={} ok={}/{} fail={} {}ms",
            ifname_log,
            ok_count,
            result.steps.len(),
            fail_count,
            elapsed
        ),
        crate::log::scope_for_command("set_dns"),
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
    buf.append_scoped(
        if ok { "Info" } else { "Error" },
        &format!(
            "get_doh_config: ifname={} entries={} {}ms",
            ifname_log, count, elapsed
        ),
        crate::log::scope_for_command("get_doh_config"),
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
    let result =
        tokio::task::spawn_blocking(move || crate::net_config::set_doh_config(&ifname, &entries))
            .await
            .map_err(|e| format!("set_doh_config panicked: {}", e))?;
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    let ok_count = result.steps.iter().filter(|s| s.ok).count();
    let fail_count = result.steps.len() - ok_count;
    buf.append_scoped(
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
        crate::log::scope_for_command("set_doh_config"),
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
    buf.append_scoped(
        if ok { "Info" } else { "Error" },
        &format!(
            "set_network_mac: ifname={} ok={} {}ms",
            ifname_log, ok, elapsed
        ),
        crate::log::scope_for_command("set_network_mac"),
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
    buf.append_scoped(
        if ok { "Info" } else { "Error" },
        &format!("deep_check_dhcp_servers: {} {}ms", summary, elapsed),
        crate::log::scope_for_command("deep_check_dhcp_servers"),
    );
    result
}

/// 扫描已移除网卡遗留的 DHCP 注册表记录（只读，无需管理员权限）
///
/// 背景：网卡被拔出/卸载后，`Tcpip\Parameters\Interfaces\{guid}` 仍保留
/// `DhcpServer` / `DhcpIPAddress` / 租约时间等 DHCP 分配记录，导致
/// `check_dhcp_servers` 的基线合并把**已不存在的** DHCP 服务器报出来
/// （本机实测 192.168.11.1 来自一台已移除的 USB/RNDIS 网卡）。
/// 本命令只报告，不修改任何注册表内容。
#[tauri::command]
pub async fn scan_dhcp_residue(
    state: State<'_, AppState>,
) -> Result<crate::dhcp_residue::DhcpResidueScanResult, String> {
    cmd_log!(
        state,
        crate::dhcp_residue::scan_dhcp_residue(),
        |r: &crate::dhcp_residue::DhcpResidueScanResult, ms| format!(
            "scan_dhcp_residue: adapters={} values={} checked={} absent={}, {}ms",
            r.adapters.len(),
            r.value_count,
            r.checked_adapters,
            r.absent_adapters,
            ms
        )
    )
}

/// 清理已移除网卡遗留的 DHCP 注册表记录（需管理员权限）
///
/// 安全约束：
/// - 服务端**重新扫描**决定删除对象，不接受前端传入的键路径/值名；
/// - 只删除 DHCP 分配/租约值（`DhcpServer` 等），不删 `IPAddress` /
///   `SubnetMask` / `DefaultGateway` 等静态配置，也不删除接口子键；
/// - 删除前必须成功写出备份（失败即中止，绝不无备份删除）。
#[tauri::command]
pub async fn clean_dhcp_residue(
    state: State<'_, AppState>,
) -> Result<crate::dhcp_residue::DhcpResidueCleanResult, String> {
    if !crate::cleanup::is_admin() {
        return Err(
            "clean_dhcp_residue: 需要管理员权限才能清理 DHCP 注册表残留（建议：以管理员身份重新运行本程序）"
                .to_string(),
        );
    }
    let start = Local::now();
    let buf = state.log_buffer.clone();
    let result = tokio::task::spawn_blocking(crate::dhcp_residue::clean_dhcp_residue)
        .await
        .map_err(|e| format!("clean_dhcp_residue panicked: {}", e))?;
    let elapsed = Local::now().signed_duration_since(start).num_milliseconds();
    let ok = result.is_ok();
    let summary = match &result {
        Ok(r) => format!(
            "removed={} adapters={} failed={} backup={:?}",
            r.removed_values,
            r.cleaned.len(),
            r.failed.len(),
            r.backup_path
        ),
        Err(e) => format!("error={}", e),
    };
    buf.append_scoped(
        if ok { "Info" } else { "Error" },
        &format!("clean_dhcp_residue: {} {}ms", summary, elapsed),
        crate::log::scope_for_command("clean_dhcp_residue"),
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
pub async fn driver_status(
    app: tauri::AppHandle,
) -> Result<crate::driver_install::DriverStatus, String> {
    tokio::task::spawn_blocking(move || crate::driver_install::ensure_temp_channel(&app))
        .await
        .map_err(|e| format!("driver_status panicked: {}", e))
}

/// 手动触发驱动安装/修复（后台执行，幂等；返回最新通道状态）。
#[tauri::command]
pub async fn driver_install(
    app: tauri::AppHandle,
) -> Result<crate::driver_install::DriverStatus, String> {
    tokio::task::spawn_blocking(move || {
        // 手动触发：清缓存后重新执行完整安装链
        crate::driver_install::invalidate_cache();
        crate::driver_install::ensure_temp_channel(&app)
    })
    .await
    .map_err(|e| format!("driver_install panicked: {}", e))
}

// ============================================================================
// 磁盘枚举 / S.M.A.R.T 命令（GET /api/disks ↔ list_disks；GET /api/disks/:id/smart ↔ get_disk_smart）
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
    cmd_result!(state, "set_hpt_state", crate::hpt::set_hpt_state(enabled))
}
