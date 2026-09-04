//! 真实模式验证：list_disks + get_disk_smart（无 mock 环境变量）
//! 运行：cargo run --example disk_info_real

fn main() {
    std::env::remove_var("SECM_DISK_MOCK");
    println!("== 真实 list_disks ==");
    let disks = secm_lib::disk_info::list_disks();
    println!("共 {} 块盘", disks.len());
    for d in &disks {
        println!(
            "#{} {} [{} {}] size={} status={} label={} alert={} alerts={:?}",
            d.id, d.model, d.interface_type, d.media_type, d.size_bytes, d.health.status,
            d.health.label, d.health.alert, d.health.alerts
        );
    }

    if let Some(first) = disks.first() {
        println!("\n== 真实 get_disk_smart('{}') ==", first.id);
        let detail = secm_lib::disk_info::get_disk_smart(&first.id);
        println!(
            "id={} model={} iface={} media={} status={} source={} attrs={} err={:?}",
            detail.id,
            detail.model,
            detail.interface_type,
            detail.media_type,
            detail.health.status,
            if detail.attributes.is_empty() && detail.nvme_health.is_none() && detail.error.is_none() { "wmi" } else { "ioctl" },
            detail.attributes.len(),
            detail.error
        );
    }
}
