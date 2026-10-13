import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  CheckCircle2,
  Loader2,
  AlertTriangle,
  Save,
  Plus,
  Trash2,
  X,
} from "lucide-react";
import { cn } from "../lib/utils";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { Field, FieldLabel } from "./ui/field";
import { ToggleGroup, ToggleGroupItem } from "./ui/toggle-group";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "./ui/alert-dialog";

// ============================================================================
// Types（对应 Rust net_config 契约）
// ============================================================================

/** 单个 IPv4 地址条目（支持同一网卡多个 IPv4 跨网段；掩码逐条独立） */
interface Ipv4Entry {
  ip: string;
  mask: string;
}

/** 网络配置应用请求 */
interface NetworkConfigRequest {
  ifname: string;
  mode_v4: "dhcp" | "static";
  /** IPv4 地址列表（static 模式至少 1 条；支持跨网段多地址） */
  ipv4s: Ipv4Entry[];
  /** IPv4 默认网关列表（可 0 个、可多个；附加地址不要求网关） */
  ipv4_gateways: string[];
  /** IPv4 DNS 模式（serde 契约必填；DNS 编辑已移至独立「DNS 配置」卡片，此处沿用系统当前值） */
  mode_dns4: "dhcp" | "static";
  /** IPv4 DNS 服务器列表（同上，幂等重放系统当前值） */
  dns4: string[];
  mode_v6: "dhcp" | "static";
  ipv6: string | null;
  ipv6_gateway: string | null;
  /** IPv6 DNS 模式（serde 契约必填；同上） */
  mode_dns6: "dhcp" | "static";
  /** IPv6 DNS 服务器列表（同上） */
  dns6: string[];
}

/** 单个应用步骤结果 */
interface NetApplyStep {
  name: string;
  ok: boolean;
  message: string;
}

/** 应用结果 */
interface NetApplyResult {
  steps: NetApplyStep[];
  all_ok: boolean;
}

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

/** 按钮组尾部「+」操作项的固定值（点击触发 onAdd，不参与模式选中） */
const ADD_ACTION_VALUE = "__add__";

/** DHCP/静态模式切换开关（尾部可追加「+」操作项，如添加网段；样式同 ButtonGroup 分段组） */
function ModeToggle({
  value,
  onChange,
  disabled,
  onAdd,
  addDisabled,
}: {
  value: "dhcp" | "static";
  onChange: (v: "dhcp" | "static") => void;
  disabled?: boolean;
  /** 尾部 + 按钮点击回调（不传则不渲染 + 项） */
  onAdd?: () => void;
  /** + 按钮独立禁用（如 DHCP 模式下添加网段无意义） */
  addDisabled?: boolean;
}) {
  return (
    <ToggleGroup
      type="single"
      variant="outline"
      value={value}
      onValueChange={(v) => {
        if (!v) return;
        // + 项：触发操作但不改变模式选中（受控 value 不含该值，UI 选中态不变）
        if (v === ADD_ACTION_VALUE) {
          onAdd?.();
          return;
        }
        onChange(v as "dhcp" | "static");
      }}
      disabled={disabled}
      className="h-7"
    >
      <ToggleGroupItem value="dhcp" className="text-xs px-2 h-full">
        自动
      </ToggleGroupItem>
      <ToggleGroupItem value="static" className="text-xs px-2 h-full">
        静态
      </ToggleGroupItem>
      {onAdd && (
        <ToggleGroupItem
          value={ADD_ACTION_VALUE}
          className="text-xs px-1.5 h-full"
          disabled={addDisabled}
          title={addDisabled ? "静态模式下可添加网段" : "添加网段"}
        >
          <Plus className="size-3.5" />
        </ToggleGroupItem>
      )}
    </ToggleGroup>
  );
}

/** 单行 = 一个网段（同网段多地址共享掩码；网关不在此处，由独立网关列表管理） */
interface V4GroupRow {
  /** 同网段内的 IPv4 地址列表 */
  ips: string[];
  /** 子网掩码（组内共享） */
  mask: string;
}

/** 计算 IPv4 网络号（ip & mask 按位与；非法输入返回原 ip 保证可编辑） */
function networkOf(ip: string, mask: string): string {
  const a = ip.split(".");
  const b = mask.split(".");
  if (a.length !== 4 || b.length !== 4) return ip;
  const an = a.map(Number);
  const bn = b.map(Number);
  if (
    an.some((n) => !Number.isInteger(n) || n < 0 || n > 255) ||
    bn.some((n) => !Number.isInteger(n) || n < 0 || n > 255)
  )
    return ip;
  return an.map((n, i) => n & bn[i]).join(".");
}

