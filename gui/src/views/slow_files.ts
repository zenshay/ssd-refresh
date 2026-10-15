import { $, el } from "../dom";
import { fmtBytes, fmtCount, fmtRate } from "../format";
import type { Store } from "../store";

export function mountSlowFiles(store: Store): void {
  const card = $("slow-files-card");
  const countEl = $("slow-files-count");
  const tableBody = $("slow-files-body");
  const reportPathEl = $("slow-report-path");

  store.subscribe((s) => {
    const outcome = s.scan?.outcome;
    const hasSlowFiles = Boolean(outcome && outcome.slowTotal > 0 && !outcome.stopped && s.mode === "idle");

    if (!hasSlowFiles || !outcome) {
      card.hidden = true;
      return;
    }

    card.hidden = false;
    countEl.textContent = `${fmtCount(outcome.slowTotal)} slow files detected`;
    reportPathEl.textContent = outcome.reportPath;
    reportPathEl.title = outcome.reportPath;

    // Render up to first 20 items in table to keep DOM fast
    const rows = outcome.slowFiles.slice(0, 50).map((file) => {
      return el(
        "tr",
        { class: "slow-row" },
        el("td", { class: "slow-cell-path", text: file.path, attrs: { title: file.path } }),
        el("td", { class: "slow-cell-size", text: fmtBytes(file.size) }),
        el("td", { class: "slow-cell-speed", text: fmtRate(file.mbps) })
      );
    });

    tableBody.replaceChildren(...rows);
  });
}
