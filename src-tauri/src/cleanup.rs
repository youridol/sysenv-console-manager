use crate::debug_info;
/// Cleanup & optimization module for SECM.
///
/// Operations:
/// - DNS cache flush (ipconfig /flushdns)
/// - GPU shader cache cleanup (NVIDIA, AMD, Intel, DirectX, Steam/CS2)
/// - Temp file cleanup
/// - Standby memory release
/// - Process priority management
/// - Process list enumeration
///
/// All registry operations check for administrator privileges before proceeding.
use serde::Serialize;
use std::path::PathBuf;

/// A running process entry for the GUI process list.
#[derive(Debug, Clone, Serialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub memory_mb: f64,
}

/// Enumerate running processes via sysinfo.
pub fn list_processes() -> Vec<ProcessInfo> {
    use sysinfo::System;

    let mut sys = System::new_all();
    sys.refresh_all();

    let mut procs: Vec<ProcessInfo> = sys
        .processes()
        .iter()
        .map(|(pid, proc_)| ProcessInfo {
            pid: pid.as_u32(),
            name: proc_.name().to_string(),
            memory_mb: proc_.memory() as f64 / (1024.0 * 1024.0),
        })
        .collect();

    procs.sort_by(|a, b| {
        b.memory_mb
            .partial_cmp(&a.memory_mb)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    procs.truncate(200);
    procs
}

/// Result of a cleanup operation
#[derive(Debug, Clone, Serialize)]
pub struct CleanupResult {
    pub operation: String,
    pub success: bool,
    pub bytes_freed: u64,
    pub message: String,
}

/// 当前进程是否以管理员权限运行。
///
/// 转发到 `driver_install::is_elevated`（ADR-0001 D14）：两处曾各自手写
/// `OpenProcessToken` + `GetTokenInformation(TokenElevation)`，API 序列完全相同；
/// driver_install 那份有完整的 `#[link]`、命名常量与 RAII 句柄保护，是更完整的实现。
/// 合并后语义不变，后续修正只需改一处。
pub fn is_admin() -> bool {
    crate::driver_install::is_elevated()
}

/// 刷新 DNS 解析缓存（P13）
///
/// 走纯 Rust 驱动（secm-datasource::dns::flush_dns，
/// 即 dnsapi.DnsFlushResolverCache，无本地化文本匹配、无 GBK 解码）。
/// 失败时返回 success=false + 中文错误消息（前端只依赖 success/operation 字段）。
pub fn flush_dns() -> CleanupResult {
    match crate::datasource::dns::flush_dns() {
        Ok(r) => CleanupResult {
            operation: r.operation,
            success: r.success,
            bytes_freed: r.bytes_freed,
            message: r.message,
        },
        Err(e) => {
            let msg = crate::ds_util::log_err("cleanup", "刷新 DNS 缓存", &e);
            CleanupResult {
                operation: "DNS 缓存刷新".to_string(),
                success: false,
                bytes_freed: 0,
                message: msg,
            }
        }
    }
}

/// 清除 Windows 只读属性（只读文件无法直接删除，缓存文件常带此属性）
#[cfg(windows)]
fn clear_readonly(path: &std::path::Path) {
    use std::os::windows::ffi::OsStrExt;
    extern "system" {
        fn SetFileAttributesW(lpFileName: *const u16, dwFileAttributes: u32) -> i32;
    }
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: SetFileAttributesW 是 kernel32 标准导出，传入以 NUL 结尾的宽字符串
    unsafe {
        SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_NORMAL);
    }
}

#[cfg(not(windows))]
fn clear_readonly(_path: &std::path::Path) {}

/// 递归清除目录树内所有文件的只读属性（供 remove_dir_all 前调用）
fn clear_readonly_recursive(path: &std::path::Path) {
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            clear_readonly(&p);
            if p.is_dir() {
                clear_readonly_recursive(&p);
            }
        }
    }
}

