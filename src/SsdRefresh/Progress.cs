using System.Diagnostics;
using System.Text;

namespace SsdRefresh;

/// <summary>暫停控制：按 P 切換，或在輸出檔旁邊放一個 .pause 檔（方便腳本控制）。</summary>
public sealed class PauseControl
{
    volatile bool _keyPaused;
    readonly string? _pauseFile;
    long _lastCheck;
    bool _fileExists;

    public PauseControl(string? pauseFile) { _pauseFile = pauseFile; }

    public void Toggle() => _keyPaused = !_keyPaused;

    public bool IsPaused
    {
        get
        {
            if (_pauseFile != null)
            {
                long now = Environment.TickCount64;
                if (now - Interlocked.Read(ref _lastCheck) > 500)
                {
                    _fileExists = File.Exists(_pauseFile);
                    Interlocked.Exchange(ref _lastCheck, now);
                }
            }
            return _keyPaused || _fileExists;
        }
    }

    public string? PauseFile => _pauseFile;

    public async Task WaitAsync(CancellationToken ct)
    {
        while (IsPaused) await Task.Delay(200, ct);
    }
}

/// <summary>原地更新的一行進度（終端機約 48 欄，所以限制在 47 個顯示寬度內）。</summary>
public sealed class Progress : IDisposable
{
    public const int Width = 47;
    readonly string _verb;
    readonly long _totalFiles, _totalBytes;
    long _files, _bytes, _slow, _errors;
    readonly Stopwatch _sw = Stopwatch.StartNew();
    readonly Timer _timer;
    readonly PauseControl _pause;
    readonly bool _interactive = !Console.IsOutputRedirected;
    readonly object _lock = new();
    long _lastLog;
    string _extra = "";

    public Progress(string verb, long totalFiles, long totalBytes, PauseControl pause)
    {
        _verb = verb; _totalFiles = totalFiles; _totalBytes = totalBytes; _pause = pause;
        _timer = new Timer(_ => Render(false), null, 250, 250);
    }

    public void AddFile(long bytes, bool slow, bool error)
    {
        Interlocked.Increment(ref _files);
        Interlocked.Add(ref _bytes, bytes);
        if (slow) Interlocked.Increment(ref _slow);
        if (error) Interlocked.Increment(ref _errors);
    }

    public void SetExtra(string s) => _extra = s;
    public long Slow => Interlocked.Read(ref _slow);
    public long Errors => Interlocked.Read(ref _errors);

    /// <summary>印一般訊息：先清掉進度列，印完再畫回來。</summary>
    public void Log(string msg)
    {
        lock (_lock)
        {
            if (_interactive) Console.Write("\r" + new string(' ', Width) + "\r");
            Console.WriteLine(msg);
        }
        Render(false);
    }

    public void Render(bool final)
    {
        lock (_lock)
        {
            double sec = _sw.Elapsed.TotalSeconds;
            long b = Interlocked.Read(ref _bytes), f = Interlocked.Read(ref _files);
            double pct = _totalBytes > 0 ? 100.0 * b / _totalBytes : (_totalFiles > 0 ? 100.0 * f / _totalFiles : 0);
            double rate = sec > 0 ? b / 1e6 / sec : 0;
            string eta = "";
            if (b > 0 && _totalBytes > b)
            {
                var left = TimeSpan.FromSeconds(sec * (_totalBytes - b) / b);
                eta = left.TotalHours >= 1 ? $"剩{(int)left.TotalHours}:{left.Minutes:00}" : $"剩{left.Minutes}:{left.Seconds:00}";
                if (left.TotalMinutes >= 10) eta += $"({DateTime.Now + left:HH:mm})";
            }
            string state = _pause.IsPaused ? "暫停" : _verb;
            string line = $"{state} {pct,5:0.0}% {Fmt(b)}/{Fmt(_totalBytes)} {rate:0}MB/s 慢{Interlocked.Read(ref _slow)} {eta}{_extra}";
            if (_interactive)
            {
                Console.Write("\r" + Fit(line) + (final ? "\n" : ""));
            }
            else if (final || Environment.TickCount64 - _lastLog > 10_000)
            {
                _lastLog = Environment.TickCount64;
                Console.WriteLine(line);
            }
        }
    }

    public static string Fmt(long bytes)
    {
        double g = bytes / 1e9;
        return g >= 100 ? $"{g:0}G" : g >= 1 ? $"{g:0.0}G" : $"{bytes / 1e6:0}M";
    }

    static int CharWidth(char c) => c >= 0x1100 && (c <= 0x115F || (c >= 0x2E80 && c <= 0xA4CF) || (c >= 0xAC00 && c <= 0xD7A3) || (c >= 0xF900 && c <= 0xFAFF) || (c >= 0xFF00 && c <= 0xFF60)) ? 2 : 1;

    /// <summary>截斷或補空白到剛好 Width 個顯示寬度（中文字算 2 格）。</summary>
    public static string Fit(string s)
    {
        var sb = new StringBuilder();
        int w = 0;
        foreach (char c in s)
        {
            int cw = CharWidth(c);
            if (w + cw > Width) break;
            sb.Append(c); w += cw;
        }
        return sb.Append(' ', Width - w).ToString();
    }

    public void Dispose()
    {
        _timer.Dispose();
        Render(true);
    }
}

/// <summary>背景讀鍵盤：P 暫停／繼續，Q 停止（目前的檔案做完才停）。</summary>
public static class KeyWatcher
{
    public static void Start(PauseControl pause, CancellationTokenSource cts, Action<string> log)
    {
        if (Console.IsInputRedirected) return;
        var t = new Thread(() =>
        {
            while (!cts.IsCancellationRequested)
            {
                try
                {
                    if (!Console.KeyAvailable) { Thread.Sleep(100); continue; }
                    var k = Console.ReadKey(true).Key;
                    if (k == ConsoleKey.P) { pause.Toggle(); log(pause.IsPaused ? "已暫停，按 P 繼續。" : "繼續。"); }
                    else if (k == ConsoleKey.Q) { log("收到 Q：做完目前的檔案後停止。"); cts.Cancel(); }
                }
                catch { return; }
            }
        }) { IsBackground = true };
        t.Start();
    }
}
