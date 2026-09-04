//! driver_install::pawnio —— PawnIO 驱动服务部署模块（LHM 温度链依赖）。
//!
//! 依据 `docs/designs/2026-08-09-pawnio-bundle.md` §3.2 实现：
//!
//! ```text
//! ensure_pawnio_ready()（幂等：30s 缓存 + 全局互斥；调用方置于 spawn_blocking）
//!   ├─ L0: 服务已存在
//!   │      运行中 → 直接复用（低权限查询，普通用户 + 官方预装场景零噪音）
//!   │      已停止 → 管理员权限下 start_service_with_retry 复用（不重建、不覆盖）
//!   ├─ L1: 服务不存在（需管理员）
//!   │      定位随包 PawnIO.sys（复用 resolve_driver_dir 布局，与 WinRing0x64.sys 同目录）
//!   │      → driver_signature_valid 签名校验（WHQL 应通过；失败拒绝部署）
//!   │      → 复制到 %windir%\System32\drivers\PawnIO.sys → 建服务 → 启动
//!   └─ L2: 任一步失败 → 错误分类（复用 classify_install_error + start_service_with_retry
//!          内置 next_retry_delay 重试）→ 日志完整（用户名/错误码/API 名/建议）
//!          → 返回 Err（调用方降级原链，本模块绝不 panic）
//! ```
//!
//! 保守策略（设计 §3.2）：服务已存在时**不覆盖/不删除**用户已有实例（可能为官方
//! DriverStore 安装）；版本/路径差异仅日志。卸载不清理本服务（与 WinRing0 一致）。

use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

// ============================================================================
// 常量（与官方安装一致：服务名 "PawnIO"、驱动 PawnIO.sys）
// ============================================================================

/// PawnIO 服务名（与官方安装器一致，见设计 §2.2）
pub const SERVICE_PAWNIO: &str = "PawnIO";
/// PawnIO 服务显示名
pub const SERVICE_PAWNIO_DISPLAY: &str = "PawnIO Service";
/// PawnIO 驱动文件名（随包分发，WHQL 2.2.0）
pub const PAWNIO_SYS: &str = "PawnIO.sys";
/// 部署目标相对子目录（%windir% 下）
const DRIVERS_REL: &str = r"System32\drivers";

// ============================================================================
// 状态缓存与部署互斥（幂等核心，仿 ensure_temp_channel）
// ============================================================================

/// 状态缓存：30s 内重复调用直接返回（Ok/Err 均缓存；避免每轮 500ms 查询触发 SCM/部署）
static PAWNIO_CACHE: Mutex<Option<(Instant, Result<(), String>)>> = Mutex::new(None);
/// 部署互斥：并发触发（启动自愈线程 + LHM 首查）只执行一次部署
static PAWNIO_MUTEX: Mutex<()> = Mutex::new(());
/// 缓存有效期
const PAWNIO_CACHE_TTL: Duration = Duration::from_secs(30);

/// 驱动资源目录缓存（init 时按 AppHandle 解析；NSIS 安装布局下定位随包 PawnIO.sys）
static PAWNIO_DRIVER_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

// ============================================================================
// 初始化与资源定位
// ============================================================================

/// 初始化：解析并缓存驱动资源目录（lib.rs setup 调用；lhm.rs 无 AppHandle 时也能定位）。
pub fn init(app: Option<&tauri::AppHandle>) {
    let dir = resolve_driver_dir(app);
    log_info("pawnio::init", &format!("驱动资源目录: {}", dir.display()));
    let _ = PAWNIO_DRIVER_DIR.set(Some(dir));
}

/// 获取驱动资源目录：优先 init 缓存，未 init 时现场解析。
fn current_driver_dir(app: Option<&tauri::AppHandle>) -> PathBuf {
    if let Some(dir) = PAWNIO_DRIVER_DIR.get().cloned().flatten() {
        return dir;
    }
    resolve_driver_dir(app)
}

