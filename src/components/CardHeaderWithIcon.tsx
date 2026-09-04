import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import { cn } from "../lib/utils";

interface CardHeaderWithIconProps {
  icon: LucideIcon;
  /** 图标容器背景类，默认 bg-muted/40 */
  iconClass?: string;
  /** 图标颜色类，默认 text-muted-foreground */
  iconColor?: string;
  title: ReactNode;
  description?: ReactNode;
  /** 右侧插槽（如 Select / Badge / 按钮） */
  action?: ReactNode;
  /** 透传根节点（如 px-4 等布局微调） */
  className?: string;
}

/**
 * 统一卡头「图标容器 + 标题 + 描述（+ 右侧操作）」结构（P1 收敛）。
 * 图标容器 size-8 rounded-md p-1.5、图标 size-4、标题 text-sm font-semibold、描述 text-xs。
 */
export function CardHeaderWithIcon({
  icon: Icon,
  iconClass = "bg-muted/40",
  iconColor = "text-muted-foreground",
  title,
  description,
  action,
  className,
}: CardHeaderWithIconProps) {
  return (
    <div className={cn("flex items-start justify-between gap-3", className)}>
      <div className="flex min-w-0 items-start gap-3">
        <div
          className={cn(
            "flex size-8 shrink-0 items-center justify-center rounded-md p-1.5",
            iconClass,
          )}
        >
          <Icon className={cn("size-4 shrink-0", iconColor)} />
        </div>
        <div className="min-w-0 space-y-0.5">
          <div className="text-sm font-semibold">{title}</div>
          {description !== undefined && (
            <p className="text-xs text-muted-foreground">{description}</p>
          )}
        </div>
      </div>
      {action && <div className="shrink-0">{action}</div>}
    </div>
  );
}
