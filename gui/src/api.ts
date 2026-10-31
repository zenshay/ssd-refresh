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
export type DataSafety = "YES" | "NO" | "UNKNOWN" | "NOT_CHECKED";

export interface VerifyOutcome {
  logPath: string;
  processedFiles: number;
  /** This is separate from completion and speed. */
  dataSafety: DataSafety;
  /** Files that verify could not compare. dataSafety does not cover them. */
  uncheckedFiles: number;
  /** The Faster result of the rewritten files. Empty when no file was measured. */
  faster: Verdict;
  /** The time since the refresh, in seconds. */
  ageSecs: number;
  /** The files that were slow before. null when no file was slow before. */
  speed: VerifySpeed | null;
  /** The time until Faster can show YES or NO, in seconds. 0 when it can now. */
  waitSecs: number;
  stopped: boolean;
}

export interface VerifySpeed {
  beforeMbps: number;
  nowMbps: number;
  slowBefore: number;
  slowNow: number;
}

/** Reject an invalid IPC result before the page can show a safety claim. */
export function validateVerifyOutcome(value: unknown): VerifyOutcome {
  const fail = (): never => {
    throw new Error("Invalid verify result. Keep the log and restart the app.");
  };
  if (typeof value !== "object" || value === null || Array.isArray(value))
    return fail();
  const r = value as Record<string, unknown>;
  const number = (v: unknown): v is number =>
    typeof v === "number" && Number.isFinite(v) && v >= 0;
  const count = (v: unknown): v is number =>
    number(v) && Number.isSafeInteger(v);
  if (
    typeof r.logPath !== "string" ||
    r.logPath.length === 0 ||
    !count(r.processedFiles) ||
    !count(r.uncheckedFiles) ||
    r.uncheckedFiles > r.processedFiles ||
    !count(r.ageSecs) ||
    !count(r.waitSecs) ||
    typeof r.stopped !== "boolean" ||
    typeof r.dataSafety !== "string" ||
    !["YES", "NO", "UNKNOWN", "NOT_CHECKED"].includes(r.dataSafety) ||
    typeof r.faster !== "string" ||
    !["YES", "NO", "NOT DECIDED", "n/a", ""].includes(r.faster) ||
    // YES needs at least one file that verify compared.
    (r.dataSafety === "YES" &&
      (r.uncheckedFiles >= r.processedFiles || r.stopped))
  )
    return fail();
  if (r.speed !== null) {
    if (typeof r.speed !== "object" || !r.speed || Array.isArray(r.speed))
      return fail();
    const s = r.speed as Record<string, unknown>;
    if (
      !number(s.beforeMbps) ||
      !number(s.nowMbps) ||
      !count(s.slowBefore) ||
      !count(s.slowNow) ||
      s.slowNow > s.slowBefore ||
      s.slowBefore > r.processedFiles
    )
      return fail();
  }
  return value as VerifyOutcome;
}

export const api = {
  environment: () => invoke<{ elevated: boolean }>("environment"),
  listVolumes: () => invoke<Volume[]>("list_volumes"),
  startScan: (root: string) => invoke<ScanOutcome>("start_scan", { root }),
  startRefresh: (reportPath: string, root: string) =>
    invoke<RefreshOutcome>("start_refresh", { reportPath, root }),
  startVerify: (logPath: string) =>
    invoke<unknown>("start_verify", { logPath }).then(validateVerifyOutcome),
  /** Returns the new state: true means paused. */
  togglePause: () => invoke<boolean>("toggle_pause"),
  stop: () => invoke<void>("stop_operation"),
  windowMinimize: () => invoke<void>("window_minimize"),
  windowToggleMaximize: () => invoke<void>("window_toggle_maximize"),
  windowClose: () => invoke<void>("window_close"),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  writeAndOpenLog: (content: string) =>
    invoke<void>("write_and_open_log", { content }),
  openDataFolder: () => invoke<void>("open_data_folder"),
  saveLogAs: (content: string) =>
    invoke<string | null>("save_log_as", { content }),
  onProgress: (handler: (p: Progress) => void): Promise<UnlistenFn> =>
    listen<Progress>("progress", (event) => handler(event.payload)),
  onMessage: (handler: (message: string) => void): Promise<UnlistenFn> =>
    listen<string>("operation-message", (event) => handler(event.payload)),
};

export type Api = typeof api;
