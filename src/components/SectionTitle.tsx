import type { LucideIcon } from "lucide-react";
import { cn } from "../lib/utils";

interface SectionTitleProps {
  icon?: LucideIcon;
  title: string;
  /** 可选计数 pill */
  count?: number;
  className?: string;
}

/**
 * 区块小标题：图标（可选）+ 大写小号标题 + 可选计数 pill。
 * 用于卡片内子分组 / 页面区块标题，统一 P4 的标题层级。
 */
export function SectionTitle({
  icon: Icon,
  title,
  count,
  className,
}: SectionTitleProps) {
  return (
    <div className={cn("flex items-center gap-2", className)}>
      {Icon && <Icon className="size-4 text-muted-foreground" />}
      <h2 className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
        {title}
      </h2>
      {count !== undefined && (
        <span className="rounded-full bg-muted px-1.5 py-0.5 text-[11px] font-medium tabular-nums text-muted-foreground">
          {count}
        </span>
      )}
    </div>
  );
}
