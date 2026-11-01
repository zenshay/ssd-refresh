import type { Api, ScheduledVerify } from "./api";

interface ScheduleState {
  tasks: ScheduledVerify[];
  busy: boolean;
  message: string;
}

/** This model owns schedule requests and keeps errors visible. */
export function createSchedules(
  api: Pick<Api, "listSchedules" | "createVerifySchedule" | "removeSchedules">,
) {
  let state: ScheduleState = { tasks: [], busy: false, message: "" };
  const listeners = new Set<(s: ScheduleState) => void>();
  const set = (patch: Partial<ScheduleState>) => {
    state = { ...state, ...patch };
    for (const listener of listeners) listener(state);
  };
  const request = async (
    work: () => Promise<ScheduledVerify[]>,
    message: string,
  ) => {
    if (state.busy) return;
    set({ busy: true, message: "Reading Windows schedules..." });
    try {
      set({ tasks: await work(), message });
    } catch (e) {
      set({ message: e instanceof Error ? e.message : String(e) });
    } finally {
      set({ busy: false });
    }
  };
  return {
    subscribe(listener: (s: ScheduleState) => void) {
      listeners.add(listener);
      listener(state);
    },
    load: () =>
      request(
        () => api.listSchedules(),
        "Results: Folder > reports. A verify error also appears in scheduled-verify-error.log.",
      ),
    create: (path: string) =>
      request(
        () => api.createVerifySchedule(path),
        "Schedule saved. Only future checks remain: 1, 7 and 30 days after the refresh.",
      ),
    remove: () =>
      request(
        () => api.removeSchedules(),
        "All verify schedules removed. A verify that already started can finish.",
      ),
  };
}

export type Schedules = ReturnType<typeof createSchedules>;

export function taskResult(task: ScheduledVerify): string {
  if (task.state === "Running") return "Running";
  if (!task.lastRun) return "Not run";
  if (task.result === 0)
    return "Finished. Read the verify log for data and speed results.";
  if (task.result === 3) return "Data mismatch. Keep the files and logs.";
  if (task.result === 2)
    return "Unsupported: administrator or root rights are required, or the platform is not supported.";
  if (task.result === 4)
    return "Stopped. Read the verify log or run Verify manually.";
  if (task.result === 5)
    return "Incomplete: the tool could not check all data. Read the verify log or run Verify manually.";
  return `Did not finish (code ${task.result}). Read the error log or run Verify manually.`;
}
