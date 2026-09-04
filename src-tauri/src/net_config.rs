/// 网络配置应用模块 — 静态 IPv4/IPv6 地址、掩码、网关、DNS 与 MAC 修改
///
/// 原理与选型：
/// - IPv4/IPv6 地址、网关、DNS 通过 netsh.exe 应用（Windows 原生网络配置接口，
///   支持 DHCP ⇄ 静态切换，比 iphlpapi SetIPAddrTable 可靠且无需重启）；
/// - MAC 修改走注册表 `NetworkAddress` + 重启网卡（高级功能，应用前备份原值）；
/// - 网络配置修改需管理员权限：命令层先检查 is_admin，非管理员返回明确错误
///   （程序本身 asInvoker 启动，不弹 UAC；netsh 需提权上下文执行）；
/// - 编码：netsh 输出为 ANSI（中文系统 GBK），encoding_rs 解码避免乱码。
///
/// 安全（R4）：
/// - 所有参数一律经 `Command::args` 传递，不经 shell，无命令注入面；
/// - netsh 的 `name="..."` 引号由本模块显式构造，接口名内嵌引号被剔除；
/// - MAC 写入前备份原值，修改注册表路径固定（网卡 Class 键），无路径穿越。
///
/// 参考：`netsh interface ip` / `interface ipv6` 命令参考（Windows 文档）。
use serde::{Deserialize, Serialize};
use std::process::Command;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE};
use winreg::RegKey;

// ============================================================================
// 常量
// ============================================================================

/// 网卡驱动 Class 注册表键（枚举 0000..00XX 实例）
const NIC_CLASS_KEY: &str =
    r"SYSTEM\CurrentControlSet\Control\Class\{4D36E972-E325-11CE-BFC1-08002BE10318}";
/// 注册表实例键中的接口 GUID 值名
const REG_NETCFG_INSTANCE_ID: &str = "NetCfgInstanceId";
/// 注册表实例键中的 MAC 覆盖值名
const REG_NETWORK_ADDRESS: &str = "NetworkAddress";
/// 网卡驱动描述值名（回退匹配用）
const REG_DRIVER_DESC: &str = "DriverDesc";

// ============================================================================
// 请求 / 结果契约（Tauri 命令返回结构，前端直接渲染）
// ============================================================================

/// 单个 IPv4 地址条目（支持同一网卡多个 IPv4 跨网段）
#[derive(Debug, Clone, Deserialize)]
pub struct Ipv4Entry {
    /// IPv4 地址（如 192.168.1.100）
    pub ip: String,
    /// IPv4 子网掩码（如 255.255.255.0）
    pub mask: String,
    /// 默认网关（可选，空则本条目不设网关）
    pub gateway: Option<String>,
}

/// 网络配置应用请求（一次提交一个接口的全部变更；未提供的字段保持原样）
#[derive(Debug, Clone, Deserialize)]
pub struct NetworkConfigRequest {
    /// netsh 接口名（FriendlyName，如"以太网"）
    pub ifname: String,
    /// IPv4 地址模式：dhcp | static
    pub mode_v4: String,
    /// IPv4 地址列表（static 模式至少 1 条；支持跨网段多地址）
    pub ipv4s: Vec<Ipv4Entry>,
    /// IPv4 DNS 模式：dhcp | static
    pub mode_dns4: String,
    /// IPv4 DNS 服务器列表（static 模式至少 1 个）
    pub dns4: Vec<String>,
    /// IPv6 地址模式：dhcp | static
    pub mode_v6: String,
    /// IPv6 全局地址（static 模式可选）
    pub ipv6: Option<String>,
    /// IPv6 默认网关（nexthop，static 模式可选）
    pub ipv6_gateway: Option<String>,
    /// IPv6 DNS 模式：dhcp | static
    pub mode_dns6: String,
    /// IPv6 DNS 服务器列表（static 模式至少 1 个）
    pub dns6: Vec<String>,
}

/// 单个应用步骤的结果
#[derive(Debug, Clone, Serialize)]
pub struct ApplyStep {
    /// 步骤中文名
    pub name: String,
    /// 是否成功
    pub ok: bool,
    /// 结果消息（成功为空提示；失败为 netsh 原始错误解码）
    pub message: String,
}

/// 网络配置应用结果
#[derive(Debug, Clone, Serialize)]
pub struct NetworkConfigApplyResult {
    /// 分步结果（按执行顺序）
    pub steps: Vec<ApplyStep>,
    /// 是否全部成功
    pub all_ok: bool,
}

/// DNS 独立配置请求（set_dns 命令契约：只应用 DNS，不动 IP 地址）
#[derive(Debug, Clone, Deserialize)]
pub struct DnsConfigRequest {
    /// netsh 接口名（FriendlyName，如"以太网"）
    pub ifname: String,
    /// IPv4 DNS 模式：dhcp | static
    pub mode_dns4: String,
    /// IPv4 DNS 服务器列表（static 模式至少 1 个）
    pub dns4: Vec<String>,
    /// IPv6 DNS 模式：dhcp | static
    pub mode_dns6: String,
    /// IPv6 DNS 服务器列表（static 模式至少 1 个）
    pub dns6: Vec<String>,
}

/// 单条 DoH 配置记录（get_doh_config 返回；template=None 表示该 IP 未启用 DoH）
#[derive(Debug, Clone, Serialize)]
pub struct DohEntry {
    /// DNS 服务器 IP（IPv4 或 IPv6）
    pub ip: String,
    /// DoH 模板 URL（None=未启用 DoH）
    pub template: Option<String>,
}

/// 单条 DoH 配置输入（set_doh_config 契约；template=Some(URL) 启用，None/空串 移除）
#[derive(Debug, Clone, Deserialize)]
pub struct DohEntryInput {
    /// DNS 服务器 IP（必须是接口当前 DNS 列表成员，否则拒绝）
    pub ip: String,
    /// DoH 模板 URL（None/空串 = 移除该 IP 的 DoH）
    pub template: Option<String>,
}

