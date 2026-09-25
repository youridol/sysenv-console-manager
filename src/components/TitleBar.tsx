import type { ReactElement } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { cn } from "../lib/utils";

/**
 * 缩放方向。与 `@tauri-apps/api/window` 内部的 `ResizeDirection` 同构，
 * 但该类型未从模块导出（仅内部使用），故此处本地声明一份。
 */
type ResizeDirection =
  | "East"
  | "North"
  | "NorthEast"
  | "NorthWest"
  | "South"
  | "SouthEast"
  | "SouthWest"
  | "West";

/**
 * 标题栏（自绘）。
 *
 * 主窗口 `decorations: false` 后，拖拽/双击最大化必须由前端提供：
 * - 本组件的空白区域带 `data-tauri-drag-region`（Tauri 内建拖拽区域，
 *   无需 IPC，性能优于 `startDragging()`；双击自动切换最大化）。
 *
 * **窗口控制按钮不在此组件内**：三栏通顶布局下它们位于右侧日志栏的顶部条
 * （即整个窗口的最右上角，见 RightLogPanel 与 WindowControls），本组件只承载
 * 中间内容列的标题与拖拽区。两条竖分割线因此不被标题栏切断。
 *
 * 高度 `h-9` 与左右两栏的顶部条一致，保证三栏顶边成一条水平线。
 */
export function TitleBar({
  /** 左侧插入位（如左栏品牌区上方的留白/标识） */
  leading,
  /** 中间标题区 */
  children,
  className,
}: {
  leading?: React.ReactNode;
  children?: React.ReactNode;
  className?: string;
}): ReactElement {
  return (
    <div
      className={cn(
        "flex h-9 shrink-0 items-center gap-2 border-b border-border px-1 select-none",
        className,
      )}
    >
      {leading}
      {/* 拖拽区：占满整条，双击最大化由 Tauri 内建处理。
          必须用 `data-tauri-drag-region="deep"`（而非裸属性）：
          Tauri 的语义是裸属性**只在直接点击该元素本身时**才拖拽
          （见 tauri/src/window/scripts/drag.js `isDragRegion`：
          `attr === '' → return el === composedPath[0]`），
          而真正的鼠标落点通常是内部的标题 `<span>`（子元素），
          裸属性会导致「点标题拖不动窗口」。`deep` 表示子树任意位置均可拖拽。 */}
      <div
        data-tauri-drag-region="deep"
        className="flex h-full min-w-0 flex-1 items-center gap-2 px-2"
      >
        {children}
      </div>
    </div>
  );
}

/** 可调整尺寸的边/角。顺序即渲染顺序（先四边后四角，角在后以取得更高命中优先级）。 */
const RESIZE_EDGES: Array<{
  dir: ResizeDirection;
  /** 命中区定位类（覆盖窗口边沿，略向内延伸保证可抓取） */
  cls: string;
  label: string;
}> = [
  {
    dir: "North",
    cls: "top-0 left-2 right-2 h-1 cursor-ns-resize",
    label: "上边",
  },
  {
    dir: "South",
    cls: "bottom-0 left-2 right-2 h-1 cursor-ns-resize",
    label: "下边",
  },
  {
    dir: "West",
    cls: "left-0 top-2 bottom-2 w-1 cursor-ew-resize",
    label: "左边",
  },
  {
    dir: "East",
    cls: "right-0 top-2 bottom-2 w-1 cursor-ew-resize",
    label: "右边",
  },
  {
    dir: "NorthWest",
    cls: "left-0 top-0 size-2 cursor-nwse-resize",
    label: "左上角",
  },
  {
    dir: "NorthEast",
    cls: "right-0 top-0 size-2 cursor-nesw-resize",
    label: "右上角",
  },
  {
    dir: "SouthWest",
    cls: "left-0 bottom-0 size-2 cursor-nesw-resize",
    label: "左下角",
  },
  {
    dir: "SouthEast",
    cls: "right-0 bottom-0 size-2 cursor-nwse-resize",
    label: "右下角",
  },
];

/**
 * 无边框窗口的调整尺寸热区。
 *
 * `decorations: false` 会移除系统提供的边框，窗口将**无法用鼠标拖拽缩放**
 * （也丢失系统级 Aero Snap 拖拽）。Tauri 提供 `startResizeDragging(direction)`
 * 来补回该能力，本组件在窗口四边四角铺一层透明热区调用它。
 *
 * 注意：热区必须浮在三栏之上（`fixed` + 高 `z-index`），且**不能拦截**内部点击——
 * 它们只有 1-2px 宽/高且贴边，正常内容不会落在其上。
 * 浏览器 dev 环境无 Tauri 宿主时静默降级（点击无效果，不抛错）。
 */
export function ResizeHandles(): ReactElement {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) {
    return <></>;
  }
  const onPointerDown = (dir: ResizeDirection) => (e: React.PointerEvent) => {
    // 仅响应主键（触控/笔无 button 概念）
    if (e.pointerType === "mouse" && e.button !== 0) return;
    e.preventDefault();
    void getCurrentWindow().startResizeDragging(dir);
  };

  return (
    <>
      {RESIZE_EDGES.map(({ dir, cls, label }) => (
        <div
          key={dir}
          role="separator"
          aria-label={`调整窗口大小（${label}）`}
          onPointerDown={onPointerDown(dir)}
          className={cn("fixed z-50", cls)}
        />
      ))}
    </>
  );
}
