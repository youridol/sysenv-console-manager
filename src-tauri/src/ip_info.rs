/// IP 信息采集模块
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct IpInfo {
    pub local_ipv4: String,
    pub local_ipv6: String,
    pub public_ipv4: String,
    pub public_ipv6: String,
    /// 国内出口 IPv4（myip.ipip.net 强制 IPv4 连接）
    pub domestic_ip: String,
    /// 国内出口 IPv6（myip.ipip.net 强制 IPv6 连接；无 IPv6 连通性时为占位）
    pub domestic_ipv6: String,
    /// 国内出口 IPv4 归属地
    pub domestic_location: String,
    /// 国内出口 IPv6 归属地
    pub domestic_ipv6_location: String,
    pub city: String,
    pub country: String,
    pub timezone: String,
}

fn na() -> String {
    "—".into()
}

pub fn get_ip_info() -> IpInfo {
    let (local_ipv4, local_ipv6) = get_local_ips();
    let (pub_ipv4, city, country, timezone) = fetch_ip_api();
    let pub_ipv6 = fetch_ipv6();
    // 国内出口分别按地址族强制连接取数：
    // 解决 myip.ipip.net 双栈 DNS（AAAA+A）下系统 IPv6 优先导致"国内"行拿到 IPv6 的问题
    let (domestic_ip, domestic_location) = fetch_domestic_v4();
    let (domestic_ipv6, domestic_ipv6_location) = fetch_domestic_v6();

    IpInfo {
        local_ipv4: if local_ipv4.is_empty() {
            na()
        } else {
            local_ipv4
        },
        local_ipv6: if local_ipv6.is_empty() {
            na()
        } else {
            local_ipv6
        },
        public_ipv4: if pub_ipv4.is_empty() { na() } else { pub_ipv4 },
        public_ipv6: if pub_ipv6.is_empty() { na() } else { pub_ipv6 },
        domestic_ip: if domestic_ip.is_empty() {
            na()
        } else {
            domestic_ip
        },
        domestic_ipv6: if domestic_ipv6.is_empty() {
            na()
        } else {
            domestic_ipv6
        },
        domestic_location: if domestic_location.is_empty() {
            na()
        } else {
            domestic_location
        },
        domestic_ipv6_location: if domestic_ipv6_location.is_empty() {
            na()
        } else {
            domestic_ipv6_location
        },
        city: if city.is_empty() { na() } else { city },
        country: if country.is_empty() { na() } else { country },
        timezone: if timezone.is_empty() { na() } else { timezone },
    }
}

/// 获取本地 IPv4/IPv6 地址（P4）
///
/// 走纯 Rust 驱动（secm-datasource::netif，GetAdaptersAddresses，与 Get-NetIPAddress 同源）。
/// 失败/无数据时返回空串（上层映射为 "—" 占位，与现状一致），并打 R8 日志。
fn get_local_ips() -> (String, String) {
    match crate::datasource::netif::local_ips() {
        Ok(ips) => (ips.ipv4.unwrap_or_default(), ips.ipv6.unwrap_or_default()),
        Err(e) => {
            let msg = crate::ds_util::log_err("ip_info", "获取本地 IP", &e);
            crate::debug_warn!("ip_info", "本地 IP 降级: {}", msg);
            (String::new(), String::new())
        }
    }
}

fn fetch_ip_api() -> (String, String, String, String) {
    let body = http_get("http://ip-api.com/json/?fields=query,city,country,timezone");
    if body.is_empty() {
        return (String::new(), String::new(), String::new(), String::new());
    }
    (
        json_val(&body, "query"),
        json_val(&body, "city"),
        json_val(&body, "country"),
        json_val(&body, "timezone"),
    )
}

fn fetch_ipv6() -> String {
    http_get("http://api64.ipify.org").trim().to_string()
}

/// 获取国内出口 IP（IPv4）+ 归属地，一次请求完成
///
/// 强制 IPv4 连接（ureq 自定义 resolver），确保拿到的是 IPv4 地址，
/// 避免系统 IPv6 优先策略下 myip.ipip.net 返回 IPv6。
fn fetch_domestic_v4() -> (String, String) {
    match crate::datasource::http::http_get_family(
        "https://myip.ipip.net",
        crate::datasource::http::IpFamily::V4,
    ) {
        Ok(body) => parse_domestic(&body, extract_ipv4),
        Err(e) => {
            let msg = crate::ds_util::log_err("ip_info", "获取国内 IPv4", &e);
            crate::debug_warn!("ip_info", "国内 IPv4 降级: {}", msg);
            (String::new(), String::new())
        }
    }
}

/// 获取国内出口 IP（IPv6）+ 归属地
///
/// 强制 IPv6 连接；无 IPv6 连通性/解析不到 AAAA 时返回空（上层映射占位）。
fn fetch_domestic_v6() -> (String, String) {
    match crate::datasource::http::http_get_family(
        "https://myip.ipip.net",
        crate::datasource::http::IpFamily::V6,
    ) {
        Ok(body) => parse_domestic(&body, extract_ipv6),
        Err(e) => {
            let msg = crate::ds_util::log_err("ip_info", "获取国内 IPv6", &e);
            crate::debug_warn!("ip_info", "国内 IPv6 降级: {}", msg);
            (String::new(), String::new())
        }
    }
}

/// 解析 myip.ipip.net 响应，返回 (指定地址族 IP, 归属地)
///
/// 响应格式示例："当前 IP：222.83.252.7  来自于：中国 广西 玉林  电信"
fn parse_domestic(body: &str, extract_ip: fn(&str) -> String) -> (String, String) {
    if body.is_empty() {
        return (String::new(), String::new());
    }
    // 响应可能包含 HTML <pre> 标签，去除所有标签后再提取
    let clean = strip_html(body);
    (extract_ip(&clean), extract_domestic_location(&clean))
}

