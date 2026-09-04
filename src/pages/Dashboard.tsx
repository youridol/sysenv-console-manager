import { logger } from "../lib/logger";
import { memo, useState, useEffect } from "react";
import type { ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip as RechartsTooltip,
  ResponsiveContainer,
  Area,
  AreaChart,
} from "recharts";
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
} from "../components/ui/card";
import { Progress } from "../components/ui/progress";
import { Skeleton } from "../components/ui/skeleton";
import { Alert, AlertTitle, AlertDescription } from "../components/ui/alert";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import {
  Cpu,
  Thermometer,
  HardDrive,
  MemoryStick,
  Zap,
  TrendingUp,
  Gauge,
  ArrowDown,
  ArrowUp,
  Network,
  Activity,
  AlertTriangle,
  ShieldAlert,
  RefreshCw,
  ExternalLink,
  Copy,
  CircuitBoard,
  Fan,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { cn, formatBytes } from "../lib/utils";
import { getDisplayNet, useNetIface } from "../contexts/NetIfaceContext";
import { LiveIndicator } from "../components/LiveIndicator";
import { PageHeader } from "../components/PageHeader";
import { CardHeaderWithIcon } from "../components/CardHeaderWithIcon";
import { SectionTitle } from "../components/SectionTitle";
interface SensorData {
  cpu: {
    name: string;
    usage: number;
    per_core: number[];
    core_count: number;
    clock_mhz: number;
    /** 频率数据源："ntapi" | "pdh" | "registry" | "sysinfo" | "none"（旧版后端缺失时为 undefined，须兼容） */
    freq_source?: string;
    temperature: number;
    power_w: number;
    /** 温度数据源："lhm" | "winring0" | "acpi" | "none"（旧版后端缺失时为 undefined） */
    temp_source?: string;
    /** 功耗数据源："rapl" | "estimated" | "none" */
    power_source?: string;
    /** true = 估算功耗（前端显示"估算"角标） */
    power_estimated?: boolean;
  };
  gpu: Array<{
    name: string;
    usage: number;
    temperature: number;
    memory_used: number;
    memory_total: number;
    clock_mhz: number;
    power_w: number;
  }>;
  memory: {
    total: number;
    used: number;
    available: number;
    usage_percent: number;
    /** 内存型号（LHM SPD 补充；旧版后端缺失时为 undefined） */
    model_name?: string;
  };
  /** LHM 主板传感器（温度/风扇/电压；LHM 不可用或无数据时为 null/undefined，不渲染卡片） */
  motherboard?: {
    name?: string | null;
    sensors: Array<{
      name: string;
      /** 传感器类型：temperature | fan | voltage */
      type: string;
      value: number;
    }>;
  } | null;
  disks: Array<{
    name: string;
    total_space: number;
    available_space: number;
    used_space: number;
    usage_percent: number;
    read_mbps: number;
    write_mbps: number;
  }>;
  diag: string;
}

/** driver_status / driver_install 命令返回的安全软件信息（与后端 serde 契约对齐） */
export interface SecuritySoftwareInfo {
  /** 内部标识：defender | huorong | 360 | ... */
  name: string;
  /** 展示名（如 "火绒安全软件"） */
  display_name: string;
  /** 检测到运行中 */
  active: boolean;
  /** 是否命中已知拦截对象（WinRing0） */
  blocked_driver: boolean;
}

/** driver_status / driver_install 命令返回的驱动状态（与后端 serde 契约对齐） */
export interface DriverStatus {
  /** 当前生效的采集通道 */
  channel: "lhm" | "winring0" | "acpi" | "none";
  /** 是否已有可用的驱动通道 */
  available: boolean;
  /** 驱动服务状态（SCM 查询结果） */
  service_state: string;
  secure_boot: boolean;
  testsigning: boolean;
  /** 降级原因链（逐条展示给用户） */
  degraded: string[];
  /** 驱动探测到的温度（℃），探测失败为 null */
  probe_temp_c: number | null;
  /** 检测到的安全软件列表 */
  security: SecuritySoftwareInfo[];
}

/** driver_status 查询超时（毫秒） */
const DRIVER_STATUS_TIMEOUT_MS = 5000;
/** driver_install 一键重试超时（毫秒）：含驱动安装与退避重试，放宽到 30s */
const DRIVER_INSTALL_TIMEOUT_MS = 30000;

/**
 * 按安全软件 name 给出的驱动放行配置步骤（name 与后端 SecuritySoftwareInfo.name 对齐）。
 * 未命中的杀软回退到 SECURITY_GUIDE_GENERIC 通用指引。
 */
const SECURITY_GUIDE: Readonly<Record<string, readonly string[]>> = {
  defender: [
    "打开 Windows 安全中心 → 设备安全性 → 内核隔离，关闭「内存完整性」开关",
    "在「易受攻击的驱动程序阻止列表」中关闭阻止列表，或把 SECM 驱动加入排除",
  ],
  huorong: [
    "打开火绒安全 → 防护中心 → 信任区 → 添加「SECM 安装/运行目录」与「WinRing0x64.sys」",
    "或在火绒 → 设置 → 驱动防护中允许 SECM 加载内核驱动",
    "添加完成后点击下方「一键重试」重新检测",
    "若提示服务标记删除/驱动加载被拒（错误码 0x431/0x430），请重启电脑后再试",
  ],
  "360": [
    "打开 360 安全卫士 → 木马查杀 → 信任区，添加 SECM 安装目录",
    "如弹窗提示拦截驱动加载，选择「允许 / 信任」",
  ],
  tencent: ["打开腾讯电脑管家 → 病毒查杀 → 信任区，添加 SECM 安装目录"],
  kaspersky: ["打开卡巴斯基 → 设置 → 排除项与受信任程序，添加 SECM 为受信任程序"],
  norton: ["打开诺顿 → 设置 → 防病毒排除项，添加 SECM 安装目录"],
  eset: ["打开 ESET 安全设置 → 检测引擎排除项，添加 SECM 安装目录"],
  lenovo: ["打开联想电脑管家 → 信任中心 → 白名单，添加 SECM 安装目录"],
};

/** 未识别杀软时的通用配置指引 */
const SECURITY_GUIDE_GENERIC: readonly string[] = [
  "在杀毒软件的信任区 / 白名单中添加 SECM 安装目录",
  "如出现驱动加载拦截弹窗，选择「允许 / 信任」，再点击下方「一键重试」",
];

interface HistoryPoint {
  time: string;
  cpu: number;
  gpu: number;
  memory: number;
}

const MAX_HISTORY = 30;
const NET_MAX_HISTORY = 30;
// 网络速率合理上限（2 GB/s，单位 KB；万兆网卡理论 1.25GB/s，超出视为异常毛刺/旧数据）
const NET_KB_LIMIT = 1024 * 1024 * 2;
/** 传感器查询超时（毫秒）：驱动挂起时避免轮询永久 pending */
const LOAD_TIMEOUT_MS = 8000;

/**
 * 给 Promise 加超时：超时后 reject（invoke 无法取消底层 spawn_blocking，
 * 用于驱动挂起时前端至少给出明确提示，避免 UI 永久 loading/请求堆积）。
 */
function withTimeout<T>(p: Promise<T>, ms: number, message: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(message)), ms);
    p.then(
      (v) => {
        clearTimeout(timer);
        resolve(v);
      },
      (e: unknown) => {
        clearTimeout(timer);
        reject(e);
      },
    );
  });
}

