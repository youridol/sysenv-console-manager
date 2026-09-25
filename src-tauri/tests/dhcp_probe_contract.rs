//! 回归护栏：DHCP 服务器检测必须覆盖**全部**网段，而不是只探测一个"最佳"接口。
//!
//! ## 这个 BUG 的用户可见现象
//!
//! 「无法正确检查当前内网所有的 dhcp 服务器」——多宿主机器（Hyper-V 虚拟交换机、
//! 多张物理网卡、虚拟机网卡）上每个网段都可能有独立的 DHCP 服务器，而旧实现
//! `read_network_config()` 只保留**评分最高的单个接口**（`best_score` + 覆盖赋值），
//! 探测也只绑该接口 IP、只发 255.255.255.255 + 网关单播。
//!
//! 本机实测（两网段各自都有真实 DHCP 服务器）：
//!   - A 192.168.3.0/24（vEthernet External）→ DHCP 服务器 192.168.3.254
//!   - B 192.168.160.0/20（vEthernet Default Switch）→ DHCP 服务器 192.168.160.1（Windows ICS）
//! 修复前 `probed=["192.168.3.254"]`（只发现 A）；修复后
//! `probed=["192.168.3.254", "192.168.160.1"]`（两个网段都发现）。
//!
//! ## 两个必须同时成立的根因
//!
//! 1. **单网段**：只探测一个接口 → 其它网段的服务器永远不可见。
//! 2. **chaddr 用了真实网卡 MAC**：`get_primary_mac()` 取 sysinfo 枚举到的第一个
//!    非零 MAC，可能是**另一张**网卡的地址。实测对 B 网段 Windows ICS 的 DHCP
//!    服务器：三个真实网卡 MAC 全部 0/6 应答；合成（本地管理）或全零 MAC 全部
//!    6/6 应答——即该服务器按 chaddr 丢弃请求。
//!
//! ## 防护层次
//!
//! - 单元测试（`src/dhcp_probe.rs` 的 `tests` 模块）：纯函数级——网段去重、
//!   定向广播计算、包预算/窗口分配、合成 MAC 属性。
//! - 本文件（源码契约 + 真实探测）：断言语义层面的设计不被回退，
//!   并在本机确实有多个 DHCP 服务器时给出可观测证据。
//!
//! 静态断言基于**源码文本**，不需要启动 GUI 或 WebView2。

use std::path::PathBuf;

fn read_probe_source() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/dhcp_probe.rs");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("无法读取 {}: {}", p.display(), e))
}

