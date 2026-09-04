import { useState, useEffect, useCallback } from "react";
import type { ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { LucideIcon } from "lucide-react";
import { cn } from "../lib/utils";
import {
  Card,
  CardHeader,
  CardContent,
} from "../components/ui/card";
import { Skeleton } from "../components/ui/skeleton";
import { Button } from "../components/ui/button";
import { Switch } from "../components/ui/switch";
import {
  Tabs,
  TabsList,
  TabsTrigger,
  TabsContent,
} from "../components/ui/tabs";
import { PageHeader } from "../components/PageHeader";
import {
  renderChecks,
  safeInvoke,
  SectionHeader,
  type DxCheck,
} from "../components/environment-shared";
import {
  CheckCircle2,
  AlertTriangle,
  XCircle,
  MonitorPlay,
  Package,
  RefreshCw,
  Gamepad2,
  Monitor,
  ShieldCheck,
  Calendar,
  Cpu,
  HardDrive,
  Clock,
} from "lucide-react";

/* ── Types ── */

interface DirectXInfo {
  version_major: number;
  version_minor: number;
  version: string;
  dx12_ultimate: boolean;
  gpu_count: number;
  gpu_names: string[];
  checks: DxCheck[];
}

interface VcRuntime {
  name: string;
  version: string;
  arch: string;
  installed: boolean;
}

interface VcRuntimeInfo {
  runtimes: VcRuntime[];
  checks: DxCheck[];
}

interface GameSetting {
  /** 设置项标识："hags" | "game_mode" | "vrr" | "mouse_precision" | "power_plan" 为可联动项，空串为纯检测项 */
  key: string;
  label: string;
  description: string;
  recommended: string;
  current: string;
  ok: boolean;
}

interface GamePreset {
  id: string;
  name: string;
  engine: string;
  settings: GameSetting[];
}

/* ── 系统信息类型 ── */

interface ActivationInfo {
  status_raw: string;
  label: string;
}

interface PatchInfo {
  kb: string;
  date: string;
  title_raw: string;
  title_cn: string;
}

interface SystemInfo {
  edition: string;
  build_number: string;
  ubr: string;
  arch: string;
  install_date: string;
  activation: ActivationInfo;
  latest_patch: PatchInfo;
  boot_mode: string;
}

/* ── 系统信息行（图标 + 标签 + 右对齐值） ── */

interface InfoRowProps {
  icon: LucideIcon;
  label: string;
  value: ReactNode;
  /** 值区域额外类（如激活状态着色、截断） */
  valueClass?: string;
}

function InfoRow({ icon: Icon, label, value, valueClass }: InfoRowProps) {
  return (
    <div className="flex items-center gap-2 px-2.5 py-1.5 rounded-md hover:bg-muted/30 transition-colors text-xs">
      <Icon className="size-3 text-sky-400 shrink-0" />
      <span className="text-muted-foreground shrink-0">{label}</span>
      <span
        className={cn("ml-auto text-right", valueClass)}
        title={typeof value === "string" ? value : undefined}
      >
        {value}
      </span>
    </div>
  );
}

/* ── Component ── */

export default function Environment() {
  const [dxInfo, setDxInfo] = useState<DirectXInfo | null>(null);
  const [vcInfo, setVcInfo] = useState<VcRuntimeInfo | null>(null);
  const [dxLoading, setDxLoading] = useState(true);
  const [vcLoading, setVcLoading] = useState(true);
  const [systemInfo, setSystemInfo] = useState<SystemInfo | null>(null);
  const [systemLoading, setSystemLoading] = useState(true);
  const [gamePresets, setGamePresets] = useState<GamePreset[]>([]);
  const [gameLoading, setGameLoading] = useState(true);
  const [gameTab, setGameTab] = useState("cs2");
  /** 正在联动切换的设置项 key（防重复点击，加载态） */
  const [togglingKey, setTogglingKey] = useState<string | null>(null);
  /** 联动切换失败消息（行内提示，切换成功后清空） */
  const [toggleError, setToggleError] = useState<string | null>(null);

  /* ── 设置项联动：与系统设置页同源（同一 setter 命令），切换后重读当前值 ── */
  /** 可联动开关的 key → setter 命令映射（与 Settings.tsx 保持一致） */
  const TOGGLE_CMD: Record<string, string> = {
    hags: "set_hags_state",
    game_mode: "set_game_mode_state",
    vrr: "set_vrr_state",
    mouse_precision: "set_mouse_precision",
  };

  /* ── 独立加载各卡片 ── */
  const loadDx = useCallback(async () => {
    setDxLoading(true);
    const dx = await safeInvoke<DirectXInfo>("check_directx");
    setDxInfo(dx);
    setDxLoading(false);
  }, []);

  const loadVc = useCallback(async () => {
    setVcLoading(true);
    const vc = await safeInvoke<VcRuntimeInfo>("check_vc_runtimes");
    setVcInfo(vc);
    setVcLoading(false);
  }, []);

  const loadSystem = useCallback(async () => {
    setSystemLoading(true);
    const info = await safeInvoke<SystemInfo>("get_system_info");
    setSystemInfo(info);
    setSystemLoading(false);
  }, []);

  const loadGames = useCallback(async () => {
    setGameLoading(true);
    const presets = await safeInvoke<GamePreset[]>("get_game_presets");
    if (presets) {
      setGamePresets(presets);
      setGameTab(presets[0]?.id || "cs2");
    }
    setGameLoading(false);
  }, []);

  /* ── 设置项联动：与系统设置页同源（同一 setter 命令），切换后重读当前值 ── */
  const handleToggle = useCallback(
    async (s: GameSetting, enabled: boolean) => {
      const cmd = TOGGLE_CMD[s.key];
      if (!cmd || togglingKey) return;
      setTogglingKey(s.key);
      setToggleError(null);
      try {
        await invoke(cmd, { enabled });
        // 切换成功：重读当前状态（current/ok 与系统设置页同步更新）
        await loadGames();
      } catch (e: unknown) {
        const msg = e instanceof Error ? e.message : String(e);
        setToggleError(`「${s.label}」切换失败: ${msg}`);
        // 回滚展示：重读真实状态
        loadGames();
      } finally {
        setTogglingKey(null);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [togglingKey, loadGames],
  );

  const loadAll = useCallback(() => {
    // 各卡片独立并行加载
    loadSystem();
    loadDx();
    loadVc();
    loadGames();
  }, [loadSystem, loadDx, loadVc, loadGames]);

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    loadAll();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 任一模块加载中 → 页头刷新按钮进入 loading 态
  const anyLoading = systemLoading || dxLoading || vcLoading || gameLoading;

  // 当前选中的游戏预设（无匹配时渲染空）
  const preset = gamePresets.find((g) => g.id === gameTab);

  /* ── 渲染 ── */

  return (
    <div className="flex flex-col gap-6">
      {/* ── Header ── */}
      <PageHeader
        title="环境检测"
        description="系统信息 · DirectX 诊断 · VC++ 运行库"
        actions={
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={loadAll}
            disabled={anyLoading}
            className="shrink-0"
            title="重新检测"
          >
            <RefreshCw className={cn("size-4", anyLoading && "animate-spin")} />
          </Button>
        }
      />

      {/* ── 4×4 布局 ── */}
      <div className="grid grid-cols-1 lg:grid-cols-2 xl:grid-cols-4 gap-4">
        {/* ── 系统信息 ── */}
        <Card className="xl:col-span-2 animate-scale-in stagger-1">
          <CardHeader className="pb-2">
            <SectionHeader
              icon={Monitor}
              iconClass="bg-sky-500/10"
              iconColor="text-sky-400"
              title="系统信息"
              description={
                systemLoading
                  ? "检测中..."
                  : systemInfo
                    ? `${systemInfo.edition} · Build ${systemInfo.build_number}`
                    : "检测失败"
              }
            />
          </CardHeader>
          <CardContent className="pt-0">
            {systemLoading && !systemInfo ? (
              <div className="flex flex-col gap-1">
                {[1, 2, 3, 4, 5, 6].map((i) => (
                  <Skeleton key={i} className="h-7 rounded-md" />
                ))}
              </div>
            ) : systemInfo ? (
              <div className="space-y-0.5">
                {/* 系统版本 */}
                <InfoRow
                  icon={Monitor}
                  label="系统版本"
                  value={
                    <>
                      {systemInfo.edition}
                      {systemInfo.build_number !== "无法获取" &&
                        ` · Build ${systemInfo.build_number}`}
                    </>
                  }
                />

                {/* 系统架构 */}
                <InfoRow icon={Cpu} label="系统架构" value={systemInfo.arch} />

                {/* 安装日期 */}
                <InfoRow
                  icon={Calendar}
                  label="安装日期"
                  value={systemInfo.install_date}
                />

                {/* 激活状态 */}
                <InfoRow
                  icon={ShieldCheck}
                  label="激活状态"
                  value={systemInfo.activation.label}
                  valueClass={
                    systemInfo.activation.status_raw === "Licensed"
                      ? "text-green-400"
                      : "text-yellow-400"
                  }
                />

                {/* 最新补丁 */}
                <InfoRow
                  icon={HardDrive}
                  label="最新补丁"
                  value={
                    systemInfo.latest_patch.kb === "无"
                      ? "—"
                      : `${systemInfo.latest_patch.kb} · ${systemInfo.latest_patch.title_cn} · ${systemInfo.latest_patch.date}`
                  }
                  valueClass="truncate max-w-[60%]"
                />

                {/* 启动模式 */}
                <InfoRow
                  icon={Clock}
                  label="启动模式"
                  value={systemInfo.boot_mode}
                />
              </div>
            ) : (
              <div className="text-xs text-muted-foreground text-center py-3">
                无法获取系统信息
              </div>
            )}
          </CardContent>
        </Card>

        {/* ── 游戏环境 ── */}
        <Card className="xl:col-span-2 animate-scale-in stagger-2">
          <CardHeader className="pb-2">
            <SectionHeader
              icon={Gamepad2}
              iconClass="bg-orange-500/10"
              iconColor="text-orange-400"
              title="游戏环境"
              description="推荐系统设置 &middot; 当前状态对比"
            />
          </CardHeader>
          <CardContent className="pt-0">
            {gameLoading ? (
              <div className="flex flex-col gap-1">
                {[1, 2, 3, 4, 5, 6].map((i) => (
                  <Skeleton key={i} className="h-7 rounded-md" />
                ))}
              </div>
            ) : (
              <>
                {/* 游戏 Tab（受控于 gameTab 状态，与 loadGames 回填逻辑一致） */}
                <Tabs value={gameTab} onValueChange={setGameTab}>
                  <TabsList className="h-auto w-full justify-start gap-1 flex-wrap bg-transparent p-0">
                    {gamePresets.map((g) => (
                      <TabsTrigger
                        key={g.id}
                        value={g.id}
                        className="h-7 flex-none rounded text-xs data-[state=active]:bg-orange-500/20 data-[state=active]:text-orange-400"
                      >
                        {g.name}
                      </TabsTrigger>
                    ))}
                  </TabsList>

                  {/* 当前选中游戏的设置清单 */}
                  <TabsContent value={gameTab} className="mt-2">
                    {preset ? (
                      <div className="space-y-1">
                        {toggleError && (
                          <div className="flex items-center gap-1.5 px-2.5 py-1 rounded-md text-xs bg-red-500/10 text-red-400">
                            <XCircle className="size-3 shrink-0" />
                            <span className="truncate">{toggleError}</span>
                          </div>
                        )}
                        {preset.settings.map((s, i) => (
                          <div
                            key={i}
                            className={cn(
                              "flex items-center gap-2 px-2.5 py-1.5 rounded-md text-xs",
                              s.ok ? "bg-green-500/5" : "bg-orange-500/5",
                            )}
                          >
                            {s.ok ? (
                              <CheckCircle2 className="size-3 text-green-400 shrink-0" />
                            ) : (
                              <AlertTriangle className="size-3 text-orange-400 shrink-0" />
                            )}
                            <span className="font-medium min-w-[100px]">
                              {s.label}
                            </span>
                            <span
                              className="text-muted-foreground flex-1 truncate"
                              title={s.description}
                            >
                              {s.description}
                            </span>
                            {/* 可联动开关：与系统设置页同源，直接切换并实时刷新检测结果 */}
                            {TOGGLE_CMD[s.key] ? (
                              <div className="flex items-center gap-1.5 shrink-0">
                                <span
                                  className={cn(
                                    "font-mono",
                                    s.ok ? "text-green-400" : "text-orange-400",
                                  )}
                                >
                                  {s.current}
                                </span>
                                <Switch
                                  checked={s.current === "开启"}
                                  disabled={togglingKey !== null}
                                  onCheckedChange={(v) => handleToggle(s, v)}
                                />
                              </div>
                            ) : (
                              <div className="flex items-center gap-1.5 shrink-0">
                                <span
                                  className={cn(
                                    "font-mono",
                                    s.ok ? "text-green-400" : "text-orange-400",
                                  )}
                                >
                                  {s.current}
                                </span>
                                {!s.ok && (
                                  <span className="text-muted-foreground">
                                    → {s.recommended}
                                  </span>
                                )}
                              </div>
                            )}
                          </div>
                        ))}
                      </div>
                    ) : null}
                  </TabsContent>
                </Tabs>
              </>
            )}
          </CardContent>
        </Card>

        {/* ── DirectX 诊断 ── */}
        <Card className="xl:col-span-2 animate-scale-in stagger-3">
          <CardHeader className="pb-2">
            <SectionHeader
              icon={MonitorPlay}
              iconClass="bg-purple-500/10"
              iconColor="text-purple-400"
              title="DirectX 诊断"
              description={
                dxLoading
                  ? "检测中..."
                  : dxInfo
                    ? `DirectX ${dxInfo.version}`
                    : "检测失败"
              }
            />
          </CardHeader>
          <CardContent className="pt-0">
            {dxLoading && !dxInfo ? (
              <div className="flex flex-col gap-1">
                {[1, 2, 3, 4, 5].map((i) => (
                  <Skeleton key={i} className="h-7 rounded-md" />
                ))}
              </div>
            ) : dxInfo ? (
              renderChecks(dxInfo.checks)
            ) : (
              <div className="text-xs text-muted-foreground text-center py-3">
                无法获取 DirectX 信息
              </div>
            )}
          </CardContent>
        </Card>

        {/* ── VC++ 运行库 ── */}
        <Card className="xl:col-span-2 animate-scale-in stagger-4">
          <CardHeader className="pb-2">
            <SectionHeader
              icon={Package}
              iconClass="bg-blue-500/10"
              iconColor="text-blue-400"
              title="VC++ 运行库"
              description={
                vcLoading
                  ? "检测中..."
                  : vcInfo
                    ? `${vcInfo.runtimes.filter((r) => r.installed).length}/${vcInfo.runtimes.length} 已安装`
                    : "检测失败"
              }
            />
          </CardHeader>
          <CardContent className="pt-0">
            {vcLoading && !vcInfo ? (
              <div className="flex flex-col gap-1">
                {[1, 2, 3].map((i) => (
                  <Skeleton key={i} className="h-7 rounded-md" />
                ))}
              </div>
            ) : vcInfo ? (
              <>
                {renderChecks(vcInfo.checks)}
                <div className="grid grid-cols-2 gap-1 mt-3">
                  {vcInfo.runtimes.map((rt) => (
                    <div
                      key={`${rt.name}-${rt.arch}`}
                      className={cn(
                        "flex items-center gap-1.5 px-2 py-1 rounded text-xs border-l-2",
                        rt.installed
                          ? "border-l-green-500 bg-green-500/5"
                          : "border-l-red-500 bg-red-500/5 opacity-50",
                      )}
                    >
                      {rt.installed ? (
                        <CheckCircle2 className="size-2.5 text-green-400 shrink-0" />
                      ) : (
                        <XCircle className="size-2.5 text-red-400 shrink-0" />
                      )}
                      <span className="truncate">
                        {rt.name} ({rt.arch})
                      </span>
                    </div>
                  ))}
                </div>
              </>
            ) : (
              <div className="text-xs text-muted-foreground text-center py-3">
                无法获取 VC++ 运行库信息
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
