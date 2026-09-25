// HardwareRulesTests.cs —— 上游 HardwareRules.cs 的优先级 / Arc 判定 / 共享显存策略。
// CONTRACT-v3.md §5.1 冻结：Basic Render=100、Nvidia=0、AMD 独显=0、
// "AMD Radeon(TM) Graphics"=2、Arc 独显=0、Arc 核显=1、Iris=2、UHD=3、其他=4。

using LibreHardwareMonitor.Hardware;
using LhmSidecar.Hardware;
using Xunit;

namespace LhmSidecar.Tests;

public class HardwareRulesTests
{
    private static int Priority(HardwareType type, string name)
        => HardwareRules.GetHwPriority(new FakeHardware(name, type));

    [Fact]
    public void BasicRender_IsLowest()                       // 上游 HardwareRules.cs:29
        => Assert.Equal(100, Priority(HardwareType.GpuIntel, "Microsoft Basic Render Driver"));

    [Fact]
    public void Nvidia_IsTop()                              // 上游 :32
        => Assert.Equal(0, Priority(HardwareType.GpuNvidia, "NVIDIA GeForce RTX 3060"));

    [Fact]
    public void Amd_DiscreteIsTop()                         // 上游 :40
        => Assert.Equal(0, Priority(HardwareType.GpuAmd, "AMD Radeon RX 7900 XTX"));

    [Fact]
    public void Amd_GenericRadeonGraphics_IsIntegrated()    // 上游 :38（严格 Equals）
        => Assert.Equal(2, Priority(HardwareType.GpuAmd, "AMD Radeon(TM) Graphics"));

    [Fact]
    public void Amd_GenericName_IsNotContainsMatch()
        => Assert.Equal(0, Priority(HardwareType.GpuAmd, "AMD Radeon(TM) Graphics 780M"));

    // ⚠ 与 IsDiscreteArc 同一上游缺陷：官方核显名含 " A" → 被当成独显（优先级 0 而非 1）。
    // 见 IsDiscreteArc_UpstreamQuirk_NamesEmbeddingSpaceA 的说明。
    [Theory]
    [InlineData("Intel Arc A770", 0)]                 // Arc 独显 A 系列 —— 上游 :50
    [InlineData("Intel Arc B580", 0)]                 // B 系列
    [InlineData("Intel Arc Pro A40", 0)]              // Pro 系列
    [InlineData("Intel Arc Graphics", 0)]             // 上游实际=0（契约 §5.1 散列描述意图为 1）
    [InlineData("Intel Arc 140V", 0)]                 // 上游实际=0（同上）
    [InlineData("Arc(TM) Graphics", 1)]               // 不含 " A" → 正确落到核显分支
    public void IntelArc_Priority(string name, int expected)
        => Assert.Equal(expected, Priority(HardwareType.GpuIntel, name));

    [Fact]
    public void Iris_IsTwo()                                // 上游 :56
        => Assert.Equal(2, Priority(HardwareType.GpuIntel, "Intel Iris Xe Graphics"));

    [Fact]
    public void Uhd_IsThree()                               // 上游 :59
        => Assert.Equal(3, Priority(HardwareType.GpuIntel, "Intel UHD Graphics 630"));

    [Fact]
    public void UnknownGpu_IsFour()                         // 上游 :63
        => Assert.Equal(4, Priority(HardwareType.GpuIntel, "Some Unknown Adapter"));

    // --- IsDiscreteArc —— 上游 HardwareRules.cs:69-78 ---
    //
    // ⚠ 上游启发式缺陷（移植时刻意保留，未"改进"）：
    //   `Has(name, " A")` 匹配的是任意位置的空格+A。Intel 核显的官方名
    //   "Intel Arc Graphics" / "Intel Arc 140V" 在 "Inte| A|rc" 处正好含 " A"，
    //   因此上游 IsDiscreteArc 对这两者返回 **true**，GetHwPriority 也随之返回 0（独显）
    //   而不是 1（核显）。CONTRACT-v3.md §5.1 的散列式描述（"Arc 核显=1"）描述的是上游
    //   意图；任务与契约同时硬性要求"映射语义与上游逐条一致，不要改进关键字列表"，
    //   故此处按上游实际行为断言；如需修正应改契约与上游，而不是在移植层私改。
    [Theory]
    [InlineData("Intel Arc A770", true)]
    [InlineData("Intel Arc B580", true)]
    [InlineData("Intel Arc Pro A40", true)]
    [InlineData("NVIDIA GeForce RTX 3060", false)]
    [InlineData("", false)]
    public void IsDiscreteArc_Cases(string name, bool expected)
        => Assert.Equal(expected, HardwareRules.IsDiscreteArc(name));

    [Theory]
    [InlineData("Intel Arc Graphics")]   // 无空格版官方名 → 含 " A" → 上游判为独显
    [InlineData("Intel Arc 140V")]
    public void IsDiscreteArc_UpstreamQuirk_NamesEmbeddingSpaceA(string name)
    {
        Assert.True(HardwareRules.Has(name, " A"));          // 证明缺陷成因
        Assert.True(HardwareRules.IsDiscreteArc(name));      // 上游真实行为
    }

    [Fact]
    public void IsDiscreteArc_NoSpaceA_IsNotDiscrete()
    {
        // "Arc Graphics" 不含 " A"（"Arc" 后是空格+G）→ 上游返回 false
        Assert.False(HardwareRules.Has("Arc Graphics", " A"));
        Assert.False(HardwareRules.IsDiscreteArc("Arc Graphics"));
    }

    [Fact]
    public void IsDiscreteArc_IntegratedNameWithoutSpaceA_IsFalse()
    {
        // 用不存在 " A" 子串的命名（如 "Arc(TM) Graphics"）才会被判为核显
        Assert.False(HardwareRules.IsDiscreteArc("Arc(TM) Graphics"));
        Assert.Equal(1, Priority(HardwareType.GpuIntel, "Arc(TM) Graphics"));
    }

    // --- ShouldUseSharedMemory —— 上游 HardwareRules.cs:83-97 ---
    [Fact]
    public void ShouldUseSharedMemory_IntelIntegrated_True()
        => Assert.True(HardwareRules.ShouldUseSharedMemory(new FakeHardware("Intel UHD Graphics 630", HardwareType.GpuIntel)));

    [Fact]
    public void ShouldUseSharedMemory_IntelDiscreteArc_False()
        => Assert.False(HardwareRules.ShouldUseSharedMemory(new FakeHardware("Intel Arc A770", HardwareType.GpuIntel)));

    [Theory]
    [InlineData(HardwareType.GpuNvidia, "NVIDIA GeForce RTX 3060")]
    [InlineData(HardwareType.GpuAmd, "AMD Radeon RX 7900 XTX")]
    [InlineData(HardwareType.GpuAmd, "AMD Radeon(TM) Graphics")]
    public void ShouldUseSharedMemory_OtherVendors_False(HardwareType type, string name)
        => Assert.False(HardwareRules.ShouldUseSharedMemory(new FakeHardware(name, type)));

    // --- Has（空值语义）—— 上游 :14-18 ---
    [Fact]
    public void Has_EmptyInputs_AreFalse()
    {
        Assert.False(HardwareRules.Has(null, "x"));
        Assert.False(HardwareRules.Has("x", null));
        Assert.False(HardwareRules.Has("", ""));
        Assert.True(HardwareRules.Has("GPU CORE", "core"));   // 忽略大小写
    }
}
