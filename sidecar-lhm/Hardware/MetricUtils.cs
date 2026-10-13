// Hardware/MetricUtils.cs
//
// 移植自 LiteMonitor（MIT）src/Core/MetricUtils.cs
// 指标类型解析、单位/阈值表、数值格式化。
//
// 移植差异（必要的去 UI 依赖，语义等价）：
//   - 上游 GetPowerStatus() 用 System.Windows.Forms.SystemInformation.PowerStatus
//     （上游 MetricUtils.cs:49-51）。SECM sidecar 是无 WinForms 的控制台进程，
//     改为等价的 kernel32!GetSystemPowerStatus：
//       PowerLineStatus.Online    ⇔ ACLineStatus == 1
//       BatteryChargeStatus.Charging ⇔ (BatteryFlag & 8) != 0
//     3000ms 节流（上游 :44）与 catch 后沿用上次值（上游 :53）保持。
//   - 上游 Settings.Load()（MemoryDisplayMode / Thresholds / DetectedRamTotalGB）
//     改为显式参数 + 上游默认值（Settings.cs:109 / 326-332），便于单测与去设置层依赖。
// 上游行号在每条规则后标注。

using System.Runtime.InteropServices;

namespace LhmSidecar.Hardware;

/// <summary>指标类型（上游 MetricUtils.cs:7-21，顺序与取值保持一致）。</summary>
public enum MetricType
{
    Percent,     // Load, BatPercent
    Temperature, // Temp
    Memory,      // Mem, Vram
    DataSize,    // Data Total
    DataSpeed,   // Net, Disk
    Frequency,   // Clock
    Power,       // Power
    Voltage,     // Voltage
    Current,     // Current
    RPM,         // Fan, Pump
    FPS,         // FPS
    Unknown
}

/// <summary>阈值区间（上游 Settings.cs:336-339 ValueRange 与 326-332 默认值）。</summary>
public readonly record struct ThresholdRange(double Warn, double Crit);

/// <summary>阈值集合（上游 Settings.cs:324-333 默认值原样）。</summary>
public sealed class ThresholdsSet
{
    public ThresholdRange Load { get; init; } = new(60, 85);
    public ThresholdRange Temp { get; init; } = new(50, 70);
    public ThresholdRange DiskIOMB { get; init; } = new(2, 8);
    public ThresholdRange NetUpMB { get; init; } = new(1, 2);
    public ThresholdRange NetDownMB { get; init; } = new(2, 8);
    public ThresholdRange DataUpMB { get; init; } = new(512, 1024);
    public ThresholdRange DataDownMB { get; init; } = new(2048, 5096);
}

/// <summary>LiteMonitor 核心指标处理工具。</summary>
public static class MetricUtils
{
    // =========================================================
    // 1. 全局状态（自动缓存，3 秒刷新）上游 MetricUtils.cs:32-34
    // =========================================================
    private static long _lastPowerCheck;
    private static bool _lastAc;
    private static bool _lastCharging;

    /// <summary>
    /// 当前电源状态（带 3000ms 节流，失败时沿用上次值）。
    /// 上游 MetricUtils.cs:41-56（WinForms → GetSystemPowerStatus 等价移植）。
    /// </summary>
    public static (bool AcOnline, bool IsCharging) GetPowerStatus()
    {
        long now = Environment.TickCount64;
        if (now - _lastPowerCheck > 3000) // 3000ms 自动节流，上游 :44
        {
            _lastPowerCheck = now;
            var raw = QueryPowerStatus();
            if (raw.HasValue)
            {
                _lastAc = raw.Value.AcOnline;
                _lastCharging = raw.Value.IsCharging;
            }
            // 查询失败：吞掉并沿用上次值（上游 :53 catch { }）
        }
        return (_lastAc, _lastCharging);
    }

