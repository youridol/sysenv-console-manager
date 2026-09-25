//! 回归护栏：启动失败必须显式失败，不得让进程以无界面状态静默残留。
//!
//! ## 守护的 BUG
//!
//! v1.21.1 修复：双击 `sysenv-console-manager.exe` 时主窗口未创建，进程仍继续
//! 运行并持有单实例互斥体 `com.secm.app-sim`；用户再次双击被单实例插件拦截，
//! 以退出码 0 在二十余毫秒内结束，表现为「打开就闪退」（详见 CHANGELOG）。
//!
//! 根因有两层，本文件分别固化：
//!
//! 1. **Tauri 的窗口创建早于用户 setup 钩子且失败不返回错误**
//!    （tauri-2.11.5/src/app.rs:2524-2532：先 `WebviewWindowBuilder::from_config
//!    (...)?.build()?` 再调用 `(setup)(app)`）。因此「主窗口缺失」这件事必须在
//!    应用侧显式检测，不能依赖 Tauri 报错。
//! 2. **单实例回调在取不到主窗口时静默返回**，用户得不到任何反馈。
//!
//! ## 断言范围
//!
//! 全部基于源码文本，不启动 GUI、不需要 WebView2：
//!   A. 存在启动守卫模块，且它按「WebView2 子进程」判定启动是否成功；
//!   B. `setup` 钩子确实启动了守卫（接线未丢失）；
//!   C. 单实例回调改为调用守卫，不再出现「取不到窗口就什么都不做」的写法；
//!   D. 关闭到托盘不再用 `.expect("main window not found")`，避免 panic 中断
//!      事件循环并跳过守卫的提示与退出流程；
//!   E. 守卫的失败路径包含退出码 1 与中文提示，避免用户看到无反馈的闪退。

use std::path::PathBuf;

/// 从 crate 目录向上找到仓库根（`src-tauri` 的父目录）。
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 应有父目录")
        .to_path_buf()
}

/// 读取 `src-tauri/src/` 下的源文件并返回文本。
fn read_src(relative: &str) -> String {
    let path = repo_root().join("src-tauri").join("src").join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("应能读取 {}: {e}", path.display()))
}

#[test]
fn 源码守卫_按msedgewebview2子进程判定启动成功() {
    let guard = read_src("startup_guard.rs");

    // 判据必须是「WebView2 子进程」：读取 Tauri 运行时期窗口状态要经消息通道
    // 同步等待，窗口不在运行时期注册表时会让等待线程永久阻塞。
    assert!(
        guard.contains("msedgewebview2.exe"),
        "启动守卫应以 msedgewebview2.exe 作为启动成功判据"
    );
    assert!(
        guard.contains("fn has_webview2_child"),
        "启动守卫应实现子进程判定函数"
    );
    assert!(
        guard.contains("pub fn spawn_startup_watchdog"),
        "启动守卫应导出看门狗启动入口"
    );
    assert!(
        guard.contains("pub fn focus_or_report_missing_main"),
        "启动守卫应导出单实例回调入口"
    );
    assert!(
        guard.contains("MAIN_WINDOW_LABEL"),
        "主窗口标签应集中定义，避免各处硬编码漂移"
    );
}

#[test]
fn 源码守卫_启动失败先提示再以退出码1结束进程() {
    let guard = read_src("startup_guard.rs");

    // 必须退出：否则进程继续持有单实例互斥体，用户重试只会得到静默退出。
    assert!(
        guard.contains("std::process::exit(1)"),
        "启动失败必须以退出码 1 结束进程，释放单实例互斥体"
    );
    // 退出前必须有中文提示，否则用户仍然只看到无反馈的闪退。
    assert!(
        guard.contains("主窗口未能创建"),
        "启动失败应给出中文排查提示"
    );
    // 提示不能阻塞 main 线程上的 setup 钩子，应是异步投递。
    assert!(
        !guard.contains("blocking_show"),
        "启动失败提示不得使用阻塞式对话框（setup 钩子运行在主线程）"
    );
}

#[test]
fn 源码守卫_setup钩子已接线启动守卫() {
    let lib = read_src("lib.rs");

    assert!(
        lib.contains("pub mod startup_guard;"),
        "lib.rs 应声明 startup_guard 模块"
    );
    assert!(
        lib.contains("startup_guard::spawn_startup_watchdog"),
        "setup 钩子应启动守卫看门狗"
    );
    assert!(
        lib.contains("startup_guard::focus_or_report_missing_main"),
        "单实例回调应改为调用守卫，而不是静默返回"
    );
}

#[test]
fn 源码守卫_单实例回调不再静默忽略缺失的主窗口() {
    let lib = read_src("lib.rs");

    // 旧写法：`if let Some(window) = app.get_webview_window("main") { ... }`
    // 取不到窗口时整个回调什么都不做，用户双击后得不到任何反馈。
    let silent_pattern = "if let Some(window) = app.get_webview_window(\"main\")";
    assert!(
        !lib.contains(silent_pattern),
        "单实例回调不应保留「取不到主窗口即静默返回」的写法"
    );
    assert!(
        lib.contains("startup_guard::focus_or_report_missing_main(app)"),
        "单实例回调应委托给启动守卫，由守卫给出明确反馈"
    );
}

#[test]
fn 源码守卫_关闭到托盘不再因主窗口缺失而panic() {
    let lib = read_src("lib.rs");

    // panic 会中断事件循环，跳过启动守卫的提示与退出流程，使进程变成僵尸。
    assert!(
        !lib.contains(".expect(\"main window not found\")"),
        "关闭到托盘不应使用 expect，主窗口缺失时应交由启动守卫处置"
    );
    assert!(
        lib.contains("关闭到托盘未注册"),
        "主窗口缺失时应记录关闭到托盘未注册，而不是 panic"
    );
}

#[test]
fn 源码守卫_主窗口标签与守卫常量一致() {
    let config_path = repo_root().join("src-tauri").join("tauri.conf.json");
    let config = std::fs::read_to_string(&config_path)
        .unwrap_or_else(|e| panic!("应能读取 {}: {e}", config_path.display()));

    // 配置未显式写 label 时，Tauri 的默认值为 "main"（tauri-utils-2.9.3/src/
    // config.rs:2362）。守卫常量必须与之一致，否则永远判定主窗口缺失。
    let guard = read_src("startup_guard.rs");
    if !config.contains("\"label\"") {
        assert!(
            guard.contains("MAIN_WINDOW_LABEL: &str = \"main\""),
            "配置未声明 label 时，守卫常量必须是 Tauri 的默认值 main"
        );
    }
}
