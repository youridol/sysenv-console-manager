import { cn } from "../lib/utils";

interface LiveIndicatorProps {
  color?: "blue" | "green";
  className?: string;
}

export function LiveIndicator({
  color = "green",
  className,
}: LiveIndicatorProps) {
  const dotColor = color === "blue" ? "bg-brand-blue" : "bg-brand-green";

  return (
    <span className={cn("relative flex h-2 w-2", className)}>
      {/* 静态指示点：animate-ping 无限动画在 WebView2 软件渲染下持续泄漏（诊断 v0.12.0） */}
      <span
        className={`relative inline-flex rounded-full h-2 w-2 ${dotColor}`}
      />
    </span>
  );
}
