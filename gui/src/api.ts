// The contract with the Rust side. This is the only file that calls `invoke` or `listen`.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type SmartLevel = "healthy" | "failing" | "unknown";

export interface Volume {
  root: string;
  fs: string;
  totalBytes: number;
  freeBytes: number;
  isSystem: boolean;
  model: string | null;
  smart: { level: SmartLevel; detail: string | null };
}

export type Stage = "find" | "read" | "settle" | "recheck" | "refresh" | "verify";

export interface Progress {
  stage: Stage;
  filesDone: number;
  /** 0 when the total is not known. */
  totalFiles: number;
  bytesDone: number;
  /** 0 when the total is not known. */
  totalBytes: number;
  mbps: number;
  etaSecs: number | null;
}

export interface SlowFile {
  path: string;
  size: number;
  mbps: number;
}

export interface ScanOutcome {
  reportPath: string;
  totalFiles: number;
  slowTotal: number;
  slowFiles: SlowFile[];
  stopped: boolean;
}

export interface RefreshOutcome {
  logPath: string;
  movedFiles: number;
  stopped: boolean;
}

export const api = {
  environment: () => invoke<{ elevated: boolean }>("environment"),
  listVolumes: () => invoke<Volume[]>("list_volumes"),
  startScan: (root: string) => invoke<ScanOutcome>("start_scan", { root }),
  startRefresh: (reportPath: string, root: string) =>
    invoke<RefreshOutcome>("start_refresh", { reportPath, root }),
  /** Returns the new state: true means paused. */
  togglePause: () => invoke<boolean>("toggle_pause"),
  stop: () => invoke<void>("stop_operation"),
  windowMinimize: () => invoke<void>("window_minimize"),
  windowToggleMaximize: () => invoke<void>("window_toggle_maximize"),
  windowClose: () => invoke<void>("window_close"),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  onProgress: (handler: (p: Progress) => void): Promise<UnlistenFn> =>
    listen<Progress>("progress", (event) => handler(event.payload)),
};
