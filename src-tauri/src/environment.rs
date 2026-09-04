/// DirectX & runtime environment diagnostics module for SECM.
///
/// - DirectX version detection (registry + system32)
/// - GPU adapter enumeration (device class registry)
/// - VC++ Redistributable runtime detection (Uninstall registry enumeration)
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

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

// ============================================================================
// AI Agent 工具检测 — Claude Code / Codex CLI / Gemini CLI / Grok Build / Hermes / OpenCode / OpenClaw / PI Agent
// ============================================================================

/// AI Agent 工具定义：命令名、显示名、npm 包名（用于获取最新版本和安装）
/// 仅收录 AI agent 类工具（编码代理 / 通用 agent），不含模型运行时（如 Ollama）与辅助工具（如 CC Switch）
const AI_TOOLS: &[(&str, &str, &str)] = &[
    ("claude", "Claude Code", "@anthropic-ai/claude-code"),
    ("codex", "Codex CLI", "@openai/codex"),
    ("gemini", "Gemini CLI", "@google/gemini-cli"),
    ("grok", "Grok Build", "grok"),
    ("hermes", "Hermes", "hermes-cli"),
    // 修正：OpenCode CLI 的 npm 包是 opencode-ai（@opencode-ai/sdk 是 SDK 库，
    // 非 CLI——旧配置导致 npm view 查 SDK 版本、安装装错包、版本检测错乱）
    ("opencode", "OpenCode", "opencode-ai"),
    ("openclaw", "OpenClaw", "openclaw"),
    ("pi", "PI Agent", "@earendil-works/pi-coding-agent"),
    // DeepSeek Harness（DSH）：npm 官方包 @deepseek-ai/dsh，bin 名 dsh
    ("dsh", "DeepSeek Harness", "@deepseek-ai/dsh"),
];

/// npm 包名合法性校验（npm 官方命名规范子集 + 白名单兜底）。
///
/// 安全用途：`install_or_upgrade_tool`/`fetch_npm_latest` 将包名拼接进
/// `cmd /c "npm ..."` 命令串，若不校验则恶意输入（含 & | ; > 等）可注入任意命令。
/// 校验规则（npm 规范）：可带 @scope/ 前缀；名称仅允许小写字母/数字/-/_/.；
/// 禁止一切空白与控制字符（cmd 元字符 & | ; < > ^ % ! " 均被排除）。
/// 白名单兜底：即使格式通过，仅允许 AI_TOOLS 声明的包名（前端无法注入任意包）。
pub(crate) fn is_valid_npm_package(pkg: &str) -> bool {
    if pkg.is_empty() || pkg.len() > 214 {
        return false;
    }
    // 拆分 @scope/name
    let name_part = if let Some(rest) = pkg.strip_prefix('@') {
        let Some((scope, name)) = rest.split_once('/') else {
            return false; // 有 @ 前缀但无 /（非法 scope 格式）
        };
        if scope.is_empty()
            || !scope
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.')
        {
            return false;
        }
        name
    } else {
        pkg
    };
    if name_part.is_empty() || name_part.len() > 214 {
        return false;
    }
    // 名称：小写字母/数字/-/_/.；首字符不能是 . 或 _
    let mut chars = name_part.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.')
}

