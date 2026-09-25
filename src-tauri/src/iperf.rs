//! iperf3 带宽测试模块 — 客户端全参数测试 + 本机服务端 + 结构化结果解析。
//!
//! ## 契约来源
//!
//! 参数与语义对照 esnet/iperf 官方手册（`src/iperf3.1`、`docs/invoking.rst`）。
//! 本模块只做**参数编排 + 输出解析 + 进程生命周期**，不重新实现任何测量逻辑：
//! 实际测量一律由 iperf3 本体完成（与 README「保留外部进程 K1」的定位一致）。
//!
//! ## 实测确认的输出形状（iperf 3.21 / Windows Cygwin 构建，本机 loopback）
//!
//! **`--json-stream`（实时逐行，首选）** —— 每行一个对象，外层包装键是
//! `{"event": ..., "data": ...}`（**不是**顶层 `start`/`intervals`/`end`）：
//!
//! ```text
//! {"event":"start","data":{"version":"iperf 3.21","test_start":{"protocol":"TCP","num_streams":1},...}}
//! {"event":"interval","data":{"streams":[...],"sum":{...},"sum_bidir_reverse":{...}}}
//! {"event":"end","data":{"streams":[...],"sum_sent":{...},"sum_received":{...},
//!                        "sum_sent_bidir_reverse":{...},"sum_received_bidir_reverse":{...}}}
//! {"event":"error","data":"unable to connect to server ..."}   // data 是**字符串**
//! ```
//!
//! **`-J`（结束时一次性输出）** —— 单个多行 JSON 文档，顶层键为
//! `start` / `intervals` / `end`，`end` 内层键与上方 `end.data` 相同。
//!
//! 关键边界（实测，勿假设）：
//! - `error` 事件的 `data` 是**字符串**而非对象；必须与对象分支分开解析。
//! - 出错时仍会打印 `{"event":"end","data":{}}`（**空对象**）→ 解析器必须容忍
//!   无 `sum_sent` 的 `end`，否则会把失败当成功。
//! - 纯文本模式（`<3.17` 无 `--json-stream`）下错误走 **stderr**，stdout 为空。
//! - 连接失败时退出码为 1，且 **stdout 的 JSON 里就有错误原因**（不是只靠 stderr）。
//!
//! ## 已知环境限制（如实报告，不假装支持）
//!
//! 本机 Cygwin 构建下 `--udp --bidir` 稳定失败：
//! `unable to read from stream socket: Resource temporarily unavailable`。
//! 该组合仍是合法 CLI 参数（iperf3 ≥3.7 支持），故**不阻止**用户选择；
//! iperf3 的报错会经 `error` 事件如实透出，UI 不会伪造结果。

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex as StdMutex};

use crate::network::StreamEvent;

// ============================================================================
// 常量
// ============================================================================

/// CREATE_NO_WINDOW：GUI 应用 spawn 控制台程序时不弹黑窗（全仓统一方案）。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 客户端结果里最多保留的诊断消息条数（错误/告警原文，防无界增长）。
const MAX_MESSAGES: usize = 50;

/// 服务端输出环形缓冲上限（1 小时测试的逐秒报告也不会撑爆内存）。
const SERVER_LINE_CAPACITY: usize = 500;

/// 默认端口（iperf3 官方默认）。
const DEFAULT_PORT: u16 = 5201;

/// 防火墙规则名前缀（固定前缀便于成对移除，且用户可辨识来源）。
pub const FIREWALL_RULE_PREFIX: &str = "SECM iperf3";

/// `--json-stream` 需要的最低 iperf3 版本（3.17 引入该选项）。
const JSON_STREAM_MIN: Iperf3Version = Iperf3Version {
    major: 3,
    minor: 17,
    patch: 0,
};

/// `--dscp` 需要的最低版本（3.2 引入；此前只有 `-S/--tos`）。
const DSCP_MIN: Iperf3Version = Iperf3Version {
    major: 3,
    minor: 2,
    patch: 0,
};

/// `--bidir` 需要的最低版本（3.7 引入）。
const BIDIR_MIN: Iperf3Version = Iperf3Version {
    major: 3,
    minor: 7,
    patch: 0,
};

/// 探测到的 iperf3 版本未知时的占位（不阻断测试，按最保守参数集运行）。
const UNKNOWN_VERSION: Iperf3Version = Iperf3Version {
    major: 0,
    minor: 0,
    patch: 0,
};

// ============================================================================
// 版本
// ============================================================================

/// iperf3 版本号（纯数值，`Copy`，可作为 const 用于版本门控）。
///
/// `major == 0` 表示**未知**（`--version` 解析失败），此时所有版本门控都按
/// 「不可用」处理（最保守），而不是乐观假设支持——避免给旧版传它不认识的参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Iperf3Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

/// 版本探测结果：归一化版本号 + `--version` 首行原文（UI 展示与排障用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Iperf3VersionProbe {
    pub version: Iperf3Version,
    pub raw: String,
}

impl Iperf3Version {
    pub fn is_known(&self) -> bool {
        self.major > 0
    }

    /// `self >= other`。未知版本恒为 `false`（保守）。
    fn at_least(&self, other: Iperf3Version) -> bool {
        self.is_known()
            && (self.major, self.minor, self.patch) >= (other.major, other.minor, other.patch)
    }

    pub fn supports_json_stream(&self) -> bool {
        self.at_least(JSON_STREAM_MIN)
    }
    pub fn supports_dscp(&self) -> bool {
        self.at_least(DSCP_MIN)
    }
    pub fn supports_bidir(&self) -> bool {
        self.at_least(BIDIR_MIN)
    }

    pub fn label(&self) -> String {
        if self.is_known() {
            format!("{}.{}.{}", self.major, self.minor, self.patch)
        } else {
            "未知".to_string()
        }
    }
}

/// 解析 `iperf3 --version` 输出（返回数值版本 + 首行原文）。
///
/// 实测两种格式都出现过：
/// - `iperf 3.21 (cJSON 1.7.15)`
/// - `iperf version 3.17 (cJSON 1.7.15)`
///
/// 解析失败返回未知版本（不 panic、不猜）。
pub fn parse_version(output: &str) -> Iperf3VersionProbe {
    let first = output.lines().next().unwrap_or("").trim();
    let mut parts = first.split_whitespace();
    // 跳过 "iperf"，再跳过可选的 "version"
    let _ = parts.next();
    let candidate = match parts.next() {
        Some("version") | Some("Version") => parts.next(),
        other => other,
    };
    let raw = first.to_string();
    let unknown = Iperf3VersionProbe {
        version: UNKNOWN_VERSION,
        raw: raw.clone(),
    };
    let Some(token) = candidate else {
        return unknown;
    };
    // token 形如 "3.21" / "3.21.0"，也可能带前缀 "v"
    let token = token.trim_start_matches('v');
    let mut nums = token.split('.').map(|p| {
        p.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
    });
    let major = nums.next().and_then(|s| s.parse::<u32>().ok());
    let minor = nums.next().and_then(|s| s.parse::<u32>().ok());
    let patch = nums.next().and_then(|s| s.parse::<u32>().ok());
    match (major, minor) {
        (Some(major), Some(minor)) if major > 0 => Iperf3VersionProbe {
            version: Iperf3Version {
                major,
                minor,
                patch: patch.unwrap_or(0),
            },
            raw,
        },
        _ => unknown,
    }
}

// 此处曾有 `parse_version_number()`：全局检索确认调用点为 0（含测试）；
// 需要版本号的调用方直接用 `parse_version(..).version` 或 `resolved()`（ADR-0001 D13）。

// ============================================================================
// 可执行文件定位
// ============================================================================

/// 缓存已解析的 exe 路径与版本（进程内只解析一次，可显式失效）。
static CACHE: StdMutex<Option<(Option<PathBuf>, Iperf3VersionProbe)>> = StdMutex::new(None);

/// 清空 exe/版本缓存（测试注入、安装完成后重试等场景）。
pub fn invalidate_cache() {
    if let Ok(mut c) = CACHE.lock() {
        *c = None;
    }
}

/// 测试/排障用的路径覆盖（仅 debug 构建生效，release 不读取，防注入面）。
#[cfg(debug_assertions)]
static EXE_OVERRIDE: StdMutex<Option<PathBuf>> = StdMutex::new(None);

