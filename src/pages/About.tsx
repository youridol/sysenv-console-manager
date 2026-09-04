import { useState } from "react";
import { Monitor, Cpu, Globe, Layers, ExternalLink } from "lucide-react";
import { useVersion } from "../hooks/useVersion";
import {
  Card,
  CardHeader,
  CardTitle,
  CardContent,
} from "../components/ui/card";
import { Separator } from "../components/ui/separator";
import { Badge } from "../components/ui/badge";
import { Avatar, AvatarFallback } from "../components/ui/avatar";
import {
  Accordion,
  AccordionItem,
  AccordionTrigger,
  AccordionContent,
} from "@/components/ui/accordion";

/** 更新日志（CHANGELOG.md 原文，构建时嵌入，随版本发布） */
import changelog from "../../CHANGELOG.md?raw";

/** 更新日志条目数（用于折叠态摘要：匹配 “## [v” 标题行） */
const CHANGELOG_COUNT = (changelog.match(/^## \[v/gm) ?? []).length;

/** 单个第三方组件/库的许可条目 */
type LicenseItem = {
  /** 组件/库名称 */
  name: string;
  /** 许可证标识（SPDX） */
  license: string;
  /** 上游项目主页（可为 null，如协议标准类） */
  url: string | null;
  /** 附加说明（分发范围、版本号等） */
  note?: string;
};

/** 许可分组 */
type LicenseGroup = {
  /** 分组标题 */
  title: string;
  items: LicenseItem[];
};

const LICENSE_GROUPS: LicenseGroup[] = [
  {
    title: "核心框架",
    items: [
      {
        name: "Tauri 2.x（含官方插件）",
        license: "MIT / Apache-2.0",
        url: "https://github.com/tauri-apps/tauri",
        note: "含 tauri-build、tauri-plugin-single-instance / store / dialog / shell / fs",
      },
      {
        name: "React 19 / react-dom",
        license: "MIT",
        url: "https://react.dev",
      },
      { name: "shadcn/ui", license: "MIT", url: "https://ui.shadcn.com" },
      {
        name: "Tailwind CSS 4（含 @tailwindcss/vite）",
        license: "MIT",
        url: "https://tailwindcss.com",
      },
    ],
  },
  {
    title: "前端依赖（npm）",
    items: [
      {
        name: "@tauri-apps/api",
        license: "MIT / Apache-2.0",
        url: "https://github.com/tauri-apps/tauri",
      },
      {
        name: "Radix UI（radix-ui + @radix-ui/react-*）",
        license: "MIT",
        url: "https://www.radix-ui.com",
        note: "react-slot / react-tabs / react-tooltip / react-separator / react-label / react-toggle 等子包",
      },
      { name: "lucide-react", license: "ISC", url: "https://lucide.dev" },
      { name: "clsx", license: "MIT", url: "https://github.com/lukeed/clsx" },
      {
        name: "class-variance-authority",
        license: "Apache-2.0",
        url: "https://cva.style",
      },
      {
        name: "tailwind-merge",
        license: "MIT",
        url: "https://github.com/dcastil/tailwind-merge",
      },
      {
        name: "embla-carousel-react",
        license: "MIT",
        url: "https://www.embla-carousel.com",
      },
      {
        name: "react-router-dom",
        license: "MIT",
        url: "https://reactrouter.com",
      },
      { name: "Recharts", license: "MIT", url: "https://recharts.org" },
      { name: "cmdk", license: "MIT", url: "https://github.com/pacocoursey/cmdk" },
      {
        name: "next-themes",
        license: "MIT",
        url: "https://github.com/pacocoursey/next-themes",
        note: "暗色主题切换（shadcn/ui 组件栈）",
      },
      { name: "sonner", license: "MIT", url: "https://sonner.emilkowal.ski" },
    ],
  },
  {
    title: "开发与构建工具（devDependencies）",
    items: [
      {
        name: "Vite",
        license: "MIT",
        url: "https://vitejs.dev",
      },
      {
        name: "TypeScript",
        license: "Apache-2.0",
        url: "https://www.typescriptlang.org",
      },
      {
        name: "@vitejs/plugin-react",
        license: "MIT",
        url: "https://github.com/vitejs/vite-plugin-react",
      },
      {
        name: "ESLint 全家（eslint + typescript-eslint + plugins + config-prettier）",
        license: "MIT",
        url: "https://eslint.org",
        note: "eslint / eslint-config-prettier / eslint-plugin-react-hooks / eslint-plugin-react-refresh / typescript-eslint",
      },
      {
        name: "Prettier",
        license: "MIT",
        url: "https://prettier.io",
      },
      {
        name: "Husky + lint-staged（pre-commit 钩子）",
        license: "MIT",
        url: "https://typicode.github.io/husky",
      },
    ],
  },
  {
    title: "Rust 后端依赖（crates.io）",
    items: [
      {
        name: "serde / serde_json",
        license: "MIT / Apache-2.0",
        url: "https://github.com/serde-rs/serde",
      },
      { name: "tokio", license: "MIT", url: "https://tokio.rs" },
      {
        name: "chrono",
        license: "MIT / Apache-2.0",
        url: "https://github.com/chronotope/chrono",
      },
      {
        name: "sysinfo",
        license: "MIT",
        url: "https://github.com/GuillaumeGomez/sysinfo",
      },
      {
        name: "parking_lot",
        license: "MIT / Apache-2.0",
        url: "https://github.com/Amanieu/parking_lot",
      },
      {
        name: "lazy_static",
        license: "MIT / Apache-2.0",
        url: "https://github.com/rust-lang-nursery/lazy-static.rs",
      },
      {
        name: "winreg",
        license: "MIT",
        url: "https://github.com/gentoo90/winreg-rs",
      },
      {
        name: "surge-ping",
        license: "MIT",
        url: "https://github.com/kolbma/surge-ping",
      },
      {
        name: "trust-dns-resolver",
        license: "MIT / Apache-2.0",
        url: "https://github.com/hickory-dns/hickory-dns",
        note: "DNS 解析，现已更名 hickory-dns",
      },
      {
        name: "rand",
        license: "MIT / Apache-2.0",
        url: "https://github.com/rust-random/rand",
      },
      {
        name: "encoding_rs",
        license: "MIT / Apache-2.0",
        url: "https://github.com/hsivonen/encoding_rs",
        note: "WHATWG 编码标准实现（P1-P13 纯 Rust 采集链路）",
      },
      {
        name: "tiny_http",
        license: "Apache-2.0 / MIT",
        url: "https://github.com/tiny-http/tiny-http",
        note: "内置本地 JSON API 服务（127.0.0.1:17860）",
      },
      {
        name: "log",
        license: "MIT / Apache-2.0",
        url: "https://github.com/rust-lang/log",
      },
      {
        name: "socket2",
        license: "MIT / Apache-2.0",
        url: "https://github.com/rust-lang/socket2",
        note: "DHCP 探测 UDP SO_REUSEADDR 绑定",
      },
      {
        name: "thiserror（1.x / 2.x）",
        license: "MIT / Apache-2.0",
        url: "https://github.com/dtolnay/thiserror",
      },
      {
        name: "ureq",
        license: "MIT / Apache-2.0",
        url: "https://github.com/algesten/ureq",
        note: "LHM sidecar HTTP 客户端 / 公网 IP 查询",
      },
      {
        name: "wmi",
        license: "MIT / Apache-2.0",
        url: "https://github.com/ohadravid/wmi-rs",
        note: "WMI 查询（激活状态 / 补丁信息，datasource）",
      },
      {
        name: "windows-sys（0.45~0.61 多版本）",
        license: "MIT / Apache-2.0",
        url: "https://github.com/microsoft/windows-rs",
        note: "Win32 API 绑定（iphlpapi / advapi32 / powrprof 等）",
      },
    ],
  },
  {
    title: "运行时与打包",
    items: [
      {
        name: "Microsoft WebView2 Runtime",
        license: "微软专有（允许再分发）",
        url: "https://developer.microsoft.com/microsoft-edge/webview2",
        note: "Tauri 渲染运行时",
      },
      {
        name: "NSIS（安装包生成器）",
        license: "zlib",
        url: "https://nsis.sourceforge.io",
        note: "生成的 setup.exe 内含 NSIS stub",
      },
      {
        name: "WinRing0（OpenLibSys）",
        license: "BSD-2-Clause",
        url: "https://openlibsys.org",
        note: "resources/driver 分发的内核驱动（L1 降级通道：SMN 温度 / RAPL 功耗读取）；本机版本为 hiyohiyo 重编译变体（third_party/），以二进制形式分发未修改",
      },
      {
        name: "LibreHardwareMonitorLib",
        license: "MPL-2.0",
        url: "https://github.com/LibreHardwareMonitor/LibreHardwareMonitor",
        note: "sidecar-lhm 温度/功耗数据源（优先通道）；库仅运行于独立 sidecar 进程（进程隔离，不链接入 SECM 主程序），完整源码与许可文本随 sidecar-lhm/ 分发",
      },
      {
        name: "PawnIO（内核驱动）",
        license: "GPL-2.0-or-later（附例外条款）",
        url: "https://github.com/namazso/PawnIO",
        note: "随包分发 v2.2.0（WHQL 签名，resources/driver/PawnIO.sys，SCM 服务方式自动部署，服务名 PawnIO）；LHM 内嵌 ring0 模块为 PawnIO.Modules 0.1.6（LGPL-2.1，经 IOCTL 加载）；GPL-2.0 全文/例外条款与源码随包（resources/driver/PawnIO-src/、resources/lhm/LICENSE-GPL-2.0-PawnIO.txt）",
      },
      {
        name: ".NET 8 Runtime（自包含发布）",
        license: "MIT",
        url: "https://dotnet.microsoft.com",
        note: "LHM sidecar 以 self-contained 形态随包分发（resources/lhm/publish/，coreclr/hostfxr 等运行时 DLL），目标机无需预装 .NET",
      },
      {
        name: "HidSharp",
        license: "Apache-2.0",
        url: "https://github.com/IntergatedCircuits/HidSharp",
        note: "LHM sidecar 传递依赖（HID 设备访问），随 sidecar 分发",
      },
      {
        name: "DiskInfoToolkit / RAMSPDToolkit-NDD",
        license: "MPL-2.0",
        url: "https://github.com/LibreHardwareMonitor/LibreHardwareMonitor",
        note: "LHM 0.9.6 的磁盘/内存 SPD 传感 toolkit（传递依赖，仅随 sidecar 分发）",
      },
    ],
  },
  {
    title: "间接依赖（copyleft 特征声明）",
    items: [
      {
        name: "cssparser / selectors / dtoa-short / option-ext",
        license: "MPL-2.0",
        url: "https://github.com/servo/servo",
        note: "Tauri 解析链（tauri-utils → dom_query）引入",
      },
      {
        name: "ICU4X 系列（Unicode 数据处理）",
        license: "Unicode-3.0",
        url: "https://github.com/unicode-org/icu4x",
        note: "icu_* / zerovec / litemap / tinystr 等 Unicode 联盟宽松许可",
      },
      {
        name: "lightningcss（含平台二进制）",
        license: "MPL-2.0",
        url: "https://github.com/parcel-bundler/lightningcss",
        note: "Tailwind CSS 4 的 CSS 解析/转换引擎（@tailwindcss/vite → lightningcss，npm 传递依赖）",
      },
      {
        name: "caniuse-lite（浏览器兼容数据）",
        license: "CC-BY-4.0",
        url: "https://github.com/browserslist/caniuse-lite",
        note: "Browserslist / Vite 构建链的数据文件（npm 传递依赖）",
      },
      {
        name: "Unlicense / CC0 系列（公有领域）",
        license: "Unlicense / CC0-1.0",
        url: "https://unlicense.org",
        note: "aho-corasick / byteorder / byteorder-lite / memchr / same-file / walkdir / winapi-util（Unlicense OR MIT）、dunce（CC0-1.0 OR MIT-0 OR Apache-2.0），均属公有领域许可",
      },
      {
        name: "r-efi（EFI 定义）",
        license: "MIT / Apache-2.0 / LGPL-2.1-or-later（多许可择一）",
        url: "https://github.com/r-efi/r-efi",
        note: "getrandom 的 WASI 目标可选依赖，Windows 构建不编译；SPDX 多许可中取 MIT 即可，无 copyleft 传染",
      },
    ],
  },
];

/** 许可条目总数（折叠态摘要显示，随 LICENSE_GROUPS 自动更新） */
const LICENSE_TOTAL = LICENSE_GROUPS.reduce((n, g) => n + g.items.length, 0);

const BUILD_INFO = [
  { icon: Cpu, label: "后端", value: "Rust / Tauri 2.x" },
  { icon: Globe, label: "前端", value: "React 19 + TypeScript + Vite" },
  { icon: Layers, label: "UI", value: "shadcn/ui + Tailwind CSS 4" },
  { icon: Monitor, label: "平台", value: "Windows 10/11 (x64)" },
];

/** 赞助卡片（文件内私有组件：图片 + 说明文案；图片加载失败时显示占位图标） */
function SponsorCard({
  src,
  alt,
  title,
  description,
}: {
  src: string;
  alt: string;
  title: string;
  description: string;
}) {
  const [imgError, setImgError] = useState(false);
  return (
    <Card className="min-h-[300px]">
      <CardHeader>
        <CardTitle className="text-sm">{title}</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col items-center justify-center gap-3 p-4">
        {imgError ? (
          <div className="w-full max-h-36 sm:max-h-44 flex-1 min-h-24 flex items-center justify-center rounded-lg bg-muted/30">
            <Monitor className="size-8 text-muted-foreground/40" aria-hidden />
          </div>
        ) : (
          <img
            src={src}
            alt={alt}
            onError={() => setImgError(true)}
            className="w-full max-h-36 sm:max-h-44 object-contain rounded-lg"
          />
        )}
        <p className="text-xs text-muted-foreground text-center leading-relaxed">
          {description}
        </p>
      </CardContent>
    </Card>
  );
}

export default function About() {
  const version = useVersion("0.1.0");

  return (
    <div className="flex flex-col gap-6 md:gap-8 max-w-5xl mx-auto">
      {/* ── Hero：复用 Card 基元（rounded-xl border bg-card），装饰渐变层纯装饰 aria-hidden ── */}
      <div className="relative animate-fade-in">
        <Card className="relative overflow-hidden">
          <div
            className="absolute inset-0 bg-gradient-to-br from-brand-blue/10 via-brand-purple/5 to-transparent"
            aria-hidden
          />
          <div className="relative flex flex-col sm:flex-row items-start sm:items-center gap-4 sm:gap-5 p-5 sm:p-6">
            <Avatar
              size="lg"
              className="size-12 sm:size-14 rounded-xl shadow-lg shadow-brand-blue/20 shrink-0"
            >
              <AvatarFallback className="bg-gradient-to-br from-brand-blue to-brand-purple text-white">
                <Monitor className="size-6 sm:size-7" />
              </AvatarFallback>
            </Avatar>
            <div className="min-w-0">
              <h1 className="text-2xl font-bold tracking-tight truncate">
                SysEnv Console Manager
              </h1>
              <p className="text-xs sm:text-sm text-muted-foreground mt-1.5 flex items-center gap-2 flex-wrap">
                <Badge
                  variant="secondary"
                  className="font-mono text-[11px] px-1.5 py-0"
                >
                  v{version}
                </Badge>
                Windows 10/11 系统环境管理工具
              </p>
            </div>
          </div>
        </Card>
      </div>

      {/* ── 简介 + 赞助关于 + 构建信息 ── */}
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4 animate-fade-in-up stagger-1">
        <Card className="sm:col-span-2 lg:col-span-1 min-h-[300px]">
          <CardHeader>
            <CardTitle className="text-sm">项目简介</CardTitle>
          </CardHeader>
          <CardContent>
            <p className="text-sm text-muted-foreground leading-relaxed">
              SECM 是面向 Windows 10/11 的系统环境管理桌面工具，集硬件监控、
              清理优化、网络诊断和系统设置于一体。本项目遵循 MIT 许可证开源。
            </p>
            {/* 构建信息（并入项目简介） */}
            <div className="mt-4 pt-4 border-t border-border space-y-2">
              {BUILD_INFO.map(({ icon: Icon, label, value }) => (
                <div key={label} className="flex items-center gap-2.5 text-sm">
                  <Icon className="size-3.5 text-muted-foreground shrink-0" />
                  <span className="text-muted-foreground shrink-0">
                    {label}
                  </span>
                  <span className="font-medium truncate">{value}</span>
                </div>
              ))}
            </div>
          </CardContent>
        </Card>

        <SponsorCard
          src="/zsm.png"
          alt="赞助赞赏"
          title="赞助赞赏"
          description="如果本项目对您有帮助，欢迎请作者喝杯咖啡 ☕"
        />

        <SponsorCard
          src="/dy.png"
          alt="关注抖音"
          title="关注抖音"
          description="扫码关注，获取更多工具与教程"
        />
      </div>

      <Separator />

      {/* ── 更新日志 + 第三方许可（默认折叠） ── */}
      <div className="animate-fade-in-up stagger-2">
        <Accordion type="single" collapsible>
          {/* 更新日志 */}
          <AccordionItem value="changelog">
            <AccordionTrigger className="py-1.5 mb-2 text-sm font-semibold text-muted-foreground uppercase tracking-wider hover:no-underline hover:text-foreground">
              <span className="flex items-baseline gap-2">
                更新日志
                <span className="text-xs font-normal normal-case tracking-normal text-muted-foreground/60">
                  {CHANGELOG_COUNT} 个版本
                </span>
              </span>
            </AccordionTrigger>
            <AccordionContent className="pb-0">
              <Card>
                <CardContent className="p-0">
                  <pre className="whitespace-pre-wrap font-mono text-[11px] leading-relaxed text-muted-foreground p-4 max-h-[60vh] overflow-y-auto">
                    {changelog}
                  </pre>
                </CardContent>
              </Card>
            </AccordionContent>
          </AccordionItem>

          {/* 第三方许可 */}
          <AccordionItem value="licenses">
            <AccordionTrigger className="py-1.5 mb-2 text-sm font-semibold text-muted-foreground uppercase tracking-wider hover:no-underline hover:text-foreground">
              <span className="flex items-baseline gap-2">
                第三方许可
                <span className="text-xs font-normal normal-case tracking-normal text-muted-foreground/60">
                  {LICENSE_TOTAL} 项 · {LICENSE_GROUPS.length} 组
                </span>
              </span>
            </AccordionTrigger>
            <AccordionContent className="pb-0">
              <Card>
                <CardContent className="p-0 divide-y divide-border">
                  {LICENSE_GROUPS.map((group) => (
                    <div key={group.title}>
                      <h3 className="px-4 pt-3.5 pb-2 text-xs font-semibold text-muted-foreground/70 uppercase tracking-wider">
                        {group.title}
                      </h3>
                      <div className="divide-y divide-border/60">
                        {group.items.map((item) => (
                          <div
                            key={item.name}
                            className="flex items-start justify-between gap-3 px-4 py-2.5 hover:bg-muted/50 transition-colors"
                          >
                            <div className="min-w-0">
                              <span className="text-sm font-medium">
                                {item.name}
                              </span>
                              <span className="text-xs text-muted-foreground ml-2">
                                {item.license}
                              </span>
                              {item.note && (
                                <span className="block text-[11px] text-muted-foreground/70 mt-0.5">
                                  {item.note}
                                </span>
                              )}
                            </div>
                            {item.url && (
                              <a
                                href={item.url}
                                target="_blank"
                                rel="noopener noreferrer"
                                className="text-muted-foreground hover:text-foreground transition-colors shrink-0 mt-0.5"
                              >
                                <ExternalLink className="size-3.5" />
                              </a>
                            )}
                          </div>
                        ))}
                      </div>
                    </div>
                  ))}
                </CardContent>
              </Card>
              <p className="text-[11px] text-muted-foreground/60 mt-3 px-1">
                以上为直接依赖及主要间接依赖清单。完整锁定清单见 Cargo.lock /
                package-lock.json。
              </p>
            </AccordionContent>
          </AccordionItem>
        </Accordion>
      </div>
    </div>
  );
}