/// 标记文件为"重启时删除"（处理被占用文件的标准方案，CCleaner 同款）
/// 通过 MoveFileEx + MOVEFILE_DELAY_UNTIL_REBOOT 注册到系统待删除队列。
#[cfg(windows)]
fn mark_delete_on_reboot(path: &std::path::Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    extern "system" {
        fn MoveFileExW(
            lpExistingFileName: *const u16,
            lpNewFileName: *const u16,
            dwFlags: u32,
        ) -> i32;
    }
    const MOVEFILE_DELAY_UNTIL_REBOOT: u32 = 0x4;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: MoveFileExW 是 kernel32 标准导出，传入 NUL 结尾宽字符串
    unsafe { MoveFileExW(wide.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) != 0 }
}

#[cfg(not(windows))]
fn mark_delete_on_reboot(_path: &std::path::Path) -> bool {
    false
}

/// 重试间隔（毫秒）：远低于原实现的 100ms。「文件被占用」已由
/// [`is_definitive_lock_error`] 分流直接走重启删除通道，这里的退避只用于覆盖
/// 杀软/索引器造成的瞬时抖动，长退避没有收益。
const RETRY_BACKOFF_MS: u64 = 15;

/// 删除重试退避的**总**等待预算（毫秒）。
///
/// 关键不变式：重试退避的总耗时必须有界，且与目录内文件数量无关。
/// 旧实现对每次失败一律 `sleep(100ms)` × 2，于是清理含 N 个被占用文件的目录
/// 会额外阻塞 `200N` 毫秒（实测：10 个占用文件 2.1s、40 个 8.2s、80 个 16.2s、
/// 120 个 24.0s，严格线性）。真实机器 TEMP 常有数百个被占用文件，
/// 因此表现为「点击清理后一直转圈」。共享截止时间把所有文件的退避总量封顶。
const RETRY_BUDGET_MS: u64 = 500;

/// 判定错误是否属于「文件确实被占用 / 拒绝访问」。
///
/// 这类失败在本轮清理内不会自行恢复，立即转入「重启后删除」通道即可，
/// 反复重试纯属浪费——正是旧实现 200ms/文件 卡顿的来源。
#[cfg(windows)]
fn is_definitive_lock_error(e: &std::io::Error) -> bool {
    // 5 = ERROR_ACCESS_DENIED, 32 = ERROR_SHARING_VIOLATION, 33 = ERROR_LOCK_VIOLATION
    matches!(e.raw_os_error(), Some(5) | Some(32) | Some(33))
}

#[cfg(not(windows))]
fn is_definitive_lock_error(_e: &std::io::Error) -> bool {
    false
}

/// 带重试的删除。
///
/// - `retry_deadline`：退避等待的共享截止时间。一旦超过，本次及后续调用都不再
///   退避，立即返回最后一次错误——保证全部文件的退避总耗时不超过
///   [`RETRY_BUDGET_MS`]（与文件数量无关）。
/// - 命中 [`is_definitive_lock_error`]（被占用）时不退避，直接交给
///   `mark_delete_on_reboot` 处理。
fn delete_with_retry(
    path: &std::path::Path,
    retry_deadline: std::time::Instant,
) -> std::io::Result<()> {
    const MAX_ATTEMPTS: u32 = 3;
    let mut last_err = None;
    for attempt in 1..=MAX_ATTEMPTS {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let retryable = !is_definitive_lock_error(&e);
                let has_budget = std::time::Instant::now() < retry_deadline;
                last_err = Some(e);
                if attempt < MAX_ATTEMPTS && retryable && has_budget {
                    std::thread::sleep(std::time::Duration::from_millis(RETRY_BACKOFF_MS));
                } else {
                    break;
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::other("delete failed")))
}

/// [`clean_directory`] 的结果。
///
/// `pending_reboot` 单独计数而不是把每个文件作为一条错误消息：真实 TEMP 下被
/// 占用文件可达数百个，逐条展开的 `标记重启后删除: <完整路径>` 文本会达到数十万
/// 字节，经 Tauri IPC 回传并整段写入日志文件。
///
/// `pub` 是为了让集成测试能**确定性地**验证消息长度上界
/// （构造 >[`MAX_REPORTED_ERRORS`] 条错误后断言消息有界）——该上界只在
/// 「无法登记重启删除」分支起作用，而提权进程上永远走不到那个分支，
/// 因此无法靠 `clean_temp_files()` 端到端触发（`cargo test --lib` 在本机因
/// 既有的 comctl32 加载问题不可用，单元测试无法覆盖）。
pub struct CleanOutcome {
    /// 成功删除的字节数（仅统计真正删掉的文件）
    pub bytes_freed: u64,
    /// 被占用、已登记「重启后删除」的文件数（正常结果，不算失败）
    pub pending_reboot: u64,
    /// 已展开的详情行（长度上界见 [`MAX_REPORTED_ERRORS`]）
    pub errors: Vec<String>,
    /// 因超出 [`MAX_REPORTED_ERRORS`] 被压掉的详情行数
    pub suppressed_errors: u64,
}

