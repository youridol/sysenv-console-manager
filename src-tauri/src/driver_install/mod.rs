//! driver_install —— 内核驱动服务（PawnIO）部署与状态查询模块。
//!
//! v3 架构（`sidecar-lhm/CONTRACT-v3.md`）：SECM 不再有自研/第三方温度通道，
//! WinRing0 与 ACPI 通道已删除。硬件数据由 sidecar 内移植的 LiteMonitor 层提供：
//! tier-1 永远可用的用户态采集，tier-2（LibreHardwareMonitor + PawnIO ring0）
//! 由用户按需 UAC 提权后在 elevated 伴生进程内采集。
//!
//! 本模块职责：
//!
//! ```text
//! ensure_temp_channel()（保留的历史入口名；语义已收敛为 PawnIO 就绪状态查询）
//!   └─ pawnio::ensure_pawnio_ready_opt()
//!        服务已运行 → 复用（普通用户可查，零噪音）
//!        服务存在但停止 / 不存在 → 需管理员，失败返回 Err（本模块绝不弹 UAC）
//! ```
//!
//! 铁律落实：
//! - **静默**：全程不弹 UAC（运行期不二次提权）、不弹控制台、不触发证书确认；
//!   提权由前端显式调用 `request_elevation` 触发（on-demand）。
//! - **幂等**：服务已运行 → 复用；重复调用不重复安装（缓存 + 互斥）。
//! - **保守**：PawnIO 为官方部署（可能与 DriverStore/官方安装器共享实例），
//!   SECM 不覆盖、不删除既有实例；不修改 bcdedit、不触碰 Secure Boot、不重启。
//! - **日志**：每次操作写 `<运行目录>/diag/driver.log`（追加、时间戳、
//!   API 名 + Win32 错误码(0x%08X) + 建议修复）。
//!
//! 无任何内核驱动随包分发：WinRing0x64.sys 已从发行包移除（Microsoft 易受攻击
//! 驱动阻止列表在列，且已无调用方）；ring0 能力统一走 PawnIO（WHQL 签名）。

pub mod pawnio;

use serde::Serialize;
use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ============================================================================
// 服务名常量
// ============================================================================

/// PawnIO 驱动文件名（随包分发；作为驱动资源目录存在性探测 marker）
pub const PAWNIO_SYS_MARKER: &str = "PawnIO.sys";

// ============================================================================
// 前端展示结构（设计文档 §4.5.1）
// ============================================================================

/// 检测到的安全软件信息（前端杀软配置引导文案用）。
#[derive(Debug, Clone, Serialize)]
pub struct SecuritySoftwareInfo {
    /// 内部标识: "defender" | "huorong" | "360" | "tencent" | "kaspersky" | "norton" | "eset" | "lenovo"
    pub name: &'static str,
    /// 中文显示名
    pub display_name: &'static str,
    /// 是否检测到运行中
    pub active: bool,
    /// 是否命中已知拦截 PawnIO 的对象（驱动防护型杀软，或 Defender 内核隔离/驱动阻止列表开启）
    pub blocked_driver: bool,
}

/// 驱动状态（Tauri 命令返回，前端硬件页展示）。
///
/// v3 收窄：只反映 PawnIO ring0 通道（tier-2）；WinRing0/ACPI 已删除，
/// `channel` 契约值固定为 "pawnio" | "none"。
#[derive(Debug, Clone, Serialize)]
pub struct DriverStatus {
    /// 当前生效通道: "pawnio" | "none"
    pub channel: &'static str,
    /// 通道是否可用（PawnIO 服务就绪）
    pub available: bool,
    /// PawnIO 服务状态: "running" | "stopped" | "not_installed" | "unknown"
    pub service_state: String,
    /// 签名环境诊断（前端展示用）
    pub secure_boot: bool,
    pub testsigning: bool,
    /// 降级原因链（人读文案，含错误码与修复建议）
    pub degraded: Vec<String>,
    /// 检测到的安全软件（Defender/火绒/360 等；前端引导杀软配置用）
    pub security: Vec<SecuritySoftwareInfo>,
}

impl DriverStatus {
    fn new(
        channel: &'static str,
        available: bool,
        service_state: &str,
        secure_boot: bool,
        testsigning: bool,
        degraded: Vec<String>,
        security: Vec<SecuritySoftwareInfo>,
    ) -> Self {
        Self {
            channel,
            available,
            service_state: service_state.to_string(),
            secure_boot,
            testsigning,
            degraded,
            security,
        }
    }
}

// ============================================================================
// 状态缓存与安装互斥（幂等核心）
// ============================================================================

/// 状态缓存：30s 内重复调用直接返回（避免高频轮询触发重复检测/安装）
static STATUS_CACHE: Mutex<Option<(Instant, DriverStatus)>> = Mutex::new(None);
/// 安装互斥：并发触发（首启 + 进页）只执行一次安装
static INSTALL_MUTEX: Mutex<()> = Mutex::new(());
/// 缓存有效期
const CACHE_TTL: Duration = Duration::from_secs(30);

// ============================================================================
// WinAPI FFI（advapi32 / kernel32，零第三方库）
// ============================================================================

/// Windows 句柄类型
type Handle = *mut c_void;

// ---- SCM 常量 ----
const SC_MANAGER_ALL_ACCESS: u32 = 0xF003F;
/// 服务全部权限（SCM 安装场景）
pub(crate) const SERVICE_ALL_ACCESS: u32 = 0xF01FF;
/// 内核驱动服务类型
pub(crate) const SERVICE_KERNEL_DRIVER: u32 = 0x1;
/// demand start（手动启动，由本模块拉起）
pub(crate) const SERVICE_DEMAND_START: u32 = 0x3;
/// 错误控制级别：正常（记录日志并继续）
pub(crate) const SERVICE_ERROR_NORMAL: u32 = 0x1;
pub(crate) const SERVICE_STOPPED: u32 = 0x1;
pub(crate) const SERVICE_RUNNING: u32 = 0x4;
/// SCM 连接权限（普通用户可持有；低权限服务状态查询用）
const SC_MANAGER_CONNECT: u32 = 0x0001;
/// 服务查询状态权限（普通用户可持有）
const SERVICE_QUERY_STATUS: u32 = 0x0004;

// ---- 常用 Win32 错误码 ----
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_SERVICE_DOES_NOT_EXIST: u32 = 1060;
const ERROR_INVALID_IMAGE_HASH: u32 = 577;
/// ERROR_DRIVER_BLOCKED（Vulnerable Driver Blocklist / 其他驱动拦截）
const ERROR_DRIVER_BLOCKED: u32 = 1275;
/// NTSTATUS STATUS_INVALID_IMAGE_HASH（驱动加载签名校验失败）
const STATUS_INVALID_IMAGE_HASH: u32 = 0xC000_0428;
/// ERROR_SERVICE_MARKED_FOR_DELETE（服务已标记删除，删除异步挂起，需重启电脑清空）
pub(crate) const ERROR_SERVICE_MARKED_FOR_DELETE: u32 = 0x430;
/// ERROR_CANNOT_DETECT_DRIVER_FAILURE（内核层驱动加载被拒绝，疑似火绒/杀软驱动防护拦截）
pub(crate) const ERROR_CANNOT_DETECT_DRIVER_FAILURE: u32 = 0x431;

// ---- 令牌常量（提权检测） ----
const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_ELEVATION_CLASS: i32 = 20;

/// SERVICE_STATUS 结构
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct ServiceStatus {
    service_type: u32,
    current_state: u32,
    controls_accepted: u32,
    win32_exit_code: u32,
    service_specific_exit_code: u32,
    check_point: u32,
    wait_hint: u32,
}