    /// <summary>无节流地直接查询电源状态（测试可用，BatteryService 用它保证符号即时正确）。</summary>
    public static (bool AcOnline, bool IsCharging)? QueryPowerStatus()
    {
        try
        {
            if (!GetSystemPowerStatus(out var s)) return null;
            // ACLineStatus: 0=Offline, 1=Online, 255=Unknown → PowerLineStatus.Online 仅 1
            bool ac = s.ACLineStatus == 1;
            // BatteryFlag 0x08 = Charging（WinForms BatteryChargeStatus.Charging）
            bool charging = (s.BatteryFlag & 0x08) == 0x08;
            return (ac, charging);
        }
        catch
        {
            return null;
        }
    }

    // 数据量单位（上游 MetricUtils.cs:59）
    private static readonly string[] _dataSizes = { "KB", "MB", "GB", "TB", "PB" };

    public const int StateSafe = 0;  // 上游 :61
    public const int StateWarn = 1;  // 上游 :62
    public const int StateCrit = 2;  // 上游 :63

    // =========================================================
    // 2. 类型解析（上游 MetricUtils.cs:68-96）
    // =========================================================
    public static MetricType GetType(string? key)
    {
        if (string.IsNullOrEmpty(key)) return MetricType.Unknown;

        if (key.Equals("FPS", StringComparison.OrdinalIgnoreCase)) return MetricType.FPS;
        if (key.StartsWith("BAT", StringComparison.OrdinalIgnoreCase))
        {
            if (key.IndexOf("Percent", StringComparison.OrdinalIgnoreCase) >= 0) return MetricType.Percent;
            if (key.IndexOf("Voltage", StringComparison.OrdinalIgnoreCase) >= 0) return MetricType.Voltage;
            if (key.IndexOf("Current", StringComparison.OrdinalIgnoreCase) >= 0) return MetricType.Current;
            if (key.IndexOf("Power", StringComparison.OrdinalIgnoreCase) >= 0) return MetricType.Power;
            return MetricType.Percent;
        }
        if (key.IndexOf("MEM", StringComparison.OrdinalIgnoreCase) >= 0 ||
            key.IndexOf("VRAM", StringComparison.OrdinalIgnoreCase) >= 0) return MetricType.Memory;

        if (key.IndexOf("LOAD", StringComparison.OrdinalIgnoreCase) >= 0) return MetricType.Percent;
        if (key.IndexOf("TEMP", StringComparison.OrdinalIgnoreCase) >= 0) return MetricType.Temperature;
        if (key.IndexOf("CLOCK", StringComparison.OrdinalIgnoreCase) >= 0) return MetricType.Frequency;
        if (key.IndexOf("POWER", StringComparison.OrdinalIgnoreCase) >= 0) return MetricType.Power;
        if (key.IndexOf("VOLTAGE", StringComparison.OrdinalIgnoreCase) >= 0) return MetricType.Voltage;
        if (key.IndexOf("FAN", StringComparison.OrdinalIgnoreCase) >= 0 ||
            key.IndexOf("PUMP", StringComparison.OrdinalIgnoreCase) >= 0) return MetricType.RPM;
        if (key.StartsWith("NET", StringComparison.OrdinalIgnoreCase) ||
            key.StartsWith("DISK", StringComparison.OrdinalIgnoreCase)) return MetricType.DataSpeed;
        if (key.IndexOf("DATA", StringComparison.OrdinalIgnoreCase) >= 0) return MetricType.DataSize;

        return MetricType.Unknown;
    }

    // =========================================================
    // 3. 格式化逻辑（上游 MetricUtils.cs:105-167）
    // =========================================================

    /// <summary>
    /// 纯数值字符串。上游 MetricUtils.cs:105-143。
    /// memoryDisplayMode 对应上游 Settings.MemoryDisplayMode（默认 1=容量模式，Settings.cs:109）。
    /// </summary>
    public static string GetValueStr(string key, float? value, bool compact = false,
        int memoryDisplayMode = 1, double totalGB = 0)
    {
        float v = value ?? 0.0f;
        var type = GetType(key);

        // 1. 内存特殊处理（容量 vs 百分比）上游 :111-127
        if (type == MetricType.Memory)
        {
            if (memoryDisplayMode == 1) // 容量模式
            {
                if (totalGB > 0)
                {
                    // 转换为字节 → 格式化 → 取数值部分（自适应小数 -1）
                    return FormatDataSizeParts((v / 100.0) * totalGB * 1073741824.0, -1).val;
                }
            }
        }

        // 2. 数据量/速度（自动单位缩放）上游 :130-134
        if (type is MetricType.DataSpeed or MetricType.DataSize)
        {
            return FormatDataSizeParts(v, -1).val;
        }

        // 4. 标准模式（Panel）上游 :137-138
        if (type is MetricType.FPS or MetricType.RPM) return $"{v:0}";
        if (type == MetricType.Frequency) return $"{v / 1000f:F1}"; // 频率通常 GHz，一位小数

        return FormatValueAdaptive(v, -1); // 上游 :142
    }