interface NetHistoryPoint {
  time: string;
  download: number;
  upload: number;
}

// 图表稳定 props：内联对象/函数每次渲染都是新引用，会触发 Recharts 内部 effect 每秒重算
// （父组件每秒重渲染）——提取为模块级常量，兼做浅比较基准（配合 memo）。
const CHART_TICK = { fontSize: 10, fill: "var(--color-muted-foreground)" };
const CHART_TOOLTIP_STYLE = {
  backgroundColor: "var(--color-card)",
  border: "1px solid var(--color-border)",
  borderRadius: "8px",
  fontSize: "12px",
  boxShadow: "0 4px 12px rgba(0,0,0,0.15)",
};
const CHART_NET_TOOLTIP_FORMATTER = (value: unknown) => [
  `${formatBytes(Number(value) * 1024)}/s`,
];

/** 网络图表 Y 轴刻度：KB 值自动换算为 KB/MB/GB（长值减少小数位，避免换行） */
const formatAxisBytes = (v: number) => {
  const bytes = v * 1024;
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  if (bytes >= 1024 ** 2) {
    const mb = bytes / 1024 ** 2;
    return mb >= 100 ? `${mb.toFixed(0)} MB` : `${mb.toFixed(1)} MB`;
  }
  const kb = bytes / 1024;
  return kb >= 100 ? `${kb.toFixed(0)} KB` : `${kb.toFixed(1)} KB`;
};

// 图表 memo 化：props 引用全部稳定（仅 data 每 2s 变一次）时，
// 隔离每秒父组件重渲染（sensor/netStats 每秒 setState）对图表树的传导。
const HardwareTrendChart = memo(function HardwareTrendChart({
  data,
}: {
  data: HistoryPoint[];
}) {
  return (
    // height="100%"：弹性填满等高卡片剩余空间（消除底部留白）
    <ResponsiveContainer width="100%" height="100%">
      <AreaChart data={data} margin={{ top: 5, right: 5, bottom: 5, left: 0 }}>
        <defs>
          {/* 渐变 stopColor 用品牌色 CSS 变量，与线条 stroke 同源（var(--color-brand-*)） */}
          <linearGradient id="cpuGrad" x1="0" y1="0" x2="0" y2="1">
            <stop offset="5%" stopColor="var(--color-brand-blue)" stopOpacity={0.25} />
            <stop offset="95%" stopColor="var(--color-brand-blue)" stopOpacity={0} />
          </linearGradient>
          <linearGradient id="gpuGrad" x1="0" y1="0" x2="0" y2="1">
            <stop offset="5%" stopColor="var(--color-brand-green)" stopOpacity={0.25} />
            <stop offset="95%" stopColor="var(--color-brand-green)" stopOpacity={0} />
          </linearGradient>
          <linearGradient id="memGrad" x1="0" y1="0" x2="0" y2="1">
            <stop offset="5%" stopColor="var(--color-brand-yellow)" stopOpacity={0.2} />
            <stop offset="95%" stopColor="var(--color-brand-yellow)" stopOpacity={0} />
          </linearGradient>
        </defs>
        <CartesianGrid
          strokeDasharray="3 3"
          stroke="var(--color-border)"
          opacity={0.5}
        />
        <XAxis dataKey="time" tick={CHART_TICK} interval={14} />
        {/* width=32 + margin.left=0：Y 轴区域收窄，图表主体向左靠 */}
        <YAxis domain={[0, 100]} tick={CHART_TICK} unit="%" width={32} />
        <RechartsTooltip contentStyle={CHART_TOOLTIP_STYLE} />
        <Area
          type="monotone"
          dataKey="cpu"
          stroke="var(--color-brand-blue)"
          strokeWidth={2}
          fill="url(#cpuGrad)"
          dot={false}
          isAnimationActive={true}
          animationDuration={1000}
          name="CPU"
        />
        <Area
          type="monotone"
          dataKey="gpu"
          stroke="var(--color-brand-green)"
          strokeWidth={2}
          fill="url(#gpuGrad)"
          dot={false}
          isAnimationActive={true}
          animationDuration={1000}
          name="GPU"
        />
        <Area
          type="monotone"
          dataKey="memory"
          stroke="var(--color-brand-yellow)"
          strokeWidth={2}
          fill="url(#memGrad)"
          dot={false}
          isAnimationActive={true}
          animationDuration={1000}
          name="内存"
        />
      </AreaChart>
    </ResponsiveContainer>
  );
});

const NetTrendChart = memo(function NetTrendChart({
  data,
}: {
  data: NetHistoryPoint[];
}) {
  return (
    <ResponsiveContainer width="100%" height={150}>
      <AreaChart data={data} margin={{ top: 5, right: 5, bottom: 5, left: 0 }}>
        <defs>
          <linearGradient id="dlGradDash" x1="0" y1="0" x2="0" y2="1">
            <stop offset="5%" stopColor="var(--color-brand-blue)" stopOpacity={0.25} />
            <stop offset="95%" stopColor="var(--color-brand-blue)" stopOpacity={0} />
          </linearGradient>
          <linearGradient id="ulGradDash" x1="0" y1="0" x2="0" y2="1">
            <stop offset="5%" stopColor="var(--color-brand-green)" stopOpacity={0.25} />
            <stop offset="95%" stopColor="var(--color-brand-green)" stopOpacity={0} />
          </linearGradient>
        </defs>
        <CartesianGrid
          strokeDasharray="3 3"
          stroke="var(--color-border)"
          opacity={0.5}
        />
        <XAxis dataKey="time" tick={CHART_TICK} interval={14} />
        {/* width=60：数值+单位单行左右排列（防止 Recharts Text 按空格换行成上下两行） */}
        <YAxis
          tick={CHART_TICK}
          tickFormatter={formatAxisBytes}
          width={60}
          domain={[
            0,
            (dataMax: number) =>
              Math.min(Math.max(dataMax, 1024), NET_KB_LIMIT),
          ]}
        />
        <RechartsTooltip
          contentStyle={CHART_TOOLTIP_STYLE}
          formatter={CHART_NET_TOOLTIP_FORMATTER}
        />
        <Area
          type="monotone"
          dataKey="download"
          stroke="var(--color-brand-blue)"
          strokeWidth={2}
          fill="url(#dlGradDash)"
          dot={false}
          isAnimationActive={true}
          animationDuration={1000}
          name="下载"
        />
        <Area
          type="monotone"
          dataKey="upload"
          stroke="var(--color-brand-green)"
          strokeWidth={2}
          fill="url(#ulGradDash)"
          dot={false}
          isAnimationActive={true}
          animationDuration={1000}
          name="上传"
        />
      </AreaChart>
    </ResponsiveContainer>
  );
});

/* ── 实时摘要卡片（CPU/GPU/内存/存储共用结构） ── */

/** 卡片内单条统计（可带前置小图标，如温度/频率） */
interface SummaryStat {
  text: string;
  icon?: LucideIcon;
  iconClass?: string;
  /** 额外布局类（如 ml-auto 使该条靠右对齐） */
  className?: string;
  /** 数值后的补充角标（如"估算"/"低精度"），可传 Badge 元素 */
  badge?: ReactNode;
}