/// TOKEN_ELEVATION 结构
#[repr(C)]
struct TokenElevation {
    token_is_elevated: u32,
}

#[link(name = "advapi32")]
extern "system" {
    fn OpenSCManagerW(
        lp_machine_name: *const u16,
        lp_database_name: *const u16,
        dw_desired_access: u32,
    ) -> Handle;
    fn OpenServiceW(
        h_sc_manager: Handle,
        lp_service_name: *const u16,
        dw_desired_access: u32,
    ) -> Handle;
    fn CreateServiceW(
        h_sc_manager: Handle,
        lp_service_name: *const u16,
        lp_display_name: *const u16,
        dw_desired_access: u32,
        dw_service_type: u32,
        dw_start_type: u32,
        dw_error_control: u32,
        lp_binary_path_name: *const u16,
        lp_load_order_group: *const u16,
        lp_dw_tag_id: *mut u32,
        lp_dependencies: *const u16,
        lp_service_start_name: *const u16,
        lp_password: *const u16,
    ) -> Handle;
    fn StartServiceW(
        h_service: Handle,
        dw_num_service_args: u32,
        lp_service_arg_vectors: *mut *const u16,
    ) -> i32;
    fn QueryServiceStatus(h_service: Handle, lp_service_status: *mut ServiceStatus) -> i32;
    fn CloseServiceHandle(h_sc_object: Handle) -> i32;
    fn OpenProcessToken(h_process: Handle, dw_desired_access: u32, h_token: *mut Handle) -> i32;
    fn GetTokenInformation(
        h_token: Handle,
        token_information_class: i32,
        token_information: *mut c_void,
        token_information_length: u32,
        return_length: *mut u32,
    ) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> Handle;
    fn CloseHandle(h_object: Handle) -> i32;
    fn GetLastError() -> u32;
}

// ---- 签名验证（wintrust.dll，WinVerifyTrust） ----

/// GUID 结构（COM/动作标识）
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// WINTRUST_ACTION_GENERIC_VERIFY_V2（标准文件签名验证动作）
const WINTRUST_ACTION_GENERIC_VERIFY_V2: Guid = Guid {
    data1: 0x00AA_C56B,
    data2: 0xCD44,
    data3: 0x11d0,
    data4: [0x8C, 0xC2, 0x00, 0xC0, 0x4F, 0xC2, 0x95, 0xEE],
};

/// WTD_UI_NONE：不弹任何 UI（静默验证）
const WTD_UI_NONE: u32 = 2;
/// WTD_REVOKE_NONE：不做吊销检查（离线可验证）
const WTD_REVOKE_NONE: u32 = 0;
/// WTD_CHOICE_FILE：验证目标为文件
const WTD_CHOICE_FILE: u32 = 1;
/// WTD_STATEACTION_VERIFY：执行验证
const WTD_STATEACTION_VERIFY: u32 = 1;
/// WTD_STATEACTION_CLOSE：释放验证状态
const WTD_STATEACTION_CLOSE: u32 = 2;

/// WINTRUST_FILE_INFO 结构（WinTrustData 的 pFile 成员指向）
#[repr(C)]
struct WinTrustFileInfo {
    cb_struct: u32,
    pcwsz_file_path: *const u16,
    h_file: Handle,
    pg_known_subject: *const c_void,
}

/// WINTRUST_DATA 结构（仅覆盖验证文件签名所需字段；union 取 pFile 成员）。
/// 布局与 WinSDK wintrust.h 一致（DWORD/指针交错，x64/x86 均无对齐填充问题）。
#[repr(C)]
struct WinTrustData {
    cb_struct: u32,
    p_policy_callback_data: *mut c_void,
    p_sip_client_data: *mut c_void,
    dw_ui_choice: u32,
    fdw_revocation_checks: u32,
    dw_union_choice: u32,
    p_file: *mut WinTrustFileInfo,
    dw_state_action: u32,
    h_wvt_state_data: Handle,
    pwsz_url_reference: *mut u16,
    dw_prov_flags: u32,
    dw_ui_context: u32,
    p_signature_settings: *mut c_void,
}

#[link(name = "wintrust")]
extern "system" {
    fn WinVerifyTrust(hwnd: Handle, p_action_id: *const Guid, p_wvt_data: *mut c_void) -> i32;
}

/// 验证驱动文件 Authenticode 签名是否有效（WinVerifyTrust，静默无 UI）。
///
/// 返回 true = 签名链验证通过（ERROR_SUCCESS）；false = 无签名/签名无效/验证失败。
/// 用于 StartServiceW 返回 5 时的"杀软拦截 vs 权限问题"分流，以及部署前签名门禁。
pub(crate) fn driver_signature_valid(path: &Path) -> bool {
    let path_str = path.to_string_lossy().to_string();
    let file_path = to_utf16(&path_str);
    let mut file_info = WinTrustFileInfo {
        cb_struct: std::mem::size_of::<WinTrustFileInfo>() as u32,
        pcwsz_file_path: file_path.as_ptr(),
        h_file: ptr::null_mut(),
        pg_known_subject: ptr::null(),
    };
    let mut wtd = WinTrustData {
        cb_struct: std::mem::size_of::<WinTrustData>() as u32,
        p_policy_callback_data: ptr::null_mut(),
        p_sip_client_data: ptr::null_mut(),
        dw_ui_choice: WTD_UI_NONE,
        fdw_revocation_checks: WTD_REVOKE_NONE,
        dw_union_choice: WTD_CHOICE_FILE,
        p_file: &mut file_info,
        dw_state_action: WTD_STATEACTION_VERIFY,
        h_wvt_state_data: ptr::null_mut(),
        pwsz_url_reference: ptr::null_mut(),
        dw_prov_flags: 0,
        dw_ui_context: 0,
        p_signature_settings: ptr::null_mut(),
    };
    // SAFETY: action_id 为合法 GUID 常量；wtd 指向栈上结构且字段初始化完整；
    // hwnd 传 NULL（WTD_UI_NONE 不会使用窗口句柄）
    let result = unsafe {
        WinVerifyTrust(
            ptr::null_mut(),
            &WINTRUST_ACTION_GENERIC_VERIFY_V2,
            &mut wtd as *mut WinTrustData as *mut c_void,
        )
    };
    // 验证结束后释放 WVT 状态（h_wvt_state_data 由 WinVerifyTrust 分配）
    if !wtd.h_wvt_state_data.is_null() {
        wtd.dw_state_action = WTD_STATEACTION_CLOSE;
        // SAFETY: 复用已初始化的 wtd 执行 CLOSE，仅释放内部状态，不弹 UI
        unsafe {
            WinVerifyTrust(
                ptr::null_mut(),
                &WINTRUST_ACTION_GENERIC_VERIFY_V2,
                &mut wtd as *mut WinTrustData as *mut c_void,
            );
        }
    }
    result == 0
}

/// 把字符串转为 NUL 结尾的 UTF-16 缓冲区。
pub(crate) fn to_utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

// ============================================================================
// SCM 服务管理封装（设计文档 §4.1.2 序列）
// ============================================================================

/// SCM 数据库句柄 RAII 包装。
pub(crate) struct ScmGuard(pub(crate) Handle);

