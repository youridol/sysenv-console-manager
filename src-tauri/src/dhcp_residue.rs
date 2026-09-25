/// DHCP 注册表残留清理模块（"清理 DHCP 残留"）
///
/// ## 这个功能解决的 BUG
///
/// 用户可见现象：基础检测报告"检测到跨网段 DHCP 服务器：192.168.160.1, **192.168.11.1**"，
/// 而本机根本没有 192.168.11.x 网段——`192.168.11.1` 是一台**早已拔掉**的
/// USB/RNDIS 网卡（"以太网 3"）留在注册表里的历史 DHCP 租约。
///
/// 本机实测（2026-09，证据链完整）：
///
/// | 事实 | 观测值 |
/// | --- | --- |
/// | 残留网段接口子键 | `HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces\{107a2f0d-8af1-475b-a26a-abd159b9037e}` |
/// | 其中 `EnableDHCP` | `1`（仍被 `read_network_config` 当作"DHCP 接口"） |
/// | 其中 `DhcpServer` | `192.168.11.1`（→ 作为 **baseline** 并入探测结果） |
/// | 该接口的 PnP 实例 | `USB\Class_CO&Subclass_RN&Prot_SU\9&7099240&1&0000` |
/// | `CM_Locate_DevNodeW(id, NORMAL)` | `CR_NO_SUCH_DEVINST (13)` → 设备**已不存在** |
/// | `Get-PnpDevice` | `Present=False`，`Problem=CM_PROB_PHANTOM` |
/// | `Get-NetAdapter` | 仍列出，`Status=Not Present`；`ipconfig` 已不显示其 IPv4 |
/// | 实测探测输出 | `servers=["192.168.3.254","192.168.160.1","192.168.11.1"] baseline=Some("192.168.11.1")` |
///
/// 即：被拔掉的网卡在 `Tcpip\Parameters\Interfaces` 里留下了完整的
/// DHCP 分配记录（`DhcpIPAddress` / `DhcpServer` / 租约时间等 14 个值），
/// 而 `dhcp_probe::read_network_config` 只按 `EnableDHCP=1` 判定"DHCP 接口"，
/// **不校验网卡是否仍然存在**，于是把幽灵服务器当成真实基线报了出来。
///
/// ## 判定"网卡已不存在"的唯一依据
///
/// 用 `Connection\PnpInstanceID` 定位设备实例，再调
/// `CM_Locate_DevNodeW(..., CM_LOCATE_DEVNODE_NORMAL)`：
/// 只有返回 `CR_NO_SUCH_DEVINST (13)` 才算"已不存在"。本机实测对照：
///
/// - 幽灵 RNDIS `USB\Class_CO&...` → `13`（**唯一**被判为残留者）
/// - Realtek 物理网卡 → `0`（存在，保留）
/// - TAP-Windows（`Problem=22` 设备报错）→ `0`（**存在**，故不清理）
/// - 蓝牙 PAN / Hyper-V vEthernet / 内核调试网卡 → `0`（存在，保留）
///
/// 关键性质：**设备被禁用/报错（如 TAP）仍然返回 `CR_SUCCESS`**，
/// 因此本模块只清理"设备实例真的没了"的网卡，绝不碰禁用网卡或在线网卡。
/// 无法判定（`PnpInstanceID` 缺失/为空，如 Hyper-V Default Switch）一律**不清理**。
///
/// ## 清理范围（只删值，不删子键）
///
/// 只删除下面 [`RESIDUE_VALUE_NAMES`] 列出的 DHCP 分配/租约值，**不动**
/// `IPAddress` / `SubnetMask` / `DefaultGateway` / `NameServer` 等静态配置，
/// 也不删除 `Tcpip\Parameters\Interfaces\{guid}` 子键本身（保守：子键删除不可回滚，
/// 且值级删除已足够让基线消失）。
///
/// 清理前把被删值的**原始字节 + 类型**完整备份为 JSON（可无损还原），
/// 路径见返回的 `backup_path`。
///
/// 清理需管理员权限（`HKLM` 写入）；扫描只读，无需管理员。
use serde::Serialize;
use std::path::PathBuf;
use winreg::enums::{RegType, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE};
use winreg::{RegKey, RegValue};

// ============================================================================
// 常量
// ============================================================================

/// 每个接口的 TCP/IP 参数子键（子键名 = 接口 GUID，含花括号）
const TCPIP_INTERFACES: &str = r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces";

/// 网络类（Class = Net）下按接口 GUID 索引的配置键。
/// `Connection\PnpInstanceID` 给出该接口对应的 PnP 设备实例 ID。
const NETWORK_CLASS: &str =
    r"SYSTEM\CurrentControlSet\Control\Network\{4d36e972-e325-11ce-bfc1-08002be10318}";