/// 部署目标路径：%windir%\System32\drivers\PawnIO.sys。
fn pawnio_dest_path() -> PathBuf {
    // Windows 必有 windir 环境变量；缺失时回退 C:\Windows（纯函数可测）
    let windir = std::env::var("windir").unwrap_or_else(|_| r"C:\Windows".to_string());
    PathBuf::from(windir).join(DRIVERS_REL).join(PAWNIO_SYS)
}

// ============================================================================
// 部署计划与错误分类（纯逻辑，可单测）
// ============================================================================

/// 部署计划：根据服务当前状态决定动作。
enum PawnioPlan {
    /// 服务已运行 → 直接复用
    Ready,
    /// 服务已存在但停止 → 管理员权限下启动复用（不重建）
    StartExisting,
    /// 服务不存在 → 管理员权限下完整部署
    Install,
}

/// 根据服务状态生成部署计划（纯函数；`None` = 服务不存在或查询失败）。
fn plan_pawnio_action(service_state: Option<u32>) -> PawnioPlan {
    match service_state {
        Some(state) if state == SERVICE_RUNNING => PawnioPlan::Ready,
        Some(_) => PawnioPlan::StartExisting,
        None => PawnioPlan::Install,
    }
}

/// PawnIO 部署错误分类标签（复用 mod.rs classify_install_error 判定语义，
/// 但文案为 PawnIO 专属，避免误导用户去放行 WinRing0x64.sys）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PawnioErrorKind {
    /// 签名/加载策略错误（577/1275/0xC0000428）
    Signature,
    /// 安全软件拦截（5 且驱动签名有效）
    BlockedBySecurity,
    /// 服务标记删除/内核拒载（0x430/0x431）
    MarkedForDelete,
    /// 其他失败
    Failed,
}

/// 把 mod.rs 的 `InstallError` 分类转换为 PawnIO 专属分类（纯逻辑，可单测）。
fn to_pawnio_kind(e: InstallError) -> PawnioErrorKind {
    match e {
        InstallError::Signature(_) => PawnioErrorKind::Signature,
        InstallError::BlockedBySecurity(_) => PawnioErrorKind::BlockedBySecurity,
        InstallError::MarkedForDelete(_) => PawnioErrorKind::MarkedForDelete,
        InstallError::Failed(_) | InstallError::MissingDriver(_) => PawnioErrorKind::Failed,
    }
}

/// 构造 R8 达标的错误消息：API 名 + Win32 错误码 + 用户名 + 建议修复。
/// `driver` 用于签名分流（5 + 签名有效 → 杀软拦截）。
fn failure_message(api: &str, code: u32, driver: Option<&Path>) -> String {
    // 复用 mod.rs 分类逻辑（含 WinVerifyTrust 签名分流）
    let kind = to_pawnio_kind(classify_install_error(code, driver));
    failure_message_inner(api, code, kind)
}

/// 错误文案纯逻辑（分类结果已注入，可单测）。
fn failure_message_inner(api: &str, code: u32, kind: PawnioErrorKind) -> String {
    let username = std::env::var("USERNAME").unwrap_or_else(|_| "unknown".to_string());
    match kind {
        PawnioErrorKind::Signature => format!(
            "[{}] PawnIO 驱动签名/加载被拒绝（win32=0x{:08X}，user={}）。WHQL 驱动在 Secure Boot 下应可加载，请检查系统驱动签名策略。",
            api, code, username
        ),
        PawnioErrorKind::BlockedBySecurity => format!(
            "[{}] PawnIO 驱动启动被安全软件拦截（win32=0x{:08X}，user={}，驱动签名有效但服务启动被拒）。请按引导卡在安全软件（火绒等）驱动防护/信任区放行 PawnIO.sys 与 SECM 目录后重试。",
            api, code, username
        ),
        PawnioErrorKind::MarkedForDelete => {
            if code == ERROR_SERVICE_MARKED_FOR_DELETE {
                format!(
                    "[{}] PawnIO 服务处于标记删除状态（win32=0x{:08X}，user={}），删除异步完成中。请重启电脑后重试。",
                    api, code, username
                )
            } else {
                format!(
                    "[{}] PawnIO 驱动加载被内核拒绝（win32=0x{:08X}，user={}，ERROR_CANNOT_DETECT_DRIVER_FAILURE），疑似安全软件驱动防护拦截。请放行 PawnIO.sys 后重试；持续失败请重启电脑。",
                    api, code, username
                )
            }
        }
        PawnioErrorKind::Failed => format!(
            "[{}] PawnIO 服务操作失败（win32=0x{:08X}，user={}）。建议：以管理员身份运行 SECM 后重试。",
            api, code, username
        ),
    }
}

