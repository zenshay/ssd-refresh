using System.Text;
using SsdRefresh;

Console.OutputEncoding = new UTF8Encoding(false);
if (!Console.IsInputRedirected) Console.InputEncoding = new UTF8Encoding(false);

if (args.Length == 0 || args[0] is "-h" or "--help" or "help") { Usage(); return 0; }

var cts = new CancellationTokenSource();
Console.CancelKeyPress += (_, e) =>
{
    if (cts.IsCancellationRequested) return; // 第二次 Ctrl+C 直接結束；搬移是檔案系統的原子操作，強制結束也不會損壞檔案
    e.Cancel = true;
    Console.WriteLine();
    Console.WriteLine("收到 Ctrl+C：做完目前的檔案後停止（再按一次立刻結束）。");
    cts.Cancel();
};

try
{
    var a = new Args(args.Skip(1));
    switch (args[0])
    {
        case "scan":
        {
            var o = new ScanOptions
            {
                Root = a.Positional(0) ?? throw new ArgumentException("缺少要掃描的路徑"),
                Hash = a.Flag("--hash"),
                Resume = a.Flag("--resume"),
                Read = ReadOpts(a),
            };
            o.Out = Path.GetFullPath(a.Value("--out") ?? $"scan-{DateTime.Now:yyyyMMdd-HHmmss}.csv");
            a.EnsureAllUsed();
            Console.WriteLine($"報告：{o.Out}");
            return await Scanner.Run(o, cts);
        }
        case "refresh":
        {
            var o = new RefreshOptions
            {
                From = Path.GetFullPath(a.Value("--from") ?? throw new ArgumentException("缺少 --from <掃描報告.csv>")),
                SelectAll = (a.Value("--select") ?? "slow") switch { "slow" => false, "all" => true, var s => throw new ArgumentException($"--select 只能是 slow 或 all：{s}") },
                Force = a.Flag("--force"),
                Confirm = a.Value("--confirm"),
                AllowSystemVolume = a.Flag("--allow-system-volume"),
                Resume = a.Flag("--resume"),
                DryRun = a.Flag("--dry-run"),
                StopAfter = int.Parse(a.Value("--stop-after") ?? "0"),
                MaxChunkMB = int.Parse(a.Value("--max-chunk-mb") ?? "16"),
                Read = ReadOpts(a),
            };
            o.Out = Path.GetFullPath(a.Value("--out") ?? Path.ChangeExtension(o.From, null) + "-refresh.csv");
            a.EnsureAllUsed();
            Console.WriteLine($"記錄：{o.Out}");
            return Refresher.Run(o, cts);
        }
        case "extents":
        {
            string p = a.Positional(0) ?? throw new ArgumentException("缺少檔案路徑");
            a.EnsureAllUsed();
            var vol = Native.GetVolume(p);
            using var h = Native.OpenMeta(Path.GetFullPath(p));
            var m = Native.GetMeta(h);
            var ex = Native.GetExtents(h);
            Console.WriteLine($"大小 {m.Size}，種類 {FileKinds.Classify(m, ex)}，叢集 {vol.ClusterSize} 位元組，{ex.Count} 段：");
            foreach (var e in ex) Console.WriteLine($"  VCN {e.Vcn,10}  叢集數 {e.Clusters,8}  LCN {(e.OnDisk ? e.Lcn.ToString() : "(未配置)")}");
            return 0;
        }
        case "volume":
        {
            var vol = Native.GetVolume(a.Positional(0) ?? ".");
            a.EnsureAllUsed();
            Console.WriteLine($"根目錄 {vol.Root}，裝置 {vol.DevicePath}，{vol.FileSystem}，叢集 {vol.ClusterSize}，" +
                              $"共 {vol.TotalClusters} 叢集，剩 {Progress.Fmt(vol.FreeClusters * vol.ClusterSize)}，系統管理員：{Native.IsAdmin()}");
            return 0;
        }
        default:
            Usage();
            return 1;
    }
}
catch (ArgumentException ex)
{
    Console.Error.WriteLine("參數錯誤：" + ex.Message);
    return 1;
}
catch (Exception ex)
{
    Console.Error.WriteLine("錯誤：" + FileKinds.ErrText(ex));
    return 1;
}

