/// 临时诊断数据磁盘化模块

///

/// 将所有运行时诊断数据写入磁盘而非内存：

/// - 传感器诊断日志 (每秒 ~200B) → `<运行目录>/diag/sensor.log`

/// - Debug 调试日志 → `<运行目录>/diag/debug.log`

/// - 图表历史 → `<运行目录>/diag/chart_history.jsonl`

///

/// 运行目录解析见 `crate::runtime_paths`（开发期 `<项目根>/build/runtime`，
/// 发行版 `<exe 同级>/runtime`）。

///

/// 自动清理策略：

/// - 单文件 >100MB 时轮转 (保留最新 100MB)

/// - 总目录 >1GB 时删除最旧文件

/// - 每次启动删除 7 天前的诊断文件

/// - 所有写入操作静默失败，不影响主功能
use chrono::Local;

use std::fs;

use std::io::Write;

use std::path::PathBuf;

use std::sync::Mutex;

// ============================================================================

// 配置常量

// ============================================================================

/// 单个诊断文件最大大小 (100MB)

const MAX_SINGLE_FILE: u64 = 100 * 1024 * 1024;

/// 诊断目录总大小上限 (1GB)

const MAX_TOTAL_SIZE: u64 = 1024 * 1024 * 1024;

/// 自动清理的文件保留天数

const RETENTION_DAYS: i64 = 7;

/// 轮转时保留的尾部大小 (保留最近 90MB，丢弃前面旧数据)

const ROTATE_KEEP_TAIL: u64 = 90 * 1024 * 1024;

// ============================================================================

// 全局 Writer

// ============================================================================

/// 诊断文件写入器 — 线程安全，静默失败

struct DiagWriter {
    file: Option<std::fs::File>,

    path: PathBuf,

    bytes_written: u64,
}

impl DiagWriter {
    fn new(name: &str) -> Self {
        let path = get_diag_dir().join(name);

        // 启动时清理旧文件

        let _ = cleanup_old_files();

        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();

        let bytes_written = file
            .as_ref()
            .map_or(0, |f| f.metadata().map_or(0, |m| m.len()));

        DiagWriter {
            file,

            path,

            bytes_written,
        }
    }

    fn write_line(&mut self, line: &str) {
        // 检查是否需要轮转

        if self.bytes_written >= MAX_SINGLE_FILE {
            let _ = rotate_file(&self.path);

            self.file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .ok();

            self.bytes_written = self
                .file
                .as_ref()
                .map_or(0, |f| f.metadata().map_or(0, |m| m.len()));
        }

        let line_with_newline = format!("{}\n", line);

        if let Some(ref mut f) = self.file {
            if f.write_all(line_with_newline.as_bytes()).is_ok() {
                let _ = f.flush();

                self.bytes_written += line_with_newline.len() as u64;
            }
        }

        // 检查总目录大小

        if self.bytes_written % 1000 < line_with_newline.len() as u64 {
            // 每 ~1KB 检查一次，不频繁

            let _ = enforce_total_limit();
        }
    }
}

// ============================================================================

// 全局实例

// ============================================================================

static SENSOR_LOG: std::sync::LazyLock<Mutex<DiagWriter>> =
    std::sync::LazyLock::new(|| Mutex::new(DiagWriter::new("sensor.log")));

static DEBUG_FILE_LOG: std::sync::LazyLock<Mutex<DiagWriter>> =
    std::sync::LazyLock::new(|| Mutex::new(DiagWriter::new("debug.log")));

/// 图表历史持久化（每秒采样点，JSON 行，跨重启恢复图表）

static CHART_LOG: std::sync::LazyLock<Mutex<DiagWriter>> =
    std::sync::LazyLock::new(|| Mutex::new(DiagWriter::new("chart_history.jsonl")));

// ============================================================================

// 公共接口

// ============================================================================

/// 写入传感器诊断日志（替代 eprintln!）

pub fn sensor_diag(msg: &str) {
    if let Ok(mut w) = SENSOR_LOG.lock() {
        let ts = Local::now().format("%H:%M:%S");

        w.write_line(&format!("[{}] {}", ts, msg));
    }
}

/// 写入 debug 日志到磁盘

pub fn debug_file_log(level: &str, module: &str, msg: &str) {
    if let Ok(mut w) = DEBUG_FILE_LOG.lock() {
        let ts = Local::now().format("%Y-%m-%d %H:%M:%S");

        w.write_line(&format!("[{}] [{}] [{}] {}", ts, level, module, msg));
    }
}

/// 追加一条图表历史采样点（JSON 行，由命令层每秒调用：记录到硬盘）

///

/// 行格式由调用方保证为合法 JSON，例如：

/// `{"type":"hw","t":"2026-08-01 19:00:00","cpu":12.3,"gpu":0.0,"mem":45.6}`

pub fn chart_append(line: &str) {
    if let Ok(mut w) = CHART_LOG.lock() {
        w.write_line(line);
    }
}

/// 读取图表历史（最后 limit 行）—— 前端启动时“再一次读取”恢复图表

///

/// 文件不存在/读失败时返回空表（首次运行无历史，优雅降级）

pub fn chart_load(limit: usize) -> Vec<String> {
    let path = get_diag_dir().join("chart_history.jsonl");

    let Ok(content) = fs::read_to_string(&path) else {
        return Vec::new();
    };

    let lines: Vec<&str> = content.lines().collect();

    let start = lines.len().saturating_sub(limit);

    lines[start..].iter().map(|s| s.to_string()).collect()
}

