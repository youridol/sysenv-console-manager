/// DHCP 服务器探测模块（本地 DHCP 冲突检测）
///
/// 实现要点（最终方案，2026-08-09 可行性验证定案）：
/// - 主动探测：**SO_REUSEADDR 绑定 UDP 68**（与 Windows Dhcp 客户端服务共存，
///   源端口 68 是标准 RFC 2131 客户端行为），发送 DHCPDISCOVER（flags=0x8000
///   广播标志，强制服务器广播 OFFER），双通道（255.255.255.255:67 广播 +
///   默认网关 :67 单播），收集 3 秒，1 秒后重发 1 次（单次检测最多 4 个
///   DISCOVER 包，防广播风暴）。
///   背景：dnsmasq（家用路由器主力）/ISC dhcpd/Kea 对 DISCOVER 一律固定回
///   68 端口，随机源端口探测会系统性漏报——绑定 68 后全部可检测（实测验证）。
/// - 降级：绑定 68 失败时回退随机高端口（6800-6899），note 注明局限。
/// - OFFER 解析：校验 magic cookie 0x63825363 与 op=2（BOOTREPLY），
///   提取 options 53（message type）与 54（server identifier）。
/// - 基线组合：读取注册表当前分配 DHCP 服务器 IP（DhcpServer 值），
///   与主动探测结果并集去重。
///
/// 参考：RFC 2131 (DHCP)、RFC 2132 (DHCP Options)
use rand::Rng;
use serde::Serialize;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};
use winreg::enums::HKEY_LOCAL_MACHINE;
use winreg::RegKey;

// ============================================================================
// 常量
// ============================================================================

/// DHCP 服务端 UDP 端口（RFC 2131：服务端 67，客户端 68）
const DHCP_SERVER_PORT: u16 = 67;
/// DHCP 客户端 UDP 端口（RFC 2131）。
/// 我们以 SO_REUSEADDR 与 Windows Dhcp 服务共存绑定 68，使服务器按标准
/// 客户端行为响应到 68（dnsmasq/ISC 一律固定回 68），广播 OFFER 可被共享接收。
const DHCP_CLIENT_PORT: u16 = 68;
/// 降级探测源端口范围下限（绑定 68 失败时使用；固定响应 68 的服务器可能漏检）
const SRC_PORT_MIN: u16 = 6800;
/// 降级探测源端口范围上限
const SRC_PORT_MAX: u16 = 6899;
/// 收集窗口（毫秒）
const COLLECT_WINDOW_MS: u64 = 3000;
/// 重发间隔（毫秒）
const RESEND_INTERVAL_MS: u64 = 1000;
/// 单次 recv 超时（毫秒），用于循环内定期检查 deadline
const RECV_TIMEOUT_MS: u64 = 400;

/// DHCP magic cookie（RFC 2131 4.1）
const DHCP_MAGIC_COOKIE: [u8; 4] = [0x63, 0x82, 0x53, 0x63];

/// DHCP option 常量（RFC 2132）
const OPT_MESSAGE_TYPE: u8 = 53;
const OPT_SERVER_IDENTIFIER: u8 = 54;
const OPT_PARAM_REQUEST_LIST: u8 = 55;
const OPT_END: u8 = 255;

/// DHCP message type 值（RFC 2132 9.6）
const MSG_TYPE_DISCOVER: u8 = 1;
const MSG_TYPE_OFFER: u8 = 2;

/// DHCP 参数请求列表（option 55）：Subnet Mask / Router / DNS / Hostname /
/// Domain / Broadcast / Lease Time / Server Identifier
const PARAM_REQUEST_LIST: [u8; 8] = [1, 3, 6, 12, 15, 28, 51, 54];

// ============================================================================
// 结果契约（Tauri 命令返回结构，前端直接渲染）
// ============================================================================

/// DHCP 探测结果
#[derive(Debug, Clone, Serialize)]
pub struct DhcpProbeResult {
    /// 去重后的 DHCP 服务器 IP 列表（主动探测源 IP ∪ 基线当前分配服务器）
    pub servers: Vec<String>,
    /// 服务器数量
    pub count: usize,
    /// 健康 = count <= 1
    pub healthy: bool,
    /// 基线：当前分配 DHCP 服务器（注册表，可能为 None）
    pub baseline_server: Option<String>,
    /// 检测说明（含局限提示，如「部分服务器固定响应 68 端口可能漏检」）
    pub note: String,
}

// ============================================================================
// 报文构造（RFC 2131 4.1）
// ============================================================================

/// 构造 DHCPDISCOVER 报文（BOOTP header 236 字节 + magic cookie + options）
///
/// 固定字段：op=1（BOOTREQUEST）、htype=1（Ethernet）、hlen=6、flags=0x8000（广播响应）。
fn build_discover(xid: u32, chaddr: &[u8; 6]) -> Vec<u8> {
    let mut pkt = vec![0u8; 236];
    pkt[0] = 1; // op: BOOTREQUEST
    pkt[1] = 1; // htype: Ethernet
    pkt[2] = 6; // hlen: 6 字节 MAC
    pkt[4..8].copy_from_slice(&xid.to_be_bytes()); // xid
    pkt[10..12].copy_from_slice(&0x8000u16.to_be_bytes()); // flags: 广播响应
    pkt[28..34].copy_from_slice(chaddr); // chaddr: 客户端 MAC（前 6 字节）
    pkt.extend_from_slice(&DHCP_MAGIC_COOKIE); // magic cookie @ offset 236
    // option 53: message type = DISCOVER
    pkt.push(OPT_MESSAGE_TYPE);
    pkt.push(1);
    pkt.push(MSG_TYPE_DISCOVER);
    // option 55: 参数请求列表
    pkt.push(OPT_PARAM_REQUEST_LIST);
    pkt.push(PARAM_REQUEST_LIST.len() as u8);
    pkt.extend_from_slice(&PARAM_REQUEST_LIST);
    // option 255: END
    pkt.push(OPT_END);
    pkt
}

// ============================================================================
// OFFER 解析
// ============================================================================

/// OFFER 报文解析结果
#[derive(Debug, Clone)]
struct OfferInfo {
    /// option 53 的 message type（OFFER=2）；生产路径已由 parse_offer 过滤为 OFFER，
    /// 保留字段供单测直接断言 message type（dead_code 由测试消费，故显式放行）。
    #[allow(dead_code)]
    message_type: u8,
    /// option 54 的 server identifier（DHCP 服务器 IP）
    server_identifier: Option<Ipv4Addr>,
}

