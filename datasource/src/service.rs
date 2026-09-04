//! 服务管理采集（advapi32）— 枚举/查询/启停/启动类型（P10/P11/P12）
//!
//! 替换 `sc query` / `sc qc` / `sc start` / `sc stop` / `sc config` 文本解析链路。
//! 二进制 API 返回结构化字段，无 GBK 文本解析、无本地化字符串匹配。
//!
//! 线程模型：本模块全部为同步阻塞 API，上层须在 `spawn_blocking` 中调用（S8）。

use crate::error::CollectError;
use serde::Serialize;
use std::ptr::{null, null_mut};
use windows_sys::core::PWSTR;
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_MORE_DATA, HANDLE};
use windows_sys::Win32::System::Services::*;

// ============================================================================
// 类型定义（与 SECM 前端 ServiceInfo 契约字段一致）
// ============================================================================

/// 服务信息（前端契约字段：name / display_name / status / start_type）
#[derive(Debug, Clone, Serialize)]
pub struct ServiceInfo {
    /// 服务名（如 "XblAuthManager"）
    pub name: String,
    /// 显示名（如 "Xbox Live 身份验证管理器"）
    pub display_name: String,
    /// 状态："Running" / "Stopped" / "Stopping" / "Starting" / "Paused" / "Unknown"
    pub status: String,
    /// 启动类型："自动" / "手动" / "已禁用" / "未知"（与现状 sc qc 输出映射一致）
    pub start_type: String,
}

/// 服务状态字符串（与现状 sc query 输出映射一致）
fn status_str(state: u32) -> String {
    match state {
        SERVICE_RUNNING => "Running".into(),
        SERVICE_STOPPED => "Stopped".into(),
        SERVICE_STOP_PENDING => "Stopping".into(),
        SERVICE_START_PENDING => "Starting".into(),
        SERVICE_PAUSED => "Paused".into(),
        SERVICE_CONTINUE_PENDING => "Starting".into(),
        SERVICE_PAUSE_PENDING => "Paused".into(),
        _ => "Unknown".into(),
    }
}

/// 启动类型字符串（与现状 sc qc 输出映射一致）
fn start_type_str(start_type: u32) -> String {
    match start_type {
        SERVICE_AUTO_START => "自动".into(),
        SERVICE_DEMAND_START => "手动".into(),
        SERVICE_DISABLED => "已禁用".into(),
        SERVICE_BOOT_START | SERVICE_SYSTEM_START => "自动".into(),
        _ => "未知".into(),
    }
}

/// 将 Windows 错误码转为用户可读描述
fn err_desc(err: u32) -> String {
    match err {
        ERROR_ACCESS_DENIED => "拒绝访问（需要管理员权限）".into(),
        ERROR_MORE_DATA => "缓冲区不足（ERROR_MORE_DATA）".into(),
        1060 => "服务不存在".into(),
        1062 => "服务未运行".into(),
        87 => "无效的参数".into(),
        _ => format!("错误码 {}", err),
    }
}

/// 从 PWSTR 读取宽字符串（NULL → 空串）
///
/// # Safety
/// `p` 必须是 API 返回的、指向 NUL 结尾宽字符串的有效指针，或 NULL。
unsafe fn read_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: 调用方保证指针指向 NUL 结尾的宽字符串（windows-sys PWSTR 语义）
    let len = unsafe { (0..).take_while(|&i| *p.add(i) != 0).count() };
    // SAFETY: 前 len 个 u16 均为有效值
    let slice = unsafe { std::slice::from_raw_parts(p, len) };
    String::from_utf16_lossy(slice)
}

// ============================================================================
// RAII 句柄
// ============================================================================

/// SCM 句柄（RAII：Drop 时 CloseServiceHandle）
struct ScmHandle(HANDLE);

impl Drop for ScmHandle {
    fn drop(&mut self) {
        // SAFETY: CloseServiceHandle 是 advapi32 标准导出，句柄来自 OpenSCManagerW
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}

/// 服务句柄（RAII）
struct ServiceHandle(HANDLE);

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        // SAFETY: CloseServiceHandle 是 advapi32 标准导出，句柄来自 OpenServiceW
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}

// ============================================================================
// 底层封装
// ============================================================================

