//! 物理磁盘枚举 + S.M.A.R.T 读取 — P14（纯 Win32 IOCTL，零外部命令）
//!
//! 数据源（全链路 Win32 API，无 smartctl / PowerShell 外部进程）：
//! - 磁盘枚举：`\\.\PhysicalDriveN` + `IOCTL_STORAGE_QUERY_PROPERTY`（StorageDeviceProperty）
//!   读取型号 / 序列号 / 总线类型；`StorageDeviceSeekPenaltyProperty` 判断 HDD/SSD；
//!   `IOCTL_DISK_GET_LENGTH_INFO` 读取容量。
//! - NVMe S.M.A.R.T：`IOCTL_STORAGE_QUERY_PROPERTY` + StorageAdapterProtocolSpecificProperty
//!   读取 `NVME_LOG_PAGE_HEALTH_INFO`（临界警告 / 温度 / 可用备用 / 磨损 / 通电时间 / 媒体错误）。
//! - ATA/SATA S.M.A.R.T：`IOCTL_ATA_PASS_THROUGH` + `ATA_PASS_THROUGH_EX` 发送
//!   SMART READ DATA（0xB0/0xD0）读取 512 字节属性表（30 条属性）。
//!
//! 错误语义：
//! - 打开物理盘权限不足（ERROR_ACCESS_DENIED）→ `CollectError::NeedsAdmin`
//! - 总线类型不支持（USB/UFS 等透传不支持的）→ `CollectError::NotFound`（不算失败）
//! - 其余 IOCTL 失败 → `CollectError::WinApi`（带 API 名 + 错误码）
//!
//! 线程模型：全部同步阻塞 API，调用方 MUST 在 `spawn_blocking` 中执行（S8）。

use crate::error::CollectError;
use serde::Serialize;
use std::mem::{size_of, MaybeUninit};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE},
    Storage::{
        FileSystem::{
            FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, BusTypeAta,
            BusTypeNvme, BusTypeSata, BusTypeScsi, BusTypeUsb,
        },
        IscsiDisc::{
            ATA_PASS_THROUGH_EX, ATA_FLAGS_DATA_IN, ATA_FLAGS_DRDY_REQUIRED, IOCTL_ATA_PASS_THROUGH,
        },
        Nvme::{NVME_HEALTH_INFO_LOG, NVME_LOG_PAGE_HEALTH_INFO},
    },
    System::{
        IO::DeviceIoControl,
        Ioctl::{
            DEVICE_SEEK_PENALTY_DESCRIPTOR, GET_LENGTH_INFORMATION, IOCTL_DISK_GET_LENGTH_INFO,
            IOCTL_STORAGE_QUERY_PROPERTY, NVMeDataTypeLogPage, ProtocolTypeNvme,
            STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY, STORAGE_PROTOCOL_DATA_DESCRIPTOR_EXT,
            STORAGE_PROTOCOL_SPECIFIC_DATA_EXT, StorageAdapterProtocolSpecificProperty,
            StorageDeviceProperty, StorageDeviceSeekPenaltyProperty, PropertyStandardQuery,
            STORAGE_QUERY_TYPE, STORAGE_PROPERTY_ID,
        },
    },
};

// ---------------------------------------------------------------------------
// 数据类型（serde Serialize → 前端消费）
// ---------------------------------------------------------------------------

/// 物理磁盘简要信息（GET /api/disks 列表项）
#[derive(Debug, Clone, Serialize)]
pub struct DiskInfo {
    /// 磁盘 id：`\\.\PhysicalDriveN` 中的 N（字符串形式，前端作 key 使用）
    pub id: String,
    /// 型号（ProductId，如 "Samsung SSD 970 EVO Plus 1TB"）
    pub model: String,
    /// 序列号
    pub serial: String,
    /// 总线类型中文标签：NVMe / SATA / ATA / USB / SCSI / 其他
    pub interface_type: String,
    /// 介质类型：HDD / SSD / 未知（由 seek penalty 推断，NVMe 恒为 SSD）
    pub media_type: String,
    /// 容量（字节）
    pub size_bytes: u64,
}

/// S.M.A.R.T 属性条目（详情弹窗逐行渲染）
#[derive(Debug, Clone, Serialize)]
pub struct SmartAttribute {
    /// 属性 id（十进制）
    pub id: u8,
    /// 属性名（中文，如 "重映射扇区数"）
    pub name: String,
    /// 原始值（raw，16 字节大端解析）
    pub raw: u64,
    /// 归一化当前值（0-255，越大越好）
    pub value: u8,
    /// 归一化最差值
    pub worst: u8,
    /// 阈值（0 表示无阈值）
    pub threshold: u8,
    /// 状态：OK / FAILING / FAILED / UNKNOWN
    pub status: String,
}

/// NVMe 健康日志页（NVME_LOG_PAGE_HEALTH_INFO 的精选字段）
#[derive(Debug, Clone, Serialize)]
pub struct NvmeHealthLog {
    /// 临界警告位掩码（bit0 温度过高 / bit1 备用空间不足 / bit2 可靠性降级 / bit3 介质只读 / bit4 易失备份失败）
    pub critical_warning: u8,
    /// 温度（摄氏度，原始开尔文 -273 转换）
    pub temperature_c: u16,
    /// 可用备用百分比
    pub available_spare: u8,
    /// 备用阈值百分比
    pub available_spare_threshold: u8,
    /// 已用寿命百分比（磨损）
    pub percentage_used: u8,
    /// 通电时间（小时）
    pub power_on_hours: u64,
    /// 通电周期
    pub power_cycles: u64,
    /// 不安全关机次数
    pub unsafe_shutdowns: u64,
    /// 媒体错误数
    pub media_errors: u64,
    /// 数据单元读取（512B 单元）
    pub data_units_read: u128,
    /// 数据单元写入（512B 单元）
    pub data_units_written: u128,
}

/// 完整 S.M.A.R.T 数据（GET /api/disks/:id/smart）
#[derive(Debug, Clone, Serialize)]
pub struct DiskSmartData {
    /// 磁盘 id（同 DiskInfo.id）
    pub id: String,
    /// 型号
    pub model: String,
    /// 接口类型标签（同 DiskInfo.interface_type）
    pub interface_type: String,
    /// 介质类型标签（同 DiskInfo.media_type）
    pub media_type: String,
    /// ATA 属性表（HDD/SSD；NVMe 为空数组，健康数据在 nvme_health）
    pub attributes: Vec<SmartAttribute>,
    /// NVMe 健康日志（仅 NVMe；ATA 盘为 None）
    pub nvme_health: Option<NvmeHealthLog>,
    /// WMI 降级健康信息（IOCTL 全量 SMART 不可用时由 MSFT_PhysicalDisk 兜底；
    /// 至少含 HealthStatus 判断字段）
    pub wmi_health: Option<WmiHealthInfo>,
    /// 数据来源：ioctl（完整 SMART）/ wmi（WMI 兜底）
    pub source: String,
}

