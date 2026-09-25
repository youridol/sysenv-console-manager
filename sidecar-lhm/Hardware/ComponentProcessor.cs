// Hardware/ComponentProcessor.cs
//
// 移植自 LiteMonitor（MIT）src/System/HardwareServices/ComponentProcessor.cs
// 上游依赖 Settings（RecordedMax* 记录）——SECM 无该设置层，故 _cfg.UpdateMaxRecord(...)
// 调用被省略（纯 UI 历史最大值记录，不影响取值语义）。除此之外逐条保持：
//   - CPU 负载：多核平均（过滤条件原样，含 !SOC/!Max/!Average）
//   - CPU 温度：核心最大值（排除 Distance/Average/Max，只接受 >0）
//   - Zen 5 频率修正：bus ∈ (1,20) 且 factor = 100/bus ∈ (2,10)
//   - 熔断：CPU.Power > 600W、GPU.Clock > 6000MHz、GPU.Power > 1200W 一律返回 null
// 上游行号在每条规则后标注。

using LibreHardwareMonitor.Hardware;

namespace LhmSidecar.Hardware;

/// <summary>组件处理器：CPU/GPU 的聚合、修正与过滤。</summary>
public sealed class ComponentProcessor
{
    private readonly IComputer _computer;
    private readonly SensorMap _sensorMap;

    private List<ISensor>? _cpuLoadSensorsCache;
    private List<ISensor>? _cpuTempSensorsCache;

    public ComponentProcessor(IComputer computer, SensorMap sensorMap)
    {
        _computer = computer;
        _sensorMap = sensorMap;
    }

    /// <summary>硬件重载时清空传感器缓存。上游 ComponentProcessor.cs:28-32。</summary>
    public void ClearCache()
    {
        _cpuLoadSensorsCache = null;
        _cpuTempSensorsCache = null;
    }

    /// <summary>
    /// CPU 负载（多核平均）。上游 ComponentProcessor.cs:37-69。
    /// 注意上游过滤条件是 Has("Core") && Has("#") 且排除 Total/SOC/Max/Average。
    /// </summary>
    public float? GetCpuLoad()
    {
        if (_cpuLoadSensorsCache == null)
        {
            var cpu = _computer.Hardware.FirstOrDefault(h => h.HardwareType == HardwareType.Cpu);
            if (cpu != null)
            {
                _cpuLoadSensorsCache = new List<ISensor>();
                foreach (var s in cpu.Sensors)
                {
                    if (s.SensorType != SensorType.Load) continue;
                    if (SensorMap.Has(s.Name, "Core") && SensorMap.Has(s.Name, "#") &&
                        !SensorMap.Has(s.Name, "Total") && !SensorMap.Has(s.Name, "SOC") &&
                        !SensorMap.Has(s.Name, "Max") && !SensorMap.Has(s.Name, "Average"))
                    {
                        _cpuLoadSensorsCache.Add(s);
                    }
                }
            }
        }

        if (_cpuLoadSensorsCache != null && _cpuLoadSensorsCache.Count > 0)
        {
            double totalLoad = 0;
            int coreCount = 0;
            foreach (var s in _cpuLoadSensorsCache)
            {
                if (s.Value.HasValue) { totalLoad += s.Value.Value; coreCount++; }
            }
            if (coreCount > 0) return (float)(totalLoad / coreCount);
        }
        return null;
    }

    /// <summary>
    /// CPU 温度（核心最大值）。上游 ComponentProcessor.cs:74-107。
    /// 排除 Distance / Average / Max；只接受 Value > 0。
    /// </summary>
    public float? GetCpuTemp()
    {
        if (_cpuTempSensorsCache == null)
        {
            var cpuT = _computer.Hardware.FirstOrDefault(h => h.HardwareType == HardwareType.Cpu);
            if (cpuT != null)
            {
                _cpuTempSensorsCache = new List<ISensor>();
                foreach (var s in cpuT.Sensors)
                {
                    if (s.SensorType == SensorType.Temperature &&
                        !SensorMap.Has(s.Name, "Distance") && !SensorMap.Has(s.Name, "Average") && !SensorMap.Has(s.Name, "Max"))
                    {
                        _cpuTempSensorsCache.Add(s);
                    }
                }
            }
        }

        if (_cpuTempSensorsCache != null && _cpuTempSensorsCache.Count > 0)
        {
            float maxTemp = -1000f;
            bool found = false;
            foreach (var s in _cpuTempSensorsCache)
            {
                if (s.Value.HasValue && s.Value.Value > 0)
                {
                    if (s.Value.Value > maxTemp) { maxTemp = s.Value.Value; found = true; }
                }
            }
            if (found) return maxTemp;
        }
        return null;
    }