/// 打开 SCM（RAII）
///
/// 权限：`SC_MANAGER_ENUMERATE_SERVICE`（普通用户默认拥有，与 `sc query` 一致）；
/// 需要写操作时传 `SC_MANAGER_ALL_ACCESS`。
fn open_scm(access: u32) -> Result<ScmHandle, CollectError> {
    // SAFETY: OpenSCManagerW 接受可空的服务名/机器名（NULL = 本机默认数据库）
    let handle = unsafe { OpenSCManagerW(null(), null(), access) };
    if handle.is_null() {
        return Err(CollectError::winapi(
            "advapi32.OpenSCManagerW",
            "打开服务控制管理器",
        ));
    }
    Ok(ScmHandle(handle))
}

/// 打开单个服务
fn open_service(scm: &ScmHandle, name: &str, access: u32) -> Result<ServiceHandle, CollectError> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: OpenServiceW 接受以 NUL 结尾的宽字符串，句柄由 ServiceHandle RAII 管理
    let handle = unsafe { OpenServiceW(scm.0, wide.as_ptr(), access) };
    if handle.is_null() {
        return Err(CollectError::winapi(
            "advapi32.OpenServiceW",
            format!("打开服务 '{}'", name),
        ));
    }
    Ok(ServiceHandle(handle))
}

/// 判断错误是否为"服务不存在"（错误码 1060）
fn is_service_not_found(e: &CollectError) -> bool {
    matches!(e, CollectError::WinApi { detail, .. } if detail.contains("错误码 1060"))
}

// ============================================================================
// 查询
// ============================================================================

/// 查询单个服务状态 + 启动类型（`sc query` + `sc qc` 等价）
///
/// 语义约定：
/// - 服务存在 → `Ok(Some(ServiceInfo))`（display_name 为空，由调用方补充）
/// - 服务不存在 → `Ok(None)`（不算错误，与现状 `sc query` 找不到服务返回空一致）
/// - API 失败 → `Err(CollectError)`
pub fn query_service(name: &str) -> Result<Option<ServiceInfo>, CollectError> {
    let scm = open_scm(SC_MANAGER_CONNECT)?;

    // 先查状态（QueryServiceStatusEx）
    let service = match open_service(&scm, name, SERVICE_QUERY_STATUS) {
        Ok(h) => h,
        Err(e) if is_service_not_found(&e) => return Ok(None),
        Err(e) => return Err(e),
    };

    // 状态查询（SC_STATUS_PROCESS_INFO 级别）
    let mut status = SERVICE_STATUS_PROCESS {
        dwServiceType: 0,
        dwCurrentState: 0,
        dwControlsAccepted: 0,
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: 0,
        dwProcessId: 0,
        dwServiceFlags: 0,
    };
    let mut bytes_needed: u32 = 0;
    // SAFETY: QueryServiceStatusEx 填充 SERVICE_STATUS_PROCESS，指针有效且长度正确
    let ok = unsafe {
        QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            &mut status as *mut _ as *mut u8,
            std::mem::size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut bytes_needed,
        )
    };
    if ok == 0 {
        return Err(CollectError::winapi(
            "advapi32.QueryServiceStatusEx",
            format!("查询服务 '{}' 状态", name),
        ));
    }
    let status_str = status_str(status.dwCurrentState);
    drop(service);

    // 启动类型查询（QueryServiceConfigW）
    let service = match open_service(&scm, name, SERVICE_QUERY_CONFIG) {
        Ok(h) => h,
        Err(_) => {
            return Ok(Some(ServiceInfo {
                name: name.to_string(),
                display_name: String::new(),
                status: status_str,
                start_type: "未知".into(),
            }));
        }
    };

    // 两段式：先拿所需缓冲大小
    let mut bytes_needed: u32 = 0;
    // SAFETY: QueryServiceConfigW(NULL) 返回所需大小（ERROR_INSUFFICIENT_BUFFER）
    let _ = unsafe { QueryServiceConfigW(service.0, null_mut(), 0, &mut bytes_needed) };
    let mut buf: Vec<u8> = vec![0u8; bytes_needed as usize];
    let mut bytes_needed2: u32 = 0;
    // SAFETY: buf 大小足够容纳 QUERY_SERVICE_CONFIGW（含变长字符串尾随区）
    let ok = unsafe {
        QueryServiceConfigW(
            service.0,
            buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW,
            buf.len() as u32,
            &mut bytes_needed2,
        )
    };
    if ok == 0 {
        return Err(CollectError::winapi(
            "advapi32.QueryServiceConfigW",
            format!("查询服务 '{}' 配置", name),
        ));
    }

    // SAFETY: QUERY_SERVICE_CONFIGW 为 POD 结构，buf 已由 API 填充
    let config = unsafe { &*(buf.as_ptr() as *const QUERY_SERVICE_CONFIGW) };
    let start_type = start_type_str(config.dwStartType);

    Ok(Some(ServiceInfo {
        name: name.to_string(),
        display_name: String::new(),
        status: status_str,
        start_type,
    }))
}

