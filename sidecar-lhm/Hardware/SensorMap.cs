// Hardware/SensorMap.cs
//
// 移植自 LiteMonitor（MIT）src/System/HardwareServices/SensorMap.cs
// 保持：
//   - 按 HardwareRules.GetHwPriority 升序注册（强卡在前）—— 上游 SensorMap.cs:216-217
//   - 冲突解决：优先级更高者不可被覆盖；同级时非 D3D（Vendor）胜过 D3D
//     —— 上游 SensorMap.cs:185-208
//   - CPU 核心缓存（Clock + 同名 Load 用 EndsWith 配对）与 Bus Speed 传感器缓存
//     —— 上游 SensorMap.cs:124-143
//   - 主板温度选择顺序 System → Motherboard → Chipset/PCH → 15-68℃ 最大值 → 0-95℃ 最大值
//     —— 上游 SensorMap.cs:224-272
//   - 每 10 分钟兜底重建 —— 上游 SensorMap.cs:45-51
// 上游依赖 Settings（PreferredGpu / IsAnyEnabled）的部分镜像为构造函数参数。
// FanMapper 调用点保留上游的"仅在确实需要风扇/水泵时才扫描"语义（上游 SensorMap.cs:275-280），
// SECM 需要风扇数据用于契约 tier-2，故 fanMappingEnabled 默认 true。

using LibreHardwareMonitor.Hardware;

namespace LhmSidecar.Hardware;

/// <summary>CPU 核心传感器对（用于加权平均）。上游 SensorMap.cs:22-26。</summary>
public sealed class CpuCoreSensors
{
    public ISensor? Clock;
    public ISensor? Load;
}

/// <summary>把物理硬件传感器映射为标准 Key（如 CPU.Temp）。</summary>
public sealed class SensorMap
{
    private readonly Dictionary<string, ISensor> _map = new(StringComparer.OrdinalIgnoreCase);
    private readonly FanMapper _fanMapper = new();
    private readonly object _lock = new();

    /// <summary>CPU 核心传感器对缓存。上游 SensorMap.cs:27。</summary>
    public List<CpuCoreSensors> CpuCoreCache { get; private set; } = new();

    /// <summary>代表显卡（优先级最高的第一张）缓存。上游 SensorMap.cs:30。</summary>
    public IHardware? CachedGpu { get; private set; }

    /// <summary>总线频率传感器（Zen 5 频率修正用）。上游 SensorMap.cs:32。</summary>
    public ISensor? CpuBusSpeedSensor { get; private set; }

    private DateTime _lastMapBuild = DateTime.MinValue;

    /// <summary>是否执行风扇/水泵扫描（对应上游 cfg.IsAnyEnabled("CPU.Fan"|"CPU.Pump"|"CASE.Fan")）。</summary>
    public bool FanMappingEnabled { get; set; } = true;

    /// <summary>复用 HardwareRules 的字符串匹配（上游 SensorMap.cs:298）。</summary>
    public static bool Has(string? source, string sub) => HardwareRules.Has(source, sub);

    /// <summary>
    /// 兜底重建：距上次构建超过 10 分钟才重建。上游 SensorMap.cs:45-51。
    /// </summary>
    public void EnsureFresh(Computer computer) => EnsureFresh((IComputer)computer);

    /// <inheritdoc cref="EnsureFresh(Computer)"/>
    public void EnsureFresh(IComputer computer)
    {
        if ((DateTime.Now - _lastMapBuild).TotalMinutes > 10)
        {
            Rebuild(computer);
        }
    }

    /// <summary>清空映射与缓存（硬件重载时）。上游 SensorMap.cs:53-63。</summary>
    public void Clear()
    {
        lock (_lock)
        {
            _map.Clear();
            CpuCoreCache.Clear();
            CachedGpu = null;
            CpuBusSpeedSensor = null;
        }
    }

    /// <summary>按键取传感器。上游 SensorMap.cs:67-73。</summary>
    public bool TryGetSensor(string key, out ISensor? sensor)
    {
        lock (_lock)
        {
            return _map.TryGetValue(key, out sensor);
        }
    }

    /// <summary>取内部映射副本。上游 SensorMap.cs:78-84。</summary>
    public Dictionary<string, ISensor> GetInternalMap()
    {
        lock (_lock)
        {
            return new Dictionary<string, ISensor>(_map, StringComparer.OrdinalIgnoreCase);
        }
    }

