// 交付物完整性标签规范化脚本（Windows）。
//
// ## 为什么需要这一步
//
// 本仓库工作区目录带 `Mandatory Label\Low Mandatory Level` 标签（实测：
// `icacls Y:\SysEnv-Console-Manager` → `Mandatory Label\Low Mandatory Level:(OI)(CI)(NW)`），
// 该标签**会被目录内所有新建文件继承**。Windows 创建进程时，若可执行文件带有完整性
// 标签，**该标签决定新进程的完整性级别**（决定性实测：同一个 whoami.exe，复制到本仓库
// 目录后以 Low 运行，复制到 %TEMP% 后以 High 运行；即便父进程是 High，由 Low 标签文件
// 启动的仍是 Low 进程。目录标签不覆盖文件标签，以文件标签为准）。
//
// NSIS 自解压安装程序运行时必须在系统 %TEMP%（中完整性对象）下释放 `$PLUGINSDIR`。
// 低完整性进程受「禁止向上写入（no-write-up）」约束，写中完整性目录被拒绝，于是安装
// 程序弹出：
//
//     NSIS Error
//     Error writing temporary file. Make sure your temp folder is valid.
//
// 该报错与安装包内容、NSIS 脚本、目标机器配置均无关，纯粹由**交付文件携带低完整性
// 标签**引起。用户从网络下载的正式发行包不含此标签（浏览器下载会重建文件），只有在本
// 仓库内就地构建才出现，因此必须在构建流程中显式规范化。
//
// ## 做法
//
// 对交付位置递归设置 `(OI)(CI)Medium`：目录本身获得 Medium 并向下继承，已存在的文件
// 被逐个改写为 Medium。`icacls /setintegritylevel` 只改 NTFS 安全描述符中的完整性 ACE，
// **不触碰文件数据**，故 SHA256 不受影响（实测：改标签前后同一文件 SHA256 完全一致）。
//
// 不修改工作区目录自身的 ACL 或标签——那属于放宽环境权限，且会让仓库继续产出带标签的
// 新文件。此处只处理**最终交付物**。
//
// 用法：
//   node scripts/fix-deliverable-integrity.mjs                 # 处理全部交付位置
//   node scripts/fix-deliverable-integrity.mjs dist_releases/v2.0.0
//   node scripts/fix-deliverable-integrity.mjs <任意目录或文件>...
//
// 幂等；非 Windows 平台直接跳过（本仓库仅支持 Windows）。

