import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  CheckCircle2,
  Fingerprint,
  Loader2,
  Save,
  AlertTriangle,
} from "lucide-react";
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
// Types（对应 Rust net_config 契约的 MAC 部分）
// ============================================================================

/** MAC 修改所需的最小网卡信息（取自 AdapterConfig 子集） */
interface MacCfg {
  /** 网卡接口名（netsh 名，FriendlyName） */
  name: string;
  /** 当前物理地址（AA-BB-CC-DD-EE-FF 格式；无则 null） */
  mac: string | null;
  /** 接口 GUID（注册表 NetCfgInstanceId；无则不可修改） */
  guid: string | null;
}

// ============================================================================
// Component
// ============================================================================

/**
 * 物理地址（MAC，高级）独立卡片 — 从 NetworkConfigEditor 拆分
 *
 * 通过注册表覆盖并重启网卡生效（set_network_mac，需管理员）；应用瞬间可能短暂断网，
 * 部分网卡驱动不支持修改。破坏性操作：修改前 AlertDialog 确认。
 */
export default function MacConfigCard({
  cfg,
  onApplied,
}: {
  cfg: MacCfg | null;
  onApplied: () => void;
}) {
  const [macText, setMacText] = useState("");
  const [macBusy, setMacBusy] = useState(false);
  const [macMsg, setMacMsg] = useState<string | null>(null);
  const [macErr, setMacErr] = useState<string | null>(null);
  const [macConfirmOpen, setMacConfirmOpen] = useState(false);

  // ── 应用 MAC ──
  const applyMac = async () => {
    setMacConfirmOpen(false);
    setMacBusy(true);
    setMacErr(null);
    setMacMsg(null);
    try {
      const msg = await invoke<string>("set_network_mac", {
        ifname: cfg?.name,
        mac: macText.trim(),
        guid: cfg?.guid,
      });
      setMacMsg(msg);
      onApplied();
    } catch (e) {
      setMacErr(String(e));
    } finally {
      setMacBusy(false);
    }
  };

  return (
    <Card className="animate-fade-in-up stagger-3">
      <CardHeader className="pb-2">
        <div className="flex items-center gap-2">
          <div className="size-8 shrink-0 flex items-center justify-center rounded-md p-1.5 bg-yellow-500/10">
            <Fingerprint className="size-4 text-yellow-400" />
          </div>
          <div>
            <CardTitle className="text-sm">物理地址（MAC，高级）</CardTitle>
            <CardDescription className="text-xs">
              当前 {cfg?.mac ?? "—"}（{cfg?.guid ? "可修改" : "缺少接口 GUID，不可修改"}）·
              注册表覆盖并重启网卡生效
            </CardDescription>
          </div>
        </div>
      </CardHeader>
      <CardContent className="pt-0">
        <div className="rounded-lg border border-yellow-500/30 bg-yellow-500/5 p-3 flex flex-col gap-2.5">
          <div className="flex items-end gap-2 flex-wrap">
            <Field className="flex-1 min-w-[200px]">
              <FieldLabel>新物理地址</FieldLabel>
              <Input
                className="h-8 font-mono text-xs"
                value={macText}
                onChange={(e) => setMacText(e.target.value)}
                placeholder="AA:BB:CC:DD:EE:FF"
                disabled={macBusy || !cfg?.guid}
              />
            </Field>
            <Button
              variant="outline"
              size="sm"
              className="h-8"
              disabled={macBusy || !cfg?.guid || !macText.trim()}
              onClick={() => setMacConfirmOpen(true)}
            >
              {macBusy ? (
                <Loader2 className="size-3.5 animate-spin" />
              ) : (
                <Save className="size-3.5" />
              )}
              应用 MAC
            </Button>
          </div>
          <p className="text-[11px] text-muted-foreground">
            通过注册表覆盖并重启网卡生效，应用瞬间可能短暂断网；部分网卡驱动不支持修改。
          </p>
          {macMsg && (
            <div className="flex items-start gap-2 rounded-lg border border-green-500/30 bg-green-500/10 p-2.5">
              <CheckCircle2 className="size-4 shrink-0 text-green-400 mt-0.5" />
              <p className="text-xs text-green-300 break-all">{macMsg}</p>
            </div>
          )}
          {macErr && (
            <div className="flex items-start gap-2 rounded-lg border border-red-500/30 bg-red-500/10 p-2.5">
              <AlertTriangle className="size-4 shrink-0 text-red-400 mt-0.5" />
              <p className="text-xs text-red-300 break-all">{macErr}</p>
            </div>
          )}
        </div>
      </CardContent>

      {/* MAC 确认对话框（破坏性操作） */}
      <AlertDialog open={macConfirmOpen} onOpenChange={setMacConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>修改物理地址（MAC）</AlertDialogTitle>
            <AlertDialogDescription>
              将接口「{cfg?.name}」的物理地址修改为{" "}
              <span className="font-mono">{macText.trim()}</span>（当前{" "}
              {cfg?.mac ?? "系统默认"}）。
              <br />
              修改通过注册表覆盖并<strong>重启网卡</strong>生效，应用瞬间可能
              <strong>短暂断网</strong>；部分网卡驱动不支持修改。
              <br />
              <span className="text-yellow-400">
                ⚠ 修改 MAC 可能影响网络准入认证（如校园网/企业网绑定），请谨慎操作。
              </span>
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction onClick={applyMac}>确认修改</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Card>
  );
}