/// 包名是否在 AI_TOOLS 白名单内（安装/查询的最新防线）。
fn is_whitelisted_npm_package(pkg: &str) -> bool {
    AI_TOOLS.iter().any(|(_, _, p)| *p == pkg)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiTool {
    pub name: String,
    pub display_name: String,
    pub npm_package: String,
    pub installed: bool,
    pub version: String,
    pub latest_version: String,
    pub path: String,
    pub upgradable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiToolsInfo {
    pub tools: Vec<AiTool>,
    pub checks: Vec<DxCheck>,
}

/// 检测 AI Agent 工具（多线程并行检测，快速返回）
pub fn check_ai_tools() -> AiToolsInfo {
    let mut tools: Vec<AiTool> = Vec::with_capacity(AI_TOOLS.len());

    // 为每个工具启动独立线程并行检测
    std::thread::scope(|s| {
        let handles: Vec<_> = AI_TOOLS
            .iter()
            .map(|(cmd, display_name, npm_pkg)| {
                s.spawn(move || {
                    let (installed, version, path) = detect_cli_tool(cmd, npm_pkg);
                    AiTool {
                        name: cmd.to_string(),
                        display_name: display_name.to_string(),
                        npm_package: npm_pkg.to_string(),
                        installed,
                        version,
                        latest_version: String::new(),
                        path,
                        upgradable: false,
                    }
                })
            })
            .collect();

        for h in handles {
            tools.push(h.join().unwrap());
        }
    });

    let installed_count = tools.iter().filter(|t| t.installed).count();
    let total = tools.len();

    let checks = vec![DxCheck {
        name: "AI Agent 工具".into(),
        status: if installed_count >= total / 2 {
            CheckStatus::Pass
        } else if installed_count > 0 {
            CheckStatus::Warn
        } else {
            CheckStatus::Info
        },
        detail: format!("{}/{} 已安装", installed_count, total),
    }];

    AiToolsInfo { tools, checks }
}

/// 批量获取所有 AI Agent 工具的 npm 最新版本（多线程并行网络请求）
pub fn fetch_ai_latest_versions() -> Vec<(String, String)> {
    let tasks: Vec<_> = AI_TOOLS
        .iter()
        .filter(|(_, _, npm_pkg)| !npm_pkg.is_empty())
        .map(|(cmd, _, npm_pkg)| (cmd.to_string(), npm_pkg.to_string()))
        .collect();

    let mut results = Vec::with_capacity(tasks.len());

    std::thread::scope(|s| {
        let handles: Vec<_> = tasks
            .iter()
            .map(|(cmd, pkg)| {
                let cmd = cmd.clone();
                let pkg = pkg.clone();
                s.spawn(move || {
                    let latest = fetch_npm_latest(&pkg);
                    (cmd, latest)
                })
            })
            .collect();

        for h in handles {
            let (cmd, ver) = h.join().unwrap();
            if !ver.is_empty() {
                results.push((cmd, ver));
            }
        }
    });

    results
}

/// 通过 npm registry 查询包的最新版本
fn fetch_npm_latest(pkg: &str) -> String {
    // 注入防护：非法包名直接返回空（不执行任何命令）
    if !is_valid_npm_package(pkg) {
        crate::debug_warn!("environment", "fetch_npm_latest 拒绝非法包名: {:?}", pkg);
        return String::new();
    }
    let cmdline = format!("npm view {} version", pkg);
    let output = run_command_silent("cmd", &["/c", &cmdline]);

    match output {
        Some(out) if out.status.success() => {
            let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
            // 排除错误信息混入
            if ver.to_lowercase().contains("error") || ver.is_empty() {
                String::new()
            } else {
                ver
            }
        }
        _ => String::new(),
    }
}

/// 通过 npm 安装或升级工具 (npm install -g <pkg>)

/// 查找可执行文件：先 where，再检查常见安装路径
/// npm 全局前缀（`npm prefix -g` 动态解析，带进程级缓存）。
///
/// 修复：旧实现硬编码 `USERPROFILE\\.npm-global`——不同机器 npm 全局目录
/// 可能为 `%AppData%\\npm` / 用户自定义 prefix / 其他盘符（本机实测
/// `C:\\home\\<user>\\.npm-global`），硬编码路径导致 where 找不到、
/// AI 工具检测为未安装/版本为空。
static NPM_PREFIX: OnceLock<String> = OnceLock::new();
fn npm_global_prefix() -> String {
    NPM_PREFIX
        .get_or_init(|| {
            if let Some(out) = run_command_silent("cmd", &["/c", "npm prefix -g"]) {
                if out.status.success() {
                    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !p.is_empty() {
                        return p;
                    }
                }
            }
            // 回退：npm 默认全局目录（%AppData%\\npm）
            let home = std::env::var("USERPROFILE").unwrap_or_default();
            if home.is_empty() {
                ".npm-global".to_string()
            } else {
                format!("{}\\AppData\\Roaming\\npm", home)
            }
        })
        .clone()
}
/// 通过 npm 安装或升级工具 (npm install -g <pkg>)
pub fn install_or_upgrade_tool(npm_pkg: &str) -> Result<String, String> {
    if npm_pkg.is_empty() {
        return Err("该工具不支持通过 npm 安装".into());
    }

    // 注入防护：包名必须合法 npm 格式 且 在白名单内——前端传入的任意
    // 字符串（含 & | ; > 等 cmd 元字符）在此被拒绝，杜绝命令注入
    if !is_whitelisted_npm_package(npm_pkg) {
        crate::debug_warn!(
            "environment",
            "install_or_upgrade_tool 拒绝非白名单包名: {:?}（已阻止命令注入）",
            npm_pkg
        );
        return Err(format!("不支持的 npm 包: {}", npm_pkg));
    }
    if !is_valid_npm_package(npm_pkg) {
        return Err("非法 npm 包名".into());
    }
    // cmd /c 后跟完整命令字符串（单参数），确保 npm.cmd 被正确调用
    // 注：包名已经白名单 + 格式双重校验，拼接安全
    let cmdline = format!("npm install -g {}", npm_pkg);
    let output = run_command_silent("cmd", &["/c", &cmdline]);

    match output {
        Some(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Ok(if stdout.is_empty() {
                format!("{} 安装/升级成功", npm_pkg)
            } else {
                stdout
            })
        }
        Some(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                format!("{} 安装失败", npm_pkg)
            } else {
                stderr
            })
        }
        None => Err("执行 npm 失败".into()),
    }
}