/// 属于"DHCP 分配 / 租约"的注册表值名——唯一可被本模块删除的集合。
///
/// 全部来自 Windows DHCP 客户端（`dhcpcore.dll`）在成功获取/续租时写入的值；
/// 网卡被移除后这些值原样残留，正是"遗留 DHCP 记录"的实体。
/// **不含** `IPAddress` / `SubnetMask` / `DefaultGateway` / `NameServer`（静态配置，
/// 删除会破坏用户的静态网络设置）。
pub const RESIDUE_VALUE_NAMES: &[&str] = &[
    "DhcpIPAddress",
    "DhcpSubnetMask",
    "DhcpSubnetMaskOpt",
    "DhcpServer",
    "DhcpNameServer",
    "DhcpDefaultGateway",
    "DhcpGatewayHardware",
    "DhcpGatewayHardwareCount",
    "DhcpInterfaceOptions",
    "Lease",
    "LeaseObtainedTime",
    "LeaseTerminatesTime",
    "T1",
    "T2",
];

/// `CM_LOCATE_DEVNODE_NORMAL`（cfgmgr32 `CM_LOCATE_DEVNODE_FLAGS`）：
/// 只定位当前**存在**的设备实例。
const CM_LOCATE_DEVNODE_NORMAL: u32 = 0;
/// `CR_SUCCESS`（CONFIGRET）
const CR_SUCCESS: u32 = 0;
/// `CR_NO_SUCH_DEVINST`（CONFIGRET 13）：设备实例不存在 → 网卡已移除。
const CR_NO_SUCH_DEVINST: u32 = 13;

#[cfg(target_os = "windows")]
#[link(name = "cfgmgr32")]
extern "system" {
    /// 由设备实例 ID 取得 devnode。`ul_flags = CM_LOCATE_DEVNODE_NORMAL` 时，
    /// 已移除（phantom）的设备返回 `CR_NO_SUCH_DEVINST`。
    /// 项目既有风格：extern "system" 直调 Win32，不引入 windows-sys。
    fn CM_Locate_DevNodeW(pdn_dev_inst: *mut u32, p_device_id: *const u16, ul_flags: u32) -> u32;
}

// ============================================================================
// 结果契约（Tauri 命令返回结构，前端直接渲染）
// ============================================================================

/// 单个残留值（值名 + 人类可读的当前值）
#[derive(Debug, Clone, Serialize)]
pub struct ResidueValue {
    /// 注册表值名（如 `DhcpServer`）
    pub name: String,
    /// 人类可读值（REG_MULTI_SZ 以 `, ` 连接；DWORD 为十进制；二进制为十六进制）
    pub value: String,
}

/// 一台"网卡已不存在但注册表仍留 DHCP 记录"的接口
#[derive(Debug, Clone, Serialize)]
pub struct ResidueAdapter {
    /// 接口 GUID（注册表子键名，含花括号）
    pub guid: String,
    /// 接口友好名（`Connection\Name`，如"以太网 3"）
    pub name: String,
    /// 网卡描述（`Connection\PnpInstanceID` 推断不出时的兜底展示）
    pub description: String,
    /// PnP 设备实例 ID（判定"已不存在"的依据）
    pub pnp_instance_id: String,
    /// 该接口上实际存在的残留值
    pub values: Vec<ResidueValue>,
    /// 其中值得关注的 DHCP 服务器地址（`DhcpServer`，供前端摘要展示）
    pub dhcp_server: Option<String>,
}

/// 扫描结果
#[derive(Debug, Clone, Serialize)]
pub struct DhcpResidueScanResult {
    /// 存在残留的接口（无残留则空表）
    pub adapters: Vec<ResidueAdapter>,
    /// 残留值总数
    pub value_count: usize,
    /// 已检查的接口子键总数
    pub checked_adapters: usize,
    /// 判定为"网卡已不存在"的接口数（含无残留者）
    pub absent_adapters: usize,
    /// 清理后备份文件的存放目录
    pub backup_dir: String,
    /// 中文说明
    pub note: String,
}

/// 单个接口的清理结果
#[derive(Debug, Clone, Serialize)]
pub struct CleanedAdapter {
    pub guid: String,
    pub name: String,
    pub removed: usize,
}

/// 清理结果
#[derive(Debug, Clone, Serialize)]
pub struct DhcpResidueCleanResult {
    /// 成功删除的值总数
    pub removed_values: usize,
    /// 被清理的接口
    pub cleaned: Vec<CleanedAdapter>,
    /// 删除失败项（值名 + 原因），成功时为空
    pub failed: Vec<String>,
    /// 备份文件路径（无残留可清理时为 None）
    pub backup_path: Option<String>,
    /// 中文说明
    pub message: String,
}

// ============================================================================
// PnP 设备存在性判定
// ============================================================================

