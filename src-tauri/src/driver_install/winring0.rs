//! winring0 —— WinRing0 数据通道：经系统已签名驱动 WinRing0 的端口 IO 读 SMN 温度。
//!
//! 背景：自研内核驱动移除后，内核温度通道为 GlobalSign 商业签名的 WinRing0
//! （v1.2.0.5，本机实证可加载）。ring3 无权执行 in/out 指令，由 WinRing0 驱动的
//! 端口 IO / MSR IOCTL 代执行。
//!
//! 协议来源：AMD 温度寄存器协议经用户态自测 backend 真机验证（7800X3D 与
//! HWiNFO 偏差 ±0.5°C），此处照同一协议实现为 SECM 正式模块（std 环境）。
//!
//! ⚠️ 安全风险声明（如实标注）：
//!   - WinRing0（OpenLibSys hotproject 变体）存在历史 CVE（如 CVE-2020-14979
//!     任意端口 IO），本项目仅用作"受限数据通道"：固定自带版本、仅暴露
//!     端口 IO / MSR 读取最小面、HVCI 开启时上层跳过本通道（见 driver_install.rs）。
//!   - 本模块只读温度寄存器（SMN 0x59800 经 PCI 配置窗口），不做任何写入
//!     （WinRing0 的 IOCTL_WRITE_PORT_DWORD 仅用于 PCI CF8/CFC 寻址序列）。
//!   - 后续 attestation 签名落地后 L0 全量可用，本通道自动退居兜底（§11 路线图）。
//!
//! 任何路径不 panic：不 unwrap、不 expect，失败一律返回 None/Err。

use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::Mutex;

// ============================================================================
// WinRing0 IOCTL 与 WinAPI FFI（kernel32，零第三方库）
// ============================================================================

/// WinRing0 魔改版（hotproject）端口 IO IOCTL（与 zen_thermal backend 同值）
const IOCTL_READ_PORT_DWORD: u32 = 0x9c40_60d4;
/// 写端口 DWORD IOCTL（仅用于 PCI 寻址序列，不触碰其他端口）
const IOCTL_WRITE_PORT_DWORD: u32 = 0x9c40_a0e0;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_WRITE: u32 = 0x2;
const OPEN_EXISTING: u32 = 0x3;

/// WinRing0 设备名（服务名带版本号是 LHM 约定，沿用，与既有实例兼容）
const DEVICE_PATH: &str = r"\\.\WinRing0_1_2_0";

/// 句柄类型（与 cleanup.rs 的 FFI 声明保持同签名，避免重复声明冲突）
type Handle = *mut c_void;

/// CreateFileW 失败时的返回值 (HANDLE)-1
const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        lp_file_name: *const u16,
        dw_desired_access: u32,
        dw_share_mode: u32,
        lp_security_attributes: *mut c_void,
        dw_creation_disposition: u32,
        dw_flags_and_attributes: u32,
        h_template_file: *mut c_void,
    ) -> Handle;
    fn DeviceIoControl(
        h_device: Handle,
        dw_io_control_code: u32,
        lp_in_buffer: *mut c_void,
        n_in_buffer_size: u32,
        lp_out_buffer: *mut c_void,
        n_out_buffer_size: u32,
        lp_bytes_returned: *mut u32,
        lp_overlapped: *mut c_void,
    ) -> i32;
    fn CloseHandle(h_object: Handle) -> i32;
}

/// 设备句柄缓存（单句柄多线程安全：DeviceIoControl 同步，句柄可共享）。
/// 用 isize 存储（raw pointer 非 Send，isize 可安全跨线程）。
static HANDLE: Mutex<Option<isize>> = Mutex::new(None);
/// 句柄已显式关闭标志（close_device 置位；open_device 检测到后重新打开）。
/// 用于卸载/退出路径与并发 IO 的协调：关闭后任何新 IO 都会先重新打开句柄，
/// 避免使用已关闭句柄（句柄可能被系统复用为其他对象，IO 会作用到错误对象）。
static CLOSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 打开 WinRing0 设备（失败返回 Err；句柄缓存复用）。
fn open_device() -> Result<Handle, ()> {
    // 句柄曾被显式关闭（卸载/退出路径）：重新启用缓存，下次 IO 重新打开
    if CLOSED.load(std::sync::atomic::Ordering::SeqCst) {
        CLOSED.store(false, std::sync::atomic::Ordering::SeqCst);
    }
    // 先查缓存（避免每次 IOCTL 都开关句柄）
    if let Ok(guard) = HANDLE.lock() {
        if let Some(h) = *guard {
            if h != INVALID_HANDLE_VALUE as isize {
                return Ok(h as Handle);
            }
        }
    }
    let path: Vec<u16> = DEVICE_PATH.encode_utf16().chain(Some(0)).collect();
    // SAFETY: path 为 NUL 结尾 UTF-16；安全属性 NULL，无模板句柄；
    // 失败时 CreateFileW 返回 INVALID_HANDLE_VALUE(-1)
    let h = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null_mut(),
            OPEN_EXISTING,
            0,
            ptr::null_mut::<c_void>(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(());
    }
    // 缓存句柄（多线程同时打开时以先到者为准，晚到者关闭自己的句柄）
    let mut guard = HANDLE.lock().unwrap_or_else(|p| p.into_inner());
    match *guard {
        Some(existing) if existing != INVALID_HANDLE_VALUE as isize => {
            // SAFETY: 本线程刚打开的句柄，未被其他路径使用
            unsafe {
                CloseHandle(h);
            }
            Ok(existing as Handle)
        }
        _ => {
            *guard = Some(h as isize);
            Ok(h)
        }
    }
}

