//! 采集链路回归测试 — 异常数据 / 错误契约 / 边界场景
//!
//! 本文件补充现有测试未覆盖的确定性边界用例（不发起网络请求、不修改系统状态）：
//! - R8 错误消息契约：`CollectError` → `ds_util::err_message` 映射含 API 名/操作名/详情
//! - HTTP 白名单边界：协议混淆 / 前缀截断 / 空串 / 大小写
//! - 链路速度格式化边界：0 / 阈值 / 小数精度
//! - 超时契约：`DEFAULT_TIMEOUT_SECS == 5`（与现状 PowerShell -TimeoutSec 5 一致）
//! - 错误类型分级：全部变体可匹配、可序列化为用户可见消息
//!
//! 运行：cargo test --test ds_regression_edge
//! 注：配合 `--features legacy-data-source` 可验证回滚链路同样通过（接口兼容性回归）。

use secm_lib::datasource as ds;

// ============================================================================
// R8 错误消息契约（ds_util::err_message 映射）
// ============================================================================

#[test]
fn err_message_winapi_contains_api_op_detail() {
    let e = ds::CollectError::winapi_detailed(
        "EnumServicesStatusExW",
        "枚举系统服务",
        "错误码 5",
    );
    let msg = secm_lib::ds_util::err_message(&e);
    // 契约：中文操作描述: [API] 详情 — 三者缺一不可
    assert!(msg.contains("枚举系统服务"), "缺操作名: {}", msg);
    assert!(msg.contains("EnumServicesStatusExW"), "缺 API 名: {}", msg);
    assert!(msg.contains("错误码 5"), "缺详情: {}", msg);
}

#[test]
fn err_message_all_variants_covered() {
    // 六个变体必须全部映射为用户可见中文消息（不允许漏配 → 走默认分支）
    let cases = [
        (
            ds::CollectError::winapi("GetAdaptersAddresses", "获取本地 IP"),
            "获取本地 IP",
        ),
        (
            ds::CollectError::registry(
                r"HKLM\SOFTWARE\Test",
                "拒绝访问",
            ),
            "注册表读取失败",
        ),
        (
            ds::CollectError::http("http://ip-api.com/json/", "连接失败"),
            "网络请求失败",
        ),
        (
            ds::CollectError::parse("HTTP URL", "'x' 不在白名单内"),
            "数据解析失败",
        ),
        (
            ds::CollectError::NeedsAdmin {
                op: "刷新 DNS 缓存".into(),
            },
            "需要管理员权限",
        ),
        (
            ds::CollectError::NotFound {
                what: "电源计划".into(),
            },
            "数据不存在",
        ),
    ];
    for (e, expect_prefix) in cases {
        let msg = secm_lib::ds_util::err_message(&e);
        assert!(
            msg.contains(expect_prefix),
            "变体未正确映射: {:?} -> {} (期望含 {})",
            e,
            msg,
            expect_prefix
        );
        assert!(!msg.is_empty());
    }
}

#[test]
fn collect_error_display_has_structured_format() {
    // thiserror Display 应含 API/操作/详情（R8 结构化错误）
    let e = ds::CollectError::winapi_detailed(
        "PowerGetActiveScheme",
        "读取电源计划",
        "错误码 13",
    );
    let disp = format!("{}", e);
    assert!(disp.contains("PowerGetActiveScheme"));
    assert!(disp.contains("读取电源计划"));
    assert!(disp.contains("错误码 13"));
}

// ============================================================================
// HTTP 白名单边界（R4 安全 — 前缀匹配的绕过尝试）
// ============================================================================

#[test]
fn allowlist_rejects_protocol_mixup() {
    // http 白名单登记，https 变体必须拒绝（协议混淆）
    assert!(ds::http::validate_url("https://ip-api.com/json/").is_err());
    assert!(ds::http::validate_url("ftp://ip-api.com/json/").is_err());
}

#[test]
fn allowlist_rejects_domain_squatting() {
    // 域名伪造：ip-api.com 的仿冒域名（白名单是前缀匹配，必须防域名截断绕过）
    assert!(ds::http::validate_url("http://ip-api.com.evil.com/json/").is_err());
    assert!(ds::http::validate_url("http://evil-ip-api.com/json/").is_err());
    // 端口变体不在白名单前缀内 → 拒绝（严格前缀匹配）
    assert!(ds::http::validate_url("http://ip-api.com:8080/json/").is_err());
}

#[test]
fn allowlist_edge_inputs() {
    // 空串 / 纯空白 / 无协议
    assert!(ds::http::validate_url("").is_err());
    assert!(ds::http::validate_url("   ").is_err());
    assert!(ds::http::validate_url("ip-api.com/json/").is_err());
}

