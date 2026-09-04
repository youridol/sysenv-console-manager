import { logger } from "../lib/logger";
import { useState, useEffect, useRef, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Globe,
  Monitor,
  Trash2,
  Zap,
  Activity,
  Search,
  ChevronRight,
  CheckCircle2,
  XCircle,
  Layers,
  Cpu,
  Gamepad2,
  ShieldAlert,
} from "lucide-react";
import { cn, formatBytes } from "../lib/utils";
import { Button } from "../components/ui/button";
import { Card, CardContent } from "../components/ui/card";
import { Input } from "../components/ui/input";
import { Field, FieldLabel } from "../components/ui/field";
import { PageHeader } from "@/components/PageHeader";
import { SectionTitle } from "@/components/SectionTitle";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { Alert, AlertDescription, AlertTitle } from "../components/ui/alert";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "../components/ui/alert-dialog";

interface CleanupResult {
  operation: string;
  success: boolean;
  bytes_freed: number;
  message: string;
}

/** 执行结果 + 完成时间戳 */
interface ResultEntry {
  result: CleanupResult;
  time: string;
}

/** HH:mm:ss 时间格式 */
function formatTime(d: Date): string {
  return `${d.getHours().toString().padStart(2, "0")}:${d
    .getMinutes()
    .toString()
    .padStart(2, "0")}:${d.getSeconds().toString().padStart(2, "0")}`;
}

interface ProcessInfo {
  pid: number;
  name: string;
  memory_mb: number;
}

/* ── 基础清理操作（融合在一个卡片容器中）── */
const BASIC_ACTIONS = [
  {
    cmd: "flush_dns",
    label: "DNS 缓存刷新",
    icon: Globe,
    color: "text-blue-400",
    bg: "bg-blue-500/10",
    desc: "刷新 DNS 解析器缓存，解决域名解析延迟问题。",
    btnLabel: "刷新 DNS 缓存",
    loadingLabel: "执行中...",
  },
  {
    cmd: "clean_temp_files",
    label: "临时文件清理",
    icon: Trash2,
    color: "text-yellow-400",
    bg: "bg-yellow-500/10",
    desc: "清理系统 TEMP、Windows Temp 和 Prefetch 目录中的临时文件。",
    btnLabel: "清理临时文件",
    loadingLabel: "清理中...",
  },
  {
    cmd: "trim_process_working_set",
    label: "工作集修剪",
    icon: Zap,
    color: "text-purple-400",
    bg: "bg-purple-500/10",
    desc: "释放当前进程工作集，减少内存占用（完整待机内存释放需管理员权限）。",
    btnLabel: "释放内存",
    loadingLabel: "执行中...",
  },
] as const;

/* ── 着色器缓存清理（按厂商拆分）── */
const SHADER_ACTIONS = [
  {
    cmd: "clean_nvidia_cache",
    label: "NVIDIA",
    icon: Monitor,
    color: "text-green-400",
    bg: "bg-green-500/10",
    desc: "NV_Cache / GLCache",
    btnLabel: "清理",
    loadingLabel: "清理中...",
  },
  {
    cmd: "clean_amd_cache",
    label: "AMD",
    icon: Cpu,
    color: "text-red-400",
    bg: "bg-red-500/10",
    desc: "DxCache / GLCache / VkCache",
    btnLabel: "清理",
    loadingLabel: "清理中...",
  },
  {
    cmd: "clean_directx_cache",
    label: "DirectX",
    icon: Layers,
    color: "text-cyan-400",
    bg: "bg-cyan-500/10",
    desc: "D3DSCache",
    btnLabel: "清理",
    loadingLabel: "清理中...",
  },
  {
    cmd: "clean_steam_cache",
    label: "Steam",
    icon: Gamepad2,
    color: "text-orange-400",
    bg: "bg-orange-500/10",
    desc: "全部库目录 + CS2 深度缓存",
    btnLabel: "清理",
    loadingLabel: "清理中...",
  },
] as const;

const PRIORITY_OPTIONS = [
  { value: "idle", label: "空闲 (Idle)" },
  { value: "below_normal", label: "低于正常" },
  { value: "normal", label: "正常" },
  { value: "above_normal", label: "高于正常" },
  { value: "high", label: "高" },
  { value: "realtime", label: "实时" },
] as const;