/// 只保留**代码行**，丢掉注释行。
///
/// 必要性：模块头注释里有意保留了对旧实现（`get_primary_mac()`、
/// `best_score` 等）的说明，用来记录 BUG 历史。静态契约断言必须针对
/// 可执行代码，否则会被"解释性文字"误伤（或反过来被文字哄过）。
fn probe_code_only() -> String {
    read_probe_source()
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with("///") && !t.starts_with("//") && !t.starts_with("//!")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A. 探测必须遍历**全部**网段（不得回退成"只挑一个最佳接口"）。
#[test]
fn probe_iterates_all_segments_not_just_one() {
    let code = probe_code_only();

    assert!(
        code.contains("segments: Vec<Segment>"),
        "NetConfig 必须持有**多个**网段（Vec<Segment>）。\n\
         旧实现只有单个 interface_ip/gateway/subnet_mask 三个 Option 字段，\n\
         多宿主机器上必然漏掉其它网段的 DHCP 服务器（本 BUG 的根因）。"
    );
    assert!(
        code.contains("for (idx, seg) in segments.iter().enumerate()"),
        "探测主流程必须逐个网段轮转（不得只探测第一个网段）。"
    );
    // 旧实现的特征写法：单接口的 best_score 覆盖赋值
    assert!(
        !code.contains("best_score"),
        "代码中不应再有 best_score 单接口选择逻辑——那正是漏报多网段服务器的写法。"
    );
    // 网段上限必须仍存在（防包预算/耗时失控）
    assert!(
        code.contains("MAX_SEGMENTS"),
        "必须保留网段数上限（受 DISCOVER 包预算约束）。"
    );

    // **多网段的下限**：上限必须 > 1，且收集网段时**真的传入该上限**。
    //
    // 为什么需要这条：仅断言"存在 Vec<Segment> + 轮转循环"不足以守护本 BUG——
    // 把上限写死成 1（等价于旧的"只挑一个接口"）仍满足上述所有断言，
    // 且实测确实能绕过。下面把"必须能装下多个网段"钉死。
    //
    // 上限可能写成字面量，也可能引用别的常量
    // （`const MAX_SEGMENTS: usize = MAX_DISCOVER_PACKETS;`），故需解析一层间接引用。
    let max_seg_value = resolve_usize_const(&code, "MAX_SEGMENTS").unwrap_or_else(|| {
        panic!(
            "无法解析 MAX_SEGMENTS 的数值（支持字面量与对其它常量的单层引用）。\
             该值必须可静态校验，以便本护栏确认其 > 1。"
        )
    });
    assert!(
        max_seg_value > 1,
        "MAX_SEGMENTS 解析为 {max_seg_value}，等价于只探测一个网段——\
         这正是本 BUG 的形态（多宿主机器漏报其它网段的 DHCP 服务器）。"
    );

    // 收集网段时必须传入 MAX_SEGMENTS（而不是写死 1）
    assert!(
        code.contains("dedup_and_limit_segments(&ordered, MAX_SEGMENTS)")
            || code.contains("dedup_and_limit_segments(&ordered,MAX_SEGMENTS)"),
        "网段收集必须使用 MAX_SEGMENTS 作为上限（写死字面量 1 会退化成单网段探测）。"
    );
}

/// 解析 `const <name>: usize = <字面量 | 另一常量名>;` 的数值（单层间接引用）。
///
/// 返回 None 表示未找到该常量、形式无法识别、或引用了无法解析的常量。
fn resolve_usize_const(code: &str, name: &str) -> Option<usize> {
    let prefix = format!("const {name}: usize =");
    let value_token = code.lines().find_map(|l| {
        let t = l.trim().trim_end_matches(';').trim();
        let rest = t.strip_prefix(&prefix)?;
        Some(rest.trim().to_string())
    })?;

    // 字面量
    if let Ok(v) = value_token.parse::<usize>() {
        return Some(v);
    }
    // 单层引用：递归解析被引用的常量（深度受限，避免循环引用导致栈溢出）
    if value_token
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return resolve_usize_const(code, &value_token);
    }
    None
}

/// B. 必须使用 subnet_mask 计算**定向广播**（旧实现读到了掩码却从未使用）。
#[test]
fn directed_broadcast_is_derived_from_subnet_mask() {
    let code = probe_code_only();

    assert!(
        code.contains("fn directed_broadcast"),
        "必须有 directed_broadcast()——由 ip|~mask 计算子网定向广播。"
    );
    assert!(
        code.contains("| !m"),
    "定向广播必须由掩码推导（ip | !mask），实际代码中未见该运算。"
    );
    // 发送通道里必须真的用上它
    assert!(
        code.contains("seg.directed_broadcast()") || code.contains(".directed_broadcast()"),
        "探测目标必须包含该网段的定向广播——部分服务器/防火墙丢弃 255.255.255.255。"
    );
}

/// C. 必须逐网段**顺序**探测（一次只持有一个 :68 socket）。
///
/// 实测：同时绑定两个 :68 socket（各绑不同接口 IP）会互相抢收，
/// 两个网段里只有一个能收到 OFFER。
#[test]
fn probes_segments_sequentially_one_socket_at_a_time() {
    let code = probe_code_only();

    assert!(
        code.contains("fn bind_probe_socket"),
        "必须保留 SO_REUSEADDR 绑定 68 的设计（与 Windows Dhcp 服务共存）。"
    );
    assert!(
        code.contains("set_reuse_address(true)"),
        "必须保留 SO_REUSEADDR。"
    );
    // 顺序性：socket 在循环体内创建（而非循环外一次创建）
    let loop_pos = code
        .find("for (idx, seg) in segments.iter().enumerate()")
        .expect("应有网段轮转循环");
    let body = &code[loop_pos..];
    assert!(
        body.contains("bind_probe_socket"),
        "socket 必须在**网段循环体内**创建——循环外只建一个 socket 会漏网段；\n\
         同时持有多个 :68 socket 又会互相抢收。"
    );
}

