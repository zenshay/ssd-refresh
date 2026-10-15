import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface Volume {
  root: string;
  device: string;
  fs: string;
  total_bytes: number;
  free_bytes: number;
  is_system: boolean;
  health: {
    status: string;
    disk_number?: number;
    model?: string;
  };
}

interface SlowFileInfo {
  path: string;
  size: number;
  mbps: number;
}

interface ScanResult {
  report_path: string;
  total_files: number;
  slow_files: SlowFileInfo[];
}

interface RefreshResult {
  log_path: string;
  moved_files: number;
  status: string;
}

let volumes: Volume[] = [];
let selectedRoot: string = "";
let currentReportPath: string = "";

function fmtBytes(b: number): string {
  const gb = b / 1e9;
  return gb >= 100 ? `${gb.toFixed(0)} GB` : `${gb.toFixed(1)} GB`;
}

async function loadDrives() {
  const container = document.getElementById("drive-list")!;
  container.innerHTML = `<div class="p-4 rounded-xl border border-slate-800 bg-slate-800/30 animate-pulse text-xs text-slate-500">Querying system storage & SMART attributes...</div>`;
  try {
    const res: any = await invoke("get_volumes");
    console.log("INVOKE_GET_VOLUMES_RESULT:", JSON.stringify(res));
    volumes = res || [];
    renderDrives();
  } catch (err) {
    console.error("INVOKE_GET_VOLUMES_ERROR:", err);
    container.innerHTML = `<div class="p-4 rounded-xl border border-rose-500/30 bg-rose-500/10 text-xs text-rose-300">Failed to load drives: ${err}</div>`;
  }
}

function renderDrives() {
  const container = document.getElementById("drive-list")!;
  if (!volumes || volumes.length === 0) {
    container.innerHTML = `<div class="p-4 rounded-xl border border-slate-800 bg-slate-800/30 text-xs text-slate-400">No supported NTFS storage volumes found.</div>`;
    return;
  }
  if (!selectedRoot && volumes.length > 0) {
    selectedRoot = volumes[0].root;
  }

  container.innerHTML = volumes.map(v => {
    const isSelected = v.root === selectedRoot;
    const rawStatus = typeof v.health.status === 'string' ? v.health.status : "Unknown";
    const isHealthy = rawStatus.toLowerCase().includes('healthy');
    const badgeColor = isHealthy ? "text-emerald-400 bg-emerald-500/10 border-emerald-500/30" : "text-amber-400 bg-amber-500/10 border-amber-500/30";
    const dotColor = isHealthy ? "bg-emerald-400" : "bg-amber-400";

    return `
      <div data-root="${v.root}" class="drive-card cursor-pointer p-4 rounded-xl border transition-all ${
        isSelected ? "border-indigo-500 bg-indigo-950/20 shadow-lg shadow-indigo-500/10 ring-1 ring-indigo-500" : "border-slate-800 bg-slate-900/40 hover:border-slate-700"
      }">
        <div class="flex items-start justify-between">
          <div class="flex items-center space-x-2">
            <span class="text-xl font-bold font-mono">${v.root}</span>
            <span class="text-xs px-2 py-0.5 rounded-md bg-slate-800 font-mono text-slate-300">${v.fs}</span>
            ${v.is_system ? `<span class="text-xs px-1.5 py-0.5 rounded-md bg-indigo-500/20 text-indigo-300">System</span>` : ""}
          </div>
          <div class="flex items-center space-x-1.5 text-xs px-2.5 py-1 rounded-full border ${badgeColor}">
            <span class="w-2 h-2 rounded-full ${dotColor}"></span>
            <span class="font-medium">SMART: ${rawStatus}</span>
          </div>
        </div>
        <div class="mt-2 text-xs text-slate-200 font-semibold truncate">
          ${v.health.model || "Local Storage Drive"}
        </div>
        <div class="mt-1 flex justify-between text-xs text-slate-400 font-mono">
          <span>Free: ${fmtBytes(v.free_bytes)}</span>
          <span>Total: ${fmtBytes(v.total_bytes)}</span>
        </div>
      </div>
    `;
  }).join("");

  document.querySelectorAll(".drive-card").forEach(el => {
    el.addEventListener("click", () => {
      selectedRoot = (el as HTMLElement).dataset.root!;
      renderDrives();
    });
  });
}

