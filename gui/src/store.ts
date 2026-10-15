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
  selectedRoot: string | null;
  mode: Mode;
  paused: boolean;
  stopping: boolean;
  progress: Progress | null;
  /** The last scan that finished. It belongs to one drive. */
  scan: { root: string; outcome: ScanOutcome } | null;
  notice: Notice | null;
}

export const initialState: State = {
  elevated: null,
  volumes: [],
  volumesLoading: true,
  selectedRoot: null,
  mode: "idle",
  paused: false,
  stopping: false,
  progress: null,
  scan: null,
  notice: null,
};

/** `prev` is null on the first call, when a view subscribes. */
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

export const selectedVolume = (s: State): Volume | null =>
  s.volumes.find((v) => v.root === s.selectedRoot) ?? null;

/** The scan result for the drive that is selected now, if it can be refreshed. */
export function refreshableScan(s: State): ScanOutcome | null {
  if (!s.scan || s.scan.root !== s.selectedRoot) return null;
  const { outcome } = s.scan;
  return !outcome.stopped && outcome.slowTotal > 0 ? outcome : null;
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
  const base = "Read-only. Finds slow files. Changes nothing.";
  if (s.mode === "scanning") {
    return { state: "running", title: "Scanning…", hint: base, label: "Running", disabled: true };
  }
  if (s.mode === "refreshing") {
    return { state: "waiting", title: "Safe Scan", hint: base, label: "Waiting", disabled: true };
  }
  if (!selectedVolume(s)) {
    return { state: "locked", title: "Safe Scan", hint: "Choose a drive first.", label: "No drive", disabled: true };
  }
  const done = s.scan?.root === s.selectedRoot && !s.scan?.outcome.stopped;
  return {
    state: done ? "done" : "ready",
    title: "Safe Scan",
    hint: base,
    label: done ? "Done · scan again" : "Ready",
    disabled: false,
  };
}

export function refreshAction(s: State): ActionView {
  const title = "Selective Refresh";
  if (s.mode === "refreshing") {
    return { state: "running", title: "Refreshing…", hint: "The file system moves each file safely.", label: "Running", disabled: true };
  }
  if (s.mode === "scanning") {
    return { state: "waiting", title, hint: "Wait for the scan to end.", label: "Waiting", disabled: true };
  }
  const volume = selectedVolume(s);
  if (volume?.smart.level === "failing") {
    return { state: "locked", title, hint: "SMART says this drive is failing. Refresh is off.", label: "Blocked", disabled: true };
  }
  const scan = refreshableScan(s);
  if (!scan) {
    const scanned = s.scan?.root === s.selectedRoot && !s.scan?.outcome.stopped;
    return {
      state: "locked",
      title,
      hint: scanned ? "The scan found no slow file. Nothing to rewrite." : "Run the scan first.",
      label: "Locked",
      disabled: true,
    };
  }
  return {
    state: "ready",
    title,
    hint: `Rewrites ${scan.slowTotal.toLocaleString("en-US")} slow files. You confirm first.`,
    label: "Ready",
    disabled: false,
  };
}
