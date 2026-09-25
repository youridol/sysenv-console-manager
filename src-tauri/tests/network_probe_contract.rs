//! 回归护栏：网络诊断页的「HTTP 延迟测试 / 端口检测」必须在**发行版**（收紧 CSP 的
//! 真实 WebView2 环境）下可用。
//!
//! ## 这个 BUG 已经复发过两次
//!
//! 1. v0.9.x：发行版里 `fetch` 被 CSP 拦截 → 全部站点显示"不可达"。
//!    当时的修法是往 CSP `connect-src` 加回通配 `https: http:`，
//!    CHANGELOG 原文：「CSP `connect-src` 追加 `https: http:`
//!    修复发行版 fetch 被拦截导致全部显示不可达」。
//! 2. v1.19.0 安全收紧：CHANGELOG 记为「CSP `connect-src` 收紧
//!    （去掉通配 https:/http:）」——**把上面那次修复回退了**，
//!    同一个用户可见 BUG 再次出现（全部站点不可达）。
//!
//! 根因是"前端直连外部站点"与"收紧的 CSP"相互矛盾，而两者分处不同文件、
//! 由不同理由驱动，因此任何一方单独改动都可能静默打破另一方。本测试把这条
//! **跨文件的隐含契约**固化成断言。
//!
//! ## 现在的设计
//!
//! 探测移到 Rust 侧（`crate::network::probe_url` / `commands::probe_url`），
//! 前端不再对外发起 `fetch`。于是：
//!   - CSP 可以保持收紧（无需通配 `https:`）；
//!   - 用户自定义的任意站点也能探测（无法用固定白名单覆盖）。
//!
//! 因此本测试断言三件事：
//!   A. 前端页面源码中**不再**出现对站点的直接 `fetch` 探测；
//!   B. 前端改为调用后端 `probe_url` 命令；
//!   C. 后端确实注册了该命令，且探测实现存在。
//!
//! 这些断言全部基于**源码文本**，不需要启动 GUI 或 WebView2。

use std::path::PathBuf;

/// 从 crate 目录向上找到仓库根（`src-tauri` 的父目录）。
fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = <repo>/src-tauri
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 应有父目录")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("无法读取 {}: {}", p.display(), e))
}

/// A + B：前端必须走后端探测，不得再对外直接 fetch。
#[test]
fn website_probe_does_not_use_browser_fetch() {
    let network_tsx = read("src/pages/Network.tsx");

    // A. 不得再存在对外部站点的 fetch 探测。
    //    允许 fetch 出现在（将来可能的）IPC/本地场景，但对外部 http(s) 的
    //    直接探测必须消失——那正是被 CSP 拦截的写法。
    assert!(
        !network_tsx.contains("mode: \"no-cors\"") && !network_tsx.contains("mode: 'no-cors'"),
        "Network.tsx 仍在用 `fetch(..., {{mode: \"no-cors\"}})` 直连外部站点。\n\
         收紧的 CSP connect-src 会在**发行版**里拦截它，导致「HTTP 延迟测试」\n\
         全部显示不可达（该 BUG 已复发两次，见本文件顶部注释）。\n\
         请改用后端命令 `probe_url`。"
    );
    assert!(
        !network_tsx.contains("function headProbe"),
        "Network.tsx 仍存在浏览器侧 headProbe。请改用后端 `probe_url` 命令。"
    );

    // B. 必须调用后端命令。
    assert!(
        network_tsx.contains("probe_url"),
        "Network.tsx 未调用后端 `probe_url` 命令——网站/端口可达性探测缺失实现路径。"
    );
}

/// C：后端必须注册命令，且探测实现必须显式设置连接超时。
#[test]
fn backend_exposes_bounded_probe_url_command() {
    let commands_rs = read("src-tauri/src/commands.rs");
    let lib_rs = read("src-tauri/src/lib.rs");
    let network_rs = read("src-tauri/src/network.rs");

    assert!(
        commands_rs.contains("pub async fn probe_url"),
        "commands.rs 缺少 `probe_url` 命令实现"
    );
    assert!(
        lib_rs.contains("commands::probe_url"),
        "lib.rs 的 invoke_handler 未注册 `probe_url`——前端调用会得到 UnknownCommand"
    );
    assert!(
        network_rs.contains("pub fn probe_url"),
        "network.rs 缺少 `probe_url` 探测实现"
    );

    // 连接超时必须显式设置：ureq 的 `.timeout()` 只管读写阶段，连接阶段由
    // `timeout_connect` 控制（默认 30s）。漏掉它会让一次探测在异常网络下
    // 悬挂半分钟，用户看到"检测中"一直转。
    assert!(
        network_rs.contains("timeout_connect"),
        "probe_url 未设置 `timeout_connect`：ureq 连接阶段默认 30s，异常网络下探测会长时间挂起"
    );
}