/// 显式指定 iperf3 路径（测试专用；release 构建下为空操作）。
pub fn set_exe_override(path: Option<PathBuf>) {
    #[cfg(debug_assertions)]
    {
        *EXE_OVERRIDE.lock().unwrap_or_else(|p| p.into_inner()) = path;
        invalidate_cache();
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = path;
    }
}

/// iperf3 可执行文件名（Windows 为 `.exe`）。
#[cfg(windows)]
const EXE_NAME: &str = "iperf3.exe";
#[cfg(not(windows))]
const EXE_NAME: &str = "iperf3";

/// 定位 iperf3 可执行文件。
///
/// 解析顺序（镜像 `lhm::resolve_lhm_exe` 的 4 段式布局）：
/// 1. 显式覆盖（仅 debug；测试注入用）
/// 2. `SECM_IPERF3_DIR` 环境变量（仅 debug）
/// 3. Tauri `resource_dir()/iperf3`（NSIS 安装布局）
/// 4. exe 同目录 `resources/iperf3`（便携版布局）
/// 5. 当前工作目录 `src-tauri/resources/iperf3`（`cargo run` 开发布局）
/// 6. 系统 PATH（保留兼容：用户已自装 iperf3 时直接可用）
///
/// **不回退到任意用户可写目录**：路径只来自上述受信位置，
/// 前端无法指定可执行文件（否则本命令会变成任意程序执行器）。
pub fn resolve_iperf3_exe(app: Option<&tauri::AppHandle>) -> Option<PathBuf> {
    // 1. 显式覆盖（测试）
    #[cfg(debug_assertions)]
    if let Some(p) = EXE_OVERRIDE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
    {
        if p.is_file() {
            return Some(p);
        }
    }

    // 2. 环境变量注入（仅 debug；release 不读取）
    #[cfg(debug_assertions)]
    if let Ok(dir) = std::env::var("SECM_IPERF3_DIR") {
        if !dir.is_empty() {
            let exe = PathBuf::from(&dir).join(EXE_NAME);
            if exe.is_file() {
                return Some(exe);
            }
        }
    }

    // 3. Tauri resource_dir（NSIS 安装布局）
    if let Some(app) = app {
        use tauri::Manager;
        if let Ok(res) = app.path().resource_dir() {
            let exe = res.join("iperf3").join(EXE_NAME);
            if exe.is_file() {
                return Some(exe);
            }
        }
    }

    // 4. exe 同目录 / 当前目录（便携 + 开发布局）
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    for base in [exe_dir, PathBuf::from(".")] {
        let exe = base.join("resources").join("iperf3").join(EXE_NAME);
        if exe.is_file() {
            return Some(exe);
        }
    }

    // 5. 开发运行目录（cargo run 场景：cwd = src-tauri）
    let dev = PathBuf::from(".")
        .join("src-tauri")
        .join("resources")
        .join("iperf3")
        .join(EXE_NAME);
    if dev.is_file() {
        return Some(dev);
    }

    // 6. 系统 PATH（用户自装场景）
    if let Some(found) = find_on_path(EXE_NAME) {
        return Some(found);
    }

    None
}

/// 在 PATH 中查找可执行文件（不依赖外部 `where`，避免再起进程）。
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// 解析并缓存 (exe 路径, 版本探测结果)。
fn resolved(app: Option<&tauri::AppHandle>) -> (Option<PathBuf>, Iperf3VersionProbe) {
    if let Ok(c) = CACHE.lock() {
        if let Some(hit) = c.as_ref() {
            return hit.clone();
        }
    }
    let exe = resolve_iperf3_exe(app);
    let probe = match &exe {
        Some(p) => probe_version(p),
        None => Iperf3VersionProbe {
            version: UNKNOWN_VERSION,
            raw: String::new(),
        },
    };
    let pair = (exe, probe);
    if let Ok(mut c) = CACHE.lock() {
        *c = Some(pair.clone());
    }
    pair
}

/// 调用 `--version` 并解析。
fn probe_version(exe: &Path) -> Iperf3VersionProbe {
    match Command::new(exe)
        .arg("--version")
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).to_string();
            text.push('\n');
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            parse_version(&text)
        }
        Err(_) => Iperf3VersionProbe {
            version: UNKNOWN_VERSION,
            raw: String::new(),
        },
    }
}

/// 获取（并缓存）iperf3 可执行文件路径。
pub fn iperf3_exe(app: Option<&tauri::AppHandle>) -> Option<PathBuf> {
    resolved(app).0
}

/// 查询 iperf3 版本；二进制缺失或无输出时返回未知版本。
pub fn iperf3_version(app: Option<&tauri::AppHandle>) -> Iperf3Version {
    resolved(app).1.version
}

/// 版本信息命令的返回结构。
#[derive(Debug, Clone, Serialize)]
pub struct Iperf3ToolInfo {
    /// 是否找到可执行文件（随包内置或系统 PATH）。
    pub available: bool,
    /// 可执行文件路径（未找到时为 null）。
    pub exe_path: Option<String>,
    /// 原始版本行（如 `iperf 3.21 (cJSON 1.7.15)`）。
    pub version_raw: String,
    /// 归一化版本号文本（未知时为「未知」）。
    pub version: String,
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    /// 是否支持 `--json-stream`（实时逐区间输出）。
    pub supports_json_stream: bool,
    /// 是否支持 `--bidir`。
    pub supports_bidir: bool,
    /// 是否支持 `--dscp`。
    pub supports_dscp: bool,
    /// 面向用户的中文说明（缺失原因 / 就绪提示）。
    pub note: String,
}

/// 组装工具信息（供前端展示「已就绪 / 未找到」与能力门控）。
pub fn tool_info(app: Option<&tauri::AppHandle>) -> Iperf3ToolInfo {
    let (exe, probe) = resolved(app);
    let version = probe.version;
    let note = if exe.is_none() {
        "未找到 iperf3（已尝试随包内置 resources/iperf3 与系统 PATH）。可执行文件缺失时无法进行带宽测试。"
            .to_string()
    } else if !version.is_known() {
        "已找到 iperf3，但无法解析其版本号；将按最保守参数集运行（不启用实时区间输出）。"
            .to_string()
    } else if !version.supports_json_stream() {
        format!(
            "iperf3 {} 低于 3.17，不支持 --json-stream：测试期间只显示原始文本，结束后给出汇总。",
            version.label()
        )
    } else {
        format!("iperf3 {} 已就绪。", version.label())
    };
    Iperf3ToolInfo {
        available: exe.is_some(),
        exe_path: exe.map(|p| p.display().to_string()),
        version_raw: probe.raw,
        version: version.label(),
        major: version.major,
        minor: version.minor,
        patch: version.patch,
        supports_json_stream: version.supports_json_stream(),
        supports_bidir: version.supports_bidir(),
        supports_dscp: version.supports_dscp(),
        note,
    }
}

// ============================================================================
// 输入校验（后端是唯一裁决者；前端校验仅为体验）
// ============================================================================

/// 校验主机名 / IP 字面量 / 可选 `%dev` 后缀。
///
/// 因为全程用 `Command::args` 数组传参（绝不经 shell），这里只需防两类问题：
/// 1. **选项注入**：值以 `-` 开头会被 iperf3 当成参数（如 `-F /etc/passwd`）。
/// 2. 控制字符 / 空白破坏参数与输出解析。
fn validate_host(host: &str, field: &str) -> Result<String, String> {
    let h = host.trim();
    if h.is_empty() {
        return Err(format!("{}不能为空", field));
    }
    if h.starts_with('-') {
        return Err(format!(
            "{}不能以 '-' 开头（会被 iperf3 解析为命令行选项）",
            field
        ));
    }
    if h.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("{}不能包含空白或控制字符", field));
    }
    if h.len() > 253 {
        return Err(format!("{}过长（上限 253 字符）", field));
    }
    Ok(h.to_string())
}

/// 纯十进制整数校验（不接受符号/小数点）。
fn is_plain_u32(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())
}

