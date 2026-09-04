//! 网络接口采集（iphlpapi）— 链路协商速度（P3）、本地 IP（P4）、完整网络配置（P4.5）
//!
//! 替换 `Get-NetAdapter` / `Get-NetIPAddress` / `ipconfig` PowerShell 链路。
//! 接口别名与 sysinfo 网卡名同源（GetIfTable2 的 Alias），上层按键名直接匹配。
//!
//! 线程模型：同步阻塞 API，上层须在 `spawn_blocking` 中调用（S8）。
//! 准静态数据（链路速度 / 网络配置），不进 1s 轮询热路径。

use crate::error::CollectError;
use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetAdaptersAddresses, GetIfTable2, GAA_FLAG_INCLUDE_GATEWAYS,
    GAA_FLAG_INCLUDE_PREFIX, IP_ADAPTER_ADDRESSES_LH, MIB_IF_TABLE2,
};
use windows_sys::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6,
};

/// GetAdaptersAddresses 缓冲不足错误码（ERROR_BUFFER_OVERFLOW）
const ERROR_BUFFER_OVERFLOW: u32 = 111;

// ============================================================================
// P3 链路协商速度 — GetIfTable2
// ============================================================================

/// 读取各网卡的链路协商速度（接口别名 → 速度字符串，如 "1 Gbps"）
///
/// 与 `Get-NetAdapter` 语义对齐：
/// - 跳过未建立链路（OperStatus != IfOperStatusUp）的网卡（现状跳过 "0 bps"/"Unknown"）
/// - 速度取 TransmitLinkSpeed（与 Get-NetAdapter 的 LinkSpeed 同值）
///
/// 失败或无网卡时返回空表（前端据此隐藏速度显示，优雅降级）。
pub fn link_speeds() -> Result<HashMap<String, String>, CollectError> {
    let mut table_ptr: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: GetIfTable2 分配 MIB_IF_TABLE2（堆内存），成功时须 FreeMibTable 释放
    let rc = unsafe { GetIfTable2(&mut table_ptr) };
    if rc != 0 {
        return Err(CollectError::winapi_detailed(
            "iphlpapi.GetIfTable2",
            "获取网络接口表",
            format!("错误码 {}", rc),
        ));
    }
    if table_ptr.is_null() {
        return Ok(HashMap::new());
    }

    // SAFETY: 结构体头部为 NumEntries + Table 数组首元素，读取头部字段安全
    let num_entries = unsafe { (*table_ptr).NumEntries };
    // SAFETY: MIB_IF_ROW2 是固定大小结构（不含变长字段），Table 是 [MIB_IF_ROW2; 1] 惯用技巧
    let first = unsafe { (*table_ptr).Table.as_ptr() };

    let mut map = HashMap::new();

    for i in 0..num_entries {
        // SAFETY: first 指向 Table[0]，指针在 GetIfTable2 分配的连续内存范围内，
        // 按元素索引步进（i * sizeof(MIB_IF_ROW2)），i < NumEntries
        let row_ptr = unsafe { first.add(i as usize) };
        // SAFETY: row 为有效 MIB_IF_ROW2
        let row = unsafe { &*row_ptr };

        // 跳过未建立链路的接口（IfOperStatusUp = 1）
        if row.OperStatus != 1 {
            continue;
        }
        // TransmitLinkSpeed = 0 视为未协商（现状跳过 "0 bps"）
        if row.TransmitLinkSpeed == 0 {
            continue;
        }
        // Alias 是固定长度 WCHAR 数组（MAX_INTERFACE_NAME_LEN=257）
        let alias = wide_array_to_string(&row.Alias);
        if alias.is_empty() {
            continue;
        }

        let speed = fmt_speed_bps(row.TransmitLinkSpeed);
        map.insert(alias, speed);
    }

    log::debug!(
        "netif.link_speeds: 共 {} 个接口，{} 个已连接且速率有效",
        num_entries,
        map.len()
    );

    // SAFETY: FreeMibTable 释放 GetIfTable2 分配的内存
    unsafe {
        FreeMibTable(table_ptr as _);
    }

    Ok(map)
}

