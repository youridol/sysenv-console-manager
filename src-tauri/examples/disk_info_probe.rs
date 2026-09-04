//! 主 crate disk_info 逻辑验证（mock 开关 + 健康度判断）
//! 运行：SECM_DISK_MOCK=1 cargo run --example disk_info_probe

fn main() {
    // mock 模式：3 块盘（良好/预警/严重）
    std::env::set_var("SECM_DISK_MOCK", "1");
    let disks = secm_lib::disk_info::list_disks();
    println!("== mock list_disks: {} 块盘 ==", disks.len());
    for d in &disks {
        println!(
            "#{} {} [{}] status={} label={} alert={} alerts={:?} temp={:?}",
            d.id, d.model, d.interface_type, d.health.status, d.health.label, d.health.alert,
            d.health.alerts, d.health.temperature_c
        );
    }

    println!("\n== mock get_disk_smart('1') ==");
    let detail = secm_lib::disk_info::get_disk_smart("1");
    println!(
        "id={} model={} iface={} media={} status={} attrs={} err={:?}",
        detail.id,
        detail.model,
        detail.interface_type,
        detail.media_type,
        detail.health.status,
        detail.attributes.len(),
        detail.error
    );
    for a in &detail.attributes {
        println!(
            "  id={:#04x} {} raw={} cur={} worst={} status={}",
            a.id, a.name, a.raw, a.value, a.worst, a.status
        );
    }

    println!("\n== mock get_disk_smart('0') NVMe ==");
    let nvme = secm_lib::disk_info::get_disk_smart("0");
    println!(
        "status={} nvme_health={:?}",
        nvme.health.status,
        nvme.nvme_health.map(|h| format!(
            "temp={}°C spare={}% wear={}% pOH={}h cw=0x{:02X}",
            h.temperature_c, h.available_spare, h.percentage_used, h.power_on_hours, h.critical_warning
        ))
    );

    println!("\n== mock get_disk_smart('9') 不存在 ==");
    let missing = secm_lib::disk_info::get_disk_smart("9");
    println!("status={} err={:?}", missing.health.status, missing.error);
}
