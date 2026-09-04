import { cn } from "../lib/utils";

interface StatusDotProps {
  /** 尺寸覆写（默认 size-2.5） */
  className?: string;
  /** 颜色类，默认 bg-brand-green */
  colorClass?: string;
  /** 是否呼吸动画（默认关闭：WebView2 软件渲染下无限 animate-ping 有性能泄漏风险，见 LiveIndicator 注释） */
  pulse?: boolean;
}

/**
 * 统一「实时指示点」（收编 LiveIndicator + AppSidebar 内联点实现，D 决策）。
 * 色板收敛到品牌色（bg-brand-*）。
 */
export function StatusDot({
  className,
  colorClass = "bg-brand-green",
  pulse = false,
}: StatusDotProps) {
  return (
    <span className={cn("relative flex size-2.5 shrink-0", className)}>
      {pulse && (
        <span
          className={cn(
            "absolute inline-flex h-full w-full animate-ping rounded-full opacity-60",
            colorClass,
          )}
        />
      )}
      <span
        className={cn(
          "relative inline-flex h-full w-full rounded-full",
          colorClass,
        )}
      />
    </span>
  );
}
