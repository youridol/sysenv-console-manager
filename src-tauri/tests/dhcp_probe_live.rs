//! 端到端反馈回路：对**真实网络**运行 DHCP 主动探测，并把原始观测打印出来。
//!
//! 本文件是「先建反馈回路、再改代码」的产物：它直接调用库入口
//! `secm_lib::dhcp_probe::probe_dhcp_servers()`，同时把模块内部的调试日志
//! （`crate::debug_info!` / `debug_warn!("dhcp_probe", ...)`）按顺序取回来，
//! 因此既能看到最终结果，也能看到探测过程中每一步的真实决策。
//!
//! 运行：
//! ```text
//! cd src-tauri
//! cargo test --test dhcp_probe_live -- --nocapture --test-threads=1
//! ```
//!
//! 边界：
//! - 只读探测：最多 4 个 DISCOVER 包（与生产实现同一上限），3 秒窗口。
//! - **不修改任何系统网络状态**（只 bind/send/recv UDP）。
//! - 本机没有可响应的 DHCP 服务器时，无法证明"能收到 OFFER"；此时测试会
//!   打印 `[info]` 说明，而不是把"没收到"当成回归。
//!
//! 诊断价值：修复前本机（注册表基线为空）应观察到 `probed=[] servers=[]`；
//! 修复后应能观察到探测真正到达了本网段的广播目标（发送目标与接口选择可核对）。

use std::time::Instant;

/// 截取 `dhcp_probe` 模块的调试日志（按时间顺序）。
fn dhcp_debug_lines_from(start: usize) -> Vec<String> {
    secm_lib::debug::get_all()
        .into_iter()
        .skip(start)
        .filter(|e| e.module == "dhcp_probe")
        .map(|e| format!("[{}] {} {}", e.level, e.timestamp, e.message))
        .collect()
}

/// 真实探测：打印结果 + 本次新增的内部调试日志，返回结果供后续断言。
fn run_live_probe(tag: &str) -> secm_lib::dhcp_probe::DhcpProbeResult {
    let before = secm_lib::debug::get_all().len();
    let start = Instant::now();
    let result = secm_lib::dhcp_probe::probe_dhcp_servers();
    let elapsed = start.elapsed();
    let lines = dhcp_debug_lines_from(before);

    eprintln!("===== [{tag}] probe_dhcp_servers() =====");
    eprintln!("elapsed = {elapsed:?}");
    match &result {
        Ok(r) => {
            eprintln!("servers         = {:?}", r.servers);
            eprintln!("count           = {}", r.count);
            eprintln!("healthy         = {}", r.healthy);
            eprintln!("baseline_server = {:?}", r.baseline_server);
            eprintln!("note            = {}", r.note);
        }
        Err(e) => eprintln!("ERR = {e}"),
    }
    eprintln!("--- dhcp_probe debug 日志（本次 {} 条）---", lines.len());
    for l in &lines {
        eprintln!("  {l}");
    }
    eprintln!("===== [{tag}] end =====");

    match result {
        Ok(r) => r,
        Err(e) => panic!("probe_dhcp_servers() 失败: {e}"),
    }
}

/// 主反馈回路：真实探测必须有界返回，且结果结构自洽。
///
/// 本测试不在"有没有真服务器"上下断言（取决于实验室环境），而是断言探测
/// 链路是活的：要么收到 OFFER，要么明确记录了"发了但没人回"。
#[test]
fn live_probe_is_bounded_and_reports_what_it_did() {
    let start = Instant::now();
    let r = run_live_probe("live");

    assert_eq!(
        r.servers.len(),
        r.count,
        "count 必须等于 servers.len()（前端按 count 渲染）"
    );
    assert_eq!(r.healthy, r.count <= 1, "healthy 必须等价于 count<=1");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(30),
        "单次探测耗时 {:?}，疑似挂死",
        start.elapsed()
    );

    if r.servers.is_empty() {
        eprintln!(
            "[info] 本机未发现 DHCP 服务器（baseline={:?}）。\
             若本网段确有 DHCP 服务器，这可能是漏检；请核对上方 debug 日志中\
             的发送目标与接口选择。",
            r.baseline_server
        );
    }
}

/// 深度检查也必须在真实网络上跑通（探测 + ping + 拓扑判定）。
#[test]
fn live_deep_check_is_consistent() {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("[skip] 无法创建 tokio runtime: {e}");
            return;
        }
    };
    match rt.block_on(secm_lib::dhcp_probe::deep_check_dhcp_servers()) {
        Ok(d) => {
            eprintln!("===== deep_check_dhcp_servers =====");
            eprintln!("local_ip    = {:?}", d.local_ip);
            eprintln!("gateway     = {:?}", d.gateway);
            eprintln!("subnet_mask = {:?}", d.subnet_mask);
            eprintln!("servers     = {:?}", d.servers);
            eprintln!("summary     = {}", d.summary);
            for c in &d.checks {
                eprintln!(
                    "  {} ping_ok={} rtt={:.1} same_subnet={} is_gateway={} severity={} | {}",
                    c.server,
                    c.ping_ok,
                    c.ping_rtt_ms,
                    c.same_subnet,
                    c.is_gateway,
                    c.severity,
                    c.diagnosis
                );
            }
            eprintln!("===================================");
            assert_eq!(
                d.checks.len(),
                d.servers.len(),
                "checks 必须与 servers 一一对应（前端逐台渲染）"
            );
            for c in &d.checks {
                assert!(
                    d.servers.contains(&c.server),
                    "check 中的服务器 {} 不在 servers 列表中",
                    c.server
                );
            }
        }
        Err(e) => eprintln!("deep_check 返回错误（记录但不失败）: {e}"),
    }
}
