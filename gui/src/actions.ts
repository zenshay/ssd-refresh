// What happens when the user interacts. Views only call these actions.

import type { Api } from "./api";
import { api, type ScanOutcome, type Verdict, type VerifyOutcome } from "./api";
import { countOf, fmtCount, wasWere } from "./format";
import type { Confirm } from "./interactions";
import {
  allSelected,
  type Notice,
  type Store,
  stillSlow,
  totalSlowAcrossSelected,
  verifyTargets,
} from "./store";

const errorText = (e: unknown): string =>
  typeof e === "string" ? e : e instanceof Error ? e.message : String(e);

function scanNotice(
  results: { root: string; outcome: ScanOutcome }[],
  cancelled: boolean,
): Notice {
  const stopped = cancelled || results.some((r) => r.outcome.stopped);
  const totalFiles = results.reduce((acc, r) => acc + r.outcome.totalFiles, 0);
  const totalSlow = results.reduce((acc, r) => acc + stillSlow(r.outcome), 0);
  const fastAfterWait = results.reduce(
    (acc, r) => acc + r.outcome.fastAfterWait,
    0,
  );
  const waitNote =
    fastAfterWait > 0
      ? ` ${countOf(fastAfterWait, "other file")} ${wasWere(fastAfterWait)} slow at first and fast after the wait.`
      : "";

  if (stopped) {
    return {
      kind: "info",
      title: "Scan stopped",
      body: "Operation was cancelled. Nothing changed on the drive(s).",
    };
  }

  if (totalSlow === 0 && fastAfterWait > 0) {
    return {
      kind: "ok",
      title: "No file stays slow",
      body: `Checked ${countOf(totalFiles, "file")} across ${countOf(results.length, "drive")}.${waitNote} No file needs a rewrite.`,
    };
  }

  if (totalSlow === 0) {
    return {
      kind: "ok",
      title: "No slow files detected",
      body: `Checked ${countOf(totalFiles, "file")} across ${countOf(results.length, "drive")} with normal read speeds.`,
    };
  }

  return {
    kind: "warn",
    title: `${countOf(totalSlow, "slow file")} detected`,
    body: `Scanned ${countOf(totalFiles, "file")} across ${countOf(results.length, "drive")}.${waitNote} Step 2 can rewrite only the slow files safely.`,
  };
}

function verifyNotice(
  results: { root: string; outcome: VerifyOutcome }[],
  cancelled: boolean,
): Notice {
  const drive = (root: string) => root.slice(0, 2);
  const unsafe = results.filter((r) => !r.outcome.dataSafe);
  if (unsafe.length > 0) {
    return {
      kind: "error",
      title: "Data changed without a program",
      body: `Data safe: NO on ${unsafe.map((r) => drive(r.root)).join(", ")}. Keep the logs and report this error.`,
    };
  }
  if (cancelled || results.some((r) => r.outcome.stopped)) {
    return {
      kind: "info",
      title: "Verify stopped",
      body: "The log has the summary of the files read until the stop. Nothing changed on the drive(s).",
    };
  }
  const verdicts = results.map((r) => r.outcome.faster);
  const all = (v: Verdict) =>
    verdicts.length > 0 && verdicts.every((x) => x === v);
  const kind = verdicts.includes("NO") ? "warn" : all("YES") ? "ok" : "info";
  const one = results.length === 1;
  const title = one
    ? `Data safe: YES. Faster: ${results[0].outcome.faster || "n/a"}`
    : "Data safe: YES on all drives";
  // One short line per fact. The speed comes first: the user wants to see it.
  const lines: string[] = [];
  for (const { root, outcome } of results) {
    const prefix = one ? "" : `${drive(root)}  `;
    const sp = outcome.speed;
    if (!sp) {
      lines.push(`${prefix}No file was slow before.`);
      continue;
    }
    if (!one) lines.push(`${prefix}Faster: ${outcome.faster || "n/a"}`);
    lines.push(
      `${prefix}Read speed: ${speedChange(sp.beforeMbps, sp.nowMbps)}`,
      `${prefix}Slow files: ${fmtCount(sp.slowBefore)} → ${fmtCount(sp.slowNow)}`,
    );
  }
  lines.push(nextStep(results.map((r) => r.outcome)));
  return { kind, title, body: lines.join("\n") };
}

