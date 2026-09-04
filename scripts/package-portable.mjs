// 便携版 zip 打包脚本（Tauri 2.11 Windows 仅支持 msi/nsis，zip 便携包由此脚本生成）
//
// 结构（与 NSIS 安装布局一致，运行时 resolve 逻辑可命中）：
//   <portable>/SysEnv Console Manager.exe
//   <portable>/lhm/publish/**        （LhmSidecar.exe + 运行时，resource_dir()/lhm/publish）
//   <portable>/lhm/source/** + 许可    （源码随包义务）
//   <portable>/resources/driver/**   （WinRing0/PawnIO 驱动，exe 同目录布局）
//
// 用法：
//   node scripts/package-portable.mjs            # 输出 dist_releases/vX.Y.Z/ 便携 zip

import { cpSync, existsSync, mkdirSync, rmSync, readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(__dirname, "..");
const pkg = JSON.parse(readFileSync(join(repoRoot, "package.json"), "utf8"));
const version = pkg.version;

const exeName = "SysEnv Console Manager.exe";
const exePath = join(repoRoot, "src-tauri", "target", "release", "sysenv-console-manager.exe");
const srcResDriver = join(repoRoot, "src-tauri", "resources", "driver");
const srcResLhm = join(repoRoot, "src-tauri", "resources", "lhm");

const missing = [];
if (!existsSync(exePath)) missing.push("release exe（先执行 tauri build）");
if (!existsSync(srcResDriver)) missing.push("resources/driver（先执行 stage:driver）");
if (!existsSync(join(srcResLhm, "publish"))) missing.push("resources/lhm/publish（先执行 stage:lhm）");
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

const zipPath = join(outDir, "SysEnv Console Manager_v" + version + "_portable.zip");
if (existsSync(zipPath)) rmSync(zipPath);

// .NET ZipFile.CreateFromDirectory：以 tmp 内容为 zip 根（不保留父目录名）
const ps = "Add-Type -AssemblyName System.IO.Compression.FileSystem; "
  + "[System.IO.Compression.ZipFile]::CreateFromDirectory(\'" + tmp + "\', \'" + zipPath + "\')";
execFileSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", ps], { stdio: "inherit" });

rmSync(tmp, { recursive: true, force: true });
console.log("[portable] 便携包已生成: " + zipPath);
console.log("[portable] 结构: exe + resources/driver + lhm/publish + lhm/source + 许可，解压即用（绿色免安装）");
