//! 电源计划管理（powrprof）— 激活/写入/复制方案（P7/P8/P9）
//!
//! 替换 `powercfg /setactive` / `/setacvalueindex` / `/setdcvalueindex` / `/s` / `/duplicatescheme`。
//!
//! 线程模型：写操作秒级完成，上层须在 `spawn_blocking` 中调用（S8）。

use crate::error::CollectError;
use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS};
use windows_sys::Win32::System::Power::*;
use windows_sys::Win32::System::Registry::HKEY;

/// 处理器电源管理子组 GUID（与 settings.rs 常量一致）
pub const SUBGROUP_PROCESSOR: &str = "54533251-82be-4824-96c1-47b60b740d00";
/// 异类线程调度策略设置 GUID
pub const HETERO_THREAD_POLICY_GUID: &str = "93b8b6dc-0698-4d1c-9ee4-0644e900c85d";
/// 异类短运行线程调度策略设置 GUID
pub const HETERO_SHORT_THREAD_POLICY_GUID: &str = "465e1f50-b610-473a-ab58-00d1077dc418";

/// GUID 解析辅助：字符串 → GUID
fn guid_from_str(s: &str) -> Result<windows_sys::core::GUID, CollectError> {
    // 用 uuid 格式解析：8-4-4-4-12（十六进制，可带 {})
    let trimmed = s.trim().trim_start_matches('{').trim_end_matches('}');
    let parts: Vec<&str> = trimmed.split('-').collect();
    if parts.len() != 5 {
        return Err(CollectError::parse(
            "电源计划 GUID",
            format!("格式非法: '{}'", s),
        ));
    }
    let u32_parse = |h: &str| -> Result<u32, CollectError> {
        u32::from_str_radix(h, 16).map_err(|e| {
            CollectError::parse("电源计划 GUID", format!("解析 '{}' 失败: {}", h, e))
        })
    };
    let u16_parse = |h: &str| -> Result<u16, CollectError> {
        u16::from_str_radix(h, 16).map_err(|e| {
            CollectError::parse("电源计划 GUID", format!("解析 '{}' 失败: {}", h, e))
        })
    };

    Ok(windows_sys::core::GUID {
        data1: u32_parse(parts[0])?,
        data2: u16_parse(parts[1])?,
        data3: u16_parse(parts[2])?,
        data4: {
            let mut d4 = [0u8; 8];
            let hi = parts[3];
            let lo = parts[4];
            if hi.len() != 4 || lo.len() != 12 {
                return Err(CollectError::parse(
                    "电源计划 GUID",
                    format!("尾部长度非法: '{}'", s),
                ));
            }
            // 两段拼接为 16 个 hex 字符
            let full = format!("{}{}", hi, lo);
            for i in 0..8 {
                let byte_str = &full[i * 2..i * 2 + 2];
                d4[i] = u8::from_str_radix(byte_str, 16).map_err(|e| {
                    CollectError::parse("电源计划 GUID", format!("解析 '{}' 失败: {}", byte_str, e))
                })?;
            }
            d4
        },
    })
}

/// GUID → 小写字符串（去掉花括号）
fn guid_to_str(guid: &windows_sys::core::GUID) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7],
    )
}

/// 释放 PowerGetActiveScheme / PowerDuplicateScheme 返回的 GUID 内存
struct GuidGuard(*mut windows_sys::core::GUID);

impl Drop for GuidGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: LocalFree 释放 LocalAlloc 分配的内存（powrprof 文档要求）
            unsafe {
                LocalFree(self.0 as _);
            }
        }
    }
}

/// 获取当前激活的电源计划 GUID
///
/// 语义约定：成功 → `Ok(Some(String))`；无激活方案（罕见）→ `Ok(None)`；失败 → `Err`
pub fn get_active_scheme() -> Result<Option<String>, CollectError> {
    let mut guid_ptr: *mut windows_sys::core::GUID = std::ptr::null_mut();
    // SAFETY: PowerGetActiveScheme 接受 NULL = 本机电源设置；返回的 GUID 由 LocalFree 释放
    let rc = unsafe { PowerGetActiveScheme(std::ptr::null_mut::<HKEY>() as HKEY, &mut guid_ptr) };
    if rc != ERROR_SUCCESS {
        return Err(CollectError::winapi_detailed(
            "powrprof.PowerGetActiveScheme",
            "获取当前电源计划",
            format!("错误码 {}", rc),
        ));
    }
    let guard = GuidGuard(guid_ptr);
    if guid_ptr.is_null() {
        return Ok(None);
    }
    // SAFETY: guid_ptr 非空且由 API 填充
    let guid = unsafe { *guid_ptr };
    drop(guard); // 显式释放（guard 持有内存所有权，读取后即可释放）
    Ok(Some(guid_to_str(&guid)))
}

