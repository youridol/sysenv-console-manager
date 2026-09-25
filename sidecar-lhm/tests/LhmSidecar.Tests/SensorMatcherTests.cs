// SensorMatcherTests.cs —— 上游 SensorMatcher.cs 的 name→key 表与排除列表。
// 每条断言后标注上游 file:line，便于对照。

using LibreHardwareMonitor.Hardware;
using LhmSidecar.Hardware;
using Xunit;

namespace LhmSidecar.Tests;

public class SensorMatcherTests
{
    private static string? Match(HardwareType hwType, string hwName, SensorType sType, string sName, float? value = 50f)
    {
        var hw = new FakeHardware(hwName, hwType);
        var s = hw.WithSensor(new FakeSensor(sName, sType, value, hw));
        return SensorMatcher.Match(hw, s);
    }

    // --- CPU 负载：total / package —— 上游 SensorMatcher.cs:30-31 ---
    [Theory]
    [InlineData("CPU Total")]
    [InlineData("CPU Package")]
    public void CpuLoad_MatchesTotalAndPackage(string name)
        => Assert.Equal("CPU.Load", Match(HardwareType.Cpu, "CPU", SensorType.Load, name));

    [Fact]
    public void CpuLoad_DoesNotMatchCoreLoad()   // 核心负载由 ComponentProcessor 多核平均处理
        => Assert.Null(Match(HardwareType.Cpu, "CPU", SensorType.Load, "CPU Core #1"));

    // --- CPU 温度黄金标准 —— 上游 SensorMatcher.cs:37-45 ---
    [Theory]
    [InlineData("CPU Package")]
    [InlineData("Core Average")]
    [InlineData("Core (Tctl/Tdie)")]
    [InlineData("CCD1 (Tdie)")]
    [InlineData("CPU Cores")]
    public void CpuTemp_GoldStandardKeywords(string name)
        => Assert.Equal("CPU.Temp", Match(HardwareType.Cpu, "CPU", SensorType.Temperature, name));

    // --- CPU 温度银牌标准 + 排除列表 —— 上游 SensorMatcher.cs:49-59 ---
    // CONTRACT-v3.md §5.1 明确冻结：排除 soc / vrm / fan / pump / liquid / coolant / distance
    [Theory]
    [InlineData("CPU Core")]                 // 应当匹配
    [InlineData("Core #1")]                  // 应当匹配
    public void CpuTemp_SilverStandard_Matches(string name)
        => Assert.Equal("CPU.Temp", Match(HardwareType.Cpu, "CPU", SensorType.Temperature, name));

    [Theory]
    [InlineData("CPU SOC")]                  // 核显/片上系统 —— 上游 :50
    [InlineData("CPU VRM")]                  // 供电 —— 上游 :51
    [InlineData("CPU Fan")]                  // 风扇 —— 上游 :52
    [InlineData("CPU Pump")]                 // 水泵 —— 上游 :53
    [InlineData("CPU Liquid")]               // 水冷液 —— 上游 :54
    [InlineData("CPU Coolant")]              // 冷却液 —— 上游 :55
    [InlineData("Core Distance to TjMax")]   // 距离 —— 上游 :56
    public void CpuTemp_SilverStandard_Exclusions(string name)
        => Assert.Null(Match(HardwareType.Cpu, "CPU", SensorType.Temperature, name));

    // --- CPU 功耗 —— 上游 :61 ---
    [Theory]
    [InlineData("CPU Package")]
    [InlineData("CPU Cores")]
    public void CpuPower_Matches(string name)
        => Assert.Equal("CPU.Power", Match(HardwareType.Cpu, "CPU", SensorType.Power, name));

    // --- CPU 电压 —— 上游 :63-68 ---
    [Theory]
    [InlineData("CPU Core")]
    [InlineData("Vcore")]
    [InlineData("CPU VID")]
    public void CpuVoltage_Matches(string name)
        => Assert.Equal("CPU.Voltage", Match(HardwareType.Cpu, "CPU", SensorType.Voltage, name));

    [Theory]
    [InlineData("CPU SOC Voltage")]
    [InlineData("CPU GT Voltage")]
    [InlineData("CPU SA Voltage")]
    [InlineData("CPU Aux Voltage")]
    public void CpuVoltage_Exclusions(string name)
        => Assert.Null(Match(HardwareType.Cpu, "CPU", SensorType.Voltage, name));

    // --- GPU 温度 —— 上游 :75 ---
    [Theory]
    [InlineData("GPU Core")]
    [InlineData("GPU Hot Spot")]
    [InlineData("GPU VR SoC")]
    public void GpuTemp_Matches(string name)
        => Assert.Equal("GPU.Temp", Match(HardwareType.GpuNvidia, "NVIDIA GeForce RTX 3060", SensorType.Temperature, name));

