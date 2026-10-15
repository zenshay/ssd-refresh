// Native <dialog> confirm prompt. Traps focus and closes on Esc.
import { $ } from "../dom";

export interface ConfirmOptions {
  title: string;
  body: string;
  warning?: string;
  okLabel: string;
}

export function mountConfirm(): void {
  const dialog = $<HTMLDialogElement>("confirm-modal");
  const cancelBtn = $("btn-confirm-cancel");
  const okBtn = $("btn-confirm-ok");

  cancelBtn.addEventListener("click", () => {
    dialog.close("cancel");
  });

  okBtn.addEventListener("click", () => {
    dialog.close("ok");
  });
}

export function askConfirm(options: ConfirmOptions): Promise<boolean> {
  const dialog = $<HTMLDialogElement>("confirm-modal");
  $("confirm-title").textContent = options.title;
  $("confirm-body").textContent = options.body;
  const warning = $("confirm-warning");
  warning.textContent = options.warning ?? "";
  warning.hidden = !options.warning;
  $("btn-confirm-ok").textContent = options.okLabel;
  dialog.returnValue = "cancel";

  return new Promise((resolve) => {
    dialog.addEventListener("close", () => resolve(dialog.returnValue === "ok"), { once: true });
    dialog.showModal();
  });
}
