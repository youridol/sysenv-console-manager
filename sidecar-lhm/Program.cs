// LhmSidecar —— LibreHardwareMonitor CPU 传感器 sidecar（进程隔离，MPL-2.0 隔离边界）
//
// 架构：
//   SECM（Rust, MIT）通过 HTTP 消费本进程输出的 JSON；LibreHardwareMonitorLib（MPL-2.0）
//   仅在本进程内使用，两进程不链接、不共享内存代码，规避许可对 MIT 主体的传染。
//   内嵌 ring0 驱动 PawnIO（LGPL-2.1，namazso/PawnIO.Modules），设备 \\.\PawnIO。
//
// 权限模型：
//   PawnIO 设备 DACL 仅 SYSTEM/Administrators 可访问，而 SECM 主进程以普通权限
//   （asInvoker）运行 → 本 sidecar 启动时自动请求 UAC 提权（runas 重启自身）：
//     - 未提权且非提权子进程 → 弹 UAC 提权重启（追加 --elevated-child），原进程退出
//     - 用户取消 UAC → 保持普通权限运行，传感器返回 available:false + 需管理员权限错误
//
// 端点（契约，见 docs/designs/2026-08-09-lhm-datasource.md §5.1）：
//   GET /health              → 200 {"status":"ok"}
//   GET /api/lhm/sensors     → 200 传感器 JSON（available/error/cpu 结构）
//   其他路径 → 404；非 GET → 405
//
// 启动参数：
//   --port <port>           监听端口（默认 45980，仅绑定 127.0.0.1 回环）
//   --interval <ms>         采集间隔毫秒（默认 1000，最小 100）
//   --elevated-child        提权子进程标记（内部使用，不再二次提权）
//
// 采集在独立后台线程执行，HTTP 请求只读取最新缓存快照，避免并发采集；
// LHM 打不开设备时不抛异常（返回空/0 值），故显式预检 PawnIO 设备 + 过滤无效温度。
// 日志同时输出 stdout/stderr 与 %TEMP%\secm-lhm-sidecar.log（提权子进程脱离
// SECM 重定向管道后，文件是可靠排障通道）。

using System.ComponentModel;
using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using System.Runtime.Versioning;
using System.Security.Principal;
using System.Text;
using System.Text.Json;
using LibreHardwareMonitor.Hardware;

// sidecar 仅面向 Windows（win-x64 发布），消除 CA1416 平台兼容性警告
[assembly: SupportedOSPlatform("windows")]

namespace LhmSidecar;

// ============================================================================
// 传感器快照 DTO（序列化契约与设计文档 §5.1 对齐，snake_case 字段名）
// ============================================================================

/// CPU 传感器数据（可空字段用 double?，无数据输出 null 而非缺字段，契约明确）。
sealed class CpuSensorData
{
    /// CPU Package 温度（℃），SECM 回填 CpuData.temperature 的主来源
    public double? PackageTempC { get; set; }
    /// 各核心/CCD 温度数组（可选，前端可展示；无则空数组）
    public double[] CoreTempsC { get; set; } = Array.Empty<double>();
    /// CPU Package 功耗（W，LHM 读取 RAPL/SMU；无则 null）
    public double? PowerW { get; set; }
    /// CPU 风扇转速（RPM，可选扩展字段，SECM 当前不消费）
    public double? FanRpm { get; set; }
    /// CPU 核心电压（V，可选扩展字段，SECM 当前不消费）
    public double? VoltageV { get; set; }
}

/// GPU 传感器数据（单卡一条；字段可空，无数据输出 null，契约明确）。
sealed class GpuSensorData
{
    /// GPU 名称（LHM 硬件节点名，如 "NVIDIA GeForce RTX 3060"）
    public string Name { get; set; } = string.Empty;
    /// GPU 核心温度（℃），无则 null
    public double? TemperatureC { get; set; }
    /// GPU 核心时钟（MHz），无则 null
    public double? CoreClockMhz { get; set; }
    /// GPU 功耗（W，LHM 经 NVAPI/ADL 读取），无则 null
    public double? PowerW { get; set; }
    /// GPU 核心负载（%），无则 null
    public double? LoadPercent { get; set; }
    /// 显存已用（字节），无则 null
    public long? MemoryUsedBytes { get; set; }
    /// 显存总量（字节），无则 null
    public long? MemoryTotalBytes { get; set; }
    /// GPU 风扇转速（RPM），无则 null
    public double? FanRpm { get; set; }
}

/// 主板传感器（SuperIO 子硬件：温度/风扇/电压）。
sealed class MotherboardSensorData
{
    /// 传感器原始名（如 "System"、"CPU Fan"、"+12V"），不做改名
    public string Name { get; set; } = string.Empty;
    /// 传感器类型："temperature"|"fan"|"voltage"（LHM SensorType 映射）
    public string Type { get; set; } = string.Empty;
    /// 传感器当前值（温度℃ / 风扇 RPM / 电压 V）
    public double Value { get; set; }
}

/// 主板数据（LHM Motherboard 节点 + SuperIO 子硬件传感器；主板不可用时整个对象为 null）。
sealed class MotherboardData
{
    /// 主板名称（LHM 硬件节点名），无则 null
    public string? Name { get; set; }
    /// 传感器列表（无数据时空数组，符合契约）
    public List<MotherboardSensorData> Sensors { get; set; } = new();
}