/* ── 破坏性操作二次确认（AlertDialog 封装，确认后才触发原 invoke）── */
function ConfirmAction({
  title,
  description,
  confirmLabel,
  onConfirm,
  children,
}: {
  title: string;
  description: string;
  confirmLabel: string;
  onConfirm: () => void;
  children: ReactNode;
}) {
  return (
    <AlertDialog>
      <AlertDialogTrigger asChild>{children}</AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle className="flex items-center gap-2">
            <ShieldAlert className="size-4 text-yellow-400" />
            {title}
          </AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>取消</AlertDialogCancel>
          <AlertDialogAction onClick={onConfirm}>
            {confirmLabel}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

export default function Cleanup() {
  const [results, setResults] = useState<ResultEntry[]>([]);
  const [loading, setLoading] = useState<string | null>(null);
  const [pid, setPid] = useState("");
  const [priority, setPriority] = useState("normal");
  const [processes, setProcesses] = useState<ProcessInfo[]>([]);
  const [procSearch, setProcSearch] = useState("");
  // 清理操作执行状态：cmd -> 成功与否（undefined=未执行）
  const [cleanStatus, setCleanStatus] = useState<Record<string, boolean>>({});
  // 高亮复位定时器（1 秒后清除状态）
  const cleanResetTimers = useRef<Record<string, number>>({});

  /** 高亮 1 秒后复位 */
  const flashStatus = (cmd: string, ok: boolean) => {
    setCleanStatus((prev) => ({ ...prev, [cmd]: ok }));
    if (cleanResetTimers.current[cmd]) {
      window.clearTimeout(cleanResetTimers.current[cmd]);
    }
    cleanResetTimers.current[cmd] = window.setTimeout(() => {
      setCleanStatus((prev) => {
        const next = { ...prev };
        delete next[cmd];
        return next;
      });
      delete cleanResetTimers.current[cmd];
    }, 1000);
  };

  /** 需记录成功态的清理命令（基础 3 项 + 着色器 4 厂商 + 全部清理） */
  const CLEAN_STATUS_CMDS: string[] = [
    ...BASIC_ACTIONS.map((a) => a.cmd),
    ...SHADER_ACTIONS.map((a) => a.cmd),
    "clean_shader_cache",
  ];

  const loadProcesses = async () => {
    try {
      const procs = await invoke<ProcessInfo[]>("list_processes");
      setProcesses(procs);
    } catch (e: unknown) {
      if (import.meta.env.DEV) logger.error("Cleanup", "list_processes failed:", e);
    }
  };

  useEffect(() => {
    // 异步数据加载：setProcesses 在 await 之后，非同步级联渲染（合法用例）
    // eslint-disable-next-line react-hooks/set-state-in-effect
    loadProcesses();
  }, []);

  const runAction = async (
    cmd: string,
    label: string,
    args?: Record<string, unknown>,
  ) => {
    setLoading(label);
    try {
      const result = await invoke<CleanupResult>(cmd, args);
      setResults((prev) => [{ result, time: formatTime(new Date()) }, ...prev]);
      // 清理操作记录执行结果（成功 → 按钮高亮绿色，1 秒后复位）
      if (CLEAN_STATUS_CMDS.includes(cmd)) {
        flashStatus(cmd, result.success);
      }
    } catch (e: unknown) {
      const failed: CleanupResult = {
        operation: label,
        success: false,
        bytes_freed: 0,
        message: `错误: ${e instanceof Error ? e.message : String(e)}`,
      };
      setResults((prev) => [
        { result: failed, time: formatTime(new Date()) },
        ...prev,
      ]);
      if (CLEAN_STATUS_CMDS.includes(cmd)) {
        flashStatus(cmd, false);
      }
    } finally {
      setLoading(null);
    }
  };

  /* ── 过滤后的进程列表 ── */
  const filteredProcs = processes.filter(
    (p) =>
      !procSearch ||
      p.name.toLowerCase().includes(procSearch.toLowerCase()) ||
      String(p.pid).includes(procSearch),
  );
  // 渲染全部进程（上限 200）：行数多时列表滚动查看，行数少时行均分填满无留白
  const visibleProcs = filteredProcs.slice(0, 200);

  // 最近一次执行结果（用于 Alert 反馈）
  const latestEntry = results[0] ?? null;

  return (
    <div className="flex flex-col gap-6">
      {/* ══════════ 页面标题（PageHeader 契约：标题 + 描述 + 右侧统计操作区） ══════════ */}
      <PageHeader
        title="清理缓存"
        description="系统清理工具 · 进程管理 · 操作可追溯"
        actions={
          <div className="flex items-center gap-1.5 text-xs text-muted-foreground shrink-0">
            <CheckCircle2 className="size-3.5 text-green-400" />
            <span>{results.filter((r) => r.result.success).length} 成功</span>
            <span className="mx-1 text-border">|</span>
            <XCircle className="size-3.5 text-red-400" />
            <span>{results.filter((r) => !r.result.success).length} 失败</span>
          </div>
        }
      />

      {/* ══════════ 第一行：系统清理 + 进程管理（宽屏两列并排） ══════════ */}
      {/* items-stretch：两列卡片等高对齐（表格行均分填满，无底部留白） */}
      <div className="grid grid-cols-1 xl:grid-cols-2 gap-5 items-stretch">
        <section className="animate-fade-in-up stagger-1 flex flex-col">
          <div className="mb-2 shrink-0">
            <SectionTitle icon={Layers} title="系统清理" />
          </div>

          {/* 基础清理 + 着色器缓存：合并为单个容器（flex-1：吃掉 section 内标题后的剩余空间） */}
          <Card className="flex-1 min-h-0">
            <CardContent className="p-4 space-y-3">
              {/* 基础清理 3 项：DNS + 临时文件 + 工作集修剪（破坏性操作均带 AlertDialog 确认） */}
              <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
                {BASIC_ACTIONS.map((action) => {
                  const Icon = action.icon;
                  const isActive = loading === action.label;
                  const isSuccess = cleanStatus[action.cmd] === true;
                  return (
                    <div
                      key={action.cmd}
                      className="flex items-center gap-3 p-3 rounded-lg border border-border bg-background/50 hover:bg-accent/40 transition-colors min-w-0 flex-wrap"
                    >
                      {/* 图标 + 名称 + 描述（超长截断），与着色器厂商清理统一为一行式 */}
                      <div
                        className={cn("p-1.5 rounded-md shrink-0", action.bg)}
                      >
                        <Icon className={cn("size-4", action.color)} />
                      </div>
                      <div className="flex-1 min-w-0">
                        <h3 className="text-sm font-semibold truncate min-w-0">
                          {action.label}
                        </h3>
                        <p className="text-[11px] text-muted-foreground truncate">
                          {action.desc}
                        </p>
                      </div>
                      {/* 按钮：确认后触发原 runAction（invoke 命令与参数不变） */}
                      <ConfirmAction
                        title={`${action.label}`}
                        description={`此操作将执行「${action.btnLabel}」。${
                          action.cmd === "clean_temp_files"
                            ? "临时文件为可再生成数据，但被删除的文件无法恢复，请确认当前无程序依赖这些文件。"
                            : action.cmd === "trim_process_working_set"
                              ? "工作集修剪仅释放内存页，不删除任何文件，但可能短暂影响进程性能。"
                              : "DNS 缓存刷新为无害操作，仅清除解析缓存，不影响网络连接。"
                        }`}
                        confirmLabel="确认执行"
                        onConfirm={() => runAction(action.cmd, action.label)}
                      >
                        <Button
                          variant="outline"
                          size="sm"
                          className={cn(
                            "shrink-0 ml-auto h-8 px-2.5 text-xs border whitespace-nowrap",
                            isSuccess &&
                              "bg-green-500/15 border-green-500/30 text-green-400 hover:bg-green-500/25",
                          )}
                          disabled={loading !== null}
                        >
                          {isActive ? (
                            <span className="animate-spin size-3 border-2 border-current border-t-transparent rounded-full" />
                          ) : isSuccess ? (
                            <span className="flex items-center gap-1">
                              <CheckCircle2 className="size-3.5" />
                              已完成
                            </span>
                          ) : (
                            <span className="flex items-center gap-1">
                              {action.btnLabel}
                              <ChevronRight className="size-3.5" />
                            </span>
                          )}
                        </Button>
                      </ConfirmAction>
                    </div>
                  );
                })}
              </div>

              {/* 分隔线 */}
              <div className="border-t border-border pt-3">
                {/* 着色器缓存：标题 + 全部清理 */}
                <div className="flex items-center justify-between gap-3 mb-3">
                  <div className="flex items-start gap-2">
                    <div className="size-8 shrink-0 flex items-center justify-center rounded-md p-1.5 bg-green-500/10">
                      <Monitor className="size-4 text-green-400" />
                    </div>
                    <div>
                      <h3 className="text-sm font-semibold">着色器缓存清理</h3>
                      <p className="text-xs text-muted-foreground mt-0.5">
                        按厂商拆分清理，可释放数百 MB 至数 GB
                      </p>
                    </div>
                  </div>
                  {/* 全部清理：破坏性操作，需 AlertDialog 确认 */}
                  <ConfirmAction
                    title="全部着色器缓存清理"
                    description="将清理 NVIDIA / AMD / DirectX / Steam 全部着色器缓存。删除后游戏首次启动需重新编译着色器（可能出现短暂卡顿），但缓存可再生成。"
                    confirmLabel="确认全部清理"
                    onConfirm={() =>
                      runAction("clean_shader_cache", "着色器缓存清理(全部)")
                    }
                  >
                    <Button
                      variant="outline"
                      size="sm"
                      className={cn(
                        "shrink-0 h-8 border",
                        cleanStatus["clean_shader_cache"] === true &&
                          "bg-green-500/15 border-green-500/30 text-green-400 hover:bg-green-500/25",
                      )}
                      disabled={loading !== null}
                    >
                      {loading === "着色器缓存清理(全部)" ? (
                        <span className="flex items-center gap-1.5">
                          <span className="animate-spin size-3 border-2 border-current border-t-transparent rounded-full" />
                          清理中...
                        </span>
                      ) : cleanStatus["clean_shader_cache"] === true ? (
                        <span className="flex items-center gap-1.5">
                          <CheckCircle2 className="size-3.5" />
                          全部已清理
                        </span>
                      ) : (
                        <span className="flex items-center gap-1.5">
                          全部清理
                          <ChevronRight className="size-3.5" />
                        </span>
                      )}
                    </Button>
                  </ConfirmAction>
                </div>

                {/* 着色器厂商子卡片（固定紧凑高度），各厂商清理均需 AlertDialog 确认 */}
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                  {SHADER_ACTIONS.map((action) => {
                    const Icon = action.icon;
                    const isActive = loading === action.label;
                    const isSuccess = cleanStatus[action.cmd] === true;
                    return (
                      <div
                        key={action.cmd}
                        className="flex items-center gap-3 p-3 rounded-lg border border-border bg-background/50 hover:bg-accent/40 transition-colors"
                      >
                        <div
                          className={cn("p-1.5 rounded-md shrink-0", action.bg)}
                        >
                          <Icon className={cn("size-4", action.color)} />
                        </div>
                        <div className="flex-1 min-w-0">
                          <h4 className="text-sm font-semibold">
                            {action.label}
                          </h4>
                          <p className="text-[11px] text-muted-foreground truncate">
                            {action.desc}
                          </p>
                        </div>
                        <ConfirmAction
                          title={`${action.label} 着色器缓存清理`}
                          description={`将清理 ${action.label} 着色器缓存目录（${action.desc}）。删除后相关游戏首次启动需重新编译着色器（可能出现短暂卡顿），缓存可再生成。`}
                          confirmLabel="确认清理"
                          onConfirm={() => runAction(action.cmd, action.label)}
                        >
                          <Button
                            variant="outline"
                            size="sm"
                            className={cn(
                              "shrink-0 h-8 px-2.5 text-xs border",
                              isSuccess &&
                                "bg-green-500/15 border-green-500/30 text-green-400 hover:bg-green-500/25",
                            )}
                            disabled={loading !== null}
                          >
                            {isActive ? (
                              <span className="animate-spin size-3 border-2 border-current border-t-transparent rounded-full" />
                            ) : isSuccess ? (
                              <span className="flex items-center gap-1">
                                <CheckCircle2 className="size-3.5" />
                                已清理
                              </span>
                            ) : (
                              action.btnLabel
                            )}
                          </Button>
                        </ConfirmAction>
                      </div>
                    );
                  })}
                </div>
              </div>
            </CardContent>
          </Card>
        </section>

        {/* 第二列：进程管理（全宽卡片，与系统清理并排） */}
        <section className="animate-fade-in-up stagger-2 flex flex-col">
          <div className="mb-2 shrink-0">
            <SectionTitle icon={Cpu} title="进程管理" count={processes.length} />
          </div>
          <Card className="flex-1 min-h-0 flex flex-col">
            <CardContent className="p-4 space-y-3 flex flex-col">
              {/* 搜索 + 优先级设置（一行显示，搜索置左） */}
              <div className="flex flex-wrap items-end gap-3">
                <Field className="flex-1 min-w-[130px]">
                  <FieldLabel>搜索进程</FieldLabel>
                  <div className="relative">
                    <Search className="absolute left-3 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground" />
                    <Input
                      value={procSearch}
                      onChange={(e) => setProcSearch(e.target.value)}
                      placeholder="名称或 PID..."
                      className="pl-9 h-9 text-sm"
                    />
                  </div>
                </Field>
                <Field className="flex-1 min-w-[90px]">
                  <FieldLabel>进程 PID</FieldLabel>
                  <Input
                    type="number"
                    value={pid}
                    onChange={(e) => setPid(e.target.value)}
                    placeholder="输入 PID"
                    className="h-9 text-sm"
                  />
                </Field>
                <Field className="flex-1 min-w-[110px]">
                  <FieldLabel>优先级</FieldLabel>
                  <Select
                    value={priority}
                    onValueChange={setPriority}
                    disabled={loading !== null}
                  >
                    <SelectTrigger className="w-full h-9 text-sm">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {PRIORITY_OPTIONS.map((opt) => (
                        <SelectItem key={opt.value} value={opt.value}>
                          {opt.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Button
                  variant="outline"
                  onClick={() =>
                    runAction("set_priority", "设置进程优先级", {
                      pid: parseInt(pid) || 0,
                      priority,
                    })
                  }
                  disabled={loading !== null || !pid}
                >
                  {loading === "设置进程优先级" ? "执行中..." : "应用优先级"}
                </Button>
              </div>

              {/* 进程表格：固定 7 行可视（285px = 表头 33 + 7×36），行多时独立滚动条查看全部 */}
              <div className="h-[285px] shrink-0 overflow-y-auto rounded-md border border-border flex flex-col text-xs proc-scrollbar">
                {/* 表头 */}
                <div className="sticky top-0 bg-muted/90 backdrop-blur-sm z-10 flex border-b border-border shrink-0">
                  <div className="text-left px-3 py-2 font-medium text-muted-foreground w-[80px] shrink-0">
                    PID
                  </div>
                  <div className="text-left px-3 py-2 font-medium text-muted-foreground flex-1 min-w-0">
                    进程名称
                  </div>
                  <div className="text-right px-3 py-2 font-medium text-muted-foreground w-[90px] shrink-0">
                    内存
                  </div>
                </div>
                {/* 数据行：flex-1 均分填满容器高度；行数多时 min-h 保底触发滚动 */}
                <div className="flex flex-col flex-1">
                  {visibleProcs.map((p) => (
                    <div
                      key={p.pid}
                      onClick={() => {
                        // 选中进程 → 填入 PID（行高亮可见）
                        setPid(String(p.pid));
                      }}
                      className={cn(
                        "flex items-center px-3 border-b border-border/50 hover:bg-accent/60 transition-colors cursor-pointer flex-1 min-h-[36px]",
                        String(p.pid) === pid &&
                          "bg-blue-500/10 hover:bg-blue-500/20",
                      )}
                    >
                      <div className="font-mono text-muted-foreground w-[80px] shrink-0">
                        {p.pid}
                      </div>
                      <div className="truncate min-w-0 flex-1">{p.name}</div>
                      <div className="text-right font-mono text-muted-foreground tabular-nums w-[90px] shrink-0">
                        {p.memory_mb.toFixed(1)} MB
                      </div>
                    </div>
                  ))}
                  {/* 空态：进程为空或搜索无匹配时显示提示行（bug B6） */}
                  {visibleProcs.length === 0 && (
                    <div className="flex items-center justify-center flex-1 min-h-[72px] text-xs text-muted-foreground gap-1.5">
                      {processes.length === 0 ? (
                        <>
                          <Activity className="size-3.5 opacity-50" />
                          未检测到进程
                        </>
                      ) : (
                        <>
                          <Search className="size-3.5 opacity-50" />
                          无匹配进程，请调整搜索条件
                        </>
                      )}
                    </div>
                  )}
                </div>
              </div>
            </CardContent>
          </Card>
        </section>
      </div>

      {/* ══════════ 第二行：执行结果（独立卡片，含 Alert 状态反馈） ══════════ */}
      <section className="animate-fade-in-up stagger-3">
        <Card>
          <CardContent className="p-4 space-y-3">
            <SectionTitle
              icon={Activity}
              title="执行结果"
              count={results.length > 0 ? results.length : undefined}
            />

            {/* 最近一次操作结果反馈（成功/失败 Alert） */}
            {latestEntry && (
              <Alert
                variant={latestEntry.result.success ? "default" : "destructive"}
                className={cn(
                  "py-2.5",
                  latestEntry.result.success &&
                    "border-green-500/30 bg-green-500/5",
                )}
              >
                {latestEntry.result.success ? (
                  <CheckCircle2 className="text-green-400" />
                ) : (
                  <XCircle className="text-red-400" />
                )}
                <AlertTitle className="text-xs">
                  {latestEntry.result.success ? "操作成功" : "操作失败"} ·{" "}
                  {latestEntry.result.operation}
                  {latestEntry.result.bytes_freed > 0 && (
                    <span className="text-green-400/80 ml-1.5">
                      +{formatBytes(latestEntry.result.bytes_freed)}
                    </span>
                  )}
                </AlertTitle>
                <AlertDescription className="font-mono text-[11px] leading-relaxed">
                  {latestEntry.result.message}
                </AlertDescription>
              </Alert>
            )}

            {results.length === 0 ? (
              <p className="text-xs text-muted-foreground/70 py-3 text-center">
                使用上方操作按钮，执行结果将显示在此处
              </p>
            ) : (
              <div className="max-h-64 overflow-y-auto rounded-md border border-border">
                {results.map((entry, i) => (
                  <div
                    key={`${entry.time}-${entry.result.operation}-${i}`}
                    className={cn(
                      "flex items-start gap-3 px-4 py-3 border-b border-border/50 last:border-b-0 text-xs",
                      entry.result.success ? "bg-green-500/5" : "bg-red-500/5",
                    )}
                  >
                    {entry.result.success ? (
                      <CheckCircle2 className="size-3.5 text-green-400 shrink-0 mt-0.5" />
                    ) : (
                      <XCircle className="size-3.5 text-red-400 shrink-0 mt-0.5" />
                    )}
                    <div className="flex-1 min-w-0">
                      {/* 操作行：时间戳置首，多行结果按行对齐 */}
                      <div className="flex items-center gap-2">
                        <span className="text-[11px] text-muted-foreground/70 font-mono shrink-0">
                          {entry.time}
                        </span>
                        <span className="font-medium">
                          {entry.result.operation}
                        </span>
                        {entry.result.bytes_freed > 0 && (
                          <span className="text-green-400/80">
                            +{formatBytes(entry.result.bytes_freed)}
                          </span>
                        )}
                      </div>
                      {/* 结果消息：结构化逐行渲染，每行独立元素 + 时间戳前缀 */}
                      <div className="mt-1.5 space-y-1">
                        {entry.result.message.split("\n").map((line, li) => (
                          <div key={li} className="flex items-start gap-2">
                            <span className="font-mono text-[11px] text-muted-foreground/50 shrink-0">
                              [{entry.time}]
                            </span>
                            <span
                              className={cn(
                                "flex-1 min-w-0 font-mono text-[11px] leading-relaxed text-muted-foreground break-all whitespace-pre",
                                !entry.result.success && "text-red-300/80",
                              )}
                            >
                              {line || "\u00A0"}
                            </span>
                          </div>
                        ))}
                      </div>
                    </div>
                    <span
                      className={cn(
                        "text-[11px] px-1.5 py-0.5 rounded shrink-0 font-medium",
                        entry.result.success
                          ? "bg-green-500/15 text-green-400"
                          : "bg-red-500/15 text-red-400",
                      )}
                    >
                      {entry.result.success ? "成功" : "失败"}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </CardContent>
        </Card>
      </section>
    </div>
  );
}