    /// <summary>自适应小数点格式化。上游 MetricUtils.cs:148-167。</summary>
    private static string FormatValueAdaptive(double val, int decimals)
    {
        if (decimals >= 0)
        {
            string format = "0." + new string('0', decimals);
            if (decimals == 0) format = "0";

            // 即使是固定模式，>= 99.95 且 decimals<=1 时按用户习惯显示整数
            if (val >= 99.95 && decimals <= 1) return val.ToString("0");
            return val.ToString(format);
        }

        if (val < 9.995) return val.ToString("0.00");  // 0.00 - 9.99
        if (val < 99.95) return val.ToString("0.0");   // 10.0 - 99.9
        return val.ToString("0");                      // 100+
    }

    /// <summary>单位上下文。上游 MetricUtils.cs:169-175。</summary>
    public enum UnitContext
    {
        Panel,           // 主界面（完整，带空格）
        Taskbar,         // 任务栏（紧凑，无空格）
        SettingsPanel,   // 设置界面 - 主界面预览（带占位符）
        SettingsTaskbar  // 设置界面 - 任务栏预览（带占位符）
    }

    /// <summary>
    /// 单位字符串。上游 MetricUtils.cs:180-254。
    /// memoryDisplayMode 默认 1=容量模式（Settings.cs:109）；totalGB 对应 Settings.DetectedRamTotalGB。
    /// </summary>
    public static string GetUnitStr(string key, float? value, UnitContext context,
        int memoryDisplayMode = 1, double totalGB = 0)
    {
        var type = GetType(key);

        // 1. 内存（GB vs %）上游 :185-195
        if (type == MetricType.Memory)
        {
            if (memoryDisplayMode != 1) return "%";
            if (context is UnitContext.SettingsPanel or UnitContext.SettingsTaskbar) return "{u}";

            return (totalGB > 0 && value.HasValue)
                ? FormatDataSizeParts((value.Value / 100.0) * totalGB * 1073741824.0, -1).unit
                : "GB";
        }

        // 2. 数据（动态单位）上游 :199-220
        if (type is MetricType.DataSpeed or MetricType.DataSize)
        {
            if (context is UnitContext.SettingsPanel or UnitContext.SettingsTaskbar)
            {
                if (type == MetricType.DataSpeed)
                    return context == UnitContext.SettingsTaskbar ? "{u}" : "{u}/s";
                return "{u}";
            }

            int decimals = (context == UnitContext.Taskbar) ? 0 : -1;
            string u = FormatDataSizeParts(value ?? 0, decimals).unit;

            if (type == MetricType.DataSpeed)
                return context == UnitContext.Taskbar ? u : u + "/s";
            return u;
        }

        // 3. 电池充电后缀：仅真正充电时显示闪电（AcOnline + IsCharging）上游 :229-238
        var pStatus = GetPowerStatus();
        string suffix = (
            key.StartsWith("BAT", StringComparison.OrdinalIgnoreCase)
            && pStatus.AcOnline
            && pStatus.IsCharging
            && context != UnitContext.SettingsPanel
            && context != UnitContext.SettingsTaskbar) ? "⚡" : "";

        // 4. 标准单位表 —— 上游 :240-253
        return type switch
        {
            MetricType.Percent => "%" + suffix,
            MetricType.Temperature => "°C",
            MetricType.Frequency => "GHz",
            MetricType.Power => "W" + suffix,
            MetricType.Voltage => "V" + suffix,
            MetricType.Current => "A" + suffix,
            MetricType.FPS => (context is UnitContext.Taskbar or UnitContext.SettingsTaskbar) ? "F" : " FPS",
            MetricType.RPM => (context is UnitContext.Taskbar or UnitContext.SettingsTaskbar) ? "R" : " RPM",
            _ => ""
        };
    }