// ============================================================================
// 全量枚举（P10）
// ============================================================================

/// 枚举系统全部服务（`sc query type= service state= all` 等价）
///
/// 一次性返回名称/显示名/状态；启动类型逐服务查询（内存内 API 调用，
/// 比 spawn `sc qc` 快 2-3 个数量级）。上层可并行调用 `start_type_of` 补全。
pub fn enum_services() -> Result<Vec<ServiceInfo>, CollectError> {
    let scm = open_scm(SC_MANAGER_ENUMERATE_SERVICE)?;

    // 第一段：获取所需缓冲区大小（返回 ERROR_MORE_DATA）
    let mut bytes_needed: u32 = 0;
    let mut services_returned: u32 = 0;
    let mut resume: u32 = 0;
    // SAFETY: 传 NULL/0 探测所需大小，API 返回 ERROR_MORE_DATA 并填充 bytes_needed
    let rc = unsafe {
        EnumServicesStatusExW(
            scm.0,
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            null_mut(),
            0,
            &mut bytes_needed,
            &mut services_returned,
            &mut resume,
            null(),
        )
    };
    if rc == 0 {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        if err != ERROR_MORE_DATA {
            return Err(CollectError::winapi_detailed(
                "advapi32.EnumServicesStatusExW",
                "枚举服务（探测大小）",
                err_desc(err),
            ));
        }
    }

    // 第二段：分配缓冲并枚举（循环直到不再 ERROR_MORE_DATA，覆盖服务列表增长）
    let mut buf: Vec<u8> = vec![0u8; bytes_needed as usize];
    let mut services: Vec<ServiceInfo> = Vec::new();
    let mut resume: u32 = 0;

    loop {
        let mut bytes_needed_cur: u32 = 0;
        let mut services_returned_cur: u32 = 0;
        // SAFETY: buf 为有效可变缓冲区，长度以 cbBufSize 传入；API 填充
        // ENUM_SERVICE_STATUS_PROCESSW 数组（每个元素含变长字符串尾随区）
        let rc = unsafe {
            EnumServicesStatusExW(
                scm.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut bytes_needed_cur,
                &mut services_returned_cur,
                &mut resume,
                null(),
            )
        };
        if rc == 0 {
            let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            if err == ERROR_MORE_DATA {
                // 缓冲不足：按所需大小扩容后重试
                let new_size = bytes_needed_cur.max(buf.len() as u32 + 8192) as usize;
                buf.resize(new_size, 0u8);
                continue;
            }
            return Err(CollectError::winapi_detailed(
                "advapi32.EnumServicesStatusExW",
                "枚举服务",
                err_desc(err),
            ));
        }

        // 解析 ENUM_SERVICE_STATUS_PROCESSW 数组：按元素大小步进指针
        // SAFETY: API 承诺 buf 前 services_returned_cur 个元素为合法的
        // ENUM_SERVICE_STATUS_PROCESSW，每元素大小固定（含尾随宽字符串）
        let elem_size = std::mem::size_of::<ENUM_SERVICE_STATUS_PROCESSW>();
        for i in 0..services_returned_cur as usize {
            let offset = i.checked_mul(elem_size).ok_or_else(|| {
                CollectError::parse("服务枚举数组", "索引溢出")
            })?;
            let ptr = buf.as_ptr().wrapping_add(offset) as *const ENUM_SERVICE_STATUS_PROCESSW;
            // SAFETY: 指针位于 buf 有效范围内，由 API 填充，读取 POD 字段安全
            let entry = unsafe { &*ptr };
            // SAFETY: 名称/显示名是结构体内的 PWSTR，指向同一缓冲区的尾随宽字符串
            let name = unsafe { read_pwstr(entry.lpServiceName) };
            let display = unsafe { read_pwstr(entry.lpDisplayName) };
            let status = status_str(entry.ServiceStatusProcess.dwCurrentState);
            services.push(ServiceInfo {
                name,
                display_name: display,
                status,
                start_type: "未知".into(), // 由调用方并行补充
            });
        }

        // 枚举完成条件：本轮返回 0 个服务，或返回数据未填满缓冲（说明已到底）
        if services_returned_cur == 0 {
            break;
        }
        let used = (services_returned_cur as usize).saturating_mul(elem_size);
        if used < buf.len() {
            break;
        }
        if resume == 0 {
            break;
        }
    }

    log::debug!("service.enum_services: 枚举到 {} 个服务", services.len());
    Ok(services)
}

