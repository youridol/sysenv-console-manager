import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import { cn } from "../lib/utils";

interface EmptyStateProps {
  icon?: LucideIcon;
  title: string;
  description?: string;
  /** 可选操作区（如重试按钮） */
  action?: ReactNode;
  className?: string;
}

/**
 * 统一空态：图标（可选）+ 标题 + 描述 + 操作区（P9 收敛）。
 */
export function EmptyState({
  icon: Icon,
  title,
  description,
  action,
  className,
}: EmptyStateProps) {
  return (
    <div
      className={cn(
        "flex flex-col items-center justify-center gap-2 py-8 text-center",
        className,
      )}
    >
      {Icon && <Icon className="size-8 opacity-20" />}
      <div className="text-sm font-medium">{title}</div>
      {description !== undefined && (
        <p className="text-xs text-muted-foreground">{description}</p>
      )}
      {action}
    </div>
  );
}
