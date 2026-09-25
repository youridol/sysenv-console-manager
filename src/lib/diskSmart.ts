import { formatBytes } from "./utils";

/* ── 磁盘 S.M.A.R.T 前端类型（与后端 disk_info.rs / datasource disk.rs 契约一致） ── */

/** 健康状态（对应后端 HealthStatus.as_str：Good/Warning/Critical/Unknown） */
export type DiskHealthStatus = "Good" | "Warning" | "Critical" | "Unknown";

/** 健康度摘要（卡片收起状态展示；GET /api/disks ↔ list_disks） */
export interface DiskHealthSummary {
  /** 健康状态字符串：Good / Warning / Critical / Unknown */
  status: DiskHealthStatus;
  /** 健康状态中文标签：健康 / 预警 / 严重 / 无法读取 */
  label: string;
  /** 是否告警（Critical 或 Warning 且有关键告警原因） */
  alert: boolean;
  /** 告警原因列表（空 = 无告警） */
  alerts: string[];
  /** 温度（摄氏度，null = 无数据） */
  temperature_c: number | null;
  /** 通电时间（小时） */
  power_on_hours: number | null;
  /** 重映射扇区数（ATA）/ 可用备用百分比换算（NVMe） */
  reallocated_sectors: number | null;
  /** 待映射扇区数（ATA） */
  pending_sectors: number | null;
  /** 磨损百分比（NVMe percentage_used） */
  wear_level_percent: number | null;
}

/** 磁盘列表项（GET /api/disks 返回体元素） */
export interface DiskSummary {
  /** 磁盘 id（\\\\.\\PhysicalDriveN 的 N） */
  id: string;
  /** 型号 */
  model: string;
  /** 序列号 */
  serial: string;
  /** 接口类型：NVMe / SATA / ATA / USB / SCSI / 其他 */
  interface_type: string;
  /** 介质类型：HDD / SSD / 未知 */
  media_type: string;
  /** 容量（字节） */
  size_bytes: number;
  /** 健康度摘要 */
  health: DiskHealthSummary;
}

/** S.M.A.R.T 属性条目（详情弹窗逐行渲染） */
export interface SmartAttribute {
  /** 属性 id（十进制） */
  id: number;
  /** 属性名（中文，如 "重映射扇区数"） */
  name: string;
  /** 原始值（raw，16 字节大端解析） */
  raw: number;
  /** 归一化当前值（0-255，越大越好） */
  value: number;
  /** 归一化最差值 */
  worst: number;
  /** 阈值（0 表示无阈值） */
  threshold: number;
  /**
   * 状态。
   *
   * 生产者 `datasource/src/disk.rs` 只产出两个字面量：
   * `"OK"` 与 `"FAILED"`（阈值存在且 `value <= threshold` 时判 FAILED）。
   * 此处曾声明 `OK / FAILING / FAILED / UNKNOWN` 四态，其中 `FAILING` 与 `UNKNOWN`
   * 的实现分支永不可达（ADR-0001 D11：契约取值域必须等于实现能产出的取值域）。
   * 保留 `string` 类型是为了向前兼容未来的中间态；但不要把未产出的取值写进契约。
   */
  status: string;
}

/** NVMe 健康日志页（NVME_LOG_PAGE_HEALTH_INFO 精选字段） */
export interface NvmeHealthLog {
  /** 临界警告位掩码（bit0 温度过高 / bit1 备用空间不足 / bit2 可靠性降级 / bit3 介质只读 / bit4 易失备份失败） */
  critical_warning: number;
  /** 温度（摄氏度） */
  temperature_c: number;
  /** 可用备用百分比 */
  available_spare: number;
  /** 备用阈值百分比 */
  available_spare_threshold: number;
  /** 已用寿命百分比（磨损） */
  percentage_used: number;
  /** 通电时间（小时） */
  power_on_hours: number;
  /** 通电周期 */
  power_cycles: number;
  /** 不安全关机次数 */
  unsafe_shutdowns: number;
  /** 媒体错误数 */
  media_errors: number;
  /** 数据单元读取（512B 单元） */
  data_units_read: number;
  /** 数据单元写入（512B 单元） */
  data_units_written: number;
}

