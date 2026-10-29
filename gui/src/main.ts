import { createActions } from "./actions";
import { api } from "./api";
import { createDiagnostics } from "./diagnostics";
import { countOf } from "./format";
import { createStore, initialState } from "./store";
import { askConfirm, mountConfirm } from "./views/confirm";
import { mountConsole } from "./views/console";
import { mountDrives } from "./views/drives";
import { mountNotice } from "./views/notice";
import { mountProgress } from "./views/progress";
import { mountSlowFiles } from "./views/slow_files";
import { mountWorkflow } from "./views/workflow";

// Global initialization
window.addEventListener("DOMContentLoaded", async () => {
  const store = createStore(initialState);
  const actions = createActions(store, askConfirm);
  const diagnostics = createDiagnostics();
  const { logMessage, logProgressStream, resetProgressLogState } = diagnostics;

  // Mount views
  mountConfirm();
  mountConsole(store, diagnostics, api);
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

  // Listen to Tauri progress events and feed live console
  try {
    await api.onMessage((message) => {
      if (message.trim()) logMessage("info", message);
    });
    await api.onProgress((p) => {
      store.set({ progress: p });
      logProgressStream(p, store.get().activeRoot);
    });
  } catch (err) {
    console.error("Failed to bind progress event listener:", err);
    logMessage("error", `IPC event binding error: ${err}`);
  }

  // Subscribe to notices and feed console log
  store.subscribe((s, prev) => {
    // Mode changed from idle to scanning or refreshing: reset trackers & output start log
    if (s.mode !== "idle" && s.mode !== prev?.mode) {
      resetProgressLogState();
      const roots = s.selectedRoots.map((r) => r.slice(0, 2)).join(", ");
      const opName = {
        scanning: "Safe Scan",
        refreshing: "Selective Refresh",
        verifying: "Verify",
      }[s.mode];
      logMessage("info", `Starting ${opName} on target storage [${roots}]...`);
    }

    // Active drive changed during multi-drive operations: reset stage trackers
    if (s.activeRoot && s.activeRoot !== prev?.activeRoot) {
      resetProgressLogState();
    }

    if (s.notice && s.notice !== prev?.notice) {
      logMessage(
        s.notice.kind === "ok" ? "info" : s.notice.kind,
        `${s.notice.title}: ${s.notice.body ?? ""}`,
      );
    }
  });

  // Load environment privileges
  try {
    const env = await api.environment();
    store.set({ elevated: env.elevated });
    logMessage(
      "info",
      `User privilege detected: ${env.elevated ? "Administrator" : "Standard user"}`,
    );
  } catch (err) {
    console.warn("Could not query user privilege:", err);
  }

  // Initial drive listing
  await actions.loadVolumes();
  const currentVols = store.get().volumes;
  logMessage(
    "info",
    `Storage inventory: ${countOf(currentVols.length, "drive")} detected (${currentVols.map((v) => v.root.slice(0, 2)).join(", ")})`,
  );
});
