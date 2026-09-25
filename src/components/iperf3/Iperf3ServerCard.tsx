import { useEffect, useRef, useState } from "react";
import { Copy, Play, Server, ShieldAlert, Square, Eraser } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Field, FieldLabel } from "@/components/ui/field";
import { Switch } from "@/components/ui/switch";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { invoke } from "@tauri-apps/api/core";
import { logger } from "@/lib/logger";
import { fmtDuration } from "@/lib/iperf3";
import { useIperf3Server } from "@/hooks/useIperf3";
import { oneLine } from "@/lib/util-line";

/**
 * 本机 iperf3 服务端卡片。
 *
 * 用途：让**对端**（另一台机器或手机上的 iperf3 客户端）连到本机测带宽。
 * 这是 iperf3 的另一半功能——只做客户端只能测"出去"的方向。
 *
 * 关键行为：
 * - 服务端是**长驻进程**，启停由用户显式控制，不做开机自启；
 * - 应用退出时主进程会强杀它（`lib.rs` 的 `RunEvent::Exit`），不留孤儿；
 * - 输出仅在卡片**展开时**以 1Hz 轮询（避免无意义的后台调用）；
 * - 防火墙默认只给**可复制的命令**，一键添加需二次确认（管理员权限 + 系统级变更）。
 */
