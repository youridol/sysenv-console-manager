//! 注册表采集辅助 — 补丁枚举（P2）、GPU 厂商（P6）与通用注册表工具
//!
//! 设计原则：
//! - 只读采集，绝不写注册表（写操作属于上层业务模块）
//! - 所有路径用 `KEY_READ | KEY_WOW64_64KEY` 打开（与项目现有 winreg 用法一致）
//! - 失败返回 `CollectError::Registry`，区分"无数据"（`Ok(None)`）与"采集失败"（`Err`）

use crate::error::CollectError;
use serde::Serialize;
use winreg::enums::*;
use winreg::RegKey;

/// 注册表根键（只读场景枚举）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegHive {
    /// HKEY_LOCAL_MACHINE
    LocalMachine,
    /// HKEY_CURRENT_USER
    CurrentUser,
}

impl RegHive {
    fn to_predef(self) -> winreg::HKEY {
        match self {
            RegHive::LocalMachine => HKEY_LOCAL_MACHINE,
            RegHive::CurrentUser => HKEY_CURRENT_USER,
        }
    }
}

/// 打开注册表子键（64 位视图，只读）
pub fn open_key(hive: RegHive, path: &str) -> Result<RegKey, CollectError> {
    let root = RegKey::predef(hive.to_predef());
    root.open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY)
        .map_err(|e| CollectError::registry(format!("{:?}\\{}", hive, path), e.to_string()))
}

/// 读取 REG_SZ 字符串值
pub fn read_string(hive: RegHive, path: &str, value: &str) -> Result<Option<String>, CollectError> {
    let key = open_key(hive, path)?;
    match key.get_value::<String, _>(value) {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CollectError::registry(
            format!("{:?}\\{}", hive, path),
            format!("读取值 '{}' 失败: {}", value, e),
        )),
    }
}

/// 读取 REG_DWORD 值
pub fn read_dword(hive: RegHive, path: &str, value: &str) -> Result<Option<u32>, CollectError> {
    let key = open_key(hive, path)?;
    match key.get_value::<u32, _>(value) {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CollectError::registry(
            format!("{:?}\\{}", hive, path),
            format!("读取值 '{}' 失败: {}", value, e),
        )),
    }
}

/// 枚举子键名列表（按返回顺序）
pub fn enum_subkeys(hive: RegHive, path: &str) -> Result<Vec<String>, CollectError> {
    let key = open_key(hive, path)?;
    let mut names = Vec::new();
    for name in key.enum_keys() {
        match name {
            Ok(n) => names.push(n),
            Err(e) => {
                return Err(CollectError::registry(
                    format!("{:?}\\{}", hive, path),
                    format!("枚举子键失败: {}", e),
                ));
            }
        }
    }
    Ok(names)
}

// ============================================================================
// P2 最新补丁 — HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\HotFix\KB*
// ============================================================================

/// 补丁信息（与 SECM 前端 `PatchInfo` 契约字段一致）
#[derive(Debug, Clone, Serialize)]
pub struct PatchInfo {
    /// KB 编号，如 "KB5039212"（保留 KB 前缀，与现状一致）
    pub kb: String,
    /// 安装日期（已归一化为 YYYY-MM-DD）
    pub date: String,
    /// 补丁标题（英文原文）
    pub title_raw: String,
    /// 补丁标题（中文翻译）
    pub title_cn: String,
}

/// Windows 补丁注册表根路径
const HOTFIX_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\HotFix";

/// WMI 查询结果结构（wmi crate 按字段名反序列化，与 WQL 选择的字段一致）
/// 字段名必须与 WMI 属性名（PascalCase）一致，故禁用 snake_case lint
#[derive(serde::Deserialize)]
#[allow(non_snake_case)]
struct QuickFixEngineering {
    /// 补丁 ID，如 "KB5039212"
    #[serde(default)]
    HotFixID: Option<String>,
    /// 安装日期（月/日/年，如 "7/15/2026"）
    #[serde(default)]
    InstalledOn: Option<String>,
    /// 补丁描述（英文原文）
    #[serde(default)]
    Description: Option<String>,
}

