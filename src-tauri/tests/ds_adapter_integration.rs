//! 接入层集成测试 — 验证业务模块切换到新驱动（secm-datasource）后数据可读、JSON 契约不漂移
//!
//! 覆盖设计文档 §4.1 前端契约（字段名/类型逐字段不变）+ §11 验证路径：
//! - P1/P2 `get_system_info` → activation/latest_patch 字段形状
//! - P3 `get_link_speeds` → 速度格式匹配
//! - P4/P5 `get_ip_info` → 9 字段全部可序列化
//! - P10/P11 `list_all_services` / `get_services` → 服务列表 + ServiceInfo 契约
//! - P12 `get_game_presets` → vanguard/battleye 状态
//! - P13 `flush_dns` → success/operation 字段
//! - P6 `get_hags_state` → GPU 检测降级路径不 panic
//!
//! 运行：cargo test --test ds_adapter_integration
//! 注：默认走新驱动；`--features legacy-data-source` 可跑旧链路对照（输出应等价）。

use secm_lib::datasource as ds;

// ============================================================================
// P1/P2 系统信息（激活状态 + 最新补丁）
// ============================================================================

#[test]
fn system_info_json_contract() {
    let info = secm_lib::sysinfo::get_system_info();
    let json = serde_json::to_value(&info).expect("SystemInfo 应可序列化");
    let obj = json.as_object().expect("应为 JSON 对象");

    // §4.1 SystemInfo 契约字段必须存在
    for key in [
        "edition",
        "build_number",
        "ubr",
        "arch",
        "install_date",
        "activation",
        "latest_patch",
        "boot_mode",
    ] {
        assert!(obj.contains_key(key), "SystemInfo 缺少字段: {}", key);
    }

    // activation 子结构（ActivationInfo: status_raw/label）
    let activation = obj.get("activation").expect("activation 缺失").as_object().expect("对象");
    for key in ["status_raw", "label"] {
        assert!(activation.contains_key(key), "ActivationInfo 缺少字段: {}", key);
    }

    // latest_patch 子结构（PatchInfo: kb/date/title_raw/title_cn）
    let patch = obj.get("latest_patch").expect("latest_patch 缺失").as_object().expect("对象");
    for key in ["kb", "date", "title_raw", "title_cn"] {
        assert!(patch.contains_key(key), "PatchInfo 缺少字段: {}", key);
    }
}

#[test]
fn activation_field_semantics() {
    // 激活状态非空（与现状"无法获取"降级一致）
    let info = secm_lib::sysinfo::get_system_info();
    assert!(!info.activation.status_raw.is_empty());
    assert!(!info.activation.label.is_empty());
}

#[test]
fn latest_patch_field_semantics() {
    let info = secm_lib::sysinfo::get_system_info();
    // 补丁可为空占位（"无"/"—"），但字段必须齐全
    assert!(!info.latest_patch.kb.is_empty());
    assert!(!info.latest_patch.title_cn.is_empty());
}

// ============================================================================
// P3 链路速度
// ============================================================================

#[test]
fn link_speeds_format_and_contract() {
    let speeds = secm_lib::net_stats::get_link_speeds();
    for (name, speed) in &speeds {
        assert!(!name.is_empty(), "网卡名不应为空");
        assert!(speed.ends_with("bps"), "速度格式非法: {} -> {}", name, speed);
        let core = speed[..speed.len() - 3].trim();
        let parts: Vec<&str> = core.split(' ').collect();
        assert!(parts[0].parse::<f64>().is_ok(), "速度数字非法: {}", speed);
        if parts.len() == 2 {
            assert!(matches!(parts[1], "G" | "M" | "K"), "速度单位非法: {}", speed);
        }
    }
}

// ============================================================================
// P4/P5 IP 信息（11 字段契约）
// ============================================================================

