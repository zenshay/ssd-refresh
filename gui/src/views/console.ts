import { $, el } from "../dom";
import type { Store } from "../store";

export interface LogEntry {
  timestamp: string;
  level: "info" | "warn" | "error" | "cmd";
  text: string;
}

const MAX_LOGS = 500;
const logs: LogEntry[] = [];

function getTimestamp(): string {
  const d = new Date();
  return d.toTimeString().split(" ")[0] + "." + String(d.getMilliseconds()).padStart(3, "0");
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
      el("span", { class: `log-tag log-tag-${level}`, text: `[${level.toUpperCase()}]` }),
      el("span", { class: "log-text", text: entry.text })
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
        `- Drive ${v.root} (${v.fs}): ${(v.totalBytes / 1e9).toFixed(1)} GB total, ${(v.freeBytes / 1e9).toFixed(1)} GB free, SMART: ${v.smart.level} (${v.smart.detail ?? "ok"})`
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
  const toggleBtn = $<HTMLButtonElement>("btn-toggle-console");
  const closeBtn = $<HTMLButtonElement>("btn-close-console");
  const copyBtn = $<HTMLButtonElement>("btn-copy-log");
  const saveBtn = $<HTMLButtonElement>("btn-save-log");
  const reportBtn = $<HTMLButtonElement>("btn-report-issue");
  const clearBtn = $<HTMLButtonElement>("btn-clear-log");
  const logView = $<HTMLElement>("console-log-view");
  const countEl = $<HTMLElement>("console-log-count");

  function toggleDrawer(): void {
    const isHidden = drawer.hidden;
    drawer.hidden = !isHidden;
    toggleBtn.classList.toggle("active", isHidden);
    if (isHidden) {
      logView.scrollTop = logView.scrollHeight;
    }
  }

  toggleBtn.addEventListener("click", toggleDrawer);
  closeBtn.addEventListener("click", () => {
    drawer.hidden = true;
    toggleBtn.classList.remove("active");
  });

  // Global Keyboard Shortcut: Ctrl + ` or F12
  window.addEventListener("keydown", (e) => {
    if ((e.ctrlKey && e.key === "`") || e.key === "F12") {
      e.preventDefault();
      toggleDrawer();
    }
  });

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

  // 2. Save Log to Disk File
  saveBtn.addEventListener("click", () => {
    const report = buildDiagnosticReport(store);
    const blob = new Blob([report], { type: "text/plain;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `ssd-refresh-debug-${Date.now()}.log`;
    a.click();
    URL.revokeObjectURL(url);
    logMessage("info", `Diagnostic log downloaded as ${a.download}`);
  });

  // 3. One-Click Report Issue on GitHub
  reportBtn.addEventListener("click", () => {
    const state = store.get();
    const body = encodeURIComponent(
      `### Problem Description\n<!-- Describe what went wrong -->\n\n### Environment\n- OS: Windows\n- Elevated: ${state.elevated}\n- Drives: ${state.volumes.map((v) => v.root.slice(0, 2)).join(", ")}\n\n### Log Summary\n\`\`\`\n${logs.slice(-20).map((l) => `[${l.timestamp}] [${l.level}] ${l.text}`).join("\n")}\n\`\`\``
    );
    const url = `https://github.com/my-org/ssd-refresh/issues/new?title=[Bug]+Diagnostic+Report&body=${body}`;
    window.open(url, "_blank");
  });

  // 4. Clear Log View
  clearBtn.addEventListener("click", () => {
    logs.length = 0;
    logView.replaceChildren();
    countEl.textContent = "0 entries";
    logMessage("info", "Console logs cleared.");
  });

  // Initial welcome message
  logMessage("info", "ssd-refresh GUI engine ready. Press Ctrl+` to toggle console anytime.");
}
