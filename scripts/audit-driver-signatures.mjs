// 驱动签名审计脚本（构建前门禁，用户需求：检查排除所有过期的驱动和签名）
//
// 作用：对随包分发的全部驱动/签名相关文件执行 Authenticode 审计，
//       输出签名状态报告（签名者 / 证书有效期 / 时间戳 / 有效性），
//       并给出「可加载 / 证书过期但有有效时间戳 / 签名无效不可分发」三级判定。
//
// 判定规则（对齐 Windows 内核驱动签名策略）：
//   - Valid + 有效时间戳       → OK（签名时证书有效，证书到期不影响加载）
//   - Valid + 证书已过期无时间戳 → WARN（部分系统/安全软件可能拦截）
//   - NotSigned / HashMismatch / 其他 → FAIL（不可分发，构建门禁阻断）
//
// 用法：
//   node scripts/audit-driver-signatures.mjs          # 审计 third_party + resources/driver
//   node scripts/audit-driver-signatures.mjs --fail-on-warn   # 含 WARN 也非零退出（严格门禁）

import { existsSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(__dirname, "..");

const failOnWarn = process.argv.includes("--fail-on-warn");

/// Windows PowerShell 5.1 的模块搜索路径（只保留 5.1 自带/用户模块目录）。
///
/// 为什么必须收敛：本脚本用 `powershell.exe`（Windows PowerShell **5.1**）取签名，
/// 但 Node 进程通常继承自 PowerShell 7（pwsh）启动的环境，其 `PSModulePath`
/// 含 PS7 的模块目录（`...\PowerShell\7\Modules`、`...\PowerShell\Modules`）。
/// 5.1 若去加载 PS7 模块，会因扩展类型数据（`types.ps1xml`）成员重复而
/// **加载 `Microsoft.PowerShell.Security` 失败**，`Get-AuthenticodeSignature`
/// 随即不可用 → 构建门禁误报失败（实测：`Import-Module` 报
/// `FormatXmlUpdateException: The member AccessToString is already present`）。
///
/// 修复：把 `PSModulePath` 收敛为仅含 `WindowsPowerShell` 的条目
/// （5.1 标准路径：`%USERPROFILE%\Documents\WindowsPowerShell\Modules`、
/// `%ProgramFiles%\WindowsPowerShell\Modules`、
/// `%WINDIR%\system32\WindowsPowerShell\v1.0\Modules`）。
/// 未命中任何条目时返回 `undefined`，交给子进程沿用继承值（保持旧行为，
/// 不引入新的失败模式——例如非标准安装位置）。
function windowsPowerShellModulePath() {
  const sep = ";";
  const inherited = (process.env.PSModulePath || "")
    .split(sep)
    .map((p) => p.trim())
    .filter(Boolean);
  const only51 = inherited.filter((p) => /WindowsPowerShell/i.test(p));
  return only51.length > 0 ? only51.join(sep) : undefined;
}

/// 子进程环境：仅在需要时覆写 PSModulePath（见上）
const POWERSHELL_ENV = (() => {
  const psModulePath = windowsPowerShellModulePath();
  if (psModulePath === undefined) return process.env;
  return { ...process.env, PSModulePath: psModulePath };
})();

function collectTargets() {
  const targets = [];
  const resDir = join(repoRoot, "src-tauri", "resources", "driver");
  const thirdDir = join(repoRoot, "third_party");
  const pawnioDir = join(thirdDir, "PawnIO");
  const candidates = [
    { name: "PawnIO.sys（LHM tier-2 唯一 ring0 驱动，WHQL）", path: join(resDir, "PawnIO.sys"), kind: "kernel" },
    { name: "PawnIO.cat（LHM 目录文件）", path: join(resDir, "PawnIO.cat"), kind: "kernel" },
  ];
  const fallbacks = {
    "PawnIO.sys（LHM tier-2 唯一 ring0 驱动，WHQL）": join(pawnioDir, "PawnIO.sys"),
    "PawnIO.cat（LHM 目录文件）": join(pawnioDir, "PawnIO.cat"),
  };
  for (const c of candidates) {
    if (existsSync(c.path)) {
      targets.push(c);
    } else if (fallbacks[c.name] && existsSync(fallbacks[c.name])) {
      targets.push({ name: c.name + "（third_party 源）", path: fallbacks[c.name], kind: c.kind });
    }
  }
  return targets;
}

function querySignatures(files) {
  // 逐文件调用 PowerShell（简单可靠；2 个 PawnIO 文件约 1s，可接受）
  // 注：WinRing0x64.sys 已停止分发，故不再纳入审计（本仓库仅保留 PawnIO ring0 驱动）。
  return files.map((f) => {
    // PowerShell 单引号字符串内的单引号需写成两个单引号；JS 侧直接输出 '' 即可
    const esc = f.path.replace(/'/g, "''");
    const script =
      "$s = Get-AuthenticodeSignature '" + esc + "'\n" +
      "[PSCustomObject]@{\n" +
      "  status = $s.Status.ToString()\n" +
      "  signer = if ($s.SignerCertificate) { $s.SignerCertificate.Subject } else { '' }\n" +
      "  notBefore = if ($s.SignerCertificate) { $s.SignerCertificate.NotBefore.ToString('yyyy-MM-dd') } else { '' }\n" +
      "  notAfter = if ($s.SignerCertificate) { $s.SignerCertificate.NotAfter.ToString('yyyy-MM-dd') } else { '' }\n" +
      "  tsSubject = if ($s.TimeStamperCertificate) { $s.TimeStamperCertificate.Subject } else { '' }\n" +
      "  tsNotAfter = if ($s.TimeStamperCertificate) { $s.TimeStamperCertificate.NotAfter.ToString('yyyy-MM-dd') } else { '' }\n" +
      "} | ConvertTo-Json -Compress";
    const out = execFileSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", script], {
      encoding: "utf8",
      maxBuffer: 16 * 1024 * 1024,
      env: POWERSHELL_ENV,
    });
    return JSON.parse(out.trim());
  });
}

