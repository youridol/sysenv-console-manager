/// Network diagnostic module for SECM.
///
/// Features:
/// - Ping (ICMP echo) with configurable count, interval, continuous mode, cancel
/// - Traceroute with incremental TTL, IPv4/IPv6 support
/// - Nslookup (DNS resolution) with A/AAAA record filtering
/// - STUN NAT type detection (RFC 3489/5780)
///
/// 取消支持由 `crate::cancel` 模块提供（通过 re-export 保持向后兼容）。
/// STUN implementation reference: RFC 3489 & RFC 5780 — IETF Standards
use rand::Rng;
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Duration;

// ============================================================================
// 取消注册表 — re-export 自 crate::cancel
// ============================================================================

pub use crate::cancel::{cancel_command, cancel_flag, clear_cancel, is_cancelled};

// ============================================================================
// Streaming event payloads
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct StreamEvent {
    pub kind: String, // "ping", "trace-hop", "dns-record", "stun-step", "summary", "error"
    pub data: String, // JSON payload or plain text
}

// ============================================================================
// Ping (streaming)
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct PingResult {
    pub sequence: u32,
    pub rtt_ms: f64,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PingSummary {
    pub target: String,
    pub sent: u32,
    pub received: u32,
    pub loss_percent: f64,
    pub avg_rtt_ms: f64,
    pub min_rtt_ms: f64,
    pub max_rtt_ms: f64,
    pub results: Vec<PingResult>,
}

/// Streaming ping: emits net-stream events and checks cancel flag each iteration.
///
/// `ttl` 会写入 ICMP socket 的 TTL 选项（surge_ping `Config::ttl` → `set_ttl_v4` /
/// `set_unicast_hops_v6`）。此前该参数名为 `_ttl`，即**静默丢弃**用户设置：
/// 前端「TTL」输入框（1–255，默认 64）展示的值从未生效（ADR-0001 D11）。
/// 取值收敛到 1..=255：0 或越界值会让部分平台直接建 socket 失败。
pub async fn ping_streaming(
    target: &str,
    count: u32,
    interval_ms: u64,
    cmd_id: &str,
    continuous: bool,
    ip_version: &str,
    _proto: &str,
    size: u32,
    ttl: u32,
    deadline_secs: u32,
    emitter: impl Fn(StreamEvent),
) -> Result<PingSummary, String> {
    use surge_ping::{Client, Config, PingIdentifier, PingSequence, ICMP};
    use tokio::time::{sleep, Instant as TokInstant};

    let count = if continuous {
        u32::MAX
    } else {
        count.min(65535).max(1)
    };
    let interval_ms = interval_ms.min(60000).max(10);
    let payload_size = size.max(32).min(65507) as usize;
    let addr = resolve_host_with_version(target, ip_version)?;
    let deadline = if deadline_secs > 0 {
        Some(TokInstant::now() + std::time::Duration::from_secs(deadline_secs as u64))
    } else {
        None
    };

    let mut results = Vec::new();

    let icmp_kind = if addr.is_ipv6() { ICMP::V6 } else { ICMP::V4 };
    // TTL 收敛到 1..=255（0 在部分平台会让 socket 创建失败，且用户输入框本就限定该区间）
    let ttl = ttl.clamp(1, 255);
    let cfg = Config::builder().kind(icmp_kind).ttl(ttl).build();
    let client = Client::new(&cfg).map_err(|e| format!("Ping client error: {}", e))?;

    for seq in 0..count {
        if is_cancelled(cmd_id) {
            emitter(StreamEvent {
                kind: "error".into(),
                data: "用户取消".into(),
            });
            break;
        }
        if let Some(dl) = deadline {
            if TokInstant::now() >= dl {
                emitter(StreamEvent {
                    kind: "info".into(),
                    data: "截止时间到达，测试结束".into(),
                });
                break;
            }
        }

        let mut pinger = client.pinger(addr, PingIdentifier(rand::random())).await;
        let start = std::time::Instant::now();

        let payload = vec![0u8; payload_size];
        let r = match pinger.ping(PingSequence(seq as u16), &payload).await {
            Ok(_) => {
                let rtt = start.elapsed().as_secs_f64() * 1000.0;
                PingResult {
                    sequence: seq,
                    rtt_ms: rtt,
                    success: true,
                }
            }
            Err(_) => PingResult {
                sequence: seq,
                rtt_ms: 0.0,
                success: false,
            },
        };

        let json = serde_json::to_string(&r).unwrap_or_default();
        emitter(StreamEvent {
            kind: "ping".into(),
            data: json,
        });
        results.push(r);

        if seq < count.saturating_sub(1) && !is_cancelled(cmd_id) {
            sleep(Duration::from_millis(interval_ms)).await;
        }
    }

    let sent = results.len() as u32;
    let received = results.iter().filter(|r| r.success).count() as u32;
    let loss_percent = if sent > 0 {
        ((sent - received) as f64 / sent as f64) * 100.0
    } else {
        100.0
    };
    let rtts: Vec<f64> = results
        .iter()
        .filter(|r| r.success)
        .map(|r| r.rtt_ms)
        .collect();
    let avg_rtt = if !rtts.is_empty() {
        rtts.iter().sum::<f64>() / rtts.len() as f64
    } else {
        0.0
    };
    let min_rtt = rtts.iter().cloned().fold(f64::MAX, f64::min);
    let max_rtt = rtts.iter().cloned().fold(0.0, f64::max);

    let summary = PingSummary {
        target: target.to_string(),
        sent,
        received,
        loss_percent,
        avg_rtt_ms: avg_rtt,
        min_rtt_ms: if min_rtt == f64::MAX { 0.0 } else { min_rtt },
        max_rtt_ms: max_rtt,
        results,
    };

    let json = serde_json::to_string(&summary).unwrap_or_default();
    emitter(StreamEvent {
        kind: "summary".into(),
        data: json,
    });

    Ok(summary)
}

/// Old synchronous wrapper kept for backward compat.

// ============================================================================
// Traceroute (streaming)
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct TraceHop {
    pub hop: u8,
    pub ip: String,
    pub rtt_avg_ms: f64,
    pub rtt_min_ms: f64,
    pub rtt_max_ms: f64,
    pub probes_answered: u8,
    pub probes_sent: u8,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceResult {
    pub target: String,
    pub max_hops: u8,
    pub hops: Vec<TraceHop>,
}

/// Streaming traceroute.
///
/// 自研 ICMP 实现（surge_ping 的 pinger 无法接收中间路由器 TTL 超时报文——
/// 其回复匹配 key 使用目标 IP，而 Time Exceeded 报文源地址是路由器 IP，
/// 导致中间跳 100% 丢失，故改为直接构造 ICMP Echo 报文 + 自定义收包匹配）。
pub async fn traceroute_streaming(
    target: &str,
    max_hops: u8,
    cmd_id: &str,
    ip_version: &str,
    _proto: &str,
    dns_server: Option<&str>,
    emitter: impl Fn(StreamEvent),
) -> Result<TraceResult, String> {
    // DNS 解析可能调用 trust-dns 同步 Resolver（内部 block_on 创建 runtime），
    // 必须在 spawn_blocking 线程执行，避免 async runtime 内 panic
    let target_owned = target.to_string();
    let ipv_owned = ip_version.to_string();
    let dns_owned = dns_server.map(|s| s.to_string());
    let addr = tokio::task::spawn_blocking(move || {
        resolve_host_with_dns(&target_owned, &ipv_owned, dns_owned.as_deref())
    })
    .await
    .map_err(|e| format!("DNS 解析线程失败: {}", e))??;
    let max_hops = max_hops.min(64).max(1);
    let mut hops = Vec::new();
    // 随机 identifier（每跳探测共享），sequence 全局递增避免跨跳混淆
    let ident = rand::random::<u16>();
    let mut seq_counter: u16 = 0;

    for ttl in 1..=max_hops {
        if is_cancelled(cmd_id) {
            emitter(StreamEvent {
                kind: "error".into(),
                data: "用户取消".into(),
            });
            break;
        }

        let (rtts, hop_ip, got_target) =
            match traceroute_probe_hop(addr, ttl as u32, ident, &mut seq_counter).await {
                Ok(r) => r,
                Err(e) => {
                    emitter(StreamEvent {
                        kind: "info".into(),
                        data: format!("跳点 {}: {}", ttl, e),
                    });
                    (Vec::new(), None, false)
                }
            };

        let probes_sent: u8 = 3;
        let probes_answered = rtts.len() as u8;
        let hop_addr = hop_ip.unwrap_or_else(|| "*".to_string());
        let avg = if !rtts.is_empty() {
            rtts.iter().sum::<f64>() / rtts.len() as f64
        } else {
            0.0
        };
        let min = rtts.iter().cloned().fold(f64::MAX, f64::min);
        let max = rtts.iter().cloned().fold(0.0, f64::max);

        let hop = TraceHop {
            hop: ttl,
            ip: hop_addr.clone(),
            rtt_avg_ms: avg,
            rtt_min_ms: if min == f64::MAX { 0.0 } else { min },
            rtt_max_ms: max,
            probes_answered,
            probes_sent,
        };

        let json = serde_json::to_string(&hop).unwrap_or_default();
        emitter(StreamEvent {
            kind: "trace-hop".into(),
            data: json,
        });
        hops.push(hop);

        if got_target {
            break;
        }
    }

    let result = TraceResult {
        target: target.to_string(),
        max_hops,
        hops,
    };
    let json = serde_json::to_string(&result).unwrap_or_default();
    emitter(StreamEvent {
        kind: "summary".into(),
        data: json,
    });
    Ok(result)
}

/// 计算 Internet 校验和（16 位反码和，RFC 1071）
fn icmp_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// 构造 ICMPv4 Echo Request 报文（type=8）
fn build_icmpv4_echo(identifier: u16, sequence: u16, payload: &[u8]) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(8 + payload.len());
    pkt.extend_from_slice(&[8, 0, 0, 0]); // type=Echo Request, code=0, checksum 占位
    pkt.extend_from_slice(&identifier.to_be_bytes());
    pkt.extend_from_slice(&sequence.to_be_bytes());
    pkt.extend_from_slice(payload);
    let cksum = icmp_checksum(&pkt);
    pkt[2..4].copy_from_slice(&cksum.to_be_bytes());
    pkt
}

/// 构造 ICMPv6 Echo Request 报文（type=128，含伪头校验和）
fn build_icmpv6_echo(
    identifier: u16,
    sequence: u16,
    src: Ipv6Addr,
    dst: Ipv6Addr,
    payload: &[u8],
) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(8 + payload.len());
    pkt.extend_from_slice(&[128, 0, 0, 0]); // type=Echo Request(128), code=0, checksum 占位
    pkt.extend_from_slice(&identifier.to_be_bytes());
    pkt.extend_from_slice(&sequence.to_be_bytes());
    pkt.extend_from_slice(payload);
    // ICMPv6 校验和覆盖伪头（src+dst+len+零+next header）+ 报文
    let mut pseudo = Vec::with_capacity(40 + pkt.len());
    pseudo.extend_from_slice(&src.octets());
    pseudo.extend_from_slice(&dst.octets());
    pseudo.extend_from_slice(&((pkt.len() as u32).to_be_bytes()));
    pseudo.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 58]); // 3 字节零 + next header(58=ICMPv6)
    pseudo.extend_from_slice(&pkt);
    let cksum = icmp_checksum(&pseudo);
    pkt[2..4].copy_from_slice(&cksum.to_be_bytes());
    pkt
}

