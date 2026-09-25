// LhmCollectorTests.cs —— 内存节点分离（冷启动提速）的行为锁定。
//
// 背景（2026-09-25 实测，发布版 v2.1.0）：
//   LHM 的 Memory 节点要逐条读取 SPD，`Computer.Open()` 因此从 ~516ms 涨到 ~2859ms，
//   而它只提供 best-effort 的内存型号名（权威值来自 GlobalMemoryStatusEx，
//   见 CONTRACT-v3.md §3 memory 注释）。把它留在主树里，等于让首屏的 CPU 频率/
//   GPU/温度一起多等 ~2.5s（A/B 实测：首份完整数据 5996ms → 3509ms）。
//
// 本文件锁定三条不变量：
//   1. 主树**不含** Memory 节点（否则提速失效）；
//   2. 内存型号改由独立探针提供，且该探针**幂等**（重复调用不得重复开树）；
//   3. `Close()` 使在途探针结果作废（代次守卫），避免句柄泄漏。
//
// 测试不注入真实 LHM 驱动：探针工厂返回"空树"（`IsMemoryEnabled = false`），
// 实测 Open+Close 仅 ~15ms，且不触碰任何 ring0 设备。

using LhmSidecar.Hardware;
using LibreHardwareMonitor.Hardware;
using Xunit;

namespace LhmSidecar.Tests;

public class LhmCollectorTests
{
    /// <summary>
    /// 主树必须关闭 Memory 节点 —— 这是冷启动提速的全部来源。
    /// 若有人把它改回 true，本用例失败，提醒先读 CreateComputer 注释里的实测数据。
    /// </summary>
    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public void CreateComputer_主树不含内存节点(bool includeTier2)
    {
        var c = LhmCollector.CreateComputer(includeTier2);
        Assert.False(c.IsMemoryEnabled);
    }

    /// <summary>tier-2 组仍按提权与否开关，且 CPU/GPU/网络/电池恒开（契约 §5.2）。</summary>
    [Fact]
    public void CreateComputer_tier2开关与tier1恒开()
    {
        var noTier2 = LhmCollector.CreateComputer(includeTier2: false);
        Assert.True(noTier2.IsCpuEnabled);
        Assert.True(noTier2.IsGpuEnabled);
        Assert.True(noTier2.IsNetworkEnabled);
        Assert.True(noTier2.IsBatteryEnabled);
        Assert.False(noTier2.IsMotherboardEnabled);
        Assert.False(noTier2.IsStorageEnabled);
        Assert.False(noTier2.IsControllerEnabled);

        var withTier2 = LhmCollector.CreateComputer(includeTier2: true);
        Assert.True(withTier2.IsMotherboardEnabled);
        Assert.True(withTier2.IsStorageEnabled);
        Assert.True(withTier2.IsControllerEnabled);
    }

    /// <summary>探针幂等：重复调用只开一次树。</summary>
    [Fact]
    public void StartMemoryProbe_重复调用只开一次树()
    {
        using var collector = new LhmCollector();
        int opened = 0;
        collector.MemoryComputerFactoryForTesting = () =>
        {
            Interlocked.Increment(ref opened);
            return new Computer { IsMemoryEnabled = false };   // 空树，~15ms
        };

        collector.StartMemoryProbe();
        collector.StartMemoryProbe();
        collector.StartMemoryProbe();

        Assert.True(collector.MemoryProbeStarted);

        // 等待探针完成（最多 5s）
        var deadline = DateTime.UtcNow.AddSeconds(5);
        while (collector.MemoryProbeComputer == null && DateTime.UtcNow < deadline)
        {
            Thread.Sleep(20);
        }

        Assert.NotNull(collector.MemoryProbeComputer);
        Assert.Equal(1, opened);
    }

