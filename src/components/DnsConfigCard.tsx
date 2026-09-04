import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  CheckCircle2,
  Loader2,
  AlertTriangle,
  Save,
  Globe,
  ShieldCheck,
} from "lucide-react";
import { cn } from "../lib/utils";
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
} from "./ui/card";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { Field, FieldLabel } from "./ui/field";
import { ToggleGroup, ToggleGroupItem } from "./ui/toggle-group";
import { Switch } from "./ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";

// ============================================================================
// Types（对应 Rust net_config 契约，与 NetworkConfigEditor 同构）
// ============================================================================

/** 网络配置（对应 Rust AdapterConfig 契约） */
interface AdapterConfig {
  name: string;
  description: string;
  guid: string | null;
  mac: string | null;
  status: string;
  ipv4: string[];
  ipv4_mask: string[];
  ipv4_gateway: string[];
  ipv4_dns: string[];
  ipv6_link_local: string[];
  ipv6_gateway: string[];
  ipv6_dns: string[];
}

/** 单个应用步骤结果（set_dns / set_doh_config 返回，与 apply_network_config 同构） */
interface NetApplyStep {
  name: string;
  ok: boolean;
  message: string;
}

/** 应用结果（set_dns / set_doh_config 返回） */
interface NetApplyResult {
  steps: NetApplyStep[];
  all_ok: boolean;
}

/** DoH 配置记录（get_doh_config 返回；template=null 表示该 IP 未启用 DoH） */
interface DohEntry {
  ip: string;
  template: string | null;
}

/** DoH 行编辑状态：开关 + 模板（null=关闭/未启用） */
interface DohRowState {
  ip: string;
  enabled: boolean;
  template: string | null;
}

// ============================================================================
// 常量与工具
// ============================================================================

/** DoH 预设模板（Cloudflare / Google / 阿里 / 腾讯） */
const DOH_PRESETS = [
  { label: "Cloudflare", value: "https://cloudflare-dns.com/dns-query" },
  { label: "Google", value: "https://dns.google/dns-query" },
  { label: "阿里 AliDNS", value: "https://dns.alidns.com/resolve" },
  { label: "腾讯 DNSPod", value: "https://doh.pub/dns-query" },
];

/** DHCP/静态模式切换开关（样式与 NetworkConfigEditor ModeToggle 一致） */
function ModeToggle({
  value,
  onChange,
  disabled,
}: {
  value: "dhcp" | "static";
  onChange: (v: "dhcp" | "static") => void;
  disabled?: boolean;
}) {
  return (
    <ToggleGroup
      type="single"
      variant="outline"
      value={value}
      onValueChange={(v) => v && onChange(v as "dhcp" | "static")}
      disabled={disabled}
      className="h-7"
    >
      <ToggleGroupItem value="dhcp" className="text-xs px-2 h-full">
        自动
      </ToggleGroupItem>
      <ToggleGroupItem value="static" className="text-xs px-2 h-full">
        静态
      </ToggleGroupItem>
    </ToggleGroup>
  );
}

/** 把用户输入的 DNS 文本（逗号/分号/空白分隔）拆为列表 */
function splitDns(text: string): string[] {
  return text
    .split(/[,，;；\s]+/)
    .map((s) => s.trim())
    .filter(Boolean);
}

// ============================================================================
// 结果展示小组件（set_dns / set_doh_config 共用）
// ============================================================================

