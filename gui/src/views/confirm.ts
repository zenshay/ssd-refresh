// The confirm dialog. It uses the native <dialog> element: it traps focus and closes with Esc.
import { $ } from "../dom";

export interface ConfirmOptions {
  title: string;
  body: string;
  warning?: string;
  okLabel: string;
}

export function askConfirm(options: ConfirmOptions): Promise<boolean> {
  const dialog = $<HTMLDialogElement>("confirm");
  $("confirm-title").textContent = options.title;
  $("confirm-body").textContent = options.body;
  const warning = $("confirm-warning");
  warning.textContent = options.warning ?? "";
  warning.hidden = !options.warning;
  $("confirm-ok").textContent = options.okLabel;
  dialog.returnValue = "cancel";

  return new Promise((resolve) => {
    dialog.addEventListener("close", () => resolve(dialog.returnValue === "ok"), { once: true });
    dialog.showModal();
  });
}
