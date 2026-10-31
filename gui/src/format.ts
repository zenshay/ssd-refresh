// Number formats. Bytes use powers of 1000, like the terminal output and the drive labels.

export const fmtCount = (n: number): string => n.toLocaleString("en-US");

// "1 file", "2 files". Give the plural only when it is not the singular plus "s".
export const countOf = (
  n: number,
  singular: string,
  plural = `${singular}s`,
): string => `${fmtCount(n)} ${n === 1 ? singular : plural}`;

export const wasWere = (n: number): string => (n === 1 ? "was" : "were");

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

// "5 minutes", "3 hours", "2 days": the largest whole unit, like the verify summary.
export function fmtAge(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  if (s >= 86400) return countOf(Math.floor(s / 86400), "day");
  if (s >= 3600) return countOf(Math.floor(s / 3600), "hour");
  return countOf(Math.floor(s / 60), "minute");
}

/** "Refreshed just now" or "Refreshed 3 days ago". The drive card and Verify use the same text. */
export function refreshedText(ageSecs: number): string {
  return ageSecs < 60
    ? "Refreshed just now"
    : `Refreshed ${fmtAge(ageSecs)} ago`;
}
