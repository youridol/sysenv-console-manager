// ComponentProcessorTests.cs —— 上游 ComponentProcessor.cs 的聚合/熔断/Zen5 修正。
// CONTRACT-v3.md §5.1 冻结：CPU.Power > 600W、GPU.Clock > 6000MHz、GPU.Power > 1200W
// 一律丢弃（返回 null）；Zen 5 频率修正：bus ∈ (1,20) 且 factor = 100/bus ∈ (2,10)。

using LibreHardwareMonitor.Hardware;
using LhmSidecar.Hardware;
using Xunit;

namespace LhmSidecar.Tests;

public class ComponentProcessorTests
{
    private static ComponentProcessor ProcessorFor(FakeComputer computer, SensorMap map)
    {
        map.Rebuild(computer);
        return new ComponentProcessor(computer, map);
    }

    // --- CPU 负载：多核平均（过滤 Core + # 且排除 Total/SOC/Max/Average）—— 上游 :37-69 ---
    [Fact]
    public void CpuLoad_AveragesCoreSensors()
    {
        var cpu = new FakeHardware("AMD Ryzen 7 5800X", HardwareType.Cpu)
            .With("CPU Core #1", SensorType.Load, 10f)
            .With("CPU Core #2", SensorType.Load, 30f)
            .With("CPU Core #3", SensorType.Load, 50f)
            .With("CPU Core #4", SensorType.Load, 90f);

        var map = new SensorMap();
        var proc = ProcessorFor(new FakeComputer().With(cpu), map);

        Assert.Equal(45f, proc.GetCpuLoad());   // (10+30+50+90)/4
    }

    [Fact]
    public void CpuLoad_ExcludesTotalSocMaxAverage()
    {
        var cpu = new FakeHardware("AMD Ryzen 7 5800X", HardwareType.Cpu)
            .With("CPU Core #1", SensorType.Load, 20f)
            .With("CPU Core #2", SensorType.Load, 40f)
            .With("CPU Total", SensorType.Load, 99f)      // 排除
            .With("CPU Core SOC", SensorType.Load, 99f)   // 排除
            .With("CPU Core Max", SensorType.Load, 99f)   // 排除
            .With("CPU Core Average", SensorType.Load, 99f); // 排除

        var map = new SensorMap();
        var proc = ProcessorFor(new FakeComputer().With(cpu), map);

        Assert.Equal(30f, proc.GetCpuLoad());
    }

    [Fact]
    public void CpuLoad_NullWhenNoCoreSensors()
    {
        var cpu = new FakeHardware("AMD Ryzen 7 5800X", HardwareType.Cpu)
            .With("CPU Total", SensorType.Load, 12f);

        var map = new SensorMap();
        var proc = ProcessorFor(new FakeComputer().With(cpu), map);

        Assert.Null(proc.GetCpuLoad());
    }

    // --- CPU 温度：核心最大值，排除 Distance/Average/Max —— 上游 :74-107 ---
    [Fact]
    public void CpuTemp_TakesMaxCoreTemperature()
    {
        var cpu = new FakeHardware("AMD Ryzen 7 5800X", HardwareType.Cpu)
            .With("Core (Tctl/Tdie)", SensorType.Temperature, 62f)
            .With("CCD1 (Tdie)", SensorType.Temperature, 55f)
            .With("Distance to TjMax", SensorType.Temperature, 95f)  // 排除
            .With("Core Average", SensorType.Temperature, 20f)       // 排除
            .With("Core Max", SensorType.Temperature, 20f);          // 排除

        var map = new SensorMap();
        var proc = ProcessorFor(new FakeComputer().With(cpu), map);

        Assert.Equal(62f, proc.GetCpuTemp());
    }

    [Fact]
    public void CpuTemp_IgnoresNonPositive()
    {
        var cpu = new FakeHardware("AMD Ryzen 7 5800X", HardwareType.Cpu)
            .With("Core #1", SensorType.Temperature, 0f)
            .With("Core #2", SensorType.Temperature, -5f);

        var map = new SensorMap();
        var proc = ProcessorFor(new FakeComputer().With(cpu), map);

        Assert.Null(proc.GetCpuTemp());
    }

    // --- CPU 功耗熔断：> 600W 丢弃 —— 上游 ComponentProcessor.cs:148（CONTRACT §5.1）---
    [Theory]
    [InlineData(65f, 65f)]
    [InlineData(600f, 600f)]      // 等于 600W 保留（严格 >）
    public void CpuPower_BelowOrEqual600W_Kept(float input, float expected)
    {
        var cpu = new FakeHardware("AMD Ryzen 7 5800X", HardwareType.Cpu)
            .With("CPU Package", SensorType.Power, input);

        var map = new SensorMap();
        var proc = ProcessorFor(new FakeComputer().With(cpu), map);

        Assert.Equal(expected, proc.GetCompositeValue("CPU.Power", map.GetInternalMap()));
    }

