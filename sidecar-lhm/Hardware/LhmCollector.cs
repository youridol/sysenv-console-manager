// Hardware/LhmCollector.cs
//
// LHM 硬件树生命周期 + tier-2 采集编排。
// 生命周期语义移植自 LiteMonitor src/System/HardwareMonitor.cs：
//   - IsCpuEnabled **恒为 true**（上游 HardwareMonitor.cs:64-69 注释：热切换只改
//     IsControllerEnabled，关掉 CPU 节点会导致温度永久读不到）—— CONTRACT-v3.md §5.2
//   - IsControllerEnabled 按需开启（上游 :355-364 ShouldEnableController）：SECM 契约
//     需要 CPU 风扇/水泵（tier-2），故恒为 true
//   - 主板/SuperIO 每 3 秒一次（慢扫），不要高频（上游 :229-239）
//   - GPU Update 失败不致命，标记需重载（上游 :199-210）
//   - 代表磁盘自适应退避（DiskManager，上游 DiskManager.cs:51-62）
//   - 传感器映射每 10 分钟兜底重建（SensorMap.EnsureFresh，上游 SensorMap.cs:45-51）
//   - OnValuesUpdated 前执行一次完整遍历而不是逐硬件 Update 的地方，与上游 :195-240 对齐
//
// 这一层不在 Contract DTO 里出现；它只产出 SnapshotBuilder 需要的原始读数。

using LibreHardwareMonitor.Hardware;

namespace LhmSidecar.Hardware;

/// <summary>原始硬件读数（tier-2 素材）+ 硬件树缓存。</summary>
public sealed class LhmCollector : IDisposable
{
    // 此处曾有 `UpdateVisitor : IVisitor`：唯一使用者是 `UpdateAll()`，两者一并删除
    // （ADR-0001 D13，全局检索无引用）。

    // 此处曾有 `UpdateAll()`（整树 `computer.Accept(_visitor)`）：全局检索确认调用点为 0
    // （含测试）。实际整树刷新走 `Tick` 内对每个硬件的 `Update()`（ADR-0001 D13）。

    private readonly SensorMap _sensorMap = new();
    private readonly DiskManager _diskManager = new();

    private Computer? _computer;
    private long _tickCounter;
    private DateTime _lastUpdate = DateTime.Now;
    private readonly object _lock = new();

    /// <summary>GPU Update 抛异常 → 下一轮需要整体重载（上游 HardwareMonitor.cs:199-210）。</summary>
    public bool ReloadRequested { get; private set; }

    /// <summary>当前 Computer 是否已打开。</summary>
    public bool IsOpen => _computer != null;

    /// <summary>是否有硬件树打开了 tier-2 组（主板/存储/控制器）。</summary>
    public bool Tier2GroupsEnabled { get; private set; }

    /// <summary>LHM CPU 节点名（缺失时空串，契约 §3 cpu.name）。</summary>
    public string CpuName { get; private set; } = string.Empty;

    public SensorMap SensorMap => _sensorMap;
    public DiskManager DiskManager => _diskManager;

    /// <summary>CPU 复合数值处理器（CPU.Clock 的 LHM 回退与 Zen 5 修正）；未 Open 时为 null。</summary>
    public ComponentProcessor? Processor => _processor;

    // ComponentProcessor 需要 IComputer，而 IComputer 在 Open() 之后才存在；
    // 用后绑定包装，避免把 Computer 传进构造函数。
    private ComponentProcessor? _processor;