// ============================================================================
// 主流程：ensure_pawnio_ready
// ============================================================================

/// 确保 PawnIO 服务就绪（幂等；调用方置于 spawn_blocking）。
///
/// 返回 `Ok(())` = 服务可运行（部署或复用成功）；`Err(msg)` = 失败（含分类/建议，
/// 调用方记录日志后降级原链，本函数绝不 panic、不弹 UAC、不二次提权）。
pub fn ensure_pawnio_ready() -> Result<(), String> {
    ensure_pawnio_ready_opt(None)
}

/// ensure 的可选 app 变体（启动自愈线程传 AppHandle 解析资源目录）。
pub fn ensure_pawnio_ready_opt(app: Option<&tauri::AppHandle>) -> Result<(), String> {
    // 1. 缓存命中（30s）→ 直接返回（Ok/Err 均缓存，节流高频查询）
    if let Ok(guard) = PAWNIO_CACHE.lock() {
        if let Some((t, r)) = guard.as_ref() {
            if t.elapsed() < PAWNIO_CACHE_TTL {
                return r.clone();
            }
        }
    }
    // 2. 部署互斥：并发触发只执行一次；其他线程返回缓存/进行中
    let _guard = match PAWNIO_MUTEX.try_lock() {
        Ok(g) => g,
        Err(_) => {
            log_warn("PAWNIO_MUTEX", "另一线程正在执行 PawnIO 部署检测，返回缓存状态");
            return cached_or_pending();
        }
    };
    // 3. 拿到互斥后复查缓存（等待期间可能已被填充）
    if let Ok(guard) = PAWNIO_CACHE.lock() {
        if let Some((t, r)) = guard.as_ref() {
            if t.elapsed() < PAWNIO_CACHE_TTL {
                return r.clone();
            }
        }
    }
    // 4. 执行检测/部署
    log_info("ensure_pawnio_ready", "开始 PawnIO 服务检测");
    let result = run_pawnio_ensure(app);
    match &result {
        Ok(()) => log_info("ensure_pawnio_ready", "PawnIO 服务就绪"),
        Err(msg) => log_warn("ensure_pawnio_ready", &format!("PawnIO 未就绪: {}", msg)),
    }
    if let Ok(mut guard) = PAWNIO_CACHE.lock() {
        *guard = Some((Instant::now(), result.clone()));
    }
    result
}

/// 互斥被占用时返回：优先缓存，否则"部署进行中"。
fn cached_or_pending() -> Result<(), String> {
    if let Ok(guard) = PAWNIO_CACHE.lock() {
        if let Some((_, r)) = guard.as_ref() {
            return r.clone();
        }
    }
    Err("PawnIO 服务部署正在进行中，稍后重试".to_string())
}