/// `n[KMG T]` 规格校验（iperf3 的 `[KMG]`/`[KMGT]` 数值形式）。
fn is_size_spec(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    let last = match t.as_bytes().last() {
        Some(b) => *b,
        None => return false,
    };
    let digits = match last {
        b'k' | b'K' | b'm' | b'M' | b'g' | b'G' | b't' | b'T' => &t[..t.len() - 1],
        _ => t,
    };
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// 码率规格校验：`n[KMGT]` 或 `n[KMGT]/burst`（`-b` 的 burst 模式）。
fn is_bitrate_spec(s: &str) -> bool {
    let t = s.trim();
    match t.split_once('/') {
        None => is_size_spec(t),
        Some((base, burst)) => is_size_spec(base) && is_plain_u32(burst),
    }
}

/// CPU 亲和性规格：`n` 或 `n,m`。
fn is_affinity_spec(s: &str) -> bool {
    let t = s.trim();
    match t.split_once(',') {
        None => is_plain_u32(t),
        Some((a, b)) => is_plain_u32(a) && is_plain_u32(b),
    }
}

/// TOS 值：0-255，支持十进制 / 八进制（前导 0）/ 十六进制（0x）。
fn parse_tos(s: &str) -> Option<u32> {
    let t = s.trim();
    let (radix, digits) = if let Some(rest) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X"))
    {
        (16, rest)
    } else if t.len() > 1 && t.starts_with('0') {
        (8, &t[1..])
    } else {
        (10, t)
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    u32::from_str_radix(digits, radix)
        .ok()
        .filter(|v| *v <= 255)
}

/// DSCP 值：0-63 的数值（十进制/八进制/十六进制）或符号名（CS1/AF11/EF…）。
fn validate_dscp(s: &str) -> Result<String, String> {
    let t = s.trim();
    if t.is_empty() {
        return Err("DSCP 值不能为空".to_string());
    }
    if let Some(v) = parse_tos(t) {
        if v <= 63 {
            return Ok(t.to_string());
        }
        return Err(format!("DSCP 数值需在 0-63 之间（收到 {}）", v));
    }
    if t.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Ok(t.to_string());
    }
    Err("DSCP 需为 0-63 的数值或符号名（如 CS1 / AF11 / EF）".to_string())
}

// ============================================================================
// 客户端参数模型
// ============================================================================

/// 测试结束条件（`-t` / `-n` / `-k` 三者互斥，用枚举从类型上杜绝同时给出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EndMode {
    /// `-t`：按时长（秒）。
    #[default]
    Time,
    /// `-n`：按传输字节数。
    Bytes,
    /// `-k`：按块（包）数。
    Blocks,
}

/// IP 网络栈选择（对应 `-4` / `-6` / 不指定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IpFamily {
    /// 按目标域名解析结果自动选择。
    #[default]
    Auto,
    /// `-4`：强制 IPv4。
    V4,
    /// `-6`：强制 IPv6。
    V6,
}

/// 传输协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Tcp,
    Udp,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}
fn default_protocol() -> Protocol {
    Protocol::Tcp
}
fn default_ip_family() -> IpFamily {
    IpFamily::Auto
}
fn default_end_mode() -> EndMode {
    EndMode::Time
}
fn default_time() -> u32 {
    10
}
fn default_parallel() -> u32 {
    1
}
fn default_interval() -> u32 {
    1
}
fn default_format() -> String {
    "m".to_string()
}

/// iperf3 客户端测试参数（前端 camelCase 传入）。
///
/// 覆盖官方手册 Client specific options 的全部**在 Windows 上有意义**的参数；
/// 平台不支持的项（SCTP / fq-rate / bind-dev / MPTCP / flowlabel / GSO 等）
/// 有意识地不提供，UI 会明确说明原因，而不是静默生成无效参数。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Iperf3ClientOptions {
    /// `-c`：服务端主机名或 IP（必填）。
    pub host: String,
    /// `-p`：服务端口。
    #[serde(default = "default_port")]
    pub port: u16,
    /// `-u`：改用 UDP（默认 TCP）。
    #[serde(default = "default_protocol")]
    pub protocol: Protocol,
    /// `-R`：反向（服务端发，客户端收）。
    #[serde(default)]
    pub reverse: bool,
    /// `--bidir`：双向同时收发。
    #[serde(default)]
    pub bidir: bool,
    /// 结束条件选择。
    #[serde(default = "default_end_mode")]
    pub end_mode: EndMode,
    /// `-t`：测试时长（秒），`end_mode == time` 时生效。
    #[serde(default = "default_time")]
    pub time: u32,
    /// `-n`：传输字节数上限（如 `100M`），`end_mode == bytes` 时生效。
    #[serde(default)]
    pub bytes: Option<String>,
    /// `-k`：块数上限（如 `1000`），`end_mode == blocks` 时生效。
    #[serde(default)]
    pub blockcount: Option<String>,
    /// `-l`：读写缓冲长度（如 `128K`）。
    #[serde(default)]
    pub length: Option<String>,
    /// `-P`：并行流数量。
    #[serde(default = "default_parallel")]
    pub parallel: u32,
    /// `-O`：预热并丢弃前 N 秒统计（跳过 TCP 慢启动）。
    #[serde(default)]
    pub omit: u32,
    /// `-b`：目标码率（如 `100M` 或 `10M/1000` burst）。
    #[serde(default)]
    pub bitrate: Option<String>,
    /// `-i`：周期报告间隔秒数（0 = 关闭周期报告）。
    #[serde(default = "default_interval")]
    pub interval: u32,
    /// `-f`：报告单位 `k|m|g|t`。
    #[serde(default = "default_format")]
    pub format: String,
    /// `-w`：socket 缓冲区/窗口大小（如 `2M`）。
    #[serde(default)]
    pub window: Option<String>,
    /// `-M`：TCP/SCTP 最大段大小。
    #[serde(default)]
    pub mss: Option<u32>,
    /// `-N`：禁用 Nagle 算法（TCP_NODELAY）。
    #[serde(default)]
    pub no_delay: bool,
    /// `-4` / `-6`。
    #[serde(default = "default_ip_family")]
    pub ip_family: IpFamily,
    /// `-S`：IP TOS 字节（0-255）。
    #[serde(default)]
    pub tos: Option<String>,
    /// `--dscp`：DSCP 值（数值或符号名）。
    #[serde(default)]
    pub dscp: Option<String>,
    /// `-Z`：零拷贝发送。
    #[serde(default)]
    pub zerocopy: bool,
    /// `-V`：详细输出。
    #[serde(default)]
    pub verbose: bool,
    /// `-T`：每行输出前缀标题。
    #[serde(default)]
    pub title: Option<String>,
    /// `--get-server-output`：测试完成后取回服务端结果。
    #[serde(default)]
    pub get_server_output: bool,
    /// `--connect-timeout`：控制连接建立超时（毫秒）。
    #[serde(default)]
    pub connect_timeout_ms: Option<u32>,
    /// `-A`：CPU 亲和性（`n` 或 `n,m`）。
    #[serde(default)]
    pub affinity: Option<String>,
    /// `-B`：绑定到指定本地地址。
    #[serde(default)]
    pub bind: Option<String>,
    /// `--cport`：绑定客户端源端口。
    #[serde(default)]
    pub cport: Option<u16>,
    /// `-C`：拥塞控制算法（**仅 Linux/FreeBSD**，Windows 构建未编译该选项）。
    #[serde(default)]
    pub congestion: Option<String>,
    /// `--repeating-payload`：使用重复载荷（而非随机字节）。
    #[serde(default)]
    pub repeating_payload: bool,
    /// `--dont-fragment`：设置 IPv4 DF 位（仅 UDP over IPv4）。
    #[serde(default)]
    pub dont_fragment: bool,
    /// `--udp-counters-64bit`：UDP 使用 64 位计数器（长时/高速测试防溢出）。
    #[serde(default)]
    pub udp_counters_64bit: bool,
    /// `--rcv-timeout`：接收空闲超时（毫秒）。
    #[serde(default)]
    pub rcv_timeout_ms: Option<u32>,
    /// `--snd-timeout`：未确认 TCP 数据超时（毫秒）。
    #[serde(default)]
    pub snd_timeout_ms: Option<u32>,
    /// `--extra-data`：附加到 JSON 输出的自定义字符串。
    #[serde(default)]
    pub extra_data: Option<String>,
    /// `-F`：以文件作为数据源（发送端）/数据槽（接收端）。
    #[serde(default)]
    pub file: Option<String>,
    /// `--forceflush`：每个区间强制刷新输出（不经管道缓冲）。
    #[serde(default)]
    pub force_flush: bool,
}

