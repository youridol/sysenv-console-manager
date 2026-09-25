// Hardware/DiskManager.cs
//
// 移植自 LiteMonitor（MIT）src/System/HardwareServices/DiskManager.cs
//
// 保持：
//   - 自适应空闲退避：目标盘总更新；后台盘 idle>5min 深睡、idle>1min 仅慢扫、
//     其余仅慢扫；采集到 >1KB/s 吞吐时重置活跃计时 —— 上游 DiskManager.cs:41-79
//   - 首选磁盘锁定（PreferredDisk）—— 上游 :34-38
//   - DriveInfo 已用率 = 100 - TotalFreeSpace/TotalSize*100（带 DriveInfo 缓存）
//     —— 上游 :104-134
//   - FindBestTempSensor：排除 warning/critical，优先精确名 "Temperature"，否则取第一个
//     —— 上游 :204-221
//   - SafeRead 的"最近有效值兜底"（避免瞬时读失败导致 UI 闪 0）—— 上游 :242-251
//   - 系统盘优先评分（+1e9）—— 上游 :147-176
//
// 移植差异（契约 v3 §3 disks[] 要求 per-volume 实例与卷名）：
//   上游 key 形如 "DISK.C.Used"（:104-134），SECM 契约直接给出卷数组。因此本移植把
//   DriveInfo 枚举提升为公开的 EnumerateVolumes()，PDH 每卷计数器位于
//   PerformanceCounterManager.GetVolumeRead/Write/Active（类别/计数器名与上游 :84-86 一致）。
// 上游行号在每条规则后标注。

using System.IO;
using LibreHardwareMonitor.Hardware;

namespace LhmSidecar.Hardware;

/// <summary>卷的静态信息（DriveInfo）。</summary>
public readonly record struct VolumeInfo(string Name, string Root, long TotalBytes, long FreeBytes)
{
    /// <summary>已用百分比（0-100）。上游 DiskManager.cs:130 同一算式。</summary>
    public double? UsedPercent => TotalBytes > 0 ? 100.0 - ((double)FreeBytes / TotalBytes * 100.0) : null;
}

/// <summary>单个磁盘硬件的自适应退避状态。</summary>
public sealed class DiskManager
{
    private readonly Dictionary<IHardware, DateTime> _diskLastActiveTime = new();
    private IHardware? _cachedDiskHw;
    private DateTime _lastDiskScan = DateTime.MinValue;

    /// <summary>最近有效的温度值（上游 lastValidMap 语义，DiskManager.cs:242-251）。</summary>
    private readonly Dictionary<string, float> _lastValidMap = new();

    /// <summary>首选磁盘（对应上游 Settings.PreferredDisk）；空则自动选优。</summary>
    public string? PreferredDisk { get; set; }

    /// <summary>上次自动选定的磁盘名（对应上游 Settings.LastAutoDisk）。</summary>
    public string? LastAutoDisk { get; private set; }

    /// <summary>清空缓存。上游 DiskManager.cs:20-26。</summary>
    public void ClearCache()
    {
        _diskLastActiveTime.Clear();
        _cachedDiskHw = null;
        _lastValidMap.Clear();
    }

    /// <summary>
    /// 每次采集对单个 Storage 硬件调用。上游 DiskManager.cs:31-80。
    /// </summary>
    public void ProcessUpdate(IHardware hw, bool isSlowScanTick, bool needDiskBgScan)
    {
        // 1. 严格遵守首选磁盘锁定 —— 上游 :34-38
        if (!string.IsNullOrEmpty(PreferredDisk) &&
            !hw.Name.Equals(PreferredDisk, StringComparison.OrdinalIgnoreCase))
        {
            return;
        }

        if (!_diskLastActiveTime.ContainsKey(hw)) _diskLastActiveTime[hw] = DateTime.Now;  // 上游 :41

        bool isTarget = (_cachedDiskHw != null && ReferenceEquals(hw, _cachedDiskHw)) ||
                        (hw.Name == LastAutoDisk) ||
                        (hw.Name == PreferredDisk);

        bool shouldUpdate = false;
        double idleMinutes = (DateTime.Now - _diskLastActiveTime[hw]).TotalMinutes;

        // === 智能退避核心逻辑 === 上游 :51-62
        if (isTarget)
        {
            shouldUpdate = true;
        }
        else
        {
            if (idleMinutes > 5) shouldUpdate = false;                                  // [深睡]
            else if (idleMinutes > 1) { if (needDiskBgScan) shouldUpdate = true; }      // [冷却]
            else { if (isSlowScanTick) shouldUpdate = true; }                           // [活跃]
        }

        if (shouldUpdate)
        {
            hw.Update();
            // 有流量则重置活跃计时器（> 1KB/s）—— 上游 :68-78
            bool hasTraffic = false;
            foreach (var s in hw.Sensors)
            {
                if (s.SensorType == SensorType.Throughput && s.Value.HasValue && s.Value.Value > 1024)
                {
                    hasTraffic = true;
                    break;
                }
            }
            if (hasTraffic) _diskLastActiveTime[hw] = DateTime.Now;
        }
    }

