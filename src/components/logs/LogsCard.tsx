import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactElement } from "react";
import { useLocation } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import { cn } from "@/lib/utils";
import {
  LOG_SCOPE_MODE_KEY,
  scopeForPath,
  type LogEntry,
  type LogScopeMode,
} from "@/lib/logs";
import {
  Activity,
  Clock,
  Download,
  RefreshCw,
  Search,
  Trash2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { EmptyState } from "@/components/EmptyState";

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

/** 输出模式选项（文案冻结：全部输出 | 按页输出） */
const SCOPE_MODE_OPTIONS = [
  { value: "all", label: "全部输出" },
  { value: "page", label: "按页输出" },
] as const;

/** 读取持久化的输出模式：缺失/非法一律回落 "all"（契约 §B.1） */
function readScopeMode(): LogScopeMode {
  try {
    const stored = localStorage.getItem(LOG_SCOPE_MODE_KEY);
    return stored === "page" ? "page" : "all";
  } catch {
    // localStorage 不可用（隐私模式等）→ 默认全部输出
    return "all";
  }
}

export interface LogsCardProps {
  /** 紧凑布局（右侧栏卡片态）：更小字号/行高、工具栏可换行 */
  compact?: boolean;
  className?: string;
}

/**
 * 日志卡片：由原「调试日志」页（src/pages/Logs.tsx）忠实迁移而来。
 *
 * 行为与迁移前等价：实时 `log-event` 订阅、级别筛选（含计数）、容量 Select、
 * 搜索、自动滚动、刷新、导出、清空、加载骨架与三种空态。
 * 新增：输出模式开关组（全部输出 | 按页输出）+ 按页 scope 过滤。
 */
export default function LogsCard({
  compact = false,
  className,
}: LogsCardProps): ReactElement {
  const { pathname } = useLocation();
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
  // 输出模式：全部输出（默认）/ 按页输出，持久化到 localStorage
  const [scopeMode, setScopeMode] = useState<LogScopeMode>(readScopeMode);

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
  }, []);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    // disposed 标记（ADR-0001 D12）：本 effect 依赖 maxDisplay，切换容量会退订重订。
    // 若在 `listen()` 的 promise 解析之前组件就卸载/重跑，cleanup 执行时 unlisten
    // 仍是 undefined，注册完成后便无人取消 → log-event 会持续推送给已失效的订阅者
    // （监听器泄漏）。同仓库的 WindowControls 已采用该写法，此处对齐。
    let disposed = false;
    const capRef = { current: maxDisplay };
    capRef.current = maxDisplay;
    listen<LogEntry>("log-event", (event) => {
      setLogs((prev) => {
        const next = [...prev, event.payload];
        return next.length > capRef.current
          ? next.slice(-capRef.current)
          : next;
      });
    })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* 订阅失败：日志卡片退化为仅显示已拉取的历史，不阻断页面 */
      });
    return () => {
      disposed = true;
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
  }, [logs, autoScroll, scopeMode, pathname]);

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

  /** 切换输出模式并持久化（缺失/非法已由 readScopeMode 回落 all） */
  const handleScopeModeChange = (value: string) => {
    if (value !== "all" && value !== "page") return;
    setScopeMode(value);
    try {
      localStorage.setItem(LOG_SCOPE_MODE_KEY, value);
    } catch {
      /* noop：localStorage 不可用时仅本次会话生效 */
    }
  };

  // scope 过滤在 level + search 之后（契约 §B.4）
  const currentScope = scopeForPath(pathname);
  const filteredLogs = logs.filter((l) => {
    if (!enabledLevels.has(l.level)) return false;
    if (search && !l.message.toLowerCase().includes(search.toLowerCase()))
      return false;
    if (scopeMode === "page" && (l.scope ?? "system") !== currentScope)
      return false;
    return true;
  });

  return (
    <div
      className={cn(
        "flex h-full min-h-0 flex-col",
        compact ? "gap-2.5 p-3" : "gap-6",
        className,
      )}
    >
      {/* ── Header：标题（紧凑态省略大标题）+ 实时推送 + 容量计数 ── */}
      <div className={cn("animate-fade-in", compact && "shrink-0")}>
        {!compact && <h1 className="text-2xl font-bold">调试日志</h1>}
        <div
          className={cn(
            "flex items-center gap-4",
            !compact && "mt-1",
            compact && "flex-wrap gap-x-3 gap-y-1",
          )}
        >
          <span className="text-xs text-muted-foreground flex items-center gap-1.5">
            <Clock className="size-3" />
            实时事件推送
          </span>
          <span className="text-xs text-muted-foreground tabular-nums">
            {logs.length} / {maxDisplay} 条
          </span>
        </div>
      </div>

      {/* ── 输出模式开关组（滑动分段；默认「全部输出」） ── */}
      {/* 视觉：无描边、无选中背景块，唯一「选中背景」是下方绝对定位的滑块 <span>。
          variant 用 default（toggleVariants 里即 bg-transparent，真正的无边框变体）；
          toggleVariants 无 ghost 成员，且 ui/toggle.tsx 不在本任务写范围内，
          故这里用 default + 显式覆盖基类注入的 border/背景/字重。 */}
      <ToggleGroup
        type="single"
        value={scopeMode}
        onValueChange={handleScopeModeChange}
        variant="default"
        size="sm"
        spacing={0}
        aria-label="日志输出模式"
        className={cn(
          "relative grid w-full shrink-0 grid-cols-2 border-0 shadow-none animate-fade-in-up stagger-1",
          compact ? "h-7 text-[11px]" : "h-8",
        )}
      >
        {/* 滑动指示器：随选中项平移（两段等宽，w-1/2 == grid-cols-2 的单列宽） */}
        <span
          aria-hidden="true"
          className={cn(
            "pointer-events-none absolute inset-y-0 left-0 z-0 w-1/2 rounded-md bg-accent shadow-xs transition-transform duration-200 ease-out",
            scopeMode === "page" && "translate-x-full",
          )}
        />
        {SCOPE_MODE_OPTIONS.map((opt) => (
          <ToggleGroupItem
            key={opt.value}
            value={opt.value}
            aria-label={opt.label}
            className={cn(
              // 位置/尺寸：与滑块两等分对齐
              "relative z-10 w-full rounded-md",
              // 杀掉 toggleVariants 注入的描边/阴影/背景/悬停底/选中底（保留 focus-visible 焦点环，勿动 a11y）
              "border-0 shadow-none bg-transparent",
              "hover:bg-transparent hover:text-muted-foreground",
              "data-[state=on]:bg-transparent data-[state=on]:text-foreground",
              // 未选中：次级文字、常规字重；选中：前景色 + 中等字重
              "text-muted-foreground font-normal data-[state=on]:font-medium",
              compact ? "px-2 text-[11px]" : "px-3",
            )}
          >
            {opt.label}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>

      {/* ── 工具栏：级别筛选（Button）+ 容量（Select）+ 刷新/导出/清空（Button） ── */}
      <div
        className={cn(
          "flex items-center gap-3 flex-wrap animate-fade-in-up stagger-1",
          compact && "gap-2",
        )}
      >
        {/* 级别统计标签（点击筛选） */}
        <div className="flex items-center gap-1.5">
          {LEVELS.map((level) => (
            <Button
              key={level}
              variant={enabledLevels.has(level) ? "secondary" : "ghost"}
              size="sm"
              onClick={() => toggleLevel(level)}
              aria-pressed={enabledLevels.has(level)}
              className={cn(
                "h-8 gap-1.5 rounded-full px-2.5 text-xs font-medium",
                compact && "h-7 gap-1 px-2 text-[11px]",
              )}
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
            className={cn(
              "h-8 w-[110px] text-xs",
              compact && "h-7 w-[96px] text-[11px]",
            )}
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
              aria-label="刷新"
              className={cn(compact && "h-7 px-2")}
            >
              <RefreshCw
                className={cn("size-3.5", refreshing && "animate-spin")}
              />
              {!compact && "刷新"}
            </Button>
          </TooltipTrigger>
          <TooltipContent>重新拉取日志缓冲区</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="outline"
              size="sm"
              onClick={handleExport}
              aria-label="导出"
              className={cn(compact && "h-7 px-2")}
            >
              <Download className="size-3.5" />
              {!compact && "导出"}
            </Button>
          </TooltipTrigger>
          <TooltipContent>导出所有日志到 .txt 文件</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="outline"
              size="sm"
              onClick={handleClear}
              aria-label="清空"
              className={cn(compact && "h-7 px-2")}
            >
              <Trash2 className="size-3.5 text-red-400" />
              {!compact && "清空"}
            </Button>
          </TooltipTrigger>
          <TooltipContent>清空内存缓冲区</TooltipContent>
        </Tooltip>
      </div>

      {/* ── 搜索 + 自动滚动开关 ── */}
      <div
        className={cn(
          "flex items-center gap-3 animate-fade-in-up stagger-2",
          compact && "gap-2",
        )}
      >
        <div className="relative flex-1 min-w-0">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground" />
          <Input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="搜索日志内容..."
            aria-label="搜索日志内容"
            className={cn(
              "pl-9 pr-3 h-9 text-sm",
              compact && "h-7 pl-8 text-xs",
            )}
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
      <Card
        className={cn(
          "flex-1 flex flex-col min-h-0 animate-fade-in-up stagger-3",
          compact && "gap-0 py-0",
        )}
      >
        {loading ? (
          <CardContent
            className={cn("flex-1 p-4 flex flex-col gap-1", compact && "p-2.5")}
          >
            {Array.from({ length: 12 }).map((_, i) => (
              <Skeleton
                key={i}
                className={cn("h-5 rounded-md", compact && "h-4")}
              />
            ))}
          </CardContent>
        ) : (
          <CardContent className="flex-1 p-0 overflow-hidden min-h-0">
            <ScrollArea className="h-full">
              <div
                ref={scrollRef}
                className={cn(
                  "min-h-full font-mono",
                  compact
                    ? "text-[11px] leading-snug"
                    : "text-xs leading-relaxed",
                )}
              >
                {fetchError && logs.length === 0 ? (
                  /* 空态 1/3：加载失败（可重试） */
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
                  /* 空态 2/3：无匹配（有搜索词）/ 空态 3/3：暂无日志 */
                  <EmptyState
                    icon={Activity}
                    title={search ? "无匹配日志" : "暂无日志"}
                    description={
                      search
                        ? "请调整搜索条件"
                        : scopeMode === "page"
                          ? "当前页面暂无日志，可切换「全部输出」查看全部"
                          : undefined
                    }
                    className="h-full"
                  />
                ) : (
                  filteredLogs.map((entry, i) => (
                    <div
                      key={`${entry.timestamp}-${i}`}
                      className={cn(
                        "flex gap-3 px-4 py-1.5 hover:bg-muted/50 transition-colors border-b border-border/30 last:border-0",
                        // 紧凑态（右侧栏 22rem）：改为纵向两行——首行「时间 + 级别」，消息独占
                        // 整行。横向排列时消息列只剩约 150px，`break-all` 会逐字符折行成
                        // 竖排文字，完全不可读；纵向布局让消息拿到全宽。
                        compact
                          ? "flex-col gap-0.5 px-2.5 py-1"
                          : "gap-3 px-4 py-1.5",
                        i % 2 === 0 ? "bg-transparent" : "bg-muted/30",
                      )}
                    >
                      <div className="flex items-center gap-2 min-w-0">
                        <span
                          className={cn(
                            "text-muted-foreground/60 shrink-0 select-none tabular-nums",
                            compact ? "text-[10px]" : "w-[130px]",
                          )}
                        >
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
                      </div>
                      <span
                        className={cn(
                          "text-foreground/90",
                          // 紧凑态用 break-words（长 token 才断）而非 break-all（逐字符断），
                          // 普通英文/中文按词与字正常折行；宽屏沿用原有 break-all 行为
                          compact ? "break-words" : "break-all",
                        )}
                      >
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
