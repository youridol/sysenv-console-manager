// ESLint 9+ flat config（此前缺失导致 `npm run lint` 无法运行）。
// 项目暂无自定义规则诉求：保持语法级检查（无规则 = 仅解析错误会报），
// 忽略构建产物与依赖目录。需要规则时在此文件按 flat config 语法添加。
export default [
  {
    ignores: ["dist/**", "node_modules/**", "src-tauri/target/**"],
  },
];
