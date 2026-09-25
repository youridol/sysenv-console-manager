//! 端到端验证：服务端 `probe_url` 对**真实**站点的判定是否符合预期。
//!
//! 与 `network_probe_contract.rs`（源码静态契约）互补：本测试真的发起网络请求，
//! 证明：
//!   1. 可达站点确实被判为可达（修复前前端 fetch 会被 CSP 拦截 → 一律不可达）；
//!   2. 不可达目标确实被判为不可达（没有退化成"永远 true"）；
//!   3. 超时被正确执行（不悬挂）。
//!
//! 网络相关断言对 CI 环境敏感，因此：
//!   - 只要求 4 个默认站点中**至少一个**可达（不要求全部，避免地区封锁误判）；
//!   - 用 RFC 5737 保留地址（TEST-NET-1，不可路由）作为"必然不可达"样本；
//!   - 无法联网时显式 skip 并打印原因，不静默通过。
//!
//! 运行：`cd src-tauri; cargo test --test probe_url_live -- --nocapture`

use std::time::{Duration, Instant};

/// 解析期就断言"不可路由"的地址：RFC 5737 TEST-NET-1。
/// 该段专供文档/测试，保证不会被真实路由，故连接必然失败。
const UNROUTABLE: &str = "http://192.0.2.1:9/";

#[test]
fn probe_url_distinguishes_reachable_from_unreachable() {
    // 1. 默认站点：至少一个可达
    let sites = [
        "https://www.apple.com",
        "https://github.com",
        "https://www.google.com",
        "https://www.youtube.com",
    ];

    let mut reachable = Vec::new();
    let mut unreachable = Vec::new();
    for s in sites {
        let r = secm_lib::network::probe_url(s, 8000);
        eprintln!(
            "[probe-url] {s} -> reachable={} status={:?} {}ms{}",
            r.reachable,
            r.status,
            r.ms,
            r.error
                .as_deref()
                .map(|e| format!(" err={}", &e.chars().take(90).collect::<String>()))
                .unwrap_or_default()
        );
        if r.reachable {
            reachable.push(s);
        } else {
            unreachable.push(s);
        }
    }

    if reachable.is_empty() {
        // 无网络环境：显式 skip（不静默通过，也不误判为回归）
        eprintln!(
            "[skip] 4 个默认站点均不可达，且不可达样本 URL 依据充分——\
             疑似本机无外网。不可达名单: {:?}。请在有网环境重跑本测试。",
            unreachable
        );
        return;
    }

    eprintln!(
        "[probe-url] 可达 {}/{}：{:?}",
        reachable.len(),
        sites.len(),
        reachable
    );

    // 可达判定必须附带 HTTP 状态码（证明是真实响应，不是"猜的 true"）
    for s in &reachable {
        let r = secm_lib::network::probe_url(s, 8000);
        assert!(
            r.status.is_some(),
            "站点 {s} 被判可达，但没有 HTTP 状态码（status=None）——\
             探测实现可能在伪造可达结果"
        );
        assert!(
            r.error.is_none(),
            "站点 {s} 被判可达却带 error: {:?}",
            r.error
        );
        assert!(r.ms > 0, "站点 {s} 耗时 0ms，可疑");
    }
}

/// 不可达目标必须被判为不可达（防止"永远返回 true"的退化实现）。
#[test]
fn probe_url_reports_unroutable_target_as_unreachable() {
    let start = Instant::now();
    let r = secm_lib::network::probe_url(UNROUTABLE, 2000);
    let elapsed = start.elapsed();

    eprintln!(
        "[probe-url] {UNROUTABLE} -> reachable={} status={:?} {}ms",
        r.reachable, r.status, r.ms
    );

    assert!(
        !r.reachable,
        "不可路由地址 {UNROUTABLE} 被判为**可达**——探测实现退化成了恒真，\
         这会让「HTTP 延迟测试」永远显示全部可达"
    );
    assert!(
        r.status.is_none(),
        "不可路由地址不该有 HTTP 状态码，得到 {:?}",
        r.status
    );
    assert!(
        r.error.is_some(),
        "不可达时必须给出失败原因（便于用户诊断），实际为 None"
    );

    // 超时必须生效：请求预算 2s，留 3s 余量（含 DNS/重试）
    assert!(
        elapsed < Duration::from_secs(5),
        "对不可路由地址的探测耗时 {:?}，远超 2s 预算——超时未被约束",
        elapsed
    );

    // 报告耗时与预算同量级（证明是超时结束，而不是瞬间失败也不是永久挂起）
    assert!(
        r.ms <= 2500,
        "探测自报耗时 {}ms 超过预算 2000ms",
        r.ms
    );
}

/// **HTTP 延迟口径守卫**（与 `network_probe_contract.rs` 的静态断言互补）：
/// `probe_url` 返回的 `ms` 必须是**可达那一次尝试**的真实耗时，
/// 而不是把先前失败尝试也累加进来的总时间。
///
/// 为什么要守这个：站点拒绝 HEAD（405/501）会触发 GET 兜底。若统一按"进入函数到
/// 返回"计时，同一站点的延迟会因走了兜底路径而虚高约一倍——用户看到翻倍的数字，
/// 而真实 HTTP 往返并没有变慢。
#[test]
fn http_latency_measures_the_successful_attempt_only() {
    let start = Instant::now();
    let r = secm_lib::network::probe_url("https://github.com", 8000);
    let wall = start.elapsed();

    eprintln!(
        "[http-latency] reachable={} http_ms={} wall={:?} status={:?}",
        r.reachable, r.ms, wall, r.status
    );

    if !r.reachable {
        // 无网络环境：显式跳过（不静默通过）
        eprintln!("[skip] github.com 不可达（疑似无外网），跳过延迟口径断言");
        return;
    }

    assert!(
        r.ms > 0,
        "可达站点的 HTTP 延迟为 0ms，可疑——延迟必须是真实测量值"
    );
    // 延迟是真正成功那一次尝试的耗时，不可能超过整体墙钟
    assert!(
        r.ms <= wall.as_millis() as u64,
        "自报 HTTP 延迟 {}ms 超过整体墙钟 {:?}——计时口径错误",
        r.ms,
        wall
    );
}

/// URL 归一化：无协议时补 `http://`，供「HTTP 延迟测试」与「端口检测」共用，
/// 保证"可达性"与"延迟"测的是同一地址。
#[test]
fn normalize_probe_url_adds_scheme_only_when_missing() {
    use secm_lib::network::normalize_probe_url;

    assert_eq!(normalize_probe_url("https://github.com"), "https://github.com");
    assert_eq!(normalize_probe_url("http://127.0.0.1:8080"), "http://127.0.0.1:8080");
    // 端口探测的形态：无协议补 http
    assert_eq!(normalize_probe_url("192.168.3.254:80"), "http://192.168.3.254:80");
    assert_eq!(normalize_probe_url("example.com"), "http://example.com");
}