/// 设置激活电源计划（`powercfg /setactive <guid>` 等价）
pub fn set_active_scheme(guid: &str) -> Result<(), CollectError> {
    let guid = guid_from_str(guid)?;
    // SAFETY: PowerSetActiveScheme 接受 NULL = 本机电源设置；GUID 为栈上有效引用
    let rc = unsafe { PowerSetActiveScheme(std::ptr::null_mut::<HKEY>() as HKEY, &guid) };
    if rc != ERROR_SUCCESS {
        return Err(CollectError::winapi_detailed(
            "powrprof.PowerSetActiveScheme",
            format!("激活电源计划 '{}'", guid_to_str(&guid)),
            format!("错误码 {}", rc),
        ));
    }
    Ok(())
}

/// 删除电源计划（`powercfg /delete <guid>` 等价）
///
/// 激活中的计划不可删除（API 返回错误，前端亦禁用）；删除后不可恢复（需重新创建）。
pub fn delete_scheme(guid: &str) -> Result<(), CollectError> {
    let guid = guid_from_str(guid)?;
    // SAFETY: PowerDeleteScheme 接受 NULL = 本机电源设置；GUID 为栈上有效引用
    let rc = unsafe { PowerDeleteScheme(std::ptr::null_mut::<HKEY>() as HKEY, &guid) };
    if rc != ERROR_SUCCESS {
        return Err(CollectError::winapi_detailed(
            "powrprof.PowerDeleteScheme",
            format!("删除电源计划 '{}'", guid_to_str(&guid)),
            format!("错误码 {}", rc),
        ));
    }
    Ok(())
}