impl Default for Iperf3ClientOptions {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: DEFAULT_PORT,
            protocol: Protocol::Tcp,
            reverse: false,
            bidir: false,
            end_mode: EndMode::Time,
            time: 10,
            bytes: None,
            blockcount: None,
            length: None,
            parallel: 1,
            omit: 0,
            bitrate: None,
            interval: 1,
            format: "m".to_string(),
            window: None,
            mss: None,
            no_delay: false,
            ip_family: IpFamily::Auto,
            tos: None,
            dscp: None,
            zerocopy: false,
            verbose: false,
            title: None,
            get_server_output: false,
            connect_timeout_ms: None,
            affinity: None,
            bind: None,
            cport: None,
            congestion: None,
            repeating_payload: false,
            dont_fragment: false,
            udp_counters_64bit: false,
            rcv_timeout_ms: None,
            snd_timeout_ms: None,
            extra_data: None,
            file: None,
            force_flush: false,
        }
    }
}

// ============================================================================
// 命令行构建
// ============================================================================

/// 把校验后的参数渲染为 iperf3 命令行参数数组。
///
/// 返回值**不含程序名**，且**必定是数组**（调用方用 `.args()` 传递）——
/// 不存在字符串拼接，因而不存在 shell 注入面。
pub fn build_client_args(
    opt: &Iperf3ClientOptions,
    version: Iperf3Version,
) -> Result<Vec<String>, String> {
    let mut a: Vec<String> = Vec::new();

    let host = validate_host(&opt.host, "目标地址")?;
    if opt.port == 0 {
        return Err("端口需在 1-65535 之间（收到 0）".to_string());
    }
    a.push("-c".into());
    a.push(host);

    a.push("-p".into());
    a.push(opt.port.to_string());

    // ── 协议与方向 ──────────────────────────────────────────────────────
    if opt.protocol == Protocol::Udp {
        a.push("-u".into());
    }
    if opt.reverse && opt.bidir {
        return Err("反向（-R）与双向（--bidir）互斥，请只选其一".to_string());
    }
    if opt.bidir {
        if version.is_known() && !version.supports_bidir() {
            return Err(format!(
                "iperf3 {} 不支持 --bidir（需 3.7 及以上）",
                version.label()
            ));
        }
        a.push("--bidir".into());
    } else if opt.reverse {
        a.push("-R".into());
    }

    // ── 结束条件（-t / -n / -k 互斥，由枚举保证只输出一个）─────────────
    match opt.end_mode {
        EndMode::Time => {
            if opt.time == 0 || opt.time > 86_400 {
                return Err("测试时长需在 1-86400 秒之间".to_string());
            }
            a.push("-t".into());
            a.push(opt.time.to_string());
        }
        EndMode::Bytes => {
            let v = opt
                .bytes
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "按字节数结束时必须填写字节数（如 100M）".to_string())?;
            if !is_size_spec(v) {
                return Err(format!("字节数格式非法：{}（应为数字 + 可选 K/M/G/T）", v));
            }
            a.push("-n".into());
            a.push(v.to_string());
        }
        EndMode::Blocks => {
            let v = opt
                .blockcount
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "按块数结束时必须填写块数（如 1000）".to_string())?;
            if !is_size_spec(v) {
                return Err(format!("块数格式非法：{}（应为数字 + 可选 K/M/G/T）", v));
            }
            a.push("-k".into());
            a.push(v.to_string());
        }
    }

    // ── 传输细节 ────────────────────────────────────────────────────────
    if let Some(v) = opt
        .length
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !is_size_spec(v) {
            return Err(format!(
                "缓冲长度格式非法：{}（应为数字 + 可选 K/M/G/T）",
                v
            ));
        }
        a.push("-l".into());
        a.push(v.to_string());
    }

    if opt.parallel == 0 || opt.parallel > 128 {
        return Err("并行流数量需在 1-128 之间".to_string());
    }
    if opt.parallel > 1 {
        a.push("-P".into());
        a.push(opt.parallel.to_string());
    }

    if opt.omit > 0 {
        if opt.omit > 3600 {
            return Err("预热丢弃时长（-O）需在 0-3600 秒之间".to_string());
        }
        a.push("-O".into());
        a.push(opt.omit.to_string());
    }

    if let Some(v) = opt
        .bitrate
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !is_bitrate_spec(v) {
            return Err(format!(
                "码率格式非法：{}（应为数字 + 可选 K/M/G/T，可加 /burst，如 100M 或 10M/1000）",
                v
            ));
        }
        a.push("-b".into());
        a.push(v.to_string());
    }

    if opt.interval > 3600 {
        return Err("报告间隔（-i）需在 0-3600 秒之间".to_string());
    }
    a.push("-i".into());
    a.push(opt.interval.to_string());

    let fmt = opt.format.trim().to_ascii_lowercase();
    if !matches!(fmt.as_str(), "k" | "m" | "g" | "t") {
        return Err(format!("报告单位非法：{}（应为 k/m/g/t）", opt.format));
    }
    a.push("-f".into());
    a.push(fmt);

    if let Some(v) = opt
        .window
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !is_size_spec(v) {
            return Err(format!(
                "窗口大小格式非法：{}（应为数字 + 可选 K/M/G/T）",
                v
            ));
        }
        a.push("-w".into());
        a.push(v.to_string());
    }

    if let Some(v) = opt.mss {
        if v == 0 || v > 32_768 {
            return Err("最大段大小（-M）需在 1-32768 之间".to_string());
        }
        a.push("-M".into());
        a.push(v.to_string());
    }

    if opt.no_delay {
        a.push("-N".into());
    }

    match opt.ip_family {
        IpFamily::Auto => {}
        IpFamily::V4 => a.push("-4".into()),
        IpFamily::V6 => a.push("-6".into()),
    }

    if let Some(v) = opt.tos.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if parse_tos(v).is_none() {
            return Err(format!(
                "TOS 值非法：{}（应为 0-255，可用 0x34 / 064 写法）",
                v
            ));
        }
        a.push("-S".into());
        a.push(v.to_string());
    }

    if let Some(v) = opt.dscp.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if version.is_known() && !version.supports_dscp() {
            return Err(format!(
                "iperf3 {} 不支持 --dscp（需 3.2 及以上）；可改用 TOS（-S）",
                version.label()
            ));
        }
        a.push("--dscp".into());
        a.push(validate_dscp(v)?);
    }

    if opt.zerocopy {
        a.push("-Z".into());
    }
    if opt.verbose {
        a.push("-V".into());
    }

    if let Some(v) = opt.title.as_deref().filter(|s| !s.is_empty()) {
        if v.chars().any(|c| c.is_control()) {
            return Err("输出标题不能包含控制字符".to_string());
        }
        a.push("-T".into());
        a.push(v.to_string());
    }

    if opt.get_server_output {
        a.push("--get-server-output".into());
    }

    if let Some(v) = opt.connect_timeout_ms {
        if v == 0 || v > 300_000 {
            return Err("连接超时需在 1-300000 毫秒之间".to_string());
        }
        a.push("--connect-timeout".into());
        a.push(v.to_string());
    }

    if let Some(v) = opt
        .affinity
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !is_affinity_spec(v) {
            return Err(format!("CPU 亲和性格式非法：{}（应为 n 或 n,m）", v));
        }
        a.push("-A".into());
        a.push(v.to_string());
    }

    if let Some(v) = opt.bind.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        a.push("-B".into());
        a.push(validate_host(v, "绑定地址")?);
    }

    if let Some(v) = opt.cport {
        if v == 0 {
            return Err("客户端源端口（--cport）需在 1-65535 之间".to_string());
        }
        a.push("--cport".into());
        a.push(v.to_string());
    }

    if opt
        .congestion
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty())
    {
        // -C 只在 Linux/FreeBSD 构建中存在（Windows 构建的 argv 不含该选项），
        // 这里明确拒绝而不是生成一个会被 iperf3 拒绝的参数。
        return Err(
            "拥塞控制算法（-C）仅 Linux/FreeBSD 版 iperf3 支持，Windows 构建不含该选项".to_string(),
        );
    }

    if opt.repeating_payload {
        a.push("--repeating-payload".into());
    }

    if opt.dont_fragment {
        if opt.protocol != Protocol::Udp {
            return Err("--dont-fragment 仅适用于 UDP over IPv4，请先选择 UDP 协议".to_string());
        }
        a.push("--dont-fragment".into());
    }

    if opt.udp_counters_64bit {
        a.push("--udp-counters-64bit".into());
    }

    if let Some(v) = opt.rcv_timeout_ms {
        if v == 0 {
            return Err("接收空闲超时需大于 0 毫秒".to_string());
        }
        a.push("--rcv-timeout".into());
        a.push(v.to_string());
    }

    if let Some(v) = opt.snd_timeout_ms {
        if v == 0 {
            return Err("发送超时需大于 0 毫秒".to_string());
        }
        a.push("--snd-timeout".into());
        a.push(v.to_string());
    }

    if let Some(v) = opt.extra_data.as_deref().filter(|s| !s.is_empty()) {
        if v.chars().any(|c| c.is_control()) {
            return Err("附加数据字符串不能包含控制字符".to_string());
        }
        a.push("--extra-data".into());
        a.push(v.to_string());
    }

    if let Some(v) = opt.file.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if opt.protocol == Protocol::Udp {
            return Err("文件传输（-F）与 UDP 测试互斥（iperf3 明确禁止该组合）".to_string());
        }
        let p = Path::new(v);
        if !p.is_file() {
            return Err(format!("文件不存在或不是普通文件：{}", v));
        }
        a.push("-F".into());
        a.push(v.to_string());
    }

    // ── JSON 输出编排 ──────────────────────────────────────────────────
    // 支持 --json-stream 时用它（实时逐区间）；否则退回 -J（结束一次性输出）。
    if version.supports_json_stream() {
        a.push("--json-stream".into());
    } else {
        a.push("-J".into());
    }

    // 管道场景下强制刷新，避免区间报告被缓冲到测试结束才可见。
    if !version.supports_json_stream() || opt.force_flush {
        a.push("--forceflush".into());
    }

    Ok(a)
}

