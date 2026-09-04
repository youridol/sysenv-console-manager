// npm 依赖许可审计：从 package-lock.json 读取全部包（含传递依赖）的 license 字段
// 用法: node scripts/audit/licenses-npm.mjs [--copyleft] [--top]
//   --copyleft: 仅输出 copyleft 特征许可（GPL/LGPL/MPL/AGPL/EPL/CC-*）
//   --top:      仅输出 package.json 直接依赖
import fs from "node:fs";

const lock = JSON.parse(fs.readFileSync("package-lock.json", "utf8"));
const rootPkg = JSON.parse(fs.readFileSync("package.json", "utf8"));
const direct = new Set([
  ...Object.keys(rootPkg.dependencies || {}),
  ...Object.keys(rootPkg.devDependencies || {}),
]);

const rows = [];
for (const [path, p] of Object.entries(lock.packages || {})) {
  if (!path) continue; // 根包
  const name = p.name || path.split("node_modules/").pop();
  if (!name) continue;
  const lic = Array.isArray(p.license)
    ? p.license.map((l) => l.type || l).join(" / ")
    : typeof p.license === "object" && p.license
      ? p.license.type
      : (p.license ?? "未知");
  rows.push({
    name,
    version: p.version ?? "?",
    license: String(lic),
    direct: direct.has(name),
  });
}

const copyleft = /GPL|LGPL|MPL|AGPL|EPL|CC-BY|CC0|Unlicense/i;
const onlyCopyleft = process.argv.includes("--copyleft");
const onlyTop = process.argv.includes("--top");

let out = rows.filter((r) => {
  if (onlyTop && !r.direct) return false;
  if (onlyCopyleft && !copyleft.test(r.license)) return false;
  return true;
});
out.sort((a, b) => a.name.localeCompare(b.name));

console.log(`共 ${rows.length} 个包；输出 ${out.length} 个`);
for (const r of out) {
  console.log(
    `${r.direct ? "DIRECT " : "trans  "} ${r.name}@${r.version}  ${r.license}`,
  );
}