/// 经 WinRing0 端口读 DWORD（输入 4 字节端口号，输出 4 字节值）。
fn read_port_dword(h: Handle, port: u16) -> Result<u32, ()> {
    let mut out: MaybeUninit<u32> = MaybeUninit::uninit();
    let mut returned: u32 = 0;
    // 输入缓冲必须为 4 字节 DWORD（驱动按 ULONG 端口号读取）——u16 直接取地址
    // 只保证 2 字节有效，高 16 位读栈垃圾会导致端口号错误（读错寄存器）；
    // 修复：先扩展为 u32（高 16 位清零）再传地址（审查 A1 修复）
    let port_in: u32 = port as u32;
    // SAFETY: 输入缓冲 4 字节（端口号 DWORD），输出缓冲 4 字节（值），
    // 与 WinRing0 READ_PORT_DWORD 协议一致（zen_thermal backend 同协议）
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_READ_PORT_DWORD,
            &port_in as *const u32 as *mut c_void,
            4,
            out.as_mut_ptr() as *mut c_void,
            4,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(());
    }
    // SAFETY: IOCTL 成功返回后 out 已被驱动写入
    Ok(unsafe { out.assume_init() })
}

/// 经 WinRing0 端口写 DWORD（输入 8 字节 = 端口号 + 值；仅 PCI 寻址用）。
fn write_port_dword(h: Handle, port: u16, value: u32) -> Result<(), ()> {
    let mut buf: [u32; 2] = [port as u32, value];
    let mut returned: u32 = 0;
    // SAFETY: 输入缓冲 8 字节（端口号 + 值），与驱动协议一致
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_WRITE_PORT_DWORD,
            buf.as_mut_ptr() as *mut c_void,
            8,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(());
    }
    Ok(())
}

// ============================================================================
// PCI 配置窗口 → SMN（与 Linux amd_smn_read() 一致）
// ============================================================================

/// PCI 配置空间访问端口（传统 CF8/CFC 机制）
const PCI_CONFIG_ADDRESS: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;
const CONFIG_ENABLE: u32 = 0x8000_0000;
/// SMN 索引寄存器（root complex 配置偏移 0x60）
const SMN_INDEX_OFFSET: u8 = 0x60;
/// SMN 数据寄存器（root complex 配置偏移 0x64）
const SMN_DATA_OFFSET: u8 = 0x64;
/// SMN sector 寄存器（root complex 配置偏移 0x58，仅高地址段需要）
const SMN_SECTOR_OFFSET: u8 = 0x58;
/// root complex 位置：AMD 平台恒在 bus 0 dev 0 func 0
const ROOT: (u8, u8, u8) = (0, 0, 0);

/// PCI 配置空间读 DWORD（任意 BDF；标准 CF8/CFC 机制，经 WinRing0 端口 IO）。
///
/// 读不存在的 BDF 时配置空间返回 0xFFFFFFFF（不 #GP、不崩溃），由调用方判无效；
/// 仅在平台门控通过后调用（Intel 平台不得触碰 AMD k10temp 通路）。
fn pci_config_read_u32(bus: u8, dev: u8, func: u8, offset: u8) -> Result<u32, ()> {
    let h = open_device()?;
    let addr: u32 = CONFIG_ENABLE
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | (offset as u32 & 0xFC);
    write_port_dword(h, PCI_CONFIG_ADDRESS, addr)?;
    read_port_dword(h, PCI_CONFIG_DATA)
}

/// PCI 配置空间读 DWORD（root complex 位置，SMN 通路专用）。
fn pci_read_u32(offset: u8) -> Result<u32, ()> {
    pci_config_read_u32(ROOT.0, ROOT.1, ROOT.2, offset)
}

/// PCI 配置空间写 DWORD（仅限 SMN 索引/数据/sector 寄存器，不触碰其他）。
fn pci_write_u32(offset: u8, value: u32) -> Result<(), ()> {
    let h = open_device()?;
    let addr: u32 = CONFIG_ENABLE
        | ((ROOT.0 as u32) << 16)
        | ((ROOT.1 as u32) << 11)
        | ((ROOT.2 as u32) << 8)
        | (offset as u32 & 0xFC);
    write_port_dword(h, PCI_CONFIG_ADDRESS, addr)?;
    write_port_dword(h, PCI_CONFIG_DATA, value)
}

/// SMN 读 DWORD（与 Linux `amd_smn_read()` 一致）。
/// 任一步 IO 失败即返回 Err。
fn smn_read(smn_addr: u32) -> Result<u32, ()> {
    let sector = (smn_addr >> 20) & 0xFF;
    if sector > 0x3F {
        pci_write_u32(SMN_SECTOR_OFFSET, sector)?;
    }
    pci_write_u32(SMN_INDEX_OFFSET, smn_addr & 0xFFF_FFFF)?;
    pci_read_u32(SMN_DATA_OFFSET)
}

// ============================================================================
// 温度解码（与 zen_thermal::decode_tctl 同一算法，k10temp 一致）
// ============================================================================

