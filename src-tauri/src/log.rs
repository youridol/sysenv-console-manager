/// 用户日志 — 线程安全环形缓冲 + 文件持久化 + Tauri 事件推送。
///
/// 与 `debug.rs` 的关系：
/// - 本模块：用户可见日志 → 文件 + Tauri 事件 + 前端日志卡片
/// - debug.rs：开发者诊断日志 → 仅内存缓冲 + 前端调试面板
/// - 两者共用相同的环形缓冲模式（VecDeque + CAPACITY），独立容量。
///
/// 缓冲容量固定为 200 条（FIFO 淘汰）。
/// 日志同步写入运行目录 `logs\<date>.log`，行格式：
/// `[时间] [级别] [scope] 消息`（scope 为来源页面，旧格式行仍是合法文本）。
/// 日志条目通过 Tauri 事件推送到前端。
use chrono::Local;
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::Emitter;

/// Maximum number of log entries held in the ring buffer
pub const CAPACITY: usize = 200;

/// A single log entry stored in the ring buffer
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
    /// 来源页面 scope（见 CHANGE-CONTRACT §B.2）；缺失时按 "system" 处理
    #[serde(default = "default_scope")]
    pub scope: String,
}

/// 缺失 `scope` 时的默认值（旧日志条目 / 旧前端 payload 一律回落 `system`）。
pub fn default_scope() -> String {
    "system".to_string()
}

/// 命令标签 → 页面 scope（唯一真源，供命令日志宏与内联写入使用）。
///
/// 取值集合冻结（CHANGE-CONTRACT §B.2）：
/// `dashboard | cleanup | network | net-config | settings | services |
///  environment | hardware | logs | about | system`。
///
/// 未命中（未知标签）返回 `"system"`——`system` 表示不属于任何页面的
/// 全局/启动/后台日志（启动、托盘、sidecar、诊断写入等）。
/// `#[cfg(test)] mod tests` 会扫描 `commands.rs` 的真实调用点，断言本表
/// 与实际使用的标签集合完全一致。
pub fn scope_for_command(label: &str) -> &'static str {
    match label {
        // ── 清理页 ──────────────────────────────────────────────────────
        "flush_dns"
        | "clean_shader_cache"
        | "clean_nvidia_cache"
        | "clean_amd_cache"
        | "clean_directx_cache"
        | "clean_steam_cache"
        | "clean_temp_files"
        | "trim_process_working_set"
        | "set_priority"
        | "list_processes" => "cleanup",

        // ── 网络页（用户主动发起的连通性测试）────────────────────────────
        // 注意：`get_link_speeds` 与 `get_net_stats` 不在此列——二者是
        // NetIfaceContext 在所有路由上 1Hz 轮询的后台采样（见下方 dashboard 段注释）。
        "ping_stream"
        | "traceroute_stream"
        | "nslookup_stream"
        | "detect_nat_type_stream"
        | "iperf3_stream"
        | "iperf3_server_start"
        | "iperf3_server_stop"
        // iperf3 的纯查询命令（走 cmd_pure! 不记日志）也在此登记，
        // 与 REGISTERED_ONLY_COMMAND_SCOPES 双向一致：将来补上日志时不会落到 system。
        | "iperf3_tool_info"
        | "iperf3_server_status"
        | "iperf3_firewall_rule"
        | "cancel_net_cmd"
        | "check_dhcp_servers"
        | "deep_check_dhcp_servers"
        | "scan_dhcp_residue"
        | "clean_dhcp_residue"
        | "get_ip_info" => "network",

        // ── 网络配置页（网卡 / 地址 / DNS / DoH / MAC 写入）─────────────
        "get_network_config"
        | "apply_network_config"
        | "set_dns"
        | "get_doh_config"
        | "set_doh_config"
        | "set_network_mac" => "net-config",

        // ── 系统设置页 ──────────────────────────────────────────────────
        "get_hags_state"
        | "set_hags_state"
        | "get_game_mode_state"
        | "set_game_mode_state"
        | "get_game_optimization_state"
        | "set_game_opt"
        | "get_mouse_precision"
        | "set_mouse_precision"
        | "get_nvidia_power_mode"
        | "set_nvidia_power_mode"
        | "get_hpt_state"
        | "set_hpt_state"
        | "get_vrr_state"
        | "set_vrr_state"
        | "get_power_plans"
        | "set_power_plan"
        | "delete_power_plan"
        | "get_hetero_policies"
        | "set_hetero_policy"
        | "enable_ultimate_perf" => "settings",

        // ── 服务页 ──────────────────────────────────────────────────────
        "list_all_services"
        | "start_service"
        | "stop_service"
        | "set_service_start_type" => "services",

        // ── 硬件检测页 ──────────────────────────────────────────────────
        "list_disks" | "get_disk_smart" => "hardware",

        // ── 环境检测页 ──────────────────────────────────────────────────
        "get_system_info" | "check_directx" | "check_vc_runtimes" | "get_game_presets" => {
            "environment"
        }

        // ── 仪表盘（传感器采样 / 图表历史 / 提权）────────────────────────
        // 注意：`get_net_stats` / `get_link_speeds` 虽是网络数据，却由
        // `NetIfaceContext`（App.tsx 中包裹全部路由）以 1Hz 轮询，属于**全局后台采样**
        // 而非任何页面的用户操作。若登记为 `network`/`dashboard`，在「按页输出」模式下
        // 它们会以每秒 1 条的速率灌满环形缓冲并把当前页面的低频日志挤掉，导致按页视图
        // 空白。故一律归 `system`（全局/后台语义）。
        "get_sensor_data" | "get_chart_history" | "request_elevation" | "auto_elevate" => "dashboard",

        // ── 日志本身（已并入右侧栏卡片，scope 仍保留）──────────────────
        "get_logs" | "clear_logs" | "export_logs" => "logs",

        // 未知标签：全局/后台
        _ => "system",
    }
}