    /// <summary>
    /// 复合数值（频率/功耗等带修正或过滤逻辑的数值）。
    /// key == "CPU.Power" 时 sensorCache 需含映射结果；key 以 "GPU" 开头时使用 _sensorMap.CachedGpu。
    /// 上游 ComponentProcessor.cs:112-183。
    /// </summary>
    public float? GetCompositeValue(string key, IReadOnlyDictionary<string, ISensor> sensorCache)
    {
        if (key == "CPU.Clock")
        {
            if (_sensorMap.CpuCoreCache.Count == 0) return null;
            double sum = 0; int count = 0; float maxRaw = 0;

            // Zen 5 频率修正逻辑 —— 上游 ComponentProcessor.cs:119-129
            float correctionFactor = 1.0f;
            if (_sensorMap.CpuBusSpeedSensor != null && _sensorMap.CpuBusSpeedSensor.Value.HasValue)
            {
                float bus = _sensorMap.CpuBusSpeedSensor.Value.Value;
                if (bus > 1.0f && bus < 20.0f)
                {
                    float factor = 100.0f / bus;
                    if (factor > 2.0f && factor < 10.0f) correctionFactor = factor;
                }
            }

            foreach (var core in _sensorMap.CpuCoreCache)
            {
                if (core.Clock == null || !core.Clock.Value.HasValue) continue;
                float clk = core.Clock.Value.Value * correctionFactor;
                if (clk > maxRaw) maxRaw = clk;
                if (clk > 400f) { sum += clk; count++; }   // 上游 :136
            }
            // 上游此处还有 _cfg.UpdateMaxRecord(key, maxRaw)：纯 UI 历史记录，SECM 不消费
            if (count > 0) return (float)(sum / count);
            return maxRaw;                                  // 上游 :140
        }

        if (key == "CPU.Power")
        {
            if (sensorCache.TryGetValue("CPU.Power", out var s) && s.Value.HasValue)
            {
                // 熔断保护：过滤异常高功耗 > 600W —— 上游 ComponentProcessor.cs:148
                if (s.Value.Value > 600.0f) return null;
                return s.Value.Value;
            }
            return null;
        }

        if (key.StartsWith("GPU", StringComparison.Ordinal))
        {
            var gpu = _sensorMap.CachedGpu;
            if (gpu == null) return null;

            if (key == "GPU.Clock") return GetGpuClockMhz(gpu);
            if (key == "GPU.Power") return GetGpuPowerW(gpu);
        }
        return null;
    }

    /// <summary>
    /// 单张显卡的核心频率（MHz）+ 熔断保护。上游 ComponentProcessor.cs:159-169。
    /// 抽成静态方法是为了让非代表显卡 / 单元测试复用完全相同的匹配与熔断规则。
    /// </summary>
    public static float? GetGpuClockMhz(IHardware gpu)
    {
        var s = gpu.Sensors.FirstOrDefault(x => x.SensorType == SensorType.Clock &&
            (SensorMap.Has(x.Name, "graphics") || SensorMap.Has(x.Name, "core") || SensorMap.Has(x.Name, "shader")));
        if (s != null && s.Value.HasValue)
        {
            float val = s.Value.Value;
            if (val > 6000.0f) return null;   // 熔断：> 6000MHz 上游 ComponentProcessor.cs:165
            return val;
        }
        return null;
    }

    /// <summary>
    /// 单张显卡的功耗（W）+ 熔断保护。上游 ComponentProcessor.cs:170-180。
    /// </summary>
    public static float? GetGpuPowerW(IHardware gpu)
    {
        var s = gpu.Sensors.FirstOrDefault(x => x.SensorType == SensorType.Power &&
            (SensorMap.Has(x.Name, "package") || SensorMap.Has(x.Name, "ppt") ||
             SensorMap.Has(x.Name, "board") || SensorMap.Has(x.Name, "core") || SensorMap.Has(x.Name, "gpu")));
        if (s != null && s.Value.HasValue)
        {
            float val = s.Value.Value;
            if (val > 1200.0f) return null;   // 熔断：> 1200W 上游 ComponentProcessor.cs:176
            return val;
        }
        return null;
    }

    /// <summary>
    /// CPU 功耗 + 熔断保护。上游 ComponentProcessor.cs:143-153。
    /// </summary>
    public static float? ApplyCpuPowerClamp(ISensor? s)
    {
        if (s != null && s.Value.HasValue)
        {
            if (s.Value.Value > 600.0f) return null;   // 熔断：> 600W 上游 ComponentProcessor.cs:148
            return s.Value.Value;
        }
        return null;
    }
}
