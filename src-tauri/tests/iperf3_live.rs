//! iperf3 真实端到端测试（live）。
//!
//! ## 这个测试证明什么
//!
//! `iperf3_contract.rs` 用**实测捕获的 JSON 片段**验证解析逻辑；本文件则跑**真实
//! 进程**：自起 iperf3 服务端 → 跑真实客户端测试 → 断言结构化结果。
//! 两者互补：前者快且确定，后者能发现"解析器与真实输出不一致"这类致命偏差。
//!
//! ## 无二进制时优雅跳过
//!
//! CI / 未执行 `npm run stage:iperf3` 的环境没有随包二进制。
//! 此时**跳过而非失败**（打印说明），避免把"缺测试素材"误报成"功能坏了"。
//! 一旦二进制存在，所有断言都必须真实通过。
//!
//! ## 运行
//!
//! ```text
//! cd src-tauri && cargo test --test iperf3_live -- --nocapture
//! ```
//!
//! **测试名一律中文**（项目约定）。中文名里夹带的协议缩写（UDP、TCP）会触发
//! rustc 的 `non_snake_case` 启发式告警——纯风格噪音，与正确性无关，故模块级豁免。

#![allow(non_snake_case)]

use secm_lib::iperf::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// 全局串行锁。
///
/// **为什么必须串行**：`iperf::server_start/stop/status` 背后是**单一全局服务端槽**
/// （`SERVER` 静态量）——这是生产语义（一个 SECK 实例只应托管一个本机服务端）。
/// 但测试默认并行执行，两个测试同时读写该槽会互相踩：A 的 `Drop` 会停掉 B 正在用的
/// 服务端，使 B 永远等不到端口就绪（表现为测试挂死）。
/// 故本文件的测试统一串行；生产代码无需改动。
fn live_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// 定位随包 iperf3（与 `resolve_iperf3_exe` 同样的候选顺序，但独立探测以便给出清晰提示）。
fn find_bundled() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        manifest.join("resources").join("iperf3").join("iperf3.exe"),
        manifest
            .join("..")
            .join("third_party")
            .join("iperf3")
            .join("iperf3.exe"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

/// 分配一个**确实空闲**的端口。
///
/// 不能简单用「固定基数 + 序号」：上一次测试运行结束时连接处于 `TIME_WAIT`，
/// 端口在若干十秒内仍不可绑定（Windows 上实测表现为 os error 10048
/// "通常每个套接字地址只允许使用一次"）。那会让**本次运行完全正常**的用例
/// 偶发失败——典型的假阳性，会掩盖真实回归。
///
/// 做法：在候选区间内逐个尝试 `TcpListener::bind`，绑定成功即释放并采用该端口。
/// 与 iperf3 `server_start` 内部的 `precheck_port` 判据一致（都是实际绑定），
/// 因此不会选出"预检通过但真实启动失败"的端口。
static PORT_SEQ: AtomicUsize = AtomicUsize::new(0);
fn next_port() -> u16 {
    // 5600 起递增，避开 5201 默认端口与其他测试；跨多次运行错开起始点。
    let base = 5600 + (PORT_SEQ.fetch_add(1, Ordering::SeqCst) as u16) % 200;
    for offset in 0..400u16 {
        let port = 5600 + (base - 5600 + offset) % 400;
        if std::net::TcpListener::bind(("0.0.0.0", port)).is_ok() {
            return port;
        }
    }
    panic!("在 5600-5999 区间内找不到空闲端口（测试环境异常）");
}

/// 探测到 iperf3 则返回 (exe, 版本探测)；否则打印跳过原因并返回 None。
fn setup(test_name: &str) -> Option<(PathBuf, Iperf3VersionProbe)> {
    let Some(exe) = find_bundled() else {
        eprintln!(
            "[iperf3_live] 跳过 {test_name}：未找到随包 iperf3。\
             请先执行 `npm run stage:iperf3`（或放入 third_party/iperf3/iperf3.exe）。"
        );
        return None;
    };
    // 直接对探测到的 exe 取版本，避免依赖进程级缓存
    let probe = version_of(&exe);
    assert!(
        probe.version.is_known(),
        "随包 iperf3 应能报出版本号，实际原始输出：{:?}",
        probe.raw
    );
    eprintln!(
        "[iperf3_live] {} 使用 {}（iperf3 {}）",
        test_name,
        exe.display(),
        probe.version.label()
    );
    if let Some(reason) = tcp_stream_unavailable(&exe) {
        eprintln!(
            "[iperf3_live] 跳过 {test_name}：本环境下 iperf3 无法建立数据流（{reason}）。\
             实测：同一 iperf3.exe 复制到无低完整性标签的目录后 TCP 测试正常（约 4 Gbit/s），\
             从本仓库目录（带 Mandatory Label\\Low Mandatory Level）运行则报 \
             \"unable to create a new stream: Permission denied\"。\
             属环境属性而非功能缺陷；请在普通（非受限令牌）终端重跑以覆盖此用例。"
        );
        return None;
    }
    Some((exe, probe))
}

/// 探测「本环境下 iperf3 能否真实建立数据流」。
///
/// 背景（2026-09-23 实测）：本仓库工作区目录带 `Mandatory Label\Low Mandatory Level`
/// 标签，低完整性进程运行的 iperf3 能绑定监听端口，但客户端建立数据流被拒
/// （`unable to create a new stream: Permission denied`）。同一二进制复制到无该标签的
/// 目录后测试完全正常，故这是**运行环境属性**，不是解析器或参数编排缺陷。
///
/// 这里做一次最小自检（本地起服务端 → 客户端连一次），仅在**明确**观测到该权限拒绝时
/// 返回原因，使用例显式跳过；其它失败（如端口占用）不跳过，仍按真实失败处理。
///
/// 实现要点：服务端**不加 `-1`**。`-1` 表示「接受一次连接后退出」，若用它做就绪探测
/// （先连一次确认监听），该连接会被服务端消耗掉并退出，随后真实客户端只会看到
/// 「Connection refused」，反而漏掉权限拒绝这一目标信号。就绪判定改用固定短等待。
/// 结果进程内缓存一次，避免每个用例各起一轮服务端。
fn tcp_stream_unavailable(exe: &std::path::Path) -> Option<String> {
    static CACHED: OnceLock<Option<String>> = OnceLock::new();
    CACHED
        .get_or_init(|| probe_tcp_stream(exe))
        .clone()
}

fn probe_tcp_stream(exe: &std::path::Path) -> Option<String> {
    let port = next_port();
    let mut server = std::process::Command::new(exe)
        .args(["-s", "-p", &port.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    // 固定短等待：iperf3 起监听通常 <300ms；此处不做「连接探测」以免消耗连接
    std::thread::sleep(std::time::Duration::from_millis(600));

    let out = std::process::Command::new(exe)
        .args(["-c", "127.0.0.1", "-p", &port.to_string(), "-t", "1"])
        .output()
        .ok();
    let _ = server.kill();
    let _ = server.wait();

    let out = out?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if text.contains("unable to create a new stream") && text.contains("Permission denied") {
        return Some("unable to create a new stream: Permission denied".to_string());
    }
    None
}

/// 通过公开 API 之外的路径取版本：临时注入覆盖再查询，然后复位。
fn version_of(exe: &std::path::Path) -> Iperf3VersionProbe {
    set_exe_override(Some(exe.to_path_buf()));
    invalidate_cache();
    let info = tool_info(None);
    let probe = Iperf3VersionProbe {
        version: Iperf3Version {
            major: info.major,
            minor: info.minor,
            patch: info.patch,
        },
        raw: info.version_raw.clone(),
    };
    probe
}

/// 直接读二进制版本（不经过缓存/覆盖），供断言使用。
fn probe_binary(exe: &std::path::Path) -> Iperf3VersionProbe {
    let out = std::process::Command::new(exe)
        .arg("--version")
        .output()
        .expect("应能执行 iperf3 --version");
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    parse_version(&text)
}

/// 启动服务端并等待其监听就绪（轮询端口），返回清理守卫。
struct ServerGuard {
    port: u16,
}

impl ServerGuard {
    fn start(exe: &std::path::Path, port: u16) -> Self {
        let status = server_start(exe, port, false).expect("服务端应能启动");
        assert!(status.running, "启动后应处于运行态");
        assert_eq!(status.port, Some(port));
        assert!(status.pid.is_some(), "应返回服务端 PID");

        // 等待端口真正可连接（iperf3 起监听需要几十毫秒）
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            if std::time::Instant::now() > deadline {
                panic!("服务端在 10s 内未开始监听端口 {port}");
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        ServerGuard { port }
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        // 无论断言是否失败都要停掉，避免留下孤儿进程占用端口
        let _ = server_stop();
        let _ = self.port;
    }
}

/// 构造一个短时 TCP 客户端测试参数（时长 1 秒，本地环回，快且稳）。
fn short_tcp(port: u16) -> Iperf3ClientOptions {
    Iperf3ClientOptions {
        host: "127.0.0.1".to_string(),
        port,
        time: 1,
        interval: 1,
        ..Default::default()
    }
}

// ============================================================================
// 端到端：TCP 正向
// ============================================================================

#[test]
fn 实测_TCP正向产出结构化结果() {
    let _serial = live_lock();
    let Some((exe, _)) = setup("live_tcp_forward") else {
        return;
    };
    let port = next_port();
    let _guard = ServerGuard::start(&exe, port);
    let probe = probe_binary(&exe);

    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&events);

    let result = run_client_streaming(
        &short_tcp(port),
        &exe,
        &probe,
        "live-tcp-forward",
        move |ev| sink.lock().unwrap().push(ev),
    )
    .expect("TCP 正向测试应成功");

    // 结构化摘要必须有真实数值（旧实现返回空串，这里必须不是空的）
    assert!(!result.cancelled, "正常完成不应标记为取消");
    assert_eq!(result.protocol, "TCP");
    assert_eq!(result.target, "127.0.0.1");
    assert_eq!(result.port, port); // 端口应原样回填
    let mbps = result
        .sent
        .mbps
        .expect("发送侧吞吐必须有值（旧实现此处无数据）");
    assert!(mbps > 0.0, "环回吞吐应大于 0，实际 {} Mbps", mbps);
    assert!(result.sent.bytes.unwrap_or(0) > 0, "发送字节数应大于 0");

    // 实时区间：1 秒测试至少 1 个区间
    assert!(
        !result.intervals.is_empty(),
        "应产出实时区间（--json-stream 的核心价值）"
    );
    for row in &result.intervals {
        assert!(row.end_s > row.start_s, "区间结束时间应大于开始时间");
        assert!(row.mbps.unwrap_or(0.0) > 0.0, "区间吞吐应大于 0");
    }

    // 前端应收到 iperf-interval 事件（实时逐行渲染的数据来源）
    let evs = events.lock().unwrap();
    assert!(
        evs.iter().any(|e| e.kind == "iperf-interval"),
        "应发出 iperf-interval 事件，实际事件类型：{:?}",
        evs.iter().map(|e| e.kind.clone()).collect::<Vec<_>>()
    );
    assert!(evs.iter().any(|e| e.kind == "info"), "应透出 start 提示");

    eprintln!(
        "[iperf3_live] TCP 正向：{} Mbps / {} 字节 / {} 个区间",
        mbps.round(),
        result.sent.bytes.unwrap_or(0),
        result.intervals.len()
    );
}

// ============================================================================
// 端到端：TCP 反向
// ============================================================================

#[test]
fn 实测_TCP反向交换方向() {
    let _serial = live_lock();
    let Some((exe, _)) = setup("live_tcp_reverse") else {
        return;
    };
    let port = next_port();
    let _guard = ServerGuard::start(&exe, port);
    let probe = probe_binary(&exe);

    let mut o = short_tcp(port);
    o.reverse = true;
    let result =
        run_client_streaming(&o, &exe, &probe, "live-tcp-rev", |_| {}).expect("TCP 反向测试应成功");

    assert_eq!(result.direction, "反向");
    // 反向时接收侧是"下载"方向，必须有真实数据
    let recv = result.received.mbps.expect("反向测试的接收侧吞吐应有值");
    assert!(recv > 0.0, "反向吞吐应大于 0，实际 {} Mbps", recv);

    eprintln!("[iperf3_live] TCP 反向：接收侧 {} Mbps", recv.round());
}

// ============================================================================
// 端到端：UDP（抖动与丢包）
// ============================================================================

#[test]
fn 实测_UDP报告抖动与丢包() {
    let _serial = live_lock();
    let Some((exe, _)) = setup("live_udp") else {
        return;
    };
    let port = next_port();
    let _guard = ServerGuard::start(&exe, port);
    let probe = probe_binary(&exe);

    let mut o = short_tcp(port);
    o.protocol = Protocol::Udp;
    // UDP 默认 1Mbps 太慢，给个明确码率
    o.bitrate = Some("10M".to_string());

    let result =
        run_client_streaming(&o, &exe, &probe, "live-udp", |_| {}).expect("UDP 测试应成功");

    assert_eq!(result.protocol, "UDP");
    // UDP 的核心指标：抖动与丢包
    let summary = if result.received.is_empty() {
        &result.sent
    } else {
        &result.received
    };
    assert!(
        summary.jitter_ms.is_some(),
        "UDP 结果应包含抖动（jitter_ms）"
    );
    assert!(
        summary.lost_percent.is_some(),
        "UDP 结果应包含丢包率（lost_percent）"
    );
    assert!(
        summary.packets.unwrap_or(0) > 0,
        "UDP 应有收发包数，实际 {:?}",
        summary.packets
    );
    assert!(summary.mbps.unwrap_or(0.0) > 0.0, "UDP 吞吐应大于 0");

    eprintln!(
        "[iperf3_live] UDP：{} Mbps / 抖动 {:?} ms / 丢包 {:?}% / 包数 {:?}",
        summary.mbps.unwrap_or(0.0).round(),
        summary.jitter_ms,
        summary.lost_percent,
        summary.packets
    );
}

// ============================================================================
// 端到端：双向（--bidir）
// ============================================================================

#[test]
fn 实测_双向报告两个方向() {
    let _serial = live_lock();
    let Some((exe, _)) = setup("live_bidir") else {
        return;
    };
    // --bidir 需要 3.7+
    let probe = probe_binary(&exe);
    if !probe.version.supports_bidir() {
        eprintln!(
            "[iperf3_live] 跳过 live_bidir：iperf3 {} 不支持 --bidir",
            probe.version.label()
        );
        return;
    }
    let port = next_port();
    let _guard = ServerGuard::start(&exe, port);

    let mut o = short_tcp(port);
    o.bidir = true;
    let result =
        run_client_streaming(&o, &exe, &probe, "live-bidir", |_| {}).expect("双向测试应成功");

    assert_eq!(result.direction, "双向");
    assert!(result.sent.mbps.unwrap_or(0.0) > 0.0, "正向发送侧应有值");
    let rev = result
        .bidir_reverse_sent
        .as_ref()
        .expect("双向测试必须给出反向通道统计（sum_sent_bidir_reverse）");
    assert!(
        rev.mbps.unwrap_or(0.0) > 0.0,
        "反向通道吞吐应大于 0，实际 {:?}",
        rev.mbps
    );

    eprintln!(
        "[iperf3_live] 双向：正向 {} Mbps / 反向 {} Mbps",
        result.sent.mbps.unwrap_or(0.0).round(),
        rev.mbps.unwrap_or(0.0).round()
    );
}

// ============================================================================
// 端到端：并行流
// ============================================================================

#[test]
fn 实测_并行流报告流数量() {
    let _serial = live_lock();
    let Some((exe, _)) = setup("live_parallel") else {
        return;
    };
    let port = next_port();
    let _guard = ServerGuard::start(&exe, port);
    let probe = probe_binary(&exe);

    let mut o = short_tcp(port);
    o.parallel = 4;
    let result =
        run_client_streaming(&o, &exe, &probe, "live-parallel", |_| {}).expect("并行测试应成功");

    assert_eq!(result.parallel, 4);
    // interval 的 streams 数组长度即并行流数
    let row = result.intervals.first().expect("应有区间");
    assert_eq!(row.streams, 4, "并行 4 流时区间应报告 4 条流");

    eprintln!("[iperf3_live] 并行：4 流，区间流数 = {}", row.streams);
}

// ============================================================================
// 端到端：失败路径与取消
// ============================================================================

/// 连不上的端口必须给出**可读的中文错误**，而不是静默成功或空串。
#[test]
fn 实测_连接拒绝给出可读错误() {
    let _serial = live_lock();
    let Some((exe, _)) = setup("live_connect_refused") else {
        return;
    };
    let probe = probe_binary(&exe);
    // 找一个几乎肯定没人监听的端口
    let port = 5999;
    let mut o = short_tcp(port);
    o.connect_timeout_ms = Some(2000); // 别让测试等太久

    let err = run_client_streaming(&o, &exe, &probe, "live-refused", |_| {})
        .expect_err("连不上时应返回错误");
    assert!(
        !err.trim().is_empty(),
        "错误消息不能为空（旧实现会返回空串）"
    );
    assert!(
        err.contains("refused")
            || err.contains("Connection")
            || err.contains("连接")
            || err.contains("unable"),
        "错误应包含可读原因，实际：{}",
        err
    );

    eprintln!("[iperf3_live] 连接拒绝错误（预期）：{}", err.trim());
}

/// 取消必须返回 `cancelled = true` 的成功结果，而不是 `Err`。
#[test]
fn 实测_取消报告为取消而非失败() {
    let _serial = live_lock();
    let Some((exe, _)) = setup("live_cancel") else {
        return;
    };
    let port = next_port();
    let _guard = ServerGuard::start(&exe, port);
    let probe = probe_binary(&exe);

    // 用较长的测试（20s），中途取消
    let mut o = short_tcp(port);
    o.time = 20;
    let cmd_id = "live-cancel-cmd";
    // 提前登记取消标志，使循环第一轮就命中取消分支
    let flag = secm_lib::cancel::cancel_flag(cmd_id);
    flag.store(true, std::sync::atomic::Ordering::SeqCst);

    let result = run_client_streaming(&o, &exe, &probe, cmd_id, |_| {})
        .expect("取消应返回 Ok 而非 Err（取消不是失败）");

    assert!(result.cancelled, "结果必须标记 cancelled");
    secm_lib::cancel::clear_cancel(cmd_id);

    eprintln!(
        "[iperf3_live] 取消：cancelled={}（预期 true）",
        result.cancelled
    );
}

// ============================================================================
// 服务端状态机
// ============================================================================

#[test]
fn 实测_服务端状态机生命周期() {
    let _serial = live_lock();
    let Some((exe, _)) = setup("live_server_status") else {
        return;
    };
    let port = next_port();

    // 启动前应为停止态
    let before = server_status(false);
    assert!(!before.running, "未启动时 running 应为 false");

    let guard = ServerGuard::start(&exe, port);

    let running = server_status(false);
    assert!(running.running, "启动后应报告运行中");
    assert_eq!(running.port, Some(port));
    assert!(running.pid.is_some(), "运行中应有 PID");
    assert!(running.started_at.is_some(), "运行中应有启动时间");

    // 端口被占用时重复启动应给出明确错误
    let dup = server_start(&exe, port, false);
    assert!(dup.is_err(), "同一端口重复启动应被拒");
    let msg = dup.unwrap_err();
    assert!(
        msg.contains("已在运行") || msg.contains("占用"),
        "重复启动的错误应说明原因，实际：{}",
        msg
    );

    // 停掉后回到停止态
    let stopped = server_stop();
    assert!(!stopped.running, "停止后 running 应为 false");

    drop(guard); // Drop 也会调 stop（幂等），验证不会 panic

    let after = server_status(false);
    assert!(!after.running, "最终应处于停止态");

    eprintln!(
        "[iperf3_live] 服务端状态机：启动→运行中(pid={:?})→停止 ✓",
        running.pid
    );
}

/// 服务端启动后应能捕获其输出（"Server listening on ..."）。
#[test]
fn 实测_服务端输出被捕获() {
    let _serial = live_lock();
    let Some((exe, _)) = setup("live_server_output") else {
        return;
    };
    let port = next_port();
    let _guard = ServerGuard::start(&exe, port);

    // 等排空线程收集首行输出
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut lines = Vec::new();
    while std::time::Instant::now() < deadline {
        lines = server_status(false).lines;
        if lines
            .iter()
            .any(|l| l.contains("listening") || l.contains("Listening"))
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    assert!(
        !lines.is_empty(),
        "服务端应捕获到输出（用户需要看到 Server listening 提示）"
    );
    eprintln!("[iperf3_live] 服务端输出首行：{:?}", lines.first());
}

/// 端口预检：已被普通程序占用的端口，启动应给出明确中文提示而不是挂起。
#[test]
fn 实测_端口占用预检拒绝启动() {
    let _serial = live_lock();
    let Some((exe, _)) = setup("live_port_precheck") else {
        return;
    };
    let port = next_port();
    // 自己占住端口
    let listener = std::net::TcpListener::bind(("0.0.0.0", port)).expect("应能占用测试端口");

    let err = server_start(&exe, port, false).expect_err("端口被占用时应失败");
    assert!(
        err.contains("占用") || err.contains("已在运行"),
        "应给出明确的中文原因，实际：{}",
        err
    );

    drop(listener);
    let _ = server_stop();

    eprintln!("[iperf3_live] 端口占用预检：{}", err);
}