// ============================================================================
// netsh 执行
// ============================================================================

/// 构造 netsh 的 `name="接口名"` 参数（接口名含空格必须引号；剔除内嵌引号防解析逃逸）
fn netsh_quoted(name: &str) -> String {
    format!("name=\"{}\"", name.replace('"', ""))
}

/// 执行 netsh 并返回 stdout 文本；失败返回 Err(netsh 错误消息解码)
///
/// CREATE_NO_WINDOW（0x08000000）防止 GUI 应用弹出控制台黑窗（与 proc_util 一致）。
fn run_netsh(args: &[&str]) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    let out = Command::new("netsh")
        .args(args)
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| format!("netsh 启动失败: {}", e))?;
    if out.status.success() {
        Ok(decode_ansi(&out.stdout))
    } else {
        // 错误消息优先取 stderr，其次 stdout（netsh 版本间输出位置有差异）
        let err = decode_ansi(&out.stderr);
        let out2 = decode_ansi(&out.stdout);
        let msg = if err.trim().is_empty() { out2 } else { err };
        let trimmed = msg.trim().to_string();
        Err(if trimmed.is_empty() {
            format!("netsh 退出码 {}", out.status.code().unwrap_or(-1))
        } else {
            trimmed
        })
    }
}

/// 解码 netsh 输出：优先 UTF-8（UTF-8 代码页系统），回退 GBK（中文 ANSI 代码页）
fn decode_ansi(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    let (cow, _, _) = encoding_rs::GBK.decode(bytes);
    cow.into_owned()
}

/// 执行单个步骤并包装为 ApplyStep（成功/失败均不中断后续步骤）
fn run_step(name: &str, f: impl FnOnce() -> Result<String, String>) -> ApplyStep {
    match f() {
        Ok(_) => ApplyStep {
            name: name.to_string(),
            ok: true,
            message: "成功".to_string(),
        },
        Err(e) => ApplyStep {
            name: name.to_string(),
            ok: false,
            message: e,
        },
    }
}

// ============================================================================
// 主入口：应用网络配置
// ============================================================================

