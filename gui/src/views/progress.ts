import type { Actions } from "../actions";
import type { Stage } from "../api";
import { $ } from "../dom";
import { countOf, fmtBytes, fmtCount, fmtDuration, fmtRate } from "../format";
import { setIcon } from "../icons";
import type { Store } from "../store";

interface StageInfo {
  step: string;
  title: string;
  indeterminate: boolean;
}

function inspectStage(stage: Stage): StageInfo {
  switch (stage) {
    case "find":
      return {
        step: "Step 1/3",
        title: "Find files to scan",
        indeterminate: true,
      };
    case "read":
      return {
        step: "Step 2/3",
        title: "Read and measure speeds",
        indeterminate: false,
      };
    case "settle":
      return {
        step: "Step 3/3",
        title: "Drive settle wait",
        indeterminate: true,
      };
    case "refresh":
      return {
        step: "Step 2/3",
        title: "Rewrite slow files",
        indeterminate: false,
      };
    case "recheck":
      return {
        step: "Step 3/3",
        title: "Recheck read speeds",
        indeterminate: false,
      };
    case "verify":
      return {
        step: "Step 2/2",
        title: "Read the refreshed files again",
        indeterminate: false,
      };
    default:
      return {
        step: "Working",
        title: "Processing disk data",
        indeterminate: false,
      };
  }
}

export function mountProgress(store: Store, actions: Actions): void {
  const card = $("progress-card");
  const stepChip = $("progress-step-chip");
  const stepIcon = $("progress-step-icon");
  const titleEl = $("progress-title");
  const subtitleEl = $("progress-subtitle");
  const pausedBadge = $("badge-paused");
  const track = $("progress-track");
  const bar = $("progress-fill");

  const statFiles = $("stat-files");
  const statBytes = $("stat-bytes");
  const statSpeed = $("stat-speed");
  const statEta = $("stat-eta");

  const btnPause = $<HTMLButtonElement>("btn-pause");
  const btnStop = $<HTMLButtonElement>("btn-stop");
  const pauseIcon = $("btn-pause-icon");
  const pauseLabel = $("btn-pause-label");

  btnPause.addEventListener("click", () => void actions.togglePause());
  btnStop.addEventListener("click", () => void actions.stop());

  setIcon(stepIcon, "spinner", 16);
  setIcon($("btn-stop-icon"), "stop", 14);

  store.subscribe((s) => {
    const isRunning = s.mode !== "idle";
    card.hidden = false;
    card.classList.toggle("is-idle", !isRunning);
    card.dataset.state = s.mode;

    if (!isRunning) {
      card.classList.remove("is-paused", "is-stopping");
      btnPause.disabled = true;
      btnStop.disabled = true;
      btnPause.dataset.paused = "false";
      setIcon(pauseIcon, "pause", 14);
      pauseLabel.textContent = "Pause";
      btnStop.textContent = "Stop";
      pausedBadge.hidden = true;
      stepChip.textContent = "Standby";
      stepChip.hidden = false;
      stepIcon.hidden = true;
      titleEl.textContent = "Operation Progress";
      // Stays true after an operation too. The notice shows the result.
      subtitleEl.textContent = "Standby · No operation is running";
      bar.style.width = "0%";
      track.classList.remove("indeterminate");
      statFiles.textContent = "--";
      statBytes.textContent = "--";
      statSpeed.textContent = "--";
      statEta.textContent = "--";
      return;
    }

    // Pause and stopping state
    const isPaused = s.paused;
    const isStopping = s.stopping;
    card.classList.toggle("is-paused", isPaused);
    card.classList.toggle("is-stopping", isStopping);

    btnPause.dataset.paused = String(isPaused);
    btnPause.disabled = isStopping;
    btnStop.disabled = isStopping;
    btnStop.textContent = isStopping ? "Stopping…" : "Stop";
    setIcon(pauseIcon, isPaused ? "play" : "pause", 14);
    pauseLabel.textContent = isPaused ? "Resume" : "Pause";
    pausedBadge.hidden = !isPaused;

    const p = s.progress;
    if (!p) {
      stepChip.textContent = "Starting…";
      stepIcon.hidden = true;
      titleEl.textContent =
        s.mode === "scanning"
          ? "Starting safe scan…"
          : s.mode === "verifying"
            ? "Starting verify…"
            : "Starting selective refresh…";
      subtitleEl.textContent = "Checking disk...";
      track.classList.add("indeterminate");
      bar.style.width = "0%";
      statFiles.textContent = "--";
      statBytes.textContent = "--";
      statSpeed.textContent = "--";
      statEta.textContent = "--";
      return;
    }

    const info = inspectStage(p.stage);
    stepChip.textContent = info.step;
    stepChip.hidden = info.indeterminate;
    stepIcon.hidden = !info.indeterminate;
    titleEl.textContent = info.title;

    const driveIdx = s.activeRoot
      ? s.selectedRoots.indexOf(s.activeRoot) + 1
      : 0;
    const drivePrefix =
      s.selectedRoots.length > 1 && s.activeRoot
        ? `[${driveIdx}/${s.selectedRoots.length}] ${s.activeRoot.slice(0, 2)} · `
        : "";

    if (info.indeterminate) {
      track.classList.add("indeterminate");
      bar.style.width = "0%";
      subtitleEl.textContent = `${drivePrefix}${countOf(p.filesDone, "file")} found so far…`;
      statFiles.textContent = countOf(p.filesDone, "file");
      statBytes.textContent = fmtBytes(p.bytesDone);
      statSpeed.textContent = fmtRate(p.mbps);
      statEta.textContent = "--";
    } else {
      track.classList.remove("indeterminate");
      const pct =
        p.totalBytes > 0
          ? Math.min(100, Math.max(0, (p.bytesDone / p.totalBytes) * 100))
          : 0;
      bar.style.width = `${pct.toFixed(1)}%`;

      subtitleEl.textContent = `${drivePrefix}${fmtBytes(p.bytesDone)} of ${fmtBytes(p.totalBytes)} (${pct.toFixed(1)}%)`;
      statFiles.textContent =
        p.totalFiles > 0
          ? `${fmtCount(p.filesDone)} / ${fmtCount(p.totalFiles)}`
          : fmtCount(p.filesDone);
      statBytes.textContent =
        p.totalBytes > 0
          ? `${fmtBytes(p.bytesDone)} / ${fmtBytes(p.totalBytes)}`
          : fmtBytes(p.bytesDone);
      statSpeed.textContent = fmtRate(p.mbps);
      statEta.textContent = isPaused
        ? "Paused"
        : p.etaSecs !== null
          ? fmtDuration(p.etaSecs)
          : "--";
    }
  });
}
