/// System settings module for SECM.
///
/// Operations:
/// - Registry read/write (HKCU and HKLM)
/// - GPU Hardware Accelerated GPU Scheduling (HAGS)
/// - Windowed game optimization
/// - Mouse precision (enhance pointer precision)
/// - Variable Refresh Rate (VRR/GSync/FreeSync)
/// - Power plan enumeration and switching
/// - Windows service management (16 predefined services)
///
/// All HKLM operations require administrator privileges.
use serde::Serialize;
use std::os::windows::process::CommandExt;
use std::process::Command;

// ============================================================================
// Registry Helpers
// ============================================================================

/// Detect GPU vendor using WMI (fast, no console window flash).（legacy 旧实现）
fn detect_gpu_via_wmi_legacy() -> (String, String) {
    if let Ok(output) = Command::new("wmic")
        .args([
            "path",
            "Win32_VideoController",
            "get",
            "Name",
            "/format:csv",
        ])
        .creation_flags(0x08000000)
        .output()
    {
        let text = String::from_utf8_lossy(&output.stdout).to_lowercase();
        if text.contains("nvidia") || text.contains("nv") {
            return ("nvidia".into(), "NVIDIA GPU 已检测".into());
        }
        if text.contains("amd")
            || text.contains("radeon")
            || text.contains("advanced micro devices")
        {
            return ("amd".into(), "AMD GPU 已检测".into());
        }
        if text.contains("intel")
            || text.contains("uhd")
            || text.contains("iris")
            || text.contains("arc")
        {
            return ("intel".into(), "Intel GPU 已检测".into());
        }
        if text.len() > 20 {
            return (
                "other".into(),
                format!("GPU 已检测: {}", text.lines().last().unwrap_or("").trim()),
            );
        }
    }
    ("unknown".into(), "未检测到 GPU".into())
}

/// Detect GPU vendor（P6）
///
/// 过渡期由 `legacy-data-source` feature 控制回退旧 wmic 实现；
/// 默认走纯 Rust 驱动（secm-datasource::registry，注册表 Class 键枚举，与 Win32_VideoController 同源）。
/// 失败/无数据时返回 "unknown"/"未检测到 GPU"（与现状一致），并打 R8 日志。
fn detect_gpu_via_wmi() -> (String, String) {
    if crate::ds_util::use_legacy() {
        return detect_gpu_via_wmi_legacy();
    }

    match crate::datasource::registry::detect_gpu() {
        Ok(Some(g)) => (g.vendor, g.message),
        Ok(None) => ("unknown".into(), "未检测到 GPU".into()),
        Err(e) => {
            let msg = crate::ds_util::log_err("settings", "检测 GPU 厂商", &e);
            crate::debug_warn!("settings", "GPU 检测降级: {}", msg);
            ("unknown".into(), "未检测到 GPU".into())
        }
    }
}

/// Read a DWORD value from the Windows registry.
pub fn read_registry_dword(hive: &str, key: &str, value: &str) -> Result<u32, String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkey = match hive.to_uppercase().as_str() {
        "HKCU" => HKEY_CURRENT_USER,
        "HKLM" => HKEY_LOCAL_MACHINE,
        _ => return Err(format!("[REG_ERR] 不支持的根键: {}", hive)),
    };

    let reg_key = RegKey::predef(hkey);
    let subkey = reg_key
        .open_subkey_with_flags(key, KEY_READ | KEY_WOW64_64KEY)
        .map_err(|e| format!("[REG_OPEN_ERR] 无法打开: {}\\{} | {}", hive, key, e))?;

    let val: u32 = subkey.get_value(value).map_err(|e| {
        format!(
            "[REG_VALUE_ERR] 无法读取 '{}': {}\\{} | {}",
            value, hive, key, e
        )
    })?;

    Ok(val)
}

/// Write a DWORD value to the Windows registry.
pub fn write_registry_dword(hive: &str, key: &str, value: &str, data: u32) -> Result<(), String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkey = match hive.to_uppercase().as_str() {
        "HKCU" => HKEY_CURRENT_USER,
        "HKLM" => HKEY_LOCAL_MACHINE,
        _ => return Err(format!("Unsupported hive: {}", hive)),
    };

    if hive.to_uppercase() == "HKLM" && !super::cleanup::is_admin() {
        return Err("需要管理员权限才能写入 HKLM。请以管理员身份运行 SECM。".to_string());
    }

    let reg_key = RegKey::predef(hkey);
    let subkey = reg_key
        .create_subkey(key)
        .map_err(|e| format!("Failed to create/open registry key {}: {}", key, e))?;

    subkey
        .0
        .set_value(value, &data)
        .map_err(|e| format!("Failed to write value {} to {}: {}", value, key, e))?;

    Ok(())
}

/// Read a REG_SZ (string) value from the Windows registry.
pub fn read_registry_string(hive: &str, key: &str, value: &str) -> Result<String, String> {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkey = match hive.to_uppercase().as_str() {
        "HKCU" => HKEY_CURRENT_USER,
        "HKLM" => HKEY_LOCAL_MACHINE,
        _ => return Err(format!("Unsupported hive: {}", hive)),
    };
    let reg_key = RegKey::predef(hkey);
    let subkey = reg_key
        .open_subkey_with_flags(key, KEY_READ | KEY_WOW64_64KEY)
        .map_err(|e| format!("[REG_OPEN_ERR] 无法打开: {}\\{} | {}", hive, key, e))?;
    let val: String = subkey.get_value(value).map_err(|e| {
        format!(
            "[REG_VALUE_ERR] REG_SZ '{}': {}\\{} | {}",
            value, hive, key, e
        )
    })?;
    Ok(val)
}

/// Write a REG_SZ (string) value to the Windows registry.
pub fn write_registry_string(hive: &str, key: &str, value: &str, data: &str) -> Result<(), String> {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkey = match hive.to_uppercase().as_str() {
        "HKCU" => HKEY_CURRENT_USER,
        "HKLM" => HKEY_LOCAL_MACHINE,
        _ => return Err(format!("Unsupported hive: {}", hive)),
    };
    if hive.to_uppercase() == "HKLM" && !super::cleanup::is_admin() {
        return Err("需要管理员权限才能写入 HKLM。".to_string());
    }
    let reg_key = RegKey::predef(hkey);
    let subkey = reg_key
        .create_subkey(key)
        .map_err(|e| format!("Failed to create/open registry key {}: {}", key, e))?;
    subkey
        .0
        .set_value(value, &data)
        .map_err(|e| format!("Failed to write REG_SZ {} to {}: {}", value, key, e))?;
    Ok(())
}