/// 前端 logger 的 module 名 → 页面 scope。
///
/// 与前端唯一真源 `src/lib/logs.ts::scopeForModule`（契约 §B.4）逐项对齐：
/// 大小写不敏感，并忽略 `_` / `-` 等全部非字母数字分隔符
/// （`"NetConfig"` / `"net-config"` / `"net_config"` 等价）；未登记的 module
/// 返回 `"system"`（全局/共享模块与后端默认值一致）。
///
/// 用途：`append_frontend_log` 把前端页面的操作与行为同时写入用户可见日志缓冲，
/// 使「按页输出」模式下当前页面能看到自己发起的前端动作。
pub fn scope_for_module(module: &str) -> &'static str {
    match normalize_module(module).as_str() {
        "dashboard" => "dashboard",
        "cleanup" => "cleanup",
        "network" => "network",
        // 网络配置页模块名常见变体
        "netconfig" | "networkconfig" | "netconfigeditor" | "networkconfigeditor" => "net-config",
        "settings" => "settings",
        "services" => "services",
        "environment" => "environment",
        "hardware" => "hardware",
        "about" => "about",
        "logs" | "log" | "logviewer" => "logs",
        _ => "system",
    }
}

/// 规范化 module 名：小写 + 去掉全部非 ASCII 字母数字字符
/// （与前端 `normalizeModule` 的 `toLowerCase().replace(/[^a-z0-9]/g, "")` 一致）。
fn normalize_module(module: &str) -> String {
    module
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// 前端日志级别（TRACE/DEBUG/INFO/WARN/ERROR）→ 用户日志级别。
///
/// 用户日志卡片只有 Debug/Info/Warn/Error 四个级别筛选按钮，故：
/// TRACE/DEBUG → `Debug`，INFO → `Info`，WARN → `Warn`，ERROR → `Error`。
/// 未知级别 → `Info`（命令侧的级别白名单已保证不会走到这里）。
pub fn user_level_for_frontend(level: &str) -> &'static str {
    match level {
        "TRACE" | "DEBUG" => "Debug",
        "INFO" => "Info",
        "WARN" => "Warn",
        "ERROR" => "Error",
        _ => "Info",
    }
}

