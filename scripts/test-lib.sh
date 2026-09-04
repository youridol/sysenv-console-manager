#!/bin/bash
# test-lib.sh —— src-tauri 单元测试运行器（绕开 comctl32 v5.82 基线问题）
#
# 背景：本机 System32\comctl32.dll 为 v5.82 基线（Win11 正常状态），
# comctl32 v6 需通过 WinSxS manifest 激活。Rust test harness 生成的
# 测试二进制无 comctl32 v6 manifest，缺少 TaskDialogIndirect 导出
# （tauri-plugin-dialog 静态导入）→ STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)。
# 主应用 exe 不受影响（tauri-build 已嵌 v6 manifest）。
#
# 解法：cargo test --no-run 编译后，用 Windows SDK mt.exe 把 comctl6.manifest
# 注入测试 exe 资源 #1，再运行测试。源码未变时 cargo 不重编译，manifest 保留。
#
# 用法（在仓库根目录）：
#   bash scripts/test-lib.sh                # 全部 lib 单元测试
#   bash scripts/test-lib.sh driver_install # 指定过滤（其余参数透传给 cargo test）
#   bash scripts/test-lib.sh -- --ignored real_machine_ensure_installed  # 真机验证
set -e
cd "$(dirname "$0")/../src-tauri"

ARGS=("$@")

# 1. 编译（不运行）
if [ ${#ARGS[@]} -gt 0 ]; then
    cargo test --lib --no-run "${ARGS[@]}" >/dev/null 2>&1 || cargo test --no-run >/dev/null 2>&1
else
    cargo test --no-run >/dev/null 2>&1
fi

# 2. 定位最新测试 exe 并注入 comctl32 v6 manifest
EXE=$(ls -t target/debug/deps/secm_lib-*.exe 2>/dev/null | head -1)
if [ -z "$EXE" ]; then
    echo "[test-lib] 未找到测试 exe，编译失败？"
    exit 1
fi
# 找到任意可用的 mt.exe（Windows SDK）
MT=$(ls "/c/Program Files (x86)/Windows Kits/10/bin/"*/x64/mt.exe 2>/dev/null | tail -1)
if [ -n "$MT" ]; then
    if ! "$MT" -manifest comctl6.manifest -outputresource:"$EXE;#1" >/dev/null 2>&1; then
        echo "[test-lib] 警告：manifest 注入失败，若测试崩溃请手动检查 comctl32 环境"
    else
        echo "[test-lib] comctl32 v6 manifest 已注入: $(basename "$EXE")"
    fi
else
    echo "[test-lib] 警告：未找到 mt.exe（Windows SDK），测试可能因 comctl32 崩溃"
fi

# 3. 运行测试
if [ ${#ARGS[@]} -gt 0 ]; then
    cargo test --lib "${ARGS[@]}" 2>&1 | tail -40
else
    cargo test --lib 2>&1 | tail -40
fi