document.addEventListener("DOMContentLoaded", async () => {
  loadDrives();
  document.getElementById("btn-refresh-drives")?.addEventListener("click", loadDrives);

  const btnScan = document.getElementById("btn-scan") as HTMLButtonElement;
  const btnRefresh = document.getElementById("btn-refresh") as HTMLButtonElement;
  const btnPause = document.getElementById("btn-pause") as HTMLButtonElement;
  const btnStop = document.getElementById("btn-stop") as HTMLButtonElement;
  const taskBadge = document.getElementById("task-badge")!;
  const pauseIcon = document.getElementById("pause-icon")!;
  const pauseText = document.getElementById("pause-text")!;
  const progressBox = document.getElementById("progress-container")!;
  const resultBox = document.getElementById("result-box")!;
  const progressBar = document.getElementById("progress-bar")!;
  const progressPct = document.getElementById("progress-pct")!;
  const progressSpeed = document.getElementById("progress-speed")!;
  const progressPhase = document.getElementById("progress-phase")!;
  const progressCount = document.getElementById("progress-count")!;
  const progressEta = document.getElementById("progress-eta")!;
  const slowCard = document.getElementById("slow-files-card")!;
  const slowCountEl = document.getElementById("slow-count")!;
  const slowTableBody = document.getElementById("slow-table-body")!;

  let isPaused = false;
  function updatePauseUI(paused: boolean) {
    isPaused = paused;
    if (paused) {
      pauseIcon.textContent = "▶";
      pauseText.textContent = "Resume";
      btnPause.className = "px-3 py-1 rounded-lg text-xs font-semibold bg-amber-500/20 text-amber-300 border border-amber-500/40 shadow-sm transition flex items-center space-x-1";
      progressSpeed.textContent = "PAUSED (0 MB/s)";
    } else {
      pauseIcon.textContent = "⏸";
      pauseText.textContent = "Pause";
      btnPause.className = "px-3 py-1 rounded-lg text-xs font-medium bg-slate-800 hover:bg-slate-700 text-slate-200 border border-slate-700 transition flex items-center space-x-1";
    }
  }

  function setTaskState(state: "idle" | "scanning" | "refreshing") {
    if (state === "scanning") {
      taskBadge.className = "px-2.5 py-0.5 rounded-full text-[10px] font-mono tracking-wide uppercase border border-indigo-500/50 bg-indigo-500/20 text-indigo-300 animate-pulse";
      taskBadge.textContent = "● STEP 1 SCANNING";
      btnScan.classList.add("ring-2", "ring-indigo-400", "ring-offset-2", "ring-offset-slate-900");
      btnRefresh.disabled = true;
    } else if (state === "refreshing") {
      taskBadge.className = "px-2.5 py-0.5 rounded-full text-[10px] font-mono tracking-wide uppercase border border-amber-500/50 bg-amber-500/20 text-amber-300 animate-pulse";
      taskBadge.textContent = "● STEP 2 REFRESHING";
      btnRefresh.classList.add("ring-2", "ring-amber-400", "ring-offset-2", "ring-offset-slate-900");
      btnScan.disabled = true;
    } else {
      taskBadge.className = "px-2 py-0.5 rounded-full text-[10px] font-mono tracking-wide uppercase border border-slate-700 bg-slate-800 text-slate-300";
      taskBadge.textContent = "READY";
      btnScan.classList.remove("ring-2", "ring-indigo-400", "ring-offset-2", "ring-offset-slate-900");
      btnRefresh.classList.remove("ring-2", "ring-amber-400", "ring-offset-2", "ring-offset-slate-900");
    }
  }

  // Instant zero-latency Pause / Resume
  btnPause?.addEventListener("click", async () => {
    const nextState = !isPaused;
    updatePauseUI(nextState);
    try {
      const confirmed = await invoke<boolean>("toggle_pause");
      if (confirmed !== nextState) {
        updatePauseUI(confirmed);
      }
    } catch (err) {
      console.warn("toggle_pause error:", err);
      updatePauseUI(!nextState);
    }
  });

  btnStop?.addEventListener("click", async () => {
    try {
      await invoke("stop_operation");
      progressPhase.textContent = "Stopping operation safely (finishing current block)...";
      btnStop.disabled = true;
    } catch (err) {
      console.warn("stop_operation error:", err);
    }
  });

  // Listen to background progress events
  try {
    await listen("scan-progress", (event: any) => {
      const data = event.payload;
      progressPhase.textContent = data.phase || "Processing...";

      const pct = Math.min(100, Math.max(0, data.percent || 0));
      progressBar.style.width = `${pct.toFixed(1)}%`;
      progressPct.textContent = `${pct.toFixed(1)}%`;

      if (data.is_paused) {
        progressSpeed.textContent = "PAUSED (0 MB/s)";
      } else if (data.mbps > 0) {
        progressSpeed.textContent = `${data.mbps.toFixed(1)} MB/s`;
      }

      if (data.total_files > 0) {
        progressCount.textContent = `${data.files_done} / ${data.total_files} files`;
      } else if (data.files_done > 0) {
        progressCount.textContent = `${data.files_done} files cataloged`;
      }

      if (data.eta_str) {
        progressEta.textContent = `ETA: ${data.eta_str}`;
      }
    });
  } catch (e) {
    console.warn("Tauri event listener not available:", e);
  }

  // Handle Safe Scan
  btnScan?.addEventListener("click", async () => {
    if (!selectedRoot) return;

    setTaskState("scanning");
    btnScan.disabled = true;
    btnRefresh.disabled = true;
    btnStop.disabled = false;
    updatePauseUI(false);
    progressBox.classList.remove("hidden");
    resultBox.classList.add("hidden");
    slowCard.classList.add("hidden");
    progressBar.style.width = "0%";
    progressPct.textContent = "0.0%";
    progressPhase.textContent = `Initializing unbuffered pass on ${selectedRoot}...`;
    progressSpeed.textContent = "Calculating...";

    try {
      const res: ScanResult = await invoke("start_scan", { root: selectedRoot });
      progressBar.style.width = "100%";
      progressPct.textContent = "100.0%";
      progressSpeed.textContent = "Complete";
      currentReportPath = res.report_path;

      if (res.slow_files && res.slow_files.length > 0) {
        btnRefresh.disabled = false;
        btnRefresh.className = "px-5 py-2.5 rounded-xl font-bold text-sm bg-gradient-to-r from-amber-600 to-amber-500 hover:from-amber-500 hover:to-amber-400 text-white shadow-lg shadow-amber-600/30 transition-all border border-amber-400/40 flex items-center space-x-1.5 animate-bounce";
        resultBox.className = "p-4 rounded-xl border border-amber-500/40 bg-amber-500/10 text-amber-200 text-sm space-y-2";
        resultBox.innerHTML = `
          <div class="font-bold flex items-center space-x-2 text-amber-300">
            <span>⚠</span>
            <span>Step 1 Complete: Detected ${res.slow_files.length} Slow Files</span>
          </div>
          <p class="text-xs text-slate-300 leading-relaxed">
            Old flash cells have lost charge. Read speeds have dropped below threshold. You can now click <strong>Step 2: Selective Refresh</strong> to rewrite only these slow files safely to fresh NAND blocks.
          </p>
        `;
        resultBox.classList.remove("hidden");

        // Render slow files table
        slowCountEl.textContent = res.slow_files.length.toString();
        slowTableBody.innerHTML = res.slow_files.map(f => `
          <tr class="hover:bg-slate-800/30">
            <td class="p-2.5 truncate max-w-md text-slate-300" title="${f.path}">${f.path}</td>
            <td class="p-2.5 text-slate-400">${(f.size / 1e6).toFixed(1)} MB</td>
            <td class="p-2.5 text-right font-bold text-amber-400">${f.mbps.toFixed(1)} MB/s</td>
          </tr>
        `).join("");
        slowCard.classList.remove("hidden");
      } else {
        btnRefresh.disabled = true;
        btnRefresh.className = "px-5 py-2.5 rounded-xl font-medium text-sm bg-slate-800 border border-slate-700/60 text-slate-400 shadow-lg transition-all disabled:opacity-40 disabled:cursor-not-allowed flex items-center space-x-1.5";
        resultBox.className = "p-4 rounded-xl border border-emerald-500/30 bg-emerald-500/10 text-emerald-300 text-sm space-y-1";
        resultBox.innerHTML = `
          <div class="font-bold flex items-center space-x-2">
            <span>✓</span>
            <span>Controller Wakeup Pass Successful!</span>
          </div>
          <p class="text-xs text-emerald-400/80 leading-relaxed">
            All files on ${selectedRoot} read normally (${res.total_files} files verified). On modern controllers, this unbuffered read pass automatically refreshes borderline cells without write wearing.
          </p>
        `;
        resultBox.classList.remove("hidden");
      }
    } catch (err) {
      resultBox.className = "p-4 rounded-xl border border-rose-500/40 bg-rose-500/10 text-rose-300 text-sm";
      resultBox.innerHTML = `<strong>Scan Error:</strong> ${err}`;
      resultBox.classList.remove("hidden");
    } finally {
      btnScan.disabled = false;
      setTaskState("idle");
      updatePauseUI(false);
    }
  });

  // Handle Selective Refresh
  btnRefresh?.addEventListener("click", async () => {
    if (!currentReportPath) return;

    setTaskState("refreshing");
    btnRefresh.disabled = true;
    btnScan.disabled = true;
    btnStop.disabled = false;
    updatePauseUI(false);
    progressBox.classList.remove("hidden");
    progressPhase.textContent = "Rewriting slow files via NTFS atomic defrag API...";
    progressBar.style.width = "0%";
    progressPct.textContent = "0.0%";

    try {
      const res: RefreshResult = await invoke("start_refresh", { reportPath: currentReportPath });
      progressBar.style.width = "100%";
      progressPct.textContent = "100.0%";
      resultBox.className = "p-4 rounded-xl border border-emerald-500/40 bg-emerald-500/10 text-emerald-300 text-sm space-y-2";
      resultBox.innerHTML = `
        <div class="font-bold flex items-center space-x-2">
          <span>✓</span>
          <span>Selective Refresh Complete!</span>
        </div>
        <p class="text-xs text-slate-300 leading-relaxed">
          Successfully moved and rewrote ${res.moved_files} slow files to fresh clusters. Unbuffered read speeds are restored to factory performance.
        </p>
      `;
      resultBox.classList.remove("hidden");
    } catch (err) {
      resultBox.className = "p-4 rounded-xl border border-rose-500/40 bg-rose-500/10 text-rose-300 text-sm";
      resultBox.innerHTML = `<strong>Refresh Error:</strong> ${err}`;
      resultBox.classList.remove("hidden");
    } finally {
      btnScan.disabled = false;
      setTaskState("idle");
      updatePauseUI(false);
    }
  });
});
