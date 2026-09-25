# AGENTS.md — SysEnv Console Manager (SECM)

面向 AI Agent 的项目导航。只写从仓库里查不到、或查起来昂贵的信息；细节按文末指针下钻。

## 项目概述

Windows 10/11 x64 系统环境管理桌面工具：硬件监控、清理优化、网络诊断、系统设置、环境检测。

- **技术栈**：Tauri 2（Rust edition 2021）+ React 19 + Vite 6 + TypeScript 5.7 + Tailwind CSS 4 + shadcn/ui。
- **入口**：`src-tauri/src/main.rs` → `secm_lib::run()`（`src-tauri/src/lib.rs`）；`run()` 负责运行目录初始化、单实例插件、托盘、启动守卫、命令注册。
- **前端入口**：`src/main.tsx` → `src/App.tsx`（React Router，9 条路由）。
- **稳定标识**：productName `SysEnv Console Manager`，identifier `com.secm.app`，主窗口 label `main`，Rust lib crate `secm_lib`。
- **IPC 面**：78 条 `#[tauri::command]`（实现集中在 `src-tauri/src/commands.rs`，注册在 `lib.rs::invoke_handler`）。

### 路由 → 页面 → 日志 scope

| 路由 | 页面文件 | 日志 scope |
| --- | --- | --- |
| `/` | `src/pages/Dashboard.tsx` | `dashboard` |
| `/cleanup` | `src/pages/Cleanup.tsx` | `cleanup` |
| `/network` | `src/pages/Network.tsx` | `network` |
| `/net-config` | `src/pages/NetConfig.tsx` | `net-config` |
| `/settings` | `src/pages/Settings.tsx` | `settings` |
| `/services` | `src/pages/Services.tsx` | `services` |
| `/environment` | `src/pages/Environment.tsx` | `environment` |
| `/hardware` | `src/pages/Hardware.tsx` | `hardware` |
| `/about` | `src/pages/About.tsx` | `about` |
| 全局 / 启动 / 托盘 / 后台 | — | `system` |

`/logs` 路由已移除：日志并入主界面右侧栏（`src/components/RightLogPanel.tsx` + `src/components/logs/LogsCard.tsx`）。scope 取值集合中仍保留 `logs`。

### 数据来源三条链路

1. **Win32 / 注册表 / HTTP 采集** —— `datasource/`（独立 crate `secm-datasource`）：注册表、SCM 服务、powrprof 电源、iphlpapi 网络、dnsapi、ureq 公网 IP、激活状态、磁盘 / SMART IOCTL。业务模块经 `lib.rs` 的 `pub use secm_datasource as datasource` 引用。
2. **硬件传感器** —— `sidecar-lhm/`（.NET 8 独立进程，HTTP `127.0.0.1:45980`）。`src-tauri/src/sensor.rs` 只做快照投影，不直接探测驱动。
3. **ring0 驱动** —— PawnIO（WHQL 签名）。由 `src-tauri/src/driver_install/pawnio.rs` 以 SCM 服务方式部署，供 sidecar 的 tier-2 使用。

**两级权限模型**（权威定义见 `sidecar-lhm/CONTRACT-v3.md` §1）：tier-1 普通用户即时可用（PDH 性能计数器 / NetworkInterface / DriveInfo / 电池 / GPU 厂商用户态通道）；tier-2 需一次 UAC（CPU 温度/功耗/电压、主板 SuperIO、磁盘温度）。未提权时 tier-2 字段为 `null` 且 `*_source="none"`，**绝不伪造 0**。主进程自身以普通权限启动，无启动期 UAC。

**提权两步流程**（进入硬件信息页触发）：先调 `auto_elevate` → `POST /api/v1/elevate/auto`，只走「本进程已是管理员」与「已注册持久授权 `schtasks /Run`」两条**零 UAC** 路径；仅当它返回 `false`（确认为非管理员且从未授权）时才调 `request_elevation` 申请一次 UAC。被拒后 sidecar 记住该状态，本会话不再重复弹窗。

## 构建命令

前置：Rust（edition 2021 + `x86_64-pc-windows-msvc` target）、Node 20+、Visual C++ Build Tools、.NET 8 SDK、Windows 10/11 SDK（提供 `mt.exe`）、Git Bash（脚本依赖）。