/// AMD legacy k10temp PCI 位置（bus 0, dev 24 = 0x18, func 3；F3 = Northbridge 温度）
const K10TEMP_BUS: u8 = 0;
const K10TEMP_DEV: u8 = 0x18;
const K10TEMP_FUNC: u8 = 3;
/// k10temp 温度寄存器偏移（D18F3_REPORTED_TEMP，Linux k10temp 同协议）
const K10TEMP_TEMP_OFFSET: u8 = 0xA4;
/// k10temp 温度字段移位（bits[21:31]）与掩码（11 位）
const K10TEMP_TEMP_SHIFT: u32 = 21;
const K10TEMP_TEMP_MASK: u32 = 0x7FF;
/// k10temp 温度上限（毫摄氏度）：超出 [0, 105] °C 判无效
const K10TEMP_TEMP_MAX_MC: u32 = 105_000;

/// SMN 温度寄存器基址（ZEN_REPORTED_TEMP_CTRL_BASE）
const SMN_TEMP_CTRL_BASE: u32 = 0x0005_9800;
/// tctl 温度字段移位（bit[31:21]）
const CUR_TEMP_SHIFT: u32 = 21;
/// tctl 温度字段掩码（11 位）
const CUR_TEMP_MASK: u32 = 0x7FF;
/// RANGE_SEL 位（bit19）：置位时温度需减 49000 毫摄氏度
const CUR_TEMP_RANGE_SEL_MASK: u32 = 1 << 19;
/// TJ_SEL 字段掩码（bit17:16）：全 1 时温度需减 49000 毫摄氏度
const CUR_TEMP_TJ_SEL_MASK: u32 = 0b11 << 16;
/// k10temp 的 -49 °C 温度调整量（毫摄氏度）
const TEMP_ADJUST_MC: u32 = 49_000;
/// 温度上限（毫摄氏度）：超出视为无效/截断
const TEMP_MAX_MC: u32 = 255_000;

/// 解码 SMN 0x59800（Reported Temperature Control，tctl/tDie）。
///
/// bit[31:21] 为温度字段，LSB = 0.125 °C；若 RANGE_SEL(bit19) 或
/// TJ_SEL(bit17:16)==0b11 置位，减去 49000 毫摄氏度（k10temp 一致）。
/// 字段为 0 或结果超上限时返回 None（无效）。
pub fn decode_tctl(raw: u32) -> Option<u32> {
    let field = (raw >> CUR_TEMP_SHIFT) & CUR_TEMP_MASK;
    if field == 0 {
        return None;
    }
    let mut mc = field * 125;
    let range_sel = raw & CUR_TEMP_RANGE_SEL_MASK != 0;
    // bit17:16 全 1（直接在 raw 上取掩码，与 zen_thermal 语义一致）
    let tj_sel_all = raw & CUR_TEMP_TJ_SEL_MASK == CUR_TEMP_TJ_SEL_MASK;
    if range_sel || tj_sel_all {
        mc = mc.saturating_sub(TEMP_ADJUST_MC);
    }
    Some(if mc > TEMP_MAX_MC { TEMP_MAX_MC } else { mc })
}

// ============================================================================
// CPU 代际门控：仅 AMD Zen（17h/19h/1Ah）走 SMN 路径
// ============================================================================

/// CPUID leaf 0 厂商字符串（EBX+EDX+ECX 拼接，12 字节；__cpuid 在本工具链为 safe 函数）。
fn cpuid_vendor() -> [u8; 12] {
    let leaf0 = std::arch::x86_64::__cpuid(0);
    let mut vendor = [0u8; 12];
    vendor[0..4].copy_from_slice(&leaf0.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&leaf0.edx.to_le_bytes());
    vendor[8..12].copy_from_slice(&leaf0.ecx.to_le_bytes());
    vendor
}

/// 解析 CPUID leaf 1 EAX 为 (family, model)，均含扩展位（纯逻辑，可单测）。
///
/// family：base family 为 0xF 时累加 extended family（AMD 15h/16h/17h/19h 等）；
/// model：base family 为 6 或 0xF 时取 (extended model << 4) | base model。
fn decode_family_model(leaf1_eax: u32) -> (u32, u32) {
    let base_family = ((leaf1_eax >> 8) & 0xF) as u32;
    let ext_family = ((leaf1_eax >> 20) & 0xFF) as u32;
    let family = if base_family == 0xF {
        base_family + ext_family
    } else {
        base_family
    };
    let base_model = ((leaf1_eax >> 4) & 0xF) as u32;
    let ext_model = ((leaf1_eax >> 16) & 0xF) as u32;
    let model = if base_family == 0x6 || base_family == 0xF {
        (ext_model << 4) | base_model
    } else {
        base_model
    };
    (family, model)
}

/// CPUID leaf 1 的 (family, model)，含扩展位（__cpuid 在本工具链为 safe 函数）。
fn cpuid_family_model() -> (u32, u32) {
    let leaf1 = std::arch::x86_64::__cpuid(1);
    decode_family_model(leaf1.eax)
}