/// PnP 设备实例的存在性
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Presence {
    /// 设备实例存在（含被禁用/报错的设备——那仍是在册硬件）
    Present,
    /// 设备实例不存在（已移除 / phantom）
    Absent,
    /// 无法判定（无 PnP 实例 ID，或 cfgmgr32 返回意外错误码）
    Unknown,
}

/// 查询设备实例是否存在（`CM_LOCATE_DEVNODE_NORMAL`）。
///
/// 纯 Win32 调用，失败一律返回 `Unknown`（**绝不当成 Absent**——
/// 只有明确拿到 `CR_NO_SUCH_DEVINST` 才允许清理）。
#[cfg(target_os = "windows")]
fn device_presence(pnp_instance_id: &str) -> Presence {
    let trimmed = pnp_instance_id.trim();
    if trimmed.is_empty() {
        return Presence::Unknown;
    }
    // 设备实例 ID → UTF-16 NUL 结尾宽字符串
    let mut wide: Vec<u16> = trimmed.encode_utf16().collect();
    wide.push(0);
    let mut dev_inst: u32 = 0;
    // SAFETY: wide 是以 NUL 结尾的有效 UTF-16 缓冲；dev_inst 为栈上有效可写指针
    let rc = unsafe { CM_Locate_DevNodeW(&mut dev_inst, wide.as_ptr(), CM_LOCATE_DEVNODE_NORMAL) };
    match rc {
        CR_SUCCESS => Presence::Present,
        CR_NO_SUCH_DEVINST => Presence::Absent,
        other => {
            crate::debug_warn!(
                "dhcp_residue",
                "CM_Locate_DevNodeW({}) 返回意外 CONFIGRET={}，按不可判定处理",
                trimmed,
                other
            );
            Presence::Unknown
        }
    }
}

/// 非 Windows 平台：无 cfgmgr32，一律"不可判定"（功能优雅降级）
#[cfg(not(target_os = "windows"))]
fn device_presence(_pnp_instance_id: &str) -> Presence {
    Presence::Unknown
}

/// 纯决策函数：该接口是否应当被清理（可单测，不触碰注册表/设备树）。
///
/// 三个条件全部满足才清理：
/// 1. PnP 实例 ID 非空（否则无法证明设备消失）；
/// 2. 存在性判定为 `Absent`（`CR_NO_SUCH_DEVINST`）；
/// 3. 该接口确实存在至少一个残留值（由调用方另行过滤）。
fn should_clean(pnp_instance_id: Option<&str>, presence: Presence) -> bool {
    let has_id = pnp_instance_id
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    has_id && presence == Presence::Absent
}

/// 读取接口 GUID 对应的 `Connection` 子键信息 (友好名, PnP 实例 ID)
fn read_connection(guid: &str) -> Option<(String, Option<String>)> {
    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(format!(r"{}\{}\Connection", NETWORK_CLASS, guid))
        .ok()?;
    let name = key.get_value::<String, _>("Name").unwrap_or_default();
    let pnp = key
        .get_value::<String, _>("PnpInstanceID")
        .ok()
        .map(|s| s.trim().to_string());
    Some((name.trim().to_string(), pnp))
}

/// 该接口的网卡是否已被移除（判定逻辑与 [`scan`] 完全一致，供测试与复用）。
///
/// `guid` 为 `Tcpip\Parameters\Interfaces` 下的子键名（含花括号）。
/// 无法判定时返回 `false`（保守：宁可保留也不误杀）。
///
/// 注意：**检测链路（`dhcp_probe`）刻意不使用本函数**。
/// 「检测 → 发现残留 → 清理 → 再检测」是用户可见的完整闭环：
/// 检测必须先把幽灵服务器暴露出来（本机实测 `baseline=192.168.11.1`），
/// 否则用户无从得知需要清理。故本函数仅供扫描/清理侧判定，
/// 不用于过滤探测结果（`DhcpProbeResult` 字段亦为冻结契约）。
pub fn is_interface_absent(guid: &str) -> bool {
    let Some((_name, pnp)) = read_connection(guid) else {
        return false;
    };
    should_clean(
        pnp.as_deref(),
        device_presence(pnp.as_deref().unwrap_or("")),
    )
}

// ============================================================================
// 值格式化 / 备份
// ============================================================================

