// VolumeDiskMapTests.cs —— 卷 → 物理盘号映射的可解析部分。
//
// IOCTL 部分需要真实卷句柄（集成层面在验收脚本中验证：C:→6 / D:→1 / H:→5 / Y:→8 / Z:→7，
// 与 Win32_DiskDrive Index 一致）。这里覆盖纯解析逻辑与"解析不出就返回 null"的保守性。

using LhmSidecar.Hardware;
using Xunit;

namespace LhmSidecar.Tests;

public class VolumeDiskMapTests
{
    // LHM 0.9.6 Storage 节点的真实 Identifier 形态（实测）
    [Theory]
    [InlineData("/nvme/6", 6u)]
    [InlineData("/ssd/5", 5u)]
    [InlineData("/ssd/1", 1u)]
    [InlineData("/ssd/0", 0u)]
    [InlineData("/hdd/2", 2u)]
    [InlineData("/hdd/3", 3u)]
    public void ParseDiskNumberFromIdentifier_ParsesLhmStorageIdentifiers(string identifier, uint expected)
        => Assert.Equal(expected, VolumeDiskMap.ParseDiskNumberFromIdentifier(identifier));

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("/nvme/")]      // 末段无数字
    [InlineData("/storage")]    // 无数字
    [InlineData("no-digits-here")]
    public void ParseDiskNumberFromIdentifier_ReturnsNullWhenNotParseable(string? identifier)
        => Assert.Null(VolumeDiskMap.ParseDiskNumberFromIdentifier(identifier));

    [Fact]
    public void ParseDiskNumberFromIdentifier_HandlesMultiDigit()
        => Assert.Equal(123u, VolumeDiskMap.ParseDiskNumberFromIdentifier("/nvme/123"));
}
