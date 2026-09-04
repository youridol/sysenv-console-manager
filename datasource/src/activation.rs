//! Windows 激活状态采集 — P1（注册表近似方案）
//!
//! 设计文档 §5.1：激活状态在注册表无稳定公开直读键；推荐方案 A 用 `wmi` crate
//! 查询 `SoftwareLicensingProduct`（有版本冲突风险，列为 M5 最高风险项）。
//!
//! 本模块提供两层：
//! 1. **注册表近似**（默认，零新依赖）：读取 SPP 存储子键判断激活基础设施状态，
//!    数据不完整时返回 `Ok(None)`（上层映射为 "无法获取"，与现状一致）；
//! 2. **LicenseStatus 映射表**：0-6 枚举 → 前端契约字符串，供 wmi 方案复用。
//!
//! 若后续集成任务引入 `wmi` crate 成功，可基于映射表直接替换采集源，
//! 前端契约（status_raw/label）零改动。

use crate::error::CollectError;
use crate::registry::{enum_subkeys, open_key, RegHive};
use serde::Serialize;

/// 激活信息（与 SECM 前端 `ActivationInfo` 契约字段一致）
#[derive(Debug, Clone, Serialize)]
pub struct ActivationInfo {
    /// 原始激活状态（Licensed / Unlicensed / Notification / Grace / Unknown）
    pub status_raw: String,
    /// 中文标签（已激活 / 未激活 / 通知模式 / 宽限期 / 未知）
    pub label: String,
}

/// SPP 存储根路径（SoftwareProtectionPlatform 服务的数据存储）
const SPP_KEY: &str =
    r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\SoftwareProtectionPlatform";

/// 激活产品查询条件（与现状 PowerShell 脚本一致）
/// ApplicationId=55c92734-... 是 Windows 激活产品标识，PartialProductKey 过滤未配置产品
const ACTIVATION_WQL: &str = "SELECT LicenseStatus FROM SoftwareLicensingProduct WHERE ApplicationId='55c92734-d682-4d71-983e-d6ec3f16059f' AND PartialProductKey IS NOT NULL";

/// WMI 查询结果结构（wmi crate 按字段名反序列化，与 WQL 选择的字段一致）
/// 字段名必须与 WMI 属性名（PascalCase）一致，故禁用 snake_case lint
#[derive(serde::Deserialize)]
#[allow(non_snake_case)]
struct SoftwareLicensingProduct {
    /// LicenseStatus: 0-6（见 `license_status_to_info` 映射）
    #[serde(default)]
    LicenseStatus: Option<u32>,
}

/// LicenseStatus 枚举 → 契约字符串映射（与 sysinfo.rs 现状一致）
pub fn license_status_to_info(status: u32) -> ActivationInfo {
    match status {
        0 => ActivationInfo {
            status_raw: "Unlicensed".into(),
            label: "未激活".into(),
        },
        1 => ActivationInfo {
            status_raw: "Licensed".into(),
            label: "已激活".into(),
        },
        2 => ActivationInfo {
            status_raw: "OOBGrace".into(),
            label: "OOB 宽限期".into(),
        },
        3 => ActivationInfo {
            status_raw: "OOTGrace".into(),
            label: "OOT 宽限期".into(),
        },
        4 => ActivationInfo {
            status_raw: "NonGenuineGrace".into(),
            label: "非正版宽限期".into(),
        },
        5 => ActivationInfo {
            status_raw: "Notification".into(),
            label: "通知模式".into(),
        },
        6 => ActivationInfo {
            status_raw: "ExtendedGrace".into(),
            label: "延长宽限期".into(),
        },
        n => ActivationInfo {
            status_raw: format!("Unknown({})", n),
            label: "未知".into(),
        },
    }
}

/// 注册表近似采集激活状态
///
/// 语义约定：
/// - SPP 键存在且含激活子键 → `Ok(Some(ActivationInfo))`
/// - 无激活数据 → `Ok(None)`（上层映射 "无法获取"，与现状一致）
/// - 注册表读取失败 → `Err(CollectError::Registry)`
pub fn activation_from_registry() -> Result<Option<ActivationInfo>, CollectError> {
    // 1) SPP 键是否存在
    let key = match open_key(RegHive::LocalMachine, SPP_KEY) {
        Ok(k) => k,
        Err(_) => return Ok(None), // 无 SPP = 无激活数据
    };

    // 2) 读取关键值：TokenStore 路径是否存在（SPP 正常初始化标志）
    let token_path: Option<String> = key.get_value("TokenStore").ok();
    let has_tokens = token_path.as_deref().map_or(false, |p| !p.is_empty());

    // 3) 枚举 SPP 子键，找激活相关条目
    let subkeys = enum_subkeys(RegHive::LocalMachine, SPP_KEY).unwrap_or_default();

    // 检测 LicenseStatus 类数据（SPP 内部存储，尽力而为）
    let license_status = detect_license_status(&subkeys);

    Ok(match license_status {
        Some(status) => Some(license_status_to_info(status)),
        None if has_tokens => {
            // 有 TokenStore 但读不到状态：保守返回"未知"（不误导用户为未激活）
            Some(ActivationInfo {
                status_raw: "Unknown".into(),
                label: "无法获取".into(),
            })
        }
        None => None,
    })
}

