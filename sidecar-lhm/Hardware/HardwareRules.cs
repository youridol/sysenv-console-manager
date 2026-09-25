// Hardware/HardwareRules.cs
//
// 移植自 LiteMonitor（MIT）src/System/HardwareServices/HardwareRules.cs
// 逐条保持优先级数字、关键字与比较语义不变（CONTRACT-v3.md §5.1 冻结）。
// 上游行号在每条规则后标注，便于对照 review。

using LibreHardwareMonitor.Hardware;

namespace LhmSidecar.Hardware;

/// <summary>
/// 硬件规则（原 HardwareClassifier）：显卡优先级、独显核显判定、共享显存策略。
/// </summary>
public static class HardwareRules
{
    /// <summary>
    /// 字符串包含匹配（忽略大小写）。上游 HardwareRules.cs:14-18。
    /// 语义与上游一致：source/sub 任一为空 → false。
    /// </summary>
    public static bool Has(string? source, string? sub)
    {
        if (string.IsNullOrEmpty(source) || string.IsNullOrEmpty(sub)) return false;
        return source.AsSpan().Contains(sub.AsSpan(), StringComparison.OrdinalIgnoreCase);
    }

    /// <summary>
    /// 硬件优先级（数字越小越强）：0 顶级独显 / 1 顶级核显 / 2 中端 / 3 低端 / 4 其他 / 100 垃圾。
    /// 上游 HardwareRules.cs:24-64。
    /// </summary>
    public static int GetHwPriority(IHardware hw)
    {
        string name = hw.Name;

        // 1. Microsoft Basic Render Driver 永远垫底 —— 上游 HardwareRules.cs:29
        if (name.Contains("Basic Render", StringComparison.OrdinalIgnoreCase)) return 100;

        // 2. Nvidia 默认视为独显/最强 —— 上游 HardwareRules.cs:32
        if (hw.HardwareType == HardwareType.GpuNvidia) return 0;

        // 3. AMD —— 上游 HardwareRules.cs:35-41
        if (hw.HardwareType == HardwareType.GpuAmd)
        {
            // 通用名 "AMD Radeon(TM) Graphics" 通常是核显 -> 2（严格 Equals，非 Contains）
            if (name.Equals("AMD Radeon(TM) Graphics", StringComparison.OrdinalIgnoreCase)) return 2;
            return 0;
        }

        // 4. Intel —— 上游 HardwareRules.cs:44-60
        if (hw.HardwareType == HardwareType.GpuIntel)
        {
            if (name.Contains("Arc", StringComparison.OrdinalIgnoreCase))
            {
                if (IsDiscreteArc(name)) return 0; // Arc 独显 A/B/Pro
                return 1;                          // Arc 核显
            }
            if (name.Contains("Iris", StringComparison.OrdinalIgnoreCase)) return 2;
            if (name.Contains("UHD", StringComparison.OrdinalIgnoreCase)) return 3;
        }

        // 其他 —— 上游 HardwareRules.cs:63
        return 4;
    }

    /// <summary>
    /// 是否独显版 Arc（A/B/Pro 系列）。上游 HardwareRules.cs:69-78。
    /// 启发式：包含 " A" / " B" / " Pro"（注意前导空格）。
    /// </summary>
    public static bool IsDiscreteArc(string name)
    {
        if (string.IsNullOrEmpty(name)) return false;
        if (!name.Contains("Arc", StringComparison.OrdinalIgnoreCase)) return false;
        return Has(name, " A") || Has(name, " B") || Has(name, " Pro");
    }

    /// <summary>
    /// 是否应优先使用共享内存（Shared Memory）。上游 HardwareRules.cs:83-97。
    /// 仅 Intel 核显为 true；Intel 独显 Arc A/B/Pro 与 Nvidia/AMD 均为 false。
    /// </summary>
    public static bool ShouldUseSharedMemory(IHardware hw)
    {
        if (hw.HardwareType == HardwareType.GpuIntel)
        {
            if (IsDiscreteArc(hw.Name)) return false;
            return true;
        }
        return false;
    }
}