/// 前端日志写入用户可见日志缓冲（`commands::append_frontend_log` 的用户日志落点）。
///
/// 与 `debug::append`（开发者诊断通道）并行，不是替代：本函数只负责把前端上报
/// 打上来源页面 scope 后写入用户日志，使「按页输出」模式下当前页面能看到自己
/// 发起的前端动作（如网络诊断的 HEAD/端口探测）。
///
/// 抽成独立函数以便直接单测（`State<'_, AppState>` 包装层无法在单测中构造）。
pub fn append_frontend_entry(buf: &LogBuffer, level: &str, module: &str, message: &str) {
    buf.append_scoped(
        user_level_for_frontend(level),
        message,
        scope_for_module(module),
    );
}

/// Thread-safe ring buffer for log entries
pub struct LogBuffer {
    entries: Mutex<VecDeque<LogEntry>>,
    log_dir: PathBuf,
    app_handle: Mutex<Option<tauri::AppHandle>>,
}

impl LogBuffer {
    /// Create a new empty log buffer, creating the log directory if needed.
    pub fn new() -> Self {
        let log_dir = get_log_dir();
        fs::create_dir_all(&log_dir).ok();

        Self {
            entries: Mutex::new(VecDeque::with_capacity(CAPACITY)),
            log_dir,
            app_handle: Mutex::new(None),
        }
    }

    /// Append a log entry with the default `system` scope.
    /// 兼容旧调用：等价于 `append_scoped(level, msg, "system")`。
    pub fn append(&self, level: &str, msg: &str) {
        self.append_scoped(level, msg, "system");
    }

    /// Append a scoped log entry to the buffer, write to file, and push event to frontend.
    /// If the buffer exceeds capacity, the oldest entry is evicted.
    /// 文件日志行：`[时间] [级别] [scope] 消息`。
    pub fn append_scoped(&self, level: &str, msg: &str, scope: &str) {
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
        let entry = LogEntry {
            timestamp,
            level: level.to_string(),
            message: msg.to_string(),
            scope: scope.to_string(),
        };

        // Write to file log
        let file_entry = format!(
            "[{}] [{}] [{}] {}\n",
            entry.timestamp, entry.level, entry.scope, entry.message
        );
        let today = Local::now().format("%Y-%m-%d").to_string();
        let log_file = self.log_dir.join(format!("{}.log", today));
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
        {
            use std::io::Write;
            let _ = file.write_all(file_entry.as_bytes());
        }

        // Write to ring buffer
        {
            // 中毒恢复：与全仓既有约定一致（lhm.rs / net_stats.rs / sensor.rs 均用
            // `unwrap_or_else(|p| p.into_inner())`）。用 `.unwrap()` 时，一旦有线程在
            // 持锁期间 panic，之后每一次日志调用都会 panic，把局部故障放大为全量不可用。
            let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
            if entries.len() >= CAPACITY {
                entries.pop_front();
            }
            entries.push_back(entry.clone());
        }

        // Push event to frontend via Tauri event system
        if let Some(handle) = self
            .app_handle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            let _ = handle.emit("log-event", &entry);
        }
    }

    /// Set the Tauri AppHandle for event emission.
    pub fn set_app_handle(&self, handle: tauri::AppHandle) {
        *self.app_handle.lock().unwrap_or_else(|p| p.into_inner()) = Some(handle);
    }

    /// Return a clone of all buffer entries as a Vec.
    pub fn get_all(&self) -> Vec<LogEntry> {
        let entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.iter().cloned().collect()
    }

    /// Clear all entries from the buffer (does not affect file logs).
    pub fn clear(&self) {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.clear();
    }

    /// Export all log files in the log directory as a concatenated string.
    pub fn export_all_files(&self) -> Result<String, String> {
        let mut result = String::new();
        let mut files: Vec<_> = fs::read_dir(&self.log_dir)
            .map_err(|e| format!("Failed to read log directory: {}", e))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |ext| ext == "log"))
            .collect();

        files.sort_by_key(|f| f.file_name());

        for file in files {
            let content = fs::read_to_string(file.path())
                .map_err(|e| format!("Failed to read log file: {}", e))?;
            result.push_str(&content);
        }

        Ok(result)
    }
}

