using System.Text;

namespace SsdRefresh;

/// <summary>簡單的 RFC 4180 CSV 讀寫。每寫一列就 flush，所以中斷後檔案內容就是檢查點。</summary>
public sealed class CsvWriter : IDisposable
{
    readonly StreamWriter _w;
    readonly string[] _header;

    public CsvWriter(string path, string[] header, bool append)
    {
        _header = header;
        bool writeHeader = !append || !File.Exists(path) || new FileInfo(path).Length == 0;
        if (append && !writeHeader) TrimPartialLastLine(path);
        _w = new StreamWriter(new FileStream(path, append ? FileMode.Append : FileMode.CreateNew, FileAccess.Write, FileShare.Read),
            new UTF8Encoding(!append || writeHeader));
        if (writeHeader) WriteRow(header);
    }

    public void WriteRow(IEnumerable<object?> values)
    {
        _w.WriteLine(string.Join(",", values.Select(v => Quote(Convert.ToString(v, System.Globalization.CultureInfo.InvariantCulture) ?? ""))));
        _w.Flush();
    }

    public static string Quote(string s) =>
        s.IndexOfAny(new[] { ',', '"', '\n', '\r' }) >= 0 ? "\"" + s.Replace("\"", "\"\"") + "\"" : s;

    /// <summary>程式被強制結束時，最後一列可能只寫了一半；續跑前先把它切掉。</summary>
    static void TrimPartialLastLine(string path)
    {
        using var fs = new FileStream(path, FileMode.Open, FileAccess.ReadWrite);
        if (fs.Length == 0) return;
        fs.Seek(-1, SeekOrigin.End);
        if (fs.ReadByte() == '\n') return;
        long pos = fs.Length - 1;
        while (pos > 0)
        {
            fs.Seek(pos - 1, SeekOrigin.Begin);
            if (fs.ReadByte() == '\n') break;
            pos--;
        }
        fs.SetLength(pos);
    }

    public void Dispose() => _w.Dispose();
}

public static class CsvReader
{
    /// <summary>讀出所有列，每列是「欄名 → 值」。最後一列不完整就略過。</summary>
    public static List<Dictionary<string, string>> ReadAll(string path)
    {
        var text = File.ReadAllText(path, Encoding.UTF8);
        var rows = Parse(text, out bool lastComplete);
        if (!lastComplete && rows.Count > 0) rows.RemoveAt(rows.Count - 1);
        var result = new List<Dictionary<string, string>>();
        if (rows.Count == 0) return result;
        var header = rows[0];
        foreach (var r in rows.Skip(1))
        {
            if (r.Count != header.Count) continue;
            var d = new Dictionary<string, string>();
            for (int i = 0; i < header.Count; i++) d[header[i]] = r[i];
            result.Add(d);
        }
        return result;
    }

    static List<List<string>> Parse(string s, out bool lastComplete)
    {
        var rows = new List<List<string>>();
        var row = new List<string>();
        var f = new StringBuilder();
        bool q = false;
        int i = 0;
        lastComplete = true;
        while (i < s.Length)
        {
            char c = s[i];
            if (q)
            {
                if (c == '"' && i + 1 < s.Length && s[i + 1] == '"') { f.Append('"'); i += 2; continue; }
                if (c == '"') { q = false; i++; continue; }
                f.Append(c); i++; continue;
            }
            if (c == '"') { q = true; i++; continue; }
            if (c == ',') { row.Add(f.ToString()); f.Clear(); i++; continue; }
            if (c == '\r') { i++; continue; }
            if (c == '\n') { row.Add(f.ToString()); f.Clear(); rows.Add(row); row = new List<string>(); i++; continue; }
            f.Append(c); i++;
        }
        if (f.Length > 0 || row.Count > 0) { row.Add(f.ToString()); rows.Add(row); lastComplete = false; }
        return rows;
    }
}
