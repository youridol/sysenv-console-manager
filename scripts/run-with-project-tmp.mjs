// 构建工具临时目录隔离 —— 在启动前端构建工具前，把 TEMP/TMP 指向项目内目录。
//
// 背景（2026-09-23 实测）：本仓库工作区目录带 `Mandatory Label\Low Mandatory Level`
// 标签，从该目录启动的进程以低完整性运行。esbuild（Vite 用它转译 vite.config.ts 与
// 各模块）会在系统 `%TEMP%` 下创建临时文件，随后尝试删除；实测该删除被拒绝：
//
//   [vite:esbuild-transpile] remove C:\Users\...\Temp\esbuild-<hash>: Access is denied.
//   → npm run build 退出 1
//
// 把 `TEMP`/`TMP` 指向项目内 `build/tmp/` 后同一命令退出 0（构建产物一致），
// 说明失败源于临时目录越界，不是源码问题。esbuild 与 Vite 均通过 `os.tmpdir()`
// 解析临时目录，而 `os.tmpdir()` 在 Windows 上读取 `TEMP`/`TMP`——即通过官方环境变量
// 机制重定向，无需修改系统权限或关闭沙箱。
//
// 注意：必须在 Vite 加载配置**之前**设置。`vite.config.ts` 本身由 esbuild 转译，
// 若在配置模块内设置环境变量则为时已晚（转译已发生），故用本包装脚本。
//
// 用法：
//   node scripts/run-with-project-tmp.mjs <命令> [参数...]
import { spawn } from "node:child_process";
import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(__dirname, "..");

// 工具临时目录：与测试临时目录同处 build/tmp/，随 build/ 整体清理
const toolTemp = join(repoRoot, "build", "tmp", "tool-temp");
mkdirSync(toolTemp, { recursive: true });

const [command, ...args] = process.argv.slice(2);
if (!command) {
  console.error("[project-tmp] 用法: node scripts/run-with-project-tmp.mjs <命令> [参数...]");
  process.exit(2);
}

const child = spawn(command, args, {
  stdio: "inherit",
  shell: true,
  env: {
    ...process.env,
    TEMP: toolTemp,
    TMP: toolTemp,
  },
});

child.on("exit", (code, signal) => {
  if (signal !== null) {
    console.error(`[project-tmp] ${command} 被信号终止: ${signal}`);
    process.exit(1);
  }
  process.exit(code ?? 1);
});

child.on("error", (err) => {
  console.error(`[project-tmp] 启动 ${command} 失败: ${err.message}`);
  process.exit(1);
});
