import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Network, Loader2 } from "lucide-react";
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
} from "../components/ui/card";
import {
  Tabs,
  TabsList,
  TabsTrigger,
  TabsContent,
} from "../components/ui/tabs";
import { PageHeader } from "@/components/PageHeader";
import NetworkConfigEditor from "@/components/NetworkConfigEditor";
import DnsConfigCard from "@/components/DnsConfigCard";
import MacConfigCard from "@/components/MacConfigCard";
import ErrorBanner from "@/components/ErrorBanner";

// ============================================================================
// Types
// ============================================================================

/** 简化网卡名：剥离冗余的 vEthernet 前缀（括号内才是实际交换机名，如 vEthernet (Default Switch) → Default Switch） */
function shortIfaceName(name: string): string {
  const m = /^vEthernet\s*\((.*)\)$/i.exec(name.trim());
  return m && m[1].trim() ? m[1].trim() : name;
}

/** 网络配置（对应 Rust AdapterConfig 契约，ipconfig /all 语义） */
interface AdapterConfig {
  name: string;
  description: string;
  guid: string | null;
  mac: string | null;
  status: string;
  ipv4: string[];
  ipv4_mask: string[];
  ipv4_gateway: string[];
  ipv4_dns: string[];
  ipv6_link_local: string[];
  ipv6_gateway: string[];
  ipv6_dns: string[];
}

// ============================================================================
// 网络配置状态机：idle / loading / done / error
// ============================================================================

/** 网络配置状态机：idle / loading / done / error */
type NetConfigState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "done"; adapters: AdapterConfig[] }
  | { status: "error"; message: string };

// ============================================================================
// Page
// ============================================================================