// ============================================================================
// npm 环境检测（AI 环境新增 npm 环境检测）
// ============================================================================

/// npm 运行环境信息（前端 npm 环境卡片）
#[derive(Debug, Clone, Serialize)]
pub struct NpmEnvironment {
    pub available: bool,
    pub node_version: String,
    pub npm_version: String,
    pub prefix: String,
    pub root: String,
    pub registry: String,
    pub global_packages: usize,
    pub global_ai_packages: Vec<String>,
    pub global_mcp_packages: Vec<String>,
}

/// 获取全局包列表：(name, version)（npm list -g --depth=0 --json）
fn list_global_packages() -> Vec<(String, String)> {
    let cmdline = "npm list -g --depth=0 --json".to_string();
    let Some(output) = run_command_silent("cmd", &["/c", &cmdline]) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let json_text = if text.trim().starts_with('{') {
        text.trim().to_string()
    } else {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if err.starts_with('{') { err } else { return Vec::new(); }
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&json_text) else {
        return Vec::new();
    };
    let Some(deps) = json.get("dependencies").and_then(|d| d.as_object()) else {
        return Vec::new();
    };
    deps.iter()
        .filter_map(|(name, info)| {
            let ver = info.get("version").and_then(|v| v.as_str()).unwrap_or("");
            Some((name.clone(), ver.to_string()))
        })
        .collect()
}

/// 检测 npm 运行环境（node/npm 版本 + 全局目录 + 包统计）
pub fn check_npm_environment() -> NpmEnvironment {
    let node_version = run_version_cmd("node", "--version");
    let npm_version = run_version_cmd("npm", "--version");
    let available = !node_version.is_empty() && !npm_version.is_empty();
    let prefix = run_version_cmd("npm", "prefix -g");
    let root = run_version_cmd("npm", "root -g");
    let registry = run_version_cmd("npm", "config get registry");

    let packages = list_global_packages();
    let ai_pkgs: Vec<String> = packages
        .iter()
        .filter(|(name, _)| AI_TOOLS.iter().any(|(_, _, p)| *p == name.as_str()))
        .map(|(n, v)| format!("{}@{}", n, v))
        .collect();
    let mcp_pkgs: Vec<String> = packages
        .iter()
        .filter(|(name, _)| MCP_SERVERS.iter().any(|(_, p)| *p == name.as_str()))
        .map(|(n, v)| format!("{}@{}", n, v))
        .collect();

    NpmEnvironment {
        available,
        node_version,
        npm_version,
        prefix,
        root,
        registry,
        global_packages: packages.len(),
        global_ai_packages: ai_pkgs,
        global_mcp_packages: mcp_pkgs,
    }
}

