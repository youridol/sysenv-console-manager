// Elevation/Tier2Task.cs —— 持久化 tier-2 授权（Windows 计划任务）
//
// 实现 CHANGE-CONTRACT-v2.md §D.2/§D.3：
//   注册一个 RunLevel=HighestAvailable 的计划任务；任务由**已是管理员**的提权伴生进程创建，
//   因此创建本身不额外弹 UAC。之后由同一用户 `schtasks /Run` 触发**也不会弹 UAC** ——
//   这正是"一次授权，冷启动不再弹 UAC"的机制基础。
//
// 实测结论（本机 admin 会话，2026-09-19，见任务报告）：
//   1. `/SC ONCE /ST 00:00` 的任务**不会**自行触发（等 8s 无执行）——"过去时间"是安全的；
//      它会给出 WARNING: Task may not run because /ST is earlier than current time.，exit=0。
//   2. 同一任务可被 `/Run` **反复**触发（连续 3 次全部执行，参数原样传递），
//      执行后 Status 回到 Ready —— 满足"按需重复运行"的用法，无需换 /SC。
//   3. 从**非提权**受限令牌 `/Run` → 任务进程 `elevated=True`，**无 UAC 弹窗**。
//   4. 非提权进程可 `/Query`（exit=0）但**不可** `/Delete`（拒绝访问 exit=1）
//      → 因此撤销删除必须在提权侧执行（见 Program 的 /api/v1/elevate/revoke 两段式流程）。
//
// 安全约束（§D.3，硬要求）：
//   * 不使用 /RU /RP —— 任务以当前用户上下文 + HIGHEST 运行，不落任何明文凭据。
//   * 所有 schtasks 调用窗口隐藏 + 有超时；失败静默降级（返回 false），绝不抛给调用方。
//   * 自动路径**只允许** /Run 一个**已存在**的任务；任务不存在时静默跳过，
//     **绝不**因此调用 runas（弹 UAC 比不启用更糟）。

using System.Diagnostics;

namespace LhmSidecar.Elevation;

/// <summary>计划任务的查询/运行/创建/删除封装（可注入执行器以便单测）。</summary>
public static class Tier2Task
{
    /// <summary>任务名（CHANGE-CONTRACT-v2.md §D.2 冻结，不得改名）。</summary>
    public const string TaskName = "SECM-LhmTier2";

    /// <summary>schtasks 调用超时（§D.3 要求有界）。</summary>
    public const int DefaultTimeoutMs = 5000;

    /// <summary>
    /// 执行器接缝：接收完整参数列表与超时，返回 (exitCode, stdout+stderr)。
    /// 单测替换它即可断言命令构造，并模拟"任务不存在""拒绝访问"等失败。
    /// 生产实现见 RunSchtasks。
    /// </summary>
    internal static Func<IReadOnlyList<string>, int, (int ExitCode, string Output)>? ExecutorForTesting;

    /// <summary>
    /// 组装提权伴生进程的参数（CHANGE-CONTRACT-v2.md §D.2 冻结）：
    /// `--port &lt;port+1&gt; --interval &lt;ms&gt; --role=elevated`
    /// </summary>
    public static string BuildCompanionArguments(int port, int intervalMs)
        => $"--port {port + 1} --interval {intervalMs} --role=elevated";

    /// <summary>
    /// `/Create` 的参数列表（不含 schtasks 本身）。
    /// 注意：用 ArgumentList 逐项传参（而不是拼一个字符串），
    /// 因为含空格/引号的 exe 路径在 PowerShell/CMD 下容易被重新转义（实测踩过这个坑）。
    /// </summary>
    public static IReadOnlyList<string> BuildCreateArguments(string exePath, int port, int intervalMs)
        => new[]
        {
            "/Create",
            "/TN", TaskName,
            "/TR", QuoteTaskRun(exePath, BuildCompanionArguments(port, intervalMs)),
            "/SC", "ONCE",
            "/ST", "00:00",
            "/RL", "HIGHEST",
            "/F"
        };

    /// <summary>
    /// 任务的 /TR 值：`"&lt;exe&gt;" &lt;args&gt;`。
    /// exe 路径含空格时必须加引号（schtasks 会把 /TR 当命令行解析）。
    /// </summary>
    public static string QuoteTaskRun(string exePath, string arguments)
    {
        string quoted = exePath.Contains(' ') ? $"\"{exePath}\"" : exePath;
        return $"{quoted} {arguments}";
    }

    /// <summary>`/Query` 参数（用于判断任务是否已注册）。</summary>
    public static IReadOnlyList<string> BuildQueryArguments()
        => new[] { "/Query", "/TN", TaskName };

    /// <summary>`/Run` 参数（触发已存在的任务；不弹 UAC）。</summary>
    public static IReadOnlyList<string> BuildRunArguments()
        => new[] { "/Run", "/TN", TaskName };

    /// <summary>`/Delete` 参数（撤销持久授权；需管理员权限）。</summary>
    public static IReadOnlyList<string> BuildDeleteArguments()
        => new[] { "/Delete", "/TN", TaskName, "/F" };

    /// <summary>
    /// 任务是否已注册。
    /// **缓存**结果：本进程内只需判定一次（任务要么在启动时已存在，要么由用户在本次会话中创建），
    /// 缓存同时保证自动路径每轮采集都查一次 schtasks 的开销为零。
    /// </summary>
    private static bool? _existsCache;

    /// <summary>任务是否已注册（结果缓存）。</summary>
    public static bool Exists()
    {
        if (_existsCache.HasValue) return _existsCache.Value;

        var (code, _) = RunSchtasks(BuildQueryArguments(), DefaultTimeoutMs);
        bool exists = code == 0;
        _existsCache = exists;
        return exists;
    }