    // --- GPU 负载 —— 上游 :74 ---
    [Fact]
    public void GpuLoad_MatchesCoreAndD3D3D()
    {
        Assert.Equal("GPU.Load", Match(HardwareType.GpuNvidia, "RTX 3060", SensorType.Load, "GPU Core"));
        Assert.Equal("GPU.Load", Match(HardwareType.GpuNvidia, "RTX 3060", SensorType.Load, "D3D 3D"));
    }

    // --- 显存：独显（Nvidia）接受 Dedicated/Generic，屏蔽 Shared —— 上游 :101-105 ---
    [Theory]
    [InlineData("GPU Memory Used")]
    [InlineData("D3D Dedicated Memory Used")]
    public void GpuVram_Discrete_MatchesUsed(string name)
        => Assert.Equal("GPU.VRAM.Used", Match(HardwareType.GpuNvidia, "RTX 3060", SensorType.SmallData, name));

    [Fact]
    public void GpuVram_Discrete_BlocksShared()
        => Assert.Null(Match(HardwareType.GpuNvidia, "RTX 3060", SensorType.SmallData, "D3D Shared Memory Used"));

    // --- 显存：Intel 核显只接受 Shared，并可用 Generic 回退，但屏蔽 Dedicated —— 上游 :88-97 ---
    [Fact]
    public void GpuVram_IntegratedIntel_PrefersShared()
    {
        Assert.Equal("GPU.VRAM.Used", Match(HardwareType.GpuIntel, "Intel UHD Graphics 630", SensorType.SmallData, "D3D Shared Memory Used"));
        Assert.Null(Match(HardwareType.GpuIntel, "Intel UHD Graphics 630", SensorType.SmallData, "D3D Dedicated Memory Used"));
    }

    // --- 显存：Intel 独显 Arc 走 Dedicated（ShouldUseSharedMemory=false）—— 上游 :83-97 ---
    [Fact]
    public void GpuVram_DiscreteArc_BlocksShared()
        => Assert.Null(Match(HardwareType.GpuIntel, "Intel Arc A770", SensorType.SmallData, "D3D Shared Memory Used"));

    [Fact]
    public void GpuVramLoad_MatchesMemoryLoad()
        => Assert.Equal("GPU.VRAM.Load", Match(HardwareType.GpuNvidia, "RTX 3060", SensorType.Load, "GPU Memory"));

    // --- 内存：拒绝虚拟内存（判定用硬件名）—— 上游 :116（CONTRACT §5.1 冻结）---
    [Fact]
    public void Memory_RejectsVirtualMemoryNode()
        => Assert.Null(Match(HardwareType.Memory, "Virtual Memory", SensorType.Data, "Memory Used"));

    [Fact]
    public void Memory_AcceptsPhysicalNode()
        => Assert.Equal("MEM.Used", Match(HardwareType.Memory, "Total Memory", SensorType.Data, "Memory Used"));

    // --- 内存：Load / Used / Available(Free) —— 上游 :120-133 ---
    [Theory]
    [InlineData("Memory", "MEM.Load")]
    [InlineData("Load", "MEM.Load")]
    public void Memory_LoadMatches(string sensorName, string expected)
        => Assert.Equal(expected, Match(HardwareType.Memory, "Total Memory", SensorType.Load, sensorName));

    [Theory]
    [InlineData("Memory Used", "MEM.Used")]
    [InlineData("Memory Available", "MEM.Available")]
    [InlineData("Memory Free", "MEM.Available")]
    public void Memory_DataMatches(string sensorName, string expected)
    {
        Assert.Equal(expected, Match(HardwareType.Memory, "Total Memory", SensorType.Data, sensorName));
        Assert.Equal(expected, Match(HardwareType.Memory, "Total Memory", SensorType.SmallData, sensorName));
    }

    // --- 电池：Level 排除 Wear/Degradation —— 上游 :142（CONTRACT §5.1 冻结）---
    [Theory]
    [InlineData("Charge Level")]
    [InlineData("Battery Level")]
    public void Battery_LevelMatches(string name)
        => Assert.Equal("BAT.Percent", Match(HardwareType.Battery, "Battery", SensorType.Level, name));

    [Theory]
    [InlineData("Battery Wear Level")]
    [InlineData("Battery Degradation")]
    public void Battery_LevelExcludesWearAndDegradation(string name)
        => Assert.Null(Match(HardwareType.Battery, "Battery", SensorType.Level, name));

    [Theory]
    [InlineData(SensorType.Power, "BAT.Power")]
    [InlineData(SensorType.Voltage, "BAT.Voltage")]
    [InlineData(SensorType.Current, "BAT.Current")]
    public void Battery_OtherTypes(SensorType type, string expected)
        => Assert.Equal(expected, Match(HardwareType.Battery, "Battery", type, "Battery " + type));

    // --- 无关传感器 → null —— 上游 :159 ---
    [Fact]
    public void UnmatchedSensor_ReturnsNull()
        => Assert.Null(Match(HardwareType.Cpu, "CPU", SensorType.Clock, "Bus Speed"));
}