/// 启动时清理 — 删除过期诊断文件 + 强制总大小检查

pub fn init_temp_data() {
    let _ = cleanup_old_files();

    let _ = enforce_total_limit();
}

// 此处曾有 `diag_dir_size()`：全局检索确认调用点为 0（含测试），
// 实际目录统计由内部 `dir_size` 在 `enforce_total_limit` 中完成（ADR-0001 D13）。

/// 诊断目录路径（运行目录下的 `diag/`）——对外暴露的唯一真源。
///
/// 供需要在该目录落盘诊断产物的模块复用（如 `dhcp_residue` 的清理前备份），
/// 避免各处重复拼路径导致目录漂移、脱离本模块的轮转与过期清理策略。
pub fn diag_dir() -> PathBuf {
    get_diag_dir()
}

// ============================================================================

// 内部实现

// ============================================================================

fn get_diag_dir() -> PathBuf {
    crate::runtime_paths::diag_dir()
}

/// 计算目录下所有文件总大小

fn dir_size(dir: &PathBuf) -> u64 {
    let mut total: u64 = 0;

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    total += meta.len();
                }
            }
        }
    }

    total
}

/// 删除 N 天前的诊断文件

fn cleanup_old_files() -> Result<(), String> {
    let diag_dir = get_diag_dir();

    let _ = fs::create_dir_all(&diag_dir);

    let cutoff = Local::now()
        .checked_sub_signed(chrono::Duration::days(RETENTION_DAYS))
        .unwrap_or_else(|| Local::now());

    if let Ok(entries) = fs::read_dir(&diag_dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    if let Ok(modified) = meta.modified() {
                        use std::time::UNIX_EPOCH;

                        if let Ok(mtime) = modified.duration_since(UNIX_EPOCH) {
                            let cutoff_secs = cutoff.timestamp().max(0) as u64;

                            if mtime.as_secs() < cutoff_secs {
                                let _ = fs::remove_file(entry.path());
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// 强制总目录大小不超过 MAX_TOTAL_SIZE

fn enforce_total_limit() -> Result<(), String> {
    let diag_dir = get_diag_dir();

    let total = dir_size(&diag_dir);

    if total <= MAX_TOTAL_SIZE {
        return Ok(());
    }

    // 收集所有文件及其修改时间

    let mut files: Vec<(PathBuf, u64)> = Vec::new();

    if let Ok(entries) = fs::read_dir(&diag_dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    files.push((entry.path(), meta.len()));
                }
            }
        }
    }

    // 按修改时间排序（最旧的在前）

    files.sort_by_key(|(p, _)| {
        fs::metadata(p)
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::UNIX_EPOCH)
    });

    // 删除最旧的文件直到总大小低于限制

    let mut current_total = total;

    for (path, size) in &files {
        if current_total <= MAX_TOTAL_SIZE {
            break;
        }

        if fs::remove_file(path).is_ok() {
            current_total = current_total.saturating_sub(*size);
        }
    }

    Ok(())
}

/// 轮转文件：保留最近 ROTATE_KEEP_TAIL 字节，截断旧数据

/// 轮转文件：保留最近 ROTATE_KEEP_TAIL 字节（按 UTF-8 字符边界对齐，避免切片
/// 落在多字节字符中间导致 String 索引 panic——日志含中文，按字节截断必然越界）。
///
/// 实现：读取全文 → 取尾部（字符安全）→ 以截断模式重写原文件（O_TRUNC 复用原
/// 文件，不依赖 rename 原文件——Windows 上 rename 正在被 append 句柄占用的文件
/// 会因共享冲突静默失败，导致轮转无效）。
fn rotate_file(path: &PathBuf) -> Result<(), String> {
    let content = fs::read_to_string(path).unwrap_or_default();
    if content.len() as u64 <= ROTATE_KEEP_TAIL {
        return Ok(());
    }
    // 按字符（而非字节）确定保留起点：char_indices 保证切片落在字符边界
    let keep_start_byte = content.len().saturating_sub(ROTATE_KEEP_TAIL as usize);
    let mut char_boundary = keep_start_byte;
    // 从 keep_start_byte 向后找到下一个字符边界（取最后一个 <= 起点的边界即可，
    // 因为 keep_start_byte 必然 >= 前一字符末尾；直接用 char_indices 找 >= 的第一个）
    let mut found = None;
    for (idx, _) in content.char_indices() {
        if idx >= keep_start_byte {
            found = Some(idx);
            break;
        }
    }
    if let Some(b) = found {
        char_boundary = b;
    }
    // 找到下一个换行符（确保从完整行开始）
    let actual_start = content[char_boundary..]
        .find('\n')
        .map_or(char_boundary, |pos| char_boundary + pos + 1);
    let kept = &content[actual_start..];
    // 截断重写原文件（O_TRUNC）：已打开的 append 句柄在 Windows 上不影响
    // 以写方式重新打开同一文件（共享模式 FILE_SHARE_READ|WRITE 由调用方打开）；
    // 不再使用 rename（原实现因共享冲突静默失败导致轮转失效）
    fs::write(path, kept).map_err(|e| format!("轮转重写 {} 失败: {}", path.display(), e))?;
    Ok(())
}
