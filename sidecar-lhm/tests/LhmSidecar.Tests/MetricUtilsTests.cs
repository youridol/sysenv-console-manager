// MetricUtilsTests.cs —— 上游 MetricUtils.cs 的类型解析、单位、阈值与格式化。
// 默认阈值取自上游 Settings.cs:326-332；历史最大值取自 Settings.cs:122-134。

using LhmSidecar.Hardware;
using Xunit;

namespace LhmSidecar.Tests;

public class MetricUtilsTests
{
    // --- GetType —— 上游 MetricUtils.cs:68-96 ---
    [Theory]
    [InlineData("CPU.Load", MetricType.Percent)]
    [InlineData("BAT.Percent", MetricType.Percent)]
    [InlineData("CPU.Temp", MetricType.Temperature)]
    [InlineData("MEM.Load", MetricType.Memory)]
    [InlineData("GPU.VRAM.Used", MetricType.Memory)]
    [InlineData("NET.Up", MetricType.DataSpeed)]
    [InlineData("DISK.Read", MetricType.DataSpeed)]
    [InlineData("CPU.Clock", MetricType.Frequency)]
    [InlineData("CPU.Power", MetricType.Power)]
    [InlineData("CPU.Voltage", MetricType.Voltage)]
    [InlineData("BAT.Current", MetricType.Current)]
    [InlineData("CPU.Fan", MetricType.RPM)]
    [InlineData("CPU.Pump", MetricType.RPM)]
    [InlineData("DATA.DayUp", MetricType.DataSize)]
    [InlineData("FPS", MetricType.FPS)]
    [InlineData("", MetricType.Unknown)]
    [InlineData(null, MetricType.Unknown)]
    [InlineData("SOMETHING.ELSE", MetricType.Unknown)]
    public void GetType_MapsKeys(string? key, MetricType expected)
        => Assert.Equal(expected, MetricUtils.GetType(key));

    // --- 单位表 —— 上游 :240-253 ---
    [Theory]
    [InlineData("CPU.Load", 50f, "%")]
    [InlineData("CPU.Temp", 50f, "°C")]
    [InlineData("CPU.Clock", 4200f, "GHz")]
    [InlineData("CPU.Power", 65f, "W")]
    [InlineData("CPU.Voltage", 1.2f, "V")]
    [InlineData("CPU.Fan", 800f, " RPM")]
    [InlineData("CPU.Pump", 2400f, " RPM")]
    public void GetUnitStr_StandardUnits(string key, float value, string expected)
        => Assert.Equal(expected, MetricUtils.GetUnitStr(key, value, MetricUtils.UnitContext.Panel));

    [Fact]
    public void GetUnitStr_TaskbarUsesCompactRpmAndFps()
    {
        Assert.Equal("R", MetricUtils.GetUnitStr("CPU.Fan", 800f, MetricUtils.UnitContext.Taskbar));
        Assert.Equal(" FPS", MetricUtils.GetUnitStr("FPS", 144f, MetricUtils.UnitContext.Panel));
        Assert.Equal("F", MetricUtils.GetUnitStr("FPS", 144f, MetricUtils.UnitContext.Taskbar));
    }

    [Fact]
    public void GetUnitStr_DataSpeedHasPerSecondSuffix()
    {
        Assert.EndsWith("/s", MetricUtils.GetUnitStr("NET.Up", 1048576f, MetricUtils.UnitContext.Panel));
        Assert.EndsWith("/s", MetricUtils.GetUnitStr("DISK.Read", 1048576f, MetricUtils.UnitContext.Panel));
    }

    // --- 内存单位：容量模式 vs 百分比模式 —— 上游 :185-195 ---
    [Fact]
    public void GetUnitStr_MemoryPercentMode_ReturnsPercent()
        => Assert.Equal("%", MetricUtils.GetUnitStr("MEM.Load", 50f, MetricUtils.UnitContext.Panel, memoryDisplayMode: 0));

