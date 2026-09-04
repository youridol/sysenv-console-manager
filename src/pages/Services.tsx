import { logger } from "../lib/logger";
import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { cn } from "../lib/utils";
import { Button } from "../components/ui/button";
import { Card, CardContent } from "../components/ui/card";
import { Input } from "../components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
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
import { PageHeader } from "../components/PageHeader";
import { CardHeaderWithIcon } from "../components/CardHeaderWithIcon";
import {
  Search,
  RefreshCw,
  Play,
  Square,
  Power,
  CheckCircle2,
  XCircle,
} from "lucide-react";

/** 系统服务信息（与 Rust 侧 list_all_services 返回结构 serde 契约一致） */
interface ServiceInfo {
  name: string;
  display_name: string;
  status: string;
  start_type: string;
}

export default function Services() {
  const [services, setServices] = useState<ServiceInfo[]>([]);
  const [svcSearch, setSvcSearch] = useState("");
  const [svcActionLoading, setSvcActionLoading] = useState<string | null>(null);
  const [svcTypeLoading, setSvcTypeLoading] = useState<string | null>(null);

  /** 操作反馈 Toast：标题右侧渐入渐出，显示 2 秒后自动渐出消失 */
  const [toastMsg, setToastMsg] = useState<{
    id: number;
    text: string;
    leaving: boolean;
  } | null>(null);
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const pushToast = useCallback((text: string) => {
    if (toastTimer.current) clearTimeout(toastTimer.current);
    setToastMsg({ id: Date.now(), text, leaving: false });
    // 2 秒后开始渐出（450ms 动画结束后移除）
    toastTimer.current = setTimeout(() => {
      setToastMsg((m) => (m ? { ...m, leaving: true } : m));
      setTimeout(() => setToastMsg(null), 450);
    }, 2000);
  }, []);

  /** 加载全部系统服务列表（操作成功后重新枚举以刷新状态） */
  const loadServices = useCallback(async () => {
    try {
      const s = await invoke<ServiceInfo[]>("list_all_services");
      setServices(s);
    } catch (e: unknown) {
      if (import.meta.env.DEV) logger.error("Services", "load services failed:", e);
      pushToast(
        `[失败] 加载服务列表: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
  }, [pushToast]);

  useEffect(() => {
    loadServices();
  }, [loadServices]);

  /** 启动 / 停止服务（停止为破坏性操作，由 AlertDialog 确认后触发） */
  const handleServiceAction = async (
    name: string,
    action: "start" | "stop",
  ) => {
    setSvcActionLoading(name);
    try {
      const cmd = action === "start" ? "start_service" : "stop_service";
      const msg = await invoke<string>(cmd, { name });
      pushToast(`[成功] ${msg}`);
      await loadServices();
    } catch (e: unknown) {
      pushToast(
        `[失败] ${action} ${name}: ${e instanceof Error ? e.message : String(e)}`,
      );
    } finally {
      setSvcActionLoading(null);
    }
  };

  /** 设置服务启动类型（自动/手动/禁用）——禁用/手动后服务不会自动启动或重启 */
  const handleStartTypeChange = async (name: string, startType: string) => {
    setSvcTypeLoading(name);
    try {
      const msg = await invoke<string>("set_service_start_type", {
        name,
        startType,
      });
      pushToast(`[成功] ${msg}`);
      await loadServices();
    } catch (e: unknown) {
      pushToast(
        `[失败] 设置 ${name} 启动类型: ${e instanceof Error ? e.message : String(e)}`,
      );
    } finally {
      setSvcTypeLoading(null);
    }
  };

  return (
    <div className="flex flex-col gap-6">
      {/* ══════════ Header：标题 + 操作反馈 Toast（最右侧，渐入渐出） ══════════ */}
      <PageHeader
        title="服务管理"
        description={`已枚举 ${services.length} 项系统服务 · 搜索过滤 + 启动类型可设自动/手动/禁用（防止停止后自动启动，需管理员权限）`}
        actions={
          toastMsg ? (
            <div
              key={toastMsg.id}
              className={cn(
                "flex items-center gap-2 max-w-[420px] px-3 py-2 rounded-lg border shadow-lg",
                toastMsg.text.startsWith("[成功]")
                  ? "border-green-500/30 bg-green-500/10"
                  : "border-red-500/30 bg-red-500/10",
              )}
              style={{
                transition: "opacity 0.45s ease, transform 0.45s ease",
                opacity: toastMsg.leaving ? 0 : 1,
                transform: toastMsg.leaving ? "translateY(-6px)" : "none",
              }}
            >
              {toastMsg.text.startsWith("[成功]") ? (
                <CheckCircle2 className="size-4 shrink-0 text-green-400" />
              ) : (
                <XCircle className="size-4 shrink-0 text-red-400" />
              )}
              <span
                className={cn(
                  "text-xs truncate",
                  toastMsg.text.startsWith("[成功]")
                    ? "text-green-400"
                    : "text-red-400",
                )}
                title={toastMsg.text}
              >
                {toastMsg.text.replace(/^\[(成功|失败)\] /, "")}
              </span>
            </div>
          ) : undefined
        }
      />

      {/* ══════════ 服务管理卡片 ══════════ */}
      <Card className="animate-scale-in stagger-1">
        <CardHeaderWithIcon
          className="px-6 pt-6"
          icon={Power}
          iconClass="bg-purple-500/10"
          iconColor="text-purple-400"
          title="服务管理"
          description={`已枚举 ${services.length} 项系统服务 · 搜索过滤 + 启动类型可设自动/手动/禁用（防止停止后自动启动，需管理员权限）`}
        />
        <CardContent className="px-6 pb-6 flex flex-col gap-3">
          {/* 关键词搜索 */}
          <div className="relative shrink-0">
            <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-muted-foreground" />
            <Input
              value={svcSearch}
              onChange={(e) => setSvcSearch(e.target.value)}
              placeholder="搜索服务名称或显示名..."
              className="pl-8 text-xs"
            />
          </div>
          {/* 表格：独立页面使用正常文档流（不再依赖合并布局的绝对定位） */}
          <div className="overflow-x-auto">
            <table className="w-full text-sm table-auto">
              <thead>
                <tr className="border-y border-border bg-muted/40">
                  <th className="text-left px-2.5 py-1.5 font-medium text-xs text-muted-foreground uppercase tracking-wider whitespace-nowrap">
                    服务名称
                  </th>
                  <th className="text-left px-2.5 py-1.5 font-medium text-xs text-muted-foreground uppercase tracking-wider whitespace-nowrap">
                    显示名称
                  </th>
                  <th className="text-center px-2.5 py-1.5 font-medium text-xs text-muted-foreground uppercase tracking-wider w-[90px]">
                    状态
                  </th>
                  <th className="text-left px-2.5 py-1.5 font-medium text-xs text-muted-foreground uppercase tracking-wider w-[100px]">
                    启动类型
                  </th>
                  <th className="text-right px-2.5 py-1.5 font-medium text-xs text-muted-foreground uppercase tracking-wider w-[70px]">
                    操作
                  </th>
                </tr>
              </thead>
              <tbody>
                {services
                  .filter(
                    (svc) =>
                      !svcSearch ||
                      svc.name
                        .toLowerCase()
                        .includes(svcSearch.toLowerCase()) ||
                      svc.display_name
                        .toLowerCase()
                        .includes(svcSearch.toLowerCase()),
                  )
                  .map((svc) => (
                    <tr
                      key={svc.name}
                      className="border-b border-border/50 hover:bg-muted/30 transition-colors"
                    >
                      <td className="px-2.5 py-1.5 font-mono text-xs whitespace-nowrap">
                        {svc.name}
                      </td>
                      <td
                        className="px-2.5 py-1.5 text-xs whitespace-nowrap max-w-[320px] truncate"
                        title={svc.display_name}
                      >
                        {svc.display_name}
                      </td>
                      <td className="px-2.5 py-1.5 text-center">
                        {/* 状态圆点：绿=运行中 红=已停止 黄=其他（悬停显示状态） */}
                        <span
                          className={cn(
                            "inline-block w-2 h-2 rounded-full",
                            svc.status === "Running" && "bg-green-400",
                            svc.status === "Stopped" && "bg-red-400",
                            svc.status !== "Running" &&
                              svc.status !== "Stopped" &&
                              "bg-yellow-400",
                          )}
                          title={svc.status}
                        />
                      </td>
                      <td className="px-2.5 py-1.5">
                        {/* 启动类型下拉：auto 自动 / manual 手动 / disabled 禁用（防自动重启） */}
                        <Select
                          value={
                            svc.start_type === "自动"
                              ? "auto"
                              : svc.start_type === "手动"
                                ? "manual"
                                : svc.start_type === "已禁用"
                                  ? "disabled"
                                  : ""
                          }
                          onValueChange={(v) =>
                            handleStartTypeChange(svc.name, v)
                          }
                          disabled={svcTypeLoading === svc.name}
                        >
                          <SelectTrigger className="h-7 w-[92px] text-xs">
                            {svcTypeLoading === svc.name ? (
                              <RefreshCw className="w-3 h-3 animate-spin" />
                            ) : (
                              <SelectValue />
                            )}
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem value="auto">自动</SelectItem>
                            <SelectItem value="manual">手动</SelectItem>
                            <SelectItem value="disabled">禁用</SelectItem>
                          </SelectContent>
                        </Select>
                      </td>
                      <td className="px-2.5 py-1.5 text-right">
                        {svc.status === "Running" ? (
                          // 停止服务为破坏性操作：AlertDialog 确认后才触发原 stop_service
                          <AlertDialog>
                            <AlertDialogTrigger asChild>
                              <Button
                                variant="ghost"
                                size="sm"
                                disabled={svcActionLoading === svc.name}
                                className="h-7 text-xs text-red-400 hover:text-red-300 hover:bg-red-500/10"
                              >
                                {svcActionLoading === svc.name ? (
                                  <RefreshCw className="w-3 h-3 animate-spin" />
                                ) : (
                                  <Square className="w-3 h-3" />
                                )}
                              </Button>
                            </AlertDialogTrigger>
                            <AlertDialogContent>
                              <AlertDialogHeader>
                                <AlertDialogTitle>
                                  停止服务 {svc.name}
                                </AlertDialogTitle>
                                <AlertDialogDescription>
                                  将停止系统服务「{svc.display_name}
                                  」。部分服务停止后可能导致相关功能不可用，需管理员权限，请确认后继续。
                                </AlertDialogDescription>
                              </AlertDialogHeader>
                              <AlertDialogFooter>
                                <AlertDialogCancel>取消</AlertDialogCancel>
                                <AlertDialogAction
                                  onClick={() =>
                                    handleServiceAction(svc.name, "stop")
                                  }
                                >
                                  确认停止
                                </AlertDialogAction>
                              </AlertDialogFooter>
                            </AlertDialogContent>
                          </AlertDialog>
                        ) : (
                          <Button
                            variant="ghost"
                            size="sm"
                            onClick={() =>
                              handleServiceAction(svc.name, "start")
                            }
                            disabled={svcActionLoading === svc.name}
                            className="h-7 text-xs text-green-400 hover:text-green-300 hover:bg-green-500/10"
                          >
                            {svcActionLoading === svc.name ? (
                              <RefreshCw className="w-3 h-3 animate-spin" />
                            ) : (
                              <Play className="w-3 h-3" />
                            )}
                          </Button>
                        )}
                      </td>
                    </tr>
                  ))}
              </tbody>
            </table>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