```bash
npm install                 # 前端依赖
npm run tauri dev           # 开发模式（Vite + Rust 热重载）
npm run build               # tsc -b + vite build（前端类型检查 + 生产构建）
npm run lint                # ESLint
npm run format              # Prettier

cd src-tauri && cargo build --release
cd datasource && cargo check --all-targets     # 要求零警告

# 随包资源：构建前必须就绪，缺失即非零退出（防止产出残废包）
npm run lhm:publish         # dotnet publish -c Release -r win-x64 --self-contained
npm run stage:driver        # PawnIO 三件套 + GPL 源码
npm run stage:iperf3        # iperf3.exe + cygwin1.dll + 许可（SHA256 门禁）
npm run stage:lhm           # sidecar publish 产物 + 许可 + 源码

npm run tauri:build         # 一键：stage 全部 + 驱动签名审计 + NSIS
npm run package:portable    # 便携 zip + 发行归档（末尾自动跑 fix:integrity）
```

`dev` / `build` / `preview` 均经 `scripts/run-with-project-tmp.mjs` 包装为工具设置 `TEMP`/`TMP`。**不要绕过包装脚本直接调 `vite`**（原因见「安全注意」）。

## 发布版本

- **版本三处同步**（不一致视为构建失败）：`package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`。当前 `2.1.0`。
- **版本类型判定**：PATCH=修复 / 格式 / 依赖 / 回退；MINOR=新功能 / 命令 / 组件；MAJOR=破坏性变更 / 重构。
- **CHANGELOG**：`CHANGELOG.md` 顶部新增 `## [vX.Y.Z] - YYYY-MM-DD`，中文记录修复、新增、变更。既有条目普遍采用「问题定位 / 关键日志证据 / 解决方案设计 / 验证结果」四段式，新条目沿用。
- **产物**：`dist_releases/v<版本>/` 是唯一权威交付位置（安装版 setup.exe、便携版 exe、便携 zip、`resources/driver`、`resources/lhm`、`SHA256SUMS.txt`）。`src-tauri/target/` 是中间产物，可随时清理重建。
- **收尾门禁**：打包后必须执行 `npm run fix:integrity`（`package:portable` 已自动调用），把交付物的 NTFS 完整性标签归一化为 Medium 并**回读校验**。标签未修复时 NSIS 安装包会以低完整性运行并报 `Error writing temporary file`。
- 禁止自动 `git push`、创建 Release、改远程分支或 remote。

## 代码风格

通用：注释中文，identifier 保留英文；不做与当前任务无关的重构与格式化。

**Rust**（`src-tauri/`、`datasource/`，edition 2021，`rustfmt --edition 2021`）

- 命令层错误边界统一 `Result<T, String>` + 中文错误消息；采集层返回 `Result<T, CollectError>`，由 `src-tauri/src/ds_util.rs` 映射为中文消息与含 API 名/操作名的 R8 日志。
- fast-fail：错误在边界立即返回；不吞异常、不做无依据 fallback、不伪造数据。
- 命令脚手架统一用 `src-tauri/src/commands.rs` 的 `cmd_result!` / `cmd_val!` / `cmd_pure!` / `cmd_log!` 宏，不手写重复的计时与日志。
- 日志两套：用户可见日志走 `state.log_buffer.append_scoped(level, msg, scope)`；开发者诊断走 `debug_info!` / `debug_warn!` / `debug_error!` / `debug_trace!(module, fmt, ...)`，落运行目录 `diag/debug.log`。
- **新增命令必须在 `src-tauri/src/log.rs` 的 `scope_for_command` 登记**：该函数是「命令标签 → 页面 scope」的唯一真源，`#[cfg(test)]` 用例会扫描 `commands.rs` 的真实调用点并断言集合完全一致，漏登记即测试失败。

**TypeScript / React**（`src/`，`tsconfig.json` 开启 strict + `noUnusedLocals` + `noUnusedParameters`）

- 路径别名 `@/*` → `src/*`；Tauri 调用 `import { invoke } from "@tauri-apps/api/core"`，流式用 `Channel`（封装在 `src/hooks/useStreaming.ts`）。
- UI 原语放 `src/components/ui/`（shadcn/ui，new-york 风格）；新增组件前先查该目录是否已有同类。
- 有意忽略的未使用变量以 `_` 前缀命名（ESLint `argsIgnorePattern: "^_"`）。

**测试命名**：Rust 测试函数名一律中文（例：`fn 版本解析_标准格式()`）。`src-tauri/src/iperf_tests.rs` 因中文名夹带英文缩写而模块级豁免 `non_snake_case`，其头部注释说明了原因。

## 安全注意

