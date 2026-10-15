import type { SlowFile } from "../api";
import { $, el } from "../dom";
import { fmtBytes, fmtCount, fmtRate } from "../format";
import type { Store } from "../store";

export function mountSlowFiles(store: Store): void {
  const card = $("slow-files-card");
  const countEl = $("slow-files-count");
  const tableBody = $("slow-files-body");
  const reportPathEl = $("slow-report-path");

  store.subscribe((s) => {
    if (s.mode !== "idle") {
      card.hidden = true;
      return;
    }

    const allSlow: SlowFile[] = [];
    const reportPaths: string[] = [];

    for (const root of s.selectedRoots) {
      const outcome = s.scans[root];
      if (outcome && !outcome.stopped && outcome.slowTotal > 0) {
        allSlow.push(...outcome.slowFiles);
        reportPaths.push(outcome.reportPath);
      }
    }

    if (allSlow.length === 0) {
      card.hidden = true;
      return;
    }

    card.hidden = false;
    countEl.textContent = `${fmtCount(allSlow.length)} slow files detected`;
    reportPathEl.textContent = reportPaths.join(" · ");
    reportPathEl.title = reportPaths.join("\n");

    const rows = allSlow.slice(0, 50).map((file: SlowFile) => {
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
