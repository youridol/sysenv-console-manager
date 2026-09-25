import { useCallback, useEffect, useRef, useState } from "react";
import type { ReactElement } from "react";
import { PanelRightClose, ScrollText } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import LogsCard from "@/components/logs/LogsCard";
import { WindowControls } from "@/components/WindowControls";

/** localStorage 键：右侧日志栏折叠状态（"1" = 折叠） */
export const LOG_PANEL_COLLAPSED_KEY = "secm:log-panel-collapsed";

/** localStorage 键：右侧日志栏展开宽度（像素整数字符串） */
export const LOG_PANEL_WIDTH_KEY = "secm:log-panel-width";

/** 窄视口阈值：窗口宽度小于该值时右侧栏默认折叠 */
const NARROW_VIEWPORT_PX = 1100;

/** 展开宽度约束（契约 v2 §A）：默认 22rem，最小 16rem，最大 min(40rem, 50vw) */
const DEFAULT_WIDTH_REM = 22;
const MIN_WIDTH_REM = 16;
const MAX_WIDTH_REM = 40;

/** 键盘调整步长（px，契约 v2 §A：←/→ 步长 16px） */
const KEYBOARD_STEP_PX = 16;

/** 根字号回落值（正常情况下为 16px） */
const REM_PX_FALLBACK = 16;

/**
 * 响应式折叠规则（契约 v2 §A）：
 *   1. `localStorage[secm:log-panel-collapsed] === "1"` → 初始折叠；`"0"` → 初始展开（显式偏好）。
 *   2. 键缺失（从未显式选择过）→ 初始值由 `window.innerWidth < 1100` 决定；
 *      并监听 window resize 跟随视口变化自动折叠/展开，**不写 localStorage**。
 *   3. 用户点击折叠按钮即为「显式选择」：写入 localStorage，此后 resize 不再改动态（显式偏好优先）。
 *   4. 键值非法（非 "0"/"1"）按缺失处理。
 */
function readStoredPref(): boolean | null {
  try {
    const raw = localStorage.getItem(LOG_PANEL_COLLAPSED_KEY);
    if (raw === "1") return true;
    if (raw === "0") return false;
    return null;
  } catch {
    // localStorage 不可用（隐私模式等）→ 视为无偏好，回落响应式默认
    return null;
  }
}

function isNarrow(): boolean {
  return (
    typeof window !== "undefined" && window.innerWidth < NARROW_VIEWPORT_PX
  );
}

/** 根元素字号（rem → px 换算基准）；不可用时回落 16 */
function rootFontPx(): number {
  if (typeof document === "undefined") return REM_PX_FALLBACK;
  const fs = parseFloat(getComputedStyle(document.documentElement).fontSize);
  return Number.isFinite(fs) && fs > 0 ? fs : REM_PX_FALLBACK;
}

/** 当前允许的宽度区间：min = 16rem，max = min(40rem, 50vw) */
function widthBounds(): { min: number; max: number } {
  const rem = rootFontPx();
  const min = MIN_WIDTH_REM * rem;
  const vw =
    typeof window === "undefined"
      ? Number.POSITIVE_INFINITY
      : window.innerWidth;
  const max = Math.min(MAX_WIDTH_REM * rem, vw * 0.5);
  // 极窄视口下 50vw 可能小于 16rem：此时以 min 为准（max 不低于 min）
  return { min, max: Math.max(min, max) };
}

/** 任意输入 → 落在 [min, max] 内的整数像素宽度 */
function clampWidth(px: number): number {
  const { min, max } = widthBounds();
  if (!Number.isFinite(px)) return clampWidth(DEFAULT_WIDTH_REM * rootFontPx());
  return Math.round(Math.min(max, Math.max(min, px)));
}

/** 读取持久化宽度：缺失/非法/越界 → 默认 22rem 并 clamp */
function readStoredWidth(): number {
  const fallback = DEFAULT_WIDTH_REM * rootFontPx();
  try {
    const raw = localStorage.getItem(LOG_PANEL_WIDTH_KEY);
    if (raw === null) return clampWidth(fallback);
    const parsed = Number.parseInt(raw, 10);
    if (!Number.isFinite(parsed)) return clampWidth(fallback);
    return clampWidth(parsed);
  } catch {
    // localStorage 不可用 → 回落默认宽度
    return clampWidth(fallback);
  }
}

/**
 * 右侧日志栏：承载 `<LogsCard compact />` 的独立列。
 *
 * - 展开宽度由拖拽手柄控制（默认 22rem，可持久化）；折叠态固定 `w-10`。
 * - 折叠时卸载日志卡片（竖条不承载内容）：重新展开会重新订阅 `log-event` 并
 *   `get_logs` 拉取一次最新缓冲区，保证「展开即最新」且自动滚动到底部可用。
 */
