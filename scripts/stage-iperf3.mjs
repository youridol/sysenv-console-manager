// iperf3 二进制 staging 脚本（构建前执行）
//
// 作用：把随包 iperf3 二进制与许可文本复制到 src-tauri/resources/iperf3/，
//       随安装包 / 便携包分发，用户无需自装即可使用「网络工具 → iperf3」。
//
// 用法：
//   npm run stage:iperf3
//
// 前置（一次性）：
//   从 https://github.com/ar51an/iperf3-win-builds/releases 下载
//   iperf-<版本>-win64.zip，解出 iperf3.exe + cygwin1.dll，放入 third_party/iperf3/。
//
// ── 为什么必须校验 SHA256 ────────────────────────────────────────────────────
// 上游 ESnet 明确不发布 Windows 二进制（docs/obtaining.rst），Windows 版由第三方
// 构建。既然无法用官方签名背书，就用**内容哈希钉死**：校验和不符即中止构建，
// 避免"某天源被替换/下载到损坏文件"却静默打进安装包。
//
// ── 为什么必须带上 cygwin1.dll ───────────────────────────────────────────────
// Windows 版 iperf3 是 **Cygwin 链接**构建（POSIX 兼容层），iperf3.exe 单独存在
// 时无法启动（会报缺 DLL）。两件套必须一起分发，否则用户看到的是"iperf3 启动失败"。
//
// ── 许可 ────────────────────────────────────────────────────────────────────
// iperf3 本体为 BSD-3-Clause（含 cJSON/MIT、net.c/MIT 等第三方条款），
// 二进制分发要求随附版权声明与免责声明；Cygwin 运行时（cygwin1.dll）为 LGPL/GPL
// 混合许可，随包附全文与来源说明。许可全文置于 src-tauri/resources/iperf3/ 并写进
// tauri.conf.json 的 bundle.resources，随安装包一并落地。

import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(__dirname, "..");

/// 源目录（与 third_party/PawnIO 同范式：原始产物入库，stage 产物不入库）
const srcDir = join(repoRoot, "third_party", "iperf3");
/// 目标目录（tauri.conf.json bundle.resources 引用 resources/iperf3 → iperf3）
const destDir = join(repoRoot, "src-tauri", "resources", "iperf3");

/// 版本与来源（与 third_party/iperf3/README.md 保持一致）
const IPERF3_VERSION = "3.21";
const SOURCE_NOTE =
  "https://github.com/ar51an/iperf3-win-builds/releases/download/3.21/iperf-3.21-win64.zip";

/// 必需文件：iperf3.exe 与其 Cygwin 运行时依赖必须成对存在。
/// `sha256` 为 null 表示"以仓库内当前文件为准"（首次落库时由 --pin 生成）。
const REQUIRED = [
  { name: "iperf3.exe", why: "iperf3 主程序（Cygwin 链接构建）" },
  { name: "cygwin1.dll", why: "Cygwin 运行时：iperf3.exe 缺它无法启动" },
];

/// 随包许可文本（BSD-3-Clause 要求二进制分发附版权与免责声明；
/// Cygwin 运行时为 LGPL-3.0-or-later，要求随附 LGPLv3 与 GPLv3 全文）。
const LICENSES = [
  {
    name: "LICENSE-IPERF3-BSD-3-Clause.txt",
    why: "iperf3 本体许可（BSD-3-Clause + 内嵌第三方条款）",
  },
  {
    name: "LICENSE-LGPL-3.0.txt",
    why: "cygwin1.dll 的 LGPL-3.0-or-later 许可全文",
  },
  {
    name: "LICENSE-GPL-3.0.txt",
    why: "LGPLv3 是对 GPLv3 的附加许可，需同时随附 GPLv3 全文",
  },
  {
    name: "THIRD-PARTY-NOTICES-IPERF3.txt",
    why: "来源、版本、SHA256、链接例外与合规说明",
  },
];

const SHA_FILE = join(srcDir, "SHA256SUMS.txt");

/// 计算文件 SHA256（小写十六进制）。
function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

/// 解析 SHA256SUMS.txt（格式：`<hash>  <filename>`，与 coreutils 一致）。
function readExpectedHashes() {
  if (!existsSync(SHA_FILE)) return {};
  const out = {};
  for (const line of readFileSync(SHA_FILE, "utf8").split(/\r?\n/)) {
    const t = line.trim();
    if (!t || t.startsWith("#")) continue;
    const m = t.match(/^([0-9a-fA-F]{64})\s+\*?(.+)$/);
    if (m) out[m[2].trim()] = m[1].toLowerCase();
  }
  return out;
}

