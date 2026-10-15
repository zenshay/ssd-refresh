// What happens when the user does something. Views only call these functions.
import { api, type ScanOutcome } from "./api";
import { fmtCount } from "./format";
import { askConfirm } from "./views/confirm";
import { refreshableScan, selectedVolume, type Notice, type Store } from "./store";

const errorText = (e: unknown): string => (typeof e === "string" ? e : e instanceof Error ? e.message : String(e));

function scanNotice(o: ScanOutcome): Notice {
  const files = fmtCount(o.totalFiles);
  if (o.stopped) {
    return {
      kind: "info",
      title: "Scan stopped",
      body: "Nothing changed on the drive. Run the scan again to get a result.",
    };
  }
  if (o.slowTotal === 0) {
    return {
      kind: "ok",
      title: "No slow files found",
      body: `The scan read ${files} files at a normal speed. Nothing changed on the drive.`,
    };
  }
  return {
    kind: "warn",
    title: `${fmtCount(o.slowTotal)} slow files found`,
    body: `The scan read ${files} files. Step 2 can rewrite only the slow files. Nothing has changed yet.`,
  };
}

export function createActions(store: Store) {
  async function loadVolumes(): Promise<void> {
    store.set({ volumesLoading: true });
    try {
      const volumes = await api.listVolumes();
      const keep = volumes.some((v) => v.root === store.get().selectedRoot);
      store.set({
        volumes,
        volumesLoading: false,
        selectedRoot: keep ? store.get().selectedRoot : (volumes[0]?.root ?? null),
      });
    } catch (e) {
      store.set({
        volumes: [],
        volumesLoading: false,
        notice: { kind: "error", title: "Cannot read the drive list", body: errorText(e) },
      });
    }
  }

  function selectDrive(root: string): void {
    const s = store.get();
    if (s.mode !== "idle" || s.selectedRoot === root) return;
    // A scan result belongs to one drive. It does not apply to another drive.
    store.set({ selectedRoot: root, notice: null });
  }

  async function runScan(): Promise<void> {
    const s = store.get();
    const volume = selectedVolume(s);
    if (s.mode !== "idle" || !volume) return;
    store.set({ mode: "scanning", paused: false, stopping: false, progress: null, scan: null, notice: null });
    try {
      const outcome = await api.startScan(volume.root);
      store.set({ mode: "idle", progress: null, scan: { root: volume.root, outcome }, notice: scanNotice(outcome) });
    } catch (e) {
      store.set({ mode: "idle", progress: null, notice: { kind: "error", title: "The scan failed", body: errorText(e) } });
    }
  }

  async function runRefresh(): Promise<void> {
    const s = store.get();
    const volume = selectedVolume(s);
    const scan = refreshableScan(s);
    if (s.mode !== "idle" || !volume || !scan || !s.scan) return;

    const count = fmtCount(scan.slowTotal);
    const confirmed = await askConfirm({
      title: `Rewrite ${count} slow files on ${volume.root.slice(0, 2)}?`,
      body: "The file system moves each file to new places on the drive. Programs can keep using the files. The tool checks each file after the move.",
      warning: volume.isSystem ? "This is the Windows drive. Close other programs first." : undefined,
      okLabel: "Rewrite files",
    });
    if (!confirmed) return;

    store.set({ mode: "refreshing", paused: false, stopping: false, progress: null, notice: null });
    try {
      const outcome = await api.startRefresh(scan.reportPath, volume.root);
      store.set({
        mode: "idle",
        progress: null,
        scan: null, // The old list is out of date now.
        notice: outcome.stopped
          ? { kind: "info", title: "Refresh stopped", body: `${fmtCount(outcome.movedFiles)} files were rewritten before the stop. Scan again to see the new list.` }
          : { kind: "ok", title: `${fmtCount(outcome.movedFiles)} files rewritten`, body: "Scan again to check the new read speed." },
      });
    } catch (e) {
      store.set({ mode: "idle", progress: null, notice: { kind: "error", title: "The refresh failed", body: errorText(e) } });
    }
  }

  async function togglePause(): Promise<void> {
    const s = store.get();
    if (s.mode === "idle" || s.stopping) return;
    const next = !s.paused;
    store.set({ paused: next }); // Show it at once. The answer from the core follows.
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

  return { loadVolumes, selectDrive, runScan, runRefresh, togglePause, stop };
}

export type Actions = ReturnType<typeof createActions>;
