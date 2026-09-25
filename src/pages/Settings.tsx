import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { cn } from "../lib/utils";
import { Button } from "../components/ui/button";
import { Card, CardContent } from "../components/ui/card";
import { Switch } from "../components/ui/switch";
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
} from "../components/ui/alert-dialog";
import {
  Monitor,
  Gamepad2,
  Mouse,
  MonitorPlay,
  CheckCircle2,
  XCircle,
  Zap,
  Cog,
  ShieldAlert,
  Timer,
  Cpu,
  Joystick,
  Trash2,
  Loader2,
} from "lucide-react";
import { PageHeader } from "../components/PageHeader";
import { Alert, AlertDescription, AlertTitle } from "../components/ui/alert";
import { SectionTitle } from "../components/SectionTitle";
import { logger } from "../lib/logger";

interface SettingState {
  name: string;
  enabled: boolean;
  admin_required: boolean;
  message: string;
}

interface PowerPlan {
  guid: string;
  name: string;
  is_active: boolean;
}

interface HeteroPolicies {
  supported: boolean;
  thread_ac: number | null;
  thread_dc: number | null;
  short_ac: number | null;
  short_dc: number | null;
}

/* 异类调度策略取值（与电源选项面板一致） */
const HETERO_OPTIONS = [
  { value: 0, label: "所有处理器" },
  { value: 1, label: "高性能处理器" },
  { value: 2, label: "首选高性能处理器" },
  { value: 3, label: "高效处理器" },
  { value: 4, label: "首选高效处理器" },
  { value: 5, label: "自动" },
] as const;

const HETERO_AC_DEFAULT = 5; // 自动（未显式设置时的展示回退）

/** NVIDIA 电源管理模式（与 Rust 侧 NvidiaPowerMode serde snake_case 契约一致） */
type NvidiaPmMode = "optimal" | "max_performance" | "adaptive";

const NVIDIA_PM_OPTIONS: { value: NvidiaPmMode; label: string }[] = [
  { value: "optimal", label: "最佳功率" },
  { value: "max_performance", label: "最高性能优先" },
  { value: "adaptive", label: "自适应" },
];

/* ── Settings Row ── */
function SettingRow({
  icon: Icon,
  iconColor,
  iconBg,
  title,
  adminRequired,
  description,
  checked,
  onToggle,
  loading,
}: {
  icon: React.FC<{ className?: string }>;
  iconColor: string;
  iconBg: string;
  title: string;
  adminRequired?: boolean;
  description?: string;
  /** 当前开关值；`null` 表示状态未知（读取失败），不得当作 false 渲染。 */
  checked: boolean | null;
  onToggle: (v: boolean) => void;
  loading?: boolean;
}) {
  // 状态未知（读取失败）：显式呈现，避免把 null 当 false 显示成「全部关闭」，
  // 那会让用户误以为真实设置已关闭并据此做出反向操作（ADR-0001 D9）。
  const unknown = checked === null;
  return (
    <div className="flex items-center justify-between gap-4 py-3 px-4 rounded-lg bg-muted/30 hover:bg-muted/50 transition-colors">
      <div className="flex items-center gap-3 min-w-0">
        <div className={cn("p-1.5 rounded-md shrink-0", iconBg)}>
          <Icon className={cn("size-4", iconColor)} />
        </div>
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span className="text-sm font-medium">{title}</span>
            {adminRequired && (
              <span className="text-[10px] text-yellow-400 flex items-center gap-0.5">
                <ShieldAlert className="size-3" />
                管理员
              </span>
            )}
          </div>
          {description !== undefined && (
            <p
              className={cn(
                "text-xs truncate",
                unknown ? "text-destructive" : "text-muted-foreground",
              )}
              title={loading || unknown ? undefined : description}
            >
              {loading
                ? "读取中..."
                : unknown
                  ? "读取失败：请检查权限或传感器服务，点击开关可重试"
                  : description}
            </p>
          )}
        </div>
      </div>
      <Switch
        checked={checked === true}
        onCheckedChange={onToggle}
        disabled={loading || unknown}
      />
    </div>
  );
}