/// 内存数据（SPD 型号为静态数据仅首次读取缓存；容量/已用由 LHM 传感器推算，不伪造）。
sealed class MemoryData
{
    /// SPD 型号汇总（如 "DDR4-2400 2x8GB"），无则 null
    public string? Name { get; set; }
    /// 物理内存总量（字节），推算不出则 null
    public long? TotalBytes { get; set; }
    /// 已用物理内存（字节），无则 null
    public long? UsedBytes { get; set; }
}

/// 传感器响应整体（HTTP /api/lhm/sensors 的 JSON 体）。
sealed class SensorSnapshot
{
    /// 采集是否可用（驱动加载成功 + 有有效温度数据）
    public bool Available { get; set; }
    /// 不可用时的错误描述（含驱动/权限原因，供 SECM 引导卡复用）
    public string? Error { get; set; }
    /// sidecar 契约版本（本次=2；旧 SECM 忽略未知字段、新 SECM 不依赖旧字段 → 向后兼容）
    public int ContractVersion { get; set; } = 2;
    /// CPU 传感器数据
    public CpuSensorData Cpu { get; set; } = new();
    /// GPU 传感器数组（无 GPU/枚举不到时为空数组）
    public List<GpuSensorData> Gpu { get; set; } = new();
    /// 主板数据（SuperIO 传感器；主板不可用时为 null）
    public MotherboardData? Motherboard { get; set; }
    /// 内存数据（SPD + 容量；LHM 内存不可用时为 null）
    public MemoryData? Memory { get; set; }
}

// ============================================================================
// LHM 硬件树遍历（刷新传感器读数）
// ============================================================================

/// LHM 官方推荐遍历方式：VisitComputer 触发整棵硬件树 Update()。
sealed class UpdateVisitor : IVisitor
{
    public void VisitComputer(IComputer computer) => computer.Traverse(this);

    public void VisitHardware(IHardware hardware)
    {
        hardware.Update();
        foreach (IHardware sub in hardware.SubHardware)
        {
            sub.Accept(this);
        }
    }

    public void VisitSensor(ISensor sensor)
    {
        // 传感器值在硬件 Update 时已刷新，无需额外处理
    }

    public void VisitParameter(IParameter parameter)
    {
        // 参数（可调阈值等）本 sidecar 不消费
    }
}

// ============================================================================
// 程序入口
// ============================================================================

