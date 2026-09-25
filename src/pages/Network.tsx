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
  Eraser,
  ShieldAlert,
  ChevronsUpDown,
  Check,
} from "lucide-react";
import { cn } from "../lib/utils";
import { oneLine } from "../lib/util-line";
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
import { Tabs, TabsList, TabsTrigger } from "../components/ui/tabs";
import { DiagOutput } from "@/components/DiagOutput";
import { PageHeader } from "@/components/PageHeader";
import ErrorBanner from "@/components/ErrorBanner";
import { Iperf3OptionsPanel } from "@/components/iperf3/Iperf3OptionsPanel";
import { Iperf3SummaryCard } from "@/components/iperf3/Iperf3SummaryCard";
import { Iperf3ServerCard } from "@/components/iperf3/Iperf3ServerCard";
import { useIperf3Client, useIperf3ToolInfo } from "@/hooks/useIperf3";
import { summarizeIperf3Result, type Iperf3Result } from "@/lib/iperf3";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
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

/** 单个残留注册表值（对应 Rust ResidueValue 契约） */
interface ResidueValue {
  name: string;
  value: string;
}

/** 一台"网卡已移除但注册表仍留 DHCP 记录"的接口（对应 Rust ResidueAdapter 契约） */
interface ResidueAdapter {
  guid: string;
  name: string;
  description: string;
  pnp_instance_id: string;
  values: ResidueValue[];
  dhcp_server: string | null;
}

/** DHCP 残留扫描结果（对应 Rust DhcpResidueScanResult 契约） */
interface DhcpResidueScanResult {
  adapters: ResidueAdapter[];
  value_count: number;
  checked_adapters: number;
  absent_adapters: number;
  backup_dir: string;
  note: string;
}

/** 单个接口的清理结果（对应 Rust CleanedAdapter 契约） */
interface CleanedAdapter {
  guid: string;
  name: string;
  removed: number;
}

/** DHCP 残留清理结果（对应 Rust DhcpResidueCleanResult 契约） */
interface DhcpResidueCleanResult {
  removed_values: number;
  cleaned: CleanedAdapter[];
  failed: string[];
  backup_path: string | null;
  message: string;
}

/** DHCP 残留清理状态机：idle / scanning / cleaning / done / error */
type ResidueState =
  | { status: "idle" }
  | { status: "scanning" }
  | { status: "cleaning" }
  | { status: "done"; result: DhcpResidueCleanResult }
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
  // 顺序即首选优先级：2026-09 实测可用者前置（stun.qq.com / stun.sipgate.net 已失效，
  // 置于末尾保留）。后端 detect_nat_streaming 亦会对失效服务器自动顺延。
  "stun.l.google.com:19302",
  "stun1.l.google.com:19302",
  "stun.freeswitch.org:3478",
  "global.stun.twilio.com:3478",
  "stun.miwifi.com:3478",
  "stun.nextcloud.com:3478",
  "stun.linphone.org:3478",
  "stun2.l.google.com:19302",
  "stun.ekiga.net:3478",
  "stun.3cx.com:3478",
  "stun.vivox.com:3478",
  "stun.blink.de:3478",
  "stun.counterpath.com:3478",
  "stun.qq.com:3478",
  "stun.sipgate.net:3478",
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

/**
 * HTTP 延迟探测结果（后端 `probe_url` 命令返回）
 *
 * @property reachable  是否可达（收到任何 HTTP 响应即算可达，含 4xx/5xx）
 * @property ms         **HTTP 延迟**（毫秒）：DNS + TCP + TLS 握手 + 请求/响应。
 *                      这是「HTTP 延迟测试」对外展示的唯一耗时口径。
 *                      它既不是 ICMP ping 的网络往返，也不是浏览器打开网页的
 *                      耗时（不含子资源与渲染），UI 与日志都必须标注为 HTTP 延迟。
 * @property status     HTTP 状态码；无响应时为 null
 * @property error      失败原因；可达时为 null
 */
interface UrlProbeResult {
  reachable: boolean;
  ms: number;
  status: number | null;
  error: string | null;
}

/**
 * 服务端 HTTP 延迟探测（「HTTP 延迟测试」与「端口检测」共用）。
 *
 * **为什么不在前端 fetch**：v1.19.0 收紧了 CSP `connect-src`（移除通配
 * `https:`/`http:`），发行版里 WebView2 会拦截所有外部 `fetch`，导致
 * 「全部测试」一律显示"不可达"。且用户可自定义任意站点（localStorage
 * `secm_sites`），无法用固定 CSP 白名单覆盖。改由 Rust 侧探测：
 * CSP 保持收紧，同时支持任意目标。
 *
 * 语义与旧的 fetch HEAD 方案对齐：**收到任何 HTTP 响应即视为可达**
 * （4xx/5xx 也算——服务确实在应答），只有连接失败/超时才不可达。
 *
 * 失败（invoke 本身出错）时返回 null，调用方据此显示"探测失败"而非
 * 误报"不可达"——后端不可用与站点不可达是两回事。
 */