/** 分步应用结果：绿/黄边框 + 逐步骤 ✓/⚠（复用 apply_network_config 的渲染样式） */
function StepResultView({ result }: { result: NetApplyResult }) {
  return (
    <div
      className={cn(
        "rounded-lg border p-3 flex flex-col gap-1.5",
        result.all_ok
          ? "border-green-500/30 bg-green-500/5"
          : "border-yellow-500/30 bg-yellow-500/5",
      )}
    >
      <span
        className={cn(
          "text-xs font-medium",
          result.all_ok ? "text-green-400" : "text-yellow-400",
        )}
      >
        {result.all_ok ? "全部配置已应用" : "部分配置失败，请检查下列步骤"}
      </span>
      {result.steps.map((s, i) => (
        <div key={i} className="flex items-start gap-1.5 text-xs">
          {s.ok ? (
            <CheckCircle2 className="size-3.5 shrink-0 text-green-400 mt-0.5" />
          ) : (
            <AlertTriangle className="size-3.5 shrink-0 text-red-400 mt-0.5" />
          )}
          <div className="min-w-0">
            <span className={s.ok ? "text-muted-foreground" : "text-red-300"}>
              {s.name}
            </span>
            {!s.ok && s.message !== "成功" && (
              <span className="text-red-300/80">：{s.message}</span>
            )}
          </div>
        </div>
      ))}
    </div>
  );
}

/** 错误横幅：红色边框 + 警告图标 + 标题 + 消息 */
function StepErrorBanner({ message }: { message: string }) {
  return (
    <div className="flex items-start gap-2 rounded-lg border border-red-500/30 bg-red-500/5 p-3">
      <AlertTriangle className="size-4 shrink-0 text-red-400 mt-0.5" />
      <div className="flex flex-col gap-0.5 text-xs min-w-0">
        <span className="font-medium text-red-400">配置失败</span>
        <span className="text-red-400/80 break-all">{message}</span>
      </div>
    </div>
  );
}

// ============================================================================
// DNS 配置卡片主组件
// ============================================================================