    /// <summary>
    /// 字节数 → (数值字符串, 单位)。上游 MetricUtils.cs:288-307。
    /// 注意：换级阈值是 1000（不是 1024），避免 "1023 KB" 这类长串。
    /// </summary>
    public static (string val, string unit) FormatDataSizeParts(double bytes, int decimals = -1)
    {
        double len = bytes / 1024.0;

        int order = 0;
        while (len >= 1000 && order < _dataSizes.Length - 1)
        {
            order++;
            len /= 1024.0;
        }

        return (FormatValueAdaptive(len, decimals), _dataSizes[order]);
    }

    /// <summary>上游 MetricUtils.cs:309-313。</summary>
    public static string FormatDataSize(double bytes, string suffix = "", int decimals = -1)
    {
        var (val, unit) = FormatDataSizeParts(bytes, decimals);
        return $"{val}{unit}{suffix}";
    }

    // =========================================================
    // 4. 评估逻辑（上游 MetricUtils.cs:320-378）
    // =========================================================

    /// <summary>状态评估：0 安全 / 1 警告 / 2 严重。上游 MetricUtils.cs:320-360。</summary>
    public static int GetState(string key, double value, ThresholdsSet? thresholds = null)
    {
        var th = thresholds ?? new ThresholdsSet();

        if (double.IsNaN(value)) return StateSafe;
        if (key.StartsWith("BAT", StringComparison.OrdinalIgnoreCase) && value < 0) return StateSafe;

        var type = GetType(key);
        double checkValue = value;

        // 动态范围指标基于历史最大值百分比判断
        if (type is MetricType.Frequency or MetricType.Power or MetricType.RPM or MetricType.FPS
            or MetricType.Voltage or MetricType.Current)
        {
            checkValue = GetAdaptivePercentage(key, value) * 100.0;

            if (checkValue >= 90) return StateCrit;
            if (checkValue >= 80) return StateWarn;
            return StateSafe;
        }
        else if (type is MetricType.DataSpeed or MetricType.DataSize)
        {
            checkValue = value / 1048576.0; // 转 MB
        }

        var (warn, crit) = GetThresholds(key, th);

        // 电池低电量反向判断
        if (key.Equals("BAT.Percent", StringComparison.OrdinalIgnoreCase))
        {
            if (checkValue <= crit) return StateCrit;
            if (checkValue <= warn) return StateWarn;
            return StateSafe;
        }

        if (checkValue >= crit) return StateCrit;
        if (checkValue >= warn) return StateWarn;

        return StateSafe;
    }

    /// <summary>阈值表。上游 MetricUtils.cs:362-378（默认值见 Settings.cs:326-332）。</summary>
    public static (double warn, double crit) GetThresholds(string key, ThresholdsSet? thresholds = null)
    {
        var th = thresholds ?? new ThresholdsSet();
        var type = GetType(key);

        return type switch
        {
            MetricType.Temperature => (th.Temp.Warn, th.Temp.Crit),
            MetricType.DataSpeed => key.StartsWith("NET", StringComparison.OrdinalIgnoreCase)
                ? (key.IndexOf("UP", StringComparison.OrdinalIgnoreCase) >= 0
                    ? (th.NetUpMB.Warn, th.NetUpMB.Crit)
                    : (th.NetDownMB.Warn, th.NetDownMB.Crit))
                : (th.DiskIOMB.Warn, th.DiskIOMB.Crit),
            MetricType.DataSize => key.IndexOf("UP", StringComparison.OrdinalIgnoreCase) >= 0
                ? (th.DataUpMB.Warn, th.DataUpMB.Crit)
                : (th.DataDownMB.Warn, th.DataDownMB.Crit),
            MetricType.Percent => key.Equals("BAT.Percent", StringComparison.OrdinalIgnoreCase)
                ? (60, 20)
                : (th.Load.Warn, th.Load.Crit),
            _ => (th.Load.Warn, th.Load.Crit)
        };
    }

