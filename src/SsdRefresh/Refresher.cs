using System.ComponentModel;
using System.Diagnostics;
using System.Text.RegularExpressions;
using Microsoft.Win32.SafeHandles;

namespace SsdRefresh;

/// <summary>磁碟區叢集點陣圖的本機副本，用來找空的叢集。1 = 使用中。</summary>
public sealed class ClusterMap
{
    readonly byte[] _b;
    readonly long _n;
    public long Cursor;

    public ClusterMap(byte[] bits, long totalClusters) { _b = bits; _n = Math.Min(totalClusters, (long)bits.Length * 8); }

    public bool Used(long l) => l >= _n || ((_b[l >> 3] >> (int)(l & 7)) & 1) != 0;

    public void Mark(long lcn, long count)
    {
        for (long l = Math.Max(0, lcn), e = Math.Min(_n, lcn + count); l < e; l++) _b[l >> 3] |= (byte)(1 << (int)(l & 7));
    }

    public long CountFree()
    {
        long used = 0;
        for (long l = 0; l < _n; l++) if (Used(l)) used++;
        return _n - used;
    }

    /// <summary>
    /// 從游標往後找（到底再從頭）第一段至少 need 個空叢集；找不到就回傳最大一段（至少 minAccept）。
    /// </summary>
    public bool Find(long need, long minAccept, out long start, out long len)
    {
        long best = -1, bestLen = 0;
        foreach (var (from, to) in new[] { (Cursor, _n), (0L, Math.Min(Cursor, _n)) })
        {
            long run = -1;
            long i = from;
            while (i < to)
            {
                if ((i & 63) == 0 && i + 64 <= to)
                {
                    ulong w = BitConverter.ToUInt64(_b, (int)(i >> 3));
                    if (w == ulong.MaxValue)
                    {
                        if (run >= 0 && i - run > bestLen) { best = run; bestLen = i - run; }
                        run = -1; i += 64; continue;
                    }
                    if (w == 0)
                    {
                        if (run < 0) run = i;
                        i += 64;
                        if (i - run >= need) { start = run; len = need; Cursor = start + need; return true; }
                        continue;
                    }
                }
                if (!Used(i))
                {
                    if (run < 0) run = i;
                    if (i + 1 - run >= need) { start = run; len = need; Cursor = start + need; return true; }
                }
                else
                {
                    if (run >= 0 && i - run > bestLen) { best = run; bestLen = i - run; }
                    run = -1;
                }
                i++;
            }
            if (run >= 0 && to - run > bestLen) { best = run; bestLen = to - run; }
        }
        start = best; len = bestLen;
        if (bestLen >= minAccept && bestLen > 0) { Cursor = best + bestLen; return true; }
        return false;
    }
}

public sealed class RefreshOptions
{
    public string From = "";
    public string Out = "";
    public bool SelectAll;
    public bool Force;
    public string? Confirm;
    public bool AllowSystemVolume;
    public bool Resume;
    public bool DryRun;
    public int StopAfter;            // 測試用：處理 N 個檔案後停止，模擬中斷
    public int MaxChunkMB = 16;
    public ReadOptions Read = new();
}

public sealed class MismatchException : Exception { public MismatchException(string m) : base(m) { } }

public sealed class Refresher
{
    public static readonly string[] Header =
    {
        "path", "size", "status", "before_mbps", "after_mbps", "before_slow", "after_slow", "moved_clusters", "moves",
        "move_failures", "extents_before", "extents_after", "writers", "meta_changed", "sha_before", "sha_after", "detail",
    };

    // 這些狀態代表檔案已處理完，續跑時跳過
    static readonly HashSet<string> Final = new() { "OK", "CHANGED", "SKIP_FAST", "SKIP_KIND", "SKIP_MISSING", "ERROR", "MOVE_FAILED", "DRYRUN" };

