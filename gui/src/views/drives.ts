import type { Volume } from "../api";
import type { Actions } from "../actions";
import { $, el } from "../dom";
import { fmtBytes } from "../format";
import type { Store } from "../store";

const SMART_TEXT = { healthy: "SMART OK", failing: "SMART FAILING", unknown: "SMART unknown" } as const;

function card(v: Volume, selected: boolean, disabled: boolean): HTMLButtonElement {
  const used = Math.max(0, v.totalBytes - v.freeBytes);
  const usedPct = v.totalBytes > 0 ? Math.min(100, (used / v.totalBytes) * 100) : 0;
  const bar = el("div", { class: "usage-fill" });
  bar.style.width = `${usedPct.toFixed(1)}%`;

  const button = el(
    "button",
    {
      class: "drive",
      attrs: {
        type: "button",
        role: "radio",
        "aria-checked": String(selected),
        "data-root": v.root,
        "data-total": String(v.totalBytes),
      },
    },
    el(
      "div",
      { class: "drive-top" },
      el("span", { class: "drive-letter", text: v.root.slice(0, 2) }),
      el("span", { class: "tag", text: v.fs }),
      v.isSystem ? el("span", { class: "tag tag-accent", text: "System" }) : "",
      el("span", { class: "pill", text: SMART_TEXT[v.smart.level], attrs: { "data-level": v.smart.level, title: v.smart.detail ?? "" } }),
    ),
    el("div", { class: "drive-model", text: v.model ?? "Local drive" }),
    el("div", { class: "usage" }, bar),
    el("div", { class: "drive-meta", text: `${fmtBytes(v.freeBytes)} free of ${fmtBytes(v.totalBytes)}` }),
  );
  button.disabled = disabled;
  return button;
}

export function mountDrives(store: Store, actions: Actions): void {
  const host = $("drives");
  const reload = $<HTMLButtonElement>("btn-reload");

  host.addEventListener("click", (event) => {
    const target = (event.target as HTMLElement).closest<HTMLElement>("[data-root]");
    if (target?.dataset.root) actions.selectDrive(target.dataset.root);
  });
  reload.addEventListener("click", () => void actions.loadVolumes());

  store.subscribe((s, prev) => {
    const changed =
      !prev || s.volumes !== prev.volumes || s.selectedRoot !== prev.selectedRoot ||
      s.mode !== prev.mode || s.volumesLoading !== prev.volumesLoading;
    if (!changed) return;

    reload.disabled = s.mode !== "idle" || s.volumesLoading;
    host.setAttribute("aria-busy", String(s.volumesLoading));
    if (s.volumes.length === 0) {
      host.replaceChildren(
        el("div", { class: "empty", text: s.volumesLoading ? "Reading the drive list…" : "No local drive found." }),
      );
      return;
    }
    host.replaceChildren(...s.volumes.map((v) => card(v, v.root === s.selectedRoot, s.mode !== "idle")));
  });
}