    [Theory]
    [InlineData(600.1f)]
    [InlineData(1200f)]
    public void CpuPower_Above600W_Discarded(float input)
    {
        var cpu = new FakeHardware("AMD Ryzen 7 5800X", HardwareType.Cpu)
            .With("CPU Package", SensorType.Power, input);

        var map = new SensorMap();
        var proc = ProcessorFor(new FakeComputer().With(cpu), map);

        Assert.Null(proc.GetCompositeValue("CPU.Power", map.GetInternalMap()));
    }

    // --- GPU 频率熔断：> 6000MHz 丢弃 —— 上游 :165 ---
    [Theory]
    [InlineData(1800f, 1800f)]
    [InlineData(6000f, 6000f)]     // 等于 6000 保留
    public void GpuClock_AtOrBelow6000_Kept(float input, float expected)
    {
        var gpu = new FakeHardware("NVIDIA GeForce RTX 3060", HardwareType.GpuNvidia)
            .With("GPU Core", SensorType.Clock, input);

        Assert.Equal(expected, ComponentProcessor.GetGpuClockMhz(gpu));
    }

    [Theory]
    [InlineData(6001f)]
    [InlineData(8000f)]
    public void GpuClock_Above6000_Discarded(float input)
    {
        var gpu = new FakeHardware("NVIDIA GeForce RTX 3060", HardwareType.GpuNvidia)
            .With("GPU Core", SensorType.Clock, input);

        Assert.Null(ComponentProcessor.GetGpuClockMhz(gpu));
    }

    [Fact]
    public void GpuClock_MatchesGraphicsCoreShaderKeyword()
    {
        var gpu = new FakeHardware("NVIDIA GeForce RTX 3060", HardwareType.GpuNvidia)
            .With("GPU Shader", SensorType.Clock, 1500f);
        Assert.Equal(1500f, ComponentProcessor.GetGpuClockMhz(gpu));

        var gpu2 = new FakeHardware("AMD Radeon RX 7900 XTX", HardwareType.GpuAmd)
            .With("GPU Graphics", SensorType.Clock, 2400f);
        Assert.Equal(2400f, ComponentProcessor.GetGpuClockMhz(gpu2));
    }

    [Fact]
    public void GpuClock_IgnoresMemoryClockKeyword()
    {
        var gpu = new FakeHardware("NVIDIA GeForce RTX 3060", HardwareType.GpuNvidia)
            .With("GPU Memory", SensorType.Clock, 7000f);
        Assert.Null(ComponentProcessor.GetGpuClockMhz(gpu));
    }

    // --- GPU 功耗熔断：> 1200W 丢弃 —— 上游 :176 ---
    [Theory]
    [InlineData(200f, 200f)]
    [InlineData(1200f, 1200f)]
    public void GpuPower_AtOrBelow1200_Kept(float input, float expected)
    {
        var gpu = new FakeHardware("NVIDIA GeForce RTX 3060", HardwareType.GpuNvidia)
            .With("GPU Package", SensorType.Power, input);

        Assert.Equal(expected, ComponentProcessor.GetGpuPowerW(gpu));
    }

    [Theory]
    [InlineData(1200.5f)]
    [InlineData(2000f)]
    public void GpuPower_Above1200_Discarded(float input)
    {
        var gpu = new FakeHardware("NVIDIA GeForce RTX 3060", HardwareType.GpuNvidia)
            .With("GPU Package", SensorType.Power, input);

        Assert.Null(ComponentProcessor.GetGpuPowerW(gpu));
    }

    // --- GPU.Clock 走 CachedGpu（代表卡）—— 上游 :157 ---
    [Fact]
    public void GpuClock_UsesCachedGpu()
    {
        var nvidia = new FakeHardware("NVIDIA GeForce RTX 3060", HardwareType.GpuNvidia)
            .With("GPU Core", SensorType.Clock, 1500f);
        var amd = new FakeHardware("AMD Radeon RX 7900 XTX", HardwareType.GpuAmd)
            .With("GPU Core", SensorType.Clock, 2400f);

        var map = new SensorMap();
        var proc = ProcessorFor(new FakeComputer().With(amd).With(nvidia), map);

        // 优先级 0 的两张卡：AMD 先注册 → CachedGpu = AMD（上游 :152-155 第一个即为代表）
        Assert.Same(amd, map.CachedGpu);
        Assert.Equal(2400f, proc.GetCompositeValue("GPU.Clock", map.GetInternalMap()));
    }

