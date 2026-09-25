# SECM 硬件数据源契约 v3（冻结）

> 本文件是 SECM 与 LhmSidecar 之间的**唯一**接口契约。Rust 侧与 C# 侧并行开发时，
> 双方都必须严格按本文件实现，不得单方面改名或增删字段。
>
> 变更历史：v1 = CPU 温度/功耗；v2 = +GPU/主板/内存；v3 = LiteMonitor 数据源移植
> （性能计数器 + 完整 LiteMonitor 映射层 + 两级权限模型）。

## 1. 背景与目标

SECM 原硬件数据来自 `sensor.rs` 内联探测（`CallNtPowerInformation` / WinRing0 / ACPI /
`nvidia-smi` / 硬编码 TDP 估算表）与 `hardware.rs`（独立 HTTP 服务 + 注册表 GPU 枚举）。
本版将这些全部移除，改为**完整移植 [Diorser/LiteMonitor](https://github.com/Diorser/LiteMonitor)
的数据来源**：

- **LibreHardwareMonitorLib 0.9.6**（MPL-2.0）+ LiteMonitor 自有的映射/规则层（MIT）
- 映射层包含 `SensorMatcher` / `HardwareRules` / `SensorMap` / `FanMapper` /
  `ComponentProcessor` / `DiskManager` / `NetworkManager` / `PerformanceCounterManager` /
  `BatteryService` 的行为语义

### 关键事实（决定了本契约的形态）

LiteMonitor 的 `app.manifest` 声明 `requireAdministrator`，且 `Program.cs` 在 PawnIO
驱动缺失时**直接退出**。LiteMonitor **没有**任何非管理员路径可以读取 CPU 温度/功耗/
主板温度——这些只能经 PawnIO ring0 设备（DACL 仅 SYSTEM/Administrators）获得。
LiteMonitor 唯一纯用户态的数据源是 `PerformanceCounterManager`（PDH 性能计数器）。

因此本 sidecar 采用**两级权限模型**，这是 LiteMonitor 没有、而 SECM 必须补上的一层：

| 层级 | 权限 | 覆盖指标 | 获取方式 |
| --- | --- | --- | --- |
| **tier-1** | 普通用户，无需 UAC | CPU 负载、CPU 频率、内存、每盘 IO/活动/占用、网络吞吐、电池、GPU 温度/功耗/显存（厂商用户态通道） | 本进程内 |
| **tier-2** | 管理员（一次 UAC） | CPU 温度/功耗/电压、主板 SuperIO 温度/风扇/电压、CPU/机箱风扇/水泵、磁盘温度 | 提权伴生进程 |

**进程模型**：本进程始终以未提权启动，立即提供 tier-1。tier-2 字段为 `null` 且
`*_source="none"`，并给出 `tier2_reason`。用户通过 `POST /api/v1/elevate` 触发一次
UAC；被接受后启动提权伴生进程（监听 `port+1`），本进程代理 tier-2 读取并与 tier-1
合并为**同一份快照**返回。被拒绝是非致命且被记住的状态（不每次轮询重复弹窗）。

## 2. HTTP 端点

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/health` | `200 {"status":"ok"}`（保持 v1 兼容，SECM 以 `"status":"ok"` 判定端口归属） |
| GET | `/api/v1/snapshot` | `200` 快照 JSON（见 §3） |
| POST | `/api/v1/elevate` | 触发一次 UAC 提权；`200 {"elevated":true/false,"reason":"..."}` |
| POST | `/api/v1/elevate/auto` | **零 UAC 自动提权**：只走「管理员直通」与「已注册持久授权 `/Run`」，**绝不 runas**；`200 {"elevated":true/false,"reason":"..."}`，`elevated:false` 表示需用户授权 |
| GET | `/api/v1/privilege` | `200 {"elevated":bool,"tier2_available":bool,"tier2_reason":string\|null}` |

兼容性：`GET /api/lhm/sensors` 仍返回同一份快照（v2 路径别名），避免旧 Rust 客户端
直接失败。非 GET → `405`；未知路径 → `404`。

仅绑定 `127.0.0.1`。请求头读取上限 8KB、读写超时 2s（沿用现有实现，防慢速连接占线程）。

启动参数：`--port <port>`（默认 45980）、`--interval <ms>`（默认 1000，最小 100）、
`--role=elevated`（提权伴生进程模式，监听 `port+1`，内部使用不再二次提权）。

## 3. 快照 JSON（契约 v3）

字段名一律 **snake_case**（`JsonNamingPolicy.SnakeCaseLower`）。
无法采集的字段一律 `null`（标量）/ `[]`（数组）/ `null`（对象）——**绝不伪造 0**。
数字类型：可空标量为 `double?` / `long?`；Rust 侧对应 `Option<f32>` / `Option<u64>`。

```jsonc
{
  "available": true,              // 是否有任一有效指标（见 §4）
  "error": null,                  // 顶层不可用原因（available=true 时为 null）
  "contract_version": 3,

  // ---- 权限状态（v3 新增）----
  "elevated": false,              // tier-2 是否已生效（含伴生进程）
  "tier2_available": false,       // tier-2 指标当前是否可读
  "tier2_reason": "需要管理员权限（PawnIO 设备仅 SYSTEM/Administrators 可访问）",

  "cpu": {
    "name": "AMD Ryzen 7 5800X 8-Core Processor",   // LHM CPU 节点名，缺失时空串
    "load_percent": 12.5,          // double?  ← PDH % Processor Utility（截断到 100）
    "package_temp_c": 45.2,        // double?  ← tier-2
    "core_temps_c": [44.1, 45.2],  // double[] ← tier-2，无则 []
    "power_w": 38.4,               // double?  ← tier-2
    "voltage_v": 1.19,             // double?  ← tier-2
    "clock_mhz": 4200.0,           // double?  ← tier-1（PDH）优先，回退 tier-2
    "fan_rpm": 812.0,              // double?  ← tier-2
    "pump_rpm": null,              // double?  ← tier-2
    "load_source":  "perfcounter", // "perfcounter" | "lhm" | "none"
    "temp_source":  "lhm",         // "lhm" | "none"
    "power_source": "lhm",         // "lhm" | "none"
    "clock_source": "perfcounter", // "perfcounter" | "lhm" | "registry" | "none"
    "power_estimated": false       // 恒 false（移植后无 TDP×usage 估算；保留字段兼容）
  },

  "gpu": [
    {
      "name": "NVIDIA GeForce RTX 3060",
      "temperature_c": 52.3,          // double?
      "core_clock_mhz": 1500.0,       // double?
      "power_w": 85.2,                // double?
      "load_percent": 45.0,           // double?
      "memory_used_bytes": 1073741824,// long?
      "memory_total_bytes": 6442450944,
      "fan_rpm": 1200.0,              // double?
      "vram_load_percent": 16.7,      // double?  ← 计算值 used/total*100
      "temp_source": "lhm",           // "lhm" | "none"
      "power_source": "lhm",          // "lhm" | "none"
      "clock_source": "lhm"           // "lhm" | "none"
    }
  ],

  "motherboard": {                  // null = tier-2 不可用/无主板节点
    "name": "HUANANZHI X99",
    "sensors": [
      { "name": "System", "type": "temperature", "value": 34.2 },
      { "name": "Fan #1", "type": "fan",         "value": 812.0 },
      { "name": "+12V",   "type": "voltage",     "value": 12.1 }
    ]
  },

  "memory": {                       // null = 无内存节点
    "name": "Physical Memory",      // LHM 内存节点名 / SPD 型号（best-effort）
    "total_bytes": 17179869184,     // long?（LHM 推算；SECM 仍以 GlobalMemoryStatusEx 为权威）
    "used_bytes": 8589934592        // long?
  },

  // ---- v3 新增：磁盘（tier-1 PDH 速率 + DriveInfo 占用；tier-2 温度）----
  "disks": [
    {
      "name": "C:",
      "read_bytes_per_sec": 1048576.0,  // double?  ← 每卷 PDH（per-volume 实例）
      "write_bytes_per_sec": 524288.0,  // double?
      "activity_percent": 12.0,         // double?  ← PDH % Disk Time，截断 0-100
      "used_percent": 63.5,             // double?  ← DriveInfo
      "temperature_c": 38.0             // double?  ← tier-2
    }
  ],

  // ---- v3 新增：电池（无电池时为 null）----
  "battery": {
    "percent": 87.0,        // double?
    "power_w": -12.5,       // double?  ← AC 在线为正、放电为负（BatteryService 符号修正）
    "voltage_v": 15.6,      // double?
    "current_a": -0.8       // double?
  }
}
```

### 3.1 `motherboard.sensors[].type` 取值

`"temperature"` | `"fan"` | `"voltage"`（LHM `SensorType` 映射，其他类型不输出）。

### 3.2 tier-2 缺失时的确切形态
`elevated=false` 时：

- `cpu.package_temp_c` / `core_temps_c` / `power_w` / `voltage_v` / `fan_rpm` / `pump_rpm` = `null` / `[]`
- `cpu.temp_source` / `cpu.power_source` = `"none"`
- `motherboard` = `null`
- `disks[].temperature_c` = `null`
- `available` 仍可为 `true`（只要 tier-1 有任意有效指标）

**不得**用 0 冒充缺失值。前端以 `null` → "—" 渲染。

## 4. `available` 语义

`available = true` 当且仅当 tier-1 **或** tier-2 至少有一个有效指标：

- CPU `load_percent` / `clock_mhz` 有效，或
- 任一 GPU 的 `temperature_c` / `load_percent` / `power_w` 有效，或
- `disks[]` 任一项速率/占用有效，或
- `memory` 非 null，或
- CPU `package_temp_c` 有效（tier-2 生效时）

全不可用 → `available=false` 且 `error` 给出可读原因（含修复建议），
SECM 据此展示降级提示。

## 5. 实现约束（C# 侧）

1. **映射语义必须与 LiteMonitor 一致**（逐条对照上游，不要"改进"）：
   - `SensorMatcher.Match` 的 CPU 温度排除列表：`soc` / `vrm` / `fan` / `pump` /
     `liquid` / `coolant` / `distance`；内存拒绝 `virtual`。
   - `HardwareRules.GetHwPriority`：`Basic Render`=100、Nvidia=0、AMD 独显=0、
     `"AMD Radeon(TM) Graphics"`=2、Arc 独显=0、Arc 核显=1、Iris=2、UHD=3、其他=4。
     **已知上游缺陷（照搬，不修）**：`IsDiscreteArc` 用 `Has(name, " A")` 子串匹配判断
     Arc 独显，而 `"Intel Arc Graphics"` 恰好内含 `l Arc` → `" A"` 命中，导致 Intel Arc
     **核显**被判为独显（优先级 0 而非 1）。上游 `HardwareRules.cs:77` 即如此，本移植
     保持上游行为并用测试固化（`IsDiscreteArc_UpstreamQuirk_NamesEmbeddingSpaceA`）。
     理由：本任务要求"完整移植"，擅自"修正"会让 GPU 优选顺序偏离上游；如需修正，
     应作为独立变更并同步评估 `ShouldUseSharedMemory` 的连带影响。
   - `SensorMap` 冲突解决：高优先级显卡胜出；同级优先非 `D3D`（Vendor）传感器。
   - 主板温度选择顺序：`System` → `Motherboard` → `Chipset`/`PCH` →
     15–68 ℃ 区间最大值 → 0–95 ℃ 区间最大值。
   - `FanMapper`：200 RPM 底噪门槛；cooler 检测（`HardwareType.Cooler` 或名称含
     `Kraken`/`Corsair`/`Liquid`/`AIO`/`Cooler`）；水泵 >3000 RPM 猜测；
     机箱风扇 `Rear`→`Chassis`→`Sys`→`Case`→最低 RPM。
   - `ComponentProcessor` 熔断：`CPU.Power > 600W`、`GPU.Clock > 6000MHz`、
     `GPU.Power > 1200W` 一律丢弃（返回 null）。
   - Zen 5 频率修正：bus speed ∈ (1,20) 且 factor = 100/bus ∈ (2,10) 时按 factor 缩放核心频率。
2. **`IsCpuEnabled` 必须恒为 true**（上游 `HardwareMonitor.cs:64-69` 注释所述原因：
   热切换只改 `IsControllerEnabled`，关闭 CPU 节点会导致温度永久读不到）。
3. **PDH 计数器首次 `NextValue()` 返回 0**：初始化时必须预热一次并丢弃，
   否则首屏为 0。
4. **`% Processor Utility` 可能 >100**（睿频）：截断到 100。
5. **不移植** `SystemOptimizer`（内存整理，非指标）、`NetworkSpeedTester`
   （测速；上游还关闭了 TLS 校验，绝不复刻）、`FpsCounter`（PresentMon + ETW，
   独立管理员功能，不在本次范围）。
6. **不移植** `DriverInstaller.cs` 的"联网下载 + 仅校验 MZ/大小"流程；
   PawnIO 部署复用 SECM 已有的签名校验门禁（`third_party/PawnIO/`，
   `driver_signature_valid`），由 Rust 侧驱动。
7. 保留现有 sidecar 的健壮性：设备预检、初始化退避重试、采集异常隔离、
   文件日志镜像、`ExecuteElevated` 的 UAC 取消处理（错误码 1223）。
8. **网络吞吐（裁决：完整移植，暂不外发）**：§1 层级表承诺 tier-1 覆盖"网络吞吐"，
   但 §3/§6 未定义对应键。`NetworkManager` 必须**完整移植并在采集循环中驱动**
   （它同时是 LHM 吞吐兜底通道的持有者），但**不新增快照字段**——字段集已冻结，
   前端 `SensorData` 的网络数据另有权威来源（`net_stats.rs` → `sysinfo` +
   `GetTcpStatistics`）。后续如需外发，应作为契约 v4 的独立变更。
9. **磁盘温度需上位映射（实现补充，非语义偏离）**：上游 `DiskManager.cs:157` 以
   盘符匹配 LHM 存储节点名，但 LHM 0.9.6 的存储节点名是型号（如
   `WDS100T3X0C-00SJG0`）、标识符形如 `/nvme/6`，该启发式**永远匹配不上**，
   会导致 `disks[].temperature_c` 恒为 null。实现需增加卷→物理盘映射
   （`IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` + 标识符尾部盘号），已用
   `Win32_DiskDrive.Index` 交叉验证；映射不上的卷保持 `null`，**不得猜测**。
10. **`MetricUtils.GetPowerStatus`** 用 `kernel32!GetSystemPowerStatus` 而非 WinForms
   （无 WinForms 的控制台宿主下会 SIGSEGV），语义等价；`BatteryService` 不节流调用，
    使插拔电源后符号立即正确而非滞后 ≤3s。

## 6. Rust 侧类型契约（并行开发的接口冻结）

`src-tauri/src/lhm.rs` 暴露以下类型（`sensor.rs` 依赖它们，不得改名）：

```rust
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmCpuData {
    pub name: String,
    pub load_percent: Option<f32>,
    pub package_temp_c: Option<f32>,
    pub core_temps_c: Vec<f32>,
    pub power_w: Option<f32>,
    pub voltage_v: Option<f32>,
    pub clock_mhz: Option<f32>,
    pub fan_rpm: Option<f32>,
    pub pump_rpm: Option<f32>,
    pub load_source: String,
    pub temp_source: String,
    pub power_source: String,
    pub clock_source: String,
    pub power_estimated: bool,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmGpuData {
    pub name: String,
    pub temperature_c: Option<f32>,
    pub core_clock_mhz: Option<f32>,
    pub power_w: Option<f32>,
    pub load_percent: Option<f32>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub fan_rpm: Option<f32>,
    pub vram_load_percent: Option<f32>,
    pub temp_source: String,
    pub power_source: String,
    pub clock_source: String,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmMbSensor { pub name: String, #[serde(rename = "type")] pub kind: String, pub value: f32 }

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmMotherboardData { pub name: Option<String>, pub sensors: Vec<LhmMbSensor> }

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmMemoryData { pub name: Option<String>, pub total_bytes: Option<u64>, pub used_bytes: Option<u64> }

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmDiskData {
    pub name: String,
    pub read_bytes_per_sec: Option<f64>,
    pub write_bytes_per_sec: Option<f64>,
    pub activity_percent: Option<f32>,
    pub used_percent: Option<f32>,
    pub temperature_c: Option<f32>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmBatteryData {
    pub percent: Option<f32>,
    pub power_w: Option<f32>,
    pub voltage_v: Option<f32>,
    pub current_a: Option<f32>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LhmSensorResponse {
    pub available: bool,
    pub error: Option<String>,
    pub contract_version: Option<u32>,
    pub elevated: bool,
    pub tier2_available: bool,
    pub tier2_reason: Option<String>,
    pub cpu: LhmCpuData,
    pub gpu: Vec<LhmGpuData>,
    pub motherboard: Option<LhmMotherboardData>,
    pub memory: Option<LhmMemoryData>,
    pub disks: Vec<LhmDiskData>,
    pub battery: Option<LhmBatteryData>,
}
```

`#[serde(default)]` 是硬性要求：保证 v2（无 `elevated`/`disks`/`battery`）与
缺字段响应不 panic。`contract_version` 为 `Option<u32>`（v1 响应无此字段）。

`lhm.rs` 的公开函数（签名不变，供 `sensor.rs` / `commands.rs` 调用）：

```rust
pub fn init(app: Option<&tauri::AppHandle>);
pub fn lhm_snapshot() -> (Option<LhmSensorResponse>, String);   // (快照, 诊断串)
pub fn lhm_cpu_temp() -> (Option<f32>, String);                 // 保留兼容
pub fn shutdown();
pub fn request_elevation() -> Result<bool, String>;             // POST /api/v1/elevate
pub fn auto_elevate() -> Result<bool, String>;                 // POST /api/v1/elevate/auto（零 UAC，绝不 runas）
```

## 7. Rust 侧对外 JSON 契约（不变，前端依赖）

`SensorData` / `CpuData` / `GpuData` / `MemoryData` / `DiskData` 字段名**保持不变**。
`*_source` 枚举**收窄**（前端类型与断言同步更新）：

| 字段 | 原取值 | v3 取值 |
| --- | --- | --- |
| `cpu.freq_source` | `ntapi` \| `pdh` \| `registry` \| `sysinfo` \| `none` | `pdh` \| `registry` \| `lhm` \| `none` |
| `cpu.temp_source` | `lhm` \| `winring0` \| `acpi` \| `none` | `lhm` \| `none` |
| `cpu.power_source` | `lhm` \| `rapl` \| `estimated` \| `none` | `lhm` \| `estimated` \| `none` |

映射规则（Rust `sensor.rs` 负责）：

- sidecar `clock_source="perfcounter"` → 前端 `freq_source="pdh"`（PDH 计数器即
  `% Processor Performance`，前端 "估算" 角标语义成立）
- sidecar `clock_source="registry"` → `freq_source="registry"`（"标称" 角标）
- sidecar `clock_source="lhm"` → `freq_source="lhm"`（**tier-2 实测频率**：当
  tier-1 的 PDH 与注册表标称都不可用、而 tier-2 已生效时，sidecar
  `SnapshotBuilder.cs:148-155` 回退到 LHM 复合 CPU 频率，来源为
  `ComponentProcessor.cs:114-141` 的真实测量值。它**是实测值**，前端不加
  "估算"/"标称" 角标——`Dashboard.tsx` 的角标分支只覆盖 `pdh` 与 `registry`，
  且 `freq_source` 在 TS 侧为 `string`，故无需前端改动。若此处映射为 `none`
  会丢弃有效数据——正是"管理员状态下必须准确"所禁止的）
- sidecar `temp_source="lhm"` → `temp_source="lhm"`；否则 `"none"`
- sidecar `power_source="lhm"` → `power_source="lhm"`，`power_estimated=false`；
  否则若 `power_w` 有值 → `"estimated"` + `power_estimated=true`；否则 `"none"`

不变式：`freq_source=="none"` 必须与 `clock_mhz==0.0` 成对出现（反之 `clock_mhz>0`
必须有非 `none` 的来源），避免"有效率值却标记无来源"或"无来源却给出数值"。

`winring0` / `acpi` / `ntapi` / `sysinfo` 不再可能出现。

## 8. 验收

非管理员（主路径）：

- 启动**无 UAC 弹窗**
- Dashboard 显示：CPU 负载/频率、内存、GPU 温度/功耗/显存（厂商用户态通道）、
  磁盘读写、网络
- CPU 温度/功耗、主板卡片**不显示**，并给出"需要管理员"提示
- 任何位置**无伪造 0**；不可用字段 `*_source="none"`

管理员（tier-2）：

- 从提示触发**一次** UAC，接受后
- CPU 温度/功耗/电压、主板温度/风扇/电压、磁盘温度全部出现

`cargo test` / `cargo clippy -- -D warnings` / `npm run build` / `npm run lint` 全绿。
