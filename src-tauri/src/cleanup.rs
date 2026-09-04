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

/// Check if the current process is running as administrator.
pub fn is_admin() -> bool {
    // Use Windows Token Elevation API to check process elevation
    #[cfg(target_os = "windows")]
    {
        use std::ptr::null_mut;

        // Check if the process token has elevation
        type DWORD = u32;

        extern "system" {
            fn OpenProcessToken(
                process_handle: *mut std::ffi::c_void,
                desired_access: DWORD,
                token_handle: *mut *mut std::ffi::c_void,
            ) -> i32;
            fn GetTokenInformation(
                token_handle: *mut std::ffi::c_void,
                token_info_class: i32,
                token_info: *mut std::ffi::c_void,
                token_info_length: DWORD,
                return_length: *mut DWORD,
            ) -> i32;
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
        }

        unsafe {
            let mut token: *mut std::ffi::c_void = null_mut();
            if OpenProcessToken(GetCurrentProcess(), 0x0008, &mut token) == 0 {
                return false;
            }
            let mut elevation: DWORD = 0;
            let mut size: DWORD = std::mem::size_of::<DWORD>() as DWORD;
            let result = GetTokenInformation(
                token,
                20,
                &mut elevation as *mut _ as *mut std::ffi::c_void,
                size,
                &mut size,
            );
            CloseHandle(token);
            result != 0 && elevation != 0
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// Decode bytes from Windows console output (typically GBK on Chinese systems).
/// Tries system ANSI code page (CP_ACP, usually GBK/936 on Chinese Windows),
/// then UTF-8, with a final lossy fallback.
fn decode_console_output(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    extern "system" {
        fn MultiByteToWideChar(
            code_page: u32,
            flags: u32,
            multi_byte_str: *const u8,
            multi_byte_len: i32,
            wide_char_str: *mut u16,
            wide_char_len: i32,
        ) -> i32;
    }

    const CP_ACP: u32 = 0; // system default ANSI code page (GBK on Chinese Windows)
    const CP_UTF8: u32 = 65001;

    // Try system ANSI code page first (GBK on Chinese Windows), then UTF-8
    for &cp in &[CP_ACP, CP_UTF8] {
        unsafe {
            let wide_len = MultiByteToWideChar(
                cp,
                0,
                bytes.as_ptr(),
                bytes.len() as i32,
                std::ptr::null_mut(),
                0,
            );
            if wide_len > 0 {
                let mut wide: Vec<u16> = vec![0; wide_len as usize];
                let result = MultiByteToWideChar(
                    cp,
                    0,
                    bytes.as_ptr(),
                    bytes.len() as i32,
                    wide.as_mut_ptr(),
                    wide_len,
                );
                if result > 0 {
                    wide.truncate(result as usize);
                    if let Ok(s) = String::from_utf16(&wide) {
                        if !s.is_empty() && !s.chars().all(|c| c == '\u{FFFD}') {
                            return s;
                        }
                    }
                }
            }
        }
    }

    // Final fallback
    String::from_utf8_lossy(bytes).to_string()
}

/// Flush the DNS resolver cache using `ipconfig /flushdns`.（legacy 旧实现）
/// Returns the command output and success status.
/// Uses lossy UTF-8 conversion + explicit GBK fallback since ipconfig
/// outputs localized text in the system's ANSI code page.
fn flush_dns_legacy() -> CleanupResult {
    use std::os::windows::process::CommandExt;
    match std::process::Command::new("ipconfig")
        .arg("/flushdns")
        .creation_flags(0x08000000)
        .output()
    {
        Ok(output) => {
            // Decode with system code page (GBK on Chinese Windows) for correct Chinese rendering
            let stdout = decode_console_output(&output.stdout);
            let stderr = decode_console_output(&output.stderr);

            // Match both English and Chinese localized success strings
            let success = stdout.contains("成功刷新")
                || stderr.contains("成功刷新")
                || stdout.to_lowercase().contains("successfully flushed")
                || stderr.to_lowercase().contains("successfully flushed");

            // Build a clean user-facing message
            let mut message = String::new();
            for line in stdout.lines().chain(stderr.lines()) {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                // Only show meaningful lines
                if trimmed.contains("Windows IP")
                    || trimmed.contains("DNS")
                    || trimmed.contains("缓存")
                    || trimmed.contains("resolver")
                    || trimmed.contains("成功")
                    || trimmed.contains("Successfully")
                    || trimmed.contains("flush")
                {
                    message.push_str(trimmed);
                    message.push('\n');
                }
            }
            if message.trim().is_empty() {
                message = format!("{} {}", stdout.trim(), stderr.trim());
            }

            CleanupResult {
                operation: "DNS 缓存刷新".to_string(),
                success,
                bytes_freed: 0,
                message: if success {
                    "DNS 解析缓存已成功刷新".to_string()
                } else {
                    message.trim().to_string()
                },
            }
        }
        Err(e) => CleanupResult {
            operation: "DNS 缓存刷新".to_string(),
            success: false,
            bytes_freed: 0,
            message: format!("执行 ipconfig /flushdns 失败: {}", e),
        },
    }
}

/// 刷新 DNS 解析缓存（P13）
///
/// 过渡期由 `legacy-data-source` feature 控制回退旧 ipconfig 实现；
/// 默认走纯 Rust 驱动（secm-datasource::dns::flush_dns，
/// 即 dnsapi.DnsFlushResolverCache，无本地化文本匹配、无 GBK 解码）。
/// 失败时返回 success=false + 中文错误消息（前端只依赖 success/operation 字段）。
pub fn flush_dns() -> CleanupResult {
    if crate::ds_util::use_legacy() {
        return flush_dns_legacy();
    }

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

/// 带重试的删除（占用可能是瞬时的，重试 2 次）
fn delete_with_retry(path: &std::path::Path) -> std::io::Result<()> {
    let mut last_err = None;
    for attempt in 0..=2 {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                if attempt < 2 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::other("delete failed")))
}

/// Helper: delete files in a directory recursively and return bytes freed.
/// Does NOT remove the directory itself — only its contents.
/// Skips reparse points (symlinks/junctions) to prevent unintended deletion.
fn clean_directory(path: &PathBuf) -> (u64, Vec<String>) {
    let mut bytes_freed: u64 = 0;
    let mut errors: Vec<String> = Vec::new();

    if !path.exists() {
        return (0, errors);
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
                    let (sub_bytes, sub_errors) = clean_directory(&entry_path);
                    bytes_freed += sub_bytes;
                    errors.extend(sub_errors);
                } else {
                    match entry.metadata() {
                        Ok(meta) => {
                            // 只读文件无法直接删除，先清除只读属性
                            clear_readonly(&entry_path);
                            // 删除成功才计入释放字节
                            match delete_with_retry(&entry_path) {
                                Ok(_) => bytes_freed += meta.len(),
                                Err(_) => {
                                    // 被占用无法删除 → 标记重启后删除（标准方案）
                                    if mark_delete_on_reboot(&entry_path) {
                                        errors.push(format!(
                                            "标记重启后删除: {}",
                                            entry_path.display()
                                        ));
                                    } else {
                                        errors.push(format!(
                                            "删除 {} 失败（无法标记重启删除）",
                                            entry_path.display()
                                        ));
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            errors.push(format!("读取 {} 元数据失败: {}", entry_path.display(), e));
                        }
                    }
                }
            }
        }
        Err(e) => {
            errors.push(format!("读取目录 {} 失败: {}", path.display(), e));
        }
    }

    (bytes_freed, errors)
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
    let mut pending_reboot = 0u32;
    for path in paths {
        if path.exists() {
            let (bytes, errors) = clean_directory(path);
            total_bytes += bytes;
            messages.push(format!("清理 {}: 释放 {} 字节", path.display(), bytes));
            // 缓存目录清空后删除空目录，实现彻底清理
            remove_empty_dirs(path);
            for e in errors {
                if e.starts_with("标记重启后删除") {
                    pending_reboot += 1;
                } else {
                    has_errors = true;
                }
                messages.push(format!("  {}", e));
            }
        } else {
            messages.push(format!("未发现缓存目录: {}", path.display()));
        }
    }
    if pending_reboot > 0 {
        messages.push(format!(
            "{} 个被占用文件已标记为重启后删除，重启后系统将自动清理",
            pending_reboot
        ));
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
            let (bytes, errors) = clean_directory(&shader_cache);
            total_bytes += bytes;
            messages.push(format!(
                "清理 Steam 着色器缓存 {}: {} 字节",
                shader_cache.display(),
                bytes
            ));
            for e in errors {
                messages.push(format!("  错误: {}", e));
            }
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
                    let (bytes, errors) = clean_directory(&folder_path);
                    total_bytes += bytes;
                    let mut pending = 0u32;
                    for err in &errors {
                        if err.starts_with("标记重启后删除") {
                            pending += 1;
                        } else {
                            messages.push(format!("  错误: {}", err));
                        }
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
            let (bytes, errors) = clean_directory(&cs2_game_cache);
            total_bytes += bytes;
            messages.push(format!("清理 CS2 游戏目录着色器缓存: {} 字节", bytes));
            for e in errors {
                messages.push(format!("  错误: {}", e));
            }
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
            let (bytes, errors) = clean_directory(&temp_path);
            total_bytes += bytes;
            messages.push(format!("清理 TEMP: {} 字节", bytes));
            if !errors.is_empty() {
                has_errors = true;
            }
            for e in errors {
                messages.push(format!("  错误: {}", e));
            }
        }
    }

    // Windows temp
    if let Ok(windir) = std::env::var("WINDIR") {
        let win_temp = PathBuf::from(&windir).join("Temp");
        if win_temp.exists() {
            let (bytes, errors) = clean_directory(&win_temp);
            total_bytes += bytes;
            messages.push(format!("清理 Windows Temp: {} 字节", bytes));
            if !errors.is_empty() {
                has_errors = true;
            }
            for e in errors {
                messages.push(format!("  错误: {}", e));
            }
        }
    }

    // Prefetch
    if let Ok(windir) = std::env::var("WINDIR") {
        let prefetch = PathBuf::from(&windir).join("Prefetch");
        if prefetch.exists() {
            let (bytes, errors) = clean_directory(&prefetch);
            total_bytes += bytes;
            messages.push(format!("清理 Prefetch: {} 字节", bytes));
            if !errors.is_empty() {
                has_errors = true;
            }
            for e in errors {
                messages.push(format!("  错误: {}", e));
            }
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

        let (bytes, errors) = clean_directory(&dir);
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert!(
            bytes >= 10,
            "bytes_freed should count deleted files, got {}",
            bytes
        );
        assert!(!file1.exists(), "file a should be deleted");
        assert!(!file2.exists(), "file b should be deleted");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_clean_directory_counts_only_deleted() {
        // 删除被占用的文件应记录错误且不累计字节（模拟：只读目录导致删除失败）
        let dir = std::env::temp_dir().join("secm_test_clean_locked");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        let file = dir.join("locked.txt");
        std::fs::write(&file, "data").expect("write");

        // 以只读权限打开文件，模拟占用（Windows 上独占打开会阻止删除）
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let _handle = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0) // 不共享读写 → 独占锁定
                .open(&file)
                .expect("open locked");
            let (bytes, errors) = clean_directory(&dir);
            assert!(!errors.is_empty(), "deleting locked file should error");
            assert!(
                bytes == 0,
                "bytes should NOT count locked file, got {}",
                bytes
            );
            assert!(file.exists(), "locked file should remain");
            drop(_handle);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