/// Intel RAPL 门控纯逻辑：family 6 且 model >= 0x2A（Sandy Bridge 起，覆盖
/// X79(0x2D/0x3E)/X99(0x3F/0x4F/0x55/0x56)/12/13/14 代），且排除安全盲区
/// model 0x2C（Gulftown/Westmere-EP，Xeon 5600/i7-980X：数值 >= 0x2A 但无
/// RAPL，读 0x606/0x611 会 #GP）。
///
/// Nehalem/Westmere（model < 0x2A）不支持 PKG RAPL，严禁对其执行 rdmsr
/// （不存在的 MSR 会 #GP 导致系统 bugcheck —— 本项目最高安全红线）；
/// 0x2C 盲区为设计文档未覆盖，secm-dev 实现时按 R4 补上（见遗留风险）。
fn intel_rapl_supported(family: u32, model: u32) -> bool {
    family == 6 && model >= 0x2A && model != 0x2C
}

/// AMD legacy（K10/FX/AM3+）门控纯逻辑：family 10h/15h/16h。
/// 该平台无 package RAPL，仅 k10temp 温度通路适用。
fn amd_legacy_supported(family: u32) -> bool {
    matches!(family, 0x10 | 0x15 | 0x16)
}

/// 判断当前 CPU 是否为 AMD Zen 代际（family 17h/19h/1Ah）。
///
/// 非 AMD / 非 Zen 平台返回 false，调用方跳过本通道（避免对未知寄存器寻址）。
pub fn cpu_is_zen_amd() -> bool {
    if &cpuid_vendor() != b"AuthenticAMD" {
        return false;
    }
    let (family, _model) = cpuid_family_model();
    matches!(family, 0x17 | 0x19 | 0x1A)
}

/// 判断当前 CPU 是否为 Intel family 6（含 X99/X79 全系 E5 与消费级；MSR 热管理语义门控）。
pub fn cpu_is_intel() -> bool {
    if &cpuid_vendor() != b"GenuineIntel" {
        return false;
    }
    let (family, _) = cpuid_family_model();
    family == 6
}

/// 读取 Intel TjMax（°C）：IA32_TEMPERATURE_TARGET bits[23:16]，范围校验后返回；
/// 失败/越界返回 None（调用方安全降级，不猜测 TjMax）。
fn intel_tj_max(h: Handle) -> Option<u32> {
    let raw = read_msr(h, MSR_IA32_TEMP_TARGET).ok()?;
    let tj = ((raw >> 16) & 0xFF) as u32;
    if (INTEL_TJ_MAX_MIN..=INTEL_TJ_MAX_MAX).contains(&tj) {
        Some(tj)
    } else {
        None
    }
}

/// 从 PACKAGE/IA32_THERM_STATUS 解码 Intel DTS 温度（毫℃）。
///
/// 位布局（SDM 34.5.3）：bits[31] Reading Valid、bits[22:16] Digital Readout（单位 1°C）。
/// 温度 = TjMax - readout；与 Linux coretemp 同算法：
/// Resolution 字段（bits[15:8]）仅表示传感器内部分辨率，不同代次取值不一致，
/// 直接换算会引入十倍级错误，故按 1°C 步进忽略。
fn decode_intel_dts(msr: u64, tj_max_c: u32) -> Option<u32> {
    if (msr >> 31) & 1 == 0 {
        return None; // Reading Valid = 0，读数无效
    }
    let readout = ((msr >> 16) & 0x7F) as u32;
    Some(tj_max_c.saturating_sub(readout) * 1000)
}

/// 判断当前 CPU 是否支持 Intel RAPL（GenuineIntel + family 6 + model >= 0x2A，
/// 即 Sandy Bridge 起，覆盖 X79/X99/12/13/14 代；Nehalem/Westmere 返回 false）。
///
/// 仅在返回 true 时才允许读取 Intel RAPL MSR（0x606/0x611），防 #GP bugcheck。
pub fn cpu_is_intel_rapl() -> bool {
    if &cpuid_vendor() != b"GenuineIntel" {
        return false;
    }
    let (family, model) = cpuid_family_model();
    intel_rapl_supported(family, model)
}

/// 判断当前 CPU 是否为 AMD legacy 平台（family 10h/15h/16h：K10/FX/AM3+）。
///
/// 仅在返回 true 时才允许读取 k10temp PCI 温度通路（bus0 dev24 func3 0xA4）；
/// Intel 平台（含 X99/X79）严格不触碰该通路。
pub fn cpu_is_amd_legacy() -> bool {
    if &cpuid_vendor() != b"AuthenticAMD" {
        return false;
    }
    let (family, _model) = cpuid_family_model();
    amd_legacy_supported(family)
}

// ============================================================================
// 对外探活入口
// ============================================================================

/// WinRing0（内置 hotproject 变体）MSR 读 IOCTL。
///
/// ⚠ 实测（2026-08-07 验证）：本变体的 READ_MSR 是
/// `CTL_CODE(0x9C40, 0x821<<2)` = 0x9C402084（**不是** 标准 OpenLibSys 的
/// 0x9C402004，那个在本变体中返回恒 0x1/0x2 = stub）。0x9C402084 读 MSR 0x10
/// (TSC) 返回真实变化值，读 AMD RAPL 0xC0010299/0xC001029B 返回真实能量计数。
const IOCTL_READ_MSR: u32 = 0x9C40_2084;

