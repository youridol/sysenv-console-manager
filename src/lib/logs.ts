// 日志来源页面（scope）——前端侧的路由映射与冻结类型。
//
// 设计：
//   - 后端 v3.1 起每条日志带 `scope` 字段（缺失/未知按 "system" 处理）；
//   - 路由 pathname → scope 由 `scopeForPath` 解析（"按页输出" 模式使用）；
//   - 输出模式（全部 / 按页）持久化键为 `LOG_SCOPE_MODE_KEY`。
//
// **module → scope 的唯一真源在 Rust 侧** `src-tauri/src/log.rs::scope_for_module`：
// 前端 `logger` 只把 module 字符串随 `append_frontend_log` 发给后端，由后端解析 scope
// 并写入日志条目。本文件此前另存一份 `MODULE_TO_SCOPE`/`normalizeModule`/`scopeForModule`，
// 经全局检索确认**零调用点**（从未执行），却与后端那套并行维护，属必然漂移的第二真源；
// ADR-0001 D10 已将其删除。新增页面时只需改后端 `scope_for_module`（本文件仅需在
// `LogScope` 中补齐取值，若该页面有独立路由则同时补 `PATH_TO_SCOPE`）。
//
// 注意：`LogScope` 类型与 `scopeForPath`、`LOG_SCOPE_MODE_KEY` 是与其它工作流共享的
// 冻结接口，禁止单方面改名或增删导出。

/** 日志输出模式：all = 全部输出（默认），page = 按页输出 */
export type LogScopeMode = "all" | "page";

/** 来源页面 scope（与路由一一对应；system = 全局/启动/后台线程日志） */
export type LogScope =
  | "dashboard"
  | "cleanup"
  | "network"
  | "net-config"
  | "settings"
  | "services"
  | "environment"
  | "hardware"
  | "logs"
  | "about"
  | "system";

/** localStorage 键：日志输出模式（"all" | "page"） */
export const LOG_SCOPE_MODE_KEY = "secm:log-scope-mode";

/** 路由 pathname → scope 映射表（键统一为小写、无尾斜杠） */
const PATH_TO_SCOPE: Record<string, LogScope> = {
  "/": "dashboard",
  "/cleanup": "cleanup",
  "/network": "network",
  "/net-config": "net-config",
  "/settings": "settings",
  "/services": "services",
  "/environment": "environment",
  "/hardware": "hardware",
  // `/logs` 路由已随右侧栏改版移除；保留映射以兼容冻结取值集合与历史条目
  "/logs": "logs",
  "/about": "about",
};

/**
 * 路由 pathname → scope。
 * "/" → dashboard；未知路径 → system。
 * 兼容尾斜杠（"/cleanup/"）、查询串/哈希与大小写差异。
 */
export function scopeForPath(pathname: string): LogScope {
  if (!pathname) return "system";
  const withoutQuery = pathname.split("?")[0].split("#")[0];
  const normalized =
    withoutQuery.length > 1 ? withoutQuery.replace(/\/+$/, "") : withoutQuery;
  return PATH_TO_SCOPE[normalized.toLowerCase()] ?? "system";
}

// 此处曾有 `MODULE_TO_SCOPE` / `normalizeModule` / `scopeForModule` 三份实现，
// 已随 ADR-0001 D10 删除：全局检索确认零调用点，实际解析由后端
// `log.rs::scope_for_module` 完成（对应测试 `test_scope_for_module_*`）。

/** 单条日志条目（后端 `get_logs` / `log-event` 载荷） */
export interface LogEntry {
  timestamp: string;
  level: string;
  message: string;
  /** 后端 v3.1 起提供；旧条目缺失时按 "system" 处理 */
  scope?: string;
}
