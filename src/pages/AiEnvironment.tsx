import { logger } from "../lib/logger";
import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { cn } from "../lib/utils";
import { Card, CardHeader, CardContent } from "../components/ui/card";
import { Skeleton } from "../components/ui/skeleton";
import { Button } from "../components/ui/button";
import { PageHeader } from "../components/PageHeader";
import {
  renderChecks,
  safeInvoke,
  SectionHeader,
  type DxCheck,
} from "../components/environment-shared";
import {
  Bot,
  CheckCircle2,
  Terminal,
  Download,
  ArrowUp,
  Loader2,
  RefreshCw,
  Package, Trash2, Plug, Puzzle, ServerCrash,
} from "lucide-react";

/* ── Types ── */

interface AiTool {
  name: string;
  display_name: string;
  npm_package: string;
  installed: boolean;
  version: string;
  latest_version: string;
  path: string;
  upgradable: boolean;
}

interface AiToolsInfo {
  tools: AiTool[];
  checks: DxCheck[];
}

interface NpmEnvironment {
  available: boolean;
  node_version: string;
  npm_version: string;
  prefix: string;
  root: string;
  registry: string;
  global_packages: number;
  global_ai_packages: string[];
  global_mcp_packages: string[];
}

interface McpServerInfo {
  name: string;
  package: string;
  installed: boolean;
  version: string;
}

interface AiExtension {
  tool: string;
  kind: string;
  name: string;
  path: string;
  description: string;
}


/* ── Component ── */