/// 每次 [`clean_directory`] 最多逐条记录的错误数。
///
/// 超出的部分只计数、不展开路径，保证消息长度有界（IPC + 日志安全）。
/// 非提权进程下 `MoveFileExW(MOVEFILE_DELAY_UNTIL_REBOOT)` 会失败
/// （该 API 要求进程属于 Administrators 组），此时每个被占用文件都走这条
/// else 分支——正是最需要封顶的场景。
///
/// 注意：上界是**每次 `clean_directory` 调用**，不是每次操作。
/// `clean_temp_files` 调 3 次，`clean_shader_cache` 会链式调用多次
/// （NVIDIA/AMD/DirectX 各路径 + Steam 各库各目录），故单次操作的消息
/// 上界约为 `调用次数 × MAX_REPORTED_ERRORS` 行。
pub const MAX_REPORTED_ERRORS: usize = 20;

impl CleanOutcome {
    /// 新建空结果。
    pub fn new() -> Self {
        Self {
            bytes_freed: 0,
            pending_reboot: 0,
            errors: Vec::new(),
            suppressed_errors: 0,
        }
    }

    /// 记录一条错误；超过 [`MAX_REPORTED_ERRORS`] 后仅计数不展开，
    /// 使消息长度与文件数解耦。
    pub fn push_error(&mut self, msg: String) {
        if self.errors.len() < MAX_REPORTED_ERRORS {
            self.errors.push(msg);
        } else {
            self.suppressed_errors += 1;
        }
    }

    /// 把本次结果追加到调用方的消息/失败标记中（各调用方共用，避免口径不一致）。
    ///
    /// - 已登记重启删除的文件 → 汇总成一行，不算失败
    /// - 有界数量的详情行 → 逐条追加，标记为失败
    /// - 超出上限而被压掉的行 → 汇总成一行，仍标记为失败（不能静默丢失信息）
    ///
    /// **调用方应优先使用本方法**；若自行拼装消息，务必一并消费
    /// `suppressed_errors`，否则会静默丢失超限信息。
    pub fn extend_messages(&self, messages: &mut Vec<String>, has_errors: &mut bool) {
        if self.pending_reboot > 0 {
            messages.push(format!(
                "{} 个文件正在使用中，已安排重启后自动删除",
                self.pending_reboot
            ));
        }
        for e in &self.errors {
            messages.push(format!("  错误: {}", e));
        }
        if self.suppressed_errors > 0 {
            messages.push(format!(
                "  另有 {} 个文件无法删除（已省略逐条明细）",
                self.suppressed_errors
            ));
        }
        *has_errors |= !self.errors.is_empty() || self.suppressed_errors > 0;
    }
}

impl Default for CleanOutcome {
    fn default() -> Self {
        Self::new()
    }
}

fn clean_directory(path: &PathBuf) -> CleanOutcome {
    let mut out = CleanOutcome::new();
    // 整棵目录树的删除重试退避共享同一截止时间（总等待量封顶，与文件数无关）
    let retry_deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(RETRY_BUDGET_MS);
    clean_directory_inner(path, retry_deadline, &mut out);
    out
}

