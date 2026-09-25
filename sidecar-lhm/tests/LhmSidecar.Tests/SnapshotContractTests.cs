// SnapshotContractTests.cs —— 契约 v3 形状与"绝不伪造 0"。
//
// 断言 CONTRACT-v3.md §3 / §3.2 / §4：
//   - 字段名一律 snake_case（JsonNamingPolicy.SnakeCaseLower）
//   - tier-2 缺失：cpu 温度/功耗/电压/风扇/水泵 null（core_temps_c 为 []）、
//     *_source = "none"、motherboard = null、disks[].temperature_c = null
//   - available 语义；未采集到的字段保持 null，绝不用 0 冒充

using System.Text.Json;
using LhmSidecar.Hardware;
using Xunit;

namespace LhmSidecar.Tests;

public class SnapshotContractTests
{
    private static readonly JsonSerializerOptions Opts = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower
    };

    private static SnapshotBuilder NewBuilder(out SensorMap map)
    {
        map = new SensorMap();
        return new SnapshotBuilder(map);
    }

    private static SnapshotBuilder.Tier1Readout Tier1With(PdhLike data) => data.ToReadout();

    /// <summary>构造 tier-1 读数（不触碰真实 PDH，避免测试受机器状态影响）。</summary>
    private sealed class PdhLike
    {
        public float? CpuLoad = 12.5f;
        public float? CpuClock = 4200f;
        public bool ClockFromRegistry;
        public float? MemTotal = 17179869184f;
        public float? MemUsed = 8589934592f;
        public List<SnapshotBuilder.DiskEntry> Disks = new()
        {
            new SnapshotBuilder.DiskEntry
            {
                Name = "C:",
                ReadBytesPerSec = 1048576,
                WriteBytesPerSec = 524288,
                ActivityPercent = 12,
                UsedPercent = 63.5
            }
        };
        public BatteryData? Battery;

        public SnapshotBuilder.Tier1Readout ToReadout() => new()
        {
            CpuLoadPercent = CpuLoad,
            CpuClockMhz = CpuClock,
            CpuClockFromRegistry = ClockFromRegistry,
            MemTotalBytes = MemTotal.HasValue ? (long)MemTotal.Value : null,
            MemUsedBytes = MemUsed.HasValue ? (long)MemUsed.Value : null,
            Disks = Disks,
            Battery = Battery
        };
    }

    // --- tier-2 缺失时的确切形态（CONTRACT-v3.md §3.2）---
    [Fact]
    public void Tier2Absent_ProducesNullFieldsAndNoneSources()
    {
        var builder = NewBuilder(out _);
        var snap = builder.Build(
            lhmCpuName: "AMD Ryzen 7 5800X 8-Core Processor",
            gpuHardware: Array.Empty<LibreHardwareMonitor.Hardware.IHardware>(),
            tier1: Tier1With(new PdhLike()),
            tier2: null,
            elevated: false,
            tier2Available: false,
            tier2Reason: "需要管理员权限（PawnIO 设备仅 SYSTEM/Administrators 可访问）");

        // tier-1 有数据 → available 仍为 true（§3.2 最后一条）
        Assert.True(snap.Available);
        Assert.Null(snap.Error);

        // 权限状态
        Assert.False(snap.Elevated);
        Assert.False(snap.Tier2Available);
        Assert.NotNull(snap.Tier2Reason);

        // CPU tier-1
        Assert.Equal("AMD Ryzen 7 5800X 8-Core Processor", snap.Cpu.Name);
        Assert.Equal(12.5, snap.Cpu.LoadPercent);
        Assert.Equal("perfcounter", snap.Cpu.LoadSource);
        Assert.Equal(4200, snap.Cpu.ClockMhz);
        Assert.Equal("perfcounter", snap.Cpu.ClockSource);
        Assert.False(snap.Cpu.PowerEstimated);

        // CPU tier-2 全部缺失
        Assert.Null(snap.Cpu.PackageTempC);
        Assert.Empty(snap.Cpu.CoreTempsC);
        Assert.Null(snap.Cpu.PowerW);
        Assert.Null(snap.Cpu.VoltageV);
        Assert.Null(snap.Cpu.FanRpm);
        Assert.Null(snap.Cpu.PumpRpm);
        Assert.Equal("none", snap.Cpu.TempSource);
        Assert.Equal("none", snap.Cpu.PowerSource);

        // motherboard null；磁盘温度 null
        Assert.Null(snap.Motherboard);
        Assert.Single(snap.Disks);
        Assert.Null(snap.Disks[0].TemperatureC);
        // tier-1 磁盘字段保留
        Assert.Equal(1048576, snap.Disks[0].ReadBytesPerSec);
        Assert.Equal(12, snap.Disks[0].ActivityPercent);
        Assert.Equal(63.5, snap.Disks[0].UsedPercent);
    }

    // --- tier-1 全空且 tier-2 缺失 → available=false 且给出可读原因（§4）---
    [Fact]
    public void FullyUnavailable_SetsAvailableFalseWithReason()
    {
        var builder = NewBuilder(out _);
        var empty = new PdhLike
        {
            CpuLoad = null,
            CpuClock = null,
            MemTotal = null,
            MemUsed = null,
            Disks = new List<SnapshotBuilder.DiskEntry>()
        };

        var snap = builder.Build("", Array.Empty<LibreHardwareMonitor.Hardware.IHardware>(),
            empty.ToReadout(), null, elevated: false, tier2Available: false, tier2Reason: "需要管理员权限");

        Assert.False(snap.Available);
        Assert.NotNull(snap.Error);
        Assert.Contains("提权", snap.Error);   // 含修复建议（§4）
        // 所有标量仍为 null，没有任何伪造的 0
        Assert.Null(snap.Cpu.LoadPercent);
        Assert.Null(snap.Cpu.ClockMhz);
        Assert.Null(snap.Cpu.PackageTempC);
        Assert.Null(snap.Memory);
    }

    // --- tier-2 生效时字段填充且 source=lhm ---
    [Fact]
    public void Tier2Present_FillsFieldsAndSources()
    {
        var builder = NewBuilder(out _);
        var tier2 = new SnapshotBuilder.Tier2Readout
        {
            CpuTempC = 45.2f,
            CoreTempsC = new double[] { 44.1, 45.2 },
            CpuPowerW = 38.4f,
            CpuVoltageV = 1.19f,
            CpuFanRpm = 812f,
            CpuPumpRpm = null,
            Motherboard = new MotherboardData
            {
                Name = "HUANANZHI X99",
                Sensors = new List<MotherboardSensorData>
                {
                    new() { Name = "System", Type = "temperature", Value = 34.2 },
                    new() { Name = "Fan #1", Type = "fan", Value = 812 },
                    new() { Name = "+12V", Type = "voltage", Value = 12.1 }
                }
            },
            DiskTemps = { ["C:"] = 38f }
        };

        var snap = builder.Build("Ryzen", Array.Empty<LibreHardwareMonitor.Hardware.IHardware>(),
            Tier1With(new PdhLike()), tier2, elevated: true, tier2Available: true, tier2Reason: null);

        Assert.True(snap.Elevated);
        Assert.True(snap.Tier2Available);
        Assert.Null(snap.Tier2Reason);

        Assert.Equal(45.2f, snap.Cpu.PackageTempC!.Value, 3);
        Assert.Equal(new double[] { 44.1, 45.2 }, snap.Cpu.CoreTempsC);
        Assert.Equal(38.4f, snap.Cpu.PowerW!.Value, 3);
        Assert.Equal(1.19f, snap.Cpu.VoltageV!.Value, 3);
        Assert.Equal(812f, snap.Cpu.FanRpm!.Value, 3);
        Assert.Null(snap.Cpu.PumpRpm);          // 无水泵 → null（不伪造 0）
        Assert.Equal("lhm", snap.Cpu.TempSource);
        Assert.Equal("lhm", snap.Cpu.PowerSource);

        Assert.NotNull(snap.Motherboard);
        Assert.Equal("HUANANZHI X99", snap.Motherboard!.Name);
        Assert.Equal(3, snap.Motherboard.Sensors.Count);

        Assert.Equal(38f, snap.Disks[0].TemperatureC!.Value, 3);
    }

    // --- 代表卡缺失但磁盘温度存在时不猜测（诚实失败）---
    [Fact]
    public void Tier2Present_ButNoDiskTempMatch_LeavesTemperatureNull()
    {
        var builder = NewBuilder(out _);
        var tier2 = new SnapshotBuilder.Tier2Readout { CpuTempC = 50f, DiskTemps = { ["D:"] = 40f } };
        var tier1 = new PdhLike();
        // 两个卷 → 不做单卷回退
        tier1.Disks.Add(new SnapshotBuilder.DiskEntry
        {
            Name = "D:",
            ReadBytesPerSec = 0,
            WriteBytesPerSec = 0,
            ActivityPercent = 0,
            UsedPercent = 50
        });

        var snap = builder.Build("Ryzen", Array.Empty<LibreHardwareMonitor.Hardware.IHardware>(),
            tier1.ToReadout(), tier2, elevated: true, tier2Available: true, tier2Reason: null);

        Assert.Null(snap.Disks[0].TemperatureC);   // "C:" 没有温度 → null
        Assert.Equal(40f, snap.Disks[1].TemperatureC!.Value, 3);
    }

    // --- 序列化形状：字段名 snake_case 且 tier-2 缺失时确为 null ---
    [Fact]
    public void Serialization_UsesSnakeCaseAndNullForMissingTier2()
    {
        var builder = NewBuilder(out _);
        var snap = builder.Build(
            lhmCpuName: "Ryzen",
            gpuHardware: Array.Empty<LibreHardwareMonitor.Hardware.IHardware>(),
            tier1: Tier1With(new PdhLike { Battery = new BatteryData { Percent = 87, PowerW = -12.5, VoltageV = 15.6, CurrentA = -0.8 } }),
            tier2: null,
            elevated: false,
            tier2Available: false,
            tier2Reason: "需要管理员权限");

        string json = JsonSerializer.Serialize(snap, Opts);
        using var doc = JsonDocument.Parse(json);
        var root = doc.RootElement;

        // 契约字段名（§3）
        Assert.True(root.TryGetProperty("contract_version", out var cv));
        Assert.Equal(3, cv.GetInt32());
        Assert.True(root.TryGetProperty("tier2_available", out _));
        Assert.True(root.TryGetProperty("tier2_reason", out _));

        var cpu = root.GetProperty("cpu");
        Assert.Equal(JsonValueKind.Null, cpu.GetProperty("package_temp_c").ValueKind);
        Assert.Equal(JsonValueKind.Null, cpu.GetProperty("power_w").ValueKind);
        Assert.Equal(JsonValueKind.Null, cpu.GetProperty("voltage_v").ValueKind);
        Assert.Equal(JsonValueKind.Null, cpu.GetProperty("fan_rpm").ValueKind);
        Assert.Equal(JsonValueKind.Null, cpu.GetProperty("pump_rpm").ValueKind);
        Assert.Equal(0, cpu.GetProperty("core_temps_c").GetArrayLength());
        Assert.Equal("none", cpu.GetProperty("temp_source").GetString());
        Assert.Equal("none", cpu.GetProperty("power_source").GetString());
        Assert.Equal("perfcounter", cpu.GetProperty("load_source").GetString());
        Assert.False(cpu.GetProperty("power_estimated").GetBoolean());

        Assert.Equal(JsonValueKind.Null, root.GetProperty("motherboard").ValueKind);
        Assert.Equal(JsonValueKind.Null, root.GetProperty("error").ValueKind);

        var disks = root.GetProperty("disks");
        Assert.Equal(1, disks.GetArrayLength());
        Assert.Equal(JsonValueKind.Null, disks[0].GetProperty("temperature_c").ValueKind);
        Assert.Equal("C:", disks[0].GetProperty("name").GetString());

        var battery = root.GetProperty("battery");
        Assert.Equal(87, battery.GetProperty("percent").GetDouble());
        Assert.Equal(-12.5, battery.GetProperty("power_w").GetDouble());

        // 空 GPU / 空数组（§3：缺失数组为 []）
        Assert.Equal(0, root.GetProperty("gpu").GetArrayLength());
    }

    // --- 无电池时必须为 null（不是零值对象）---
    [Fact]
    public void NoBattery_SerializesAsNull()
    {
        var builder = NewBuilder(out _);
        var tier1 = new PdhLike { Battery = null };
        var snap = builder.Build("Ryzen", Array.Empty<LibreHardwareMonitor.Hardware.IHardware>(),
            tier1.ToReadout(), null, false, false, "需要管理员权限");

        string json = JsonSerializer.Serialize(snap, Opts);
        using var doc = JsonDocument.Parse(json);
        Assert.Equal(JsonValueKind.Null, doc.RootElement.GetProperty("battery").ValueKind);
    }

    // --- 注册表频率回退 → clock_source="registry"（契约 §3 / §7）---
    [Fact]
    public void RegistryClockFallback_SetsRegistrySource()
    {
        var builder = NewBuilder(out _);
        var tier1 = new PdhLike { CpuClock = 3600f, ClockFromRegistry = true };
        var snap = builder.Build("Ryzen", Array.Empty<LibreHardwareMonitor.Hardware.IHardware>(),
            tier1.ToReadout(), null, false, false, "需要管理员权限");

        Assert.Equal(3600, snap.Cpu.ClockMhz);
        Assert.Equal("registry", snap.Cpu.ClockSource);
    }

    // --- available 语义（§4）：单凭内存或磁盘或 GPU 也可为 true ---
    [Fact]
    public void Available_TrueWhenOnlyMemoryPresent()
    {
        var builder = NewBuilder(out _);
        var tier1 = new PdhLike { CpuLoad = null, CpuClock = null, Disks = new List<SnapshotBuilder.DiskEntry>() };
        var snap = builder.Build("", Array.Empty<LibreHardwareMonitor.Hardware.IHardware>(),
            tier1.ToReadout(), null, false, false, "需要管理员权限");
        Assert.True(snap.Available);
    }

    [Fact]
    public void Available_TrueWhenOnlyPackageTempPresent()
    {
        var builder = NewBuilder(out _);
        var tier1 = new PdhLike { CpuLoad = null, CpuClock = null, MemTotal = null, MemUsed = null, Disks = new List<SnapshotBuilder.DiskEntry>() };
        var tier2 = new SnapshotBuilder.Tier2Readout { CpuTempC = 55f };
        var snap = builder.Build("", Array.Empty<LibreHardwareMonitor.Hardware.IHardware>(),
            tier1.ToReadout(), tier2, true, true, null);
        Assert.True(snap.Available);
    }
}
