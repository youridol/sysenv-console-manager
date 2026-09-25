//! 回归护栏：清理页的**执行结果**必须同时进入右侧栏日志面板。
//!
//! ## 背景
//!
//! 清理页原本只有页内「执行结果」卡片承载 `CleanupResult{operation, success,
//! bytes_freed, message}`。后端 `cmd_log!` 另有一条**摘要**日志
//! （`clean_temp_files: ok=true, freed=0, 1234ms`），但它不含用户可见的 message
//! 正文（真实释放量、逐条错误、重启删除汇总），因此右侧栏看不到清理结果。
//!
//! 本仓库**没有前端测试运行器**（无 vitest/jest，`src/**` 无 `*.test.ts`），
//! 因此沿用 `network_probe_contract.rs` 已确立的做法：用 Rust 集成测试对前端
//! **源码文本**断言跨文件契约。这类断言不需要 GUI/WebView2，可在 CI 稳定运行。
//!
//! ## 被固化的契约
//!
//!   A. 每次执行恰好上报**一条**日志（成功与失败分支各一次，且都在防重入
//!      序号守卫之后，避免过期响应重复记账）；
//!   B. 上报走 `logger.action`（**不节流**）——若走 `logger.info`，
//!      其 `级别:模块` 5s 节流会让 5 秒内的第二次清理操作被静默丢弃；
//!   C. 消息长度有界（前端 4096 截断 + 后端 MAX_LEN 4096 字节），
//!       不能依赖后端截断作为设计；
//!   D. 失败按 warn/error 上报，成功按 info。
//!
//! 这些全部基于源码文本。若将来重构改名，请同步更新本测试——那正是它的用途。

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 应有父目录")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("无法读取 {}: {}", p.display(), e))
}