/// D. chaddr 不得再取自真实网卡（`get_primary_mac`）。
#[test]
fn chaddr_is_synthetic_not_a_real_nic_mac() {
    let code = probe_code_only();

    assert!(
        code.contains("fn synthetic_client_mac"),
        "必须用合成 chaddr（synthetic_client_mac）。"
    );
    assert!(
        !code.contains("get_primary_mac"),
        "不得再用 get_primary_mac()：它取任意真实网卡 MAC，\
         实测会让只认『非本机真实 MAC』的 DHCP 服务器（如 Windows ICS）不应答。"
    );
    assert!(
        !code.contains("sysinfo::Networks"),
        "不应再用 sysinfo 枚举网卡取 chaddr。"
    );
    // 必须真的把合成 MAC 用于探测报文
    assert!(
        code.contains("build_discover(xid, &synthetic_client_mac())"),
        "探测报文必须使用 synthetic_client_mac() 作为 chaddr。"
    );
}

/// E. 反广播风暴上限必须保留（单次检测 ≤ 4 个 DISCOVER 包）。
#[test]
fn discover_packet_cap_is_preserved() {
    let code = probe_code_only();
    assert!(
        code.contains("MAX_DISCOVER_PACKETS: usize = 4"),
        "必须保留 DISCOVER 包总数上限 4（防广播风暴）；\
         多网段探测不得通过提高该上限来『解决』覆盖问题。"
    );
    assert!(
        code.contains("per_segment_allowance"),
        "多网段共享包预算时必须有预留逻辑（保证每网段至少 1 包）。"
    );
}

/// F. 结果契约不得改变（前端按这些字段渲染）。
#[test]
fn result_contract_is_unchanged() {
    let code = probe_code_only();
    for field in [
        "pub servers: Vec<String>",
        "pub count: usize",
        "pub healthy: bool",
        "pub baseline_server: Option<String>",
        "pub note: String",
    ] {
        assert!(
            code.contains(field),
            "DhcpProbeResult 契约字段缺失或改名：{field}\n\
             （前端 src/pages/Network.tsx 按 servers/count/healthy/baseline_server/note 渲染）"
        );
    }
    // 深度检查契约
    for field in [
        "pub local_ip: Option<String>",
        "pub gateway: Option<String>",
        "pub subnet_mask: Option<String>",
        "pub checks: Vec<DhcpServerCheck>",
        "pub summary: String",
    ] {
        assert!(
            code.contains(field),
            "DhcpDeepCheckResult 契约字段缺失或改名：{field}"
        );
    }
    // note 的"降级端口局限"文案必须保留（含中文文案，故在原始源码里查）
    assert!(
        read_probe_source().contains("降级为随机源端口探测"),
        "note 的降级局限提示文案必须保留（绑定 68 失败时的用户可见说明）。"
    );
}

/// G. 真实探测：结果结构自洽，且**多网段能力**可观测。
///
/// 本机确实有两个网段的真实 DHCP 服务器（192.168.3.254 与 192.168.160.1），
/// 因此这里给出可观测证据。网络环境会变化（离开本机/换网络），
/// 所以断言只针对**结构性不变量**，发现数量作为信息打印。
#[test]
fn live_probe_result_is_structurally_consistent() {
    let r = match secm_lib::dhcp_probe::probe_dhcp_servers() {
        Ok(r) => r,
        Err(e) => panic!("probe_dhcp_servers() 失败: {e}"),
    };

    eprintln!("[dhcp-contract] servers={:?}", r.servers);
    eprintln!("[dhcp-contract] count={} healthy={}", r.count, r.healthy);
    eprintln!("[dhcp-contract] baseline={:?}", r.baseline_server);
    eprintln!("[dhcp-contract] note={}", r.note);

    assert_eq!(r.count, r.servers.len(), "count 必须等于 servers.len()");
    assert_eq!(r.healthy, r.count <= 1, "healthy 必须等价于 count<=1");
    assert!(!r.note.is_empty(), "note 不应为空（前端会渲染）");
    // 去重性
    let mut uniq = r.servers.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), r.servers.len(), "servers 不应含重复项");
    // 每项都是合法 IPv4
    for s in &r.servers {
        assert!(
            s.parse::<std::net::Ipv4Addr>().is_ok(),
            "servers 中的 {s:?} 不是合法 IPv4"
        );
    }
    // 基线（若存在）应已被并入结果
    if let Some(b) = &r.baseline_server {
        assert!(
            r.servers.contains(b),
            "基线服务器 {b} 必须出现在最终 servers 中（基线应与主动探测并集）"
        );
    }
}
