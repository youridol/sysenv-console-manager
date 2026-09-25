import type { ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { LucideIcon } from "lucide-react";
import { CheckCircle2, AlertTriangle, XCircle, Info } from "lucide-react";
import { cn } from "../lib/utils";
import { CardTitle, CardDescription } from "./ui/card";

/**
 * 环境检测域共享模块（从 src/pages/Environment.tsx 拆出）：
 * - DxCheck / renderChecks：检测项清单，DirectX / VC++ 共用渲染
 * - safeInvoke：命令调用安全包装（失败返回 null，不抛出）
 * - SectionHeader：卡片头部（图标 + 标题 + 描述）
 */

/** 检测项条目（与后端 checks 字段契约一致） */
export interface DxCheck {
  name: string;
  status: "pass" | "warn" | "fail" | "info";
  detail: string;
}

/* ── Status config ── */

const statusConfig: Record<
  string,
  { icon: typeof CheckCircle2; color: string; bg: string }
> = {
  pass: { icon: CheckCircle2, color: "text-green-400", bg: "bg-green-500/10" },
  warn: {
    icon: AlertTriangle,
    color: "text-yellow-400",
    bg: "bg-yellow-500/10",
  },
  fail: { icon: XCircle, color: "text-red-400", bg: "bg-red-500/10" },
  info: { icon: Info, color: "text-blue-400", bg: "bg-blue-500/10" },
};

/* ── 检测项清单（DirectX / VC++ 共用） ── */
export function renderChecks(checks: DxCheck[]) {
  return (
    <div className="space-y-1">
      {checks.map((check) => {
        const cfg = statusConfig[check.status];
        const Icon = cfg.icon;
        return (
          <div
            key={check.name}
            className="flex items-center gap-2 px-2.5 py-1.5 rounded-md hover:bg-muted/30 transition-colors"
          >
            <div className={cn("p-1 rounded shrink-0", cfg.bg)}>
              <Icon className={cn("size-3", cfg.color)} />
            </div>
            <span className="text-xs">{check.name}</span>
            <span className="text-xs text-muted-foreground ml-auto text-right truncate max-w-[50%]">
              {check.detail}
            </span>
          </div>
        );
      })}
    </div>
  );
}

/* ── Safe invoke wrapper ── */
export async function safeInvoke<T>(cmd: string): Promise<T | null> {
  try {
    return await invoke<T>(cmd);
  } catch {
    return null;
  }
}

/* ── 卡片头部（图标 + 标题 + 描述） ── */

interface SectionHeaderProps {
  icon: LucideIcon;
  /** 图标容器背景类（如 bg-sky-500/10） */
  iconClass: string;
  /** 图标颜色类（如 text-sky-400） */
  iconColor: string;
  title: string;
  description: ReactNode;
}

export function SectionHeader({
  icon: Icon,
  iconClass,
  iconColor,
  title,
  description,
}: SectionHeaderProps) {
  return (
    <div className="flex items-center gap-2">
      <div
        className={cn(
          "flex size-8 shrink-0 items-center justify-center rounded-md p-1.5",
          iconClass,
        )}
      >
        <Icon className={cn("size-4 shrink-0", iconColor)} />
      </div>
      <div>
        <CardTitle className="text-sm">{title}</CardTitle>
        <CardDescription className="text-xs">{description}</CardDescription>
      </div>
    </div>
  );
}
