import type { Progress } from "./api";
import { fmtBytes, fmtCount, fmtDuration, fmtRate } from "./format";
import type { Store } from "./store";

export interface LogEntry {
  timestamp: string;
  level: "info" | "warn" | "error" | "cmd";
  text: string;
}

export function createDiagnostics() {
  const MAX_LOGS = 500;
  const logs: LogEntry[] = [];
  const listeners = new Set<() => void>();

  let lastStage = "";
  let lastLoggedPct = -1;
  let lastCatalogedCount = 0;

  function resetProgressLogState(): void {
    lastStage = "";
    lastLoggedPct = -1;
    lastCatalogedCount = 0;
  }

  function logProgressStream(p: Progress, activeRoot?: string | null): void {
    const currentRoot = activeRoot ?? "Drive";

    // Stage transition logging
    if (p.stage !== lastStage) {
      lastStage = p.stage;
      lastLoggedPct = -1;
      lastCatalogedCount = 0;

      if (p.stage === "find") {
        logMessage(
          "info",
          `[Step 1/3] Finding files on ${currentRoot}... Traversing directories.`,
        );
      } else if (p.stage === "read") {
        logMessage(
          "info",
          `[Step 2/3] Measuring read speeds on ${currentRoot}... (${fmtCount(p.totalFiles)} files, ${fmtBytes(p.totalBytes)})`,
        );
      } else if (p.stage === "settle") {
        logMessage(
          "info",
          `[Step 3/3] Settle wait on ${currentRoot}. Drive cooldown in progress.`,
        );
      } else if (p.stage === "refresh") {
        logMessage(
          "info",
          `[Step 2/3] Rewriting slow files on ${currentRoot} via safe relocation.`,
        );
      } else if (p.stage === "recheck" || p.stage === "verify") {
        logMessage(
          "info",
          `[Step 3/3] Rechecking read speeds to verify improvements.`,
        );
      }
    }

    // Live stream logs during find (every 25,000 files)
    if (p.stage === "find" && p.filesDone >= lastCatalogedCount + 25000) {
      lastCatalogedCount = Math.floor(p.filesDone / 25000) * 25000;
      logMessage("cmd", `Cataloged ${fmtCount(p.filesDone)} files so far...`);
    }

    // Live stream logs during read (every 10%)
    if (p.stage === "read" && p.totalBytes > 0) {
      const pct = Math.floor((p.bytesDone / p.totalBytes) * 10);
      if (pct > lastLoggedPct) {
        lastLoggedPct = pct;
        const pctText = ((p.bytesDone / p.totalBytes) * 100).toFixed(0);
        logMessage(
          "cmd",
          `[Progress ${pctText}%] Read ${fmtBytes(p.bytesDone)}/${fmtBytes(p.totalBytes)} · Speed: ${fmtRate(p.mbps)} · Time Left: ${p.etaSecs !== null ? fmtDuration(p.etaSecs) : "--"}`,
        );
      }
    }
  }

  function getTimestamp(): string {
    const d = new Date();
    return (
      d.toTimeString().split(" ")[0] +
      "." +
      String(d.getMilliseconds()).padStart(3, "0")
    );
  }

  function logMessage(level: LogEntry["level"], text: string): void {
    const entry: LogEntry = {
      timestamp: getTimestamp(),
      level,
      text,
    };
    logs.push(entry);
    if (logs.length > MAX_LOGS) {
      logs.shift();
    }

    for (const listener of listeners) listener();
  }

  function buildDiagnosticReport(store: Store): string {
    const state = store.get();
    const lines: string[] = [
      "# ssd-refresh Diagnostics Report",
      `Generated at: ${new Date().toISOString()}`,
      `OS: Windows (elevated: ${String(state.elevated)})`,
      "",
      "## Storage Volumes",
      ...state.volumes.map(
        (v) =>
          `- Drive ${v.root} (${v.fs}): ${(v.totalBytes / 1e9).toFixed(1)} GB total, ${(v.freeBytes / 1e9).toFixed(1)} GB free, SMART: ${v.smart.level} (${v.smart.detail ?? "ok"})`,
      ),
      "",
      "## Console Event Stream",
      "```",
      ...logs.map(
        (l) => `[${l.timestamp}] [${l.level.toUpperCase()}] ${l.text}`,
      ),
      "```",
    ];
    return lines.join("\n");
  }

  function clear(): void {
    logs.length = 0;
    resetProgressLogState();
    for (const listener of listeners) listener();
  }

  return {
    logMessage,
    resetProgressLogState,
    logProgressStream,
    buildDiagnosticReport,
    clear,
    entries: (): readonly LogEntry[] => logs,
    subscribe(listener: () => void): () => void {
      listeners.add(listener);
      listener();
      return () => listeners.delete(listener);
    },
  };
}

export type Diagnostics = ReturnType<typeof createDiagnostics>;
