/// DirectX & runtime environment diagnostics module for SECM.
///
/// - DirectX version detection (registry + system32)
/// - GPU adapter enumeration (device class registry)
/// - VC++ Redistributable runtime detection (Uninstall registry enumeration)
use serde::{Deserialize, Serialize};

// ============================================================================
// Shared types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DxCheck {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
    Info,
}

// ============================================================================
// DirectX
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectXInfo {
    pub version_major: u32,
    pub version_minor: u32,
    pub version: String,
    pub dx12_ultimate: bool,
    pub gpu_count: usize,
    pub gpu_names: Vec<String>,
    pub checks: Vec<DxCheck>,
}

pub fn check_directx() -> DirectXInfo {
    let mut checks = Vec::new();

    let (major, minor) = read_dx_version();
    let version = format!("{}.{}", major, minor);

    checks.push(DxCheck {
        name: "DirectX 运行时版本".into(),
        status: if major >= 12 {
            CheckStatus::Pass
        } else if major >= 11 {
            CheckStatus::Warn
        } else {
            CheckStatus::Fail
        },
        detail: format!("DirectX {}.{}", major, minor),
    });

    let gpu_names = detect_gpu_names();
    let gpu_count = gpu_names.len();

    checks.push(DxCheck {
        name: "GPU 适配器".into(),
        status: if gpu_count > 0 {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail: if gpu_count > 0 {
            format!("{} 个适配器: {}", gpu_count, gpu_names.join(" · "))
        } else {
            "未检测到 GPU".into()
        },
    });

    let d3d11_available = check_d3d11_available();
    let d3d12_available = check_d3d12_available();
    let dx12_ultimate = major >= 12 && d3d12_available;

    checks.push(DxCheck {
        name: "Direct3D 11".into(),
        status: if d3d11_available {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail: if d3d11_available {
            "D3D11 运行时可用".into()
        } else {
            "D3D11 运行时不可用".into()
        },
    });

    checks.push(DxCheck {
        name: "Direct3D 12".into(),
        status: if d3d12_available {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        detail: if d3d12_available {
            "D3D12 运行时可用".into()
        } else {
            "D3D12 运行时不可用".into()
        },
    });

    checks.push(DxCheck {
        name: "WDDM 驱动模型".into(),
        status: CheckStatus::Info,
        detail: if check_wddm_2() {
            "WDDM 2.x+ (支持 DX12)".into()
        } else {
            "WDDM 1.x".into()
        },
    });

    DirectXInfo {
        version_major: major,
        version_minor: minor,
        version,
        dx12_ultimate,
        gpu_count,
        gpu_names,
        checks,
    }
}

/// 从注册表读取 DirectX 版本（两处回退）
fn read_dx_version() -> (u32, u32) {
    use winreg::enums::*;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);

    // 路径 1: HKLM\SOFTWARE\Microsoft\DirectX\Version (REG_SZ "12.0.xxxxx")
    let paths = [
        "SOFTWARE\\Microsoft\\DirectX",
        "SOFTWARE\\WOW6432Node\\Microsoft\\DirectX",
    ];

    for path in &paths {
        if let Ok(key) = hklm.open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY) {
            if let Ok(ver_str) = key.get_value::<String, _>("Version") {
                let parts: Vec<&str> = ver_str.split('.').collect();
                if parts.len() >= 2 {
                    if let (Ok(major), Ok(minor)) =
                        (parts[0].parse::<u32>(), parts[1].parse::<u32>())
                    {
                        if major >= 9 {
                            return (major, minor);
                        }
                    }
                }
            }
        }
    }

    // 路径 2: 检查 d3d12.dll 存在 → 视为 DX12
    if check_d3d12_available() {
        return (12, 0);
    }

    // 路径 3: 检查 d3d11.dll 存在 → 视为 DX11
    if let Ok(windir) = std::env::var("WINDIR") {
        if std::path::PathBuf::from(&windir)
            .join("System32")
            .join("d3d11.dll")
            .exists()
        {
            return (11, 0);
        }
    }

    (0, 0)
}

/// 从显卡设备类注册表检测当前活动 GPU（仅 PCI 硬件 GPU，排除虚拟适配器）
///
/// 通过 MatchingDeviceId 区分真实硬件（pci\* / acpi\*）与虚拟适配器（Root\*）
fn detect_gpu_names() -> Vec<String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let mut names = Vec::new();
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let adapter_key =
        "SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}";

    if let Ok(parent) = hklm.open_subkey_with_flags(adapter_key, KEY_READ) {
        for i in 0..30u32 {
            let sub_name = format!("{:04}", i);
            if let Ok(sub) = parent.open_subkey_with_flags(&sub_name, KEY_READ) {
                // 仅保留 PCI/ACPI 硬件 GPU，排除 Microsoft Basic / RDP / 虚拟适配器 (Root\*)
                if let Ok(desc) = sub.get_value::<String, _>("DriverDesc") {
                    let desc = desc.trim().to_string();
                    if desc.is_empty()
                        || desc.contains("Microsoft Basic")
                        || desc.contains("RDP")
                        || desc.contains("Citrix")
                    {
                        continue;
                    }

                    // 检查 MatchingDeviceId：仅 PCI (pci\*) 或 ACPI (acpi\*) 硬件
                    // 虚拟适配器使用 Root\* 前缀（如 Root\GameViewerIddDriver）
                    let is_hardware = sub
                        .get_value::<String, _>("MatchingDeviceId")
                        .map(|id| {
                            let id_lower = id.to_lowercase();
                            id_lower.starts_with("pci\\") || id_lower.starts_with("acpi\\")
                        })
                        .unwrap_or(false);

                    if is_hardware {
                        names.push(desc);
                    }
                }
            }
        }
    }

    names
}

