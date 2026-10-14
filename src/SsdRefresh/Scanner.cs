using System.ComponentModel;
using System.IO.Enumeration;

namespace SsdRefresh;

public enum FileKind { Normal, Empty, Resident, Sparse, Compressed, Encrypted, Wof, Reparse }

public static class FileKinds
{
    public static FileKind Classify(FileMeta m, List<Extent> extents)
    {
        if ((m.Attributes & Native.FILE_ATTRIBUTE_REPARSE) != 0)
            return m.ReparseTag == Native.IO_REPARSE_TAG_WOF ? FileKind.Wof : FileKind.Reparse;
        if ((m.Attributes & Native.FILE_ATTRIBUTE_COMPRESSED) != 0) return FileKind.Compressed;
        if (m.Size == 0) return FileKind.Empty;
        if (!extents.Any(e => e.OnDisk)) return FileKind.Resident;
        if ((m.Attributes & Native.FILE_ATTRIBUTE_ENCRYPTED) != 0) return FileKind.Encrypted;
        if ((m.Attributes & Native.FILE_ATTRIBUTE_SPARSE) != 0) return FileKind.Sparse;
        return FileKind.Normal;
    }

    /// <summary>可以測速、也可以搬移的種類。</summary>
    public static bool Movable(FileKind k) => k is FileKind.Normal or FileKind.Sparse or FileKind.Encrypted;

    public static string Hex(byte[]? h) => h == null ? "" : Convert.ToHexString(h).ToLowerInvariant();

    public static string ErrText(Exception ex) => ex is Win32Exception w ? $"0x{w.NativeErrorCode:X}: {w.Message}" : ex.Message;
}

public sealed class ScanOptions
{
    public string Root = "";
    public string Out = "";
    public bool Hash;
    public bool Resume;
    public ReadOptions Read = new();
}

public static class Scanner
{
    public static readonly string[] Header =
        { "path", "size", "kind", "extents", "disk_bytes", "read_ms", "mbps", "min_extent_mbps", "slow", "slow_bytes", "confirmed", "sha256", "status", "error" };

    public static IEnumerable<(string Path, long Size)> Enumerate(string root)
    {
        root = Path.GetFullPath(root);
        if (File.Exists(root)) { yield return (root, new FileInfo(root).Length); yield break; }
        var opts = new EnumerationOptions
        {
            RecurseSubdirectories = true,
            IgnoreInaccessible = true,
            AttributesToSkip = 0,
            ReturnSpecialDirectories = false,
        };
        var e = new FileSystemEnumerable<(string, long)>(root, (ref FileSystemEntry en) => (en.ToFullPath(), en.Length), opts)
        {
            ShouldIncludePredicate = (ref FileSystemEntry en) => !en.IsDirectory,
            // 不進入 junction／符號連結資料夾，避免重複掃描或跑到別的磁碟
            ShouldRecursePredicate = (ref FileSystemEntry en) => (en.Attributes & FileAttributes.ReparsePoint) == 0,
        };
        foreach (var x in e) yield return x;
    }

    public static async Task<int> Run(ScanOptions o, CancellationTokenSource cts)
    {
        var vol = Native.GetVolume(o.Root);
        Console.WriteLine($"磁碟區 {vol.Root}（{vol.FileSystem}，叢集 {vol.ClusterSize} 位元組）");
        if (Native.IsAdmin()) Native.EnablePrivileges();
        else Console.WriteLine("提示：不是系統管理員，權限不足的檔案會記成 ERROR。");

        var done = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        if (o.Resume && File.Exists(o.Out))
        {
            foreach (var r in CsvReader.ReadAll(o.Out)) done.Add(r["path"]);
            Console.WriteLine($"續跑：已完成 {done.Count} 個檔案。");
        }
        else if (File.Exists(o.Out)) { Console.Error.WriteLine($"輸出檔已存在：{o.Out}（要續跑請加 --resume）"); return 1; }

        Console.WriteLine("列出檔案中…");
        var files = Enumerate(o.Root).Where(f => !done.Contains(f.Path)).ToList();
        long totalBytes = files.Sum(f => f.Size);
        Console.WriteLine($"要掃描 {files.Count} 個檔案，共 {Progress.Fmt(totalBytes)}。按 P 暫停，Q 停止。");

        var pause = new PauseControl(o.Out + ".pause");
        var csv = new CsvWriter(o.Out, Header, append: o.Resume);
        var seen = new HashSet<(uint, ulong)>();
        var prog = new Progress("掃描", files.Count, totalBytes, pause);
        KeyWatcher.Start(pause, cts, prog.Log);

        foreach (var (path, size) in files)
        {
            if (cts.IsCancellationRequested) break;
            await pause.WaitAsync(cts.Token).ContinueWith(_ => { });
            var row = await ScanOne(path, vol, o, seen, pause, cts.Token);
            if (row == null) break; // 中途取消，不寫這一列，續跑時重掃
            csv.WriteRow(row.Values);
            prog.AddFile(size, row.Slow, row.Status == "ERROR");
        }
        prog.Dispose();
        csv.Dispose();
        Console.WriteLine(cts.IsCancellationRequested ? "已停止。用同樣的指令加 --resume 可以續跑。" : "掃描完成。");
        PrintSummary(o.Out, o.Read.ThresholdMBps);
        return cts.IsCancellationRequested ? 4 : 0;
    }

