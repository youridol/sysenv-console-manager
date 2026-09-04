import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  HardDrive,
  AlertTriangle,
  Gauge,
  Loader2,
  RefreshCw,
  Cpu,
} from "lucide-react";
import { cn, withTimeout } from "../../lib/utils";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "../ui/dialog";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Skeleton } from "../ui/skeleton";
import { Alert, AlertDescription, AlertTitle } from "../ui/alert";
import {
  Table,
  TableHeader,
  TableBody,
  TableHead,
  TableRow,
  TableCell,
} from "../ui/table";
import {
  HEALTH_META,
  formatPowerOnHours,
  formatDataUnits,
  formatRawValue,
  attributeStatusClass,
  attributeStatusLabel,
  criticalWarningText,
  type DiskSummary,
  type DiskSmartDetail,
} from "../../lib/diskSmart";

/** 详情查询超时（毫秒）：后端 IOCTL 挂起时避免弹窗无限 loading */
const DETAIL_TIMEOUT_MS = 8000;

/** 健康度关键指标行（详情弹窗"健康度分析"区；value 为 null 显示 —） */
interface HealthMetricRow {
  label: string;
  value: string | null;
  /** 该指标是否为异常值（高亮着色） */
  warn?: boolean;
}

interface DiskSmartDialogProps {
  /** 选中的磁盘（null = 弹窗关闭） */
  disk: DiskSummary | null;
  /** 关闭回调 */
  onClose: () => void;
}

/**
 * 磁盘 S.M.A.R.T 详情弹窗。
 *
 * 打开时通过 get_disk_smart(id) 拉取完整属性列表 + NVMe 健康日志 + 健康度分析；
 * 读取失败（权限不足/不支持）不崩页面，error 字段以 Alert 展示。
 * 支持弹窗内手动刷新（重新查询）。
 */