/// 执行单参数版本命令（cmd /c 中转，输出清理）
fn run_version_cmd(cmd: &str, args: &str) -> String {
    let cmdline = format!("{} {}", cmd, args);
    let Some(out) = run_command_silent("cmd", &["/c", &cmdline]) else {
        return String::new();
    };
    if !out.status.success() {
        return String::new();
    }
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// ============================================================================
// AI 工具卸载（AI 环境新增移除卸载管理）
// ============================================================================

/// 卸载 AI 工具（npm uninstall -g，白名单校验防注入）
pub fn uninstall_ai_tool(npm_pkg: &str) -> Result<String, String> {
    // 注入防护：与安装同等级——白名单 + 格式双重校验
    if !is_whitelisted_npm_package(npm_pkg) {
        crate::debug_warn!(
            "environment",
            "uninstall_ai_tool 拒绝非白名单包名: {:?}（已阻止命令注入）",
            npm_pkg
        );
        return Err(format!("不支持的 npm 包: {}", npm_pkg));
    }
    if !is_valid_npm_package(npm_pkg) {
        return Err("非法 npm 包名".into());
    }
    let cmdline = format!("npm uninstall -g {}", npm_pkg);
    let output = run_command_silent("cmd", &["/c", &cmdline]);
    match output {
        Some(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Ok(if stdout.is_empty() {
                format!("{} 卸载成功", npm_pkg)
            } else {
                stdout
            })
        }
        Some(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                format!("{} 卸载失败", npm_pkg)
            } else {
                stderr
            })
        }
        None => Err("执行 npm 失败".into()),
    }
}

// ============================================================================
// MCP 服务器管理（新增 MCP 管理）
// ============================================================================

/// MCP 服务器白名单（显示名, npm 包名）——官方 @modelcontextprotocol/* 常用集
pub const MCP_SERVERS: &[(&str, &str)] = &[
    ("GitHub", "@modelcontextprotocol/server-github"),
    ("Memory", "@modelcontextprotocol/server-memory"),
    ("Sequential Thinking", "@modelcontextprotocol/server-sequential-thinking"),
    ("Filesystem", "@modelcontextprotocol/server-filesystem"),
    ("Fetch", "@modelcontextprotocol/server-fetch"),
    ("Time", "@modelcontextprotocol/server-time"),
    ("Everything", "@modelcontextprotocol/server-everything"),
    ("Brave Search", "@modelcontextprotocol/server-brave-search"),
    ("Puppeteer", "@modelcontextprotocol/server-puppeteer"),
    ("Google Maps", "@modelcontextprotocol/server-google-maps"),
    ("Slack", "@modelcontextprotocol/server-slack"),
    ("PostgreSQL", "@modelcontextprotocol/server-postgres"),
    ("Playwright", "@executeautomation/playwright-mcp-server"),
];

/// MCP 服务器状态（前端 MCP 卡片）
#[derive(Debug, Clone, Serialize)]
pub struct McpServerInfo {
    pub name: String,
    pub package: String,
    pub installed: bool,
    pub version: String,
}

/// 检测 MCP 服务器安装状态（基于 npm 全局包列表）
pub fn list_mcp_servers() -> Vec<McpServerInfo> {
    let packages = list_global_packages();
    MCP_SERVERS
        .iter()
        .map(|(name, pkg)| {
            let found = packages.iter().find(|(n, _)| n == pkg);
            McpServerInfo {
                name: name.to_string(),
                package: pkg.to_string(),
                installed: found.is_some(),
                version: found.map(|(_, v)| v.clone()).unwrap_or_default(),
            }
        })
        .collect()
}

/// MCP 包是否在白名单内
fn is_whitelisted_mcp_package(pkg: &str) -> bool {
    MCP_SERVERS.iter().any(|(_, p)| *p == pkg)
}

/// 安装 MCP 服务器（npm install -g，白名单校验）
pub fn install_mcp_server(pkg: &str) -> Result<String, String> {
    if !is_whitelisted_mcp_package(pkg) {
        crate::debug_warn!("environment", "install_mcp_server 拒绝非白名单包: {:?}", pkg);
        return Err(format!("不支持的 MCP 服务器: {}", pkg));
    }
    if !is_valid_npm_package(pkg) {
        return Err("非法 npm 包名".into());
    }
    let cmdline = format!("npm install -g {}", pkg);
    let output = run_command_silent("cmd", &["/c", &cmdline]);
    match output {
        Some(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Ok(if stdout.is_empty() {
                format!("{} 安装成功", pkg)
            } else {
                stdout
            })
        }
        Some(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                format!("{} 安装失败", pkg)
            } else {
                stderr
            })
        }
        None => Err("执行 npm 失败".into()),
    }
}

