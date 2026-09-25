// Hardware/Tier1Collector.cs
//
// tier-1 采集编排：**纯用户态、无需提权**（CONTRACT-v3.md §1 层级表）。
//   CPU 负载/频率 ← PerformanceCounterManager（PDH，上游唯一纯用户态源）
//   内存           ← PerformanceCounterManager（GlobalMemoryStatusEx + Available MBytes）
//   每卷 IO/活动   ← PDH PhysicalDisk（per-volume 实例）+ DriveInfo 占用
//   网络吞吐       ← NetworkManager（原生 IP 统计 + LHM 兜底）
//   电池           ← LHM Battery 节点 + BatteryService 符号归一
//
// 该采集与 LHM 硬件树解耦：即使 PawnIO/ring0 完全不可用（未提权），tier-1 仍必须工作——
// 这正是 LiteMonitor 缺失、SECM 必须补上的非管理员路径。
//
// 注意：契约 v3 §3 的快照 JSON **没有** network 字段（§1 层级表列了"网络吞吐"，
// 但冻结的 §3/§6 均无对应键）。本移植完整实现 NetworkManager 并在采集循环中驱动它
// （保持上游语义可复用、可测），但不向快照新增字段——字段冻结，不得单方面增删。

namespace LhmSidecar.Hardware;

/// <summary>tier-1 采集器（PDH + DriveInfo + 电池）。</summary>
public sealed class Tier1Collector : IDisposable
{
    private readonly PerformanceCounterManager _perf = new();
    private readonly NetworkManager _network;
    private readonly object _lock = new();

    private SnapshotBuilder.Tier1Readout _readout = new();
    private bool _disposed;

    public Tier1Collector()
    {
        // SMB 扣除默认开启（上游 Settings.IgnoreSmbTraffic 默认 true，Settings.cs:119）
        _network = new NetworkManager(_perf, ignoreSmbTraffic: true);
    }

    /// <summary>PDH 计数器是否就绪（未就绪时 CPU/内存/磁盘速率回退 LHM 或 null，而非伪造 0）。</summary>
    public bool PerfCounterReady => _perf.IsInitialized;

    // 此处曾有 `PerfCounters` 属性：全局检索确认调用点为 0（含测试），ADR-0001 D13 删除。
    // 需要 PDH 读数时用 `Current`（Tier1Readout）或 `PerfCounterReady`。

    /// <summary>网络管理器（单测与采集循环可见；快照契约暂不含 network 字段）。</summary>
    public NetworkManager Network => _network;

    /// <summary>
    /// 预热 PDH 计数器。对应上游 HardwareMonitor.cs:310（_perfCounterManager.InitializeAsync()）。
    /// 首次 NextValue() 恒为 0，必须预热丢弃一次（CONTRACT-v3.md §5.3）。
    /// </summary>
    public void WarmUp()
    {
        _perf.Initialize();   // 内部已含 11 个计数器的预热读取（上游 PerformanceCounterManager.cs:100-118）
    }

    /// <summary>当前 tier-1 读数快照（引用不可变读法：整体替换）。</summary>
    public SnapshotBuilder.Tier1Readout Current
    {
        get { lock (_lock) return _readout; }
    }

    // 此处曾有 `LastNetworkRate` 属性：它只被赋值、无任何读取点（快照的对外字段是
    // `NetUpBytesPerSec` / `NetDownBytesPerSec`）。ADR-0001 D13 删除只写不读属性，
    // 速率仍按原样写入读数对象。

    /// <summary>
    /// 每个采集周期调用一次：读 PDH + DriveInfo + 网络增量 + 电池。
    /// </summary>
    public void Collect(BatteryData? battery)
    {
        var r = new SnapshotBuilder.Tier1Readout();

        // ---- CPU 负载（PDH % Processor Utility，上游 HardwareValueProvider.cs:295-299）----
        var load = _perf.GetCpuLoad();
        if (load.HasValue) r.CpuLoadPercent = load.Value;

        // ---- CPU 频率（PDH % Processor Performance × 基准频率，上游 :191-201）----
        var freq = _perf.GetCpuFreq();
        if (freq.HasValue && freq.Value > 0)
        {
            r.CpuClockMhz = freq.Value;
            r.CpuClockFromRegistry = false;
        }
        else if (_perf.CpuBaseFreqDetected)
        {
            // 计数器缺失时回退注册表标称频率（契约 §3 允许 clock_source="registry"），
            // 否则输出 0 会违反"绝不伪造 0"。上游 :158 的 ~MHz 兜底常量（2500）不算实测值，
            // 故必须先确认 CpuBaseFreqDetected。
            r.CpuClockMhz = _perf.CpuBaseFreqMhz;
            r.CpuClockFromRegistry = true;
        }

        // ---- 内存（上游 :203-218；权威来源是 GlobalMemoryStatusEx，契约 §3 注释）----
        var (memLoad, usedMB) = _perf.GetMemoryData();
        if (_perf.TotalMemoryDetected)
        {
            r.MemTotalBytes = (long)Math.Round(_perf.TotalMemoryGB * 1024.0 * 1024.0 * 1024.0);
        }
        if (usedMB.HasValue && usedMB.Value >= 0)
        {
            r.MemUsedBytes = (long)Math.Round(usedMB.Value * 1024.0 * 1024.0);
        }
        r.MemLoadPercent = memLoad;

        // ---- 每卷 IO / 活动 / 占用 ----
        foreach (var vol in DiskManager.EnumerateVolumes())
        {
            var entry = new SnapshotBuilder.DiskEntry
            {
                Name = vol.Name,
                UsedPercent = vol.UsedPercent,                 // DriveInfo（上游 DiskManager.cs:130 同一算式）
                ReadBytesPerSec = _perf.GetVolumeRead(vol.Name),   // PDH per-volume 实例
                WriteBytesPerSec = _perf.GetVolumeWrite(vol.Name),
                ActivityPercent = Clamp(_perf.GetVolumeActive(vol.Name), 0, 100)
            };
            r.Disks.Add(entry);
        }

        // ---- 网络吞吐（NetworkManager；快照无该字段，保留供日志/future 契约）----
        var (upBytes, downBytes, seconds) = _network.ConsumeDelta();
        if (seconds > 0)
        {
            var rate = ((float)(upBytes / seconds), (float)(downBytes / seconds));
            r.NetUpBytesPerSec = rate.Item1;
            r.NetDownBytesPerSec = rate.Item2;
        }

        // ---- 电池 ----
        r.Battery = battery;

        lock (_lock)
        {
            _readout = r;
        }
    }

    private static double? Clamp(float? v, double min, double max)
    {
        if (!v.HasValue) return null;
        double d = v.Value;
        return d < min ? min : (d > max ? max : d);
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        _perf.Dispose();      // 上游 HardwareMonitor.cs:293 _perfCounterManager.Dispose()
        _network.ClearCache();
    }
}
