import { createActions } from "./actions";
import { api } from "./api";
import { createStore, initialState } from "./store";
import { mountConfirm } from "./views/confirm";
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

  // Listen to Tauri progress events
  try {
    await api.onProgress((p) => {
      // Direct pass-through: update progress snapshot in store
      store.set({ progress: p });
    });
  } catch (err) {
    console.error("Failed to bind progress event listener:", err);
  }

  // Load environment privileges
  try {
    const env = await api.environment();
    store.set({ elevated: env.elevated });
  } catch (err) {
    console.warn("Could not query user privilege:", err);
  }

  // Initial drive listing
  await actions.loadVolumes();
});