/// WMI 查询最新补丁（设计文档 §5.2 兜底：个别系统注册表 HotFix 键缺失但 WMI 有数据）
///
/// 与现状 PowerShell `Get-CimInstance Win32_QuickFixEngineering` 同源（同一 WMI 类）。
/// 语义约定：查到 → `Ok(Some(PatchInfo))`；无数据 → `Ok(None)`；失败 → `Err`
fn latest_patch_from_wmi() -> Result<Option<PatchInfo>, CollectError> {
    let conn = wmi::WMIConnection::new().map_err(|e| {
        CollectError::winapi_detailed(
            "WMI.CoCreateInstance",
            "连接 WMI 服务",
            format!("{}", e),
        )
    })?;

    let patches: Vec<QuickFixEngineering> = conn
        .raw_query("SELECT HotFixID, InstalledOn, Description FROM Win32_QuickFixEngineering")
        .map_err(|e| {
            CollectError::winapi_detailed(
                "WMI.ExecQuery",
                "查询 Win32_QuickFixEngineering",
                format!("{}", e),
            )
        })?;

    let mut latest: Option<PatchInfo> = None;
    for p in patches {
        let kb = p.HotFixID.unwrap_or_default();
        if kb.is_empty() {
            continue;
        }
        let date_raw = p.InstalledOn.unwrap_or_default();
        let desc = p.Description.unwrap_or_default();
        let normalized = normalize_date(&date_raw);

        // 按日期降序比较（与注册表路径同规则）
        let is_newer = match &latest {
            None => true,
            Some(cur) => {
                let cur_date = cur.date.as_str();
                if normalized == cur_date {
                    kb > cur.kb
                } else if normalized.is_empty() {
                    false
                } else if cur_date.is_empty() {
                    true
                } else {
                    normalized > cur_date.to_string()
                }
            }
        };
        if is_newer {
            latest = Some(PatchInfo {
                kb,
                date: normalized,
                title_raw: desc.clone(),
                title_cn: translate_patch_title(&desc),
            });
        }
    }
    Ok(latest)
}

/// 采集最新安装的补丁（注册表 HotFix 枚举为主，WMI Win32_QuickFixEngineering 兜底）
///
/// 语义约定：
/// - 系统有补丁 → `Ok(Some(PatchInfo))`
/// - 系统无补丁记录 → `Ok(None)`（不算错误）
/// - 两条路径全部失败 → `Err(CollectError)`
pub fn latest_patch() -> Result<Option<PatchInfo>, CollectError> {
    // 主路径：注册表 HotFix 枚举（零依赖，与 WMI 同源）
    match latest_patch_from_registry() {
        Ok(Some(p)) => return Ok(Some(p)),
        Ok(None) => {
            log::debug!("registry.latest_patch: 注册表无补丁记录，尝试 WMI 兜底");
        }
        Err(e) => {
            log::warn!("registry.latest_patch: 注册表读取失败，尝试 WMI 兜底: {}", e);
        }
    }

    // 兜底：WMI 查询（个别系统注册表 HotFix 键缺失但 WMI 有数据，如精简镜像）
    match latest_patch_from_wmi() {
        Ok(Some(p)) => {
            log::debug!("registry.latest_patch: WMI 兜底命中: {}", p.kb);
            Ok(Some(p))
        }
        Ok(None) => Ok(None),
        Err(e) => {
            log::warn!("registry.latest_patch: WMI 兜底也失败: {}", e);
            Err(e)
        }
    }
}

/// 注册表 HotFix 枚举实现（原 latest_patch 主体）
fn latest_patch_from_registry() -> Result<Option<PatchInfo>, CollectError> {
    let subkeys = match enum_subkeys(RegHive::LocalMachine, HOTFIX_KEY) {
        Ok(keys) => keys,
        // 键不存在 = 无补丁记录（部分精简系统如此）
        Err(_) => return Ok(None),
    };

    let mut latest: Option<PatchInfo> = None;

    for subkey in subkeys {
        // 只处理 KB 开头的补丁子键（跳过非补丁键）
        if !subkey.starts_with("KB") {
            continue;
        }
        let sub_path = format!("{}\\{}", HOTFIX_KEY, subkey);
        let key = match open_key(RegHive::LocalMachine, &sub_path) {
            Ok(k) => k,
            Err(_) => continue, // 单个子键打开失败不阻塞整体
        };

        // InstallDate（REG_SZ，格式如 "2026/7/15"）与 Description（REG_SZ）
        let date_raw = key
            .get_value::<String, _>("InstallDate")
            .ok()
            .unwrap_or_default();
        let desc = key
            .get_value::<String, _>("Description")
            .ok()
            .unwrap_or_default();

        let normalized = normalize_date(&date_raw);
        let kb = subkey.clone();

        // 按日期降序比较：无日期条目排最后
        let is_newer = match &latest {
            None => true,
            Some(cur) => {
                let cur_date = cur.date.as_str();
                if normalized == cur_date {
                    // 同日期的取 KB 号大者（安装顺序近似）
                    kb > cur.kb
                } else if normalized.is_empty() {
                    false
                } else if cur_date.is_empty() {
                    true
                } else {
                    normalized > cur_date.to_string()
                }
            }
        };

        if is_newer {
            latest = Some(PatchInfo {
                kb,
                date: normalized,
                title_raw: desc.clone(),
                title_cn: translate_patch_title(&desc),
            });
        }
    }

    log::debug!(
        "registry.latest_patch: 枚举到补丁条目，最新={}",
        latest.as_ref().map(|p| p.kb.as_str()).unwrap_or("无")
    );
    Ok(latest)
}

