/// 通用取消注册表 — 为长时间运行的命令提供取消支持。
///
/// 工作原理：
/// - 每个命令通过 `cancel_flag(id)` 获取一个 `Arc<AtomicBool>`
/// - 命令在执行循环中通过 `is_cancelled(id)` 检查是否应终止
/// - 前端调用 `cancel_command(id)` 设置取消标志
/// - 命令完成后调用 `clear_cancel(id)` 清理
///
/// 使用者：network 模块（ping / traceroute / nslookup / NAT / iperf3）
/// 以及任何需要可取消长时间操作的新命令。
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

lazy_static::lazy_static! {
    static ref CANCEL_MAP: Mutex<HashMap<String, Arc<AtomicBool>>> =
        Mutex::new(HashMap::new());
}

/// 获取或创建指定命令 ID 的取消标志
pub fn cancel_flag(cmd_id: &str) -> Arc<AtomicBool> {
    let mut map = CANCEL_MAP.lock();
    map.entry(cmd_id.to_string())
        .or_insert_with(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

/// 标记命令为已取消
pub fn cancel_command(cmd_id: &str) {
    let map = CANCEL_MAP.lock();
    if let Some(flag) = map.get(cmd_id) {
        flag.store(true, Ordering::SeqCst);
    }
}

/// 命令完成后清理取消标志
pub fn clear_cancel(cmd_id: &str) {
    CANCEL_MAP.lock().remove(cmd_id);
}

/// 检查命令是否已被取消
pub fn is_cancelled(cmd_id: &str) -> bool {
    CANCEL_MAP
        .lock()
        .get(cmd_id)
        .map(|f| f.load(Ordering::SeqCst))
        .unwrap_or(false)
}
