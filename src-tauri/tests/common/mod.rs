//! 集成测试共享辅助：测试产物落点。
//!
//! 背景（2026-09-23 实测）：本仓库工作区目录带 `Mandatory Label\Low Mandatory Level`
//! 标签，从该目录启动的测试进程即低完整性进程，**无法在系统 `%TEMP%` 下创建目录**
//! （`create_dir_all` 返回 OS 错误 5「拒绝访问」），因此原先用 `std::env::temp_dir()`
//! 建夹具的用例会因环境原因失败，掩盖被测代码的真实结论。
//!
//! 解决：夹具根目录改到项目内 `build/tmp/fixtures/`（进程可写，且随 `build/` 整体清理）。
//! 该目录与 `scripts/test-lib.sh` 注入的 `TEMP`/`TMP`/`SECM_RUNTIME_DIR` 属同一隔离区，
//! 但**不依赖**测试被脚本启动——直接 `cargo test --test <名>` 时同样生效。
#![allow(dead_code)]

use std::path::PathBuf;

/// 测试夹具根目录：`<项目根>/build/tmp/fixtures`。
///
/// `src-tauri/tests/*.rs` 的编译单元位于 `<项目根>/src-tauri`，因此自
/// `CARGO_MANIFEST_DIR` 上溯一级即项目根。
pub fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 应有上级目录（项目根）")
        .join("build")
        .join("tmp")
        .join("fixtures")
}

/// 取（并按需创建）一个具名夹具目录，已存在时先清空。
///
/// `name` 需在同一测试目标内唯一，避免并行用例互相干扰。
pub fn fixture_dir(name: &str) -> PathBuf {
    let dir = fixtures_root().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| {
        panic!(
            "创建测试夹具目录失败: {} ({})。该目录位于项目内 build/tmp/fixtures，应始终可写；\
             若失败请检查项目目录权限。",
            dir.display(),
            e
        )
    });
    dir
}