#[test]
fn timeout_contract_is_5_seconds() {
    // 与现状 PowerShell Invoke-RestMethod -TimeoutSec 5 一致（设计文档 §4.3）
    assert_eq!(ds::http::DEFAULT_TIMEOUT_SECS, 5);
}

// ============================================================================
// 链路速度格式化边界（fmt_speed_bps）
// ============================================================================

#[test]
fn speed_format_zero_and_thresholds() {
    assert_eq!(ds::netif::fmt_speed_bps(0), "0 bps");
    assert_eq!(ds::netif::fmt_speed_bps(999), "999 bps");
    assert_eq!(ds::netif::fmt_speed_bps(1000), "1 Kbps");
    assert_eq!(ds::netif::fmt_speed_bps(999_999), "1000 Kbps");
    assert_eq!(ds::netif::fmt_speed_bps(1_000_000), "1 Mbps");
    assert_eq!(ds::netif::fmt_speed_bps(1_000_000_000), "1 Gbps");
    assert_eq!(ds::netif::fmt_speed_bps(2_500_000_000), "2.5 Gbps");
    assert_eq!(ds::netif::fmt_speed_bps(10_000_000_000), "10 Gbps");
}

#[test]
fn speed_format_matches_contract_regex() {
    // 设计文档 §11：格式匹配 ^\d+(\.\d+)? (G|M|K)?bps$
    let samples = [
        ds::netif::fmt_speed_bps(0),
        ds::netif::fmt_speed_bps(123_456),
        ds::netif::fmt_speed_bps(123_456_789),
        ds::netif::fmt_speed_bps(1_500_000_000),
        ds::netif::fmt_speed_bps(u64::MAX / 2),
    ];
    for s in samples {
        let ok = regex_like_check(&s);
        assert!(ok, "速度格式非法: {}", s);
    }
}

/// 轻量正则等价检查（不引入 regex 依赖）：数字(可选小数) + 单位
fn regex_like_check(s: &str) -> bool {
    if !s.ends_with("bps") {
        return false;
    }
    let core = &s[..s.len() - 3].trim();
    if core.is_empty() {
        return false;
    }
    // 数字部分
    let mut idx = 0;
    let mut saw_dot = false;
    for (i, c) in core.chars().enumerate() {
        if c.is_ascii_digit() {
            idx = i + 1;
        } else if c == '.' && !saw_dot && idx > 0 {
            saw_dot = true;
            idx = i + 1;
        } else {
            break;
        }
    }
    let rest = core[idx..].trim();
    rest.is_empty() || matches!(rest, "G" | "M" | "K")
}

// ============================================================================
// 注册表补丁标题清洗边界（异常数据 → 空串/乱码不 panic）
// ============================================================================

#[test]
fn patch_title_cleanup_tolerates_garbage() {
    // 空标题 / 全空白 / 无分隔符 — 均应返回非空或安全占位，不 panic
    let cases = [
        "",
        "   ",
        "更新程序",
        "Security Update for Windows",
        "2026-07 Cumulative Update for Windows 11 Version 24H2 for x64-based Systems (KB5101650)",
    ];
    for c in cases {
        let r = ds::registry::translate_patch_title(c);
        // 契约：返回串可为原样或清洗后，但必须是 String 且不 panic
        assert!(!r.is_empty() || c.is_empty(), "输入 {:?} -> {:?}", c, r);
    }
}

// ============================================================================
// 服务查询边界（不存在服务 → Ok(None)，不 panic）
// ============================================================================

#[test]
fn missing_service_edge_cases() {
    // 空名 / 超长名 / 不存在名 — 均不允许 panic（返回 Ok/Err 皆可，但不能崩溃）
    for name in ["", " ", "SECM_No_Such_Service_XYZ", "A".repeat(300).as_str()] {
        let r = ds::service::query_service(name);
        // 不允许 panic；Ok(None) 或 Err 均符合降级语义
        let _ = r;
    }
}

// ============================================================================
// 版本契约（与父任务交付一致）
// ============================================================================

#[test]
fn datasource_crate_path_contract() {
    // 依赖路径契约：业务模块经 secm-datasource 采集（crate 顶层可访问模块与错误类型）
    // 编译期即验证 path 依赖 + pub use 链正常（lib.rs re-export CollectError）
    let e: ds::CollectError = ds::CollectError::NotFound {
        what: "版本契约验证".into(),
    };
    assert!(format!("{}", e).contains("数据不存在"));
}