/// 解析 DHCP OFFER 报文。
///
/// 校验：op=2（BOOTREPLY）、magic cookie 0x63825363、options 53 = OFFER。
/// 非法报文（短包 / 坏 cookie / 非 OFFER / 非 BOOTREPLY）返回 None，不 panic。
fn parse_offer(pkt: &[u8]) -> Option<OfferInfo> {
    // 最小长度：BOOTP 头 236 + magic cookie 4 + 至少 1 个 option 头 2 = 242
    if pkt.len() < 242 || pkt[0] != 2 || pkt[236..240] != DHCP_MAGIC_COOKIE {
        return None;
    }
    let mut message_type: Option<u8> = None;
    let mut server_identifier: Option<Ipv4Addr> = None;
    let mut pos = 240;
    while pos < pkt.len() {
        let code = pkt[pos];
        if code == OPT_END {
            break;
        }
        if code == 0 {
            // option padding（填充字节）
            pos += 1;
            continue;
        }
        if pos + 1 >= pkt.len() {
            break;
        }
        let len = pkt[pos + 1] as usize;
        let val_start = pos + 2;
        if val_start + len > pkt.len() {
            break;
        }
        match code {
            OPT_MESSAGE_TYPE if len >= 1 => message_type = Some(pkt[val_start]),
            OPT_SERVER_IDENTIFIER if len >= 4 => {
                server_identifier = Some(Ipv4Addr::new(
                    pkt[val_start],
                    pkt[val_start + 1],
                    pkt[val_start + 2],
                    pkt[val_start + 3],
                ));
            }
            _ => {}
        }
        pos = val_start + len;
    }
    if message_type == Some(MSG_TYPE_OFFER) {
        Some(OfferInfo {
            message_type: MSG_TYPE_OFFER,
            server_identifier,
        })
    } else {
        None
    }
}

// ============================================================================
// 去重与基线合并（纯函数，可单测）
// ============================================================================

/// 保序去重服务器 IP 列表（跳过空串与纯空白项）
fn dedup_servers(servers: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for s in servers {
        let t = s.trim().to_string();
        if t.is_empty() {
            continue;
        }
        if seen.insert(t.clone()) {
            out.push(t);
        }
    }
    out
}

/// 将基线（当前分配 DHCP 服务器）合并进主动探测结果，去重保序
fn merge_baseline(probed: Vec<String>, baseline: Option<String>) -> Vec<String> {
    let mut merged = probed;
    if let Some(b) = baseline {
        let t = b.trim().to_string();
        if !t.is_empty() {
            merged.push(t);
        }
    }
    dedup_servers(&merged)
}

// ============================================================================
// 注册表网络配置读取
// ============================================================================

/// 从注册表解析的网络配置（活动接口 + 基线 DHCP 服务器）
#[derive(Debug, Default, Clone)]
struct NetConfig {
    /// 活动接口 IPv4（首选有网关的接口）
    interface_ip: Option<Ipv4Addr>,
    /// 活动接口默认网关（静态或 DHCP 分配）
    gateway: Option<Ipv4Addr>,
    /// 活动接口 IPv4 子网掩码（与 interface_ip 对应）
    subnet_mask: Option<Ipv4Addr>,
    /// 当前分配的 DHCP 服务器 IP（基线）
    baseline_dhcp_server: Option<String>,
}

/// 读取网络配置：活动接口 IP/网关 + 基线 DHCP 服务器
///
/// 数据源：HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces
/// 各接口子键的 IPAddress（REG_MULTI_SZ）、DefaultGateway / DhcpDefaultGateway
/// （REG_MULTI_SZ）、DhcpServer（REG_SZ）。
///
/// 读取失败或键缺失时返回默认值（全部 None），调用方据此降级
/// （bind 0.0.0.0 / 仅广播通道），不中断探测。
fn read_network_config() -> NetConfig {
    let mut cfg = NetConfig::default();
    let hklm = match RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces")
    {
        Ok(k) => k,
        Err(e) => {
            crate::debug_warn!(
                "dhcp_probe",
                "读取网络接口注册表失败（RegKey::open_subkey, err={}），使用默认降级配置",
                e
            );
            return cfg;
        }
    };
    // 活动接口选择：有网关优先（分数 1），否则有 IP（分数 0）
    let mut best_score: i32 = -1;
    // 仅 DHCP 接口（EnableDHCP=1）中第一个有效的 DhcpServer（活动接口无值时回退用；
    // 静态接口/历史租约记录不提供基线，避免假冲突）
    let mut first_dhcp: Option<String> = None;
    for sub in hklm.enum_keys() {
        let Ok(name) = sub else { continue };
        let Ok(key) = hklm.open_subkey(&name) else {
            continue;
        };
        // 接口 IP 列表（REG_MULTI_SZ，可能多 IP）
        let ips = read_multi_strings(&key, "IPAddress");
        let ip = ips.iter().find_map(|s| {
            s.trim()
                .parse::<Ipv4Addr>()
                .ok()
                .filter(|a| !a.is_unspecified())
        });
        // 子网掩码（REG_MULTI_SZ，与 IPAddress 按索引一一对应；取首个 IP+掩码均有效的配对）
        let masks = read_multi_strings(&key, "SubnetMask");
        let mask = ips
            .iter()
            .zip(masks.iter())
            .find_map(|(i, m)| {
                let ip_ok = i.trim().parse::<Ipv4Addr>().ok().filter(|a| !a.is_unspecified());
                let mask_ok = m.trim().parse::<Ipv4Addr>().ok();
                ip_ok.zip(mask_ok)
            })
            .map(|(_, m)| m);
        // 网关：静态 DefaultGateway 优先，DHCP 分配 DhcpDefaultGateway 次之
        let gw = read_gateway(&key);
        // 仅 DHCP 接口（EnableDHCP=1）的 DhcpServer 可作基线（静态接口/历史租约不污染）
        let dhcp_enabled = key.get_value::<u32, _>("EnableDHCP").unwrap_or(0) == 1;
        // 本接口当前分配的 DHCP 服务器（过滤空值与 255.255.255.255）
        let dhcp = if dhcp_enabled {
            key.get_value::<String, _>("DhcpServer")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && s != "255.255.255.255")
        } else {
            None
        };
        if first_dhcp.is_none() {
            first_dhcp = dhcp.clone();
        }
        let score = if gw.is_some() { 1 } else { 0 };
        if let Some(ip) = ip {
            if score > best_score {
                best_score = score;
                cfg.interface_ip = Some(ip);
                cfg.gateway = gw;
                cfg.subnet_mask = mask;
                // 基线优先取活动接口自身的 DhcpServer（残留接口记录不作为首选）
                cfg.baseline_dhcp_server = dhcp.clone();
            }
        }
    }
    // 活动接口无有效 DhcpServer 时，回退 DHCP 接口中第一个有效值
    if cfg.baseline_dhcp_server.is_none() {
        cfg.baseline_dhcp_server = first_dhcp;
    }
    cfg
}