interface SummaryCardProps {
  icon: LucideIcon;
  /** 图标容器背景类（如 bg-blue-500/10） */
  iconClass: string;
  /** 图标颜色类（如 text-blue-400） */
  iconColor: string;
  label: string;
  /** 标题右侧说明（如核数/磁盘数） */
  meta?: string;
  /** 主数值（不含后缀，统一由组件追加） */
  value: string;
  /** 主数值后缀（默认 "%"；如存储卡片传 " MB/s"） */
  valueSuffix?: string;
  progressValue: number;
  /** Progress 指示条颜色类（静态字面量） */
  progressClass: string;
  stats: SummaryStat[];
  /** 无数据时的占位内容（如"未检测到 GPU"）；非空时不再渲染主数值区 */
  fallback?: ReactNode;
  className?: string;
}

function SummaryCard({
  icon: Icon,
  iconClass,
  iconColor,
  label,
  meta,
  value,
  valueSuffix = "%",
  progressValue,
  progressClass,
  stats,
  fallback,
  className,
}: SummaryCardProps) {
  return (
    <Card className={className}>
      <CardContent className="p-4">
        <div className="flex items-center gap-2 mb-2">
          <div
            className={cn(
              "flex size-8 shrink-0 items-center justify-center rounded-md p-1.5",
              iconClass,
            )}
          >
            <Icon className={cn("size-4 shrink-0", iconColor)} />
          </div>
          <span className="text-sm font-medium">{label}</span>
          {meta && (
            <span className="text-xs text-muted-foreground ml-auto">
              {meta}
            </span>
          )}
        </div>
        {fallback ?? (
          <>
            <div className="text-2xl font-bold tabular-nums">
              {value}
              <span className="text-sm text-muted-foreground ml-0.5">
                {valueSuffix}
              </span>
            </div>
            <Progress value={progressValue} className={progressClass} />
            <div className="flex gap-3 mt-2 text-xs text-muted-foreground">
              {stats.map((s) => (
                <span
                  key={s.text}
                  className={cn("flex items-center gap-1", s.className)}
                >
                  {s.icon && <s.icon className={cn("size-3", s.iconClass)} />}
                  {s.text}
                  {s.badge}
                </span>
              ))}
            </div>
          </>
        )}
      </CardContent>
    </Card>
  );
}

/* ── 主板传感器卡（LHM SuperIO：温度/风扇/电压分组只读展示；无数据不渲染） ── */

interface MotherboardCardProps {
  /** LHM 主板传感器数据（null = LHM 不可用/无数据） */
  mb: NonNullable<SensorData["motherboard"]>;
}

/** 传感器类型 → 分组展示元数据（图标/单位/颜色） */
const MB_GROUP_META: Record<
  string,
  { label: string; unit: string; icon: LucideIcon; colorClass: string; iconClass: string }
> = {
  temperature: {
    label: "温度",
    unit: "°C",
    icon: Thermometer,
    colorClass: "text-orange-400",
    iconClass: "bg-orange-500/10",
  },
  fan: {
    label: "风扇",
    unit: " RPM",
    icon: Fan,
    colorClass: "text-sky-400",
    iconClass: "bg-sky-500/10",
  },
  voltage: {
    label: "电压",
    unit: " V",
    icon: Zap,
    colorClass: "text-yellow-400",
    iconClass: "bg-yellow-500/10",
  },
};

/**
 * 主板传感器卡：按 type（temperature/fan/voltage）分组展示 LHM SuperIO 传感器。
 * 无数据/未知类型传感器不渲染；仅当 mb.sensors 非空时由父级挂载。
 */
function MotherboardCard({ mb }: MotherboardCardProps) {
  // 按类型分组（保持 sidecar 返回顺序，同组内稳定）
  const groups = (["temperature", "fan", "voltage"] as const)
    .map((kind) => ({
      kind,
      meta: MB_GROUP_META[kind],
      items: mb.sensors.filter((s) => s.type === kind),
    }))
    .filter((g) => g.items.length > 0);

  return (
    <Card className="animate-scale-in stagger-4">
      <CardHeaderWithIcon
        icon={CircuitBoard}
        iconClass="bg-fuchsia-500/10"
        iconColor="text-fuchsia-400"
        title={mb.name && mb.name.trim().length > 0 ? mb.name : "主板传感器"}
        description={`LHM SuperIO 共 ${mb.sensors.length} 项`}
      />
      <CardContent className="px-4 pb-4 grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
        {groups.map((g) => (
          <div
            key={g.kind}
            className="rounded-lg border border-border/60 bg-muted/20 p-3"
          >
            <div className="flex items-center gap-1.5 mb-2">
              <g.meta.icon className={cn("size-3", g.meta.colorClass)} />
              <span className="text-xs text-muted-foreground">
                {g.meta.label}
              </span>
              <span className="ml-auto text-[10px] text-muted-foreground">
                {g.items.length} 项
              </span>
            </div>
            <div className="space-y-1">
              {g.items.map((s, i) => (
                <div
                  key={`${s.name}-${i}`}
                  className="flex items-baseline justify-between gap-2 text-xs"
                >
                  <span className="truncate text-muted-foreground">
                    {s.name || "—"}
                  </span>
                  <span className="shrink-0 tabular-nums font-medium">
                    {s.value.toFixed(
                      g.kind === "voltage" ? 2 : g.kind === "temperature" ? 1 : 0,
                    )}
                    <span className="text-[10px] text-muted-foreground">
                      {g.meta.unit}
                    </span>
                  </span>
                </div>
              ))}
            </div>
          </div>
        ))}
      </CardContent>
    </Card>
  );
}

/* ── 存储卡（复用 SummaryCard 结构，磁盘实时读写 + 容量进度，不移动位置） ── */

interface StorageCardProps {
  icon: LucideIcon;
  /** 图标容器背景类（如 bg-purple-500/10） */
  iconClass: string;
  /** 图标颜色类（如 text-purple-400） */
  iconColor: string;
  label: string;
  /** 标题右侧说明（磁盘数） */
  meta?: string;
  /** 磁盘数据（与 SensorData.disks 同构） */
  disks: SensorData["disks"];
  className?: string;
}

/**
 * 存储卡：由 Dashboard 底部网格内联实现提取（B 决策：复用 SummaryCard 结构，消除重复实现）。
 * 行 1：卷标 + 总容量 + 读写速度；行 2：容量进度条 + 已用 + 可用。
 */