static class Program
{
    /// JSON 序列化选项：snake_case 字段名 + UTF-8 无 BOM。
    static readonly JsonSerializerOptions JsonOpts = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower
    };

    /// 快照读写锁（采集线程写、HTTP 线程读）。
    static readonly object SnapshotLock = new();

    /// 最新传感器快照（HTTP 响应直接序列化此对象）。
    static SensorSnapshot _snapshot = new()
    {
        Available = false,
        Error = "LHM 尚未完成首次采集"
    };

    /// 当前 LHM Computer 实例（仅采集线程访问；关闭/释放也仅在该线程）。
    static Computer? _computer;

    /// 上次输出的错误消息（去重，避免驱动/权限失败时每 5s 刷屏一条日志）。
    static string? _lastError;

    /// 传感器清单是否已打印（首次成功采集时输出一次，供 SECM 日志诊断）。
    static bool _sensorListLogged;

    /// SPD 型号汇总缓存（静态数据，仅首次采集读取一次缓存，不进入 1s 轮询热路径）。
    static string? _memorySpdName;

    /// 初始化失败后的重试退避间隔：驱动/设备可能随后可用，周期性重试。
    const int InitRetryMs = 5000;

    /// 当前进程是否以管理员（提权）令牌运行。
    static bool _elevated;

    /// 文件日志路径（提权子进程脱离 SECM 重定向管道后仍可写）。
    static readonly string LogFilePath = Path.Combine(Path.GetTempPath(), "secm-lhm-sidecar.log");

    // ============================================================================
    // 提权（runas 自重启）
    // ============================================================================

    /// 非管理员且非提权子进程时：runas 重启自身（追加 --elevated-child），原进程退出。
    /// 用户取消 UAC → 保持普通权限运行，传感器返回需要管理员权限错误。
    static void TryElevate(string[] args)
    {
        _elevated = IsElevated();
        if (_elevated || args.Contains("--elevated-child", StringComparer.OrdinalIgnoreCase))
        {
            return;
        }

        string exe = Environment.ProcessPath ?? string.Empty;
        if (string.IsNullOrEmpty(exe))
        {
            // 无法定位自身可执行文件：保持未提权运行（传感器将报需要管理员权限）
            return;
        }

        // 重组参数并追加 --elevated-child（Windows 命令行转义：含空格/引号的参数加引号并转义内部引号）
        var quoted = args
            .Where(a => !string.Equals(a, "--elevated-child", StringComparison.OrdinalIgnoreCase))
            .Select(QuoteWindowsArg);
        string argLine = string.Join(" ", quoted);
        if (!string.IsNullOrEmpty(argLine))
        {
            argLine += " ";
        }
        argLine += "--elevated-child";

        try
        {
            Process.Start(new ProcessStartInfo
            {
                FileName = exe,
                Arguments = argLine,
                Verb = "runas",        // UAC 提权
                UseShellExecute = true
            });
            // 提权子进程已拉起（或已存在实例），原进程退出，由子进程接管
            Environment.Exit(0);
        }
        catch (Win32Exception ex)
        {
            // 用户取消 UAC（错误码 1223）或其他失败：继续以普通权限运行
            LogError($"[LhmSidecar] UAC 提权被取消/失败（Win32 错误码 0x{ex.NativeErrorCode:X8}），继续以普通权限运行：传感器将不可用，请以管理员身份运行 SECM");
        }
        catch (Exception ex)
        {
            LogError($"[LhmSidecar] 提权启动失败: {ex.GetType().Name}: {ex.Message}");
        }
    }

    /// Windows 命令行参数转义：含空白/引号时整段加引号，内部引号前置反斜杠转义。
    /// （当前 SECM 仅传 --port 数字常量，无实际注入面；此为防御性完善）
    static string QuoteWindowsArg(string arg)
    {
        if (arg.IndexOfAny(new[] { ' ', '\t', '"' }) < 0)
        {
            return arg;
        }
        return "\"" + arg.Replace("\"", "\\\"") + "\"";
    }

    /// 检测当前进程是否以管理员令牌运行。
    static bool IsElevated()
    {
        try
        {
            using var identity = WindowsIdentity.GetCurrent();
            var principal = new WindowsPrincipal(identity);
            return principal.IsInRole(WindowsBuiltInRole.Administrator);
        }
        catch
        {
            // 令牌查询失败按未提权处理（保守：宁可报需要权限也不静默）
            return false;
        }
    }

    // ============================================================================
    // PawnIO 设备预检（LHM 打不开设备时不抛异常而是返回空/0 值，需显式探测）
    // ============================================================================

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateFileW(string lpFileName, uint dwDesiredAccess, uint dwShareMode,
        IntPtr lpSecurityAttributes, uint dwCreationDisposition, uint dwFlagsAndAttributes,
        IntPtr hTemplateFile);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool CloseHandle(IntPtr hObject);

    const uint GENERIC_READ = 0x8000_0000;
    const uint OPEN_EXISTING = 3;
    const int ERROR_FILE_NOT_FOUND = 2;
    const int ERROR_ACCESS_DENIED = 5;

    /// 探测 WinRing0 设备可达性（\\.\WinRing0_1_2_0，OpenLibSys 已签名驱动，
    /// Secure Boot 下可加载；SECM 驱动安装引导会部署为 SCM 服务）。
    static (bool ok, string? reason) ProbeWinRing0()
    {
        IntPtr h = CreateFileW(@"\\.\WinRing0_1_2_0", GENERIC_READ, 0, IntPtr.Zero, OPEN_EXISTING, 0, IntPtr.Zero);
        if (h == new IntPtr(-1) || h == IntPtr.Zero)
        {
            int err = Marshal.GetLastWin32Error();
            string hint = err switch
            {
                ERROR_FILE_NOT_FOUND => "驱动未就绪：SECM 随包提供 WinRing0x64.sys（WHQL 交叉签名，Secure Boot 可加载），请运行 SECM 的驱动安装引导部署后重试",
                ERROR_ACCESS_DENIED => "当前进程无管理员权限（WinRing0 设备需管理员），请以管理员身份运行 SECM 或允许本进程 UAC 提权",
                _ => $"错误码 0x{err:X8}"
            };
            return (false, $"WinRing0 设备不可访问（CreateFileW \\\\.\\WinRing0_1_2_0 失败：{hint}）");
        }
        CloseHandle(h);
        return (true, null);
    }

    /// 探测 PawnIO 设备可达性（\\.\PawnIO DACL 仅 SYSTEM/Administrators）。
    /// 返回 (可达, 不可达原因)。错误码映射为可读引导文案。
    ///
    /// 签名事实（2026-08-14 核实）：随包 PawnIO 2.2.0 的 WHQL 签名证书虽于
    /// 2026-07-15 到期，但带微软有效时间戳（2025-08-15 签名，时间戳证书
    /// 2026-11-14 到期）——按 Windows 内核驱动签名策略，签名时证书有效且
    /// 时间戳有效即视为有效签名，Secure Boot 下可正常加载（Get-AuthenticodeSignature
    /// = Valid）。因此 PawnIO 为 LHM 主 ring0 通道，不因证书到期降级。
    static (bool ok, string? reason) ProbePawnIo()
    {
        IntPtr h = CreateFileW(@"\\.\PawnIO", GENERIC_READ, 0, IntPtr.Zero, OPEN_EXISTING, 0, IntPtr.Zero);
        if (h == new IntPtr(-1) || h == IntPtr.Zero)
        {
            int err = Marshal.GetLastWin32Error();
            string hint = err switch
            {
                ERROR_FILE_NOT_FOUND => "驱动未就绪：SECM 随包提供 PawnIO 2.2.0（WHQL 签名 + 有效时间戳，Secure Boot 可加载），请运行 SECM 的驱动安装引导自动部署后重试；若部署失败请检查安全软件（Windows Defender 内核隔离 / 火绒驱动防护 / 360 等）是否拦截 PawnIO.sys 加载",
                ERROR_ACCESS_DENIED => "当前进程无管理员权限（PawnIO 设备仅 SYSTEM/Administrators 可访问），请以管理员身份运行 SECM 或允许本进程 UAC 提权",
                _ => $"错误码 0x{err:X8}"
            };
            return (false, $"PawnIO 设备不可访问（CreateFileW \\\\.\\PawnIO 失败：{hint}）");
        }
        CloseHandle(h);
        return (true, null);
    }

    /// 双后端预检：PawnIO（主，LHM 官方 ring0 后端，WHQL + 有效时间戳）或
    /// WinRing0（回退，GlobalSign 商业签名）任一可达即可。
    /// 返回 (可达, 不可达原因列表)；两个都不可达时返回完整诊断。
    static (bool ok, string? reason) ProbeRing0Backend()
    {
        var (pOk, pReason) = ProbePawnIo();
        if (pOk) return (true, null);
        var (wOk, wReason) = ProbeWinRing0();
        if (wOk) return (true, null);
        return (false, $"{pReason}；{wReason}");
    }

    // ============================================================================
    // 日志（stdout/stderr + 文件镜像）
    // ============================================================================

    /// 标准日志：stdout + 文件。
    static void Log(string line)
    {
        Console.WriteLine(line);
        FileLog(line);
    }

    /// 错误日志：stderr + 文件。
    static void LogError(string line)
    {
        Console.Error.WriteLine(line);
        FileLog(line);
    }

    /// 追加写入文件日志（失败不中断主流程）。
    static void FileLog(string line)
    {
        try
        {
            File.AppendAllText(LogFilePath, line + Environment.NewLine);
        }
        catch
        {
            // 文件日志失败不影响主流程
        }
    }

    // ============================================================================
    // 程序入口
    // ============================================================================

    static void Main(string[] args)
    {
        // ---- 第一步：提权（runas 自重启，UAC）----
        TryElevate(args);

        // ---- 第二步：命令行参数解析（极简 --key value；--elevated-child 已在提权中消费）----
        int port = 45980;
        int intervalMs = 1000;
        for (int i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--port" when i + 1 < args.Length
                    && int.TryParse(args[i + 1], out int p) && p is > 0 and < 65536:
                    port = p;
                    i++;
                    break;
                case "--interval" when i + 1 < args.Length
                    && int.TryParse(args[i + 1], out int iv) && iv >= 100:
                    intervalMs = iv;
                    i++;
                    break;
            }
        }

        Log($"[LhmSidecar] starting port={port} interval={intervalMs}ms elevated={_elevated}");

        // Ctrl+C 优雅退出：关闭 LHM 驱动句柄后退出（SECM 正常 kill 亦覆盖）。
        Console.CancelKeyPress += (_, e) =>
        {
            e.Cancel = true;
            ShutdownComputer();
            Environment.Exit(0);
        };

        // ---- 采集后台线程（异常全部捕获，不中断 HTTP 服务）----
        Thread collector = new(() => CollectionLoop(intervalMs))
        {
            IsBackground = true,
            Name = "lhm-collector"
        };
        collector.Start();

        // ---- HTTP 服务（主线程阻塞监听）----
        RunHttpServer(port);
    }

    // ============================================================================
    // LHM 采集
    // ============================================================================

    /// 创建 LHM Computer 组件：启用 CPU/GPU/主板/内存四类枚举（其余保持 false，控制遍历开销）。
    /// 注意：IsGpuEnabled 下 LHM 0.9.6 经 NVAPI/ADL 动态加载显卡驱动自带原生 DLL（nvapi64.dll /
    /// atiadlxx.dll 等，位于系统驱动目录，不需随 publish 分发）；枚举不到 GPU 时 gpu[] 为空数组。
    static Computer CreateComputer()
    {
        return new Computer
        {
            IsCpuEnabled = true,
            IsGpuEnabled = true,
            IsMemoryEnabled = true,
            IsMotherboardEnabled = true,
            IsControllerEnabled = false,
            IsStorageEnabled = false,
            IsBatteryEnabled = false,
            IsNetworkEnabled = false,
            IsPsuEnabled = false
        };
    }

    /// 后台采集主循环：提权/预检 → Open → 周期刷新传感器 → 更新快照。
    static void CollectionLoop(int intervalMs)
    {
        var visitor = new UpdateVisitor();
        var lastInitAttempt = DateTime.MinValue;

        while (true)
        {
            try
            {
                if (_computer is null)
                {
                    // 初始化（含驱动加载）失败后按退避间隔重试，避免热循环占用 CPU
                    if ((DateTime.UtcNow - lastInitAttempt).TotalMilliseconds < InitRetryMs)
                    {
                        Thread.Sleep(Math.Min(intervalMs, InitRetryMs));
                        continue;
                    }
                    lastInitAttempt = DateTime.UtcNow;

                    // 未提权（UAC 被拒）：直接报需要管理员权限，不尝试 Open
                    if (!_elevated)
                    {
                        SetError("LHM 需要管理员权限读取传感器（PawnIO 设备仅 SYSTEM/Administrators 可访问）：请以管理员身份运行 SECM，或允许本进程的 UAC 提权提示");
                        Thread.Sleep(Math.Min(intervalMs, InitRetryMs));
                        continue;
                    }

                    // PawnIO 设备预检（LHM 打不开设备时不抛异常，需显式探测）
                    // PawnIO 设备预检（LHM 打不开设备时不抛异常，需显式探测）
                    // Ring0 双后端预检（LHM 打不开设备时不抛异常，需显式探测）：
                    // WinRing0（主，WHQL 交叉签名 Secure Boot 可加载）或 PawnIO（回退）任一可达即可
                    var (probeOk, probeReason) = ProbeRing0Backend();
                    if (!probeOk)
                    {
                        SetError(probeReason ?? "Ring0 后端不可访问（未知原因）");
                        Thread.Sleep(Math.Min(intervalMs, InitRetryMs));
                        continue;
                    }

                    _computer = CreateComputer();
                    _computer.Open();   // LHM 自动选择可用 ring0 后端（WinRing0 优先/PawnIO 回退）；异常时捕获
                    _computer.Accept(visitor);
                    UpdateSnapshot();
                    Log("[LhmSidecar] LHM opened, CPU sensors available");
                    continue;
                }

                _computer.Accept(visitor);
                UpdateSnapshot();
            }
            catch (Exception ex)
            {
                // 驱动加载失败/传感器读取失败：记录可辨识错误（含异常类型与原始消息）。
                // 消息中保留 LHM 原始异常文本（可能含驱动名/错误码），
                // 供 SECM 引导卡判断是否属于杀软拦截场景。
                SetError($"LHM 采集失败: {ex.GetType().Name}: {ex.Message}");
                // 采集异常时释放 Computer（驱动句柄可能失效），下一轮退避后重试 Open
                try
                {
                    _computer?.Close();
                }
                catch
                {
                    // Close 失败不致命，忽略（驱动句柄失效由系统回收）
                }
                _computer = null;
            }

            Thread.Sleep(intervalMs);
        }
    }

    /// 从当前 Computer 读取全部硬件（CPU/GPU/主板/内存）传感器并刷新快照
    /// （过滤无效值：<=0 / NaN / Inf；沿用既有过滤与主温度/功耗匹配模式）。
    static void UpdateSnapshot()
    {
        if (_computer is null)
        {
            return;
        }

        var cpu = new CpuSensorData();
        var gpus = new List<GpuSensorData>();
        MotherboardData? motherboard = null;
        MemoryData? memory = null;
        // LHM 0.9.6 会枚举 Virtual/Total 两个 Memory 硬件节点，先收集再合并（优先物理内存 Total 节点）
        var memoryNodes = new List<IHardware>();

        foreach (IHardware hardware in _computer.Hardware)
        {
            switch (hardware.HardwareType)
            {
                case HardwareType.Cpu:
                    ReadCpuSensors(hardware, cpu);
                    break;
                // LHM 0.9.6 无统一 Gpu 枚举，GPU 按厂商分三类（Nvidia/Amd/Intel），统一归入 GPU 分流
                case HardwareType.GpuNvidia:
                case HardwareType.GpuAmd:
                case HardwareType.GpuIntel:
                    gpus.Add(ReadGpuSensors(hardware));
                    break;
                case HardwareType.Motherboard:
                    // 主板节点存在即返回对象（无传感器时空数组）；节点不存在时保持 null
                    motherboard = ReadMotherboardSensors(hardware);
                    break;
                case HardwareType.Memory:
                    memoryNodes.Add(hardware);
                    break;
            }
        }

        if (memoryNodes.Count > 0)
        {
            // 物理内存取名称含 "Total" 的节点（Virtual Memory 节点含 pagefile，不适用于物理内存契约）
            IHardware? memNode = memoryNodes.FirstOrDefault(n =>
                n.Name.Contains("Total", StringComparison.OrdinalIgnoreCase)) ?? memoryNodes[0];
            memory = ReadMemorySensors(memNode);
        }

        // 仅 package_temp_c 为有效正值（>0 且有限）时 available:true，防 LHM 静默 0 值污染
        bool available = cpu.PackageTempC.HasValue
            && double.IsFinite(cpu.PackageTempC.Value)
            && cpu.PackageTempC.Value > 0.0;

        // 首次成功采集时打印全部传感器清单（CPU/GPU/主板/内存一行汇总，沿用 StringBuilder 模式），
        // 供 SECM 日志与排障（_sensorListLogged 语义保留：仅首次输出一次）
        if (available && !_sensorListLogged)
        {
            _sensorListLogged = true;
            var sb = new StringBuilder("[LhmSidecar] sensors: ");
            foreach (IHardware hardware in _computer.Hardware)
            {
                AppendHardwareSensors(hardware, sb);
            }
            Log(sb.ToString());
        }

        lock (SnapshotLock)
        {
            _snapshot = new SensorSnapshot
            {
                Available = available,
                Error = available
                    ? null
                    : "传感器读取返回无效值（LHM 驱动可能被安全软件拦截或设备不可访问）",
                Cpu = cpu,
                Gpu = gpus,
                Motherboard = motherboard,
                Memory = memory
            };
        }
    }

    /// 递归追加单个硬件节点及其子硬件的传感器清单（名称+类型+值），一行打印。
    static void AppendHardwareSensors(IHardware hardware, StringBuilder sb)
    {
        sb.Append(hardware.HardwareType).Append('[').Append(hardware.Name).Append("]: ");
        foreach (ISensor s in hardware.Sensors)
        {
            sb.Append(s.Name).Append('(').Append(s.SensorType).Append('=')
              .Append(s.Value.HasValue ? s.Value.Value.ToString("0.##") : "n/a").Append(") ");
        }
        sb.Append("; ");
        foreach (IHardware sub in hardware.SubHardware)
        {
            AppendHardwareSensors(sub, sb);
        }
    }

    /// 从单个 GPU 硬件节点提取温度/时钟/功耗/负载/显存/风扇值。
    static GpuSensorData ReadGpuSensors(IHardware gpu)
    {
        var data = new GpuSensorData { Name = gpu.Name };
        // 无 "GPU Core" 命名时取数值最大者兜底（记录候选，避免把 VRAM 温度误当核心）
        var fallbackTemps = new List<double>();
        foreach (ISensor sensor in gpu.Sensors)
        {
            double? value = sensor.Value;
            if (!value.HasValue || !double.IsFinite(value.Value) || value.Value <= 0.0)
            {
                continue;
            }
            double v = value.Value;
            switch (sensor.SensorType)
            {
                case SensorType.Temperature:
                    // 主温度取名称含 "GPU Core" 的传感器（NVIDIA/AMD 命名），防 VRAM 温度误当核心
                    if (sensor.Name.Contains("GPU Core", StringComparison.OrdinalIgnoreCase))
                    {
                        if (data.TemperatureC is null)
                        {
                            data.TemperatureC = v;
                        }
                    }
                    else
                    {
                        fallbackTemps.Add(v);
                    }
                    break;
                case SensorType.Power:
                    // 功耗取名称含 "GPU Package"/"GPU" 的传感器（NVIDIA "GPU Package"）
                    if ((sensor.Name.Contains("GPU Package", StringComparison.OrdinalIgnoreCase)
                            || sensor.Name.Contains("GPU", StringComparison.OrdinalIgnoreCase))
                        && data.PowerW is null)
                    {
                        data.PowerW = v;
                    }
                    break;
                case SensorType.Fan:
                    if (data.FanRpm is null)
                    {
                        data.FanRpm = v;
                    }
                    break;
                case SensorType.Load:
                    if (sensor.Name.Contains("GPU Core", StringComparison.OrdinalIgnoreCase)
                        && data.LoadPercent is null)
                    {
                        data.LoadPercent = v;
                    }
                    break;
                case SensorType.Clock:
                    if (sensor.Name.Contains("GPU Core", StringComparison.OrdinalIgnoreCase)
                        && data.CoreClockMhz is null)
                    {
                        data.CoreClockMhz = v;
                    }
                    break;
                case SensorType.SmallData:
                    // LHM GPU 显存传感器单位为 MB，换算为字节（1 MB = 1024*1024 B）。
                    // 精确匹配 "GPU Memory Used"/"GPU Memory Total"（排除 "D3D Dedicated Memory Used" 等同类传感器）
                    if (sensor.Name.StartsWith("GPU Memory Used", StringComparison.OrdinalIgnoreCase)
                        && data.MemoryUsedBytes is null)
                    {
                        data.MemoryUsedBytes = (long)Math.Round(v * 1024.0 * 1024.0);
                    }
                    else if (sensor.Name.StartsWith("GPU Memory Total", StringComparison.OrdinalIgnoreCase)
                        && data.MemoryTotalBytes is null)
                    {
                        data.MemoryTotalBytes = (long)Math.Round(v * 1024.0 * 1024.0);
                    }
                    break;
            }
        }
        // 该 GPU 无 "GPU Core" 命名温度时，取数值最大者（单温度传感器的场景兜底）
        if (data.TemperatureC is null && fallbackTemps.Count > 0)
        {
            data.TemperatureC = fallbackTemps.Max();
        }
        return data;
    }

    /// 读取主板节点 + SuperIO 子硬件的温度/风扇/电压传感器（name 保留原始名）。
    static MotherboardData ReadMotherboardSensors(IHardware motherboard)
    {
        var data = new MotherboardData { Name = motherboard.Name };
        CollectSubHardwareSensors(motherboard, data.Sensors);
        return data;
    }

    /// 递归收集主板节点下（含 SuperIO 子硬件）的温度/风扇/电压传感器。
    static void CollectSubHardwareSensors(IHardware hardware, List<MotherboardSensorData> sensors)
    {
        foreach (ISensor sensor in hardware.Sensors)
        {
            double? value = sensor.Value;
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
            if (type is null)
            {
                continue;
            }
            sensors.Add(new MotherboardSensorData { Name = sensor.Name, Type = type, Value = value.Value });
        }
        foreach (IHardware sub in hardware.SubHardware)
        {
            CollectSubHardwareSensors(sub, sensors);
        }
    }

    /// 读取内存节点：容量/已用由 LHM 传感器推算（不伪造），SPD 型号仅首次读取缓存（静态数据）。
    /// LHM 内存不可用（无有效传感器）时返回 null。
    static MemoryData? ReadMemorySensors(IHardware memory)
    {
        long? used = null;
        long? available = null;
        foreach (ISensor sensor in memory.Sensors)
        {
            double? value = sensor.Value;
            if (!value.HasValue || !double.IsFinite(value.Value) || value.Value <= 0.0)
            {
                continue;
            }
            if (sensor.SensorType != SensorType.SmallData && sensor.SensorType != SensorType.Data)
            {
                continue;
            }
            // LHM 0.9.6 内存传感器为 Data 类型（值单位 GB），换算为字节（1 GB = 1024^3 B）
            double bytes = value.Value * 1024.0 * 1024.0 * 1024.0;
            if (sensor.Name.Contains("Memory Used", StringComparison.OrdinalIgnoreCase))
            {
                used = (long)Math.Round(bytes);
            }
            else if (sensor.Name.Contains("Memory Available", StringComparison.OrdinalIgnoreCase))
            {
                available = (long)Math.Round(bytes);
            }
        }

        // 无任何有效内存传感器 → 视为不可用（返回 null）
        if (used is null && available is null)
        {
            return null;
        }
        // 仅有 "Memory Available" 时总量推算不出 → 数据不足视为不可用（不伪造）
        if (used is null)
        {
            return null;
        }

        // SPD 型号是静态数据，仅在首次采集读取一次并缓存（不进入 1s 轮询热路径）
        if (_memorySpdName is null)
        {
            // LHM 0.9.6 Memory 节点名称恒定 "Memory"（SPD 型号深读 SMBIOS 收益低，保留原始名，
            // 真实值以 LHM 输出为准；未来 LHM 升级可在此填充 "DDR4-2400 2x8GB" 式汇总）
            _memorySpdName = memory.Name;
        }

        var data = new MemoryData { Name = _memorySpdName };
        data.UsedBytes = used;
        if (available.HasValue)
        {
            // total = used + available（两个传感器互为补充，静态推算不虚报）
            data.TotalBytes = used.Value + available.Value;
        }
        return data;
    }

    /// 从单个 CPU 硬件节点提取温度/功耗/风扇/电压值。
    static void ReadCpuSensors(IHardware cpu, CpuSensorData data)
    {
        var coreTemps = new List<double>();
        foreach (ISensor sensor in cpu.Sensors)
        {
            double? value = sensor.Value;
            if (!value.HasValue || !double.IsFinite(value.Value) || value.Value <= 0.0)
            {
                // 跳过无值/非有限/非正值传感器（LHM 静默降级时返回 0 值，需过滤）
                continue;
            }
            switch (sensor.SensorType)
            {
                case SensorType.Temperature:
                    // 主温度：Intel "CPU Package" / AMD "Core (Tctl/Tdie)"；
                    // 其余温度（AMD "CCD1 (Tdie)"、Intel "Core #1" 等）归入核心/CCD 温度数组
                    if (IsMainPackageTemp(sensor.Name))
                    {
                        if (data.PackageTempC is null)
                        {
                            data.PackageTempC = value.Value;
                        }
                    }
                    else
                    {
                        coreTemps.Add(value.Value);
                    }
                    break;
                case SensorType.Power:
                    // AMD 命名为 "Package"，Intel 为 "CPU Package"，统一按 Package 匹配
                    if (sensor.Name.Contains("Package", StringComparison.OrdinalIgnoreCase)
                        && data.PowerW is null)
                    {
                        data.PowerW = value.Value;
                    }
                    break;
                case SensorType.Fan:
                    if (data.FanRpm is null)
                    {
                        data.FanRpm = value.Value;
                    }
                    break;
                case SensorType.Voltage:
                    if (data.VoltageV is null)
                    {
                        data.VoltageV = value.Value;
                    }
                    break;
            }
        }
        data.CoreTempsC = coreTemps.ToArray();
    }

    /// 判断是否为 CPU Package 主温度传感器（Intel "CPU Package" / AMD "Core (Tctl/Tdie)"）。
    /// 注意 AMD 的 "CCD1 (Tdie)" 含 "Tdie" 但属 CCD 温度，应归核心数组，故仅匹配 Tctl。
    static bool IsMainPackageTemp(string name)
    {
        if (name.Contains("Tctl", StringComparison.OrdinalIgnoreCase))
        {
            return true; // AMD "Core (Tctl/Tdie)"
        }
        if (name.StartsWith("CPU Package", StringComparison.OrdinalIgnoreCase))
        {
            return true; // Intel "CPU Package"
        }
        if (string.Equals(name, "Package", StringComparison.OrdinalIgnoreCase))
        {
            return true; // 兜底：AMD 功耗传感器同名字段，温度场景兜底
        }
        return false;
    }

    /// 记录不可用快照 + 去重日志（错误消息变化时才输出，避免刷屏）。
    static void SetError(string message)
    {
        lock (SnapshotLock)
        {
            _snapshot = new SensorSnapshot { Available = false, Error = message };
        }
        if (_lastError != message)
        {
            _lastError = message;
            LogError($"[LhmSidecar] {message}");
        }
    }

    /// 关闭 LHM Computer（释放驱动句柄），幂等。
    static void ShutdownComputer()
    {
        try
        {
            _computer?.Close();
        }
        catch
        {
            // 关闭失败不致命
        }
        _computer = null;
    }

    // ============================================================================
    // HTTP 服务（极简 HTTP/1.1，无 ASP.NET Core 依赖）
    // ============================================================================

    /// 绑定 127.0.0.1 回环并循环处理请求（每连接独立线程池任务）。
    static void RunHttpServer(int port)
    {
        var listener = new TcpListener(IPAddress.Loopback, port);
        try
        {
            listener.Start();
        }
        catch (Exception ex)
        {
            LogError($"[LhmSidecar] 绑定 127.0.0.1:{port} 失败: {ex.GetType().Name}: {ex.Message}（端口被占用？可用 --port 指定其他端口）");
            Environment.Exit(2);
        }
        Log($"[LhmSidecar] HTTP listening on http://127.0.0.1:{port}");

        while (true)
        {
            try
            {
                TcpClient client = listener.AcceptTcpClient();
                ThreadPool.QueueUserWorkItem(HandleClient, client);
            }
            catch (Exception ex)
            {
                LogError($"[LhmSidecar] accept 失败: {ex.GetType().Name}: {ex.Message}");
                Thread.Sleep(200);
            }
        }
    }

    /// 单个连接处理：极简请求头解析 + 路由 + JSON 响应。
    static void HandleClient(object? state)
    {
        if (state is not TcpClient client)
        {
            return;
        }
        try
        {
            using (client)
            using (NetworkStream stream = client.GetStream())
            {
                // 读超时：防慢速连接（连接后不发数据）长期占用线程池线程（M7）
                client.ReceiveTimeout = 2000;
                stream.ReadTimeout = 2000;
                // 读取请求头直到空行（\r\n\r\n），上限 8KB 防异常大包
                byte[] headerBuf = new byte[8192];
                int headerLen = 0;
                while (headerLen < headerBuf.Length)
                {
                    int n = stream.Read(headerBuf, headerLen, headerBuf.Length - headerLen);
                    if (n <= 0)
                    {
                        return;
                    }
                    headerLen += n;
                    if (ContainsHeaderEnd(headerBuf, headerLen))
                    {
                        break;
                    }
                }

                string header = Encoding.ASCII.GetString(headerBuf, 0, headerLen);
                string requestLine = header.Split('\n')[0].TrimEnd('\r');

                // 请求行格式: "METHOD SP PATH SP HTTP/1.1"
                string[] parts = requestLine.Split(' ');
                string method = parts.Length > 0 ? parts[0] : string.Empty;
                string path = parts.Length > 1 ? parts[1] : "/";
                int query = path.IndexOf('?');
                if (query >= 0)
                {
                    path = path[..query];
                }

                int status;
                string body;
                if (!string.Equals(method, "GET", StringComparison.OrdinalIgnoreCase))
                {
                    status = 405;
                    body = "{\"error\":\"method not allowed: only GET supported\"}";
                }
                else
                {
                    switch (path)
                    {
                        case "/health":
                            status = 200;
                            body = "{\"status\":\"ok\"}";
                            break;
                        case "/api/lhm/sensors":
                        case "/api/lhm/sensors/":
                            status = 200;
                            body = SerializeSnapshot();
                            break;
                        default:
                            status = 404;
                            body = "{\"error\":\"not found\"}";
                            break;
                    }
                }

                byte[] bodyBytes = Encoding.UTF8.GetBytes(body);
                string head = "HTTP/1.1 " + status + " " + StatusText(status) + "\r\n"
                    + "Content-Type: application/json; charset=utf-8\r\n"
                    + "Content-Length: " + bodyBytes.Length + "\r\n"
                    + "Connection: close\r\n\r\n";
                byte[] headBytes = Encoding.ASCII.GetBytes(head);
                stream.Write(headBytes, 0, headBytes.Length);
                stream.Write(bodyBytes, 0, bodyBytes.Length);
                stream.Flush();
            }
        }
        catch (Exception ex)
        {
            // 单连接异常不致命（客户端可能中途断开）
            LogError($"[LhmSidecar] 处理请求失败: {ex.GetType().Name}: {ex.Message}");
        }
    }

    /// 在缓冲区中查找 HTTP 请求头结束标记 \r\n\r\n。
    static bool ContainsHeaderEnd(byte[] buf, int len)
    {
        for (int i = 0; i + 3 < len; i++)
        {
            if (buf[i] == '\r' && buf[i + 1] == '\n' && buf[i + 2] == '\r' && buf[i + 3] == '\n')
            {
                return true;
            }
        }
        return false;
    }

    static string StatusText(int code) => code switch
    {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "OK"
    };

    /// 序列化最新快照（JSON 序列化失败时返回降级 JSON，保证 HTTP 层不崩）。
    static string SerializeSnapshot()
    {
        SensorSnapshot snap;
        lock (SnapshotLock)
        {
            snap = _snapshot;
        }
        try
        {
            return JsonSerializer.Serialize(snap, JsonOpts);
        }
        catch (Exception ex)
        {
            return $"{{\"available\":false,\"error\":\"LHM 快照序列化失败: {ex.GetType().Name}: {ex.Message}\",\"contract_version\":2,\"gpu\":[],\"motherboard\":null,\"memory\":null,\"cpu\":{{\"package_temp_c\":null,\"core_temps_c\":[],\"power_w\":null,\"fan_rpm\":null,\"voltage_v\":null}}}}";
        }
    }
}