/// 读取 REG_MULTI_SZ 字符串列表（类型不符或缺失时返回空列表，不中断）
fn read_multi_strings(key: &RegKey, name: &str) -> Vec<String> {
    key.get_value::<Vec<String>, _>(name).unwrap_or_default()
}

/// 读取接口网关：DefaultGateway（静态）优先，DhcpDefaultGateway（DHCP）次之
fn read_gateway(key: &RegKey) -> Option<Ipv4Addr> {
    for name in ["DefaultGateway", "DhcpDefaultGateway"] {
        let v = read_multi_strings(key, name);
        if let Some(gw) = v.iter().find_map(|s| {
            s.trim()
                .parse::<Ipv4Addr>()
                .ok()
                .filter(|a| !a.is_unspecified())
        }) {
            return Some(gw);
        }
    }
    None
}

// ============================================================================
// 活动网卡 MAC
// ============================================================================

/// 获取第一个非零 MAC 地址（sysinfo Networks），用于 DISCOVER 报文 chaddr。
///
/// 多网卡环境下取任意活动网卡 MAC 即可——服务器 OFFER 响应回到探测源端口，
/// chaddr 仅作客户端标识，不参与响应路由。
fn get_primary_mac() -> Option<[u8; 6]> {
    let networks = sysinfo::Networks::new_with_refreshed_list();
    for (_name, data) in networks.list() {
        let mac = data.mac_address();
        if !mac.is_unspecified() {
            return Some(mac.0);
        }
    }
    None
}

// ============================================================================
// 探测收集循环
// ============================================================================

/// 收集循环：发送 DISCOVER → 监听 OFFER 直到 deadline，中途 1 次重发。
///
/// 参数化时长（collect_ms / resend_ms）以便单测用短窗口覆盖超时路径。
/// 返回 (去重后的服务器 IP 列表, 是否至少一次发送成功)。
fn collect_servers(
    socket: &UdpSocket,
    targets: &[SocketAddr],
    pkt: &[u8],
    xid: u32,
    collect_ms: u64,
    resend_ms: u64,
) -> (Vec<String>, bool) {
    let start = Instant::now();
    let deadline = start + Duration::from_millis(collect_ms);
    let resend_at = start + Duration::from_millis(resend_ms);
    let mut sent_ok = false;
    let mut resent = false;
    let mut servers: Vec<String> = Vec::new();
    let mut buf = [0u8; 1024];
    let xid_be = xid.to_be_bytes();

    // 初始发送（广播 + 单播双通道）
    sent_ok |= send_discover(socket, targets, pkt);

    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        // 到重发时刻且未重发 → 重发一次（广播风暴防护：总包数 ≤ 4）
        if !resent && now >= resend_at {
            resent = true;
            sent_ok |= send_discover(socket, targets, pkt);
        }
        match socket.recv_from(&mut buf) {
            Ok((len, src)) => {
                let data = &buf[..len];
                // xid 快速匹配：长度足够且 xid 一致才解析，避免误收其他事务
                if data.len() >= 8 && data[4..8] == xid_be {
                    if let Some(off) = parse_offer(data) {
                        // server_identifier（option 54）优先；缺失时回退响应源 IP
                        let server = off
                            .server_identifier
                            .map(|ip| ip.to_string())
                            .unwrap_or_else(|| src.ip().to_string());
                        servers.push(server);
                    }
                }
            }
            Err(_) => {
                // 超时 / ECONNRESET（发往无监听端口的 ICMP 回馈）等：
                // 忽略并继续循环，deadline 由循环头控制
            }
        }
    }
    (dedup_servers(&servers), sent_ok)
}

/// 向所有目标发送 DISCOVER，返回是否至少一个通道成功
fn send_discover(socket: &UdpSocket, targets: &[SocketAddr], pkt: &[u8]) -> bool {
    let mut ok = false;
    for t in targets {
        match socket.send_to(pkt, t) {
            Ok(_) => ok = true,
            Err(e) => {
                crate::debug_warn!("dhcp_probe", "send_to({}) 失败: {}", t, e);
            }
        }
    }
    ok
}