    /// <summary>
    /// 代次守卫：`Close()` 自增代次，使在途探针的结果被丢弃。
    /// 若不实现该守卫，探针会在 Close 之后把树赋回来 → 驱动句柄泄漏。
    /// </summary>
    [Fact]
    public void Close_自增代次并使在途探针作废()
    {
        var collector = new LhmCollector();
        int gen0 = collector.MemoryProbeGeneration;

        collector.Close();

        Assert.Equal(gen0 + 1, collector.MemoryProbeGeneration);
        Assert.False(collector.MemoryProbeStarted);
        Assert.Null(collector.MemoryProbeComputer);
    }

    /// <summary>Close 之后探针可再次启动（代次复位语义）。</summary>
    [Fact]
    public void Close后探针可再次启动()
    {
        using var collector = new LhmCollector();
        collector.MemoryComputerFactoryForTesting = () => new Computer { IsMemoryEnabled = false };

        collector.StartMemoryProbe();
        var deadline = DateTime.UtcNow.AddSeconds(5);
        while (collector.MemoryProbeComputer == null && DateTime.UtcNow < deadline) Thread.Sleep(20);
        Assert.NotNull(collector.MemoryProbeComputer);

        collector.Close();
        Assert.False(collector.MemoryProbeStarted);

        collector.StartMemoryProbe();
        Assert.True(collector.MemoryProbeStarted);
    }

    /// <summary>主树未 Open 时，内存读取走探针树且安全返回 null。</summary>
    [Fact]
    public void 探针未完成时内存读取返回null()
    {
        using var collector = new LhmCollector();
        Assert.Null(collector.ReadMemoryName());
        var (total, used) = collector.ReadMemoryFromLhm();
        Assert.Null(total);
        Assert.Null(used);
    }

    /// <summary>
    /// 内存型号必须取 **SPD 型号节点**，而不是 "Total Memory" 泛称。
    ///
    /// 实测（2026-09-25）内存树 4 个节点：Virtual Memory(3 传感器)、
    /// Total Memory(3)、A-DATA ... (#1)(21)、A-DATA ... (#3)(21)。
    /// 旧实现取"第一个非 virtual"→ 返回 "Total Memory"，与契约 §3
    /// 及前端 `model_name`（内存型号）语义不符。
    /// </summary>
    [Fact]
    public void SelectMemoryModelNode_优先取SPD型号节点而非TotalMemory()
    {
        var virt = new FakeHardware("Virtual Memory", HardwareType.Memory);
        AddSensors(virt, 3);
        var total = new FakeHardware("Total Memory", HardwareType.Memory);
        AddSensors(total, 3);
        var spd1 = new FakeHardware("A-DATA Technology - AX5U6400C3216G-BB300 (#1)", HardwareType.Memory);
        AddSensors(spd1, 21);
        var spd2 = new FakeHardware("A-DATA Technology - AX5U6400C3216G-BB300 (#3)", HardwareType.Memory);
        AddSensors(spd2, 21);

        var picked = LhmCollector.SelectMemoryModelNode(new[] { virt, total, spd1, spd2 });

        Assert.NotNull(picked);
        Assert.Contains("A-DATA", picked!.Name);
        Assert.DoesNotContain("Total Memory", picked.Name);
    }

    /// <summary>只有虚拟内存节点时退化取它，而不是返回 null（永不抛异常）。</summary>
    [Fact]
    public void SelectMemoryModelNode_仅虚拟内存时退化取用()
    {
        var virt = new FakeHardware("Virtual Memory", HardwareType.Memory);
        AddSensors(virt, 3);

        var picked = LhmCollector.SelectMemoryModelNode(new[] { virt });

        Assert.NotNull(picked);
        Assert.Equal("Virtual Memory", picked!.Name);
    }

    /// <summary>空列表返回 null，不抛异常。</summary>
    [Fact]
    public void SelectMemoryModelNode_空列表返回null()
    {
        Assert.Null(LhmCollector.SelectMemoryModelNode(Array.Empty<IHardware>()));
    }

    /// <summary>给假硬件挂 n 个传感器（传感器数量是"是否为 SPD 节点"的判据）。</summary>
    private static void AddSensors(FakeHardware hw, int n)
    {
        for (int i = 0; i < n; i++)
        {
            hw.With("s" + i, SensorType.Temperature, 1f);
        }
    }
}
