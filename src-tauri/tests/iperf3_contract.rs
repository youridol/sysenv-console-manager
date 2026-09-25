//! iperf3 网络工具契约测试（行为 + 源码文本双通道）。
//!
//! ## 为什么同时用两种通道
//!
//! - **行为断言**（直接调用 `secm_lib::iperf` 的公开 API）覆盖参数编排、输入校验、
//!   JSON 解析这些纯逻辑——它们是本功能最容易出错、也最需要回归保护的部分。
//! - **源码文本断言**用于锁住那些「行为正确但结构会悄悄退化」的不变量，例如
//!   `stderr` 必须被并发排空（否则管道写满会死锁）、取消必须返回取消语义而非
//!   报错、应用退出必须清理服务端进程。这些无法仅靠调用 API 断言。
//!
//! 全部断言不启动 GUI、不依赖真实安装（真实端到端在 `iperf3_live.rs`）。
//!
//! ## 实测基准（iperf 3.21 / Windows Cygwin 构建，本机 loopback）
//!
//! 测试里内嵌的 JSON 片段均为**实测捕获**的真实输出，而非按文档想象：
//! `--json-stream` 的外层是 `{"event":...,"data":...}`（不是顶层
//! `start`/`intervals`/`end`），`error` 事件的 `data` 是字符串，
//! 失败时 `end` 的 `data` 是空对象。这些边界曾极容易写错。
//!
//! **测试名一律中文**（项目约定）。中文名里夹带的协议/格式类英文缩写
//! （UDP、TCP、JSON、CPU、PATH、DLL…）会触发 rustc 的 `non_snake_case`
//! 启发式告警——纯风格噪音，与正确性无关，故模块级豁免。

#![allow(non_snake_case)]

use secm_lib::iperf::*;
use std::path::{Path, PathBuf};

// ============================================================================
// 工具
// ============================================================================

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = <repo>/src-tauri
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 应有父目录")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("无法读取 {}: {}", p.display(), e))
}

fn opt(host: &str) -> Iperf3ClientOptions {
    Iperf3ClientOptions {
        host: host.to_string(),
        ..Default::default()
    }
}

fn v321() -> Iperf3Version {
    Iperf3Version {
        major: 3,
        minor: 21,
        patch: 0,
    }
}

fn v316() -> Iperf3Version {
    Iperf3Version {
        major: 3,
        minor: 16,
        patch: 0,
    }
}

fn args_of(o: &Iperf3ClientOptions, v: Iperf3Version) -> Vec<String> {
    build_client_args(o, v).expect("参数构建应成功")
}

fn assert_pair(args: &[String], flag: &str, value: &str) {
    let pos = args
        .iter()
        .position(|a| a == flag)
        .unwrap_or_else(|| panic!("缺少参数 {}，实际：{:?}", flag, args));
    assert_eq!(
        args.get(pos + 1).map(String::as_str),
        Some(value),
        "参数 {} 的值不符，实际：{:?}",
        flag,
        args
    );
}

fn assert_has(args: &[String], flag: &str) {
    assert!(
        args.iter().any(|a| a == flag),
        "缺少参数 {}，实际：{:?}",
        flag,
        args
    );
}

fn parser(json_stream: bool) -> Iperf3Parser {
    Iperf3Parser::with_tool_version(&opt("1.2.3.4"), "iperf 3.21", json_stream)
}

// ============================================================================
// 版本解析与能力门控
// ============================================================================

#[test]
fn 版本解析_覆盖真实格式与垃圾输入() {
    let p = parse_version("iperf 3.21 (cJSON 1.7.15)");
    assert_eq!(
        (p.version.major, p.version.minor, p.version.patch),
        (3, 21, 0)
    );
    assert_eq!(p.raw, "iperf 3.21 (cJSON 1.7.15)");

    // 上游另一种写法（带 version 中缀 + 三段号）
    let p2 = parse_version("iperf version 3.17.1 (cJSON 1.7.15)");
    assert_eq!(
        (p2.version.major, p2.version.minor, p2.version.patch),
        (3, 17, 1)
    );

    // 垃圾输入必须回落"未知"，绝不 panic、绝不猜
    for bad in [
        "",
        "   ",
        "not iperf at all",
        "iperf",
        "iperf x.y",
        "iperf 0.0",
    ] {
        assert!(
            !parse_version(bad).version.is_known(),
            "输入 {:?} 不应解析出已知版本",
            bad
        );
    }
}

/// 版本能力门控必须**保守**：未知版本一律按不支持处理，
/// 否则会给旧版 iperf3 传它不认识的参数（如 `--json-stream`）。
#[test]
fn 版本门控_能力判定保持保守() {
    let unknown = parse_version("完全无法解析").version;
    assert!(!unknown.is_known());
    assert!(
        !unknown.supports_json_stream(),
        "未知版本不得假设支持 --json-stream"
    );
    assert!(!unknown.supports_bidir());
    assert!(!unknown.supports_dscp());

    let v317 = Iperf3Version {
        major: 3,
        minor: 17,
        patch: 0,
    };
    assert!(v317.supports_json_stream(), "3.17 恰好引入 --json-stream");
    assert!(!v316().supports_json_stream(), "3.16 不应支持");
}

// ============================================================================
// 参数编排：完整性（对照官方手册的 client options）
// ============================================================================