/// 注册表原始值 → 人类可读字符串（**纯函数**，供 UI 展示；备份仍存原始字节）。
///
/// `vtype` 为 `RegType` 的数值（`REG_SZ=1` / `REG_EXPAND_SZ=2` / `REG_BINARY=3` /
/// `REG_DWORD=4` / `REG_MULTI_SZ=7`）。
///
/// 抽成 `pub` 纯函数是为了让**集成测试能真正执行**这段格式化逻辑：
/// 本机 `cargo test --lib` 因既有的 comctl32 加载问题（STATUS_ENTRYPOINT_NOT_FOUND）
/// 不可用，放在 `#[cfg(test)] mod tests` 里的断言永远不会运行。
pub fn residue_value_display(vtype: u32, bytes: &[u8]) -> String {
    match vtype {
        // REG_SZ / REG_EXPAND_SZ：单个以 NUL 结尾的 UTF-16LE 串
        1 | 2 => utf16_le_cstr_to_string(bytes),
        // REG_MULTI_SZ：多个以 NUL 分隔的 UTF-16LE 串，**全部**都要列出
        7 => utf16_le_multi_strings(bytes).join(", "),
        // REG_DWORD：原始字节为小端（winreg 不做字节序转换）
        4 => bytes
            .get(..4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]).to_string())
            .unwrap_or_else(|| "0".to_string()),
        // REG_BINARY
        3 => bytes
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(" "),
        _ => format!("<{} 字节>", bytes.len()),
    }
}

/// 把注册表原始值格式化为人类可读字符串（仅用于 UI 展示，备份仍存原始字节）
fn raw_to_display(v: &RegValue) -> String {
    // RegType 非 Copy，按引用取判别值后再转 u32（不移动 v.vtype）
    let vtype = match v.vtype {
        RegType::REG_SZ => 1u32,
        RegType::REG_EXPAND_SZ => 2,
        RegType::REG_BINARY => 3,
        RegType::REG_DWORD => 4,
        RegType::REG_MULTI_SZ => 7,
        _ => 0,
    };
    residue_value_display(vtype, &v.bytes)
}

