// Hardware/NetworkManager.cs
//
// 移植自 LiteMonitor（MIT）src/System/HardwareServices/NetworkManager.cs
//
// 保持（本次移植的核心）：
//   - NetworkInterface.GetIPStatistics() 增量：BytesSent/BytesReceived 两次采样相减
//     —— 上游 NetworkManager.cs:359-367
//   - LHM Throughput 兜底与"匹配错误"判定：原生增量为 0 而 LHM 增量 > 51200 时
//     判定网卡匹配错误，改用 LHM 值并清空匹配 —— 上游 NetworkManager.cs:374-393
//   - 网卡令牌匹配：名称/描述精确 Equals → 分词模糊匹配（>2 个令牌命中且占比 >0.6）
//     —— 上游 NetworkManager.cs:429-475
//   - 分词分隔符与虚拟网卡黑名单关键字表 —— 上游 NetworkManager.cs:476 / :532-534
//   - 10GB 单次增量安全阀（超限直接丢弃本次采样）—— 上游 NetworkManager.cs:419
//   - SMB 内网流量扣除（含 1.2 协议开销，见 PerformanceCounterManager）—— 上游 :397-415
//
// 移植差异（去设置层/UI 依赖）：
//   - 上游用 cfg.PreferredNetwork / cfg.LastAutoNetwork / cfg.SessionUploadBytes /
//     TrafficLogger。SECM 契约只要求"当前吞吐"，故：网卡选择 = 上游"全盘扫描 + 虚拟网卡惩罚"
//     评分逻辑（上游 :253-289）只跑一次做锁定；会话累计与流量日志不移植（非指标）。
//   - 上游 _startTime 3 秒启动相位抑制（:68）保留，避免启动抖动。
//   - 为避免重复计数，原生计数器未匹配到时**保留**最近一次已持有的 (delta, seconds)，
//     而不是叠加两路增量（上游按 cfg.SessionUploadBytes 累加，SECM 只需速率窗口）。
// 上游行号在每条规则后标注。

using System.Net.NetworkInformation;
using LibreHardwareMonitor.Hardware;

namespace LhmSidecar.Hardware;

/// <summary>网络吞吐管理器（tier-1）。</summary>
public sealed class NetworkManager
{
    /// <summary>每个硬件的独立网络状态。上游 NetworkManager.cs:16-26。</summary>
    private sealed class NetworkState
    {
        public NetworkInterface? NativeAdapter;
        public long LastNativeUpload;
        public long LastNativeDownload;
        public DateTime LastMatchAttempt = DateTime.MinValue;

        // 缓存 LHM 传感器
        public ISensor? CachedUpSensor;
        public ISensor? CachedDownSensor;
    }

    private readonly Dictionary<IHardware, NetworkState> _netStates = new();
    private IHardware? _cachedNetHw;
    private readonly DateTime _startTime = DateTime.Now;   // 上游 :32
    private readonly PerformanceCounterManager? _perfManager;
    private readonly bool _ignoreSmbTraffic;
    private readonly object _deltaLock = new();

    // 最近一次"待消费"的增量与窗口（供快照换算 Bytes/sec）
    private long _deltaUp;
    private long _deltaDown;
    private double _deltaSeconds;

    public NetworkManager(PerformanceCounterManager? perfManager = null, bool ignoreSmbTraffic = true)
    {
        _perfManager = perfManager;                    // 上游 :40-43（允许为空则内部禁用）
        _ignoreSmbTraffic = ignoreSmbTraffic;          // 上游 Settings.IgnoreSmbTraffic 默认 true（Settings.cs:119）
    }

    /// <summary>清空状态缓存。上游 NetworkManager.cs:53-57。</summary>
    public void ClearCache()
    {
        _netStates.Clear();
        _cachedNetHw = null;
    }

    /// <summary>
    /// 每次采集对单个网络硬件调用。上游 NetworkManager.cs:62-83。
    /// </summary>
    public void ProcessUpdate(IHardware hw, double timeDelta, bool isSlowScanTick)
    {
        bool isTarget = (_cachedNetHw != null && ReferenceEquals(hw, _cachedNetHw));

        bool isStartupPhase = (DateTime.Now - _startTime).TotalSeconds < 3;   // 上游 :68

        if (isTarget)
        {
            hw.Update();
            AccumulateTraffic(hw, timeDelta);
        }
        else if (isStartupPhase || IsVirtualNetwork(hw.Name))
        {
            return;                                                            // 上游 :75-78
        }
        else if (isSlowScanTick)
        {
            hw.Update();                                                       // 上游 :79-82
        }
    }