/// 构建服务端命令行参数。
pub fn build_server_args(port: u16, one_off: bool) -> Result<Vec<String>, String> {
    if port == 0 {
        return Err("端口需在 1-65535 之间（收到 0）".to_string());
    }
    let mut a = vec!["-s".to_string(), "-p".to_string(), port.to_string()];
    if one_off {
        a.push("--one-off".into());
    }
    Ok(a)
}

// ============================================================================
// 结果模型
// ============================================================================

/// 单个方向的收发统计（正向或反向）。
fn num_f64(v: Option<&serde_json::Value>) -> Option<f64> {
    v.and_then(|x| x.as_f64()).filter(|f| f.is_finite())
}

fn num_u64(v: Option<&serde_json::Value>) -> Option<u64> {
    match v {
        Some(x) => x.as_u64().or_else(|| {
            x.as_f64()
                .filter(|f| f.is_finite() && *f >= 0.0)
                .map(|f| f as u64)
        }),
        None => None,
    }
}

/// 汇总某方向的统计。**缺失/NaN/负数一律 `None`，绝不伪造 0**（全仓「不伪造」原则）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct Iperf3DirectionSummary {
    /// 平均吞吐（Mbit/s），由 `bits_per_second` 换算。
    pub mbps: Option<f64>,
    pub bytes: Option<u64>,
    pub seconds: Option<f64>,
    /// TCP 重传次数（服务端/接收端可能不提供）。
    pub retransmits: Option<u64>,
    /// UDP 抖动（毫秒）。
    pub jitter_ms: Option<f64>,
    /// UDP 丢包率（百分比）。
    pub lost_percent: Option<f64>,
    /// UDP 丢包数。
    pub lost_packets: Option<u64>,
    /// UDP 收发包数。
    pub packets: Option<u64>,
}

impl Iperf3DirectionSummary {
    /// 该方向是否完全无数据（用于 UI 判断是否渲染）。
    pub fn is_empty(&self) -> bool {
        self.mbps.is_none()
    }

    fn from_value(v: &serde_json::Value) -> Option<Self> {
        let o = v.as_object()?;
        Some(Self {
            mbps: num_f64(o.get("bits_per_second")).map(|bps| bps / 1_000_000.0),
            bytes: num_u64(o.get("bytes")),
            seconds: num_f64(o.get("seconds")),
            retransmits: num_u64(o.get("retransmits")),
            jitter_ms: num_f64(o.get("jitter_ms")),
            lost_percent: num_f64(o.get("lost_percent")),
            lost_packets: num_u64(o.get("lost_packets")),
            packets: num_u64(o.get("packets")),
        })
    }
}

/// 单个测量区间的行（用于表格化展示与结构化摘要）。
#[derive(Debug, Clone, Serialize)]
pub struct Iperf3IntervalRow {
    pub start_s: f64,
    pub end_s: f64,
    /// 方向标签（正向 / 双向-反向）。
    pub direction: String,
    /// 该区间内的流数量（并行测试时 > 1）。
    pub streams: usize,
    pub mbps: Option<f64>,
    pub bytes: Option<u64>,
    pub retransmits: Option<u64>,
    pub jitter_ms: Option<f64>,
    pub lost_percent: Option<f64>,
    /// 是否处于 `-O` 预热丢弃段（该区间统计被 iperf3 标记为 omitted）。
    pub omitted: bool,
}

/// CPU 占用（iperf3 在 `end` 中提供；缺失则为 `None`）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct Iperf3Cpu {
    pub host_total: Option<f64>,
    pub remote_total: Option<f64>,
}

/// iperf3 客户端测试结果。
///
/// 字段名保持 snake_case（与 `network.rs` 的 `PingSummary` 等既有结果契约一致），
/// 前端按此读取。
#[derive(Debug, Clone, Serialize)]
pub struct Iperf3Result {
    pub target: String,
    pub port: u16,
    /// `TCP` / `UDP`（优先取自 iperf3 自报的 `test_start.protocol`）。
    pub protocol: String,
    /// 正向 / 反向 / 双向。
    pub direction: String,
    /// tcp / udp（输入侧归一化值，便于前端做分支）。
    pub protocol_key: String,
    pub parallel: u32,
    /// 实际使用的测试条件描述（供日志与结果卡展示）。
    pub end_condition: String,
    /// iperf3 自报版本（如 `iperf 3.21`）。
    pub tool_version: String,
    /// 本次是否使用了实时逐区间输出。
    pub json_stream: bool,
    /// 正向（默认 client→server；`-R` 时为 server→client）发送侧统计。
    pub sent: Iperf3DirectionSummary,
    /// 正向接收侧统计。
    pub received: Iperf3DirectionSummary,
    /// `--bidir` 时反向发送侧统计。
    pub bidir_reverse_sent: Option<Iperf3DirectionSummary>,
    /// `--bidir` 时反向接收侧统计。
    pub bidir_reverse_received: Option<Iperf3DirectionSummary>,
    pub cpu: Option<Iperf3Cpu>,
    pub intervals: Vec<Iperf3IntervalRow>,
    /// 是否被用户取消（取消不是失败，UI 不应报红）。
    pub cancelled: bool,
    /// 错误/告警原文（有界收集，供 UI 展示与排障）。
    pub messages: Vec<String>,
}

impl Iperf3Result {
    fn empty(opt: &Iperf3ClientOptions, tool_version: &str, json_stream: bool) -> Self {
        Self {
            target: opt.host.clone(),
            port: opt.port,
            protocol: match opt.protocol {
                Protocol::Tcp => "TCP".to_string(),
                Protocol::Udp => "UDP".to_string(),
            },
            direction: if opt.bidir {
                "双向".to_string()
            } else if opt.reverse {
                "反向".to_string()
            } else {
                "正向".to_string()
            },
            protocol_key: match opt.protocol {
                Protocol::Tcp => "tcp".to_string(),
                Protocol::Udp => "udp".to_string(),
            },
            parallel: opt.parallel,
            end_condition: end_condition_label(opt),
            tool_version: tool_version.to_string(),
            json_stream,
            sent: Iperf3DirectionSummary::default(),
            received: Iperf3DirectionSummary::default(),
            bidir_reverse_sent: None,
            bidir_reverse_received: None,
            cpu: None,
            intervals: Vec::new(),
            cancelled: false,
            messages: Vec::new(),
        }
    }
}

