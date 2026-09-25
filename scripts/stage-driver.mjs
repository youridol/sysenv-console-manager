// 驱动产物 staging 脚本（构建前执行）
//
// 作用：把随包分发的第三方驱动产物复制到 src-tauri/resources/driver/：
//   - PawnIO.sys / PawnIO.cat / pawnio.inf（third_party/PawnIO/，官方 2.2.0 三件套，WHQL，
//     LHM sidecar tier-2 通道依赖的 ring0 驱动，SCM 服务方式部署）
//   - PawnIO-src/（官方 namazso/PawnIO tag 2.2.0 源码，GPL-2.0 源码提供义务，随包分发）
//
// 注 1：自研内核驱动 hwmon-driver 已从项目中移除，不再 stage 任何 hwmon 产物。
// 注 2：WinRing0x64.sys 不再随包分发（Microsoft 漏洞驱动阻断列表收录，且 LHM 0.9.6 的
//       唯一 ring0 后端是 PawnIO）。PawnIO 是本项目唯一的 ring0 驱动。third_party/ 下
//       的历史副本仅作署名/存档保留，不再 stage 进安装包；本脚本会清理 resources/driver/
//       中可能残留的旧副本，保证目标目录收敛为 PawnIO 三件套 + PawnIO-src/。
//
// 用法：
//   npm run stage:driver          # 产物存在则复制；缺失则报错退出（见下）
//   npm run tauri:build           # stage + tauri build（推荐发布流程）
//
// 前置：
//   third_party/PawnIO/ 随仓库分发（官方 2.2.0 三件套 + PawnIO-src 源码，哈希见
//   third_party/PawnIO/SHA256SUMS.txt；缺失时本脚本报错给出修复指引）
//
// 语义：PawnIO 为发布必选组件（LHM sidecar tier-2 温度/功耗通道依赖它）——产物缺失时
//       以非零退出码失败并给出修复指引，防止产出不含驱动、tier-2 永远不可用的"残废"安装包。