/// 执行检测/部署（L0 → L1 → L2 错误分类）。
fn run_pawnio_ensure(app: Option<&tauri::AppHandle>) -> Result<(), String> {
    // L0 快路径：低权限查询（普通用户可查）；服务已运行 → 直接复用
    let low_state = query_service_state_low_priv(SERVICE_PAWNIO).map(|(s, _)| s);
    if matches!(plan_pawnio_action(low_state), PawnioPlan::Ready) {
        log_info("QueryServiceStatus", &format!("{} 已运行，直接复用", SERVICE_PAWNIO));
        return Ok(());
    }
    // 需要管理员操作（启动已存在服务 / 完整部署）；运行期不弹 UAC（权限模型铁律）
    if !is_elevated() {
        let state_desc = match low_state {
            Some(s) => format!("0x{:X}", s),
            None => "不存在".to_string(),
        };
        let msg = format!(
            "PawnIO 服务需要管理员权限（服务状态: {}）。请以管理员身份运行 SECM 后重试；已运行的服务可被 sidecar 直接复用。",
            state_desc
        );
        log_warn("run_pawnio_ensure", &msg);
        return Err(msg);
    }
    match plan_pawnio_action(low_state) {
        PawnioPlan::Ready => Ok(()),
        PawnioPlan::StartExisting => start_existing_pawnio(),
        PawnioPlan::Install => install_pawnio(&current_driver_dir(app)),
    }
}

/// L0：启动已存在的 PawnIO 服务（保守：不重建、不覆盖用户实例）。
fn start_existing_pawnio() -> Result<(), String> {
    let scm = open_scm().map_err(|code| failure_message("OpenSCManagerW", code, None))?;
    let scm_raw = scm.0;
    let service = open_service(scm_raw, SERVICE_PAWNIO)
        .map_err(|code| failure_message("OpenServiceW", code, None))?
        .ok_or_else(|| "PawnIO 服务在启动前消失（并发删除？），请重试".to_string())?;
    // 带退避重试（杀软延迟放行 / 签名校验瞬时失败，next_retry_delay 内置）
    // 签名分流：StartServiceW 返回 5（ERROR_ACCESS_DENIED）时，若随包驱动存在且
    // 签名有效 → BlockedBySecurity（杀软拦截建议）；驱动缺失时传 None 保持原分类
    // （Failed → 提权建议）。
    let bundled_sys = current_driver_dir(None).join(PAWNIO_SYS);
    let driver_ref = bundled_sys.exists().then(|| bundled_sys.as_path());
    start_service_with_retry(service.0, SERVICE_PAWNIO)
        .map_err(|code| failure_message("StartServiceW", code, driver_ref))
}