/// 结束条件的中文描述（日志与结果卡共用）。
pub fn end_condition_label(opt: &Iperf3ClientOptions) -> String {
    match opt.end_mode {
        EndMode::Time => format!("{} 秒", opt.time),
        EndMode::Bytes => format!("{} 字节", opt.bytes.as_deref().unwrap_or("-")),
        EndMode::Blocks => format!("{} 块", opt.blockcount.as_deref().unwrap_or("-")),
    }
}

// ============================================================================
// JSON 解析
// ============================================================================

/// 有界收集诊断消息（避免长测试刷爆内存）。
fn push_message(buf: &mut Vec<String>, msg: String) {
    if buf.len() < MAX_MESSAGES {
        buf.push(msg);
    }
}

/// 由汇总对象构造区间行。
fn interval_row(
    sum: &serde_json::Value,
    direction: &str,
    streams: usize,
) -> Option<Iperf3IntervalRow> {
    let o = sum.as_object()?;
    let start = num_f64(o.get("start")).unwrap_or(0.0);
    let end = num_f64(o.get("end")).unwrap_or(start);
    Some(Iperf3IntervalRow {
        start_s: start,
        end_s: end,
        direction: direction.to_string(),
        streams,
        mbps: num_f64(o.get("bits_per_second")).map(|bps| bps / 1_000_000.0),
        bytes: num_u64(o.get("bytes")),
        retransmits: num_u64(o.get("retransmits")),
        jitter_ms: num_f64(o.get("jitter_ms")),
        lost_percent: num_f64(o.get("lost_percent")),
        omitted: o.get("omitted").and_then(|x| x.as_bool()).unwrap_or(false),
    })
}

/// iperf3 输出解析器 — 把 `--json-stream` 事件流 / `-J` 文档汇聚成最终结果。
///
/// **公开为独立组件**（而非埋在进程执行里）的理由：解析逻辑是本模块最容易出错、
/// 也最需要回归保护的部分（`error` 事件的 data 是字符串、失败时 `end` 是空对象、
/// 双向另有 `sum_*_bidir_reverse` 等），拆出来才能在不启动真实进程的前提下穷尽单测。
pub struct Iperf3Parser {
    result: Iperf3Result,
    /// 最后一个 `end` 事件的数据对象（结果摘要的权威来源）。
    end_obj: Option<serde_json::Value>,
}

impl Iperf3Parser {
    /// 由参数与版本探测结果构造解析器。
    pub fn new(opt: &Iperf3ClientOptions, version: &Iperf3VersionProbe) -> Self {
        Self::with_tool_version(opt, &version.raw, version.version.supports_json_stream())
    }

    /// 构造解析器（直接给工具版本原文；测试与纯文本路径用）。
    pub fn with_tool_version(
        opt: &Iperf3ClientOptions,
        tool_version: &str,
        json_stream: bool,
    ) -> Self {
        Self {
            result: Iperf3Result::empty(opt, tool_version, json_stream),
            end_obj: None,
        }
    }

    /// 消费一行输出，返回 (应透传的事件, 是否为 JSON 事件行)。
    ///
    /// `false` 表示该行不是 JSON，调用方应按纯文本透传。
    pub fn feed_line(&mut self, line: &str) -> (Vec<StreamEvent>, bool) {
        let mut events = Vec::new();
        let handled = self.parse_json_line(line, &mut events);
        (events, handled)
    }

    /// 消费整份 `-J` 多行文档（不支持 `--json-stream` 的旧版路径）。
    /// 返回是否解析成功。
    pub fn feed_document(&mut self, text: &str) -> bool {
        self.parse_full_document(text)
    }

    /// 记录一条诊断/错误原文。
    pub fn push_diagnostic(&mut self, msg: &str) {
        push_message(&mut self.result.messages, msg.to_string());
    }

    /// 收尾：把 `end` 对象填入结果，并标记取消状态。
    pub fn finish(mut self, cancelled: bool) -> Iperf3Result {
        self.result.cancelled = cancelled;
        self.apply_end_object();
        self.result
    }

    // 此处曾有 `interval_count()`：全局检索确认调用点为 0（含测试），
    // 区间数量由 `result().intervals.len()` 直接取得（ADR-0001 D13）。

    /// 解析 `--json-stream` 的一行（内部实现）。
    ///
    /// 成功识别（含合法 JSON 但未知 event）返回 `true`；非 JSON 行返回 `false`。
    fn parse_json_line(&mut self, line: &str, events: &mut Vec<StreamEvent>) -> bool {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            return false;
        };
        if !v.is_object() {
            return false;
        }
        let event = v.get("event").and_then(|e| e.as_str()).unwrap_or("");
        let data = v.get("data");

        match event {
            "start" => {
                if let Some(d) = data {
                    if let Some(ver) = d.get("version").and_then(|x| x.as_str()) {
                        self.result.tool_version = ver.to_string();
                    }
                    if let Some(proto) = d
                        .get("test_start")
                        .and_then(|t| t.get("protocol"))
                        .and_then(|x| x.as_str())
                    {
                        self.result.protocol = proto.to_string();
                    }
                }
                events.push(StreamEvent {
                    kind: "info".into(),
                    data: format!(
                        "已连接 {}（iperf3 {}，协议 {}，并行 {} 流）",
                        self.result.target,
                        self.result.tool_version,
                        self.result.protocol,
                        self.result.parallel
                    ),
                });
                true
            }
            "interval" => {
                let Some(d) = data else { return true };
                let streams = d
                    .get("streams")
                    .and_then(|s| s.as_array())
                    .map(|s| s.len())
                    .unwrap_or(0);
                // 正向汇总；双向时另有 sum_bidir_reverse
                let fwd = self.result.direction.clone();
                if let Some(sum) = d.get("sum") {
                    if let Some(row) = interval_row(sum, &fwd, streams) {
                        events.push(StreamEvent {
                            kind: "iperf-interval".into(),
                            data: serde_json::to_string(&row).unwrap_or_default(),
                        });
                        self.result.intervals.push(row);
                    }
                }
                if let Some(sum) = d.get("sum_bidir_reverse") {
                    if let Some(row) = interval_row(sum, "双向-反向", streams) {
                        events.push(StreamEvent {
                            kind: "iperf-interval".into(),
                            data: serde_json::to_string(&row).unwrap_or_default(),
                        });
                        self.result.intervals.push(row);
                    }
                }
                true
            }
            "end" => {
                // 出错时 data 可能是空对象 {}——必须容忍，不能据此判定成功。
                self.end_obj = data.cloned();
                true
            }
            "error" => {
                // 实测：error 的 data 是**字符串**（不是对象），故单独分支。
                let msg = match data {
                    Some(serde_json::Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => "iperf3 报告未知错误".to_string(),
                };
                push_message(&mut self.result.messages, msg.clone());
                events.push(StreamEvent {
                    kind: "error".into(),
                    data: msg,
                });
                true
            }
            _ => true,
        }
    }

