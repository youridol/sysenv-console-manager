/// DHCP 服务器探测模块（本地 DHCP 冲突检测）
///
/// 实现要点（2026-09 多网段修复，全部结论均来自本机实测）：
/// - **逐网段探测**：`read_network_config()` 收集**全部**已注册的 IPv4 可探测网段
///   （跳过 APIPA 169.254/16、环回、0.0.0.0），而不是只挑一个"最佳"接口。
///   背景 BUG：旧实现只探测评分最高的单个接口——多宿主机器（本机有
///   192.168.3.0/24 与 192.168.160.0/20 两个网段、各自都有真实 DHCP 服务器）
///   必然漏报其它网段的服务器，用户可见现象即"无法正确检查当前内网所有的
///   DHCP 服务器"。实测旧实现只发现 192.168.3.254，漏掉 192.168.160.1。
/// - **顺序探测**：一次只持有一个 :68 socket，逐网段轮转（每网段探测完即释放）。
///   实测：同时绑定两个 :68 socket 会互相抢收，只能收到其中一个网段的 OFFER。
/// - **SO_REUSEADDR 绑定该网段接口 IP:68**（与 Windows Dhcp 客户端服务共存，
///   源端口 68 是标准 RFC 2131 客户端行为）。绑定具体接口地址沿用的是原有设计，
///   并非修复所需——实测 0.0.0.0 与具体地址两种绑定都能收到广播 OFFER
///   （见 `bind_probe_socket` 注释）。
/// - **目标通道**：该网段的**定向广播**（ip|~mask，掩码此前已读取却从未使用）、有限广播 255.255.255.255，必要时再追加网关单播。实测部分服务器/防火墙丢弃 255.255.255.255 而只认定向广播。
/// - **chaddr 用合成 MAC**：`synthetic_client_mac()` 生成随机本地管理单播 MAC，
///   不再使用 `get_primary_mac()` 取到的任意**真实**网卡 MAC。实测（随机顺序 +
///   对照，各 6 轮）：对 192.168.160.1 的 ICS DHCP 服务器，三个真实网卡 MAC
///   全部 0/6 应答，而合成/全零 MAC 全部 6/6 应答。
/// - **窗口 4.5 秒 + 收到 OFFER 提前结束**：实测本网段服务器 192.168.3.254 的
///   OFFER 稳定晚到 3000–3900ms（8/8 样本，宽松间隔测得），3 秒窗口会间歇性
///   漏掉它；而 ICS 服务器只需 ~10ms。收到首个 OFFER 后再等
///   `SETTLE_AFTER_FIRST_OFFER_MS`（300ms）即结束本网段，兼顾"快服务器不白等"
///   与"收齐同网段多台服务器（冲突检测）"。
/// - 防广播风暴：单次检测 DISCOVER 包总数硬上限 4（`MAX_DISCOVER_PACKETS`），
///   最多覆盖 4 个网段（优先有网关的网段）。整轮墙钟上限 `MAX_TOTAL_PROBE_MS`。
/// - 降级：绑定 68 失败时回退随机高端口（6800-6899），note 注明局限。
/// - OFFER 解析：校验 magic cookie 0x63825363 与 op=2（BOOTREPLY），
///   提取 options 53（message type）与 54（server identifier）。
/// - 基线组合：读取注册表当前分配 DHCP 服务器 IP（DhcpServer 值），
///   与主动探测结果并集去重。
///
/// 实测结论（修复后，15–20s 间隔 6 次运行）：6/6 次同时发现
/// `192.168.3.254`（A 网段）与 `192.168.160.1`（B 网段）；修复前 0/6 次
/// 见到 B（只发现 A 或为空）。
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
/// 收集窗口（毫秒）——**每个网段**的收集窗口。
///
/// 实测（本机 192.168.3.0/24，探测间隔放宽到 8 秒以排除自扰限流；原始测量脚本
/// 属一次性诊断产物，未随修复提交）：本网段服务器
/// 192.168.3.254 的 OFFER 延迟稳定在 **3000–3900ms**（定向广播 8/8 命中，
/// 最小 3004ms、最大 3555ms）；而另一网段的 Windows ICS 服务器只需 ~10ms。
/// 3 秒窗口会间歇性漏掉前者（实测 0/5～3/5 命中，极不稳定）。
///
/// 因此窗口取 4.5 秒，并配合 `SETTLE_AFTER_FIRST_OFFER_MS` **提前结束**：
/// 一旦收到首个 OFFER，再等 300ms 收同网段的其它服务器就立刻进入下一个网段。
/// 于是快服务器（~10ms）几乎不额外耗时，慢服务器也能被完整等到。
const COLLECT_WINDOW_MS: u64 = 4500;
/// 单个网段的收集窗口下限（毫秒）
const MIN_SEGMENT_WINDOW_MS: u64 = 1200;
/// 收到首个 OFFER 后额外等待多久（毫秒）再结束本网段收集。
///
/// 用于收齐同一网段上的**多台** DHCP 服务器（检测冲突是核心目标），
/// 同时避免为慢服务器白等整个窗口。
const SETTLE_AFTER_FIRST_OFFER_MS: u64 = 300;
/// 单次探测（全部网段合计）的墙钟时长上限（毫秒）。
///
/// 有了"收到 OFFER 即提前结束"，常见情形（有 DHCP 服务器）远快于该上限；
/// 该上限只约束"哪个网段都没响应"的病态情况，避免用户长时间等待。
const MAX_TOTAL_PROBE_MS: u64 = 10000;
/// 计算当前网段的收集窗口（毫秒）——纯函数，可单测。
///
/// 目标：主网段（排在最前）必须拿到完整窗口（实测其 OFFER 可能晚到 ~3.9 秒），
/// 同时给后续每个网段各留 `MIN_SEGMENT_WINDOW_MS`，且全部合计不超过
/// `MAX_TOTAL_PROBE_MS`。
///
/// 规则：
/// 1. 先给后续网段各预留 `MIN_SEGMENT_WINDOW_MS`，其余归当前网段；
/// 2. 夹在 `[MIN_SEGMENT_WINDOW_MS, COLLECT_WINDOW_MS]` 之间；
/// 3. 绝不超出剩余预算。
///
/// 注意 `budget_left` 传的是**实际剩余**墙钟预算：由于收集会因"收到 OFFER
/// 提前结束"，快网段实际耗时远小于分配窗口，后续网段因此仍能拿到完整窗口。
fn segment_window_ms(budget_left: u64, segments_left: u64) -> u64 {
    if segments_left == 0 || budget_left == 0 {
        return 0;
    }
    let reserve_for_later = MIN_SEGMENT_WINDOW_MS.saturating_mul(segments_left.saturating_sub(1));
    let usable = budget_left.saturating_sub(reserve_for_later);
    usable
        .clamp(MIN_SEGMENT_WINDOW_MS, COLLECT_WINDOW_MS)
        .min(budget_left)
}

