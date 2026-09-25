// Hardware/SensorMatcher.cs
//
// 移植自 LiteMonitor（MIT）src/System/HardwareServices/SensorMatcher.cs
// 职责：原始传感器名 → 标准 Key（"CPU.Temp" 之类）。逐条保持关键字与排除列表不变
// （CONTRACT-v3.md §5.1 明确冻结：CPU 温度排除 soc/vrm/fan/pump/liquid/coolant/distance；
// 内存拒绝 virtual；电池 Level 必须排除 Wear/Degradation）。
// 上游行号在每条规则后标注。
//
// 上游返回的是 LiteMonitor 的 key 字符串；本移植保持同样的 key 字符串，由 SnapshotBuilder
// 决定每个 key 落到契约 v3 的哪个字段。RawKey 前缀 "SENSOR." 仅用于区分"上游 key"与
// SnapshotBuilder 自有的派生键（如 "TIER1.CpuLoad"），不参与匹配。

using LibreHardwareMonitor.Hardware;

namespace LhmSidecar.Hardware;

/// <summary>上游 SensorMatcher 产出的标准 Key 常量（原样保留上游字符串）。</summary>
public static class SensorKeys
{
    public const string CpuLoad = "CPU.Load";
    public const string CpuTemp = "CPU.Temp";
    public const string CpuPower = "CPU.Power";
    public const string CpuVoltage = "CPU.Voltage";
    public const string GpuLoad = "GPU.Load";
    public const string GpuTemp = "GPU.Temp";
    public const string GpuVramUsed = "GPU.VRAM.Used";
    public const string GpuVramTotal = "GPU.VRAM.Total";
    public const string GpuVramLoad = "GPU.VRAM.Load";
    public const string MemLoad = "MEM.Load";
    public const string MemUsed = "MEM.Used";
    public const string MemAvailable = "MEM.Available";
    public const string BatPercent = "BAT.Percent";
    public const string BatPower = "BAT.Power";
    public const string BatVoltage = "BAT.Voltage";
    public const string BatCurrent = "BAT.Current";
}

/// <summary>传感器匹配器（上游原名 SensorMap.NormalizeKey）：把原始传感器名翻译成标准 Key。</summary>
public static class SensorMatcher
{
    /// <summary>复用 HardwareRules 的字符串匹配（上游 SensorMatcher.cs:14）。</summary>
    private static bool Has(string? source, string sub) => HardwareRules.Has(source, sub);

