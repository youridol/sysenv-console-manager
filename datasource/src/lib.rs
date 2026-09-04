//! secm-datasource — SECM 纯 Rust 驱动采集模块
//!
//! 按设计文档 `docs/designs/2026-08-06-rust-data-source-migration.md` 实现，
//! 替换原有 PowerShell / 外部命令（`sc` / `powercfg` / `wmic` / `ipconfig` / `curl`）
//! 数据源，消除外部进程依赖。
//!
//! 模块职责（全部为底层采集，零业务逻辑）：
//! - [`registry`]：注册表采集（补丁 P2 / GPU 厂商 P6 / 通用读写）
//! - [`service`]：服务管理（P10/P11/P12，advapi32）
//! - [`power`]：电源计划（P7/P8/P9，powrprof）
//! - [`netif`]：网络接口（P3 链路速度 / P4 本地 IP，iphlpapi）
//! - [`dns`]：DNS 缓存刷新（P13，dnsapi）
//! - [`http`]：HTTP 客户端（P5 公网 IP/地理位置，ureq + URL 白名单）
//! - [`activation`]：Windows 激活状态（P1，注册表近似 + LicenseStatus 映射表）
//! - [`error`]：统一错误模型 `CollectError`
//!
//! 依赖方向：本 crate 是叶子模块，仅依赖 windows-sys / winreg / ureq / serde；
//! 业务模块（sysinfo / settings / net_stats / ip_info / game_env / cleanup）依赖本 crate。
//! 禁止反向依赖。
//!
//! 线程模型：本 crate 全部为同步阻塞 API，调用方须在 `spawn_blocking` / `thread::scope`
//! 中执行（S8 异步并发多线程：GUI 主线程永不接触采集逻辑）。

pub mod activation;
pub mod cpu_freq;
pub mod dns;
pub mod disk;
pub mod disk_io;
pub mod error;
pub mod http;
pub mod netif;
pub mod power;
pub mod registry;
pub mod service;

pub use error::CollectError;