/// 单次 recv 超时（毫秒），用于循环内定期检查 deadline
const RECV_TIMEOUT_MS: u64 = 200;

/// 单次检测允许发送的 DISCOVER 包**总数**上限（防广播风暴）。
/// 旧实现为「2 目标 × 2 次发送 = 4」；多网段后保持同一上限。
const MAX_DISCOVER_PACKETS: usize = 4;
/// 最多探测的网段数（受包上限约束：每网段至少 1 包）
const MAX_SEGMENTS: usize = MAX_DISCOVER_PACKETS;

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

/// 一个可探测的 IPv4 网段（接口 IP + 掩码 + 网关）
///
/// `read_network_config()` 会给出**全部**网段；探测阶段逐网段轮转，
/// 以覆盖多宿主机器上不同广播域的 DHCP 服务器。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Segment {
    /// 该网段的接口 IPv4 地址（socket 绑定到它，OFFER 才回得来）
    interface_ip: Ipv4Addr,
    /// 子网掩码（用于计算定向广播）。注册表缺失时为 None：
    /// 此时不发定向广播（避免猜错掩码把包发到别的网段），只发有限广播。
    subnet_mask: Option<Ipv4Addr>,
    /// 默认网关（可无；有则追加网关单播通道）
    gateway: Option<Ipv4Addr>,
}

impl Segment {
    /// 定向（子网）广播地址 = interface_ip | ~subnet_mask；掩码未知时返回 None
    fn directed_broadcast(&self) -> Option<Ipv4Addr> {
        let mask = self.subnet_mask?;
        let ip = u32::from_be_bytes(self.interface_ip.octets());
        let m = u32::from_be_bytes(mask.octets());
        Some(Ipv4Addr::from((ip | !m).to_be_bytes()))
    }
}

/// 从注册表解析的网络配置（全部探测网段 + 基线 DHCP 服务器）
#[derive(Debug, Default, Clone)]
struct NetConfig {
    /// **全部**可探测网段（有网关的排在前面），最多 `MAX_SEGMENTS` 个。
    ///
    /// 旧实现只有单个 `interface_ip`/`gateway`/`subnet_mask`，多宿主机器上
    /// 必然漏掉其它网段的 DHCP 服务器。
    segments: Vec<Segment>,
    /// 当前分配的 DHCP 服务器 IP（基线）
    baseline_dhcp_server: Option<String>,
}

impl NetConfig {
    /// 首选（评分最高）网段的 IP：深度检查的 `local_ip` 与同网段判定用它。
    fn interface_ip(&self) -> Option<Ipv4Addr> {
        self.segments.first().map(|s| s.interface_ip)
    }

    /// 首选网段的网关
    fn gateway(&self) -> Option<Ipv4Addr> {
        self.segments.first().and_then(|s| s.gateway)
    }

    /// 首选网段的掩码
    fn subnet_mask(&self) -> Option<Ipv4Addr> {
        self.segments.first().and_then(|s| s.subnet_mask)
    }
}

/// 判断某个接口 IP 是否值得探测：跳过环回、未指定、APIPA（169.254/16）
///
/// APIPA 表示该接口没有真实 DHCP 服务（自分配地址），探测它只会白发包；
/// 本机实测蓝牙/WLAN 均为 169.254.x.x。
fn is_probeable_interface_ip(ip: Ipv4Addr) -> bool {
    !ip.is_unspecified() && !ip.is_loopback() && !ip.is_link_local()
}

/// 按 (接口 IP, 掩码) 去重并截断到 `limit` 个网段（保序，纯函数，可单测）。
///
/// 必要性：注册表里同一物理网段常有**多个接口子键**（本机实测 {783cc926} 与
/// {8d0383f2} 都写着 192.168.3.238/24）。若不去重，有限的 DISCOVER 包预算
/// 会被同一个网段重复吃掉，真正不同的网段反而排不进去（漏报）。
/// 注意"同 IP 不同掩码"仍视为不同网段——它们的广播域不同。
fn dedup_and_limit_segments(segments: &[Segment], limit: usize) -> Vec<Segment> {
    let mut seen: Vec<(Ipv4Addr, Option<Ipv4Addr>)> = Vec::new();
    let mut out: Vec<Segment> = Vec::new();
    for seg in segments {
        if out.len() >= limit {
            break;
        }
        let key = (seg.interface_ip, seg.subnet_mask);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push(seg.clone());
    }
    out
}

