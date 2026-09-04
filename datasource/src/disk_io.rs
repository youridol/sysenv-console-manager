//! 磁盘 IO 读写速度（PDH 性能计数器）— 每卷独立 MB/s
//!
//! 数据源：pdh.dll 的 `\PhysicalDisk(*)\Disk Read Bytes/sec` /
//! `\PhysicalDisk(*)\Disk Write Bytes/sec` 通配符计数器（Windows 标准性能
//! 计数器，任务管理器同源，普通用户可读，无需管理员权限 / 内核驱动）。
//!
//! 实例语义：PDH 的 PhysicalDisk 按「卷」提供实例，实例名形如 `0 C:`、
//! `1 D:`（物理盘号 + 盘符）。`PdhGetFormattedCounterArrayW` 返回全部实例的
//! 名称与值，与 sysinfo 的 `mount_point()`（如 `D:\`）按盘符归一化后匹配，
//! 从而每块硬盘获得各自独立的读写速度（v0.19.0 为 _Total 聚合均摊，已升级）。
//!
//! 时序语义：
//! - `PdhCollectQueryData` 两次调用间隔须 ≥1s 才能得到有效速率（PDH 内部采样）；
//!   SECM 传感器每秒轮询一次（`get_sensor_data` → `get_disk_data`），天然满足。
//! - 首次调用仅建立采样基线（返回空映射）；此后每次调用返回最近 ~1s 平均速率。
//! - 采样间隔不足（PDH_INVALID_DATA）时沿用上次有效值，避免抖动归零。
//! - 计数器不可用（虚拟机 / 无物理磁盘 / 被系统禁用）时静默降级空映射，
//!   不中断其余数据源（S8 降级语义）。
//!
//! 线程模型：同步阻塞 API，查询开销微秒级；调用方须在 `spawn_blocking` 中执行（S8）。

use std::collections::HashMap;
use std::sync::Mutex;
use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
};

/// PDH 成功状态码
const PDH_CSTATUS_VALID_DATA: u32 = 0x0000_0000;
const PDH_CSTATUS_NEW_DATA: u32 = 0x0000_0001;
/// 两次采样间隔不足（<1s）时单个值返回该状态
const PDH_INVALID_DATA: u32 = 0xC000_0BC6;
/// 缓冲不足（首次查询大小用）
const PDH_MORE_DATA: u32 = 0x8000_07D2;

/// 通配符计数器路径（`*` 展开为全部卷实例）
const COUNTER_READ: &str = r"\PhysicalDisk(*)\Disk Read Bytes/sec";
const COUNTER_WRITE: &str = r"\PhysicalDisk(*)\Disk Write Bytes/sec";

/// PDH 查询状态（进程级单例；查询句柄生命周期 = 进程生命周期，退出由 OS 回收）
struct PdhState {
    query: isize,
    read_counter: isize,
    write_counter: isize,
    /// 上次成功读取：卷盘符（"C:" 大写）→ (读 MB/s, 写 MB/s)；间隔不足时沿用
    speed_map: HashMap<String, (f32, f32)>,
    /// 初始化失败标记（计数器不存在等）——失败后不再重试
    failed: bool,
}

static PDH_STATE: Mutex<Option<PdhState>> = Mutex::new(None);

/// 字符串 → NUL 结尾 UTF-16。
fn to_utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// 初始化 PDH 查询（打开查询 + 添加读写通配计数器 + 建立首次采样基线）。
///
/// 计数器不存在（无物理磁盘/虚拟机）时返回 Err，调用方标记不可用。
fn pdh_init() -> Result<PdhState, String> {
    let mut query: isize = 0;
    // SAFETY: NULL 数据源 = 本地实时性能数据；query 由 API 写入
    let rc = unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut query) };
    if rc != 0 {
        return Err(format!("PdhOpenQueryW failed win32=0x{:08X}", rc));
    }
    let read_path = to_utf16(COUNTER_READ);
    let write_path = to_utf16(COUNTER_WRITE);
    let mut read_counter: isize = 0;
    let mut write_counter: isize = 0;
    // SAFETY: 计数器路径为 NUL 结尾 UTF-16；句柄由 API 写入
    let rc_read = unsafe { PdhAddEnglishCounterW(query, read_path.as_ptr(), 0, &mut read_counter) };
    let rc_write =
        unsafe { PdhAddEnglishCounterW(query, write_path.as_ptr(), 0, &mut write_counter) };
    if rc_read != 0 || rc_write != 0 {
        // SAFETY: query 为 PdhOpenQueryW 返回的有效句柄
        unsafe { PdhCloseQuery(query) };
        return Err(format!(
            "PdhAddEnglishCounterW failed read=0x{:08X} write=0x{:08X}",
            rc_read, rc_write
        ));
    }
    // 首次收集建立采样基线（此刻无速率数据，下轮起出数）
    // SAFETY: query 为有效句柄
    unsafe { PdhCollectQueryData(query) };
    Ok(PdhState {
        query,
        read_counter,
        write_counter,
        speed_map: HashMap::new(),
        failed: false,
    })
}

