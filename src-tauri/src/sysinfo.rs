/// 系统信息模块 — 静态 Windows OS 信息采集
///
/// 采集项目：
/// - 产品名称 (Edition)
/// - Build 号 + UBR
/// - 系统架构 (32/64 位)
/// - 安装日期
/// - 激活状态
/// - 最新安装的补丁 (KB)
/// - 启动模式 (UEFI / BIOS)
///
/// 所有采集函数独立运行，单字段失败不影响其他字段。
use serde::Serialize;
use std::os::windows::process::CommandExt;

// ============================================================================
// 类型定义
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct SystemInfo {
    /// 产品名称，如 "Windows 11 专业版"
    pub edition: String,
    /// Build 号，如 "22631"
    pub build_number: String,
    /// 更新版本号 (UBR)，如 "4602"
    pub ubr: String,
    /// 系统架构，如 "64 位"
    pub arch: String,
    /// 系统安装日期，如 "2024-03-15"
    pub install_date: String,
    /// 激活状态信息
    pub activation: ActivationInfo,
    /// 最新安装的补丁
    pub latest_patch: PatchInfo,
    /// 启动模式："UEFI"、"BIOS" 或 "未知"
    pub boot_mode: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivationInfo {
    /// 原始激活状态 (Licensed / Unlicensed / Notification / Grace / Unknown)
    pub status_raw: String,
    /// 中文标签 (已激活 / 未激活 / 通知模式 / 宽限期 / 未知)
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PatchInfo {
    /// KB 编号，如 "KB5039212"
    pub kb: String,
    /// 安装日期，如 "2024-07-09"
    pub date: String,
    /// 补丁标题（英文原文），如 "Security Update"
    pub title_raw: String,
    /// 补丁标题（中文翻译），如 "安全更新"
    pub title_cn: String,
}

// ============================================================================
// 主入口
// ============================================================================

/// 采集全部系统信息（各字段独立获取，单字段失败返回 "无法获取"）
/// 采集 Windows 系统信息（多线程并行采集，各字段独立执行）
pub fn get_system_info() -> SystemInfo {
    std::thread::scope(|s| {
        let edition = s.spawn(|| get_edition());
        let build_number = s.spawn(|| get_build_number());
        let ubr = s.spawn(|| get_ubr());
        let arch = s.spawn(|| get_arch());
        let install_date = s.spawn(|| get_install_date());
        let activation = s.spawn(|| get_activation());
        let latest_patch = s.spawn(|| get_latest_patch());
        let boot_mode = s.spawn(|| get_boot_mode());

        SystemInfo {
            edition: edition.join().unwrap(),
            build_number: build_number.join().unwrap(),
            ubr: ubr.join().unwrap(),
            arch: arch.join().unwrap(),
            install_date: install_date.join().unwrap(),
            activation: activation.join().unwrap(),
            latest_patch: latest_patch.join().unwrap(),
            boot_mode: boot_mode.join().unwrap(),
        }
    })
}

// ============================================================================
// 1. 产品名称 (Edition)
// ============================================================================

/// 从注册表读取 Windows 产品名称
///
/// 获取正确的 Windows 版本名（处理注册表 ProductName 残留旧值问题）
///
/// 微软在升级安装时不会更新 ProductName（如 Win11 系统可能残留 "Windows 10 Pro"），
/// 因此必须用 Build 号判定：Build >= 22000 = Windows 11，否则 Windows 10。
fn get_edition() -> String {
    let build = get_build_number_raw();
    let is_win11 = build >= 22000;

    // 基础版本名（按 Build 判定，不用注册表 ProductName）
    let base = if is_win11 { "Windows 11" } else { "Windows 10" };

    // 用注册表 ProductName 提取 SKU 名（如 "Pro for Workstations"），但要清理掉残留的版本前缀
    let product_name = read_reg_string(
        "HKLM",
        "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
        "ProductName",
    )
    .unwrap_or_default();

    // 从 ProductName 提取版本名之后的部分（去掉 "Windows 10" / "Windows 11" 前缀）
    let sku = product_name
        .trim_start_matches("Windows 10")
        .trim_start_matches("Windows 11")
        .trim()
        .to_string();

    // 组合：Windows 11 <SKU>（或回退到 EditionID 中文映射）
    if !sku.is_empty() {
        format!("{} {}", base, sku)
    } else {
        // 回退：用 EditionID 拼装
        if let Some(edition) = read_reg_string(
            "HKLM",
            "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
            "EditionID",
        ) {
            format!("{} {}", base, edition)
        } else {
            base.to_string()
        }
    }
}

/// 读取原始 Build 号（u32），用于版本判定
fn get_build_number_raw() -> u32 {
    read_reg_string(
        "HKLM",
        "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
        "CurrentBuild",
    )
    .and_then(|s| s.trim().parse::<u32>().ok())
    .unwrap_or(0)
}

// ============================================================================
// 2. Build 号
// ============================================================================

/// 从注册表读取 CurrentBuild
///
/// 路径: HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\CurrentBuild
fn get_build_number() -> String {
    read_reg_string(
        "HKLM",
        "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
        "CurrentBuild",
    )
    .unwrap_or_else(|| "无法获取".into())
}
// ============================================================================
// 3. UBR (Update Build Revision)
// ============================================================================

/// 从注册表读取 UBR
///
/// 路径: HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\UBR
fn get_ubr() -> String {
    read_reg_dword(
        "HKLM",
        "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
        "UBR",
    )
    .map(|v| v.to_string())
    .unwrap_or_else(|| "0".into())
}

// ============================================================================
// 4. 系统架构
// ============================================================================

/// 通过 PROCESSOR_ARCHITECTURE 环境变量判断架构
///
/// 返回值: "64 位" / "32 位" / "ARM64"
fn get_arch() -> String {
    let arch = std::env::var("PROCESSOR_ARCHITECTURE").unwrap_or_default();
    match arch.to_uppercase().as_str() {
        "AMD64" => "64 位".into(),
        "ARM64" => "ARM64".into(),
        "X86" => "32 位".into(),
        "IA64" => "IA-64".into(),
        _ if arch.is_empty() => "无法获取".into(),
        other => other.to_string(),
    }
}

// ============================================================================
// 5. 系统安装日期
// ============================================================================

/// 从注册表读取系统安装日期 (Unix 时间戳)
///
/// 路径: HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\InstallDate
/// 回退: HKLM\SYSTEM\Setup\State → ImageStateDate (字符串格式)
fn get_install_date() -> String {
    // 主路径：Unix 时间戳
    if let Some(ts) = read_reg_dword(
        "HKLM",
        "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
        "InstallDate",
    ) {
        if let Some(dt) = chrono::DateTime::from_timestamp(ts as i64, 0) {
            return dt.format("%Y-%m-%d").to_string();
        }
    }

    // 回退：ImageStateDate (REG_SZ "YYYYMMDD")
    if let Some(raw) = read_reg_string("HKLM", "SYSTEM\\Setup\\State", "ImageStateDate") {
        if let Some(formatted) = parse_image_state_date(&raw) {
            return formatted;
        }
    }

    "无法获取".into()
}

/// 解析 `ImageStateDate`（形如 `"YYYYMMDD"`）为 `"YYYY-MM-DD"`。
///
/// 抽成纯函数以便直接单测（注册表读取无法在单测中构造）。
///
/// **按字符而非字节取前 8 位**（ADR-0001 D5「先证明，再访问」）：
/// 原实现用 `date_str.len() >= 8` 判断后直接 `&date_str[0..4]` 切片，而 `len()` 是
/// **字节**长度。该注册表值可被第三方工具改写；一旦首字节是多字节字符（如中文），
/// 字节切片就落在字符边界之外并 panic——这是跨 FFI 边界不可恢复的失败。
/// 现在改为先收集字符再切片，字符数不足 8 时返回 `None`。
fn parse_image_state_date(raw: &str) -> Option<String> {
    let chars: Vec<char> = raw.trim().chars().collect();
    if chars.len() < 8 {
        return None;
    }
    let y: String = chars[0..4].iter().collect();
    let m: String = chars[4..6].iter().collect();
    let d: String = chars[6..8].iter().collect();
    // 仅接受全数字，避免把 "20xx-…" 之类误当日期
    if y.parse::<u32>().is_ok() && m.parse::<u32>().is_ok() && d.parse::<u32>().is_ok() {
        Some(format!("{}-{}-{}", y, m, d))
    } else {
        None
    }
}

// ============================================================================
// 6. 激活状态
// ============================================================================

/// 通过 PowerShell Get-CimInstance 查询 Windows 激活状态（legacy 旧实现）
///
/// 查询 SoftwareLicensingProduct where ApplicationId=55c92734-... AND PartialProductKey IS NOT NULL
/// 使用 PowerShell 替代 wmic（wmic 在 Win11 24H2+ 已被移除）
/// LicenseStatus 映射:
///   0 → Unlicensed (未激活)
///   1 → Licensed (已激活)
///   2 → OOBGrace (OOB 宽限期)
///   3 → OOTGrace (OOT 宽限期)
///   4 → NonGenuineGrace (非正版宽限期)
///   5 → Notification (通知模式)
///   6 → ExtendedGrace (延长宽限期)
fn get_activation_legacy() -> ActivationInfo {
    let script = r#"Get-CimInstance -ClassName SoftwareLicensingProduct -Filter "ApplicationId='55c92734-d682-4d71-983e-d6ec3f16059f' AND PartialProductKey IS NOT NULL" | Select-Object -ExpandProperty LicenseStatus"#;

    let output = run_powershell_silent(script);
    let status_num: Option<u32> = output
        .lines()
        .find(|l| !l.trim().is_empty())
        .and_then(|l| l.trim().parse::<u32>().ok());

    match status_num {
        Some(0) => ActivationInfo {
            status_raw: "Unlicensed".into(),
            label: "未激活".into(),
        },
        Some(1) => ActivationInfo {
            status_raw: "Licensed".into(),
            label: "已激活".into(),
        },
        Some(2) => ActivationInfo {
            status_raw: "OOBGrace".into(),
            label: "OOB 宽限期".into(),
        },
        Some(3) => ActivationInfo {
            status_raw: "OOTGrace".into(),
            label: "OOT 宽限期".into(),
        },
        Some(4) => ActivationInfo {
            status_raw: "NonGenuineGrace".into(),
            label: "非正版宽限期".into(),
        },
        Some(5) => ActivationInfo {
            status_raw: "Notification".into(),
            label: "通知模式".into(),
        },
        Some(6) => ActivationInfo {
            status_raw: "ExtendedGrace".into(),
            label: "延长宽限期".into(),
        },
        Some(n) => ActivationInfo {
            status_raw: format!("Unknown({})", n),
            label: "未知".into(),
        },
        None => ActivationInfo {
            status_raw: "Unknown".into(),
            label: "无法获取".into(),
        },
    }
}

/// 采集 Windows 激活状态（P1）
///
/// 过渡期（M5 前）由 `legacy-data-source` feature 控制回退旧 PowerShell 实现；
/// 默认走纯 Rust 驱动（secm-datasource::activation）。
/// 失败/无数据时降级为 "Unknown"/"无法获取"（与现状文案一致），并打 R8 日志。
fn get_activation() -> ActivationInfo {
    if crate::ds_util::use_legacy() {
        return get_activation_legacy();
    }

    match crate::datasource::activation::get_activation() {
        Ok(Some(info)) => ActivationInfo {
            status_raw: info.status_raw,
            label: info.label,
        },
        Ok(None) => ActivationInfo {
            status_raw: "Unknown".into(),
            label: "无法获取".into(),
        },
        Err(e) => {
            let msg = crate::ds_util::log_err("sysinfo", "获取激活状态", &e);
            crate::debug_warn!("sysinfo", "激活状态降级: {}", msg);
            ActivationInfo {
                status_raw: "Unknown".into(),
                label: "无法获取".into(),
            }
        }
    }
}

// ============================================================================
// 7. 最新补丁
// ============================================================================

/// 通过 PowerShell Get-CimInstance 查询已安装补丁列表，返回最新一条（legacy 旧实现）
///
/// 查询 Win32_QuickFixEngineering → HotFixID, InstalledOn, Description
/// 按 InstalledOn 降序排序，取第一条
/// 使用 PowerShell 替代 wmic（wmic 在 Win11 24H2+ 已被移除）
fn get_latest_patch_legacy() -> PatchInfo {
    let script = r#"Get-CimInstance -ClassName Win32_QuickFixEngineering | Sort-Object InstalledOn -Descending | Select-Object -First 1 | ForEach-Object { "$($_.HotFixID)|$($_.InstalledOn)|$($_.Description)" }"#;

    let output = run_powershell_silent(script);

    // 解析输出: KB1234567|2026/7/15 0:00:00|Security Update
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return PatchInfo {
            kb: "无".into(),
            date: "—".into(),
            title_raw: "—".into(),
            title_cn: "无补丁记录".into(),
        };
    }

    let parts: Vec<&str> = trimmed.split('|').collect();
    if parts.len() < 3 {
        return PatchInfo {
            kb: "无".into(),
            date: "—".into(),
            title_raw: "—".into(),
            title_cn: "无补丁记录".into(),
        };
    }

    let kb = parts[0].trim().to_string();
    let date_raw = parts[1].trim().to_string();
    let desc = parts[2].trim().to_string();

    // PowerShell 返回的日期格式可能是 "7/15/2026 0:00:00" — 只取日期部分
    let date_part = if let Some(space_idx) = date_raw.find(' ') {
        &date_raw[..space_idx]
    } else {
        &date_raw
    };
    let formatted_date = normalize_date(date_part);

    PatchInfo {
        kb,
        date: formatted_date,
        title_raw: desc.clone(),
        title_cn: translate_patch_title(&desc),
    }
}

/// 采集最新安装的补丁（P2）
///
/// 过渡期由 `legacy-data-source` feature 控制回退旧 PowerShell 实现；
/// 默认走纯 Rust 驱动（secm-datasource::registry，注册表 HotFix 枚举，与 WMI 同源）。
/// 失败/无数据时降级为空占位（与现状文案一致），并打 R8 日志。
fn get_latest_patch() -> PatchInfo {
    if crate::ds_util::use_legacy() {
        return get_latest_patch_legacy();
    }

    match crate::datasource::registry::latest_patch() {
        Ok(Some(p)) => PatchInfo {
            kb: p.kb,
            date: p.date,
            title_raw: p.title_raw,
            title_cn: p.title_cn,
        },
        Ok(None) => PatchInfo {
            kb: "无".into(),
            date: "—".into(),
            title_raw: "—".into(),
            title_cn: "无补丁记录".into(),
        },
        Err(e) => {
            let msg = crate::ds_util::log_err("sysinfo", "获取最新补丁", &e);
            crate::debug_warn!("sysinfo", "最新补丁降级: {}", msg);
            PatchInfo {
                kb: "无".into(),
                date: "—".into(),
                title_raw: "—".into(),
                title_cn: "无补丁记录".into(),
            }
        }
    }
}

/// 将 M/D/YYYY 格式归一化为 YYYY-MM-DD
fn normalize_date(raw: &str) -> String {
    let parts: Vec<&str> = raw.split('/').collect();
    if parts.len() == 3 {
        if let (Ok(m), Ok(d), Ok(y)) = (
            parts[0].parse::<u32>(),
            parts[1].parse::<u32>(),
            parts[2].parse::<u32>(),
        ) {
            return format!("{:04}-{:02}-{:02}", y, m, d);
        }
    }
    // 若已是 YYYY-MM-DD 格式则原样返回
    if raw.len() == 10 && raw.chars().nth(4) == Some('-') {
        return raw.to_string();
    }
    raw.to_string()
}

/// 补丁标题英文 → 中文翻译
fn translate_patch_title(title: &str) -> String {
    let lower = title.to_lowercase();
    if lower.contains("security") && lower.contains("cumulative") {
        "累积安全更新".into()
    } else if lower.contains("security") && lower.contains("quality") {
        "质量安全更新".into()
    } else if lower.contains("security") || lower.contains("security update") {
        "安全更新".into()
    } else if lower.contains("cumulative update") {
        "累积更新".into()
    } else if lower.contains("update rollup") {
        "更新汇总".into()
    } else if lower.contains("service pack") {
        "服务包".into()
    } else if lower.contains("hotfix") {
        "热修复".into()
    } else if lower.contains("update") {
        "更新".into()
    } else if lower.contains("preview") {
        "预览更新".into()
    } else {
        // 无法匹配则返回原标题
        title.to_string()
    }
}

// ============================================================================
// 8. 启动模式 (UEFI / BIOS)
// ============================================================================

/// 通过 kernel32.GetFirmwareType 检测固件类型
///
/// 返回值:
///   0 → Unknown → "未知"
///   1 → BIOS
///   2 → UEFI
///   3 → Max (保留值)
fn get_boot_mode() -> String {
    let mut firmware_type: u32 = 0;

    // SAFETY: GetFirmwareType 是 kernel32 的安全导出函数，传递有效的 &mut u32 指针
    let result = unsafe { GetFirmwareType(&mut firmware_type) };

    if result == 0 {
        // 函数调用失败（可能是 Win7 或 kernel32 无此导出）
        return "无法获取".into();
    }

    match firmware_type {
        1 => "BIOS".into(),
        2 => "UEFI".into(),
        0 => "未知".into(),
        _ => "未知".into(),
    }
}

// SAFETY: kernel32.dll 自 Windows 8+ 提供此函数，签名稳定
#[cfg(windows)]
extern "system" {
    fn GetFirmwareType(firmware_type: *mut u32) -> i32;
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 读取注册表 REG_SZ 字符串值
fn read_reg_string(hive: &str, key: &str, value: &str) -> Option<String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkey = match hive {
        "HKLM" => HKEY_LOCAL_MACHINE,
        "HKCU" => HKEY_CURRENT_USER,
        _ => return None,
    };

    let reg_key = RegKey::predef(hkey);
    let subkey = reg_key
        .open_subkey_with_flags(key, KEY_READ | KEY_WOW64_64KEY)
        .ok()?;
    subkey.get_value::<String, _>(value).ok()
}

/// 读取注册表 REG_DWORD 值
fn read_reg_dword(hive: &str, key: &str, value: &str) -> Option<u32> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkey = match hive {
        "HKLM" => HKEY_LOCAL_MACHINE,
        "HKCU" => HKEY_CURRENT_USER,
        _ => return None,
    };

    let reg_key = RegKey::predef(hkey);
    let subkey = reg_key
        .open_subkey_with_flags(key, KEY_READ | KEY_WOW64_64KEY)
        .ok()?;
    subkey.get_value::<u32, _>(value).ok()
}

