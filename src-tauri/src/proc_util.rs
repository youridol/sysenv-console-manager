//! 进程调用共享工具 — PowerShell / 外部程序执行（GUI 应用全程无窗口）
//!
//! 供需要调用系统命令的模块复用（当前为 `net_config` 的 netsh 查询）。

/// 执行 PowerShell 脚本，返回 stdout 文本；失败返回含 stderr 的中文可读错误（R8）
///
/// 错误不再静默吞掉——启动失败、非零退出码、cmdlet 报错（stderr/非空输出）
/// 均以 `Err(String)` 返回，消息含错误码与修复建议。
/// 统一 `CREATE_NO_WINDOW`（0x08000000，防止 GUI 应用弹出控制台黑窗）
/// 与 `[Console]::OutputEncoding = UTF-8`（无 BOM）：中文 Windows 控制台默认 GBK，
/// 直接重定向会让中文输出（如网卡别名"以太网"）在 `from_utf8_lossy` 下乱码。
pub(crate) fn run_ps_result(script: &str) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    let wrapped = format!("[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); {script}");
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &wrapped])
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| {
            format!(
                "PowerShell 启动失败（API: CreateProcess / powershell.exe）: {}（建议：检查系统 PowerShell 是否可用或被组策略禁用）",
                e
            )
        })?;
    if output.status.success() {
        Ok(decode_ps(&output.stdout).trim().to_string())
    } else {
        // cmdlet 错误流（stderr）可能不受 [Console]::OutputEncoding 控制而仍为 GBK
        // （中文系统），故统一走 UTF-8→GBK 兜底解码，避免错误消息乱码（R8）
        let stderr = decode_ps(&output.stderr).trim().to_string();
        let stdout = decode_ps(&output.stdout).trim().to_string();
        let code = output.status.code().unwrap_or(-1);
        // cmdlet 错误优先取 stderr；部分场景错误在 stdout（如命令语法回显）
        let msg = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            "(无输出)".to_string()
        };
        Err(format!(
            "PowerShell 执行失败（退出码 {}，API: powershell -NoProfile -Command）: {}（建议：确认命令与参数合法、具备管理员权限后重试）",
            code, msg
        ))
    }
}

/// 解码外部进程输出：优先 UTF-8，回退 GBK（中文系统的 ANSI 代码页）。
///
/// 这是本 crate **唯一**的「UTF-8 优先、GBK 兜底」解码实现（ADR-0001 D14）：
/// `net_config::decode_ansi` 曾是同构副本，已改为复用它。
pub(crate) fn decode_ps(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    let (cow, _, _) = encoding_rs::GBK.decode(bytes);
    cow.into_owned()
}
