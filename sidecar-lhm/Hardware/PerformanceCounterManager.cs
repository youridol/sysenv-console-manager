// Hardware/PerformanceCounterManager.cs
//
// 移植自 LiteMonitor（MIT）src/System/HardwareServices/PerformanceCounterManager.cs
// 这是上游**唯一**的纯用户态数据源（CONTRACT-v3.md §1 关键事实），因此它是 SECM tier-1
// 的 CPU 负载/频率来源，也是磁盘速率/活动的来源。
//
// 逐条保持：
//   - 类别/计数器/实例名严格原样（上游 PerformanceCounterManager.cs:67-98）
//     Processor Information / % Processor Utility / _Total（回退 % Processor Time、Processor）
//     Processor Information / % Processor Performance / _Total（回退 Processor）
//     Memory / Available MBytes；PhysicalDisk / Disk Read|Write Bytes/sec、% Disk Time / _Total
//     System / System Up Time；SMB Client Shares / Read|Write Bytes/sec / _Total
//     SMB Server Shares / Received|Sent Bytes/sec / _Total
//   - 首次 NextValue() 恒为 0 → 初始化必须预热一次并丢弃（上游 :100-118）—— CONTRACT §5.3
//   - % Processor Utility 睿频可能 >100 → 截断 100（上游 :187-188）—— CONTRACT §5.4
//   - CreateCounter 先做 PerformanceCounterCategory.Exists 检查（上游 :132-145）
//   - 基准频率读注册表 ~MHz，读不到兜底 2500（上游 :147-164）
//   - 物理内存总量走 GlobalMemoryStatusEx，读不到兜底 16GB（上游 :166-178）
//   - SMB 协议开销系数 1.2（上游 :238）
//   - SafeRead 吞异常返回 null（上游 :273-285）
//
// 契约 v3 新增（CONTRACT-v3.md §3 disks[]）：每卷（per-volume 实例）读盘。上游只用
// _Total 聚合实例（:84-86），本移植在保留 _Total 的同时增加按实例计数的同名计数器工厂
// GetVolumeRead/Write/Active，类别/计数器名与上游一致，仅实例名改为卷标（"C:"）。

using System.Diagnostics;
using System.Runtime.InteropServices;
using Microsoft.Win32;
using Pc = System.Diagnostics.PerformanceCounter;

namespace LhmSidecar.Hardware;

/// <summary>Windows 性能计数器统一管理器（tier-1，纯用户态）。</summary>
public sealed class PerformanceCounterManager : IDisposable
{
    // --- 核心计数器实例（上游字段对应 :16-32）---
    private Pc? _cpuLoadCounter;       // CPU 总使用率
    private Pc? _cpuFreqCounter;       // CPU 性能百分比（用于算频率）
    private Pc? _ramAvailableCounter;  // 可用内存 (MB)
    private Pc? _diskReadCounter;      // 磁盘总读取
    private Pc? _diskWriteCounter;     // 磁盘总写入
    private Pc? _diskActiveCounter;    // 磁盘活动时间 (%)
    private Pc? _uptimeCounter;        // 系统运行时间

    // --- SMB 计数器（用于忽略内网流量）上游 :29-32 ---
    private Pc? _smbClientReadCounter;
    private Pc? _smbClientWriteCounter;
    private Pc? _smbServerReadCounter;
    private Pc? _smbServerWriteCounter;

    // --- 每卷计数器缓存（契约 v3 新增，类别/计数器名同上游）---
    private readonly Dictionary<string, Pc?> _volumeRead = new(StringComparer.OrdinalIgnoreCase);
    private readonly Dictionary<string, Pc?> _volumeWrite = new(StringComparer.OrdinalIgnoreCase);
    private readonly Dictionary<string, Pc?> _volumeActive = new(StringComparer.OrdinalIgnoreCase);

    // --- 静态基准数据（启动时取一次）上游 :35-36 ---
    private float _cpuBaseFreq;    // 基准频率 (MHz)
    private float _totalMemoryMB;  // 物理内存总量 (MB)
    private bool _cpuBaseFreqDetected; // 注册表 ~MHz 是否真的读到了（区分"实测"与上游的 2500 兜底常量）
    private bool _totalMemoryDetected; // GlobalMemoryStatusEx 是否真的读到了

    /// <summary>计数器是否已完成初始化与预热。上游 :41。</summary>
    public bool IsInitialized { get; private set; }

    /// <summary>物理内存总量 (GB)。上游 :46。</summary>
    public float TotalMemoryGB => _totalMemoryMB / 1024f;