/// 解析收到的 IPv4 报文（Windows DGRAM+ICMP socket 递送完整 IP 包）
/// 返回 (icmp_type, code, identifier, sequence, 源 IP)；仅识别 Echo Reply(0) 与 Time Exceeded(11)
fn parse_ipv4_icmp_reply(buf: &[u8]) -> Option<(u8, u8, u16, u16, IpAddr)> {
    if buf.len() < 20 {
        return None;
    }
    let ihl = ((buf[0] & 0x0f) as usize) * 4;
    if buf.len() < ihl + 8 {
        return None;
    }
    let src = Ipv4Addr::new(buf[12], buf[13], buf[14], buf[15]);
    let icmp_type = buf[ihl];
    let icmp_code = buf[ihl + 1];
    match icmp_type {
        0 => {
            // Echo Reply：id/seq 位于 ICMP 头
            let id = u16::from_be_bytes([buf[ihl + 4], buf[ihl + 5]]);
            let seq = u16::from_be_bytes([buf[ihl + 6], buf[ihl + 7]]);
            Some((icmp_type, icmp_code, id, seq, IpAddr::V4(src)))
        }
        11 => {
            // Time Exceeded：ICMP 头(8B 含 unused)之后 = 内嵌原始 IP 头 + 原始 echo 头(8B)
            let inner = &buf[ihl + 8..];
            // 上面的长度检查只保证 `buf.len() >= ihl + 8`，故 `inner` 可能是空切片。
            // 必须先判空再取 `inner[0]`，否则空切片索引 panic（ADR-0001 D5）。
            // 对照 IPv6 分支（下方 parse_ipv6_icmp_reply）：其长度校验在索引之前，写法正确。
            if inner.is_empty() {
                return None;
            }
            // 内嵌 IP 头：inner[0] 是版本/IHL 字段（inner[4] 是 TTL）
            let inner_ihl = ((inner[0] & 0x0f) as usize) * 4;
            if inner.len() < inner_ihl + 8 {
                return None;
            }
            // echo 头 = type/code/checksum(4) + identifier(2) + sequence(2)
            let id = u16::from_be_bytes([inner[inner_ihl + 4], inner[inner_ihl + 5]]);
            let seq = u16::from_be_bytes([inner[inner_ihl + 6], inner[inner_ihl + 7]]);
            Some((icmp_type, icmp_code, id, seq, IpAddr::V4(src)))
        }
        _ => None,
    }
}