/// 应用网络配置（IPv4 地址/掩码/网关 + IPv4 DNS + IPv6 地址/网关 + IPv6 DNS）
///
/// 逐项执行并收集结果；单项失败不影响其他项（如 DNS 失败时 IP 已生效，
/// 前端可依据步骤结果定位失败项）。要求管理员权限（R8：失败给出修复建议）。
pub fn apply_network_config(req: &NetworkConfigRequest) -> NetworkConfigApplyResult {
    let mut steps = Vec::new();
    let ifname = req.ifname.trim().to_string();
    if ifname.is_empty() {
        return NetworkConfigApplyResult {
            steps: vec![ApplyStep {
                name: "参数校验".to_string(),
                ok: false,
                message: "接口名不能为空".to_string(),
            }],
            all_ok: false,
        };
    }
    let quoted = netsh_quoted(&ifname);

    // ── 1. IPv4 地址 / 掩码 / 网关 ──
    match req.mode_v4.as_str() {
        "dhcp" => {
            steps.push(run_step("IPv4 自动获取 (DHCP)", || {
                run_netsh(&["interface", "ip", "set", "address", &quoted, "dhcp"])
            }));
        }
        "static" => {
            // 过滤空白行后逐条处理（支持同一网卡多个 IPv4 跨网段地址）
            let entries: Vec<&Ipv4Entry> = req
                .ipv4s
                .iter()
                .filter(|e| !e.ip.trim().is_empty() || !e.mask.trim().is_empty())
                .collect();
            if entries.is_empty() {
                steps.push(ApplyStep {
                    name: "IPv4 静态配置".into(),
                    ok: false,
                    message: "至少需要一个 IPv4 地址".into(),
                });
            } else {
                // 前置校验：所有行地址/掩码必填且格式合法，网关可选合法
                let mut invalid: Option<String> = None;
                for e in &entries {
                    if let Err(err) = validate_ipv4_opt(&e.ip.trim(), "IPv4 地址") {
                        invalid = Some(err);
                        break;
                    }
                    if let Err(err) = validate_ipv4_opt(&e.mask.trim(), "IPv4 子网掩码") {
                        invalid = Some(err);
                        break;
                    }
                    if let Some(g) = e.gateway.as_deref() {
                        let g = g.trim();
                        if !g.is_empty() {
                            if let Err(err) = validate_ipv4_opt(g, "IPv4 默认网关") {
                                invalid = Some(err);
                                break;
                            }
                        }
                    }
                }
                if let Some(err) = invalid {
                    steps.push(ApplyStep {
                        name: "IPv4 静态配置".into(),
                        ok: false,
                        message: err,
                    });
                } else {
                    // 主地址：set address（覆盖式设置，可带网关）
                    let first = &entries[0];
                    // 审查修复：校验用 trim 值但执行传未 trim 值会导致 netsh 解析失败——
                    // 统一以 trim 后的值执行
                    let first_ip = first.ip.trim();
                    let first_mask = first.mask.trim();
                    let first_gw = first.gateway.as_deref().unwrap_or("").trim().to_string();
                    steps.push(run_step("IPv4 静态配置（主地址）", || {
                        if first_gw.is_empty() {
                            run_netsh(&[
                                "interface", "ip", "set", "address", &quoted, "static",
                                &first_ip, &first_mask,
                            ])
                        } else {
                            run_netsh(&[
                                "interface", "ip", "set", "address", &quoted, "static",
                                &first_ip, &first_mask, &first_gw,
                            ])
                        }
                    }));
                    // 附加地址（跨网段）：add address 逐条追加，每条约可带独立网关
                    for (i, e) in entries.iter().enumerate().skip(1) {
                        let g = e.gateway.as_deref().unwrap_or("").trim().to_string();
                        // 审查修复：与主地址一致，执行用 trim 值
                        let e_ip = e.ip.trim();
                        let e_mask = e.mask.trim();
                        let name = format!("IPv4 附加地址 #{}", i);
                        steps.push(run_step(&name, || {
                            if g.is_empty() {
                                run_netsh(&[
                                    "interface", "ip", "add", "address", &quoted,
                                    &e_ip, &e_mask,
                                ])
                            } else {
                                run_netsh(&[
                                    "interface", "ip", "add", "address", &quoted,
                                    &e_ip, &e_mask, &g,
                                ])
                            }
                        }));
                    }
                }
            }
        }
        other => {
            steps.push(ApplyStep {
                name: "IPv4 地址".into(),
                ok: false,
                message: format!("未知的 IPv4 模式: {}（应为 dhcp 或 static）", other),
            });
        }
    }

    // ── 2. IPv4 DNS（共享 apply_dns4_steps，与 set_dns 命令共用，行为零回归）──
    apply_dns4_steps(&req.mode_dns4, &req.dns4, &mut steps, &quoted);

    // ── 3. IPv6 地址 ──
    match req.mode_v6.as_str() {
        "dhcp" => {
            steps.push(run_step("IPv6 自动获取 (DHCP)", || {
                run_netsh(&[
                    "interface", "ipv6", "set", "address", &quoted, "source=dhcp",
                ])
            }));
        }
        "static" => {
            let ipv6 = req.ipv6.as_deref().unwrap_or("").trim().to_string();
            if !ipv6.is_empty() {
                if let Err(e) = validate_ipv6(&ipv6) {
                    steps.push(ApplyStep { name: "IPv6 静态地址".into(), ok: false, message: e });
                } else {
                    // 先 add；地址已存在时 netsh 报错 → 改用 set 更新
                    let r1 = run_netsh(&[
                        "interface", "ipv6", "add", "address", &quoted, &format!("address={}", ipv6),
                    ]);
                    match r1 {
                        Ok(_) => steps.push(ApplyStep {
                            name: "IPv6 静态地址".into(),
                            ok: true,
                            message: "成功".into(),
                        }),
                        Err(_) => {
                            let r2 = run_netsh(&[
                                "interface", "ipv6", "set", "address", &quoted, &format!("address={}", ipv6),
                            ]);
                            match r2 {
                                Ok(_) => steps.push(ApplyStep {
                                    name: "IPv6 静态地址".into(),
                                    ok: true,
                                    message: "成功（更新已存在地址）".into(),
                                }),
                                Err(e) => steps.push(ApplyStep {
                                    name: "IPv6 静态地址".into(),
                                    ok: false,
                                    message: format!("{}（若地址已存在请用相同地址重试以更新）", e),
                                }),
                            }
                        }
                    }
                }
            } else {
                steps.push(ApplyStep {
                    name: "IPv6 静态地址".into(),
                    ok: true,
                    message: "跳过（未填写 IPv6 地址）".into(),
                });
            }
            // IPv6 默认网关（nexthop）：先删旧默认路由（忽略不存在），再添加新路由
            let gw6 = req.ipv6_gateway.as_deref().unwrap_or("").trim().to_string();
            if !gw6.is_empty() {
                if let Err(e) = validate_ipv6(&gw6) {
                    steps.push(ApplyStep { name: "IPv6 默认网关".into(), ok: false, message: e });
                } else {
                    let _ = run_netsh(&[
                        "interface", "ipv6", "delete", "route", "::/0", &quoted,
                    ]);
                    steps.push(run_step("IPv6 默认网关", || {
                        run_netsh(&[
                            "interface", "ipv6", "add", "route", "::/0", &quoted,
                            &format!("nexthop={}", gw6),
                        ])
                    }));
                }
            }
        }
        other => {
            steps.push(ApplyStep {
                name: "IPv6 地址".into(),
                ok: false,
                message: format!("未知的 IPv6 模式: {}（应为 dhcp 或 static）", other),
            });
        }
    }

    // ── 4. IPv6 DNS（共享 apply_dns6_steps，与 set_dns 命令共用，行为零回归）──
    apply_dns6_steps(&req.mode_dns6, &req.dns6, &mut steps, &quoted);

    let all_ok = steps.iter().all(|s| s.ok);
    NetworkConfigApplyResult { steps, all_ok }
}

// ============================================================================
// DNS 独立应用（set_dns 命令）— 只动 DNS，不动 IP 地址
// ============================================================================

/// IPv4 DNS 步骤（dhcp：set dns dhcp；static：set dns static 主地址 + add dns 逐条追加）
///
/// 从 apply_network_config 第 2 步抽取，供 apply_network_config 与 set_dns 共用；
/// 步骤名与原实现完全一致，保证前端分步渲染零回归。
fn apply_dns4_steps(mode: &str, dns: &[String], steps: &mut Vec<ApplyStep>, quoted: &str) {
    match mode {
        "dhcp" => {
            steps.push(run_step("IPv4 DNS 自动获取 (DHCP)", || {
                run_netsh(&["interface", "ip", "set", "dns", quoted, "dhcp"])
            }));
        }
        "static" => {
            let dns_list: Vec<String> = dns
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if dns_list.is_empty() {
                steps.push(ApplyStep {
                    name: "IPv4 DNS 静态配置".into(),
                    ok: false,
                    message: "未提供 DNS 服务器地址".into(),
                });
            } else if let Some(bad) = dns_list
                .iter()
                .find(|s| s.parse::<std::net::Ipv4Addr>().is_err())
            {
                steps.push(ApplyStep {
                    name: "IPv4 DNS 静态配置".into(),
                    ok: false,
                    message: format!("无效的 DNS 服务器地址: {}", bad),
                });
            } else {
                // 主 DNS：set dns（覆盖现有）
                let primary = &dns_list[0];
                steps.push(run_step("IPv4 DNS 静态配置", || {
                    run_netsh(&["interface", "ip", "set", "dns", quoted, "static", primary])
                }));
                // 附加 DNS：add dns（index 递增，跳过失败不阻断——部分环境仅需主 DNS）
                for (i, d) in dns_list.iter().enumerate().skip(1) {
                    let idx = (i + 1).to_string();
                    let name = format!("IPv4 附加 DNS #{}", i + 1);
                    steps.push(run_step(&name, || {
                        run_netsh(&[
                            "interface", "ip", "add", "dns", quoted, d, &format!("index={}", idx),
                        ])
                    }));
                }
            }
        }
        other => {
            steps.push(ApplyStep {
                name: "IPv4 DNS".into(),
                ok: false,
                message: format!("未知的 IPv4 DNS 模式: {}（应为 dhcp 或 static）", other),
            });
        }
    }
}