    readonly RefreshOptions _o;
    readonly VolumeInfo _vol;
    readonly CancellationTokenSource _cts;
    SafeFileHandle? _volHandle;
    bool _volWritable;
    ClusterMap? _map;
    long _mftZoneStart, _mftZoneEnd;
    long _totalMoves;
    Progress? _prog;
    PauseControl _pause = new(null);

    Refresher(RefreshOptions o, VolumeInfo vol, CancellationTokenSource cts) { _o = o; _vol = vol; _cts = cts; }

    public static int Run(RefreshOptions o, CancellationTokenSource cts)
    {
        // ---------- 讀掃描報告、選檔 ----------
        var rows = CsvReader.ReadAll(o.From);
        var picked = rows.Where(r => r.GetValueOrDefault("status") == "OK" && (o.SelectAll || r.GetValueOrDefault("slow") == "1"))
                         .Where(r => Enum.TryParse<FileKind>(r["kind"], out var k) && FileKinds.Movable(k))
                         .Select(r => (Path: r["path"], Size: long.Parse(r["size"]))).ToList();
        if (picked.Count == 0) { Console.WriteLine("報告裡沒有要刷新的檔案。"); return 0; }

        var vol = Native.GetVolume(picked[0].Path);
        var others = picked.Where(p => !p.Path.StartsWith(vol.Root, StringComparison.OrdinalIgnoreCase)).ToList();
        if (others.Count > 0) { Console.Error.WriteLine($"報告裡有 {others.Count} 個檔案不在 {vol.Root}，一次只能刷新一個磁碟區。"); return 1; }
        if (!string.Equals(vol.FileSystem, "NTFS", StringComparison.OrdinalIgnoreCase)) { Console.Error.WriteLine($"只支援 NTFS，這個磁碟區是 {vol.FileSystem}。"); return 1; }

        // ---------- 權限與安全檢查 ----------
        if (!o.DryRun && !Native.IsAdmin())
        {
            Console.Error.WriteLine("【需要系統管理員權限】刷新要搬移叢集，請用「以系統管理員身分執行」開 PowerShell 再執行。");
            return 2;
        }
        if (Native.IsAdmin()) Native.EnablePrivileges();
        string sysDrive = (Environment.GetEnvironmentVariable("SystemDrive") ?? "C:").ToUpperInvariant();
        bool isSystem = vol.Letter == sysDrive;
        if (isSystem && !o.DryRun && !o.AllowSystemVolume)
        {
            Console.Error.WriteLine($"{vol.Letter} 是系統磁碟。要刷新系統磁碟必須另外加 --allow-system-volume。");
            return 1;
        }

        var done = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        if (o.Resume && File.Exists(o.Out))
        {
            foreach (var r in CsvReader.ReadAll(o.Out))
                if (Final.Contains(r["status"])) done.Add(r["path"]);
        }
        else if (File.Exists(o.Out)) { Console.Error.WriteLine($"記錄檔已存在：{o.Out}（要續跑請加 --resume）"); return 1; }
        var todo = picked.Where(p => !done.Contains(p.Path)).ToList();
        long todoBytes = todo.Sum(p => p.Size);

        Console.WriteLine($"磁碟區：{vol.Root}（{vol.FileSystem}，叢集 {vol.ClusterSize} 位元組{(isSystem ? "，系統磁碟" : "")}）");
        Console.WriteLine($"要處理：{todo.Count} 個檔案，{Progress.Fmt(todoBytes)}（已完成 {done.Count} 個）");
        Console.WriteLine($"空間：剩 {Progress.Fmt(vol.FreeClusters * vol.ClusterSize)}");
        if (!o.DryRun)
        {
            int shadows = CountShadowCopies(vol);
            if (shadows > 0)
                Console.WriteLine($"注意：{vol.Letter} 有 {shadows} 份陰影複製（還原點）。搬移叢集會讓陰影複製的差異區變大，舊的還原點可能被自動刪除。");
            else if (shadows < 0)
                Console.WriteLine("注意：無法查詢陰影複製（vssadmin 失敗）。");
            // ---------- 二次確認 ----------
            string want = vol.Letter.TrimEnd(':').ToUpperInvariant();
            string? got = o.Confirm?.Trim().TrimEnd(':', '\\').ToUpperInvariant();
            if (got == null)
            {
                Console.Write($"刷新會把上面的檔案搬到新的叢集。請輸入磁碟代號「{want}」確認，其他輸入取消：");
                got = Console.ReadLine()?.Trim().TrimEnd(':').ToUpperInvariant();
            }
            if (got != want) { Console.WriteLine("已取消，沒有做任何變更。"); return 1; }
        }

        var r2 = new Refresher(o, vol, cts);
        int code = 0;
        var t = new Thread(() => code = r2.Loop(todo, todoBytes)) { Name = "refresh" };
        t.Start();
        t.Join();
        return code;
    }

