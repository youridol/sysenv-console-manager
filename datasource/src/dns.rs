//! DNS 缓存刷新（dnsapi）— P13
//!
//! 替换 `ipconfig /flushdns` + 本地化文本匹配（"成功刷新"/"Successfully flushed"）。
//! `DnsFlushResolverCache()` 无参返回 BOOL，成功即 TRUE。
//!
//! 线程模型：同步阻塞 API，上层须在 `spawn_blocking` 中调用（S8）。
//!
//! 说明：windows-sys 0.59 未导出 `DnsFlushResolverCache`，此处手工 FFI 声明
//! （项目已有先例：sensor.rs / net_stats.rs 手工 extern）。

use crate::error::CollectError;
use serde::Serialize;

/// DNS 刷新结果（与 SECM cleanup.rs `CleanupResult` 契约字段对齐）
#[derive(Debug, Clone, Serialize)]
pub struct DnsFlushResult {
    /// 操作名（固定 "DNS 缓存刷新"）
    pub operation: String,
    /// 是否成功
    pub success: bool,
    /// 释放字节数（DNS 刷新恒为 0）
    pub bytes_freed: u64,
    /// 用户可读消息
    pub message: String,
}

// SAFETY: dnsapi.dll 自 Windows 2000 起提供此导出，签名稳定（无参，返回 BOOL）
#[cfg(windows)]
#[link(name = "dnsapi")]
extern "system" {
    fn DnsFlushResolverCache() -> i32;
}

/// 刷新 DNS 解析缓存（`ipconfig /flushdns` 等价）
///
/// 语义约定：
/// - 成功 → `Ok(DnsFlushResult { success: true })`
/// - API 失败 → `Err(CollectError::WinApi { api: "dnsapi.DnsFlushResolverCache", .. })`
pub fn flush_dns() -> Result<DnsFlushResult, CollectError> {
    #[cfg(windows)]
    {
        // SAFETY: DnsFlushResolverCache 是 dnsapi.dll 标准导出，无参调用
        let ok = unsafe { DnsFlushResolverCache() };
        if ok == 0 {
            return Err(CollectError::winapi(
                "dnsapi.DnsFlushResolverCache",
                "刷新 DNS 解析缓存",
            ));
        }
    }

    Ok(DnsFlushResult {
        operation: "DNS 缓存刷新".to_string(),
        success: true,
        bytes_freed: 0,
        message: "DNS 解析缓存已成功刷新".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flush_dns_shape() {
        // 实机：flushdns 不应失败（普通用户权限即可执行）
        match flush_dns() {
            Ok(r) => {
                assert!(r.success, "flush_dns 应成功: {:?}", r.message);
                assert_eq!(r.operation, "DNS 缓存刷新");
                assert_eq!(r.bytes_freed, 0);
                assert!(!r.message.is_empty());
            }
            Err(e) => panic!("flush_dns 不应失败: {}", e),
        }
    }
}