    /// <summary>
    /// 注册表 ~MHz 是否读到真实值。上游在读不到时兜底 2500MHz 以免除零（上游 :164），
    /// 但兜底常量是"数学兜底"而非实测值——契约 §3 要求"绝不伪造"，故对外报告频率前
    /// 必须检查此标志，未实测到就不输出 clock_mhz（返回 null / source="none"）。
    /// </summary>
    public bool CpuBaseFreqDetected => _cpuBaseFreqDetected;

    /// <summary>GlobalMemoryStatusEx 是否读到真实值（兜底 16GB 同理不得当作实测值输出）。</summary>
    public bool TotalMemoryDetected => _totalMemoryDetected;

    /// <summary>
    /// 同步初始化（上游是 Task.Run 异步；本 sidecar 在采集后台线程里调用，等价）。
    /// 上游 PerformanceCounterManager.cs:52-127。
    /// </summary>
    public void Initialize()
    {
        try
        {
            // 1. 静态硬件信息（总内存、基准频率）
            InitStaticHardwareInfo();

            // 2. 创建计数器实例 —— 上游 :62-98
            // CPU 负载：Processor Information 类别（Win8+ 推荐）
            // [Fix] 用 "% Processor Utility" 而非 "% Processor Time"：任务管理器在 Win8+ 显示 Utility（含睿频）
            _cpuLoadCounter = CreateCounter("Processor Information", "% Processor Utility", "_Total");
            if (_cpuLoadCounter == null)
                _cpuLoadCounter = CreateCounter("Processor Information", "% Processor Time", "_Total"); // 回退1
            if (_cpuLoadCounter == null)
                _cpuLoadCounter = CreateCounter("Processor", "% Processor Time", "_Total");              // 旧系统回退

            // CPU 频率：性能百分比（Performance Limit），配合基准频率计算
            _cpuFreqCounter = CreateCounter("Processor Information", "% Processor Performance", "_Total");
            if (_cpuFreqCounter == null)
                _cpuFreqCounter = CreateCounter("Processor", "% Processor Performance", "_Total");

            // 内存：只读"可用内存"，用 (总内存 - 可用) 算占用，比直接读占用率更准
            _ramAvailableCounter = CreateCounter("Memory", "Available MBytes");

            // 磁盘：物理磁盘总和 (_Total)
            _diskReadCounter = CreateCounter("PhysicalDisk", "Disk Read Bytes/sec", "_Total");
            _diskWriteCounter = CreateCounter("PhysicalDisk", "Disk Write Bytes/sec", "_Total");
            _diskActiveCounter = CreateCounter("PhysicalDisk", "% Disk Time", "_Total");

            // 系统：运行时间
            _uptimeCounter = CreateCounter("System", "System Up Time");

            // SMB（内网流量）：SMB Client/Server Shares 类别
            _smbClientReadCounter = CreateCounter("SMB Client Shares", "Read Bytes/sec", "_Total");
            _smbClientWriteCounter = CreateCounter("SMB Client Shares", "Write Bytes/sec", "_Total");
            _smbServerReadCounter = CreateCounter("SMB Server Shares", "Received Bytes/sec", "_Total");
            _smbServerWriteCounter = CreateCounter("SMB Server Shares", "Sent Bytes/sec", "_Total");

            // 3. ★关键步骤★ 预热（Pre-warming）—— 上游 :100-118
            // PerformanceCounter 计算两次采样的时间差，第一次 NextValue() 永远返回 0，
            // 必须在这里先读一次，否则首屏数据为 0。（CONTRACT-v3.md §5.3）
            SafeRead(_cpuLoadCounter);
            SafeRead(_cpuFreqCounter);
            SafeRead(_ramAvailableCounter);
            SafeRead(_diskReadCounter);
            SafeRead(_diskWriteCounter);
            SafeRead(_diskActiveCounter);
            SafeRead(_uptimeCounter);
            SafeRead(_smbClientReadCounter);
            SafeRead(_smbClientWriteCounter);
            SafeRead(_smbServerReadCounter);
            SafeRead(_smbServerWriteCounter);

            IsInitialized = true;
        }
        catch
        {
            // 初始化失败通常是系统组件损坏（精简版 Windows）；IsInitialized 保持 false，上层回退 LHM
            // —— 上游 :121-125
        }
    }

    /// <summary>安全创建计数器。上游 :132-145。</summary>
    private static Pc? CreateCounter(string category, string counter, string instance = "")
    {
        try
        {
            if (!PerformanceCounterCategory.Exists(category)) return null;
            return string.IsNullOrEmpty(instance)
                ? new Pc(category, counter)
                : new Pc(category, counter, instance);
        }
        catch
        {
            return null;
        }
    }