/// 将 bps 格式化为 "1 Gbps" / "100 Mbps" / "10 Kbps"（与 Get-NetAdapter 输出对齐）
pub fn fmt_speed_bps(bps: u64) -> String {
    if bps >= 1_000_000_000 {
        let g = bps as f64 / 1_000_000_000.0;
        // 保留最多一位小数（2.5 Gbps 等），整数不带小数
        if (g.fract() * 10.0).abs() < f64::EPSILON {
            format!("{:.0} Gbps", g)
        } else {
            format!("{:.1} Gbps", g)
        }
    } else if bps >= 1_000_000 {
        let m = bps as f64 / 1_000_000.0;
        if (m.fract() * 10.0).abs() < f64::EPSILON {
            format!("{:.0} Mbps", m)
        } else {
            format!("{:.1} Mbps", m)
        }
    } else if bps >= 1_000 {
        let k = bps as f64 / 1_000.0;
        format!("{:.0} Kbps", k)
    } else {
        format!("{} bps", bps)
    }
}

// ============================================================================
// P4 本地 IP — GetAdaptersAddresses
// ============================================================================

/// 本地 IP 采集结果
#[derive(Debug, Clone, Default)]
pub struct LocalIps {
    /// 第一个非回环 IPv4
    pub ipv4: Option<String>,
    /// 第一个非回环非链路本地 IPv6（去掉 %zone 后缀）
    pub ipv6: Option<String>,
}