    /// <summary>
    /// 创建 LHM Computer。
    ///
    /// tier-1 组（CPU/GPU/内存/电池）**无论是否提权都开启**：GPU 温度/功耗/显存走厂商
    /// 用户态通道（NVAPI/ADL，见 CONTRACT-v3.md §1 层级表），无需 ring0；CPU 节点必须恒开
    /// （契约 §5.2）。ring0 不可达时 LHM 只是返回空值，不会抛异常（既有 sidecar 的注释即
    /// 说明了这一点，故保留 ProbeRing0Backend 显式预检来决定 tier-2 是否可用）。
    ///
    /// tier-2 组（主板/SuperIO、存储温度、控制器风扇）仅在提权时开启，避免无权限时做
    /// 无意义的 SuperIO 探测。
    /// </summary>
    public static Computer CreateComputer(bool includeTier2)
    {
        return new Computer
        {
            // ★★★ 必须始终为 true ★★★ 上游 HardwareMonitor.cs:64-69（CONTRACT-v3.md §5.2）
            IsCpuEnabled = true,

            IsGpuEnabled = true,          // GPU 温度/功耗/显存：厂商用户态通道（tier-1）
            IsMemoryEnabled = true,       // 内存节点（tier-1 补充）
            IsNetworkEnabled = true,      // NetworkManager 的 LHM 吞吐兜底（上游 :73）
            IsBatteryEnabled = true,      // 电池（tier-1）

            // --- tier-2 组（需 ring0 / 管理员）---
            IsStorageEnabled = includeTier2,      // 磁盘温度（tier-2）
            IsMotherboardEnabled = includeTier2,  // 主板 SuperIO 温度/风扇/电压（tier-2）

            // SECM 契约需要 CPU 风扇/水泵（tier-2），等价于上游
            // IsAnyEnabled("CPU.Fan"|"CPU.Pump"|"CASE.Fan")（上游 :355-364）
            IsControllerEnabled = includeTier2,

            IsPsuEnabled = false          // 上游 :85
        };
    }

    /// <summary>
    /// 打开硬件树并建立首次传感器映射。上游 HardwareMonitor.cs:312-325（Open + WarmUp + Rebuild）。
    /// includeTier2=false 时只开启 tier-1 组，未提权也能拿到 GPU/内存/电池。
    /// 调用方负责异常处理与退避重试（见 Program.CollectionLoop）。
    /// </summary>
    public void Open(bool includeTier2)
    {
        var computer = CreateComputer(includeTier2);
        computer.Open();

        lock (_lock)
        {
            _computer = computer;
            Tier2GroupsEnabled = includeTier2;
            _processor = new ComponentProcessor(computer, _sensorMap);
            CpuName = computer.Hardware
                .FirstOrDefault(h => h.HardwareType == HardwareType.Cpu)?.Name ?? string.Empty;
        }

        // 启动预热：主板/电池（上游 WarmUpMotherboardSensors :366-378 / WarmUpBatterySensors :380-392），
        // 保证首次映射就能拿到有效值。
        WarmUpMotherboardAndBattery();

        // 首次建立映射（上游 :321 _sensorMap.Rebuild）
        _sensorMap.Rebuild(computer);
        _tickCounter = 0;
        _lastUpdate = DateTime.Now;
    }

    /// <summary>上游 HardwareMonitor.cs:366-392 的两次预热（主板含 SuperIO 递归 + 电池）。</summary>
    private void WarmUpMotherboardAndBattery()
    {
        var computer = _computer;
        if (computer == null) return;

        foreach (var hw in computer.Hardware)
        {
            try
            {
                if (hw.HardwareType is HardwareType.Motherboard or HardwareType.SuperIO)
                {
                    UpdateWithSubHardware(hw);
                }
                else if (hw.HardwareType == HardwareType.Battery)
                {
                    hw.Update();
                }
            }
            catch
            {
                // 单个硬件预热失败不影响其他硬件（上游每处都有独立 try/catch）
            }
        }
    }

