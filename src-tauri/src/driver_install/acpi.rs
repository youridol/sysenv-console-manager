//! acpi —— ACPI 热区温度通道（纯用户态 WMI，无驱动）。
//!
//! 背景：当 WinRing0 通道不可用时，退到 ACPI 热区读取 CPU 温度 —— 由固件报告，
//! 无需任何内核驱动，Secure Boot / HVCI 均不拦截。
//!
//! 实现：COM 手写 FFI（ole32，零第三方库），WQL 查询 root\wmi 命名空间的
//! MSAcpi_ThermalZoneTemperature 类，取首个实例的 CurrentTemperature 字段
//! （uint32，单位 0.1 开尔文）。
//!
//! 已知局限（如实告知，不静默）：部分主板 _TMP 热区缺失（ACPI 表未暴露），
//! 查询返回空枚举 → 返回 None，上层显式提示 channel=none，不伪装有数据。

use std::ffi::c_void;
use std::ptr;

// ============================================================================
// COM 基础（ole32，零第三方库）
// ============================================================================

/// HRESULT 成功码
const S_OK: i32 = 0;
const S_FALSE: i32 = 1;
const COINIT_MULTITHREADED: u32 = 0x0;
const CLSCTX_INPROC_SERVER: u32 = 0x1;
/// WQL 查询语言标识
const WQL: &str = "WQL";
/// 查询语句：热区温度（含 InstanceName 以便区分多热区）
const QUERY: &str = "SELECT * FROM MSAcpi_ThermalZoneTemperature";
/// WBEM_FLAG_FORWARD_ONLY：单向枚举器，快且省内存
const WBEM_FLAG_FORWARD_ONLY: i32 = 0x20;
/// WBEM_INFINITE：Next 无限等待（查询本地 WMI 服务，毫秒级返回）
const WBEM_INFINITE: i32 = -1;
/// VARIANT 类型：VT_UI4（无符号 32 位整数，栈内联，无需 VariantClear）
const VT_UI4: u16 = 19;

/// GUID 结构（COM 标识）
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// 由 4 段字符串字面量构造 GUID（小端序，与 WIDL 一致）
const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> Guid {
    Guid {
        data1: a,
        data2: b,
        data3: c,
        data4: d,
    }
}

/// CLSID_WbemLocator（COM 类标识，同时也是 IWbemLocator 的 IID）
const CLSID_WBEM_LOCATOR: Guid = guid(0xDC12A687, 0x737F, 0x11CF, [0x88, 0x4D, 0x00, 0xAA, 0x00, 0x4B, 0x2E, 0x24]);

/// VARIANT（仅需覆盖 VT_UI4 读取的布局；其他类型字段留空）
#[repr(C)]
#[derive(Clone, Copy)]
struct Variant {
    vt: u16,
    rsvd: u16,
    // VT_UI4 时取 ul_val；其他类型（BSTR/对象）本模块不读取，无需释放
    val: u64,
}

/// COM 对象指针
type ComPtr = *mut c_void;
/// vtable 函数指针类型（统一签名：首参为对象自身）
/// COM vtable 方法函数指针类型（精确定参签名，替代 C 变参——C 变参调用定参
/// 方法在 ABI 上是未定义行为（x64 下变参函数浮点参数寄存器约定不同，虽整数/
/// 指针参数碰巧兼容，但属 UB，审查修复）；BSTR 用 *const u16（本模块只传
/// NUL 结尾 UTF-16 字面量，不分配 BSTR）
type ConnectServerFn = unsafe extern "system" fn(
    this: ComPtr,
    network_resource: *const u16,
    user: *const u16,
    password: *const u16,
    locale: *const u16,
    security_flags: i32,
    authority: *const u16,
    ctx: ComPtr,
    services: *mut ComPtr,
) -> i32;
type ExecQueryFn = unsafe extern "system" fn(
    this: ComPtr,
    language: *const u16,
    query: *const u16,
    flags: i32,
    ctx: ComPtr,
    enum_out: *mut ComPtr,
) -> i32;
type NextFn = unsafe extern "system" fn(
    this: ComPtr,
    timeout: i32,
    count: u32,
    objects: *mut ComPtr,
    returned: *mut u32,
) -> i32;
type GetFn = unsafe extern "system" fn(
    this: ComPtr,
    field: *const u16,
    flags: i32,
    value: *mut Variant,
    cim_type: *mut u32,
    flavor: *mut i32,
) -> i32;
type ReleaseFn = unsafe extern "system" fn(this: ComPtr) -> u32;

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(pv_reserved: *mut c_void, dw_co_init: u32) -> i32;
    fn CoUninitialize();
    fn CoCreateInstance(
        rclsid: *const Guid,
        p_unk_outer: ComPtr,
        dw_cls_context: u32,
        riid: *const Guid,
        ppv: *mut ComPtr,
    ) -> i32;
}

