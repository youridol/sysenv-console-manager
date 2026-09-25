// FanMapperTests.cs —— 上游 FanMapper.cs 的风扇/水泵/机箱风扇选择顺序与 200 RPM 底噪门槛。
// CONTRACT-v3.md §5.1 冻结：200 RPM 底噪门槛；cooler 检测（HardwareType.Cooler 或名称含
// Kraken/Corsair/Liquid/AIO/Cooler）；水泵 >3000 RPM 猜测；
// 机箱风扇 Rear→Chassis→Sys→Case→最低 RPM。

using LibreHardwareMonitor.Hardware;
using LhmSidecar.Hardware;
using Xunit;

namespace LhmSidecar.Tests;

public class FanMapperTests
{
    private static Dictionary<string, ISensor> Map(FakeComputer computer)
    {
        var sensorMap = new SensorMap();
        sensorMap.Rebuild(computer);
        return sensorMap.GetInternalMap();
    }

    // --- 200 RPM 底噪门槛（严格 > 200）—— 上游 FanMapper.cs:44 ---
    [Fact]
    public void FansBelowOrEqualTo200Rpm_AreIgnored()
    {
        var mobo = new FakeHardware("B650", HardwareType.Motherboard)
            .With("System Fan", SensorType.Fan, 200f)    // 等于 200 → 排除
            .With("CPU Fan", SensorType.Fan, 0f);        // 0 → 排除

        var map = Map(new FakeComputer().With(mobo));

        Assert.False(map.ContainsKey("CPU.Fan"));
        Assert.False(map.ContainsKey("CASE.Fan"));
        Assert.False(map.ContainsKey("CPU.Pump"));
    }

    [Fact]
    public void FansAbove200Rpm_AreEligible()
    {
        var mobo = new FakeHardware("B650", HardwareType.Motherboard)
            .With("CPU Fan", SensorType.Fan, 812f);

        var map = Map(new FakeComputer().With(mobo));

        Assert.True(map.ContainsKey("CPU.Fan"));
        Assert.Equal(812f, map["CPU.Fan"].Value);
    }

    // --- CPU 风扇选择顺序：名字含 CPU 优先于"第一个"—— 上游 :76-82 ---
    [Fact]
    public void CpuFan_PrefersNameContainingCpu()
    {
        var mobo = new FakeHardware("B650", HardwareType.Motherboard)
            .With("Chassis Fan #1", SensorType.Fan, 900f)   // 先出现
            .With("CPU Fan", SensorType.Fan, 812f);         // 后出现但含 "CPU"

        var map = Map(new FakeComputer().With(mobo));

        Assert.True(map.ContainsKey("CPU.Fan"));
        Assert.Equal(812f, map["CPU.Fan"].Value);
    }

    // --- CPU 风扇：cooler 硬件（名称含 AIO）优先，且排除 Pump 名 —— 上游 :70-73 ---
    [Fact]
    public void CpuFan_PrefersCoolerHardwareFan()
    {
        var mobo = new FakeHardware("B650", HardwareType.Motherboard)
            .With("CPU Fan", SensorType.Fan, 812f);
        var aio = new FakeHardware("NZXT Kraken X63", HardwareType.Cooler)
            .With("Fan", SensorType.Fan, 1100f)
            .With("Pump", SensorType.Fan, 2400f);

        var map = Map(new FakeComputer().With(mobo).With(aio));

        // cooler 硬件上的非 Pump 风扇优先当 CPU 风扇
        Assert.Equal(1100f, map["CPU.Fan"].Value);
        // Pump 命名的风扇转成水泵
        Assert.True(map.ContainsKey("CPU.Pump"));
        Assert.Equal(2400f, map["CPU.Pump"].Value);
    }

    // --- 水泵命名优先级：名字含 Pump/Water/AIO —— 上游 :101-103 ---
    [Fact]
    public void CpuPump_PrefersNamedPump()
    {
        var mobo = new FakeHardware("X670E", HardwareType.Motherboard)
            .With("CPU Fan", SensorType.Fan, 800f)
            .With("Water Pump", SensorType.Fan, 2600f);

        var map = Map(new FakeComputer().With(mobo));

        Assert.True(map.ContainsKey("CPU.Pump"));
        Assert.Equal(2600f, map["CPU.Pump"].Value);
    }

    // --- 水泵智能猜测：>3000 RPM 中取最大 —— 上游 :108-114 ---
    [Fact]
    public void CpuPump_GuessesHighestFanAbove3000Rpm()
    {
        var mobo = new FakeHardware("X670E", HardwareType.Motherboard)
            .With("CPU Fan", SensorType.Fan, 800f)
            .With("Fan #2", SensorType.Fan, 3200f)
            .With("Fan #3", SensorType.Fan, 4200f);

        var map = Map(new FakeComputer().With(mobo));

        Assert.True(map.ContainsKey("CPU.Pump"));
        Assert.Equal(4200f, map["CPU.Pump"].Value);
    }

