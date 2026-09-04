#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // SECM 不要求管理员权限启动：
    // - 温度/功耗主数据源为 LHM sidecar（独立提权部署 PawnIO 驱动）与
    //   WinRing0/ACPI 降级链，普通用户可读（无需主进程管理员权限）；
    // - 需要管理员的功能（清理/服务管理等）由各命令独立检查权限并返回错误提示。
    secm_lib::run()
}