    /// <summary>
    /// 选定代表磁盘（自动评分：吞吐量之和 + 系统盘 +1e9）。上游 DiskManager.cs:147-189。
    /// </summary>
    public IHardware? SelectBestDiskHardware(IComputer computer, bool forceRescan = false)
    {
        if (!forceRescan && _cachedDiskHw != null && computer.Hardware.Contains(_cachedDiskHw))
        {
            return _cachedDiskHw;
        }

        // 上游 :136-145 —— 启动时按 LastAutoDisk 直接命中
        if (!forceRescan && _cachedDiskHw == null && !string.IsNullOrEmpty(LastAutoDisk))
        {
            var savedHw = computer.Hardware.FirstOrDefault(h => h.Name == LastAutoDisk);
            if (savedHw != null)
            {
                _cachedDiskHw = savedHw;
                _lastDiskScan = DateTime.Now;
                return savedHw;
            }
        }

        // 系统盘前缀 —— 上游 :148-149
        string sysPrefix = string.Empty;
        try { sysPrefix = Path.GetPathRoot(Environment.SystemDirectory)?.Substring(0, 2) ?? string.Empty; } catch { }

        IHardware? bestHw = null;
        double bestScore = double.MinValue;

        foreach (var hw in computer.Hardware.Where(h => h.HardwareType == HardwareType.Storage))
        {
            // 上游 :157 —— 系统盘优先
            bool isSystem = !string.IsNullOrEmpty(sysPrefix) &&
                            (SensorMap.Has(hw.Name, sysPrefix) || hw.Sensors.Any(s => SensorMap.Has(s.Name, sysPrefix)));

            ISensor? read = null, write = null;
            foreach (var s in hw.Sensors)
            {
                if (s.SensorType != SensorType.Throughput) continue;
                if (SensorMap.Has(s.Name, "read")) read ??= s;
                if (SensorMap.Has(s.Name, "write")) write ??= s;
            }
            if (read == null && write == null) continue;    // 上游 :165

            double score = (read?.Value ?? 0) + (write?.Value ?? 0);
            if (isSystem) score += 1e9;

            if (score > bestScore)
            {
                bestScore = score;
                bestHw = hw;
            }
        }

        if (bestHw != null)
        {
            _cachedDiskHw = bestHw;
            _lastDiskScan = DateTime.Now;
            LastAutoDisk = bestHw.Name;   // 上游 :185-188
        }
        return bestHw;
    }

    /// <summary>
    /// 代表磁盘的温度（tier-2：LHM 存储节点的 Temperature 传感器）。
    /// FindBestTempSensor + SafeRead 兜底 —— 上游 DiskManager.cs:204-251。
    /// </summary>
    public float? GetDiskTemperature(IHardware? hw)
    {
        if (hw == null) return null;
        var best = FindBestTempSensor(hw);
        if (best == null) return null;

        if (best.Value is float v && !float.IsNaN(v))
        {
            _lastValidMap["DISK.Temp"] = v;
            return v;
        }
        // 最近有效值兜底（上游 :249）
        return _lastValidMap.TryGetValue("DISK.Temp", out var last) ? last : null;
    }

    /// <summary>
    /// 最佳温度传感器。上游 DiskManager.cs:204-221。
    /// 排除 warning / critical；精确名 "Temperature" 直接返回，否则取第一个温度传感器。
    /// </summary>
    public static ISensor? FindBestTempSensor(IHardware hw)
    {
        ISensor? best = null;
        foreach (var s in hw.Sensors)
        {
            if (s.SensorType == SensorType.Temperature)
            {
                // 排除干扰项
                if (s.Name.IndexOf("warning", StringComparison.OrdinalIgnoreCase) >= 0 ||
                    s.Name.IndexOf("critical", StringComparison.OrdinalIgnoreCase) >= 0)
                    continue;

                if (s.Name == "Temperature") return s;
                if (best == null) best = s;
            }
        }
        return best;
    }

    /// <summary>
    /// 枚举就绪的固定卷（契约 §3 disks[].name 为卷名如 "C:"）。
    /// DriveInfo 使用与上游 :117-123 相同的构造/就绪判断（IsReady 不产生 Volume 字符串垃圾）。
    /// </summary>
    public static List<VolumeInfo> EnumerateVolumes()
    {
        var result = new List<VolumeInfo>();
        DriveInfo[] drives;
        try { drives = DriveInfo.GetDrives(); }
        catch { return result; }

        foreach (var di in drives)
        {
            try
            {
                if (di.DriveType != DriveType.Fixed) continue;   // 卷速率走 PDH 卷实例，仅固定盘有意义
                if (!di.IsReady) continue;                       // 上游 :130
                string name = di.Name.TrimEnd('\\');             // "C:\" → "C:"
                result.Add(new VolumeInfo(name, di.Name, di.TotalSize, di.TotalFreeSpace));
            }
            catch
            {
                // 单个卷失败不影响其他卷（等价于上游 :122 catch { return null; } 的隔离语义）
            }
        }

        result.Sort((a, b) => string.CompareOrdinal(a.Name, b.Name));
        return result;
    }
}