/** Verify shows YES or NO only with this many slow files or more (src/verify.rs MIN_SLOW_FILES). */
const MIN_SLOW_FILES = 10;

/** "1298 → 255 MB/s (5.1 times slower)" */
export function speedChange(before: number, now: number): string {
  const factor = before > 0 && now > 0 ? now / before : 1;
  const times =
    factor >= 1
      ? `${factor.toFixed(1)} times faster`
      : `${(1 / factor).toFixed(1)} times slower`;
  return `${Math.round(before)} → ${Math.round(now)} MB/s (${times})`;
}

/** The last line of the verify notice: what the user can do next. */
export function nextStep(outcomes: VerifyOutcome[], now = Date.now()): string {
  const undecided = outcomes.filter(
    (o) => o.faster === "NOT DECIDED" && o.speed,
  );
  const few = undecided.filter(
    (o) => (o.speed?.slowBefore ?? 0) < MIN_SLOW_FILES,
  );
  if (few.length > 0) {
    return `Faster needs ${MIN_SLOW_FILES} or more slow files. The numbers above show the change.`;
  }
  const wait = Math.max(0, ...undecided.map((o) => o.waitSecs));
  if (wait > 0) {
    const at = new Date(now + wait * 1000).toLocaleString("en-US", {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
      hour12: false,
    });
    return `Next step: verify again after ${at}. New data is always fast at first.`;
  }
  return "The log has the full summary.";
}

type Operations = Pick<
  Api,
  | "listVolumes"
  | "startScan"
  | "startRefresh"
  | "startVerify"
  | "togglePause"
  | "stop"
>;

