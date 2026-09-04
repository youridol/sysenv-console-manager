import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  RefreshCw,
  Loader2,
  AlertTriangle,
  Database,
} from "lucide-react";
import { cn, withTimeout } from "../lib/utils";
import { Card, CardContent } from "../components/ui/card";
import { Skeleton } from "../components/ui/skeleton";
import { Button } from "../components/ui/button";
import { Alert, AlertDescription, AlertTitle } from "../components/ui/alert";
import { Badge } from "../components/ui/badge";
import { DiskSmartCard } from "../components/disk/DiskSmartCard";
import { DiskSmartDialog } from "../components/disk/DiskSmartDialog";
import type { DiskSummary } from "../lib/diskSmart";
import { PageHeader } from "@/components/PageHeader";

/** 列表查询超时（毫秒）：后端 IOCTL 挂起时避免页面无限 loading */
const LIST_TIMEOUT_MS = 8000;

/** 磁盘自动刷新间隔（毫秒）：磁盘数量/健康度变化时卡片自动增减（不进 1s 轮询热路径） */
const REFRESH_INTERVAL_MS = 30000;

/** 是否运行在 Tauri 桌面环境（浏览器 dev 无 invoke，给出明确提示） */
function isTauriEnv(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * 硬件检测页 — 磁盘 S.M.A.R.T 卡片视图。
 *
 * 数据源：
 * - 磁盘：Tauri 命令 `list_disks`（S5 铁律：前端走 invoke IPC）
 * - 每块物理磁盘一张卡片：收起态只显示型号/健康状态/关键告警；点击弹出 S.M.A.R.T 详情
 * - 磁盘列表每 30 秒自动刷新（数量/健康度变化时卡片自动增减），组件卸载时清理定时器
 *
 * 注：GPU/CPU 实时传感器已从本页移除（实时数据展示见「硬件信息」首页）。
 */
export default function Hardware() {
  const [disks, setDisks] = useState<DiskSummary[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [lastTime, setLastTime] = useState<string | null>(null);
  const [selected, setSelected] = useState<DiskSummary | null>(null);
  const inFlightRef = useRef(false);

  /** 加载磁盘列表（含健康度摘要）。silent=true 用于自动轮询，不切换 loading 态避免闪烁 */
  const load = useCallback(async (silent = false) => {
    if (inFlightRef.current) return;
    inFlightRef.current = true;
    if (!silent) setLoading(true);
    try {
      if (!isTauriEnv()) {
        throw new Error(
          "当前为浏览器开发环境，无 Tauri IPC。请在 SysEnv Console Manager 桌面应用中打开本页面，或通过后端 SECM_DISK_MOCK=1 联调。",
        );
      }
      const result = await withTimeout(
        invoke<DiskSummary[]>("list_disks"),
        LIST_TIMEOUT_MS,
        `磁盘列表查询超时（超过 ${LIST_TIMEOUT_MS / 1000} 秒无响应）。后端 IOCTL 可能挂起，请检查后重试。`,
      );
      setDisks(result);
      setError(null);
      const d = new Date();
      setLastTime(
        `${d.getHours().toString().padStart(2, "0")}:${d.getMinutes().toString().padStart(2, "0")}:${d.getSeconds().toString().padStart(2, "0")}`,
      );
    } catch (e: unknown) {
      setError(`磁盘检测失败: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      inFlightRef.current = false;
      if (!silent) setLoading(false);
    }
  }, []);

  // 首次加载 + 磁盘自动刷新（磁盘数量变化时卡片自动增减）
  useEffect(() => {
    void load();
    const timer = setInterval(() => {
      void load(true);
    }, REFRESH_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [load]);

  // 统计：总盘数 / 告警数（用于页头副标题与徽标）
  const alertCount = disks?.filter((d) => d.health.alert).length ?? 0;
  const criticalCount =
    disks?.filter((d) => d.health.status === "Critical").length ?? 0;

  return (
    <div className="flex flex-col gap-6">
      {/* ── 页头（PageHeader 契约：标题 + 描述 + 右侧操作区） ── */}
      <PageHeader
        title="硬件检测"
        description="磁盘 S.M.A.R.T 健康状态"
        actions={
          <div className="flex flex-wrap items-center gap-2">
            {disks && disks.length > 0 && (
              <>
                <Badge variant="secondary" className="shrink-0">
                  {disks.length} 块磁盘
                </Badge>
                {alertCount > 0 && (
                  <Badge
                    variant="destructive"
                    className={cn(
                      "shrink-0",
                      criticalCount > 0 &&
                        "bg-red-500/15 text-red-500 border-red-500/40",
                    )}
                  >
                    <AlertTriangle className="size-3" aria-hidden />
                    {alertCount} 块需关注
                  </Badge>
                )}
              </>
            )}
            {lastTime && (
              <span className="text-xs text-muted-foreground">
                磁盘检测 <span className="font-mono">{lastTime}</span>
              </span>
            )}
            <Button
              variant="outline"
              size="sm"
              onClick={() => void load()}
              disabled={loading}
              className="shrink-0"
            >
              {loading ? (
                <Loader2 className="size-3.5 animate-spin" />
              ) : (
                <RefreshCw className="size-3.5" />
              )}
              {loading ? "检测中..." : "重新检测"}
            </Button>
          </div>
        }
      />

      {/* ── 磁盘错误状态 ── */}
      {error && (
        <Alert variant="destructive" className="animate-fade-in">
          <AlertTriangle className="size-4" />
          <AlertTitle className="text-sm">磁盘检测失败</AlertTitle>
          <AlertDescription className="text-xs">{error}</AlertDescription>
        </Alert>
      )}

      {/* ── 磁盘加载状态（首次）：骨架卡片 ── */}
      {loading && !disks && (
        <div className="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-3 gap-4">
          {[1, 2, 3, 4].map((i) => (
            <Skeleton key={i} className="h-44 rounded-md" />
          ))}
        </div>
      )}

      {/* ── 磁盘空数据状态：无磁盘 ── */}
      {!loading && !error && disks && disks.length === 0 && (
        <Card className="animate-fade-in-up">
          <CardContent className="flex flex-col items-center justify-center py-14 gap-3 text-center">
            <div className="p-3 rounded-full bg-muted/40">
              <Database className="size-8 text-muted-foreground" aria-hidden />
            </div>
            <div>
              <p className="text-sm font-medium">未检测到物理磁盘</p>
              <p className="text-xs text-muted-foreground mt-1">
                系统未返回可用的物理磁盘，或当前环境不支持磁盘枚举。
              </p>
            </div>
            <Button variant="outline" size="sm" onClick={() => void load()}>
              <RefreshCw className="size-3.5" />
              重新检测
            </Button>
          </CardContent>
        </Card>
      )}

      {/* ── 磁盘卡片网格（每块物理磁盘一张；入场动画在父级统一编排，stagger 连续） ── */}
      {!loading && disks && disks.length > 0 && (
        <div className="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-3 gap-4">
          {disks.map((disk, i) => (
            <div
              key={`disk-${disk.id}`}
              className={cn(
                "animate-fade-in-up",
                `stagger-${Math.min(i + 1, 8)}`,
              )}
            >
              <DiskSmartCard disk={disk} onOpen={setSelected} />
            </div>
          ))}
        </div>
      )}

      {/* 底部汇总（异常时可见）：健康度说明 */}
      {disks && disks.length > 0 && alertCount === 0 && (
        <p className="text-xs text-muted-foreground">
          所有磁盘健康状态正常。点击卡片可查看完整 S.M.A.R.T 属性与健康度分析。
        </p>
      )}

      {/* ── 详情弹窗 ── */}
      <DiskSmartDialog disk={selected} onClose={() => setSelected(null)} />
    </div>
  );
}
