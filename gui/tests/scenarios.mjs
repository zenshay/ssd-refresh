// Synthetic GUI states for screens.mjs and layout.mjs. No real drive data.
// The numbers agree between scenarios: the scan finds 300 slow files of 4,800,
// the refresh moves them, and verify reads the same 300 files again.
import { installIpc } from "./harness.mjs";

const DAY = 86400;
const now = Math.floor(Date.now() / 1000);
const volume = (root, patch = {}) => ({
  root,
  fs: "NTFS",
  totalBytes: 1.8e12,
  freeBytes: 1.2e11,
  isSystem: false,
  model: "Synthetic SSD",
  smart: { level: "healthy", detail: null },
  lastRefresh: null,
  ...patch,
});
const fresh = volume("G:\\");
const refreshedAgo = (secs) =>
  volume("G:\\", {
    lastRefresh: { logPath: "G:\\synthetic-refresh.csv", unixSecs: now - secs },
  });
const refreshed = refreshedAgo(21 * DAY);

// Old data on an SSD reads at 5–60 MB/s. Fast files read at 500 MB/s or more.
const scanOf = (root, slow, total) => ({
  reportPath: `${root}ssd-refresh-logs\\scan-${root[0]}-synthetic.csv`,
  totalFiles: total,
  slowTotal: slow,
  fastAfterWait: 0,
  stillSlowAfterWait: slow,
  slowFiles: Array.from({ length: slow }, (_, i) => ({
    path: `${root}Photos\\${2016 + (i % 5)}\\IMG_${String(4000 + i).padStart(5, "0")}.jpg`,
    size: 2_000_000 + ((i * 7919) % 9_000_000),
    mbps: 5 + ((i * 37) % 55),
    fastAfterWait: false,
  })),
  stopped: false,
});
const scan = scanOf("G:\\", 300, 4800);
const messages = [
  "Step 1/3  Find the files",
  "Found 4,800 files (38.2 GB).",
  "Step 2/3  Read each file one time",
  "Step 3/3  Check the files on the check list again",
];
const verify = (patch) => ({
  logPath: "G:\\synthetic-refresh.verify.log",
  processedFiles: 300,
  dataSafety: "YES",
  uncheckedFiles: 0,
  mismatchedFiles: 0,
  faster: "YES",
  ageSecs: 21 * DAY,
  speed: { beforeMbps: 25, nowMbps: 414, slowBefore: 300, slowNow: 3 },
  waitSecs: 0,
  stopped: false,
  ...patch,
});

// Steps: click a selector, then wait until a selector matches a visible element.
const NOTICE = "#notice-card:not([hidden])";
const scanStep = { click: "#btn-scan", until: NOTICE };
const refreshStep = { click: "#btn-refresh", until: "#confirm-modal[open]" };
const confirmStep = (until) => ({ click: "#btn-confirm-ok", until });

