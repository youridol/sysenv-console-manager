// Hardware/SnapshotBuilder.cs
//
// 把移植后的 LiteMonitor 映射层 + 数据源组装成 **契约 v3 快照**（CONTRACT-v3.md §3/§4）。
//
// 这一层是 SECM 特有的"两级权限模型"落地点（LiteMonitor 没有该层）：
//   tier-1（未提权也可读）：CPU 负载/频率（PDH）、内存、每卷 IO/活动/占用（PDH + DriveInfo）、
//                          网络吞吐、电池、GPU 温度/功耗/显存（厂商用户态通道）
//   tier-2（需提权伴生进程）：CPU 温度/功耗/电压/风扇/水泵、主板 SuperIO、磁盘温度
//
// 硬性规则：
//   * **绝不伪造 0**：任何采集不到的值保持 null / []；`*_source` 相应为 "none"。
//   * tier-2 缺失时 motherboard = null、disks[].temperature_c = null、
//     cpu.temp/power/voltage/fan/pump = null。
//   * available 语义见 CONTRACT-v3.md §4。
//
// 上游规则出处：本文件只做"取哪个传感器、放到哪个契约字段"的编排；
// 所有阈值/优先级/关键字判定都在 Hardware/*.cs 中并已标注上游 file:line。

using LibreHardwareMonitor.Hardware;

namespace LhmSidecar.Hardware;

/// <summary>
/// 采集结果 → 契约快照。无状态：每次调用传入当前的硬件树与自身缓存对象。
/// </summary>
public sealed class SnapshotBuilder
{
    private readonly SensorMap _sensorMap;

    /// <summary>
    /// 代表显卡（SensorMap 冲突解决后的 CachedGpu，上游 SensorMap.cs:30/152-155）。
    /// </summary>
    /// <summary>CPU 频率的 LHM 回退通道（未提权时为 null，此时不回退）。</summary>
    public ComponentProcessor? Processor { get; set; }

    public SnapshotBuilder(SensorMap sensorMap)
    {
        _sensorMap = sensorMap;
    }

    /// <summary>收集 LHM 硬件树中的 tier-2 原始素材。</summary>
    public sealed class Tier2Readout
    {
        /// <summary>CPU 温度传感器候选（ComponentProcessor.GetCpuTemp 的逻辑，上游 :74-107）。</summary>
        public float? CpuTempC;
        /// <summary>CPU 各核心/CCD 温度（契约 cpu.core_temps_c）。</summary>
        public double[] CoreTempsC = Array.Empty<double>();
        /// <summary>CPU 功耗（已过 600W 熔断）。</summary>
        public float? CpuPowerW;
        /// <summary>CPU 电压。</summary>
        public float? CpuVoltageV;
        /// <summary>CPU 风扇 RPM（FanMapper CPU.Fan，上游 FanMapper.cs:138）。</summary>
        public float? CpuFanRpm;
        /// <summary>CPU 水泵 RPM（FanMapper CPU.Pump，上游 FanMapper.cs:140）。</summary>
        public float? CpuPumpRpm;

        /// <summary>主板传感器（已由生产侧 BuildMotherboard 组装；节点不存在时为 null）。</summary>
        public MotherboardData? Motherboard;

        /// <summary>卷名 → 磁盘温度 ℃。</summary>
        public Dictionary<string, float> DiskTemps = new(StringComparer.OrdinalIgnoreCase);

        /// <summary>代表磁盘温度（仅单卷时回退使用）。</summary>
        public float? RepresentativeDiskTempC;

        /// <summary>任一 tier-2 指标可读 → tier-2 生效。</summary>
        public bool HasAny =>
            CpuTempC.HasValue || (CoreTempsC.Length > 0) || CpuPowerW.HasValue || CpuVoltageV.HasValue ||
            CpuFanRpm.HasValue || CpuPumpRpm.HasValue || Motherboard != null ||
            DiskTemps.Count > 0 || RepresentativeDiskTempC.HasValue;
    }

    /// <summary>tier-1 素材。</summary>
    public sealed class Tier1Readout
    {
        public float? CpuLoadPercent;
        public float? CpuClockMhz;
        /// <summary>频率是否为注册表 ~MHz 标称值（源标记 registry）。</summary>
        public bool CpuClockFromRegistry;

        public float? MemLoadPercent;
        public long? MemTotalBytes;
        public long? MemUsedBytes;
        public string? MemName;

