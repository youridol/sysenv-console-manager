//! iperf3 模块单元测试。
//!
//! 通过 `#[path]` 从 `iperf.rs` 引入（同一编译单元，可访问私有项）。
//!
//! 测试数据主要来自**实测捕获**的真实 iperf3 3.21 输出（本机 loopback，
//! 见 `iperf.rs` 顶部「实测确认的输出形状」），而非凭文档想象。
//!
//! **测试名一律中文**（项目约定，例：`校验_主机名拒绝选项注入`）。中文名里会夹带
//! 协议/格式类英文缩写（UDP、TCP、JSON、DSCP、TOS、MSS…），rustc 的
//! `non_snake_case` 启发式会把它误判为"非 snake_case"并逐条告警。这些告警纯粹是
//! 命名风格噪音、与正确性无关，故在模块级豁免；否则 12 条无关告警会淹没真正的问题。

#![allow(non_snake_case)]

use super::*;

/// 构造基础参数（host 必填，其余默认）。
fn opt(host: &str) -> Iperf3ClientOptions {
    Iperf3ClientOptions {
        host: host.to_string(),
        ..Default::default()
    }
}

/// 已确认支持 `--json-stream` 的版本（3.21）。
fn v321() -> Iperf3Version {
    Iperf3Version {
        major: 3,
        minor: 21,
        patch: 0,
    }
}

/// 未知版本（`--version` 解析失败）。
fn v_unknown() -> Iperf3Version {
    UNKNOWN_VERSION
}

/// 旧版本（无 `--json-stream`，应退回 `-J`）。
fn v316() -> Iperf3Version {
    Iperf3Version {
        major: 3,
        minor: 16,
        patch: 0,
    }
}

fn args_str(o: &Iperf3ClientOptions, v: Iperf3Version) -> Vec<String> {
    build_client_args(o, v).expect("参数构建应成功")
}

/// 断言参数序列中 `flag` 紧跟 `value`。
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

// ── 版本解析 ────────────────────────────────────────────────────────────

#[test]
fn 版本解析_标准格式() {
    let p = parse_version("iperf 3.21 (cJSON 1.7.15)");
    assert_eq!(p.version.major, 3);
    assert_eq!(p.version.minor, 21);
    assert_eq!(p.version.patch, 0);
    assert_eq!(p.raw, "iperf 3.21 (cJSON 1.7.15)");
}

#[test]
fn 版本解析_version中缀与三段号() {
    let p = parse_version("iperf version 3.17.1 (cJSON 1.7.15)");
    assert_eq!(
        (p.version.major, p.version.minor, p.version.patch),
        (3, 17, 1)
    );
}

#[test]
fn 版本解析_v前缀与多行输入() {
    // 只取首行；v 前缀应被剥离
    let p = parse_version("iperf v3.9\n其他信息\n");
    assert_eq!((p.version.major, p.version.minor), (3, 9));
}

#[test]
fn 版本解析_垃圾输入回落未知且不崩溃() {
    for bad in [
        "",
        "   ",
        "not iperf at all",
        "iperf",
        "iperf x.y",
        "iperf 0.0",
    ] {
        let p = parse_version(bad);
        assert!(!p.version.is_known(), "输入 {:?} 不应解析出已知版本", bad);
    }
}

#[test]
fn 版本门控_未知版本恒为不支持() {
    let v = v_unknown();
    // 关键：未知版本必须保守（不给旧版传它不认识的参数）
    assert!(!v.supports_json_stream());
    assert!(!v.supports_dscp());
    assert!(!v.supports_bidir());
    assert_eq!(v.label(), "未知");
}

#[test]
fn 版本门控_边界值() {
    let v317 = Iperf3Version {
        major: 3,
        minor: 17,
        patch: 0,
    };
    assert!(v317.supports_json_stream(), "3.17 恰好引入 --json-stream");
    assert!(
        !v316().supports_json_stream(),
        "3.16 不应支持 --json-stream"
    );
    let v302 = Iperf3Version {
        major: 3,
        minor: 2,
        patch: 0,
    };
    assert!(v302.supports_dscp());
    let v307 = Iperf3Version {
        major: 3,
        minor: 7,
        patch: 0,
    };
    assert!(v307.supports_bidir());
}

// ── 命令行构建：基础 ────────────────────────────────────────────────────

#[test]
fn 参数_最小TCP测试() {
    let a = args_str(&opt("127.0.0.1"), v321());
    assert_pair(&a, "-c", "127.0.0.1");
    assert_pair(&a, "-p", "5201");
    assert_pair(&a, "-t", "10");
    assert_pair(&a, "-i", "1");
    assert_pair(&a, "-f", "m");
    assert_has(&a, "--json-stream");
    // 默认不应出现的方向/协议开关
    assert!(!a.iter().any(|x| x == "-u"), "默认应为 TCP");
    assert!(!a.iter().any(|x| x == "-R"), "默认不应为反向");
    assert!(!a.iter().any(|x| x == "--bidir"), "默认不应为双向");
}

