import { logger } from "../lib/logger";
import { memo, useEffect, useRef, useState } from "react";
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
  Boxes,
  Layers,
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
  Info,
  RefreshCw,
  ExternalLink,
  Copy,
  CircuitBoard,
  Fan,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { cn, formatBytes, withTimeout } from "../lib/utils";
import { getDisplayNet, useNetIface } from "../contexts/NetIfaceContext";
import { LiveIndicator } from "../components/LiveIndicator";
import { PageHeader } from "../components/PageHeader";
import { CardHeaderWithIcon } from "../components/CardHeaderWithIcon";
import { SectionTitle } from "../components/SectionTitle";
import { safeInvoke } from "../components/environment-shared";

/** 运行数据落点（与后端 `runtime_paths::RuntimePathsInfo` 契约一致） */
interface RuntimePathsInfo {
  root: string;
  logs: string;
  diag: string;
  sidecar_log: string;
}

interface SensorData {
  cpu: {
    name: string;
    usage: number;
    per_core: number[];
    core_count: number;
    clock_mhz: number;
    /**
     * 频率数据源："pdh" | "registry" | "none"
     * （契约 v3 §7 收窄：旧版内核驱动 / 兜底探测通道已整体移除；
     *  旧版后端缺失该字段时为 undefined，须兼容）
     */
    freq_source?: string;
    temperature: number;
    power_w: number;
    /** 温度数据源："lhm" | "none"（契约 v3 §7 收窄；旧版后端缺失时为 undefined） */
    temp_source?: string;
    /** 功耗数据源："lhm" | "estimated" | "none"（契约 v3 §7 收窄，已无硬件计量通道） */
    power_source?: string;
    /** true = 估算功耗（前端显示"估算"角标） */
    power_estimated?: boolean;
    /**
     * CPU 占用率是否为有效采样（false/缺失 = 进程内首次采样未完成）。
     *
     * sysinfo 首次 `refresh_cpu()` 拿不到差分基准，必然返回 100% 伪值
     * （实测独立探针第 1 次 100.00%、第 2 次 9.64%；生产 chart_history
     * 27 个会话中 23 个首采样为 100%）。首帧渲染该伪值会让用户看到
     * "CPU 100%" 满格进度条。缺失（旧版后端）按 true 处理：退化为原行为。
     */
    usage_ready?: boolean;
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
  /**
   * tier-2（管理员）权限状态：CONTRACT-v3 §3 由 sidecar 提供。
   * 后端（task-2）尚未投影这三个字段时缺失 → undefined，前端退化为按
   * `cpu.temp_source !== "lhm"` 推断 tier-2 不可用（不阻塞渲染）。
   */
  tier2_available?: boolean;
  /** tier-2 不可用原因（如「需要管理员权限（PawnIO 设备仅 SYSTEM/Administrators 可访问）」） */
  tier2_reason?: string | null;
  /** tier-2 是否已生效（提权伴生进程已贡献数据） */
  elevated?: boolean;
}

/** driver_status / driver_install 命令返回的安全软件信息（与后端 serde 契约对齐） */
export interface SecuritySoftwareInfo {
  /** 内部标识：defender | huorong | 360 | ... */
  name: string;
  /** 展示名（如 "火绒安全软件"） */
  display_name: string;
  /** 检测到运行中 */
  active: boolean;
  /** 是否命中已知拦截对象（PawnIO） */
  blocked_driver: boolean;
}

/** driver_status / driver_install 命令返回的驱动状态（与后端 serde 契约对齐） */
export interface DriverStatus {
  /**
   * 当前生效的采集通道："pawnio" | "none"
   * （v3 起内核驱动只剩 PawnIO；此处不做穷尽 switch，未知取值仍原样渲染。）
   */
  channel: "pawnio" | "none";
  /** 是否已有可用的驱动通道 */
  available: boolean;
  /** PawnIO 服务状态（SCM 查询结果）："running" | "stopped" | "not_installed" | "unknown" */
  service_state: string;
  secure_boot: boolean;
  testsigning: boolean;
  /** 降级原因链（逐条展示给用户） */
  degraded: string[];
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
 *
 * 注意：v3 起内核驱动只剩 PawnIO.sys，放行对象一律是 PawnIO.sys。
 */
const SECURITY_GUIDE: Readonly<Record<string, readonly string[]>> = {
  defender: [
    "打开 Windows 安全中心 → 设备安全性 → 内核隔离，关闭「内存完整性」开关",
    "在「易受攻击的驱动程序阻止列表」中关闭阻止列表，或把 PawnIO.sys 加入排除",
  ],
  huorong: [
    "打开火绒安全 → 防护中心 → 信任区 → 添加「SECM 安装/运行目录」与「PawnIO.sys」",
    "或在火绒 → 设置 → 驱动防护中允许 SECM 加载内核驱动",
    "添加完成后点击下方「一键重试」重新检测",
    "若提示服务标记删除/驱动加载被拒（错误码 0x431/0x430），请重启电脑后再试",
  ],
  "360": [
    "打开 360 安全卫士 → 木马查杀 → 信任区，添加 SECM 安装目录",
    "如弹窗提示拦截驱动加载，选择「允许 / 信任」",
  ],
  tencent: ["打开腾讯电脑管家 → 病毒查杀 → 信任区，添加 SECM 安装目录"],
  kaspersky: [
    "打开卡巴斯基 → 设置 → 排除项与受信任程序，添加 SECM 为受信任程序",
  ],
  norton: ["打开诺顿 → 设置 → 防病毒排除项，添加 SECM 安装目录"],
  eset: ["打开 ESET 安全设置 → 检测引擎排除项，添加 SECM 安装目录"],
  lenovo: ["打开联想电脑管家 → 信任中心 → 白名单，添加 SECM 安装目录"],
};

/** 未识别杀软时的通用配置指引 */
const SECURITY_GUIDE_GENERIC: readonly string[] = [
  "在杀毒软件的信任区 / 白名单中添加 SECM 安装目录，以及内核驱动 PawnIO.sys",
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
 * request_elevation 调用超时（毫秒）：含 UAC 弹窗的用户思考时间，
 * 不能按普通 invoke 的 5s 计时（用户在弹窗上停留超过 5s 是常态）。
 */
const ELEVATION_TIMEOUT_MS = 120000;

/**
 * auto_elevate 调用超时（毫秒）。
 *
 * 该命令只走零 UAC 路径（schtasks /Run + 有界等待 tier-2，最长约 8s），
 * 不含用户在 UAC 弹窗上的思考时间，故预算远小于 ELEVATION_TIMEOUT_MS。
 * 超时视为不可行/瞬时故障，交由权限卡按钮重试，不阻塞页面。
 */
const AUTO_ELEVATE_TIMEOUT_MS = 20000;

/**
 * 首屏"引导加载"最长等待（毫秒）。
 *
 * 背景（发布版实测时间线）：tier-1 的 PDH 计数器需要两次采样才能出数，
 * 冷启动约 2.5–4.4s 内 `clock_mhz`/disk 速率均为空；更早的采样还会拿到
 * sidecar `available:false`。
 *
 * 现状说明（重要）：`hasSubstantialData` 纳入 tier-1 首帧信号（内存/CPU）后，
 * 任何真实快照都会满足它，因此在 Windows 上 `bootstrapping` 实际恒为 false
 * ——骨架屏只由 `!data`（首个响应未到）决定。本常量因此是**纯保险**：
 * 它只在"快照已返回但无实质信号"这一理论分支起作用，在正常机器上不可达。
 * 保留它是为了在采集彻底异常（`SensorData` 全零）时仍有确定的结束点。
 *
 * 首帧值伪值问题由 `cpuSampleReady`（后端 `cpu.usage_ready`）单独处理：
 * sysinfo 首次采样恒为 100%，故首帧 CPU 主数值留空并显示"正在采样…"，
 * 而不是把一个假的 100% 呈现给用户。
 */
const BOOTSTRAP_GRACE_MS = 8000;

/**
 * 首屏引导判据：快照是否已含"实质数据"。
 *
 * 只要出现任一真实信号即可结束等待：
 * - 内存总量（`GlobalMemoryStatusEx`，tier-1，**首帧即有真实值**）
 * - CPU 负载（sysinfo，tier-1，首帧即有值）
 * - GPU（厂商用户态通道，无需授权）
 * - CPU 时钟（PDH 预热完成后即出数）
 * - 温度（tier-2 生效）
 *
 * **不等待授权**：tier-2 未授权时也应尽快用 tier-1 数据渲染完整页面，
 * 授权卡在加载完成后才判定。
 *
 * 设计修正（实测）：旧判据只认 `gpu || clock_mhz || temperature`，三者全部来自
 * LHM sidecar 的 PDH 计数器——需两次采样才出数。生产日志（2026-09-20）显示：
 * 应用 15:17:16.689 启动，首个"含实质数据"的快照出现在 **15:17:28.773（+12.1s）**，
 * 而同期 CPU 负载与内存总量在首个 1ms 快照里就已可用。于是用户在整个 PDH 预热
 * 窗口内（远超 8s 上限）干看骨架屏——这正是「加载时间过久」的用户可见部分，
 * 与后端首个请求阻塞（4.1s）是两个独立成因。
 *
 * 加入内存/CPU 这两个**首帧即可用**的 tier-1 信号后，页面在后端预热完成即刻
 * 渲染真实数据；GPU/时钟/温度随后到达时原地填充（缺项仍显示「—」）。
 * `BOOTSTRAP_GRACE_MS` 保留为无数据机器上的兜底上限。
 */
function hasSubstantialData(d: SensorData | null): boolean {
  if (!d) return false;
  return (
    d.gpu.length > 0 ||
    d.cpu.clock_mhz > 0 ||
    d.cpu.temperature > 0 ||
    // tier-1 首帧即可用：内存总量恒 > 0（GlobalMemoryStatusEx 同步返回）
    d.memory.total > 0 ||
    // CPU 核数由 sysinfo 首帧即给出
    d.cpu.core_count > 0
  );
}

/**
 * 采集管线是否仍在「预热」：sidecar 的 PDH 计数器尚未产出任何真实信号。
 *
 * 与 [`hasSubstantialData`] 的区别：后者现在可由 tier-1 首帧信号（内存/CPU）
 * 满足，用于**尽早结束骨架屏**；本判据只认 sidecar 侧信号
 * （GPU/时钟/温度），用于**门控"数据缺失类"提示卡**。
 *
 * 为什么必须分开：`Tier2PrivilegeCard`/`DriverGuidanceCard`/`FrequencyDiagCard`
 * 都是"某个指标确实拿不到"的结论。预热期 GPU 列表必为空、`clock_mhz` 必为 0，
 * 此时渲染它们会给出**错误结论**（例如 PDH 还没出数就断言"CPU 频率数据缺失"，
 * 或误报驱动被拦截）。因此这些卡只在 sidecar 首次出数后才允许出现——与修复前
 * 的时机一致（修复前骨架屏恰好也等到这一刻）。
 *
 * `deadline_passed`：超过 [`PIPELINE_WARMUP_MAX_MS`] 后强制返回 false，
 * 避免"sidecar 始终起不来"的机器永远显示不了本该显示的诊断卡。
 */
function isPipelineWarmingUp(
  d: SensorData | null,
  deadline_passed: boolean,
): boolean {
  if (deadline_passed) return false;
  if (!d) return true;
  return d.gpu.length === 0 && d.cpu.clock_mhz <= 0 && d.cpu.temperature <= 0;
}

/**
 * 采集管线"预热"判定的兜底时限（毫秒）。
 *
 * [`isPipelineWarmingUp`] 只认 sidecar 侧信号，因此在"sidecar 永远起不来 /
 * 该机确实没有 GPU 且 PDH 拿不到频率"的机器上它会**恒为真**，把
 * `FrequencyDiagCard`（正是为这种机器准备的诊断卡）永久挡住。
 * 超过本时限后一律不再视为预热，让诊断卡按原有条件出现。
 *
 * 取值须大于 sidecar 内部实测预热上界（冷机 8.4s，见生产日志 9.1–12.1s），
 * 否则会在真实预热尚未结束时就把"数据缺失"结论过早呈现给用户。
 */
const PIPELINE_WARMUP_MAX_MS = 15000;

// 此处曾有 `withTimeout` 的本地副本，与 `src/lib/utils.ts` 的同名导出逐行同构。
// Cleanup / Hardware / DiskSmartDialog 都从 lib/utils 导入，只有本文件用副本，
// 后续修复（如接 AbortSignal）会漏改（ADR-0001 D14：合并到唯一实现）。

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
            <stop
              offset="5%"
              stopColor="var(--color-brand-blue)"
              stopOpacity={0.25}
            />
            <stop
              offset="95%"
              stopColor="var(--color-brand-blue)"
              stopOpacity={0}
            />
          </linearGradient>
          <linearGradient id="gpuGrad" x1="0" y1="0" x2="0" y2="1">
            <stop
              offset="5%"
              stopColor="var(--color-brand-green)"
              stopOpacity={0.25}
            />
            <stop
              offset="95%"
              stopColor="var(--color-brand-green)"
              stopOpacity={0}
            />
          </linearGradient>
          <linearGradient id="memGrad" x1="0" y1="0" x2="0" y2="1">
            <stop
              offset="5%"
              stopColor="var(--color-brand-yellow)"
              stopOpacity={0.2}
            />
            <stop
              offset="95%"
              stopColor="var(--color-brand-yellow)"
              stopOpacity={0}
            />
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
            <stop
              offset="5%"
              stopColor="var(--color-brand-blue)"
              stopOpacity={0.25}
            />
            <stop
              offset="95%"
              stopColor="var(--color-brand-blue)"
              stopOpacity={0}
            />
          </linearGradient>
          <linearGradient id="ulGradDash" x1="0" y1="0" x2="0" y2="1">
            <stop
              offset="5%"
              stopColor="var(--color-brand-green)"
              stopOpacity={0.25}
            />
            <stop
              offset="95%"
              stopColor="var(--color-brand-green)"
              stopOpacity={0}
            />
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

/* ── 实时概览卡（CPU / GPU / 内存 合并为单卡） ── */

/**
 * 概览卡的输入快照：只声明卡片真正读取的字段，便于测试与复用。
 * 字段名与后端 `SensorData` 完全一致（契约不变）。
 */
interface OverviewSnapshot {
  cpu: {
    usage: number;
    core_count: number;
    clock_mhz: number;
    temperature: number;
    power_w: number;
    power_estimated?: boolean;
    freq_source?: string;
  };
  gpu: SensorData["gpu"];
  memory: SensorData["memory"];
}

interface OverviewCardProps {
  data: OverviewSnapshot;
  /** false = CPU 占用率为进程内首采样伪值，主数值留空并提示"正在采样…" */
  cpuSampleReady: boolean;
  /** 采集管线是否仍在预热（预热期 GPU 列表必为空，此时不能断言"未检测到 GPU"） */
  warmingUp: boolean;
  className?: string;
}

/**
 * 合并概览卡的内部数据源标识：仅自身使用，不参与对外契约。
 * 用"文字 + 语义色"而非角标承载口径，因为角标用于表示异常状态。
 */
interface SourceTag {
  text: string;
  className: string;
}

/** 值 + 单位 + 来源标识的展示项 */
interface OverviewValue {
  text: string;
  suffix?: string;
  /** 来源/口径标识（如"估算""标称"）；异常时由 tone 覆盖 */
  source?: SourceTag;
  /** 语义色覆盖（用于"—"等无效值，弱化呈现、避免被误读为正常值） */
  tone?: string;
}

/** 单条进度度量（占用率可量化时渲染进度条，否则退化为同高的占位线保持列对齐） */
interface OverviewMetric {
  label: string;
  icon: LucideIcon;
  /** 标签行右侧的静态说明（如"32 核"、内存型号）；过长时截断 */
  meta?: string;
  /** 图标着色类 */
  iconColor: string;
  /** 进度条颜色类（写成 [&>div]:bg-* 覆盖 Progress 指示条） */
  barClass: string;
  /** 主数值（含百分号） */
  value: string;
  /** 进度值 0~100；null = 无度量（渲染占位线） */
  progress: number | null;
  /** 度量说明（如"实时占用率"） */
  caption: string;
}

/** 指标常态色标：CPU 蓝 / GPU 绿 / 内存黄（与趋势图图例、网络卡片同一套语义色） */
const OV_METRIC_TONE = {
  cpu: { iconColor: "text-blue-400", barClass: "[&>div]:bg-blue-500" },
  gpu: { iconColor: "text-green-400", barClass: "[&>div]:bg-green-500" },
  memory: { iconColor: "text-yellow-400", barClass: "[&>div]:bg-yellow-500" },
} as const;

const OV_MUTED = "text-muted-foreground";

/** 功耗为估算值时的口径标识 */
const OV_POWER_ESTIMATED: SourceTag = {
  text: "估算",
  className: "text-muted-foreground",
};

/** 频率数据源 → 口径标识（"估算"= PDH 实时推算，"标称"= 注册表静态值） */
const OV_FREQ_SOURCE: Record<string, SourceTag> = {
  pdh: { text: "估算", className: "text-blue-300/80" },
  registry: { text: "标称", className: "text-muted-foreground" },
};

/** 带单位的数值：无效值只给单位占位符，避免出现"— °C"这类读不通的组合 */
function ovUnit(v: number, digits: number, unit: string): OverviewValue {
  const ok = Number.isFinite(v) && v > 0;
  return ok
    ? { text: v.toFixed(digits), suffix: unit }
    : { text: "—", tone: OV_MUTED };
}

/** 温度语义色：<60 绿 / <80 橙 / ≥80 红；无数据时弱化 */
function ovTempTone(v: number): string {
  if (!Number.isFinite(v) || v <= 0) return OV_MUTED;
  if (v >= 80) return "text-red-400";
  if (v >= 60) return "text-orange-400";
  return "text-green-400";
}

/** 内存占用分级色：>90 红 / >70 黄 / 其余蓝；非有限值回落蓝，不产生 NaN 比较 */
function ovMemBarClass(pct: number): string {
  if (pct > 90) return "[&>div]:bg-red-500";
  if (pct > 70) return "[&>div]:bg-yellow-500";
  return "[&>div]:bg-blue-500";
}

/**
 * 设备列头：图标 + 名称 + 静态说明（核数 / 适配器数 / 内存型号）→ 大号占用率
 * → 进度条 → 口径说明。同一台硬件的全部数据收在同一列内，列头与列内读数由同一
 * 容器承载，不再拆成上下两条互不对齐的带。
 */
function OverviewColumnHead({ metric }: { metric: OverviewMetric }) {
  const Icon = metric.icon;
  return (
    <div className="flex min-w-0 flex-col gap-2">
      <div className="flex min-w-0 items-center gap-2">
        <span className="flex size-6 shrink-0 items-center justify-center rounded-md bg-muted/50">
          <Icon className={cn("size-3.5", metric.iconColor)} aria-hidden />
        </span>
        <span className="shrink-0 text-xs font-medium tracking-wide text-muted-foreground">
          {metric.label}
        </span>
        {metric.meta && (
          <span
            className="ml-auto min-w-0 truncate text-[10px] text-muted-foreground"
            title={metric.meta}
          >
            {metric.meta}
          </span>
        )}
      </div>
      {/* 数字字距收紧：近距数字成组；标签放宽、远距标签成组（排版细节） */}
      <div className="flex items-baseline gap-1">
        <span className="text-3xl font-semibold leading-none tracking-tight tabular-nums">
          {metric.value}
        </span>
      </div>
      {metric.progress === null ? (
        <div className="h-1 w-full rounded-full bg-muted/60" aria-hidden />
      ) : (
        <Progress
          value={metric.progress}
          className={cn("h-1", metric.barClass)}
        />
      )}
      <p className="text-[11px] leading-snug text-muted-foreground">
        {metric.caption}
      </p>
    </div>
  );
}

/** 设备列内的一条读数（温度 / 频率 / 功耗；内存为 已用 / 可用 / 总计） */
interface OverviewReadingItem {
  /** 图标可选：内存三项无同类图标可用，省略后仅由文字标签区分 */
  icon?: LucideIcon;
  iconClass?: string;
  label: string;
  value: OverviewValue;
}

/** 一台硬件一列：列头（占用率）+ 该硬件自己的读数行 */
interface OverviewColumn {
  metric: OverviewMetric;
  readings: OverviewReadingItem[];
}

/**
 * 列内读数行：名称在左、数值在右，同一列内上下堆叠。数值右对齐后
 * 各列宽度独立，窄容器下也不会互相挤压。
 */
function OverviewReadingRow({
  icon: Icon,
  iconClass,
  label,
  value,
}: OverviewReadingItem) {
  return (
    <div className="flex min-w-0 items-center gap-2 text-xs">
      {Icon ? (
        <Icon className={cn("size-3 shrink-0", iconClass)} aria-hidden />
      ) : (
        // 无图标时保留等宽占位，使标签左边缘在三列间对齐
        <span className="size-3 shrink-0" aria-hidden />
      )}
      <span className="min-w-0 truncate text-muted-foreground">{label}</span>
      <span
        className={cn(
          "ml-auto flex shrink-0 items-baseline gap-1 font-medium tabular-nums",
          value.tone,
        )}
      >
        {value.text}
        {value.suffix && (
          <span className="text-[10px] font-normal text-muted-foreground">
            {value.suffix}
          </span>
        )}
        {value.source && (
          <span
            className={cn("text-[10px] font-normal", value.source.className)}
          >
            {value.source.text}
          </span>
        )}
      </span>
    </div>
  );
}

/**
 * 实时概览卡：CPU / GPU / 内存 各占一列，同一台硬件的全部数据（占用率与
 * 温度 / 频率 / 功耗；内存为容量三项）都收在自己那一列内，
 * 不再拆成「上行占用率、下行读数」两条互不对齐的带。
 *
 * 数据缺失一律呈现「—」，绝不补 0（契约 v3：tier-2 未授权时温度/功耗为 null）。
 * 预热期 GPU 列表必为空，此时给出「正在采集…」，避免断言「未检测到 GPU」的错误结论。
 * 多显卡时列头与列内读数均取自占用率最高的一张，不混用不同适配器的数据。
 */
function OverviewCard({
  data,
  cpuSampleReady,
  warmingUp,
  className,
}: OverviewCardProps) {
  // 多显卡时取占用率最高的一张（与趋势图的 maxGpu 口径一致）；列头与列内读数
  // 必须取自同一块卡，否则一列里会混入两张适配器的数据。
  const gpuMain: SensorData["gpu"][number] | null =
    data.gpu.length === 0
      ? null
      : data.gpu.reduce((a, b) => (b.usage > a.usage ? b : a));
  const gpuAvailable = gpuMain !== null;
  const memPct = data.memory.usage_percent;
  const memPctValid = Number.isFinite(memPct);
  const cpuPowerValid =
    Number.isFinite(data.cpu.power_w) && data.cpu.power_w > 0;

  const columns: OverviewColumn[] = [
    {
      metric: {
        label: "CPU",
        icon: Boxes,
        ...OV_METRIC_TONE.cpu,
        meta: data.cpu.core_count > 0 ? data.cpu.core_count + " 核" : undefined,
        value: cpuSampleReady ? data.cpu.usage.toFixed(0) + "%" : "—",
        progress: cpuSampleReady ? data.cpu.usage : null,
        caption: cpuSampleReady ? "实时占用率" : "正在采样…",
      },
      readings: [
        {
          icon: Thermometer,
          iconClass: ovTempTone(data.cpu.temperature),
          label: "温度",
          value: ovUnit(data.cpu.temperature, 0, "°C"),
        },
        {
          icon: TrendingUp,
          iconClass: "text-blue-300",
          label: "频率",
          value: {
            ...ovUnit(data.cpu.clock_mhz / 1000, 1, "GHz"),
            source: OV_FREQ_SOURCE[data.cpu.freq_source ?? ""],
          },
        },
        {
          icon: Zap,
          iconClass: "text-yellow-400",
          label: "功耗",
          value: {
            // 「≈」前缀与「估算」标识同时出现是刻意的冗余：前者利于扫读、后者说明依据
            text: cpuPowerValid
              ? (data.cpu.power_estimated ? "≈" : "") +
                data.cpu.power_w.toFixed(1)
              : "—",
            suffix: cpuPowerValid ? "W" : undefined,
            tone: cpuPowerValid ? undefined : OV_MUTED,
            source: data.cpu.power_estimated ? OV_POWER_ESTIMATED : undefined,
          },
        },
      ],
    },
    {
      metric: {
        label: "GPU",
        icon: Layers,
        ...OV_METRIC_TONE.gpu,
        meta: gpuAvailable ? data.gpu.length + " 个适配器" : undefined,
        value: gpuMain ? gpuMain.usage.toFixed(0) + "%" : "—",
        progress: gpuMain ? gpuMain.usage : null,
        caption: gpuAvailable
          ? "实时占用率"
          : warmingUp
            ? "正在采集…"
            : "未检测到 GPU",
      },
      readings: gpuMain
        ? [
            {
              icon: Thermometer,
              iconClass: ovTempTone(gpuMain.temperature),
              label: "温度",
              value: ovUnit(gpuMain.temperature, 0, "°C"),
            },
            {
              icon: TrendingUp,
              iconClass: "text-green-300",
              label: "频率",
              value: ovUnit(gpuMain.clock_mhz / 1000, 1, "GHz"),
            },
            {
              icon: Zap,
              iconClass: "text-yellow-400",
              label: "功耗",
              value: ovUnit(gpuMain.power_w, 1, "W"),
            },
          ]
        : [],
    },
    {
      metric: {
        label: "内存",
        icon: MemoryStick,
        iconColor: OV_METRIC_TONE.memory.iconColor,
        barClass: memPctValid ? ovMemBarClass(memPct) : OV_MUTED,
        meta: data.memory.model_name?.trim() || undefined,
        value: memPctValid ? memPct.toFixed(0) + "%" : "—",
        progress: memPctValid ? memPct : null,
        caption: memPctValid ? "实时占用率" : "内存容量读取失败",
      },
      readings: [
        { label: "已用", value: { text: formatBytes(data.memory.used) } },
        { label: "可用", value: { text: formatBytes(data.memory.available) } },
        { label: "总计", value: { text: formatBytes(data.memory.total) } },
      ],
    },
  ];

  return (
    <Card className={cn("gap-4 p-5", className)}>
      {/* 卡内区块标题沿用页面既有的 SectionTitle（大写小号 + 图标） */}
      <div className="flex items-center gap-2">
        <SectionTitle icon={Gauge} title="实时摘要" />
        <span className="ml-auto flex items-center gap-1.5 text-[10px] text-muted-foreground">
          <LiveIndicator className="size-2" color="blue" />
          实时
        </span>
      </div>

      {/* 一台硬件一列：列头是该硬件的占用率，列内是该硬件的读数行。
          列间竖线用 border-l 而非 divide（divide 会给被隐藏的列也画线）。 */}
      <div className="grid grid-cols-1 @xl/main:grid-cols-3">
        {columns.map(({ metric, readings }) => (
          <div
            key={metric.label}
            className="flex min-w-0 flex-col gap-3 border-t border-border/60 pt-3 first:border-t-0 first:pt-0 @xl/main:border-l @xl/main:border-t-0 @xl/main:px-4 @xl/main:first:border-l-0 @xl/main:first:pl-0"
          >
            <OverviewColumnHead metric={metric} />
            {readings.length > 0 && (
              <div className="flex flex-col gap-1.5 border-t border-border/50 pt-2.5">
                {readings.map((r) => (
                  <OverviewReadingRow key={r.label} {...r} />
                ))}
              </div>
            )}
          </div>
        ))}
      </div>
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
  {
    label: string;
    unit: string;
    icon: LucideIcon;
    colorClass: string;
    iconClass: string;
  }
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
      <CardContent className="px-4 pb-4 grid grid-cols-1 @md/main:grid-cols-2 @3xl/main:grid-cols-3 gap-3">
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
                      g.kind === "voltage"
                        ? 2
                        : g.kind === "temperature"
                          ? 1
                          : 0,
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
            {disks.map((d, i) => {
              // 进度条颜色：按使用率分级（90% 红 / 70% 黄 / 正常紫）
              const barClass =
                d.usage_percent > 90
                  ? "[&>div]:bg-red-500"
                  : d.usage_percent > 70
                    ? "[&>div]:bg-yellow-500"
                    : "[&>div]:bg-purple-500";
              return (
                <div
                  key={`${d.name}-${i}`}
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
    logger.warn(
      "Dashboard",
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
    logger.warn(
      "Dashboard",
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
  // 运行数据落点（由后端 runtime_paths 解析，界面不硬编码路径）
  const [sidecarLog, setSidecarLog] = useState<string | null>(null);

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
    void safeInvoke<RuntimePathsInfo>("get_runtime_paths").then((p) => {
      if (!mounted || p === null) return;
      setSidecarLog(p.sidecar_log);
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
        logger.warn(
          "Dashboard",
          "open windowsdefender:// returned null (popup may be blocked)",
        );
      }
    } catch (e: unknown) {
      logger.warn(
        "Dashboard",
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
          {missingItems}
          数据不可用。若已在上方完成一次管理员授权仍无数据，通常是安全软件（Windows
          Defender / 火绒 / 360 等）拦截了内核驱动 PawnIO
          的加载，可按下方步骤放行后重试。
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
            LHM 采集日志：
            <code className="rounded bg-muted px-1 font-mono">
              {sidecarLog ?? "加载中…"}
            </code>
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

/* ── tier-2 提权（一次 UAC 解锁 CPU 温度/功耗/电压、主板、磁盘温度） ── */

/**
 * 是否「后端未注册该命令」型错误：Tauri v2 对未注册命令返回
 * `Command <name> not found`（错误类型 UnknownCommand）。
 * 命中时隐藏按钮而非报错 —— 旧版后端或命令尚未落地时优雅降级。
 */
function isUnknownCommandError(e: unknown): boolean {
  const msg = (e instanceof Error ? e.message : String(e)).toLowerCase();
  return msg.includes("not found") || msg.includes("unknown command");
}

/**
 * 提权结果：
 * - ok          已提权（tier-2 生效）
 * - declined    用户在 UAC 弹窗上取消 / 后端返回 false（非致命，被后端记住，不重复弹窗）
 * - unsupported 后端没有 `request_elevation` 命令（旧版后端 / 尚未落地）→ 隐藏按钮
 * - error       其它失败（超时、sidecar 不可达等）→ 允许重试
 */
type ElevationOutcome = "ok" | "declined" | "unsupported" | "error";

/**
 * 触发一次 UAC 提权：调用后端 Tauri 命令 `request_elevation`（无参，返回 bool）。
 * Rust 侧 `Err(String)` 会被 Tauri reject 成字符串，故失败原因须从消息文本判断；
 * 本函数不抛出（返回 outcome 与原因），避免打断 1s 传感器轮询热路径。
 *
 * 原因透出：`error` 分支返回后端的原始消息，供 `Tier2PrivilegeCard` 展示。
 * 此前只把消息写进 `logger.warn`，界面固定显示「提权请求失败，可稍后重试」，
 * 用户与排障都拿不到真实原因（例如 sidecar 未就绪、计划任务注册失败）。
 */
interface ElevationResult {
  outcome: ElevationOutcome;
  /** 失败原因（后端原始消息）；成功或用户取消时为 null */
  reason: string | null;
}

async function requestElevation(): Promise<ElevationResult> {
  try {
    const raised = await withTimeout(
      invoke<boolean>("request_elevation"),
      ELEVATION_TIMEOUT_MS,
      `request_elevation 超时（超过 ${ELEVATION_TIMEOUT_MS / 1000} 秒无响应）`,
    );
    return { outcome: raised ? "ok" : "declined", reason: null };
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : String(e);
    logger.warn("Dashboard", "request_elevation failed:", msg);
    if (isUnknownCommandError(e))
      return { outcome: "unsupported", reason: null };
    // Win32 1223 / 0x04C7 = ERROR_CANCELLED（用户在 UAC 点了「否」）
    if (msg.includes("1223") || msg.includes("0x4c7") || msg.includes("取消")) {
      return { outcome: "declined", reason: null };
    }
    return { outcome: "error", reason: msg };
  }
}

/**
 * 自动提权（零 UAC）：调用后端 `auto_elevate`，只走「管理员直通」与
 * 「已注册持久授权 /Run」两条路径，**绝不弹 UAC**。
 *
 * 返回值语义（与 [`Tier2PrivilegeCard`] 的两步提权流程对应）：
 * - `"ok"`           tier-2 已自动生效（用户零操作）
 * - `"infeasible"`   自动路径不可行（非管理员且无持久授权）→ 才由调用方申请用户授权
 * - `"unsupported"`  旧版后端无该命令 → 不申请，交给权限卡按钮
 * - `"error"`        sidecar 未就绪等瞬时故障 → 不弹窗，下轮轮询/按钮重试
 */
async function autoElevate(): Promise<
  "ok" | "infeasible" | "unsupported" | "error"
> {
  try {
    const raised = await withTimeout(
      invoke<boolean>("auto_elevate"),
      AUTO_ELEVATE_TIMEOUT_MS,
      `auto_elevate 超时（超过 ${AUTO_ELEVATE_TIMEOUT_MS / 1000} 秒无响应）`,
    );
    return raised ? "ok" : "infeasible";
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : String(e);
    if (isUnknownCommandError(e)) return "unsupported";
    logger.warn("Dashboard", "auto_elevate failed:", msg);
    return "error";
  }
}

/**
 * tier-2 权限提示卡（CONTRACT-v3 §1 两级权限模型）：
 * - tier-1（普通用户，无需 UAC）：CPU 负载/频率、内存、GPU、磁盘 IO、网络、电池
 * - tier-2（管理员，一次 UAC）：CPU 温度/功耗/电压、主板 SuperIO 温度/风扇/电压、磁盘温度
 *   —— 只能经 PawnIO ring0 设备读取（DACL 仅 SYSTEM / Administrators）
 * - 按钮调用后端 `request_elevation`；命令缺失/旧版后端 → 隐藏按钮只留说明
 * - 用户在 UAC 取消为非致命且被后端记住（不会每次轮询重复弹窗）
 */
function Tier2PrivilegeCard({ reason }: { reason: string | null }) {
  const [phase, setPhase] = useState<
    "idle" | "busy" | "ok" | "declined" | "unsupported" | "error"
  >("idle");
  /** 提权失败时后端返回的具体原因（用于界面直出，不再只写日志） */
  const [failureReason, setFailureReason] = useState<string | null>(null);

  const handleElevate = async () => {
    setPhase("busy");
    setFailureReason(null);
    const result = await requestElevation();
    setFailureReason(result.reason);
    setPhase(result.outcome);
  };

  return (
    <Alert className="animate-fade-in border-sky-500/30 bg-sky-500/5">
      <Info className="text-sky-400" />
      <AlertTitle>CPU 温度 / 功耗与主板传感器需要管理员授权</AlertTitle>
      <AlertDescription>
        <p>
          SECM 以普通用户权限启动，CPU
          负载/频率、内存、GPU、磁盘与网络无需授权即可读取；但 CPU
          温度/功耗/电压、主板温度/风扇/电压与磁盘温度只能经 PawnIO
          内核驱动获取（该设备仅 SYSTEM / Administrators
          可访问），需要一次管理员授权。
        </p>

        {reason && (
          <p className="mt-2 text-xs text-muted-foreground">
            后端报告：{reason}
          </p>
        )}

        {phase === "ok" && (
          <p className="mt-2 text-xs text-emerald-400">
            已授权。tier-2 指标会在数秒内（驱动就绪后）自动出现在下方卡片中。
          </p>
        )}
        {phase === "declined" && (
          <p className="mt-2 text-xs text-amber-400">
            未获授权（已在 UAC 弹窗中取消）。tier-2
            指标保持不可用；如需启用可再次点击下方按钮。
          </p>
        )}
        {phase === "error" && (
          <>
            <p className="mt-2 text-xs text-destructive">
              提权请求失败，可稍后重试；若持续失败请查看上方日志路径。
            </p>
            {failureReason && (
              <p className="mt-1 text-xs text-destructive/80 break-all">
                原因：{failureReason}
              </p>
            )}
          </>
        )}
        {phase === "unsupported" && (
          <p className="mt-2 text-xs text-muted-foreground">
            当前后端未提供提权命令，无法在此发起管理员授权（请更新到包含该命令的版本）。
          </p>
        )}

        {(phase === "idle" || phase === "declined" || phase === "error") && (
          <div className="mt-3">
            <Button
              size="sm"
              onClick={() => void handleElevate()}
              className="bg-sky-500/15 text-sky-400 hover:bg-sky-500/25"
            >
              <ShieldAlert className="h-3.5 w-3.5" />
              {phase === "idle" ? "授权并启用" : "重新授权"}
            </Button>
          </div>
        )}
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

/**
 * 传感器数据源标识 → 展示文案（前端契约值映射；未知值原样显示兜底）。
 * 只保留 CONTRACT-v3 §7 冻结的取值；旧版已删除通道的标识不再在此出现，
 * 万一后端回传未知值则由 `?? source` 兜底原样展示。
 */
const SOURCE_LABELS: Record<string, string> = {
  pdh: "估算(pdh)",
  registry: "标称(registry)",
  none: "无",
  lhm: "LHM(lhm)",
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
          logger.warn(
            "Dashboard",
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
      logger.warn(
        "Dashboard",
        "复制诊断摘要失败:",
        e instanceof Error ? e.message : String(e),
      );
    }
  };

  return (
    <Card className="animate-fade-in border-amber-500/30 bg-amber-500/5">
      <CardHeader className="flex flex-row items-start justify-between gap-3 pb-2">
        <div className="space-y-1">
          <CardTitle className="text-sm">CPU 频率数据缺失 · 诊断摘要</CardTitle>
          <CardDescription className="text-xs">
            v3 起频率只有两个来源：PDH 性能计数器（tier-1，无需
            UAC）与注册表标称值回退（tier-1）。两者都拿不到时值为
            「—」，不会再尝试任何内核驱动；点击复制后发送给开发者便于定位。
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
  /**
   * 首屏引导加载态（见 BOOTSTRAP_GRACE_MS 注释）。
   *
   * 初始 true；首个"含实质数据"的快照到达即置 false，或超过 BOOTSTRAP_GRACE_MS
   * 后由定时器兜底置 false（保证必然结束）。加载期只渲染骨架屏，
   * **不渲染卡片容器、不渲染授权/降级提示卡**——避免让用户误以为页面在等授权。
   */
  const [bootstrapping, setBootstrapping] = useState(true);

  /**
   * 采集管线"预热"判定的兜底时限状态（见 [`PIPELINE_WARMUP_MAX_MS`]）。
   *
   * 初始 false；挂载后经一次定时器置 true，此后 `isPipelineWarmingUp` 一律
   * 返回 false，保证"数据缺失类"提示卡最终必定能出现。
   */
  const [warmupDeadlinePassed, setWarmupDeadlinePassed] = useState(false);

  /**
   * 进入硬件信息页即后台自动提权（不等待用户授权）。
   *
   * 两步流程（用户诉求：「自动提权不等待授权，非管理员环境无法自动提权才申请用户提权」）：
   *   1. `auto_elevate` —— 零 UAC 路径（管理员直通 / 已注册持久授权 /Run），
   *      可行时用户零操作即拿到 tier-2；**不阻塞**页面加载与轮询。
   *   2. 仅当返回 `infeasible`（确认为非管理员且从未授权）时，才调用
   *      `request_elevation` 申请一次用户授权（UAC）。被用户取消后 sidecar
   *      会记住该状态，本会话不再重复弹窗（契约 §1）。
   *
   * `autoElevateAttemptedRef` 保证**每次进入本页**只自动发起一次（用 ref 而非 state：
   * 1s 轮询会持续重渲染；页面重挂载则会重新尝试，便于侧车稍后就绪时自动补上）。
   */
  const autoElevateAttemptedRef = useRef(false);

  useEffect(() => {
    if (autoElevateAttemptedRef.current) return;
    autoElevateAttemptedRef.current = true;
    let cancelled = false;
    const run = async () => {
      const outcome = await autoElevate();
      if (cancelled) return;
      if (outcome === "infeasible") {
        // 自动路径确实不可行 → 申请一次用户授权（这是唯一的 UAC 触发点）。
        const result = await requestElevation();
        if (!cancelled && result.outcome === "error") {
          logger.warn("Dashboard", "自动申请用户授权失败:", result.reason);
        }
      }
    };
    void run();
    return () => {
      cancelled = true;
    };
  }, []);

  // 预热兜底定时器：到点后不再把缺失信号解释为"还在预热"
  useEffect(() => {
    const t = window.setTimeout(
      () => setWarmupDeadlinePassed(true),
      PIPELINE_WARMUP_MAX_MS,
    );
    return () => window.clearTimeout(t);
  }, []);

  // 引导兜底定时器：到点无条件结束加载（无 GPU / 采集异常环境也不会永久骨架）
  useEffect(() => {
    if (!bootstrapping) return;
    const t = window.setTimeout(
      () => setBootstrapping(false),
      BOOTSTRAP_GRACE_MS,
    );
    return () => window.clearTimeout(t);
  }, [bootstrapping]);

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
          logger.warn(
            "Dashboard",
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
  //
  // 采样节奏为「上一轮真正 settle 后才排下一次」（settle-then-schedule），
  // 而不是固定 1s 的 setInterval——原因见 ADR-0001 D7：
  // `withTimeout` 只让定时器 reject，**无法取消底层 Tauri invoke / spawn_blocking**。
  // 原实现在 finally 里立即把 inFlight 置回 false，于是超时后 1s 就会再发一次新请求，
  // 而旧请求仍在挂起：并发请求数随时间线性累积（约每 8s 加一），注释声称的
  // 「避免每秒堆积 pending invoke」并未实现。改成「等本轮真实结束再排下一轮」后，
  // 挂起期间不再产生新请求，并发度恒为 1。
  useEffect(() => {
    let mounted = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const fetchData = async () => {
      try {
        // 超时保护：驱动无响应时 8 秒内给出明确错误，避免 UI 永久 loading
        const result = await withTimeout(
          invoke<SensorData>("get_sensor_data"),
          LOAD_TIMEOUT_MS,
          `传感器数据获取超时（超过 ${LOAD_TIMEOUT_MS / 1000} 秒无响应）。传感器采集可能被安全软件拦截或驱动挂起，请检查驱动服务状态。`,
        );
        if (!mounted) return;
        setData(result);
        setError(null);
        // 首个含实质数据的快照到达 → 结束首屏引导（不等待授权：tier-1 的
        // GPU/时钟足够形成完整页面，授权卡在加载完成后再判定）
        if (hasSubstantialData(result)) {
          setBootstrapping(false);
        }
        const now = new Date();
        const time = `${now.getHours().toString().padStart(2, "0")}:${now.getMinutes().toString().padStart(2, "0")}:${now.getSeconds().toString().padStart(2, "0")}`;
        const maxGpu =
          result.gpu.length > 0
            ? Math.max(...result.gpu.map((g) => g.usage))
            : 0;
        // 首次采样是 100% 伪值（见 SensorData.cpu.usage_ready）：不写入趋势历史，
        // 否则曲线首点会被伪值顶到 Y 轴顶端（domain [0,100]）造成视觉跳变。
        // 旧版后端缺该字段时视为已就绪，保持原行为。
        if (result.cpu.usage_ready !== false) {
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
        }
      } catch (e: unknown) {
        if (mounted)
          setError(
            `传感器数据获取失败: ${e instanceof Error ? e.message : String(e)}`,
          );
      } finally {
        // 本轮**真实**结束后（成功、失败或超时）才排下一轮。
        // 超时分支里底层 promise 仍未 settle，但那正是本设计要覆盖的情形：
        // 只要不在这里立刻发新请求，就不会有并发累积。
        if (mounted) {
          timer = setTimeout(fetchData, 1000);
        }
      }
    };
    void fetchData();
    return () => {
      mounted = false;
      if (timer !== undefined) clearTimeout(timer);
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

  // 首屏引导加载：只渲染骨架屏，**不渲染**卡片容器、授权卡与降级卡。
  //
  // 两个触发条件合并处理：
  //   - `!data`：首个快照尚未返回（**正常路径下唯一实际生效的条件**）；
  //   - `bootstrapping`：快照已返回但无任何实质信号。`hasSubstantialData`
  //     纳入 tier-1 首帧信号（内存/CPU）后此条件在 Windows 上不可达，
  //     保留作纯保险（见 BOOTSTRAP_GRACE_MS 注释）。
  //
  // 预热期不再靠骨架屏"挡住"提示卡：改由 `isPipelineWarmingUp` 显式门控
  // 授权/降级/频率诊断卡（否则预热期会误报"CPU 频率缺失"等错误结论）。
  if (!data || bootstrapping) {
    return (
      <div className="flex flex-col gap-6 animate-fade-in">
        <PageHeader
          title="硬件信息"
          description="正在加载硬件信息，首次采集需要预热性能计数器…"
        />
        {/* 加载过渡：与内容态同构的骨架网格（摘要 3 卡 + 下方 3 卡），
            尺寸/间距对齐真实布局，切换时不产生跳动 */}
        <div>
          <Skeleton className="mb-3 h-5 w-24 rounded-md" />
          <Skeleton className="h-[248px] w-full rounded-xl" />
        </div>
        <div className="grid grid-cols-1 @2xl/main:grid-cols-2 @5xl/main:grid-cols-3 gap-4">
          {[1, 2, 3].map((i) => (
            <Skeleton key={`detail-${i}`} className="h-[320px] rounded-xl" />
          ))}
        </div>
      </div>
    );
  }

  // 降级判定：CPU 温度/功耗缺失 → 两层原因分流
  //   1) tier-2 未授权（最常见）→ 权限提示卡（可一键触发一次 UAC）
  //   2) tier-2 已授权仍缺 → 驱动引导卡（杀软拦截 PawnIO）
  const cpuTempMissing = data.cpu.temperature <= 0;
  const cpuPowerMissing = data.cpu.power_w <= 0;
  const showDriverGuidance = cpuTempMissing || cpuPowerMissing;
  const missingItems = [
    cpuTempMissing ? "CPU 温度" : null,
    cpuPowerMissing ? "CPU 功耗" : null,
  ]
    .filter((x): x is string => x !== null)
    .join("、");
  // tier-2 状态优先取后端显式字段；旧版后端缺失时退化为按 temp_source 推断
  const tier2Available = data.tier2_available ?? data.cpu.temp_source === "lhm";
  const showPrivilegeNotice = !tier2Available && missingItems.length > 0;
  /**
   * CPU 占用率是否可用于展示。
   *
   * `usage_ready === false` = 后端明确告知"这是进程内首次采样"——sysinfo 首次
   * `refresh_cpu()` 无差分基准，必然返回 100% 伪值（实测 85.2% 的生产会话首采样
   * 为 100%）。此时主数值与进度条留空（并显示"正在采样…"），下一次 1s 轮询即得真值。
   *
   * `undefined`（旧版后端无该字段）按可用处理，退化为修复前的行为。
   */
  const cpuSampleReady = data.cpu.usage_ready !== false;

  return (
    <div className="flex flex-col gap-6 animate-fade-in">
      {/* ── tier-2 权限提示卡（未授权 → 一次 UAC 解锁温度/功耗/主板） ──
          仅在 sidecar 首次出数后判定：预热期 GPU/时钟/温度必为空，此时渲染
          "需要管理员授权"会给出错误结论。`isPipelineWarmingUp` 代替了原先
          "骨架屏恰好挡住"的隐式保护——现在骨架屏结束得更早（tier-1 首帧信号），
          必须显式门控。 */}
      {!isPipelineWarmingUp(data, warmupDeadlinePassed) &&
        showPrivilegeNotice && (
          <Tier2PrivilegeCard reason={data.tier2_reason ?? null} />
        )}

      {/* ── 驱动降级引导卡（已授权仍缺 → 杀软拦截 PawnIO） ── */}
      {!isPipelineWarmingUp(data, warmupDeadlinePassed) &&
        showDriverGuidance &&
        !showPrivilegeNotice && (
          <DriverGuidanceCard missingItems={missingItems} />
        )}

      {/* ── 频率全链路失败：一次性诊断摘要（可复制上报，FREQ=0 场景） ──
          同样需等预热结束：PDH 首次采样前 `clock_mhz` 必然为 0，
          否则会误报"CPU 频率数据缺失"。 */}
      {!isPipelineWarmingUp(data, warmupDeadlinePassed) &&
        ((data.cpu.freq_source ?? "none") === "none" ||
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
              : isPipelineWarmingUp(data, warmupDeadlinePassed)
                ? "正在采集…"
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

      {/* ── 实时摘要（CPU / GPU / 内存 整合为单卡；卡内自编排入场动画） ── */}
      <OverviewCard
        className="animate-scale-in stagger-1"
        data={data}
        cpuSampleReady={cpuSampleReady}
        warmingUp={isPipelineWarmingUp(data, warmupDeadlinePassed)}
      />

      {/* ── 主板传感器卡（LHM SuperIO；无数据不渲染） ── */}
      {data.motherboard && data.motherboard.sensors.length > 0 && (
        <MotherboardCard mb={data.motherboard} />
      )}

      {/* ── Trend Chart + 存储 + 网络流量（并排） ── */}
      {/* 实测（默认 1400px 窗口，右栏展开）：主内容容器内容宽仅 738px。3 列时「网络流量」卡头
          需 ≥300px 卡宽才能单行显示标题，<300px 即逐字竖排（三栏布局引入的回归）。
          实测阈值：3 列需内容宽 ≥1024px（卡宽 331px→1 行）；2 列需 ≥620px（卡宽 302px，
          600px 时仍 2 行）。故 2 列取 @2xl(42rem=672px，留余量)，3 列取 @5xl(64rem=1024px)；
          默认窗口 738px 落在 2 列区间（卡宽 361px，标题单行），不再竖排。 */}
      <div className="grid grid-cols-1 @2xl/main:grid-cols-2 @5xl/main:grid-cols-3 gap-4">
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
