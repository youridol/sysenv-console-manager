import type { ReactNode } from "react";
import { cn } from "../lib/utils";

interface PageHeaderProps {
  title: ReactNode;
  description?: ReactNode;
  /** 右侧操作区插槽（可空，如刷新按钮 / 状态指示） */
  actions?: ReactNode;
  className?: string;
}

/**
 * 页面级页头：收敛各页 3 种页头形态（P4）。
 * 结构：左（h1 标题 + p 描述）｜ 右（actions 插槽）。
 * 根节点带 animate-fade-in，与 MainLayout 内容区入场动画一致。
 */
export function PageHeader({
  title,
  description,
  actions,
  className,
}: PageHeaderProps) {
  return (
    <div
      className={cn(
        "animate-fade-in flex items-center justify-between gap-4",
        className,
      )}
    >
      <div className="min-w-0">
        <h1 className="text-2xl font-bold tracking-tight">{title}</h1>
        {description !== undefined && (
          <p className="mt-1 text-sm text-muted-foreground">{description}</p>
        )}
      </div>
      {actions && (
        <div className="flex shrink-0 items-center gap-2">{actions}</div>
      )}
    </div>
  );
}