    /// <summary>
    /// 构建传感器映射与缓存。上游 SensorMap.cs:89-295。
    /// preferredGpu：上游 Settings.PreferredGpu；非空且能唯一定位时只映射该显卡，
    /// 否则回退自动优先级（上游 SensorMap.cs:103-119 + HardwareScanner.FindGpuByConfig:130-142）。
    /// </summary>
    public void Rebuild(IComputer computer, string? preferredGpu = null)
    {
        var newMap = new Dictionary<string, ISensor>(StringComparer.OrdinalIgnoreCase);
        var newCpuCache = new List<CpuCoreSensors>();
        IHardware? newGpu = null;
        ISensor? newBusSensor = null;

        // 主板温度候选（上游 SensorMap.cs:101）
        var candidatesMoboTemps = new List<ISensor>(capacity: 10);

        IHardware? selectedGpu = FindGpuByConfig(computer, preferredGpu);

        void RegisterTo(IHardware hw)
        {
            bool isGpuHardware = IsGpuHardware(hw);

            // 用户选择的显卡仍存在时只映射指定显卡 —— 上游 SensorMap.cs:113-119
            if (isGpuHardware && selectedGpu != null && !ReferenceEquals(hw, selectedGpu))
            {
                return;
            }

            // --- CPU 缓存 —— 上游 SensorMap.cs:122-144 ---
            if (hw.HardwareType == HardwareType.Cpu)
            {
                // 优先查找名为 "Bus Speed" 的时钟传感器
                if (newBusSensor == null)
                {
                    newBusSensor = hw.Sensors.FirstOrDefault(s =>
                        s.SensorType == SensorType.Clock && s.Name.Contains("Bus Speed", StringComparison.Ordinal));
                }

                // 核心频率（排除总线频率）
                var clocks = hw.Sensors.Where(s =>
                    s.SensorType == SensorType.Clock && Has(s.Name, "core") && !Has(s.Name, "bus"));

                foreach (var clock in clocks)
                {
                    // AMD 负载叫 "CPU Core #1"、频率叫 "Core #1"，用 EndsWith 配对
                    var load = hw.Sensors.FirstOrDefault(s =>
                        s.SensorType == SensorType.Load &&
                        s.Name.EndsWith(clock.Name, StringComparison.OrdinalIgnoreCase));

                    newCpuCache.Add(new CpuCoreSensors { Clock = clock, Load = load });
                }
            }

            // --- GPU 缓存 —— 上游 SensorMap.cs:146-161 ---
            if (isGpuHardware)
            {
                // 外层已按 GetHwPriority 排序，第一个遇到的就是最强的
                if (newGpu == null) newGpu = hw;

                // GPU 风扇映射：Fan 优先，其次 Control
                var fan = hw.Sensors.FirstOrDefault(s => s.SensorType == SensorType.Fan);
                if (fan == null) fan = hw.Sensors.FirstOrDefault(s => s.SensorType == SensorType.Control);
                if (fan != null) newMap["GPU.Fan"] = fan;
            }

            // --- 收集主板/SuperIO 温度候选 —— 上游 SensorMap.cs:163-171 ---
            if (hw.HardwareType == HardwareType.Motherboard || hw.HardwareType == HardwareType.SuperIO)
            {
                foreach (var s in hw.Sensors)
                {
                    if (s.SensorType == SensorType.Temperature) candidatesMoboTemps.Add(s);
                }
            }

            // --- 普通传感器映射 —— 上游 SensorMap.cs:173-210 ---
            foreach (var s in hw.Sensors)
            {
                string? key = SensorMatcher.Match(hw, s);
                if (!string.IsNullOrEmpty(key))
                {
                    if (!newMap.ContainsKey(key))
                    {
                        newMap[key] = s;
                    }
                    else
                    {
                        // 冲突解决策略：优先 Vendor（非 D3D），且严格保护高优先级显卡
                        var existing = newMap[key];

                        // 1. existing 来自"更强"的显卡（Priority 数值更小）→ 绝不覆盖
                        if (HardwareRules.GetHwPriority(existing.Hardware) < HardwareRules.GetHwPriority(hw))
                        {
                            continue;
                        }

                        // 2. 同优先级：D3D → 非 D3D 时替换；其他情况保持现有（First Wins / Vendor Wins）
                        bool existingIsD3D = existing.Name.Contains("D3D", StringComparison.OrdinalIgnoreCase);
                        bool newIsD3D = s.Name.Contains("D3D", StringComparison.OrdinalIgnoreCase);

                        if (existingIsD3D && !newIsD3D)
                        {
                            newMap[key] = s;
                        }
                    }
                }
            }

            foreach (var sub in hw.SubHardware) RegisterTo(sub);
        }

        // 按优先级排序并注册 —— 上游 SensorMap.cs:216-217
        var ordered = computer.Hardware.OrderBy(h => HardwareRules.GetHwPriority(h));
        foreach (var hw in ordered) RegisterTo(hw);

        // A. 主板温度 —— 上游 SensorMap.cs:224-272
        if (!newMap.ContainsKey("MOBO.Temp") && candidatesMoboTemps.Count > 0)
        {
            // 1. 标准名称
            var best = candidatesMoboTemps.FirstOrDefault(x => Has(x.Name, "System"));
            if (best == null) best = candidatesMoboTemps.FirstOrDefault(x => Has(x.Name, "Motherboard"));

            // 2. 芯片组
            if (best == null) best = candidatesMoboTemps.FirstOrDefault(x => Has(x.Name, "Chipset") || Has(x.Name, "PCH"));

            // 3. 兜底：15-68℃ 最大值优先，其次 0-95℃ 最大值
            if (best == null)
            {
                ISensor? bestSafe = null;
                float maxSafe = -999f;
                ISensor? bestFallback = null;
                float maxFallback = -999f;

                foreach (var t in candidatesMoboTemps)
                {
                    if (!t.Value.HasValue) continue;
                    float v = t.Value.Value;

                    // 宽范围 (0 - 95)，严格过滤 >95 的异常/虚假值
                    if (v > 0 && v < 95 && v > maxFallback)
                    {
                        maxFallback = v;
                        bestFallback = t;
                    }

                    // 合理范围 (15 - 68)：68℃ 通常是主板/系统温度合理上限
                    if (v >= 15 && v <= 68 && v > maxSafe)
                    {
                        maxSafe = v;
                        bestSafe = t;
                    }
                }

                best = bestSafe ?? bestFallback;
            }

            if (best != null) newMap["MOBO.Temp"] = best;
        }

        // B. 风扇/水泵 —— 上游 SensorMap.cs:275-280
        if (FanMappingEnabled)
        {
            _fanMapper.ScanAndMapFans(computer, newMap);
        }

        // 原子交换 —— 上游 SensorMap.cs:283-294
        lock (_lock)
        {
            _map.Clear();
            foreach (var kv in newMap) _map[kv.Key] = kv.Value;

            CpuCoreCache = newCpuCache;
            CachedGpu = newGpu;
            CpuBusSpeedSensor = newBusSensor;
            _lastMapBuild = DateTime.Now;
        }
    }