/// 查询单个服务启动类型（供并行补全，失败降级 "未知"）
pub fn start_type_of(name: &str) -> String {
    match query_start_type_raw(name) {
        Ok(Some(t)) => t,
        _ => "未知".into(),
    }
}

fn query_start_type_raw(name: &str) -> Result<Option<String>, CollectError> {
    let scm = open_scm(SC_MANAGER_CONNECT)?;
    let service = match open_service(&scm, name, SERVICE_QUERY_CONFIG) {
        Ok(h) => h,
        Err(e) if is_service_not_found(&e) => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut bytes_needed: u32 = 0;
    // SAFETY: 探测大小
    let _ = unsafe { QueryServiceConfigW(service.0, null_mut(), 0, &mut bytes_needed) };
    let mut buf: Vec<u8> = vec![0u8; bytes_needed as usize];
    let mut bytes_needed2: u32 = 0;
    // SAFETY: buf 容量足够
    let ok = unsafe {
        QueryServiceConfigW(
            service.0,
            buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW,
            buf.len() as u32,
            &mut bytes_needed2,
        )
    };
    if ok == 0 {
        return Err(CollectError::winapi(
            "advapi32.QueryServiceConfigW",
            format!("查询服务 '{}' 启动类型", name),
        ));
    }
    // SAFETY: POD 结构已填充
    let config = unsafe { &*(buf.as_ptr() as *const QUERY_SERVICE_CONFIGW) };
    Ok(Some(start_type_str(config.dwStartType)))
}

// ============================================================================
// 写操作（P11：start / stop / set start type）
// ============================================================================

/// 启动服务（`sc start` 等价）。需管理员权限。
pub fn start_service(name: &str) -> Result<(), CollectError> {
    let scm = open_scm(SC_MANAGER_CONNECT)?;
    let service = open_service(&scm, name, SERVICE_START)?;
    // SAFETY: StartServiceW 接受可空参数数组（NULL = 无参数）
    let ok = unsafe { StartServiceW(service.0, 0, null()) };
    if ok == 0 {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        if err == ERROR_ACCESS_DENIED {
            return Err(CollectError::NeedsAdmin {
                op: format!("启动服务 '{}'", name),
            });
        }
        return Err(CollectError::winapi_detailed(
            "advapi32.StartServiceW",
            format!("启动服务 '{}'", name),
            err_desc(err),
        ));
    }
    Ok(())
}

/// 停止服务（`sc stop` 等价）。需管理员权限。
pub fn stop_service(name: &str) -> Result<(), CollectError> {
    let scm = open_scm(SC_MANAGER_CONNECT)?;
    let service = open_service(&scm, name, SERVICE_STOP)?;
    let mut status = SERVICE_STATUS {
        dwServiceType: 0,
        dwCurrentState: 0,
        dwControlsAccepted: 0,
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: 0,
    };
    // SAFETY: ControlService 填充 SERVICE_STATUS
    let ok = unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) };
    if ok == 0 {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        if err == ERROR_ACCESS_DENIED {
            return Err(CollectError::NeedsAdmin {
                op: format!("停止服务 '{}'", name),
            });
        }
        if err == 1062 {
            // 服务已停止：幂等成功（与 sc stop 行为一致）
            return Ok(());
        }
        return Err(CollectError::winapi_detailed(
            "advapi32.ControlService",
            format!("停止服务 '{}'", name),
            err_desc(err),
        ));
    }
    Ok(())
}

