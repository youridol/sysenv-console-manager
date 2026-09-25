/// 开发者调试日志 — 全局环形缓冲。
///
/// 与 `log.rs` 的关系：
/// - `log.rs` (LogBuffer)：用户可见日志 → 文件持久化 + Tauri 事件推送 + 前端日志页
/// - `debug.rs` (本模块)：开发者诊断日志 → 仅内存缓冲 + 前端调试面板
/// - 两者共用相同的环形缓冲模式（VecDeque + 容量限制），但服务于不同的受众。
///
/// 使用 `debug_info!` / `debug_warn!` / `debug_error!` / `debug_trace!` 宏
/// 在领域模块中记录运行时诊断信息。
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;

// ============================================================================
// 类型定义
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct DebugEntry {
    pub timestamp: String,
    pub level: String,
    pub module: String,
    pub message: String,
    pub context: Option<String>,
}

/// 环形缓冲容量（与 log::CAPACITY 独立，调试日志可保留更多条目）
pub const CAPACITY: usize = 500;

// ============================================================================
// 全局缓冲（OnceLock 惰性初始化）
// ============================================================================

static DEBUG_LOG: std::sync::OnceLock<Mutex<VecDeque<DebugEntry>>> = std::sync::OnceLock::new();

fn log_buf() -> &'static Mutex<VecDeque<DebugEntry>> {
    DEBUG_LOG.get_or_init(|| Mutex::new(VecDeque::with_capacity(CAPACITY)))
}

// ============================================================================
// 公共接口
// ============================================================================

/// 追加一条调试日志条目
pub fn append(level: &str, module: &str, message: &str) {
    // 统一落盘：所有级别写入运行目录 diag/debug.log（与前端调试面板一致），
    // 实现「所有前后端功能函数的 debug.log 统一出口」（temp_data 自带轮转）
    crate::temp_data::debug_file_log(level, module, message);
    let entry = DebugEntry {
        timestamp: chrono::Local::now().format("%H:%M:%S%.3f").to_string(),
        level: level.to_string(),
        module: module.to_string(),
        message: message.to_string(),
        context: None,
    };
    // 中毒恢复：见 log.rs 同名注释（ADR-0001 D16）。
    let mut buf = log_buf().lock().unwrap_or_else(|p| p.into_inner());
    if buf.len() >= CAPACITY {
        buf.pop_front();
    }
    buf.push_back(entry);
}

// 此处曾有 `append_ctx`（带上下文的调试条目）与 `clear_log`（清空缓冲）：
// 全局检索确认二者的调用点均为 0（含测试），ADR-0001 D13 已删除。
// `append_ctx` 是 `DebugEntry.context` 的唯一写入者，删除后该字段恒为 None；
// 字段本身保留在序列化契约中（前端调试面板按既有形状读取，缺字段会破坏兼容）。

/// 获取全部缓冲条目（克隆）
pub fn get_all() -> Vec<DebugEntry> {
    log_buf()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .cloned()
        .collect()
}

// ============================================================================
// 便利宏
// ============================================================================

#[macro_export]
macro_rules! debug_trace {
    ($m:expr, $fmt:expr $(, $arg:expr)* $(,)?) => {
        $crate::debug::append("TRACE", $m, &format!($fmt $(, $arg)*))
    };
}

#[macro_export]
macro_rules! debug_info {
    ($m:expr, $fmt:expr $(, $arg:expr)* $(,)?) => {
        $crate::debug::append("INFO", $m, &format!($fmt $(, $arg)*))
    };
}

#[macro_export]
macro_rules! debug_warn {
    ($m:expr, $fmt:expr $(, $arg:expr)* $(,)?) => {
        $crate::debug::append("WARN", $m, &format!($fmt $(, $arg)*))
    };
}

#[macro_export]
macro_rules! debug_error {
    ($m:expr, $fmt:expr $(, $arg:expr)* $(,)?) => {
        $crate::debug::append("ERROR", $m, &format!($fmt $(, $arg)*))
    };
}

// ============================================================================
// log crate 桥接（统一 datasource crate 的 log::* 到 SECM debug 体系）
// ============================================================================

/// 全局 log::Log 实现：把 datasource crate（disk/registry/service/netif 等）
/// 的 log::info!/warn!/debug!/error! 转发到 SECM debug 缓冲 + debug.log 文件。
///
/// 背景：datasource 是叶子 crate（禁止反向依赖 secm_lib），其 30 处 log::* 调用
/// 在无 logger 注册时被静默丢弃——分发机器 SMART 失败等关键诊断不可见。
/// 本桥接在 lib.rs 启动时注册一次，统一前后端日志出口。
pub struct SecmLogBridge;

impl log::Log for SecmLogBridge {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // 全部级别转发（debug 缓冲容量 500，文件落盘有轮转）
        metadata.level() <= log::Level::Trace
    }

    fn log(&self, record: &log::Record) {
        let level = match record.level() {
            log::Level::Error => "ERROR",
            log::Level::Warn => "WARN",
            log::Level::Info => "INFO",
            log::Level::Debug => "DEBUG",
            log::Level::Trace => "TRACE",
        };
        let module = record.module_path().unwrap_or("datasource");
        append(level, module, &record.args().to_string());
    }

    fn flush(&self) {}
}

/// 注册 log 桥接（幂等：仅首次生效；与其他 logger 冲突时静默跳过，不 panic）。
pub fn init_log_bridge() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if log::set_logger(&SecmLogBridge).is_ok() {
            log::set_max_level(log::LevelFilter::Trace);
        }
    });
}