/// [`clean_directory`] 的递归实现：`retry_deadline` 在整棵目录树内共享。
fn clean_directory_inner(
    path: &PathBuf,
    retry_deadline: std::time::Instant,
    out: &mut CleanOutcome,
) {
    if !path.exists() {
        return;
    }

    match std::fs::read_dir(path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                if entry_path.is_dir() {
                    if is_reparse_point(&entry_path) {
                        debug_info!(
                            "cleanup",
                            "skipping reparse point: {}",
                            entry_path.display()
                        );
                        continue;
                    }
                    clean_directory_inner(&entry_path, retry_deadline, out);
                } else {
                    match entry.metadata() {
                        Ok(meta) => {
                            // 只读文件无法直接删除，先清除只读属性
                            clear_readonly(&entry_path);
                            // 删除成功才计入释放字节
                            match delete_with_retry(&entry_path, retry_deadline) {
                                Ok(_) => out.bytes_freed += meta.len(),
                                Err(_) => {
                                    // 被占用无法删除 → 标记重启后删除（标准方案）
                                    if mark_delete_on_reboot(&entry_path) {
                                        // 已入重启队列 = 正常结果，不是失败
                                        out.pending_reboot += 1;
                                    } else {
                                        // 非提权进程会走到这里（MoveFileExW 要求
                                        // Administrators 组）。push_error 保证详情行
                                        // 数量有上界，避免消息随文件数线性膨胀。
                                        out.push_error(format!(
                                            "删除 {} 失败（无法标记重启删除）",
                                            entry_path.display()
                                        ));
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            out.push_error(format!(
                                "读取 {} 元数据失败: {}",
                                entry_path.display(),
                                e
                            ));
                        }
                    }
                }
            }
        }
        Err(e) => {
            out.push_error(format!("读取目录 {} 失败: {}", path.display(), e));
        }
    }
}

/// Check if a path is a Windows reparse point (symlink / junction).
/// Returns false on non-Windows or if the check fails.
fn is_reparse_point(path: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        extern "system" {
            fn GetFileAttributesW(lpFileName: *const u16) -> u32;
        }

        let attrs = unsafe { GetFileAttributesW(wide.as_ptr()) };
        if attrs == u32::MAX {
            return false;
        }
        attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Get NVIDIA shader cache paths.
/// 注意：DXCache/GLCache 位于 LOCALAPPDATA（用户级），NV_Cache 位于 PROGRAMDATA。
fn get_nvidia_cache_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    // NV_Cache（ProgramData，驱动级着色器缓存）
    if let Ok(programdata) = std::env::var("PROGRAMDATA") {
        paths.push(
            PathBuf::from(&programdata)
                .join("NVIDIA Corporation")
                .join("NV_Cache"),
        );
    }
    // DXCache + GLCache（LOCALAPPDATA — 曾误用 APPDATA 导致路径不存在）
    if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
        paths.push(PathBuf::from(&localappdata).join("NVIDIA").join("DXCache"));
        paths.push(PathBuf::from(&localappdata).join("NVIDIA").join("GLCache"));
    }
    paths
}

/// Get DirectX shader cache paths.
fn get_directx_cache_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
        paths.push(PathBuf::from(&localappdata).join("D3DSCache"));
    }
    paths
}

/// Get AMD shader cache paths.
fn get_amd_cache_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
        paths.push(PathBuf::from(&localappdata).join("AMD").join("DxCache"));
        paths.push(PathBuf::from(&localappdata).join("AMD").join("GLCache"));
        paths.push(PathBuf::from(&localappdata).join("AMD").join("VkCache"));
    }
    paths
}

/// 递归删除空目录（清理后移除残留的缓存目录结构）
/// 仅删除空目录，非空时 remove_dir 失败无害。
fn remove_empty_dirs(path: &std::path::Path) {
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                remove_empty_dirs(&entry.path());
            }
        }
    }
    let _ = std::fs::remove_dir(path);
}

/// 通用：清理一组路径并汇总结果（success 反映真实删除结果）
/// 被占用文件标记重启删除 → 不算失败（已安排系统重启时清理）
fn clean_paths(paths: &[PathBuf], operation: &str) -> CleanupResult {
    let mut total_bytes: u64 = 0;
    let mut messages: Vec<String> = Vec::new();
    let mut has_errors = false;
    for path in paths {
        if path.exists() {
            let out = clean_directory(path);
            total_bytes += out.bytes_freed;
            messages.push(format!(
                "清理 {}: 释放 {} 字节",
                path.display(),
                out.bytes_freed
            ));
            // 缓存目录清空后删除空目录，实现彻底清理
            remove_empty_dirs(path);
            out.extend_messages(&mut messages, &mut has_errors);
        } else {
            messages.push(format!("未发现缓存目录: {}", path.display()));
        }
    }
    CleanupResult {
        operation: operation.to_string(),
        success: !has_errors,
        bytes_freed: total_bytes,
        message: messages.join("\n"),
    }
}