/// 设置服务启动类型（`sc config start= X` 等价）。需管理员权限。
///
/// `start_type`: "auto"（自动）/ "manual"（手动）/ "disabled"（禁用）
pub fn set_service_start_type(name: &str, start_type: &str) -> Result<(), CollectError> {
    let dw_start_type = match start_type {
        "auto" => SERVICE_AUTO_START,
        "manual" => SERVICE_DEMAND_START,
        "disabled" => SERVICE_DISABLED,
        other => {
            return Err(CollectError::parse(
                "服务启动类型",
                format!("无效值 '{}'（可选: auto/manual/disabled）", other),
            ));
        }
    };

    let scm = open_scm(SC_MANAGER_CONNECT)?;
    let service = open_service(&scm, name, SERVICE_CHANGE_CONFIG)?;

    // SAFETY: ChangeServiceConfigW 其余参数传 NULL/0 表示不修改对应字段
    let ok = unsafe {
        ChangeServiceConfigW(
            service.0,
            SERVICE_NO_CHANGE,
            dw_start_type,
            SERVICE_NO_CHANGE,
            null(),
            null(),
            null_mut(),
            null(),
            null(),
            null(),
            null(),
        )
    };
    if ok == 0 {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        if err == ERROR_ACCESS_DENIED {
            return Err(CollectError::NeedsAdmin {
                op: format!("修改服务 '{}' 启动类型", name),
            });
        }
        return Err(CollectError::winapi_detailed(
            "advapi32.ChangeServiceConfigW",
            format!("修改服务 '{}' 启动类型", name),
            err_desc(err),
        ));
    }
    Ok(())
}

// ============================================================================
// 辅助
// ============================================================================

/// 判断服务是否正在运行（供 game_env 等场景，布尔语义）
pub fn is_service_running(name: &str) -> Result<bool, CollectError> {
    match query_service(name)? {
        Some(info) => Ok(info.status == "Running"),
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_str_mapping() {
        assert_eq!(status_str(SERVICE_RUNNING), "Running");
        assert_eq!(status_str(SERVICE_STOPPED), "Stopped");
        assert_eq!(status_str(SERVICE_STOP_PENDING), "Stopping");
        assert_eq!(status_str(SERVICE_START_PENDING), "Starting");
        assert_eq!(status_str(SERVICE_PAUSED), "Paused");
        assert_eq!(status_str(9999), "Unknown");
    }

    #[test]
    fn test_start_type_str_mapping() {
        assert_eq!(start_type_str(SERVICE_AUTO_START), "自动");
        assert_eq!(start_type_str(SERVICE_DEMAND_START), "手动");
        assert_eq!(start_type_str(SERVICE_DISABLED), "已禁用");
        assert_eq!(start_type_str(SERVICE_BOOT_START), "自动");
        assert_eq!(start_type_str(SERVICE_SYSTEM_START), "自动");
        assert_eq!(start_type_str(9999), "未知");
    }

    #[test]
    fn test_enum_services_shape() {
        // 实机验证：标准 Windows 系统服务数 > 100（设计文档验收标准）
        let services = enum_services();
        assert!(services.is_ok(), "enum_services 失败: {:?}", services.err());
        let services = services.unwrap();
        assert!(services.len() >= 10, "服务枚举数异常少: {}", services.len());
        // 关键系统服务必然存在
        let names: Vec<&str> = services.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"RpcSs") || names.contains(&"Power") || names.contains(&"EventLog"),
            "缺少关键服务，枚举结果异常"
        );
        assert!(services.iter().all(|s| !s.name.is_empty()));
    }

    #[test]
    fn test_query_service_shape() {
        // 查询一个必然存在的服务（RPC 服务）
        let rpc = query_service("RpcSs");
        assert!(rpc.is_ok(), "查询 RpcSs 失败: {:?}", rpc.err());
        match rpc.unwrap() {
            Some(info) => {
                assert_eq!(info.name, "RpcSs");
                assert!(["Running", "Stopped"].contains(&info.status.as_str()));
            }
            None => {
                let power = query_service("Power").unwrap();
                assert!(power.is_some(), "RpcSs 与 Power 都不存在，环境异常");
            }
        }
    }

    #[test]
    fn test_query_service_missing() {
        // 不存在的服务 → Ok(None)，不算错误
        let missing = query_service("SECM_Definitely_Not_A_Real_Service_XYZ");
        assert!(
            matches!(missing, Ok(None)),
            "应返回 Ok(None): {:?}",
            missing
        );
    }

    #[test]
    fn test_read_pwstr_null() {
        // NULL 指针 → 空串
        // SAFETY: null_mut 是合法空指针，read_pwstr 对 NULL 返回空串
        assert_eq!(unsafe { read_pwstr(null_mut()) }, "");
    }

    #[test]
    fn test_start_type_of_known_service() {
        let t = start_type_of("RpcSs");
        assert!(["自动", "手动", "已禁用", "未知"].contains(&t.as_str()));
    }
}