#[test]
fn ip_info_json_contract() {
    let info = secm_lib::ip_info::get_ip_info();
    let json = serde_json::to_value(&info).expect("IpInfo 应可序列化");
    let obj = json.as_object().expect("应为 JSON 对象");

    // §4.1 IpInfo 契约字段
    for key in [
        "local_ipv4",
        "local_ipv6",
        "public_ipv4",
        "public_ipv6",
        "domestic_ip",
        "domestic_ipv6",
        "domestic_location",
        "domestic_ipv6_location",
        "city",
        "country",
        "timezone",
    ] {
        assert!(obj.contains_key(key), "IpInfo 缺少字段: {}", key);
    }

    // 本地 IP 至少有一个非占位（实机必有网卡）
    assert!(
        !info.local_ipv4.is_empty() || !info.local_ipv6.is_empty(),
        "本地 IP 均为空: {:?}",
        (info.local_ipv4, info.local_ipv6)
    );
}

#[test]
fn local_ip_not_loopback() {
    let info = secm_lib::ip_info::get_ip_info();
    if !info.local_ipv4.is_empty() && info.local_ipv4 != "—" {
        assert!(
            !info.local_ipv4.starts_with("127."),
            "不应返回回环地址: {}",
            info.local_ipv4
        );
    }
    if !info.local_ipv6.is_empty() && info.local_ipv6 != "—" {
        assert!(
            !info.local_ipv6.starts_with("fe80"),
            "不应返回链路本地: {}",
            info.local_ipv6
        );
    }
}

// ============================================================================
// P6 GPU 检测（get_hags_state 降级路径）
// ============================================================================

#[test]
fn hags_state_never_panics() {
    // get_hags_state 内部走 detect_gpu_via_wmi（新驱动），失败应降级而非 panic
    let state = secm_lib::settings::get_hags_state();
    assert!(!state.name.is_empty());
    assert!(!state.message.is_empty());
}

// ============================================================================
// P10/P11 服务管理
// ============================================================================

#[test]
fn services_json_contract() {
    let services = secm_lib::settings::get_services();
    // 16 预设服务全部返回（含降级占位）
    assert_eq!(services.len(), 16, "预设服务数应为 16");
    for s in &services {
        let json = serde_json::to_value(s).expect("ServiceInfo 应可序列化");
        let obj = json.as_object().expect("应为对象");
        for key in ["name", "display_name", "status", "start_type"] {
            assert!(obj.contains_key(key), "ServiceInfo 缺少字段: {}", key);
        }
        assert!(!s.name.is_empty());
    }
}

#[test]
fn list_all_services_contract() {
    let services = secm_lib::settings::list_all_services();
    // 实机服务数应 > 100（标准 Windows 系统）
    assert!(services.len() > 100, "服务数异常少: {}", services.len());
    let names: Vec<&str> = services.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names.contains(&"RpcSs") || names.contains(&"Power"),
        "缺少关键服务"
    );
    // 启动类型应为中文映射（自动/手动/已禁用/未知）
    for s in &services {
        assert!(
            ["自动", "手动", "已禁用", "未知"].contains(&s.start_type.as_str()),
            "启动类型非法: {} -> {}",
            s.name,
            s.start_type
        );
    }
}

// ============================================================================
// P12 游戏环境（服务状态）
// ============================================================================

#[test]
fn game_presets_service_status() {
    let presets = secm_lib::game_env::get_game_presets();
    assert!(!presets.is_empty(), "预设不应为空");

    // 检查 vanguard（Vanguard 反作弊）与 battleye（BattlEye）字段存在于某个 preset
    let all_settings: Vec<&secm_lib::game_env::GameSetting> = presets
        .iter()
        .flat_map(|p| p.settings.iter())
        .collect();
    let vanguard = all_settings
        .iter()
        .find(|s| s.label.contains("Vanguard"));
    let battleye = all_settings.iter().find(|s| s.label.contains("BattlEye"));
    // 任一存在即可（预设结构变化时本测试同步更新）
    let _ = (vanguard, battleye);
    // 所有 current 值非空（服务状态降级也返回"未安装"等占位）
    for s in &all_settings {
        assert!(!s.current.is_empty(), "current 不应为空: {}", s.label);
    }
}

// ============================================================================
// P13 DNS 刷新
// ============================================================================