/// 提取归属地："中国 广西 玉林  电信" → 去掉"中国"，显示"广西 · 玉林 · 电信"
fn extract_domestic_location(clean: &str) -> String {
    if let Some(pos) = clean.find("来自于") {
        let rest = &clean[pos + "来自于".len()..];
        let parts: Vec<&str> = rest
            .split_whitespace()
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_start_matches('：').trim_start_matches(':'))
            .collect();
        // 去掉第一项"中国"，直接显示省市+ISP
        let location_parts: Vec<&str> = if parts.len() > 1 {
            parts[1..].to_vec()
        } else {
            parts
        };
        if !location_parts.is_empty() {
            return location_parts.join(" · ");
        }
    }
    String::new()
}

/// 去除 HTML 标签
fn strip_html(s: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        if c == '<' {
            in_tag = true;
            continue;
        }
        if c == '>' {
            in_tag = false;
            continue;
        }
        if !in_tag {
            result.push(c);
        }
    }
    result
}

fn extract_ipv4(text: &str) -> String {
    // 使用字节级扫描，比 char 级更可靠（不受 Unicode 宽度影响）
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        // 找连续的数字+点模式
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        let mut dots = 0u8;
        while i < len {
            let b = bytes[i];
            if b.is_ascii_digit() {
                i += 1;
            } else if b == b'.' {
                dots += 1;
                i += 1;
            } else {
                break;
            }
        }
        if dots == 3 && start < i {
            let candidate = &text[start..i];
            // 验证每段均为 0-255
            if candidate.split('.').all(|p| p.parse::<u8>().is_ok()) {
                return candidate.to_string();
            }
        }
        if i == start {
            i += 1;
        }
    }
    String::new()
}

/// 提取 IPv6 地址（含冒号的十六进制序列，排除 IPv4 点分格式）
fn extract_ipv6(text: &str) -> String {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        // 找连续的 [0-9a-fA-F: ] 模式起始点
        if !(bytes[i].is_ascii_hexdigit() || bytes[i] == b':') {
            i += 1;
            continue;
        }
        let start = i;
        while i < len && (bytes[i].is_ascii_hexdigit() || bytes[i] == b':') {
            i += 1;
        }
        let candidate = &text[start..i];
        if is_valid_ipv6(candidate) {
            return candidate.to_string();
        }
        if i == start {
            i += 1;
        }
    }
    String::new()
}

/// 校验候选串是否为合法 IPv6 地址（宽松校验：含冒号、每段为空或 1-4 位十六进制、无点）
fn is_valid_ipv6(candidate: &str) -> bool {
    if candidate.len() < 2 || !candidate.contains(':') || candidate.contains('.') {
        return false;
    }
    candidate
        .split(':')
        .all(|seg| seg.is_empty() || (seg.len() <= 4 && seg.chars().all(|c| c.is_ascii_hexdigit())))
}

// ============================================================================
// 工具函数
// ============================================================================

/// 发起 HTTP GET 请求，返回响应体文本（P5）
///
/// 走纯 Rust 驱动（secm-datasource::http，ureq + URL 白名单，超时 5s 与现状一致）。
/// 失败/无数据时返回空串（上层映射为 "—" 占位，与现状一致），并打 R8 日志。
/// 注意：URL 必须命中 datasource 白名单（R4 安全），否则直接返回空串。
fn http_get(url: &str) -> String {
    match crate::datasource::http::http_get(url) {
        Ok(body) => body,
        Err(e) => {
            let msg = crate::ds_util::log_err("ip_info", "HTTP 请求", &e);
            crate::debug_warn!("ip_info", "HTTP 请求降级（url={}）: {}", url, msg);
            String::new()
        }
    }
}

/// 从 JSON 文本中取出 `key` 对应的字符串值；缺失/类型不符/解析失败 → 空串。
///
/// 用 `serde_json` 而非手写扫描（ADR-0001 D11）：原实现按 `"key":"` 定位后找下一个
/// `"`，遇到值内含转义引号（`\"`）会提前截断，得到被截断的城市名/时区。
/// 项目本就依赖 `serde_json`，手写解析属重复实现成熟能力。
fn json_val(json: &str, key: &str) -> String {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| v.get(key).and_then(|x| x.as_str()).map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::json_val;

    #[test]
    fn 取值_普通字段() {
        let body = r#"{"query":"1.2.3.4","city":"Hangzhou","timezone":"Asia/Shanghai"}"#;
        assert_eq!(json_val(body, "query"), "1.2.3.4");
        assert_eq!(json_val(body, "city"), "Hangzhou");
        assert_eq!(json_val(body, "timezone"), "Asia/Shanghai");
    }

    /// 回归：值内含转义引号时，旧的手写扫描会提前截断。
    #[test]
    fn 取值_值内含转义引号不截断() {
        let body = r#"{"city":"He said \"hi\"","query":"9.9.9.9"}"#;
        assert_eq!(json_val(body, "city"), "He said \"hi\"");
    }

    #[test]
    fn 取值_缺失与非法输入() {
        assert_eq!(json_val(r#"{"a":"1"}"#, "b"), "");
        assert_eq!(json_val("not json at all", "a"), "");
        assert_eq!(json_val("", "a"), "");
        // 值不是字符串（数字/对象）时返回空串，不做类型强转
        assert_eq!(json_val(r#"{"n":123}"#, "n"), "");
        assert_eq!(json_val(r#"{"o":{"k":"v"}}"#, "o"), "");
    }
}