- **运行数据落点**由 `src-tauri/src/runtime_paths.rs` 单点定义，禁止在其他模块拼路径：debug 构建 → `<项目根>/build/runtime`；release → 优先可写的 `<exe 同级>/runtime`，不可写时回退 `%APPDATA%\SECM`；测试与排障用环境变量 `SECM_RUNTIME_DIR` 注入。
- **WebView2 User Data 目录保持 `%LOCALAPPDATA%\com.secm.app`**（Tauri 默认），不要移入项目目录：实测项目目录承载带低完整性标签的受限 ACL，移入会使外壳交互（托盘登记）失败。
- **本仓库工作区目录带 `Mandatory Label\Low Mandatory Level:(OI)(CI)(NW)` 标签**（`icacls .` 可复验），从该目录启动的进程即低完整性进程，写系统 `%TEMP%` 会被拒绝（OS 错误 5 拒绝访问）。构建、测试、运行数据统一重定向到 `build/tmp/` 与 `build/runtime/`；这是包装脚本存在的理由，不要绕过，也不要通过改 ACL、降权限或关沙箱来绕过。
- **权限**：应用以普通用户启动，启动期不弹 UAC；进入硬件信息页自动尝试零 UAC 提权（管理员直通 / 已注册持久授权），自动路径不可行才申请一次 UAC；清理、服务管理等命令各自检查权限并返回中文提示。
- **驱动**：PawnIO 是唯一随包 ring0 驱动，`npm run audit:drivers` 是构建门禁。`third_party/WinRing0x64.sys` 仅作署名/存档，不再 stage 进安装包。
- **不提交**：`build/`、`tmp/`、`dist_releases/`、`src-tauri/target/`、`src-tauri/resources/{driver,lhm,iperf3}/`（stage 产物）、`sidecar-lhm/{bin,obj}/`、`*.zip`、`*.bak`。完整清单见 `.gitignore`。
- CSP 与 `additionalBrowserArgs` 收窄在 `src-tauri/tauri.conf.json`，放宽需有明确理由。

## 测试要求

| 范围 | 命令 | 说明 |
| --- | --- | --- |
| src-tauri 单元测试 | `bash scripts/test-lib.sh` | 源码含 249 处 `#[test]`/`#[tokio::test]`；脚本编译后用 SDK 的 `mt.exe` 注入 comctl32 v6 manifest，并把 `TEMP`/`TMP`/`APPDATA`/`SECM_RUNTIME_DIR` 固定到 `build/tmp/`。**必须用本脚本**：直接 `cargo test` 会重编译并抹掉注入的 manifest，测试二进制以 `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)` 崩溃 |
| 单个集成测试 | `cd src-tauri && cargo test --test <名>` | `src-tauri/tests/` 下 19 个测试目标；夹具根目录由 `tests/common/mod.rs` 固定为 `build/tmp/fixtures` |
| 采集 crate | `cd datasource && cargo test && cargo check --all-targets` | 源码含 48 处 `#[test]`，其中 1 个 `#[ignore]` 真机项；`check` 要求零警告 |
| LHM sidecar | `cd sidecar-lhm/tests/LhmSidecar.Tests && dotnet test -c Release` | xUnit（274 用例），覆盖映射规则、契约、提权、tier-2 快照并发与合并语义 |
| 前端 | 无测试框架 | 仓库内无 vitest/jest 与 `*.test.tsx`；前端改动以 `npm run build`（tsc）与 `npm run lint` 为门禁 |

- 干净环境或缺 Windows SDK 时用 `REQUIRE_MANIFEST=1 bash scripts/test-lib.sh`：缺 `mt.exe` 时脚本明确失败而非静默降级。缺 manifest 的测试二进制会以 `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)` 崩溃，此时测试结论不可信。
- 改动数据源、契约或降级语义时，必须同时满足 `hardware_contract`（sidecar 缺失时的降级与「不伪造 0」）与 `datasource_native`。
- `iperf3_live` / `dhcp_probe_live` / `probe_url_live` 依赖真实网络，无网络时失败属预期；报失败须连同命令与原始输出。
- `scripts/test-lib.sh` 采用 `set -euo pipefail` 且测试阶段不经管道：**测试失败时脚本以非零码退出**，并回显日志末尾。若看到退出码为 0 但有失败用例，属于运行器回归，必须先修运行器。
- 新增「跨文件/跨进程契约」时，优先按仓库既有模式补**源码守卫测试**（如 `stream_cancel_registration_contract.rs`、`startup_guard_contract.rs`）：断言源码中必须同时存在的调用点与调用顺序，不启动 GUI、不依赖真实硬件。
- 涉及 tier-2（LHM ring0）改动时，除 `hardware_contract` 外还需保证 sidecar 的 `Tier2SnapshotTests` 通过：它锁定「HTTP 线程不得进入采集路径」这一单写者不变量。
- 涉及代码、配置、依赖、构建或运行行为的改动必须实际运行验证；仅文档、注释、`AGENTS.md` 变更不要求运行测试。

