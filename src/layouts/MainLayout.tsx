import { Outlet } from "react-router-dom";
import AppSidebar from "../components/AppSidebar";
import { SidebarInset, SidebarProvider } from "../components/ui/sidebar";

/**
 * 主布局：SidebarProvider 托管折叠状态（cookie 持久化 + 快捷键 Ctrl/Cmd+B），
 * 侧边栏组件拆至 AppSidebar，主内容区用 SidebarInset 包裹并保持内部滚动。
 */
export default function MainLayout() {
  return (
    <SidebarProvider>
      <AppSidebar />
      {/* 主内容区：h-svh 固定视口高度，内容超高时在内部滚动（与原布局等价） */}
      <SidebarInset className="h-svh overflow-auto">
        <div className="animate-fade-in p-6">
          <Outlet />
        </div>
      </SidebarInset>
    </SidebarProvider>
  );
}
