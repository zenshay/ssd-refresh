import type { SlowFile } from "../api";
import { $, el } from "../dom";
import { countOf, fmtBytes, fmtCount, fmtRate, truncatePath } from "../format";
import { type Store, stillSlow } from "../store";

export function mountSlowFiles(store: Store): void {
  const card = $("slow-files-card");
  const countEl = $("slow-files-count");
  const tableBody = $("slow-files-body");
  const reportPathEl = $("slow-report-path");

  // The click handler and the store both use these values.
  let reportText = "";
  let reportLabel = "";
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;

  const copyReportPaths = async (): Promise<void> => {
    if (!reportText) return;
    try {
      await navigator.clipboard.writeText(reportText);
      reportPathEl.textContent = "Copied ✓";
    } catch {
      reportPathEl.textContent = "Cannot copy the report path";
    }
    // Show the paths again after 1.5 s. A second click restarts the timer.
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => {
      reportPathEl.textContent = reportLabel;
    }, 1500);
  };

  reportPathEl.addEventListener("click", () => {
    void copyReportPaths();
  });
  reportPathEl.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();
    void copyReportPaths();
  });

  store.subscribe((s) => {
    if (s.mode !== "idle") {
      card.hidden = true;
      return;
    }

    // List only the files that stayed slow after the wait.
    const allSlow: SlowFile[] = [];
    const reportPaths: string[] = [];
    let slowCount = 0;
    let fastAfterWait = 0;

    for (const root of s.selectedRoots) {
      const outcome = s.scans[root];
      if (outcome && !outcome.stopped && outcome.slowTotal > 0) {
        allSlow.push(
          ...outcome.slowFiles.filter((f) => f.fastAfterWait !== true),
        );
        reportPaths.push(outcome.reportPath);
        slowCount += stillSlow(outcome);
        fastAfterWait += outcome.fastAfterWait;
      }
    }

    if (slowCount === 0 && fastAfterWait === 0) {
      card.hidden = true;
      return;
    }

    card.hidden = false;
    countEl.textContent =
      `${countOf(slowCount, "slow file")} detected` +
      (fastAfterWait > 0
        ? ` · ${fmtCount(fastAfterWait)} fast after the wait`
        : "");
    reportText = reportPaths.join("\n");
    reportLabel = reportPaths.join(" · ");
    clearTimeout(copiedTimer);
    reportPathEl.textContent = reportLabel;
    reportPathEl.title = `${reportText}\nClick to copy the report path.`;

    const rows = allSlow.slice(0, 50).map((file: SlowFile) => {
      return el(
        "tr",
        { class: "slow-row" },
        el("td", {
          class: "slow-cell-path",
          text: truncatePath(file.path, 50),
          attrs: { title: file.path },
        }),
        el("td", { class: "slow-cell-size", text: fmtBytes(file.size) }),
        el("td", { class: "slow-cell-speed", text: fmtRate(file.mbps) }),
      );
    });

    tableBody.replaceChildren(...rows);
  });
}