/// 读取通配计数器的全部实例值（实例名 → MB/s）。
///
/// 两次分配调用：先查所需缓冲大小，再填充数组；成功时逐个校验 CStatus。
///
/// ⚠ 缓冲布局：PDH 返回的条目数组为**变长布局**（每个 `PDH_FMT_COUNTERVALUE_ITEM_W`
/// 后紧跟该实例的 UTF-16 名称串），所需缓冲字节数 > 条目数 × 结构大小；
/// 必须按第一次调用返回的 `buf_size` 字节数分配，否则 PDH 越界写入导致
/// 0xc0000005 崩溃（v0.20.0 闪退事故根因，见 Application Error 1000）。
fn read_counter_array_mbps(counter: isize) -> HashMap<String, f32> {
    let mut buf_size: u32 = 0;
    let mut item_count: u32 = 0;
    // SAFETY: 首次调用仅查询大小（buffer 为 null）
    let rc = unsafe {
        PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            &mut buf_size,
            &mut item_count,
            std::ptr::null_mut(),
        )
    };
    // PDH_MORE_DATA 是首次查询的预期返回（缓冲不足）；其余非 0 视为失败
    if rc != PDH_MORE_DATA && rc != PDH_CSTATUS_VALID_DATA && rc != PDH_CSTATUS_NEW_DATA {
        return HashMap::new();
    }
    if buf_size == 0 || item_count == 0 {
        return HashMap::new();
    }
    // 按所需**字节数**分配（含实例名串），元素为对齐正确的结构类型；
    // 零值初始化（POD 类型，零值合法），PDH 成功返回后才读取。
    let capacity = (buf_size as usize).div_ceil(std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>());
    // SAFETY: PDH_FMT_COUNTERVALUE_ITEM_W 为 repr(C) 数值/指针 POD，零值合法；
    // 此处仅作输出缓冲，API 返回成功（下方 rc 校验）后才读取内容
    let zero = unsafe { std::mem::zeroed::<PDH_FMT_COUNTERVALUE_ITEM_W>() };
    let mut buf = vec![zero; capacity.max(item_count as usize)];
    let mut out_size: u32 = buf_size;
    let mut out_count: u32 = item_count;
    // SAFETY: buf 为对齐正确的输出缓冲，容量 >= buf_size 字节
    let rc = unsafe {
        PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            &mut out_size,
            &mut out_count,
            buf.as_mut_ptr(),
        )
    };
    if rc != PDH_CSTATUS_VALID_DATA && rc != PDH_CSTATUS_NEW_DATA {
        return HashMap::new();
    }
    let mut map = HashMap::new();
    for item in buf.iter().take(out_count as usize) {
        // 采样间隔不足/无效的单项跳过
        if item.FmtValue.CStatus == PDH_INVALID_DATA {
            continue;
        }
        // SAFETY: CStatus 有效时 doubleValue 已由 API 写入
        let bps = unsafe { item.FmtValue.Anonymous.doubleValue };
        if !bps.is_finite() || bps < 0.0 {
            continue;
        }
        // 实例名：NUL 结尾 UTF-16（指向缓冲内），解析为 String
        // SAFETY: szName 由 API 填充，指向缓冲内 NUL 结尾字符串
        let name = unsafe {
            let mut end = 0usize;
            while *item.szName.add(end) != 0 {
                end += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(item.szName, end))
        };
        // 实例名形如 "0 C:"（盘号 + 盘符）；提取字母开头的盘符 token
        if let Some(drive) = name
            .split_whitespace()
            .find(|t| t.len() == 2 && t.as_bytes()[1] == b':' && t.as_bytes()[0].is_ascii_alphabetic())
        {
            map.insert(drive.to_ascii_uppercase(), (bps / 1024.0 / 1024.0) as f32);
        }
    }
    map
}

