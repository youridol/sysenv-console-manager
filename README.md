# SysEnv Console Manager (SECM)

> Windows 10/11 系统环境管理桌面工具 — 硬件监控、清理优化、网络诊断、系统设置、环境检测一站式平台

[![Version](https://img.shields.io/badge/version-v2.0.1-blue)](CHANGELOG.md)
[![License](https://img.shields.io/badge/license-MIT-green)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2021_edition-orange)](https://www.rust-lang.org/)
[![Tauri](https://img.shields.io/badge/tauri-2.x-purple)](https://tauri.app/)

## 功能特性

应用包含 9 个页面（React Router 路由），全部功能由 Rust 后端 **78 条 Tauri 命令**支撑：

| 页面         | 路由           | 功能                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| ------------ | -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **硬件信息** | `/`            | 每秒轮询传感器（CPU 占用/频率、内存、磁盘）；CPU/GPU/内存 60 秒趋势图 + 下载/上传速率趋势图（图表历史持久化，跨重启恢复）；网络流量卡片（各网卡/总量切换、0.5s–5s 轮询间隔、活跃 TCP 连接数、链路协商速度）；磁盘存储卡片与 **SMART 健康状态**（型号/容量/温度/健康度，损坏风险黄色警示）                                                                                                                                                                                                                                       |
| **清理缓存** | `/cleanup`     | DNS 缓存刷新（`DnsFlushResolverCache`，dnsapi）；临时文件清理（TEMP / Windows Temp / Prefetch）；工作集修剪（释放当前进程内存，需管理员）；着色器缓存清理（NVIDIA / AMD / DirectX / Steam 全部库目录 + CS2 深度缓存 / 一键全清，被占用文件标记重启后删除）；进程管理（内存 Top 200、名称/PID 搜索、6 档优先级设置）；执行结果追溯面板                                                                                                                        |
| **网络诊断** | `/network`     | **网络配置（可编辑）**：物理地址 MAC / IPv4 地址+掩码+默认网关 / IPv4 DNS / 链路本地 IPv6 / IPv6 网关与 DNS，DHCP⇄静态一键切换，静态设置或 MAC 注册表修改（重启网卡生效，应用前备份原值）；**DHCP 服务器检测**（DHCPDISCOVER 主动探测 + 注册表基线合并，1 台健康 / 多台冲突警告）与 **深度检查**（逐台 ping 可达性 + 同网段/网关拓扑判定，跨网段 OFFER 识别）以及 **清理残留**（已移除网卡遗留的 DHCP 注册表记录，清理前展示清单并自动备份，清理后自动重新检测）；HTTP 延迟测试（DNS + TCP + TLS 握手 + 请求/响应的应用层 HTTP 往返延迟，4 个默认站点 + 自定义增删改，localStorage 持久化，手动/自动检测）；Ping（ICMP，可配次数/间隔/包大小/TTL/截止时间/连续模式）；Traceroute（最大 64 跳，每跳 3 探针）；Nslookup（A / AAAA 记录）；NAT 类型检测（15 个 STUN 服务器 + 自定义，RFC 3489 三阶段检测，NAT0–NAT4 分类）；端口连通性检测；iperf3 带宽测试（**随包内置，无需自装**）：客户端覆盖官方手册在 Windows 上有意义的全部测试参数（协议 TCP/UDP、方向 正向/反向/双向、结束条件 时长/字节/块数、并行流、预热丢弃、限速含 burst、缓冲/窗口/MSS、Nagle、网络栈 v4/v6、报告间隔与单位、TOS/DSCP、零拷贝、CPU 亲和性、绑定地址与源端口、重复载荷、IPv4 DF、UDP 64 位计数、收发超时、文件传输、附加数据）并提供**实时区间输出 + 结构化方向分列摘要**（UDP 突出抖动·丢包，TCP 突出重传，另附 CPU 占用），同时提供**本机服务端模式**（供对端设备连接测速，含防火墙放行辅助与退出自动清理）；全部工具支持流式输出与取消 |
| **系统设置** | `/settings`    | 系统优化开关（GPU 硬件加速调度 HAGS / 游戏模式 / 窗口化游戏优化 / 鼠标精准度 / 可变刷新率 VRR / **关闭高精度计时器**，均需管理员/部分重启生效）；电源计划（注册表读取、滑动开关切换、删除确认、卓越性能自动导入并激活）；**核心线程调度策略**（异类线程/短运行线程，AC/DC 六档，AMD/Intel 混合架构均受益）；**NVIDIA 显卡电源管理模式**（NVAPI DRS 读写，自适应/最高性能/最佳功率三档）；服务管理（全量服务枚举、搜索、启动/停止、启动类型 自动/手动/禁用）                                                                                                    |
| **环境检测** | `/environment` | 系统信息（版本/Build/架构/安装日期/激活状态/最新补丁/启动模式，8 字段并行采集）；游戏环境（CS2 / 三角洲行动 / 瓦洛兰特 / PUBG / 英雄联盟 5 款预设推荐设置与当前系统状态对比，**推荐项可一键切换并与系统设置页开关实时联动**）；DirectX 诊断（版本/GPU 适配器/D3D11/D3D12/WDDM）；VC++ 运行库完整性（2005–2022 共 12 项） |
| **硬件检测** | `/hardware`    | 手动刷新的硬件清单与实时状态（CPU / GPU / 内存 / 磁盘明细），支持按核心展开的 CPU 实时频率（PDH 估算层）                                                                                                                                                                                                                                                                                                                                             |
| **关于**     | `/about`       | 应用版本、构建信息、更新日志（内嵌 CHANGELOG.md）、第三方许可清单（6 组 50+ 项，含 LHM/PawnIO/HidSharp/.NET Runtime 等全部依赖）、赞助/抖音二维码                                                                                                                                                                                                                                                                                                                                                 |

> **日志查看**已并入主界面右侧栏（`RightLogPanel` + `LogsCard`），不再是独立页面：
> 支持 Tauri 事件实时推送、200 条环形缓冲、级别筛选与统计、关键词搜索、自动滚动、
> 「全部 / 按页」输出模式切换、导出为 .txt、清空缓冲；日志同步落盘至运行目录
> `logs/<日期>.log`（见下文「数据与日志存储」）。

系统级功能（`main.rs` / `lib.rs`）：

- **权限模型**：**无需管理员权限启动，启动时无 UAC 弹窗**。硬件数据来自 LHM sidecar：
  tier-1（CPU 负载/频率、内存、磁盘、网络、电池、GPU 厂商通道）以普通用户权限常驻可用；
  tier-2（CPU 温度/功耗/电压、主板 SuperIO、磁盘温度）需要一次按需 UAC —— 用户点击引导卡
  才弹窗，拒绝后不再重复打扰。需要管理员的其他功能（清理/服务管理等）由各命令独立检查权限并返回错误提示
- **驱动自愈链**：启动时按需部署/装载 PawnIO（WHQL 签名验证 → 服务创建，唯一 ring0 驱动），安全软件（火绒等）拦截时提供精确放行指引与一键重试
- **单实例**：`tauri-plugin-single-instance` 防止多实例占用 WebView2 user-data-dir
- **系统托盘**：右键菜单（显示主窗口 / 退出）、左键单击恢复窗口、关闭按钮隐藏到托盘（`prevent_close`）
- **启动后台任务**：过期诊断文件清理（7 天）
- **诊断数据落盘**：运行目录 `diag/` 下的 sensor.log / debug.log（单文件 100MB 轮转、目录 1GB 上限、7 天保留）+ chart_history.jsonl（图表历史持久化）。运行目录解析见 `src-tauri/src/runtime_paths.rs`（详见「数据与日志存储」）

## 技术栈

| 层级       | 技术                                            | 版本            |
| ---------- | ----------------------------------------------- | --------------- |
| 桌面框架   | Tauri（含 CLI）                                 | 2.x（CLI 2.11） |
| Tauri 插件 | single-instance / store / dialog / shell / fs   | 2.x             |
| 后端语言   | Rust（edition 2021）                            | 稳定版          |
| 驱动采集   | secm-datasource（windows-sys / winreg / ureq / wmi / thiserror） | 0.1.0 |
| 硬件数据源 | LiteMonitor 数据源层移植（.NET 8 sidecar，MIT/MPL-2.0 进程隔离）| LiteMonitor + LHM 0.9.6 |
| ring0 驱动 | PawnIO（WHQL 签名，唯一内核驱动）                | 2.2.0           |
| 前端框架   | React（react-router-dom 7）                     | 19              |
| 构建工具   | Vite                                            | 6               |
| UI 组件库  | shadcn/ui（new-york 风格，Radix UI 原语）       | radix-ui 1.6.7  |
| 样式方案   | Tailwind CSS（@tailwindcss/vite）               | 4               |
| 图表       | Recharts                                        | 3.10.1          |
| 图标       | lucide-react                                    | 0.468           |
| 安装包     | NSIS（currentUser 模式，SimpChinese + English） | —               |

## 架构

```
┌─────────────────────────────────────────────────────────────┐
│  React 19 前端（8 页面，Tauri 命令 + Channel 流式事件）        │
│  硬件信息 / 清理缓存 / 网络诊断 / 系统设置                    │
│  环境检测 / 硬件检测 / 调试日志 / 关于                        │
└──────────────────────┬──────────────────────────────────────┘
                       │ Tauri v2 IPC（invoke / Channel / event）
┌──────────────────────▼──────────────────────────────────────┐
│  Rust 后端（24 个模块 + datasource 独立 crate）               │
│  sensor / lhm / driver_install /                             │
│  cleanup / network / net_config / dhcp_probe / dhcp_residue /│
│  hpt / nvidia_drs / environment / game_env / sysinfo /      │
│  net_stats / ip_info / disk_info / ds_util / log / debug /  │
│  tray / cancel / temp_data / proc_util / commands / main    │
│                                                              │
│  ┌──────────────────────────────────────────────────────┐   │
│  │ secm-datasource（datasource/ 独立 crate，P1-P14 底层）│   │
│  │ registry / service / power / netif / dns / http /     │   │
│  │ activation / disk / error                              │   │
│  └──────────────────────────────────────────────────────┘   │
│  sensor.rs = 纯投影层：把 sidecar 快照映射为对外 SensorData   │
└─────────────────────────┬───────────────────────────────────┘
                          │ HTTP 127.0.0.1:45980（/api/v1/snapshot）
┌─────────────────────────▼───────────────────────────────────┐
│  LhmSidecar（sidecar-lhm/，.NET 8 独立进程，MPL-2.0 隔离）   │
│                                                              │
│  tier-1（普通用户，无 UAC，始终可用）                         │
│    PDH 性能计数器 / NetworkInterface / DriveInfo / 电池 /     │
│    GPU 厂商用户态通道                                        │
│                                                              │
│  tier-2（管理员，按需一次 UAC）                               │
│    提权伴生进程：LibreHardwareMonitorLib 0.9.6 → PawnIO ring0 │
│    覆盖 CPU 温度/功耗/电压、主板 SuperIO、磁盘温度            │
│                                                              │
│  未提权时 tier-2 字段为 null 且 *_source="none"（绝不伪造 0） │
│  （进程隔离规避 GPL/MPL 传染，源码与许可随包分发）            │
└─────────────────────────────────────────────────────────────┘
```

> 硬件数据来源为 [Diorser/LiteMonitor](https://github.com/Diorser/LiteMonitor)（MIT）数据源层的移植
> （性能计数器 + 完整映射/规则层），ring0 读取由 LibreHardwareMonitorLib 0.9.6（MPL-2.0）
> 经 **PawnIO**（唯一 ring0 驱动）完成。WinRing0 通道已彻底移除，不再随包分发。
> 权威接口契约见 `sidecar-lhm/CONTRACT-v3.md`。

### 驱动采集模块（`datasource/`，crate `secm-datasource`）

**数据源采集层已全面切换为纯 Rust**。
原 PowerShell 调用链与 `sc` / `powercfg` / `wmic` / `ipconfig` / `curl` 外部命令（P1-P14 共 14 个采集点）
已由独立 crate `secm-datasource` 的 Win32 API / 注册表 / 纯 Rust HTTP 实现替代，前端命令契约零改动：

| 模块            | 采集点                                             | 实现方式                                   |
| --------------- | -------------------------------------------------- | ------------------------------------------ |
| `registry`      | P2 最新补丁 / P6 GPU 厂商                           | 注册表（HotFix 子键 / Class 键 DriverDesc） |
| `service`       | P10-P12 服务管理（枚举/状态/启停/启动类型）         | advapi32（OpenSCManager / EnumServicesStatusExW） |
| `power`         | P7-P9 电源计划（切换/AC-DC 写入/导入卓越性能）      | powrprof（PowerSetActiveScheme 等）        |
| `netif`         | P3 链路协商速度 / P4 本地 IP / 完整网络配置         | iphlpapi（GetIfTable2 / GetAdaptersAddresses） |
| `dns`           | P13 DNS 缓存刷新                                    | dnsapi（DnsFlushResolverCache）            |
| `http`          | P5 公网 IP / 地理位置 / 时区                        | ureq + URL 白名单（HTTP_ALLOWLIST，5s 超时）|
| `activation`    | P1 Windows 激活状态                                 | 注册表近似 + WMI 兜底（LicenseStatus 映射）|
| `disk`          | P14 磁盘枚举 / SMART 读取                           | 存储设备 IOCTL（SMART_RCV_DRIVE_DATA）     |
| `error`         | 统一错误模型 `CollectError`（WinApi/Registry/Http/Parse/NeedsAdmin/NotFound） | thiserror          |

> **注（v3 硬件路径）**：CPU 实时频率（`freq_source="pdh"`）与每卷磁盘读写速度
> **不再由 `secm-datasource` 采集**。原 `cpu_freq` / `disk_io` 两个 PDH 模块已随本次
> 重构删除（它们是"当前数据来源"的一部分，已被 LHM sidecar 的 tier-1 完全取代）：
> 频率走 sidecar 的 `PerformanceCounterManager`（`% Processor Performance` × `~MHz`），
> 每卷 IO 走 sidecar 的 PDH 实例（`VolumeDiskMap` 做卷→物理盘映射）。
> `sensor.rs` 只做快照投影。前端角标语义不变
> （`freq_source="pdh"` → “估算” / `"registry"` → “标称” / `"lhm"` → 实测无角标）。

- **错误显式化**：新采集器返回 `Result<T, CollectError>`，由 `ds_util.rs` 映射为用户可见中文消息并打 R8 日志（含 API 名/操作名/详情）
- **回滚开关**：编译期 feature `legacy-data-source`（`ds_util::use_legacy()`）可一键回退旧 PowerShell/外部命令实现，见下文「数据源切换」
- **保留外部进程**（仅 1 处）：`iperf3` 带宽测试（K1，**随包分发**，见 `third_party/iperf3/` 与 `scripts/stage-iperf3.mjs`；用户无需自装，缺随包文件时回退系统 PATH）
- 独立构建/测试：`cd datasource && cargo build && cargo test`（47 单测 + 1 真机忽略项）

### 温度/功耗数据源（`sensor.rs` / `lhm.rs` / `driver_install/`）

**降级链**：`lhm → none`（`temp_source` 取值仅 `lhm` | `none`）。旧的
`winring0 → acpi` 通道已随驱动移除整体删除，**不存在内核/ACPI 兜底**：

| 取值     | 数据源                                                     | 说明                                  |
| -------- | ---------------------------------------------------------- | ------------------------------------- |
| **lhm**  | LHM sidecar tier-2（提权伴生进程，HTTP 45980）              | CPU 封装温度/功耗/电压、主板 SuperIO、磁盘温度；经 PawnIO ring0（WHQL 签名）读取；进程隔离规避 MPL/GPL 传染，源码与许可随包 |
| **none** | 无                                                         | tier-2 未生效（用户未放行 UAC / PawnIO 不可用）。字段为 `null`、`*_source="none"`，**绝不伪造 0**；Dashboard 显示"需要管理员"引导卡（一键触发一次 UAC）与频率诊断摘要卡 |

**两级权限模型**（`CONTRACT-v3.md` §1，这是 LiteMonitor 没有、SECM 必须补上的一层）：

| 层级 | 权限 | 覆盖指标 | 获取方式 |
| --- | --- | --- | --- |
| **tier-1** | 普通用户，**无需 UAC，启动无弹窗** | CPU 负载/频率、内存、每盘 IO/活动/占用、网络吞吐、电池、GPU 温度/功耗/显存（厂商用户态通道） | sidecar 本进程内（PDH / NetworkInterface / DriveInfo / 电池） |
| **tier-2** | 管理员，**按需一次 UAC** | CPU 温度/功耗/电压、主板 SuperIO 温度/风扇/电压、CPU/机箱风扇/水泵、磁盘温度 | 提权伴生进程（`--role=elevated`，监听 port+1），主进程代理并与 tier-1 合并为同一份快照 |

- sidecar 始终以**未提权**启动并立即提供 tier-1；只有用户点击引导卡触发 `POST /api/v1/elevate`
  才弹**一次** UAC。被拒绝是**非致命且被记住**的状态，不会每次轮询重复弹窗。
- `GET /api/v1/privilege` 返回 `{elevated, tier2_available, tier2_reason}`，前端据此解释缺失原因。

> 自研内核驱动 hwmon-driver 已从项目移除（2026-09 开源整理）；WinRing0x64.sys 亦于同批移除
> （Microsoft 漏洞驱动阻断列表收录，且 LHM 0.9.6 唯一 ring0 后端即 PawnIO）——`third_party/`
> 仅保留署名/存档副本，**不再 stage 进安装包**。

- **驱动部署**（`driver_install/`）：PawnIO 是**唯一** ring0 驱动，随包分发 WHQL 三件套（`resources/driver/`，源自 `third_party/PawnIO/`）+ GPL-2.0 源码包，签名验证门禁后创建 SCM 服务并启动（幂等，30s 缓存）；安全软件（火绒等）拦截时提供精确放行步骤与一键重试
- CPU 频率：由 sidecar tier-1 的 PDH 计数器提供（任务管理器同源），映射为 `freq_source="pdh"`（前端“估算”角标）；注册表标称回退映射为 `"registry"`（“标称”角标）；tier-1 两者都不可用而 tier-2 已生效时，回退 LHM 复合 CPU 频率并映射为 `"lhm"`（实测值，前端不加角标）；全部不可用才是 `"none"`。`ntapi` / `sysinfo` 取值已不再出现
- 内存：`GlobalMemoryStatusEx` 为**权威值**（sidecar 的内存数据仅作型号补充）；磁盘：DriveInfo 占用 + sidecar PDH 速率 + SMART（P14）
- 集成测试：`cargo test --test hardware_contract` / `datasource_native`（契约收窄 + 降级/不伪造断言）

## 构建产物

| 类型   | 文件                                                               | 位置                                    |
| ------ | ------------------------------------------------------------------ | --------------------------------------- |
| 便携版 | `sysenv-console-manager.exe`                                       | `src-tauri/target/release/`             |
| 安装版 | `SysEnv Console Manager_<版本>_x64-setup.exe`（NSIS，currentUser） | `src-tauri/target/release/bundle/nsis/` |
| 便携包 | `SECM_<版本>_portable.zip`（解压即用，免安装）                     | `dist_releases/<版本>/`                 |

**发行归档约定**：每次发版将最终产物归档至 `dist_releases/<版本>/`（唯一权威交付位置），包含安装版 setup.exe、便携版 exe、便携压缩包（zip）、`resources/driver/`、`resources/lhm/`、`SHA256SUMS.txt` 校验和。`src-tauri/target/` 为中间构建产物，可随时清理重建。

安装包内置资源：随包分发 PawnIO WHQL 三件套 + GPL-2.0 源码（源自 `third_party/PawnIO/`；**PawnIO 是唯一 ring0 驱动**，WinRing0x64.sys 不再随包）、LHM sidecar（.NET 8 自包含发布 + 完整源码含 `Snapshot.cs`/`Hardware/*.cs` + 许可文本）与 **iperf3 带宽测试二进制**（源自 `third_party/iperf3/`，含 `iperf3.exe` + `cygwin1.dll` 两件套与四份许可文本，**SHA256 校验通过后才打包**）。构建前需执行 `npm run stage:driver` / `npm run stage:iperf3` / `npm run stage:lhm` 就绪产物（推荐一键 `npm run tauri:build`）。

## 开发环境配置

### 前置条件

- [Rust](https://www.rust-lang.org/tools/install)（edition 2021，稳定版；需 `x86_64-pc-windows-msvc` target）
- [Node.js](https://nodejs.org/) 20+
- [Microsoft Visual C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)（含 MSVC 编译器 + Windows SDK，Rust MSVC 工具链链接必需）
- [.NET 8 SDK](https://dotnet.microsoft.com/download/dotnet/8.0)（构建 LHM sidecar：`npm run lhm:publish`）
- Windows 10/11 (x64)

> 编译说明：`secm-datasource`（datasource/）是独立 crate，使用 `windows-sys` / `winreg` / `ureq` / `wmi` / `thiserror`。
> 其中 `wmi` crate 依赖 `windows-core 0.61.x`，`Cargo.lock` 已统一版本（`cargo update --precise 0.61.2` 锁定），
> 改动依赖版本后若出现 windows-core 冲突，先按此方式对齐再构建。

### 快速开始

```bash
# 安装前端依赖
npm install

# 开发模式（无需管理员；需要管理员的功能运行时会提示）
npm run tauri dev

# 生产构建（tsc → vite build → cargo build --release → stage 驱动/sidecar → NSIS）
npm run tauri:build
```

> **普通用户可直接运行**：启动无 UAC 弹窗，tier-1 硬件指标（CPU 负载/频率、内存、磁盘、网络、
> 电池、GPU 厂商通道）立即显示。CPU 温度/功耗/主板/磁盘温度属 tier-2，需**按需一次** UAC ——
> 在硬件信息页点击"需要管理员"引导卡触发；拒绝后不再重复提示（非致命且被记住）。
> 清理、服务管理等操作由各命令内部检查管理员权限。

### 常用命令

```bash
npm run build          # 前端类型检查（tsc -b）+ vite 生产构建
npm run lint           # ESLint 检查（js/ts 推荐集 + react-hooks 经典规则）
npm run format         # Prettier 格式化
npm run stage:driver   # 就绪 PawnIO 三件套 + 源码（唯一 ring0 驱动）
npm run stage:iperf3   # 就绪 iperf3 随包二进制（iperf3.exe + cygwin1.dll + 许可，SHA256 校验）
npm run stage:lhm      # 就绪 LHM sidecar 发布产物 + 许可文本 + 递归源码
npm run lhm:publish    # 构建 LHM sidecar（dotnet publish，win-x64 self-contained）
npm run tauri:build    # 一键：stage 全部资源 + 驱动签名审计 + NSIS 打包
npm run package:portable  # 生成便携 zip（dist_releases/v<版本>/）
bash scripts/test-lib.sh                         # src-tauri 单元测试（运行数据落 build/tmp/）
node scripts/audit-driver-signatures.mjs         # 驱动签名审计（PawnIO）
node scripts/audit/licenses-npm.mjs --copyleft   # 依赖许可审计（npm）
node scripts/audit/licenses-cargo.mjs --copyleft # 依赖许可审计（Rust crates）
```

### 数据源切换（新驱动 ⇄ legacy 回滚）

数据源采集层默认使用纯 Rust 驱动（`secm-datasource`）。过渡期保留编译期开关：

```bash
# 默认：纯 Rust 驱动采集（P1-P14 全部走 Win32/注册表/HTTP）
npm run tauri dev

# 回退：启用 legacy-data-source feature，回到旧 PowerShell/外部命令实现
cd src-tauri && cargo build --features legacy-data-source
```

- 切换由 `src-tauri/src/ds_util.rs::use_legacy()`（编译期 `cfg!(feature = "legacy-data-source")`）控制，运行时不可变，前端零感知
- **不建议在发布包中启用 legacy**：该链路依赖 `powershell.exe` / `sc` / `powercfg` / `wmic` / `ipconfig` / `curl`，在 Win11 24H2+ 部分组件已移除

### 测试

```bash
# src-tauri 单元测试（脚本把临时与运行数据落点固定到 build/tmp/，并注入 comctl32 v6 manifest）
bash scripts/test-lib.sh

# 单个集成测试
cd src-tauri && cargo test --test hardware_contract

# 数据源采集 crate
cd datasource && cargo test && cargo check --all-targets   # 47 单测；check 要求零警告

# LHM sidecar（.NET 8）
cd sidecar-lhm/tests/LhmSidecar.Tests && dotnet test -c Release
```

> `scripts/test-lib.sh` 需要 Windows SDK 的 `mt.exe` 注入 comctl32 v6 manifest：测试二进制缺少
> 该 manifest 时会以 `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)` 崩溃。干净环境请以
> `REQUIRE_MANIFEST=1 bash scripts/test-lib.sh` 运行，缺 SDK 时会明确失败而非静默降级。

### 驱动采集模块单独开发（可选）

```bash
cd datasource
cargo test      # 47 个单元测试（注册表/服务/电源/网络/HTTP/激活/磁盘/SMART）
cargo check --all-targets   # 零警告门禁
```

### 项目结构

```
secm/
├── src/                          # React 19 前端
│   ├── components/               # PageHeader / CardHeaderWithIcon / SectionTitle /
│   │                             # EmptyState / StatusDot / DiagOutput / LiveIndicator /
│   │                             # NetworkConfigEditor / DiskSmartCard / ui/（shadcn 组件）
│   ├── contexts/                 # NetIfaceContext（网络流量单数据源共享轮询）
│   ├── hooks/                    # useStreaming（Channel 流式封装）、useVersion、useIperf3、use-mobile
│   ├── layouts/                  # MainLayout（侧边栏 + 右侧日志栏 + 自绘标题栏）
│   ├── pages/                    # Dashboard / Cleanup / Network / NetConfig /
│   │                             # Settings / Services / Environment / Hardware / About
│   ├── lib/                      # cn() / formatBytes() / diskSmart() / iperf3 / logs / logger
│   ├── App.tsx                   # 路由配置（TooltipProvider + NetIfaceProvider）
│   └── main.tsx                  # 入口
├── src-tauri/                    # Rust + Tauri 后端
│   ├── src/
│   │   ├── main.rs               # 入口（无提权；需要管理员的功能由各命令内部检查）
│   │   ├── lib.rs                # AppState、插件注册、setup、Tauri 命令注册
│   │   ├── commands.rs           # 命令处理器（宏脚手架：cmd_result / cmd_val / cmd_log）
│   │   ├── sensor.rs             # 对外 SensorData 投影层（sidecar 快照 → 前端契约；无驱动探测）
│   │   ├── lhm.rs                # LHM sidecar 生命周期/健康检查/轮询/降级/tier-2 提权
│   │   ├── runtime_paths.rs      # 运行数据目录解析（唯一真源：开发期 build/runtime，发行版 exe 同级 runtime）
│   │   ├── driver_install/       # 驱动部署（pawnio.rs WHQL+SCM；PawnIO 为唯一 ring0 驱动）
│   │   ├── cleanup.rs            # 清理优化（缓存/临时文件/进程/优先级/DNS 刷新）
│   │   ├── network.rs            # 网络诊断（Ping/Traceroute/Nslookup/STUN/iperf3）
│   │   ├── net_config.rs         # 网络配置应用（netsh 静态 IP/DHCP/DNS/IPv6、MAC 注册表修改）
│   │   ├── dhcp_probe.rs         # DHCP 服务器探测（DISCOVER/OFFER 解析）+ 深度检查
│   │   ├── dhcp_residue.rs       # DHCP 注册表残留清理（已移除网卡的幽灵基线；清理前自动备份）
│   │   ├── settings.rs           # 系统设置（HAGS/游戏模式/VRR/电源计划/核心调度/服务）
│   │   ├── hpt.rs                # 高精度计时器开关（bcdedit）
│   │   ├── nvidia_drs.rs         # NVIDIA 电源管理模式（NVAPI DRS）
│   │   ├── environment.rs        # DirectX 诊断 / VC++ 运行库完整性
│   │   ├── game_env.rs           # 5 款游戏推荐设置预设（与设置页联动）
│   │   ├── sysinfo.rs            # Windows 系统信息（8 字段并行采集）
│   │   ├── net_stats.rs          # 网络流量统计（会话基线/速率快照/TCP 连接数/链路速度）
│   │   ├── ip_info.rs            # IP 信息（本地/公网 IPv4+IPv6/国内 IP 与位置）
│   │   ├── disk_info.rs          # 磁盘枚举 / SMART 健康
│   │   ├── ds_util.rs            # 数据源接入层（CollectError → 中文消息 + R8 日志 + use_legacy 开关）
│   │   ├── log.rs                # 用户日志（200 条环形缓冲 + 文件 + Tauri 事件）
│   │   ├── debug.rs              # 开发者诊断日志（500 条环形缓冲 + 磁盘落盘）
│   │   ├── temp_data.rs          # 诊断数据磁盘化（轮转/总量/保留策略）
│   │   ├── cancel.rs             # 命令取消注册表（ping/traceroute/nslookup/NAT/iperf3）
│   │   ├── iperf.rs              # iperf3 能力（参数编排/JSON 解析/版本门控/客户端/服务端/防火墙）
│   │   ├── proc_util.rs          # 外部命令执行（powershell 调用的共享封装）
│   │   └── tray.rs               # 系统托盘（右键菜单 / 左键恢复 / 关闭到托盘）
│   ├── tests/                    # 集成测试（hardware_contract / datasource_native / iperf3_contract /
│   │                             # iperf3_live / ds_adapter_integration / ds_regression_edge / ...）
│   ├── capabilities/             # Tauri v2 权限配置（core/event/dialog/fs）
│   ├── resources/                # 随包资源（driver/ PawnIO 三件套+源码 + lhm/ sidecar + iperf3/ + 许可文本）
│   ├── build.rs                  # tauri_build 构建脚本
│   ├── Cargo.toml
│   └── tauri.conf.json           # NSIS 打包配置（resources / 语言 / currentUser）
├── datasource/                   # 纯 Rust 驱动采集 crate（secm-datasource，P1-P14 底层）
│   └── src/                      # registry / service / power / netif / dns / http /
│                                 # activation / disk / error
├── sidecar-lhm/                  # LHM sidecar（.NET 8；LiteMonitor 数据源层移植，MPL-2.0 隔离）
│   ├── Snapshot.cs               # 契约 v3 DTO（snake_case）
│   ├── Hardware/                 # 移植的映射/规则层 + tier-1 PDH/网络/磁盘/电池采集
│   ├── tests/                    # xUnit 单测（映射规则/契约/提权/合并语义）
│   ├── CONTRACT-v3.md            # 冻结的 Rust ⇄ C# 接口契约（随源码分发）
│   └── licenses/                 # 第三方许可（GPL/LGPL/MPL/NOTICES）
├── third_party/                  # PawnIO 2.2.0 官方三件套 + 源码（WinRing0 副本仅署名/存档）
│                                 # + iperf3/（随包二进制 + 许可文本，SHA256SUMS 校验）
├── scripts/                      # stage-driver / stage-iperf3 / stage-lhm / audit（驱动签名 + 许可审计）
├── public/                       # 静态资源（赞助/抖音二维码）
├── package.json
├── vite.config.ts
└── tsconfig.json
```

## 故障排查

| 现象                                     | 可能原因                                       | 排查/解决                                                                                                      |
| ---------------------------------------- | ---------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| CPU 温度/功耗、主板卡片不显示（降级为 `none`） | tier-2 未生效：用户尚未放行 UAC / PawnIO 不可用 | 属**设计行为**（普通用户主路径）。硬件信息页点"需要管理员"引导卡触发**一次** UAC；或用 `GET /api/v1/privilege` 的 `tier2_reason` 查看具体原因；日志确认 PawnIO 服务状态 |
| 温度/功耗显示 `none` 且已授权                 | PawnIO 驱动未就绪 / 安全软件拦截               | 启动自愈链会自动部署 PawnIO（WHQL 三件套 + 签名门禁）；火绒等拦截时按引导卡放行 `PawnIO.sys`（错误码 0x430/0x431 需重启） |
| 授予 tier-2 后仍提示需要管理员            | UAC 被拒绝（错误码 1223）或提权伴生进程未接管  | 拒绝是**非致命且被记住**的状态（不会重复弹窗）——重新点击引导卡再授权一次；确认 45981 端口未被占用 |
| 环境检测页激活状态显示「无法获取」        | WMI 服务（winmgmt）未运行；激活数据本身缺失    | 检查 `Get-Service winmgmt`；新链路先查注册表近似、后走 WMI 兜底，两者都失败才降级占位（`ds_util::err_message` 有具体原因） |
| 最新补丁显示「无法获取」                  | 注册表 HotFix 键缺失（个别精简系统）           | `reg query HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\HotFix` 查看 KB 子键；日志 `debug.log` 有 `[registry]` 错误详情 |
| 服务列表为空 / 提示权限不足               | 非管理员枚举 SCM 受限                          | 以管理员运行 SECM（UAC 提权）；`NeedsAdmin` 错误会明确提示"需要管理员权限"                                      |
| 公网 IP 显示「—」                         | 网络不通 / 域名被墙 / 超时（5s）               | 新链路走 `ureq` + 3 个白名单 URL（ip-api.com / api64.ipify.org / myip.ipip.net），任一失败自动降级占位；检查网络与代理 |
| 网络配置修改失败                          | 非管理员运行 / netsh 输出解析失败              | 以管理员运行；后端返回 netsh 原始错误消息（GBK 解码）；修改 MAC 前会备份原值，失败可恢复 |
| DHCP 检测到多台服务器 / 跨网段 OFFER      | 多 DHCP 冲突 / 中继（ip helper）/ 旁路由 / **已移除网卡的残留记录** | 深度检查输出逐台诊断：同网段非网关=冲突风险，跨网段=OFFER 经中继转发；按建议清单检查路由器/AP/软路由。若某服务器已不存在（如网卡被拔出）仍被检出，点「清理残留」删除其历史 DHCP 记录（清理前自动备份至运行目录 `diag\`） |
| 温度/风扇/电压无数据（硬件页降级提示）    | tier-2 未授权 / PawnIO 未部署 / 安全软件拦截   | 点击引导卡授权一次（tier-2）；管理员执行一次驱动部署（PawnIO WHQL 自动装）；火绒拦截时按引导放行 PawnIO |
| 启动后日志目录无文件                       | 运行目录未创建（权限不足 / exe 所在目录只读）  | 确认运行目录可写（位置见「数据与日志存储」）；exe 装在只读位置时会自动回退 `%APPDATA%\SECM\`；诊断目录自动维护（单文件 100MB 轮转 / 目录 1GB 上限 / 7 天清理） |
| 疑似旧链路残留（进程树出现 powershell 等）| 使用了 `--features legacy-data-source` 构建     | 默认构建即新驱动；legacy 仅在过渡期调试用，见「数据源切换」                                                        |
| 依赖版本冲突（windows-core 报错）         | wmi crate 与 tauri 的 windows 依赖版本漂移     | `cd src-tauri && cargo update --precise 0.61.2`（windows-core 统一版本），见「开发环境配置 → 编译说明」          |
| 集成测试失败（hardware_contract 等）      | 环境权限/驱动未装 / 上游重构未落地             | `cargo test --test hardware_contract`、`--test datasource_native`、`--test ds_adapter_integration`、`--test ds_regression_edge`；`hardware_contract` 断言 sidecar 缺失时的降级与"不伪造 0"契约（无需驱动） |

> 通用排查入口：运行目录下的 `diag\debug.log`（模块级日志，含 `[API] 操作` 错误详情，R8 规范）；应用右侧栏日志卡片可实时查看与导出。运行目录位置见「数据与日志存储」。

## 数据与日志存储

程序运行数据全部落在**运行目录**下，解析规则由 `src-tauri/src/runtime_paths.rs` 单点定义：

| 场景 | 运行目录 |
| ---- | -------- |
| 开发调试（debug 构建） | `<项目根>/build/runtime/` |
| 便携版（release，exe 目录可写） | `<exe 同级>/runtime/` |
| 安装版装在只读位置（exe 目录不可写） | `%APPDATA%\SECM\`（回退，保持可用） |
| 测试与排障 | `SECM_RUNTIME_DIR` 环境变量指定的绝对路径 |

> WebView2 User Data 目录**不在**运行目录内：保持 Tauri 默认的
> `%LOCALAPPDATA%\<identifier>`（即 `%LOCALAPPDATA%\com.secm.app`）。
> 原因见 `runtime_paths.rs` 模块文档与 CHANGELOG：把 WebView2 UDF 放进项目目录会
> 使其落入带低完整性标签的受限 ACL 根目录，实测会导致外壳交互（如托盘登记）失败。

| 路径（相对运行目录）        | 内容                                    |
| --------------------------- | --------------------------------------- |
| `logs\<日期>.log`           | 用户日志（文件持久化，按天分文件）      |
| `logs\secm-lhm-sidecar.log` | LHM sidecar 标准输出重定向（排障入口）  |
| `diag\sensor.log`           | 传感器诊断日志（每秒采样）              |
| `diag\debug.log`            | 模块级调试日志（TRACE/INFO/WARN/ERROR） |
| `diag\driver.log`           | 驱动部署日志（PawnIO 安装/自愈）        |
| `diag\chart_history.jsonl`  | 图表历史采样点（启动时恢复曲线）        |
| `diag\dhcp-backup-*.json`   | DHCP 残留清理前的注册表备份             |
| `data\` `cache\` `state\`   | 预留的数据 / 缓存 / 状态目录            |

首次以新布局启动时会**一次性复制**历史 `%APPDATA%\SECM` 下的 `logs/`、`diag/`（含
`chart_history.jsonl`）到运行目录，完成后写入 `state/migrated-from-appdata.marker`；
只复制不删除源目录，已存在的目标文件不覆盖。

诊断目录自动维护：单文件超 100MB 轮转（保留尾部 90MB）、目录总量超 1GB 删除最旧文件、启动时清理 7 天前文件。

## 引用声明

| 项目                                                             | 许可证           | 用途                           |
| ---------------------------------------------------------------- | ---------------- | ------------------------------ |
| [Tauri](https://github.com/tauri-apps/tauri)                     | MIT / Apache-2.0 | 桌面应用框架 + 官方插件        |
| [React](https://react.dev)                                       | MIT              | 前端 UI 框架                   |
| [shadcn/ui](https://ui.shadcn.com)                               | MIT              | UI 组件（基于 Radix UI）       |
| [Recharts](https://recharts.org)                                 | MIT              | 数据可视化                     |
| [lucide-react](https://lucide.dev)                               | ISC              | 图标库                         |
| [sysinfo](https://github.com/GuillaumeGomez/sysinfo)             | MIT              | 进程/磁盘/网络信息采集         |
| [surge-ping](https://github.com/kolbma/surge-ping)               | MIT              | ICMP Ping / Traceroute 实现    |
| [trust-dns-resolver](https://github.com/hickory-dns/hickory-dns) | MIT / Apache-2.0 | DNS 解析（已更名 hickory-dns） |
| [windows-sys](https://github.com/microsoft/windows-rs)           | MIT / Apache-2.0 | Win32 API 绑定（服务/电源/网络/DNS/注册表） |
| [winreg](https://github.com/gentoo90/winreg-rs)                  | MIT              | Windows 注册表读写             |
| [ureq](https://github.com/algesten/ureq)                         | MIT / Apache-2.0 | 纯 Rust HTTP 客户端（公网 IP 采集） |
| [wmi](https://github.com/ozankasikci/wmi-rs)                     | MIT              | WMI 查询（激活状态兜底）       |
| [thiserror](https://github.com/dtolnay/thiserror)                | MIT / Apache-2.0 | 采集错误模型 CollectError      |
| [LiteMonitor](https://github.com/Diorser/LiteMonitor)            | MIT              | **硬件数据源层移植**：PDH 性能计数器采集 + 完整映射/规则层（SensorMatcher / HardwareRules / SensorMap / FanMapper / ComponentProcessor / DiskManager / NetworkManager / BatteryService） |
| [LibreHardwareMonitorLib](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor) | MPL-2.0 | CPU/主板 温度、功耗、电压传感（独立 sidecar 进程隔离，源码随包；0.9.6） |
| [PawnIO](https://github.com/namazso/PawnIO)                     | GPL-2.0-or-later（附例外条款） | **唯一 ring0 内核驱动**（WHQL，SCM 服务部署，tier-2 提权伴生进程使用）；PawnIO.Modules 0.1.6 为 LGPL-2.1 |
| [HidSharp](https://github.com/IntergatedCircuits/HidSharp)       | Apache-2.0       | LHM sidecar 传递依赖（HID 访问）|
| .NET 8 Runtime（自包含发布）                                      | MIT              | LHM sidecar 运行时（随包分发）  |
| [STUN RFC 3489/5780](https://tools.ietf.org/html/rfc3489)        | IETF             | NAT 类型检测协议               |

完整许可清单见应用内「关于 → 第三方许可」页面；依赖许可审计脚本见 `scripts/audit/`。

## 许可证

MIT License — Copyright (c) 2026 SECM Team
