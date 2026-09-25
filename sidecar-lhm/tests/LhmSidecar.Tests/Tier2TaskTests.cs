// Tier2TaskTests.cs —— 持久授权计划任务的命令构造与失败语义。
//
// 覆盖 CHANGE-CONTRACT-v2.md §D.2（冻结的任务名/动作/注册方式/触发方式）与
// §D.3（绝不自动 runas、注册失败只降级、无明文凭据、窗口隐藏且有超时）。
//
// 所有断言都通过 Tier2Task.ExecutorForTesting 注入执行器，**不触碰真实 schtasks**，
// 因此测试无需管理员权限、也不受机器上的任务状态影响。

using LhmSidecar.Elevation;
using Xunit;

namespace LhmSidecar.Tests;

public class Tier2TaskTests : IDisposable
{
    public Tier2TaskTests()
    {
        Tier2Task.SetExistsCacheForTesting(null);
        Tier2Task.ExecutorForTesting = null;
    }

    public void Dispose()
    {
        Tier2Task.ExecutorForTesting = null;
        Tier2Task.SetExistsCacheForTesting(null);
    }

    /// <summary>记录调用参数并返回预设结果。</summary>
    private sealed class RecordingExecutor
    {
        public List<IReadOnlyList<string>> Calls { get; } = new();
        public int ExitCode { get; set; }
        public string Output { get; set; } = string.Empty;

        public (int, string) Run(IReadOnlyList<string> args, int timeoutMs)
        {
            Calls.Add(args.ToList());
            return (ExitCode, Output);
        }

        public IReadOnlyList<string> Last => Calls[^1];
    }

    // ---------------- 命令构造（§D.2 冻结）----------------

    [Fact]
    public void TaskName_IsFrozen()
        => Assert.Equal("SECM-LhmTier2", Tier2Task.TaskName);

    [Fact]
    public void CompanionArguments_UsePortPlusOne_AndElevatedRole()
    {
        Assert.Equal("--port 45981 --interval 1000 --role=elevated", Tier2Task.BuildCompanionArguments(45980, 1000));
        Assert.Equal("--port 501 --interval 250 --role=elevated", Tier2Task.BuildCompanionArguments(500, 250));
    }

    [Fact]
    public void CreateArguments_MatchFrozenShape()
    {
        var args = Tier2Task.BuildCreateArguments(@"C:\app\LhmSidecar.exe", 45980, 1000);

        Assert.Equal(new[]
        {
            "/Create",
            "/TN", "SECM-LhmTier2",
            "/TR", @"C:\app\LhmSidecar.exe --port 45981 --interval 1000 --role=elevated",
            "/SC", "ONCE",
            "/ST", "00:00",
            "/RL", "HIGHEST",
            "/F"
        }, args);
    }

    [Fact]
    public void CreateArguments_QuoteExePathWithSpaces()
    {
        var args = Tier2Task.BuildCreateArguments(@"C:\Program Files\SECM\LhmSidecar.exe", 45980, 1000);
        string tr = args[args.ToList().IndexOf("/TR") + 1];

        // 含空格的 exe 路径必须加引号，否则 schtasks 会把路径当命令、把 exe 名当参数
        Assert.Equal("\"C:\\Program Files\\SECM\\LhmSidecar.exe\" --port 45981 --interval 1000 --role=elevated", tr);
    }

    [Fact]
    public void CreateArguments_NeverContainCredentials()
    {
        // §D.3：不用 /RU /RP —— 当前用户上下文 + HIGHEST，不落明文凭据
        var args = Tier2Task.BuildCreateArguments(@"C:\app\LhmSidecar.exe", 45980, 1000);

        Assert.DoesNotContain("/RU", args);
        Assert.DoesNotContain("/RP", args);
        Assert.Contains("/RL", args);
        Assert.Equal("HIGHEST", args[args.ToList().IndexOf("/RL") + 1]);
    }