    /// <summary>
    /// 一个采集周期。返回 false 表示当前没有可用的硬件树。
    /// </summary>
    public bool Tick(Action<double>? observeNetwork)
    {
        var computer = _computer;
        if (computer == null) return false;

        // 计时：与上游 HardwareMonitor.cs:394-417 一致，用真实时间凑满 1 秒才增加秒计数，
        // 保证 %3 / %10 的慢扫节奏基于真实时间而非调用次数。
        DateTime now = DateTime.Now;
        double timeDelta = (now - _lastUpdate).TotalSeconds;
        _lastUpdate = now;
        if (timeDelta > 5.0) timeDelta = 0;   // 防休眠唤醒后的数据突刺（上游 :403）

        _tickCounter++;
        bool isSlowScanTick = (_tickCounter % 3 == 0);     // 3 秒一次（上游 :191）
        bool needDiskBgScan = (_tickCounter % 10 == 0);    // 10 秒一次（上游 :193）

        lock (_lock)
        {
            // 传感器映射兜底重建（10 分钟）上游 :188
            _sensorMap.EnsureFresh(computer);

            foreach (var hw in computer.Hardware)
            {
                try
                {
                    switch (hw.HardwareType)
                    {
                        case HardwareType.Cpu:
                            hw.Update();                                  // 上游 :197
                            break;

                        case HardwareType.GpuNvidia:
                        case HardwareType.GpuAmd:
                        case HardwareType.GpuIntel:
                            // 上游 :200-211：GPU Update 可能抛异常（双显卡切换），标记重载而不是崩掉采集
                            try { hw.Update(); }
                            catch { ReloadRequested = true; }
                            break;

                        case HardwareType.Memory:
                            hw.Update();                                  // 上游 :213
                            break;

                        case HardwareType.Battery:
                            hw.Update();                                  // 上游 :214
                            break;

                        case HardwareType.Network:
                            // 上游 :216-221 → NetworkManager.ProcessUpdate(timeDelta, isSlowScanTick)
                            observeNetwork?.Invoke(timeDelta);
                            break;

                        case HardwareType.Storage:
                            // 上游 :222-226 → DiskManager.ProcessUpdate(isSlowScanTick, needDiskBgScan)
                            _diskManager.ProcessUpdate(hw, isSlowScanTick, needDiskBgScan);
                            break;

                        case HardwareType.Motherboard:
                        case HardwareType.SuperIO:
                        case HardwareType.Cooler:
                            // 上游 :229-239：主板/SuperIO 只在慢扫周期更新，禁止高频
                            if (isSlowScanTick) UpdateWithSubHardware(hw);
                            break;
                    }
                }
                catch
                {
                    // 采集异常隔离：单个硬件失败不中断其余硬件（CONTRACT-v3.md §5.7 健壮性要求）
                }
            }
        }

        return true;
    }

    /// <summary>递归更新子硬件（SuperIO 刷新）。上游 HardwareMonitor.cs:546-550。</summary>
    private static void UpdateWithSubHardware(IHardware hw)
    {
        hw.Update();
        foreach (var sub in hw.SubHardware) UpdateWithSubHardware(sub);
    }

    /// <summary>代表显卡（SensorMap 冲突解决结果）。上游 SensorMap.cs:30。</summary>
    public IHardware? CachedGpu => _sensorMap.CachedGpu;

    /// <summary>全部显卡硬件（含非代表卡，契约 §3 gpu[] 期望列出可用 GPU）。</summary>
    public List<IHardware> GpuHardware()
    {
        var computer = _computer;
        if (computer == null) return new List<IHardware>();
        return computer.Hardware.Where(SensorMap.IsGpuHardware).ToList();
    }

    /// <summary>全部网络硬件（tier-2 路径下才枚举得到；NetworkManager 兜底通道）。</summary>
    public List<IHardware> NetworkHardware()
    {
        var computer = _computer;
        if (computer == null) return new List<IHardware>();
        return computer.Hardware.Where(h => h.HardwareType == HardwareType.Network).ToList();
    }


    /// <summary>
    /// 在当前硬件树上选定并锁定网络硬件（供采集循环在开树后调用一次）。
    ///
    /// CONTRACT-v3.md §8 要求完整移植并驱动 `NetworkManager`。只调用 `ProcessUpdate`
    /// 而不先选定目标硬件时，`_cachedNetHw` 恒为 null、`isTarget` 恒 false，
    /// 于是流量累积与 `ConsumeDelta` 全程空转（整条吞吐链路只写不读）。
    /// 选定需要 LHM 的 `IComputer`，故把调用点放在本模块内，不对外暴露硬件树引用。
    /// </summary>
    public void SelectNetworkHardware(NetworkManager network)
    {
        var computer = _computer;
        if (computer != null) network.SelectBestNetworkHardware(computer);
    }