/** 掩码 → CIDR 前缀长度（如 255.255.255.0 → 24；非法返回 null） */
function maskToCidr(mask: string): number | null {
  const parts = mask.split(".");
  if (parts.length !== 4) return null;
  let bits = 0;
  for (const p of parts) {
    const n = Number(p);
    if (!Number.isInteger(n) || n < 0 || n > 255) return null;
    bits += n.toString(2).replace(/0/g, "").length;
  }
  return bits;
}

/** 按网段分组：网络号+掩码相同的 IPv4 合并到同一行（跨网段地址分列各行） */
function groupBySubnet(cfg: {
  ipv4: string[];
  ipv4_mask: string[];
}): V4GroupRow[] {
  const groups = new Map<string, V4GroupRow>();
  cfg.ipv4.forEach((ip, i) => {
    const mask = cfg.ipv4_mask[i] ?? "";
    const key = `${networkOf(ip, mask)}|${mask}`;
    const g = groups.get(key) ?? { ips: [], mask };
    g.ips.push(ip);
    groups.set(key, g);
  });
  const rows = [...groups.values()];
  return rows.length > 0 ? rows : [{ ips: [""], mask: "" }];
}

// ============================================================================
// 编辑器主组件
// ============================================================================

export default function NetworkConfigEditor({
  cfg,
  onApplied,
}: {
  cfg: AdapterConfig;
  /** 应用成功后回调（父组件据此重新加载配置） */
  onApplied: () => void;
}) {
  // ── 表单状态（初始值从当前配置填充；同网段地址合并到同一行，跨网段分行）──
  const [v4Mode, setV4Mode] = useState<"dhcp" | "static">(
    cfg.ipv4.length > 0 ? "static" : "dhcp",
  );
  const [v4Rows, setV4Rows] = useState<V4GroupRow[]>(() =>
    groupBySubnet(cfg),
  );
  /** IPv4 默认网关列表（独立于网段：可空、可多个；第 1 个为主网关） */
  const [v4Gateways, setV4Gateways] = useState<string[]>(() =>
    cfg.ipv4_gateway.length > 0 ? [...cfg.ipv4_gateway] : [],
  );
  const [v6Mode, setV6Mode] = useState<"dhcp" | "static">("dhcp");
  const [v6Ip, setV6Ip] = useState("");
  const [v6Gw, setV6Gw] = useState(cfg.ipv6_gateway[0] ?? "");

  // ── 应用状态 ──
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [applying, setApplying] = useState(false);
  const [applyResult, setApplyResult] = useState<NetApplyResult | null>(null);
  const [applyErr, setApplyErr] = useState<string | null>(null);

  /** 更新行内指定地址 */
  const updateV4Ip = (gi: number, ii: number, v: string) =>
    setV4Rows((rows) =>
      rows.map((r, idx) =>
        idx === gi
          ? { ...r, ips: r.ips.map((ip, j) => (j === ii ? v : ip)) }
          : r,
      ),
    );
  /** 更新行级字段（掩码，组内共享） */
  const updateV4RowField = (gi: number, field: "mask", v: string) =>
    setV4Rows((rows) =>
      rows.map((r, idx) => (idx === gi ? { ...r, [field]: v } : r)),
    );
  /** 行内追加同网段地址 */
  const addV4Ip = (gi: number) =>
    setV4Rows((rows) =>
      rows.map((r, idx) => (idx === gi ? { ...r, ips: [...r.ips, ""] } : r)),
    );
  /** 行内删除地址（至少保留一个） */
  const removeV4Ip = (gi: number, ii: number) =>
    setV4Rows((rows) =>
      rows.map((r, idx) =>
        idx === gi
          ? {
              ...r,
              ips: r.ips.length <= 1 ? r.ips : r.ips.filter((_, j) => j !== ii),
            }
          : r,
      ),
    );
  /** 追加新网段行 */
  const addV4Row = () =>
    setV4Rows((rows) => [...rows, { ips: [""], mask: "" }]);
  /** 删除整行（至少保留一行） */
  const removeV4Row = (gi: number) =>
    setV4Rows((rows) =>
      rows.length <= 1 ? rows : rows.filter((_, idx) => idx !== gi),
    );
  /** 更新指定网关；网关数量不限，空值提交时由后端过滤 */
  const updateV4Gateway = (i: number, v: string) =>
    setV4Gateways((gs) => gs.map((g, idx) => (idx === i ? v : g)));
  /** 追加一个网关输入框（第 1 个为主网关，其余为附加网关） */
  const addV4Gateway = () => setV4Gateways((gs) => [...gs, ""]);
  /** 删除指定网关 */
  const removeV4Gateway = (i: number) =>
    setV4Gateways((gs) => gs.filter((_, idx) => idx !== i));

  // ── 前端校验 ──
  const validate = (): string | null => {
    if (v4Mode === "static") {
      // 过滤全空行；非空行逐行校验（地址/掩码必填合法）
      const filled = v4Rows.filter(
        (r) => r.ips.some((ip) => ip.trim()) || r.mask.trim(),
      );
      if (filled.length === 0) return "静态模式下至少需要一个 IPv4 地址";
      for (const r of filled) {
        if (!r.mask.trim()) return "IPv4 子网掩码不能为空";
        if (
          r.mask.split(".").length !== 4 ||
          r.mask.split(".").some((n) => !/^\d{1,3}$/.test(n))
        )
          return "IPv4 子网掩码格式不正确";
        for (const ip of r.ips) {
          if (!ip.trim()) return "IPv4 地址不能为空";
          if (
            ip.split(".").length !== 4 ||
            ip.split(".").some((n) => !/^\d{1,3}$/.test(n))
          )
            return "IPv4 地址格式不正确";
        }
      }
      // 网关可空、可多个；填了的必须合法
      for (const g of v4Gateways) {
        const t = g.trim();
        if (t && !/^\d{1,3}(\.\d{1,3}){3}$/.test(t))
          return "IPv4 默认网关格式不正确";
      }
    }
    if (v6Mode === "static" && v6Ip.trim() && !v6Ip.includes(":"))
      return "IPv6 地址格式不正确";
    if (v6Mode === "static" && v6Gw.trim() && !v6Gw.includes(":"))
      return "IPv6 默认网关格式不正确";
    return null;
  };

  // ── 应用配置 ──
  const applyConfig = async () => {
    const err = validate();
    if (err) {
      setApplyErr(err);
      return;
    }
    setConfirmOpen(false);
    setApplying(true);
    setApplyErr(null);
    setApplyResult(null);
    try {
      const req: NetworkConfigRequest = {
        ifname: cfg.name,
        mode_v4: v4Mode,
        // 静态模式：过滤全空行后展平（同网段地址共享掩码，逐条提交跨网段附加）
        ipv4s:
          v4Mode === "static"
            ? v4Rows
                .filter(
                  (r) => r.ips.some((ip) => ip.trim()) || r.mask.trim(),
                )
                .flatMap((r) =>
                  r.ips
                    .filter((ip) => ip.trim())
                    .map((ip) => ({
                      ip: ip.trim(),
                      mask: r.mask.trim(),
                    })),
                )
            : [],
        // 网关独立于网段：可空、可多个；空串由后端过滤（第 1 个为主网关）
        ipv4_gateways:
          v4Mode === "static"
            ? v4Gateways.map((g) => g.trim()).filter((g) => g.length > 0)
            : [],
        // DNS 已移至独立「DNS 配置」卡片（DnsConfigCard）；此处沿用系统当前值，
        // 与后端 NetworkConfigRequest serde 契约一致（字段必填），幂等重放不改变 DNS
        mode_dns4: cfg.ipv4_dns.length > 0 ? "static" : "dhcp",
        dns4: cfg.ipv4_dns,
        mode_v6: v6Mode,
        ipv6: v6Mode === "static" && v6Ip.trim() ? v6Ip.trim() : null,
        ipv6_gateway:
          v6Mode === "static" && v6Gw.trim() ? v6Gw.trim() : null,
        mode_dns6: cfg.ipv6_dns.length > 0 ? "static" : "dhcp",
        dns6: cfg.ipv6_dns,
      };
      const result = await invoke<NetApplyResult>("apply_network_config", {
        req,
      });
      setApplyResult(result);
      if (result.all_ok) onApplied();
    } catch (e) {
      setApplyErr(String(e));
    } finally {
      setApplying(false);
    }
  };

  return (
    <div className="flex flex-col gap-3">
      {/* ── IPv4 ── */}
      <div className="rounded-lg border border-border/60 bg-muted/10 p-3 flex flex-col gap-2.5">
        <div className="flex items-center justify-between flex-wrap gap-2">
          <span className="text-xs font-medium text-foreground">IPv4 配置</span>
          <ModeToggle
            value={v4Mode}
            onChange={setV4Mode}
            disabled={applying}
            onAdd={addV4Row}
            addDisabled={applying || v4Mode === "dhcp"}
          />
        </div>
        {v4Mode === "static" && (
          <div className="flex flex-col gap-3">
            {/* 每行 = 一个网段：同网段多地址并列，掩码/网关组内共享；跨网段分列各行 */}
            {v4Rows.map((row, gi) => {
              const firstIp = row.ips.find((ip) => ip.trim()) ?? "";
              const cidr = maskToCidr(row.mask);
              return (
                <div
                  key={gi}
                  className="rounded-md border border-border/50 bg-background/40 p-2.5 flex flex-col gap-2"
                >
                  {/* 网段头：网络号/前缀 + 行操作（添加同网段地址 / 删除网段） */}
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-[11px] font-medium text-muted-foreground font-mono">
                      {cidr !== null
                        ? `${networkOf(firstIp, row.mask) || "—"}/${cidr}`
                        : row.mask.trim()
                          ? `掩码 ${row.mask.trim()}`
                          : "新网段"}
                    </span>
                    <div className="flex items-center gap-1">
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7 text-muted-foreground hover:text-foreground"
                        disabled={applying}
                        title="添加同网段地址"
                        onClick={() => addV4Ip(gi)}
                      >
                        <Plus className="size-3.5" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7 text-muted-foreground hover:text-red-400 hover:bg-red-500/10"
                        disabled={applying || v4Rows.length <= 1}
                        title="删除此网段（至少保留一行）"
                        onClick={() => removeV4Row(gi)}
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                    </div>
                  </div>
                  {/* 地址区：同网段地址并列（flex-wrap）+ 共享掩码 */}
                  <div className="flex flex-wrap gap-2 items-end">
                    {row.ips.map((ip, ii) => (
                      <div key={ii} className="flex items-end gap-1">
                        <Field className="w-[8.5rem]">
                          <FieldLabel>
                            {ii === 0 ? "IPv4 地址" : `地址 #${ii + 1}`}
                          </FieldLabel>
                          <Input
                            className="h-8 font-mono text-xs"
                            value={ip}
                            onChange={(e) => updateV4Ip(gi, ii, e.target.value)}
                            placeholder="192.168.1.100"
                            disabled={applying}
                          />
                        </Field>
                        {row.ips.length > 1 && (
                          <Button
                            variant="ghost"
                            size="icon"
                            className="h-8 w-7 text-muted-foreground hover:text-red-400 hover:bg-red-500/10"
                            disabled={applying}
                            title="删除此地址"
                            onClick={() => removeV4Ip(gi, ii)}
                          >
                            <X className="size-3.5" />
                          </Button>
                        )}
                      </div>
                    ))}
                    <Field className="w-[8rem]">
                      <FieldLabel>子网掩码</FieldLabel>
                      <Input
                        className="h-8 font-mono text-xs"
                        value={row.mask}
                        onChange={(e) =>
                          updateV4RowField(gi, "mask", e.target.value)
                        }
                        placeholder="255.255.255.0"
                        disabled={applying}
                      />
                    </Field>
                  </div>
                </div>
              );
            })}

            {/* 默认网关（独立于网段）：可留空、可多个；第 1 个为主网关 */}
            <div className="flex flex-col gap-2 rounded-md border border-border/50 bg-background/40 p-2.5">
              <div className="flex items-center justify-between gap-2">
                <span className="text-[11px] font-medium text-muted-foreground">
                  默认网关（全部可留空；第 1 个为主网关，其余为附加网关）
                </span>
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-7 w-7 text-muted-foreground hover:text-foreground"
                  disabled={applying}
                  title="添加网关"
                  onClick={addV4Gateway}
                >
                  <Plus className="size-3.5" />
                </Button>
              </div>
              {v4Gateways.length === 0 && (
                <span className="text-[11px] text-muted-foreground/70">
                  未配置网关（点击 + 可添加）
                </span>
              )}
              <div className="flex flex-wrap gap-2 items-end">
                {v4Gateways.map((g, i) => (
                  <div key={i} className="flex items-end gap-1">
                    <Field className="w-[9rem]">
                      <FieldLabel>
                        {i === 0 ? "主网关（可选）" : `附加网关 #${i}`}
                      </FieldLabel>
                      <Input
                        className="h-8 font-mono text-xs"
                        value={g}
                        onChange={(e) => updateV4Gateway(i, e.target.value)}
                        placeholder="192.168.1.1"
                        disabled={applying}
                      />
                    </Field>
                    <Button
                      variant="ghost"
                      size="icon"
                      className="h-8 w-7 text-muted-foreground hover:text-red-400 hover:bg-red-500/10"
                      disabled={applying}
                      title="删除此网关"
                      onClick={() => removeV4Gateway(i)}
                    >
                      <X className="size-3.5" />
                    </Button>
                  </div>
                ))}
              </div>
            </div>
          </div>
        )}
      </div>

      {/* ── IPv6 ── */}
      <div className="rounded-lg border border-border/60 bg-muted/10 p-3 flex flex-col gap-2.5">
        <div className="flex items-center justify-between flex-wrap gap-2">
          <span className="text-xs font-medium text-foreground">IPv6 配置</span>
          <ModeToggle value={v6Mode} onChange={setV6Mode} disabled={applying} />
        </div>
        {v6Mode === "static" && (
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
            <Field>
              <FieldLabel>IPv6 地址（可选）</FieldLabel>
              <Input
                className="h-8 font-mono text-xs"
                value={v6Ip}
                onChange={(e) => setV6Ip(e.target.value)}
                placeholder="2001:db8::1"
                disabled={applying}
              />
            </Field>
            <Field>
              <FieldLabel>IPv6 默认网关（可选）</FieldLabel>
              <Input
                className="h-8 font-mono text-xs"
                value={v6Gw}
                onChange={(e) => setV6Gw(e.target.value)}
                placeholder="fe80::1"
                disabled={applying}
              />
            </Field>
          </div>
        )}
      </div>

      {/* 应用结果 */}
      {applyErr && (
        <div className="flex items-start gap-2 rounded-lg border border-red-500/30 bg-red-500/5 p-3">
          <AlertTriangle className="size-4 shrink-0 text-red-400 mt-0.5" />
          <div className="flex flex-col gap-0.5 text-xs min-w-0">
            <span className="font-medium text-red-400">配置失败</span>
            <span className="text-red-400/80 break-all">{applyErr}</span>
          </div>
        </div>
      )}
      {applyResult && (
        <div
          className={cn(
            "rounded-lg border p-3 flex flex-col gap-1.5",
            applyResult.all_ok
              ? "border-green-500/30 bg-green-500/5"
              : "border-yellow-500/30 bg-yellow-500/5",
          )}
        >
          <span
            className={cn(
              "text-xs font-medium",
              applyResult.all_ok ? "text-green-400" : "text-yellow-400",
            )}
          >
            {applyResult.all_ok
              ? "全部配置已应用"
              : "部分配置失败，请检查下列步骤"}
          </span>
          {applyResult.steps.map((s, i) => (
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
      )}

      {/* 底部操作 */}
      <div className="flex items-center justify-end gap-2 pt-1">
        <Button
          size="sm"
          onClick={() => {
            setApplyErr(null);
            setConfirmOpen(true);
          }}
          disabled={applying}
        >
          {applying ? (
            <Loader2 className="size-3.5 animate-spin" />
          ) : (
            <Save className="size-3.5" />
          )}
          应用配置
        </Button>
      </div>

      {/* 应用确认对话框 */}
      <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>应用网络配置</AlertDialogTitle>
            <AlertDialogDescription>
              将对接口「{cfg.name}」应用以下配置：
              <br />
              {v4Mode === "static" ? (
                <>
                  {v4Rows
                    .filter(
                      (r) => r.ips.some((ip) => ip.trim()) || r.mask.trim(),
                    )
                    .map((r, i) => {
                      const cidr = maskToCidr(r.mask);
                      return (
                        <span key={i} className="block">
                          网段 {i + 1}
                          {cidr !== null
                            ? `（${networkOf(r.ips.find((ip) => ip.trim()) ?? "", r.mask)}/${cidr}）`
                            : ""}
                          ：{r.ips.filter((ip) => ip.trim()).join(", ") || "—"} /{" "}
                          {r.mask.trim() || "—"}
                        </span>
                      );
                    })}
                  {(() => {
                    const gws = v4Gateways
                      .map((g) => g.trim())
                      .filter((g) => g.length > 0);
                    return (
                      <span className="block">
                        默认网关：{gws.length > 0 ? gws.join(", ") : "未配置"}
                      </span>
                    );
                  })()}
                </>
              ) : (
                "IPv4：自动获取 (DHCP)"
              )}
              <br />
              {v6Mode === "static"
                ? `IPv6 静态：${v6Ip || "—"}${
                    v6Gw.trim() ? `，网关 ${v6Gw.trim()}` : ""
                  }`
                : "IPv6：自动获取 (DHCP)"}
              <br />
              <span className="text-yellow-400">
                ⚠ 配置错误可能导致断网，请确认地址与网段正确。
              </span>
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction onClick={applyConfig}>确认应用</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