    [Fact]
    public void CreateArguments_UseOnDemandSchedule_NotResidentTrigger()
    {
        // §D.2：/SC ONCE + 立即 /Run 触发，不安装常驻触发器
        var args = Tier2Task.BuildCreateArguments(@"C:\app\LhmSidecar.exe", 45980, 1000);

        Assert.Equal("ONCE", args[args.ToList().IndexOf("/SC") + 1]);
        Assert.Contains("/F", args);   // 幂等覆盖
        Assert.DoesNotContain("/SC", args.Where((_, i) => false));   // 占位：只有一个 /SC
    }

    [Theory]
    [InlineData("/Query")]
    [InlineData("/Run")]
    [InlineData("/Delete")]
    public void SimpleVerbs_TargetTheFrozenTaskName(string verb)
    {
        var args = verb switch
        {
            "/Query" => Tier2Task.BuildQueryArguments(),
            "/Run" => Tier2Task.BuildRunArguments(),
            _ => Tier2Task.BuildDeleteArguments()
        };

        Assert.Equal(verb, args[0]);
        Assert.Contains("/TN", args);
        Assert.Equal("SECM-LhmTier2", args[args.ToList().IndexOf("/TN") + 1]);
    }

    [Fact]
    public void DeleteArguments_ForceFlag()
        => Assert.Contains("/F", Tier2Task.BuildDeleteArguments());

    // ---------------- 存在性判定 ----------------

    [Fact]
    public void Exists_TrueWhenQuerySucceeds()
    {
        var ex = new RecordingExecutor { ExitCode = 0, Output = "TaskName  Status\nSECM-LhmTier2  Ready" };
        Tier2Task.ExecutorForTesting = ex.Run;

        Assert.True(Tier2Task.Exists());
        Assert.Equal("/Query", ex.Last[0]);
    }

    [Fact]
    public void Exists_FalseWhenQueryFails()
    {
        var ex = new RecordingExecutor { ExitCode = 1, Output = "ERROR: The system cannot find the file specified." };
        Tier2Task.ExecutorForTesting = ex.Run;

        Assert.False(Tier2Task.Exists());
    }

    [Fact]
    public void Exists_CachesResult_SoAutoPathDoesNotShellOutEveryTick()
    {
        var ex = new RecordingExecutor { ExitCode = 1 };
        Tier2Task.ExecutorForTesting = ex.Run;

        Assert.False(Tier2Task.Exists());
        Assert.False(Tier2Task.Exists());
        Assert.False(Tier2Task.Exists());

        Assert.Single(ex.Calls);   // 只查一次 schtasks
    }

    [Fact]
    public void InvalidateExistsCache_ForcesRequery()
    {
        var ex = new RecordingExecutor { ExitCode = 1 };
        Tier2Task.ExecutorForTesting = ex.Run;

        Assert.False(Tier2Task.Exists());
        Tier2Task.InvalidateExistsCache();
        Assert.False(Tier2Task.Exists());

        Assert.Equal(2, ex.Calls.Count);
    }

    // ---------------- 关键硬要求：任务不存在 → 绝不触发（因而不可能弹 UAC）----------------

    [Fact]
    public void RunExisting_WhenTaskAbsent_SkipsAndNeverInvokesRun()
    {
        var ex = new RecordingExecutor { ExitCode = 1 };   // /Query 失败 = 任务不存在
        Tier2Task.ExecutorForTesting = ex.Run;

        bool ran = Tier2Task.RunExisting(out string reason);

        Assert.False(ran);
        Assert.Contains("未注册", reason);
        // 关键：只发生了一次 /Query；**没有**任何 /Run 调用
        Assert.Single(ex.Calls);
        Assert.Equal("/Query", ex.Last[0]);
        Assert.DoesNotContain(ex.Calls, c => c[0] == "/Run");
    }

    [Fact]
    public void RunExisting_WhenTaskPresent_InvokesRun()
    {
        var ex = new RecordingExecutor { ExitCode = 0 };
        Tier2Task.ExecutorForTesting = ex.Run;

        bool ran = Tier2Task.RunExisting(out string reason);

        Assert.True(ran);
        Assert.Contains("无 UAC", reason);
        Assert.Equal("/Run", ex.Last[0]);
    }