    /// <summary>主板节点（tier-2）。</summary>
    public IHardware? MotherboardHardware()
    {
        var computer = _computer;
        return computer?.Hardware.FirstOrDefault(h => h.HardwareType == HardwareType.Motherboard);
    }

    /// <summary>
    /// tier-2 原始读数：CPU 温度（核心最大）/核心温度数组/功耗（600W 熔断）/电压/风扇/水泵、
    /// 主板节点、代表磁盘温度。全部来自 SensorMap 映射结果（上游 SensorMap/SensorMatcher）。
    /// </summary>
    public SnapshotBuilder.Tier2Readout ReadTier2()
    {
        var r = new SnapshotBuilder.Tier2Readout();

        // CPU 温度：上游 ComponentProcessor.GetCpuTemp()（核心最大值，排除 Distance/Average/Max）
        if (_processor is { } proc)
        {
            r.CpuTempC = proc.GetCpuTemp();
        }

        // 核心温度数组：所有被 SensorMatcher 匹配为 CPU.Temp 的温度传感器
        var computer = _computer;
        var cpuNode = computer?.Hardware.FirstOrDefault(h => h.HardwareType == HardwareType.Cpu);
        if (cpuNode != null)
        {
            var temps = new List<double>();
            foreach (var s in cpuNode.Sensors)
            {
                if (SensorMatcher.Match(cpuNode, s) != SensorKeys.CpuTemp) continue;
                if (s.Value is float v && !float.IsNaN(v) && v > 0) temps.Add(v);
            }
            r.CoreTempsC = temps.ToArray();
        }

        // 功耗（>600W 熔断，上游 ComponentProcessor.cs:148）
        if (_sensorMap.TryGetSensor(SensorKeys.CpuPower, out var powerSensor))
        {
            r.CpuPowerW = ComponentProcessor.ApplyCpuPowerClamp(powerSensor);
        }

        // 电压
        if (_sensorMap.TryGetSensor(SensorKeys.CpuVoltage, out var voltSensor) &&
            voltSensor?.Value is float vv && !float.IsNaN(vv) && vv > 0)
        {
            r.CpuVoltageV = vv;
        }

        // 风扇 / 水泵（FanMapper 200RPM 门槛 + cooler 检测等，上游 FanMapper.cs）
        if (_sensorMap.TryGetSensor("CPU.Fan", out var fan) && fan?.Value is float fv && !float.IsNaN(fv) && fv > 0)
            r.CpuFanRpm = fv;
        if (_sensorMap.TryGetSensor("CPU.Pump", out var pump) && pump?.Value is float pv && !float.IsNaN(pv) && pv > 0)
            r.CpuPumpRpm = pv;

        // 主板节点（SuperIO 温度/风扇/电压递归收集，仅 temperature/fan/voltage）
        r.Motherboard = SnapshotBuilder.BuildMotherboard(MotherboardHardware());

        // 磁盘温度：卷 → 物理盘映射（含代表磁盘回退）
        var diskTemps = ReadDiskTemperatures(DiskManager.EnumerateVolumes());
        foreach (var kv in diskTemps) r.DiskTemps[kv.Key] = kv.Value;
        if (_computer != null)
        {
            r.RepresentativeDiskTempC =
                _diskManager.GetDiskTemperature(_diskManager.SelectBestDiskHardware(_computer));
        }

        return r;
    }