/// IPv6 DNS 步骤（dhcp：set dnsservers dhcp；static：set dnsservers static 主地址 + add dnsservers 逐条追加）
///
/// 从 apply_network_config 第 4 步抽取，供 apply_network_config 与 set_dns 共用。
fn apply_dns6_steps(mode: &str, dns: &[String], steps: &mut Vec<ApplyStep>, quoted: &str) {
    match mode {
        "dhcp" => {
            steps.push(run_step("IPv6 DNS 自动获取 (DHCP)", || {
                run_netsh(&["interface", "ipv6", "set", "dnsservers", quoted, "dhcp"])
            }));
        }
        "static" => {
            let dns_list: Vec<String> = dns
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if dns_list.is_empty() {
                steps.push(ApplyStep {
                    name: "IPv6 DNS 静态配置".into(),
                    ok: false,
                    message: "未提供 DNS 服务器地址".into(),
                });
            } else if let Some(bad) = dns_list
                .iter()
                .find(|s| s.parse::<std::net::Ipv6Addr>().is_err())
            {
                steps.push(ApplyStep {
                    name: "IPv6 DNS 静态配置".into(),
                    ok: false,
                    message: format!("无效的 IPv6 DNS 服务器地址: {}", bad),
                });
            } else {
                // 主 DNS：set dnsservers（覆盖现有）
                let primary = &dns_list[0];
                steps.push(run_step("IPv6 DNS 静态配置", || {
                    run_netsh(&[
                        "interface", "ipv6", "set", "dnsservers", quoted, "static", primary,
                    ])
                }));
                // 附加 DNS：add dnsservers（index 递增）
                for (i, d) in dns_list.iter().enumerate().skip(1) {
                    let idx = (i + 1).to_string();
                    let name = format!("IPv6 附加 DNS #{}", i + 1);
                    steps.push(run_step(&name, || {
                        run_netsh(&[
                            "interface", "ipv6", "add", "dnsservers", quoted, d,
                            &format!("index={}", idx),
                        ])
                    }));
                }
            }
        }
        other => {
            steps.push(ApplyStep {
                name: "IPv6 DNS".into(),
                ok: false,
                message: format!("未知的 IPv6 DNS 模式: {}（应为 dhcp 或 static）", other),
            });
        }
    }
}

/// DNS 步骤共享入口（set_dns 命令契约）：IPv4 DNS + IPv6 DNS 一次执行
pub fn apply_dns_steps(req: &DnsConfigRequest, steps: &mut Vec<ApplyStep>, quoted: &str) {
    apply_dns4_steps(&req.mode_dns4, &req.dns4, steps, quoted);
    apply_dns6_steps(&req.mode_dns6, &req.dns6, steps, quoted);
}

/// set_dns 命令后端入口：接口名校验 + 仅执行 DNS 步骤（不动 IP 地址）
pub fn apply_dns_config(req: &DnsConfigRequest) -> NetworkConfigApplyResult {
    let mut steps = Vec::new();
    let ifname = req.ifname.trim().to_string();
    if ifname.is_empty() {
        return NetworkConfigApplyResult {
            steps: vec![ApplyStep {
                name: "参数校验".to_string(),
                ok: false,
                message: "接口名不能为空".to_string(),
            }],
            all_ok: false,
        };
    }
    let quoted = netsh_quoted(&ifname);
    apply_dns_steps(req, &mut steps, &quoted);
    let all_ok = steps.iter().all(|s| s.ok);
    NetworkConfigApplyResult { steps, all_ok }
}

// ============================================================================
// DoH 配置（Windows DoH：Get/Set/Remove-DnsClientDohServerAddress cmdlet）
//
// 安全（R4）：用户输入（ifname / ip / url）一律单引号包裹并转义内嵌单引号，
// 脚本仅作数据载体经 `-Command` 传入，不经 shell 拼接执行（proc_util 模式）。
// ============================================================================

/// PowerShell 单引号字符串转义（`'` 翻倍为 `''`，防脚本注入逃逸）
fn ps_esc(s: &str) -> String {
    s.replace('\'', "''")
}

/// 校验 DoH 模板 URL：非空且必须以 https:// 开头（拒绝其他协议，R4）
fn validate_doh_template(url: &str) -> Result<(), String> {
    let t = url.trim();
    if t.is_empty() {
        return Err("DoH 模板 URL 不能为空".to_string());
    }
    if !t.starts_with("https://") {
        return Err(format!(
            "DoH 模板 URL 必须以 https:// 开头（当前: {}）",
            t
        ));
    }
    Ok(())
}

/// 解析 `Get-DnsClientDohServerAddress` 定制输出（每行 `IP<TAB>template`，template 可为空）
fn parse_doh_output(out: &str) -> Vec<DohEntry> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            let mut parts = l.splitn(2, '\t');
            let ip = parts.next()?.trim();
            if ip.is_empty() {
                return None;
            }
            let template = parts
                .next()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            Some(DohEntry {
                ip: ip.to_string(),
                template,
            })
        })
        .collect()
}