/// 断言官方手册中**在 Windows 上有意义**的客户端参数都能被生成。
///
/// 参数名逐项对照 esnet/iperf `src/iperf3.1` 的 CLIENT SPECIFIC OPTIONS。
#[test]
fn 参数编排_覆盖官方手册选项() {
    let mut o = opt("iperf.example.com");
    o.port = 5202;
    o.protocol = Protocol::Udp;
    o.end_mode = EndMode::Time;
    o.time = 30;
    o.length = Some("128K".into());
    o.parallel = 4;
    o.omit = 2;
    o.bitrate = Some("100M".into());
    o.interval = 2;
    o.format = "g".into();
    o.window = Some("2M".into());
    o.mss = Some(1460);
    o.no_delay = true;
    o.ip_family = IpFamily::V4;
    o.tos = Some("0x34".into());
    o.dscp = Some("46".into());
    o.zerocopy = true;
    o.verbose = true;
    o.title = Some("SECM-TEST".into());
    o.get_server_output = true;
    o.connect_timeout_ms = Some(4000);
    o.affinity = Some("1,2".into());
    o.bind = Some("192.168.1.50".into());
    o.cport = Some(51000);
    o.repeating_payload = true;
    o.dont_fragment = true;
    o.udp_counters_64bit = true;
    o.rcv_timeout_ms = Some(5000);
    o.snd_timeout_ms = Some(6000);
    o.extra_data = Some("tag".into());
    o.force_flush = true;

    let a = args_of(&o, v321());

    // 基础
    assert_pair(&a, "-c", "iperf.example.com");
    assert_pair(&a, "-p", "5202");
    assert_has(&a, "-u");
    // 时长/传输
    assert_pair(&a, "-t", "30");
    assert_pair(&a, "-l", "128K");
    assert_pair(&a, "-P", "4");
    assert_pair(&a, "-O", "2");
    assert_pair(&a, "-b", "100M");
    assert_pair(&a, "-i", "2");
    assert_pair(&a, "-f", "g");
    // 套接字与协议细节
    assert_pair(&a, "-w", "2M");
    assert_pair(&a, "-M", "1460");
    assert_has(&a, "-N");
    assert_has(&a, "-4");
    assert_pair(&a, "-S", "0x34");
    assert_pair(&a, "--dscp", "46");
    // 输出与诊断
    assert_has(&a, "-Z");
    assert_has(&a, "-V");
    assert_pair(&a, "-T", "SECM-TEST");
    assert_has(&a, "--get-server-output");
    assert_pair(&a, "--connect-timeout", "4000");
    // 绑定与亲和
    assert_pair(&a, "-A", "1,2");
    assert_pair(&a, "-B", "192.168.1.50");
    assert_pair(&a, "--cport", "51000");
    // 载荷与 UDP
    assert_has(&a, "--repeating-payload");
    assert_has(&a, "--dont-fragment");
    assert_has(&a, "--udp-counters-64bit");
    assert_pair(&a, "--rcv-timeout", "5000");
    assert_pair(&a, "--snd-timeout", "6000");
    assert_pair(&a, "--extra-data", "tag");
    assert_has(&a, "--forceflush");
    // 实时区间输出
    assert_has(&a, "--json-stream");
}

/// 反向与双向必须互斥（iperf3 语义上只能选一个）。
#[test]
fn 反向与双向必须互斥() {
    let mut o = opt("1.2.3.4");
    o.reverse = true;
    o.bidir = true;
    let e = build_client_args(&o, v321()).unwrap_err();
    assert!(e.contains("互斥"), "应提示互斥，实际：{}", e);
}

/// `-t` / `-n` / `-k` 三者互斥：任何时刻只能出现一个。
#[test]
fn 结束条件_三选项互斥() {
    let a = args_of(&opt("h"), v321());
    assert_pair(&a, "-t", "10");
    assert!(!a.iter().any(|x| x == "-n" || x == "-k"));

    let mut ob = opt("h");
    ob.end_mode = EndMode::Bytes;
    ob.bytes = Some("100M".into());
    let ab = args_of(&ob, v321());
    assert_pair(&ab, "-n", "100M");
    assert!(!ab.iter().any(|x| x == "-t" || x == "-k"), "三者必须互斥");

    let mut ok = opt("h");
    ok.end_mode = EndMode::Blocks;
    ok.blockcount = Some("1000".into());
    let ak = args_of(&ok, v321());
    assert_pair(&ak, "-k", "1000");
    assert!(!ak.iter().any(|x| x == "-t" || x == "-n"), "三者必须互斥");
}

/// 旧版 iperf3（无 `--json-stream`）必须自动退回 `-J`，而不是传一个它不认识的参数。
#[test]
fn 旧版本_退回纯J输出() {
    let a = args_of(&opt("1.2.3.4"), v316());
    assert_has(&a, "-J");
    assert!(
        !a.iter().any(|x| x == "--json-stream"),
        "3.16 不认识 --json-stream，绝不能传"
    );
    // -J 是结束时一次性输出，必须靠 --forceflush 才能及时看到内容
    assert_has(&a, "--forceflush");
}

/// 平台不支持的选项必须**明确拒绝**并说明原因，而不是静默生成无效参数。
#[test]
fn 平台不支持项_显式拒绝并说明() {
    // -C 拥塞控制：Windows 构建的 argv 不含该选项
    let mut o = opt("h");
    o.congestion = Some("cubic".into());
    let e = build_client_args(&o, v321()).unwrap_err();
    assert!(e.contains("Linux"), "应说明仅 Linux/FreeBSD，实际：{}", e);

    // --dscp 在 <3.2 不存在，且应提示可改用 -S/--tos
    let mut o2 = opt("h");
    o2.dscp = Some("46".into());
    let v31 = Iperf3Version {
        major: 3,
        minor: 1,
        patch: 0,
    };
    let e2 = build_client_args(&o2, v31).unwrap_err();
    assert!(e2.contains("TOS"), "应提示改用 TOS，实际：{}", e2);

    // --bidir 在 <3.7 不存在（3.7 引入）
    let mut o3 = opt("h");
    o3.bidir = true;
    let v36 = Iperf3Version {
        major: 3,
        minor: 6,
        patch: 0,
    };
    assert!(build_client_args(&o3, v36).is_err(), "3.6 不支持 --bidir");
    // 3.16 引入了 --bidir 之后的版本，必须放行（别把门槛设错）
    let mut o4 = opt("h");
    o4.bidir = true;
    assert!(
        build_client_args(&o4, v316()).is_ok(),
        "3.16 已支持 --bidir（3.7 引入），不应被拒"
    );
}