/// 解析收到的 IPv6 报文（IPv6 头固定 40 字节 + ICMP）
/// 返回 (icmp_type, code, identifier, sequence, 源 IP)；仅识别 Echo Reply(129) 与 Time Exceeded(3)
fn parse_ipv6_icmp_reply(buf: &[u8]) -> Option<(u8, u8, u16, u16, IpAddr)> {
    if buf.len() < 48 {
        return None;
    }
    let src = Ipv6Addr::from(<[u8; 16]>::try_from(&buf[8..24]).ok()?);
    let icmp_type = buf[40];
    let icmp_code = buf[41];
    match icmp_type {
        129 => {
            // Echo Reply
            let id = u16::from_be_bytes([buf[44], buf[45]]);
            let seq = u16::from_be_bytes([buf[46], buf[47]]);
            Some((icmp_type, icmp_code, id, seq, IpAddr::V6(src)))
        }
        3 => {
            // Time Exceeded：ICMP 头(8B)之后 = 内嵌原始 IPv6 头(40) + 原始 echo 头(8B)
            let inner = &buf[48..];
            if inner.len() < 40 + 8 {
                return None;
            }
            // echo 头 = type/code/checksum(4) + identifier(2) + sequence(2)
            let id = u16::from_be_bytes([inner[40 + 4], inner[40 + 5]]);
            let seq = u16::from_be_bytes([inner[40 + 6], inner[40 + 7]]);
            Some((icmp_type, icmp_code, id, seq, IpAddr::V6(src)))
        }
        _ => None,
    }
}

/// 单跳探测：创建带指定 TTL 的 ICMP socket，发送 3 个 probe，共享收包循环
/// 返回 (各 probe RTT 列表, 首个响应源地址, 是否到达目标)
async fn traceroute_probe_hop(
    target: IpAddr,
    ttl: u32,
    ident: u16,
    seq_counter: &mut u16,
) -> Result<(Vec<f64>, Option<String>, bool), String> {
    use socket2::{Domain, Protocol, Type};
    use std::net::UdpSocket;

    let (domain, proto) = if target.is_ipv4() {
        (Domain::IPV4, Protocol::ICMPV4)
    } else {
        (Domain::IPV6, Protocol::ICMPV6)
    };
    // Windows 上 SOCK_DGRAM+IPPROTO_ICMP 可能不可用（WSAEPROTONOSUPPORT），
    // 回退到 SOCK_RAW+IPPROTO_ICMP（需要管理员权限，与 surge_ping 的 fallback 策略一致）
    let sock2 = match socket2::Socket::new(domain, Type::DGRAM, Some(proto)) {
        Ok(s) => s,
        Err(_) => socket2::Socket::new(domain, Type::RAW, Some(proto))
            .map_err(|e| format!("创建 ICMP socket 失败（需要管理员权限）: {}", e))?,
    };
    sock2
        .set_nonblocking(true)
        .map_err(|e| format!("设置非阻塞失败: {}", e))?;
    // 设置 TTL（IPv4: IP_TTL；IPv6: IPV6_UNICAST_HOPS）
    {
        let sref = socket2::SockRef::from(&sock2);
        if target.is_ipv4() {
            sref.set_ttl_v4(ttl)
                .map_err(|e| format!("设置 TTL 失败: {}", e))?;
        } else {
            sref.set_unicast_hops_v6(ttl)
                .map_err(|e| format!("设置 TTL 失败: {}", e))?;
        }
    }
    let udp = UdpSocket::from(sock2);
    let udp = tokio::net::UdpSocket::from_std(udp)
        .map_err(|e| format!("转换 tokio socket 失败: {}", e))?;

    // ICMPv6 需要本机源地址构造伪头校验和：connect 后由内核选定源地址
    let v6_src = if target.is_ipv6() {
        let _ = udp.connect(SocketAddr::new(target, 0)).await;
        udp.local_addr().ok().map(|a| a.ip())
    } else {
        None
    };

    // 发送 3 个 probe，记录每个 sequence 的发送时刻
    const PROBE_COUNT: usize = 3;
    let mut send_times = [std::time::Instant::now(); PROBE_COUNT];
    let mut seqs = [0u16; PROBE_COUNT];
    for i in 0..PROBE_COUNT {
        let seq = seq_counter.wrapping_add(i as u16);
        seqs[i] = seq;
        send_times[i] = std::time::Instant::now();
        let pkt = if target.is_ipv4() {
            build_icmpv4_echo(ident, seq, &[0u8; 32])
        } else {
            let src6 = match v6_src {
                Some(IpAddr::V6(a)) => a,
                _ => Ipv6Addr::UNSPECIFIED,
            };
            let dst6 = match target {
                IpAddr::V6(a) => a,
                _ => Ipv6Addr::UNSPECIFIED,
            };
            build_icmpv6_echo(ident, seq, src6, dst6, &[0u8; 32])
        };
        let _ = udp.send_to(&pkt, SocketAddr::new(target, 0)).await;
    }
    *seq_counter = seq_counter.wrapping_add(PROBE_COUNT as u16);

    // 收包循环：匹配 Echo Reply 与 Time Exceeded，直到集齐 3 个回复或超时
    let hop_timeout = Duration::from_millis(1200);
    let deadline = std::time::Instant::now() + hop_timeout;
    let mut results: Vec<(u16, f64, String)> = Vec::new();
    let mut buf = [0u8; 2048];
    while results.len() < PROBE_COUNT {
        let now = std::time::Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline.saturating_duration_since(now);
        let r = tokio::time::timeout(remaining, udp.recv_from(&mut buf)).await;
        match r {
            Ok(Ok((sz, _src))) => {
                let parsed = if target.is_ipv4() {
                    parse_ipv4_icmp_reply(&buf[..sz])
                } else {
                    parse_ipv6_icmp_reply(&buf[..sz])
                };

                if let Some((itype, _code, id, seq, src)) = parsed {
                    let _ = itype;
                    if id == ident
                        && seqs.contains(&seq)
                        && !results.iter().any(|(s, _, _)| *s == seq)
                    {
                        let idx = seqs.iter().position(|s| *s == seq).unwrap_or(0);
                        let rtt = send_times[idx].elapsed().as_secs_f64() * 1000.0;
                        results.push((seq, rtt, src.to_string()));
                    }
                }
            }
            Ok(Err(_)) => {}
            Err(_) => break, // 超时
        }
    }

    let mut rtts = Vec::new();
    let mut hop_src: Option<String> = None;
    let mut got_target = false;
    for (_, rtt, src) in &results {
        if hop_src.is_none() {
            hop_src = Some(src.clone());
        }
        if *src == target.to_string() {
            got_target = true;
        }
        rtts.push(*rtt);
    }
    Ok((rtts, hop_src, got_target))
}