/// 卸载 MCP 服务器（npm uninstall -g，白名单校验）
pub fn uninstall_mcp_server(pkg: &str) -> Result<String, String> {
    if !is_whitelisted_mcp_package(pkg) {
        crate::debug_warn!("environment", "uninstall_mcp_server 拒绝非白名单包: {:?}", pkg);
        return Err(format!("不支持的 MCP 服务器: {}", pkg));
    }
    let cmdline = format!("npm uninstall -g {}", pkg);
    let output = run_command_silent("cmd", &["/c", &cmdline]);
    match output {
        Some(out) if out.status.success() => {
            Ok(format!("{} 卸载成功", pkg))
        }
        Some(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                format!("{} 卸载失败", pkg)
            } else {
                stderr
            })
        }
        None => Err("执行 npm 失败".into()),
    }
}

// ============================================================================
// Skills / 插件管理（新增 skill 插件管理）
// ============================================================================

/// 扩展条目（skill / plugin），来源：各 AI 工具用户目录扫描
#[derive(Debug, Clone, Serialize)]
pub struct AiExtension {
    pub tool: String,
    pub kind: String,
    pub name: String,
    pub path: String,
    pub description: String,
}

/// 各工具扩展目录（用户主目录下的常见约定位置）
const EXTENSION_DIRS: &[(&str, &str, &str)] = &[
    ("claude", "skill", ".claude\\skills"),
    ("claude", "plugin", ".claude\\plugins"),
    ("dsh", "skill", ".dsh\\skills"),
    ("dsh", "plugin", ".dsh\\plugins"),
    ("pi", "skill", ".pi\\skills"),
    ("pi", "plugin", ".pi\\plugins"),
    ("codex", "skill", ".codex\\skills"),
    ("opencode", "skill", ".config\\opencode\\skill"),
    ("opencode", "plugin", ".config\\opencode\\plugin"),
    ("gemini", "skill", ".gemini\\skills"),
];

/// 扫描各 AI 工具的 skills / 插件目录，返回全部扩展条目
pub fn list_extensions() -> Vec<AiExtension> {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    if home.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<AiExtension> = Vec::new();
    for (tool, kind, rel) in EXTENSION_DIRS {
        let dir = std::path::Path::new(&home).join(rel);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path().to_string_lossy().to_string();
            let description = read_extension_description(&entry.path());
            out.push(AiExtension {
                tool: tool.to_string(),
                kind: kind.to_string(),
                name,
                path,
                description,
            });
        }
    }
    out.sort_by(|a, b| (&a.tool, &a.kind, &a.name).cmp(&(&b.tool, &b.kind, &b.name)));
    out
}

/// 读取扩展目录描述（SKILL.md / README.md 首行）
fn read_extension_description(dir: &std::path::Path) -> String {
    for fname in ["SKILL.md", "README.md", "skill.md", "readme.md", "description.md"] {
        let f = dir.join(fname);
        if let Ok(content) = std::fs::read_to_string(&f) {
            for line in content.lines() {
                let l = line.trim();
                if !l.is_empty() && !l.starts_with('#') {
                    return l.chars().take(120).collect();
                }
            }
        }
    }
    String::new()
}



fn find_executable(cmd: &str) -> String {
    // 1. 通过 where 命令查找（PATH 全覆盖：npm 全局 bin / 系统目录 / 其他）
    let cmdline = format!("where {}", cmd);
    if let Some(out) = run_command_silent("cmd", &["/c", &cmdline]) {
        if out.status.success() {
            if let Ok(s) = String::from_utf8(out.stdout) {
                let path = s.lines().next().unwrap_or("").trim().to_string();
                if !path.is_empty() {
                    return path;
                }
            }
        }
    }

    // 2. npm prefix -g 动态目录（npm 全局安装的 CLI 工具 bin 所在）
    let npm_prefix = npm_global_prefix();
    let npm_candidates: Vec<String> = [
        format!("{}\\{}.cmd", npm_prefix, cmd),
        format!("{}\\{}.exe", npm_prefix, cmd),
        format!("{}\\{}", npm_prefix, cmd),
    ]
    .to_vec();
    for c in &npm_candidates {
        if std::path::Path::new(c).exists() {
            return c.clone();
        }
    }

    // 3. 特殊安装路径（非 npm 安装的工具）
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let cherry_bin = format!("{}\\.cherrystudio\\bin", home);
    let special: &[String] = match cmd {
        "openclaw" => &[format!("{}\\openclaw.exe", cherry_bin)],
        "claude" => &[format!("{}\\claude.exe", cherry_bin)],
        _ => &[],
    };
    for sp in special {
        if std::path::Path::new(sp).exists() {
            return sp.clone();
        }
    }

    String::new()
}

