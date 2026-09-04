// 统一前端日志模块（前后端 debug.log 统一出口）
//
// 设计：
//   - 统一格式：时间戳 + 级别 + 模块名 + 消息（与后端 debug.rs 条目对齐）
//   - 输出：DEV 下 console 全量输出；生产仅 WARN/ERROR 输出 console；
//     WARN/ERROR 同时经 append_frontend_log 上报后端 → 写入 %APPDATA%\SECM\diag\debug.log
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

/** 上报后端 debug.log（WARN/ERROR；带节流；失败静默不影响前端） */
function reportToBackend(level: LogLevel, module: string, message: string) {
  if (level !== "WARN" && level !== "ERROR") return;
  const key = `${level}:${module}`;
  const now = Date.now();
  const last = lastSent.get(key) ?? 0;
  if (now - last < BACKEND_THROTTLE_MS) return;
  lastSent.set(key, now);
  invoke("append_frontend_log", {
    level,
    module: module.slice(0, 64),
    message: message.slice(0, 4096),
  }).catch(() => {
    // 上报失败静默（如后端未就绪）
  });
}

function log(level: LogLevel, module: string, ...args: unknown[]) {
  const msg = formatArgs(args);
  const line = `[${timestamp()}] [${level}] [${module}] ${msg}`;
  // DEV 全量输出 console；生产仅 WARN/ERROR（减少控制台噪音）
  if (IS_DEV || level === "WARN" || level === "ERROR") {
    const fn =
      level === "ERROR" ? console.error : level === "WARN" ? console.warn : console.debug;
    fn(line);
  }
  reportToBackend(level, module, msg);
}

export const logger = {
  trace: (module: string, ...args: unknown[]) => log("TRACE", module, ...args),
  debug: (module: string, ...args: unknown[]) => log("DEBUG", module, ...args),
  info: (module: string, ...args: unknown[]) => log("INFO", module, ...args),
  warn: (module: string, ...args: unknown[]) => log("WARN", module, ...args),
  error: (module: string, ...args: unknown[]) => log("ERROR", module, ...args),
};