    /// <summary>
    /// 诊断用：所有存储硬件（含子硬件）的 "名称 [类型]" 列表。
    /// 磁盘温度（tier-2）只在提权侧可读，卷↔盘映射失败时只能从此处排查。
    /// </summary>
    public List<string> DescribeStorageHardware()
    {
        var list = new List<string>();
        var computer = _computer;
        if (computer == null) return list;

        void Walk(IHardware hw)
        {
            string props = hw.Properties.Count == 0
                ? ""
                : " props{" + string.Join(",", hw.Properties.Select(kv => kv.Key + "=" + kv.Value)) + "}";
            list.Add($"{hw.Name} [{hw.HardwareType}] id={hw.Identifier}{props}");
            foreach (var sub in hw.SubHardware) Walk(sub);
        }

        foreach (var hw in computer.Hardware)
        {
            if (hw.HardwareType == HardwareType.Storage) Walk(hw);
        }
        return list;
    }

    /// <summary>
    /// 磁盘温度（tier-2）。返回 卷名 → 温度。
    ///
    /// 映射策略（按优先级，任一命中即用）：
    ///   1. 上游盘符启发式（DiskManager.cs:157）：盘名或传感器名含卷标（"C:"）——保留以便
    ///      在 LHM 将来把盘符写进节点名的机型上继续生效；
    ///   2. 物理盘号映射（VolumeDiskMap）：卷 extent 的 DiskNumber ↔ LHM Storage 节点
    ///      Identifier 末段盘号（如 "/nvme/6"）。本机实测 LHM 节点名不含盘符，必须靠这一条
    ///      才能如实定位（见 VolumeDiskMap.cs 顶部说明）。
    /// 均无法定位 → 该卷温度保持缺失（绝不把某块盘的温度安到未经证实的卷上）。
    /// </summary>
    public Dictionary<string, float> ReadDiskTemperatures(IEnumerable<VolumeInfo> volumes)
    {
        var map = new Dictionary<string, float>(StringComparer.OrdinalIgnoreCase);
        var computer = _computer;
        if (computer == null) return map;

        var storages = computer.Hardware.Where(h => h.HardwareType == HardwareType.Storage).ToList();
        var volumeList = volumes.ToList();

        // 盘号 → 存储硬件（Identifier 末段数字，解析不出则跳过）
        var byDiskNumber = new Dictionary<uint, IHardware>();
        foreach (var hw in storages)
        {
            uint? n = VolumeDiskMap.ParseDiskNumberFromIdentifier(hw.Identifier?.ToString());
            if (n.HasValue && !byDiskNumber.ContainsKey(n.Value)) byDiskNumber[n.Value] = hw;
        }

        foreach (var vol in volumeList)
        {
            // 1. 上游盘符启发式 —— 上游 DiskManager.cs:157
            var hw = storages.FirstOrDefault(h =>
                SensorMap.Has(h.Name, vol.Name) || h.Sensors.Any(s => SensorMap.Has(s.Name, vol.Name)));

            // 2. 物理盘号映射（Win32 卷 extent）—— 见 VolumeDiskMap.cs
            if (hw == null)
            {
                uint? diskNumber = VolumeDiskMap.GetPhysicalDiskNumber(vol.Name);
                if (diskNumber.HasValue) byDiskNumber.TryGetValue(diskNumber.Value, out hw);
            }

            if (hw == null) continue;   // 无法证实 → 保持缺失

            var temp = _diskManager.GetDiskTemperature(hw);
            if (temp.HasValue) map[vol.Name] = temp.Value;
        }

        return map;
    }

