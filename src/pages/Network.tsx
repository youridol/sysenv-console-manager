import { useEffect, useState, useRef } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import {
  Activity,
  Globe,
  MapIcon,
  Search,
  Zap,
  Play,
  Square,
  Network,
  ExternalLink,
  Plus,
  Pencil,
  Trash2,
  Loader2,
  Server,
  AlertTriangle,
  CheckCircle2,
  ChevronsUpDown,
  Check,
} from "lucide-react";
import { cn } from "../lib/utils";
import { Badge } from "../components/ui/badge";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { Button } from "../components/ui/button";
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
} from "../components/ui/card";
import { Input } from "../components/ui/input";
import { Field, FieldLabel } from "../components/ui/field";
import { Switch } from "../components/ui/switch";
import { ToggleGroup, ToggleGroupItem } from "../components/ui/toggle-group";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "../components/ui/popover";
import { useStreaming, type StreamEvent } from "../hooks/useStreaming";
import {
  Tabs,
  TabsList,
  TabsTrigger,
} from "../components/ui/tabs";
import { DiagOutput } from "@/components/DiagOutput";
import { PageHeader } from "@/components/PageHeader";
import ErrorBanner from "@/components/ErrorBanner";
import { logger } from "../lib/logger";

// ============================================================================
// Types
// ============================================================================

interface PingResult {
  sequence: number;
  rtt_ms: number;
  success: boolean;
}

interface TraceHop {
  hop: number;
  ip: string;
  rtt_avg_ms: number;
  rtt_min_ms: number;
  rtt_max_ms: number;
  probes_answered: number;
  probes_sent: number;
}

interface StunTestStep {
  phase: string;
  description: string;
}

/** DHCP 服务器检测结果（对应 Rust DhcpProbeResult 契约） */
interface DhcpProbeResult {
  servers: string[];
  count: number;
  healthy: boolean;
  baseline_server: string | null;
  note: string;
}

/** DHCP 检测卡片状态机：idle / loading / done / error */
type DhcpState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "done"; result: DhcpProbeResult }
  | { status: "error"; message: string };

/** 单台 DHCP 服务器深度检查（对应 Rust DhcpServerCheck 契约） */
interface DhcpServerCheck {
  server: string;
  ping_ok: boolean;
  ping_rtt_ms: number;
  same_subnet: boolean;
  is_gateway: boolean;
  severity: "ok" | "warn" | "critical";
  diagnosis: string;
}

/** DHCP 深度检查结果（对应 Rust DhcpDeepCheckResult 契约） */
interface DhcpDeepCheckResult {
  local_ip: string | null;
  gateway: string | null;
  subnet_mask: string | null;
  servers: string[];
  checks: DhcpServerCheck[];
  summary: string;
}

/** DHCP 深度检查状态机：idle / loading / done / error */
type DeepDhcpState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "done"; result: DhcpDeepCheckResult }
  | { status: "error"; message: string };

const STUN_SERVERS = [
  "stun.qq.com:3478",
  "stun.l.google.com:19302",
  "stun1.l.google.com:19302",
  "stun2.l.google.com:19302",
  "stun.freeswitch.org:3478",
  "global.stun.twilio.com:3478",
  "stun.counterpath.com:3478",
  "stun.sipgate.net:3478",
  "stun.ekiga.net:3478",
  "stun.3cx.com:3478",
  "stun.blink.de:3478",
  "stun.vivox.com:3478",
  "stun.nextcloud.com:3478",
  "stun.linphone.org:3478",
  "stun.miwifi.com:3478",
];

/** Traceroute DNS 查询预设服务器列表 */
const TRACE_DNS_PRESETS = [
  { label: "腾讯 DNSPod", value: "119.29.29.29" },
  { label: "阿里 AliDNS", value: "223.5.5.5" },
  { label: "百度 DNS", value: "180.76.76.76" },
  { label: "114 DNS", value: "114.114.114.114" },
  { label: "Google DNS", value: "8.8.8.8" },
  { label: "Cloudflare", value: "1.1.1.1" },
];

function generateId() {
  return `cmd-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

/** fetch HEAD 探测：超时由 AbortController 控制，成功返回 true，失败/超时返回 false（网站测试与端口检测共用） */
async function headProbe(url: string, timeoutMs: number): Promise<boolean> {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), timeoutMs);
  try {
    await fetch(url, {
      method: "HEAD",
      mode: "no-cors",
      signal: ctrl.signal,
    });
    return true;
  } catch {
    return false;
  } finally {
    clearTimeout(timer);
  }
}

// ============================================================================
// 流式消息格式化
// ============================================================================

/** 将 StreamEvent 格式化为用户可读的文本行 */
function formatStreamMessage(ev: StreamEvent): string | null {
  // 安全解析：后端异常数据（非 JSON）时降级为原文，不抛异常中断事件流
  const safeParse = <T,>(data: string): T | null => {
    try {
      return JSON.parse(data) as T;
    } catch {
      return null;
    }
  };
  switch (ev.kind) {
    case "ping": {
      const r = safeParse<PingResult>(ev.data);
      if (r === null) return ev.data;
      return r.success
        ? `第 #${String(r.sequence).padStart(3, "0")} 包: 成功  |  延迟 = ${r.rtt_ms.toFixed(1)} ms`
        : `第 #${String(r.sequence).padStart(3, "0")} 包: 超时  |  请求超时，无响应`;
    }
    case "trace-hop": {
      const h = safeParse<TraceHop>(ev.data);
      if (h === null) return ev.data;
      return h.rtt_avg_ms > 0
        ? `跃点 ${String(h.hop).padStart(2, "0")}: ${h.ip.padEnd(18)}  avg=${h.rtt_avg_ms.toFixed(1)}ms  min=${h.rtt_min_ms.toFixed(1)}ms  max=${h.rtt_max_ms.toFixed(1)}ms  (${h.probes_answered}/${h.probes_sent})`
        : `跃点 ${String(h.hop).padStart(2, "0")}: ${" *".padEnd(18)}  * * *  (${h.probes_answered}/${h.probes_sent})`;
    }
    case "dns-record":
    case "info":
      return ev.data;
    case "stun-step": {
      const s = safeParse<StunTestStep>(ev.data);
      if (s === null) return ev.data;
      return `[${s.phase}] ${s.description}`;
    }
    default:
      return ev.data;
  }
}

// ============================================================================
// 共享控件组件
// ============================================================================