/// Clean NVIDIA shader caches (NV_Cache + GLCache).
pub fn clean_nvidia_cache() -> CleanupResult {
    clean_paths(&get_nvidia_cache_paths(), "NVIDIA 着色器缓存清理")
}

/// Clean DirectX shader cache (D3DSCache).
pub fn clean_directx_cache() -> CleanupResult {
    clean_paths(&get_directx_cache_paths(), "DirectX 着色器缓存清理")
}

/// Clean AMD shader caches (DxCache + GLCache + VkCache).
pub fn clean_amd_cache() -> CleanupResult {
    clean_paths(&get_amd_cache_paths(), "AMD 着色器缓存清理")
}

/// Clean Steam shader caches (all library folders + CS2 deep clean).
pub fn clean_steam_cache() -> CleanupResult {
    let steam_path = match get_steam_install_path() {
        Ok(p) => p,
        Err(_) => {
            return CleanupResult {
                operation: "Steam 着色器缓存清理".to_string(),
                success: false,
                bytes_freed: 0,
                message: "未找到 Steam 安装目录（未安装 Steam 或注册表缺失）".to_string(),
            };
        }
    };
    let (bytes, messages) = clean_steam_shader_cache(&steam_path);
    // 消息中含"失败/错误"即视为部分失败（如删除被占用文件失败）
    let has_errors = messages
        .iter()
        .any(|m| m.contains("失败") || m.contains("错误"));
    CleanupResult {
        operation: "Steam 着色器缓存清理".to_string(),
        success: !has_errors,
        bytes_freed: bytes,
        message: messages.join("\n"),
    }
}

/// Clean GPU shader caches (all vendors: NVIDIA, AMD, DirectX, Steam) — 一键全清。
pub fn clean_shader_cache() -> CleanupResult {
    let nv = clean_nvidia_cache();
    let amd = clean_amd_cache();
    let dx = clean_directx_cache();
    let steam = clean_steam_cache();
    let total = nv.bytes_freed + amd.bytes_freed + dx.bytes_freed + steam.bytes_freed;
    // 任一子项失败 → 整体视为未完全成功
    let all_ok = nv.success && amd.success && dx.success && steam.success;
    let mut messages = vec![nv.message, amd.message, dx.message, steam.message];
    messages.retain(|m| !m.is_empty());
    CleanupResult {
        operation: "GPU 着色器缓存清理".to_string(),
        success: all_ok,
        bytes_freed: total,
        message: messages.join("\n"),
    }
}

/// Read Steam install path from Windows registry.
fn get_steam_install_path() -> Result<PathBuf, String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let steam_key = hklm
        .open_subkey("SOFTWARE\\WOW6432Node\\Valve\\Steam")
        .map_err(|e| format!("无法读取 Steam 注册表: {}", e))?;

    let install_path: String = steam_key
        .get_value("InstallPath")
        .map_err(|e| format!("无法读取 Steam InstallPath: {}", e))?;

    Ok(PathBuf::from(install_path))
}

/// Parse Steam libraryfolders.vdf to get all library root directories.
/// VDF 路径转义还原：`\\`（文件中的双反斜杠）→ `\`（单个反斜杠）
/// Steam libraryfolders.vdf 中路径字段为 `"D:\\\\SteamLibrary"`（双反斜杠转义），
/// 必须还原为 Windows 真实路径 `D:\\SteamLibrary`，否则 exists() 检查失败导致库被跳过。
fn vdf_unescape(s: &str) -> String {
    s.replace("\\\\", "\\")
}