/// 尽力从 SPP 子键中检测激活状态（注册表近似，数据来源不稳定时返回 None）
fn detect_license_status(_subkeys: &[String]) -> Option<u32> {
    // SPP 存储的激活数据在加密 TokenStore 中，注册表无明文 LicenseStatus。
    // 近似策略：不做猜测，返回 None 交由上层降级。
    // 完整方案由 wmi crate（方案 A）提供，见模块头注释。
    None
}

/// 通过 WMI 查询激活状态（设计文档 §5.1 方案 A：wmi crate 查询 SoftwareLicensingProduct）
///
/// 与现状 PowerShell `Get-CimInstance SoftwareLicensingProduct` 同源（同一 WMI 类）。
/// 语义约定：
/// - 查到 LicenseStatus → `Ok(Some(ActivationInfo))`
/// - WMI 可用但无匹配产品 → `Ok(None)`
/// - WMI 连接/查询失败 → `Err(CollectError::WinApi)`
fn activation_from_wmi() -> Result<Option<ActivationInfo>, CollectError> {
    let conn = wmi::WMIConnection::new().map_err(|e| {
        CollectError::winapi_detailed(
            "WMI.CoCreateInstance",
            "连接 WMI 服务",
            format!("{}", e),
        )
    })?;

    let products: Vec<SoftwareLicensingProduct> = conn
        .raw_query(ACTIVATION_WQL)
        .map_err(|e| {
            CollectError::winapi_detailed(
                "WMI.ExecQuery",
                "查询 SoftwareLicensingProduct",
                format!("{}", e),
            )
        })?;

    // 取第一条匹配产品的 LicenseStatus（现状 PowerShell 也取首个输出行）
    for p in products {
        if let Some(status) = p.LicenseStatus {
            return Ok(Some(license_status_to_info(status)));
        }
    }
    Ok(None)
}

/// 采集入口：WMI 优先（方案 A），失败回退注册表近似（方案 B，零依赖兜底）
///
/// 语义约定（与现状一致）：
/// - 有激活数据 → `Ok(Some(ActivationInfo))`
/// - 无激活数据 → `Ok(None)`（上层映射 "无法获取"）
/// - 全部路径失败 → `Err(CollectError)`（上层降级并打 R8 日志）
pub fn get_activation() -> Result<Option<ActivationInfo>, CollectError> {
    // 主路径：WMI 查询（数据与现状 PowerShell 完全同源）
    match activation_from_wmi() {
        Ok(Some(info)) => {
            log::debug!("activation.wmi: LicenseStatus={}", info.status_raw);
            return Ok(Some(info));
        }
        Ok(None) => {
            log::debug!("activation.wmi: 无匹配激活产品，回退注册表近似");
        }
        Err(e) => {
            log::warn!("activation.wmi: 查询失败，回退注册表近似: {}", e);
        }
    }

    // 兜底：注册表 SPP 近似（SPP 服务不可用时仍可返回基础设施状态）
    activation_from_registry()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_license_status_mapping() {
        assert_eq!(license_status_to_info(0).status_raw, "Unlicensed");
        assert_eq!(license_status_to_info(0).label, "未激活");
        assert_eq!(license_status_to_info(1).status_raw, "Licensed");
        assert_eq!(license_status_to_info(1).label, "已激活");
        assert_eq!(license_status_to_info(5).status_raw, "Notification");
        assert_eq!(license_status_to_info(6).label, "延长宽限期");
        assert_eq!(license_status_to_info(99).status_raw, "Unknown(99)");
    }

    #[test]
    fn test_activation_shape() {
        // 实机：不应 Err；可能 Ok(None)（无 SPP）或 Ok(Some(...))
        match get_activation() {
            Ok(Some(info)) => {
                assert!(!info.status_raw.is_empty());
                assert!(!info.label.is_empty());
            }
            Ok(None) => {}
            Err(e) => panic!("get_activation 不应报错: {}", e),
        }
    }
}
