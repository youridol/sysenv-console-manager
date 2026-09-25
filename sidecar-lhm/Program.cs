// LhmSidecar —— SECM 硬件数据源 sidecar（进程隔离，MPL-2.0 隔离边界）
//
// 架构：
//   SECM（Rust, MIT）通过 HTTP 消费本进程输出的 **契约 v3** JSON；LibreHardwareMonitorLib
//   （MPL-2.0）仅在本进程内使用，两进程不链接、不共享内存代码，规避许可对 MIT 主体的传染。
//   内嵌 ring0 驱动 PawnIO（LGPL-2.1，namazso/PawnIO.Modules），设备 \\.\PawnIO。
//
// 数据来源：完整移植 Diorser/LiteMonitor（MIT）的数据源层 ——
//   PerformanceCounterManager（PDH，上游唯一纯用户态源）/ NetworkManager / DiskManager /
//   BatteryService / SensorMatcher / HardwareRules / SensorMap / FanMapper /
//   ComponentProcessor / MetricUtils。逐条规则在 Hardware/*.cs 中标注上游 file:line。
//
// 权限模型（契约 v3 §1，LiteMonitor 没有、SECM 必须补上的一层）：
//   - 本进程**始终以未提权启动**，立即提供 tier-1（CPU 负载/频率、内存、每盘 IO/活动/占用、
//     网络、电池、GPU 温度/功耗/显存）。启动**不弹 UAC**。
//   - tier-2（CPU 温度/功耗/电压、主板 SuperIO、风扇/水泵、磁盘温度）需要 PawnIO 设备
//     （DACL 仅 SYSTEM/Administrators）。用户通过 POST /api/v1/elevate 触发**一次** UAC；
//     被接受后启动提权伴生进程（--role=elevated，监听 port+1）。
//   - 本进程代理 tier-2 读取（GET /api/v1/tier2）并与 tier-1 合并为**同一份**快照。
//   - UAC 被拒绝是**非致命且被记住**的状态：绝不每次轮询重复弹窗。
//
// 端点（契约 §2）：
//   GET  /health               → 200 {"status":"ok"}
//   GET  /api/v1/snapshot      → 200 契约 v3 快照
//   GET  /api/lhm/sensors      → 200 同一份快照（v2 路径别名）
//   POST /api/v1/elevate       → 200 {"elevated":bool,"reason":string}
//   GET  /api/v1/privilege     → 200 {"elevated":bool,"tier2_available":bool,"tier2_reason":string|null}
//   GET  /api/v1/tier2         → 仅提权伴生进程提供（内部通道，非对外契约）
//   非 GET（且非 elevate 的 POST）→ 405；未知路径 → 404
//
// 启动参数：--port <port>（默认 45980）、--interval <ms>（默认 1000，最小 100）、
//           --role=elevated（提权伴生进程模式，监听 port+1）
//
// 健壮性（自既有 sidecar 沿用，契约 §5.7）：PawnIO/WinRing0 设备预检、初始化退避重试、
// 采集异常隔离、stdout + 文件双路日志、8KB 有界请求头读取 + 2s 读写超时、
// UAC 取消处理（Win32 错误 1223）。

using System.ComponentModel;
using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using System.Runtime.Versioning;
using System.Security.Principal;
using System.Text;
using System.Text.Json;
using LhmSidecar.Elevation;
using LhmSidecar.Hardware;

// sidecar 仅面向 Windows（win-x64 发布），消除 CA1416 平台兼容性警告
[assembly: SupportedOSPlatform("windows")]
// 测试工程需要验证"UAC 被拒绝后非致命且被记住"的行为（无法在 CI 里真实弹出/取消 UAC，
// 故通过 internal 测试接缝注入）。
[assembly: System.Runtime.CompilerServices.InternalsVisibleTo("LhmSidecar.Tests")]

namespace LhmSidecar;

