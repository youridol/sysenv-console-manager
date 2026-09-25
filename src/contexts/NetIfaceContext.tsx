import { logger } from "../lib/logger";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";

// ============================================================================
// 网络流量单数据源共享状态
//
// 解决两个问题：
//   1. Dashboard 与侧边栏各自独立轮询 get_net_stats → 采样时刻不同步
//      （Rust 端速度基于全局快照链，两个调用者取到不同时间段的速度）
//   2. 网卡选择不同步 → Dashboard 手动选择后侧边栏不跟随
//
// 设计：
//   - Provider 内统一轮询 get_net_stats（单数据源），netStats 全局共享
//   - selectedIface 全局共享（默认 "全部" = 总流量）
//   - 用户手动选择后锁定，不再自动切换
//   - 未手动选择时自动跟随"有流量的物理网卡"
//   - 消费方通过 getDisplayNet() 取统一口径的展示数据（以网络卡片为准）
// ============================================================================

// ============================================================================
// 类型
// ============================================================================

export interface NetInterfaceStats {
  name: string;
  download_speed: number;
  upload_speed: number;
  total_download: number;
  total_upload: number;
}

export interface NetStatsData {
  interfaces: NetInterfaceStats[];
  total_download_speed: number;
  total_upload_speed: number;
  total_download: number;
  total_upload: number;
  active_connections: number;
}

/** 网卡展示汇总（统一口径：选中网卡无数据时回退总流量） */
export interface DisplayNet {
  downloadSpeed: number;
  uploadSpeed: number;
  downloadTotal: number;
  uploadTotal: number;
}

interface NetIfaceContextValue {
  /** 当前选中的网卡名（"全部" = 总流量） */
  selectedIface: string;
  /** 手动选择网卡（锁定选择，停止自动切换） */
  setSelectedIface: (name: string) => void;
  /** 共享网络流量数据（Provider 统一轮询） */
  netStats: NetStatsData | null;
  /** 轮询间隔 ms（网络卡片可调） */
  netInterval: number;
  /** 调整轮询间隔 */
  setNetInterval: (ms: number) => void;
  /** 各网卡链路协商速度（如 "1 Gbps"），键为网卡别名；空表表示不可用 */
  linkSpeeds: Record<string, string>;
}

const NetIfaceContext = createContext<NetIfaceContextValue | null>(null);

// ============================================================================
// 网卡选择工具
// ============================================================================

/** 是否为伪网卡（环回/蓝牙/虚拟伪接口） */
function isFakeIface(name: string): boolean {
  const n = name.toLowerCase();
  return (
    n.includes("loopback") ||
    n.includes("bluetooth") ||
    n.includes("pseudo") ||
    n.includes("isatap") ||
    n.includes("teredo")
  );
}

/** 网卡名字优先级（越小越优先） */
function ifacePriority(name: string): number {
  const n = name.toLowerCase();
  // ① 物理以太网
  if (
    (n.includes("ether") ||
      n.includes("以太") ||
      n.includes("realtek") ||
      n.includes("intel") ||
      n.includes("pcie") ||
      n.includes("gbe") ||
      n.includes("local area")) &&
    !n.includes("vethernet")
  ) {
    return 0;
  }
  // ② Hyper-V 外部交换机（映射物理网卡）
  if (n.includes("vethernet") && n.includes("external")) return 1;
  // ③ WLAN
  if (
    n.includes("wi-fi") ||
    n.includes("wifi") ||
    n.includes("wireless") ||
    n.includes("wlan") ||
    n.includes("无线")
  ) {
    return 2;
  }
  // ④ 其他
  return 3;
}

/**
 * 选择最优网卡：
 * 优先"有流量的真实网卡"（名字优先级 > 流量大小），全部空闲时按名字优先级。
 */
function pickBestIface(interfaces: NetInterfaceStats[]): string {
  const real = interfaces.filter((i) => !isFakeIface(i.name));
  if (real.length === 0) return "全部";

  // 有流量的真实网卡（避免仅按名字选中空闲网卡）
  const active = real.filter((i) => i.download_speed > 0 || i.upload_speed > 0);
  const pool = active.length > 0 ? active : real;

  const sorted = [...pool].sort((a, b) => {
    const pa = ifacePriority(a.name);
    const pb = ifacePriority(b.name);
    if (pa !== pb) return pa - pb;
    // 同优先级：流量大的优先
    return (
      b.download_speed + b.upload_speed - (a.download_speed + a.upload_speed)
    );
  });

  return sorted[0].name;
}

