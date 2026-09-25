// 统一前端日志模块（前后端 debug.log 统一出口）
//
// 设计：
//   - 统一格式：时间戳 + 级别 + 模块名 + 消息（与后端 debug.rs 条目对齐）
//   - 输出：DEV 下 console 全量输出；生产仅 WARN/ERROR 输出 console；
//     WARN/ERROR 同时经 append_frontend_log 上报后端 → 写入运行目录 diag/debug.log
//   - 用法：logger.info("Dashboard", "数据加载完成", {count: 3})
import { invoke } from "@tauri-apps/api/core";

export type LogLevel = "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR";

const IS_DEV = import.meta.env.DEV;

/** 上报后端节流：同一模块+级别 5s 内最多 1 条（防高频错误刷爆日志） */
const BACKEND_THROTTLE_MS = 5000;
const lastSent = new Map<string, number>();

function formatArgs(args: unknown[]): string {
  return args
    .map((a) => {
      if (typeof a === "string") return a;
      try {
        return JSON.stringify(a);
      } catch {
        return String(a);
      }
    })
    .join(" ");
}

function timestamp(): string {
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${String(d.getMilliseconds()).padStart(3, "0")}`;
}

/**
 * 上报后端（带节流；失败静默不影响前端）。
 *
 * 双通道语义（对应后端 `append_frontend_log`）：
 *   - WARN/ERROR → 开发者 `debug.log` + 用户日志缓冲（右侧栏「调试日志」卡片）
 *   - INFO       → 仅用户日志缓冲
 *
 * INFO 也上报的原因：页面级的**用户操作结果**（如网络诊断的网站/端口探测可达性、
 * 已安装工具的卸载成功）本身就是「按页输出」应当展示的"操作与行为"。若只上报
 * WARN/ERROR，成功路径在按页视图里会完全静默，与该模式的设计目标相悖。
 * 当前全仓 INFO 调用仅 3 处且都在用户显式触发路径上，无高频轮询风险；
 * 下方节流仍按 `级别:模块` 5s 生效，防止异常情况下刷屏。
 * TRACE/DEBUG 不上报（开发者噪音，无用户价值）。
 */
function reportToBackend(level: LogLevel, module: string, message: string) {
  if (level !== "INFO" && level !== "WARN" && level !== "ERROR") return;
  const key = `${level}:${module}`;
  const now = Date.now();
  const last = lastSent.get(key) ?? 0;
  if (now - last < BACKEND_THROTTLE_MS) return;
  lastSent.set(key, now);
  postToBackend(level, module, message);
}

/** 实际 IPC 上报（不含节流判定；长度上限与后端 `append_frontend_log` 对齐） */
function postToBackend(level: LogLevel, module: string, message: string) {
  invoke("append_frontend_log", {
    level,
    module: module.slice(0, 64),
    message: message.slice(0, 4096),
  }).catch(() => {
    // 上报失败静默（如后端未就绪）
  });
}

/** 统一 console 输出：DEV 全量；生产仅 WARN/ERROR（减少控制台噪音） */
function emitConsole(level: LogLevel, module: string, msg: string) {
  if (!IS_DEV && level !== "WARN" && level !== "ERROR") return;
  const line = `[${timestamp()}] [${level}] [${module}] ${msg}`;
  const fn =
    level === "ERROR"
      ? console.error
      : level === "WARN"
        ? console.warn
        : console.debug;
  fn(line);
}

function log(level: LogLevel, module: string, ...args: unknown[]) {
  const msg = formatArgs(args);
  emitConsole(level, module, msg);
  reportToBackend(level, module, msg);
}

/**
 * 用户显式触发的操作结果上报（**不节流**，每次调用必定上报一条）。
 *
 * 为什么必须独立于 `log`：`reportToBackend` 以 `级别:模块` 为键做 5s 节流，
 * 用于抑制异常情况下的高频错误刷屏。但用户操作结果（例如在清理页连续点击
 * 两个清理按钮，两次都是 INFO + "Cleanup"）与异常刷屏性质不同：它们天然低频
 * （受按钮 disabled + 二次确认约束），且每一条都对应一次真实执行，漏一条就是
 * 「按页输出」里少一次操作记录。走同一节流键会让 5s 内的第二条被**静默丢弃**。
 * 故此入口不做节流判定，只保留长度截断；正常 info/warn/error 的节流语义不变。
 *
 * 仅允许 INFO/WARN/ERROR（TRACE/DEBUG 不上报后端，与 `log` 一致）。
 */
function logAction(level: LogLevel, module: string, ...args: unknown[]) {
  if (level !== "INFO" && level !== "WARN" && level !== "ERROR") return;
  const msg = formatArgs(args);
  emitConsole(level, module, msg);
  postToBackend(level, module, msg);
}

export const logger = {
  trace: (module: string, ...args: unknown[]) => log("TRACE", module, ...args),
  debug: (module: string, ...args: unknown[]) => log("DEBUG", module, ...args),
  info: (module: string, ...args: unknown[]) => log("INFO", module, ...args),
  warn: (module: string, ...args: unknown[]) => log("WARN", module, ...args),
  error: (module: string, ...args: unknown[]) => log("ERROR", module, ...args),
  /**
   * 用户操作结果上报（无 5s 节流；一次用户操作 = 一条日志）。
   * 用法：`logger.action("INFO", "Cleanup", logMessage)`
   */
  action: (
    level: "INFO" | "WARN" | "ERROR",
    module: string,
    ...args: unknown[]
  ) => logAction(level, module, ...args),
};