// ============================================================================
// Nslookup (streaming)
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct DnsResult {
    pub domain: String,
    pub record_type: String,
    pub records: Vec<String>,
    pub server: String,
}

/// Nslookup with streaming (DNS is fast, but we still stream for UX consistency).
pub fn nslookup_streaming(
    domain: &str,
    record_type: &str,
    cmd_id: &str,
    emitter: impl Fn(StreamEvent),
) -> Result<DnsResult, String> {
    use trust_dns_resolver::config::{ResolverConfig, ResolverOpts};
    use trust_dns_resolver::Resolver;

    if is_cancelled(cmd_id) {
        emitter(StreamEvent {
            kind: "error".into(),
            data: "用户取消".into(),
        });
        return Err("Cancelled".into());
    }

    emitter(StreamEvent {
        kind: "info".into(),
        data: format!(
            "开始 DNS 查询: {}  记录类型: {}  服务器: 系统默认 DNS",
            domain,
            record_type.to_uppercase()
        ),
    });

    let resolver = match Resolver::new(ResolverConfig::default(), ResolverOpts::default()) {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("DNS 解析器初始化失败: {}", e);
            emitter(StreamEvent {
                kind: "error".into(),
                data: msg.clone(),
            });
            return Err(msg);
        }
    };

    if is_cancelled(cmd_id) {
        emitter(StreamEvent {
            kind: "error".into(),
            data: "用户取消".into(),
        });
        return Err("Cancelled".into());
    }

    let records: Vec<String> = match record_type.to_uppercase().as_str() {
        "A" => match resolver.ipv4_lookup(domain) {
            Ok(ips) => ips.iter().map(|ip| ip.to_string()).collect(),
            Err(e) => {
                let msg = format!("A记录查询失败: {}", e);
                emitter(StreamEvent {
                    kind: "error".into(),
                    data: msg.clone(),
                });
                return Err(msg);
            }
        },
        "AAAA" => match resolver.ipv6_lookup(domain) {
            Ok(ips) => ips.iter().map(|ip| ip.to_string()).collect(),
            Err(e) => {
                let msg = format!("AAAA记录查询失败: {}", e);
                emitter(StreamEvent {
                    kind: "error".into(),
                    data: msg.clone(),
                });
                return Err(msg);
            }
        },
        _ => {
            let msg = format!("不支持的记录类型: {}", record_type);
            emitter(StreamEvent {
                kind: "error".into(),
                data: msg.clone(),
            });
            return Err(msg);
        }
    };

    for rec in &records {
        emitter(StreamEvent {
            kind: "dns-record".into(),
            data: format!("[{} 记录] {}", record_type.to_uppercase(), rec),
        });
    }

    let result = DnsResult {
        domain: domain.to_string(),
        record_type: record_type.to_uppercase(),
        records,
        server: "系统默认 DNS".to_string(),
    };
    let json = serde_json::to_string(&result).unwrap_or_default();
    emitter(StreamEvent {
        kind: "summary".into(),
        data: json,
    });
    Ok(result)
}

// ============================================================================
// STUN NAT Type Detection (RFC 3489/5780) - streaming
// ============================================================================

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum NatType {
    #[serde(rename = "NAT0")]
    Nat0,
    #[serde(rename = "NAT1")]
    Nat1,
    #[serde(rename = "NAT2")]
    Nat2,
    #[serde(rename = "NAT3")]
    Nat3,
    #[serde(rename = "NAT4")]
    Nat4,
}