    /// <summary>
    /// 首次选定并锁定网络硬件：上游 GetBestValue 的全盘扫描评分（NetworkManager.cs:253-289）。
    /// 虚拟网卡加 -1e9 惩罚，取 (up + down) 评分最高者。
    /// </summary>
    public IHardware? SelectBestNetworkHardware(IComputer computer)
    {
        if (_cachedNetHw != null && computer.Hardware.Contains(_cachedNetHw)) return _cachedNetHw;

        IHardware? bestHw = null;
        double bestScore = double.MinValue;

        foreach (var hw in computer.Hardware.Where(h => h.HardwareType == HardwareType.Network))
        {
            double penalty = IsVirtualNetwork(hw.Name) ? -1e9 : 0;   // 上游 :260
            ISensor? up = null, down = null;
            foreach (var s in hw.Sensors)
            {
                if (s.SensorType != SensorType.Throughput) continue;
                if (_upKW.Any(k => SensorMap.Has(s.Name, k))) up ??= s;
                if (_downKW.Any(k => SensorMap.Has(s.Name, k))) down ??= s;
            }
            if (up == null && down == null) continue;                // 上游 :268

            double score = (up?.Value ?? 0) + (down?.Value ?? 0) + penalty;
            if (score > bestScore)
            {
                bestScore = score;
                bestHw = hw;
            }
        }

        if (bestHw != null) _cachedNetHw = bestHw;                   // 上游 :279-289
        return bestHw;
    }

    /// <summary>
    /// 取出并清空自上次调用以来累积的流量增量（含窗口秒数）。
    /// 快照按 delta / seconds 换算 Bytes/sec。
    /// </summary>
    public (long UpBytes, long DownBytes, double Seconds) ConsumeDelta()
    {
        lock (_deltaLock)
        {
            var r = (_deltaUp, _deltaDown, _deltaSeconds);
            _deltaUp = 0;
            _deltaDown = 0;
            _deltaSeconds = 0;
            return r;
        }
    }

    /// <summary>
    /// 流量累积与网卡匹配。上游 NetworkManager.cs:323-427。
    /// </summary>
    private void AccumulateTraffic(IHardware hw, double seconds)
    {
        if (!_netStates.TryGetValue(hw, out var state))
        {
            state = new NetworkState();
            _netStates[hw] = state;
        }

        long finalUp = 0;
        long finalDown = 0;

        // A. LHM 估算值 —— 上游 :335-346
        if (state.CachedUpSensor == null || state.CachedDownSensor == null)
        {
            foreach (var s in hw.Sensors)
            {
                if (s.SensorType != SensorType.Throughput) continue;
                if (_upKW.Any(k => SensorMap.Has(s.Name, k))) state.CachedUpSensor ??= s;
                if (_downKW.Any(k => SensorMap.Has(s.Name, k))) state.CachedDownSensor ??= s;
            }
        }
        long lhmUpDelta = (long)((state.CachedUpSensor?.Value ?? 0) * seconds);
        long lhmDownDelta = (long)((state.CachedDownSensor?.Value ?? 0) * seconds);

        // B. 原生精准值 —— 上游 :348-371
        MatchNativeNetworkAdapter(hw.Name, state);

        bool nativeValid = false;
        long nativeUpDelta = 0;
        long nativeDownDelta = 0;

        if (state.NativeAdapter != null)
        {
            try
            {
                var stats = state.NativeAdapter.GetIPStatistics();
                long currUp = stats.BytesSent;
                long currDown = stats.BytesReceived;

                if (currUp >= state.LastNativeUpload) nativeUpDelta = currUp - state.LastNativeUpload;
                if (currDown >= state.LastNativeDownload) nativeDownDelta = currDown - state.LastNativeDownload;

                state.LastNativeUpload = currUp;
                state.LastNativeDownload = currDown;
                nativeValid = true;
            }
            catch { state.NativeAdapter = null; }   // 上游 :370
        }

        // C. 决策时刻 —— 上游 :373-393
        if (nativeValid)
        {
            if ((nativeUpDelta + nativeDownDelta == 0) && (lhmUpDelta + lhmDownDelta > 51200))
            {
                // 匹配错误（上游阈值 51200 = 50KB）
                finalUp = lhmUpDelta;
                finalDown = lhmDownDelta;
                state.NativeAdapter = null;
                state.LastMatchAttempt = DateTime.MinValue; // 允许立即重新匹配（上游靠 10s 冷却，这里的重置等价于下一次尝试）
            }
            else
            {
                finalUp = nativeUpDelta;
                finalDown = nativeDownDelta;
            }
        }
        else
        {
            finalUp = lhmUpDelta;
            finalDown = lhmDownDelta;
        }

        // SMB 内网流量扣除 —— 上游 :395-415
        if (_ignoreSmbTraffic && _perfManager is { IsInitialized: true })
        {
            var smb = _perfManager.GetEstimatedSmbBytes(seconds);

            if (smb.UpBytes > 0 || smb.DownBytes > 0)
            {
                // 防止扣成负数
                if (smb.UpBytes > 0) finalUp = Math.Max(0, finalUp - smb.UpBytes);
                if (smb.DownBytes > 0) finalDown = Math.Max(0, finalDown - smb.DownBytes);
            }
        }

        // D. 安全阀：单次增量超过 10GB 视为异常丢弃 —— 上游 :419
        if (finalUp > 10737418240L || finalDown > 10737418240L) return;

        if (finalUp > 0 || finalDown > 0)
        {
            lock (_deltaLock)
            {
                _deltaUp += finalUp;
                _deltaDown += finalDown;
                _deltaSeconds += seconds;
            }
        }
        else if (seconds > 0)
        {
            // 保留窗口长度（速率为 0 但窗口有效）
            lock (_deltaLock) { _deltaSeconds += seconds; }
        }
    }

