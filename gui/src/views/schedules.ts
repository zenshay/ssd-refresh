import { $ } from "../dom";
import { type Schedules, taskResult } from "../schedules";
import { type Store, verifyTargets } from "../store";

export function mountSchedules(store: Store, model: Schedules): void {
  const panel = $("schedule-panel") as HTMLDialogElement;
  const create = $("schedule-create") as HTMLButtonElement;
  const remove = $("schedule-remove") as HTMLButtonElement;
  const reload = $("schedule-reload") as HTMLButtonElement;
  let busy = false;
  const paint = () => {
    const state = store.get();
    const targets = verifyTargets(state);
    create.disabled =
      busy ||
      state.mode !== "idle" ||
      targets.length !== 1 ||
      state.selectedRoots.length !== 1;
    $("schedule-target").textContent =
      targets.length === 1 && state.selectedRoots.length === 1
        ? `Target: ${targets[0].root}`
        : "Select one drive with a refresh log.";
  };
  $("schedule-open").addEventListener("click", () => {
    panel.showModal();
    void model.load();
  });
  $("schedule-close").addEventListener("click", () => panel.close());
  reload.addEventListener("click", () => void model.load());
  create.addEventListener("click", () => {
    if (!create.disabled) {
      const target = verifyTargets(store.get())[0]?.lastRefresh;
      if (target) void model.create(target.logPath);
    }
  });
  remove.addEventListener("click", () => void model.remove());
  store.subscribe(paint);
  model.subscribe((state) => {
    busy = state.busy;
    paint();
    remove.disabled = busy || state.tasks.length === 0;
    reload.disabled = busy;
    $("schedule-message").textContent = state.message;
    const list = $("schedule-list");
    list.replaceChildren();
    for (const task of state.tasks) {
      const row = document.createElement("li");
      const due = new Date(task.due).toLocaleString("en-US");
      row.textContent = `${task.name.split("-verify-")[1]?.replace(/^S-1-[\d-]+?-(?=scan-)/, "") ?? task.name}: ${due}. ${taskResult(task)}`;
      list.append(row);
    }
  });
}
