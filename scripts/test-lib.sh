#!/bin/bash
# test-lib.sh —— src-tauri 单元测试运行器（comctl32 manifest 注入 + 运行数据本地化 + 失败必须失败）
#
# 背景 1（comctl32）：本机 System32\comctl32.dll 为 v5.82 基线（Win11 正常状态），
# comctl32 v6 需通过 WinSxS manifest 激活。Rust test harness 生成的测试二进制无
# comctl32 v6 manifest，缺少 TaskDialogIndirect 导出（tauri-plugin-dialog 静态导入）
# → STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)。主应用 exe 不受影响（tauri-build 已嵌 manifest）。
#
# 解法：cargo test --no-run 编译后，用 Windows SDK mt.exe 把 comctl6.manifest
# 注入测试 exe 资源 #1，再运行测试。源码未变时 cargo 不重编译，manifest 保留。
#
# 背景 2（运行数据越界，2026-09-23 实测）：测试进程原先写系统 %TEMP% 与 %APPDATA%\SECM。
# 本仓库工作区目录带 Low Mandatory Level 标签，从该目录启动的进程即低完整性进程，
# 实测无法在 %TEMP% 建目录（OS 错误 5 拒绝访问）、也无法向 %APPDATA%\SECM\logs 追加，
# 导致 cleanup / log 模块共 4 个用例恒失败。把 TEMP / APPDATA / SECM_RUNTIME_DIR 指向
# 项目内 build/tmp/ 后同样 4 个用例全部通过，证明失败源于落点越界而非被测代码缺陷。
#
# 背景 3（门禁失真，ADR-0001 D2 修复）：原实现测试阶段为
#     cargo test --lib ... 2>&1 | tail -40
# 在只有 set -e（无 pipefail）时，管道退出码取末命令 tail 的 0，于是任何测试失败都被吞掉，
# 脚本仍以 0 退出——而本脚本是 AGENTS.md 指定的 src-tauri 单测门禁。已用返回 101 的桩 cargo
# 实测复现：原写法退出码为 0。
#
# 现修复为：
#   - set -euo pipefail；
#   - 编译与测试输出分别落 build/tmp/test-lib-build.log 与 test-lib-run.log，失败时回显末 40 行；
#   - 测试阶段不经管道：直接重定向到文件后取 $?，原样作为脚本退出码；
#   - 编译失败立即失败并给出 cargo 原始错误（不再用 || 重试吞掉错误）；
#   - 测试 exe 优先取 cargo JSON 的 executable 字段（消除 ls -t 取错二进制），失败再回退；
#   - mt.exe 同时探测 Git Bash（/c/...）与 WSL（/mnt/c/...）挂载点。
#
# 用法（在仓库根目录，或 scripts/ 内直接执行）：
#   bash scripts/test-lib.sh                # 全部 lib 单元测试
#   bash scripts/test-lib.sh driver_install # 指定过滤（其余参数透传给 cargo test）
#   bash scripts/test-lib.sh -- --ignored real_machine_ensure_installed  # 真机验证
#   REQUIRE_MANIFEST=1 bash scripts/test-lib.sh   # 缺 mt.exe 时直接失败（CI/干净环境门禁）
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT/src-tauri"

# ---- 运行数据本地化：测试产物全部落在 build/tmp/ ----
TEST_ROOT="$REPO_ROOT/build/tmp"
BUILD_LOG="$TEST_ROOT/test-lib-build.log"
TEST_LOG="$TEST_ROOT/test-lib-run.log"
mkdir -p "$TEST_ROOT/test-temp" "$TEST_ROOT/test-runtime"
export TEMP="$TEST_ROOT/test-temp"
export TMP="$TEST_ROOT/test-temp"
export APPDATA="$TEST_ROOT/test-runtime"
export SECM_RUNTIME_DIR="$TEST_ROOT/test-runtime"

ARGS=("$@")

# 失败诊断：回显日志末尾，保留 cargo 原始错误
tail_log() {
    echo "[test-lib] ---- $1 末 40 行 ----"
    tail -40 "$1" 2>/dev/null || true
    echo "[test-lib] ---- 完整日志: $1 ----"
}