/// Parse a semicolon-delimited key=value string (DirectXUserGlobalSettings format).
fn parse_dx_setting(settings: &str, target_key: &str) -> Option<bool> {
    for part in settings.split(';') {
        let part = part.trim();
        if let Some((k, v)) = part.split_once('=') {
            if k.trim() == target_key {
                return Some(v.trim() != "0");
            }
        }
    }
    None
}

/// Update a specific key in the DirectXUserGlobalSettings semicolon-delimited string.
fn update_dx_setting(settings: &str, target_key: &str, enabled: bool) -> String {
    let val = if enabled { "1" } else { "0" };
    let mut found = false;
    let mut result = String::new();
    for part in settings.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((k, _v)) = part.split_once('=') {
            if k.trim() == target_key {
                result.push_str(&format!("{}={};", target_key, val));
                found = true;
                continue;
            }
        }
        result.push_str(part);
        result.push(';');
    }
    if !found {
        result.push_str(&format!("{}={};", target_key, val));
    }
    result
}

const DX_GPU_PREFS_KEY: &str = "Software\\Microsoft\\DirectX\\UserGpuPreferences";
const DX_GLOBAL_SETTINGS_VAL: &str = "DirectXUserGlobalSettings";

// ============================================================================
// Feature Settings
// ============================================================================

/// Represents a toggle-able system setting
#[derive(Debug, Clone, Serialize)]
pub struct SettingState {
    pub name: String,
    pub enabled: bool,
    pub admin_required: bool,
    pub message: String,
}

/// GPU Hardware Accelerated GPU Scheduling (HAGS)
/// Registry: HKLM\SYSTEM\CurrentControlSet\Control\GraphicsDrivers\HwSchMode (DWORD)
///   2 = enabled   |   1/other = disabled   |   absent = not configured
/// Only DWORD=2 means enabled per Windows spec.
pub fn get_hags_state() -> SettingState {
    match read_registry_dword(
        "HKLM",
        "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers",
        "HwSchMode",
    ) {
        Ok(val) => {
            let enabled = val == 2;
            SettingState {
                name: "GPU 硬件加速调度 (HAGS)".to_string(),
                enabled,
                admin_required: true,
                message: if enabled {
                    format!("[HAGS_OK] HwSchMode={} -> 已启用", val)
                } else {
                    format!("[HAGS_OK] HwSchMode={} -> 已禁用", val)
                },
            }
        }
        Err(e) => {
            let (_v, gpu) = detect_gpu_via_wmi();
            SettingState {
                name: "GPU 硬件加速调度 (HAGS)".to_string(),
                enabled: false,
                admin_required: true,
                message: format!("[HAGS_ERR] {} | {}", e, gpu),
            }
        }
    }
}

// ============================================================================
// Windows 游戏模式（设置 → 游戏 → 游戏模式）
// ============================================================================

/// 游戏模式注册表：HKCU\Software\Microsoft\GameBar\AutoGameModeEnabled (DWORD)
///   1 = 启用（后台关闭内容优化游戏） | 0 = 禁用 | 缺失 = 系统默认（启用）
const GAMEBAR_KEY: &str = "Software\\Microsoft\\GameBar";
const GAME_MODE_VAL: &str = "AutoGameModeEnabled";

/// 读取游戏模式开关状态
pub fn get_game_mode_state() -> SettingState {
    match read_registry_dword("HKCU", GAMEBAR_KEY, GAME_MODE_VAL) {
        Ok(val) => {
            let enabled = val == 1;
            SettingState {
                name: "游戏模式".to_string(),
                enabled,
                admin_required: false,
                message: format!(
                    "[GAMEMODE_OK] AutoGameModeEnabled={} -> {}",
                    val,
                    if enabled { "已启用" } else { "已禁用" }
                ),
            }
        }
        Err(_) => SettingState {
            // Windows 默认启用游戏模式：注册表项缺失时视为启用
            name: "游戏模式".to_string(),
            enabled: true,
            admin_required: false,
            message: "[GAMEMODE_OK] 未显式配置（Windows 默认启用）".to_string(),
        },
    }
}

/// 设置游戏模式开关（HKCU，无需管理员）
pub fn set_game_mode_state(enabled: bool) -> Result<SettingState, String> {
    write_registry_dword(
        "HKCU",
        GAMEBAR_KEY,
        GAME_MODE_VAL,
        if enabled { 1 } else { 0 },
    )?;
    Ok(SettingState {
        name: "游戏模式".to_string(),
        enabled,
        admin_required: false,
        message: format!(
            "[GAMEMODE_OK] AutoGameModeEnabled={} -> {}",
            if enabled { 1 } else { 0 },
            if enabled { "已启用" } else { "已禁用" }
        ),
    })
}

pub fn set_hags_state(enabled: bool) -> Result<SettingState, String> {
    let current = read_registry_dword(
        "HKLM",
        "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers",
        "HwSchMode",
    )
    .ok();
    let value = if enabled {
        2u32
    } else {
        if current == Some(2) {
            1u32
        } else {
            0u32
        }
    };
    write_registry_dword(
        "HKLM",
        "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers",
        "HwSchMode",
        value,
    )?;
    Ok(SettingState {
        name: "GPU 硬件加速调度 (HAGS)".to_string(),
        enabled,
        admin_required: true,
        message: format!(
            "HAGS 已{} (HwSchMode={})。需要重启。",
            if enabled { "启用" } else { "禁用" },
            value
        ),
    })
}

// ============================================================================
// Windowed Game Optimization + VRR
// Both use: HKCU\Software\Microsoft\DirectX\UserGpuPreferences
// Value: DirectXUserGlobalSettings (REG_SZ) — semicolon-delimited "Key=Val;"
// ============================================================================

/// Windowed Game Optimization
/// Reg: HKCU\...\DirectXUserGlobalSettings -> SwapEffectUpgradeEnable
///   1=enabled, 0=disabled, absent=default enabled (true)
pub fn get_game_optimization_state() -> SettingState {
    match read_registry_string("HKCU", DX_GPU_PREFS_KEY, DX_GLOBAL_SETTINGS_VAL) {
        Ok(settings) => match parse_dx_setting(&settings, "SwapEffectUpgradeEnable") {
            Some(true) => SettingState {
                name: "窗口化游戏优化".to_string(),
                enabled: true,
                admin_required: false,
                message: "[GAMEOPT_OK] SwapEffectUpgradeEnable=1".to_string(),
            },
            Some(false) => SettingState {
                name: "窗口化游戏优化".to_string(),
                enabled: false,
                admin_required: false,
                message: "[GAMEOPT_OK] SwapEffectUpgradeEnable=0".to_string(),
            },
            None => SettingState {
                name: "窗口化游戏优化".to_string(),
                enabled: true,
                admin_required: false,
                message: "[GAMEOPT_DEF] SwapEffectUpgradeEnable 不存在 -> 默认启用".to_string(),
            },
        },
        Err(_) => {
            if let Ok(val) =
                read_registry_dword("HKCU", "System\\GameConfigStore", "GameDVR_FSEBehaviorMode")
            {
                return SettingState {
                    name: "窗口化游戏优化".to_string(),
                    enabled: val == 2,
                    admin_required: false,
                    message: format!("[GAMEOPT_FB] GameDVR_FSEBehaviorMode={} (旧版Win)", val),
                };
            }
            let (_v, gpu) = detect_gpu_via_wmi();
            SettingState {
                name: "窗口化游戏优化".to_string(),
                enabled: false,
                admin_required: false,
                message: format!("[GAMEOPT_NF] {} 不存在 | {}", DX_GLOBAL_SETTINGS_VAL, gpu),
            }
        }
    }
}

