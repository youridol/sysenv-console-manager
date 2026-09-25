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
import { cn, formatBytes, withTimeout } from "../lib/utils";
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

/* ── 执行结果 → 右侧栏操作日志（cleanup scope）的边界预算 ── */
/** 单条正文行的字符数上限（先按行裁剪，控制单行爆炸） */
const OP_LOG_MAX_LINE_CHARS = 200;
/** 单条正文行数上限（超出的行折叠为一行省略提示，不逐行上报） */
const OP_LOG_MAX_BODY_LINES = 12;
/**
 * 整条消息的字符数硬上限（最终兜底；头部 + 正文 + 省略提示共用）。
 *
 * 字节预算（后端 `append_frontend_log` MAX_LEN = 4096 **字节**，按 UTF-8 计）：
 *   - 头部（成功/失败 + 操作名 + formatBytes + ASCII 标点）≈ 110B；
 *   - 正文先按行裁剪为 ≤12 行 × 200 字符，但该理论上限（7200B）不可接受，
 *     故再用本上限二次兜底：截断结果为「前 1024 字符 + 省略号」= 最多 1025 字符，
 *     1025 × 3B/字符 = **3075B**（上限）。
 *     单字符 UTF-8 最大字节数是 3B（BMP 中文/全角占 1 个 UTF-16 码元）；
 *     补充平面字符为 4B 但占 2 个码元，即 2B/码元，不突破 3B/码元的上界。
 *   - 最终消息 ≤ 3075B < 4096B，留出约 1KB 余量；
 *     `message.slice(0, 4096)`（前端）与后端 MAX_LEN 均不会触发截断。
 */
const OP_LOG_MAX_TOTAL_CHARS = 1024;

/** 按字符数截断（`slice` 作用于 UTF-16 码元，不会切断码点，故 UTF-8 边界安全） */
function clipChars(s: string, max: number): string {
  return s.length <= max ? s : `${s.slice(0, max)}…`;
}

/** 把执行结果压成单条有界日志消息（不依赖后端截断兜底） */
function buildActionResultMessage(result: CleanupResult): string {
  const head = `${result.success ? "成功" : "失败"} · ${result.operation} · 释放 ${formatBytes(
    result.bytes_freed,
  )}`;
  const lines = result.message
    .split("\n")
    .map((l) => l.trimEnd())
    .filter((l) => l.length > 0);
  if (lines.length === 0) return clipChars(head, OP_LOG_MAX_TOTAL_CHARS);
  const kept = lines
    .slice(0, OP_LOG_MAX_BODY_LINES)
    .map((l) => clipChars(l, OP_LOG_MAX_LINE_CHARS));
  if (lines.length > kept.length) {
    kept.push(`… 其余 ${lines.length - kept.length} 行已省略`);
  }
  return clipChars([head, ...kept].join("\n"), OP_LOG_MAX_TOTAL_CHARS);
}

/**
 * 把一次操作结果同步写入右侧栏日志面板（「按页输出」下 `/cleanup` 可见）。
 *
 * 为什么用 `logger.action` 而非 `logger.info`：`reportToBackend` 以
 * `级别:模块` 为键做 5s 节流，连续两次清理操作（同为 INFO + "Cleanup"）会让
 * 第二条被静默丢弃，违背「每次执行恰好一条日志」。`action` 不做节流判定，
 * 每次调用必定上报一条；正常 info/warn/error 的节流语义不变。
 *
 * 次数保证：`runAction` 的 success 分支与 catch 分支互斥且都在
 * `seq === actionSeq.current` 校验之后，故一次执行恰好调用一次本函数。
 */
function reportActionResult(
  result: CleanupResult,
  failureLevel: "WARN" | "ERROR" = "WARN",
) {
  logger.action(
    result.success ? "INFO" : failureLevel,
    "Cleanup",
    buildActionResultMessage(result),
  );
}