# 1. 编译（不运行）。编译失败必须显式失败并回显 cargo 原始错误。
# 始终用 `--lib --no-run`：过滤参数只影响「运行哪些用例」，不影响编译产物，
# 故不把它们传给编译步骤（原实现的两分支会产生不同产物，是本次修复前的隐患）。
echo "[test-lib] 编译测试二进制（完整输出: build/tmp/test-lib-build.log）"
set +e
cargo test --lib --no-run --message-format=json >"$BUILD_LOG" 2>&1
BUILD_RC=$?
set -e
if [ "$BUILD_RC" -ne 0 ]; then
    echo "[test-lib] 错误：编译失败（cargo 退出码 $BUILD_RC）"
    tail_log "$BUILD_LOG"
    exit "$BUILD_RC"
fi

# 2. 定位本次编译产出的测试 exe：取 cargo JSON 里 "executable" 字段的最后一条。
# 用 sed 的 BRE 提取（已在本机 bash 5.3 实测语法与取值均正确）。
EXE="$(sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' "$BUILD_LOG" 2>/dev/null | grep -v "^null$" | tail -1)"
if [ -z "$EXE" ] || [ ! -f "$EXE" ]; then
    # 回退：cargo 未报告 executable 时按 mtime 取最新
    EXE="$(ls -t target/debug/deps/secm_lib-*.exe 2>/dev/null | head -1)"
fi

# 3. 注入 comctl32 v6 manifest。mt.exe 同时探测 Git Bash 与 WSL 的挂载点。
MT=""
for root in /c /mnt/c; do
    for candidate in "$root/Program Files (x86)/Windows Kits/10/bin/"*/x64/mt.exe; do
        if [ -f "$candidate" ]; then
            MT="$candidate"
        fi
    done
done
if [ -n "$MT" ]; then
    if ! "$MT" -manifest comctl6.manifest -outputresource:"$EXE;#1" >/dev/null 2>&1; then
        echo "[test-lib] 错误：manifest 注入失败。测试二进制缺少 comctl32 v6 manifest 时会以"
        echo "[test-lib]       STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) 崩溃，测试结论不可信。"
        exit 1
    fi
    echo "[test-lib] comctl32 v6 manifest 已注入: $(basename "$EXE")"
elif [ "${REQUIRE_MANIFEST:-0}" = "1" ]; then
    echo "[test-lib] 错误：未找到 mt.exe（需要 Windows SDK），且 REQUIRE_MANIFEST=1。"
    echo "[test-lib]       请安装 Windows 10/11 SDK 后重试。"
    exit 1
else
    echo "[test-lib] 警告：未找到 mt.exe（Windows SDK）。仅当测试 exe 已带 comctl32 v6"
    echo "[test-lib]       manifest（此前注入过且源码未变）时才可运行；否则会崩溃。"
    echo "[test-lib]       干净环境请设置 REQUIRE_MANIFEST=1 以获得明确失败。"
fi

# 4. 运行测试。
#
# 直接执行**已注入 manifest 的那个二进制**，不再经 `cargo test`：
# cargo 会按指纹重新编译并覆盖 exe，把注入的 comctl32 v6 manifest 抹掉，
# 于是测试以 0xc0000139 (STATUS_ENTRYPOINT_NOT_FOUND) 崩溃——本次修复中实测复现
# （REAL_EXIT=127）。直接运行既保住 manifest，也顺带避免二次编译。
#
# 仍不经管道：重定向到日志后取 $?，避免退出码被管道末命令覆盖。
echo "[test-lib] 运行数据落点: $TEST_ROOT"
echo "[test-lib] 测试 exe: $EXE（完整输出: build/tmp/test-lib-run.log）"

# 透传过滤参数给 libtest：去掉可能存在的分隔符 `--`（cargo 语法的一部分）。
FILTERS=()
for arg in "${ARGS[@]}"; do
    if [ "$arg" = "--" ]; then
        continue
    fi
    FILTERS+=("$arg")
done

set +e
"$EXE" "${FILTERS[@]}" >"$TEST_LOG" 2>&1
TEST_RC=$?
set -e

tail -40 "$TEST_LOG" || true
if [ "$TEST_RC" -ne 0 ]; then
    echo "[test-lib] 测试失败（cargo 退出码 $TEST_RC）"
    echo "[test-lib] 完整日志: $TEST_LOG"
    exit "$TEST_RC"
fi

echo "[test-lib] 测试通过"
exit 0
