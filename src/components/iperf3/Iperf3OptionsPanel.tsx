import { useState } from "react";
import { ChevronDown, ChevronRight, Info } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Field, FieldLabel } from "@/components/ui/field";
import { Switch } from "@/components/ui/switch";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  IPERF3_DEFAULTS,
  type Iperf3EndMode,
  type Iperf3Format,
  type Iperf3Options,
  type Iperf3Protocol,
  type Iperf3ToolInfo,
} from "@/lib/iperf3";

interface Props {
  value: Iperf3Options;
  onChange: (p: Partial<Iperf3Options>) => void;
  disabled: boolean;
  /** 工具信息（版本能力门控 + 缺失提示）；加载中为 null */
  info: Iperf3ToolInfo | null;
}

/** 数值输入（空串表示不传该选项）。 */
function TextField({
  label,
  value,
  onChange,
  placeholder,
  disabled,
  hint,
  className,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  disabled?: boolean;
  hint?: string;
  className?: string;
}) {
  return (
    <Field className={className}>
      <FieldLabel className="text-xs" title={hint}>
        {label}
      </FieldLabel>
      <Input
        className="h-8 w-full text-xs"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        disabled={disabled}
      />
    </Field>
  );
}

/** 开关型选项（带说明 tooltip）。 */
function SwitchField({
  label,
  checked,
  onChange,
  disabled,
  hint,
}: {
  label: string;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  hint?: string;
}) {
  return (
    <div className="flex items-center gap-2 pt-4" title={hint}>
      <Switch
        checked={checked}
        onCheckedChange={onChange}
        disabled={disabled}
      />
      <span className="text-xs text-muted-foreground">{label}</span>
    </div>
  );
}

/**
 * iperf3 参数面板。
 *
 * 分两层：**基础**（决定测什么、怎么测，默认展开）与**高级**（协议细节微调，
 * 默认折叠）。高级项过多会让基础项被淹没；折叠后默认视图保持可读。
 *
 * 平台/版本不支持的选项会**置灰并说明原因**，而不是隐藏或静默忽略——
 * 用户需要知道"为什么这个选项点不了"。
 */
