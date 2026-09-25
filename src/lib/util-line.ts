/**
 * 日志/输出单行化工具 — Network 页面各工具与 iperf3 钩子共用。
 *
 * 从 `src/pages/Network.tsx` 提取，避免 iperf3 钩子重复实现同一语义。
 */

/**
 * 单行化 + 长度上限。
 *
 * 后端 `append_frontend_log` 上限 4096 字节；这里更严（默认 300 字符），
 * 避免单条日志在右侧栏占满可视区。按**码点**截断，不会切断代理对
 * （emoji / 生僻字）。
 */
export function oneLine(s: string, max = 300): string {
  const flat = s.replace(/\s+/g, " ").trim();
  const chars = Array.from(flat);
  return chars.length > max ? `${chars.slice(0, max).join("")}…` : flat;
}
