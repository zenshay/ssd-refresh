// Browser test with synthetic IPC. This test does not access storage volumes.
import assert from "node:assert/strict";
import { once } from "node:events";
import { existsSync, readFileSync } from "node:fs";
import { createServer as createHttpServer } from "node:http";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import puppeteer from "puppeteer-core";
import { createServer } from "vite";
import viteConfig from "../vite.config.js";

const root = fileURLToPath(new URL("../", import.meta.url));
const candidates = [
  process.env.SSD_REFRESH_BROWSER,
  process.env.ProgramFiles &&
    resolve(process.env.ProgramFiles, "Google/Chrome/Application/chrome.exe"),
  process.env["ProgramFiles(x86)"] &&
    resolve(
      process.env["ProgramFiles(x86)"],
      "Microsoft/Edge/Application/msedge.exe",
    ),
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
];
const executablePath = candidates.find((path) => path && existsSync(path));
assert.ok(
  executablePath,
  "Set SSD_REFRESH_BROWSER to a Chrome or Edge executable path.",
);
// The end of the path has a character with two UTF-16 units. A bad cut breaks it.
const longSlowPath = `X:\\${"synthetic-folder\\".repeat(5)}😀-synthetic.dat`;
const vite = await createServer({
  root,
  configFile: false,
  // The plugins put the version from package.json in the page.
  plugins: viteConfig.plugins,
  server: { host: "127.0.0.1", middlewareMode: true, hmr: false },
});
const server = createHttpServer(vite.middlewares);
let browser;
try {
  await once(server.listen(0, "127.0.0.1"), "listening");
  const address = server.address();
  assert.equal(address.address, "127.0.0.1");
  assert.ok(address.port > 0);
  console.log(`TEST SERVER: ${address.address}:${address.port}`);
  browser = await puppeteer.launch({
    executablePath,
    headless: true,
    pipe: true,
    dumpio: process.env.CI === "true",
    args: ["--no-first-run"],
  });
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(String(error)));
  await page.evaluateOnNewDocument((slowPath) => {
    const callbacks = new Map();
    const listeners = new Map();
    let id = 0;
    const calls = [];
    const requests = [];
    // Synthetic clipboard. The test does not write to the system clipboard.
    const copied = [];
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        async writeText(text) {
          if (window.__mockRejectCopy) {
            throw new Error("Synthetic copy failure.");
          }
          copied.push(text);
        },
      },
    });
    const emit = (event, payload) =>
      callbacks.get(listeners.get(event))?.({ event, id: 1, payload });
    window.__TAURI_INTERNALS__ = {
      transformCallback(callback) {
        callbacks.set(++id, callback);
        return id;
      },
      unregisterCallback(callback) {
        callbacks.delete(callback);
      },
      async invoke(command, args) {
        calls.push(command);
        requests.push({ command, args });
        if (command === "plugin:event|listen") {
          listeners.set(args.event, args.handler);
          return id;
        }
        if (command === "environment")
          return { elevated: !window.__mockStandardUser };
        if (command === "list_volumes")
          return [
            {
              root: "X:\\",
              fs: "NTFS",
              totalBytes: 100000,
              freeBytes: 50000,
              isSystem: false,
              model: "Synthetic",
              smart: { level: "healthy", detail: null },
              lastRefresh: null,
            },
          ];
        if (command === "start_scan") {
          emit("operation-message", "Synthetic core message");
          emit(
            "operation-message",
            "Synthetic private data: X:\\private\\synthetic-patient.txt; synthetic-secret",
          );
          emit("progress", {
            stage: "read",
            filesDone: 1,
            totalFiles: 2,
            bytesDone: 1024,
            totalBytes: 2048,
            mbps: 12,
            etaSecs: 1,
          });
          return {
            reportPath: "synthetic.csv",
            totalFiles: 2,
            slowTotal: 1,
            fastAfterWait: 0,
            stillSlowAfterWait: 1,
            slowFiles: [
              {
                path: slowPath,
                size: 1024,
                mbps: 12,
                fastAfterWait: false,
              },
            ],
            stopped: false,
          };
        }
        if (command === "start_refresh")
          return {
            logPath: "synthetic-refresh.csv",
            movedFiles: 1,
            stopped: false,
          };
        if (command === "start_verify") {
          emit("progress", {
            stage: "verify",
            filesDone: 1,
            totalFiles: 1,
            bytesDone: 1024,
            totalBytes: 1024,
            mbps: 400,
            etaSecs: 0,
          });
          return {
            logPath: "synthetic-refresh.verify.log",
            processedFiles: 1,
            dataSafe: true,
            faster: "NOT DECIDED",
            ageSecs: 60,
            speed: { beforeMbps: 12, nowMbps: 480, slowBefore: 1, slowNow: 0 },
            waitSecs: 12 * 3600 - 60,
            stopped: false,
          };
        }
        if (command === "toggle_pause") return true;
        if (command === "open_url" && window.__mockRejectOpen) {
          throw new Error("Synthetic browser failure.");
        }
        return null;
      },
    };
    window.__mockCalls = calls;
    window.__mockRequests = requests;
    window.__mockRejectOpen = false;
    window.__mockCopied = copied;
    window.__mockRejectCopy = false;
  }, longSlowPath);
  await page.goto(`http://127.0.0.1:${address.port}`, {
    waitUntil: "networkidle0",
  });
  await page.waitForFunction(
    () => !document.querySelector("#btn-scan").disabled,
  );
  // The version chip shows the version of package.json.
  const { version } = JSON.parse(
    readFileSync(new URL("../package.json", import.meta.url), "utf8"),
  );
  assert.equal(
    await page.$eval(".version-chip", (node) => node.textContent),
    `v${version}`,
  );
  // Before a refresh, the drive has no refresh log. Verify is locked.
  assert.equal(await page.$eval("#btn-verify", (node) => node.disabled), true);
  await page.click("#btn-scan");
  await page.waitForFunction(
    () => !document.querySelector("#btn-refresh").disabled,
  );
  assert.match(
    await page.$eval("#console-log-view", (node) => node.textContent),
    /Synthetic core message/,
  );

  // Slow files card: cut the path in the middle and keep the file name.
  await page.waitForFunction(
    () => !document.querySelector("#slow-files-card").hidden,
  );
  assert.equal(
    await page.$eval("#slow-files-count", (node) => node.textContent),
    "1 slow file detected",
  );
  const cell = await page.$eval(".slow-cell-path", (node) => ({
    text: node.textContent,
    title: node.title,
  }));
  assert.equal(Array.from(cell.text).length, 50);
  assert.ok(cell.text.includes("…"));
  assert.ok(cell.text.endsWith("😀-synthetic.dat"));
  assert.ok(cell.text.isWellFormed());
  assert.equal(cell.title, longSlowPath);

  // Slow files card: copy the report path with the mouse and the keyboard.
  const reportPath = "#slow-report-path";
  const labelIs = (text) =>
    page.waitForFunction(
      (selector, expected) =>
        document.querySelector(selector).textContent === expected,
      { timeout: 5000 },
      reportPath,
      text,
    );
  const copiedCount = () => page.evaluate(() => window.__mockCopied.length);
  assert.deepEqual(
    await page.$eval(reportPath, (node) => [
      node.getAttribute("role"),
      node.tabIndex,
    ]),
    ["button", 0],
  );
  await page.click(reportPath);
  await labelIs("Copied ✓");
  assert.deepEqual(await page.evaluate(() => window.__mockCopied), [
    "synthetic.csv",
  ]);
  await labelIs("synthetic.csv");
  // A second click while "Copied" shows must not keep "Copied" on the screen.
  await page.click(reportPath);
  await labelIs("Copied ✓");
  await page.click(reportPath);
  await labelIs("synthetic.csv");
  assert.equal(await copiedCount(), 3);
  await page.focus(reportPath);
  await page.keyboard.press("Enter");
  await labelIs("Copied ✓");
  assert.equal(await copiedCount(), 4);
  await labelIs("synthetic.csv");
  await page.evaluate(() => {
    window.__mockRejectCopy = true;
  });
  await page.click(reportPath);
  await labelIs("Cannot copy the report path");
  assert.equal(await copiedCount(), 4);
  await labelIs("synthetic.csv");
  await page.evaluate(() => {
    window.__mockRejectCopy = false;
  });

  await page.click("#btn-refresh");
  await page.waitForFunction(
    () => document.querySelector("#confirm-modal").open,
  );
  await page.click("#btn-confirm-cancel");
  assert.equal(
    await page.evaluate(
      () =>
        window.__mockCalls.filter((call) => call === "start_refresh").length,
    ),
    0,
  );
  await page.click("#btn-refresh");
  await page.waitForFunction(
    () => document.querySelector("#confirm-modal").open,
  );
  await page.click("#btn-confirm-ok");
  await page.waitForFunction(
    () =>
      window.__mockCalls.filter((call) => call === "start_refresh").length ===
      1,
  );
  await page.waitForFunction(
    () => document.querySelector("#mode-chip").textContent === "Ready",
  );
  assert.equal(
    await page.evaluate(
      () =>
        window.__mockCalls.filter((call) => call === "start_refresh").length,
    ),
    1,
  );
  assert.match(
    await page.$eval("#console-log-view", (node) => node.textContent),
    /synthetic-secret/,
  );

  // Verify reads the log of the refresh that just ran.
  await page.waitForFunction(
    () => !document.querySelector("#btn-verify").disabled,
  );
  assert.match(
    await page.$eval("#btn-verify .action-hint", (node) => node.textContent),
    /Speed result after 12 hours/,
  );
  // The drive card shows the refresh. The text must fit in the card.
  const refreshedCard = await page.$eval('[data-root="X:\\\\"]', (node) => {
    const meta = node.querySelector(".drive-meta");
    return {
      text: node.querySelector(".drive-refreshed")?.textContent,
      fits: meta.scrollWidth <= meta.clientWidth,
    };
  });
  assert.deepEqual(refreshedCard, { text: "Refreshed just now", fits: true });
  // Long real values must also fit at the default window size.
  assert.equal(
    await page.$eval('[data-root="X:\\\\"]', (node) => {
      const meta = node.querySelector(".drive-meta");
      meta.firstElementChild.textContent = "123.4 GB free of 1.8 TB";
      node.querySelector(".drive-refreshed").textContent =
        "Refreshed 30 days ago";
      return meta.scrollWidth <= meta.clientWidth;
    }),
    true,
  );
  await page.click("#btn-verify");
  await page.waitForFunction(() =>
    document
      .querySelector("#notice-title")
      .textContent.includes("Data safe: YES. Faster: NOT DECIDED"),
  );
  assert.deepEqual(
    await page.evaluate(() =>
      window.__mockRequests
        .filter((request) => request.command === "start_verify")
        .map((request) => request.args),
    ),
    [{ logPath: "synthetic-refresh.csv" }],
  );
  // The notice shows one fact per line. The speed is on the first line.
  const noticeLines = await page.$eval(
    "#notice-body",
    // biome-ignore lint/performance/useDomNodeTextContent: innerText shows the line breaks that CSS renders.
    (node) => node.innerText.split("\n"),
  );
  assert.deepEqual(noticeLines.slice(0, 2), [
    "Read speed: 12 → 480 MB/s (40.0 times faster)",
    "Slow files: 1 → 0",
  ]);
  assert.equal(noticeLines.length, 3);
  assert.match(noticeLines[2], /^Next step: verify again after /);
  assert.equal(
    await page.$eval("#mode-chip", (node) => node.textContent),
    "Ready",
  );
  await page.click("#btn-report-issue");
  await page.waitForFunction(() =>
    document
      .querySelector("#console-log-view")
      .textContent.includes("Opened the GitHub issue page."),
  );
  assert.deepEqual(
    await page.evaluate(() =>
      window.__mockRequests
        .filter((request) => request.command === "open_url")
        .map((request) => request.args),
    ),
    [{ url: "https://github.com/zenshay/ssd-refresh/issues/new" }],
  );
  await page.click("#btn-clear-log");
  await page.evaluate(() => {
    window.__mockRejectOpen = true;
  });
  await page.click("#btn-report-issue");
  await page.waitForFunction(() =>
    document
      .querySelector("#console-log-view .log-error")
      ?.textContent.includes(
        "Cannot open the GitHub issue page: Error: Synthetic browser failure.",
      ),
  );
  assert.doesNotMatch(
    await page.$eval("#console-log-view", (node) => node.textContent),
    /Opened the GitHub issue page\./,
  );
  assert.deepEqual(
    await page.evaluate(() =>
      window.__mockRequests
        .filter((request) => request.command === "open_url")
        .map((request) => request.args),
    ),
    Array.from({ length: 2 }, () => ({
      url: "https://github.com/zenshay/ssd-refresh/issues/new",
    })),
  );

  // A standard user sees a notice at start, and Selective Refresh is locked after a scan.
  await page.evaluateOnNewDocument(() => {
    window.__mockStandardUser = true;
  });
  await page.reload({ waitUntil: "networkidle0" });
  await page.waitForFunction(
    () => !document.querySelector("#btn-scan").disabled,
  );
  assert.equal(
    await page.$eval("#notice-title", (node) => node.textContent),
    "Not running as administrator",
  );
  await page.click("#btn-scan");
  await page.waitForFunction(
    () => !document.querySelector("#slow-files-card").hidden,
  );
  assert.equal(await page.$eval("#btn-refresh", (node) => node.disabled), true);
  assert.match(
    await page.$eval("#btn-refresh .action-hint", (node) => node.textContent),
    /administrator/,
  );

  assert.deepEqual(errors, []);
  console.log(
    "RESULT: YES (scan, messages, slow path, copy, copy failure, cancel, confirm, refresh, verify, report privacy, report failure, standard user)",
  );
} finally {
  await browser?.close();
  await vite.close();
  if (server.listening) {
    await new Promise((resolve, reject) => {
      server.close((error) => (error ? reject(error) : resolve()));
    });
  }
}