pub fn set_game_optimization(enabled: bool) -> Result<SettingState, String> {
    let cur =
        read_registry_string("HKCU", DX_GPU_PREFS_KEY, DX_GLOBAL_SETTINGS_VAL).unwrap_or_default();
    let upd = update_dx_setting(&cur, "SwapEffectUpgradeEnable", enabled);
    write_registry_string("HKCU", DX_GPU_PREFS_KEY, DX_GLOBAL_SETTINGS_VAL, &upd)?;
    Ok(SettingState {
        name: "窗口化游戏优化".to_string(),
        enabled,
        admin_required: false,
        message: format!(
            "窗口化游戏优化已{} (SwapEffectUpgradeEnable={})",
            if enabled { "启用" } else { "禁用" },
            if enabled { "1" } else { "0" }
        ),
    })
}

/// Variable Refresh Rate (VRR)
/// Reg: HKCU\...\DirectXUserGlobalSettings -> VRROptimizeEnable
///   1=enabled, 0=disabled, absent=default disabled (false)
pub fn get_vrr_state() -> SettingState {
    match read_registry_string("HKCU", DX_GPU_PREFS_KEY, DX_GLOBAL_SETTINGS_VAL) {
        Ok(settings) => match parse_dx_setting(&settings, "VRROptimizeEnable") {
            Some(true) => SettingState {
                name: "可变刷新率 (VRR/G-Sync/FreeSync)".to_string(),
                enabled: true,
                admin_required: false,
                message: "[VRR_OK] VRROptimizeEnable=1".to_string(),
            },
            Some(false) => SettingState {
                name: "可变刷新率 (VRR/G-Sync/FreeSync)".to_string(),
                enabled: false,
                admin_required: false,
                message: "[VRR_OK] VRROptimizeEnable=0".to_string(),
            },
            None => SettingState {
                name: "可变刷新率 (VRR/G-Sync/FreeSync)".to_string(),
                enabled: false,
                admin_required: false,
                message: "[VRR_DEF] VRROptimizeEnable 不存在 -> 默认关闭".to_string(),
            },
        },
        Err(_) => {
            let (_v, gpu) = detect_gpu_via_wmi();
            SettingState {
                name: "可变刷新率 (VRR/G-Sync/FreeSync)".to_string(),
                enabled: false,
                admin_required: false,
                message: format!("[VRR_NF] {} 不存在 | {}", DX_GLOBAL_SETTINGS_VAL, gpu),
            }
        }
    }
}

pub fn set_vrr_state(enabled: bool) -> Result<SettingState, String> {
    let cur =
        read_registry_string("HKCU", DX_GPU_PREFS_KEY, DX_GLOBAL_SETTINGS_VAL).unwrap_or_default();
    let upd = update_dx_setting(&cur, "VRROptimizeEnable", enabled);
    write_registry_string("HKCU", DX_GPU_PREFS_KEY, DX_GLOBAL_SETTINGS_VAL, &upd)?;
    Ok(SettingState {
        name: "可变刷新率 (VRR/G-Sync/FreeSync)".to_string(),
        enabled,
        admin_required: false,
        message: format!(
            "VRR 已{} (VRROptimizeEnable={})",
            if enabled { "启用" } else { "禁用" },
            if enabled { "1" } else { "0" }
        ),
    })
}

// ============================================================================
// Mouse Precision (Enhance Pointer Precision)
// Uses SystemParametersInfoW(SPI_GETMOUSE/SPI_SETMOUSE)
// per Windows spec: "必须使用 SystemParametersInfo，纯注册表写入无效"
// ============================================================================

pub fn get_mouse_precision_state() -> SettingState {
    let mut params: [i32; 3] = [0; 3];
    let ok = unsafe {
        extern "system" {
            fn SystemParametersInfoW(
                ui_action: u32,
                ui_param: u32,
                pv_param: *mut std::ffi::c_void,
                f_win_ini: u32,
            ) -> i32;
        }
        SystemParametersInfoW(0x0003, 0, params.as_mut_ptr() as *mut std::ffi::c_void, 0)
        // SPI_GETMOUSE
    };

    if ok == 0 {
        let reg_enabled = read_registry_dword("HKCU", "Control Panel\\Mouse", "MouseSpeed")
            .map(|v| v == 1)
            .unwrap_or(false);
        SettingState {
            name: "鼠标精准度 (提高指针精确度)".to_string(),
            enabled: reg_enabled,
            admin_required: false,
            message: if reg_enabled {
                "提高指针精确度已启用 (注册表回退)".to_string()
            } else {
                "无法通过系统API读取，注册表显示已禁用".to_string()
            },
        }
    } else {
        let enabled = params[2] != 0;
        SettingState {
            name: "鼠标精准度 (提高指针精确度)".to_string(),
            enabled,
            admin_required: false,
            message: if enabled {
                format!(
                    "已启用 (Threshold1={}, Threshold2={}, Speed={})",
                    params[0], params[1], params[2]
                )
            } else {
                format!(
                    "已禁用 (Thresholds: {}/{} Speed={})",
                    params[0], params[1], params[2]
                )
            },
        }
    }
}

pub fn set_mouse_precision(enabled: bool) -> Result<SettingState, String> {
    // Per Windows spec: SPI_SETMOUSE with SPIF_UPDATEINIFILE | SPIF_SENDCHANGE is the ONLY valid way.
    // SPI_GETMOUSE=0x0003, SPI_SETMOUSE=0x0004
    // SPIF_UPDATEINIFILE=0x0001, SPIF_SENDCHANGE=0x0002
    let mut params: [i32; 3] = if enabled { [6, 10, 1] } else { [0, 0, 0] };
    let ok = unsafe {
        extern "system" {
            fn SystemParametersInfoW(
                ui_action: u32,
                ui_param: u32,
                pv_param: *mut std::ffi::c_void,
                f_win_ini: u32,
            ) -> i32;
        }
        SystemParametersInfoW(
            0x0004,
            0,
            params.as_mut_ptr() as *mut std::ffi::c_void,
            0x0001 | 0x0002,
        ) // SPI_SETMOUSE | SPIF_UPDATEINIFILE | SPIF_SENDCHANGE
    };
    if ok == 0 {
        return Err("SystemParametersInfoW(SPI_SETMOUSE) 调用失败".to_string());
    }
    Ok(SettingState {
        name: "鼠标精准度".to_string(),
        enabled,
        admin_required: false,
        message: format!(
            "鼠标精准度已{} (SPI_SETMOUSE)",
            if enabled { "启用" } else { "禁用" }
        ),
    })
}

