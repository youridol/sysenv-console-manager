import { HardDrive, AlertTriangle, Thermometer, Clock3, Gauge } from "lucide-react";
import { cn, formatBytes } from "../../lib/utils";
import { Card, CardContent } from "../ui/card";
import { Badge } from "../ui/badge";
import {
  HEALTH_META,
  formatPowerOnHours,
  type DiskSummary,
  type DiskHealthStatus,
} from "../../lib/diskSmart";

/** 卡片上最多展示的告警条数（其余折叠进 "+N 条"） */
const CARD_MAX_ALERTS = 2;

/**
 * 异常态外描边 ring 颜色（与 HEALTH_META 语义色对齐；
 * diskSmart.ts 的 borderClass 是 border-* 类，ring-1 需要 ring-* 类，故在本文件内映射）
 */
const RING_CLASS: Record<DiskHealthStatus, string> = {
  Good: "ring-green-500/40",
  Warning: "ring-yellow-500/50",
  Critical: "ring-red-500/60",
  Unknown: "ring-border",
};

interface DiskSmartCardProps {
  disk: DiskSummary;
  /** 点击卡片回调（打开详情弹窗） */
  onOpen: (disk: DiskSummary) => void;
}

/**
 * 磁盘 S.M.A.R.T 卡片（收起状态）。
 *
 * 收起态只展示简略信息：型号、健康状态、关键告警、温度/通电时间；
 * 健康度异常（Warning/Critical）时卡片边框着色 + 顶部色条 + 告警文案醒目提醒。
 * 点击整卡打开详情弹窗。
 */
export function DiskSmartCard({ disk, onOpen }: DiskSmartCardProps) {
  const meta = HEALTH_META[disk.health.status] ?? HEALTH_META.Unknown;
  const isAbnormal = disk.health.alert;

  // 关键告警：最多展示 CARD_MAX_ALERTS 条，其余折叠计数
  const visibleAlerts = disk.health.alerts.slice(0, CARD_MAX_ALERTS);
  const hiddenCount = disk.health.alerts.length - visibleAlerts.length;

  return (
    <Card
      className={cn(
        "group cursor-pointer transition-colors hover:bg-accent/50",
        // 健康度异常：ring 模拟外描边（ring-1 不参与布局，避免 border-2 引起的 1px 跳动）
        isAbnormal && cn("ring-1", RING_CLASS[disk.health.status]),
      )}
      onClick={() => onOpen(disk)}
      role="button"
      tabIndex={0}
      aria-label={`查看磁盘 ${disk.model} 的 S.M.A.R.T 详情`}
      onKeyDown={(e: React.KeyboardEvent) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpen(disk);
        }
      }}
    >
      {/* 顶部色条：异常时醒目标识（健康状态色） */}
      {isAbnormal && (
        <div
          className={cn("h-1 rounded-t-lg", meta.topBarClass)}
          aria-hidden
        />
      )}

      <CardContent className="p-4 pt-3.5">
        {/* 型号 + 健康徽标 */}
        <div className="flex items-start gap-2.5">
          <div
            className={cn(
              "size-8 shrink-0 flex items-center justify-center rounded-md p-1.5",
              isAbnormal ? meta.bgClass : "bg-muted/40",
            )}
          >
            <HardDrive
              className={cn("size-4", isAbnormal ? meta.textClass : "text-muted-foreground")}
            />
          </div>
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-1.5">
              <h3 className="text-sm font-semibold truncate min-w-0">
                {disk.model || "未知型号"}
              </h3>
              <Badge
                variant="outline"
                className={cn("shrink-0 text-[11px] px-1.5 py-0", meta.badgeClass)}
              >
                {disk.health.label}
              </Badge>
            </div>
            <p className="text-[11px] text-muted-foreground mt-0.5 truncate">
              {disk.interface_type || "未知接口"} · {disk.media_type || "未知介质"} ·{" "}
              {disk.size_bytes > 0 ? formatBytes(disk.size_bytes) : "容量未知"}
            </p>
          </div>
        </div>

        {/* 关键告警（异常时醒目） */}
        {isAbnormal && disk.health.alerts.length > 0 ? (
          <div
            className={cn(
              "mt-3 rounded-md border p-2",
              "border-destructive/30 bg-destructive/10",
            )}
            role="alert"
          >
            <div className="flex items-start gap-1.5">
              <AlertTriangle className="size-3.5 text-destructive shrink-0 mt-0.5" />
              <div className="space-y-0.5 min-w-0">
                {visibleAlerts.map((a, i) => (
                  <p
                    key={`${disk.id}-alert-${i}`}
                    className="text-xs text-destructive leading-snug"
                  >
                    {a}
                  </p>
                ))}
                {hiddenCount > 0 && (
                  <p className="text-[11px] text-muted-foreground">
                    另有 {hiddenCount} 条告警，点击查看详情
                  </p>
                )}
              </div>
            </div>
          </div>
        ) : (
          // 正常状态：展示温度 / 通电时间等关键字段
          <div className="mt-3 flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
            {disk.health.temperature_c !== null && (
              <span className="flex items-center gap-1 tabular-nums">
                <Thermometer className="size-3 text-orange-400" />
                温度 {disk.health.temperature_c}°C
              </span>
            )}
            {disk.health.power_on_hours !== null && (
              <span className="flex items-center gap-1 tabular-nums">
                <Clock3 className="size-3 text-blue-400" />
                通电 {formatPowerOnHours(disk.health.power_on_hours)}
              </span>
            )}
            {disk.health.wear_level_percent !== null && (
              <span className="flex items-center gap-1 tabular-nums">
                <Gauge className="size-3 text-yellow-400" />
                磨损 {disk.health.wear_level_percent}%
              </span>
            )}
          </div>
        )}

        {/* 底部提示：点击展开详情 */}
        <p className="mt-3 text-[11px] text-muted-foreground/80 group-hover:text-foreground transition-colors">
          点击查看 S.M.A.R.T 详情
        </p>
      </CardContent>
    </Card>
  );
}