impl std::fmt::Display for NatType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NatType::Nat0 => write!(f, "NAT0 — 公网 / 无 NAT"),
            NatType::Nat1 => write!(f, "NAT1 — 全锥形 NAT (Full Cone)"),
            NatType::Nat2 => write!(f, "NAT2 — 受限锥形 NAT (Restricted Cone)"),
            NatType::Nat3 => write!(f, "NAT3 — 端口受限锥形 NAT (Port Restricted Cone)"),
            NatType::Nat4 => write!(f, "NAT4 — 对称 NAT (Symmetric)"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StunTestStep {
    pub phase: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NatDetectionResult {
    pub nat_type: NatType,
    pub nat_label: String,
    pub public_ip: String,
    pub public_port: u16,
    pub local_ip: String,
    pub steps: Vec<StunTestStep>,
    pub success: bool,
    pub error_message: String,
}

/// 内置 STUN 服务器列表（顺序即首选优先级）。
///
/// 排序依据 2026-09 实测（UDP Binding Request 往返）：可用者前置，失效者后置。
/// 实测结果：`stun.l.google.com` / `stun1/2.l.google.com` / `stun.freeswitch.org` /
/// `global.stun.twilio.com` / `stun.miwifi.com` / `stun.nextcloud.com` /
/// `stun.linphone.org` 正常响应；`stun.qq.com`（连接被重置）与 `stun.sipgate.net`
/// （无响应）已失效，故移到末尾并保留（服务可能恢复，且 detect_nat_streaming 会
/// 自动顺延，不会阻塞用户）。
const STUN_SERVERS: &[(&str, &str, u16)] = &[
    ("Google Primary", "stun.l.google.com", 19302),
    ("Google Alt", "stun1.l.google.com", 19302),
    ("Freeswitch", "stun.freeswitch.org", 3478),
    ("Twilio", "global.stun.twilio.com", 3478),
    ("Miwifi", "stun.miwifi.com", 3478),
    ("Nextcloud", "stun.nextcloud.com", 3478),
    ("Linphone", "stun.linphone.org", 3478),
    ("Google v6", "stun2.l.google.com", 19302),
    ("Ekiga", "stun.ekiga.net", 3478),
    ("3CX", "stun.3cx.com", 3478),
    ("Vivox", "stun.vivox.com", 3478),
    ("Blink", "stun.blink.de", 3478),
    ("Numb", "stun.counterpath.com", 3478),
    // 实测失效（保留在列表末尾，恢复后自动可用）
    ("QQ", "stun.qq.com", 3478),
    ("Sipgate", "stun.sipgate.net", 3478),
];

pub fn get_stun_servers() -> Vec<String> {
    STUN_SERVERS
        .iter()
        .map(|s| format!("{}:{}", s.1, s.2))
        .collect()
}

// ============================================================================
// 网站/端口 HTTP 延迟探测（服务端执行）
// ============================================================================

/// 单次 HTTP 延迟探测结果（`probe_url` 命令载荷）。
#[derive(Debug, Clone, Serialize)]
pub struct UrlProbeResult {
    /// 探测是否成功（收到任何 HTTP 响应即算可达，含 4xx/5xx）
    pub reachable: bool,
    /// **HTTP 延迟**（毫秒）：本机发起请求到收到 HTTP 响应头之间的真实墙钟时间，
    /// 即 DNS + TCP + TLS 握手 + 请求/响应（HEAD 优先，405/501 时 GET 兜底）。
    ///
    /// 口径说明（避免再次被误读）：这是**应用层 HTTP 往返**，不是 ICMP ping 的
    /// 网络往返，也不是浏览器打开网页的耗时——它不含子资源加载与渲染。
    /// 「网站延迟测试」展示的就是本字段，UI 与日志必须标注为 HTTP 延迟，
    /// 不得用 ICMP 数值或浏览器页面耗时冒充（见 `network_probe_contract.rs`）。
    pub ms: u64,
    /// HTTP 状态码；未取得响应时为 None
    pub status: Option<u16>,
    /// 失败原因（可达时为 None）
    pub error: Option<String>,
}

/// 探测目标的归一化 URL（无协议时补 `http://`）。
///
/// 「HTTP 延迟测试」与「端口检测」共用同一归一化结果，
/// 保证两者测的是同一个地址。
pub fn normalize_probe_url(url: &str) -> String {
    if url.contains("://") {
        url.to_string()
    } else {
        format!("http://{}", url)
    }
}

/// 探测目标 URL / host:port 的 HTTP 延迟与可达性（**服务端**执行，不受 WebView CSP 限制）。
///
/// 为什么必须在后端做：
///   HTTP 延迟测试原本用浏览器 `fetch(url, {mode:"no-cors"})` 直连外部站点，
///   但 v1.19.0 收紧了 CSP `connect-src`（移除通配 `https:`/`http:`），
///   于是**发行版**里所有外部 fetch 都被 WebView2 拦截 → 全部显示"不可达"。
///   用户还能自定义任意站点（localStorage `secm_sites`），无法用固定的
///   CSP 白名单覆盖；把探测移到 Rust 侧即可**保持 CSP 收紧**，
///   同时支持任意目标。v0.9.x 曾用"往 CSP 加回 https: http:"修过同一 BUG，
///   那次改动在 v1.19.0 的安全收紧中被回退，故本次改为不依赖 CSP 的方案。
///
/// 语义与前端旧的 `headProbe` 对齐：**收到任何 HTTP 响应即视为可达**
/// （4xx/5xx 也算——目标服务确实在应答；只有连接/超时/协议错误才算不可达）。
/// 返回的 `ms` 是 HTTP 层延迟，即本命令对外承诺的**唯一**耗时口径。
///
/// 连接超时单独设置：ureq 的 `.timeout()` 只作用于读写阶段，连接阶段受
/// `timeout_connect` 控制（默认 30s，实测回环"拒绝连接"要等 ~2s）。
/// 不显式设置会让一次探测在异常网络下悬挂很久。
pub fn probe_url(url: &str, timeout_ms: u64) -> UrlProbeResult {
    let started = std::time::Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    // 未带协议时按 http 处理（端口探测传 "http://host:port"，站点传完整 URL）
    let normalized = normalize_probe_url(url);

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .timeout_write(timeout)
        .build();

    // 整个探测（含 HEAD→GET 兜底）共享**同一个**总预算。
    // 若把 timeout 按"每次尝试"计，不可达目标会 HEAD 超时 + GET 再超时 = 2×budget
    // （实测预算 2s 时耗 4.0s），前端"检测中"转圈时间翻倍。
    let deadline = started + timeout;

    // HEAD 优先（轻量）；部分站点对 HEAD 返回 405/501，此时 GET 兜底。
    // `ureq` 把 4xx/5xx 也作为 Err(Status) 返回，需区分"有响应"与"真不可达"。
    // 预算耗尽时直接返回 None，不再启动第二次尝试。
    //
    // 每次尝试**单独计时**：可达时对外展示的 HTTP 延迟必须是"成功那次尝试"的
    // 耗时，不能把先前失败尝试（例如站点拒绝 HEAD）的时间累加进延迟，
    // 否则同一站点的读数会因兜底路径而虚高约一倍。
    let attempt = |method: &str| -> Option<Result<(u16, u64), String>> {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let attempt_started = std::time::Instant::now();
        let req = if method == "HEAD" {
            agent.head(&normalized)
        } else {
            agent.get(&normalized)
        };
        let took = |t: &std::time::Instant| t.elapsed().as_millis() as u64;
        Some(match req.call() {
            Ok(resp) => Ok((resp.status(), took(&attempt_started))),
            Err(ureq::Error::Status(code, _)) => Ok((code, took(&attempt_started))),
            Err(ureq::Error::Transport(t)) => Err(t.to_string()),
        })
    };

    let (reachable, status, error, ms) = match attempt("HEAD") {
        Some(Ok((code, took))) => (true, Some(code), None, took),
        head => {
            let head_err = match head {
                Some(Err(e)) => Some(e),
                // 预算已耗尽（None）：也走不可达，理由如实说明
                None => None,
                Some(Ok(_)) => unreachable!("Ok 分支已在上面处理"),
            };
            match attempt("GET") {
                Some(Ok((code, took))) => (true, Some(code), None, took),
                get => {
                    let get_err = match get {
                        Some(Err(e)) => Some(e),
                        None => None,
                        Some(Ok(_)) => unreachable!("Ok 分支已在上面处理"),
                    };
                    let detail = match (head_err, get_err) {
                        (Some(h), Some(g)) => format!("HEAD: {}; GET: {}", h, g),
                        (Some(h), None) => format!("HEAD: {}（预算已耗尽，未再尝试 GET）", h),
                        (None, Some(g)) => format!("GET: {}", g),
                        (None, None) => "预算已耗尽（连接超时）".to_string(),
                    };
                    // 不可达时无可用的单次尝试耗时，如实给出总消耗（即超时预算）
                    (
                        false,
                        None,
                        Some(detail),
                        started.elapsed().as_millis() as u64,
                    )
                }
            }
        }
    };

    UrlProbeResult {
        reachable,
        ms,
        status,
        error,
    }
}

fn build_binding_request(transaction_id: &[u8; 12]) -> Vec<u8> {
    let mut pkt = vec![0u8; 20];
    pkt[0..2].copy_from_slice(&0x0001u16.to_be_bytes());
    pkt[2..4].copy_from_slice(&0u16.to_be_bytes());
    pkt[4..8].copy_from_slice(&0x2112A442u32.to_be_bytes());
    pkt[8..20].copy_from_slice(transaction_id);
    pkt
}

fn parse_mapped_address(response: &[u8]) -> Option<(IpAddr, u16)> {
    if response.len() < 20 {
        return None;
    }
    let msg_len = u16::from_be_bytes([response[2], response[3]]) as usize;
    if response.len() < 20 + msg_len {
        return None;
    }
    let mut pos = 20;
    while pos + 4 <= 20 + msg_len {
        let attr_type = u16::from_be_bytes([response[pos], response[pos + 1]]);
        let attr_len = u16::from_be_bytes([response[pos + 2], response[pos + 3]]) as usize;
        if attr_type == 0x0001 && pos + 4 + attr_len <= response.len() && attr_len >= 8 {
            let family = response[pos + 5];
            let port = u16::from_be_bytes([response[pos + 6], response[pos + 7]]);
            if family == 0x01 {
                return Some((
                    IpAddr::from([
                        response[pos + 8],
                        response[pos + 9],
                        response[pos + 10],
                        response[pos + 11],
                    ]),
                    port,
                ));
            }
        } else if attr_type == 0x0020 && pos + 4 + attr_len <= response.len() && attr_len >= 8 {
            let family = response[pos + 5];
            let xport = u16::from_be_bytes([response[pos + 6], response[pos + 7]]);
            let port = xport ^ 0x2112;
            if family == 0x01 {
                let magic = &response[4..8];
                return Some((
                    IpAddr::from([
                        response[pos + 8] ^ magic[0],
                        response[pos + 9] ^ magic[1],
                        response[pos + 10] ^ magic[2],
                        response[pos + 11] ^ magic[3],
                    ]),
                    port,
                ));
            }
        }
        pos += 4 + attr_len;
        if attr_len % 4 != 0 {
            pos += 4 - (attr_len % 4);
        }
    }
    None
}

fn build_change_request(tid: &[u8; 12], change_ip: bool, change_port: bool) -> Vec<u8> {
    let mut value: u32 = 0;
    if change_ip {
        value |= 0x0002;
    }
    if change_port {
        value |= 0x0004;
    }
    let mut pkt = vec![0u8; 28];
    pkt[0..2].copy_from_slice(&0x0001u16.to_be_bytes());
    pkt[2..4].copy_from_slice(&8u16.to_be_bytes());
    pkt[4..8].copy_from_slice(&0x2112A442u32.to_be_bytes());
    pkt[8..20].copy_from_slice(tid);
    pkt[20..22].copy_from_slice(&0x0003u16.to_be_bytes());
    pkt[22..24].copy_from_slice(&4u16.to_be_bytes());
    pkt[24..28].copy_from_slice(&value.to_be_bytes());
    pkt
}

fn stun_step(emitter: &impl Fn(StreamEvent), phase: &str, description: &str) {
    let step = StunTestStep {
        phase: phase.to_string(),
        description: description.to_string(),
    };
    let json = serde_json::to_string(&step).unwrap_or_default();
    emitter(StreamEvent {
        kind: "stun-step".into(),
        data: json,
    });
}

/// STUN 候选列表：用户选定服务器优先，其后依次追加内置列表（去重）。
///
/// 动机：公网 STUN 服务长期存在失效情况（实测 `stun.qq.com`、
/// `stun.sipgate.net` 均无响应/连接被重置），而 NAT 检测三阶段必须使用**同一个**
/// 服务器，因此不能只在阶段一重试。原实现只尝试用户选中的单个服务器，导致
/// 默认项失效时整个功能在首次使用即报错。此处改为按序探测并自动顺延。
fn stun_candidates(requested: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::with_capacity(STUN_SERVERS.len() + 1);
    let trimmed = requested.trim();
    if !trimmed.is_empty() {
        v.push(trimmed.to_string());
    }
    for (_, host, port) in STUN_SERVERS {
        let s = format!("{}:{}", host, port);
        if !v.iter().any(|x| x.eq_ignore_ascii_case(&s)) {
            v.push(s);
        }
    }
    v
}

pub fn detect_nat_streaming(
    stun_server: &str,
    cmd_id: &str,
    emitter: impl Fn(StreamEvent),
) -> Result<NatDetectionResult, String> {
    let local_ip = get_local_ip();
    let mut rng = rand::thread_rng();

    // Phase 1：对候选服务器依次探测（首个有响应者胜出），失败自动顺延。
    let mut chosen: Option<(SocketAddr, IpAddr, u16, String)> = None;
    let mut last_err = String::from("无候选服务器");
    for cand in stun_candidates(stun_server) {
        if is_cancelled(cmd_id) {
            emitter(StreamEvent {
                kind: "error".into(),
                data: "用户取消".into(),
            });
            return Err("Cancelled".into());
        }
        let addr = match resolve_stun(&cand) {
            Ok(a) => a,
            Err(e) => {
                last_err = format!("{}: {}", cand, e);
                continue;
            }
        };
        let sk = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(e) => {
                last_err = format!("UDP bind失败: {}", e);
                continue;
            }
        };
        sk.set_read_timeout(Some(Duration::from_secs(3))).ok();
        let tid: [u8; 12] = rng.gen();
        if let Err(e) = sk.send_to(&build_binding_request(&tid), addr) {
            last_err = format!("{}: STUN send失败: {}", cand, e);
            continue;
        }
        let mut b = [0u8; 2048];
        match sk.recv_from(&mut b) {
            Ok((l, _)) => match parse_mapped_address(&b[..l]) {
                Some(m) => {
                    chosen = Some((addr, m.0, m.1, cand.clone()));
                    break;
                }
                None => last_err = format!("{}: 无法解析 MAPPED-ADDRESS", cand),
            },
            Err(e) => last_err = format!("{}: 阶段1超时: {}", cand, e),
        }
    }

    let (server_addr, mapped_ip, mapped_port, used) = match chosen {
        Some(v) => v,
        None => {
            let msg = format!(
                "所有 STUN 服务器均不可用（已尝试 {} 个，最后错误: {}）",
                stun_candidates(stun_server).len(),
                last_err
            );
            emitter(StreamEvent {
                kind: "error".into(),
                data: msg.clone(),
            });
            return Err(msg);
        }
    };

    stun_step(
        &emitter,
        "初始化",
        &format!(
            "STUN 服务器地址: {}:{} | 本机 IP 地址: {} | 开始 NAT 类型检测 (RFC 3489)",
            server_addr.ip(),
            server_addr.port(),
            local_ip
        ),
    );
    // 发生顺延时透出，便于用户理解为何不是自己选的服务器
    if !used.eq_ignore_ascii_case(stun_server.trim()) {
        stun_step(
            &emitter,
            "服务器自动切换",
            &format!(
                "选定服务器 {} 无响应，已改用 {}（公网 STUN 服务可能已失效）",
                stun_server.trim(),
                used
            ),
        );
    }

    let mapped = (mapped_ip, mapped_port);
    stun_step(
        &emitter,
        "阶段1: 绑定请求",
        &format!(
            "发送 Binding Request → 收到响应 | NAT 映射后的公网地址: {}:{}",
            mapped.0, mapped.1
        ),
    );

    if is_cancelled(cmd_id) {
        emitter(StreamEvent {
            kind: "error".into(),
            data: "用户取消".into(),
        });
        return Err("Cancelled".into());
    }

    // Phase 2
    let socket2 = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("UDP bind2失败: {}", e))?;
    socket2.set_read_timeout(Some(Duration::from_secs(3))).ok();
    let tid2: [u8; 12] = rng.gen();
    socket2
        .send_to(&build_binding_request(&tid2), server_addr)
        .map_err(|e| format!("STUN send2失败: {}", e))?;
    let mut buf2 = [0u8; 2048];
    let (len2, _) = socket2
        .recv_from(&mut buf2)
        .map_err(|_| "阶段2超时".to_string())?;
    let mapped2 = parse_mapped_address(&buf2[..len2]).ok_or_else(|| "阶段2解析失败".to_string())?;
    let port_changed = mapped.1 != mapped2.1;
    stun_step(
        &emitter,
        "阶段2: 映射行为检测",
        &format!(
            "第二次绑定请求 | 映射1: {}:{} | 映射2: {}:{} → 端口是否变化: {} (判断是否为对称NAT)",
            mapped.0,
            mapped.1,
            mapped2.0,
            mapped2.1,
            if port_changed { "是" } else { "否" }
        ),
    );

    if is_cancelled(cmd_id) {
        emitter(StreamEvent {
            kind: "error".into(),
            data: "用户取消".into(),
        });
        return Err("Cancelled".into());
    }

    // Phase 3
    let socket3 = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("UDP bind3失败: {}", e))?;
    socket3.set_read_timeout(Some(Duration::from_secs(3))).ok();
    let tid3: [u8; 12] = rng.gen();
    socket3
        .send_to(&build_change_request(&tid3, true, true), server_addr)
        .map_err(|e| format!("STUN send3失败: {}", e))?;
    let mut buf3 = [0u8; 2048];
    let filtered = socket3.recv_from(&mut buf3).is_err();
    stun_step(
        &emitter,
        "阶段3: 过滤行为检测",
        &format!(
            "发送 Change-Request (变更IP+端口) → {} | 判断是否允许外部主机回包",
            if filtered {
                "无响应 → NAT/防火墙过滤了外部数据包"
            } else {
                "收到响应 → NAT 允许外部数据包通过"
            }
        ),
    );

    let nat_type = if mapped.0 == local_ip {
        NatType::Nat0
    } else if !port_changed && !filtered {
        NatType::Nat1
    } else if !port_changed && filtered {
        NatType::Nat2
    } else if port_changed && filtered {
        NatType::Nat4
    } else {
        NatType::Nat3
    };

    let result = NatDetectionResult {
        nat_type: nat_type.clone(),
        nat_label: nat_type.to_string(),
        public_ip: mapped.0.to_string(),
        public_port: mapped.1,
        local_ip: local_ip.to_string(),
        steps: vec![], // steps already emitted individually
        success: true,
        error_message: String::new(),
    };
    let json = serde_json::to_string(&result).unwrap_or_default();
    emitter(StreamEvent {
        kind: "summary".into(),
        data: json,
    });
    Ok(result)
}

