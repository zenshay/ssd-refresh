using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace SsdRefresh;

/// <summary>一段連續的叢集：從 Vcn 開始的 Clusters 個叢集，放在 Lcn。Lcn = -1 表示沒有配置（稀疏或壓縮）。</summary>
public readonly record struct Extent(long Vcn, long Clusters, long Lcn)
{
    public bool OnDisk => Lcn >= 0;
}

public sealed class VolumeInfo
{
    public string Root = "";          // 例如 "C:\"
    public string DevicePath = "";    // 例如 "\\.\C:"
    public string FileSystem = "";
    public uint SerialNumber;
    public int ClusterSize;
    public long TotalClusters;
    public long FreeClusters;
    public string Letter => Root.Length >= 2 && Root[1] == ':' ? Root.Substring(0, 2).ToUpperInvariant() : Root;
}

public struct FileMeta
{
    public uint Attributes;
    public long Size;
    public long LastWrite;
    public long ChangeTime;
    public uint VolumeSerial;
    public ulong FileIndex;
    public uint Links;
    public uint ReparseTag;
    public long Usn;
}

public static unsafe class Native
{
    public const uint GENERIC_READ = 0x80000000, GENERIC_WRITE = 0x40000000;
    public const uint FILE_READ_DATA = 0x1, FILE_READ_ATTRIBUTES = 0x80, SYNCHRONIZE = 0x100000;
    public const uint FILE_SHARE_READ = 1, FILE_SHARE_WRITE = 2, FILE_SHARE_DELETE = 4;
    public const uint SHARE_ALL = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    public const uint OPEN_EXISTING = 3;
    public const uint FILE_FLAG_NO_BUFFERING = 0x20000000, FILE_FLAG_OVERLAPPED = 0x40000000;
    public const uint FILE_FLAG_BACKUP_SEMANTICS = 0x02000000, FILE_FLAG_OPEN_REPARSE_POINT = 0x00200000;

    public const uint FILE_ATTRIBUTE_DIRECTORY = 0x10, FILE_ATTRIBUTE_SPARSE = 0x200, FILE_ATTRIBUTE_REPARSE = 0x400;
    public const uint FILE_ATTRIBUTE_COMPRESSED = 0x800, FILE_ATTRIBUTE_ENCRYPTED = 0x4000;
    public const uint IO_REPARSE_TAG_WOF = 0x80000017;

    public const uint FSCTL_GET_NTFS_VOLUME_DATA = 0x00090064;
    public const uint FSCTL_GET_VOLUME_BITMAP = 0x0009006F;
    public const uint FSCTL_GET_RETRIEVAL_POINTERS = 0x00090073;
    public const uint FSCTL_MOVE_FILE = 0x00090074;
    public const uint FSCTL_READ_FILE_USN_DATA = 0x000900EB;

    public const int ERROR_ACCESS_DENIED = 5, ERROR_SHARING_VIOLATION = 32, ERROR_HANDLE_EOF = 38;
    public const int ERROR_MORE_DATA = 234, ERROR_LOCK_VIOLATION = 33;

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern SafeFileHandle CreateFileW(string name, uint access, uint share, IntPtr sa, uint disposition, uint flags, IntPtr template);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool DeviceIoControl(SafeFileHandle h, uint code, void* inBuf, int inSize, void* outBuf, int outSize, out int returned, IntPtr overlapped);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetFileInformationByHandle(SafeFileHandle h, out BY_HANDLE_FILE_INFORMATION info);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetFileInformationByHandleEx(SafeFileHandle h, int cls, void* buf, int size);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetFileInformationByHandle(SafeFileHandle h, int cls, void* buf, int size);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool GetVolumePathNameW(string path, char* buf, int len);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool GetVolumeNameForVolumeMountPointW(string mount, char* buf, int len);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool GetVolumeInformationW(string root, char* name, int nameLen, out uint serial, out uint maxComp, out uint flags, char* fs, int fsLen);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool GetDiskFreeSpaceW(string root, out uint secPerCluster, out uint bytesPerSector, out uint freeClusters, out uint totalClusters);