/// iperf3 明确禁止 `-F` 与 `-u` 组合，必须在前端/后端都拦住。
#[test]
fn 文件传输_拒绝UDP与缺失文件() {
    let mut o = opt("h");
    o.protocol = Protocol::Udp;
    o.file = Some("Cargo.toml".into());
    assert!(build_client_args(&o, v321()).unwrap_err().contains("互斥"));

    let mut o2 = opt("h");
    o2.file = Some("__definitely_missing_9f3a__.bin".into());
    assert!(build_client_args(&o2, v321())
        .unwrap_err()
        .contains("不存在"));

    // 目录不是普通文件，必须拒绝（否则 iperf3 行为未定义）
    let mut o3 = opt("h");
    o3.file = Some(".".into());
    assert!(build_client_args(&o3, v321()).is_err(), "目录应被拒");
}

// ============================================================================
// 安全：参数注入与选项注入
// ============================================================================

/// 主机名以 `-` 开头会被 iperf3 当成命令行选项（选项注入），必须拒绝。
///
/// 全程用 `Command::args` 数组传参（不经 shell），因此这是**唯一**剩下的注入面。
#[test]
fn 主机名校验_拒绝选项注入与控制字符() {
    for bad in ["-F", "--bind", "-c evil"] {
        let e = build_client_args(&opt(bad), v321()).unwrap_err();
        assert!(
            e.contains('\'') || e.contains("主机") || e.contains("目标"),
            "选项注入输入 {:?} 应被拒，实际错误：{}",
            bad,
            e
        );
    }
    for bad in ["a b", "a\tb", "a\nb", "a\rb", ""] {
        assert!(
            build_client_args(&opt(bad), v321()).is_err(),
            "主机 {:?} 应被拒",
            bad
        );
    }
    // 过长主机名
    assert!(build_client_args(&opt(&"a".repeat(254)), v321()).is_err());
    // 绑定地址走同一条校验
    let mut o = opt("h");
    o.bind = Some("-evil".into());
    assert!(build_client_args(&o, v321()).is_err());
}

/// 数值规格必须走白名单，避免把任意字符串塞进命令行。
#[test]
fn 数值规格_走白名单校验() {
    for bad in ["abc", "100X", "1.5M", "M"] {
        let mut o = opt("h");
        o.bitrate = Some(bad.into());
        assert!(
            build_client_args(&o, v321()).is_err(),
            "码率 {:?} 应被拒",
            bad
        );
    }
    // burst 写法合法
    let mut ok = opt("h");
    ok.bitrate = Some("10M/1000".into());
    assert_pair(&args_of(&ok, v321()), "-b", "10M/1000");

    for bad in ["10M/", "/100", "10M/abc"] {
        let mut o = opt("h");
        o.bitrate = Some(bad.into());
        assert!(
            build_client_args(&o, v321()).is_err(),
            "码率 {:?} 应被拒",
            bad
        );
    }
}

/// 数值范围必须收敛，防止异常大值把一次测试拖成"假死"。
#[test]
fn 数值范围_全部收敛() {
    let mut o = opt("h");
    o.port = 0;
    assert!(build_client_args(&o, v321()).is_err());
    let mut o2 = opt("h");
    o2.time = 86_401;
    assert!(build_client_args(&o2, v321()).is_err());
    let mut o3 = opt("h");
    o3.parallel = 0;
    assert!(build_client_args(&o3, v321()).is_err());
    let mut o4 = opt("h");
    o4.parallel = 129;
    assert!(build_client_args(&o4, v321()).is_err());
    let mut o5 = opt("h");
    o5.mss = Some(32_769);
    assert!(build_client_args(&o5, v321()).is_err());
    let mut o6 = opt("h");
    o6.format = "X".into();
    assert!(build_client_args(&o6, v321()).is_err());
    // 边界值应被接受
    let mut ok = opt("h");
    ok.time = 86_400;
    assert!(build_client_args(&ok, v321()).is_ok());
    let mut ok2 = opt("h");
    ok2.interval = 0; // 关闭周期报告是合法的
    assert_pair(&args_of(&ok2, v321()), "-i", "0");
}

/// 控制字符不得进入输出标题/附加数据（会破坏日志与终端渲染）。
#[test]
fn 文本选项_拒绝控制字符() {
    let mut o = opt("h");
    o.title = Some("bad\u{0007}".into());
    assert!(build_client_args(&o, v321()).is_err());
    let mut o2 = opt("h");
    o2.extra_data = Some("bad\u{0007}".into());
    assert!(build_client_args(&o2, v321()).is_err());
}

// ============================================================================
// JSON 解析：实测边界
// ============================================================================

/// 实测捕获的 TCP interval 行。
const INTERVAL_TCP: &str = r#"{"event":"interval","data":{"streams":[{"socket":5,"start":0,"end":1.004657,"seconds":1.0046570301055908,"bytes":482082816,"bits_per_second":3838785189.8021951,"omitted":false,"sender":true}],"sum":{"start":0,"end":1.004657,"seconds":1.0046570301055908,"bytes":482082816,"bits_per_second":3838785189.8021951,"omitted":false,"sender":true}}}"#;