fn check_d3d11_available() -> bool {
    if let Ok(windir) = std::env::var("WINDIR") {
        std::path::PathBuf::from(&windir)
            .join("System32")
            .join("d3d11.dll")
            .exists()
    } else {
        false
    }
}

fn check_d3d12_available() -> bool {
    if let Ok(windir) = std::env::var("WINDIR") {
        std::path::PathBuf::from(&windir)
            .join("System32")
            .join("d3d12.dll")
            .exists()
    } else {
        false
    }
}

fn check_wddm_2() -> bool {
    use winreg::enums::*;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok(key) = hklm.open_subkey_with_flags(
        "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers",
        KEY_READ,
    ) {
        if let Ok(wddm) = key.get_value::<u32, _>("WDDMVersion") {
            return wddm >= 2;
        }
    }
    // 回退：有 d3d12.dll = WDDM 2.x+
    check_d3d12_available()
}

// ============================================================================
// VC++ Runtime — 枚举 Uninstall 注册表
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VcRuntimeInfo {
    pub runtimes: Vec<VcRuntime>,
    pub checks: Vec<DxCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VcRuntime {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub installed: bool,
}

/// 期望的 VC++ 运行库版本列表（用于完整性检查）
const EXPECTED_VC: &[(&str, &str)] = &[
    ("Microsoft Visual C++ 2015-2022", "x64"),
    ("Microsoft Visual C++ 2015-2022", "x86"),
    ("Microsoft Visual C++ 2013", "x64"),
    ("Microsoft Visual C++ 2013", "x86"),
    ("Microsoft Visual C++ 2012", "x64"),
    ("Microsoft Visual C++ 2012", "x86"),
    ("Microsoft Visual C++ 2010", "x64"),
    ("Microsoft Visual C++ 2010", "x86"),
    ("Microsoft Visual C++ 2008", "x64"),
    ("Microsoft Visual C++ 2008", "x86"),
    ("Microsoft Visual C++ 2005", "x64"),
    ("Microsoft Visual C++ 2005", "x86"),
];

/// 通过枚举 Uninstall 注册表检测已安装的 VC++ 运行库
pub fn check_vc_runtimes() -> VcRuntimeInfo {
    use winreg::enums::*;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);

    // 直接枚举两个注册表路径（不使用 KEY_WOW64 标志，避免 winreg 行为差异）：
    // - SOFTWARE\Microsoft\...\Uninstall  → 64-bit 原生视图（64位应用默认）
    // - SOFTWARE\WOW6432Node\...\Uninstall → 32-bit 重定向视图
    let uninstall_paths = [
        (
            "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            "x64",
        ),
        (
            "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            "x86",
        ),
    ];

    let mut found: Vec<(String, String, String, String)> = Vec::new(); // (name, version, arch, display_version)

    for (path, default_arch) in &uninstall_paths {
        if let Ok(key) = hklm.open_subkey_with_flags(path, KEY_READ) {
            let subkeys: Vec<String> = key.enum_keys().filter_map(|k| k.ok()).collect();
            for sub_name in &subkeys {
                if let Ok(sub) = key.open_subkey_with_flags(sub_name, KEY_READ) {
                    if let Ok(display_name) = sub.get_value::<String, _>("DisplayName") {
                        if display_name.contains("Microsoft Visual C++") {
                            let version = sub
                                .get_value::<String, _>("DisplayVersion")
                                .unwrap_or_default();

                            // 推断架构
                            let arch = infer_arch(&display_name, default_arch);

                            // 提取友好名称
                            let short_name = extract_vc_name(&display_name);

                            // 去重（同一名称+架构只保留一个）
                            let key_id = format!("{}|{}", short_name, arch);
                            if !found
                                .iter()
                                .any(|(n, _, a, _)| format!("{}|{}", n, a) == key_id)
                            {
                                found.push((short_name, version, arch.to_string(), display_name));
                            }
                        }
                    }
                }
            }
        }
    }

    // 按版本排序 (2015-2022 排最前)
    found.sort_by(|a, b| {
        let order_a = vc_version_order(&a.0);
        let order_b = vc_version_order(&b.0);
        order_a.cmp(&order_b).then_with(|| a.2.cmp(&b.2))
    });

    // 构建详细列表
    let mut runtimes: Vec<VcRuntime> = Vec::new();

    for (expected_name, expected_arch) in EXPECTED_VC {
        // 匹配规则：名称模糊匹配 + 架构必须精确匹配
        let installed = found.iter().any(|(name, _, arch, _)| {
            arch == *expected_arch
                && (name.contains(expected_name) || expected_name.contains(name.as_str()))
        });

        let existing = found.iter().find(|(name, _, arch, _)| {
            arch == *expected_arch
                && (name.contains(expected_name) || expected_name.contains(name.as_str()))
        });

        runtimes.push(VcRuntime {
            name: format!(
                "VC++ {}",
                expected_name.trim_start_matches("Microsoft Visual C++ ")
            ),
            version: existing.map(|(_, v, _, _)| v.clone()).unwrap_or_default(),
            arch: expected_arch.to_string(),
            installed,
        });
    }

    let installed_count = runtimes.iter().filter(|r| r.installed).count();
    let total = runtimes.len();

    let checks = vec![
        DxCheck {
            name: "VC++ 运行库完整性".into(),
            status: if installed_count == total {
                CheckStatus::Pass
            } else if installed_count >= total * 2 / 3 {
                CheckStatus::Warn
            } else {
                CheckStatus::Fail
            },
            detail: format!("{}/{} 已安装", installed_count, total),
        },
        DxCheck {
            name: "VC++ 2015-2022 (最新)".into(),
            status: if runtimes
                .iter()
                .any(|r| r.name.contains("2015-2022") && r.installed)
            {
                CheckStatus::Pass
            } else {
                CheckStatus::Warn
            },
            detail: if runtimes
                .iter()
                .any(|r| r.name.contains("2015-2022") && r.installed)
            {
                "已安装最新版本".into()
            } else {
                "建议安装 VC++ 2015-2022 Redist".into()
            },
        },
    ];

    VcRuntimeInfo { runtimes, checks }
}

