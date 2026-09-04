/// 用户日志 — 线程安全环形缓冲 + 文件持久化 + Tauri 事件推送。
///
/// 与 `debug.rs` 的关系：
/// - 本模块：用户可见日志 → 文件 + Tauri 事件 + 前端日志页
/// - debug.rs：开发者诊断日志 → 仅内存缓冲 + 前端调试面板
/// - 两者共用相同的环形缓冲模式（VecDeque + CAPACITY），独立容量。
///
/// 缓冲容量固定为 200 条（FIFO 淘汰）。
/// 日志同步写入 %APPDATA%\SECM\logs\<date>.log。
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

    /// Append a log entry to the buffer, write to file, and push event to frontend.
    /// If the buffer exceeds capacity, the oldest entry is evicted.
    pub fn append(&self, level: &str, msg: &str) {
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
        let entry = LogEntry {
            timestamp,
            level: level.to_string(),
            message: msg.to_string(),
        };

        // Write to file log
        let file_entry = format!(
            "[{}] [{}] {}\n",
            entry.timestamp, entry.level, entry.message
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
            let mut entries = self.entries.lock().unwrap();
            if entries.len() >= CAPACITY {
                entries.pop_front();
            }
            entries.push_back(entry.clone());
        }

        // Push event to frontend via Tauri event system
        if let Some(handle) = self.app_handle.lock().unwrap().as_ref() {
            let _ = handle.emit("log-event", &entry);
        }
    }

    /// Set the Tauri AppHandle for event emission.
    pub fn set_app_handle(&self, handle: tauri::AppHandle) {
        *self.app_handle.lock().unwrap() = Some(handle);
    }

    /// Return a clone of all buffer entries as a Vec.
    pub fn get_all(&self) -> Vec<LogEntry> {
        let entries = self.entries.lock().unwrap();
        entries.iter().cloned().collect()
    }

    /// Clear all entries from the buffer (does not affect file logs).
    pub fn clear(&self) {
        let mut entries = self.entries.lock().unwrap();
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

/// Get the log directory: %APPDATA%\SECM\logs\
fn get_log_dir() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(appdata).join("SECM").join("logs")
}

/// Initialize the log directory — used by LogBuffer for file output.
/// File writing is handled exclusively by LogBuffer::append.
pub fn init_file_logger() -> Result<(), String> {
    let log_dir = get_log_dir();
    std::fs::create_dir_all(&log_dir).map_err(|e| format!("Cannot create log dir: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