    /// <summary>
    /// 尝试把传感器匹配到标准 Key；不匹配返回 null。上游 SensorMatcher.cs:19-160。
    /// </summary>
    public static string? Match(IHardware hw, ISensor s)
    {
        string name = s.Name;
        var type = hw.HardwareType;

        // --- CPU --- 上游 SensorMatcher.cs:25-69
        if (type == HardwareType.Cpu)
        {
            // 负载：某些 CPU 把总负载叫 "CPU Package" —— 上游 SensorMatcher.cs:28-32
            if (s.SensorType == SensorType.Load)
            {
                if (Has(name, "total") || Has(name, "package"))
                    return SensorKeys.CpuLoad;
            }

            // 温度 —— 上游 SensorMatcher.cs:34-60
            if (s.SensorType == SensorType.Temperature)
            {
                // 1. 黄金标准 —— 上游 SensorMatcher.cs:37-45
                if (Has(name, "package") ||  // Intel/AMD 标准
                    Has(name, "average") ||  // LHM 聚合数据
                    Has(name, "tctl") ||     // AMD 风扇控制温度（最准）
                    Has(name, "tdie") ||     // AMD 核心硅片温度
                    Has(name, "ccd") ||      // AMD 核心板
                    Has(name, "cores"))      // 通用核心温度
                {
                    return SensorKeys.CpuTemp;
                }

                // 2. 银牌标准：通用名称兜底 —— 上游 SensorMatcher.cs:49-59
                // 必须严格排除干扰项（soc / vrm / fan / pump / liquid / coolant / distance）
                if ((Has(name, "cpu") || Has(name, "core")) &&
                    !Has(name, "soc") &&     // 排除核显/片上系统
                    !Has(name, "vrm") &&     // 排除供电
                    !Has(name, "fan") &&     // 排除风扇（虽类型不同，但防名字干扰）
                    !Has(name, "pump") &&    // 排除水泵
                    !Has(name, "liquid") &&  // 排除水冷液
                    !Has(name, "coolant") && // 排除冷却液
                    !Has(name, "distance"))  // 排除 "Distance to TjMax"
                {
                    return SensorKeys.CpuTemp;
                }
            }

            // 功耗 —— 上游 SensorMatcher.cs:61
            if (s.SensorType == SensorType.Power && (Has(name, "package") || Has(name, "cores")))
                return SensorKeys.CpuPower;

            // 电压 —— 上游 SensorMatcher.cs:63-68
            if (s.SensorType == SensorType.Voltage &&
                (Has(name, "core") || Has(name, "cpu") || Has(name, "vcore") || Has(name, "vid")))
            {
                // Exclude distractions（上游原样）
                if (!Has(name, "soc") && !Has(name, "gt") && !Has(name, "sa") && !Has(name, "aux"))
                    return SensorKeys.CpuVoltage;
            }
        }

        // --- GPU --- 上游 SensorMatcher.cs:72-110
        if (type is HardwareType.GpuNvidia or HardwareType.GpuAmd or HardwareType.GpuIntel)
        {
            if (s.SensorType == SensorType.Load && (Has(name, "core") || Has(name, "d3d 3d")))
                return SensorKeys.GpuLoad;

            if (s.SensorType == SensorType.Temperature &&
                (Has(name, "core") || Has(name, "hot spot") || Has(name, "soc") || Has(name, "vr")))
                return SensorKeys.GpuTemp;

            // VRAM：按硬件规则决定是否优先找共享内存（核显）—— 上游 SensorMatcher.cs:79
            bool preferShared = HardwareRules.ShouldUseSharedMemory(hw);

            if (s.SensorType == SensorType.SmallData &&
                (Has(name, "memory") || Has(name, "dedicated") || Has(name, "shared")))
            {
                bool isShared = Has(name, "shared");
                bool isDedicated = Has(name, "dedicated");
                // 厂商传感器常只写 "Memory Used"
                bool isGenericMem = !isShared && !isDedicated && Has(name, "memory");

                if (preferShared)
                {
                    // 核显：只接受 Shared Memory —— 上游 SensorMatcher.cs:91-92
                    if (isShared && Has(name, "used")) return SensorKeys.GpuVramUsed;
                    if (isShared && Has(name, "total")) return SensorKeys.GpuVramTotal;

                    // 回退：老驱动没有 Shared，但有 Generic Memory —— 上游 SensorMatcher.cs:96
                    if (isGenericMem && Has(name, "used")) return SensorKeys.GpuVramUsed;
                }
                else
                {
                    // 独显：接受 Dedicated 或 Generic（屏蔽 Shared）—— 上游 SensorMatcher.cs:101-105
                    if (!isShared)
                    {
                        if (Has(name, "used")) return SensorKeys.GpuVramUsed;
                        if (Has(name, "total")) return SensorKeys.GpuVramTotal;
                    }
                }
            }

            // 显存负载 —— 上游 SensorMatcher.cs:109
            if (s.SensorType == SensorType.Load && Has(name, "memory")) return SensorKeys.GpuVramLoad;
        }

        // --- Memory --- 上游 SensorMatcher.cs:113-134
        if (type == HardwareType.Memory)
        {
            // [Critical Fix] 严格排除虚拟内存（判定用的是 **硬件名**，不是传感器名）
            // —— 上游 SensorMatcher.cs:116
            if (Has(hw.Name, "virtual")) return null;

            // 1. 负载 —— 上游 SensorMatcher.cs:120-125
            if (s.SensorType == SensorType.Load)
            {
                if (Has(name, "memory")) return SensorKeys.MemLoad;
                // 兼容只有 "Load" 字样的传感器（Equals，非 Contains）
                if (name.Equals("Load", StringComparison.OrdinalIgnoreCase)) return SensorKeys.MemLoad;
            }

            // 2. 同时接受 Data 与 SmallData —— 上游 SensorMatcher.cs:128-133
            if (s.SensorType == SensorType.Data || s.SensorType == SensorType.SmallData)
            {
                if (Has(name, "used")) return SensorKeys.MemUsed;
                // 恢复对 Free 的支持，防止部分系统 Available 传感器缺失或命名为 Free
                if (Has(name, "available") || Has(name, "free")) return SensorKeys.MemAvailable;
            }
        }

        // --- Battery --- 上游 SensorMatcher.cs:137-157
        if (type == HardwareType.Battery)
        {
            if (s.SensorType == SensorType.Level)
            {
                // 过滤电池损耗/健康度（同样标记为 Level 类型）—— 上游 SensorMatcher.cs:142
                if (Has(name, "Degradation") || Has(name, "Wear")) return null;

                // 优先包含 "Charge" 的传感器 —— 上游 SensorMatcher.cs:145
                if (Has(name, "Charge")) return SensorKeys.BatPercent;

                // 其他作为备选
                return SensorKeys.BatPercent;
            }
            if (s.SensorType == SensorType.Power) return SensorKeys.BatPower;       // 上游 :150-154
            if (s.SensorType == SensorType.Voltage) return SensorKeys.BatVoltage;   // 上游 :155
            if (s.SensorType == SensorType.Current) return SensorKeys.BatCurrent;   // 上游 :156
        }

        return null; // 上游 SensorMatcher.cs:159
    }
}