/// 实测捕获的 TCP end 行（含 CPU 占用）。
const END_TCP: &str = r#"{"event":"end","data":{"streams":[],"sum_sent":{"start":0,"end":2.007015,"seconds":2.007015,"bytes":966918144,"bits_per_second":3854154130.3876653,"sender":true},"sum_received":{"start":0,"end":2.007173,"seconds":2.007173,"bytes":966787072,"bits_per_second":3853328325.9589486,"sender":true},"cpu_utilization_percent":{"host_total":8.49,"host_user":1.53,"host_system":6.95,"remote_total":10.11,"remote_user":2.34,"remote_system":7.77}}}"#;

/// 实测捕获的 start 行。
const START_TCP: &str = r#"{"event":"start","data":{"connected":[{"socket":5,"local_host":"127.0.0.1","local_port":8206}],"version":"iperf 3.21","test_start":{"protocol":"TCP","num_streams":1,"duration":2}}}"#;

/// **最关键的形状断言**：`--json-stream` 的外层是 `{"event":...,"data":...}`。
///
/// 若误以为是顶层 `start`/`intervals`/`end`（那是纯 `-J` 的形状），
/// 解析器会一行都取不到值——这正是本次实现前必须先用真实二进制核对的原因。
#[test]
fn 解析_流式输出为event_data包装() {
    let mut p = parser(true);
    let (events, handled) = p.feed_line(START_TCP);
    assert!(handled, "start 事件应被识别为 JSON 事件行");
    assert_eq!(events.len(), 1, "start 应透出一条人类可读提示");
    let r = p.finish(false);
    assert_eq!(r.tool_version, "iperf 3.21", "应从 data.version 取版本");
    assert_eq!(r.protocol, "TCP", "应从 data.test_start.protocol 取协议");
}

#[test]
fn 解析_区间事件转为结构化行() {
    let mut p = parser(true);
    let (events, handled) = p.feed_line(INTERVAL_TCP);
    assert!(handled);
    assert!(
        events.iter().any(|e| e.kind == "iperf-interval"),
        "应发出 iperf-interval 事件供前端实时逐行渲染"
    );
    let r = p.finish(false);
    assert_eq!(r.intervals.len(), 1);
    let row = &r.intervals[0];
    assert!((row.end_s - 1.004657).abs() < 1e-6);
    assert_eq!(row.bytes, Some(482082816));
    assert_eq!(row.streams, 1);
    // bits_per_second → Mbit/s
    let mbps = row.mbps.expect("应解析出吞吐");
    assert!((mbps - 3838.785).abs() < 1.0, "实际 {} Mbps", mbps);
    assert!(!row.omitted, "未传 -O 时不应标记为 omitted");
}

#[test]
fn 解析_end填充方向摘要与CPU() {
    let mut p = parser(true);
    p.feed_line(END_TCP);
    let r = p.finish(false);
    let mbps = r.sent.mbps.expect("发送侧吞吐应存在");
    assert!((mbps - 3854.15).abs() < 1.0, "实际 {} Mbps", mbps);
    assert_eq!(r.sent.bytes, Some(966918144));
    assert!(r.received.mbps.is_some(), "接收侧吞吐应存在");
    let cpu = r.cpu.expect("应解析出 CPU 占用");
    assert!(cpu.host_total.is_some());
    assert!(cpu.remote_total.is_some());
}

/// `--bidir` 的反向通道藏在 `sum_*_bidir_reverse`，必须单独取。
#[test]
fn 解析_双向反向通道单独取值() {
    let end = r#"{"event":"end","data":{"streams":[],"sum_sent":{"start":0,"end":1.011514,"seconds":1.011514,"bytes":452984832,"bits_per_second":3582628274.05,"sender":true},"sum_received":{"start":0,"end":1.011872,"seconds":1.011872,"bytes":452722688,"bits_per_second":3579288194.55,"sender":true},"sum_sent_bidir_reverse":{"start":0,"end":1.011514,"seconds":1.011514,"bytes":496893952,"bits_per_second":3929902716.13,"sender":false},"sum_received_bidir_reverse":{"start":0,"end":1.011872,"seconds":1.011872,"bytes":496500736,"bits_per_second":3925403497.68,"sender":false}}}"#;
    let mut p = parser(true);
    p.feed_line(end);
    let r = p.finish(false);
    assert!(r.sent.mbps.is_some(), "正向发送侧");
    assert!(r.received.mbps.is_some(), "正向接收侧");
    let rev_sent = r
        .bidir_reverse_sent
        .expect("反向发送侧应存在（sum_sent_bidir_reverse）");
    let rev_recv = r
        .bidir_reverse_received
        .expect("反向接收侧应存在（sum_received_bidir_reverse）");
    assert!((rev_sent.mbps.unwrap() - 3929.90).abs() < 1.0);
    assert!((rev_recv.mbps.unwrap() - 3925.40).abs() < 1.0);
}

/// 双向测试的 interval 同时给 `sum` 与 `sum_bidir_reverse`，应产出两行且方向可区分。
#[test]
fn 解析_双向区间产出两个方向() {
    let interval = r#"{"event":"interval","data":{"streams":[{"socket":5},{"socket":7}],"sum":{"start":0,"end":1.014556,"seconds":1.014556,"bytes":432930816,"bits_per_second":3413755727.9,"sender":true},"sum_bidir_reverse":{"start":0,"end":1.014556,"seconds":1.014556,"bytes":428081152,"bits_per_second":3375515049.15,"sender":false}}}"#;
    let mut p = parser(true);
    p.feed_line(interval);
    let r = p.finish(false);
    assert_eq!(r.intervals.len(), 2, "双向应产出正向与反向两行");
    assert_eq!(r.intervals[0].direction, "正向");
    assert_eq!(r.intervals[1].direction, "双向-反向");
    assert_eq!(r.intervals[0].streams, 2, "并行流数量来自 streams 数组长度");
}