/// 将日期归一化为 YYYY-MM-DD（与 SECM sysinfo.rs 同逻辑，兼容两种输入）
///
/// 输入格式：
/// - 注册表 InstallDate：`2026/7/15`（年/月/日）
/// - PowerShell InstalledOn：`7/15/2026`（月/日/年）
/// - ISO：`2026-07-15`（原样返回）
pub fn normalize_date(raw: &str) -> String {
    // 截掉时间部分
    let date_part = match raw.find(' ') {
        Some(idx) => &raw[..idx],
        None => raw,
    };
    // 若已是 YYYY-MM-DD 格式则原样返回
    if date_part.len() >= 10 && date_part.as_bytes()[4] == b'-' {
        return date_part[..10].to_string();
    }
    let parts: Vec<&str> = date_part.split('/').collect();
    if parts.len() == 3 {
        let p0 = parts[0].parse::<u32>();
        let p1 = parts[1].parse::<u32>();
        let p2 = parts[2].parse::<u32>();
        if let (Ok(a), Ok(b), Ok(c)) = (p0, p1, p2) {
            // 首段是 4 位年份 → 年/月/日（注册表 InstallDate）
            if a >= 1000 {
                return format!("{:04}-{:02}-{:02}", a, b, c);
            }
            // 尾段是 4 位年份 → 月/日/年（PowerShell InstalledOn）
            if c >= 1000 {
                return format!("{:04}-{:02}-{:02}", c, a, b);
            }
        }
    }
    raw.to_string()
}

/// 补丁标题英文 → 中文翻译（与 SECM sysinfo.rs 同逻辑）
pub fn translate_patch_title(title: &str) -> String {
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
// P6 GPU 厂商 — HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-...}\0000..000F
// ============================================================================

/// 显卡类注册表路径（设备管理器显示名同源）
const GPU_CLASS_KEY: &str =
    r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";

/// GPU 检测结果
#[derive(Debug, Clone, Serialize)]
pub struct GpuInfo {
    /// 厂商标识：nvidia / amd / intel / other / unknown
    pub vendor: String,
    /// 用户可读描述
    pub message: String,
}

/// 单张显卡（枚举全部显卡用，含注册表子键序号作为稳定 id 来源）
#[derive(Debug, Clone, Serialize)]
pub struct GpuCard {
    /// 注册表子键名（如 "0000"，设备实例序号的稳定标识）
    pub key: String,
    /// 设备管理器显示名（Win32_VideoController.Name 同源）
    pub name: String,
    /// 厂商标识：nvidia / amd / intel / other
    pub vendor: String,
}

/// 按 DriverDesc 归类厂商标识。
fn classify_vendor(desc: &str) -> &'static str {
    let lower = desc.to_lowercase();
    if lower.contains("nvidia") || lower.contains("nv") {
        "nvidia"
    } else if lower.contains("amd")
        || lower.contains("radeon")
        || lower.contains("advanced micro devices")
    {
        "amd"
    } else if lower.contains("intel") || lower.contains("uhd") || lower.contains("iris") {
        "intel"
    } else {
        "other"
    }
}

/// 枚举全部显卡（HKLM Class 键 0000..000F 全部 DriverDesc 非空条目）
///
/// 语义约定：
/// - 找到显卡 → `Ok(Vec<GpuCard>)`（可能多张：独显+核显/多卡）
/// - 无显卡记录 → `Ok(vec![])`
/// - 注册表读取失败 → `Err(CollectError::Registry)`
pub fn enum_gpu_cards() -> Result<Vec<GpuCard>, CollectError> {
    let subkeys = enum_subkeys(RegHive::LocalMachine, GPU_CLASS_KEY)?;

    // 枚举 0000..000F（显卡通常占 0000/0001；双显卡 0000+0001）
    let mut names: Vec<String> = subkeys
        .into_iter()
        .filter(|k| k.len() == 4 && k.chars().all(|c| c.is_ascii_hexdigit()))
        .collect();
    names.sort();

    let mut cards = Vec::new();
    for name in names {
        let sub_path = format!("{}\\{}", GPU_CLASS_KEY, name);
        let key = match open_key(RegHive::LocalMachine, &sub_path) {
            Ok(k) => k,
            Err(_) => continue,
        };
        // DriverDesc 即设备管理器显示名（Win32_VideoController.Name 同源）
        let desc = match key.get_value::<String, _>("DriverDesc") {
            Ok(d) => d,
            Err(_) => continue,
        };
        if desc.trim().is_empty() {
            continue;
        }
        let vendor = classify_vendor(&desc);
        log::debug!("registry.enum_gpu_cards: 在 {} 发现 {} ({})", name, vendor, desc);
        cards.push(GpuCard {
            key: name,
            name: desc,
            vendor: vendor.to_string(),
        });
    }

    if cards.is_empty() {
        log::debug!("registry.enum_gpu_cards: 未在 0000-000F 发现显卡");
    }
    Ok(cards)
}