function judge(item, sig) {
  const status = String(sig.status || "");
  const now = new Date();
  const notAfter = sig.notAfter ? new Date(sig.notAfter + "T23:59:59") : null;
  const tsNotAfter = sig.tsNotAfter ? new Date(sig.tsNotAfter + "T23:59:59") : null;
  const certExpired = notAfter !== null && notAfter < now;
  const tsValid = tsNotAfter !== null && tsNotAfter >= now;
  const signer = String(sig.signer || "");
  // 测试签名（WDRLocalTestCert 等自签测试证书）：合法开发通道，仅测试签名模式可加载
  const isTestSigned = /WDRLocalTestCert|Test|测试/i.test(signer) && status !== "Valid";

  if (status === "Valid") {
    if (!certExpired) return { verdict: "OK", detail: "签名有效，证书在有效期内" };
    if (tsValid) {
      return {
        verdict: "OK",
        detail: "证书已于 " + sig.notAfter + " 到期但带有效时间戳（" + sig.tsNotAfter + " 到期）——按 Windows 驱动签名策略可正常加载",
      };
    }
    return {
      verdict: "WARN",
      detail: "证书已于 " + sig.notAfter + " 到期且无有效时间戳——部分系统/安全软件可能拒绝加载",
    };
  }
  // 测试签名（内核驱动）：WARN 而非 FAIL（合法 L0 通道，随包分发为测试签名模式用户准备）
  if (isTestSigned && item.kind === "kernel") {
    return {
      verdict: "WARN",
      detail: "测试签名（" + signer + "）——仅测试签名模式（bcdedit /set testsigning on）可加载，Secure Boot 下自动回退其他通道；随包分发为设计行为",
    };
  }
  // 此处曾有 `item.kind === "user"` 分支：`collectTargets()` 只构造 `kind: "kernel"`
  // 的目标（PawnIO.sys / PawnIO.cat），全仓无 `kind: "user"` 生产者，该分支不可达。
  // 删除以保证「审计覆盖全部随包驱动」的语义与实现一致（ADR-0001 D15）。
  // 将来若纳入用户态组件，需同时恢复本分支与 collectTargets 的候选列表。
  return { verdict: "FAIL", detail: "签名状态 " + status + "——不可分发" };
}

function main() {
  const targets = collectTargets();
  if (targets.length === 0) {
    console.error("[audit] 未找到任何驱动文件（resources/driver 与 third_party 均为空）");
    process.exit(1);
  }
  console.log("[audit] 驱动签名审计开始（" + targets.length + " 个文件）");
  console.log("=".repeat(78));
  let failed = 0;
  let warned = 0;
  const list = querySignatures(targets);
  for (let i = 0; i < targets.length; i++) {
    const t = targets[i];
    // querySignatures 按入参顺序返回，直接按下标取即可。
    // 原实现为 `list.find((s) => String(s.idx) === String(i)) ?? list[i]`，
    // 但返回对象里从来没有 `idx` 字段（只有 status/signer/notBefore/notAfter/
    // tsSubject/tsNotAfter），该 find 恒不命中，每次都退化为 list[i]
    // ——一次空跑的 O(n) 扫描（ADR-0001 D15）。
    const sig = list[i];
    const r = judge(t, sig);
    if (r.verdict === "FAIL") failed++;
    if (r.verdict === "WARN") warned++;
    const mark = r.verdict === "OK" ? "OK" : r.verdict === "WARN" ? "WARN" : "FAIL";
    console.log(mark + " [" + r.verdict + "] " + t.name);
    console.log("    文件: " + t.path);
    console.log("    状态: " + sig.status + " | 签名者: " + (sig.signer || "无"));
    console.log("    证书: " + (sig.notBefore || "-") + " ~ " + (sig.notAfter || "-") + " | 时间戳: " + (sig.tsSubject ? sig.tsNotAfter + " 到期" : "无"));
    console.log("    判定: " + r.detail);
  }
  console.log("=".repeat(78));
  console.log("[audit] 结果: OK=" + (targets.length - failed - warned) + " WARN=" + warned + " FAIL=" + failed);
  if (failed > 0) {
    console.error("[audit] 存在签名无效/缺失文件，禁止分发（请检查 stage 产物与 third_party）");
    process.exit(1);
  }
  if (warned > 0 && failOnWarn) {
    console.error("[audit] 存在过期无时间戳签名（--fail-on-warn），构建门禁阻断");
    process.exit(1);
  }
  console.log("[audit] 全部通过，可分发");
}

main();