export default function NetConfigPage() {
  // ── 网络配置 ──
  const [netConfigState, setNetConfigState] = useState<NetConfigState>({
    status: "idle",
  });
  // 当前激活接口（接口切换按钮组，与「环境监测-游戏环境」卡片同款样式）
  const [activeIface, setActiveIface] = useState<string | null>(null);

  /** 加载本地网络配置（MAC / IPv4+掩码 / 网关 / DNS / 链路本地 IPv6） */
  const loadNetConfig = async () => {
    if (netConfigState.status === "loading") return;
    setNetConfigState({ status: "loading" });
    try {
      const adapters = await invoke<AdapterConfig[]>("get_network_config");
      // 仅显示上网网卡：主判定 = 状态 Up 且有默认网关（vEthernet/WSL 等虚拟网卡一般无网关会被滤掉）
      const withGateway = adapters.filter(
        (a) =>
          a.status === "Up" &&
          (a.ipv4_gateway.length > 0 || a.ipv6_gateway.length > 0),
      );
      // 回退判定：网卡 Up 但无网关（罕见），退而求其次用实际地址
      const withAddr = adapters.filter(
        (a) =>
          a.status === "Up" &&
          (a.ipv4.length > 0 || a.ipv6_link_local.length > 0),
      );
      const active = withGateway.length > 0 ? withGateway : withAddr;
      setNetConfigState({ status: "done", adapters: active });
    } catch (e) {
      setNetConfigState({ status: "error", message: String(e) });
    }
  };

  // 进入页面自动加载网络配置（可编辑态直出，无需手动点击加载）
  useEffect(() => {
    void loadNetConfig();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ── Render ──
  // 当前操作的网卡（单卡直出 / 多卡 Tabs 联动 activeIface），DNS 配置卡共享此对象
  const activeCfg =
    netConfigState.status === "done"
      ? netConfigState.adapters.length === 1
        ? netConfigState.adapters[0]
        : (netConfigState.adapters.find((a) => a.name === activeIface) ??
          netConfigState.adapters[0])
      : null;
  return (
    <div className="grid grid-cols-1 @3xl/main:grid-cols-2 gap-4 items-start">
      {/* 页头：垂直结构（PageHeader 契约：标题 + 描述 + 可选操作区）；col-span-full 保证页头独占整行，不占 grid 左列 */}
      <PageHeader
        className="col-span-full"
        title="网络配置"
        description="当前上网网卡 · IPv4 / 网关 / DNS / DoH"
      />

      {/* ── 网络配置（ipconfig /all 语义）── */}
      <Card className="animate-fade-in-up stagger-1">
        <CardHeader className="pb-2">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2">
              <div className="size-8 shrink-0 flex items-center justify-center rounded-md p-1.5 bg-blue-500/10">
                <Network className="size-4 text-blue-400" />
              </div>
              <div>
                <CardTitle className="text-sm">网络配置</CardTitle>
                <CardDescription className="text-xs">
                  仅显示当前上网网卡（状态 Up 且有默认网关）；物理地址 / IPv4 /
                  网关 / 链路本地 IPv6（直接编辑，应用即生效）
                </CardDescription>
              </div>
            </div>
          </div>
        </CardHeader>
        <CardContent className="pt-0">
          {(netConfigState.status === "idle" ||
            netConfigState.status === "loading") && (
            <div className="flex items-center gap-2 text-xs text-muted-foreground">
              <Loader2 className="size-3.5 animate-spin text-blue-400" />
              正在读取网卡配置…
            </div>
          )}
          {netConfigState.status === "done" &&
            (netConfigState.adapters.length === 0 ? (
              <p className="text-xs text-muted-foreground">
                未检测到上网网卡（状态 Up 且无默认网关）。
              </p>
            ) : netConfigState.adapters.length === 1 ? (
              <NetworkConfigEditor
                cfg={netConfigState.adapters[0]}
                onApplied={() => {
                  // 延迟刷新：先让用户看到应用结果提示，再重新读取生效后的配置
                  setTimeout(() => void loadNetConfig(), 1500);
                }}
              />
            ) : (
              <Tabs
                value={activeIface ?? netConfigState.adapters[0].name}
                onValueChange={setActiveIface}
              >
                {/* 接口切换按钮组：完全复刻「环境监测-游戏环境」卡片样式；名称简化 + 截断 + 换行（无滚动条） */}
                <TabsList className="h-auto w-full justify-start gap-1 flex-wrap bg-transparent p-0">
                  {netConfigState.adapters.map((a) => (
                    <TabsTrigger
                      key={a.name}
                      value={a.name}
                      title={a.name || "未知接口"}
                      className="h-7 flex-none rounded text-xs data-[state=active]:bg-orange-500/20 data-[state=active]:text-orange-400 max-w-[10rem]"
                    >
                      {/* 简化显示（vEthernet 前缀剥离），长名称截断，悬停 title 查看全名 */}
                      <span className="min-w-0 truncate">
                        {shortIfaceName(a.name) || "未知接口"}
                      </span>
                    </TabsTrigger>
                  ))}
                </TabsList>
                {netConfigState.adapters.map((a) => (
                  <TabsContent key={a.name} value={a.name} className="mt-2">
                    <NetworkConfigEditor
                      cfg={a}
                      onApplied={() => {
                        // 延迟刷新：先让用户看到应用结果提示，再重新读取生效后的配置
                        setTimeout(() => void loadNetConfig(), 1500);
                      }}
                    />
                  </TabsContent>
                ))}
              </Tabs>
            ))}
          {netConfigState.status === "error" && (
            <ErrorBanner title="读取失败" message={netConfigState.message} />
          )}
        </CardContent>
      </Card>

      {/* 右列：DNS 配置 + 物理地址（MAC，高级）独立卡片，共享上网网卡与 activeIface */}
      <div className="flex flex-col gap-4">
        <DnsConfigCard
          cfg={activeCfg}
          onApplied={() => {
            // 延迟刷新：先让用户看到应用结果提示，再重新读取生效后的配置
            setTimeout(() => void loadNetConfig(), 1500);
          }}
        />
        {/* 物理地址（MAC，高级）：独立卡片；与网络配置卡/DNS 卡共享当前上网网卡 */}
        <MacConfigCard
          cfg={activeCfg}
          onApplied={() => {
            // 延迟刷新：先让用户看到应用结果提示，再重新读取生效后的配置
            setTimeout(() => void loadNetConfig(), 1500);
          }}
        />
      </div>
    </div>
  );
}