#[test]
fn 参数_旧版本退回J而非json_stream() {
    let a = args_str(&opt("1.2.3.4"), v316());
    assert_has(&a, "-J");
    assert!(!a.iter().any(|x| x == "--json-stream"));
    // 旧版走 -J 时应补 --forceflush，避免输出被缓冲到测试结束
    assert_has(&a, "--forceflush");
}

#[test]
fn 参数_未知版本按最保守集() {
    let a = args_str(&opt("1.2.3.4"), v_unknown());
    assert_has(&a, "-J");
    assert!(!a.iter().any(|x| x == "--json-stream"));
}

// ── 命令行构建：协议与方向 ──────────────────────────────────────────────

#[test]
fn 参数_UDP与反向往双向() {
    let mut o = opt("1.2.3.4");
    o.protocol = Protocol::Udp;
    assert_has(&args_str(&o, v321()), "-u");

    let mut o2 = opt("1.2.3.4");
    o2.reverse = true;
    assert_has(&args_str(&o2, v321()), "-R");

    let mut o3 = opt("1.2.3.4");
    o3.bidir = true;
    assert_has(&args_str(&o3, v321()), "--bidir");
}

#[test]
fn 参数_反向与双向互斥() {
    let mut o = opt("1.2.3.4");
    o.reverse = true;
    o.bidir = true;
    let e = build_client_args(&o, v321()).unwrap_err();
    assert!(e.contains("互斥"), "应提示互斥，实际：{}", e);
}

#[test]
fn 参数_bidir在旧版本被拒() {
    let mut o = opt("1.2.3.4");
    o.bidir = true;
    let v36 = Iperf3Version {
        major: 3,
        minor: 6,
        patch: 0,
    };
    let e = build_client_args(&o, v36).unwrap_err();
    assert!(e.contains("bidir"), "应说明版本不支持，实际：{}", e);
}

// ── 命令行构建：结束条件三选一 ──────────────────────────────────────────

#[test]
fn 参数_结束条件三选一互斥() {
    // -t
    let a = args_str(&opt("h"), v321());
    assert_pair(&a, "-t", "10");
    assert!(!a.iter().any(|x| x == "-n" || x == "-k"));

    // -n
    let mut ob = opt("h");
    ob.end_mode = EndMode::Bytes;
    ob.bytes = Some("100M".into());
    let ab = args_str(&ob, v321());
    assert_pair(&ab, "-n", "100M");
    assert!(!ab.iter().any(|x| x == "-t" || x == "-k"), "三者必须互斥");

    // -k
    let mut ok = opt("h");
    ok.end_mode = EndMode::Blocks;
    ok.blockcount = Some("1000".into());
    let ak = args_str(&ok, v321());
    assert_pair(&ak, "-k", "1000");
    assert!(!ak.iter().any(|x| x == "-t" || x == "-n"), "三者必须互斥");
}

#[test]
fn 参数_结束条件缺失值时报错() {
    let mut ob = opt("h");
    ob.end_mode = EndMode::Bytes;
    ob.bytes = None;
    assert!(build_client_args(&ob, v321())
        .unwrap_err()
        .contains("字节数"));

    let mut ok = opt("h");
    ok.end_mode = EndMode::Blocks;
    ok.blockcount = Some("  ".into());
    assert!(build_client_args(&ok, v321()).unwrap_err().contains("块数"));
}

#[test]
fn 参数_时长边界() {
    let mut o = opt("h");
    o.time = 0;
    assert!(build_client_args(&o, v321()).is_err(), "0 秒应被拒");
    let mut o2 = opt("h");
    o2.time = 86_401;
    assert!(
        build_client_args(&o2, v321()).is_err(),
        "超过 86400 秒应被拒"
    );
    let mut o3 = opt("h");
    o3.time = 86_400;
    assert!(build_client_args(&o3, v321()).is_ok(), "86400 秒应被接受");
}

// ── 命令行构建：各项参数 ────────────────────────────────────────────────

#[test]
fn 参数_并行与预热与码率() {
    let mut o = opt("h");
    o.parallel = 8;
    o.omit = 3;
    o.bitrate = Some("100M".into());
    let a = args_str(&o, v321());
    assert_pair(&a, "-P", "8");
    assert_pair(&a, "-O", "3");
    assert_pair(&a, "-b", "100M");
}