/// 去掉 `//` 行注释、`/* */` 块注释与 `{/* */}` JSX 注释后的源码。
///
/// **为什么必须去注释**：这些契约断言用"源码里是否出现某段文本"来表达。
/// 若不去注释，**文档里提到该标识符**就会让断言假绿——本测试第一版就踩过：
/// 注释里写着「为什么用 `logger.action` 而非 `logger.info`」，
/// 于是把真实调用改成 `logger.info(` 后，断言因命中注释而仍然通过。
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let mut in_line = false;
    let mut in_block = false;
    let mut in_str: Option<char> = None;
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        if in_line {
            if c == '\n' {
                in_line = false;
                out.push(c);
            }
            i += 1;
            continue;
        }
        if in_block {
            if c == '*' && next == Some('/') {
                in_block = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if let Some(q) = in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n2) = next {
                    out.push(n2);
                }
                i += 2;
                continue;
            }
            if c == q {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match (c, next) {
            ('/', Some('/')) => {
                in_line = true;
                i += 2;
            }
            ('/', Some('*')) => {
                in_block = true;
                i += 2;
            }
            ('"', _) | ('\'', _) | ('`', _) => {
                in_str = Some(c);
                out.push(c);
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// A + D：执行结果上报存在，且成功/失败分支都覆盖。
#[test]
fn cleanup_reports_action_result_on_both_branches() {
    let raw = read("src/pages/Cleanup.tsx");
    // 断言必须基于**去注释**源码：注释里提到 reportActionResult 会造成假绿
    let cleanup = strip_comments(&raw);

    assert!(
        cleanup.contains("reportActionResult"),
        "Cleanup.tsx 缺少 `reportActionResult`——清理执行结果未进入右侧栏日志面板"
    );

    // 成功分支按 INFO
    assert!(
        cleanup.contains("reportActionResult(result)"),
        "未在成功分支上报执行结果（预期 `reportActionResult(result)`）"
    );

    // 失败分支按 WARN/ERROR
    assert!(
        cleanup.contains("reportActionResult(failed"),
        "未在失败分支上报执行结果（预期 `reportActionResult(failed, ...)`）"
    );
    assert!(
        cleanup.contains("\"WARN\"") && cleanup.contains("\"ERROR\""),
        "失败上报未区分 WARN/ERROR 级别"
    );

    // 上报必须位于防重入守卫之后：过期响应不得重复记账。
    //
    // 结构（见 runAction）：每个分支各自先做 `if (seq !== actionSeq.current) return;`，
    // 再做上报。因此要断言的是"每个上报调用之前**最近**的一个守卫属于同一分支"。
    // `reportActionResult` 的**定义**（`function reportActionResult(`）必然排在前面，
    // 故用带分号/带括号的**调用**形式定位，避开定义行。
    let guards: Vec<usize> = cleanup
        .match_indices("seq !== actionSeq.current")
        .map(|(i, _)| i)
        .collect();
    assert!(
        guards.len() >= 2,
        "Cleanup.tsx 应保留 actionSeq 防重入守卫（成功与失败分支各一处），实际找到 {} 处",
        guards.len()
    );

    for (label, call_pat) in [
        ("成功分支", "reportActionResult(result);"),
        ("失败分支", "reportActionResult(failed"),
    ] {
        let call = cleanup
            .find(call_pat)
            .unwrap_or_else(|| panic!("{label}应有上报调用 `{call_pat}`"));
        let guarded = guards.iter().any(|&g| g < call);
        assert!(
            guarded,
            "{label}的执行结果上报出现在任何防重入序号守卫之前——\
             超时后的过期响应会重复记账"
        );
    }
}

/// B：必须用不节流的入口，否则 5s 内第二次操作会被静默丢弃。
#[test]
fn cleanup_uses_unthrottled_action_logger() {
    let cleanup = strip_comments(&read("src/pages/Cleanup.tsx"));
    let logger_ts = strip_comments(&read("src/lib/logger.ts"));

    assert!(
        cleanup.contains("logger.action("),
        "Cleanup.tsx 未**实际调用** `logger.action`（注释里提到不算）。\n\
         使用 `logger.info/warn/error` 会命中 `reportToBackend` 的 5s 节流\n\
         （键为「级别:模块」），导致连续两次清理操作中第二条被**静默丢弃**。"
    );

    // logger.action 必须存在，且其实现不含节流判定
    assert!(
        logger_ts.contains("action:"),
        "logger.ts 未导出 `action` 入口"
    );
    let action_doc = logger_ts
        .find("function logAction")
        .expect("logger.ts 应有 `logAction` 实现");
    let action_body = &logger_ts[action_doc..];
    let end = action_body.find("\n}").unwrap_or(action_body.len());
    let body = &action_body[..end];
    assert!(
        !body.contains("BACKEND_THROTTLE_MS") && !body.contains("lastSent"),
        "`logger.action` 的实现里出现了节流逻辑——用户操作结果会被静默丢弃，\
         违背「一次执行 = 一条日志」"
    );

    // 正常入口的节流必须保留（不能为了让 action 不节流而全局放开）
    assert!(
        logger_ts.contains("BACKEND_THROTTLE_MS") && logger_ts.contains("lastSent"),
        "logger.ts 丢失了 `BACKEND_THROTTLE_MS` / `lastSent` 节流——\
         全局节流被移除会让高频日志刷爆环形缓冲"
    );
}

/// C：消息长度必须有界，且不依赖后端截断。
#[test]
fn cleanup_action_message_is_char_bounded() {
    let raw = read("src/pages/Cleanup.tsx");
    let cleanup = strip_comments(&raw);

    assert!(
        cleanup.contains("clipChars"),
        "Cleanup.tsx 未见字符裁剪助手 `clipChars`——多行清理消息（含逐条错误）\
         可能突破 4096 字节上限"
    );
    // 常量值需从**原始**源码解析（声明行的名字不能被去注释影响，但保留 raw 更稳）
    let total_cap = const_value(&raw, "OP_LOG_MAX_TOTAL_CHARS")
        .expect("OP_LOG_MAX_TOTAL_CHARS 应为数字字面量");
    assert!(
        raw.contains("const OP_LOG_MAX_BODY_LINES"),
        "未见正文行数上限常量 `OP_LOG_MAX_BODY_LINES`"
    );

    // 校验预算：CJK 每 UTF-16 码元最多 3 字节（见 Cleanup.tsx 注释论证）
    const BACKEND_MAX_BYTES: usize = 4096;
    // 截断结果 = cap 字符 + 「…」= cap+1 码元；上界 3 B/码元
    let worst_bytes = (total_cap + 1) * 3;
    assert!(
        worst_bytes < BACKEND_MAX_BYTES,
        "最坏消息长度为 {worst_bytes} 字节（{total_cap} 字符 × 3 B/码元 + 省略号），\
         已达/超过后端 {BACKEND_MAX_BYTES} 字节上限——必须留出余量，\
         否则消息会被后端静默截断"
    );
}

/// 从 `const NAME[: TYPE] = <数字>;` 提取数值。
///
/// 注意：常量名也会出现在文档注释与使用处（如 `clipChars(head, OP_LOG_MAX_TOTAL_CHARS)`），
/// 因此必须逐行匹配**真正的声明行**（以 `const ` 开头且含 `=`），不能只找名字首次出现处。
/// 声明可带类型标注（`const X: number = 1`）也可不带（`const X = 1`）。
fn const_value(src: &str, name: &str) -> Option<usize> {
    let prefix = format!("const {}:", name);
    let prefix_no_type = format!("const {} ", name);
    for line in src.lines() {
        let trimmed = line.trim_start();
        let is_decl =
            trimmed.starts_with(&prefix) || trimmed.starts_with(&prefix_no_type);
        if !is_decl {
            continue;
        }
        let Some(eq) = trimmed.find('=') else { continue };
        let digits: String = trimmed[eq + 1..]
            .chars()
            .skip_while(|c| c.is_whitespace())
            .take_while(|c| c.is_ascii_digit() || *c == '_')
            .filter(|c| c.is_ascii_digit())
            .collect();
        return digits.parse().ok();
    }
    None
}

/// scope 归属：确保条目落在 `cleanup` 上，「按页输出」才能在清理页看到。
///
/// module → scope 的解析有**两个**必须同时成立的环节（ADR-0001 D10）：
///   1. 前端 `Cleanup.tsx` 上报时使用模块名 `"Cleanup"`；
///   2. 后端 `log::scope_for_module("Cleanup")` 解析为 `"cleanup"`。
///
/// 本用例原先断言 `src/lib/logs.ts` 里存在 `cleanup: "cleanup"` —— 那份前端映射
/// 零调用点、从未执行，其断言无法证明任何真实行为；ADR-0001 D10 删除该映射后，
/// 改为断言真正生效的后端解析函数与本文件的路由映射。
#[test]
fn cleanup_scope_resolves_to_cleanup() {
    // 1. 路由 /cleanup → scope cleanup（前端「按页输出」按路由过滤时使用）
    let logs_ts = strip_comments(&read("src/lib/logs.ts"));
    assert!(
        logs_ts.contains("\"/cleanup\": \"cleanup\""),
        "src/lib/logs.ts 的 /cleanup 路由映射被改动，清理结果在「按页输出」模式下不可见"
    );

    // 2. 后端 module → scope 的唯一真源必须把 "Cleanup" 解析为 "cleanup"
    let log_rs = strip_comments(&read("src-tauri/src/log.rs"));
    assert!(
        log_rs.contains("\"cleanup\" => \"cleanup\""),
        "src-tauri/src/log.rs::scope_for_module 的 cleanup 映射被改动：\
         module \"Cleanup\" 会落到 system，「按页输出」看不到清理结果"
    );

    // 3. 上报时必须使用模块名 "Cleanup"（否则会落到 system）
    let logger_call = strip_comments(&read("src/pages/Cleanup.tsx"));
    assert!(
        logger_call.contains("\"Cleanup\""),
        "Cleanup.tsx 上报时未**实际使用**模块名 \"Cleanup\"（scope 会落到 system，\
         导致「按页输出」看不到）"
    );
}