/// **HTTP 延迟口径守卫**：「HTTP 延迟测试」展示的时间必须是**应用层 HTTP 延迟**，
/// 且全站只有这一个耗时口径。
///
/// 历史上这里并列过两个口径（裸 HTTP `ms` + 隐藏 WebView2 实测 `page_ms`），
/// UI 在两者之间来回回退显示，同一站点在不同时刻给出两套数字。本版本起统一为
/// HTTP 延迟：`page_ms`、`page_probe` 模块、`measure_page` 开关与
/// `SECM_PAGE_PROBE_SELFTEST` 自检一并移除。
///
/// 同时守住"不得用别的东西冒充延迟"这条线：不得再引入 ICMP RTT
/// （网络层往返，与 HTTP 延迟不是同一个量）。
///
/// 因此本测试锁定：
///   A. 结果结构体只有 HTTP 延迟 `ms`，不再有 `page_ms`；
///   B. 浏览器测速模块与自检入口已彻底移除（不得复活）；
///   C. 前端展示 `ms` 且文案写明是「HTTP 延迟」；
///   D. 「HTTP 延迟测试」与「端口检测」共用同一个两参数探测函数。
#[test]
fn http_latency_is_the_single_metric_for_site_test() {
    let network_rs = read("src-tauri/src/network.rs");
    let commands_rs = read("src-tauri/src/commands.rs");
    let network_tsx = read("src/pages/Network.tsx");
    let lib_rs = read("src-tauri/src/lib.rs");

    // A. 结构体：只有 HTTP 延迟一个耗时口径
    assert!(
        network_rs.contains("pub ms: u64"),
        "UrlProbeResult 缺少 `ms`（HTTP 延迟口径）——「HTTP 延迟测试」没有可展示的数值"
    );
    assert!(
        !network_rs.contains("pub page_ms"),
        "UrlProbeResult 又出现了 `page_ms`：耗时口径必须唯一（HTTP 延迟），\
         两个口径会让同一站点在不同时刻给出两套数字"
    );

    // B. 浏览器测速模块与自检入口不得复活。
    //
    // 断言只针对**声明与调用点**，不针对注释里出现的名字：注释里说明"这里曾经
    // 有什么、为什么移除"是有价值的，不该为了守卫而删掉历史说明。
    let page_probe_path = repo_root().join("src-tauri/src/page_probe.rs");
    assert!(
        !page_probe_path.exists(),
        "src-tauri/src/page_probe.rs 又出现了：隐藏 WebView2 页面测速已被移除，\
         自动检测每 5s 为每个站点开一次浏览器引擎的开销与轮询频率完全不匹配"
    );
    assert!(
        !lib_rs.contains("mod page_probe") && !lib_rs.contains("var(\"SECM_PAGE_PROBE_SELFTEST\")"),
        "lib.rs 又声明了 page_probe 模块或启用了 SECM_PAGE_PROBE_SELFTEST 自检入口：\
         浏览器页面测速的模块声明与自检都应保持移除状态"
    );
    // 只断言**参数声明**（`measure_page:`）而非注释里提到的开关名。
    assert!(
        !commands_rs.contains("measure_page:"),
        "commands::probe_url 又接受了 `measure_page` 参数：\
         统一为 HTTP 延迟后不存在需要跳过浏览器测速的调用方"
    );

    // C. 前端：展示 ms，且文案写明是「HTTP 延迟」
    assert!(
        network_tsx.contains("{status.ms}ms"),
        "Network.tsx 未展示 `status.ms`——HTTP 延迟口径未接上后端"
    );
    assert!(
        network_tsx.contains("HTTP 延迟"),
        "Network.tsx 未出现「HTTP 延迟」字样：只给数字而不写明口径，\
         用户会把它当成 ICMP ping 或页面打开耗时"
    );

    // D. 两处调用共用同一个两参数（无 measurePage）探测函数。
    // 注意：这里**去掉全部空白**后再匹配，避免 Prettier 折行导致"纯格式化"就让守卫
    // 误报失败。去空白后仍要求两处调用的实参都只有 url 与 timeoutMs。
    let compact: String = network_tsx.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        compact.contains("probeReachable(`http://${target}:${port}`,5000)"),
        "端口检测未以两参数调用 `probeReachable`：\
         统一为 HTTP 延迟后不应再传第三个 measurePage 实参"
    );
    assert!(
        compact.contains("probeReachable(url,8000)"),
        "HTTP 延迟测试未以两参数调用 `probeReachable`"
    );
}

