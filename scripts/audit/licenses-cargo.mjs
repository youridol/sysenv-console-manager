// Rust crate 许可审计：解析 Cargo.lock，从本地 registry 缓存读各 crate 的 license
// 用法: node scripts/audit/licenses-cargo.mjs [--copyleft] [--top]
//   --copyleft: 仅输出 copyleft 特征许可（GPL/LGPL/MPL/AGPL/EPL/CC-*）
//   --top:      仅输出 src-tauri 与 datasource 的直接依赖
import fs from "node:fs";
import path from "node:path";
import os from "node:os";

const CARGO_LOCK = "src-tauri/Cargo.lock";
const lock = parseLock(fs.readFileSync(CARGO_LOCK, "utf8"));

function parseLock(text) {
  const pkgs = [];
  const lines = text.split("\n");
  let i = 0;
  while (i < lines.length) {
    if (lines[i].trim() === "[[package]]") {
      const pkg = {};
      i++;
      // Cargo.lock v4 格式：字段行无缩进，以 `key = value` 形式出现
      while (i < lines.length && /^[a-zA-Z0-9_]+\s*=/.test(lines[i].trim())) {
        const m = lines[i].trim().match(/^([a-zA-Z0-9_]+)\s*=\s*"?(.+?)"?$/);
        if (m) pkg[m[1]] = m[2];
        i++;
      }
      pkgs.push(pkg);
    } else {
      i++;
    }
  }
  return pkgs;
}

// 直接依赖（src-tauri + datasource Cargo.toml）
function directDeps() {
  const set = new Set();
  for (const f of ["src-tauri/Cargo.toml", "datasource/Cargo.toml"]) {
    const text = fs.readFileSync(f, "utf8");
    const depSection = text.match(/\[dependencies\]([\s\S]*?)(?=\n\[|\n\[dev-dependencies\])/);
    if (!depSection) continue;
    for (const m of depSection[1].matchAll(/^([a-zA-Z0-9_-]+)\s*=/gm)) {
      set.add(m[1]);
    }
  }
  return set;
}
const direct = directDeps();

// 从 registry 缓存读 license
const registryDir = path.join(
  os.homedir(),
  ".cargo/registry/src",
);
let cacheDirs = [];
try {
  cacheDirs = fs.readdirSync(registryDir).map((d) => path.join(registryDir, d));
} catch {
  /* 无缓存 */
}

function findLicense(pkgName, ver) {
  for (const dir of cacheDirs) {
    const p = path.join(dir, `${pkgName}-${ver}`, "Cargo.toml");
    if (fs.existsSync(p)) {
      const text = fs.readFileSync(p, "utf8");
      const m = text.match(/^license\s*=\s*"([^"]+)"/m);
      return m ? m[1] : "（无 license 字段）";
    }
  }
  return "（本地无缓存，待查 crates.io）";
}

const rows = lock
  .filter((p) => p.name)
  .map((p) => ({
    name: p.name,
    version: p.version,
    license: findLicense(p.name, p.version),
    direct: direct.has(p.name),
  }));

const copyleft = /GPL|LGPL|MPL|AGPL|EPL|CC-BY|CC0|Unlicense/i;
const onlyCopyleft = process.argv.includes("--copyleft");
const onlyTop = process.argv.includes("--top");

let out = rows.filter((r) => {
  if (onlyTop && !r.direct) return false;
  if (onlyCopyleft && !copyleft.test(r.license)) return false;
  return true;
});
out.sort((a, b) => a.name.localeCompare(b.name));

console.log(`共 ${rows.length} 个 crate；输出 ${out.length} 个`);
for (const r of out) {
  console.log(`${r.direct ? "DIRECT" : "trans "} ${r.name}@${r.version}  ${r.license}`);
}
