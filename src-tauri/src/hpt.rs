//! 高精度计时器（HPET）关闭/恢复模块
//!
//! 关闭高精度计时器（降低系统计时器中断频率，减少 CPU 占用，需重启生效）：
//! ```text
//! bcdedit /set useplatformclock no
//! bcdedit /set useplatformtick no
//! bcdedit /set disabledynamictick yes
//! ```
//! 恢复默认：
//! ```text
//! bcdedit /deletevalue useplatformclock
//! bcdedit /deletevalue useplatformtick
//! bcdedit /deletevalue disabledynamictick
//! ```
//!
//! 状态判定：三个条目均存在且为
//! `useplatformclock=no + useplatformtick=no + disabledynamictick=yes` 视为已关闭。
//! 注意：bcdedit 需要管理员权限；输出值为本地化文本（是/否、Yes/No、true/false），
//! 解析时兼容多种词表。

use crate::settings::SettingState;

/// 关闭高精度计时器的 bcdedit 参数序列（enabled=true 时执行）
const CMD_DISABLE: &[&[&str]] = &[
    &["/set", "useplatformclock", "no"],
    &["/set", "useplatformtick", "no"],
    &["/set", "disabledynamictick", "yes"],
];

/// 恢复默认的 bcdedit 参数序列（enabled=false 时执行）
const CMD_RESTORE: &[&[&str]] = &[
    &["/deletevalue", "useplatformclock"],
    &["/deletevalue", "useplatformtick"],
    &["/deletevalue", "disabledynamictick"],
];

/// 执行单条 bcdedit 命令；失败返回含命令、退出码与输出摘要的错误
fn run_bcdedit(args: &[&str]) -> Result<(), String> {
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW：避免 GUI 应用 spawn bcdedit.exe 时弹出黑色控制台窗口
    let output = std::process::Command::new("bcdedit")
        .args(args)
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| {
            format!(
                "启动 bcdedit 失败: {}（bcdedit 需管理员权限，请以管理员身份运行本应用）",
                e
            )
        })?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "bcdedit {} 执行失败 (exit={}): {} {}",
            args.join(" "),
            output.status.code().unwrap_or(-1),
            stdout.trim(),
            stderr.trim()
        ));
    }
    Ok(())
}

/// 解析 bcdedit 输出行的布尔值（兼容本地化词表）
/// 形如 `useplatformclock        否` / `useplatformclock          No`
fn parse_bcd_bool(line: &str, key: &str) -> Option<bool> {
    let rest = line.strip_prefix(key)?;
    let val = rest.trim_start().split_whitespace().next()?;
    match val.to_lowercase().as_str() {
        "yes" | "true" | "1" | "是" => Some(true),
        "no" | "false" | "0" | "否" => Some(false),
        _ => None,
    }
}

/// 读取三个计时器条目的当前值
/// 返回 (useplatformclock, useplatformtick, disabledynamictick)，None = 条目不存在
fn read_current() -> (Option<bool>, Option<bool>, Option<bool>) {
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("bcdedit")
        .args(["/enum", "{current}"])
        .creation_flags(0x08000000)
        .output();
    let Ok(output) = output else {
        return (None, None, None);
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut clock: Option<bool> = None;
    let mut tick: Option<bool> = None;
    let mut dyn_tick: Option<bool> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = parse_bcd_bool(line, "useplatformclock") {
            clock = Some(v);
        } else if let Some(v) = parse_bcd_bool(line, "useplatformtick") {
            tick = Some(v);
        } else if let Some(v) = parse_bcd_bool(line, "disabledynamictick") {
            dyn_tick = Some(v);
        }
    }
    (clock, tick, dyn_tick)
}

/// 读取高精度计时器关闭状态（enabled = 已关闭）
pub fn get_hpt_state() -> SettingState {
    let (clock, tick, dyn_tick) = read_current();
    let enabled =
        clock == Some(false) && tick == Some(false) && dyn_tick == Some(true);
    SettingState {
        name: "hpt".into(),
        enabled,
        admin_required: true,
        message: if enabled {
            "高精度计时器已关闭（重启后生效）".into()
        } else {
            "使用系统默认计时器".into()
        },
    }
}

/// 设置高精度计时器：enabled=true 关闭（写入三条 /set），false 恢复默认（三条 /deletevalue）
pub fn set_hpt_state(enabled: bool) -> Result<SettingState, String> {
    let cmds = if enabled { CMD_DISABLE } else { CMD_RESTORE };
    for args in cmds {
        run_bcdedit(args)?;
    }
    // 重新读取确认（bcdedit 同步写入，重读即最新状态）
    Ok(get_hpt_state())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 布尔值解析兼容中英文词表() {
        assert_eq!(parse_bcd_bool("useplatformclock          No", "useplatformclock"), Some(false));
        assert_eq!(parse_bcd_bool("useplatformclock        否", "useplatformclock"), Some(false));
        assert_eq!(parse_bcd_bool("useplatformclock         Yes", "useplatformclock"), Some(true));
        assert_eq!(parse_bcd_bool("useplatformclock        是", "useplatformclock"), Some(true));
        assert_eq!(parse_bcd_bool("useplatformtick          false", "useplatformtick"), Some(false));
        assert_eq!(parse_bcd_bool("disabledynamictick       true", "disabledynamictick"), Some(true));
        // 不相关行/不完整行返回 None
        assert_eq!(parse_bcd_bool("path                    partition=C:", "useplatformclock"), None);
        assert_eq!(parse_bcd_bool("useplatformclock", "useplatformclock"), None);
        // 前缀不误匹配（如其他条目名）
        assert_eq!(parse_bcd_bool("useplatformclockx         Yes", "useplatformclock"), None);
    }

    #[test]
    fn 关闭状态判定组合() {
        // 三条齐全且正确 → 已关闭
        let (c, t, d) = (Some(false), Some(false), Some(true));
        assert!(c == Some(false) && t == Some(false) && d == Some(true));
        // 部分缺失 → 未关闭
        let (c2, t2, d2) = (Some(false), None, None);
        assert!(!(c2 == Some(false) && t2 == Some(false) && d2 == Some(true)));
        // useplatformclock=yes（开启高精度时钟）→ 未关闭
        let (c3, t3, d3) = (Some(true), Some(false), Some(true));
        assert!(!(c3 == Some(false) && t3 == Some(false) && d3 == Some(true)));
    }
}