/// Handles both old key-value format and newer JSON-like format.
fn parse_library_folders(steam_path: &PathBuf) -> Vec<PathBuf> {
    let mut libraries = Vec::new();

    // Add the Steam install directory itself
    libraries.push(steam_path.clone());

    let vdf_path = steam_path.join("steamapps").join("libraryfolders.vdf");
    if !vdf_path.exists() {
        return libraries;
    }

    let content = match std::fs::read_to_string(&vdf_path) {
        Ok(c) => c,
        Err(_) => return libraries,
    };

    // Parse "path" entries from VDF - both old format ("path" "C:\\...") and new JSON-like format
    for line in content.lines() {
        let trimmed = line.trim();
        // Match lines like: "path"		"D:\\SteamLibrary"
        if let Some(path_start) = trimmed.find("\"path\"") {
            // Find the value after "path"
            let after_key = &trimmed[path_start + 6..];
            if let Some(quote_start) = after_key.find('"') {
                let value_part = &after_key[quote_start + 1..];
                if let Some(quote_end) = value_part.find('"') {
                    let raw = &value_part[..quote_end];
                    let lib_path = PathBuf::from(vdf_unescape(raw));
                    if lib_path != *steam_path && lib_path.exists() {
                        libraries.push(lib_path);
                    }
                }
            }
        }
    }

    libraries
}

/// Clean Steam-specific shader caches across all library folders.
/// For CS2 (appid 730), performs deep cleaning of known cache folders.
fn clean_steam_shader_cache(steam_path: &PathBuf) -> (u64, Vec<String>) {
    let mut total_bytes: u64 = 0;
    let mut messages: Vec<String> = Vec::new();

    let libraries = parse_library_folders(steam_path);

    for lib in &libraries {
        // 1. Clean generic shader cache folder for the library
        let shader_cache = lib.join("steamapps").join("shadercache");
        if shader_cache.exists() {
            let out = clean_directory(&shader_cache);
            total_bytes += out.bytes_freed;
            messages.push(format!(
                "清理 Steam 着色器缓存 {}: {} 字节",
                shader_cache.display(),
                out.bytes_freed
            ));
            // clean_steam_shader_cache 无失败通道，success 由调用方按消息文本判定
            let mut unreported = false;
            out.extend_messages(&mut messages, &mut unreported);
        }

        // 2. CS2 (730) specific deep clean
        let cs2_cache = lib.join("steamapps").join("shadercache").join("730");
        let cs2_folders = [
            "DXVK_state_cache",
            "fozmediav1",
            "fozpipelinesv6",
            "nvidiav1",
            "vulkan",
        ];
        for folder in &cs2_folders {
            let folder_path = cs2_cache.join(folder);
            if folder_path.exists() {
                // 先清除只读属性（缓存文件常为只读，否则 remove_dir_all 失败）
                clear_readonly_recursive(&folder_path);
                if let Err(e) = std::fs::remove_dir_all(&folder_path) {
                    // 部分文件被占用 → 改用 clean_directory 删可删的，占用文件标记重启删除
                    let out = clean_directory(&folder_path);
                    total_bytes += out.bytes_freed;
                    let pending = out.pending_reboot;
                    for err in &out.errors {
                        messages.push(format!("  错误: {}", err));
                    }
                    // 超限被压掉的明细必须汇总可见（否则本分支会静默丢信息）
                    if out.suppressed_errors > 0 {
                        messages.push(format!(
                            "  另有 {} 个文件无法删除（已省略逐条明细）",
                            out.suppressed_errors
                        ));
                    }
                    let _ = remove_empty_dirs(&folder_path);
                    if pending > 0 {
                        messages.push(format!(
                            "CS2 缓存 {}: {} 个占用文件标记重启后删除（remove_dir_all: {}）",
                            folder_path.display(),
                            pending,
                            e
                        ));
                    } else {
                        messages.push(format!(
                            "删除 CS2 缓存文件夹 {} 失败: {}",
                            folder_path.display(),
                            e
                        ));
                    }
                } else {
                    messages.push(format!("已删除 CS2 缓存: {}", folder_path.display()));
                }
            }
        }

        // 3. CS2 game directory shader cache
        let cs2_game_cache = lib
            .join("steamapps")
            .join("common")
            .join("Counter-Strike Global Offensive")
            .join("game")
            .join("csgo")
            .join("shadercache");
        if cs2_game_cache.exists() {
            let out = clean_directory(&cs2_game_cache);
            total_bytes += out.bytes_freed;
            messages.push(format!(
                "清理 CS2 游戏目录着色器缓存: {} 字节",
                out.bytes_freed
            ));
            let mut unreported = false;
            out.extend_messages(&mut messages, &mut unreported);
        }
    }

    (total_bytes, messages)
}

