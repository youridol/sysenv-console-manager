//! 纯 Rust HTTP 客户端（ureq 薄封装 + 固定 URL 白名单）— P5
//!
//! 替换 `Invoke-RestMethod`（PowerShell）+ `curl` 外部命令链路。
//! 安全约束（R4）：**禁止拼接任意 URL** — 所有请求必须先通过白名单校验。
//! 校验按 `(scheme, host, port)` 全等比较（原实现为字符串前缀匹配，可被主机后缀、
//! userinfo、端口等形式绕过；见 ADR-0001 D4 与 `validate_url` 注释）。
//!
//! 线程模型：ureq 为同步阻塞模型，天然适配 `spawn_blocking`（S8）；
//! 超时 5s（与现状一致），网络失败降级占位由上层处理。

use crate::error::CollectError;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

/// 白名单项的结构化形式：`(scheme, host, port)` + 路径前缀约束。
///
/// 原实现用 `url.starts_with(allowed)` 做字符串前缀比对，只要以白名单字符串开头
/// 即通过。实测以下四种输入全部绕过校验并会被实际发起请求（ADR-0001 D4）：
///
/// - `http://api64.ipify.org.evil.com/`     主机后缀伪装
/// - `https://myip.ipip.net.evil.com/x`     主机后缀伪装
/// - `http://api64.ipify.org:8080@evil.com/` userinfo 伪装
/// - `http://ip-api.com/json/../evil`       路径穿越
///
/// 现改为解析 URL 后按 scheme/host/port 全等比较；`path_prefix` 保留原白名单的
/// 路径层级约束（`ip-api.com` 仍只允许 `/json/` 之下），并额外拒绝路径穿越。
#[derive(Debug, Clone, Copy)]
pub struct AllowlistEntry {
    /// 协议（大小写不敏感）。
    pub scheme: &'static str,
    /// 主机名（大小写不敏感，全等）。
    pub host: &'static str,
    /// 端口；白名单项与请求 URL 都按 scheme 默认端口展开后比较。
    pub port: u16,
    /// 路径前缀约束（空串表示不限制路径）。
    pub path_prefix: &'static str,
}

/// 固定 URL 白名单（R4 安全：禁止拼接任意 URL）
///
/// 新增外部接口须先在此登记，否则请求被拒。
pub const HTTP_ALLOWLIST: &[AllowlistEntry] = &[
    AllowlistEntry {
        scheme: "http",
        host: "ip-api.com",
        port: 80,
        path_prefix: "/json/",
    },
    AllowlistEntry {
        scheme: "http",
        host: "api64.ipify.org",
        port: 80,
        path_prefix: "",
    },
    AllowlistEntry {
        scheme: "https",
        host: "myip.ipip.net",
        port: 443,
        path_prefix: "",
    },
];

/// scheme 的默认端口；未登记的 scheme 返回 `None`（其 URL 一律被拒）。
fn default_port(scheme: &str) -> Option<u16> {
    if scheme.eq_ignore_ascii_case("http") {
        Some(80)
    } else if scheme.eq_ignore_ascii_case("https") {
        Some(443)
    } else {
        None
    }
}

/// 把 URL 切成 `(scheme, host, port, path)`；任何不合规形态返回 `None`。
///
/// 拒绝：无 `://`、scheme 为空或未登记、authority 为空、含 userinfo（`@`）、
/// 端口非数字或越界、路径含 `..`。
fn split_url(url: &str) -> Option<(&str, &str, u16, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty() {
        return None;
    }
    // authority 到第一个 `/`、`?` 或 `#` 结束
    let end = rest
        .find(|c: char| c == '/' || c == '?' || c == '#')
        .unwrap_or(rest.len());
    let authority = &rest[..end];
    let path = &rest[end..];
    if authority.is_empty() {
        return None;
    }
    // userinfo（`user:pass@host`）一律拒绝：白名单只登记裸主机名
    if authority.contains('@') {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok()?),
        None => (authority, default_port(scheme)?),
    };
    if host.is_empty() {
        return None;
    }
    // 路径穿越：任何 `..` 段都拒绝（合法调用点不会使用）
    if path.contains("..") {
        return None;
    }
    Some((scheme, host, port, path))
}

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

