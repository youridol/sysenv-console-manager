//! 运行数据目录解析 —— 程序运行数据落点的唯一真源。
//!
//! 背景（2026-09-23 实测）：原实现把用户日志、诊断日志、图表历史、驱动日志与
//! sidecar 输出分别写到 `%APPDATA%\SECM\{logs,diag}` 与 `%TEMP%`。这违反项目目录模型
//! （程序运行数据应落在项目运行目录），并产生两类可复现故障：
//!
//! 1. 受限完整性环境下写入失败。本仓库工作区目录带 `Mandatory Label\Low Mandatory Level`
//!    标签，从该目录启动的进程即低完整性进程，实测无法在 `%TEMP%` 建目录
//!    （`create_dir_all` 返回 OS 错误 5「拒绝访问」），也无法向 `%APPDATA%\SECM\logs`
//!    追加（断言依赖的标记行不出现）；同一二进制复制到无该标签的目录后两者均成功。
//! 2. APPDATA 变量缺失时 `unwrap_or(".")` 把运行数据落到当前工作目录，实测产生了源码树内
//!    未跟踪的 `src-tauri/SECM/{diag,logs}` 残留。
//!
//! 解析顺序（见 `resolve_runtime_root`）：
//! 1. `SECM_RUNTIME_DIR` 环境变量（测试与排障注入；必须为非空绝对路径）；
//! 2. debug 构建：自当前可执行文件上溯定位含 `package.json` 与 `src-tauri` 的项目根，
//!    使用 `<项目根>/build/runtime`；
//! 3. release 构建：优先可创建且可写的 `<exe 同级>/runtime`（便携版与 currentUser
//!    安装布局均可写），探测失败回退 `%APPDATA%\SECM`（安装到只读位置时保持可用）。
//!
//! 本模块禁止在路径解析过程中写日志：`debug_warn!` 会经 `temp_data` 间接回调本模块，
//! 造成无限递归。目录创建结果改由 `try_init` 显式返回，由启动路径统一上报。
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::Serialize;

/// 运行目录注入环境变量（测试与排障用；仅接受非空绝对路径）。
pub const RUNTIME_DIR_ENV: &str = "SECM_RUNTIME_DIR";

/// 运行数据落点信息（`get_runtime_paths` 返回，前端展示真实路径）。
#[derive(Debug, Clone, Serialize)]
pub struct RuntimePathsInfo {
    /// 运行根目录
    pub root: String,
    /// 用户日志目录
    pub logs: String,
    /// 诊断日志目录
    pub diag: String,
    /// LHM sidecar 输出日志文件
    pub sidecar_log: String,
}

/// 导出当前运行数据落点（供前端展示，避免界面硬编码路径）。
pub fn info() -> RuntimePathsInfo {
    RuntimePathsInfo {
        root: runtime_root().display().to_string(),
        logs: logs_dir().display().to_string(),
        diag: diag_dir().display().to_string(),
        sidecar_log: sidecar_log_path().display().to_string(),
    }
}

/// 回退目录在 `%APPDATA%` 下的子目录名（与历史布局一致）。
const LEGACY_SUBDIR: &str = "SECM";

/// 迁移完成标记文件名（位于 `state/`）。
const MIGRATION_MARKER: &str = "migrated-from-appdata.marker";

/// 解析后的运行根目录（进程内缓存一次）。
static RUNTIME_ROOT: LazyLock<PathBuf> = LazyLock::new(resolve_runtime_root);

/// 当前生效的运行根目录。
///
/// 目录本身可能尚未创建；使用 `try_init` 在启动阶段统一创建并取得结果。
pub fn runtime_root() -> &'static Path {
    &RUNTIME_ROOT
}

/// 用户日志目录（`<runtime>/logs`）：用户可见日志与 sidecar 输出。
pub fn logs_dir() -> PathBuf {
    RUNTIME_ROOT.join("logs")
}

/// 诊断目录（`<runtime>/diag`）：debug / sensor / driver 日志与清理前备份。
pub fn diag_dir() -> PathBuf {
    RUNTIME_ROOT.join("diag")
}

/// 持久状态目录（`<runtime>/state`）。
pub fn state_dir() -> PathBuf {
    RUNTIME_ROOT.join("state")
}

/// 数据目录（`<runtime>/data`）。
pub fn data_dir() -> PathBuf {
    RUNTIME_ROOT.join("data")
}