/// 从 DisplayName 推断架构
fn infer_arch<'a>(display_name: &str, default: &'a str) -> &'a str {
    let lower = display_name.to_lowercase();
    if lower.contains("x64") || lower.contains("64-bit") || lower.contains("amd64") {
        "x64"
    } else if lower.contains("x86") || lower.contains("32-bit") {
        "x86"
    } else {
        default
    }
}

/// 提取归一化名称
///
/// 将各种 DisplayName 变体归一为与 EXPECTED_VC 一致的格式：
/// - "2015-2022" / "v14" / "2022" / "2017" / "2019" → "Microsoft Visual C++ 2015-2022"
/// - "2013" → "Microsoft Visual C++ 2013"
/// - 其他年份同理
fn extract_vc_name(display_name: &str) -> String {
    let name = display_name
        .trim_start_matches("Microsoft Visual C++ ")
        .to_string();

    // 处理 "v14" 这种内部版本号格式（v14 = VC++ 2015-2022 系列）
    if let Some(rest) = name.strip_prefix('v') {
        if rest.chars().next().map_or(false, |c| c.is_ascii_digit()) {
            // "v14 Redistributable (x64)" → 归入 2015-2022 系列
            return "Microsoft Visual C++ 2015-2022".to_string();
        }
    }

    // 提取年份部分（以数字开头的连续数字/横线/空格）
    let first_char = name.chars().next();
    if first_char.map_or(true, |c| !c.is_ascii_digit()) {
        return "Microsoft Visual C++".to_string();
    }

    let year_part: String = name
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == ' ')
        .collect();

    let year_part = year_part.trim().to_string();

    if year_part.is_empty() {
        return "Microsoft Visual C++".to_string();
    }

    // 归一化：2015/2017/2019/2022 都归入 2015-2022 系列（ABI 兼容）
    let canonical_year = match year_part.as_str() {
        "2015" | "2017" | "2019" | "2022" => "2015-2022".to_string(),
        other => other.to_string(),
    };

    format!("Microsoft Visual C++ {}", canonical_year)
}

