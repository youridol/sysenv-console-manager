// SensorMapTests.cs —— 上游 SensorMap.cs 的注册顺序、冲突解决与主板温度选择。
// CONTRACT-v3.md §5.1 冻结：高优先级显卡胜出；同级优先非 D3D（Vendor）传感器；
// 主板温度顺序 System → Motherboard → Chipset/PCH → 15-68℃ 最大值 → 0-95℃ 最大值。

using LibreHardwareMonitor.Hardware;
using LhmSidecar.Hardware;
using Xunit;

namespace LhmSidecar.Tests;

public class SensorMapTests
{
    // --- 冲突解决：高优先级显卡胜出 —— 上游 SensorMap.cs:189-195 ---
    [Fact]
    public void HigherPriorityGpu_Wins_RegardlessOfOrder()
    {
        // 注册顺序被 GetHwPriority 排序保证（Nvidia=0 在 AMD=0 之前；这里用 Nvidia vs Intel=2）
        var nvidia = new FakeHardware("NVIDIA GeForce RTX 3060", HardwareType.GpuNvidia)
            .With("GPU Core", SensorType.Temperature, 61f);
        var intel = new FakeHardware("Intel UHD Graphics 630", HardwareType.GpuIntel)
            .With("GPU Core", SensorType.Temperature, 42f);

        // 故意把低优先级卡放在 Computer 列表前面，验证排序而非插入顺序生效
        var computer = new FakeComputer().With(intel).With(nvidia);

        var map = new SensorMap();
        map.Rebuild(computer);

        Assert.True(map.TryGetSensor("GPU.Temp", out var sensor));
        Assert.Equal(61f, sensor!.Value);   // Nvidia 优先级 0 < Intel 优先级 3 → Nvidia 胜出

        // 单一代表卡 = Nvidia（上游 SensorMap.cs:152-155）
        Assert.Same(nvidia, map.CachedGpu);
    }

    // --- 同级：Vendor（非 D3D）胜过 D3D —— 上游 SensorMap.cs:199-207 ---
    [Fact]
    public void EqualPriority_VendorBeatsD3D_WhenD3DComesFirst()
    {
        var amd = new FakeHardware("AMD Radeon RX 7900 XTX", HardwareType.GpuAmd)
            .With("D3D 3D", SensorType.Load, 30f)     // 先注册的是 D3D
            .With("GPU Core", SensorType.Load, 77f);  // Vendor 后到

        var map = new SensorMap();
        map.Rebuild(new FakeComputer().With(amd));

        Assert.True(map.TryGetSensor("GPU.Load", out var sensor));
        Assert.Equal(77f, sensor!.Value);   // D3D → 非 D3D 时替换
    }

    [Fact]
    public void EqualPriority_VendorKept_WhenVendorComesFirst()
    {
        var amd = new FakeHardware("AMD Radeon RX 7900 XTX", HardwareType.GpuAmd)
            .With("GPU Core", SensorType.Load, 77f)   // Vendor 先注册
            .With("D3D 3D", SensorType.Load, 30f);    // D3D 后到 → 保持现有

        var map = new SensorMap();
        map.Rebuild(new FakeComputer().With(amd));

        Assert.True(map.TryGetSensor("GPU.Load", out var sensor));
        Assert.Equal(77f, sensor!.Value);
    }

    // --- 主板温度：System > Motherboard —— 上游 SensorMap.cs:227-228 ---
    [Fact]
    public void Mobo_TempPrefersSystem()
    {
        var mobo = new FakeHardware("HUANANZHI X99", HardwareType.Motherboard)
            .With("Motherboard", SensorType.Temperature, 40f)
            .With("System", SensorType.Temperature, 34f);

        var map = new SensorMap();
        map.Rebuild(new FakeComputer().With(mobo));

        Assert.True(map.TryGetSensor("MOBO.Temp", out var sensor));
        Assert.Equal(34f, sensor!.Value);   // System 优先，即使值更低
    }

    // --- 主板温度：无 System/Motherboard 时取 Chipset/PCH —— 上游 :231 ---
    [Fact]
    public void Mobo_TempFallsBackToChipset()
    {
        var mobo = new FakeHardware("Z790", HardwareType.Motherboard)
            .With("PCH", SensorType.Temperature, 55f)
            .With("Temperature #5", SensorType.Temperature, 70f);

        var map = new SensorMap();
        map.Rebuild(new FakeComputer().With(mobo));

        Assert.True(map.TryGetSensor("MOBO.Temp", out var sensor));
        Assert.Equal(55f, sensor!.Value);
    }

    // --- 主板温度兜底：无标准名时取 15-68℃ 区间最大值（而非全局最大）—— 上游 :236-268 ---
    [Fact]
    public void Mobo_TempPrefersSafeRangeMaxOverGlobalMax()
    {
        var mobo = new FakeHardware("X670E", HardwareType.Motherboard)
            .With("Temperature #1", SensorType.Temperature, 45f)   // 合理区间内
            .With("Temperature #2", SensorType.Temperature, 88f)   // 合理区间外，但 <95
            .With("Temperature #3", SensorType.Temperature, 120f); // 异常值，应被 >95 过滤

        var map = new SensorMap();
        map.Rebuild(new FakeComputer().With(mobo));

        Assert.True(map.TryGetSensor("MOBO.Temp", out var sensor));
        Assert.Equal(45f, sensor!.Value);   // 15-68 优先于 0-95
    }

