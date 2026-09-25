//! 启动守卫 — 主窗口未真正就绪时给出可见反馈并释放单实例互斥体。
//!
//! 背景与实测事实（2026-09-23，release 构建 sysenv-console-manager.exe）：
//!
//! Tauri 在 `RuntimeRunEvent::Ready` 分支里的执行顺序是「先按 tauri.conf.json
//! 创建窗口，再调用用户 setup 钩子」（tauri-2.11.5/src/app.rs:2524-2532）。窗口
//! 创建失败只写一条日志，不返回错误，因此主窗口不可用时 setup 钩子仍会执行完，
//! 进程继续存活。
//!
//! 实测失败态：进程存活，顶层窗口只有隐藏窗口 `com.secm.app-siw`（单实例插件的
//! 消息窗口）与托盘窗口，没有 `Tauri Window` 类主窗口，没有 msedgewebview2.exe
//! 子进程。此时进程持有单实例互斥体 `com.secm.app-sim`，用户再次双击时单实例插件
//! 发送 `WM_COPYDATA` 后调用 `std::process::exit(0)`
//! （tauri-plugin-single-instance-2.4.3/src/platform_impl/windows.rs:72-95），而本
//! 应用的单实例回调在取不到可用主窗口时静默返回。实测第二次启动 23 毫秒退出、
//! 退出码 0，第三次 22 毫秒退出、退出码 0，即用户看到的「打开就闪退」。
//!
//! 实测判别条件（同一二进制、同一环境）：
//!
//! | 状态 | `Tauri Window` 类主窗口 | msedgewebview2.exe 子进程 |
//! | --- | --- | --- |
//! | 正常 | 存在（1400×800） | 1 |
//! | 失败 | 不存在 | 0 |
//!
//! WebView2 环境创建失败的原始错误为
//! `WindowsError(HRESULT(0x800700AA))`，即 ERROR_BUSY「请求的资源在使用中」。
//!
//! 本模块做两件事：
//! 1. `spawn_startup_watchdog`：在启动窗口期内确认 WebView2 子进程已出现。只有
//!    这一项判据，因为它与实测判别条件一一对应，且不依赖 Tauri 运行时期状态
//!    （读取窗口状态要经消息通道同步等待，窗口不在运行时期注册表时会让等待线程
//!    永久阻塞）。超时后提示用户、结束进程，释放单实例互斥体。
//! 2. `focus_or_report_missing_main`：单实例回调不再静默返回。
//!
//! 时序约束：看门狗线程只做「轮询 + 提示 + 退出」，提示投递后留出固定时间让
//! 对话框显示，再以退出码 1 结束进程。退出必须由本线程直接完成，不能依赖主线程
//! 事件循环——启动失败路径上主线程状态不可假定。
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

/// 主窗口标签，与 tauri.conf.json 的 windows 数组一致（未写 `label` 时代码默认值）。
pub const MAIN_WINDOW_LABEL: &str = "main";

/// 等待 WebView2 子进程出现的上限。正常启动实测在数秒内完成握手，这里取 25 秒
/// 以覆盖磁盘繁忙与安全软件首次扫描的机器。
const STARTUP_TIMEOUT: Duration = Duration::from_secs(25);

/// 启动期轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// 提示投递后等待用户阅读的时间，随后结束进程。
const DIALOG_GRACE: Duration = Duration::from_secs(20);

/// WebView2 浏览器进程文件名（小写后缀比较）。
const WEBVIEW2_PROCESS_NAME: &str = "msedgewebview2.exe";

/// 启动失败时给用户的中文排查提示。
fn missing_main_window_message() -> String {
    format!(
        "SysEnv Console Manager 的主窗口未能创建，程序无法继续运行。\n\n\
可以依次检查：\n\
1. 是否已安装 Microsoft Edge WebView2 Runtime（Windows 设置 → 应用）；\n\
2. 任务管理器中是否存在残留的 msedgewebview2.exe 进程，结束后重试；\n\
3. 诊断日志 {} 中是否记录 failed to create webview。",
        crate::runtime_paths::diag_dir().join("debug.log").display()
    )
}