/// 按 vtable 槽位调用 COM 方法（IUnknown 3 个方法之后从 index 3 起）。
///
/// # Safety
/// - obj 必须为有效的 COM 对象指针（vtable 首字段）
/// - slot 必须 < vtable 实际方法数，且签名与目标方法一致（调用方保证）
/// 按 vtable 槽位调用 COM 方法（IUnknown 3 个方法之后从 index 3 起）。
///
/// # Safety
/// - obj 必须为有效的 COM 对象指针（vtable 首字段）
/// - slot 必须 < vtable 实际方法数，且与返回的精确函数指针类型匹配（调用方保证）
unsafe fn com_call<T>(obj: ComPtr, slot: usize) -> T {
    // SAFETY: COM 对象内存布局：首字段为 vtable 指针数组；slot 索引由调用方
    // 按接口规范（MSDN）核对，保证落在有效方法槽位；T 必须为对应方法的
    // 精确函数指针类型（调用方保证）
    unsafe {
        let vtbl = *(obj as *const *const core::ffi::c_void);
        std::mem::transmute_copy(&*vtbl.add(slot))
    }
}

/// 调用 COM 方法的 IUnknown::Release（所有接口的共同收尾）。
///
/// # Safety
/// obj 必须为有效 COM 对象指针
unsafe fn com_release(obj: ComPtr) {
    if obj.is_null() {
        return;
    }
    // SAFETY: Release 是 IUnknown 的第 3 个方法（vtable index 2）
    unsafe {
        let release: ReleaseFn = com_call(obj, 2);
        let _ = release(obj);
    }
}

// ============================================================================
// WMI 查询：ACPI 热区温度
// ============================================================================

/// 把 WMI 原始温度值（0.1 开尔文，uint32）转为摄氏温度。
///
/// 值 0 视为无效（部分固件以 0 表示"无数据"），返回 None。
pub fn kelvin_tenths_to_celsius(raw: u32) -> Option<f64> {
    if raw == 0 {
        return None;
    }
    let c = raw as f64 / 10.0 - 273.15;
    // 合理范围 [-60, 150] °C，超出视为无效（固件异常值防御）
    if (-60.0..=150.0).contains(&c) {
        Some(c)
    } else {
        None
    }
}

/// 从多个热区原始温度（0.1K）中取最大值；0 视为无效忽略。
///
/// 多热区主板只有其一有效时，取最大值最接近真实 CPU 热区（纯逻辑，可单测）。
fn max_valid_thermal_raw(values: &[u32]) -> Option<u32> {
    values.iter().copied().filter(|&v| v != 0).max()
}

/// 查询 ACPI 热区温度（摄氏）。通道不可用 / 固件无热区时返回 None。
///
/// 完整调用链：CoInitializeEx → CoCreateInstance(WbemLocator) →
/// ConnectServer(root\wmi) → ExecQuery(WQL) → Next → Get(CurrentTemperature)。
/// 任一步失败返回 None（本模块仅作兜底探活，不向调用方透传 HRESULT 细节）。
pub fn thermal_zone_temp() -> Option<f64> {
    // SAFETY: 参数为 NULL + COINIT_MULTITHREADED，符合 ole32 契约
    let hr = unsafe { CoInitializeEx(ptr::null_mut(), COINIT_MULTITHREADED) };
    if hr != S_OK && hr != S_FALSE {
        // 初始化失败（如 OLE 已以其他模式初始化）→ 通道不可用
        return None;
    }
    let result = query_thermal_zone();
    // SAFETY: 与 CoInitializeEx 成对调用，线程退出前清理 COM 状态
    unsafe {
        CoUninitialize();
    }
    result
}

