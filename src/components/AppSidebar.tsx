import { NavLink, useLocation } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { cn, formatBytes } from "../lib/utils";
import {
  ArrowDown,
  ArrowUp,
  ChevronLeft,
  ClipboardCheck,
  Bot,
  Cpu,
  Gauge,
  Globe,
  Info,
  LayoutDashboard,
  Monitor,
  PanelLeftOpen,
  ScrollText,
  Server,
  Settings,
  Trash2,
  Wifi,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "./ui/card";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from "./ui/sidebar";
import { useVersion } from "../hooks/useVersion";
import { getDisplayNet, useNetIface } from "../contexts/NetIfaceContext";
import { StatusDot } from "@/components/StatusDot";

/** 侧边栏导航项配置：激活态由 useLocation 判定，图标用各自语义色着色 */
interface NavItem {
  to: string;
  label: string;
  icon: LucideIcon;
  /** 激活态图标着色类 */
  color: string;
}

const navItems: NavItem[] = [
  { to: "/", label: "硬件信息", icon: LayoutDashboard, color: "text-blue-400" },
  { to: "/cleanup", label: "清理缓存", icon: Trash2, color: "text-green-400" },
  { to: "/network", label: "网络诊断", icon: Globe, color: "text-cyan-400" },
  { to: "/net-config", label: "网络配置", icon: Wifi, color: "text-blue-400" },
  {
    to: "/settings",
    label: "系统设置",
    icon: Settings,
    color: "text-purple-400",
  },
  {
    to: "/services",
    label: "服务管理",
    icon: Server,
    color: "text-fuchsia-400",
  },
  {
    to: "/environment",
    label: "环境检测",
    icon: ClipboardCheck,
    color: "text-emerald-400",
  },
  {
    to: "/ai-env",
    label: "AI 环境",
    icon: Bot,
    color: "text-emerald-400",
  },
  { to: "/hardware", label: "硬件检测", icon: Cpu, color: "text-orange-400" },
  {
    to: "/logs",
    label: "调试日志",
    icon: ScrollText,
    color: "text-yellow-400",
  },
  { to: "/about", label: "关于", icon: Info, color: "text-pink-400" },
];

/** IP 信息一次性获取的返回结构（与 get_ip_info 命令契约一致） */
interface IpInfo {
  local_ipv4: string;
  local_ipv6: string;
  public_ipv4: string;
  public_ipv6: string;
  domestic_ip: string;
  domestic_ipv6: string;
  domestic_location: string;
  domestic_ipv6_location: string;
  city: string;
  country: string;
  timezone: string;
}

/**
 * 应用侧边栏（从 MainLayout 拆出的公共组件）。
 * 结构：品牌区 / 导航菜单 / 网络信息卡片 + 折叠切换，折叠动画由 Sidebar 组件托管。
 */
export default function AppSidebar() {
  const location = useLocation();
  const { state, toggleSidebar } = useSidebar();
  const collapsed = state === "collapsed";
  const version = useVersion("0.1.0");

  // 侧边栏网络流量迷你显示（单数据源：共享 Provider 轮询的 netStats）
  const { netStats, selectedIface, linkSpeeds } = useNetIface();
  const [ipInfo, setIpInfo] = useState<IpInfo | null>(null);
  const [ipLoading, setIpLoading] = useState(true);

  // 与网络流量卡片同一口径的展示数据（以卡片为准）
  const displayNet = useMemo(
    () => getDisplayNet(netStats, selectedIface),
    [netStats, selectedIface],
  );
  // 链路协商速度：仅选中具体网卡且有数据时显示（"全部" 视图无单一速度）
  const linkSpeed =
    selectedIface !== "全部" ? linkSpeeds[selectedIface] : undefined;

  // 公网 IPv6 展示：优先国内出口（ipip.net 强制 IPv6），回退 ipify；与公网 IPv4 相同则隐藏
  const ipv6Addr =
    ipInfo && ipInfo.domestic_ipv6 !== "—"
      ? ipInfo.domestic_ipv6
      : (ipInfo?.public_ipv6 ?? "");
  const ipv6Loc =
    ipInfo && ipInfo.domestic_ipv6 !== "—"
      ? ipInfo.domestic_ipv6_location
      : "";
  const showIpv6 =
    !!ipv6Addr && ipv6Addr !== "—" && ipv6Addr !== ipInfo?.public_ipv4;

  useEffect(() => {
    // IP 信息一次性获取
    invoke<IpInfo>("get_ip_info")
      .then((info) => {
        setIpInfo(info);
        setIpLoading(false);
      })
      .catch(() => {
        setIpInfo(null);
        setIpLoading(false);
      });
  }, []);

  return (
    <Sidebar collapsible="icon" variant="floating">
      {/* 品牌区：Monitor 图标 + 绿色状态点 + 应用名与副标题 */}
      <SidebarHeader
        className={cn(
          "h-14 flex-row items-center gap-2 border-b border-sidebar-border px-3",
          collapsed && "justify-center px-2",
        )}
      >
        <div className="relative shrink-0">
          <Monitor className="size-5 text-brand-blue" />
          {/* 静态状态点：animate-ping 无限动画在 WebView2 软件渲染下持续泄漏（诊断 v0.12.0）；收编 StatusDot 统一实现 */}
          <StatusDot className="absolute -right-0.5 -top-0.5" />
        </div>
        {!collapsed && (
          <div className="min-w-0">
            <span className="gradient-text block truncate text-sm font-semibold">
              SECM
            </span>
            <p className="mt-0.5 text-[10px] leading-tight text-muted-foreground/60">
              事实上，恢复就是优化的终点。
            </p>
          </div>
        )}
      </SidebarHeader>

      {/* 导航区：8 个页面入口；折叠态由 SidebarMenuButton 的 tooltip 自动显示标签 */}
      <SidebarContent>
        <SidebarGroup>
          <SidebarMenu>
            {navItems.map(({ to, label, icon: Icon, color }) => {
              const active = location.pathname === to;
              return (
                <SidebarMenuItem key={to}>
                  <SidebarMenuButton
                    asChild
                    isActive={active}
                    tooltip={label}
                    className={cn(
                      "text-muted-foreground",
                      "data-[active=true]:bg-primary/10 data-[active=true]:text-primary data-[active=true]:shadow-sm",
                    )}
                  >
                    <NavLink to={to}>
                      <Icon
                        className={cn("shrink-0", active ? color : undefined)}
                      />
                      <span>{label}</span>
                    </NavLink>
                  </SidebarMenuButton>
                </SidebarMenuItem>
              );
            })}
          </SidebarMenu>
        </SidebarGroup>
      </SidebarContent>

      {/* 底部：网络信息卡片 + 折叠切换与版本号 */}
      <SidebarFooter className="p-0">
        {/* 网络信息迷你卡片：数据与逻辑保持原样，仅换 shadcn 结构 */}
        {!collapsed && (
          <div className="border-t border-sidebar-border px-3 py-1.5">
            {/* py-1.5 gap-1 覆盖 shadcn Card 默认 py-6 gap-6，消除卡片留白 */}
            <Card className="gap-1 rounded-lg py-1.5 shadow-none hover:shadow-none">
              <CardHeader className="px-2.5 py-0.5">
                {/* 标题 + 链路协商速度（移到标题旁） */}
                <div className="flex min-w-0 items-center gap-1.5">
                  <CardTitle className="shrink-0 text-[11px] font-semibold text-muted-foreground">
                    网络信息
                  </CardTitle>
                  {linkSpeed && (
                    <>
                      <Gauge className="size-3 shrink-0 text-yellow-400" />
                      {/* 字号与数据行（text-[10px]）一致 */}
                      <span className="min-w-0 truncate text-[10px] font-medium tabular-nums text-yellow-400/80">
                        {linkSpeed}
                      </span>
                    </>
                  )}
                </div>
              </CardHeader>
              <CardContent className="px-2.5 pb-1">
                {/* 第一行：↓下载 → ↑上传 */}
                <div className="flex items-center gap-1.5 text-[10px] text-muted-foreground">
                  <ArrowDown className="size-3 shrink-0 text-blue-400" />
                  <span className="font-medium tabular-nums text-blue-400/80">
                    {formatBytes(displayNet.downloadSpeed)}/s
                  </span>
                  <ArrowUp className="size-3 shrink-0 text-green-400" />
                  <span className="font-medium tabular-nums text-green-400/80">
                    {formatBytes(displayNet.uploadSpeed)}/s
                  </span>
                </div>
                {/* 下方：IP 信息 */}
                <div className="mt-1 text-[10px] leading-snug text-muted-foreground">
                  {ipLoading ? (
                    <span className="opacity-40">IP 信息获取中...</span>
                  ) : ipInfo ? (
                    <>
                      {ipInfo.local_ipv4 && ipInfo.local_ipv4 !== "—" && (
                        <div className="truncate">
                          本地 {ipInfo.local_ipv4}{" "}
                          <span className="opacity-50">
                            {ipInfo.timezone !== "—" ? ipInfo.timezone : ""}
                          </span>
                        </div>
                      )}
                      {ipInfo.public_ipv4 && ipInfo.public_ipv4 !== "—" && (
                        <div className="truncate">
                          公网 {ipInfo.public_ipv4}{" "}
                          <span className="opacity-50">
                            {[ipInfo.city, ipInfo.country]
                              .filter((v) => v !== "—")
                              .join(" · ")}
                          </span>
                        </div>
                      )}
                      {/* 公网 IPv4 国内出口（ipip.net 强制 IPv4 连接） */}
                      {ipInfo.domestic_ip && ipInfo.domestic_ip !== "—" && (
                        <div className="truncate">
                          公网 {ipInfo.domestic_ip}{" "}
                          <span className="opacity-50">
                            {ipInfo.domestic_location !== "—"
                              ? ipInfo.domestic_location
                              : ""}
                          </span>
                        </div>
                      )}
                      {/* 公网 IPv6 国内出口（优先 ipip.net 强制 IPv6，回退 ipify） */}
                      {showIpv6 && (
                        <div className="truncate text-[11px]">
                          公网 {ipv6Addr}{" "}
                          {ipv6Loc && ipv6Loc !== "—" && (
                            <span className="opacity-50">{ipv6Loc}</span>
                          )}
                        </div>
                      )}
                    </>
                  ) : (
                    <span className="opacity-40">IP 信息不可用</span>
                  )}
                </div>
              </CardContent>
            </Card>
          </div>
        )}

        {/* 折叠切换 + 版本号（折叠态只留切换按钮并居中） */}
        <div
          className={cn(
            "flex items-center border-t border-sidebar-border p-3",
            collapsed ? "justify-center" : "justify-between",
          )}
        >
          {!collapsed && (
            <span className="text-xs text-muted-foreground font-mono">
              v{version || "0.1.0"}
            </span>
          )}
          <button
            type="button"
            onClick={toggleSidebar}
            aria-label={collapsed ? "展开侧边栏" : "折叠侧边栏"}
            className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-accent"
          >
            {collapsed ? (
              <PanelLeftOpen className="size-4" />
            ) : (
              <ChevronLeft className="size-4" />
            )}
          </button>
        </div>
      </SidebarFooter>

    </Sidebar>
  );
}
