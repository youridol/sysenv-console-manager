# 变更契约：三栏布局 + 日志按页输出 + 移除 AI 环境

> 本文件冻结三个并行工作流之间的接口。双方不得单方面改名或增删字段。

## A. 三栏布局

```
┌──────────┬────────────────────────────┬──────────────────┐
│ 左导航    │ 主内容区（Outlet，可滚动）    │ 右侧栏（日志卡片） │
│ Sidebar  │ SidebarInset                │ RightLogPanel    │
└──────────┴────────────────────────────┴──────────────────┘
```

- 左导航：现有 `AppSidebar` 保持不变（仅移除两个导航项，见 §C）。
- 右侧栏：**新增** `src/components/RightLogPanel.tsx`（Lead 负责）。
  - 宽度固定 `22rem`（`w-88`），`h-svh`，独立滚动。
  - 可折叠：折叠后只留一条带图标的竖条（宽度 `2.5rem`）；展开/折叠状态存 `localStorage`
    键 `secm:log-panel-collapsed`（`"1"` = 折叠）。
  - 内容 = `<LogsCard compact />`。
- 调试日志页面（`/logs` 路由）**移除**，其内容改为右侧栏卡片（见 §C）。
- 响应式：主内容区 `flex-1 min-w-0`，右侧栏 `shrink-0`；窄窗口（< 1100px）时右侧栏
  默认折叠（不自动改 localStorage，仅初始值判断）。

## B. 日志输出模式（全部输出 | 按页输出）

### B.1 滑动开关组（Segmented）

`LogsCard` 顶部新增二选一开关组，文案固定：

| 值 | 文案 | 语义 |
| --- | --- | --- |
| `all` | 全部输出 | **默认**。行为与现状完全一致（输出全部日志） |
| `page` | 按页输出 | 仅输出当前所在页面的操作与行为 |

- 用现有 `src/components/ui/toggle-group.tsx`（shadcn ToggleGroup，单选）实现，
  视觉为「滑动」分段控件；两段等宽、紧凑，置于工具栏首行。
- 选择持久化到 `localStorage` 键 `secm:log-scope-mode`（`"all"` | `"page"`），
  默认 `all`（缺失/非法值一律回落 `all`）。

### B.2 日志条目新增字段 `scope`

前后端都要给每条日志打**来源页面**标签。字段名 `scope`，类型 `string`。

冻结取值集合（与路由一一对应，移除 AI 环境后不含 `ai-env`）：

```
dashboard | cleanup | network | net-config | settings | services
| environment | hardware | logs | about | system
```

- `system` = 不属于任何页面的全局/启动/后台线程日志（启动、托盘、sidecar、诊断写入等）。
  在「按页输出」模式下，`system` 条目**仅在**当前页面无对应 scope 时… **不显示**
  （即严格按页；全局信息请切回「全部输出」）。
- 未知/缺失 scope 的条目按 `system` 处理（`#[serde(default)]` + 前端 `?? "system"`）。

### B.3 Rust 侧接口（冻结）

`src-tauri/src/log.rs`：

```rust
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
    /// 来源页面 scope（见 §B.2）；缺失时按 "system" 处理
    #[serde(default = "default_scope")]
    pub scope: String,
}

pub fn default_scope() -> String { "system".into() }

impl LogBuffer {
    /// 兼容旧调用：等价于 append_scoped(level, msg, "system")
    pub fn append(&self, level: &str, msg: &str);
    /// 带来源页面的写入（文件日志同步写入 scope：[ts] [level] [scope] msg）
    pub fn append_scoped(&self, level: &str, msg: &str, scope: &str);
}

/// 命令标签 → 页面 scope（唯一真源，供命令日志宏使用）
pub fn scope_for_command(label: &str) -> &'static str;
```

- 文件日志行格式升级为 `[时间] [级别] [scope] 消息`（`log.rs:60`）。
  旧格式行仍是合法文本，导出/查看不受影响。
- `scope_for_command` 为**穷尽 match**，未命中返回 `"system"`；
  加 `#[cfg(test)]` 覆盖全部已登记命令标签与未知标签回落。

`commands.rs` 日志宏（`cmd_log!` / `cmd_result!` / `cmd_val!`）改为
`append_scoped(..., crate::log::scope_for_command($label))`，宏签名不变。

其他 `append` 调用点（`lib.rs` 5 处、`lhm.rs` 1 处、`temp_data.rs` 2 处、
`driver_install/mod.rs` 1 处）逐个显式传入正确 scope（启动/托盘/sidecar/诊断 → `system`）。

### B.4 前端侧接口（冻结）

`src/lib/logs.ts`（**新增**，Lead 与 logs-card 共同依赖的唯一真源）：