    /// <summary>
    /// 从 LHM 电池节点读取契约 battery 对象（tier-1）。
    /// 无电池节点／无有效传感器 → null（契约 §3：无电池时为 null）。
    /// 符号规则来自 BatteryService（上游 BatteryService.cs:30-52）。
    /// </summary>
    public BatteryData? ReadBattery()
    {
        var computer = _computer;
        if (computer == null) return null;

        var batteryHw = computer.Hardware.FirstOrDefault(h => h.HardwareType == HardwareType.Battery);
        if (batteryHw == null) return null;

        // 按 SensorMatcher 建立 key → sensor 缓存（上游 SensorMap + SensorMatcher 的等价局部映射）
        var cache = new Dictionary<string, ISensor>(StringComparer.OrdinalIgnoreCase);
        foreach (var s in batteryHw.Sensors)
        {
            string? key = SensorMatcher.Match(batteryHw, s);
            if (key != null && !cache.ContainsKey(key)) cache[key] = s;
        }

        float? percent = BatteryService.GetBatteryValue(SensorKeys.BatPercent, cache);
        float? power = BatteryService.GetBatteryValue(SensorKeys.BatPower, cache);
        float? voltage = BatteryService.GetBatteryValue(SensorKeys.BatVoltage, cache);
        float? current = BatteryService.GetBatteryValue(SensorKeys.BatCurrent, cache);

        // 无任何有效值 → 视为无电池（不伪造 0）
        if (!percent.HasValue && !power.HasValue && !voltage.HasValue && !current.HasValue) return null;

        return new BatteryData
        {
            Percent = percent,
            PowerW = power,
            VoltageV = voltage,
            CurrentA = current
        };
    }

    /// <summary>
    /// LHM 内存节点名（best-effort，契约 §3 memory.name）。
    /// 优先取非 Virtual 的物理内存节点（上游 SensorMatcher 用 hw.Name 含 "virtual" 排除虚拟内存，
    /// 见 SensorMatcher.cs:116）。
    /// </summary>
    public string? ReadMemoryName()
    {
        var computer = _computer;
        if (computer == null) return null;

        var nodes = computer.Hardware.Where(h => h.HardwareType == HardwareType.Memory).ToList();
        if (nodes.Count == 0) return null;

        var physical = nodes.FirstOrDefault(n => !SensorMap.Has(n.Name, "virtual")) ?? nodes[0];
        return physical.Name;
    }

    /// <summary>
    /// LHM 推算的物理内存（契约 §3 memory，best-effort；SECM 仍以 GlobalMemoryStatusEx 为权威）。
    /// 用上游 SensorMatcher 的 MEM.Used / MEM.Available 键值相加得到总量。
    /// </summary>
    public (long? Total, long? Used) ReadMemoryFromLhm()
    {
        var computer = _computer;
        if (computer == null) return (null, null);

        var node = computer.Hardware
            .Where(h => h.HardwareType == HardwareType.Memory)
            .FirstOrDefault(n => !SensorMap.Has(n.Name, "virtual"));
        if (node == null) return (null, null);

        float? used = null, avail = null;
        foreach (var s in node.Sensors)
        {
            string? key = SensorMatcher.Match(node, s);
            if (key == null) continue;
            if (s.Value is not float v || float.IsNaN(v) || v <= 0) continue;

            // Data / SmallData 的单位是 GB（LHM 内存节点约定），换算为字节
            double bytes = v * 1024.0 * 1024.0 * 1024.0;
            if (key == SensorKeys.MemUsed && used == null) used = (float)(bytes / (1024.0 * 1024.0));
            else if (key == SensorKeys.MemAvailable && avail == null) avail = (float)(bytes / (1024.0 * 1024.0));
        }

        if (used == null && avail == null) return (null, null);

        long? usedBytes = used.HasValue ? (long)Math.Round(used.Value * 1024.0 * 1024.0) : null;
        long? totalBytes = (used.HasValue && avail.HasValue)
            ? (long)Math.Round((used.Value + avail.Value) * 1024.0 * 1024.0)
            : null;
        return (totalBytes, usedBytes);
    }

    /// <summary>关闭硬件树（幂等）。上游 HardwareMonitor.cs:275-297 / ReloadComputerSafe:462-506。</summary>
    public void Close()
    {
        lock (_lock)
        {
            try
            {
                _computer?.Close();
            }
            catch
            {
                // 关闭失败不致命（上游 :285-289 同样吞掉）
            }
            _computer = null;
            _processor = null;
            Tier2GroupsEnabled = false;
            CpuName = string.Empty;
            ReloadRequested = false;
            _tickCounter = 0;
        }
        _sensorMap.Clear();
        _diskManager.ClearCache();
    }

    public void Dispose() => Close();
}