    /// <summary>是否显卡硬件。上游 HardwareScanner.cs:157-162。</summary>
    public static bool IsGpuHardware(IHardware hw) =>
        hw.HardwareType == HardwareType.GpuNvidia ||
        hw.HardwareType == HardwareType.GpuAmd ||
        hw.HardwareType == HardwareType.GpuIntel;

    /// <summary>
    /// 按配置定位显卡（Identifier 优先，旧配置按唯一名称）。上游 HardwareScanner.cs:130-142。
    /// </summary>
    public static IHardware? FindGpuByConfig(IComputer computer, string? configValue)
    {
        if (string.IsNullOrWhiteSpace(configValue)) return null;

        var gpus = computer.Hardware.Where(IsGpuHardware).ToList();
        var byIdentifier = gpus.FirstOrDefault(h =>
            string.Equals(GetHardwareValue(h), configValue, StringComparison.OrdinalIgnoreCase));
        if (byIdentifier != null) return byIdentifier;

        // 兼容旧配置：若同名多卡，旧值无法唯一定位 → 交给自动模式
        var byName = gpus.Where(h => string.Equals(h.Name, configValue, StringComparison.OrdinalIgnoreCase)).ToList();
        return byName.Count == 1 ? byName[0] : null;
    }

    /// <summary>硬件配置值（Identifier 为空时回退名称）。上游 HardwareScanner.cs:164-168。</summary>
    private static string GetHardwareValue(IHardware hardware)
    {
        string identifier = hardware.Identifier?.ToString() ?? string.Empty;
        return string.IsNullOrWhiteSpace(identifier) ? (hardware.Name ?? string.Empty) : identifier;
    }
}