/// 校验 URL 是否在白名单内（`(scheme, host, port)` 全等 + 路径前缀）
///
/// 语义约定：合法 → `Ok(())`；非法 → `Err(CollectError::Parse)`。
///
/// 与旧前缀匹配的差别：`http://api64.ipify.org.evil.com/` 之类不再通过。
pub fn validate_url(url: &str) -> Result<(), CollectError> {
    let ok = match split_url(url) {
        Some((scheme, host, port, path)) => HTTP_ALLOWLIST.iter().any(|entry| {
            scheme.eq_ignore_ascii_case(entry.scheme)
                && host.eq_ignore_ascii_case(entry.host)
                && port == entry.port
                && path.starts_with(entry.path_prefix)
        }),
        None => false,
    };
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
        // 白名单 URL 自身也是合法目标（路径前缀允许更深的子路径）
        assert!(validate_url("http://ip-api.com/json/").is_ok());
        // 主机大小写不敏感（DNS 语义）
        assert!(validate_url("http://IP-API.COM/json/").is_ok());
    }

    /// 前缀匹配时代的四类绕过输入：全部必须被拒（ADR-0001 D4）。
    ///
    /// 修复前的 `url.starts_with(allowed)` 对这四个输入返回 `true`，
    /// 并会真正发起请求（实测）。本用例固化修复后的行为。
    #[test]
    fn test_validate_url_rejects_prefix_bypasses() {
        // 1) 主机后缀伪装：白名单主机名 + 额外域名后缀
        assert!(validate_url("http://api64.ipify.org.evil.com/").is_err());
        assert!(validate_url("https://myip.ipip.net.evil.com/x").is_err());
        assert!(validate_url("http://ip-api.com.evil.com/json/").is_err());
        // 2) userinfo 伪装：把白名单主机塞进 userinfo，真正连的是 evil.com
        assert!(validate_url("http://api64.ipify.org:8080@evil.com/").is_err());
        assert!(validate_url("http://user@api64.ipify.org/").is_err());
        // 3) 端口伪装：前缀命中但端口不是白名单端口
        assert!(validate_url("http://ip-api.com:8080/json/").is_err());
        assert!(validate_url("https://myip.ipip.net:8443/").is_err());
        // 4) 路径穿越：前缀命中但路径越出约束范围
        assert!(validate_url("http://ip-api.com/json/../evil").is_err());
        assert!(validate_url("http://api64.ipify.org/../evil").is_err());
    }

    /// 结构化比较的边界：路径前缀必须按段匹配，不做子串匹配。
    #[test]
    fn test_validate_url_path_prefix_is_structural() {
        // `ip-api.com` 只允许 `/json/` 之下；`/jsonx/` 不满足前缀
        assert!(validate_url("http://ip-api.com/jsonx/").is_err());
        assert!(validate_url("http://ip-api.com/").is_err());
        // 不带路径的主机允许（白名单 path_prefix 为空）
        assert!(validate_url("http://api64.ipify.org/?x=1").is_ok());
    }

    /// `split_url` 的拒绝面：无协议 / 空 authority / 非数字端口 / 未登记协议。
    #[test]
    fn test_split_url_rejects_malformed() {
        assert!(split_url("ip-api.com/json/").is_none());
        assert!(split_url("http://").is_none());
        assert!(split_url("http://:8080/").is_none());
        assert!(split_url("http://ip-api.com:abc/json/").is_none());
        assert!(split_url("ftp://ip-api.com/json/").is_none());
        // 合法样本的解析结果
        assert_eq!(
            split_url("http://ip-api.com/json/?a=1"),
            Some(("http", "ip-api.com", 80, "/json/?a=1"))
        );
        assert_eq!(
            split_url("https://myip.ipip.net"),
            Some(("https", "myip.ipip.net", 443, ""))
        );
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
