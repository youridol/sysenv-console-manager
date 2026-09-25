// ESLint flat config（ESLint 9+）。
//
// 说明：此前本文件只含 `ignores`，等于仅做语法解析，`npm run lint` 不具检查能力。
// 现补齐最小可用规则集：
//   - js.configs.recommended        —— JS 通用正确性问题
//   - tseslint.configs.recommended  —— TypeScript 类型与常见陷阱
//   - react-hooks / react-refresh   —— React Hooks 依赖与热更新边界
// 不做与项目现状无关的风格裁决（格式由 Prettier 负责，见 eslint-config-prettier）。
import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import prettier from "eslint-config-prettier";
import globals from "globals";

export default tseslint.config(
  {
    ignores: [
      "dist/**",
      "node_modules/**",
      "src-tauri/target/**",
      "build/**",
      "tmp/**",
      "sidecar-lhm/bin/**",
      "sidecar-lhm/obj/**",
      ".verify_tmp/**",
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    // 前端源码：浏览器环境全局量
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      globals: { ...globals.browser },
    },
  },
  {
    // 构建与审计脚本：Node 环境全局量（console / process / Buffer 等）
    files: ["scripts/**/*.mjs", "*.config.js", "*.config.ts", "vite.config.ts"],
    languageOptions: {
      globals: { ...globals.node },
    },
  },
  {
    files: ["**/*.{ts,tsx}"],
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      // 经典 Hooks 正确性规则：违反即真实缺陷，保持 error
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      // 说明：本插件 7.x 的 `recommended` 还包含 React Compiler 专用规则
      // （set-state-in-effect / refs / purity / immutability 等）。本项目未接入
      // React Compiler，启用它们会把 15 处既有页面模式下合法的「effect 内异步加载后
      // setState」判为错误，进而要求与本轮审计无关的页面重构；故此处只启用与运行时
      // 正确性直接相关的经典规则，不扩大检查面到编译器优化约束。
      "react-refresh/only-export-components": [
        "warn",
        { allowConstantExport: true },
      ],
      // 未使用变量：允许以 `_` 前缀显式表示有意忽略
      "@typescript-eslint/no-unused-vars": [
        "error",
        {
          argsIgnorePattern: "^_",
          varsIgnorePattern: "^_",
          caughtErrorsIgnorePattern: "^_",
        },
      ],
    },
  },
  // 关闭与 Prettier 冲突的格式化规则（必须最后一项）
  prettier,
);
