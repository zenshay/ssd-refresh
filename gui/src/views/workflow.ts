import type { Actions } from "../actions";
import { $ } from "../dom";
import { setIcon } from "../icons";
import {
  type ActionView,
  refreshAction,
  type Store,
  scanAction,
  totalSlowAcrossSelected,
  verifyAction,
} from "../store";

function paint(button: HTMLElement, view: ActionView): void {
  button.dataset.state = view.state;
  (button as HTMLButtonElement).disabled = view.disabled;
  button.querySelector(".action-title")!.textContent = view.title;
  button.querySelector(".action-hint")!.textContent = view.hint;
  button.querySelector(".action-label")!.textContent = view.label;
  button.setAttribute("aria-disabled", String(view.disabled));
}

/** The three big step buttons, the mode chip and the header badge. */
export function mountWorkflow(store: Store, actions: Actions): void {
  const app = $("app");
  const scan = $("btn-scan");
  const refresh = $("btn-refresh");
  const verify = $("btn-verify");
  const chip = $("mode-chip");

  scan.addEventListener("click", () => void actions.runScan());
  refresh.addEventListener("click", () => void actions.runRefresh());
  verify.addEventListener("click", () => void actions.runVerify());
  setIcon(scan.querySelector<HTMLElement>(".action-icon")!, "scan", 20);
  setIcon(refresh.querySelector<HTMLElement>(".action-icon")!, "rewrite", 20);
  setIcon(verify.querySelector<HTMLElement>(".action-icon")!, "verify", 20);

  store.subscribe((s) => {
    app.dataset.mode = s.mode;
    // Tell the user which step is next. The next step gets the strong color.
    const hasSlow = totalSlowAcrossSelected(s) > 0;
    app.dataset.next = hasSlow && s.mode === "idle" ? "refresh" : "scan";
    paint(scan, scanAction(s));
    paint(refresh, refreshAction(s));
    paint(verify, verifyAction(s));

    const chipText = {
      idle: "Ready",
      scanning: "Scan is running",
      refreshing: "Refresh is running",
      verifying: "Verify is running",
    }[s.mode];
    chip.textContent = chipText;
    chip.dataset.tone = s.mode;
  });
}