/// 通过注册表 Class 键枚举检测 GPU 厂商（替代 wmic Win32_VideoController）
///
/// 语义约定：
/// - 找到显卡 → `Ok(Some(GpuInfo))`（vendor 已归类）
/// - 无显卡记录 → `Ok(None)`
/// - 注册表读取失败 → `Err(CollectError::Registry)`
pub fn detect_gpu() -> Result<Option<GpuInfo>, CollectError> {
    let cards = enum_gpu_cards()?;
    match cards.into_iter().next() {
        Some(card) => {
            let message = match card.vendor.as_str() {
                "nvidia" => "NVIDIA GPU 已检测".to_string(),
                "amd" => "AMD GPU 已检测".to_string(),
                "intel" => "Intel GPU 已检测".to_string(),
                _ => format!("GPU 已检测: {}", card.name),
            };
            log::debug!("registry.detect_gpu: {}", message);
            Ok(Some(GpuInfo {
                vendor: card.vendor,
                message,
            }))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_date_slash() {
        assert_eq!(normalize_date("2026/7/15"), "2026-07-15");
        assert_eq!(normalize_date("7/15/2026"), "2026-07-15");
        assert_eq!(normalize_date("2026/7/15 0:00:00"), "2026-07-15");
        assert_eq!(normalize_date("7/15/2026 8:30:00"), "2026-07-15");
    }

    #[test]
    fn test_normalize_date_iso() {
        assert_eq!(normalize_date("2026-07-15"), "2026-07-15");
        assert_eq!(normalize_date("2026-07-15T00:00:00"), "2026-07-15");
    }

    #[test]
    fn test_translate_patch_title() {
        assert_eq!(translate_patch_title("Security Update"), "安全更新");
        assert_eq!(
            translate_patch_title("2026-07 Cumulative Update for Windows 11"),
            "累积更新"
        );
        assert_eq!(translate_patch_title("Update Rollup"), "更新汇总");
        assert_eq!(translate_patch_title("Something Else"), "Something Else");
    }

    #[test]
    fn test_enum_subkeys_known_key() {
        // 通用注册表路径应可枚举（Windows 系统必有），且子键列表非空
        let keys = enum_subkeys(
            RegHive::LocalMachine,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        );
        assert!(keys.is_ok());
        let keys = keys.unwrap();
        assert!(!keys.is_empty(), "CurrentVersion 下应有子键");
    }

    #[test]
    fn test_read_string_missing_value_ok() {
        // 不存在的值 → Ok(None)，不报错
        let v = read_string(
            RegHive::LocalMachine,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "SECM_TEST_NONEXISTENT_VALUE",
        );
        assert!(matches!(v, Ok(None)));
    }

    #[test]
    fn test_read_string_known_value() {
        let v = read_string(
            RegHive::LocalMachine,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "CurrentBuild",
        );
        assert!(v.is_ok());
        let v = v.unwrap();
        assert!(v.is_some());
        let s = v.unwrap();
        assert!(s.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_latest_patch_shape() {
        // 只验证形状：Ok 且若 Some 则字段非空规范
        match latest_patch() {
            Ok(Some(p)) => {
                assert!(p.kb.starts_with("KB"), "kb 应保留 KB 前缀: {}", p.kb);
                assert!(!p.title_raw.is_empty());
            }
            Ok(None) => {
                // 精简系统可能无补丁记录，可接受
            }
            Err(e) => panic!("latest_patch 不应报错: {}", e),
        }
    }

    #[test]
    fn test_detect_gpu_shape() {
        // 只验证形状：Ok 且若 Some 则 vendor 属于已知枚举
        match detect_gpu() {
            Ok(Some(g)) => {
                assert!(
                    ["nvidia", "amd", "intel", "other"].contains(&g.vendor.as_str()),
                    "vendor 未知: {}",
                    g.vendor
                );
                assert!(!g.message.is_empty());
            }
            Ok(None) => {}
            Err(e) => panic!("detect_gpu 不应报错: {}", e),
        }
    }
}
