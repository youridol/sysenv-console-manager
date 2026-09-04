/// SysEnv Console Manager - Main library entry point

///

/// Module structure:

/// - log: Thread-safe ring buffer logging (memory + file)

/// - tray: System tray icon and menu

/// - commands: Tauri command handlers

pub mod cancel;

pub mod cleanup;

pub mod commands;

pub mod debug;

pub mod dhcp_probe;

pub mod disk_info;

pub mod ds_util;

pub mod driver_install;

pub mod environment;

pub mod game_env;

pub mod hardware;

pub mod hpt;

pub mod ip_info;

pub mod lhm;

pub mod log;

pub mod net_stats;

pub mod net_config;

pub mod network;

pub mod nvidia_drs;

pub mod proc_util;

pub mod sensor;

pub mod settings;

pub mod sysinfo;

pub mod temp_data;

pub mod tray;

// 纯 Rust 驱动采集模块（独立 crate secm-datasource，见 datasource/）
// 业务模块经此引用：use secm_lib::datasource::{service, power, ...};
pub use secm_datasource as datasource;



use std::sync::Arc;

use tauri::Manager;



/// Application state shared across all handlers

pub struct AppState {

    pub log_buffer: Arc<log::LogBuffer>,

}



impl AppState {

    pub fn new() -> Self {

        Self {

            log_buffer: Arc::new(log::LogBuffer::new()),

        }

    }

}



#[cfg_attr(mobile, tauri::mobile_entry_point)]