export default function RightLogPanel({
  className,
}: {
  className?: string;
}): ReactElement {
  // 挂载时求值一次：有显式偏好则沿用，否则按视口宽度决定（不写 localStorage）
  const storedPrefRef = useRef<boolean | null>(readStoredPref());
  const [collapsed, setCollapsed] = useState<boolean>(() =>
    storedPrefRef.current === null ? isNarrow() : storedPrefRef.current,
  );

  // 展开宽度（px）；初始值来自 localStorage 并已 clamp
  const [width, setWidth] = useState<number>(() => readStoredWidth());
  const [dragging, setDragging] = useState(false);
  /** 拖拽会话：起点指针 x 与起点宽度（用 ref 避免每次 pointermove 触发重渲染依赖） */
  const dragRef = useRef<{ startX: number; startWidth: number } | null>(null);
  /** 最新宽度（供拖拽结束时的持久化 effect 读取，避免把 width 放进依赖导致高频写入） */
  const widthRef = useRef(width);
  widthRef.current = width;

  // resize：仅在「从未显式选择过」时跟随视口自动折叠/展开
  useEffect(() => {
    const onResize = () => {
      if (storedPrefRef.current !== null) return; // 显式偏好优先，不被 resize 覆盖
      setCollapsed(isNarrow());
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  // resize：视口变化时重新 clamp 宽度（max 依赖 50vw），避免宽度越界
  useEffect(() => {
    const onResize = () => setWidth((w) => clampWidth(w));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  // 拖拽期间在 window 上监听：指针移出窗口后仍能收到 up/cancel（外加 blur 兜底），
  // 保证任何情况下都能结束拖拽并清理副作用。
  useEffect(() => {
    if (!dragging) return;
    const onMove = (e: PointerEvent) => {
      const session = dragRef.current;
      if (!session) return;
      // 手柄在左边缘：指针左移 → 面板变宽
      setWidth(clampWidth(session.startWidth - (e.clientX - session.startX)));
    };
    const endDrag = () => {
      dragRef.current = null;
      setDragging(false);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", endDrag);
    window.addEventListener("pointercancel", endDrag);
    window.addEventListener("blur", endDrag);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", endDrag);
      window.removeEventListener("pointercancel", endDrag);
      window.removeEventListener("blur", endDrag);
    };
  }, [dragging]);

  // 拖拽期间锁定光标与文本选择；始终在 cleanup 中恢复（含跨窗口结束、组件卸载）
  useEffect(() => {
    if (!dragging) return;
    const { body } = document;
    body.classList.add("cursor-col-resize", "select-none");
    return () => {
      body.classList.remove("cursor-col-resize", "select-none");
    };
  }, [dragging]);

  // 持久化宽度：拖拽过程中不写（高频）；拖拽结束/键盘调整后写入整数像素串。
  // 跳过首次执行 —— 挂载时不要写入：否则窄视口下初始 clamp 出的偏小值会被固化，
  // 窗口变宽后仍停留在该宽度（等价于丢失默认 22rem）。
  const persistedOnceRef = useRef(false);
  useEffect(() => {
    if (dragging) return;
    if (!persistedOnceRef.current) {
      persistedOnceRef.current = true;
      return;
    }
    try {
      localStorage.setItem(
        LOG_PANEL_WIDTH_KEY,
        String(Math.round(widthRef.current)),
      );
    } catch {
      /* noop：localStorage 不可用时仅本次会话生效 */
    }
  }, [dragging, width]);

  const toggle = useCallback(() => {
    setCollapsed((prev) => {
      const next = !prev;
      // 用户显式选择：持久化，并让此后 resize 不再介入
      storedPrefRef.current = next;
      try {
        localStorage.setItem(LOG_PANEL_COLLAPSED_KEY, next ? "1" : "0");
      } catch {
        /* noop：localStorage 不可用时仅本次会话生效 */
      }
      return next;
    });
  }, []);

  const startDrag = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    // 仅响应主键（触控/笔 pointerType 无 button 概念）
    if (e.pointerType === "mouse" && e.button !== 0) return;
    e.preventDefault();
    // 指针捕获：即使光标移出窗口，pointermove/pointerup 仍会派发到该元素
    // （并经冒泡到达下面的 window 监听），避免拖拽「卡住」。
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      /* noop：个别环境不支持捕获，window 监听 + blur 兜底仍可结束拖拽 */
    }
    dragRef.current = { startX: e.clientX, startWidth: widthRef.current };
    setDragging(true);
  }, []);

  /** 键盘调整：←（手柄左移）= 变宽，→（手柄右移）= 变窄，步长 16px */
  const onHandleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
      e.preventDefault();
      const delta =
        e.key === "ArrowLeft" ? KEYBOARD_STEP_PX : -KEYBOARD_STEP_PX;
      setWidth((w) => clampWidth(w + delta));
    },
    [],
  );

  const bounds = widthBounds();

  return (
    <aside
      data-slot="right-log-panel"
      data-state={collapsed ? "collapsed" : "expanded"}
      aria-label="日志面板"
      style={collapsed ? undefined : { width: `${width}px` }}
      className={cn(
        "relative flex h-svh shrink-0 flex-col overflow-hidden border-l border-border bg-background",
        // 拖拽中禁用宽度过渡：transition-[width] 会让面板滞后于光标；结束后恢复
        !dragging && "transition-[width] duration-200 ease-linear",
        collapsed ? "w-10" : undefined,
        className,
      )}
    >
      {/* 左侧拖拽手柄（约 4px）；折叠态隐藏（折叠宽度固定 w-10） */}
      {!collapsed && (
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="调整日志面板宽度"
          aria-valuenow={Math.round(width)}
          aria-valuemin={Math.round(bounds.min)}
          aria-valuemax={Math.round(bounds.max)}
          tabIndex={0}
          onPointerDown={startDrag}
          onKeyDown={onHandleKeyDown}
          className={cn(
            "absolute inset-y-0 left-0 z-20 w-1 cursor-col-resize touch-none transition-colors",
            // hover / 拖拽中高亮；键盘聚焦时同样可见（保留可访问的焦点反馈）
            dragging
              ? "bg-primary"
              : "bg-transparent hover:bg-primary/40 focus-visible:bg-primary/40",
            "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60",
          )}
        />
      )}

      {/* 顶部条：折叠切换 + 标题；展开态右侧放窗口控制按钮。
          高度 h-9 与左侧栏品牌区、中间标题栏一致 —— 三栏顶边对齐成一条水平线，
          左栏 border-r / 本栏 border-l 两条分割线自窗口顶端贯穿到底。

          窗口控制按钮放在本栏最右：三栏通顶布局下这是整个窗口的最右上角，
          符合 Windows「控制按钮在右上角」的惯例（见 WindowControls 注释）。 */}
      <div
        className={cn(
          // @container/panel：供下面标题的 @[15rem]: 变体按**本栏自身宽度**判断，
          // 而非视口宽度（三栏下视口宽并不代表本栏宽）。
          "@container/panel flex h-9 shrink-0 items-center border-b border-border",
          collapsed ? "justify-center px-1" : "gap-2 pl-3",
        )}
        /* 顶条空白处可拖拽窗口；用 "deep" 使子元素（标题文字/图标）上的
           点击也能拖动（裸属性只认直接点击，见 TitleBar 注释）。 */
        data-tauri-drag-region="deep"
      >
        {!collapsed && (
          <>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  onClick={toggle}
                  aria-label="折叠日志面板"
                  aria-expanded={true}
                >
                  <PanelRightClose className="size-4" />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="left">折叠日志面板</TooltipContent>
            </Tooltip>
            {/* 标题：窄宽度下（栏宽受 22rem 默认/拖拽上限约束）优先让位给窗口
                控制按钮——用 hidden @[15rem]:flex 在不足 240px 时整体隐藏，
                避免出现被截断的「调试日」这类半截文字。 */}
            <span className="hidden min-w-0 items-center gap-1.5 truncate text-sm font-semibold @[15rem]:flex">
              <ScrollText className="size-3.5 shrink-0 text-muted-foreground" />
              调试日志
            </span>
            {/* ml-auto：把窗口控制按钮推到最右侧 */}
            <WindowControls className="ml-auto" />
          </>
        )}
      </div>

      {collapsed ? (
        /* 折叠态：本栏仅 40px 宽，横排放不下控制按钮 → 竖向排列在顶部，
             下方是「日志」竖条（点竖条任意处展开）。 */
        <>
          <WindowControls vertical />
          <button
            type="button"
            onClick={toggle}
            aria-label="展开日志面板"
            className="flex flex-1 cursor-pointer items-start justify-center pt-3 text-muted-foreground transition-colors hover:bg-accent"
          >
            <span className="[writing-mode:vertical-rl] text-[11px] tracking-widest select-none">
              日志
            </span>
          </button>
        </>
      ) : (
        /* 展开态：紧凑日志卡片；卡片内部自带滚动区，本列独立滚动 */
        <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
          <LogsCard compact className="h-full" />
        </div>
      )}
    </aside>
  );
}