export default function AiEnvironment() {
  const [aiInfo, setAiInfo] = useState<AiToolsInfo | null>(null);
  const [aiLoading, setAiLoading] = useState(true);
  const [installingPkg, setInstallingPkg] = useState<string | null>(null);
  const [latestMap, setLatestMap] = useState<Record<string, string>>({});
  const [npmEnv, setNpmEnv] = useState<NpmEnvironment | null>(null);
  const [mcpServers, setMcpServers] = useState<McpServerInfo[]>([]);
  const [extensions, setExtensions] = useState<AiExtension[]>([]);
  const [opPkg, setOpPkg] = useState<string | null>(null);

  /* ── 独立加载 ── */
  const loadAi = useCallback(async () => {
    setAiLoading(true);
    const ai = await safeInvoke<AiToolsInfo>("check_ai_tools");
    setAiInfo(ai);
    setAiLoading(false);
  }, []);

  const loadAiLatest = useCallback(async () => {
    try {
      const results = await invoke<[string, string][]>(
        "fetch_ai_latest_versions",
      );
      const map: Record<string, string> = {};
      for (const [name, ver] of results) {
        map[name] = ver;
      }
      setLatestMap(map);
    } catch {
      // 静默失败，保持 "获取中..."
    }
  }, []);

  const loadAll = useCallback(() => {
    loadAi().then(() => loadAiLatest());
    // npm 环境 / MCP / 扩展：独立加载，失败不影响主列表
    invoke<NpmEnvironment>("check_npm_environment")
      .then(setNpmEnv)
      .catch(() => logger.warn("AiEnvironment", "npm 环境检测失败"));
    invoke<McpServerInfo[]>("list_mcp_servers")
      .then(setMcpServers)
      .catch(() => logger.warn("AiEnvironment", "MCP 列表加载失败"));
    invoke<AiExtension[]>("list_extensions")
      .then(setExtensions)
      .catch(() => logger.warn("AiEnvironment", "扩展列表加载失败"));
  }, [loadAi, loadAiLatest]);
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    loadAll();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleInstall = async (npmPackage: string) => {
    if (!npmPackage) return;
    setInstallingPkg(npmPackage);
    try {
      const result = await invoke<string>("install_ai_tool", { npmPackage });
      logger.info("AiEnvironment", "安装/升级成功:", result);
      // 等待片刻让文件系统同步新的二进制文件
      await new Promise((r) => setTimeout(r, 500));
      await loadAi();
      await loadAiLatest();
    } catch (e) {
      logger.error("AiEnvironment", "安装/升级失败:", e);
      // 即使失败也尝试刷新状态（可能部分成功）
      try {
        await loadAi();
      } catch {
        logger.debug("AiEnvironment", "刷新AI状态失败，忽略");
      }
    } finally {
      setInstallingPkg(null);
    }
  };

  const handleUninstall = async (npmPackage: string) => {
    if (!npmPackage) return;
    // 确认卸载（防误触）
    const ok = window.confirm(`确认卸载 ${npmPackage}？（npm uninstall -g）`);
    if (!ok) return;
    setOpPkg(npmPackage);
    try {
      const result = await invoke<string>("uninstall_ai_tool", {
        npmPackage,
      });
      logger.info("AiEnvironment", "卸载成功:", result);
      await loadAll();
    } catch (e) {
      logger.error("AiEnvironment", "卸载失败:", e);
      try { await loadAi(); } catch {}
    } finally {
      setOpPkg(null);
    }
  };

  const handleMcpAction = async (pkg: string, action: "install" | "uninstall") => {
    setOpPkg(pkg);
    try {
      const cmd = action === "install" ? "install_mcp_server" : "uninstall_mcp_server";
      const result = await invoke<string>(cmd, { package: pkg });
      logger.info("AiEnvironment", `MCP ${action}:`, result);
      const servers = await invoke<McpServerInfo[]>("list_mcp_servers");
      setMcpServers(servers);
      // 刷新 npm 环境（全局包数变化）
      const env = await invoke<NpmEnvironment>("check_npm_environment");
      setNpmEnv(env);
    } catch (e) {
      logger.error("AiEnvironment", `MCP ${action} 失败:`, e);
    } finally {
      setOpPkg(null);
    }
  };


  /* ── 将远端版本合并到工具列表 ── */
  const mergedTools =
    aiInfo?.tools.map((t) => {
      const latest = latestMap[t.name];
      const upgradable =
        t.installed && !!t.version && !!latest && t.version !== latest;
      return { ...t, latest_version: latest || t.latest_version, upgradable };
    }) ?? [];

  /* ── 渲染 ── */

  return (
    <div className="flex flex-col gap-6">
      {/* ── Header ── */}
      <PageHeader
        title="AI 环境"
        description="检测项清单 &middot; 工具安装与升级"
        actions={
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={loadAll}
            disabled={aiLoading}
            className="shrink-0"
            title="重新检测"
          >
            <RefreshCw className={cn("size-4", aiLoading && "animate-spin")} />
          </Button>
        }
      />

      {/* ── AI 工具检测 ── */}
      <Card className="animate-scale-in stagger-1">
        <CardHeader className="pb-2">
          <SectionHeader
            icon={Bot}
            iconClass="bg-emerald-500/10"
            iconColor="text-emerald-400"
            title="AI 环境"
            description={
              aiLoading
                ? "检测中..."
                : aiInfo
                  ? `${aiInfo.tools.filter((t) => t.installed).length}/${aiInfo.tools.length} 已安装`
                  : "检测失败"
            }
          />
        </CardHeader>
        <CardContent className="pt-0">
          {aiLoading && !aiInfo ? (
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
              {[1, 2, 3, 4, 5, 6, 7, 8].map((i) => (
                <Skeleton key={i} className="h-8 rounded-md" />
              ))}
            </div>
          ) : aiInfo ? (
            <>
              {renderChecks(aiInfo.checks)}
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-2 mt-3">
                {mergedTools.map((tool) => (
                  <div
                    key={tool.name}
                    className={cn(
                      "flex items-center gap-3 px-3 py-2 rounded-lg border transition-colors",
                      tool.installed
                        ? tool.upgradable
                          ? "border-amber-500/40 bg-amber-500/5"
                          : "border-green-500/30 bg-green-500/5"
                        : "border-border bg-muted/20 opacity-60",
                    )}
                  >
                    <Terminal
                      className={cn(
                        "size-4 shrink-0",
                        tool.installed
                          ? tool.upgradable
                            ? "text-amber-400"
                            : "text-green-400"
                          : "text-muted-foreground",
                      )}
                    />
                    <span className="text-sm font-medium min-w-[100px]">
                      {tool.display_name}
                    </span>

                    {tool.installed ? (
                      <div className="flex flex-col gap-0.5 text-xs">
                        <div className="flex items-center gap-1">
                          <span className="text-muted-foreground text-xs">
                            当前
                          </span>
                          <span className="font-mono">
                            {tool.version || "?"}
                          </span>
                        </div>
                        <div className="flex items-center gap-1">
                          <span className="text-muted-foreground text-xs">
                            最新
                          </span>
                          <span
                            className={cn(
                              "font-mono",
                              tool.latest_version
                                ? tool.upgradable
                                  ? "text-amber-400"
                                  : "text-green-400"
                                : "text-muted-foreground",
                            )}
                          >
                            {tool.latest_version || "获取中..."}
                          </span>
                        </div>
                      </div>
                    ) : (
                      <span className="text-xs text-muted-foreground flex-1">
                        未安装
                      </span>
                    )}

                    <div className="flex items-center gap-2 ml-auto shrink-0">
                      {tool.installed && (
                        <span
                          className={cn(
                            "text-xs font-medium",
                            tool.upgradable
                              ? "text-amber-400"
                              : "text-emerald-400",
                          )}
                        >
                          {tool.upgradable ? "可升级" : "已安装"}
                        </span>
                      )}
                      {tool.npm_package ? (
                        <>
                        <Button
                          size="xs"
                          variant="ghost"
                          onClick={() => handleInstall(tool.npm_package)}
                          disabled={installingPkg === tool.npm_package}
                          className={cn(
                            "font-semibold",
                            tool.installed
                              ? tool.upgradable
                                ? "bg-amber-500/15 text-amber-400 hover:bg-amber-500/25 cursor-pointer"
                                : "bg-emerald-500/15 text-emerald-400 border border-emerald-500/30 cursor-default hover:bg-emerald-500/25"
                              : "bg-sky-500/15 text-sky-400 hover:bg-sky-500/25 cursor-pointer",
                          )}
                        >
                          {installingPkg === tool.npm_package ? (
                            <Loader2 className="size-3 animate-spin" />
                          ) : tool.installed ? (
                            tool.upgradable ? (
                              <ArrowUp className="size-3" />
                            ) : (
                              <CheckCircle2 className="size-3" />
                            )
                          ) : (
                            <Download className="size-3" />
                          )}
                          {tool.installed
                            ? tool.upgradable
                              ? "升级"
                              : "已最新"
                            : "安装"}
                        </Button>
                      {tool.installed && tool.npm_package && (
                        <Button
                          size="xs"
                          variant="ghost"
                          onClick={() => handleUninstall(tool.npm_package)}
                          disabled={opPkg === tool.npm_package}
                          className="font-semibold text-red-400/80 hover:text-red-400 hover:bg-red-500/10 cursor-pointer"
                          title={`卸载 ${tool.npm_package}`}
                        >
                          {opPkg === tool.npm_package ? (
                            <Loader2 className="size-3 animate-spin" />
                          ) : (
                            <Trash2 className="size-3" />
                          )}
                          <span className="sr-only">卸载</span>
                        </Button>
                      )}
                        </>
                      ) : !tool.installed ? (
                        <span className="text-xs text-muted-foreground/60 italic">
                          手动安装
                        </span>
                      ) : (
                        <span className="text-xs text-muted-foreground/50 italic">
                          已安装
                        </span>
                      )}
                    </div>
                  </div>
                ))}
              </div>
            </>
          ) : (
            <div className="text-xs text-muted-foreground text-center py-3">
              无法获取 AI 工具信息
            </div>
          )}
        </CardContent>
      </Card>
      {/* ── npm 环境检测 ── */}
      <Card className="animate-scale-in stagger-2">
        <CardHeader className="pb-2">
          <SectionHeader
            icon={Package}
            iconClass="bg-sky-500/10"
            iconColor="text-sky-400"
            title="npm 环境"
            description={
              npmEnv
                ? npmEnv.available
                  ? `${npmEnv.global_packages} 个全局包`
                  : "npm 不可用"
                : "检测中..."
            }
          />
        </CardHeader>
        <CardContent className="pt-0">
          {npmEnv ? (
            <div className="grid grid-cols-2 sm:grid-cols-3 gap-2 text-xs">
              <div className="rounded-lg border border-border/60 bg-muted/20 px-2 py-1.5">
                <div className="text-muted-foreground">Node.js</div>
                <div className="font-mono text-sm">{npmEnv.node_version || "—"}</div>
              </div>
              <div className="rounded-lg border border-border/60 bg-muted/20 px-2 py-1.5">
                <div className="text-muted-foreground">npm</div>
                <div className="font-mono text-sm">{npmEnv.npm_version || "—"}</div>
              </div>
              <div className="rounded-lg border border-border/60 bg-muted/20 px-2 py-1.5">
                <div className="text-muted-foreground">Registry</div>
                <div className="font-mono text-sm truncate">{npmEnv.registry || "—"}</div>
              </div>
              <div className="rounded-lg border border-border/60 bg-muted/20 px-2 py-1.5 col-span-2">
                <div className="text-muted-foreground">全局目录（prefix）</div>
                <div className="font-mono text-sm truncate">{npmEnv.prefix || "—"}</div>
              </div>
              <div className="rounded-lg border border-border/60 bg-muted/20 px-2 py-1.5 col-span-2">
                <div className="text-muted-foreground">全局包目录（root）</div>
                <div className="font-mono text-sm truncate">{npmEnv.root || "—"}</div>
              </div>
              {npmEnv.global_ai_packages.length > 0 && (
                <div className="rounded-lg border border-emerald-500/20 bg-emerald-500/5 px-2 py-1.5 col-span-2">
                  <div className="text-emerald-400">AI 相关全局包</div>
                  <div className="font-mono text-xs truncate">
                    {npmEnv.global_ai_packages.join(" · ") || "—"}
                  </div>
                </div>
              )}
              {npmEnv.global_mcp_packages.length > 0 && (
                <div className="rounded-lg border border-sky-500/20 bg-sky-500/5 px-2 py-1.5 col-span-2">
                  <div className="text-sky-400">MCP 全局包</div>
                  <div className="font-mono text-xs truncate">
                    {npmEnv.global_mcp_packages.join(" · ") || "—"}
                  </div>
                </div>
              )}
            </div>
          ) : (
            <div className="text-xs text-muted-foreground text-center py-2">检测中...</div>
          )}
        </CardContent>
      </Card>

      {/* ── MCP 服务器管理 ── */}
      <Card className="animate-scale-in stagger-3">
        <CardHeader className="pb-2">
          <SectionHeader
            icon={Plug}
            iconClass="bg-violet-500/10"
            iconColor="text-violet-400"
            title="MCP 服务器"
            description={`${mcpServers.filter((s) => s.installed).length}/${mcpServers.length} 已安装`}
          />
        </CardHeader>
        <CardContent className="pt-0">
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
            {mcpServers.map((s) => (
              <div
                key={s.package}
                className={cn(
                  "flex items-center gap-2 px-3 py-2 rounded-lg border",
                  s.installed
                    ? "border-green-500/30 bg-green-500/5"
                    : "border-border bg-muted/20 opacity-70",
                )}
              >
                <ServerCrash
                  className={cn(
                    "size-4 shrink-0",
                    s.installed ? "text-green-400" : "text-muted-foreground",
                  )}
                />
                <div className="flex flex-col min-w-0">
                  <span className="text-sm font-medium truncate">{s.name}</span>
                  <span className="text-xs text-muted-foreground font-mono truncate">
                    {s.installed ? s.version : s.package}
                  </span>
                </div>
                <div className="ml-auto shrink-0">
                  {s.installed ? (
                    <Button
                      size="xs"
                      variant="ghost"
                      onClick={() => handleMcpAction(s.package, "uninstall")}
                      disabled={opPkg === s.package}
                      className="text-red-400/80 hover:text-red-400 hover:bg-red-500/10 cursor-pointer"
                    >
                      {opPkg === s.package ? (
                        <Loader2 className="size-3 animate-spin" />
                      ) : (
                        <Trash2 className="size-3" />
                      )}
                      卸载
                    </Button>
                  ) : (
                    <Button
                      size="xs"
                      variant="ghost"
                      onClick={() => handleMcpAction(s.package, "install")}
                      disabled={opPkg === s.package}
                      className="text-sky-400 hover:bg-sky-500/15 cursor-pointer"
                    >
                      {opPkg === s.package ? (
                        <Loader2 className="size-3 animate-spin" />
                      ) : (
                        <Download className="size-3" />
                      )}
                      安装
                    </Button>
                  )}
                </div>
              </div>
            ))}
          </div>
        </CardContent>
      </Card>

      {/* ── Skills & 插件 ── */}
      <Card className="animate-scale-in stagger-4">
        <CardHeader className="pb-2">
          <SectionHeader
            icon={Puzzle}
            iconClass="bg-amber-500/10"
            iconColor="text-amber-400"
            title="Skills & 插件"
            description={`${extensions.length} 个扩展`}
          />
        </CardHeader>
        <CardContent className="pt-0">
          {extensions.length === 0 ? (
            <div className="text-xs text-muted-foreground text-center py-2">
              未检测到 skills / 插件（各 AI 工具用户目录）
            </div>
          ) : (
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
              {extensions.map((ext, i) => (
                <div
                  key={`${ext.tool}-${ext.kind}-${ext.name}-${i}`}
                  className="flex items-start gap-2 px-3 py-2 rounded-lg border border-border/60 bg-muted/20"
                >
                  <Puzzle className="size-4 shrink-0 mt-0.5 text-amber-400" />
                  <div className="flex flex-col min-w-0 flex-1">
                    <div className="flex items-center gap-1.5">
                      <span className="text-sm font-medium truncate">{ext.name}</span>
                      <span className="text-[10px] px-1 rounded bg-muted text-muted-foreground">{ext.tool}</span>
                      <span className="text-[10px] px-1 rounded bg-violet-500/10 text-violet-400">{ext.kind}</span>
                    </div>
                    {ext.description && (
                      <div className="text-xs text-muted-foreground truncate mt-0.5">
                        {ext.description}
                      </div>
                    )}
                    <div className="text-[10px] font-mono text-muted-foreground/60 truncate mt-0.5">
                      {ext.path}
                    </div>
                  </div>
                </div>
              ))}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