pub fn run() {

    let app_state = AppState::new();



    // 注册 log crate 桥接（datasource 的 log::* → SECM debug 体系，统一 debug.log）
    crate::debug::init_log_bridge();

    // 初始化文件日志和诊断目录

    if let Err(e) = log::init_file_logger() {

        eprintln!(

            "Warning: Failed to initialize file logger: {}. Logging to memory only.",

            e

        );

    }

    // 启动时清理过期诊断文件 + 强制总大小检查

    temp_data::init_temp_data();



    tauri::Builder::default()

        // 单实例机制：防止多个实例同时占用 WebView2 user-data-dir 导致黑屏/无法加载

        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {

            // 聚焦已有实例的主窗口

            if let Some(window) = app.get_webview_window("main") {

                let _ = window.show();

                let _ = window.set_focus();

            }

        }))

        .plugin(tauri_plugin_store::Builder::new().build())

        .plugin(tauri_plugin_dialog::init())

        .plugin(tauri_plugin_shell::init())

        .plugin(tauri_plugin_fs::init())

        .manage(app_state)

        .setup(move |app| {

            debug_info!(

                "startup",

                "SECM starting... version={}",

                env!("CARGO_PKG_VERSION")

            );



            // Store app handle for event emission (log_buffer uses std::sync::Mutex, no async needed)

            let handle = app.handle().clone();

            let state = app.state::<AppState>();

            state.log_buffer.set_app_handle(handle);

            state

                .log_buffer

                .append("Info", "SECM application started successfully");

            debug_info!("startup", "log_buffer initialized");



            // Initialize system tray (non-fatal: window still opens without tray)

            match tray::create_tray(app.handle()) {

                Ok(_tray) => {

                    state.log_buffer.append("Info", "System tray initialized");

                }

                Err(e) => {

                    eprintln!("Tray creation failed (non-fatal): {}", e);

                    state

                        .log_buffer

                        .append("Warn", &format!("Tray creation failed: {}", e));

                }

            }



            // 硬件传感器 HTTP 服务(non-fatal:绑定失败不中断主程序)

            match crate::hardware::start_http_server() {

                Ok(port) => {

                    state.log_buffer.append(

                        "Info",

                        &format!("Hardware sensor HTTP service on 127.0.0.1:{}", port),

                    );

                }

                Err(e) => {

                    eprintln!("Hardware HTTP service failed (non-fatal): {}", e);

                    state

                        .log_buffer

                        .append("Warn", &format!("Hardware HTTP service failed: {}", e));

                }

            }



            // LHM sidecar 资源定位（非阻塞：仅解析并缓存 exe 路径；进程 lazy 启动）

            crate::lhm::init(Some(app.handle()));

            // PawnIO 驱动资源目录定位（缓存供 lhm.rs / 自愈线程无 AppHandle 时使用）

            crate::driver_install::pawnio::init(Some(app.handle()));



            // 驱动通道自愈（后台线程，非阻塞启动）：
            // 启动时自动执行驱动安装/探测链（WinRing0 → ACPI），
            // 修复「WinRing0 服务停止/被改动后 CPU 温度、功率数据永久丢失」问题
            // （v0.17.5 修复：此前仅 driver_status/driver_install 命令触发且前端未调用，
            //   服务停止后无人拉起，CPU 卡片温度/功率恒为 0）。
            // 非管理员进程走既有通道探测（不弹 UAC）；管理员进程自动重建并启动
            // 失效的驱动服务（scm_ensure_service_running 内置「存在但启动失败→重建」逻辑，
            //   已在本机验证：系统目录旧实例启动失败 5，重建指向资源目录后正常加载）。
            {
                let app_handle = app.handle().clone();
                std::thread::Builder::new()
                    .name("driver-ensure".to_string())
                    .spawn(move || {
                        // 并列自愈 PawnIO 服务（LHM 温度链依赖；失败仅日志不阻断——
                        // sidecar 会报 available:false 由 SECM 降级原链，不劣化现状）
                        if let Err(e) =
                            crate::driver_install::pawnio::ensure_pawnio_ready_opt(Some(&app_handle))
                        {
                            debug_info!(
                                "driver_install",
                                "PawnIO 服务自愈失败（LHM 将降级原链）: {}",
                                e
                            );
                        }
                        let status = crate::driver_install::ensure_temp_channel(&app_handle);
                        debug_info!(
                            "driver_install",
                            "启动自愈完成: channel={} available={}",
                            status.channel,
                            status.available
                        );
                        // 通道状态写入主日志（用户日志页可见，便于排查）
                        let degraded_note = if status.degraded.is_empty() {
                            String::new()
                        } else {
                            format!(" degraded={}", status.degraded.join("; "))
                        };
                        // channel=none 且检测到运行中安全软件时，追加显式拦截指引（火绒等
                        // 驱动防护可能拦截 WinRing0 加载；0x430/0x431 服务标记删除需重启电脑）
                        let security_note = if status.channel == "none"
                            && status.security.iter().any(|s| s.active)
                        {
                            " 提示：检测到运行中的安全软件（火绒等），其驱动防护可能拦截 WinRing0 加载，请按引导卡步骤放行后一键重试；服务标记删除（0x430/0x431）时请重启电脑".to_string()
                        } else {
                            String::new()
                        };
                        if let Some(state) = app_handle.try_state::<AppState>() {
                            state.log_buffer.append(
                                "Info",
                                &format!(
                                    "驱动通道自愈完成: channel={} available={}{}{}",
                                    status.channel,
                                    status.available,
                                    degraded_note,
                                    security_note
                                ),
                            );
                        }
                    })
                    .ok();
            }

            // Set up close-to-tray behavior

            let window = app

                .get_webview_window("main")

                .expect("main window not found");

            let window_clone = window.clone();

            window.on_window_event(move |event| {

                if let tauri::WindowEvent::CloseRequested { api, .. } = event {

                    api.prevent_close();

                    let _ = window_clone.hide();

                }

            });



            Ok(())

        })

        .invoke_handler(tauri::generate_handler![

            commands::get_logs,

            commands::clear_logs,

            commands::export_logs,

            commands::get_app_version,
            commands::append_frontend_log,

            commands::get_sensor_data,

            commands::get_hardware_sensors,

            commands::get_debug_logs,

            commands::flush_dns,

            commands::clean_shader_cache,

            commands::clean_nvidia_cache,

            commands::clean_amd_cache,

            commands::clean_directx_cache,

            commands::clean_steam_cache,

            commands::clean_temp_files,

            commands::trim_process_working_set,

            commands::set_priority,

            commands::ping_stream,

            commands::traceroute_stream,

            commands::nslookup_stream,

            commands::detect_nat_type_stream,

            commands::cancel_net_cmd,

            commands::iperf3_stream,

            commands::get_hags_state,

            commands::set_hags_state,

            commands::get_game_mode_state,

            commands::set_game_mode_state,

            commands::get_game_optimization_state,

            commands::set_game_optimization,

            commands::get_mouse_precision_state,

            commands::set_mouse_precision,

            commands::get_nvidia_power_mode,

            commands::set_nvidia_power_mode,

            commands::get_hpt_state,

            commands::set_hpt_state,

            commands::get_vrr_state,

            commands::set_vrr_state,

            commands::get_power_plans,

            commands::set_power_plan,

            commands::delete_power_plan,

            commands::get_hetero_policies,

            commands::set_hetero_policy,

            commands::enable_ultimate_performance,

            commands::get_services,

            commands::list_all_services,

            commands::start_service,

            commands::stop_service,

            commands::set_service_start_type,

            commands::get_log_capacity,

            commands::list_processes,

            commands::check_directx,

            commands::check_vc_runtimes,

            commands::check_ai_tools,

            commands::install_ai_tool,

            commands::fetch_ai_latest_versions,
            commands::check_npm_environment,
            commands::uninstall_ai_tool,
            commands::list_mcp_servers,
            commands::install_mcp_server,
            commands::uninstall_mcp_server,
            commands::list_extensions,

            commands::get_game_presets,

            commands::get_system_info,

            commands::get_net_stats,

            commands::get_chart_history,

            commands::get_link_speeds,

            commands::get_ip_info,

            commands::check_dhcp_servers,

            commands::deep_check_dhcp_servers,

            commands::get_network_config,

            commands::apply_network_config,

            commands::set_dns,

            commands::get_doh_config,

            commands::set_doh_config,

            commands::set_network_mac,

            commands::driver_status,

            commands::driver_install,

            commands::driver_uninstall,

            commands::list_disks,

            commands::get_disk_smart,

        ])

        .build(tauri::generate_context!())

        .expect("error while building tauri application")

        .run(|_app, event| {
            // M1：应用退出前释放 WinRing0 设备句柄，避免服务停止卡 STOP_PENDING
            if let tauri::RunEvent::Exit = event {
                crate::driver_install::winring0::close_device();
            }
            // LHM sidecar 退出清理（kill 子进程 + taskkill 兜底孤儿）
            if let tauri::RunEvent::Exit = event {
                crate::lhm::shutdown();
            }
        });

}

