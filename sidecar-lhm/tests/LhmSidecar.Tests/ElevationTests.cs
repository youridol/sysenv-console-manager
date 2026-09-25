// ElevationTests.cs —— 按需提权 + 持久授权（两阶段权限模型）的行为。
//
// 契约 §1 / CHANGE-CONTRACT-v2.md §D 硬性要求：
//   * 启动**不弹 UAC**；提权只在 POST /api/v1/elevate（或已授权的计划任务 /Run）时发生。
//   * **管理员环境零点击**：已是管理员 → 自动启用 tier-2（AutoAuthorizeOnce）。
//   * **一次授权，冷启动免 UAC**：任务已注册 → 后台 /Run；任务不存在 → 静默跳过，
//     **绝不**自动 runas（§D.3 硬要求）。
//   * UAC 被拒绝是**非致命且被记住**的：第二次调用不得再尝试弹窗。
//
// 真实 UAC 弹窗与真实 schtasks 都不在单测中执行：通过 Program/Tier2Task 的
// internal 接缝注入（IsElevatedForTesting / ElevationLauncherForTesting /
// Tier2ProbeForTesting / Tier2Task.ExecutorForTesting）。

using System.ComponentModel;
using LhmSidecar;
using LhmSidecar.Elevation;
using Xunit;

namespace LhmSidecar.Tests;

/// <summary>
/// 提权测试共享同一个进程内的静态状态，必须串行执行（禁用并行）。
/// </summary>
[CollectionDefinition("Elevation", DisableParallelization = true)]
public class ElevationCollection { }

[Collection("Elevation")]
public class ElevationTests : IDisposable
{
    private static readonly object Gate = new();

    public void Dispose()
    {
        // 恢复生产路径，避免影响其他测试
        Program.IsElevatedForTesting = null;
        Program.ElevationLauncherForTesting = null;
        Program.Tier2ProbeForTesting = null;
        Program.ElevationWaitMs = 8000;
        Tier2Task.ExecutorForTesting = null;
        Tier2Task.SetExistsCacheForTesting(null);
        Program.ResetElevationStateForTesting();
    }

    /// <summary>把 schtasks 执行器替换为"任务不存在"（/Query 失败）。</summary>
    private static void SimulateTaskAbsent()
    {
        Tier2Task.SetExistsCacheForTesting(null);
        Tier2Task.ExecutorForTesting = (args, _) => args[0] == "/Query"
            ? (1, "ERROR: The system cannot find the file specified.")
            : (0, string.Empty);
    }

    /// <summary>把 schtasks 执行器替换为"任务已注册"，返回调用记录。</summary>
    private static List<IReadOnlyList<string>> SimulateTaskPresent()
    {
        var calls = new List<IReadOnlyList<string>>();
        Tier2Task.SetExistsCacheForTesting(null);
        Tier2Task.ExecutorForTesting = (args, _) =>
        {
            calls.Add(args.ToList());
            return (0, string.Empty);
        };
        return calls;
    }

    // ========================================================================
    // 用户投诉 2：管理员环境 → 零点击自动启用
    // ========================================================================

    [Fact]
    public void AutoAuthorize_WhenAlreadyAdmin_EnablesTier2WithZeroClicks()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => true;
            SimulateTaskAbsent();   // 即便任务不存在也必须自动启用

            var (elevated, reason) = Program.AutoAuthorizeOnce();