// ============================================================================
// Helpers
// ============================================================================

fn resolve_stun(server: &str) -> Result<SocketAddr, String> {
    let (host, port) = if let Some((h, p)) = server.rsplit_once(':') {
        (h, p.parse::<u16>().unwrap_or(3478))
    } else {
        (server, 3478)
    };
    format!("{}:{}", host, port)
        .to_socket_addrs()
        .map_err(|e| format!("STUN DNS: {}", e))?
        .next()
        .ok_or_else(|| format!("No addr for {}", server))
}

fn get_local_ip() -> IpAddr {
    if let Ok(s) = UdpSocket::bind("0.0.0.0:0") {
        let _ = s.connect("8.8.8.8:53");
        if let Ok(addr) = s.local_addr() {
            return addr.ip();
        }
    }
    IpAddr::from([127, 0, 0, 1])
}

#[cfg(test)]
fn resolve_host(host: &str) -> Result<IpAddr, String> {
    resolve_host_with_version(host, "auto")
}

/// 解析目标地址：dns_server 为 Some(IP) 时使用指定 DNS 服务器查询（本机/预设/自定义）
/// dns_server 为 None / 空串 / "system" 时走系统默认解析
fn resolve_host_with_dns(
    host: &str,
    ip_version: &str,
    dns_server: Option<&str>,
) -> Result<IpAddr, String> {
    // 目标本身是 IP 时直接校验版本，无需 DNS
    if let Ok(ip) = host.parse::<IpAddr>() {
        match ip_version {
            "v4" if !ip.is_ipv4() => return Err("目标地址不是 IPv4 地址".to_string()),
            "v6" if !ip.is_ipv6() => return Err("目标地址不是 IPv6 地址".to_string()),
            _ => return Ok(ip),
        }
    }
    match dns_server {
        None | Some("") | Some("system") => resolve_host_with_version(host, ip_version),
        Some(srv) => {
            let server_ip: IpAddr = srv
                .parse()
                .map_err(|_| format!("DNS 服务器地址无效: {}", srv))?;
            use trust_dns_resolver::config::{
                NameServerConfig, Protocol, ResolverConfig, ResolverOpts,
            };
            use trust_dns_resolver::Resolver;

            let ns = NameServerConfig::new(SocketAddr::new(server_ip, 53), Protocol::Udp);
            let cfg = ResolverConfig::from_parts(None, Vec::new(), vec![ns]);
            // 调优：单次查询 2s 超时、不重试、不走 TCP 回退，
            // 避免 DNS 服务器不可达时 traceroute 启动卡顿数秒
            let mut opts = ResolverOpts::default();
            opts.timeout = Duration::from_secs(2);
            opts.attempts = 1;
            opts.try_tcp_on_error = false;
            let resolver =
                Resolver::new(cfg, opts).map_err(|e| format!("DNS 解析器初始化失败: {}", e))?;

            let mut ipv4: Option<IpAddr> = None;
            let mut ipv6: Option<IpAddr> = None;
            let mut v4_err: Option<String> = None;
            let mut v6_err: Option<String> = None;
            if ip_version != "v6" {
                match resolver.ipv4_lookup(host) {
                    Ok(lookup) => {
                        if let Some(a) = lookup.iter().next() {
                            ipv4 = Some(IpAddr::V4(a.0));
                        }
                    }
                    Err(e) => v4_err = Some(e.to_string()),
                }
            }
            if ip_version != "v4" {
                match resolver.ipv6_lookup(host) {
                    Ok(lookup) => {
                        if let Some(a) = lookup.iter().next() {
                            ipv6 = Some(IpAddr::V6(a.0));
                        }
                    }
                    Err(e) => v6_err = Some(e.to_string()),
                }
            }
            match ip_version {
                "v4" => ipv4.ok_or_else(|| {
                    v4_err
                        .map(|e| format!("通过 DNS {} 查询 {} 的 IPv4 记录失败: {}", srv, host, e))
                        .unwrap_or_else(|| {
                            format!("通过 DNS {} 查询 {} 的 IPv4 记录失败", srv, host)
                        })
                }),
                "v6" => ipv6.ok_or_else(|| {
                    v6_err
                        .map(|e| format!("通过 DNS {} 查询 {} 的 IPv6 记录失败: {}", srv, host, e))
                        .unwrap_or_else(|| {
                            format!("通过 DNS {} 查询 {} 的 IPv6 记录失败", srv, host)
                        })
                }),
                _ => ipv4.or(ipv6).ok_or_else(|| {
                    let detail = v4_err
                        .or(v6_err)
                        .map(|e| format!(": {}", e))
                        .unwrap_or_default();
                    format!("通过 DNS {} 查询 {} 失败{}", srv, host, detail)
                }),
            }
        }
    }
}