    [Fact]
    public void CpuPump_NotGuessed_WhenNoFanAbove3000Rpm()
    {
        var mobo = new FakeHardware("X670E", HardwareType.Motherboard)
            .With("CPU Fan", SensorType.Fan, 800f)
            .With("Fan #2", SensorType.Fan, 3000f);   // 等于 3000 → 不猜

        var map = Map(new FakeComputer().With(mobo));

        Assert.False(map.ContainsKey("CPU.Pump"));
    }

    // --- 机箱风扇顺序：Rear → Chassis → Sys → Case —— 上游 :123-126 ---
    // 每行给出"存在的机箱风扇名"，注册顺序刻意从低优先级到高优先级，
    // 证明选择依据是关键字优先级而不是出现顺序。
    [Theory]
    [InlineData("Case Fan", new[] { "Case Fan", "Sys Fan", "Chassis Fan", "Rear Fan" }, "Rear Fan")]
    [InlineData("Sys Fan", new[] { "Sys Fan", "Chassis Fan" }, "Chassis Fan")]
    [InlineData("Chassis Fan", new[] { "Case Fan", "Sys Fan" }, "Sys Fan")]
    [InlineData("Rear Fan", new[] { "Case Fan" }, "Case Fan")]
    public void CaseFan_FollowsKeywordOrder(string label, string[] present, string expected)
    {
        // label 仅为可读性；断言用 expected
        _ = label;
        var mobo = new FakeHardware("B650", HardwareType.Motherboard)
            .With("CPU Fan", SensorType.Fan, 700f);
        foreach (var n in present) mobo.With(n, SensorType.Fan, 900f);

        var map = Map(new FakeComputer().With(mobo));

        Assert.True(map.ContainsKey("CASE.Fan"));
        Assert.Equal(expected, map["CASE.Fan"].Name);
        Assert.Equal(900f, map["CASE.Fan"].Value);
    }

    // --- 机箱风扇兜底：无关键字匹配时取最低 RPM —— 上游 :129-133 ---
    [Fact]
    public void CaseFan_FallsBackToLowestRpm()
    {
        var mobo = new FakeHardware("B650", HardwareType.Motherboard)
            .With("CPU Fan", SensorType.Fan, 1500f)
            .With("Fan #4", SensorType.Fan, 1300f)
            .With("Fan #5", SensorType.Fan, 640f)
            .With("Fan #6", SensorType.Fan, 2200f);

        var map = Map(new FakeComputer().With(mobo));

        Assert.True(map.ContainsKey("CASE.Fan"));
        Assert.Equal(640f, map["CASE.Fan"].Value);
    }

    // --- 兜底同时猜水泵：剩余 >1 时取最高 RPM 当水泵 —— 上游 :133 ---
    [Fact]
    public void CaseFanFallback_AlsoGuessesPumpFromHighestRpm()
    {
        var mobo = new FakeHardware("B650", HardwareType.Motherboard)
            .With("CPU Fan", SensorType.Fan, 1500f)
            .With("Fan #4", SensorType.Fan, 1300f)
            .With("Fan #5", SensorType.Fan, 640f)
            .With("Fan #6", SensorType.Fan, 2200f);

        var map = Map(new FakeComputer().With(mobo));

        Assert.Equal(640f, map["CASE.Fan"].Value);   // 最低
        Assert.Equal(2200f, map["CPU.Pump"].Value);  // 最高（剩余里的）
    }

    // --- 黑名单：CPU/GPU/Storage/Memory/Network 类型上的风扇不参与 —— 上游 :28-34 ---
    [Fact]
    public void BlacklistedHardwareTypes_AreExcluded()
    {
        var cpu = new FakeHardware("AMD Ryzen 7 5800X", HardwareType.Cpu)
            .With("CPU Fan", SensorType.Fan, 900f);
        var gpu = new FakeHardware("NVIDIA GeForce RTX 3060", HardwareType.GpuNvidia)
            .With("GPU Fan", SensorType.Fan, 1200f);
        var storage = new FakeHardware("Samsung SSD 990", HardwareType.Storage)
            .With("Fan", SensorType.Fan, 800f);
        var network = new FakeHardware("Intel Ethernet", HardwareType.Network)
            .With("Fan", SensorType.Fan, 800f);
        var memory = new FakeHardware("Total Memory", HardwareType.Memory)
            .With("Fan", SensorType.Fan, 800f);

        var map = Map(new FakeComputer().With(cpu).With(gpu).With(storage).With(network).With(memory));

        Assert.False(map.ContainsKey("CPU.Fan"));
        Assert.False(map.ContainsKey("CASE.Fan"));
        Assert.False(map.ContainsKey("CPU.Pump"));
        // GPU 风扇由 SensorMap 单独映射到 GPU.Fan（上游 SensorMap.cs:157-160）
        Assert.True(map.ContainsKey("GPU.Fan"));
    }
}
