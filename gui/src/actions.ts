// What happens when the user interacts. Views only call these actions.
import { api, type ScanOutcome } from "./api";
import { fmtCount } from "./format";
import {
  allSelected,
  type Notice,
  type Store,
  totalSlowAcrossSelected,
} from "./store";
import { askConfirm } from "./views/confirm";

const errorText = (e: unknown): string =>
  typeof e === "string" ? e : e instanceof Error ? e.message : String(e);

function scanNotice(results: { root: string; outcome: ScanOutcome }[]): Notice {
  const stopped = results.some((r) => r.outcome.stopped);
  const totalFiles = results.reduce((acc, r) => acc + r.outcome.totalFiles, 0);
  const totalSlow = results.reduce((acc, r) => acc + r.outcome.slowTotal, 0);

  if (stopped) {
    return {
      kind: "info",
      title: "Scan stopped",
      body: "Operation was cancelled. Nothing changed on the drive(s).",
    };
  }

  if (totalSlow === 0) {
    return {
      kind: "ok",
      title: "No slow files detected",
      body: `Checked ${fmtCount(totalFiles)} files across ${results.length} drive(s) with normal read speeds.`,
    };
  }

  return {
    kind: "warn",
    title: `${fmtCount(totalSlow)} slow files detected`,
    body: `Scanned ${fmtCount(totalFiles)} files across ${results.length} drive(s). Step 2 can rewrite only the slow files safely.`,
  };
}

