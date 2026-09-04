import { useState, useEffect, useRef, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import { cn } from "../lib/utils";
import {
  Trash2,
  Download,
  Search,
  Clock,
  Activity,
  RefreshCw,
} from "lucide-react";
import { Button } from "../components/ui/button";
import { Card, CardContent } from "../components/ui/card";
import { Badge } from "../components/ui/badge";
import { Skeleton } from "../components/ui/skeleton";
import { Input } from "../components/ui/input";
import { Switch } from "../components/ui/switch";
import { ScrollArea } from "../components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
} from "../components/ui/tooltip";
import { EmptyState } from "@/components/EmptyState";

interface LogEntry {
  timestamp: string;
  level: string;
  message: string;
}

const LEVELS = ["Info", "Warn", "Error", "Debug"] as const;

const LEVEL_CONFIG: Record<
  string,
  {
    color: string;
    dot: string;
    variant: "default" | "secondary" | "destructive" | "outline";
  }
> = {
  Info: { color: "text-blue-400", dot: "bg-blue-400", variant: "secondary" },
  Warn: { color: "text-yellow-400", dot: "bg-yellow-400", variant: "outline" },
  Error: { color: "text-red-400", dot: "bg-red-400", variant: "destructive" },
  Debug: {
    color: "text-muted-foreground",
    dot: "bg-muted-foreground",
    variant: "secondary",
  },
};

const FALLBACK_CAPACITY = 200;

/** 内存缓冲容量选项（Select 筛选用；默认值 200 与后端 log::CAPACITY 一致） */
const CAPACITY_OPTIONS = [100, 200, 500, 1000] as const;