export default function DnsConfigCard({
  cfg,
  onApplied,
}: {
  /** 当前操作网卡（父组件过滤后的上网网卡，未检测到时为 null） */
  cfg: AdapterConfig | null;
  /** 应用成功后回调（父组件延迟 1.5s 重新加载配置） */
  onApplied: () => void;
}) {
  // ── IPv4 / IPv6 DNS 编辑态（初值来自当前配置；网卡切换时重置）──
  const [dns4Mode, setDns4Mode] = useState<"dhcp" | "static">(
    cfg ? (cfg.ipv4_dns.length > 0 ? "static" : "dhcp") : "dhcp",
  );
  const [dns4Text, setDns4Text] = useState(cfg?.ipv4_dns.join(", ") ?? "");
  const [dns6Mode, setDns6Mode] = useState<"dhcp" | "static">(
    cfg ? (cfg.ipv6_dns.length > 0 ? "static" : "dhcp") : "dhcp",
  );
  const [dns6Text, setDns6Text] = useState(cfg?.ipv6_dns.join(", ") ?? "");

  // ── set_dns 应用状态 ──
  const [dnsApplying, setDnsApplying] = useState(false);
  const [dnsResult, setDnsResult] = useState<NetApplyResult | null>(null);
  const [dnsErr, setDnsErr] = useState<string | null>(null);

  // ── DoH 状态 ──
  // 系统 DoH 记录（IP → 模板），用于新建行的初始态（开关 = template 非空）
  const [dohMap, setDohMap] = useState<Map<string, string>>(new Map());
  const [dohRows, setDohRows] = useState<DohRowState[]>([]);
  const [dohLoading, setDohLoading] = useState(false);
  const [dohErr, setDohErr] = useState<string | null>(null);
  const [dohApplying, setDohApplying] = useState(false);
  const [dohResult, setDohResult] = useState<NetApplyResult | null>(null);

  /** 加载系统 DoH 配置（get_doh_config；仅用于初始化行的开关/模板状态） */
  const loadDoh = async (ifname: string) => {
    setDohLoading(true);
    setDohErr(null);
    try {
      const entries = await invoke<DohEntry[]>("get_doh_config", { ifname });
      const map = new Map<string, string>();
      for (const e of entries) map.set(e.ip, e.template ?? "");
      setDohMap(map);
    } catch (e) {
      setDohErr(String(e));
    } finally {
      setDohLoading(false);
    }
  };

  /** 当前静态 DNS 列表（IPv4 + IPv6 合并去重，DoH 操作对象） */
  const collectStaticIps = (): string[] => {
    const ips = new Set<string>();
    if (dns4Mode === "static") {
      for (const ip of splitDns(dns4Text)) ips.add(ip);
    }
    if (dns6Mode === "static") {
      for (const ip of splitDns(dns6Text)) ips.add(ip);
    }
    return [...ips];
  };

  // 网卡切换：重置 DNS 编辑态 + 重载 DoH（cfg 非空才生效）
  useEffect(() => {
    if (!cfg) return;
    setDns4Mode(cfg.ipv4_dns.length > 0 ? "static" : "dhcp");
    setDns4Text(cfg.ipv4_dns.join(", "));
    setDns6Mode(cfg.ipv6_dns.length > 0 ? "static" : "dhcp");
    setDns6Text(cfg.ipv6_dns.join(", "));
    setDnsErr(null);
    setDnsResult(null);
    setDohErr(null);
    setDohResult(null);
    void loadDoh(cfg.name);
    // 仅在网卡名变化时执行（loadDoh 等函数为组件内定义，依赖闭包值）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cfg?.name]);

  // DNS 静态列表变化：同步 DoH 行（保留已编辑行状态；新 IP 以系统 DoH 记录初始化）
  useEffect(() => {
    if (!cfg) return;
    const ips = collectStaticIps();
    setDohRows((prev) =>
      ips.map((ip) => {
        const existing = prev.find((r) => r.ip === ip);
        if (existing) return existing;
        const sys = dohMap.get(ip) ?? "";
        return { ip, enabled: sys !== "", template: sys || null };
      }),
    );
    // 依赖仅模式与 DNS 文本（collectStaticIps 闭包读取）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dns4Mode, dns4Text, dns6Mode, dns6Text]);

  /** 应用 DNS（set_dns：IPv4 + IPv6 一次提交，不动 IP 地址） */
  const applyDns = async () => {
    if (!cfg) return;
    if (dns4Mode === "static" && splitDns(dns4Text).length === 0) {
      setDnsErr("静态 DNS 模式下请填写至少一个 IPv4 DNS 服务器");
      return;
    }
    if (dns6Mode === "static" && splitDns(dns6Text).length === 0) {
      setDnsErr("静态 DNS 模式下请填写至少一个 IPv6 DNS 服务器");
      return;
    }
    setDnsApplying(true);
    setDnsErr(null);
    setDnsResult(null);
    try {
      const result = await invoke<NetApplyResult>("set_dns", {
        req: {
          ifname: cfg.name,
          mode_dns4: dns4Mode,
          dns4: dns4Mode === "static" ? splitDns(dns4Text) : [],
          mode_dns6: dns6Mode,
          dns6: dns6Mode === "static" ? splitDns(dns6Text) : [],
        },
      });
      setDnsResult(result);
      if (result.all_ok) {
        // 应用后延迟刷新（父组件处理），并同步重载系统 DoH 记录（DNS 变更影响 DoH 可用性）
        onApplied();
        void loadDoh(cfg.name);
      }
    } catch (e) {
      setDnsErr(String(e));
    } finally {
      setDnsApplying(false);
    }
  };

  /** 应用 DoH（set_doh_config：逐 IP 分步结果；开关关 → template null） */
  const applyDoh = async () => {
    if (!cfg) return;
    setDohApplying(true);
    setDohErr(null);
    setDohResult(null);
    try {
      const entries = dohRows.map((r) => ({
        ip: r.ip,
        template:
          r.enabled && r.template && r.template.trim()
            ? r.template.trim()
            : null,
      }));
      const result = await invoke<NetApplyResult>("set_doh_config", {
        ifname: cfg.name,
        entries,
      });
      setDohResult(result);
      if (result.all_ok) {
        onApplied();
        void loadDoh(cfg.name);
      }
    } catch (e) {
      setDohErr(String(e));
    } finally {
      setDohApplying(false);
    }
  };

  /** 行下拉当前值：关闭 / 匹配预设 / 自定义 */
  const rowSelectValue = (r: DohRowState): string => {
    if (!r.enabled || !r.template) return "off";
    const hit = DOH_PRESETS.find((p) => p.value === r.template);
    return hit ? hit.value : "custom";
  };

  /** 更新单行状态（按 IP 定位） */
  const updateRow = (ip: string, patch: Partial<DohRowState>) =>
    setDohRows((rows) =>
      rows.map((r) => (r.ip === ip ? { ...r, ...patch } : r)),
    );

  // 仅静态 DNS 模式才展示 DoH 区块
  const showDoh = dns4Mode === "static" || dns6Mode === "static";
  const staticIps = collectStaticIps();

  return (
    <Card className="animate-fade-in-up stagger-1">
      <CardHeader className="pb-2">
        <div className="flex items-center gap-2">
          <div className="size-8 shrink-0 flex items-center justify-center rounded-md p-1.5 bg-cyan-500/10">
            <Globe className="size-4 text-cyan-400" />
          </div>
          <div>
            <CardTitle className="text-sm">DNS 配置</CardTitle>
            <CardDescription className="text-xs">
              IPv4/IPv6 DNS 服务器 + DoH（DNS over HTTPS）加密解析
            </CardDescription>
          </div>
        </div>
      </CardHeader>
      <CardContent className="pt-0">
        {!cfg ? (
          <p className="text-xs text-muted-foreground">
            未检测到上网网卡，无法配置 DNS。
          </p>
        ) : (
          <div className="flex flex-col gap-3">
            {/* ── IPv4 DNS ── */}
            <div className="rounded-lg border border-border/60 bg-muted/10 p-3 flex flex-col gap-2.5">
              <div className="flex items-center justify-between flex-wrap gap-2">
                <span className="text-xs font-medium text-foreground">
                  IPv4 DNS
                </span>
                <ModeToggle
                  value={dns4Mode}
                  onChange={setDns4Mode}
                  disabled={dnsApplying}
                />
              </div>
              {dns4Mode === "static" && (
                <Field>
                  <FieldLabel>DNS 服务器（多个用逗号分隔）</FieldLabel>
                  <Input
                    className="h-8 font-mono text-xs"
                    value={dns4Text}
                    onChange={(e) => setDns4Text(e.target.value)}
                    placeholder="223.5.5.5, 8.8.8.8"
                    disabled={dnsApplying}
                  />
                </Field>
              )}
            </div>

            {/* ── IPv6 DNS ── */}
            <div className="rounded-lg border border-border/60 bg-muted/10 p-3 flex flex-col gap-2.5">
              <div className="flex items-center justify-between flex-wrap gap-2">
                <span className="text-xs font-medium text-foreground">
                  IPv6 DNS
                </span>
                <ModeToggle
                  value={dns6Mode}
                  onChange={setDns6Mode}
                  disabled={dnsApplying}
                />
              </div>
              {dns6Mode === "static" && (
                <Field>
                  <FieldLabel>DNS 服务器（多个用逗号分隔）</FieldLabel>
                  <Input
                    className="h-8 font-mono text-xs"
                    value={dns6Text}
                    onChange={(e) => setDns6Text(e.target.value)}
                    placeholder="2400:3200::1, 2001:4860:4860::8888"
                    disabled={dnsApplying}
                  />
                </Field>
              )}
            </div>

            {/* DNS 应用按钮 + 结果（独立于 DoH） */}
            <div className="flex items-center justify-end gap-2">
              <Button size="sm" onClick={applyDns} disabled={dnsApplying}>
                {dnsApplying ? (
                  <Loader2 className="size-3.5 animate-spin" />
                ) : (
                  <Save className="size-3.5" />
                )}
                应用 DNS
              </Button>
            </div>
            {dnsErr && <StepErrorBanner message={dnsErr} />}
            {dnsResult && <StepResultView result={dnsResult} />}

            {/* ── DoH（DNS over HTTPS）── */}
            <div className="rounded-lg border border-cyan-500/20 bg-cyan-500/5 p-3 flex flex-col gap-2.5">
              <div className="flex items-center gap-2">
                <ShieldCheck className="size-3.5 text-cyan-400" />
                <span className="text-xs font-medium text-foreground">
                  DoH（DNS over HTTPS）
                </span>
              </div>
              {!showDoh ? (
                <p className="text-xs text-muted-foreground">
                  需先将 DNS 切换为静态后才能配置 DoH。
                </p>
              ) : dohLoading ? (
                <div className="flex items-center gap-2 text-xs text-muted-foreground">
                  <Loader2 className="size-3.5 animate-spin text-cyan-400" />
                  正在读取 DoH 配置…
                </div>
              ) : staticIps.length === 0 ? (
                <p className="text-xs text-muted-foreground">
                  静态 DNS 列表为空，请先在上方填写 DNS 服务器。
                </p>
              ) : (
                <div className="flex flex-col gap-2">
                  {dohRows.map((r) => (
                    <div
                      key={r.ip}
                      className="flex items-center gap-2 flex-wrap rounded-md border border-border/50 bg-background/40 p-2"
                    >
                      <span className="font-mono text-xs min-w-0 break-all">
                        {r.ip}
                      </span>
                      <Switch
                        checked={r.enabled}
                        onCheckedChange={(v) =>
                          updateRow(r.ip, {
                            enabled: v,
                            template: v ? r.template : null,
                          })
                        }
                        disabled={dohApplying}
                        size="sm"
                      />
                      {r.enabled && (
                        <Select
                          value={rowSelectValue(r)}
                          onValueChange={(v) => {
                            if (v === "off") {
                              updateRow(r.ip, { enabled: false, template: null });
                            } else if (v === "custom") {
                              updateRow(r.ip, { enabled: true, template: "" });
                            } else {
                              updateRow(r.ip, { enabled: true, template: v });
                            }
                          }}
                          disabled={dohApplying}
                        >
                          <SelectTrigger className="h-8 w-44 text-xs">
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem value="off">关闭</SelectItem>
                            {DOH_PRESETS.map((p) => (
                              <SelectItem key={p.value} value={p.value}>
                                {p.label}
                              </SelectItem>
                            ))}
                            <SelectItem value="custom">自定义</SelectItem>
                          </SelectContent>
                        </Select>
                      )}
                      {r.enabled && rowSelectValue(r) === "custom" && (
                        <Input
                          className="h-8 font-mono text-xs flex-1 min-w-[200px]"
                          value={r.template ?? ""}
                          onChange={(e) =>
                            updateRow(r.ip, { template: e.target.value })
                          }
                          placeholder="https://dns.example.com/dns-query"
                          disabled={dohApplying}
                        />
                      )}
                    </div>
                  ))}
                </div>
              )}
              {/* 已知系统限制提示 */}
              <p className="text-[11px] text-muted-foreground">
                部分 Windows 系统仅支持修改已存在的 DoH 记录（系统预设）；若启用失败，请先在系统设置
                → 网络 → DNS over HTTPS 中配置，或更换受支持的 DNS 服务器。
              </p>
            </div>

            {/* DoH 应用按钮 + 结果（独立于 DNS） */}
            <div className="flex items-center justify-end gap-2">
              <Button
                size="sm"
                onClick={applyDoh}
                disabled={dohApplying || !showDoh || staticIps.length === 0}
              >
                {dohApplying ? (
                  <Loader2 className="size-3.5 animate-spin" />
                ) : (
                  <Save className="size-3.5" />
                )}
                应用 DoH
              </Button>
            </div>
            {dohErr && <StepErrorBanner message={dohErr} />}
            {dohResult && <StepResultView result={dohResult} />}
          </div>
        )}
      </CardContent>
    </Card>
  );
}