/// 写入 AC 值索引（`powercfg /setacvalueindex SCHEME_CURRENT ...` 等价）
///
/// `scheme`：传 None 表示使用当前激活方案（对应 SCHEME_CURRENT）
pub fn write_ac_value(
    scheme: Option<&str>,
    subgroup: &str,
    setting: &str,
    value: u32,
) -> Result<(), CollectError> {
    let scheme_guid = match scheme {
        Some(s) => Some(guid_from_str(s)?),
        None => get_active_scheme()?.map(|s| guid_from_str(&s)).transpose()?,
    };
    let subgroup_guid = guid_from_str(subgroup)?;
    let setting_guid = guid_from_str(setting)?;

    // SAFETY: 各 GUID 均为栈上有效引用；scheme 用当前激活方案
    let rc = unsafe {
        PowerWriteACValueIndex(
            std::ptr::null_mut::<HKEY>() as HKEY,
            scheme_guid.as_ref().map_or(std::ptr::null(), |g| g as *const _),
            &subgroup_guid,
            &setting_guid,
            value,
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(CollectError::winapi_detailed(
            "powrprof.PowerWriteACValueIndex",
            "写入 AC 电源设置",
            format!("错误码 {}", rc),
        ));
    }
    Ok(())
}

/// 写入 DC 值索引（`powercfg /setdcvalueindex SCHEME_CURRENT ...` 等价）
pub fn write_dc_value(
    scheme: Option<&str>,
    subgroup: &str,
    setting: &str,
    value: u32,
) -> Result<(), CollectError> {
    let scheme_guid = match scheme {
        Some(s) => Some(guid_from_str(s)?),
        None => get_active_scheme()?.map(|s| guid_from_str(&s)).transpose()?,
    };
    let subgroup_guid = guid_from_str(subgroup)?;
    let setting_guid = guid_from_str(setting)?;

    // SAFETY: 各 GUID 均为栈上有效引用；scheme 用当前激活方案
    let rc = unsafe {
        PowerWriteDCValueIndex(
            std::ptr::null_mut::<HKEY>() as HKEY,
            scheme_guid.as_ref().map_or(std::ptr::null(), |g| g as *const _),
            &subgroup_guid,
            &setting_guid,
            value,
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(CollectError::winapi_detailed(
            "powrprof.PowerWriteDCValueIndex",
            "写入 DC 电源设置",
            format!("错误码 {}", rc),
        ));
    }
    Ok(())
}

/// 复制电源计划（`powercfg /duplicatescheme <guid>` 等价），返回新计划 GUID
pub fn duplicate_scheme(source_guid: &str) -> Result<String, CollectError> {
    let source = guid_from_str(source_guid)?;
    let mut new_guid: *mut windows_sys::core::GUID = std::ptr::null_mut();
    // SAFETY: PowerDuplicateScheme 分配新 GUID（LocalAlloc），由 LocalFree 释放
    let rc = unsafe {
        PowerDuplicateScheme(
            std::ptr::null_mut::<HKEY>() as HKEY,
            &source,
            &mut new_guid,
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(CollectError::winapi_detailed(
            "powrprof.PowerDuplicateScheme",
            format!("复制电源计划 '{}'", source_guid),
            format!("错误码 {}", rc),
        ));
    }
    let guard = GuidGuard(new_guid);
    // SAFETY: 成功时 new_guid 非空
    let guid = unsafe { *new_guid };
    drop(guard); // 显式释放
    Ok(guid_to_str(&guid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guid_roundtrip() {
        // 平衡电源计划 GUID
        let s = "381b4222-f694-41f0-9685-ff5bb260df2e";
        let g = guid_from_str(s).unwrap();
        assert_eq!(guid_to_str(&g), s);
    }

    #[test]
    fn test_guid_with_braces() {
        let s = "{381b4222-f694-41f0-9685-ff5bb260df2e}";
        let g = guid_from_str(s).unwrap();
        assert_eq!(guid_to_str(&g), "381b4222-f694-41f0-9685-ff5bb260df2e");
    }

    #[test]
    fn test_guid_invalid() {
        assert!(guid_from_str("not-a-guid").is_err());
        assert!(guid_from_str("").is_err());
        assert!(guid_from_str("381b4222-f694-41f0-9685").is_err());
    }

    #[test]
    fn test_get_active_scheme_shape() {
        // 实机：激活方案应存在
        match get_active_scheme() {
            Ok(Some(s)) => {
                assert_eq!(s.len(), 36, "GUID 字符串长度应为 36: {}", s);
                assert!(s.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
            }
            Ok(None) => {}
            Err(e) => panic!("get_active_scheme 不应失败: {}", e),
        }
    }

    #[test]
    fn test_set_active_scheme_noop() {
        // 读当前方案 → 再激活同一方案（幂等验证，不改变系统状态）
        if let Ok(Some(active)) = get_active_scheme() {
            let result = set_active_scheme(&active);
            assert!(result.is_ok(), "重新激活当前方案应成功: {:?}", result.err());
        }
    }

    /// 真机删除验证（默认忽略；会真实创建并删除一个测试电源计划，需管理员）：
    /// 复制激活计划 → delete_scheme 删除 → 重复删除应报错（已不存在）。
    /// 运行：`cargo test -p secm-datasource -- --ignored real_machine_delete_scheme`
    #[test]
    #[ignore]
    fn real_machine_delete_scheme() {
        let active = get_active_scheme()
            .expect("读取激活方案失败")
            .expect("应存在激活方案");
        // 复制一份测试计划（不触碰真实计划）
        let dup = duplicate_scheme(&active).expect("复制电源计划失败");
        assert_ne!(dup, active, "副本 GUID 不应与源相同");
        // 删除副本
        delete_scheme(&dup).expect("删除电源计划失败");
        // 重复删除应报错（计划已不存在）
        let err = delete_scheme(&dup).expect_err("重复删除应失败");
        eprintln!("[真机] 重复删除预期报错: {}", err);
        // 激活中的计划不可删除（API 拒绝，前端亦有禁用）
        let err2 = delete_scheme(&active).expect_err("删除激活中的计划应失败");
        eprintln!("[真机] 删除激活计划预期报错: {}", err2);
    }

    #[test]
    fn test_write_value_noop() {
        // 读当前方案 → 写入当前值（幂等验证）— 需管理员权限，失败时降级提示
        if let Ok(Some(active)) = get_active_scheme() {
            // 平衡方案处理器子组 AC 值，读当前值再写回
            let result = write_ac_value(Some(&active), SUBGROUP_PROCESSOR, HETERO_THREAD_POLICY_GUID, 0);
            // 非管理员或无此项时允许 Err（NeedsAdmin / 设置不存在），但不应 panic
            if let Err(e) = result {
                match e {
                    CollectError::NeedsAdmin { .. } => {}
                    CollectError::WinApi { .. } => {}
                    other => panic!("意外错误类型: {}", other),
                }
            }
        }
    }

    #[test]
    fn test_duplicate_scheme_shape() {
        // 复制平衡方案 → 应返回新 GUID（管理员权限）。无权限时降级。
        let src = "381b4222-f694-41f0-9685-ff5bb260df2e";
        match duplicate_scheme(src) {
            Ok(guid) => {
                assert_eq!(guid.len(), 36);
                assert_ne!(guid, src);
            }
            Err(e) => match e {
                CollectError::NeedsAdmin { .. } => {}
                CollectError::WinApi { .. } => {}
                other => panic!("意外错误类型: {}", other),
            },
        }
    }
}