    [Fact]
    public void Mobo_TempFallsBackToWideRange_WhenNoSafeCandidate()
    {
        var mobo = new FakeHardware("X670E", HardwareType.Motherboard)
            .With("Temperature #1", SensorType.Temperature, 80f)   // 仅 0-95 区间
            .With("Temperature #2", SensorType.Temperature, 96f);  // >95 过滤

        var map = new SensorMap();
        map.Rebuild(new FakeComputer().With(mobo));

        Assert.True(map.TryGetSensor("MOBO.Temp", out var sensor));
        Assert.Equal(80f, sensor!.Value);
    }

    // --- SuperIO 子硬件的温度也进入候选 —— 上游 :164-171 ---
    [Fact]
    public void Mobo_TempIncludesSuperIoSubHardware()
    {
        var mobo = new FakeHardware("B650", HardwareType.Motherboard);
        var superIo = new FakeHardware("Nuvoton NCT6798D", HardwareType.SuperIO)
            .With("System", SensorType.Temperature, 36f);
        mobo.WithSub(superIo);

        var map = new SensorMap();
        map.Rebuild(new FakeComputer().With(mobo));

        Assert.True(map.TryGetSensor("MOBO.Temp", out var sensor));
        Assert.Equal(36f, sensor!.Value);
    }

    // --- CPU 核心缓存：Clock 与同名 Load 用 EndsWith 配对 —— 上游 :132-142 ---
    [Fact]
    public void CpuCoreCache_PairsClockWithLoad_UsingEndsWith()
    {
        var cpu = new FakeHardware("AMD Ryzen 7 5800X", HardwareType.Cpu)
            .With("Core #1", SensorType.Clock, 4200f)        // AMD：频率叫 "Core #1"
            .With("CPU Core #1", SensorType.Load, 12f)       // AMD：负载叫 "CPU Core #1"
            .With("Bus Speed", SensorType.Clock, 100f);

        var map = new SensorMap();
        map.Rebuild(new FakeComputer().With(cpu));

        Assert.Single(map.CpuCoreCache);
        Assert.NotNull(map.CpuCoreCache[0].Clock);
        Assert.NotNull(map.CpuCoreCache[0].Load);
        Assert.Equal("Core #1", map.CpuCoreCache[0].Clock!.Name);
        Assert.Equal("CPU Core #1", map.CpuCoreCache[0].Load!.Name);

        // Bus Speed 被单独缓存（供 Zen 5 修正），且不计入核心缓存
        Assert.NotNull(map.CpuBusSpeedSensor);
        Assert.Equal("Bus Speed", map.CpuBusSpeedSensor!.Name);
    }

    // --- PreferredGpu（Identifier 优先）—— 上游 HardwareScanner.cs:130-142 ---
    [Fact]
    public void PreferredGpu_ByIdentifier_LocksMappingToThatGpu()
    {
        var nvidia = new FakeHardware("NVIDIA GeForce RTX 3060", HardwareType.GpuNvidia)
            .With("GPU Core", SensorType.Temperature, 61f);
        var amd = new FakeHardware("AMD Radeon RX 7900 XTX", HardwareType.GpuAmd)
            .With("GPU Core", SensorType.Temperature, 45f);

        var map = new SensorMap();
        // 显式指定 AMD 卡（模拟用户首选显卡配置）
        map.Rebuild(new FakeComputer().With(nvidia).With(amd), amd.Identifier.ToString());

        Assert.True(map.TryGetSensor("GPU.Temp", out var sensor));
        Assert.Equal(45f, sensor!.Value);
        Assert.Same(amd, map.CachedGpu);
    }

    // --- GPU.Fan：Fan 优先，其次 Control —— 上游 :157-160 ---
    [Fact]
    public void GpuFan_PrefersFanOverControl()
    {
        var gpu = new FakeHardware("NVIDIA GeForce RTX 3060", HardwareType.GpuNvidia)
            .With("GPU Fan", SensorType.Fan, 1200f)
            .With("GPU Control", SensorType.Control, 55f);

        var map = new SensorMap();
        map.Rebuild(new FakeComputer().With(gpu));

        Assert.True(map.TryGetSensor("GPU.Fan", out var sensor));
        Assert.Equal(SensorType.Fan, sensor!.SensorType);
    }

    // --- 内存虚拟节点被拒绝（SensorMatcher 用硬件名判定）—— 上游 SensorMatcher.cs:116 ---
    [Fact]
    public void VirtualMemoryNode_ContributesNoKeys()
    {
        var virt = new FakeHardware("Virtual Memory", HardwareType.Memory)
            .With("Memory Used", SensorType.Data, 8f);
        var phys = new FakeHardware("Total Memory", HardwareType.Memory)
            .With("Memory Used", SensorType.Data, 16f);

        var map = new SensorMap();
        map.Rebuild(new FakeComputer().With(virt).With(phys));

        Assert.True(map.TryGetSensor("MEM.Used", out var sensor));
        Assert.Same(phys, sensor!.Hardware);
    }
}