    [Fact]
    public void GetUnitStr_MemoryCapacityMode_ReturnsUnitOrGbFallback()
    {
        // totalGB 未知 → "GB" 兜底（上游 :194）
        Assert.Equal("GB", MetricUtils.GetUnitStr("MEM.Load", 50f, MetricUtils.UnitContext.Panel,
            memoryDisplayMode: 1, totalGB: 0));
        // totalGB 已知 → 动态单位
        string unit = MetricUtils.GetUnitStr("MEM.Load", 50f, MetricUtils.UnitContext.Panel,
            memoryDisplayMode: 1, totalGB: 16);
        Assert.Equal("GB", unit);
    }

    // --- FormatDataSizeParts：换级阈值是 1000 而不是 1024 —— 上游 :296-303 ---
    [Theory]
    [InlineData(0, "0.00", "KB")]
    [InlineData(1024, "1.00", "KB")]
    [InlineData(1024 * 1000, "0.98", "MB")]          // >=1000KB 提前换算
    [InlineData(1024.0 * 1024 * 1024, "1.00", "GB")]
    [InlineData(1024.0 * 1024 * 1024 * 1024, "1.00", "TB")]
    public void FormatDataSizeParts_ScalesWith1000Threshold(double bytes, string expectedVal, string expectedUnit)
    {
        var (val, unit) = MetricUtils.FormatDataSizeParts(bytes, -1);
        Assert.Equal(expectedVal, val);
        Assert.Equal(expectedUnit, unit);
    }

    [Fact]
    public void FormatDataSize_AppendsSuffix()
        => Assert.Equal("1.00MB/s", MetricUtils.FormatDataSize(1024.0 * 1024, "/s"));

    // --- FormatValueAdaptive —— 上游 :148-167 ---
    [Theory]
    [InlineData(9.994, "9.99")]
    [InlineData(45.2, "45.2")]
    [InlineData(100.0, "100")]
    public void GetValueStr_AdaptiveDecimals(double value, string expected)
        => Assert.Equal(expected, MetricUtils.GetValueStr("CPU.Load", (float)value));

    [Fact]
    public void GetValueStr_FrequencyIsGigahertzWithOneDecimal()
        => Assert.Equal("4.2", MetricUtils.GetValueStr("CPU.Clock", 4200f));

    [Fact]
    public void GetValueStr_RpmAndFpsAreIntegers()
    {
        Assert.Equal("812", MetricUtils.GetValueStr("CPU.Fan", 812.4f));
        Assert.Equal("144", MetricUtils.GetValueStr("FPS", 143.6f));
    }

    // --- 阈值表（上游 Settings.cs:326-332 默认值）—— 上游 MetricUtils.cs:362-378 ---
    [Fact]
    public void GetThresholds_UsesUpstreamDefaults()
    {
        Assert.Equal((50d, 70d), MetricUtils.GetThresholds("CPU.Temp"));
        Assert.Equal((60d, 85d), MetricUtils.GetThresholds("CPU.Load"));
        Assert.Equal((1d, 2d), MetricUtils.GetThresholds("NET.Up"));
        Assert.Equal((2d, 8d), MetricUtils.GetThresholds("NET.Down"));
        Assert.Equal((2d, 8d), MetricUtils.GetThresholds("DISK.Read"));
        Assert.Equal((60d, 20d), MetricUtils.GetThresholds("BAT.Percent"));  // 反向
        Assert.Equal((512d, 1024d), MetricUtils.GetThresholds("DATA.DayUp"));
    }

    // --- GetState —— 上游 :320-360 ---
    [Fact]
    public void GetState_TemperatureBoundaries()
    {
        Assert.Equal(MetricUtils.StateSafe, MetricUtils.GetState("CPU.Temp", 49.9));
        Assert.Equal(MetricUtils.StateWarn, MetricUtils.GetState("CPU.Temp", 50));
        Assert.Equal(MetricUtils.StateWarn, MetricUtils.GetState("CPU.Temp", 69.9));
        Assert.Equal(MetricUtils.StateCrit, MetricUtils.GetState("CPU.Temp", 70));
    }