/// 缓存目录（`<runtime>/cache`）。
pub fn cache_dir() -> PathBuf {
    RUNTIME_ROOT.join("cache")
}

/// LHM sidecar 标准输出重定向目标（原为 `%TEMP%\secm-lhm-sidecar.log`）。
pub fn sidecar_log_path() -> PathBuf {
    logs_dir().join("secm-lhm-sidecar.log")
}

/// 历史布局根目录（`%APPDATA%\SECM`）。
///
/// 仅两处用途：迁移旧数据，以及在诊断信息中说明旧位置。不用于新增写入。
pub fn legacy_appdata_root() -> Option<PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    if appdata.trim().is_empty() {
        return None;
    }
    Some(PathBuf::from(appdata).join(LEGACY_SUBDIR))
}

/// 创建全部运行子目录。返回失败项（目录名 + 原因），空表示全部就绪。
///
/// 由启动路径调用以把创建失败显式暴露给日志；不在此处写日志以避免递归。
pub fn try_init() -> Vec<(String, String)> {
    let mut failures = Vec::new();
    let targets: [(&str, PathBuf); 5] = [
        ("logs", logs_dir()),
        ("diag", diag_dir()),
        ("state", state_dir()),
        ("data", data_dir()),
        ("cache", cache_dir()),
    ];
    for (name, path) in targets {
        if let Err(e) = std::fs::create_dir_all(&path) {
            failures.push((name.to_string(), e.to_string()));
        }
    }
    failures
}

/// 迁移历史 `%APPDATA%\SECM` 数据到新运行目录（幂等，只复制不删除源）。
///
/// 语义：
/// - 已存在标记文件时直接返回（不重复迁移）；
/// - 仅复制目标目录中**尚不存在**的文件，不覆盖新数据；
/// - 复制失败逐项记录并继续，单项失败不影响其余文件；
/// - 源目录不存在视为无需迁移，仍写入标记。
///
/// 返回 `(复制文件数, 失败项)`。失败项为空表示迁移完整。
pub fn migrate_legacy_data() -> (usize, Vec<String>) {
    let marker = state_dir().join(MIGRATION_MARKER);
    if marker.exists() {
        return (0, Vec::new());
    }

    let mut copied = 0usize;
    let mut failures: Vec<String> = Vec::new();

    if let Some(legacy) = legacy_appdata_root() {
        // 旧的 logs/ 与 diag/ 各自按目录整体迁移到新布局的同名子目录。
        for sub in ["logs", "diag"] {
            let src = legacy.join(sub);
            if !src.is_dir() {
                continue;
            }
            let dst = RUNTIME_ROOT.join(sub);
            if let Err(e) = std::fs::create_dir_all(&dst) {
                failures.push(format!("创建 {} 失败: {}", dst.display(), e));
                continue;
            }
            copy_missing_files(&src, &dst, &mut copied, &mut failures);
        }
    }

    // 标记写入失败不视为迁移失败：下次启动会重试（复制幂等，不会重复计数）。
    if let Err(e) = std::fs::create_dir_all(state_dir()) {
        failures.push(format!("创建 state 目录失败: {}", e));
    } else if let Err(e) = std::fs::write(&marker, MIGRATION_MARKER.as_bytes()) {
        failures.push(format!("写入迁移标记失败: {}", e));
    }

    (copied, failures)
}

/// 递归复制 `src` 下目标不存在的文件到 `dst`（不覆盖，不删除）。
fn copy_missing_files(src: &Path, dst: &Path, copied: &mut usize, failures: &mut Vec<String>) {
    let entries = match std::fs::read_dir(src) {
        Ok(e) => e,
        Err(e) => {
            failures.push(format!("读取 {} 失败: {}", src.display(), e));
            return;
        }
    };
    for entry in entries.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        match entry.file_type() {
            Ok(t) if t.is_dir() => {
                if let Err(e) = std::fs::create_dir_all(&to) {
                    failures.push(format!("创建 {} 失败: {}", to.display(), e));
                    continue;
                }
                copy_missing_files(&from, &to, copied, failures);
            }
            Ok(t) if t.is_file() => {
                if to.exists() {
                    continue;
                }
                match std::fs::copy(&from, &to) {
                    Ok(_) => *copied += 1,
                    Err(e) => failures.push(format!(
                        "复制 {} -> {} 失败: {}",
                        from.display(),
                        to.display(),
                        e
                    )),
                }
            }
            _ => {}
        }
    }
}