    static int CountShadowCopies(VolumeInfo vol)
    {
        try
        {
            var psi = new ProcessStartInfo("vssadmin", $"list shadows /for={vol.Letter}\\")
            { RedirectStandardOutput = true, RedirectStandardError = true, UseShellExecute = false, CreateNoWindow = true };
            using var p = Process.Start(psi)!;
            string s = p.StandardOutput.ReadToEnd();
            p.WaitForExit();
            return Regex.Matches(s, @"HarddiskVolumeShadowCopy\d+").Count;
        }
        catch { return -1; }
    }

    int Loop(List<(string Path, long Size)> todo, long todoBytes)
    {
        if (_o.Read.IoPriority < 2) Native.SetThreadBackground(true);
        try
        {
            if (!_o.DryRun) OpenVolume();
            _pause = new PauseControl(_o.Out + ".pause");
            using var csv = new CsvWriter(_o.Out, Header, append: _o.Resume);
            _prog = new Progress(_o.DryRun ? "試跑" : "刷新", todo.Count, todoBytes, _pause);
            KeyWatcher.Start(_pause, _cts, _prog.Log);
            var counts = new Dictionary<string, int>();
            int processed = 0, code = 0;
            foreach (var (path, size) in todo)
            {
                if (_cts.IsCancellationRequested) { code = 4; break; }
                _pause.WaitAsync(_cts.Token).ContinueWith(_ => { }).Wait();
                if (_cts.IsCancellationRequested) { code = 4; break; }
                var row = ProcessFile(path);
                csv.WriteRow(row.Values);
                counts[row.Status] = counts.GetValueOrDefault(row.Status) + 1;
                _prog.AddFile(size, row.Status is "OK" or "CHANGED" && row.AfterSlow, row.Status is "ERROR" or "MOVE_FAILED");
                if (row.Status == "MISMATCH")
                {
                    _prog.Log($"【停止】雜湊不一致而且找不到合理原因：{path}");
                    code = 3; break;
                }
                if (_o.StopAfter > 0 && ++processed >= _o.StopAfter) { _prog.Log($"已處理 {processed} 個檔案（--stop-after），停止。"); code = 4; break; }
            }
            _prog.Dispose();
            Console.WriteLine("結果：" + string.Join("、", counts.OrderBy(k => k.Key).Select(k => $"{k.Key} {k.Value}")));
            if (code == 4) Console.WriteLine("已中斷。用同樣的指令加 --resume 可以續跑。");
            if (!_o.DryRun) Console.WriteLine("建議做完後執行 Optimize-Volume -DriveLetter X -ReTrim（系統管理員），讓 SSD 回收舊位置。");
            return code;
        }
        finally
        {
            _volHandle?.Dispose();
            if (_o.Read.IoPriority < 2) Native.SetThreadBackground(false);
        }
    }

    void OpenVolume()
    {
        _volHandle?.Dispose();
        _volHandle = Native.OpenVolume(_vol, _volWritable);
        if (_o.Read.IoPriority < 2) Native.SetIoPriority(_volHandle, _o.Read.IoPriority);
        var nd = Native.GetNtfsData(_volHandle);
        _mftZoneStart = nd.MftZoneStart;
        _mftZoneEnd = nd.MftZoneEnd;
        ReloadBitmap(nd.TotalClusters);
    }