/// 创建探测 socket。
///
/// 首选：socket2 设置 SO_REUSEADDR 后绑定 UDP 68（与 Windows Dhcp 客户端服务共存，
/// 源端口 68 是标准 RFC 2131 客户端行为；主流 DHCP 服务器 dnsmasq/ISC 固定响应到 68，
/// 绑定 68 即可收到 OFFER；SO_REUSEADDR 使广播 OFFER 被多 socket 共享接收）。
/// 降级：绑定失败（个别系统不允许共存）时回退随机高端口（6800-6899），
/// 此时固定响应 68 的服务器可能漏检（返回降级标记供 note 提示局限）。
///
/// 返回 (socket, 是否降级到随机高端口)。
fn bind_probe_socket(interface_ip: Option<Ipv4Addr>) -> Result<(UdpSocket, bool), String> {
    // 首选：SO_REUSEADDR 绑定 68（与 Dhcp 服务共存，标准客户端行为）
    let domain = socket2::Domain::IPV4;
    let ty = socket2::Type::DGRAM;
    let reuse_result = socket2::Socket::new(domain, ty, Some(socket2::Protocol::UDP))
        .and_then(|raw| {
            raw.set_reuse_address(true)?;
            raw.set_broadcast(true)?;
            let bind_addr = match interface_ip {
                Some(ip) => SocketAddr::from((ip, DHCP_CLIENT_PORT)),
                None => SocketAddr::from((Ipv4Addr::UNSPECIFIED, DHCP_CLIENT_PORT)),
            };
            raw.bind(&socket2::SockAddr::from(bind_addr))?;
            raw.set_read_timeout(Some(Duration::from_millis(RECV_TIMEOUT_MS)))?;
            Ok::<socket2::Socket, std::io::Error>(raw)
        });
    match reuse_result {
        Ok(raw) => {
            let sock = UdpSocket::from(raw);
            crate::debug_info!(
                "dhcp_probe",
                "SO_REUSEADDR 绑定 UDP {} 成功（与 Dhcp 服务共存）",
                DHCP_CLIENT_PORT
            );
            return Ok((sock, false));
        }
        Err(e) => {
            crate::debug_warn!(
                "dhcp_probe",
                "SO_REUSEADDR 绑定 UDP {} 失败（{}, err={}），降级随机高端口",
                DHCP_CLIENT_PORT,
                interface_ip
                    .map(|ip| ip.to_string())
                    .unwrap_or_else(|| "0.0.0.0".to_string()),
                e
            );
        }
    }
    // 降级：随机高端口
    let src_port = random_src_port();
    let bind_addr = interface_ip
        .map(|ip| SocketAddr::from((ip, src_port)))
        .unwrap_or_else(|| SocketAddr::from((Ipv4Addr::UNSPECIFIED, src_port)));
    let sock = UdpSocket::bind(bind_addr).map_err(|e| {
        format!(
            "dhcp_probe: UdpSocket::bind({}) 失败: {}（建议：检查网络适配器状态后重试）",
            bind_addr, e
        )
    })?;
    sock.set_broadcast(true).map_err(|e| {
        format!(
            "dhcp_probe: UdpSocket::set_broadcast(true) 失败: {}（建议：检查防火墙与网络权限后重试）",
            e
        )
    })?;
    sock.set_read_timeout(Some(Duration::from_millis(RECV_TIMEOUT_MS)))
        .map_err(|e| format!("dhcp_probe: UdpSocket::set_read_timeout 失败: {}", e))?;
    Ok((sock, true))
}

// ============================================================================
// 主探测入口
// ============================================================================

/// 检测本地子网内的 DHCP 服务器（主动探测 ∪ 注册表基线，去重计数）
pub fn probe_dhcp_servers() -> Result<DhcpProbeResult, String> {
    let started = Instant::now();

    // 1. 读取网络配置（活动接口 IP / 网关 / 基线 DHCP 服务器）
    let cfg = read_network_config();

    // 2. 构造 DISCOVER 报文（随机 xid + 活动网卡 MAC）
    let xid = new_xid();
    let chaddr = get_primary_mac().unwrap_or([0u8; 6]);
    let pkt = build_discover(xid, &chaddr);

    // 3. 绑定 socket：SO_REUSEADDR 绑定 68（与 Dhcp 服务共存），失败降级随机高端口
    let (socket, degraded_port) = bind_probe_socket(cfg.interface_ip)?;

    // 4. 双通道目标：广播 255.255.255.255:67 + 网关 :67 单播（网关存在时）
    let mut targets: Vec<SocketAddr> = vec![SocketAddr::from((
        Ipv4Addr::BROADCAST,
        DHCP_SERVER_PORT,
    ))];
    if let Some(gw) = cfg.gateway {
        targets.push(SocketAddr::from((gw, DHCP_SERVER_PORT)));
    }

    // 5. 收集 OFFER（3 秒窗口 + 1 秒后重发 1 次，最多 4 个 DISCOVER 包）
    let (probed, sent_ok) = collect_servers(
        &socket,
        &targets,
        &pkt,
        xid,
        COLLECT_WINDOW_MS,
        RESEND_INTERVAL_MS,
    );
    if !sent_ok {
        return Err(
            "dhcp_probe: DHCP Discover 广播与单播通道均发送失败（UDP send_to 全部失败，建议：检查网络连接与防火墙后重试）"
                .to_string(),
        );
    }

    // 6. 合并基线 + 生成说明
    let baseline = cfg.baseline_dhcp_server;
    let servers = merge_baseline(probed.clone(), baseline.clone());
    let count = servers.len();
    let healthy = count <= 1;
    let note = build_note(&servers, &probed, &baseline, degraded_port);

    crate::debug_info!(
        "dhcp_probe",
        "探测完成: probed={:?} baseline={:?} servers={:?} degraded={} elapsed={}ms",
        probed,
        baseline,
        servers,
        degraded_port,
        started.elapsed().as_millis()
    );

    Ok(DhcpProbeResult {
        servers,
        count,
        healthy,
        baseline_server: baseline,
        note,
    })
}

/// 随机 xid（非零，避免全零事务被服务器忽略）
fn new_xid() -> u32 {
    let mut rng = rand::thread_rng();
    let x = rng.gen::<u32>();
    if x == 0 { 1 } else { x }
}

/// 随机探测源端口（6800-6899）
fn random_src_port() -> u16 {
    let mut rng = rand::thread_rng();
    SRC_PORT_MIN + rng.gen_range(0..=(SRC_PORT_MAX - SRC_PORT_MIN))
}

/// 生成中文检测说明（按场景区分文案；降级模式追加局限提示）
fn build_note(
    servers: &[String],
    probed: &[String],
    baseline: &Option<String>,
    degraded: bool,
) -> String {
    let degrade_hint = if degraded {
        "（注意：本机无法绑定标准 DHCP 客户端端口 68，已降级为随机源端口探测，部分固定响应 68 端口的服务器可能漏检）"
    } else {
        ""
    };
    match servers.len() {
        0 => format!(
            "未检测到 DHCP 服务器。当前网络可能未启用 DHCP；部分 DHCP 服务器固定响应 68 端口（被 Windows DHCP 客户端占用）可能漏检。{}",
            degrade_hint
        ),
        1 if probed.is_empty() && baseline.is_some() => format!(
            "检测到 1 台 DHCP 服务器（来自系统当前 DHCP 分配记录），DHCP 服务正常。主动探测未捕获到响应——部分 DHCP 服务器固定响应 68 端口可能漏检。{}",
            degrade_hint
        ),
        1 => format!("检测到 1 台 DHCP 服务器，DHCP 服务正常。{}", degrade_hint),
        n => format!(
            "检测到 {} 台 DHCP 服务器，可能存在 DHCP 冲突！请检查：① 路由器 DHCP 设置 ② 多路由器级联 ③ 软路由/旁路由/热点 ④ AP 的 DHCP 是否关闭。{}",
            n, degrade_hint
        ),
    }
}

// ============================================================================
// 深度检查（可达性 + 网段关系 + 网关对比）
// ============================================================================

