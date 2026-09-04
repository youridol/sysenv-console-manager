//! 纯 Rust HTTP 客户端（ureq 薄封装 + 固定 URL 白名单）— P5
//!
//! 替换 `Invoke-RestMethod`（PowerShell）+ `curl` 外部命令链路。
//! 安全约束（R4）：**禁止拼接任意 URL** — 所有请求必须先通过白名单前缀校验。
//!
//! 线程模型：ureq 为同步阻塞模型，天然适配 `spawn_blocking`（S8）；
//! 超时 5s（与现状一致），网络失败降级占位由上层处理。

use crate::error::CollectError;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

/// 固定 URL 白名单（R4 安全：禁止拼接任意 URL）
///
/// 新增外部接口须先在此登记，否则请求被拒。
pub const HTTP_ALLOWLIST: &[&str] = &[
    "http://ip-api.com/json/",
    "http://api64.ipify.org",
    "https://myip.ipip.net",
];

/// 默认超时（秒，与现状 PowerShell Invoke-RestMethod -TimeoutSec 5 一致）
pub const DEFAULT_TIMEOUT_SECS: u64 = 5;

/// 地址族过滤：强制请求走 IPv4 / IPv6 连接
///
/// 背景：`myip.ipip.net` 同时解析出 AAAA 与 A 记录，系统默认 IPv6 优先，
/// 导致同一域名返回的出口 IP 不稳定（国内出口 IPv4 / IPv6 需分别取数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpFamily {
    V4,
    V6,
}

/// 按地址族过滤解析结果（纯函数，便于单测）
fn filter_by_family(addrs: Vec<SocketAddr>, family: IpFamily) -> Vec<SocketAddr> {
    addrs
        .into_iter()
        .filter(|a| match family {
            IpFamily::V4 => a.is_ipv4(),
            IpFamily::V6 => a.is_ipv6(),
        })
        .collect()
}

/// 校验 URL 是否在白名单内（前缀匹配）
///
/// 语义约定：合法 → `Ok(())`；非法 → `Err(CollectError::Parse)`
pub fn validate_url(url: &str) -> Result<(), CollectError> {
    let ok = HTTP_ALLOWLIST
        .iter()
        .any(|allowed| url.starts_with(allowed));
    if ok {
        Ok(())
    } else {
        Err(CollectError::parse(
            "HTTP URL",
            format!("'{}' 不在白名单内（允许: {:?}）", url, HTTP_ALLOWLIST),
        ))
    }
}

/// 发起 GET 请求，返回响应体文本
///
/// 语义约定：
/// - 成功 → `Ok(String)`（响应体原文）
/// - URL 不在白名单 → `Err(CollectError::Parse)`（R4 安全）
/// - 网络失败/超时 → `Err(CollectError::Http)`（上层降级占位）
pub fn http_get(url: &str) -> Result<String, CollectError> {
    http_get_inner(url, None)
}

/// 按指定地址族强制连接的 GET 请求（如国内出口 IPv4 / IPv6 分别取数）
///
/// 通过 ureq 自定义 resolver 过滤 DNS 解析结果，仅保留目标地址族，
/// 避免系统 IPv6 优先策略导致同一域名返回不同地址族的出口 IP。
pub fn http_get_family(url: &str, family: IpFamily) -> Result<String, CollectError> {
    http_get_inner(url, Some(family))
}

/// 内部实现：`family` 为 `None` 时使用系统默认解析（与现状一致）
fn http_get_inner(url: &str, family: Option<IpFamily>) -> Result<String, CollectError> {
    validate_url(url)?;

    // 构造带超时的 agent（ureq 2.x：AgentBuilder.timeout / resolver）
    let mut builder = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS));
    if let Some(fam) = family {
        // 闭包实现 ureq::Resolver（2.12 支持 Fn(&str) -> io::Result<Vec<SocketAddr>>）
        builder = builder.resolver(move |netloc: &str| -> io::Result<Vec<SocketAddr>> {
            let all: Vec<SocketAddr> = netloc.to_socket_addrs()?.collect();
            let filtered = filter_by_family(all, fam);
            if filtered.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    format!("{}: 无可用 {:?} 地址", netloc, fam),
                ));
            }
            Ok(filtered)
        });
    }
    let agent = builder.build();

    let resp = agent.get(url).call().map_err(|e| {
        log::warn!("http.http_get: {} 失败: {}", url, describe_ureq_error(&e));
        CollectError::http(
            url,
            format!("{}（超时 {}s）", describe_ureq_error(&e), DEFAULT_TIMEOUT_SECS),
        )
    })?;

    resp.into_string()
        .map_err(|e| CollectError::http(url, format!("读取响应体失败: {}", e)))
}

/// 将 ureq 2.x 错误转为可读描述
fn describe_ureq_error(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, _resp) => format!("HTTP 状态码 {}", code),
        ureq::Error::Transport(t) => match t.kind() {
            ureq::ErrorKind::Dns => "无法解析主机名".to_string(),
            ureq::ErrorKind::ConnectionFailed => "连接失败".to_string(),
            ureq::ErrorKind::Io => format!("网络 IO 错误: {}", t),
            _ => format!("网络错误: {}", t),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_url_allowlist() {
        // 白名单内 → Ok
        assert!(validate_url("http://ip-api.com/json/?fields=query").is_ok());
        assert!(validate_url("http://api64.ipify.org").is_ok());
        assert!(validate_url("https://myip.ipip.net").is_ok());
        // 前缀匹配边界：白名单 URL 自身也是合法目标
        assert!(validate_url("http://ip-api.com/json/").is_ok());
    }

    #[test]
    fn test_validate_url_reject() {
        // 白名单外 → Err（R4：禁止任意 URL）
        assert!(validate_url("http://evil.example.com").is_err());
        assert!(validate_url("http://ip-api.com.evil.com/").is_err());
        assert!(validate_url("https://google.com").is_err());
        assert!(validate_url("").is_err());
        // 协议混淆（https vs http 白名单前缀不匹配）
        assert!(validate_url("https://ip-api.com/json/").is_err());
    }

    #[test]
    fn test_http_get_reject_unlisted() {
        // 未登记 URL 直接被拒，不发起请求
        let r = http_get("http://example.com");
        assert!(matches!(r, Err(CollectError::Parse { .. })));
    }

    #[test]
    fn test_filter_by_family() {
        let v4_1: SocketAddr = "192.168.1.1:443".parse().unwrap();
        let v4_2: SocketAddr = "220.185.184.53:443".parse().unwrap();
        let v6_1: SocketAddr = "[240e::1]:443".parse().unwrap();
        let v6_2: SocketAddr = "[240e:f7:7c00:800::1]:443".parse().unwrap();

        // 混合列表按地址族过滤
        let mixed = vec![v4_1, v6_1, v4_2, v6_2];
        assert_eq!(filter_by_family(mixed.clone(), IpFamily::V4), vec![v4_1, v4_2]);
        assert_eq!(filter_by_family(mixed.clone(), IpFamily::V6), vec![v6_1, v6_2]);

        // 空结果场景：列表中无目标地址族
        assert!(filter_by_family(vec![v4_1], IpFamily::V6).is_empty());
        assert!(filter_by_family(vec![v6_1], IpFamily::V4).is_empty());
    }

    #[test]
    fn test_http_get_family_reject_unlisted() {
        // 地址族请求同样受白名单约束（R4 安全）
        let r = http_get_family("http://example.com", IpFamily::V4);
        assert!(matches!(r, Err(CollectError::Parse { .. })));
    }
}