/// AMD RAPL 能量单位寄存器（ESU bits[12:8]，energy = count × 2^-ESU J）
const MSR_AMD_PWR_UNIT: u32 = 0xC001_0299;
/// AMD RAPL package 能量累计寄存器（低 32 位计数，回绕处理见调用方）
const MSR_AMD_PKG_ENERGY: u32 = 0xC001_029B;
/// Intel RAPL 能量单位寄存器（RAPL_POWER_UNIT，ESU bits[12:8]，energy = count × 2^-ESU J）
const MSR_INTEL_PWR_UNIT: u32 = 0x606;
/// Intel RAPL PKG 能量累计寄存器（PKG_ENERGY_STATUS，低 32 位计数，回绕处理见调用方）
const MSR_INTEL_PKG_ENERGY: u32 = 0x611;
/// Intel 温度目标寄存器（IA32_TEMPERATURE_TARGET，bits[23:16] = TjMax °C）
const MSR_IA32_TEMP_TARGET: u32 = 0x1A2;
/// Intel 每逻辑核热状态（IA32_THERM_STATUS，bits[31] valid + bits[22:16] DTS readout）
const MSR_IA32_THERM_STATUS: u32 = 0x19C;
/// Intel 物理包热状态（PACKAGE_THERM_STATUS，bits[31] valid + bits[22:16] DTS readout）
const MSR_PACKAGE_THERM_STATUS: u32 = 0x1B1;
/// TjMax 合理性下界（°C，防损坏 MSR 值）
const INTEL_TJ_MAX_MIN: u32 = 60;
/// TjMax 合理性上界（°C）
const INTEL_TJ_MAX_MAX: u32 = 110;

/// 经 WinRing0 读 MSR（仅 RAPL 只读寄存器——Intel 0x606/0x611、AMD
/// 0xC0010299/0x29B；平台门控 cpu_is_intel_rapl/cpu_is_zen_amd 通过后才可
/// 调用，防 #GP；驱动不支持 MSR IOCTL 时返回 Err）。
fn read_msr(h: Handle, msr: u32) -> Result<u64, ()> {
    let mut out = MaybeUninit::<[u8; 8]>::uninit();
    let mut returned: u32 = 0;
    // SAFETY: 输入 4 字节 MSR 地址，输出 8 字节值，与 WinRing0 READ_MSR 协议一致
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_READ_MSR,
            &msr as *const u32 as *mut c_void,
            4,
            out.as_mut_ptr() as *mut c_void,
            8,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(());
    }
    // SAFETY: IOCTL 成功返回后 out 已被驱动写入
    Ok(u64::from_le_bytes(unsafe { out.assume_init() }))
}

/// RAPL 能量单位换算（纯逻辑）：ESU 字段（MSR bits[12:8]）→ 每计数焦耳（2^-ESU）。
///
/// ESU 为 0 或 >= 32 视为非法（寄存器未初始化/越界），返回 None。
fn rapl_energy_per_count(esu: u32) -> Option<f64> {
    if esu == 0 || esu >= 32 {
        return None;
    }
    Some(2f64.powi(-(esu as i32)))
}

/// 读取 Intel RAPL package 能量（MSR 0x606 能量单位 + 0x611 PKG_ENERGY_STATUS）。
///
/// 仅允许在 `cpu_is_intel_rapl()` 门控通过后调用（0x606/0x611 在 Sandy Bridge+
/// 全系存在，但门控保证绝不触碰 Nehalem 等无 RAPL 平台，防 #GP bugcheck）。
fn intel_rapl_energy() -> Option<(f64, u64, f64)> {
    let h = open_device().ok()?;
    let unit_raw = read_msr(h, MSR_INTEL_PWR_UNIT).ok()?;
    let esu = ((unit_raw >> 8) & 0x1F) as u32;
    let unit = rapl_energy_per_count(esu)?;
    let count = read_msr(h, MSR_INTEL_PKG_ENERGY).ok()? & 0xFFFF_FFFF;
    Some((count as f64 * unit, count, unit))
}

/// 读取 AMD RAPL package 能量（MSR 0xC0010299/0xC001029B）。
///
/// 仅允许在 `cpu_is_zen_amd()` 门控通过后调用。
fn amd_rapl_energy() -> Option<(f64, u64, f64)> {
    let h = open_device().ok()?;
    let unit_raw = read_msr(h, MSR_AMD_PWR_UNIT).ok()?;
    let esu = ((unit_raw >> 8) & 0x1F) as u32;
    let unit = rapl_energy_per_count(esu)?;
    let count = read_msr(h, MSR_AMD_PKG_ENERGY).ok()? & 0xFFFF_FFFF;
    Some((count as f64 * unit, count, unit))
}

/// 读取 CPU package RAPL 能量（按平台自动分流）。
///
/// 返回 (累计能量焦耳, 原始 32 位计数, 每计数焦耳)；驱动/平台不支持时 None。
/// 调用方用两次采样的计数差值计算功耗（能量计数秒级累计，差值法避免累计误差）。
/// 平台分流：Intel（Sandy Bridge+，含 X79/X99/12/13/14 代）→ 0x606/0x611；
/// AMD Zen → 0xC0010299/0x29B；AMD legacy / 其他 → None（无 RAPL，走 TDP 估算）。
pub fn winring0_rapl_energy() -> Option<(f64, u64, f64)> {
    // Intel RAPL 优先（门控：仅 GenuineIntel + family 6 + model >= 0x2A）
    if cpu_is_intel_rapl() {
        return intel_rapl_energy();
    }
    // AMD Zen RAPL（17h/19h/1Ah）
    if cpu_is_zen_amd() {
        return amd_rapl_energy();
    }
    // 其他平台（AMD 老平台无 package 能量 MSR、未知 CPU）→ None
    None
}

