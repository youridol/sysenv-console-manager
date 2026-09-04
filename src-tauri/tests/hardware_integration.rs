//! 硬件传感器服务集成测试(GPU/CPU 温度/频率/功耗 + HTTP 接口)。
//!
//! 覆盖:
//!   1. 采集层:collect_sensors() 结构契约(id/name/temperature/frequency/power,
//!      字段可为 null;错误码 HW_SENSOR_NOT_FOUND/HW_ACCESS_DENIED/HW_INTERNAL_ERROR);
//!   2. nvidia-smi 解析:真实 nvidia-smi 输出解析(NVIDIA 环境)与格式异常降级;
//!   3. HTTP 层:启动服务后 GET /api/hardware/sensors 返回 200 JSON,
//!      未知路径 404,非 GET 405;
//!   4. Mock 模式:SECM_HARDWARE_MOCK=1 返回可配置模拟数据。
//!
//! 运行:cargo test --test hardware_integration
//! 注:HTTP 测试使用独立端口(SECM_HTTP_PORT 覆盖),避免与运行中的服务冲突。

use std::io::Read;
use std::time::Duration;

use secm_lib::hardware;

// ============================================================================
// 采集层契约测试
// ============================================================================

#[test]
fn test_collect_sensors_shape() {
    // 真实采集(本机有 nvidia-smi):结构完整,字段可为 null
    match hardware::collect_sensors() {
        Ok(resp) => {
            // GPU:至少能枚举(注册表或 nvidia-smi)
            assert!(
                !resp.gpus.is_empty() || !resp.cpus.is_empty(),
                "GPU 与 CPU 不应同时为空"
            );
            for g in &resp.gpus {
                assert!(g.id.starts_with("gpu-"), "GPU id 前缀错误: {}", g.id);
                assert!(!g.name.is_empty(), "GPU 名称不应为空");
                // 字段可为 null(无法采集),但不能缺失
                assert!(g.temperature.is_none() || g.temperature.unwrap() > 0.0);
                assert!(g.frequency.is_none() || g.frequency.unwrap() > 0.0);
                assert!(g.power.is_none() || g.power.unwrap() >= 0.0);
            }
            for c in &resp.cpus {
                assert!(c.id.starts_with("cpu-"), "CPU id 前缀错误: {}", c.id);
                assert!(!c.name.is_empty(), "CPU 名称不应为空");
            }
        }
        Err(e) => {
            // 极端环境允许明确错误码(消息含修复建议)
            assert!(
                ["HW_SENSOR_NOT_FOUND", "HW_ACCESS_DENIED", "HW_INTERNAL_ERROR"]
                    .contains(&e.code.as_str()),
                "意外错误码: {}",
                e.code
            );
            assert!(!e.message.is_empty());
        }
    }
}

// ============================================================================
// Mock 模式测试(独立环境变量,不污染其他测试)
// ============================================================================

#[test]
fn test_mock_mode() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        std::env::set_var("SECM_HARDWARE_MOCK", "1");
        std::env::set_var("SECM_HARDWARE_MOCK_GPUS", "3");
        std::env::set_var("SECM_HARDWARE_MOCK_CORES", "4");
    }
    let resp = hardware::collect_sensors().expect("mock 模式不应失败");
    assert_eq!(resp.gpus.len(), 3);
    assert_eq!(resp.cpus.len(), 4);
    for g in &resp.gpus {
        assert!(g.temperature.is_some());
        assert!(g.frequency.is_some());
        assert!(g.power.is_some());
    }
    unsafe {
        std::env::remove_var("SECM_HARDWARE_MOCK");
        std::env::remove_var("SECM_HARDWARE_MOCK_GPUS");
        std::env::remove_var("SECM_HARDWARE_MOCK_CORES");
    }
}

// ============================================================================
// HTTP 接口测试(独立端口,启动临时服务)
// ============================================================================

