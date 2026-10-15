import { api, type Progress } from "../api";
import { $, el } from "../dom";
import { fmtBytes, fmtCount, fmtDuration, fmtRate } from "../format";
import type { Store } from "../store";

export interface LogEntry {
  timestamp: string;
  level: "info" | "warn" | "error" | "cmd";
  text: string;
}

const MAX_LOGS = 500;
const logs: LogEntry[] = [];

let lastStage = "";
let lastLoggedPct = -1;
let lastCatalogedCount = 0;

export function resetProgressLogState(): void {
  lastStage = "";
  lastLoggedPct = -1;
  lastCatalogedCount = 0;
}

export function logProgressStream(
  p: Progress,
  activeRoot?: string | null,
): void {
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

export function logMessage(level: LogEntry["level"], text: string): void {
  const entry: LogEntry = {
    timestamp: getTimestamp(),
    level,
    text,
  };
  logs.push(entry);
  if (logs.length > MAX_LOGS) {
    logs.shift();
  }

  // If view is mounted, append entry directly to DOM
  const view = document.getElementById("console-log-view");
  const countEl = document.getElementById("console-log-count");
  if (view && countEl) {
    const row = el(
      "div",
      { class: `log-row log-${level}` },
      el("span", { class: "log-time", text: `[${entry.timestamp}]` }),
      el("span", {
        class: `log-tag log-tag-${level}`,
        text: `[${level.toUpperCase()}]`,
      }),
      el("span", { class: "log-text", text: entry.text }),
    );
    view.appendChild(row);
    view.scrollTop = view.scrollHeight;
    countEl.textContent = `${logs.length} entries`;
  }
}

export function buildDiagnosticReport(store: Store): string {
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
    ...logs.map((l) => `[${l.timestamp}] [${l.level.toUpperCase()}] ${l.text}`),
    "```",
  ];
  return lines.join("\n");
}

export function mountConsole(store: Store): void {
  const drawer = $<HTMLElement>("console-drawer");
  const copyBtn = $<HTMLButtonElement>("btn-copy-log");
  const saveBtn = $<HTMLButtonElement>("btn-save-log");
  const openBtn = $<HTMLButtonElement>("btn-open-log");
  const openFolderBtn = $<HTMLButtonElement>("btn-open-folder");
  const reportBtn = $<HTMLButtonElement>("btn-report-issue");
  const clearBtn = $<HTMLButtonElement>("btn-clear-log");
  const logView = $<HTMLElement>("console-log-view");
  const countEl = $<HTMLElement>("console-log-count");

  drawer.hidden = false;

  // 1. Copy Diagnostic Log to Clipboard
  copyBtn.addEventListener("click", async () => {
    const report = buildDiagnosticReport(store);
    try {
      await navigator.clipboard.writeText(report);
      const originalText = copyBtn.innerHTML;
      copyBtn.innerHTML = "<span>Copied! ✓</span>";
      setTimeout(() => {
        copyBtn.innerHTML = originalText;
      }, 1500);
    } catch (err) {
      logMessage("error", `Failed to copy log: ${err}`);
    }
  });

  // 2. Save Log to Selected File Path via Native Save Dialog
  saveBtn.addEventListener("click", async () => {
    const report = buildDiagnosticReport(store);
    try {
      const savedPath = await api.saveLogAs(report);
      if (savedPath) {
        logMessage("info", `Diagnostic report saved: ${savedPath}`);
      }
    } catch (err) {
      logMessage("error", `Failed to save log file: ${err}`);
    }
  });

  // 3. Open Diagnostic Report directly in system default text editor
  openBtn.addEventListener("click", async () => {
    const report = buildDiagnosticReport(store);
    try {
      await api.writeAndOpenLog(report);
      logMessage(
        "info",
        "Diagnostic report opened in system default text editor.",
      );
    } catch (err) {
      logMessage("error", `Failed to open log file: ${err}`);
    }
  });

  // 4. Open Application Logs Directory in File Explorer
  openFolderBtn.addEventListener("click", async () => {
    try {
      await api.openLogFolder();
      logMessage("info", "Opened logs directory in File Explorer.");
    } catch (err) {
      logMessage("error", `Failed to open log folder: ${err}`);
    }
  });

  // 4. One-Click Report Issue on GitHub
  reportBtn.addEventListener("click", () => {
    const state = store.get();
    const body = encodeURIComponent(
      `### Problem Description\n<!-- Describe what went wrong -->\n\n### Environment\n- OS: Windows\n- Elevated: ${state.elevated}\n- Drives: ${state.volumes.map((v) => v.root.slice(0, 2)).join(", ")}\n\n### Log Summary\n\`\`\`\n${logs
        .slice(-20)
        .map((l) => `[${l.timestamp}] [${l.level}] ${l.text}`)
        .join("\n")}\n\`\`\``,
    );
    const url = `https://github.com/my-org/ssd-refresh/issues/new?title=[Bug]+Diagnostic+Report&body=${body}`;
    void api.openUrl(url);
    logMessage("info", "Opened GitHub Issues in default browser.");
  });

  // 5. Clear Log View
  clearBtn.addEventListener("click", () => {
    logs.length = 0;
    logView.replaceChildren();
    countEl.textContent = "0 entries";
    resetProgressLogState();
    logMessage("info", "Console output cleared. System ready.");
  });

  // Initial welcome message
  logMessage("info", "ssd-refresh GUI engine ready.");
}