    /// <summary>清空存在性缓存（注册/删除后必须调用，否则缓存会给出过期答案）。</summary>
    public static void InvalidateExistsCache() => _existsCache = null;

    /// <summary>测试用：强制设置存在性缓存。</summary>
    internal static void SetExistsCacheForTesting(bool? value) => _existsCache = value;

    /// <summary>
    /// 注册任务（幂等，/F 覆盖）。**必须由已是管理员的进程调用**，否则会失败（无 UAC 弹窗）。
    /// 失败只记录并返回 false —— 调用方回退到按需 runas，功能不劣化（§D.3）。
    /// </summary>
    public static bool Register(string exePath, int port, int intervalMs, out string reason)
    {
        if (string.IsNullOrEmpty(exePath))
        {
            reason = "无法定位自身可执行文件，跳过计划任务注册";
            return false;
        }

        var args = BuildCreateArguments(exePath, port, intervalMs);
        var (code, output) = RunSchtasks(args, DefaultTimeoutMs);

        if (code == 0)
        {
            InvalidateExistsCache();
            reason = $"计划任务 {TaskName} 已注册（RunLevel=HIGHEST，按需 /Run 触发，无常驻触发器）";
            return true;
        }

        // 组策略禁用计划任务 / 权限不足 等：仅记日志 + 返回原因，不抛异常
        reason = $"计划任务注册失败（exit={code}）：{Compact(output)}";
        return false;
    }

    /// <summary>
    /// 运行已存在的任务。**调用方必须先确认 Exists()** —— 本方法自身也再确认一次，
    /// 保证"自动路径绝不触发不存在的任务/绝不弹 UAC"这一硬要求在任何调用点都成立。
    /// </summary>
    public static bool RunExisting(out string reason)
    {
        if (!Exists())
        {
            reason = $"计划任务 {TaskName} 未注册，跳过（自动路径绝不 runas）";
            return false;
        }

        var (code, output) = RunSchtasks(BuildRunArguments(), DefaultTimeoutMs);
        if (code == 0)
        {
            reason = $"已通过计划任务 {TaskName} 触发提权伴生进程（无 UAC）";
            return true;
        }

        reason = $"计划任务触发失败（exit={code}）：{Compact(output)}";
        return false;
    }

    /// <summary>
    /// 删除任务（撤销持久授权）。需管理员权限；非提权调用会返回 false。
    /// </summary>
    public static bool Delete(out string reason)
    {
        var (code, output) = RunSchtasks(BuildDeleteArguments(), DefaultTimeoutMs);
        InvalidateExistsCache();   // 无论成功与否都重查，避免缓存过期
        if (code == 0)
        {
            reason = $"计划任务 {TaskName} 已删除";
            return true;
        }
        reason = $"计划任务删除失败（exit={code}）：{Compact(output)}";
        return false;
    }

    /// <summary>当前进程是否有权删除该任务（非提权时 schtasks /Delete 会拒绝访问）。</summary>
    public static bool CanManage => Program.IsElevated();

    /// <summary>执行 schtasks；窗口隐藏、有超时、任何异常都吞掉并返回非 0。</summary>
    internal static (int ExitCode, string Output) RunSchtasks(IReadOnlyList<string> args, int timeoutMs)
    {
        if (ExecutorForTesting != null) return ExecutorForTesting(args, timeoutMs);

        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "schtasks.exe",
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,      // 需要重定向输出
                CreateNoWindow = true,        // §D.3：不弹控制台窗口
                WindowStyle = ProcessWindowStyle.Hidden
            };
            foreach (var a in args) psi.ArgumentList.Add(a);

            using var proc = Process.Start(psi);
            if (proc == null) return (-1, "无法启动 schtasks.exe");

            string stdout = proc.StandardOutput.ReadToEnd();
            string stderr = proc.StandardError.ReadToEnd();

            if (!proc.WaitForExit(timeoutMs))
            {
                try { proc.Kill(entireProcessTree: true); } catch { /* 已退出 */ }
                return (-1, $"schtasks 超时（{timeoutMs}ms）");
            }

            return (proc.ExitCode, (stdout + stderr).Trim());
        }
        catch (Exception ex)
        {
            // §D.3：失败静默降级，绝不把异常抛给调用方
            return (-1, $"{ex.GetType().Name}: {ex.Message}");
        }
    }

    /// <summary>
    /// 失败原因文本：保留**全部**输出的首部（而非只取第一行）。
    ///
    /// 实测（2026-09-23）：`/Create` 失败时 schtasks 会先打印一条无害的
    /// `警告: 因为 /ST 早于当前的时间…`，真正的失败信息在其后；只取第一行
    /// 会让日志与界面把这条警告当成失败原因，排障时看不到 `exit=1` 的真实语义。
    /// 这里改为紧凑拼接所有非空行。
    /// </summary>
    private static string Compact(string s, int maxLen = 400)
    {
        if (string.IsNullOrWhiteSpace(s)) return "(无输出)";
        var parts = s
            .Split(NewLineChars, StringSplitOptions.RemoveEmptyEntries)
            .Select(line => line.Trim())
            .Where(line => line.Length > 0);
        string joined = string.Join(" | ", parts);
        return joined.Length <= maxLen ? joined : joined[..maxLen] + "…";
    }

    /// <summary>换行字符（复用静态实例，避免每次调用分配数组）。</summary>
    private static readonly char[] NewLineChars = { '\r', '\n' };
}