/// Clean Windows temporary files.
pub fn clean_temp_files() -> CleanupResult {
    let mut total_bytes: u64 = 0;
    let mut messages: Vec<String> = Vec::new();
    let mut has_errors = false;

    // System temp
    if let Ok(temp) = std::env::var("TEMP") {
        let temp_path = PathBuf::from(&temp);
        if temp_path.exists() {
            let out = clean_directory(&temp_path);
            total_bytes += out.bytes_freed;
            messages.push(format!("清理 TEMP: {} 字节", out.bytes_freed));
            out.extend_messages(&mut messages, &mut has_errors);
        }
    }

    // Windows temp
    if let Ok(windir) = std::env::var("WINDIR") {
        let win_temp = PathBuf::from(&windir).join("Temp");
        if win_temp.exists() {
            let out = clean_directory(&win_temp);
            total_bytes += out.bytes_freed;
            messages.push(format!("清理 Windows Temp: {} 字节", out.bytes_freed));
            out.extend_messages(&mut messages, &mut has_errors);
        }
    }

    // Prefetch
    if let Ok(windir) = std::env::var("WINDIR") {
        let prefetch = PathBuf::from(&windir).join("Prefetch");
        if prefetch.exists() {
            let out = clean_directory(&prefetch);
            total_bytes += out.bytes_freed;
            messages.push(format!("清理 Prefetch: {} 字节", out.bytes_freed));
            out.extend_messages(&mut messages, &mut has_errors);
        }
    }

    CleanupResult {
        operation: "临时文件清理".to_string(),
        success: !has_errors,
        bytes_freed: total_bytes,
        message: messages.join("\n"),
    }
}

/// Trim the current process working set to free physical memory.
/// Note: this only affects the current process, not system-wide standby memory.
pub fn trim_process_working_set() -> CleanupResult {
    if !is_admin() {
        return CleanupResult {
            operation: "工作集修剪".to_string(),
            success: false,
            bytes_freed: 0,
            message: "需要管理员权限".to_string(),
        };
    }

    // Use Windows API to trim working set
    // This is a limited approach — full standby memory release requires kernel-level access
    #[cfg(target_os = "windows")]
    {
        extern "system" {
            fn SetProcessWorkingSetSize(
                process: *mut std::ffi::c_void,
                min_size: usize,
                max_size: usize,
            ) -> i32;
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
        }

        let ret = unsafe { SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX) };
        CleanupResult {
            operation: "工作集修剪".to_string(),
            success: ret != 0,
            bytes_freed: 0,
            message: if ret != 0 {
                "已修剪当前进程工作集".to_string()
            } else {
                "工作集修剪失败（系统调用返回错误）".to_string()
            },
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        CleanupResult {
            operation: "工作集修剪".to_string(),
            success: false,
            bytes_freed: 0,
            message: "此功能仅在 Windows 上可用".to_string(),
        }
    }
}

