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
  /** The newest refresh log of this drive. null = no refresh yet. */
  lastRefresh: LastRefresh | null;
}

export interface LastRefresh {
  logPath: string;
  /** The time of the last change to the log, in Unix seconds. */
  unixSecs: number;
}

export type Stage =
  | "find"
  | "read"
  | "settle"
  | "recheck"
  | "refresh"
  | "verify";

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
  /** true = fast when the scan read it again after the wait. null = not read again. */
  fastAfterWait: boolean | null;
}

export interface ScanOutcome {
  reportPath: string;
  totalFiles: number;
  slowTotal: number;
  /** Of the slow files: fast when the scan read them again after the wait. */
  fastAfterWait: number;
  /** Of the slow files: still slow when the scan read them again after the wait. */
  stillSlowAfterWait: number;
  slowFiles: SlowFile[];
  stopped: boolean;
}

export interface RefreshOutcome {
  logPath: string;
  movedFiles: number;
  stopped: boolean;
}

export type Verdict = "YES" | "NO" | "NOT DECIDED" | "n/a" | "";

export interface VerifyOutcome {
  logPath: string;
  processedFiles: number;
  /** false = the data of a file changed, but no program changed the file. */
  dataSafe: boolean;
  /** The Faster result of the rewritten files. Empty when no file was measured. */
  faster: Verdict;
  /** The time since the refresh, in seconds. */
  ageSecs: number;
  stopped: boolean;
}

export const api = {
  environment: () => invoke<{ elevated: boolean }>("environment"),
  listVolumes: () => invoke<Volume[]>("list_volumes"),
  startScan: (root: string) => invoke<ScanOutcome>("start_scan", { root }),
  startRefresh: (reportPath: string, root: string) =>
    invoke<RefreshOutcome>("start_refresh", { reportPath, root }),
  startVerify: (logPath: string) =>
    invoke<VerifyOutcome>("start_verify", { logPath }),
  /** Returns the new state: true means paused. */
  togglePause: () => invoke<boolean>("toggle_pause"),
  stop: () => invoke<void>("stop_operation"),
  windowMinimize: () => invoke<void>("window_minimize"),
  windowToggleMaximize: () => invoke<void>("window_toggle_maximize"),
  windowClose: () => invoke<void>("window_close"),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  writeAndOpenLog: (content: string) =>
    invoke<void>("write_and_open_log", { content }),
  openLogFolder: () => invoke<void>("open_log_folder"),
  saveLogAs: (content: string) =>
    invoke<string | null>("save_log_as", { content }),
  onProgress: (handler: (p: Progress) => void): Promise<UnlistenFn> =>
    listen<Progress>("progress", (event) => handler(event.payload)),
  onMessage: (handler: (message: string) => void): Promise<UnlistenFn> =>
    listen<string>("operation-message", (event) => handler(event.payload)),
};

export type Api = typeof api;