/// UTF-16LE 字节 → String，**在首个 NUL 处停止**（REG_SZ / REG_EXPAND_SZ 语义）
fn utf16_le_cstr_to_string(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|u| *u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// UTF-16LE 字节 → 多个字符串（REG_MULTI_SZ 语义：项之间以 NUL 分隔，末尾双 NUL）。
///
/// 注意**不能**复用 [`utf16_le_cstr_to_string`]：那会在首个 NUL 截断，
/// 导致多值列表只显示第一项（实测 `["192.168.11.1", "8.8.8.8"]` 会丢掉 `8.8.8.8`）。
fn utf16_le_multi_strings(bytes: &[u8]) -> Vec<String> {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    // 先整体解码（不去 NUL），再按 NUL 切分并丢弃空项
    String::from_utf16_lossy(&units)
        .split('\0')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 备份文件中的单个值（原始字节以 hex 保存，可无损还原）
#[derive(Debug, Serialize)]
struct BackupValue {
    name: String,
    /// 注册表类型码的十进制值
    vtype: u32,
    /// 原始字节（大写 hex，无分隔）
    bytes_hex: String,
    /// 人类可读值（仅供人工核对，还原时应使用 bytes_hex）
    display: String,
}

/// 备份文件中的单个接口
#[derive(Debug, Serialize)]
struct BackupAdapter {
    /// 接口在 `Tcpip\Parameters\Interfaces` 下的子键名
    guid: String,
    /// 接口友好名
    name: String,
    /// 完整注册表路径（还原时直接定位）
    registry_path: String,
    pnp_instance_id: String,
    values: Vec<BackupValue>,
}

/// 备份文件根对象
#[derive(Debug, Serialize)]
struct BackupFile {
    created_at: String,
    reason: String,
    adapters: Vec<BackupAdapter>,
}

/// 备份目录：运行目录 `diag/`（与其它诊断数据同处，随启动清理策略轮转）
fn backup_dir() -> PathBuf {
    crate::temp_data::diag_dir()
}

/// 把扫描结果转成备份记录（原始字节以 hex 保存，可无损还原）
fn scanned_to_backup(a: &ScannedAdapter) -> BackupAdapter {
    BackupAdapter {
        guid: a.guid.clone(),
        name: a.name.clone(),
        registry_path: format!(r"HKEY_LOCAL_MACHINE\{}\{}", TCPIP_INTERFACES, a.guid),
        pnp_instance_id: a.pnp_instance_id.clone(),
        values: a
            .values
            .iter()
            .map(|v| BackupValue {
                name: v.name.clone(),
                vtype: v.vtype,
                bytes_hex: v
                    .bytes
                    .iter()
                    .map(|b| format!("{:02X}", b))
                    .collect::<Vec<_>>()
                    .join(""),
                display: v.display.clone(),
            })
            .collect(),
    }
}

/// 写出备份文件，返回路径。任一步失败返回 Err（调用方据此**中止**清理）。
fn write_backup(scanned: &[ScannedAdapter]) -> Result<PathBuf, String> {
    let dir = backup_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("创建备份目录 {} 失败: {}", dir.display(), e))?;
    let stem = chrono::Local::now().format("dhcp-residue-%Y%m%d-%H%M%S");
    let path = dir.join(format!("{}.json", stem));
    let file = BackupFile {
        created_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        reason: "清理已移除网卡遗留的 DHCP 分配/租约注册表记录（值级删除，可由此文件还原）"
            .to_string(),
        adapters: scanned.iter().map(scanned_to_backup).collect(),
    };
    let json = serde_json::to_string_pretty(&file)
        .map_err(|e| format!("序列化 DHCP 残留备份失败: {}", e))?;
    std::fs::write(&path, json)
        .map_err(|e| format!("写入备份文件 {} 失败: {}", path.display(), e))?;
    Ok(path)
}

// ============================================================================
// 扫描
// ============================================================================

/// 扫描时收集到的单个残留值（含还原所需的原始数据）
struct ScannedValue {
    name: String,
    vtype: u32,
    bytes: Vec<u8>,
    display: String,
}

/// 扫描时收集到的单个接口
struct ScannedAdapter {
    guid: String,
    name: String,
    description: String,
    pnp_instance_id: String,
    values: Vec<ScannedValue>,
}

/// 扫描 `Tcpip\Parameters\Interfaces`，返回"网卡已移除但仍有 DHCP 记录"的接口。
///
/// 只读；无管理员权限亦可调用。返回 (残留接口, 检查总数, 判定已移除数)。
fn scan() -> (Vec<ScannedAdapter>, usize, usize) {
    let mut out: Vec<ScannedAdapter> = Vec::new();
    let mut checked = 0usize;
    let mut absent = 0usize;

    let hklm = match RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(TCPIP_INTERFACES) {
        Ok(k) => k,
        Err(e) => {
            crate::debug_warn!(
                "dhcp_residue",
                "打开 {} 失败（err={}），无法扫描 DHCP 残留",
                TCPIP_INTERFACES,
                e
            );
            return (out, checked, absent);
        }
    };

    for sub in hklm.enum_keys() {
        let Ok(guid) = sub else { continue };
        let Ok(key) = hklm.open_subkey(&guid) else {
            continue;
        };
        checked += 1;

        // 判定依据：Connection\PnpInstanceID + CM_Locate_DevNodeW
        let Some((conn_name, pnp)) = read_connection(&guid) else {
            crate::debug_info!(
                "dhcp_residue",
                "接口 {} 无 Connection 子键，跳过（无法判定网卡是否仍在）",
                guid
            );
            continue;
        };
        let presence = device_presence(pnp.as_deref().unwrap_or(""));
        if !should_clean(pnp.as_deref(), presence) {
            continue;
        }
        absent += 1;

        // 收集该接口上**实际存在**的残留值
        let mut values: Vec<ScannedValue> = Vec::new();
        for name in RESIDUE_VALUE_NAMES {
            let Ok(raw) = key.get_raw_value(name) else {
                continue;
            };
            // 先取展示文本（借用 raw），再依次移动 vtype / bytes 两个字段
            // （RegType 非 Copy，`raw.vtype as u32` 会移动该字段，
            //  故必须在借用结束之后再取）
            let display = raw_to_display(&raw);
            let vtype = raw.vtype as u32;
            values.push(ScannedValue {
                name: (*name).to_string(),
                vtype,
                display,
                bytes: raw.bytes,
            });
        }
        if values.is_empty() {
            crate::debug_info!(
                "dhcp_residue",
                "接口 {}（{}）网卡已移除，但无 DHCP 残留值",
                guid,
                conn_name
            );
            continue;
        }
        let description = pnp.clone().unwrap_or_default();
        crate::debug_info!(
            "dhcp_residue",
            "发现残留: guid={} name={} pnp={} values={}",
            guid,
            conn_name,
            description,
            values.len()
        );
        out.push(ScannedAdapter {
            guid,
            name: if conn_name.is_empty() {
                "(未知接口)".to_string()
            } else {
                conn_name
            },
            description,
            pnp_instance_id: pnp.unwrap_or_default(),
            values,
        });
    }

    (out, checked, absent)
}

/// 扫描 DHCP 注册表残留（只读，无需管理员权限）
pub fn scan_dhcp_residue() -> DhcpResidueScanResult {
    let (scanned, checked, absent) = scan();
    let value_count: usize = scanned.iter().map(|a| a.values.len()).sum();
    let adapters: Vec<ResidueAdapter> = scanned
        .iter()
        .map(|a| ResidueAdapter {
            guid: a.guid.clone(),
            name: a.name.clone(),
            description: a.description.clone(),
            pnp_instance_id: a.pnp_instance_id.clone(),
            values: a
                .values
                .iter()
                .map(|v| ResidueValue {
                    name: v.name.clone(),
                    value: v.display.clone(),
                })
                .collect(),
            dhcp_server: a
                .values
                .iter()
                .find(|v| v.name == "DhcpServer")
                .map(|v| v.display.clone()),
        })
        .collect();

    let note = if adapters.is_empty() {
        format!(
            "未发现 DHCP 残留：已检查 {} 个网络接口，均不存在「网卡已移除但保留 DHCP 记录」的情况。",
            checked
        )
    } else {
        let servers: Vec<String> = adapters
            .iter()
            .filter_map(|a| a.dhcp_server.clone())
            .collect();
        format!(
            "发现 {} 个已移除的网卡仍保留 DHCP 记录，共 {} 项（已检查 {} 个接口，其中 {} 个网卡已移除）{}。这些是历史租约残留，不是当前生效的 DHCP 服务器；清理后不会影响在线网卡与静态配置。",
            adapters.len(),
            value_count,
            checked,
            absent,
            if servers.is_empty() {
                String::new()
            } else {
                format!("；其中记录的 DHCP 服务器为 {}", servers.join(", "))
            }
        )
    };

    crate::debug_info!(
        "dhcp_residue",
        "扫描完成: checked={} absent={} residue_adapters={} residue_values={}",
        checked,
        absent,
        adapters.len(),
        value_count
    );

    DhcpResidueScanResult {
        adapters,
        value_count,
        checked_adapters: checked,
        absent_adapters: absent,
        backup_dir: backup_dir().display().to_string(),
        note,
    }
}

// ============================================================================
// 清理
// ============================================================================

/// 清理 DHCP 注册表残留（需管理员权限）。
///
/// **服务端重新扫描**决定删除对象，不接受前端传入的 GUID/值名——
/// 避免被诱导删除任意注册表键值。
///
/// 流程：扫描 → 备份（失败即中止，绝不无备份删除）→ 逐值删除 → 汇总。
/// 单个值删除失败不中断其余值（逐项记录到 `failed`）。
pub fn clean_dhcp_residue() -> Result<DhcpResidueCleanResult, String> {
    let (scanned, _checked, _absent) = scan();
    if scanned.is_empty() {
        return Ok(DhcpResidueCleanResult {
            removed_values: 0,
            cleaned: Vec::new(),
            failed: Vec::new(),
            backup_path: None,
            message: "未发现 DHCP 残留，无需清理。".to_string(),
        });
    }

    // 1. 备份（失败即中止：不允许无备份删除）
    let backup_path = write_backup(&scanned)?;
    crate::debug_info!(
        "dhcp_residue",
        "清理前备份已写入: {}",
        backup_path.display()
    );

    // 2. 逐值删除（KEY_SET_VALUE 打开，只删值不删子键）
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let mut removed_values = 0usize;
    let mut cleaned: Vec<CleanedAdapter> = Vec::new();
    let mut failed: Vec<String> = Vec::new();

    for adapter in &scanned {
        let key = match hklm.open_subkey_with_flags(
            format!(r"{}\{}", TCPIP_INTERFACES, adapter.guid),
            KEY_READ | KEY_SET_VALUE,
        ) {
            Ok(k) => k,
            Err(e) => {
                failed.push(format!(
                    "{}（{}）: 无法以写权限打开注册表键: {}",
                    adapter.name, adapter.guid, e
                ));
                continue;
            }
        };
        let mut removed_here = 0usize;
        for value in &adapter.values {
            match key.delete_value(&value.name) {
                Ok(()) => {
                    removed_here += 1;
                    removed_values += 1;
                }
                Err(e) => {
                    failed.push(format!(
                        "{}（{}）\\{}: 删除失败: {}",
                        adapter.name, adapter.guid, value.name, e
                    ));
                }
            }
        }
        if removed_here > 0 {
            cleaned.push(CleanedAdapter {
                guid: adapter.guid.clone(),
                name: adapter.name.clone(),
                removed: removed_here,
            });
        }
    }

    let message = if failed.is_empty() {
        format!(
            "已清理 {} 个已移除网卡的 DHCP 残留，共删除 {} 项。备份：{}",
            cleaned.len(),
            removed_values,
            backup_path.display()
        )
    } else {
        format!(
            "已清理 {} 个网卡、删除 {} 项，另有 {} 项失败（详见失败列表）。备份：{}",
            cleaned.len(),
            removed_values,
            failed.len(),
            backup_path.display()
        )
    };

    crate::debug_info!(
        "dhcp_residue",
        "清理完成: adapters={} removed_values={} failed={} backup={}",
        cleaned.len(),
        removed_values,
        failed.len(),
        backup_path.display()
    );

    Ok(DhcpResidueCleanResult {
        removed_values,
        cleaned,
        failed,
        backup_path: Some(backup_path.display().to_string()),
        message,
    })
}

// ============================================================================
// 单测
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 残留值白名单必须**只**含 DHCP 分配/租约值，绝不能含静态配置。
    ///
    /// 这是本模块最关键的护栏：误删 `IPAddress` / `DefaultGateway` 会破坏
    /// 用户的静态网络配置（本机实测这两项承载 192.168.3.238 + 网关 192.168.3.254）。
    #[test]
    fn residue_names_exclude_static_config() {
        for forbidden in [
            "IPAddress",
            "SubnetMask",
            "DefaultGateway",
            "NameServer",
            "EnableDHCP",
            "DhcpDefaultGateway", // 例外：这是 DHCP 下发的网关记录，属残留
        ] {
            if forbidden == "DhcpDefaultGateway" {
                assert!(
                    RESIDUE_VALUE_NAMES.contains(&forbidden),
                    "DhcpDefaultGateway 是 DHCP 下发值，应可清理"
                );
                continue;
            }
            assert!(
                !RESIDUE_VALUE_NAMES.contains(&forbidden),
                "静态配置值 {forbidden} 绝不能被清理"
            );
        }
        // 核心残留值必须在列
        for required in ["DhcpServer", "DhcpIPAddress", "LeaseObtainedTime"] {
            assert!(
                RESIDUE_VALUE_NAMES.contains(&required),
                "核心残留值 {required} 应可清理"
            );
        }
        // 无重复项
        let mut sorted = RESIDUE_VALUE_NAMES.to_vec();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len(), "残留值白名单不应含重复项");
    }

    /// 清理决策：只有"有 PnP 实例 ID" + "明确 Absent" 才清理。
    #[test]
    fn should_clean_requires_absent_and_nonempty_id() {
        assert!(should_clean(
            Some("PCI\\VEN_10EC&DEV_8125\\x"),
            Presence::Absent
        ));
        // 设备存在（含被禁用/报错，如本机 TAP-Windows Problem=22）→ 绝不清理
        assert!(!should_clean(Some("ROOT\\NET\\0000"), Presence::Present));
        // 不可判定（cfgmgr32 意外错误码）→ 绝不清理
        assert!(!should_clean(Some("X\\Y"), Presence::Unknown));
        // 无 PnP 实例 ID（如 Hyper-V Default Switch）→ 绝不清理
        assert!(!should_clean(None, Presence::Absent));
        assert!(!should_clean(Some(""), Presence::Absent));
        assert!(!should_clean(Some("   "), Presence::Absent));
    }

    /// 真实设备树对照：本机幽灵 RNDIS 网卡应判为 Absent，在线网卡应判为 Present。
    ///
    /// 这不是纯函数测试——它真的调用 cfgmgr32。设备实例 ID 取自本机实测，
    /// 若换机器/设备被移除，断言会相应失配（故仅断言"存在者必须 Present"这一
    /// 稳定方向，幽灵网卡的存在性作为信息打印）。
    #[test]
    fn realtek_nic_is_present_on_this_machine() {
        let realtek = r"PCI\VEN_10EC&DEV_8125&SUBSYS_7D761462&REV_05\01000000684CE00000";
        let p = device_presence(realtek);
        eprintln!("[dhcp-residue] realtek presence = {p:?}");
        assert_eq!(
            p,
            Presence::Present,
            "本机在用的 Realtek 网卡必须判为 Present（若判为 Absent 会误删其配置）"
        );
        // 长这样但完全不存在的实例 ID 必须判为 Absent（不能误判成 Present）
        let bogus = r"PCI\VEN_10EC&DEV_8125&SUBSYS_DEADBEEF&REV_05\000000000000000000";
        assert_eq!(
            device_presence(bogus),
            Presence::Absent,
            "不存在的设备实例 ID 必须判为 Absent"
        );
    }

    /// 空/空白 PnP ID 一律 Unknown（不清理）。
    #[test]
    fn empty_pnp_id_is_unknown() {
        assert_eq!(device_presence(""), Presence::Unknown);
        assert_eq!(device_presence("   "), Presence::Unknown);
    }

    /// REG_MULTI_SZ / REG_SZ / REG_DWORD 格式化。
    #[test]
    fn raw_to_display_formats_by_type() {
        // REG_SZ "192.168.11.1"（UTF-16LE + NUL 结尾）
        let mut sz: Vec<u8> = "192.168.11.1"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        sz.extend_from_slice(&[0, 0]);
        assert_eq!(
            raw_to_display(&RegValue {
                bytes: sz,
                vtype: RegType::REG_SZ
            }),
            "192.168.11.1"
        );

        // REG_DWORD 21600（小端）
        assert_eq!(
            raw_to_display(&RegValue {
                bytes: 21600u32.to_le_bytes().to_vec(),
                vtype: RegType::REG_DWORD
            }),
            "21600"
        );

        // REG_MULTI_SZ ["192.168.11.1", "8.8.8.8"]
        let mut ms: Vec<u8> = Vec::new();
        for s in ["192.168.11.1", "8.8.8.8"] {
            ms.extend(s.encode_utf16().flat_map(|u| u.to_le_bytes()));
            ms.extend_from_slice(&[0, 0]);
        }
        ms.extend_from_slice(&[0, 0]);
        let got = raw_to_display(&RegValue {
            bytes: ms,
            vtype: RegType::REG_MULTI_SZ,
        });
        assert!(
            got.contains("192.168.11.1") && got.contains("8.8.8.8"),
            "REG_MULTI_SZ 应列出全部项: {got}"
        );
    }

    /// 备份文件必须是**可解析 JSON** 且含还原所需的原始字节。
    ///
    /// 只在临时目录构造备份对象做序列化校验（不触碰真实注册表）。
    #[test]
    fn backup_payload_is_lossless_json() {
        let adapters = vec![BackupAdapter {
            guid: "{107a2f0d-8af1-475b-a26a-abd159b9037e}".to_string(),
            name: "以太网 3".to_string(),
            registry_path:
                r"HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces\{107a2f0d-8af1-475b-a26a-abd159b9037e}"
                    .to_string(),
            pnp_instance_id: r"USB\Class_CO&Subclass_RN&Prot_SU\9&7099240&1&0000".to_string(),
            values: vec![BackupValue {
                name: "DhcpServer".to_string(),
                vtype: RegType::REG_SZ as u32,
                bytes_hex: "3100390032002E003100360038002E00310031002E0031000000".to_string(),
                display: "192.168.11.1".to_string(),
            }],
        }];
        let file = BackupFile {
            created_at: "2026-09-01 00:00:00".to_string(),
            reason: "test".to_string(),
            adapters,
        };
        let json = serde_json::to_string_pretty(&file).expect("备份应可序列化");
        let back: serde_json::Value = serde_json::from_str(&json).expect("备份应是合法 JSON");
        let v = &back["adapters"][0]["values"][0];
        assert_eq!(v["name"], "DhcpServer");
        assert_eq!(v["vtype"], RegType::REG_SZ as u32);
        assert!(
            v["bytes_hex"].as_str().unwrap().len() > 0,
            "备份必须保留原始字节以便还原"
        );
        assert_eq!(v["display"], "192.168.11.1");
    }

    /// 真实扫描：结果自洽，且**绝不**把在线网卡列为残留。
    ///
    /// 本机实测：Realtek（192.168.3.238）与 vEthernet (External network) 均在线，
    /// 绝不能出现在残留列表中；`{107a2f0d}` 幽灵网卡若仍在册则应被列出
    /// （设备可被系统清理，故数量只作信息打印，不作为断言）。
    #[test]
    fn live_scan_never_flags_online_adapters() {
        let r = scan_dhcp_residue();
        eprintln!(
            "[dhcp-residue] checked={} absent={} residue_adapters={} values={}",
            r.checked_adapters,
            r.absent_adapters,
            r.adapters.len(),
            r.value_count
        );
        for a in &r.adapters {
            eprintln!(
                "[dhcp-residue]   {} {} pnp={} server={:?} values={}",
                a.guid,
                a.name,
                a.pnp_instance_id,
                a.dhcp_server,
                a.values.len()
            );
        }
        eprintln!("[dhcp-residue] note={}", r.note);

        assert_eq!(
            r.value_count,
            r.adapters.iter().map(|a| a.values.len()).sum::<usize>(),
            "value_count 必须等于各接口残留值之和"
        );
        assert!(r.checked_adapters > 0, "应至少检查到一个接口子键");
        assert!(!r.note.is_empty(), "note 不应为空（前端会渲染）");

        // 在线网卡的 GUID（本机实测）绝不能被列为残留
        const ONLINE_GUIDS: &[&str] = &[
            "{8d0383f2-cbce-425c-a22b-5486f844e7e5}", // Realtek（在用，192.168.3.238）
            "{783cc926-e9f2-4052-8d0a-b737255112c8}", // vEthernet (External network)
            "{4a4d26cf-cac0-44fc-bc01-0abc3e357e67}", // vEthernet (Default Switch)
        ];
        for g in ONLINE_GUIDS {
            assert!(
                !r.adapters.iter().any(|a| a.guid.eq_ignore_ascii_case(g)),
                "在线网卡 {g} 被误判为 DHCP 残留——清理会破坏其网络配置"
            );
        }
        // 任何被列为残留的接口，都必须有非空 PnP 实例 ID（判定依据不可缺失）
        for a in &r.adapters {
            assert!(
                !a.pnp_instance_id.trim().is_empty(),
                "残留接口 {} 缺少 PnP 实例 ID，判定依据不成立",
                a.guid
            );
            assert!(!a.values.is_empty(), "残留接口不应出现在列表里却无值");
        }
    }
}
