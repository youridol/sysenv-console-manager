// Hardware/FanMapper.cs
//
// 移植自 LiteMonitor（MIT）src/System/HardwareServices/FanMapper.cs
// 逐条保持：
//   - 风扇黑名单（GPU/Cpu/Storage/Memory/Network 类型排除）—— 上游 FanMapper.cs:28-34
//   - 200 RPM 底噪门槛（严格 > 200）—— 上游 FanMapper.cs:44
//   - CPU 风扇：cooler 硬件（非 Pump 名）→ 名字含 CPU → 第一个
//     —— 上游 FanMapper.cs:68-84
//   - 水泵：cooler+Pump/Speed → cooler → Pump/Water/AIO 名 → 转速 >3000 取最大
//     —— 上游 FanMapper.cs:89-114
//   - 机箱风扇：Rear → Chassis → Sys → Case → 最低 RPM；若水泵仍空且剩余 >1 则取最高 RPM 当水泵
//     —— 上游 FanMapper.cs:119-135
// 上游依赖 Settings（PreferredCpuFan/PreferredCaseFan/PreferredCpuPump 手动锁定）——
// SECM 无该设置层，对应分支省略（等价于三个 Preferred* 均为空）。
// 上游行号在每条规则后标注。

using LibreHardwareMonitor.Hardware;

namespace LhmSidecar.Hardware;

