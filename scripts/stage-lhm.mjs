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

import { copyFileSync, cpSync, existsSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(__dirname, "..");

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

// 3. source/（sidecar 源码随包；只复制源文件，排除 bin/obj 构建产物）
rmSync(destSource, { recursive: true, force: true });
mkdirSync(destSource, { recursive: true });
for (const name of ["Program.cs", "sidecar-lhm.csproj"]) {
  const src = join(sourceDir, name);
  if (existsSync(src)) {
    copyFileSync(src, join(destSource, name));
  }
}
// README 说明（含源码获取途径与许可说明）
const readme = join(sourceDir, "README.md");
if (existsSync(readme)) {
  copyFileSync(readme, join(destSource, "README.md"));
}
console.log(`[stage:lhm] 已复制 sidecar 源码 -> src-tauri/resources/lhm/source/`);
console.log(`[stage:lhm] 完成：sidecar 产物已就绪，将随安装包分发`);
