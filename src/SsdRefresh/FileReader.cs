using System.Buffers;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using Microsoft.Win32.SafeHandles;

namespace SsdRefresh;

/// <summary>4096 位元組對齊的原生記憶體。FILE_FLAG_NO_BUFFERING 要求緩衝區對齊磁區大小。</summary>
public sealed unsafe class AlignedBuffer : MemoryManager<byte>
{
    readonly byte* _p;
    readonly int _len;
    public AlignedBuffer(int len) { _len = len; _p = (byte*)NativeMemory.AlignedAlloc((nuint)len, 4096); }
    public override Span<byte> GetSpan() => new(_p, _len);
    public override MemoryHandle Pin(int elementIndex = 0) => new(_p + elementIndex);
    public override void Unpin() { }
    protected override void Dispose(bool disposing) => NativeMemory.AlignedFree(_p);
}

public sealed class ReadOptions
{
    public int ChunkBytes = 1 << 20;     // 每個 I/O 的大小
    public int QueueDepth = 4;           // 同時排隊的 I/O 數
    public double ThresholdMBps = 100;   // 低於這個速度算慢（MB = 10^6 位元組）
    public double AllowanceMs = 2;       // 每個 extent 容許的額外延遲，避免小檔因延遲被誤判
    public int IoPriority = 0;           // 大量讀取與搬移：0 = VeryLow, 1 = Low, 2 = Normal
    public int MeasurePriority = 2;      // 確認重讀、刷新前後測速。VeryLow 會被系統壓速，測出來不準
}

public sealed class ExtentTiming
{
    public long Offset, Bytes, Lcn;
    public double Seconds;
    public bool Slow;
    public double MBps => Seconds > 0 ? Bytes / 1e6 / Seconds : double.PositiveInfinity;
}

public sealed class ReadResult
{
    public long DiskBytes;          // 實際從磁碟讀的位元組（不含稀疏區、常駐資料）
    public double DiskSeconds;      // 讀取這些位元組花的時間（扣掉暫停）
    public long BytesHashed;
    public bool ShortRead;          // 讀到的比預期少（檔案在讀取途中變小）
    public byte[]? Sha256;
    public List<ExtentTiming> Extents = new();
    public double MBps => DiskSeconds > 0 ? DiskBytes / 1e6 / DiskSeconds : 0;
    public long SlowBytes => Extents.Where(e => e.Slow).Sum(e => e.Bytes);
    public bool IsSlow => Extents.Any(e => e.Slow);
    public double MinExtentMBps => Extents.Count == 0 ? 0 : Extents.Min(e => e.MBps);
}

public static class FileReader
{
    record struct Seg(long Offset, long Length, int Extent); // Extent = -1 表示不測速的區段

    static long AlignUp(long v, long a) => (v + a - 1) / a * a;

