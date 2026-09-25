/**
 * iperf3 前端契约类型 — 与 Rust 侧 `src-tauri/src/iperf.rs` 逐字段对齐。
 *
 * 字段名保持 snake_case（与后端 `#[derive(Serialize)]` 一致），
 * 参数名保持 camelCase（与后端 `#[serde(rename_all = "camelCase")]` 一致）。
 * 两边改名必须同步，否则 invoke 会静默丢参或读取到 undefined。
 */

// ============================================================================
// 参数（invoke 入参）
// ============================================================================

/** 测试结束条件（`-t` / `-n` / `-k` 三选一，后端用枚举强制互斥）。 */
export type Iperf3EndMode = "time" | "bytes" | "blocks";

/** 传输协议。 */
export type Iperf3Protocol = "tcp" | "udp";

/** IP 网络栈（`-4` / `-6` / 自动）。 */
export type Iperf3IpFamily = "auto" | "v4" | "v6";

/** iperf3 客户端全部测试参数（可选字段留空即不传该命令行选项）。 */
export interface Iperf3Options {
  /** `-p` 服务端口 */
  port: number;
  /** `-u` 改用 UDP */
  protocol: Iperf3Protocol;
  /** `-R` 反向（服务端发，客户端收） */
  reverse: boolean;
  /** `--bidir` 双向同时收发 */
  bidir: boolean;
  /** 结束条件选择 */
  endMode: Iperf3EndMode;
  /** `-t` 时长（秒） */
  time: number;
  /** `-n` 字节数（如 `100M`） */
  bytes: string;
  /** `-k` 块数（如 `1000`） */
  blockcount: string;
  /** `-l` 缓冲长度（如 `128K`） */
  length: string;
  /** `-P` 并行流数量 */
  parallel: number;
  /** `-O` 预热丢弃秒数 */
  omit: number;
  /** `-b` 目标码率（如 `100M` 或 `10M/1000`） */
  bitrate: string;
  /** `-i` 报告间隔秒数（0 = 关闭周期报告） */
  interval: number;
  /** `-f` 报告单位 */
  format: Iperf3Format;
  /** `-w` socket 缓冲区/窗口大小 */
  window: string;
  /** `-M` 最大段大小 */
  mss: string;
  /** `-N` 禁用 Nagle 算法 */
  noDelay: boolean;
  /** `-4` / `-6` */
  ipFamily: Iperf3IpFamily;
  /** `-S` TOS 字节（0-255，可 0x34 / 064 写法） */
  tos: string;
  /** `--dscp` DSCP 值（0-63 或 CS1/AF11/EF 等符号名） */
  dscp: string;
  /** `-Z` 零拷贝发送 */
  zerocopy: boolean;
  /** `-V` 详细输出 */
  verbose: boolean;
  /** `-T` 输出标题前缀 */
  title: string;
  /** `--get-server-output` 取回服务端结果 */
  getServerOutput: boolean;
  /** `--connect-timeout` 连接超时（毫秒） */
  connectTimeoutMs: string;
  /** `-A` CPU 亲和性（n 或 n,m） */
  affinity: string;
  /** `-B` 绑定本地地址 */
  bind: string;
  /** `--cport` 客户端源端口 */
  cport: string;
  /** `--repeating-payload` 重复载荷 */
  repeatingPayload: boolean;
  /** `--dont-fragment` IPv4 DF 位（仅 UDP） */
  dontFragment: boolean;
  /** `--udp-counters-64bit` UDP 64 位计数器 */
  udpCounters64bit: boolean;
  /** `--rcv-timeout` 接收空闲超时（毫秒） */
  rcvTimeoutMs: string;
  /** `--snd-timeout` 未确认数据超时（毫秒） */
  sndTimeoutMs: string;
  /** `--extra-data` 附加到 JSON 的字符串 */
  extraData: string;
  /** `-F` 文件数据源/数据槽 */
  file: string;
}