export function createActions(store: Store) {
  async function loadVolumes(): Promise<void> {
    store.set({ volumesLoading: true });
    try {
      const volumes = await api.listVolumes();
      const currentSelected = store.get().selectedRoots;
      // Retain already selected roots if they still exist, otherwise default to all or first
      const validSelected = currentSelected.filter((r) =>
        volumes.some((v) => v.root === r),
      );
      const nextSelected =
        validSelected.length > 0
          ? validSelected
          : volumes.length > 0
            ? [volumes[0].root]
            : [];

      store.set({
        volumes,
        volumesLoading: false,
        selectedRoots: nextSelected,
      });
    } catch (e) {
      store.set({
        volumes: [],
        volumesLoading: false,
        notice: {
          kind: "error",
          title: "Cannot read drive list",
          body: errorText(e),
        },
      });
    }
  }

  function toggleSelect(root: string): void {
    const s = store.get();
    if (s.mode !== "idle") return;

    const current = s.selectedRoots;
    let next: string[];
    if (current.includes(root)) {
      next = current.filter((r) => r !== root);
    } else {
      next = [...current, root];
    }
    store.set({ selectedRoots: next, notice: null });
  }

  function toggleSelectAll(): void {
    const s = store.get();
    if (s.mode !== "idle") return;

    if (allSelected(s)) {
      // If all selected, deselect all (or keep at least 0)
      store.set({ selectedRoots: [], notice: null });
    } else {
      // Select all available volumes
      store.set({ selectedRoots: s.volumes.map((v) => v.root), notice: null });
    }
  }

  async function runScan(): Promise<void> {
    const s = store.get();
    const rootsToScan = [...s.selectedRoots];
    if (s.mode !== "idle" || rootsToScan.length === 0) return;

    store.set({
      mode: "scanning",
      paused: false,
      stopping: false,
      progress: null,
      notice: null,
    });

    const finishedOutcomes: { root: string; outcome: ScanOutcome }[] = [];

    for (let i = 0; i < rootsToScan.length; i++) {
      const root = rootsToScan[i];
      // Check if user stopped during or before this drive
      if (store.get().stopping) break;

      store.set({ activeRoot: root, progress: null });

      try {
        const outcome = await api.startScan(root);
        finishedOutcomes.push({ root, outcome });

        // Update scan results cache
        const updatedScans = { ...store.get().scans, [root]: outcome };
        store.set({ scans: updatedScans });

        if (outcome.stopped) {
          break;
        }
      } catch (e) {
        store.set({
          mode: "idle",
          activeRoot: null,
          progress: null,
          notice: {
            kind: "error",
            title: `Scan failed on ${root.slice(0, 2)}`,
            body: errorText(e),
          },
        });
        return;
      }
    }

    store.set({
      mode: "idle",
      activeRoot: null,
      progress: null,
      stopping: false,
      notice: scanNotice(finishedOutcomes),
    });
  }

  async function runRefresh(): Promise<void> {
    const s = store.get();
    const slowCount = totalSlowAcrossSelected(s);
    if (s.mode !== "idle" || slowCount === 0) return;

    const drivesWithSlow = s.selectedRoots.filter(
      (r) => s.scans[r] && !s.scans[r].stopped && s.scans[r].slowTotal > 0,
    );

    const driveNames = drivesWithSlow.map((r) => r.slice(0, 2)).join(", ");
    const hasSystem = s.volumes.some(
      (v) => drivesWithSlow.includes(v.root) && v.isSystem,
    );

    const confirmed = await askConfirm({
      title: `Rewrite ${fmtCount(slowCount)} slow files across ${driveNames}?`,
      body: "The file system safely moves each slow file to fresh NAND blocks. Unchanged and fast files are not touched. Programs can continue running.",
      warning: hasSystem
        ? "Includes your Windows system drive. Close other apps before starting."
        : undefined,
      okLabel: "Rewrite slow files",
    });

    if (!confirmed) return;

    store.set({
      mode: "refreshing",
      paused: false,
      stopping: false,
      progress: null,
      notice: null,
    });

    let totalMoved = 0;
    let wasStopped = false;

    for (const root of drivesWithSlow) {
      if (store.get().stopping) {
        wasStopped = true;
        break;
      }

      const scanResult = s.scans[root];
      if (!scanResult) continue;

      store.set({ activeRoot: root, progress: null });

      try {
        const outcome = await api.startRefresh(scanResult.reportPath, root);
        totalMoved += outcome.movedFiles;
        if (outcome.stopped) {
          wasStopped = true;
          break;
        }
      } catch (e) {
        store.set({
          mode: "idle",
          activeRoot: null,
          progress: null,
          notice: {
            kind: "error",
            title: `Refresh failed on ${root.slice(0, 2)}`,
            body: errorText(e),
          },
        });
        return;
      }
    }

    // Invalidate scans for refreshed drives
    const nextScans = { ...store.get().scans };
    for (const root of drivesWithSlow) {
      delete nextScans[root];
    }

    store.set({
      mode: "idle",
      activeRoot: null,
      progress: null,
      stopping: false,
      scans: nextScans,
      notice: wasStopped
        ? {
            kind: "info",
            title: "Refresh stopped",
            body: `${fmtCount(totalMoved)} files were moved before the stop. Scan again to check remaining files.`,
          }
        : {
            kind: "ok",
            title: `${fmtCount(totalMoved)} files rewritten successfully`,
            body: "Run Safe Scan to verify new read speeds.",
          },
    });
  }

  async function togglePause(): Promise<void> {
    const s = store.get();
    if (s.mode === "idle" || s.stopping) return;
    const next = !s.paused;
    store.set({ paused: next });
    try {
      const confirmed = await api.togglePause();
      if (confirmed !== next) store.set({ paused: confirmed });
    } catch {
      store.set({ paused: !next });
    }
  }

  async function stop(): Promise<void> {
    const s = store.get();
    if (s.mode === "idle" || s.stopping) return;
    store.set({ stopping: true, paused: false });
    try {
      await api.stop();
    } catch {
      store.set({ stopping: false });
    }
  }

  return {
    loadVolumes,
    toggleSelect,
    toggleSelectAll,
    runScan,
    runRefresh,
    togglePause,
    stop,
  };
}

export type Actions = ReturnType<typeof createActions>;