fn suppress_windows_error_dialogs() {
    #[cfg(windows)]
    {
        // SetErrorMode 在 kernel32.dll 中，始终可用
        extern "system" {
            fn SetErrorMode(uMode: u32) -> u32;
        }
        const SEM_FAILCRITICALERRORS: u32 = 0x0001;
        const SEM_NOGPFAULTERRORBOX: u32 = 0x0002;
        unsafe {
            SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
        }
    }
}

/// 执行外部命令（自动抑制 Windows 错误弹窗 + 隐藏控制台窗口）
fn run_command_silent(cmd: &str, args: &[&str]) -> Option<std::process::Output> {
    suppress_windows_error_dialogs();
    let mut command = std::process::Command::new(cmd);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.output().ok()
}

/// 尝试多个版本标志获取版本号，并清理输出
///
/// K2 优化：先解析工具绝对路径；若为 .exe 直接 spawn（去 `cmd /c` 中转，
/// 收窄命令注入面）；.cmd/.bat 批处理无法被 CreateProcess 直接执行
/// （Windows 硬限制），保留 `cmd /c` 中转。
fn try_get_version(cmd: &str, npm_pkg: &str) -> String {
    let flags = ["--version", "-v", "-V", "version"];

    // 解析绝对路径：.exe 走直连，.cmd/.bat 走 cmd 中转
    let exe_path = find_executable(cmd);
    let is_batch = exe_path
        .to_lowercase()
        .ends_with(".cmd")
        || exe_path.to_lowercase().ends_with(".bat");
    let direct_exe = if exe_path.is_empty() || is_batch {
        None
    } else {
        Some(exe_path)
    };

    for flag in &flags {
        let output = match &direct_exe {
            Some(path) => run_command_silent(path, &[flag]),
            None => {
                let cmdline = format!("{} {}", cmd, flag);
                run_command_silent("cmd", &["/c", &cmdline])
            }
        };
        if let Some(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            // 优先用 stdout，如果为空且 stderr 不含错误关键词则用 stderr
            let combined = if !stdout.is_empty() {
                stdout
            } else if !stderr.is_empty()
                && !stderr.to_lowercase().contains("error")
                && !stderr.to_lowercase().contains("fail")
                && !stderr.to_lowercase().contains("could not create")
            {
                stderr
            } else {
                continue;
            };

            if !combined.is_empty() {
                let ver = combined.lines().next().unwrap_or("").trim().to_string();
                let ver = clean_version(cmd, &ver);
                if !ver.is_empty() {
                    return ver;
                }
            }
        }
    }
    // 回退：npm 全局版本（CLI 输出不可用时）
    npm_list_global_version(npm_pkg).map(|(v, _)| v).unwrap_or_default()
}

/// 通过 npm list -g 查找已安装的 CLI 工具版本
/// 通过 `npm list -g <pkg> --depth=0 --json` 获取已安装版本（npm 全局权威源）。
///
/// 返回 `(version, resolved_path)`；未安装/解析失败返回 None。
/// JSON 格式：{"dependencies": {"<pkg>": {"version": "x.y.z", "resolved": "..."}}}
/// 注意：npm list 对未安装包退出码非 0 但仍输出 JSON——必须解析 JSON 而非依赖退出码。
fn npm_list_global_version(pkg: &str) -> Option<(String, String)> {
    if !is_valid_npm_package(pkg) {
        return None;
    }
    let cmdline = format!("npm list -g {} --depth=0 --json", pkg);
    let output = run_command_silent("cmd", &["/c", &cmdline])?;
    let text = String::from_utf8_lossy(&output.stdout);
    // 兼容 npm 把 JSON 写在 stdout 或 stderr（部分版本错误信息进 stderr）
    let json_text = if text.trim().starts_with('{') {
        text.trim().to_string()
    } else {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if err.starts_with('{') { err } else { return None; }
    };
    let json: serde_json::Value = serde_json::from_str(&json_text).ok()?;
    let entry = json.get("dependencies")?.get(pkg)?;
    let version = entry.get("version")?.as_str()?.to_string();
    let resolved = entry
        .get("resolved")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();
    if version.is_empty() {
        None
    } else {
        Some((version, resolved))
    }
}