    // --- Zen 5 频率修正 —— 上游 ComponentProcessor.cs:119-129（CONTRACT §5.1）---
    [Fact]
    public void Zen5Correction_ScalesCoreClock_WhenBusInRangeAndFactorInRange()
    {
        // bus=10 → factor = 100/10 = 10 → factor 必须严格 < 10，故不在修正区间 → 用 bus=20? 也不在
        // 选 bus=12.5 → factor=8 ∈ (2,10) → 修正生效
        var cpu = new FakeHardware("AMD Ryzen 9 9950X", HardwareType.Cpu)
            .With("Core #1", SensorType.Clock, 100f)
            .With("Bus Speed", SensorType.Clock, 12.5f);

        var map = new SensorMap();
        var proc = ProcessorFor(new FakeComputer().With(cpu), map);

        // 100MHz × 8 = 800MHz > 400 → 进入平均
        Assert.Equal(800f, proc.GetCompositeValue("CPU.Clock", map.GetInternalMap()));
    }

    [Fact]
    public void Zen5Correction_NotApplied_WhenBusOutsideRange()
    {
        // bus = 200（>20）→ 不修正
        var cpu = new FakeHardware("AMD Ryzen 9 9950X", HardwareType.Cpu)
            .With("Core #1", SensorType.Clock, 4200f)
            .With("Bus Speed", SensorType.Clock, 200f);

        var map = new SensorMap();
        var proc = ProcessorFor(new FakeComputer().With(cpu), map);

        Assert.Equal(4200f, proc.GetCompositeValue("CPU.Clock", map.GetInternalMap()));
    }

    [Fact]
    public void Zen5Correction_NotApplied_WhenFactorOutOfRange()
    {
        // bus = 10 → factor = 10.0，必须严格 < 10 → 不修正
        var cpu = new FakeHardware("AMD Ryzen 9 9950X", HardwareType.Cpu)
            .With("Core #1", SensorType.Clock, 4200f)
            .With("Bus Speed", SensorType.Clock, 10f);

        var map = new SensorMap();
        var proc = ProcessorFor(new FakeComputer().With(cpu), map);

        Assert.Equal(4200f, proc.GetCompositeValue("CPU.Clock", map.GetInternalMap()));
    }

    [Fact]
    public void CpuClock_ClocksAtOrBelow400Mhz_AreExcludedFromAverage()
    {
        // 上游 :136：只有 clk > 400 才进入平均；maxRaw 仍取最大值
        var cpu = new FakeHardware("AMD Ryzen 7 5800X", HardwareType.Cpu)
            .With("Core #1", SensorType.Clock, 300f)
            .With("Core #2", SensorType.Clock, 1000f);

        var map = new SensorMap();
        var proc = ProcessorFor(new FakeComputer().With(cpu), map);

        Assert.Equal(1000f, proc.GetCompositeValue("CPU.Clock", map.GetInternalMap()));
    }

    [Fact]
    public void CpuClock_ReturnsMaxRaw_WhenAllClocksAtOrBelow400()
    {
        var cpu = new FakeHardware("AMD Ryzen 7 5800X", HardwareType.Cpu)
            .With("Core #1", SensorType.Clock, 300f)
            .With("Core #2", SensorType.Clock, 380f);

        var map = new SensorMap();
        var proc = ProcessorFor(new FakeComputer().With(cpu), map);

        // count == 0 → 返回 maxRaw（上游 :140）
        Assert.Equal(380f, proc.GetCompositeValue("CPU.Clock", map.GetInternalMap()));
    }

    [Fact]
    public void CpuClock_NullWhenNoCoreCache()
    {
        var cpu = new FakeHardware("AMD Ryzen 7 5800X", HardwareType.Cpu)
            .With("Bus Speed", SensorType.Clock, 100f);

        var map = new SensorMap();
        var proc = ProcessorFor(new FakeComputer().With(cpu), map);

        Assert.Null(proc.GetCompositeValue("CPU.Clock", map.GetInternalMap()));
    }

    // --- ClearCache 后重新收集 —— 上游 :28-32 ---
    [Fact]
    public void ClearCache_ForcesRecollectionOfLoadSensors()
    {
        var cpu = new FakeHardware("AMD Ryzen 7 5800X", HardwareType.Cpu)
            .With("CPU Core #1", SensorType.Load, 20f);

        var map = new SensorMap();
        var proc = ProcessorFor(new FakeComputer().With(cpu), map);

        Assert.Equal(20f, proc.GetCpuLoad());
        proc.ClearCache();
        Assert.Equal(20f, proc.GetCpuLoad());
    }
}