/**
 * 清理/进程操作超时（毫秒）。
 *
 * Tauri invoke 无法取消底层 spawn_blocking，后端若卡住（例如遍历被占用的
 * 磁盘目录、OpenProcess 挂起），`loading` 会永久为真、按钮永远转圈。
 * 本页此前是唯一没有超时保护的操作页（Dashboard / Hardware 均已用
 * withTimeout 兜底）。给所有操作统一加保护：到点即报错并复位 loading，
 * 用户至少能看到失败原因并重试。
 *
 * 取值需大于最慢的正常清理（全量着色器缓存清理含遍历多库目录 + 重启删除
 * 登记），同时远小于用户忍受上限。着色器全清单独放宽。
 */
const CLEANUP_TIMEOUT_MS = 60000;
/** 「全部清理」需串行遍历 NVIDIA/AMD/DirectX/Steam 四类缓存，放宽到 3 分钟 */
const CLEAN_SHADER_ALL_TIMEOUT_MS = 180000;
/** 进程列表枚举（sysinfo 全量刷新）超时 */
const LIST_PROCESSES_TIMEOUT_MS = 15000;

/** 按命令取超时：全部清理用放宽值，其余用默认值 */
function timeoutFor(cmd: string): number {
  return cmd === "clean_shader_cache"
    ? CLEAN_SHADER_ALL_TIMEOUT_MS
    : CLEANUP_TIMEOUT_MS;
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
  /** 进程列表读取失败原因；非空时卡片显示「读取失败」而非「未检测到进程」。 */
  const [processesError, setProcessesError] = useState<string | null>(null);
  const [procSearch, setProcSearch] = useState("");
  // 清理操作执行状态：cmd -> 成功与否（undefined=未执行）
  const [cleanStatus, setCleanStatus] = useState<Record<string, boolean>>({});
  // 高亮复位定时器（1 秒后清除状态）
  const cleanResetTimers = useRef<Record<string, number>>({});
  /**
   * 操作请求序号：每次 runAction 递增。超时后底层 invoke 仍在后台运行，
   * 其 finally 若直接 setLoading(null) 会误清新操作的转圈态。
   * 响应/超时返回时序号不匹配即视为过期，不触碰 loading。
   */
  const actionSeq = useRef(0);

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
      const procs = await withTimeout(
        invoke<ProcessInfo[]>("list_processes"),
        LIST_PROCESSES_TIMEOUT_MS,
        `进程列表获取超时（超过 ${LIST_PROCESSES_TIMEOUT_MS / 1000} 秒无响应）`,
      );
      setProcesses(procs);
      setProcessesError(null);
    } catch (e: unknown) {
      // 失败必须可见且与「无进程」区分（ADR-0001 D9）：原实现仅 DEV 记日志，
      // 生产环境静默，进程列表保持空数组 → UI 显示「未检测到进程」，
      // 把「读取失败」呈现成「机器上没有进程」。
      const msg = e instanceof Error ? e.message : String(e);
      logger.error("Cleanup", "list_processes failed:", msg);
      setProcessesError(msg);
    }
  };

  useEffect(() => {
    // 异步数据加载：setProcesses 在 await 之后，非同步级联渲染（合法用例）
    void loadProcesses();
    // 卸载时清掉全部高亮复位定时器（ADR-0001 D12）：否则成功后 1 秒内切换路由，
    // 定时器仍会触发 setCleanStatus，对已卸载组件调用 setState。
    return () => {
      for (const timer of Object.values(cleanResetTimers.current)) {
        window.clearTimeout(timer);
      }
      cleanResetTimers.current = {};
    };
  }, []);

  const runAction = async (
    cmd: string,
    label: string,
    args?: Record<string, unknown>,
  ) => {
    setLoading(label);
    const seq = ++actionSeq.current;
    try {
      // 超时保护：后端卡住时到点 reject，finally 复位 loading，
      // 避免按钮永久转圈（本页唯一的 loading 复位路径）
      const result = await withTimeout(
        invoke<CleanupResult>(cmd, args),
        timeoutFor(cmd),
        `操作超时（超过 ${timeoutFor(cmd) / 1000} 秒无响应）。后端可能正被占用文件阻塞，请稍后重试；部分文件可能已登记重启后删除。`,
      );
      if (seq !== actionSeq.current) return;
      setResults((prev) => [{ result, time: formatTime(new Date()) }, ...prev]);
      // 同步写入右侧栏日志面板（每次执行恰好一条；不节流，见 reportActionResult）
      reportActionResult(result);
      // 清理操作记录执行结果（成功 → 按钮高亮绿色，1 秒后复位）
      if (CLEAN_STATUS_CMDS.includes(cmd)) {
        flashStatus(cmd, result.success);
      }
    } catch (e: unknown) {
      if (seq !== actionSeq.current) return;
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
      // 超时属预期的保护路径（withTimeout 以「操作超时（」开头），记为 WARN；其余异常记为 ERROR
      const timedOut = e instanceof Error && e.message.startsWith("操作超时（");
      reportActionResult(failed, timedOut ? "WARN" : "ERROR");
      if (CLEAN_STATUS_CMDS.includes(cmd)) {
        flashStatus(cmd, false);
      }
    } finally {
      if (seq === actionSeq.current) setLoading(null);
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

      {/* ══════════ 系统清理 / 进程管理（全宽纵向堆叠） ══════════ */}
      {/* 全宽排列：原为 @2xl/main:grid-cols-2 两列并排，现改为单列堆叠，
          三张卡片（系统清理 / 进程管理 / 执行结果）均占满内容列宽度。
          卡片内的嵌套网格因此获得完整宽度，按 @container/section 自然展开更多列。 */}
      <div className="flex flex-col gap-5">
        <section className="animate-fade-in-up stagger-1 flex flex-col">
          <div className="mb-2 shrink-0">
            <SectionTitle icon={Layers} title="系统清理" />
          </div>

          {/* 基础清理 + 着色器缓存：合并为单个容器 */}
          <Card>
            {/* @container/section（本地命名容器）：本卡内两个网格是**嵌套网格**，
                按卡片自身宽度（现为内容列全宽）解析列数，而非页面级 @container/main。
                用本地容器可让列数随卡片实际宽度响应，不依赖外层网格形态。 */}
            <CardContent className="@container/section p-4 space-y-3">
              {/* 基础清理 3 项：DNS + 临时文件 + 工作集修剪（破坏性操作均带 AlertDialog 确认） */}
              {/* 上限 2 列（≥512px）：每格 ≈355px，图标+标题+按钮均可完整显示。
                  不取 3 列是因为每格会压到 ≈240px，标题被截断成「DN...」「临时...」
                  （实测全宽卡片下 3 列即出现该回归）。 */}
              <div className="grid grid-cols-1 @lg/section:grid-cols-2 gap-3">
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
                {/* 同为嵌套网格 → 按本地 @container/section 响应：≥512px 才并排 2 列（上限不变） */}
                <div className="grid grid-cols-1 @lg/section:grid-cols-2 gap-4">
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

        {/* 进程管理（全宽卡片，与系统清理纵向堆叠） */}
        <section className="animate-fade-in-up stagger-2 flex flex-col">
          <div className="mb-2 shrink-0">
            <SectionTitle
              icon={Cpu}
              title="进程管理"
              count={processes.length}
            />
          </div>
          <Card>
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

              {/* 进程表格：固定 10 行可视（393px = 表头 33 + 10×36），行多时可滚动查看全部。
                  全宽排列后卡片不再被两列挤压，可视行数由 7 提升到 10（更充分利用高度）。
                  滚动条已按全局要求隐藏（见 index.css），滚轮/键盘滚动不受影响。 */}
              <div className="h-[393px] shrink-0 overflow-y-auto rounded-md border border-border flex flex-col text-xs">
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
                    <div className="flex items-center justify-center flex-1 min-h-[72px] text-xs gap-1.5 text-muted-foreground">
                      {processesError ? (
                        /* 读取失败与「无进程」必须可区分（ADR-0001 D9） */
                        <span className="flex items-center gap-1.5 text-destructive">
                          <Activity className="size-3.5 opacity-70" />
                          读取失败：{processesError}
                        </span>
                      ) : processes.length === 0 ? (
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

      {/* ══════════ 执行结果（独立卡片，含 Alert 状态反馈；同占内容列全宽） ══════════ */}
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