/// 清理版本字符串中的工具名前缀和 git hash 后缀
/// 清理版本字符串：ANSI 转义 / 工具名前缀（含 -cli 变体）/ git hash 后缀，
/// 并提取标准版本号模式。修复：旧实现仅剥离 `cmd ` 前缀，gemini-cli/hermes-cli
/// 等输出 `name-cli x.y.z` 时前缀剥离失败 → 版本显示错乱、升级比较恒不等。
fn clean_version(cmd: &str, raw: &str) -> String {
    let mut s = raw.trim().to_string();
    // 1. 去除 ANSI 转义序列（\x1b[...m）——claude 等输出带颜色
    while let Some(pos) = s.find('\x1b') {
        if let Some(end) = s[pos..].find('m') {
            s = format!("{}{}", &s[..pos], &s[pos + end + 1..]);
        } else {
            break;
        }
    }
    let s = s.trim().to_string();
    // 2. 剥离工具名前缀（支持 `cmd`、`cmd v`、`cmd-cli`、`Cmd` 首字母大写等变体）
    let lower = s.to_lowercase();
    let variants: Vec<String> = vec![
        cmd.to_string(),
        format!("{}-cli", cmd),
        format!("{} cli", cmd),
        format!("{}-code", cmd),
    ];
    let mut stripped = s.clone();
    for v in &variants {
        for suffix in [" ", " v", " version "] {
            let prefix = format!("{}{}", v, suffix);
            if lower.starts_with(&prefix.to_lowercase()) {
                stripped = s[prefix.len()..].trim().to_string();
                break;
            }
        }
        if stripped != s {
            break;
        }
    }
    let s = stripped;
    // 3. 剥离 git hash 等括号后缀（如 "2026.7.1-2 (deadbeef)"）
    let s = if let Some(paren) = s.find(" (") {
        s[..paren].trim().to_string()
    } else {
        s
    };
    // 4. 提取标准版本号：v 前缀可选 + 数字.数字[.数字][-预发布]（首个匹配）
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // 跳过到数字或 v+数字
        if bytes[i] == b'v' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            i += 1;
        }
        if bytes[i].is_ascii_digit() {
            let start = if i > 0 && s.as_bytes()[i - 1] == b'v' { i - 1 } else { i };
            let rest = &s[start..];
            // 版本号字符集：数字/./-（预发布）+ 字母（如 rc.6、beta.2）
            let end = rest
                .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c.is_ascii_alphabetic() || c == '+'))
                .unwrap_or(rest.len());
            let candidate = &rest[..end];
            // 必须包含至少一个点（x.y 或 x.y.z），避免把 "1" 当版本
            if candidate.contains('.') {
                return candidate.to_string();
            }
        }
        i += 1;
    }
    // 5. 兜底：无版本模式时返回清理后的原始字符串
    s
}