    [Fact]
    public void GetState_BatteryLowIsReversed()
    {
        // BAT.Percent 阈值 (60 warn, 20 crit)，越低越严重 —— 上游 :375 / :349-354
        Assert.Equal(MetricUtils.StateSafe, MetricUtils.GetState("BAT.Percent", 80));
        Assert.Equal(MetricUtils.StateWarn, MetricUtils.GetState("BAT.Percent", 50));
        Assert.Equal(MetricUtils.StateCrit, MetricUtils.GetState("BAT.Percent", 10));
    }

    [Fact]
    public void GetState_NegativeBatteryIsSafe()
    {
        // 上游 :323：BAT* 且 value < 0 → 直接 Safe（放电功率为负不该报警）
        Assert.Equal(MetricUtils.StateSafe, MetricUtils.GetState("BAT.Power", -65));
    }

    [Fact]
    public void GetState_NaNIsSafe()
        => Assert.Equal(MetricUtils.StateSafe, MetricUtils.GetState("CPU.Temp", double.NaN));

    // --- 动态范围指标用历史最大值百分比 —— 上游 :332-340 ---
    [Fact]
    public void GetState_DynamicTypesUseAdaptivePercentage()
    {
        // CPU.Clock 历史最大 4200（Settings.cs:123 默认）→ 4200 = 100% → Crit
        Assert.Equal(MetricUtils.StateCrit, MetricUtils.GetState("CPU.Clock", 4200));
        // 3360/4200 = 80% → Warn
        Assert.Equal(MetricUtils.StateWarn, MetricUtils.GetState("CPU.Clock", 3360));
        // 2100/4200 = 50% → Safe
        Assert.Equal(MetricUtils.StateSafe, MetricUtils.GetState("CPU.Clock", 2100));
    }

    // --- GetAdaptivePercentage —— 上游 :404-426 ---
    [Theory]
    [InlineData("CPU.Clock", 2100d, 0.5d)]     // /4200
    [InlineData("CPU.Power", 32.5d, 0.5d)]     // /65
    [InlineData("GPU.Clock", 900d, 0.5d)]      // /1800
    [InlineData("GPU.Power", 50d, 0.5d)]       // /100
    [InlineData("CPU.Voltage", 0.8d, 0.5d)]    // /1.6
    [InlineData("BAT.Power", 50d, 0.5d)]       // /100
    [InlineData("BAT.Voltage", 10d, 0.5d)]     // /20
    [InlineData("BAT.Current", 2.5d, 0.5d)]    // /5
    [InlineData("CPU.Fan", 2000d, 0.5d)]       // /4000
    [InlineData("CPU.Pump", 2500d, 0.5d)]      // /5000
    public void GetAdaptivePercentage_HalvesAtHalfOfRecordedMax(string key, double value, double expected)
        => Assert.Equal(expected, MetricUtils.GetAdaptivePercentage(key, value), 6);

    [Fact]
    public void GetAdaptivePercentage_ClampsToOne()
        => Assert.Equal(1.0, MetricUtils.GetAdaptivePercentage("CPU.Clock", 99999));

    // --- GetProgressValue：最小视觉宽度 5% —— 上游 :384-402 ---
    [Fact]
    public void GetProgressValue_ClampsToMinimumFivePercentAndMaxOne()
    {
        Assert.Equal(0.05, MetricUtils.GetProgressValue("CPU.Load", 0));
        Assert.Equal(0.5, MetricUtils.GetProgressValue("CPU.Load", 50));
        Assert.Equal(1.0, MetricUtils.GetProgressValue("CPU.Load", 150));
        // 负值取绝对值（充电状态）
        Assert.Equal(0.5, MetricUtils.GetProgressValue("BAT.Percent", -50));
    }

    // --- ParseInt / ParseDouble / ToStr —— 上游 :431-433 ---
    [Fact]
    public void ParseHelpers()
    {
        Assert.Equal(42, MetricUtils.ParseInt("42 Core"));
        Assert.Equal(-7, MetricUtils.ParseInt("x-7y"));
        Assert.Equal(0, MetricUtils.ParseInt(null));
        Assert.Equal(3.5, MetricUtils.ParseDouble("3.5 GHz"));
        Assert.Equal("1.2", MetricUtils.ToStr(1.23));
    }
}
