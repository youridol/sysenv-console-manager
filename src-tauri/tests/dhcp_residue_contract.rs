//! 回归护栏：「清理 DHCP 残留」必须只清理**已移除网卡**的 DHCP 记录。
//!
//! ## 这个功能解决的 BUG
//!
//! 用户可见现象：基础检测报告"检测到跨网段 DHCP 服务器：192.168.160.1, **192.168.11.1**"，
//! 但本机根本没有 192.168.11.x 网段。`192.168.11.1` 是一台**早已拔掉**的
//! USB/RNDIS 网卡（"以太网 3"）留在注册表里的历史 DHCP 租约。
//!
//! 本机实测证据链（`cargo test --test dhcp_probe_contract` 同机基线）：
//!
//! ```text
//! [dhcp-contract] servers=["192.168.3.254", "192.168.160.1", "192.168.11.1"]
//! [dhcp-contract] baseline=Some("192.168.11.1")
//! ```
//!
//! 其中 `192.168.11.1` 来自：
//! `HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces\{107a2f0d-8af1-475b-a26a-abd159b9037e}`
//! 的 `DhcpServer` 值（该接口 `EnableDHCP=1`，故被 `dhcp_probe::read_network_config`
//! 当作真实 DHCP 接口并入基线），而该接口对应的 PnP 设备
//! `USB\Class_CO&Subclass_RN&Prot_SU\9&7099240&1&0000` 已 `Present=False`
//! （`Get-PnpDevice` 报 `CM_PROB_PHANTOM`）。
//!
//! ## 本文件守护的安全性质
//!
//! 1. **绝不把在线网卡列为残留**——误判会删掉在用网卡的注册表项（最高风险）。
//! 2. **绝不触碰静态配置**——`IPAddress` / `SubnetMask` / `DefaultGateway` /
//!    `NameServer` / `EnableDHCP` 不在可删白名单内（本机这些值承载
//!    192.168.3.238 + 网关 192.168.3.254）。
//! 3. **无法判定一律不清理**——`PnpInstanceID` 缺失/空（如 Hyper-V Default Switch）
//!    或 PnP 查询异常时，必须保持原样。
//! 4. **设备被禁用/报错 ≠ 已移除**——本机 TAP-Windows 报 `Problem=22` 但仍
//!    `Present`，其接口不得被判为残留。
//!
//! 只做**只读**扫描断言；不会调用 `clean_dhcp_residue`（那是破坏性注册表写入，
//! 由用户在 UI 上显式确认后触发）。
//!
//! 静态断言基于**源码文本**与**公开 API**，无需启动 GUI 或 WebView2。

use std::path::PathBuf;

/// 读取 `dhcp_residue.rs` 源码（用于静态契约断言）。
fn read_residue_source() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/dhcp_residue.rs");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("无法读取 {}: {}", p.display(), e))
}