    void ReloadBitmap(long total = 0)
    {
        if (total == 0) total = Native.GetNtfsData(_volHandle!).TotalClusters;
        long cursor = _map?.Cursor ?? 0;
        _map = new ClusterMap(Native.GetVolumeBitmap(_volHandle!, total), total);
        _map.Mark(_mftZoneStart, _mftZoneEnd - _mftZoneStart); // 不搬進 MFT 保留區
        _map.Cursor = cursor;
    }

    sealed class Row
    {
        public string Path = "", Status = "", ShaBefore = "", ShaAfter = "", Detail = "";
        public long Size, MovedClusters, Moves, Failures, ExtBefore, ExtAfter;
        public double BeforeMBps, AfterMBps;
        public bool BeforeSlow, AfterSlow, Writers, MetaChanged;
        public object?[] Values => new object?[]
        {
            Path, Size, Status, BeforeMBps.ToString("0.0"), AfterMBps.ToString("0.0"), BeforeSlow ? 1 : 0, AfterSlow ? 1 : 0,
            MovedClusters, Moves, Failures, ExtBefore, ExtAfter, Writers ? 1 : 0, MetaChanged ? 1 : 0, ShaBefore, ShaAfter, Detail,
        };
    }

    Row ProcessFile(string path)
    {
        var row = new Row { Path = path };
        try
        {
            if (!File.Exists(path)) { row.Status = "SKIP_MISSING"; return row; }
            using var meta = Native.OpenMeta(path);
            var m1 = Native.GetMeta(meta);
            row.Size = m1.Size;
            var ex1 = (m1.Attributes & Native.FILE_ATTRIBUTE_REPARSE) != 0 ? new List<Extent>() : Native.GetExtents(meta);
            var kind = FileKinds.Classify(m1, ex1);
            if (!FileKinds.Movable(kind)) { row.Status = "SKIP_KIND"; row.Detail = kind.ToString(); return row; }
            row.ExtBefore = ex1.Count(e => e.OnDisk);
            bool writers1 = Native.HasOtherWriters(path);

            using var data = Native.OpenUnbufferedRead(path);
            Native.SetIoPriority(data, _o.Read.MeasurePriority);
            var pre = Read(data, m1.Size, ex1);
            row.BeforeMBps = pre.MBps;
            row.BeforeSlow = pre.IsSlow;
            row.ShaBefore = FileKinds.Hex(pre.Sha256);
            if (!_o.Force && !pre.IsSlow) { row.Status = "SKIP_FAST"; return row; }
            if (_o.DryRun) { row.Status = "DRYRUN"; return row; }

            bool partial = false;
            try { MoveFile(meta, ex1, row, ref partial); }
            catch (Exception ex) when (ex is not OperationCanceledException)
            {
                row.Status = "MOVE_FAILED";
                row.Detail = FileKinds.ErrText(ex);
                // 搬到一半也要驗證，下面照常做
            }

            var m2 = Native.GetMeta(meta);
            var ex2 = Native.GetExtents(meta);
            row.ExtAfter = ex2.Count(e => e.OnDisk);
            var post = Read(data, m2.Size, ex2);
            row.AfterMBps = post.MBps;
            row.AfterSlow = post.IsSlow;
            row.ShaAfter = FileKinds.Hex(post.Sha256);
            row.Writers = writers1 || Native.HasOtherWriters(path);
            row.MetaChanged = m1.Size != m2.Size || m1.LastWrite != m2.LastWrite || m1.ChangeTime != m2.ChangeTime || m1.Usn != m2.Usn;
            bool explained = row.Writers || row.MetaChanged || pre.ShortRead || post.ShortRead;

            if (row.ShaBefore != row.ShaAfter && !explained)
            {
                row.Status = "MISMATCH";
                row.Detail = "搬移前後雜湊不同，沒有偵測到其他程式寫入";
                return row;
            }
            if (row.Status == "MOVE_FAILED") return row;
            if (partial) { row.Status = "PARTIAL"; return row; }
            row.Status = row.ShaBefore == row.ShaAfter ? "OK" : "CHANGED";
            if (row.Status == "CHANGED") row.Detail = "搬移期間檔案被其他程式修改（屬正常）";
        }
        catch (OperationCanceledException) { row.Status = "PARTIAL"; }
        catch (Exception ex)
        {
            row.Status = "ERROR";
            row.Detail = FileKinds.ErrText(ex);
        }
        return row;
    }