    /// <summary>
    /// 匹配原生网卡。上游 NetworkManager.cs:429-475。
    /// </summary>
    private void MatchNativeNetworkAdapter(string lhmName, NetworkState state)
    {
        if (state.NativeAdapter != null) return;
        // 10 秒冷却 —— 上游 :432
        if ((DateTime.Now - state.LastMatchAttempt).TotalSeconds < 10) return;
        state.LastMatchAttempt = DateTime.Now;

        try
        {
            var nics = NetworkInterface.GetAllNetworkInterfaces();

            var lhmTokens = new List<string>(capacity: 10);
            SplitTokens(lhmName, lhmTokens);

            foreach (var nic in nics)
            {
                // 1. 匹配连接名称 —— 上游 :446
                if (nic.Name.Equals(lhmName, StringComparison.OrdinalIgnoreCase)) { SetNativeAdapter(nic, state); return; }
                // 2. 匹配硬件描述 —— 上游 :448
                if (nic.Description.Equals(lhmName, StringComparison.OrdinalIgnoreCase)) { SetNativeAdapter(nic, state); return; }
                // 3. 模糊匹配 —— 上游 :450-471
                if (lhmTokens.Count > 0 && lhmName.Length > 5)
                {
                    var nicTokens = new List<string>(capacity: 10);
                    SplitTokens(nic.Description, nicTokens);

                    int matchCount = 0;
                    foreach (var token in lhmTokens)
                    {
                        if (nicTokens.Contains(token, StringComparer.OrdinalIgnoreCase))
                        {
                            matchCount++;
                            if (matchCount > 2) break;   // 提前退出
                        }
                    }

                    if (matchCount > 2 && (double)matchCount / lhmTokens.Count > 0.6)
                    {
                        SetNativeAdapter(nic, state);
                        return;
                    }
                }
            }
        }
        catch { state.NativeAdapter = null; }   // 上游 :474
    }

    /// <summary>令牌分隔符。上游 NetworkManager.cs:476。</summary>
    private static readonly char[] _tokenSeparators = { ' ', '(', ')', '[', ']', '-', '_', '#' };

    /// <summary>分词。上游 NetworkManager.cs:478-501（UIUtils.Intern 字符串池为 UI 优化，省去）。</summary>
    private static void SplitTokens(string input, List<string> result)
    {
        result.Clear();
        int startIndex = 0;
        int length = input.Length;

        for (int i = 0; i < length; i++)
        {
            if (Array.IndexOf(_tokenSeparators, input[i]) >= 0)
            {
                if (i > startIndex)
                {
                    result.Add(input.Substring(startIndex, i - startIndex));
                }
                startIndex = i + 1;
            }
        }

        if (startIndex < length)
        {
            result.Add(input.Substring(startIndex));
        }
    }

    /// <summary>设置原生网卡并记录基线。上游 NetworkManager.cs:511-521。</summary>
    private static void SetNativeAdapter(NetworkInterface nic, NetworkState state)
    {
        state.NativeAdapter = nic;
        try
        {
            var stats = nic.GetIPStatistics();
            state.LastNativeUpload = stats.BytesSent;
            state.LastNativeDownload = stats.BytesReceived;
        }
        catch { state.NativeAdapter = null; }
    }

    /// <summary>是否虚拟网卡。上游 NetworkManager.cs:523-530。</summary>
    public static bool IsVirtualNetwork(string? name)
    {
        if (string.IsNullOrEmpty(name)) return false;
        foreach (var k in _virtualNicKW)
        {
            if (name.IndexOf(k, StringComparison.OrdinalIgnoreCase) >= 0) return true;
        }
        return false;
    }

    /// <summary>上/下行关键字表与虚拟网卡黑名单。上游 NetworkManager.cs:532-534（原样，不要"改进"）。</summary>
    private static readonly string[] _upKW = { "upload", "up", "sent", "send", "tx", "transmit" };
    private static readonly string[] _downKW = { "download", "down", "received", "receive", "rx" };
    private static readonly string[] _virtualNicKW = { "virtual", "vmware", "hyper-v", "hyper v", "vbox", "loopback", "tunnel", "tap", "tun", "bluetooth", "zerotier", "tailscale", "wan miniport" };
}