/// UDP 结果看的是抖动与丢包，不是吞吐独占字段。
#[test]
fn 解析_UDP抖动与丢包() {
    let end = r#"{"event":"end","data":{"sum_sent":{"start":0,"end":1.014339,"seconds":1.014339,"bytes":1251220,"bits_per_second":9868259.03,"jitter_ms":0,"lost_packets":0,"packets":857,"lost_percent":0,"sender":true},"sum_received":{"start":0,"end":1.015256,"seconds":1.015256,"bytes":1251220,"bits_per_second":9859345.82,"jitter_ms":0.00870916424068173,"lost_packets":0,"packets":857,"lost_percent":0,"sender":false}}}"#;
    let mut p = parser(true);
    p.feed_line(end);
    let r = p.finish(false);
    assert_eq!(r.received.packets, Some(857));
    assert_eq!(r.received.lost_packets, Some(0));
    assert!((r.received.jitter_ms.unwrap() - 0.008709).abs() < 1e-6);
}

/// `-O` 预热段的区间必须如实标记，UI 不应把它计入有效测量。
#[test]
fn 解析_预热区间被标记() {
    let interval = r#"{"event":"interval","data":{"streams":[],"sum":{"start":0,"end":1,"seconds":1,"bytes":1000,"bits_per_second":8000,"omitted":true}}}"#;
    let mut p = parser(true);
    p.feed_line(interval);
    let r = p.finish(false);
    assert!(r.intervals[0].omitted, "-O 预热段应标记 omitted");
}

/// **实测边界**：`error` 事件的 `data` 是**字符串**而非对象。
#[test]
fn 解析_error事件字符串载荷() {
    let err = r#"{"event":"error","data":"unable to connect to server - server may have stopped running or use a different port, firewall issue, etc.: Connection refused"}"#;
    let mut p = parser(true);
    let (events, handled) = p.feed_line(err);
    assert!(handled);
    assert!(
        events.iter().any(|e| e.kind == "error"),
        "error 事件应透出为 error 类型，前端才会标红"
    );
    let r = p.finish(false);
    assert_eq!(r.messages.len(), 1);
    assert!(
        r.messages[0].contains("Connection refused"),
        "应保留真实原因"
    );
}

