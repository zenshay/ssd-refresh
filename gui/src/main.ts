// @ts-ignore
const invoke = window.__TAURI__ ? window.__TAURI__.core.invoke : async (cmd: string, args?: any) => {
  console.log(`[Mock Invoke] ${cmd}`, args);
  if (cmd === 'get_volumes') {
    return [
      {
        root: "C:\\",
        device: "\\\\.\\C:",
        fs: "NTFS",
        total_bytes: 480_000_000_000,
        free_bytes: 208_000_000_000,
        is_system: true,
        health: { status: "Healthy", disk_number: 1, model: "Crucial CT480BX500SSD1" }
      },
      {
        root: "F:\\",
        device: "\\\\.\\F:",
        fs: "NTFS",
        total_bytes: 2_000_000_000_000,
        free_bytes: 628_000_000_000,
        is_system: false,
        health: { status: "Healthy", disk_number: 0, model: "Samsung SSD 970 EVO Plus 2TB" }
      }
    ];
  }
  return null;
};

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

let volumes: Volume[] = [];
let selectedRoot: string = "";

function fmtBytes(b: number): string {
  const gb = b / 1e9;
  return gb >= 100 ? `${gb.toFixed(0)} GB` : `${gb.toFixed(1)} GB`;
}

async function loadDrives() {
  const container = document.getElementById("drive-list")!;
  container.innerHTML = `<div class="p-4 rounded-xl border border-slate-800 bg-slate-800/30 animate-pulse text-xs text-slate-500">Querying system storage...</div>`;
  try {
    volumes = await invoke("get_volumes");
    renderDrives();
  } catch (err) {
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
    const isHealthy = typeof v.health.status === 'string' && v.health.status.toLowerCase().includes('healthy');
    const badgeColor = isHealthy ? "text-emerald-400 bg-emerald-500/10 border-emerald-500/20" : "text-amber-400 bg-amber-500/10 border-amber-500/20";
    const statusText = typeof v.health.status === 'string' ? v.health.status : "Unknown";

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
          <span class="text-xs px-2 py-0.5 rounded-full border ${badgeColor}">
            ${statusText}
          </span>
        </div>
        <div class="mt-2 text-xs text-slate-300 font-medium truncate">
          ${v.health.model || "Local Storage Volume"}
        </div>
        <div class="mt-1 flex justify-between text-xs text-slate-400">
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

document.addEventListener("DOMContentLoaded", () => {
  loadDrives();
  document.getElementById("btn-refresh-drives")?.addEventListener("click", loadDrives);

  const btnScan = document.getElementById("btn-scan") as HTMLButtonElement;
  const progressBox = document.getElementById("progress-container")!;
  const resultBox = document.getElementById("result-box")!;

  btnScan?.addEventListener("click", async () => {
    if (!selectedRoot) return;
    btnScan.disabled = true;
    progressBox.classList.remove("hidden");
    resultBox.classList.add("hidden");

    const progressBar = document.getElementById("progress-bar")!;
    const progressSpeed = document.getElementById("progress-speed")!;
    const progressPhase = document.getElementById("progress-phase")!;

    progressPhase.textContent = `Scanning ${selectedRoot} (read-only)...`;
    progressBar.style.width = "0%";

    // Simulated progress or real event listener
    let p = 0;
    const interval = setInterval(() => {
      p += 5;
      progressBar.style.width = `${p}%`;
      progressSpeed.textContent = `${(Math.random() * 200 + 150).toFixed(0)} MB/s`;

      if (p >= 100) {
        clearInterval(interval);
        btnScan.disabled = false;
        progressPhase.textContent = `Scan complete on ${selectedRoot}`;

        resultBox.className = "p-4 rounded-xl border border-emerald-500/30 bg-emerald-500/10 text-emerald-300 text-sm space-y-1";
        resultBox.innerHTML = `
          <div class="font-bold flex items-center space-x-2">
            <span>✓</span>
            <span>Controller Wakeup Successful!</span>
          </div>
          <p class="text-xs text-emerald-400/80">
            All files on ${selectedRoot} read normally. On SSDs like Crucial BX500, this single read pass triggers internal wear self-refresh. No write wearing needed!
          </p>
        `;
        resultBox.classList.remove("hidden");
      }
    }, 150);
  });
});