/** 磁盘 S.M.A.R.T 详情（GET /api/disks/:id/smart ↔ get_disk_smart） */
export interface DiskSmartDetail {
  /** 磁盘 id */
  id: string;
  /** 型号 */
  model: string;
  /** 接口类型 */
  interface_type: string;
  /** 介质类型 */
  media_type: string;
  /** 健康度摘要（与列表一致，弹窗顶部展示） */
  health: DiskHealthSummary;
  /** 完整 S.M.A.R.T 属性表（ATA；NVMe 为空数组） */
  attributes: SmartAttribute[];
  /** NVMe 健康日志精选字段（仅 NVMe） */
  nvme_health: NvmeHealthLog | null;
  /** 读取失败时的可读错误（null = 正常；非 null = 权限不足/不支持等，前端展示但页面不崩） */
  error: string | null;
}

/* ── 健康度 UI 元数据（语义色；类名写全静态字面量保证 Tailwind 构建期扫描） ── */

export interface HealthMeta {
  /** 徽标类（Badge 变体覆盖） */
  badgeClass: string;
  /** 文本颜色类 */
  textClass: string;
  /** 图标容器背景类 */
  bgClass: string;
  /** 卡片边框强调类（异常时醒目） */
  borderClass: string;
  /** 顶部色条类 */
  topBarClass: string;
}

/** 健康状态 → UI 语义色映射（Good 绿 / Warning 黄 / Critical 红 / Unknown 灰） */
export const HEALTH_META: Record<DiskHealthStatus, HealthMeta> = {
  Good: {
    badgeClass: "bg-green-500/15 text-green-400 border-green-500/40",
    textClass: "text-green-400",
    bgClass: "bg-green-500/10",
    borderClass: "border-green-500/40",
    topBarClass: "bg-green-500",
  },
  Warning: {
    badgeClass: "bg-yellow-500/15 text-yellow-400 border-yellow-500/40",
    textClass: "text-yellow-400",
    bgClass: "bg-yellow-500/10",
    borderClass: "border-yellow-500/50",
    topBarClass: "bg-yellow-500",
  },
  Critical: {
    badgeClass: "bg-red-500/15 text-red-500 border-red-500/40",
    textClass: "text-red-500",
    bgClass: "bg-red-500/10",
    borderClass: "border-red-500/60",
    topBarClass: "bg-red-500",
  },
  Unknown: {
    badgeClass: "bg-muted text-muted-foreground border-border",
    textClass: "text-muted-foreground",
    bgClass: "bg-muted/40",
    borderClass: "border-border",
    topBarClass: "bg-muted-foreground/40",
  },
};

/* ── 格式化辅助 ── */

/** 通电时间（小时）→ 人读文案：≥24 小时显示天，否则显示小时 */
/** 通电时间（小时；与 CrystalDiskInfo 一致，直接显示小时数，不换算天） */
export function formatPowerOnHours(hours: number | null): string {
  if (hours === null) return "—";
  return `${hours} 小时`;
}

/** 原始值列（对齐 CrystalDiskInfo：16 进制 12 位补零，如 60897 → 00000000EDE1）
 *  raw 为 6 字节小端（最大 2^48-1 < JS 安全整数，精度无损） */
export function formatRawValue(raw: number): string {
  return raw.toString(16).toUpperCase().padStart(12, "0");
}

/** NVMe 数据单元（512B）→ 字节格式化（u128 经 JSON 转 number，超出安全整数时展示近似值） */
export function formatDataUnits(units: number | null): string {
  if (units === null) return "—";
  return formatBytes(units * 512);
}

/** S.M.A.R.T 属性状态 → 语义色（FAILED 红 / OK 绿 / 其余灰）。
 *
 * 只有两态可达（见 `SmartAttribute.status` 注释）：曾有的 `"FAILING"` 黄色分支
 * 永不可达，已删除（ADR-0001 D11）。`default` 保留给未知/缺失输入。 */
export function attributeStatusClass(status: string): string {
  switch (status) {
    case "FAILED":
      return "text-red-500 font-semibold";
    case "OK":
      return "text-green-400";
    default:
      return "text-muted-foreground";
  }
}

/** 属性状态中文标签（表格列展示用）。两态可达，见 `SmartAttribute.status`。 */
export function attributeStatusLabel(status: string): string {
  switch (status) {
    case "FAILED":
      return "失败";
    case "OK":
      return "正常";
    default:
      return status || "未知";
  }
}

/** NVMe 临界警告位 → 中文说明（bit0-4；0 = 无警告） */
export function criticalWarningText(mask: number): string {
  if (mask === 0) return "无";
  const bits: string[] = [];
  if (mask & 0x01) bits.push("温度过高");
  if (mask & 0x02) bits.push("备用空间不足");
  if (mask & 0x04) bits.push("可靠性降级");
  if (mask & 0x08) bits.push("介质只读");
  if (mask & 0x10) bits.push("易失备份失败");
  return bits.join("、") || `0x${mask.toString(16)}`;
}
