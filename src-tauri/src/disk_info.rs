//! 磁盘 S.M.A.R.T 业务模块 — 健康度判断 / 告警字段 / mock 开关
//!
//! 职责边界（沿项目架构）：
//! - 底层采集（Win32 IOCTL）在 `secm_datasource::disk`（datasource crate，叶子模块）
//! - 本模块做业务加工：健康状态汇总（Good/Warning/Critical）、告警原因、mock 数据开关
//! - 命令层（commands.rs）只做 spawn_blocking 包装 + 错误映射
//!
//! 接口语义（对应前端任务 t_1cec17db 的 GET /api/disks、GET /api/disks/:id/smart）：
//! - `list_disks()`  ↔ GET /api/disks          → 磁盘简要列表（id/型号/健康摘要/是否告警）
//! - `get_disk_smart(id)` ↔ GET /api/disks/:id/smart → 完整 S.M.A.R.T 属性 + 健康分析
//! 落地为 Tauri 命令（S5 铁律：前端走 invoke IPC，不直连系统接口）。
//!
//! mock 开关：环境变量 `SECM_DISK_MOCK=1`（或任意非空值）启用可配置 mock 数据，
//! 用于前端渲染 / 无 SMART 权限环境 / 测试。默认真实采集。
//!
//! 线程模型：同步阻塞采集 MUST 由命令层 spawn_blocking 执行（S8）。

use crate::datasource::disk::{self, DiskInfo, DiskSmartData, NvmeHealthLog, SmartAttribute};
use serde::Serialize;

// ---------------------------------------------------------------------------
// 对外数据模型（serde Serialize → 前端）
// ---------------------------------------------------------------------------

/// 健康状态枚举（摘要 + 判断字段）
#[derive(Debug, Clone, Serialize)]
pub enum HealthStatus {
    /// 健康
    Good,
    /// 有风险（预警）
    Warning,
    /// 严重故障（告警）
    Critical,
    /// 无法读取 / 不支持
    Unknown,
}

impl HealthStatus {
    /// 序列化为前端可用字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            HealthStatus::Good => "Good",
            HealthStatus::Warning => "Warning",
            HealthStatus::Critical => "Critical",
            HealthStatus::Unknown => "Unknown",
        }
    }
}

/// 磁盘列表项（GET /api/disks 返回体元素）
#[derive(Debug, Clone, Serialize)]
pub struct DiskSummary {
    /// 磁盘 id（`\\.\PhysicalDriveN` 的 N）
    pub id: String,
    /// 型号
    pub model: String,
    /// 序列号
    pub serial: String,
    /// 接口类型：NVMe / SATA / ATA / USB / SCSI / 其他
    pub interface_type: String,
    /// 介质类型：HDD / SSD / 未知
    pub media_type: String,
    /// 容量（字节）
    pub size_bytes: u64,
    /// 健康度摘要
    pub health: HealthSummary,
}

/// 健康度摘要（卡片收起状态展示）
#[derive(Debug, Clone, Serialize)]
pub struct HealthSummary {
    /// 健康状态字符串：Good / Warning / Critical / Unknown
    pub status: String,
    /// 健康状态中文标签
    pub label: String,
    /// 是否告警（Critical 或 Warning 且有关键告警原因）
    pub alert: bool,
    /// 告警原因列表（空 = 无告警）
    pub alerts: Vec<String>,
    /// 温度（摄氏度，None = 无数据）
    pub temperature_c: Option<u16>,
    /// 通电时间（小时）
    pub power_on_hours: Option<u64>,
    /// 关键属性原始值：重新分配扇区数（ATA）/ 可用备用百分比（NVMe）
    pub reallocated_sectors: Option<u64>,
    /// 当前待定扇区数（ATA）
    pub pending_sectors: Option<u64>,
    /// 磨损百分比（NVMe percentage_used）
    pub wear_level_percent: Option<u8>,
}