export function DiskSmartDialog({ disk, onClose }: DiskSmartDialogProps) {
  const [detail, setDetail] = useState<DiskSmartDetail | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 请求序号：磁盘快速切换/连点刷新时丢弃过期响应，防止旧请求覆盖新盘详情
  // （v0.21.1：此前 inFlightRef 会跳过新请求且旧响应可覆盖 →
  //   「SMART 详情完全没有改变」bug）
  const reqSeqRef = useRef(0);

  /** 加载详情（可重试）。磁盘切换时自动重新加载 */
  const load = useCallback(async () => {
    if (!disk) return;
    const seq = ++reqSeqRef.current;
    setLoading(true);
    setError(null);
    try {
      const result = await withTimeout(
        invoke<DiskSmartDetail>("get_disk_smart", {
          id: disk.id,
        }),
        DETAIL_TIMEOUT_MS,
        `S.M.A.R.T 详情查询超时（超过 ${DETAIL_TIMEOUT_MS / 1000} 秒无响应）。后端 IOCTL 可能挂起，请重试。`,
      );
      // 过期响应（期间已切换磁盘/关闭弹窗）丢弃
      if (seq !== reqSeqRef.current) return;
      setDetail(result);
    } catch (e: unknown) {
      if (seq !== reqSeqRef.current) return;
      const msg = e instanceof Error ? e.message : String(e);
      setError(`S.M.A.R.T 详情读取失败: ${msg}`);
    } finally {
      if (seq === reqSeqRef.current) setLoading(false);
    }
  }, [disk]);

  // 磁盘切换（或首次打开）时加载；关闭时清空并作废在途请求
  useEffect(() => {
    reqSeqRef.current += 1; // 作废上一轮在途请求
    if (!disk) {
      setDetail(null);
      setError(null);
      setLoading(false);
      return;
    }
    setDetail(null);
    setError(null);
    void load();
  }, [disk, load]);

  const open = disk !== null;
  const meta = disk
    ? HEALTH_META[disk.health.status] ?? HEALTH_META.Unknown
    : HEALTH_META.Unknown;

  /** 健康度分析指标行（温度/通电/重映射/待映射/磨损） */
  const metrics: HealthMetricRow[] = detail
    ? [
        {
          label: "温度",
          value:
            detail.health.temperature_c !== null
              ? `${detail.health.temperature_c}°C`
              : null,
          warn:
            detail.health.temperature_c !== null &&
            detail.health.temperature_c >= 60,
        },
        {
          label: "通电时间",
          value:
            detail.health.power_on_hours !== null
              ? formatPowerOnHours(detail.health.power_on_hours)
              : null,
        },
        {
          label: "重映射扇区",
          value:
            detail.health.reallocated_sectors !== null
              ? String(detail.health.reallocated_sectors)
              : null,
          warn:
            detail.health.reallocated_sectors !== null &&
            detail.health.reallocated_sectors > 0,
        },
        {
          label: "待映射扇区",
          value:
            detail.health.pending_sectors !== null
              ? String(detail.health.pending_sectors)
              : null,
          warn:
            detail.health.pending_sectors !== null &&
            detail.health.pending_sectors > 0,
        },
        {
          label: "磨损百分比",
          value:
            detail.health.wear_level_percent !== null
              ? `${detail.health.wear_level_percent}%`
              : null,
          warn:
            detail.health.wear_level_percent !== null &&
            detail.health.wear_level_percent >= 80,
        },
      ]
    : [];

  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      {/* 三行 grid 布局：头部 / 可滚动主体 / 底部按钮；
          不用 flex（与 DialogContent 默认 grid 冲突导致 flex-1/min-h-0 失效、
          内容溢出无法滚动 —— v0.21.1 修复） */}
      <DialogContent className="sm:max-w-2xl max-h-[85vh] grid-rows-[auto_minmax(0,1fr)_auto] gap-0 p-0 overflow-hidden">
        {disk && (
          <>
            {/* ── 弹窗头：型号 + 健康徽标 + 基本信息（grid 行 1） ── */}
            <DialogHeader className="p-5 pb-3 border-b">
              <div className="flex items-start gap-3">
                <div className={cn("p-2.5 rounded-lg shrink-0", meta.bgClass)}>
                  <HardDrive
                    className={cn("size-5", meta.textClass)}
                    aria-hidden
                  />
                </div>
                <div className="min-w-0 flex-1">
                  <DialogTitle className="text-base leading-snug break-all pr-8">
                    {disk.model || "未知型号"}
                  </DialogTitle>
                  <DialogDescription className="mt-1 text-xs">
                    {disk.interface_type || "未知接口"} ·{" "}
                    {disk.media_type || "未知介质"} · 序列号{" "}
                    {disk.serial || "无"}
                  </DialogDescription>
                </div>
                <Badge
                  variant="outline"
                  className={cn("shrink-0 text-xs", meta.badgeClass)}
                >
                  {disk.health.label}
                </Badge>
              </div>
            </DialogHeader>

            {/* ── 弹窗主体：可滚动区域（grid 行 2；属性表较长时内部滚动） ── */}
            <div className="overflow-y-auto min-h-0 overscroll-contain">
              <div className="p-5 space-y-4">
                {/* 读取失败：可读错误提示（页面不崩） */}
                {error && (
                  <Alert variant="destructive">
                    <AlertTriangle className="size-4" />
                    <AlertTitle className="text-sm">读取失败</AlertTitle>
                    <AlertDescription className="text-xs">
                      {error}
                    </AlertDescription>
                  </Alert>
                )}

                {/* 详情 error 字段：后端可读错误（权限不足/不支持等） */}
                {detail?.error && (
                  <Alert variant="destructive">
                    <AlertTriangle className="size-4" />
                    <AlertTitle className="text-sm">
                      无法读取完整 S.M.A.R.T 数据
                    </AlertTitle>
                    <AlertDescription className="text-xs">
                      {detail.error}
                    </AlertDescription>
                  </Alert>
                )}

                {loading && !detail ? (
                  /* ── 加载态：骨架屏 ── */
                  <div className="space-y-3" aria-busy="true">
                    <Skeleton className="h-20 rounded-lg" />
                    <Skeleton className="h-40 rounded-lg" />
                  </div>
                ) : (
                  <>
                    {/* ── 健康度分析：告警 + 关键指标 ── */}
                    {detail && (
                      <section
                        className="rounded-lg border p-4 space-y-3"
                        aria-label="健康度分析"
                      >
                        <h4 className="text-sm font-semibold flex items-center gap-1.5">
                          <Gauge className="size-4 text-muted-foreground" />
                          健康度分析
                        </h4>

                        {/* 告警原因列表（异常时醒目） */}
                        {detail.health.alert &&
                        detail.health.alerts.length > 0 ? (
                          <div
                            className={cn(
                              "rounded-md border p-2.5 space-y-1",
                              detail.health.status === "Critical"
                                ? "border-destructive/30 bg-destructive/10"
                                : "border-yellow-500/30 bg-yellow-500/10",
                            )}
                            role="alert"
                          >
                            {detail.health.alerts.map((a, i) => (
                              <p
                                key={`${detail.id}-detail-alert-${i}`}
                                className="text-xs text-foreground flex items-start gap-1.5"
                              >
                                <AlertTriangle
                                  className={cn(
                                    "size-3.5 shrink-0 mt-0.5",
                                    detail.health.status === "Critical"
                                      ? "text-destructive"
                                      : "text-yellow-400",
                                  )}
                                />
                                <span>{a}</span>
                              </p>
                            ))}
                          </div>
                        ) : (
                          <p className="text-xs text-green-400 flex items-center gap-1.5">
                            <span className="size-1.5 rounded-full bg-green-400/80 inline-block" />
                            当前未发现健康告警
                          </p>
                        )}

                        {/* 关键指标网格 */}
                        <div className="grid grid-cols-2 sm:grid-cols-3 gap-2">
                          {metrics.map((m) => (
                            <div
                              key={m.label}
                              className="rounded-md bg-muted/40 px-3 py-2"
                            >
                              <p className="text-[11px] text-muted-foreground">
                                {m.label}
                              </p>
                              <p
                                className={cn(
                                  "text-sm font-medium tabular-nums mt-0.5",
                                  m.warn ? "text-destructive" : "text-foreground",
                                )}
                              >
                                {m.value ?? "—"}
                              </p>
                            </div>
                          ))}
                        </div>
                      </section>
                    )}

                    {/* ── ATA 属性表（NVMe 盘为空数组则跳过） ── */}
                    {detail && detail.attributes.length > 0 && (
                      <section
                        className="rounded-lg border p-4"
                        aria-label="S.M.A.R.T 属性表"
                      >
                        <h4 className="text-sm font-semibold mb-3">
                          S.M.A.R.T 属性（{detail.attributes.length} 项）
                        </h4>
                        <Table>
                          <TableHeader>
                            <TableRow>
                              <TableHead className="w-10">ID</TableHead>
                              <TableHead>属性名</TableHead>
                              <TableHead className="text-right">当前值</TableHead>
                              <TableHead className="text-right">最差值</TableHead>
                              <TableHead className="text-right">阈值</TableHead>
                              <TableHead className="text-right">原始值</TableHead>
                              <TableHead>状态</TableHead>
                            </TableRow>
                          </TableHeader>
                          <TableBody>
                            {detail.attributes.map((attr) => (
                              <TableRow
                                key={`${detail.id}-attr-${attr.id}`}
                                className="text-xs"
                              >
                                <TableCell className="text-muted-foreground tabular-nums">
                                  0x{attr.id.toString(16).padStart(2, "0")}
                                </TableCell>
                                <TableCell className="max-w-40 truncate">
                                  {attr.name}
                                </TableCell>
                                <TableCell className="text-right tabular-nums">
                                  {attr.value}
                                </TableCell>
                                <TableCell className="text-right tabular-nums">
                                  {attr.worst}
                                </TableCell>
                                <TableCell className="text-right tabular-nums">
                                  {attr.threshold}
                                </TableCell>
                                <TableCell className="text-right tabular-nums font-mono text-[11px]">
                                  {formatRawValue(attr.raw)}
                                </TableCell>
                                <TableCell>
                                  <span
                                    className={attributeStatusClass(attr.status)}
                                  >
                                    {attributeStatusLabel(attr.status)}
                                  </span>
                                </TableCell>
                              </TableRow>
                            ))}
                          </TableBody>
                        </Table>
                      </section>
                    )}

                    {/* ── NVMe 健康日志（仅 NVMe 盘） ── */}
                    {detail?.nvme_health && (
                      <section
                        className="rounded-lg border p-4"
                        aria-label="NVMe 健康日志"
                      >
                        <h4 className="text-sm font-semibold mb-3 flex items-center gap-1.5">
                          <Cpu className="size-4 text-muted-foreground" />
                          NVMe 健康日志
                        </h4>
                        <div className="grid grid-cols-2 sm:grid-cols-3 gap-2">
                          <MetricCell
                            label="临界警告"
                            value={criticalWarningText(
                              detail.nvme_health.critical_warning,
                            )}
                            warn={detail.nvme_health.critical_warning !== 0}
                          />
                          <MetricCell
                            label="温度"
                            value={`${detail.nvme_health.temperature_c}°C`}
                            warn={detail.nvme_health.temperature_c >= 60}
                          />
                          <MetricCell
                            label="备用空间"
                            value={`${detail.nvme_health.available_spare}%`}
                            warn={
                              detail.nvme_health.available_spare <
                              detail.nvme_health.available_spare_threshold
                            }
                          />
                          <MetricCell
                            label="已用寿命"
                            value={`${detail.nvme_health.percentage_used}%`}
                            warn={detail.nvme_health.percentage_used >= 80}
                          />
                          <MetricCell
                            label="通电时间"
                            value={formatPowerOnHours(
                              detail.nvme_health.power_on_hours,
                            )}
                          />
                          <MetricCell
                            label="通电周期"
                            value={String(detail.nvme_health.power_cycles)}
                          />
                          <MetricCell
                            label="不安全关机"
                            value={String(detail.nvme_health.unsafe_shutdowns)}
                            warn={detail.nvme_health.unsafe_shutdowns >= 100}
                          />
                          <MetricCell
                            label="媒体错误"
                            value={String(detail.nvme_health.media_errors)}
                            warn={detail.nvme_health.media_errors > 0}
                          />
                          <MetricCell
                            label="数据读取"
                            value={formatDataUnits(
                              detail.nvme_health.data_units_read,
                            )}
                          />
                          <MetricCell
                            label="数据写入"
                            value={formatDataUnits(
                              detail.nvme_health.data_units_written,
                            )}
                          />
                        </div>
                      </section>
                    )}

                    {/* ── 兜底：无属性表且无 NVMe 日志且无 error（理论上不会出现） ── */}
                    {detail &&
                      detail.attributes.length === 0 &&
                      !detail.nvme_health &&
                      !detail.error && (
                        <p className="text-xs text-muted-foreground">
                          该磁盘暂无可用 S.M.A.R.T 属性数据。
                        </p>
                      )}
                  </>
                )}
              </div>
            </div>

            {/* ── 弹窗脚：刷新 + 关闭（grid 行 3） ── */}
            <div className="p-4 pt-3 border-t flex items-center justify-end gap-2 shrink-0">
              {!loading && detail && (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => void load()}
                  className="shrink-0"
                >
                  <RefreshCw className="size-3.5" />
                  刷新
                </Button>
              )}
              {loading && (
                <span className="text-xs text-muted-foreground flex items-center gap-1.5 mr-auto">
                  <Loader2 className="size-3.5 animate-spin" />
                  正在读取 S.M.A.R.T 数据…
                </span>
              )}
              <Button variant="outline" size="sm" onClick={onClose}>
                关闭
              </Button>
            </div>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

/** NVMe 健康日志指标单元格（小标签 + 值，异常高亮） */
function MetricCell({
  label,
  value,
  warn = false,
}: {
  label: string;
  value: string;
  warn?: boolean;
}) {
  return (
    <div className="rounded-md bg-muted/40 px-3 py-2">
      <p className="text-[11px] text-muted-foreground">{label}</p>
      <p
        className={cn(
          "text-sm font-medium tabular-nums mt-0.5 break-all",
          warn ? "text-destructive" : "text-foreground",
        )}
      >
        {value}
      </p>
    </div>
  );
}