/// 获取本地 IPv4/IPv6 地址（`Get-NetIPAddress` 语义等价）
///
/// 过滤规则（与现状 PowerShell 对齐）：
/// - 跳过回环（Loopback 接口 / 127.x / ::1）
/// - IPv6 跳过链路本地 fe80::*，去掉 %zone 后缀
/// - 取"第一个非回环非 WellKnown 地址"（现状 Select-Object -First 1）
pub fn local_ips() -> Result<LocalIps, CollectError> {
    // 两段式：先探测所需缓冲大小（传 NULL，返回 ERROR_BUFFER_OVERFLOW 并填充 size）
    let mut size: u32 = 0;
    // SAFETY: 探测调用，NULL 缓冲合法；reserved 参数必须为 NULL
    let rc = unsafe {
        GetAdaptersAddresses(
            AF_UNSPEC as u32,
            GAA_FLAG_INCLUDE_PREFIX,
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if rc != 0 && rc != ERROR_BUFFER_OVERFLOW {
        return Err(CollectError::winapi_detailed(
            "iphlpapi.GetAdaptersAddresses",
            "探测本地 IP 缓冲大小",
            format!("错误码 {}", rc),
        ));
    }

    // 分配缓冲（size 为所需字节数；放大防止地址表在两次调用间增长）
    let buf_len = (size as usize).saturating_add(15 * 1024);
    let mut buf: Vec<u8> = vec![0u8; buf_len];
    let mut out_size: u32 = buf_len as u32;

    // SAFETY: buf 为有效可变缓冲区，长度以 size 传入；API 填充 IP_ADAPTER_ADDRESSES_LH 链表
    let rc = unsafe {
        GetAdaptersAddresses(
            AF_UNSPEC as u32,
            GAA_FLAG_INCLUDE_PREFIX,
            std::ptr::null(),
            buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
            &mut out_size,
        )
    };
    if rc != 0 {
        return Err(CollectError::winapi_detailed(
            "iphlpapi.GetAdaptersAddresses",
            "获取本地 IP 地址",
            format!("错误码 {}", rc),
        ));
    }

    let mut result = LocalIps::default();

    // SAFETY: buf 指向 IP_ADAPTER_ADDRESSES_LH 链表头，Next 指针遍历（结构为 POD 链表）
    let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !p.is_null() {
        // SAFETY: 链表节点在 buf 有效范围内
        let adapter = unsafe { &*p };

        // 跳过回环接口（IF_TYPE_SOFTWARE_LOOPBACK = 24）
        if adapter.IfType == 24 {
            p = adapter.Next;
            continue;
        }

        // 遍历单播地址链表
        // SAFETY: FirstUnicastAddress 指向有效节点链表
        let mut addr_p = adapter.FirstUnicastAddress;
        while !addr_p.is_null() {
            // SAFETY: 节点有效
            let addr = unsafe { &*addr_p };
            let sockaddr = addr.Address.lpSockaddr;
            if sockaddr.is_null() {
                addr_p = addr.Next;
                continue;
            }

            // SAFETY: sockaddr 为有效 SOCKADDR（sa_family 首字段）
            let family = unsafe { (*sockaddr).sa_family };

            if family == AF_INET {
                // SAFETY: family 已确认为 AF_INET，可按 SOCKADDR_IN 布局读取
                let sin = unsafe { &*(sockaddr as *const SOCKADDR_IN) };
                // SAFETY: 读取 union 的 S_addr 字段（u32，网络字节序）
                let raw = unsafe { sin.sin_addr.S_un.S_addr };
                let ip = Ipv4Addr::from(raw.to_ne_bytes());
                // 跳过回环（127.x）
                if !ip.is_loopback() && result.ipv4.is_none() {
                    result.ipv4 = Some(ip.to_string());
                }
            } else if family == AF_INET6 {
                // SAFETY: family 已确认为 AF_INET6，可按 SOCKADDR_IN6 布局读取
                let sin6 = unsafe { &*(sockaddr as *const SOCKADDR_IN6) };
                // SAFETY: 读取 union 的 Byte 字段（[u8; 16]）
                let bytes = unsafe { sin6.sin6_addr.u.Byte };
                let ip = Ipv6Addr::from(bytes);
                if !ip.is_loopback()
                    && !ip.is_unspecified()
                    && !is_link_local_v6(&ip)
                    && result.ipv6.is_none()
                {
                    result.ipv6 = Some(ip.to_string());
                }
            }

            addr_p = addr.Next;
        }

        // 现状语义：取第一个非回环非 WellKnown 地址（Select-Object -First 1）
        if result.ipv4.is_some() && result.ipv6.is_some() {
            break;
        }

        p = adapter.Next;
    }

    Ok(result)
}

// ============================================================================
// P4.5 完整网络配置 — GetAdaptersAddresses（MAC / IPv4+掩码 / 网关 / DNS / 链路本地 IPv6）
// ============================================================================

/// 单个适配器的完整网络配置（`ipconfig /all` 语义等价）
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AdapterConfig {
    /// 接口友好名（如 "以太网"，与 Get-NetAdapter 的 Name 同源）
    pub name: String,
    /// 接口描述（如 "Realtek PCIe GbE Family Controller"）
    pub description: String,
    /// 接口 GUID（{...} 格式，注册表 NetCfgInstanceId 匹配键，MAC 修改用）
    pub guid: Option<String>,
    /// 物理地址（MAC，AA-BB-CC-DD-EE-FF 格式；无则 None）
    pub mac: Option<String>,
    /// 接口状态（"Up" / "Down"，对齐 ipconfig 的连接状态）
    pub status: String,
    /// IPv4 地址列表
    pub ipv4: Vec<String>,
    /// IPv4 子网掩码列表（与 ipv4 一一对应，前缀长度换算）
    pub ipv4_mask: Vec<String>,
    /// IPv4 默认网关列表
    pub ipv4_gateway: Vec<String>,
    /// IPv4 DNS 服务器列表
    pub ipv4_dns: Vec<String>,
    /// 链路本地 IPv6 地址列表（fe80::/10，带 %zone 后缀以区分接口，对齐 ipconfig）
    pub ipv6_link_local: Vec<String>,
    /// IPv6 默认网关列表
    pub ipv6_gateway: Vec<String>,
    /// IPv6 DNS 服务器列表
    pub ipv6_dns: Vec<String>,
}

/// 采集所有非回环适配器的完整网络配置（`ipconfig /all` 语义等价）
///
/// 数据源：`GetAdaptersAddresses`（`GAA_FLAG_INCLUDE_PREFIX | GAA_FLAG_INCLUDE_GATEWAYS`），
/// 与 PowerShell `Get-NetIPConfiguration` / `ipconfig /all` 同源。
/// 每条地址项（单播/网关/DNS）解析为 IP 字符串，IPv6 链路本地地址保留 %zone 后缀。
/// 回环接口（IfType=24）跳过，避免前端噪声。
pub fn adapter_configs() -> Result<Vec<AdapterConfig>, CollectError> {
    // 两段式：先探测所需缓冲大小（传 NULL，返回 ERROR_BUFFER_OVERFLOW 并填充 size）
    let mut size: u32 = 0;
    let flags = GAA_FLAG_INCLUDE_PREFIX | GAA_FLAG_INCLUDE_GATEWAYS;
    // SAFETY: 探测调用，NULL 缓冲合法；reserved 参数必须为 NULL
    let rc = unsafe {
        GetAdaptersAddresses(
            AF_UNSPEC as u32,
            flags,
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if rc != 0 && rc != ERROR_BUFFER_OVERFLOW {
        return Err(CollectError::winapi_detailed(
            "iphlpapi.GetAdaptersAddresses",
            "探测网络配置缓冲大小",
            format!("错误码 {}", rc),
        ));
    }

    // 分配缓冲（size 为所需字节数；放大防止地址表在两次调用间增长）
    let buf_len = (size as usize).saturating_add(15 * 1024);
    let mut buf: Vec<u8> = vec![0u8; buf_len];
    let mut out_size: u32 = buf_len as u32;

    // SAFETY: buf 为有效可变缓冲区，长度以 size 传入；API 填充 IP_ADAPTER_ADDRESSES_LH 链表
    let rc = unsafe {
        GetAdaptersAddresses(
            AF_UNSPEC as u32,
            flags,
            std::ptr::null(),
            buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
            &mut out_size,
        )
    };
    if rc != 0 {
        return Err(CollectError::winapi_detailed(
            "iphlpapi.GetAdaptersAddresses",
            "获取网络配置",
            format!("错误码 {}", rc),
        ));
    }

    let mut configs: Vec<AdapterConfig> = Vec::new();

    // SAFETY: buf 指向 IP_ADAPTER_ADDRESSES_LH 链表头，Next 指针遍历（结构为 POD 链表）
    let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !p.is_null() {
        // SAFETY: 链表节点在 buf 有效范围内
        let adapter = unsafe { &*p };

        // 跳过回环接口（IF_TYPE_SOFTWARE_LOOPBACK = 24）
        if adapter.IfType != 24 {
            let mut cfg = AdapterConfig::default();
            cfg.name = wide_ptr_to_string(adapter.FriendlyName);
            cfg.description = wide_ptr_to_string(adapter.Description);
            // 接口 GUID（{...}，注册表 NetCfgInstanceId 匹配键）
            let guid = pstr_to_string(adapter.AdapterName);
            cfg.guid = if guid.is_empty() { None } else { Some(guid) };
            // 物理地址（ipconfig /all 的"物理地址"字段）
            if adapter.PhysicalAddressLength > 0 {
                let len = (adapter.PhysicalAddressLength as usize).min(8);
                let parts: Vec<String> = adapter.PhysicalAddress[..len]
                    .iter()
                    .map(|b| format!("{:02X}", b))
                    .collect();
                cfg.mac = Some(parts.join("-"));
            }
            // 状态（IfOperStatusUp = 1）
            cfg.status = if adapter.OperStatus == 1 {
                "Up".to_string()
            } else {
                "Down".to_string()
            };

            // 单播地址（IPv4 + 掩码、链路本地 IPv6）
            // SAFETY: FirstUnicastAddress 指向有效节点链表
            let mut addr_p = adapter.FirstUnicastAddress;
            while !addr_p.is_null() {
                // SAFETY: 节点有效
                let addr = unsafe { &*addr_p };
                if let Some((ip, family)) = sockaddr_to_ip(addr.Address.lpSockaddr) {
                    if family == AF_INET {
                        cfg.ipv4.push(ip);
                        // 前缀长度 → 子网掩码（与 IP 一一对应）
                        cfg.ipv4_mask.push(prefix_len_to_mask(addr.OnLinkPrefixLength).to_string());
                    } else if family == AF_INET6 {
                        // 仅采集链路本地（fe80::/10，任务契约"连接-本地IPv6地址"）
                        if is_link_local_v6_str(&ip) {
                            cfg.ipv6_link_local.push(ip);
                        }
                    }
                }
                addr_p = addr.Next;
            }

            // 默认网关
            // SAFETY: FirstGatewayAddress 指向有效节点链表
            let mut gw_p = adapter.FirstGatewayAddress;
            while !gw_p.is_null() {
                // SAFETY: 节点有效
                let gw = unsafe { &*gw_p };
                if let Some((ip, family)) = sockaddr_to_ip(gw.Address.lpSockaddr) {
                    if family == AF_INET {
                        cfg.ipv4_gateway.push(ip);
                    } else {
                        cfg.ipv6_gateway.push(ip);
                    }
                }
                gw_p = gw.Next;
            }

            // DNS 服务器
            // SAFETY: FirstDnsServerAddress 指向有效节点链表
            let mut dns_p = adapter.FirstDnsServerAddress;
            while !dns_p.is_null() {
                // SAFETY: 节点有效
                let dns = unsafe { &*dns_p };
                if let Some((ip, family)) = sockaddr_to_ip(dns.Address.lpSockaddr) {
                    if family == AF_INET {
                        cfg.ipv4_dns.push(ip);
                    } else {
                        cfg.ipv6_dns.push(ip);
                    }
                }
                dns_p = dns.Next;
            }

            configs.push(cfg);
        }

        p = adapter.Next;
    }

    log::debug!("netif.adapter_configs: 共 {} 个非回环适配器", configs.len());
    Ok(configs)
}

/// 从 SOCKET_ADDRESS 读取 IP 字符串，返回 (ip 字符串, 地址族)
///
/// IPv6 链路本地地址保留 %zone 后缀（对齐 ipconfig 的"连接-本地 IPv6 地址"显示）；
/// 其余 IPv6 不带 zone。无效指针 / 未知地址族返回 None。
fn sockaddr_to_ip(sockaddr: *const SOCKADDR) -> Option<(String, u16)> {
    if sockaddr.is_null() {
        return None;
    }
    // SAFETY: sockaddr 为有效 SOCKADDR（sa_family 首字段）
    let family = unsafe { (*sockaddr).sa_family };
    match family {
        AF_INET => {
            // SAFETY: family 已确认为 AF_INET，可按 SOCKADDR_IN 布局读取
            let sin = unsafe { &*(sockaddr as *const SOCKADDR_IN) };
            // SAFETY: 读取 union 的 S_addr 字段（u32，网络字节序）
            let raw = unsafe { sin.sin_addr.S_un.S_addr };
            Some((Ipv4Addr::from(raw.to_ne_bytes()).to_string(), AF_INET))
        }
        AF_INET6 => {
            // SAFETY: family 已确认为 AF_INET6，可按 SOCKADDR_IN6 布局读取
            let sin6 = unsafe { &*(sockaddr as *const SOCKADDR_IN6) };
            // SAFETY: 读取 union 的 Byte 字段（[u8; 16]）与 scope id（u32）
            let bytes = unsafe { sin6.sin6_addr.u.Byte };
            let scope = unsafe { sin6.Anonymous.sin6_scope_id };
            let ip = Ipv6Addr::from(bytes);
            if ip.is_loopback() || ip.is_unspecified() {
                return None;
            }
            // 链路本地（fe80::/10）带 %zone 后缀，与 ipconfig 显示一致
            let s = if scope != 0 && is_link_local_v6(&ip) {
                format!("{}%{}", ip, scope)
            } else {
                ip.to_string()
            };
            Some((s, AF_INET6))
        }
        _ => None,
    }
}

/// 判断带 zone 后缀的 IP 字符串是否为链路本地 IPv6（fe80::/10）
fn is_link_local_v6_str(ip_str: &str) -> bool {
    let bare = ip_str.split('%').next().unwrap_or(ip_str);
    match bare.parse::<Ipv6Addr>() {
        Ok(ip) => is_link_local_v6(&ip),
        Err(_) => false,
    }
}

/// IPv4 前缀长度 → 子网掩码（如 24 → 255.255.255.0）
fn prefix_len_to_mask(prefix: u8) -> Ipv4Addr {
    let p = prefix.min(32);
    // p=0 时移位溢出，单独分支
    let mask = if p == 0 { 0u32 } else { u32::MAX << (32 - p) };
    // Ipv4Addr::from(u32) 按网络字节序解释，与掩码语义一致
    Ipv4Addr::from(mask)
}

/// 判断 IPv6 是否链路本地（fe80::/10）
fn is_link_local_v6(ip: &Ipv6Addr) -> bool {
    let seg = ip.segments();
    seg[0] & 0xffc0 == 0xfe80
}

/// 将固定长度 u16 数组（宽字符）转为 String（截至首个 NUL）
fn wide_array_to_string(arr: &[u16]) -> String {
    let end = arr.iter().position(|&c| c == 0).unwrap_or(arr.len());
    String::from_utf16_lossy(&arr[..end])
}

/// 将 NUL 结尾的宽字符指针（PWSTR）转为 String（空指针返回空串）
///
/// GetAdaptersAddresses 的 FriendlyName / Description 为动态分配的 PWSTR，
/// 在缓冲区生命周期内有效（本函数仅在 buf 有效范围内调用）。
fn wide_ptr_to_string(ptr: windows_sys::core::PWSTR) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // 扫描 NUL 结尾长度
    let mut len = 0usize;
    // SAFETY: ptr 指向 API 分配的有效 NUL 结尾宽字符串，越界读取由 NUL 终止保证
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: [0..len) 均为有效 u16 元素
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    String::from_utf16_lossy(slice)
}

/// 将 NUL 结尾的 ANSI 字符串指针（PSTR）转为 String（空指针返回空串）
///
/// GetAdaptersAddresses 的 AdapterName 为窄字符 GUID（如 "{GUID}"），ASCII 安全。
fn pstr_to_string(ptr: windows_sys::core::PSTR) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: ptr 指向 API 分配的有效 NUL 结尾 ANSI 字符串（*mut u8 转 *const i8 布局一致）
    let cstr = unsafe { std::ffi::CStr::from_ptr(ptr.cast::<i8>()) };
    cstr.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fmt_speed_bps() {
        assert_eq!(fmt_speed_bps(1_000_000_000), "1 Gbps");
        assert_eq!(fmt_speed_bps(2_500_000_000), "2.5 Gbps");
        assert_eq!(fmt_speed_bps(100_000_000), "100 Mbps");
        assert_eq!(fmt_speed_bps(10_000_000), "10 Mbps");
        assert_eq!(fmt_speed_bps(1_500_000), "1.5 Mbps");
        assert_eq!(fmt_speed_bps(10_000), "10 Kbps");
        assert_eq!(fmt_speed_bps(500), "500 bps");
    }

    #[test]
    fn test_fmt_speed_regex_shape() {
        // 设计文档验收：格式匹配 ^\d+(\.\d+)? (G|M|K)?bps$（"bps" 前的单位可选）
        for bps in [1u64, 999, 1000, 1500, 1_000_000, 1_500_000, 1_000_000_000] {
            let s = fmt_speed_bps(bps);
            // "1 Gbps" | "100 Mbps" | "500 bps" 等
            let valid = s.ends_with("bps");
            let core = &s[..s.len() - 3]; // 去掉 "bps"
            let core = core.trim();
            // 核心部分形如 "1 G" / "100 M" / "500"
            let parts: Vec<&str> = core.split(' ').collect();
            let num_ok = parts[0].parse::<f64>().is_ok();
            let unit_ok = parts.len() == 1
                || (parts.len() == 2 && matches!(parts[1], "G" | "M" | "K"));
            assert!(valid && num_ok && unit_ok, "速度格式非法: {} (bps={})", s, bps);
        }
    }

    #[test]
    fn test_link_speeds_shape() {
        // 实机：不应失败；若有已连接网卡，返回键非空、值格式合法
        match link_speeds() {
            Ok(map) => {
                for (name, speed) in &map {
                    assert!(!name.is_empty());
                    assert!(speed.ends_with("bps"), "速度格式非法: {}", speed);
                }
            }
            Err(e) => panic!("link_speeds 不应失败: {}", e),
        }
    }

    #[test]
    fn test_local_ips_shape() {
        // 实机：应至少返回一个本地 IP（IPv4 或 IPv6）
        match local_ips() {
            Ok(ips) => {
                assert!(
                    ips.ipv4.is_some() || ips.ipv6.is_some(),
                    "本地 IP 均为空: {:?}",
                    ips
                );
                if let Some(v4) = &ips.ipv4 {
                    assert!(!v4.starts_with("127."), "不应返回回环地址: {}", v4);
                    assert!(!v4.starts_with("0."), "不应返回未指定地址: {}", v4);
                }
                if let Some(v6) = &ips.ipv6 {
                    assert!(!v6.starts_with("fe80"), "不应返回链路本地: {}", v6);
                    assert!(!v6.contains('%'), "不应包含 zone 后缀: {}", v6);
                }
            }
            Err(e) => panic!("local_ips 不应失败: {}", e),
        }
    }

    #[test]
    fn test_wide_array_to_string() {
        let arr: Vec<u16> = "以太网".encode_utf16().chain(std::iter::once(0)).collect();
        let mut padded = [0u16; 257];
        padded[..arr.len()].copy_from_slice(&arr);
        assert_eq!(wide_array_to_string(&padded), "以太网");
    }

    #[test]
    fn test_adapter_configs_shape() {
        // 实机：不应失败；非回环接口均应含名称与状态，字段值与 IP 数一致
        match adapter_configs() {
            Ok(configs) => {
                assert!(!configs.is_empty(), "应至少有一个非回环适配器");
                for c in &configs {
                    assert!(!c.name.is_empty(), "适配器名称不应为空");
                    assert!(c.status == "Up" || c.status == "Down");
                    // 掩码与 IPv4 地址一一对应
                    assert_eq!(c.ipv4.len(), c.ipv4_mask.len(), "掩码应与 IPv4 一一对应: {:?}", c.name);
                    for m in &c.ipv4_mask {
                        assert!(m.parse::<Ipv4Addr>().is_ok(), "掩码非法: {}", m);
                    }
                    if let Some(mac) = &c.mac {
                        // 形如 AA-BB-CC-DD-EE-FF（12 个十六进制字符 + 5 个分隔符）
                        assert_eq!(mac.len(), 17, "MAC 格式非法: {}", mac);
                    }
                }
                // 至少一个接口具备 IPv4 或链路本地 IPv6（实机正常网络环境）
                let has_ip = configs.iter().any(|c| !c.ipv4.is_empty() || !c.ipv6_link_local.is_empty());
                assert!(has_ip, "应至少有一个接口含 IP 地址");
                // 至少一个接口带 GUID（MAC 修改前置条件）
                let has_guid = configs.iter().any(|c| c.guid.is_some());
                assert!(has_guid, "应至少有一个接口含 GUID");
            }
            Err(e) => panic!("adapter_configs 不应失败: {}", e),
        }
    }

    #[test]
    fn test_prefix_len_to_mask() {
        assert_eq!(prefix_len_to_mask(0), Ipv4Addr::from([0, 0, 0, 0]));
        assert_eq!(prefix_len_to_mask(8), Ipv4Addr::from([255, 0, 0, 0]));
        assert_eq!(prefix_len_to_mask(16), Ipv4Addr::from([255, 255, 0, 0]));
        assert_eq!(prefix_len_to_mask(24), Ipv4Addr::from([255, 255, 255, 0]));
        assert_eq!(prefix_len_to_mask(32), Ipv4Addr::from([255, 255, 255, 255]));
        // 越界前缀收敛到 32
        assert_eq!(prefix_len_to_mask(40), Ipv4Addr::from([255, 255, 255, 255]));
    }

    #[test]
    fn test_is_link_local_v6_str() {
        assert!(is_link_local_v6_str("fe80::1%12"));
        assert!(is_link_local_v6_str("fe80::a15c:2f1c:1a2b:3c4d%5"));
        assert!(!is_link_local_v6_str("240e:390:1234::1"));
        assert!(!is_link_local_v6_str("192.168.1.1"));
    }

    #[test]
    fn test_is_link_local_v6() {
        let fe80: Ipv6Addr = "fe80::1".parse().unwrap();
        let normal: Ipv6Addr = "240e::1".parse().unwrap();
        assert!(is_link_local_v6(&fe80));
        assert!(!is_link_local_v6(&normal));
    }
}
