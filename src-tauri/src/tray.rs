/// System tray module for SECM.
///
/// Provides:
/// - Right-click menu: "Show Window" / "Exit"
/// - Double-click to show window
/// - Close-to-tray behavior
///
/// 注册结果校验（2026-09-23）：
/// `tauri::tray::TrayIconBuilder::build` 的成功只代表隐藏窗口创建成功。底层的
/// `Shell_NotifyIconW(NIM_ADD)` 由 `tray-icon` 调用，其返回值在该 crate 内部即被
/// 丢弃（`tray-icon-0.24.x/src/platform_impl/windows/mod.rs` 的
/// `register_tray_icon` 末行 `== TRUE` 的结果不参与错误传播）。因此「窗口建好但
/// 图标没进通知区域」时 `build` 仍返回 `Ok`，调用方会误记一条成功日志。
///
/// 本模块用 `TrayIcon::rect()` 做落地校验：它的 Windows 实现走
/// `Shell_NotifyIconGetRect`，只有外壳真的登记了该图标才会返回矩形。校验失败时
/// 重试 `set_visible(true)` 触发重新登记，仍失败则返回错误交由上层记录。
use std::time::Duration;

use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime,
};

/// 注册校验的重试次数与间隔：外壳在登录初期可能尚未就绪，留出短暂重试窗口。
const VERIFY_ATTEMPTS: u32 = 10;
const VERIFY_INTERVAL: Duration = Duration::from_millis(200);

/// Create and configure the system tray icon.
pub fn create_tray<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<tauri::tray::TrayIcon<R>, tauri::Error> {
    // Build the tray menu
    let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let exit_item = MenuItem::with_id(app, "exit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &exit_item])?;

    // Embed icon PNG bytes at compile time so we don't depend on runtime CWD
    let icon_bytes = include_bytes!("../icons/icon.png");
    let icon = Image::from_bytes(icon_bytes)?;

    let tray = TrayIconBuilder::new()
        .icon(icon)
        .menu(&menu)
        .tooltip("SysEnv Console Manager")
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "exit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
        .build(app)?;

    // 校验失败**不返回 Err**：`Err` 会让本函数返回的 `TrayIcon` 被丢弃，而
    // `TrayIcon` 的 `Drop` 会注销图标——那样反而把 `tray-icon` 内建的恢复路径
    // （收到 `TaskbarCreated` 广播后自动重新登记）一并放弃，比不校验更糟。
    // 因此这里只如实记录状态，托盘对象继续由调用方持有，等待外壳就绪后自动补登记。
    if !verify_registered(&tray) {
        crate::debug_error!(
            "tray",
            "托盘图标未登记进通知区域（{} 次校验均未取到图标矩形）；\
             已保留托盘对象等待 explorer 重建任务栏（TaskbarCreated）时自动补登记。\
             若始终不出现，请检查 explorer 是否运行、以及第三方安全软件是否拦截 Shell_NotifyIcon",
            VERIFY_ATTEMPTS
        );
    }

    Ok(tray)
}

/// 确认图标真的登记进通知区域；未登记时重试可见性切换。
///
/// `TrayIcon::rect()` 经 `with_inner_tray_icon` 在主线程执行
/// `Shell_NotifyIconGetRect`。本函数由 `setup` 钩子调用，而 `setup` 本身就在主
/// 线程上运行（`tauri-runtime-wry` 的 `send_user_message` 对主线程调用做直接
/// 派发，不投递消息队列），因此不会自等死锁。
///
/// 返回 `bool` 而非 `Result`：调用方**必须**保留托盘对象（原因见 `create_tray`）。
fn verify_registered<R: Runtime>(tray: &tauri::tray::TrayIcon<R>) -> bool {
    for attempt in 1..=VERIFY_ATTEMPTS {
        match tray.with_inner_tray_icon(|inner| inner.rect()) {
            Ok(Some(_)) => {
                crate::debug_info!("tray", "托盘登记校验通过（第 {} 次）", attempt);
                return true;
            }
            Ok(None) => {
                // 未登记：切换可见性促使重新调用 Shell_NotifyIcon(NIM_ADD)
                let _ = tray.set_visible(false);
                let _ = tray.set_visible(true);
            }
            Err(e) => {
                crate::debug_warn!("tray", "托盘登记校验第 {} 次取值失败: {}", attempt, e);
            }
        }
        std::thread::sleep(VERIFY_INTERVAL);
    }
    false
}