/// 只保留**代码行**，丢掉注释行（模块头注释有意记录了 BUG 历史与旧实现）
fn residue_code_only() -> String {
    read_residue_source()
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with("///") && !t.starts_with("//") && !t.starts_with("//!")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A. 可删白名单**只**含 DHCP 分配/租约值，绝不含静态网络配置。
///
/// 这是本功能最高风险的护栏：误删 `IPAddress` / `DefaultGateway` 会让用户的
/// 静态网络配置失效（本机实测这些值承载 192.168.3.238 / 网关 192.168.3.254）。
#[test]
fn cleanup_whitelist_excludes_static_network_config() {
    let whitelist = secm_lib::dhcp_residue::RESIDUE_VALUE_NAMES;

    // 静态配置：绝不能出现在可删名单里
    for forbidden in [
        "IPAddress",
        "SubnetMask",
        "DefaultGateway",
        "NameServer",
        "EnableDHCP",
        "Domain",
        "MTU",
    ] {
        assert!(
            !whitelist.contains(&forbidden),
            "静态/接口配置值 {forbidden} 出现在可删白名单中——\
             清理会破坏用户的静态网络配置（本机 IPAddress=192.168.3.238、\
             DefaultGateway=192.168.3.254 即存在这些值上）。"
        );
    }

    // 核心 DHCP 残留值必须在列（否则功能无效）
    for required in [
        "DhcpServer",
        "DhcpIPAddress",
        "DhcpSubnetMask",
        "LeaseObtainedTime",
        "LeaseTerminatesTime",
    ] {
        assert!(
            whitelist.contains(&required),
            "核心 DHCP 残留值 {required} 缺失——清理将无法消除幽灵服务器。"
        );
    }

    // 无重复项（重复会导致计数虚高）
    let mut sorted = whitelist.to_vec();
    sorted.sort_unstable();
    let before = sorted.len();
    sorted.dedup();
    assert_eq!(before, sorted.len(), "可删白名单不应含重复项");
}

/// B. 只删值，**不删子键**——守卫"不可回滚的破坏"。
#[test]
fn cleanup_deletes_values_only_never_subkeys() {
    let code = residue_code_only();

    assert!(
        code.contains("delete_value"),
        "清理必须走 delete_value（值级删除）。"
    );
    assert!(
        !code.contains("delete_subkey"),
        "不得删除接口子键——子键删除不可回滚；值级删除已足够让幽灵基线消失。"
    );
    // 必须显式声明子键删除被排除（用删除动作断言，而非仅靠注释）
    assert!(
        code.contains("KEY_SET_VALUE"),
        "必须以最小权限（KEY_SET_VALUE）打开注册表键。"
    );
}

/// C. 备份必须存在，且备份失败时**中止**清理（不允许无备份删除）。
#[test]
fn cleanup_requires_successful_backup_before_deleting() {
    let code = residue_code_only();

    assert!(
        code.contains("write_backup"),
        "必须有备份步骤（原值可还原）。"
    );
    // 关键顺序不变量：备份用 `?` 提前返回，因此删除发生在备份成功之后
    assert!(
        code.contains("let backup_path = write_backup(&scanned)?;"),
        "备份失败必须中止清理（`?` 提前返回）——不允许在无备份的情况下删除。"
    );
    // 删除动作必须出现在备份之后
    let backup_pos = code
        .find("write_backup(&scanned)?")
        .expect("应有备份调用");
    let delete_pos = code.find("delete_value").expect("应有删除调用");
    assert!(
        delete_pos > backup_pos,
        "删除必须发生在备份成功之后（先备份后删除）。"
    );
}

/// D. 清理对象由**服务端重新扫描**决定，不接受前端传入的注册表路径/值名。
///
/// 若接受前端入参，被诱导即可删除任意注册表键值（本功能以管理员权限运行）。
#[test]
fn cleanup_does_not_trust_frontend_supplied_targets() {
    let code = residue_code_only();

    // 公开的清理入口必须无参数（除 Tauri State 外）
    assert!(
        code.contains("pub fn clean_dhcp_residue() -> Result<DhcpResidueCleanResult, String>"),
        "clean_dhcp_residue 必须无入参：删除对象只能由服务端扫描决定。"
    );
}

/// E. 判定"网卡已不存在"必须使用 cfgmgr32 的存在性查询，且**只有**
///    `CR_NO_SUCH_DEVINST` 才允许判定为已移除。
#[test]
fn absence_is_decided_by_cfgmgr32_no_such_devinst_only() {
    let code = residue_code_only();

    assert!(
        code.contains("CM_Locate_DevNodeW"),
        "必须用 CM_Locate_DevNodeW 判定设备实例是否存在。"
    );
    assert!(
        code.contains("CM_LOCATE_DEVNODE_NORMAL"),
        "必须用 CM_LOCATE_DEVNODE_NORMAL（只定位存在的设备；\
         PHANTOM 标志会反过来把幽灵设备也当存在）。"
    );
    assert!(
        code.contains("CR_NO_SUCH_DEVINST"),
        "仅 CR_NO_SUCH_DEVINST 可判定为已移除。"
    );
    // 保守性：未知错误码必须归入 Unknown（不可当成 Absent）
    assert!(
        code.contains("Presence::Unknown"),
        "无法判定必须落到 Unknown 分支（保守不清理）。"
    );
    assert!(
        code.contains("fn should_clean"),
        "清理决策必须是可单测的纯函数。"
    );
}

/// F. 真实扫描（只读）：结构自洽，且**在线网卡绝不被列为残留**。
///
/// 本机实测在线网卡（清理它们会直接破坏当前网络）：
///   - `{8d0383f2-...}` Realtek —— 在用，192.168.3.238
///   - `{783cc926-...}` vEthernet (External network) —— 承载 192.168.3.238
///   - `{4a4d26cf-...}` vEthernet (Default Switch) —— 192.168.160.1（ICS DHCP）
#[test]
fn live_scan_never_flags_online_adapters() {
    let r = secm_lib::dhcp_residue::scan_dhcp_residue();

    eprintln!("[dhcp-residue] checked={}", r.checked_adapters);
    eprintln!("[dhcp-residue] absent={}", r.absent_adapters);
    eprintln!("[dhcp-residue] residue_adapters={}", r.adapters.len());
    eprintln!("[dhcp-residue] residue_values={}", r.value_count);
    for a in &r.adapters {
        eprintln!(
            "[dhcp-residue]   name={:?} guid={} pnp={} dhcp_server={:?} values={}",
            a.name,
            a.guid,
            a.pnp_instance_id,
            a.dhcp_server,
            a.values.len()
        );
        for v in &a.values {
            eprintln!("[dhcp-residue]       {} = {}", v.name, v.value);
        }
    }
    eprintln!("[dhcp-residue] note={}", r.note);

    // 结构自洽
    assert_eq!(
        r.value_count,
        r.adapters.iter().map(|a| a.values.len()).sum::<usize>(),
        "value_count 必须等于各接口残留值之和"
    );
    assert!(r.checked_adapters > 0, "应至少检查到一个接口子键");
    assert!(!r.note.is_empty(), "note 不应为空（前端会渲染）");
    assert!(
        r.absent_adapters <= r.checked_adapters,
        "判定已移除的接口数不可能超过检查总数"
    );

    // 安全性质 1：在线网卡绝不能被列为残留
    const ONLINE_GUIDS: &[&str] = &[
        "{8d0383f2-cbce-425c-a22b-5486f844e7e5}", // Realtek（在用）
        "{783cc926-e9f2-4052-8d0a-b737255112c8}", // vEthernet (External network)
        "{4a4d26cf-cac0-44fc-bc01-0abc3e357e67}", // vEthernet (Default Switch)
    ];
    for g in ONLINE_GUIDS {
        assert!(
            !r.adapters.iter().any(|a| a.guid.eq_ignore_ascii_case(g)),
            "在线网卡 {g} 被判为 DHCP 残留——清理会破坏其网络配置（IP/网关/DNS）！"
        );
    }

    // 安全性质 2：任何被列为残留的接口都必须有非空 PnP 实例 ID 与至少一个值
    for a in &r.adapters {
        assert!(
            !a.pnp_instance_id.trim().is_empty(),
            "残留接口 {} 缺少 PnP 实例 ID，判定依据不成立",
            a.guid
        );
        assert!(
            !a.values.is_empty(),
            "残留接口 {} 出现在列表里却没有任何值",
            a.guid
        );
        // 列出的值名必须全部落在白名单内（不可能出现静态配置值）
        for v in &a.values {
            assert!(
                secm_lib::dhcp_residue::RESIDUE_VALUE_NAMES.contains(&v.name.as_str()),
                "残留接口 {} 列出了白名单外的值 {}——存在误删风险",
                a.guid,
                v.name
            );
        }
    }

    // 安全性质 3：本机若仍存在那台已拔掉的 USB/RNDIS 网卡（1970-01 实测
    // {107a2f0d} 的 DhcpServer=192.168.11.1），它必须被识别为残留。
    // 设备可能被系统清理，故只在存在时断言其判定正确性。
    if let Some(ghost) = r
        .adapters
        .iter()
        .find(|a| a.guid.eq_ignore_ascii_case("{107a2f0d-8af1-475b-a26a-abd159b9037e}"))
    {
        eprintln!("[dhcp-residue] 幽灵 RNDIS 网卡已被正确识别: {ghost:?}");
        assert_eq!(
            ghost.dhcp_server.as_deref(),
            Some("192.168.11.1"),
            "幽灵网卡的 DhcpServer 应为 192.168.11.1（检测报告的跨网段服务器来源）"
        );
    } else {
        eprintln!(
            "[dhcp-residue] 注：{{107a2f0d}} 幽灵网卡已不在册（可能已被清理），跳过该项断言"
        );
    }
}

/// G. `is_interface_absent` 对未知/不存在的 GUID 必须返回 false（保守不清理）。
#[test]
fn unknown_interface_guid_is_not_treated_as_absent() {
    // 不存在的 GUID：读不到 Connection 子键 → 不可判定 → false
    assert!(
        !secm_lib::dhcp_residue::is_interface_absent(
            "{00000000-0000-0000-0000-000000000000}"
        ),
        "不存在的接口 GUID 不得被判为『网卡已移除』"
    );
    assert!(
        !secm_lib::dhcp_residue::is_interface_absent("not-a-guid"),
        "非法 GUID 不得被判为『网卡已移除』"
    );
    // 本机在用 Realtek 网卡：必须为 false（否则清理会误删其配置）
    assert!(
        !secm_lib::dhcp_residue::is_interface_absent(
            "{8d0383f2-cbce-425c-a22b-5486f844e7e5}"
        ),
        "在用 Realtek 网卡不得被判为『网卡已移除』"
    );
}

/// H. 注册表值格式化：REG_MULTI_SZ 必须列出**全部**项（不得在首个 NUL 截断）。
///
/// 这是本文件里唯一覆盖"展示文本"正确性的用例，刻意放在集成测试而非
/// `#[cfg(test)] mod tests`：本机 `cargo test --lib` 因既有 comctl32 加载问题
/// （STATUS_ENTRYPOINT_NOT_FOUND）完全无法运行，单元测试里的断言永不执行。
/// 实测证据：`["192.168.11.1", "8.8.8.8"]` 若在首个 NUL 截断，只会显示
/// `192.168.11.1`——用户将看不到 DHCP 下发的全部 DNS/网关项。
#[test]
fn multi_sz_format_lists_every_item() {
    // 构造 REG_MULTI_SZ ["192.168.11.1", "8.8.8.8"]（每项 UTF-16LE + NUL，末尾再补 NUL）
    let mut ms: Vec<u8> = Vec::new();
    for s in ["192.168.11.1", "8.8.8.8"] {
        ms.extend(s.encode_utf16().flat_map(|u| u.to_le_bytes()));
        ms.extend_from_slice(&[0, 0]);
    }
    ms.extend_from_slice(&[0, 0]);
    let got = secm_lib::dhcp_residue::residue_value_display(7 /* REG_MULTI_SZ */, &ms);
    assert!(
        got.contains("192.168.11.1"),
        "REG_MULTI_SZ 应含第一项，实际: {got}"
    );
    assert!(
        got.contains("8.8.8.8"),
        "REG_MULTI_SZ 必须含**第二项**（在首个 NUL 截断会丢掉它），实际: {got}"
    );
    assert_eq!(got, "192.168.11.1, 8.8.8.8", "多值应以逗号连接");

    // REG_SZ：单个以 NUL 结尾的串
    let mut sz: Vec<u8> = "192.168.11.1"
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    sz.extend_from_slice(&[0, 0]);
    assert_eq!(
        secm_lib::dhcp_residue::residue_value_display(1, &sz),
        "192.168.11.1"
    );

    // REG_DWORD：小端（21600 = 0x5460）
    assert_eq!(
        secm_lib::dhcp_residue::residue_value_display(4, &21600u32.to_le_bytes()),
        "21600"
    );

    // REG_BINARY：hex
    assert_eq!(
        secm_lib::dhcp_residue::residue_value_display(3, &[0xC0, 0xA8, 0x0B, 0x01]),
        "C0 A8 0B 01"
    );

    // 未知类型：只报字节数，不 panic
    assert_eq!(
        secm_lib::dhcp_residue::residue_value_display(0, &[1, 2, 3]),
        "<3 字节>"
    );

    // 真实残留值：本机幽灵网卡的 DhcpServer（REG_SZ）格式化结果
    let mut server_sz: Vec<u8> = "192.168.11.1"
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    server_sz.extend_from_slice(&[0, 0]);
    assert_eq!(
        secm_lib::dhcp_residue::residue_value_display(1, &server_sz),
        "192.168.11.1",
        "DhcpServer 应格式化为点分十进制 IP"
    );
}

/// I. 扫描结果必须序列化为前端契约字段（改名会静默破坏 UI）。
#[test]
fn scan_result_serializes_frontend_contract_fields() {
    let r = secm_lib::dhcp_residue::scan_dhcp_residue();
    let json = serde_json::to_value(&r).expect("扫描结果应可序列化");

    for field in [
        "adapters",
        "value_count",
        "checked_adapters",
        "absent_adapters",
        "backup_dir",
        "note",
    ] {
        assert!(
            json.get(field).is_some(),
            "DhcpResidueScanResult 缺少契约字段 {field}（前端 src/pages/Network.tsx 依赖）"
        );
    }
    // 残留项字段
    if let Some(first) = json["adapters"].as_array().and_then(|a| a.first()) {
        for field in ["guid", "name", "values", "dhcp_server"] {
            assert!(
                first.get(field).is_some(),
                "ResidueAdapter 缺少契约字段 {field}"
            );
        }
        if let Some(v) = first["values"].as_array().and_then(|v| v.first()) {
            for field in ["name", "value"] {
                assert!(v.get(field).is_some(), "ResidueValue 缺少契约字段 {field}");
            }
        }
    }
}
