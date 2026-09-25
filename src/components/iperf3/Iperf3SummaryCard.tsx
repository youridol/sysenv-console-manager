import {
  Activity,
  ArrowDownUp,
  ArrowLeftRight,
  ArrowRight,
  Info,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import {
  fmtBytes,
  fmtDuration,
  fmtMbpsAdaptive,
  fmtNum,
  type Iperf3DirectionSummary,
  type Iperf3Result,
} from "@/lib/iperf3";

/**
 * iperf3 结构化结果卡。
 *
 * 设计要点：
 * - **两个方向分列**（正向 / 双向反向），因为双向测试时两个方向的吞吐是独立结论，
 *   合并成一个数字会丢失信息。
 * - **缺失值显示 `—` 而非 0**：与后端「不伪造」原则一致；UDP 没有重传、
 *   TCP 一般没有抖动，混用 0 会让用户误判为"真有 0 丢包"。
 * - UDP 突出抖动/丢包，TCP 突出重传，按协议选择展示重点列。
 */
export function Iperf3SummaryCard({ result }: { result: Iperf3Result }) {
  const isUdp = result.protocol_key === "udp";
  const hasReverse =
    result.bidir_reverse_sent !== null ||
    result.bidir_reverse_received !== null;

  return (
    <div className="rounded-lg border border-border bg-muted/20 p-3 space-y-3 animate-fade-in">
      {/* 头部：目标 / 协议 / 方向 / 测试条件 / 版本 */}
      <div className="flex flex-wrap items-center gap-2">
        <Badge
          variant="outline"
          className="gap-1.5 border-blue-500/30 bg-blue-500/10 text-blue-400"
        >
          <Activity className="size-3" />
          {result.tool_version || "iperf3"}
        </Badge>
        <Badge variant="outline" className="border-border bg-background/50">
          {result.protocol}
        </Badge>
        <Badge
          variant="outline"
          className="gap-1 border-border bg-background/50"
        >
          {result.direction === "双向" ? (
            <ArrowLeftRight className="size-3" />
          ) : result.direction === "反向" ? (
            <ArrowRight className="size-3 rotate-180" />
          ) : (
            <ArrowRight className="size-3" />
          )}
          {result.direction}
        </Badge>
        <span className="text-xs text-muted-foreground font-mono">
          {result.target}:{result.port}
        </span>
        <span className="text-xs text-muted-foreground">
          条件 {result.end_condition}
          {result.parallel > 1 ? ` · ${result.parallel} 流` : ""}
        </span>
        {result.cancelled && (
          <Badge
            variant="outline"
            className="border-yellow-500/30 bg-yellow-500/10 text-yellow-400"
          >
            已取消（非失败）
          </Badge>
        )}
        {!result.json_stream && (
          <Badge
            variant="outline"
            className="border-border bg-background/50 text-muted-foreground"
            title="当前 iperf3 版本不支持 --json-stream，测试期间无实时区间，仅在结束时给出汇总"
          >
            无实时区间
          </Badge>
        )}
      </div>

      {/* 方向分列 */}
      <div
        className={cn(
          "grid gap-3",
          hasReverse ? "grid-cols-1 @3xl/main:grid-cols-2" : "grid-cols-1",
        )}
      >
        <DirectionBlock
          title={
            result.direction === "双向" ? "正向（客户端 → 服务端）" : "链路统计"
          }
          sent={result.sent}
          received={result.received}
          isUdp={isUdp}
        />
        {hasReverse && (
          <DirectionBlock
            title="反向（服务端 → 客户端）"
            sent={result.bidir_reverse_sent}
            received={result.bidir_reverse_received}
            isUdp={isUdp}
          />
        )}
      </div>

      {/* CPU 占用（iperf3 提供时展示，便于判断是否为 CPU 瓶颈） */}
      {result.cpu &&
        (result.cpu.host_total !== null ||
          result.cpu.remote_total !== null) && (
          <div className="flex flex-wrap items-center gap-3 text-[11px] text-muted-foreground border-t border-border/60 pt-2">
            <span>CPU 占用：</span>
            <span>本机 {fmtNum(result.cpu.host_total, 1)}%</span>
            <span>对端 {fmtNum(result.cpu.remote_total, 1)}%</span>
            <span className="text-muted-foreground/70">
              （接近单核 100% 说明是 CPU 瓶颈，可增加并行流）
            </span>
          </div>
        )}

      {/* 错误/告警原文 */}
      {result.messages.length > 0 && (
        <div className="border-t border-border/60 pt-2">
          <div className="flex items-start gap-1.5">
            <Info className="size-3.5 shrink-0 mt-0.5 text-amber-400" />
            <div className="flex flex-col gap-0.5 min-w-0">
              {result.messages.slice(0, 5).map((m, i) => (
                <span
                  key={`${i}-${m.slice(0, 24)}`}
                  className="text-[11px] text-muted-foreground break-all"
                >
                  {m}
                </span>
              ))}
              {result.messages.length > 5 && (
                <span className="text-[11px] text-muted-foreground/70">
                  …共 {result.messages.length} 条
                </span>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * 单个方向的统计块。
 *
 * iperf3 的 `sum_sent`（发送侧视角）与 `sum_received`（接收侧视角）会给出
 * 略有差异的数字（链路在途字节等）；两者都展示，用户才能看到真实一致性。
 */
function DirectionBlock({
  title,
  sent,
  received,
  isUdp,
}: {
  title: string;
  sent: Iperf3DirectionSummary | null;
  received: Iperf3DirectionSummary | null;
  isUdp: boolean;
}) {
  const primary = sent && sent.mbps !== null ? sent : received;
  if (!primary) return null;

  return (
    <div className="rounded-md border border-border/70 bg-background/40 p-2.5">
      <div className="flex items-center gap-1.5 mb-2">
        <ArrowDownUp className="size-3.5 text-blue-400" />
        <span className="text-xs font-medium">{title}</span>
      </div>

      {/* 主指标：吞吐 */}
      <div className="flex items-baseline gap-2 mb-2">
        <span className="text-lg font-semibold font-mono text-blue-400">
          {fmtMbpsAdaptive(primary.mbps)}
        </span>
        {primary.seconds !== null && (
          <span className="text-[11px] text-muted-foreground">
            历时 {fmtDuration(Math.round(primary.seconds))}
          </span>
        )}
      </div>

      {/* 明细网格：按协议强调不同重点 */}
      <div className="grid grid-cols-2 gap-x-3 gap-y-1 text-[11px]">
        <Row label="发送" value={fmtBytes(sent?.bytes ?? null)} />
        <Row label="接收" value={fmtBytes(received?.bytes ?? null)} />

        {isUdp ? (
          <>
            <Row
              label="抖动"
              value={
                primary.jitter_ms === null
                  ? "—"
                  : `${primary.jitter_ms.toFixed(3)} ms`
              }
            />
            <Row
              label="丢包率"
              value={
                primary.lost_percent === null
                  ? "—"
                  : `${primary.lost_percent.toFixed(2)} %`
              }
            />
            <Row
              label="丢包数"
              value={
                primary.lost_packets === null
                  ? "—"
                  : String(primary.lost_packets)
              }
            />
            <Row
              label="收发包数"
              value={primary.packets === null ? "—" : String(primary.packets)}
            />
          </>
        ) : (
          <>
            <Row
              label="重传"
              value={
                primary.retransmits === null ? "—" : String(primary.retransmits)
              }
            />
            <Row
              label="接收侧"
              value={fmtMbpsAdaptive(received?.mbps ?? null)}
            />
          </>
        )}
      </div>
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center justify-between gap-2">
      <span className="text-muted-foreground">{label}</span>
      <span className="font-mono text-foreground/90">{value}</span>
    </div>
  );
}
