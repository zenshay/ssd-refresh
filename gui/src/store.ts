// One source of truth. Views read this state and never keep a second copy.
import type { Progress, ScanOutcome, Volume } from "./api";
import { countOf, fmtCount, refreshedText } from "./format";

export type Mode = "idle" | "scanning" | "refreshing" | "verifying";
export type NoticeKind = "ok" | "info" | "warn" | "error";

export interface Notice {
  kind: NoticeKind;
  title: string;
  body?: string;
}

/** The notice at start when the app does not run as administrator. */
export const STANDARD_USER_NOTICE: Notice = {
  kind: "warn",
  title: "Not running as administrator",
  body: 'Safe Scan and Verify work. Selective Refresh needs administrator rights. To refresh, close the app. Right-click it and select "Run as administrator".',
};

export interface State {
  elevated: boolean | null;
  volumes: Volume[];
  volumesLoading: boolean;
  /** Multi-select: array of drive roots selected by the user. */
  selectedRoots: string[];
  /** Currently scanning/refreshing drive root. */
  activeRoot: string | null;
  mode: Mode;
  paused: boolean;
  stopping: boolean;
  progress: Progress | null;
  /** The last scan results by drive root. */
  scans: Record<string, ScanOutcome>;
  notice: Notice | null;
}

export const initialState: State = {
  elevated: null,
  volumes: [],
  volumesLoading: true,
  selectedRoots: [],
  activeRoot: null,
  mode: "idle",
  paused: false,
  stopping: false,
  progress: null,
  scans: {},
  notice: null,
};

export type Listener = (state: State, prev: State | null) => void;

export function createStore(initial: State) {
  let state = initial;
  const listeners = new Set<Listener>();
  return {
    get: (): State => state,
    set(patch: Partial<State>): void {
      const prev = state;
      state = { ...state, ...patch };
      for (const listener of listeners) {
        listener(state, prev);
      }
    },
    subscribe(listener: Listener): void {
      listeners.add(listener);
      listener(state, null);
    },
  };
}

export type Store = ReturnType<typeof createStore>;

// ---- Selectors: pure functions of the state ----

export const selectedVolumes = (s: State): Volume[] =>
  s.volumes.filter((v) => s.selectedRoots.includes(v.root));

export const isSelected = (s: State, root: string): boolean =>
  s.selectedRoots.includes(root);

export const allSelected = (s: State): boolean =>
  s.volumes.length > 0 &&
  s.volumes.every((v) => s.selectedRoots.includes(v.root));

/**
 * The slow files that were not fast after the wait.
 * A file that was fast after the wait is not counted.
 */
export const stillSlow = (o: ScanOutcome): number =>
  Math.max(0, o.slowTotal - o.fastAfterWait);

/** Still-slow files across selected drives that are ready to refresh. */
export function totalSlowAcrossSelected(s: State): number {
  let total = 0;
  for (const root of s.selectedRoots) {
    const outcome = s.scans[root];
    if (outcome && !outcome.stopped) {
      total += stillSlow(outcome);
    }
  }
  return total;
}

/** Slow files across selected drives that were fast after the wait. */
export function totalFastAfterWait(s: State): number {
  let total = 0;
  for (const root of s.selectedRoots) {
    const outcome = s.scans[root];
    if (outcome && !outcome.stopped) {
      total += outcome.fastAfterWait;
    }
  }
  return total;
}

/** Before this age, verify shows the speed result as NOT DECIDED. The core uses the same limit. */
export const MIN_VERIFY_AGE_SECS = 12 * 3600;

/** The selected drives that have a refresh log. */
export const verifyTargets = (s: State): Volume[] =>
  selectedVolumes(s).filter((v) => v.lastRefresh !== null);

/** The text on the card of the drive that the current operation uses. */
export function activeDriveText(mode: Mode): string | null {
  if (mode === "scanning") return "Scanning…";
  if (mode === "refreshing") return "Refreshing…";
  if (mode === "verifying") return "Verifying…";
  return null;
}

export interface SafetyBadge {
  text: string;
  title: string;
  /** "read" is green. "write" is amber. */
  tone: "read" | "write";
}

/**
 * The header badge tells if the running operation writes to the drive.
 * When idle, it is hidden: the step cards tell what each step does.
 */
export function safetyBadge(mode: Mode): SafetyBadge | null {
  if (mode === "refreshing")
    return {
      text: "Rewriting files",
      title:
        "Refresh moves each slow file to a new place on the SSD. The file data does not change.",
      tone: "write",
    };
  if (mode === "scanning" || mode === "verifying")
    return {
      text: "Read-only",
      title:
        "This operation reads files and writes a log. It does not change file data.",
      tone: "read",
    };
  return null;
}

export type ActionState = "ready" | "running" | "done" | "waiting" | "locked";

export interface ActionView {
  state: ActionState;
  title: string;
  hint: string;
  label: string;
  disabled: boolean;
}

