import { Outlet, useLocation } from "react-router-dom";
import AppSidebar from "../components/AppSidebar";
import RightLogPanel from "../components/RightLogPanel";
import { SidebarInset, SidebarProvider } from "../components/ui/sidebar";
import { TitleBar, ResizeHandles } from "../components/TitleBar";
import { scopeForPath } from "../lib/logs";

/** 路由 pathname → 页面标题（自绘标题栏中显示，替代原生标题栏） */
const PAGE_TITLES: Record<string, string> = {
  "/": "硬件信息",
  "/cleanup": "清理缓存",
  "/network": "网络诊断",
  "/net-config": "网络配置",
  "/settings": "系统设置",
  "/services": "服务管理",
  "/environment": "环境检测",
  "/hardware": "硬件检测",
  "/about": "关于",
};

/**
 * 主布局（三栏通高）：左导航 AppSidebar ｜ 主内容区 SidebarInset ｜ 右侧日志栏 RightLogPanel。
 *
 * 布局要点（本次改版）：
 * 1. **窗口无边框**（tauri.conf.json `decorations: false`），标题栏由前端自绘，
 *    因此三栏各自从窗口最顶部开始 —— 左右两条竖分割线（左栏 `border-r`、
 *    右栏 `border-l`）得以从顶端完整贯穿到底部，不再被原生标题栏在顶部切断。
 * 2. 标题栏只画在**中间内容列**顶部（含页面标题 + 窗口控制按钮），
 *    左/右栏的顶部条与之等高（h-9）以保持顶边对齐。
 * 3. `h-svh` 固定视口高度：三栏等高铁定，各自内部滚动。
 * 4. `ResizeHandles` 补回无边框窗口丢失的鼠标缩放能力。
 *
 * SidebarProvider 托管左侧折叠状态（cookie 持久化 + 快捷键 Ctrl/Cmd+B），
 * 主内容区 `flex-1 min-w-0` 吸收剩余宽度并在内部滚动；右侧栏 `shrink-0` 固定宽度。
 */
export default function MainLayout() {
  const { pathname } = useLocation();
  // scopeForPath 已归一化尾斜杠/查询串/大小写，未知路径回落 system
  const scope = scopeForPath(pathname);
  const title =
    PAGE_TITLES[
      pathname.length > 1 ? pathname.replace(/\/+$/, "") : pathname
    ] ?? "SECM";
  // 兜底：未知路由（scope 回落 system）时仍显示一个可辨识的标题
  const displayTitle = title === "SECM" && scope === "system" ? "SECM" : title;

  return (
    <SidebarProvider>
      <AppSidebar />
      {/* 主内容区：h-svh 固定视口高度，内容超高时在内部滚动。
          内层 `@container/main` 是关键：三栏下主内容列 = 视口 − 左栏(16rem) − 右栏(22rem)，
          页面网格若继续用视口断点（lg/xl）会在 1400px 窗口下被误判为宽屏而挤压卡片
          （实测「网络流量」标签竖排）。改以主内容列自身为容器，页面用 `@…/main:`
          变体即可按真实可用宽度决定列数。 */}
      <SidebarInset className="flex h-svh min-w-0 flex-1 flex-col overflow-hidden">
        {/* 标题栏（仅中间列）：自绘标题 + 窗口控制按钮。
            高度 h-9 与左右栏顶部条一致，三栏顶边成一条水平线。 */}
        <TitleBar>
          <span className="truncate text-xs font-medium text-muted-foreground">
            {displayTitle}
          </span>
        </TitleBar>
        <div className="@container/main animate-fade-in min-h-0 flex-1 overflow-auto p-6">
          <Outlet />
        </div>
      </SidebarInset>
      {/* 右侧日志栏：独立列，日志卡片（compact）宿主 */}
      <RightLogPanel />
      {/* 无边框窗口的缩放热区（四边四角），浮于三栏之上 */}
      <ResizeHandles />
    </SidebarProvider>
  );
}