/// 查询接口当前 DoH 配置（Get-DnsClientDohServerAddress，Win10 2004+）
///
/// 双模型兼容：支持 `-InterfaceAlias` 的 DnsClient 模块（接口级）走 if 分支；
/// 本机 DnsClient 1.0.0.0 为 IP 级全局模型（无 -InterfaceAlias 参数）走 else 分支
/// 查询全部 DoH 记录，由前端按接口 DNS 对照渲染。
/// 无 DoH 记录时返回空 Vec；接口名无效或系统不支持时返回 Err。
fn query_doh_entries(ifname: &str) -> Result<Vec<DohEntry>, String> {
    // 输出定制为 tab 分隔（反引号 t），避免默认表格对长 URL 截断
    let script = format!(
        "if ((Get-Command Get-DnsClientDohServerAddress).Parameters.ContainsKey('InterfaceAlias')) {{\n  \
         Get-DnsClientDohServerAddress -InterfaceAlias '{ifname}' | ForEach-Object {{ \"$($_.ServerAddress)`t$($_.DohTemplate)\" }}\n\
         }} else {{\n  \
         Get-DnsClientDohServerAddress | ForEach-Object {{ \"$($_.ServerAddress)`t$($_.DohTemplate)\" }}\n\
         }}",
        ifname = ps_esc(ifname)
    );
    let out = crate::proc_util::run_ps_result(&script)?;
    Ok(parse_doh_output(&out))
}

/// 取接口当前 DNS 服务器列表（IPv4 + IPv6 合并；DoH 前置防御校验用）
fn current_dns_ips(ifname: &str) -> Result<Vec<String>, String> {
    let adapters = crate::datasource::netif::adapter_configs()
        .map_err(|e| format!("获取网卡配置失败（API: GetAdaptersAddresses）: {}", e))?;
    let found = adapters
        .iter()
        .find(|a| a.name == ifname)
        .ok_or_else(|| format!("未找到接口 \"{}\"，无法校验 DoH 目标", ifname))?;
    let mut ips: Vec<String> = found.ipv4_dns.clone();
    ips.extend(found.ipv6_dns.clone());
    Ok(ips)
}

/// 启用/修改单个 IP 的 DoH（Set-DnsClientDohServerAddress）
///
/// 双模型兼容：接口级模块带 -InterfaceAlias；本机 IP 级模型（无该参数）直接 -ServerAddress。
/// 注意：IP 级模型的 cmdlet 仅能修改已存在的 DoH 记录（Query 定位），
/// 对无记录 IP 报“找不到实例”——错误如实传播，由前端提示用户。
fn set_doh(ifname: &str, ip: &str, url: &str) -> Result<String, String> {
    let script = format!(
        "if ((Get-Command Set-DnsClientDohServerAddress).Parameters.ContainsKey('InterfaceAlias')) {{\n  \
         Set-DnsClientDohServerAddress -InterfaceAlias '{ifname}' -ServerAddress '{ip}' -DohTemplate '{url}'\n\
         }} else {{\n  \
         Set-DnsClientDohServerAddress -ServerAddress '{ip}' -DohTemplate '{url}'\n\
         }}",
        ifname = ps_esc(ifname),
        ip = ps_esc(ip),
        url = ps_esc(url)
    );
    crate::proc_util::run_ps_result(&script)
}

/// 移除单个 IP 的 DoH（Remove-DnsClientDohServerAddress；调用方已保证目标存在才调用）
fn remove_doh(ifname: &str, ip: &str) -> Result<String, String> {
    let script = format!(
        "if ((Get-Command Remove-DnsClientDohServerAddress).Parameters.ContainsKey('InterfaceAlias')) {{\n  \
         Remove-DnsClientDohServerAddress -InterfaceAlias '{ifname}' -ServerAddress '{ip}' -Confirm:$false\n\
         }} else {{\n  \
         Remove-DnsClientDohServerAddress -ServerAddress '{ip}' -Confirm:$false\n\
         }}",
        ifname = ps_esc(ifname),
        ip = ps_esc(ip)
    );
    crate::proc_util::run_ps_result(&script)
}

/// get_doh_config 命令后端：查询接口 DoH 配置（只读，无需管理员）
pub fn get_doh_config(ifname: &str) -> Result<Vec<DohEntry>, String> {
    let ifname = ifname.trim();
    if ifname.is_empty() {
        return Err("接口名不能为空".to_string());
    }
    query_doh_entries(ifname)
}