        /// <summary>每卷 IO/占用（卷名 → 指标）。</summary>
        public List<DiskEntry> Disks = new();

        public float? NetUpBytesPerSec;
        public float? NetDownBytesPerSec;

        public BatteryData? Battery;
    }

    public sealed class DiskEntry
    {
        public string Name = string.Empty;
        public double? ReadBytesPerSec;
        public double? WriteBytesPerSec;
        public double? ActivityPercent;
        public double? UsedPercent;
        public double? TemperatureC;
    }

    /// <summary>
    /// 组装快照。lhmCpuName 来自 LHM CPU 节点（缺失时空串）。
    /// memoryName / memoryLhm 为 LHM 内存节点的 best-effort 补充（tier-2 不可用时为 null）。
    /// </summary>
    public Snapshot Build(
        string lhmCpuName,
        IReadOnlyList<IHardware> gpuHardware,
        Tier1Readout tier1,
        Tier2Readout? tier2,
        bool elevated,
        bool tier2Available,
        string? tier2Reason,
        string? memoryName = null,
        (long? Total, long? Used) memoryLhm = default)
    {
        bool tier2Live = elevated && tier2Available && tier2 != null;

        var snap = new Snapshot
        {
            ContractVersion = 3,
            Elevated = elevated,
            Tier2Available = tier2Available,
            Tier2Reason = tier2Reason,
            Available = false,
            Error = null
        };

        // ---------------- CPU ----------------
        var cpu = new CpuData { Name = lhmCpuName };

        // 负载：PDH 优先（上游 HardwareValueProvider.cs:292-318 的计数器优先 + 100 截断）
        if (tier1.CpuLoadPercent.HasValue)
        {
            cpu.LoadPercent = Clamp(tier1.CpuLoadPercent.Value, 0f, 100f);
            cpu.LoadSource = SourceNames.PerfCounter;
        }

        // 频率：PDH 优先，回退 LHM（ComponentProcessor CPU.Clock），再回退注册表标称值
        if (tier1.CpuClockMhz.HasValue)
        {
            cpu.ClockMhz = tier1.CpuClockMhz.Value;
            cpu.ClockSource = tier1.CpuClockFromRegistry ? SourceNames.Registry : SourceNames.PerfCounter;
        }
        else if (tier2Live)
        {
            var lhmClock = Processor?.GetCompositeValue("CPU.Clock", _sensorMap.GetInternalMap());
            if (lhmClock.HasValue && lhmClock.Value > 0)
            {
                cpu.ClockMhz = lhmClock.Value;
                cpu.ClockSource = SourceNames.Lhm;
            }
        }

        // tier-2 字段
        if (tier2Live)
        {
            cpu.PackageTempC = tier2!.CpuTempC.HasValue ? tier2.CpuTempC.Value : null;
            cpu.CoreTempsC = tier2.CoreTempsC;
            cpu.PowerW = tier2.CpuPowerW;
            cpu.VoltageV = tier2.CpuVoltageV;
            cpu.FanRpm = tier2.CpuFanRpm;
            cpu.PumpRpm = tier2.CpuPumpRpm;

            if (cpu.PackageTempC.HasValue) cpu.TempSource = SourceNames.Lhm;
            if (cpu.PowerW.HasValue) cpu.PowerSource = SourceNames.Lhm;
        }
        // tier-2 不生效时：以上全部保持 null / []，source 保持 "none"（契约 §3.2）

        cpu.PowerEstimated = false; // 移植后无 TDP×usage 估算路径（契约 §3 恒 false）
        snap.Cpu = cpu;

        // ---------------- GPU（全部 tier-1：厂商用户态通道）----------------
        snap.Gpu = BuildGpus(gpuHardware);

        // ---------------- 主板（tier-2）----------------
        snap.Motherboard = tier2Live ? tier2!.Motherboard : null;

        // ---------------- 内存（tier-1：GlobalMemoryStatusEx 权威 + PDH 推算；LHM best-effort）----------------
        {
            long? total = tier1.MemTotalBytes;   // GlobalMemoryStatusEx（上游 PerformanceCounterManager.InitStaticHardwareInfo）
            long? used = tier1.MemUsedBytes;     // (总内存 - 可用)（上游 GetMemoryData）
            string? name = memoryName ?? tier1.MemName;

            // LHM best-effort 补充（仅在硬件树打开即 tier-2 路径可用时才有值）
            if (total is null && memoryLhm.Total.HasValue) total = memoryLhm.Total;
            if (used is null && memoryLhm.Used.HasValue) used = memoryLhm.Used;

            if (total.HasValue || used.HasValue)
            {
                snap.Memory = new MemoryData { Name = name, TotalBytes = total, UsedBytes = used };
            }
        }

        // ---------------- 磁盘 ----------------
        foreach (var d in tier1.Disks)
        {
            double? temp = null;
            if (tier2Live &&
                tier2!.DiskTemps.TryGetValue(d.Name, out var t))
            {
                // 卷 → 物理盘温度的映射由生产侧负责（LhmCollector.ReadDiskTemperatures：
                // 盘符启发式 + Win32 卷 extent 盘号映射）。映射不到就保持 null，
                // **不做"代表磁盘温度"这类未经证实的回填**（契约 §3.2 绝不伪造）。
                temp = t;
            }

            snap.Disks.Add(new DiskData
            {
                Name = d.Name,
                ReadBytesPerSec = d.ReadBytesPerSec,
                WriteBytesPerSec = d.WriteBytesPerSec,
                ActivityPercent = d.ActivityPercent,
                UsedPercent = d.UsedPercent,
                TemperatureC = temp
            });
        }

        // ---------------- 电池 ----------------
        snap.Battery = tier1.Battery;

        // ---------------- available（契约 §4）----------------
        bool memValid = snap.Memory != null &&
                        (snap.Memory.TotalBytes.HasValue || snap.Memory.UsedBytes.HasValue);
        bool gpuValid = snap.Gpu.Any(g =>
            g.TemperatureC.HasValue || g.LoadPercent.HasValue || g.PowerW.HasValue);
        bool diskValid = snap.Disks.Any(d =>
            d.ReadBytesPerSec.HasValue || d.WriteBytesPerSec.HasValue ||
            d.ActivityPercent.HasValue || d.UsedPercent.HasValue);

        bool available =
            cpu.LoadPercent.HasValue ||
            cpu.ClockMhz.HasValue ||
            gpuValid ||
            diskValid ||
            memValid ||
            cpu.PackageTempC.HasValue;

        snap.Available = available;
        snap.Error = available ? null : UnavailableReason(tier2Live);
        return snap;
    }