/// 排序权重（越新权重越小）
fn vc_version_order(name: &str) -> u32 {
    if name.contains("2015-2022") || name.contains("2015") {
        1
    } else if name.contains("2013") {
        2
    } else if name.contains("2012") {
        3
    } else if name.contains("2010") {
        4
    } else if name.contains("2008") {
        5
    } else if name.contains("2005") {
        6
    } else {
        99
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_directx() {
        let info = check_directx();
        assert!(!info.version.is_empty());
        assert!(info.checks.len() >= 5);
    }

    #[test]
    fn test_check_vc_runtimes() {
        let info = check_vc_runtimes();
        // 至少应该有期望数量的条目
        assert!(info.runtimes.len() >= 10);
        assert!(!info.checks.is_empty());
    }

    #[test]
    fn test_extract_vc_name() {
        // 标准 2015-2022 Redistributable
        assert_eq!(
            extract_vc_name("Microsoft Visual C++ 2015-2022 Redistributable (x64) - 14.42.34433"),
            "Microsoft Visual C++ 2015-2022"
        );
        // v14 内部版本号格式 → 归入 2015-2022
        assert_eq!(
            extract_vc_name("Microsoft Visual C++ v14 Redistributable (x64) - 14.51.36247"),
            "Microsoft Visual C++ 2015-2022"
        );
        // 2022 Runtime（非 Redist） → 归入 2015-2022
        assert_eq!(
            extract_vc_name("Microsoft Visual C++ 2022 X64 Additional Runtime - 14.51.36247"),
            "Microsoft Visual C++ 2015-2022"
        );
        // 2008 带 x64 后缀
        assert_eq!(
            extract_vc_name("Microsoft Visual C++ 2008 Redistributable - x64 9.0.30729.6161"),
            "Microsoft Visual C++ 2008"
        );
        // 2013 x64 Additional Runtime
        assert_eq!(
            extract_vc_name("Microsoft Visual C++ 2013 x64 Additional Runtime - 12.0.40664"),
            "Microsoft Visual C++ 2013"
        );
        // 2005 Redistributable (无架构后缀)
        assert_eq!(
            extract_vc_name("Microsoft Visual C++ 2005 Redistributable"),
            "Microsoft Visual C++ 2005"
        );
    }
}