/// 单台 DHCP 服务器的深度检查结果
#[derive(Debug, Clone, Serialize)]
pub struct DhcpServerCheck {
    /// 服务器 IP
    pub server: String,
    /// ICMP ping 是否可达
    pub ping_ok: bool,
    /// 平均 RTT（毫秒，不可达为 0）
    pub ping_rtt_ms: f64,
    /// 与活动接口是否同一子网（掩码对比）
    pub same_subnet: bool,
    /// 是否等于默认网关（DHCP 服务器即网关为正常拓扑）
    pub is_gateway: bool,
    /// 严重级别：ok / warn / critical
    pub severity: String,
    /// 中文诊断建议
    pub diagnosis: String,
}

/// DHCP 深度检查结果（探测 ∪ 可达性 ∪ 拓扑关系）
#[derive(Debug, Clone, Serialize)]
pub struct DhcpDeepCheckResult {
    /// 活动接口 IPv4
    pub local_ip: Option<String>,
    /// 默认网关
    pub gateway: Option<String>,
    /// 子网掩码
    pub subnet_mask: Option<String>,
    /// 检测到的全部服务器 IP
    pub servers: Vec<String>,
    /// 每台服务器的深度检查
    pub checks: Vec<DhcpServerCheck>,
    /// 综合诊断结论
    pub summary: String,
}

/// 生成单台服务器的诊断结论（纯函数，可单测）
///
/// 判定矩阵（severity / diagnosis）：
/// - 可达且 = 网关 → ok：服务正常
/// - 可达且同网段但 ≠ 网关 → warn：多 DHCP 冲突风险
/// - 可达但跨网段 → critical：OFFER 经 DHCP 中继/另一广播域，地址分配异常风险
/// - 不可达且 = 网关 → critical：网关不可达，网络连接异常
/// - 不可达但同网段 → warn：禁 ICMP 或已下线仍响应
/// - 不可达且跨网段 → critical：中继/遗留设备
fn build_server_check(
    server: &str,
    local_ip: Option<Ipv4Addr>,
    gateway: Option<Ipv4Addr>,
    mask: Option<Ipv4Addr>,
    ping_ok: bool,
    rtt_ms: f64,
) -> DhcpServerCheck {
    let server_ip = server.trim().parse::<Ipv4Addr>().ok();
    // 同网段判定：server_ip & mask == local_ip & mask（IPv4 点分十进制按字节比较）
    let same_subnet = match (server_ip, local_ip, mask) {
        (Some(s), Some(l), Some(m)) => {
            let (ms, ml, mm) = (mask_u32(s), mask_u32(l), mask_u32(m));
            (ms & mm) == (ml & mm)
        }
        _ => false,
    };
    let is_gateway = server_ip.is_some() && gateway == server_ip;

    // 本地网络配置缺失（读取失败降级）：same_subnet 无法计算（mask/local_ip 未知），
    // 此时同网段/跨网段均属"不可判定"——不得误报为 critical 跨网段风险
    let config_missing = local_ip.is_none() || mask.is_none();

    let (severity, diagnosis) = match (ping_ok, is_gateway, same_subnet) {
        (true, true, _) => (
            "ok",
            "DHCP 服务器即默认网关且可达，DHCP 服务正常".to_string(),
        ),
        (true, false, true) => (
            "warn",
            "同网段但非默认网关：可能存在多台 DHCP 服务器，建议检查路由器/AP/软路由的 DHCP 设置".to_string(),
        ),
        // 配置缺失时不可判定：降级为 warn（与测试契约一致，避免配置读取失败误报跨网段）
        (true, false, false) if config_missing => (
            "warn",
            "本地网络配置缺失（读取失败降级），无法判定是否跨网段：按不可判定处理".to_string(),
        ),
        (true, false, false) => (
            "critical",
            "跨网段 DHCP 服务器：OFFER 经 DHCP 中继（ip helper）或另一广播域转发而来，本机无法直连，存在地址分配异常风险".to_string(),
        ),
        (false, true, _) => (
            "critical",
            "DHCP 服务器即默认网关但不可达：网络连接异常，检查网线/无线/防火墙（或网关禁 ICMP）".to_string(),
        ),
        (false, false, true) => (
            "warn",
            "同网段但 ping 不可达：服务器可能禁用了 ICMP，或已下线但仍响应 OFFER".to_string(),
        ),
        // 配置缺失时不可判定：降级为 warn（同上）
        (false, false, false) if config_missing => (
            "warn",
            "本地网络配置缺失且服务器不可达：无法判定跨网段状态，按不可判定处理".to_string(),
        ),
        (false, false, false) => (
            "critical",
            "跨网段且不可达：OFFER 可能来自已下线的 DHCP 中继/遗留设备，或服务器禁 ICMP，本机无法直接验证其状态".to_string(),
        ),
    };

    DhcpServerCheck {
        server: server.to_string(),
        ping_ok,
        ping_rtt_ms: rtt_ms,
        same_subnet,
        is_gateway,
        severity: severity.to_string(),
        diagnosis,
    }
}

/// 将 IPv4 转为 u32（网络字节序语义，用于掩码运算）
fn mask_u32(ip: Ipv4Addr) -> u32 {
    u32::from_be_bytes(ip.octets())
}