impl Drop for ScmGuard {
    fn drop(&mut self) {
        // SAFETY: 句柄由 OpenSCManagerW 返回，关闭是安全操作
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}

/// 服务句柄 RAII 包装。
pub(crate) struct ServiceGuard(pub(crate) Handle);

impl Drop for ServiceGuard {
    fn drop(&mut self) {
        // SAFETY: 句柄由 OpenServiceW/CreateServiceW 返回，关闭是安全操作
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}

/// 打开服务控制管理器（需要管理员权限）。
pub(crate) fn open_scm() -> Result<ScmGuard, u32> {
    // SAFETY: NULL 机器名/数据库名 = 本机默认数据库；权限要求管理员
    let h = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_ALL_ACCESS) };
    if h.is_null() {
        // SAFETY: GetLastError 紧邻失败调用
        return Err(unsafe { GetLastError() });
    }
    Ok(ScmGuard(h))
}

/// 打开指定服务句柄；不存在时返回 Ok(None)。
pub(crate) fn open_service(scm: Handle, name: &str) -> Result<Option<ServiceGuard>, u32> {
    let name_utf16 = to_utf16(name);
    // SAFETY: name 为 NUL 结尾 UTF-16；访问权限为全部（安装场景）
    let h = unsafe { OpenServiceW(scm, name_utf16.as_ptr(), SERVICE_ALL_ACCESS) };
    if h.is_null() {
        // SAFETY: GetLastError 紧邻失败调用
        let code = unsafe { GetLastError() };
        if code == ERROR_SERVICE_DOES_NOT_EXIST {
            return Ok(None);
        }
        return Err(code);
    }
    Ok(Some(ServiceGuard(h)))
}

/// 低权限查询服务状态：SC_MANAGER_CONNECT + SERVICE_QUERY_STATUS，普通用户可查。
///
/// 返回 `Some((current_state, win32_exit_code))`；服务不存在 / 无权限 / 查询失败
/// 返回 `None`。PawnIO 快路径用：服务已运行时普通用户可直接复用，无需管理员。
pub(crate) fn query_service_state_low_priv(name: &str) -> Option<(u32, u32)> {
    // SAFETY: NULL 机器名/数据库名 = 本机默认 SCM 数据库；SC_MANAGER_CONNECT 普通用户可持有
    let scm = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
    if scm.is_null() {
        return None;
    }
    let name_utf16 = to_utf16(name);
    // SAFETY: name 为 NUL 结尾 UTF-16；SERVICE_QUERY_STATUS 普通用户可查
    let h = unsafe { OpenServiceW(scm, name_utf16.as_ptr(), SERVICE_QUERY_STATUS) };
    if h.is_null() {
        // SAFETY: scm 由 OpenSCManagerW 返回
        unsafe {
            CloseServiceHandle(scm);
        }
        return None;
    }
    let mut st = ServiceStatus {
        service_type: 0,
        current_state: 0,
        controls_accepted: 0,
        win32_exit_code: 0,
        service_specific_exit_code: 0,
        check_point: 0,
        wait_hint: 0,
    };
    // SAFETY: st 指向栈上结构，长度充足；h 为有效服务句柄
    let ok = unsafe { QueryServiceStatus(h, &mut st) };
    // SAFETY: 句柄分别由 OpenServiceW / OpenSCManagerW 返回
    unsafe {
        CloseServiceHandle(h);
        CloseServiceHandle(scm);
    }
    if ok == 0 {
        None
    } else {
        Some((st.current_state, st.win32_exit_code))
    }
}

/// 启动服务，失败时退避重试（初始尝试 + 2 次重试，间隔 500ms / 1500ms）。
///
/// 处理杀软延迟放行 / 签名校验瞬时失败（D6）：重试仍失败才返回 Err。
/// 日志完整记录第几次尝试 + API 名 + 错误码（R8）。
pub(crate) fn start_service_with_retry(service: Handle, name: &str) -> Result<(), u32> {
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        // SAFETY: 无启动参数（内核驱动服务）
        let ok = unsafe { StartServiceW(service, 0, ptr::null_mut()) };
        if ok != 0 {
            log_info(
                "StartServiceW",
                &format!("{} 驱动启动成功（第 {} 次尝试）", name, attempt),
            );
            return Ok(());
        }
        // SAFETY: GetLastError 紧邻失败调用
        let code = unsafe { GetLastError() };
        // 计算下次重试延迟：0x430/0x431 追加一次 2s 重试（SCM 删除异步完成 / 内核状态清理）
        let delay = match next_retry_delay(attempt, code) {
            Some(d) => d,
            None => {
                log_error(
                    "StartServiceW",
                    &format!("{} 尝试 {} 次后仍失败 win32=0x{:08X}", name, attempt, code),
                );
                return Err(code);
            }
        };
        let delay_ms = delay.as_millis() as u64;
        if is_marked_for_delete_code(code) {
            log_warn(
                "StartServiceW",
                &format!(
                    "{} 服务标记删除/驱动加载被内核拒绝（win32=0x{:08X}），等待 {}ms 后重试",
                    name, code, delay_ms
                ),
            );
        } else {
            log_warn(
                "StartServiceW",
                &format!(
                    "{} 第 {} 次启动失败 win32=0x{:08X}，{}ms 后重试",
                    name, attempt, code, delay_ms
                ),
            );
        }
        std::thread::sleep(delay);
    }
}

/// 判定是否为驱动签名/加载策略错误（触发降级链）。
fn is_signature_error(code: u32) -> bool {
    matches!(
        code,
        ERROR_INVALID_IMAGE_HASH | ERROR_DRIVER_BLOCKED | STATUS_INVALID_IMAGE_HASH
    )
}

/// 判定是否为"安全软件拦截"候选错误码。
///
/// 仅 StartServiceW/CreateServiceW 上下文中返回 5（ERROR_ACCESS_DENIED）且
/// 驱动文件签名有效时，才最终归类为 `BlockedBySecurity`（见 `classify_install_error`）。
/// 5 也可能是 DACL/权限问题，故必须配合签名验证避免误判（设计文档 §7 风险缓解）。
fn is_security_blocked(code: u32) -> bool {
    code == ERROR_ACCESS_DENIED
}

/// 判定是否为"服务标记删除/驱动加载被内核拒绝"候选错误码（0x430/0x431）。
///
/// - 0x430 ERROR_SERVICE_MARKED_FOR_DELETE：删除异步挂起，需重启电脑清空；
/// - 0x431 ERROR_CANNOT_DETECT_DRIVER_FAILURE：内核层驱动加载被拒绝（火绒 sysdiag
///   hook 驱动加载路径后的典型转译错误码），可尝试重试，持续则重启电脑。
/// 两者修复动作一致（重试 → 放行 → 必要时重启），实现中一并归类 `MarkedForDelete`。
fn is_marked_for_delete_code(code: u32) -> bool {
    code == ERROR_SERVICE_MARKED_FOR_DELETE || code == ERROR_CANNOT_DETECT_DRIVER_FAILURE
}

/// 计算给定本轮尝试序号 `attempt`（从 1 开始）失败 error `code` 后的下一次重试延迟。
///
/// 返回 `Some(Duration)` 表示应等待该时长后重试；`None` 表示到达重试上限、放弃（返回 Err）。
///
/// 规则：
/// - 常规错误码：attempt 1 → 500ms，attempt 2 → 1500ms，之后放弃；
/// - 0x430/0x431（服务标记删除 / 驱动加载被内核拒绝）：在常规上限之外追加一次 2000ms
///   重试（等待 SCM 删除异步完成 / 内核状态清理），attempt 3 → 2000ms，之后放弃。
///
/// 纯函数（无 FFI / 无 I/O），便于单测覆盖重试路径。
fn next_retry_delay(attempt: u32, code: u32) -> Option<Duration> {
    let marked = is_marked_for_delete_code(code);
    match attempt {
        1 => Some(Duration::from_millis(500)),
        2 => Some(Duration::from_millis(1500)),
        3 if marked => Some(Duration::from_millis(2000)),
        _ => None,
    }
}