#[test]
fn 参数_码率burst写法被接受() {
    let mut o = opt("h");
    o.bitrate = Some("10M/1000".into());
    assert_pair(&args_str(&o, v321()), "-b", "10M/1000");
}

#[test]
fn 参数_非法码率被拒() {
    for bad in ["abc", "100X", "10M/", "/100", "10M/abc"] {
        let mut o = opt("h");
        o.bitrate = Some(bad.into());
        assert!(
            build_client_args(&o, v321()).is_err(),
            "码率 {:?} 应被拒",
            bad
        );
    }
}

#[test]
fn 参数_传输细节全家桶() {
    let mut o = opt("h");
    o.length = Some("128K".into());
    o.window = Some("2M".into());
    o.mss = Some(1460);
    o.no_delay = true;
    o.ip_family = IpFamily::V6;
    o.zerocopy = true;
    o.verbose = true;
    o.title = Some("SECM".into());
    o.get_server_output = true;
    o.connect_timeout_ms = Some(5000);
    o.affinity = Some("1".into());
    o.bind = Some("192.168.1.10".into());
    o.cport = Some(51000);
    o.repeating_payload = true;
    o.udp_counters_64bit = true;
    o.rcv_timeout_ms = Some(3000);
    o.snd_timeout_ms = Some(4000);
    o.extra_data = Some("tag".into());
    let a = args_str(&o, v321());
    assert_pair(&a, "-l", "128K");
    assert_pair(&a, "-w", "2M");
    assert_pair(&a, "-M", "1460");
    assert_has(&a, "-N");
    assert_has(&a, "-6");
    assert_has(&a, "-Z");
    assert_has(&a, "-V");
    assert_pair(&a, "-T", "SECM");
    assert_has(&a, "--get-server-output");
    assert_pair(&a, "--connect-timeout", "5000");
    assert_pair(&a, "-A", "1");
    assert_pair(&a, "-B", "192.168.1.10");
    assert_pair(&a, "--cport", "51000");
    assert_has(&a, "--repeating-payload");
    assert_has(&a, "--udp-counters-64bit");
    assert_pair(&a, "--rcv-timeout", "3000");
    assert_pair(&a, "--snd-timeout", "4000");
    assert_pair(&a, "--extra-data", "tag");
}

#[test]
fn 参数_强制IPv4() {
    let mut o = opt("h");
    o.ip_family = IpFamily::V4;
    assert_has(&args_str(&o, v321()), "-4");
}

#[test]
fn 参数_TOS支持八进制与十六进制() {
    let mut o = opt("h");
    o.tos = Some("0x34".into());
    assert_pair(&args_str(&o, v321()), "-S", "0x34");
    let mut o2 = opt("h");
    o2.tos = Some("064".into());
    assert_pair(&args_str(&o2, v321()), "-S", "064");
    let mut o3 = opt("h");
    o3.tos = Some("999".into());
    assert!(build_client_args(&o3, v321()).is_err());
}

#[test]
fn 参数_DSCP数值与符号名() {
    let mut o = opt("h");
    o.dscp = Some("46".into());
    assert_pair(&args_str(&o, v321()), "--dscp", "46");
    let mut o2 = opt("h");
    o2.dscp = Some("EF".into());
    assert_pair(&args_str(&o2, v321()), "--dscp", "EF");
    let mut o3 = opt("h");
    o3.dscp = Some("64".into());
    assert!(build_client_args(&o3, v321()).is_err(), "DSCP 64 超出 0-63");
}

#[test]
fn 参数_DSCP在旧版本被拒并提示改用TOS() {
    let mut o = opt("h");
    o.dscp = Some("46".into());
    let v31 = Iperf3Version {
        major: 3,
        minor: 1,
        patch: 0,
    };
    let e = build_client_args(&o, v31).unwrap_err();
    assert!(e.contains("TOS"), "应提示改用 TOS，实际：{}", e);
}

#[test]
fn 参数_文件传输与UDP互斥() {
    let mut o = opt("h");
    o.protocol = Protocol::Udp;
    o.file = Some("Cargo.toml".into());
    let e = build_client_args(&o, v321()).unwrap_err();
    assert!(e.contains("互斥"), "应提示互斥，实际：{}", e);
}

#[test]
fn 参数_文件不存在被拒() {
    let mut o = opt("h");
    o.file = Some("__definitely_missing_file_9f3a__.bin".into());
    let e = build_client_args(&o, v321()).unwrap_err();
    assert!(e.contains("不存在"), "实际：{}", e);
}

#[test]
fn 参数_目录不可当文件() {
    let mut o = opt("h");
    o.file = Some(".".into());
    assert!(build_client_args(&o, v321()).is_err(), "目录不应被当成文件");
}

