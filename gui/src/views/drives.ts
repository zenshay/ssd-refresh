import type { Actions } from "../actions";
import type { Volume } from "../api";
import { $, el } from "../dom";
import { fmtBytes, refreshedText } from "../format";
import { activeDriveText, allSelected, isSelected, type Store } from "../store";

const SMART_TEXT = {
  healthy: "SMART OK",
  failing: "SMART FAILING",
  unknown: "SMART unknown",
} as const;

function card(
  v: Volume,
  selected: boolean,
  activeText: string | null,
  disabled: boolean,
  nowSecs: number,
): HTMLButtonElement {
  const active = activeText !== null;
  const used = Math.max(0, v.totalBytes - v.freeBytes);
  const usedPct =
    v.totalBytes > 0 ? Math.min(100, (used / v.totalBytes) * 100) : 0;
  const bar = el("div", { class: "usage-fill" });
  bar.style.width = `${usedPct.toFixed(1)}%`;

  // Custom checkbox visual indicator
  const checkbox = el("span", {
    class: `drive-checkbox ${selected ? "checked" : ""}`,
    attrs: { "aria-hidden": "true" },
  });

  const button = el(
    "button",
    {
      class: `drive ${selected ? "is-selected" : ""} ${active ? "is-active" : ""}`,
      attrs: {
        type: "button",
        role: "checkbox",
        "aria-checked": String(selected),
        "data-root": v.root,
      },
    },
    el(
      "div",
      { class: "drive-top" },
      checkbox,
      el("span", { class: "drive-letter", text: v.root.slice(0, 2) }),
      el("span", { class: "tag", text: v.fs }),
      v.isSystem ? el("span", { class: "tag tag-accent", text: "System" }) : "",
      el("span", {
        class: "pill",
        text: SMART_TEXT[v.smart.level],
        attrs: { "data-level": v.smart.level, title: v.smart.detail ?? "" },
      }),
    ),
    el("div", { class: "drive-model", text: v.model ?? "Local drive" }),
    el("div", { class: "usage" }, bar),
    el(
      "div",
      { class: "drive-meta" },
      el("span", {
        text: `${fmtBytes(v.freeBytes)} free of ${fmtBytes(v.totalBytes)}`,
      }),
      active
        ? el("span", { class: "drive-status-active", text: activeText })
        : v.lastRefresh
          ? el("span", {
              class: "drive-refreshed",
              text: refreshedText(nowSecs - v.lastRefresh.unixSecs),
            })
          : "",
    ),
  );

  button.disabled = disabled;
  return button;
}

export function mountDrives(store: Store, actions: Actions): void {
  const host = $("drives");
  const reload = $<HTMLButtonElement>("btn-reload");
  const selectAllBtn = $<HTMLButtonElement>("btn-select-all");

  host.addEventListener("click", (event) => {
    const target = (event.target as HTMLElement).closest<HTMLElement>(
      "[data-root]",
    );
    if (target?.dataset.root) {
      actions.toggleSelect(target.dataset.root);
    }
  });

  reload.addEventListener("click", () => void actions.loadVolumes());
  selectAllBtn.addEventListener("click", () => actions.toggleSelectAll());

  store.subscribe((s, prev) => {
    const changed =
      !prev ||
      s.volumes !== prev.volumes ||
      s.selectedRoots !== prev.selectedRoots ||
      s.activeRoot !== prev.activeRoot ||
      s.mode !== prev.mode ||
      s.volumesLoading !== prev.volumesLoading;

    if (!changed) return;

    const isRunning = s.mode !== "idle";
    reload.disabled = isRunning || s.volumesLoading;
    selectAllBtn.disabled =
      isRunning || s.volumesLoading || s.volumes.length === 0;

    const areAllSelected = allSelected(s);
    selectAllBtn.textContent = areAllSelected ? "Deselect All" : "Select All";

    host.setAttribute("aria-busy", String(s.volumesLoading));
    if (s.volumes.length === 0) {
      host.replaceChildren(
        el("div", {
          class: "empty",
          text: s.volumesLoading
            ? "Reading drive list…"
            : "No local drive found.",
        }),
      );
      return;
    }

    const nowSecs = Date.now() / 1000;
    host.replaceChildren(
      ...s.volumes.map((v) =>
        card(
          v,
          isSelected(s, v.root),
          s.activeRoot === v.root ? activeDriveText(s.mode) : null,
          isRunning,
          nowSecs,
        ),
      ),
    );
  });
}