    /// <summary>
    /// 依 extent 順序讀完整個檔案。同時排 QueueDepth 個 I/O，依序計算雜湊，並算出每個 extent 的讀取時間。
    /// extents 為空時（常駐檔、壓縮檔）整個檔案當成一段，不判定快慢。
    /// </summary>
    public static async Task<ReadResult> ReadAsync(SafeFileHandle h, long size, IReadOnlyList<Extent> extents, int clusterSize,
        bool measure, bool hash, ReadOptions opt, PauseControl pause, CancellationToken ct)
    {
        var result = new ReadResult();
        long end = AlignUp(size, 4096);
        var segs = BuildSegments(extents, clusterSize, end, measure, result);

        // 切成 chunk
        var chunks = new List<Seg>();
        foreach (var s in segs)
            for (long o = 0; o < s.Length; o += opt.ChunkBytes)
                chunks.Add(new Seg(s.Offset + o, Math.Min(opt.ChunkBytes, s.Length - o), s.Extent));

        using var hasher = hash ? IncrementalHash.CreateHash(HashAlgorithmName.SHA256) : null;
        int qd = Math.Max(1, Math.Min(opt.QueueDepth, chunks.Count));
        var bufs = new AlignedBuffer[qd];
        for (int i = 0; i < qd; i++) bufs[i] = new AlignedBuffer(opt.ChunkBytes);
        var inflight = new Task<(int n, long ts)>[qd];
        var issueTs = new long[qd];
        var firstIssue = new long[result.Extents.Count];
        var lastDone = new long[result.Extents.Count];
        var pausedTicks = new long[result.Extents.Count];
        Array.Fill(firstIssue, -1);

        try
        {
            int next = 0;   // 下一個要送出的 chunk
            int done = 0;   // 下一個要收的 chunk
            while (done < chunks.Count)
            {
                // 補滿佇列
                while (next < chunks.Count && next - done < qd)
                {
                    if (pause.IsPaused && next == done)
                    {
                        long t0 = Stopwatch.GetTimestamp();
                        await pause.WaitAsync(ct);
                        int ex = chunks[next].Extent;
                        if (ex >= 0 && firstIssue[ex] >= 0) pausedTicks[ex] += Stopwatch.GetTimestamp() - t0;
                    }
                    if (pause.IsPaused) break; // 先把排隊中的收完再暫停
                    ct.ThrowIfCancellationRequested();
                    var c = chunks[next];
                    int slot = next % qd;
                    long ts = Stopwatch.GetTimestamp();
                    if (c.Extent >= 0 && firstIssue[c.Extent] < 0) firstIssue[c.Extent] = ts;
                    inflight[slot] = ReadOne(h, bufs[slot].Memory.Slice(0, (int)AlignUp(c.Length, 4096)), c.Offset, ct);
                    next++;
                }

                var cd = chunks[done];
                int s2 = done % qd;
                var (n, doneTs) = await inflight[s2];
                if (cd.Extent >= 0)
                {
                    lastDone[cd.Extent] = Math.Max(lastDone[cd.Extent], doneTs);
                    result.DiskBytes += Math.Min(n, cd.Length);
                }
                long want = Math.Min(cd.Length, size - cd.Offset);
                if (want > 0)
                {
                    if (n < want) result.ShortRead = true;
                    int use = (int)Math.Min(n, want);
                    hasher?.AppendData(bufs[s2].Memory.Span.Slice(0, use));
                    result.BytesHashed += use;
                }
                done++;
            }
        }
        finally
        {
            // 取消時要等所有 I/O 結束才能釋放緩衝區
            foreach (var t in inflight) if (t != null) try { await t; } catch { }
            foreach (var b in bufs) ((IDisposable)b).Dispose();
        }

        // 依序把時間分給每個 extent：從「它的第一個 I/O 送出」或「前一個 extent 讀完」較晚者，到它最後一個 I/O 完成。
        long prev = 0;
        double tick = Stopwatch.Frequency;
        for (int i = 0; i < result.Extents.Count; i++)
        {
            var e = result.Extents[i];
            if (firstIssue[i] < 0) continue;
            long begin = Math.Max(firstIssue[i], prev);
            long t = Math.Max(0, lastDone[i] - begin - pausedTicks[i]);
            e.Seconds = t / tick;
            prev = lastDone[i];
            result.DiskSeconds += e.Seconds;
            e.Slow = measure && e.Seconds > e.Bytes / (opt.ThresholdMBps * 1e6) + opt.AllowanceMs / 1000.0;
        }
        result.Sha256 = hasher?.GetHashAndReset();
        return result;
    }

    /// <summary>兩次讀同一個檔案：每個 extent 取比較快的那次，重新判定快慢。</summary>
    public static void KeepFaster(ReadResult a, ReadResult b, ReadOptions opt)
    {
        if (a.Extents.Count != b.Extents.Count) return;
        a.DiskSeconds = 0;
        for (int i = 0; i < a.Extents.Count; i++)
        {
            var e = a.Extents[i];
            e.Seconds = Math.Min(e.Seconds, b.Extents[i].Seconds);
            e.Slow = e.Seconds > e.Bytes / (opt.ThresholdMBps * 1e6) + opt.AllowanceMs / 1000.0;
            a.DiskSeconds += e.Seconds;
        }
    }

    static List<Seg> BuildSegments(IReadOnlyList<Extent> extents, int clusterSize, long end, bool measure, ReadResult result)
    {
        var segs = new List<Seg>();
        long pos = 0;
        foreach (var x in extents)
        {
            long a = x.Vcn * clusterSize, b = Math.Min((x.Vcn + x.Clusters) * clusterSize, end);
            if (b <= a || a >= end) continue;
            if (a > pos) segs.Add(new Seg(pos, a - pos, -1)); // 空洞
            int idx = -1;
            if (x.OnDisk && measure)
            {
                idx = result.Extents.Count;
                result.Extents.Add(new ExtentTiming { Offset = a, Bytes = b - a, Lcn = x.Lcn });
            }
            segs.Add(new Seg(a, b - a, idx));
            pos = b;
        }
        if (pos < end) segs.Add(new Seg(pos, end - pos, -1));
        return segs;
    }

    static async Task<(int, long)> ReadOne(SafeFileHandle h, Memory<byte> m, long off, CancellationToken ct)
    {
        int total = 0;
        while (total < m.Length)
        {
            int n = await RandomAccess.ReadAsync(h, m.Slice(total), off + total, ct);
            if (n == 0) break;
            total += n;
            if (n % 4096 != 0) break; // 檔尾
        }
        return (total, Stopwatch.GetTimestamp());
    }
}