#[test]
fn 参数_存在的文件被接受() {
    // Cargo.toml 为 cargo test 运行目录（src-tauri）下的真实文件
    if Path::new("Cargo.toml").is_file() {
        let mut o = opt("h");
        o.file = Some("Cargo.toml".into());
        assert_pair(&args_str(&o, v321()), "-F", "Cargo.toml");
    }
}

#[test]
fn 参数_DF位要求UDP() {
    let mut o = opt("h");
    o.dont_fragment = true;
    let e = build_client_args(&o, v321()).unwrap_err();
    assert!(e.contains("UDP"), "实际：{}", e);

    let mut o2 = opt("h");
    o2.protocol = Protocol::Udp;
    o2.dont_fragment = true;
    assert_has(&args_str(&o2, v321()), "--dont-fragment");
}

#[test]
fn 参数_拥塞控制在Windows构建被拒() {
    let mut o = opt("h");
    o.congestion = Some("cubic".into());
    let e = build_client_args(&o, v321()).unwrap_err();
    assert!(e.contains("Linux"), "应说明仅 Linux/FreeBSD，实际：{}", e);
}

#[test]
fn 参数_forceflush可显式开启() {
    let mut o = opt("h");
    o.force_flush = true;
    assert_has(&args_str(&o, v321()), "--forceflush");
}

#[test]
fn 参数_单位须为kmgt() {
    let mut o = opt("h");
    o.format = "X".into();
    assert!(build_client_args(&o, v321()).is_err());
    let mut o2 = opt("h");
    o2.format = "G".into();
    assert_pair(&args_str(&o2, v321()), "-f", "g");
}

#[test]
fn 参数_并行与间隔边界() {
    let mut o = opt("h");
    o.parallel = 0;
    assert!(build_client_args(&o, v321()).is_err());
    let mut o2 = opt("h");
    o2.parallel = 129;
    assert!(build_client_args(&o2, v321()).is_err());
    let mut o3 = opt("h");
    o3.interval = 3601;
    assert!(build_client_args(&o3, v321()).is_err());
    // interval=0（关闭周期报告）应被接受
    let mut o4 = opt("h");
    o4.interval = 0;
    assert_pair(&args_str(&o4, v321()), "-i", "0");
}

// ── 输入校验（安全）────────────────────────────────────────────────────

#[test]
fn 校验_主机名拒绝选项注入() {
    // 以 '-' 开头会被 iperf3 当成选项（如 -F /etc/passwd）
    let e = build_client_args(&opt("-F"), v321()).unwrap_err();
    assert!(e.contains('\''), "实际：{}", e);
    assert!(build_client_args(&opt("--bind"), v321()).is_err());
}

#[test]
fn 校验_主机名拒绝空白与控制字符() {
    for bad in ["a b", "a\tb", "a\nb", "a\rb", ""] {
        assert!(
            build_client_args(&opt(bad), v321()).is_err(),
            "主机 {:?} 应被拒",
            bad
        );
    }
}

#[test]
fn 校验_主机名过长被拒() {
    let long = "a".repeat(254);
    assert!(build_client_args(&opt(&long), v321()).is_err());
}

#[test]
fn 校验_端口为0被拒() {
    let mut o = opt("h");
    o.port = 0;
    assert!(build_client_args(&o, v321()).is_err());
}

#[test]
fn 校验_绑定地址同走主机名校验() {
    let mut o = opt("h");
    o.bind = Some("-evil".into());
    assert!(build_client_args(&o, v321()).is_err());
}

#[test]
fn 校验_标题与附加数据拒绝控制字符() {
    let mut o = opt("h");
    o.title = Some("bad\u{0007}".into());
    assert!(build_client_args(&o, v321()).is_err());
    let mut o2 = opt("h");
    o2.extra_data = Some("bad\u{0007}".into());
    assert!(build_client_args(&o2, v321()).is_err());
}

#[test]
fn 校验_数值规格工具函数() {
    assert!(is_size_spec("100"));
    assert!(is_size_spec("100M"));
    assert!(is_size_spec(" 2G "));
    assert!(!is_size_spec(""));
    assert!(!is_size_spec("M"));
    assert!(!is_size_spec("1.5M"));
    assert!(is_bitrate_spec("100M"));
    assert!(is_bitrate_spec("10M/1000"));
    assert!(!is_bitrate_spec("10M/"));
    assert!(is_affinity_spec("1"));
    assert!(is_affinity_spec("1,2"));
    assert!(!is_affinity_spec("1,"));
}

#[test]
fn 校验_TOS解析() {
    assert_eq!(parse_tos("52"), Some(52));
    assert_eq!(parse_tos("0x34"), Some(52));
    assert_eq!(parse_tos("064"), Some(52));
    assert_eq!(parse_tos("255"), Some(255));
    assert_eq!(parse_tos("256"), None);
    assert_eq!(parse_tos(""), None);
    assert_eq!(parse_tos("abc"), None);
}

