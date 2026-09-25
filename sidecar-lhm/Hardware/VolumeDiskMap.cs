// Hardware/VolumeDiskMap.cs
//
// 卷名（"C:"）↔ 物理盘号（LHM Storage 节点 Identifier 末段，如 "/nvme/6" → 6）的映射。
//
// 为什么需要这一层：
//   上游 LiteMonitor 用盘符启发式把 DISK.C.Used 之类的键映射到磁盘硬件
//   （DiskManager.cs:157：盘名或传感器名包含 "C:"）。但 LHM 0.9.6 的 Storage 节点
//   名称是磁盘型号（"WDS100T3X0C-00SJG0"），**不含盘符**，该启发式在本机实测恒不命中，
//   磁盘温度（契约 §3 disks[].temperature_c，tier-2）会永远为 null。
//
// 本文件的映射是**如实**的、可解释的：
//   1. IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS 问系统"这个卷落在哪些物理盘上"（卷的第一个
//      extent 的 DiskNumber）——这是 Windows 官方卷映射接口，不靠命名猜测；
//   2. LHM Storage 节点 Identifier 的末段数字即物理盘号（实测：Identifier=/nvme/6 对应
//      Win32_DiskDrive Index=6，/ssd/5 对应 5，/ssd/1 对应 1）；无法解析出数字的节点被跳过。
//   任一步失败 → 该卷温度保持 null（绝不把某个盘的温度安到别的卷上，也不伪造 0）。

using System.Runtime.InteropServices;

namespace LhmSidecar.Hardware;

/// <summary>卷 → 物理盘号映射（Win32 卷 extent 查询）。</summary>
public static class VolumeDiskMap
{
    private const uint GENERIC_READ = 0x8000_0000;
    private const uint FILE_SHARE_READ = 0x00000001;
    private const uint FILE_SHARE_WRITE = 0x00000002;
    private const uint OPEN_EXISTING = 3;

    /// <summary>IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS（winioctl.h）。</summary>
    private const uint IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS = 0x00560000;

    private const int ERROR_INSUFFICIENT_BUFFER = 122;

    [StructLayout(LayoutKind.Sequential)]
    private struct DISK_EXTENT
    {
        public uint DiskNumber;
        public long StartingOffset;
        public long ExtentLength;
    }

    /// <summary>
    /// VOLUME_DISK_EXTENTS 的固定头部 + 第一个 extent。
    /// 注意 DISK_EXTENT 含 long 字段 → 按 8 字节对齐，因此 NumberOfDiskExtents（4 字节）
    /// 之后有 4 字节填充，第一个 extent 从偏移 8 开始（实测 br=32 = 4+4+24）。
    /// 用 PtrToStructure 而不是手算偏移，避免再犯这个对齐错误。
    /// </summary>
    [StructLayout(LayoutKind.Sequential)]
    private struct VOLUME_DISK_EXTENTS_HEAD
    {
        public uint NumberOfDiskExtents;
        private uint _padding;
        public DISK_EXTENT FirstExtent;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateFileW(string lpFileName, uint dwDesiredAccess, uint dwShareMode,
        IntPtr lpSecurityAttributes, uint dwCreationDisposition, uint dwFlagsAndAttributes,
        IntPtr hTemplateFile);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DeviceIoControl(IntPtr hDevice, uint dwIoControlCode,
        IntPtr lpInBuffer, uint nInBufferSize,
        IntPtr lpOutBuffer, uint nOutBufferSize,
        out uint lpBytesReturned, IntPtr lpOverlapped);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool CloseHandle(IntPtr hObject);

    /// <summary>缓存卷 → 盘号（卷映射是静态信息，避免每轮采集重复查询）。</summary>
    private static readonly Dictionary<string, uint?> _cache = new(StringComparer.OrdinalIgnoreCase);

    /// <summary>
    /// 查询卷（"C:"）所在的物理盘号；查询不到返回 null。
    /// 多 extent（Storage Spaces 等）取第一个 extent 的盘号。
    /// </summary>
    public static uint? GetPhysicalDiskNumber(string volume)
    {
        lock (_cache)
        {
            if (_cache.TryGetValue(volume, out var cached)) return cached;
        }

        uint? result = Query(volume);

        lock (_cache)
        {
            _cache[volume] = result;
        }
        return result;
    }

    /// <summary>清空缓存（卷布局变化时可用）。</summary>
    public static void ClearCache()
    {
        lock (_cache) _cache.Clear();
    }

    private static uint? Query(string volume)
    {
        IntPtr h = IntPtr.Zero;
        IntPtr buffer = IntPtr.Zero;
        try
        {
            // 卷句柄：\\.\C:
            h = CreateFileW(@"\\.\" + volume, 0, FILE_SHARE_READ | FILE_SHARE_WRITE,
                IntPtr.Zero, OPEN_EXISTING, 0, IntPtr.Zero);
            if (h == new IntPtr(-1) || h == IntPtr.Zero) return null;

            // 先按 1 个 extent 申请，缓冲不足（ERROR_INSUFFICIENT_BUFFER）时按返回的
            // 实际需要量重试一次（固定上限，避免无限增长）。
            int size = Marshal.SizeOf<VOLUME_DISK_EXTENTS_HEAD>();
            for (int attempt = 0; attempt < 3; attempt++)
            {
                buffer = Marshal.AllocHGlobal(size);
                bool ok = DeviceIoControl(h, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
                    IntPtr.Zero, 0, buffer, (uint)size, out _, IntPtr.Zero);

                if (ok)
                {
                    var head = Marshal.PtrToStructure<VOLUME_DISK_EXTENTS_HEAD>(buffer);
                    if (head.NumberOfDiskExtents == 0) return null;
                    // 第一个 extent 的 DiskNumber（多 extent 如 Storage Spaces 取第一个）
                    return head.FirstExtent.DiskNumber;
                }

                if (Marshal.GetLastWin32Error() != ERROR_INSUFFICIENT_BUFFER) return null;

                Marshal.FreeHGlobal(buffer);
                buffer = IntPtr.Zero;
                size *= 4;   // 放大缓冲重试
            }
            return null;
        }
        catch
        {
            return null;
        }
        finally
        {
            if (buffer != IntPtr.Zero) Marshal.FreeHGlobal(buffer);
            if (h != IntPtr.Zero && h != new IntPtr(-1)) CloseHandle(h);
        }
    }

    /// <summary>
    /// 从 LHM 硬件 Identifier 解析物理盘号。
    /// LHM 0.9.6 的 Storage 节点形如 "/nvme/6"、"/ssd/5"、"/hdd/2"（末段为盘号）。
    /// 解析不出数字 → null（跳过该节点，不做任何猜测）。
    /// </summary>
    public static uint? ParseDiskNumberFromIdentifier(string? identifier)
    {
        if (string.IsNullOrEmpty(identifier)) return null;

        int end = identifier.Length;
        int start = end;
        while (start > 0 && char.IsDigit(identifier[start - 1])) start--;
        if (start == end) return null;

        return uint.TryParse(identifier.AsSpan(start, end - start), out uint n) ? n : null;
    }
}