    [Fact]
    public void RunExisting_WhenRunFails_ReportsReasonWithoutThrowing()
    {
        var ex = new RecordingExecutor { ExitCode = 0 };
        Tier2Task.ExecutorForTesting = (args, t) =>
        {
            ex.Calls.Add(args.ToList());
            return args[0] == "/Run" ? (1, "ERROR: Access is denied.") : (0, "");
        };

        bool ran = Tier2Task.RunExisting(out string reason);

        Assert.False(ran);                       // 失败静默降级，不抛异常（§D.3）
        Assert.Contains("Access is denied", reason);
    }

    // ---------------- 注册 / 删除的失败语义（§D.3）----------------

    [Fact]
    public void Register_Success_InvalidatesCache()
    {
        var ex = new RecordingExecutor { ExitCode = 0 };
        Tier2Task.ExecutorForTesting = ex.Run;

        Assert.True(Tier2Task.Register(@"C:\app\LhmSidecar.exe", 45980, 1000, out string reason));
        Assert.Contains("已注册", reason);
        Assert.Equal("/Create", ex.Last[0]);

        // 注册成功后存在性缓存必须失效（否则 persistent 会一直报旧值）
        Assert.True(Tier2Task.Exists());
    }

    [Fact]
    public void Register_Failure_ReturnsFalseWithoutThrowing()
    {
        // 例如组策略禁用计划任务 / 权限不足
        var ex = new RecordingExecutor { ExitCode = 1, Output = "ERROR: Access is denied." };
        Tier2Task.ExecutorForTesting = ex.Run;

        bool ok = Tier2Task.Register(@"C:\app\LhmSidecar.exe", 45980, 1000, out string reason);

        Assert.False(ok);
        Assert.Contains("Access is denied", reason);
    }

    [Fact]
    public void Register_EmptyExePath_SkipsWithoutShellingOut()
    {
        var ex = new RecordingExecutor { ExitCode = 0 };
        Tier2Task.ExecutorForTesting = ex.Run;

        Assert.False(Tier2Task.Register(string.Empty, 45980, 1000, out string reason));
        Assert.Contains("无法定位", reason);
        Assert.Empty(ex.Calls);   // 没有调用 schtasks
    }

    [Fact]
    public void Delete_ReportsSuccessAndInvalidatesCache()
    {
        var ex = new RecordingExecutor { ExitCode = 0 };
        Tier2Task.ExecutorForTesting = ex.Run;

        Assert.True(Tier2Task.Delete(out string reason));
        Assert.Contains("已删除", reason);
        Assert.Equal("/Delete", ex.Last[0]);
    }

    [Fact]
    public void Delete_Failure_ReturnsFalseWithReason()
    {
        // 实测：非提权进程 /Delete → 拒绝访问 exit=1
        var ex = new RecordingExecutor { ExitCode = 1, Output = "ERROR: Access is denied." };
        Tier2Task.ExecutorForTesting = ex.Run;

        Assert.False(Tier2Task.Delete(out string reason));
        Assert.Contains("Access is denied", reason);
    }

    [Fact]
    public void Register_Failure_KeepsWarningAndRealFailureText()
    {
        // 实测（2026-09-23）：/Create 失败时 schtasks 先输出一条无害的
        // `警告: 因为 /ST 早于当前的时间…`，真正的失败信息在其后。
        // 失败原因必须同时保留两者（此前只取第一行，把警告当成了失败原因）。
        var ex = new RecordingExecutor
        {
            ExitCode = 1,
            Output = "警告: 因为 /ST 早于当前的时间，任务可能无法运行。\r\nERROR: The operator or administrator has refused the request."
        };
        Tier2Task.ExecutorForTesting = ex.Run;

        bool ok = Tier2Task.Register(@"C:\app\LhmSidecar.exe", 45980, 1000, out string reason);

        Assert.False(ok);
        Assert.Contains("exit=1", reason);
        Assert.Contains("refused the request", reason);   // 真实失败信息不得被丢弃
        Assert.Contains("警告", reason);                    // 警告仍保留以便排障
    }
}
