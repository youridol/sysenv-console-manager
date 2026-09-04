## [v1.19.0] - 2026-09-05
### 变更（开源整理：移除自研内核驱动 + 安全修复）
- 移除自研内核驱动 hwmon-driver：删除 `hwmon.rs`（`#[path]` 外部 ioctl.rs 使源码树无法独立编译）、驱动 stage/签名脚本中的 hwmon 产物、`resources/driver` 的 hwmon 文件、证书与自研安装器、`hwmon_integration.rs` 测试；温度/功耗降级链收敛为 `lhm → winring0 → acpi → none`（`driver_install` 精简 L0 通道，WinRing0/PawnIO/ACPI 保留）
- 安全修复：`append_frontend_log` 超长消息按 UTF-8 字符边界安全截断（原字节切片在中文/emoji 边界可 panic，前端可控）；移除生产窗口 `--remote-debugging-port=9223`（本机任意进程可经 CDP 完全接管 WebView2 的风险口）；CSP `connect-src` 收紧（去掉通配 https:/http:）
- 清理个人/内部残留：`environment.rs` 注释去除本机用户名路径；移除内部验收文档（tests/*.md 含机器路径与已删套件引用）；删除失效 CDP 调试脚本（cdp_eval/inject_nav/sample）
- 文档与合规：README 同步（徽章版本、温度链、目录结构、构建说明）；third_party 说明重写（移除 hwmon 引用与官方安装器 exe）；`index.html` 移除不存在的 favicon 引用
## [v1.18.6] - 2026-08-14
### 新增
- AI 环境 · npm 环境检测：node/npm 版本、全局目录（prefix/root）、registry、全局包统计、AI/MCP 相关全局包标注（environment.rs check_npm_environment + 前端 npm 环境卡片）
- AI 环境 · 移除卸载管理：npm uninstall -g 卸载 AI 工具（白名单 + 格式双重校验防注入），前端工具行新增卸载按钮（确认提示 + 状态刷新）
- AI 环境 · MCP 服务器管理：13 个常用 MCP 服务器白名单（@modelcontextprotocol/* 官方集 + Playwright），安装/卸载一键管理（npm install/uninstall -g），前端 MCP 卡片展示版本与状态
- AI 环境 · Skills & 插件管理：扫描各 AI 工具（claude/dsh/pi/codex/opencode/gemini）用户目录的 skills/plugins（10 个约定位置），列出名称/类型/描述/路径，前端扩展卡片展示
## [v1.18.5] - 2026-08-14
### 修复（AI 检测版本错乱 + 统一 npm 版本管理）
- 修复 AI 工具版本检测错乱：版本权威源统一为 `npm list -g <pkg> --depth=0 --json`（JSON 解析，与 npm view latest 同格式可正确比较升级）；不依赖各工具 --version 输出格式差异（environment.rs）
- 修复版本字符串清理：gemini-cli/hermes-cli 前缀剥离失败、ANSI 颜色、git hash 后缀等旧 bug；新增 clean_version 增强（多前缀变体 + ANSI 清理 + 版本模式提取）及单测
- 修复工具路径查找：`npm prefix -g` 动态解析全局目录（旧硬编码 USERPROFILE\.npm-global 在其他机器/自定义 prefix 下找不到）；where 全覆盖 + prefix 候选 + 特殊路径三级查找
- 修正 OpenCode 包名：`@opencode-ai/sdk`（SDK 库）→ `opencode-ai`（CLI 包）——旧配置导致查错版本、装错包

### 新增
- DeepSeek Harness（DSH）：安装检测 + 版本管理（npm 包 @deepseek-ai/dsh，bin 名 dsh）——与全部 AI 工具统一走 npm 安装/升级/版本比对
## [v1.18.4] - 2026-08-14
### 修复
- 修复分发机器「硬件检测-磁盘信息-读取不到 SMART」：SMART 读取三级降级链增强——MSFT_PhysicalDisk WHERE 查询为空时（DeviceId 与盘号不一致的 RAID/多控制器环境）全量查询按索引匹配；Storage WMI 不可用时新增 Win32_DiskDrive（root\CIMV2，无 Storage 服务依赖）二级兜底；磁盘列表 IOCTL 全失败（普通权限）时新增 WMI 兜底枚举，保证磁盘可见（datasource/src/disk.rs）

### 新增（统一 debug.log）
- 统一日志出口：debug.rs append 同步落盘 %APPDATA%\SECM\diag\debug.log（原仅内存缓冲，文件几乎无写入）
- log crate 桥接：datasource crate 30 处 log::* 调用（disk/cpu_freq/registry/service 等）原被静默丢弃，现经 SecmLogBridge 转发到 SECM debug 体系（src-tauri/src/debug.rs + lib.rs 注册）
- 前端统一 logger：新增 src/lib/logger.ts（统一格式/级别/模块名/节流），批量替换 8 文件 19 处 console.*；WARN/ERROR 经新命令 append_frontend_log 上报后端落盘 debug.log，实现前后端日志统一
## [v1.18.3] - 2026-08-14
### 修复（NVIDIA 电源管理模式跨机器分发问题）
- 修复分发到其他机器时「NvAPI_DRS_GetSetting 失败: rc=0xFFFFFF60」（NVAPI_APPLICATION_PROFILE_NOT_FOUND）：新装 NVIDIA 驱动/干净配置的机器全局 DRS profile 无「Power management mode」设置项，原实现直接报错；现读取降级返回驱动默认（自适应），设置时构造完整设置项（settingName+settingId+DWORD 类型）新建设置并写入（nvidia_drs.rs）
- 修复 DRS 值字段读写错误：`value_length` 为二进制长度字段（DWORD=4），真实值在 `value_data` 前 4 字节；原实现读写 value_length 自洽"验证通过"但驱动实际应用全零值，用户设置的「最高性能优先/最佳功率」从未真实生效；已改为 value_data 读写并新增单元测试（nvidia_drs.rs）
- 修正 commands.rs 电源模式注释（0=自适应 1=最高性能优先 5=最佳功率）

### 新增
- nvidia_drs.rs 单元测试：DWORD 值字段读写 roundtrip、NOT_FOUND 错误判定，共 2 例
## [v1.18.2] - 2026-08-14
### 修复（全代码审查 BUG_AUDIT_2026-08-14，critical/major 项）
- 安全：npm 命令注入（CVE 级）——install_or_upgrade_tool / fetch_npm_latest 包名未校验直接拼接 cmd /c，恶意输入可执行任意命令；已加 npm 包名格式校验 + AI_TOOLS 白名单双重防护（environment.rs）
- 驱动：WinRing0 READ_PORT_DWORD 输入缓冲以 u16 指针声明 4 字节，高 16 位读栈垃圾可致读错寄存器；改为 u32 清零扩展（winring0.rs）
- 驱动：close_device 与并发 IO 竞态——句柄关闭后缓存未失效；新增 CLOSED 标志，关闭后新 IO 自动重开句柄（winring0.rs）
- 驱动：ACPI 热区 COM 调用使用 C 变参函数指针（ABI 未定义行为）；改为各方法精确函数指针类型（acpi.rs）
- 日志：rotate_file 按字节切片多字节 UTF-8 日志可 panic 并污染日志锁；改为字符边界切片 + O_TRUNC 重写（temp_data.rs）
- LHM：sidecar 崩溃自动重启路径不可达（ready 状态永不复位，崩溃后直接降级）；轮询失败时复位 ready（lhm.rs）
- 磁盘：NVMe available_spare > 100 时 100 - u8 整数下溢；改为 saturating_sub（disk_info.rs）
- 网络：IPv4 静态地址校验用 trim 值但执行传未 trim 值致 netsh 解析失败；统一执行 trim 值（net_config.rs）
- 前端：formatBytes 对 NaN/负值/超大值产出 undefined 单位；增加边界处理（utils.ts）
- 前端：流式消息 JSON.parse 无异常保护，异常数据可中断事件流；新增 safeParse 降级（Network.tsx）

### 新增
- environment.rs 单元测试：npm 包名校验（合法/注入载荷/白名单）共 3 例
## [v1.18.1] - 2026-08-14
### 修复
- 温度读取分发链路：恢复 LHM PawnIO 为主 ring0 通道，修正 sidecar 对 PawnIO WHQL 签名「已过期无法加载」的误判文案（签名带微软有效时间戳，Secure Boot 可正常加载）；PawnIO 未就绪时提示运行 SECM 驱动安装引导自动部署
- 驱动随包分发：补齐 hwmon.sys/hwmon.inf/hwmon.cat/WDRLocalTestCert.cer/hwmon.exe/WinRing0x64.sys/PawnIO.sys/PawnIO.cat/pawnio.inf 构建产物并 stage 到 resources/driver/，修复发行版「驱动文件缺失（resources\\driver）」导致温度通道全挂（channel=none）的问题
- 补齐 LhmSidecar.exe 随包产物（dotnet publish self-contained win-x64 → resources/lhm/publish/），修复 sidecar 缺失导致 LHM 温度源不可用

### 新增
- scripts/audit-driver-signatures.mjs：驱动签名审计（Authenticode 状态/签名者/证书有效期/时间戳/三级判定），接入 tauri:build 构建门禁（npm run audit:drivers）

### 变更
- 驱动通道策略：PawnIO（LHM 主，WHQL+有效时间戳）→ WinRing0（回退，签名已过期仅作最后兜底）→ ACPI；审计确认 PawnIO.sys/PawnIO.cat 可正常分发
# Changelog

本文件采用 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.0.0/) 格式。

## [v1.18.0] - 2026-08-13

### 修复

- **LHM sidecar ring0 后端切换 WinRing0（关键修复）**：目标机（华南 X99 + E5 v4，Secure Boot 开启）传感器全部不可用的根因——随包 PawnIO 2.2.0 驱动的 WHQL 签名已于 **2026-07-15 过期**，Secure Boot 下内核拒载（`CreateFileW \\.\PawnIO` 失败，此前被误报为「驱动未安装」）。sidecar 预检改为**双后端**：WinRing0（主，WHQL 交叉签名 + 时间戳，Secure Boot 可加载）或 PawnIO（回退）任一可达即可初始化，错误文案区分两类失败。

### 新增

- **WinRing0 用户态 Intel 温度读取**：`winring0_cpu_temp()` 新增 Intel family 6 路径（X99/X79 全系 E5 与消费级）——经 WinRing0 MSR 读 IA32_TEMPERATURE_TARGET (0x1A2) TjMax + PACKAGE_THERM_STATUS (0x1B1) 包温度（回退 0x19C 每核），算法与 Linux coretemp 一致（1°C 步进，忽略 Resolution 字段）；TjMax 越界/无效读数安全降级；新增 4 个解码/门控单测。目标机 WinRing0 驱动已加载成功，此路径立即可用。

## [v1.17.1] - 2026-08-13

### 修复

- **LHM sidecar 发行包回退 v1 旧契约（关键修复）**：v1.17.0 发布时 `dotnet publish` 产物未正确落盘，发行包内 LhmSidecar.dll 为 v1 旧版（仅 CPU 结构，19456 B），与 Rust 侧 v2 解析（contract_version=2 / gpu / motherboard / memory）不匹配，导致 GPU/主板/内存采集实际不生效。已从最新源码重新 `dotnet publish`（LhmSidecar.dll 25600 B，含 GpuSensorData/MotherboardSensorData/contract_version，199 文件）并替换发行资源后重新打包。

## [v1.17.0] - 2026-08-13

### 新增

- **LHM 全硬件采集契约 v2（GPU / 主板 SuperIO / 内存 SPD）**：LhmSidecar 全硬件数据契约升级至 v2——GPU 传感器数组（`gpu[]`：独立/核显温度、利用率、显存占用、功耗、显存温度）、主板传感器（SuperIO 风扇/温度/电压通道 + 芯片型号）、内存 SPD（厂商/型号/容量/频率），前端可全量消费主板传感器卡与内存型号。
- **LHM 温度/功耗/GPU 主数据源**：Dashboard 与硬件页数据链路切换——温度、功耗、GPU 信息以 LHM sidecar 采集为主数据源，Rust 侧 `sensor.rs` 消费 LHM 契约 v2 结构化数据，替代旧版分散轮询路径。
- **LHM 失败诊断无条件透出**：LHM sidecar 启动/采集失败时，诊断信息（错误码、阶段、建议）无条件向前端透出，不再因降级链遮蔽诊断细节。
- **主板传感器卡 / 内存型号 / GPU 占位文案前端更新**：硬件页新增主板传感器卡片（SuperIO 风扇转速、温度、电压），内存卡片显示 SPD 型号，GPU 卡片支持占位文案（LHM 未就绪时展示引导说明）。

### 修复

- **UAC 提权竞态修复**：应用启动提权（UAC 弹窗）与 LHM sidecar 首次拉起的竞态——sidecar 等待权限就绪后再初始化，避免提权窗口期采集空数据或误报失败。
- **残留 sidecar 进程清理**：应用退出或重启时清理残留 LhmSidecar.exe 进程，避免多实例 sidecar 并发占用采集通道。

## [v1.16.0] - 2026-08-13

### 新增

- **Intel RAPL 功耗实测（X79 / X99 / 全 Intel）**：用户态 WinRing0 新增 `cpu_is_intel_rapl()` 平台门控（GenuineIntel + family 6 + model ≥ 0x2A，Sandy Bridge 起全系覆盖），`winring0_rapl_energy()` 按平台分流——Intel 读 MSR 0x606（能量单位 esu）+ 0x611（PKG_ENERGY_STATUS 计数）差值换算实时功耗，AMD Zen 走既有 MSR 0xC0010299/0x29B；未知平台安全降级 TDP 估算，零写入零风险。
- **AMD 老平台（K10 / FX）k10temp 兼容温度**：`winring0_cpu_temp()` 扩展路径——family 10h/15h/16h 严格门控下读 PCI（bus 0 / dev 24 / func 3）寄存器 0xA4 温度字段（bits[21:31] × 0.125℃，与 Linux k10temp 同协议），字段无效安全降级（不误读、不 #GP），Intel 平台不触碰该通路。
- **TDP 表扩充 134 条**：Xeon E5 v1~v4 / E5-1600 v3/v4（E5-2690 135W 等）、AMD FX（9590/9370 220W、8350 125W 等）、Phenom II X6/X4、Intel 12 代（12900K 125W 等）、老桌面与至尊（3960X/5960X/9900K 等）；匹配策略顺序敏感（长关键词前置，如 "E5-2690 V3" 先于 "E5-2690"）。
- **SuperIO 补识别 IT8790E**：hwmon 驱动 superio.rs 新增 ID 0x8790（Linux it87 驱动有据可查），覆盖部分 X99 后段新 ITE 平台主板传感器。

### 修复

- **驱动 TjMax IVB-E 0x3E 修正**：hwmon-driver cpu.rs 中 model 0x3E 原误归 Haswell 组（100℃），修正为 Ivy Bridge-E（X79 至尊/服务器，105℃，与 Linux coretemp 一致）。
- **DHCP 深度检查配置缺失降级修复**：dhcp_probe 本地网络配置读取失败（IP/掩码缺失）时不再误报 critical 跨网段风险，降级为 warn 按不可判定处理。

## [v1.15.5] - 2026-08-13

### 变更

- **网络诊断页 DHCP 检测与网站测试并排两列**：DHCP 服务器检测与网站测试卡片改为 `grid grid-cols-1 lg:grid-cols-2` 左右并排（窄屏自动回退单列），网络工具卡保持整行，页面更紧凑。

## [v1.15.4] - 2026-08-13

### 变更

- **DNS 配置模式按钮组精简**：「自动 (DHCP)」文案改为「自动」，按钮组缩小一号（h-8 → h-7，内边距收窄），与网络配置页按钮组风格统一。

## [v1.15.3] - 2026-08-13

### 变更

- **IPv4/IPv6 模式按钮组精简**：「自动 (DHCP)」文案改为「自动」，按钮组整体缩小一号（h-8 → h-7，内边距收窄），分段组更紧凑。

## [v1.15.2] - 2026-08-13

### 变更

- **「添加网段」按钮合并进模式按钮组**：IPv4 配置的「自动 (DHCP) | 静态」按钮组尾部新增「+」图标项（样式同 ButtonGroup 分段组：无缝分段 + 首尾圆角），点击直接添加网段，取代原先 static 模式下独立的「添加网段」按钮；DHCP 模式下 + 禁用（悬停提示「静态模式下可添加网段」），IPv6 模式组不受影响。

## [v1.15.1] - 2026-08-13

### 变更

- **物理地址（MAC，高级）独立卡片**：网络配置页（/net-config）右列新增独立 MAC 卡片（`src/components/MacConfigCard.tsx`，Fingerprint 黄色警示主题），从 NetworkConfigEditor 中拆分；包含当前 MAC 展示、新物理地址输入、AlertDialog 确认修改（注册表覆盖重启网卡生效）与成功/失败反馈，与 DNS 卡共享当前上网网卡；NetworkConfigEditor 移除 MAC 区块（-60 行）。

## [v1.15.0] - 2026-08-13

### 新增

- **「网络配置」独立页面**：新增 /net-config 页面（src/pages/NetConfig.tsx，208 行），整体迁移网络诊断页的网络配置卡（NetworkConfigEditor 组件：适配器选择、DHCP/静态切换、IP/网关/前缀编辑、应用）与 DNS 配置卡（DnsConfigCard 组件：IPv4/IPv6 DNS 的 DHCP/静态切换、DoH 开关与模板选择），复用既有 `get_network_config` / `set_dns` / `get_doh_config` / `set_doh_config` 后端命令契约，后端零改动；侧边栏新增「网络配置」导航入口（Wifi 图标 blue 色系）。

### 变更

- **网络诊断页布局调整**：移除网络配置卡与 DNS 配置卡，保留网络工具与上网网卡信息，页面聚焦诊断场景；网络配置相关操作统一收敛至 /net-config 独立页面。

## [v1.14.0] - 2026-08-13

### 修复

- **网络配置仅显示当前上网网卡**：网络诊断页网络配置卡片不再展示全部非回环适配器，改为仅显示「状态 Up 且存在默认网关（IPv4 或 IPv6）」的上网网卡（过滤为空时回退到 Up 且有 IP 的网卡，仍为空显示明确提示）；多网卡同时满足时保留切换并注明筛选口径，避免物理网卡与 vEthernet/虚拟网卡混杂。

### 新增

- **DNS 配置独立卡片**：DNS 配置从网络配置编辑器拆出为独立卡片（src/components/DnsConfigCard.tsx），支持 IPv4/IPv6 DNS 的 DHCP/静态模式切换与服务器编辑，通过新增 `set_dns` 命令独立应用（只改 DNS 不动 IP 地址）；操作对象与网络配置卡一致（当前上网网卡）。
- **DoH（DNS over HTTPS）配置**：DNS 卡片内新增 DoH 区块，每个静态 DNS 服务器可独立开关 DoH 并选择模板（Cloudflare / Google / 阿里 / 腾讯预设 + 自定义 URL）；后端新增 `get_doh_config`（回显现有 DoH 记录）与 `set_doh_config`（批量应用，逐 IP 分步结果）命令，基于 Windows `Get/Set/Remove-DnsClientDohServerAddress` cmdlet（双模型兼容：接口级 / IP 级），管理员权限失败给出明确修复建议。

## [v1.13.1] - 2026-08-13

### 变更

- **移除侧边栏边缘竖线提示**：删除 `SidebarRail`（Toggle Sidebar 竖线拖拽条）；侧边栏底部已有折叠/展开按钮，竖线提示多余，避免折叠态右缘出现多余交互入口。

## [v1.13.0] - 2026-08-13

### 新增

- **AI 环境独立页面**：环境检测页的 AI 环境区域独立为 /ai-env 页面（src/pages/AiEnvironment.tsx），功能完整迁移（check_ai_tools 检测、fetch_ai_latest_versions 最新版本拉取、install_ai_tool 安装/升级、Skeleton 加载占位、手动安装标注、错误态）；侧边栏「环境检测」正下方新增「AI 环境」导航入口（Bot 图标 emerald 色系）。

### 重构

- **检测项清单渲染公共化**：新增 src/components/environment-shared.tsx 共享模块，DxCheck 类型 / renderChecks / SectionHeader / safeInvoke 从 Environment.tsx 拆出，DirectX / VC++ / AI 环境三处共用同一实现，避免双份漂移；Environment.tsx 精简（880 → 556 行），PageHeader 描述改为「系统信息 · DirectX 诊断 · VC++ 运行库」，四卡片布局保持两行两列。

## [v1.12.0] - 2026-08-13

### 新增

- **服务管理独立页面**：系统设置页右列的服务管理区域独立为 /services 页面（src/pages/Services.tsx），功能完整迁移（服务枚举、名称/显示名搜索过滤、启动类型设置 自动/手动/禁用、启动/停止、停止操作 AlertDialog 二次确认、操作结果 toast 反馈）；侧边栏「系统设置」下方新增「服务管理」导航入口；系统设置页由三列改为两列（系统优化 + 电源计划并排）。

## [v1.11.0] - 2026-08-12

### 重构

- **网络工具可合并项合并（纯重构，行为不变）**：`src/pages/Network.tsx` 消除 4 个巨型 switch 分发（runActiveTool / activeState / renderParams / 工具选择器内 toolLoading）与 5 个流式工具（ping/trace/nsl/nat/iperf）各自手写的同构骨架：
  - 新增 `useStreamingTool` hook 统一封装 useStreaming + loading + opId 竞态 + cmdId 取消，5 个流式工具共用同一 `ToolController` 契约；非流式 port 工具由 `usePortTool` 实现同构接口，6 工具统一走 `controllers` 映射分发。
  - 参数渲染数据驱动化：`ParamDef` 联合类型 + 参数工厂函数（num/sel/txt/dns/swt）+ 统一 `renderParams`，取代每个工具独立的参数 JSX。
  - 抽 `headProbe(url, timeoutMs)` 公共探测函数：网站测试与端口检测共用 fetch HEAD + AbortController 逻辑。
  - 抽 `ErrorBanner` 组件合并网络配置 / DHCP / 深度检查 3 处重复错误横幅；网络配置 idle/loading 两段合并。
  - 功能完整保留：6 工具参数（含连续 ping、NAT 自定义 STUN、iperf 端口/时长）、网站测试增删改/自动检测/localStorage、DHCP 检测与深度检查、网络配置编辑。文件 1925 → 1866 行。

## [v1.10.7] - 2026-08-12

### 变更

- **IPv4 网关展示回退为网段共享单框**：撤回 v1.10.6 的「地址-网关一一配对」展示（跨网段网关会被误显示为其他网段卡片的地址，造成分组混淆）；恢复网段子卡片内「同网段地址并列 + 掩码/网关组内共享单框」结构，同网段地址（如 192.168.3.238 与 192.168.3.254）仍合并到同一子卡片。

## [v1.10.6] - 2026-08-12

### 优化

- **同网段 IPv4 地址与网关同卡片展示**：网络配置编辑器网段子卡片内，每个 IPv4 地址配对显示自己的网关（读取时按 ipv4_gateway[i] 与地址一一对应回填，不再组内共享单框、不丢失同网段其他地址的网关）；掩码仍网段共享单框；提交时逐地址携带各自网关应用。

## [v1.10.5] - 2026-08-12

### 优化

- **IPv4 同网段地址排列到同一行**：网络配置编辑器按网段分组展示（网络号+掩码相同的地址合并到同一行，行内可增删同网段地址、掩码/网关组内共享；跨网段地址分列各行），提交时展平逐条应用。
- **网络接口按钮名称简化**：剥离 vEthernet 冗余前缀（vEthernet (Default Switch) → Default Switch），过长名称仍截断 + 悬停查看全名。
- **按钮组移除横向滚动条**：游戏环境/网络配置/网络工具三处按钮切换组件由 overflow-x-auto 改为 flex-wrap 换行排布，按钮过多时自动换行、不再出现滚动条。

## [v1.10.4] - 2026-08-12

### 修复

- **网络配置读取/编辑不到多个 IPv4 地址**：同一网卡配置多个 IPv4（跨网段）时界面仅显示第一个。修复：前端 IPv4 配置改为多行编辑（主地址 + 附加地址，支持添加/删除行，从 cfg 全量回填）；Rust 写入侧契约 ipv4/ipv4_mask/ipv4_gateway 升级为 ipv4s 列表，主地址 set address + 附加地址逐条 add address（跨网段，每条约可带独立网关）。
- **网络配置按钮切换页网卡名称过长**：接口切换按钮名称过长撑爆按钮。修复：按钮加 max-w-[10rem] + truncate 截断，悬停 title 显示完整名称。

## [v1.10.3] - 2026-08-12

### 修复

- **网络诊断按钮切换组件样式未完全复刻游戏环境样式 BUG**：统一基准修正为「环境监测-游戏环境」卡片——网络配置接口切换与网络工具选择器均改为同款 Tabs 样式（透明容器 + h-7 圆角按钮 + 激活橙色高亮 data-[state=active]:bg-orange-500/20），三处按钮切换组件 className 完全一致。

## [v1.10.2] - 2026-08-12

### 变更

- **网络诊断按钮切换组件样式统一**：网络配置卡片的接口切换从 Tabs 胶囊滑块风格改为与网络工具卡片同款 ToggleGroup outline 样式（透明容器 + 独立边框按钮 + 等宽全宽排布 + accent 激活高亮），两卡片按钮切换组件视觉完全一致。

## [v1.10.1] - 2026-08-12

### 变更

- **系统设置页卡片恢复固定布局**：移除卡片自由拉伸功能（ResizableCard），系统优化+电源计划合并卡与服务管理卡恢复为普通固定卡片（不可拖拽调整宽高），与 v1.9.0 布局一致；同步删除 ResizableCard 组件。

## [v1.10.0] - 2026-08-12

### 新增

- **系统设置页卡片自由拉伸**：系统设置页所有卡片（系统优化+电源计划合并卡、服务管理卡）均可自由拖拽调整宽高——悬停卡片边缘浮现拖拽条（左/右缘调宽度、下缘调高度），右下角斜角三角可同时拖拽宽高，双击三角恢复自适应；尺寸持久化到 localStorage，刷新/重开保持；卡片被压缩至低于内容高度时内部滚动兜底，内容不溢出。

## [v1.9.0] - 2026-08-12

### 新增

- **侧边栏悬浮卡片化**：侧边栏导航从贴边布局改为 `floating` 变体独立悬浮卡片（圆角 + 边框 + 投影，距窗口边缘 12px 悬浮），折叠态自动变为悬浮图标栏；悬浮间距、圆角与阴影随折叠状态平滑过渡。

### 变更

- **卡片阴影全面升级（漂浮立体感）**：新增 `.shadow-card` / `.shadow-card-hover` 双层投影工具类（亮色柔和投影 / 暗色加重投影），所有 shadcn Card 容器默认阴影由 `shadow-sm` 升级为 `shadow-card`，hover 时加深为 `shadow-card-hover` 并带 300ms 过渡，营造整体漂浮立体视觉。

## [v1.8.2] - 2026-08-12

### 修复

- **DNS 可编辑输入框只能输入 1 个字符 BUG**：DnsSelect 输入框聚焦/输入时打开 Popover，Radix 默认把焦点自动转移到下拉列表内第一个可聚焦元素（「本机 DNS」按钮），输入框失焦导致后续字符丢失。修复：Popover 设为非 modal 并阻止 onOpenAutoFocus / onCloseAutoFocus 自动转移焦点，输入框保持焦点可连续输入。

## [v1.8.1] - 2026-08-12

### 修复

- **路由跟踪 DNS 查询与下拉框优化**：
  - 「DNS 查询」改为可编辑下拉（Input + 预设列表），自定义 DNS 与下拉共用同一输入框，直接输入 IP 即生效，不再有独立的「自定义 DNS」输入框与状态分离问题（合并 traceDnsMode / traceDnsCustom 为单一状态）。
  - DNS 查询调优：单次查询超时 5s→2s、不重试、不走 TCP 回退，DNS 服务器不可达时不再卡顿数秒（实测不可达地址 0.01s 快速返回）；查询失败时错误信息保留底层原因（原静默丢弃）。

## [v1.8.0] - 2026-08-12

### 修复

- **路由跟踪（Traceroute）无效 BUG**：根因为 surge_ping 的回复匹配 key 使用目标 IP，中间路由器返回的 ICMP Time Exceeded 报文源地址是路由器 IP，导致中间跳 100% 丢失（全 `* * *`）。改为自研 ICMP 实现（socket2 + tokio 非阻塞收包）：构造 ICMP Echo 报文（v4/v6 含伪头校验和）、逐跳设置 TTL、同时匹配 Echo Reply 与 Time Exceeded（从内嵌报文提取 identifier/sequence），中间跳正常显示（实测 网关→运营商→出口节点 链路完整捕获）。Windows 上 DGRAM+ICMP socket 不可用时自动回退 RAW socket。

### 新增

- **路由跟踪 DNS 查询配置**：参数区新增「DNS 查询」下拉（本机 DNS / 预设服务器：腾讯 DNSPod 119.29.29.29、阿里 AliDNS 223.5.5.5、百度 180.76.76.76、114 DNS、Google 8.8.8.8、Cloudflare 1.1.1.1 / 自定义 DNS 可输入 IP），解析目标域名时使用所选 DNS 服务器（trust-dns-resolver 指定服务器查询，A/AAAA 按网络栈选择）。

## [v1.7.2] - 2026-08-12

### 变更

- **网络工具共用输入区紧凑化**：目标地址 / 端口 / 网络栈 / 协议四项压缩为同行排列（端口 w-24→w-20、网络栈与协议按钮 padding px-2.5→px-2、目标地址 min-w 180→100px、间距 gap-3→gap-2.5），1366px 及以上屏幕四项同行不再换行；窄屏仍由 flex-wrap 兜底防溢出。

## [v1.7.1] - 2026-08-12

### 变更

- **网络工具卡片布局优化**：
  - 「协议」与「网络栈」组合为不可拆分单元，始终同行相邻排列，避免窄列宽度下 flex-wrap 把协议挤到下一行。
  - 统一「开始/停止」按钮上移至卡片标题栏右侧（所有工具共用），共用输入区仅保留 目标地址 / 端口 / 网络栈+协议，行宽更紧凑。

## [v1.7.0] - 2026-08-12

### 修复

- 网络工具卡片工具选择器按钮组换行排版错乱，改为标题/按钮两行布局、按钮等宽整齐排列。

### 新增

- 目标地址右侧公共端口输入框，Port 检测改用公共端口。

### 变更

- **环境检测 — AI 环境聚焦 AI Agent 工具**：
  - 检测项由「AI CLI 工具」改为「AI Agent 工具」，检测范围聚焦 AI agent 类工具（Claude Code / Codex CLI / Gemini CLI / Grok Build / Hermes / OpenCode / OpenClaw / PI Agent）。
  - 移除不属于 agent 的工具：Ollama（本地模型运行时）与 CC Switch（Claude Code 配置切换器），前端同步移除 Ollama「官网下载」按钮分支。

## [v1.5.1] - 2026-08-10

### 变更

- **侧边栏 IP 信息标签简化**：国内出口两行（国内 IPv4 / 国内 IPv6）标签统一为「公网」，去掉「IPv4 国内 / IPv6 国内」后缀，仅保留地址与归属地，与国外出口行同口径。

## [v1.5.0] - 2026-08-10

### 新增

- **侧边栏 IP 信息四行展示（国内 IPv4 / IPv6 双通道）**：
  - 修复 `myip.ipip.net` 双栈 DNS（同时解析 AAAA + A 记录）下系统 IPv6 优先导致「国内」行实际拿到 IPv6 地址的问题（`extract_ipv4` 提取不到、显示异常）。
  - `secm-datasource` 新增 `http_get_family`（ureq 自定义 resolver 强制 IPv4 / IPv6 连接，仅过滤 DNS 解析结果，URL 白名单约束不变）；`ip_info` 国内出口改为一次请求同时提取地址+归属地，新增 `domestic_ipv6` / `domestic_ipv6_location` 字段（无 IPv6 连通性时占位「—」，前端回退 ipify）。
  - 侧边栏网络信息卡片改为四行：本地 IPv4 / 公网 IPv4（国外出口） / 公网 IPv4 国内 / 公网 IPv6 国内（优先 ipip.net 强制 IPv6 出口，回退 ipify）。

## [v1.4.5] - 2026-08-10

### 变更

- **第三方依赖与许可严格审计更新**：
  - 新增审计脚本 `scripts/audit/licenses-npm.mjs` / `licenses-cargo.mjs`（从 package-lock.json 与 Cargo.lock + 本地 registry 缓存读取许可，支持 --top/--copyleft 过滤），供后续发版复用。
  - 应用内「关于 → 第三方许可」页补全：新增「开发与构建工具（devDependencies）」分组（Vite/TypeScript/ESLint 全家/Prettier/Husky/lint-staged）；Rust 依赖补 log、socket2、thiserror、ureq、wmi、windows-sys（0.45~0.61）；传递依赖 copyleft 声明补 lightningcss（MPL-2.0）、caniuse-lite（CC-BY-4.0）、Unlicense/CC0 公有领域系列、r-efi（多许可择一说明）；运行时补 .NET 8 Runtime（MIT）、HidSharp（Apache-2.0）、DiskInfoToolkit/RAMSPDToolkit-NDD（MPL-2.0）。
  - `THIRD-PARTY-NOTICES-LHM.txt` 补全：LibreHardwareMonitorLib（MPL-2.0 指引）、HidSharp（Apache-2.0 全文）、DiskInfoToolkit/RAMSPDToolkit-NDD（MPL-2.0）、Mono.Posix.NETStandard（MIT）、.NET 8 Runtime（MIT），并同步至发行资源（stage:lhm）。
  - README 引用声明表补 LHM/PawnIO/HidSharp/.NET Runtime，许可清单计数更新为 6 组 50+ 项。

## [v1.4.4] - 2026-08-10

### 修复

- **网络诊断页布局错位**：grid 两列布局下页头（PageHeader）作为首个 grid 子项占用左上角，把网络配置卡挤到右上角（表现：卡片集中在右上/左上）。修复：页头加 `col-span-full` 横跨整行，恢复「左列网络配置 + 右列 DHCP/网站测试/网络工具三行」布局。

## [v1.4.3] - 2026-08-10

### 变更

- **网络诊断页布局调整**：网络配置卡片独占左列（整列高度），右列自上而下依次为 DHCP 服务器检测 / 网站测试 / 网络工具三张卡片（`grid-cols-1 lg:grid-cols-2` + 右列 `flex-col`，窄窗口回退单列）。

## [v1.4.2] - 2026-08-10

### 变更

- **网络诊断页布局改为 2x2 两列网格**：网络配置 / 网站测试 / DHCP 服务器检测 / 网络工具四张卡片由纵向堆叠改为两列排版（`grid grid-cols-1 lg:grid-cols-2`，窄窗口自动回退单列），stagger 动画序号同步调整。

## [v1.4.1] - 2026-08-10

### 变更

- **网络配置卡片去除多余操作，直接可编辑**：
  - 移除「编辑配置 / 取消编辑 / 加载 / 刷新」按钮，进入页面自动加载并直接处于可编辑状态（表单即视图）。
  - 多接口 Tabs 内直接渲染编辑器；删除只读展示组件（AdapterConfigView/ConfigRow）。
  - 应用成功后延迟 1.5s 刷新，先展示结果提示再重新读取生效后的配置。

## [v1.4.0] - 2026-08-09

### 新增

- **网络配置卡片升级为可编辑（静态 IP / DHCP 切换 / DNS / IPv6 / MAC 修改）**：
  - 新增 `net_config.rs` 模块：通过 netsh 应用 IPv4 地址/子网掩码/默认网关（DHCP ⇄ 静态切换）、IPv4 DNS（主/附加）、IPv6 地址/默认网关（add/set 双路径 + 默认路由替换）、IPv6 DNS；管理员权限（manifest requireAdministrator）+ `is_admin()` 双重校验，R8 非管理员给出明确提示。
  - 编码处理：netsh 输出 ANSI（中文系统 GBK）经 encoding_rs 解码；安全：参数全走 `Command::args` 无 shell 注入面，netsh `name="..."` 引号显式构造并剔除内嵌引号。
  - 新增 MAC 修改：注册表 `NetworkAddress` 覆盖（NetCfgInstanceId == 接口 GUID 精确匹配网卡实例，回退 DriverDesc）+ 重启网卡生效，应用前备份原值并提示恢复方式；`AdapterConfig` 新增 `guid` 字段（GetAdaptersAddresses AdapterName）。
  - 新增命令 `apply_network_config`（分步结果 steps：单项失败不影响其他项）与 `set_network_mac`；单测：参数构造/引号转义/IPv4+IPv6 校验/MAC 格式化/GBK 解码/空接口名与未知模式分支。
  - 前端：新增 `NetworkConfigEditor` 组件（DHCP/静态模式切换、IPv4/IPv6/DNS 输入、多 DNS 逗号分隔、应用前 AlertDialog 确认并展示完整配置预览、分步结果展示、成功后自动刷新）；网络配置卡片增加「编辑配置」入口与多接口 Tabs 受控切换。

## [v1.3.0] - 2026-08-09

### 新增

- **网络诊断页新增「网络配置」卡片（ipconfig /all 语义）**：
  - 纯 Rust 驱动采集（secm-datasource::netif::adapter_configs，GetAdaptersAddresses + GAA_FLAG_INCLUDE_GATEWAYS），替代 PowerShell/ipconfig 外部命令。
  - 展示字段：物理地址（MAC）、IPv4 地址、IPv4 子网掩码（前缀长度换算）、IPv4 默认网关、IPv4 DNS 服务器、连接-本地 IPv6 地址（fe80::，带 %zone）、IPv6 默认网关、IPv6 DNS 服务器。
  - 多网卡 Tabs 切换，按接口状态标记已连接/未连接；仅展示具备实际配置的接口；单测：实机适配器形状断言 + 掩码换算/链路本地判定纯函数。
  - 新增命令 `get_network_config`。

- **网络诊断页新增「DHCP 深度检查」**：
  - 在 DHCP 服务器检测基础上，对每台检测到的服务器执行 ICMP ping 可达性探测（JoinSet 并发，2s 超时），并做网段/网关拓扑判定：同网段 vs 跨网段、是否等于默认网关。
  - 判定矩阵输出中文诊断与严重级别（ok/warn/critical）：DHCP 服务器即默认网关且可达 = 正常；同网段非网关 = 多 DHCP 冲突风险；跨网段 = OFFER 经 DHCP 中继（ip helper）或旁路由转发（典型场景：检测到 192.168.11.1 而默认网关 192.168.3.254，本机 ping 不通属预期，摘要给出排查建议）。
  - 新增命令 `deep_check_dhcp_servers`；纯函数单测覆盖用户典型跨网段场景与全部判定分支。

## [v1.2.0] - 2026-08-09

### 新增

- **网络诊断页新增「本地 DHCP 服务器检测」**：
  - 主动探测（构造 DHCPDISCOVER，SO_REUSEADDR 绑定 68 接收 DHCPOFFER，收集去重源 IP）+ 基线合并（注册表仅取 EnableDHCP=1 接口的 DhcpServer，静态接口/历史租约不污染）→ 服务器数判定。
  - 1 台 = 健康（绿色）；多台 = 友好警告（黄色卡片 + 建议清单：检查多路由器级联/软路由/热点等 DHCP 冲突源）。
  - 真机验证：检测到本机 DHCP 分配记录 1 台（healthy）；多源场景（网关 192.168.3.254 + 历史租约）在修正基线前实测 count=2 触发警告，逻辑正确。
  - 新增 `src-tauri/src/dhcp_probe.rs`（827 行 + 单测：报文构造/响应解析/多源去重/超时路径）+ 命令 `check_dhcp_servers`；socket2 依赖；降级链：主动探测无响应时基线合并兜底。

## [v1.1.1] - 2026-08-09

### 修复

- **清理缓存-系统清理-基础清理三项按钮溢出容器**：三列网格（sm:grid-cols-3）下「刷新 DNS 缓存」「清理临时文件」等长文本按钮固定宽超出列宽导致溢出卡片边界。修复：操作项容器允许换行（flex-wrap），按钮右对齐（ml-auto）——窄列时按钮自动换行至第二行右侧，不再溢出；宽屏下布局不变。

## [v1.1.0] - 2026-08-09

### 新增

- **PawnIO 2.2.0 驱动随包分发与 SCM 服务自动部署**：
  - 随包分发 `resources/driver/` 下 WHQL 签名驱动三件套（`PawnIO.sys` 哈希 `fca6e7d58b0cf38dbb913a2b9e532f48629145d395f454b16a9f58e97b8d3940` / `PawnIO.cat` / `pawnio.inf`）与 GPL-2.0 源码包 `PawnIO-src/`（namazso/PawnIO tag 2.2.0，含 COPYING）。
  - 新增 `driver_install::pawnio` 部署模块：L0 检测已装载驱动直接复用（幂等，30s 缓存）；L1 签名校验门禁（WHQL 签名验证失败不建服务）+ 创建 PawnIO 服务 + 启动；L2 错误分类（驱动已存在/服务已存在/拒绝启动等），失败分类可区分。
  - 应用启动与 LHM sidecar 启动前调用 `ensure_pawnio_ready` 确保驱动就绪，与既有自愈链路（lib.rs）并列，互不阻塞。
  - **LHM 链路适配**：sidecar 启动前确保 PawnIO 内核驱动就绪（lhm.rs `should_ensure_pawnio` 降级判定），LibreHardwareMonitor 温度主链路的 ring0 通道不再依赖系统预装驱动。

### 变更

- **许可合规声明修正**（About 页 PawnIO 条目）：license 改为 GPL-2.0-or-later 附例外条款（namazso/PawnIO 官方授权），URL 改为 `https://github.com/namazso/PawnIO`，注明随包分发 v2.2.0（`resources/driver/PawnIO.sys` + `PawnIO-src/` 源码 + `resources/lhm/LICENSE-GPL-2.0-PawnIO.txt` 全文/例外）。
- sidecar `ProbePawnIo` 诊断文案更新：提示驱动已随包部署（无需用户手工安装 PawnIO）。

### 修复

- 无（本版本为驱动随包分发与许可合规增强，未引入缺陷修复）。

## [v1.0.0] - 2026-08-09

### 重大变更（架构重构）

- **温度/功耗数据源切换为 LibreHardwareMonitor（LHM）sidecar**：
  - 新增 `sidecar-lhm/`（.NET 8 独立进程，LibreHardwareMonitorLib 0.9.6，MPL-2.0）：PawnIO 内核驱动（WHQL 签名，LGPL-2.1）读取 CPU 封装温度/功耗，HTTP JSON 输出（Loopback 端口 45980），进程隔离规避 GPL/MPL 传染，许可文本与源码随包分发。
  - Rust 侧新增 `src-tauri/src/lhm.rs`（进程生命周期/健康检查/轮询/降级），温度链改为 `lhm → hwmon → winring0 → acpi → none`，`temp_source` 新增 `"lhm"`。
  - **解决火绒拦截问题**：PawnIO 2.2.0 已 WHQL 签名且火绒不拦（真机验证：LHM 通道读取 73.1°C 成功，WinRing0 同机 ERROR 5 被拦）。
  - 安全加固：SECM_LHM_DIR 注入仅 debug 生效（M1）、health 响应体校验防端口误占（M2）、About 页补 LHM/PawnIO 许可条目（M5）。
  - 真机验证：sidecar 独立返回 `{available:true, package_temp_c:73.1, power_w:41.8}`；release 集成链路与降级路径就绪。
  - 遗留风险：PawnIO 2.2.0（已装）与 LHM 内嵌 0.1.6 的 LoadBinary 兼容性需 7800X3D 复测；提权 sidecar 崩溃后仅降级不自动重启。

## [v0.25.1] - 2026-08-09

### 修复

- 火绒等安全软件驱动防护拦截 WinRing0 驱动加载时，驱动引导卡提供精确放行步骤：火绒 → 防护中心 → 信任区添加「SECM 安装/运行目录」与「WinRing0x64.sys」，或驱动防护中放行，完成后一键重试，必要时重启电脑
- WinRing0 服务启动失败错误码专项处理：0x430（ERROR_SERVICE_MARKED_FOR_DELETE 服务标记删除）与 0x431（ERROR_CANNOT_DETECT_DRIVER_FAILURE 内核拒载，疑似杀软拦截）分类为 MarkedForDelete，文案分别表述并明确「重启电脑」指引
- WinRing0 启动遇 0x430/0x431 时追加一次 2 秒重试（等待 SCM 删除异步完成 / 内核状态清理），不无限重试
- 启动自愈汇总日志在通道不可用且检测到运行中安全软件时，追加显式拦截指引（火绒放行 + 重启提示）

## [v0.25.0] - 2026-08-09

### 新增

- 前端现代化布局整合：全部 8 页统一页面间距 / 卡片网格 / 卡片头结构，新增共享组件 PageHeader / CardHeaderWithIcon / SectionTitle / EmptyState / StatusDot
- 系统设置「系统优化开关」子分组（游戏类 5 项 / 系统类 1 项，带小标题与分隔线）

### 修复

- 游戏模式开关默认态误判（后端缺失时误显示开启，??true → ??false）
- About 页第三方锁定清单文案重复（Cargo.lock 出现两次）
- 调试日志未知级别导致页面崩溃（LEVEL_CONFIG 可选链防御）
- 网络诊断卡片入场动画倒序、DiagOutput 双分隔线、自动检测 Select 截断（移除 scale-75）
- 清理进程表无匹配结果时无空态提示

### 变更

- 视觉规范全站统一：深色主题色彩层次（bg-muted/30、hover:bg-muted/50）、图标尺寸（size-4/size-3.5）、数字字体（tabular-nums）、最小业务字号 11px、圆角层级
- 硬编码实色清理：bg-green-500 实色成功态（8 处）、amber-500 黄色域、border-neutral-700、bg-sky-500 实色按钮
- Dashboard 摘要/底部网格断点与间距统一（lg:grid-cols-3 gap-4）、存储卡组件化复用 SummaryCard 结构、图表渐变改用主题 CSS 变量
- DiskSmart 健康状态黄色域统一（amber → yellow）
- 空态 / 加载骨架 / 错误提示跨页统一（EmptyState、Skeleton rounded-md）

## [v0.24.2] - 2026-08-09

### 变更

- **电源计划卡片标题精简**：「核心线程调度策略（AMD/Intel 都受益）」→「核心策略（Amd/Intel 适用）」；「NVIDIA 显卡电源管理模式」→「NVIDIA 电源模式」。

## [v0.24.1] - 2026-08-09

### 变更

- **系统优化开关标题精简**：「GPU 硬件加速调度 (HAGS)」→「GPU 硬件加速调度」；「可变刷新率 (VRR / G-Sync / FreeSync)」→「可变刷新率」。

## [v0.24.0] - 2026-08-09

### 新增

- **系统优化开关新增「关闭高精度计时器」开关**：
  - 关闭：`bcdedit /set useplatformclock no` + `useplatformtick no` + `disabledynamictick yes`；恢复默认：三条 `/deletevalue`。
  - 新增 `src-tauri/src/hpt.rs`：bcdedit 执行（CREATE_NO_WINDOW，错误含命令/退出码/输出摘要，R8）+ 状态解析（兼容中英文词表 是/否、Yes/No、true/false；判定=三条齐全且 useplatformclock no + useplatformtick no + disabledynamictick yes）。
  - 新增命令 `get_hpt_state` / `set_hpt_state`；前端开关带「管理员」标识，描述提示需重启生效。
  - 真机验证（探针复用模块）：关闭→重读（BCD 持久化）→恢复→重读闭环一致，最终恢复默认不残留。

## [v0.23.5] - 2026-08-09

### 变更

- **系统设置第一行：系统优化开关与电源计划合并为一张卡片（内部两列）**：
  - 合并卡占原两列宽（`xl:col-span-2`），内部左右两列：左=系统优化开关（5 个开关），右=电源计划（计划列表 + 核心线程调度策略 + NVIDIA 电源管理模式），窄屏自动上下堆叠（顶部边框分隔）。
  - 服务管理保持独立卡片不变。

## [v0.23.4] - 2026-08-09

### 变更

- **电源计划卡片标题文案与样式统一**：
  - 「大小核线程调度策略（需混合架构 CPU）」改为「核心线程调度策略（AMD/Intel 都受益）」。
  - 标题块样式与 NVIDIA 显卡电源管理模式统一：图标容器（`p-1.5 rounded-md bg-*`）+ 图标 `w-5 h-5` + 标题 `text-sm font-semibold`（原为无容器小图标 + `text-xs font-medium`）。

## [v0.23.3] - 2026-08-09

### 修复

- **NVIDIA 电源管理模式值域错误（控制面板恒显示「自适应」根因）**：
  - 根因：`Optimal Power` 的 DRS 值为 **5** 而非 2（权威证据：NVIDIA App `featureEnum`：`NVCPLAPI_VALUE_POWER_MANAGEMENT_MODE_OPTIMAL_POWER=5`、`MODE_MAX=1`、`MODE_ADAPTIVE=0`）。旧版写 2 为无效值，控制面板回退显示默认「自适应」，表现为修改无效。
  - 修复：值域改为 Adaptive=0 / MaxPerformance=1 / Optimal=5；`from_raw` 拒绝旧版误写的 2（读到时提示重新选择）。
  - 真机验证（RTX 2080 Ti）：三档（0/1/5）写读闭环一致，多轮循环稳定。

### 变更

- **电源计划卡片移除刷新按钮**（卡片级标题已足够，按钮冗余）。
- **操作记录卡片移除，改为标题右侧渐入渐出 Toast**：系统设置页标题最右侧显示操作反馈（成功绿色/失败红色），2 秒后自动渐出消失；流式消息自动替换上一条。

## [v0.23.2] - 2026-08-09

### 修复

- **NVIDIA 显卡电源管理模式修改不生效（真机诊断修复）**：
  - 根因：写入时未先 GetSetting 取完整结构（settingName/location 等字段为空），驱动应用逻辑不完整。
  - 修复：`set_power_mode` 改为标准流程——先 GetSetting 完整结构 → 改值 → 标记 `isCurrentPredefined=0`（用户自定义值）→ SetSetting → SaveSettings → **写入后立即重读验证持久化**，不一致则报错。
  - 真机验证（RTX 2080 Ti / 595.79）：Optimal/Adaptive/MaxPerformance 三档写读闭环一致，DRS 数据库（nvdrsdb0.bin）时间戳确认持久化。
  - 命令日志增强：`set_nvidia_power_mode` 记录「旧值 → 新值」对比与耗时（R8）；失败时记录 Error 级日志含错误码。
  - 前端切换后立即重读真实值同步 UI（防驱动拒绝导致界面与实况不符），失败时回滚显示并提示原因。

### 变更

- **电源计划卡片移除内部重复图标与标题**：独立卡片化后区块内不再显示副 Zap 图标与「电源计划」标题（卡片头已含），仅保留刷新按钮（右对齐）。

## [v0.23.1] - 2026-08-09

### 变更

- **系统设置页第一行改为三列布局**：原「系统优化（含电源计划）+ 服务管理」两列拆为三张卡片三列并排：
  - 左列「系统优化开关」：仅保留 5 个硬件/游戏开关（HAGS / 游戏模式 / 窗口化游戏优化 / 鼠标精准度 / VRR）。
  - 中列「电源计划」（新独立卡片）：电源计划列表 + 删除确认 + 大小核线程调度策略（异类调度）+ NVIDIA 显卡电源管理模式。
  - 右列「服务管理」：不变。

## [v0.23.0] - 2026-08-09

### 新增

- **系统设置-电源计划区域新增「NVIDIA 显卡电源管理模式」下拉框**：
  - 新增 `src-tauri/src/nvidia_drs.rs`（NVAPI DRS 模块）：加载 `nvapi64.dll` → `nvapi_QueryInterface(ordinal)` 解析 DRS 接口，按设置名 `Power management mode` 动态解析 settingId 后读写 NVIDIA 控制面板「管理 3D 设置 → 电源管理模式」（DRS 全局 profile，无注册表路径）。
  - 值域：0=最佳功率（Optimal Power）/ 1=最高性能优先（Prefer Maximum Performance）/ 2=自适应（Adaptive），与 NVIDIA 控制面板同源。
  - 序数与结构均来自 NVIDIA 官方 SDK 提取的 nvapi-sys 序数表 + 本机真机实测（RTX 2080 Ti / 驱动 595.79）：读/写/保存/重读闭环验证通过，写回当前值不改动实际设置。
  - 前端系统设置页电源计划下方新增下拉框（无 NVIDIA 显卡时显示错误原因、隐藏下拉框）；切换后写 DRS 并保存，实时生效。
  - 新增命令：`get_nvidia_power_mode` / `set_nvidia_power_mode`；单测：结构大小（12320 字节）与值域映射断言。

## [v0.22.0] - 2026-08-09

### 新增

- **环境检测-游戏环境推荐设置与系统设置页开关对齐联动**：
  - Rust 侧 `GameSetting` 新增 `key` 字段标识设置项（hags / game_mode / vrr / mouse_precision / power_plan，纯检测项为空串），`gs()` 按 label 自动推断。
  - 前端环境检测页可联动项（硬件加速 GPU 调度 / 游戏模式 / VRR / 鼠标精准度）渲染为 Switch 开关，切换时调用与系统设置页**同一 setter 命令**（`set_hags_state` / `set_game_mode_state` / `set_vrr_state` / `set_mouse_precision`），成功后实时重读当前值刷新检测结果，失败行内提示并回滚展示。
  - 修正「游戏模式」检测源：由 `get_game_optimization_state`（窗口化游戏优化）改为 `get_game_mode_state`（系统设置页「游戏模式」开关同源），消除两页语义不一致。
  - 联动同源机制：两页均读写同一注册表/API，环境检测页切换后系统设置页开关状态即同步，反之亦然。

## [v0.21.8] - 2026-08-09

### 变更

- **硬件信息页布局：存储卡片移至第二行中间**：
  - 存储卡片从第一行「实时摘要」（原 CPU/GPU/内存/存储 4 列）移至第二行，位于「硬件使用趋势」与「网络流量」之间（3 列并排：趋势 → 存储 → 网络流量）。
  - 第一行相应改为 3 列（CPU/GPU/内存）；卡片入场动画统一为 stagger-5（与趋势/网络一致）。

## [v0.21.7] - 2026-08-09

### 修复

- **CPU 频率 ntapi 层"非零即成功"缺陷导致 AMD 平台恒显示标称基频**：`CallNtPowerInformation` 在 AMD 平台（如 Ryzen 7 7800X3D）恒返回标称基础频率（4201MHz）不随负载变化，v0.21.6 起 FREQ 卡死在标称值。修复：新增"疑似标称"判定 `is_suspected_nominal`（平均当前频率 ≥ 平均最大频率 × 0.98 即判定为非实时标称值），不再直接采用，继续走 PDH 实时估算层（任务管理器同源）；PDH 不可用时回退 ntapi 标称值（diag 标注 `ntapi,标称`），不劣化现状（`datasource/src/cpu_freq.rs`、`src-tauri/src/sensor.rs` 主链路、`src-tauri/src/hardware.rs` per-core 链路同步接线）。
- **`ProcessorPowerInfo.max_mhz` 字段此前从未用于校验**：判定函数利用平均 `max_mhz` 与当前频率比对识别"疑似标称"，两处 ntapi 使用点均接入（sensor.rs 主链路 + hardware.rs per-core）。

### 新增

- **`is_suspected_nominal` 纯函数单测**：覆盖 current==max → 疑似、current<max×0.98 → 非疑似（Intel 真实波动值不受影响）、边界值、max<=0 / current<=0 防御分支（`datasource/src/cpu_freq.rs`）。

### 验证

- 真机验证（7800X3D）：修复前 FREQ 恒 4201(ntapi)；修复后 FREQ 随负载波动（空闲约 4538-4600 MHz vs 负载约 4842-4846 MHz，source=pdh），满足修复判定标准。

## [v0.21.6] - 2026-08-09

### 修复

- **CPU 实时频率 PDH 估算在 1s 默认轮询下采样间隔不足导致长期不可用**：PDH 估算依赖相邻两次 `PdhCollectQueryData` 之间的时间差，默认 1s 轮询间隔与 PDH 最小采样间隔（约 1s）接近，时钟抖动导致间隔不足（0/负值）被反复判为无效、频率长期不可用。修复：首调 warmup——进程内第一次读取时补齐最小采样间隔（仅首次阻塞 ≤1.05s），此后不再阻塞，PDH 可用时首调即返回有效估算值（`datasource/src/cpu_freq.rs`）。
- **`PdhCollectQueryData` 返回值未检查**：此前对 PDH API 返回码未作校验，失败时静默吞掉（违反 R8 日志完整）。修复：显式检查返回值并透出 API 名 + 错误码（`PdhCollectQueryData` → Win32 错误码），供诊断链定位。

### 新增

- **`get_cpu_freq_diag()` 透出降级链各层失败原因**：返回 `(频率, 数据源, diag 字符串)`，diag 记录四层降级链（ntapi→pdh→registry→sysinfo）各层失败原因（API 名 + 错误码）；`freq_source=none` 时后端日志写 `FREQ=0(none) 原因:...`，替代原 `FREQ=0`（`sensor.rs` CPU 链路同步接入）。
- **Dashboard 频率全失败时显示一次性诊断摘要卡（FrequencyDiagCard）**：应用版本 + 温度/功耗/频率数据源 + diag 摘要，一键复制上报，便于用户环境取证（`src/pages/Dashboard.tsx`）。

### 文档

- 新增 `docs/CPU_FREQ_USER_GUIDE.md`（用户取证指引：降级链说明 + 诊断摘要卡复制上报步骤）。
- 新增 `docs/designs/2026-08-09-cpu-freq-diag-enhance.md`（诊断增强设计文档）。

## [v0.21.5] - 2026-08-09

### 修复

- **CPU 实时频率在杀软拦截场景仍显示为空/0/无刷新（v0.21.4 降级链未真正生效）**：
  - 根因一：v0.21.4 的 sysinfo 回退与主路径共用 `CallNtPowerInformation`（失败点相同，回退无效）——主路径返回 0 时 sysinfo 回退必然同样返回 0。
  - 根因二：sysinfo 0.30.13 频率首次获取后缓存永不刷新（`CpusWrapper::get_frequencies` 被 `got_cpu_frequency` 短路），即使回退成功前端也「无刷新」。
  - 修复：频率读取改为**四层用户态降级链** `CallNtPowerInformation（实时）→ PDH % Processor Performance × 标称频率（实时估算，任务管理器同源）→ 注册表 ~MHz（标称保底）→ sysinfo（兜底）`，全部不依赖内核驱动，杀软拦截场景下链路完整（新增 `datasource/src/cpu_freq.rs` 模块，含 PDH 单例采样与 `estimate_freq` 纯逻辑）。
  - 契约：`CpuData` 新增 `freq_source` 数据源标记字段（`"ntapi" | "pdh" | "registry" | "sysinfo" | "none"`），`hardware.rs` CPU 频率链路同步接入降级（不再因 `CallNtPowerInformation` 失败而全 `None`）；前端 Dashboard 显示「估算」「标称」角标区分实时/估算值（旧版后端缺失字段时兼容）。
  - 健壮性：频率缓冲区改为按 `GetSystemInfo().dwNumberOfProcessors` 权威逻辑处理器数分配（修复 >64 核/平台差异下缓冲区不匹配）。

## [v0.21.4] - 2026-08-08

### 修复

- **开启杀毒软件（Windows Defender / 火绒 / 360 等）时无法获取 CPU 温度、频率和功耗**：
  - 根因：杀软驱动防护拦截 WinRing0/hwmon 内核驱动服务启动（ERROR_ACCESS_DENIED 5），叠加 hwmon 测试签名在 Secure Boot 下加载失败（577），温度/功耗降级链全部断裂，前端显示空白。
  - 诊断：错误分类新增 `InstallError::BlockedBySecurity`（StartServiceW 返回 5 且驱动签名有效时判定为安全软件拦截）；新增常见安全软件检测（Defender/火绒/360/腾讯/卡巴/诺顿/ESET/联想管家，进程名 + 服务名 + 驱动阻止列表/HVCI 状态，只读零特权）。
  - 健壮性：驱动服务启动失败退避重试 2 次（500ms/1500ms），处理杀软延迟放行与签名校验瞬时失败；`DriverStatus` 新增 `security` 字段透传检测结果。
  - 降级：WinRing0 RAPL 不可用时按 CPU 标称 TDP × 使用率估算功耗（型号表匹配，失败默认 65W 并标注未知）；`CallNtPowerInformation` 频率失败时用 sysinfo 二次回退；ACPI 热区枚举全部实例取最大值（多热区主板）。
  - 引导：Dashboard 新增驱动降级引导卡（降级原因链 + 检测到的杀软 + 分杀软配置步骤 + 一键重试/重新检测）；功耗估算值显示「估算」角标、ACPI 温度显示数据源角标；`CpuData` 新增 `temp_source`/`power_source`/`power_estimated` 数据源标记契约。
  - 用户指南：新增 `docs/AV_BLOCK_USER_GUIDE.md`（分杀软配置步骤 + 回滚说明）。

## [v0.21.3] - 2026-08-08

### 变更

- **硬件信息页存储卡片：移除幻灯片子容器，全部硬盘直接在主卡片同期显示**：
  - 删除 Carousel 轮播嵌套，每块硬盘直接排列在主卡片内（分隔线区分），每盘两行：行 1 = 卷标 + 总容量 + 读/写速度（MB/s，PDH 按卷独立采样）；行 2 = 容量使用率进度条 + 已用 % + 可用空间。
  - 所有硬盘的读写与存储信息同时可见，无需翻页。

## [v0.21.2] - 2026-08-08

### 修复

- **SMART 读数严格对齐 CrystalDiskInfo（全面核对）**：
  - **原始值列改为 16 进制 12 位补零**（CDI 默认显示格式）：如 0x09 通电时间 60897 → `00000000EDE1`、E9 raw=0 → `000000000000`（此前十进制千分位，完全对不上 CDI）。
  - **临界值（阈值）列直接显示数值 0**（CDI 显示 0；此前 0 显示为 "—"）。
  - **通电时间改为直接显示小时**（CDI 一致）：此前换算为「X 天 X 小时」，现显示「60897 小时」。
  - **0xAF 恢复「未知属性」**：Intel 后期固件 0xAF 为废弃字段（value 恒 100），CDI 不显示其磨损含义；此前与 0xE9 同名「介质磨损指示器」造成两行同名干扰，现仅 0xE9 为介质磨损指示器（当前值 99/最差值 99/临界值 0/原始值 000000000000，与 CDI 完全一致）。
  - 验证（真机，序列号 BTWA7173012B480FGN 的盘）：序列号读取一致、E9 四列与 CDI 逐项吻合、通电时间 60897 小时；datasource 43 + tsc/vite 全过。

## [v0.21.1] - 2026-08-08

### 修复

- **硬件检测页 SMART 详情弹窗数据不更新**：
  - 根因：详情加载用 `inFlightRef` 防重入——磁盘快速切换/连点刷新时新请求被跳过，且**旧请求响应会覆盖新盘的详情**（表现为「SMART 详情完全没有改变」）。
  - 修复：改用请求序号（seq）机制——每次加载递增序号，响应返回时与当前序号比对，过期响应直接丢弃；关闭弹窗/切换磁盘时作废在途请求。
- **SMART 详情弹窗内容溢出无法滚动**：
  - 根因：`DialogContent` 默认类为 `grid`，弹窗传入 `flex flex-col` 与之冲突（两个 display 类并存，CSS 顺序不可控），导致 `flex-1 min-h-0` 失效、主体区高度不受约束 → 内容超出弹窗边界。
  - 修复：改为三行 grid 布局（`grid-rows-[auto_minmax(0,1fr)_auto]`）：头部 / 可滚动主体（`overflow-y-auto min-h-0`）/ 底部按钮，长属性表在弹窗内滚动。

## [v0.21.0] - 2026-08-08

### 变更

- **硬盘 S.M.A.R.T 读数全面对齐 CrystalDiskInfo（CDI）**：
  - 属性名表重写为 CDI 同名体系：标准 ATA 属性全表（0x01-0x0D、0xB1-0xFF 共 60+ 项，如「重新分配扇区数」「通电时间」「通电周期计数」「当前待定扇区数」「UDMA CRC 错误计数」）+ Intel 专有属性（可用备用空间/备用空间阈值/寿命已用百分比/意外断电计数/介质磨损指示器 0xAF+0xE9/SATA 降速错误计数/总主机写入读取/NAND 写入等）+ 三星/西数/希捷/东芝常见属性（SSD 剩余寿命/平均擦除计数/优良块数/坏块数等）。
  - 0xAF 恢复「介质磨损指示器」（CDI 定义），0xE9 同名（Intel 后期固件，value=健康度 %）。
  - 详情弹窗属性表已具备 CDI 六列（ID/属性名/当前值/最差值/阈值/原始值）+ 状态，无需前端改动。
  - 验证（真机）：drive1 通电 60897h/通电 157 次/磨损指示 99/温度 33°C 与 CDI 一致；WD 监控盘 SMART 数据为空属盘本身特性（CDI 同样无法读取）；datasource 43 + src-tauri 65 单测全过。

## [v0.20.6] - 2026-08-08

### 修复

- **Intel 硬盘 S.M.A.R.T 读数修正（以 SSDSC2BB480G6 为例）**：
  - **属性名映射错误**：0xE9 才是 Intel 介质磨损指示器（后期固件，value=99/worst=99 = 健康度 99%），此前误命名为「未知属性」；0xAF 为早期固件废弃字段（value 恒 100、raw 无意义）却误命名为「介质磨损指示器」——已互换正名。
  - **ATA 磨损提取缺失**：0xE9 的 value 即剩余寿命健康度 %，此前未提取 → 硬盘卡片无「磨损」显示；现提取 `wear_level_percent = 100 - value`（如 value=99 → 磨损 1%），并新增磨损告警（健康度 ≤50% 预警 / ≤10% 严重）。
  - 验证（真机）：drive1 通电时间 60897h、通电 157 次、磨损指示 99/99、温度 34°C，与参考工具读数一致；datasource 43 + src-tauri 65 单测全过。

## [v0.20.5] - 2026-08-08

### 修复

- **硬件检测磁盘 S.M.A.R.T 读数/健康状态错误（三层根因）**：
  1. **IOCTL 全盘失败**：磁盘句柄仅 `GENERIC_READ` 打开，ATA 透传与 NVMe 协议查询要求读写句柄（只读打开返回 ACCESS_DENIED / INVALID_PARAMETER）→ 改为 `GENERIC_READ | GENERIC_WRITE`，SATA 属性表恢复读取。
  2. **健康状态误报「严重」**：ATA 属性 flags 的 bit1 是「在线采集位」而非失败位，此前误判导致全部属性标 FAILED、所有盘 Critical → 改为读取 READ THRESHOLDS（0xD1）阈值表后按 `value <= threshold` 判定；无阈值（Intel 企业盘阈值表全 0）时标 OK，健康由业务层按 raw 值判断。
  3. **WMI Unknown 误判预警**：`MSFT_PhysicalDisk.HealthStatus=5`（Unknown）此前升为 Warning → 修正为 Unknown 且不告警（未知 ≠ 预警）。
- **NVMe 协议查询结构修正**（微软官方布局）：协议数据偏移 = `sizeof(STORAGE_PROPERTY_QUERY)`（12B，此前 8B）、数据偏移 = `sizeof(STORAGE_PROTOCOL_DATA_DESCRIPTOR_EXT)`（72B，此前 64B）、`FixedProtocolReturnData=1`、温度字节序修正为 little-endian（NVMe 规范）。本机 Windows 存储栈无 NVMe 数据（PowerShell 验证）属环境限制。
- **读数完善**：新增 Intel 企业盘属性名（0xAA/AB/AC/AE/AF 可用备用/寿命/不安全关机/磨损指示）。
- 验证：真机 6 块 SATA 盘恢复真实读数（温度 34°C、通电时间 73727h 等）且状态 Good；Storage Space 盘正确显示 Unknown；datasource 43 + src-tauri 65 单测全过。

## [v0.20.4] - 2026-08-08

### 变更

- **存储幻灯片进度条下方新增读写速度展示行**：每张硬盘幻灯片在容量使用率进度条下一行显示「读 X.X MB/s」左、「写 X.X MB/s」右（紧凑文本行，非独立容器）。

## [v0.20.3] - 2026-08-08

### 变更

- **存储幻灯片移除「读写速度」容器**：每张硬盘幻灯片不再显示 MB/s 读/写块，仅保留卷标、总容量、已用百分比与可用空间（后端 PDH 采集保留，接口字段不变，便于后续恢复展示）。

## [v0.20.2] - 2026-08-08

### 修复

- **存储幻灯片默认只显示一张硬盘容器**：`CarouselItem` 移除 `sm:basis-1/2`（宽屏不再并排两屏），统一 `basis-full` 单张显示，左右箭头翻页。
- **读写速度数值超出「存储-硬盘-子容器」边界**：读写块由单行内联 span（数值+单位同行，窄容器时文本总宽溢出边框）改为**数值/单位分两行**布局 + `min-w-0`（防 flex 溢出）+ `truncate`（极端宽度截断），任何数值长度/容器宽度下均不越界。

## [v0.20.1] - 2026-08-08

### 修复

- **存储幻灯片打开后闪退（pdh.dll 0xc0000005 访问冲突）**：
  - 根因：`disk_io::read_counter_array_mbps` 缓冲分配错误——`PdhGetFormattedCounterArrayW` 的条目数组为**变长布局**（每个结构后紧跟实例名 UTF-16 串），所需缓冲字节数大于「条目数 × 结构大小」；此前按条目数分配，PDH 写入越界 → 进程崩溃（Windows Application Error 1000，故障模块 pdh.dll，异常 0xc0000005）。
  - 修复：按第一次查询返回的所需字节数分配（对齐容器 `PDH_FMT_COUNTERVALUE_ITEM_W` 数组），并校验第二次调用返回码。
  - 验证：压测 30 轮 × 1.1s 连续调用无崩溃，5 实例稳定出数。
- **存储卡片幻灯片布局溢出**：
  - 根因：carousel 用 `flex gap-4` 实现间距，`basis-1/2` 的百分比在 gap 之外累加 → 两屏宽度 = 100% + 16px 溢出容器。
  - 修复：改为标准 shadcn padding 方案（间距在 item 内 padding，content 以 `-ml-4` 补偿首项），`basis` 百分比含 padding 不再溢出；`CarouselItem` 默认 `basis-full pl-4`。

## [v0.20.0] - 2026-08-08

### 新增

- **硬件信息页「存储」卡片改造（幻灯片容器 + 读写左右布局）**：
  - 卡片改为轮播（Carousel）容器：**读到多少块硬盘就有多少张幻灯片**，每张显示该盘卷标、总容量、读写速度、已用百分比与可用空间。
  - 读写速度**左右布局**（左「X MB/s 读」右「X MB/s 写」）。
  - 后端升级为 **per-instance 独立采样**：PDH 通配计数器 `\PhysicalDisk(*)\Disk Read/Write Bytes/sec` + `PdhGetFormattedCounterArrayW` 按卷实例采集，与 sysinfo `mount_point()` 盘符匹配（v0.19.0 为 _Total 聚合均摊，现每盘真实独立速度，任务管理器同源）。
  - 验证：真机 `real_machine_disk_io_speed_map` 通过（5 卷全部映射 C/D/H/Y/Z，各自独立速率）；datasource 43 + src-tauri 64 单测全过。

## [v0.19.1] - 2026-08-08

### 修复

- **启动时初始化闪出命令指示符（cmd 窗口闪烁）**：
  - 根因：v0.17.5 启动自愈线程每次启动执行 `ensure_hwmon_installed` → `testsigning_enabled()` → `bcdedit /enum {current}` 子进程，该调用缺少 `CREATE_NO_WINDOW`（0x08000000）标志；GUI 进程无控制台可继承，控制台子进程启动时会弹出 cmd 窗口（每次打开应用闪一次）。
  - 修复：bcdedit 调用补上 `creation_flags(0x08000000)`（与 nvidia-smi / powercfg / sc / iperf3 等全仓其余 10 处子进程调用同款方案）。
  - 审计：全仓扫描全部 `Command::new` 调用点，确认无其他缺失该标志的启动路径子进程。

## [v0.19.0] - 2026-08-08

### 新增

- **硬件信息页「存储」卡片改为硬盘读写速度显示（P15 磁盘 IO 采集）**：
  - 后端：新增 `secm-datasource::disk_io` 模块（PDH 性能计数器 `\PhysicalDisk(_Total)\Disk Read/Write Bytes/sec`，任务管理器同源，普通用户可读、零驱动零外部进程）；`get_disk_data` 以聚合值均摊填充各盘 `read_mbps`/`write_mbps`（此前硬编码 0）。
  - 前端：存储卡片主值 = 聚合读速度（MB/s）+ 写速度 stats + 聚合容量使用率进度条 + 总可用空间；`SummaryCard` 新增 `valueSuffix` prop（默认 "%"）。
  - 验证：真机 `real_machine_disk_io_speed` 通过（空闲 0.2~1.2 MB/s 后台 IO、大文件复制时写 9.1 MB/s 量级正确）；全量单测 datasource 42 + src-tauri 64 全过。
  - 已知限制：PDH 采样首次调用为基线（首 1 秒显示 0）；虚拟化环境无物理磁盘计数器时降级为 0。

## [v0.18.0] - 2026-08-08

### 新增

- **电源计划删除功能（每条计划新增删除按钮）**：
  - 后端：`secm-datasource::power::delete_scheme`（powrprof.PowerDeleteScheme，与 `powercfg /delete` 等价，零进程 spawn）+ `settings::delete_power_plan` + `delete_power_plan` 命令；激活中的计划由 API 拒绝（错误码 50），前端亦禁用其删除按钮。
  - 前端：设置页电源计划每条新增删除按钮（Trash2 图标，hover 红色警示），点击弹 AlertDialog 二次确认（防误删，删除不可恢复），删除后自动刷新列表并记录操作消息。
  - 验证：真机 `real_machine_delete_scheme` 通过（复制→删除→重复删除报错 2→删激活计划报错 50）；全量单测 64 passed。

### 修复

- **`sensor::tests::test_cpu` 过时断言**：v0.17.5 驱动自愈后本机 WinRing0 通道可用，温度不再恒为 0；断言改为「非负 + 不 panic」的降级语义。
- **`net_stats::manual_tests::dump_net_stats` 过强断言**：会话累计量首次调用基线=当前值（差值 0），`total_download > 0` 恒不成立；改为断言接口存在 + 速率有限值。

## [v0.17.5] - 2026-08-08

### 修复

- **CPU 卡片温度/功率数据丢失（启动自愈驱动通道）**：
  - 根因：驱动安装/探测链 `ensure_hwmon_installed` 此前仅由 `driver_status` / `driver_install` 命令触发且前端从未调用；WinRing0（DEMAND_START）服务停止或被第三方改动后（如 binPath 指向系统目录的旧实例启动失败 0x5），无人拉起 → CPU 温度（hwmon→WinRing0 SMN 链）与功率（WinRing0 RAPL）恒为 0，卡片显示 "—"。
  - 修复：应用启动时后台线程自动执行一次 `ensure_hwmon_installed`（非管理员走既有通道探测不弹 UAC；管理员自动重建并启动失效服务，`scm_ensure_service_running` 内置「存在但启动失败→重建」逻辑），通道状态写入用户日志页便于排查。
  - 验证：真机 `real_machine_ensure_installed` 通过（Secure Boot 下 L0 577 → 自动转 L1 WinRing0，CPU 77.4°C，幂等通过）。

## [v0.17.4] - 2026-08-07

### 变更

- **关于页「第三方许可」更新（严格核对 v0.17.4 依赖树）**：
  - 新增：cmdk / next-themes / sonner（前端）、encoding_rs / tiny_http（Rust）、WinRing0（OpenLibSys，BSD-2-Clause，resources/driver 分发的内核驱动）
  - 移除：「协议标准」组（STUN 为自研 RFC 实现，非第三方许可）
  - 保留：MPL-2.0（cssparser 等）与 ICU4X（Unicode-3.0）copyleft 声明（cargo tree 验证仍在依赖树）

## [v0.17.3] - 2026-08-07

### 修复

- **CPU 功耗未显示（RAPL 读取生效）**： 的 MSR 读 IOCTL 常量修正为
  0x9C402084（本变体实际值；0x9C402004 为 stub 返回恒值）。实测 7800X3D
  PWR=37-45W 实时显示（能量差值法，32 位回绕处理），Dashboard CPU 功耗从 "—" 恢复。

## [v0.17.2] - 2026-08-07

### 修复

- **CPU 温度读取恢复**：`get_cpu_data` 温度回退链扩展为 hwmon 驱动 → WinRing0 SMN（AMD Zen，tctl 真实温度）→ 0。Secure Boot 下 hwmon 测试签名被拒（577）时，WinRing0 通道自动接管（实测 7800X3D 57-60°C 与 HWiNFO 同量级）。
- **硬件信息页刷新闪 cmd 窗口**：nvidia-smi 子进程调用补 `CREATE_NO_WINDOW`（sensor.rs + hardware.rs 两处）——每秒传感器轮询不再弹出控制台窗口。

### 新增

- **CPU 功耗 RAPL 读取链路（就绪）**：`winring0.rs` 新增 MSR IOCTL + AMD RAPL 能量差值法（PWR_UNIT 0xC0010299 / PKG_ENERGY 0xC001029B）。内置 WinRing0 变体 MSR 为 stub（恒 0x2），功耗暂降级 n/a；更换支持 MSR 的完整版驱动后自动生效。

## [v0.17.1] - 2026-08-07

### 修复

- **硬件信息页 GPU/CPU 数据采集来源恢复**：`sensor.rs` 的 `get_gpu_data` 重写为 nvidia-smi 真实采集（占用率/温度/频率/功耗/显存），3 秒超时 + 2 秒缓存（避免每秒子进程开销）；非 NVIDIA 环境自动降级为空。此前 GPU 数据随 LHM bridge 移除而缺失（首页显示"GPU 数据源已随旧采集链移除"）。
- **构建流程修复**：release 构建必须使用 `npx tauri build`（CLI）；直接 `cargo build --release` 会导致窗口加载 devUrl(127.0.0.1:5173) 而空白。

### 变更

- **硬件检测页移除 GPU/CPU 实时传感器**（数据展示统一收敛到硬件信息页）；页面保留磁盘 S.M.A.R.T 卡片视图。

## [v0.17.0] - 2026-08-07

### 新增

- 纯 Rust 驱动采集模块 `secm-datasource`（`datasource/` 独立 crate，v0.1.0）：13 个采集点（P1-P13）底层全部切换为 Win32 API / 注册表 / 纯 Rust HTTP——服务管理 advapi32（P10-P12）、电源计划 powrprof（P7-P9）、链路速度/本地 IP iphlpapi（P3/P4）、DNS 刷新 dnsapi（P13）、公网 IP ureq + URL 白名单（P5）、注册表补丁/GPU（P2/P6）、激活状态注册表近似 + WMI 兜底（P1）；统一错误模型 `CollectError`（thiserror，6 变体）
- 接入层 `src-tauri/src/ds_util.rs`：`CollectError` → 用户可见中文消息 + R8 日志（含 API 名/操作名/详情）+ `use_legacy()` 编译期开关
- 集成测试 `datasource_native`（12 例）+ `ds_adapter_integration`（13 例）；datasource 单测 42 例
- 部署说明 `docs/DEPLOY.md`：构建/打包/安装/数据源切换/故障排查操作手册
- 磁盘枚举与 S.M.A.R.T 读取接口 `datasource/disk.rs`：PhysicalDriveN 枚举 + NVMe 健康日志 / ATA 属性表（IOCTL 三级降级链：IOCTL 全量 → WMI HealthStatus 兜底 → 可读错误），`SECM_DISK_MOCK` 开关；`disk_info.rs` 健康度判断（Good/Warning/Critical）+ 告警字段；命令 `list_disks` / `get_disk_smart`（对应 `GET /api/disks`、`GET /api/disks/:id/smart`）；实测虚拟化环境 9 盘枚举 + WMI 兜底健康状态（设计见 `docs/designs/2026-08-07-disk-smart.md`）
- 硬件检测页重构为磁盘 SMART 卡片视图：`DiskSmartCard`（收起态型号/健康 Badge/接口介质容量/温度通电磨损，Warning/Critical 时 2px 边框 + 顶部色条 + 告警文案）+ `DiskSmartDialog`（`get_disk_smart` 动态加载、健康度分析、ATA 属性表 ID 十六进制/原始值千分位/语义色、NVMe 健康日志、8s 超时保护 + 弹窗内刷新）；30s 静默轮询 + 手动刷新（磁盘数量变化卡片自动增减）；`src/lib/diskSmart.ts` 契约类型 + 语义色映射
- GPU/CPU 传感器实时卡片 + 可配刷新间隔：消费 `get_hardware_sensors` / `GET /api/hardware/sensors`（温度/频率/功耗，无法采集显示 —；CPU 多核聚合最高核温 + 平均频率）；页头刷新间隔控件（实时 1s ↔ 自定义秒数互斥，正整数校验 ≥1s）；请求失败 destructive Alert + 重试按钮，有数据时保留旧卡片
- 硬件传感器服务 `src-tauri/src/hardware.rs`：GPU/CPU 温度/频率/功耗结构化采集接口，JSON 契约 `{ gpus: [{id, name, temperature, frequency, power}], cpus: [...] }`（无法采集字段为 null）；GPU 走 nvidia-smi 真实采集（温度/频率/功耗）+ 注册表 Class 键枚举全部物理显卡（过滤虚拟显示适配器），CPU 每逻辑处理器频率走 CallNtPowerInformation、温度走 hwmon 驱动；`SECM_HARDWARE_MOCK=1` 可配置模拟数据（`SECM_HARDWARE_MOCK_GPUS`/`SECM_HARDWARE_MOCK_CORES`）
- 硬件传感器 HTTP 服务（tiny_http，127.0.0.1:17860，`SECM_HTTP_PORT` 可覆盖）：`GET /api/hardware/sensors` 响应 < 500ms（实测 ~50ms），404/405/403/500 携带明确错误码（HW_NOT_FOUND / HW_METHOD_NOT_ALLOWED / HW_ACCESS_DENIED / HW_SENSOR_NOT_FOUND / HW_INTERNAL_ERROR）与中文修复建议；Tauri 命令 `get_hardware_sensors` 同链路直连
- 集成测试 `hardware_integration`（4 例）：采集契约 / mock 模式 / HTTP 端点（含 <500ms 耗时断言）/ mock HTTP；datasource `registry::enum_gpu_cards()` 枚举全部显卡 + `GpuCard` 结构

### 变更

- 7 个业务模块（sysinfo / net_stats / ip_info / settings / game_env / cleanup / environment）P1-P13 + K2 全部切换到 `secm-datasource` 驱动输出，前端 54 条 Tauri 命令契约零改动
- 清理缓存页 DNS 刷新实现：`ipconfig /flushdns` → `DnsFlushResolverCache`（dnsapi，纯 Rust）
- AI 工具版本检测（K2）去 `cmd /c` 中转：`.exe` 工具直接 spawn 绝对路径（`.cmd`/`.bat` 保留 `cmd /c`）

### 保留（过渡期）

- `legacy-data-source` feature 一键回滚旧 PowerShell/外部命令链路（`cargo build --features legacy-data-source`）；`proc_util::run_ps` 保留至发布回归后删除（见设计文档 §14.4）

### 修复

- hwmon 驱动发行链路（数据缺失事故，见 `docs/DIAG_hwmon_v0.17.0_data_missing.md`）：
  - `tauri.conf.json` `bundle.resources` 打包驱动（hwmon.sys/inf/cat/cer），新增 `scripts/stage-driver.mjs` + `npm run tauri:build`（发布构建缺驱动产物即报错，防止产出不含驱动的安装包）
  - 驱动补注册 `IRP_MJ_READ` 派发（`dispatch::read`，ReadFile 与 IOCTL_QUERY_ALL 等价返回全量数据）；hwmonctl 新增 `read` 子命令验证
  - `install.bat` 内置测试签名预检 + 安装后自验证；`hwmon-cli install` 成功后自动执行查询冒烟
  - 修正 ABI 尺寸文档契约：`HwmonSensorData` 实为 **112 字节**（4×u32 + 4×f64 + 32×u16，早期误记 88B），QUERY_ALL 回复 14352B（非 11280B），QUERY_SENSOR 输出 128B（含对齐 padding）——三端经 `#[path]` 共享源码运行时不漂移，文档与断言已同步
  - 新增单元测试：ioctl ABI 尺寸/编码/常量契约（7 例）、dispatch 缓冲区校验（6 例）、sensor 数据构造契约（10 例），驱动单测 16 → 39 例

## [v0.16.0] - 2026-08-06

### 新增

- hwmon 数据源集成（src-tauri/src/hwmon.rs）：应用直接经 DeviceIoControl 查询 hwmon 内核驱动（\\.\HWMON，QUERY_ALL），`#[path]` 共享驱动 ioctl.rs ABI 零漂移，手写 kernel32 FFI 零第三方依赖；温度/风扇/电压展示在硬件页（hwmon 传感器卡片）
- 集成测试 tests/hwmon_integration.rs：解析层构造回复验证分类/状态/回填逻辑 + 模拟普通用户（非提权）真实查询与连续读取
- eslint.config.js（flat config）：修复 `npm run lint` 自仓库初始化起因缺少 ESLint 配置而无法运行的问题（无规则仅语法级检查）
- 布局层迁移 shadcn/ui sidebar 组件族：MainLayout 重构为 SidebarProvider + AppSidebar（折叠状态 cookie 持久化、Ctrl/Cmd+B 快捷键切换）
- 组件库补齐 20 个 shadcn 组件（dialog / dropdown-menu / popover / scroll-area / sheet / sidebar / sonner / table / alert / alert-dialog / command / breadcrumb 等），ui 组件共 36 个
- Cleanup 页 8 个清理操作全部增加 AlertDialog 二次确认，防止误触
- Logs 页日志区改用 ScrollArea 滚动 + 日志级别 Badge + 容量 Select

### 变更

- 传感器数据源切换为 hwmon：CPU 温度由 hwmon 驱动回填（槽位 0 = CPU Package），不可用时降级提示；main.rs 移除强制管理员启动（普通用户可直接运行，需要管理员的功能由各命令内部检查）
- 8 个页面全面 shadcn 组件化重构（Card / Badge / Alert / Skeleton / Tabs / Switch / Select / Button），统一页面骨架与语义色，暗色主题兼容
- Dashboard / Hardware / Environment 卡片化重构，抽取 SectionHeader / InfoRow 复用组件；环境检测游戏分组改用受控 Tabs
- 修复 Hardware 页进度条动态类名拼接不生效问题（改为静态字面量映射表，颜色按设计意图生效）
- 侧边栏网络信息卡片改用 shadcn Card 结构（数据逻辑不变）

## [v0.15.53] - 2026-08-02

### 修复

**服务管理显示名称列乱码**

- 根因：中文 Windows 的 sc 命令输出为 GBK（ANSI 代码页）编码，Rust from_utf8_lossy 按 UTF-8 解析 → 中文显示名乱码
- 修复：新增 decode_sc_output（UTF-8 尝试失败回退 GBK 解码，encoding_rs crate），list_all_services / query_start_type 统一使用
- 命令行验证："聚合数据平台服务"等中文显示名 GBK 解码正确

## [v0.15.52] - 2026-08-02

### 修复

**服务列表全部展开彻底修复（绝对定位方案，无头实测验证）**

- v0.15.51 失效根因：grid 行高按 max-content 计算，flex-1/overflow-hidden 均不影响 max-content → 表格内容仍撑爆行高
- 修复：表格容器绝对定位（脱离文档流 → 不参与 max-content）→ 行高由左列（系统优化开关）决定 → 表格高度 = 左列高度联动
- 无头实测：200 行数据下两列等高 850px、表格 677px 滚动、不全部展开

## [v0.15.51] - 2026-08-02

### 修复

**服务列表跟随系统优化开关弹性高度（修复全部展开过长）**

- 根因：右列 CardContent 缺少 flex-1 → 高度内容驱动 → 表格 flex-1 无剩余空间 → 表格高度 = 全部行（数百行全部展开）
- 修复：CardContent 加 flex-1 min-h-0 + CardHeader shrink-0 → 表格弹性高度 = 卡片剩余 = 随左列（系统优化开关）高度联动，行多时滚动、无留白

## [v0.15.50] - 2026-08-02

### 修复

**服务列表下方 180px 留白（根因：等高下左列更高）**

- 诊断：无头实测无留白（数据空）；真实环境左列多渲染电源计划列表 + 异类策略 Select 完整区（约 +180px）→ 等高拉伸右列 → 表格固定 473px 不跟随 → 留白
- 修复：表格容器 h-[473px] → flex-1 min-h-[473px]，弹性填满卡片剩余空间（保底 11 行可视，行少均分、行多滚动），留白归零

## [v0.15.49] - 2026-08-02

### 变更

**服务列表默认显示 11 条**

- 行高紧凑化：表头/单元格 py-2 → py-1.5（行高 40px）
- 表格容器固定 473px（表头 33 + 11×40）：默认可视 11 条，行少时 table h-full 均分铺满、行多时滚动

## [v0.15.48] - 2026-08-02

### 变更

**服务列表状态列圆点化 + 垂直铺满容器**

- 状态列移除"运行中/已停止"文字，改为彩色圆点：绿=运行中、红=已停止、黄=其他（悬停显示状态）
- 列表铺满修复：CardContent flex-col + 表格容器 flex-1 填满卡片剩余高度 + table h-full（行少时行均分铺满、行多时滚动）

## [v0.15.47] - 2026-08-02

### 变更

**服务管理列表布局整理**

- 单元格 padding 紧凑化：px-4 py-2.5 → px-2.5 py-2，行高缩小、内容铺满容器宽度
- 服务名称/显示名称列 whitespace-nowrap：列宽随名称长度响应式自适应（超长显示名 max-w 截断）
- 表格显式 table-auto，状态/操作列宽微调

## [v0.15.46] - 2026-08-02

### 新增

**服务管理：全量枚举 + 关键词搜索**

- 新增 list_all_services 命令：sc query type= service state= all 一次遍历全部服务，启动类型 8 线程并行 sc qc 查询（数百服务约 1-3 秒）
- 服务列表从固定 16 项扩展为系统全部服务（数百项）
- 新增关键词搜索框（匹配服务名/显示名，实时过滤）
- 列表固定高度 440px 滚动容器，防止页面被数百行撑爆

## [v0.15.45] - 2026-08-02

### 修复

**服务管理启动类型读取当前状态**

- 根因：sc query 输出不含 START_TYPE（仅 STATE），启动类型解析永远命中"未知"
- 修复：状态用 sc query、启动类型改用 sc qc（QueryServiceConfig）读取，实测验证 DISABLED/AUTO_START/DEMAND_START 正确解析

## [v0.15.44] - 2026-08-02

### 新增

**系统设置 → 服务管理：启动类型设置**

- 新增 set_service_start_type 命令（sc config start= auto/demand/disabled，管理员检查）
- 服务列表"启动类型"列改为下拉：自动/手动/禁用，即时生效并刷新列表
- 设置手动/禁用后服务不会随系统启动自动拉起，配合停止操作防止自动重启

## [v0.15.43] - 2026-08-02

### 修复

**硬件趋势卡底部留白消除（图表弹性填满）**

- 根因：等高布局下硬件趋势卡内容矮于网络流量卡，底部残留大量空白
- 修复：图表容器 flex-1 弹性填满等高剩余空间（ResponsiveContainer height 100%），CPU/GPU/内存标签组贴底
- 效果：图表整体下移放大、底部零留白，与网络流量卡等高对齐

## [v0.15.42] - 2026-08-02

### 修复

**网络图表 Y 轴刻度数值/单位改为左右单行排列**

- 根因：Recharts Text 组件在 tick 宽度不足时按空格自动换行（"1.9 GB" → "1.9"/"GB" 上下两行）
- 修复：Y 轴宽度 42→60px 容纳单行；formatAxisBytes 长值减少小数位（≥100 时 toFixed(0)）缩短文本

## [v0.15.41] - 2026-08-02

### 变更

**硬件检测页加入开发提示**

- 页头下方新增提示条："⚠ 当前页面正在开发其他功能，仅供展示。"（黄色警示样式）

## [v0.15.40] - 2026-08-02

### 修复

**网络图表 Y 轴初始化异常（真根因：历史数据单位不匹配）**

- v0.15.39 过滤未生效原因：真实速率 1.9MB/s 换算后恰好在阈值内
- 真根因：Rust chart_append 写盘 dl/ul 单位为 bytes/s（未 /1024），前端恢复时误当 KB → 显示值 ×1024 膨胀（1.9MB/s → 1.9GB）
- 修复：恢复历史时 /1024 统一为 KB（与实时采样路径一致），过滤阈值同步 ×1024
- 实测验证：原始 2,085,327 bytes/s → 恢复后 1.99 MB ✓

## [v0.15.39] - 2026-08-02

### 修复

**网络流量图表 Y 轴初始化异常（从 2.3G 开始）**

- 根因：chart_history.jsonl 累积历史含异常采样（瞬时毛刺/旧版本单位差异），恢复后撑大 Y 轴 domain，30 点滑出才恢复
- 修复：恢复历史时过滤异常点（负值/非有限数/超 2GB/s）；实时采样同样过滤短 interval 速率毛刺
- 兜底：Y 轴 domain 上限钳制到 2GB/s（保留万兆级真实数据），异常值不再影响坐标轴

## [v0.15.38] - 2026-08-02

### 新增

**硬件检测页（位于环境检测上方）**

- 侧边栏新增"硬件检测"入口（橙色 Cpu 图标），位于环境检测之前
- 页面展示：CPU（型号/核心数/频率/温度/功耗）、GPU（型号/显存/频率/温度/功耗）、内存（容量/可用/已用）、磁盘明细（逐盘容量/使用率）
- 支持手动重新检测（含检测时间戳），数据源复用 get_sensor_data

## [v0.15.37] - 2026-08-02

### 变更

**网络工具目标地址默认值改为 bilibili.com**

- 共享目标地址初始值 8.8.8.8 → bilibili.com

## [v0.15.36] - 2026-08-02

### 变更

**网络工具卡片融合**

- 共享控制卡（工具选择 + 共用输入 + 按钮）与当前工具卡（参数 + 输出）融合为单一卡片
- 卡片标题"网络工具"：头部左侧标题 + 右侧工具选择器；内容自上而下：共用输入 → 分隔线 → 参数区 → 实时输出

## [v0.15.35] - 2026-08-02

### 变更

**网站测试卡片移至网络诊断页第一行**

- 卡片顺序调整：页头 → 网站测试（第一行）→ 共享控制卡 → 当前工具卡

## [v0.15.34] - 2026-08-02

### 变更

**网站测试卡片迁移至网络诊断页**

- Dashboard 移除网站测试卡片（含全部状态/逻辑/localStorage 持久化）
- Network 页底部新增网站测试卡片：功能、自定义站点、自动检测定时器完全保留
- 清理 Dashboard 无用 imports（ExternalLink/Plus/Pencil/Trash2/Input/Switch/Button）

## [v0.15.33] - 2026-08-02

### 变更

**图表布局优化（v0.15.32 变更实际生效于此版）**

- 硬件趋势图：图表高度 260→200px、Y 轴区域 60→32px、margin.left 5→0 → 主体向左靠
- 网络流量图：图表高度 180→150px、Y 轴区域 62→42px、margin.left 5→0 → 主体向左靠
- 两卡内容区 px-4→px-3（左边距 -8px）、pb-3→pb-2
- 网络卡：Card py-4 + 头部 pt-0 + 统计卡 mb-3→mb-2 + p-2
- 图例间距 mt-2→mt-1.5

## [v0.15.32] - 2026-08-02

### 变更

**图表布局优化（注：编辑丢失未生效，实际变更见 v0.15.33）**

- Y 轴区域收窄：硬件图 width 60→40、网络图 62→50，图表主体向左靠
- 卡片高度降低：硬件图 260→200px、网络图 180→150px；卡片 py-4 + 头部/内容 padding 压缩；统计卡 p-2.5→p-2；图例间距收紧

## [v0.15.31] - 2026-08-02

### 修复

**内容约束恢复 + 底部留白收敛（实测 1-8px）**

- 回退 v0.15.30 弹性拉伸：着色器子卡恢复固定紧凑高度、进程表格恢复固定 7 行
- 根因（无头浏览器实测）：Card h-full = 100% section 高度，空白恒 = 标题高度（28px），压缩/加高内容均无效
- 根治：section 改 flex flex-col + Card 改 flex-1（吃掉标题后的剩余），等高保持且空白收敛
- 表格行高 33→36px（285px = 表头 33 + 7×36），输入框 py-2→py-2.5 同步加高
- 实测：左列净空白 39→8px、右列 29→1px

## [v0.15.30] - 2026-08-02

### 修复

**卡片底部留白彻底消除（弹性填满，实测验证）**

- 无头浏览器像素级实测：左列子卡下方 39px、右列表格下方 29px 空白
- 根因：等高拉伸后两列内容为固定高度，不吸收剩余空间
- 左列：CardContent 加 flex-1 + 着色器子卡区 flex-1 + auto-rows-fr 两行均分拉伸（子卡 57→76px 填满）
- 右列：进程表格 flex-1 min-h-[264px] 弹性吸收剩余（264→292px），7 行保底不变
- 实测两列底部仅剩标准 padding（37px = p-3 + py-6），零空白

## [v0.15.29] - 2026-08-02

### 修复

**卡片底部留白进一步消除（等高基准右移）**

- 根因：等高取两列最大值，左列（系统清理）高于右列（进程管理），差额成为右列底部留白
- 修复：压缩左列 32px（基础卡/子卡 p-3→p-2.5、着色器标题 mb-3→mb-2、分隔线 pt-3→pt-2），左列高度 ≤ 右列，等高基准降至右列 → 右列零留白

## [v0.15.28] - 2026-08-02

### 变更

**系统清理/进程管理卡片高度减少约 30px**

- 两列同步压缩：标题行 mb-3→mb-2（-4px）+ CardContent p-4→p-3（-16px）+ space-y-4→space-y-3（-8px）
- 等高布局下每列 -28px，页面更紧凑；7 行进程表格与功能不受影响

## [v0.15.27] - 2026-08-02

### 变更

**进程列表可视区扩展为 7 行**

- 表格高度 231px → 264px（表头 33 + 7×33），默认展示 7 条进程
- 超出 7 行仍由独立滚动条滚动查看

## [v0.15.26] - 2026-08-02

### 变更

**移除进程列表底部统计描述**

- 删除"共 N 个进程 · 滚轮/滚动条查看全部"统计行（含搜索结果计数）
- 列表底部不再显示文字，界面更简洁

## [v0.15.25] - 2026-08-02

### 修复

**进程列表固定 6 行可视 + 独立常驻滚动条**

- 可视区固定 231px（表头 33 + 6×33 行），不再随卡片高度展开全部
- 渲染全部进程（上限 200），超出 6 行时滚轮/滚动条查看其余
- 新增 .proc-scrollbar 专属滚动条样式：8px 宽 + 轨道底色，常驻可见（替代 WebView2 默认 overlay 细滚动条）
- 行数 ≤ 6 时行均分填满（无留白），> 6 时行高 33px 保底滚动

## [v0.15.24] - 2026-08-02

### 修复

**进程列表滚轮/滚动条查看全部恢复**

- 根因：非搜索时 slice(0, 6) 只渲染 6 条，列表无其他进程可滚
- 修复：始终渲染全部（上限 200），行数多时溢出滚动（行高 33px），行数少时行均分填满无留白
- 统计文案同步：共 N 个进程 · 滚轮/滚动条查看全部

## [v0.15.23] - 2026-08-02

### 修复

**进程管理列表滚动条真正恢复（根因修正）**

- 无头浏览器实测确认：min-height:auto 陷阱在 CardContent 层（父层被 200 行内容撑开到 6800px，子层 overflow 永不触发）
- 修复：CardContent 加 min-h-0，表格外层收缩到卡片内分配高度，行多时正确滚动

## [v0.15.22] - 2026-08-02

### 修复

**进程管理列表滚动条丢失**

- 根因：flex 链 min-height:auto 陷阱 — 滚动容器被内容撑开，overflow-y-auto 永不触发
- 修复：外层滚动容器加 min-h-0（允许收缩到 flex 分配高度），行多时（搜索全部匹配）滚动条恢复

## [v0.15.21] - 2026-08-02

### 修复

**进程管理与系统清理卡片等高对齐**

- 恢复 items-stretch 等高布局（section 加回 h-full）
- 进程表格 table → div 列表，行 flex-1 均分填满卡片剩余高度：等高 + 固定 6 条 + 底部零留白三者兼得
- 行数多时（搜索）min-h-[33px] 保底高度自动触发滚动，行高不塌缩

**搜索进程字体统一**

- 搜索输入框 text-xs → text-sm，与进程 PID / 优先级输入控件字号一致（label 均 text-xs）

## [v0.15.20] - 2026-08-02

### 变更

**进程管理操作区布局：搜索置左一行显示**

- 搜索进程输入框移到最左（PID 之前），一行排列：搜索 → PID → 优先级 → 应用优先级
- 微调各输入框最小宽度（130/90/110px）确保双列布局下一行容纳

## [v0.15.19] - 2026-08-02

### 变更

**进程管理列表固定 6 条 + 移除展开全部**

- 默认仅渲染前 6 条进程（搜索时展示全部匹配，滚动查看）
- 移除"展开全部/收起"按钮及相关状态，列表不再展开
- 统计文案同步：共 N 个进程 · 显示前 6 条

### 修复

**进程管理卡片底部留白消除**

- 两列布局 items-stretch → items-start，取消等高拉伸
- 表格移除 flex-1/min-h/max-h 强制高度，高度自适应 6 行内容，底部零留白

## [v0.15.18] - 2026-08-02

### 修复

**进程管理底部留白减少**

- 表格容器 flex-1 填充等高剩余空间（收起保底 6 行），底部零留白

## [v0.15.17] - 2026-08-02

### 变更

**进程列表收起状态：6 行可视 + 滚动全部条目**

- 收起：可视区 240px（约 6 行），渲染全部 200 条，滚动条滚动查看/挑选
- 展开：可视区 460px；统计文字同步（共 N 个进程 · 滚动查看）

## [v0.15.16] - 2026-08-02

### 变更

**进程列表收起状态即可滚动**

- 收起条数 6 → 20（20 条超出滚动区高度，滚动条必然出现，可直接滚动挑选）
- 展开全部保留（200 条）

## [v0.15.15] - 2026-08-02

### 修复

**进程管理列表滚动区保障**

- 表格容器 min-h-0 → min-h-[280px]：等高布局下保证可滚动区域，滚动条稳定可拖动挑选
- 展开全部保留（200 条，max-h-[460px] 内滚动）

## [v0.15.14] - 2026-08-02

### 修复

**进程管理列表无法选取其他进程**

- 点击进程行后自动收起列表并填入 PID（选中高亮可见）
- 表格滚动容器增加 max-h-[460px] 上限，展开后滚动/点击稳定可靠

## [v0.15.13] - 2026-08-02

### 变更

**进程管理列表：默认 6 条 + 展开/收起下拉**

- 默认展示 6 条，底部"展开全部"（ChevronDown）展开至 200 条，"收起"恢复
- 恢复两列等高（items-stretch + 表格 flex-1 填充）

## [v0.15.12] - 2026-08-02

### 修复

**清理缓存页：进程管理去底部留白 + 执行结果行距 + 行间距**

- 进程管理解除强制等高（items-start + 自然高度），消除卡片底部大片空白
- 执行结果列表项行距 py-2.5→py-3、消息行 space-y-0.5→space-y-1
- 页面纵向间距 32px→40px，第二行不再贴近第一行底部

## [v0.15.11] - 2026-08-02

### 变更

**进程管理列表展示条数 200 → 8**

- 表格默认只展示前 8 条进程（统计文字同步更新），页面更紧凑

## [v0.15.10] - 2026-08-02

### 修复

**清理缓存页行距调整**

- 页面纵向间距 24px → 32px（space-y-8），第一行两卡片间距 16px → 20px
- 执行结果第二行卡片与第一排卡片间距清晰，不再紧贴

## [v0.15.9] - 2026-08-02

### 变更

**清理缓存页：执行结果拆分到第二行独立卡片**

- 执行结果从系统清理卡片拆出，独立第二行卡片（标题 + 逐行时间戳列表）
- 进程管理卡片 flex-1 跟随系统清理卡片高度（表格区自适应填充，超出滚动）

## [v0.15.8] - 2026-08-02

### 变更

**网络流量趋势图单位自动换算**

- Y 轴刻度与悬浮提示由固定 KB/KB/s 改为按量级自动换算（KB / MB / GB）

## [v0.15.7] - 2026-08-02

### 变更

**关于页：项目展示改关注抖音 + 构建信息并入项目简介**

- 图片卡片标题改为"关注抖音"并补充引导文案（扫码关注，获取更多工具与教程）
- 构建信息（后端/前端/UI/平台）移入项目简介卡片，分隔线区隔；移除独立构建信息卡片
- 同步入库 源文件（ 为分发副本）

## [v0.15.6] - 2026-08-02

### 变更

**关于页：新增项目展示图片卡片 + 更新日志去图标**

- 赞助赞赏右侧新增"项目展示"卡片（public/dy.png，随安装包分发）
- 更新日志折叠项移除 History 图标，仅文字 + 版本数

## [v0.15.5] - 2026-08-02

### 新增

**关于页：更新日志**

- 折叠式展示 CHANGELOG.md 全文（构建时嵌入，随版本发布），标题旁显示版本数

## [v0.15.4] - 2026-08-02

### 变更

**侧边栏：链路协商速度字号与数据行对齐**

- 标题旁链路速度改为 text-[10px]，与下载/上传速率行一致

## [v0.15.3] - 2026-08-02

### 变更

**侧边栏：链路协商速度移至网络信息标题旁**

- 第一行数据行只保留下载/上传实时速率，更紧凑

## [v0.15.2] - 2026-08-02

### 修复

**网络流量速率/总量读数颠倒 + 会话累计**

- 根因：sysinfo 0.30 语义变更——`received()`/`transmitted()` 仅为自上次 refresh 的增量，`total_received()`/`total_transmitted()` 才是网卡累计字节。此前误用前者当累计量：速率卡片（差值≈0）不动、总量卡片显示每秒增量（形似速率）
- 修复：改用 `total_*` 累计字节作为采样源，速率 = 累计差值 / 间隔（仪表盘 + 侧边栏同步修正）
- 下载/上传总量改为**会话累计**：程序启动后以首次采样为基线，只累计增量；关闭程序自动清零（不持久化），网卡重插自动重建基线

## [v0.15.1] - 2026-08-02

### 变更

**系统优化开关各项补充描述**

- HAGS：使用 GPU 调度器管理显存，降低渲染延迟
- 窗口化游戏优化：提升窗口化 / 无边框游戏流畅度
- 鼠标精准度：提高指针精确度（鼠标加速），FPS 玩家通常关闭
- VRR：显示器动态刷新率，消除画面撕裂

## [v0.15.0] - 2026-08-02

### 新增

**系统优化开关：游戏模式**

- 新命令 `get_game_mode_state` / `set_game_mode_state`（HKCU\Software\Microsoft\GameBar\AutoGameModeEnabled，无需管理员）
- 对应 Windows 设置→游戏→游戏模式开关（通过在后台关闭内容来优化电脑玩游戏）
- 注册表项缺失时按系统默认（启用）展示；开关切换即时生效

## [v0.14.3] - 2026-08-02

### 变更

**电源计划选择改为滑动开关**

- 计划列表由点击按钮改为滑动开关（选中态 = 当前活动计划），视觉与系统优化开关统一
- 活动计划黄色高亮保留，开关切换即切换电源计划

## [v0.14.2] - 2026-08-02

### 变更

**系统设置页：左右两列布局**

- 宽屏（xl+）两列并排：左列系统优化（含电源计划/异类调度），右列服务管理；小屏自动堆叠
- 两卡片等高（items-stretch + h-full）；操作记录保持底部全宽

## [v0.14.1] - 2026-08-02

### 变更

**系统设置页：电源计划并入系统优化开关卡片**

- 电源计划（计划列表 + 异类调度策略）从独立卡片移入系统优化开关卡片底部，分隔线隔开
- 原双卡片并排布局改为单卡片全宽，设置项同屏可见

## [v0.14.0] - 2026-08-02

### 新增

**电源计划卡片：异类线程调度策略配置（混合架构 CPU）**

- 新命令 `get_hetero_policies` / `set_hetero_policy`（注册表读取 + powercfg 写入，AC/DC 同步）
- 异类线程调度策略与异类短运行线程调度策略两个下拉选择器（对应处理器电源管理设置，取值：所有处理器/高性能/首选高性能/高效/首选高效/自动）
- 未显式设置时回退显示系统默认值；非混合架构 CPU 显示"不支持"提示

## [v0.13.6] - 2026-08-02

### 变更

**网络诊断页重构：工具选择器 + 共用输入 + 统一按钮**

- 六个工具（Ping/Traceroute/Nslookup/NAT/Port/iperf3）改为顶部工具选择器切换
- 共用目标地址/网络栈/协议输入 + 统一开始/停止按钮（按当前工具执行），参数区与输出区随工具切换（各自状态保留）
- 单卡片布局：工具选择 → 配置参数 → 开始诊断 → 单一流式输出区，页面高度减半

## [v0.13.5] - 2026-08-02

### 修复

**环境检测页频繁弹黑窗（1-2 秒）**

- `game_env.rs` 的 `sc query`（Vanguard/BEService 服务检测，环境页 `get_game_presets` 触发链）缺 `CREATE_NO_WINDOW`，GUI 应用 spawn 控制台程序时弹出黑窗——已补标志
- 同类排查：iperf3（网络诊断）补 `CREATE_NO_WINDOW`（长时运行防黑窗常驻）；全项目 spawn 点扫描确认全部覆盖

## [v0.13.4] - 2026-08-02

### 修复

**清理缓存页：基础清理三卡片布局溢出**

- DNS 刷新 / 临时文件 / 工作集修剪卡片由横向布局改为纵向（图标+标题行 / 描述 / 全宽按钮）
- 标题超长截断、描述最多两行省略、按钮 `whitespace-nowrap` 全宽——窄列下不再互相挤压溢出

## [v0.13.3] - 2026-08-02

### 变更

**清理缓存页：进程管理移至系统清理右侧并排**

- 宽屏（xl 及以上）两列并排：左列系统清理（含执行结果）、右列进程管理；小屏自动堆叠
- 两卡片等高（grid items-stretch + h-full），与仪表盘图表区布局风格一致

## [v0.13.2] - 2026-08-02

### 变更

**清理缓存页：执行结果并入系统清理卡片**

- 执行结果从独立卡片移入系统清理容器（着色器缓存下方，分隔线隔开），操作与结果同屏可见
- 空态改为卡片内居中提示文本（不再嵌套 Card）

## [v0.13.1] - 2026-08-02

### 修复

**清理缓存执行结果多行显示 + 每行时间戳**

- 临时文件清理等多行结果（TEMP / Windows Temp / Prefetch）此前 `\n` 被渲染折叠为单行，现按行拆分显示
- 每行前缀 `[HH:mm:ss]` 时间戳，操作名行时间戳置首，等宽字体 + 保留缩进（错误子行对齐）

## [v0.13.0] - 2026-08-01

### 修复

**安装包缺 Mono.Posix 运行时资源导致传感器不可用**

- `tauri.conf.json` bundle.resources 补上 `runtimes/win-x64/` 下 3 个文件（Mono.Posix.NETStandard.dll + MonoPosixHelper.dll ×2）
- 缺失时 LhmBridge 加载 HidSharp 失败 → LHM Open() 失败 → 误报 "Run as Administrator"，安装版传感器全部不可用（target 直跑正常，仅 NSIS 包受影响）

### 新增

**图表历史持久化（跨重启恢复趋势图）**

- 新命令 `get_chart_history`（三处同步：lib.rs → commands.rs → 前端 invoke）
- 传感器/网络采样点每秒追加写盘 `chart_history.jsonl`（`temp_data::chart_append`），启动时读回拼接实时流，重启后趋势图不断档
- 损坏行容错（文件轮转/并发写入残留跳过，不中断恢复）

**WebView2 内存诊断工具集 `scripts/anim-leak/`**

- 进程级内存监控（monitor2/monitor3.mjs：SysEnv/LhmBridge/WebView2 全组工作集采样）
- 最小复现页 + CDP 采样、环境清理脚本，README 记录完整实验矩阵

### 变更

**图表恢复每秒实时绘制 + 启用动画（v0.12.1 限制解除）**

- 移除图表 5s 渲染节流（数据仍 1s 收集），趋势图实时跟随数据更新
- 重新启用 Recharts Area 动画（`isAnimationActive=true` + `animationDuration=1000`）：Recharts 已升级 3.10.1（动画系统重写：key 强制卸载 + rAF cleanup），release 安装版 30 分钟进程级监控验证零泄漏，v0.12.1 关闭动画的修复不再必要
- dev 模式（tauri dev）下 WebView2 内存增长为 React 开发构建 + 软件渲染的固有行为（与图表/动画无关，已实测验证），内存验证请用 release 构建

## [v0.12.1] - 2026-08-01

### 修复

**仪表盘图表内存泄漏根治（WebView2 软件渲染环境）**

- 关闭 Recharts Area 动画（每秒数据更新时动画永不完成，合成层无限累积——泄漏主源）
- 图表渲染 5s 节流（数据仍 1s 收集），重绘量降 5 倍
- 图表组件 memo 化 + 模块级稳定 props（tick/tooltip/formatter），阻断每秒父组件重渲染传导
- 历史数据点 60→30，每帧绘制量减半

**隐藏基线泄漏：移除 animate-ping 常驻动画**

- 侧边栏状态点与 LiveIndicator 的无限呼吸灯动画改为静态指示点
- 该动画在 WebView2 软件光栅化下持续泄漏（所有页面基线 ~0.15MB/s+），为 v0.10.0 修复未覆盖的第二泄漏源
- release 模式验证：renderer 10 分钟内存零增长（115MB 稳定），此前 2s 节流仍以 0.34MB/s 增长

**侧边栏网络信息卡片留白过多**

- shadcn Card 默认 `py-6 gap-6`（上下 24px 内边距）未覆盖导致留白，改为 `py-1.5 gap-1` 并收紧内部 padding/行距

## [v0.12.0] - 2026-08-01

### 新增

**链路协商速度显示**

- 新命令 `get_link_speeds`（PowerShell Get-NetAdapter 一次性查询，不进 1s 轮询热路径；UTF-8 强制输出修复中文网卡名乱码）
- 仪表盘网络流量卡标题：活跃连接旁显示链路速度（如 `5 个活跃连接 · 1 Gbps`）
- 侧边栏网络信息卡片：链路速度 + 下载/上传单行展示 + IP 信息（卡片化重构）

### 变更

**网络诊断结果拆分到功能卡片**

- 每张功能卡（Ping / Traceroute / Nslookup / NAT / Port / iperf3）内嵌独立输出区（新增 `DiagOutput` 共享组件）
- 仅点击开始时才流式输出，移除底部聚合「诊断结果流」卡片
- 每卡新增「清除」按钮，点击复位输出（loading 时禁用）
- 流式追加自动滚动到底部

## [v0.11.0] - 2026-08-01

### 变更

**关于页第三方许可改为折叠展示**

- 许可区块默认折叠，点击表头展开/收起（首次启用 shadcn Accordion 组件）
- 折叠态显示条目摘要（37 项 · 8 组，从清单数据派生，随条目增删自动更新）
- `index.css` 补齐 accordion 展开/收起动画 keyframes（此前组件已存在但动画未注册）

## [v0.10.3] - 2026-08-01

### 新增

**第三方许可完整声明（关于页）**

- 新增 30+ 项第三方组件许可声明，按 8 组分类展示：核心框架 / 前端依赖 / Rust 后端 / C# 桥接器 / 内核驱动 / 运行时与打包 / 间接依赖 / 协议标准
- 覆盖 npm、crates.io、NuGet、本地 DLL、内核驱动及 WebView2 / NSIS / .NET 8 Runtime 等分发组件
- 补充 MPL-2.0 间接依赖（cssparser / selectors 等）与 ICU4X（Unicode-3.0）声明

### 修复

**关于页第三方许可声明错误**

- 修正 `LiteMonitor — MIT` → `LibreHardwareMonitorLib 0.9.7 — MPL-2.0`（实际使用的库与其真实许可证）
- 补充 PawnIO 内核驱动作者声明（© namazso）

## [v0.10.2] - 2026-08-01

### 新增

**着色器缓存按厂商拆分清理**

- 新增 4 个独立命令：`clean_nvidia_cache` / `clean_amd_cache` / `clean_directx_cache` / `clean_steam_cache`
- 保留 `clean_shader_cache` 一键全清（合并子结果）

**清理缓存页面重构**

- 导航「清理优化」→「清理缓存」
- 基础清理（DNS/临时文件/工作集）与着色器缓存合并为单容器
- 执行结果添加 HH:mm:ss 时间戳
- 清理按钮：描边 + 执行中动画 + 成功绿色高亮 1 秒后复位

### 修复

**清理任务误报成功（实际未清理）**

- 根因：`success` 硬编码 true，路径不存在/删除失败也报成功
- 修复：`success` 反映真实删除结果（有失败即报失败）

**NVIDIA 缓存路径错误**

- GLCache 误用 APPDATA → 修正为 `LOCALAPPDATA\NVIDIA\GLCache`
- 新增缺失的 `LOCALAPPDATA\NVIDIA\DXCache`

**Steam/CS2 清理失效（任意盘符游戏库）**

- 根因：`libraryfolders.vdf` 中路径的 `\\` 转义未还原，非安装盘库被跳过
- 修复：新增 `vdf_unescape()`，正确遍历所有 Steam 游戏库（含 Y 盘等）

**被占用/只读缓存文件无法删除**

- 只读文件：删除前 `clear_readonly()` 清除只读属性
- 被占用文件：重试 2 次后 `MoveFileEx` 标记重启后删除（标准方案），不再误报失败
- CS2 `remove_dir_all` 失败时回退逐文件清理 + 标记
- 清理后删除空缓存目录（`remove_empty_dirs`）

**删除失败仍累计释放字节**

- `clean_directory` 改为删除成功才计入 `bytes_freed`

## [v0.10.1] - 2026-08-01

### 修复

**应用黑屏（dev 与 release）**

- 根因 1：`NetIfaceProvider` 层级错误 — MainLayout 在 Provider 外调用 `useNetIface()` 抛错，导致 React 崩溃黑屏。Provider 提升至 `App.tsx` 根层
- 根因 2：多虚拟显示适配器（ToDesk/GameViewer/MuMu）环境下 WebView2 GPU 渲染失败。`additionalBrowserArgs` 增加 `--disable-gpu`（软件渲染）

**网络流量模块与侧边栏数据不同步**

- 根因：Dashboard 与侧边栏各自独立轮询 `get_net_stats`，Rust 端速度基于全局快照链，两侧取到不同时间段的速度
- 修复：重构为单数据源架构 — `NetIfaceProvider` 统一轮询，Dashboard 与侧边栏读取同一份 `netStats`（以网络流量卡片为准）；新增 `getDisplayNet()` 统一展示口径
- 网卡选择联动：手动选择后两侧同步；未手动选择时自动跟随有流量的物理网卡

**dev 模式反复重建（watch 循环）**

- 根因：LhmBridge dotnet 构建产物（`lhm-bridge/obj`）变化被 Tauri 误判为源码变化，触发无限重建
- 修复：新增 `.taurignore` 忽略构建产物目录

**多实例冲突**

- 根因：SECM 无单实例机制，多个实例抢占 WebView2 user-data-dir，残留进程锁住构建产物
- 修复：接入 `tauri-plugin-single-instance`，第二实例启动时聚焦已有窗口并退出

**发行版 127.0.0.1 拒绝访问**

- 根因：`--proxy-bypass-list=<-loopback>` 非标准语法可能失效
- 修复：改用标准语法 `--proxy-bypass-list=<local>;ipc.localhost;localhost;127.0.0.1;::1;[::1]`

## [v0.10.0] - 2026-08-01

### 修复

**内存泄漏 — WebView2 renderer 无限增长 (3GB+)**

- 根因：Recharts 2.x 与 React 19 不兼容，Dashboard 图表每秒推送数据时内部累积对象不释放
- 修复：升级 `recharts` 2.15.4 → 3.10.1（官方支持 React 19），Tooltip `formatter` 适配 3.x 新类型签名
- 验证：release 模式 renderer 内存从 3.1GB 降至 ~150MB，页面恢复正常

**发行版页面无法加载 (127.0.0.1 拒绝连接)**

- 根因：裸 `cargo build --release` 未能正确嵌入前端资源
- 修复：改用 `npx tauri build` 完整构建流程

**孤儿 LhmBridge 进程残留**

- 根因：SECM 被强制终止（Ctrl+C/崩溃）时无法执行 `shutdown_bridge()`
- 修复：新增 `cleanup_orphan_bridges()`，启动时自动清理父进程已退出的 LhmBridge 残留

**Rust 侧内存累积优化**

- `System::new_all()` → `System::new()`（避免持久化全量进程快照，节省 10-50MB）
- `sysinfo::Disks` / `Networks` 周期性重建实例，防止 `refresh` 内部条目累积
- C# LhmBridge 字典对象池复用 + 每 60 轮强制 `GC.Collect(0)`

### 新增

**临时诊断数据磁盘化 (`temp_data.rs`)**

- 传感器/调试/bridge 诊断日志从内存/stderr 移至 `%APPDATA%\SECM\diag\` 磁盘文件
- 自动清理：单文件 >100MB 轮转、总目录 >1GB 删除最旧、启动删除 7 天前文件
- 消除高频 `eprintln!` 输出在 dev 模式下的缓冲累积

**内存测量工具**

- 新增 `scripts/mem_trace.ps1`，按 Chromium 角色（browser/gpu/renderer）跟踪 SECM 进程组内存

### 文档

- 新增 `CONTEXT.md` 领域词汇表（进程模型/内存现象/轮询术语）

## [v0.9.14] - 2026-08-01

### 修复

**环境检测 — AI 工具升级**

- 修复 `cmd /c` 分隔参数导致 npm 批处理文件无法正确执行的问题
- 全部 shell 调用统一为单字符串命令格式（`/c "npm install -g pkg"`）
- 涉及：`install_or_upgrade_tool`、`fetch_npm_latest`、`find_executable`、`try_get_version`、`try_npm_list_version`

## [v0.9.13] - 2026-08-01

### 修复

**环境检测**

- 修复 AI 工具安装/升级按钮高亮缺失
- 修复 opencode / openclaw 点击升级无响应（`cmd /c` 参数分隔 → 直接调用 `npm`）
- 升级后添加 500ms 文件系统同步等待

### 变更

**清理缓存**

- 页面标题「清理优化」→「清理缓存」

**性能优化**

- AI 工具检测 10 线程并行（原串行 `for` 循环）
- npm 最新版本查询 8 线程并行（原串行网络请求）
- 系统信息 8 字段并行采集（原串行调用）

## [v0.9.12] - 2026-08-01

### 新增

**环境检测 — AI 工具**

- 新增 PI Agent 检测（`@earendil-works/pi-coding-agent`）
- 新增 CC Switch 检测

## [v0.9.11] - 2026-08-01

### 变更

**架构深化 — 日志系统统一 + 流式基础设施提取**

- `debug.rs`：统一使用 `std::sync::Mutex` + `VecDeque`，与 `log.rs` 共享环形缓冲模式
- `log.rs` / `debug.rs`：明确职责边界文档（用户日志 vs 开发者诊断）
- 新增 `cancel.rs`：通用取消注册表独立模块，解耦 network 与 iperf3
- 新增 `hooks/useStreaming.ts`：通用流式操作 React 钩子（Channel 管理 + 竞态保护）
- `Network.tsx`：替换内联 hook → 导入通用 `useStreaming`

## [v0.9.10] - 2026-08-01

### 变更

**架构深化 — commands.rs 收拢**

- 命令脚手架统一为 4 个宏：`cmd_result!` / `cmd_val!` / `cmd_pure!` / `cmd_log!`
- 消除 25+ 条命令中的手工重复样板代码（计时 + spawn_blocking + 日志）
- 模块从 888 行缩减至 601 行（-32%），新增命令从 20 行样板缩减至 1~3 行

## [v0.9.9] - 2026-08-01

### 新增

**关于页面**

- 新增赞助赞赏卡片，展示赞赏二维码及致谢文案
- 关于页面三卡片改为多断点响应式网格布局（手机单列 / 平板二列 / 桌面三列）
- Hero 区域响应式适配（图标/文字随屏幕缩放）

### 变更

- 三卡片统一 min-h-[300px]，视觉高度一致

## [v0.9.8] - 2026-08-01

### 变更

**网络诊断**

- 全局共享控件栏：目标地址、网络栈（自动/v4/v6）、协议（ICMP/TCP/UDP）统一共用，一行排列
- 各卡片移除重复的输入框和按钮组，仅保留专有参数
- 网络栈/协议按钮组添加 outline 描边样式

## [v0.9.7] - 2026-08-01

### 新增

**网络诊断**

- 新增 iperf3 带宽测试命令 `iperf3_stream`，支持流式输出测试进度
- 网络页面重构，集成 iperf3 带宽测试 UI

### 变更

- `network::is_cancelled` 可见性改为 `pub`，支持跨模块取消操作

## [v0.9.6] - 2026-07-31

### 修复

**系统版本**

- 系统版本判定改为按 Build 号区分 Windows 10/11（Build ≥ 22000 判定为 Win11）
- 修复注册表 ProductName 残留旧值问题（Win11 升级安装后可能残留 "Windows 10"）

**发行版稳定性**

- 补全 LHM 桥接器运行时 DLL（`runtimes/win/lib/net8.0/*.dll`）到打包资源，修复发行版硬件信息无法获取
- 便携版 ZIP 包含完整 `Drivers/` + `lhm-bridge/` 目录
- 补充 `LhmBridge.deps.json` 到打包资源

**窗体稳定性**

- WebView2 添加 `--proxy-bypass-list=<-loopback>` 绕过系统代理，修复长时间运行后 127.0.0.1 拒绝访问（代理工具劫持 Tauri IPC 域名 ipc.localhost）

**黑框闪烁**

- `dotnet publish` 命令添加 `CREATE_NO_WINDOW` 标志，修复环境检测页切换时黑框闪烁 1-2 秒

### 变更

- 网站测试卡片从网络诊断页移至硬件信息页

## [v0.9.5] - 2026-07-31

### 新增

- 侧边栏网速下方新增 IP 信息栏（本地/公网 IPv4/IPv6 + 地理位置 + 时区）
- 国内 IP 检测：通过 curl 调用 myip.ipip.net 获取国内出口 IP 及省市 ISP 信息
- 硬件信息页：网络流量卡片与硬件使用趋势并排显示
- 网络流量卡片：新增网卡选择下拉框

### 修复

- Select 下拉框透明不可读（新增 `--color-popover` CSS 变量）
- 国内 IP 乱码：改用 curl 替代 PowerShell 正确处理 UTF-8 编码
- 国内 IP 截断：字节级扫描替代字符级避免 Unicode 偏移
- CPU 频率精度：仅采集核心时钟 + 取最大值
- 网卡下拉框频繁刷新闪烁
- 网络流量速率始终为 0（Networks 持久化实例 + refresh_list）

### 变更

- 窗口默认尺寸 1200→1400×800
- 网络流量卡片从网络诊断页移至硬件信息页
- AI 环境卡片移至环境检测页底部
- 全站 shadcn/ui 组件替换（Progress / Skeleton / Select / ToggleGroup / Switch / Badge）
- 全站动画系统（fade-in / scale-in / slide-in + 交错延迟）

## [v0.9.4] - 2026-07-31

### 新增

- 环境检测页：Windows 系统信息卡片（版本/Build/架构/激活/补丁/启动模式）
- 网络诊断页：网络流量统计卡片（速率/总量/趋势图/刷新间隔）

### 修复

- wmic → PowerShell 兼容 Win11 24H2+
- CPU 频率精度修复

## [v0.9.3] - 2026-07-30

### 变更

- 仪表盘导航栏及页面标题更名为「硬件信息」

## [v0.9.2] - 2026-07-30

### 新增

- 侧边栏 SECM 标语："事实上，恢复就是优化的终点。"

## [v0.9.1] - 2026-07-30

### 修复

**环境检测**

- 抑制 Windows 错误弹窗：`SetErrorMode` 阻止 "不支持的16位应用程序" 对话框
- 隐藏控制台黑框：外部命令添加 `CREATE_NO_WINDOW` 标志

**网站测试**

- CSP `connect-src` 追加 `https: http:` 修复发行版 fetch 被拦截导致全部显示不可达

## [v0.9.0] - 2026-07-30

### 新增

**网站测试**

- 网络诊断页新增网站测试卡片：Apple / GitHub / Google / YouTube 默认站点
- 点击卡片检测连通性和响应时间（HTTP HEAD）
- 支持自定义新增站点、编辑 URL、删除管理
- 数据持久化到 localStorage

### 变更

**网络诊断**

- 按钮组（Ping / Traceroute / Nslookup / NAT检测）移至标题同一行

**清理优化**

- 执行结果卡片移至进程管理下方

## [v0.8.0] - 2026-07-30

### 新增

**AI 环境检测**

- 全新 AI 环境卡片：检测 Claude Code / Codex CLI / Gemini CLI / Grok Build / Hermes / Ollama / OpenCode / OpenClaw
- 每个工具显示当前版本 + npm 最新版本，支持安装/升级一键操作
- Ollama 支持常见安装路径检测（LocalAppData / ProgramFiles），未安装时跳转官网
- OpenCode 自动检测 `.ps1` / `.cmd` 脚本执行器版本
- Claude Code 三级版本回退：`--version` → npm list 回退
- 独立异步加载：AI 环境、DirectX、VC++ 三卡片互不阻塞

**DirectX 诊断**

- 新增 Direct3D 11 诊断检测项
- GPU 适配器仅显示当前活动硬件 GPU（pci\*），过滤虚拟适配器（Root\*）

### 修复

- OpenClaw 版本号剥离工具名前缀和 git hash 后缀
- 所有 CLI 检测改用 `cmd /c` 包裹确保 PATH 和脚本执行器正确

## [v0.7.1] - 2026-07-30

### 修复

**VC++ 运行库检测**

- 修复去重闭包变量捕获错误导致 27 条条目被错误去重为 2 条（`extract_vc_name(&display_name)` → `n`）
- 修复 `installed` 判定缺少架构过滤导致单架构安装时双架构均标记为已安装
- 修复 `KEY_WOW64_*` 标志在 winreg 中行为异常导致注册表枚举不完整
- 移除冗余 WOW6432Node 路径，改为直接访问两个显式注册表路径
- `extract_vc_name` 增加 `v14` / `2022` / `2017` / `2019` → `2015-2022` 归一化

## [v0.7.0] - 2026-07-29

### 新增

**环境检测**

- 全新页面：DirectX 运行时版本 / GPU 适配器 / D3D12 / WDDM 诊断
- VC++ 运行库完整性检测（2005-2022，x64+x86，共 12 项）
- 通过枚举 Uninstall 注册表自动发现已安装版本，准确反映真实环境

### 变更

**仪表盘**

- 实时摘要移至第一行，4 个融合卡片（CPU/GPU/内存/存储）
- 移除轮播组件，趋势图下移至第二行
- Header 副标题简化为 CPU 型号 + GPU 型号

**清理优化**

- 执行结果移至第一行全宽显示，统一日志面板替代独立卡片
- 系统清理（第二行全宽 4 列）+ 进程管理（第三行全宽）
- 进程列表常驻展开，搜索框内联至优先级设置行

**网络诊断**

- 所有输入框、下拉框、按钮统一高度 30px
- 控件行统一底部对齐，移除不一致的 margin

**系统设置**

- 系统优化开关 (2/3 宽) + 电源计划 (1/3 宽) 并排布局
- 服务管理全宽表格，操作记录常驻底部
- 电源计划自动检测并导入卓越性能、自动应用
- 过滤 Windows 11 Overlay 虚假电源计划（前后端双重过滤）

### 修复

- DNS 缓存刷新 GBK 编码乱码：使用 `MultiByteToWideChar(CP_ACP)` 解码 ipconfig 输出
- 环境检测页首次加载可能黑屏：添加 `safeInvoke` 容错包装

## [v0.2.0] - 2026-07-25

### 新增

**网络诊断**

- 流式输出使用 Tauri v2 `Channel` API（替代 `emit`/`listen`），修复流式输出卡死问题
- 所有流式输出行添加毫秒级时间戳 `[HH:mm:ss.SSS]`
- Ping 统计卡片实时计算 min/avg/max（诊断过程中动态更新）
- Ping 新增自定义参数：包大小、TTL、截止时间
- 新增 `自动 | IPv4 | IPv6` 协议版本按钮组（Ping、Traceroute）
- 新增 `ICMP | TCP | UDP` 协议选择按钮组（Ping、Traceroute）
- 后端 `resolve_host_with_version()` 支持强制 IPv4/IPv6 解析
- 后端 `ping_streaming` 支持动态包大小 + 截止时间自动停止
- Ping 添加连续模式滑动开关
- Traceroute 新增 `probes_answered`/`probes_sent` 探测计数
- 诊断总结卡片全部固定显示（四选项卡）：实时统计 + 详情列表
- 流式输出框始终可见（空闲/运行/出错三种状态）
- 所有摘要卡片添加 min/avg/max 统计网格
- 网络诊断 Tab 迁移到 shadcn/ui `Tabs` 组件
- 所有输入框统一使用 shadcn/ui `Field` + `FieldLabel` + `Input`
- 控件行统一单行布局 `flex-nowrap`，输入框宽度自适应

**系统设置**

- 鼠标精准度：使用 `SystemParametersInfoW(SPI_GETMOUSE)` 官方 API 读取
- 鼠标精准度写入：修复 `SPI_SETMOUSE` 动作码 (`0x0004`)，添加 `SPIF_UPDATEINIFILE` 标志
- 窗口化游戏优化 + VRR：改为读取 `DirectXUserGlobalSettings` REG_SZ（与 Windows 设置完全对齐）
- 窗口化游戏优化 + VRR 共用同一注册表键，`update_dx_setting()` 互不覆盖
- HAGS：仅 `HwSchMode=2` 视为启用（对齐 Windows 规范）
- 电源计划：从 `powercfg /list` 改为纯注册表读取（零进程派生）
- 电源计划卡片改为滑动开关选择器 + 刷新按钮
- 系统优化开关卡片与电源计划卡片左右并排布局
- 切换操作添加 `loadingToggle` 防重入锁
- 设置项添加详细诊断日志 `[HAGS诊断]` `[GAMEOPT诊断]` `[VRR诊断]`

**基础设施**

- 创建 `src-tauri/capabilities/default.json`（Tauri v2 权限配置，生产环境必需）
- 所有 `Command::new()` 添加 `.creation_flags(0x08000000)` (CREATE_NO_WINDOW) 消除控制台闪烁
- 同步阻塞操作包裹 `tokio::task::spawn_blocking` 防止线程池饥饿
- 移除 `simplelog` 重复日志依赖
- Vite 配置添加 `@` 路径别名解析
- CSP 安全策略设置
- 版本号动态加载（侧边栏）
- 侧边栏版本号动态获取（invoke get_app_version）

### 修复

- 流式输出卡死在加载状态（Tauri v2 Channel API 迁移）
- `nslookup_stream` 缺少 `clear_cancel` 资源泄漏
- `set_game_optimization` 写入键与读取键不匹配（`GameDVR_Enabled` vs `GameDVR_FSEBehaviorMode`）
- VRR 使用虚构注册表路径
- 鼠标精准度读取不完整（仅读 `MouseSpeed`）
- `Module-level mutable refs` 反模式（移至 `useRef`）
- `Logs.tsx` 事件监听器过期闭包
- 构建目标 `all` → `nsis`
- `set_vrr_state` 命令未注册

---

## [v0.6.5] - 2026-07-28

### 发布

- NSIS 安装包构建（含 LhmBridge 完整依赖 + PawnIO 驱动）
- 便携版单文件 exe（需与 bridge/Drivers 同目录运行）

---

## [v0.6.4] - 2026-07-28

### 修复

- **修复 Release 二进制发行版读数问题** — 根因：`build.rs` 仅复制单层文件，遗漏 `lhm-bridge/bin/runtimes/` 平台特定程序集目录（含 `System.Threading.AccessControl.dll`、`System.Management.dll`、`System.IO.Ports.dll`），导致 .NET 运行时无法加载依赖 DLL
- `build.rs` 改为递归目录复制 `copy_dir_recursive()`
- `tauri.conf.json` 新增 `runtimes/win/lib/net8.0/` 下 3 个平台 DLL 资源条目

### 已知问题

- 使用 `cargo tauri build` 全链路构建（非独立 `cargo build --release`），前端正确嵌入，已解决 127.0.0.1 REFUSED 问题

---

## [v0.6.3] - 2026-07-28

### 修复

- **仪表盘读数恢复正常** — 根本原因：LhmBridge `OutputType=WinExe` 在 Rust pipe spawn 环境下 .NET 运行时初始化异常，导致 LibreHardwareMonitorLib `Computer.Open()` 静默失败
- **Bridge** `OutputType` 从 `WinExe` 改为 `Exe`（控制台应用），配合 Rust 侧 `CREATE_NO_WINDOW` 标志保证 .NET 正常初始化同时无弹窗
- **Bridge** 新增 `System.AccessViolationException` 捕获 + `HandleProcessCorruptedStateExceptions` runtimeconfig 防御 CSE 静默崩溃
- **Bridge** 移除 `AssemblyResolve` 事件处理器（无限递归栈溢出）
- **Rust** `build.rs` 新增 `rerun-if-changed` 指令确保 bridge 更新后自动重编译
- **Rust** dev profile `opt-level=0` 修复 MSVC `LNK2019` 链接错误
- **Rust** Bridge spawn 添加 `current_dir` + `creation_flags(CREATE_NO_WINDOW)`

### 变更

- `sensor.rs` 移除 WMI/PowerShell 回退路径，数据源统一为 LHM + PawnIO MSR + GlobalMemoryStatusEx
- `main.rs` dev 模式不静默提权，提示用户以管理员身份运行 CMD
- CPU Power 仅取 Package 传感器（不再累加每核 SMU）
- GPU VRAM 增加 `SmallData` 类型匹配

---

## [v0.6.2] - 2026-07-28

### 修复

- 修复 C# LhmBridge `Computer.Open()` 在非管理员令牌下触发 `AccessViolationException` (CSE) 导致进程静默崩溃
- 修复 `AssemblyResolve` 事件处理器无限递归导致栈溢出
- 修复 C# bridge 与 Rust spawn 环境不兼容（DLL 解析、工作目录）
- 修复 Rust dev profile `opt-level=2` 在 MSVC 工具链下产生 `LNK2019` 链接错误

### 变更

- bridge 工作目录设为自身 bin 目录以确保 .NET 正确解析依赖 DLL
- bridge 添加 `--debug` 诊断输出支持
- main.rs dev 模式不静默提权，提示用户以管理员身份打开终端
- sensor.rs 移除 WMI/PowerShell 回退路径，数据源统一为 LHM + PawnIO + 系统 API

---

## [v0.6.1] - 2026-07-28

### 修复

- 修复 lhm_bridge 5 处非关键路径 Mutex 裸 `lock().unwrap()`（中毒时级联 panic）
- 修复 useVersion Promise 静默吞错（后端断连无提示）
- 修复 tauri-plugin-fs 仅前端注册导致日志导出静默失败（新增 Rust 后端注册 + capability）
- 修复 Dashboard GPU/磁盘列表数组索引 React key（实时列表 DOM 错位风险）
- 修复 Cleanup 按钮 loading 状态文本与 label 不匹配
- 修复 Settings silent empty catch（refreshPowerPlans 无诊断日志）

### 变更

- 所有 `catch (e)` 改为 `catch (e: unknown)` 类型标注（12 处）
- 所有 `console.error` 添加 `import.meta.env.DEV` 守卫
- Dashboard SVG 渐变硬编码颜色改为 CSS 变量
- index.css 梯度文字颜色改用 CSS 变量，新增 `--color-brand-yellow/purple/pink`
- build.rs `to_str().unwrap()` 改为 `to_string_lossy()`
- main.rs `current_exe().expect()` 改为 `unwrap_or_else`
- sensor.rs 移除 dead function `wmic_f32`，修复 `SYS_INSTANCE` Mutex unwrap

### 清理

- 消除 10 个 Cargo 编译警告（unused variables、dead code、unnecessary mut）
- About.tsx 移除不存在的 simplelog 引用
- Vite build 分离 recharts 到独立 chunk（主 chunk 828 kB → 421 kB）

---

## [v0.6.0] - 2026-07-27

### 修复

- 修复 LhmBridge.exe 启动时出现命令提示符窗口（添加 CREATE_NO_WINDOW 标志）
- 修复 dev 模式仪表盘无数据（CSP 缺少 `https://ipc.localhost` 导致 IPC 被拦截）

### 变更

- 仪表盘卡片按类别重组（CPU / GPU / 内存 / 存储 四卡片各自包含完整同类数据）
- MiniStat 图标实时摘要移至仪表盘底部

---

## [v0.6.0-alpha.1] - 2026-07-24

### 新增 (P5)

- 日志系统: Tauri 事件推送 (`log-event`) 替代前端轮询
- 日志页面: 实时监听, 事件推送模式标识

### 变更

- Debug 日志: 被动轮询 → 主动推送
- LogBuffer: 新增 `app_handle` 字段

---

## [v0.5.0-alpha.1] - 2026-07-24

### 新增 (P4 — 系统设置模块)

- 注册表读写 (HKCU/HKLM, winreg)
- HAGS / 游戏优化 / 鼠标精准度 / VRR 开关
- 电源计划枚举 + 切换 + 卓越性能导入
- 16 项 Windows 服务管理

---

## [v0.4.0-alpha.1] - 2026-07-24

### 新增 (P3 — 网络诊断模块)

- Ping / Traceroute / Nslookup / STUN NAT 类型检测
- 15 个预设 STUN 服务器, 三阶段测试: 绑定/映射行为/过滤行为

---

## [v0.3.0-alpha.1] - 2026-07-24

### 新增 (P2 — 清理优化模块)

- DNS 刷新, GPU 着色器缓存清理, 临时文件清理, 待机内存释放, 进程优先级管理

---

## [v0.2.0-alpha.1] - 2026-07-24

### 新增 (P1 — 硬件监控模块)

- CPU/GPU/内存/存储监控, Recharts 折线图 (60 秒历史)

---

## [v0.1.0-alpha.1] - 2026-07-24

### 新增 (P0 — 基础骨架)

- Tauri 2.x + React + TypeScript + Vite 工程初始化
- 环形缓冲区日志 + 文件持久化
- 系统托盘, 6 路由页面, 调试日志页面
