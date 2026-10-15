// Number formats. Bytes use powers of 1000, like the terminal output and the drive labels.

export const fmtCount = (n: number): string => n.toLocaleString("en-US");

export function fmtBytes(bytes: number): string {
  const gb = bytes / 1e9;
  if (gb >= 100) return `${gb.toFixed(0)} GB`;
  if (gb >= 1) return `${gb.toFixed(1)} GB`;
  return `${(bytes / 1e6).toFixed(0)} MB`;
}

export const fmtRate = (mbps: number): string =>
  mbps >= 100 ? `${mbps.toFixed(0)} MB/s` : `${mbps.toFixed(1)} MB/s`;

export function fmtDuration(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = s % 60;
  if (h > 0) return `${h}h ${String(m).padStart(2, "0")}m`;
  if (m > 0) return `${m}m ${String(r).padStart(2, "0")}s`;
  return `${r}s`;
}

export const fmtPercent = (fraction: number): string =>
  `${(fraction * 100).toFixed(1)}%`;

// Shorten a path in the middle. The end keeps more characters because it holds the file name.
// Count code points, so the cut does not break a character into two halves.
export function truncatePath(path: string, maxLen = 60): string {
  const chars = Array.from(path);
  if (chars.length <= maxLen) return path;
  const keep = Math.max(maxLen - 1, 2);
  const head = Math.floor(keep / 2);
  const tail = keep - head;
  return `${chars.slice(0, head).join("")}…${chars.slice(-tail).join("")}`;
}