/// **实测边界**：连接失败时 iperf3 仍会打印 `{"event":"end","data":{}}`（空对象）。
/// 绝不能据此判定"测试成功"。
#[test]
fn 解析_空end不被误判为成功() {
    let mut p = parser(true);
    let (_, handled) = p.feed_line(r#"{"event":"end","data":{}}"#);
    assert!(handled);
    let r = p.finish(false);
    assert!(r.sent.is_empty(), "空 end 不应产生任何统计");
    assert!(r.received.is_empty());
}

/// 纯文本行（警告等）必须交回调用方按原文透传，不能被解析器吞掉。
#[test]
fn 解析_非JSON行交回纯文本() {
    let mut p = parser(true);
    assert!(!p.feed_line("iperf3: warning: 纯文本警告").1);
    assert!(!p.feed_line("").1);
    assert!(!p.feed_line("{不是合法JSON").1);
    assert!(!p.feed_line("[1,2,3]").1, "非对象应返回 false");
}

/// 新版本 iperf3 可能新增未知 event；解析器必须安全忽略而不是崩溃。
#[test]
fn 解析_未知事件安全忽略() {
    let mut p = parser(true);
    assert!(
        p.feed_line(r#"{"event":"something_new_in_3_99","data":{"x":1}}"#)
            .1
    );
    let r = p.finish(false);
    assert!(r.intervals.is_empty());
}

/// 不伪造原则：缺失/NaN/负数一律 None，绝不伪装成 0。
#[test]
fn 解析_缺失与非法数值不伪造为0() {
    let bad =
        r#"{"event":"end","data":{"sum_sent":{"bits_per_second":-1,"bytes":-5,"jitter_ms":null}}}"#;
    let mut p = parser(true);
    p.feed_line(bad);
    let r = p.finish(false);
    assert_eq!(r.sent.bytes, None, "负字节数不应被当作有效值");
    assert_eq!(r.sent.jitter_ms, None, "缺失抖动应为 None 而非 0");

    let d = Iperf3DirectionSummary::default();
    assert!(d.is_empty());
}

/// 长测试的错误消息必须有界，否则会刷爆内存并挤爆日志。
#[test]
fn 解析_诊断消息有界() {
    let mut p = parser(true);
    for i in 0..600 {
        let line = format!(r#"{{"event":"error","data":"e{}"}}"#, i);
        p.feed_line(&line);
    }
    let r = p.finish(false);
    assert!(
        r.messages.len() <= 50,
        "诊断消息必须有界（实际 {} 条）",
        r.messages.len()
    );
}

/// 取消不是失败：结果里必须标记 cancelled，供 UI 与失败区分开。
#[test]
fn 取消_是独立于失败的结果() {
    let mut p = parser(true);
    p.feed_line(START_TCP);
    let r = p.finish(true);
    assert!(r.cancelled);
    // 未取消的对照
    let mut p2 = parser(true);
    p2.feed_line(START_TCP);
    assert!(!p2.finish(false).cancelled);
}

/// 不支持 `--json-stream` 的旧版走整份 `-J` 文档（顶层 start/intervals/end）。
#[test]
fn 解析_纯J文档兼容旧版形状() {
    let doc = r#"{
  "start": {"version": "iperf 3.21", "test_start": {"protocol": "TCP"}},
  "intervals": [{"streams": [{"socket":5}], "sum": {"start":0,"end":1,"seconds":1,"bytes":1000000,"bits_per_second":8000000}}],
  "end": {"sum_sent": {"start":0,"end":1,"seconds":1,"bytes":1000000,"bits_per_second":8000000}}
}"#;
    let mut p = parser(false);
    assert!(p.feed_document(doc));
    let r = p.finish(false);
    assert_eq!(r.tool_version, "iperf 3.21");
    assert_eq!(r.intervals.len(), 1);
    assert!((r.sent.mbps.unwrap() - 8.0).abs() < 0.01);

    let mut p2 = parser(false);
    assert!(!p2.feed_document("not json at all"));
}

// ============================================================================
// 服务端与防火墙
// ============================================================================

#[test]
fn 服务端_参数最小且正确() {
    assert_eq!(
        build_server_args(5201, false).unwrap(),
        vec!["-s", "-p", "5201"]
    );
    let one_off = build_server_args(5300, true).unwrap();
    assert!(one_off.iter().any(|x| x == "--one-off"));
    assert!(build_server_args(0, false).is_err(), "端口 0 应被拒");
}

/// 防火墙规则必须用**数组参数**构造（不经 shell 字符串），且规则名可成对移除。
#[test]
fn 防火墙_以参数数组构造规则() {
    let add = build_firewall_args_add(5201, "tcp").unwrap();
    assert_eq!(add[0], "advfirewall");
    assert!(add.iter().any(|x| x == "firewall"));
    assert!(add.iter().any(|x| x == "add"));
    assert!(add.iter().any(|x| x == "rule"));
    assert!(add.iter().any(|x| x == "dir=in"), "只放行入站");
    assert!(add.iter().any(|x| x == "action=allow"));
    assert!(
        add.iter().any(|x| x == "protocol=TCP"),
        "协议应归一化为大写"
    );
    assert!(add.iter().any(|x| x == "localport=5201"));
    // 规则名带固定前缀，便于用户辨识与成对移除
    assert!(add.iter().any(|x| x.contains(FIREWALL_RULE_PREFIX)));

    let rm = build_firewall_args_remove(5201, "tcp").unwrap();
    assert!(rm.iter().any(|x| x == "delete"));
    assert!(
        rm.iter().any(|x| x.contains(FIREWALL_RULE_PREFIX)),
        "移除必须按同一规则名，否则会留下孤儿规则"
    );

    // 非法协议/端口必须被拒
    assert!(build_firewall_args_add(5201, "ICMP").is_err());
    assert!(build_firewall_args_remove(0, "TCP").is_err());
    // 动作白名单
    assert!(apply_firewall_rule("evil", 5201, "TCP").is_err());
}

// ============================================================================
// 前端字段契约（反序列化）
// ============================================================================

/// 前端按 camelCase 传参；字段名一改，`invoke` 就会静默丢参或报错。
#[test]
fn 反序列化_前端camelCase字段() {
    let json = r#"{
      "host": "192.168.1.1",
      "port": 5300,
      "protocol": "udp",
      "reverse": true,
      "endMode": "bytes",
      "bytes": "50M",
      "parallel": 4,
      "noDelay": true,
      "ipFamily": "v4",
      "getServerOutput": true,
      "connectTimeoutMs": 3000,
      "repeatingPayload": true,
      "udpCounters64bit": true,
      "rcvTimeoutMs": 4000,
      "sndTimeoutMs": 5000
    }"#;
    let o: Iperf3ClientOptions = serde_json::from_str(json).expect("应能反序列化前端参数");
    assert_eq!(o.host, "192.168.1.1");
    assert_eq!(o.port, 5300);
    assert_eq!(o.protocol, Protocol::Udp);
    assert!(o.reverse);
    assert_eq!(o.end_mode, EndMode::Bytes);
    assert_eq!(o.bytes.as_deref(), Some("50M"));
    assert_eq!(o.parallel, 4);
    assert!(o.no_delay);
    assert_eq!(o.ip_family, IpFamily::V4);
    assert!(o.get_server_output);
    assert_eq!(o.connect_timeout_ms, Some(3000));
    assert!(o.repeating_payload);
    assert!(o.udp_counters_64bit);
    assert_eq!(o.rcv_timeout_ms, Some(4000));
    assert_eq!(o.snd_timeout_ms, Some(5000));
}

/// 只传 host 时，其余字段必须落到与 iperf3 一致的默认值。
#[test]
fn 反序列化_默认值对齐iperf3() {
    let o: Iperf3ClientOptions =
        serde_json::from_str(r#"{"host":"example.com"}"#).expect("最小参数应可反序列化");
    assert_eq!(o.port, 5201, "iperf3 默认端口");
    assert_eq!(o.protocol, Protocol::Tcp);
    assert_eq!(o.end_mode, EndMode::Time);
    assert_eq!(o.time, 10, "iperf3 默认时长 10 秒");
    assert_eq!(o.parallel, 1);
    assert_eq!(o.interval, 1, "iperf3 默认报告间隔 1 秒");
    assert_eq!(o.format, "m");
    assert!(!o.reverse);
    assert!(!o.bidir);
}

// ============================================================================
// 源码文本守卫（行为正确但结构容易退化的不变量）
// ============================================================================

/// **stderr 必须并发排空。**
///
/// 旧实现只读 stdout：当 iperf3 往 stderr 写得足够多时管道缓冲区写满，
/// 子进程阻塞，父进程却在等 stdout → 死锁（表现为"测试永远不结束"）。
/// 这个缺陷在短测试里看不出来，必须用结构断言锁住。
#[test]
fn 源码守卫_stderr必须并发排空() {
    let src = read("src-tauri/src/iperf.rs");
    assert!(
        src.contains("stderr_handle"),
        "iperf.rs 缺少 stderr 排空线程句柄：只读 stdout 会在 stderr 管道写满时死锁"
    );
    assert!(
        src.contains("std::thread::spawn") && src.contains("BufReader::new(stderr)"),
        "iperf.rs 必须为 stderr 起独立线程读取（BufReader::new(stderr)）"
    );
    assert!(
        src.contains("stderr_handle.join()"),
        "iperf.rs 必须在收尾等待 stderr 线程，否则错误原文会丢失"
    );
}

/// **取消必须返回取消语义，而不是报错。**
///
/// 取消是用户主动行为；若走 `Err` 分支，UI 会标红报错、日志记「失败」，
/// 与"我按了停止"的语义完全相悖。
#[test]
fn 源码守卫_取消返回取消语义而非错误() {
    let src = read("src-tauri/src/iperf.rs");
    assert!(
        src.contains("Ok(parser.finish(true))"),
        "取消路径必须返回 Ok(cancelled=true)，而不是 Err（否则 UI 把用户主动停止报成失败）"
    );
}

/// **应用退出必须清理服务端进程**，否则会留下孤儿 iperf3 占着端口。
#[test]
fn 源码守卫_应用退出清理服务端进程() {
    let lib = read("src-tauri/src/lib.rs");
    assert!(
        lib.contains("crate::iperf::shutdown_server()"),
        "lib.rs 的 RunEvent::Exit 未调用 iperf::shutdown_server()：\
         退出后会留下孤儿 iperf3 服务端进程占用端口"
    );
    assert!(
        lib.contains("crate::lhm::shutdown()"),
        "既有的 LHM 退出清理不应被破坏"
    );
}

/// 禁止把命令拼成字符串交给 shell 执行（注入面）。
#[test]
fn 源码守卫_禁止shell字符串执行() {
    let src = read("src-tauri/src/iperf.rs");
    for banned in ["cmd /c", "cmd.exe /c", "sh -c", "Command::new(\"cmd\")"] {
        assert!(
            !src.contains(banned),
            "iperf.rs 出现 shell 字符串执行痕迹 `{}`：必须用 Command::args 数组传参",
            banned
        );
    }
    // 正向：必须用 .args(&args) 数组
    assert!(
        src.contains(".args(&args)"),
        "iperf.rs 应以 .args(&args) 数组形式传参"
    );
}

/// 二进制定位必须优先随包内置资源，并且**不接受前端传入路径**
/// （否则本命令会退化成任意程序执行器）。
#[test]
fn 源码守卫_随包优先且不接受前端路径() {
    let src = read("src-tauri/src/iperf.rs");
    assert!(
        src.contains("resource_dir()"),
        "iperf.rs 必须支持 Tauri resource_dir() 定位（NSIS 安装布局）"
    );
    assert!(
        src.contains("\"resources\"")
            && src.contains("\"iperf3\"")
            && src.contains(EXE_NAME_EXPECTED),
        "iperf.rs 必须支持 exe 同目录 resources/iperf3 布局（便携版）"
    );
    assert!(
        src.contains("find_on_path"),
        "iperf.rs 应保留系统 PATH 探测（用户自装 iperf3 场景）"
    );
    // 前端不得传入可执行文件路径
    let commands = read("src-tauri/src/commands.rs");
    let start = commands
        .find("pub async fn iperf3_stream")
        .expect("commands.rs 应有 iperf3_stream");
    let body: String = commands[start..].chars().take(1800).collect();
    assert!(
        !body.contains("exe_path") && !body.contains("exePath"),
        "iperf3_stream 不得接受前端传入的可执行文件路径（任意程序执行风险）"
    );
}

const EXE_NAME_EXPECTED: &str = "iperf3.exe";

/// 服务端进程句柄必须做代际校验，避免重启后旧排空线程写入新会话的输出。
#[test]
fn 源码守卫_服务端输出有代际号守卫() {
    let src = read("src-tauri/src/iperf.rs");
    assert!(
        src.contains("generation"),
        "iperf.rs 服务端缺少 generation 代际号：重启后旧线程会污染新会话输出"
    );
    assert!(
        src.contains("st.generation != generation"),
        "排空线程必须校验代际号后才写入"
    );
}

/// 服务端输出必须有容量上限，长测试（1 小时逐秒报告）不得无界增长。
#[test]
fn 源码守卫_服务端输出有容量上限() {
    let src = read("src-tauri/src/iperf.rs");
    assert!(
        src.contains("SERVER_LINE_CAPACITY"),
        "iperf.rs 服务端输出缺少容量上限常量"
    );
    assert!(
        src.contains("st.lines.pop_front()"),
        "服务端输出必须在超限时丢弃最旧行（环形缓冲）"
    );
}

/// 全仓统一：spawn 控制台程序必须带 CREATE_NO_WINDOW，否则 GUI 会冒黑窗。
#[test]
fn 源码守卫_所有spawn抑制控制台窗口() {
    let src = read("src-tauri/src/iperf.rs");
    assert!(
        src.contains("CREATE_NO_WINDOW"),
        "iperf.rs 应统一定义 CREATE_NO_WINDOW"
    );
    // 每个 Command::new 之后都应带 creation_flags（iperf3.exe / iperf3 / netsh 三处）
    let spawns = src.matches("Command::new(").count();
    let flags = src.matches(".creation_flags(CREATE_NO_WINDOW)").count();
    assert!(
        flags >= spawns,
        "有 {} 处 Command::new 但只有 {} 处 creation_flags：会有子进程弹出黑窗",
        spawns,
        flags
    );
}

/// 二进制随包分发：安装包资源清单、便携打包脚本与 .gitignore 必须一致，
/// 否则会产出"没有 iperf3 的安装包"（且 stage 产物被误提交）。
#[test]
fn 源码守卫_随包二进制已接入打包() {
    let conf = read("src-tauri/tauri.conf.json");
    assert!(
        conf.contains("resources/iperf3"),
        "tauri.conf.json 的 bundle.resources 未包含 resources/iperf3：\
         安装版将丢失随包 iperf3"
    );

    let portable = read("scripts/package-portable.mjs");
    assert!(
        portable.contains("iperf3"),
        "package-portable.mjs 未随包拷贝 iperf3：便携版将缺少二进制"
    );

    let ignore = read(".gitignore");
    assert!(
        ignore.contains("src-tauri/resources/iperf3/"),
        ".gitignore 未忽略 src-tauri/resources/iperf3/：stage 产物会被误提交"
    );

    let stage = read("scripts/stage-iperf3.mjs");
    assert!(
        stage.contains("SHA256") || stage.contains("sha256"),
        "stage-iperf3.mjs 必须校验 SHA256：二进制来源不可静默替换"
    );
}

/// Cygwin 版的 Windows iperf3 依赖同目录 `cygwin1.dll`；
/// 打包必须把 DLL 一起带上，否则解压即用时会直接启动失败。
#[test]
fn 源码守卫_Cygwin运行时DLL成对分发() {
    let stage = read("scripts/stage-iperf3.mjs");
    assert!(
        stage.contains("cygwin1.dll"),
        "stage-iperf3.mjs 未处理 cygwin1.dll：Cygwin 链接的 iperf3.exe 无法独立启动"
    );
}

/// 许可合规：随包第三方二进制必须附许可全文与来源说明。
#[test]
fn 源码守卫_随包二进制附许可与声明() {
    let stage = read("scripts/stage-iperf3.mjs");
    assert!(
        stage.contains("LICENSE") || stage.contains("许可"),
        "stage-iperf3.mjs 未随包许可文本：BSD-3-Clause 要求二进制分发附许可"
    );
    let conf = read("src-tauri/tauri.conf.json");
    assert!(
        conf.contains("iperf3/LICENSE") || conf.contains("LICENSE-IPERF3"),
        "tauri.conf.json 的资源清单未包含 iperf3 许可文本"
    );
}

/// 命令必须在 `generate_handler!` 注册，否则前端 invoke 报 UnknownCommand。
#[test]
fn 源码守卫_命令已注册且登记scope() {
    let lib = read("src-tauri/src/lib.rs");
    for cmd in [
        "commands::iperf3_stream",
        "commands::iperf3_tool_info",
        "commands::iperf3_server_start",
        "commands::iperf3_server_stop",
        "commands::iperf3_server_status",
        "commands::iperf3_firewall_rule",
    ] {
        assert!(
            lib.contains(cmd),
            "lib.rs 的 invoke_handler 未注册 {}——前端调用会得到 UnknownCommand",
            cmd
        );
    }

    // 命令名必须在 network scope 登记（否则右侧栏「按页输出」看不到它们）
    let log = read("src-tauri/src/log.rs");
    for label in [
        "iperf3_tool_info",
        "iperf3_server_start",
        "iperf3_server_stop",
        "iperf3_server_status",
        "iperf3_firewall_rule",
    ] {
        assert!(
            log.contains(label),
            "log.rs 的 scope 表未登记 {}——会落入 system scope，按页输出丢失",
            label
        );
    }
}

/// 旧实现的缺陷守卫：成功时返回 stderr（通常为空串）→ UI 只能显示"完成"。
/// 现在必须返回结构化结果。
#[test]
fn 源码守卫_结果为结构化而非原始stderr() {
    let commands = read("src-tauri/src/commands.rs");
    let src = read("src-tauri/src/iperf.rs");
    assert!(
        src.contains("pub struct Iperf3Result"),
        "iperf.rs 缺少结构化结果类型"
    );
    assert!(
        !src.contains("Ok(String::from_utf8_lossy(&output.stderr)"),
        "iperf.rs 仍在把 stderr 当结果返回（旧缺陷：成功时几乎总是空串）"
    );
    assert!(
        commands.contains("Iperf3Result"),
        "commands::iperf3_stream 的返回类型应是结构化 Iperf3Result"
    );
}

/// 二进制解析顺序必须把**随包资源**排在 PATH 之前，
/// 否则用户 PATH 里的旧版 iperf3 会覆盖随包版本。
#[test]
fn 源码守卫_随包二进制优先于PATH() {
    let src = read("src-tauri/src/iperf.rs");
    let resource_pos = src.find("resource_dir()").expect("应有 resource_dir 分支");
    let path_pos = src.find("find_on_path(EXE_NAME)").expect("应有 PATH 分支");
    assert!(
        resource_pos < path_pos,
        "随包资源必须在 PATH 之前解析，否则旧版 PATH iperf3 会覆盖随包版本"
    );
}

// ============================================================================
// 定位/降级行为（不依赖真实安装）
// ============================================================================

/// 指向不存在的路径时不能被采纳（否则会 spawn 失败并报出误导性错误）。
#[test]
fn 定位_不存在的覆盖路径被忽略() {
    set_exe_override(Some(PathBuf::from("__no_such_iperf3_binary__.exe")));
    invalidate_cache();
    let got = iperf3_exe(None);
    assert!(
        got.as_deref() != Some(Path::new("__no_such_iperf3_binary__.exe")),
        "不存在的路径不应作为解析结果"
    );
    set_exe_override(None);
    invalidate_cache();
}

/// 工具信息在二进制缺失时必须给出**可读的中文说明**，而不是空白或 panic。
#[test]
fn 定位_二进制缺失时优雅降级() {
    set_exe_override(Some(PathBuf::from("__no_such_iperf3_binary__.exe")));
    invalidate_cache();
    let info = tool_info(None);
    if !info.available {
        assert!(!info.note.is_empty(), "缺失时必须给出说明，不能空白");
        assert!(
            info.note.contains("未找到") || info.note.contains("iperf3"),
            "说明应明确指出未找到 iperf3，实际：{}",
            info.note
        );
    }
    set_exe_override(None);
    invalidate_cache();
}
