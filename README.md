# SysEnv Console Manager (SECM)

> Windows 10/11 系统环境管理桌面工具 — 硬件监控、清理优化、网络诊断、系统设置、环境检测一站式平台

[![Version](https://img.shields.io/badge/version-v1.19.0-blue)](CHANGELOG.md)
[![License](https://img.shields.io/badge/license-MIT-green)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2021_edition-orange)](https://www.rust-lang.org/)
[![Tauri](https://img.shields.io/badge/tauri-2.x-purple)](https://tauri.app/)

## 功能特性

应用包含 8 个页面（React Router 路由），全部功能由 Rust 后端 **70 条 Tauri 命令**支撑：

| 页面         | 路由           | 功能                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| ------------ | -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **硬件信息** | `/`            | 每秒轮询传感器（CPU 占用/频率、内存、磁盘）；CPU/GPU/内存 60 秒趋势图 + 下载/上传速率趋势图（图表历史持久化，跨重启恢复）；网络流量卡片（各网卡/总量切换、0.5s–5s 轮询间隔、活跃 TCP 连接数、链路协商速度）；磁盘存储卡片与 **SMART 健康状态**（型号/容量/温度/健康度，损坏风险黄色警示）                                                                                                                                                                                                                                       |
| **清理缓存** | `/cleanup`     | DNS 缓存刷新（`DnsFlushResolverCache`，dnsapi）；临时文件清理（TEMP / Windows Temp / Prefetch）；工作集修剪（释放当前进程内存，需管理员）；着色器缓存清理（NVIDIA / AMD / DirectX / Steam 全部库目录 + CS2 深度缓存 / 一键全清，被占用文件标记重启后删除）；进程管理（内存 Top 200、名称/PID 搜索、6 档优先级设置）；执行结果追溯面板                                                                                                                        |
| **网络诊断** | `/network`     | **网络配置（可编辑）**：物理地址 MAC / IPv4 地址+掩码+默认网关 / IPv4 DNS / 链路本地 IPv6 / IPv6 网关与 DNS，DHCP⇄静态一键切换，静态设置或 MAC 注册表修改（重启网卡生效，应用前备份原值）；**DHCP 服务器检测**（DHCPDISCOVER 主动探测 + 注册表基线合并，1 台健康 / 多台冲突警告）与 **深度检查**（逐台 ping 可达性 + 同网段/网关拓扑判定，跨网段 OFFER 识别）；网站测试（4 个默认站点 + 自定义增删改，localStorage 持久化，手动/自动检测）；Ping（ICMP，可配次数/间隔/包大小/TTL/截止时间/连续模式）；Traceroute（最大 64 跳，每跳 3 探针）；Nslookup（A / AAAA 记录）；NAT 类型检测（15 个 STUN 服务器 + 自定义，RFC 3489 三阶段检测，NAT0–NAT4 分类）；端口连通性检测；iperf3 带宽测试（需系统已安装 iperf3）；全部工具支持流式输出与取消 |
| **系统设置** | `/settings`    | 系统优化开关（GPU 硬件加速调度 HAGS / 游戏模式 / 窗口化游戏优化 / 鼠标精准度 / 可变刷新率 VRR / **关闭高精度计时器**，均需管理员/部分重启生效）；电源计划（注册表读取、滑动开关切换、删除确认、卓越性能自动导入并激活）；**核心线程调度策略**（异类线程/短运行线程，AC/DC 六档，AMD/Intel 混合架构均受益）；**NVIDIA 显卡电源管理模式**（NVAPI DRS 读写，自适应/最高性能/最佳功率三档）；服务管理（全量服务枚举、搜索、启动/停止、启动类型 自动/手动/禁用）                                                                                                    |
| **环境检测** | `/environment` | 系统信息（版本/Build/架构/安装日期/激活状态/最新补丁/启动模式，8 字段并行采集）；游戏环境（CS2 / 三角洲行动 / 瓦洛兰特 / PUBG / 英雄联盟 5 款预设推荐设置与当前系统状态对比，**推荐项可一键切换并与系统设置页开关实时联动**）；DirectX 诊断（版本/GPU 适配器/D3D11/D3D12/WDDM）；VC++ 运行库完整性（2005–2022 共 12 项）；AI 开发工具（Claude Code / Codex / Gemini CLI / Grok / Hermes / Ollama / OpenCode / OpenClaw / PI Agent / CC Switch 共 10 项并行检测、npm 最新版本对比、一键安装/升级） |
| **硬件检测** | `/hardware`    | 手动刷新的硬件清单与实时状态（CPU / GPU / 内存 / 磁盘明细），支持按核心展开的 CPU 实时频率（PDH 估算层）                                                                                                                                                                                                                                                                                                                                             |
| **调试日志** | `/logs`        | Tauri 事件实时推送、200 条环形缓冲、Info/Warn/Error/Debug 级别筛选与统计、关键词搜索、自动滚动、导出为 .txt、清空缓冲；日志同步落盘 `%APPDATA%\SECM\logs\<日期>.log`                                                                                                                                                                                                                                                                              |
| **关于**     | `/about`       | 应用版本、构建信息、更新日志（内嵌 CHANGELOG.md）、第三方许可清单（6 组 50+ 项，含 LHM/PawnIO/HidSharp/.NET Runtime 等全部依赖）、赞助/抖音二维码                                                                                                                                                                                                                                                                                                                                                 |

系统级功能（`main.rs` / `lib.rs`）：

- **权限模型**：无需管理员权限启动 —— 温度/功耗主数据源为 LHM sidecar（独立提权部署 PawnIO 驱动），WinRing0/ACPI 为降级链，普通用户可读；需要管理员的功能（清理/服务管理等）由各命令独立检查权限并返回错误提示
- **驱动自愈链**：启动时按需部署/装载驱动（PawnIO WHQL 签名验证 → 服务创建；WinRing0 温度通道），安全软件（火绒等）拦截时提供精确放行指引与一键重试
- **单实例**：`tauri-plugin-single-instance` 防止多实例占用 WebView2 user-data-dir
- **系统托盘**：右键菜单（显示主窗口 / 退出）、左键单击恢复窗口、关闭按钮隐藏到托盘（`prevent_close`）
- **启动后台任务**：过期诊断文件清理（7 天）
- **诊断数据落盘**：`%APPDATA%\SECM\diag\` 下的 sensor.log / debug.log（单文件 100MB 轮转、目录 1GB 上限、7 天保留）+ chart_history.jsonl（图表历史持久化）

## 技术栈

| 层级       | 技术                                            | 版本            |
| ---------- | ----------------------------------------------- | --------------- |
| 桌面框架   | Tauri（含 CLI）                                 | 2.x（CLI 2.11） |
| Tauri 插件 | single-instance / store / dialog / shell / fs   | 2.x             |
| 后端语言   | Rust（edition 2021）                            | 稳定版          |
| 驱动采集   | secm-datasource（windows-sys / winreg / ureq / wmi / thiserror） | 0.1.0 |
| 温度传感   | LibreHardwareMonitor sidecar（.NET 8，MPL-2.0 进程隔离） | LHM 0.9.6 |
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
│  React 19 前端（8 页面，70 条 Tauri 命令 + Channel 流式事件） │
│  硬件信息 / 清理缓存 / 网络诊断 / 系统设置                    │
│  环境检测 / 硬件检测 / 调试日志 / 关于                        │
└──────────────────────┬──────────────────────────────────────┘
                       │ Tauri v2 IPC（invoke / Channel / event）
┌──────────────────────▼──────────────────────────────────────┐
│  Rust 后端（25 个模块 + datasource 独立 crate）               │
│  sensor / lhm / hardware / driver_install /                  │
│  cleanup / network / net_config / dhcp_probe / settings /   │
│  hpt / nvidia_drs / environment / game_env / sysinfo /      │
│  net_stats / ip_info / disk_info / ds_util / log / debug /  │
│  tray / cancel / temp_data / proc_util / commands / main    │
│                                                              │
│  ┌──────────────────────────────────────────────────────┐   │
│  │ secm-datasource（datasource/ 独立 crate，P1-P15 底层）│   │
│  │ registry / service / power / netif / dns / http /     │   │
│  │ activation / cpu_freq / disk / disk_io / error         │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────┬───────────────────────────────────┘
                          │ HTTP (127.0.0.1:45980)
┌─────────────────────────▼───────────────────────────────────┐
│  LhmSidecar（sidecar-lhm/，.NET 8 独立进程，MPL-2.0 隔离）   │
│  LibreHardwareMonitorLib 0.9.6 → PawnIO 内核驱动（ring0）    │
│  （进程隔离规避 GPL/MPL 传染，源码与许可随包分发）            │
└─────────────────────────────────────────────────────────────┘
```

### 驱动采集模块（`datasource/`，crate `secm-datasource`）

**数据源采集层已全面切换为纯 Rust**。
原 PowerShell 调用链与 `sc` / `powercfg` / `wmic` / `ipconfig` / `curl` 外部命令（P1-P15 共 15 个采集点）
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
| `cpu_freq`      | CPU 实时频率                                        | PDH 性能计数器（任务管理器同源，warmup 采样）+ ntapi 标称检测 |
| `disk`          | P14 磁盘枚举 / SMART 读取                           | 存储设备 IOCTL（SMART_RCV_DRIVE_DATA）     |
| `disk_io`       | P15 磁盘 IO 速度                                    | PDH 性能计数器（pdh.dll）                  |
| `error`         | 统一错误模型 `CollectError`（WinApi/Registry/Http/Parse/NeedsAdmin/NotFound） | thiserror          |

- **错误显式化**：新采集器返回 `Result<T, CollectError>`，由 `ds_util.rs` 映射为用户可见中文消息并打 R8 日志（含 API 名/操作名/详情）
- **回滚开关**：编译期 feature `legacy-data-source`（`ds_util::use_legacy()`）可一键回退旧 PowerShell/外部命令实现，见下文「数据源切换」
- **保留外部进程**（仅 2 处）：`iperf3` 带宽测试（K1，用户自装工具）；AI Agent 工具版本检测（K2，检测对象即外部工具）
- 独立构建/测试：`cd datasource && cargo build && cargo test`（48 单测）

### 温度/功耗数据源（`sensor.rs` / `lhm.rs` / `driver_install/`）

**降级链**：`lhm → winring0 → acpi → none`（`temp_source` 标识当前通道）：

| 通道      | 数据源                                                     | 说明                                  |
| --------- | ---------------------------------------------------------- | ------------------------------------- |
| **lhm**  | LibreHardwareMonitor sidecar（.NET 8 独立进程，HTTP 45980）| **主通道**：CPU 封装温度/功耗，PawnIO 内核驱动（WHQL 签名）经 ring0 读取；进程隔离规避 MPL/GPL 传染，源码与许可随包 |
| **winring0** | WinRing0x64.sys（GlobalSign 签名 v1.2.0.5，third_party/） | 内核温度/功耗通道（SMN 温度 / RAPL 功耗）；可能被安全软件拦截，提供放行指引 |
| **acpi**  | ACPI 温度（纯用户态 WMI）                                  | 最终降级（精度较低）                   |
| **none**  | 无                                                   | 驱动未就绪，Dashboard 显示一次性诊断摘要卡（FrequencyDiagCard，一键复制上报） |

> 自研内核驱动 hwmon-driver 已从项目移除（2026-09 开源整理）；不再分发自签名测试证书与自研驱动二进制。

- **驱动部署**（`driver_install/`）：PawnIO 随包分发 WHQL 三件套（`resources/driver/`，源自 `third_party/PawnIO/`）+ GPL-2.0 源码包，签名验证门禁后创建 SCM 服务并启动（幂等，30s 缓存）；WinRing0 服务自愈链（错误码 0x430/0x431 分类 + 2 秒重试）；安全软件拦截时提供精确放行步骤与一键重试
- CPU 频率：PDH 实时估算（任务管理器同源，首调 warmup 补齐采样间隔）+ ntapi 标称值疑似判定；降级链失败原因透出 diag 字符串
- 内存：`GlobalMemoryStatusEx` → sysinfo；磁盘：sysinfo + SMART（P14/P15）
- 集成测试：`cargo test --test hardware_integration` / `datasource_native`（采集层契约 + 降级断言）

## 构建产物

| 类型   | 文件                                                               | 位置                                    |
| ------ | ------------------------------------------------------------------ | --------------------------------------- |
| 便携版 | `sysenv-console-manager.exe`                                       | `src-tauri/target/release/`             |
| 安装版 | `SysEnv Console Manager_<版本>_x64-setup.exe`（NSIS，currentUser） | `src-tauri/target/release/bundle/nsis/` |
| 便携包 | `SECM_<版本>_portable.zip`（解压即用，免安装）                     | `dist_releases/<版本>/`                 |

**发行归档约定**：每次发版将最终产物归档至 `dist_releases/<版本>/`（唯一权威交付位置），包含安装版 setup.exe、便携版 exe、便携压缩包（zip）、`resources/driver/`、`resources/lhm/`、`SHA256SUMS.txt` 校验和。`src-tauri/target/` 为中间构建产物，可随时清理重建。

安装包内置资源：随包分发第三方驱动（PawnIO WHQL 三件套 + GPL 源码、WinRing0x64.sys，均源自 `third_party/`）与 LHM sidecar（.NET 8 自包含发布 + 完整源码 + 许可文本）。构建前需执行 `npm run stage:driver` / `npm run stage:lhm` 就绪产物（推荐一键 `npm run tauri:build`）。

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

> 普通用户可直接运行；清理、服务管理等操作由各命令内部检查管理员权限。

### 常用命令

```bash
npm run build          # 前端类型检查 + 生产构建
npm run lint           # ESLint 检查
npm run format         # Prettier 格式化
npm run stage:driver   # 就绪驱动产物（WinRing0 / PawnIO 三件套 + 源码）
npm run stage:lhm      # 就绪 LHM sidecar 发布产物 + 许可文本
npm run lhm:publish    # 构建 LHM sidecar（dotnet publish，win-x64 self-contained）
node scripts/audit/licenses-npm.mjs --copyleft   # 依赖许可审计（npm）
node scripts/audit/licenses-cargo.mjs --copyleft # 依赖许可审计（Rust crates）
```

### 数据源切换（新驱动 ⇄ legacy 回滚）

数据源采集层默认使用纯 Rust 驱动（`secm-datasource`）。过渡期保留编译期开关：

```bash
# 默认：纯 Rust 驱动采集（P1-P15 全部走 Win32/注册表/HTTP）
npm run tauri dev

# 回退：启用 legacy-data-source feature，回到旧 PowerShell/外部命令实现
cd src-tauri && cargo build --features legacy-data-source
```

- 切换由 `src-tauri/src/ds_util.rs::use_legacy()`（编译期 `cfg!(feature = "legacy-data-source")`）控制，运行时不可变，前端零感知
- **不建议在发布包中启用 legacy**：该链路依赖 `powershell.exe` / `sc` / `powercfg` / `wmic` / `ipconfig` / `curl`，在 Win11 24H2+ 部分组件已移除

### 驱动采集模块单独开发（可选）

```bash
cd datasource
cargo test      # 48 个单元测试（注册表/服务/电源/网络/HTTP/激活/CPU 频率/磁盘）
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
│   ├── hooks/                    # useStreaming（Channel 流式封装）、useVersion
│   ├── layouts/                  # MainLayout（侧边栏 + 网络信息卡片）
│   ├── pages/                    # Dashboard / Cleanup / Network / Settings /
│   │                             # Environment / Hardware / Logs / About
│   ├── lib/                      # cn() / formatBytes() / diskSmart() 工具
│   ├── App.tsx                   # 路由配置（TooltipProvider + NetIfaceProvider）
│   └── main.tsx                  # 入口
├── src-tauri/                    # Rust + Tauri 后端
│   ├── src/
│   │   ├── main.rs               # 入口（无提权；需要管理员的功能由各命令内部检查）
│   │   ├── lib.rs                # AppState、插件注册、setup、70 条命令注册
│   │   ├── commands.rs           # 命令处理器（宏脚手架：cmd_result / cmd_val / cmd_log）
│   │   ├── sensor.rs             # 传感器主链路（频率/温度/功耗降级链）
│   │   ├── lhm.rs                # LHM sidecar 生命周期/健康检查/轮询/降级
│   │   ├── hardware.rs           # 硬件页清单与实时状态
│   │   ├── driver_install/       # 驱动部署（pawnio.rs WHQL+SCM / winring0.rs 温度通道）
│   │   ├── cleanup.rs            # 清理优化（缓存/临时文件/进程/优先级/DNS 刷新）
│   │   ├── network.rs            # 网络诊断（Ping/Traceroute/Nslookup/STUN/iperf3）
│   │   ├── net_config.rs         # 网络配置应用（netsh 静态 IP/DHCP/DNS/IPv6、MAC 注册表修改）
│   │   ├── dhcp_probe.rs         # DHCP 服务器探测（DISCOVER/OFFER 解析）+ 深度检查
│   │   ├── settings.rs           # 系统设置（HAGS/游戏模式/VRR/电源计划/核心调度/服务）
│   │   ├── hpt.rs                # 高精度计时器开关（bcdedit）
│   │   ├── nvidia_drs.rs         # NVIDIA 电源管理模式（NVAPI DRS）
│   │   ├── environment.rs        # DirectX / VC++ / AI Agent 工具检测
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
│   │   ├── proc_util.rs          # 外部命令执行（保留 iperf3/AI Agent 工具检测用）
│   │   └── tray.rs               # 系统托盘（右键菜单 / 左键恢复 / 关闭到托盘）
│   ├── tests/                    # 集成测试（datasource_native / ds_adapter_integration /
│   │                             # ds_regression_edge / hardware_integration）
│   ├── capabilities/             # Tauri v2 权限配置（core/event/dialog/shell/fs）
│   ├── resources/                # 随包资源（driver/ 驱动三件套 + lhm/ sidecar + 许可文本）
│   ├── build.rs                  # tauri_build 构建脚本
│   ├── Cargo.toml
│   └── tauri.conf.json           # NSIS 打包配置（resources / 语言 / currentUser）
├── datasource/                   # 纯 Rust 驱动采集 crate（secm-datasource，P1-P15 底层）
│   └── src/                      # registry / service / power / netif / dns / http /
│                                 # activation / cpu_freq / disk / disk_io / error
├── sidecar-lhm/                  # LHM sidecar（.NET 8，LibreHardwareMonitorLib，MPL-2.0 隔离）
│   └── licenses/                 # 第三方许可（GPL/LGPL/MPL/NOTICES）
├── third_party/                  # WinRing0x64.sys（签名版）、PawnIO 2.2.0 官方三件套 + 源码
├── scripts/                      # stage-driver / stage-lhm / audit（许可审计）
├── public/                       # 静态资源（赞助/抖音二维码）
├── package.json
├── vite.config.ts
└── tsconfig.json
```

## 故障排查

| 现象                                     | 可能原因                                       | 排查/解决                                                                                                      |
| ---------------------------------------- | ---------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| 温度/功耗显示降级（acpi/none）            | LHM sidecar 未启动 / PawnIO 驱动未就绪         | 应用内硬件页查看 `temp_source`；日志确认 PawnIO 服务状态；管理员运行后驱动自愈链自动部署；PawnIO 装载失败时提供火绒等安全软件放行指引 |
| 环境检测页激活状态显示「无法获取」        | WMI 服务（winmgmt）未运行；激活数据本身缺失    | 检查 `Get-Service winmgmt`；新链路先查注册表近似、后走 WMI 兜底，两者都失败才降级占位（`ds_util::err_message` 有具体原因） |
| 最新补丁显示「无法获取」                  | 注册表 HotFix 键缺失（个别精简系统）           | `reg query HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\HotFix` 查看 KB 子键；日志 `debug.log` 有 `[registry]` 错误详情 |
| 服务列表为空 / 提示权限不足               | 非管理员枚举 SCM 受限                          | 以管理员运行 SECM（UAC 提权）；`NeedsAdmin` 错误会明确提示"需要管理员权限"                                      |
| 公网 IP 显示「—」                         | 网络不通 / 域名被墙 / 超时（5s）               | 新链路走 `ureq` + 3 个白名单 URL（ip-api.com / api64.ipify.org / myip.ipip.net），任一失败自动降级占位；检查网络与代理 |
| 网络配置修改失败                          | 非管理员运行 / netsh 输出解析失败              | 以管理员运行；后端返回 netsh 原始错误消息（GBK 解码）；修改 MAC 前会备份原值，失败可恢复 |
| DHCP 检测到多台服务器 / 跨网段 OFFER      | 多 DHCP 冲突 / 中继（ip helper）/ 旁路由       | 深度检查输出逐台诊断：同网段非网关=冲突风险，跨网段=OFFER 经中继转发；按建议清单检查路由器/AP/软路由 |
| 温度/风扇/电压无数据（硬件页降级提示）    | 驱动未安装 / 安全软件拦截                      | 管理员执行一次驱动部署（PawnIO WHQL 自动装）；火绒拦截时按引导放行 WinRing0/PawnIO |
| 启动后日志目录无文件                       | `%APPDATA%\SECM\diag\` 未创建（权限）          | 检查 `%APPDATA%\SECM\` 可写；诊断目录自动维护（单文件 100MB 轮转 / 目录 1GB 上限 / 7 天清理）                  |
| 疑似旧链路残留（进程树出现 powershell 等）| 使用了 `--features legacy-data-source` 构建     | 默认构建即新驱动；legacy 仅在过渡期调试用，见「数据源切换」                                                        |
| 依赖版本冲突（windows-core 报错）         | wmi crate 与 tauri 的 windows 依赖版本漂移     | `cd src-tauri && cargo update --precise 0.61.2`（windows-core 统一版本），见「开发环境配置 → 编译说明」          |
| 集成测试失败（datasource_native 等）      | 环境权限/驱动未装                              | `cargo test --test datasource_native`、`--test ds_adapter_integration`、`--test hardware_integration`；驱动依赖测试在无驱动环境走降级断言 |

> 通用排查入口：`%APPDATA%\SECM\diag\debug.log`（模块级日志，含 `[API] 操作` 错误详情，R8 规范）；应用内「调试日志」页可实时查看与导出。

## 数据与日志存储

| 路径                                      | 内容                                    |
| ----------------------------------------- | --------------------------------------- |
| `%APPDATA%\SECM\logs\<日期>.log`          | 用户日志（文件持久化，按天分文件）      |
| `%APPDATA%\SECM\diag\sensor.log`          | 传感器诊断日志（每秒采样）              |
| `%APPDATA%\SECM\diag\debug.log`           | 模块级调试日志（TRACE/INFO/WARN/ERROR） |
| `%APPDATA%\SECM\diag\chart_history.jsonl` | 图表历史采样点（启动时恢复曲线）        |

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
| [LibreHardwareMonitorLib](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor) | MPL-2.0 | CPU/GPU 温度与功耗传感（独立 sidecar 进程隔离，源码随包） |
| [PawnIO](https://github.com/namazso/PawnIO)                     | GPL-2.0-or-later（附例外条款） | 内核驱动（WHQL，SCM 服务部署）；PawnIO.Modules 0.1.6 为 LGPL-2.1 |
| [HidSharp](https://github.com/IntergatedCircuits/HidSharp)       | Apache-2.0       | LHM sidecar 传递依赖（HID 访问）|
| .NET 8 Runtime（自包含发布）                                      | MIT              | LHM sidecar 运行时（随包分发）  |
| [STUN RFC 3489/5780](https://tools.ietf.org/html/rfc3489)        | IETF             | NAT 类型检测协议               |

完整许可清单见应用内「关于 → 第三方许可」页面；依赖许可审计脚本见 `scripts/audit/`。

## 许可证

MIT License — Copyright (c) 2026 SECM Team