/// 获取各卷磁盘 IO 速度（键为盘符 "C:" 形式，大写）。
///
/// 返回 盘符 → (读 MB/s, 写 MB/s)；通道不可用/初始化失败时返回空映射
/// （降级不中断，调用方按盘符匹配不到即为 0）。
pub fn get_disk_io_speed_map() -> HashMap<String, (f32, f32)> {
    let mut guard = match PDH_STATE.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if let Some(s) = guard.as_mut() {
        if s.failed {
            return HashMap::new();
        }
        // 每次调用收集新样本：首次调用（init）已建立基线，此后每次 Collect
        // 后即可取最近 ~1s 平均速率（间隔不足时单项 CStatus=PDH_INVALID_DATA，
        // 该实例沿用上次值）
        // SAFETY: query 为有效句柄
        unsafe { PdhCollectQueryData(s.query) };
        let reads = read_counter_array_mbps(s.read_counter);
        let writes = read_counter_array_mbps(s.write_counter);
        // 按实例名合并读/写
        let mut next: HashMap<String, (f32, f32)> = HashMap::new();
        for (drive, r) in reads {
            let w = writes.get(&drive).copied().unwrap_or(0.0);
            next.insert(drive, (r, w));
        }
        for (drive, w) in writes {
            next.entry(drive).or_insert((0.0, w));
        }
        // 本次无有效数据的实例沿用上次值（间隔不足场景）
        for (drive, (r, w)) in &s.speed_map {
            next.entry(drive.clone()).or_insert((*r, *w));
        }
        s.speed_map = next;
        return s.speed_map.clone();
    }
    // 首次调用：初始化
    match pdh_init() {
        Ok(state) => {
            *guard = Some(state);
            HashMap::new()
        }
        Err(e) => {
            eprintln!("[disk_io] PDH 初始化失败（磁盘 IO 速度降级为 0）: {e}");
            *guard = Some(PdhState {
                query: 0,
                read_counter: 0,
                write_counter: 0,
                speed_map: HashMap::new(),
                failed: true,
            });
            HashMap::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机验证（默认忽略；PDH 通道真实采样，无需管理员）：
    /// 首次调用建立基线（空映射），间隔 >1s 后的调用应返回非空实例映射，
    /// 且每个值均为非负有限速率（磁盘空闲时为 0）。
    /// 运行：`cargo test -p secm-datasource -- --ignored real_machine_disk_io_speed_map`
    #[test]
    #[ignore]
    fn real_machine_disk_io_speed_map() {
        let m1 = get_disk_io_speed_map();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let m2 = get_disk_io_speed_map();
        eprintln!("[真机] disk_io 首调实例数={} 次调实例数={}", m1.len(), m2.len());
        for (drive, (r, w)) in &m2 {
            eprintln!("[真机] disk_io {drive}: read={r:.2} write={w:.2} MB/s");
            assert!(r.is_finite() && *r >= 0.0, "读速度非法: {}", r);
            assert!(w.is_finite() && *w >= 0.0, "写速度非法: {}", w);
            // 盘符键格式校验："C:" 形式
            assert_eq!(drive.len(), 2, "盘符键格式非法: {}", drive);
        }
        assert!(!m2.is_empty(), "应至少解析到一个磁盘实例");
    }

    /// 实例名解析单元测试（纯逻辑）
    #[test]
    fn test_instance_name_parsing() {
        // 从 "0 C:" / "1 D:" / "0 C: D:"（同盘多卷，取首个）提取盘符
        fn parse(name: &str) -> Option<&str> {
            name.split_whitespace().find(|t| {
                t.len() == 2 && t.as_bytes()[1] == b':' && t.as_bytes()[0].is_ascii_alphabetic()
            })
        }
        assert_eq!(parse("0 C:"), Some("C:"));
        assert_eq!(parse("1 D:"), Some("D:"));
        assert_eq!(parse("0 C: D:"), Some("C:"));
        assert_eq!(parse("3 X:"), Some("X:"));
        assert_eq!(parse(""), None);
        assert_eq!(parse("0"), None);
    }
}
