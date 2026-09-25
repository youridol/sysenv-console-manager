//! 数据源接入层工具 — 旧数据源（PowerShell/外部命令）与新驱动（secm-datasource）切换
//!
//! - 过渡期保留 `legacy-data-source` feature：启用后 P1-P13 回退旧 PowerShell/外部命令实现
//! - 错误显式化：新采集器返回 `Result<T, CollectError>`，本模块负责映射为
//!   用户可见中文消息（`"中文操作描述: 技术细节"`）并打 R8 日志（含 API 名/操作名/详情）
//!
//! 依赖方向：本模块只依赖 datasource crate（叶子），业务模块依赖本模块；禁止反向依赖。

use secm_datasource::CollectError;

/// 是否启用旧数据源（编译期 feature，运行时不可变）
///
/// 迁移完成后（M6）删除本函数与全部 `legacy` 分支。
#[inline]
pub fn use_legacy() -> bool {
    cfg!(feature = "legacy-data-source")
}

/// 将 CollectError 映射为用户可见消息（格式：`中文操作描述: 技术细节`）
///
/// 各采集点调用方在降级/失败路径使用，与现状占位文案（"无法获取"/"—"）组合。
pub fn err_message(e: &CollectError) -> String {
    match e {
        CollectError::WinApi { api, op, detail } => {
            format!("{}失败: [{}] {}", op, api, detail)
        }
        CollectError::Registry { path, detail } => {
            format!("注册表读取失败: {} | {}", path, detail)
        }
        CollectError::Http { url, detail } => {
            format!("网络请求失败: {} | {}", url, detail)
        }
        CollectError::Parse { what, detail } => {
            format!("数据解析失败: {} | {}", what, detail)
        }
        CollectError::NeedsAdmin { op } => format!("需要管理员权限: {}", op),
        CollectError::NotFound { what } => format!("数据不存在: {}", what),
    }
}

/// 错误日志 + 返回用户可见消息（R8：错误含操作名/API 名/详情/建议修复）
///
/// 用法：`Err(ds::CollectError) => return Err(ds_util::log_err("net_stats", "获取链路速度", &e))`
pub fn log_err(module: &str, op: &str, e: &CollectError) -> String {
    let msg = err_message(e);
    crate::debug_error!(module, "{} | 建议: 检查系统服务与权限后重试 | {}", op, msg);
    msg
}