function StorageCard({
  icon: Icon,
  iconClass,
  iconColor,
  label,
  meta,
  disks,
  className,
}: StorageCardProps) {
  return (
    <Card className={className}>
      <CardContent className="p-4">
        <div className="flex items-center gap-2 mb-2">
          <div
            className={cn(
              "flex size-8 shrink-0 items-center justify-center rounded-md p-1.5",
              iconClass,
            )}
          >
            <Icon className={cn("size-4 shrink-0", iconColor)} />
          </div>
          <span className="text-sm font-medium">{label}</span>
          {meta && (
            <span className="text-xs text-muted-foreground ml-auto">
              {meta}
            </span>
          )}
        </div>
        {disks.length === 0 ? (
          <p className="text-sm text-muted-foreground">无磁盘数据</p>
        ) : (
          <div className="flex flex-col divide-y divide-border/50">
            {disks.map((d) => {
              // 进度条颜色：按使用率分级（90% 红 / 70% 黄 / 正常紫）
              const barClass =
                d.usage_percent > 90
                  ? "[&>div]:bg-red-500"
                  : d.usage_percent > 70
                    ? "[&>div]:bg-yellow-500"
                    : "[&>div]:bg-purple-500";
              return (
                <div
                  key={d.name}
                  className="py-2 first:pt-0 last:pb-0 space-y-1"
                >
                  {/* 行 1：卷标 + 总容量 + 读写速度 */}
                  <div className="flex items-center gap-1.5 text-xs min-w-0">
                    <HardDrive className="size-3 text-purple-400 shrink-0" />
                    <span className="font-medium truncate" title={d.name}>
                      {d.name || "系统盘"}
                    </span>
                    <span className="text-xs text-muted-foreground shrink-0">
                      {formatBytes(d.total_space)}
                    </span>
                    <span className="ml-auto text-xs tabular-nums shrink-0">
                      读 {d.read_mbps.toFixed(1)} MB/s
                    </span>
                    <span className="text-xs tabular-nums text-muted-foreground shrink-0">
                      写 {d.write_mbps.toFixed(1)} MB/s
                    </span>
                  </div>
                  {/* 行 2：容量进度条 + 已用 + 可用 */}
                  <div className="flex items-center gap-2">
                    <Progress
                      value={d.usage_percent}
                      className={cn("flex-1 h-1.5", barClass)}
                    />
                    <span className="text-xs text-muted-foreground tabular-nums shrink-0 w-14 text-right">
                      已用 {d.usage_percent.toFixed(0)}%
                    </span>
                    <span className="text-xs text-muted-foreground tabular-nums shrink-0 w-20 text-right">
                      {formatBytes(d.available_space)} 可用
                    </span>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

/* ── 驱动降级引导卡（杀软拦截内核驱动导致温度/功耗缺失时展示） ── */

/**
 * 查询驱动状态（只读检测）。失败返回 null（调用方自行降级为通用文案），
 * 不抛出 —— 避免阻塞传感器轮询热路径。
 */
async function queryDriverStatus(): Promise<DriverStatus | null> {
  try {
    return await withTimeout(
      invoke<DriverStatus>("driver_status"),
      DRIVER_STATUS_TIMEOUT_MS,
      `driver_status 查询超时（超过 ${DRIVER_STATUS_TIMEOUT_MS / 1000} 秒无响应）`,
    );
  } catch (e: unknown) {
    logger.warn("Dashboard", 
      "driver_status failed:",
      e instanceof Error ? e.message : String(e),
    );
    return null;
  }
}

/**
 * 一键重试：invalidate 驱动缓存并重新检测安装（后端处理权限门与安装链）。
 * 失败返回 null，不抛出。
 */
async function retryDriverInstall(): Promise<DriverStatus | null> {
  try {
    return await withTimeout(
      invoke<DriverStatus>("driver_install"),
      DRIVER_INSTALL_TIMEOUT_MS,
      `driver_install 执行超时（超过 ${DRIVER_INSTALL_TIMEOUT_MS / 1000} 秒无响应）`,
    );
  } catch (e: unknown) {
    logger.warn("Dashboard", 
      "driver_install failed:",
      e instanceof Error ? e.message : String(e),
    );
    return null;
  }
}

interface DriverGuidanceCardProps {
  /** 触发降级的数据项描述（如 "CPU 温度、CPU 功耗"） */
  missingItems: string;
}

/**
 * 降级引导卡：
 * - 挂载时查询一次 driver_status（一次性，不进传感器 1s 轮询热路径）
 * - 展示驱动状态摘要 + 降级原因链 + 检测到的安全软件 + 具体配置步骤
 * - 「一键重试」调 driver_install 刷新卡片；「重新检测」重查 driver_status
 * - invoke 失败静默降级为通用文案（console.warn），不阻塞传感器轮询
 */
function DriverGuidanceCard({ missingItems }: DriverGuidanceCardProps) {
  const [status, setStatus] = useState<DriverStatus | null>(null);
  const [queryError, setQueryError] = useState<string | null>(null);
  const [busy, setBusy] = useState<"retry" | "redetect" | null>(null);

  // 降级条件触发时查询一次驱动状态（一次性，不进 1s 轮询热路径）
  useEffect(() => {
    let mounted = true;
    void queryDriverStatus().then((s) => {
      if (!mounted) return;
      if (s === null) {
        setQueryError("驱动状态查询失败，暂无法获取降级原因。");
      } else {
        setStatus(s);
        setQueryError(null);
      }
    });
    return () => {
      mounted = false;
    };
  }, []);

  // 一键重试：driver_install 完成后用返回的 DriverStatus 刷新卡片
  const handleRetry = async () => {
    setBusy("retry");
    const s = await retryDriverInstall();
    if (s === null) {
      setQueryError(
        "重试失败：驱动安装未完成。请按上方指引在安全软件中放行后再次尝试。",
      );
    } else {
      setStatus(s);
      setQueryError(null);
    }
    setBusy(null);
  };

  // 重新检测：仅重查 driver_status
  const handleRedetect = async () => {
    setBusy("redetect");
    const s = await queryDriverStatus();
    if (s === null) {
      setQueryError("重新检测失败：驱动状态查询无响应。");
    } else {
      setStatus(s);
      setQueryError(null);
    }
    setBusy(null);
  };

  // 打开 Windows 安全中心：固定协议 URL（windowsdefender://），无用户输入拼接，无注入面
  const openWindowsSecurity = () => {
    try {
      const win = window.open("windowsdefender://", "_self");
      if (win === null) {
        logger.warn("Dashboard", 
          "open windowsdefender:// returned null (popup may be blocked)",
        );
      }
    } catch (e: unknown) {
      logger.warn("Dashboard", 
        "open windowsdefender:// failed:",
        e instanceof Error ? e.message : String(e),
      );
    }
  };

  // 命中运行中的安全软件 → 具体配置步骤；否则通用指引
  const activeSecurity = status?.security.find((s) => s.active);
  const guideSteps = activeSecurity
    ? (SECURITY_GUIDE[activeSecurity.name] ?? SECURITY_GUIDE_GENERIC)
    : SECURITY_GUIDE_GENERIC;

  return (
    <Alert className="animate-fade-in border-amber-500/30 bg-amber-500/5">
      <ShieldAlert className="text-amber-400" />
      <AlertTitle>硬件数据受安全软件限制</AlertTitle>
      <AlertDescription>
        <p>
          {missingItems}数据不可用。这通常由安全软件（Windows Defender / 火绒
          / 360 等）拦截内核驱动加载导致，可按下方步骤放行后重试。
        </p>

        {/* 驱动状态摘要 */}
        {status && (
          <div className="mt-2 flex flex-wrap items-center gap-1.5">
            <Badge
              variant="outline"
              className="border-amber-500/40 text-amber-400"
            >
              驱动通道 {status.channel}
            </Badge>
            <Badge
              variant="outline"
              className={
                status.available
                  ? "border-emerald-500/40 text-emerald-400"
                  : "border-amber-500/40 text-amber-400"
              }
            >
              {status.available ? "驱动可用" : "驱动不可用"}
            </Badge>
            {status.secure_boot && (
              <Badge variant="outline">Secure Boot 开启</Badge>
            )}
            {status.testsigning && (
              <Badge variant="outline">测试签名开启</Badge>
            )}
          </div>
        )}

        {/* 降级原因链 */}
        {status && status.degraded.length > 0 && (
          <ul className="mt-2 space-y-1 text-xs">
            {status.degraded.map((d, i) => (
              <li key={i} className="flex items-start gap-1.5">
                <span className="mt-1.5 inline-block h-1 w-1 shrink-0 rounded-full bg-amber-400" />
                {d}
              </li>
            ))}
          </ul>
        )}

        {/* 检测到的安全软件 */}
        {status && status.security.length > 0 && (
          <div className="mt-2 flex flex-wrap items-center gap-1.5">
            <span className="text-xs text-muted-foreground">
              检测到安全软件：
            </span>
            {status.security.map((s) => (
              <Badge
                key={s.name}
                variant="outline"
                className={s.active ? "border-red-500/40 text-red-400" : ""}
              >
                {s.display_name}
                {s.active ? " · 运行中" : " · 未运行"}
              </Badge>
            ))}
          </div>
        )}

        {/* 配置指引 */}
        <ol className="mt-3 space-y-1 text-xs">
          {guideSteps.map((step, i) => (
            <li key={i} className="flex items-start gap-1.5">
              <span className="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-amber-500/20 text-[10px] font-medium text-amber-400">
                {i + 1}
              </span>
              {step}
            </li>
          ))}
        </ol>

        {/* LHM sidecar 日志路径：排障入口（UAC 未确认 / PawnIO 拦截可查此日志上报） */}
        <p className="mt-3 flex items-start gap-1.5 rounded-md border border-border/60 bg-muted/20 px-2 py-1.5 text-xs text-muted-foreground">
          <AlertTriangle className="mt-0.5 size-3 shrink-0 text-amber-400" />
          <span>
            LHM 采集日志：<code className="rounded bg-muted px-1 font-mono">%TEMP%\secm-lhm-sidecar.log</code>
            （如弹窗提示 UAC 请点击「是」；仍失败可复制该日志上报）
          </span>
        </p>

        {/* 查询失败提示（不阻塞操作按钮） */}
        {queryError && (
          <p className="mt-2 text-xs text-destructive">{queryError}</p>
        )}

        {/* 操作区 */}
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            onClick={() => void handleRetry()}
            disabled={busy !== null}
            className="bg-amber-500/15 text-amber-400 hover:bg-amber-500/25"
          >
            <RefreshCw
              className={cn("h-3.5 w-3.5", busy === "retry" && "animate-spin")}
            />
            {busy === "retry" ? "重试中…" : "一键重试"}
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={() => void handleRedetect()}
            disabled={busy !== null}
          >
            {busy === "redetect" ? "检测中…" : "重新检测"}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={openWindowsSecurity}
            disabled={busy !== null}
          >
            <ExternalLink className="h-3.5 w-3.5" />
            打开 Windows 安全中心
          </Button>
        </div>
      </AlertDescription>
    </Alert>
  );
}

/** 频率全链路失败诊断卡：一次性摘要（可复制上报），与 DriverGuidanceCard 同风格 */
interface FrequencyDiagCardProps {
  freqSource: string;
  tempSource: string;
  powerSource: string;
  /** 后端诊断字符串（data.diag，含各层降级原因） */
  diag: string;
}

/** 传感器数据源标识 → 展示文案（前端契约值映射；未知值原样显示兜底） */
const SOURCE_LABELS: Record<string, string> = {
  ntapi: "实时(ntapi)",
  pdh: "估算(pdh)",
  registry: "标称(registry)",
  sysinfo: "标称(sysinfo)",
  none: "无",
  lhm: "LHM 驱动(lhm)",
  winring0: "驱动(winring0)",
  acpi: "低精度(acpi)",
  rapl: "实测(rapl)",
  estimated: "估算(estimated)",
};

/**
 * 频率诊断摘要卡（v0.21.6）：
 * - 仅 FREQ=0（freq_source="none"）时挂载；一次性获取版本号（不进 1s 轮询热路径）
 * - 展示应用版本 + 各传感器数据源 + 后端诊断字符串，一键复制便于用户上报
 * - 复制失败（webview 非 secure context 时 Clipboard API 不可用）降级
 *   textarea + execCommand 兜底，console.warn 记录（不抛出）
 */
function FrequencyDiagCard({
  freqSource,
  tempSource,
  powerSource,
  diag,
}: FrequencyDiagCardProps) {
  const [version, setVersion] = useState("…");
  const [copied, setCopied] = useState(false);

  // 一次性获取应用版本（诊断摘要内容，不进传感器 1s 轮询热路径）
  useEffect(() => {
    let mounted = true;
    invoke<string>("get_app_version")
      .then((v) => {
        if (mounted) setVersion(v);
      })
      .catch((e: unknown) => {
        if (mounted) {
          logger.warn("Dashboard", 
            "get_app_version failed:",
            e instanceof Error ? e.message : String(e),
          );
          setVersion("unknown");
        }
      });
    return () => {
      mounted = false;
    };
  }, []);

  // 组装可复制的诊断摘要（多行文本，便于粘贴到 Issue / 聊天窗口）
  const summaryText = [
    `SECM v${version} 频率诊断`,
    `温度源: ${tempSource} | 功耗源: ${powerSource} | 频率源: ${freqSource}`,
    `diag: ${diag}`,
  ].join("\n");

  const handleCopy = async () => {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(summaryText);
      } else {
        // Tauri webview 非 secure context：textarea + execCommand 兜底
        const ta = document.createElement("textarea");
        ta.value = summaryText;
        ta.style.position = "fixed";
        ta.style.opacity = "0";
        document.body.appendChild(ta);
        ta.select();
        const ok = document.execCommand("copy");
        document.body.removeChild(ta);
        if (!ok) {
          throw new Error("execCommand copy returned false");
        }
      }
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch (e: unknown) {
      logger.warn("Dashboard", 
        "复制诊断摘要失败:",
        e instanceof Error ? e.message : String(e),
      );
    }
  };

  return (
    <Card className="animate-fade-in border-amber-500/30 bg-amber-500/5">
      <CardHeader className="flex flex-row items-start justify-between gap-3 pb-2">
        <div className="space-y-1">
          <CardTitle className="text-sm">
            CPU 频率数据缺失 · 诊断摘要
          </CardTitle>
          <CardDescription className="text-xs">
            点击复制后发送给开发者，便于定位降级原因
          </CardDescription>
        </div>
        <Button
          size="sm"
          variant="outline"
          onClick={() => void handleCopy()}
          className="shrink-0"
        >
          <Copy className="h-3.5 w-3.5" />
          {copied ? "已复制" : "复制"}
        </Button>
      </CardHeader>
      <CardContent className="space-y-2">
        {/* 数据源摘要徽标 */}
        <div className="flex flex-wrap items-center gap-1.5">
          <Badge
            variant="outline"
            className="border-amber-500/40 text-amber-400"
          >
            版本 v{version}
          </Badge>
          <Badge variant="outline">
            温度源 {SOURCE_LABELS[tempSource] ?? tempSource}
          </Badge>
          <Badge variant="outline">
            功耗源 {SOURCE_LABELS[powerSource] ?? powerSource}
          </Badge>
          <Badge variant="outline">
            频率源 {SOURCE_LABELS[freqSource] ?? freqSource}
          </Badge>
        </div>
        {/* 后端诊断字符串（含各层降级原因，过长自动换行） */}
        <p className="break-all text-xs text-muted-foreground">{diag}</p>
      </CardContent>
    </Card>
  );
}

/* ── 图表图例（趋势图 / 网络流量图共用） ── */

interface LegendItem {
  /** 色点类（如 bg-blue-500），与 icon 二选一 */
  color?: string;
  icon?: LucideIcon;
  iconClass?: string;
  label: string;
}

function ChartLegend({ items }: { items: LegendItem[] }) {
  return (
    <div className="flex items-center justify-center gap-4 mt-1.5 shrink-0">
      {items.map((item) => (
        <span
          key={item.label}
          className="flex items-center gap-1.5 text-xs text-muted-foreground"
        >
          {item.icon ? (
            <item.icon className={cn("size-3", item.iconClass)} />
          ) : (
            <span className={cn("size-2.5 rounded-full", item.color)} />
          )}
          {item.label}
        </span>
      ))}
    </div>
  );
}

/* ── 网络速率小卡片（下载/上传/总量） ── */

interface NetStatTileProps {
  icon: LucideIcon;
  /** 容器背景/边框类（如 bg-blue-500/5 border border-blue-500/20） */
  tileClass: string;
  /** 图标与数值颜色类 */
  iconClass: string;
  /** 数值颜色类（缺省取默认前景色） */
  valueClass?: string;
  label: string;
  value: string;
}

function NetStatTile({
  icon: Icon,
  tileClass,
  iconClass,
  valueClass,
  label,
  value,
}: NetStatTileProps) {
  return (
    <div className={cn("p-2.5 rounded-lg", tileClass)}>
      <div className="flex items-center gap-1.5 mb-0.5">
        <Icon className={cn("size-3", iconClass)} />
        <span className="text-xs text-muted-foreground">{label}</span>
      </div>
      <div
        className={cn(
          "text-sm font-bold tabular-nums",
          valueClass ?? "text-foreground",
        )}
      >
        {value}
      </div>
    </div>
  );
}

export default function Dashboard() {
  const [data, setData] = useState<SensorData | null>(null);
  const [history, setHistory] = useState<HistoryPoint[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [netHistory, setNetHistory] = useState<NetHistoryPoint[]>([]);

  // [实验D] 恢复 v0.12.1 图表 5s 节流（诊断二分测试用）

  // 图表历史持久化：启动时从硬盘读回上次会话数据（记录到硬盘 → 再一次读取）
  useEffect(() => {
    let mounted = true;
    invoke<string[]>("get_chart_history", { limit: 60 })
      .then((lines) => {
        if (!mounted) return;
        const hw: HistoryPoint[] = [];
        const net: NetHistoryPoint[] = [];
        for (const line of lines) {
          try {
            const p = JSON.parse(line) as {
              type?: string;
              t?: string;
              cpu?: number;
              gpu?: number;
              mem?: number;
              dl?: number;
              ul?: number;
            };
            // 完整时间戳 "YYYY-MM-DD HH:mm:ss" → 图表仅用 "HH:mm:ss" 段
            const time = p.t ? p.t.slice(11) : "";
            if (p.type === "hw" && p.cpu !== undefined && p.mem !== undefined) {
              hw.push({ time, cpu: p.cpu, gpu: p.gpu ?? 0, memory: p.mem });
            } else if (
              p.type === "net" &&
              p.dl !== undefined &&
              p.ul !== undefined
            ) {
              // Rust 端 chart_append 写盘单位为 bytes/s（未 /1024），实时采样为 KB/s —— 恢复时 /1024 统一为 KB
              // 过滤异常采样：负值、非有限数、超 2GB/s（bytes）丢弃，避免撑大 Y 轴
              if (
                Number.isFinite(p.dl) &&
                Number.isFinite(p.ul) &&
                p.dl >= 0 &&
                p.ul >= 0 &&
                p.dl < NET_KB_LIMIT * 1024 &&
                p.ul < NET_KB_LIMIT * 1024
              ) {
                net.push({ time, download: p.dl / 1024, upload: p.ul / 1024 });
              }
            }
          } catch {
            // 跳过损坏行（文件轮转/并发写入残留），不中断恢复
          }
        }
        // 历史 + 当前实时流拼接，仍受 MAX_HISTORY 上限约束
        setHistory((prev) =>
          [...hw.slice(-MAX_HISTORY), ...prev].slice(-MAX_HISTORY),
        );
        setNetHistory((prev) =>
          [...net.slice(-NET_MAX_HISTORY), ...prev].slice(-NET_MAX_HISTORY),
        );
      })
      .catch((e: unknown) => {
        // 读取失败 → 空历史启动（优雅降级，不阻塞实时轮询）
        if (import.meta.env.DEV) {
          logger.warn("Dashboard", 
            "get_chart_history failed:",
            e instanceof Error ? e.message : String(e),
          );
        }
      });
    return () => {
      mounted = false;
    };
  }, []);

  // 单数据源：netStats / 网卡选择 / 轮询间隔均来自 Provider（与侧边栏完全同步）
  const {
    netStats,
    selectedIface,
    setSelectedIface,
    netInterval,
    setNetInterval,
    linkSpeeds,
  } = useNetIface();
  const ifaceNames = netStats?.interfaces.map((i) => i.name) ?? [];

  const NET_OPTIONS = [
    { label: "0.5s", value: 500 },
    { label: "1s", value: 1000 },
    { label: "2s", value: 2000 },
    { label: "3s", value: 3000 },
    { label: "5s", value: 5000 },
  ];

  // 传感器采样（独立 effect，不受网卡选择影响）
  useEffect(() => {
    let mounted = true;
    // 并发防护：驱动挂起（等待超时）期间跳过本轮轮询，避免每秒堆积 pending invoke
    let inFlight = false;
    const fetchData = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        // 超时保护：驱动无响应时 8 秒内给出明确错误，避免轮询永久 pending
        const result = await withTimeout(
          invoke<SensorData>("get_sensor_data"),
          LOAD_TIMEOUT_MS,
          `传感器数据获取超时（超过 ${LOAD_TIMEOUT_MS / 1000} 秒无响应）。传感器采集可能被安全软件拦截或驱动挂起，请检查驱动服务状态。`,
        );
        if (!mounted) return;
        setData(result);
        setError(null);
        const now = new Date();
        const time = `${now.getHours().toString().padStart(2, "0")}:${now.getMinutes().toString().padStart(2, "0")}:${now.getSeconds().toString().padStart(2, "0")}`;
        const maxGpu =
          result.gpu.length > 0
            ? Math.max(...result.gpu.map((g) => g.usage))
            : 0;
        setHistory((prev) => {
          const next = [
            ...prev,
            {
              time,
              cpu: result.cpu.usage,
              gpu: maxGpu,
              memory: result.memory.usage_percent,
            },
          ];
          return next.length > MAX_HISTORY ? next.slice(-MAX_HISTORY) : next;
        });
      } catch (e: unknown) {
        if (mounted)
          setError(
            `传感器数据获取失败: ${e instanceof Error ? e.message : String(e)}`,
          );
      } finally {
        inFlight = false;
      }
    };
    fetchData();
    const interval = setInterval(fetchData, 1000);
    return () => {
      mounted = false;
      clearInterval(interval);
    };
  }, []);

  // 网络历史曲线：随共享 netStats / 网卡选择变化更新（与侧边栏同源）
  useEffect(() => {
    if (!netStats) return;
    const { downloadSpeed, uploadSpeed } = getDisplayNet(
      netStats,
      selectedIface,
    );
    const now = new Date();
    const time = `${now.getHours().toString().padStart(2, "0")}:${now.getMinutes().toString().padStart(2, "0")}:${now.getSeconds().toString().padStart(2, "0")}`;
    // netHistory 是外部轮询数据(netStats)的累积历史，必须在 effect 中追加（合法用例）
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setNetHistory((prev) => {
      const dl = downloadSpeed / 1024;
      const ul = uploadSpeed / 1024;
      // 过滤瞬时异常速率（短 interval 下速率计算毛刺），保持 Y 轴稳定
      if (
        !Number.isFinite(dl) ||
        !Number.isFinite(ul) ||
        dl < 0 ||
        ul < 0 ||
        dl >= NET_KB_LIMIT ||
        ul >= NET_KB_LIMIT
      ) {
        return prev;
      }
      const next = [...prev, { time, download: dl, upload: ul }];
      return next.length > NET_MAX_HISTORY
        ? next.slice(-NET_MAX_HISTORY)
        : next;
    });
  }, [netStats, selectedIface]);

  // 统一展示口径（与侧边栏同一数据源，以网络流量卡片为准）
  const display = getDisplayNet(netStats, selectedIface);
  const displayDownloadSpeed = display.downloadSpeed;
  const displayUploadSpeed = display.uploadSpeed;
  const displayDownloadTotal = display.downloadTotal;
  const displayUploadTotal = display.uploadTotal;
  // 链路协商速度：仅选中具体网卡且有数据时显示（"全部" 视图无单一速度）
  const linkSpeed =
    selectedIface !== "全部" ? linkSpeeds[selectedIface] : undefined;

  // 传感器数据获取失败：整页错误态（不渲染轮询内容）
  if (error) {
    return (
      <Alert variant="destructive" className="animate-fade-in">
        <AlertTriangle className="h-4 w-4" />
        <AlertDescription>{error}</AlertDescription>
      </Alert>
    );
  }

  // 首次加载中：标题区 + 骨架网格
  if (!data) {
    return (
      <div className="flex flex-col gap-6">
        <PageHeader title="硬件信息" description="正在读取硬件传感器数据…" />
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          {[1, 2, 3, 4].map((i) => (
            <Skeleton key={i} className="h-48 rounded-md" />
          ))}
        </div>
      </div>
    );
  }

  // 降级判定：CPU 温度/功耗缺失 → 展示驱动引导卡（杀软拦截内核驱动场景）
  const cpuTempMissing = data.cpu.temperature <= 0;
  const cpuPowerMissing = data.cpu.power_w <= 0;
  const showDriverGuidance = cpuTempMissing || cpuPowerMissing;
  const missingItems = [
    cpuTempMissing ? "CPU 温度" : null,
    cpuPowerMissing ? "CPU 功耗" : null,
  ]
    .filter((x): x is string => x !== null)
    .join("、");

  return (
    <div className="flex flex-col gap-6">
      {/* ── 驱动降级引导卡（杀软拦截内核驱动） ── */}
      {showDriverGuidance && <DriverGuidanceCard missingItems={missingItems} />}

      {/* ── 频率全链路失败：一次性诊断摘要（可复制上报，FREQ=0 场景） ── */}
      {((data.cpu.freq_source ?? "none") === "none" ||
        data.cpu.clock_mhz <= 0) && (
        <FrequencyDiagCard
          freqSource={data.cpu.freq_source ?? "none"}
          tempSource={data.cpu.temp_source ?? "none"}
          powerSource={data.cpu.power_source ?? "none"}
          diag={data.diag}
        />
      )}

      {/* ── 页头：标题 + 在线状态 ── */}
      <PageHeader
        title="硬件信息"
        description={
          <>
            {data.cpu.name} ·{" "}
            {data.gpu.length > 0
              ? data.gpu.map((g) => g.name).join(" · ")
              : "未检测到 GPU"}
          </>
        }
        actions={
          <div className="flex items-center gap-2">
            <LiveIndicator className="size-2.5" />
            <span className="text-xs text-muted-foreground">在线</span>
          </div>
        }
      />

      {/* ── 实时摘要（卡内 stagger-1~3，外层不叠加动画） ── */}
      <div>
        <SectionTitle icon={Gauge} title="实时摘要" className="mb-3" />
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
          {/* CPU 融合卡片 */}
          <SummaryCard
            className="animate-scale-in stagger-1"
            icon={Cpu}
            iconClass="bg-blue-500/10"
            iconColor="text-blue-400"
            label="CPU"
            meta={`${data.cpu.core_count} 核`}
            value={data.cpu.usage.toFixed(0)}
            progressValue={data.cpu.usage}
            progressClass="[&>div]:bg-blue-500"
            stats={[
              {
                icon: Zap,
                iconClass: "text-red-400",
                text:
                  data.cpu.temperature > 0
                    ? `${data.cpu.temperature.toFixed(0)}°C`
                    : "—",
                badge:
                  data.cpu.temperature > 0 &&
                  data.cpu.temp_source === "acpi" ? (
                    <Badge
                      variant="outline"
                      className="h-4 px-1 text-[10px] font-normal"
                    >
                      低精度
                    </Badge>
                  ) : undefined,
              },
              {
                icon: TrendingUp,
                iconClass: "text-blue-300",
                text:
                  data.cpu.clock_mhz > 0
                    ? `${(data.cpu.clock_mhz / 1000).toFixed(1)} GHz`
                    : "—",
                // 频率数据源角标：pdh=实时估算（secondary，同功耗"估算"角标）；
                // registry/sysinfo=静态标称（outline，同温度"低精度"角标）；ntapi/缺失/其它不加角标
                badge:
                  data.cpu.clock_mhz > 0 &&
                  (data.cpu.freq_source === "pdh" ||
                    data.cpu.freq_source === "registry" ||
                    data.cpu.freq_source === "sysinfo") ? (
                    <Badge
                      variant={
                        data.cpu.freq_source === "pdh"
                          ? "secondary"
                          : "outline"
                      }
                      className="h-4 px-1 text-[10px] font-normal"
                    >
                      {data.cpu.freq_source === "pdh" ? "估算" : "标称"}
                    </Badge>
                  ) : undefined,
              },
              {
                icon: Zap,
                iconClass: "text-yellow-400",
                text:
                  data.cpu.power_w > 0
                    ? data.cpu.power_estimated
                      ? `≈${data.cpu.power_w.toFixed(1)}W`
                      : `${data.cpu.power_w.toFixed(1)}W`
                    : "—",
                badge:
                  data.cpu.power_estimated && data.cpu.power_w > 0 ? (
                    <Badge
                      variant="secondary"
                      className="h-4 px-1 text-[10px] font-normal"
                    >
                      估算
                    </Badge>
                  ) : undefined,
              },
            ]}
          />

          {/* GPU 融合卡片（无 GPU 时显示占位） */}
          <SummaryCard
            className="animate-scale-in stagger-2"
            icon={Thermometer}
            iconClass="bg-green-500/10"
            iconColor="text-green-400"
            label="GPU"
            value={data.gpu.length > 0 ? data.gpu[0].usage.toFixed(0) : ""}
            progressValue={data.gpu.length > 0 ? data.gpu[0].usage : 0}
            progressClass="[&>div]:bg-green-500"
            stats={
              data.gpu.length > 0
                ? [
                    {
                      icon: Zap,
                      iconClass: "text-orange-400",
                      text:
                        data.gpu[0].temperature > 0
                          ? `${data.gpu[0].temperature.toFixed(0)}°C`
                          : "—",
                    },
                    {
                      icon: TrendingUp,
                      iconClass: "text-green-300",
                      text:
                        data.gpu[0].clock_mhz > 0
                          ? `${(data.gpu[0].clock_mhz / 1000).toFixed(1)} GHz`
                          : "—",
                    },
                    {
                      icon: Zap,
                      iconClass: "text-yellow-400",
                      text:
                        data.gpu[0].power_w > 0
                          ? `${data.gpu[0].power_w.toFixed(1)}W`
                          : "—",
                    },
                  ]
                : []
            }
            fallback={
              data.gpu.length === 0 ? (
                <p className="text-sm text-muted-foreground">未检测到 GPU</p>
              ) : undefined
            }
          />

          {/* 内存卡片 */}
          <SummaryCard
            className="animate-scale-in stagger-3"
            icon={MemoryStick}
            iconClass="bg-yellow-500/10"
            iconColor="text-yellow-400"
            label="内存"
            meta={
              data.memory.model_name && data.memory.model_name.trim().length > 0
                ? data.memory.model_name
                : undefined
            }
            value={data.memory.usage_percent.toFixed(0)}
            progressValue={data.memory.usage_percent}
            progressClass={cn(
              data.memory.usage_percent > 90
                ? "[&>div]:bg-red-500"
                : data.memory.usage_percent > 70
                  ? "[&>div]:bg-yellow-500"
                  : "[&>div]:bg-blue-500",
            )}
            stats={[
              { text: `${formatBytes(data.memory.used)} 已用` },
              {
                text: `${formatBytes(data.memory.total)} 总计`,
                className: "ml-auto",
              },
            ]}
          />

        </div>
      </div>

      {/* ── 主板传感器卡（LHM SuperIO；无数据不渲染） ── */}
      {data.motherboard && data.motherboard.sensors.length > 0 && (
        <MotherboardCard mb={data.motherboard} />
      )}

      {/* ── Trend Chart + 存储 + 网络流量（并排） ── */}
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-4">
        {/* ── 硬件使用趋势 ── */}
        {/* py-4 覆盖 shadcn Card 默认 py-6，减少卡片内部留白 */}
        <Card className="animate-scale-in stagger-4 py-4">
          <CardHeaderWithIcon
            className="px-4 pb-1"
            icon={TrendingUp}
            iconClass="bg-blue-500/10"
            iconColor="text-blue-400"
            title="硬件使用趋势"
            description="最近 60 秒 CPU / GPU / 内存占用变化"
          />
          <CardContent className="px-3 pb-2 flex-1 flex flex-col">
            {/* 图表区弹性填满等高剩余空间（底部零留白），标签组贴底 */}
            <div className="flex-1 min-h-0 flex flex-col">
              <HardwareTrendChart data={history} />
            </div>
            <ChartLegend
              items={[
                { color: "bg-blue-500", label: "CPU" },
                { color: "bg-green-500", label: "GPU" },
                { color: "bg-yellow-500", label: "内存" },
              ]}
            />
          </CardContent>
        </Card>

        {/* 存储卡片（全部硬盘直接在主卡片内同期显示：读写速度 + 容量信息） */}
        {/* 速度：PDH 通配计数器按卷独立采样（per-instance，任务管理器同源） */}
        <StorageCard
          className="animate-scale-in stagger-5 py-4"
          icon={HardDrive}
          iconClass="bg-purple-500/10"
          iconColor="text-purple-400"
          label="存储"
          meta={`${data.disks.length} 磁盘`}
          disks={data.disks}
        />

        {/* ── 网络流量 ── */}
        <Card className="animate-scale-in stagger-6 py-4">
          <CardHeaderWithIcon
            className="px-3 pt-0 pb-1"
            icon={Network}
            iconClass="bg-blue-500/10"
            iconColor="text-blue-400"
            title="网络流量"
            description={
              netStats
                ? `${netStats.active_connections} 个活跃连接${linkSpeed ? ` · ${linkSpeed}` : ""}`
                : "流量统计 · 活动连接"
            }
            action={
              <div className="flex items-center gap-2">
                <Select
                  value={String(netInterval)}
                  onValueChange={(v) => setNetInterval(Number(v))}
                >
                  <SelectTrigger className="h-7 w-[68px] text-xs">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {NET_OPTIONS.map((opt) => (
                      <SelectItem key={opt.value} value={String(opt.value)}>
                        {opt.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Select
                  value={selectedIface}
                  onValueChange={(v) => {
                    setSelectedIface(v);
                    setNetHistory([]);
                  }}
                >
                  <SelectTrigger className="h-7 max-w-[130px] text-xs truncate">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="全部">全部网卡</SelectItem>
                    {ifaceNames.map((name) => (
                      <SelectItem key={name} value={name}>
                        {name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            }
          />
          <CardContent className="px-3 pt-0 pb-2">
            {!netStats ? (
              <div className="flex flex-col gap-3">
                <div className="grid grid-cols-2 gap-3">
                  {[1, 2, 3, 4].map((i) => (
                    <Skeleton key={i} className="h-[60px] rounded-md" />
                  ))}
                </div>
                <Skeleton className="h-[180px] rounded-md" />
              </div>
            ) : (
              <>
                <div className="grid grid-cols-2 gap-2 mb-2">
                  <NetStatTile
                    icon={ArrowDown}
                    tileClass="bg-blue-500/5 border border-blue-500/20"
                    iconClass="text-blue-400"
                    valueClass="text-blue-400"
                    label="下载"
                    value={`${formatBytes(displayDownloadSpeed)}/s`}
                  />
                  <NetStatTile
                    icon={ArrowUp}
                    tileClass="bg-green-500/5 border border-green-500/20"
                    iconClass="text-green-400"
                    valueClass="text-green-400"
                    label="上传"
                    value={`${formatBytes(displayUploadSpeed)}/s`}
                  />
                  <NetStatTile
                    icon={ArrowDown}
                    tileClass="bg-muted/30"
                    iconClass="text-muted-foreground"
                    label="下载总量"
                    value={formatBytes(displayDownloadTotal)}
                  />
                  <NetStatTile
                    icon={ArrowUp}
                    tileClass="bg-muted/30"
                    iconClass="text-muted-foreground"
                    label="上传总量"
                    value={formatBytes(displayUploadTotal)}
                  />
                </div>
                <NetTrendChart data={netHistory} />
                <ChartLegend
                  items={[
                    { color: "bg-blue-500", label: "下载" },
                    { color: "bg-green-500", label: "上传" },
                    {
                      icon: Activity,
                      iconClass: "text-muted-foreground",
                      label: `${netStats.active_connections} 连接`,
                    },
                  ]}
                />
              </>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