    [DllImport("kernel32.dll")]
    static extern IntPtr GetCurrentThread();

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetThreadPriority(IntPtr thread, int priority);

    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);

    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool LookupPrivilegeValueW(string? system, string name, out long luid);

    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool AdjustTokenPrivileges(IntPtr token, bool disableAll, void* newState, int len, IntPtr prev, IntPtr retLen);

    [DllImport("kernel32.dll")]
    static extern IntPtr GetCurrentProcess();

    [DllImport("kernel32.dll")]
    static extern bool CloseHandle(IntPtr h);

    [StructLayout(LayoutKind.Sequential)]
    struct BY_HANDLE_FILE_INFORMATION
    {
        public uint Attributes;
        public uint CreationLow, CreationHigh, AccessLow, AccessHigh, WriteLow, WriteHigh;
        public uint VolumeSerial, SizeHigh, SizeLow, Links, IndexHigh, IndexLow;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct MOVE_FILE_DATA
    {
        public IntPtr FileHandle;
        public long StartingVcn;
        public long StartingLcn;
        public uint ClusterCount;
    }

    // ---------- 開檔 ----------

    static SafeFileHandle Open(string path, uint access, uint share, uint flags)
    {
        var h = CreateFileW(LongPath(path), access, share, IntPtr.Zero, OPEN_EXISTING, flags | FILE_FLAG_BACKUP_SEMANTICS, IntPtr.Zero);
        if (h.IsInvalid)
        {
            int err = Marshal.GetLastWin32Error();
            h.Dispose();
            throw new Win32Exception(err);
        }
        return h;
    }

    /// <summary>只讀屬性的同步 handle。不檢查共用模式，所以別的程式鎖住的檔案也打得開。用來查 extent、搬移叢集。</summary>
    public static SafeFileHandle OpenMeta(string path) =>
        Open(path, FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_ALL, FILE_FLAG_OPEN_REPARSE_POINT);

    /// <summary>不經系統快取、可同時排多個 I/O 的讀取 handle。允許別人同時讀寫刪除。</summary>
    public static SafeFileHandle OpenUnbufferedRead(string path) =>
        Open(path, FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE, SHARE_ALL, FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED);

    /// <summary>
    /// 檢查是否有別的程式開著這個檔案的寫入權限：用「不允許別人寫」的共用模式開一下立刻關掉。
    /// 回傳 true 表示有人正在寫（或檔案被鎖）。
    /// </summary>
    public static bool HasOtherWriters(string path)
    {
        var h = CreateFileW(LongPath(path), FILE_READ_DATA, FILE_SHARE_READ | FILE_SHARE_DELETE, IntPtr.Zero, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, IntPtr.Zero);
        if (h.IsInvalid)
        {
            int err = Marshal.GetLastWin32Error();
            h.Dispose();
            return err == ERROR_SHARING_VIOLATION || err == ERROR_LOCK_VIOLATION;
        }
        h.Dispose();
        return false;
    }

    public static SafeFileHandle OpenVolume(VolumeInfo v, bool write) =>
        Open(v.DevicePath, GENERIC_READ | (write ? GENERIC_WRITE : 0), FILE_SHARE_READ | FILE_SHARE_WRITE, 0);

    public static string LongPath(string p)
    {
        if (p.StartsWith(@"\\?\") || p.StartsWith(@"\\.\")) return p;
        if (p.StartsWith(@"\\")) return @"\\?\UNC\" + p.Substring(2);
        return @"\\?\" + p;
    }

    // ---------- 檔案資訊 ----------

    public static FileMeta GetMeta(SafeFileHandle h)
    {
        if (!GetFileInformationByHandle(h, out var bi)) throw new Win32Exception();
        var m = new FileMeta
        {
            Attributes = bi.Attributes,
            Size = ((long)bi.SizeHigh << 32) | bi.SizeLow,
            VolumeSerial = bi.VolumeSerial,
            FileIndex = ((ulong)bi.IndexHigh << 32) | bi.IndexLow,
            Links = bi.Links,
        };
        // FILE_BASIC_INFO：Creation, LastAccess, LastWrite, ChangeTime, Attributes
        byte* basic = stackalloc byte[40];
        if (GetFileInformationByHandleEx(h, 0, basic, 40))
        {
            m.LastWrite = *(long*)(basic + 16);
            m.ChangeTime = *(long*)(basic + 24);
        }
        // FILE_ATTRIBUTE_TAG_INFO
        uint* tag = stackalloc uint[2];
        if (GetFileInformationByHandleEx(h, 9, tag, 8)) m.ReparseTag = tag[1];
        // USN（USN 日誌沒啟用時會失敗，維持 0）
        byte* usn = stackalloc byte[1024];
        if (DeviceIoControl(h, FSCTL_READ_FILE_USN_DATA, null, 0, usn, 1024, out _, IntPtr.Zero))
        {
            ushort major = *(ushort*)(usn + 4);
            m.Usn = major == 2 ? *(long*)(usn + 24) : *(long*)(usn + 40);
        }
        return m;
    }

    /// <summary>IoPriorityHintVeryLow = 0、Low = 1、Normal = 2。</summary>
    public static void SetIoPriority(SafeFileHandle h, int hint)
    {
        int v = hint;
        SetFileInformationByHandle(h, 12, &v, 4); // FileIoPriorityHintInfo；失敗不影響正確性
    }

    public static void SetThreadBackground(bool on) =>
        SetThreadPriority(GetCurrentThread(), on ? 0x00010000 : 0x00020000);

    // ---------- extent ----------

    public static List<Extent> GetExtents(SafeFileHandle h)
    {
        var list = new List<Extent>();
        long start = 0;
        const int size = 64 * 1024;
        byte* p = stackalloc byte[size];
        while (true)
        {
            long input = start;
            bool ok = DeviceIoControl(h, FSCTL_GET_RETRIEVAL_POINTERS, &input, 8, p, size, out _, IntPtr.Zero);
            int err = ok ? 0 : Marshal.GetLastWin32Error();
            if (!ok && err == ERROR_HANDLE_EOF) return list; // 沒有非常駐資料（小檔放在 MFT 裡，或空檔）
            if (!ok && err != ERROR_MORE_DATA) throw new Win32Exception(err);
            int count = *(int*)p;
            long vcn = *(long*)(p + 8);
            for (int i = 0; i < count; i++)
            {
                long next = *(long*)(p + 16 + i * 16);
                long lcn = *(long*)(p + 24 + i * 16);
                list.Add(new Extent(vcn, next - vcn, lcn));
                vcn = next;
            }
            if (ok || count == 0) return list;
            start = vcn;
        }
    }

    // ---------- 磁碟區 ----------

    public static VolumeInfo GetVolume(string path)
    {
        var full = Path.GetFullPath(path);
        char* buf = stackalloc char[1024];
        if (!GetVolumePathNameW(full, buf, 1024)) throw new Win32Exception();
        string root = new string(buf);
        var v = new VolumeInfo { Root = root };
        if (root.Length == 3 && root[1] == ':') v.DevicePath = @"\\.\" + root.Substring(0, 2);
        else
        {
            if (!GetVolumeNameForVolumeMountPointW(root, buf, 1024)) throw new Win32Exception();
            v.DevicePath = new string(buf).TrimEnd('\\');
        }
        char* fs = stackalloc char[64];
        if (!GetVolumeInformationW(root, null, 0, out v.SerialNumber, out _, out _, fs, 64)) throw new Win32Exception();
        v.FileSystem = new string(fs);
        if (!GetDiskFreeSpaceW(root, out uint spc, out uint bps, out uint free, out uint total)) throw new Win32Exception();
        v.ClusterSize = (int)(spc * bps);
        v.FreeClusters = free;
        v.TotalClusters = total;
        return v;
    }

    public struct NtfsData
    {
        public long TotalClusters, FreeClusters, MftStartLcn, MftZoneStart, MftZoneEnd;
        public int BytesPerSector, BytesPerCluster;
    }

    public static NtfsData GetNtfsData(SafeFileHandle vol)
    {
        byte* b = stackalloc byte[256];
        if (!DeviceIoControl(vol, FSCTL_GET_NTFS_VOLUME_DATA, null, 0, b, 256, out _, IntPtr.Zero)) throw new Win32Exception();
        return new NtfsData
        {
            TotalClusters = *(long*)(b + 16),
            FreeClusters = *(long*)(b + 24),
            BytesPerSector = *(int*)(b + 40),
            BytesPerCluster = *(int*)(b + 44),
            MftStartLcn = *(long*)(b + 64),
            MftZoneStart = *(long*)(b + 80),
            MftZoneEnd = *(long*)(b + 88),
        };
    }

    /// <summary>讀整個磁碟區的叢集點陣圖（1 = 使用中）。回傳的 bit 0 對應 LCN 0。</summary>
    public static byte[] GetVolumeBitmap(SafeFileHandle vol, long totalClusters)
    {
        long bytes = (totalClusters + 7) / 8 + 64;
        while (true)
        {
            var outBuf = new byte[16 + bytes];
            long start = 0;
            fixed (byte* p = outBuf)
            {
                bool ok = DeviceIoControl(vol, FSCTL_GET_VOLUME_BITMAP, &start, 8, p, outBuf.Length, out int ret, IntPtr.Zero);
                int err = ok ? 0 : Marshal.GetLastWin32Error();
                long startLcn = *(long*)p;
                long bitmapSize = *(long*)(p + 8);
                if (!ok && err == ERROR_MORE_DATA && (bitmapSize + 7) / 8 > bytes) { bytes = (bitmapSize + 7) / 8 + 64; continue; }
                if (!ok) throw new Win32Exception(err);
                if (startLcn != 0) throw new InvalidOperationException("點陣圖起點不是 LCN 0");
                var bits = new byte[(bitmapSize + 7) / 8];
                Array.Copy(outBuf, 16, bits, 0, bits.Length);
                return bits;
            }
        }
    }

    /// <summary>請檔案系統把 file 從 vcn 起的 count 個叢集搬到 lcn。成功回傳 0，否則回傳 Win32 錯誤碼。</summary>
    public static int MoveClusters(SafeFileHandle vol, SafeFileHandle file, long vcn, long lcn, uint count)
    {
        var d = new MOVE_FILE_DATA
        {
            FileHandle = file.DangerousGetHandle(),
            StartingVcn = vcn,
            StartingLcn = lcn,
            ClusterCount = count,
        };
        bool ok = DeviceIoControl(vol, FSCTL_MOVE_FILE, &d, sizeof(MOVE_FILE_DATA), null, 0, out _, IntPtr.Zero);
        return ok ? 0 : Marshal.GetLastWin32Error();
    }

    // ---------- 權限 ----------

    public static bool IsAdmin()
    {
        using var id = System.Security.Principal.WindowsIdentity.GetCurrent();
        return new System.Security.Principal.WindowsPrincipal(id).IsInRole(System.Security.Principal.WindowsBuiltInRole.Administrator);
    }

    /// <summary>啟用 SeBackupPrivilege（讀取 ACL 限制的檔案）與 SeManageVolumePrivilege（搬移叢集）。非管理員時無效果。</summary>
    public static void EnablePrivileges()
    {
        if (!OpenProcessToken(GetCurrentProcess(), 0x20 | 0x8, out var token)) return; // ADJUST_PRIVILEGES | QUERY
        try
        {
            byte* tp = stackalloc byte[16];
            foreach (var name in new[] { "SeBackupPrivilege", "SeManageVolumePrivilege" })
            {
                if (!LookupPrivilegeValueW(null, name, out long luid)) continue;
                *(int*)tp = 1;
                *(long*)(tp + 4) = luid;
                *(int*)(tp + 12) = 2; // SE_PRIVILEGE_ENABLED
                AdjustTokenPrivileges(token, false, tp, 16, IntPtr.Zero, IntPtr.Zero);
            }
        }
        finally { CloseHandle(token); }
    }
}