/// 磁盘 S.M.A.R.T 详情（GET /api/disks/:id/smart 返回体）
#[derive(Debug, Clone, Serialize)]
pub struct DiskSmartDetail {
    /// 磁盘 id
    pub id: String,
    /// 型号
    pub model: String,
    /// 接口类型
    pub interface_type: String,
    /// 介质类型
    pub media_type: String,
    /// 健康度摘要（与列表一致，弹窗顶部展示）
    pub health: HealthSummary,
    /// 完整 S.M.A.R.T 属性表（ATA；NVMe 为空数组）
    pub attributes: Vec<SmartAttribute>,
    /// NVMe 健康日志精选字段（仅 NVMe）
    pub nvme_health: Option<NvmeHealthLog>,
    /// 读取失败时的可读错误（None = 正常；Some = 权限不足/不支持等，前端展示但页面不崩）
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// 公共 API
// ---------------------------------------------------------------------------

/// 是否启用 mock 数据（环境变量 SECM_DISK_MOCK 非空即启用）
pub fn mock_enabled() -> bool {
    std::env::var("SECM_DISK_MOCK")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// 磁盘简要列表（GET /api/disks）
///
/// - mock 启用 → 返回可配置 mock 列表（3 块盘：NVMe SSD 良好 / SATA SSD 预警 / SATA HDD 告警）
/// - 真实采集 → 每块盘附加健康摘要（读取失败盘 → Unknown + error 字段）
pub fn list_disks() -> Vec<DiskSummary> {
    if mock_enabled() {
        return mock_disk_summaries();
    }

    let disks = disk::enumerate_disks();
    disks
        .into_iter()
        .map(|info| {
            let health = summarize_health_for(&info);
            DiskSummary {
                id: info.id,
                model: info.model,
                serial: info.serial,
                interface_type: info.interface_type,
                media_type: info.media_type,
                size_bytes: info.size_bytes,
                health,
            }
        })
        .collect()
}

/// 单块磁盘完整 S.M.A.R.T 数据（GET /api/disks/:id/smart）
///
/// 读取失败（权限不足/总线不支持/盘不存在）不返回 Err（前端页面不崩），
/// 而是返回 `DiskSmartDetail` + `error` 字段 + Unknown 健康度（任务要求 5）。
pub fn get_disk_smart(id: &str) -> DiskSmartDetail {
    if mock_enabled() {
        return mock_smart_detail(id);
    }

    match disk::read_smart(id) {
        Ok(data) => {
            let health = summarize_smart(&data);
            DiskSmartDetail {
                id: data.id.clone(),
                model: data.model,
                interface_type: data.interface_type,
                media_type: data.media_type,
                health,
                attributes: data.attributes,
                nvme_health: data.nvme_health,
                error: None,
            }
        }
        Err(e) => {
            crate::debug_error!(
                "disk_smart",
                "读取磁盘 {} S.M.A.R.T 失败: {}（建议: 检查权限/接口类型）",
                id,
                e
            );
            DiskSmartDetail {
                id: id.to_string(),
                model: String::new(),
                interface_type: String::new(),
                media_type: String::new(),
                health: HealthSummary {
                    status: HealthStatus::Unknown.as_str().to_string(),
                    label: "无法读取".to_string(),
                    alert: false,
                    alerts: vec![format!("S.M.A.R.T 读取失败：{}", e)],
                    temperature_c: None,
                    power_on_hours: None,
                    reallocated_sectors: None,
                    pending_sectors: None,
                    wear_level_percent: None,
                },
                attributes: Vec::new(),
                nvme_health: None,
                error: Some(e.to_string()),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 健康度判断（任务要求 5：提供健康度提醒所需判断字段）
// ---------------------------------------------------------------------------

/// 根据磁盘基本信息生成健康摘要（读取 SMART 失败/无数据时用于列表展示）
fn summarize_health_for(info: &DiskInfo) -> HealthSummary {
    // 列表场景尝试读一次 SMART 摘要；失败则 Unknown（不阻塞列表渲染）
    match disk::read_smart(&info.id) {
        Ok(data) => summarize_smart(&data),
        Err(e) => HealthSummary {
            status: HealthStatus::Unknown.as_str().to_string(),
            label: "无法读取".to_string(),
            alert: false,
            alerts: vec![format!("S.M.A.R.T 读取失败：{}", e)],
            temperature_c: None,
            power_on_hours: None,
            reallocated_sectors: None,
            pending_sectors: None,
            wear_level_percent: None,
        },
    }
}

/// 汇总健康度：从 SMART 数据提取判断字段 + 告警
///
/// 数据来源分三种（按优先级）：
/// - IOCTL 完整数据（nvme_health / attributes）
/// - WMI 兜底（wmi_health：虚拟化环境，仅 HealthStatus 判断字段）
/// - 两者皆无（Unknown）
///
/// 判断逻辑（阈值可按需调整）：
/// - Critical：NVMe critical_warning 非零 / 温度 ≥ 70°C / 媒体错误 > 0 / ATA 属性 FAILED 或 FAILING
///   / 重新分配扇区 ≥ 100 / 当前待定扇区 ≥ 10 / 磨损 100% / WMI HealthStatus=2(Unhealthy)
/// - Warning：NVMe 备用空间低于阈值 / 磨损 ≥ 80% / 温度 ≥ 60°C / ATA 重新分配 > 0 / 当前待定 > 0
///   / WMI HealthStatus=1(Warning)
/// - Good：其余（含 WMI HealthStatus=0(Healthy)）
fn summarize_smart(data: &DiskSmartData) -> HealthSummary {
    let mut alerts: Vec<String> = Vec::new();
    let mut temperature_c: Option<u16> = None;
    let mut power_on_hours: Option<u64> = None;
    let mut reallocated_sectors: Option<u64> = None;
    let mut pending_sectors: Option<u64> = None;
    let mut wear_level_percent: Option<u8> = None;

    let mut severity = 0u8; // 0=Good 1=Warning 2=Critical
                            // WMI 报告 Unknown（5）且无其他告警时，最终状态应为 Unknown 而非 Good
    let mut wmi_unknown = false;

    // WMI 兜底路径（虚拟化环境）：先按 HealthStatus 判定，再叠加 IOCTL 数据
    if let Some(wmi) = &data.wmi_health {
        match wmi.health_status {
            1 => {
                severity = severity.max(1);
                alerts.push("系统报告磁盘预警（Warning）".to_string());
            }
            2 => {
                severity = severity.max(2);
                alerts.push("系统报告磁盘故障（Unhealthy）".to_string());
            }
            // 5=Unknown：不升 severity（未知 ≠ 预警；曾误判为 Warning 导致
            // 无数据盘显示「预警 需关注」，v0.20.x 健康状态错误根因之一）
            5 => {
                wmi_unknown = true;
                alerts.push("磁盘健康状态未知（Unknown）".to_string());
            }
            _ => {}
        }
        if temperature_c.is_none() {
            temperature_c = wmi.temperature_c;
        }
        if power_on_hours.is_none() {
            power_on_hours = wmi.power_on_hours;
        }
        if let Some(r) = wmi.read_errors_total {
            if r > 0 {
                severity = severity.max(1);
                alerts.push(format!("读取错误总数 {}", r));
            }
        }
        if let Some(w) = wmi.write_errors_total {
            if w > 0 {
                severity = severity.max(1);
                alerts.push(format!("写入错误总数 {}", w));
            }
        }
    }

    if let Some(nvme) = &data.nvme_health {
        temperature_c = Some(nvme.temperature_c);
        power_on_hours = Some(nvme.power_on_hours);
        wear_level_percent = Some(nvme.percentage_used);
        reallocated_sectors = Some(100u64.saturating_sub(nvme.available_spare as u64));

        // 临界警告位：bit0 温度过高 bit1 备用空间不足 bit2 可靠性降级 bit3 介质只读 bit4 易失备份失败
        if nvme.critical_warning != 0 {
            severity = severity.max(2);
            if nvme.critical_warning & 0x01 != 0 {
                alerts.push("控制器温度过高（临界警告位 0）".to_string());
            }
            if nvme.critical_warning & 0x02 != 0 {
                alerts.push("备用空间不足（临界警告位 1）".to_string());
            }
            if nvme.critical_warning & 0x04 != 0 {
                alerts.push("可靠性降级（临界警告位 2）".to_string());
            }
            if nvme.critical_warning & 0x08 != 0 {
                alerts.push("介质已进入只读模式（临界警告位 3）".to_string());
            }
            if nvme.critical_warning & 0x10 != 0 {
                alerts.push("易失性备份电源失效（临界警告位 4）".to_string());
            }
        }
        if nvme.media_errors > 0 {
            severity = severity.max(2);
            alerts.push(format!("媒体错误 {} 次", nvme.media_errors));
        }
        if nvme.available_spare < nvme.available_spare_threshold {
            severity = severity.max(1);
            alerts.push(format!(
                "备用空间不足（{}% < 阈值 {}%）",
                nvme.available_spare, nvme.available_spare_threshold
            ));
        }
        if nvme.percentage_used >= 100 {
            severity = severity.max(2);
            alerts.push("寿命已用尽（磨损 100%）".to_string());
        } else if nvme.percentage_used >= 80 {
            severity = severity.max(1);
            alerts.push(format!(
                "寿命磨损 {}%（接近寿命末期）",
                nvme.percentage_used
            ));
        }
        if nvme.temperature_c >= 70 {
            severity = severity.max(2);
            alerts.push(format!("温度过高 {}°C", nvme.temperature_c));
        } else if nvme.temperature_c >= 60 {
            severity = severity.max(1);
            alerts.push(format!("温度偏高 {}°C", nvme.temperature_c));
        }
        if nvme.unsafe_shutdowns >= 100 {
            severity = severity.max(1);
            alerts.push(format!(
                "不安全关机次数较多（{} 次）",
                nvme.unsafe_shutdowns
            ));
        }
    }

    // ATA 属性表
    for attr in &data.attributes {
        if attr.status == "FAILED" || attr.status == "FAILING" {
            severity = severity.max(2);
            alerts.push(format!(
                "属性 {} 状态 {}（raw={}）",
                attr.name, attr.status, attr.raw
            ));
        }
        match attr.id {
            0x09 => power_on_hours = Some(attr.raw),
            0xC2 => {
                // 温度 raw 单位：多数盘摄氏度，个别盘为开尔文/0.1°C；按摄氏度解释
                temperature_c = Some(attr.raw as u16);
            }
            0x05 => {
                reallocated_sectors = Some(attr.raw);
                if attr.raw >= 100 {
                    severity = severity.max(2);
                    alerts.push(format!("重新分配扇区数 {}（严重）", attr.raw));
                } else if attr.raw > 0 {
                    severity = severity.max(1);
                    alerts.push(format!("重新分配扇区数 {}（注意）", attr.raw));
                }
            }
            0xC5 => {
                pending_sectors = Some(attr.raw);
                if attr.raw >= 10 {
                    severity = severity.max(2);
                    alerts.push(format!("当前待定扇区数 {}（严重）", attr.raw));
                } else if attr.raw > 0 {
                    severity = severity.max(1);
                    alerts.push(format!("当前待定扇区数 {}（注意）", attr.raw));
                }
            }
            0xC6 => {
                if attr.raw > 0 {
                    severity = severity.max(2);
                    alerts.push(format!("无法校正的扇区数 {}", attr.raw));
                }
            }
            // Intel 介质磨损指示器（0xE9）：value 即剩余寿命健康度 %（如 99 = 健康度 99%），
            // 磨损百分比 = 100 - value；value 下降预示寿命衰减
            0xE9 => {
                if attr.value <= 10 {
                    severity = severity.max(2);
                    alerts.push(format!("介质磨损严重（健康度仅 {}%）", attr.value));
                } else if attr.value <= 50 {
                    severity = severity.max(1);
                    alerts.push(format!("介质磨损较快（健康度 {}%）", attr.value));
                }
                wear_level_percent = Some(100u8.saturating_sub(attr.value));
            }
            _ => {}
        }
    }

    let (status, label) = match severity {
        0 if wmi_unknown => (HealthStatus::Unknown, "未知".to_string()),
        0 => (HealthStatus::Good, "健康".to_string()),
        1 => (HealthStatus::Warning, "预警".to_string()),
        _ => (HealthStatus::Critical, "严重".to_string()),
    };
    // 未知不算告警（无真实风险证据，避免误导用户）
    let alert = severity >= 1 && !wmi_unknown;

    HealthSummary {
        status: status.as_str().to_string(),
        label,
        alert,
        alerts,
        temperature_c,
        power_on_hours,
        reallocated_sectors,
        pending_sectors,
        wear_level_percent,
    }
}

// ---------------------------------------------------------------------------
// Mock 数据（可配置：环境变量 SECM_DISK_MOCK=1 启用）
// ---------------------------------------------------------------------------

/// mock 磁盘列表：3 块盘覆盖 良好/预警/严重 三种健康度
fn mock_disk_summaries() -> Vec<DiskSummary> {
    vec![
        DiskSummary {
            id: "0".to_string(),
            model: "Samsung SSD 970 EVO Plus 1TB (Mock)".to_string(),
            serial: "MOCK-SATA-0001".to_string(),
            interface_type: "NVMe".to_string(),
            media_type: "SSD".to_string(),
            size_bytes: 1_000_204_886_016,
            health: HealthSummary {
                status: "Good".to_string(),
                label: "健康".to_string(),
                alert: false,
                alerts: Vec::new(),
                temperature_c: Some(38),
                power_on_hours: Some(2345),
                reallocated_sectors: Some(0),
                pending_sectors: Some(0),
                wear_level_percent: Some(12),
            },
        },
        DiskSummary {
            id: "1".to_string(),
            model: "WDC WD40EZAZ-00SF3B0 (Mock)".to_string(),
            serial: "MOCK-HDD-0002".to_string(),
            interface_type: "SATA".to_string(),
            media_type: "HDD".to_string(),
            size_bytes: 4_000_787_030_016,
            health: HealthSummary {
                status: "Warning".to_string(),
                label: "预警".to_string(),
                alert: true,
                alerts: vec![
                    "重新分配扇区数 12（注意）".to_string(),
                    "当前待定扇区数 3（注意）".to_string(),
                ],
                temperature_c: Some(46),
                power_on_hours: Some(18432),
                reallocated_sectors: Some(12),
                pending_sectors: Some(3),
                wear_level_percent: None,
            },
        },
        DiskSummary {
            id: "2".to_string(),
            model: "Samsung SSD 860 EVO 500GB (Mock)".to_string(),
            serial: "MOCK-SATA-0003".to_string(),
            interface_type: "SATA".to_string(),
            media_type: "SSD".to_string(),
            size_bytes: 500_107_862_016,
            health: HealthSummary {
                status: "Critical".to_string(),
                label: "严重".to_string(),
                alert: true,
                alerts: vec![
                    "重新分配扇区数 356（严重）".to_string(),
                    "无法校正的扇区数 42".to_string(),
                ],
                temperature_c: Some(62),
                power_on_hours: Some(31560),
                reallocated_sectors: Some(356),
                pending_sectors: Some(0),
                wear_level_percent: None,
            },
        },
    ]
}

/// mock S.M.A.R.T 详情：按 id 返回（0=NVMe 健康良好，1=HDD 预警，2=SSD 严重，其他=未找到）
fn mock_smart_detail(id: &str) -> DiskSmartDetail {
    let nvme = id == "0";
    let warning = id == "1";
    let critical = id == "2";

    let (model, _serial, interface_type, media_type) = if nvme {
        (
            "Samsung SSD 970 EVO Plus 1TB (Mock)",
            "MOCK-SATA-0001",
            "NVMe",
            "SSD",
        )
    } else if warning {
        (
            "WDC WD40EZAZ-00SF3B0 (Mock)",
            "MOCK-HDD-0002",
            "SATA",
            "HDD",
        )
    } else if critical {
        (
            "Samsung SSD 860 EVO 500GB (Mock)",
            "MOCK-SATA-0003",
            "SATA",
            "SSD",
        )
    } else {
        return DiskSmartDetail {
            id: id.to_string(),
            model: String::new(),
            interface_type: String::new(),
            media_type: String::new(),
            health: HealthSummary {
                status: "Unknown".to_string(),
                label: "未找到".to_string(),
                alert: false,
                alerts: vec!["未找到该磁盘".to_string()],
                temperature_c: None,
                power_on_hours: None,
                reallocated_sectors: None,
                pending_sectors: None,
                wear_level_percent: None,
            },
            attributes: Vec::new(),
            nvme_health: None,
            error: Some(format!("磁盘 {} 不存在", id)),
        };
    };

    let health = if nvme {
        HealthSummary {
            status: "Good".to_string(),
            label: "健康".to_string(),
            alert: false,
            alerts: Vec::new(),
            temperature_c: Some(38),
            power_on_hours: Some(2345),
            reallocated_sectors: Some(0),
            pending_sectors: Some(0),
            wear_level_percent: Some(12),
        }
    } else if warning {
        HealthSummary {
            status: "Warning".to_string(),
            label: "预警".to_string(),
            alert: true,
            alerts: vec![
                "重新分配扇区数 12（注意）".to_string(),
                "当前待定扇区数 3（注意）".to_string(),
            ],
            temperature_c: Some(46),
            power_on_hours: Some(18432),
            reallocated_sectors: Some(12),
            pending_sectors: Some(3),
            wear_level_percent: None,
        }
    } else {
        HealthSummary {
            status: "Critical".to_string(),
            label: "严重".to_string(),
            alert: true,
            alerts: vec![
                "重新分配扇区数 356（严重）".to_string(),
                "无法校正的扇区数 42".to_string(),
            ],
            temperature_c: Some(62),
            power_on_hours: Some(31560),
            reallocated_sectors: Some(356),
            pending_sectors: Some(0),
            wear_level_percent: None,
        }
    };

    let attributes = mock_attributes(warning, critical);
    let nvme_health = if nvme {
        Some(NvmeHealthLog {
            critical_warning: 0,
            temperature_c: 38,
            available_spare: 88,
            available_spare_threshold: 10,
            percentage_used: 12,
            power_on_hours: 2345,
            power_cycles: 312,
            unsafe_shutdowns: 4,
            media_errors: 0,
            data_units_read: 45_678_901_234,
            data_units_written: 67_890_123_456,
        })
    } else {
        None
    };

    DiskSmartDetail {
        id: id.to_string(),
        model: model.to_string(),
        interface_type: interface_type.to_string(),
        media_type: media_type.to_string(),
        health,
        attributes,
        nvme_health,
        error: None,
    }
}

/// mock ATA 属性表（warning/critical 控制关键属性原始值）
fn mock_attributes(warning: bool, critical: bool) -> Vec<SmartAttribute> {
    let (realloc, pending, uncorrect, temp, power_hours) = if critical {
        (356u64, 0u64, 42u64, 62u16, 31560u64)
    } else if warning {
        (12u64, 3u64, 0u64, 46u16, 18432u64)
    } else {
        (0u64, 0u64, 0u64, 38u16, 2345u64)
    };

    vec![
        SmartAttribute {
            id: 0x01,
            name: "底层读错误率".to_string(),
            raw: if critical { 9_999_999 } else { 1_234 },
            value: 100,
            worst: 100,
            threshold: 0,
            status: "OK".to_string(),
        },
        SmartAttribute {
            id: 0x05,
            name: "重新分配扇区数".to_string(),
            raw: realloc,
            value: if critical { 60 } else { 100 },
            worst: 60,
            threshold: 0,
            status: if critical { "FAILING" } else { "OK" }.to_string(),
        },
        SmartAttribute {
            id: 0x09,
            name: "通电时间".to_string(),
            raw: power_hours,
            value: 90,
            worst: 90,
            threshold: 0,
            status: "OK".to_string(),
        },
        SmartAttribute {
            id: 0x0C,
            name: "通电周期".to_string(),
            raw: 1000,
            value: 99,
            worst: 99,
            threshold: 0,
            status: "OK".to_string(),
        },
        SmartAttribute {
            id: 0xC2,
            name: "温度".to_string(),
            raw: temp as u64,
            value: 100,
            worst: 100,
            threshold: 0,
            status: "OK".to_string(),
        },
        SmartAttribute {
            id: 0xC5,
            name: "当前待定扇区数".to_string(),
            raw: pending,
            value: if warning { 90 } else { 100 },
            worst: 90,
            threshold: 0,
            status: if warning { "FAILING" } else { "OK" }.to_string(),
        },
        SmartAttribute {
            id: 0xC6,
            name: "离线不可校正扇区".to_string(),
            raw: uncorrect,
            value: if critical { 50 } else { 100 },
            worst: 50,
            threshold: 0,
            status: if critical { "FAILING" } else { "OK" }.to_string(),
        },
        SmartAttribute {
            id: 0xC7,
            name: "UDMA CRC 错误率".to_string(),
            raw: if critical { 88 } else { 0 },
            value: 100,
            worst: 100,
            threshold: 0,
            status: "OK".to_string(),
        },
    ]
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn nvme_data() -> DiskSmartData {
        DiskSmartData {
            id: "0".to_string(),
            model: "Test NVMe".to_string(),
            interface_type: "NVMe".to_string(),
            media_type: "SSD".to_string(),
            attributes: Vec::new(),
            nvme_health: Some(NvmeHealthLog {
                critical_warning: 0,
                temperature_c: 38,
                available_spare: 88,
                available_spare_threshold: 10,
                percentage_used: 12,
                power_on_hours: 2345,
                power_cycles: 100,
                unsafe_shutdowns: 3,
                media_errors: 0,
                data_units_read: 100,
                data_units_written: 200,
            }),
            wmi_health: None,
            source: "ioctl".to_string(),
        }
    }

    #[test]
    fn test_summarize_nvme_good() {
        let s = summarize_smart(&nvme_data());
        assert_eq!(s.status, "Good");
        assert!(!s.alert);
        assert!(s.alerts.is_empty());
        assert_eq!(s.temperature_c, Some(38));
        assert_eq!(s.wear_level_percent, Some(12));
    }

    #[test]
    fn test_summarize_nvme_critical_warning() {
        let mut d = nvme_data();
        d.nvme_health.as_mut().unwrap().critical_warning = 0x01; // 温度过高
        d.nvme_health.as_mut().unwrap().media_errors = 5;
        let s = summarize_smart(&d);
        assert_eq!(s.status, "Critical");
        assert!(s.alert);
        assert!(s.alerts.iter().any(|a| a.contains("温度过高")));
        assert!(s.alerts.iter().any(|a| a.contains("媒体错误")));
    }

    #[test]
    fn test_summarize_nvme_wear_warning() {
        let mut d = nvme_data();
        d.nvme_health.as_mut().unwrap().percentage_used = 85;
        let s = summarize_smart(&d);
        assert_eq!(s.status, "Warning");
        assert!(s.alert);
        assert!(s.alerts.iter().any(|a| a.contains("磨损")));
    }

    #[test]
    fn test_summarize_ata_reallocated() {
        let d = DiskSmartData {
            id: "1".to_string(),
            model: "Test HDD".to_string(),
            interface_type: "SATA".to_string(),
            media_type: "HDD".to_string(),
            attributes: vec![SmartAttribute {
                id: 0x05,
                name: "重新分配扇区数".to_string(),
                raw: 200,
                value: 80,
                worst: 80,
                threshold: 0,
                status: "OK".to_string(),
            }],
            nvme_health: None,
            wmi_health: None,
            source: "ioctl".to_string(),
        };
        let s = summarize_smart(&d);
        assert_eq!(s.status, "Critical");
        assert!(s.alerts.iter().any(|a| a.contains("重新分配")));
        assert_eq!(s.reallocated_sectors, Some(200));
    }

    #[test]
    fn test_summarize_ata_failed_status() {
        let d = DiskSmartData {
            id: "2".to_string(),
            model: "Test SSD".to_string(),
            interface_type: "SATA".to_string(),
            media_type: "SSD".to_string(),
            attributes: vec![SmartAttribute {
                id: 0xC6,
                name: "离线不可校正扇区".to_string(),
                raw: 42,
                value: 50,
                worst: 50,
                threshold: 0,
                status: "FAILED".to_string(),
            }],
            nvme_health: None,
            wmi_health: None,
            source: "ioctl".to_string(),
        };
        let s = summarize_smart(&d);
        assert_eq!(s.status, "Critical");
        assert!(s.alert);
    }

    #[test]
    fn test_mock_smart_detail_ids() {
        let good = mock_smart_detail("0");
        assert_eq!(good.health.status, "Good");
        assert!(good.nvme_health.is_some());
        let warning = mock_smart_detail("1");
        assert_eq!(warning.health.status, "Warning");
        let critical = mock_smart_detail("2");
        assert_eq!(critical.health.status, "Critical");
        let missing = mock_smart_detail("9");
        assert_eq!(missing.health.status, "Unknown");
        assert!(missing.error.is_some());
    }

    /// WMI Unknown（5）不应升级为 Warning（v0.20.x 健康状态误判回归测试）
    #[test]
    fn test_wmi_unknown_not_warning() {
        use crate::datasource::disk::WmiHealthInfo;
        let d = DiskSmartData {
            id: "9".to_string(),
            model: "Test".to_string(),
            interface_type: "SATA".to_string(),
            media_type: "HDD".to_string(),
            attributes: Vec::new(),
            nvme_health: None,
            wmi_health: Some(WmiHealthInfo {
                health_status: 5, // Unknown
                temperature_c: None,
                power_on_hours: None,
                read_errors_total: None,
                write_errors_total: None,
            }),
            source: "wmi".to_string(),
        };
        let s = summarize_smart(&d);
        // 未知 ≠ 预警：状态 Unknown 且不告警
        assert_eq!(s.status, "Unknown");
        assert!(!s.alert);
    }

    /// 真机端到端（默认忽略）：list_disks 应给出真实健康摘要（温度/通电时间/状态）
    #[test]
    #[ignore]
    fn real_machine_list_disks_health() {
        let disks = list_disks();
        assert!(!disks.is_empty(), "应枚举到磁盘");
        for d in &disks {
            eprintln!(
                "[真机] disk{} {} {}: status={} label={} alert={} temp={:?} pOH={:?} alerts={:?}",
                d.id,
                d.model,
                d.interface_type,
                d.health.status,
                d.health.label,
                d.health.alert,
                d.health.temperature_c,
                d.health.power_on_hours,
                d.health.alerts
            );
        }
    }
}
