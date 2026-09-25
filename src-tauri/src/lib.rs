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

pub mod dhcp_residue;

pub mod disk_info;

pub mod ds_util;

pub mod driver_install;

pub mod environment;

pub mod game_env;

pub mod hpt;

pub mod ip_info;

pub mod iperf;

pub mod lhm;

pub mod log;

pub mod net_stats;

pub mod net_config;

pub mod network;

pub mod nvidia_drs;

pub mod proc_util;

pub mod runtime_paths;

pub mod sensor;

pub mod settings;

pub mod sysinfo;

pub mod startup_guard;

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

    // 初始化运行目录（开发期 <项目根>/build/runtime；发行版 <exe 同级>/runtime，
    // 不可写时回退 %APPDATA%\SECM）。创建结果在下列日志初始化之后显式上报。
    let runtime_failures = runtime_paths::try_init();

    // 初始化文件日志和诊断目录

    if let Err(e) = log::init_file_logger() {
        eprintln!(
            "Warning: Failed to initialize file logger: {}. Logging to memory only.",
            e
        );
    }

    // 运行目录创建失败显式上报（debug 体系此时已可用）；不阻断启动：
    // 写入方各自的失败路径已按 fast-fail 语义返回错误或记日志。
    for (name, reason) in &runtime_failures {
        debug_error!(
            "startup",
            "运行目录子目录 {} 创建失败: {}（运行数据落点: {}）",
            name,
            reason,
            runtime_paths::runtime_root().display()
        );
    }

    // 一次性迁移历史 %APPDATA%\SECM 数据到新运行目录（幂等，只复制不删除源）
    let (migrated, migration_failures) = runtime_paths::migrate_legacy_data();
    if migrated > 0 {
        debug_info!(
            "startup",
            "已从历史 %APPDATA%\\SECM 迁移 {} 个文件到 {}",
            migrated,
            runtime_paths::runtime_root().display()
        );
    }
    for reason in &migration_failures {
        debug_warn!("startup", "历史运行数据迁移跳过一项: {}", reason);
    }

    // 启动时清理过期诊断文件 + 强制总大小检查

    temp_data::init_temp_data();

    tauri::Builder::default()

        // 单实例机制：防止多个实例同时占用 WebView2 user-data-dir 导致黑屏/无法加载

        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {

            // 聚焦已有实例的主窗口；已有实例处于「无主窗口」的启动失败状态时
            // 给出中文提示，不再静默返回（用户看不到任何反馈即「闪退」）。

            crate::startup_guard::focus_or_report_missing_main(app);

        }))

        .plugin(tauri_plugin_dialog::init())

        .plugin(tauri_plugin_fs::init())

        .manage(app_state)

        .setup(move |app| {

            debug_info!(

                "startup",

                "SECM starting... version={}",

                env!("CARGO_PKG_VERSION")

            );

            // 后台确认主窗口真的带起了 WebView2。主窗口创建失败时 Tauri 只写日志
            // 不返回错误（tauri-2.11.5/src/app.rs:2524-2532），实测失败态下进程会以
            // 无界面状态继续运行并占用单实例互斥体，用户再次双击只会得到二十余毫秒的
            // 静默退出。看门狗超时后提示用户并结束进程，释放互斥体（详见 startup_guard.rs）。
            crate::startup_guard::spawn_startup_watchdog(app.handle());



            // Store app handle for event emission (log_buffer uses std::sync::Mutex, no async needed)

            let handle = app.handle().clone();

            let state = app.state::<AppState>();

            state.log_buffer.set_app_handle(handle);

            state

                .log_buffer

                .append_scoped("Info", "SECM application started successfully", "system");

            debug_info!("startup", "log_buffer initialized");



            // Initialize system tray (non-fatal: window still opens without tray)

            match tray::create_tray(app.handle()) {

                Ok(_tray) => {

                    state.log_buffer.append_scoped("Info", "System tray initialized", "system");

                }

                Err(e) => {

                    eprintln!("Tray creation failed (non-fatal): {}", e);

                    state

                        .log_buffer

                        .append_scoped("Warn", &format!("Tray creation failed: {}", e), "system");

                }

            }



            // LHM sidecar 资源定位（非阻塞：仅解析并缓存 exe 路径；进程 lazy 启动）

            crate::lhm::init(Some(app.handle()));

            // LHM sidecar 预热（后台线程，非阻塞 setup）：把 .NET 冷启动 +
            // 健康检查握手 + LHM 开树的等待从「首个 get_sensor_data 请求路径」
            // 移到应用启动阶段。生产日志实测首个请求因此阻塞 4073ms 且返回
            // available:false 空占位快照（硬件信息页长时间停在骨架屏）。
            // 复用与请求路径完全相同的 lhm_snapshot()，不改变契约与降级语义，
            // 不弹 UAC（sidecar 仍以普通权限启动，tier-2 需用户显式提权）。
            crate::lhm::warm_up();

            // PawnIO 驱动资源目录定位（缓存供 lhm.rs / 自愈线程无 AppHandle 时使用）

            crate::driver_install::pawnio::init(Some(app.handle()));

            // 这里曾有「隐藏 WebView2 页面测速窗口」与 `SECM_PAGE_PROBE_SELFTEST`
            // 自检入口（v1.19.2 引入）。「网站测试」统一为 HTTP 延迟测试后，
            // 浏览器页面测速不再存在，故测速窗口与自检一并移除：不再创建任何
            // 隐藏 webview，也就不再需要处理 0x8007139F 那类
            // "窗口在、webview 不在"的初始化陷阱。



            // PawnIO 驱动自愈（后台线程，非阻塞启动）：
            // SECM 自身已无 WinRing0/ACPI 温度通道；驱动侧只剩 PawnIO 就绪探测，
            // 供 LHM sidecar 的 tier-2（ring0）使用。失败仅记日志不阻断：
            // 非管理员进程探测不到时由 sidecar 报 available=false，SECM 正常降级。
            {
                let app_handle = app.handle().clone();
                std::thread::Builder::new()
                    .name("driver-ensure".to_string())
                    .spawn(move || {
                        // PawnIO 服务自愈（LHM tier-2 ring0 依赖；失败仅日志不阻断）
                        // 注意：不主动弹 UAC——提权由用户在前端按需触发。
                        match crate::driver_install::pawnio::ensure_pawnio_ready_opt(Some(&app_handle))
                        {
                            Ok(()) => {
                                debug_info!("driver_install", "PawnIO 就绪探测完成: available=true");
                                if let Some(state) = app_handle.try_state::<AppState>() {
                                    state
                                        .log_buffer
                                        .append_scoped(
                                            "Info",
                                            "PawnIO 驱动就绪（LHM tier-2 ring0 可用）",
                                            "system",
                                        );
                                }
                            }
                            Err(e) => {
                                debug_info!(
                                    "driver_install",
                                    "PawnIO 服务自愈失败（LHM 将仅提供 tier-1 用户态数据）: {}",
                                    e
                                );
                                if let Some(state) = app_handle.try_state::<AppState>() {
                                    state.log_buffer.append_scoped(
                                        "Info",
                                        &format!(
                                            "PawnIO 驱动不可用（LHM tier-2 ring0 数据缺失，需管理员按需提权）: {}",
                                            e
                                        ),
                                        "system",
                                    );
                                }
                            }
                        }
                    })
                    .ok();
            }

            // Set up close-to-tray behavior
            //
            // 主窗口缺失时不在此处 panic：panic 会中断事件循环并跳过启动守卫的
            // 提示与退出流程。缺少主窗口的情况统一由 startup_guard 的看门狗判定，
            // 它会提示用户并结束进程。
            if let Some(window) = app.get_webview_window(crate::startup_guard::MAIN_WINDOW_LABEL)
            {
                let window_clone = window.clone();

                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();

                        let _ = window_clone.hide();
                    }
                });
            } else {
                debug_error!(
                    "startup",
                    "关闭到托盘未注册：主窗口 {} 不可用",
                    crate::startup_guard::MAIN_WINDOW_LABEL
                );
            }

            Ok(())

        })

        .invoke_handler(tauri::generate_handler![

            commands::get_logs,

            commands::clear_logs,

            commands::export_logs,

            commands::get_app_version,
            commands::append_frontend_log,

            commands::get_sensor_data,

            commands::request_elevation,

            commands::auto_elevate,

            commands::get_runtime_paths,

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

            commands::iperf3_tool_info,

            commands::iperf3_server_start,

            commands::iperf3_server_stop,

            commands::iperf3_server_status,

            commands::iperf3_firewall_rule,

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

            commands::list_all_services,

            commands::start_service,

            commands::stop_service,

            commands::set_service_start_type,

            commands::get_log_capacity,

            commands::list_processes,

            commands::check_directx,

            commands::check_vc_runtimes,

            commands::get_game_presets,

            commands::get_system_info,

            commands::get_net_stats,

            commands::get_chart_history,

            commands::get_link_speeds,

            commands::get_ip_info,

            commands::check_dhcp_servers,

            commands::deep_check_dhcp_servers,

            commands::scan_dhcp_residue,

            commands::clean_dhcp_residue,

            commands::probe_url,

            commands::get_network_config,

            commands::apply_network_config,

            commands::set_dns,

            commands::get_doh_config,

            commands::set_doh_config,

            commands::set_network_mac,

            commands::driver_status,

            commands::driver_install,

            commands::list_disks,

            commands::get_disk_smart,

        ])

        .build(tauri::generate_context!())

        .expect("error while building tauri application")

        .run(|_app, event| {
            // LHM sidecar 退出清理（kill 子进程 + taskkill 兜底孤儿）
            if let tauri::RunEvent::Exit = event {
                crate::lhm::shutdown();
                // iperf3 服务端退出清理：用户在页面上启动的是长驻 `iperf3 -s`
                // 子进程，不显式杀死会在应用退出后继续占用端口（成为孤儿进程）。
                crate::iperf::shutdown_server();
            }
        });
}