function IpToggle({
  value,
  onChange,
  disabled,
}: {
  value: string;
  onChange: (v: string) => void;
  disabled: boolean;
}) {
  return (
    <ToggleGroup
      type="single"
      variant="outline"
      value={value}
      onValueChange={(v) => v && onChange(v)}
      disabled={disabled}
      className="h-8"
    >
      {(["auto", "v4", "v6"] as const).map((v) => (
        <ToggleGroupItem key={v} value={v} className="text-xs px-2 h-full">
          {v === "auto" ? "自动" : v.toUpperCase()}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  );
}

function ProtoToggle({
  value,
  onChange,
  disabled,
}: {
  value: string;
  onChange: (v: string) => void;
  disabled: boolean;
}) {
  return (
    <ToggleGroup
      type="single"
      variant="outline"
      value={value}
      onValueChange={(v) => v && onChange(v)}
      disabled={disabled}
      className="h-8"
    >
      {(["icmp", "tcp", "udp"] as const).map((v) => (
        <ToggleGroupItem key={v} value={v} className="text-xs px-2 h-full">
          {v.toUpperCase()}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  );
}

/** 可编辑 DNS 下拉：输入框直接输入自定义 DNS，下拉选择本机/预设，共用同一状态 */
function DnsSelect({
  value,
  onChange,
  disabled,
  className,
}: {
  value: string; // "system"=本机 DNS，或 DNS IP，或空串
  onChange: (v: string) => void;
  disabled?: boolean;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  // 输入框显示：本机 DNS 时留空（placeholder 提示），其余显示 IP
  const display = value === "system" ? "" : value;
  const isCustom =
    value !== "" && value !== "system" &&
    !TRACE_DNS_PRESETS.some((d) => d.value === value);

  return (
    <div className={cn("flex items-center gap-1.5", className)}>
      <Input
        className="h-8 w-full min-w-0 flex-1"
        value={display}
        placeholder="本机 DNS / 输入自定义 IP"
        disabled={disabled}
        onChange={(e) => {
          onChange(e.target.value);
          setOpen(true);
        }}
        onFocus={() => setOpen(true)}
        onKeyDown={(e) => {
          if (e.key === "Escape") setOpen(false);
        }}
      />
      <Popover open={open} onOpenChange={setOpen} modal={false}>
        <PopoverTrigger asChild>
          <Button
            variant="outline"
            size="icon"
            className="h-8 w-8 shrink-0"
            disabled={disabled}
            aria-label="选择 DNS 服务器"
          >
            <ChevronsUpDown className="size-3.5" />
          </Button>
        </PopoverTrigger>
        <PopoverContent
          align="start"
          className="w-64 p-1.5 z-50"
          // 阻止 Radix 打开/关闭时自动转移焦点：否则输入框失焦只能输入 1 个字符
          onOpenAutoFocus={(e) => e.preventDefault()}
          onCloseAutoFocus={(e) => e.preventDefault()}
        >
          <div className="flex flex-col gap-0.5">
            <div className="px-2 py-1 text-[10px] text-muted-foreground">
              选择预设或直接在上方输入自定义 DNS IP
            </div>
            <button
              type="button"
              disabled={disabled}
              onClick={() => {
                onChange("system");
                setOpen(false);
              }}
              className={cn(
                "flex items-center gap-2 rounded-md px-2 py-1.5 text-xs text-left hover:bg-muted",
                value === "system" && "bg-muted/60",
              )}
            >
              <Globe className="size-3.5 text-muted-foreground shrink-0" />
              <span className="flex-1">本机 DNS</span>
              {value === "system" && <Check className="size-3.5 text-primary" />}
            </button>
            {TRACE_DNS_PRESETS.map((d) => (
              <button
                key={d.value}
                type="button"
                disabled={disabled}
                onClick={() => {
                  onChange(d.value);
                  setOpen(false);
                }}
                className={cn(
                  "flex items-center gap-2 rounded-md px-2 py-1.5 text-xs text-left hover:bg-muted",
                  value === d.value && "bg-muted/60",
                )}
              >
                <span className="flex-1">{d.label}</span>
                <span className="font-mono text-[10px] text-muted-foreground">
                  {d.value}
                </span>
                {value === d.value && (
                  <Check className="size-3.5 text-primary" />
                )}
              </button>
            ))}
            {isCustom && (
              <button
                type="button"
                className="flex items-center gap-2 rounded-md px-2 py-1.5 text-xs text-left bg-primary/10 text-primary"
              >
                <span className="flex-1">自定义</span>
                <span className="font-mono text-[10px]">{value}</span>
                <Check className="size-3.5" />
              </button>
            )}
          </div>
        </PopoverContent>
      </Popover>
    </div>
  );
}

function NumInput({
  label,
  value,
  onChange,
  min,
  max,
  placeholder,
  disabled,
  className,
}: {
  label: string;
  value: number;
  onChange: (v: number) => void;
  min: number;
  max: number;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <Field>
      <FieldLabel>{label}</FieldLabel>
      <Input
        className={cn("h-8 w-full", className)}
        type="number"
        value={value}
        onChange={(e) => onChange(parseInt(e.target.value) || 0)}
        min={min}
        max={max}
        placeholder={placeholder}
        disabled={disabled}
      />
    </Field>
  );
}

/** DHCP 检测结果视图：健康（绿）/ 冲突（黄 + 建议清单）两态 */
function DhcpResultView({ result }: { result: DhcpProbeResult }) {
  return (
    <div className="flex flex-col gap-2.5">
      <div
        className={cn(
          "flex items-start gap-2 rounded-lg border p-3",
          result.healthy
            ? "border-green-500/30 bg-green-500/10 text-green-400"
            : "border-yellow-500/30 bg-yellow-500/10 text-yellow-400",
        )}
      >
        {result.healthy ? (
          <CheckCircle2 className="size-4 shrink-0 mt-0.5" />
        ) : (
          <AlertTriangle className="size-4 shrink-0 mt-0.5" />
        )}
        <div className="flex flex-col gap-1.5 min-w-0">
          <span className="text-sm font-medium">
            {result.healthy
              ? `健康 · ${result.count} 台`
              : `检测到 ${result.count} 台 DHCP 服务器，可能存在 DHCP 冲突`}
          </span>
          {result.servers.length > 0 && (
            <div className="flex flex-wrap gap-1.5">
              {result.servers.map((s) => (
                <Badge
                  key={s}
                  variant="outline"
                  className="font-mono text-[11px] border-border bg-background/50"
                >
                  {s}
                </Badge>
              ))}
            </div>
          )}
        </div>
      </div>
      {!result.healthy && (
        <div className="rounded-lg border border-yellow-500/30 bg-yellow-500/5 p-3">
          <div className="text-xs font-medium text-yellow-400 mb-1.5">
            建议检查清单
          </div>
          <ul className="list-disc list-inside space-y-1 text-xs text-muted-foreground">
            <li>路由器 DHCP 设置</li>
            <li>多路由器级联</li>
            <li>软路由 / 旁路由 / 热点</li>
            <li>AP 的 DHCP 是否关闭</li>
          </ul>
        </div>
      )}
      {result.note && <p className="text-xs text-muted-foreground">{result.note}</p>}
    </div>
  );
}

// ============================================================================
// DHCP 深度检查展示
// ============================================================================

/** 深度检查结果视图：逐台服务器诊断 + 综合结论 */
function DeepDhcpResultView({ result }: { result: DhcpDeepCheckResult }) {
  const sevStyle = (sev: DhcpServerCheck["severity"]) =>
    sev === "ok"
      ? "border-green-500/30 bg-green-500/5"
      : sev === "warn"
        ? "border-yellow-500/30 bg-yellow-500/5"
        : "border-red-500/30 bg-red-500/5";
  const sevText = (sev: DhcpServerCheck["severity"]) =>
    sev === "ok"
      ? "text-green-400"
      : sev === "warn"
        ? "text-yellow-400"
        : "text-red-400";
  const sevIcon = (sev: DhcpServerCheck["severity"]) =>
    sev === "ok" ? (
      <CheckCircle2 className="size-4 shrink-0 mt-0.5 text-green-400" />
    ) : sev === "warn" ? (
      <AlertTriangle className="size-4 shrink-0 mt-0.5 text-yellow-400" />
    ) : (
      <AlertTriangle className="size-4 shrink-0 mt-0.5 text-red-400" />
    );

  return (
    <div className="flex flex-col gap-2.5">
      {/* 本机网络定位信息 */}
      <div className="flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground">
        <span>本机 IP</span>
        <Badge variant="outline" className="font-mono text-[10px] h-5 border-border bg-background/50">
          {result.local_ip ?? "—"}
        </Badge>
        <span>掩码</span>
        <Badge variant="outline" className="font-mono text-[10px] h-5 border-border bg-background/50">
          {result.subnet_mask ?? "—"}
        </Badge>
        <span>默认网关</span>
        <Badge variant="outline" className="font-mono text-[10px] h-5 border-border bg-background/50">
          {result.gateway ?? "—"}
        </Badge>
      </div>
      {/* 逐台服务器诊断 */}
      {result.checks.map((c) => (
        <div
          key={c.server}
          className={cn(
            "flex flex-col gap-1.5 rounded-lg border p-3",
            sevStyle(c.severity),
          )}
        >
          <div className="flex items-start gap-2">
            {sevIcon(c.severity)}
            <div className="flex flex-col gap-1.5 min-w-0">
              <div className="flex flex-wrap items-center gap-1.5">
                <span className="font-mono text-sm font-medium text-foreground">
                  {c.server}
                </span>
                {c.ping_ok ? (
                  <Badge
                    variant="outline"
                    className="border-green-500/30 bg-green-500/10 text-green-400 text-[10px] h-5 gap-1"
                  >
                    <span className="size-1 rounded-full bg-green-400" />
                    ping {c.ping_rtt_ms.toFixed(1)}ms
                  </Badge>
                ) : (
                  <Badge
                    variant="outline"
                    className="border-red-500/30 bg-red-500/10 text-red-400 text-[10px] h-5 gap-1"
                  >
                    <span className="size-1 rounded-full bg-red-400" />
                    ping 不通
                  </Badge>
                )}
                <Badge
                  variant="outline"
                  className={cn(
                    "text-[10px] h-5",
                    c.same_subnet
                      ? "border-border bg-background/50 text-muted-foreground"
                      : "border-orange-500/30 bg-orange-500/10 text-orange-400",
                  )}
                >
                  {c.same_subnet ? "同网段" : "跨网段"}
                </Badge>
                <Badge
                  variant="outline"
                  className={cn(
                    "text-[10px] h-5",
                    c.is_gateway
                      ? "border-green-500/30 bg-green-500/10 text-green-400"
                      : "border-border bg-background/50 text-muted-foreground",
                  )}
                >
                  {c.is_gateway ? "= 默认网关" : "≠ 默认网关"}
                </Badge>
              </div>
              <p className={cn("text-xs", sevText(c.severity))}>{c.diagnosis}</p>
            </div>
          </div>
        </div>
      ))}
      {/* 综合结论 */}
      {result.summary && (
        <div
          className={cn(
            "rounded-lg border p-3 text-xs",
            result.checks.some((c) => c.severity === "critical")
              ? "border-red-500/30 bg-red-500/10 text-red-300"
              : result.checks.some((c) => c.severity === "warn")
                ? "border-yellow-500/30 bg-yellow-500/10 text-yellow-300"
                : "border-green-500/30 bg-green-500/10 text-green-300",
          )}
        >
          {result.summary}
        </div>
      )}
    </div>
  );
}

// ============================================================================
// 网站测试（自 Dashboard 迁移）
// ============================================================================

const DEFAULT_SITES = [
  { name: "Apple", url: "https://www.apple.com" },
  { name: "GitHub", url: "https://github.com" },
  { name: "Google", url: "https://www.google.com" },
  { name: "YouTube", url: "https://www.youtube.com" },
];

type SiteItem = { name: string; url: string };

// ============================================================================
// Page
// ============================================================================

/** 诊断工具标识 */
type ToolId = "ping" | "trace" | "nsl" | "nat" | "port" | "iperf";

const TOOLS: Array<{
  id: ToolId;
  label: string;
  icon: typeof Activity;
  color: string;
  bg: string;
}> = [
  {
    id: "ping",
    label: "Ping",
    icon: Activity,
    color: "text-blue-400",
    bg: "bg-blue-500/10",
  },
  {
    id: "trace",
    label: "Traceroute",
    icon: MapIcon,
    color: "text-purple-400",
    bg: "bg-purple-500/10",
  },
  {
    id: "nsl",
    label: "Nslookup",
    icon: Search,
    color: "text-cyan-400",
    bg: "bg-cyan-500/10",
  },
  {
    id: "nat",
    label: "NAT",
    icon: Globe,
    color: "text-orange-400",
    bg: "bg-orange-500/10",
  },
  {
    id: "port",
    label: "Port",
    icon: Zap,
    color: "text-green-400",
    bg: "bg-green-500/10",
  },
  {
    id: "iperf",
    label: "iperf3",
    icon: Activity,
    color: "text-yellow-400",
    bg: "bg-yellow-500/10",
  },
];

// ============================================================================
// 工具控制器抽象（6 个网络工具统一骨架：loading / 输出 / 竞态 / 取消）
// ============================================================================

/** 网络工具统一控制器接口：流式工具与非流式 port 工具均实现此契约 */
interface ToolController {
  loading: boolean;
  lines: string[];
  error: string | null;
  /** 启动当前工具 */
  run: () => void;
  /** 停止当前工具（取消后端命令并复位） */
  stop: () => void;
  /** 开始/停止切换（供统一开始按钮调用） */
  toggle: () => void;
  /** 清除输出并复位 */
  clear: () => void;
}

interface UseStreamingToolOptions {
  /** 后端流式命令名（如 "ping_stream"） */
  cmd: string;
  /** 构建 invoke 参数（cmdId / channel 由调用方透传） */
  buildArgs: (ctx: {
    cmdId: string;
    channel: Channel<StreamEvent>;
  }) => Record<string, unknown>;
}

/**
 * 流式工具控制器 — 封装 useStreaming + loading + opId 竞态 + cmdId 取消。
 * 替代 5 个流式工具（ping/trace/nsl/nat/iperf）各自手写的同构骨架。
 */
function useStreamingTool({
  cmd,
  buildArgs,
}: UseStreamingToolOptions): ToolController {
  const { lines, error, start, reset } = useStreaming(formatStreamMessage);
  const [loading, setLoading] = useState(false);
  const opIdRef = useRef(0);
  const cmdIdRef = useRef<string | null>(null);

  const run = () => {
    const id = generateId();
    cmdIdRef.current = id;
    const opId = ++opIdRef.current;
    setLoading(true);
    void start(id, async (cmdId, ch) => {
      await invoke(cmd, buildArgs({ cmdId, channel: ch }));
    }).finally(() => {
      // 竞态保护：期间若有新的 stop/run，opId 已递增，不再改写 loading
      if (opIdRef.current === opId) setLoading(false);
    });
  };

  const stop = () => {
    const cid = cmdIdRef.current;
    cmdIdRef.current = null;
    opIdRef.current++;
    if (cid) {
      void invoke("cancel_net_cmd", { cmdId: cid }).catch(() => {
        // 取消失败不阻塞复位（服务端可能已结束），仅记录控制台日志
        logger.error("Network", `cancel_net_cmd failed for ${cid}`);
      });
    }
    setLoading(false);
    reset();
  };

  const toggle = () => {
    if (loading) stop();
    else run();
  };

  return { loading, lines, error, run, stop, toggle, clear: reset };
}

/**
 * 端口检测工具控制器 — 非流式（fetch HEAD 探测），实现与流式工具同构的 ToolController。
 * 目标/端口通过 getter 注入，避免在 hook 内引用组件状态。
 */
function usePortTool(opts: {
  getTarget: () => string;
  getPort: () => number;
}): ToolController {
  const [loading, setLoading] = useState(false);
  const [lines, setLines] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const opIdRef = useRef(0);

  const run = () => {
    const target = opts.getTarget();
    const port = opts.getPort();
    if (!target || !port) return;
    const opId = ++opIdRef.current;
    setLoading(true);
    setError(null);
    void (async () => {
      // 5s 超时探测（与网站测试共用 headProbe）
      const ok = await headProbe(`http://${target}:${port}`, 5000);
      const d = new Date();
      const tag = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}:${String(d.getSeconds()).padStart(2, "0")}`;
      setLines((prev) => [
        ...prev,
        `[${tag}]  端口 ${target}:${port} ${ok ? "✓ 开放" : "✗ 不可达"}`,
      ]);
      // 竞态保护：期间若有新的 stop/run，opId 已递增，不再改写 loading
      if (opIdRef.current === opId) setLoading(false);
    })();
  };

  const stop = () => {
    opIdRef.current++;
    setLoading(false);
  };

  const clear = () => {
    setLines([]);
    setError(null);
  };

  return {
    loading,
    lines,
    error,
    run,
    stop,
    toggle: () => {
      if (loading) stop();
      else run();
    },
    clear,
  };
}

/** 工具参数描述：数据驱动渲染（取代 renderParams 巨型 switch） */
type ParamDef =
  | {
      type: "num";
      label: string;
      value: number;
      set: (v: number) => void;
      min: number;
      max: number;
      placeholder?: string;
      disabled: boolean;
    }
  | {
      type: "select";
      label: string;
      value: string;
      set: (v: string) => void;
      options: readonly { value: string; label: string }[];
      disabled: boolean;
    }
  | {
      type: "text";
      label: string;
      value: string;
      set: (v: string) => void;
      placeholder?: string;
      disabled: boolean;
    }
  | {
      type: "dns";
      label: string;
      value: string;
      set: (v: string) => void;
      disabled: boolean;
    }
  | {
      type: "switch";
      label: string;
      checked: boolean;
      set: (v: boolean) => void;
      disabled: boolean;
    };

// 参数描述工厂函数（紧凑写法，语义同 ParamDef 各变体）
const num = (
  label: string,
  value: number,
  set: (v: number) => void,
  min: number,
  max: number,
  disabled: boolean,
  placeholder?: string,
): ParamDef => ({ type: "num", label, value, set, min, max, disabled, placeholder });
const sel = (
  label: string,
  value: string,
  set: (v: string) => void,
  options: readonly { value: string; label: string }[],
  disabled: boolean,
): ParamDef => ({ type: "select", label, value, set, options, disabled });
const txt = (
  label: string,
  value: string,
  set: (v: string) => void,
  placeholder: string,
  disabled: boolean,
): ParamDef => ({ type: "text", label, value, set, placeholder, disabled });
const dns = (
  label: string,
  value: string,
  set: (v: string) => void,
  disabled: boolean,
): ParamDef => ({ type: "dns", label, value, set, disabled });
const swt = (
  label: string,
  checked: boolean,
  set: (v: boolean) => void,
  disabled: boolean,
): ParamDef => ({ type: "switch", label, checked, set, disabled });

export default function NetworkPage() {
  // ── 当前选中工具 ──
  const [activeTool, setActiveTool] = useState<ToolId>("ping");

  // ── 全局共享控件 ──
  const [sharedTarget, setSharedTarget] = useState("bilibili.com");
  // 公共端口（Port 检测使用，默认 443）
  const [sharedPort, setSharedPort] = useState(443);
  const [sharedIpVersion, setSharedIpVersion] = useState<"auto" | "v4" | "v6">(
    "auto",
  );
  const [sharedProto, setSharedProto] = useState<"icmp" | "tcp" | "udp">(
    "icmp",
  );

  // ── Ping 参数 ──
  const [pingCount, setPingCount] = useState(4);
  const [pingInterval, setPingInterval] = useState(1000);
  const [continuousPing, setContinuousPing] = useState(false);
  const [pingSize, setPingSize] = useState(56);
  const [pingTtl, setPingTtl] = useState(64);
  const [pingDeadline, setPingDeadline] = useState(0);

  // ── Traceroute 参数 ──
  const [traceMaxHops, setTraceMaxHops] = useState(30);
  // Traceroute DNS 查询："system"=本机 DNS，或 DNS IP（预设/自定义共用同一状态）
  const [traceDns, setTraceDns] = useState<string>("system");

  // ── Nslookup 参数 ──
  const [nslType, setNslType] = useState("A");

  // ── NAT 参数 ──
  const [stunServer, setStunServer] = useState(STUN_SERVERS[0]);
  const [stunCustom, setStunCustom] = useState("");

  // ── iperf3 参数 ──
  const [iperfPort, setIperfPort] = useState(5201);
  const [iperfDuration, setIperfDuration] = useState(10);

  // ── 6 个网络工具控制器（统一骨架：loading / 输出 / 竞态 / 取消）──
  const ping = useStreamingTool({
    cmd: "ping_stream",
    buildArgs: ({ cmdId, channel }) => ({
      target: sharedTarget,
      count: pingCount,
      intervalMs: pingInterval,
      cmdId,
      continuous: continuousPing,
      ipVersion: sharedIpVersion,
      proto: sharedProto,
      size: pingSize,
      ttl: pingTtl,
      deadline: pingDeadline,
      channel,
    }),
  });
  const trace = useStreamingTool({
    cmd: "traceroute_stream",
    buildArgs: ({ cmdId, channel }) => ({
      target: sharedTarget,
      maxHops: traceMaxHops,
      cmdId,
      ipVersion: sharedIpVersion,
      proto: sharedProto,
      // 本机 DNS（"system"/空）传 null，其余传自定义或预设 IP
      dnsServer: !traceDns || traceDns === "system" ? null : traceDns.trim(),
      channel,
    }),
  });
  const nsl = useStreamingTool({
    cmd: "nslookup_stream",
    buildArgs: ({ cmdId, channel }) => ({
      domain: sharedTarget,
      recordType: nslType,
      cmdId,
      channel,
    }),
  });
  const nat = useStreamingTool({
    cmd: "detect_nat_type_stream",
    buildArgs: ({ cmdId, channel }) => ({
      server: stunCustom || stunServer,
      cmdId,
      channel,
    }),
  });
  const iperf = useStreamingTool({
    cmd: "iperf3_stream",
    buildArgs: ({ cmdId, channel }) => ({
      host: sharedTarget || "127.0.0.1",
      port: iperfPort,
      duration: iperfDuration,
      cmdId,
      channel,
    }),
  });
  // 端口检测为非流式（fetch HEAD 探测），控制器与流式工具同构
  const port = usePortTool({
    getTarget: () => sharedTarget,
    getPort: () => sharedPort,
  });
  const controllers: Record<ToolId, ToolController> = {
    ping,
    trace,
    nsl,
    nat,
    port,
    iperf,
  };
  // 当前工具控制器（渲染分发统一走这里，取代 runActiveTool / activeState 两个 switch）
  const activeCtrl = controllers[activeTool];

  // ── 网站测试（自 Dashboard 迁移）──
  const [customSites, setCustomSites] = useState<SiteItem[]>(() => {
    try {
      return JSON.parse(localStorage.getItem("secm_sites") || "[]");
    } catch {
      return [];
    }
  });
  const [siteStatus, setSiteStatus] = useState<
    Record<string, { ok: boolean; ms: number } | null>
  >({});
  const [siteTesting, setSiteTesting] = useState<Record<string, boolean>>({});
  const [editingSite, setEditingSite] = useState<{
    idx: number;
    name: string;
    url: string;
  } | null>(null);
  const [addingSite, setAddingSite] = useState(false);
  const [newName, setNewName] = useState("");
  const [newUrl, setNewUrl] = useState("");
  const [autoTest, setAutoTest] = useState(false);
  const [autoTestInterval, setAutoTestInterval] = useState(5000);
  const allSites = [...DEFAULT_SITES, ...customSites];

  // ── DHCP 服务器检测 ──
  const [dhcpState, setDhcpState] = useState<DhcpState>({ status: "idle" });

  // ── DHCP 深度检查 ──
  const [deepDhcpState, setDeepDhcpState] = useState<DeepDhcpState>({
    status: "idle",
  });

  /** 发起 DHCP 深度检查（探测 + 逐台 ping + 网段/网关拓扑判定） */
  const runDeepDhcpCheck = async () => {
    if (deepDhcpState.status === "loading") return;
    setDeepDhcpState({ status: "loading" });
    try {
      const result = await invoke<DhcpDeepCheckResult>("deep_check_dhcp_servers");
      setDeepDhcpState({ status: "done", result });
    } catch (e) {
      setDeepDhcpState({ status: "error", message: String(e) });
    }
  };

  /** 发起 DHCP 服务器探测（按钮防重复：loading 时禁用） */
  const runDhcpProbe = async () => {
    if (dhcpState.status === "loading") return;
    setDhcpState({ status: "loading" });
    try {
      const result = await invoke<DhcpProbeResult>("check_dhcp_servers");
      setDhcpState({ status: "done", result });
    } catch (e) {
      setDhcpState({ status: "error", message: String(e) });
    }
  };
  const saveSites = (sites: SiteItem[]) => {
    setCustomSites(sites);
    localStorage.setItem("secm_sites", JSON.stringify(sites));
  };
  const testSite = async (url: string) => {
    setSiteTesting((t) => ({ ...t, [url]: true }));
    const start = performance.now();
    // 统一探测逻辑：fetch HEAD + 8s 超时（与端口检测共用 headProbe）
    const ok = await headProbe(url, 8000);
    const ms = Math.round(performance.now() - start);
    setSiteStatus((s) => ({
      ...s,
      [url]: ok ? { ok: true, ms } : { ok: false, ms: 0 },
    }));
    setSiteTesting((t) => ({ ...t, [url]: false }));
  };
  const testAllSites = () => {
    allSites.forEach((s) => testSite(s.url));
  };

  // 自动检测定时器
  useEffect(() => {
    if (!autoTest || allSites.length === 0) return;
    testAllSites();
    const timer = setInterval(() => testAllSites(), autoTestInterval);
    return () => clearInterval(timer);
  }, [autoTest, autoTestInterval, allSites.length]);

  // ── 全局禁用判定（任一工具运行中，共享输入禁用）──
  const globalLoading = Object.values(controllers).some((c) => c.loading);

  // ── 工具执行与展示统一走 controllers 映射（见上方 activeCtrl）──

  // ── 各工具参数描述（数据驱动渲染，取代 renderParams 巨型 switch）──
  const toolParams: Record<ToolId, ParamDef[]> = {
    ping: [
      num("次数", pingCount, setPingCount, 1, 9999, ping.loading || continuousPing),
      num("间隔ms", pingInterval, setPingInterval, 10, 60000, ping.loading || continuousPing),
      num("字节", pingSize, setPingSize, 32, 65507, ping.loading),
      num("TTL", pingTtl, setPingTtl, 1, 255, ping.loading),
      num("截止s", pingDeadline, setPingDeadline, 0, 3600, ping.loading, "不限"),
      swt("连续", continuousPing, setContinuousPing, ping.loading),
    ],
    trace: [
      num("最大跳数", traceMaxHops, setTraceMaxHops, 1, 64, trace.loading),
      dns("DNS 查询", traceDns, setTraceDns, trace.loading),
    ],
    nsl: [
      sel(
        "记录类型",
        nslType,
        setNslType,
        [
          { value: "A", label: "A (IPv4)" },
          { value: "AAAA", label: "AAAA (IPv6)" },
        ],
        nsl.loading,
      ),
    ],
    nat: [
      sel(
        "STUN 服务器",
        stunServer,
        setStunServer,
        STUN_SERVERS.map((s) => ({ value: s, label: s })),
        nat.loading,
      ),
      txt("自定义", stunCustom, setStunCustom, "host:port", nat.loading),
    ],
    // 端口为公共参数，已移到共享输入区，无需单独渲染
    port: [],
    iperf: [
      num("端口", iperfPort, setIperfPort, 1, 65535, iperf.loading),
      num("秒数", iperfDuration, setIperfDuration, 1, 120, iperf.loading),
    ],
  };

  /** 当前工具专用参数区（统一渲染 ParamDef 描述） */
  const renderParams = () =>
    toolParams[activeTool].map((p) => {
      switch (p.type) {
        case "num":
          return (
            <NumInput
              key={p.label}
              label={p.label}
              value={p.value}
              onChange={p.set}
              min={p.min}
              max={p.max}
              placeholder={p.placeholder}
              disabled={p.disabled}
            />
          );
        case "select":
          return (
            <Field key={p.label}>
              <FieldLabel>{p.label}</FieldLabel>
              <Select value={p.value} onValueChange={p.set} disabled={p.disabled}>
                <SelectTrigger className="h-8 w-full text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {p.options.map((o) => (
                    <SelectItem key={o.value} value={o.value}>
                      {o.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
          );
        case "text":
          return (
            <Field key={p.label}>
              <FieldLabel>{p.label}</FieldLabel>
              <Input
                className="h-8 w-full"
                value={p.value}
                onChange={(e) => p.set(e.target.value)}
                placeholder={p.placeholder}
                disabled={p.disabled}
              />
            </Field>
          );
        case "dns":
          return (
            <Field key={p.label} className="sm:col-span-2">
              <FieldLabel>{p.label}</FieldLabel>
              <DnsSelect value={p.value} onChange={p.set} disabled={p.disabled} />
            </Field>
          );
        case "switch":
          return (
            <div key={p.label} className="flex items-center gap-2 pt-4">
              <Switch
                checked={p.checked}
                onCheckedChange={p.set}
                disabled={p.disabled}
              />
              <span className="text-xs text-muted-foreground">{p.label}</span>
            </div>
          );
      }
    });

  return (
    <div className="flex flex-col gap-4">
          {/* 页头：垂直结构（PageHeader 契约：标题 + 描述 + 可选操作区）；flex 布局下自然独占一行 */}
          <PageHeader
            title="网络诊断"
            description="选择工具 → 配置参数 → 开始诊断，输出实时流式展示"
          />

          {/* DHCP 服务器检测 / 网站测试：左右两列并排（工具类诊断区） */}
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-4 items-start">
            {/* ── DHCP 服务器检测（本地 DHCP 冲突检测）── */}
            <Card className="animate-fade-in-up stagger-2">
              <CardHeader className="pb-2">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <div className="size-8 shrink-0 flex items-center justify-center rounded-md p-1.5 bg-cyan-500/10">
                      <Server className="size-4 text-cyan-400" />
                    </div>
                    <div>
                      <CardTitle className="text-sm">DHCP 服务器检测</CardTitle>
                      <CardDescription className="text-xs">
                        检测当前子网内能响应 Discover 的 DHCP 服务器，排查 DHCP 冲突
                      </CardDescription>
                    </div>
                  </div>
                  <div className="flex items-center gap-2">
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={runDeepDhcpCheck}
                      disabled={deepDhcpState.status === "loading"}
                    >
                      {deepDhcpState.status === "loading" ? (
                        <>
                          <Loader2 className="size-3.5 animate-spin" />
                          深度检查中
                        </>
                      ) : (
                        <>
                          <Search className="size-3.5" />
                          深度检查
                        </>
                      )}
                    </Button>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={runDhcpProbe}
                      disabled={dhcpState.status === "loading"}
                    >
                      {dhcpState.status === "loading" ? (
                        <>
                          <Loader2 className="size-3.5 animate-spin" />
                          检测中
                        </>
                      ) : (
                        <>
                          <Zap className="size-3.5" />
                          检测
                        </>
                      )}
                    </Button>
                  </div>
                </div>
              </CardHeader>
              <CardContent className="pt-0">
                {dhcpState.status === "idle" && (
                  <p className="text-xs text-muted-foreground">
                    未检测。点击「检测」向当前子网广播 DHCP Discover（约 3-4 秒），统计能响应的 DHCP 服务器数量；1 台为正常，多台可能冲突。
                  </p>
                )}
                {dhcpState.status === "loading" && (
                  <div className="flex items-center gap-2 text-xs text-muted-foreground">
                    <Loader2 className="size-3.5 animate-spin text-cyan-400" />
                    正在广播 DHCP Discover 并收集响应…
                  </div>
                )}
                {dhcpState.status === "done" && (
                  <DhcpResultView result={dhcpState.result} />
                )}
                {dhcpState.status === "error" && (
                  <ErrorBanner title="检测失败" message={dhcpState.message} />
                )}
                {/* 深度检查结果区（与基础检测独立状态） */}
                {deepDhcpState.status === "loading" && (
                  <div className="flex items-center gap-2 text-xs text-muted-foreground mt-2">
                    <Loader2 className="size-3.5 animate-spin text-cyan-400" />
                    正在深度检查：探测 DHCP 服务器 → 逐台 ping 可达性 → 网段/网关拓扑判定…
                  </div>
                )}
                {deepDhcpState.status === "done" && (
                  <div className="mt-2">
                    <DeepDhcpResultView result={deepDhcpState.result} />
                  </div>
                )}
                {deepDhcpState.status === "error" && (
                  <ErrorBanner
                    title="深度检查失败"
                    message={deepDhcpState.message}
                    className="mt-2"
                  />
                )}
              </CardContent>
            </Card>

              {/* ── 网站测试（自 Dashboard 迁移）── */}
            <Card className="animate-fade-in-up stagger-3">
              <CardHeader className="pb-2">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <div className="size-8 shrink-0 flex items-center justify-center rounded-md p-1.5 bg-cyan-500/10">
                      <ExternalLink className="size-4 text-cyan-400" />
                    </div>
                    <div>
                      <CardTitle className="text-sm">网站测试</CardTitle>
                      <CardDescription className="text-xs">
                        快速检测网站连通性
                      </CardDescription>
                    </div>
                  </div>
                  <div className="flex items-center gap-2 flex-wrap">
                    <Button variant="outline" size="sm" onClick={testAllSites}>
                      <Zap className="size-3.5" />
                      全部检测
                    </Button>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => setAddingSite(true)}
                    >
                      <Plus className="size-3.5" />
                      添加
                    </Button>
                    <div className="flex items-center gap-1.5 ml-2 pl-2 border-l border-border">
                      <Switch
                        checked={autoTest}
                        onCheckedChange={setAutoTest}
                        size="sm"
                      />
                      <span className="text-[11px] text-muted-foreground whitespace-nowrap">
                        {autoTest ? "自动检测" : "手动"}
                      </span>
                      {autoTest && (
                        <Select
                          value={String(autoTestInterval)}
                          onValueChange={(v) => setAutoTestInterval(Number(v))}
                        >
                          <SelectTrigger className="h-8 w-16 text-xs">
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            {[3000, 5000, 10000, 15000, 30000, 60000].map((ms) => (
                              <SelectItem key={ms} value={String(ms)}>
                                {ms >= 60000 ? `${ms / 60000}min` : `${ms / 1000}s`}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      )}
                    </div>
                  </div>
                </div>
              </CardHeader>
              <CardContent className="pt-0">
                {addingSite && (
                  <div className="flex items-center gap-2 mb-3 p-2 rounded-lg bg-muted/30">
                    <Input
                      placeholder="名称"
                      value={newName}
                      onChange={(e) => setNewName(e.target.value)}
                      className="h-7 text-xs w-24"
                    />
                    <Input
                      placeholder="https://..."
                      value={newUrl}
                      onChange={(e) => setNewUrl(e.target.value)}
                      className="h-7 text-xs flex-1"
                    />
                    <Button
                      size="sm"
                      onClick={() => {
                        if (newName && newUrl) {
                          saveSites([...customSites, { name: newName, url: newUrl }]);
                          setNewName("");
                          setNewUrl("");
                          setAddingSite(false);
                        }
                      }}
                    >
                      保存
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => {
                        setAddingSite(false);
                        setNewName("");
                        setNewUrl("");
                      }}
                    >
                      取消
                    </Button>
                  </div>
                )}
                {editingSite !== null && (
                  <div className="flex items-center gap-2 mb-3 p-2 rounded-lg bg-muted/30">
                    <Input
                      placeholder="名称"
                      value={editingSite.name}
                      onChange={(e) =>
                        setEditingSite({ ...editingSite, name: e.target.value })
                      }
                      className="h-7 text-xs w-24"
                    />
                    <Input
                      placeholder="https://..."
                      value={editingSite.url}
                      onChange={(e) =>
                        setEditingSite({ ...editingSite, url: e.target.value })
                      }
                      className="h-7 text-xs flex-1"
                    />
                    <Button
                      size="sm"
                      onClick={() => {
                        if (editingSite.name && editingSite.url) {
                          const updated = [...customSites];
                          updated[editingSite.idx] = {
                            name: editingSite.name,
                            url: editingSite.url,
                          };
                          saveSites(updated);
                          setEditingSite(null);
                        }
                      }}
                    >
                      保存
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => setEditingSite(null)}
                    >
                      取消
                    </Button>
                  </div>
                )}
                <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 gap-2">
                  {allSites.map((site, idx) => {
                    const status = siteStatus[site.url];
                    const testing = siteTesting[site.url];
                    const isDefault = idx < DEFAULT_SITES.length;
                    const customIdx = idx - DEFAULT_SITES.length;
                    return (
                      <div
                        key={site.url}
                        onClick={() => testSite(site.url)}
                        className={cn(
                          "flex flex-col gap-1.5 px-3 py-2.5 rounded-lg border transition-colors cursor-pointer",
                          testing
                            ? "border-blue-500/30 bg-blue-500/5"
                            : status?.ok
                              ? "border-green-500/30 bg-green-500/5"
                              : status && !status.ok
                                ? "border-red-500/30 bg-red-500/5"
                                : "border-border bg-muted/20 hover:bg-muted/30",
                        )}
                      >
                        <div className="flex items-center justify-between">
                          <span className="text-sm font-medium">{site.name}</span>
                          {!isDefault && (
                            <div className="flex items-center gap-0.5">
                              <button
                                onClick={(e) => {
                                  e.stopPropagation();
                                  setEditingSite({
                                    idx: customIdx,
                                    name: site.name,
                                    url: site.url,
                                  });
                                }}
                                className="p-0.5 rounded hover:bg-muted"
                              >
                                <Pencil className="size-3.5 text-muted-foreground" />
                              </button>
                              <button
                                onClick={(e) => {
                                  e.stopPropagation();
                                  const updated = customSites.filter(
                                    (_, i) => i !== customIdx,
                                  );
                                  saveSites(updated);
                                }}
                                className="p-0.5 rounded hover:bg-muted"
                              >
                                <Trash2 className="size-3.5 text-muted-foreground" />
                              </button>
                            </div>
                          )}
                        </div>
                        <div className="text-[11px] text-muted-foreground truncate">
                          {site.url}
                        </div>
                        <div className="flex items-center gap-1.5">
                          {testing ? (
                            <Badge
                              variant="outline"
                              className="border-blue-500/30 bg-blue-500/10 text-blue-400"
                            >
                              <Loader2 className="size-3 animate-spin" />
                              检测中
                            </Badge>
                          ) : status?.ok ? (
                            <Badge
                              variant="outline"
                              className="border-green-500/30 bg-green-500/10 text-green-400 gap-1.5"
                            >
                              <span className="size-1.5 rounded-full bg-green-400" />
                              <span className="font-mono">{status.ms}ms</span>
                            </Badge>
                          ) : status && !status.ok ? (
                            <Badge
                              variant="outline"
                              className="border-red-500/30 bg-red-500/10 text-red-400 gap-1.5"
                            >
                              <span className="size-1.5 rounded-full bg-red-400" />
                              不可达
                            </Badge>
                          ) : (
                            <span className="text-xs text-muted-foreground">
                              点击检测
                            </span>
                          )}
                        </div>
                      </div>
                    );
                  })}
                </div>
              </CardContent>
            </Card>
          </div>

          {/* ── 网络工具：工具选择 + 参数配置 + 实时输出（融合单一卡片）── */}
            <Card className="animate-fade-in-up stagger-4 flex flex-col">
              <CardHeader className="pb-2">
                {/* 标题 + 统一开始/停止按钮独占一行，工具选择器另起一行全宽等宽排布 */}
                <div className="flex flex-col gap-2">
                  <div className="flex items-center justify-between gap-2">
                    <div className="flex items-center gap-2">
                      <div className="size-8 shrink-0 flex items-center justify-center rounded-md p-1.5 bg-blue-500/10">
                        <Network className="size-4 text-blue-400" />
                      </div>
                      <div>
                        <CardTitle className="text-sm">网络工具</CardTitle>
                      </div>
                    </div>
                    {/* 统一开始/停止（标题栏右侧，所有工具共用） */}
                    <Button
                      className="h-8 shrink-0"
                      variant={activeCtrl.loading ? "destructive" : "default"}
                      onClick={() => activeCtrl.toggle()}
                      disabled={
                        !activeCtrl.loading && activeTool === "port" && !sharedTarget
                      }
                    >
                      {activeCtrl.loading ? (
                        <>
                          <Square className="size-4" />
                          停止
                        </>
                      ) : (
                        <>
                          <Play className="size-4" />
                          {activeTool === "nsl"
                            ? "查询"
                            : activeTool === "nat"
                              ? "检测"
                              : activeTool === "iperf"
                                ? "测试"
                                : "开始"}
                        </>
                      )}
                    </Button>
                  </div>
                  {/* 工具选择器：完全复刻「环境监测-游戏环境」卡片样式（透明容器 + h-7 圆角按钮 + 激活橙色高亮，换行无滚动条） */}
                  <Tabs
                    value={activeTool}
                    onValueChange={(v) => v && setActiveTool(v as ToolId)}
                  >
                    <TabsList className="h-auto w-full justify-start gap-1 flex-wrap bg-transparent p-0">
                      {TOOLS.map((t) => {
                        const TIcon = t.icon;
                        const isActive = activeTool === t.id;
                        const toolLoading = controllers[t.id].loading;
                        return (
                          <TabsTrigger
                            key={t.id}
                            value={t.id}
                            className="h-7 flex-none rounded text-xs data-[state=active]:bg-orange-500/20 data-[state=active]:text-orange-400"
                          >
                            <TIcon
                              className={cn(
                                "size-3.5",
                                isActive ? t.color : "text-muted-foreground",
                              )}
                            />
                            {t.label}
                            {toolLoading && (
                              <span className="w-2 h-2 rounded-full bg-blue-400 animate-pulse" />
                            )}
                          </TabsTrigger>
                        );
                      })}
                    </TabsList>
                  </Tabs>
                </div>
              </CardHeader>
              <CardContent className="pt-0 flex-1 flex flex-col space-y-3">
                {/* 共用输入：目标地址 / 端口 / 网络栈+协议 同行紧凑排布（窄屏时 flex-wrap 兜底） */}
                <div className="flex items-end gap-2.5 flex-wrap">
                  {/* 目标地址（NAT 使用 STUN 服务器，提示占位） */}
                  <Field className="flex-1 min-w-[100px]">
                    <FieldLabel>
                      {activeTool === "nat"
                        ? "STUN 服务器（下方配置）"
                        : "目标地址（域名/IP）"}
                    </FieldLabel>
                    <Input
                      className="h-8 w-full"
                      value={sharedTarget}
                      onChange={(e) => setSharedTarget(e.target.value)}
                      placeholder={
                        activeTool === "nat"
                          ? "NAT 检测使用 STUN 服务器"
                          : "8.8.8.8 或 google.com"
                      }
                      disabled={globalLoading || activeTool === "nat"}
                    />
                  </Field>
                  {/* 端口（公共参数，Port 检测使用） */}
                  <Field className="w-20 shrink-0">
                    <FieldLabel>端口</FieldLabel>
                    <Input
                      type="number"
                      min={1}
                      max={65535}
                      className="h-8 w-full"
                      value={sharedPort}
                      onChange={(e) =>
                        setSharedPort(parseInt(e.target.value) || 0)
                      }
                      disabled={globalLoading}
                    />
                  </Field>
                  {/* 网络栈 + 协议：组合为不可拆分单元，保证始终同行相邻排列 */}
                  <div className="flex items-end gap-3 shrink-0">
                    <Field className="w-auto">
                      <FieldLabel>网络栈</FieldLabel>
                      <IpToggle
                        value={sharedIpVersion}
                        onChange={setSharedIpVersion as (v: string) => void}
                        disabled={globalLoading}
                      />
                    </Field>
                    <Field className="w-auto">
                      <FieldLabel>协议</FieldLabel>
                      <ProtoToggle
                        value={sharedProto}
                        onChange={setSharedProto as (v: string) => void}
                        disabled={globalLoading}
                      />
                    </Field>
                  </div>
                </div>

                {/* 当前工具参数区 */}
                <div
                  key={activeTool}
                  className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-2 animate-fade-in"
                >
                  {renderParams()}
                </div>
                {/* 单一输出区 */}
                <DiagOutput
                  lines={activeCtrl.lines}
                  error={activeCtrl.error}
                  loading={activeCtrl.loading}
                  onClear={activeCtrl.clear}
                />
              </CardContent>
            </Card>
        </div>
      );
    }