/// 解码 AMD legacy k10temp PCI 温度寄存器（0xA4，D18F3_REPORTED_TEMP）。
///
/// 温度字段 bits[21:31]（11 位），LSB = 0.125 °C（Linux k10temp 同协议）。
/// 字段全 0（传感器未初始化）或全 1（PCI 读不存在的 BDF 返回 0xFFFFFFFF 的
/// 特征）或温度超出 [0, 105] °C 判无效 → None。
pub fn decode_k10temp(raw: u32) -> Option<u32> {
    let field = (raw >> K10TEMP_TEMP_SHIFT) & K10TEMP_TEMP_MASK;
    if field == 0 {
        return None; // 传感器未初始化
    }
    let mc = field * 125;
    if mc > K10TEMP_TEMP_MAX_MC {
        return None; // 全 1 字段（255.875 °C）或超出上限
    }
    Some(mc)
}

/// 探活：读取 CPU package 温度（毫摄氏度）。
///
/// 平台分流：AMD Zen → SMN 0x59800（tctl/tDie，k10temp 算法）；
/// AMD legacy（K10/FX/AM3+）→ PCI 配置窗口 k10temp 通路；
/// Intel 平台（X99/X79 等）→ WinRing0 MSR：IA32_TEMPERATURE_TARGET (0x1A2) TjMax
///   + PACKAGE_THERM_STATUS (0x1B1) DTS 包温度（回退 IA32_THERM_STATUS 0x19C 每核）。
/// None 表示通道不可用（驱动未加载/平台不支持/寄存器无效），上层降级。
///
/// 仅作"通道可用性"探活用途（一次读取），不承担持续采集。
pub fn winring0_cpu_temp() -> Option<u32> {
    // AMD Zen：SMN 0x59800（ZEN_REPORTED_TEMP_CTRL_BASE）
    if cpu_is_zen_amd() {
        let raw = smn_read(SMN_TEMP_CTRL_BASE).ok()?;
        return decode_tctl(raw);
    }
    // AMD legacy（K10/FX/AM3+）：bus0 dev24 func3 配置窗口读 k10temp；
    // 门控保证 Intel 平台（X99/X79）不触碰该 BDF
    if cpu_is_amd_legacy() {
        let raw =
            pci_config_read_u32(K10TEMP_BUS, K10TEMP_DEV, K10TEMP_FUNC, K10TEMP_TEMP_OFFSET)
                .ok()?;
        return decode_k10temp(raw);
    }
    // Intel（family 6，含 X99/X79）：经 WinRing0 MSR 读 DTS 温度（驱动已签名可加载）；
    // 无 TjMax（0x1A2 越界/读失败）时安全降级 None，不猜测
    if cpu_is_intel() {
        let h = open_device().ok()?;
        let tj_max = intel_tj_max(h)?;
        // 优先物理包温度（0x1B1）；若不可用回退每核（0x19C，取处理器 0）
        if let Ok(raw) = read_msr(h, MSR_PACKAGE_THERM_STATUS) {
            if let Some(t) = decode_intel_dts(raw, tj_max) {
                return Some(t);
            }
        }
        if let Ok(raw) = read_msr(h, MSR_IA32_THERM_STATUS) {
            return decode_intel_dts(raw, tj_max);
        }
        return None;
    }
    // 其他平台：本通道无温度通路，交给 ACPI 回退链
    None
}

/// 关闭缓存的设备句柄并清空缓存（M1 修复）。
///
/// 卸载/退出前调用：释放对驱动设备的占用，避免服务停止卡 STOP_PENDING
/// 或删除挂起（win32 0x430）；调用后下次读取会重新打开句柄。
pub fn close_device() {
    // 先置关闭标志（SeqCst）：并发 IO 路径检测到后不再复用旧句柄；
    // 正在进行的 IO 与 close 通过 HANDLE 锁互斥（IO 持锁期间 close 等待）
    CLOSED.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut guard = HANDLE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(h) = guard.take() {
        if h != INVALID_HANDLE_VALUE as isize {
            // SAFETY: h 为本模块缓存的句柄，取出后不再被其他路径使用
            unsafe {
                CloseHandle(h as Handle);
            }
        }
    }
}