// ============================================================================
// Power Plans (registry-based reading, powercfg for writing only)
// ============================================================================

/// Win11 overlay GUIDs — not real power plans, must be filtered out
const OVERLAY_GUIDS: &[&str] = &[
    "961cc777-2547-4f9d-8174-7d86181b8a7a",
    "ded574b5-45a0-4f42-8737-46345c09c238",
];

/// Known power plan GUID → Chinese name mapping
fn power_plan_name(guid: &str, raw_name: &str) -> String {
    match guid.to_lowercase().as_str() {
        "381b4222-f694-41f0-9685-ff5bb260df2e" => "平衡".to_string(),
        "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c" => "高性能".to_string(),
        "a1841308-3541-4fab-bc81-f71556f20b4a" => "节能".to_string(),
        "e9a42b02-d5df-448d-aa00-03f14749eb61" => "卓越性能".to_string(),
        _ => {
            // Parse friendly name from registry — may be "@C:\\Windows\\...dll,-15,Balanced"
            let name = if let Some(idx) = raw_name.rfind(',') {
                raw_name[idx + 1..].trim().to_string()
            } else {
                raw_name.to_string()
            };
            // English → Chinese fallback
            match name.to_lowercase().as_str() {
                "balanced" => "平衡".into(),
                "high performance" => "高性能".into(),
                "power saver" => "节能".into(),
                "ultimate performance" => "卓越性能".into(),
                // 子键名可能短于 8 字节或含多字节字符，直接 `guid[..8]` 会越界/
                // 落在字符边界外并 panic（ADR-0001 D5）：不能取前 8 位时回退整串。
                _ if name.is_empty() => guid.get(..8).unwrap_or(guid).to_string(),
                _ => name,
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PowerPlan {
    pub guid: String,
    pub name: String,
    pub is_active: bool,
}

const POWER_SCHEMES_KEY: &str = "SYSTEM\\CurrentControlSet\\Control\\Power\\User\\PowerSchemes";

/// Read power plans from registry (zero process spawn, no console flash, no localized text parsing).
/// Active plan determined by reading HKLM\...\PowerSchemes\ActivePowerScheme REG_SZ value.
pub fn get_power_plans() -> Result<Vec<PowerPlan>, String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let schemes = hklm
        .open_subkey_with_flags(POWER_SCHEMES_KEY, KEY_READ | KEY_WOW64_64KEY)
        .map_err(|e| format!("[REG] 无法打开 PowerSchemes 注册表: {}", e))?;

    // Read active plan GUID
    let active_guid: String = schemes.get_value("ActivePowerScheme").unwrap_or_default();

    let mut plans = Vec::new();

    // Enumerate subkeys — each is a power plan GUID
    for guid_result in schemes.enum_keys() {
        let guid = match guid_result {
            Ok(g) => g,
            Err(_) => continue,
        };

        // Skip Win11 overlay GUIDs
        if OVERLAY_GUIDS.contains(&guid.to_lowercase().as_str()) {
            continue;
        }

        let is_active = guid.eq_ignore_ascii_case(&active_guid);

        // Read friendly name from subkey
        let raw_name: String = if let Ok(sub) = schemes.open_subkey_with_flags(&guid, KEY_READ) {
            sub.get_value("FriendlyName").unwrap_or_default()
        } else {
            String::new()
        };

        let name = power_plan_name(&guid, &raw_name);

        // 跳过包含 "Overlay" 的非真实电源计划
        if name.to_lowercase().contains("overlay") {
            continue;
        }

        plans.push(PowerPlan {
            guid: guid.to_lowercase(),
            name,
            is_active,
        });
    }

    Ok(plans)
}

/// Set active power plan via powercfg (write-only, user-triggered).（legacy 旧实现）
fn set_power_plan_legacy(guid: &str) -> Result<(), String> {
    let output = Command::new("powercfg")
        .args(["/setactive", guid])
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| format!("powercfg /setactive failed: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "set_power_plan failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// 切换激活电源计划（P7）
///
/// 过渡期由 `legacy-data-source` feature 控制回退旧 powercfg 实现；
/// 默认走纯 Rust 驱动（secm-datasource::power，PowerSetActiveScheme，与 powercfg /setactive 等价）。
pub fn set_power_plan(guid: &str) -> Result<(), String> {
    if crate::ds_util::use_legacy() {
        return set_power_plan_legacy(guid);
    }

    crate::datasource::power::set_active_scheme(guid)
        .map_err(|e| crate::ds_util::log_err("settings", "切换电源计划", &e))
}

/// 删除电源计划（P7 扩展：powrprof.PowerDeleteScheme，与 powercfg /delete 等价）
///
/// 激活中的计划不可删除（前端禁用删除按钮，后端亦由 API 返回错误兜底）；
/// 删除后不可恢复，需重新创建（前端有二次确认）。
pub fn delete_power_plan(guid: &str) -> Result<(), String> {
    crate::datasource::power::delete_scheme(guid)
        .map_err(|e| crate::ds_util::log_err("settings", "删除电源计划", &e))
}

// ============================================================================
// 异类线程调度策略（混合架构 CPU 大小核调度：Intel 12代+ / AMD 大小核）
// ============================================================================

/// 处理器电源管理子组 GUID
const SUBGROUP_PROCESSOR: &str = "54533251-82be-4824-96c1-47b60b740d00";
/// 异类线程调度策略设置 GUID
const HETERO_THREAD_POLICY_GUID: &str = "93b8b6dc-0698-4d1c-9ee4-0644e900c85d";
/// 异类短运行线程调度策略设置 GUID
const HETERO_SHORT_THREAD_POLICY_GUID: &str = "465e1f50-b610-473a-ab58-00d1077dc418";

/// 异类调度策略 AC/DC 当前值（None = 使用系统默认，未显式设置）
#[derive(Debug, Clone, Serialize)]
pub struct HeteroPolicies {
    pub supported: bool,
    pub thread_ac: Option<u32>,
    pub thread_dc: Option<u32>,
    pub short_ac: Option<u32>,
    pub short_dc: Option<u32>,
}

/// 读取活动电源方案下异类调度策略的 AC/DC 值（注册表读取，零进程 spawn）
///
/// 取值枚举（与电源选项面板一致）：
///   0=所有处理器 1=高性能处理器 2=首选高性能处理器 3=高效处理器 4=首选高效处理器 5=自动
pub fn get_hetero_policies() -> Result<HeteroPolicies, String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let schemes = hklm
        .open_subkey_with_flags(POWER_SCHEMES_KEY, KEY_READ | KEY_WOW64_64KEY)
        .map_err(|e| format!("[REG] 无法打开 PowerSchemes: {}", e))?;
    let power_settings = hklm
        .open_subkey_with_flags(
            "SYSTEM\\CurrentControlSet\\Control\\Power\\PowerSettings",
            KEY_READ | KEY_WOW64_64KEY,
        )
        .ok();
    let power_settings = power_settings.as_ref();

    let active_guid: String = schemes.get_value("ActivePowerScheme").unwrap_or_default();

    /// 读取方案下某设置的 AC/DC 索引（未显式设置时回退到系统默认值）
    fn read_index(
        schemes: &winreg::RegKey,
        power_settings: &winreg::RegKey,
        active: &str,
        setting: &str,
        ac: bool,
    ) -> Option<u32> {
        let value_name = if ac {
            "ACSettingIndex"
        } else {
            "DCSettingIndex"
        };
        let fallback = if ac {
            "DefaultACSettingIndex"
        } else {
            "DefaultDCSettingIndex"
        };
        if active.is_empty() {
            return None;
        }
        // 1) 活动方案下的显式值
        let key = format!("{}\\{}\\{}", active, SUBGROUP_PROCESSOR, setting);
        if let Ok(sub) = schemes.open_subkey_with_flags(&key, KEY_READ) {
            if let Ok(v) = sub.get_value::<u32, _>(value_name) {
                return Some(v);
            }
        }
        // 2) 回退：系统默认值（PowerSettings 全局定义）
        let default_key = format!("{}\\{}", SUBGROUP_PROCESSOR, setting);
        if let Ok(sub) = power_settings.open_subkey_with_flags(&default_key, KEY_READ) {
            if let Ok(v) = sub.get_value::<u32, _>(fallback) {
                return Some(v);
            }
        }
        None
    }

    let read = |setting: &str, ac: bool| -> Option<u32> {
        match power_settings {
            Some(ps) => read_index(&schemes, ps, &active_guid, setting, ac),
            None => read_index(&schemes, &schemes, &active_guid, setting, ac),
        }
    };

    // 非混合架构 CPU 上这两个设置键不存在（或读取失败）→ 标记不支持
    let thread_ac = read(HETERO_THREAD_POLICY_GUID, true);
    let thread_dc = read(HETERO_THREAD_POLICY_GUID, false);
    let short_ac = read(HETERO_SHORT_THREAD_POLICY_GUID, true);
    let short_dc = read(HETERO_SHORT_THREAD_POLICY_GUID, false);

    let supported = thread_ac.is_some() || thread_dc.is_some();

    Ok(HeteroPolicies {
        supported,
        thread_ac,
        thread_dc,
        short_ac,
        short_dc,
    })
}

/// 设置异类调度策略（AC/DC 同步写入 + 激活当前方案）（legacy 旧实现：powercfg）
///
/// `kind`: "thread" = 异类线程调度策略, "short" = 异类短运行线程调度策略
fn set_hetero_policy_legacy(kind: &str, value: u32) -> Result<(), String> {
    let setting = match kind {
        "short" => HETERO_SHORT_THREAD_POLICY_GUID,
        _ => HETERO_THREAD_POLICY_GUID,
    };

    let run = |args: &[&str]| -> Result<(), String> {
        let out = Command::new("powercfg")
            .args(args)
            .creation_flags(0x08000000)
            .output()
            .map_err(|e| format!("powercfg 启动失败: {}", e))?;
        if !out.status.success() {
            return Err(format!(
                "powercfg {} 失败: {}",
                args[0],
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(())
    };

    run(&[
        "/setacvalueindex",
        "SCHEME_CURRENT",
        SUBGROUP_PROCESSOR,
        setting,
        &value.to_string(),
    ])?;
    run(&[
        "/setdcvalueindex",
        "SCHEME_CURRENT",
        SUBGROUP_PROCESSOR,
        setting,
        &value.to_string(),
    ])?;
    run(&["/s", "SCHEME_CURRENT"])?;
    Ok(())
}

/// 设置异类调度策略（P8，AC/DC 同步写入 + 重新激活当前方案使更改生效）
///
/// 过渡期由 `legacy-data-source` feature 控制回退旧 powercfg 实现；
/// 默认走纯 Rust 驱动（secm-datasource::power：PowerWriteACValueIndex /
/// PowerWriteDCValueIndex / PowerSetActiveScheme，与 powercfg /setacvalueindex 等等价）。
///
/// `kind`: "thread" = 异类线程调度策略, "short" = 异类短运行线程调度策略
pub fn set_hetero_policy(kind: &str, value: u32) -> Result<(), String> {
    if crate::ds_util::use_legacy() {
        return set_hetero_policy_legacy(kind, value);
    }

    let setting = match kind {
        "short" => HETERO_SHORT_THREAD_POLICY_GUID,
        _ => HETERO_THREAD_POLICY_GUID,
    };

    use crate::datasource::power as ds_power;

    // 写 AC/DC 值索引（None = 当前激活方案，对应 SCHEME_CURRENT 语义）
    ds_power::write_ac_value(None, SUBGROUP_PROCESSOR, setting, value)
        .map_err(|e| crate::ds_util::log_err("settings", "写入异类策略 AC 值", &e))?;
    ds_power::write_dc_value(None, SUBGROUP_PROCESSOR, setting, value)
        .map_err(|e| crate::ds_util::log_err("settings", "写入异类策略 DC 值", &e))?;

    // 重新激活当前方案（对应 powercfg /s SCHEME_CURRENT，使更改立即生效）
    match ds_power::get_active_scheme() {
        Ok(Some(active)) => {
            ds_power::set_active_scheme(&active)
                .map_err(|e| crate::ds_util::log_err("settings", "应用电源策略", &e))?;
        }
        Ok(None) => {
            crate::debug_warn!("settings", "异类策略写入成功但无激活方案可刷新");
        }
        Err(e) => {
            return Err(crate::ds_util::log_err("settings", "获取当前电源计划", &e));
        }
    }
    Ok(())
}

/// Create Ultimate Performance plan via powercfg (write-only, user-triggered).（legacy 旧实现）
fn enable_ultimate_performance_legacy() -> Result<String, String> {
    // Check if already exists via registry
    let existing = get_power_plans().unwrap_or_default();
    for plan in &existing {
        if plan.guid.contains("e9a42b02-d5df-448d-aa00-03f14749eb61") {
            return Ok(format!("卓越性能电源计划已存在: {}", plan.guid));
        }
    }
    let output = Command::new("powercfg")
        .args(["/duplicatescheme", "e9a42b02-d5df-448d-aa00-03f14749eb61"])
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| format!("powercfg /duplicatescheme failed: {}", e))?;
    if !output.status.success() {
        Err(format!(
            "导入卓越性能失败: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    } else {
        Ok(format!(
            "卓越性能已导入: {}",
            String::from_utf8_lossy(&output.stdout).trim()
        ))
    }
}

/// 导入卓越性能电源计划（P9）
///
/// 过渡期由 `legacy-data-source` feature 控制回退旧 powercfg 实现；
/// 默认走纯 Rust 驱动（secm-datasource::power::duplicate_scheme，
/// 即 PowerDuplicateScheme，与 powercfg /duplicatescheme 等价）。
/// 新实现直接取 API 返回的新 GUID（旧实现解析 powercfg stdout，更脆弱）。
pub fn enable_ultimate_performance() -> Result<String, String> {
    if crate::ds_util::use_legacy() {
        return enable_ultimate_performance_legacy();
    }

    // 先查是否已存在（注册表读路径，与旧实现一致）
    let existing = get_power_plans().unwrap_or_default();
    for plan in &existing {
        if plan.guid.contains("e9a42b02-d5df-448d-aa00-03f14749eb61") {
            return Ok(format!("卓越性能电源计划已存在: {}", plan.guid));
        }
    }

    crate::datasource::power::duplicate_scheme("e9a42b02-d5df-448d-aa00-03f14749eb61")
        .map(|guid| format!("卓越性能已导入: {}", guid))
        .map_err(|e| crate::ds_util::log_err("settings", "导入卓越性能电源计划", &e))
}

// ============================================================================
// Service Management
// ============================================================================

pub const SERVICE_LIST: &[(&str, &str)] = &[
    ("XblAuthManager", "Xbox Live 身份验证管理器"),
    ("XboxNetApiSvc", "Xbox Live 网络服务"),
    ("XboxGipSvc", "Xbox Accessory 管理服务"),
    ("GameDVR", "游戏 DVR 和广播"),
    ("BcastDVRUserService", "游戏 DVR 用户服务"),
    ("WSearch", "Windows Search"),
    ("SysMain", "SysMain (Superfetch)"),
    ("DiagTrack", "诊断跟踪服务"),
    ("dmwappushservice", "设备管理 WAP 推送"),
    ("MapsBroker", "已下载地图管理器"),
    ("lfsvc", "地理位置服务"),
    ("Fax", "传真服务"),
    ("wisvc", "Windows 预览体验计划服务"),
    ("WMPNetworkSvc", "Windows Media Player 网络共享"),
    ("RemoteRegistry", "远程注册表"),
    ("WerSvc", "Windows 错误报告服务"),
];

#[derive(Debug, Clone, Serialize)]
pub struct ServiceInfo {
    pub name: String,
    pub display_name: String,
    pub status: String,
    pub start_type: String,
}

struct ServiceInfoRaw {
    status: String,
    start_type: String,
}

/// 查询 16 个预设服务（legacy 旧实现：sc query / sc qc）
fn get_services_legacy() -> Vec<ServiceInfo> {
    let mut services = Vec::new();
    for (svc_name, display_name) in SERVICE_LIST {
        let status = query_service(svc_name);
        services.push(ServiceInfo {
            name: svc_name.to_string(),
            display_name: display_name.to_string(),
            status: status.status,
            start_type: status.start_type,
        });
    }
    services
}

/// 查询 16 个预设服务（P11）
///
/// 过渡期由 `legacy-data-source` feature 控制回退旧 sc 实现；
/// 默认走纯 Rust 驱动（secm-datasource::service::query_service，
/// 即 QueryServiceStatusEx + QueryServiceConfigW，与 sc query/qc 等价）。
/// 单服务失败不阻塞整体：降级为该服务 "Unknown"/"未知"（与现状 sc 无输出一致）。
pub fn get_services() -> Vec<ServiceInfo> {
    if crate::ds_util::use_legacy() {
        return get_services_legacy();
    }

    let mut services = Vec::new();
    for (svc_name, display_name) in SERVICE_LIST {
        match crate::datasource::service::query_service(svc_name) {
            Ok(Some(info)) => services.push(ServiceInfo {
                name: svc_name.to_string(),
                display_name: display_name.to_string(),
                status: info.status,
                start_type: info.start_type,
            }),
            Ok(None) => {
                // 服务不存在（与 sc query 找不到服务一致）
                services.push(ServiceInfo {
                    name: svc_name.to_string(),
                    display_name: display_name.to_string(),
                    status: "Stopped".into(),
                    start_type: "未知".into(),
                });
            }
            Err(e) => {
                let msg = crate::ds_util::log_err("settings", "查询预设服务", &e);
                crate::debug_warn!("settings", "查询服务 '{}' 降级: {}", svc_name, msg);
                services.push(ServiceInfo {
                    name: svc_name.to_string(),
                    display_name: display_name.to_string(),
                    status: "Unknown".into(),
                    start_type: "未知".into(),
                });
            }
        }
    }
    services
}

/// 解码 sc 命令输出：复用 `proc_util::decode_ps`（ADR-0001 D14）。
///
/// 原实现为 `String::from_utf8(bytes.to_vec())` 再回退 GBK，语义与 decode_ps 一致，
/// 但多一次堆分配；合并后行为不变且只维护一份编码回退策略。
fn decode_sc_output(bytes: &[u8]) -> String {
    crate::proc_util::decode_ps(bytes)
}

/// 枚举系统所有服务（legacy 旧实现：sc query type= service state= all + 8 线程并行 sc qc）
fn list_all_services_legacy() -> Vec<ServiceInfo> {
    // 1. 一次枚举：名称 / 显示名 / 状态
    let output = Command::new("sc")
        .args(["query", "type=", "service", "state=", "all"])
        .creation_flags(0x08000000)
        .output();
    let Ok(out) = output else {
        return Vec::new();
    };
    // GBK 解码（中文系统 sc 输出非 UTF-8，直接 from_utf8_lossy 会乱码）
    let text = decode_sc_output(&out.stdout) + &decode_sc_output(&out.stderr);

    // 解析多服务块：SERVICE_NAME / DISPLAY_NAME / STATE
    let mut entries: Vec<(String, String, String)> = Vec::new(); // (name, display, status)
    let mut cur: Option<(String, String, String)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("SERVICE_NAME:") {
            if let Some(e) = cur.take() {
                entries.push(e);
            }
            cur = Some((
                rest.trim().to_string(),
                String::new(),
                "Unknown".to_string(),
            ));
        } else if let Some(rest) = trimmed.strip_prefix("DISPLAY_NAME:") {
            if let Some(e) = cur.as_mut() {
                e.1 = rest.trim().to_string();
            }
        } else if trimmed.contains("STATE") {
            if let Some(e) = cur.as_mut() {
                e.2 = if trimmed.contains("RUNNING") {
                    "Running".into()
                } else if trimmed.contains("STOPPED") {
                    "Stopped".into()
                } else if trimmed.contains("STOP_PENDING") {
                    "Stopping".into()
                } else if trimmed.contains("START_PENDING") {
                    "Starting".into()
                } else if trimmed.contains("PAUSED") {
                    "Paused".into()
                } else {
                    "Unknown".into()
                };
            }
        }
    }
    if let Some(e) = cur.take() {
        entries.push(e);
    }

    // 2. 并行查询启动类型
    let names: Vec<String> = entries.iter().map(|e| e.0.clone()).collect();
    let start_types = query_start_types_parallel(&names);

    entries
        .into_iter()
        .zip(start_types)
        .map(|((name, display, status), start_type)| ServiceInfo {
            name,
            display_name: display,
            status,
            start_type,
        })
        .collect()
}

/// 枚举系统所有服务（P10）
///
/// 过渡期由 `legacy-data-source` feature 控制回退旧 sc 实现；
/// 默认走纯 Rust 驱动（secm-datasource::service::enum_services，
/// 即 EnumServicesStatusExW 一次返回全部名称/显示名/状态），
/// 启动类型仍保留 8 线程并行补全（但为内存内 API 调用，比 spawn `sc qc` 快 2-3 个数量级）。
/// 失败时返回空表（与现状 sc 无输出一致），并打 R8 日志。
pub fn list_all_services() -> Vec<ServiceInfo> {
    if crate::ds_util::use_legacy() {
        return list_all_services_legacy();
    }

    let entries = match crate::datasource::service::enum_services() {
        Ok(list) => list,
        Err(e) => {
            let msg = crate::ds_util::log_err("settings", "枚举系统服务", &e);
            crate::debug_warn!("settings", "服务枚举降级为空表: {}", msg);
            return Vec::new();
        }
    };

    // 并行补全启动类型（8 线程，内存内 QueryServiceConfigW 调用）
    let names: Vec<String> = entries.iter().map(|s| s.name.clone()).collect();
    let start_types = query_start_types_parallel(&names);

    entries
        .into_iter()
        .zip(start_types)
        .map(|(info, start_type)| ServiceInfo {
            name: info.name,
            display_name: info.display_name,
            status: info.status,
            start_type,
        })
        .collect()
}

/// 并行查询启动类型（sc qc），8 线程分摊数百次子进程调用
fn query_start_types_parallel(names: &[String]) -> Vec<String> {
    let n = names.len();
    if n == 0 {
        return Vec::new();
    }
    let threads = 8usize.min(n);
    let chunk = n.div_ceil(threads);
    let pairs: Vec<(usize, &str)> = names
        .iter()
        .enumerate()
        .map(|(i, s)| (i, s.as_str()))
        .collect();
    let mut result = vec!["未知".to_string(); n];
    std::thread::scope(|scope| {
        let handles: Vec<_> = pairs
            .chunks(chunk)
            .map(|c| {
                scope.spawn(move || {
                    let mut local: Vec<(usize, String)> = Vec::new();
                    for (idx, name) in c {
                        local.push((*idx, query_start_type(name)));
                    }
                    local
                })
            })
            .collect();
        for h in handles {
            if let Ok(local) = h.join() {
                for (idx, st) in local {
                    result[idx] = st;
                }
            }
        }
    });
    result
}

/// 查询单个服务启动类型（legacy 旧实现：sc qc —— sc query 不输出 START_TYPE）
fn query_start_type_legacy(name: &str) -> String {
    let output = Command::new("sc")
        .args(["qc", name])
        .creation_flags(0x08000000)
        .output();
    let Ok(out) = output else {
        return "未知".to_string();
    };
    let text = decode_sc_output(&out.stdout) + &decode_sc_output(&out.stderr);
    if text.contains("AUTO_START") {
        "自动".into()
    } else if text.contains("DEMAND_START") {
        "手动".into()
    } else if text.contains("DISABLED") {
        "已禁用".into()
    } else {
        "未知".into()
    }
}

/// 查询单个服务启动类型（P10/P11 辅助，内存内 QueryServiceConfigW 调用）
///
/// 过渡期由 `legacy-data-source` feature 控制回退旧 sc qc 实现；
/// 默认走纯 Rust 驱动（secm-datasource::service::start_type_of）。
/// 失败/服务不存在时降级 "未知"（与现状 sc qc 无输出一致）。
fn query_start_type(name: &str) -> String {
    if crate::ds_util::use_legacy() {
        return query_start_type_legacy(name);
    }
    crate::datasource::service::start_type_of(name)
}

fn query_service(name: &str) -> ServiceInfoRaw {
    // 注意：sc query 输出不包含 START_TYPE（仅 STATE），启动类型需用 sc qc 查询配置
    let run = |args: &[&str]| -> String {
        Command::new("sc")
            .args(args)
            .creation_flags(0x08000000)
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr)
            })
            .unwrap_or_default()
    };
    let q_text = run(&["query", name]);
    let qc_text = run(&["qc", name]);
    let status = if q_text.contains("RUNNING") {
        "Running".into()
    } else if q_text.contains("STOPPED") {
        "Stopped".into()
    } else if q_text.contains("STOP_PENDING") {
        "Stopping".into()
    } else if q_text.contains("START_PENDING") {
        "Starting".into()
    } else if q_text.contains("PAUSED") {
        "Paused".into()
    } else {
        "Unknown".into()
    };
    let start_type = if qc_text.contains("AUTO_START") {
        "自动".into()
    } else if qc_text.contains("DEMAND_START") {
        "手动".into()
    } else if qc_text.contains("DISABLED") {
        "已禁用".into()
    } else {
        "未知".into()
    };
    ServiceInfoRaw { status, start_type }
}

/// 启动服务（legacy 旧实现：sc start）
fn start_service_legacy(name: &str) -> Result<String, String> {
    if !super::cleanup::is_admin() {
        return Err("需要管理员权限".to_string());
    }
    let output = Command::new("sc")
        .args(["start", name])
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| format!("sc start failed: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if output.status.success() {
        Ok(format!("{} started: {}", name, stdout.trim()))
    } else {
        Err(format!(
            "start {} failed: {} {}",
            name,
            stdout,
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// 启动服务（P11，advapi32 StartServiceW）
///
/// 过渡期由 `legacy-data-source` feature 控制回退旧 sc 实现；
/// 默认走纯 Rust 驱动（secm-datasource::service::start_service）。
/// 需管理员权限（现状 is_admin 前置检查保留；API 层对 ACCESS_DENIED 再兜底一次）。
pub fn start_service(name: &str) -> Result<String, String> {
    if crate::ds_util::use_legacy() {
        return start_service_legacy(name);
    }
    if !super::cleanup::is_admin() {
        return Err("需要管理员权限".to_string());
    }
    crate::datasource::service::start_service(name)
        .map(|_| format!("{} 已启动", name))
        .map_err(|e| crate::ds_util::log_err("settings", "启动服务", &e))
}

/// 停止服务（legacy 旧实现：sc stop）
fn stop_service_legacy(name: &str) -> Result<String, String> {
    if !super::cleanup::is_admin() {
        return Err("需要管理员权限".to_string());
    }
    let output = Command::new("sc")
        .args(["stop", name])
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| format!("sc stop failed: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if output.status.success() {
        Ok(format!("{} stopped: {}", name, stdout.trim()))
    } else {
        Err(format!(
            "stop {} failed: {} {}",
            name,
            stdout,
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// 停止服务（P11，advapi32 ControlService）
///
/// 过渡期由 `legacy-data-source` feature 控制回退旧 sc 实现；
/// 默认走纯 Rust 驱动（secm-datasource::service::stop_service）。
/// 需管理员权限；服务已停止时幂等成功（与 sc stop 行为一致）。
pub fn stop_service(name: &str) -> Result<String, String> {
    if crate::ds_util::use_legacy() {
        return stop_service_legacy(name);
    }
    if !super::cleanup::is_admin() {
        return Err("需要管理员权限".to_string());
    }
    crate::datasource::service::stop_service(name)
        .map(|_| format!("{} 已停止", name))
        .map_err(|e| crate::ds_util::log_err("settings", "停止服务", &e))
}

/// 设置服务启动类型（legacy 旧实现：sc config start= X）
///
/// 设置为手动/禁用后，服务不会随系统启动自动拉起；配合停止操作可防止服务自动重启。
/// 注意：sc config 的 `start=` 参数后必须有空格（sc 命令特性）。
fn set_service_start_type_legacy(name: &str, start_type: &str) -> Result<String, String> {
    if !super::cleanup::is_admin() {
        return Err("需要管理员权限".to_string());
    }
    let sc_type = match start_type {
        "auto" => "auto",
        "manual" => "demand",
        "disabled" => "disabled",
        _ => return Err(format!("无效的启动类型: {}", start_type)),
    };
    let label = match start_type {
        "auto" => "自动",
        "manual" => "手动",
        _ => "禁用",
    };
    let output = Command::new("sc")
        .args(["config", name, "start=", sc_type])
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| format!("sc config failed: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if output.status.success() {
        Ok(format!("{} 启动类型已设置为 {}", name, label))
    } else {
        Err(format!(
            "设置 {} 启动类型失败: {} {}",
            name,
            stdout,
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// 设置服务启动类型（P11，advapi32 ChangeServiceConfigW）
///
/// 过渡期由 `legacy-data-source` feature 控制回退旧 sc 实现；
/// 默认走纯 Rust 驱动（secm-datasource::service::set_service_start_type）。
/// 参数（与前端契约一致）："auto" 自动 / "manual" 手动 / "disabled" 禁用。
/// 需管理员权限（现状 is_admin 前置检查保留；API 层对 ACCESS_DENIED 再兜底一次）。
pub fn set_service_start_type(name: &str, start_type: &str) -> Result<String, String> {
    if crate::ds_util::use_legacy() {
        return set_service_start_type_legacy(name, start_type);
    }
    if !super::cleanup::is_admin() {
        return Err("需要管理员权限".to_string());
    }
    let label = match start_type {
        "auto" => "自动",
        "manual" => "手动",
        _ => "禁用",
    };
    crate::datasource::service::set_service_start_type(name, start_type)
        .map(|_| format!("{} 启动类型已设置为 {}", name, label))
        .map_err(|e| crate::ds_util::log_err("settings", "设置服务启动类型", &e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0001 D5 回归：未知计划的 GUID 短于 8 字节时，
    /// `guid[..8]` 越界 panic；修复后应回退整串。
    #[test]
    fn 电源计划名_短guid不panic() {
        // 空友好名走 `_ if name.is_empty()` 分支；GUID 只有 3 字节
        assert_eq!(power_plan_name("abc", ""), "abc");
        // 5 字节：仍不足 8
        assert_eq!(power_plan_name("abcd1", ""), "abcd1");
        // 恰好 8 字节：取前 8 位即全串
        assert_eq!(power_plan_name("abcdefgh", ""), "abcdefgh");
        // 长 GUID：取前 8 位
        assert_eq!(power_plan_name("0123456789abcdef", ""), "01234567");
    }

    /// 多字节字符的 GUID：按字节切分曾可能落在字符边界外。
    #[test]
    fn 电源计划名_多字节guid不panic() {
        // 3 个中文字符 = 9 字节，旧逻辑 `guid[..8]` 会落在字符边界外 panic
        let name = power_plan_name("电源计划", "");
        assert!(!name.is_empty(), "应返回可读名称而非 panic");
    }

    #[test]
    fn test_get_power_plans() {
        let plans = get_power_plans();
        assert!(plans.is_ok());
        let plans = plans.unwrap();
        assert!(!plans.is_empty());
        assert!(plans.iter().any(|p| p.is_active));
    }

    #[test]
    fn test_service_list() {
        assert_eq!(SERVICE_LIST.len(), 16);
    }

    #[test]
    fn test_parse_dx_setting() {
        let s = "SwapEffectUpgradeEnable=1;VRROptimizeEnable=0;";
        assert_eq!(parse_dx_setting(s, "SwapEffectUpgradeEnable"), Some(true));
        assert_eq!(parse_dx_setting(s, "VRROptimizeEnable"), Some(false));
        assert_eq!(parse_dx_setting(s, "NotFound"), None);
    }

    #[test]
    fn test_update_dx_setting() {
        let s = "SwapEffectUpgradeEnable=1;VRROptimizeEnable=0;";
        let s2 = update_dx_setting(s, "VRROptimizeEnable", true);
        assert!(s2.contains("VRROptimizeEnable=1"));
        assert!(s2.contains("SwapEffectUpgradeEnable=1")); // preserved
    }
}
