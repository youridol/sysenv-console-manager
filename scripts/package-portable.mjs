// 便携版 zip 打包脚本（Tauri 2.11 Windows 仅支持 msi/nsis，zip 便携包由此脚本生成）
//
// 结构（与 NSIS 安装布局一致，运行时 resolve 逻辑可命中）：
//   <portable>/SysEnv Console Manager.exe
//   <portable>/lhm/publish/**        （LhmSidecar.exe + 运行时，resource_dir()/lhm/publish）
//   <portable>/lhm/source/** + 许可    （源码随包义务）
//   <portable>/resources/driver/**   （PawnIO ring0 驱动 + 源码，exe 同目录布局）
//
// 同时产出发行归档 `dist_releases/v<版本>/` 的全部组成（README「发行归档约定」）：
//   安装版 setup.exe、便携版 exe、便携 zip、resources/（driver/lhm/iperf3）、SHA256SUMS.txt
//
// 用法：
//   node scripts/package-portable.mjs            # 输出 dist_releases/vX.Y.Z/ 归档
//
// 前置：先执行 `npm run tauri:build`（产出 release exe 与 NSIS setup.exe）
//       以及 `npm run stage:driver` / `stage:iperf3` / `stage:lhm`。

import {
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(__dirname, "..");
const pkg = JSON.parse(readFileSync(join(repoRoot, "package.json"), "utf8"));
const version = pkg.version;

const exeName = "SysEnv Console Manager.exe";
const exePath = join(
  repoRoot,
  "src-tauri",
  "target",
  "release",
  "sysenv-console-manager.exe",
);
const setupPath = join(
  repoRoot,
  "src-tauri",
  "target",
  "release",
  "bundle",
  "nsis",
  `SysEnv Console Manager_${version}_x64-setup.exe`,
);
const srcResDriver = join(repoRoot, "src-tauri", "resources", "driver");
const srcResLhm = join(repoRoot, "src-tauri", "resources", "lhm");
const srcResIperf3 = join(repoRoot, "src-tauri", "resources", "iperf3");

const missing = [];
if (!existsSync(exePath)) missing.push("release exe（先执行 tauri build）");
if (!existsSync(setupPath))
  missing.push(`NSIS setup.exe（先执行 tauri build；期望 ${setupPath}）`);
// 驱动必须**逐项**校验：只判目录存在会放行空目录——stage-driver 在任何失败分支
// 之前就 mkdirSync 了目标目录，因此失败会留下空目录，于是便携包静默缺失 ring0 驱动，
// 正是 stage-driver 头部注释声称要防止的「残废安装包」（ADR-0001 D8）。
const driverFiles = ["PawnIO.sys", "PawnIO.cat", "pawnio.inf"];
const missingDriverFiles = driverFiles.filter(
  (f) => !existsSync(join(srcResDriver, f)),
);
if (missingDriverFiles.length > 0) {
  missing.push(
    `resources/driver 缺少 ${missingDriverFiles.join(", ")}（先执行 stage:driver）`,
  );
}
if (!existsSync(join(srcResLhm, "publish")))
  missing.push("resources/lhm/publish（先执行 stage:lhm）");
// iperf3 是可选的「用户自装替代」能力：缺失不应让整个便携包失败，
// 但必须显式告警，避免用户拿到手才发现网络工具里没有 iperf3。
if (!existsSync(join(srcResIperf3, "iperf3.exe"))) {
  console.warn(
    "[portable] 警告：resources/iperf3/iperf3.exe 缺失（先执行 npm run stage:iperf3）",
  );
  console.warn(
    "[portable]         便携包将不含随包 iperf3，网络工具需用户自装或改用系统 PATH",
  );
}
if (missing.length > 0) {
  console.error("[portable] 前置产物缺失: " + missing.join(", "));
  process.exit(1);
}

const outDir = join(repoRoot, "dist_releases", "v" + version);
const tmp = join(outDir, "portable-tmp");
rmSync(tmp, { recursive: true, force: true });
mkdirSync(join(tmp, "resources"), { recursive: true });

cpSync(exePath, join(tmp, exeName));
cpSync(srcResDriver, join(tmp, "resources", "driver"), { recursive: true });
cpSync(srcResLhm, join(tmp, "lhm"), { recursive: true });
// iperf3 放在 resources/iperf3 —— 与 iperf.rs 的「exe 同目录 resources/iperf3」
// 便携版布局一致（resolve_iperf3_exe 第 4 段），解压即用。
if (existsSync(srcResIperf3)) {
  cpSync(srcResIperf3, join(tmp, "resources", "iperf3"), { recursive: true });
}

const zipPath = join(
  outDir,
  "SysEnv Console Manager_v" + version + "_portable.zip",
);
if (existsSync(zipPath)) rmSync(zipPath);

// .NET ZipFile.CreateFromDirectory：以 tmp 内容为 zip 根（不保留父目录名）
// PowerShell 单引号字符串内的单引号需写成两个单引号，否则路径含 ' 时脚本语法错误
const psQuote = (p) => "'" + String(p).replace(/'/g, "''") + "'";
const ps =
  "Add-Type -AssemblyName System.IO.Compression.FileSystem; " +
  "[System.IO.Compression.ZipFile]::CreateFromDirectory(" +
  psQuote(tmp) +
  ", " +
  psQuote(zipPath) +
  ")";
execFileSync(
  "powershell.exe",
  ["-NoProfile", "-NonInteractive", "-Command", ps],
  { stdio: "inherit" },
);

rmSync(tmp, { recursive: true, force: true });

// ---- 发行归档：补齐 setup.exe / 便携 exe / resources/ 与校验和 ----
// 这三个顶层副本此前靠手工放置，导致归档内容不可复现；此处纳入脚本。
const archivedExe = join(outDir, `sysenv-console-manager_v${version}.exe`);
const archivedSetup = join(
  outDir,
  `SysEnv Console Manager_${version}_x64-setup.exe`,
);
const archivedRes = join(outDir, "resources");
cpSync(exePath, archivedExe);
cpSync(setupPath, archivedSetup);
rmSync(archivedRes, { recursive: true, force: true });
mkdirSync(archivedRes, { recursive: true });
cpSync(srcResDriver, join(archivedRes, "driver"), { recursive: true });
cpSync(srcResLhm, join(archivedRes, "lhm"), { recursive: true });
if (existsSync(srcResIperf3)) {
  cpSync(srcResIperf3, join(archivedRes, "iperf3"), { recursive: true });
}

// 规范化交付物完整性标签（必须在计算 SHA256SUMS 之前）。
// 本仓库目录带 Low Mandatory Level 标签且会被新文件继承；带该标签的 setup.exe 启动后
// 安装程序为低完整性进程，无法在 %TEMP% 释放 $PLUGINSDIR，用户会看到
// 「Error writing temporary file. Make sure your temp folder is valid.」（详见 CHANGELOG v2.0.1）。
// 放在校验和之前，确保 SHA256SUMS.txt 记录的是交付物的最终状态。
// 该步骤只改 NTFS 安全描述符中的完整性 ACE、不改文件数据，但顺序上仍以"先定稿后度量"为准。
const integrityScript = join(
  repoRoot,
  "scripts",
  "fix-deliverable-integrity.mjs",
);
execFileSync(process.execPath, [integrityScript], { stdio: "inherit" });

// 校验和：**仅覆盖三个顶层交付物**（setup.exe / 便携 zip / 便携 exe）。
// 注意：zip 内并不存在 SHA256SUMS.txt，resources/ 也没有逐文件哈希——原注释声称
// 「逐文件哈希见 zip 内 SHA256SUMS」与事实不符（ADR-0001 D8），已删去该说法。
// 如需校验随包资源，请对 resources/ 目录自行逐文件计算。
const sha256 = (p) =>
  createHash("sha256").update(readFileSync(p)).digest("hex");
const sums = [archivedSetup, zipPath, archivedExe]
  .map((p) => `${sha256(p)}  ${basename(p)}`)
  .join("\n");
writeFileSync(join(outDir, "SHA256SUMS.txt"), sums + "\n", "utf8");

console.log("[portable] 便携包已生成: " + zipPath);
console.log(
  "[portable] 结构: exe + resources/driver + resources/iperf3 + lhm/publish + lhm/source + 许可，解压即用（绿色免安装）",
);
console.log("[portable] 发行归档已就绪: " + outDir);
console.log(
  "[portable] 含 setup.exe / 便携 exe / 便携 zip / resources/ / SHA256SUMS.txt",
);