static class Program
{
    /// <summary>JSON 序列化选项：契约 §3 要求字段名一律 snake_case。</summary>
    internal static readonly JsonSerializerOptions JsonOpts = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower
    };

    /// <summary>快照读写锁（采集线程写、HTTP 线程读）。</summary>
    private static readonly object SnapshotLock = new();

    /// <summary>最新快照（HTTP 响应直接序列化此对象）。启动即给出 tier-1 形态的可用快照。</summary>
    private static Snapshot _snapshot = NewEmptySnapshot("正在进行首次采集（tier-1 性能计数器预热中）");

    /// <summary>
    /// tier-2 负载快照（单写者模型，见 ADR-0001 D3）。
    ///
    /// 背景：`GET /api/v1/tier2` 原先由 HTTP 线程直接调用 `BuildLocalTier2Payload()`，
    /// 而采集线程也在每轮调用它。两条线程共享 `_lhm._computer`、`DiskManager._lastValidMap`、
    /// `_tier2LastError` 等未加锁可变状态：`ReadTier2()` 全程无锁，采集线程的 `Close()`
    /// 却会在持锁时把 `_computer` 置 null；`DiskManager` 的 `Dictionary<string,float>`
    /// 会在同一调用内既读又写。并发下可能抛异常被吞（表现为 tier-2 间歇性为 null）。
    ///
    /// 现改为：采集线程在本轮结束后把负载作为**不可变值**发布到此快照，HTTP 线程只读取
    /// 该引用后序列化，绝不进入采集路径。代价是 HTTP 最多滞后一轮采集，而采集周期本身
    /// 就是该数据的新鲜度上限。
    /// </summary>
    private static Tier2Payload? _tier2Snapshot;

    /// <summary>发布 tier-2 快照（仅采集线程调用）。</summary>
    private static void PublishTier2Snapshot(Tier2Payload? payload)
    {
        lock (SnapshotLock)
        {
            _tier2Snapshot = payload;
        }
    }

    /// <summary>读取 tier-2 快照（仅 HTTP 线程调用；未采集过时返回 null）。</summary>
    internal static Tier2Payload? ReadTier2Snapshot()
    {
        lock (SnapshotLock)
        {
            return _tier2Snapshot;
        }
    }

    /// <summary>文件日志路径（提权子进程脱离 SECM 重定向管道后仍可写）。</summary>
    private static readonly string LogFilePath = Path.Combine(Path.GetTempPath(), "secm-lhm-sidecar.log");

    /// <summary>上次输出的错误消息（去重，避免失败时刷屏）。</summary>
    private static string? _lastError;

    /// <summary>初始化失败后的重试退避间隔（驱动/设备可能随后可用）。</summary>
    private const int InitRetryMs = 5000;

    /// <summary>当前进程是否以管理员令牌运行。</summary>
    private static bool _elevated;

    /// <summary>
    /// UAC 是否已被用户拒绝（或提权失败）——**记住**该状态，绝不每次轮询重复弹窗。
    /// 契约 §1："被拒绝是非致命且被记住的状态（不每次轮询重复弹窗）"。
    /// </summary>
    private static bool _elevationRefused;

    private static string? _elevationRefusedReason;

    /// <summary>
    /// 提权尝试进行中（互斥，防止并发 POST 触发多次 UAC）。
    /// </summary>
    private static readonly SemaphoreSlim ElevateGate = new(1, 1);

    /// <summary>测试接缝：允许单测覆盖"本进程是否已是管理员"的判断（CI 中无法真实触发/取消 UAC）。
    /// 生产路径始终为 null → 走 WindowsIdentity 真实判断。
    /// </summary>
    internal static Func<bool>? IsElevatedForTesting;

    /// <summary>伴生进程 PID（0 = 未启动）。</summary>
    private static int _companionPid;

    /// <summary>伴生进程 PID（测试可见）。</summary>
    internal static int CompanionPidForTesting => _companionPid;

    /// <summary>UAC 是否已被拒绝（测试可见）。</summary>
    internal static bool ElevationRefusedForTesting => _elevationRefused;

    /// <summary>
    /// 测试专用：把提权相关静态状态复位（生产路径不会调用）。
    /// </summary>
    internal static void ResetElevationStateForTesting()
    {
        _elevatedRole = false;
        _elevationRefused = false;
        _elevationRefusedReason = null;
        _companionPid = 0;
        _elevationAttempted = false;
        _autoAuthorizeSkipped = false;
        _tier2LastSuccess = DateTime.MinValue;
        _tier2LastError = null;
        _tier2Failures = 0;
    }

    /// <summary>tier-2 最近一次成功读取时间（用于判活）。</summary>
    private static DateTime _tier2LastSuccess = DateTime.MinValue;

    /// <summary>tier-2 最近一次失败原因。</summary>
    private static string? _tier2LastError;

    /// <summary>
    /// 是否至少请求过一次提权。用于区分两种"没有 tier-2"：
    ///   - 从未请求 → 原因应是"需要管理员权限 + 如何提权"的引导文案；
    ///   - 请求过但失败/伴生进程挂了 → 原因是具体的失败诊断。
    /// </summary>
    private static bool _elevationAttempted;

    /// <summary>tier-2 连续失败次数（超过阈值则丢弃伴生进程句柄，允许重新拉起）。</summary>
    private static int _tier2Failures;

    /// <summary>端口/间隔/角色等运行参数。</summary>
    private static int _port = 45980;
    private static int _intervalMs = 1000;
    private static bool _elevatedRole;

    /// <summary>
    /// 主进程（对外服务 /api/v1/snapshot 的那个进程）的端口。
    /// 持久授权任务的命令行必须指向 **主进程端口 + 1**，否则任务拉起的伴生进程会与主进程抢端口：
    ///   - 提权伴生进程：本进程 _port 已经是主进程 port+1 → 主进程端口 = _port - 1；
    ///   - 主进程（含以管理员启动的主进程）：本进程 _port 就是主进程端口 → 主进程端口 = _port。
    /// 该值在参数解析时确定，之后不再变化（任何角色都不改写它）。
    /// </summary>
    private static int _primaryPort = 45980;

    /// <summary>主进程端口（测试可见）。</summary>
    internal static int PrimaryPortForTesting => _primaryPort;

    /// <summary>
    /// 由当前进程的 --port 与角色推导**主进程端口**（持久授权任务命令行要用它 +1）。
    /// 纯函数，便于单测覆盖——把主进程端口写成伴生进程自己的端口会导致
    /// 任务拉起的伴生进程与主进程抢同一个端口（实测踩过）。
    /// </summary>
    internal static int DerivePrimaryPort(int port, bool elevatedRole)
        => elevatedRole && port > 1 ? port - 1 : port;

    /// <summary>
    /// 本进程是否应直接提供 tier-2（并打开 hardware tree 的 tier-2 组）。
    /// 两种情形：① 提权伴生进程（--role=elevated）；② 本进程本身以管理员运行
    /// （SECM 以管理员启动时无需再拉一个伴生进程）。
    /// 注意：RequestElevation() 发现自身已是管理员时会把 _elevatedRole 置 true 作为
    /// "运行中升级"的标记，故两者取或。
    /// </summary>
    private static bool LocalTier2Enabled => _elevatedRole;

    // ============================================================================
    // 提权（按需 runas，不再启动即弹窗）
    // ============================================================================

    /// <summary>检测当前进程是否以管理员令牌运行。</summary>
    internal static bool IsElevated()
    {
        // 测试接缝（生产路径为 null）
        if (IsElevatedForTesting != null) return IsElevatedForTesting();

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

    /// <summary>
    /// 提权启动器（可替换以便单测注入"用户取消 UAC"）。
    /// 生产实现用 runas 拉起提权伴生进程；测试实现抛 Win32Exception(1223)。
    /// </summary>
    internal static Func<string, string, Process?>? ElevationLauncherForTesting;

    /// <summary>
    /// 启动提权伴生进程的可替换实现。
    /// 返回 null 表示已启动但拿不到进程对象；抛异常表示失败。
    /// </summary>
    private static Process? LaunchElevated(string exe, string argLine)
    {
        if (ElevationLauncherForTesting != null) return ElevationLauncherForTesting(exe, argLine);

        return Process.Start(new ProcessStartInfo
        {
            FileName = exe,
            Arguments = argLine,
            Verb = "runas",          // UAC 提权
            UseShellExecute = true
        });
    }

    /// <summary>
    /// 只走「零 UAC」的自动提权路径：管理员直通 / 已注册计划任务 `/Run`。
    ///
    /// **绝不回退 runas** —— 这是与 <see cref="RequestElevation"/> 的唯一差别。
    /// 调用方（前端进入硬件信息页时）先调本入口；返回 false 才由调用方另行发起
    /// 用户 UAC 申请，从而实现「自动提权不等待授权，自动不可行才申请用户授权」。
    /// </summary>
    internal static (bool ok, string reason) TryAutoElevate()
    {
        _elevationAttempted = true;

        // 情形 1：本进程已是管理员（含以管理员启动的主进程）
        if (IsElevated())
        {
            _elevatedRole = true;
            _elevationRefused = false;
            _elevationRefusedReason = null;
            return (true, "本进程已具备管理员权限，tier-2 由本进程直接提供");
        }

        // 情形 2：提权伴生进程已在本会话启动
        if (_companionPid != 0)
        {
            bool live = (DateTime.UtcNow - _tier2LastSuccess).TotalSeconds < 10;
            return live
                ? (true, "已连接提权伴生进程")
                : (false, _tier2LastError ?? "提权伴生进程已启动但尚未返回 tier-2 数据");
        }

        // 情形 3：从未授权 → 自动路径不可行，如实上报（**绝不 runas**）
        if (!Tier2Task.Exists())
        {
            Log($"[LhmSidecar] 计划任务 {Tier2Task.TaskName} 未注册：自动提权不可行（绝不自动 runas）");
            return (false, "未注册持久授权：自动提权不可行，需用户确认一次 UAC");
        }

        // 情形 4：已注册持久授权 → /Run（无 UAC）+ 有界等待 tier-2
        if (!Tier2Task.RunExisting(out string taskReason))
        {
            LogError($"[LhmSidecar] 自动提权跳过：{taskReason}");
            return (false, taskReason);
        }

        var sw = Stopwatch.StartNew();
        while (sw.ElapsedMilliseconds < ElevationWaitMs)
        {
            if (ProbeTier2Available())
            {
                Log($"[LhmSidecar] {taskReason}；tier-2 已在 {sw.ElapsedMilliseconds}ms 内就绪（无 UAC）");
                return (true, $"{taskReason}；tier-2 已在 {sw.ElapsedMilliseconds}ms 内就绪");
            }
            Thread.Sleep(ProbeIntervalMs);
        }

        LogError($"[LhmSidecar] 已触发计划任务但 {ElevationWaitMs}ms 内未取到 tier-2 数据");
        return (false, $"{ElevationWaitMs}ms 内未取到 tier-2 数据");
    }

    /// <summary>
    /// 按需提权。流程（CHANGE-CONTRACT-v2.md §D.2）：
    ///   1. 已是管理员 → 本进程直接提供 tier-2（零点击）；
    ///   2. 已被用户拒绝过 → 直接返回失败，**不再重复弹窗**；
    ///   3. **先尝试已注册的计划任务**（`schtasks /Run`，无 UAC）并在有界时间内等 tier-2 生效；
    ///   4. 仍无 tier-2 → 回退到既有 runas 伴生路径（会弹 UAC）。
    /// 返回 (是否已提权生效, 原因)；原因文案区分来源（already-admin / scheduled-task / companion）。
    /// </summary>
    internal static (bool ok, string reason) RequestElevation()
    {
        _elevationAttempted = true;

        // 已经是管理员（例如 SECM 本身以管理员运行）：无需伴生进程，直接开 tier-2
        if (IsElevated())
        {
            _elevatedRole = true;   // 本进程即可访问 ring0（采集循环据此打开 tier-2 组）
            _elevationRefused = false;
            _elevationRefusedReason = null;
            return (true, "本进程已具备管理员权限，tier-2 由本进程直接提供");
        }

        // 已被用户拒绝过：不再重复弹窗（契约 §1 硬性要求）
        if (_elevationRefused)
        {
            return (false, _elevationRefusedReason ?? "UAC 提权此前已被用户取消");
        }

        if (_companionPid != 0)
        {
            // 伴生进程已启动：等待其 tier-2 就绪由采集线程负责，这里如实反馈当前状态
            bool live = (DateTime.UtcNow - _tier2LastSuccess).TotalSeconds < 10;
            return live
                ? (true, "已连接提权伴生进程")
                : (false, _tier2LastError ?? "提权伴生进程已启动但尚未返回 tier-2 数据");
        }

        // ---- 3. 优先复用已注册的持久授权：schtasks /Run（无 UAC）----
        // 只有任务确实已存在时才会触发；不存在则由 RunExisting 内部拒绝（绝不 runas）。
        // §D.2：任务不存在或未在 ≤8s 内产出 tier-2 → 回退 runas。
        if (Tier2Task.Exists())
        {
            bool ran = Tier2Task.RunExisting(out string taskReason);
            if (ran)
            {
                var sw = Stopwatch.StartNew();
                while (sw.ElapsedMilliseconds < ElevationWaitMs)
                {
                    if (ProbeTier2Available())
                    {
                        Log($"[LhmSidecar] {taskReason}；tier-2 已在 {sw.ElapsedMilliseconds}ms 内就绪");
                        return (true, $"{taskReason}；tier-2 已在 {sw.ElapsedMilliseconds}ms 内就绪");
                    }
                    Thread.Sleep(ProbeIntervalMs);
                }
                LogError($"[LhmSidecar] 已触发计划任务 {Tier2Task.TaskName}，但 {ElevationWaitMs}ms 内未取到 tier-2 数据；回退 runas 伴生路径");
            }
            else
            {
                LogError($"[LhmSidecar] {taskReason}；回退到 runas 伴生路径");
            }
        }

        // ---- 4. 回退：runas 伴生路径（会弹 UAC）----
        return LaunchViaRunAs();
    }

    /// <summary>
    /// 提权有界等待上限（CHANGE-CONTRACT-v2.md §D.2：≤8s）。
    /// 非 const：单测可置 0 以避免真实等待。
    /// </summary>
    internal static int ElevationWaitMs = 8000;

    /// <summary>等待 tier-2 就绪的轮询间隔。</summary>
    internal const int ProbeIntervalMs = 250;

    /// <summary>
    /// 测试接缝：等待循环内的 tier-2 可用性探测。
    /// 默认（null）走真实的伴生进程 HTTP 代理；单测注入以避免真实网络/进程。
    /// </summary>
    internal static Func<bool>? Tier2ProbeForTesting;

    /// <summary>tier-2 是否已就绪（真实探测 = 代理 port+1 的伴生进程）。</summary>
    private static bool ProbeTier2Available()
        => Tier2ProbeForTesting?.Invoke() ?? (FetchCompanionTier2()?.Available == true);

    /// <summary>
    /// 回退路径：runas 拉起提权伴生进程（会弹 UAC）。被 RequestElevation 与单测共用。
    /// </summary>
    private static (bool ok, string reason) LaunchViaRunAs()
    {
        string exe = Environment.ProcessPath ?? string.Empty;
        if (string.IsNullOrEmpty(exe))
        {
            return (false, "无法定位自身可执行文件，无法启动提权伴生进程");
        }

        string argLine = Tier2Task.BuildCompanionArguments(_port, _intervalMs);

        try
        {
            var proc = LaunchElevated(exe, argLine);
            _companionPid = proc?.Id ?? 0;
            _tier2Failures = 0;
            _tier2LastError = null;
            Log($"[LhmSidecar] 提权伴生进程已启动 pid={_companionPid} port={_port + 1}");
            return (true, "提权伴生进程已启动（UAC）");
        }
        catch (Win32Exception ex)
        {
            // 用户取消 UAC（错误码 1223）或其他失败：非致命，**记住**该状态
            _elevationRefused = true;
            _elevationRefusedReason = ex.NativeErrorCode == 1223
                ? "UAC 提权被用户取消（Win32 错误码 1223）；tier-1 指标不受影响，如需 tier-2 请重启 SECM 后再次触发"
                : $"UAC 提权失败（Win32 错误码 0x{ex.NativeErrorCode:X8}）：{ex.Message}";
            LogError($"[LhmSidecar] {_elevationRefusedReason}");
            return (false, _elevationRefusedReason);
        }
        catch (Exception ex)
        {
            _elevationRefused = true;
            _elevationRefusedReason = $"提权启动失败: {ex.GetType().Name}: {ex.Message}";
            LogError($"[LhmSidecar] {_elevationRefusedReason}");
            return (false, _elevationRefusedReason);
        }
    }

    /// <summary>
    /// 撤销持久授权：删除计划任务、复位提权状态。
    /// 非管理员进程无权删除任务（实测 schtasks /Delete → 拒绝访问），
    /// 此时请求提权伴生进程代为删除（`/api/v1/revoke`），并同时停用本次会话的 tier-2。
    /// </summary>
    internal static (bool revoked, string reason) RevokeElevation()
    {
        string exe = Environment.ProcessPath ?? string.Empty;

        // 情形 A：本进程就是提权进程（或 SECM 本身是管理员）→ 直接删
        if (IsElevated())
        {
            bool deleted = Tier2Task.Delete(out string delReason);
            ResetElevationStateAfterRevoke();
            _elevationRefused = true;
            _elevationRefusedReason = "持久授权已被用户撤销";
            return (deleted, delReason);
        }

        // 情形 B：非提权进程 → 请提权伴生进程代删（它已是管理员，删除无需 UAC）
        bool companionDeleted = false;
        string companionReason = "未连接提权伴生进程";
        if (!string.IsNullOrEmpty(exe))
        {
            (companionDeleted, companionReason) = AskCompanionToRevoke();
        }

        // 无论任务是否删掉，都要停用本会话的 tier-2（用户意图是"收回授权"）
        ResetElevationStateAfterRevoke();
        _elevationRefused = true;
        _elevationRefusedReason = "持久授权已被用户撤销";

        if (companionDeleted)
        {
            return (true, companionReason);
        }

        // 删除失败：如实报告，但本会话 tier-2 已停用
        return (false, $"已停用本会话 tier-2，但计划任务删除未完成（{companionReason}）");
    }

    /// <summary>请提权伴生进程删除计划任务（GET /api/v1/revoke，内部通道）。</summary>
    private static (bool ok, string reason) AskCompanionToRevoke()
    {
        try
        {
            using var client = new TcpClient();
            var connect = client.BeginConnect(IPAddress.Loopback, _port + 1, null, null);
            if (!connect.AsyncWaitHandle.WaitOne(500))
            {
                client.Close();
                return (false, "提权伴生进程未在 500ms 内接受连接");
            }
            client.EndConnect(connect);

            using NetworkStream stream = client.GetStream();
            client.ReceiveTimeout = 3000;
            stream.ReadTimeout = 3000;

            byte[] req = Encoding.ASCII.GetBytes(
                "GET /api/v1/revoke HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
            stream.Write(req, 0, req.Length);
            stream.Flush();

            byte[] buf = new byte[8 * 1024];
            int total = 0;
            while (total < buf.Length)
            {
                int n = stream.Read(buf, total, buf.Length - total);
                if (n <= 0) break;
                total += n;
            }

            string text = Encoding.UTF8.GetString(buf, 0, total);
            int split = text.IndexOf("\r\n\r\n", StringComparison.Ordinal);
            if (split < 0) return (false, "伴生进程响应缺少头部结束标记");

            using var doc = JsonDocument.Parse(text[(split + 4)..]);
            bool revoked = doc.RootElement.TryGetProperty("revoked", out var rv) && rv.GetBoolean();
            string reason = doc.RootElement.TryGetProperty("reason", out var rs)
                ? rs.GetString() ?? string.Empty : string.Empty;
            return (revoked, string.IsNullOrEmpty(reason) ? "伴生进程已处理撤销" : reason);
        }
        catch (Exception ex)
        {
            return (false, $"{ex.GetType().Name}: {ex.Message}");
        }
    }

    /// <summary>撤销后复位提权状态（§D.2）。</summary>
    private static void ResetElevationStateAfterRevoke()
    {
        _elevatedRole = false;
        _companionPid = 0;
        _tier2LastSuccess = DateTime.MinValue;
        _tier2LastError = null;
        _tier2Failures = 0;
        Tier2Task.InvalidateExistsCache();
    }

    // ============================================================================
    // PawnIO / WinRing0 设备预检（LHM 打不开设备时不抛异常而是返回空/0 值，需显式探测）
    // ============================================================================

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateFileW(string lpFileName, uint dwDesiredAccess, uint dwShareMode,
        IntPtr lpSecurityAttributes, uint dwCreationDisposition, uint dwFlagsAndAttributes,
        IntPtr hTemplateFile);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool CloseHandle(IntPtr hObject);

    private const uint GENERIC_READ = 0x8000_0000;
    private const uint OPEN_EXISTING = 3;
    private const int ERROR_FILE_NOT_FOUND = 2;
    private const int ERROR_ACCESS_DENIED = 5;

    /// <summary>
    /// 探测 WinRing0 设备可达性（\\.\WinRing0_1_2_0）。保留自既有 sidecar。
    /// </summary>
    private static (bool ok, string? reason) ProbeWinRing0()
    {
        IntPtr h = CreateFileW(@"\\.\WinRing0_1_2_0", GENERIC_READ, 0, IntPtr.Zero, OPEN_EXISTING, 0, IntPtr.Zero);
        if (h == new IntPtr(-1) || h == IntPtr.Zero)
        {
            int err = Marshal.GetLastWin32Error();
            string hint = err switch
            {
                ERROR_FILE_NOT_FOUND => "驱动未就绪：SECM 随包提供 WinRing0x64.sys（WHQL 交叉签名，Secure Boot 可加载），请运行 SECM 的驱动安装引导部署后重试",
                ERROR_ACCESS_DENIED => "当前进程无管理员权限（WinRing0 设备需管理员），请以管理员身份运行 SECM 或调用 POST /api/v1/elevate 提权",
                _ => $"错误码 0x{err:X8}"
            };
            return (false, $"WinRing0 设备不可访问（CreateFileW \\\\.\\WinRing0_1_2_0 失败：{hint}）");
        }
        CloseHandle(h);
        return (true, null);
    }

    /// <summary>
    /// 探测 PawnIO 设备可达性（\\.\PawnIO，DACL 仅 SYSTEM/Administrators）。保留自既有 sidecar。
    ///
    /// 签名事实（2026-08-14 核实）：随包 PawnIO 2.2.0 的 WHQL 签名证书虽于 2026-07-15 到期，
    /// 但带微软有效时间戳（2025-08-15 签名，时间戳证书 2026-11-14 到期）——按 Windows 内核驱动
    /// 签名策略，签名时证书有效且时间戳有效即视为有效签名，Secure Boot 下可正常加载
    /// （Get-AuthenticodeSignature = Valid）。因此 PawnIO 为 LHM 主 ring0 通道，不因证书到期降级。
    /// </summary>
    private static (bool ok, string? reason) ProbePawnIo()
    {
        IntPtr h = CreateFileW(@"\\.\PawnIO", GENERIC_READ, 0, IntPtr.Zero, OPEN_EXISTING, 0, IntPtr.Zero);
        if (h == new IntPtr(-1) || h == IntPtr.Zero)
        {
            int err = Marshal.GetLastWin32Error();
            string hint = err switch
            {
                ERROR_FILE_NOT_FOUND => "驱动未就绪：SECM 随包提供 PawnIO 2.2.0（WHQL 签名 + 有效时间戳，Secure Boot 可加载），请运行 SECM 的驱动安装引导自动部署后重试；若部署失败请检查安全软件（Windows Defender 内核隔离 / 火绒驱动防护 / 360 等）是否拦截 PawnIO.sys 加载",
                ERROR_ACCESS_DENIED => "当前进程无管理员权限（PawnIO 设备仅 SYSTEM/Administrators 可访问），请调用 POST /api/v1/elevate 触发一次 UAC 提权（tier-1 指标不受影响）",
                _ => $"错误码 0x{err:X8}"
            };
            return (false, $"PawnIO 设备不可访问（CreateFileW \\\\.\\PawnIO 失败：{hint}）");
        }
        CloseHandle(h);
        return (true, null);
    }

    /// <summary>
    /// 双后端预检：PawnIO（主）或 WinRing0（回退）任一可达即可。
    /// </summary>
    private static (bool ok, string? reason) ProbeRing0Backend()
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

    /// <summary>标准日志：stdout + 文件。</summary>
    internal static void Log(string line)
    {
        Console.WriteLine(line);
        FileLog(line);
    }

    /// <summary>错误日志：stderr + 文件。</summary>
    internal static void LogError(string line)
    {
        Console.Error.WriteLine(line);
        FileLog(line);
    }

    /// <summary>追加写入文件日志（失败不中断主流程）。</summary>
    private static void FileLog(string line)
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
        // ---- 参数解析（--port / --interval / --role=elevated）----
        _elevatedRole = false;
        for (int i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--port" when i + 1 < args.Length
                    && int.TryParse(args[i + 1], out int p) && p is > 0 and < 65535:
                    _port = p;
                    i++;
                    break;
                case "--interval" when i + 1 < args.Length
                    && int.TryParse(args[i + 1], out int iv) && iv >= 100:
                    _intervalMs = iv;
                    i++;
                    break;
                case "--role=elevated":
                    _elevatedRole = true;
                    break;
            }
        }

        _elevated = IsElevated();

        // 主进程端口的权威推导（见 _primaryPort 注释）：伴生进程的 --port 已是主进程 port+1。
        _primaryPort = DerivePrimaryPort(_port, _elevatedRole);

        Log($"[LhmSidecar] starting port={_port} interval={_intervalMs}ms role={(_elevatedRole ? "elevated" : "primary")} elevated={_elevated} contract=3");

        // 管理员环境零等待直通（CHANGE-CONTRACT-v2 §D.1 的提前化）。
        //
        // 原实现把「管理员 → tier-2」只放在 AutoAuthorizeLoop 线程里，而该线程要先等
        // tier-1 出数（最多 10s）。结果是管理员环境下**首次开树 tier2=False**，出数后
        // 才发现自己是管理员，于是关闭并重开一次硬件树——温度/功耗/主板整整晚一轮
        // （实测日志顺序：`LHM hardware tree opened ... tier2=False` 之后才是
        // 「检测到管理员权限：tier-2 已自动启用」）。
        //
        // 在这里（端口推导之后，见 _primaryPort 注释：置位会改变 DerivePrimaryPort 的结果，
        // 故必须在它之后）立即置位，使**首次开树即含 tier-2 组**，与 tier-1 同批出数。
        if (_elevated)
        {
            _elevatedRole = true;
            _elevationAttempted = true;
            Log("[LhmSidecar] 管理员令牌：首次开树即启用 tier-2（零点击，契约 §D.1）");
        }

        // ---- 提权伴生进程：注册持久授权（本进程已是管理员 → 注册不额外弹 UAC）----
        // 放在采集/HTTP 之前同步执行：注册是毫秒级操作，且必须在"用户已授权"的这一刻完成。
        if (_elevatedRole)
        {
            RegisterPersistentAuthorization();
        }

        // Ctrl+C 优雅退出：关闭 LHM 驱动句柄后退出（SECM 正常 kill 亦覆盖）。
        Console.CancelKeyPress += (_, e) =>
        {
            e.Cancel = true;
            Shutdown();
            Environment.Exit(0);
        };

        // ---- 采集后台线程（异常全部捕获，不中断 HTTP 服务）----
        Thread collector = new(() => CollectionLoop())
        {
            IsBackground = true,
            Name = "lhm-collector"
        };
        collector.Start();

        // ---- 自动授权线程：让 tier-2 在"管理员环境"与"已授权"两种情形下都零点击 ----
        // 独立后台线程，不阻塞 HTTP 服务与 tier-1 采集（§D.1/§D.2 要求后台、非阻塞）。
        Thread autoElevate = new(() => AutoAuthorizeLoop())
        {
            IsBackground = true,
            Name = "lhm-auto-authorize"
        };
        autoElevate.Start();

        // ---- HTTP 服务（主线程阻塞监听）----
        RunHttpServer(_port);
    }

    /// <summary>
    /// tier-1 就绪后的自动授权尝试（CHANGE-CONTRACT-v2.md §D.1/§D.2）：
    ///
    ///   情形 1（管理员环境，零点击）：本进程已是管理员 → 直接置位 _elevatedRole，
    ///           采集循环下一轮即打开 tier-2 组。这是用户投诉 2 的修复。
    ///   情形 2（曾授权一次，冷启动静默）：计划任务已注册 → 后台 `schtasks /Run`（**无 UAC**），
    ///           tier-2 自动回归。这是用户投诉 1 的修复。
    ///   情形 3（从未授权）：任务不存在 → **静默跳过，绝不 runas**（硬要求，§D.3）。
    ///
    /// 等 tier-1 出数后再动手，避免与首次 PDH 预热/LHM 打开争抢启动时间。
    /// </summary>
    private static void AutoAuthorizeLoop()
    {
        // 立即执行零 UAC 授权尝试，**不再等 tier-1 出数**（性能修复）。
        //
        // 原实现先等 tier-1 首次采样（最多 10s）再尝试授权，理由是避免与 PDH
        // 预热/LHM 开树争抢启动时间。但两条零 UAC 路径（管理员直通已在 Main 处理、
        // 已注册持久授权的 schtasks /Run）本身都不弹 UAC，让它们空等 tier-1 只会
        // 把提权伴生进程的 .NET 冷启动串行推到主进程预热之后——实测管理员环境下
        // tier-2 首次可用整整晚一轮采集。现在两者**重叠并发**启动。
        try
        {
            AutoAuthorizeOnce();
        }
        catch (Exception ex)
        {
            // 自动授权失败绝不能影响 tier-1/HTTP 服务
            _autoAuthorizeSkipped = true;
            LogError($"[LhmSidecar] 自动授权异常（已忽略）: {ex.GetType().Name}: {ex.Message}");
        }
    }

    /// <summary>
    /// 一次自动授权决策（与等待 tier-1 的循环解耦，便于单测）：
    ///   管理员环境 → 零点击启用；任务已注册 → 后台 /Run（免 UAC）；任务不存在 → 静默跳过。
    /// </summary>
    internal static (bool elevated, string reason) AutoAuthorizeOnce()
    {
        // ---- 情形 1：已是管理员 → 零点击启用（无需计划任务）----
        if (IsElevated())
        {
            _elevatedRole = true;
            _elevationRefused = false;
            _elevationRefusedReason = null;
            _elevationAttempted = true;
            Log("[LhmSidecar] 检测到管理员权限：tier-2 已自动启用（无需用户点击，契约 §D.1）");

            // 顺便建立持久授权：本进程已是管理员，注册计划任务同样不需要 UAC
            // （§D.2 的注册时机是"提权进程启动时"，本进程即是）。这样管理员首次运行后，
            // 之后即使以普通用户身份冷启动也能免 UAC 恢复 tier-2（用户投诉 1）。
            RegisterPersistentAuthorization();

            return (true, "本进程已具备管理员权限，tier-2 已自动启用");
        }

        // ---- 情形 2/3：计划任务 ----
        // Exists() 走缓存：不存在时本进程生命周期内只查一次 schtasks，开销可忽略。
        if (!Tier2Task.Exists())
        {
            // 硬要求：绝不因为"自动尝试"而弹 UAC
            _autoAuthorizeSkipped = true;
            Log($"[LhmSidecar] 计划任务 {Tier2Task.TaskName} 未注册：跳过自动授权（绝不自动 runas，契约 §D.3）");
            return (false, "未注册持久授权，跳过自动授权（绝不在未授权时 runas）");
        }

        // ---- 已注册：/Run 已有任务（绝不 runas）----
        bool ran = Tier2Task.RunExisting(out string runReason);
        if (!ran)
        {
            _autoAuthorizeSkipped = true;
            LogError($"[LhmSidecar] 自动授权跳过：{runReason}");
            return (false, runReason);
        }

        Log($"[LhmSidecar] {runReason}：等待 tier-2 就绪（冷启动免 UAC，契约 §D.2）");

        // 有界等待 tier-2 数据出现；出现即可，实际合并由采集循环完成
        var wait = Stopwatch.StartNew();
        while (wait.ElapsedMilliseconds < ElevationWaitMs)
        {
            if (ProbeTier2Available())
            {
                _elevationAttempted = true;
                Log($"[LhmSidecar] tier-2 已由持久授权自动恢复（{wait.ElapsedMilliseconds}ms，无 UAC）");
                return (true, "tier-2 已由持久授权自动恢复（无 UAC）");
            }
            Thread.Sleep(ProbeIntervalMs);
        }

        _autoAuthorizeSkipped = true;
        LogError($"[LhmSidecar] 已触发计划任务但 {ElevationWaitMs}ms 内未取到 tier-2 数据");
        return (false, $"{ElevationWaitMs}ms 内未取到 tier-2 数据");
    }

    /// <summary>自动授权是否已放弃（未授权场景），用于日志与测试观测。</summary>
    private static bool _autoAuthorizeSkipped;

    /// <summary>测试可见：自动授权是否跳过（未注册任务 / 管理员直通则会提前返回）。</summary>
    internal static bool AutoAuthorizeSkippedForTesting => _autoAuthorizeSkipped;

    /// <summary>测试可见：本进程是否已直接提供 tier-2（管理员直通或提权伴生角色）。</summary>
    internal static bool LocalTier2EnabledForTesting => LocalTier2Enabled;

    /// <summary>
    /// 提权伴生进程启动时的持久化注册（CHANGE-CONTRACT-v2.md §D.2）：
    /// 本进程**已是管理员**，故 `schtasks /Create ... /RL HIGHEST /F` **不额外弹 UAC**。
    /// 注册成功后，今后冷启动就能靠 /Run 免 UAC 恢复 tier-2。
    ///
    /// 说明：不把 IsElevated() 作为前置门槛 —— 提权伴生进程本身要么已是管理员，
    /// 要么（极端情况下令牌检测失败/受限令牌误判）schtasks 会直接拒绝访问。
    /// 两种结果都是"失败只记日志"（§D.3），因此这里以 `_elevatedRole` 为唯一条件更稳健：
    /// 真正的管理员一定会注册成功，而非管理员只是一次无害的失败尝试。
    /// </summary>
    private static void RegisterPersistentAuthorization()
    {
        string exe = Environment.ProcessPath ?? string.Empty;
        if (string.IsNullOrEmpty(exe))
        {
            LogError("[LhmSidecar] 无法定位自身可执行文件，跳过持久授权注册");
            return;
        }

        // 注册的命令行必须让任务拉起的伴生进程监听**主进程的 port+1**。
        // _primaryPort 在 Main 里按角色推导（见其注释）：主进程 = _port，伴生进程 = _port-1。
        // 用 _primaryPort 而非 _port，避免"以管理员启动的主进程"把任务端口写成自己的端口，
        // 导致任务拉起的伴生进程与主进程抢同一个端口。
        if (Tier2Task.Register(exe, _primaryPort, _intervalMs, out string reason))
        {
            Log($"[LhmSidecar] {reason}");
        }
        else
        {
            LogError($"[LhmSidecar] 持久授权注册失败（回退按需 runas，功能不劣化）：{reason}");
        }
    }

    /// <summary>关闭 LHM 与计数器资源（幂等）。</summary>
    private static void Shutdown()
    {
        try { _lhm?.Dispose(); } catch { /* 关闭失败不致命 */ }
        try { _tier1?.Dispose(); } catch { /* 关闭失败不致命 */ }
    }

    // ============================================================================
    // 采集
    // ============================================================================

    private static LhmCollector? _lhm;
    private static Tier1Collector? _tier1;
    private static SnapshotBuilder? _builder;

    /// <summary>
    /// 首次进入采集循环时创建 tier-1 采集器（PDH 预热 + DriveInfo）。
    /// **不经任何提权路径**：即使 PawnIO 完全不可用，tier-1 也必须工作（契约 §1）。
    /// </summary>
    private static Tier1Collector EnsureTier1()
    {
        if (_tier1 != null) return _tier1;

        var t = new Tier1Collector();
        try
        {
            t.WarmUp();   // 首次 NextValue() 恒为 0，必须预热丢弃（契约 §5.3）
            Log(t.PerfCounterReady
                ? "[LhmSidecar] tier-1 PDH counters initialized (pre-warmed)"
                : "[LhmSidecar] tier-1 PDH counters unavailable（CPU 负载/频率/每盘速率将回退 LHM 或留空，绝不伪造 0）");
        }
        catch (Exception ex)
        {
            LogError($"[LhmSidecar] tier-1 计数器初始化失败: {ex.GetType().Name}: {ex.Message}");
        }
        _tier1 = t;
        return t;
    }

    /// <summary>
    /// 后台采集主循环：tier-1 永远采集；tier-2 仅在提权可用时尝试（含退避重试与异常隔离）。
    /// </summary>
    private static void CollectionLoop()
    {
        var tier1 = EnsureTier1();
        _lhm = new LhmCollector();
        _builder = new SnapshotBuilder(_lhm.SensorMap);

        var lastInitAttempt = DateTime.MinValue;
        var lastErrorLogged = new Dictionary<string, DateTime>();
        var networkHardwares = new List<LibreHardwareMonitor.Hardware.IHardware>();

        while (true)
        {
            try
            {
                // ---------------- LHM 硬件树生命周期 ----------------
                // tier-1 组（CPU/GPU/内存/电池）**始终打开**：GPU 走厂商用户态通道，
                // 未提权也能读温度/功耗/显存（CONTRACT-v3.md §1 层级表）。
                // tier-2 组（主板/SuperIO、存储温度、控制器风扇）仅在本进程已具备管理员令牌
                // （提权伴生进程，或 SECM 本身以管理员运行）且 ring0 预检通过时打开。
                bool tier2Allowed = LocalTier2Enabled;

                // 权限在运行中升级（例如 SECM 本身是管理员，POST /api/v1/elevate 后）：
                // 已打开但只含 tier-1 组 → 关闭以便下一轮按 tier-2 组重开。
                if (tier2Allowed && _lhm.IsOpen && !_lhm.Tier2GroupsEnabled)
                {
                    Log("[LhmSidecar] 权限升级：重开 LHM 硬件树以启用 tier-2 组（主板/存储/控制器）");
                    _lhm.Close();
                    _builder.Processor = null;
                    networkHardwares = new List<LibreHardwareMonitor.Hardware.IHardware>();
                }

                if (!_lhm.IsOpen)
                {
                    // 初始化失败后按退避间隔重试，避免热循环占 CPU
                    if ((DateTime.UtcNow - lastInitAttempt).TotalMilliseconds < InitRetryMs)
                    {
                        Thread.Sleep(Math.Min(_intervalMs, InitRetryMs));
                        continue;
                    }
                    lastInitAttempt = DateTime.UtcNow;

                    bool includeTier2 = false;
                    if (tier2Allowed)
                    {
                        // Ring0 双后端预检（LHM 打不开设备时不抛异常而是静默降级，必须显式探测）。
                        // 预检失败不再 return —— tier-1 组仍然要打开。
                        var (probeOk, probeReason) = ProbeRing0Backend();
                        if (probeOk)
                        {
                            includeTier2 = true;
                        }
                        else
                        {
                            ReportTier2Problem(probeReason ?? "Ring0 后端不可访问（未知原因）", lastErrorLogged);
                        }
                    }

                    try
                    {
                        _lhm.Open(includeTier2);
                        _builder.Processor = _lhm.Processor;
                        networkHardwares = _lhm.NetworkHardware();
                        // CONTRACT-v3.md §8：「`NetworkManager` 必须完整移植并在采集循环中
                        // 驱动（它同时是 LHM 吞吐兜底通道的持有者）」。此前只驱动了
                        // ProcessUpdate，却从未调用 SelectBestNetworkHardware 选定目标硬件，
                        // 导致 `NetworkManager._cachedNetHw` 恒为 null → `ProcessUpdate` 的
                        // isTarget 恒 false → AccumulateTraffic 从不执行 → ConsumeDelta 恒为
                        // (0,0,0)，整条吞吐链路空转（ADR-0001 D13 的「只写不读」同类问题）。
                        // CONTRACT-v3.md §8 要求 NetworkManager 在采集循环中被驱动。
                        // 只调用 ProcessUpdate 而不先选定目标硬件时，`_cachedNetHw` 恒为 null
                        // → `isTarget` 恒 false → 流量累积与 ConsumeDelta 全程空转。
                        // 选定动作需要 IComputer，故由 LhmCollector 在内部完成。
                        _lhm.SelectNetworkHardware(tier1.Network);
                        Log($"[LhmSidecar] LHM hardware tree opened (cpu=\"{_lhm.CpuName}\" gpus={_lhm.GpuHardware().Count} network={networkHardwares.Count} tier2={includeTier2})");
                    }
                    catch (Exception ex)
                    {
                        ReportTier2Problem($"LHM 打开失败: {ex.GetType().Name}: {ex.Message}", lastErrorLogged);
                        _lhm.Close();
                        Thread.Sleep(Math.Min(_intervalMs, InitRetryMs));
                        continue;
                    }
                }

                // ---------------- 采集（含 tier-2；异常隔离：失败不影响 tier-1）----------------
                bool tier2Ticked = false;
                if (_lhm.IsOpen)
                {
                    try
                    {
                        tier2Ticked = _lhm.Tick(dt =>
                        {
                            // 上游 HardwareMonitor.cs:216-221 / NetworkManager.ProcessUpdate
                            // 必须遍历**全部**网络硬件：ProcessUpdate 内部按 isTarget 判定
                            // 目标网卡（非目标在启动窗口内直接 return），只处理第 0 个会让
                            // 选定结果与遍历顺序不一致时彻底采不到流量。
                            foreach (var networkHw in networkHardwares)
                            {
                                tier1.Network.ProcessUpdate(networkHw, dt, true);
                            }
                        });

                        // 上游 HardwareMonitor.cs:250-270：GPU Update 失败触发重载
                        if (_lhm.ReloadRequested)
                        {
                            LogError("[LhmSidecar] GPU 更新失败，重载 LHM 硬件树（上游 HardwareMonitor.cs:250-270）");
                            _lhm.Close();
                            _builder.Processor = null;
                            networkHardwares = new List<LibreHardwareMonitor.Hardware.IHardware>();
                        }
                    }
                    catch (Exception ex)
                    {
                        // 采集异常隔离：本轮 tier-2 缺失，tier-1 照常输出
                        ReportTier2Problem($"LHM 采集失败: {ex.GetType().Name}: {ex.Message}", lastErrorLogged);
                        _lhm.Close();
                        _builder.Processor = null;
                        networkHardwares = new List<LibreHardwareMonitor.Hardware.IHardware>();
                    }
                }

                // ---------------- tier-1（PDH + DriveInfo，永不依赖提权）----------------
                BatteryData? battery = null;
                if (_lhm.IsOpen)
                {
                    try { battery = _lhm.ReadBattery(); }
                    catch (Exception ex) { LogThrottled(lastErrorLogged, "battery", $"电池读取失败: {ex.Message}"); }
                }
                tier1.Collect(battery);

                // ---------------- tier-2 取值（本地或代理伴生进程）----------------
                Tier2Payload? payload;
                bool elevatedEffective;
                bool tier2Available;
                string? tier2Reason;

                if (LocalTier2Enabled)
                {
                    payload = tier2Ticked ? BuildLocalTier2Payload() : null;
                    tier2Available = payload?.Available == true;
                    elevatedEffective = tier2Available;
                    tier2Reason = tier2Available
                        ? null
                        : (_tier2LastError ?? "提权进程已就绪但未读到任何 tier-2 指标");
                }
                else
                {
                    payload = FetchCompanionTier2();
                    tier2Available = payload?.Available == true;
                    elevatedEffective = tier2Available;
                    // 优先级：用户级持久状态（撤销/拒绝）> 伴生进程诊断 > 通用引导文案。
                    // 撤销后不应显示"伴生进程未连接"这类瞬时诊断，那会误导用户以为要重试连接。
                    tier2Reason = tier2Available
                        ? null
                        : (_elevationRefused
                           ? (_elevationRefusedReason ?? "授权已被用户撤销")
                           : (payload?.Reason
                              ?? (_elevationAttempted ? _tier2LastError : null)
                              ?? Tier2ReasonForAbsent(tier2Allowed)));
                }

                // ---------------- 组装并发布快照 ----------------
                bool lhmOpen = _lhm.IsOpen;
                var snap = _builder.Build(
                    lhmCpuName: lhmOpen ? _lhm.CpuName : string.Empty,
                    gpuHardware: lhmOpen
                        ? _lhm.GpuHardware()
                        : new List<LibreHardwareMonitor.Hardware.IHardware>(),
                    tier1: tier1.Current,
                    tier2: payload?.ToReadout(),
                    elevated: elevatedEffective,
                    tier2Available: tier2Available,
                    tier2Reason: tier2Reason,
                    memoryName: lhmOpen ? _lhm.ReadMemoryName() : null,
                    memoryLhm: lhmOpen ? _lhm.ReadMemoryFromLhm() : (null, null));

                // tier-2 快照必须与整份快照同批发布：两者出自同一轮采集，
                // 分开发布会让 HTTP 读到"新一轮整份快照 + 上一轮 tier-2"的错配组合。
                PublishTier2Snapshot(payload);

                lock (SnapshotLock)
                {
                    _snapshot = snap;
                }
                _lastError = null;
            }
            catch (Exception ex)
            {
                // 兜底：任何未预期异常都不能拖垮采集循环或 HTTP 服务
                string msg = $"采集循环异常: {ex.GetType().Name}: {ex.Message}";
                if (_lastError != msg)
                {
                    _lastError = msg;
                    LogError($"[LhmSidecar] {msg}");
                }
            }

            Thread.Sleep(_intervalMs);
        }
    }

    /// <summary>提权进程本地组装 tier-2 负载。</summary>
    private static Tier2Payload BuildLocalTier2Payload()
    {
        try
        {
            var r = _lhm!.ReadTier2();
            var payload = new Tier2Payload
            {
                Available = r.HasAny,
                Reason = r.HasAny ? null : (_tier2LastError ?? "提权进程已就绪但未读到任何 tier-2 传感器"),
                Cpu = new Tier2Cpu
                {
                    PackageTempC = r.CpuTempC.HasValue ? r.CpuTempC.Value : null,
                    CoreTempsC = r.CoreTempsC,
                    PowerW = r.CpuPowerW.HasValue ? r.CpuPowerW.Value : null,
                    VoltageV = r.CpuVoltageV.HasValue ? r.CpuVoltageV.Value : null,
                    FanRpm = r.CpuFanRpm.HasValue ? r.CpuFanRpm.Value : null,
                    PumpRpm = r.CpuPumpRpm.HasValue ? r.CpuPumpRpm.Value : null
                },
                Motherboard = r.Motherboard,
                DiskTemps = r.DiskTemps.Count > 0
                    ? r.DiskTemps.ToDictionary(kv => kv.Key, kv => (double)kv.Value)
                    : null,
                RepresentativeDiskTempC = r.RepresentativeDiskTempC.HasValue
                    ? r.RepresentativeDiskTempC.Value : null,
                StorageHardware = _lhm?.DescribeStorageHardware()
            };

            if (payload.Available)
            {
                _tier2LastSuccess = DateTime.UtcNow;
                _tier2LastError = null;
            }
            else
            {
                _tier2LastError = payload.Reason;
            }
            return payload;
        }
        catch (Exception ex)
        {
            string reason = $"tier-2 读取失败: {ex.GetType().Name}: {ex.Message}";
            _tier2LastError = reason;
            return new Tier2Payload { Available = false, Reason = reason };
        }
    }

    /// <summary>
    /// 未提权主进程通过 HTTP 代理伴生进程（127.0.0.1:port+1）的 tier-2 读取。
    /// 无条件尝试：回环连接被拒绝是立即返回（ECONNREFUSED），无伴生进程时开销可忽略；
    /// 这样"提权伴生进程已在服务"这一事实本身就足以让快照含 tier-2，不依赖本进程的内部状态。
    /// 伴生进程不存在/未就绪时返回 null（字段保持 null，绝不伪造 0）。
    /// </summary>
    private static Tier2Payload? FetchCompanionTier2()
    {
        try
        {
            using var client = new TcpClient();
            // 连接超时：伴生进程未起来时不能拖慢采集循环
            var connect = client.BeginConnect(IPAddress.Loopback, _port + 1, null, null);
            if (!connect.AsyncWaitHandle.WaitOne(500))
            {
                client.Close();
                NoteTier2Failure("提权伴生进程未在 500ms 内接受连接");
                return null;
            }
            client.EndConnect(connect);

            using NetworkStream stream = client.GetStream();
            client.ReceiveTimeout = 2000;
            stream.ReadTimeout = 2000;

            byte[] req = Encoding.ASCII.GetBytes(
                "GET /api/v1/tier2 HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
            stream.Write(req, 0, req.Length);
            stream.Flush();

            byte[] buf = new byte[64 * 1024];
            int total = 0;
            while (total < buf.Length)
            {
                int n = stream.Read(buf, total, buf.Length - total);
                if (n <= 0) break;
                total += n;
            }

            string text = Encoding.UTF8.GetString(buf, 0, total);
            int split = text.IndexOf("\r\n\r\n", StringComparison.Ordinal);
            if (split < 0) { NoteTier2Failure("伴生进程响应缺少头部结束标记"); return null; }

            string json = text[(split + 4)..];
            var payload = JsonSerializer.Deserialize<Tier2Payload>(json, JsonOpts);
            if (payload == null) { NoteTier2Failure("伴生进程 tier-2 响应无法解析"); return null; }

            if (payload.Available)
            {
                _tier2LastSuccess = DateTime.UtcNow;
                _tier2LastError = null;
                _tier2Failures = 0;
            }
            else
            {
                _tier2LastError = payload.Reason;
            }
            return payload;
        }
        catch (Exception ex)
        {
            NoteTier2Failure($"tier-2 代理失败: {ex.GetType().Name}: {ex.Message}");
            return null;
        }
    }

    private static void NoteTier2Failure(string reason)
    {
        _tier2LastError = reason;
        _tier2Failures++;
        // 连续失败过多：丢弃句柄，允许用户重新触发一次 UAC（不自动重弹窗）
        if (_tier2Failures == 30)
        {
            LogError($"[LhmSidecar] tier-2 连续失败 30 次，停止代理伴生进程（{reason}）");
            _companionPid = 0;
        }
    }

    /// <summary>未提权时的 tier-2 原因文案（含修复建议）。</summary>
    private static string Tier2ReasonForUnprivileged()
    {
        if (_elevationRefused) return _elevationRefusedReason ?? "UAC 提权此前已被用户取消";
        return "需要管理员权限（PawnIO 设备仅 SYSTEM/Administrators 可访问）：调用 POST /api/v1/elevate 触发一次 UAC 提权后，CPU 温度/功耗/电压、主板传感器与磁盘温度即可用";
    }

    /// <summary>
    /// tier-2 缺失时的原因文案（区分"本进程本可直读"与"需要伴生进程"两种情形，
    /// 避免在已是管理员的进程上给出"伴生进程未连接"这种误导性诊断）。
    /// </summary>
    private static string Tier2ReasonForAbsent(bool tier2AllowedLocally)
    {
        if (tier2AllowedLocally)
        {
            return "本进程具备管理员权限，但 ring0 后端不可访问或尚未完成 tier-2 采集：" +
                   "请检查 PawnIO/WinRing0 驱动状态；tier-1 指标不受影响";
        }
        return Tier2ReasonForUnprivileged();
    }

    /// <summary>tier-2 问题记录（同文案在 60s 内只记一次，避免刷屏）。</summary>
    private static void ReportTier2Problem(string message, Dictionary<string, DateTime> logTimes)
    {
        _tier2LastError = message;
        if (logTimes.TryGetValue(message, out var last) && (DateTime.UtcNow - last).TotalSeconds < 60) return;
        logTimes[message] = DateTime.UtcNow;
        LogError($"[LhmSidecar] tier-2 不可用：{message}");
    }

    private static void LogThrottled(Dictionary<string, DateTime> logTimes, string key, string message)
    {
        if (logTimes.TryGetValue(key, out var last) && (DateTime.UtcNow - last).TotalSeconds < 60) return;
        logTimes[key] = DateTime.UtcNow;
        LogError($"[LhmSidecar] {message}");
    }

    /// <summary>空快照（启动瞬间的占位，tier-1 立刻接上）。</summary>
    private static Snapshot NewEmptySnapshot(string reason) => new()
    {
        Available = false,
        Error = reason,
        ContractVersion = 3,
        Elevated = false,
        Tier2Available = false,
        Tier2Reason = null,
        Cpu = new CpuData(),
        Gpu = new List<GpuData>(),
        Motherboard = null,
        Memory = null,
        Disks = new List<DiskData>(),
        Battery = null
    };

    // ============================================================================
    // HTTP 服务（极简 HTTP/1.1，无 ASP.NET Core 依赖）
    // ============================================================================

    /// <summary>绑定 127.0.0.1 回环并循环处理请求（每连接独立线程池任务）。</summary>
    private static void RunHttpServer(int port)
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

    /// <summary>单个连接处理：极简请求头解析 + 路由 + JSON 响应。</summary>
    private static void HandleClient(object? state)
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
                // 读超时：防慢速连接长期占用线程池线程；契约 §2 要求读写超时 2s
                client.ReceiveTimeout = 2000;
                stream.ReadTimeout = 2000;
                stream.WriteTimeout = 2000;

                // 读取请求头直到空行（\r\n\r\n），上限 8KB（契约 §2）
                byte[] headerBuf = new byte[8192];
                int headerLen = 0;
                while (headerLen < headerBuf.Length)
                {
                    int n = stream.Read(headerBuf, headerLen, headerBuf.Length - headerLen);
                    if (n <= 0) return;
                    headerLen += n;
                    if (ContainsHeaderEnd(headerBuf, headerLen)) break;
                }

                string header = Encoding.ASCII.GetString(headerBuf, 0, headerLen);
                string requestLine = header.Split('\n')[0].TrimEnd('\r');

                // 请求行格式: "METHOD SP PATH SP HTTP/1.1"
                string[] parts = requestLine.Split(' ');
                string method = parts.Length > 0 ? parts[0] : string.Empty;
                string path = parts.Length > 1 ? parts[1] : "/";
                int query = path.IndexOf('?');
                if (query >= 0) path = path[..query];

                int status;
                string body;
                switch (path)
                {
                    case "/health":
                        if (!IsGet(method)) { status = 405; body = MethodNotAllowed(); }
                        else { status = 200; body = "{\"status\":\"ok\"}"; }
                        break;

                    case "/api/v1/snapshot":
                    case "/api/lhm/sensors":           // v2 路径别名（契约 §2 兼容性）
                        if (!IsGet(method)) { status = 405; body = MethodNotAllowed(); }
                        else { status = 200; body = SerializeSnapshot(); }
                        break;

                    case "/api/v1/privilege":
                        if (!IsGet(method)) { status = 405; body = MethodNotAllowed(); }
                        else { status = 200; body = SerializePrivilege(); }
                        break;

                    case "/api/v1/elevate":
                        if (!IsPost(method)) { status = 405; body = MethodNotAllowed(); }
                        else { status = 200; body = HandleElevate(); }
                        break;

                    case "/api/v1/elevate/auto":
                        // 零 UAC 自动提权（管理员直通 / 持久授权 /Run），绝不 runas。
                        // 失败由调用方决定是否另行发起用户 UAC 申请。
                        if (!IsPost(method)) { status = 405; body = MethodNotAllowed(); }
                        else { status = 200; body = HandleElevateAuto(); }
                        break;

                    case "/api/v1/elevate/revoke":
                        // 撤销持久授权（CHANGE-CONTRACT-v2.md §D.2/§D.4）
                        if (!IsPost(method)) { status = 405; body = MethodNotAllowed(); }
                        else { status = 200; body = HandleRevoke(); }
                        break;

                    case "/api/v1/tier2":
                        // 内部通道：仅提权进程提供 tier-2 原始负载
                        if (!IsGet(method)) { status = 405; body = MethodNotAllowed(); }
                        else if (!_elevatedRole)
                        {
                            status = 404;
                            body = "{\"error\":\"not found\"}";
                        }
                        else
                        {
                            // 只读采集线程已发布的快照，绝不在 HTTP 线程现场采集
                            // （ADR-0001 D3：消除跨线程共享可变状态）。
                            Tier2Payload? tier2Snap = ReadTier2Snapshot();
                            if (tier2Snap is null)
                            {
                                // 尚未完成首轮采集：如实返回不可用，不伪造数据。
                                status = 200;
                                body = JsonSerializer.Serialize(
                                    new Tier2Payload { Available = false, Reason = "提权进程尚未完成首轮采集" },
                                    JsonOpts);
                            }
                            else
                            {
                                status = 200;
                                body = JsonSerializer.Serialize(tier2Snap, JsonOpts);
                            }
                        }
                        break;

                    case "/api/v1/revoke":
                        // 内部通道：主进程请提权伴生进程代删计划任务
                        // （非提权进程 schtasks /Delete 会被拒绝访问——实测 exit=1）
                        if (!IsGet(method)) { status = 405; body = MethodNotAllowed(); }
                        else if (!_elevatedRole)
                        {
                            status = 404;
                            body = "{\"error\":\"not found\"}";
                        }
                        else
                        {
                            bool deleted = Tier2Task.Delete(out string taskReason);
                            ResetElevationStateAfterRevoke();
                            status = 200;
                            body = JsonSerializer.Serialize(
                                new RevokeResponse { Revoked = deleted, Reason = taskReason }, JsonOpts);
                        }
                        break;

                    default:
                        status = 404;
                        body = "{\"error\":\"not found\"}";
                        break;
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

    private static bool IsGet(string method) => string.Equals(method, "GET", StringComparison.OrdinalIgnoreCase);
    private static bool IsPost(string method) => string.Equals(method, "POST", StringComparison.OrdinalIgnoreCase);
    private static string MethodNotAllowed() =>
        "{\"error\":\"method not allowed: GET /health, /api/v1/snapshot, /api/v1/privilege; POST /api/v1/elevate, /api/v1/elevate/revoke\"}";

    /// <summary>POST /api/v1/elevate：触发一次提权（互斥，避免并发多次弹窗）。</summary>
    private static string HandleElevate()
    {
        ElevateGate.Wait();
        try
        {
            var (ok, reason) = RequestElevation();
            return JsonSerializer.Serialize(new ElevateResponse { Elevated = ok, Reason = reason }, JsonOpts);
        }
        finally
        {
            ElevateGate.Release();
        }
    }

    /// <summary>POST /api/v1/elevate/auto：只走零 UAC 自动路径（绝不 runas）。</summary>
    private static string HandleElevateAuto()
    {
        ElevateGate.Wait();
        try
        {
            var (ok, reason) = TryAutoElevate();
            return JsonSerializer.Serialize(new ElevateResponse { Elevated = ok, Reason = reason }, JsonOpts);
        }
        finally
        {
            ElevateGate.Release();
        }
    }

    /// <summary>POST /api/v1/elevate/revoke：撤销持久授权并停用本次会话 tier-2。</summary>
    private static string HandleRevoke()
    {
        ElevateGate.Wait();
        try
        {
            var (revoked, reason) = RevokeElevation();
            return JsonSerializer.Serialize(new RevokeResponse { Revoked = revoked, Reason = reason }, JsonOpts);
        }
        finally
        {
            ElevateGate.Release();
        }
    }

    /// <summary>POST /api/v1/elevate 的响应体（契约 §2）。</summary>
    private sealed class ElevateResponse
    {
        public bool Elevated { get; set; }
        public string Reason { get; set; } = string.Empty;
    }

    /// <summary>GET /api/v1/privilege 的响应体（契约 §2 + CHANGE-CONTRACT-v2.md §D.4）。</summary>
    private sealed class PrivilegeResponse
    {
        public bool Elevated { get; set; }
        public bool Tier2Available { get; set; }
        public string? Tier2Reason { get; set; }

        /// <summary>持久授权（计划任务）是否已注册。Rust 侧 #[serde(default)]，缺字段不 panic。</summary>
        public bool Persistent { get; set; }
    }

    /// <summary>POST /api/v1/elevate/revoke 的响应体（CHANGE-CONTRACT-v2.md §D.4）。</summary>
    private sealed class RevokeResponse
    {
        public bool Revoked { get; set; }
        public string Reason { get; set; } = string.Empty;
    }

    /// <summary>序列化权限状态（快照三字段 + persistent）。</summary>
    private static string SerializePrivilege()
    {
        Snapshot snap;
        lock (SnapshotLock) snap = _snapshot;

        // persistent = 计划任务是否已注册。
        // 注意 IsElevated() 直通（管理员环境）时任务可能未注册，但 tier-2 已由本进程提供——
        // persistent 描述的是"授权能否跨冷启动存活"，不是"当前是否有 tier-2"，故两者独立。
        bool persistent;
        try { persistent = Tier2Task.Exists(); }
        catch { persistent = false; }   // 查询失败按未注册处理（保守，不影响其他字段）

        return JsonSerializer.Serialize(new PrivilegeResponse
        {
            Elevated = snap.Elevated,
            Tier2Available = snap.Tier2Available,
            Tier2Reason = snap.Tier2Reason,
            Persistent = persistent
        }, JsonOpts);
    }

    /// <summary>在缓冲区中查找 HTTP 请求头结束标记 \r\n\r\n。</summary>
    private static bool ContainsHeaderEnd(byte[] buf, int len)
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

    private static string StatusText(int code) => code switch
    {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "OK"
    };

    /// <summary>序列化最新快照（序列化失败时返回降级 JSON，保证 HTTP 层不崩）。</summary>
    private static string SerializeSnapshot()
    {
        Snapshot snap;
        lock (SnapshotLock) snap = _snapshot;

        try
        {
            return JsonSerializer.Serialize(snap, JsonOpts);
        }
        catch (Exception ex)
        {
            // 降级响应仍严格保持契约 v3 形状：tier-2 字段 null，绝不用 0 冒充
            return "{\"available\":false," +
                   $"\"error\":\"LHM 快照序列化失败: {ex.GetType().Name}\"," +
                   "\"contract_version\":3,\"elevated\":false,\"tier2_available\":false," +
                   "\"tier2_reason\":\"快照序列化失败\",\"cpu\":{\"name\":\"\",\"load_percent\":null," +
                   "\"package_temp_c\":null,\"core_temps_c\":[],\"power_w\":null,\"voltage_v\":null," +
                   "\"clock_mhz\":null,\"fan_rpm\":null,\"pump_rpm\":null,\"load_source\":\"none\"," +
                   "\"temp_source\":\"none\",\"power_source\":\"none\",\"clock_source\":\"none\"," +
                   "\"power_estimated\":false},\"gpu\":[],\"motherboard\":null,\"memory\":null," +
                   "\"disks\":[],\"battery\":null}";
        }
    }
}
