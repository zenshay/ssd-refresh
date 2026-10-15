// One source of truth. Views read this state and never keep a second copy.
import type { Progress, ScanOutcome, Volume } from "./api";

export type Mode = "idle" | "scanning" | "refreshing";
export type NoticeKind = "ok" | "info" | "warn" | "error";

export interface Notice {
  kind: NoticeKind;
  title: string;
  body?: string;
}

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
      listeners.forEach((listener) => listener(state, prev));
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
  s.volumes.length > 0 && s.volumes.every((v) => s.selectedRoots.includes(v.root));

/** Slow files count across selected drives that are ready to refresh. */
export function totalSlowAcrossSelected(s: State): number {
  let total = 0;
  for (const root of s.selectedRoots) {
    const outcome = s.scans[root];
    if (outcome && !outcome.stopped && outcome.slowTotal > 0) {
      total += outcome.slowTotal;
    }
  }
  return total;
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
  const baseHint = "Read-only unbuffered read. Finds slow files. Changes nothing.";

  if (s.mode === "scanning") {
    return {
      state: "running",
      title: count > 1 ? `Scanning (${count} drives)…` : "Scanning…",
      hint: baseHint,
      label: "Running",
      disabled: true,
    };
  }
  if (s.mode === "refreshing") {
    return { state: "waiting", title: "Safe Scan", hint: baseHint, label: "Waiting", disabled: true };
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
  const allDone = count > 0 && s.selectedRoots.every((root) => s.scans[root] && !s.scans[root].stopped);

  return {
    state: allDone ? "done" : "ready",
    title,
    hint: count > 1 ? `Scans ${count} selected drives sequentially.` : baseHint,
    label: allDone ? "Done · scan again" : "Ready",
    disabled: false,
  };
}

export function refreshAction(s: State): ActionView {
  const title = "Selective Refresh";
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
    return { state: "waiting", title, hint: "Wait for the scan to end.", label: "Waiting", disabled: true };
  }

  // Check if any selected drive is failing SMART
  const failingVolume = selectedVolumes(s).find((v) => v.smart.level === "failing");
  if (failingVolume) {
    return {
      state: "locked",
      title,
      hint: `SMART says ${failingVolume.root.slice(0, 2)} is failing. Refresh is locked for safety.`,
      label: "Blocked",
      disabled: true,
    };
  }

  const slowTotal = totalSlowAcrossSelected(s);
  if (slowTotal === 0) {
    const hasScanned = s.selectedRoots.length > 0 && s.selectedRoots.every((r) => s.scans[r]);
    return {
      state: "locked",
      title,
      hint: hasScanned ? "The scan found no slow files on selected drives." : "Run Safe Scan first.",
      label: "Locked",
      disabled: true,
    };
  }

  return {
    state: "ready",
    title: `Selective Refresh (${slowTotal.toLocaleString("en-US")} files)`,
    hint: `Rewrites ${slowTotal.toLocaleString("en-US")} slow files safely. You confirm first.`,
    label: "Ready",
    disabled: false,
  };
}
