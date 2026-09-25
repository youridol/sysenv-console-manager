// Snapshot.cs —— 契约 v3 快照 DTO（冻结于 sidecar-lhm/CONTRACT-v3.md §3）
//
// 序列化一律使用 JsonNamingPolicy.SnakeCaseLower（见 Program.JsonOpts）。
// 硬性规则（CONTRACT-v3.md §3）：
//   - 缺失标量 → null（double? / long?），缺失数组 → []，缺失对象 → null；
//   - **绝不伪造 0**：任何"读不到"的字段都必须保持 null，由前端渲染为 "—"。
//   - 字段名与顺序严格按 §3 jsonc：available/error/contract_version/elevated/
//     tier2_available/tier2_reason/cpu/gpu/motherboard/memory/disks/battery。

namespace LhmSidecar;

/// <summary>CPU 指标（契约 §3 cpu）。tier-1：load_percent / clock_mhz；tier-2：温度/功耗/电压/风扇。</summary>
public sealed class CpuData
{
    /// LHM CPU 节点名（缺失时空串，非 null）
    public string Name { get; set; } = string.Empty;

    /// 负载 % —— PDH % Processor Utility（截断 100）；Tier1
    public double? LoadPercent { get; set; }

    /// Package / 最高核心温度 ℃ —— Tier2
    public double? PackageTempC { get; set; }

    /// 各核心/CCD 温度数组 —— Tier2；不可用时为 []
    public double[] CoreTempsC { get; set; } = Array.Empty<double>();

    /// Package 功耗 W —— Tier2
    public double? PowerW { get; set; }

    /// 核心电压 V —— Tier2
    public double? VoltageV { get; set; }

    /// 频率 MHz —— tier-1（PDH % Processor Performance × 基准频率）
    public double? ClockMhz { get; set; }

    /// CPU 风扇 RPM —— Tier2
    public double? FanRpm { get; set; }

    /// 水泵 RPM —— Tier2
    public double? PumpRpm { get; set; }

    /// "perfcounter" | "lhm" | "none"
    public string LoadSource { get; set; } = SourceNames.None;

    /// "lhm" | "none"
    public string TempSource { get; set; } = SourceNames.None;

    /// "lhm" | "none"
    public string PowerSource { get; set; } = SourceNames.None;

    /// "perfcounter" | "lhm" | "registry" | "none"
    public string ClockSource { get; set; } = SourceNames.None;

    /// 恒 false：移植后不再有 TDP×usage 估算路径（保留字段以兼容前端）
    public bool PowerEstimated { get; set; }
}

/// <summary>单张 GPU 指标（契约 §3 gpu[]）。全部 tier-1（厂商用户态通道）。</summary>
public sealed class GpuData
{
    /// 优先分组的代表显卡名（LHM 硬件节点名）
    public string Name { get; set; } = string.Empty;

    public double? TemperatureC { get; set; }
    public double? CoreClockMhz { get; set; }
    public double? PowerW { get; set; }
    public double? LoadPercent { get; set; }
    public long? MemoryUsedBytes { get; set; }
    public long? MemoryTotalBytes { get; set; }
    public double? FanRpm { get; set; }

    /// 计算值 used/total*100；仅当两者都有值且 total>0 时非 null
    public double? VramLoadPercent { get; set; }

    /// "lhm" | "none"
    public string TempSource { get; set; } = SourceNames.None;

    /// "lhm" | "none"
    public string PowerSource { get; set; } = SourceNames.None;

    /// "lhm" | "none"
    public string ClockSource { get; set; } = SourceNames.None;
}

/// <summary>主板传感器（契约 §3.1：仅 temperature / fan / voltage 三种 type）。</summary>
public sealed class MotherboardSensorData
{
    public string Name { get; set; } = string.Empty;

    /// "temperature" | "fan" | "voltage"
    public string Type { get; set; } = string.Empty;

    public double Value { get; set; }
}

/// <summary>主板数据（契约 §3 motherboard）。tier-2 不可用或节点缺失时为 null。</summary>
public sealed class MotherboardData
{
    public string? Name { get; set; }

    public List<MotherboardSensorData> Sensors { get; set; } = new();
}

/// <summary>内存数据（契约 §3 memory）。tier-1，SECM 仍以 GlobalMemoryStatusEx 为权威。</summary>
public sealed class MemoryData
{
    public string? Name { get; set; }
    public long? TotalBytes { get; set; }
    public long? UsedBytes { get; set; }
}

/// <summary>单卷磁盘指标（契约 §3 disks[]）。速率/占用 tier-1；温度 tier-2。</summary>
public sealed class DiskData
{
    /// 卷名（"C:"），契约示例为卷名
    public string Name { get; set; } = string.Empty;

    /// 每卷 PDH "Disk Read Bytes/sec"（实例 = 卷标），Tier1
    public double? ReadBytesPerSec { get; set; }

    /// 每卷 PDH "Disk Write Bytes/sec"（实例 = 卷标），Tier1
    public double? WriteBytesPerSec { get; set; }

    /// PDH "% Disk Time"（实例 = 卷标），截断 0-100，Tier1
    public double? ActivityPercent { get; set; }

    /// DriveInfo 已用百分比，Tier1
    public double? UsedPercent { get; set; }

    /// 磁盘温度 ℃ —— Tier2；不可用时 null
    public double? TemperatureC { get; set; }
}

/// <summary>电池（契约 §3 battery）。无电池时为 null。</summary>
public sealed class BatteryData
{
    public double? Percent { get; set; }

    /// 已做 AC 在线符号归一：插电为正、放电为负（BatteryService）
    public double? PowerW { get; set; }

    public double? VoltageV { get; set; }

    /// 已做 AC 在线符号归一（BatteryService）
    public double? CurrentA { get; set; }
}

/// <summary>契约 v3 快照根对象（GET /api/v1/snapshot 响应体）。</summary>
public sealed class Snapshot
{
    /// 见 CONTRACT-v3.md §4 的 available 语义
    public bool Available { get; set; }

    /// 顶层不可用原因（available=true 时为 null）
    public string? Error { get; set; }

    /// 常量 3
    public int ContractVersion { get; set; } = 3;

    /// tier-2 是否已生效（含提权伴生进程实际贡献数据）
    public bool Elevated { get; set; }

    /// tier-2 指标当前是否可读
    public bool Tier2Available { get; set; }

    /// tier-2 不可用原因（可读时为 null）
    public string? Tier2Reason { get; set; }

    public CpuData Cpu { get; set; } = new();
    public List<GpuData> Gpu { get; set; } = new();
    public MotherboardData? Motherboard { get; set; }
    public MemoryData? Memory { get; set; }
    public List<DiskData> Disks { get; set; } = new();
    public BatteryData? Battery { get; set; }
}

/// <summary>`*_source` 取值常量（契约 §3 冻结枚举）。</summary>
public static class SourceNames
{
    public const string None = "none";
    /// CPU 负载/频率来自 PDH 性能计数器
    public const string PerfCounter = "perfcounter";
    /// 来自 LHM（tier-2 ring0 或厂商用户态通道）
    public const string Lhm = "lhm";
    /// 频率回退到注册表 ~MHz 标称值
    public const string Registry = "registry";
}