export default function Settings() {
  const [hags, setHags] = useState<SettingState | null>(null);
  const [gameMode, setGameMode] = useState<SettingState | null>(null);
  const [gameOpt, setGameOpt] = useState<SettingState | null>(null);
  const [mousePrec, setMousePrec] = useState<SettingState | null>(null);
  const [vrr, setVrr] = useState<SettingState | null>(null);
  /** 高精度计时器开关状态（enabled = 已关闭，需重启生效） */
  const [hpt, setHpt] = useState<SettingState | null>(null);
  const [plans, setPlans] = useState<PowerPlan[]>([]);
  const [hetero, setHetero] = useState<HeteroPolicies | null>(null);
  const [heteroSaving, setHeteroSaving] = useState<"thread" | "short" | null>(
    null,
  );
  /** 操作反馈 Toast：标题右侧渐入渐出，显示 2 秒后自动渐出消失 */
  const [toastMsg, setToastMsg] = useState<{
    id: number;
    text: string;
    leaving: boolean;
  } | null>(null);
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** 首次加载设置失败的原因；非空时页面顶部显示可重试的横幅。 */
  const [loadError, setLoadError] = useState<string | null>(null);

  const pushToast = useCallback((text: string) => {
    if (toastTimer.current) clearTimeout(toastTimer.current);
    setToastMsg({ id: Date.now(), text, leaving: false });
    // 2 秒后开始渐出（450ms 动画结束后移除）
    toastTimer.current = setTimeout(() => {
      setToastMsg((m) => (m ? { ...m, leaving: true } : m));
      setTimeout(() => setToastMsg(null), 450);
    }, 2000);
  }, []);

  const [loadingToggle, setLoadingToggle] = useState<string | null>(null);

  /** NVIDIA 电源管理模式（serde snake_case 契约，无 NVIDIA 卡时为 null + 错误态） */
  const [nvidiaPm, setNvidiaPm] = useState<NvidiaPmMode | null>(null);
  const [nvidiaPmError, setNvidiaPmError] = useState<string | null>(null);
  const [nvidiaPmSaving, setNvidiaPmSaving] = useState(false);

  const loadSettings = async () => {
    try {
      const [h, gm, go, m, v, hptState, p, hp] = await Promise.all([
        invoke<SettingState>("get_hags_state"),
        invoke<SettingState>("get_game_mode_state"),
        invoke<SettingState>("get_game_optimization_state"),
        invoke<SettingState>("get_mouse_precision_state"),
        invoke<SettingState>("get_vrr_state"),
        invoke<SettingState>("get_hpt_state"),
        invoke<PowerPlan[]>("get_power_plans"),
        invoke<HeteroPolicies>("get_hetero_policies"),
      ]);
      setHags(h);
      setGameMode(gm);
      setGameOpt(go);
      setMousePrec(m);
      setVrr(v);
      setHpt(hptState);
      setPlans(p);
      setHetero(hp);
      setLoadError(null);
    } catch (e: unknown) {
      // 读取失败必须给用户可见反馈（ADR-0001 D9）：原实现只在 DEV 记日志，
      // 生产环境静默，且各开关保持 null → 渲染成「全部关闭」，
      // 用户会误以为真实设置已关闭并据此执行反向操作。
      const msg = e instanceof Error ? e.message : String(e);
      logger.error("Settings", "Settings load error:", msg);
      setLoadError(msg);
      pushToast(`[失败] 读取设置失败: ${msg}`);
    }
    // NVIDIA 电源管理模式：独立加载（无 NVIDIA 显卡时置错误态，不阻塞其他设置）
    invoke<NvidiaPmMode>("get_nvidia_power_mode")
      .then((m) => {
        setNvidiaPm(m);
        setNvidiaPmError(null);
      })
      .catch((e: unknown) => {
        setNvidiaPm(null);
        setNvidiaPmError(e instanceof Error ? e.message : String(e));
      });
  };

  useEffect(() => {
    loadSettings();
  }, []);

  // 自动检测并导入"卓越性能"计划
  useEffect(() => {
    if (plans.length === 0) return;
    const hasUltimate = plans.some((p) => p.name === "卓越性能");
    if (!hasUltimate) {
      invoke<string>("enable_ultimate_performance")
        .then(() => refreshPowerPlans())
        .catch((e: unknown) => {
          if (import.meta.env.DEV)
            logger.warn("Settings", "auto-import ultimate performance:", e);
        });
    } else {
      // 已存在则自动切换为卓越性能
      const ultimatePlan = plans.find((p) => p.name === "卓越性能");
      if (ultimatePlan && !ultimatePlan.is_active) {
        handlePowerPlan(ultimatePlan.guid);
      }
    }
  }, [plans.length]);

  const toggleSetting = async (
    name: string,
    enabled: boolean,
    getCmd: string,
    setCmd: string,
  ) => {
    if (loadingToggle) return;
    setLoadingToggle(name);
    try {
      await invoke<SettingState>(setCmd, { enabled });
      const confirmed = await invoke<SettingState>(getCmd);
      switch (name) {
        case "hags":
          setHags(confirmed);
          break;
        case "gameMode":
          setGameMode(confirmed);
          break;
        case "gameOpt":
          setGameOpt(confirmed);
          break;
        case "mousePrec":
          setMousePrec(confirmed);
          break;
        case "vrr":
          setVrr(confirmed);
          break;
        case "hpt":
          setHpt(confirmed);
          break;
        default:
          break;
      }
    } catch (e: unknown) {
      const err = e instanceof Error ? e.message : String(e);
      if (import.meta.env.DEV)
        logger.error("Settings", `Settings toggle ${name} failed:`, err);
      pushToast(`[失败] ${name}: ${err}`);
      loadSettings();
    } finally {
      setLoadingToggle(null);
    }
  };

  /** 切换 NVIDIA 电源模式（写入 DRS 后立即重读验证，UI 与真实值同步） */
  const handleNvidiaPmChange = async (v: string) => {
    if (nvidiaPmSaving) return;
    setNvidiaPmSaving(true);
    setNvidiaPmError(null);
    try {
      await invoke("set_nvidia_power_mode", { mode: v });
      // 写入后立即重读：后端已校验持久化，此处以真实值同步 UI（防驱动拒绝导致界面与实况不符）
      const confirmed = await invoke<NvidiaPmMode>("get_nvidia_power_mode");
      setNvidiaPm(confirmed);
      const label =
        NVIDIA_PM_OPTIONS.find((o) => o.value === confirmed)?.label ??
        confirmed;
      pushToast(`[成功] NVIDIA 电源管理模式已切换为「${label}」`);
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setNvidiaPmError(msg);
      pushToast(`[失败] NVIDIA 电源管理模式: ${msg}`);
      // 回滚 UI：重读真实状态
      try {
        const cur = await invoke<NvidiaPmMode>("get_nvidia_power_mode");
        setNvidiaPm(cur);
      } catch {
        // 读取也失败时保持原值（不覆盖错误态）
      }
    } finally {
      setNvidiaPmSaving(false);
    }
  };

  const [powSwitching, setPowSwitching] = useState<string | null>(null);

  const refreshPowerPlans = async () => {
    try {
      const p = await invoke<PowerPlan[]>("get_power_plans");
      setPlans(p);
    } catch (e: unknown) {
      if (import.meta.env.DEV)
        logger.warn("Settings", "refreshPowerPlans failed:", e);
    }
  };

  const handlePowerPlan = async (guid: string) => {
    if (powSwitching) return;
    setPowSwitching(guid);
    try {
      await invoke("set_power_plan", { guid });
      const p = await invoke<PowerPlan[]>("get_power_plans");
      setPlans(p);
      pushToast(`[成功] 已切换电源计划`);
    } catch (e: unknown) {
      pushToast(
        `[失败] 电源计划切换: ${e instanceof Error ? e.message : String(e)}`,
      );
    } finally {
      setPowSwitching(null);
    }
  };

  const [powDeleting, setPowDeleting] = useState<string | null>(null);
  const [confirmDeletePlan, setConfirmDeletePlan] = useState<PowerPlan | null>(
    null,
  );

  /** 删除电源计划（激活中的计划前端禁用，后端 PowerDeleteScheme 亦有兜底） */
  const handleDeletePowerPlan = async (guid: string) => {
    if (powDeleting) return;
    setPowDeleting(guid);
    setConfirmDeletePlan(null);
    try {
      await invoke("delete_power_plan", { guid });
      const p = await invoke<PowerPlan[]>("get_power_plans");
      setPlans(p);
      pushToast(`[成功] 已删除电源计划`);
    } catch (e: unknown) {
      pushToast(
        `[失败] 电源计划删除: ${e instanceof Error ? e.message : String(e)}`,
      );
    } finally {
      setPowDeleting(null);
    }
  };

  const handleHeteroChange = async (
    kind: "thread" | "short",
    value: number,
  ) => {
    if (heteroSaving) return;
    setHeteroSaving(kind);
    try {
      await invoke("set_hetero_policy", { kind, value });
      const h = await invoke<HeteroPolicies>("get_hetero_policies");
      setHetero(h);
      pushToast(
        `[成功] 已设置${kind === "thread" ? "异类线程调度策略" : "异类短运行线程调度策略"}`,
      );
    } catch (e: unknown) {
      pushToast(
        `[失败] 设置${kind === "thread" ? "异类线程调度策略" : "异类短运行线程调度策略"}: ${
          e instanceof Error ? e.message : String(e)
        }`,
      );
    } finally {
      setHeteroSaving(null);
    }
  };

  return (
    <div className="flex flex-col gap-6">
      {/* ══════════ Header：标题 + 操作反馈 Toast（最右侧，渐入渐出） ══════════ */}
      <PageHeader
        title="系统设置"
        description="硬件加速 · 电源计划"
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

      {/* 加载失败横幅（ADR-0001 D9）：读取任一设置失败时可见，并可一键重试。
          没有它时页面只会呈现一排「关闭」的开关，把基础设施故障伪装成正常状态。 */}
      {loadError && (
        <Alert variant="destructive" className="mb-4">
          <AlertTitle>设置读取失败</AlertTitle>
          <AlertDescription className="flex items-center justify-between gap-3">
            <span className="min-w-0 break-all">
              {loadError}
              <span className="block text-xs opacity-80">
                下方开关处于「读取失败」状态，不代表真实设置值。
              </span>
            </span>
            <Button
              variant="outline"
              size="sm"
              className="shrink-0"
              onClick={() => {
                setLoadError(null);
                void loadSettings();
              }}
            >
              重试
            </Button>
          </AlertDescription>
        </Alert>
      )}

      {/* ══════════ 第一行：系统优化开关 / 电源计划（两列并排） ══════════ */}
      <div className="grid grid-cols-1 @2xl/main:grid-cols-2 gap-6 items-stretch">
        {/* 左卡：系统优化开关 */}
        <Card className="animate-scale-in stagger-1 h-full">
          <CardContent className="px-6 py-6">
            <div className="space-y-1 min-w-0">
              <div className="flex items-center gap-2 mb-3">
                <div className="p-1.5 rounded-md bg-blue-500/10">
                  <Cog className="w-5 h-5 text-blue-400" />
                </div>
                <div>
                  <h3 className="text-sm font-semibold">系统优化开关</h3>
                  <p className="text-xs text-muted-foreground">
                    管理 Windows 游戏和性能相关设置
                  </p>
                </div>
              </div>
              {/* 游戏类子分组（5 项） */}
              <SectionTitle icon={Gamepad2} title="游戏类" />
              <SettingRow
                icon={Monitor}
                iconColor="text-blue-400"
                iconBg="bg-blue-500/10"
                title="GPU 硬件加速调度"
                adminRequired={hags?.admin_required}
                description="使用 GPU 调度器管理显存，降低渲染延迟"
                checked={hags ? hags.enabled : null}
                loading={loadingToggle === "hags"}
                onToggle={(v) =>
                  toggleSetting("hags", v, "get_hags_state", "set_hags_state")
                }
              />
              <SettingRow
                icon={Joystick}
                iconColor="text-orange-400"
                iconBg="bg-orange-500/10"
                title="游戏模式"
                description="通过在后台关闭内容来优化电脑玩游戏"
                checked={gameMode ? gameMode.enabled : null}
                loading={loadingToggle === "gameMode"}
                onToggle={(v) =>
                  toggleSetting(
                    "gameMode",
                    v,
                    "get_game_mode_state",
                    "set_game_mode_state",
                  )
                }
              />
              <SettingRow
                icon={Gamepad2}
                iconColor="text-green-400"
                iconBg="bg-green-500/10"
                title="窗口化游戏优化"
                description="提升窗口化 / 无边框游戏流畅度"
                checked={gameOpt ? gameOpt.enabled : null}
                loading={loadingToggle === "gameOpt"}
                onToggle={(v) =>
                  toggleSetting(
                    "gameOpt",
                    v,
                    "get_game_optimization_state",
                    "set_game_optimization",
                  )
                }
              />
              <SettingRow
                icon={Mouse}
                iconColor="text-purple-400"
                iconBg="bg-purple-500/10"
                title="鼠标精准度"
                description="提高指针精确度（鼠标加速），FPS 玩家通常关闭"
                checked={mousePrec ? mousePrec.enabled : null}
                loading={loadingToggle === "mousePrec"}
                onToggle={(v) =>
                  toggleSetting(
                    "mousePrec",
                    v,
                    "get_mouse_precision_state",
                    "set_mouse_precision",
                  )
                }
              />
              <SettingRow
                icon={MonitorPlay}
                iconColor="text-cyan-400"
                iconBg="bg-cyan-500/10"
                title="可变刷新率"
                adminRequired={vrr?.admin_required}
                description="显示器动态刷新率，消除画面撕裂"
                checked={vrr ? vrr.enabled : null}
                loading={loadingToggle === "vrr"}
                onToggle={(v) =>
                  toggleSetting("vrr", v, "get_vrr_state", "set_vrr_state")
                }
              />
              {/* 系统类子分组（1 项）：border-t 分隔 + 小标题 */}
              <div className="pt-3 mt-2 border-t border-border">
                <SectionTitle icon={Timer} title="系统类" />
              </div>
              <SettingRow
                icon={Timer}
                iconColor="text-red-400"
                iconBg="bg-red-500/10"
                title="关闭高精度计时器"
                adminRequired={hpt?.admin_required}
                description="降低计时器中断频率，减少 CPU 占用（需重启生效）"
                checked={hpt ? hpt.enabled : null}
                loading={loadingToggle === "hpt"}
                onToggle={(v) =>
                  toggleSetting("hpt", v, "get_hpt_state", "set_hpt_state")
                }
              />
            </div>
          </CardContent>
        </Card>

        {/* 右卡：电源计划（含大小核调度 + NVIDIA 显卡电源管理） */}
        <Card className="animate-scale-in stagger-2 h-full">
          <CardContent className="px-6 py-6">
            <div className="space-y-2 min-w-0">
              <div className="flex items-center gap-2 mb-3">
                <div className="p-1.5 rounded-md bg-yellow-500/10">
                  <Zap className="w-5 h-5 text-yellow-400" />
                </div>
                <div>
                  <h3 className="text-sm font-semibold">电源计划</h3>
                  <p className="text-xs text-muted-foreground">
                    性能电源计划 &middot; 大小核调度 &middot; NVIDIA 电源管理
                  </p>
                </div>
              </div>
              {/* ══════════ 电源计划列表 ══════════ */}
              <div className="space-y-2">
                {plans
                  .filter((p) => !p.name.toLowerCase().includes("overlay"))
                  .map((plan) => (
                    <div
                      key={plan.guid}
                      className={cn(
                        "flex items-center justify-between gap-3 rounded-lg border px-3 py-2.5 transition-colors",
                        plan.is_active
                          ? "border-yellow-500/50 bg-yellow-500/10"
                          : "border-border bg-muted/30 hover:bg-muted/50",
                      )}
                    >
                      <span
                        className={cn(
                          "text-sm font-medium truncate",
                          plan.is_active && "text-yellow-400",
                        )}
                        title={plan.name}
                      >
                        {plan.name}
                      </span>
                      {/* 右侧：删除（非激活计划）+ 激活开关 */}
                      <div className="flex items-center gap-1 shrink-0">
                        <Button
                          variant="ghost"
                          size="icon"
                          className="h-7 w-7 text-muted-foreground hover:text-red-400 hover:bg-red-500/10"
                          disabled={plan.is_active || powDeleting !== null}
                          title={
                            plan.is_active
                              ? "当前激活中的计划不可删除，请先切换其他计划"
                              : "删除该电源计划"
                          }
                          onClick={() => setConfirmDeletePlan(plan)}
                        >
                          {powDeleting === plan.guid ? (
                            <Loader2 className="w-3.5 h-3.5 animate-spin" />
                          ) : (
                            <Trash2 className="w-3.5 h-3.5" />
                          )}
                        </Button>
                        {/* 滑动开关切换电源计划 */}
                        <Switch
                          checked={plan.is_active}
                          onCheckedChange={() => {
                            if (!plan.is_active) handlePowerPlan(plan.guid);
                          }}
                          disabled={powSwitching !== null}
                        />
                      </div>
                    </div>
                  ))}

                {/* 删除电源计划二次确认（防误删；激活计划由后端 API 兜底拒绝） */}
                <AlertDialog
                  open={confirmDeletePlan !== null}
                  onOpenChange={(open) => {
                    if (!open) setConfirmDeletePlan(null);
                  }}
                >
                  <AlertDialogContent>
                    <AlertDialogHeader>
                      <AlertDialogTitle>删除电源计划</AlertDialogTitle>
                      <AlertDialogDescription>
                        确定要删除电源计划「{confirmDeletePlan?.name}」吗？
                        此操作不可恢复（需重新创建）。
                      </AlertDialogDescription>
                    </AlertDialogHeader>
                    <AlertDialogFooter>
                      <AlertDialogCancel disabled={powDeleting !== null}>
                        取消
                      </AlertDialogCancel>
                      <AlertDialogAction
                        className="bg-red-600 hover:bg-red-700 text-white"
                        disabled={powDeleting !== null}
                        onClick={() => {
                          if (confirmDeletePlan) {
                            void handleDeletePowerPlan(confirmDeletePlan.guid);
                          }
                        }}
                      >
                        删除
                      </AlertDialogAction>
                    </AlertDialogFooter>
                  </AlertDialogContent>
                </AlertDialog>

                {/* 异类线程调度策略（大小核 CPU） */}
                <div className="border-t border-border pt-3 mt-2 space-y-3">
                  {/* 标题样式与 NVIDIA 显卡电源管理模式统一（图标容器 + 图标大小 + 字体） */}
                  <div className="flex items-center gap-2">
                    <div className="p-1.5 rounded-md bg-yellow-500/10 shrink-0">
                      <Cpu className="w-5 h-5 text-yellow-400" />
                    </div>
                    <h3 className="text-sm font-semibold">
                      核心策略（Amd/Intel 适用）
                    </h3>
                  </div>
                  {!hetero ? (
                    <p className="text-xs text-muted-foreground/70">
                      读取中...
                    </p>
                  ) : !hetero.supported ? (
                    <p className="text-xs text-muted-foreground/70">
                      当前设备不支持（非混合架构 CPU）
                    </p>
                  ) : (
                    <>
                      <div className="flex items-center justify-between gap-3">
                        <span className="text-xs text-muted-foreground shrink-0">
                          异类线程调度策略
                        </span>
                        <Select
                          value={String(hetero.thread_ac ?? HETERO_AC_DEFAULT)}
                          onValueChange={(v) =>
                            handleHeteroChange("thread", Number(v))
                          }
                          disabled={heteroSaving !== null}
                        >
                          <SelectTrigger className="h-8 w-[170px] text-xs">
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            {HETERO_OPTIONS.map((opt) => (
                              <SelectItem
                                key={opt.value}
                                value={String(opt.value)}
                              >
                                {opt.label}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      </div>
                      <div className="flex items-center justify-between gap-3">
                        <span className="text-xs text-muted-foreground shrink-0">
                          异类短运行线程调度策略
                        </span>
                        <Select
                          value={String(hetero.short_ac ?? HETERO_AC_DEFAULT)}
                          onValueChange={(v) =>
                            handleHeteroChange("short", Number(v))
                          }
                          disabled={heteroSaving !== null}
                        >
                          <SelectTrigger className="h-8 w-[170px] text-xs">
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            {HETERO_OPTIONS.map((opt) => (
                              <SelectItem
                                key={opt.value}
                                value={String(opt.value)}
                              >
                                {opt.label}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      </div>
                    </>
                  )}
                </div>
              </div>

              {/* ══════════ NVIDIA 显卡电源管理模式（并入电源计划卡片） ══════════ */}
              <div className="space-y-2 border-t border-border pt-3 mt-2">
                <div className="flex items-center justify-between gap-3">
                  <div className="flex items-center gap-2 min-w-0">
                    <div className="p-1.5 rounded-md bg-green-500/10 shrink-0">
                      <MonitorPlay className="w-5 h-5 text-green-400" />
                    </div>
                    <div className="min-w-0">
                      <h3 className="text-sm font-semibold">NVIDIA 电源模式</h3>
                      <p
                        className={cn(
                          "text-xs truncate",
                          nvidiaPmError
                            ? "text-red-400/80"
                            : "text-muted-foreground",
                        )}
                        title={nvidiaPmError ?? undefined}
                      >
                        {nvidiaPmError ??
                          (nvidiaPm
                            ? (NVIDIA_PM_OPTIONS.find(
                                (o) => o.value === nvidiaPm,
                              )?.label ?? "—")
                            : "读取中…")}
                      </p>
                    </div>
                  </div>
                  {!nvidiaPmError && (
                    <Select
                      value={nvidiaPm ?? undefined}
                      onValueChange={handleNvidiaPmChange}
                      disabled={nvidiaPmSaving}
                    >
                      <SelectTrigger className="h-8 w-[170px] text-xs">
                        <SelectValue placeholder="读取中…" />
                      </SelectTrigger>
                      <SelectContent>
                        {NVIDIA_PM_OPTIONS.map((opt) => (
                          <SelectItem key={opt.value} value={opt.value}>
                            {opt.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  )}
                </div>
              </div>
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