## 目录说明

| 路径 | 内容 |
| --- | --- |
| `src/` | React 前端源码：`pages/` `components/` `components/ui/` `hooks/` `contexts/` `layouts/` `lib/` |
| `src-tauri/src/` | Rust 后端 28 个模块（另有 `main.rs` 入口与 `iperf_tests.rs` 测试模块）；关键：`commands.rs`、`sensor.rs`、`lhm.rs`、`runtime_paths.rs` |
| `src-tauri/tests/` | 集成测试（契约 / 降级 / 真机） |
| `src-tauri/capabilities/` | Tauri v2 权限配置（`default.json`） |
| `src-tauri/resources/` | stage 产物 `driver/` `lhm/` `iperf3/`，不入库 |
| `datasource/` | 独立 crate `secm-datasource`：`registry` `service` `power` `netif` `dns` `http` `activation` `disk` `error` |
| `sidecar-lhm/` | .NET 8 sidecar：`Snapshot.cs`（契约 DTO）、`Hardware/`（映射 / 规则 / 采集）、`Elevation/`、`tests/` |
| `third_party/` | 原始第三方产物：PawnIO 2.2.0 三件套 + 源码、iperf3 二进制 + 许可、WinRing0 存档 |
| `scripts/` | stage、签名与许可审计、打包、测试运行器、临时目录包装脚本 |
| `dist_releases/` | 发行归档，不入库，仅本地保留 |
| `build/` `tmp/` | 构建产物与临时产物；`build/runtime/` 为开发期运行数据 |
| `CHANGE-CONTRACT.md` `CHANGE-CONTRACT-v2.md` `CHANGE-CONTRACT-v3.md` | 冻结的跨工作流改动契约（三栏布局与日志 scope / 数据源切换 / 硬件页加载流程） |
| `docs/adr/` | 架构决策记录（ADR，MADR 精简形式）；索引见 `docs/adr/README.md`。只写决策与理由，证据留在审查报告与 CHANGELOG |
| `docs/plans/` | 与 ADR 配套的任务分解与验收判据 |

## 参考文档（按需下钻）

- `README.md` —— 功能矩阵（9 页面 / 78 命令）、完整架构图、故障排查表、数据与日志存储、第三方许可清单。
- `CHANGELOG.md` —— 逐版本的根因与实测证据（当前 `v2.1.0`）。
- `sidecar-lhm/CONTRACT-v3.md` —— Rust ⇄ C# 冻结接口契约与两级权限模型。
- `src-tauri/src/startup_guard.rs` —— 主窗口（WebView2）创建失败时的看门狗与用户提示设计。
- `eslint.config.js` —— 前端检查面的有意取舍（未启用 React Compiler 规则的原因写在注释里，改规则前先读）。
- `scripts/test-lib.sh` —— 单测运行器为何注入 manifest、为何重定向临时目录、为何测试阶段不经管道。
- `docs/adr/ADR-0001-审查缺陷修复基线.md` —— 2026-09-24 全量审查 16 项决策的背景、理由与被否决方案。

## 常见任务

- **新增 Tauri 命令**：`commands.rs` 实现（优先复用命令宏）→ `lib.rs::invoke_handler` 注册 → `log::scope_for_command` 登记标签 → 前端 `invoke` 调用 → 补测试。
- **新增页面**：`src/pages/` 建页面 → `App.tsx` 加路由 → `src/components/AppSidebar.tsx` 加导航项 → 与 scope 取值集合保持一致。
- **改造数据源**：`datasource/` 实现 → `ds_util.rs` 接错误映射 → 维护 `--features legacy-data-source` 回滚开关语义 → 补契约测试。
- **升级依赖**：注意 `wmi` 与 tauri 的 `windows-core` 版本漂移；冲突时 `cd src-tauri && cargo update --precise 0.61.2` 对齐后重建。
- **发版**：三处版本同步 → `CHANGELOG.md` 加条目 → `npm run tauri:build` → `npm run package:portable` → 核对 `dist_releases/v<版本>/` 与 `SHA256SUMS.txt` → `git status` 确认无产物入库。
- **排障**：先看运行目录下 `diag/debug.log`（模块级，含 API 名与操作名）与 `logs/<日期>.log`；sidecar 输出在 `logs/secm-lhm-sidecar.log`。