/// L1：完整部署——定位随包 sys → 签名校验 → 复制到 System32\drivers → 建服务 → 启动。
fn install_pawnio(driver_dir: &Path) -> Result<(), String> {
    let src_sys = driver_dir.join(PAWNIO_SYS);
    if !src_sys.exists() {
        let msg = format!(
            "随包驱动缺失: {}（请检查安装目录 resources\\driver\\）",
            src_sys.display()
        );
        log_error("install_pawnio", &msg);
        return Err(msg);
    }
    // 签名校验门禁：WHQL 应通过；失败拒绝部署（未签名驱动不得进入系统目录）
    if !driver_signature_valid(&src_sys) {
        let msg = format!(
            "PawnIO.sys 签名验证失败（WinVerifyTrust 拒绝），拒绝部署到系统目录: {}",
            src_sys.display()
        );
        log_error("WinVerifyTrust", &msg);
        return Err(msg);
    }
    // 复制到系统驱动目录（来源经签名校验；目标已存在则记录覆盖）
    let dest = pawnio_dest_path();
    if dest.exists() {
        log_warn("fs::copy", &format!("目标已存在，将覆盖: {}", dest.display()));
    }
    fs::copy(&src_sys, &dest)
        .map_err(|e| format!("复制 {} → {} 失败: {}", src_sys.display(), dest.display(), e))?;
    log_info("fs::copy", &format!("{} 已部署到 {}", PAWNIO_SYS, dest.display()));

    // 打开 SCM（管理员），防竞态复查服务是否存在
    let scm = open_scm().map_err(|code| failure_message("OpenSCManagerW", code, None))?;
    let scm_raw = scm.0;
    match open_service(scm_raw, SERVICE_PAWNIO)
        .map_err(|code| failure_message("OpenServiceW", code, None))?
    {
        // 竞态窗口内服务被创建（如用户手动安装）：按 L0 启动复用，绝不删除重建
        Some(service) => {
            log_warn("install_pawnio", "检测到 PawnIO 服务已存在（竞态），改为启动复用");
            return start_service_with_retry(service.0, SERVICE_PAWNIO)
                .map_err(|code| failure_message("StartServiceW", code, Some(&src_sys)));
        }
        None => {}
    }
    // 创建内核驱动服务（demand start）+ 启动
    let bin_path = normalize_win_path(&dest);
    let name_utf16 = to_utf16(SERVICE_PAWNIO);
    let display_utf16 = to_utf16(SERVICE_PAWNIO_DISPLAY);
    let bin_utf16 = to_utf16(&bin_path);
    // SAFETY: 所有字符串参数均为 NUL 结尾 UTF-16；组/依赖/账号用默认（NULL）
    let service = unsafe {
        CreateServiceW(
            scm_raw,
            name_utf16.as_ptr(),
            display_utf16.as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_KERNEL_DRIVER,
            SERVICE_DEMAND_START,
            SERVICE_ERROR_NORMAL,
            bin_utf16.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
        )
    };
    if service.is_null() {
        // SAFETY: GetLastError 紧邻失败调用
        let code = unsafe { GetLastError() };
        return Err(failure_message("CreateServiceW", code, Some(&src_sys)));
    }
    let guard = ServiceGuard(service);
    log_info("CreateServiceW", &format!("{} 服务创建成功（{}）", SERVICE_PAWNIO, bin_path));
    // 新建后启动（带退避重试）
    start_service_with_retry(guard.0, SERVICE_PAWNIO)
        .map_err(|code| failure_message("StartServiceW", code, Some(&src_sys)))
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_ready_when_running() {
        // 服务已运行 → 直接复用
        assert!(matches!(plan_pawnio_action(Some(SERVICE_RUNNING)), PawnioPlan::Ready));
    }

    #[test]
    fn plan_start_existing_when_stopped() {
        // 服务存在但停止（SERVICE_STOPPED=1 或其他非运行态）→ 启动复用，不重建
        assert!(matches!(
            plan_pawnio_action(Some(SERVICE_STOPPED)),
            PawnioPlan::StartExisting
        ));
        assert!(matches!(plan_pawnio_action(Some(2)), PawnioPlan::StartExisting));
        assert!(matches!(plan_pawnio_action(Some(0)), PawnioPlan::StartExisting));
    }

    #[test]
    fn plan_install_when_missing() {
        // 服务不存在 / 查询失败 → 完整部署
        assert!(matches!(plan_pawnio_action(None), PawnioPlan::Install));
    }

    #[test]
    fn classify_pawnio_signature_errors() {
        // 577 / 1275 / 0xC0000428 均归 Signature（复用 mod.rs 判定）
        assert_eq!(
            to_pawnio_kind(classify_install_error_inner(577, true)),
            PawnioErrorKind::Signature
        );
        assert_eq!(
            to_pawnio_kind(classify_install_error_inner(1275, false)),
            PawnioErrorKind::Signature
        );
        assert_eq!(
            to_pawnio_kind(classify_install_error_inner(0xC000_0428, true)),
            PawnioErrorKind::Signature
        );
    }

    #[test]
    fn classify_pawnio_security_block() {
        // 5 + 签名有效 → 杀软拦截（BlockedBySecurity）
        assert_eq!(
            to_pawnio_kind(classify_install_error_inner(5, true)),
            PawnioErrorKind::BlockedBySecurity
        );
        // 5 + 签名无效 → 普通失败（避免误判 DACL/权限问题）
        assert_eq!(
            to_pawnio_kind(classify_install_error_inner(5, false)),
            PawnioErrorKind::Failed
        );
    }

    #[test]
    fn classify_pawnio_marked_for_delete() {
        // 0x430（服务标记删除）/ 0x431（内核拒载）→ MarkedForDelete，不受签名影响
        assert_eq!(
            to_pawnio_kind(classify_install_error_inner(0x430, true)),
            PawnioErrorKind::MarkedForDelete
        );
        assert_eq!(
            to_pawnio_kind(classify_install_error_inner(0x431, false)),
            PawnioErrorKind::MarkedForDelete
        );
    }

    #[test]
    fn classify_pawnio_failed_others() {
        // 其他错误码（1060 服务不存在 / 2 找不到文件）→ Failed
        assert_eq!(
            to_pawnio_kind(classify_install_error_inner(1060, false)),
            PawnioErrorKind::Failed
        );
        assert_eq!(
            to_pawnio_kind(classify_install_error_inner(2, true)),
            PawnioErrorKind::Failed
        );
    }

    #[test]
    fn failure_message_r8_requirements() {
        // R8：错误文案必须含 API 名 + 错误码 + 用户名 + 建议修复
        let msg = failure_message_inner("StartServiceW", 5, PawnioErrorKind::BlockedBySecurity);
        assert!(msg.contains("StartServiceW"), "应含 API 名: {}", msg);
        assert!(msg.contains("0x00000005"), "应含错误码: {}", msg);
        assert!(msg.contains("安全软件"), "应含分类语义: {}", msg);
        assert!(msg.contains("PawnIO.sys"), "应含放行对象: {}", msg);
        // 用户名（不依赖真实 USERNAME，unknown 兜底也可接受，但格式必须存在）
        assert!(
            msg.contains("user="),
            "应含用户名字段: {}",
            msg
        );
    }

    #[test]
    fn failure_message_marked_for_delete_guidance() {
        // 0x430：重启电脑建议；0x431：内核拒绝 + 重启建议
        let m430 = failure_message_inner("StartServiceW", 0x430, PawnioErrorKind::MarkedForDelete);
        assert!(m430.contains("0x00000430"));
        assert!(m430.contains("标记删除"));
        assert!(m430.contains("重启电脑"));
        let m431 = failure_message_inner("StartServiceW", 0x431, PawnioErrorKind::MarkedForDelete);
        assert!(m431.contains("0x00000431"));
        assert!(m431.contains("内核拒绝"));
        assert!(m431.contains("重启电脑"));
        // 0x431 不得混称「标记删除」语义（与 mod.rs 红线一致）
        assert!(!m431.contains("标记删除"));
    }

    #[test]
    fn failure_message_signature_guidance() {
        let msg = failure_message_inner("CreateServiceW", 577, PawnioErrorKind::Signature);
        assert!(msg.contains("0x00000241"));
        assert!(msg.contains("签名"));
        assert!(msg.contains("Secure Boot"));
    }

    #[test]
    fn dest_path_uses_windir() {
        // 部署路径契约：%windir%\System32\drivers\PawnIO.sys
        std::env::set_var("windir", r"C:\TestWindows");
        let p = pawnio_dest_path();
        assert_eq!(p, PathBuf::from(r"C:\TestWindows\System32\drivers\PawnIO.sys"));
        std::env::remove_var("windir");
        // windir 缺失 → 回退 C:\Windows
        let p2 = pawnio_dest_path();
        assert_eq!(p2, PathBuf::from(r"C:\Windows\System32\drivers\PawnIO.sys"));
    }

    #[test]
    fn cache_ttl_is_30s() {
        // 缓存 TTL 契约（设计 §3.2：30s 节流）
        assert_eq!(PAWNIO_CACHE_TTL, Duration::from_secs(30));
    }
}
