// Tier2WireTests.cs —— 内部 tier-2 负载 → SnapshotBuilder 读数的转换。
// 该负载不是对外契约（对外只有 §3 的 v3 快照），但它是两级权限模型的关键一环：
// 未提权主进程靠它把伴生进程的 tier-2 数据合并进同一份快照。

using LhmSidecar.Hardware;
using Xunit;

namespace LhmSidecar.Tests;

public class Tier2WireTests
{
    [Fact]
    public void ToReadout_MapsCpuFieldsAndDiskTemps()
    {
        var payload = new Tier2Payload
        {
            Available = true,
            Cpu = new Tier2Cpu
            {
                PackageTempC = 73.6,
                CoreTempsC = new double[] { 73.6, 63.2 },
                PowerW = 40.2,
                VoltageV = 0.54,
                FanRpm = 1692,
                PumpRpm = null
            },
            Motherboard = new MotherboardData
            {
                Name = "MSI MAG B650M MORTAR WIFI (MS-7D76)",
                Sensors = new List<MotherboardSensorData>
                {
                    new() { Name = "System", Type = "temperature", Value = 49.5 }
                }
            },
            DiskTemps = new Dictionary<string, double> { ["C:"] = 45, ["D:"] = 34 },
            RepresentativeDiskTempC = 45
        };

        var r = payload.ToReadout();

        Assert.Equal(73.6f, r.CpuTempC!.Value, 3);
        Assert.Equal(new double[] { 73.6, 63.2 }, r.CoreTempsC);
        Assert.Equal(40.2f, r.CpuPowerW!.Value, 3);
        Assert.Equal(0.54f, r.CpuVoltageV!.Value, 3);
        Assert.Equal(1692f, r.CpuFanRpm!.Value, 3);
        Assert.Null(r.CpuPumpRpm);
        Assert.NotNull(r.Motherboard);
        Assert.Equal(45f, r.DiskTemps["C:"], 3);
        Assert.Equal(34f, r.DiskTemps["D:"], 3);
        Assert.Equal(45f, r.RepresentativeDiskTempC!.Value, 3);
        Assert.True(r.HasAny);
    }

    [Fact]
    public void ToReadout_UnavailablePayload_HasNoData()
    {
        var payload = new Tier2Payload { Available = false, Reason = "PawnIO 不可访问" };
        var r = payload.ToReadout();

        Assert.Null(r.CpuTempC);
        Assert.Empty(r.CoreTempsC);
        Assert.Null(r.CpuPowerW);
        Assert.Null(r.CpuVoltageV);
        Assert.Null(r.CpuFanRpm);
        Assert.Null(r.CpuPumpRpm);
        Assert.Null(r.Motherboard);
        Assert.Empty(r.DiskTemps);
        Assert.Null(r.RepresentativeDiskTempC);
        Assert.False(r.HasAny);
    }

    [Fact]
    public void ToReadout_AvailableButNullCpu_DoesNotThrow()
    {
        var payload = new Tier2Payload { Available = true, Cpu = null };
        var r = payload.ToReadout();
        Assert.False(r.HasAny);
    }

    // --- 合并进快照后：tier-2 字段填充，映射不到的卷温度仍为 null ---
    [Fact]
    public void MergedSnapshot_DiskTempOnlyForMappedVolumes()
    {
        var map = new SensorMap();
        var builder = new SnapshotBuilder(map);

        var tier1 = new SnapshotBuilder.Tier1Readout
        {
            CpuLoadPercent = 20,
            Disks = new List<SnapshotBuilder.DiskEntry>
            {
                new() { Name = "C:", UsedPercent = 52, ReadBytesPerSec = 1000 },
                new() { Name = "Y:", UsedPercent = 65, ReadBytesPerSec = 0 }
            }
        };

        var payload = new Tier2Payload
        {
            Available = true,
            Cpu = new Tier2Cpu { PackageTempC = 70 },
            DiskTemps = new Dictionary<string, double> { ["C:"] = 45 }
        };

        var snap = builder.Build("Ryzen",
            Array.Empty<LibreHardwareMonitor.Hardware.IHardware>(),
            tier1, payload.ToReadout(), elevated: true, tier2Available: true, tier2Reason: null);

        Assert.Equal(45, snap.Disks[0].TemperatureC);
        Assert.Null(snap.Disks[1].TemperatureC);   // Y: 无映射 → null，不猜测
        Assert.Equal(70, snap.Cpu.PackageTempC);
        Assert.True(snap.Elevated);
    }
}