/// 安装错误分类（供 PawnIO 部署路径使用）。
///
/// - 签名/加载策略错误（577/1275/0xC0000428）→ `Signature`
/// - 5 且驱动文件签名验证有效 → `BlockedBySecurity`（杀软拦截）
/// - 其余 → `Failed`
pub(crate) fn classify_install_error(code: u32, driver_path: Option<&Path>) -> InstallError {
    let signature_valid = match driver_path {
        Some(p) => driver_signature_valid(p),
        None => false,
    };
    classify_install_error_inner(code, signature_valid)
}

/// 分类纯逻辑（签名验证结果已注入，可单测）。
pub(crate) fn classify_install_error_inner(code: u32, signature_valid: bool) -> InstallError {
    if is_signature_error(code) {
        return InstallError::Signature(code);
    }
    if is_marked_for_delete_code(code) {
        log_warn(
            "classify",
            &format!(
                "win32=0x{:08X} 服务标记删除/驱动加载被内核拒绝 → MarkedForDelete",
                code
            ),
        );
        return InstallError::MarkedForDelete(code);
    }
    if is_security_blocked(code) && signature_valid {
        log_warn(
            "classify",
            &format!("win32=0x{:08X} 且驱动签名有效 → 判定为安全软件拦截", code),
        );
        return InstallError::BlockedBySecurity(code);
    }
    if is_security_blocked(code) {
        log_warn(
            "classify",
            &format!(
                "win32=0x{:08X} 但驱动签名验证未通过/无文件可验证 → 按普通失败处理",
                code
            ),
        );
    }
    InstallError::Failed(code)
}

// ============================================================================
// 环境检测
// ============================================================================

/// 当前进程是否以管理员权限运行（TokenElevation）。
pub(crate) fn is_elevated() -> bool {
    // SAFETY: GetCurrentProcess 返回伪句柄，无需关闭
    let process = unsafe { GetCurrentProcess() };
    let mut token: Handle = ptr::null_mut();
    // SAFETY: OpenProcessToken 写入 token；process 为当前进程伪句柄
    let ok = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) };
    if ok == 0 || token.is_null() {
        return false;
    }
    let mut elev = TokenElevation {
        token_is_elevated: 0,
    };
    let mut ret_len: u32 = 0;
    // SAFETY: elev 为栈上结构，长度足够；GetTokenInformation 写入实际长度
    let ok = unsafe {
        GetTokenInformation(
            token,
            TOKEN_ELEVATION_CLASS,
            &mut elev as *mut TokenElevation as *mut c_void,
            std::mem::size_of::<TokenElevation>() as u32,
            &mut ret_len,
        )
    };
    // SAFETY: token 由 OpenProcessToken 返回
    unsafe {
        CloseHandle(token);
    }
    ok != 0 && elev.token_is_elevated != 0
}

/// Secure Boot 状态（注册表读取，零特权）。
///
/// 说明：GetFirmwareEnvironmentVariableW 读取 SecureBoot 固件变量需要
/// SE_SYSTEM_ENVIRONMENT 特权（管理员默认不持有，实测 1314），而
/// `HKLM\SYSTEM\CurrentControlSet\Control\SecureBoot\State\UEFISecureBootEnabled`
/// 由系统在启动时写入，普通用户可读（已在本机实测 =1）。
fn secure_boot_enabled() -> bool {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    let hk = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SYSTEM\CurrentControlSet\Control\SecureBoot\State");
    match hk {
        Ok(k) => k.get_value::<u32, _>("UEFISecureBootEnabled").unwrap_or(0) == 1,
        Err(_) => false,
    }
}

/// 测试签名状态（bcdedit 只读查询，不修改任何设置）。
///
/// 中文系统输出为 GBK，但键名/值（testsigning / Yes）为 ASCII，
/// from_utf8_lossy 仅乱码中文行，不影响匹配。
fn testsigning_enabled() -> bool {
    use std::os::windows::process::CommandExt;
    match std::process::Command::new("bcdedit")
        .args(["/enum", "{current}"])
        // CREATE_NO_WINDOW：禁止子进程创建控制台窗口（GUI 进程无控制台可继承，
        // 不设此标志会闪出 cmd 窗口——v0.17.5 启动自愈线程每次启动触发，
        // 即「打开时初始化闪出命令指示符」的根因）
        .creation_flags(0x08000000)
        .output()
    {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let on = text
                .lines()
                .any(|l| l.contains("testsigning") && l.contains("Yes"));
            log_info("bcdedit", &format!("testsigning 查询完成: enabled={}", on));
            on
        }
        Err(e) => {
            log_warn(
                "bcdedit",
                &format!("查询失败（非 Windows/权限异常）: {}", e),
            );
            false
        }
    }
}

/// HVCI（内存完整性）状态：Defender 在开启时对内核驱动拦截更严，用于杀软引导判定。
fn hvci_enabled() -> bool {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    let hk = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(
        r"SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity",
    );
    match hk {
        Ok(k) => k.get_value::<u32, _>("Enabled").unwrap_or(0) == 1,
        Err(_) => false,
    }
}

// ============================================================================
// 杀软检测（只读、零特权、永不 panic、不阻塞主采集）
//
// 实现：tasklist 子进程一次枚举进程名（零第三方库、零提权），匹配已知杀软
// 进程表。检测结果仅用于前端引导文案与 degraded 提示，不参与权限决策。
// ============================================================================

/// 已知杀软候选（去重后唯一）：(内部标识, 中文显示名, 是否已知拦截驱动)。
///
/// `blocked_driver` 表示该杀软具备内核驱动防护能力，可能拦截 PawnIO
/// 驱动加载。Defender 的判定额外叠加内核隔离/驱动阻止列表注册表状态。
const SECURITY_CANDIDATES: &[(&str, &str, bool)] = &[
    ("defender", "Windows Defender", true),
    ("huorong", "火绒安全软件", true),
    ("360", "360 安全卫士", true),
    ("tencent", "腾讯电脑管家", true),
    ("kaspersky", "卡巴斯基", true),
    ("norton", "诺顿", true),
    ("eset", "ESET NOD32", true),
    ("lenovo", "联想电脑管家", false),
];

/// 杀软内部标识 → 已知进程名列表（tasklist 小写匹配）。
const SECURITY_PROCESS_MAP: &[(&str, &[&str])] = &[
    ("defender", &["msmpeng.exe"] as &[&str]),
    (
        "huorong",
        &["hipsdaemon.exe", "hipstray.exe", "hipsmain.exe"] as &[&str],
    ),
    (
        "360",
        &["360safe.exe", "360tray.exe", "zhudongfangyu.exe"] as &[&str],
    ),
    (
        "tencent",
        &["qqpctray.exe", "qqpcmgr.exe", "qqpcrtp.exe"] as &[&str],
    ),
    ("kaspersky", &["avp.exe"] as &[&str]),
    ("norton", &["ccsvchst.exe"] as &[&str]),
    ("eset", &["ekrn.exe"] as &[&str]),
    (
        "lenovo",
        &["lenovopcmanager.exe", "lenovoutility.exe"] as &[&str],
    ),
];