/// set_doh_config 命令后端：逐 IP 应用 DoH 配置（Set/Remove cmdlet）
///
/// 前置防御：执行前取接口当前 DNS 列表，非成员 IP 直接标记失败步骤；
/// 幂等：移除目标本无 DoH 记录视为成功；entries 为空视为清空接口全部 DoH。
pub fn set_doh_config(ifname: &str, entries: &[DohEntryInput]) -> NetworkConfigApplyResult {
    let mut steps = Vec::new();
    let ifname = ifname.trim().to_string();
    if ifname.is_empty() {
        return NetworkConfigApplyResult {
            steps: vec![ApplyStep {
                name: "参数校验".to_string(),
                ok: false,
                message: "接口名不能为空".to_string(),
            }],
            all_ok: false,
        };
    }

    // 前置防御：取接口当前 DNS 列表（取不到则拒绝执行，避免对非 DNS 目标误设 DoH）
    let dns_ips = match current_dns_ips(&ifname) {
        Ok(v) => v,
        Err(e) => {
            return NetworkConfigApplyResult {
                steps: vec![ApplyStep {
                    name: "DoH 前置校验".to_string(),
                    ok: false,
                    message: e,
                }],
                all_ok: false,
            };
        }
    };

    // 当前已启用 DoH 的 IP 集合（用于移除幂等与空 entries 清空）
    let current_doh: Vec<String> = match query_doh_entries(&ifname) {
        Ok(v) => v.into_iter().map(|e| e.ip).collect(),
        Err(e) => {
            return NetworkConfigApplyResult {
                steps: vec![ApplyStep {
                    name: "DoH 查询".to_string(),
                    ok: false,
                    message: e,
                }],
                all_ok: false,
            };
        }
    };

    // 空 entries = 清空接口全部 DoH
    if entries.is_empty() {
        if current_doh.is_empty() {
            steps.push(ApplyStep {
                name: "DoH 清空".to_string(),
                ok: true,
                message: "接口当前无 DoH 记录，无需清理".to_string(),
            });
        } else {
            for ip in &current_doh {
                steps.push(run_step(&format!("DoH 移除 {}", ip), || {
                    remove_doh(&ifname, ip)
                }));
            }
        }
        let all_ok = steps.iter().all(|s| s.ok);
        return NetworkConfigApplyResult { steps, all_ok };
    }

    // 逐条校验并执行（每 IP 一步）
    for entry in entries {
        let ip = entry.ip.trim().to_string();
        let display = if ip.is_empty() {
            "(空)".to_string()
        } else {
            ip.clone()
        };
        let name = format!("DoH {}", display);
        // 防御：IP 必须是接口当前 DNS 服务器
        if ip.is_empty() || !dns_ips.iter().any(|d| d == &ip) {
            steps.push(ApplyStep {
                name,
                ok: false,
                message: format!("该 IP 不是当前 DNS 服务器: {}", display),
            });
            continue;
        }
        match entry.template.as_deref().map(str::trim) {
            // 移除 DoH（幂等：无记录视为成功）
            None | Some("") => {
                if current_doh.contains(&ip) {
                    steps.push(run_step(&name, || remove_doh(&ifname, &ip)));
                } else {
                    steps.push(ApplyStep {
                        name,
                        ok: true,
                        message: "该 IP 当前未启用 DoH，无需移除（幂等）".to_string(),
                    });
                }
            }
            // 启用 DoH（先校验模板 URL）
            Some(url) => {
                if let Err(e) = validate_doh_template(url) {
                    steps.push(ApplyStep {
                        name,
                        ok: false,
                        message: e,
                    });
                    continue;
                }
                let url = url.to_string();
                steps.push(run_step(&name, || set_doh(&ifname, &ip, &url)));
            }
        }
    }

    let all_ok = steps.iter().all(|s| s.ok);
    NetworkConfigApplyResult { steps, all_ok }
}

// ============================================================================
// MAC 修改（注册表 NetworkAddress + 重启网卡）
// ============================================================================

/// 修改网卡物理地址（MAC）
///
/// 流程：校验格式 → 定位网卡注册表实例键（NetCfgInstanceId == 接口 GUID）→
/// 备份原值 → 写入 NetworkAddress → 禁用/启用网卡使生效。
/// 返回消息含原值备份提示（回滚方式：重新填写原值应用，或网卡高级属性清空）。
///
/// 注意：重启网卡瞬间会短暂断网（毫秒级~秒级），部分网卡驱动不支持覆盖 MAC，
/// 失败时原值已备份于返回消息，可恢复。
pub fn set_network_mac(ifname: &str, mac: &str, guid: Option<&str>) -> Result<String, String> {
    // 1. 校验 MAC 格式：允许 AA:BB:CC:DD:EE:FF / AA-BB-... / AABBCCDDEEFF
    let clean: String = mac
        .chars()
        .filter(|c| *c != ':' && *c != '-' && !c.is_whitespace())
        .collect();
    if clean.len() != 12 || !clean.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(
            "无效的 MAC 地址格式（示例: AA:BB:CC:DD:EE:FF；共 12 个十六进制字符）".to_string(),
        );
    }
    let ifname = ifname.trim();
    if ifname.is_empty() {
        return Err("接口名不能为空".to_string());
    }
    let guid = guid.map(|s| s.trim()).filter(|s| !s.is_empty());
    if guid.is_none() {
        return Err("未获取到接口 GUID，无法定位网卡注册表实例（请重新加载网络配置后重试）".to_string());
    }
    let guid = guid.unwrap();

    // 2. 定位网卡注册表实例键
    let instance = find_nic_instance(guid, ifname)?;

    // 3. 备份原值（无则记录 None）
    let backup: Option<String> = match instance.get_value::<String, _>(REG_NETWORK_ADDRESS) {
        Ok(v) => Some(v),
        Err(_) => None,
    };

    // 4. 写入新值（REG_SZ）
    instance
        .set_value(REG_NETWORK_ADDRESS, &clean)
        .map_err(|e| format!("写入注册表 NetworkAddress 失败: {}（错误码 {}）", e, e.raw_os_error().unwrap_or(-1)))?;

    // 5. 重启网卡使生效（禁用→启用）
    let quoted = netsh_quoted(ifname);
    run_netsh(&["interface", "set", "interface", &quoted, "admin=disable"])
        .map_err(|e| format!("禁用网卡失败（注册表已写入，恢复原值需手动）：{}", e))?;
    run_netsh(&["interface", "set", "interface", &quoted, "admin=enable"])
        .map_err(|e| format!("启用网卡失败（网卡当前为禁用状态）：{}", e))?;

    let backup_hint = match backup {
        Some(b) => format!("原物理地址 {} 已备份，如需恢复请重新应用该值。", b),
        None => "网卡此前未覆盖 MAC（系统默认地址），如需恢复请在网卡高级属性中清空网络地址。".to_string(),
    };
    Ok(format!(
        "物理地址已修改为 {} 并重启网卡生效（可能短暂断网）。{}",
        format_mac(&clean),
        backup_hint
    ))
}