/// 静默执行 PowerShell 命令（隐藏控制台窗口），返回 stdout 字符串
///
/// 使用 PowerShell 替代已移除的 wmic.exe（Win11 24H2+）
fn run_powershell_silent(script: &str) -> String {
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0001 D5 回归：`ImageStateDate` 含多字节前导字符时不得 panic。
    ///
    /// 修复前 `&date_str[0..4]` 会落在字符边界外并 panic（`len()` 是字节长度）。
    #[test]
    fn 安装日期_多字节前导不panic() {
        // 4 个中文字符 = 12 字节，满足旧 `len() >= 8`，但字符数只有 4
        assert_eq!(parse_image_state_date("系统安装日期"), None);
        // 前导 1 个中文 + 8 个数字 = 11 字节，旧实现切片 [0..4] 落在多字节字符中间
        assert_eq!(parse_image_state_date("安20240101"), None);
    }

    /// 正常与边界输入：合法返回格式化结果，非法返回 None。
    #[test]
    fn 安装日期_正常与边界() {
        assert_eq!(
            parse_image_state_date("20240101").as_deref(),
            Some("2024-01-01")
        );
        // 前后空白被 trim
        assert_eq!(
            parse_image_state_date("  20240101  ").as_deref(),
            Some("2024-01-01")
        );
        // 超过 8 位：只看前 8 位（与原行为一致）
        assert_eq!(
            parse_image_state_date("20240101999").as_deref(),
            Some("2024-01-01")
        );
        // 不足 8 字符
        assert_eq!(parse_image_state_date("2024"), None);
        assert_eq!(parse_image_state_date(""), None);
        // 非数字
        assert_eq!(parse_image_state_date("20xx0101"), None);
        // 全长空白
        assert_eq!(parse_image_state_date("        "), None);
    }
}
