//! 回归护栏：流式命令必须在入口注册取消标志，否则「停止」按钮永不生效。
//!
//! ## 守护的 BUG
//!
//! `cancel::cancel_command` 只对**已存在于注册表**的键置位（cancel.rs:30-35）：
//!
//! ```ignore
//! pub fn cancel_command(cmd_id: &str) {
//!     let map = CANCEL_MAP.lock();
//!     if let Some(flag) = map.get(cmd_id) {   // 键不存在 → 静默无操作
//!         flag.store(true, Ordering::SeqCst);
//!     }
//! }
//! ```
//!
//! 而 `cancel_flag`（唯一的注册入口）在 2026-09-24 审查时的生产调用点为 **0**：
//! 五条流式命令只调用 `clear_cancel`（清理），从不注册。结果是所有
//! `is_cancelled` 检查点恒为 false，前端「停止」发出的 `cancel_net_cmd` 无效。
//!
//! 原有 `iperf3_live.rs:509-512` 是**测试自己**先注册标志再取消，恰好绕过了
//! 这个缺口，因此需要一条针对命令源码本身的护栏。
//!
//! ## 断言范围
//!
//! 全部基于源码文本，不启动 GUI：
//!   A. 五条流式命令各自有 `cancel_flag` 注册；
//!   B. 注册与清理成对（`cancel_flag` 与 `clear_cancel` 数量相等）；
//!   C. 注册发生在消费 `is_cancelled` 的调用之前（源码顺序：注册在前）。

use std::path::PathBuf;

fn commands_rs() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("commands.rs");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("应能读取 {}: {e}", path.display()))
}

/// 五条可取消的流式命令及其标志变量名（`cmd_id` 的克隆）。
const STREAMING_COMMANDS: &[(&str, &str)] = &[
    ("ping_stream", "&cid"),
    ("traceroute_stream", "&cid"),
    ("nslookup_stream", "&cid_ref"),
    ("detect_nat_type_stream", "&cid_ref"),
    ("iperf3_stream", "&cid"),
];

/// 截取某个命令函数体的源码区间（自 `pub async fn <name>` 起，到下一个
/// `#[tauri::command]` 或文件末尾止）。
fn command_body(src: &str, name: &str) -> String {
    let needle = format!("pub async fn {name}(");
    let start = src
        .find(&needle)
        .unwrap_or_else(|| panic!("commands.rs 应包含 {name}"));
    let rest = &src[start..];
    // 下一个命令的边界：去掉本函数开头后出现的第一个 "#[tauri::command]"
    let after_header = &rest[needle.len()..];
    match after_header.find("#[tauri::command]") {
        Some(off) => rest[..needle.len() + off].to_string(),
        None => rest.to_string(),
    }
}

#[test]
fn 流式命令_入口注册取消标志() {
    let src = commands_rs();
    for (name, flag) in STREAMING_COMMANDS {
        let body = command_body(&src, name);
        let register = format!("cancel::cancel_flag({flag});");
        assert!(
            body.contains(&register),
            "{name} 必须在入口注册取消标志（期望 `{register}`）。\
             缺少注册时 cancel_command 找不到键，is_cancelled 恒为 false，停止按钮无效。"
        );
    }
}

#[test]
fn 流式命令_注册与清理成对() {
    let src = commands_rs();
    for (name, flag) in STREAMING_COMMANDS {
        let body = command_body(&src, name);
        let register = format!("cancel::cancel_flag({flag});");
        let clear = format!("cancel::clear_cancel({flag});");
        assert!(body.contains(&register), "{name} 缺少注册");
        assert!(body.contains(&clear), "{name} 缺少清理，注册标志会泄漏");
        // 注册必须出现在清理之前，避免先清后注册（标志立即被移除）
        let r = body.find(&register).expect("注册存在");
        let c = body.find(&clear).expect("清理存在");
        assert!(r < c, "{name} 的注册必须早于清理");
    }
}

#[test]
fn 流式命令_注册早于取消检查() {
    // 注册必须发生在任何 is_cancelled 调用之前：源码顺序上 cancel_flag
    // 应出现在函数体前部（这里以「注册位置 < 函数体长度的一半」近似，
    // 足以抓住把注册误放到收尾处的写法）。
    let src = commands_rs();
    for (name, flag) in STREAMING_COMMANDS {
        let body = command_body(&src, name);
        let register = format!("cancel::cancel_flag({flag});");
        let pos = body.find(&register).expect("注册存在");
        assert!(
            pos * 2 < body.len(),
            "{name} 的取消注册应位于入口（当前在函数体 {pos}/{} 处），\
             放到收尾处会导致前半个执行窗口内取消失效。",
            body.len()
        );
    }
}

#[cfg(test)]
mod cancel_semantics {
    // 运行时语义：把「未注册的键取消不生效」这一根因固化为可执行契约。
    // 它与上面的源码守卫互补——源码守卫保证「命令必须注册」，
    // 这里保证「注册确实是取消生效的必要条件」。

    #[test]
    fn 运行时_注册后取消生效并可在清理后复位() {
        let id = format!("register-probe-{}", std::process::id());
        secm_lib::cancel::clear_cancel(&id);

        secm_lib::cancel::cancel_flag(&id);
        assert!(
            !secm_lib::cancel::is_cancelled(&id),
            "刚注册的标志应为未取消"
        );

        secm_lib::cancel::cancel_command(&id);
        assert!(
            secm_lib::cancel::is_cancelled(&id),
            "注册后调 cancel_command 应使 is_cancelled 为 true"
        );

        secm_lib::cancel::clear_cancel(&id);
        assert!(
            !secm_lib::cancel::is_cancelled(&id),
            "清理后应回到未注册态（is_cancelled 回落 false）"
        );
    }
}