    /// <summary>静态硬件信息：CPU 基准频率（注册表 ~MHz）+ 物理内存总量（GlobalMemoryStatusEx）。上游 :147-178。</summary>
    private void InitStaticHardwareInfo()
    {
        // A. CPU 基准频率（注册表比 WMI 快）—— 上游 :149-164
        try
        {
            if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
            {
                using var key = Registry.LocalMachine.OpenSubKey(
                    @"HARDWARE\DESCRIPTION\System\CentralProcessor\0");
                if (key != null)
                {
                    var val = key.GetValue("~MHz");
                    if (val is int freq && freq > 0)
                    {
                        _cpuBaseFreq = freq;
                        _cpuBaseFreqDetected = true;
                    }
                }
            }
        }
        catch { /* 上游 :162 */ }
        // 兜底：读不到设 2.5GHz，防除零
        if (_cpuBaseFreq <= 0) _cpuBaseFreq = 2500;

        // B. 物理内存总量（GlobalMemoryStatusEx）—— 上游 :166-178
        try
        {
            var memStatus = new MEMORYSTATUSEX();
            if (GlobalMemoryStatusEx(memStatus) && memStatus.ullTotalPhys > 0)
            {
                _totalMemoryMB = memStatus.ullTotalPhys / 1024f / 1024f;
                _totalMemoryDetected = true;
            }
        }
        catch { /* 上游 :175 */ }
        // 兜底：默认 16GB
        if (_totalMemoryMB <= 0) _totalMemoryMB = 16 * 1024;
    }

    // ==========================================
    // 安全取值方法（内部消化异常，失败返回 null）上游 :184-223
    // ==========================================

    /// <summary>CPU 负载 %；>100 截断为 100。上游 :184-189。</summary>
    public float? GetCpuLoad()
    {
        var val = SafeRead(_cpuLoadCounter);
        // [Fix] Utility 计数器在睿频时可能 >100%，截断以匹配任务管理器行为
        return val > 100f ? 100f : val;
    }

    /// <summary>CPU 频率 MHz = 基准频率 × (性能百分比 / 100)。上游 :191-201。</summary>
    public float? GetCpuFreq()
    {
        var percent = SafeRead(_cpuFreqCounter);
        if (percent.HasValue && _cpuBaseFreq > 0)
        {
            return _cpuBaseFreq * (percent.Value / 100.0f);
        }
        return null;
    }

    /// <summary>CPU 基准频率（MHz）——契约 §3 的 "registry" 频率回退用。上游 :158 同源。</summary>
    public float CpuBaseFreqMhz => _cpuBaseFreq > 0 ? _cpuBaseFreq : 2500f;

    /// <summary>内存数据 (占用率%, 已用MB)。上游 :203-218。</summary>
    public (float? Load, float? UsedMB) GetMemoryData()
    {
        var availableMB = SafeRead(_ramAvailableCounter);
        if (availableMB.HasValue && _totalMemoryMB > 0)
        {
            float usedMB = _totalMemoryMB - availableMB.Value;
            float load = (usedMB / _totalMemoryMB) * 100f;
            return (load, usedMB);
        }
        return (null, null);
    }

    public float? GetDiskRead() => SafeRead(_diskReadCounter);      // 上游 :220
    public float? GetDiskWrite() => SafeRead(_diskWriteCounter);    // 上游 :221
    public float? GetDiskActive() => SafeRead(_diskActiveCounter);  // 上游 :222
    public float? GetUptime() => SafeRead(_uptimeCounter);          // 上游 :223

    // ==========================================
    // 每卷计数器（契约 v3 §3 disks[]：per-volume 实例）
    // 类别/计数器名与上游 :84-86 完全一致，实例名 = 卷标（如 "C:"）
    // ==========================================

    /// <summary>每卷读速率（实例 = 卷标）。首次访问创建并预热一次。</summary>
    public float? GetVolumeRead(string volume)
        => VolumeCounter(_volumeRead, "PhysicalDisk", "Disk Read Bytes/sec", volume) is { } c
            ? SafeRead(c) : null;

    /// <summary>每卷写速率（实例 = 卷标）。</summary>
    public float? GetVolumeWrite(string volume)
        => VolumeCounter(_volumeWrite, "PhysicalDisk", "Disk Write Bytes/sec", volume) is { } c
            ? SafeRead(c) : null;

    /// <summary>每卷活动时间 %（实例 = 卷标）。</summary>
    public float? GetVolumeActive(string volume)
        => VolumeCounter(_volumeActive, "PhysicalDisk", "% Disk Time", volume) is { } c
            ? SafeRead(c) : null;

    private Pc? VolumeCounter(Dictionary<string, Pc?> cache, string category, string counter, string volume)
    {
        if (cache.TryGetValue(volume, out var existing)) return existing;

        // PhysicalDisk 的卷实例名带物理盘序号前缀（如 "0 C:"、"1 D:"），
        // 而不是裸卷标 "C:"。上游只用 _Total 聚合实例（PerformanceCounterManager.cs:84-86），
        // 故这里需要把卷标解析成真实实例名后才能复用同一类别/计数器名。
        string? instance = ResolveVolumeInstance(category, volume);
        var created = instance == null ? null : CreateCounter(category, counter, instance);
        if (created != null)
        {
            // 新计数器同样需要预热一次（上游 :100-118 的同一机制）
            SafeRead(created);
        }
        cache[volume] = created;
        return created;
    }