// ── JSON 解析：实测捕获的真实输出 ──────────────────────────────────────

fn empty_parser(json_stream: bool) -> Iperf3Parser {
    Iperf3Parser::with_tool_version(&opt("1.2.3.4"), "iperf 3.21", json_stream)
}

/// 实测捕获的 TCP interval 行（iperf 3.21，loopback）。
const INTERVAL_TCP: &str = r#"{"event":"interval","data":{"streams":[{"socket":5,"start":0,"end":1.004657,"seconds":1.0046570301055908,"bytes":482082816,"bits_per_second":3838785189.8021951,"omitted":false,"sender":true}],"sum":{"start":0,"end":1.004657,"seconds":1.0046570301055908,"bytes":482082816,"bits_per_second":3838785189.8021951,"omitted":false,"sender":true}}}"#;

/// 实测捕获的 end 行（TCP），含 cpu_utilization_percent。
const END_TCP: &str = r#"{"event":"end","data":{"streams":[{"sender":{"socket":5,"start":0,"end":2.007015,"seconds":2.007015,"bytes":966918144,"bits_per_second":3854154130.3876653,"sender":true},"receiver":{"socket":5,"start":0,"end":2.007173,"seconds":2.007173,"bytes":966787072,"bits_per_second":3853328325.9589486,"sender":true}}],"sum_sent":{"start":0,"end":2.007015,"seconds":2.007015,"bytes":966918144,"bits_per_second":3854154130.3876653,"sender":true},"sum_received":{"start":0,"end":2.007173,"seconds":2.007173,"bytes":966787072,"bits_per_second":3853328325.9589486,"sender":true},"cpu_utilization_percent":{"host_total":8.4919743393392952,"host_user":1.5394807281843166,"host_system":6.9524936111549778,"remote_total":10.113580994837591,"remote_user":2.3415680135830872,"remote_system":7.7720129812545027}}}"#;

/// 实测捕获的 start 行（TCP）。
const START_TCP: &str = r#"{"event":"start","data":{"connected":[{"socket":5,"local_host":"127.0.0.1","local_port":8206,"remote_host":"127.0.0.1","remote_port":5299}],"version":"iperf 3.21","test_start":{"protocol":"TCP","num_streams":1,"duration":2}}}"#;

#[test]
fn 解析_start事件取版本与协议() {
    let mut p = empty_parser(true);
    let (events, handled) = p.feed_line(START_TCP);
    assert!(handled);
    assert_eq!(events.len(), 1, "start 应产生一条提示");
    let r = p.finish(false);
    assert_eq!(r.tool_version, "iperf 3.21");
    assert_eq!(r.protocol, "TCP");
}

#[test]
fn 解析_interval事件产出区间行() {
    let mut p = empty_parser(true);
    let (events, handled) = p.feed_line(INTERVAL_TCP);
    assert!(handled);
    assert!(
        events.iter().any(|e| e.kind == "iperf-interval"),
        "应发出 iperf-interval 事件供前端实时渲染"
    );
    let r = p.finish(false);
    assert_eq!(r.intervals.len(), 1);
    let row = &r.intervals[0];
    assert!((row.start_s - 0.0).abs() < 1e-9);
    assert!((row.end_s - 1.004657).abs() < 1e-6);
    assert_eq!(row.bytes, Some(482082816));
    assert_eq!(row.streams, 1);
    let mbps = row.mbps.expect("应解析出吞吐");
    assert!((mbps - 3838.785).abs() < 1.0, "实际 {} Mbps", mbps);
}

#[test]
fn 解析_end事件填充收发摘要() {
    let mut p = empty_parser(true);
    p.feed_line(END_TCP);
    let r = p.finish(false);
    // 966918144 bytes / 2.007015 s ≈ 3854 Mbit/s
    let mbps = r.sent.mbps.expect("发送侧吞吐应存在");
    assert!((mbps - 3854.15).abs() < 1.0, "实际 {} Mbps", mbps);
    assert_eq!(r.sent.bytes, Some(966918144));
    assert!((r.received.mbps.expect("接收侧吞吐应存在") - 3853.33).abs() < 1.0);
    let cpu = r.cpu.expect("应解析出 CPU 占用");
    assert!(cpu.host_total.is_some());
    assert!(cpu.remote_total.is_some());
}