/// Set process priority class.
/// Valid priorities: idle, below_normal, normal, above_normal, high, realtime
pub fn set_process_priority(pid: u32, priority: &str) -> CleanupResult {
    #[cfg(target_os = "windows")]
    {
        type DWORD = u32;

        extern "system" {
            fn OpenProcess(
                desired_access: DWORD,
                inherit_handle: i32,
                process_id: DWORD,
            ) -> *mut std::ffi::c_void;
            fn SetPriorityClass(process: *mut std::ffi::c_void, priority_class: DWORD) -> i32;
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }

        let priority_class: DWORD = match priority.to_lowercase().as_str() {
            "idle" => 0x00000040,
            "below_normal" => 0x00004000,
            "normal" => 0x00000020,
            "above_normal" => 0x00008000,
            "high" => 0x00000080,
            "realtime" => 0x00000100,
            _ => {
                return CleanupResult {
                    operation: format!("设置进程优先级 (PID={})", pid),
                    success: false,
                    bytes_freed: 0,
                    message: format!("无效的优先级: {}", priority),
                };
            }
        };

        unsafe {
            let handle = OpenProcess(0x0200, 0, pid);
            if handle.is_null() {
                return CleanupResult {
                    operation: format!("设置进程优先级 (PID={})", pid),
                    success: false,
                    bytes_freed: 0,
                    message: format!("无法打开进程 {} (可能需要管理员权限)", pid),
                };
            }
            let result = SetPriorityClass(handle, priority_class);
            CloseHandle(handle);
            CleanupResult {
                operation: format!("设置进程优先级 (PID={})", pid),
                success: result != 0,
                bytes_freed: 0,
                message: if result != 0 {
                    format!("进程 {} 优先级已设置为 {}", pid, priority)
                } else {
                    format!("设置进程 {} 优先级失败", pid)
                },
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        CleanupResult {
            operation: format!("设置进程优先级 (PID={})", pid),
            success: false,
            bytes_freed: 0,
            message: "此功能仅在 Windows 上可用".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flush_dns() {
        let result = flush_dns();
        // DNS flush should work on any Windows machine
        // It should at least not panic
        assert!(!result.operation.is_empty());
    }

    #[test]
    fn test_get_shader_cache_paths() {
        // 拆分后的路径函数应返回非空候选（NVIDIA/AMD/DirectX 至少一组）
        let nv = get_nvidia_cache_paths();
        let amd = get_amd_cache_paths();
        let dx = get_directx_cache_paths();
        assert!(!nv.is_empty() || !amd.is_empty() || !dx.is_empty());
    }

    #[test]
    fn test_parse_library_folders_non_existent() {
        let fake_path = PathBuf::from("Z:\\NonExistent\\Steam");
        let libs = parse_library_folders(&fake_path);
        // Should return at least the steam path itself
        assert_eq!(libs.len(), 1);
    }

    #[test]
    fn test_invalid_priority() {
        let result = set_process_priority(99999, "invalid");
        assert!(!result.success);
        assert!(result.message.contains("无效的优先级"));
    }

    #[test]
    fn test_vdf_unescape() {
        // VDF 双反斜杠转义 → Windows 单反斜杠路径
        assert_eq!(vdf_unescape("D:\\\\SteamLibrary"), "D:\\SteamLibrary");
        assert_eq!(vdf_unescape("E:\\Games\\Library"), "E:\\Games\\Library");
        // 无转义路径保持不变
        assert_eq!(vdf_unescape("C:\\Steam"), "C:\\Steam");
    }

    #[test]
    fn test_clean_directory_deletes_files() {
        // 验证 clean_directory 真实删除文件，且删除成功才累计字节数
        let dir = std::env::temp_dir().join("secm_test_clean_dir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        let file1 = dir.join("a.txt");
        let file2 = dir.join("b.txt");
        std::fs::write(&file1, "hello").expect("write a");
        std::fs::write(&file2, "world").expect("write b");

        let out = clean_directory(&dir);
        assert!(out.errors.is_empty(), "errors: {:?}", out.errors);
        assert!(
            out.bytes_freed >= 10,
            "bytes_freed should count deleted files, got {}",
            out.bytes_freed
        );
        assert_eq!(out.pending_reboot, 0);
        assert!(!file1.exists(), "file a should be deleted");
        assert!(!file2.exists(), "file b should be deleted");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_clean_directory_counts_only_deleted() {
        // 被占用的文件不得计入释放字节；且应走「重启后删除」通道，
        // 而不是被当成硬错误（旧行为会把每个占用文件记为 error，
        // 使 clean_temp_files 在有占用文件时必然返回 success=false）。
        let dir = std::env::temp_dir().join("secm_test_clean_locked");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        let file = dir.join("locked.txt");
        std::fs::write(&file, "data").expect("write");

        // 以独占方式打开文件，模拟占用（Windows 上独占打开会阻止删除）
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let _handle = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0) // 不共享读写 → 独占锁定
                .open(&file)
                .expect("open locked");
            let out = clean_directory(&dir);
            assert_eq!(out.bytes_freed, 0, "bytes should NOT count locked file");
            assert!(file.exists(), "locked file should remain");
            // 提权进程可登记重启删除；非提权进程登记失败 → 记错误。
            // 两种都是可接受结果，关键是绝不能静默吞掉。
            assert!(
                out.pending_reboot == 1 || !out.errors.is_empty(),
                "locked file must be either scheduled for reboot-delete or reported: {:?}",
                out.errors
            );
            drop(_handle);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