export function scanAction(s: State): ActionView {
  const count = s.selectedRoots.length;
  const baseHint =
    "Read-only unbuffered read. Finds slow files. Changes nothing.";

  if (s.mode === "scanning") {
    return {
      state: "running",
      title: count > 1 ? `Scanning (${count} drives)…` : "Scanning…",
      hint: baseHint,
      label: "Running",
      disabled: true,
    };
  }
  if (s.mode === "refreshing" || s.mode === "verifying") {
    return {
      state: "waiting",
      title: "Safe Scan",
      hint: baseHint,
      label: "Waiting",
      disabled: true,
    };
  }
  if (count === 0) {
    return {
      state: "locked",
      title: "Safe Scan",
      hint: "Select at least one drive above.",
      label: "No drive",
      disabled: true,
    };
  }

  const title = count > 1 ? `Safe Scan (${count} drives)` : "Safe Scan";
  const allDone =
    count > 0 &&
    s.selectedRoots.every((root) => s.scans[root] && !s.scans[root].stopped);

  return {
    state: allDone ? "done" : "ready",
    title,
    hint: allDone
      ? "Scan done. Click to scan again. Changes nothing."
      : count > 1
        ? `Scans ${count} selected drives sequentially.`
        : baseHint,
    label: allDone ? "Scanned" : "Ready",
    disabled: false,
  };
}

export function refreshAction(s: State): ActionView {
  const title = "Selective Refresh";
  if (s.mode === "verifying") {
    return {
      state: "waiting",
      title,
      hint: "Wait for the verify to end.",
      label: "Waiting",
      disabled: true,
    };
  }
  if (s.mode === "refreshing") {
    return {
      state: "running",
      title: "Refreshing…",
      hint: "The file system moves each file safely.",
      label: "Running",
      disabled: true,
    };
  }
  if (s.mode === "scanning") {
    return {
      state: "waiting",
      title,
      hint: "Wait for the scan to end.",
      label: "Waiting",
      disabled: true,
    };
  }

  // Check if any selected drive is failing SMART
  const failingVolume = selectedVolumes(s).find(
    (v) => v.smart.level === "failing",
  );
  if (failingVolume) {
    return {
      state: "locked",
      title,
      hint: `SMART says ${failingVolume.root.slice(0, 2)} is failing. Back it up. Deselect it to refresh the other drives.`,
      label: "Blocked",
      disabled: true,
    };
  }

  // Refresh moves clusters. Windows allows this only for administrators.
  // Tell the user before a long scan, not after it.
  if (s.elevated === false) {
    return {
      state: "locked",
      title,
      hint: "Needs administrator rights. Restart as administrator.",
      label: "Locked",
      disabled: true,
    };
  }

  const slowTotal = totalSlowAcrossSelected(s);
  if (slowTotal === 0) {
    const hasScanned =
      s.selectedRoots.length > 0 && s.selectedRoots.every((r) => s.scans[r]);
    const fastAfterWait = totalFastAfterWait(s);
    return {
      state: "locked",
      title,
      hint: !hasScanned
        ? "Run Safe Scan first."
        : fastAfterWait > 0
          ? `${fastAfterWait === 1 ? "The slow file was" : `All ${fmtCount(fastAfterWait)} slow files were`} fast after the wait. No file needs a rewrite.`
          : "The scan found no slow files on selected drives.",
      label: "Locked",
      disabled: true,
    };
  }

  return {
    state: "ready",
    // The hint has the count. A short title keeps the card on one title line.
    title,
    hint: `Rewrites ${countOf(slowTotal, "slow file")} safely. You confirm first.`,
    label: "Ready",
    disabled: false,
  };
}

export function verifyAction(
  s: State,
  nowSecs: number = Date.now() / 1000,
): ActionView {
  const title = "Verify";
  if (s.mode === "verifying") {
    return {
      state: "running",
      title: "Verifying…",
      hint: "Reads the refreshed files again. Changes nothing.",
      label: "Running",
      disabled: true,
    };
  }
  if (s.mode !== "idle") {
    return {
      state: "waiting",
      title,
      hint: "Wait for the current operation to end.",
      label: "Waiting",
      disabled: true,
    };
  }
  const targets = verifyTargets(s);
  if (targets.length === 0) {
    // A drive that is not selected can have a refresh log. Name it, so the user selects it.
    const others = s.volumes
      .filter((v) => v.lastRefresh !== null)
      .map((v) => v.root.slice(0, 2));
    return {
      state: "locked",
      title,
      hint:
        others.length === 0
          ? "Run Selective Refresh first."
          : others.length === 1
            ? `${others[0]} has a refresh log. Select it to verify.`
            : `${others.join(", ")} have a refresh log. Select them to verify.`,
      label: "Locked",
      disabled: true,
    };
  }
  const newest = Math.max(...targets.map((v) => v.lastRefresh?.unixSecs ?? 0));
  const age = Math.max(0, nowSecs - newest);
  return {
    state: "ready",
    title:
      targets.length > 1
        ? `Verify (${countOf(targets.length, "drive")})`
        : title,
    hint:
      age < MIN_VERIFY_AGE_SECS
        ? `${refreshedText(age)}. Checks data now. Speed result after 12 hours.`
        : `${refreshedText(age)}. Read-only. Changes nothing.`,
    label: "Ready",
    disabled: false,
  };
}
