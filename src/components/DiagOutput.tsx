import { useEffect, useRef } from "react";
import { Eraser } from "lucide-react";
import { cn } from "../lib/utils";
import { Button } from "./ui/button";

interface DiagOutputProps {
  /** 输出行（流式追加） */
  lines: string[];
  /** 错误消息（红色高亮） */
  error: string | null;
  /** 是否正在流式输出 */
  loading: boolean;
  /** 清除输出并复位（复位后整块隐藏） */
  onClear: () => void;
  className?: string;
}

/**
 * 诊断输出区 — 网络诊断各功能卡片内嵌使用。
 * 无卡片框直接输出文本行；仅点击开始（loading）或已有输出时渲染；
 * 清除按钮复位后恢复隐藏。流式追加时自动滚动到底部。
 */
export function DiagOutput({
  lines,
  error,
  loading,
  onClear,
  className,
}: DiagOutputProps) {
  const scrollRef = useRef<HTMLDivElement>(null);

  // 流式追加时自动滚到底部，保持最新行可见
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lines.length, error]);

  const hasContent = lines.length > 0 || error !== null;
  // 未开始且无任何输出 → 不渲染（只有点击开始时才出现）
  if (!loading && !hasContent) return null;

  return (
    <div className={cn("mt-3 border-t border-border/60 pt-2", className)}>
      <div className="flex items-center justify-between mb-1">
        <span className="text-xs text-muted-foreground">
          {loading ? (
            <span className="text-blue-400 animate-pulse">诊断进行中…</span>
          ) : (
            `输出 ${lines.length} 行`
          )}
        </span>
        <Button
          variant="ghost"
          size="sm"
          className="h-6 px-2 text-xs text-muted-foreground hover:text-foreground"
          onClick={onClear}
          disabled={loading}
        >
          <Eraser className="size-3.5" />
          清除
        </Button>
      </div>
      <div
        ref={scrollRef}
        className="font-mono text-xs flex flex-col gap-1 max-h-[220px] overflow-y-auto"
      >
        {lines.map((l, i) => (
          <span
            key={`${i}-${l.slice(0, 32)}`}
            className="break-all text-muted-foreground"
          >
            {l}
          </span>
        ))}
        {error && <span className="break-all text-red-400">{error}</span>}
      </div>
    </div>
  );
}