```ts
export type LogScopeMode = "all" | "page";
export type LogScope =
  | "dashboard" | "cleanup" | "network" | "net-config" | "settings"
  | "services" | "environment" | "hardware" | "logs" | "about" | "system";

export const LOG_SCOPE_MODE_KEY = "secm:log-scope-mode";

/** 路由 pathname → scope（"/" → dashboard；未知 → system） */
export function scopeForPath(pathname: string): LogScope;

/** 前端 logger 的 module 名 → scope（"Dashboard" → dashboard；未知 → system） */
export function scopeForModule(module: string): LogScope;

export interface LogEntry {
  timestamp: string;
  level: string;
  message: string;
  /** 后端 v3.1 起提供；旧条目缺失时按 "system" 处理 */
  scope?: string;
}
```

`LogsCard` 内过滤逻辑：

```
level 过滤 → search 过滤 → （mode === "page" ? scope === scopeForPath(location.pathname) : 不过滤）
```

### B.5 `LogsCard` 组件契约（冻结）

`src/components/logs/LogsCard.tsx`：

```tsx
export interface LogsCardProps {
  /** 紧凑布局（右侧栏卡片态）：更小字号/行高、工具栏可换行 */
  compact?: boolean;
  className?: string;
}
export default function LogsCard(props: LogsCardProps): React.ReactElement;
```

行为与现有 `Logs.tsx` **完全等价**，仅新增输出模式开关组与 scope 过滤：
实时 `log-event` 订阅、级别筛选（Info/Warn/Error/Debug + 计数）、容量 Select
（100/200/500/1000，初值取自 `get_log_capacity`，失败回落 200）、搜索、
自动滚动、刷新、导出（`export_logs` + `save` + `writeTextFile`）、清空、加载骨架、
空态（加载失败 / 无匹配 / 暂无日志）。

## C. 移除 AI 环境（全链路）

### 后端
- `environment.rs`：删除 §AI 段（`AI_TOOLS`、`AiTool`、`AiToolsInfo`、`check_ai_tools`、
  `fetch_ai_latest_versions`、`fetch_npm_latest`、`npm_global_prefix`、
  `install_or_upgrade_tool`、`NpmEnvironment`、`list_global_packages`、
  `check_npm_environment`、`run_version_cmd`、`uninstall_ai_tool`、`MCP_SERVERS`、
  `McpServerInfo`、`list_mcp_servers`、`is_whitelisted_mcp_package`、
  `install_mcp_server`、`uninstall_mcp_server`、`AiExtension`、`EXTENSION_DIRS`、
  `list_extensions`、`read_extension_description`）及其专用 helper
  （`find_executable`、`suppress_windows_error_dialogs`、`run_command_silent`、
  `try_get_version`、`npm_list_global_version`、`clean_version`、`detect_cli_tool`、
  `is_whitelisted_npm_package`）与相关 `#[cfg(test)]`。
- **保留**：`check_directx` / `check_vc_runtimes` 及其全部依赖（`DxCheck`、
  `DirectXInfo`、`VcRuntimeInfo`、`VcRuntime`、`EXPECTED_VC`、`infer_arch`、
  `extract_vc_name`、`vc_version_order`、`detect_gpu_names`、
  `read_dx_version`、`check_d3d11/12_available`、`check_wddm_2`）。
- `commands.rs`：删除 `check_ai_tools`、`install_ai_tool`、`fetch_ai_latest_versions`、
  `check_npm_environment`、`uninstall_ai_tool`、`list_mcp_servers`、
  `install_mcp_server`、`uninstall_mcp_server`、`list_extensions`。
- `lib.rs`：删除上述 9 个 `generate_handler!` 注册项。

### 前端
- 删除 `src/pages/AiEnvironment.tsx`（576 行）。
- `App.tsx`：删除 `import AiEnvironment` 与 `/ai-env` 路由；同时删除 `/logs` 路由（§A）。
- `AppSidebar.tsx`：删除「AI 环境」与「调试日志」两个导航项；移除随之不再使用的
  `Bot`、`ScrollText` 图标导入。
- `environment-shared.tsx`：保留（`Environment.tsx` 仍使用）。
- 清理：全仓 grep `ai-env|AiEnvironment|AI 环境|check_ai_tools|install_ai_tool|list_mcp_servers|list_extensions|check_npm_environment|uninstall_ai_tool` 不得有残留
  （`CHANGELOG.md` 历史条目除外——那是发布记录，不改）。

## D. 验收

1. `npm run build` / `npm run lint` 退出 0。
2. `cd src-tauri; cargo check --lib --tests` 退出 0；lib 单测全绿（既有 1 个
   `environment::tests::clean_version_variants` 属 AI 段，随移除一并消失）。
3. `cargo test --test hardware_contract` 等 4 个集成测试全绿。
4. 运行态：三栏可见；右侧栏日志卡片实时滚动；切换「按页输出」后仅显示当前页面日志，
   切回「全部输出」恢复全量；右侧栏可折叠且刷新后保持。
5. AI 环境页与导航项均不存在；9 个 AI 命令已从 handler 注销（invoke 报 command not found）。