async function probeReachable(
  url: string,
  timeoutMs: number,
): Promise<UrlProbeResult | null> {
  try {
    return await invoke<UrlProbeResult>("probe_url", { url, timeoutMs });
  } catch (e: unknown) {
    logger.warn(
      "Network",
      `探测 ${url} 失败（后端不可用）: ${e instanceof Error ? e.message : String(e)}`,
    );
    return null;
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
// 右侧栏日志（「按页输出」）— 流式工具结果的单行渲染
// ============================================================================
//
// 设计约束（后端 `log::CAPACITY = 200` 环形缓冲）：
//   - 每次运行只写 **2 条**：START + 终态（完成/失败/取消），与运行时长内的
//     包数/跳数/输出行数**无关**——绝不按事件行上报，逐行展示由页面输出区承担。
//   - 终态取自 `invoke` 返回值（后端契约里本次运行的权威结果），而非流式
//     `summary` 事件：返回值与 invoke 响应同源，不存在「summary 事件与 invoke
//     响应先后到达」的竞态，天然保证一次运行**恰好一条**结果行。
//   - 入口统一用不节流的 `logger.action`：`logger.info/warn/error` 按
//     `${级别}:${模块}` 做 5s 节流，START 与终态若走同一级别，5s 内的后一条会被
//     **静默丢弃**；而这两条每一条都对应一次真实用户操作，必须落盘。

// 此处曾有 `oneLine` 的本地副本：`src/lib/util-line.ts` 头部注释已写明该函数
// 「从 Network.tsx 提取」，但原实现未删除，两份逐字相同（连默认值 300 一致）。
// ADR-0001 D14：删除副本，统一从 `@/lib/util-line` 导入。

/** 安全读取后端返回值字段（契约固定；类型异常时回落，绝不抛异常中断日志） */
function asRecord(v: unknown): Record<string, unknown> {
  return typeof v === "object" && v !== null
    ? (v as Record<string, unknown>)
    : {};
}
function asNum(v: unknown): number {
  return typeof v === "number" && Number.isFinite(v) ? v : 0;
}
function asStr(v: unknown, fallback = "-"): string {
  return typeof v === "string" && v !== "" ? v : fallback;
}
function asStrArr(v: unknown): string[] {
  return Array.isArray(v)
    ? v.filter((x): x is string => typeof x === "string")
    : [];
}

/**
 * 单行结果渲染（右侧栏「按页输出」）。
 *
 * 每个流式工具的 invoke 返回值即该次运行的最终结果：
 * ping→PingSummary / traceroute→TraceResult / nslookup→DnsResult /
 * nat→NatDetectionResult / iperf3→stderr 摘要文本。
 * `desc` 为开始时的参数快照（NAT 结果本身不含 STUN 服务器名，需由 desc 补齐）。
 */
function formatToolResult(cmd: string, ret: unknown, desc: string): string {
  switch (cmd) {
    case "ping_stream": {
      const r = asRecord(ret);
      return `目标 ${asStr(r.target)}: 发送 ${asNum(r.sent)} / 接收 ${asNum(r.received)} / 丢包 ${asNum(r.loss_percent).toFixed(1)}% / 平均 ${asNum(r.avg_rtt_ms).toFixed(1)}ms（min ${asNum(r.min_rtt_ms).toFixed(1)} / max ${asNum(r.max_rtt_ms).toFixed(1)}）`;
    }
    case "traceroute_stream": {
      const r = asRecord(ret);
      const hops = Array.isArray(r.hops) ? r.hops.length : 0;
      return `目标 ${asStr(r.target)}: ${hops} 跳`;
    }
    case "nslookup_stream": {
      const r = asRecord(ret);
      const records = asStrArr(r.records);
      // 记录数可能很多：最多展示 8 条，其余只报总数（整行另有 oneLine 上限兜底）
      const shown = records.slice(0, 8).join(", ");
      const suffix =
        records.length > 8
          ? ` [${shown} …共 ${records.length} 条]`
          : records.length
            ? ` [${shown}]`
            : "";
      return `域名 ${asStr(r.domain)} ${asStr(r.record_type, "")}: ${records.length} 条记录${suffix}`;
    }
    case "detect_nat_type_stream": {
      const r = asRecord(ret);
      return `${desc} → ${asStr(r.nat_label)}（公网 ${asStr(r.public_ip)}:${asNum(r.public_port)}，本机 ${asStr(r.local_ip)}）`;
    }
    case "iperf3_stream": {
      // 后端返回结构化 Iperf3Result；用共享摘要函数生成单行文本
      // （旧实现返回原始 stderr，成功时几乎总是空串 → 只显示"完成"）
      const r = asRecord(ret);
      if (Object.keys(r).length === 0) return "完成";
      return summarizeIperf3Result(r as unknown as Iperf3Result);
    }
    default:
      return "完成";
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
    value !== "" &&
    value !== "system" &&
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
              {value === "system" && (
                <Check className="size-3.5 text-primary" />
              )}
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
      {result.note && (
        <p className="text-xs text-muted-foreground">{result.note}</p>
      )}
    </div>
  );
}

// ============================================================================
// DHCP 残留清理
// ============================================================================

/**
 * 清理结果视图：删除项统计 + 备份路径 + 失败列表。
 *
 * 备份路径必须展示——出问题时用户据此还原（备份文件含值级原始字节）。
 */
function ResidueCleanView({ result }: { result: DhcpResidueCleanResult }) {
  const nothingToDo = result.removed_values === 0 && result.failed.length === 0;
  return (
    <div
      className={cn(
        "flex flex-col gap-2 rounded-lg border p-3",
        result.failed.length > 0
          ? "border-yellow-500/30 bg-yellow-500/10"
          : "border-green-500/30 bg-green-500/10",
      )}
    >
      <div className="flex items-start gap-2">
        {result.failed.length > 0 ? (
          <AlertTriangle className="size-4 shrink-0 mt-0.5 text-yellow-400" />
        ) : (
          <CheckCircle2 className="size-4 shrink-0 mt-0.5 text-green-400" />
        )}
        <div className="flex flex-col gap-1.5 min-w-0">
          <span className="text-sm font-medium">
            {nothingToDo
              ? "无残留可清理"
              : `已清理 ${result.cleaned.length} 个网卡 · ${result.removed_values} 项`}
          </span>
          {result.cleaned.length > 0 && (
            <div className="flex flex-wrap gap-1.5">
              {result.cleaned.map((c) => (
                <Badge
                  key={c.guid}
                  variant="outline"
                  className="text-[11px] border-border bg-background/50"
                >
                  {c.name} · {c.removed} 项
                </Badge>
              ))}
            </div>
          )}
          {result.backup_path && (
            <p className="text-xs text-muted-foreground break-all">
              备份已保存：
              <span className="font-mono text-[11px]">
                {result.backup_path}
              </span>
            </p>
          )}
          {result.failed.length > 0 && (
            <ul className="list-disc list-inside space-y-1 text-xs text-yellow-400">
              {result.failed.slice(0, 5).map((f) => (
                <li key={f} className="break-all">
                  {f}
                </li>
              ))}
              {result.failed.length > 5 && (
                <li>另有 {result.failed.length - 5} 项失败（详见日志）</li>
              )}
            </ul>
          )}
        </div>
      </div>
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
        <Badge
          variant="outline"
          className="font-mono text-[10px] h-5 border-border bg-background/50"
        >
          {result.local_ip ?? "—"}
        </Badge>
        <span>掩码</span>
        <Badge
          variant="outline"
          className="font-mono text-[10px] h-5 border-border bg-background/50"
        >
          {result.subnet_mask ?? "—"}
        </Badge>
        <span>默认网关</span>
        <Badge
          variant="outline"
          className="font-mono text-[10px] h-5 border-border bg-background/50"
        >
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
              <p className={cn("text-xs", sevText(c.severity))}>
                {c.diagnosis}
              </p>
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
// HTTP 延迟测试（自 Dashboard 迁移）
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
  /** 右侧栏日志里的工具名（中文，如 "Ping"） */
  label: string;
  /** 运行参数快照（START 行与结果行共用；只读参数，不含 cmdId/channel） */
  describe: (args: Record<string, unknown>) => string;
}

/**
 * 流式工具控制器 — 封装 useStreaming + loading + opId 竞态 + cmdId 取消。
 * 替代 5 个流式工具（ping/trace/nsl/nat/iperf）各自手写的同构骨架。
 *
 * 日志落点（每次运行**恰好 2 条**，见上方「右侧栏日志」段注释）：
 *   1. START（`logger.action` INFO）— 紧接 invoke 之前；
 *   2. 终态（完成 INFO / 失败 ERROR）；被 `stop()` 接管时改由 stop 写「已取消」WARN。
 * 两条都用不节流的 `logger.action`：START 与终态同属 INFO，若走 `logger.info`
 * 会被 `${级别}:${模块}` 的 5s 节流丢掉其中一条。
 */
function useStreamingTool({
  cmd,
  buildArgs,
  label,
  describe,
}: UseStreamingToolOptions): ToolController {
  const { lines, error, start, reset } = useStreaming(formatStreamMessage);
  const [loading, setLoading] = useState(false);
  const opIdRef = useRef(0);
  const cmdIdRef = useRef<string | null>(null);
  /** 最近一次运行的参数快照（供 stop() 写「已取消」终态行） */
  const descRef = useRef<string | null>(null);

  const run = () => {
    const id = generateId();
    cmdIdRef.current = id;
    descRef.current = null;
    const opId = ++opIdRef.current;
    setLoading(true);
    void start(id, async (cmdId, ch) => {
      const args = buildArgs({ cmdId, channel: ch });
      const desc = describe(args);
      descRef.current = desc;
      logger.action("INFO", "Network", `${label} 开始：${oneLine(desc)}`);
      try {
        const ret = await invoke(cmd, args);
        // 竞态保护：本次运行已被 stop()/新 run 接管 → 终态由接管方负责，避免重复行
        if (opIdRef.current !== opId) return;
        descRef.current = null;
        logger.action(
          "INFO",
          "Network",
          `${label} 完成：${oneLine(formatToolResult(cmd, ret, desc))}`,
        );
      } catch (e: unknown) {
        // 上报失败行，但**必须继续抛出**：useStreaming 依赖它把错误显示到页面输出区
        if (opIdRef.current === opId) {
          descRef.current = null;
          logger.action(
            "ERROR",
            "Network",
            `${label} 失败：${oneLine(e instanceof Error ? e.message : String(e))}`,
          );
        }
        throw e;
      }
    }).finally(() => {
      // 竞态保护：期间若有新的 stop/run，opId 已递增，不再改写 loading
      if (opIdRef.current === opId) setLoading(false);
    });
  };

  const stop = () => {
    const cid = cmdIdRef.current;
    const desc = descRef.current;
    cmdIdRef.current = null;
    descRef.current = null;
    opIdRef.current++;
    if (cid) {
      // 本次运行确实在进行中：补一条终态「已取消」，避免右侧栏只留悬空的 START
      if (desc !== null) {
        logger.action("WARN", "Network", `${label} 已取消：${oneLine(desc)}`);
      }
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
      // 5s 超时探测（服务端执行，与 HTTP 延迟测试共用）
      const probe = await probeReachable(`http://${target}:${port}`, 5000);
      const ok = probe?.reachable ?? null;
      // 页面级探测结果上报日志（用户触发的探测；供右侧栏「按页输出」展示）
      // 用不节流的 `logger.action`：同一目标连续两次探测（结果同为 INFO/WARN）
      // 若走 `logger.info/warn`，5s 内的第二条会被 `${级别}:${模块}` 节流静默丢弃。
      // 每次运行**仅 1 条**（非流式，无中间进度），不会刷屏。
      if (ok === true) {
        logger.action("INFO", "Network", `端口检测 ${target}:${port}: 开放`);
      } else if (ok === false) {
        logger.action("WARN", "Network", `端口检测 ${target}:${port}: 不可达`);
      } else {
        logger.action(
          "WARN",
          "Network",
          `端口检测 ${target}:${port}: 探测失败（后端无响应）`,
        );
      }
      const d = new Date();
      const tag = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}:${String(d.getSeconds()).padStart(2, "0")}`;
      const verdict =
        ok === true ? "✓ 开放" : ok === false ? "✗ 不可达" : "⚠ 探测失败";
      setLines((prev) => [
        ...prev,
        `[${tag}]  端口 ${target}:${port} ${verdict}`,
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
): ParamDef => ({
  type: "num",
  label,
  value,
  set,
  min,
  max,
  disabled,
  placeholder,
});
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

  // ── iperf3 ──
  // 参数状态与结构化结果都在 useIperf3Client 内部（见下方 iperfTool）；
  // 工具可用性与版本能力单独查一次（二进制随包分发，运行期不变）。
  const iperfInfo = useIperf3ToolInfo();

  // ── 6 个网络工具控制器（统一骨架：loading / 输出 / 竞态 / 取消）──
  // label/describe 只用于右侧栏日志（START + 终态各一条），不参与页面输出渲染。
  const ping = useStreamingTool({
    cmd: "ping_stream",
    label: "Ping",
    describe: (a) =>
      `${sharedTarget} 次数=${a.continuous ? "连续" : a.count} 间隔=${a.intervalMs}ms 字节=${a.size} TTL=${a.ttl}${a.deadline ? ` 截止=${a.deadline}s` : ""} 网络栈=${a.ipVersion} 协议=${String(a.proto).toUpperCase()}`,
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
    label: "Traceroute",
    describe: (a) =>
      `${sharedTarget} 最大跳数=${a.maxHops} DNS=${a.dnsServer ?? "本机"} 网络栈=${a.ipVersion} 协议=${String(a.proto).toUpperCase()}`,
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
    label: "Nslookup",
    describe: (a) => `${sharedTarget} 记录类型=${a.recordType}`,
    buildArgs: ({ cmdId, channel }) => ({
      domain: sharedTarget,
      recordType: nslType,
      cmdId,
      channel,
    }),
  });
  const nat = useStreamingTool({
    cmd: "detect_nat_type_stream",
    label: "NAT",
    describe: (a) => `STUN ${a.server}`,
    buildArgs: ({ cmdId, channel }) => ({
      server: stunCustom || stunServer,
      cmdId,
      channel,
    }),
  });
  // iperf3：参数与结构化结果由专用钩子承载（见 useIperf3.ts + lib/iperf3.ts）。
  // controller 满足统一的 ToolController 契约，故开始/停止按钮、共享输入禁用、
  // 右侧栏「每次运行恰好 2 条日志」等既有行为全部保持不变。
  const iperfTool = useIperf3Client({
    getTarget: () => sharedTarget,
    getIpFamily: () => sharedIpVersion,
  });
  const iperf = iperfTool.controller;
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

  // ── HTTP 延迟测试（自 Dashboard 迁移）──
  const [customSites, setCustomSites] = useState<SiteItem[]>(() => {
    try {
      return JSON.parse(localStorage.getItem("secm_sites") || "[]");
    } catch {
      return [];
    }
  });
  /**
   * 站点 HTTP 延迟状态。
   *
   * `ms` 即后端测得的 **HTTP 延迟**（DNS + TCP + TLS 握手 + 请求/响应），
   * 是本卡片唯一展示的耗时口径。
   * `probeFailed` = 探测本身没能执行（后端无响应），与"站点不可达"区分显示，
   * 避免把基础设施故障误报成网络故障。
   */
  const [siteStatus, setSiteStatus] = useState<
    Record<
      string,
      {
        ok: boolean;
        ms: number;
        probeFailed?: boolean;
      } | null
    >
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

  // ── DHCP 残留清理 ──
  const [residueState, setResidueState] = useState<ResidueState>({
    status: "idle",
  });
  /** 清理确认对话框（破坏性注册表写入：必须先展示将删除的清单再确认） */
  const [residueConfirm, setResidueConfirm] = useState(false);
  /** 清理前的扫描结果（用于确认框展示"将删除什么"） */
  const [residueScan, setResidueScan] = useState<DhcpResidueScanResult | null>(
    null,
  );

  /**
   * 扫描 DHCP 注册表残留（只读）并弹出确认框。
   *
   * 先扫描再确认：让用户看到具体是哪张网卡、哪些值将被删除，
   * 而不是盲点「清理」。扫描结果为空时不开确认框，直接给出提示。
   */
  const openResidueConfirm = async () => {
    if (
      residueState.status === "scanning" ||
      residueState.status === "cleaning"
    )
      return;
    setResidueState({ status: "scanning" });
    try {
      const scan = await invoke<DhcpResidueScanResult>("scan_dhcp_residue");
      logger.action(
        "INFO",
        "Network",
        oneLine(
          `DHCP 残留扫描: ${scan.adapters.length} 个已移除网卡 / ${scan.value_count} 项（已检查 ${scan.checked_adapters} 个接口）`,
        ),
      );
      if (scan.adapters.length === 0) {
        setResidueScan(null);
        setResidueState({
          status: "done",
          result: {
            removed_values: 0,
            cleaned: [],
            failed: [],
            backup_path: null,
            message: scan.note,
          },
        });
        return;
      }
      setResidueScan(scan);
      setResidueState({ status: "idle" });
      setResidueConfirm(true);
    } catch (e) {
      const message = String(e);
      setResidueState({ status: "error", message });
      logger.action(
        "ERROR",
        "Network",
        `DHCP 残留扫描失败: ${oneLine(message)}`,
      );
    }
  };

  /**
   * 执行清理，随后**自动重新检测**本地 DHCP 服务器。
   *
   * 闭环要求：清理的意义就是让残留服务器从检测结果中消失，
   * 故清理完立即重新探测，用户无需再手点一次「检测」即可看到结果变化。
   */
  const runResidueClean = async () => {
    setResidueConfirm(false);
    if (residueState.status === "cleaning") return;
    setResidueState({ status: "cleaning" });
    try {
      const result = await invoke<DhcpResidueCleanResult>("clean_dhcp_residue");
      setResidueState({ status: "done", result });
      logger.action(
        "INFO",
        "Network",
        oneLine(
          `清理 DHCP 残留: 删除 ${result.removed_values} 项 / ${result.cleaned.length} 个网卡${result.failed.length > 0 ? `，失败 ${result.failed.length} 项` : ""}${result.backup_path ? `，备份 ${result.backup_path}` : ""}`,
        ),
      );
      // 清理后立刻重新检测，让用户直接看到残留服务器是否消失
      await runDhcpProbe();
    } catch (e) {
      const message = String(e);
      setResidueState({ status: "error", message });
      logger.action(
        "ERROR",
        "Network",
        `清理 DHCP 残留失败: ${oneLine(message)}`,
      );
    }
  };

  /** 发起 DHCP 深度检查（探测 + 逐台 ping + 网段/网关拓扑判定） */
  const runDeepDhcpCheck = async () => {
    if (deepDhcpState.status === "loading") return;
    setDeepDhcpState({ status: "loading" });
    // 日志入口：非流式、每次运行**恰好 1 条**终态（无 START——单次 invoke 无进度可言）。
    // 用不节流的 `logger.action`：用户可能连续点「检测」与「深度检查」，两者都成功
    // 即为两条 INFO + "Network"，走 `logger.info` 时 5s 内的第二条会被静默丢弃。
    try {
      const result = await invoke<DhcpDeepCheckResult>(
        "deep_check_dhcp_servers",
      );
      setDeepDhcpState({ status: "done", result });
      logger.action(
        "INFO",
        "Network",
        `DHCP 深度检查: ${oneLine(
          `服务器 ${result.servers.length} 台 [${result.servers.slice(0, 5).join(", ")}] 本机 ${result.local_ip ?? "-"} 网关 ${result.gateway ?? "-"} — ${result.summary}`,
        )}`,
      );
    } catch (e) {
      const message = String(e);
      setDeepDhcpState({ status: "error", message });
      logger.action(
        "ERROR",
        "Network",
        `DHCP 深度检查失败: ${oneLine(message)}`,
      );
    }
  };

  /** 发起 DHCP 服务器探测（按钮防重复：loading 时禁用） */
  const runDhcpProbe = async () => {
    if (dhcpState.status === "loading") return;
    setDhcpState({ status: "loading" });
    // 日志入口同上：1 条终态，`logger.action` 不节流（理由见 runDeepDhcpCheck）
    try {
      const result = await invoke<DhcpProbeResult>("check_dhcp_servers");
      setDhcpState({ status: "done", result });
      logger.action(
        "INFO",
        "Network",
        `DHCP 服务器检测: ${oneLine(
          `${result.count} 台 [${result.servers.slice(0, 5).join(", ")}] ${result.healthy ? "正常" : "疑似冲突"} — ${result.note}`,
        )}`,
      );
    } catch (e) {
      const message = String(e);
      setDhcpState({ status: "error", message });
      logger.action(
        "ERROR",
        "Network",
        `DHCP 服务器检测失败: ${oneLine(message)}`,
      );
    }
  };
  const saveSites = (sites: SiteItem[]) => {
    setCustomSites(sites);
    localStorage.setItem("secm_sites", JSON.stringify(sites));
  };
  /**
   * 站点 HTTP 延迟测试。
   * @param report 是否把结果上报日志。仅用户显式触发（点单个站点 / 点「全部测试」）时为 true；
   *   自动检测定时器传 false —— 否则每 autoTestInterval 会刷屏，淹没其他页面日志。
   */
  const testSite = async (url: string, report = true) => {
    setSiteTesting((t) => ({ ...t, [url]: true }));
    // 服务端 HTTP 延迟探测（HEAD 优先，GET 兜底）
    const probe = await probeReachable(url, 8000);
    // 后端不可用 ≠ 站点不可达：区分呈现，避免把基础设施故障误报成网络问题
    const ok = probe?.reachable ?? null;
    // 展示口径：**HTTP 延迟**（DNS + TCP + TLS 握手 + 请求/响应）。
    // 这不是 ICMP ping 的网络往返，也不是浏览器打开网页的耗时，
    // 日志与 tooltip 都写明是 HTTP 延迟，避免用户混淆三种口径。
    const ms = probe?.ms ?? 0;
    // 页面级探测结果上报日志（供右侧栏「按页输出」展示）
    // 用不节流的 `logger.action`：点「全部检测」会并发探测全部站点，结果同为
    // INFO/WARN；走 `logger.info/warn` 时 `${级别}:${模块}` 的 5s 节流只会留下第一条，
    // 其余站点**静默丢失**。每次运行 1 条/站点，站点数由用户定义且自动检测定时器
    // 传 report=false（见 testSite 注释），故条数有界、不会刷屏。
    if (report) {
      const siteName = allSites.find((s) => s.url === url)?.name ?? url;
      if (ok === true) {
        logger.action(
          "INFO",
          "Network",
          oneLine(
            `HTTP 延迟测试 ${siteName}: 可达 (${probe?.status ?? "-"}，HTTP 延迟 ${ms}ms)`,
          ),
        );
      } else if (ok === false) {
        logger.action(
          "WARN",
          "Network",
          oneLine(
            `HTTP 延迟测试 ${siteName}: 不可达${probe?.error ? ` — ${probe.error}` : ""}`,
          ),
        );
      } else {
        logger.action(
          "WARN",
          "Network",
          `HTTP 延迟测试 ${siteName}: 探测失败（后端无响应）`,
        );
      }
    }
    setSiteStatus((s) => ({
      ...s,
      [url]:
        ok === null
          ? // 探测本身失败：不计入可达性状态（渲染为"探测失败"，不误报不可达）
            { ok: false, ms: 0, probeFailed: true }
          : { ok, ms },
    }));
    setSiteTesting((t) => ({ ...t, [url]: false }));
  };
  const testAllSites = (report = true) => {
    allSites.forEach((s) => void testSite(s.url, report));
  };

  // 自动检测定时器（轮询结果不上报日志，避免刷屏）
  useEffect(() => {
    if (!autoTest || allSites.length === 0) return;
    testAllSites(false);
    const timer = setInterval(() => testAllSites(false), autoTestInterval);
    return () => clearInterval(timer);
  }, [autoTest, autoTestInterval, allSites.length]);

  // ── 全局禁用判定（任一工具运行中，共享输入禁用）──
  const globalLoading = Object.values(controllers).some((c) => c.loading);

  // ── 工具执行与展示统一走 controllers 映射（见上方 activeCtrl）──

  // ── 各工具参数描述（数据驱动渲染，取代 renderParams 巨型 switch）──
  const toolParams: Record<ToolId, ParamDef[]> = {
    ping: [
      num(
        "次数",
        pingCount,
        setPingCount,
        1,
        9999,
        ping.loading || continuousPing,
      ),
      num(
        "间隔ms",
        pingInterval,
        setPingInterval,
        10,
        60000,
        ping.loading || continuousPing,
      ),
      num("字节", pingSize, setPingSize, 32, 65507, ping.loading),
      num("TTL", pingTtl, setPingTtl, 1, 255, ping.loading),
      num(
        "截止s",
        pingDeadline,
        setPingDeadline,
        0,
        3600,
        ping.loading,
        "不限",
      ),
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
    // iperf3 参数由 Iperf3OptionsPanel 专门渲染（基础 + 可折叠高级，共 20+ 项）
    iperf: [],
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
              <Select
                value={p.value}
                onValueChange={p.set}
                disabled={p.disabled}
              >
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
            <Field key={p.label} className="@3xl/main:col-span-2">
              <FieldLabel>{p.label}</FieldLabel>
              <DnsSelect
                value={p.value}
                onChange={p.set}
                disabled={p.disabled}
              />
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

      {/* DHCP 服务器检测 / HTTP 延迟测试：左右两列并排（工具类诊断区） */}
      <div className="grid grid-cols-1 @3xl/main:grid-cols-2 gap-4 items-start">
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
                <Button
                  variant="outline"
                  size="sm"
                  onClick={openResidueConfirm}
                  disabled={
                    residueState.status === "scanning" ||
                    residueState.status === "cleaning"
                  }
                  title="清理已移除网卡遗留的 DHCP 注册表记录（清理后自动重新检测）"
                >
                  {residueState.status === "scanning" ||
                  residueState.status === "cleaning" ? (
                    <>
                      <Loader2 className="size-3.5 animate-spin" />
                      清理中
                    </>
                  ) : (
                    <>
                      <Eraser className="size-3.5" />
                      清理残留
                    </>
                  )}
                </Button>
              </div>
            </div>
          </CardHeader>
          <CardContent className="pt-0">
            {dhcpState.status === "idle" && (
              <p className="text-xs text-muted-foreground">
                未检测。点击「检测」向当前子网广播 DHCP Discover（约 3-4
                秒），统计能响应的 DHCP 服务器数量；1 台为正常，多台可能冲突。
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
                正在深度检查：探测 DHCP 服务器 → 逐台 ping 可达性 →
                网段/网关拓扑判定…
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
            {/* 残留清理结果区（清理后已自动重新检测，故与检测结果同屏） */}
            {residueState.status === "done" && (
              <div className="mt-2">
                <ResidueCleanView result={residueState.result} />
              </div>
            )}
            {residueState.status === "error" && (
              <ErrorBanner
                title="清理 DHCP 残留失败"
                message={residueState.message}
                className="mt-2"
              />
            )}
          </CardContent>
        </Card>

        {/* ── HTTP 延迟测试（自 Dashboard 迁移）── */}
        <Card className="animate-fade-in-up stagger-3">
          <CardHeader className="pb-2">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2">
                <div className="size-8 shrink-0 flex items-center justify-center rounded-md p-1.5 bg-cyan-500/10">
                  <ExternalLink className="size-4 text-cyan-400" />
                </div>
                <div>
                  <CardTitle className="text-sm">HTTP 延迟测试</CardTitle>
                  <CardDescription className="text-xs">
                    测量各站点 HTTP 响应延迟（DNS + TCP + TLS + 往返）
                  </CardDescription>
                </div>
              </div>
              <div className="flex items-center gap-2 flex-wrap">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => testAllSites()}
                >
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
                      saveSites([
                        ...customSites,
                        { name: newName, url: newUrl },
                      ]);
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
            <div className="grid grid-cols-2 @4xl/main:grid-cols-3 @6xl/main:grid-cols-4 gap-2">
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
                        : status?.probeFailed
                          ? "border-amber-500/30 bg-amber-500/5"
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
                      ) : status?.probeFailed ? (
                        <Badge
                          variant="outline"
                          className="border-amber-500/30 bg-amber-500/10 text-amber-400 gap-1.5"
                        >
                          <span className="size-1.5 rounded-full bg-amber-400" />
                          探测失败
                        </Badge>
                      ) : status?.ok ? (
                        <Badge
                          variant="outline"
                          className="border-green-500/30 bg-green-500/10 text-green-400 gap-1.5"
                          title={`HTTP 延迟 ${status.ms}ms（DNS + TCP + TLS 握手 + 请求/响应，HEAD 优先、GET 兜底）。该值不是 ICMP ping 的网络往返，也不含页面子资源加载与渲染。`}
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
                onChange={(e) => setSharedPort(parseInt(e.target.value) || 0)}
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
          {activeTool === "iperf" ? (
            <Iperf3OptionsPanel
              value={iperfTool.options}
              onChange={iperfTool.patch}
              disabled={iperf.loading}
              info={iperfInfo}
            />
          ) : (
            <div
              key={activeTool}
              className="grid grid-cols-2 @3xl/main:grid-cols-3 @5xl/main:grid-cols-6 gap-2 animate-fade-in"
            >
              {renderParams()}
            </div>
          )}

          {/* iperf3 工具不可用时的明确指引（缺失/版本过旧都走这里） */}
          {activeTool === "iperf" && iperfInfo && !iperfInfo.available && (
            <div className="rounded-lg border border-amber-500/30 bg-amber-500/5 p-2.5">
              <p className="text-[11px] text-amber-400 break-all">
                {iperfInfo.note}
              </p>
              <p className="text-[11px] text-muted-foreground mt-1">
                修复：确认安装包完整（随包资源 resources/iperf3/iperf3.exe 与
                cygwin1.dll 需同时存在），或自行安装 iperf3 并确保其在系统 PATH
                中。
              </p>
            </div>
          )}

          {/* 结构化结果摘要（成功运行时展示） */}
          {activeTool === "iperf" && iperfTool.lastResult && (
            <Iperf3SummaryCard result={iperfTool.lastResult} />
          )}

          {/* 单一输出区 */}
          <DiagOutput
            lines={activeCtrl.lines}
            error={activeCtrl.error}
            loading={activeCtrl.loading}
            onClear={activeCtrl.clear}
          />
        </CardContent>
      </Card>

      {/* ── iperf3 本机服务端（供对端连接测速）── */}
      <div className="stagger-5">
        <Iperf3ServerCard />
      </div>

      {/* ── DHCP 残留清理确认（破坏性注册表写入，先展示将删除的清单）── */}
      <AlertDialog open={residueConfirm} onOpenChange={setResidueConfirm}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle className="flex items-center gap-2">
              <ShieldAlert className="size-4 text-yellow-400" />
              清理 DHCP 残留
            </AlertDialogTitle>
            <AlertDialogDescription>
              以下网卡已被移除，但注册表仍保留其历史 DHCP
              记录。这些记录会被并入「DHCP
              服务器检测」的基线，导致误报幽灵服务器。确认后将删除这些记录（仅删除
              DHCP 分配与租约字段，不影响在线网卡、静态 IP / 网关 /
              DNS），删除前会自动备份原值。
            </AlertDialogDescription>
          </AlertDialogHeader>
          {residueScan && (
            <div className="flex flex-col gap-2 max-h-56 overflow-y-auto">
              {residueScan.adapters.map((a) => (
                <div
                  key={a.guid}
                  className="rounded-lg border border-border bg-muted/30 p-2.5"
                >
                  <div className="flex flex-wrap items-center gap-1.5">
                    <span className="text-xs font-medium">{a.name}</span>
                    {a.dhcp_server && (
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] h-5 border-yellow-500/30 bg-yellow-500/10 text-yellow-400"
                      >
                        DHCP {a.dhcp_server}
                      </Badge>
                    )}
                    <span className="text-[11px] text-muted-foreground">
                      {a.values.length} 项
                    </span>
                  </div>
                  <p className="mt-1 font-mono text-[10px] text-muted-foreground break-all">
                    {a.guid}
                  </p>
                  <div className="mt-1.5 flex flex-wrap gap-1">
                    {a.values.map((v) => (
                      <Badge
                        key={v.name}
                        variant="outline"
                        className="font-mono text-[10px] h-5 border-border bg-background/50"
                        title={`${v.name} = ${v.value}`}
                      >
                        {v.name}
                      </Badge>
                    ))}
                  </div>
                </div>
              ))}
            </div>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction onClick={runResidueClean}>
              确认清理（{residueScan?.value_count ?? 0} 项）
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