/// 检测单个 AI Agent 工具：先查找路径，再尝试获取版本
/// 检测 CLI 工具：已安装 + 版本 + 路径。
///
/// 版本来源统一为 npm 全局管理（修复版本检测错乱）：
/// 1. `npm list -g <pkg> --depth=0 --json` 权威版本（与 npm view latest 同格式，
///    可正确比较升级；不依赖各工具 --version 输出格式差异——gemini-cli/hermes-cli
///    等输出前缀剥离失败、ANSI 颜色、git hash 后缀等旧 bug 一并消除）；
/// 2. 回退 CLI --version（非 npm 安装/手动安装场景）。
fn detect_cli_tool(cmd: &str, npm_pkg: &str) -> (bool, String, String) {
    // 1. npm 全局版本（权威）
    if let Some((version, _resolved)) = npm_list_global_version(npm_pkg) {
        let path = find_executable(cmd);
        return (true, version, path);
    }
    // 2. 回退：可执行文件存在 + CLI --version
    let path = find_executable(cmd);
    if path.is_empty() {
        return (false, String::new(), String::new());
    }
    let version = try_get_version(cmd, npm_pkg);
    (true, version, path)
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

    #[test]
    fn npm_package_valid_names() {
        // 合法 npm 包名（含 scope）
        assert!(is_valid_npm_package("claude"));
        assert!(is_valid_npm_package("@anthropic-ai/claude-code"));
        assert!(is_valid_npm_package("@openai/codex"));
        assert!(is_valid_npm_package("hermes-cli"));
        assert!(is_valid_npm_package("grok"));
        assert!(is_valid_npm_package("@earendil-works/pi-coding-agent"));
        assert!(is_valid_npm_package("pkg.name_1-2"));
    }

    #[test]
    fn npm_package_rejects_injection() {
        // 命令注入载荷：全部应拒绝（不进入 cmd /c 拼接）
        assert!(!is_valid_npm_package("pkg & calc"));
        assert!(!is_valid_npm_package("pkg | whoami"));
        assert!(!is_valid_npm_package("pkg; rm -rf /"));
        assert!(!is_valid_npm_package("pkg && dir"));
        assert!(!is_valid_npm_package("pkg > out.txt"));
        assert!(!is_valid_npm_package("pkg %PATH%"));
        assert!(!is_valid_npm_package("pkg \" & echo"));
        assert!(!is_valid_npm_package("pkg ^& echo"));
        assert!(!is_valid_npm_package("pkg `echo`"));
        assert!(!is_valid_npm_package("pkg $(whoami)"));
        assert!(!is_valid_npm_package("pkg ${x}"));
        assert!(!is_valid_npm_package("a b"));
        assert!(!is_valid_npm_package(""));
        assert!(!is_valid_npm_package("@scope")); // 无 / 的 scope 前缀
        assert!(!is_valid_npm_package("@/name")); // 空 scope
        assert!(!is_valid_npm_package("_underscore")); // 首字符
        assert!(!is_valid_npm_package(".dot")); // 首字符
        assert!(!is_valid_npm_package("UPPER")); // 大写非法
        assert!(!is_valid_npm_package("中文包")); // 非 ASCII
    }

    #[test]
    fn whitelist_guards_install() {
        // 白名单：AI_TOOLS 声明的包名放行，其他一律拒绝（即使格式合法）
        assert!(is_whitelisted_npm_package("@anthropic-ai/claude-code"));
        assert!(is_whitelisted_npm_package("@openai/codex"));
        assert!(is_whitelisted_npm_package("@earendil-works/pi-coding-agent"));
        assert!(is_whitelisted_npm_package("@deepseek-ai/dsh"));
        // OpenCode CLI 包（修正：旧配置 @opencode-ai/sdk 是 SDK 库）
        assert!(is_whitelisted_npm_package("opencode-ai"));
        // 格式合法但不在白名单 → 拒绝（防止任意包安装）
        assert!(!is_whitelisted_npm_package("lodash"));
        assert!(!is_whitelisted_npm_package("@evil/package"));
    }


    #[test]
    fn clean_version_variants() {
        // gemini-cli / hermes-cli 前缀（旧实现剥离失败 → 版本错乱的根因）
        assert_eq!(clean_version("gemini", "gemini-cli 0.55.1"), "0.55.1");
        assert_eq!(clean_version("hermes", "hermes-cli 1.0.0"), "1.0.0");
        assert_eq!(clean_version("gemini", "Gemini CLI 1.2.3"), "1.2.3");
        // 标准前缀
        assert_eq!(clean_version("claude", "2.1.231"), "2.1.231");
        assert_eq!(clean_version("claude", "Claude Code 2.1.231"), "2.1.231");
        assert_eq!(clean_version("codex", "codex 0.147.0"), "0.147.0");
        assert_eq!(clean_version("codex", "codex v0.147.0"), "0.147.0");
        // git hash 后缀剥离
        assert_eq!(clean_version("openclaw", "OpenClaw 2026.7.1-2 (deadbeef)"), "2026.7.1-2");
        // ANSI 转义清理（claude 输出带颜色）
        assert_eq!(clean_version("claude", "\x1b[32m2.1.231\x1b[0m"), "2.1.231");
        // 预发布版本
        assert_eq!(clean_version("dsh", "0.1.0-rc.6"), "0.1.0-rc.6");
        // 无版本模式：返回清理后原文
        assert_eq!(clean_version("grok", "grok command line tool"), "grok command line tool");
    }

}