    /// <summary>
    /// 组装 GPU 数组。代表显卡（SensorMap 优先级/冲突解决结果）排在首位，其余按硬件顺序跟随。
    /// 每卡的匹配/熔断规则来自上游 ComponentProcessor.cs:159-180、SensorMatcher.cs:72-110。
    /// </summary>
    private List<GpuData> BuildGpus(IReadOnlyList<IHardware> gpuHardware)
    {
        var result = new List<GpuData>();
        var ordered = new List<IHardware>();
        if (_sensorMap.CachedGpu != null) ordered.Add(_sensorMap.CachedGpu);   // 上游 SensorMap.cs:152-155
        foreach (var g in gpuHardware)
        {
            if (!ReferenceEquals(g, _sensorMap.CachedGpu)) ordered.Add(g);
        }

        foreach (var gpu in ordered)
        {
            var data = new GpuData { Name = gpu.Name };

            // 温度：SensorMatcher GPU.Temp（core / hot spot / soc / vr），无效值不伪造
            var temp = gpu.Sensors.FirstOrDefault(s => SensorMatcher.Match(gpu, s) == SensorKeys.GpuTemp);
            if (temp?.Value is float tv && !float.IsNaN(tv))
            {
                data.TemperatureC = tv;
                data.TempSource = SourceNames.Lhm;
            }

            // 频率：ComponentProcessor.GetGpuClockMhz（含 >6000MHz 熔断）
            var clock = ComponentProcessor.GetGpuClockMhz(gpu);
            if (clock.HasValue)
            {
                data.CoreClockMhz = clock.Value;
                data.ClockSource = SourceNames.Lhm;
            }

            // 功耗：ComponentProcessor.GetGpuPowerW（含 >1200W 熔断）
            var power = ComponentProcessor.GetGpuPowerW(gpu);
            if (power.HasValue)
            {
                data.PowerW = power.Value;
                data.PowerSource = SourceNames.Lhm;
            }

            // 负载：SensorMatcher GPU.Load（core / "d3d 3d"）
            var load = gpu.Sensors.FirstOrDefault(s => SensorMatcher.Match(gpu, s) == SensorKeys.GpuLoad);
            if (load?.Value is float lv && !float.IsNaN(lv))
            {
                data.LoadPercent = Clamp(lv, 0f, 100f);
            }

            // 显存：SensorMatcher GPU.VRAM.Used/Total（注意 ShouldUseSharedMemory 的策略分支）
            var vramUsed = gpu.Sensors.FirstOrDefault(s => SensorMatcher.Match(gpu, s) == SensorKeys.GpuVramUsed);
            var vramTotal = gpu.Sensors.FirstOrDefault(s => SensorMatcher.Match(gpu, s) == SensorKeys.GpuVramTotal);

            // LHM 显存传感器单位为 MB → 字节（1MB = 1024*1024；沿用现有 sidecar 的换算，不擅改）
            if (vramUsed?.Value is float uv && !float.IsNaN(uv) && uv > 0)
                data.MemoryUsedBytes = (long)Math.Round(uv * 1024.0 * 1024.0);
            if (vramTotal?.Value is float tvv && !float.IsNaN(tvv) && tvv > 0)
                data.MemoryTotalBytes = (long)Math.Round(tvv * 1024.0 * 1024.0);

            // vram_load_percent = used/total*100（契约 §3 明示为计算值）
            if (data.MemoryUsedBytes.HasValue && data.MemoryTotalBytes is > 0)
            {
                data.VramLoadPercent =
                    (double)data.MemoryUsedBytes.Value / data.MemoryTotalBytes.Value * 100.0;
            }

            // 风扇：GPU.Fan（Fan 优先，其次 Control）——上游 SensorMap.cs:157-160
            var fan = gpu.Sensors.FirstOrDefault(s => s.SensorType == SensorType.Fan)
                   ?? gpu.Sensors.FirstOrDefault(s => s.SensorType == SensorType.Control);
            if (fan?.Value is float fv && !float.IsNaN(fv) && fv > 0)
            {
                data.FanRpm = fv;
            }

            result.Add(data);
        }

        return result;
    }