/// 系统默认解析：域名/IP 混合校验
fn resolve_host_with_version(host: &str, ip_version: &str) -> Result<IpAddr, String> {
    // If already an IP address, validate against requested version
    if let Ok(ip) = host.parse::<IpAddr>() {
        match ip_version {
            "v4" if !ip.is_ipv4() => return Err("目标地址不是 IPv4 地址".to_string()),
            "v6" if !ip.is_ipv6() => return Err("目标地址不是 IPv6 地址".to_string()),
            _ => return Ok(ip),
        }
    }
    // Resolve hostname
    let addrs: Vec<_> = format!("{}:0", host)
        .to_socket_addrs()
        .map_err(|e| format!("DNS 解析失败 {}: {}", host, e))?
        .collect();

    match ip_version {
        "v4" => addrs
            .iter()
            .find(|a| a.is_ipv4())
            .map(|a| a.ip())
            .ok_or_else(|| format!("未找到 {} 的 IPv4 地址", host)),
        "v6" => addrs
            .iter()
            .find(|a| a.is_ipv6())
            .map(|a| a.ip())
            .ok_or_else(|| format!("未找到 {} 的 IPv6 地址", host)),
        _ => Ok(addrs
            .first()
            .map(|a| a.ip())
            .ok_or_else(|| format!("No addr: {}", host))?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0001 D5 回归：ICMP Time Exceeded 报文长度恰好为 `IP 头 + 8` 时，
    /// 内嵌载荷为空切片；修复前在长度校验之前取 `inner[0]`，会 panic。
    ///
    /// 该形态可由同网段设备构造，panic 发生在 spawn_blocking 内，
    /// 会让整次 traceroute 以 "Traceroute panicked" 失败。
    #[test]
    fn icmp报文_空内嵌载荷不panic() {
        // 20 字节 IPv4 头（IHL=5）+ 8 字节 ICMP 头，总长 28，无内嵌报文
        let mut buf = vec![0u8; 28];
        buf[0] = 0x45; // version=4, IHL=5
        buf[12..16].copy_from_slice(&[192, 168, 1, 1]); // 源 IP
        buf[20] = 11; // ICMP Time Exceeded
        assert_eq!(
            parse_ipv4_icmp_reply(&buf),
            None,
            "空内嵌载荷应返回 None 而非 panic"
        );
    }

    /// 内嵌 IP 头存在但不足以容纳 echo 头（8 字节）时同样返回 None。
    #[test]
    fn icmp报文_内嵌载荷过短应返回空() {
        // 20 + 8 + 4（内嵌 4 字节，不足 IHL+8）
        let mut buf = vec![0u8; 32];
        buf[0] = 0x45;
        buf[20] = 11;
        assert_eq!(parse_ipv4_icmp_reply(&buf), None);
    }

    /// 正常 Echo Reply 仍能被解析（确认判空没有破坏主路径）。
    #[test]
    fn icmp报文_正常echo回复可解析() {
        let mut buf = vec![0u8; 28];
        buf[0] = 0x45;
        buf[12..16].copy_from_slice(&[10, 0, 0, 2]);
        buf[20] = 0; // Echo Reply
        buf[24..26].copy_from_slice(&0x1234u16.to_be_bytes()); // identifier
        buf[26..28].copy_from_slice(&7u16.to_be_bytes()); // sequence
        let parsed = parse_ipv4_icmp_reply(&buf).expect("应能解析 Echo Reply");
        assert_eq!(parsed.2, 0x1234);
        assert_eq!(parsed.3, 7);
        assert_eq!(parsed.4, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
    }

    #[test]
    fn test_resolve_localhost() {
        let addr = resolve_host("127.0.0.1").unwrap();
        assert_eq!(addr, IpAddr::from([127, 0, 0, 1]));
    }

    #[test]
    fn test_build_binding_request() {
        let tid = [0u8; 12];
        let pkt = build_binding_request(&tid);
        assert_eq!(pkt.len(), 20);
        assert_eq!(&pkt[0..2], &0x0001u16.to_be_bytes());
    }

    #[test]
    fn test_get_stun_servers() {
        let servers = get_stun_servers();
        assert!(servers.len() >= 15);
    }

    /// 首选服务器必须是实测可用项：stun.qq.com 已失效，不得再排第一
    /// （否则用户首次点「检测」即失败）。
    #[test]
    fn test_stun_first_choice_is_live_server() {
        let first = get_stun_servers().into_iter().next().expect("至少一台");
        assert_ne!(first, "stun.qq.com:3478", "失效服务器不得作为首选");
        assert_ne!(first, "stun.sipgate.net:3478", "失效服务器不得作为首选");
    }

    /// 候选列表：用户选择优先，其后补齐内置列表且去重（大小写不敏感）。
    #[test]
    fn test_stun_candidates_dedup_and_priority() {
        let c = stun_candidates("stun.l.google.com:19302");
        assert_eq!(
            c[0], "stun.l.google.com:19302",
            "用户选择必须排在首位（不重复追加）"
        );
        // 用户选择已在内置列表中 → 只应出现一次
        assert_eq!(
            c.iter()
                .filter(|x| x.as_str() == "stun.l.google.com:19302")
                .count(),
            1,
            "候选列表必须去重: {:?}",
            c
        );
        // 内置列表全部覆盖
        for s in get_stun_servers() {
            assert!(c.contains(&s), "候选列表缺少内置项 {}", s);
        }
        // 大小写不敏感去重：不同大小写视为同一项
        let c2 = stun_candidates("STUN.L.GOOGLE.COM:19302");
        assert_eq!(
            c2.iter()
                .filter(|x| x.to_ascii_lowercase() == "stun.l.google.com:19302")
                .count(),
            1,
            "大小写不敏感去重失败: {:?}",
            c2
        );
        // 空/空白选择 → 仅内置列表，且与 get_stun_servers() 完全一致
        assert_eq!(stun_candidates(""), get_stun_servers());
        assert_eq!(stun_candidates("   "), get_stun_servers());
    }
}