export default function Logs() {
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [enabledLevels, setEnabledLevels] = useState<Set<string>>(
    new Set(LEVELS),
  );
  const [autoScroll, setAutoScroll] = useState(true);
  const scrollRef = useRef<HTMLDivElement>(null);
  const [search, setSearch] = useState("");
  const [maxDisplay, setMaxDisplay] = useState(FALLBACK_CAPACITY);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  // 拉取失败错误信息（区分「加载失败 / 暂无日志」两种空态）
  const [fetchError, setFetchError] = useState<string | null>(null);

  const stats = useMemo(() => {
    const counts: Record<string, number> = {
      Info: 0,
      Warn: 0,
      Error: 0,
      Debug: 0,
    };
    logs.forEach((l) => {
      counts[l.level] = (counts[l.level] || 0) + 1;
    });
    return counts;
  }, [logs]);

  useEffect(() => {
    invoke<number>("get_log_capacity")
      .then(setMaxDisplay)
      .catch(() => setMaxDisplay(FALLBACK_CAPACITY));
  }, []);

  /** 拉取最新日志（工具栏刷新按钮复用，invoke 命令与初始加载一致） */
  const loadLogs = async () => {
    setRefreshing(true);
    try {
      const data = await invoke<LogEntry[]>("get_logs");
      setLogs(data);
      setFetchError(null);
      setLoading(false);
    } catch (e: unknown) {
      setFetchError(e instanceof Error ? e.message : String(e));
      setLoading(false);
    } finally {
      setRefreshing(false);
    }
  };

  useEffect(() => {
    loadLogs();
    // 仅挂载时加载一次（刷新由工具栏按钮触发）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    const capRef = { current: maxDisplay };
    capRef.current = maxDisplay;
    listen<LogEntry>("log-event", (event) => {
      setLogs((prev) => {
        const next = [...prev, event.payload];
        return next.length > capRef.current
          ? next.slice(-capRef.current)
          : next;
      });
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      unlisten?.();
    };
  }, [maxDisplay]);

  useEffect(() => {
    if (autoScroll && scrollRef.current) {
      // ScrollArea 的滚动容器是内部 viewport（data-slot 定位，组件未导出 Viewport ref）
      const viewport = scrollRef.current.closest(
        '[data-slot="scroll-area-viewport"]',
      );
      if (viewport instanceof HTMLElement) {
        viewport.scrollTop = viewport.scrollHeight;
      }
    }
  }, [logs, autoScroll]);

  const handleClear = async () => {
    try {
      await invoke("clear_logs");
      setLogs([]);
    } catch {
      /* noop */
    }
  };

  const handleExport = async () => {
    try {
      const content = await invoke<string>("export_logs");
      const filePath = await save({
        defaultPath: "secm_logs.txt",
        filters: [{ name: "Text Files", extensions: ["txt"] }],
      });
      if (filePath) await writeTextFile(filePath, content);
    } catch {
      /* noop */
    }
  };

  const toggleLevel = (level: string) => {
    setEnabledLevels((prev) => {
      const next = new Set(prev);
      if (next.has(level)) {
        next.delete(level);
      } else {
        next.add(level);
      }
      return next;
    });
  };

  const filteredLogs = logs.filter((l) => {
    if (!enabledLevels.has(l.level)) return false;
    if (search && !l.message.toLowerCase().includes(search.toLowerCase()))
      return false;
    return true;
  });

  return (
    <div className="flex flex-col gap-6 h-[calc(100vh-8rem)]">
      {/* ── Header ── */}
      <div className="animate-fade-in">
        <h1 className="text-2xl font-bold">调试日志</h1>
        <div className="flex items-center gap-4 mt-1">
          <span className="text-xs text-muted-foreground flex items-center gap-1.5">
            <Clock className="size-3" />
            实时事件推送
          </span>
          <span className="text-xs text-muted-foreground">
            {logs.length} / {maxDisplay} 条
          </span>
        </div>
      </div>

      {/* ── 工具栏：级别筛选（Button）+ 容量（Select）+ 刷新/导出/清空（Button） ── */}
      <div className="flex items-center gap-3 flex-wrap animate-fade-in-up stagger-1">
        {/* 级别统计标签（点击筛选） */}
        <div className="flex items-center gap-1.5">
          {LEVELS.map((level) => (
            <Button
              key={level}
              variant={enabledLevels.has(level) ? "secondary" : "ghost"}
              size="sm"
              onClick={() => toggleLevel(level)}
              className="h-8 gap-1.5 rounded-full px-2.5 text-xs font-medium"
            >
              <span
                className={cn(
                  "size-1.5 rounded-full",
                  LEVEL_CONFIG[level]?.dot ?? "bg-muted-foreground",
                )}
              />
              {level}
              <span className="tabular-nums text-[11px] opacity-60">
                {stats[level] || 0}
              </span>
            </Button>
          ))}
        </div>

        <div className="flex-1" />

        {/* 显示容量筛选 */}
        <Select
          value={String(maxDisplay)}
          onValueChange={(v) => setMaxDisplay(Number(v))}
        >
          <SelectTrigger
            className="h-8 w-[110px] text-xs"
            aria-label="显示容量"
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {CAPACITY_OPTIONS.map((cap) => (
              <SelectItem key={cap} value={String(cap)}>
                最多 {cap} 条
              </SelectItem>
            ))}
          </SelectContent>
        </Select>

        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="outline"
              size="sm"
              onClick={loadLogs}
              disabled={refreshing}
            >
              <RefreshCw
                className={cn("size-3.5", refreshing && "animate-spin")}
              />
              刷新
            </Button>
          </TooltipTrigger>
          <TooltipContent>重新拉取日志缓冲区</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button variant="outline" size="sm" onClick={handleExport}>
              <Download className="size-3.5" />
              导出
            </Button>
          </TooltipTrigger>
          <TooltipContent>导出所有日志到 .txt 文件</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button variant="outline" size="sm" onClick={handleClear}>
              <Trash2 className="size-3.5 text-red-400" />
              清空
            </Button>
          </TooltipTrigger>
          <TooltipContent>清空内存缓冲区</TooltipContent>
        </Tooltip>
      </div>

      {/* ── 搜索 + 自动滚动开关 ── */}
      <div className="flex items-center gap-3 animate-fade-in-up stagger-2">
        <div className="relative flex-1">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground" />
          <Input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="搜索日志内容..."
            className="pl-9 pr-3 h-9 text-sm"
          />
        </div>
        <label className="flex items-center gap-1.5 text-xs text-muted-foreground cursor-pointer shrink-0 select-none">
          <Switch
            size="sm"
            checked={autoScroll}
            onCheckedChange={setAutoScroll}
            aria-label="自动滚动"
          />
          自动滚动
        </label>
      </div>

      {/* ── 日志列表（ScrollArea 滚动区 + 等宽字体 + 级别 Badge） ── */}
      <Card className="flex-1 flex flex-col min-h-0 animate-fade-in-up stagger-3">
        {loading ? (
          <CardContent className="flex-1 p-4 flex flex-col gap-1">
            {Array.from({ length: 12 }).map((_, i) => (
              <Skeleton key={i} className="h-5 rounded-md" />
            ))}
          </CardContent>
        ) : (
          <CardContent className="flex-1 p-0 overflow-hidden">
            <ScrollArea className="h-full">
              <div
                ref={scrollRef}
                className="min-h-full font-mono text-xs leading-relaxed"
              >
                {fetchError && logs.length === 0 ? (
                  <EmptyState
                    icon={Activity}
                    title="日志加载失败"
                    description={fetchError}
                    action={
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => void loadLogs()}
                      >
                        <RefreshCw className="size-3.5" />
                        重试
                      </Button>
                    }
                    className="h-full"
                  />
                ) : filteredLogs.length === 0 ? (
                  <EmptyState
                    icon={Activity}
                    title={search ? "无匹配日志" : "暂无日志"}
                    description={search ? "请调整搜索条件" : undefined}
                    className="h-full"
                  />
                ) : (
                  filteredLogs.map((entry, i) => (
                    <div
                      key={`${entry.timestamp}-${i}`}
                      className={cn(
                        "flex gap-3 px-4 py-1.5 hover:bg-muted/50 transition-colors border-b border-border/30 last:border-0",
                        i % 2 === 0 ? "bg-transparent" : "bg-muted/30",
                      )}
                    >
                      <span className="text-muted-foreground/60 shrink-0 w-[130px] select-none">
                        {entry.timestamp}
                      </span>
                      <Badge
                        variant={
                          LEVEL_CONFIG[entry.level]?.variant ?? "secondary"
                        }
                        className="shrink-0 text-[11px] px-1.5 py-0 h-4 font-mono"
                      >
                        {entry.level}
                      </Badge>
                      <span className="text-foreground/90 break-all">
                        {entry.message}
                      </span>
                    </div>
                  ))
                )}
              </div>
            </ScrollArea>
          </CardContent>
        )}
      </Card>
    </div>
  );
}