import { existsSync, readdirSync, statSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(__dirname, "..");

if (process.platform !== "win32") {
  console.log("[integrity] 非 Windows 平台，无需处理完整性标签，跳过");
  process.exit(0);
}

/**
 * 设置单个路径的完整性级别为 Medium。
 *
 * **目录与文件必须用不同语法，且目录的 `/T` 无法覆盖这一需求**：
 *   - 文件必须用不带继承标志的 `Medium`；文件上写 `(OI)(CI)Medium` 会被 icacls
 *     **静默忽略**（仍打印 `Successfully processed`，标签却不变，实测确认）。
 *   - 目录需加 `(OI)(CI)Medium` 让**将来**新建的文件继承 Medium。
 *
 * 关于 `/T` 对「已存在文件」的作用（2026-09-24 复核，ADR-0001 D8）：
 *   - 本注释原先断言 `/T` **不会**改写目录下已存在文件的标签。实测**相反**：
 *     对只含 `z.exe`（Low）的目录执行 `icacls <dir> /setintegritylevel "(OI)(CI)Medium" /T`
 *     后，`z.exe` 由 `Low Mandatory Level:(I)(NW)` 变为 `Medium Mandatory Level:(I)(NW)`。
 *   - 因此下面的逐文件循环**不是**正确性所必需，它的实际作用是把「继承 ACE」
 *     替换为「显式 ACE」，使标签不随父目录变化而回退。
 *   - 该循环予以保留（更稳健、且无副作用），但不应再以「/T 无效」为由解释它。
 *   - CHANGELOG v2.0.1 的第 5 条实测结论与本次复测不符，保留原文不改写历史，
 *     以此处为准。
 */
function setMedium(target) {
  const isDir = statSync(target).isDirectory();
  const args = isDir
    ? [target, "/setintegritylevel", "(OI)(CI)Medium", "/T"]
    : [target, "/setintegritylevel", "Medium"];
  try {
    execFileSync("icacls", args, { stdio: "pipe", windowsHide: true });
    return { ok: true };
  } catch (e) {
    const out =
      `${e.stdout ?? ""}${e.stderr ?? ""}`.trim() || String(e.message ?? e);
    return { ok: false, detail: out };
  }
}

/**
 * 规范化一个路径：目录本身设 Medium 并向下继承，同时**逐个文件**设置 Medium。
 *
 * 返回被处理的文件数，供日志核对。
 */
function normalize(target) {
  const files = [];
  const collect = (p) => {
    if (statSync(p).isDirectory()) {
      for (const n of readdirSync(p)) {
        const c = join(p, n);
        try {
          collect(c);
        } catch {
          /* 读取失败的子项跳过，交由后续校验暴露 */
        }
      }
    } else {
      files.push(p);
    }
  };

  // 目录先设继承标志（覆盖将来新建文件）
  if (statSync(target).isDirectory()) {
    const r = setMedium(target);
    if (!r.ok) return { ok: false, detail: r.detail, count: 0 };
    collect(target);
  } else {
    files.push(target);
  }

  // 再逐个文件显式设置（覆盖已存在文件）
  let count = 0;
  for (const f of files) {
    const r = setMedium(f);
    if (!r.ok) return { ok: false, detail: `${f}: ${r.detail}`, count };
    count++;
  }
  return { ok: true, count };
}

/** 读取路径当前的 Mandatory Label，用于修复前后核对。 */
function readLabel(target) {
  try {
    const out = execFileSync("icacls", [target], {
      stdio: "pipe",
      windowsHide: true,
    }).toString();
    const line = out
      .split(/\r?\n/)
      .map((s) => s.trim())
      .find((s) => s.startsWith("Mandatory Label"));
    return line ?? "<无完整性标签>";
  } catch {
    return "<读取失败>";
  }
}

/**
 * 收集交付位置。
 *
 * 需覆盖全部三条用户可获取安装包/便携 exe 的路径，缺一即可能把带标签的产物交出去：
 *   1. `dist_releases/v<版本>/`    —— AGENTS.md 规定的唯一权威发行归档
 *   2. `src-tauri/target/release/bundle/nsis/` —— tauri build 的 NSIS 输出
 *   3. `src-tauri/target/release/*.exe`        —— 便携版 exe 源
 */
function defaultTargets() {
  const list = [];

  const releasesRoot = join(repoRoot, "dist_releases");
  if (existsSync(releasesRoot)) {
    for (const n of readdirSync(releasesRoot)) {
      if (!/^v\d+\.\d+\.\d+$/.test(n)) continue;
      const p = join(releasesRoot, n);
      if (statSync(p).isDirectory()) list.push(p);
    }
  }

  const nsisDir = join(
    repoRoot,
    "src-tauri",
    "target",
    "release",
    "bundle",
    "nsis",
  );
  if (existsSync(nsisDir)) list.push(nsisDir);

  const relDir = join(repoRoot, "src-tauri", "target", "release");
  if (existsSync(relDir)) {
    for (const n of readdirSync(relDir)) {
      if (!n.endsWith(".exe")) continue;
      const p = join(relDir, n);
      if (statSync(p).isFile()) list.push(p);
    }
  }

  return list;
}

const argv = process.argv.slice(2);
const targets =
  argv.length > 0
    ? argv.map((p) => (p.match(/^[A-Za-z]:[\\/]|^\//) ? p : join(repoRoot, p)))
    : defaultTargets();

if (targets.length === 0) {
  console.error(
    "[integrity] 未找到任何交付位置。请先执行 `npm run tauri:build` 与 `npm run package:portable`，" +
      "或显式传入路径。",
  );
  process.exit(1);
}

let failed = 0;
/**
 * 递归校验目录/文件下每个可执行交付物都已是 Medium 或更高完整性。
 *
 * 仅凭 icacls 退出码不足以下结论：文件上误用 `(OI)(CI)` 语法时 icacls 仍返回成功但标签
 * 不变。这里回读实际标签，凡是仍为 Low 的一律判失败，避免把无效修复当成已修复。
 */
function verifyNoLowIntegrity(target) {
  const offenders = [];
  const check = (p) => {
    const label = readLabel(p);
    if (/Low Mandatory Level/i.test(label))
      offenders.push(`${relative(repoRoot, p) || p}: ${label}`);
  };
  if (statSync(target).isDirectory()) {
    check(target);
    const walk = (dir) => {
      for (const n of readdirSync(dir)) {
        const p = join(dir, n);
        let st;
        try {
          st = statSync(p);
        } catch {
          continue;
        }
        if (st.isDirectory()) walk(p);
        else if (/\.(exe|dll)$/i.test(n)) check(p);
      }
    };
    walk(target);
  } else {
    check(target);
  }
  return offenders;
}

for (const dir of targets) {
  if (!existsSync(dir)) {
    failed++;
    console.error(`[integrity] ✗ 路径不存在: ${dir}`);
    continue;
  }
  const label = relative(repoRoot, dir) || dir;
  const before = readLabel(dir);

  const r = normalize(dir);
  if (!r.ok) {
    failed++;
    console.error(`[integrity] ✗ ${label} 处理失败: ${r.detail}`);
    continue;
  }

  const after = readLabel(dir);
  const changed = before !== after;
  console.log(
    `[integrity] ${changed ? "已规范化" : "已符合"} ${label}（逐个处理 ${r.count} 个文件）`,
  );
  console.log(`            修复前: ${before}`);
  console.log(`            修复后: ${after}`);

  // 回读校验：确认没有遗漏仍带 Low 标签的交付文件。
  // 必须做这一步——icacls 在"语法被忽略"时也返回成功，只看退出码会漏判。
  const offenders = verifyNoLowIntegrity(dir);
  if (offenders.length > 0) {
    failed++;
    console.error(`[integrity] ✗ ${label} 仍存在低完整性交付文件:`);
    for (const o of offenders) console.error(`              ${o}`);
  }
}

if (failed > 0) {
  console.error(`[integrity] ${failed} 个路径处理失败或校验未通过`);
  process.exit(1);
}
console.log(
  "[integrity] 完成：交付物完整性标签已规范化为 Medium（安装包可正常在 %TEMP% 释放临时文件）",
);
