// Tier2SnapshotTests.cs —— ADR-0001 D3：tier-2 的单写者快照模型。
//
// 守护的 BUG（2026-09-24 全量审查）：
//
//   `GET /api/v1/tier2` 由 HTTP 线程直接调用 `BuildLocalTier2Payload()`，
//   而采集线程（CollectionLoop，每 `--interval` 一轮）也在调用同一个方法。
//   该方法进入的 `LhmCollector.ReadTier2()` 全程无锁，却要读 `_computer`，
//   而采集线程的 `Close()` 会在持 `_lock` 时把 `_computer` 置 null；
//   `DiskManager._lastValidMap` 是**无锁** Dictionary，在同一调用内既读又写；
//   `_tier2LastError` / `_tier2LastSuccess` 同样被两条线程写。
//
//   并发后果：字典并发写可能丢条目或损坏内部结构；对正在被 Close 的 LHM 对象
//   并发访问会抛异常并被 catch 吞掉，表现为 tier-2 数据间歇性为 null。
//
// 修复：采集线程把负载作为不可变值发布到 `_tier2Snapshot`（`PublishTier2Snapshot`），
// HTTP 线程只调 `ReadTier2Snapshot()` 读引用后序列化——**不再进入采集路径**。
//
// 断言范围（源码文本，不启动 HTTP）：
//   A. `ReadTier2Snapshot` / `PublishTier2Snapshot` 存在且由 `SnapshotLock` 保护；
//   B. 采集线程发布快照，HTTP 分支读快照；
//   C. 生产代码里 `BuildLocalTier2Payload()` 只出现在采集线程分支（一条调用），
//      不出现在 HTTP 处理分支内 —— 这是本 ADR 的核心不变量。

using Xunit;

namespace LhmSidecar.Tests;

public class Tier2SnapshotTests
{
    private static string ProgramSource()
    {
        // 测试程序集与 sidecar 源码同仓；用 AppContext.BaseDirectory 上溯到仓库根。
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null && !File.Exists(Path.Combine(dir.FullName, "sidecar-lhm", "Program.cs")))
        {
            dir = dir.Parent;
        }
        Assert.NotNull(dir);
        var path = Path.Combine(dir!.FullName, "sidecar-lhm", "Program.cs");
        return File.ReadAllText(path);
    }

    /// <summary>A：读写访问器存在，且都由 SnapshotLock 保护。</summary>
    [Fact]
    public void 读写访问器均由快照锁保护()
    {
        var src = ProgramSource();
        Assert.Contains("private static Tier2Payload? _tier2Snapshot;", src);
        Assert.Contains("private static void PublishTier2Snapshot(Tier2Payload? payload)", src);
        Assert.Contains("internal static Tier2Payload? ReadTier2Snapshot()", src);

        // 两个访问器体内都应有 lock (SnapshotLock)（防止后续被改成裸字段访问）
        var publish = src.IndexOf("private static void PublishTier2Snapshot", StringComparison.Ordinal);
        var read = src.IndexOf("internal static Tier2Payload? ReadTier2Snapshot", StringComparison.Ordinal);
        Assert.True(publish > 0 && read > publish, "两个访问器应按声明顺序出现");
        var segment = src.Substring(publish, read - publish);
        Assert.Contains("lock (SnapshotLock)", segment);
        var readBody = src.Substring(read, Math.Min(400, src.Length - read));
        Assert.Contains("lock (SnapshotLock)", readBody);
    }

    /// <summary>B：采集循环在与整份快照同批的位置发布 tier-2 快照。</summary>
    [Fact]
    public void 采集线程发布tier2快照()
    {
        var src = ProgramSource();
        Assert.Contains("PublishTier2Snapshot(payload);", src);
        // 发布必须先于（或紧邻）整份快照的赋值，保证两者同批
        var publishAt = src.IndexOf("PublishTier2Snapshot(payload);", StringComparison.Ordinal);
        var snapshotAssign = src.IndexOf("_snapshot = snap;", StringComparison.Ordinal);
        Assert.True(publishAt > 0 && snapshotAssign > publishAt,
            "tier-2 快照应与整份快照同批发布，避免读到错配组合");
    }

    /// <summary>
    /// C：核心不变量——`BuildLocalTier2Payload()` 是**采集专用**的，
    /// 生产代码中只允许出现一处调用（采集循环内），HTTP 分支不得调用。
    /// </summary>
    [Fact]
    public void 采集函数只被采集线程调用()
    {
        var src = ProgramSource();
        // 去掉注释行后再计数，避免文档里提到的名字造成误判
        var codeLines = src
            .Split('\n')
            .Where(l => !l.TrimStart().StartsWith("//", StringComparison.Ordinal))
            .Where(l => !l.TrimStart().StartsWith("///", StringComparison.Ordinal))
            .ToArray();
        var code = string.Join("\n", codeLines);

        var calls = 0;
        var idx = 0;
        while ((idx = code.IndexOf("BuildLocalTier2Payload(", idx, StringComparison.Ordinal)) >= 0)
        {
            calls++;
            idx += "BuildLocalTier2Payload(".Length;
        }
        // 一次定义（方法签名）+ 一次调用（采集分支）= 2
        Assert.Equal(2, calls);

        // HTTP 分支必须走快照读取
        var httpCase = code.IndexOf("case \"/api/v1/tier2\":", StringComparison.Ordinal);
        Assert.True(httpCase > 0, "应存在 /api/v1/tier2 路由");
        var httpSegment = code.Substring(httpCase, Math.Min(1200, code.Length - httpCase));
        Assert.Contains("ReadTier2Snapshot()", httpSegment);
        Assert.DoesNotContain("BuildLocalTier2Payload()", httpSegment);
    }
}