/**
 * 统一展示口径（以网络流量卡片为准）：
 * 选中具体网卡 → 显示该网卡数据（无则回退总流量）；"全部" → 总流量。
 */
export function getDisplayNet(
  netStats: NetStatsData | null,
  selectedIface: string,
): DisplayNet {
  if (!netStats) {
    return {
      downloadSpeed: 0,
      uploadSpeed: 0,
      downloadTotal: 0,
      uploadTotal: 0,
    };
  }
  const iface =
    selectedIface !== "全部"
      ? netStats.interfaces.find((i) => i.name === selectedIface)
      : null;
  return {
    downloadSpeed: iface?.download_speed ?? netStats.total_download_speed,
    uploadSpeed: iface?.upload_speed ?? netStats.total_upload_speed,
    downloadTotal: iface?.total_download ?? netStats.total_download,
    uploadTotal: iface?.total_upload ?? netStats.total_upload,
  };
}

// ============================================================================
// Provider — 单数据源轮询
// ============================================================================

export function NetIfaceProvider({ children }: { children: ReactNode }) {
  const [selectedIface, setSelectedIfaceState] = useState("全部");
  const [netStats, setNetStats] = useState<NetStatsData | null>(null);
  const [netInterval, setNetInterval] = useState(1000);
  const [linkSpeeds, setLinkSpeeds] = useState<Record<string, string>>({});
  const userSelectedRef = useRef(false);

  /** 手动选择：锁定选择，停止自动切换 */
  const setSelectedIface = useCallback((name: string) => {
    userSelectedRef.current = true;
    setSelectedIfaceState(name);
  }, []);

  // 统一轮询 get_net_stats（单数据源，Dashboard 与侧边栏读同一份数据）
  //
  // in-flight 守卫（ADR-0001 D7）：`netInterval` 最小可到 500ms（见 Dashboard 的
  // 刷新间隔选项），而后端 get_net_stats 要持 NETWORKS 互斥锁并做全量网卡刷新。
  // 单次耗时超过间隔时，固定 setInterval 会让未完成的请求逐轮叠加。
  // Dashboard 的传感器轮询已有同等守卫，两处必须一致。
  useEffect(() => {
    let mounted = true;
    let inFlight = false;
    const fetch = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const r = await invoke<NetStatsData>("get_net_stats");
        if (!mounted) return;
        setNetStats(r);
        // 未手动选择时，自动跟随有流量的网卡
        if (!userSelectedRef.current && r.interfaces.length > 0) {
          const best = pickBestIface(r.interfaces);
          setSelectedIfaceState((prev) => (best !== prev ? best : prev));
        }
      } catch {
        /* 静默：轮询失败不中断 */
      } finally {
        inFlight = false;
      }
    };
    void fetch();
    const timer = setInterval(fetch, netInterval);
    return () => {
      mounted = false;
      clearInterval(timer);
    };
  }, [netInterval]);

  // 链路协商速度（准静态数据：挂载时读取一次，网卡集合变化时刷新，不进 1s 轮询）
  const ifaceKey = netStats?.interfaces.map((i) => i.name).join("|") ?? "";
  useEffect(() => {
    let mounted = true;
    const fetchSpeeds = async () => {
      try {
        const r = await invoke<Record<string, string>>("get_link_speeds");
        if (mounted) setLinkSpeeds(r);
      } catch (e: unknown) {
        // 读取失败 → 保持空表，卡片隐藏速度显示（优雅降级，不打断流量轮询）
        if (import.meta.env.DEV) {
          logger.warn(
            "NetIfaceContext",
            "get_link_speeds failed:",
            e instanceof Error ? e.message : String(e),
          );
        }
      }
    };
    fetchSpeeds();
    return () => {
      mounted = false;
    };
  }, [ifaceKey]);

  return (
    <NetIfaceContext.Provider
      value={{
        selectedIface,
        setSelectedIface,
        netStats,
        netInterval,
        setNetInterval,
        linkSpeeds,
      }}
    >
      {children}
    </NetIfaceContext.Provider>
  );
}

// ============================================================================
// Hook
// ============================================================================

export function useNetIface(): NetIfaceContextValue {
  const ctx = useContext(NetIfaceContext);
  if (!ctx) {
    throw new Error("useNetIface must be used within NetIfaceProvider");
  }
  return ctx;
}