/// 生成综合诊断结论（纯函数，可单测）
///
/// 覆盖用户场景：DHCP 服务器与默认网关不同网段（如检测到 192.168.11.1 而网关 192.168.3.254）。
fn build_deep_summary(
    checks: &[DhcpServerCheck],
    local_ip: Option<Ipv4Addr>,
    gateway: Option<Ipv4Addr>,
) -> String {
    if checks.is_empty() {
        return "未检测到 DHCP 服务器，无需深度检查".to_string();
    }
    let criticals: Vec<&DhcpServerCheck> = checks
        .iter()
        .filter(|c| c.severity == "critical")
        .collect();
    let warns: Vec<&DhcpServerCheck> = checks
        .iter()
        .filter(|c| c.severity == "warn")
        .collect();

    // 跨网段 + 网关对比：用户典型场景（DHCP 服务器 192.168.11.1 ≠ 网关 192.168.3.254）
    let cross_subnet: Vec<String> = checks
        .iter()
        .filter(|c| !c.same_subnet)
        .map(|c| c.server.clone())
        .collect();
    if !cross_subnet.is_empty() {
        let gw_txt = gateway
            .map(|g| g.to_string())
            .unwrap_or_else(|| "未知".to_string());
        let local_txt = local_ip
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "未知".to_string());
        return format!(
            "⚠ 检测到跨网段 DHCP 服务器：{} 与本机 IP {} 不在同一网段，而默认网关为 {}。OFFER 通常经 DHCP 中继（ip helper）或旁路由转发，本机无法直接 ping 通属预期现象；建议检查主路由 DHCP 是否开启、是否存在旁路由/软路由二次分配，优先保留与网关同网段的 DHCP 服务。",
            cross_subnet.join(", "),
            local_txt,
            gw_txt,
        );
    }

    if criticals.is_empty() && warns.is_empty() {
        return "全部正常：检测到的 DHCP 服务器即默认网关且可达".to_string();
    }
    let mut parts = Vec::new();
    if !criticals.is_empty() {
        parts.push(format!(
            "存在 {} 台异常 DHCP 服务器（{}）：请优先处理，检查网络拓扑与 DHCP 配置",
            criticals.len(),
            criticals
                .iter()
                .map(|c| c.server.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !warns.is_empty() {
        parts.push(format!(
            "存在 {} 台需关注服务器（{}）：建议核查多 DHCP 冲突或 ICMP 策略",
            warns.len(),
            warns
                .iter()
                .map(|c| c.server.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    parts.join("；")
}

/// 对单台服务器执行 ICMP ping（1 包，2 秒超时，返回 (可达, RTT ms)）
///
/// 独立于 network.rs 的流式 ping：深度检查只需快速可达性，失败静默返回不可达。
async fn ping_server(ip: &str) -> (bool, f64) {
    use surge_ping::{Client, Config, PingIdentifier, PingSequence, ICMP};

    let addr: std::net::IpAddr = match ip.trim().parse() {
        Ok(a) => a,
        Err(_) => return (false, 0.0),
    };
    let kind = if addr.is_ipv6() { ICMP::V6 } else { ICMP::V4 };
    let cfg = Config::builder().kind(kind).build();
    let client = match Client::new(&cfg) {
        Ok(c) => c,
        Err(_) => return (false, 0.0),
    };
    let mut pinger = client.pinger(addr, PingIdentifier(rand::random())).await;
    pinger.timeout(Duration::from_secs(2));
    let start = std::time::Instant::now();
    match pinger.ping(PingSequence(1), &[0u8; 32]).await {
        Ok(_) => (true, start.elapsed().as_secs_f64() * 1000.0),
        Err(_) => (false, 0.0),
    }
}

/// DHCP 深度检查主流程：探测服务器 → 读取本地配置 → 并发 ping → 组装诊断
///
/// 覆盖用户场景：深度检查 DHCP 服务器可达性，并对比默认网关与子网关系，
/// 识别跨网段中继 OFFER / 多 DHCP 冲突 / 网关不可达等异常。
pub async fn deep_check_dhcp_servers() -> Result<DhcpDeepCheckResult, String> {
    let started = Instant::now();

    // 1. 主动探测 DHCP 服务器（同步，spawn_blocking）
    let probe = tokio::task::spawn_blocking(probe_dhcp_servers)
        .await
        .map_err(|e| format!("deep_check_dhcp_servers panicked (probe): {}", e))??;
    let servers = probe.servers;

    // 2. 读取本地网络配置（同步，spawn_blocking）
    let cfg = tokio::task::spawn_blocking(read_network_config)
        .await
        .map_err(|e| format!("deep_check_dhcp_servers panicked (config): {}", e))?;

    // 3. 并发 ping 每台服务器（JoinSet，避免串行等待）
    let mut set = tokio::task::JoinSet::new();
    for s in &servers {
        let s = s.clone();
        set.spawn(async move {
            let r = ping_server(&s).await;
            (s, r)
        });
    }
    let mut ping_map: std::collections::HashMap<String, (bool, f64)> =
        std::collections::HashMap::new();
    while let Some(res) = set.join_next().await {
        match res {
            Ok((s, r)) => {
                ping_map.insert(s, r);
            }
            Err(e) => {
                crate::debug_warn!("dhcp_probe", "深度检查 ping 任务失败: {}", e);
            }
        }
    }

    // 4. 组装逐台检查 + 综合结论
    let checks: Vec<DhcpServerCheck> = servers
        .iter()
        .map(|s| {
            let (ping_ok, rtt) = ping_map.get(s).copied().unwrap_or((false, 0.0));
            build_server_check(s, cfg.interface_ip, cfg.gateway, cfg.subnet_mask, ping_ok, rtt)
        })
        .collect();
    let summary = build_deep_summary(&checks, cfg.interface_ip, cfg.gateway);

    crate::debug_info!(
        "dhcp_probe",
        "深度检查完成: servers={:?} elapsed={}ms",
        servers,
        started.elapsed().as_millis()
    );

    Ok(DhcpDeepCheckResult {
        local_ip: cfg.interface_ip.map(|ip| ip.to_string()),
        gateway: cfg.gateway.map(|ip| ip.to_string()),
        subnet_mask: cfg.subnet_mask.map(|ip| ip.to_string()),
        servers,
        checks,
        summary,
    })
}

// ============================================================================
// 单测
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造合法 OFFER 报文的辅助函数（op=2 + magic cookie + options 53/54）
    fn build_test_reply(xid: u32, msg_type: u8, server_ip: [u8; 4]) -> Vec<u8> {
        let mut pkt = vec![0u8; 240];
        pkt[0] = 2; // op: BOOTREPLY
        pkt[1] = 1; // htype: Ethernet
        pkt[2] = 6; // hlen
        pkt[4..8].copy_from_slice(&xid.to_be_bytes());
        pkt[28] = 0x02; // chaddr 首字节非零即可
        pkt[236..240].copy_from_slice(&DHCP_MAGIC_COOKIE);
        // option 53: message type
        pkt.push(OPT_MESSAGE_TYPE);
        pkt.push(1);
        pkt.push(msg_type);
        // option 54: server identifier
        pkt.push(OPT_SERVER_IDENTIFIER);
        pkt.push(4);
        pkt.extend_from_slice(&server_ip);
        pkt.push(OPT_END);
        pkt
    }

    #[test]
    fn test_build_discover_fields() {
        let chaddr = [0xD8, 0x43, 0xAE, 0x13, 0xBF, 0x7F];
        let xid = 0x12345678u32;
        let pkt = build_discover(xid, &chaddr);
        assert_eq!(pkt[0], 1, "op 应为 BOOTREQUEST=1");
        assert_eq!(pkt[1], 1, "htype 应为 Ethernet=1");
        assert_eq!(pkt[2], 6, "hlen 应为 6");
        assert_eq!(&pkt[4..8], &xid.to_be_bytes(), "xid 应与入参一致");
        assert_eq!(
            &pkt[10..12],
            &0x8000u16.to_be_bytes(),
            "flags 应为广播响应 0x8000"
        );
        assert_eq!(&pkt[28..34], &chaddr, "chaddr 应等于活动网卡 MAC");
        assert_eq!(&pkt[236..240], &DHCP_MAGIC_COOKIE, "magic cookie 应正确");
        // 断言 options 中存在 53=DISCOVER
        let mut pos = 240;
        let mut found_53 = false;
        while pos < pkt.len() {
            let code = pkt[pos];
            if code == OPT_END {
                break;
            }
            let len = pkt[pos + 1] as usize;
            if code == OPT_MESSAGE_TYPE && pkt.get(pos + 2) == Some(&MSG_TYPE_DISCOVER) {
                found_53 = true;
            }
            pos += 2 + len;
        }
        assert!(found_53, "options 应包含 option 53 且值为 DISCOVER=1");
    }

    #[test]
    fn test_new_xid_nonzero() {
        // 多次生成确保 xid 恒非零（全零事务会被服务器忽略）
        for _ in 0..64 {
            assert_ne!(new_xid(), 0, "xid 不应为 0");
        }
    }

    #[test]
    fn test_parse_offer_valid() {
        let pkt = build_test_reply(0x12345678, MSG_TYPE_OFFER, [192, 168, 1, 1]);
        let off = match parse_offer(&pkt) {
            Some(o) => o,
            None => {
                assert!(false, "合法 OFFER 应解析成功");
                return;
            }
        };
        assert_eq!(off.message_type, MSG_TYPE_OFFER, "message_type 应为 OFFER=2");
        assert_eq!(
            off.server_identifier,
            Some(Ipv4Addr::new(192, 168, 1, 1)),
            "server identifier 应正确提取"
        );
    }

    #[test]
    fn test_parse_offer_invalid() {
        // 短包（不足 242 字节）
        assert!(parse_offer(&[0u8; 10]).is_none(), "短包应返回 None");
        assert!(parse_offer(&[0u8; 241]).is_none(), "缺 options 应返回 None");
        // 坏 magic cookie
        let mut bad = build_test_reply(1, MSG_TYPE_OFFER, [10, 0, 0, 1]);
        bad[236] = 0x00;
        assert!(parse_offer(&bad).is_none(), "坏 cookie 应返回 None");
        // 非 BOOTREPLY（op != 2）
        let mut bad_op = build_test_reply(1, MSG_TYPE_OFFER, [10, 0, 0, 1]);
        bad_op[0] = 1;
        assert!(parse_offer(&bad_op).is_none(), "op=1 的请求报文应返回 None");
        // 非 OFFER（message type = NAK=6）
        let nak = build_test_reply(1, 6, [10, 0, 0, 1]);
        assert!(parse_offer(&nak).is_none(), "NAK 报文应返回 None");
    }

    #[test]
    fn test_dedup_servers() {
        let input = vec![
            "192.168.1.1".to_string(),
            "192.168.1.1".to_string(),
            "192.168.1.2".to_string(),
            " 192.168.1.3 ".to_string(),
            "".to_string(),
            "   ".to_string(),
        ];
        let out = dedup_servers(&input);
        assert_eq!(
            out,
            vec!["192.168.1.1", "192.168.1.2", "192.168.1.3"],
            "应保序去重并跳过空白项"
        );
        assert!(dedup_servers(&[]).is_empty(), "空输入返回空");
    }

    #[test]
    fn test_merge_baseline() {
        // 无重复：探测 + 基线并集
        let merged = merge_baseline(vec!["192.168.1.1".into()], Some("192.168.1.254".into()));
        assert_eq!(merged, vec!["192.168.1.1", "192.168.1.254"]);
        // 基线重复：去重
        let merged = merge_baseline(vec!["192.168.1.1".into()], Some("192.168.1.1".into()));
        assert_eq!(merged, vec!["192.168.1.1"]);
        // 无基线
        let merged = merge_baseline(vec!["192.168.1.1".into()], None);
        assert_eq!(merged, vec!["192.168.1.1"]);
        // 空基线：不污染结果
        let merged = merge_baseline(vec![], Some("".into()));
        assert!(merged.is_empty(), "空基线应被忽略");
        // 空白基线
        let merged = merge_baseline(vec![], Some("   ".into()));
        assert!(merged.is_empty(), "空白基线应被忽略");
    }

    #[test]
    fn test_build_server_check_gateway_ok() {
        // 服务器 = 网关且可达 → ok
        let local = Some(Ipv4Addr::new(192, 168, 3, 100));
        let gw = Some(Ipv4Addr::new(192, 168, 3, 254));
        let mask = Some(Ipv4Addr::new(255, 255, 255, 0));
        let c = build_server_check("192.168.3.254", local, gw, mask, true, 1.2);
        assert!(c.is_gateway);
        assert!(c.same_subnet);
        assert_eq!(c.severity, "ok");
    }

    #[test]
    fn test_build_server_check_cross_subnet_unreachable() {
        // 用户典型场景：DHCP 服务器 192.168.11.1，本机 192.168.3.100/24，网关 192.168.3.254
        let local = Some(Ipv4Addr::new(192, 168, 3, 100));
        let gw = Some(Ipv4Addr::new(192, 168, 3, 254));
        let mask = Some(Ipv4Addr::new(255, 255, 255, 0));
        let c = build_server_check("192.168.11.1", local, gw, mask, false, 0.0);
        assert!(!c.is_gateway, "192.168.11.1 ≠ 网关 192.168.3.254");
        assert!(!c.same_subnet, "11 网段 ≠ 3 网段");
        assert_eq!(c.severity, "critical");
        assert!(c.diagnosis.contains("跨网段"), "诊断应提示跨网段: {}", c.diagnosis);
    }

    #[test]
    fn test_build_server_check_same_subnet_extra() {
        // 同网段非网关：warn（多 DHCP 冲突风险）
        let local = Some(Ipv4Addr::new(192, 168, 3, 100));
        let gw = Some(Ipv4Addr::new(192, 168, 3, 254));
        let mask = Some(Ipv4Addr::new(255, 255, 255, 0));
        let c = build_server_check("192.168.3.1", local, gw, mask, true, 5.0);
        assert!(c.same_subnet);
        assert!(!c.is_gateway);
        assert_eq!(c.severity, "warn");
    }

    #[test]
    fn test_build_server_check_gateway_unreachable() {
        // 网关不可达 → critical
        let local = Some(Ipv4Addr::new(192, 168, 3, 100));
        let gw = Some(Ipv4Addr::new(192, 168, 3, 254));
        let mask = Some(Ipv4Addr::new(255, 255, 255, 0));
        let c = build_server_check("192.168.3.254", local, gw, mask, false, 0.0);
        assert!(c.is_gateway);
        assert_eq!(c.severity, "critical");
    }

    #[test]
    fn test_build_server_check_missing_config() {
        // 无本地配置（读取失败降级）：不 panic，按不可判定处理
        let c = build_server_check("192.168.1.1", None, None, None, true, 3.0);
        assert!(!c.same_subnet);
        assert!(!c.is_gateway);
        assert_eq!(c.severity, "warn");
    }

    #[test]
    fn test_build_deep_summary_cross_subnet() {
        // 用户典型场景：跨网段 DHCP 服务器（192.168.11.1）+ 网关 192.168.3.254
        let local = Some(Ipv4Addr::new(192, 168, 3, 100));
        let gw = Some(Ipv4Addr::new(192, 168, 3, 254));
        let mask = Some(Ipv4Addr::new(255, 255, 255, 0));
        let check = build_server_check("192.168.11.1", local, gw, mask, false, 0.0);
        let summary = build_deep_summary(&[check], local, gw);
        assert!(summary.contains("192.168.11.1"), "摘要应列出服务器: {}", summary);
        assert!(summary.contains("192.168.3.254"), "摘要应列出网关: {}", summary);
        assert!(summary.contains("跨网段"), "摘要应提示跨网段: {}", summary);
        assert!(summary.contains("中继"), "摘要应提示 DHCP 中继: {}", summary);
    }

    #[test]
    fn test_build_deep_summary_ok_and_empty() {
        // 全部正常
        let local = Some(Ipv4Addr::new(192, 168, 3, 100));
        let gw = Some(Ipv4Addr::new(192, 168, 3, 254));
        let mask = Some(Ipv4Addr::new(255, 255, 255, 0));
        let check = build_server_check("192.168.3.254", local, gw, mask, true, 1.0);
        let summary = build_deep_summary(&[check], local, gw);
        assert!(summary.contains("全部正常"), "{summary}");
        // 空列表
        let summary = build_deep_summary(&[], local, gw);
        assert!(summary.contains("未检测到"), "{summary}");
    }

    #[test]
    fn test_mask_u32() {
        assert_eq!(mask_u32(Ipv4Addr::new(255, 255, 255, 0)), 0xFFFF_FF00);
        assert_eq!(mask_u32(Ipv4Addr::new(192, 168, 3, 100)), 0xC0A8_0364);
    }

    #[test]
    fn test_collect_servers_timeout_returns_empty() {
        // 回环 socket + 无监听目标（discard 端口）：覆盖超时路径，不应 panic
        let socket = match UdpSocket::bind("127.0.0.1:0") {
            Ok(s) => s,
            Err(e) => {
                assert!(false, "回环 bind 失败: {}", e);
                return;
            }
        };
        let _ = socket.set_read_timeout(Some(Duration::from_millis(50)));
        let xid = 0xDEADBEEF;
        let chaddr = [0x02u8, 0, 0, 0, 0, 1];
        let pkt = build_discover(xid, &chaddr);
        let targets = vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 9))]; // discard
        let start = Instant::now();
        let (servers, sent_ok) = collect_servers(&socket, &targets, &pkt, xid, 300, 100);
        assert!(sent_ok, "发送到回环目标应成功");
        assert!(servers.is_empty(), "无响应时应返回空列表");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "收集应按窗口按时返回，不阻塞"
        );
    }

    #[test]
    fn test_collect_servers_receives_offer() {
        // 模拟 DHCP 服务器：监听回环端口，收到 DISCOVER 后回 OFFER（响应到源地址）
        let server = match UdpSocket::bind("127.0.0.1:0") {
            Ok(s) => s,
            Err(e) => {
                assert!(false, "服务器 bind 失败: {}", e);
                return;
            }
        };
        let server_addr = match server.local_addr() {
            Ok(a) => a,
            Err(e) => {
                assert!(false, "服务器 local_addr 失败: {}", e);
                return;
            }
        };
        let server_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            match server.recv_from(&mut buf) {
                Ok((len, src)) => {
                    let req = &buf[..len];
                    // 沿用请求的 xid，构造 OFFER（服务器 IP = 192.168.50.1）
                    let xid = u32::from_be_bytes([req[4], req[5], req[6], req[7]]);
                    let mut off = vec![0u8; 240];
                    off[0] = 2;
                    off[1] = 1;
                    off[2] = 6;
                    off[4..8].copy_from_slice(&xid.to_be_bytes());
                    off[28..34].copy_from_slice(&req[28..34]);
                    off[236..240].copy_from_slice(&DHCP_MAGIC_COOKIE);
                    off.push(OPT_MESSAGE_TYPE);
                    off.push(1);
                    off.push(MSG_TYPE_OFFER);
                    off.push(OPT_SERVER_IDENTIFIER);
                    off.push(4);
                    off.extend_from_slice(&[192, 168, 50, 1]);
                    off.push(OPT_END);
                    let _ = server.send_to(&off, src);
                }
                Err(_) => {}
            }
        });

        let client = match UdpSocket::bind("127.0.0.1:0") {
            Ok(s) => s,
            Err(e) => {
                assert!(false, "客户端 bind 失败: {}", e);
                return;
            }
        };
        let _ = client.set_read_timeout(Some(Duration::from_millis(50)));
        let xid = 0xCAFEBABE;
        let chaddr = [0x02u8, 0x11, 0x22, 0x33, 0x44, 0x55];
        let pkt = build_discover(xid, &chaddr);
        let targets = vec![server_addr];
        let (servers, sent_ok) = collect_servers(&client, &targets, &pkt, xid, 1000, 600);
        let _ = server_thread.join();
        assert!(sent_ok, "发送到模拟服务器应成功");
        assert_eq!(
            servers,
            vec!["192.168.50.1".to_string()],
            "应解析出 OFFER 的 server identifier"
        );
    }
}