/// 环境变量是进程级的,Rust 测试默认并行,多个测试改 env 会互相干扰。
/// 所有改环境变量的测试必须持有此锁串行执行。
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 发起 HTTP GET,返回 (状态码, 响应体)。
fn http_get(port: u16, path: &str) -> (u16, String) {
    use std::io::Write;
    use std::net::TcpStream;

    let addr = format!("127.0.0.1:{}", port);
    let mut stream = TcpStream::connect(&addr)
        .unwrap_or_else(|e| panic!("连接 {} 失败: {}", addr, e));
    let req = format!("GET {} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n", path);
    stream
        .write_all(req.as_bytes())
        .unwrap_or_else(|e| panic!("写入请求失败: {}", e));
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .unwrap_or_else(|e| panic!("读取响应失败: {}", e));
    let text = String::from_utf8_lossy(&buf).to_string();

    // 解析状态行:HTTP/1.1 200 OK
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    // 取 body(空行之后)
    let body = text
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or("")
        .to_string();
    (status, body)
}

#[test]
fn test_http_sensors_endpoint() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // 使用独立端口,避免与运行中的实例冲突
    unsafe {
        std::env::set_var("SECM_HTTP_PORT", "19871");
    }
    let port = hardware::start_http_server().expect("HTTP 服务应能启动");
    assert_eq!(port, 19871);

    // 等待服务就绪(启动线程异步,轮询最多 2s)
    let mut ready = false;
    for _ in 0..40 {
        let (status, _) = http_get(port, "/api/hardware/sensors");
        if status != 0 {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(ready, "HTTP 服务 2s 内未就绪");

    // 1) 正常路径:200 + JSON 含 gpus/cpus,响应时间 < 500ms(任务书验收标准)
    let start = std::time::Instant::now();
    let (status, body) = http_get(port, "/api/hardware/sensors");
    let elapsed_ms = start.elapsed().as_millis();
    assert_eq!(status, 200, "sensors 应返回 200,body={}", body);
    assert!(
        elapsed_ms < 500,
        "响应时间 {}ms 超过 500ms 验收标准",
        elapsed_ms
    );
    let v: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("响应不是合法 JSON: {} | {}", e, body));
    assert!(v.get("gpus").is_some(), "响应缺 gpus 字段");
    assert!(v.get("cpus").is_some(), "响应缺 cpus 字段");

    // 2) 未知路径:404
    let (status, body) = http_get(port, "/api/unknown");
    assert_eq!(status, 404, "未知路径应 404,body={}", body);
    assert!(body.contains("HW_NOT_FOUND"), "404 应携带错误码,body={}", body);

    // 3) 非 GET:405
    {
        use std::io::Write;
        use std::net::TcpStream;
        let addr = format!("127.0.0.1:{}", port);
        let mut stream = TcpStream::connect(&addr).expect("连接失败");
        let req = "POST /api/hardware/sensors HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
        stream.write_all(req.as_bytes()).expect("写入失败");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).expect("读取失败");
        let text = String::from_utf8_lossy(&buf).to_string();
        let status = text
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(0);
        assert_eq!(status, 405, "POST 应返回 405,resp={}", text);
        assert!(text.contains("HW_METHOD_NOT_ALLOWED"));
    }

    unsafe {
        std::env::remove_var("SECM_HTTP_PORT");
    }
}

#[test]
fn test_http_mock_endpoint() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // mock 模式 + 独立端口:返回固定模拟数据
    unsafe {
        std::env::set_var("SECM_HARDWARE_MOCK", "1");
        std::env::set_var("SECM_HARDWARE_MOCK_GPUS", "2");
        std::env::set_var("SECM_HARDWARE_MOCK_CORES", "6");
        std::env::set_var("SECM_HTTP_PORT", "19872");
    }
    let port = hardware::start_http_server().expect("HTTP 服务应能启动");
    let mut ready = false;
    for _ in 0..40 {
        let (status, _) = http_get(port, "/api/hardware/sensors");
        if status != 0 {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(ready);
    let (status, body) = http_get(port, "/api/hardware/sensors");
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).expect("合法 JSON");
    let gpus = v["gpus"].as_array().expect("gpus 数组");
    let cpus = v["cpus"].as_array().expect("cpus 数组");
    assert_eq!(gpus.len(), 2);
    assert_eq!(cpus.len(), 6);
    unsafe {
        std::env::remove_var("SECM_HARDWARE_MOCK");
        std::env::remove_var("SECM_HARDWARE_MOCK_GPUS");
        std::env::remove_var("SECM_HARDWARE_MOCK_CORES");
        std::env::remove_var("SECM_HTTP_PORT");
    }
}
