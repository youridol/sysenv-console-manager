// Hardware/Tier2Wire.cs
//
// 提权伴生进程 ↔ 未提权主进程之间的**内部** tier-2 通道负载。
//
// 该负载不属于 CONTRACT-v3.md §3 的对外快照契约（对外只有 /api/v1/snapshot 的 v3 形状），
// 它是 SECM 两级权限模型的实现细节：伴生进程（--role=elevated，监听 port+1）在
// GET /api/v1/tier2 上返回本结构，主进程解析后与 tier-1 合并成**同一份**快照。
//
// 非契约字段（内部）：这里可以自由增删；§3 的对外字段名一个都不能动。

namespace LhmSidecar.Hardware;

/// <summary>伴生进程返回的 tier-2 原始负载。</summary>
public sealed class Tier2Payload
{
    /// <summary>伴生进程是否真的读到了至少一项 tier-2 指标。</summary>
    public bool Available { get; set; }

    /// <summary>不可用原因（可用时为 null）。</summary>
    public string? Reason { get; set; }

    public Tier2Cpu? Cpu { get; set; }

    /// <summary>主板 SuperIO 温度/风扇/电压（节点缺失时为 null）。</summary>
    public MotherboardData? Motherboard { get; set; }

    /// <summary>卷名 → 磁盘温度 ℃（如 "C:" → 38.0）。</summary>
    public Dictionary<string, double>? DiskTemps { get; set; }

    /// <summary>代表磁盘温度（仅单卷时作为回退，避免把某块盘的温度安到多个卷上）。</summary>
    public double? RepresentativeDiskTempC { get; set; }

    /// <summary>
    /// 诊断用：LHM 存储硬件的 "名称 [类型]" 列表（含子硬件）。
    /// 用于排查"卷名 ↔ 物理盘"映射失败（磁盘温度只有 tier-2 能读，映射问题只能在提权侧观察）。
    /// 非契约字段，主进程不消费。
    /// </summary>
    public List<string>? StorageHardware { get; set; }

    /// <summary>转换成 SnapshotBuilder 需要的读数结构。</summary>
    public SnapshotBuilder.Tier2Readout ToReadout()
    {
        var r = new SnapshotBuilder.Tier2Readout
        {
            CpuTempC = Cpu?.PackageTempC.HasValue == true ? (float)Cpu.PackageTempC!.Value : null,
            CoreTempsC = Cpu?.CoreTempsC ?? Array.Empty<double>(),
            CpuPowerW = Cpu?.PowerW.HasValue == true ? (float)Cpu.PowerW!.Value : null,
            CpuVoltageV = Cpu?.VoltageV.HasValue == true ? (float)Cpu.VoltageV!.Value : null,
            CpuFanRpm = Cpu?.FanRpm.HasValue == true ? (float)Cpu.FanRpm!.Value : null,
            CpuPumpRpm = Cpu?.PumpRpm.HasValue == true ? (float)Cpu.PumpRpm!.Value : null,
            Motherboard = Motherboard,
            RepresentativeDiskTempC = RepresentativeDiskTempC.HasValue
                ? (float)RepresentativeDiskTempC.Value : null
        };

        if (DiskTemps != null)
        {
            foreach (var kv in DiskTemps)
            {
                r.DiskTemps[kv.Key] = (float)kv.Value;
            }
        }
        return r;
    }
}

/// <summary>负载中的 CPU tier-2 字段（对外契约 cpu.package_temp_c 等一一对应）。</summary>
public sealed class Tier2Cpu
{
    public double? PackageTempC { get; set; }
    public double[] CoreTempsC { get; set; } = Array.Empty<double>();
    public double? PowerW { get; set; }
    public double? VoltageV { get; set; }
    public double? FanRpm { get; set; }
    public double? PumpRpm { get; set; }
}