/// WMI 降级健康信息（MSFT_PhysicalDisk + MSFT_StorageReliabilityCounter）
#[derive(Debug, Clone, Serialize)]
pub struct WmiHealthInfo {
    /// HealthStatus 枚举：0=Healthy 1=Warning 2=Unhealthy 5=Unknown
    pub health_status: u8,
    /// 温度（摄氏度，None = 无数据）
    pub temperature_c: Option<u16>,
    /// 通电时间（小时，None = 无数据）
    pub power_on_hours: Option<u64>,
    /// 读取错误总数（None = 无数据）
    pub read_errors_total: Option<u64>,
    /// 写入错误总数（None = 无数据）
    pub write_errors_total: Option<u64>,
}

/// 打开的物理磁盘句柄（RAII：Drop 时 CloseHandle）
struct PhysicalDrive {
    handle: HANDLE,
    index: u32,
}

impl Drop for PhysicalDrive {
    fn drop(&mut self) {
        if self.handle != INVALID_HANDLE_VALUE {
            // SAFETY: CloseHandle 对有效句柄安全；INVALID 已排除
            unsafe { CloseHandle(self.handle) };
        }
    }
}

impl PhysicalDrive {
    /// 打开 `\\.\PhysicalDriveN`
    ///
    /// 打开失败返回错误（权限不足 → NeedsAdmin；不存在 → NotFound）。
    fn open(index: u32) -> Result<Self, CollectError> {
        let path = format!("\\\\.\\PhysicalDrive{}", index);
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

        // SAFETY: CreateFileW 宽字符串以 NUL 结尾；dwDesiredAccess=GENERIC_READ|GENERIC_WRITE，
        // SMART 透传（IOCTL_ATA_PASS_THROUGH / 协议特定查询）要求读写句柄，只读打开会返回
        // ERROR_ACCESS_DENIED / ERROR_INVALID_PARAMETER（v0.20.x 全盘 IOCTL 失败根因之一）；
        // 共享模式允许其他进程读写（SMART 查询不应独占磁盘），OPEN_EXISTING 不创建文件。
        let handle = unsafe {
            windows_sys::Win32::Storage::FileSystem::CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };

        if handle == INVALID_HANDLE_VALUE {
            let code = last_error_code();
            if code == 5 {
                // ERROR_ACCESS_DENIED：管理员权限不足
                return Err(CollectError::NeedsAdmin {
                    op: format!("打开物理磁盘 {}（路径 {}）", index, path),
                });
            }
            if code == 2 || code == 3 {
                // ERROR_FILE_NOT_FOUND / ERROR_PATH_NOT_FOUND：盘号不存在
                return Err(CollectError::NotFound {
                    what: format!("物理磁盘 {}（路径 {}）", index, path),
                });
            }
            return Err(CollectError::winapi_detailed(
                "CreateFileW",
                format!("打开物理磁盘 {}", index),
                format!("错误码 {}，路径 {}", code, path),
            ));
        }

        Ok(Self { handle, index })
    }

    /// 执行 IOCTL_STORAGE_QUERY_PROPERTY，输出为 T（固定大小结构）
    fn query_property<T>(&self, property: STORAGE_PROPERTY_ID) -> Result<T, CollectError> {
        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: property,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0; 1],
        };

        let mut out: MaybeUninit<T> = MaybeUninit::uninit();
        let mut returned: u32 = 0;

        // SAFETY: query 为栈上初始化结构；out 缓冲区大小 = size_of::<T>() 足够容纳
        // 标准属性描述符；DeviceIoControl 返回 FALSE 时 out 未写入（忽略内容）。
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                &query as *const _ as *const core::ffi::c_void,
                size_of::<STORAGE_PROPERTY_QUERY>() as u32,
                out.as_mut_ptr() as *mut core::ffi::c_void,
                size_of::<T>() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };

        if ok == 0 {
            return Err(CollectError::winapi(
                "IOCTL_STORAGE_QUERY_PROPERTY",
                format!("查询磁盘 {} 属性 {:?}", self.index, property),
            ));
        }

        // SAFETY: DeviceIoControl 成功且缓冲区大小正确，out 已完全初始化
        Ok(unsafe { out.assume_init() })
    }
}

// ---------------------------------------------------------------------------
// 公共 API
// ---------------------------------------------------------------------------

/// 枚举系统全部物理磁盘（`\\.\PhysicalDrive0..N` 逐个尝试）
///
/// - 盘号不存在（NotFound）→ 停止枚举（物理盘号连续，缺号即枚举结束）
/// - 单块盘打开失败（权限/IO 错误）→ 记日志跳过，不拖垮整表
pub fn enumerate_disks() -> Vec<DiskInfo> {
    let mut disks: Vec<DiskInfo> = Vec::new();
    for index in 0..64u32 {
        match PhysicalDrive::open(index) {
            Ok(handle) => match read_disk_info(&handle) {
                Ok(info) => disks.push(info),
                Err(e) => log::warn!(
                    "disk.enumerate: 磁盘 {} 信息读取失败: {}",
                    index,
                    e
                ),
            },
            Err(CollectError::NotFound { .. }) => break,
            Err(e) => {
                log::warn!("disk.enumerate: 磁盘 {} 打开失败: {}", index, e);
                // 权限不足等错误：继续尝试后续盘号
                continue;
            }
        }
    }
    // 普通权限 / IOCTL 全失败 → 列表为空：WMI 兜底枚举（保证磁盘列表可见，
    // 修复分发机器「磁盘信息读不到」问题——SMART 详情仍走 WMI 兜底链）
    if disks.is_empty() {
        let wmi_disks = wmi_enumerate_disks();
        if !wmi_disks.is_empty() {
            log::warn!("disk.enumerate: IOCTL 枚举为空（权限不足？），使用 WMI 兜底枚举 {} 块磁盘", wmi_disks.len());
            return wmi_disks;
        }
    }
    disks
}