    /// 用 `end` 对象填充结果摘要。
    fn apply_end_object(&mut self) {
        let Some(end) = self.end_obj.clone() else {
            return;
        };
        let get = |k: &str| end.get(k);

        if let Some(v) = get("sum_sent") {
            if let Some(s) = Iperf3DirectionSummary::from_value(v) {
                self.result.sent = s;
            }
        }
        if let Some(v) = get("sum_received") {
            if let Some(s) = Iperf3DirectionSummary::from_value(v) {
                self.result.received = s;
            }
        }
        if let Some(v) = get("sum_sent_bidir_reverse") {
            self.result.bidir_reverse_sent = Iperf3DirectionSummary::from_value(v);
        }
        if let Some(v) = get("sum_received_bidir_reverse") {
            self.result.bidir_reverse_received = Iperf3DirectionSummary::from_value(v);
        }

        // UDP 场景 iperf3 可能只给 sum（接收侧视角）。仅在正向接收侧为空时兜底，
        // 绝不把反向数据张冠李戴到正向。
        if self.result.received.is_empty() {
            if let Some(v) = get("sum") {
                if let Some(s) = Iperf3DirectionSummary::from_value(v) {
                    self.result.received = s;
                }
            }
        }

        if let Some(cpu) = get("cpu_utilization_percent") {
            self.result.cpu = Some(Iperf3Cpu {
                host_total: num_f64(cpu.get("host_total")),
                remote_total: num_f64(cpu.get("remote_total")),
            });
        }
    }

    /// 解析纯 `-J` 多行文档（不支持 `--json-stream` 的旧版或一次性输出场景）。
    fn parse_full_document(&mut self, text: &str) -> bool {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
            return false;
        };
        if let Some(start) = v.get("start") {
            if let Some(ver) = start.get("version").and_then(|x| x.as_str()) {
                self.result.tool_version = ver.to_string();
            }
            if let Some(proto) = start
                .get("test_start")
                .and_then(|t| t.get("protocol"))
                .and_then(|x| x.as_str())
            {
                self.result.protocol = proto.to_string();
            }
        }
        if let Some(intervals) = v.get("intervals").and_then(|x| x.as_array()) {
            let fwd = self.result.direction.clone();
            for it in intervals {
                let streams = it
                    .get("streams")
                    .and_then(|s| s.as_array())
                    .map(|s| s.len())
                    .unwrap_or(0);
                if let Some(sum) = it.get("sum") {
                    if let Some(row) = interval_row(sum, &fwd, streams) {
                        self.result.intervals.push(row);
                    }
                }
            }
        }
        self.end_obj = v.get("end").cloned();
        true
    }
}

// ============================================================================
// 客户端执行
// ============================================================================

/// 运行一次 iperf3 客户端测试（同步阻塞，调用方用 `spawn_blocking`）。
///
/// 关键实现要点：
/// - **stderr 独立线程并发排空**：只读 stdout 会让 stderr 管道写满后阻塞子进程
///   （旧实现即有此隐患），必须并发读取。
/// - 每行检查取消标志；取消时 kill 并返回 `cancelled = true`（不是 Err）。
/// - 失败时把 iperf3 的真实报错原文返回（绝不返回空串）。
pub fn run_client_streaming(
    opt: &Iperf3ClientOptions,
    exe: &Path,
    version: &Iperf3VersionProbe,
    cmd_id: &str,
    emit: impl Fn(StreamEvent) + Send + Sync + 'static,
) -> Result<Iperf3Result, String> {
    let args = build_client_args(opt, version.version)?;
    let json_stream = version.version.supports_json_stream();
    let mut parser = Iperf3Parser::new(opt, version);

    let mut child = Command::new(exe)
        .args(&args)
        // CREATE_NO_WINDOW：iperf3 持续运行，防止黑窗常驻
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| {
            format!(
                "iperf3 无法启动（{}）：{}。请确认二进制未被安全软件拦截",
                exe.display(),
                e
            )
        })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法捕获 iperf3 stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法捕获 iperf3 stderr".to_string())?;

    // stderr 并发排空线程（防管道写满导致子进程阻塞）
    let stderr_buf: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
    let stderr_handle = {
        let buf = Arc::clone(&stderr_buf);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Ok(mut b) = buf.lock() {
                    if b.len() < MAX_MESSAGES {
                        b.push(line);
                    }
                }
            }
        })
    };

    let mut cancelled = false;
    if json_stream {
        // 逐行 JSON 事件；非 JSON 行（警告文本等）按纯文本透传
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if crate::cancel::is_cancelled(cmd_id) {
                cancelled = true;
                let _ = child.kill();
                break;
            }
            let (events, handled) = parser.feed_line(&line);
            for ev in &events {
                emit(ev.clone());
            }
            if handled {
                continue;
            }
            let t = line.trim();
            if !t.is_empty() {
                parser.push_diagnostic(t);
                emit(StreamEvent {
                    kind: "info".into(),
                    data: t.to_string(),
                });
            }
        }
    } else {
        // 旧版：整份 -J 文档在结束时输出；同时把文本行实时透出
        let mut raw = String::new();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if crate::cancel::is_cancelled(cmd_id) {
                cancelled = true;
                let _ = child.kill();
                break;
            }
            raw.push_str(&line);
            raw.push('\n');
            let t = line.trim();
            if !t.is_empty() {
                emit(StreamEvent {
                    kind: "info".into(),
                    data: t.to_string(),
                });
            }
        }
        if !cancelled && !parser.feed_document(&raw) {
            // 连 -J 都解析不了：把原文留作诊断，不当作成功
            parser.push_diagnostic("无法解析 iperf3 JSON 输出（已按原始文本处理）");
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("等待 iperf3 退出失败：{}", e))?;
    let _ = stderr_handle.join();

    let stderr_text = stderr_buf.lock().map(|b| b.join("\n")).unwrap_or_default();
    let stderr_trimmed = stderr_text.trim().to_string();
    if !stderr_trimmed.is_empty() {
        for line in stderr_trimmed.lines() {
            parser.push_diagnostic(line.trim());
        }
    }

    if cancelled {
        return Ok(parser.finish(true));
    }

    let result = parser.finish(false);

    if !status.success() {
        // 失败原因优先级：error 事件 > stderr > 退出码。
        let msg = result
            .messages
            .iter()
            .find(|m| !m.starts_with("无法解析"))
            .cloned()
            .or_else(|| {
                if stderr_trimmed.is_empty() {
                    None
                } else {
                    Some(stderr_trimmed)
                }
            })
            .unwrap_or_else(|| {
                format!(
                    "iperf3 以非零退出码结束（{}），且未提供错误详情",
                    status.code().unwrap_or(-1)
                )
            });
        return Err(msg);
    }

    Ok(result)
}

// ============================================================================
// 服务端模式（长驻进程生命周期）
// ============================================================================

struct ServerState {
    child: Option<Child>,
    port: u16,
    started_at: Option<String>,
    lines: VecDeque<String>,
    /// 代际号：重启后递增，使上一次的排空线程自行失效（避免竞态写入）。
    generation: u64,
    error: Option<String>,
}

impl Default for ServerState {
    fn default() -> Self {
        Self {
            child: None,
            port: DEFAULT_PORT,
            started_at: None,
            lines: VecDeque::new(),
            generation: 0,
            error: None,
        }
    }
}

lazy_static::lazy_static! {
    static ref SERVER: StdMutex<ServerState> = StdMutex::new(ServerState::default());
}

/// 服务端状态（供 UI 轮询展示）。
#[derive(Debug, Clone, Serialize)]
pub struct Iperf3ServerStatus {
    pub running: bool,
    pub port: Option<u16>,
    pub pid: Option<u32>,
    pub started_at: Option<String>,
    /// 已运行秒数（未运行时为 null）。
    pub uptime_s: Option<u64>,
    /// 是否以 `--one-off` 启动（处理一个连接后自动退出）。
    pub one_off: bool,
    /// 服务端输出（最近 N 行，文本）。
    pub lines: Vec<String>,
    /// 启动失败等操作系统级错误（如端口被占用）。
    pub error: Option<String>,
}

impl Iperf3ServerStatus {
    fn stopped() -> Self {
        Self {
            running: false,
            port: None,
            pid: None,
            started_at: None,
            uptime_s: None,
            one_off: false,
            lines: Vec::new(),
            error: None,
        }
    }
}

/// 检查并回收已自行退出的服务端进程（`--one-off` 完成后会退出）。
/// 返回当前是否仍在运行。
fn reap_if_exited(st: &mut ServerState) -> bool {
    let running = match st.child.as_mut() {
        Some(child) => match child.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) => true,
            // 查询失败按已退出处理（保守：不让 UI 一直显示"运行中"）
            Err(_) => false,
        },
        None => false,
    };
    if !running && st.child.is_some() {
        st.child = None;
        st.started_at = None;
    }
    running
}