    /// <summary>进度条值（0.0-1.0，含 5% 最小视觉宽度）。上游 MetricUtils.cs:384-402。</summary>
    public static double GetProgressValue(string key, double value)
    {
        var type = GetType(key);
        double rawPct;

        if (type is MetricType.Frequency or MetricType.Power or MetricType.RPM or MetricType.FPS
            or MetricType.Voltage or MetricType.Current)
            rawPct = GetAdaptivePercentage(key, value);
        else
            rawPct = value / 100.0;

        // 负值（充电）→ 取绝对值
        if (rawPct < 0) rawPct = Math.Abs(rawPct);

        // 视觉修正：限制在 [0.05, 1.0]
        return Math.Max(0.05, Math.Min(1.0, rawPct));
    }

    /// <summary>
    /// 自适应百分比（相对历史最大值）。上游 MetricUtils.cs:404-426。
    /// recordedMax 对应上游 Settings.RecordedMax*（Settings.cs:122-134 默认值）。
    /// </summary>
    public static double GetAdaptivePercentage(string key, double val, RecordedMaxSet? recordedMax = null)
    {
        var rm = recordedMax ?? new RecordedMaxSet();
        float max = 1.0f;

        if (key == "CPU.Clock") max = rm.CpuClock;
        else if (key == "CPU.Power") max = rm.CpuPower;
        else if (key == "GPU.Clock") max = rm.GpuClock;
        else if (key == "GPU.Power") max = rm.GpuPower;
        else if (key == "CPU.Fan") max = rm.CpuFan;
        else if (key == "CPU.Pump") max = rm.CpuPump;
        else if (key == "CASE.Fan") max = rm.ChassisFan;
        else if (key == "GPU.Fan") max = rm.GpuFan;
        else if (key == "FPS") max = rm.Fps;
        else if (key == "CPU.Voltage") max = 1.6f;
        else if (key == "BAT.Power") max = 100f;
        else if (key == "BAT.Voltage") max = 20f;
        else if (key == "BAT.Current") max = 5f;

        if (max < 1) max = 1;
        double pct = val / max;
        return pct > 1.0 ? 1.0 : pct;
    }

    // =========================================================
    // 5. 基础转换（上游 MetricUtils.cs:431-433）
    // =========================================================
    public static int ParseInt(string? s) =>
        int.TryParse(new string((s ?? string.Empty).Where(c => char.IsDigit(c) || c == '-').ToArray()), out int v) ? v : 0;

    public static double ParseDouble(string? s) =>
        double.TryParse(new string((s ?? string.Empty).Where(c => char.IsDigit(c) || c == '.' || c == '-').ToArray()), out double v) ? v : 0;

    public static string ToStr(double v, string format = "F1") => v.ToString(format);

    // --- kernel32!GetSystemPowerStatus（替代 WinForms SystemInformation.PowerStatus）---
    [StructLayout(LayoutKind.Sequential)]
    private struct SYSTEM_POWER_STATUS
    {
        public byte ACLineStatus;
        public byte BatteryFlag;
        public byte BatteryLifePercent;
        public byte SystemStatusFlag;
        public uint BatteryLifeTime;
        public uint BatteryFullLifeTime;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetSystemPowerStatus(out SYSTEM_POWER_STATUS lpSystemPowerStatus);
}

/// <summary>历史最大值（上游 Settings.cs:122-134 默认值原样）。</summary>
public sealed class RecordedMaxSet
{
    public float CpuPower { get; init; } = 65.0f;
    public float CpuClock { get; init; } = 4200.0f;
    public float GpuPower { get; init; } = 100.0f;
    public float GpuClock { get; init; } = 1800.0f;
    public float Fps { get; init; } = 144.0f;
    public float CpuFan { get; init; } = 4000;
    public float CpuPump { get; init; } = 5000;
    public float GpuFan { get; init; } = 3500;
    public float ChassisFan { get; init; } = 3000;
}