#[test]
fn 解析_双向end填充两个方向() {
    // 实测捕获的 --bidir end（含 sum_*_bidir_reverse）
    let end = r#"{"event":"end","data":{"streams":[],"sum_sent":{"start":0,"end":1.011514,"seconds":1.011514,"bytes":452984832,"bits_per_second":3582628274.0525589,"sender":true},"sum_received":{"start":0,"end":1.011872,"seconds":1.011872,"bytes":452722688,"bits_per_second":3579288194.5542517,"sender":true},"sum_sent_bidir_reverse":{"start":0,"end":1.011514,"seconds":1.011514,"bytes":496893952,"bits_per_second":3929902716.1265192,"sender":false},"sum_received_bidir_reverse":{"start":0,"end":1.011872,"seconds":1.011872,"bytes":496500736,"bits_per_second":3925403497.6755948,"sender":false}}}"#;
    let mut p = empty_parser(true);
    p.feed_line(end);
    let r = p.finish(false);
    assert!(r.sent.mbps.is_some(), "正向发送侧");
    assert!(r.received.mbps.is_some(), "正向接收侧");
    let rev_sent = r.bidir_reverse_sent.expect("反向发送侧应存在");
    let rev_recv = r.bidir_reverse_received.expect("反向接收侧应存在");
    assert!((rev_sent.mbps.unwrap() - 3929.90).abs() < 1.0);
    assert!((rev_recv.mbps.unwrap() - 3925.40).abs() < 1.0);
}

#[test]
fn 解析_双向interval标注反向方向() {
    // 实测：bidir 的 interval 同时给 sum 与 sum_bidir_reverse
    let interval = r#"{"event":"interval","data":{"streams":[{"socket":5},{"socket":7}],"sum":{"start":0,"end":1.014556,"seconds":1.014556,"bytes":432930816,"bits_per_second":3413755727.9,"sender":true},"sum_bidir_reverse":{"start":0,"end":1.014556,"seconds":1.014556,"bytes":428081152,"bits_per_second":3375515049.15,"sender":false}}}"#;
    let mut p = empty_parser(true);
    p.feed_line(interval);
    let r = p.finish(false);
    assert_eq!(r.intervals.len(), 2, "双向应产出正向与反向两行");
    assert_eq!(r.intervals[0].direction, "正向");
    assert_eq!(r.intervals[1].direction, "双向-反向");
    assert_eq!(r.intervals[0].streams, 2, "并行流数量应来自 streams 数组");
}

#[test]
fn 解析_UDP区间与抖动丢包() {
    let interval = r#"{"event":"interval","data":{"streams":[{"socket":5,"start":0,"end":1.014339,"seconds":1.0143389701843262,"bytes":1251220,"bits_per_second":9868259.32378505,"packets":857,"omitted":false,"sender":true}],"sum":{"start":0,"end":1.014339,"seconds":1.0143389701843262,"bytes":1251220,"bits_per_second":9868259.32378505,"packets":857,"omitted":false,"sender":true}}}"#;
    let end = r#"{"event":"end","data":{"sum_sent":{"start":0,"end":1.014339,"seconds":1.014339,"bytes":1251220,"bits_per_second":9868259.03371555,"jitter_ms":0,"lost_packets":0,"packets":857,"lost_percent":0,"sender":true},"sum_received":{"start":0,"end":1.015256,"seconds":1.015256,"bytes":1251220,"bits_per_second":9859345.8201675247,"jitter_ms":0.00870916424068173,"lost_packets":0,"packets":857,"lost_percent":0,"sender":false}}}"#;
    let mut p = empty_parser(true);
    p.feed_line(interval);
    p.feed_line(end);
    let r = p.finish(false);
    assert!((r.intervals[0].mbps.unwrap() - 9.868).abs() < 0.01);
    assert_eq!(r.received.lost_packets, Some(0));
    assert_eq!(r.received.packets, Some(857));
    assert!((r.received.jitter_ms.unwrap() - 0.008709).abs() < 1e-6);
}

#[test]
fn 解析_预热段omitted标记被保留() {
    let interval = r#"{"event":"interval","data":{"streams":[],"sum":{"start":0,"end":1,"seconds":1,"bytes":1000,"bits_per_second":8000,"omitted":true}}}"#;
    let mut p = empty_parser(true);
    p.feed_line(interval);
    let r = p.finish(false);
    assert!(r.intervals[0].omitted, "-O 预热段应标记 omitted");
}

#[test]
fn 解析_error事件data是字符串() {
    // 实测：error 的 data 是字符串而非对象——必须与对象分支分开处理
    let err = r#"{"event":"error","data":"unable to connect to server - server may have stopped running or use a different port, firewall issue, etc.: Connection refused"}"#;
    let mut p = empty_parser(true);
    let (events, handled) = p.feed_line(err);
    assert!(handled);
    assert!(
        events.iter().any(|e| e.kind == "error"),
        "error 事件应透出为 error 类型"
    );
    let r = p.finish(false);
    assert_eq!(r.messages.len(), 1);
    assert!(r.messages[0].contains("Connection refused"));
}