// ── 前置校验：源产物必须齐全 ────────────────────────────────────────────────
if (!existsSync(srcDir)) {
  console.error(`[stage:iperf3] 错误：源目录不存在：${srcDir}`);
  console.error(`[stage:iperf3]   请从 ${SOURCE_NOTE} 下载并放入 iperf3.exe + cygwin1.dll`);
  console.error(`[stage:iperf3]   详见 third_party/iperf3/README.md`);
  process.exit(1);
}

const missing = REQUIRED.filter((f) => !existsSync(join(srcDir, f.name)));
if (missing.length > 0) {
  console.error(`[stage:iperf3] 错误：随包产物缺失：`);
  for (const f of missing) console.error(`[stage:iperf3]   ${f.name} — ${f.why}`);
  console.error(`[stage:iperf3]   来源：${SOURCE_NOTE}`);
  process.exit(1);
}

// ── SHA256 校验：不符即中止，绝不静默打包 ──────────────────────────────────
const expected = readExpectedHashes();
if (Object.keys(expected).length === 0) {
  console.error(`[stage:iperf3] 错误：缺少 ${SHA_FILE}`);
  console.error(`[stage:iperf3]   为避免把来源不明的二进制打进安装包，必须有校验和清单。`);
  // 本脚本不解析命令行参数（全文无 process.argv），原先提示的 `--pin` 并不存在，
  // 按提示操作只会再次得到同一条错误（ADR-0001 D8）。改为给出可实际执行的生成方式。
  console.error(`[stage:iperf3]   生成方式：对 third_party/iperf3/ 下每个文件计算 SHA-256，`);
  console.error(`[stage:iperf3]   按 "<hash>  <文件名>" 每行一条写入 ${SHA_FILE}（顺序不限，文件名需与上表一致）。`);
  console.error(`[stage:iperf3]   另：文件头注释里提到的 \`sha256: null\` 字段已不存在，勿再引用。`);
  process.exit(1);
}

let hashMismatch = false;
for (const f of REQUIRED) {
  const actual = sha256(join(srcDir, f.name));
  const want = expected[f.name];
  if (!want) {
    console.error(`[stage:iperf3] 错误：SHA256SUMS.txt 未登记 ${f.name}`);
    hashMismatch = true;
    continue;
  }
  if (actual !== want) {
    console.error(`[stage:iperf3] 错误：${f.name} SHA256 不符`);
    console.error(`[stage:iperf3]   期望：${want}`);
    console.error(`[stage:iperf3]   实际：${actual}`);
    hashMismatch = true;
  }
}
if (hashMismatch) {
  console.error(`[stage:iperf3] 校验失败：产物与清单不一致，已中止（不会产出含未知二进制的安装包）`);
  process.exit(1);
}
console.log(`[stage:iperf3] SHA256 校验通过（${REQUIRED.length} 个文件）`);

// ── 复制产物 ────────────────────────────────────────────────────────────────
rmSync(destDir, { recursive: true, force: true });
mkdirSync(destDir, { recursive: true });

for (const f of REQUIRED) {
  copyFileSync(join(srcDir, f.name), join(destDir, f.name));
}
console.log(`[stage:iperf3] 已复制 iperf3.exe + cygwin1.dll -> src-tauri/resources/iperf3/`);

// ── 许可文本（缺一即失败：BSD-3-Clause 的二进制分发义务）────────────────────
const missingLicenses = LICENSES.filter((l) => !existsSync(join(srcDir, l.name)));
if (missingLicenses.length > 0) {
  console.error(`[stage:iperf3] 错误：许可文本缺失（BSD-3-Clause 二进制分发义务）：`);
  for (const l of missingLicenses) console.error(`[stage:iperf3]   ${l.name} — ${l.why}`);
  console.error(`[stage:iperf3]   许可全文来源见 third_party/iperf3/README.md`);
  process.exit(1);
}
for (const l of LICENSES) {
  copyFileSync(join(srcDir, l.name), join(destDir, l.name));
  console.log(`[stage:iperf3] 已复制 ${l.name}`);
}

// ── 就绪校验：复制后逐项断言（不"复制完就算成功"）──────────────────────────
const notCopied = REQUIRED.filter((f) => !existsSync(join(destDir, f.name)));
if (notCopied.length > 0) {
  console.error(`[stage:iperf3] 错误：复制后仍有文件缺失：${notCopied.map((f) => f.name).join(", ")}`);
  process.exit(1);
}

console.log(
  `[stage:iperf3] 完成：iperf3 ${IPERF3_VERSION}（含 Cygwin 运行时与许可文本）已就绪，将随安装包分发`,
);