/** Each scenario: the IPC data and the steps. */
export const SCENARIOS = {
  idle: { data: { volumes: [fresh] } },
  "scan-many-slow": {
    data: { volumes: [fresh], messages, scan },
    steps: [scanStep],
  },
  "scan-running": {
    data: {
      volumes: [fresh],
      messages,
      scan: "pending",
      progress: {
        stage: "scan",
        filesDone: 1900,
        totalFiles: 4800,
        bytesDone: 1.5e10,
        totalBytes: 3.82e10,
        mbps: 520,
        etaSecs: 44,
      },
    },
    steps: [{ click: "#btn-scan", until: "#progress-card:not(.is-idle)" }],
  },
  "refresh-confirm": {
    data: { volumes: [fresh], messages, scan },
    steps: [scanStep, refreshStep],
  },
  "refresh-running": {
    data: {
      volumes: [fresh],
      messages,
      scan,
      refresh: "pending",
      progress: {
        stage: "refresh",
        filesDone: 137,
        totalFiles: 300,
        bytesDone: 1.15e9,
        totalBytes: 2.52e9,
        mbps: 180,
        etaSecs: 55,
      },
    },
    steps: [scanStep, refreshStep, confirmStep("#progress-card:not(.is-idle)")],
  },
  "refresh-partial": {
    data: {
      volumes: [fresh],
      messages,
      scan,
      refresh: {
        logPath: "G:\\synthetic-refresh.csv",
        movedFiles: 287,
        errorFiles: 13,
        stopped: false,
      },
    },
    steps: [scanStep, refreshStep, confirmStep(NOTICE)],
  },
  "refresh-skipped": {
    data: {
      volumes: [fresh],
      messages,
      scan,
      refresh: {
        logPath: "G:\\synthetic-refresh.csv",
        movedFiles: 0,
        errorFiles: 0,
        skippedFiles: 300,
        stopped: false,
      },
    },
    steps: [scanStep, refreshStep, confirmStep(NOTICE)],
  },
  "refresh-check-failed": {
    data: {
      volumes: [fresh],
      messages,
      scan,
      refresh: {
        logPath: "G:\\synthetic-refresh.csv",
        movedFiles: 1,
        errorFiles: 1,
        skippedFiles: 0,
        stopped: true,
      },
    },
    steps: [scanStep, refreshStep, confirmStep(NOTICE)],
  },
  "refresh-stopped": {
    data: {
      volumes: [fresh],
      messages,
      scan,
      refresh: {
        logPath: "G:\\synthetic-refresh.csv",
        movedFiles: 137,
        errorFiles: 0,
        stopped: true,
      },
    },
    steps: [scanStep, refreshStep, confirmStep(NOTICE)],
  },
  "scan-no-slow": {
    data: { volumes: [fresh], messages, scan: scanOf("G:\\", 0, 4800) },
    steps: [scanStep],
  },
  "verify-faster-no": {
    data: {
      volumes: [refreshedAgo(3 * DAY)],
      verify: verify({
        faster: "NO",
        ageSecs: 3 * DAY,
        speed: { beforeMbps: 25, nowMbps: 31, slowBefore: 300, slowNow: 287 },
      }),
    },
    steps: [{ click: "#btn-verify", until: NOTICE }],
  },
  "verify-yes-unchecked": {
    data: { volumes: [refreshed], verify: verify({ uncheckedFiles: 37 }) },
    steps: [{ click: "#btn-verify", until: NOTICE }],
  },
  "verify-not-decided": {
    data: {
      // The drive card must agree with ageSecs: refreshed 1 minute ago.
      volumes: [refreshedAgo(60)],
      verify: verify({
        faster: "NOT DECIDED",
        ageSecs: 60,
        waitSecs: 12 * 3600 - 60,
      }),
    },
    steps: [{ click: "#btn-verify", until: NOTICE }],
  },
  "verify-no": {
    data: {
      volumes: [refreshed],
      verify: verify({ dataSafety: "NO", mismatchedFiles: 2 }),
    },
    steps: [{ click: "#btn-verify", until: NOTICE }],
  },
  "multi-drive-failing": {
    data: {
      volumes: [
        volume("C:\\", {
          isSystem: true,
          model: "Synthetic NVMe 1TB",
          totalBytes: 1e12,
          freeBytes: 4.12e11,
        }),
        volume("D:\\", {
          model: "Synthetic SATA 2TB",
          totalBytes: 2e12,
          freeBytes: 6.55e11,
        }),
        volume("E:\\", {
          model: "Synthetic SATA 512GB",
          totalBytes: 5.12e11,
          freeBytes: 1e10,
          smart: { level: "failing", detail: "Reallocated sectors: 212" },
        }),
        volume("F:\\", {
          model: null,
          totalBytes: 2.56e11,
          freeBytes: 1.9e11,
          smart: { level: "unknown", detail: "SMART query not supported" },
        }),
      ],
      messages,
      scans: {
        "C:\\": scanOf("C:\\", 12, 210000),
        "D:\\": scanOf("D:\\", 85, 52000),
        "E:\\": scanOf("E:\\", 40, 9000),
        "F:\\": scanOf("F:\\", 0, 1200),
      },
    },
    steps: [{ click: "#btn-select-all" }, scanStep],
  },
  "standard-user": {
    data: { volumes: [fresh], elevated: false, messages, scan },
    steps: [scanStep],
  },
};

/** Open a new page at the viewport, run the scenario steps, and wait until the page is still. */
export async function openScenario(browser, url, scenario, viewport) {
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(String(error)));
  await page.setViewport(viewport);
  await installIpc(page, scenario.data);
  // Load assets, then wait for the application state instead of network silence.
  await page.goto(url, { waitUntil: "load" });
  await page.waitForFunction(
    () => document.querySelector("#mode-chip").textContent === "Ready",
  );
  for (const step of scenario.steps ?? []) {
    await page.waitForFunction(
      (selector) => !document.querySelector(selector).disabled,
      {},
      step.click,
    );
    await page.click(step.click);
    if (step.until) await page.waitForSelector(step.until, { visible: true });
  }
  await page.evaluate(() => document.fonts.ready);
  // Let the transitions finish.
  await new Promise((done) => setTimeout(done, 300));
  return { page, errors };
}