#[test]
fn 解析_空end对象不被当成成功() {
    // 实测：连接失败时仍会打印 {"event":"end","data":{}}（空对象）
    let mut p = empty_parser(true);
    let (_, handled) = p.feed_line(r#"{"event":"end","data":{}}"#);
    assert!(handled);
    let r = p.finish(false);
    assert!(r.sent.is_empty(), "空 end 不应产生任何统计");
    assert!(r.received.is_empty());
}

#[test]
fn 解析_非JSON行交回纯文本处理() {
    let mut p = empty_parser(true);
    assert!(!p.feed_line("iperf3: warning: 这是一个纯文本警告").1);
    assert!(!p.feed_line("").1);
    assert!(!p.feed_line("{不是合法JSON").1);
    assert!(!p.feed_line("[1,2,3]").1, "非对象应返回 false");
}

#[test]
fn 解析_未知event被安全忽略() {
    let mut p = empty_parser(true);
    assert!(p.feed_line(r#"{"event":"something_new","data":{"x":1}}"#).1);
    let r = p.finish(false);
    assert!(r.intervals.is_empty());
}

#[test]
fn 解析_负数不被伪造为有效值() {
    // 不伪造原则：缺失/NaN/负数一律 None
    let bad =
        r#"{"event":"end","data":{"sum_sent":{"bits_per_second":-1,"bytes":-5,"jitter_ms":null}}}"#;
    let mut p = empty_parser(true);
    p.feed_line(bad);
    let r = p.finish(false);
    assert_eq!(r.sent.bytes, None, "负字节数不应被当作有效值");
    assert_eq!(r.sent.jitter_ms, None);
}

#[test]
fn 解析_诊断消息有界收集() {
    let mut p = empty_parser(true);
    for i in 0..(MAX_MESSAGES * 3) {
        let line = format!(r#"{{"event":"error","data":"e{}"}}"#, i);
        p.feed_line(&line);
    }
    let r = p.finish(false);
    assert_eq!(
        r.messages.len(),
        MAX_MESSAGES,
        "诊断消息必须有界，防止长测试刷爆内存"
    );
}

#[test]
fn 解析_取消标记被如实设置() {
    let mut p = empty_parser(true);
    p.feed_line(START_TCP);
    let r = p.finish(true);
    assert!(r.cancelled, "取消不是失败，但必须如实标记");
}

// ── JSON 解析：纯 -J 文档 ──────────────────────────────────────────────

#[test]
fn 解析_纯J多行文档() {
    // 实测 -J 是单个多行文档，顶层键 start/intervals/end
    let doc = r#"{
  "start": {"version": "iperf 3.21", "test_start": {"protocol": "TCP"}},
  "intervals": [{"streams": [{"socket":5}], "sum": {"start":0,"end":1,"seconds":1,"bytes":1000000,"bits_per_second":8000000}}],
  "end": {"sum_sent": {"start":0,"end":1,"seconds":1,"bytes":1000000,"bits_per_second":8000000}}
}"#;
    let mut p = empty_parser(false);
    assert!(p.feed_document(doc));
    let r = p.finish(false);
    assert_eq!(r.tool_version, "iperf 3.21");
    assert_eq!(r.intervals.len(), 1);
    assert!((r.sent.mbps.unwrap() - 8.0).abs() < 0.01);
}

#[test]
fn 解析_纯J文档非法时回落() {
    let mut p = empty_parser(false);
    assert!(!p.feed_document("not json"));
}

// ── 服务端与防火墙参数 ─────────────────────────────────────────────────

#[test]
fn 服务端_命令行参数构建() {
    assert_eq!(
        build_server_args(5201, false).unwrap(),
        vec!["-s", "-p", "5201"]
    );
    let b = build_server_args(5300, true).unwrap();
    assert!(b.iter().any(|x| x == "--one-off"));
    assert!(build_server_args(0, false).is_err());
}

#[test]
fn 防火墙_规则名含前缀与端口协议() {
    let n = firewall_rule_name(5201, "TCP");
    assert!(n.starts_with(FIREWALL_RULE_PREFIX), "实际：{}", n);
    assert!(n.contains("5201"));
    assert!(n.contains("TCP"));
}

#[test]
fn 防火墙_参数为数组且无shell拼接() {
    let add = build_firewall_args_add(5201, "tcp").unwrap();
    assert_eq!(add[0], "advfirewall");
    assert!(add.iter().any(|x| x == "dir=in"));
    assert!(add.iter().any(|x| x == "action=allow"));
    assert!(add.iter().any(|x| x == "localport=5201"));
    assert!(
        add.iter().any(|x| x == "protocol=TCP"),
        "协议应归一化为大写"
    );

    let rm = build_firewall_args_remove(5201, "UDP").unwrap();
    assert!(rm.iter().any(|x| x == "delete"));
    assert!(rm.iter().any(|x| x == "rule"));
}

#[test]
fn 防火墙_拒绝非法协议与端口() {
    assert!(build_firewall_args_add(5201, "ICMP").is_err());
    assert!(build_firewall_args_add(0, "TCP").is_err());
}

#[test]
fn 防火墙_动作白名单() {
    // 非法动作应被拒（本测试不依赖是否以管理员运行）
    assert!(apply_firewall_rule("evil", 5201, "TCP").is_err());
}

// ── 结果模型 ────────────────────────────────────────────────────────────

#[test]
fn 结果_结束条件标签() {
    let mut o = opt("h");
    assert_eq!(end_condition_label(&o), "10 秒");
    o.end_mode = EndMode::Bytes;
    o.bytes = Some("100M".into());
    assert_eq!(end_condition_label(&o), "100M 字节");
    o.end_mode = EndMode::Blocks;
    o.blockcount = Some("500".into());
    assert_eq!(end_condition_label(&o), "500 块");
}

#[test]
fn 结果_方向标签随参数变化() {
    let mut o = opt("h");
    let r = Iperf3Result::empty(&o, "iperf 3.21", true);
    assert_eq!(r.direction, "正向");
    o.reverse = true;
    assert_eq!(Iperf3Result::empty(&o, "v", true).direction, "反向");
    o.reverse = false;
    o.bidir = true;
    assert_eq!(Iperf3Result::empty(&o, "v", true).direction, "双向");
    assert_eq!(Iperf3Result::empty(&o, "v", true).end_condition, "10 秒");
}

#[test]
fn 结果_空统计判定与参数快照() {
    let d = Iperf3DirectionSummary::default();
    assert!(d.is_empty());
    let d2 = Iperf3DirectionSummary {
        mbps: Some(1.0),
        ..Default::default()
    };
    assert!(!d2.is_empty());

    let mut o = opt("example.com");
    o.port = 5300;
    o.protocol = Protocol::Udp;
    o.parallel = 4;
    let r = Iperf3Result::empty(&o, "iperf 3.21", true);
    assert_eq!(r.target, "example.com");
    assert_eq!(r.port, 5300);
    assert_eq!(r.protocol, "UDP");
    assert_eq!(r.protocol_key, "udp");
    assert_eq!(r.parallel, 4);
    assert!(r.json_stream);
}

// ── 反序列化契约（前端字段名）──────────────────────────────────────────

#[test]
fn 反序列化_前端camelCase字段() {
    // 前端按 camelCase 传参，必须与 serde rename_all 对齐
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
      "udpCounters64bit": true
    }"#;
    let o: Iperf3ClientOptions = serde_json::from_str(json).expect("应能反序列化");
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
}

#[test]
fn 反序列化_缺省值对齐iperf3默认() {
    let o: Iperf3ClientOptions =
        serde_json::from_str(r#"{"host":"example.com"}"#).expect("最小参数应可反序列化");
    assert_eq!(o.port, 5201);
    assert_eq!(o.protocol, Protocol::Tcp);
    assert_eq!(o.end_mode, EndMode::Time);
    assert_eq!(o.time, 10);
    assert_eq!(o.parallel, 1);
    assert_eq!(o.interval, 1);
    assert_eq!(o.format, "m");
    assert!(!o.reverse);
    assert!(!o.bidir);
}

// ── 二进制/版本定位（不依赖真实安装）────────────────────────────────────

#[test]
fn 定位_不存在的覆盖路径被忽略() {
    set_exe_override(Some(PathBuf::from("__no_such_iperf3_binary__.exe")));
    let got = iperf3_exe(None);
    assert!(
        got.as_deref() != Some(Path::new("__no_such_iperf3_binary__.exe")),
        "不存在的路径不应作为解析结果"
    );
    set_exe_override(None);
    invalidate_cache();
}

#[test]
fn 定位_PATH查找不崩溃() {
    let _ = find_on_path("__definitely_not_on_path_7a2b__.exe");
}

#[test]
fn 定位_二进制缺失时优雅降级() {
    // 指向不存在的路径 → 若本机也无 PATH 安装，则应报告不可用且给出说明
    set_exe_override(Some(PathBuf::from("__no_such_iperf3_binary__.exe")));
    invalidate_cache();
    let info = tool_info(None);
    if !info.available {
        assert!(!info.note.is_empty(), "缺失时必须给出可读说明，不能空白");
        assert_eq!(info.version, "未知");
    }
    set_exe_override(None);
    invalidate_cache();
}
