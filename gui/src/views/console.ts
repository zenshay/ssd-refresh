import type { Api } from "../api";
import type { Diagnostics } from "../diagnostics";
import { $, el } from "../dom";
import type { Store } from "../store";

export function mountConsole(
  store: Store,
  diagnostics: Diagnostics,
  api: Api,
): void {
  const { logMessage, buildDiagnosticReport } = diagnostics;

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

  // 4. Open the data folder of the app (reports and logs) in File Explorer
  openFolderBtn.addEventListener("click", async () => {
    try {
      await api.openDataFolder();
      logMessage(
        "info",
        "Opened the data folder. Refresh logs are in reports. Diagnostic logs are in logs.",
      );
    } catch (err) {
      logMessage("error", `Failed to open log folder: ${err}`);
    }
  });

  // 4. One-Click Report Issue on GitHub
  reportBtn.addEventListener("click", async () => {
    try {
      await api.openUrl("https://github.com/zenshay/ssd-refresh/issues/new");
      logMessage("info", "Opened the GitHub issue page.");
    } catch (err) {
      logMessage("error", `Cannot open the GitHub issue page: ${err}`);
    }
  });

  // 5. Clear Log View
  clearBtn.addEventListener("click", () => {
    diagnostics.clear();
    logMessage("info", "Console output cleared. System ready.");
  });

  diagnostics.subscribe(() => {
    logView.replaceChildren(
      ...diagnostics.entries().map((entry) =>
        el(
          "div",
          { class: `log-row log-${entry.level}` },
          el("span", { class: "log-time", text: `[${entry.timestamp}]` }),
          el("span", {
            class: `log-tag log-${entry.level}`,
            text: `[${entry.level.toUpperCase()}]`,
          }),
          el("span", { class: "log-text", text: entry.text }),
        ),
      ),
    );
    countEl.textContent = `${diagnostics.entries().length} entries`;
    logView.scrollTop = logView.scrollHeight;
  });

  // Initial welcome message
  logMessage("info", "ssd-refresh GUI engine ready.");
}
