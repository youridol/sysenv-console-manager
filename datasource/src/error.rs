//! 采集错误模型（thiserror）— 驱动内部传播用，命令层映射为 `String`
//!
//! 错误消息规范（R8）：含 API 名（`api`）、操作名（`op`）、错误详情（`detail`）。
//! 用户可见消息格式沿用团队规范 `"中文操作描述: 技术细节"`，由上层命令层组装。

/// 采集错误（驱动层统一错误类型）
#[derive(Debug, thiserror::Error)]
pub enum CollectError {
    /// Win32 API 调用失败（含 WMI/COM 封装失败）
    #[error("[{api}] {op} 失败: {detail}")]
    WinApi {
        api: &'static str,
        op: String,
        detail: String,
    },

    /// 注册表读取失败
    #[error("注册表读取失败: {path} | {detail}")]
    Registry { path: String, detail: String },

    /// HTTP 请求失败
    #[error("HTTP 请求失败: {url} | {detail}")]
    Http { url: String, detail: String },

    /// 数据解析失败
    #[error("数据解析失败: {what} | {detail}")]
    Parse { what: String, detail: String },

    /// 需要管理员权限
    #[error("需要管理员权限: {op}")]
    NeedsAdmin { op: String },

    /// 数据不存在（系统无此数据，非错误场景，用于 `Ok(None)` 语义的内部桥梁）
    #[error("数据不存在: {what}")]
    NotFound { what: String },
}

impl CollectError {
    /// 构造 WinApi 错误：自动附带 `GetLastError` 错误码
    pub fn winapi(api: &'static str, op: impl Into<String>) -> Self {
        CollectError::WinApi {
            api,
            op: op.into(),
            detail: format!("错误码 {}", last_error_code()),
        }
    }

    /// 构造 WinApi 错误（带附加说明）
    pub fn winapi_detailed(
        api: &'static str,
        op: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        CollectError::WinApi {
            api,
            op: op.into(),
            detail: detail.into(),
        }
    }

    /// 构造注册表错误
    pub fn registry(path: impl Into<String>, detail: impl Into<String>) -> Self {
        CollectError::Registry {
            path: path.into(),
            detail: detail.into(),
        }
    }

    /// 构造 HTTP 错误
    pub fn http(url: impl Into<String>, detail: impl Into<String>) -> Self {
        CollectError::Http {
            url: url.into(),
            detail: detail.into(),
        }
    }

    /// 构造解析错误
    pub fn parse(what: impl Into<String>, detail: impl Into<String>) -> Self {
        CollectError::Parse {
            what: what.into(),
            detail: detail.into(),
        }
    }
}

/// 读取 GetLastError 错误码（Windows）
#[cfg(windows)]
fn last_error_code() -> u32 {
    // SAFETY: GetLastError 是无参标准导出，线程局部存储，无副作用
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

/// 非 Windows 平台占位（编译期兼容，实际不会执行）
#[cfg(not(windows))]
fn last_error_code() -> u32 {
    0
}