/// 运行 tasklist 并返回进程行（小写；一次调用，避免多次子进程开销）。
///
/// 失败（非 Windows/权限异常）返回 Err，调用方降级为空列表（不 panic）。
fn tasklist_lines() -> Result<Vec<String>, String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("tasklist")
        .args(["/fo", "csv", "/nh"])
        // CREATE_NO_WINDOW：禁止子进程创建控制台窗口（GUI 进程无控制台可继承）
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| format!("tasklist 启动失败: {}", e))?;
    if !out.status.success() {
        return Err(format!("tasklist 退出码非 0: {:?}", out.status.code()));
    }
    // 进程名（映像名称）为 ASCII，GBK 编码下 from_utf8_lossy 无损
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.lines().map(|l| l.to_ascii_lowercase()).collect())
}

/// 纯字符串匹配：进程行列表中是否存在指定进程名（对行与进程名都做小写归一，可单测）。
fn process_running(lines: &[String], process_name: &str) -> bool {
    let needle = process_name.to_ascii_lowercase();
    lines
        .iter()
        .any(|l| l.to_ascii_lowercase().contains(&needle))
}

/// Windows 易受攻击的驱动程序阻止列表（Vulnerable Driver Blocklist）状态。
///
/// 开启时 Defender 会拦截未受信任/已知易受攻击的内核驱动。零特权可读。
fn vulnerable_driver_blocklist_enabled() -> bool {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    match RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SYSTEM\CurrentControlSet\Control\CI\Config")
    {
        Ok(k) => {
            k.get_value::<u32, _>("VulnerableDriverBlocklistEnable")
                .unwrap_or(0)
                == 1
        }
        Err(_) => false,
    }
}

/// 从进程行列表构造杀软信息（纯逻辑，可注入测试）。
fn security_list_for_lines(lines: &[String]) -> Vec<SecuritySoftwareInfo> {
    SECURITY_CANDIDATES
        .iter()
        .map(|(name, display, blocks)| {
            let procs = SECURITY_PROCESS_MAP
                .iter()
                .find(|(n, _)| *n == *name)
                .map(|(_, p)| *p)
                .unwrap_or(&[]);
            let active = procs.iter().any(|p| process_running(lines, p));
            // Defender 更精确：仅在内核隔离（HVCI）或驱动阻止列表开启时才拦截旧驱动
            let blocked = if active && *name == "defender" {
                hvci_enabled() || vulnerable_driver_blocklist_enabled()
            } else {
                active && *blocks
            };
            SecuritySoftwareInfo {
                name,
                display_name: display,
                active,
                blocked_driver: blocked,
            }
        })
        .collect()
}

/// 检测常见安全软件（Defender/火绒/360/腾讯/卡巴/诺顿/ESET/联想管家）。
///
/// 只读、零特权、永不 panic（tasklist 失败时返回全量 inactive 列表）。
/// 返回全量候选（含 active=false），前端可展示"已检测/未检测"状态。
pub fn detect_security_software() -> Vec<SecuritySoftwareInfo> {
    let lines = match tasklist_lines() {
        Ok(l) => l,
        Err(e) => {
            log_warn("tasklist", &format!("杀软检测失败: {}", e));
            Vec::new()
        }
    };
    security_list_for_lines(&lines)
}

// ============================================================================
// 日志（运行目录 diag/driver.log，追加写）
// ============================================================================

/// driver.log 写入器（线程安全：静态互斥保护追加写）。
fn driver_log_path() -> PathBuf {
    crate::runtime_paths::diag_dir().join("driver.log")
}

/// 写一行 driver.log（追加；目录不存在则创建；写失败静默——日志失败不阻断主流程）。
fn log_line(level: &str, msg: &str) {
    let ts = chrono::Local::now().format("%H:%M:%S%.3f").to_string();
    let line = format!("[{}] [{}] {}\n", ts, level, msg);
    let path = driver_log_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}

/// INFO 级日志（操作动作）。
pub(crate) fn log_info(api: &str, msg: &str) {
    log_line("INFO", &format!("{}: {}", api, msg));
}

/// WARN 级日志（降级触发、非致命异常）。
pub(crate) fn log_warn(api: &str, msg: &str) {
    log_line("WARN", &format!("{}: {}", api, msg));
}

/// ERROR 级日志（含 API 名 + Win32 错误码 + 建议修复，R8 达标）。
pub(crate) fn log_error(api: &str, msg: &str) {
    log_line("ERROR", &format!("{}: {}", api, msg));
}

// ============================================================================
// 资源定位（兼容 NSIS 安装目录 / 便携版两种布局）
// ============================================================================