import { copyFileSync, cpSync, existsSync, mkdirSync, readdirSync, rmSync, readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(__dirname, "..");

/// 计算文件 SHA-256（幂等跳过判断用；读取失败向上抛错，不做静默容错）。
function sha256(filePath) {
  return createHash("sha256").update(readFileSync(filePath)).digest("hex");
}

// 第三方驱动目录（随仓库分发）
const thirdPartyDir = join(repoRoot, "third_party");
// PawnIO 来源目录（third_party/PawnIO/，含官方三件套副本与 tag 2.2.0 源码）
const pawnioDir = join(thirdPartyDir, "PawnIO");

// 需要 stage 到 src-tauri/resources/driver/ 的文件（文件名 → 源目录）
// PawnIO 为唯一 ring0 驱动；WinRing0x64.sys 已停止分发（见头部注 2）。
const FILES = [
  { name: "PawnIO.sys", srcDir: pawnioDir },
  { name: "PawnIO.cat", srcDir: pawnioDir },
  { name: "pawnio.inf", srcDir: pawnioDir },
];

// 已停止分发的历史驱动：显式从目标目录清除，避免旧版本残留导致继续随包分发
const RETIRED = ["WinRing0x64.sys"];

// PawnIO tag 2.2.0 源码目录（GPL-2.0 源码提供义务；third_party/PawnIO/PawnIO-src/ 为权威副本）
const pawnioSrcDir = join(pawnioDir, "PawnIO-src");
// PawnIO 源码关键文件（COPYING 为 GPL-2.0 全文，用于完整性校验）
const PAWNIO_SRC_MARKERS = ["COPYING", "CMakeLists.txt", "README.md"];

// 目标目录（tauri.conf.json bundle.resources 引用）
const destDir = join(__dirname, "..", "src-tauri", "resources", "driver");

const missing = FILES.filter((f) => !existsSync(join(f.srcDir, f.name)));
if (missing.length > 0) {
  console.error(`[stage:driver] 错误：驱动产物缺失：${missing.map((f) => f.name).join(", ")}`);
  missing.forEach((f) => {
    console.error(`[stage:driver]   ${f.name}: 缺失 third_party/PawnIO/ 官方三件套副本（PawnIO.sys/PawnIO.cat/pawnio.inf）`);
    console.error(`[stage:driver]     来源：本机 DriverStore（pawnio.inf_amd64_*/），哈希见 third_party/PawnIO/SHA256SUMS.txt`);
  });
  console.error(`[stage:driver] 若仅需开发（不发布），请直接使用 npm run tauri dev`);
  process.exit(1);
}

// 停止分发的历史驱动清理：即便源已不再 stage，旧构建残留的副本也必须移除，
// 否则它会继续被 tauri.conf.json / package-portable.mjs 打进安装包。
mkdirSync(destDir, { recursive: true });
for (const name of RETIRED) {
  const stale = join(destDir, name);
  if (existsSync(stale)) {
    rmSync(stale, { force: true });
    console.log(`[stage:driver] 已移除停止分发的历史驱动 ${name}（不再随包）`);
  }
}

// PawnIO 源码完整性检查（GPL-2.0 源码提供义务：COPYING/CMakeLists/README 必须齐全）
const missingSrcMarkers = PAWNIO_SRC_MARKERS.filter((m) => !existsSync(join(pawnioSrcDir, m)));
if (missingSrcMarkers.length > 0) {
  console.error(`[stage:driver] 错误：PawnIO 源码目录不完整：${missingSrcMarkers.join(", ")} 缺失`);
  console.error(`[stage:driver]   ${pawnioSrcDir}`);
  console.error(`[stage:driver]     请从官方仓库 namazso/PawnIO tag 2.2.0 重新解压源码到该目录（保留 PawnIO-2.2.0 内层结构，去掉顶层包装目录）`);
  process.exit(1);
}

mkdirSync(destDir, { recursive: true });
for (const f of FILES) {
  const src = join(f.srcDir, f.name);
  const dest = join(destDir, f.name);
  // 幂等保护：目标已存在且与源哈希一致时跳过复制。
  // 规避安全软件（如火绒 HipsDaemon）对 .sys 驱动文件长期写锁导致的 EBUSY，
  // 且保证重跑 stage 不产生无谓写 IO；目标缺失/不一致时仍强制复制（失败即报错退出）。
  if (existsSync(dest) && sha256(dest) === sha256(src)) {
    console.log(`[stage:driver] 已存在且哈希一致，跳过 ${f.name}`);
    continue;
  }
  copyFileSync(src, dest);
  console.log(`[stage:driver] 已复制 ${f.name} -> src-tauri/resources/driver/`);
}

// PawnIO 源码目录复制（先清空旧目录避免残留；GPL-2.0 源码随包义务）
const destPawnioSrc = join(destDir, "PawnIO-src");
rmSync(destPawnioSrc, { recursive: true, force: true });
cpSync(pawnioSrcDir, destPawnioSrc, { recursive: true });
console.log(`[stage:driver] 已复制 PawnIO-src/（源码，含 COPYING） -> src-tauri/resources/driver/PawnIO-src/`);

// 收敛校验：目标目录只允许 PawnIO 三件套 + PawnIO-src/（防止历史残留再次混入）
const allowed = new Set([...FILES.map((f) => f.name), "PawnIO-src"]);
const unexpected = readdirSync(destDir).filter((n) => !allowed.has(n));
if (unexpected.length > 0) {
  console.error(`[stage:driver] 错误：resources/driver/ 含非预期条目：${unexpected.join(", ")}`);
  console.error(`[stage:driver]   该目录应仅含 PawnIO 三件套 + PawnIO-src/；请检查 third_party 残留或手写文件`);
  process.exit(1);
}

console.log(`[stage:driver] 完成：${FILES.length} 个驱动文件 + PawnIO-src 源码已就绪，将随安装包分发`);