export function Iperf3OptionsPanel({
  value: o,
  onChange,
  disabled,
  info,
}: Props) {
  const [advanced, setAdvanced] = useState(false);
  const d = disabled;

  // 版本能力（info 为 null 表示还在加载：按"未知"保守处理，不误启用）
  const supportsBidir = info?.supports_bidir ?? false;
  const supportsDscp = info?.supports_dscp ?? false;
  const known = (info?.major ?? 0) > 0;
  const bidirBlocked = known && !supportsBidir;
  const dscpBlocked = known && !supportsDscp;

  return (
    <div className="flex flex-col gap-3">
      {/* ── 基础区 ─────────────────────────────────────────────────── */}
      <div className="grid grid-cols-2 @3xl/main:grid-cols-4 gap-2">
        {/* 协议 */}
        <Field>
          <FieldLabel className="text-xs">协议</FieldLabel>
          <ToggleGroup
            type="single"
            variant="outline"
            value={o.protocol}
            onValueChange={(v) =>
              v && onChange({ protocol: v as Iperf3Protocol })
            }
            disabled={d}
            className="h-8"
          >
            <ToggleGroupItem value="tcp" className="text-xs px-2 h-full">
              TCP
            </ToggleGroupItem>
            <ToggleGroupItem value="udp" className="text-xs px-2 h-full">
              UDP
            </ToggleGroupItem>
          </ToggleGroup>
        </Field>

        {/* 方向 */}
        <Field>
          <FieldLabel className="text-xs">方向</FieldLabel>
          <ToggleGroup
            type="single"
            variant="outline"
            value={o.bidir ? "bidir" : o.reverse ? "reverse" : "forward"}
            onValueChange={(v) => {
              if (!v) return;
              onChange({
                reverse: v === "reverse",
                bidir: v === "bidir",
              });
            }}
            disabled={d || bidirBlocked}
            className="h-8"
            title={
              bidirBlocked
                ? `当前 iperf3 ${info?.version} 不支持 --bidir（需 3.7 及以上）`
                : "正向：客户端发、服务端收；反向：服务端发；双向：同时收发"
            }
          >
            <ToggleGroupItem value="forward" className="text-xs px-2 h-full">
              正向
            </ToggleGroupItem>
            <ToggleGroupItem value="reverse" className="text-xs px-2 h-full">
              反向
            </ToggleGroupItem>
            <ToggleGroupItem value="bidir" className="text-xs px-2 h-full">
              双向
            </ToggleGroupItem>
          </ToggleGroup>
        </Field>

        {/* 结束条件（-t/-n/-k 互斥，用三选一强制） */}
        <Field>
          <FieldLabel className="text-xs">结束条件</FieldLabel>
          <ToggleGroup
            type="single"
            variant="outline"
            value={o.endMode}
            onValueChange={(v) =>
              v && onChange({ endMode: v as Iperf3EndMode })
            }
            disabled={d}
            className="h-8"
            title="-t（时长）/ -n（字节数）/ -k（块数）三者互斥，iperf3 只接受其中一个"
          >
            <ToggleGroupItem value="time" className="text-xs px-2 h-full">
              时长
            </ToggleGroupItem>
            <ToggleGroupItem value="bytes" className="text-xs px-2 h-full">
              字节
            </ToggleGroupItem>
            <ToggleGroupItem value="blocks" className="text-xs px-2 h-full">
              块数
            </ToggleGroupItem>
          </ToggleGroup>
        </Field>

        {/* 结束条件对应数值 */}
        {o.endMode === "time" ? (
          <Field>
            <FieldLabel className="text-xs">时长（秒）</FieldLabel>
            <Input
              type="number"
              min={1}
              max={86400}
              className="h-8 w-full text-xs"
              value={o.time}
              onChange={(e) =>
                onChange({ time: parseInt(e.target.value) || 0 })
              }
              disabled={d}
            />
          </Field>
        ) : o.endMode === "bytes" ? (
          <TextField
            label="字节数"
            value={o.bytes}
            onChange={(bytes) => onChange({ bytes })}
            placeholder="如 100M"
            disabled={d}
            hint="支持 K/M/G/T 后缀"
          />
        ) : (
          <TextField
            label="块数"
            value={o.blockcount}
            onChange={(blockcount) => onChange({ blockcount })}
            placeholder="如 1000"
            disabled={d}
            hint="支持 K/M/G/T 后缀"
          />
        )}

        {/* 并行流 */}
        <Field>
          <FieldLabel
            className="text-xs"
            title="并行客户端流数量；多流可突破单流 CPU 瓶颈"
          >
            并行流（-P）
          </FieldLabel>
          <Input
            type="number"
            min={1}
            max={128}
            className="h-8 w-full text-xs"
            value={o.parallel}
            onChange={(e) =>
              onChange({ parallel: parseInt(e.target.value) || 1 })
            }
            disabled={d}
          />
        </Field>

        {/* 码率（UDP 常用） */}
        <TextField
          label="码率（-b）"
          value={o.bitrate}
          onChange={(bitrate) => onChange({ bitrate })}
          placeholder={
            o.protocol === "udp" ? "如 10M（UDP 默认 1M）" : "留空=不限速"
          }
          disabled={d}
          hint="支持 n[KMG] 或 n[KMG]/burst，如 100M、10M/1000；留空表示不限速"
        />

        {/* 报告间隔 */}
        <Field>
          <FieldLabel
            className="text-xs"
            title="周期报告间隔；0 表示关闭周期报告"
          >
            间隔（-i 秒）
          </FieldLabel>
          <Input
            type="number"
            min={0}
            max={3600}
            className="h-8 w-full text-xs"
            value={o.interval}
            onChange={(e) =>
              onChange({ interval: parseInt(e.target.value) || 0 })
            }
            disabled={d}
          />
        </Field>

        {/* 单位 */}
        <Field>
          <FieldLabel className="text-xs">单位（-f）</FieldLabel>
          <Select
            value={o.format}
            onValueChange={(v) => onChange({ format: v as Iperf3Format })}
            disabled={d}
          >
            <SelectTrigger className="h-8 w-full text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="k">Kbits</SelectItem>
              <SelectItem value="m">Mbits</SelectItem>
              <SelectItem value="g">Gbits</SelectItem>
              <SelectItem value="t">Tbits</SelectItem>
            </SelectContent>
          </Select>
        </Field>
      </div>

      {/* UDP 未指定码率时的提醒（iperf3 默认仅 1 Mbps，容易被误读为"测不出来"） */}
      {o.protocol === "udp" && o.bitrate.trim() === "" && (
        <p className="text-[11px] text-amber-400/90 flex items-start gap-1.5">
          <Info className="size-3.5 shrink-0 mt-0.5" />
          UDP 未指定码率时 iperf3 默认仅 1
          Mbps。若吞吐看起来异常低，请显式填写码率（如 100M）。
        </p>
      )}

      {/* ── 高级区（可折叠）─────────────────────────────────────────── */}
      <Collapsible open={advanced} onOpenChange={setAdvanced}>
        <CollapsibleTrigger
          className="flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground transition-colors"
          disabled={d}
        >
          {advanced ? (
            <ChevronDown className="size-3.5" />
          ) : (
            <ChevronRight className="size-3.5" />
          )}
          高级选项（预热 / 缓冲 / MSS / TOS·DSCP / 绑定 / 超时 / 文件等）
        </CollapsibleTrigger>
        <CollapsibleContent className="pt-3">
          <div className="grid grid-cols-2 @3xl/main:grid-cols-4 gap-2">
            {/* 预热丢弃 */}
            <Field>
              <FieldLabel
                className="text-xs"
                title="先跑 N 秒再开始统计，跳过 TCP 慢启动"
              >
                预热丢弃（-O 秒）
              </FieldLabel>
              <Input
                type="number"
                min={0}
                max={3600}
                className="h-8 w-full text-xs"
                value={o.omit}
                onChange={(e) =>
                  onChange({ omit: parseInt(e.target.value) || 0 })
                }
                disabled={d}
              />
            </Field>

            <TextField
              label="缓冲长度（-l）"
              value={o.length}
              onChange={(length) => onChange({ length })}
              placeholder="如 128K"
              disabled={d}
              hint="TCP 默认 128KB；UDP 按路径 MTU 动态决定"
            />

            <TextField
              label="窗口/缓冲（-w）"
              value={o.window}
              onChange={(window) => onChange({ window })}
              placeholder="如 2M"
              disabled={d}
              hint="socket 缓冲区大小（间接决定 TCP 窗口）"
            />

            <TextField
              label="最大段大小（-M）"
              value={o.mss}
              onChange={(mss) => onChange({ mss })}
              placeholder="如 1460"
              disabled={d}
              hint="TCP/SCTP MSS（MTU - 40）；上限 32768"
            />

            <TextField
              label="TOS（-S）"
              value={o.tos}
              onChange={(tos) => onChange({ tos })}
              placeholder="如 0x34 / 52"
              disabled={d}
              hint="IP 服务类型字节，0-255；支持十进制/八进制/十六进制写法"
            />

            <TextField
              label="DSCP（--dscp）"
              value={o.dscp}
              onChange={(dscp) => onChange({ dscp })}
              placeholder={dscpBlocked ? `需 iperf3 ≥3.2` : "如 46 或 EF"}
              disabled={d || dscpBlocked}
              hint={
                dscpBlocked
                  ? `当前 iperf3 ${info?.version} 不支持 --dscp（需 3.2 及以上）；可改用 TOS（-S）`
                  : "DSCP 值 0-63（十进制/八进制/十六进制）或符号名（CS1/AF11/EF）"
              }
            />

            <TextField
              label="连接超时（--connect-timeout ms）"
              value={o.connectTimeoutMs}
              onChange={(connectTimeoutMs) => onChange({ connectTimeoutMs })}
              placeholder="如 5000"
              disabled={d}
              hint="控制连接建立超时；缩短可在服务端不可达时更快报错"
            />

            <TextField
              label="CPU 亲和性（-A）"
              value={o.affinity}
              onChange={(affinity) => onChange({ affinity })}
              placeholder="如 1 或 1,2"
              disabled={d}
              hint="将进程绑定到指定 CPU 核心（n 或 n,m）"
            />

            <TextField
              label="绑定地址（-B）"
              value={o.bind}
              onChange={(bind) => onChange({ bind })}
              placeholder="如 192.168.1.50"
              disabled={d}
              hint="绑定到指定本地 IP（多网卡时指定走哪块网卡）"
            />

            <TextField
              label="源端口（--cport）"
              value={o.cport}
              onChange={(cport) => onChange({ cport })}
              placeholder="如 51000"
              disabled={d}
              hint="绑定客户端源端口（默认随机）"
            />

            <TextField
              label="接收超时（--rcv-timeout ms）"
              value={o.rcvTimeoutMs}
              onChange={(rcvTimeoutMs) => onChange({ rcvTimeoutMs })}
              placeholder="默认 120000"
              disabled={d}
              hint="接收空闲超时；超过该时长未收到数据即结束测试"
            />

            <TextField
              label="发送超时（--snd-timeout ms）"
              value={o.sndTimeoutMs}
              onChange={(sndTimeoutMs) => onChange({ sndTimeoutMs })}
              placeholder="默认跟随系统"
              disabled={d}
              hint="未确认 TCP 数据超时；网络中断时可更快结束"
            />

            <TextField
              label="输出标题（-T）"
              value={o.title}
              onChange={(title) => onChange({ title })}
              placeholder="每行前缀"
              disabled={d}
              hint="为每行输出加前缀，便于日志截取"
            />

            <TextField
              label="附加数据（--extra-data）"
              value={o.extraData}
              onChange={(extraData) => onChange({ extraData })}
              placeholder="随 JSON 输出"
              disabled={d}
              hint="写入 JSON 输出的自定义字符串字段"
            />

            {/* 文件（-F）：与 UDP 互斥 */}
            <TextField
              label="文件（-F）"
              value={o.file}
              onChange={(file) => onChange({ file })}
              placeholder={
                o.protocol === "udp" ? "UDP 不可用" : "本地文件绝对路径"
              }
              disabled={d || o.protocol === "udp"}
              hint={
                o.protocol === "udp"
                  ? "iperf3 明确禁止 -F 与 UDP 组合；请切换为 TCP"
                  : "以文件作为发送源/接收槽（用于判断存储是否为瓶颈）；文件必须存在"
              }
            />

            <SwitchField
              label="禁用 Nagle（-N）"
              checked={o.noDelay}
              onChange={(noDelay) => onChange({ noDelay })}
              disabled={d}
              hint="设置 TCP_NODELAY，降低小包延迟（吞吐可能下降）"
            />
            <SwitchField
              label="零拷贝（-Z）"
              checked={o.zerocopy}
              onChange={(zerocopy) => onChange({ zerocopy })}
              disabled={d}
              hint="使用 sendfile 等零拷贝方式发送数据"
            />
            <SwitchField
              label="详细输出（-V）"
              checked={o.verbose}
              onChange={(verbose) => onChange({ verbose })}
              disabled={d}
              hint="输出更详细的诊断信息"
            />
            <SwitchField
              label="取服务端结果"
              checked={o.getServerOutput}
              onChange={(getServerOutput) => onChange({ getServerOutput })}
              disabled={d}
              hint="--get-server-output：测试结束后把服务端结果一并取回"
            />
            <SwitchField
              label="重复载荷"
              checked={o.repeatingPayload}
              onChange={(repeatingPayload) => onChange({ repeatingPayload })}
              disabled={d}
              hint="使用重复字节而非随机数据；可暴露硬件压缩（部分 WiFi AP）问题"
            />
            <SwitchField
              label="IPv4 DF 位"
              checked={o.dontFragment}
              onChange={(dontFragment) => onChange({ dontFragment })}
              disabled={d || o.protocol !== "udp"}
              hint="--dont-fragment：仅适用于 UDP over IPv4"
            />
            <SwitchField
              label="UDP 64 位计数"
              checked={o.udpCounters64bit}
              onChange={(udpCounters64bit) => onChange({ udpCounters64bit })}
              disabled={d}
              hint="长时/高速 UDP 测试防止计数器溢出"
            />
          </div>

          {/* 明确告知不支持的项及原因（而不是让用户以为功能缺失） */}
          <div className="mt-3 rounded-lg border border-border bg-muted/20 p-2.5">
            <p className="text-[11px] text-muted-foreground flex items-start gap-1.5">
              <Info className="size-3.5 shrink-0 mt-0.5" />
              <span>
                <span className="font-medium">
                  Windows 版 iperf3 不支持的选项
                </span>
                （已确认，不做无效参数生成）：拥塞控制算法（-C，仅
                Linux/FreeBSD）、公平队列限速（--fq-rate，仅 Linux）、SCTP
                相关（--sctp/-X/--nstreams）、IPv6 流标签（-L，仅
                Linux）、MPTCP（-m）、GSO/GRO（--gsro）、--bind-dev、--pacing-timer。
                认证类选项（--username/--rsa-public-key-path）需 OpenSSL
                构建与交互式口令输入， GUI 无法安全提供。
              </span>
            </p>
          </div>
        </CollapsibleContent>
      </Collapsible>

      {/* 恢复默认（局部操作，方便用户回到已知可用状态） */}
      <div className="flex justify-end">
        <button
          type="button"
          className="text-[11px] text-muted-foreground hover:text-foreground transition-colors"
          onClick={() => onChange({ ...IPERF3_DEFAULTS })}
          disabled={d}
        >
          恢复默认参数
        </button>
      </div>
    </div>
  );
}
