import { createActions } from "./actions";
import { api } from "./api";
import { fmtBytes, fmtCount, fmtDuration, fmtRate } from "./format";
import { createStore, initialState } from "./store";
import { mountConfirm } from "./views/confirm";
import { mountConsole, logMessage } from "./views/console";
import { mountDrives } from "./views/drives";
import { mountNotice } from "./views/notice";
import { mountProgress } from "./views/progress";
import { mountSlowFiles } from "./views/slow_files";
import { mountWorkflow } from "./views/workflow";

// Global initialization
window.addEventListener("DOMContentLoaded", async () => {
  const store = createStore(initialState);
  const actions = createActions(store);

  // Mount views
  mountConfirm();
  mountConsole(store);
  mountWorkflow(store, actions);
  mountDrives(store, actions);
  mountProgress(store, actions);
  mountNotice(store);
  mountSlowFiles(store);

  // Custom frameless window controls
  const minBtn = document.getElementById("btn-win-min");
  const maxBtn = document.getElementById("btn-win-max");
  const closeBtn = document.getElementById("btn-win-close");

  minBtn?.addEventListener("click", () => void api.windowMinimize());
  maxBtn?.addEventListener("click", () => void api.windowToggleMaximize());
  closeBtn?.addEventListener("click", () => void api.windowClose());

  let lastStage = "";
  let lastLoggedPct = -1;
  let lastCatalogedCount = 0;

  // Listen to Tauri progress events and feed live console
  try {
    await api.onProgress((p) => {
      store.set({ progress: p });

      // Stage transition logging
      if (p.stage !== lastStage) {
        lastStage = p.stage;
        lastLoggedPct = -1;
        lastCatalogedCount = 0;

        const currentRoot = store.get().activeRoot ?? "Drive";
        if (p.stage === "find") {
          logMessage("info", `[Step 1/3] Finding files on ${currentRoot}... Traversing directories.`);
        } else if (p.stage === "read") {
          logMessage(
            "info",
            `[Step 2/3] Measuring read speeds on ${currentRoot}... (${fmtCount(p.totalFiles)} files, ${fmtBytes(p.totalBytes)})`
          );
        } else if (p.stage === "settle") {
          logMessage("info", `[Step 3/3] Settle wait on ${currentRoot}. Drive cooldown in progress.`);
        } else if (p.stage === "refresh") {
          logMessage("info", `[Step 2/3] Rewriting slow files on ${currentRoot} via safe relocation.`);
        } else if (p.stage === "recheck" || p.stage === "verify") {
          logMessage("info", `[Step 3/3] Rechecking read speeds to verify improvements.`);
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
            `[Progress ${pctText}%] Read ${fmtBytes(p.bytesDone)}/${fmtBytes(p.totalBytes)} · Speed: ${fmtRate(p.mbps)} · Time Left: ${p.etaSecs !== null ? fmtDuration(p.etaSecs) : "--"}`
          );
        }
      }
    });
  } catch (err) {
    console.error("Failed to bind progress event listener:", err);
    logMessage("error", `IPC event binding error: ${err}`);
  }

  // Subscribe to notices and feed console log
  store.subscribe((s, prev) => {
    if (s.notice && s.notice !== prev?.notice) {
      logMessage(s.notice.kind === "ok" ? "info" : s.notice.kind, `${s.notice.title}: ${s.notice.body ?? ""}`);
    }
  });

  // Load environment privileges
  try {
    const env = await api.environment();
    store.set({ elevated: env.elevated });
    logMessage("info", `User privilege detected: ${env.elevated ? "Administrator" : "Standard user"}`);
  } catch (err) {
    console.warn("Could not query user privilege:", err);
  }

  // Initial drive listing
  await actions.loadVolumes();
  const currentVols = store.get().volumes;
  logMessage(
    "info",
    `Storage inventory: ${currentVols.length} drives detected (${currentVols.map((v) => v.root.slice(0, 2)).join(", ")})`
  );
});