/// WMI 兜底枚举磁盘（IOCTL 权限不足/不可用时）：
/// 1. Storage 命名空间 MSFT_PhysicalDisk（含 BusType/MediaType）
/// 2. root\CIMV2 Win32_DiskDrive（无 Storage 服务兜底）
fn wmi_enumerate_disks() -> Vec<DiskInfo> {
    let mut out: Vec<DiskInfo> = Vec::new();
    // 1) MSFT_PhysicalDisk
    if let Ok(conn) = wmi::WMIConnection::with_namespace_path("root\\Microsoft\\Windows\\Storage") {
        let wql = "SELECT DeviceId,HealthStatus,FriendlyName,MediaType,BusType FROM MSFT_PhysicalDisk";
        if let Ok(disks) = conn.raw_query::<WmiPhysicalDisk>(wql) {
            for d in disks {
                if let Some(id) = d.DeviceId {
                    out.push(DiskInfo {
                        id: id.clone(),
                        model: d.FriendlyName.clone().unwrap_or_default(),
                        serial: String::new(),
                        interface_type: wmi_bus_type_label(d.BusType),
                        media_type: wmi_media_type_label(d.MediaType),
                        size_bytes: 0, // MSFT_PhysicalDisk 无容量字段（列表按型号/接口展示）
                    });
                }
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    // 2) Win32_DiskDrive（含容量与序列号）
    if let Ok(conn) = wmi::WMIConnection::new() {
        let wql = "SELECT Index,Status,Model,SerialNumber,InterfaceType,MediaType,Size FROM Win32_DiskDrive";
        if let Ok(disks) = conn.raw_query::<WmiWin32DiskDrive>(wql) {
            for d in disks {
                if let Some(idx) = d.Index {
                    let is_fixed = d.MediaType.as_deref() == Some("Fixed hard disk media");
                    out.push(DiskInfo {
                        id: idx.to_string(),
                        model: d.Model.clone().unwrap_or_default(),
                        serial: d.SerialNumber.clone().unwrap_or_default(),
                        interface_type: win32_interface_label(d.InterfaceType.as_deref()),
                        media_type: if is_fixed { "HDD".to_string() } else { "SSD".to_string() },
                        size_bytes: d.Size.unwrap_or(0),
                    });
                }
            }
        }
    }
    out
}

/// 读取指定磁盘的完整 S.M.A.R.T 数据
///
/// 三级降级链：
/// 1. IOCTL 全量采集（NVMe 健康日志 / ATA 属性表）— 真实物理机
/// 2. WMI 兜底（MSFT_PhysicalDisk.HealthStatus + StorageReliabilityCounter）—
///    虚拟化环境 / 驱动拒绝透传时仍有健康状态判断字段
/// 3. 仍失败 → `Err`（权限不足/总线不支持，前端显示可读错误）
///
/// 返回体 `source` 标记数据来源（"ioctl"/"wmi"），前端可提示数据完整性。
pub fn read_smart(disk_id: &str) -> Result<DiskSmartData, CollectError> {
    let index: u32 = disk_id
        .parse()
        .map_err(|_| CollectError::parse("磁盘 id", format!("'{}' 不是合法盘号", disk_id)))?;
    let drive = match PhysicalDrive::open(index) {
        Ok(d) => d,
        Err(open_err) => {
            // 打开失败（权限不足等）：尝试 WMI 兜底（不需要物理盘句柄）
            return wmi_fallback(disk_id, open_err);
        }
    };
    let info = match read_disk_info(&drive) {
        Ok(i) => i,
        Err(e) => return wmi_fallback(disk_id, e),
    };

    let ioctl_result = match info.interface_type.as_str() {
        "NVMe" => read_nvme_health(&drive).map(|health| DiskSmartData {
            id: info.id,
            model: info.model,
            interface_type: info.interface_type,
            media_type: info.media_type,
            attributes: Vec::new(),
            nvme_health: Some(health),
            wmi_health: None,
            source: "ioctl".to_string(),
        }),
        "SATA" | "ATA" => read_ata_attributes(&drive).map(|attributes| DiskSmartData {
            id: info.id,
            model: info.model,
            interface_type: info.interface_type,
            media_type: info.media_type,
            attributes,
            nvme_health: None,
            wmi_health: None,
            source: "ioctl".to_string(),
        }),
        other => Err(CollectError::NotFound {
            what: format!("磁盘 {}（总线类型 {}）的 S.M.A.R.T 透传", info.id, other),
        }),
    };

    match ioctl_result {
        Ok(data) => Ok(data),
        Err(e) => wmi_fallback(disk_id, e),
    }
}

/// WMI 兜底结果：健康信息 + 型号/接口/介质（一次查询带回，避免二次 WMI 调用）
struct WmiFallbackData {
    health: WmiHealthInfo,
    model: String,
    interface_type: String,
    media_type: String,
}

/// WMI 兜底：IOCTL 失败时从 MSFT_PhysicalDisk 获取健康状态
///
/// 该路径不依赖物理盘句柄，虚拟化环境（Hyper-V/VMware）下仍可用；
/// 返回 `source="wmi"`，`attributes/nvme_health` 为空。
fn wmi_fallback(disk_id: &str, ioctl_err: CollectError) -> Result<DiskSmartData, CollectError> {
    match wmi_health_info(disk_id) {
        Ok(Some(fb)) => {
            log::warn!(
                "disk.smart: 磁盘 {} IOCTL 采集失败，使用 WMI 兜底: {}",
                disk_id,
                ioctl_err
            );
            Ok(DiskSmartData {
                id: disk_id.to_string(),
                model: fb.model,
                interface_type: fb.interface_type,
                media_type: fb.media_type,
                attributes: Vec::new(),
                nvme_health: None,
                wmi_health: Some(fb.health),
                source: "wmi".to_string(),
            })
        }
        Ok(None) => Err(ioctl_err),
        Err(wmi_err) => {
            log::warn!("disk.smart: 磁盘 {} WMI 兜底失败: {}", disk_id, wmi_err);
            Err(ioctl_err)
        }
    }
}

// ---------------------------------------------------------------------------
// 磁盘基本信息读取
// ---------------------------------------------------------------------------

/// 读取磁盘型号 / 序列号 / 总线 / 容量
fn read_disk_info(drive: &PhysicalDrive) -> Result<DiskInfo, CollectError> {
    // StorageDeviceProperty 返回 STORAGE_DEVICE_DESCRIPTOR + RawDeviceProperties
    // （厂商/型号/序列号 UTF-16 字符串紧随结构体，由偏移量定位），需原始字节缓冲
    let raw = query_device_descriptor_buffer(drive)?;

    // SAFETY: IOCTL 成功且缓冲 >= size_of::<STORAGE_DEVICE_DESCRIPTOR>()，
    // 头部为合法描述符结构
    let desc = unsafe { &*(raw.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR) };
    let model = parse_descriptor_string(&raw, desc.SerialNumberOffset != 0, desc.ProductIdOffset);
    let serial = parse_descriptor_string(&raw, desc.SerialNumberOffset != 0, desc.SerialNumberOffset);

    // 接口类型映射（STORAGE_BUS_TYPE 为 i32 别名，常量直接比较）
    let interface_type = if desc.BusType == BusTypeNvme {
        "NVMe".to_string()
    } else if desc.BusType == BusTypeSata {
        "SATA".to_string()
    } else if desc.BusType == BusTypeAta {
        "ATA".to_string()
    } else if desc.BusType == BusTypeUsb {
        "USB".to_string()
    } else if desc.BusType == BusTypeScsi {
        "SCSI".to_string()
    } else {
        format!("其他({})", desc.BusType)
    };

    // 介质类型：seek penalty=true → HDD；false → SSD；NVMe 恒为 SSD；查询失败 → 未知
    let media_type = if interface_type == "NVMe" {
        "SSD".to_string()
    } else {
        match drive.query_property::<DEVICE_SEEK_PENALTY_DESCRIPTOR>(StorageDeviceSeekPenaltyProperty)
        {
            Ok(d) => {
                if d.IncursSeekPenalty != 0 {
                    "HDD".to_string()
                } else {
                    "SSD".to_string()
                }
            }
            Err(_) => "未知".to_string(),
        }
    };

    // 容量（IOCTL_DISK_GET_LENGTH_INFO）
    let size_bytes = drive.disk_length().unwrap_or(0);

    Ok(DiskInfo {
        id: drive.index.to_string(),
        model,
        serial,
        interface_type,
        media_type,
        size_bytes,
    })
}

/// 查询 StorageDeviceProperty 并返回完整原始缓冲（描述符 + 字符串区）
fn query_device_descriptor_buffer(drive: &PhysicalDrive) -> Result<Vec<u8>, CollectError> {
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0; 1],
    };

    // 描述符 + 字符串区预留 512 字节（型号/序列号一般 < 40 字符）
    let mut out = vec![0u8; 512];
    let mut returned: u32 = 0;

    // SAFETY: query 为栈上初始化结构；out 缓冲区足够大；DeviceIoControl 失败时内容无效
    let ok = unsafe {
        DeviceIoControl(
            drive.handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &query as *const _ as *const core::ffi::c_void,
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            out.as_mut_ptr() as *mut core::ffi::c_void,
            out.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };

    if ok == 0 {
        return Err(CollectError::winapi(
            "IOCTL_STORAGE_QUERY_PROPERTY",
            format!("查询磁盘 {} 设备描述符", drive.index),
        ));
    }

    out.truncate(returned as usize);
    Ok(out)
}

/// 从 STORAGE_DEVICE_DESCRIPTOR 原始缓冲中按偏移解析字符串
///
/// 注意：Windows 存储描述符的 VendorId/ProductId/SerialNumber 均为
/// **单字节 ANSI/OEM 字符串**（非 UTF-16，实测验证：raw bytes 为 ASCII 序列），
/// 以 NUL 结尾。`field_offset==0` 表示字段不存在。最大 128 字符防越界。
fn parse_descriptor_string(raw: &[u8], _has_serial: bool, field_offset: u32) -> String {
    if field_offset == 0 {
        return String::new();
    }
    let start = field_offset as usize;
    if start >= raw.len() {
        return String::new();
    }
    let mut end = start;
    while end < raw.len() && raw[end] != 0 && end - start < 128 {
        end += 1;
    }
    // 单字节 ANSI：GBK 区域字符用 lossy 转换，ASCII 直接通过
    String::from_utf8_lossy(&raw[start..end]).trim().to_string()
}

impl PhysicalDrive {
    /// 读取磁盘容量（字节）
    fn disk_length(&self) -> Result<u64, CollectError> {
        let mut info: MaybeUninit<GET_LENGTH_INFORMATION> = MaybeUninit::uninit();
        let mut returned: u32 = 0;

        // SAFETY: 输出缓冲大小 = size_of::<GET_LENGTH_INFORMATION>()；IOCTL 失败时忽略内容
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_DISK_GET_LENGTH_INFO,
                std::ptr::null(),
                0,
                info.as_mut_ptr() as *mut core::ffi::c_void,
                size_of::<GET_LENGTH_INFORMATION>() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };

        if ok == 0 {
            return Err(CollectError::winapi(
                "IOCTL_DISK_GET_LENGTH_INFO",
                format!("读取磁盘 {} 容量", self.index),
            ));
        }

        // SAFETY: IOCTL 成功，info 已初始化
        let info = unsafe { info.assume_init() };
        Ok(info.Length as u64)
    }
}

// ---------------------------------------------------------------------------
// NVMe S.M.A.R.T 健康日志读取
// ---------------------------------------------------------------------------

/// 查询缓冲：STORAGE_PROPERTY_QUERY（PropertyId/QueryType/AdditionalParameters）+ STORAGE_PROTOCOL_SPECIFIC_DATA_EXT
///
/// 协议数据必须紧随 STORAGE_PROPERTY_QUERY 结构之后（偏移 = sizeof(STORAGE_PROPERTY_QUERY) = 12，
/// 微软官方布局；此前放在偏移 8（AdditionalParameters 处）导致 stornvme 解析错位 →
/// ERROR_INVALID_PARAMETER (87)）。
#[repr(C)]
struct NvmeSmartQuery {
    property_id: STORAGE_PROPERTY_ID,
    query_type: STORAGE_QUERY_TYPE,
    /// AdditionalParameters 占位（含结构尾部 padding，共 4 字节）
    _additional: [u8; 4],
    protocol: STORAGE_PROTOCOL_SPECIFIC_DATA_EXT,
}

/// 读取 NVMe 健康日志页（NVME_LOG_PAGE_HEALTH_INFO）
fn read_nvme_health(drive: &PhysicalDrive) -> Result<NvmeHealthLog, CollectError> {
    let query = NvmeSmartQuery {
        property_id: StorageAdapterProtocolSpecificProperty,
        query_type: PropertyStandardQuery,
        // AdditionalParameters 占位（结构尾部 padding 随对齐自动为零）
        _additional: [0; 4],
        protocol: STORAGE_PROTOCOL_SPECIFIC_DATA_EXT {
            ProtocolType: ProtocolTypeNvme,
            DataType: NVMeDataTypeLogPage as u32,
            ProtocolDataValue: NVME_LOG_PAGE_HEALTH_INFO as u32,
            ProtocolDataSubValue: 0,
            // 数据偏移从**输出缓冲起点**（descriptor 头）算起：
            // 必须为 descriptor 结构总大小（头 8B + EXT 64B = 72B）；
            // 误用 EXT 大小（64B）会落在 ProtocolSpecificData 内部，
            // 驱动返回 ERROR_INVALID_FUNCTION（v0.20.x 全盘 IOCTL 失败根因）
            ProtocolDataOffset: size_of::<STORAGE_PROTOCOL_DATA_DESCRIPTOR_EXT>() as u32,
            ProtocolDataLength: size_of::<NVME_HEALTH_INFO_LOG>() as u32,
            // FixedProtocolReturnData=1：固定大小返回（部分 stornvme 版本要求）
            FixedProtocolReturnData: 1,
            ProtocolDataSubValue2: 0,
            ProtocolDataSubValue3: 0,
            ProtocolDataSubValue4: 0,
            ProtocolDataSubValue5: 0,
            Reserved: [0; 5],
        },
    };

    // 输出缓冲：descriptor 头 + 512 字节日志数据
    let mut out = [0u8; size_of::<STORAGE_PROTOCOL_DATA_DESCRIPTOR_EXT>() + 512];
    let mut returned: u32 = 0;

    // SAFETY: query 布局正确（属性 49 + 标准查询 + 协议扩展数据）；out 缓冲区足够大；
    // 输入/输出均为栈上连续内存。
    let ok = unsafe {
        DeviceIoControl(
            drive.handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &query as *const _ as *const core::ffi::c_void,
            size_of::<NvmeSmartQuery>() as u32,
            out.as_mut_ptr() as *mut core::ffi::c_void,
            out.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };

    if ok == 0 {
        let code = last_error_code();
        if code == 5 {
            return Err(CollectError::NeedsAdmin {
                op: format!("读取磁盘 {} 的 NVMe 健康日志", drive.index),
            });
        }
        return Err(CollectError::winapi_detailed(
            "IOCTL_STORAGE_QUERY_PROPERTY(NVMe)",
            format!("读取磁盘 {} 健康日志", drive.index),
            format!("错误码 {}", code),
        ));
    }

    // 解析返回的 STORAGE_PROTOCOL_DATA_DESCRIPTOR_EXT 头，拿日志页偏移
    // SAFETY: out 缓冲区前部为 descriptor 结构，大小足够
    let desc = unsafe { &*(out.as_ptr() as *const STORAGE_PROTOCOL_DATA_DESCRIPTOR_EXT) };
    let data_offset = desc.ProtocolSpecificData.ProtocolDataOffset as usize;
    if data_offset + size_of::<NVME_HEALTH_INFO_LOG>() > out.len() {
        return Err(CollectError::parse(
            "NVMe 健康日志",
            format!("返回数据偏移越界（offset={}, buf={}）", data_offset, out.len()),
        ));
    }

    // SAFETY: 偏移+大小已校验在缓冲内，且 IOCTL 返回的日志页为合法 NVME_HEALTH_INFO_LOG 布局
    let log = unsafe { &*(out.as_ptr().add(data_offset) as *const NVME_HEALTH_INFO_LOG) };

    Ok(NvmeHealthLog {
        critical_warning: unsafe { log.CriticalWarning.AsUchar },
        // Temperature 为小端 u16（NVMe 规范所有多字节字段均为 little-endian），
        // 原始单位开尔文 → 摄氏度；曾误用大端解析导致温度读成荒谬值（0x013E→15873K）
        temperature_c: u16::from_le_bytes(log.Temperature).saturating_sub(273),
        available_spare: log.AvailableSpare,
        available_spare_threshold: log.AvailableSpareThreshold,
        percentage_used: log.PercentageUsed,
        power_on_hours: u128_le(&log.PowerOnHours) as u64,
        power_cycles: u128_le(&log.PowerCycle) as u64,
        unsafe_shutdowns: u128_le(&log.UnsafeShutdowns) as u64,
        media_errors: u128_le(&log.MediaErrors) as u64,
        data_units_read: u128_le(&log.DataUnitRead),
        data_units_written: u128_le(&log.DataUnitWritten),
    })
}

/// 解析 NVMe 日志页中的 16 字节小端整数
fn u128_le(bytes: &[u8; 16]) -> u128 {
    let mut buf = [0u8; 16];
    buf.copy_from_slice(bytes);
    u128::from_le_bytes(buf)
}

// ---------------------------------------------------------------------------
// ATA / SATA S.M.A.R.T 属性读取
// ---------------------------------------------------------------------------

/// SMART 属性 id → 中文名映射（对齐 CrystalDiskInfo 属性名表）
fn attribute_name(id: u8) -> &'static str {
    match id {
        // ── 标准 ATA 属性（CDI 通用表） ──
        0x01 => "读取错误率",
        0x02 => "吞吐量性能",
        0x03 => "主轴起转时间",
        0x04 => "起转/停止次数",
        0x05 => "重新分配扇区数",
        0x06 => "读取通道余量",
        0x07 => "寻道错误率",
        0x08 => "寻道时间性能",
        0x09 => "通电时间",
        0x0A => "旋转重试次数",
        0x0B => "校准重试次数",
        0x0C => "通电周期计数",
        0x0D => "软读取错误率",
        0xAA => "可用备用空间",
        0xAB => "备用空间阈值",
        0xAC => "寿命已用百分比",
        0xAD => "平均擦除计数",
        0xAE => "意外断电计数",
        // 0xE9 = Intel 介质磨损指示器（后期固件；value 即剩余寿命健康度 %，
        // 与 CDI 显示一致：当前值 99/最差值 99）。
        // 0xAF 早期固件曾作磨损指示，后期固件为废弃字段（value 恒 100、raw 无意义），
        // CDI 亦不显示其磨损含义——保持「未知属性」避免同名干扰。
        0xB1 => "磨损范围计数",
        0xB7 => "SATA 降速错误计数",
        0xB8 => "端到端错误检测",
        0xB9 => "磁头稳定性",
        0xBA => "感应振荡检测",
        0xBB => "报告不可校正错误",
        0xBC => "命令超时",
        0xBD => "高飞写错误",
        0xBE => "气流温度",
        0xBF => "G 传感器错误率",
        0xC0 => "断电返回计数",
        0xC1 => "加载/卸载周期计数",
        0xC2 => "温度",
        0xC3 => "硬件 ECC 恢复",
        0xC4 => "重新分配事件计数",
        0xC5 => "当前待定扇区数",
        0xC6 => "无法校正的扇区数",
        0xC7 => "UDMA CRC 错误计数",
        0xC8 => "多字错误率",
        0xC9 => "写入错误率",
        0xCA => "软读取错误率",
        0xCB => "数据地址标记错误",
        0xCC => "运行异常取消",
        0xCD => "软 ECC 校正",
        0xCE => "热颤动错误率",
        0xCF => "飞行高度",
        0xD0 => "写入时振动",
        0xD1 => "运行时间错误率",
        0xDC => "盘片偏移",
        0xDD => "G 传感器错误率",
        0xDE => "加载时间",
        0xDF => "加载/卸载重试次数",
        0xE0 => "负载摩擦",
        0xE1 => "总主机写入",
        0xE2 => "总主机读取",
        0xE6 => "飞行高度",
        0xE7 => "SSD 剩余寿命",
        0xE8 => "可用备用空间（SSD）",
        0xE9 => "介质磨损指示器",
        0xEA => "平均擦除计数",
        0xEB => "优良块数",
        0xEC => "意外断电",
        0xED => "平均擦写计数",
        0xEE => "坏块数",
        0xF0 => "飞行时间",
        0xF1 => "总主机写入（SSD）",
        0xF2 => "总主机读取（SSD）",
        0xF3 => "NAND 写入",
        0xF4 => "NAND 读取",
        0xF5 => "非 4K 对齐访问",
        0xF6 => "读命令数",
        0xF7 => "写命令数",
        0xF8 => "错误记录",
        0xF9 => "磨损平均计数",
        0xFA => "平均写入计数",
        0xFB => "最小擦除计数",
        0xFC => "最大擦除计数",
        0xFD => "平均擦除计数",
        0xFE => "已用保留块计数",
        0xFF => "备用块总数",
        _ => "未知属性",
    }
}

/// 通过 IOCTL_ATA_PASS_THROUGH 发送 SMART READ DATA（0xB0/0xD0）
fn read_ata_attributes(drive: &PhysicalDrive) -> Result<Vec<SmartAttribute>, CollectError> {
    // 1) READ DATA（0xD0）：512 字节属性表
    let data = ata_smart_read(drive, 0xD0)?;
    // 2) READ THRESHOLDS（0xD1）：阈值表；失败不阻断（status 置 UNKNOWN，不误报）
    let thresholds = match ata_smart_read(drive, 0xD1) {
        Ok(t) => Some(t),
        Err(e) => {
            log::debug!("disk.smart: 磁盘 {} 读取 SMART 阈值表失败（状态将置 UNKNOWN）: {}", drive.index, e);
            None
        }
    };
    parse_ata_attributes(&data, thresholds.as_ref().map(|t| &t[..]))
}

/// 发送一次 SMART 命令并返回 512 字节数据（feature：0xD0=READ DATA / 0xD1=READ THRESHOLDS）
fn ata_smart_read(drive: &PhysicalDrive, feature: u8) -> Result<[u8; 512], CollectError> {
    // 命令块：command=0xB0(SMART) feature=READ DATA/THRESHOLDS count=1 LBA=0x0000C24F device=0xA0
    let mut apt = ATA_PASS_THROUGH_EX {
        Length: size_of::<ATA_PASS_THROUGH_EX>() as u16,
        AtaFlags: (ATA_FLAGS_DRDY_REQUIRED | ATA_FLAGS_DATA_IN) as u16,
        PathId: 0,
        TargetId: 0,
        Lun: 0,
        ReservedAsUchar: 0,
        DataTransferLength: 512,
        TimeOutValue: 5,
        ReservedAsUlong: 0,
        DataBufferOffset: size_of::<ATA_PASS_THROUGH_EX>() as usize,
        PreviousTaskFile: [0; 8],
        CurrentTaskFile: [0; 8],
    };
    // CurrentTaskFile 布局（ATA 寄存器顺序）：
    // [0]=Error/Features [1]=SectorCount [2]=LBALow [3]=LBAMid [4]=LBAHigh [5]=Device [6]=Command [7]=Reserved
    apt.CurrentTaskFile[0] = feature; // Features: SMART READ DATA(0xD0) / THRESHOLDS(0xD1)
    apt.CurrentTaskFile[1] = 0x01; // SectorCount: 1 扇区
    apt.CurrentTaskFile[2] = 0x4F; // LBA Low (SMART signature 低字节)
    apt.CurrentTaskFile[3] = 0xC2; // LBA Mid (SMART signature)
    apt.CurrentTaskFile[4] = 0x00; // LBA High
    apt.CurrentTaskFile[5] = 0xA0; // Device
    apt.CurrentTaskFile[6] = 0xB0; // Command: SMART

    // 输出缓冲：ATA_PASS_THROUGH_EX + 512 字节数据
    let mut out = [0u8; size_of::<ATA_PASS_THROUGH_EX>() + 512];
    let mut returned: u32 = 0;

    // SAFETY: apt 为栈上初始化结构；out 缓冲尾部 512 字节为数据区；
    // DataBufferOffset 指向 out 中数据区起始。
    let ok = unsafe {
        DeviceIoControl(
            drive.handle,
            IOCTL_ATA_PASS_THROUGH,
            &apt as *const _ as *const core::ffi::c_void,
            size_of::<ATA_PASS_THROUGH_EX>() as u32,
            out.as_mut_ptr() as *mut core::ffi::c_void,
            out.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };

    if ok == 0 {
        let code = last_error_code();
        if code == 5 {
            return Err(CollectError::NeedsAdmin {
                op: format!("读取磁盘 {} 的 ATA S.M.A.R.T 属性", drive.index),
            });
        }
        return Err(CollectError::winapi_detailed(
            "IOCTL_ATA_PASS_THROUGH",
            format!("读取磁盘 {} S.M.A.R.T 属性", drive.index),
            format!("错误码 {}", code),
        ));
    }

    // 属性表布局：512 字节 = 2B revision + 30 × 12B 属性 + 1B checksum
    let data_start = size_of::<ATA_PASS_THROUGH_EX>();
    let mut table = [0u8; 512];
    table.copy_from_slice(&out[data_start..data_start + 512]);
    Ok(table)
}

/// 解析 ATA S.M.A.R.T 属性表（READ DATA）与阈值表（READ THRESHOLDS）
///
/// 每条属性 12 字节：id(1) flags(2) current(1) worst(1) raw(6, 小端)。
/// 阈值表每条 12 字节：id(1) flags(2) threshold(1) reserved(8)。
///
/// ⚠ 状态判定：ATA 属性 flags 的 bit0=pre-failure 类型位、bit1=在线采集位，
/// **不代表当前已失败**；曾误把 bit1 当失败位导致全部属性标 FAILED、
/// 所有盘误报「严重」（v0.20.x 健康状态错误根因）。正确判定：
/// `value <= threshold`（阈值 >0 时）；阈值表不可用时标 UNKNOWN（不误报）。
fn parse_ata_attributes(
    table: &[u8],
    thresholds: Option<&[u8]>,
) -> Result<Vec<SmartAttribute>, CollectError> {
    if table.len() < 362 {
        return Err(CollectError::parse(
            "ATA S.M.A.R.T 属性表",
            format!("长度不足（{} < 362）", table.len()),
        ));
    }
    let mut attrs: Vec<SmartAttribute> = Vec::new();
    // 属性区：offset 2 起，最多 30 条，每条 12 字节
    for i in 0..30 {
        let base = 2 + i * 12;
        let id = table[base];
        if id == 0 {
            continue;
        }
        let flags = u16::from_le_bytes([table[base + 1], table[base + 2]]);
        let value = table[base + 3];
        let worst = table[base + 4];
        let raw = u64::from_le_bytes([
            table[base + 5],
            table[base + 6],
            table[base + 7],
            table[base + 8],
            table[base + 9],
            table[base + 10],
            0,
            0,
        ]);
        let _ = flags; // flags 仅作采集类型说明，不参与失败判定（见模块注释）

        // 阈值判定（阈值表与属性表同布局，threshold 在第 4 字节）
        let threshold = thresholds
            .filter(|t| t.len() >= base + 4)
            .map(|t| t[base + 3])
            .unwrap_or(0);
        // 有阈值：value<=threshold 明确失败；无阈值（Intel 企业盘阈值表全 0）：
        // 不判定失败（避免误报），健康由业务层按 raw 值判断（disk_info.rs）
        let status = if threshold > 0 && value <= threshold {
            "FAILED".to_string()
        } else {
            "OK".to_string()
        };

        attrs.push(SmartAttribute {
            id,
            name: attribute_name(id).to_string(),
            raw,
            value,
            worst,
            threshold,
            status,
        });
    }
    Ok(attrs)
}

// ---------------------------------------------------------------------------
// WMI 兜底（MSFT_PhysicalDisk / MSFT_StorageReliabilityCounter）
// ---------------------------------------------------------------------------

/// WMI 查询结果：MSFT_PhysicalDisk（root\Microsoft\Windows\Storage 命名空间）
/// 字段名与 WMI 属性 PascalCase 一致，故禁用 snake_case lint
#[derive(serde::Deserialize)]
#[allow(non_snake_case)]
struct WmiPhysicalDisk {
    /// 盘号（DeviceId，字符串；与 WHERE 子句匹配用）
    #[serde(default)]
    DeviceId: Option<String>,
    /// HealthStatus：0=Healthy 1=Warning 2=Unhealthy 5=Unknown
    #[serde(default)]
    HealthStatus: Option<u16>,
    /// 型号（FriendlyName）
    #[serde(default)]
    FriendlyName: Option<String>,
    /// 介质类型：3=HDD 4=SSD
    #[serde(default)]
    MediaType: Option<u16>,
    /// 总线类型：11=SATA 17=NVMe 7=USB 1=SCSI 3=ATA
    #[serde(default)]
    BusType: Option<u16>,
}

/// WMI 查询结果：MSFT_StorageReliabilityCounter（可靠性计数器，尽力读取）
#[derive(serde::Deserialize)]
#[allow(non_snake_case)]
struct WmiReliabilityCounter {
    /// 温度（摄氏度）
    #[serde(default)]
    Temperature: Option<u16>,
    /// 通电时间（小时）
    #[serde(default)]
    PowerOnHours: Option<u64>,
    #[serde(default)]
    ReadErrorsTotal: Option<u64>,
    #[serde(default)]
    WriteErrorsTotal: Option<u64>,
}

/// WMI 查询结果：Win32_DiskDrive（root\CIMV2 命名空间，无 Storage 服务依赖）
/// — 分发机器 Storage WMI 不可用时的二级兜底（仅健康状态 + 基本信息）
#[derive(serde::Deserialize)]
#[allow(non_snake_case)]
struct WmiWin32DiskDrive {
    /// 磁盘索引（与 \\.\PhysicalDriveN 对应）
    #[serde(default)]
    Index: Option<u32>,
    /// 状态字符串："OK" / "Degraded" / "Pred Fail" / "Error"
    #[serde(default)]
    Status: Option<String>,
    #[serde(default)]
    Model: Option<String>,
    #[serde(default)]
    SerialNumber: Option<String>,
    /// "IDE" / "SCSI" / "USB" / "RAID"（NVMe 在 Win32_DiskDrive 下通常显示 SCSI）
    #[serde(default)]
    InterfaceType: Option<String>,
    #[serde(default)]
    MediaType: Option<String>,
    #[serde(default)]
    Size: Option<u64>,
}

/// Win32_DiskDrive.Status → WmiHealthInfo.health_status 映射
fn win32_status_to_health(status: Option<&str>) -> u8 {
    match status.unwrap_or("").to_ascii_lowercase().as_str() {
        "ok" => 0,          // Healthy
        "degraded" => 1,    // Warning
        "pred fail" | "error" | "failed" => 2, // Unhealthy
        _ => 5,             // Unknown
    }
}

/// Win32_DiskDrive.InterfaceType → 中文标签
fn win32_interface_label(t: Option<&str>) -> String {
    match t.unwrap_or("").to_ascii_uppercase().as_str() {
        "IDE" => "ATA".to_string(),
        "SCSI" | "SAS" => "SCSI".to_string(),
        "USB" => "USB".to_string(),
        "RAID" => "RAID".to_string(),
        other if other.is_empty() => String::new(),
        other => format!("其他({})", other),
    }
}


/// 查询单块盘的 WMI 健康信息（IOCTL 不可用时的兜底）
///
/// - MSFT_PhysicalDisk 存在 → `Ok(Some(WmiFallbackData))`（至少含 HealthStatus + 型号）
/// - 盘不存在 / WMI 不可用 → `Ok(None)`（上层透传原 IOCTL 错误）
/// - 连接失败 → `Err`（上层记日志）
fn wmi_health_info(disk_id: &str) -> Result<Option<WmiFallbackData>, CollectError> {
    // 第一优先：Storage 命名空间（MSFT_PhysicalDisk 完整健康状态）
    if let Ok(conn) = wmi::WMIConnection::with_namespace_path("root\\Microsoft\\Windows\\Storage") {
        // 1a) WHERE 精确查询（常规路径）
        let wql = format!("SELECT DeviceId,HealthStatus,FriendlyName,MediaType,BusType FROM MSFT_PhysicalDisk WHERE DeviceId='{}'", disk_id);
        if let Ok(disks) = conn.raw_query::<WmiPhysicalDisk>(&wql) {
            if let Some(disk) = disks.first() {
                return wmi_build_from_storage(&conn, disk_id, disk);
            }
            // 1b) WHERE 查空：DeviceId 与 PhysicalDriveN 编号不一致（RAID/虚拟盘/
            //     多控制器环境常见）→ 全量查询按索引顺序匹配
            let all_wql = "SELECT DeviceId,HealthStatus,FriendlyName,MediaType,BusType FROM MSFT_PhysicalDisk";
            if let Ok(all) = conn.raw_query::<WmiPhysicalDisk>(all_wql) {
                let idx: usize = disk_id.parse().unwrap_or(usize::MAX);
                if let Some(disk) = all.get(idx) {
                    return wmi_build_from_storage(&conn, disk_id, disk);
                }
            }
        }
    } else {
        log::warn!("disk.wmi: 连接 Storage 命名空间失败，尝试 Win32_DiskDrive 兜底");
    }

    // 第二优先：root\\CIMV2 Win32_DiskDrive（无 Storage 服务/精简系统仍可用）
    if let Ok(conn) = wmi::WMIConnection::new() {
        let idx: u32 = disk_id.parse().unwrap_or(u32::MAX);
        let wql = format!("SELECT Index,Status,Model,SerialNumber,InterfaceType,MediaType,Size FROM Win32_DiskDrive WHERE Index={}", idx);
        match conn.raw_query::<WmiWin32DiskDrive>(&wql) {
            Ok(disks) => {
                if let Some(d) = disks.first() {
                    log::warn!("disk.wmi: 磁盘 {} 使用 Win32_DiskDrive 兜底（Status={:?}）", disk_id, d.Status);
                    return Ok(Some(WmiFallbackData {
                        health: WmiHealthInfo {
                            health_status: win32_status_to_health(d.Status.as_deref()),
                            temperature_c: None,
                            power_on_hours: None,
                            read_errors_total: None,
                            write_errors_total: None,
                        },
                        model: d.Model.clone().unwrap_or_default(),
                        interface_type: win32_interface_label(d.InterfaceType.as_deref()),
                        media_type: "未知".to_string(),
                    }));
                }
            }
            Err(e) => log::warn!("disk.wmi: Win32_DiskDrive 查询失败: {}", e),
        }
    }

    Ok(None)
}

/// 由 MSFT_PhysicalDisk 记录构建兜底数据（附可靠性计数器尽力读取）
fn wmi_build_from_storage(
    conn: &wmi::WMIConnection,
    disk_id: &str,
    disk: &WmiPhysicalDisk,
) -> Result<Option<WmiFallbackData>, CollectError> {
    let mut temperature_c = None;
    let mut power_on_hours = None;
    let mut read_errors = None;
    let mut write_errors = None;
    let rc_wql = format!("SELECT DeviceId,Temperature,PowerOnHours,ReadErrorsTotal,WriteErrorsTotal FROM MSFT_StorageReliabilityCounter WHERE DeviceId='{}'", disk_id);
    match conn.raw_query::<WmiReliabilityCounter>(&rc_wql) {
        Ok(counters) => {
            if let Some(c) = counters.first() {
                temperature_c = c.Temperature;
                power_on_hours = c.PowerOnHours;
                read_errors = c.ReadErrorsTotal;
                write_errors = c.WriteErrorsTotal;
            }
        }
        Err(e) => log::debug!("disk.wmi: 可靠性计数器查询失败（非阻断）: {}", e),
    }
    Ok(Some(WmiFallbackData {
        health: WmiHealthInfo {
            health_status: disk.HealthStatus.unwrap_or(5) as u8,
            temperature_c,
            power_on_hours,
            read_errors_total: read_errors,
            write_errors_total: write_errors,
        },
        model: disk.FriendlyName.clone().unwrap_or_default(),
        interface_type: wmi_bus_type_label(disk.BusType),
        media_type: wmi_media_type_label(disk.MediaType),
    }))
}

fn wmi_bus_type_label(bus: Option<u16>) -> String {
    match bus {
        Some(17) => "NVMe".to_string(),
        Some(11) => "SATA".to_string(),
        Some(3) => "ATA".to_string(),
        Some(7) => "USB".to_string(),
        Some(1) => "SCSI".to_string(),
        Some(n) => format!("其他({})", n),
        None => String::new(),
    }
}

/// WMI MediaType 枚举 → 中文标签（3=HDD 4=SSD）
fn wmi_media_type_label(media: Option<u16>) -> String {
    match media {
        Some(3) => "HDD".to_string(),
        Some(4) => "SSD".to_string(),
        _ => "未知".to_string(),
    }
}

// ---------------------------------------------------------------------------
// 辅助
// ---------------------------------------------------------------------------

/// 读取 GetLastError 错误码
fn last_error_code() -> u32 {
    // SAFETY: GetLastError 为无参标准导出，线程局部存储
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ata_attributes_empty() {
        // 全 0 表 → 空属性
        let table = [0u8; 512];
        let attrs = parse_ata_attributes(&table, None).expect("parse 应成功");
        assert!(attrs.is_empty());
    }

    #[test]
    fn test_parse_ata_attributes_one_entry() {
        // 手工构造一条属性：id=0x05（重映射扇区），flags 任意（不影响状态判定），
        // current=90 worst=85 raw=123（小端）；无阈值表 → status=OK（不误报）
        let mut table = [0u8; 512];
        table[2 + 0] = 0x05; // id
        table[2 + 1] = 0x01; // flags 低字节（在线采集位，非失败位）
        table[2 + 2] = 0x00; // flags 高字节
        table[2 + 3] = 90; // current
        table[2 + 4] = 85; // worst
        table[2 + 5] = 123; // raw 低字节
        table[2 + 6] = 0;
        table[2 + 7] = 0;
        table[2 + 8] = 0;
        table[2 + 9] = 0;
        table[2 + 10] = 0;
        table[2 + 11] = 0; // raw 高字节

        let attrs = parse_ata_attributes(&table, None).expect("parse 应成功");
        assert_eq!(attrs.len(), 1);
        let a = &attrs[0];
        assert_eq!(a.id, 0x05);
        assert_eq!(a.name, "重新分配扇区数");
        assert_eq!(a.raw, 123);
        assert_eq!(a.value, 90);
        assert_eq!(a.worst, 85);
        // 无阈值 → 不判定失败（v0.20.x 误报 FAILED 回归修复）
        assert_eq!(a.status, "OK");
    }

    #[test]
    fn test_parse_ata_attributes_raw_le() {
        // raw 6 字节小端：0x112233445566 → 0x665544332211
        let mut table = [0u8; 512];
        table[2 + 0] = 0x09; // 通电时间
        table[2 + 5] = 0x11;
        table[2 + 6] = 0x22;
        table[2 + 7] = 0x33;
        table[2 + 8] = 0x44;
        table[2 + 9] = 0x55;
        table[2 + 10] = 0x66;
        let attrs = parse_ata_attributes(&table, None).expect("parse 应成功");
        assert_eq!(attrs[0].raw, 0x665544332211);
        assert_eq!(attrs[0].name, "通电时间");
    }

    #[test]
    fn test_parse_ata_attributes_status_failed() {
        // 有阈值表且 value <= threshold → FAILED（正确失败判定）
        let mut table = [0u8; 512];
        table[2 + 0] = 0xC6;
        table[2 + 1] = 0x03; // flags 含在线采集位（不参与判定）
        table[2 + 3] = 80; // current=80
        let mut thr = [0u8; 512];
        thr[2 + 0] = 0xC6;
        thr[2 + 3] = 85; // threshold=85 → 80 <= 85 → FAILED
        let attrs = parse_ata_attributes(&table, Some(&thr)).expect("parse 应成功");
        assert_eq!(attrs[0].status, "FAILED");
        assert_eq!(attrs[0].threshold, 85);
        // 对照组：value 高于阈值 → OK
        table[2 + 3] = 90;
        let attrs2 = parse_ata_attributes(&table, Some(&thr)).expect("parse 应成功");
        assert_eq!(attrs2[0].status, "OK");
    }

    #[test]
    fn test_parse_ata_attributes_short_table() {
        let r = parse_ata_attributes(&[0u8; 100], None);
        assert!(r.is_err());
    }

    #[test]
    fn test_u128_le() {
        let mut b = [0u8; 16];
        b[0] = 0x78;
        b[1] = 0x56;
        b[2] = 0x34;
        b[3] = 0x12;
        assert_eq!(u128_le(&b), 0x12345678);
    }

    #[test]
    fn test_attribute_name_known() {
        // CDI 对齐命名抽查
        assert_eq!(attribute_name(0x05), "重新分配扇区数");
        assert_eq!(attribute_name(0x09), "通电时间");
        assert_eq!(attribute_name(0x0C), "通电周期计数");
        assert_eq!(attribute_name(0xC2), "温度");
        assert_eq!(attribute_name(0xBE), "气流温度");
        assert_eq!(attribute_name(0xE9), "介质磨损指示器");
        assert_eq!(attribute_name(0xAF), "未知属性");
        assert_eq!(attribute_name(0xC7), "UDMA CRC 错误计数");
        assert_eq!(attribute_name(0xFF), "备用块总数");
        assert_eq!(attribute_name(0x99), "未知属性");
    }

    #[test]
    fn test_read_smart_invalid_id() {
        // 非数字 id → 解析错误
        let r = read_smart("abc");
        assert!(matches!(r, Err(CollectError::Parse { .. })));
    }
}