export function createActions(
  store: Store,
  confirm: Confirm,
  backend: Operations = api,
) {
  let confirming = false;
  async function loadVolumes(): Promise<void> {
    store.set({ volumesLoading: true });
    try {
      const volumes = await backend.listVolumes();
      const currentSelected = store.get().selectedRoots;
      // Keep the selected drives that still exist. Without a selection (for example at start),
      // select the drives with a refresh log, so Verify is ready after a restart.
      // Without a refresh log, select the first drive.
      const validSelected = currentSelected.filter((r) =>
        volumes.some((v) => v.root === r),
      );
      const refreshed = volumes
        .filter((v) => v.lastRefresh !== null)
        .map((v) => v.root);
      const nextSelected =
        validSelected.length > 0
          ? validSelected
          : refreshed.length > 0
            ? refreshed
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
        const outcome = await backend.startScan(root);
        finishedOutcomes.push({ root, outcome });

        // Update scan results cache
        const updatedScans = { ...store.get().scans, [root]: outcome };
        store.set({ scans: updatedScans });

        if (outcome.stopped) {
          break;
        }
      } catch (e) {
        store.set({
          paused: false,
          stopping: false,
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
      notice: scanNotice(finishedOutcomes, store.get().stopping),
    });
  }

  async function runRefresh(): Promise<void> {
    const s = store.get();
    const slowCount = totalSlowAcrossSelected(s);
    if (s.mode !== "idle" || slowCount === 0 || confirming) return;

    const drivesWithSlow = s.selectedRoots.filter(
      (r) => s.scans[r] && !s.scans[r].stopped && stillSlow(s.scans[r]) > 0,
    );
    const fastAfterWait = drivesWithSlow.reduce(
      (acc, r) => acc + s.scans[r].fastAfterWait,
      0,
    );

    const driveNames = drivesWithSlow.map((r) => r.slice(0, 2)).join(", ");
    const hasSystem = s.volumes.some(
      (v) => drivesWithSlow.includes(v.root) && v.isSystem,
    );

    confirming = true;
    let confirmed: boolean;
    try {
      confirmed = await confirm({
        title: `Rewrite ${countOf(slowCount, "slow file")} across ${driveNames}?`,
        body:
          "The file system safely moves each slow file to fresh NAND blocks. Unchanged and fast files are not touched. Programs can continue running." +
          (fastAfterWait > 0
            ? ` ${countOf(fastAfterWait, "file")} ${wasWere(fastAfterWait)} fast after the wait. The refresh reads each one again and skips it if it is fast.`
            : ""),
        warning: hasSystem
          ? "Includes your Windows system drive. Close other apps before starting."
          : undefined,
        okLabel: "Rewrite slow files",
      });
    } finally {
      confirming = false;
    }
    if (!confirmed || store.get().mode !== "idle") return;

    store.set({
      mode: "refreshing",
      paused: false,
      stopping: false,
      progress: null,
      notice: null,
    });

    let totalMoved = 0;
    const attemptedRoots = new Set<string>();
    let wasStopped = false;

    for (const root of drivesWithSlow) {
      if (store.get().stopping) {
        wasStopped = true;
        break;
      }

      const scanResult = s.scans[root];
      if (!scanResult) continue;

      store.set({ activeRoot: root, progress: null });

      attemptedRoots.add(root);
      try {
        const nextScans = { ...store.get().scans };
        delete nextScans[root];
        store.set({ scans: nextScans });
        const outcome = await backend.startRefresh(scanResult.reportPath, root);
        totalMoved += outcome.movedFiles;
        // The new log is now the newest refresh of this drive. Verify reads it.
        const lastRefresh = {
          logPath: outcome.logPath,
          unixSecs: Math.floor(Date.now() / 1000),
        };
        store.set({
          volumes: store
            .get()
            .volumes.map((v) => (v.root === root ? { ...v, lastRefresh } : v)),
        });
        if (outcome.stopped) {
          wasStopped = true;
          break;
        }
      } catch (e) {
        store.set({
          paused: false,
          stopping: false,
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
    for (const root of attemptedRoots) {
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
            body: `${countOf(totalMoved, "file")} ${wasWere(totalMoved)} moved before the stop. Scan again to check remaining files.`,
          }
        : {
            kind: "ok",
            title: `${countOf(totalMoved, "file")} rewritten successfully`,
            body: "New data always reads fast at first. Run Verify after 1, 7 and 30 days.",
          },
    });
  }

  async function runVerify(): Promise<void> {
    const s = store.get();
    const targets = verifyTargets(s);
    if (s.mode !== "idle" || targets.length === 0) return;

    store.set({
      mode: "verifying",
      paused: false,
      stopping: false,
      progress: null,
      notice: null,
    });

    const finished: { root: string; outcome: VerifyOutcome }[] = [];
    for (const volume of targets) {
      if (store.get().stopping) break;
      const logPath = volume.lastRefresh?.logPath;
      if (!logPath) continue;
      store.set({ activeRoot: volume.root, progress: null });
      try {
        const outcome = await backend.startVerify(logPath);
        finished.push({ root: volume.root, outcome });
        if (outcome.stopped || !outcome.dataSafe) break;
      } catch (e) {
        store.set({
          paused: false,
          stopping: false,
          mode: "idle",
          activeRoot: null,
          progress: null,
          notice: {
            kind: "error",
            title: `Verify failed on ${volume.root.slice(0, 2)}`,
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
      notice: verifyNotice(finished, store.get().stopping),
    });
  }

  async function togglePause(): Promise<void> {
    const s = store.get();
    if (s.mode === "idle" || s.stopping) return;
    const next = !s.paused;
    store.set({ paused: next });
    try {
      const confirmed = await backend.togglePause();
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
      await backend.stop();
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
    runVerify,
    togglePause,
    stop,
  };
}

export type Actions = ReturnType<typeof createActions>;