    /// <summary>
    /// 把卷标（"C:"）解析为 PhysicalDisk 的真实实例名（"0 C:"）。
    /// 匹配后缀 "<空格><卷标>"，避免把 "C:" 误配到别的实例上。
    /// </summary>
    private static string? ResolveVolumeInstance(string category, string volume)
    {
        try
        {
            if (!PerformanceCounterCategory.Exists(category)) return null;
            var cat = new PerformanceCounterCategory(category);
            string suffix = " " + volume;
            foreach (var name in cat.GetInstanceNames())
            {
                if (name.EndsWith(suffix, StringComparison.OrdinalIgnoreCase)) return name;
            }
        }
        catch
        {
            // 类别不可用/权限不足 → 该卷速率留空（绝不伪造 0）
        }
        return null;
    }

    /// <summary>
    /// 本周期内估算的 SMB 流量增量（含协议开销补偿）。上游 :229-244。
    /// </summary>
    public (long UpBytes, long DownBytes) GetEstimatedSmbBytes(double seconds)
    {
        var rate = GetSmbTrafficRate();

        // 增加 10%→后续回到 1.2 的冗余系数，抵消 TCP/IP 与 SMB 协议头开销
        // [Update] 上游最终值 = 1.2（上游 :238），不要"改进"
        const double OverheadFactor = 1.2;

        long up = (long)(rate.UpRate * seconds * OverheadFactor);
        long down = (long)(rate.DownRate * seconds * OverheadFactor);

        return (up, down);
    }

    /// <summary>当前内网 SMB 流量速率 (Bytes/sec)，Client + Server。上游 :250-268。</summary>
    private (float UpRate, float DownRate) GetSmbTrafficRate()
    {
        float up = 0;
        float down = 0;

        // 1. Client (Redirector)：Upload = Transmitted，Download = Received
        if (_smbClientWriteCounter != null) up += (SafeRead(_smbClientWriteCounter) ?? 0);
        if (_smbClientReadCounter != null) down += (SafeRead(_smbClientReadCounter) ?? 0);

        // 2. Server：Upload = Transmitted（别人下载我的文件），Download = Received
        if (_smbServerWriteCounter != null) up += (SafeRead(_smbServerWriteCounter) ?? 0);
        if (_smbServerReadCounter != null) down += (SafeRead(_smbServerReadCounter) ?? 0);

        return (up, down);
    }

    /// <summary>包裹 try-catch 的读取。上游 :273-285。</summary>
    private static float? SafeRead(Pc? pc)
    {
        if (pc == null) return null;
        try
        {
            return pc.NextValue();
        }
        catch
        {
            return null;
        }
    }

    /// <summary>释放所有计数器资源。上游 :287-303（并补上每卷计数器）。</summary>
    public void Dispose()
    {
        _cpuLoadCounter?.Dispose();
        _cpuFreqCounter?.Dispose();
        _ramAvailableCounter?.Dispose();
        _diskReadCounter?.Dispose();
        _diskWriteCounter?.Dispose();
        _diskActiveCounter?.Dispose();
        _uptimeCounter?.Dispose();

        _smbClientReadCounter?.Dispose();
        _smbClientWriteCounter?.Dispose();
        _smbServerReadCounter?.Dispose();
        _smbServerWriteCounter?.Dispose();

        foreach (var c in _volumeRead.Values) c?.Dispose();
        foreach (var c in _volumeWrite.Values) c?.Dispose();
        foreach (var c in _volumeActive.Values) c?.Dispose();
        _volumeRead.Clear();
        _volumeWrite.Clear();
        _volumeActive.Clear();

        IsInitialized = false;
    }

    // --- Win32 API 内存结构体定义（上游 :305-326）---
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Auto)]
    private sealed class MEMORYSTATUSEX
    {
        public uint dwLength;
        public uint dwMemoryLoad;
        public ulong ullTotalPhys;
        public ulong ullAvailPhys;
        public ulong ullTotalPageFile;
        public ulong ullAvailPageFile;
        public ulong ullTotalVirtual;
        public ulong ullAvailVirtual;
        public ulong ullAvailExtendedVirtual;
        public MEMORYSTATUSEX()
        {
            dwLength = (uint)Marshal.SizeOf(typeof(MEMORYSTATUSEX));
        }
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Auto, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GlobalMemoryStatusEx([In, Out] MEMORYSTATUSEX lpBuffer);
}