/// 记录一行服务端输出（供排空线程调用）。
fn push_server_line(generation: u64, line: String) {
    if let Ok(mut st) = SERVER.lock() {
        if st.generation != generation {
            return;
        }
        if st.lines.len() >= SERVER_LINE_CAPACITY {
            st.lines.pop_front();
        }
        st.lines.push_back(line);
    }
}

/// 端口占用预检：尝试在 0.0.0.0 上绑定，成功即释放。
fn precheck_port(port: u16) -> Result<(), String> {
    match TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => {
            drop(l);
            Ok(())
        }
        Err(e) => Err(format!(
            "端口 {} 已被占用，无法启动 iperf3 服务端：{}。请先停止占用该端口的程序或改用其他端口。",
            port, e
        )),
    }
}

/// 启动本机 iperf3 服务端。
///
/// `one_off = true` 时处理一个连接后自动退出（适合做一次验证性测试）。
pub fn server_start(exe: &Path, port: u16, one_off: bool) -> Result<Iperf3ServerStatus, String> {
    let args = build_server_args(port, one_off)?;

    {
        let mut st = SERVER.lock().map_err(|_| "服务端状态锁异常".to_string())?;
        if reap_if_exited(&mut st) {
            return Err(format!(
                "iperf3 服务端已在运行（端口 {}，PID {}）。请先停止后再启动。",
                st.port,
                st.child.as_ref().map(|c| c.id()).unwrap_or(0)
            ));
        }
        st.error = None;
        st.lines.clear();
    }

    precheck_port(port)?;

    let mut child = Command::new(exe)
        .args(&args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| {
            format!(
                "iperf3 服务端无法启动（{}）：{}。请确认二进制未被安全软件拦截",
                exe.display(),
                e
            )
        })?;

    let pid = child.id();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let generation = {
        let mut st = SERVER.lock().map_err(|_| "服务端状态锁异常".to_string())?;
        st.generation = st.generation.wrapping_add(1);
        let gen = st.generation;
        st.child = Some(child);
        st.port = port;
        st.started_at = Some(chrono::Local::now().to_rfc3339());
        st.lines.clear();
        gen
    };

    // stdout / stderr 各一个排空线程
    if let Some(out) = stdout {
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                push_server_line(generation, line);
            }
        });
    }
    if let Some(err) = stderr {
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                push_server_line(generation, format!("[stderr] {}", line));
            }
        });
    }

    Ok(Iperf3ServerStatus {
        running: true,
        port: Some(port),
        pid: Some(pid),
        started_at: SERVER.lock().ok().and_then(|s| s.started_at.clone()),
        uptime_s: Some(0),
        one_off,
        lines: Vec::new(),
        error: None,
    })
}

/// 查询服务端状态（UI 在展开服务端卡片时轮询调用）。
pub fn server_status(one_off: bool) -> Iperf3ServerStatus {
    let Ok(mut st) = SERVER.lock() else {
        return Iperf3ServerStatus::stopped();
    };
    let running = reap_if_exited(&mut st);
    let uptime = st.started_at.as_ref().and_then(|s| {
        chrono::DateTime::parse_from_rfc3339(s).ok().map(|t| {
            let secs = chrono::Local::now()
                .signed_duration_since(t.with_timezone(&chrono::Local))
                .num_seconds();
            secs.max(0) as u64
        })
    });
    Iperf3ServerStatus {
        running,
        port: if running { Some(st.port) } else { None },
        pid: if running {
            st.child.as_ref().map(|c| c.id())
        } else {
            None
        },
        started_at: if running { st.started_at.clone() } else { None },
        uptime_s: if running { uptime } else { None },
        one_off,
        lines: st.lines.iter().cloned().collect(),
        error: st.error.clone(),
    }
}

/// 停止服务端（杀死进程；幂等，未运行时返回 stopped 状态）。
pub fn server_stop() -> Iperf3ServerStatus {
    let Ok(mut st) = SERVER.lock() else {
        return Iperf3ServerStatus::stopped();
    };
    let lines: Vec<String> = st.lines.iter().cloned().collect();
    // 保留已收集的输出便于用户回看
    if let Some(mut child) = st.child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    st.started_at = None;
    // 代际号递增：让在途的排空线程停止写入
    st.generation = st.generation.wrapping_add(1);
    Iperf3ServerStatus {
        running: false,
        port: None,
        pid: None,
        started_at: None,
        uptime_s: None,
        one_off: false,
        lines,
        error: None,
    }
}

/// 应用退出清理（挂到 `RunEvent::Exit`）：强杀仍在运行的服务端，避免孤儿进程。
pub fn shutdown_server() {
    if let Ok(mut st) = SERVER.lock() {
        if let Some(mut child) = st.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        st.started_at = None;
        st.generation = st.generation.wrapping_add(1);
    }
}

// ============================================================================
// 防火墙（服务端可被外部访问的必要一环）
// ============================================================================

/// 校验防火墙协议参数。
fn normalize_proto(proto: &str) -> Result<String, String> {
    match proto.trim().to_ascii_uppercase().as_str() {
        "TCP" => Ok("TCP".to_string()),
        "UDP" => Ok("UDP".to_string()),
        other => Err(format!("协议需为 TCP 或 UDP（收到 {}）", other)),
    }
}

/// 规则名（固定前缀 + 端口 + 协议，便于成对移除与用户辨识）。
pub fn firewall_rule_name(port: u16, proto: &str) -> String {
    format!("{} {} {}", FIREWALL_RULE_PREFIX, port, proto)
}

/// 生成入站放行规则（需管理员）。
pub fn build_firewall_args_add(port: u16, proto: &str) -> Result<Vec<String>, String> {
    if port == 0 {
        return Err("端口需在 1-65535 之间（收到 0）".to_string());
    }
    let proto = normalize_proto(proto)?;
    Ok(vec![
        "advfirewall".into(),
        "firewall".into(),
        "add".into(),
        "rule".into(),
        format!("name={}", firewall_rule_name(port, &proto)),
        "dir=in".into(),
        "action=allow".into(),
        format!("protocol={}", proto),
        format!("localport={}", port),
    ])
}

/// 生成移除规则参数（按固定规则名删除）。
pub fn build_firewall_args_remove(port: u16, proto: &str) -> Result<Vec<String>, String> {
    if port == 0 {
        return Err("端口需在 1-65535 之间（收到 0）".to_string());
    }
    let proto = normalize_proto(proto)?;
    Ok(vec![
        "advfirewall".into(),
        "firewall".into(),
        "delete".into(),
        "rule".into(),
        format!("name={}", firewall_rule_name(port, &proto)),
    ])
}

/// 执行防火墙规则增删。返回面向用户的中文结果。
pub fn apply_firewall_rule(action: &str, port: u16, proto: &str) -> Result<String, String> {
    if !crate::cleanup::is_admin() {
        return Err(
            "需要管理员权限才能修改 Windows 防火墙规则（建议：以管理员身份重新运行本程序）"
                .to_string(),
        );
    }
    let (args, verb) = match action.trim().to_ascii_lowercase().as_str() {
        "add" => (build_firewall_args_add(port, proto)?, "添加"),
        "remove" => (build_firewall_args_remove(port, proto)?, "移除"),
        other => return Err(format!("动作需为 add 或 remove（收到 {}）", other)),
    };
    let out = Command::new("netsh")
        .args(&args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("调用 netsh 失败：{}", e))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        let detail = if stderr.is_empty() { stdout } else { stderr };
        return Err(format!(
            "防火墙规则{}失败（退出码 {}）：{}",
            verb,
            out.status.code().unwrap_or(-1),
            detail
        ));
    }
    Ok(format!(
        "已{}防火墙入站规则「{}」（端口 {}）",
        verb,
        firewall_rule_name(port, &normalize_proto(proto)?),
        port
    ))
}

// ============================================================================
// 单元测试
// ============================================================================
//
// 通过 `#[path]` 引入独立测试文件：测试体量与本模块相当，混在一起会淹没实现。
// `#[path]` 保证它仍是**同一编译单元**，因此可以直接测试私有函数
// （`parse_version` / `build_client_args` / 解析器内部行为等）。

#[cfg(test)]
#[path = "iperf_tests.rs"]
mod tests;