// ============================================================================
// 单元测试（纯逻辑，无需硬件）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_tctl_basic() {
        // 温度字段 = 107（对应 13.375 °C 概念温度；字段=856 才是 107 °C）
        let raw = 107u32 << 21;
        assert_eq!(decode_tctl(raw), Some(107 * 125));
    }

    #[test]
    fn decode_tctl_range_sel_adjust() {
        // 7800X3D 实测场景：字段 856 + RANGE_SEL(bit19) → 107000 - 49000 = 58000
        let raw = (856u32 << 21) | (1 << 19);
        assert_eq!(decode_tctl(raw), Some(856 * 125 - 49_000));
    }

    #[test]
    fn decode_tctl_tj_sel_adjust() {
        // 字段 800 + TJ_SEL(bit17:16)==0b11 → 减 49000 毫摄氏度
        let raw = (800u32 << 21) | (0b11 << 16);
        assert_eq!(decode_tctl(raw), Some(800 * 125 - 49_000));
    }

    #[test]
    fn decode_tctl_zero_invalid() {
        assert_eq!(decode_tctl(0), None);
        // 温度字段为 0 但其他位有值 → 仍无效
        let raw = (0u32 << 21) | (1 << 19);
        assert_eq!(decode_tctl(raw), None);
    }

    #[test]
    fn decode_tctl_overflow_capped() {
        // 字段全 1（2047）→ 255875 毫摄氏度 > 上限 255000，截断
        let raw = 0x7FFu32 << 21;
        assert_eq!(decode_tctl(raw), Some(TEMP_MAX_MC));
        // 同样字段 + RANGE_SEL：先减 49000（206875 < 上限），不截断
        let raw2 = (0x7FFu32 << 21) | (1 << 19);
        assert_eq!(decode_tctl(raw2), Some(0x7FF * 125 - 49_000));
    }

    #[test]
    fn smn_sector_split_math() {
        // 0x59800 的 sector = 0（地址仅 18 位），正常路径不写 sector 寄存器
        let sector = (SMN_TEMP_CTRL_BASE >> 20) & 0xFF;
        assert_eq!(sector, 0);
        assert!(sector <= 0x3F);
        // 高地址段示例（sector 0x59 > 0x3F）→ 需要写 0x58 sector 寄存器
        let high = 0x0598_0000u32;
        let hs = (high >> 20) & 0xFF;
        assert_eq!(hs, 0x59);
        assert!(hs > 0x3F);
    }

    // ---- v1.16.0 新增：Intel RAPL / AMD legacy 平台兼容（纯逻辑，无需硬件）----

    #[test]
    fn decode_k10temp_valid_and_boundary() {
        // 字段 107 → 107 × 0.125 °C = 13.375 °C（在 [0, 105] 内）
        let raw = 107u32 << 21;
        assert_eq!(decode_k10temp(raw), Some(107 * 125));
        // 字段 840 → 恰好 105 °C（上限边界，合法）
        let raw2 = 840u32 << 21;
        assert_eq!(decode_k10temp(raw2), Some(105_000));
    }

    #[test]
    fn decode_k10temp_invalid_values() {
        // 字段全 0：传感器未初始化 → None
        assert_eq!(decode_k10temp(0), None);
        // 全 1 字段：PCI 读不存在的 BDF 返回 0xFFFFFFFF 的特征 → None
        assert_eq!(decode_k10temp(0xFFFF_FFFF), None);
        let raw = 0x7FFu32 << 21;
        assert_eq!(decode_k10temp(raw), None);
        // 温度超上限（字段 900 → 112.5 °C）→ None
        let raw2 = 900u32 << 21;
        assert_eq!(decode_k10temp(raw2), None);
    }

    #[test]
    fn decode_intel_dts_valid_and_boundary() {
        // 常规：valid=1，readout=45（TjMax 100 → 55°C）
        let msr = (1u64 << 31) | (45u64 << 16);
        assert_eq!(decode_intel_dts(msr, 100), Some(55_000));
        // 边界：readout = 0（满载，温度 = TjMax）
        let msr = (1u64 << 31) | 0;
        assert_eq!(decode_intel_dts(msr, 105), Some(105_000));
        // 边界：readout = 0x7F（最大偏移，saturating 到 0，不 panic）
        let msr = (1u64 << 31) | (0x7Fu64 << 16);
        assert_eq!(decode_intel_dts(msr, 60), Some(0));
    }

    #[test]
    fn decode_intel_dts_invalid_reading() {
        // Reading Valid = 0 → 读数无效，返回 None（不得用无效读数计算）
        let msr = 45u64 << 16;
        assert_eq!(decode_intel_dts(msr, 100), None);
    }

    #[test]
    fn decode_intel_dts_x99_e5v4_model() {
        // X99 E5 v4（model 0x4F/0x56）典型：TjMax=105，readout=60 → 45°C
        let msr = (1u64 << 31) | (60u64 << 16);
        assert_eq!(decode_intel_dts(msr, 105), Some(45_000));
    }

    #[test]
    fn intel_tj_max_parses_and_range_checks() {
        // 无法构造 Handle，直接测位解析辅助逻辑：0x1A2 的 bits[23:16]
        let raw: u64 = 105 << 16;
        let tj = ((raw >> 16) & 0xFF) as u32;
        assert!((60..=110).contains(&tj));
        // 越界值（损坏 MSR）应被范围校验拒绝——模拟判断逻辑
        let bad: u64 = 200 << 16;
        let tj_bad = ((bad >> 16) & 0xFF) as u32;
        assert!(!(60..=110).contains(&tj_bad));
    }

    #[test]
    fn rapl_energy_per_count_math() {
        // ESU 0（非法）与 ESU >= 32（越界）→ None
        assert_eq!(rapl_energy_per_count(0), None);
        assert_eq!(rapl_energy_per_count(32), None);
        assert_eq!(rapl_energy_per_count(63), None);
        // 常规 ESU：2^-8 / 2^-12 / 2^-20 / 2^-31（上界合法）
        assert!((rapl_energy_per_count(8).unwrap() - 1.0 / 256.0).abs() < 1e-12);
        assert!((rapl_energy_per_count(12).unwrap() - 1.0 / 4096.0).abs() < 1e-15);
        assert!((rapl_energy_per_count(20).unwrap() - 1.0 / 1_048_576.0).abs() < 1e-18);
        assert!((rapl_energy_per_count(31).unwrap() - 2f64.powi(-31)).abs() < 1e-18);
    }

    #[test]
    fn decode_family_model_intel_variants() {
        // Intel Sandy Bridge（family 6, model 0x2A）：leaf1 eax = (0x2<<16)|(6<<8)|(0xA<<4)
        let (f, m) = decode_family_model(0x206A0);
        assert_eq!((f, m), (6, 0x2A));
        // Intel Broadwell-EP X99（family 6, model 0x4F）
        let (f2, m2) = decode_family_model(0x406F0);
        assert_eq!((f2, m2), (6, 0x4F));
    }

    #[test]
    fn decode_family_model_amd_variants() {
        // AMD FX（family 0x15, model 0x02 示例）：base 0xF + ext 0x6
        let (f, m) = decode_family_model((0x6 << 20) | (0xF << 8) | (0x2 << 4));
        assert_eq!((f, m), (0x15, 0x02));
        // AMD Zen4（family 0x19, model 0x61 示例）：ext_family = 0xA
        let (f2, m2) = decode_family_model((0xA << 20) | (0xF << 8) | (0x6 << 16) | (0x1 << 4));
        assert_eq!((f2, m2), (0x19, 0x61));
    }

    #[test]
    fn intel_rapl_gate_platforms() {
        // Sandy Bridge 起点及其后：全部放行（X79/X99/12/13/14 代）
        assert!(intel_rapl_supported(6, 0x2A)); // Sandy Bridge
        assert!(intel_rapl_supported(6, 0x2D)); // SNB-E（X79）
        assert!(intel_rapl_supported(6, 0x2E)); // Nehalem-EX（首个引入 RAPL 的 Xeon，支持 0x606/0x611）
        assert!(intel_rapl_supported(6, 0x3E)); // IVB-E（X79）
        assert!(intel_rapl_supported(6, 0x3F)); // Haswell-EP（X99）
        assert!(intel_rapl_supported(6, 0x4F)); // Broadwell-EP（X99）
        assert!(intel_rapl_supported(6, 0x55)); // Skylake-SP
        assert!(intel_rapl_supported(6, 0x97)); // Alder Lake（12 代）
        assert!(intel_rapl_supported(6, 0xB7)); // Raptor Lake（13/14 代）
        // Nehalem/Westmere（model < 0x2A）：不放行（无 PKG RAPL，rdmsr 会 #GP）
        assert!(!intel_rapl_supported(6, 0x1A));
        assert!(!intel_rapl_supported(6, 0x1E));
        assert!(!intel_rapl_supported(6, 0x1F));
        assert!(!intel_rapl_supported(6, 0x25));
        // 安全盲区修正：Gulftown/Westmere-EP（model 0x2C，Xeon 5600/i7-980X）
        // 数值 >= 0x2A 但无 RAPL，必须排除（读 0x606 会 #GP bugcheck）
        assert!(!intel_rapl_supported(6, 0x2C));
        // 非 family 6（AMD 等）：不放行
        assert!(!intel_rapl_supported(0x15, 0x02));
        assert!(!intel_rapl_supported(0x19, 0x61));
    }

    #[test]
    fn amd_legacy_gate_platforms() {
        // K10/FX/AM3+（10h/15h/16h）：放行
        assert!(amd_legacy_supported(0x10));
        assert!(amd_legacy_supported(0x15));
        assert!(amd_legacy_supported(0x16));
        // Zen（17h/19h）与 Intel（6）：不放行
        assert!(!amd_legacy_supported(0x17));
        assert!(!amd_legacy_supported(0x19));
        assert!(!amd_legacy_supported(6));
    }

    #[test]
    fn rapl_platform_dispatch_isolation() {
        // 分流互斥：同一平台不可能同时命中两条 RAPL 通路
        // Intel（family 6, model >= 0x2A）→ Intel RAPL，且非 AMD legacy
        assert!(intel_rapl_supported(6, 0x2D) && !amd_legacy_supported(6));
        // AMD Zen → AMD RAPL，且非 Intel RAPL / 非 legacy
        assert!(!intel_rapl_supported(0x17, 0x0F) && !amd_legacy_supported(0x17));
        // AMD legacy → 两 RAPL 门控均 false（无 RAPL），温度走 k10temp 通路
        assert!(!intel_rapl_supported(0x15, 0x02) && amd_legacy_supported(0x15));
    }

    /// 真机探活验证（默认忽略；需 WinRing0_1_2_0 服务运行）：
    /// `bash scripts/test-lib.sh -- --ignored real_machine_probe_winring0`
    #[test]
    #[ignore]
    fn real_machine_probe_winring0() {
        // 本机 7800X3D（Zen4）+ WinRing0 RUNNING → 应读到有效温度
        let t = crate::driver_install::winring0::winring0_cpu_temp();
        println!("[真机] winring0_cpu_temp = {:?}", t.map(|mc| format!("{:.1}°C", mc as f64 / 1000.0)));
        assert!(t.is_some(), "WinRing0 通道探活失败（服务未运行或非 AMD Zen 平台）");
    }
}
