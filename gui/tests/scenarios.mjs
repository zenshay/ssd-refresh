// Synthetic GUI states for screens.mjs and layout.mjs. No real drive data.
import { installIpc } from "./harness.mjs";

const DAY = 86400;
const now = Math.floor(Date.now() / 1000);
const volume = (lastRefresh) => ({
  root: "G:\\",
  fs: "NTFS",
  totalBytes: 1.8e12,
  freeBytes: 1.2e11,
  isSystem: false,
  model: "Synthetic SSD",
  smart: { level: "healthy", detail: null },
  lastRefresh,
});
const refreshed = volume({
  logPath: "G:\\synthetic-refresh.csv",
  unixSecs: now - 21 * DAY,
});
const verify = (patch) => ({
  logPath: "G:\\synthetic.verify.log",
  processedFiles: 600,
  dataSafety: "YES",
  uncheckedFiles: 0,
  faster: "YES",
  ageSecs: 21 * DAY,
  speed: { beforeMbps: 25, nowMbps: 414, slowBefore: 340, slowNow: 3 },
  waitSecs: 0,
  stopped: false,
  ...patch,
});

/** Each scenario: the IPC data and the button to click. */
export const SCENARIOS = {
  idle: { data: { volumes: [volume(null)] } },
  "scan-many-slow": {
    click: "#btn-scan",
    data: {
      volumes: [volume(null)],
      messages: Array.from(
        { length: 12 },
        (_, i) => `Synthetic message ${i + 1}`,
      ),
      scan: {
        reportPath: "G:\\synthetic-scan.csv",
        totalFiles: 400,
        slowTotal: 300,
        fastAfterWait: 0,
        stillSlowAfterWait: 300,
        slowFiles: Array.from({ length: 300 }, (_, i) => ({
          path: `G:\\data\\set3\\file${String(i).padStart(3, "0")}.bin`,
          size: 8_400_000,
          mbps: 1300 - i,
          fastAfterWait: false,
        })),
        stopped: false,
      },
    },
  },
  "verify-yes-unchecked": {
    click: "#btn-verify",
    data: { volumes: [refreshed], verify: verify({ uncheckedFiles: 37 }) },
  },
  "verify-not-decided": {
    click: "#btn-verify",
    data: {
      volumes: [refreshed],
      verify: verify({
        faster: "NOT DECIDED",
        ageSecs: 60,
        waitSecs: 12 * 3600 - 60,
      }),
    },
  },
  "verify-no": {
    click: "#btn-verify",
    data: { volumes: [refreshed], verify: verify({ dataSafety: "NO" }) },
  },
};

/** Open a new page at the viewport, load the scenario, and wait until the page is still. */
export async function openScenario(browser, url, scenario, viewport) {
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(String(error)));
  await page.setViewport(viewport);
  await installIpc(page, scenario.data);
  await page.goto(url, { waitUntil: "networkidle0" });
  await page.waitForFunction(
    () => document.querySelector("#mode-chip").textContent === "Ready",
  );
  if (scenario.click) {
    await page.waitForFunction(
      (selector) => !document.querySelector(selector).disabled,
      {},
      scenario.click,
    );
    await page.click(scenario.click);
    await page.waitForFunction(
      () => !document.querySelector("#notice-card").hidden,
    );
  }
  await page.evaluate(() => document.fonts.ready);
  // Let the transitions finish.
  await new Promise((done) => setTimeout(done, 300));
  return { page, errors };
}
