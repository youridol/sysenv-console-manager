import { useEffect, useState, type ReactElement } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Minus, Square, X } from "lucide-react";
import { cn } from "../lib/utils";

/**
 * 是否运行在 Tauri 桌面环境。
 *
 * 浏览器 dev（`npm run dev` 直开 5173）没有 `__TAURI_INTERNALS__`，
 * 此时窗口控制按钮无宿主可控制；按钮仍渲染（保持布局一致）但点击静默降级，
 * 避免在纯浏览器调试时报错中断渲染。
 */
function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * 窗口控制按钮组（最小化 / 最大化-还原 / 关闭）。
 *
 * 背景：主窗口改为 `decorations: false`（无边框），由前端自绘标题栏，
 * 目的是让左右侧边栏的分割线能从窗口顶端完整贯穿到底部（原生标题栏会在
 * 顶部把三栏切断）。因此最小化/最大化/关闭必须由前端提供。
 *
 * 行为对齐 Windows 原生：
 * - 关闭 → `close()`（后端 `CloseRequested` 会 `prevent_close` 并隐藏到托盘，
 *   保持既有 close-to-tray 语义不变，见 lib.rs `on_window_event`）；
 * - 最大化/还原 → `toggleMaximize()`，并根据 `isMaximized()` 切换图标；
 * - 双击标题栏空白处 → 等价于切换最大化（Windows 惯例）。
 *
 * 最大化状态通过 `onResized` 监听刷新：不只依赖点击，用户用快捷键
 * （Win+↑）或拖拽到屏幕边缘触发系统最大化时，图标也要跟着变。
 */
export function WindowControls({
  className,
  /**
   * 竖向排布（右侧日志栏折叠态，宽度仅 40px 时使用）。
   * 折叠态横排放不下三个 46px 按钮，改为竖排避免挤压/溢出。
   */
  vertical = false,
}: {
  className?: string;
  vertical?: boolean;
}): ReactElement {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    const win = getCurrentWindow();
    let unlisten: (() => void) | undefined;
    let disposed = false;

    void win
      .isMaximized()
      .then((v) => {
        if (!disposed) setMaximized(v);
      })
      .catch(() => {
        /* 查询失败：保持默认图标，不影响其余交互 */
      });

    // 尺寸变化即刷新最大化状态（覆盖系统快捷键/贴边最大化）
    void win
      .onResized(() => {
        void win
          .isMaximized()
          .then((v) => {
            if (!disposed) setMaximized(v);
          })
          .catch(() => {});
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {});

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const onMinimize = () => {
    if (!isTauri()) return;
    void getCurrentWindow().minimize();
  };
  const onToggleMaximize = () => {
    if (!isTauri()) return;
    void getCurrentWindow().toggleMaximize();
  };
  const onClose = () => {
    if (!isTauri()) return;
    void getCurrentWindow().close();
  };

  /** 通用按钮样式：40×36 命中区（Windows 标准约 46px，此处收窄以省出标题空间），
      hover 反馈，关闭按钮 hover 变红（Windows 惯例）。 */
  const btn = cn(
    "inline-flex shrink-0 items-center justify-center text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
    vertical ? "h-8 w-full" : "h-9 w-10",
  );

  return (
    <div
      className={cn(
        "flex shrink-0 items-center",
        vertical ? "w-full flex-col" : "flex-row",
        className,
      )}
      /* 按钮区不参与标题栏拖拽，否则点按钮会变成拖窗 */
      data-tauri-drag-region={false}
    >
      <button
        type="button"
        onClick={onMinimize}
        aria-label="最小化"
        title="最小化"
        className={btn}
      >
        <Minus className="size-4" />
      </button>
      <button
        type="button"
        onClick={onToggleMaximize}
        aria-label={maximized ? "向下还原" : "最大化"}
        title={maximized ? "向下还原" : "最大化"}
        className={btn}
      >
        {maximized ? (
          /* 还原图标：两个错位方框（Windows 视觉） */
          <span className="relative size-4">
            <Square className="absolute left-0 top-1 size-3" />
            <Square className="absolute left-1 top-0 size-3" />
          </span>
        ) : (
          <Square className="size-3.5" />
        )}
      </button>
      <button
        type="button"
        onClick={onClose}
        aria-label="关闭"
        title="关闭"
        className={cn(btn, "hover:bg-red-500 hover:text-white")}
      >
        <X className="size-4" />
      </button>
    </div>
  );
}

export default WindowControls;
