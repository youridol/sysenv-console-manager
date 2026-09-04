import { AlertTriangle } from "lucide-react";
import { cn } from "../lib/utils";

// ============================================================================
// 共享错误横幅
// ============================================================================

/** 错误横幅：红色边框 + 警告图标 + 标题 + 错误消息（网络配置 / DHCP / 深度检查共用） */
export default function ErrorBanner({
  title,
  message,
  className,
}: {
  title: string;
  message: string;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex items-start gap-2 rounded-lg border border-red-500/30 bg-red-500/5 p-3",
        className,
      )}
    >
      <AlertTriangle className="size-4 shrink-0 text-red-400 mt-0.5" />
      <div className="flex flex-col gap-0.5 text-xs min-w-0">
        <span className="font-medium text-red-400">{title}</span>
        <span className="text-red-400/80 break-all">{message}</span>
      </div>
    </div>
  );
}
