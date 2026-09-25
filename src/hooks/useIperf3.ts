import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useStreaming, type StreamEvent } from "./useStreaming";
import { logger } from "../lib/logger";
import {
  IPERF3_DEFAULTS,
  describeIperf3,
  summarizeIperf3Result,
  toInvokeOptions,
  type Iperf3IntervalRow,
  type Iperf3Options,
  type Iperf3Result,
  type Iperf3ServerStatus,
  type Iperf3ToolInfo,
} from "../lib/iperf3";
import { oneLine } from "../lib/util-line";

/** 与 Network.tsx 的 ToolController 契约一致（统一开始/停止按钮复用）。 */
export interface Iperf3Controller {
  loading: boolean;
  lines: string[];
  error: string | null;
  run: () => void;
  stop: () => void;
  toggle: () => void;
  clear: () => void;
}

/** 生成命令 id（取消注册表按此定位；仅需进程内唯一）。 */
function generateId(): string {
  return `iperf-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

/**
 * 把区间事件渲染成一行等宽文本（供 DiagOutput 逐行展示）。
 *
 * 展示口径：`[起-止] 方向 吞吐 [重传/抖动/丢包]`。
 * 数值缺失显示 `—`，绝不填 0（与后端「不伪造」原则一致）。
 */
export function formatIntervalLine(raw: string): string | null {
  let row: Iperf3IntervalRow;
  try {
    row = JSON.parse(raw) as Iperf3IntervalRow;
  } catch {
    return raw;
  }
  const range = `[${row.start_s.toFixed(2)}-${row.end_s.toFixed(2)}]s`;
  const dir = row.direction === "双向-反向" ? "反向" : "正向";
  const mbps =
    row.mbps === null
      ? "—"
      : row.mbps >= 1000
        ? `${(row.mbps / 1000).toFixed(2)} Gbps`
        : `${row.mbps.toFixed(1)} Mbps`;
  const extra: string[] = [];
  if (row.streams > 1) extra.push(`${row.streams} 流`);
  if (row.retransmits !== null) extra.push(`重传 ${row.retransmits}`);
  if (row.jitter_ms !== null) extra.push(`抖动 ${row.jitter_ms.toFixed(2)}ms`);
  if (row.lost_percent !== null)
    extra.push(`丢包 ${row.lost_percent.toFixed(2)}%`);
  const omitted = row.omitted ? "（预热丢弃）" : "";
  return `${range} ${dir.padEnd(4)} ${mbps.padStart(12)}${omitted}${
    extra.length ? `  ${extra.join("  ")}` : ""
  }`;
}

/** 流式消息格式化（区间 → 表格行；其余原样透传）。 */
function formatIperfMessage(ev: StreamEvent): string | null {
  if (ev.kind === "iperf-interval") return formatIntervalLine(ev.data);
  return ev.data;
}

/**
 * iperf3 客户端控制器。
 *
 * 日志落点与既有 5 个流式工具**完全一致**（每次运行恰好 2 条）：
 *   1. START（`logger.action` INFO）；
 *   2. 终态（完成 INFO / 失败 ERROR / 取消 WARN）。
 * 逐区间明细走页面输出区，不上报日志（否则长测试会灌满 200 条环形缓冲）。
 */
export function useIperf3Client(opts: {
  /** 页面上共享的「目标地址」 */
  getTarget: () => string;
  /** 页面上共享的「网络栈」，作为 ipFamily 的初始联动值 */
  getIpFamily: () => "auto" | "v4" | "v6";
}): {
  controller: Iperf3Controller;
  options: Iperf3Options;
  patch: (p: Partial<Iperf3Options>) => void;
  /** 最近一次成功运行的结构化结果（失败/取消时为 null） */
  lastResult: Iperf3Result | null;
} {
  const { lines, error, start, reset } = useStreaming(formatIperfMessage);
  const [options, setOptions] = useState<Iperf3Options>(IPERF3_DEFAULTS);
  const [loading, setLoading] = useState(false);
  const [lastResult, setLastResult] = useState<Iperf3Result | null>(null);
  const opIdRef = useRef(0);
  const cmdIdRef = useRef<string | null>(null);
  const descRef = useRef<string | null>(null);

  const patch = useCallback((p: Partial<Iperf3Options>) => {
    setOptions((prev) => ({ ...prev, ...p }));
  }, []);

  // 共享的「网络栈」切换时联动 iperf 的 ipFamily（保持与页面其他工具一致）
  const ipFamily = opts.getIpFamily();
  const ipRef = useRef(ipFamily);
  useEffect(() => {
    if (ipRef.current === ipFamily) return;
    ipRef.current = ipFamily;
    setOptions((prev) => ({ ...prev, ipFamily }));
  }, [ipFamily]);

  const run = () => {
    const host = opts.getTarget().trim();
    if (!host) return;
    const id = generateId();
    cmdIdRef.current = id;
    descRef.current = null;
    const opId = ++opIdRef.current;
    setLoading(true);

    void start(id, async (cmdId, ch) => {
      const args = {
        options: toInvokeOptions(host, options),
        cmdId,
        channel: ch,
      };
      const desc = describeIperf3(host, options);
      descRef.current = desc;
      logger.action("INFO", "Network", `iperf3 开始：${oneLine(desc)}`);
      try {
        const ret = await invoke<Iperf3Result>("iperf3_stream", args);
        // 竞态保护：本次运行已被 stop()/新 run 接管 → 终态由接管方负责
        if (opIdRef.current !== opId) return;
        descRef.current = null;
        setLastResult(ret);
        logger.action(
          "INFO",
          "Network",
          `iperf3 完成：${oneLine(summarizeIperf3Result(ret))}`,
        );
      } catch (e: unknown) {
        if (opIdRef.current === opId) {
          descRef.current = null;
          setLastResult(null);
          logger.action(
            "ERROR",
            "Network",
            `iperf3 失败：${oneLine(e instanceof Error ? e.message : String(e))}`,
          );
        }
        // 必须继续抛出：useStreaming 依赖它把错误显示到页面输出区
        throw e;
      }
    }).finally(() => {
      if (opIdRef.current === opId) setLoading(false);
    });
  };

  const stop = () => {
    const cid = cmdIdRef.current;
    const desc = descRef.current;
    cmdIdRef.current = null;
    descRef.current = null;
    opIdRef.current++;
    if (cid) {
      if (desc !== null) {
        logger.action("WARN", "Network", `iperf3 已取消：${oneLine(desc)}`);
      }
      void invoke("cancel_net_cmd", { cmdId: cid }).catch(() => {
        logger.error("Network", `cancel_net_cmd failed for ${cid}`);
      });
    }
    setLoading(false);
    reset();
  };

  return {
    controller: {
      loading,
      lines,
      error,
      run,
      stop,
      toggle: () => (loading ? stop() : run()),
      clear: () => {
        setLastResult(null);
        reset();
      },
    },
    options,
    patch,
    lastResult,
  };
}

/**
 * iperf3 工具信息（可用性 / 版本 / 能力门控）。
 *
 * 页面打开时查一次即可：二进制随包分发，运行期不会变。
 */
export function useIperf3ToolInfo(): Iperf3ToolInfo | null {
  const [info, setInfo] = useState<Iperf3ToolInfo | null>(null);
  useEffect(() => {
    let alive = true;
    void invoke<Iperf3ToolInfo>("iperf3_tool_info")
      .then((v) => {
        if (alive) setInfo(v);
      })
      .catch(() => {
        // 查询失败不阻塞页面：按"不可用"处理，由 UI 显示指引
        if (alive) {
          setInfo({
            available: false,
            exe_path: null,
            version_raw: "",
            version: "未知",
            major: 0,
            minor: 0,
            patch: 0,
            supports_json_stream: false,
            supports_bidir: false,
            supports_dscp: false,
            note: "无法查询 iperf3 状态（后端命令未响应）。",
          });
        }
      });
    return () => {
      alive = false;
    };
  }, []);
  return info;
}

const IDLE_SERVER: Iperf3ServerStatus = {
  running: false,
  port: null,
  pid: null,
  started_at: null,
  uptime_s: null,
  one_off: false,
  lines: [],
  error: null,
};

/**
 * 本机 iperf3 服务端控制器。
 *
 * `polling` 为 true 时以 1Hz 轮询状态（仅在用户展开服务端卡片时开启，
 * 避免不必要的后台调用；与仓库既有轮询范式一致，不为长驻进程新造事件通道）。
 */
export function useIperf3Server(polling: boolean): {
  status: Iperf3ServerStatus;
  busy: boolean;
  error: string | null;
  startServer: (port: number, oneOff: boolean) => void;
  stopServer: () => void;
} {
  const [status, setStatus] = useState<Iperf3ServerStatus>(IDLE_SERVER);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const oneOffRef = useRef(false);

  const refresh = useCallback(async () => {
    try {
      const s = await invoke<Iperf3ServerStatus>("iperf3_server_status", {
        oneOff: oneOffRef.current,
      });
      setStatus(s);
    } catch {
      // 轮询失败静默（服务端可能刚被停止）；下次轮询自然恢复
    }
  }, []);

  useEffect(() => {
    if (!polling) return;
    void refresh();
    const timer = setInterval(() => void refresh(), 1000);
    return () => clearInterval(timer);
  }, [polling, refresh]);

  const startServer = useCallback((port: number, oneOff: boolean) => {
    setBusy(true);
    setError(null);
    oneOffRef.current = oneOff;
    logger.action(
      "INFO",
      "Network",
      `iperf3 服务端启动：端口=${port} one_off=${oneOff}`,
    );
    void invoke<Iperf3ServerStatus>("iperf3_server_start", { port, oneOff })
      .then((s) => {
        setStatus(s);
        logger.action(
          "INFO",
          "Network",
          `iperf3 服务端已启动：端口=${s.port ?? port} pid=${s.pid ?? "-"}`,
        );
      })
      .catch((e: unknown) => {
        const msg = e instanceof Error ? e.message : String(e);
        setError(msg);
        logger.action(
          "ERROR",
          "Network",
          `iperf3 服务端启动失败：${oneLine(msg)}`,
        );
      })
      .finally(() => setBusy(false));
  }, []);

  const stopServer = useCallback(() => {
    setBusy(true);
    setError(null);
    void invoke<Iperf3ServerStatus>("iperf3_server_stop")
      .then((s) => {
        setStatus(s);
        logger.action("INFO", "Network", "iperf3 服务端已停止");
      })
      .catch((e: unknown) => {
        const msg = e instanceof Error ? e.message : String(e);
        setError(msg);
        logger.action(
          "ERROR",
          "Network",
          `iperf3 服务端停止失败：${oneLine(msg)}`,
        );
      })
      .finally(() => setBusy(false));
  }, []);

  return { status, busy, error, startServer, stopServer };
}