/// 防止有人"用近似量代替 HTTP 延迟"地再次改变这个口径。
///
/// 历史上 ICMP RTT 曾被当作延迟口径加进来（github.com ~106ms），它是网络层
/// 往返；HTTP 延迟额外含 TCP/TLS 与应用层往返，两者不可互换。本测试要求：
/// 网络探测路径不得再引入 ICMP 作为延迟口径。
#[test]
fn no_icmp_proxy_metric_for_http_latency() {
    let network_rs = read("src-tauri/src/network.rs");
    let commands_rs = read("src-tauri/src/commands.rs");

    assert!(
        !network_rs.contains("pub async fn icmp_rtt_ms"),
        "network.rs 又出现了 `icmp_rtt_ms`：ICMP RTT 是网络层往返，\
         与 HTTP 延迟（含 TCP/TLS/应用层往返）不是同一个量，不得作为该口径"
    );
    assert!(
        !network_rs.contains("pub rtt_ms: Option<f64>"),
        "UrlProbeResult 又出现了 `rtt_ms`：不要再用 ICMP 往返时间冒充 HTTP 延迟"
    );
    assert!(
        !commands_rs.contains("measure_rtt"),
        "commands::probe_url 又出现了 `measure_rtt`：ICMP 口径不应回归"
    );
}

/// 防止有人"图省事"把通配 `https:` 加回 CSP。
///
/// 这不是为了禁止该写法本身，而是要求：**若 CSP 被放宽，本测试必须同步更新**，
/// 从而强制改动者在 PR 里显式说明"为什么又要放开外部连接"——而不是无意识地
/// 再次制造/回退这个 BUG。
#[test]
fn csp_connect_src_stays_tight_and_is_documented() {
    let conf = read("src-tauri/tauri.conf.json");

    // 提取 csp 的值（配置为单行；用引号边界切出，避免把后续键也算进来）
    let csp_key = conf.find("\"csp\"").expect("tauri.conf.json 应有 csp 配置");
    let after = &conf[csp_key..];
    let open = after.find('"').expect("csp 键应有引号");
    // 第二个引号是 csp 键名的结尾，第三个引号开始是值
    let key_rest = &after[open + 1..];
    let key_end = key_rest.find('"').expect("csp 键名应有结尾引号");
    let val_rest = &key_rest[key_end + 1..];
    let val_open = val_rest.find('"').expect("csp 值应有起始引号");
    let val_body = &val_rest[val_open + 1..];
    let val_end = val_body.find('"').expect("csp 值应有结束引号");
    let csp = &val_body[..val_end];

    assert!(
        csp.contains("connect-src"),
        "csp 缺少 connect-src 指令：{csp}"
    );

    // 取出 connect-src 指令的所有来源 token（到下一个指令 `;` 为止）
    let cs_start = csp.find("connect-src").expect("已断言存在 connect-src") + "connect-src".len();
    let cs_tail = &csp[cs_start..];
    let cs_end = cs_tail.find(';').unwrap_or(cs_tail.len());
    let sources: Vec<&str> = cs_tail[..cs_end].split_whitespace().collect();

    // 当前设计下前端不需要对外连接（探测走 Rust 侧），故不应出现
    // 通配 scheme 源 `https:` / `http:`（单独成 token 的裸 scheme）。
    // 注意区分：`https://ipc.localhost` 是合法且必要的来源，不算通配。
    for wildcard in ["https:", "http:"] {
        assert!(
            !sources.iter().any(|s| *s == wildcard),
            "csp connect-src 出现通配来源 `{wildcard}`（sources={sources:?}）。\n\
             当前设计是探测走 Rust 侧、CSP 保持收紧；放宽前请确认不会重新引入\n\
             「发行版 fetch 被拦截导致网站全部不可达」的历史 BUG，\
             并同步更新本测试与注释。"
        );
    }

    // 正向断言：IPC 来源必须保留（Tauri invoke 依赖它）
    assert!(
        sources
            .iter()
            .any(|s| s.starts_with("https://ipc.localhost"))
            && sources
                .iter()
                .any(|s| s.starts_with("http://ipc.localhost")),
        "csp connect-src 缺少 IPC 来源（Tauri invoke 需要 ipc.localhost）：sources={sources:?}"
    );
    assert!(
        sources.contains(&"ipc:"),
        "csp connect-src 缺少 `ipc:` 来源：sources={sources:?}"
    );
}