/// 定位驱动资源目录（内含 PawnIO.sys，随包分发）。
///
/// 查找顺序：
/// 1. 环境变量 SECM_DRIVER_DIR（测试注入用）
/// 2. Tauri resource_dir()/driver（NSIS 安装布局）
/// 3. exe 同目录 resources/driver（便携版布局）
/// 4. 当前工作目录 resources/driver（开发运行布局）
pub(crate) fn resolve_driver_dir(app: Option<&tauri::AppHandle>) -> PathBuf {
    if let Ok(p) = std::env::var("SECM_DRIVER_DIR") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Some(app) = app {
        use tauri::Manager;
        if let Ok(res) = app.path().resource_dir() {
            let candidate = res.join("driver");
            if candidate.join(PAWNIO_SYS_MARKER).exists() {
                return candidate;
            }
        }
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_default();
    for base in [exe_dir.clone(), PathBuf::from(".")] {
        let c = base.join("resources").join("driver");
        if c.join(PAWNIO_SYS_MARKER).exists() {
            return c;
        }
    }
    exe_dir.join("resources").join("driver")
}

// ============================================================================
// 安装错误类型
// ============================================================================

/// 安装错误分类（PawnIO 部署路径共用）。
///
/// v3：SECM 自身的温度通道（WinRing0/ACPI）已删除，本枚举不再有内建安装流程
/// 调用方，仅作为 `classify_install_error(_inner)` 的返回契约保留 ——
/// `pawnio.rs`（含其单测）通过 `to_pawnio_kind` 消费全部变体并生成
/// PawnIO 专属文案，故变体与载荷全部保留。
#[allow(dead_code)]
pub(crate) enum InstallError {
    /// 驱动文件缺失（指定缺失文件）
    MissingDriver(String),
    /// 签名/加载策略错误（577/1275/0xC0000428）→ 转下一级通道
    Signature(u32),
    /// 安全软件拦截（StartServiceW/CreateServiceW 返回 5 且驱动签名有效）
    BlockedBySecurity(u32),
    /// 服务标记删除/驱动加载被内核拒绝（0x430 删除异步挂起需重启 / 0x431 内核拒载疑似杀软拦截）
    MarkedForDelete(u32),
    /// 其他 SCM/启动失败
    Failed(u32),
}

impl InstallError {
    /// 人读文案（含错误码，R8 达标；放行对象为 PawnIO.sys）。
    ///
    /// PawnIO 部署路径使用 `pawnio.rs::failure_message_inner`（PawnIO 专属文案）；
    /// 本方法作为 `InstallError` 的通用 R8 文案契约保留（单测覆盖）。
    #[allow(dead_code)]
    fn message(&self) -> String {
        match self {
            InstallError::MissingDriver(f) => format!("驱动文件缺失: {}（请检查安装目录 resources\\driver\\）", f),
            InstallError::Signature(code) => format!(
                "驱动签名无效（win32=0x{:08X}）。测试签名在 Secure Boot 下不可加载，已自动切换到备用通道。",
                code
            ),
            InstallError::BlockedBySecurity(code) => format!(
                "驱动启动被安全软件拦截（win32=0x{:08X}，驱动签名有效但服务启动被拒绝）。请按杀毒软件类型调整设置：\
                 Windows Defender：设置→隐私和安全性→Windows 安全中心→设备安全性→内核隔离，关闭\"易受攻击的驱动程序阻止列表\"或\"内存完整性\"；\
                 火绒：安全设置→驱动防护/信任区，将 PawnIO.sys 与 SECM 加入白名单；\
                 360：安全卫士→防护中心→信任区，添加 SECM 安装目录；\
                 腾讯电脑管家/卡巴斯基/ESET 等：在杀软信任区/排除项中添加 SECM 目录后重试。",
                code
            ),
            InstallError::MarkedForDelete(code) => {
                if *code == ERROR_SERVICE_MARKED_FOR_DELETE {
                    format!("PawnIO 服务处于标记删除状态（win32=0x{:08X}），删除操作异步完成中。请重启电脑后重试；重启会清空服务删除状态。", code)
                } else {
                    format!("PawnIO 驱动加载被内核拒绝（win32=0x{:08X}，ERROR_CANNOT_DETECT_DRIVER_FAILURE），疑似被火绒/杀软驱动防护拦截。请按引导卡步骤在火绒信任区放行 SECM 目录与 PawnIO.sys 后点击一键重试；若持续失败请重启电脑后再试。", code)
                }
            }
            InstallError::Failed(code) => format!("服务安装/启动失败（win32=0x{:08X}）。", code),
        }
    }
}

/// 规范化 Win32 路径（去掉 `\\?\` 前缀，CreateServiceW binPath 契约）。
pub(crate) fn normalize_win_path(p: &PathBuf) -> String {
    let s = p.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    s.to_string()
}

// ============================================================================
// 主流程：驱动状态查询（PawnIO only）
// ============================================================================

/// 查询驱动通道状态（保留的历史入口名；v3 语义已收敛为 PawnIO）。
///
/// 幂等：内部 30s 缓存 + 全局互斥；调用方置于 spawn_blocking，不阻塞 UI。
/// 运行期绝不弹 UAC、绝不二次提权：无管理员权限时静默降级并给出提示，
/// 提权由前端按需调用 `request_elevation`（sidecar tier-2）触发。
pub fn ensure_temp_channel(app: &tauri::AppHandle) -> DriverStatus {
    ensure_temp_channel_opt(Some(app))
}

/// ensure 的可选 app 变体（测试 / 无 AppHandle 上下文用）。
pub fn ensure_temp_channel_opt(app: Option<&tauri::AppHandle>) -> DriverStatus {
    // 1. 缓存命中（30s 内）→ 直接返回
    if let Ok(guard) = STATUS_CACHE.lock() {
        if let Some((t, s)) = guard.as_ref() {
            if t.elapsed() < CACHE_TTL {
                return s.clone();
            }
        }
    }
    // 2. 安装互斥：并发触发只执行一次探测；其他线程等待期间直接返回缓存/进行中状态
    let _guard = match INSTALL_MUTEX.try_lock() {
        Ok(g) => g,
        Err(_) => {
            log_warn(
                "INSTALL_MUTEX",
                "另一线程正在执行驱动探测，返回当前缓存状态",
            );
            return cached_or_pending();
        }
    };
    // 3. 拿到互斥后复查缓存（等待期间可能已被填充）
    if let Ok(guard) = STATUS_CACHE.lock() {
        if let Some((t, s)) = guard.as_ref() {
            if t.elapsed() < CACHE_TTL {
                return s.clone();
            }
        }
    }
    // 4. 完整检测（同步执行；命令层已 spawn_blocking）
    log_info("ensure_temp_channel", "开始 PawnIO 通道检测");
    let status = run_ensure(app);
    if let Ok(mut guard) = STATUS_CACHE.lock() {
        *guard = Some((Instant::now(), status.clone()));
    }
    log_info(
        "ensure_temp_channel",
        &format!(
            "通道判定完成: channel={} available={} service_state={}",
            status.channel, status.available, status.service_state
        ),
    );
    status
}

/// 使状态缓存失效（手动重试/卸载后调用，强制下次重新检测）。
pub fn invalidate_cache() {
    if let Ok(mut guard) = STATUS_CACHE.lock() {
        *guard = None;
    }
}

/// 互斥被占用时返回的状态：优先缓存，否则"探测进行中"降级状态。
fn cached_or_pending() -> DriverStatus {
    if let Ok(guard) = STATUS_CACHE.lock() {
        if let Some((_, s)) = guard.as_ref() {
            return s.clone();
        }
    }
    DriverStatus::new(
        "none",
        false,
        "unknown",
        secure_boot_enabled(),
        false,
        vec!["驱动探测正在进行中，请稍后刷新。".to_string()],
        detect_security_software(),
    )
}

/// 执行 PawnIO 就绪检测，返回驱动状态。
///
/// 由 `DriverStatus` + `pawnio::ensure_pawnio_ready_opt` 派生：PawnIO 是 SECM
/// 唯一保留的内核驱动通道（sidecar tier-2 ring0 依赖），其部署/复用成功即
/// `channel="pawnio" available=true`。
fn run_ensure(app: Option<&tauri::AppHandle>) -> DriverStatus {
    let secure_boot = secure_boot_enabled();
    let testsigning = testsigning_enabled();
    let mut degraded: Vec<String> = Vec::new();
    // 杀软检测（一次；仅用于引导文案与 degraded 提示）
    let security = detect_security_software();
    let active_security_note = {
        let names: Vec<&str> = security
            .iter()
            .filter(|s| s.active)
            .map(|s| s.display_name)
            .collect();
        if names.is_empty() {
            String::new()
        } else {
            format!("（检测到安全软件：{}）", names.join("、"))
        }
    };
    // SCM 服务状态（低权限可查；用于 service_state 与 degraded 文案）
    let low_state = query_service_state_low_priv(pawnio::SERVICE_PAWNIO).map(|(s, _)| s);

    // PawnIO 就绪探测（幂等；内部 30s 缓存 + 互斥；失败仅返回 Err 不 panic）
    match pawnio::ensure_pawnio_ready_opt(app) {
        Ok(()) => {
            // 服务已在运行且由本次探测确认可用
            if low_state != Some(SERVICE_RUNNING) {
                degraded.push(
                    "PawnIO 服务已就绪，CPU 温度/功耗与主板传感器由 sidecar tier-2 提供。"
                        .to_string(),
                );
            }
            DriverStatus::new(
                "pawnio",
                true,
                "running",
                secure_boot,
                testsigning,
                degraded,
                security,
            )
        }
        Err(e) => {
            log_warn("run_ensure", &format!("PawnIO 未就绪: {}", e));
            degraded.push(e);
            if !active_security_note.is_empty() {
                degraded.push(format!(
                    "检测到安全软件{}，其驱动防护可能拦截 PawnIO 加载。请按引导卡步骤放行 PawnIO.sys 后重试。",
                    active_security_note
                ));
            }
            let service_state = match low_state {
                Some(SERVICE_RUNNING) => "running",
                Some(SERVICE_STOPPED) => "stopped",
                Some(_) => "stopped",
                None => "not_installed",
            };
            DriverStatus::new(
                "none",
                false,
                service_state,
                secure_boot,
                testsigning,
                degraded,
                security,
            )
        }
    }
}

// 此处曾有 `driver_uninstall()`：不在 `lib.rs::invoke_handler` 的注册表中，
// 也无任何模块调用它——属于「既未注册、也无调用」的中间态，与其注释声称的
// 「保留的历史命令名」不符。ADR-0001 D13 已删除；状态缓存失效仍由
// `invalidate_cache()` 在需要处直接调用。

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_error_judgement() {
        // 577 / 1275 / 0xC0000428 均判定为签名/加载策略错误
        assert!(is_signature_error(577));
        assert!(is_signature_error(1275));
        assert!(is_signature_error(0xC000_0428));
        // 其他错误不误判
        assert!(!is_signature_error(5));
        assert!(!is_signature_error(2));
        assert!(!is_signature_error(1060));
    }

    #[test]
    fn driver_status_serialize_channel() {
        // DriverStatus 序列化为 JSON，字段契约固定（前端依赖）
        let security = vec![SecuritySoftwareInfo {
            name: "huorong",
            display_name: "火绒安全软件",
            active: true,
            blocked_driver: true,
        }];
        let s = DriverStatus::new(
            "pawnio",
            true,
            "running",
            true,
            false,
            vec!["降级提示".to_string()],
            security,
        );
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"channel\":\"pawnio\""));
        assert!(json.contains("\"available\":true"));
        assert!(json.contains("\"service_state\":\"running\""));
        assert!(json.contains("\"secure_boot\":true"));
        assert!(json.contains("\"testsigning\":false"));
        assert!(json.contains("降级提示"));
        // security 字段契约（前端杀软引导依赖）
        assert!(json.contains("\"security\""));
        assert!(json.contains("\"name\":\"huorong\""));
        assert!(json.contains("\"display_name\":\"火绒安全软件\""));
        assert!(json.contains("\"active\":true"));
        assert!(json.contains("\"blocked_driver\":true"));
        // v3 收窄：probe_temp_c 已随内核温度通道删除
        assert!(
            !json.contains("probe_temp_c"),
            "v3 不应再输出 probe_temp_c: {}",
            json
        );
    }

    #[test]
    fn security_blocked_judgement() {
        // 仅 5（ERROR_ACCESS_DENIED）为安全拦截候选；577/1275/0xC0000428 归签名
        assert!(is_security_blocked(5));
        assert!(!is_security_blocked(577));
        assert!(!is_security_blocked(1275));
        assert!(!is_security_blocked(0xC000_0428));
        assert!(!is_security_blocked(2));
        assert!(!is_security_blocked(1060));
        assert!(!is_security_blocked(0));
    }

    #[test]
    fn classify_error_judgement() {
        // 分类纯逻辑：签名错误优先，5+签名有效 → BlockedBySecurity，其余 → Failed
        assert!(matches!(
            classify_install_error_inner(577, true),
            InstallError::Signature(577)
        ));
        assert!(matches!(
            classify_install_error_inner(1275, false),
            InstallError::Signature(1275)
        ));
        assert!(matches!(
            classify_install_error_inner(0xC000_0428, true),
            InstallError::Signature(0xC000_0428)
        ));
        // 5 + 签名有效 = 杀软拦截
        assert!(matches!(
            classify_install_error_inner(5, true),
            InstallError::BlockedBySecurity(5)
        ));
        // 5 + 签名无效/无法验证 = 普通失败（避免误判 DACL/权限问题）
        assert!(matches!(
            classify_install_error_inner(5, false),
            InstallError::Failed(5)
        ));
        // 其他错误码不受签名影响
        assert!(matches!(
            classify_install_error_inner(1060, true),
            InstallError::Failed(1060)
        ));
        // 0x430/0x431（服务标记删除/内核拒载）→ MarkedForDelete，不受签名影响
        assert!(matches!(
            classify_install_error_inner(0x430, true),
            InstallError::MarkedForDelete(0x430)
        ));
        assert!(matches!(
            classify_install_error_inner(0x431, false),
            InstallError::MarkedForDelete(0x431)
        ));
    }

    #[test]
    fn next_retry_delay_pure_logic() {
        // 纯函数重试路径：非 0x430/0x431 走 500ms/1500ms；
        // 0x430/0x431 在常规上限外追加 2000ms；attempt 越界返回 None（放弃）
        // --- 常规错误码（如 5 / 577 / 1060）---
        assert_eq!(next_retry_delay(1, 5), Some(Duration::from_millis(500)));
        assert_eq!(next_retry_delay(2, 5), Some(Duration::from_millis(1500)));
        assert_eq!(
            next_retry_delay(3, 5),
            None,
            "常规错误码第 3 次尝试后应放弃"
        );
        assert_eq!(next_retry_delay(4, 5), None);
        assert_eq!(next_retry_delay(3, ERROR_ACCESS_DENIED), None);
        // --- 0x430（服务标记删除）---
        assert_eq!(
            next_retry_delay(1, ERROR_SERVICE_MARKED_FOR_DELETE),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            next_retry_delay(2, ERROR_SERVICE_MARKED_FOR_DELETE),
            Some(Duration::from_millis(1500))
        );
        // 第 3 次尝试（第 4 次尝试前置）：0x430 追加 2000ms 重试
        assert_eq!(
            next_retry_delay(3, ERROR_SERVICE_MARKED_FOR_DELETE),
            Some(Duration::from_millis(2000))
        );
        assert_eq!(next_retry_delay(4, ERROR_SERVICE_MARKED_FOR_DELETE), None);
        // --- 0x431（内核拒载）---
        assert_eq!(
            next_retry_delay(1, ERROR_CANNOT_DETECT_DRIVER_FAILURE),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            next_retry_delay(2, ERROR_CANNOT_DETECT_DRIVER_FAILURE),
            Some(Duration::from_millis(1500))
        );
        assert_eq!(
            next_retry_delay(3, ERROR_CANNOT_DETECT_DRIVER_FAILURE),
            Some(Duration::from_millis(2000)),
            "0x431 第 3 次尝试应追加 2s 重试"
        );
        assert_eq!(
            next_retry_delay(4, ERROR_CANNOT_DETECT_DRIVER_FAILURE),
            None
        );
    }

    #[test]
    fn marked_for_delete_message_contains_guidance() {
        // R8 + 任务验收：0x430 文案含「重启电脑」；0x431 文案含火绒/放行/一键重试/重启/0x00000431
        let e430 = InstallError::MarkedForDelete(ERROR_SERVICE_MARKED_FOR_DELETE);
        let m430 = e430.message();
        assert!(m430.contains("标记删除"));
        assert!(m430.contains("0x00000430"));
        assert!(m430.contains("重启电脑"));
        // 0x431 不应混称「标记删除」语义（设计文档：分别表述避免误导）
        let e431 = InstallError::MarkedForDelete(ERROR_CANNOT_DETECT_DRIVER_FAILURE);
        let m431 = e431.message();
        assert!(m431.contains("0x00000431"));
        assert!(m431.contains("火绒"));
        assert!(m431.contains("信任区"));
        assert!(m431.contains("PawnIO.sys"));
        assert!(m431.contains("一键重试"));
        assert!(m431.contains("重启电脑"));
        // 反向断言：0x431 不得混称「标记删除」语义（设计文档红线：分别表述避免误导）
        assert!(!m431.contains("标记删除"));
    }

    #[test]
    fn blocked_message_contains_guidance() {
        // R8：BlockedBySecurity 文案必须含错误码 + 杀软配置指引
        let e = InstallError::BlockedBySecurity(5);
        let msg = e.message();
        assert!(msg.contains("0x00000005"));
        assert!(msg.contains("安全软件"));
        assert!(msg.contains("Defender"));
        assert!(msg.contains("火绒"));
        assert!(msg.contains("360"));
        assert!(msg.contains("白名单"));
    }

    #[test]
    fn process_running_matches() {
        // 进程行小写匹配：命中 / 未命中 / 大小写不敏感
        let lines = vec![
            "\"msmpeng.exe\",\"1234\",\"Services\"".to_string(),
            "\"HipsDaemon.exe\",\"5678\",\"Console\"".to_string(),
        ];
        assert!(process_running(&lines, "MsMpEng.exe"));
        assert!(process_running(&lines, "hipsdaemon.exe"));
        assert!(!process_running(&lines, "avp.exe"));
        assert!(!process_running(&lines, "notepad.exe"));
        // 空列表不误报
        assert!(!process_running(&[], "msmpeng.exe"));
    }

    #[test]
    fn security_list_from_lines() {
        // 注入进程行：火绒 active + blocked；Defender 未运行 inactive；联想不标 blocked
        let lines = vec!["\"HipsTray.exe\",\"1\"".to_string()];
        let list = security_list_for_lines(&lines);
        let huorong = list.iter().find(|s| s.name == "huorong").unwrap();
        assert!(huorong.active);
        assert!(huorong.blocked_driver);
        assert_eq!(huorong.display_name, "火绒安全软件");
        let defender = list.iter().find(|s| s.name == "defender").unwrap();
        assert!(!defender.active);
        assert!(!defender.blocked_driver);
        let lenovo = list.iter().find(|s| s.name == "lenovo").unwrap();
        assert!(!lenovo.active);
        assert!(!lenovo.blocked_driver);
        // 空列表：全量 inactive（不 panic）
        let empty = security_list_for_lines(&[]);
        assert_eq!(empty.len(), SECURITY_CANDIDATES.len());
        assert!(empty.iter().all(|s| !s.active));
    }

    /// 真机杀软检测验证（默认忽略）：当前机器上应能检测到运行中的杀软进程。
    /// `bash scripts/test-lib.sh -- --ignored real_machine_detect_security`
    #[test]
    #[ignore]
    fn real_machine_detect_security() {
        let list = detect_security_software();
        println!("[真机] 杀软检测 = {:#?}", list);
        assert!(!list.is_empty(), "应返回全量候选列表（至少 8 项）");
    }

    /// 真机签名验证（默认忽略）：系统已签名文件应通过 WinVerifyTrust。
    /// `bash scripts/test-lib.sh -- --ignored real_machine_signature_valid`
    #[test]
    #[ignore]
    fn real_machine_signature_valid() {
        // 用系统自带、必然有 Microsoft 签名的文件验证签名函数本身工作正常
        let path = PathBuf::from(r"C:\Windows\System32\kernel32.dll");
        assert!(path.exists(), "kernel32.dll 应存在");
        assert!(driver_signature_valid(&path), "kernel32.dll 签名应验证通过");
    }

    #[test]
    fn normalize_win_path_strips_prefix() {
        let p = PathBuf::from(r"\\?\C:\Program Files\SECM\resources\driver\PawnIO.sys");
        assert_eq!(
            normalize_win_path(&p),
            r"C:\Program Files\SECM\resources\driver\PawnIO.sys"
        );
        let plain = PathBuf::from(r"C:\SECM\PawnIO.sys");
        assert_eq!(normalize_win_path(&plain), r"C:\SECM\PawnIO.sys");
    }

    #[test]
    fn install_error_messages_contain_code() {
        // R8：错误文案必须含 Win32 错误码
        let e = InstallError::Signature(577);
        let msg = e.message();
        assert!(msg.contains("0x00000241") || msg.contains("0x241"));
        assert!(msg.contains("签名"));
        let e2 = InstallError::MissingDriver("PawnIO.sys".to_string());
        assert!(e2.message().contains("PawnIO.sys"));
    }

    #[test]
    fn error_code_formatting_matches_log_style() {
        // 日志格式约定：0x%08X
        let code = 577u32;
        assert_eq!(format!("0x{:08X}", code), "0x00000241");
        let code2 = 5u32;
        assert_eq!(format!("0x{:08X}", code2), "0x00000005");
    }

    #[test]
    fn cache_ttl_logic() {
        // 缓存 TTL 常量与逻辑：30s 内视为有效
        assert_eq!(CACHE_TTL, Duration::from_secs(30));
        let t = Instant::now();
        assert!(t.elapsed() < CACHE_TTL);
    }

    #[test]
    fn resolve_driver_dir_env_override() {
        // 环境变量注入优先（测试隔离）
        std::env::set_var("SECM_DRIVER_DIR", r"C:\tmp\secm_driver");
        let d = resolve_driver_dir(None);
        assert_eq!(d, PathBuf::from(r"C:\tmp\secm_driver"));
        std::env::remove_var("SECM_DRIVER_DIR");
    }

    /// 查询服务状态（真机测试辅助，直接 FFI 核对服务实态）。
    fn scm_query_service_state(name: &str) -> Option<(u32, u32)> {
        let name_utf16 = to_utf16(name);
        // SAFETY: NULL 机器名/数据库名 = 本机默认 SCM 数据库
        let scm = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_ALL_ACCESS) };
        if scm.is_null() {
            return None;
        }
        // SAFETY: name 为 NUL 结尾 UTF-16；SERVICE_QUERY_STATUS(0x4) 普通用户可查
        let h = unsafe { OpenServiceW(scm, name_utf16.as_ptr(), 0x0004) };
        if h.is_null() {
            // SAFETY: 句柄由 OpenSCManagerW 返回
            unsafe {
                CloseServiceHandle(scm);
            }
            return None;
        }
        let mut st = ServiceStatus {
            service_type: 0,
            current_state: 0,
            controls_accepted: 0,
            win32_exit_code: 0,
            service_specific_exit_code: 0,
            check_point: 0,
            wait_hint: 0,
        };
        // SAFETY: st 指向栈上结构
        let ok = unsafe { QueryServiceStatus(h, &mut st) };
        // SAFETY: 句柄分别由 OpenServiceW/OpenSCManagerW 返回
        unsafe {
            CloseServiceHandle(h);
            CloseServiceHandle(scm);
        }
        if ok == 0 {
            None
        } else {
            Some((st.current_state, st.win32_exit_code))
        }
    }

    /// 真机端到端验证（默认忽略；会真实查询 SCM 服务，需管理员）：
    /// 验证 PawnIO 通道探测与幂等。
    /// `bash scripts/test-lib.sh -- --ignored real_machine_ensure_installed`
    #[test]
    #[ignore]
    fn real_machine_ensure_installed() {
        // 用环境变量指向真实驱动资源目录（src-tauri/resources/driver）
        let driver_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("driver");
        std::env::set_var("SECM_DRIVER_DIR", &driver_dir);
        let status = crate::driver_install::ensure_temp_channel_opt(None);
        println!("[真机] DriverStatus = {:#?}", status);
        // v3 收窄：通道只能是 pawnio（服务就绪）或 none（未就绪）
        assert!(
            ["pawnio", "none"].contains(&status.channel),
            "通道应命中 pawnio/none（实际 {}）",
            status.channel
        );
        // 幂等验证：连调 3 次结果一致（缓存命中）
        for _ in 0..2 {
            let s2 = crate::driver_install::ensure_temp_channel_opt(None);
            assert_eq!(s2.channel, status.channel);
        }
        // 服务实态核对（仅当 PawnIO 判定可用时）
        if status.channel == "pawnio" {
            let state = scm_query_service_state(crate::driver_install::pawnio::SERVICE_PAWNIO);
            assert_eq!(state, Some((SERVICE_RUNNING, 0)));
        }
    }
}
