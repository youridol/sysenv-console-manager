/// 网络流量统计模块
///
/// 采集活动网卡的:
/// - 上下行实时速度 (bytes/sec)
/// - 上下行总量（程序启动后会话累计，关闭自动清零）
/// - 活跃 TCP 连接数
///
/// 速度通过前后两次采样（网卡累计字节）的差值除以时间间隔计算。
/// 注意 sysinfo 0.30 语义：`total_received()`/`total_transmitted()` 是网卡累计字节，
/// `received()`/`transmitted()` 仅是自上次 refresh 的增量（曾误用导致速率/总量颠倒）。
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

// ============================================================================
// 类型定义
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct NetStatsData {
    /// 活动网卡列表（按总流量降序）
    pub interfaces: Vec<NetInterfaceStats>,
    /// 总下载速度 (bytes/sec)
    pub total_download_speed: f64,
    /// 总上传速度 (bytes/sec)
    pub total_upload_speed: f64,
    /// 总下载量 (bytes)
    pub total_download: u64,
    /// 总上传量 (bytes)
    pub total_upload: u64,
    /// 活跃 TCP 连接数
    pub active_connections: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetInterfaceStats {
    /// 网卡名称
    pub name: String,
    /// 下载速度 (bytes/sec)
    pub download_speed: f64,
    /// 上传速度 (bytes/sec)
    pub upload_speed: f64,
    /// 累计下载量 (bytes)
    pub total_download: u64,
    /// 累计上传量 (bytes)
    pub total_upload: u64,
}

// ============================================================================
// 采样状态
// ============================================================================

struct Snapshot {
    time: Instant,
    interfaces: HashMap<String, (u64, u64)>, // (total_rx, total_tx)
}

static PREV_SNAPSHOT: std::sync::LazyLock<Mutex<Option<Snapshot>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

/// 会话流量基线：网卡名 -> (基线 rx, 基线 tx)。
/// 程序启动后首次见到网卡时记录其累计值，之后返回相对增量；进程退出自动清除（不持久化）。
static SESSION_BASELINE: std::sync::LazyLock<Mutex<HashMap<String, (u64, u64)>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// 持久化 Networks 实例 + 调用计数（每 120 次调用重建，防止 refresh 内部累积）
/// sysinfo v0.30 的 Networks::refresh() 在 Windows 上可能追加而非替换旧条目
static NETWORKS: std::sync::LazyLock<Mutex<(sysinfo::Networks, u32)>> =
    std::sync::LazyLock::new(|| Mutex::new((sysinfo::Networks::new_with_refreshed_list(), 0)));

/// Networks 实例重建间隔（调用次数），防止 refresh 内部条目累积
const NETWORKS_REBUILD_INTERVAL: u32 = 120;

// ============================================================================
// 主入口
// ============================================================================

/// 采集网络流量统计（含速度计算）
pub fn get_net_stats() -> NetStatsData {
    let now = Instant::now();

    // 复用持久化实例 + 刷新数据（refresh 更新计数器，refresh_list 仅更新列表）
    let mut guard = NETWORKS.lock().unwrap_or_else(|p| p.into_inner());
    let (networks, call_count) = &mut *guard;
    *call_count += 1;

    // 周期性重建 Networks 实例，防止 refresh 内部条目累积
    if *call_count >= NETWORKS_REBUILD_INTERVAL {
        *networks = sysinfo::Networks::new_with_refreshed_list();
        *call_count = 0;
    } else {
        networks.refresh();
    }

    // 收集非回环网卡数据（复制完成后即可释放锁）
    let mut current_ifaces: Vec<(String, u64, u64)> = Vec::new();
    for (name, data) in networks.list() {
        if name.to_lowercase().contains("loopback") {
            continue;
        }
        // sysinfo 0.30：total_* 为网卡累计字节（received()/transmitted() 仅是 refresh 周期增量）
        let total_rx = data.total_received();
        let total_tx = data.total_transmitted();
        current_ifaces.push((name.clone(), total_rx, total_tx));
    }
    drop(guard); // 释放 Networks 锁

    // 会话累计：首次见到网卡时以累计值为基线，之后只累计增量（关闭程序自动清零）
    let mut baseline = SESSION_BASELINE.lock().unwrap_or_else(|p| p.into_inner());
    let session_ifaces: Vec<(String, u64, u64)> = current_ifaces
        .iter()
        .map(|(name, rx, tx)| {
            let entry = baseline.entry(name.clone()).or_insert((*rx, *tx));
            (
                name.clone(),
                rx.saturating_sub(entry.0),
                tx.saturating_sub(entry.1),
            )
        })
        .collect();
    drop(baseline);

    // 按会话总流量降序
    let mut sorted_ifaces = session_ifaces;
    sorted_ifaces.sort_by(|a, b| {
        (b.1 + b.2)
            .partial_cmp(&(a.1 + a.2))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // 计算总下载/上传量（会话累计）
    let total_download: u64 = sorted_ifaces.iter().map(|(_, rx, _)| *rx).sum();
    let total_upload: u64 = sorted_ifaces.iter().map(|(_, _, tx)| *tx).sum();

    // 获取上一次快照，计算速度
    let mut prev_guard = PREV_SNAPSHOT.lock().unwrap_or_else(|p| p.into_inner());
    let prev_snap = prev_guard.take();

    let (total_download_speed, total_upload_speed, per_iface_speeds) =
        compute_speeds(&current_ifaces, &prev_snap, now);

    // 保存当前快照
    *prev_guard = Some(Snapshot {
        time: now,
        interfaces: current_ifaces
            .iter()
            .map(|(n, rx, tx)| (n.clone(), (*rx, *tx)))
            .collect(),
    });
    drop(prev_guard);

    // 组装接口数据（会话累计量）
    let interfaces: Vec<NetInterfaceStats> = sorted_ifaces
        .into_iter()
        .map(|(name, rx, tx)| {
            let (dl, ul) = per_iface_speeds.get(&name).copied().unwrap_or((0.0, 0.0));
            NetInterfaceStats {
                name,
                download_speed: dl,
                upload_speed: ul,
                total_download: rx,
                total_upload: tx,
            }
        })
        .collect();

    let active_connections = get_tcp_connections();

    NetStatsData {
        interfaces,
        total_download_speed,
        total_upload_speed,
        total_download,
        total_upload,
        active_connections,
    }
}

/// 基于前后两次快照计算速度
///
/// 返回 (总下载速度, 总上传速度, 每个网卡的速度映射)
fn compute_speeds(
    current: &[(String, u64, u64)],
    prev: &Option<Snapshot>,
    now: Instant,
) -> (f64, f64, HashMap<String, (f64, f64)>) {
    let prev_snap = match prev {
        Some(s) => s,
        None => return (0.0, 0.0, HashMap::new()),
    };

    let elapsed = now.duration_since(prev_snap.time).as_secs_f64();
    // 间隔过长（>15s）视为无效采样
    if elapsed <= 0.0 || elapsed > 15.0 {
        return (0.0, 0.0, HashMap::new());
    }

    let mut total_dl: f64 = 0.0;
    let mut total_ul: f64 = 0.0;
    let mut per_iface: HashMap<String, (f64, f64)> = HashMap::new();

    for (name, rx, tx) in current {
        let (dl, ul) = if let Some((prev_rx, prev_tx)) = prev_snap.interfaces.get(name) {
            let d = if *rx >= *prev_rx {
                (*rx - *prev_rx) as f64 / elapsed
            } else {
                0.0
            };
            let u = if *tx >= *prev_tx {
                (*tx - *prev_tx) as f64 / elapsed
            } else {
                0.0
            };
            (d, u)
        } else {
            (0.0, 0.0)
        };
        total_dl += dl;
        total_ul += ul;
        per_iface.insert(name.clone(), (dl, ul));
    }

    (total_dl, total_ul, per_iface)
}

// ============================================================================
// TCP 连接数 — iphlpapi::GetTcpStatistics
// ============================================================================

/// MIB_TCPSTATS 结构体（Windows IPv4 TCP 统计）
#[repr(C)]
struct MibTcpStats {
    _rto_algorithm: u32,
    _rto_min: u32,
    _rto_max: u32,
    _max_conn: u32,
    _active_opens: u32,
    _passive_opens: u32,
    _attempt_fails: u32,
    _estab_resets: u32,
    curr_estab: u32, // 当前已建立 (ESTABLISHED / CLOSE-WAIT) 的连接数
    _in_segs: u32,
    _out_segs: u32,
    _retrans_segs: u32,
    _in_errs: u32,
    _out_rsts: u32,
    _num_conns: u32,
}

/// 通过 iphlpapi.dll 的 GetTcpStatistics 获取当前活跃 TCP 连接数
fn get_tcp_connections() -> u32 {
    let mut stats = std::mem::MaybeUninit::<MibTcpStats>::zeroed();

    // SAFETY: GetTcpStatistics 是 iphlpapi.dll 的标准导出函数
    let result = unsafe { GetTcpStatistics(stats.as_mut_ptr()) };

    if result != 0 {
        return 0;
    }

    unsafe { stats.assume_init().curr_estab }
}

#[cfg(windows)]
extern "system" {
    fn GetTcpStatistics(p_stats: *mut MibTcpStats) -> u32;
}

// ============================================================================
// 链路协商速度 — PowerShell Get-NetAdapter（准静态数据，勿入 1s 轮询热路径）
// ============================================================================

/// 读取各网卡的链路协商速度（接口别名 → 速度字符串，如 "1 Gbps"）（legacy 旧实现）
///
/// 接口别名与 sysinfo 网卡名同源（GetIfTable2 的 Alias），前端按键名直接匹配。
/// 中文别名（如"以太网"）依赖 proc_util::run_ps 的 UTF-8 强制输出避免乱码。
/// 失败或无网卡时返回空表（前端据此隐藏速度显示，优雅降级）。
fn get_link_speeds_legacy() -> HashMap<String, String> {
    // @() 强制数组输出：单网卡时 ConvertTo-Json 默认返回对象而非数组
    let out = crate::proc_util::run_ps(
        "@(Get-NetAdapter -ErrorAction SilentlyContinue | Select-Object Name, LinkSpeed) | ConvertTo-Json -Compress",
    );
    // 防御性去除可能的 BOM（PS 5.1 某些输出路径会带 U+FEFF）
    let text = out.trim_start_matches('\u{feff}');
    let parsed: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return HashMap::new(),
    };
    let mut map = HashMap::new();
    if let Some(arr) = parsed.as_array() {
        for item in arr {
            let name = item.get("Name").and_then(|v| v.as_str()).unwrap_or("");
            let speed = item.get("LinkSpeed").and_then(|v| v.as_str()).unwrap_or("");
            // 跳过未建立链路的网卡（Get-NetAdapter 返回 "0 bps" / "Unknown"）
            let is_disconnected =
                speed.is_empty() || speed == "0 bps" || speed.eq_ignore_ascii_case("unknown");
            if !name.is_empty() && !is_disconnected {
                map.insert(name.to_string(), speed.to_string());
            }
        }
    }
    map
}

/// 读取各网卡的链路协商速度（P3，接口别名 → 速度字符串，如 "1 Gbps"）
///
/// 过渡期由 `legacy-data-source` feature 控制回退旧 PowerShell 实现；
/// 默认走纯 Rust 驱动（secm-datasource::netif，GetIfTable2，与 Get-NetAdapter 同源）。
/// 失败或无网卡时返回空表（前端据此隐藏速度显示，优雅降级），并打 R8 日志。
pub fn get_link_speeds() -> HashMap<String, String> {
    if crate::ds_util::use_legacy() {
        return get_link_speeds_legacy();
    }

    match crate::datasource::netif::link_speeds() {
        Ok(map) => map,
        Err(e) => {
            let msg = crate::ds_util::log_err("net_stats", "获取链路速度", &e);
            crate::debug_warn!("net_stats", "链路速度降级为空表: {}", msg);
            HashMap::new()
        }
    }
}

#[cfg(test)]
mod manual_tests {
    use super::*;
    #[test]
    fn dump_net_stats() {
        let a = get_net_stats();
        std::thread::sleep(std::time::Duration::from_secs(3));
        let b = get_net_stats();
        println!(
            "A: dl_spd={:.0} ul_spd={:.0} dl_total={} ul_total={} ifaces={}",
            a.total_download_speed,
            a.total_upload_speed,
            a.total_download,
            a.total_upload,
            a.interfaces.len()
        );
        for i in &a.interfaces {
            println!(
                "  iface {}: spd={:.0}/{} total={}/{}",
                i.name, i.download_speed, i.upload_speed, i.total_download, i.total_upload
            );
        }
        println!(
            "B(3s后): dl_spd={:.0} ul_spd={:.0} dl_total={} ul_total={}",
            b.total_download_speed, b.total_upload_speed, b.total_download, b.total_upload
        );
        // 会话累计量首次调用基线=当前值（差值 0），故不断言 total > 0；
        // 断言接口存在且速率计算为有限值（无 NaN）即可
        assert!(!a.interfaces.is_empty(), "应至少有一个非回环网络接口");
        assert!(a.total_download_speed.is_finite(), "下载速率应为有限值");
        assert!(b.total_download_speed.is_finite(), "3s 后速率应为有限值");
    }
}
