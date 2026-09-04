import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function formatBytes(bytes: number): string {
  // 边界处理（审查修复）：非有限值 / 负值 / 0~1 之间 / 超出单位表范围
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes === 0) return "0 B";
  if (bytes < 1) return bytes.toFixed(1) + " B";
  const units = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];
  const i = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1,
  );
  return `${(bytes / Math.pow(1024, i)).toFixed(1)} ${units[i]}`;
}

/**
 * 给 Promise 加超时：超时后 reject。
 * 用于 Tauri invoke 无法取消底层 spawn_blocking 的场景（如 IOCTL 挂起），
 * 前端在超时后给出明确错误提示，避免 UI 永久 loading。
 */
export function withTimeout<T>(
  p: Promise<T>,
  ms: number,
  message: string,
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(message)), ms);
    p.then(
      (v) => {
        clearTimeout(timer);
        resolve(v);
      },
      (e: unknown) => {
        clearTimeout(timer);
        reject(e);
      },
    );
  });
}