    /// <summary>
    /// 主板 SuperIO 传感器（仅 temperature / fan / voltage，契约 §3.1）。
    /// 主板节点不存在时返回 null（契约 §3.2）。
    /// </summary>
    public static MotherboardData? BuildMotherboard(IHardware? motherboard)
    {
        if (motherboard == null) return null;

        var data = new MotherboardData { Name = motherboard.Name };
        CollectSubHardwareSensors(motherboard, data.Sensors);
        return data;
    }

    /// <summary>递归收集主板 + SuperIO 的 temperature/fan/voltage 传感器（保留原始名）。</summary>
    private static void CollectSubHardwareSensors(IHardware hardware, List<MotherboardSensorData> sensors)
    {
        foreach (ISensor sensor in hardware.Sensors)
        {
            double? value = sensor.Value;
            // 与既有 sidecar 一致的无效值过滤：无值 / 非有限 / <=0 不输出（不伪造 0）
            if (!value.HasValue || !double.IsFinite(value.Value) || value.Value <= 0.0)
            {
                continue;
            }

            string? type = sensor.SensorType switch
            {
                SensorType.Temperature => "temperature",
                SensorType.Fan => "fan",
                SensorType.Voltage => "voltage",
                _ => null
            };
            if (type is null) continue;   // 其他类型不输出（契约 §3.1）

            sensors.Add(new MotherboardSensorData { Name = sensor.Name, Type = type, Value = value.Value });
        }
        foreach (IHardware sub in hardware.SubHardware)
        {
            CollectSubHardwareSensors(sub, sensors);
        }
    }

    /// <summary>顶层不可用原因（含修复建议，契约 §4）。tier-2 缺失时给出权限提示。</summary>
    private static string UnavailableReason(bool tier2Live)
    {
        if (!tier2Live)
        {
            return "未采集到任何有效指标：tier-1 数据源（PDH 性能计数器 / 厂商用户态通道）不可用；" +
                   "CPU 温度/功耗/主板传感器需要管理员权限（PawnIO 设备仅 SYSTEM/Administrators 可访问）——" +
                   "请调用 POST /api/v1/elevate 触发一次 UAC 提权";
        }
        return "未采集到任何有效指标：LHM 硬件树可能被安全软件拦截或 ring0 设备不可访问，" +
               "请检查 PawnIO 驱动状态后重试";
    }

    private static double Clamp(double v, double min, double max) =>
        v < min ? min : (v > max ? max : v);
}
