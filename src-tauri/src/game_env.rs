/// 游戏环境配置预设模块
///
/// 为热门游戏提供推荐的系统设置清单，并与当前系统状态对比。
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GamePreset {
    pub id: String,
    pub name: String,
    pub engine: String,
    pub settings: Vec<GameSetting>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameSetting {
    /// 设置项标识（前端契约）：
    /// "hags" | "game_mode" | "vrr" | "mouse_precision" | "power_plan" 为可联动开关/设置项，
    /// 空串表示纯检测项（GPU 驱动/虚拟内存/反作弊等，不可切换）。
    pub key: String,
    pub label: String,
    pub description: String,
    pub recommended: String,
    pub current: String,
    pub ok: bool,
}

/// 获取所有游戏的推荐配置预设（当前值从系统实时读取）
pub fn get_game_presets() -> Vec<GamePreset> {
    let current = read_current_settings();

    vec![
        // ── CS2 ──
        GamePreset {
            id: "cs2".into(),
            name: "CS2".into(),
            engine: "Source 2".into(),
            settings: vec![
                gs(
                    "硬件加速 GPU 调度",
                    "开启以降低渲染延迟",
                    "开启",
                    &current.hags,
                ),
                gs("游戏模式", "自动优化后台进程", "开启", &current.game_mode),
                gs(
                    "VRR / G-Sync",
                    "竞技 FPS 建议关闭以避免输入延迟",
                    "关闭",
                    &current.vrr,
                ),
                gs(
                    "鼠标精准度",
                    "原始输入需要关闭增强指针精准度",
                    "关闭",
                    &current.mouse_precision,
                ),
                gs(
                    "电源计划",
                    "高性能确保 CPU 频率稳定",
                    "高性能",
                    &current.power_plan,
                ),
                gs(
                    "GPU 驱动",
                    "NVIDIA 驱动 ≥ 545.xx 支持 Reflex",
                    "≥545.xx",
                    &current.gpu_driver,
                ),
            ],
        },
        // ── 三角洲行动 ──
        GamePreset {
            id: "delta".into(),
            name: "三角洲行动".into(),
            engine: "Unreal Engine 5".into(),
            settings: vec![
                gs(
                    "硬件加速 GPU 调度",
                    "UE5 引擎推荐开启",
                    "开启",
                    &current.hags,
                ),
                gs("游戏模式", "减少后台干扰", "开启", &current.game_mode),
                gs(
                    "VRR / G-Sync",
                    "单人/合作可开启 G-Sync 消除撕裂",
                    "开启",
                    &current.vrr,
                ),
                gs(
                    "电源计划",
                    "卓越性能避免降频",
                    "卓越性能",
                    &current.power_plan,
                ),
                gs(
                    "GPU 驱动",
                    "NVIDIA 驱动 ≥ 545.xx 支持 DLSS 3",
                    "≥545.xx",
                    &current.gpu_driver,
                ),
                gs(
                    "虚拟内存",
                    "推荐 ≥ 16GB（UE5 高显存占用）",
                    "≥16GB",
                    &current.virtual_memory,
                ),
            ],
        },
        // ── 瓦洛兰特 ──
        GamePreset {
            id: "valorant".into(),
            name: "瓦洛兰特".into(),
            engine: "Unreal Engine 4".into(),
            settings: vec![
                gs(
                    "硬件加速 GPU 调度",
                    "Valorant CPU 密集，建议关闭减少调度延迟",
                    "关闭",
                    &current.hags,
                ),
                gs("游戏模式", "开启", "开启", &current.game_mode),
                gs(
                    "VRR / G-Sync",
                    "竞技 FPS 关闭以降低输入延迟",
                    "关闭",
                    &current.vrr,
                ),
                gs(
                    "鼠标精准度",
                    "必须关闭保证瞄准精度",
                    "关闭",
                    &current.mouse_precision,
                ),
                gs(
                    "电源计划",
                    "高性能确保帧率稳定",
                    "高性能",
                    &current.power_plan,
                ),
                gs(
                    "Vanguard 反作弊",
                    "Riot Vanguard 需运行中",
                    "运行中",
                    &current.vanguard,
                ),
            ],
        },
        // ── 绝地求生 PUBG ──
        GamePreset {
            id: "pubg".into(),
            name: "绝地求生".into(),
            engine: "Unreal Engine 4".into(),
            settings: vec![
                gs(
                    "硬件加速 GPU 调度",
                    "开启以提升最低帧",
                    "开启",
                    &current.hags,
                ),
                gs("游戏模式", "开启", "开启", &current.game_mode),
                gs(
                    "VRR / G-Sync",
                    "竞技模式关闭，普通模式可开",
                    "关闭",
                    &current.vrr,
                ),
                gs(
                    "鼠标精准度",
                    "关闭以使用原始输入",
                    "关闭",
                    &current.mouse_precision,
                ),
                gs("电源计划", "高性能", "高性能", &current.power_plan),
                gs(
                    "BattlEye 反作弊",
                    "BattlEye 服务需运行中",
                    "运行中",
                    &current.battleye,
                ),
            ],
        },
        // ── 英雄联盟 LoL ──
        GamePreset {
            id: "lol".into(),
            name: "英雄联盟".into(),
            engine: "专有引擎".into(),
            settings: vec![
                gs("游戏模式", "开启", "开启", &current.game_mode),
                gs("VRR / G-Sync", "可开启消除画面撕裂", "开启", &current.vrr),
                gs(
                    "鼠标精准度",
                    "建议关闭保证精准点击",
                    "关闭",
                    &current.mouse_precision,
                ),
                gs(
                    "电源计划",
                    "平衡即可，LoL 资源占用低",
                    "平衡",
                    &current.power_plan,
                ),
                gs("GPU 驱动", "任意较新驱动均可", "任意", &current.gpu_driver),
            ],
        },
    ]
}

/// 系统当前设置快照
struct CurrentSettings {
    hags: String,
    game_mode: String,
    vrr: String,
    mouse_precision: String,
    power_plan: String,
    gpu_driver: String,
    virtual_memory: String,
    vanguard: String,
    battleye: String,
}

fn read_current_settings() -> CurrentSettings {
    // HAGS
    let hags = crate::settings::get_hags_state();
    let hags_str = if hags.enabled { "开启" } else { "关闭" }.to_string();

    // 游戏模式（与系统设置页「游戏模式」开关同源联动：get_game_mode_state）
    let game_mode = crate::settings::get_game_mode_state();
    let gm_str = if game_mode.enabled {
        "开启"
    } else {
        "关闭"
    }
    .to_string();

    // VRR
    let vrr = crate::settings::get_vrr_state();
    let vrr_str = if vrr.enabled { "开启" } else { "关闭" }.to_string();

    // 鼠标精准度
    let mp = crate::settings::get_mouse_precision_state();
    let mp_str = if mp.enabled { "开启" } else { "关闭" }.to_string();

    // 电源计划
    let plans = crate::settings::get_power_plans().unwrap_or_default();
    let active = plans
        .iter()
        .find(|p| p.is_active)
        .map(|p| {
            if p.name.contains("卓越") {
                "卓越性能"
            } else if p.name.contains("高性能") {
                "高性能"
            } else if p.name.contains("平衡") {
                "平衡"
            } else if p.name.contains("节能") {
                "节能"
            } else {
                &p.name
            }
        })
        .unwrap_or("未知");
    let pp_str = active.to_string();

    // GPU 驱动
    let gpu_str = read_gpu_driver_version().unwrap_or_else(|| "未知".into());

    // 虚拟内存
    let vm_str = read_virtual_memory();

    // Vanguard
    let vanguard_str = check_service("vgc"); // Vanguard service

    // BattlEye
    let battleye_str = check_service("BEService");

    CurrentSettings {
        hags: hags_str,
        game_mode: gm_str,
        vrr: vrr_str,
        mouse_precision: mp_str,
        power_plan: pp_str,
        gpu_driver: gpu_str,
        virtual_memory: vm_str,
        vanguard: vanguard_str,
        battleye: battleye_str,
    }
}

fn gs(label: &str, desc: &str, recommended: &str, current: &str) -> GameSetting {
    // 设置项标识：与系统设置页开关一一对应（前端据此渲染 Switch 并调用对应 setter 命令联动）
    let key = match label {
        "硬件加速 GPU 调度" => "hags",
        "游戏模式" => "game_mode",
        "VRR / G-Sync" => "vrr",
        "鼠标精准度" => "mouse_precision",
        "电源计划" => "power_plan",
        _ => "",
    };
    let ok = match (recommended, current) {
        ("开启", "开启") | ("关闭", "关闭") => true,
        ("高性能", "高性能") | ("高性能", "卓越性能") | ("卓越性能", "卓越性能") => {
            true
        }
        ("平衡", "平衡") | ("平衡", "高性能") | ("平衡", "卓越性能") => true,
        ("运行中", "运行中") => true,
        (r, _c) if r.starts_with('≥') || r.starts_with("任意") => true,
        _ => false,
    };
    GameSetting {
        key: key.into(),
        label: label.into(),
        description: desc.into(),
        recommended: recommended.into(),
        current: current.into(),
        ok,
    }
}

fn read_gpu_driver_version() -> Option<String> {
    use winreg::enums::*;
    use winreg::RegKey;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm.open_subkey_with_flags(
        "SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}\\0000",
        KEY_READ,
    ).ok()?;
    let ver: String = key.get_value("DriverVersion").ok()?;
    let date: String = key.get_value("DriverDate").ok().unwrap_or_default();
    Some(format!("{} ({})", ver, date))
}

fn read_virtual_memory() -> String {
    use winreg::enums::*;
    use winreg::RegKey;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok(key) = hklm.open_subkey_with_flags(
        "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Memory Management",
        KEY_READ,
    ) {
        // PagingFiles 包含分页文件大小信息
        if let Ok(pf) = key.get_value::<String, _>("PagingFiles") {
            // 格式: "C:\pagefile.sys 2048 4096"
            if let Some(last) = pf.split_whitespace().last() {
                if let Ok(mb) = last.parse::<u64>() {
                    let gb = mb / 1024;
                    return format!("{}GB", gb);
                }
            }
        }
    }
    "未知".into()
}

/// 检查服务运行状态（P12，advapi32 QueryServiceStatusEx）
///
/// 走纯 Rust 驱动（secm-datasource::service::query_service）。
/// 服务不存在/查询失败 → "未安装"。
fn check_service(name: &str) -> String {
    match crate::datasource::service::query_service(name) {
        Ok(Some(info)) if info.status == "Running" => "运行中".into(),
        Ok(Some(_)) => "已停止".into(),
        Ok(None) => "未安装".into(),
        Err(e) => {
            let msg = crate::ds_util::log_err("game_env", "检查服务状态", &e);
            crate::debug_warn!("game_env", "服务 '{}' 状态降级: {}", name, msg);
            "未安装".into()
        }
    }
}