/** `-f` 报告单位。 */
export type Iperf3Format = "k" | "m" | "g" | "t";

/** 默认参数 — 与 iperf3 官方默认值一致，打开即可直接跑一次 TCP 正向测试。 */
export const IPERF3_DEFAULTS: Iperf3Options = {
  port: 5201,
  protocol: "tcp",
  reverse: false,
  bidir: false,
  endMode: "time",
  time: 10,
  bytes: "100M",
  blockcount: "1000",
  length: "",
  parallel: 1,
  omit: 0,
  bitrate: "",
  interval: 1,
  format: "m",
  window: "",
  mss: "",
  noDelay: false,
  ipFamily: "auto",
  tos: "",
  dscp: "",
  zerocopy: false,
  verbose: false,
  title: "",
  getServerOutput: false,
  connectTimeoutMs: "",
  affinity: "",
  bind: "",
  cport: "",
  repeatingPayload: false,
  dontFragment: false,
  udpCounters64bit: false,
  rcvTimeoutMs: "",
  sndTimeoutMs: "",
  extraData: "",
  file: "",
};

// ============================================================================
// 结果（invoke 返回值）
// ============================================================================

/** 单个方向的收发汇总。缺失值一律为 null（后端「不伪造」原则）。 */
export interface Iperf3DirectionSummary {
  mbps: number | null;
  bytes: number | null;
  seconds: number | null;
  retransmits: number | null;
  jitter_ms: number | null;
  lost_percent: number | null;
  lost_packets: number | null;
  packets: number | null;
}

/** 单个测量区间行（配合实时输出逐行展示）。 */
export interface Iperf3IntervalRow {
  start_s: number;
  end_s: number;
  /** 正向 / 双向-反向 */
  direction: string;
  /** 该区间的并行流数量 */
  streams: number;
  mbps: number | null;
  bytes: number | null;
  retransmits: number | null;
  jitter_ms: number | null;
  lost_percent: number | null;
  /** `-O` 预热段（统计被 iperf3 丢弃） */
  omitted: boolean;
}

/** CPU 占用（iperf3 提供，可能缺失）。 */
export interface Iperf3Cpu {
  host_total: number | null;
  remote_total: number | null;
}

/** iperf3 客户端测试结果。 */
export interface Iperf3Result {
  target: string;
  port: number;
  /** TCP / UDP */
  protocol: string;
  /** 正向 / 反向 / 双向 */
  direction: string;
  protocol_key: string;
  parallel: number;
  end_condition: string;
  /** iperf3 自报版本，如 `iperf 3.21` */
  tool_version: string;
  json_stream: boolean;
  sent: Iperf3DirectionSummary;
  received: Iperf3DirectionSummary;
  bidir_reverse_sent: Iperf3DirectionSummary | null;
  bidir_reverse_received: Iperf3DirectionSummary | null;
  cpu: Iperf3Cpu | null;
  intervals: Iperf3IntervalRow[];
  /** 用户取消（取消不是失败） */
  cancelled: boolean;
  /** 错误/告警原文 */
  messages: string[];
}

/** 随包/系统 iperf3 的可用性与版本能力（用于门控与缺失提示）。 */
export interface Iperf3ToolInfo {
  available: boolean;
  exe_path: string | null;
  version_raw: string;
  version: string;
  major: number;
  minor: number;
  patch: number;
  supports_json_stream: boolean;
  supports_bidir: boolean;
  supports_dscp: boolean;
  note: string;
}

/** 本机 iperf3 服务端状态。 */
export interface Iperf3ServerStatus {
  running: boolean;
  port: number | null;
  pid: number | null;
  started_at: string | null;
  uptime_s: number | null;
  one_off: boolean;
  /** 服务端最近输出行（文本） */
  lines: string[];
  error: string | null;
}

// ============================================================================
// 工具函数
// ============================================================================