/// 在网卡 Class 键下枚举实例，按 NetCfgInstanceId（接口 GUID）精确匹配；
/// 找不到 GUID 匹配时回退 DriverDesc == 接口描述（避免误匹配，需唯一）
fn find_nic_instance(guid: &str, ifname: &str) -> Result<RegKey, String> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let class = hklm
        .open_subkey_with_flags(NIC_CLASS_KEY, KEY_READ)
        .map_err(|e| format!("打开网卡 Class 注册表键失败: {}（错误码 {}）", e, e.raw_os_error().unwrap_or(-1)))?;

    let guid_norm = guid.trim().trim_start_matches('{').trim_end_matches('}').to_lowercase();
    let mut by_desc: Option<RegKey> = None;
    let mut by_desc_name: Option<String> = None;

    for i in 0..=64 {
        let name = format!("{:04}", i);
        let Ok(inst) = class.open_subkey_with_flags(&name, KEY_READ | KEY_SET_VALUE) else {
            continue;
        };
        let instance_id: Option<String> = inst.get_value(REG_NETCFG_INSTANCE_ID).ok();
        if let Some(id) = instance_id {
            let id_norm = id.trim().trim_start_matches('{').trim_end_matches('}').to_lowercase();
            if id_norm == guid_norm {
                return Ok(inst);
            }
        }
        // 回退候选：DriverDesc == ifname（仅在无 GUID 匹配时使用）
        if by_desc.is_none() {
            let desc: Option<String> = inst.get_value(REG_DRIVER_DESC).ok();
            if desc.as_deref() == Some(ifname) {
                by_desc = Some(inst);
                by_desc_name = Some(name);
            }
        }
    }

    if let Some(k) = by_desc {
        crate::debug_warn!(
            "net_config",
            "MAC 修改回退到 DriverDesc 匹配（实例 {}），GUID {} 未找到",
            by_desc_name.unwrap_or_default(),
            guid
        );
        return Ok(k);
    }
    Err(format!(
        "未找到接口 \"{}\"（GUID {}）对应的网卡注册表实例（枚举范围 0000-0064）",
        ifname, guid
    ))
}

/// 格式化 MAC 为 AA:BB:CC:DD:EE:FF
fn format_mac(hex: &str) -> String {
    hex.as_bytes()
        .chunks(2)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(":")
}

// ============================================================================
// 校验辅助（纯函数，可单测）
// ============================================================================

/// 校验 IPv4 地址（非空 + 格式合法）
fn validate_ipv4_opt(s: &str, what: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err(format!("{}不能为空", what));
    }
    s.parse::<std::net::Ipv4Addr>()
        .map(|_| ())
        .map_err(|_| format!("无效的{}: {}", what, s))
}

/// 校验 IPv6 地址（格式合法；允许带 zone 后缀 fe80::1%12 或 /64 前缀）
fn validate_ipv6(s: &str) -> Result<(), String> {
    let bare = s.split('%').next().unwrap_or(s);
    let bare = bare.split('/').next().unwrap_or(bare);
    bare.parse::<std::net::Ipv6Addr>()
        .map(|_| ())
        .map_err(|_| format!("无效的 IPv6 地址: {}", s))
}