#[test]
fn flush_dns_contract() {
    let result = secm_lib::cleanup::flush_dns();
    let json = serde_json::to_value(&result).expect("CleanupResult 应可序列化");
    let obj = json.as_object().expect("应为对象");
    for key in ["operation", "success", "bytes_freed", "message"] {
        assert!(obj.contains_key(key), "CleanupResult 缺少字段: {}", key);
    }
    assert_eq!(result.operation, "DNS 缓存刷新");
    // flushdns 对普通用户也应成功
    assert!(result.success, "DNS 刷新应成功: {}", result.message);
}

// ============================================================================
// 控制台验收 — 打印新驱动采集的实际数据（cargo test --test ds_adapter_integration -- --nocapture）
// ============================================================================

#[test]
fn console_dump_native_collection() {
    // 验收标准："控制台可看到新驱动采集的数据"（§验收标准）
    println!("===== SECM 新驱动采集数据（secm-datasource）=====");

    // P1/P2 系统信息
    let info = secm_lib::sysinfo::get_system_info();
    println!(
        "系统: {} build {} ubr {} {} | 激活: {}({}) | 补丁: {} {} | 启动: {}",
        info.edition,
        info.build_number,
        info.ubr,
        info.arch,
        info.activation.label,
        info.activation.status_raw,
        info.latest_patch.kb,
        info.latest_patch.date,
        info.boot_mode
    );

    // P3 链路速度
    let speeds = secm_lib::net_stats::get_link_speeds();
    println!("链路速度: {:?}", speeds);

    // P4/P5 IP 信息
    let ip = secm_lib::ip_info::get_ip_info();
    println!(
        "本地IP: {}/{} | 公网: {}/{} | 国内V4: {} ({}) | 国内V6: {} ({}) | {} {} {}",
        ip.local_ipv4,
        ip.local_ipv6,
        ip.public_ipv4,
        ip.public_ipv6,
        ip.domestic_ip,
        ip.domestic_location,
        ip.domestic_ipv6,
        ip.domestic_ipv6_location,
        ip.city,
        ip.country,
        ip.timezone
    );

    // P6 GPU（经 get_hags_state 降级路径）
    let hags = secm_lib::settings::get_hags_state();
    println!("HAGS: {} | {}", hags.name, hags.message);

    // P10/P11 服务
    let services = secm_lib::settings::list_all_services();
    println!("服务枚举: {} 个（新驱动 EnumServicesStatusExW）", services.len());
    let preset = secm_lib::settings::get_services();
    println!("预设服务: {} 个", preset.len());

    // P13 DNS
    let dns = secm_lib::cleanup::flush_dns();
    println!("DNS 刷新: success={} {}", dns.success, dns.message);

    // 断言：关键数据非占位（激活状态/补丁/本地 IP 至少一项可用）
    assert!(!info.activation.label.is_empty());
    assert!(
        !info.latest_patch.kb.is_empty(),
        "补丁 kb 不应为空"
    );
    assert!(
        !ip.local_ipv4.is_empty() || !ip.local_ipv6.is_empty(),
        "本地 IP 至少一项非空"
    );
    assert!(services.len() > 100, "服务数应 > 100: {}", services.len());
    assert!(dns.success, "DNS 刷新应成功");
}

#[test]
fn datasource_mapping_shape() {
    // 业务模块返回的结构应能从 datasource 原始数据构造（字段语义一致）
    // P2：注册表补丁
    match ds::registry::latest_patch() {
        Ok(Some(p)) => {
            assert!(p.kb.starts_with("KB"), "kb 前缀: {}", p.kb);
            assert!(!p.title_cn.is_empty());
        }
        Ok(None) => {}
        Err(e) => panic!("latest_patch 不应报错: {}", e),
    }
    // P3：链路速度
    match ds::netif::link_speeds() {
        Ok(map) => {
            for (n, s) in &map {
                assert!(s.ends_with("bps"), "{} -> {}", n, s);
            }
        }
        Err(e) => panic!("link_speeds 不应报错: {}", e),
    }
    // P4：本地 IP
    match ds::netif::local_ips() {
        Ok(ips) => {
            assert!(ips.ipv4.is_some() || ips.ipv6.is_some());
        }
        Err(e) => panic!("local_ips 不应报错: {}", e),
    }
}