/** 空串或纯空白 → undefined（不把空输入传给后端当作有效参数）。 */
export function optStr(v: string): string | undefined {
  const t = v.trim();
  return t === "" ? undefined : t;
}

/** 空串或非正数 → undefined。 */
export function optNum(v: string): number | undefined {
  const t = v.trim();
  if (t === "") return undefined;
  const n = Number(t);
  return Number.isFinite(n) && n > 0 ? n : undefined;
}

/**
 * 把 UI 参数转为后端 `Iperf3ClientOptions`（camelCase，与 serde 对齐）。
 *
 * `host` 由调用方注入（来自页面共享的「目标地址」输入框）。
 */
export function toInvokeOptions(
  host: string,
  o: Iperf3Options,
): Record<string, unknown> {
  const args: Record<string, unknown> = {
    host: host.trim(),
    port: o.port,
    protocol: o.protocol,
    reverse: o.reverse,
    bidir: o.bidir,
    endMode: o.endMode,
    time: o.time,
    parallel: o.parallel,
    omit: o.omit,
    interval: o.interval,
    format: o.format,
    noDelay: o.noDelay,
    ipFamily: o.ipFamily,
    zerocopy: o.zerocopy,
    verbose: o.verbose,
    getServerOutput: o.getServerOutput,
    repeatingPayload: o.repeatingPayload,
    dontFragment: o.dontFragment,
    udpCounters64bit: o.udpCounters64bit,
  };
  // 可选字段：留空即不发，后端 serde default 会回落
  const optional: Array<[string, unknown]> = [
    ["bytes", o.endMode === "bytes" ? optStr(o.bytes) : undefined],
    ["blockcount", o.endMode === "blocks" ? optStr(o.blockcount) : undefined],
    ["length", optStr(o.length)],
    ["bitrate", optStr(o.bitrate)],
    ["window", optStr(o.window)],
    ["mss", optNum(o.mss)],
    ["tos", optStr(o.tos)],
    ["dscp", optStr(o.dscp)],
    ["title", optStr(o.title)],
    ["connectTimeoutMs", optNum(o.connectTimeoutMs)],
    ["affinity", optStr(o.affinity)],
    ["bind", optStr(o.bind)],
    ["cport", optNum(o.cport)],
    ["rcvTimeoutMs", optNum(o.rcvTimeoutMs)],
    ["sndTimeoutMs", optNum(o.sndTimeoutMs)],
    ["extraData", optStr(o.extraData)],
    ["file", optStr(o.file)],
  ];
  for (const [k, v] of optional) {
    if (v !== undefined) args[k] = v;
  }
  return args;
}

