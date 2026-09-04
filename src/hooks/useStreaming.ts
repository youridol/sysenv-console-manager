import { useState, useRef, useCallback } from "react";
import { Channel } from "@tauri-apps/api/core";

// ============================================================================
// 类型
// ============================================================================

export interface StreamEvent {
  kind: string;
  data: string;
}

/** 消息格式化回调：接收事件，返回要追加的文本行（或 null 表示不追加） */
export type StreamMessageHandler = (ev: StreamEvent) => string | null;

export interface UseStreamingReturn {
  lines: string[];
  summary: string | null;
  error: string | null;
  /** 启动流式操作 */
  start: (
    cmdId: string,
    runFn: (cmdId: string, channel: Channel<StreamEvent>) => Promise<void>,
  ) => Promise<void>;
  /** 重置状态（竞态保护） */
  reset: () => void;
}

// ============================================================================
// Hook
// ============================================================================

/**
 * 通用流式操作钩子 — Channel 管理 + 竞态保护 + 错误处理。
 *
 * @param onMessage 消息格式化回调。每收到一条 StreamEvent 时调用，
 *                  返回要追加到 lines 的字符串，返回 null 则不追加。
 * @param onSummary 收到 "summary" 事件时的回调（可选）
 */
export function useStreaming(
  onMessage: StreamMessageHandler,
  onSummary?: (data: string) => void,
): UseStreamingReturn {
  const [lines, setLines] = useState<string[]>([]);
  const [summary, setSummary] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const genRef = useRef(0);

  const start = useCallback(
    async (
      cmdId: string,
      runFn: (cmdId: string, channel: Channel<StreamEvent>) => Promise<void>,
    ) => {
      setLines([]);
      setSummary(null);
      setError(null);
      const gen = ++genRef.current;
      const channel = new Channel<StreamEvent>();
      channel.onmessage = (ev: StreamEvent) => {
        if (gen !== genRef.current) return;

        if (ev.kind === "summary") {
          setSummary(ev.data);
          onSummary?.(ev.data);
          return;
        }

        if (ev.kind === "error") {
          setError(timestamp() + "  " + ev.data);
          return;
        }

        const line = onMessage(ev);
        if (line !== null) {
          setLines((prev) => [...prev, line]);
        }
      };
      try {
        await runFn(cmdId, channel);
      } catch (e: unknown) {
        const msg = e instanceof Error ? e.message : String(e);
        setError((prev) => prev ?? timestamp() + "  " + msg);
      }
    },
    [onMessage, onSummary],
  );

  const reset = useCallback(() => {
    genRef.current++;
    setLines([]);
    setSummary(null);
    setError(null);
  }, []);

  return { lines, summary, error, start, reset };
}

// ============================================================================
// 工具
// ============================================================================

function timestamp(): string {
  const d = new Date();
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  const ss = String(d.getSeconds()).padStart(2, "0");
  const ms = String(d.getMilliseconds()).padStart(3, "0");
  return `[${hh}:${mm}:${ss}.${ms}]`;
}