// ============================================================================
// 单测（仅纯函数；不实际执行 netsh / 不改网络）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_netsh_quoted() {
        assert_eq!(netsh_quoted("以太网"), "name=\"以太网\"");
        assert_eq!(netsh_quoted("以太网 2"), "name=\"以太网 2\"");
        // 内嵌引号被剔除（防 netsh 解析逃逸）
        assert_eq!(netsh_quoted("a\"b"), "name=\"ab\"");
    }

    #[test]
    fn test_validate_ipv4_opt() {
        assert!(validate_ipv4_opt("192.168.1.100", "IPv4 地址").is_ok());
        assert!(validate_ipv4_opt("", "IPv4 地址").is_err());
        assert!(validate_ipv4_opt("999.1.1.1", "IPv4 地址").is_err());
        assert!(validate_ipv4_opt("abc", "IPv4 地址").is_err());
        assert!(validate_ipv4_opt("255.255.255.0", "IPv4 子网掩码").is_ok());
    }

    #[test]
    fn test_validate_ipv6() {
        assert!(validate_ipv6("2001:db8::1").is_ok());
        assert!(validate_ipv6("fe80::1%12").is_ok());
        assert!(validate_ipv6("2001:db8::1/64").is_ok());
        assert!(validate_ipv6("not-an-ip").is_err());
        assert!(validate_ipv6("192.168.1.1").is_err());
    }

    #[test]
    fn test_format_mac() {
        assert_eq!(format_mac("AABBCCDDEEFF"), "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn test_decode_ansi_utf8_first() {
        // UTF-8 直接透传
        let s = decode_ansi("OK".as_bytes());
        assert_eq!(s, "OK");
        // GBK 中文回退解码（"接口" GBK 编码）
        let gbk = [0xBD, 0xD3, 0xBF, 0xDA]; // "接口"
        let s = decode_ansi(&gbk);
        assert!(s.contains("接口"), "GBK 解码失败: {}", s);
    }

    // ── DNS 独立应用（set_dns）——仅校验失败路径，避免真实执行 netsh ──

    #[test]
    fn test_apply_dns_empty_ifname() {
        // 空接口名：提前返回参数校验失败，不执行任何 netsh
        let req = DnsConfigRequest {
            ifname: "  ".into(),
            mode_dns4: "dhcp".into(),
            dns4: vec![],
            mode_dns6: "dhcp".into(),
            dns6: vec![],
        };
        let r = apply_dns_config(&req);
        assert!(!r.all_ok);
        assert_eq!(r.steps.len(), 1);
        assert!(r.steps[0].message.contains("接口名不能为空"));
    }

    #[test]
    fn test_apply_dns_unknown_mode() {
        // 两个模式均非法：两步都失败，不执行真实命令
        let req = DnsConfigRequest {
            ifname: "以太网".into(),
            mode_dns4: "bogus".into(),
            dns4: vec![],
            mode_dns6: "bogus".into(),
            dns6: vec![],
        };
        let r = apply_dns_config(&req);
        assert!(!r.all_ok);
        assert!(r.steps[0].message.contains("未知的 IPv4 DNS 模式"));
        assert!(r.steps[1].message.contains("未知的 IPv6 DNS 模式"));
    }

    #[test]
    fn test_apply_dns_static_empty_list() {
        // static 但未提供 DNS：失败步骤（不执行真实 netsh）
        let req = DnsConfigRequest {
            ifname: "以太网".into(),
            mode_dns4: "static".into(),
            dns4: vec![],
            mode_dns6: "bogus".into(),
            dns6: vec![],
        };
        let r = apply_dns_config(&req);
        assert!(!r.all_ok);
        assert!(r.steps[0].message.contains("未提供 DNS 服务器地址"));
    }

    #[test]
    fn test_apply_dns_static_invalid_ip() {
        // static + 非法 IP：失败步骤（不执行真实 netsh）
        let req = DnsConfigRequest {
            ifname: "以太网".into(),
            mode_dns4: "static".into(),
            dns4: vec!["999.1.1.1".into()],
            mode_dns6: "bogus".into(),
            dns6: vec![],
        };
        let r = apply_dns_config(&req);
        assert!(!r.all_ok);
        assert!(r.steps[0].message.contains("无效的 DNS 服务器地址"));
    }

    #[test]
    fn test_apply_dns_step_names_preserved() {
        // 步骤名与原 apply_network_config 内联实现完全一致（前端渲染零回归）
        let mut steps = Vec::new();
        let req = DnsConfigRequest {
            ifname: "以太网".into(),
            mode_dns4: "bogus".into(),
            dns4: vec![],
            mode_dns6: "bogus".into(),
            dns6: vec![],
        };
        apply_dns_steps(&req, &mut steps, &netsh_quoted("以太网"));
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].name, "IPv4 DNS");
        assert_eq!(steps[1].name, "IPv6 DNS");
    }

    // ── DoH 输入校验 / 解析（纯函数）──

    #[test]
    fn test_validate_doh_template() {
        assert!(validate_doh_template("https://cloudflare-dns.com/dns-query").is_ok());
        assert!(validate_doh_template("https://dns.google/dns-query").is_ok());
        assert!(validate_doh_template("http://insecure.example/dns-query").is_err());
        assert!(validate_doh_template("not-a-url").is_err());
        assert!(validate_doh_template("").is_err());
        assert!(validate_doh_template("  ").is_err());
    }

    #[test]
    fn test_ps_esc() {
        // 单引号翻倍转义（PowerShell 单引号字符串内嵌引号）；无引号输入原样返回
        assert_eq!(ps_esc("以太网"), "以太网");
        assert_eq!(ps_esc("it's"), "it''s");
        assert_eq!(ps_esc("a'b'c"), "a''b''c");
    }

    #[test]
    fn test_parse_doh_output() {
        // tab 分隔：IP + template（可空）；空行忽略；template 缺失视为 None
        let out = "1.1.1.1\thttps://cloudflare-dns.com/dns-query\n192.168.1.1\t\n\n8.8.8.8\thttps://dns.google/dns-query";
        let entries = parse_doh_output(out);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].ip, "1.1.1.1");
        assert_eq!(
            entries[0].template.as_deref(),
            Some("https://cloudflare-dns.com/dns-query")
        );
        assert_eq!(entries[1].ip, "192.168.1.1");
        assert!(entries[1].template.is_none());
        assert_eq!(entries[2].ip, "8.8.8.8");
        assert_eq!(entries[2].template.as_deref(), Some("https://dns.google/dns-query"));
    }

    #[test]
    fn test_parse_doh_output_empty() {
        // 空输出 / 纯空行：返回空 Vec，无 panic
        assert!(parse_doh_output("").is_empty());
        assert!(parse_doh_output("\n\n").is_empty());
    }

    #[test]
    fn test_parse_doh_output_ip_only_line() {
        // 无 template 列的行（DohTemplate 为空的边缘情况）
        let out = "1.1.1.1";
        let entries = parse_doh_output(out);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].ip, "1.1.1.1");
        assert!(entries[0].template.is_none());
    }

    #[test]
    fn test_apply_request_empty_ifname() {
        let req = NetworkConfigRequest {
            ifname: "  ".to_string(),
            mode_v4: "dhcp".into(),
            ipv4s: vec![],
            mode_dns4: "dhcp".into(),
            dns4: vec![],
            mode_v6: "dhcp".into(),
            ipv6: None,
            ipv6_gateway: None,
            mode_dns6: "dhcp".into(),
            dns6: vec![],
        };
        let r = apply_network_config(&req);
        assert!(!r.all_ok);
        assert_eq!(r.steps.len(), 1);
        assert!(r.steps[0].message.contains("接口名不能为空"));
    }

    #[test]
    fn test_apply_request_invalid_ipv4() {
        // 静态模式 + 非法 IP：步骤失败但结构完整、无 panic（不实际执行 netsh）
        let req = NetworkConfigRequest {
            ifname: "以太网".to_string(),
            mode_v4: "static".into(),
            ipv4s: vec![Ipv4Entry {
                ip: "not-an-ip".into(),
                mask: "255.255.255.0".into(),
                gateway: None,
            }],
            mode_dns4: "dhcp".into(),
            dns4: vec![],
            mode_v6: "dhcp".into(),
            ipv6: None,
            ipv6_gateway: None,
            mode_dns6: "dhcp".into(),
            dns6: vec![],
        };
        let r = apply_network_config(&req);
        assert!(!r.all_ok);
        assert!(
            r.steps.iter().any(|s| s.message.contains("无效的IPv4 地址") || s.message.contains("无效的 IPv4 地址")),
            "应包含 IP 校验失败: {:?}",
            r.steps
        );
    }

    #[test]
    fn test_apply_request_unknown_mode() {
        // 未知模式：返回错误步骤，不 panic
        let req = NetworkConfigRequest {
            ifname: "以太网".to_string(),
            mode_v4: "bogus".into(),
            ipv4s: vec![],
            mode_dns4: "dhcp".into(),
            dns4: vec![],
            mode_v6: "dhcp".into(),
            ipv6: None,
            ipv6_gateway: None,
            mode_dns6: "dhcp".into(),
            dns6: vec![],
        };
        let r = apply_network_config(&req);
        assert!(!r.all_ok);
        assert!(r.steps[0].message.contains("未知的 IPv4 模式"));
    }
}
