// Hardware/BatteryService.cs
//
// 移植自 LiteMonitor（MIT）src/System/HardwareServices/BatteryService.cs
//
// 核心语义（CONTRACT-v3.md §5.1 冻结）：AC 在线符号归一 ——
//   插电（AcOnline=true）→ 功率/电流取 +|v|（输入/充电，正数）
//   电池供电（AcOnline=false）→ 强制 -|v|（输出/放电，必定负数）
// 无论传感器原始值是正还是负（不同驱动标准不一），都只按"是否插电"强制赋号。
// —— 上游 BatteryService.cs:30-52
//
// 移植差异：上游的模拟模式（GetSimulatedValue，:59-83）是 UI 测试开关（simulateBattery
// 恒为 false，:19），属测试脚手架而非指标语义，且会产出假数据（与"绝不伪造 0"精神冲突），
// 故不移植。
// 上游行号在每条规则后标注。

using LibreHardwareMonitor.Hardware;

namespace LhmSidecar.Hardware;

/// <summary>电池服务：取值 + AC 在线符号修正。</summary>
public static class BatteryService
{
    /// <summary>
    /// 取电池数值；BAT.Power / BAT.Current 做符号修正。上游 BatteryService.cs:16-57。
    /// 传感器缺失或无值时返回 null（绝不伪造 0）。
    /// </summary>
    public static float? GetBatteryValue(string key, IReadOnlyDictionary<string, ISensor> sensorCache)
    {
        if (sensorCache.TryGetValue(key, out var sensor) && sensor.Value.HasValue)
        {
            float val = sensor.Value.Value;

            // 符号修正：充电时为正，放电时为负 —— 上游 :30-52
            if (key is "BAT.Power" or "BAT.Current")
            {
                // 上游用 MetricUtils.GetPowerStatus()（3000ms 节流）。这里用无节流的直接查询，
                // 保证本次采样的符号与当前 AC 状态严格一致（节流会让插拔瞬间符号滞后）。
                var powerStatus = MetricUtils.QueryPowerStatus() ?? MetricUtils.GetPowerStatus();

                if (powerStatus.AcOnline)
                {
                    val = Math.Abs(val);          // 上游 :41
                }
                else
                {
                    val = -Math.Abs(val);         // 上游 :47 —— 强制负号
                }
            }
            return val;
        }

        return null;   // 上游 :56
    }
}
