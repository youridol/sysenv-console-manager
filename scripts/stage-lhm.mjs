// LHM sidecar 产物 staging 脚本（构建前执行）
//
// 作用：把 sidecar 发布产物复制到 src-tauri/resources/lhm/，随安装包分发：
//   - publish/（dotnet publish self-contained win-x64 产物：LhmSidecar.exe + 运行时 DLL）
//   - LICENSE-MPL-2.0.txt / PawnIo-COPYING-LGPL-2.1.txt / THIRD-PARTY-NOTICES-LHM.txt（许可合规）
//   - LICENSE-GPL-2.0-PawnIO.txt（随包 PawnIO 2.2.0 驱动的 GPL-2.0 全文 + 例外条款，许可合规）
//   - source/（sidecar 源码随包，MPL-2.0/LGPL-2.1 源码提供义务）
//
// 用法：
//   npm run stage:lhm            # publish 产物存在则复制；缺失则报错退出
//
// 前置：
//   1. 首次：npm run lhm:publish  # 执行 dotnet publish 生成 self-contained 产物
//   2. 二次发布：直接 stage（产物已在 sidecar-lhm/bin/Release/net8.0/win-x64/publish/）
//
// 语义：sidecar 为 CPU 温度主数据源（temp_source="lhm"）——产物缺失时以非零退出码
//       失败并给出修复指引，防止产出不含 sidecar 的"残废"安装包。

import { copyFileSync, cpSync, existsSync, mkdirSync, readdirSync, rmSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(__dirname, "..");

/// 递归统计目录下的普通文件数（用于日志与体积可观测性）。
function countFiles(dir) {
  let n = 0;
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      n += countFiles(full);
    } else {
      n += 1;
    }
  }
  return n;
}

// sidecar dotnet publish 产物目录（--self-contained true -r win-x64）
const publishDir = join(
  repoRoot,
  "sidecar-lhm",
  "bin",
  "Release",
  "net8.0",
  "win-x64",
  "publish",
);
// sidecar 源码目录（随包分发，MPL-2.0 源码义务）
const sourceDir = join(repoRoot, "sidecar-lhm");
// 许可文本源（与 sidecar 源码同仓库管理）
const licenseDir = join(repoRoot, "sidecar-lhm", "licenses");
// 目标目录（tauri.conf.json bundle.resources map 引用：resources/lhm/... → lhm/...）
const destRoot = join(repoRoot, "src-tauri", "resources", "lhm");
const destPublish = join(destRoot, "publish");
const destSource = join(destRoot, "source");

// 必须存在的核心产物
const REQUIRED = [
  { name: "LhmSidecar.exe", srcDir: publishDir },
  { name: "LhmSidecar.dll", srcDir: publishDir },
  { name: "LibreHardwareMonitorLib.dll", srcDir: publishDir },
  { name: "LhmSidecar.runtimeconfig.json", srcDir: publishDir },
];

const missing = REQUIRED.filter((f) => !existsSync(join(f.srcDir, f.name)));
if (missing.length > 0) {
  console.error(`[stage:lhm] 错误：sidecar 产物缺失：${missing.map((f) => f.name).join(", ")}`);
  console.error(`[stage:lhm]   请先执行 npm run lhm:publish（dotnet publish -c Release -r win-x64 --self-contained true）`);
  console.error(`[stage:lhm]   产物目录：${publishDir}`);
  process.exit(1);
}

// 1. publish/（整目录复制；先清空旧产物避免残留 DLL 混入）
rmSync(destPublish, { recursive: true, force: true });
mkdirSync(destPublish, { recursive: true });
cpSync(publishDir, destPublish, { recursive: true });
console.log(`[stage:lhm] 已复制 publish/（${existsSync(join(destPublish, "LhmSidecar.exe")) ? "LhmSidecar.exe 确认存在" : "异常"}） -> src-tauri/resources/lhm/publish/`);