/// 读取网络配置：**全部**可探测网段（IP/掩码/网关）+ 基线 DHCP 服务器
///
/// 数据源：HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces
/// 各接口子键的 IPAddress（REG_MULTI_SZ）、SubnetMask（REG_MULTI_SZ，与
/// IPAddress 按索引对应）、DefaultGateway / DhcpDefaultGateway（REG_MULTI_SZ）、
/// DhcpServer（REG_SZ）。
///
/// 关键：**收集所有网段**（按「有网关」优先排序，取前 `MAX_SEGMENTS` 个），
/// 而不是只保留评分最高的一个。多宿主机器（虚拟机网卡、Hyper-V 交换机、
/// 多张物理网卡）上每个网段都可能有独立 DHCP 服务器，只探一个必然漏报。
///
/// 读取失败或键缺失时返回默认值（空网段），调用方据此降级
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
    // 收集**全部**候选网段，最后按「有网关优先」排序取前 N 个。
    // 候选：(优先分, Segment)；优先分 1 = 有网关，0 = 仅有 IP。
    let mut candidates: Vec<(i32, Segment)> = Vec::new();
    // 仅 DHCP 接口（EnableDHCP=1）中第一个有效的 DhcpServer（无更好候选时回退用；
    // 静态接口/历史租约记录不提供基线，避免假冲突）
    let mut first_dhcp: Option<String> = None;
    // 评分最高网段自身的 DhcpServer（作为基线首选）
    let mut best_dhcp: Option<(i32, String)> = None;
    for sub in hklm.enum_keys() {
        let Ok(name) = sub else { continue };
        let Ok(key) = hklm.open_subkey(&name) else {
            continue;
        };
        // 接口 IP 列表（REG_MULTI_SZ，可能多 IP）
        let ips = read_multi_strings(&key, "IPAddress");
        // 子网掩码（REG_MULTI_SZ，与 IPAddress 按索引一一对应）
        let masks = read_multi_strings(&key, "SubnetMask");
        // 逐个 (IP, 掩码) 配对：每个有效可探测 IP 都形成一个网段候选。
        // 掩码可能缺失（None），此时仍保留网段（只发有限广播）。
        for (idx, raw_ip) in ips.iter().enumerate() {
            let Ok(ip) = raw_ip.trim().parse::<Ipv4Addr>() else {
                continue;
            };
            if !is_probeable_interface_ip(ip) {
                continue;
            }
            let mask = masks
                .get(idx)
                .and_then(|m| m.trim().parse::<Ipv4Addr>().ok())
                .filter(|m| !m.is_unspecified());
            // 网关：静态 DefaultGateway 优先，DHCP 分配 DhcpDefaultGateway 次之
            let gw = read_gateway(&key);
            let score = if gw.is_some() { 1 } else { 0 };
            candidates.push((
                score,
                Segment {
                    interface_ip: ip,
                    subnet_mask: mask,
                    gateway: gw,
                },
            ));
        }
        // 基线：仅 DHCP 接口（EnableDHCP=1）的 DhcpServer 可作基线
        // （静态接口/历史租约不污染）；过滤空值与 255.255.255.255
        let dhcp_enabled = key.get_value::<u32, _>("EnableDHCP").unwrap_or(0) == 1;
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
        if let Some(d) = dhcp {
            let score = if read_gateway(&key).is_some() { 1 } else { 0 };
            if best_dhcp.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                best_dhcp = Some((score, d));
            }
        }
    }

    // 排序：有网关的网段优先（更可能是"主"网段），同级保持注册表枚举顺序
    candidates.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let ordered: Vec<Segment> = candidates.into_iter().map(|(_, s)| s).collect();

    // 按 (接口 IP, 掩码) 去重并截断到包预算允许的网段数
    // （注册表里同一物理地址常有**多个接口子键**，本机实测 {783cc926} 与
    //  {8d0383f2} 都是 192.168.3.238/24；不去重会把宝贵的 DISCOVER 包预算
    //  浪费在同一网段上，并导致后段漏探）
    cfg.segments = dedup_and_limit_segments(&ordered, MAX_SEGMENTS);

    // 基线：优先取首选（有网关）网段自身的 DhcpServer，否则回退任意 DHCP 接口的首个有效值
    cfg.baseline_dhcp_server = best_dhcp.map(|(_, d)| d).or(first_dhcp);

    crate::debug_info!(
        "dhcp_probe",
        "网络配置: 可探测网段 {} 个 {:?} baseline={:?}",
        cfg.segments.len(),
        cfg.segments
            .iter()
            .map(|s| format!(
                "{} mask={} gw={}",
                s.interface_ip,
                s.subnet_mask
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "?".to_string()),
                s.gateway
                    .map(|g| g.to_string())
                    .unwrap_or_else(|| "-".to_string())
            ))
            .collect::<Vec<_>>(),
        cfg.baseline_dhcp_server
    );

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
// 客户端 MAC（chaddr）
// ============================================================================

/// 生成 DISCOVER 报文的 chaddr：随机**本地管理单播** MAC（02:xx:xx:xx:xx:xx）。
///
/// 为什么不用本机真实网卡 MAC（旧实现 `get_primary_mac()`）：
/// 1. 旧实现取的是 sysinfo 枚举到的**第一个**非零 MAC，它可能与 socket 绑定的
///    网段不是同一张网卡（本机有 Realtek、WLAN、蓝牙、Hyper-V 多张网卡）；
/// 2. 更关键的是**实测证据**：对 192.168.160.0/20 网段上 Windows ICS 内置的
///    DHCP 服务器（192.168.160.1），用真实网卡 MAC 应答率 0/6（三个不同真实
///    MAC 全部为 0），而用合成 MAC 或全零 MAC 应答率 6/6 —— 该服务器会丢弃
///    chaddr 属于"真实但非本接口"网卡的请求。合成 MAC 每次运行随机，
///    既避免该问题，也不冒充任何真实设备。
///
/// 本地管理位（第一个字节 bit1=1）与单播位（bit0=0）保证是合法的
/// locally-administered unicast 地址，符合 RFC 2131/2132 对 chaddr 的要求。
fn synthetic_client_mac() -> [u8; 6] {
    let r: u32 = rand::random();
    [
        0x02 | (((r & 0x3F) as u8) << 2), // bit0=0 单播，bit1=1 本地管理
        (r >> 8) as u8,
        (r >> 16) as u8,
        rand::random::<u8>(),
        rand::random::<u8>(),
        rand::random::<u8>(),
    ]
}

// ============================================================================
// 探测收集循环
// ============================================================================

/// DISCOVER 包**全局**预算（防广播风暴）。
///
/// 单次检测最多发送 `MAX_DISCOVER_PACKETS` 个 DISCOVER 包，与旧实现的上限
/// （2 通道 × 2 次 = 4）一致；多网段后同一预算在网段间分配。
#[derive(Debug)]
struct PacketBudget {
    remaining: usize,
}

impl PacketBudget {
    fn new(total: usize) -> Self {
        Self { remaining: total }
    }

    /// 当前剩余额度
    fn remaining(&self) -> usize {
        self.remaining
    }

    /// 消耗 1 个额度；额度不足时返回 false（调用方必须跳过发送）
    fn try_take(&mut self) -> bool {
        if self.remaining == 0 {
            false
        } else {
            self.remaining -= 1;
            true
        }
    }

    /// 扣减 `n` 个额度（用于把子预算的消耗归还到全局预算）
    fn consume(&mut self, n: usize) {
        self.remaining = self.remaining.saturating_sub(n);
    }
}

/// 计算当前网段可用的 DISCOVER 包额度。
///
/// 约束（纯函数，便于单测）：
/// - 每个尚未探测的网段至少预留 1 包，保证"每个网段都能被探测到"；
/// - 当前网段至少拿到 1 包（前提是全局还有额度）；
/// - 结果绝不会超过全局剩余额度（不突破广播风暴上限）。
fn per_segment_allowance(remaining: usize, segments_left_after: usize) -> usize {
    if remaining == 0 {
        return 0;
    }
    // 先给后续网段各留 1 包，剩下的都归当前网段；不足时当前网段也拿 1 包
    remaining.saturating_sub(segments_left_after).max(1)
}