    ReadResult Read(SafeFileHandle data, long size, List<Extent> ex) =>
        FileReader.ReadAsync(data, size, ex, _vol.ClusterSize, measure: true, hash: true, _o.Read, _pause, CancellationToken.None)
                  .GetAwaiter().GetResult();

    /// <summary>
    /// 依 VCN 順序把檔案的每個 extent 分段搬到空的叢集。每一次 FSCTL_MOVE_FILE 由檔案系統保證不可分割：
    /// 要嘛整段搬好，要嘛完全沒動，所以中途失敗或中斷，檔案內容都是完整的。
    /// </summary>
    void MoveFile(SafeFileHandle meta, List<Extent> extents, Row row, ref bool partial)
    {
        long maxChunk = Math.Max(1, (long)_o.MaxChunkMB * 1024 * 1024 / _vol.ClusterSize);
        long minRun = Math.Max(1, Math.Min(maxChunk, 1024 * 1024 / _vol.ClusterSize)); // 優先找至少 1 MB 的空位
        long end = extents.Where(e => e.OnDisk).Select(e => e.Vcn + e.Clusters).DefaultIfEmpty(0).Max();
        var cur = extents;
        long v = 0;
        int failures = 0;
        while (v < end)
        {
            if (_cts.IsCancellationRequested) { partial = true; return; }
            if (_pause.IsPaused) _pause.WaitAsync(_cts.Token).ContinueWith(_ => { }).Wait();
            if (_cts.IsCancellationRequested) { partial = true; return; }

            int idx = cur.FindIndex(x => x.Vcn <= v && v < x.Vcn + x.Clusters);
            if (idx < 0)
            {
                var nxt = cur.Where(x => x.Vcn > v).Select(x => x.Vcn).DefaultIfEmpty(-1).Min();
                if (nxt < 0) break; // 檔案在搬移途中變小了
                v = nxt; continue;
            }
            var e = cur[idx];
            if (!e.OnDisk) { v = e.Vcn + e.Clusters; continue; }
            long want = Math.Min(e.Vcn + e.Clusters - v, maxChunk);
            if (!_map!.Find(want, Math.Min(want, minRun), out long lcn, out long len) &&
                !_map.Find(want, 1, out lcn, out len))
                throw new IOException("磁碟區沒有足夠的空叢集");
            uint cnt = (uint)Math.Min(len, want);
            int err = Native.MoveClusters(_volHandle!, meta, v, lcn, cnt);
            if (err == 0)
            {
                _map.Mark(lcn, cnt);
                v += cnt;
                row.MovedClusters += cnt;
                row.Moves++;
                _totalMoves++;
                continue;
            }

            // 失敗：目標位置可能剛被別人用掉，或檔案在途中被改。重新讀點陣圖與 extent 後再試。
            failures++;
            row.Failures = failures;
            _map.Mark(lcn, cnt);
            if (err == Native.ERROR_ACCESS_DENIED && _totalMoves == 0 && !_volWritable)
            {
                _prog?.Log("唯讀的磁碟區 handle 搬移被拒，改用讀寫 handle 重開（不會直接寫入磁區）。");
                _volWritable = true;
                OpenVolume();
            }
            if (failures > 10) throw new Win32Exception(err, $"搬移失敗太多次（VCN {v}）：{new Win32Exception(err).Message}");
            if (failures % 3 == 0) ReloadBitmap();
            cur = Native.GetExtents(meta);
        }
    }
}