/** 单行参数快照（右侧栏日志 START 行与结果行共用）。 */
export function describeIperf3(host: string, o: Iperf3Options): string {
  const dir = o.bidir ? "双向" : o.reverse ? "反向" : "正向";
  const proto = o.protocol.toUpperCase();
  const end =
    o.endMode === "time"
      ? `${o.time}秒`
      : o.endMode === "bytes"
        ? `${o.bytes}字节`
        : `${o.blockcount}块`;
  const parts = [`${host.trim()}:${o.port}`, proto, dir, `条件=${end}`];
  if (o.parallel > 1) parts.push(`并行=${o.parallel}`);
  if (o.omit > 0) parts.push(`预热丢弃=${o.omit}s`);
  if (opts(o.bitrate)) parts.push(`码率=${o.bitrate.trim()}`);
  if (o.interval !== 1) parts.push(`间隔=${o.interval}s`);
  if (opts(o.length)) parts.push(`缓冲=${o.length.trim()}`);
  if (opts(o.window)) parts.push(`窗口=${o.window.trim()}`);
  if (opts(o.mss)) parts.push(`MSS=${o.mss.trim()}`);
  if (o.noDelay) parts.push("禁用Nagle");
  if (o.ipFamily !== "auto")
    parts.push(o.ipFamily === "v4" ? "强制IPv4" : "强制IPv6");
  if (opts(o.tos)) parts.push(`TOS=${o.tos.trim()}`);
  if (opts(o.dscp)) parts.push(`DSCP=${o.dscp.trim()}`);
  if (o.zerocopy) parts.push("零拷贝");
  if (o.verbose) parts.push("详细");
  if (opts(o.title)) parts.push(`标题=${o.title.trim()}`);
  if (o.getServerOutput) parts.push("取服务端结果");
  if (opts(o.connectTimeoutMs)) parts.push(`连接超时=${o.connectTimeoutMs}ms`);
  if (opts(o.affinity)) parts.push(`亲和=${o.affinity.trim()}`);
  if (opts(o.bind)) parts.push(`绑定=${o.bind.trim()}`);
  if (opts(o.cport)) parts.push(`源端口=${o.cport}`);
  if (o.repeatingPayload) parts.push("重复载荷");
  if (o.dontFragment) parts.push("DF位");
  if (o.udpCounters64bit) parts.push("64位计数");
  if (opts(o.rcvTimeoutMs)) parts.push(`收超时=${o.rcvTimeoutMs}ms`);
  if (opts(o.sndTimeoutMs)) parts.push(`发超时=${o.sndTimeoutMs}ms`);
  if (opts(o.extraData)) parts.push(`附加=${o.extraData.trim()}`);
  if (opts(o.file)) parts.push(`文件=${o.file.trim()}`);
  return parts.join(" ");
}

function opts(v: string): boolean {
  return v.trim() !== "";
}

/** 单行结果摘要（右侧栏日志终态行）。 */
export function summarizeIperf3Result(r: Iperf3Result): string {
  const fmt = (
    label: string,
    d: Iperf3DirectionSummary | null | undefined,
  ): string => {
    if (!d || d.mbps === null) return "";
    const extra =
      d.lost_percent !== null
        ? ` 丢包${d.lost_percent.toFixed(2)}%`
        : d.jitter_ms !== null
          ? ` 抖动${d.jitter_ms.toFixed(2)}ms`
          : d.retransmits !== null
            ? ` 重传${d.retransmits}`
            : "";
    return ` | ${label}${d.mbps.toFixed(1)}Mbps${extra}`;
  };
  const body = [
    fmt("发", r.sent),
    fmt("收", r.received),
    fmt("反向发", r.bidir_reverse_sent),
    fmt("反向收", r.bidir_reverse_received),
  ]
    .filter(Boolean)
    .join("");
  const cancelled = r.cancelled ? "（用户取消）" : "";
  return `${r.target}:${r.port} ${r.protocol} ${r.direction} 区间${r.intervals.length}条${body}${cancelled}`;
}

/** 吞吐/字节/抖动等数值的展示格式（缺失显示 `—`，绝不显示 0 冒充）。 */
export function fmtMbps(v: number | null): string {
  return v === null ? "—" : `${v.toFixed(1)} Mbps`;
}

/** 直接换算为更易读的单位（>=1000 Mbps 显示 Gbps）。 */
export function fmtMbpsAdaptive(v: number | null): string {
  if (v === null) return "—";
  return v >= 1000 ? `${(v / 1000).toFixed(2)} Gbps` : `${v.toFixed(1)} Mbps`;
}

export function fmtBytes(v: number | null): string {
  if (v === null) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let n = v;
  let i = 0;
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024;
    i += 1;
  }
  return `${n.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

export function fmtNum(v: number | null, digits = 2): string {
  return v === null ? "—" : v.toFixed(digits);
}

/** 秒数 → 友好时长（服务端运行时长展示）。 */
export function fmtDuration(sec: number | null): string {
  if (sec === null) return "—";
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  const s = sec % 60;
  if (h > 0) return `${h}小时${m}分${s}秒`;
  if (m > 0) return `${m}分${s}秒`;
  return `${s}秒`;
}