    sealed class Row
    {
        public string Path = "", Kind = "", Sha = "", Status = "OK", Error = "";
        public long Size, Extents, DiskBytes, SlowBytes;
        public double ReadMs, MBps, MinMBps;
        public bool Slow, Confirmed;
        public object?[] Values => new object?[]
        {
            Path, Size, Kind, Extents, DiskBytes, ReadMs.ToString("0.###"), MBps.ToString("0.0"),
            double.IsInfinity(MinMBps) ? "" : MinMBps.ToString("0.0"), Slow ? 1 : 0, SlowBytes, Confirmed ? 1 : 0, Sha, Status, Error,
        };
    }

    static async Task<Row?> ScanOne(string path, VolumeInfo vol, ScanOptions o, HashSet<(uint, ulong)> seen, PauseControl pause, CancellationToken ct)
    {
        var row = new Row { Path = path };
        try
        {
            using var meta = Native.OpenMeta(path);
            var m = Native.GetMeta(meta);
            row.Size = m.Size;
            if (m.Links > 1 && !seen.Add((m.VolumeSerial, m.FileIndex))) { row.Status = "DUP_LINK"; return row; }
            var ex = (m.Attributes & Native.FILE_ATTRIBUTE_REPARSE) != 0 ? new List<Extent>() : Native.GetExtents(meta);
            var kind = FileKinds.Classify(m, ex);
            row.Kind = kind.ToString();
            row.Extents = ex.Count(e => e.OnDisk);
            if (!FileKinds.Movable(kind)) { row.Status = "SKIP"; return row; }

            using var data = Native.OpenUnbufferedRead(path);
            Native.SetIoPriority(data, o.Read.IoPriority);
            var r = await FileReader.ReadAsync(data, m.Size, ex, vol.ClusterSize, measure: true, hash: o.Hash, o.Read, pause, ct);
            if (r.IsSlow && o.Read.MeasurePriority != o.Read.IoPriority)
            {
                // 低優先權的 I/O 會被系統壓速，容易誤判；用測速優先權重讀一次，兩次都慢才算慢
                Native.SetIoPriority(data, o.Read.MeasurePriority);
                var r2 = await FileReader.ReadAsync(data, m.Size, ex, vol.ClusterSize, measure: true, hash: false, o.Read, pause, ct);
                Native.SetIoPriority(data, o.Read.IoPriority);
                FileReader.KeepFaster(r, r2, o.Read);
                row.Confirmed = true;
            }
            row.DiskBytes = r.DiskBytes;
            row.ReadMs = r.DiskSeconds * 1000;
            row.MBps = r.MBps;
            row.MinMBps = r.MinExtentMBps;
            row.Slow = r.IsSlow;
            row.SlowBytes = r.SlowBytes;
            row.Sha = FileKinds.Hex(r.Sha256);
        }
        catch (OperationCanceledException) { return null; }
        catch (Exception ex)
        {
            row.Status = "ERROR";
            row.Error = FileKinds.ErrText(ex);
        }
        return row;
    }

    public static void PrintSummary(string csvPath, double threshold)
    {
        var rows = CsvReader.ReadAll(csvPath);
        var ok = rows.Where(r => r["status"] == "OK").ToList();
        var slow = ok.Where(r => r["slow"] == "1").ToList();
        long slowBytes = slow.Sum(r => long.Parse(r["size"]));
        Console.WriteLine($"共 {rows.Count} 個檔案：測速 {ok.Count}、慢 {slow.Count}（{Progress.Fmt(slowBytes)}）、" +
                          $"跳過 {rows.Count(r => r["status"] is "SKIP" or "DUP_LINK")}、錯誤 {rows.Count(r => r["status"] == "ERROR")}。");
        Console.WriteLine($"門檻 {threshold} MB/s。最慢的 10 個（大於 1 MB）：");
        foreach (var r in ok.Where(r => long.Parse(r["disk_bytes"]) >= 1_000_000)
                     .OrderBy(r => double.Parse(r["mbps"])).Take(10))
            Console.WriteLine($"  {double.Parse(r["mbps"]),7:0.0} MB/s  {Progress.Fmt(long.Parse(r["size"])),6}  {r["path"]}");
    }
}