static ReadOptions ReadOpts(Args a) => new()
{
    ThresholdMBps = double.Parse(a.Value("--threshold") ?? "100"),
    AllowanceMs = double.Parse(a.Value("--allowance-ms") ?? "2"),
    ChunkBytes = int.Parse(a.Value("--chunk-kb") ?? "1024") * 1024,
    QueueDepth = int.Parse(a.Value("--qd") ?? "4"),
    IoPriority = (a.Value("--priority") ?? "verylow") switch
    {
        "verylow" => 0, "low" => 1, "normal" => 2,
        var s => throw new ArgumentException($"--priority 只能是 verylow、low、normal：{s}"),
    },
    MeasurePriority = (a.Value("--measure-priority") ?? "normal") switch
    {
        "verylow" => 0, "low" => 1, "normal" => 2,
        var s => throw new ArgumentException($"--measure-priority 只能是 verylow、low、normal：{s}"),
    },
};

static void Usage()
{
    Console.WriteLine(@"
ssd-refresh：檔案層級的 SSD 刷新工具（只支援 NTFS）

  ssd-refresh scan <資料夾或檔案> [--out 報告.csv] [--hash] [--resume]
      只讀。逐檔讀取（不經系統快取），記錄每個 extent 的速度，列出慢的檔案。

  ssd-refresh refresh --from 報告.csv [--out 記錄.csv] [--select slow|all] [--force]
                      [--confirm X:] [--allow-system-volume] [--resume] [--dry-run]
      【需要系統管理員】把慢的檔案搬到新的叢集，前後比對 SHA256 並重新測速。
      執行前會要求輸入磁碟代號確認。--dry-run 只重新測速，不搬移。

  ssd-refresh extents <檔案>     顯示檔案的叢集分布
  ssd-refresh volume <路徑>      顯示磁碟區資訊

共用參數：
  --threshold 100     低於多少 MB/s 算慢（MB = 10^6 位元組）
  --allowance-ms 2    每個 extent 容許的額外延遲，避免小檔被誤判
  --chunk-kb 1024     每個 I/O 的大小
  --qd 4              同時排隊的 I/O 數
  --priority verylow  大量讀取與搬移的 I/O 優先權：verylow、low、normal
  --measure-priority normal  確認重讀與刷新前後測速的優先權

執行中：P 暫停／繼續，Q 或 Ctrl+C 做完目前的檔案後停止。
也可以建立「輸出檔名.pause」檔案來暫停，刪掉就繼續。
");
}

sealed class Args
{
    readonly List<string> _list;
    readonly HashSet<int> _used = new();
    public Args(IEnumerable<string> a) { _list = a.ToList(); }

    public bool Flag(string name)
    {
        int i = _list.IndexOf(name);
        if (i < 0) return false;
        _used.Add(i);
        return true;
    }

    public string? Value(string name)
    {
        int i = _list.IndexOf(name);
        if (i < 0) return null;
        if (i + 1 >= _list.Count) throw new ArgumentException($"{name} 後面缺少值");
        _used.Add(i); _used.Add(i + 1);
        return _list[i + 1];
    }

    public string? Positional(int n)
    {
        int k = 0;
        for (int i = 0; i < _list.Count; i++)
        {
            if (_list[i].StartsWith("--")) { i++; if (i < _list.Count && IsFlagOnly(_list[i - 1])) i--; continue; }
            if (k++ == n) { _used.Add(i); return _list[i]; }
        }
        return null;
    }

    static bool IsFlagOnly(string s) => s is "--hash" or "--resume" or "--force" or "--allow-system-volume" or "--dry-run";

    public void EnsureAllUsed()
    {
        for (int i = 0; i < _list.Count; i++)
            if (!_used.Contains(i)) throw new ArgumentException($"不認得的參數：{_list[i]}");
    }
}