export function Iperf3ServerCard() {
  const [expanded, setExpanded] = useState(false);
  const { status, busy, error, startServer, stopServer } =
    useIperf3Server(expanded);

  const [port, setPort] = useState(5201);
  const [oneOff, setOneOff] = useState(false);
  const [confirmFirewall, setConfirmFirewall] = useState(false);
  const [firewallMsg, setFirewallMsg] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const logRef = useRef<HTMLDivElement>(null);

  // 输出自动滚到底部
  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [status.lines.length]);

  const addRuleCmd = `netsh advfirewall firewall add rule name="SECM iperf3 ${port} TCP" dir=in action=allow protocol=TCP localport=${port}`;

  const copyCmd = () => {
    void navigator.clipboard
      .writeText(addRuleCmd)
      .then(() => {
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      })
      .catch(() => {
        // 剪贴板不可用时不影响用户手动选中复制
      });
  };

  const applyFirewall = (action: "add" | "remove") => {
    setConfirmFirewall(false);
    setFirewallMsg(null);
    logger.action(
      "INFO",
      "Network",
      `iperf3 防火墙规则${action === "add" ? "添加" : "移除"}：端口=${port}`,
    );
    void invoke<string>("iperf3_firewall_rule", {
      action,
      port,
      protocol: "TCP",
    })
      .then((msg) => {
        setFirewallMsg(msg);
        logger.action("INFO", "Network", `iperf3 防火墙：${oneLine(msg)}`);
      })
      .catch((e: unknown) => {
        const msg = e instanceof Error ? e.message : String(e);
        setFirewallMsg(msg);
        logger.action("ERROR", "Network", `iperf3 防火墙失败：${oneLine(msg)}`);
      });
  };

  return (
    <Card className="animate-fade-in-up stagger-5">
      <CardHeader className="pb-2">
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <div className="size-8 shrink-0 flex items-center justify-center rounded-md p-1.5 bg-emerald-500/10">
              <Server className="size-4 text-emerald-400" />
            </div>
            <div>
              <CardTitle className="text-sm">iperf3 本机服务端</CardTitle>
              <CardDescription className="text-[11px]">
                供对端设备连接测速；应用退出时自动清理
              </CardDescription>
            </div>
          </div>
          <div className="flex items-center gap-2">
            {status.running ? (
              <Badge
                variant="outline"
                className="border-green-500/30 bg-green-500/10 text-green-400 gap-1.5"
              >
                <span className="size-1.5 rounded-full bg-green-400 animate-pulse" />
                运行中 · 端口 {status.port}
              </Badge>
            ) : (
              <Badge
                variant="outline"
                className="border-border bg-background/50 text-muted-foreground"
              >
                已停止
              </Badge>
            )}
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-2 text-xs text-muted-foreground"
              onClick={() => setExpanded((v) => !v)}
            >
              {expanded ? "收起" : "展开"}
            </Button>
          </div>
        </div>
      </CardHeader>

      {expanded && (
        <CardContent className="pt-0 space-y-3">
          {/* 启停控制 */}
          <div className="flex flex-wrap items-end gap-2">
            <Field className="w-24">
              <FieldLabel className="text-xs">监听端口</FieldLabel>
              <Input
                type="number"
                min={1}
                max={65535}
                className="h-8 text-xs"
                value={port}
                onChange={(e) => setPort(parseInt(e.target.value) || 0)}
                disabled={status.running || busy}
              />
            </Field>
            <div
              className="flex items-center gap-2 pb-1.5"
              title="处理一个连接后自动退出（适合做一次验证）"
            >
              <Switch
                checked={oneOff}
                onCheckedChange={setOneOff}
                disabled={status.running || busy}
              />
              <span className="text-xs text-muted-foreground whitespace-nowrap">
                单次连接后退出
              </span>
            </div>
            <Button
              className="h-8"
              variant={status.running ? "destructive" : "default"}
              onClick={() =>
                status.running ? stopServer() : startServer(port, oneOff)
              }
              disabled={busy || (!status.running && port <= 0)}
            >
              {status.running ? (
                <>
                  <Square className="size-4" />
                  停止服务端
                </>
              ) : (
                <>
                  <Play className="size-4" />
                  启动服务端
                </>
              )}
            </Button>
            {status.running && (
              <span className="text-[11px] text-muted-foreground pb-2">
                PID {status.pid ?? "—"} · 已运行 {fmtDuration(status.uptime_s)}
              </span>
            )}
          </div>

          {error && (
            <p className="text-[11px] text-red-400 break-all">{error}</p>
          )}
          {status.error && (
            <p className="text-[11px] text-red-400 break-all">{status.error}</p>
          )}

          {/* 对端连接提示 */}
          {status.running && (
            <div className="rounded-lg border border-border bg-muted/20 p-2.5">
              <p className="text-[11px] text-muted-foreground">
                在对端设备上执行：
              </p>
              <code className="text-[11px] font-mono text-foreground/90 break-all">
                iperf3 -c &lt;本机IP&gt; -p {status.port}
              </code>
            </div>
          )}

          {/* 服务端输出 */}
          {status.lines.length > 0 && (
            <div className="border-t border-border/60 pt-2">
              <div className="flex items-center justify-between mb-1">
                <span className="text-xs text-muted-foreground">
                  服务端输出 {status.lines.length} 行
                </span>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-6 px-2 text-xs text-muted-foreground"
                  onClick={() => startServer(port, oneOff)}
                  disabled={busy}
                  title="重启服务端以清空输出"
                >
                  <Eraser className="size-3.5" />
                  重启清空
                </Button>
              </div>
              <div
                ref={logRef}
                className="font-mono text-[11px] flex flex-col gap-0.5 max-h-[160px] overflow-y-auto"
              >
                {status.lines.map((l, i) => (
                  <span
                    key={`${i}-${l.slice(0, 24)}`}
                    className="break-all text-muted-foreground"
                  >
                    {l}
                  </span>
                ))}
              </div>
            </div>
          )}

          {/* 防火墙：默认给可复制命令；一键添加需二次确认 */}
          <div className="border-t border-border/60 pt-2">
            <div className="flex items-start gap-1.5">
              <ShieldAlert className="size-3.5 shrink-0 mt-0.5 text-amber-400" />
              <div className="flex-1 min-w-0 space-y-2">
                <p className="text-[11px] text-muted-foreground">
                  若对端连不上，通常是 Windows 防火墙拦截了入站 {port} 端口。
                  下面命令需**管理员权限**执行（右键「以管理员身份运行」终端后粘贴）：
                </p>
                <div className="flex items-center gap-2">
                  <code className="flex-1 text-[10px] font-mono text-muted-foreground bg-muted/40 rounded px-2 py-1.5 break-all">
                    {addRuleCmd}
                  </code>
                  <Button
                    variant="outline"
                    size="sm"
                    className="h-7 px-2 text-xs shrink-0"
                    onClick={copyCmd}
                  >
                    <Copy className="size-3.5" />
                    {copied ? "已复制" : "复制"}
                  </Button>
                </div>
                {!confirmFirewall ? (
                  <div className="flex flex-wrap gap-2">
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-7 px-2 text-xs"
                      onClick={() => setConfirmFirewall(true)}
                    >
                      一键添加放行规则
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-7 px-2 text-xs text-muted-foreground"
                      onClick={() => applyFirewall("remove")}
                    >
                      移除该规则
                    </Button>
                  </div>
                ) : (
                  <div className="rounded-lg border border-amber-500/30 bg-amber-500/5 p-2.5 space-y-2">
                    <p className="text-[11px] text-amber-400">
                      即将修改系统防火墙（需管理员权限）。将添加规则名 「SECM
                      iperf3 {port} TCP」的入站 TCP 放行。
                    </p>
                    <div className="flex gap-2">
                      <Button
                        size="sm"
                        className="h-7 px-2 text-xs"
                        onClick={() => applyFirewall("add")}
                      >
                        确认添加
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        className="h-7 px-2 text-xs"
                        onClick={() => setConfirmFirewall(false)}
                      >
                        取消
                      </Button>
                    </div>
                  </div>
                )}
                {firewallMsg && (
                  <p className="text-[11px] text-muted-foreground break-all">
                    {firewallMsg}
                  </p>
                )}
              </div>
            </div>
          </div>
        </CardContent>
      )}
    </Card>
  );
}