// ============================================================================
// 解析实现
// ============================================================================

/// 解析运行根目录（进程内仅执行一次）。
fn resolve_runtime_root() -> PathBuf {
    // 1. 显式注入（仅接受非空绝对路径，避免把相对路径解析到当前工作目录）
    if let Ok(v) = std::env::var(RUNTIME_DIR_ENV) {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            let p = PathBuf::from(trimmed);
            if p.is_absolute() {
                return p;
            }
        }
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));

    // 2. debug 构建：项目内 build/runtime
    #[cfg(debug_assertions)]
    if let Some(dir) = exe_dir.as_deref().and_then(find_project_root) {
        return dir.join("build").join("runtime");
    }

    // 3. release 构建：exe 同级 runtime（可写则采用），否则回退 %APPDATA%\SECM
    if let Some(exe_dir) = exe_dir.as_deref() {
        let candidate = exe_dir.join("runtime");
        if is_usable_runtime_dir(&candidate) {
            return candidate;
        }
    }

    if let Some(legacy) = legacy_appdata_root() {
        return legacy;
    }

    // 4. 最后兜底：exe 同级 runtime（%APPDATA% 不可得时的唯一可用位置）
    exe_dir
        .unwrap_or_else(|| PathBuf::from("."))
        .join("runtime")
}

/// 自 `start` 上溯查找项目根（同时含 `package.json` 与 `src-tauri` 目录）。
///
/// 测试二进制位于 `<root>/src-tauri/target/debug/deps/`，上溯四层即可命中。
#[cfg(debug_assertions)]
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(dir) = cur {
        if dir.join("package.json").is_file() && dir.join("src-tauri").is_dir() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// 判定候选运行目录是否可创建且可写（release 采用前的实际探测）。
fn is_usable_runtime_dir(candidate: &Path) -> bool {
    if std::fs::create_dir_all(candidate).is_err() {
        return false;
    }
    let probe = candidate.join(".secm-write-probe");
    match std::fs::write(&probe, b"probe") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 注入必须为绝对路径才生效：相对路径会被忽略而回落到常规解析。
    ///
    /// 该用例不修改全局环境（解析结果进程内已缓存），只验证判定函数本身。
    #[test]
    fn absolute_path_detection() {
        assert!(PathBuf::from(r"C:\secm-probe").is_absolute());
        assert!(!PathBuf::from("relative/dir").is_absolute());
    }

    /// 运行根目录必须为绝对路径（相对路径会把运行数据落到工作目录）。
    #[test]
    fn runtime_root_is_absolute() {
        assert!(
            runtime_root().is_absolute(),
            "运行根目录必须为绝对路径，实际: {}",
            runtime_root().display()
        );
    }

    /// 各子目录均位于运行根目录之下（保证不越界到系统目录）。
    #[test]
    fn subdirs_are_under_runtime_root() {
        let root = runtime_root();
        for p in [logs_dir(), diag_dir(), state_dir(), data_dir(), cache_dir()] {
            assert!(
                p.starts_with(root),
                "子目录 {} 不在运行根目录 {} 之下",
                p.display(),
                root.display()
            );
        }
        assert!(sidecar_log_path().starts_with(logs_dir()));
    }

    /// sidecar 日志不再落在系统临时目录。
    #[test]
    fn sidecar_log_not_in_system_temp() {
        let p = sidecar_log_path();
        let temp = std::env::temp_dir();
        assert!(
            !p.starts_with(&temp),
            "sidecar 日志仍在系统临时目录: {}",
            p.display()
        );
        assert_eq!(
            p.file_name().and_then(|s| s.to_str()),
            Some("secm-lhm-sidecar.log")
        );
    }

    /// 项目根定位：测试二进制上溯应命中含 package.json 与 src-tauri 的目录。
    #[test]
    fn project_root_detected_from_test_binary() {
        let exe = std::env::current_exe().expect("current_exe");
        let dir = exe.parent().expect("exe dir");
        let root = find_project_root(dir).expect("应能自测试二进制上溯定位项目根");
        assert!(root.join("src-tauri").is_dir());
        assert!(root.join("package.json").is_file());
    }
}