// 2. 许可文本（MPL-2.0 / PawnIO LGPL-2.1 / PawnIO GPL-2.0+例外 / THIRD-PARTY-NOTICES）
const licenses = [
  { name: "LICENSE-MPL-2.0.txt", srcDir: licenseDir },
  { name: "PawnIo-COPYING-LGPL-2.1.txt", srcDir: licenseDir },
  { name: "LICENSE-GPL-2.0-PawnIO.txt", srcDir: licenseDir },
  { name: "THIRD-PARTY-NOTICES-LHM.txt", srcDir: licenseDir },
];
for (const l of licenses) {
  const src = join(l.srcDir, l.name);
  if (!existsSync(src)) {
    console.error(`[stage:lhm] 错误：许可文本缺失：${src}`);
    console.error(`[stage:lhm]   请先执行 npm run lhm:licenses 获取 MPL-2.0/LGPL-2.1 全文与 THIRD-PARTY-NOTICES，并确认 LICENSE-GPL-2.0-PawnIO.txt 已生成（来源：官方 namazso/PawnIO tag 2.2.0 源码 COPYING + README License 段）`);
    process.exit(1);
  }
  copyFileSync(src, join(destRoot, l.name));
  console.log(`[stage:lhm] 已复制 ${l.name}`);
}

// 3. source/（sidecar 源码随包，MPL-2.0 / LGPL-2.1 源码提供义务）
//
//    递归复制整个 sidecar-lhm/ 树，排除：
//      - bin/      构建产物（体积巨大且非源码；publish/ 已单独复制）
//      - obj/      NuGet/MSBuild 中间产物
//      - licenses/ 许可全文已由上方第 2 步复制到 destRoot，避免重复占体积
//
//    递归而非白名单的原因：契约 v3 起源码分为多个文件（Program.cs / Snapshot.cs /
//    Hardware/*.cs），白名单漏掉任一文件都会破坏源码提供义务。CONTRACT-v3.md 是本
//    移植的权威接口契约，随源码一并分发（便于收件人对照 Rust 侧与 C# 侧实现）。
const SOURCE_EXCLUDE_DIRS = new Set(["bin", "obj", "licenses"]);

/// node:fs cpSync 的 filter：返回 false 的条目（及其子树）被跳过。
/// 判断路径的**任意一段**是否为排除目录——不仅仅是顶层：`sidecar-lhm/tests/
/// LhmSidecar.Tests/bin/` 与 `obj/` 是嵌套在子项目下的构建产物，同样必须排除
/// （只判顶层会把这些 DLL/testhost 打进源码包）。Windows 与 POSIX 分隔符都处理。
function isSourceIncluded(src) {
  const rel = relative(sourceDir, src);
  if (rel === "") return true; // 根目录本身
  const segments = rel.split(/[\\/]+/);
  return !segments.some((seg) => SOURCE_EXCLUDE_DIRS.has(seg));
}

rmSync(destSource, { recursive: true, force: true });
mkdirSync(destSource, { recursive: true });
cpSync(sourceDir, destSource, { recursive: true, filter: isSourceIncluded });

// 源码提供义务的完整性校验：核心源文件必须真实落地（递归复制后逐项断言，
// 而不是"复制完就算成功"——filter 写错时会静默少文件）。
const REQUIRED_SOURCE = ["Program.cs", "sidecar-lhm.csproj", "CONTRACT-v3.md"];
const missingSource = REQUIRED_SOURCE.filter((n) => !existsSync(join(destSource, n)));
if (missingSource.length > 0) {
  console.error(`[stage:lhm] 错误：sidecar 源码复制不完整，缺失：${missingSource.join(", ")}`);
  console.error(`[stage:lhm]   源目录：${sourceDir}`);
  process.exit(1);
}
// 排除项必须确实被排除（否则构建产物会被打进安装包）——
// 递归扫描整个 source/ 树，任一深度都不允许出现 bin/obj/licenses 目录。
const leakedDirs = [];
(function scan(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      const full = join(dir, entry.name);
      if (SOURCE_EXCLUDE_DIRS.has(entry.name)) {
        leakedDirs.push(full);
      } else {
        scan(full);
      }
    }
  }
})(destSource);
if (leakedDirs.length > 0) {
  console.error(`[stage:lhm] 错误：source/ 含应被排除的目录（任意深度均不允许）：`);
  leakedDirs.forEach((d) => console.error(`[stage:lhm]   ${d}`));
  process.exit(1);
}
const sourceFileCount = countFiles(destSource);
console.log(
  `[stage:lhm] 已复制 sidecar 源码（${sourceFileCount} 个文件，已排除 bin/obj/licenses）-> src-tauri/resources/lhm/source/`,
);
console.log(`[stage:lhm] 完成：sidecar 产物已就绪，将随安装包分发`);