/// <summary>专用服务：智能识别并映射风扇/水泵/机箱风扇。</summary>
public sealed class FanMapper
{
    /// <summary>
    /// 全系统扫描并写入 CPU.Fan / CASE.Fan / CPU.Pump。上游 FanMapper.cs:16-141。
    /// </summary>
    public void ScanAndMapFans(IComputer computer, Dictionary<string, ISensor> targetMap)
    {
        ISensor? cpuFan = null;
        ISensor? cpuPump = null;
        ISensor? caseFan = null;

        // 1. 搜集全系统所有"活着"的风扇（黑名单机制）
        var activeFans = new List<(IHardware Hw, ISensor S, float Rpm)>();

        void CollectFans(IHardware hw)
        {
            // 黑名单：坚决排除这些类型的风扇 —— 上游 FanMapper.cs:28-34
            bool isExcluded = hw.HardwareType == HardwareType.GpuNvidia ||
                              hw.HardwareType == HardwareType.GpuAmd ||
                              hw.HardwareType == HardwareType.GpuIntel ||
                              hw.HardwareType == HardwareType.Cpu ||
                              hw.HardwareType == HardwareType.Storage ||
                              hw.HardwareType == HardwareType.Memory ||
                              hw.HardwareType == HardwareType.Network;

            if (!isExcluded)
            {
                hw.Update();   // 上游 FanMapper.cs:38
                foreach (var s in hw.Sensors)
                {
                    if (s.SensorType == SensorType.Fan && s.Value.HasValue)
                    {
                        // [底噪过滤] 门槛 200 RPM —— 上游 FanMapper.cs:44
                        if (s.Value.Value > 200)
                        {
                            activeFans.Add((hw, s, s.Value.Value));
                        }
                    }
                }
            }
            foreach (var sub in hw.SubHardware) CollectFans(sub);
        }
        foreach (var hw in computer.Hardware) CollectFans(hw);

        bool IsTaken(ISensor s) => ReferenceEquals(s, cpuFan) || ReferenceEquals(s, cpuPump) || ReferenceEquals(s, caseFan);

        // 步骤 2（手动配置锁定）在上游为 Settings.Preferred* 分支，SECM 无设置层 → 全部为空时跳过。

        // -----------------------------------------------------------
        // 步骤 3：确立 CPU 风扇 —— 上游 FanMapper.cs:68-84
        // -----------------------------------------------------------
        if (cpuFan == null && activeFans.Count > 0)
        {
            var coolerFan = activeFans.FirstOrDefault(x => !IsTaken(x.S) &&
                IsCoolerHardware(x.Hw) && !SensorMap.Has(x.S.Name, "Pump"));

            if (coolerFan.S != null) cpuFan = coolerFan.S;
            else
            {
                var namedCpu = activeFans.FirstOrDefault(x => !IsTaken(x.S) && SensorMap.Has(x.S.Name, "CPU"));
                if (namedCpu.S != null) cpuFan = namedCpu.S;
                else
                {
                    var first = activeFans.FirstOrDefault(x => !IsTaken(x.S));
                    if (first.S != null) cpuFan = first.S;
                }
            }
        }

        // -----------------------------------------------------------
        // 步骤 4：确立水泵 —— 上游 FanMapper.cs:89-114
        // -----------------------------------------------------------
        if (cpuPump == null && activeFans.Count > 0)
        {
            var coolerPump = activeFans.FirstOrDefault(x => !IsTaken(x.S) &&
                IsCoolerHardware(x.Hw) &&
                (SensorMap.Has(x.S.Name, "Pump") || SensorMap.Has(x.S.Name, "Speed")));

            if (coolerPump.S == null)
                coolerPump = activeFans.FirstOrDefault(x => !IsTaken(x.S) && IsCoolerHardware(x.Hw));

            if (coolerPump.S != null) cpuPump = coolerPump.S;
            else
            {
                var namedPump = activeFans.FirstOrDefault(x => !IsTaken(x.S) &&
                    (SensorMap.Has(x.S.Name, "Pump") || SensorMap.Has(x.S.Name, "Water") || SensorMap.Has(x.S.Name, "AIO")));
                if (namedPump.S != null) cpuPump = namedPump.S;
            }
        }

        // 智能猜想水泵（转速 > 3000，取最高）—— 上游 FanMapper.cs:108-114
        if (cpuPump == null)
        {
            var highSpeedCandidate = activeFans.Where(x => !IsTaken(x.S) && x.Rpm > 3000)
                                               .OrderByDescending(x => x.Rpm)
                                               .FirstOrDefault();
            if (highSpeedCandidate.S != null) cpuPump = highSpeedCandidate.S;
        }

        // -----------------------------------------------------------
        // 步骤 5：确立机箱风扇 —— 上游 FanMapper.cs:119-135
        // -----------------------------------------------------------
        if (caseFan == null && activeFans.Count > 0)
        {
            var leftovers = activeFans.Where(x => !IsTaken(x.S)).ToList();

            var best = leftovers.FirstOrDefault(x => SensorMap.Has(x.S.Name, "Rear"));
            if (best.S == null) best = leftovers.FirstOrDefault(x => SensorMap.Has(x.S.Name, "Chassis"));
            if (best.S == null) best = leftovers.FirstOrDefault(x => SensorMap.Has(x.S.Name, "Sys"));
            if (best.S == null) best = leftovers.FirstOrDefault(x => SensorMap.Has(x.S.Name, "Case"));

            if (best.S != null) caseFan = best.S;
            else if (leftovers.Count > 0)
            {
                var sorted = leftovers.OrderBy(x => x.Rpm).ToList();
                caseFan = sorted.First().S;
                if (cpuPump == null && sorted.Count > 1) cpuPump = sorted.Last().S;
            }
        }

        // 3. 写入 Map —— 上游 FanMapper.cs:138-140
        if (cpuFan != null) targetMap["CPU.Fan"] = cpuFan;
        if (caseFan != null) targetMap["CASE.Fan"] = caseFan;
        if (cpuPump != null) targetMap["CPU.Pump"] = cpuPump;
    }

    /// <summary>cooler 检测。上游 FanMapper.cs:153-158。</summary>
    private static bool IsCoolerHardware(IHardware h)
    {
        if (h.HardwareType == HardwareType.Cooler) return true;
        string n = h.Name;
        return SensorMap.Has(n, "Kraken") || SensorMap.Has(n, "Corsair") ||
               SensorMap.Has(n, "Liquid") || SensorMap.Has(n, "AIO") || SensorMap.Has(n, "Cooler");
    }
}