/// 执行 WMI 查询的主体（在 CoInitializeEx 保护内调用）。
fn query_thermal_zone() -> Option<f64> {
    // 1. 创建 WbemLocator
    let mut locator: ComPtr = ptr::null_mut();
    // SAFETY: CLSID_WBEM_LOCATOR/IID_IWBEM_LOCATOR 均为标准常量；
    // ppv 指向栈上指针，由 COM 写入
    let hr = unsafe {
        CoCreateInstance(
            &CLSID_WBEM_LOCATOR,
            ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &CLSID_WBEM_LOCATOR,
            &mut locator,
        )
    };
    if hr != S_OK || locator.is_null() {
        return None;
    }

    // 2. ConnectServer(root\wmi)
    let mut services: ComPtr = ptr::null_mut();
    let ns = to_utf16("root\\wmi");
    let lang = to_utf16(WQL);
    // SAFETY: ConnectServer 是 IWbemLocator 的第 4 个方法（vtable index 3）；
    // 参数（MSDN）：strNetworkResource, strUser, strPassword, strLocale,
    // lSecurityFlags, strAuthority, pCtx, ppNamespace —— 顺序严格对齐
    let hr = unsafe {
        let connect: ConnectServerFn = com_call(locator, 3);
        connect(
            locator,
            ns.as_ptr(),
            ptr::null::<u16>(),
            ptr::null::<u16>(),
            ptr::null::<u16>(),
            0i32,
            ptr::null::<u16>(),
            ptr::null_mut::<c_void>(),
            &mut services,
        )
    };
    // SAFETY: locator 不再使用
    unsafe {
        com_release(locator);
    }
    if hr != S_OK || services.is_null() {
        return None;
    }

    // 3. ExecQuery(WQL, "SELECT * FROM MSAcpi_ThermalZoneTemperature")
    let query = to_utf16(QUERY);
    let mut enum_ptr: ComPtr = ptr::null_mut();
    // SAFETY: ExecQuery 是 IWbemServices 的第 21 个方法（vtable index 20，
    // IUnknown 0-2 + 18 个前置方法后，按 MSDN 接口方法顺序核对）；
    // 参数：语言, 查询, flags=WBEM_FLAG_FORWARD_ONLY, context=NULL, 枚举输出
    let hr = unsafe {
        let exec: ExecQueryFn = com_call(services, 20);
        exec(
            services,
            lang.as_ptr(),
            query.as_ptr(),
            WBEM_FLAG_FORWARD_ONLY,
            ptr::null_mut::<c_void>(),
            &mut enum_ptr,
        )
    };
    if hr != S_OK || enum_ptr.is_null() {
        // SAFETY: services 不再使用
        unsafe {
            com_release(services);
        }
        return None;
    }

    // 4. Next：循环枚举全部热区实例，取 CurrentTemperature 最大值
    //    （部分固件多热区只有其一有效，取最大最接近真实 CPU 热区）
    let mut raws: Vec<u32> = Vec::new();
    loop {
        let mut obj: ComPtr = ptr::null_mut();
        let mut ret_count: u32 = 0;
        // SAFETY: Next 是 IEnumWbemClassObject 的第 5 个方法（vtable index 4）；
        // 参数：timeout=WBEM_INFINITE, count=1, 对象数组输出, 实际返回数
        let hr = unsafe {
        let next: NextFn = com_call(enum_ptr, 4);
            next(enum_ptr, WBEM_INFINITE, 1, &mut obj, &mut ret_count)
        };
        // 枚举结束（S_FALSE）或出错 → 停止循环
        if hr != S_OK || ret_count == 0 || obj.is_null() {
            break;
        }
        // 5. Get("CurrentTemperature")
        let field = to_utf16("CurrentTemperature");
        let mut var = Variant {
            vt: 0,
            rsvd: 0,
            val: 0,
        };
        let mut cim_type: u32 = 0;
        // SAFETY: Get 是 IWbemClassObject 的第 5 个方法（vtable index 4）；
        // 参数：字段名, flags=0, VARIANT 输出, 类型输出, 语义输出(NULL)
        let hr = unsafe {
        let get: GetFn = com_call(obj, 4);
            get(
                obj,
                field.as_ptr(),
                0i32,
                &mut var as *mut Variant,
                &mut cim_type,
                ptr::null_mut::<i32>(),
            )
        };
        // SAFETY: obj 不再使用
        unsafe {
            com_release(obj);
        }
        if hr == S_OK && var.vt == VT_UI4 {
            raws.push(var.val as u32);
        }
    }
    // SAFETY: enum_ptr 不再使用
    unsafe {
        com_release(enum_ptr);
    }
    // SAFETY: services 不再使用
    unsafe {
        com_release(services);
    }
    let raw = max_valid_thermal_raw(&raws)?;
    kelvin_tenths_to_celsius(raw)
}

/// 把字符串转为 NUL 结尾的 UTF-16 缓冲区。
fn to_utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

// ============================================================================
// 单元测试（纯逻辑：温度换算防御）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kelvin_normal_room() {
        // 2950 = 295.0 K = 21.85 °C
        let c = kelvin_tenths_to_celsius(2950).unwrap();
        assert!((c - 21.85).abs() < 1e-9);
    }

    #[test]
    fn kelvin_hot_cpu() {
        // 3450 = 345.0 K = 71.85 °C
        let c = kelvin_tenths_to_celsius(3450).unwrap();
        assert!((c - 71.85).abs() < 1e-9);
    }

    #[test]
    fn kelvin_zero_invalid() {
        // 固件 0 = 无数据
        assert_eq!(kelvin_tenths_to_celsius(0), None);
    }

    #[test]
    fn kelvin_out_of_range_capped() {
        // 12345 = 1234.5 K → 961.35 °C，超出合理范围 → None
        assert_eq!(kelvin_tenths_to_celsius(12345), None);
    }

    #[test]
    fn max_thermal_raw_multi_zone() {
        // 多热区取最大值；0（固件无效）忽略
        assert_eq!(max_valid_thermal_raw(&[]), None);
        assert_eq!(max_valid_thermal_raw(&[0, 0]), None);
        assert_eq!(max_valid_thermal_raw(&[2950, 3450]), Some(3450));
        assert_eq!(max_valid_thermal_raw(&[3450, 0, 2980]), Some(3450));
        assert_eq!(max_valid_thermal_raw(&[0, 2900]), Some(2900));
    }

    #[test]
    fn max_thermal_raw_then_celsius() {
        // 组合链路：取最大原始值 → 转摄氏
        let raw = max_valid_thermal_raw(&[2950, 3450]).unwrap();
        let c = kelvin_tenths_to_celsius(raw).unwrap();
        assert!((c - 71.85).abs() < 1e-9);
    }
}