            Assert.True(elevated);
            Assert.True(Program.LocalTier2EnabledForTesting);   // ← 用户投诉 2 的修复点
            Assert.Contains("管理员", reason);
            Assert.False(Program.AutoAuthorizeSkippedForTesting);
        }
    }

    [Fact]
    public void AutoAuthorize_AdminPath_EnablesTier2WithoutRunningATask()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => true;
            var calls = SimulateTaskPresent();

            Program.AutoAuthorizeOnce();

            // 管理员直通靠本进程读 tier-2，**不需要** /Run 任何任务
            Assert.DoesNotContain(calls, c => c[0] == "/Run");

            // 但会顺便建立持久授权（/Create）：这样管理员首次运行之后，
            // 即便将来以普通用户身份冷启动，也能免 UAC 恢复 tier-2（§D.2）。
            Assert.Contains(calls, c => c[0] == "/Create");
        }
    }

    [Fact]
    public void AutoAuthorize_AdminPath_StillEnablesTier2_WhenRegistrationFails()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => true;
            // 组策略禁用计划任务：注册失败不得影响管理员直通
            Tier2Task.SetExistsCacheForTesting(null);
            Tier2Task.ExecutorForTesting = (_, _) => (1, "ERROR: Access is denied.");

            var (elevated, _) = Program.AutoAuthorizeOnce();

            Assert.True(elevated);
            Assert.True(Program.LocalTier2EnabledForTesting);
        }
    }

    // ========================================================================
    // 硬要求 §D.3：未授权 → 自动路径绝不 runas
    // ========================================================================

    [Fact]
    public void AutoAuthorize_WhenTaskAbsent_SkipsAndNeverRunAs()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => false;
            SimulateTaskAbsent();

            int runAsLaunches = 0;
            Program.ElevationLauncherForTesting = (_, _) => { runAsLaunches++; return null; };

            var (elevated, reason) = Program.AutoAuthorizeOnce();

            Assert.False(elevated);
            Assert.False(Program.LocalTier2EnabledForTesting);
            Assert.True(Program.AutoAuthorizeSkippedForTesting);
            Assert.Equal(0, runAsLaunches);          // 关键：绝不弹 UAC
            Assert.Contains("绝不", reason);
        }
    }

    // ========================================================================
    // 用户投诉 1：授权一次 → 冷启动免 UAC（走计划任务）
    // ========================================================================

    [Fact]
    public void AutoAuthorize_WhenTaskPresent_RunsTaskOnly_AndNeverRunAs()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => false;
            var calls = SimulateTaskPresent();

            // tier-2 首次探测即就绪（模拟伴生进程被任务拉起）
            Program.Tier2ProbeForTesting = () => true;
            Program.ElevationWaitMs = 2000;

            int runAsLaunches = 0;
            Program.ElevationLauncherForTesting = (_, _) => { runAsLaunches++; return null; };

            var (elevated, reason) = Program.AutoAuthorizeOnce();

            Assert.True(elevated);
            Assert.Contains("无 UAC", reason);
            Assert.Equal(0, runAsLaunches);                  // 没有 runas
            Assert.Contains(calls, c => c[0] == "/Run");     // 走了计划任务
        }
    }

    // ========================================================================
    // RequestElevation：先任务（免 UAC），后 runas（弹 UAC）
    // ========================================================================

    [Fact]
    public void RequestElevation_PrefersScheduledTask_WhenTier2BecomesReady()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => false;
            var calls = SimulateTaskPresent();
            Program.Tier2ProbeForTesting = () => true;
            Program.ElevationWaitMs = 2000;

            int runAsLaunches = 0;
            Program.ElevationLauncherForTesting = (_, _) => { runAsLaunches++; return null; };

            var (ok, reason) = Program.RequestElevation();

            Assert.True(ok);
            Assert.Contains("计划任务", reason);
            Assert.Equal(0, runAsLaunches);
            Assert.Contains(calls, c => c[0] == "/Run");
        }
    }

    [Fact]
    public void RequestElevation_FallsBackToRunAs_WhenTaskPresentButNoTier2InTime()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => false;
            SimulateTaskPresent();
            Program.Tier2ProbeForTesting = () => false;   // 任务拉起的进程始终没产出 tier-2
            Program.ElevationWaitMs = 400;                // 有界等待（§D.2 生产值 8s）

            int runAsLaunches = 0;
            Program.ElevationLauncherForTesting = (_, args) =>
            {
                runAsLaunches++;
                Assert.Contains("--role=elevated", args);  // 回退路径仍是伴生进程角色
                return null;
            };

            var (ok, reason) = Program.RequestElevation();

            Assert.True(ok);
            Assert.Equal(1, runAsLaunches);                // 回退到 runas
            Assert.Contains("UAC", reason);
        }
    }

    [Fact]
    public void RequestElevation_FallsBackToRunAs_WhenTaskAbsent()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => false;
            SimulateTaskAbsent();

            int runAsLaunches = 0;
            Program.ElevationLauncherForTesting = (_, _) => { runAsLaunches++; return null; };

            var (ok, _) = Program.RequestElevation();

            Assert.True(ok);
            Assert.Equal(1, runAsLaunches);   // 用户点击时才允许 runas
        }
    }

    // ========================================================================
    // 既有行为回归
    // ========================================================================

    // --- 已是管理员：不启动伴生进程，直接由本进程提供 tier-2 ---
    [Fact]
    public void AlreadyElevated_EnablesLocalTier2WithoutLaunchingCompanion()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => true;

            int launches = 0;
            Program.ElevationLauncherForTesting = (_, _) => { launches++; return null; };

            var (ok, reason) = Program.RequestElevation();

            Assert.True(ok);
            Assert.Equal(0, launches);                       // 不拉伴生进程
            Assert.False(Program.ElevationRefusedForTesting);
            Assert.Contains("管理员", reason);
        }
    }

    // --- UAC 被取消（Win32 1223）：非致命 + 被记住 ---
    [Fact]
    public void UacCancelled_IsNonFatalAndRemembered()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => false;
            SimulateTaskAbsent();

            int launches = 0;
            Program.ElevationLauncherForTesting = (_, _) =>
            {
                launches++;
                throw new Win32Exception(1223);   // ERROR_CANCELLED
            };

            var (ok1, reason1) = Program.RequestElevation();

            Assert.False(ok1);                                // 非致命：返回失败而不是抛异常
            Assert.Equal(1, launches);
            Assert.True(Program.ElevationRefusedForTesting);  // 状态被记住
            Assert.Contains("1223", reason1);

            // 第二次调用：**绝不**再次弹窗
            var (ok2, reason2) = Program.RequestElevation();

            Assert.False(ok2);
            Assert.Equal(1, launches);                        // 启动次数没有增加
            Assert.Contains("取消", reason2);
        }
    }

    // --- 提权成功：记录伴生进程 PID ---
    [Fact]
    public void SuccessfulLaunch_RecordsCompanionPid()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => false;
            SimulateTaskAbsent();
            Program.ElevationLauncherForTesting = (_, _) =>
            {
                // 起一个真实存在但无害的进程作为"伴生进程"占位
                var psi = new System.Diagnostics.ProcessStartInfo
                {
                    FileName = "cmd.exe",
                    Arguments = "/c exit 0",
                    CreateNoWindow = true,
                    UseShellExecute = false
                };
                var p = System.Diagnostics.Process.Start(psi);
                p!.WaitForExit(3000);
                return p;
            };

            var (ok, reason) = Program.RequestElevation();

            Assert.True(ok);
            Assert.True(Program.CompanionPidForTesting > 0);
            Assert.Contains("已启动", reason);
        }
    }

    // --- 非 1223 的启动失败：同样被记住且给出可读原因 ---
    [Fact]
    public void NonCancelLaunchFailure_IsRememberedWithErrorMessage()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => false;
            SimulateTaskAbsent();
            Program.ElevationLauncherForTesting = (_, _) => throw new Win32Exception(5);  // ACCESS_DENIED

            var (ok, reason) = Program.RequestElevation();

            Assert.False(ok);
            Assert.True(Program.ElevationRefusedForTesting);
            Assert.Contains("0x00000005", reason);
        }
    }

    // --- 伴生进程已就绪后再调用：不重复启动 ---
    [Fact]
    public void RepeatedCall_AfterCompanionAlreadyRunning_DoesNotRelaunch()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => false;
            SimulateTaskAbsent();
            int launches = 0;
            Program.ElevationLauncherForTesting = (_, _) =>
            {
                launches++;
                var p = System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo
                {
                    FileName = "cmd.exe",
                    Arguments = "/c exit 0",
                    CreateNoWindow = true,
                    UseShellExecute = false
                });
                p!.WaitForExit(3000);
                return p;
            };

            Assert.True(Program.RequestElevation().ok);
            Assert.Equal(1, launches);

            // 第二次：伴生进程已存在 → 不重复启动（避免每次轮询弹窗）
            Program.RequestElevation();
            Assert.Equal(1, launches);
        }
    }

    // ========================================================================
    // 持久授权任务的端口推导（回归：主进程端口不得写成伴生进程自己的端口）
    // ========================================================================

    [Fact]
    public void DerivePrimaryPort_PrimaryRole_UsesOwnPort()
    {
        // 主进程：任务动作必须是 <主进程 port>+1，即 46001+1
        Assert.Equal(46001, Program.DerivePrimaryPort(46001, elevatedRole: false));
        Assert.Equal(46002, Program.DerivePrimaryPort(46001, elevatedRole: false) + 1);
    }

    [Fact]
    public void DerivePrimaryPort_ElevatedRole_SubtractsOne()
    {
        // 伴生进程：--port 已是主进程 port+1 → 反推主进程端口后，任务端口仍是同一个
        Assert.Equal(46001, Program.DerivePrimaryPort(46002, elevatedRole: true));
        Assert.Equal(46002, Program.DerivePrimaryPort(46002, elevatedRole: true) + 1);
    }

    [Fact]
    public void DerivePrimaryPort_DoesNotUnderflow()
    {
        // 防御：端口 1 / 0 时不应算出 0 或负数
        Assert.Equal(1, Program.DerivePrimaryPort(1, elevatedRole: true));
        Assert.Equal(0, Program.DerivePrimaryPort(0, elevatedRole: true));
    }

    // ========================================================================
    // 撤销（§D.2/§D.4）
    // ========================================================================

    [Fact]
    public void Revoke_WhenElevated_DeletesTaskAndResetsState()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => true;
            var calls = SimulateTaskPresent();

            var (revoked, reason) = Program.RevokeElevation();

            Assert.True(revoked);
            Assert.Contains("已删除", reason);
            Assert.Contains(calls, c => c[0] == "/Delete");
            // 复位：本会话不再提供 tier-2
            Assert.False(Program.LocalTier2EnabledForTesting);
            Assert.True(Program.ElevationRefusedForTesting);
        }
    }

    [Fact]
    public void Revoke_WhenNotElevated_StillDisablesSessionTier2()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => false;
            SimulateTaskAbsent();   // 也无伴生进程可代删

            var (_, reason) = Program.RevokeElevation();

            Assert.False(Program.LocalTier2EnabledForTesting);   // 用户意图 = 收回授权
            Assert.True(Program.ElevationRefusedForTesting);
            Assert.Contains("未完成", reason);                   // 如实报告删除未完成
        }
    }

    // ========================================================================
    // TryAutoElevate：零 UAC 自动路径（进页面即尝试，不等待用户授权）
    // ========================================================================

    [Fact]
    public void TryAutoElevate_WhenAlreadyAdmin_EnablesTier2WithoutAnyRunAs()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => true;
            SimulateTaskAbsent();

            int runAsLaunches = 0;
            Program.ElevationLauncherForTesting = (_, _) => { runAsLaunches++; return null; };

            var (ok, reason) = Program.TryAutoElevate();

            Assert.True(ok);
            Assert.True(Program.LocalTier2EnabledForTesting);
            Assert.Equal(0, runAsLaunches);   // 关键：自动路径绝不弹 UAC
            Assert.Contains("管理员", reason);
        }
    }

    [Fact]
    public void TryAutoElevate_WhenTaskAbsent_ReportsInfeasibleAndNeverRunAs()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => false;
            SimulateTaskAbsent();

            int runAsLaunches = 0;
            Program.ElevationLauncherForTesting = (_, _) => { runAsLaunches++; return null; };

            var (ok, reason) = Program.TryAutoElevate();

            // 返回 false = 自动路径不可行 → 前端据此才申请一次用户 UAC
            Assert.False(ok);
            Assert.False(Program.LocalTier2EnabledForTesting);
            Assert.Equal(0, runAsLaunches);   // 关键：自动入口绝不自行弹 UAC
            Assert.Contains("用户", reason);
        }
    }

    [Fact]
    public void TryAutoElevate_WhenTaskPresent_RunsTaskAndNeverRunAs()
    {
        lock (Gate)
        {
            Program.ResetElevationStateForTesting();
            Program.IsElevatedForTesting = () => false;
            var calls = SimulateTaskPresent();
            Program.Tier2ProbeForTesting = () => true;
            Program.ElevationWaitMs = 2000;

            int runAsLaunches = 0;
            Program.ElevationLauncherForTesting = (_, _) => { runAsLaunches++; return null; };

            var (ok, reason) = Program.TryAutoElevate();

            Assert.True(ok);
            Assert.Equal(0, runAsLaunches);
            Assert.Contains(calls, c => c[0] == "/Run");
            Assert.Contains("无 UAC", reason);
        }
    }
}
