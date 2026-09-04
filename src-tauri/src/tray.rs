/// System tray module for SECM.
///
/// Provides:
/// - Right-click menu: "Show Window" / "Exit"
/// - Double-click to show window
/// - Close-to-tray behavior
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime,
};

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

    Ok(tray)
}