/// 用户日志目录（运行目录解析见 `runtime_paths`）。
///
/// 原实现写 `%APPDATA%\SECM\logs`，且在 `APPDATA` 缺失时以 `unwrap_or(".")` 落到
/// 当前工作目录——实测因此在源码树内产生了未跟踪的 `src-tauri/SECM/logs` 残留。
fn get_log_dir() -> PathBuf {
    crate::runtime_paths::logs_dir()
}

/// Initialize the log directory — used by LogBuffer for file output.
/// File writing is handled exclusively by LogBuffer::append_scoped.
pub fn init_file_logger() -> Result<(), String> {
    let log_dir = get_log_dir();
    std::fs::create_dir_all(&log_dir).map_err(|e| format!("Cannot create log dir: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// commands.rs 中**实际产生日志**的命令标签 → 期望 scope。
    ///
    /// 这是与 `scope_for_command` 对照的唯一真源：测试会扫描 `commands.rs`
    /// 真实的宏调用点与内联 `scope_for_command("...")` 调用点，断言本表与
    /// 源码提取结果**完全一致**（既不能少登记，也不能留陈旧条目）。
    const EMITTED_COMMAND_SCOPES: &[(&str, &str)] = &[
        // 清理页
        ("flush_dns", "cleanup"),
        ("clean_shader_cache", "cleanup"),
        ("clean_nvidia_cache", "cleanup"),
        ("clean_amd_cache", "cleanup"),
        ("clean_directx_cache", "cleanup"),
        ("clean_steam_cache", "cleanup"),
        ("clean_temp_files", "cleanup"),
        ("trim_process_working_set", "cleanup"),
        ("set_priority", "cleanup"),
        ("list_processes", "cleanup"),
        // 网络页
        ("ping_stream", "network"),
        ("traceroute_stream", "network"),
        ("nslookup_stream", "network"),
        ("detect_nat_type_stream", "network"),
        ("iperf3_stream", "network"),
        ("iperf3_server_start", "network"),
        ("iperf3_server_stop", "network"),
        ("iperf3_firewall_rule", "network"),
        ("check_dhcp_servers", "network"),
        ("deep_check_dhcp_servers", "network"),
        ("scan_dhcp_residue", "network"),
        ("clean_dhcp_residue", "network"),
        ("get_ip_info", "network"),
        // 网络配置页
        ("get_network_config", "net-config"),
        ("apply_network_config", "net-config"),
        ("set_dns", "net-config"),
        ("get_doh_config", "net-config"),
        ("set_doh_config", "net-config"),
        ("set_network_mac", "net-config"),
        // 系统设置页
        ("get_hags_state", "settings"),
        ("set_hags_state", "settings"),
        ("get_game_mode_state", "settings"),
        ("set_game_mode_state", "settings"),
        ("get_game_optimization_state", "settings"),
        ("set_game_opt", "settings"),
        ("get_mouse_precision", "settings"),
        ("set_mouse_precision", "settings"),
        ("get_nvidia_power_mode", "settings"),
        ("set_nvidia_power_mode", "settings"),
        ("get_hpt_state", "settings"),
        ("set_hpt_state", "settings"),
        ("get_vrr_state", "settings"),
        ("set_vrr_state", "settings"),
        ("get_power_plans", "settings"),
        ("set_power_plan", "settings"),
        ("delete_power_plan", "settings"),
        ("get_hetero_policies", "settings"),
        ("set_hetero_policy", "settings"),
        ("enable_ultimate_perf", "settings"),
        // 服务页
        ("list_all_services", "services"),
        ("start_service", "services"),
        ("stop_service", "services"),
        ("set_service_start_type", "services"),
        // 硬件检测页
        ("list_disks", "hardware"),
        ("get_disk_smart", "hardware"),
        // 环境检测页
        ("get_system_info", "environment"),
        // 仪表盘（`get_net_stats` / `get_link_speeds` 属全局 1Hz 后台采样，
        // 由 NetIfaceContext 在全部路由上轮询 → 归 `system`，见 scope_for_command 注释）
        ("get_sensor_data", "dashboard"),
        ("get_net_stats", "system"),
        ("get_link_speeds", "system"),
        // 日志本身
        ("get_logs", "logs"),
        ("clear_logs", "logs"),
        ("export_logs", "logs"),
    ];

    /// 已登记但当前不产生日志的命令（`cmd_pure!` / `cmd_pure_result!` 路径，
    /// 或仅由前端触发的纯查询）。预登记的原因：将来这些命令补上日志时，
    /// 不会因为漏登记而落到 `system`。
    const REGISTERED_ONLY_COMMAND_SCOPES: &[(&str, &str)] = &[
        ("get_chart_history", "dashboard"),
        ("request_elevation", "dashboard"),
        ("auto_elevate", "dashboard"),
        ("check_directx", "environment"),
        ("check_vc_runtimes", "environment"),
        ("get_game_presets", "environment"),
        ("cancel_net_cmd", "network"),
        // iperf3 的纯查询命令：页面会周期性刷新（工具信息 / 服务端状态轮询），
        // 每次都记日志会灌满 200 条环形缓冲并挤掉用户真正关心的操作日志，
        // 故走 cmd_pure! 不产生日志——但 scope 仍预登记，避免将来补日志时落到 system。
        ("iperf3_tool_info", "network"),
        ("iperf3_server_status", "network"),
    ];

    /// 日志宏所在源码（同 crate 内真实文件，测试直接扫描，避免手抄漂移）。
    const COMMANDS_RS: &str = include_str!("commands.rs");

    /// 合法命令标签：小写字母 / 数字 / 下划线，且首字符为字母或下划线。
    fn is_command_label(s: &str) -> bool {
        !s.is_empty()
            && s.chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    }

    /// 从 `name(` 之后的源码中截出宏调用的完整实参（按括号配平，跳过字符串字面量）。
    fn macro_body(tail: &str) -> Option<&str> {
        let mut depth = 1i32;
        let mut in_str = false;
        let mut escaped = false;
        for (i, c) in tail.char_indices() {
            if in_str {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_str = false;
                }
                continue;
            }
            match c {
                '"' => in_str = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&tail[..i]);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// 从单个宏调用实参中提取标签。
    ///
    /// - `cmd_result!` / `cmd_val!`：标签是第 2 个实参 `$label:literal`；
    /// - `cmd_log!`：签名保持 3 参数不变，标签取自日志闭包
    ///   `format!("<命令标签>: <详情>")` 的首行前缀（与宏运行时取法一致）。
    fn macro_label(body: &str, is_cmd_log: bool) -> Option<String> {
        if is_cmd_log {
            return closure_label(body);
        }
        standalone_label(body).or_else(|| inline_macro_label(body))
    }

    /// `cmd_log!` 的标签：日志闭包内首个 `format!("<label>: ...")` 的 `label` 前缀。
    fn closure_label(body: &str) -> Option<String> {
        let mut rest = body;
        while let Some(pos) = rest.find("format!(") {
            let after = &rest[pos + "format!(".len()..];
            if let Some(open) = after.find('"') {
                let tail = &after[open + 1..];
                if let Some(close) = tail.find('"') {
                    let literal = &tail[..close];
                    if let Some((prefix, _)) = literal.split_once(':') {
                        let prefix = prefix.trim();
                        if is_command_label(prefix) {
                            return Some(prefix.to_string());
                        }
                    }
                }
            }
            rest = &rest[pos + 1..];
        }
        None
    }

    /// 多行调用形式：标签独占一行，形如 `        "flush_dns",`。
    fn standalone_label(body: &str) -> Option<String> {
        for line in body.lines() {
            let trimmed = line.trim();
            if trimmed.len() >= 3 && trimmed.starts_with('"') && trimmed.ends_with("\",") {
                let inner = &trimmed[1..trimmed.len() - 2];
                if is_command_label(inner) {
                    return Some(inner.to_string());
                }
            }
        }
        None
    }

    /// 单行调用形式：`cmd_result!(state, "list_all_services", ...)`。
    fn inline_macro_label(body: &str) -> Option<String> {
        let line = body.lines().next()?;
        let mut rest = line;
        while let Some(open) = rest.find('"') {
            let after = &rest[open + 1..];
            let close = after.find('"')?;
            let inner = &after[..close];
            if is_command_label(inner) {
                return Some(inner.to_string());
            }
            rest = &after[close + 1..];
        }
        None
    }

    /// 扫描 commands.rs，提取所有经日志宏登记的命令标签。
    fn macro_labels_in_commands_rs() -> Vec<String> {
        let mut labels = Vec::new();
        for name in ["cmd_result!(", "cmd_val!(", "cmd_log!("] {
            let mut offset = 0usize;
            while let Some(rel) = COMMANDS_RS[offset..].find(name) {
                let start = offset + rel + name.len();
                let tail = &COMMANDS_RS[start..];
                if let Some(body) = macro_body(tail) {
                    if let Some(label) = macro_label(body, name == "cmd_log!(") {
                        labels.push(label);
                    }
                }
                offset = start;
            }
        }
        labels
    }

    /// 扫描 commands.rs，提取内联 `crate::log::scope_for_command("...")` 的标签。
    fn inline_labels_in_commands_rs() -> Vec<String> {
        const NEEDLE: &str = "scope_for_command(\"";
        let mut labels = Vec::new();
        let mut offset = 0usize;
        while let Some(rel) = COMMANDS_RS[offset..].find(NEEDLE) {
            let start = offset + rel + NEEDLE.len();
            let rest = &COMMANDS_RS[start..];
            if let Some(end) = rest.find('"') {
                let label = &rest[..end];
                if is_command_label(label) {
                    labels.push(label.to_string());
                }
            }
            offset = start;
        }
        labels
    }

    #[test]
    fn test_buffer_capacity() {
        let buf = LogBuffer::new();
        for i in 0..250 {
            buf.append("Info", &format!("Message {}", i));
        }
        let entries = buf.get_all();
        assert_eq!(entries.len(), 200);
        // Oldest should have been evicted
        assert_eq!(entries[0].message, "Message 50");
        assert_eq!(entries[199].message, "Message 249");
    }

    #[test]
    fn test_clear_buffer() {
        let buf = LogBuffer::new();
        buf.append("Info", "test");
        buf.clear();
        assert_eq!(buf.get_all().len(), 0);
    }

    #[test]
    fn test_default_scope_is_system() {
        assert_eq!(default_scope(), "system");
        // append（旧调用）落 system
        let buf = LogBuffer::new();
        buf.append("Info", "legacy-append-default-scope");
        let entry = buf
            .get_all()
            .into_iter()
            .rev()
            .find(|e| e.message == "legacy-append-default-scope")
            .expect("entry written");
        assert_eq!(entry.scope, "system");
        // 旧条目（无 scope 字段）反序列化回落 system
        let legacy: LogEntry =
            serde_json::from_str(r#"{"timestamp":"t","level":"Info","message":"m"}"#)
                .expect("legacy entry deserializes");
        assert_eq!(legacy.scope, "system");
    }

    #[test]
    fn test_append_scoped_round_trips_scope() {
        let buf = LogBuffer::new();
        buf.append_scoped("Warn", "scoped-entry-probe", "net-config");
        let entries = buf.get_all();
        let entry = entries
            .iter()
            .rev()
            .find(|e| e.message == "scoped-entry-probe")
            .expect("entry written");
        assert_eq!(entry.level, "Warn");
        assert_eq!(entry.scope, "net-config");
        // serde round-trip 保留 scope
        let json = serde_json::to_string(entry).expect("serialize");
        assert!(json.contains(r#""scope":"net-config""#), "json = {json}");
        let back: LogEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.scope, "net-config");
    }

    #[test]
    fn test_file_log_line_includes_scope() {
        let buf = LogBuffer::new();
        let marker = format!("scoped-file-line-{}", std::process::id());
        buf.append_scoped("Error", &marker, "hardware");
        let today = Local::now().format("%Y-%m-%d").to_string();
        let content = fs::read_to_string(buf.log_dir.join(format!("{}.log", today)))
            .expect("today's log file readable");
        let line = content
            .lines()
            .rev()
            .find(|l| l.contains(&marker))
            .expect("marker line present in file log");
        assert!(line.contains("[Error]"), "line = {line}");
        assert!(line.contains("[hardware]"), "line = {line}");
        assert!(line.ends_with(&marker), "line = {line}");
    }

    #[test]
    fn test_scope_for_command_unknown_falls_back_to_system() {
        assert_eq!(scope_for_command("no_such_command"), "system");
        assert_eq!(scope_for_command(""), "system");
        assert_eq!(scope_for_command("FLUSH_DNS"), "system");
        assert_eq!(scope_for_command("flush-dns"), "system");
    }

    #[test]
    fn test_scope_for_command_covers_every_command_label() {
        // 1. 源码中经日志宏登记的标签 + 内联 scope_for_command("...") 的标签
        let mut used: Vec<String> = macro_labels_in_commands_rs();
        used.extend(inline_labels_in_commands_rs());
        assert!(!used.is_empty(), "未能从 commands.rs 提取任何命令标签");
        used.sort();
        used.dedup();

        // 2. 与对照表双向一致（不允许漏登记，也不允许陈旧条目）
        let mut expected: Vec<String> = EMITTED_COMMAND_SCOPES
            .iter()
            .map(|(label, _)| label.to_string())
            .collect();
        expected.sort();
        expected.dedup();
        assert_eq!(
            used, expected,
            "commands.rs 实际使用的命令标签与 EMITTED_COMMAND_SCOPES 不一致"
        );

        // 3. 每个标签映射到登记的 scope（允许 `system`：全局/后台采样命令，
        //    例如由 NetIfaceContext 在全部路由上 1Hz 轮询的 get_net_stats /
        //    get_link_speeds。它们不属于任何页面，若强行归页会灌满环形缓冲并挤掉
        //    当前页面的日志，使「按页输出」失效。）
        for (label, scope) in EMITTED_COMMAND_SCOPES {
            assert_eq!(
                scope_for_command(label),
                *scope,
                "标签 {label} 的 scope 不匹配"
            );
        }

        // 4. 预登记（当前无日志）的标签同样映射正确
        for (label, scope) in REGISTERED_ONLY_COMMAND_SCOPES {
            assert_eq!(
                scope_for_command(label),
                *scope,
                "标签 {label} 的 scope 不匹配"
            );
            assert!(
                !used.contains(&label.to_string()),
                "标签 {label} 已开始产生日志，应从 REGISTERED_ONLY_COMMAND_SCOPES 移入 EMITTED_COMMAND_SCOPES"
            );
        }
    }

    #[test]
    fn test_scope_for_module_matches_frontend_mapping() {
        // 与 src/lib/logs.ts::MODULE_TO_SCOPE 逐项对齐
        assert_eq!(scope_for_module("Dashboard"), "dashboard");
        assert_eq!(scope_for_module("Cleanup"), "cleanup");
        assert_eq!(scope_for_module("Network"), "network");
        assert_eq!(scope_for_module("NetConfig"), "net-config");
        assert_eq!(scope_for_module("NetworkConfig"), "net-config");
        assert_eq!(scope_for_module("NetworkConfigEditor"), "net-config");
        assert_eq!(scope_for_module("NetConfigEditor"), "net-config");
        assert_eq!(scope_for_module("Settings"), "settings");
        assert_eq!(scope_for_module("Services"), "services");
        assert_eq!(scope_for_module("Environment"), "environment");
        assert_eq!(scope_for_module("Hardware"), "hardware");
        assert_eq!(scope_for_module("About"), "about");
        assert_eq!(scope_for_module("Logs"), "logs");
        assert_eq!(scope_for_module("Log"), "logs");
        assert_eq!(scope_for_module("LogViewer"), "logs");
    }

    #[test]
    fn test_scope_for_module_normalizes_separators_and_case() {
        // 大小写不敏感 + 忽略 `_` / `-` / 空格等分隔符（与前端 normalizeModule 一致）
        assert_eq!(scope_for_module("net-config"), "net-config");
        assert_eq!(scope_for_module("net_config"), "net-config");
        assert_eq!(scope_for_module("NET-CONFIG"), "net-config");
        assert_eq!(scope_for_module("  net config  "), "net-config");
        assert_eq!(scope_for_module("net.config"), "net-config");
        assert_eq!(scope_for_module("d-a-s-h_b-o-a-r-d"), "dashboard");
        assert_eq!(scope_for_module("HARDWARE"), "hardware");
        // 规范化只保留 ASCII 字母数字：纯非 ASCII 名 → 空串 → system
        assert_eq!(scope_for_module("清理页"), "system");
    }

    #[test]
    fn test_scope_for_module_unknown_falls_back_to_system() {
        assert_eq!(scope_for_module(""), "system");
        assert_eq!(scope_for_module("NetIfaceContext"), "system");
        assert_eq!(scope_for_module("frontend"), "system");
        assert_eq!(scope_for_module("SomeRandomModule"), "system");
    }

    #[test]
    fn test_frontend_level_mapping() {
        // TRACE/DEBUG → Debug（卡片只有 Debug/Info/Warn/Error 四个筛选级别）
        assert_eq!(user_level_for_frontend("TRACE"), "Debug");
        assert_eq!(user_level_for_frontend("DEBUG"), "Debug");
        assert_eq!(user_level_for_frontend("INFO"), "Info");
        assert_eq!(user_level_for_frontend("WARN"), "Warn");
        assert_eq!(user_level_for_frontend("ERROR"), "Error");
        // 未知级别 → Info（命令侧白名单已挡住非法值）
        assert_eq!(user_level_for_frontend("BOGUS"), "Info");
        assert_eq!(user_level_for_frontend(""), "Info");
    }

    /// 证明 `append_frontend_log` 的新落点确实进入用户日志缓冲且 scope 正确。
    ///
    /// 该命令本身需要 `State<'_, AppState>`（单测无法构造），故直接测试它调用的
    /// `append_frontend_entry`——即新路径的全部逻辑。
    #[test]
    fn test_append_frontend_entry_lands_in_user_buffer_with_scope() {
        let buf = LogBuffer::new();

        // 1. Network 页 HEAD 探测（生产仅 WARN/ERROR 上报，故用 ERROR）
        let probe = format!("headProbe failed probe-{}", std::process::id());
        append_frontend_entry(&buf, "ERROR", "Network", &probe);
        let entry = buf
            .get_all()
            .into_iter()
            .rev()
            .find(|e| e.message == probe)
            .expect("frontend entry must land in the user log buffer");
        assert_eq!(entry.level, "Error", "前端 ERROR 应映射为用户日志 Error");
        assert_eq!(entry.scope, "network", "Network 模块应解析为 network scope");

        // 2. WARN → Warn + 大小写/分隔符变体归并到同一 scope
        append_frontend_entry(&buf, "WARN", "net-config", "dns write failed");
        let entry = buf
            .get_all()
            .into_iter()
            .rev()
            .find(|e| e.message == "dns write failed")
            .expect("entry written");
        assert_eq!(entry.level, "Warn");
        assert_eq!(entry.scope, "net-config");

        // 3. 未登记 module → system（全局/共享模块）
        append_frontend_entry(&buf, "INFO", "NetIfaceContext", "global poll");
        let entry = buf
            .get_all()
            .into_iter()
            .rev()
            .find(|e| e.message == "global poll")
            .expect("entry written");
        assert_eq!(entry.level, "Info");
        assert_eq!(entry.scope, "system");

        // 4. 该条目同时进入文件日志（scope 落盘，导出/查看可用）
        let today = Local::now().format("%Y-%m-%d").to_string();
        let content = fs::read_to_string(buf.log_dir.join(format!("{}.log", today)))
            .expect("today's log file readable");
        let line = content
            .lines()
            .rev()
            .find(|l| l.contains(&probe))
            .expect("frontend entry must also be written to the file log");
        assert!(line.contains("[Error]"), "line = {line}");
        assert!(line.contains("[network]"), "line = {line}");
    }
}