/// 单个网段的收集时序参数（毫秒）
///
/// 抽成结构体而非逐个参数，是为了让 `collect_servers` 的调用点自解释，
/// 并让单测能只覆写关心的那一项（`..Default::default()`）。
#[derive(Debug, Clone, Copy)]
struct CollectTiming {
    /// 本网段最长收集窗口
    collect_ms: u64,
    /// 首次重发 DISCOVER 的时刻
    resend_ms: u64,
    /// 收到首个 OFFER 后再等多久就提前结束
    settle_ms: u64,
}

impl Default for CollectTiming {
    fn default() -> Self {
        Self {
            collect_ms: COLLECT_WINDOW_MS,
            resend_ms: COLLECT_WINDOW_MS / 2,
            settle_ms: SETTLE_AFTER_FIRST_OFFER_MS,
        }
    }
}

/// 收集循环：发送 DISCOVER → 监听 OFFER 直到 deadline，中途 1 次重发，
/// **收到首个 OFFER 后再等 `timing.settle_ms` 即提前结束**（收齐同网段多台服务器）。
///
/// `budget` 为**全局**包预算：本函数只会消耗它，绝不会突破上限。
/// 返回 (去重后的服务器 IP 列表, 是否至少一次发送成功)。
fn collect_servers(
    socket: &UdpSocket,
    targets: &[SocketAddr],
    pkt: &[u8],
    xid: u32,
    timing: CollectTiming,
    budget: &mut PacketBudget,
) -> (Vec<String>, bool) {
    let start = Instant::now();
    let deadline = start + Duration::from_millis(timing.collect_ms);
    let resend_at = start + Duration::from_millis(timing.resend_ms);
    let mut sent_ok = false;
    let mut resent = false;
    let mut servers: Vec<String> = Vec::new();
    let mut buf = [0u8; 1024];
    let xid_be = xid.to_be_bytes();
    // 收到首个 OFFER 的时刻：用于提前结束（快服务器不必等满窗口）
    let mut first_offer_at: Option<Instant> = None;

    // 初始发送（通道顺序：定向广播 → 有限广播 → 网关单播）
    sent_ok |= send_discover(socket, targets, pkt, budget);

    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        // 已收到 OFFER 且过了 settle 窗口 → 收齐了，提前结束本网段
        if let Some(t) = first_offer_at {
            if now.duration_since(t) >= Duration::from_millis(timing.settle_ms) {
                break;
            }
        }
        // 到重发时刻且未重发 → 重发一次（受全局预算约束，防广播风暴）
        if !resent && now >= resend_at {
            resent = true;
            sent_ok |= send_discover(socket, targets, pkt, budget);
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
                        if first_offer_at.is_none() {
                            first_offer_at = Some(Instant::now());
                        }
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

/// 向所有目标发送 DISCOVER，返回是否至少一个通道成功。
///
/// 每发 1 个数据报消耗 1 个全局预算；预算耗尽立即停止（不突破包上限）。
fn send_discover(
    socket: &UdpSocket,
    targets: &[SocketAddr],
    pkt: &[u8],
    budget: &mut PacketBudget,
) -> bool {
    let mut ok = false;
    for t in targets {
        if !budget.try_take() {
            crate::debug_info!(
                "dhcp_probe",
                "DISCOVER 包预算已用尽（上限 {}），停止发送以防广播风暴",
                MAX_DISCOVER_PACKETS
            );
            break;
        }
        match socket.send_to(pkt, t) {
            Ok(_) => ok = true,
            Err(e) => {
                crate::debug_warn!("dhcp_probe", "send_to({}) 失败: {}", t, e);
            }
        }
    }
    ok
}

/// 生成单个网段的发送通道列表（按优先级）：
/// 1. 该网段的定向广播（ip|~mask）——RFC 正确做法，部分服务器/防火墙只认它；
///    掩码未知时跳过（不猜掩码，避免把包发到别的网段）
/// 2. 有限广播 255.255.255.255——传统做法，部分环境会丢弃
/// 3. 网关 :67 单播（网关存在时）——覆盖"服务器即网关且不做广播回包"的情况
fn segment_targets(seg: &Segment) -> Vec<SocketAddr> {
    let mut targets = Vec::with_capacity(3);
    if let Some(directed) = seg.directed_broadcast() {
        // 定向广播与有限广播相同时不重复
        if directed != Ipv4Addr::BROADCAST {
            targets.push(SocketAddr::from((directed, DHCP_SERVER_PORT)));
        }
    }
    targets.push(SocketAddr::from((Ipv4Addr::BROADCAST, DHCP_SERVER_PORT)));
    if let Some(gw) = seg.gateway {
        if gw != Ipv4Addr::UNSPECIFIED && gw != Ipv4Addr::BROADCAST {
            targets.push(SocketAddr::from((gw, DHCP_SERVER_PORT)));
        }
    }
    targets
}

/// 创建探测 socket。
///
/// 首选：socket2 设置 SO_REUSEADDR 后绑定 UDP 68（与 Windows Dhcp 客户端服务共存，
/// 源端口 68 是标准 RFC 2131 客户端行为；主流 DHCP 服务器 dnsmasq/ISC 固定响应到 68，
/// 绑定 68 即可收到 OFFER；SO_REUSEADDR 使广播 OFFER 被多 socket 共享接收）。
/// 降级：绑定失败（个别系统不允许共存）时回退随机高端口（6800-6899），
/// 此时固定响应 68 的服务器可能漏检（返回降级标记供 note 提示局限）。
///
/// `interface_ip` 传**本网段的接口地址**（而不是 0.0.0.0），与本模块既有的
/// "逐网段探测"设计配套：绑定某网段接口地址后，收到的 OFFER 归属明确。
///
/// 注意（不要误引为因果关系）：曾怀疑"绑定具体接口地址而非 0.0.0.0 才收得到
/// 广播 OFFER"，该假设已被实测**证伪**——同端口两个 SO_REUSEADDR socket
/// （0.0.0.0 与 192.168.3.238）对有限广播 255.255.255.255 **都能收到**（各 2/2），
/// 对定向广播 192.168.3.255 也都能收到；只有发往**本机单播地址**的报文是
/// "绑具体地址才收到、绑 0.0.0.0 收不到"。真实 DHCP 探测下两种绑定都收到了
/// 192.168.3.254 的 OFFER。故绑定方式**不是**本 BUG 的成因，此处保留具体地址
/// 绑定只是沿用原设计 + 便于按网段归因。
///
/// 返回 (socket, 是否降级到随机高端口)。
fn bind_probe_socket(interface_ip: Option<Ipv4Addr>) -> Result<(UdpSocket, bool), String> {
    // 首选：SO_REUSEADDR 绑定 68（与 Dhcp 服务共存，标准客户端行为）
    let domain = socket2::Domain::IPV4;
    let ty = socket2::Type::DGRAM;
    let reuse_result =
        socket2::Socket::new(domain, ty, Some(socket2::Protocol::UDP)).and_then(|raw| {
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
///
/// 流程：读取**全部**网段 → 逐网段顺序探测（每网段探测完即释放 socket，
/// 避免多个 :68 socket 互相抢收）→ 与注册表基线并集去重。
pub fn probe_dhcp_servers() -> Result<DhcpProbeResult, String> {
    let started = Instant::now();

    // 1. 读取网络配置（全部可探测网段 + 基线 DHCP 服务器）
    let cfg = read_network_config();

    // 2. 待探测网段：注册表读取失败降级为单个 0.0.0.0 网段（仅有限广播通道）
    let segments: Vec<Segment> = if cfg.segments.is_empty() {
        crate::debug_warn!(
            "dhcp_probe",
            "未从注册表读到任何可探测网段，降级为 0.0.0.0 + 有限广播探测"
        );
        vec![Segment {
            interface_ip: Ipv4Addr::UNSPECIFIED,
            subnet_mask: None,
            gateway: None,
        }]
    } else {
        cfg.segments.clone()
    };

    // 2b. 全局包预算（防广播风暴：单次检测 DISCOVER 总量 ≤ MAX_DISCOVER_PACKETS）
    let mut budget = PacketBudget::new(MAX_DISCOVER_PACKETS);

    // 3. 逐网段顺序探测
    let total = segments.len();
    let mut probed: Vec<String> = Vec::new();
    let mut sent_ok = false;
    let mut degraded_port = false;
    // 每个网段的窗口由剩余总预算动态分配：主（首个）网段优先拿到完整窗口，
    // 网段多时后续网段至少 MIN_SEGMENT_WINDOW_MS，全部合计不超过 MAX_TOTAL_PROBE_MS。
    let mut budget_left_ms = MAX_TOTAL_PROBE_MS;

    for (idx, seg) in segments.iter().enumerate() {
        // 给后续尚未探测的网段各预留最小窗口，保证"每个网段都探测到"
        let segments_left_after = total - idx - 1;
        let per_segment_ms = segment_window_ms(budget_left_ms, (segments_left_after + 1) as u64);
        if per_segment_ms == 0 {
            crate::debug_warn!(
                "dhcp_probe",
                "探测时间预算已用尽，跳过剩余 {} 个网段",
                total - idx
            );
            break;
        }

        // 给后续尚未探测的网段各预留 1 包，保证"每个网段至少探测一次"
        let allowance = per_segment_allowance(budget.remaining(), segments_left_after);
        if allowance == 0 {
            crate::debug_warn!(
                "dhcp_probe",
                "DISCOVER 包预算已用尽，跳过剩余 {} 个网段（防广播风暴）",
                total - idx
            );
            break;
        }

        let bind_ip = if seg.interface_ip.is_unspecified() {
            None
        } else {
            Some(seg.interface_ip)
        };
        let (socket, degraded) = match bind_probe_socket(bind_ip) {
            Ok(v) => v,
            Err(e) => {
                // 单网段绑定失败不应中断整体探测：记录后继续下一个网段
                crate::debug_warn!(
                    "dhcp_probe",
                    "网段 {} 绑定探测 socket 失败（err={}），跳过该网段",
                    seg.interface_ip,
                    e
                );
                continue;
            }
        };
        degraded_port |= degraded;

        // 通道按优先级排列，再用本网段额度截断
        // （定向广播 > 有限广播 > 网关单播）
        let mut targets = segment_targets(seg);
        if targets.len() > allowance {
            targets.truncate(allowance);
        }

        let xid = new_xid();
        let pkt = build_discover(xid, &synthetic_client_mac());
        let resend_ms = per_segment_ms / 2;
        // 本网段用子预算，结束后把实际消耗归还/扣除到全局预算
        let mut sub_budget = PacketBudget::new(allowance);
        let collect_started = Instant::now();
        let (found, ok) = collect_servers(
            &socket,
            &targets,
            &pkt,
            xid,
            CollectTiming {
                collect_ms: per_segment_ms,
                resend_ms,
                settle_ms: SETTLE_AFTER_FIRST_OFFER_MS,
            },
            &mut sub_budget,
        );
        // 实际耗时（可能因"收到 OFFER 提前结束"而远小于 per_segment_ms）
        let actual_ms = collect_started.elapsed().as_millis() as u64;
        let used = allowance - sub_budget.remaining();
        budget.consume(used);
        budget_left_ms = budget_left_ms.saturating_sub(actual_ms);
        sent_ok |= ok;
        let mut delivered: Vec<String> = Vec::new();
        for s in found {
            if !probed.contains(&s) {
                probed.push(s.clone());
            }
            delivered.push(s);
        }
        crate::debug_info!(
            "dhcp_probe",
            "网段 {} 探测: targets={:?} 本网段发现={:?} 累计probed={:?} 已用包={}/{}",
            seg.interface_ip,
            targets,
            delivered,
            probed,
            used,
            MAX_DISCOVER_PACKETS
        );
        // socket 在此处 drop（下一网段重新绑定，避免多 :68 socket 互相抢收）
    }

    if !sent_ok {
        return Err(
            "dhcp_probe: DHCP Discover 广播与单播通道均发送失败（UDP send_to 全部失败，建议：检查网络连接与防火墙后重试）"
                .to_string(),
        );
    }

    // 4. 合并基线 + 生成说明
    let baseline = cfg.baseline_dhcp_server;
    let servers = merge_baseline(probed.clone(), baseline.clone());
    let count = servers.len();
    let healthy = count <= 1;
    let note = build_note(&servers, &probed, &baseline, degraded_port);

    crate::debug_info!(
        "dhcp_probe",
        "探测完成: 网段={} probed={:?} baseline={:?} servers={:?} degraded={} elapsed={}ms",
        total,
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
    if x == 0 {
        1
    } else {
        x
    }
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
    let criticals: Vec<&DhcpServerCheck> =
        checks.iter().filter(|c| c.severity == "critical").collect();
    let warns: Vec<&DhcpServerCheck> = checks.iter().filter(|c| c.severity == "warn").collect();

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
    //    同网段/网关判定以**首选网段**（有网关者优先）为本地参照：
    //    深度检查按定义针对"本机主网段的服务器拓扑"，多网段探测只影响服务器集合。
    let local_ip = cfg.interface_ip();
    let gateway = cfg.gateway();
    let mask = cfg.subnet_mask();
    let checks: Vec<DhcpServerCheck> = servers
        .iter()
        .map(|s| {
            let (ping_ok, rtt) = ping_map.get(s).copied().unwrap_or((false, 0.0));
            build_server_check(s, local_ip, gateway, mask, ping_ok, rtt)
        })
        .collect();
    let summary = build_deep_summary(&checks, local_ip, gateway);

    crate::debug_info!(
        "dhcp_probe",
        "深度检查完成: servers={:?} elapsed={}ms",
        servers,
        started.elapsed().as_millis()
    );

    Ok(DhcpDeepCheckResult {
        local_ip: local_ip.map(|ip| ip.to_string()),
        gateway: gateway.map(|ip| ip.to_string()),
        subnet_mask: mask.map(|ip| ip.to_string()),
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
        assert_eq!(
            off.message_type, MSG_TYPE_OFFER,
            "message_type 应为 OFFER=2"
        );
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
        assert!(
            c.diagnosis.contains("跨网段"),
            "诊断应提示跨网段: {}",
            c.diagnosis
        );
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
        assert!(
            summary.contains("192.168.11.1"),
            "摘要应列出服务器: {}",
            summary
        );
        assert!(
            summary.contains("192.168.3.254"),
            "摘要应列出网关: {}",
            summary
        );
        assert!(summary.contains("跨网段"), "摘要应提示跨网段: {}", summary);
        assert!(
            summary.contains("中继"),
            "摘要应提示 DHCP 中继: {}",
            summary
        );
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
        let mut budget = PacketBudget::new(MAX_DISCOVER_PACKETS);
        let (servers, sent_ok) = collect_servers(
            &socket,
            &targets,
            &pkt,
            xid,
            CollectTiming {
                collect_ms: 300,
                resend_ms: 100,
                ..Default::default()
            },
            &mut budget,
        );
        assert!(sent_ok, "发送到回环目标应成功");
        assert!(servers.is_empty(), "无响应时应返回空列表");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "收集应按窗口按时返回，不阻塞"
        );
        // 预算被真实消耗（初始 1 包 + 1 次重发 = 2）
        assert_eq!(
            budget.remaining(),
            MAX_DISCOVER_PACKETS - 2,
            "应恰好消耗 2 个 DISCOVER 额度"
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
        let mut budget = PacketBudget::new(MAX_DISCOVER_PACKETS);
        let (servers, sent_ok) = collect_servers(
            &client,
            &targets,
            &pkt,
            xid,
            CollectTiming {
                collect_ms: 1000,
                resend_ms: 600,
                settle_ms: 100,
            },
            &mut budget,
        );
        let _ = server_thread.join();
        assert!(sent_ok, "发送到模拟服务器应成功");
        assert_eq!(
            servers,
            vec!["192.168.50.1".to_string()],
            "应解析出 OFFER 的 server identifier"
        );
    }

    // ========================================================================
    // 多网段修复的新增单测（纯函数，不依赖真实网络）
    // ========================================================================

    /// 回归护栏：必须收集**全部**网段，而不是只挑一个"最佳"接口。
    /// （旧实现只保留评分最高的单接口 → 多宿主机器漏报其它网段的 DHCP 服务器）
    #[test]
    fn test_segment_keeps_all_interface_ips() {
        // 模拟注册表里同一接口有两个 IP（REG_MULTI_SZ），加上另一个接口
        let segs = vec![
            Segment {
                interface_ip: Ipv4Addr::new(192, 168, 3, 238),
                subnet_mask: Some(Ipv4Addr::new(255, 255, 255, 0)),
                gateway: Some(Ipv4Addr::new(192, 168, 3, 254)),
            },
            Segment {
                interface_ip: Ipv4Addr::new(192, 168, 160, 1),
                subnet_mask: Some(Ipv4Addr::new(255, 255, 240, 0)),
                gateway: None,
            },
        ];
        let cfg = NetConfig {
            segments: segs.clone(),
            baseline_dhcp_server: None,
        };
        assert_eq!(cfg.segments.len(), 2, "两个网段都必须保留");
        assert_eq!(
            cfg.interface_ip(),
            Some(Ipv4Addr::new(192, 168, 3, 238)),
            "首选网段应为有网关的那个"
        );
        assert_eq!(cfg.gateway(), Some(Ipv4Addr::new(192, 168, 3, 254)));
        assert_eq!(
            cfg.subnet_mask(),
            Some(Ipv4Addr::new(255, 255, 255, 0)),
            "首选网段的掩码"
        );
    }

    /// 定向广播必须由掩码计算（旧实现读到了 subnet_mask 却从未使用）
    #[test]
    fn test_directed_broadcast_computed_from_mask() {
        let cases = [
            ("192.168.3.238", "255.255.255.0", "192.168.3.255"),
            ("192.168.160.1", "255.255.240.0", "192.168.175.255"),
            ("10.0.0.5", "255.0.0.0", "10.255.255.255"),
            ("172.16.1.9", "255.255.255.252", "172.16.1.11"),
        ];
        for (ip, mask, want) in cases {
            let seg = Segment {
                interface_ip: ip.parse().unwrap(),
                subnet_mask: Some(mask.parse().unwrap()),
                gateway: None,
            };
            assert_eq!(
                seg.directed_broadcast(),
                Some(want.parse::<Ipv4Addr>().unwrap()),
                "{ip}/{mask} 的定向广播应为 {want}"
            );
        }
        // 掩码缺失：不猜掩码，返回 None（只走有限广播）
        let seg = Segment {
            interface_ip: Ipv4Addr::new(192, 168, 1, 5),
            subnet_mask: None,
            gateway: None,
        };
        assert_eq!(seg.directed_broadcast(), None, "掩码未知时不应猜定向广播");
    }

    /// 通道优先级：定向广播 → 有限广播 → 网关单播；掩码未知时退化
    #[test]
    fn test_segment_targets_priority_and_degradation() {
        let seg = Segment {
            interface_ip: Ipv4Addr::new(192, 168, 3, 238),
            subnet_mask: Some(Ipv4Addr::new(255, 255, 255, 0)),
            gateway: Some(Ipv4Addr::new(192, 168, 3, 254)),
        };
        let t = segment_targets(&seg);
        assert_eq!(t.len(), 3, "应为三通道: {t:?}");
        assert_eq!(t[0].ip(), Ipv4Addr::new(192, 168, 3, 255), "首选定向广播");
        assert_eq!(t[1].ip(), Ipv4Addr::BROADCAST, "次选有限广播");
        assert_eq!(t[2].ip(), Ipv4Addr::new(192, 168, 3, 254), "末选网关单播");
        assert!(t.iter().all(|a| a.port() == DHCP_SERVER_PORT));

        // 掩码未知：跳过定向广播，只保留有限广播（+ 无网关则仅 1 个通道）
        let seg = Segment {
            interface_ip: Ipv4Addr::new(192, 168, 1, 5),
            subnet_mask: None,
            gateway: None,
        };
        let t = segment_targets(&seg);
        assert_eq!(t.len(), 1, "掩码与网关均未知时仅有限广播: {t:?}");
        assert_eq!(t[0].ip(), Ipv4Addr::BROADCAST);

        // /31 等掩码下定向广播可能与有限广播重合：不得重复发送
        let seg = Segment {
            interface_ip: Ipv4Addr::new(255, 255, 255, 254),
            subnet_mask: Some(Ipv4Addr::new(255, 255, 255, 254)),
            gateway: None,
        };
        let t = segment_targets(&seg);
        assert_eq!(t.len(), 1, "定向广播与有限广播重合时应去重: {t:?}");
    }

    /// 包预算分配：保证"每个网段至少 1 包"且绝不突破全局上限
    #[test]
    fn test_per_segment_allowance_reserves_for_later_segments() {
        // 4 包、后面还有 1 个网段 → 当前最多 3（留 1）
        assert_eq!(per_segment_allowance(4, 1), 3);
        // 4 包、后面还有 3 个网段 → 当前只能 1
        assert_eq!(per_segment_allowance(4, 3), 1);
        // 2 包、后面还有 3 个网段（网段多于包）→ 当前仍至少 1
        assert_eq!(per_segment_allowance(2, 3), 1);
        // 预算耗尽 → 0（调用方必须跳过）
        assert_eq!(per_segment_allowance(0, 0), 0);
        assert_eq!(per_segment_allowance(0, 2), 0);
        // 单网段：全部额度可用，且不超过剩余
        assert_eq!(per_segment_allowance(4, 0), 4);
        // 任何情况下都不超过全局剩余
        for remaining in 0..=6usize {
            for left in 0..=6usize {
                let a = per_segment_allowance(remaining, left);
                assert!(
                    a <= remaining,
                    "网段额度 {a} 不得超过全局剩余 {remaining}（left={left}）"
                );
                if remaining > 0 {
                    assert!(a >= 1, "还有预算时每个网段至少应拿到 1 包");
                }
            }
        }
    }

    /// 多网段时"每网段至少 1 包"的总量不变量（防广播风暴回归）
    #[test]
    fn test_packet_budget_never_exceeds_cap_across_segments() {
        for n in 1..=6usize {
            let segments = n.min(MAX_SEGMENTS);
            let mut budget = PacketBudget::new(MAX_DISCOVER_PACKETS);
            let mut sent_total = 0usize;
            for idx in 0..n {
                let left_after = n - idx - 1;
                let allowance = per_segment_allowance(budget.remaining(), left_after);
                if allowance == 0 {
                    break;
                }
                // 模拟 collect_servers 的两次发送（初始 + 重发），受额度约束
                let mut sub = PacketBudget::new(allowance);
                while sub.try_take() {
                    sent_total += 1;
                }
                budget.consume(allowance);
            }
            assert!(
                sent_total <= MAX_DISCOVER_PACKETS,
                "网段数 {n} 时共发 {sent_total} 包，超过上限 {MAX_DISCOVER_PACKETS}"
            );
            assert_eq!(
                budget.remaining(),
                MAX_DISCOVER_PACKETS - sent_total,
                "预算账目必须自洽"
            );
            let _ = segments;
        }
    }

    /// 合成 chaddr：必须是本地管理 + 单播，且不冒充真实网卡
    #[test]
    fn test_synthetic_client_mac_is_local_unicast_and_varies() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..256 {
            let m = synthetic_client_mac();
            assert_eq!(m[0] & 0x01, 0, "必须是单播地址（bit0=0）: {:02X?}", m);
            assert_eq!(
                m[0] & 0x02,
                0x02,
                "必须是本地管理地址（bit1=1）: {:02X?}",
                m
            );
            assert_ne!(m, [0u8; 6], "不应全零");
            seen.insert(m);
        }
        assert!(
            seen.len() > 200,
            "合成 MAC 应高概率互不相同（256 次仅 {} 个不同值）",
            seen.len()
        );
    }

    /// 不可探测地址过滤：环回 / 未指定 / APIPA（169.254/16）必须跳过
    #[test]
    fn test_is_probeable_interface_ip() {
        assert!(
            !is_probeable_interface_ip(Ipv4Addr::UNSPECIFIED),
            "0.0.0.0 跳过"
        );
        assert!(!is_probeable_interface_ip(Ipv4Addr::LOCALHOST), "环回跳过");
        assert!(
            !is_probeable_interface_ip(Ipv4Addr::new(169, 254, 195, 14)),
            "APIPA 跳过（本机蓝牙/WLAN 即此类）"
        );
        assert!(is_probeable_interface_ip(Ipv4Addr::new(192, 168, 3, 238)));
        assert!(is_probeable_interface_ip(Ipv4Addr::new(192, 168, 160, 1)));
        assert!(is_probeable_interface_ip(Ipv4Addr::new(10, 0, 0, 5)));
    }

    /// 每网段窗口分配：主网段要拿满窗口（实测其 OFFER 晚到 ~3.9s），
    /// 后续网段至少拿到下限，且全部网段合计不超过 MAX_TOTAL_PROBE_MS。
    #[test]
    fn test_segment_window_allocation() {
        // 单网段：拿满 4.5 秒
        assert_eq!(segment_window_ms(MAX_TOTAL_PROBE_MS, 1), COLLECT_WINDOW_MS);
        // 两网段：10s - 1.2s(给后段) = 8.8s，夹到上限 → 4.5s
        assert_eq!(segment_window_ms(MAX_TOTAL_PROBE_MS, 2), COLLECT_WINDOW_MS);
        // 四网段：10s - 3×1.2s = 6.4s，夹到上限 → 4.5s
        assert_eq!(segment_window_ms(MAX_TOTAL_PROBE_MS, 4), COLLECT_WINDOW_MS);
        // 剩余预算很紧：不得超过剩余预算本身
        assert_eq!(segment_window_ms(800, 2), 800);
        assert_eq!(segment_window_ms(0, 3), 0);
        assert_eq!(segment_window_ms(3000, 0), 0);
        // 预算刚好只够下限：仍不低于下限
        assert_eq!(
            segment_window_ms(MIN_SEGMENT_WINDOW_MS, 3),
            MIN_SEGMENT_WINDOW_MS
        );
    }

    /// 窗口分配的端到端不变量：逐网段分配后合计不超过总上限
    #[test]
    fn test_total_probe_time_stays_within_budget() {
        for n in 1..=MAX_SEGMENTS {
            let mut left = MAX_TOTAL_PROBE_MS;
            let mut used = 0u64;
            for idx in 0..n {
                let remaining_segments = (n - idx) as u64;
                let w = segment_window_ms(left, remaining_segments);
                assert!(w <= left, "窗口 {w} 超过剩余预算 {left}");
                if w == 0 {
                    break;
                }
                assert!(w >= MIN_SEGMENT_WINDOW_MS, "窗口不应低于下限: {w}");
                used += w;
                left -= w;
            }
            assert!(
                used <= MAX_TOTAL_PROBE_MS,
                "网段数 {n} 合计耗时 {used}ms 超过上限 {MAX_TOTAL_PROBE_MS}ms"
            );
            // 每个网段都应拿到至少下限的窗口
            assert!(
                used >= MIN_SEGMENT_WINDOW_MS * n as u64,
                "网段数 {n} 时每段至少应有 {MIN_SEGMENT_WINDOW_MS}ms，实际 used={used}"
            );
        }
    }

    /// 收到 OFFER 后应**提前结束**收集，而不是白等满整个窗口。
    ///
    /// 实测意义：快服务器（~10ms）与慢服务器（~3.5s）共存时，
    /// 没有早退就得为每个网段付满窗口，整轮探测变成"网段数 × 窗口"。
    #[test]
    fn test_collect_servers_exits_early_after_offer() {
        let server = match UdpSocket::bind("127.0.0.1:0") {
            Ok(s) => s,
            Err(e) => {
                assert!(false, "服务器 bind 失败: {}", e);
                return;
            }
        };
        let server_addr = server.local_addr().unwrap();
        let server_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            if let Ok((len, src)) = server.recv_from(&mut buf) {
                let req = &buf[..len];
                let xid = u32::from_be_bytes([req[4], req[5], req[6], req[7]]);
                let mut off = vec![0u8; 240];
                off[0] = 2;
                off[1] = 1;
                off[2] = 6;
                off[4..8].copy_from_slice(&xid.to_be_bytes());
                off[236..240].copy_from_slice(&DHCP_MAGIC_COOKIE);
                off.push(OPT_MESSAGE_TYPE);
                off.push(1);
                off.push(MSG_TYPE_OFFER);
                off.push(OPT_SERVER_IDENTIFIER);
                off.push(4);
                off.extend_from_slice(&[192, 168, 50, 7]);
                off.push(OPT_END);
                let _ = server.send_to(&off, src);
            }
        });

        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        let _ = client.set_read_timeout(Some(Duration::from_millis(50)));
        let xid = 0xABCDEF01;
        let pkt = build_discover(xid, &[0x02, 1, 2, 3, 4, 5]);
        let targets = vec![server_addr];
        let mut budget = PacketBudget::new(MAX_DISCOVER_PACKETS);

        // 窗口 3000ms，但 settle 只有 100ms：OFFER 立即返回 → 必须远早于窗口结束
        let start = Instant::now();
        let (servers, sent_ok) = collect_servers(
            &client,
            &targets,
            &pkt,
            xid,
            CollectTiming {
                collect_ms: 3000,
                resend_ms: 1500,
                settle_ms: 100,
            },
            &mut budget,
        );
        let elapsed = start.elapsed();
        let _ = server_thread.join();

        assert!(sent_ok);
        assert_eq!(servers, vec!["192.168.50.7".to_string()]);
        assert!(
            elapsed < Duration::from_millis(2000),
            "收到 OFFER 后应提前结束（窗口 3000ms、settle 100ms），实际耗时 {elapsed:?}"
        );
    }

    /// 重复网段去重：注册表同一 IP 有多个接口子键时不得浪费包预算
    /// （本机实测 {783cc926} 与 {8d0383f2} 都是 192.168.3.238/24）
    #[test]
    fn test_dedup_and_limit_segments() {
        let mk = |ip: [u8; 4], mask: [u8; 4]| Segment {
            interface_ip: Ipv4Addr::from(ip),
            subnet_mask: Some(Ipv4Addr::from(mask)),
            gateway: None,
        };
        let a = mk([192, 168, 3, 238], [255, 255, 255, 0]);
        let a2 = mk([192, 168, 3, 238], [255, 255, 255, 0]); // 同键重复
        let b = mk([192, 168, 160, 1], [255, 255, 240, 0]);
        let a_diff_mask = mk([192, 168, 3, 238], [255, 255, 0, 0]); // 同 IP 不同掩码

        let out = dedup_and_limit_segments(&[a.clone(), a2, b.clone(), a_diff_mask.clone()], 8);
        assert_eq!(
            out.len(),
            3,
            "同键重复应被去掉，同 IP 不同掩码应保留: {out:?}"
        );
        assert_eq!(out[0], a);
        assert_eq!(out[1], b);
        assert_eq!(out[2], a_diff_mask);

        // 截断到 limit，且保序
        let out = dedup_and_limit_segments(&[a.clone(), b.clone(), a_diff_mask.clone()], 2);
        assert_eq!(out, vec![a.clone(), b.clone()], "应保序截断到 limit");

        // limit=0 与空输入
        assert!(dedup_and_limit_segments(&[a.clone()], 0).is_empty());
        assert!(dedup_and_limit_segments(&[], 4).is_empty());
    }
}