/// 已有实例不可用时给用户的提示。
const EXISTING_INSTANCE_MESSAGE: &str =
    "SysEnv Console Manager 已有一个实例在运行，但该实例的主窗口不可用。\n\n\
请在任务管理器中结束所有 sysenv-console-manager.exe 与 msedgewebview2.exe 进程，然后重新打开程序。";

/// 错误提示标题。
const ERROR_TITLE: &str = "SysEnv Console Manager 启动失败";

/// 后台确认 WebView2 已起来；始终未起则提示用户并结束进程。
///
/// 结束进程是必需动作：失败态下进程会一直持有单实例互斥体，用户后续双击只会得到
/// 二十余毫秒的静默退出。释放互斥体后，用户重试才可能真正启动成功。
///
/// 本线程不阻塞主线程：setup 钩子必须返回，事件循环才能继续运行并显示对话框。
pub fn spawn_startup_watchdog<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    let spawned = std::thread::Builder::new()
        .name("startup-watchdog".to_string())
        .spawn(move || {
            let deadline = Instant::now() + STARTUP_TIMEOUT;
            let own_pid = std::process::id();

            while Instant::now() < deadline {
                if has_webview2_child(own_pid) {
                    crate::debug_info!("startup", "启动守卫：WebView2 已就绪");
                    return;
                }
                std::thread::sleep(POLL_INTERVAL);
            }

            crate::debug_error!(
                "startup",
                "启动守卫：{} 秒内未观察到 WebView2 子进程，判定主窗口创建失败，提示用户后以退出码 1 结束（释放单实例互斥体）",
                STARTUP_TIMEOUT.as_secs()
            );

            handle
                .dialog()
                .message(missing_main_window_message())
                .title(ERROR_TITLE)
                .kind(MessageDialogKind::Error)
                .show(|_| {});

            // 留出时间让提示显示；随后由本线程直接结束进程，不依赖事件循环。
            std::thread::sleep(DIALOG_GRACE);
            std::process::exit(1);
        });

    if let Err(e) = spawned {
        crate::debug_warn!("startup", "启动守卫线程创建失败（不影响启动）: {e}");
    }
}

/// 单实例回调：已有实例的主窗口不可用时给出明确反馈，不再静默返回。
///
/// 返回 `true` 表示已定位并聚焦主窗口；返回 `false` 表示已有实例没有注册主窗口，
/// 此时本函数已提示用户已有实例不可用。
///
/// 主窗口存在但当前被隐藏（关闭到托盘）属于正常状态，直接显示并聚焦。
pub fn focus_or_report_missing_main<R: Runtime>(app: &AppHandle<R>) -> bool {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return true;
    }

    crate::debug_error!(
        "startup",
        "单实例回调未找到主窗口 {}，已有实例可能处于启动失败状态",
        MAIN_WINDOW_LABEL
    );

    // 本回调运行在主线程的 WM_COPYDATA 处理内，投递提示即可；已有实例继续运行，
    // 因此这里不退出进程。
    app.dialog()
        .message(EXISTING_INSTANCE_MESSAGE)
        .title(ERROR_TITLE)
        .kind(MessageDialogKind::Error)
        .show(|_| {});

    false
}

/// 判定当前进程是否已有 WebView2 浏览器子进程。
///
/// 只认自己的直接子进程，避免把其它应用（例如系统小组件、启动器）的 WebView2
/// 进程算作成功。
fn has_webview2_child(own_pid: u32) -> bool {
    use sysinfo::{Pid, System};

    let mut system = System::new();
    system.refresh_processes();

    system.processes().values().any(|process| {
        process.parent() == Some(Pid::from_u32(own_pid))
            && process
                .name()
                .to_ascii_lowercase()
                .ends_with(WEBVIEW2_PROCESS_NAME)
    })
}
